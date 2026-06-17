// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original pure TETRA SNDCP PDCH/MLE-CONFIGURE planning primitive.

use std::collections::BTreeMap;

use super::state::SwmiSndcpState;
use tetra_core::{EndpointId, Layer2Service, LinkId, Todo};
use tetra_saps::lcmc::{
    enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment},
    fields::chan_alloc_req::CmceChanAllocReq,
};
use tetra_saps::ltpd::{LtpdMleConfigureInd, LtpdMleConfigureReq, LtpdMleUnitdataInd};
use tetra_saps::sn::{SnPacketDataMsType, validate_nsapi};

pub const SNDCP_LTPD_NOT_APPLICABLE: Todo = -1;
pub const SNDCP_BASIC_LINK_ID: LinkId = 0;

pub const SNDCP_STATUS_IDLE_TODO: Todo = 0;
pub const SNDCP_STATUS_STANDBY_TODO: Todo = 1;
pub const SNDCP_STATUS_READY_TODO: Todo = 2;

pub const LTPD_CONFIG_REASON_RECEPTION_STOPPED: Todo = 0;
pub const LTPD_CONFIG_REASON_TRANSMISSION_STOPPED: Todo = 1;
pub const LTPD_CONFIG_REASON_USAGE_MARKER_MISMATCH: Todo = 2;
pub const LTPD_CONFIG_REASON_LOSS_OF_RADIO_RESOURCES: Todo = 3;
pub const LTPD_CONFIG_REASON_RECOVERY_OF_RADIO_RESOURCES: Todo = 4;
pub const SNDCP_PDCH_TIMESLOT_MIN: u8 = 1;
pub const SNDCP_PDCH_TIMESLOT_MAX: u8 = 4;
pub const SNDCP_PDCH_ASSIGNED_SCCH_TIMESLOTS: [bool; 4] = [false, true, true, true];
pub const SNDCP_PDCH_SINGLE_ASSIGNED_SCCH_TIMESLOT: [bool; 4] = [false, true, false, false];
pub const SNDCP_TRAFFIC_USAGE_MARKER_MIN: u8 = 4;
pub const SNDCP_TRAFFIC_USAGE_MARKER_MAX: u8 = 62;

