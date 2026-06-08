use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::cmce::subentities::cc_bs) enum GroupEvent {
    TxDemand,
    TxCeased,
    NetworkCallStart,
    NetworkCallEnd,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::cmce::subentities::cc_bs) enum GroupTransitionError {
    UnknownCall(u16),
    InvalidTransition {
        call_id: u16,
        state: GroupCallState,
        event: GroupEvent,
    },
    NotCurrentSpeaker {
        call_id: u16,
        sender_issi: u32,
        current_speaker_issi: u32,
    },
    MissingCachedSetup(u16),
}

impl CcBsSubentity {
    fn is_preemptive_tx_demand_priority(priority: u8) -> bool {
        matches!(priority, 2 | 3)
    }

    fn validate_group_transition(call_id: u16, state: GroupCallState, event: GroupEvent) -> Result<(), GroupTransitionError> {
        let allowed = matches!(
            (state, event),
            (GroupCallState::Transmitting, GroupEvent::TxDemand)
                | (GroupCallState::NoActiveSpeaker { .. }, GroupEvent::TxDemand)
                | (GroupCallState::Transmitting, GroupEvent::TxCeased)
                | (GroupCallState::Transmitting, GroupEvent::NetworkCallStart)
                | (GroupCallState::NoActiveSpeaker { .. }, GroupEvent::NetworkCallStart)
                | (GroupCallState::Transmitting, GroupEvent::NetworkCallEnd)
                | (GroupCallState::NoActiveSpeaker { .. }, GroupEvent::NetworkCallEnd)
        );
        if allowed {
            Ok(())
        } else {
            Err(GroupTransitionError::InvalidTransition { call_id, state, event })
        }
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_send_d_tx_granted_individual(
        &self,
        queue: &mut MessageQueue,
        call_id: u16,
        target_addr: TetraAddress,
        ts: u8,
        usage: u8,
        transmission_grant: TransmissionGrant,
        _transmitting_party_issi: Option<u32>,
    ) {
        self.fsm_send_d_tx_granted_individual_inner(
            queue,
            call_id,
            target_addr,
            ts,
            usage,
            transmission_grant,
            _transmitting_party_issi,
            None,
        );
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_send_d_tx_granted_individual_reported(
        &self,
        queue: &mut MessageQueue,
        call_id: u16,
        target_addr: TetraAddress,
        ts: u8,
        usage: u8,
        transmission_grant: TransmissionGrant,
        transmitting_party_issi: Option<u32>,
    ) -> TxReporter {
        debug_assert_eq!(transmission_grant, TransmissionGrant::Granted);
        let reporter = TxReporter::new_unacked();
        self.fsm_send_d_tx_granted_individual_inner(
            queue,
            call_id,
            target_addr,
            ts,
            usage,
            transmission_grant,
            transmitting_party_issi,
            Some(reporter.clone()),
        );
        reporter
    }

    fn fsm_send_d_tx_granted_individual_inner(
        &self,
        queue: &mut MessageQueue,
        call_id: u16,
        target_addr: TetraAddress,
        ts: u8,
        usage: u8,
        transmission_grant: TransmissionGrant,
        _transmitting_party_issi: Option<u32>,
        reporter: Option<TxReporter>,
    ) {
        // EN 300 392-2 table 14.18 makes transmitting-party IEs optional.
        // Keep group floor responses compact so D-TX GRANTED fits on
        // assigned-channel FACCH/STCH rather than falling back to SCH/F while
        // the MS is already on the traffic channel. Clause 14.5.2.2.1 floor
        // state remains encoded by the mandatory transmission-grant IE.
        let d_tx_granted = DTxGranted {
            call_identifier: call_id,
            transmission_grant: transmission_grant.into_raw() as u8,
            transmission_request_permission: false,
            encryption_control: false,
            reserved: false,
            notification_indicator: None,
            transmitting_party_type_identifier: None,
            transmitting_party_address_ssi: None,
            transmitting_party_extension: None,
            external_subscriber_number: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };

        tracing::info!(
            "FSM -> D-TX GRANTED (individual, {}) call_id={} to ISSI {}",
            transmission_grant,
            call_id,
            target_addr.ssi
        );
        let mut sdu = BitBuffer::new_autoexpand(50);
        d_tx_granted.to_bitbuf(&mut sdu).expect("Failed to serialize DTxGranted");
        sdu.seek(0);

        // EN 300 392-2 clause 23.8.2.3.1 requires both CC authorization and
        // an applicable uplink traffic usage marker before an MS may transmit.
        // Only a positive floor grant therefore carries Both; queued/denied
        // responses are downlink-only assigned-channel signalling.
        let ul_dl_assigned = if transmission_grant == TransmissionGrant::Granted {
            UlDlAssignment::Both
        } else {
            UlDlAssignment::Dl
        };
        let msg = Self::build_sapmsg_stealing_ul_dl_reported_with_repetitions(
            sdu,
            target_addr,
            ts,
            Some(usage),
            ul_dl_assigned,
            reporter,
            Some(0),
        );
        queue.push_back(msg);
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_group_reassert_current_speaker_floor(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        speaker: TetraAddress,
    ) -> Result<(), GroupTransitionError> {
        if self.pending_group_releases.contains_key(&call_id) {
            tracing::debug!("FSM: ignoring floor reassert for pending group release call_id={}", call_id);
            return Ok(());
        }

        let Some(call_snapshot) = self.active_calls.get(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };

        let state = call_snapshot.state();
        Self::validate_group_transition(call_id, state, GroupEvent::TxDemand)?;
        if !call_snapshot.is_current_speaker(speaker.ssi) {
            return Err(GroupTransitionError::NotCurrentSpeaker {
                call_id,
                sender_issi: speaker.ssi,
                current_speaker_issi: call_snapshot.source_issi,
            });
        }

        let ts = call_snapshot.ts;
        let usage = call_snapshot.usage;
        let dest_ssi = call_snapshot.dest_gssi;
        let current_speaker = call_snapshot.source_issi;

        if !self.subscriber_affiliated_to_group(speaker.ssi, dest_ssi) {
            tracing::info!(
                "FSM: rejecting floor reassert call_id={} from unaffiliated ISSI {} for GSSI {}",
                call_id,
                speaker.ssi,
                dest_ssi
            );
            self.fsm_send_d_tx_granted_individual(
                queue,
                call_id,
                speaker,
                ts,
                usage,
                TransmissionGrant::NotGranted,
                Some(current_speaker),
            );
            return Ok(());
        }

        if self.fsm_group_queue_same_speaker_retake_during_tx_ceased_tail(queue, call_id, speaker, ts, usage, current_speaker)? {
            return Ok(());
        }

        let Some(cached) = self.cached_setups.get(&call_id) else {
            return Err(GroupTransitionError::MissingCachedSetup(call_id));
        };
        let dest_addr = cached.dest_addr;

        self.cancel_matching_group_tx_ceased_tail_drain(call_id, speaker.ssi, "same group speaker reasserted floor");
        self.refresh_group_cached_d_setup_speaker(call_id, speaker.ssi);

        // EN 300 392-2 clause 14.5.2.2.1 b) defines D-TX GRANTED as the
        // explicit SwMI floor response. A repeated same-GSSI U-SETUP accepted
        // as an existing-call re-entry is not treated as unsolicited floor
        // signalling; this confirms that the already-current speaker still has
        // transmit permission and refreshes the local traffic scheduler.
        let reporter = self.fsm_send_d_tx_granted_individual_reported(
            queue,
            call_id,
            speaker,
            ts,
            usage,
            TransmissionGrant::Granted,
            Some(speaker.ssi),
        );
        self.send_group_listener_d_tx_granted_facch(queue, call_id, speaker.ssi, dest_addr.ssi, ts, usage);
        self.reset_group_t310_after_floor_grant(call_id);

        self.queue_group_floor_activation(
            call_id,
            speaker.ssi,
            dest_addr.ssi,
            ts,
            reporter,
            net_brew::is_brew_gssi_routable(&self.config, dest_ssi),
        );

        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_group_on_tx_demand(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        requesting_party: TetraAddress,
        tx_demand_priority: u8,
    ) -> Result<(), GroupTransitionError> {
        if self.pending_group_releases.contains_key(&call_id) {
            tracing::debug!("FSM: ignoring U-TX DEMAND for pending group release call_id={}", call_id);
            return Ok(());
        }

        let Some(call_snapshot) = self.active_calls.get(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };

        let state = call_snapshot.state();
        Self::validate_group_transition(call_id, state, GroupEvent::TxDemand)?;

        let ts = call_snapshot.ts;
        let usage = call_snapshot.usage;
        let dest_ssi = call_snapshot.dest_gssi;
        let current_speaker = call_snapshot.source_issi;
        let local_origin = matches!(&call_snapshot.origin, CallOrigin::Local { .. });

        let requester_affiliated = self.subscriber_affiliated_to_group(requesting_party.ssi, dest_ssi);
        if !requester_affiliated {
            tracing::info!(
                "FSM: rejecting U-TX DEMAND call_id={} from unaffiliated ISSI {} for GSSI {}",
                call_id,
                requesting_party.ssi,
                dest_ssi
            );
            self.fsm_send_d_tx_granted_individual(
                queue,
                call_id,
                requesting_party,
                ts,
                usage,
                TransmissionGrant::NotGranted,
                Some(current_speaker),
            );
            return Ok(());
        }

        if self.config.config().cell.legacy_gssi_group_call
            && local_origin
            && matches!(state, GroupCallState::NoActiveSpeaker { .. })
            && current_speaker == requesting_party.ssi
        {
            // EN 300 392-2 clause 14.5.2.2.1(b) makes D-TX GRANTED the floor
            // authorization during a maintained group call. In the local legacy
            // profile, same-speaker retake on a no-speaker hangtime allocation is
            // deliberately not maintained: old Motorola MR5/MR19 class terminals
            // can acknowledge this fast grant but fail to transmit TCH/S. Release
            // the maintained call so the next PTT uses normal group setup
            // signalling rather than opening a silent over.
            tracing::info!(
                "FSM: legacy GSSI group call mode releasing hangtime call_id={} before same-speaker retake by ISSI {}",
                call_id,
                requesting_party.ssi
            );
            self.release_group_call(queue, call_id, DisconnectCause::SwmiRequestedDisconnection);
            return Ok(());
        }

        if current_speaker == requesting_party.ssi
            && self.fsm_group_queue_same_speaker_retake_during_tx_ceased_tail(
                queue,
                call_id,
                requesting_party,
                ts,
                usage,
                current_speaker,
            )?
        {
            return Ok(());
        }

        let interruption_enabled = self.config.config().cell.transmission_interruption_enabled;
        let may_preempt = matches!(state, GroupCallState::Transmitting)
            && current_speaker != requesting_party.ssi
            && interruption_enabled
            && Self::is_preemptive_tx_demand_priority(tx_demand_priority);

        if may_preempt {
            let Some(cached) = self.cached_setups.get(&call_id) else {
                return Err(GroupTransitionError::MissingCachedSetup(call_id));
            };
            let dest_addr = cached.dest_addr;

            {
                let Some(call) = self.active_calls.get_mut(&call_id) else {
                    return Err(GroupTransitionError::UnknownCall(call_id));
                };
                call.grant_floor(requesting_party.ssi, Some(requesting_party));
            }
            self.cancel_group_tx_ceased_tail_drain(call_id, "pre-emptive group floor grant");

            // EN 300 392-2 clause 14.5.2.2.1 f) and table 14.85: pre-emptive
            // U-TX DEMAND may withdraw the current speaker when SwMI supports
            // transmission interruption. This remains default-off in config.
            self.send_d_tx_interrupt_facch(
                queue,
                call_id,
                TetraAddress::new(current_speaker, SsiType::Issi),
                requesting_party.ssi,
                ts,
                usage,
                TransmissionGrant::GrantedToOtherUser,
            );
            self.send_d_tx_interrupt_facch(
                queue,
                call_id,
                dest_addr,
                requesting_party.ssi,
                ts,
                usage,
                TransmissionGrant::GrantedToOtherUser,
            );

            let reporter = self.fsm_send_d_tx_granted_individual_reported(
                queue,
                call_id,
                requesting_party,
                ts,
                usage,
                TransmissionGrant::Granted,
                Some(requesting_party.ssi),
            );
            self.send_group_listener_d_tx_granted_facch(queue, call_id, requesting_party.ssi, dest_addr.ssi, ts, usage);
            self.reset_group_t310_after_floor_grant(call_id);
            self.refresh_group_cached_d_setup_speaker(call_id, requesting_party.ssi);

            self.emit(crate::net_telemetry::TelemetryEvent::GroupCallSpeakerChanged {
                call_id,
                gssi: dest_ssi,
                speaker_issi: requesting_party.ssi,
            });

            self.queue_group_floor_activation(
                call_id,
                requesting_party.ssi,
                dest_addr.ssi,
                ts,
                reporter,
                net_brew::is_brew_gssi_routable(&self.config, dest_ssi),
            );

            return Ok(());
        }

        let Some(call) = self.active_calls.get_mut(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };
        let grant_now = matches!(state, GroupCallState::NoActiveSpeaker { .. });
        let queue_result = if grant_now {
            call.grant_floor(requesting_party.ssi, Some(requesting_party));
            None
        } else {
            Some(call.queue_tx_demand(requesting_party))
        };

        let Some(cached) = self.cached_setups.get(&call_id) else {
            return Err(GroupTransitionError::MissingCachedSetup(call_id));
        };
        let dest_addr = cached.dest_addr;

        if let Some(queue_result) = queue_result {
            match queue_result {
                TxDemandQueueResult::FromCurrentSpeaker => {
                    tracing::trace!(
                        "FSM: U-TX DEMAND call_id={} from current speaker ISSI {}, reasserting existing floor",
                        call_id,
                        requesting_party.ssi
                    );
                    self.fsm_group_reassert_current_speaker_floor(queue, call_id, requesting_party)?;
                }
                TxDemandQueueResult::Queued | TxDemandQueueResult::AlreadyQueuedBySameUser => {
                    // Non-pre-emptive: keep current speaker active, queue requester.
                    self.fsm_send_d_tx_granted_individual(
                        queue,
                        call_id,
                        requesting_party,
                        ts,
                        usage,
                        TransmissionGrant::RequestQueued,
                        Some(current_speaker),
                    );
                }
                TxDemandQueueResult::QueueBusy => {
                    self.fsm_send_d_tx_granted_individual(
                        queue,
                        call_id,
                        requesting_party,
                        ts,
                        usage,
                        TransmissionGrant::NotGranted,
                        Some(current_speaker),
                    );
                }
            }
            return Ok(());
        }

        // NoActiveSpeaker -> Transmitting transition with granted floor.
        let reporter = self.fsm_send_d_tx_granted_individual_reported(
            queue,
            call_id,
            requesting_party,
            ts,
            usage,
            TransmissionGrant::Granted,
            Some(requesting_party.ssi),
        );
        self.send_group_listener_d_tx_granted_facch(queue, call_id, requesting_party.ssi, dest_addr.ssi, ts, usage);
        self.reset_group_t310_after_floor_grant(call_id);
        self.refresh_group_cached_d_setup_speaker(call_id, requesting_party.ssi);

        // Notify dashboard that the speaker changed (hangtime -> new speaker).
        self.emit(crate::net_telemetry::TelemetryEvent::GroupCallSpeakerChanged {
            call_id,
            gssi: dest_ssi,
            speaker_issi: requesting_party.ssi,
        });

        self.queue_group_floor_activation(
            call_id,
            requesting_party.ssi,
            dest_addr.ssi,
            ts,
            reporter,
            net_brew::is_brew_gssi_routable(&self.config, dest_ssi),
        );

        Ok(())
    }

    fn fsm_group_queue_same_speaker_retake_during_tx_ceased_tail(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        requester: TetraAddress,
        ts: u8,
        usage: u8,
        current_speaker: u32,
    ) -> Result<bool, GroupTransitionError> {
        let matching_tail = self
            .pending_group_tx_ceased_tail_drains
            .get(&call_id)
            .is_some_and(|pending| pending.sender.ssi == requester.ssi && pending.ts == ts);
        if !matching_tail {
            return Ok(false);
        }

        let queue_result = {
            let Some(call) = self.active_calls.get_mut(&call_id) else {
                return Err(GroupTransitionError::UnknownCall(call_id));
            };
            call.queue_tx_demand_after_cease_tail(requester)
        };

        let transmission_grant = match queue_result {
            TxDemandQueueResult::Queued | TxDemandQueueResult::AlreadyQueuedBySameUser => TransmissionGrant::RequestQueued,
            TxDemandQueueResult::QueueBusy => TransmissionGrant::NotGranted,
            TxDemandQueueResult::FromCurrentSpeaker => TransmissionGrant::RequestQueued,
        };

        tracing::info!(
            "FSM: same-speaker group retake call_id={} ISSI {} during TX-CEASED tail drain -> {:?}; positive grant deferred until tail completes",
            call_id,
            requester.ssi,
            transmission_grant
        );

        // EN 300 392-2 clause 14.5.2.2.1 e) lets the SwMI grant a queued
        // requester after U-TX CEASED without an explicit D-TX CEASED. While
        // the previous transmission is still tail-draining, answer the fast
        // same-speaker retake as queued so the old lower-layer cease cannot
        // switch the MS U-plane off after a new positive grant.
        self.fsm_send_d_tx_granted_individual(queue, call_id, requester, ts, usage, transmission_grant, Some(current_speaker));

        Ok(true)
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_group_on_tx_ceased(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        sender: TetraAddress,
    ) -> Result<(), GroupTransitionError> {
        if self.pending_group_releases.contains_key(&call_id) {
            tracing::debug!("FSM: ignoring U-TX CEASED for pending group release call_id={}", call_id);
            return Ok(());
        }

        let Some(call) = self.active_calls.get(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };

        let state = call.state();
        let current_speaker_issi = call.source_issi;
        let is_current_speaker = call.is_current_speaker(sender.ssi);
        Self::validate_group_transition(call_id, state, GroupEvent::TxCeased)?;

        if !is_current_speaker {
            if let Some(call) = self.active_calls.get_mut(&call_id)
                && call.clear_queued_tx_demand_from(sender.ssi)
            {
                // EN 300 392-2 clause 14.5.2.2.1 a) allows an MS with a
                // queued request-to-transmit to withdraw it using U-TX CEASED.
                // No CC protocol response is expected from the SwMI.
                tracing::info!(
                    "FSM: U-TX CEASED call_id={} from queued group requester ISSI {} withdrew pending floor request",
                    call_id,
                    sender.ssi
                );
                return Ok(());
            }
            return Err(GroupTransitionError::NotCurrentSpeaker {
                call_id,
                sender_issi: sender.ssi,
                current_speaker_issi,
            });
        }

        let Some(call) = self.active_calls.get(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };
        let ts = call.ts;
        let usage = call.usage;
        let dest_ssi = call.dest_gssi;
        let queued_request = self.first_affiliated_group_floor_requester(call_id, call, "U-TX CEASED");

        let Some(call) = self.active_calls.get_mut(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };
        let queued_request = call.take_queued_tx_demand_through(queued_request.map(|requester| requester.ssi));
        if let Some(requester) = queued_request {
            // Transmitting -> Transmitting, hand over floor directly to queued requester.
            call.grant_floor(requester.ssi, Some(requester));
        }

        let Some(cached) = self.cached_setups.get(&call_id) else {
            return Err(GroupTransitionError::MissingCachedSetup(call_id));
        };
        let dest_addr = cached.dest_addr;

        if let Some(requester) = queued_request {
            let reporter = self.fsm_send_d_tx_granted_individual_reported(
                queue,
                call_id,
                requester,
                ts,
                usage,
                TransmissionGrant::Granted,
                Some(requester.ssi),
            );
            self.send_group_listener_d_tx_granted_facch(queue, call_id, requester.ssi, dest_addr.ssi, ts, usage);
            self.reset_group_t310_after_floor_grant(call_id);
            self.refresh_group_cached_d_setup_speaker(call_id, requester.ssi);

            // Notify dashboard that the queued speaker got the floor.
            self.emit(crate::net_telemetry::TelemetryEvent::GroupCallSpeakerChanged {
                call_id,
                gssi: dest_ssi,
                speaker_issi: requester.ssi,
            });

            self.queue_group_floor_activation(
                call_id,
                requester.ssi,
                dest_addr.ssi,
                ts,
                reporter,
                net_brew::is_brew_gssi_routable(&self.config, dest_ssi),
            );
            return Ok(());
        }

        // EN 300 392-2 clauses 14.5.2.2.1 e) and 23.8.2.2 require the
        // SwMI to end the transmission, but clause 23.8.5 keeps bearer tail
        // ordering intact. Delay D-TX CEASED/FloorReleased very briefly so
        // UMAC can flush a deferred TCH/S half-slot before hangtime purges it.
        self.begin_group_tx_ceased_tail_drain(
            call_id,
            sender,
            dest_addr.ssi,
            ts,
            usage,
            net_brew::is_brew_gssi_routable(&self.config, dest_ssi),
        );

        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_group_on_current_speaker_departed(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        speaker_issi: u32,
    ) -> Result<(), GroupTransitionError> {
        if self.pending_group_releases.contains_key(&call_id) {
            tracing::debug!("FSM: ignoring departed group speaker for pending group release call_id={}", call_id);
            return Ok(());
        }

        let Some(call) = self.active_calls.get(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };

        let state = call.state();
        Self::validate_group_transition(call_id, state, GroupEvent::TxCeased)?;
        if !call.is_current_speaker(speaker_issi) {
            return Ok(());
        }

        let ts = call.ts;
        let usage = call.usage;
        let dest_ssi = call.dest_gssi;

        let Some(call) = self.active_calls.get_mut(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };
        call.clear_all_queued_tx_demands();
        call.enter_hangtime(self.dltime);

        let Some(cached) = self.cached_setups.get(&call_id) else {
            return Err(GroupTransitionError::MissingCachedSetup(call_id));
        };
        let dest_addr = cached.dest_addr;

        // EN 300 392-2 clause 14.5.2.2.1 says the SwMI fully controls which MS
        // may transmit. If the current speaker leaves the group, withdraw the
        // active permission and make remaining listeners receive-only again.
        self.send_d_tx_ceased_facch(queue, call_id, dest_addr.ssi, ts, usage);

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id, ts }),
        });

        if net_brew::is_brew_gssi_routable(&self.config, dest_ssi) {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id, ts }),
            });
        }

        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_group_on_network_call_start(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        brew_uuid: uuid::Uuid,
        source_issi: u32,
        priority: u8,
    ) -> Result<(), GroupTransitionError> {
        if self.pending_group_releases.contains_key(&call_id) {
            tracing::debug!(
                "FSM: ignoring network speaker change for pending group release call_id={} source={}",
                call_id,
                source_issi
            );
            return Ok(());
        }

        let current_speaker_is_local = self
            .active_calls
            .get(&call_id)
            .is_some_and(|call| call.tx_active && self.subscriber_affiliated_to_group(call.source_issi, call.dest_gssi));

        self.cancel_group_tx_ceased_tail_drain(call_id, "network group speaker took floor");

        let (preempted_local_speaker, ts, usage, dest_gssi) = {
            let Some(call) = self.active_calls.get_mut(&call_id) else {
                return Err(GroupTransitionError::UnknownCall(call_id));
            };

            let state = call.state();
            Self::validate_group_transition(call_id, state, GroupEvent::NetworkCallStart)?;

            let preempted_local_speaker = if current_speaker_is_local && call.source_issi != source_issi {
                Some(TetraAddress::new(call.source_issi, SsiType::Issi))
            } else {
                None
            };

            call.grant_floor(source_issi, None);
            call.priority = priority;
            call.brew_uuid = Some(brew_uuid);
            if matches!(&call.origin, CallOrigin::Network { brew_uuid: old_uuid } if *old_uuid != brew_uuid) {
                // Each new speaker from Brew arrives with a fresh UUID — expected behavior.
                tracing::debug!("CMCE FSM: network call speaker change updated brew_uuid call_id={}", call_id);
            }
            call.origin = CallOrigin::Network { brew_uuid };

            (preempted_local_speaker, call.ts, call.usage, call.dest_gssi)
        };

        if let Some(speaker) = preempted_local_speaker {
            // EN 300 392-2 clause 14.5.2.2.1 f) requires D-TX INTERRUPT to
            // the MS that currently has transmit permission. Table 14.19
            // carries the new transmitting party when the floor moves now.
            self.send_d_tx_interrupt_facch(
                queue,
                call_id,
                speaker,
                source_issi,
                ts,
                usage,
                TransmissionGrant::GrantedToOtherUser,
            );
            self.send_d_tx_interrupt_facch(
                queue,
                call_id,
                TetraAddress::new(dest_gssi, SsiType::Gssi),
                source_issi,
                ts,
                usage,
                TransmissionGrant::GrantedToOtherUser,
            );
        }

        self.send_group_listener_d_tx_granted_facch(queue, call_id, source_issi, dest_gssi, ts, usage);
        self.reset_group_t310_after_floor_grant(call_id);
        self.refresh_group_cached_d_setup_speaker(call_id, source_issi);

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id,
                source_issi,
                dest_gssi,
                ts,
            }),
        });

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Brew,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid,
                call_id,
                ts,
                usage,
            }),
        });

        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_group_on_network_call_end(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
    ) -> Result<(), GroupTransitionError> {
        if self.pending_group_releases.contains_key(&call_id) {
            tracing::debug!("FSM: ignoring network call end for pending group release call_id={}", call_id);
            return Ok(());
        }

        let Some(call) = self.active_calls.get(&call_id).cloned() else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };

        let state = call.state();
        Self::validate_group_transition(call_id, state, GroupEvent::NetworkCallEnd)?;

        if matches!(state, GroupCallState::Transmitting) {
            if let Some(active_call) = self.active_calls.get_mut(&call_id) {
                active_call.enter_hangtime(self.dltime);
                active_call.brew_uuid = None;
            }

            self.send_d_tx_ceased_facch(queue, call_id, call.dest_gssi, call.ts, call.usage);
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id, ts: call.ts }),
            });
            return Ok(());
        }

        self.release_group_call(queue, call_id, DisconnectCause::SwmiRequestedDisconnection);
        Ok(())
    }
}
