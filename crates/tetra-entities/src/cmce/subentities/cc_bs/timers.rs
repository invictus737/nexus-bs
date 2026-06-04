use super::*;
use crate::net_telemetry::TelemetryEvent;

// Local cleanup guard: EN 300 392-2 14.7.1.6 expects U-RELEASE after
// D-DISCONNECT, but the BS must eventually free a circuit if the peer vanishes.
const INDIVIDUAL_DISCONNECT_PENDING_TIMEOUT_TIMESLOTS: i32 = 16;

impl CcBsSubentity {
    pub fn tick_start_with_events(&mut self, queue: &mut MessageQueue, dltime: TdmaTime) -> Vec<TelemetryEvent> {
        // Snapshot before tick so we can detect changes
        let calls_before: std::collections::HashSet<u16> = self.active_calls.keys().copied().collect();
        let ind_before: std::collections::HashSet<u16> = self.individual_calls.keys().copied().collect();

        self.tick_start(queue, dltime);

        // Emit events for ended calls
        let mut events = Vec::new();
        for id in calls_before.iter() {
            if !self.active_calls.contains_key(id) {
                events.push(TelemetryEvent::GroupCallEnded { call_id: *id, gssi: 0 });
            }
        }
        for id in ind_before.iter() {
            if !self.individual_calls.contains_key(id) {
                events.push(TelemetryEvent::IndividualCallEnded { call_id: *id });
            }
        }
        events
    }

    pub fn tick_start(&mut self, queue: &mut MessageQueue, dltime: TdmaTime) {
        self.dltime = dltime;
        self.drain_pending_group_releases(queue);
        self.drain_pending_individual_releases(queue);
        self.drain_pending_individual_disconnect_deliveries(queue);
        self.check_individual_disconnect_pending_timeout(queue);

        // ETSI T310 equivalent for active calls.
        self.check_call_timeout_expiry(queue);
        // ETSI T301/T302 equivalent while waiting for call completion.
        self.check_individual_setup_timeout(queue);
        // Check hangtime expiry for active local calls
        self.check_hangtime_expiry(queue);

        if let Some(tasks) = self.circuits.tick_start(dltime) {
            for task in tasks {
                match task {
                    CircuitMgrCmd::SendDSetup(call_id, usage, ts) => {
                        if self.pending_group_releases.contains_key(&call_id) {
                            tracing::debug!("CMCE: suppressing D-SETUP resend for pending group release call_id={}", call_id);
                            continue;
                        }
                        if self.has_pending_individual_release(call_id) {
                            tracing::debug!(
                                "CMCE: suppressing D-SETUP resend for pending individual release call_id={}",
                                call_id
                            );
                            continue;
                        }
                        let individual_setup_retry_blocked = self
                            .individual_calls
                            .get(&call_id)
                            .is_some_and(|call| call.state != IndividualCallState::CallSetupPending);
                        // Get our cached D-SETUP, build a prim and send it down the stack
                        let Some(cached) = self.cached_setups.get_mut(&call_id) else {
                            tracing::trace!(
                                "CMCE: skipping D-SETUP resend for call_id={} (no cached D-SETUP; likely Brew-routed individual call)",
                                call_id
                            );
                            continue;
                        };
                        if !cached.resend {
                            continue;
                        }
                        if cached.is_individual && individual_setup_retry_blocked {
                            tracing::debug!(
                                "CMCE: suppressing D-SETUP resend for individual call_id={} outside setup-pending state",
                                call_id
                            );
                            continue;
                        }
                        if let Some(reporter) = &cached.last_resend_reporter
                            && !reporter.is_in_final_state()
                        {
                            tracing::debug!(
                                "CMCE: suppressing D-SETUP resend for call_id={} while prior resend is {:?}",
                                call_id,
                                reporter.get_state()
                            );
                            continue;
                        }
                        // Late-entry D-SETUP keeps listeners attached to an established group call.
                        // During hangtime there is no current speaker, but sending NotGranted makes
                        // some terminals treat PTT as denied. Keep them in listener state and allow
                        // floor requests via D-TX-CEASED/TRP=0.
                        if self.active_calls.contains_key(&call_id) {
                            cached.pdu.transmission_grant = TransmissionGrant::GrantedToOtherUser;
                            cached.pdu.transmission_request_permission = false;
                        }
                        let dest_addr = cached.dest_addr;
                        let is_individual = cached.is_individual;
                        let reporter = TxReporter::new_unacked();
                        cached.last_resend_reporter = Some(reporter.clone());
                        if is_individual {
                            // P2P individual call in setup phase: resend DSetup on MCCH
                            // (no chan_alloc, no circuit open yet). The called MS may be
                            // sleeping (EE) and will receive it at its next monitoring window.
                            let mut sdu = BitBuffer::new_autoexpand(80);
                            cached.pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DSetup");
                            sdu.seek(0);
                            let prim = Self::build_sapmsg(sdu, None, dest_addr, Layer2Service::Unacknowledged, Some(reporter));
                            queue.push_back(prim);
                        } else {
                            let (sdu, chan_alloc) = Self::build_d_setup_prim(&cached.pdu, usage, ts, UlDlAssignment::Both);
                            let prim = Self::build_sapmsg(sdu, Some(chan_alloc), dest_addr, Layer2Service::Unacknowledged, Some(reporter));
                            queue.push_back(prim);
                        }
                    }

                    CircuitMgrCmd::SendClose(call_id, circuit) => {
                        if self.pending_group_releases.contains_key(&call_id) || self.has_pending_individual_release(call_id) {
                            tracing::debug!(
                                "CMCE: suppressing stale circuit close call_id={} ts={} while release is already pending",
                                call_id,
                                circuit.ts
                            );
                            continue;
                        }

                        if self.active_calls.contains_key(&call_id) {
                            tracing::warn!(
                                "CMCE: stale group circuit call_id={} ts={} entering pending D-RELEASE drain",
                                call_id,
                                circuit.ts
                            );
                            self.begin_group_release(queue, call_id, DisconnectCause::ExpiryOfTimer, Some(circuit));
                            continue;
                        }

                        if self.individual_calls.contains_key(&call_id) {
                            tracing::warn!(
                                "CMCE: stale individual circuit call_id={} ts={} entering pending D-RELEASE drain",
                                call_id,
                                circuit.ts
                            );
                            // EN 300 392-2 clause 14.5.1.3.2 releases an
                            // established individual call by sending D-RELEASE
                            // before the traffic circuit is released. The
                            // CircuitMgr expiry has already removed this
                            // circuit from its table, so carry it through the
                            // pending release and notify UMAC only after
                            // FACCH/STCH D-RELEASE is reported or the local
                            // guard expires.
                            self.begin_individual_release(queue, call_id, DisconnectCause::ExpiryOfTimer, vec![circuit], true);
                            continue;
                        }

                        tracing::warn!(
                            "CMCE: closing stale circuit call_id={} ts={} with no active CMCE call state",
                            call_id,
                            circuit.ts
                        );
                        let ts = circuit.ts;
                        Self::signal_umac_circuit_close(queue, circuit);
                        self.release_timeslot(ts);
                    }
                }
            }
        }
    }

