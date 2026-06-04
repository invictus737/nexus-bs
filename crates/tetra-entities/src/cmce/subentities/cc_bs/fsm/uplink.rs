use super::super::dtmf::{DtmfKind, decode_dtmf, pack_type3_bits_to_bytes};
use super::*;

impl CcBsSubentity {
    fn push_direct_d_tx_not_granted(
        queue: &mut MessageQueue,
        call_id: u16,
        target: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
    ) {
        let reject = DTxGranted {
            call_identifier: call_id,
            transmission_grant: TransmissionGrant::NotGranted.into_raw() as u8,
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
        let mut sdu = BitBuffer::new_autoexpand(32);
        reject.to_bitbuf(&mut sdu).expect("Failed to serialize DTxGranted");
        sdu.seek(0);
        queue.push_back(Self::build_sapmsg_direct(sdu, target, handle, link_id, endpoint_id));
    }

    pub(in crate::cmce::subentities::cc_bs) fn push_individual_d_tx_granted(
        queue: &mut MessageQueue,
        call_id: u16,
        target_addr: TetraAddress,
        target_ts: u8,
        target_usage: u8,
        ul_dl_assigned: UlDlAssignment,
        transmission_grant: TransmissionGrant,
        _transmitting_issi: u32,
    ) {
        // EN 300 392-2 table 14.18 makes the transmitting party identifier
        // optional. For an individually addressed P2P floor response, the
        // transmission grant value is sufficient and keeping the PDU short
        // lets it fit on assigned-channel FACCH instead of falling back to
        // common-channel SCH/F while the MS is already on traffic.
        let pdu = DTxGranted {
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
        let mut sdu = BitBuffer::new_autoexpand(50);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DTxGranted");
        sdu.seek(0);
        queue.push_back(Self::build_sapmsg_stealing_ul_dl(
            sdu,
            target_addr,
            target_ts,
            Some(target_usage),
            ul_dl_assigned,
        ));
    }

    pub(in crate::cmce::subentities::cc_bs) fn push_individual_d_tx_ceased(
        queue: &mut MessageQueue,
        call_id: u16,
        target_addr: TetraAddress,
        target_ts: u8,
        target_usage: u8,
    ) {
        // EN 300 392-2 clause 14.5.1.2.1 e) allows the SwMI to send
        // D-TX CEASED to each MS at the end of a simplex individual
        // transmission. This is not a floor grant; table 14.81 raw value 0
        // (`false` here) means the MS is allowed to request transmission.
        let pdu = DTxCeased {
            call_identifier: call_id,
            transmission_request_permission: false,
            notification_indicator: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };
        let mut sdu = BitBuffer::new_autoexpand(30);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DTxCeased");
        sdu.seek(0);
        queue.push_back(Self::build_sapmsg_stealing_ul_dl(
            sdu,
            target_addr,
            target_ts,
            Some(target_usage),
            UlDlAssignment::Dl,
        ));
    }

    /// Handle parsed U-SETUP and dispatch into group/individual FSM paths.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_setup(
        &mut self,
        queue: &mut MessageQueue,
        message: &SapMsg,
        pdu: &USetup,
        calling_party: TetraAddress,
    ) {
        // Check if we can satisfy this request
        if !Self::feature_check_u_setup(pdu) {
            tracing::info!(
                "CMCE: rejecting U-SETUP from ISSI {} due to unsupported critical feature(s)",
                calling_party.ssi
            );
            let SapMsgInner::LcmcMleUnitdataInd(prim) = &message.msg else {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            };
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
                DisconnectCause::IncompatibleTrafficCase,
            );
            return;
        }

        // Handle P2P (individual) call setup separately
        if pdu.basic_service_information.communication_type == CommunicationType::P2p {
            self.fsm_on_u_setup_p2p(queue, message, pdu, calling_party);
            return;
        }
        self.fsm_on_u_setup_group(queue, message, pdu, calling_party);
    }

