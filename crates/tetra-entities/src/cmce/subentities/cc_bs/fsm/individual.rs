use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::cmce::subentities::cc_bs) enum IndividualEvent {
    CreateSetup,
    BindCalledContext,
    SetNetworkCall,
    MarkConnectRequestSent,
    Alert,
    Connect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::cmce::subentities::cc_bs) enum IndividualTransitionError {
    UnknownCall(u16),
    DuplicateCall(u16),
    InvalidTransition {
        call_id: u16,
        state: IndividualCallState,
        event: IndividualEvent,
    },
    MissingBrewUuid(u16),
    NotBrewOriginated(u16),
    ConnectRequestAlreadySent(u16),
}

impl CcBsSubentity {
    pub(in crate::cmce::subentities::cc_bs) fn private_simplex_called_ms_transmits_first(
        simplex_duplex: bool,
        hook_method_selection: bool,
        request_to_transmit_send_data: bool,
    ) -> bool {
        if simplex_duplex {
            return false;
        }

        // EN 300 392-2 table 14.74 encodes this raw bit as:
        // 0 = request to transmit/send data, 1 = request that other MS may
        // transmit/send data. Clause 14.5.1.2.1 gives that field
        // setup-method-specific meaning for the initial private-simplex floor.
        hook_method_selection && request_to_transmit_send_data
    }

    fn validate_individual_transition(
        call_id: u16,
        state: IndividualCallState,
        event: IndividualEvent,
    ) -> Result<(), IndividualTransitionError> {
        let allowed = matches!(
            (state, event),
            (IndividualCallState::CallSetupPending, IndividualEvent::BindCalledContext)
                | (IndividualCallState::IncomingSetupPending, IndividualEvent::BindCalledContext)
                | (IndividualCallState::IncomingAlerting, IndividualEvent::BindCalledContext)
                | (IndividualCallState::IncomingSetupWaitNetworkAck, IndividualEvent::BindCalledContext)
                | (IndividualCallState::CallSetupPending, IndividualEvent::SetNetworkCall)
                | (IndividualCallState::IncomingSetupPending, IndividualEvent::SetNetworkCall)
                | (IndividualCallState::IncomingAlerting, IndividualEvent::SetNetworkCall)
                | (IndividualCallState::IncomingSetupWaitNetworkAck, IndividualEvent::SetNetworkCall)
                | (IndividualCallState::CallSetupPending, IndividualEvent::MarkConnectRequestSent)
                | (IndividualCallState::IncomingSetupPending, IndividualEvent::MarkConnectRequestSent)
                | (IndividualCallState::IncomingAlerting, IndividualEvent::MarkConnectRequestSent)
                | (
                    IndividualCallState::IncomingSetupWaitNetworkAck,
                    IndividualEvent::MarkConnectRequestSent
                )
                | (IndividualCallState::CallSetupPending, IndividualEvent::Alert)
                | (IndividualCallState::IncomingSetupPending, IndividualEvent::Alert)
                | (IndividualCallState::IncomingAlerting, IndividualEvent::Alert)
                | (IndividualCallState::CallSetupPending, IndividualEvent::Connect)
                | (IndividualCallState::IncomingSetupPending, IndividualEvent::Connect)
                | (IndividualCallState::IncomingAlerting, IndividualEvent::Connect)
                | (IndividualCallState::IncomingSetupWaitNetworkAck, IndividualEvent::Connect)
        );
        if allowed {
            Ok(())
        } else {
            Err(IndividualTransitionError::InvalidTransition { call_id, state, event })
        }
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_individual_create_setup_call(
        &mut self,
        call_id: u16,
        call: IndividualCall,
    ) -> Result<(), IndividualTransitionError> {
        if self.individual_calls.contains_key(&call_id) {
            return Err(IndividualTransitionError::DuplicateCall(call_id));
        }

        if !matches!(
            call.state,
            IndividualCallState::CallSetupPending | IndividualCallState::IncomingSetupPending
        ) {
            return Err(IndividualTransitionError::InvalidTransition {
                call_id,
                state: call.state,
                event: IndividualEvent::CreateSetup,
            });
        }

        self.individual_calls.insert(call_id, call);
        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_individual_bind_called_context(
        &mut self,
        call_id: u16,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
    ) -> Result<(), IndividualTransitionError> {
        let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
            return Err(IndividualTransitionError::UnknownCall(call_id));
        };

        Self::validate_individual_transition(call_id, call_snapshot.state, IndividualEvent::BindCalledContext)?;

        if let Some(call) = self.individual_calls.get_mut(&call_id)
            && call.called_handle.is_none()
        {
            call.called_handle = Some(handle);
            call.called_link_id = Some(link_id);
            call.called_endpoint_id = Some(endpoint_id);
        }
        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_individual_set_network_call(
        &mut self,
        call_id: u16,
        network_call: NetworkCircuitCall,
    ) -> Result<(), IndividualTransitionError> {
        let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
            return Err(IndividualTransitionError::UnknownCall(call_id));
        };

        Self::validate_individual_transition(call_id, call_snapshot.state, IndividualEvent::SetNetworkCall)?;

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            call.network_call = Some(network_call);
        }
        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_individual_mark_connect_request_sent(
        &mut self,
        call_id: u16,
        network_call: NetworkCircuitCall,
    ) -> Result<(), IndividualTransitionError> {
        let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
            return Err(IndividualTransitionError::UnknownCall(call_id));
        };

        Self::validate_individual_transition(call_id, call_snapshot.state, IndividualEvent::MarkConnectRequestSent)?;

        if !call_snapshot.calling_over_brew {
            return Err(IndividualTransitionError::NotBrewOriginated(call_id));
        }
        if call_snapshot.connect_request_sent {
            return Err(IndividualTransitionError::ConnectRequestAlreadySent(call_id));
        }

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            call.connect_request_sent = true;
            call.network_call = Some(network_call);
            call.state = IndividualCallState::IncomingSetupWaitNetworkAck;
        }
        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_individual_on_alert(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        called_handle_ctx: Option<(u32, u32, u32)>, // handle, link_id, endpoint_id
        setup_timeout: CallTimeoutSetupPhase,
    ) -> Result<(), IndividualTransitionError> {
        let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
            return Err(IndividualTransitionError::UnknownCall(call_id));
        };

