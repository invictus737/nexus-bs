// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original pure TETRA SNDCP bearer-control coordinator primitive.

use std::collections::BTreeMap;

use super::pdp::{SndcpActivatePdpContextAccept, SndcpActivatePdpContextDemand, SndcpActivatePdpContextReject, SndcpDeactivation};
use super::pdp_service::{SndcpPdpActivationResult, SndcpPdpDeactivationResult, SndcpPdpPolicy, SndcpPdpService};
use super::state::{SwmiSndcpAction, SwmiSndcpState, SwmiSndcpStateError, SwmiSndcpStateMachine, SwmiSndcpTransition};
use super::transfer::{
    SndcpDataTransmitRequest, SndcpDataTransmitResponse, SndcpDataTransmitResponseResult, SndcpEndOfData, SndcpReconnect,
    SndcpTransferRejectCause, encode_data_transmit_request, encode_data_transmit_response, encode_end_of_data,
};
use tetra_core::BitBuffer;

#[derive(Debug, Clone)]
pub struct SndcpBearerManager {
    pdp: SndcpPdpService,
    states: BTreeMap<u32, SwmiSndcpStateMachine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SndcpBearerActivationOutcome {
    Accepted {
        accept: SndcpActivatePdpContextAccept,
        transition: SwmiSndcpTransition,
    },
    Rejected(SndcpActivatePdpContextReject),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SndcpBearerDeactivationOutcome {
    pub deactivation: SndcpPdpDeactivationResult,
    pub transitions: Vec<SwmiSndcpTransition>,
}

#[derive(Debug, Clone)]
pub struct SndcpBearerControlOutcome {
    pub transition: Option<SwmiSndcpTransition>,
    pub control_pdu: Option<BitBuffer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SndcpBearerError {
    MissingPdpContext { issi: u32, nsapi: u8 },
    PacketDataTransferNotReady { issi: u32, state: SwmiSndcpState },
    State(SwmiSndcpStateError),
    EncodeTransferControl,
}

impl SndcpBearerManager {
    pub fn new(policy: SndcpPdpPolicy) -> Self {
        Self {
            pdp: SndcpPdpService::new(policy),
            states: BTreeMap::new(),
        }
    }

    pub fn pdp(&self) -> &SndcpPdpService {
        &self.pdp
    }

    pub fn state_for_issi(&self, issi: u32) -> SwmiSndcpState {
        self.states
            .get(&issi)
            .map(SwmiSndcpStateMachine::state)
            .unwrap_or(SwmiSndcpState::Idle)
    }

    pub fn handle_activate_demand(
        &mut self,
        issi: u32,
        demand: SndcpActivatePdpContextDemand,
    ) -> Result<SndcpBearerActivationOutcome, SndcpBearerError> {
        match self.pdp.handle_activate_demand(issi, demand) {
            SndcpPdpActivationResult::Accepted { accept, .. } => {
                let transition = self
                    .state_for_issi_mut(issi)
                    .activate_pdp_context()
                    .map_err(SndcpBearerError::State)?;
                Ok(SndcpBearerActivationOutcome::Accepted { accept, transition })
            }
            SndcpPdpActivationResult::Rejected(reject) => Ok(SndcpBearerActivationOutcome::Rejected(reject)),
        }
    }

    pub fn handle_deactivate_demand(&mut self, issi: u32, deactivation: SndcpDeactivation) -> SndcpBearerDeactivationOutcome {
        let deactivation = self.pdp.handle_deactivate_demand(issi, deactivation);
        let mut transitions = Vec::new();
        for _ in 0..deactivation.removed_contexts {
            let Some(state) = self.states.get_mut(&issi) else {
                break;
            };
            let Ok(transition) = state.deactivate_pdp_context() else {
                break;
            };
            transitions.push(transition);
        }
        self.remove_idle_state(issi);
        SndcpBearerDeactivationOutcome { deactivation, transitions }
    }

    pub fn handle_ms_data_transmit_request(
        &mut self,
        issi: u32,
        request: SndcpDataTransmitRequest,
    ) -> Result<SndcpBearerControlOutcome, SndcpBearerError> {
        if self.context_missing(issi, request.nsapi) {
            return Ok(SndcpBearerControlOutcome {
                transition: None,
                control_pdu: Some(encode_reject_response(request.nsapi, SndcpTransferRejectCause::UnknownNsapi)?),
            });
        }

        let transition = match self.state_for_issi_mut(issi).data_transmit_request_received() {
            Ok(transition) => transition,
            Err(error) => {
                return Ok(SndcpBearerControlOutcome {
                    transition: None,
                    control_pdu: Some(encode_reject_response(request.nsapi, state_error_to_reject_cause(error))?),
                });
            }
        };

        Ok(SndcpBearerControlOutcome {
            transition: Some(transition),
            control_pdu: Some(encode_data_transmit_response(&SndcpDataTransmitResponse {
                nsapi: request.nsapi,
                result: SndcpDataTransmitResponseResult::Accepted,
            })?),
        })
    }

    pub fn handle_swmi_service_user_data_request(&mut self, issi: u32, nsapi: u8) -> Result<SndcpBearerControlOutcome, SndcpBearerError> {
        if self.context_missing(issi, nsapi) {
            return Err(SndcpBearerError::MissingPdpContext { issi, nsapi });
        }

        let transition = self
            .state_for_issi_mut(issi)
            .service_user_data_request()
            .map_err(SndcpBearerError::State)?;
        Ok(SndcpBearerControlOutcome {
            transition: Some(transition),
            control_pdu: Some(encode_data_transmit_request(&SndcpDataTransmitRequest {
                nsapi,
                logical_link_status: false,
            })?),
        })
    }

    pub fn handle_packet_data_transferred(&mut self, issi: u32) -> Result<SwmiSndcpTransition, SndcpBearerError> {
        self.state_for_issi_mut(issi)
            .packet_data_transferred()
            .map_err(SndcpBearerError::State)
    }

    pub fn prepare_swmi_unitdata_transfer(&mut self, issi: u32, nsapi: u8) -> Result<SwmiSndcpTransition, SndcpBearerError> {
        if self.context_missing(issi, nsapi) {
            return Err(SndcpBearerError::MissingPdpContext { issi, nsapi });
        }

        let state = self.state_for_issi(issi);
        if state != SwmiSndcpState::Ready {
            return Err(SndcpBearerError::PacketDataTransferNotReady { issi, state });
        }

        self.handle_packet_data_transferred(issi)
    }

    pub fn handle_ready_timer_expired(&mut self, issi: u32) -> Result<SndcpBearerControlOutcome, SndcpBearerError> {
        let transition = self
            .state_for_issi_mut(issi)
            .ready_timer_expired()
            .map_err(SndcpBearerError::State)?;
        Ok(SndcpBearerControlOutcome {
            control_pdu: control_pdu_for_transition(&transition)?,
            transition: Some(transition),
        })
    }

    pub fn handle_end_of_data_received(
        &mut self,
        issi: u32,
        end_of_data: SndcpEndOfData,
        swmi_transmitting_tl_sdu: bool,
    ) -> Result<SndcpBearerControlOutcome, SndcpBearerError> {
        let transition = self
            .state_for_issi_mut(issi)
            .end_of_data_received(end_of_data.immediate_service_change, swmi_transmitting_tl_sdu)
            .map_err(SndcpBearerError::State)?;
        Ok(SndcpBearerControlOutcome {
            control_pdu: control_pdu_for_transition(&transition)?,
            transition: Some(transition),
        })
    }

    pub fn handle_reconnect_received(
        &mut self,
        issi: u32,
        reconnect: SndcpReconnect,
    ) -> Result<SndcpBearerControlOutcome, SndcpBearerError> {
        if let Some(nsapi) = reconnect.nsapi {
            if self.context_missing(issi, nsapi) {
                return Ok(SndcpBearerControlOutcome {
                    transition: None,
                    control_pdu: Some(encode_reject_response(nsapi, SndcpTransferRejectCause::UnknownNsapi)?),
                });
            }
        }

        let transition = self
            .state_for_issi_mut(issi)
            .reconnect_received()
            .map_err(SndcpBearerError::State)?;
        Ok(SndcpBearerControlOutcome {
            transition: Some(transition),
            control_pdu: None,
        })
    }

    pub fn deregister_issi(&mut self, issi: u32) -> Result<SwmiSndcpTransition, SndcpBearerError> {
        self.pdp.handle_deactivate_demand(issi, SndcpDeactivation::AllNsapis);
        let transition = self.state_for_issi_mut(issi).deregister_ms().map_err(SndcpBearerError::State)?;
        self.remove_idle_state(issi);
        Ok(transition)
    }

    fn state_for_issi_mut(&mut self, issi: u32) -> &mut SwmiSndcpStateMachine {
        self.states.entry(issi).or_default()
    }

    fn remove_idle_state(&mut self, issi: u32) {
        if self.state_for_issi(issi) == SwmiSndcpState::Idle {
            self.states.remove(&issi);
        }
    }

    fn context_missing(&self, issi: u32, nsapi: u8) -> bool {
        self.pdp.contexts().get_issi_nsapi(issi, nsapi).ok().flatten().is_none()
    }
}

impl Default for SndcpBearerManager {
    fn default() -> Self {
        Self::new(SndcpPdpPolicy::default())
    }
}

impl From<super::transfer::SndcpTransferError> for SndcpBearerError {
    fn from(_: super::transfer::SndcpTransferError) -> Self {
        SndcpBearerError::EncodeTransferControl
    }
}

fn encode_reject_response(nsapi: u8, cause: SndcpTransferRejectCause) -> Result<BitBuffer, SndcpBearerError> {
    Ok(encode_data_transmit_response(&SndcpDataTransmitResponse {
        nsapi,
        result: SndcpDataTransmitResponseResult::Rejected(cause),
    })?)
}

fn control_pdu_for_transition(transition: &SwmiSndcpTransition) -> Result<Option<BitBuffer>, SndcpBearerError> {
    if transition.actions.contains(&SwmiSndcpAction::TransmitEndOfData) {
        Ok(Some(encode_end_of_data(&SndcpEndOfData {
            immediate_service_change: false,
        })?))
    } else {
        Ok(None)
    }
}

fn state_error_to_reject_cause(error: SwmiSndcpStateError) -> SndcpTransferRejectCause {
    match error {
        SwmiSndcpStateError::NoActivePdpContext => SndcpTransferRejectCause::UnknownNsapi,
        SwmiSndcpStateError::InvalidEvent { .. } | SwmiSndcpStateError::SwmiStillTransmittingTlSdu => SndcpTransferRejectCause::Undefined,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sndcp::pdp::{SndcpActivateAddressDemand, SndcpActivationRejectCause};
    use crate::sndcp::transfer::{decode_data_transmit_request, decode_data_transmit_response, decode_end_of_data};
    use tetra_saps::sn::SnPacketDataMsType;

    const ISSI: u32 = 2_260_618;
    const NSAPI: u8 = 2;

    fn demand(nsapi: u8) -> SndcpActivatePdpContextDemand {
        SndcpActivatePdpContextDemand {
            sndcp_version: 1,
            nsapi,
            address: SndcpActivateAddressDemand::Ipv4Dynamic,
            packet_data_ms_type: SnPacketDataMsType::TypeAParallel,
            pcomp_negotiation: 0,
        }
    }

    fn manager() -> SndcpBearerManager {
        SndcpBearerManager::new(SndcpPdpPolicy::experimental_wap_ipv4())
    }

    fn activated_manager() -> SndcpBearerManager {
        let mut manager = manager();
        assert!(matches!(
            manager.handle_activate_demand(ISSI, demand(NSAPI)),
            Ok(SndcpBearerActivationOutcome::Accepted { .. })
        ));
        manager
    }

    #[test]
    fn default_policy_rejects_activation_without_creating_state() {
        let mut manager = SndcpBearerManager::default();

        assert_eq!(
            manager.handle_activate_demand(ISSI, demand(NSAPI)),
            Ok(SndcpBearerActivationOutcome::Rejected(SndcpActivatePdpContextReject {
                nsapi: NSAPI,
                cause: SndcpActivationRejectCause::SndcpServiceTemporarilyNotAvailable
            }))
        );
        assert_eq!(manager.state_for_issi(ISSI), SwmiSndcpState::Idle);
        assert!(manager.pdp().contexts().is_empty());
    }

    #[test]
    fn accepted_pdp_activation_enters_standby_and_starts_standby_timer() {
        let mut manager = manager();

        let outcome = manager
            .handle_activate_demand(ISSI, demand(NSAPI))
            .expect("activation should not fail state handling");

        let SndcpBearerActivationOutcome::Accepted { accept, transition } = outcome else {
            panic!("activation should be accepted");
        };
        assert_eq!(accept.nsapi, NSAPI);
        assert_eq!(transition.previous_state, SwmiSndcpState::Idle);
        assert_eq!(transition.new_state, SwmiSndcpState::Standby);
        assert_eq!(transition.actions, vec![SwmiSndcpAction::StartStandbyTimer]);
        assert_eq!(manager.state_for_issi(ISSI), SwmiSndcpState::Standby);
        assert!(manager.pdp().contexts().get_issi_nsapi(ISSI, NSAPI).unwrap().is_some());
    }

    #[test]
    fn ms_data_transmit_request_for_active_context_returns_accept_and_enters_ready() {
        let mut manager = activated_manager();

        let outcome = manager
            .handle_ms_data_transmit_request(
                ISSI,
                SndcpDataTransmitRequest {
                    nsapi: NSAPI,
                    logical_link_status: false,
                },
            )
            .expect("MS data transmit request should be handled");

        let response = decode_data_transmit_response(outcome.control_pdu.as_ref().expect("response PDU should be present"))
            .expect("response should decode");
        assert_eq!(
            response,
            SndcpDataTransmitResponse {
                nsapi: NSAPI,
                result: SndcpDataTransmitResponseResult::Accepted
            }
        );
        let transition = outcome.transition.expect("accepted request should change state");
        assert_eq!(transition.previous_state, SwmiSndcpState::Standby);
        assert_eq!(transition.new_state, SwmiSndcpState::Ready);
        assert_eq!(
            transition.actions,
            vec![
                SwmiSndcpAction::TransmitDataTransmitResponseAccept,
                SwmiSndcpAction::StopStandbyTimer,
                SwmiSndcpAction::StartReadyTimer,
                SwmiSndcpAction::StartContextReadyTimers
            ]
        );
        assert_eq!(manager.state_for_issi(ISSI), SwmiSndcpState::Ready);
    }

    #[test]
    fn ms_data_transmit_request_for_unknown_nsapi_returns_reject_without_state_change() {
        let mut manager = activated_manager();

        let outcome = manager
            .handle_ms_data_transmit_request(
                ISSI,
                SndcpDataTransmitRequest {
                    nsapi: 3,
                    logical_link_status: false,
                },
            )
            .expect("unknown NSAPI should produce reject response");

        assert!(outcome.transition.is_none());
        let response = decode_data_transmit_response(outcome.control_pdu.as_ref().expect("reject PDU should be present"))
            .expect("reject should decode");
        assert_eq!(
            response.result,
            SndcpDataTransmitResponseResult::Rejected(SndcpTransferRejectCause::UnknownNsapi)
        );
        assert_eq!(manager.state_for_issi(ISSI), SwmiSndcpState::Standby);
    }

    #[test]
    fn swmi_service_user_data_request_sends_downlink_data_transmit_request_and_enters_ready() {
        let mut manager = activated_manager();

        let outcome = manager
            .handle_swmi_service_user_data_request(ISSI, NSAPI)
            .expect("SwMI data request should be handled");

        let request = decode_data_transmit_request(outcome.control_pdu.as_ref().expect("request PDU should be present"))
            .expect("request should decode");
        assert_eq!(
            request,
            SndcpDataTransmitRequest {
                nsapi: NSAPI,
                logical_link_status: false
            }
        );
        let transition = outcome.transition.expect("request should change state");
        assert_eq!(transition.previous_state, SwmiSndcpState::Standby);
        assert_eq!(transition.new_state, SwmiSndcpState::Ready);
        assert_eq!(
            transition.actions,
            vec![
                SwmiSndcpAction::TransmitDataTransmitRequest,
                SwmiSndcpAction::StopStandbyTimer,
                SwmiSndcpAction::StartReadyTimer,
                SwmiSndcpAction::StartContextReadyTimers
            ]
        );
    }

    #[test]
    fn packet_transfer_and_ready_timer_expiry_restart_then_close_ready_period() {
        let mut manager = activated_manager();
        manager
            .handle_swmi_service_user_data_request(ISSI, NSAPI)
            .expect("SwMI request should enter READY");

        let transfer = manager
            .handle_packet_data_transferred(ISSI)
            .expect("packet transfer should restart READY timers");
        assert_eq!(
            transfer.actions,
            vec![SwmiSndcpAction::RestartReadyTimer, SwmiSndcpAction::RestartContextReadyTimers]
        );

        let expiry = manager
            .handle_ready_timer_expired(ISSI)
            .expect("READY timer expiry should produce end-of-data");
        let end =
            decode_end_of_data(expiry.control_pdu.as_ref().expect("end-of-data PDU should be present")).expect("end-of-data should decode");
        assert_eq!(
            end,
            SndcpEndOfData {
                immediate_service_change: false
            }
        );
        assert_eq!(manager.state_for_issi(ISSI), SwmiSndcpState::Standby);
    }

    #[test]
    fn unitdata_transfer_requires_active_context_and_ready_state() {
        let mut manager = activated_manager();

        assert_eq!(
            manager.prepare_swmi_unitdata_transfer(ISSI, NSAPI),
            Err(SndcpBearerError::PacketDataTransferNotReady {
                issi: ISSI,
                state: SwmiSndcpState::Standby
            })
        );
        assert_eq!(
            manager.prepare_swmi_unitdata_transfer(ISSI, 3),
            Err(SndcpBearerError::MissingPdpContext { issi: ISSI, nsapi: 3 })
        );

        manager
            .handle_ms_data_transmit_request(
                ISSI,
                SndcpDataTransmitRequest {
                    nsapi: NSAPI,
                    logical_link_status: false,
                },
            )
            .expect("MS request should enter READY");
        let transfer = manager
            .prepare_swmi_unitdata_transfer(ISSI, NSAPI)
            .expect("READY state should allow SN-UNITDATA transfer");
        assert_eq!(
            transfer.actions,
            vec![SwmiSndcpAction::RestartReadyTimer, SwmiSndcpAction::RestartContextReadyTimers]
        );
    }

    #[test]
    fn end_of_data_with_immediate_service_change_enters_standby_without_echo() {
        let mut manager = activated_manager();
        manager
            .handle_swmi_service_user_data_request(ISSI, NSAPI)
            .expect("SwMI request should enter READY");

        let outcome = manager
            .handle_end_of_data_received(
                ISSI,
                SndcpEndOfData {
                    immediate_service_change: true,
                },
                false,
            )
            .expect("immediate service change should enter STANDBY");

        assert!(outcome.control_pdu.is_none());
        assert_eq!(manager.state_for_issi(ISSI), SwmiSndcpState::Standby);
    }

    #[test]
    fn reconnect_with_unknown_nsapi_returns_reject_without_state_change() {
        let mut manager = activated_manager();
        manager
            .handle_swmi_service_user_data_request(ISSI, NSAPI)
            .expect("SwMI request should enter READY");

        let outcome = manager
            .handle_reconnect_received(ISSI, SndcpReconnect { nsapi: Some(3) })
            .expect("unknown NSAPI reconnect should reject");
        assert!(outcome.transition.is_none());
        assert!(matches!(
            decode_data_transmit_response(outcome.control_pdu.as_ref().expect("reject PDU should be present")),
            Ok(SndcpDataTransmitResponse {
                result: SndcpDataTransmitResponseResult::Rejected(SndcpTransferRejectCause::UnknownNsapi),
                ..
            })
        ));
        assert_eq!(manager.state_for_issi(ISSI), SwmiSndcpState::Ready);
    }

    #[test]
    fn deactivation_and_deregistration_remove_contexts_and_return_idle() {
        let mut manager = manager();
        manager
            .handle_activate_demand(ISSI, demand(NSAPI))
            .expect("activation should succeed");
        manager
            .handle_activate_demand(ISSI, demand(3))
            .expect("second activation should succeed");

        let first = manager.handle_deactivate_demand(ISSI, SndcpDeactivation::Nsapi(NSAPI));
        assert_eq!(first.deactivation.removed_contexts, 1);
        assert_eq!(first.transitions.len(), 1);
        assert_eq!(manager.state_for_issi(ISSI), SwmiSndcpState::Standby);

        let deregister = manager.deregister_issi(ISSI).expect("deregister should clear state");
        assert_eq!(deregister.new_state, SwmiSndcpState::Idle);
        assert!(deregister.actions.contains(&SwmiSndcpAction::DeleteAllPdpContexts));
        assert_eq!(manager.state_for_issi(ISSI), SwmiSndcpState::Idle);
        assert!(manager.pdp().contexts().get_issi_nsapi(ISSI, 3).unwrap().is_none());
    }
}
