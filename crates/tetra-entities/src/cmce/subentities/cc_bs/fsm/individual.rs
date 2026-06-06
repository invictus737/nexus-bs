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

const INDIVIDUAL_CONNECT_ACK_PENDING_TIMEOUT_TIMESLOTS: i32 = 2 * 18 * 4;
const INDIVIDUAL_CONNECT_ACK_MAX_ATTEMPTS: u8 = 5;
const INDIVIDUAL_CALLER_CONNECT_MAX_ATTEMPTS: u8 = 3;

#[derive(Clone, Copy)]
enum PendingConnectAckAction {
    CalledDeliveryComplete,
    CallerDeliveryComplete,
    RetryOrFailCalledDelivery,
    RetryOrFailCallerDelivery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrivateCalledConnectAckDelivery {
    CurrentChannel,
    AssignedChannelRecovery,
}

impl PrivateCalledConnectAckDelivery {
    fn retry_for_attempt(attempt: u8) -> Self {
        match attempt {
            1 | 2 | 4 => Self::CurrentChannel,
            _ => Self::AssignedChannelRecovery,
        }
    }

    fn retry_after_local_delivery_failure(attempt: u8) -> Self {
        match attempt {
            1 | 2 | 4 => Self::CurrentChannel,
            _ => Self::AssignedChannelRecovery,
        }
    }

    fn stealing_permission(self) -> bool {
        matches!(self, Self::AssignedChannelRecovery)
    }

    fn requires_l2_ack(self) -> bool {
        true
    }

    fn layer2_service(self) -> Layer2Service {
        Layer2Service::Acknowledged
    }

    fn unacked_bl_repetitions(self) -> Option<u8> {
        None
    }

    fn label(self) -> &'static str {
        match self {
            Self::CurrentChannel => "current-channel",
            Self::AssignedChannelRecovery => "assigned-channel recovery",
        }
    }
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

        // EN 300 392-2 clauses 14.5.1.1.1, 14.5.1.1.2 and table 14.74:
        // when on/off-hook signalling is selected, value 1 means "request
        // that other MS may transmit/send data". This selects the preferred
        // setup-phase talker; the U-plane floor is still granted only by the
        // transmission control procedure in clause 14.5.1.2.1.
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
                | (IndividualCallState::CallerConnectAckPending, IndividualEvent::Connect)
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

    fn private_connect_chan_alloc(ts: u8, usage: u8) -> CmceChanAllocReq {
        let mut timeslots = [false; 4];
        timeslots[ts as usize - 1] = true;
        CmceChanAllocReq {
            usage: Some(usage),
            alloc_type: ChanAllocType::Replace,
            carrier: None,
            timeslots,
            ul_dl_assigned: UlDlAssignment::Both,
        }
    }

    fn private_simplex_called_ms_transmits_first_for_call(&self, call_id: u16, call: &IndividualCall) -> Option<bool> {
        let cached = self.cached_setups.get(&call_id)?;
        Some(Self::private_simplex_called_ms_transmits_first(
            call.simplex_duplex,
            cached.pdu.hook_method_selection,
            call.request_to_transmit_send_data,
        ))
    }

    fn private_connect_grants(simplex_duplex: bool, called_ms_transmits_first: bool) -> (TransmissionGrant, TransmissionGrant) {
        if simplex_duplex {
            return (TransmissionGrant::Granted, TransmissionGrant::Granted);
        }

        // EN 300 392-2 14.5.1.1.1/14.5.1.1.2 require the connect PDUs to
        // indicate which simplex party has the setup-phase transmit role.
        // Clause 14.5.1.2.1 b) treats that setup response as the initial
        // transmit permission; later floor changes are driven by U-TX DEMAND.
        if called_ms_transmits_first {
            (TransmissionGrant::Granted, TransmissionGrant::GrantedToOtherUser)
        } else {
            (TransmissionGrant::GrantedToOtherUser, TransmissionGrant::Granted)
        }
    }