    /// Release active calls when their configured call timeout expires.
    pub(super) fn check_call_timeout_expiry(&mut self, queue: &mut MessageQueue) {
        let expired_group_calls: Vec<u16> = self
            .active_calls
            .iter()
            .filter_map(|(&call_id, call)| {
                if self.pending_group_releases.contains_key(&call_id) {
                    return None;
                }
                call.call_timeout_expired(self.dltime).then_some(call_id)
            })
            .collect();

        for call_id in expired_group_calls {
            tracing::info!("Call timeout expired for group call_id={}, releasing", call_id);
            // EN 300 392-2 clause 14.5.2.3.5: T310 expiry reports
            // disconnect cause "expiry of timer", not user release.
            self.release_group_call(queue, call_id, DisconnectCause::ExpiryOfTimer);
        }

        let expired_individual_calls: Vec<u16> = self
            .individual_calls
            .iter()
            .filter_map(|(&call_id, call)| call.active_timeout_expired(self.dltime).then_some(call_id))
            .collect();

        for call_id in expired_individual_calls {
            tracing::info!("Call timeout expired for individual call_id={}, releasing", call_id);
            self.release_individual_call(queue, call_id, DisconnectCause::ExpiryOfTimer);
        }
    }

    /// Release active P2P calls waiting for the peer U-RELEASE response to D-DISCONNECT.
    pub(super) fn check_individual_disconnect_pending_timeout(&mut self, queue: &mut MessageQueue) {
        let expired_individual_calls: Vec<(u16, DisconnectCause)> = self
            .individual_calls
            .iter()
            .filter_map(|(&call_id, call)| {
                call.pending_disconnect_timeout_expired(self.dltime, INDIVIDUAL_DISCONNECT_PENDING_TIMEOUT_TIMESLOTS)
                    .map(|cause| (call_id, cause))
            })
            .collect();

        for (call_id, cause) in expired_individual_calls {
            tracing::warn!(
                "Pending individual D-DISCONNECT timed out for call_id={}, releasing circuit",
                call_id
            );
            self.release_individual_call(queue, call_id, cause);
        }
    }

