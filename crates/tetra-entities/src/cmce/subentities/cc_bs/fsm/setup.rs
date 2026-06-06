use super::*;

impl CcBsSubentity {
    /// Handle U-SETUP for group calls (non-P2P communication types).
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_setup_group(
        &mut self,
        queue: &mut MessageQueue,
        message: &SapMsg,
        pdu: &USetup,
        calling_party: TetraAddress,
    ) {
        // Extract UL message routing info for individually-addressed responses.
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let ul_handle = prim.handle;
        let ul_link_id = prim.link_id;
        let ul_endpoint_id = prim.endpoint_id;

        if !self.config.config().cell.transmission_interruption_enabled && Self::is_preemptive_call_priority(pdu.call_priority) {
            tracing::info!(
                "CMCE: rejecting pre-emptive group U-SETUP from issi={} priority={} (transmission interruption disabled)",
                calling_party.ssi,
                pdu.call_priority
            );
            // EN 300 392-2 table 14.46 defines call priorities 12..=15 as
            // pre-emptive. Clause 14.5.2.2.1 f) makes pre-emptive
            // transmission interruption conditional on SwMI support, and
            // clause 14.5.2.3.2 uses D-RELEASE when the SwMI cannot support
            // the requested group call before a SwMI call identity exists.
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                ul_handle,
                ul_link_id,
                ul_endpoint_id,
                DisconnectCause::RequestedServiceNotAvailable,
            );
            return;
        }