    fn send_private_called_d_connect_ack_delivery_guard(
        &self,
        queue: &mut MessageQueue,
        call_id: u16,
        call: &IndividualCall,
        called_ms_transmits_first: bool,
        delivery: PrivateCalledConnectAckDelivery,
    ) -> TxReporter {
        let (called_grant, _) = Self::private_connect_grants(call.simplex_duplex, called_ms_transmits_first);

        // EN 300 392-2 clauses 14.5.1.1.1/14.5.1.1.2 require
        // D-CONNECT ACKNOWLEDGE to tell the called MS the setup transmit
        // state. The matching UMAC FloorGranted is emitted only after caller
        // D-CONNECT delivery, so both MSs have completed setup before the
        // setup-time U-plane floor is opened.
        let d_connect_ack = DConnectAcknowledge {
            call_identifier: call_id,
            call_time_out: if call.simplex_duplex {
                CallTimeout::Infinite
            } else {
                self.config_call_timeout()
            },
            transmission_grant: called_grant,
            // EN 300 392-2 14.8.43/table 14.81 raw value 0 (`false`
            // here) means the MS is allowed to request transmit permission.
            transmission_request_permission: false,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };

        tracing::info!(
            "-> {:?} (called leg first, {} D-CONNECT ACK, {})",
            d_connect_ack,
            delivery.label(),
            "waiting for called BL-ACK before caller D-CONNECT"
        );
        let mut ack_sdu = BitBuffer::new_autoexpand(28);
        d_connect_ack
            .to_bitbuf(&mut ack_sdu)
            .expect("Failed to serialize DConnectAcknowledge");
        ack_sdu.seek(0);
        let pdu_prio = Self::cmce_downlink_pdu_prio(&ack_sdu);
        let reporter = if delivery.requires_l2_ack() {
            TxReporter::new()
        } else {
            TxReporter::new_unacked()
        };

        queue.push_back(SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: ack_sdu,
                handle: call.called_handle.unwrap_or(0),
                endpoint_id: call.called_endpoint_id.unwrap_or(0),
                link_id: call.called_link_id.unwrap_or(0),
                layer2service: delivery.layer2_service(),
                pdu_prio,
                layer2_qos: 0,
                stealing_permission: delivery.stealing_permission(),
                stealing_repeats_flag: false,
                unacked_bl_repetitions: delivery.unacked_bl_repetitions(),
                chan_alloc: Some(Self::private_connect_chan_alloc(call.called_ts, call.called_usage)),
                main_address: call.called_addr,
                tx_reporter: Some(reporter.clone()),
            }),
        });

        reporter
    }

    pub(in crate::cmce::subentities::cc_bs) fn drain_pending_individual_connect_acks(&mut self, queue: &mut MessageQueue) {
        let ready: Vec<(u16, PendingConnectAckAction)> = self
            .pending_individual_connect_acks
            .iter()
            .filter_map(|(&call_id, pending)| match pending.stage {
                PendingIndividualConnectAckStage::CalledConnectAck { requires_l2_ack, .. } => {
                    if requires_l2_ack && pending.reporter.is_acknowledged() || !requires_l2_ack && pending.reporter.is_transmitted() {
                        Some((call_id, PendingConnectAckAction::CalledDeliveryComplete))
                    } else if pending.reporter.is_in_final_state()
                        || pending.started_at.age(self.dltime) >= INDIVIDUAL_CONNECT_ACK_PENDING_TIMEOUT_TIMESLOTS
                    {
                        Some((call_id, PendingConnectAckAction::RetryOrFailCalledDelivery))
                    } else {
                        None
                    }
                }
                PendingIndividualConnectAckStage::CallerDConnect { .. } => {
                    if pending.reporter.is_acknowledged() {
                        Some((call_id, PendingConnectAckAction::CallerDeliveryComplete))
                    } else if pending.reporter.is_in_final_state()
                        || pending.started_at.age(self.dltime) >= INDIVIDUAL_CONNECT_ACK_PENDING_TIMEOUT_TIMESLOTS
                    {
                        Some((call_id, PendingConnectAckAction::RetryOrFailCallerDelivery))
                    } else {
                        None
                    }
                }
            })
            .collect();

        for (call_id, action) in ready {
            match action {
                PendingConnectAckAction::CalledDeliveryComplete => {
                    self.pending_individual_connect_acks.remove(&call_id);
                    tracing::info!(
                        "CMCE: called D-CONNECT ACK acknowledged for call_id={}; sending caller D-CONNECT",
                        call_id
                    );
                    self.complete_private_connect_after_called_delivery(queue, call_id);
                }
                PendingConnectAckAction::CallerDeliveryComplete => {
                    self.pending_individual_connect_acks.remove(&call_id);
                    tracing::info!(
                        "CMCE: caller D-CONNECT L2-acknowledged for call_id={}; activating private call with setup-time simplex floor",
                        call_id
                    );
                    self.complete_private_connect_after_caller_connect_delivery(queue, call_id);
                }
                PendingConnectAckAction::RetryOrFailCalledDelivery => {
                    let attempts = self
                        .pending_individual_connect_acks
                        .get(&call_id)
                        .and_then(|pending| match pending.stage {
                            PendingIndividualConnectAckStage::CalledConnectAck { attempts, .. } => Some(attempts),
                            PendingIndividualConnectAckStage::CallerDConnect { .. } => None,
                        })
                        .unwrap_or(INDIVIDUAL_CONNECT_ACK_MAX_ATTEMPTS);
                    if attempts >= INDIVIDUAL_CONNECT_ACK_MAX_ATTEMPTS {
                        self.pending_individual_connect_acks.remove(&call_id);
                        tracing::warn!(
                            "CMCE: called D-CONNECT ACK was not locally transmitted after {} attempts for call_id={}; releasing setup",
                            attempts,
                            call_id
                        );
                        self.release_individual_call(queue, call_id, DisconnectCause::AcknowledgedServiceNotComplete);
                        continue;
                    }

                    let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
                        self.pending_individual_connect_acks.remove(&call_id);
                        continue;
                    };
                    let Some(called_ms_transmits_first) = self.private_simplex_called_ms_transmits_first_for_call(call_id, &call_snapshot)
                    else {
                        self.pending_individual_connect_acks.remove(&call_id);
                        continue;
                    };

                    let prior_state = self
                        .pending_individual_connect_acks
                        .get(&call_id)
                        .map(|pending| pending.reporter.get_state())
                        .unwrap_or(TxState::Lost);
                    let delivery = match prior_state {
                        TxState::Transmitted | TxState::Lost | TxState::Acknowledged => {
                            PrivateCalledConnectAckDelivery::retry_for_attempt(attempts + 1)
                        }
                        TxState::Pending | TxState::Discarded => {
                            PrivateCalledConnectAckDelivery::retry_after_local_delivery_failure(attempts + 1)
                        }
                    };

                    tracing::warn!(
                        "CMCE: retrying called D-CONNECT ACK for call_id={} attempt {}/{} via {} recovery",
                        call_id,
                        attempts + 1,
                        INDIVIDUAL_CONNECT_ACK_MAX_ATTEMPTS,
                        delivery.label()
                    );
                    let reporter = self.send_private_called_d_connect_ack_delivery_guard(
                        queue,
                        call_id,
                        &call_snapshot,
                        called_ms_transmits_first,
                        delivery,
                    );
                    if let Some(pending) = self.pending_individual_connect_acks.get_mut(&call_id) {
                        pending.reporter = reporter;
                        pending.stage = PendingIndividualConnectAckStage::CalledConnectAck {
                            attempts: attempts + 1,
                            requires_l2_ack: delivery.requires_l2_ack(),
                        };
                        pending.started_at = self.dltime;
                    }
                }
                PendingConnectAckAction::RetryOrFailCallerDelivery => {
                    let attempts = self
                        .pending_individual_connect_acks
                        .get(&call_id)
                        .and_then(|pending| match pending.stage {
                            PendingIndividualConnectAckStage::CallerDConnect { attempts } => Some(attempts),
                            PendingIndividualConnectAckStage::CalledConnectAck { .. } => None,
                        })
                        .unwrap_or(INDIVIDUAL_CALLER_CONNECT_MAX_ATTEMPTS);
                    if attempts >= INDIVIDUAL_CALLER_CONNECT_MAX_ATTEMPTS {
                        self.pending_individual_connect_acks.remove(&call_id);
                        tracing::warn!(
                            "CMCE: caller D-CONNECT was not locally transmitted after {} attempts for call_id={}; releasing setup",
                            attempts,
                            call_id
                        );
                        self.release_individual_call(queue, call_id, DisconnectCause::AcknowledgedServiceNotComplete);
                        continue;
                    }

                    let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
                        self.pending_individual_connect_acks.remove(&call_id);
                        continue;
                    };
                    let Some(cached) = self.cached_setups.get(&call_id) else {
                        self.pending_individual_connect_acks.remove(&call_id);
                        self.release_individual_call(queue, call_id, DisconnectCause::AcknowledgedServiceNotComplete);
                        continue;
                    };
                    let Some(called_ms_transmits_first) = self.private_simplex_called_ms_transmits_first_for_call(call_id, &call_snapshot)
                    else {
                        self.pending_individual_connect_acks.remove(&call_id);
                        continue;
                    };

                    tracing::warn!(
                        "CMCE: retrying caller D-CONNECT for call_id={} attempt {}/{} before initial floor",
                        call_id,
                        attempts + 1,
                        INDIVIDUAL_CALLER_CONNECT_MAX_ATTEMPTS
                    );
                    let reporter = self.send_private_caller_d_connect_delivery_guard(
                        queue,
                        call_id,
                        &call_snapshot,
                        cached,
                        called_ms_transmits_first,
                    );
                    if let Some(pending) = self.pending_individual_connect_acks.get_mut(&call_id) {
                        pending.reporter = reporter;
                        pending.stage = PendingIndividualConnectAckStage::CallerDConnect { attempts: attempts + 1 };
                        pending.started_at = self.dltime;
                    }
                }
            }
        }
    }

    pub(super) fn complete_private_connect_after_called_delivery(&mut self, queue: &mut MessageQueue, call_id: u16) {
        let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
            tracing::debug!("CMCE: pending private connect call_id={} disappeared before D-CONNECT", call_id);
            return;
        };
        let Some(cached) = self.cached_setups.get(&call_id) else {
            tracing::error!("CMCE: pending private connect call_id={} missing cached D-SETUP", call_id);
            self.release_individual_call(queue, call_id, DisconnectCause::AcknowledgedServiceNotComplete);
            return;
        };
        let Some(called_ms_transmits_first) = self.private_simplex_called_ms_transmits_first_for_call(call_id, &call_snapshot) else {
            return;
        };

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            call.mark_caller_connect_ack_pending();
        }

        let reporter = self.send_private_caller_d_connect_delivery_guard(queue, call_id, &call_snapshot, cached, called_ms_transmits_first);
        tracing::info!(
            "CMCE: caller D-CONNECT queued for call_id={}; waiting for caller L2 ACK before private call activation",
            call_id
        );
        self.pending_individual_connect_acks.insert(
            call_id,
            PendingIndividualConnectAck {
                reporter,
                stage: PendingIndividualConnectAckStage::CallerDConnect { attempts: 1 },
                started_at: self.dltime,
            },
        );
    }

    fn send_private_caller_d_connect_delivery_guard(
        &self,
        queue: &mut MessageQueue,
        call_id: u16,
        call_snapshot: &IndividualCall,
        cached: &CachedSetup,
        called_ms_transmits_first: bool,
    ) -> TxReporter {
        let (_, calling_grant) = Self::private_connect_grants(call_snapshot.simplex_duplex, called_ms_transmits_first);
        let d_connect = DConnect {
            call_identifier: call_id,
            call_time_out: if call_snapshot.simplex_duplex {
                CallTimeout::Infinite
            } else {
                self.config_call_timeout()
            },
            hook_method_selection: cached.pdu.hook_method_selection,
            simplex_duplex_selection: call_snapshot.simplex_duplex,
            transmission_grant: calling_grant,
            // EN 300 392-2 14.8.43/table 14.81 raw value 0 (`false`
            // here) means the MS is allowed to request transmit permission.
            transmission_request_permission: false,
            call_ownership: true,
            call_priority: None,
            basic_service_information: None,
            temporary_address: None,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };

        tracing::info!(
            "-> {:?} (caller leg after called D-CONNECT ACK BL-ACK, waiting for D-CONNECT local transmission before call activation)",
            d_connect
        );
        let mut connect_sdu = BitBuffer::new_autoexpand(30);
        d_connect.to_bitbuf(&mut connect_sdu).expect("Failed to serialize DConnect");
        connect_sdu.seek(0);
        let pdu_prio = Self::cmce_downlink_pdu_prio(&connect_sdu);
        let reporter = TxReporter::new();

        queue.push_back(SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: connect_sdu,
                handle: call_snapshot.calling_handle,
                endpoint_id: call_snapshot.calling_endpoint_id,
                link_id: call_snapshot.calling_link_id,
                layer2service: Layer2Service::Acknowledged,
                pdu_prio,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                unacked_bl_repetitions: None,
                chan_alloc: Some(Self::private_connect_chan_alloc(
                    call_snapshot.calling_ts,
                    call_snapshot.calling_usage,
                )),
                main_address: call_snapshot.calling_addr,
                tx_reporter: Some(reporter.clone()),
            }),
        });

        reporter
    }

    fn complete_private_connect_after_caller_connect_delivery(&mut self, queue: &mut MessageQueue, call_id: u16) {
        let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
            tracing::debug!(
                "CMCE: pending private connect call_id={} disappeared before caller connect delivery",
                call_id
            );
            return;
        };

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
            return;
        }

        if !call_snapshot.simplex_duplex {
            let Some(called_ms_transmits_first) = self.private_simplex_called_ms_transmits_first_for_call(call_id, &call_snapshot) else {
                return;
            };
            let (source_issi, dest_issi, floor_ts) = if called_ms_transmits_first {
                (
                    call_snapshot.called_addr.ssi,
                    call_snapshot.calling_addr.ssi,
                    call_snapshot.called_ts,
                )
            } else {
                (
                    call_snapshot.calling_addr.ssi,
                    call_snapshot.called_addr.ssi,
                    call_snapshot.calling_ts,
                )
            };
            if let Some(call) = self.individual_calls.get_mut(&call_id) {
                call.set_floor_holder(source_issi);
            }
            tracing::info!(
                "CMCE: private-simplex call_id={} active; setup grant seeds U-plane floor for ISSI {} per EN 300 392-2 14.5.1.1.1/14.5.1.1.2",
                call_id,
                source_issi
            );
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
            if self.send_pending_individual_release_direct_ack(
                queue,
                call_id,
                received_tetra_address,
                handle,
                link_id,
                endpoint_id,
                "late U-ALERT",
            ) {
                return;
            }
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
            if self.send_pending_individual_release_direct_ack(
                queue,
                call_id,
                received_tetra_address,
                handle,
                link_id,
                endpoint_id,
                "late U-CONNECT",
            ) {
                return;
            }
            tracing::warn!("U-CONNECT for unknown call_id={}", call_id);
            return;
        };

        if call_snapshot.is_active() {
            tracing::debug!("U-CONNECT for active call_id={}, ignoring", call_id);
            return;
        }
        if call_snapshot.is_connect_ack_pending() {
            tracing::debug!(
                "U-CONNECT duplicate ignored for private call_id={} while connect ACK state is {:?}",
                call_id,
                call_snapshot.state
            );
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
        if self.pending_individual_connect_acks.contains_key(&call_id) {
            tracing::debug!(
                "CMCE: duplicate U-CONNECT for call_id={} ignored while waiting for private direct-setup delivery guard",
                call_id
            );
            return;
        }

        let mut calling_timeslots = [false; 4];
        calling_timeslots[calling_ts as usize - 1] = true;
        let mut called_timeslots = [false; 4];
        called_timeslots[called_ts as usize - 1] = true;

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

        let (primary_active_addr, active_secondary_addrs) = if called_ts == calling_ts {
            // On the shared same-timeslot private-simplex bearer, keep the
            // called ISSI as bearer primary until FloorGranted names the
            // actual speaker. Direct setup first completes called-leg
            // signalling, then caller D-CONNECT, then floor authorization.
            (called_addr, vec![calling_addr])
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

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            call.mark_called_connect_ack_pending();
        }
        let current_call = self.individual_calls.get(&call_id).cloned().unwrap_or(call_snapshot);
        let reporter = self.send_private_called_d_connect_ack_delivery_guard(
            queue,
            call_id,
            &current_call,
            called_ms_transmits_first,
            PrivateCalledConnectAckDelivery::CurrentChannel,
        );
        self.pending_individual_connect_acks.insert(
            call_id,
            PendingIndividualConnectAck {
                reporter,
                stage: PendingIndividualConnectAckStage::CalledConnectAck {
                    attempts: 1,
                    requires_l2_ack: PrivateCalledConnectAckDelivery::CurrentChannel.requires_l2_ack(),
                },
                started_at: self.dltime,
            },
        );
    }
}