    /// Release individual setup attempts that exceed setup timeout.
    pub(super) fn check_individual_setup_timeout(&mut self, queue: &mut MessageQueue) {
        let expired_setup_calls: Vec<u16> = self
            .individual_calls
            .iter()
            .filter_map(|(&call_id, call)| call.setup_timeout_expired(self.dltime).then_some(call_id))
            .collect();

        for call_id in expired_setup_calls {
            tracing::info!("Setup timeout expired for individual call_id={}, releasing", call_id);
            self.release_individual_call(queue, call_id, DisconnectCause::ExpiryOfTimer);
        }

        // EE DSetup retry: for P2P individual calls still in CallSetupPending state
        // (called MS has not yet sent U-ALERT), periodically retransmit DSetup on MCCH
        // so that a sleeping MS can receive it at its next monitoring window.
        // EN 300 392-2 clauses 14.5.1.3.4 and 14.8.17 bound the setup phase
        // by T301/T302. The repeated D-SETUP is a local EE reachability guard
        // inside that setup window: retry once after a full multiframe, then
        // at a restrained 10 s cadence.
        const TETRA_TIMESLOTS_PER_SECOND: i32 = 18 * 4;
        const DSETUP_FIRST_RETRY_AFTER_TIMESLOTS: i32 = TETRA_TIMESLOTS_PER_SECOND;
        const DSETUP_RETRY_INTERVAL_TIMESLOTS: i32 = 10 * TETRA_TIMESLOTS_PER_SECOND;
        let retry_calls: Vec<u16> = self
            .individual_calls
            .iter()
            .filter_map(|(&call_id, call)| {
                if call.state != IndividualCallState::CallSetupPending {
                    return None;
                }
                let Some(started) = call.setup_timer_started else {
                    return None;
                };
                let age_timeslots = started.age(self.dltime);
                if age_timeslots >= DSETUP_FIRST_RETRY_AFTER_TIMESLOTS
                    && (age_timeslots - DSETUP_FIRST_RETRY_AFTER_TIMESLOTS) % DSETUP_RETRY_INTERVAL_TIMESLOTS == 0
                {
                    Some(call_id)
                } else {
                    None
                }
            })
            .collect();

        for call_id in retry_calls {
            let Some(cached) = self.cached_setups.get_mut(&call_id) else {
                continue;
            };
            if !cached.is_individual {
                continue;
            }
            if let Some(reporter) = &cached.last_resend_reporter
                && !reporter.is_in_final_state()
            {
                tracing::debug!(
                    "CMCE: suppressing EE D-SETUP retry for call_id={} while prior retry is {:?}",
                    call_id,
                    reporter.get_state()
                );
                continue;
            }
            let mut sdu = BitBuffer::new_autoexpand(80);
            if cached.pdu.to_bitbuf(&mut sdu).is_err() {
                continue;
            }
            sdu.seek(0);
            let dest_addr = cached.dest_addr;
            let reporter = TxReporter::new_unacked();
            cached.last_resend_reporter = Some(reporter.clone());
            let prim = Self::build_sapmsg(sdu, None, dest_addr, Layer2Service::Unacknowledged, Some(reporter));
            tracing::debug!(
                "EE DSetup retry for call_id={} to ISSI {} (setup pending, MS may be sleeping)",
                call_id,
                dest_addr.ssi
            );
            queue.push_back(prim);
        }
    }

