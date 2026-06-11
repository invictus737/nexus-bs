use super::*;

const NETWORK_INDIVIDUAL_CONNECT_PENDING_TIMEOUT_TIMESLOTS: i32 = 2 * 18 * 4;

#[derive(Clone, Copy)]
enum PendingNetworkConnectAction {
    Complete,
    Fail,
}

impl CcBsSubentity {
    /// EN 300 392-2 table 14.46 defines call priority 12..=15 as
    /// pre-emptive priorities, with 15 as emergency.
    pub(in crate::cmce::subentities::cc_bs) fn is_preemptive_call_priority(priority: u8) -> bool {
        (12..=15).contains(&priority)
    }

    /// EN 300 392-2 table 14.50 defines the 4-bit call time-out field; value
    /// 15 is reserved and must not be reflected into D-SETUP/D-CONNECT.
    fn network_circuit_call_timeout(raw: u8, fallback: CallTimeout) -> CallTimeout {
        match CallTimeout::try_from(raw as u64) {
            Ok(CallTimeout::Reserved) | Err(_) => fallback,
            Ok(timeout) => timeout,
        }
    }

    /// EN 300 392-2 table 14.62 is represented as a boolean in CMCE PDUs.
    fn network_circuit_hook_method(raw: u8) -> bool {
        raw != 0
    }

    fn network_circuit_grant(raw: u8) -> TransmissionGrant {
        TransmissionGrant::try_from((raw & 0x03) as u64).unwrap_or(TransmissionGrant::Granted)
    }

    fn apply_brew_simplex_initial_floor(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        local_addr: TetraAddress,
        peer_addr: TetraAddress,
        ts: u8,
        simplex_duplex: bool,
        grant: TransmissionGrant,
    ) {
        if simplex_duplex {
            return;
        }

        let floor_holder = match grant {
            TransmissionGrant::Granted => Some(local_addr.ssi),
            TransmissionGrant::GrantedToOtherUser => Some(peer_addr.ssi),
            TransmissionGrant::NotGranted | TransmissionGrant::RequestQueued => None,
        };

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            if let Some(holder_issi) = floor_holder {
                call.set_floor_holder(holder_issi);
            } else {
                call.clear_floor_holder();
            }
        }

        tracing::info!(
            "CMCE: Brew simplex private call_id={} initial floor_holder={:?} local_issi={} peer_issi={} grant={:?}",
            call_id,
            floor_holder,
            local_addr.ssi,
            peer_addr.ssi,
            grant
        );