        // Get destination GSSI (called party)
        let Some(dest_gssi) = pdu.called_party_ssi else {
            tracing::warn!("U-SETUP group without called_party_ssi, rejecting");
            // EN 300 392-2 clause 14.5.2.3.2 requires D-RELEASE when the
            // SwMI cannot support a group-call request. No SwMI call identity
            // exists for a malformed setup, so use the dummy call identity 0.
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                ul_handle,
                ul_link_id,
                ul_endpoint_id,
                DisconnectCause::RequestedServiceNotAvailable,
            );
            return;
        };
        let dest_gssi = dest_gssi as u32;
        let dest_addr = TetraAddress::new(dest_gssi, SsiType::Gssi);

        if !self.has_listener(dest_gssi) {
            tracing::info!(
                "CMCE: rejecting U-SETUP from issi={} to gssi={} (no listeners)",
                calling_party.ssi,
                dest_gssi
            );
            // EN 300 392-2 clause 14.5.2.3.2 says the SwMI shall send a
            // D-RELEASE to the calling MS when it cannot support a group-call
            // request. No SwMI call identity has been allocated yet, so use
            // the dummy call identity 0 defined in clause 3.1.
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                ul_handle,
                ul_link_id,
                ul_endpoint_id,
                DisconnectCause::RequestedServiceNotAvailable,
            );
            return;
        }

        let same_gssi_active_call_id = self.active_calls.iter().find_map(|(&call_id, call)| {
            (call.dest_gssi == dest_gssi && !self.pending_group_releases.contains_key(&call_id)).then_some(call_id)
        });

        let same_gssi_pending_release_call_id = self.active_calls.iter().find_map(|(&call_id, call)| {
            (call.dest_gssi == dest_gssi && self.pending_group_releases.contains_key(&call_id)).then_some(call_id)
        });

        if let Some(active_call_id) = same_gssi_active_call_id {
            let Some(active_call) = self.active_calls.get(&active_call_id) else {
                tracing::warn!(
                    "CMCE: same-GSSI active call_id={} disappeared while handling U-SETUP from issi={} to gssi={}",
                    active_call_id,
                    calling_party.ssi,
                    dest_gssi
                );
                return;
            };

            if !self.subscriber_affiliated_to_group(calling_party.ssi, dest_gssi) {
                tracing::info!(
                    "CMCE: rejecting U-SETUP from unaffiliated issi={} to active gssi={} call_id={}",
                    calling_party.ssi,
                    dest_gssi,
                    active_call_id
                );
                Self::reject_u_setup_before_call_id(
                    queue,
                    calling_party,
                    ul_handle,
                    ul_link_id,
                    ul_endpoint_id,
                    DisconnectCause::RequestedServiceNotAvailable,
                );
                return;
            }

            let Some(cached_setup) = self.cached_setups.get(&active_call_id) else {
                tracing::warn!(
                    "CMCE: rejecting U-SETUP from issi={} to active gssi={} call_id={} because cached D-SETUP is missing",
                    calling_party.ssi,
                    dest_gssi,
                    active_call_id
                );
                Self::reject_u_setup_before_call_id(
                    queue,
                    calling_party,
                    ul_handle,
                    ul_link_id,
                    ul_endpoint_id,
                    DisconnectCause::RequestedServiceNotAvailable,
                );
                return;
            };

            let existing_setup = &cached_setup.pdu;
            let compatible_existing_call = existing_setup.hook_method_selection == pdu.hook_method_selection
                && existing_setup.simplex_duplex_selection == pdu.simplex_duplex_selection
                && existing_setup.basic_service_information.circuit_mode_type == pdu.basic_service_information.circuit_mode_type
                && existing_setup.basic_service_information.encryption_flag == pdu.basic_service_information.encryption_flag
                && existing_setup.basic_service_information.communication_type == pdu.basic_service_information.communication_type
                && existing_setup.basic_service_information.slots_per_frame == pdu.basic_service_information.slots_per_frame
                && existing_setup.basic_service_information.speech_service == pdu.basic_service_information.speech_service;
            if !compatible_existing_call {
                tracing::info!(
                    "CMCE: rejecting incompatible same-GSSI U-SETUP from issi={} to active gssi={} call_id={}",
                    calling_party.ssi,
                    dest_gssi,
                    active_call_id
                );
                Self::reject_u_setup_before_call_id(
                    queue,
                    calling_party,
                    ul_handle,
                    ul_link_id,
                    ul_endpoint_id,
                    DisconnectCause::RequestedServiceNotAvailable,
                );
                return;
            }

            let active_state = active_call.state();
            let current_speaker = active_call.source_issi;

            tracing::info!(
                "CMCE: mapping repeated U-SETUP from issi={} to active gssi={} call_id={} state={:?} as floor request",
                calling_party.ssi,
                dest_gssi,
                active_call_id,
                active_state
            );

            // EN 300 392-2 clause 14.5.2.1 covers group-call setup. Once the
            // same GSSI call is already maintained, clause 14.5.2.2.1 makes
            // transmit permission a floor-control procedure using
            // D-TX GRANTED. Treat a compatible repeated U-SETUP from field
            // radios as that floor request instead of mixing setup-phase
            // D-CALL PROCEEDING/D-CONNECT with active-call floor signalling.
            let floor_result = if matches!(active_state, GroupCallState::Transmitting) && current_speaker == calling_party.ssi {
                self.fsm_group_reassert_current_speaker_floor(queue, active_call_id, calling_party)
            } else {
                self.fsm_group_on_tx_demand(queue, active_call_id, calling_party, 0)
            };
            if let Err(err) = floor_result {
                tracing::warn!(
                    "CMCE: repeated U-SETUP floor handling failed call_id={} issi={} gssi={} err={:?}",
                    active_call_id,
                    calling_party.ssi,
                    dest_gssi,
                    err
                );
            }
            return;
        } else if let Some(pending_call_id) = same_gssi_pending_release_call_id {
            tracing::info!(
                "CMCE: accepting new U-SETUP from issi={} to gssi={} while stale call_id={} release drains",
                calling_party.ssi,
                dest_gssi,
                pending_call_id
            );
            // EN 300 392-2 clause 14.5.2.3 clears the old group call with
            // D-RELEASE, while clause 14.5.2.1 permits a later normal setup
            // for the same GSSI. Keep the stale release on its old assigned
            // channel until its reporter/guard completes, so EG subscribers
            // are not assumed to receive D-RELEASE immediately and the stale
            // D-RELEASE is not sent over the fresh call's allocation.
        }

        // Allocate circuit (DL+UL for group call)
        let occupied_call_ids = self.occupied_call_ids();
        let circuit = match {
            let mut state = self.config.state_write();
            self.circuits.allocate_circuit_with_allocator_duplex_avoiding(
                Direction::Both,
                pdu.basic_service_information.communication_type,
                pdu.simplex_duplex_selection,
                &mut state.timeslot_alloc,
                TimeslotOwner::Cmce,
                &occupied_call_ids,
            )
        } {
            Ok(circuit) => circuit.clone(),
            Err(e) => {
                tracing::error!("Failed to allocate circuit for U-SETUP: {:?}", e);
                Self::reject_u_setup_before_call_id(
                    queue,
                    calling_party,
                    ul_handle,
                    ul_link_id,
                    ul_endpoint_id,
                    DisconnectCause::CongestionInInfrastructure,
                );
                return;
            }
        };

        tracing::info!(
            "rx_u_setup: call from ISSI {} to GSSI {} -> ts={} call_id={} usage={}",
            calling_party.ssi,
            dest_gssi,
            circuit.ts,
            circuit.call_id,
            circuit.usage
        );

        // Emit telemetry event for dashboard
        self.emit(crate::net_telemetry::TelemetryEvent::GroupCallStarted {
            call_id: circuit.call_id,
            gssi: dest_gssi,
            caller_issi: calling_party.ssi,
            ts: circuit.ts,
        });

        // Signal UMAC to open DL+UL circuits. The primary active address
        // remains the GSSI, so UMAC treats the bearer as group-scoped. The
        // initial speaker ISSI is also recorded for assigned-channel state,
        // but must not make this look like a private/P2P participant list.
        Self::signal_umac_circuit_open_with_secondary(
            queue,
            &circuit,
            None,
            CircuitDlMediaSource::LocalLoopback,
            Some(TetraAddress::new(dest_gssi, SsiType::Gssi)),
            vec![calling_party],
        );

        // Build channel allocation timeslot mask for this call.
        let mut timeslots = [false; 4];
        timeslots[circuit.ts as usize - 1] = true;

        // 1) D-CALL-PROCEEDING to caller.
        self.send_d_call_proceeding(
            queue,
            message,
            pdu,
            circuit.call_id,
            CallTimeoutSetupPhase::T10s,
            pdu.hook_method_selection,
        );

        // 2) D-CONNECT to caller.
        let d_connect = DConnect {
            call_identifier: circuit.call_id,
            call_time_out: if pdu.simplex_duplex_selection {
                CallTimeout::Infinite
            } else {
                self.config_call_timeout()
            },
            hook_method_selection: pdu.hook_method_selection,
            simplex_duplex_selection: pdu.simplex_duplex_selection,
            transmission_grant: TransmissionGrant::Granted,
            transmission_request_permission: false,
            call_ownership: true,
            call_priority: None,
            basic_service_information: None,
            temporary_address: None,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };

        tracing::info!("-> {:?}", d_connect);
        let mut connect_sdu = BitBuffer::new_autoexpand(30);
        d_connect.to_bitbuf(&mut connect_sdu).expect("Failed to serialize DConnect");
        connect_sdu.seek(0);

        let connect_msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: connect_sdu,
                handle: ul_handle,
                endpoint_id: ul_endpoint_id,
                link_id: ul_link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                unacked_bl_repetitions: None,
                chan_alloc: Some(CmceChanAllocReq {
                    usage: Some(circuit.usage),
                    alloc_type: ChanAllocType::Replace,
                    carrier: None,
                    timeslots,
                    ul_dl_assigned: UlDlAssignment::Both,
                }),
                main_address: calling_party,
                tx_reporter: None,
            }),
        };
        queue.push_back(connect_msg);

        // 3) D-SETUP to group.
        let d_setup = DSetup {
            call_identifier: circuit.call_id,
            call_time_out: if pdu.simplex_duplex_selection {
                CallTimeout::Infinite
            } else {
                self.config_call_timeout()
            },
            hook_method_selection: pdu.hook_method_selection,
            simplex_duplex_selection: pdu.simplex_duplex_selection,
            basic_service_information: pdu.basic_service_information.clone(),
            transmission_grant: TransmissionGrant::GrantedToOtherUser,
            transmission_request_permission: false,
            call_priority: pdu.call_priority,
            notification_indicator: None,
            temporary_address: None,
            calling_party_address_ssi: Some(calling_party.ssi),
            calling_party_extension: None,
            external_subscriber_number: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };

        self.cached_setups.insert(
            circuit.call_id,
            CachedSetup {
                pdu: d_setup,
                dest_addr,
                resend: true,
                last_resend_reporter: None,
                is_individual: false,
            },
        );
        let d_setup_ref = &self.cached_setups.get(&circuit.call_id).unwrap().pdu;

        let (setup_sdu, setup_chan_alloc) = Self::build_d_setup_prim(d_setup_ref, circuit.usage, circuit.ts, UlDlAssignment::Both);
        let setup_msg = Self::build_sapmsg(setup_sdu, Some(setup_chan_alloc), dest_addr, Layer2Service::Unacknowledged, None);
        queue.push_back(setup_msg);

        // Track active group call.
        self.active_calls.insert(
            circuit.call_id,
            ActiveCall::new_local(
                calling_party,
                dest_gssi,
                calling_party.ssi,
                circuit.ts,
                circuit.usage,
                self.dltime,
                self.config_call_timeout(),
                pdu.call_priority,
            ),
        );

        // EN 300 392-2 clauses 14.5.2.1 and 14.5.2.2.1: after local group
        // setup the calling MS normally holds the first transmit permission.
        // UMAC needs the concrete ISSI because STCH MAC-U-SIGNAL carries no
        // address field (clause 21.4.5).
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: circuit.call_id,
                source_issi: calling_party.ssi,
                dest_gssi,
                ts: circuit.ts,
            }),
        });

        if net_brew::is_brew_gssi_routable(&self.config, dest_gssi) {
            let msg = SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                    call_id: circuit.call_id,
                    source_issi: calling_party.ssi,
                    dest_gssi,
                    ts: circuit.ts,
                }),
            };
            queue.push_back(msg);
        }
    }

    /// Handle U-SETUP for point-to-point (individual) duplex calls.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_setup_p2p(
        &mut self,
        queue: &mut MessageQueue,
        message: &SapMsg,
        pdu: &USetup,
        calling_party: TetraAddress,
    ) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        if Self::is_preemptive_call_priority(pdu.call_priority) {
            tracing::info!(
                "CMCE: rejecting pre-emptive P2P U-SETUP from issi={} priority={} (private call interruption not supported)",
                calling_party.ssi,
                pdu.call_priority
            );
            // EN 300 392-2 table 14.46 defines priorities 12..=15 as
            // pre-emptive. Clause 14.5.1.2.1 f) makes private-call
            // interruption conditional on SwMI support. This stack only
            // implements configured group transmission interruption, so reject
            // private pre-emption rather than accepting partial semantics.
            // Clause 14.5.1.3.2 uses D-RELEASE when the request cannot be
            // supported.
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
                DisconnectCause::RequestedServiceNotAvailable,
            );
            return;
        }

        let is_issi_address = pdu.called_party_type_identifier == tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier::Ssi
            || pdu.called_party_type_identifier == tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier::Tsi;
        if !is_issi_address && !net_brew::is_active(&self.config) {
            tracing::warn!(
                "U-SETUP P2P with non-ISSI called_party_type_identifier={} (rejecting, Brew disabled)",
                pdu.called_party_type_identifier
            );
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
                DisconnectCause::RequestedServiceNotAvailable,
            );
            return;
        }
        if is_issi_address
            && (pdu.called_party_short_number_address.is_some()
                || (pdu.called_party_extension.is_some()
                    && pdu.called_party_type_identifier != tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier::Tsi))
        {
            tracing::warn!("U-SETUP P2P with invalid called party fields (short number/extension mismatch), rejecting");
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
                DisconnectCause::RequestedServiceNotAvailable,
            );
            return;
        }
        if pdu.called_party_type_identifier == tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier::Tsi {
            let Some(extension) = pdu.called_party_extension else {
                tracing::warn!("U-SETUP P2P with TSI called party missing called_party_extension, rejecting");
                Self::reject_u_setup_before_call_id(
                    queue,
                    calling_party,
                    prim.handle,
                    prim.link_id,
                    prim.endpoint_id,
                    DisconnectCause::RequestedServiceNotAvailable,
                );
                return;
            };
            let called_mcc = ((extension >> 14) & 0x03ff) as u16;
            let called_mnc = (extension & 0x3fff) as u16;
            let net = self.config.config().net.clone();
            if called_mcc != net.mcc || called_mnc != net.mnc {
                // EN 300 392-2 table 14.41 defines called party extension as
                // the MCC+MNC extended TSI part. Do not collapse a foreign TSI
                // onto a local ISSI that happens to share the same 24-bit SSI.
                tracing::warn!(
                    "U-SETUP P2P to non-local TSI mcc={} mnc={} ssi={:?} rejected (local mcc={} mnc={})",
                    called_mcc,
                    called_mnc,
                    pdu.called_party_ssi,
                    net.mcc,
                    net.mnc
                );
                Self::reject_u_setup_before_call_id(
                    queue,
                    calling_party,
                    prim.handle,
                    prim.link_id,
                    prim.endpoint_id,
                    DisconnectCause::RequestedServiceNotAvailable,
                );
                return;
            }
        }

        let called_ssi = pdu.called_party_ssi.map(|v| v as u32).unwrap_or(0);
        let has_external_number = pdu.external_subscriber_number.is_some() || pdu.called_party_short_number_address.is_some();
        if called_ssi == 0 && !has_external_number {
            tracing::warn!("U-SETUP P2P without called ISSI/number, ignoring");
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
                DisconnectCause::RequestedServiceNotAvailable,
            );
            return;
        }

        let called_addr = TetraAddress::new(called_ssi, SsiType::Issi);

        if let Some((active_call_id, state)) = self.find_individual_call_by_issi(calling_party.ssi) {
            tracing::info!(
                "CMCE: rejecting U-SETUP P2P from busy calling ISSI {} to ISSI {} (caller already in call_id={} state={:?})",
                calling_party.ssi,
                called_addr.ssi,
                active_call_id,
                state
            );
            // EN 300 392-2 clause 14.5.1.1.2 starts outgoing private call
            // setup from an idle CC sub-entity. If the calling ISSI is already
            // bound to an individual call, clause 14.5.1.3.2 permits
            // D-RELEASE because the SwMI cannot support this second setup.
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
                DisconnectCause::NoIdleCcEntity,
            );
            return;
        }

        // ── Echo service (ISSI 999) ──────────────────────────────────────────
        if called_ssi == crate::cmce::subentities::cc_bs::echo::ECHO_ISSI {
            self.fsm_on_u_setup_echo(queue, message, pdu, calling_party);
            return;
        }

        // PBX/phone calls (no concrete local ISSI) always go through Brew.
        if called_ssi == 0 {
            self.fsm_on_u_setup_p2p_over_brew(queue, message, pdu, calling_party, called_addr);
            return;
        }

        let called_is_configured_local = {
            let config = self.config.config();
            config.cell.local_ssi_ranges.contains(called_addr.ssi)
        };

        if !self.subscriber_groups.contains_key(&called_addr.ssi) {
            if called_is_configured_local {
                tracing::info!(
                    "CMCE: rejecting U-SETUP P2P from ISSI {} to local unregistered ISSI {}",
                    calling_party.ssi,
                    called_addr.ssi
                );
                // EN 300 392-2 clause 14.5.1.1.2 scopes the first SwMI
                // response to a private-call setup. No SwMI call identity
                // exists yet, so clause 14.5.1.3.2 rejection uses the dummy
                // call reference. The local SSI range is deployment policy:
                // it prevents a configured-local ISSI from being reclassified
                // as an external Brew destination just because it is offline.
                Self::reject_u_setup_before_call_id(
                    queue,
                    calling_party,
                    prim.handle,
                    prim.link_id,
                    prim.endpoint_id,
                    DisconnectCause::CalledPartyNotReachable,
                );
                return;
            }
            self.fsm_on_u_setup_p2p_over_brew(queue, message, pdu, calling_party, called_addr);
            return;
        }

        if let Some((active_call_id, state)) = self.find_individual_call_by_issi(called_addr.ssi) {
            tracing::info!(
                "CMCE: rejecting U-SETUP P2P from ISSI {} to ISSI {} (called party busy in call_id={} state={:?})",
                calling_party.ssi,
                called_addr.ssi,
                active_call_id,
                state
            );
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
                DisconnectCause::CalledPartyBusy,
            );
            return;
        }

        // Allocate circuit(s). Duplex uses two traffic timeslots, one per MS, with cross-routing.
        let occupied_call_ids = self.occupied_call_ids();
        let (circuit_calling, circuit_called) = {
            let mut state = self.config.state_write();
            let circuit_calling = match self.circuits.allocate_circuit_with_allocator_duplex_avoiding(
                Direction::Both,
                pdu.basic_service_information.communication_type,
                pdu.simplex_duplex_selection,
                &mut state.timeslot_alloc,
                TimeslotOwner::Cmce,
                &occupied_call_ids,
            ) {
                Ok(circuit) => circuit.clone(),
                Err(e) => {
                    tracing::info!(
                        "CMCE: rejecting U-SETUP P2P from ISSI {} to ISSI {}, failed to allocate circuit for U-SETUP P2P, error: {:?}",
                        calling_party.ssi,
                        called_addr.ssi,
                        e
                    );
                    Self::reject_u_setup_before_call_id(
                        queue,
                        calling_party,
                        prim.handle,
                        prim.link_id,
                        prim.endpoint_id,
                        DisconnectCause::CongestionInInfrastructure,
                    );
                    return;
                }
            };

            let circuit_called = if pdu.simplex_duplex_selection {
                match self.circuits.allocate_circuit_for_call_with_allocator(
                    circuit_calling.call_id,
                    Direction::Both,
                    pdu.basic_service_information.communication_type,
                    pdu.simplex_duplex_selection,
                    &mut state.timeslot_alloc,
                    TimeslotOwner::Cmce,
                ) {
                    Ok(circuit) => Some(circuit.clone()),
                    Err(e) => {
                        let _ = self.circuits.close_circuit(Direction::Both, circuit_calling.ts);
                        let _ = state.timeslot_alloc.release(TimeslotOwner::Cmce, circuit_calling.ts);
                        tracing::info!(
                            "CMCE: rejecting U-SETUP P2P from ISSI {} to ISSI {}, failed to allocate second circuit for duplex P2P, error {:?}",
                            calling_party.ssi,
                            called_addr.ssi,
                            e
                        );
                        Self::reject_u_setup_before_call_id(
                            queue,
                            calling_party,
                            prim.handle,
                            prim.link_id,
                            prim.endpoint_id,
                            DisconnectCause::CongestionInInfrastructure,
                        );
                        return;
                    }
                }
            } else {
                None
            };

            (circuit_calling, circuit_called)
        };

        let calling_ts = circuit_calling.ts;
        let calling_usage = circuit_calling.usage;
        let call_id = circuit_calling.call_id;
        let (called_ts, called_usage) = if let Some(called) = &circuit_called {
            (called.ts, called.usage)
        } else {
            (calling_ts, calling_usage)
        };

        tracing::info!(
            "CMCE: rx_u_setup_p2p call from ISSI {} to ISSI {} -> call_id={} ts(call)={} usage(call)={} ts(called)={} usage(called)={}",
            calling_party.ssi,
            called_addr.ssi,
            call_id,
            calling_ts,
            calling_usage,
            called_ts,
            called_usage
        );

        // Emit telemetry event for dashboard
        self.emit(crate::net_telemetry::TelemetryEvent::IndividualCallStarted {
            call_id,
            calling_issi: calling_party.ssi,
            called_issi: called_addr.ssi,
            simplex: !pdu.hook_method_selection,
            ts: calling_ts,
        });

        // Do not open traffic channel yet. Let called MS respond on MCCH.
        self.send_d_call_proceeding(queue, message, pdu, call_id, CallTimeoutSetupPhase::T60s, pdu.hook_method_selection);

        let setup_grant = if !pdu.simplex_duplex_selection
            && pdu.hook_method_selection
            && Self::private_simplex_called_ms_transmits_first(
                pdu.simplex_duplex_selection,
                pdu.hook_method_selection,
                pdu.request_to_transmit_send_data,
            ) {
            TransmissionGrant::Granted
        } else if !pdu.simplex_duplex_selection && pdu.hook_method_selection {
            TransmissionGrant::GrantedToOtherUser
        } else {
            TransmissionGrant::NotGranted
        };

        let d_setup = DSetup {
            call_identifier: call_id,
            call_time_out: if pdu.simplex_duplex_selection {
                CallTimeout::Infinite
            } else {
                self.config_call_timeout()
            },
            hook_method_selection: pdu.hook_method_selection,
            simplex_duplex_selection: pdu.simplex_duplex_selection,
            basic_service_information: pdu.basic_service_information.clone(),
            transmission_grant: setup_grant,
            transmission_request_permission: false,
            call_priority: pdu.call_priority,
            notification_indicator: None,
            temporary_address: None,
            calling_party_address_ssi: Some(calling_party.ssi),
            calling_party_extension: None,
            external_subscriber_number: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };
        tracing::debug!("-> {:?}", d_setup);

        let initial_setup_reporter = TxReporter::new_unacked();
        self.cached_setups.insert(
            call_id,
            CachedSetup {
                pdu: d_setup,
                dest_addr: called_addr,
                resend: true,
                last_resend_reporter: Some(initial_setup_reporter.clone()),
                is_individual: true,
            },
        );

        let d_setup_ref = &self.cached_setups.get(&call_id).unwrap().pdu;
        let mut setup_sdu = BitBuffer::new_autoexpand(80);
        d_setup_ref.to_bitbuf(&mut setup_sdu).expect("Failed to serialize DSetup");
        setup_sdu.seek(0);
        // EN 300 392-2 clause 14.5.1.1.1 requires the SwMI to deliver
        // D-SETUP to the called MS before U-CONNECT can arrive. Track this
        // first MCCH transfer so local EE retries do not duplicate it while MAC
        // still has the request pending for the called MS's receive window.
        let setup_msg = Self::build_sapmsg(
            setup_sdu,
            None,
            called_addr,
            Layer2Service::Unacknowledged,
            Some(initial_setup_reporter),
        );
        queue.push_back(setup_msg);

        if let Err(err) = self.fsm_individual_create_setup_call(
            call_id,
            IndividualCall {
                calling_addr: calling_party,
                called_addr,
                calling_handle: prim.handle,
                calling_link_id: prim.link_id,
                calling_endpoint_id: prim.endpoint_id,
                called_handle: None,
                called_link_id: None,
                called_endpoint_id: None,
                calling_ts,
                called_ts,
                calling_usage,
                called_usage,
                simplex_duplex: pdu.simplex_duplex_selection,
                request_to_transmit_send_data: pdu.request_to_transmit_send_data,
                state: IndividualCallState::CallSetupPending,
                setup_timer_started: Some(self.dltime),
                setup_timeout: Some(CallTimeoutSetupPhase::T60s),
                active_timer_started: None,
                call_timeout: self.config_call_timeout(),
                called_over_brew: false,
                calling_over_brew: false,
                brew_uuid: None,
                network_call: None,
                connect_request_sent: false,
                floor_holder: None,
                last_floor_holder: None,
                queued_tx_demand: None,
            },
        ) {
            match err {
                IndividualTransitionError::DuplicateCall(_) => {
                    tracing::warn!("CMCE: duplicate call_id={} while creating local P2P setup", call_id);
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::warn!("CMCE: local P2P setup call_id={} creation rejected for state {:?}", call_id, state);
                }
                IndividualTransitionError::UnknownCall(_)
                | IndividualTransitionError::MissingBrewUuid(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }
    }

    /// Handle U-SETUP for non-local ISSI, PBX and phone calls via Brew.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_setup_p2p_over_brew(
        &mut self,
        queue: &mut MessageQueue,
        message: &SapMsg,
        pdu: &USetup,
        calling_party: TetraAddress,
        called_addr: TetraAddress,
    ) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let mut network_call = Self::build_network_circuit_call_from_u_setup(pdu, calling_party.ssi);

        // Short service numbers (< 1_000_000) must be sent to TetraPack
        // with destination=0 and the number as a string.
        // Keep communication=P2p(0), mode=TchS(0), duplex=0 — this combination
        // resulted in SETUP_ACCEPT in previous tests.
        if network_call.destination > 0 && network_call.destination < 1_000_000 && network_call.number.is_empty() {
            tracing::debug!(
                "CMCE: converting short service SSI {} to number string for TetraPack",
                network_call.destination
            );
            network_call.number = network_call.destination.to_string();
            network_call.destination = 0;
            network_call.duplex = 0;
        }

        if !net_brew::is_active(&self.config) {
            tracing::info!(
                "CMCE: rejecting U-SETUP P2P from ISSI {} (Brew disabled, called_ssi={})",
                calling_party.ssi,
                called_addr.ssi
            );
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
                DisconnectCause::RequestedServiceNotAvailable,
            );
            return;
        }

        if !self.config.state_read().network_connected {
            tracing::info!(
                "CMCE: rejecting U-SETUP over Brew src={} dst={} (backhaul disconnected)",
                calling_party.ssi,
                called_addr.ssi
            );
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
                DisconnectCause::RequestedServiceNotAvailable,
            );
            return;
        }

        if !net_brew::is_brew_issi_routable(&self.config, calling_party.ssi) {
            tracing::info!(
                "CMCE: rejecting U-SETUP P2P over Brew src={} dst={} (source ISSI not routable)",
                calling_party.ssi,
                called_addr.ssi
            );
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
                DisconnectCause::CalledPartyNotReachable,
            );
            return;
        }

        let has_external_called_party = Self::has_external_called_party(pdu, &network_call);
        let destination_routable = network_call.destination == 0 || net_brew::is_brew_issi_routable(&self.config, network_call.destination);

        if !has_external_called_party && !destination_routable {
            tracing::info!(
                "CMCE: rejecting U-SETUP P2P over Brew src={} dst={} (destination ISSI not routable)",
                calling_party.ssi,
                network_call.destination
            );
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
                DisconnectCause::CalledPartyNotReachable,
            );
            return;
        }

        if has_external_called_party && !destination_routable && network_call.destination != 0 {
            // Only override if the number field is non-empty and destination is not a
            // short service number (< 1_000_000). Short numbers like 600, 000 etc. are
            // service codes on TetraPack and must be forwarded as-is via the number field.
            let number_is_service_code = !network_call.number.is_empty() && network_call.number.chars().all(|c| c.is_ascii_digit());
            if !number_is_service_code {
                tracing::debug!(
                    "CMCE: overriding non-routable destination SSI {} with 0 for external-number call src={} number='{}'",
                    network_call.destination,
                    calling_party.ssi,
                    network_call.number
                );
                network_call.destination = 0;
            } else {
                tracing::debug!(
                    "CMCE: keeping destination SSI {} for service-code call src={} number='{}'",
                    network_call.destination,
                    calling_party.ssi,
                    network_call.number
                );
            }
        }

        // Allocate one bearer for the local MS.
        let occupied_call_ids = self.occupied_call_ids();
        let circuit_calling = {
            let mut state = self.config.state_write();
            match self.circuits.allocate_circuit_with_allocator_duplex_avoiding(
                Direction::Both,
                pdu.basic_service_information.communication_type,
                pdu.simplex_duplex_selection,
                &mut state.timeslot_alloc,
                TimeslotOwner::Cmce,
                &occupied_call_ids,
            ) {
                Ok(circuit) => circuit.clone(),
                Err(e) => {
                    tracing::info!(
                        "CMCE: rejecting U-SETUP over Brew src={} dst={} (allocation failed: {:?})",
                        calling_party.ssi,
                        called_addr.ssi,
                        e
                    );
                    Self::reject_u_setup_before_call_id(
                        queue,
                        calling_party,
                        prim.handle,
                        prim.link_id,
                        prim.endpoint_id,
                        DisconnectCause::CongestionInInfrastructure,
                    );
                    return;
                }
            }
        };

        let call_id = circuit_calling.call_id;
        let ts = circuit_calling.ts;
        let usage = circuit_calling.usage;
        let brew_uuid = uuid::Uuid::new_v4();

        tracing::info!(
            "CMCE: forwarding U-SETUP over Brew call_id={} src={} dst={} ts={} duplex={} number='{}' uuid={}",
            call_id,
            calling_party.ssi,
            network_call.destination,
            ts,
            network_call.duplex,
            network_call.number,
            brew_uuid
        );

        self.send_d_call_proceeding(queue, message, pdu, call_id, CallTimeoutSetupPhase::T60s, pdu.hook_method_selection);

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Brew,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest {
                brew_uuid,
                call: network_call.clone(),
            }),
        });

        if let Err(err) = self.fsm_individual_create_setup_call(
            call_id,
            IndividualCall {
                calling_addr: calling_party,
                called_addr,
                calling_handle: prim.handle,
                calling_link_id: prim.link_id,
                calling_endpoint_id: prim.endpoint_id,
                called_handle: None,
                called_link_id: None,
                called_endpoint_id: None,
                calling_ts: ts,
                called_ts: ts,
                calling_usage: usage,
                called_usage: usage,
                simplex_duplex: pdu.simplex_duplex_selection,
                request_to_transmit_send_data: pdu.request_to_transmit_send_data,
                state: IndividualCallState::CallSetupPending,
                setup_timer_started: Some(self.dltime),
                setup_timeout: Some(CallTimeoutSetupPhase::T60s),
                active_timer_started: None,
                call_timeout: self.config_call_timeout(),
                called_over_brew: true,
                calling_over_brew: false,
                brew_uuid: Some(brew_uuid),
                network_call: Some(network_call),
                connect_request_sent: false,
                floor_holder: None,
                last_floor_holder: None,
                queued_tx_demand: None,
            },
        ) {
            match err {
                IndividualTransitionError::DuplicateCall(_) => {
                    tracing::warn!("CMCE: duplicate call_id={} while creating Brew P2P setup", call_id);
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::warn!("CMCE: Brew P2P setup call_id={} creation rejected for state {:?}", call_id, state);
                }
                IndividualTransitionError::UnknownCall(_)
                | IndividualTransitionError::MissingBrewUuid(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }
    }
    /// Handle U-SETUP toward ISSI 999 — local echo service.
    /// Answers immediately with DConnect (full-duplex), no Brew involved.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_setup_echo(
        &mut self,
        queue: &mut MessageQueue,
        message: &SapMsg,
        pdu: &USetup,
        calling_party: TetraAddress,
    ) {
        use tetra_saps::control::call_control::CircuitDlMediaSource;

        let SapMsgInner::LcmcMleUnitdataInd(prim) = &message.msg else {
            tracing::error!("BUG: unexpected message in fsm_on_u_setup_echo");
            return;
        };

        // Reject if another echo call is already active
        if self.echo_session.is_some() {
            tracing::info!("CMCE: echo service busy, rejecting call from ISSI {}", calling_party.ssi);
            Self::reject_u_setup_before_call_id(
                queue,
                calling_party,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
                DisconnectCause::CalledPartyBusy,
            );
            return;
        }

        // Allocate a single full-duplex circuit
        let occupied_call_ids = self.occupied_call_ids();
        let circuit = {
            let mut state = self.config.state_write();
            match self.circuits.allocate_circuit_with_allocator_duplex_avoiding(
                Direction::Both,
                pdu.basic_service_information.communication_type,
                false,
                &mut state.timeslot_alloc,
                TimeslotOwner::Cmce,
                &occupied_call_ids,
            ) {
                Ok(c) => c.clone(),
                Err(e) => {
                    tracing::warn!("CMCE: echo service failed to allocate circuit: {:?}", e);
                    Self::reject_u_setup_before_call_id(
                        queue,
                        calling_party,
                        prim.handle,
                        prim.link_id,
                        prim.endpoint_id,
                        DisconnectCause::CongestionInInfrastructure,
                    );
                    return;
                }
            }
        };

        let call_id = circuit.call_id;
        let ts = circuit.ts;
        let usage = circuit.usage;

        tracing::info!(
            "CMCE: echo service answering call_id={} src={} ts={}",
            call_id,
            calling_party.ssi,
            ts
        );

        // Open UMAC circuit — LocalLoopback so UL frames are echoed back as DL
        CcBsSubentity::signal_umac_circuit_open(queue, &circuit, None, CircuitDlMediaSource::LocalLoopback, Some(calling_party));

        // D-CALL-PROCEEDING
        self.send_d_call_proceeding(queue, message, pdu, call_id, CallTimeoutSetupPhase::T10s, pdu.hook_method_selection);

        // D-CONNECT — grant transmission immediately (full duplex, calling party talks)
        {
            let d_connect = DConnect {
                call_identifier: call_id,
                call_time_out: if pdu.simplex_duplex_selection {
                    CallTimeout::Infinite
                } else {
                    self.config_call_timeout()
                },
                hook_method_selection: pdu.hook_method_selection,
                simplex_duplex_selection: pdu.simplex_duplex_selection,
                transmission_grant: TransmissionGrant::Granted,
                transmission_request_permission: false,
                call_ownership: true,
                call_priority: None,
                basic_service_information: None,
                temporary_address: None,
                notification_indicator: None,
                facility: None,
                proprietary: None,
            };
            tracing::info!("CMCE: echo service -> {:?}", d_connect);
            let mut connect_sdu = BitBuffer::new_autoexpand(30);
            d_connect.to_bitbuf(&mut connect_sdu).expect("Failed to serialize DConnect");
            connect_sdu.seek(0);
            // Include chan_alloc so the terminal knows to transmit on the assigned timeslot.
            let mut timeslots = [false; 4];
            timeslots[ts as usize - 1] = true;
            let connect_msg = SapMsg {
                sap: Sap::LcmcSap,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Mle,
                msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                    sdu: connect_sdu,
                    handle: prim.handle,
                    endpoint_id: prim.endpoint_id,
                    link_id: prim.link_id,
                    layer2service: Layer2Service::Unacknowledged,
                    pdu_prio: 0,
                    layer2_qos: 0,
                    stealing_permission: false,
                    stealing_repeats_flag: false,
                    unacked_bl_repetitions: None,
                    chan_alloc: Some(CmceChanAllocReq {
                        usage: Some(usage),
                        alloc_type: ChanAllocType::Replace,
                        carrier: None,
                        timeslots,
                        ul_dl_assigned: UlDlAssignment::Both,
                    }),
                    main_address: calling_party,
                    tx_reporter: None,
                }),
            };
            queue.push_back(connect_msg);
        }

        // Store echo session — UL frames will be echoed back as DL
        self.echo_session = Some(crate::cmce::subentities::cc_bs::echo::EchoSession::new(ts, call_id));

        // Register individual call directly (bypass create FSM which only accepts Pending states)
        self.individual_calls.insert(
            call_id,
            IndividualCall {
                calling_addr: calling_party,
                called_addr: TetraAddress::new(crate::cmce::subentities::cc_bs::echo::ECHO_ISSI, tetra_core::SsiType::Issi),
                calling_handle: prim.handle,
                calling_link_id: prim.link_id,
                calling_endpoint_id: prim.endpoint_id,
                called_handle: None,
                called_link_id: None,
                called_endpoint_id: None,
                calling_ts: ts,
                called_ts: ts,
                calling_usage: usage,
                called_usage: usage,
                simplex_duplex: pdu.simplex_duplex_selection,
                request_to_transmit_send_data: pdu.request_to_transmit_send_data,
                state: crate::cmce::subentities::cc_bs::call::IndividualCallState::Active,
                setup_timer_started: None,
                setup_timeout: None,
                active_timer_started: Some(self.dltime),
                call_timeout: self.config_call_timeout(),
                called_over_brew: false,
                calling_over_brew: false,
                brew_uuid: None,
                network_call: None,
                connect_request_sent: false,
                floor_holder: Some(calling_party.ssi),
                last_floor_holder: Some(calling_party.ssi),
                queued_tx_demand: None,
            },
        );

        // Notify UMAC that the floor is granted — this resets the UL inactivity timer
        // so the circuit stays alive while the caller is talking.
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id,
                source_issi: calling_party.ssi,
                dest_gssi: calling_party.ssi,
                ts,
            }),
        });
    }
}
