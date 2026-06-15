// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original pure TETRA SNDCP SwMI state/timer primitive.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwmiSndcpState {
    Idle,
    Standby,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SwmiSndcpTimers {
    pub standby: bool,
    pub ready: bool,
    pub context_ready: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwmiSndcpEvent {
    PdpContextActivated,
    PdpContextDeactivated,
    StandbyTimerExpired,
    MsDeregistered,
    ServiceUserDataRequest,
    DataTransmitRequestReceived,
    PacketDataTransferred,
    ReadyTimerExpired,
    EndOfDataReceived,
    ReconnectReceived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwmiSndcpAction {
    StartStandbyTimer,
    StopStandbyTimer,
    StartReadyTimer,
    RestartReadyTimer,
    StopReadyTimer,
    StartContextReadyTimers,
    RestartContextReadyTimers,
    StopContextReadyTimers,
    DeleteAllPdpContexts,
    IndicateAllNsapisDeallocated,
    TransmitDataTransmitRequest,
    TransmitDataTransmitResponseAccept,
    TransmitEndOfData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwmiSndcpTransition {
    pub previous_state: SwmiSndcpState,
    pub new_state: SwmiSndcpState,
    pub actions: Vec<SwmiSndcpAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwmiSndcpStateError {
    InvalidEvent { state: SwmiSndcpState, event: SwmiSndcpEvent },
    NoActivePdpContext,
    SwmiStillTransmittingTlSdu,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwmiSndcpStateMachine {
    state: SwmiSndcpState,
    active_pdp_contexts: usize,
    timers: SwmiSndcpTimers,
}

impl Default for SwmiSndcpStateMachine {
    fn default() -> Self {
        Self {
            state: SwmiSndcpState::Idle,
            active_pdp_contexts: 0,
            timers: SwmiSndcpTimers::default(),
        }
    }
}

impl SwmiSndcpStateMachine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> SwmiSndcpState {
        self.state
    }

    pub fn active_pdp_contexts(&self) -> usize {
        self.active_pdp_contexts
    }

    pub fn timers(&self) -> SwmiSndcpTimers {
        self.timers
    }

    pub fn activate_pdp_context(&mut self) -> Result<SwmiSndcpTransition, SwmiSndcpStateError> {
        let previous_state = self.state;
        if self.active_pdp_contexts == 0 {
            if self.state != SwmiSndcpState::Idle {
                return Err(invalid(self.state, SwmiSndcpEvent::PdpContextActivated));
            }
            self.active_pdp_contexts = 1;
            self.enter_standby_from_idle();
            Ok(transition(previous_state, self.state, vec![SwmiSndcpAction::StartStandbyTimer]))
        } else {
            self.active_pdp_contexts += 1;
            Ok(transition(previous_state, self.state, Vec::new()))
        }
    }

    pub fn deactivate_pdp_context(&mut self) -> Result<SwmiSndcpTransition, SwmiSndcpStateError> {
        if self.active_pdp_contexts == 0 {
            return Err(SwmiSndcpStateError::NoActivePdpContext);
        }

        let previous_state = self.state;
        if self.active_pdp_contexts > 1 {
            self.active_pdp_contexts -= 1;
            return Ok(transition(previous_state, self.state, Vec::new()));
        }

        if self.state != SwmiSndcpState::Standby {
            return Err(invalid(self.state, SwmiSndcpEvent::PdpContextDeactivated));
        }

        self.active_pdp_contexts = 0;
        self.enter_idle();
        Ok(transition(previous_state, self.state, vec![SwmiSndcpAction::StopStandbyTimer]))
    }

    pub fn standby_timer_expired(&mut self) -> Result<SwmiSndcpTransition, SwmiSndcpStateError> {
        if self.state != SwmiSndcpState::Standby {
            return Err(invalid(self.state, SwmiSndcpEvent::StandbyTimerExpired));
        }

        let previous_state = self.state;
        self.active_pdp_contexts = 0;
        self.enter_idle();
        Ok(transition(
            previous_state,
            self.state,
            vec![
                SwmiSndcpAction::StopStandbyTimer,
                SwmiSndcpAction::DeleteAllPdpContexts,
                SwmiSndcpAction::IndicateAllNsapisDeallocated,
            ],
        ))
    }

    pub fn deregister_ms(&mut self) -> Result<SwmiSndcpTransition, SwmiSndcpStateError> {
        let previous_state = self.state;
        let actions = match self.state {
            SwmiSndcpState::Idle => Vec::new(),
            SwmiSndcpState::Standby => vec![SwmiSndcpAction::StopStandbyTimer, SwmiSndcpAction::DeleteAllPdpContexts],
            SwmiSndcpState::Ready => vec![
                SwmiSndcpAction::StopReadyTimer,
                SwmiSndcpAction::StopContextReadyTimers,
                SwmiSndcpAction::DeleteAllPdpContexts,
            ],
        };

        self.active_pdp_contexts = 0;
        self.enter_idle();
        Ok(transition(previous_state, self.state, actions))
    }

    pub fn service_user_data_request(&mut self) -> Result<SwmiSndcpTransition, SwmiSndcpStateError> {
        if self.active_pdp_contexts == 0 {
            return Err(SwmiSndcpStateError::NoActivePdpContext);
        }
        if self.state != SwmiSndcpState::Standby {
            return Err(invalid(self.state, SwmiSndcpEvent::ServiceUserDataRequest));
        }

        let previous_state = self.state;
        self.enter_ready();
        Ok(transition(
            previous_state,
            self.state,
            vec![
                SwmiSndcpAction::TransmitDataTransmitRequest,
                SwmiSndcpAction::StopStandbyTimer,
                SwmiSndcpAction::StartReadyTimer,
                SwmiSndcpAction::StartContextReadyTimers,
            ],
        ))
    }

    pub fn data_transmit_request_received(&mut self) -> Result<SwmiSndcpTransition, SwmiSndcpStateError> {
        if self.active_pdp_contexts == 0 {
            return Err(SwmiSndcpStateError::NoActivePdpContext);
        }
        if self.state != SwmiSndcpState::Standby {
            return Err(invalid(self.state, SwmiSndcpEvent::DataTransmitRequestReceived));
        }

        let previous_state = self.state;
        self.enter_ready();
        Ok(transition(
            previous_state,
            self.state,
            vec![
                SwmiSndcpAction::TransmitDataTransmitResponseAccept,
                SwmiSndcpAction::StopStandbyTimer,
                SwmiSndcpAction::StartReadyTimer,
                SwmiSndcpAction::StartContextReadyTimers,
            ],
        ))
    }

    pub fn packet_data_transferred(&mut self) -> Result<SwmiSndcpTransition, SwmiSndcpStateError> {
        if self.active_pdp_contexts == 0 {
            return Err(SwmiSndcpStateError::NoActivePdpContext);
        }
        if self.state != SwmiSndcpState::Ready {
            return Err(invalid(self.state, SwmiSndcpEvent::PacketDataTransferred));
        }

        let previous_state = self.state;
        self.timers.ready = true;
        self.timers.context_ready = true;
        Ok(transition(
            previous_state,
            self.state,
            vec![SwmiSndcpAction::RestartReadyTimer, SwmiSndcpAction::RestartContextReadyTimers],
        ))
    }

    pub fn ready_timer_expired(&mut self) -> Result<SwmiSndcpTransition, SwmiSndcpStateError> {
        if self.state != SwmiSndcpState::Ready {
            return Err(invalid(self.state, SwmiSndcpEvent::ReadyTimerExpired));
        }

        let previous_state = self.state;
        self.enter_standby_from_ready();
        Ok(transition(
            previous_state,
            self.state,
            vec![
                SwmiSndcpAction::TransmitEndOfData,
                SwmiSndcpAction::StopReadyTimer,
                SwmiSndcpAction::StopContextReadyTimers,
                SwmiSndcpAction::StartStandbyTimer,
            ],
        ))
    }

    pub fn end_of_data_received(
        &mut self,
        immediate_service_change: bool,
        swmi_transmitting_tl_sdu: bool,
    ) -> Result<SwmiSndcpTransition, SwmiSndcpStateError> {
        if self.state != SwmiSndcpState::Ready {
            return Err(invalid(self.state, SwmiSndcpEvent::EndOfDataReceived));
        }
        if !immediate_service_change && swmi_transmitting_tl_sdu {
            return Err(SwmiSndcpStateError::SwmiStillTransmittingTlSdu);
        }

        let previous_state = self.state;
        self.enter_standby_from_ready();
        let mut actions = Vec::new();
        if !immediate_service_change {
            actions.push(SwmiSndcpAction::TransmitEndOfData);
        }
        actions.extend([
            SwmiSndcpAction::StopReadyTimer,
            SwmiSndcpAction::StopContextReadyTimers,
            SwmiSndcpAction::StartStandbyTimer,
        ]);
        Ok(transition(previous_state, self.state, actions))
    }

    pub fn reconnect_received(&mut self) -> Result<SwmiSndcpTransition, SwmiSndcpStateError> {
        if self.state != SwmiSndcpState::Ready {
            return Err(invalid(self.state, SwmiSndcpEvent::ReconnectReceived));
        }

        let previous_state = self.state;
        self.enter_standby_from_ready();
        Ok(transition(
            previous_state,
            self.state,
            vec![
                SwmiSndcpAction::StopReadyTimer,
                SwmiSndcpAction::StopContextReadyTimers,
                SwmiSndcpAction::StartStandbyTimer,
            ],
        ))
    }

    fn enter_idle(&mut self) {
        self.state = SwmiSndcpState::Idle;
        self.timers = SwmiSndcpTimers::default();
    }

    fn enter_standby_from_idle(&mut self) {
        self.state = SwmiSndcpState::Standby;
        self.timers = SwmiSndcpTimers {
            standby: true,
            ready: false,
            context_ready: false,
        };
    }

    fn enter_standby_from_ready(&mut self) {
        self.state = SwmiSndcpState::Standby;
        self.timers = SwmiSndcpTimers {
            standby: true,
            ready: false,
            context_ready: false,
        };
    }

    fn enter_ready(&mut self) {
        self.state = SwmiSndcpState::Ready;
        self.timers = SwmiSndcpTimers {
            standby: false,
            ready: true,
            context_ready: true,
        };
    }
}

fn transition(previous_state: SwmiSndcpState, new_state: SwmiSndcpState, actions: Vec<SwmiSndcpAction>) -> SwmiSndcpTransition {
    SwmiSndcpTransition {
        previous_state,
        new_state,
        actions,
    }
}

fn invalid(state: SwmiSndcpState, event: SwmiSndcpEvent) -> SwmiSndcpStateError {
    SwmiSndcpStateError::InvalidEvent { state, event }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_machine() -> SwmiSndcpStateMachine {
        let mut machine = SwmiSndcpStateMachine::new();
        machine.activate_pdp_context().expect("first PDP context should enter STANDBY");
        machine
            .service_user_data_request()
            .expect("service data request should enter READY");
        machine
    }

    #[test]
    fn first_pdp_context_activation_enters_standby() {
        let mut machine = SwmiSndcpStateMachine::new();

        let transition = machine.activate_pdp_context().expect("first activation should enter STANDBY");

        assert_eq!(transition.previous_state, SwmiSndcpState::Idle);
        assert_eq!(transition.new_state, SwmiSndcpState::Standby);
        assert_eq!(transition.actions, vec![SwmiSndcpAction::StartStandbyTimer]);
        assert_eq!(machine.active_pdp_contexts(), 1);
        assert_eq!(
            machine.timers(),
            SwmiSndcpTimers {
                standby: true,
                ready: false,
                context_ready: false
            }
        );
    }

    #[test]
    fn additional_contexts_do_not_change_swmi_state() {
        let mut machine = SwmiSndcpStateMachine::new();
        machine.activate_pdp_context().unwrap();

        let transition = machine
            .activate_pdp_context()
            .expect("additional PDP context should not change state");

        assert_eq!(transition.previous_state, SwmiSndcpState::Standby);
        assert_eq!(transition.new_state, SwmiSndcpState::Standby);
        assert!(transition.actions.is_empty());
        assert_eq!(machine.active_pdp_contexts(), 2);
    }

    #[test]
    fn last_context_deactivation_returns_standby_to_idle() {
        let mut machine = SwmiSndcpStateMachine::new();
        machine.activate_pdp_context().unwrap();
        machine.activate_pdp_context().unwrap();

        let first = machine
            .deactivate_pdp_context()
            .expect("non-last context deactivation should not change state");
        assert_eq!(first.new_state, SwmiSndcpState::Standby);
        assert!(first.actions.is_empty());

        let last = machine
            .deactivate_pdp_context()
            .expect("last context deactivation should enter IDLE");
        assert_eq!(last.previous_state, SwmiSndcpState::Standby);
        assert_eq!(last.new_state, SwmiSndcpState::Idle);
        assert_eq!(last.actions, vec![SwmiSndcpAction::StopStandbyTimer]);
        assert_eq!(machine.active_pdp_contexts(), 0);
        assert_eq!(machine.timers(), SwmiSndcpTimers::default());
    }

    #[test]
    fn standby_timer_expiry_deletes_all_contexts_and_returns_idle() {
        let mut machine = SwmiSndcpStateMachine::new();
        machine.activate_pdp_context().unwrap();
        machine.activate_pdp_context().unwrap();

        let transition = machine.standby_timer_expired().expect("STANDBY timer expiry should enter IDLE");

        assert_eq!(transition.previous_state, SwmiSndcpState::Standby);
        assert_eq!(transition.new_state, SwmiSndcpState::Idle);
        assert_eq!(
            transition.actions,
            vec![
                SwmiSndcpAction::StopStandbyTimer,
                SwmiSndcpAction::DeleteAllPdpContexts,
                SwmiSndcpAction::IndicateAllNsapisDeallocated
            ]
        );
        assert_eq!(machine.active_pdp_contexts(), 0);
    }

    #[test]
    fn swmi_origin_data_request_moves_standby_to_ready() {
        let mut machine = SwmiSndcpStateMachine::new();
        machine.activate_pdp_context().unwrap();

        let transition = machine
            .service_user_data_request()
            .expect("SwMI data request in STANDBY should enter READY");

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
        assert_eq!(
            machine.timers(),
            SwmiSndcpTimers {
                standby: false,
                ready: true,
                context_ready: true
            }
        );
    }

    #[test]
    fn ms_data_transmit_request_moves_swmi_standby_to_ready_with_accept_response() {
        let mut machine = SwmiSndcpStateMachine::new();
        machine.activate_pdp_context().unwrap();

        let transition = machine
            .data_transmit_request_received()
            .expect("MS SN-DATA TRANSMIT REQUEST should enter READY");

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
    }

    #[test]
    fn point_to_point_packet_data_transfer_restarts_ready_timers() {
        let mut machine = ready_machine();

        let transition = machine
            .packet_data_transferred()
            .expect("P2P SN-DATA/SN-UNITDATA transfer in READY should restart timers");

        assert_eq!(transition.previous_state, SwmiSndcpState::Ready);
        assert_eq!(transition.new_state, SwmiSndcpState::Ready);
        assert_eq!(
            transition.actions,
            vec![SwmiSndcpAction::RestartReadyTimer, SwmiSndcpAction::RestartContextReadyTimers]
        );
    }

    #[test]
    fn ready_timer_expiry_transmits_end_of_data_and_returns_standby() {
        let mut machine = ready_machine();

        let transition = machine.ready_timer_expired().expect("READY timer expiry should enter STANDBY");

        assert_eq!(transition.previous_state, SwmiSndcpState::Ready);
        assert_eq!(transition.new_state, SwmiSndcpState::Standby);
        assert_eq!(
            transition.actions,
            vec![
                SwmiSndcpAction::TransmitEndOfData,
                SwmiSndcpAction::StopReadyTimer,
                SwmiSndcpAction::StopContextReadyTimers,
                SwmiSndcpAction::StartStandbyTimer
            ]
        );
        assert_eq!(
            machine.timers(),
            SwmiSndcpTimers {
                standby: true,
                ready: false,
                context_ready: false
            }
        );
    }

    #[test]
    fn end_of_data_without_service_change_echoes_end_of_data_when_swmi_is_not_transmitting() {
        let mut machine = ready_machine();

        let transition = machine
            .end_of_data_received(false, false)
            .expect("SN-END OF DATA without immediate service change should enter STANDBY");

        assert_eq!(transition.new_state, SwmiSndcpState::Standby);
        assert_eq!(
            transition.actions,
            vec![
                SwmiSndcpAction::TransmitEndOfData,
                SwmiSndcpAction::StopReadyTimer,
                SwmiSndcpAction::StopContextReadyTimers,
                SwmiSndcpAction::StartStandbyTimer
            ]
        );
    }

    #[test]
    fn end_of_data_with_service_change_returns_standby_without_echo() {
        let mut machine = ready_machine();

        let transition = machine
            .end_of_data_received(true, false)
            .expect("immediate service change should enter STANDBY");

        assert_eq!(
            transition.actions,
            vec![
                SwmiSndcpAction::StopReadyTimer,
                SwmiSndcpAction::StopContextReadyTimers,
                SwmiSndcpAction::StartStandbyTimer
            ]
        );
    }

    #[test]
    fn end_of_data_without_service_change_waits_when_swmi_is_still_transmitting() {
        let mut machine = ready_machine();

        assert_eq!(
            machine.end_of_data_received(false, true),
            Err(SwmiSndcpStateError::SwmiStillTransmittingTlSdu)
        );
        assert_eq!(machine.state(), SwmiSndcpState::Ready);
    }

    #[test]
    fn reconnect_received_returns_ready_to_standby() {
        let mut machine = ready_machine();

        let transition = machine.reconnect_received().expect("SN-RECONNECT should return SwMI to STANDBY");

        assert_eq!(transition.previous_state, SwmiSndcpState::Ready);
        assert_eq!(transition.new_state, SwmiSndcpState::Standby);
        assert_eq!(
            transition.actions,
            vec![
                SwmiSndcpAction::StopReadyTimer,
                SwmiSndcpAction::StopContextReadyTimers,
                SwmiSndcpAction::StartStandbyTimer
            ]
        );
    }

    #[test]
    fn deregistration_from_ready_deletes_contexts_and_returns_idle() {
        let mut machine = ready_machine();

        let transition = machine.deregister_ms().expect("deregistration should return to IDLE");

        assert_eq!(transition.previous_state, SwmiSndcpState::Ready);
        assert_eq!(transition.new_state, SwmiSndcpState::Idle);
        assert_eq!(
            transition.actions,
            vec![
                SwmiSndcpAction::StopReadyTimer,
                SwmiSndcpAction::StopContextReadyTimers,
                SwmiSndcpAction::DeleteAllPdpContexts
            ]
        );
        assert_eq!(machine.active_pdp_contexts(), 0);
        assert_eq!(machine.timers(), SwmiSndcpTimers::default());
    }

    #[test]
    fn invalid_events_are_rejected_without_state_mutation() {
        let mut machine = SwmiSndcpStateMachine::new();

        assert_eq!(machine.service_user_data_request(), Err(SwmiSndcpStateError::NoActivePdpContext));
        assert_eq!(
            machine.ready_timer_expired(),
            Err(SwmiSndcpStateError::InvalidEvent {
                state: SwmiSndcpState::Idle,
                event: SwmiSndcpEvent::ReadyTimerExpired
            })
        );
        assert_eq!(machine.state(), SwmiSndcpState::Idle);
        assert_eq!(machine.active_pdp_contexts(), 0);
        assert_eq!(machine.timers(), SwmiSndcpTimers::default());
    }
}