        if floor_holder == Some(local_addr.ssi) {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                    call_id,
                    source_issi: local_addr.ssi,
                    dest_gssi: peer_addr.ssi,
                    ts,
                }),
            });
        }
    }

    fn push_network_circuit_media_ready(queue: &mut MessageQueue, brew_uuid: uuid::Uuid, call_id: u16, ts: u8) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Brew,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady { brew_uuid, call_id, ts }),
        });
    }

    fn complete_pending_network_individual_connect(&mut self, queue: &mut MessageQueue, pending: PendingNetworkIndividualConnect) {
        let Some(call_snapshot) = self.individual_calls.get(&pending.call_id).cloned() else {
            tracing::debug!(
                "CMCE: Brew private connect call_id={} disappeared before media-ready completion",
                pending.call_id
            );
            return;
        };

        // EN 300 392-2 clauses 14.5.1.1.1/14.5.1.1.2 define the local MS
        // state change on D-CONNECT ACKNOWLEDGE/D-CONNECT. Annex D.4's
        // conservative direct-setup example waits for local acknowledgement
        // before authorizing the opposite side. For Brew-origin simplex where
        // the external caller already owns the initial floor, field terminals
        // may not BL-ACK D-CONNECT ACK quickly enough; once RF transmission is
        // confirmed, keep the call alive and let Brew floor control drive
        // subsequent speech permission.
        if let Err(err) = self.fsm_individual_transition_to_active(pending.call_id) {
            match err {
                IndividualTransitionError::UnknownCall(_) => {
                    tracing::warn!("CMCE: Brew private connect activation unknown call_id={}", pending.call_id);
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::warn!(
                        "CMCE: Brew private connect activation rejected call_id={} from state {:?}",
                        pending.call_id,
                        state
                    );
                }
                IndividualTransitionError::MissingBrewUuid(_)
                | IndividualTransitionError::DuplicateCall(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }

        self.apply_brew_simplex_initial_floor(
            queue,
            pending.call_id,
            pending.local_addr,
            pending.peer_addr,
            pending.ts,
            pending.simplex_duplex,
            pending.grant,
        );

        if pending.kind == PendingNetworkIndividualConnectKind::LocalCallerDConnect {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
                    brew_uuid: pending.brew_uuid,
                    grant: pending.grant.into_raw() as u8,
                    permission: pending.permission,
                }),
            });
        }

        Self::push_network_circuit_media_ready(queue, pending.brew_uuid, pending.call_id, pending.ts);

        let completion_basis = if pending.reporter.is_acknowledged() {
            "local L2 ACK"
        } else {
            "local RF transmission"
        };
        tracing::info!(
            "CMCE: Brew private connect media-ready after {} call_id={} uuid={} local_issi={} peer_issi={} kind={:?}",
            completion_basis,
            pending.call_id,
            pending.brew_uuid,
            pending.local_addr.ssi,
            pending.peer_addr.ssi,
            pending.kind
        );

        let _ = call_snapshot;
    }

    fn pending_network_simplex_connect_can_complete_on_tx(pending: &PendingNetworkIndividualConnect) -> bool {
        // Keep Brew private simplex independent from duplex: simplex has
        // explicit floor ownership and may complete once the initial owner is
        // unambiguous and the local connect PDU reached RF.
        !pending.simplex_duplex
            && matches!(
                (pending.kind, pending.grant),
                (
                    PendingNetworkIndividualConnectKind::LocalCalledDConnectAck,
                    TransmissionGrant::GrantedToOtherUser
                ) | (PendingNetworkIndividualConnectKind::LocalCallerDConnect, TransmissionGrant::Granted)
            )
    }

    fn pending_network_duplex_connect_can_complete_on_tx(pending: &PendingNetworkIndividualConnect) -> bool {
        // Duplex does not use simplex floor-control. Brew bridge media may be
        // opened after the local connect PDU has reached RF on either leg:
        // D-CONNECT for local-origin calls, D-CONNECT ACK for network-origin.
        pending.simplex_duplex
            && matches!(
                (pending.kind, pending.grant),
                (PendingNetworkIndividualConnectKind::LocalCallerDConnect, TransmissionGrant::Granted)
                    | (
                        PendingNetworkIndividualConnectKind::LocalCalledDConnectAck,
                        TransmissionGrant::Granted
                    )
            )
    }

    fn pending_network_individual_connect_can_complete_on_tx(pending: &PendingNetworkIndividualConnect) -> bool {
        Self::pending_network_simplex_connect_can_complete_on_tx(pending)
            || Self::pending_network_duplex_connect_can_complete_on_tx(pending)
    }

    pub(in crate::cmce::subentities::cc_bs) fn drain_pending_network_individual_connects(&mut self, queue: &mut MessageQueue) {
        let ready: Vec<(u16, PendingNetworkConnectAction)> = self
            .pending_network_individual_connects
            .iter()
            .filter_map(|(&call_id, pending)| {
                if pending.reporter.is_acknowledged()
                    || (Self::pending_network_individual_connect_can_complete_on_tx(pending) && pending.reporter.is_transmitted())
                {
                    Some((call_id, PendingNetworkConnectAction::Complete))
                } else if pending.reporter.is_in_final_state()
                    || pending.started_at.age(self.dltime) >= NETWORK_INDIVIDUAL_CONNECT_PENDING_TIMEOUT_TIMESLOTS
                {
                    Some((call_id, PendingNetworkConnectAction::Fail))
                } else {
                    None
                }
            })
            .collect();

        for (call_id, action) in ready {
            let Some(pending) = self.pending_network_individual_connects.remove(&call_id) else {
                continue;
            };
            match action {
                PendingNetworkConnectAction::Complete => self.complete_pending_network_individual_connect(queue, pending),
                PendingNetworkConnectAction::Fail => {
                    tracing::warn!(
                        "CMCE: Brew private connect local RF leg did not receive L2 ACK for call_id={} uuid={} kind={:?}; releasing",
                        pending.call_id,
                        pending.brew_uuid,
                        pending.kind
                    );
                    self.release_individual_call(queue, pending.call_id, DisconnectCause::AcknowledgedServiceNotComplete);
                }
            }
        }
    }

    /// Handle network-initiated circuit setup request (Brew -> local called MS).
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_network_circuit_setup_request(
        &mut self,
        queue: &mut MessageQueue,
        brew_uuid: uuid::Uuid,
        call: NetworkCircuitCall,
    ) {
        let called_addr = TetraAddress::new(call.destination, SsiType::Issi);
        if call.destination == 0 {
            tracing::info!(
                "CMCE: rejecting Brew setup request uuid={} src={} dst=0 number='{}' (missing called ISSI)",
                brew_uuid,
                call.source_issi,
                call.number
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject {
                    brew_uuid,
                    cause: DisconnectCause::CalledPartyNotReachable.into_raw() as u8,
                }),
            });
            return;
        }

        if !self.subscriber_groups.contains_key(&called_addr.ssi) {
            tracing::info!(
                "CMCE: rejecting Brew setup request uuid={} src={} dst={} number='{}' (called ISSI not registered locally)",
                brew_uuid,
                call.source_issi,
                call.destination,
                call.number
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject {
                    brew_uuid,
                    cause: DisconnectCause::CalledPartyNotReachable.into_raw() as u8,
                }),
            });
            return;
        }

        if Self::is_preemptive_call_priority(call.priority) {
            // EN 300 392-2 table 14.46 defines priorities 12..=15 as
            // pre-emptive, while clause 14.5.1.2.1 f) makes private-call
            // interruption conditional on SwMI support. The current
            // configurable interruption path is group-call D-TX-INTERRUPT, not
            // individual-call pre-emption, so reject the network-origin
            // private setup instead of downgrading the requested semantics.
            tracing::info!(
                "CMCE: rejecting Brew setup request uuid={} src={} dst={} priority={} (private call interruption not supported)",
                brew_uuid,
                call.source_issi,
                call.destination,
                call.priority
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject {
                    brew_uuid,
                    cause: DisconnectCause::RequestedServiceNotAvailable.into_raw() as u8,
                }),
            });
            return;
        }

        if let Some((active_call_id, state)) = self.find_individual_call_by_issi(called_addr.ssi) {
            tracing::info!(
                "CMCE: rejecting Brew setup request uuid={} src={} dst={} number='{}' (called ISSI busy in call_id={} state={:?})",
                brew_uuid,
                call.source_issi,
                call.destination,
                call.number,
                active_call_id,
                state
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject {
                    brew_uuid,
                    cause: DisconnectCause::CalledPartyBusy.into_raw() as u8,
                }),
            });
            return;
        }

        let communication = CommunicationType::try_from(call.communication as u64).unwrap_or(CommunicationType::P2p);
        let simplex_duplex = call.duplex != 0;

        let occupied_call_ids = self.occupied_call_ids();
        let circuit_called = {
            let mut state = self.config.state_write();
            match self.circuits.allocate_circuit_with_allocator_duplex_avoiding(
                Direction::Both,
                communication,
                simplex_duplex,
                &mut state.timeslot_alloc,
                TimeslotOwner::Cmce,
                &occupied_call_ids,
            ) {
                Ok(circuit) => circuit.clone(),
                Err(e) => {
                    tracing::info!(
                        "CMCE: rejecting Brew setup request uuid={} src={} dst={} (allocation failed: {:?})",
                        brew_uuid,
                        call.source_issi,
                        call.destination,
                        e
                    );
                    queue.push_back(SapMsg {
                        sap: Sap::Control,
                        src: TetraEntity::Cmce,
                        dest: TetraEntity::Brew,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject {
                            brew_uuid,
                            cause: DisconnectCause::CongestionInInfrastructure.into_raw() as u8,
                        }),
                    });
                    return;
                }
            }
        };

        let call_id = circuit_called.call_id;
        let ts = circuit_called.ts;
        let usage = circuit_called.usage;
        let call_timeout = Self::network_circuit_call_timeout(call.timeout, CallTimeout::T5m);
        let hook_method_selection = Self::network_circuit_hook_method(call.method);
        let circuit_mode = CircuitModeType::try_from(call.mode as u64).unwrap_or(CircuitModeType::TchS);
        let external_subscriber_number = Self::encode_external_subscriber_number(&call.number);
        let calling_issi = call.source_issi;

        tracing::info!(
            "CMCE: accepting Brew setup request uuid={} call_id={} src={} dst={} ts={} duplex={} number='{}'",
            brew_uuid,
            call_id,
            call.source_issi,
            call.destination,
            ts,
            simplex_duplex,
            call.number
        );

        // Acknowledge setup to Brew first so network call state progresses while local MS is alerted.
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Brew,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { brew_uuid }),
        });

        let d_setup = DSetup {
            call_identifier: call_id,
            call_time_out: call_timeout,
            hook_method_selection,
            simplex_duplex_selection: simplex_duplex,
            basic_service_information: BasicServiceInformation {
                circuit_mode_type: circuit_mode,
                encryption_flag: false,
                communication_type: communication,
                slots_per_frame: None,
                speech_service: Some(call.service),
            },
            transmission_grant: TransmissionGrant::NotGranted,
            transmission_request_permission: false,
            call_priority: call.priority,
            notification_indicator: None,
            temporary_address: None,
            calling_party_address_ssi: Some(call.source_issi),
            calling_party_extension: None,
            external_subscriber_number,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };
        tracing::debug!("-> {:?}", d_setup);

        self.cached_setups.insert(
            call_id,
            CachedSetup {
                pdu: d_setup,
                dest_addr: called_addr,
                resend: false, // no late-entry resends for individual calls
                last_resend_reporter: None,
                is_individual: true,
            },
        );

        let d_setup_ref = &self.cached_setups.get(&call_id).unwrap().pdu;
        let mut setup_sdu = BitBuffer::new_autoexpand(80);
        d_setup_ref.to_bitbuf(&mut setup_sdu).expect("Failed to serialize DSetup");
        setup_sdu.seek(0);
        let setup_msg = Self::build_sapmsg(setup_sdu, None, called_addr, Layer2Service::Unacknowledged, None);
        queue.push_back(setup_msg);

        let create_result = self.fsm_individual_create_setup_call(
            call_id,
            IndividualCall {
                calling_addr: TetraAddress::new(calling_issi, SsiType::Issi),
                called_addr,
                calling_handle: 0,
                calling_link_id: 0,
                calling_endpoint_id: 0,
                called_handle: None,
                called_link_id: None,
                called_endpoint_id: None,
                calling_ts: ts,
                called_ts: ts,
                calling_usage: usage,
                called_usage: usage,
                simplex_duplex,
                request_to_transmit_send_data: false,
                state: IndividualCallState::IncomingSetupPending,
                setup_timer_started: Some(self.dltime),
                setup_timeout: Some(CallTimeoutSetupPhase::T60s),
                active_timer_started: None,
                call_timeout,
                called_over_brew: false,
                calling_over_brew: true,
                brew_uuid: Some(brew_uuid),
                network_call: Some(call),
                connect_request_sent: false,
                floor_holder: None,
                last_floor_holder: None,
                queued_tx_demand: None,
            },
        );
        if create_result.is_ok() {
            self.emit(crate::net_telemetry::TelemetryEvent::IndividualCallStarted {
                call_id,
                calling_issi,
                called_issi: called_addr.ssi,
                simplex: !simplex_duplex,
                ts,
                secondary_ts: None,
            });
        } else if let Err(err) = create_result {
            match err {
                IndividualTransitionError::DuplicateCall(_) => {
                    tracing::warn!("CMCE: duplicate call_id={} while creating inbound Brew setup", call_id);
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::warn!(
                        "CMCE: inbound Brew setup call_id={} creation rejected for state {:?}",
                        call_id,
                        state
                    );
                }
                IndividualTransitionError::UnknownCall(_)
                | IndividualTransitionError::MissingBrewUuid(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }
    }

    /// Handle network circuit connect request (Brew -> local called MS).
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_network_circuit_connect_request(
        &mut self,
        queue: &mut MessageQueue,
        brew_uuid: uuid::Uuid,
        call_info: NetworkCircuitCall,
    ) {
        let Some((call_id, call)) = self.find_brew_individual_call(brew_uuid) else {
            tracing::debug!("CMCE: Brew connect request for unknown uuid={}", brew_uuid);
            return;
        };

        if call.calling_over_brew {
            tracing::warn!(
                "CMCE: unexpected Brew CONNECT_REQUEST for Brew-originated call uuid={} call_id={}, treating as CONNECT_CONFIRM",
                brew_uuid,
                call_id
            );
            self.fsm_on_network_circuit_connect_confirm(queue, brew_uuid, call_info.grant, call_info.permission);
            return;
        }

        if call.is_active() {
            tracing::trace!("CMCE: Brew connect request for active call_id={}, ignoring", call_id);
            return;
        }

        tracing::info!(
            "CMCE: Brew connect request uuid={} call_id={} dst={} number='{}'",
            brew_uuid,
            call_id,
            call_info.destination,
            call_info.number
        );

        let connect_timeout = Self::network_circuit_call_timeout(call_info.timeout, call.call_timeout);
        let hook_method_selection = Self::network_circuit_hook_method(call_info.method);

        if let Err(err) = self.fsm_individual_set_network_call(call_id, call_info.clone()) {
            match err {
                IndividualTransitionError::UnknownCall(_) => {
                    tracing::warn!("CMCE: Brew connect request state update unknown call_id={}", call_id);
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::warn!(
                        "CMCE: Brew connect request state update rejected call_id={} from state {:?}",
                        call_id,
                        state
                    );
                }
                IndividualTransitionError::DuplicateCall(_)
                | IndividualTransitionError::MissingBrewUuid(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }

        let mut calling_timeslots = [false; 4];
        calling_timeslots[call.calling_ts as usize - 1] = true;
        let chan_alloc_calling = CmceChanAllocReq {
            usage: Some(call.calling_usage),
            alloc_type: ChanAllocType::Replace,
            carrier: None,
            timeslots: calling_timeslots,
            ul_dl_assigned: UlDlAssignment::Both,
        };

        let grant_enum = Self::network_circuit_grant(call_info.grant);
        let d_connect = DConnect {
            call_identifier: call_id,
            call_time_out: connect_timeout,
            hook_method_selection,
            simplex_duplex_selection: call.simplex_duplex,
            transmission_grant: grant_enum,
            transmission_request_permission: call_info.permission != 0,
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

        let reporter = TxReporter::new();
        let connect_msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: connect_sdu,
                handle: call.calling_handle,
                endpoint_id: call.calling_endpoint_id,
                link_id: call.calling_link_id,
                layer2service: Layer2Service::Acknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                unacked_bl_repetitions: None,
                chan_alloc: Some(chan_alloc_calling),
                main_address: call.calling_addr,
                tx_reporter: Some(reporter.clone()),
            }),
        };
        queue.push_back(connect_msg);

        let circuit = CmceCircuit {
            ts_created: self.dltime,
            direction: Direction::Both,
            ts: call.calling_ts,
            call_id,
            usage: call.calling_usage,
            circuit_mode: CircuitModeType::TchS,
            comm_type: CommunicationType::P2p,
            simplex_duplex: call.simplex_duplex,
            speech_service: Some(0),
            etee_encrypted: false,
        };
        Self::signal_umac_circuit_open(queue, &circuit, None, CircuitDlMediaSource::SwMI, Some(call.calling_addr));

        self.pending_network_individual_connects.insert(
            call_id,
            PendingNetworkIndividualConnect {
                reporter,
                brew_uuid,
                call_id,
                ts: call.calling_ts,
                local_addr: call.calling_addr,
                peer_addr: call.called_addr,
                simplex_duplex: call.simplex_duplex,
                grant: grant_enum,
                permission: call_info.permission,
                started_at: self.dltime,
                kind: PendingNetworkIndividualConnectKind::LocalCallerDConnect,
            },
        );
    }

    /// Handle network circuit connect confirm (Brew -> local calling MS).
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_network_circuit_connect_confirm(
        &mut self,
        queue: &mut MessageQueue,
        brew_uuid: uuid::Uuid,
        grant: u8,
        permission: u8,
    ) {
        let Some((call_id, call)) = self.find_brew_individual_call(brew_uuid) else {
            tracing::debug!(
                "CMCE: Brew connect confirm for unknown uuid={} grant={} permission={}",
                brew_uuid,
                grant,
                permission
            );
            return;
        };

        if !call.calling_over_brew {
            tracing::trace!(
                "CMCE: ignoring unexpected Brew connect confirm for local-origin call uuid={} call_id={}",
                brew_uuid,
                call_id
            );
            return;
        }

        if call.is_active() {
            tracing::trace!("CMCE: Brew connect confirm for active call_id={}, ignoring", call_id);
            return;
        }

        let (Some(called_handle), Some(called_link_id), Some(called_endpoint_id)) =
            (call.called_handle, call.called_link_id, call.called_endpoint_id)
        else {
            tracing::warn!(
                "CMCE: Brew connect confirm uuid={} call_id={} before local U-CONNECT context is known",
                brew_uuid,
                call_id
            );
            return;
        };

        tracing::info!(
            "CMCE: Brew connect confirm uuid={} call_id={} grant={} permission={}",
            brew_uuid,
            call_id,
            grant,
            permission
        );

        let mut called_timeslots = [false; 4];
        called_timeslots[call.called_ts as usize - 1] = true;
        let chan_alloc_called = CmceChanAllocReq {
            usage: Some(call.called_usage),
            alloc_type: ChanAllocType::Replace,
            carrier: None,
            timeslots: called_timeslots,
            ul_dl_assigned: UlDlAssignment::Both,
        };

        let grant_enum = if call.calling_over_brew && !call.simplex_duplex {
            let origin_grant = call
                .network_call
                .as_ref()
                .map(|call_info| Self::network_circuit_grant(call_info.grant))
                .unwrap_or_else(|| Self::network_circuit_grant(grant));
            Self::opposite_private_simplex_grant(origin_grant)
        } else {
            Self::network_circuit_grant(grant)
        };
        let d_connect_ack = DConnectAcknowledge {
            call_identifier: call_id,
            call_time_out: call.call_timeout,
            transmission_grant: grant_enum,
            transmission_request_permission: permission != 0,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };

        tracing::info!("-> {:?}", d_connect_ack);
        let mut ack_sdu = BitBuffer::new_autoexpand(28);
        d_connect_ack
            .to_bitbuf(&mut ack_sdu)
            .expect("Failed to serialize DConnectAcknowledge");
        ack_sdu.seek(0);

        let reporter = TxReporter::new();
        let ack_msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: ack_sdu,
                handle: called_handle,
                endpoint_id: called_endpoint_id,
                link_id: called_link_id,
                layer2service: Layer2Service::Acknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                unacked_bl_repetitions: None,
                chan_alloc: Some(chan_alloc_called),
                main_address: call.called_addr,
                tx_reporter: Some(reporter.clone()),
            }),
        };
        queue.push_back(ack_msg);

        let (circuit_mode, comm_type, speech_service, etee_encrypted) = if let Some(cached) = self.cached_setups.get(&call_id) {
            (
                cached.pdu.basic_service_information.circuit_mode_type,
                cached.pdu.basic_service_information.communication_type,
                cached.pdu.basic_service_information.speech_service,
                cached.pdu.basic_service_information.encryption_flag,
            )
        } else {
            (CircuitModeType::TchS, CommunicationType::P2p, Some(0), false)
        };

        let circuit = CmceCircuit {
            ts_created: self.dltime,
            direction: Direction::Both,
            ts: call.called_ts,
            call_id,
            usage: call.called_usage,
            circuit_mode,
            comm_type,
            simplex_duplex: call.simplex_duplex,
            speech_service,
            etee_encrypted,
        };
        Self::signal_umac_circuit_open(queue, &circuit, None, CircuitDlMediaSource::SwMI, Some(call.called_addr));

        self.pending_network_individual_connects.insert(
            call_id,
            PendingNetworkIndividualConnect {
                reporter,
                brew_uuid,
                call_id,
                ts: call.called_ts,
                local_addr: call.called_addr,
                peer_addr: call.calling_addr,
                simplex_duplex: call.simplex_duplex,
                grant: grant_enum,
                permission,
                started_at: self.dltime,
                kind: PendingNetworkIndividualConnectKind::LocalCalledDConnectAck,
            },
        );
    }

    /// Handle Brew simplex floor grant on an active private circuit.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_network_circuit_simplex_granted(
        &mut self,
        queue: &mut MessageQueue,
        brew_uuid: uuid::Uuid,
        grant: u8,
        permission: u8,
    ) {
        let Some((call_id, call)) = self.find_brew_individual_call(brew_uuid) else {
            tracing::debug!(
                "CMCE: Brew SIMPLEX_GRANTED for unknown uuid={} grant={} permission={}",
                brew_uuid,
                grant,
                permission
            );
            return;
        };

        if call.simplex_duplex {
            tracing::debug!(
                "CMCE: ignoring Brew SIMPLEX_GRANTED for duplex call_id={} uuid={}",
                call_id,
                brew_uuid
            );
            return;
        }
        if !call.is_active() {
            tracing::debug!(
                "CMCE: ignoring Brew SIMPLEX_GRANTED for inactive call_id={} uuid={} state={:?}",
                call_id,
                brew_uuid,
                call.state
            );
            return;
        }

        let Some((local_addr, local_ts, local_usage, peer_addr)) = Self::brew_private_local_and_peer_legs(&call) else {
            tracing::warn!(
                "CMCE: Brew SIMPLEX_GRANTED call_id={} uuid={} has no external private leg",
                call_id,
                brew_uuid
            );
            return;
        };

        let grant_enum = Self::network_circuit_grant(grant);
        tracing::info!(
            "CMCE: Brew SIMPLEX_GRANTED uuid={} call_id={} local_issi={} peer_issi={} grant={:?} permission={}",
            brew_uuid,
            call_id,
            local_addr.ssi,
            peer_addr.ssi,
            grant_enum,
            permission
        );

        match grant_enum {
            TransmissionGrant::Granted => {
                if let Some(active) = self.individual_calls.get_mut(&call_id) {
                    active.set_floor_holder(local_addr.ssi);
                }
                Self::push_individual_d_tx_granted(
                    queue,
                    call_id,
                    local_addr,
                    local_ts,
                    local_usage,
                    UlDlAssignment::Both,
                    TransmissionGrant::Granted,
                    local_addr.ssi,
                );
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Umac,
                    msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                        call_id,
                        source_issi: local_addr.ssi,
                        dest_gssi: peer_addr.ssi,
                        ts: local_ts,
                    }),
                });
            }
            TransmissionGrant::GrantedToOtherUser => {
                if let Some(active) = self.individual_calls.get_mut(&call_id) {
                    active.set_floor_holder(peer_addr.ssi);
                }
                Self::push_individual_d_tx_granted(
                    queue,
                    call_id,
                    local_addr,
                    local_ts,
                    local_usage,
                    UlDlAssignment::Both,
                    TransmissionGrant::GrantedToOtherUser,
                    peer_addr.ssi,
                );
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Umac,
                    msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                        call_id,
                        source_issi: peer_addr.ssi,
                        dest_gssi: local_addr.ssi,
                        ts: local_ts,
                    }),
                });
            }
            TransmissionGrant::RequestQueued | TransmissionGrant::NotGranted => {
                let current_speaker = call.floor_holder.unwrap_or(peer_addr.ssi);
                Self::push_individual_d_tx_granted(
                    queue,
                    call_id,
                    local_addr,
                    local_ts,
                    local_usage,
                    UlDlAssignment::Dl,
                    grant_enum,
                    current_speaker,
                );
            }
        }
    }

    /// Handle Brew simplex idle on an active private circuit.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_network_circuit_simplex_idle(
        &mut self,
        queue: &mut MessageQueue,
        brew_uuid: uuid::Uuid,
        grant: u8,
        permission: u8,
    ) {
        let Some((call_id, call)) = self.find_brew_individual_call(brew_uuid) else {
            tracing::debug!(
                "CMCE: Brew SIMPLEX_IDLE for unknown uuid={} grant={} permission={}",
                brew_uuid,
                grant,
                permission
            );
            return;
        };

        if call.simplex_duplex || !call.is_active() {
            tracing::debug!(
                "CMCE: ignoring Brew SIMPLEX_IDLE for call_id={} uuid={} duplex={} state={:?}",
                call_id,
                brew_uuid,
                call.simplex_duplex,
                call.state
            );
            return;
        }

        let Some((local_addr, local_ts, local_usage, peer_addr)) = Self::brew_private_local_and_peer_legs(&call) else {
            tracing::warn!(
                "CMCE: Brew SIMPLEX_IDLE call_id={} uuid={} has no external private leg",
                call_id,
                brew_uuid
            );
            return;
        };

        tracing::info!(
            "CMCE: Brew SIMPLEX_IDLE uuid={} call_id={} local_issi={} peer_issi={} grant={} permission={}",
            brew_uuid,
            call_id,
            local_addr.ssi,
            peer_addr.ssi,
            grant,
            permission
        );

        if call.floor_holder.is_some_and(|holder| holder != peer_addr.ssi) {
            tracing::debug!(
                "CMCE: Brew SIMPLEX_IDLE call_id={} ignored because current floor holder is {:?}",
                call_id,
                call.floor_holder
            );
            return;
        }

        if let Some(requester) = call.queued_tx_demand {
            if let Some(active) = self.individual_calls.get_mut(&call_id) {
                active.set_floor_holder(requester.ssi);
                active.queued_tx_demand = None;
            }

            tracing::info!(
                "CMCE: Brew SIMPLEX_IDLE uuid={} call_id={} -> granting queued local requester ISSI {}",
                brew_uuid,
                call_id,
                requester.ssi
            );

            Self::push_individual_d_tx_granted(
                queue,
                call_id,
                local_addr,
                local_ts,
                local_usage,
                UlDlAssignment::Both,
                TransmissionGrant::Granted,
                local_addr.ssi,
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                    call_id,
                    source_issi: local_addr.ssi,
                    dest_gssi: peer_addr.ssi,
                    ts: local_ts,
                }),
            });
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexGranted {
                    brew_uuid,
                    grant: TransmissionGrant::GrantedToOtherUser.into_raw() as u8,
                    permission: 0,
                }),
            });
            return;
        }

        if let Some(active) = self.individual_calls.get_mut(&call_id) {
            active.clear_floor_holder();
        }
        Self::push_individual_d_tx_ceased(queue, call_id, local_addr, local_ts, local_usage);
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id, ts: local_ts }),
        });
    }

    fn brew_private_local_and_peer_legs(call: &IndividualCall) -> Option<(TetraAddress, u8, u8, TetraAddress)> {
        if call.called_over_brew && !call.calling_over_brew {
            Some((call.calling_addr, call.calling_ts, call.calling_usage, call.called_addr))
        } else if call.calling_over_brew && !call.called_over_brew {
            Some((call.called_addr, call.called_ts, call.called_usage, call.calling_addr))
        } else {
            None
        }
    }

    fn opposite_private_simplex_grant(grant: TransmissionGrant) -> TransmissionGrant {
        match grant {
            TransmissionGrant::Granted => TransmissionGrant::GrantedToOtherUser,
            TransmissionGrant::GrantedToOtherUser => TransmissionGrant::Granted,
            TransmissionGrant::RequestQueued => TransmissionGrant::RequestQueued,
            TransmissionGrant::NotGranted => TransmissionGrant::NotGranted,
        }
    }

    /// Handle network-initiated group call start.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_network_call_start(
        &mut self,
        queue: &mut MessageQueue,
        brew_uuid: uuid::Uuid,
        source_issi: u32,
        dest_gssi: u32,
        priority: u8,
    ) {
        if !net_brew::is_brew_gssi_routable(&self.config, dest_gssi) {
            tracing::warn!(
                "CMCE: fsm_on_network_call_start called for non-routable gssi={}, uuid={}, dropping",
                dest_gssi,
                brew_uuid
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
            });
            return;
        }

        if !self.has_local_listener(dest_gssi) {
            tracing::info!(
                "CMCE: ignoring network call start uuid={} gssi={} (no local RF listeners)",
                brew_uuid,
                dest_gssi
            );
            self.drop_group_calls_if_unlistened(queue, dest_gssi);

            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
            });
            return;
        }

        if !self.config.config().cell.transmission_interruption_enabled && Self::is_preemptive_call_priority(priority) {
            // EN 300 392-2 table 14.46 defines call priority 12..=15 as
            // pre-emptive, and clause 14.5.2.2.1 f) only permits transmission
            // interruption when the SwMI supports it. Keep the default-off
            // configuration fail-closed for new network-origin group calls.
            tracing::info!(
                "CMCE: rejecting network group call uuid={} gssi={} source={} priority={} (transmission interruption disabled)",
                brew_uuid,
                dest_gssi,
                source_issi,
                priority
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
            });
            return;
        }

        // Speaker change for an existing GSSI call
        if let Some((call_id, old_speaker)) = self
            .active_calls
            .iter()
            .find(|(id, c)| c.dest_gssi == dest_gssi && !self.pending_group_releases.contains_key(id))
            .map(|(id, c)| (*id, c.source_issi))
        {
            // If a local MS currently holds the floor, only allow network
            // preemption when explicitly configured and when the incoming call
            // priority is an ETSI pre-emptive priority strictly above the
            // current call. See EN 300 392-2 clauses 14.5.2.2.1 f) and 14.8.12.
            if let Some(call) = self.active_calls.get(&call_id) {
                let local_speaker_has_floor = call.tx_active && self.subscriber_affiliated_to_group(call.source_issi, call.dest_gssi);
                if local_speaker_has_floor {
                    let interruption_enabled = self.config.config().cell.transmission_interruption_enabled;
                    let may_preempt = interruption_enabled && Self::is_preemptive_call_priority(priority) && priority > call.priority;

                    if !may_preempt {
                        tracing::info!(
                            "CMCE: rejecting network speaker change gssi={} src={} — \
                             local MS {} holds floor (incoming prio={}, current prio={}, interruption_enabled={})",
                            dest_gssi,
                            source_issi,
                            call.source_issi,
                            priority,
                            call.priority,
                            interruption_enabled
                        );
                        queue.push_back(SapMsg {
                            sap: Sap::Control,
                            src: TetraEntity::Cmce,
                            dest: TetraEntity::Brew,
                            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
                        });
                        return;
                    }
                }
            }

            tracing::info!(
                "CMCE: network call speaker change gssi={} new_speaker={} (was {})",
                dest_gssi,
                source_issi,
                old_speaker
            );

            if let Err(err) = self.fsm_group_on_network_call_start(queue, call_id, brew_uuid, source_issi, priority) {
                match err {
                    GroupTransitionError::UnknownCall(_) => {
                        tracing::warn!(
                            "CMCE: network speaker change gssi={} resolved unknown call_id={}",
                            dest_gssi,
                            call_id
                        );
                    }
                    GroupTransitionError::InvalidTransition { state, .. } => {
                        tracing::warn!("CMCE: network speaker change rejected call_id={} from state {:?}", call_id, state);
                    }
                    GroupTransitionError::NotCurrentSpeaker { .. } => {
                        tracing::debug!(
                            "CMCE: network speaker change produced unexpected NotCurrentSpeaker for call_id={}",
                            call_id
                        );
                    }
                    GroupTransitionError::MissingCachedSetup(_) => {
                        tracing::debug!(
                            "CMCE: network speaker change call_id={} without cached setup (not required for this transition)",
                            call_id
                        );
                    }
                }
            }
            return;
        }

        // New network call - allocate circuit
        let occupied_call_ids = self.occupied_call_ids();
        let circuit = match {
            let mut state = self.config.state_write();
            self.circuits.allocate_circuit_with_allocator_duplex_avoiding(
                Direction::Both,
                CommunicationType::P2Mp,
                false,
                &mut state.timeslot_alloc,
                TimeslotOwner::Cmce,
                &occupied_call_ids,
            )
        } {
            Ok(c) => c.clone(),
            Err(err) => {
                tracing::warn!("CMCE: failed to allocate circuit for network call: {:?}", err);
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Brew,
                    msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
                });
                return;
            }
        };

        let call_id = circuit.call_id;
        let ts = circuit.ts;
        let usage = circuit.usage;

        tracing::info!(
            "CMCE: starting NEW network call brew_uuid={} gssi={} speaker={} ts={} call_id={}",
            brew_uuid,
            dest_gssi,
            source_issi,
            ts,
            call_id
        );

        Self::signal_umac_circuit_open_with_secondary(
            queue,
            &circuit,
            None,
            CircuitDlMediaSource::SwMI,
            Some(TetraAddress::new(dest_gssi, SsiType::Gssi)),
            vec![TetraAddress::issi(source_issi)],
        );

        tracing::debug!(
            "CMCE: sending D-SETUP for NEW call call_id={} gssi={} (network-initiated)",
            call_id,
            dest_gssi
        );

        let dest_addr = TetraAddress::new(dest_gssi, SsiType::Gssi);
        let d_setup = DSetup {
            call_identifier: call_id,
            call_time_out: self.config_call_timeout(),
            hook_method_selection: false,
            simplex_duplex_selection: false,
            basic_service_information: BasicServiceInformation {
                circuit_mode_type: CircuitModeType::TchS,
                encryption_flag: false,
                communication_type: CommunicationType::P2Mp,
                slots_per_frame: None,
                speech_service: Some(0),
            },
            transmission_grant: TransmissionGrant::GrantedToOtherUser,
            transmission_request_permission: false,
            call_priority: priority,
            notification_indicator: None,
            temporary_address: None,
            calling_party_address_ssi: Some(source_issi),
            calling_party_extension: None,
            external_subscriber_number: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };

        self.cached_setups.insert(
            call_id,
            CachedSetup {
                pdu: d_setup,
                dest_addr: dest_addr.clone(),
                resend: true,
                last_resend_reporter: None,
                is_individual: false,
            },
        );
        let d_setup_ref = &self.cached_setups.get(&call_id).unwrap().pdu;

        let setup_reporter = TxReporter::new_unacked();
        let (setup_sdu, setup_chan_alloc) = Self::build_d_setup_prim(d_setup_ref, usage, ts, UlDlAssignment::Both);
        let setup_msg = Self::build_sapmsg(
            setup_sdu,
            Some(setup_chan_alloc),
            dest_addr.clone(),
            Layer2Service::Unacknowledged,
            Some(setup_reporter.clone()),
        );
        queue.push_back(setup_msg);

        self.active_calls.insert(
            call_id,
            ActiveCall::new_network(
                brew_uuid,
                dest_gssi,
                source_issi,
                ts,
                usage,
                self.dltime,
                self.config_call_timeout(),
                priority,
            ),
        );

        // Emit telemetry so dashboard shows Brew-initiated calls
        self.emit(crate::net_telemetry::TelemetryEvent::GroupCallStarted {
            call_id,
            gssi: dest_gssi,
            caller_issi: source_issi,
            ts,
        });

        self.queue_network_group_ready(call_id, brew_uuid, source_issi, dest_gssi, ts, usage, vec![setup_reporter], false);
    }
}
