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

    fn fsm_send_d_tx_granted_individual(
        &self,
        queue: &mut MessageQueue,
        call_id: u16,
        target_addr: TetraAddress,
        ts: u8,
        transmission_grant: TransmissionGrant,
        transmitting_party_issi: Option<u32>,
    ) {
        let d_tx_granted = DTxGranted {
            call_identifier: call_id,
            transmission_grant: transmission_grant.into_raw() as u8,
            transmission_request_permission: false,
            encryption_control: false,
            reserved: false,
            notification_indicator: None,
            transmitting_party_type_identifier: transmitting_party_issi.map(|_| 1), // SSI
            transmitting_party_address_ssi: transmitting_party_issi.map(|ssi| ssi as u64),
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

        let msg = Self::build_sapmsg_stealing(sdu, target_addr, ts, None);
        queue.push_back(msg);
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
        let dest_ssi = call_snapshot.dest_gssi;
        let current_speaker = call_snapshot.source_issi;

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
                TransmissionGrant::NotGranted,
                Some(current_speaker),
            );
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
                call.reset_timeout(self.dltime);
            }

            // EN 300 392-2 clause 14.5.2.2.1 f) and table 14.85: pre-emptive
            // U-TX DEMAND may withdraw the current speaker when SwMI supports
            // transmission interruption. This remains default-off in config.
            self.send_d_tx_interrupt_facch(
                queue,
                call_id,
                TetraAddress::new(current_speaker, SsiType::Issi),
                requesting_party.ssi,
                ts,
                TransmissionGrant::GrantedToOtherUser,
            );
            self.send_d_tx_interrupt_facch(
                queue,
                call_id,
                dest_addr,
                requesting_party.ssi,
                ts,
                TransmissionGrant::GrantedToOtherUser,
            );

            self.fsm_send_d_tx_granted_individual(
                queue,
                call_id,
                requesting_party,
                ts,
                TransmissionGrant::Granted,
                Some(requesting_party.ssi),
            );
            self.send_d_tx_granted_facch(queue, call_id, requesting_party.ssi, dest_addr.ssi, ts);

            self.emit(crate::net_telemetry::TelemetryEvent::GroupCallSpeakerChanged {
                call_id,
                gssi: dest_ssi,
                speaker_issi: requesting_party.ssi,
            });

            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                    call_id,
                    source_issi: requesting_party.ssi,
                    dest_gssi: dest_addr.ssi,
                    ts,
                }),
            });

            if net_brew::is_brew_gssi_routable(&self.config, dest_ssi) {
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Brew,
                    msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                        call_id,
                        source_issi: requesting_party.ssi,
                        dest_gssi: dest_addr.ssi,
                        ts,
                    }),
                });
            }

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
                        "FSM: U-TX DEMAND call_id={} from current speaker ISSI {}, ignoring duplicate",
                        call_id,
                        requesting_party.ssi
                    );
                }
                TxDemandQueueResult::Queued | TxDemandQueueResult::AlreadyQueuedBySameUser => {
                    // Non-pre-emptive: keep current speaker active, queue requester.
                    self.fsm_send_d_tx_granted_individual(
                        queue,
                        call_id,
                        requesting_party,
                        ts,
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
                        TransmissionGrant::NotGranted,
                        Some(current_speaker),
                    );
                }
            }
            return Ok(());
        }

        // NoActiveSpeaker -> Transmitting transition with granted floor.
        self.fsm_send_d_tx_granted_individual(
            queue,
            call_id,
            requesting_party,
            ts,
            TransmissionGrant::Granted,
            Some(requesting_party.ssi),
        );
        self.send_d_tx_granted_facch(queue, call_id, requesting_party.ssi, dest_addr.ssi, ts);

        // Notify dashboard that the speaker changed (hangtime -> new speaker).
        self.emit(crate::net_telemetry::TelemetryEvent::GroupCallSpeakerChanged {
            call_id,
            gssi: dest_ssi,
            speaker_issi: requesting_party.ssi,
        });

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id,
                source_issi: requesting_party.ssi,
                dest_gssi: dest_addr.ssi,
                ts,
            }),
        });

        if net_brew::is_brew_gssi_routable(&self.config, dest_ssi) {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                    call_id,
                    source_issi: requesting_party.ssi,
                    dest_gssi: dest_addr.ssi,
                    ts,
                }),
            });
        }

        Ok(())
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
        Self::validate_group_transition(call_id, state, GroupEvent::TxCeased)?;

        if !call.is_current_speaker(sender.ssi) {
            return Err(GroupTransitionError::NotCurrentSpeaker {
                call_id,
                sender_issi: sender.ssi,
                current_speaker_issi: call.source_issi,
            });
        }

        let ts = call.ts;
        let dest_ssi = call.dest_gssi;
        let queued_request = call.queued_tx_demand;
        let queued_request = queued_request.filter(|requester| {
            let affiliated = self.subscriber_affiliated_to_group(requester.ssi, dest_ssi);
            if !affiliated {
                tracing::info!(
                    "FSM: dropping queued group floor requester ISSI {} for call_id={} gssi={} after affiliation loss",
                    requester.ssi,
                    call_id,
                    dest_ssi
                );
            }
            affiliated
        });

        let Some(call) = self.active_calls.get_mut(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };
        let _ = call.take_queued_tx_demand();
        if let Some(requester) = queued_request {
            // Transmitting -> Transmitting, hand over floor directly to queued requester.
            call.grant_floor(requester.ssi, Some(requester));
        } else {
            // Transmitting -> NoActiveSpeaker.
            call.enter_hangtime(self.dltime);
        }

        let Some(cached) = self.cached_setups.get(&call_id) else {
            return Err(GroupTransitionError::MissingCachedSetup(call_id));
        };
        let dest_addr = cached.dest_addr;

        if let Some(requester) = queued_request {
            self.fsm_send_d_tx_granted_individual(queue, call_id, requester, ts, TransmissionGrant::Granted, Some(requester.ssi));
            self.send_d_tx_granted_facch(queue, call_id, requester.ssi, dest_addr.ssi, ts);

            // Notify dashboard that the queued speaker got the floor.
            self.emit(crate::net_telemetry::TelemetryEvent::GroupCallSpeakerChanged {
                call_id,
                gssi: dest_ssi,
                speaker_issi: requester.ssi,
            });

            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                    call_id,
                    source_issi: requester.ssi,
                    dest_gssi: dest_addr.ssi,
                    ts,
                }),
            });

            if net_brew::is_brew_gssi_routable(&self.config, dest_ssi) {
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Brew,
                    msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                        call_id,
                        source_issi: requester.ssi,
                        dest_gssi: dest_addr.ssi,
                        ts,
                    }),
                });
            }
            return Ok(());
        }

        self.send_d_tx_ceased_facch(queue, call_id, dest_addr.ssi, ts);

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
        let dest_ssi = call.dest_gssi;

        let Some(call) = self.active_calls.get_mut(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };
        let _ = call.take_queued_tx_demand();
        call.enter_hangtime(self.dltime);

        let Some(cached) = self.cached_setups.get(&call_id) else {
            return Err(GroupTransitionError::MissingCachedSetup(call_id));
        };
        let dest_addr = cached.dest_addr;

        // EN 300 392-2 clause 14.5.2.2.1 says the SwMI fully controls which MS
        // may transmit. If the current speaker leaves the group, withdraw the
        // active permission and make remaining listeners receive-only again.
        self.send_d_tx_ceased_facch(queue, call_id, dest_addr.ssi, ts);

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
            call.reset_timeout(self.dltime);
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
            self.send_d_tx_interrupt_facch(queue, call_id, speaker, source_issi, ts, TransmissionGrant::GrantedToOtherUser);
            self.send_d_tx_interrupt_facch(
                queue,
                call_id,
                TetraAddress::new(dest_gssi, SsiType::Gssi),
                source_issi,
                ts,
                TransmissionGrant::GrantedToOtherUser,
            );
        }

        self.send_d_tx_granted_facch(queue, call_id, source_issi, dest_gssi, ts);

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

            self.send_d_tx_ceased_facch(queue, call_id, call.dest_gssi, call.ts);
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