    /// Handle parsed U-TX CEASED.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_tx_ceased(
        &mut self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        pdu: UTxCeased,
    ) {
        let call_id = pdu.call_identifier;

        if let Some(call) = self.individual_calls.get(&call_id).cloned() {
            // For simplex PTT individual calls: MS released PTT.
            // Send D-TX-CEASED to both parties when no queued request exists;
            // send D-TX-GRANTED only when an earlier U-TX DEMAND is queued.
            // EN 300 392-2 clause 14.5.1.2.1 forbids unsolicited grants but
            // allows D-TX CEASED to each MS at end of transmission.
            if self.has_pending_individual_disconnect_tail_drain(call_id) || self.has_pending_individual_disconnect_delivery(call_id) {
                tracing::debug!(
                    "U-TX CEASED ignored while individual disconnect clear is pending call_id={}",
                    call_id
                );
                return;
            }
            if !call.is_active() {
                tracing::debug!("U-TX CEASED for inactive individual call_id={}, ignoring", call_id);
                return;
            }
            let Some(peer_issi) = call.peer_issi_for(sender.ssi) else {
                tracing::info!(
                    "U-TX CEASED (individual) from non-participant ISSI {} rejected for call_id={}",
                    sender.ssi,
                    call_id
                );
                return;
            };
            if call.simplex_duplex {
                // EN 300 392-2 clause 14.5.1.2.1 grants full-duplex
                // individual calls to both parties at connect time. The
                // TX-DEMAND/TX-CEASED floor exchange is for simplex PTT calls
                // and must not rewrite duplex traffic-channel direction.
                tracing::debug!(
                    "U-TX CEASED ignored for full-duplex individual call_id={} from ISSI {}",
                    call_id,
                    sender.ssi
                );
                return;
            }
            let (sender_ts, sender_usage, peer_addr, peer_ts, peer_usage) = if sender.ssi == call.calling_addr.ssi {
                (
                    call.calling_ts,
                    call.calling_usage,
                    call.called_addr,
                    call.called_ts,
                    call.called_usage,
                )
            } else {
                (
                    call.called_ts,
                    call.called_usage,
                    call.calling_addr,
                    call.calling_ts,
                    call.calling_usage,
                )
            };
            debug_assert_eq!(peer_addr.ssi, peer_issi);

            if call.floor_holder != Some(sender.ssi) {
                let cleared_queued_request = self.individual_calls.get_mut(&call_id).is_some_and(|c| {
                    if c.queued_tx_demand.is_some_and(|requester| requester.ssi == sender.ssi) {
                        c.queued_tx_demand = None;
                        true
                    } else {
                        false
                    }
                });

                if cleared_queued_request {
                    tracing::info!(
                        "U-TX CEASED (individual) call_id={} from queued ISSI {} -> withdrew queued request; current floor holder is {:?}",
                        call_id,
                        sender.ssi,
                        call.floor_holder
                    );
                } else {
                    tracing::debug!(
                        "U-TX CEASED (individual) call_id={} from ISSI {} ignored; current floor holder is {:?}",
                        call_id,
                        sender.ssi,
                        call.floor_holder
                    );
                }
                return;
            }

            let queued_requester = call.queued_tx_demand;

            if queued_requester.is_some() {
                let queued_requester = self.individual_calls.get_mut(&call_id).and_then(|c| {
                    c.floor_holder = None;
                    c.take_queued_tx_demand()
                });
                let Some(requester) = queued_requester else {
                    return;
                };
                let (requester_addr, requester_ts, requester_usage, listener_addr, listener_ts, listener_usage) =
                    if requester.ssi == peer_addr.ssi {
                        (peer_addr, peer_ts, peer_usage, sender, sender_ts, sender_usage)
                    } else {
                        (sender, sender_ts, sender_usage, peer_addr, peer_ts, peer_usage)
                    };

                tracing::info!(
                    "U-TX CEASED (individual) call_id={} from ISSI {} -> granting queued floor to ISSI {}",
                    call_id,
                    sender.ssi,
                    requester_addr.ssi
                );

                if let Some(c) = self.individual_calls.get_mut(&call_id) {
                    c.floor_holder = Some(requester_addr.ssi);
                }

                // EN 300 392-2 clause 14.5.1.2.1 e): if a request was queued
                // when U-TX CEASED arrives, the SwMI should send D-TX GRANTED
                // to both MSs without an explicit D-TX CEASED. The grant IE
                // decides who may transmit; keep the traffic-channel allocation
                // bidirectional per clause 21.5.2 so FACCH and receive audio
                // stay coherent on radios that reject UL-only reallocation.
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

                if (call.called_over_brew || call.calling_over_brew)
                    && let Some(brew_uuid) = call.brew_uuid
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

            tracing::info!(
                "U-TX CEASED (individual) call_id={} from ISSI {} -> delaying D-TX-CEASED for bearer tail drain",
                call_id,
                sender.ssi
            );

            // D-TX-CEASED to both MSs so the sender stops U-plane transmit and
            // the peer leaves the "other user transmitting" state. Clause
            // 14.5.1.2.1 b) still forbids unsolicited D-TX GRANTED, so the
            // peer is not granted merely because the current speaker stopped.
            self.begin_individual_tx_ceased_tail_drain(
                call_id,
                IndividualTailDrainLeg {
                    addr: sender,
                    ts: sender_ts,
                    usage: sender_usage,
                },
                IndividualTailDrainLeg {
                    addr: peer_addr,
                    ts: peer_ts,
                    usage: peer_usage,
                },
                call.called_over_brew || call.calling_over_brew,
            );

            return;
        }

        if let Err(err) = self.fsm_group_on_tx_ceased(queue, call_id, sender) {
            match err {
                GroupTransitionError::UnknownCall(_) => {
                    tracing::warn!("U-TX CEASED for unknown call_id={}", call_id);
                }
                GroupTransitionError::InvalidTransition { state, .. } => {
                    tracing::debug!(
                        "U-TX CEASED ignored for call_id={} due to invalid transition in state {:?}",
                        call_id,
                        state
                    );
                }
                GroupTransitionError::NotCurrentSpeaker {
                    sender_issi,
                    current_speaker_issi,
                    ..
                } => {
                    tracing::warn!(
                        "U-TX CEASED from non-current speaker ISSI {} on call_id={} (current speaker={}), ignoring",
                        sender_issi,
                        call_id,
                        current_speaker_issi
                    );
                }
                GroupTransitionError::MissingCachedSetup(_) => {
                    tracing::error!("U-TX CEASED call_id={} missing cached D-SETUP", call_id);
                }
            }
        }
    }

    /// Handle parsed U-TX DEMAND.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_tx_demand(
        &mut self,
        queue: &mut MessageQueue,
        requesting_party: TetraAddress,
        ul_handle: u32,
        ul_link_id: u32,
        ul_endpoint_id: u32,
        pdu: UTxDemand,
    ) {
        let call_id = pdu.call_identifier;

        if let Some(call) = self.individual_calls.get(&call_id).cloned() {
            // For simplex PTT individual calls: MS requests PTT floor.
            if self.has_pending_individual_disconnect_tail_drain(call_id) || self.has_pending_individual_disconnect_delivery(call_id) {
                tracing::debug!(
                    "U-TX DEMAND ignored while individual disconnect clear is pending call_id={}",
                    call_id
                );
                return;
            }
            if !call.is_active() {
                tracing::debug!("U-TX DEMAND for inactive individual call_id={}, ignoring", call_id);
                return;
            }
            let Some(peer_issi) = call.peer_issi_for(requesting_party.ssi) else {
                tracing::info!(
                    "U-TX DEMAND (individual) from non-participant ISSI {} rejected for call_id={}",
                    requesting_party.ssi,
                    call_id
                );
                // EN 300 392-2 table 14.80 defines "Transmission not
                // granted". For a private call, a third ISSI must not be
                // placed onto either participant's assigned channel, so answer
                // on the requester's LLC link without a channel allocation.
                Self::push_direct_d_tx_not_granted(queue, call_id, requesting_party, ul_handle, ul_link_id, ul_endpoint_id);
                return;
            };
            if call.simplex_duplex {
                // EN 300 392-2 clause 14.5.1.2.1 grants full-duplex
                // individual calls to both parties at connect time. A
                // later floor demand is only meaningful for simplex PTT.
                tracing::debug!(
                    "U-TX DEMAND ignored for full-duplex individual call_id={} from ISSI {}",
                    call_id,
                    requesting_party.ssi
                );
                return;
            }
            let (peer_addr, peer_ts, peer_usage, req_ts, req_usage) = if requesting_party.ssi == call.calling_addr.ssi {
                (
                    call.called_addr,
                    call.called_ts,
                    call.called_usage,
                    call.calling_ts,
                    call.calling_usage,
                )
            } else {
                (
                    call.calling_addr,
                    call.calling_ts,
                    call.calling_usage,
                    call.called_ts,
                    call.called_usage,
                )
            };
            debug_assert_eq!(peer_addr.ssi, peer_issi);

            if let Some(holder_issi) = call.floor_holder
                && holder_issi != requesting_party.ssi
            {
                let queue_result = self
                    .individual_calls
                    .get_mut(&call_id)
                    .map(|c| c.queue_tx_demand(requesting_party))
                    .unwrap_or(TxDemandQueueResult::QueueBusy);
                let response_grant = match queue_result {
                    TxDemandQueueResult::Queued | TxDemandQueueResult::AlreadyQueuedBySameUser => TransmissionGrant::RequestQueued,
                    TxDemandQueueResult::QueueBusy => TransmissionGrant::NotGranted,
                    TxDemandQueueResult::FromCurrentSpeaker => TransmissionGrant::Granted,
                };

                tracing::info!(
                    "U-TX DEMAND (individual) call_id={} from ISSI {} while ISSI {} has floor -> {:?}",
                    call_id,
                    requesting_party.ssi,
                    holder_issi,
                    response_grant
                );

                // EN 300 392-2 clause 14.5.1.2.1 b): when the other MS is
                // transmitting, SwMI should normally wait for U-TX CEASED. If
                // the request is queued, the requester must get an explicit
                // D-TX GRANTED with transmission grant=RequestQueued.
                Self::push_individual_d_tx_granted(
                    queue,
                    call_id,
                    requesting_party,
                    req_ts,
                    req_usage,
                    UlDlAssignment::Dl,
                    response_grant,
                    holder_issi,
                );
                return;
            }

            tracing::info!(
                "U-TX DEMAND (individual) call_id={} from ISSI {} -> granting floor, notifying peer ISSI {}",
                call_id,
                requesting_party.ssi,
                peer_addr.ssi
            );

            // Requester now owns the floor. EN 300 392-2 clause 14.5.1.2.1 b)
            // uses the D-TX GRANTED transmission-grant IE to decide who may
            // transmit. The channel allocation stays Both (clause 21.5.2) so
            // the already assigned traffic channel remains valid for FACCH and
            // receive audio; UMAC's FloorGranted control gates the active UL
            // speaker by ISSI.
            if let Some(c) = self.individual_calls.get_mut(&call_id) {
                c.floor_holder = Some(requesting_party.ssi);
            }
            Self::push_individual_d_tx_granted(
                queue,
                call_id,
                requesting_party,
                req_ts,
                req_usage,
                UlDlAssignment::Both,
                TransmissionGrant::Granted,
                requesting_party.ssi,
            );
            // Peer is now the listener; the grant IE says GrantedToOtherUser,
            // while the channel allocation remains bidirectional for channel
            // continuity.
            Self::push_individual_d_tx_granted(
                queue,
                call_id,
                peer_addr,
                peer_ts,
                peer_usage,
                UlDlAssignment::Both,
                TransmissionGrant::GrantedToOtherUser,
                requesting_party.ssi,
            );

            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                    call_id,
                    source_issi: requesting_party.ssi,
                    dest_gssi: peer_addr.ssi,
                    ts: req_ts,
                }),
            });

            if (call.called_over_brew || call.calling_over_brew)
                && let Some(brew_uuid) = call.brew_uuid
            {
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Brew,
                    msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                        call_id,
                        source_issi: requesting_party.ssi,
                        dest_gssi: peer_addr.ssi,
                        ts: req_ts,
                    }),
                });
                let _ = brew_uuid;
            }
            return;
        }

        if self.has_pending_individual_release(call_id) {
            tracing::debug!(
                "U-TX DEMAND ignored for pending individual release call_id={} from ISSI {}",
                call_id,
                requesting_party.ssi
            );
            return;
        }

        tracing::info!("U-TX DEMAND: ISSI {} requests floor on call_id={}", requesting_party.ssi, call_id);
        if let Err(err) = self.fsm_group_on_tx_demand(queue, call_id, requesting_party, pdu.tx_demand_priority) {
            match err {
                GroupTransitionError::UnknownCall(_) => {
                    tracing::warn!("U-TX DEMAND for unknown call_id={}", call_id);
                    // EN 300 392-2 clause 14.5.1.2.1 b) requires an explicit
                    // D-TX GRANTED/not-granted response when SwMI rejects a
                    // request-to-transmit. A stale private-call PTT can arrive
                    // after local call state has already cleared; answer on the
                    // requester's current signalling link without assigning any
                    // traffic channel.
                    Self::push_direct_d_tx_not_granted(queue, call_id, requesting_party, ul_handle, ul_link_id, ul_endpoint_id);
                }
                GroupTransitionError::InvalidTransition { state, .. } => {
                    tracing::debug!(
                        "U-TX DEMAND ignored for call_id={} due to invalid transition in state {:?}",
                        call_id,
                        state
                    );
                }
                GroupTransitionError::MissingCachedSetup(_) => {
                    tracing::error!("U-TX DEMAND call_id={} missing cached D-SETUP", call_id);
                }
                GroupTransitionError::NotCurrentSpeaker { .. } => {
                    tracing::debug!("U-TX DEMAND hit unexpected NotCurrentSpeaker transition error call_id={}", call_id);
                }
            }
        }
    }

    /// Handle parsed U-INFO.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_info(&mut self, queue: &mut MessageQueue, pdu: UInfo) {
        let call_id = pdu.call_identifier;
        let Some(call) = self.individual_calls.get(&call_id).cloned() else {
            tracing::trace!("U-INFO for unknown/non-individual call_id={}, ignoring", call_id);
            return;
        };

        if !call.called_over_brew && !call.calling_over_brew {
            tracing::trace!("U-INFO call_id={} is local individual call, no Brew forwarding", call_id);
            return;
        }

        let Some(brew_uuid) = call.brew_uuid else {
            tracing::warn!("U-INFO call_id={} marked Brew-routed but missing brew_uuid", call_id);
            return;
        };

        let Some(dtmf) = pdu.dtmf.as_ref() else {
            tracing::trace!(
                "U-INFO call_id={} has no DTMF element (modify={:?} facility={} proprietary={}), ignoring",
                call_id,
                pdu.modify,
                pdu.facility.is_some(),
                pdu.proprietary.is_some()
            );
            return;
        };

        let decoded = decode_dtmf(dtmf);
        if decoded.full_len_bits > decoded.parsed_bits {
            tracing::warn!(
                "U-INFO call_id={} DTMF payload is {} bits, parser retained only first {} bits",
                call_id,
                decoded.full_len_bits,
                decoded.parsed_bits
            );
        }
        if decoded.malformed {
            tracing::warn!(
                "U-INFO call_id={} has malformed DTMF payload (len={} bits, parsed={} bits, data={:?}, kind={:?})",
                call_id,
                decoded.full_len_bits,
                decoded.parsed_bits,
                dtmf.data,
                decoded.kind
            );
        }

        match decoded.kind {
            DtmfKind::ToneStart | DtmfKind::LegacyDigits => {}
            DtmfKind::ToneEnd => {
                tracing::trace!("U-INFO call_id={} DTMF tone end", call_id);
                return;
            }
            DtmfKind::NotSupported => {
                tracing::info!("U-INFO call_id={} DTMF not supported indication", call_id);
                return;
            }
            DtmfKind::NotSubscribed => {
                tracing::info!("U-INFO call_id={} DTMF not subscribed indication", call_id);
                return;
            }
            DtmfKind::Reserved(v) => {
                tracing::trace!("U-INFO call_id={} DTMF reserved type value {}", call_id, v);
                return;
            }
            DtmfKind::Invalid => {
                tracing::trace!("U-INFO call_id={} invalid/empty DTMF payload, ignoring", call_id);
                return;
            }
        }
        if decoded.digits.is_empty() {
            tracing::trace!("U-INFO call_id={} DTMF has no decoded digits, ignoring", call_id);
            return;
        }

        let (length_bits, data) = pack_type3_bits_to_bytes(dtmf);
        if length_bits == 0 || data.is_empty() {
            tracing::debug!("U-INFO call_id={} has empty DTMF payload, ignoring", call_id);
            return;
        }

        tracing::info!(
            "U-INFO (individual Brew) call_id={} uuid={} dtmf_kind={:?} digits='{}' dtmf_bits={} dtmf_bytes={}",
            call_id,
            brew_uuid,
            decoded.kind,
            decoded.digits,
            length_bits,
            data.len()
        );

        for ch in decoded.digits.chars() {
            let digit = ch as u8;

            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitDtmf {
                    brew_uuid,
                    length_bits: 8,
                    data: vec![digit],
                }),
            });
        }
    }

    /// Handle parsed U-RELEASE.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_release(
        &mut self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        ul_handle: u32,
        ul_link_id: u32,
        ul_endpoint_id: u32,
        pdu: URelease,
    ) {
        let call_id = pdu.call_identifier;
        let disconnect_cause = pdu.disconnect_cause;

        tracing::info!("U-RELEASE: call_id={} cause={}", call_id, disconnect_cause);
        if let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() {
            tracing::info!("U-RELEASE (individual) call_id={} cause={}", call_id, disconnect_cause);
            if let Some((pending_cause, release_to_issi)) =
                self.take_individual_disconnect_delivery_release_if_awaited_by(call_id, sender.ssi)
            {
                tracing::info!(
                    "U-RELEASE completes pending individual D-DISCONNECT delivery call_id={} from ISSI {}",
                    call_id,
                    sender.ssi
                );
                self.complete_individual_disconnect_peer_release(queue, call_id, pending_cause, release_to_issi);
                return;
            }

            if let Some((pending_cause, release_to_issi)) = call_snapshot.pending_disconnect_release_if_awaited_by(sender.ssi) {
                tracing::info!(
                    "U-RELEASE completes pending individual disconnect call_id={} from ISSI {}",
                    call_id,
                    sender.ssi
                );
                self.complete_individual_disconnect_peer_release(queue, call_id, pending_cause, release_to_issi);
                return;
            }

            if matches!(call_snapshot.state, IndividualCallState::DisconnectPending { .. }) {
                tracing::debug!(
                    "U-RELEASE ignored for pending individual disconnect call_id={} from non-awaited ISSI {}",
                    call_id,
                    sender.ssi
                );
                return;
            }

            if call_snapshot.peer_issi_for(sender.ssi).is_none() {
                tracing::info!(
                    "U-RELEASE (individual) from non-participant ISSI {} rejected for call_id={}",
                    sender.ssi,
                    call_id
                );
                let sdu = Self::build_d_release(call_id, DisconnectCause::RequestedServiceNotAvailable);
                let sender_addr = TetraAddress::new(sender.ssi, SsiType::Issi);
                let msg = Self::build_sapmsg_direct(sdu, sender_addr, ul_handle, ul_link_id, ul_endpoint_id);
                queue.push_back(msg);
                return;
            }

            tracing::info!(
                "U-RELEASE ignored for active individual call_id={} from ISSI {}; expected only after D-DISCONNECT",
                call_id,
                sender.ssi
            );
        } else {
            let Some(call) = self.active_calls.get(&call_id) else {
                tracing::debug!("U-RELEASE for unknown call_id={} (likely duplicate)", call_id);
                return;
            };

            let is_call_owner = matches!(&call.origin, CallOrigin::Local { caller_addr } if caller_addr.ssi == sender.ssi);
            if is_call_owner {
                tracing::info!(
                    "U-RELEASE ignored for active group call_id={} owner ISSI {}; group disconnection uses U-DISCONNECT",
                    call_id,
                    sender.ssi
                );
                return;
            }

            tracing::info!(
                "U-RELEASE: non-call-owner ISSI {} rejected for call_id={} cause={}",
                sender.ssi,
                call_id,
                disconnect_cause
            );
            let sdu = Self::build_d_release(call_id, DisconnectCause::RequestedServiceNotAvailable);
            let sender_addr = TetraAddress::new(sender.ssi, SsiType::Issi);
            let msg = Self::build_sapmsg_direct(sdu, sender_addr, ul_handle, ul_link_id, ul_endpoint_id);
            queue.push_back(msg);
        }
    }

    /// Handle parsed U-DISCONNECT.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_disconnect(
        &mut self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        ul_handle: u32,
        ul_link_id: u32,
        ul_endpoint_id: u32,
        pdu: UDisconnect,
    ) {
        let call_id = pdu.call_identifier;
        let disconnect_cause = pdu.disconnect_cause;

        if let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() {
            tracing::info!("U-DISCONNECT (individual) call_id={} cause={}", call_id, disconnect_cause);
            if matches!(call_snapshot.state, IndividualCallState::DisconnectPending { .. }) {
                tracing::debug!(
                    "U-DISCONNECT ignored for pending individual disconnect call_id={} from ISSI {}; expected U-RELEASE response to D-DISCONNECT",
                    call_id,
                    sender.ssi
                );
                return;
            }

            let Some(peer_issi) = call_snapshot.peer_issi_for(sender.ssi) else {
                tracing::info!(
                    "U-DISCONNECT (individual) from non-participant ISSI {} rejected for call_id={}",
                    sender.ssi,
                    call_id
                );
                let sdu = Self::build_d_release(call_id, DisconnectCause::RequestedServiceNotAvailable);
                let sender_addr = TetraAddress::new(sender.ssi, SsiType::Issi);
                let msg = Self::build_sapmsg_direct(sdu, sender_addr, ul_handle, ul_link_id, ul_endpoint_id);
                queue.push_back(msg);
                return;
            };

            if !call_snapshot.called_over_brew && !call_snapshot.calling_over_brew && call_snapshot.is_active() {
                if self.has_pending_individual_disconnect_tail_drain(call_id) || self.has_pending_individual_disconnect_delivery(call_id) {
                    tracing::debug!(
                        "U-DISCONNECT duplicate ignored while individual disconnect clear is pending call_id={}",
                        call_id
                    );
                    return;
                }
                self.send_individual_disconnect_release_ack(queue, call_id, &call_snapshot, sender.ssi, disconnect_cause);
                if !call_snapshot.simplex_duplex && call_snapshot.floor_holder.is_some() {
                    self.begin_individual_disconnect_tail_drain(call_id, sender, peer_issi, disconnect_cause);
                    return;
                }
                if let Some(reporter) = self.send_d_disconnect_individual(queue, call_id, &call_snapshot, sender, disconnect_cause) {
                    self.begin_individual_disconnect_delivery(call_id, peer_issi, sender.ssi, reporter, disconnect_cause);
                } else if let Some(call) = self.individual_calls.get_mut(&call_id) {
                    call.begin_disconnect_pending(peer_issi, sender.ssi, self.dltime, disconnect_cause);
                }
                return;
            }

            self.release_individual_call(queue, call_id, disconnect_cause);
            return;
        }

        let Some(call) = self.active_calls.get(&call_id) else {
            tracing::debug!("U-DISCONNECT for unknown call_id={} (likely duplicate)", call_id);
            return;
        };

        let is_call_owner = matches!(&call.origin, CallOrigin::Local { caller_addr } if caller_addr.ssi == sender.ssi);

        if is_call_owner {
            tracing::info!("U-DISCONNECT: call owner ISSI {} disconnecting call_id={}", sender.ssi, call_id);
            self.release_group_call(queue, call_id, DisconnectCause::UserRequestedDisconnection);
            return;
        }

        tracing::info!(
            "U-DISCONNECT: non-call-owner ISSI {} rejected for call_id={} cause={}",
            sender.ssi,
            call_id,
            disconnect_cause
        );

        let d_release = DRelease {
            call_identifier: call_id,
            disconnect_cause: DisconnectCause::RequestedServiceNotAvailable,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };
        tracing::info!("-> {:?} (to ISSI {})", d_release, sender.ssi);

        let mut sdu = BitBuffer::new_autoexpand(32);
        d_release.to_bitbuf(&mut sdu).expect("Failed to serialize DRelease");
        sdu.seek(0);

        let sender_addr = TetraAddress::new(sender.ssi, SsiType::Issi);
        let msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: ul_handle,
                endpoint_id: ul_endpoint_id,
                link_id: ul_link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                chan_alloc: None,
                main_address: sender_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }
}