    /// Check if any active calls in NoActiveSpeaker (hangtime) have expired and release them.
    pub(super) fn check_hangtime_expiry(&mut self, queue: &mut MessageQueue) {
        // Hangtime in TDMA timeslots: hangtime_secs * frames_per_sec * timeslots_per_frame
        // TETRA: 18 frames/multiframe, 4 timeslots/frame → 72 timeslots/second
        let hangtime_secs = self.config.config().cell.hangtime_secs as i32;
        let hangtime_frames: i32 = hangtime_secs * 18 * 4;

        let expired: Vec<u16> = self
            .active_calls
            .iter()
            .filter_map(|(&call_id, call)| match call.state() {
                GroupCallState::NoActiveSpeaker { since }
                    if !self.pending_group_releases.contains_key(&call_id) && since.age(self.dltime) > hangtime_frames =>
                {
                    Some(call_id)
                }
                _ => None,
            })
            .collect();

        for call_id in expired {
            tracing::info!("Hangtime expired for call_id={}, releasing", call_id);
            self.release_group_call(queue, call_id, DisconnectCause::ExpiryOfTimer);
        }
    }

    /// Handle UL inactivity timeout: force TX ceased for the transmitting MS on the given timeslot.
    /// Called when UMAC detects no voice frames on a traffic channel (UL side) for the timeout period.
    /// Corresponds to BS-side T323 expiry (ETSI EN 300 392-2 §14.9.2).
    pub(super) fn handle_ul_inactivity_timeout(&mut self, queue: &mut MessageQueue, ts: u8) {
        // Check individual (P2P simplex) calls first — they were not checked before,
        // causing UL inactivity to silently drop frames without forcing TX-CEASED on the radio.
        let individual_call_id = self
            .individual_calls
            .iter()
            .find(|(_, call)| {
                call.is_active() && !call.simplex_duplex && call.floor_holder.is_some() && {
                    // Only trigger if the inactivity is on the floor holder's TS,
                    // not on the listening party's TS (which is expected to be silent).
                    let holder_ssi = call.floor_holder.unwrap();
                    let holder_ts = if holder_ssi == call.calling_addr.ssi {
                        call.calling_ts
                    } else {
                        call.called_ts
                    };
                    holder_ts == ts
                }
            })
            .map(|(id, _)| *id);

        if let Some(call_id) = individual_call_id {
            let (
                holder_ssi,
                holder_addr,
                holder_ts,
                holder_usage,
                peer_addr,
                peer_ts,
                peer_usage,
                queued_requester,
                called_over_brew,
                calling_over_brew,
                brew_uuid,
            ) = {
                let call = self.individual_calls.get_mut(&call_id).unwrap();
                let Some(holder_ssi) = call.floor_holder else {
                    return;
                };

                let (holder_addr, holder_ts, holder_usage, peer_addr, peer_ts, peer_usage) = if holder_ssi == call.calling_addr.ssi {
                    (
                        call.calling_addr,
                        call.calling_ts,
                        call.calling_usage,
                        call.called_addr,
                        call.called_ts,
                        call.called_usage,
                    )
                } else {
                    (
                        call.called_addr,
                        call.called_ts,
                        call.called_usage,
                        call.calling_addr,
                        call.calling_ts,
                        call.calling_usage,
                    )
                };

                call.floor_holder = None;
                let queued_requester = call.take_queued_tx_demand();

                (
                    holder_ssi,
                    holder_addr,
                    holder_ts,
                    holder_usage,
                    peer_addr,
                    peer_ts,
                    peer_usage,
                    queued_requester,
                    call.called_over_brew,
                    call.calling_over_brew,
                    call.brew_uuid,
                )
            };

            if let Some(requester) = queued_requester {
                let (requester_addr, requester_ts, requester_usage, listener_addr, listener_ts, listener_usage) =
                    if requester.ssi == peer_addr.ssi {
                        (peer_addr, peer_ts, peer_usage, holder_addr, holder_ts, holder_usage)
                    } else {
                        (holder_addr, holder_ts, holder_usage, peer_addr, peer_ts, peer_usage)
                    };

                tracing::warn!(
                    "UL inactivity timeout on ts={} for individual call_id={}, forcing floor release from ISSI {} and granting queued requester ISSI {}",
                    ts,
                    call_id,
                    holder_ssi,
                    requester_addr.ssi
                );

                if let Some(call) = self.individual_calls.get_mut(&call_id) {
                    call.floor_holder = Some(requester_addr.ssi);
                }

                // EN 300 392-2 clause 14.5.1.2.1 b/e): D-TX GRANTED is a
                // response to a request-to-transmit. If a request was queued
                // when the current speaker ceased, SwMI may hand over with
                // D-TX GRANTED to both MSs and without a separate D-TX CEASED.
                // Keep channel allocation Both (clause 21.5.2); the grant IE
                // and UMAC FloorGranted state decide who is allowed to talk.
                Self::push_individual_d_tx_granted(
                    queue,
                    call_id,
                    requester_addr,
                    requester_ts,
                    requester_usage,
                    UlDlAssignment::Both,
                    TransmissionGrant::Granted,
                    requester_addr.ssi,
                );
                Self::push_individual_d_tx_granted(
                    queue,
                    call_id,
                    listener_addr,
                    listener_ts,
                    listener_usage,
                    UlDlAssignment::Both,
                    TransmissionGrant::GrantedToOtherUser,
                    requester_addr.ssi,
                );

                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Umac,
                    msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                        call_id,
                        source_issi: requester_addr.ssi,
                        dest_gssi: listener_addr.ssi,
                        ts: requester_ts,
                    }),
                });

                if (called_over_brew || calling_over_brew)
                    && let Some(brew_uuid) = brew_uuid
                {
                    queue.push_back(SapMsg {
                        sap: Sap::Control,
                        src: TetraEntity::Cmce,
                        dest: TetraEntity::Brew,
                        msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                            call_id,
                            source_issi: requester_addr.ssi,
                            dest_gssi: listener_addr.ssi,
                            ts: requester_ts,
                        }),
                    });
                    let _ = brew_uuid;
                }

                return;
            }

            tracing::warn!(
                "UL inactivity timeout on ts={} for individual call_id={}, forcing TX-CEASED on ISSI {} and notifying peer without unsolicited grant",
                ts,
                call_id,
                holder_ssi
            );

            // D-TX-CEASED confirms the floor is idle to both MSs. The peer is
            // allowed to request transmission, but is not granted without a
            // queued U-TX DEMAND.
            Self::push_individual_d_tx_ceased(queue, call_id, holder_addr, holder_ts, holder_usage);
            Self::push_individual_d_tx_ceased(queue, call_id, peer_addr, peer_ts, peer_usage);

            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id, ts: holder_ts }),
            });

            if (called_over_brew || calling_over_brew)
                && let Some(brew_uuid) = brew_uuid
            {
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Brew,
                    msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id, ts: holder_ts }),
                });
                let _ = brew_uuid;
            }
            return;
        }

        let call_entry = self
            .active_calls
            .iter()
            .find(|(_, call)| call.ts == ts && call.tx_active)
            .map(|(id, _)| *id);

        let Some(call_id) = call_entry else {
            // Check if an echo session owns this timeslot — if so, reset the UL inactivity
            // timer by emitting FloorGranted so UMAC keeps the circuit alive.
            if let Some(ref session) = self.echo_session {
                if session.ts == ts {
                    tracing::debug!("UL inactivity timeout on echo ts={} — refreshing FloorGranted", ts);
                    let call_id = session.call_id;
                    let fake_issi = 0u32;
                    queue.push_back(tetra_saps::SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: tetra_core::tetra_entities::TetraEntity::Cmce,
                        dest: tetra_core::tetra_entities::TetraEntity::Umac,
                        msg: tetra_saps::SapMsgInner::CmceCallControl(tetra_saps::control::call_control::CallControl::FloorGranted {
                            call_id,
                            source_issi: fake_issi,
                            dest_gssi: fake_issi,
                            ts,
                        }),
                    });
                    return;
                }
            }
            tracing::debug!("UL inactivity timeout on ts={} but no active transmitting call found", ts);
            return;
        };

        if self.pending_group_releases.contains_key(&call_id) {
            tracing::debug!(
                "UL inactivity timeout on ts={} ignored for pending group release call_id={}",
                ts,
                call_id
            );
            return;
        }

        let call = self.active_calls.get_mut(&call_id).unwrap();
        tracing::warn!("UL inactivity timeout on ts={}, forcing TX ceased for call_id={}", ts, call_id);

        let dest_gssi = call.dest_gssi;
        let usage = call.usage;
        call.tx_active = false;
        call.hangtime_start = Some(self.dltime);

        self.send_d_tx_ceased_facch(queue, call_id, dest_gssi, ts, usage);

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id, ts }),
        });

        if net_brew::is_brew_gssi_routable(&self.config, dest_gssi) {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id, ts }),
            });
        }
    }
}