pub fn normalize_pdch_timeslots_to_single(timeslots: [bool; 4]) -> [bool; 4] {
    let selected_idx = timeslots
        .iter()
        .enumerate()
        .find_map(|(idx, assigned)| (*assigned && idx > 0).then_some(idx))
        .or_else(|| timeslots.iter().enumerate().find_map(|(idx, assigned)| (*assigned).then_some(idx)));

    let mut normalized = [false; 4];
    if let Some(selected_idx) = selected_idx {
        normalized[selected_idx] = true;
    }
    normalized
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpPdchState {
    CommonControl,
    ChannelChangePending { handle: Todo },
    PdchReady,
    RadioResourceLost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpChannelChangeDecision {
    Accept,
    Reject,
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpLtpdConfigureReason {
    ReceptionStopped,
    TransmissionStopped,
    UsageMarkerMismatch,
    LossOfRadioResources,
    RecoveryOfRadioResources,
    Unknown(Todo),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpStatusForMle {
    Idle,
    StandingBy,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpMsDefaultDataPriority {
    NotApplicable,
    Priority(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SndcpPdchSession {
    pub endpoint_id: EndpointId,
    pub link_id: LinkId,
    pub state: SndcpPdchState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpPacketDataAllocationDecision {
    CurrentCommonControl,
    ExistingPdch,
    NewPdchAllocation,
    ReturnToCommonControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpMacChannelAllocationPlacement {
    None,
    MacResource,
    MacEndAfterFragmentation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpChannelAdviceRequest {
    None,
    NonConformingPdchRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpPacketDataResourceRequest {
    None,
    PhaseModulation(SndcpPhaseModulationResourceRequest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SndcpPhaseModulationResourceRequest {
    pub uplink_timeslots: u8,
    pub downlink_timeslots: u8,
    pub full_phase_modulation_capability_timeslots: u8,
    pub unspecified_phase_modulation_resource: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SndcpPacketDataPlanInput {
    pub issi: u32,
    pub nsapi: u8,
    pub endpoint_id: EndpointId,
    pub link_id: LinkId,
    pub swmi_state: SwmiSndcpState,
    pub packet_data_ms_type: SnPacketDataMsType,
    pub layer2service: Layer2Service,
    pub pdu_priority: u8,
    pub data_priority: Option<u8>,
    pub unacked_bl_repetitions: Option<u8>,
    pub scheduled_data_status: Option<Todo>,
    pub fcs_flag: bool,
    pub current_channel_packet_data_suitable: bool,
    pub allow_common_control_packet_data: bool,
    pub pdch_available: bool,
    pub channel_advice_request: SndcpChannelAdviceRequest,
    pub resource_request: SndcpPacketDataResourceRequest,
    pub downlink_sdu_bits: usize,
    pub nonfragmented_sdu_capacity_bits: Option<usize>,
    pub fragmented_channel_allocation_mac_end_supported: bool,
    pub active_circuit_mode_service: bool,
    pub parallel_voice_data_permitted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SndcpPacketDataChannelPlan {
    pub issi: u32,
    pub nsapi: u8,
    pub endpoint_id: EndpointId,
    pub link_id: LinkId,
    pub allocation: SndcpPacketDataAllocationDecision,
    pub layer2service: Layer2Service,
    pub pdu_priority: u8,
    pub data_priority: Option<u8>,
    pub unacked_bl_repetitions: Option<u8>,
    pub scheduled_data_status: Option<Todo>,
    pub packet_data_flag: bool,
    pub fcs_flag: bool,
    pub mac_channel_allocation_placement: SndcpMacChannelAllocationPlacement,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SndcpEndOfDataPlanInput {
    pub issi: u32,
    pub endpoint_id: EndpointId,
    pub link_id: LinkId,
    pub swmi_state: SwmiSndcpState,
    pub layer2service: Layer2Service,
    pub pdu_priority: u8,
    pub downlink_sdu_bits: usize,
    pub nonfragmented_sdu_capacity_bits: Option<usize>,
    pub fragmented_channel_allocation_mac_end_supported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SndcpEndOfDataChannelPlan {
    pub issi: u32,
    pub endpoint_id: EndpointId,
    pub link_id: LinkId,
    pub allocation: SndcpPacketDataAllocationDecision,
    pub layer2service: Layer2Service,
    pub pdu_priority: u8,
    pub packet_data_flag: bool,
    pub mac_channel_allocation_placement: SndcpMacChannelAllocationPlacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SndcpPdchAllocationPolicy {
    pub timeslots: [bool; 4],
    /// Packet-data PDCH is an assigned SCCH. ETSI traffic usage markers are
    /// not required for assigned control, but this stays optional so callers
    /// that deliberately test legacy marker-bearing allocations still validate.
    pub usage_marker: Option<u8>,
    pub carrier: Option<Todo>,
    pub ul_dl_assignment: UlDlAssignment,
    pub allow_mcch_timeslot: bool,
}

#[derive(Debug, Clone)]
pub struct SndcpLowerChannelAllocation {
    pub issi: u32,
    pub allocation: SndcpPacketDataAllocationDecision,
    pub placement: SndcpMacChannelAllocationPlacement,
    pub chan_alloc: CmceChanAllocReq,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SndcpPdchError {
    ReservedNsapi(u8),
    PacketDataStateNotReady {
        issi: u32,
        state: SwmiSndcpState,
    },
    UnsupportedPacketDataMsType(SnPacketDataMsType),
    UnsupportedLayer2Service(Layer2Service),
    UnsupportedChannelAdvice(SndcpChannelAdviceRequest),
    UnsupportedResourceRequest(SndcpPacketDataResourceRequest),
    ResourceRequestTimeslotsOutOfRange(u8),
    PduPriorityOutOfRange(u8),
    DataPriorityOutOfRange(u8),
    NegativeScheduledDataStatus(Todo),
    CircuitModeConflict {
        issi: u32,
    },
    NoSuitablePacketDataChannel {
        issi: u32,
    },
    MissingChannelChangeHandle {
        issi: u32,
    },
    UnexpectedChannelChangeHandle {
        issi: u32,
        handle: Todo,
    },
    ChannelChangeResponseRequired {
        issi: u32,
        handle: Todo,
    },
    NoPendingChannelChange {
        issi: u32,
    },
    UnknownSession {
        issi: u32,
    },
    EndpointLinkMismatch {
        issi: u32,
        expected_endpoint_id: EndpointId,
        expected_link_id: LinkId,
        actual_endpoint_id: EndpointId,
        actual_link_id: LinkId,
    },
    PacketDataBearerNotReady {
        issi: u32,
        state: SndcpPdchState,
    },
    UnknownNonfragmentedMacCapacity {
        issi: u32,
    },
    FragmentedChannelAllocationNeedsMacEndSupport {
        issi: u32,
        downlink_sdu_bits: usize,
        nonfragmented_sdu_capacity_bits: usize,
    },
    PacketDataChannelAllocationNotRequired {
        issi: u32,
        allocation: SndcpPacketDataAllocationDecision,
    },
    InvalidPdchTimeslot(u8),
    McchTimeslotRequiresExplicitPolicy(u8),
    MissingPdchUsageMarker,
    InvalidPdchUsageMarker(u8),
    UnsupportedPdchCarrier(Todo),
    UnsupportedPdchDirection(UlDlAssignment),
    UnsupportedPdchAllocationType(ChanAllocType),
    InvalidPdchTimeslotSelection(usize),
    UnsupportedAllocationPlacement(SndcpMacChannelAllocationPlacement),
}

#[derive(Debug, Clone, Default)]
pub struct SndcpPdchManager {
    sessions: BTreeMap<u32, SndcpPdchSession>,
}

impl From<SwmiSndcpState> for SndcpStatusForMle {
    fn from(value: SwmiSndcpState) -> Self {
        match value {
            SwmiSndcpState::Idle => SndcpStatusForMle::Idle,
            SwmiSndcpState::Standby => SndcpStatusForMle::StandingBy,
            SwmiSndcpState::Ready => SndcpStatusForMle::Ready,
        }
    }
}

impl SndcpPdchManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn session(&self, issi: u32) -> Option<SndcpPdchSession> {
        self.sessions.get(&issi).copied()
    }

    pub fn session_issis_for_endpoint(&self, endpoint_id: EndpointId) -> Vec<u32> {
        self.sessions
            .iter()
            .filter_map(|(issi, session)| (session.endpoint_id == endpoint_id).then_some(*issi))
            .collect()
    }

    pub fn observe_ltpd_unitdata_ind(&mut self, issi: u32, ind: &LtpdMleUnitdataInd) -> Result<(), SndcpPdchError> {
        let session = self.ensure_session(issi, ind.endpoint_id, ind.link_id);
        validate_endpoint_link(issi, session, ind.endpoint_id, ind.link_id)?;

        if ind.chan_change_resp_req {
            let handle = ind.chan_change_handle.ok_or(SndcpPdchError::MissingChannelChangeHandle { issi })?;
            session.state = SndcpPdchState::ChannelChangePending { handle };
            return Err(SndcpPdchError::ChannelChangeResponseRequired { issi, handle });
        }
        if let Some(handle) = ind.chan_change_handle {
            return Err(SndcpPdchError::UnexpectedChannelChangeHandle { issi, handle });
        }

        Ok(())
    }

    pub fn handle_ltpd_configure_ind_fail_closed(
        &mut self,
        issi: u32,
        ind: &LtpdMleConfigureInd,
        status: SndcpStatusForMle,
    ) -> Result<Option<LtpdMleConfigureReq>, SndcpPdchError> {
        let session = self.ensure_session(issi, ind.endpoint_id, SNDCP_BASIC_LINK_ID);

        match SndcpLtpdConfigureReason::from_todo(ind.reason_for_config_indication) {
            SndcpLtpdConfigureReason::LossOfRadioResources => {
                session.state = SndcpPdchState::RadioResourceLost;
                return build_ltpd_configure_req(ind.endpoint_id, None, None, status, SndcpMsDefaultDataPriority::NotApplicable).map(Some);
            }
            SndcpLtpdConfigureReason::RecoveryOfRadioResources => {
                session.state = SndcpPdchState::CommonControl;
                session.link_id = SNDCP_BASIC_LINK_ID;
            }
            _ => {}
        }

        if ind.chan_change_responce_required {
            session.state = SndcpPdchState::ChannelChangePending {
                handle: ind.chan_change_handle,
            };
            return self
                .respond_to_pending_channel_change(
                    issi,
                    SndcpChannelChangeDecision::Reject,
                    status,
                    SndcpMsDefaultDataPriority::NotApplicable,
                )
                .map(Some);
        }

        Ok(None)
    }

    pub fn respond_to_pending_channel_change(
        &mut self,
        issi: u32,
        decision: SndcpChannelChangeDecision,
        status: SndcpStatusForMle,
        ms_default_data_priority: SndcpMsDefaultDataPriority,
    ) -> Result<LtpdMleConfigureReq, SndcpPdchError> {
        let session = self.sessions.get_mut(&issi).ok_or(SndcpPdchError::UnknownSession { issi })?;
        let SndcpPdchState::ChannelChangePending { handle } = session.state else {
            return Err(SndcpPdchError::NoPendingChannelChange { issi });
        };

        let req = build_ltpd_configure_req(session.endpoint_id, Some(decision), Some(handle), status, ms_default_data_priority)?;

        match decision {
            SndcpChannelChangeDecision::Accept => {
                session.state = SndcpPdchState::ChannelChangePending { handle };
            }
            SndcpChannelChangeDecision::Reject | SndcpChannelChangeDecision::Ignore => {
                session.state = SndcpPdchState::CommonControl;
            }
        }

        Ok(req)
    }

    pub fn mark_pdch_ready(&mut self, issi: u32, endpoint_id: EndpointId, link_id: LinkId) {
        let session = self.ensure_session(issi, endpoint_id, link_id);
        session.endpoint_id = endpoint_id;
        session.link_id = link_id;
        session.state = SndcpPdchState::PdchReady;
    }

    pub fn mark_common_control(&mut self, issi: u32) -> Result<(), SndcpPdchError> {
        let session = self.sessions.get_mut(&issi).ok_or(SndcpPdchError::UnknownSession { issi })?;
        session.link_id = SNDCP_BASIC_LINK_ID;
        session.state = SndcpPdchState::CommonControl;
        Ok(())
    }

    pub fn mark_common_control_on_link(&mut self, issi: u32, endpoint_id: EndpointId, link_id: LinkId) {
        let session = self.ensure_session(issi, endpoint_id, link_id);
        session.endpoint_id = endpoint_id;
        session.link_id = link_id;
        session.state = SndcpPdchState::CommonControl;
    }

    pub fn deregister_issi(&mut self, issi: u32) {
        self.sessions.remove(&issi);
    }

    pub fn ensure_packet_data_ready(&self, issi: u32, endpoint_id: EndpointId, link_id: LinkId) -> Result<(), SndcpPdchError> {
        let session = self.sessions.get(&issi).ok_or(SndcpPdchError::PacketDataBearerNotReady {
            issi,
            state: SndcpPdchState::CommonControl,
        })?;
        validate_endpoint_link(issi, session, endpoint_id, link_id)?;

        if session.state != SndcpPdchState::PdchReady {
            return Err(SndcpPdchError::PacketDataBearerNotReady {
                issi,
                state: session.state,
            });
        }

        Ok(())
    }

    pub fn plan_swmi_unitdata_channel(&self, input: SndcpPacketDataPlanInput) -> Result<SndcpPacketDataChannelPlan, SndcpPdchError> {
        validate_packet_data_plan_input(input)?;

        if input.swmi_state != SwmiSndcpState::Ready {
            return Err(SndcpPdchError::PacketDataStateNotReady {
                issi: input.issi,
                state: input.swmi_state,
            });
        }

        if input.active_circuit_mode_service && !input.parallel_voice_data_permitted {
            return Err(SndcpPdchError::CircuitModeConflict { issi: input.issi });
        }

        if let Some(session) = self.sessions.get(&input.issi) {
            validate_endpoint_link(input.issi, session, input.endpoint_id, input.link_id)?;
            match session.state {
                SndcpPdchState::PdchReady => {
                    return packet_data_channel_plan(input, SndcpPacketDataAllocationDecision::ExistingPdch);
                }
                SndcpPdchState::ChannelChangePending { .. } | SndcpPdchState::RadioResourceLost => {
                    return Err(SndcpPdchError::PacketDataBearerNotReady {
                        issi: input.issi,
                        state: session.state,
                    });
                }
                SndcpPdchState::CommonControl => {}
            }
        }

        if input.current_channel_packet_data_suitable && input.allow_common_control_packet_data && !input.active_circuit_mode_service {
            return packet_data_channel_plan(input, SndcpPacketDataAllocationDecision::CurrentCommonControl);
        }

        if input.pdch_available {
            return packet_data_channel_plan(input, SndcpPacketDataAllocationDecision::NewPdchAllocation);
        }

        Err(SndcpPdchError::NoSuitablePacketDataChannel { issi: input.issi })
    }

    pub fn plan_swmi_end_of_data_channel(&self, input: SndcpEndOfDataPlanInput) -> Result<SndcpEndOfDataChannelPlan, SndcpPdchError> {
        validate_end_of_data_plan_input(input)?;

        if input.swmi_state != SwmiSndcpState::Ready {
            return Err(SndcpPdchError::PacketDataStateNotReady {
                issi: input.issi,
                state: input.swmi_state,
            });
        }

        self.ensure_packet_data_ready(input.issi, input.endpoint_id, input.link_id)?;

        Ok(SndcpEndOfDataChannelPlan {
            issi: input.issi,
            endpoint_id: input.endpoint_id,
            link_id: SNDCP_BASIC_LINK_ID,
            allocation: SndcpPacketDataAllocationDecision::ReturnToCommonControl,
            layer2service: input.layer2service,
            pdu_priority: input.pdu_priority,
            packet_data_flag: false,
            mac_channel_allocation_placement: plan_mac_allocation_placement(
                input.issi,
                SndcpPacketDataAllocationDecision::ReturnToCommonControl,
                input.downlink_sdu_bits,
                input.nonfragmented_sdu_capacity_bits,
                input.fragmented_channel_allocation_mac_end_supported,
            )?,
        })
    }

    pub fn mark_return_to_common_control_transmitted(&mut self, issi: u32) -> Result<(), SndcpPdchError> {
        self.mark_common_control(issi)
    }

    fn ensure_session(&mut self, issi: u32, endpoint_id: EndpointId, link_id: LinkId) -> &mut SndcpPdchSession {
        self.sessions.entry(issi).or_insert(SndcpPdchSession {
            endpoint_id,
            link_id,
            state: SndcpPdchState::CommonControl,
        })
    }
}

impl Default for SndcpPacketDataPlanInput {
    fn default() -> Self {
        Self {
            issi: 0,
            nsapi: 1,
            endpoint_id: 0,
            link_id: SNDCP_BASIC_LINK_ID,
            swmi_state: SwmiSndcpState::Ready,
            packet_data_ms_type: SnPacketDataMsType::TypeAParallel,
            layer2service: Layer2Service::Unacknowledged,
            pdu_priority: 4,
            data_priority: None,
            unacked_bl_repetitions: Some(0),
            scheduled_data_status: None,
            fcs_flag: false,
            current_channel_packet_data_suitable: false,
            allow_common_control_packet_data: false,
            pdch_available: false,
            channel_advice_request: SndcpChannelAdviceRequest::None,
            resource_request: SndcpPacketDataResourceRequest::None,
            downlink_sdu_bits: 0,
            nonfragmented_sdu_capacity_bits: None,
            fragmented_channel_allocation_mac_end_supported: false,
            active_circuit_mode_service: false,
            parallel_voice_data_permitted: false,
        }
    }
}

impl SndcpPdchAllocationPolicy {
    pub fn single_slot(timeslot: u8, usage_marker: Option<u8>) -> Self {
        let mut timeslots = [false; 4];
        if (SNDCP_PDCH_TIMESLOT_MIN..=SNDCP_PDCH_TIMESLOT_MAX).contains(&timeslot) {
            timeslots[(timeslot - 1) as usize] = true;
        }
        Self::timeslots(timeslots, usage_marker)
    }

    pub fn timeslots(timeslots: [bool; 4], usage_marker: Option<u8>) -> Self {
        Self {
            timeslots,
            usage_marker,
            carrier: None,
            ul_dl_assignment: UlDlAssignment::Both,
            allow_mcch_timeslot: false,
        }
    }

    pub fn assigned_scch_for_resource_request(resource_request: SndcpPacketDataResourceRequest) -> Self {
        Self::timeslots(assigned_scch_pdch_timeslots_for_resource_request(resource_request), None)
    }

    pub fn with_allow_mcch_timeslot(mut self, allow_mcch_timeslot: bool) -> Self {
        self.allow_mcch_timeslot = allow_mcch_timeslot;
        self
    }

    pub fn with_carrier(mut self, carrier: Option<Todo>) -> Self {
        self.carrier = carrier;
        self
    }

    pub fn with_ul_dl_assignment(mut self, ul_dl_assignment: UlDlAssignment) -> Self {
        self.ul_dl_assignment = ul_dl_assignment;
        self
    }
}

impl SndcpLtpdConfigureReason {
    pub fn from_todo(value: Todo) -> Self {
        match value {
            LTPD_CONFIG_REASON_RECEPTION_STOPPED => SndcpLtpdConfigureReason::ReceptionStopped,
            LTPD_CONFIG_REASON_TRANSMISSION_STOPPED => SndcpLtpdConfigureReason::TransmissionStopped,
            LTPD_CONFIG_REASON_USAGE_MARKER_MISMATCH => SndcpLtpdConfigureReason::UsageMarkerMismatch,
            LTPD_CONFIG_REASON_LOSS_OF_RADIO_RESOURCES => SndcpLtpdConfigureReason::LossOfRadioResources,
            LTPD_CONFIG_REASON_RECOVERY_OF_RADIO_RESOURCES => SndcpLtpdConfigureReason::RecoveryOfRadioResources,
            other => SndcpLtpdConfigureReason::Unknown(other),
        }
    }
}

impl SndcpStatusForMle {
    pub fn to_todo(self) -> Todo {
        match self {
            SndcpStatusForMle::Idle => SNDCP_STATUS_IDLE_TODO,
            SndcpStatusForMle::StandingBy => SNDCP_STATUS_STANDBY_TODO,
            SndcpStatusForMle::Ready => SNDCP_STATUS_READY_TODO,
        }
    }
}

impl SndcpMsDefaultDataPriority {
    pub fn to_todo(self) -> Result<Todo, SndcpPdchError> {
        match self {
            SndcpMsDefaultDataPriority::NotApplicable => Ok(SNDCP_LTPD_NOT_APPLICABLE),
            SndcpMsDefaultDataPriority::Priority(priority) if priority <= 7 => Ok(priority as Todo),
            SndcpMsDefaultDataPriority::Priority(priority) => Err(SndcpPdchError::DataPriorityOutOfRange(priority)),
        }
    }
}

pub fn packet_data_plan_to_lower_channel_allocation(
    plan: &SndcpPacketDataChannelPlan,
    policy: SndcpPdchAllocationPolicy,
) -> Result<Option<SndcpLowerChannelAllocation>, SndcpPdchError> {
    match plan.allocation {
        SndcpPacketDataAllocationDecision::CurrentCommonControl | SndcpPacketDataAllocationDecision::ExistingPdch => Ok(None),
        SndcpPacketDataAllocationDecision::NewPdchAllocation => {
            validate_pdch_allocation_policy(policy)?;
            let placement = validate_new_allocation_placement(plan.mac_channel_allocation_placement)?;
            let policy = single_slot_pdch_allocation_policy(policy);

            Ok(Some(SndcpLowerChannelAllocation {
                issi: plan.issi,
                allocation: plan.allocation,
                placement,
                chan_alloc: CmceChanAllocReq {
                    usage: policy.usage_marker,
                    carrier: policy.carrier,
                    timeslots: policy.timeslots,
                    alloc_type: ChanAllocType::Replace,
                    ul_dl_assigned: policy.ul_dl_assignment,
                },
            }))
        }
        SndcpPacketDataAllocationDecision::ReturnToCommonControl => Err(SndcpPdchError::PacketDataChannelAllocationNotRequired {
            issi: plan.issi,
            allocation: plan.allocation,
        }),
    }
}

pub fn end_of_data_plan_to_lower_channel_allocation(
    plan: &SndcpEndOfDataChannelPlan,
) -> Result<SndcpLowerChannelAllocation, SndcpPdchError> {
    if plan.allocation != SndcpPacketDataAllocationDecision::ReturnToCommonControl {
        return Err(SndcpPdchError::PacketDataChannelAllocationNotRequired {
            issi: plan.issi,
            allocation: plan.allocation,
        });
    }
    let placement = validate_new_allocation_placement(plan.mac_channel_allocation_placement)?;

    Ok(SndcpLowerChannelAllocation {
        issi: plan.issi,
        allocation: plan.allocation,
        placement,
        chan_alloc: CmceChanAllocReq {
            usage: None,
            carrier: None,
            timeslots: [false; 4],
            alloc_type: ChanAllocType::QuitAndGo,
            ul_dl_assigned: UlDlAssignment::Both,
        },
    })
}

pub fn validate_lower_channel_allocation(allocation: &SndcpLowerChannelAllocation) -> Result<(), SndcpPdchError> {
    validate_new_allocation_placement(allocation.placement)?;
    if allocation.chan_alloc.carrier.is_some() {
        return Err(SndcpPdchError::UnsupportedPdchCarrier(allocation.chan_alloc.carrier.unwrap()));
    }

    match allocation.allocation {
        SndcpPacketDataAllocationDecision::NewPdchAllocation => {
            if let Some(usage_marker) = allocation.chan_alloc.usage
                && !(SNDCP_TRAFFIC_USAGE_MARKER_MIN..=SNDCP_TRAFFIC_USAGE_MARKER_MAX).contains(&usage_marker)
            {
                return Err(SndcpPdchError::InvalidPdchUsageMarker(usage_marker));
            }
            let timeslot_count = allocation.chan_alloc.timeslots.iter().filter(|assigned| **assigned).count();
            if timeslot_count != 1 {
                return Err(SndcpPdchError::InvalidPdchTimeslotSelection(timeslot_count));
            }
            if allocation.chan_alloc.timeslots[0] {
                return Err(SndcpPdchError::McchTimeslotRequiresExplicitPolicy(1));
            }
            if allocation.chan_alloc.alloc_type != ChanAllocType::Replace {
                return Err(SndcpPdchError::UnsupportedPdchAllocationType(allocation.chan_alloc.alloc_type));
            }
            if allocation.chan_alloc.ul_dl_assigned != UlDlAssignment::Both {
                return Err(SndcpPdchError::UnsupportedPdchDirection(allocation.chan_alloc.ul_dl_assigned));
            }
        }
        SndcpPacketDataAllocationDecision::ReturnToCommonControl => {
            if allocation.chan_alloc.usage.is_some()
                || allocation.chan_alloc.timeslots.iter().any(|assigned| *assigned)
                || allocation.chan_alloc.alloc_type != ChanAllocType::QuitAndGo
            {
                return Err(SndcpPdchError::UnsupportedPdchAllocationType(allocation.chan_alloc.alloc_type));
            }
        }
        SndcpPacketDataAllocationDecision::CurrentCommonControl | SndcpPacketDataAllocationDecision::ExistingPdch => {
            return Err(SndcpPdchError::PacketDataChannelAllocationNotRequired {
                issi: allocation.issi,
                allocation: allocation.allocation,
            });
        }
    }

    Ok(())
}

pub fn build_ltpd_configure_req(
    endpoint_id: EndpointId,
    channel_change_decision: Option<SndcpChannelChangeDecision>,
    channel_change_handle: Option<Todo>,
    status: SndcpStatusForMle,
    ms_default_data_priority: SndcpMsDefaultDataPriority,
) -> Result<LtpdMleConfigureReq, SndcpPdchError> {
    Ok(LtpdMleConfigureReq {
        chan_change_accepted: channel_change_decision.and_then(channel_change_decision_to_ltpd),
        chan_change_handle: channel_change_handle.unwrap_or(SNDCP_LTPD_NOT_APPLICABLE),
        call_release: SNDCP_LTPD_NOT_APPLICABLE,
        endpoint_id,
        encryption_flag: false,
        ms_default_data_prio: ms_default_data_priority.to_todo()?,
        layer2_data_prio_lifetime: SNDCP_LTPD_NOT_APPLICABLE,
        layer2_data_prio_signalling_delay: SNDCP_LTPD_NOT_APPLICABLE,
        data_prio_random_access_delay_factor: SNDCP_LTPD_NOT_APPLICABLE,
        data_class_info: SNDCP_LTPD_NOT_APPLICABLE,
        schedule_repetition_info: SNDCP_LTPD_NOT_APPLICABLE,
        sndcp_status: status.to_todo(),
    })
}

fn single_slot_pdch_allocation_policy(mut policy: SndcpPdchAllocationPolicy) -> SndcpPdchAllocationPolicy {
    policy.timeslots = normalize_pdch_timeslots_to_single(policy.timeslots);
    policy
}

fn validate_pdch_allocation_policy(policy: SndcpPdchAllocationPolicy) -> Result<(), SndcpPdchError> {
    let timeslot_count = policy.timeslots.iter().filter(|assigned| **assigned).count();
    if timeslot_count == 0 || timeslot_count > SNDCP_PDCH_TIMESLOT_MAX.saturating_sub(1) as usize {
        return Err(SndcpPdchError::InvalidPdchTimeslotSelection(timeslot_count));
    }
    for (idx, assigned) in policy.timeslots.iter().enumerate() {
        if !*assigned {
            continue;
        }
        let timeslot = (idx + 1) as u8;
        if !(SNDCP_PDCH_TIMESLOT_MIN..=SNDCP_PDCH_TIMESLOT_MAX).contains(&timeslot) {
            return Err(SndcpPdchError::InvalidPdchTimeslot(timeslot));
        }
    }
    if policy.timeslots[0] && !policy.allow_mcch_timeslot {
        return Err(SndcpPdchError::McchTimeslotRequiresExplicitPolicy(1));
    }
    if let Some(usage_marker) = policy.usage_marker
        && !(SNDCP_TRAFFIC_USAGE_MARKER_MIN..=SNDCP_TRAFFIC_USAGE_MARKER_MAX).contains(&usage_marker)
    {
        return Err(SndcpPdchError::InvalidPdchUsageMarker(usage_marker));
    }
    if let Some(carrier) = policy.carrier {
        return Err(SndcpPdchError::UnsupportedPdchCarrier(carrier));
    }
    if policy.ul_dl_assignment != UlDlAssignment::Both {
        return Err(SndcpPdchError::UnsupportedPdchDirection(policy.ul_dl_assignment));
    }
    Ok(())
}

fn validate_new_allocation_placement(
    placement: SndcpMacChannelAllocationPlacement,
) -> Result<SndcpMacChannelAllocationPlacement, SndcpPdchError> {
    match placement {
        SndcpMacChannelAllocationPlacement::MacResource | SndcpMacChannelAllocationPlacement::MacEndAfterFragmentation => Ok(placement),
        SndcpMacChannelAllocationPlacement::None => Err(SndcpPdchError::UnsupportedAllocationPlacement(placement)),
    }
}

fn validate_packet_data_plan_input(input: SndcpPacketDataPlanInput) -> Result<(), SndcpPdchError> {
    if validate_nsapi(input.nsapi).is_err() {
        return Err(SndcpPdchError::ReservedNsapi(input.nsapi));
    }
    match input.packet_data_ms_type {
        SnPacketDataMsType::TypeAParallel | SnPacketDataMsType::TypeBAlternating | SnPacketDataMsType::TypeCIpSingleMode => {}
        SnPacketDataMsType::TypeDRestrictedIpSingleMode => {
            return Err(SndcpPdchError::UnsupportedPacketDataMsType(input.packet_data_ms_type));
        }
    }
    validate_channel_advice_and_resource_request(input.channel_advice_request, input.resource_request)?;
    match input.layer2service {
        Layer2Service::Acknowledged | Layer2Service::AcknowledgedResponse | Layer2Service::Unacknowledged => {}
        other => return Err(SndcpPdchError::UnsupportedLayer2Service(other)),
    }
    if input.pdu_priority > 7 {
        return Err(SndcpPdchError::PduPriorityOutOfRange(input.pdu_priority));
    }
    if let Some(data_priority) = input.data_priority {
        if data_priority > 7 {
            return Err(SndcpPdchError::DataPriorityOutOfRange(data_priority));
        }
    }
    if let Some(scheduled_data_status) = input.scheduled_data_status {
        if scheduled_data_status < 0 {
            return Err(SndcpPdchError::NegativeScheduledDataStatus(scheduled_data_status));
        }
    }
    Ok(())
}

fn validate_end_of_data_plan_input(input: SndcpEndOfDataPlanInput) -> Result<(), SndcpPdchError> {
    match input.layer2service {
        Layer2Service::Acknowledged | Layer2Service::AcknowledgedResponse | Layer2Service::Unacknowledged => {}
        other => return Err(SndcpPdchError::UnsupportedLayer2Service(other)),
    }
    if input.pdu_priority > 7 {
        return Err(SndcpPdchError::PduPriorityOutOfRange(input.pdu_priority));
    }
    Ok(())
}

fn validate_channel_advice_and_resource_request(
    channel_advice_request: SndcpChannelAdviceRequest,
    resource_request: SndcpPacketDataResourceRequest,
) -> Result<(), SndcpPdchError> {
    if channel_advice_request != SndcpChannelAdviceRequest::None {
        return Err(SndcpPdchError::UnsupportedChannelAdvice(channel_advice_request));
    }

    if let SndcpPacketDataResourceRequest::PhaseModulation(request) = resource_request {
        for timeslots in [
            request.uplink_timeslots,
            request.downlink_timeslots,
            request.full_phase_modulation_capability_timeslots,
        ] {
            if !(1..=4).contains(&timeslots) {
                return Err(SndcpPdchError::ResourceRequestTimeslotsOutOfRange(timeslots));
            }
        }
        if request.uplink_timeslots > request.full_phase_modulation_capability_timeslots
            || request.downlink_timeslots > request.full_phase_modulation_capability_timeslots
        {
            return Err(SndcpPdchError::UnsupportedResourceRequest(resource_request));
        }
        if request.uplink_timeslots == 1 && request.downlink_timeslots == 1 {
            return Ok(());
        }
        // Keep asymmetric or impossible phase-modulation requests fail-closed;
        // symmetric multi-slot requests are then scaled by the lower allocation
        // policy to non-MCCH assigned-SCCH slots.
        if request.uplink_timeslots == request.downlink_timeslots {
            return Ok(());
        }
        if request.unspecified_phase_modulation_resource
            && request.uplink_timeslots == request.downlink_timeslots
            && request.uplink_timeslots == request.full_phase_modulation_capability_timeslots
        {
            return Ok(());
        }
        return Err(SndcpPdchError::UnsupportedResourceRequest(resource_request));
    }

    Ok(())
}

fn assigned_scch_pdch_timeslots_for_resource_request(_resource_request: SndcpPacketDataResourceRequest) -> [bool; 4] {
    SNDCP_PDCH_SINGLE_ASSIGNED_SCCH_TIMESLOT
}

fn packet_data_channel_plan(
    input: SndcpPacketDataPlanInput,
    allocation: SndcpPacketDataAllocationDecision,
) -> Result<SndcpPacketDataChannelPlan, SndcpPdchError> {
    Ok(SndcpPacketDataChannelPlan {
        issi: input.issi,
        nsapi: input.nsapi,
        endpoint_id: input.endpoint_id,
        link_id: input.link_id,
        allocation,
        layer2service: input.layer2service,
        pdu_priority: input.pdu_priority,
        data_priority: input.data_priority,
        unacked_bl_repetitions: input.unacked_bl_repetitions,
        scheduled_data_status: input.scheduled_data_status,
        packet_data_flag: true,
        fcs_flag: input.fcs_flag,
        mac_channel_allocation_placement: plan_mac_allocation_placement(
            input.issi,
            allocation,
            input.downlink_sdu_bits,
            input.nonfragmented_sdu_capacity_bits,
            input.fragmented_channel_allocation_mac_end_supported,
        )?,
    })
}

fn plan_mac_allocation_placement(
    issi: u32,
    allocation: SndcpPacketDataAllocationDecision,
    downlink_sdu_bits: usize,
    nonfragmented_sdu_capacity_bits: Option<usize>,
    fragmented_channel_allocation_mac_end_supported: bool,
) -> Result<SndcpMacChannelAllocationPlacement, SndcpPdchError> {
    if matches!(
        allocation,
        SndcpPacketDataAllocationDecision::CurrentCommonControl | SndcpPacketDataAllocationDecision::ExistingPdch
    ) {
        return Ok(SndcpMacChannelAllocationPlacement::None);
    }

    let nonfragmented_sdu_capacity_bits =
        nonfragmented_sdu_capacity_bits.ok_or(SndcpPdchError::UnknownNonfragmentedMacCapacity { issi })?;
    if downlink_sdu_bits <= nonfragmented_sdu_capacity_bits {
        return Ok(SndcpMacChannelAllocationPlacement::MacResource);
    }
    if fragmented_channel_allocation_mac_end_supported {
        return Ok(SndcpMacChannelAllocationPlacement::MacEndAfterFragmentation);
    }

    Err(SndcpPdchError::FragmentedChannelAllocationNeedsMacEndSupport {
        issi,
        downlink_sdu_bits,
        nonfragmented_sdu_capacity_bits,
    })
}

fn channel_change_decision_to_ltpd(decision: SndcpChannelChangeDecision) -> Option<bool> {
    match decision {
        SndcpChannelChangeDecision::Accept => Some(true),
        SndcpChannelChangeDecision::Reject => Some(false),
        SndcpChannelChangeDecision::Ignore => None,
    }
}

fn validate_endpoint_link(issi: u32, session: &SndcpPdchSession, endpoint_id: EndpointId, link_id: LinkId) -> Result<(), SndcpPdchError> {
    if session.state == SndcpPdchState::PdchReady && session.endpoint_id != endpoint_id {
        return Err(SndcpPdchError::EndpointLinkMismatch {
            issi,
            expected_endpoint_id: session.endpoint_id,
            expected_link_id: session.link_id,
            actual_endpoint_id: endpoint_id,
            actual_link_id: link_id,
        });
    }
    if session.state == SndcpPdchState::PdchReady {
        return Ok(());
    }
    if session.endpoint_id != endpoint_id || session.link_id != link_id {
        return Err(SndcpPdchError::EndpointLinkMismatch {
            issi,
            expected_endpoint_id: session.endpoint_id,
            expected_link_id: session.link_id,
            actual_endpoint_id: endpoint_id,
            actual_link_id: link_id,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_core::{BitBuffer, SsiType, TetraAddress};

    const ISSI: u32 = 2_260_618;

    fn ltpd_ind(endpoint_id: EndpointId, link_id: LinkId) -> LtpdMleUnitdataInd {
        LtpdMleUnitdataInd {
            sdu: BitBuffer::from_bytes(&[0x40, 0x20]),
            endpoint_id,
            link_id,
            received_tetra_address: TetraAddress::new(ISSI, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }
    }

    fn packet_plan_input() -> SndcpPacketDataPlanInput {
        SndcpPacketDataPlanInput {
            issi: ISSI,
            nsapi: 2,
            endpoint_id: 3,
            link_id: 7,
            pdu_priority: 4,
            data_priority: Some(2),
            downlink_sdu_bits: 80,
            nonfragmented_sdu_capacity_bits: Some(124),
            ..SndcpPacketDataPlanInput::default()
        }
    }

    fn pdch_policy() -> SndcpPdchAllocationPolicy {
        SndcpPdchAllocationPolicy::timeslots([false, true, true, true], None)
    }

    fn end_of_data_input() -> SndcpEndOfDataPlanInput {
        SndcpEndOfDataPlanInput {
            issi: ISSI,
            endpoint_id: 3,
            link_id: 7,
            swmi_state: SwmiSndcpState::Ready,
            layer2service: Layer2Service::Acknowledged,
            pdu_priority: 4,
            downlink_sdu_bits: 6,
            nonfragmented_sdu_capacity_bits: Some(124),
            fragmented_channel_allocation_mac_end_supported: false,
        }
    }

    #[test]
    fn configure_request_maps_status_and_default_priority_to_ltpd_parameters() {
        let req = build_ltpd_configure_req(3, None, None, SndcpStatusForMle::Ready, SndcpMsDefaultDataPriority::Priority(5))
            .expect("valid configure request should build");

        assert_eq!(req.endpoint_id, 3);
        assert_eq!(req.chan_change_accepted, None);
        assert_eq!(req.chan_change_handle, SNDCP_LTPD_NOT_APPLICABLE);
        assert_eq!(req.ms_default_data_prio, 5);
        assert_eq!(req.sndcp_status, SNDCP_STATUS_READY_TODO);
    }

    #[test]
    fn configure_request_rejects_out_of_range_ms_default_priority() {
        assert_eq!(
            build_ltpd_configure_req(
                3,
                None,
                None,
                SndcpStatusForMle::StandingBy,
                SndcpMsDefaultDataPriority::Priority(8)
            )
            .expect_err("out-of-range MS default data priority should reject"),
            SndcpPdchError::DataPriorityOutOfRange(8)
        );
    }

    #[test]
    fn channel_change_accept_does_not_mark_pdch_ready_until_lower_layer_fact_arrives() {
        let mut manager = SndcpPdchManager::new();
        let mut ind = ltpd_ind(3, 7);
        ind.chan_change_resp_req = true;
        ind.chan_change_handle = Some(44);

        assert_eq!(
            manager.observe_ltpd_unitdata_ind(ISSI, &ind),
            Err(SndcpPdchError::ChannelChangeResponseRequired { issi: ISSI, handle: 44 })
        );
        let req = manager
            .respond_to_pending_channel_change(
                ISSI,
                SndcpChannelChangeDecision::Accept,
                SndcpStatusForMle::Ready,
                SndcpMsDefaultDataPriority::NotApplicable,
            )
            .expect("pending channel change can be accepted");

        assert_eq!(req.chan_change_accepted, Some(true));
        assert_eq!(req.chan_change_handle, 44);
        assert_eq!(
            manager.ensure_packet_data_ready(ISSI, 3, 7),
            Err(SndcpPdchError::PacketDataBearerNotReady {
                issi: ISSI,
                state: SndcpPdchState::ChannelChangePending { handle: 44 }
            })
        );

        manager.mark_pdch_ready(ISSI, 3, 7);
        assert_eq!(manager.ensure_packet_data_ready(ISSI, 3, 7), Ok(()));
    }

    #[test]
    fn packet_data_ready_bearer_accepts_advanced_link_data_on_same_endpoint() {
        let mut manager = SndcpPdchManager::new();
        manager.mark_pdch_ready(ISSI, 3, SNDCP_BASIC_LINK_ID);

        assert!(
            manager.ensure_packet_data_ready(ISSI, 3, 1).is_ok(),
            "SN-DATA TRANSMIT REQUEST establishes the RF PDCH bearer on basic link, but SN-UNITDATA arrives on the negotiated AL link"
        );
        assert_eq!(
            manager.ensure_packet_data_ready(ISSI, 4, 1),
            Err(SndcpPdchError::EndpointLinkMismatch {
                issi: ISSI,
                expected_endpoint_id: 3,
                expected_link_id: SNDCP_BASIC_LINK_ID,
                actual_endpoint_id: 4,
                actual_link_id: 1,
            }),
            "PDCH bearer readiness is endpoint-scoped, not globally shared"
        );
    }

    #[test]
    fn packet_data_plan_uses_existing_pdch_only_after_pdch_ready_fact() {
        let mut manager = SndcpPdchManager::new();
        manager.mark_pdch_ready(ISSI, 3, 7);

        let plan = manager
            .plan_swmi_unitdata_channel(packet_plan_input())
            .expect("ready PDCH should produce packet-data plan");

        assert_eq!(plan.allocation, SndcpPacketDataAllocationDecision::ExistingPdch);
        assert_eq!(plan.endpoint_id, 3);
        assert_eq!(plan.link_id, 7);
        assert_eq!(plan.layer2service, Layer2Service::Unacknowledged);
        assert_eq!(plan.pdu_priority, 4);
        assert_eq!(plan.data_priority, Some(2));
        assert!(plan.packet_data_flag);
        assert_eq!(plan.mac_channel_allocation_placement, SndcpMacChannelAllocationPlacement::None);
    }

    #[test]
    fn packet_data_plan_requires_swmi_ready_and_supported_ms_type() {
        let manager = SndcpPdchManager::new();

        assert_eq!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                swmi_state: SwmiSndcpState::Standby,
                ..packet_plan_input()
            }),
            Err(SndcpPdchError::PacketDataStateNotReady {
                issi: ISSI,
                state: SwmiSndcpState::Standby
            })
        );
        for packet_data_ms_type in [SnPacketDataMsType::TypeBAlternating, SnPacketDataMsType::TypeCIpSingleMode] {
            assert!(matches!(
                manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                    packet_data_ms_type,
                    pdch_available: true,
                    ..packet_plan_input()
                }),
                Ok(SndcpPacketDataChannelPlan {
                    allocation: SndcpPacketDataAllocationDecision::NewPdchAllocation,
                    ..
                })
            ));
        }
        assert_eq!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                packet_data_ms_type: SnPacketDataMsType::TypeDRestrictedIpSingleMode,
                ..packet_plan_input()
            }),
            Err(SndcpPdchError::UnsupportedPacketDataMsType(
                SnPacketDataMsType::TypeDRestrictedIpSingleMode
            ))
        );
    }

    #[test]
    fn packet_data_plan_rejects_voice_conflict_without_explicit_parallel_policy() {
        let manager = SndcpPdchManager::new();

        assert_eq!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                active_circuit_mode_service: true,
                pdch_available: true,
                ..packet_plan_input()
            }),
            Err(SndcpPdchError::CircuitModeConflict { issi: ISSI })
        );

        let plan = manager
            .plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                active_circuit_mode_service: true,
                parallel_voice_data_permitted: true,
                pdch_available: true,
                ..packet_plan_input()
            })
            .expect("explicit parallel policy can request a PDCH allocation");
        assert_eq!(plan.allocation, SndcpPacketDataAllocationDecision::NewPdchAllocation);
        assert_eq!(
            plan.mac_channel_allocation_placement,
            SndcpMacChannelAllocationPlacement::MacResource
        );
    }

    #[test]
    fn packet_data_plan_uses_common_control_only_when_explicitly_allowed() {
        let manager = SndcpPdchManager::new();

        assert_eq!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                current_channel_packet_data_suitable: true,
                allow_common_control_packet_data: false,
                ..packet_plan_input()
            }),
            Err(SndcpPdchError::NoSuitablePacketDataChannel { issi: ISSI })
        );

        let plan = manager
            .plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                current_channel_packet_data_suitable: true,
                allow_common_control_packet_data: true,
                ..packet_plan_input()
            })
            .expect("explicit common-control permission should produce a plan");
        assert_eq!(plan.allocation, SndcpPacketDataAllocationDecision::CurrentCommonControl);
        assert_eq!(plan.mac_channel_allocation_placement, SndcpMacChannelAllocationPlacement::None);
    }

    #[test]
    fn packet_data_plan_models_channel_advice_and_resource_request_fail_closed() {
        let manager = SndcpPdchManager::new();

        assert_eq!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                channel_advice_request: SndcpChannelAdviceRequest::NonConformingPdchRequested,
                current_channel_packet_data_suitable: true,
                allow_common_control_packet_data: true,
                ..packet_plan_input()
            }),
            Err(SndcpPdchError::UnsupportedChannelAdvice(
                SndcpChannelAdviceRequest::NonConformingPdchRequested
            ))
        );

        let single_slot_resource_request = SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: 1,
            downlink_timeslots: 1,
            full_phase_modulation_capability_timeslots: 1,
            unspecified_phase_modulation_resource: false,
        });
        assert!(matches!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                resource_request: single_slot_resource_request,
                current_channel_packet_data_suitable: false,
                allow_common_control_packet_data: false,
                pdch_available: true,
                ..packet_plan_input()
            }),
            Ok(SndcpPacketDataChannelPlan {
                allocation: SndcpPacketDataAllocationDecision::NewPdchAllocation,
                ..
            })
        ));

        let unspecified_four_slot_capability_request =
            SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
                uplink_timeslots: 4,
                downlink_timeslots: 4,
                full_phase_modulation_capability_timeslots: 4,
                unspecified_phase_modulation_resource: true,
            });
        assert!(matches!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                resource_request: unspecified_four_slot_capability_request,
                current_channel_packet_data_suitable: false,
                allow_common_control_packet_data: false,
                pdch_available: true,
                ..packet_plan_input()
            }),
            Ok(SndcpPacketDataChannelPlan {
                allocation: SndcpPacketDataAllocationDecision::NewPdchAllocation,
                ..
            })
        ));

        let specific_four_slot_symmetric_request = SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: 4,
            downlink_timeslots: 4,
            full_phase_modulation_capability_timeslots: 4,
            unspecified_phase_modulation_resource: false,
        });
        assert!(matches!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                resource_request: specific_four_slot_symmetric_request,
                current_channel_packet_data_suitable: false,
                allow_common_control_packet_data: false,
                pdch_available: true,
                ..packet_plan_input()
            }),
            Ok(SndcpPacketDataChannelPlan {
                allocation: SndcpPacketDataAllocationDecision::NewPdchAllocation,
                ..
            })
        ));

        let multi_slot_resource_request = SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: 2,
            downlink_timeslots: 1,
            full_phase_modulation_capability_timeslots: 2,
            unspecified_phase_modulation_resource: false,
        });
        assert_eq!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                resource_request: multi_slot_resource_request,
                current_channel_packet_data_suitable: true,
                allow_common_control_packet_data: true,
                ..packet_plan_input()
            }),
            Err(SndcpPdchError::UnsupportedResourceRequest(multi_slot_resource_request))
        );

        assert_eq!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                resource_request: SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
                    uplink_timeslots: 0,
                    downlink_timeslots: 1,
                    full_phase_modulation_capability_timeslots: 1,
                    unspecified_phase_modulation_resource: false,
                }),
                current_channel_packet_data_suitable: true,
                allow_common_control_packet_data: true,
                ..packet_plan_input()
            }),
            Err(SndcpPdchError::ResourceRequestTimeslotsOutOfRange(0))
        );
    }

    #[test]
    fn packet_data_plan_places_new_pdch_allocation_only_when_mac_boundary_is_known() {
        let manager = SndcpPdchManager::new();

        assert_eq!(
            manager
                .plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                    pdch_available: true,
                    downlink_sdu_bits: 120,
                    nonfragmented_sdu_capacity_bits: Some(124),
                    ..packet_plan_input()
                })
                .expect("non-fragmented allocation should be allowed")
                .mac_channel_allocation_placement,
            SndcpMacChannelAllocationPlacement::MacResource
        );

        assert_eq!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                pdch_available: true,
                nonfragmented_sdu_capacity_bits: None,
                ..packet_plan_input()
            }),
            Err(SndcpPdchError::UnknownNonfragmentedMacCapacity { issi: ISSI })
        );

        assert_eq!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                pdch_available: true,
                downlink_sdu_bits: 180,
                nonfragmented_sdu_capacity_bits: Some(124),
                fragmented_channel_allocation_mac_end_supported: false,
                ..packet_plan_input()
            }),
            Err(SndcpPdchError::FragmentedChannelAllocationNeedsMacEndSupport {
                issi: ISSI,
                downlink_sdu_bits: 180,
                nonfragmented_sdu_capacity_bits: 124
            })
        );

        assert_eq!(
            manager
                .plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                    pdch_available: true,
                    downlink_sdu_bits: 180,
                    nonfragmented_sdu_capacity_bits: Some(124),
                    fragmented_channel_allocation_mac_end_supported: true,
                    ..packet_plan_input()
                })
                .expect("fragmented allocation is allowed only with MAC-END support")
                .mac_channel_allocation_placement,
            SndcpMacChannelAllocationPlacement::MacEndAfterFragmentation
        );
    }

    #[test]
    fn packet_data_plan_validates_lower_layer_fields() {
        let manager = SndcpPdchManager::new();

        assert_eq!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                nsapi: 0,
                ..packet_plan_input()
            }),
            Err(SndcpPdchError::ReservedNsapi(0))
        );
        assert_eq!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                layer2service: Layer2Service::Todo,
                ..packet_plan_input()
            }),
            Err(SndcpPdchError::UnsupportedLayer2Service(Layer2Service::Todo))
        );
        assert_eq!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                pdu_priority: 8,
                ..packet_plan_input()
            }),
            Err(SndcpPdchError::PduPriorityOutOfRange(8))
        );
        assert_eq!(
            manager.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                scheduled_data_status: Some(-2),
                ..packet_plan_input()
            }),
            Err(SndcpPdchError::NegativeScheduledDataStatus(-2))
        );
    }

    #[test]
    fn channel_change_reject_clears_pending_and_keeps_packet_data_closed() {
        let mut manager = SndcpPdchManager::new();
        let mut ind = ltpd_ind(3, 7);
        ind.chan_change_resp_req = true;
        ind.chan_change_handle = Some(45);
        assert!(manager.observe_ltpd_unitdata_ind(ISSI, &ind).is_err());

        let req = manager
            .respond_to_pending_channel_change(
                ISSI,
                SndcpChannelChangeDecision::Reject,
                SndcpStatusForMle::StandingBy,
                SndcpMsDefaultDataPriority::NotApplicable,
            )
            .expect("pending channel change can be rejected");

        assert_eq!(req.chan_change_accepted, Some(false));
        assert_eq!(req.chan_change_handle, 45);
        assert_eq!(
            manager.ensure_packet_data_ready(ISSI, 3, 7),
            Err(SndcpPdchError::PacketDataBearerNotReady {
                issi: ISSI,
                state: SndcpPdchState::CommonControl
            })
        );
    }

    #[test]
    fn channel_change_handle_without_response_required_fails_closed() {
        let mut manager = SndcpPdchManager::new();
        let mut ind = ltpd_ind(3, 7);
        ind.chan_change_resp_req = false;
        ind.chan_change_handle = Some(46);

        assert_eq!(
            manager.observe_ltpd_unitdata_ind(ISSI, &ind),
            Err(SndcpPdchError::UnexpectedChannelChangeHandle { issi: ISSI, handle: 46 })
        );
        assert_eq!(
            manager.ensure_packet_data_ready(ISSI, 3, 7),
            Err(SndcpPdchError::PacketDataBearerNotReady {
                issi: ISSI,
                state: SndcpPdchState::CommonControl
            })
        );
    }

    #[test]
    fn new_pdch_plan_maps_to_lower_channel_allocation_request() {
        let manager = SndcpPdchManager::new();
        let plan = manager
            .plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                pdch_available: true,
                ..packet_plan_input()
            })
            .expect("new PDCH should produce a pure channel plan");

        let lower = packet_data_plan_to_lower_channel_allocation(&plan, pdch_policy())
            .expect("PDCH plan should map to lower allocation")
            .expect("new PDCH plan should carry a lower allocation");

        assert_eq!(lower.issi, ISSI);
        assert_eq!(lower.allocation, SndcpPacketDataAllocationDecision::NewPdchAllocation);
        assert_eq!(lower.placement, SndcpMacChannelAllocationPlacement::MacResource);
        assert_eq!(lower.chan_alloc.usage, None);
        assert_eq!(lower.chan_alloc.carrier, None);
        assert_eq!(lower.chan_alloc.timeslots, [false, true, false, false]);
        assert_eq!(
            lower.chan_alloc.timeslots.iter().filter(|assigned| **assigned).count(),
            1,
            "packet-data lower allocation must advertise exactly one PDCH timeslot"
        );
        assert_eq!(lower.chan_alloc.alloc_type, ChanAllocType::Replace);
        assert_eq!(lower.chan_alloc.ul_dl_assigned, UlDlAssignment::Both);
    }

    #[test]
    fn lower_pdch_allocation_validation_is_single_slot_only() {
        let manager = SndcpPdchManager::new();
        let plan = manager
            .plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                pdch_available: true,
                ..packet_plan_input()
            })
            .expect("new PDCH should produce a pure channel plan");
        let mut lower = packet_data_plan_to_lower_channel_allocation(&plan, pdch_policy())
            .expect("PDCH plan should map to lower allocation")
            .expect("new PDCH plan should carry a lower allocation");

        lower.chan_alloc.timeslots = [false, true, true, false];
        assert_eq!(
            validate_lower_channel_allocation(&lower),
            Err(SndcpPdchError::InvalidPdchTimeslotSelection(2)),
            "lower PDCH handoff must reject parallel packet-data timeslots"
        );

        lower.chan_alloc.timeslots = [false, false, false, false];
        assert_eq!(
            validate_lower_channel_allocation(&lower),
            Err(SndcpPdchError::InvalidPdchTimeslotSelection(0)),
            "lower PDCH handoff must reject an empty assigned-channel bitmap"
        );
    }

    #[test]
    fn resource_aware_assigned_scch_policy_maps_single_slot_and_four_slot_requests() {
        let default_policy = SndcpPdchAllocationPolicy::assigned_scch_for_resource_request(SndcpPacketDataResourceRequest::None);
        assert_eq!(default_policy.usage_marker, None);
        assert_eq!(default_policy.timeslots, SNDCP_PDCH_SINGLE_ASSIGNED_SCCH_TIMESLOT);

        let single_slot = SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: 1,
            downlink_timeslots: 1,
            full_phase_modulation_capability_timeslots: 1,
            unspecified_phase_modulation_resource: false,
        });
        let single_policy = SndcpPdchAllocationPolicy::assigned_scch_for_resource_request(single_slot);

        assert_eq!(single_policy.usage_marker, None);
        assert_eq!(single_policy.timeslots, SNDCP_PDCH_SINGLE_ASSIGNED_SCCH_TIMESLOT);
        assert!(!single_policy.timeslots[0], "single-slot PDCH must not allocate MCCH TS1");
        assert!(
            !single_policy.timeslots[2] && !single_policy.timeslots[3],
            "single-slot phase-modulation capability must not expand to TS3/TS4"
        );

        let unspecified_four_slot = SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: 4,
            downlink_timeslots: 4,
            full_phase_modulation_capability_timeslots: 4,
            unspecified_phase_modulation_resource: true,
        });
        let unspecified_policy = SndcpPdchAllocationPolicy::assigned_scch_for_resource_request(unspecified_four_slot);

        assert_eq!(unspecified_policy.usage_marker, None);
        assert_eq!(unspecified_policy.timeslots, SNDCP_PDCH_SINGLE_ASSIGNED_SCCH_TIMESLOT);

        let specific_four_slot = SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: 4,
            downlink_timeslots: 4,
            full_phase_modulation_capability_timeslots: 4,
            unspecified_phase_modulation_resource: false,
        });
        let specific_policy = SndcpPdchAllocationPolicy::assigned_scch_for_resource_request(specific_four_slot);

        assert_eq!(specific_policy.usage_marker, None);
        assert_eq!(specific_policy.timeslots, SNDCP_PDCH_SINGLE_ASSIGNED_SCCH_TIMESLOT);
    }

    #[test]
    fn channel_allocation_adapter_omits_current_or_existing_pdch_allocations() {
        let mut manager = SndcpPdchManager::new();
        let common = manager
            .plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                current_channel_packet_data_suitable: true,
                allow_common_control_packet_data: true,
                ..packet_plan_input()
            })
            .expect("current common control plan should build");
        assert!(
            packet_data_plan_to_lower_channel_allocation(&common, pdch_policy())
                .expect("current-channel plan should not fail")
                .is_none()
        );

        manager.mark_pdch_ready(ISSI, 3, 7);
        let existing = manager
            .plan_swmi_unitdata_channel(packet_plan_input())
            .expect("existing PDCH plan should build");
        assert!(
            packet_data_plan_to_lower_channel_allocation(&existing, pdch_policy())
                .expect("existing PDCH plan should not fail")
                .is_none()
        );
    }

    #[test]
    fn channel_allocation_adapter_validates_pdch_policy_fail_closed() {
        let manager = SndcpPdchManager::new();
        let plan = manager
            .plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                pdch_available: true,
                ..packet_plan_input()
            })
            .expect("new PDCH should produce a pure channel plan");

        assert_eq!(
            packet_data_plan_to_lower_channel_allocation(&plan, SndcpPdchAllocationPolicy::single_slot(0, Some(4)))
                .expect_err("timeslot 0 is invalid"),
            SndcpPdchError::InvalidPdchTimeslotSelection(0)
        );
        assert_eq!(
            packet_data_plan_to_lower_channel_allocation(&plan, SndcpPdchAllocationPolicy::single_slot(1, Some(4)))
                .expect_err("MCCH timeslot should require explicit policy"),
            SndcpPdchError::McchTimeslotRequiresExplicitPolicy(1)
        );
        let ts1 = packet_data_plan_to_lower_channel_allocation(
            &plan,
            SndcpPdchAllocationPolicy::single_slot(1, Some(4)).with_allow_mcch_timeslot(true),
        )
        .expect("explicit MCCH opt-in should be represented")
        .expect("explicit MCCH opt-in should still produce allocation");
        assert_eq!(ts1.chan_alloc.timeslots, [true, false, false, false]);

        let no_marker = packet_data_plan_to_lower_channel_allocation(&plan, SndcpPdchAllocationPolicy::single_slot(2, None))
            .expect("PDCH allocation does not require a traffic usage marker")
            .expect("valid PDCH policy should produce allocation");
        assert_eq!(no_marker.chan_alloc.usage, None);
        assert_eq!(
            packet_data_plan_to_lower_channel_allocation(&plan, SndcpPdchAllocationPolicy::single_slot(2, Some(3)))
                .expect_err("reserved usage marker should reject"),
            SndcpPdchError::InvalidPdchUsageMarker(3)
        );
        assert_eq!(
            packet_data_plan_to_lower_channel_allocation(
                &plan,
                SndcpPdchAllocationPolicy::single_slot(2, Some(4)).with_carrier(Some(1529)),
            )
            .expect_err("non-main-carrier PDCH allocation should fail closed until UMAC serializes carrier"),
            SndcpPdchError::UnsupportedPdchCarrier(1529)
        );
        assert_eq!(
            packet_data_plan_to_lower_channel_allocation(
                &plan,
                SndcpPdchAllocationPolicy::single_slot(2, Some(4)).with_ul_dl_assignment(UlDlAssignment::Dl),
            )
            .expect_err("WAP/IP PDCH MVP requires bidirectional allocation"),
            SndcpPdchError::UnsupportedPdchDirection(UlDlAssignment::Dl)
        );
    }

    #[test]
    fn end_of_data_plan_maps_to_quit_and_go_lower_allocation() {
        let mut manager = SndcpPdchManager::new();
        manager.mark_pdch_ready(ISSI, 3, 7);
        let plan = manager
            .plan_swmi_end_of_data_channel(end_of_data_input())
            .expect("SN-END OF DATA should plan return to common control from PDCH");

        let lower = end_of_data_plan_to_lower_channel_allocation(&plan).expect("SN-END plan should map to lower allocation");

        assert_eq!(lower.issi, ISSI);
        assert_eq!(lower.allocation, SndcpPacketDataAllocationDecision::ReturnToCommonControl);
        assert_eq!(lower.placement, SndcpMacChannelAllocationPlacement::MacResource);
        assert_eq!(lower.chan_alloc.usage, None);
        assert_eq!(lower.chan_alloc.carrier, None);
        assert_eq!(lower.chan_alloc.timeslots, [false, false, false, false]);
        assert_eq!(lower.chan_alloc.alloc_type, ChanAllocType::QuitAndGo);
        assert_eq!(lower.chan_alloc.ul_dl_assigned, UlDlAssignment::Both);
    }

    #[test]
    fn lower_allocation_adapter_rejects_missing_allocation_placement() {
        let plan = SndcpPacketDataChannelPlan {
            issi: ISSI,
            nsapi: 2,
            endpoint_id: 3,
            link_id: 7,
            allocation: SndcpPacketDataAllocationDecision::NewPdchAllocation,
            layer2service: Layer2Service::Unacknowledged,
            pdu_priority: 4,
            data_priority: Some(2),
            unacked_bl_repetitions: Some(0),
            scheduled_data_status: None,
            packet_data_flag: true,
            fcs_flag: false,
            mac_channel_allocation_placement: SndcpMacChannelAllocationPlacement::None,
        };

        assert_eq!(
            packet_data_plan_to_lower_channel_allocation(&plan, pdch_policy())
                .expect_err("new allocation without MAC placement should fail closed"),
            SndcpPdchError::UnsupportedAllocationPlacement(SndcpMacChannelAllocationPlacement::None)
        );

        let end_plan = SndcpEndOfDataChannelPlan {
            issi: ISSI,
            endpoint_id: 3,
            link_id: SNDCP_BASIC_LINK_ID,
            allocation: SndcpPacketDataAllocationDecision::ReturnToCommonControl,
            layer2service: Layer2Service::Acknowledged,
            pdu_priority: 4,
            packet_data_flag: false,
            mac_channel_allocation_placement: SndcpMacChannelAllocationPlacement::None,
        };
        assert_eq!(
            end_of_data_plan_to_lower_channel_allocation(&end_plan)
                .expect_err("SN-END allocation without MAC placement should fail closed"),
            SndcpPdchError::UnsupportedAllocationPlacement(SndcpMacChannelAllocationPlacement::None)
        );
    }

    #[test]
    fn loss_of_radio_resources_clears_pdch_readiness_fail_closed() {
        let mut manager = SndcpPdchManager::new();
        manager.mark_pdch_ready(ISSI, 3, 7);

        let req = manager
            .handle_ltpd_configure_ind_fail_closed(
                ISSI,
                &LtpdMleConfigureInd {
                    received_tetra_address: Some(TetraAddress::issi(ISSI)),
                    endpoint_id: 3,
                    chan_change_responce_required: false,
                    chan_change_handle: -1,
                    reason_for_config_indication: LTPD_CONFIG_REASON_LOSS_OF_RADIO_RESOURCES,
                    conflicting_endpoint_id: 0,
                },
                SndcpStatusForMle::Idle,
            )
            .expect("loss indication should be handled")
            .expect("loss indication should produce an MLE-CONFIGURE.req");

        assert_eq!(req.ms_default_data_prio, SNDCP_LTPD_NOT_APPLICABLE);
        assert_eq!(req.sndcp_status, SNDCP_STATUS_IDLE_TODO);
        assert_eq!(
            manager.ensure_packet_data_ready(ISSI, 3, 7),
            Err(SndcpPdchError::PacketDataBearerNotReady {
                issi: ISSI,
                state: SndcpPdchState::RadioResourceLost
            })
        );
    }

    #[test]
    fn recovery_of_radio_resources_resets_session_to_basic_common_control_link() {
        let mut manager = SndcpPdchManager::new();
        manager.mark_pdch_ready(ISSI, 3, 7);
        let loss = LtpdMleConfigureInd {
            received_tetra_address: Some(TetraAddress::issi(ISSI)),
            endpoint_id: 3,
            chan_change_responce_required: false,
            chan_change_handle: -1,
            reason_for_config_indication: LTPD_CONFIG_REASON_LOSS_OF_RADIO_RESOURCES,
            conflicting_endpoint_id: 0,
        };
        manager
            .handle_ltpd_configure_ind_fail_closed(ISSI, &loss, SndcpStatusForMle::Idle)
            .expect("loss indication should be handled");

        let recovery = LtpdMleConfigureInd {
            reason_for_config_indication: LTPD_CONFIG_REASON_RECOVERY_OF_RADIO_RESOURCES,
            ..loss
        };
        manager
            .handle_ltpd_configure_ind_fail_closed(ISSI, &recovery, SndcpStatusForMle::Ready)
            .expect("recovery indication should be handled");

        let plan = manager
            .plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
                link_id: SNDCP_BASIC_LINK_ID,
                pdch_available: true,
                ..packet_plan_input()
            })
            .expect("post-recovery retry on common control should plan a fresh PDCH");

        assert_eq!(plan.allocation, SndcpPacketDataAllocationDecision::NewPdchAllocation);
    }

    #[test]
    fn end_of_data_plan_returns_ms_to_common_control_after_transmission_report() {
        let mut manager = SndcpPdchManager::new();
        manager.mark_pdch_ready(ISSI, 3, 7);

        let plan = manager
            .plan_swmi_end_of_data_channel(end_of_data_input())
            .expect("SN-END OF DATA should plan return to common control from PDCH");

        assert_eq!(plan.allocation, SndcpPacketDataAllocationDecision::ReturnToCommonControl);
        assert_eq!(plan.endpoint_id, 3);
        assert_eq!(plan.link_id, SNDCP_BASIC_LINK_ID);
        assert_eq!(plan.layer2service, Layer2Service::Acknowledged);
        assert!(!plan.packet_data_flag);
        assert_eq!(
            plan.mac_channel_allocation_placement,
            SndcpMacChannelAllocationPlacement::MacResource
        );

        manager
            .mark_return_to_common_control_transmitted(ISSI)
            .expect("completed SN-END OF DATA should clear PDCH readiness");
        assert_eq!(
            manager.ensure_packet_data_ready(ISSI, 3, SNDCP_BASIC_LINK_ID),
            Err(SndcpPdchError::PacketDataBearerNotReady {
                issi: ISSI,
                state: SndcpPdchState::CommonControl
            })
        );
    }

    #[test]
    fn fragmented_end_of_data_allocation_requires_mac_end_support() {
        let mut manager = SndcpPdchManager::new();
        manager.mark_pdch_ready(ISSI, 3, 7);

        assert_eq!(
            manager.plan_swmi_end_of_data_channel(SndcpEndOfDataPlanInput {
                downlink_sdu_bits: 80,
                nonfragmented_sdu_capacity_bits: Some(32),
                fragmented_channel_allocation_mac_end_supported: false,
                ..end_of_data_input()
            }),
            Err(SndcpPdchError::FragmentedChannelAllocationNeedsMacEndSupport {
                issi: ISSI,
                downlink_sdu_bits: 80,
                nonfragmented_sdu_capacity_bits: 32
            })
        );

        let plan = manager
            .plan_swmi_end_of_data_channel(SndcpEndOfDataPlanInput {
                downlink_sdu_bits: 80,
                nonfragmented_sdu_capacity_bits: Some(32),
                fragmented_channel_allocation_mac_end_supported: true,
                ..end_of_data_input()
            })
            .expect("MAC-END capable fragmentation should be accepted");
        assert_eq!(
            plan.mac_channel_allocation_placement,
            SndcpMacChannelAllocationPlacement::MacEndAfterFragmentation
        );
    }

    #[test]
    fn ready_pdch_rejects_unexpected_endpoint_or_link() {
        let mut manager = SndcpPdchManager::new();
        manager.mark_pdch_ready(ISSI, 3, 7);

        assert_eq!(
            manager.ensure_packet_data_ready(ISSI, 4, 7),
            Err(SndcpPdchError::EndpointLinkMismatch {
                issi: ISSI,
                expected_endpoint_id: 3,
                expected_link_id: 7,
                actual_endpoint_id: 4,
                actual_link_id: 7
            })
        );
    }
}