        Self::validate_individual_transition(call_id, call_snapshot.state, IndividualEvent::Alert)?;

        if let Some((handle, link_id, endpoint_id)) = called_handle_ctx {
            self.fsm_individual_bind_called_context(call_id, handle, link_id, endpoint_id)?;
        }

        if call_snapshot.calling_over_brew {
            let Some(brew_uuid) = call_snapshot.brew_uuid else {
                return Err(IndividualTransitionError::MissingBrewUuid(call_id));
            };

            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitAlert { brew_uuid }),
            });
        } else if !call_snapshot.is_alerted() {
            self.send_d_alert_individual(
                queue,
                call_id,
                call_snapshot.simplex_duplex,
                call_snapshot.calling_addr,
                call_snapshot.calling_handle,
                call_snapshot.calling_link_id,
                call_snapshot.calling_endpoint_id,
                setup_timeout,
            );
        }

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            call.mark_alerted(self.dltime, setup_timeout);
        }

        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_individual_transition_to_active(
        &mut self,
        call_id: u16,
    ) -> Result<(), IndividualTransitionError> {
        let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
            return Err(IndividualTransitionError::UnknownCall(call_id));
        };

        Self::validate_individual_transition(call_id, call_snapshot.state, IndividualEvent::Connect)?;

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            call.activate(self.dltime);
        }
        Ok(())
    }

    fn accept_called_simplex_offer_for_requested_duplex(&mut self, call_id: u16, source_pdu: &str) -> Option<IndividualCall> {
        let mut call_snapshot = self.individual_calls.get(&call_id).cloned()?;

        if !call_snapshot.simplex_duplex {
            return Some(call_snapshot);
        }

        // EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 allow the called
        // MS to offer simplex in U-ALERT or U-CONNECT when it cannot support
        // a requested duplex private call. Accept the offered service locally
        // so a valid simple private-call answer can continue.
        tracing::info!(
            "{} call_id={} offered simplex for requested duplex; accepting offered private-call service",
            source_pdu,
            call_id
        );
        if call_snapshot.called_ts != call_snapshot.calling_ts {
            if let Ok(circuit) = self.circuits.close_circuit(Direction::Both, call_snapshot.called_ts) {
                tracing::debug!(
                    "CMCE: released unused duplex called-side bearer call_id={} ts={} after simplex offer",
                    call_id,
                    circuit.ts
                );
            }
            self.release_timeslot(call_snapshot.called_ts);
        }
        call_snapshot.simplex_duplex = false;
        call_snapshot.called_ts = call_snapshot.calling_ts;
        call_snapshot.called_usage = call_snapshot.calling_usage;
        call_snapshot.call_timeout = self.config_call_timeout();
        if let Some(network_call) = &mut call_snapshot.network_call {
            network_call.duplex = 0;
        }

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            call.simplex_duplex = call_snapshot.simplex_duplex;
            call.called_ts = call_snapshot.called_ts;
            call.called_usage = call_snapshot.called_usage;
            call.call_timeout = call_snapshot.call_timeout;
            if let Some(network_call) = &mut call.network_call {
                network_call.duplex = 0;
            }
        }

        Some(call_snapshot)
    }

    /// Handle parsed U-ALERT.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_alert(
        &mut self,
        queue: &mut MessageQueue,
        received_tetra_address: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        pdu: UAlert,
    ) {
        let call_id = pdu.call_identifier;
        let Some(call) = self.individual_calls.get(&call_id).cloned() else {
            tracing::warn!("U-ALERT for unknown call_id={}", call_id);
            return;
        };

        if call.called_addr.ssi != received_tetra_address.ssi {
            tracing::warn!(
                "U-ALERT call_id={} from unexpected ISSI {} (expected {})",
                call_id,
                received_tetra_address.ssi,
                call.called_addr.ssi
            );
            return;
        }

        if call.simplex_duplex && !pdu.simplex_duplex_selection {
            let _ = self.accept_called_simplex_offer_for_requested_duplex(call_id, "U-ALERT");
        }

        if let Err(err) = self.fsm_individual_on_alert(queue, call_id, Some((handle, link_id, endpoint_id)), CallTimeoutSetupPhase::T60s) {
            match err {
                IndividualTransitionError::UnknownCall(_) => {
                    tracing::warn!("U-ALERT for unknown call_id={}", call_id);
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::debug!("U-ALERT call_id={} ignored due to invalid transition in state {:?}", call_id, state);
                }
                IndividualTransitionError::MissingBrewUuid(_) => {
                    tracing::warn!("CMCE: Brew-originated call_id={} missing brew_uuid on U-ALERT", call_id);
                }
                IndividualTransitionError::DuplicateCall(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }
    }

    /// Handle parsed U-CONNECT.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_connect(
        &mut self,
        queue: &mut MessageQueue,
        received_tetra_address: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        pdu: UConnect,
        received_pdu: BitBuffer,
    ) {
        let call_id = pdu.call_identifier;
        let Some(mut call_snapshot) = self.individual_calls.get(&call_id).cloned() as Option<IndividualCall> else {
            tracing::warn!("U-CONNECT for unknown call_id={}", call_id);
            return;
        };

        if call_snapshot.is_active() {
            tracing::debug!("U-CONNECT for active call_id={}, ignoring", call_id);
            return;
        }

        if call_snapshot.called_addr.ssi != received_tetra_address.ssi {
            tracing::warn!(
                "U-CONNECT call_id={} from unexpected ISSI {} (expected {})",
                call_id,
                received_tetra_address.ssi,
                call_snapshot.called_addr.ssi
            );
            return;
        }

        let requested_basic_service = self.cached_setups.get(&call_id).map(|cached| &cached.pdu.basic_service_information);
        if let Some((pointer, reason)) = Self::unsupported_u_connect_function(&pdu, requested_basic_service) {
            tracing::info!(
                "CMCE: rejecting unsupported U-CONNECT call_id={} from ISSI {}: {}; responding CMCE FUNCTION NOT SUPPORTED",
                call_id,
                received_tetra_address.ssi,
                reason
            );
            queue.push_back(Self::build_cmce_function_not_supported_element_direct(
                CmcePduTypeUl::UConnect,
                call_id,
                pointer,
                &received_pdu,
                received_tetra_address,
                handle,
                link_id,
                endpoint_id,
            ));
            return;
        }

        if call_snapshot.simplex_duplex && !pdu.simplex_duplex_selection {
            call_snapshot = self
                .accept_called_simplex_offer_for_requested_duplex(call_id, "U-CONNECT")
                .unwrap_or(call_snapshot);
        }

        if let Err(err) = self.fsm_individual_bind_called_context(call_id, handle, link_id, endpoint_id) {
            match err {
                IndividualTransitionError::UnknownCall(_) => {
                    tracing::warn!("U-CONNECT context bind failed, unknown call_id={}", call_id);
                    return;
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::debug!("U-CONNECT context bind rejected for call_id={} in state {:?}", call_id, state);
                    return;
                }
                IndividualTransitionError::DuplicateCall(_)
                | IndividualTransitionError::MissingBrewUuid(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }

        if call_snapshot.calling_over_brew {
            let Some(brew_uuid) = call_snapshot.brew_uuid else {
                tracing::warn!("CMCE: Brew-originated call_id={} missing brew_uuid on U-CONNECT", call_id);
                return;
            };

            let mut call_info = call_snapshot.network_call.clone().unwrap_or(NetworkCircuitCall {
                source_issi: call_snapshot.calling_addr.ssi,
                destination: call_snapshot.called_addr.ssi,
                number: call_snapshot.called_addr.ssi.to_string(),
                priority: 0,
                service: 0,
                mode: CircuitModeType::TchS.into_raw() as u8,
                duplex: call_snapshot.simplex_duplex as u8,
                method: pdu.hook_method_selection as u8,
                communication: CommunicationType::P2p.into_raw() as u8,
                grant: 0,
                permission: 0,
                timeout: CallTimeout::T5m.into_raw() as u8,
                ownership: 0,
                queued: 0,
            });
            call_info.duplex = pdu.simplex_duplex_selection as u8;
            call_info.method = pdu.hook_method_selection as u8;
            // Update these fields as the call is accepted
            call_info.grant = 0;
            call_info.permission = 0;

            if let Err(err) = self.fsm_individual_mark_connect_request_sent(call_id, call_info.clone()) {
                match err {
                    IndividualTransitionError::ConnectRequestAlreadySent(_) => {
                        tracing::trace!(
                            "CMCE: duplicate U-CONNECT for Brew-originated call_id={}, CONNECT_REQUEST already sent",
                            call_id
                        );
                        return;
                    }
                    IndividualTransitionError::UnknownCall(_) => {
                        tracing::warn!("CMCE: U-CONNECT Brew mark sent failed unknown call_id={}", call_id);
                        return;
                    }
                    IndividualTransitionError::InvalidTransition { state, .. } => {
                        tracing::warn!("CMCE: U-CONNECT Brew mark sent rejected call_id={} from state {:?}", call_id, state);
                        return;
                    }
                    IndividualTransitionError::NotBrewOriginated(_)
                    | IndividualTransitionError::MissingBrewUuid(_)
                    | IndividualTransitionError::DuplicateCall(_) => {
                        tracing::warn!("CMCE: U-CONNECT Brew mark sent inconsistent state for call_id={}", call_id);
                        return;
                    }
                }
            }

            tracing::info!(
                "CMCE: forwarding U-CONNECT as Brew CONNECT_REQUEST uuid={} call_id={} dst={} number='{}' grant='{}'",
                brew_uuid,
                call_id,
                call_info.destination,
                call_info.number,
                call_info.grant,
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest {
                    brew_uuid,
                    call: call_info.clone(),
                }),
            });
            return;
        }

        let calling_addr = call_snapshot.calling_addr;
        let called_addr = call_snapshot.called_addr;
        let calling_handle = call_snapshot.calling_handle;
        let calling_link_id = call_snapshot.calling_link_id;
        let calling_endpoint_id = call_snapshot.calling_endpoint_id;
        let calling_ts = call_snapshot.calling_ts;
        let called_ts = call_snapshot.called_ts;
        let calling_usage = call_snapshot.calling_usage;
        let called_usage = call_snapshot.called_usage;
        let simplex_duplex = call_snapshot.simplex_duplex;

        let Some(cached) = self.cached_setups.get(&call_id) else {
            tracing::error!("No cached D-SETUP for call_id={}", call_id);
            return;
        };
        let called_ms_transmits_first = Self::private_simplex_called_ms_transmits_first(
            simplex_duplex,
            cached.pdu.hook_method_selection,
            call_snapshot.request_to_transmit_send_data,
        );

        let mut calling_timeslots = [false; 4];
        calling_timeslots[calling_ts as usize - 1] = true;
        let mut called_timeslots = [false; 4];
        called_timeslots[called_ts as usize - 1] = true;

        // For simplex P2P: both MS initially get Both so they can receive the D-CONNECT /
        // D-CONNECT-ACK PDUs on the traffic channel. The floor (Ul/Dl restriction) is
        // enforced later via D-TX-GRANTED when either MS presses PTT (U-TX-DEMAND).
        // For duplex P2P: Both on both TS (cross-routed audio).
        let (calling_ul_dl, called_ul_dl) = (UlDlAssignment::Both, UlDlAssignment::Both);

        let chan_alloc_calling = CmceChanAllocReq {
            usage: Some(calling_usage),
            alloc_type: ChanAllocType::Replace,
            carrier: None,
            timeslots: calling_timeslots,
            ul_dl_assigned: calling_ul_dl,
        };
        let chan_alloc_called = CmceChanAllocReq {
            usage: Some(called_usage),
            alloc_type: ChanAllocType::Replace,
            carrier: None,
            timeslots: called_timeslots,
            ul_dl_assigned: called_ul_dl,
        };
        tracing::debug!(
            "P2P chan_alloc: calling ts={} usage={} slots={:?}, called ts={} usage={} slots={:?}",
            calling_ts,
            calling_usage,
            calling_timeslots,
            called_ts,
            called_usage,
            called_timeslots
        );

        // Open UMAC circuits FIRST so traffic channel is ready before MS arrives
        let circuit_calling = CmceCircuit {
            ts_created: self.dltime,
            direction: Direction::Both,
            ts: calling_ts,
            call_id,
            usage: calling_usage,
            circuit_mode: cached.pdu.basic_service_information.circuit_mode_type,
            comm_type: cached.pdu.basic_service_information.communication_type,
            simplex_duplex,
            speech_service: cached.pdu.basic_service_information.speech_service,
            etee_encrypted: cached.pdu.basic_service_information.encryption_flag,
        };

        // For P2P calls (both simplex and duplex) on different timeslots:
        // peer_ts must be set on BOTH circuits so UMAC cross-routes audio in BOTH directions.
        //
        // Simplex: only one MS transmits at a time (UL inactivity + floor control), but the
        // cross-routing path must be ready on both sides. Without peer_ts on called_ts,
        // when the called MS takes the floor (U-TX-DEMAND), its UL goes to LocalLoopback
        // instead of being routed to calling_ts — calling MS hears nothing; called MS hears
        // their own voice echoed back.
        let cross_peer_calling = if calling_ts != called_ts { Some(called_ts) } else { None };
        let cross_peer_called = if calling_ts != called_ts { Some(calling_ts) } else { None };

        let (primary_active_addr, active_secondary_addrs) = if called_ts == calling_ts && called_ms_transmits_first {
            (called_addr, vec![calling_addr])
        } else if called_ts == calling_ts {
            (calling_addr, vec![called_addr])
        } else {
            (calling_addr, Vec::new())
        };
        Self::signal_umac_circuit_open_with_secondary(
            queue,
            &circuit_calling,
            cross_peer_calling,
            CircuitDlMediaSource::LocalLoopback,
            Some(primary_active_addr),
            active_secondary_addrs,
        );

        if called_ts != calling_ts {
            let circuit_called = CmceCircuit {
                ts_created: self.dltime,
                direction: Direction::Both,
                ts: called_ts,
                call_id,
                usage: called_usage,
                circuit_mode: cached.pdu.basic_service_information.circuit_mode_type,
                comm_type: cached.pdu.basic_service_information.communication_type,
                simplex_duplex,
                speech_service: cached.pdu.basic_service_information.speech_service,
                etee_encrypted: cached.pdu.basic_service_information.encryption_flag,
            };
            Self::signal_umac_circuit_open(
                queue,
                &circuit_called,
                cross_peer_called,
                CircuitDlMediaSource::LocalLoopback,
                Some(called_addr),
            );
        }

        // D-CONNECT to calling MS. The initial floor mirrors the clause
        // 14.5.1.2.1 setup-method rule above.
        let calling_grant = if simplex_duplex || !called_ms_transmits_first {
            TransmissionGrant::Granted
        } else {
            TransmissionGrant::GrantedToOtherUser
        };
        let d_connect = DConnect {
            call_identifier: call_id,
            call_time_out: if simplex_duplex {
                CallTimeout::Infinite
            } else {
                self.config_call_timeout()
            },
            hook_method_selection: cached.pdu.hook_method_selection,
            simplex_duplex_selection: simplex_duplex,
            transmission_grant: calling_grant,
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

        // --- STEP 1: DConnect via FACCH stealing (terminal already on TCH) ---
        let connect_msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: connect_sdu,
                handle: calling_handle,
                endpoint_id: calling_endpoint_id,
                link_id: calling_link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: true,
                stealing_repeats_flag: true,
                unacked_bl_repetitions: None,
                chan_alloc: Some(chan_alloc_calling.clone()),
                main_address: calling_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(connect_msg);

        // --- STEP 2: DConnect via MCCH as fallback (terminal still on control channel) ---
        let mut connect_sdu2 = BitBuffer::new_autoexpand(30);
        d_connect
            .to_bitbuf(&mut connect_sdu2)
            .expect("Failed to serialize DConnect (fallback)");
        connect_sdu2.seek(0);
        let connect_msg2 = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: connect_sdu2,
                handle: calling_handle,
                endpoint_id: calling_endpoint_id,
                link_id: calling_link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                unacked_bl_repetitions: None,
                chan_alloc: Some(chan_alloc_calling.clone()),
                main_address: calling_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(connect_msg2);

        let called_grant = if simplex_duplex || called_ms_transmits_first {
            TransmissionGrant::Granted
        } else {
            TransmissionGrant::GrantedToOtherUser
        };

        // D-CONNECT-ACKNOWLEDGE to called MS mirrors the same initial floor
        // decision as D-CONNECT while duplex remains granted to both parties.
        let d_connect_ack = DConnectAcknowledge {
            call_identifier: call_id,
            call_time_out: if simplex_duplex {
                CallTimeout::Infinite
            } else {
                self.config_call_timeout()
            },
            transmission_grant: called_grant,
            transmission_request_permission: false,
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

        // --- STEP 1: DConnectAcknowledge via FACCH stealing (terminal already on TCH) ---
        let ack_msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: ack_sdu,
                handle,
                endpoint_id,
                link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: true,
                stealing_repeats_flag: true,
                unacked_bl_repetitions: None,
                chan_alloc: Some(chan_alloc_called.clone()),
                main_address: called_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(ack_msg);

        // --- STEP 2: DConnectAcknowledge via MCCH as fallback (terminal still on control channel) ---
        let mut ack_sdu2 = BitBuffer::new_autoexpand(28);
        d_connect_ack
            .to_bitbuf(&mut ack_sdu2)
            .expect("Failed to serialize DConnectAcknowledge (fallback)");
        ack_sdu2.seek(0);
        let ack_msg2 = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: ack_sdu2,
                handle,
                endpoint_id,
                link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                unacked_bl_repetitions: None,
                chan_alloc: Some(chan_alloc_called.clone()),
                main_address: called_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(ack_msg2);

        if let Err(err) = self.fsm_individual_transition_to_active(call_id) {
            match err {
                IndividualTransitionError::UnknownCall(_) => {
                    tracing::warn!("U-CONNECT activation failed, unknown call_id={}", call_id);
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::warn!("U-CONNECT activation rejected for call_id={} from state {:?}", call_id, state);
                }
                IndividualTransitionError::MissingBrewUuid(_)
                | IndividualTransitionError::DuplicateCall(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }

        // Set initial floor_holder for simplex P2P. This must match the
        // TransmissionGrant pair above so UL inactivity timeout is associated
        // with the MS that actually owns the floor.
        // Duplex: floor_holder stays None — both MS can transmit anytime.
        if !simplex_duplex {
            if let Some(c) = self.individual_calls.get_mut(&call_id) {
                let initial_floor_holder = if called_ms_transmits_first {
                    called_addr.ssi
                } else {
                    calling_addr.ssi
                };
                c.set_floor_holder(initial_floor_holder);
                tracing::info!(
                    "Simplex P2P call_id={} initial floor_holder = ISSI {}",
                    call_id,
                    initial_floor_holder
                );

                let (source_issi, dest_issi, floor_ts) = if called_ms_transmits_first {
                    (called_addr.ssi, calling_addr.ssi, called_ts)
                } else {
                    (calling_addr.ssi, called_addr.ssi, calling_ts)
                };
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Umac,
                    msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                        call_id,
                        source_issi,
                        dest_gssi: dest_issi,
                        ts: floor_ts,
                    }),
                });
            }
        }
    }
}
