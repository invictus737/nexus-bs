// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original pure TETRA SNDCP to MLE/LTPD adapter primitive.

use std::collections::BTreeMap;

use super::pdch::{SndcpLowerChannelAllocation, SndcpPdchError, validate_lower_channel_allocation};
use super::priority::{SndcpDataScheduling, SndcpPriorityError, SndcpPriorityPolicy};
use super::unitdata::{SndcpEncodeError, sn_data_req_to_pdu, sn_unitdata_req_to_pdu};
use tetra_core::{BitBuffer, EndpointId, Layer2Service, LinkId, MleHandle, SsiType, TetraAddress, Todo};
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;
use tetra_saps::ltpd::{LtpdMleReportInd, LtpdMleUnitdataReq};
use tetra_saps::sn::{SnDeliveryInd, SnDeliveryStatus, SnUnitdataReq};
use tetra_saps::tla::{
    TLA_REPORT_FAILED_TRANSFER, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION, TLA_REPORT_NO_SPECIFIC_REPORT, TLA_REPORT_SUCCESSFUL_TRANSFER,
    TlaTlUnitdataReqBl,
};

pub const SNDCP_CONTROL_PDU_PRIORITY: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SndcpLtpdUnitdataOptions {
    pub address: TetraAddress,
    pub endpoint_id: EndpointId,
    pub link_id: LinkId,
    pub layer2service: Layer2Service,
    pub pdu_priority: u8,
    pub data_priority: Option<u8>,
    pub nsapi_data_priority: Option<u8>,
    pub ms_default_data_priority: Option<u8>,
    pub data_scheduling: SndcpDataScheduling,
    pub mle_data_priority_signalling_required: bool,
    pub unacked_bl_repetitions: Option<u8>,
    pub stealing_permission: bool,
    pub stealing_repeats_flag: bool,
    pub channel_advice_flag: bool,
    pub data_class_info: Option<Todo>,
    pub scheduled_data_status: Option<Todo>,
    pub max_schedule_interval: Option<Todo>,
    pub fcs_flag: bool,
    pub packet_data_flag: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SndcpMleAdapterError {
    EmptySdu,
    HandleOutOfRange(MleHandle),
    UnsupportedLayer2Service(Layer2Service),
    PduPriorityOutOfRange(u8),
    DataPriorityOutOfRange(u8),
    MleDataPriorityFlagWithoutDefinedPriority,
    NegativeDataClassInfo(Todo),
    NegativeScheduledDataStatus(Todo),
    NegativeMaxScheduleInterval(Todo),
    UnitdataEncode(SndcpEncodeError),
    Priority(SndcpPriorityError),
    PacketDataFlagRequired,
    PacketDataStealingNotPermitted,
    NonIssiPacketDataAddress(TetraAddress),
    LowerAllocationIssiMismatch { address_issi: u32, allocation_issi: u32 },
    LowerAllocation(SndcpPdchError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpDeliveryTrackerError {
    DuplicateHandle(MleHandle),
    UnknownHandle(MleHandle),
    NegativeHandle(Todo),
    HandleOutOfRange(MleHandle),
    UnsupportedTransferResult(Todo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SndcpDeliveryReportOutcome {
    Progress,
    CompletedWithoutSnDelivery,
    Delivered(SnDeliveryInd),
}

#[derive(Debug, Clone, Default)]
pub struct SndcpDeliveryTracker {
    pending: BTreeMap<MleHandle, SndcpTrackedDelivery>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SndcpTrackedDelivery {
    handle: MleHandle,
}

impl From<SndcpEncodeError> for SndcpMleAdapterError {
    fn from(value: SndcpEncodeError) -> Self {
        SndcpMleAdapterError::UnitdataEncode(value)
    }
}

impl From<SndcpPriorityError> for SndcpMleAdapterError {
    fn from(value: SndcpPriorityError) -> Self {
        SndcpMleAdapterError::Priority(value)
    }
}

impl From<SndcpPdchError> for SndcpMleAdapterError {
    fn from(value: SndcpPdchError) -> Self {
        SndcpMleAdapterError::LowerAllocation(value)
    }
}

impl SndcpLtpdUnitdataOptions {
    pub fn unacknowledged_basic_link(address: TetraAddress, endpoint_id: EndpointId, link_id: LinkId, pdu_priority: u8) -> Self {
        Self {
            address,
            endpoint_id,
            link_id,
            layer2service: Layer2Service::Unacknowledged,
            pdu_priority,
            data_priority: None,
            nsapi_data_priority: None,
            ms_default_data_priority: None,
            data_scheduling: SndcpDataScheduling::NonScheduled,
            mle_data_priority_signalling_required: false,
            unacked_bl_repetitions: Some(0),
            stealing_permission: false,
            stealing_repeats_flag: false,
            channel_advice_flag: false,
            data_class_info: None,
            scheduled_data_status: None,
            max_schedule_interval: None,
            fcs_flag: false,
            packet_data_flag: true,
        }
    }

    pub fn acknowledged_basic_link(address: TetraAddress, endpoint_id: EndpointId, link_id: LinkId, pdu_priority: u8) -> Self {
        Self {
            address,
            endpoint_id,
            link_id,
            layer2service: Layer2Service::Acknowledged,
            pdu_priority,
            data_priority: None,
            nsapi_data_priority: None,
            ms_default_data_priority: None,
            data_scheduling: SndcpDataScheduling::NonScheduled,
            mle_data_priority_signalling_required: false,
            unacked_bl_repetitions: None,
            stealing_permission: false,
            stealing_repeats_flag: false,
            channel_advice_flag: false,
            data_class_info: None,
            scheduled_data_status: None,
            max_schedule_interval: None,
            fcs_flag: false,
            packet_data_flag: false,
        }
    }

    pub fn control_acknowledged(address: TetraAddress, endpoint_id: EndpointId, link_id: LinkId) -> Self {
        Self::acknowledged_basic_link(address, endpoint_id, link_id, SNDCP_CONTROL_PDU_PRIORITY).with_packet_data_flag(false)
    }

    pub fn packet_data_unacknowledged(address: TetraAddress, endpoint_id: EndpointId, link_id: LinkId, pdu_priority: u8) -> Self {
        Self::unacknowledged_basic_link(address, endpoint_id, link_id, pdu_priority).with_packet_data_flag(true)
    }

    pub fn packet_data_acknowledged(address: TetraAddress, endpoint_id: EndpointId, link_id: LinkId, pdu_priority: u8) -> Self {
        Self::acknowledged_basic_link(address, endpoint_id, link_id, pdu_priority).with_packet_data_flag(true)
    }

    pub fn with_pdu_priority(mut self, pdu_priority: u8) -> Self {
        self.pdu_priority = pdu_priority;
        self
    }

    pub fn with_data_priority(mut self, data_priority: Option<u8>) -> Self {
        self.data_priority = data_priority;
        self
    }

    pub fn with_nsapi_data_priority(mut self, data_priority: Option<u8>) -> Self {
        self.nsapi_data_priority = data_priority;
        self
    }

    pub fn with_ms_default_data_priority(mut self, data_priority: Option<u8>) -> Self {
        self.ms_default_data_priority = data_priority;
        self
    }

    pub fn with_data_scheduling(mut self, data_scheduling: SndcpDataScheduling) -> Self {
        self.data_scheduling = data_scheduling;
        self
    }

    pub fn with_mle_data_priority_signalling_required(mut self, required: bool) -> Self {
        self.mle_data_priority_signalling_required = required;
        self
    }

    pub fn with_unacked_bl_repetitions(mut self, repetitions: Option<u8>) -> Self {
        self.unacked_bl_repetitions = repetitions;
        self
    }

    pub fn with_layer2service(mut self, layer2service: Layer2Service) -> Self {
        self.layer2service = layer2service;
        self
    }

    pub fn with_fcs(mut self, fcs_flag: bool) -> Self {
        self.fcs_flag = fcs_flag;
        self
    }

    pub fn with_packet_data_flag(mut self, packet_data_flag: bool) -> Self {
        self.packet_data_flag = packet_data_flag;
        self
    }
}

pub fn sndcp_pdu_to_ltpd_mle_unitdata_req(
    sdu: BitBuffer,
    handle: MleHandle,
    options: SndcpLtpdUnitdataOptions,
) -> Result<LtpdMleUnitdataReq, SndcpMleAdapterError> {
    validate_options(&sdu, handle, options)?;

    Ok(LtpdMleUnitdataReq {
        sdu,
        handle: handle_to_todo(handle)?,
        address: options.address,
        layer2service: options.layer2service,
        unacked_bl_repetitions: optional_u8_to_todo(options.unacked_bl_repetitions),
        pdu_prio: options.pdu_priority as Todo,
        endpoint_id: options.endpoint_id,
        link_id: options.link_id,
        stealing_permission: options.stealing_permission,
        stealing_repeats_flag: options.stealing_repeats_flag,
        channel_advice_flag: options.channel_advice_flag,
        data_class_info: optional_nonnegative_todo(options.data_class_info, SndcpMleAdapterError::NegativeDataClassInfo)?,
        data_prio: optional_u8_to_todo(options.data_priority),
        mle_data_prio_flag: options.mle_data_priority_signalling_required,
        packet_data_flag: options.packet_data_flag,
        scheduled_data_status: optional_nonnegative_todo(options.scheduled_data_status, SndcpMleAdapterError::NegativeScheduledDataStatus)?,
        max_schedule_interval: optional_nonnegative_todo(options.max_schedule_interval, SndcpMleAdapterError::NegativeMaxScheduleInterval)?,
        fcs_flag: options.fcs_flag,
        chan_alloc: None,
    })
}

pub fn sn_unitdata_req_to_ltpd_mle_unitdata_req(
    req: &SnUnitdataReq,
    handle: MleHandle,
    mut options: SndcpLtpdUnitdataOptions,
) -> Result<LtpdMleUnitdataReq, SndcpMleAdapterError> {
    options.packet_data_flag = true;
    let resolved = SndcpPriorityPolicy::packet_data(options.pdu_priority)
        .with_sn_sap_pdu_priority(req.pdu_priority)
        .with_sn_sap_data_priority(req.data_priority)
        .with_nsapi_data_priority(options.nsapi_data_priority)
        .with_ms_default_data_priority(options.ms_default_data_priority)
        .with_scheduling(options.data_scheduling)
        .resolve_unitdata()?;
    options.pdu_priority = resolved.pdu_priority;
    options.data_priority = resolved.data_priority;
    options.mle_data_priority_signalling_required = false;

    let pdu = if options.layer2service == Layer2Service::Acknowledged {
        sn_data_req_to_pdu(req)?
    } else {
        sn_unitdata_req_to_pdu(req)?
    };
    sndcp_pdu_to_ltpd_mle_unitdata_req(pdu, handle, options)
}

pub fn ltpd_unitdata_req_to_tla_unitdata_req_with_allocation(
    req: &LtpdMleUnitdataReq,
    tla_handle: MleHandle,
    subscriber_class: Todo,
    allocation: Option<&SndcpLowerChannelAllocation>,
) -> Result<TlaTlUnitdataReqBl, SndcpMleAdapterError> {
    validate_ltpd_packet_data_unitdata_req(req, tla_handle, allocation)?;

    let sdu_len = req.sdu.get_len();
    let mut tl_sdu = BitBuffer::new(3 + sdu_len);
    tl_sdu.write_bits(MleProtocolDiscriminator::Sndcp.into_raw(), 3);
    let mut sdu = BitBuffer::from_bitbuffer(&req.sdu);
    sdu.seek(0);
    tl_sdu.copy_bits(&mut sdu, sdu_len);
    tl_sdu.seek(0);

    Ok(TlaTlUnitdataReqBl {
        main_address: req.address,
        link_id: req.link_id,
        endpoint_id: req.endpoint_id,
        tl_sdu,
        pdu_prio: req.pdu_prio,
        stealing_permission: false,
        subscriber_class,
        fcs_flag: req.fcs_flag,
        air_interface_encryption: None,
        stealing_repeats_flag: Some(req.stealing_repeats_flag),
        packet_data_flag: true,
        n_tlsdu_repeats: todo_to_optional_u8(req.unacked_bl_repetitions),
        data_class_info: todo_to_optional_todo(req.data_class_info),
        req_handle: handle_to_todo(tla_handle)?,
        chan_alloc: allocation.map(|allocation| allocation.chan_alloc.clone()),
        tx_reporter: None,
    })
}

impl SndcpDeliveryTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn contains_handle(&self, handle: MleHandle) -> bool {
        self.pending.contains_key(&handle)
    }

    pub fn track_unitdata_request(&mut self, req: &SnUnitdataReq) -> Result<(), SndcpDeliveryTrackerError> {
        self.track_handle(req.handle)
    }

    pub fn track_handle(&mut self, handle: MleHandle) -> Result<(), SndcpDeliveryTrackerError> {
        validate_tracker_handle(handle)?;
        if self.pending.contains_key(&handle) {
            return Err(SndcpDeliveryTrackerError::DuplicateHandle(handle));
        }
        self.pending.insert(handle, SndcpTrackedDelivery { handle });
        Ok(())
    }

    pub fn handle_mle_report(&mut self, report: &LtpdMleReportInd) -> Result<SndcpDeliveryReportOutcome, SndcpDeliveryTrackerError> {
        let handle = todo_to_tracker_handle(report.handle)?;
        match report.transfer_result {
            TLA_REPORT_NO_SPECIFIC_REPORT | TLA_REPORT_FIRST_COMPLETE_TRANSMISSION => {
                if self.pending.contains_key(&handle) {
                    Ok(SndcpDeliveryReportOutcome::Progress)
                } else {
                    Err(SndcpDeliveryTrackerError::UnknownHandle(handle))
                }
            }
            TLA_REPORT_SUCCESSFUL_TRANSFER => {
                self.pending
                    .remove(&handle)
                    .ok_or(SndcpDeliveryTrackerError::UnknownHandle(handle))?;
                Ok(SndcpDeliveryReportOutcome::CompletedWithoutSnDelivery)
            }
            TLA_REPORT_FAILED_TRANSFER => {
                let tracked = self
                    .pending
                    .remove(&handle)
                    .ok_or(SndcpDeliveryTrackerError::UnknownHandle(handle))?;
                Ok(SndcpDeliveryReportOutcome::Delivered(SnDeliveryInd {
                    handle: tracked.handle,
                    status: SnDeliveryStatus::Failure,
                }))
            }
            other => Err(SndcpDeliveryTrackerError::UnsupportedTransferResult(other)),
        }
    }

    pub fn cancel(&mut self, handle: MleHandle) -> Result<SnDeliveryInd, SndcpDeliveryTrackerError> {
        validate_tracker_handle(handle)?;
        let tracked = self
            .pending
            .remove(&handle)
            .ok_or(SndcpDeliveryTrackerError::UnknownHandle(handle))?;
        Ok(SnDeliveryInd {
            handle: tracked.handle,
            status: SnDeliveryStatus::DeletedOrCancelledBySndcp,
        })
    }
}

fn validate_options(sdu: &BitBuffer, handle: MleHandle, options: SndcpLtpdUnitdataOptions) -> Result<(), SndcpMleAdapterError> {
    if sdu.get_len() == 0 {
        return Err(SndcpMleAdapterError::EmptySdu);
    }
    handle_to_todo(handle)?;
    if options.layer2service == Layer2Service::Todo {
        return Err(SndcpMleAdapterError::UnsupportedLayer2Service(options.layer2service));
    }
    if options.pdu_priority > 7 {
        return Err(SndcpMleAdapterError::PduPriorityOutOfRange(options.pdu_priority));
    }
    if let Some(data_priority) = options.data_priority {
        if data_priority > 7 {
            return Err(SndcpMleAdapterError::DataPriorityOutOfRange(data_priority));
        }
    }
    if options.mle_data_priority_signalling_required && options.data_priority.is_none() {
        return Err(SndcpMleAdapterError::MleDataPriorityFlagWithoutDefinedPriority);
    }
    optional_nonnegative_todo(options.data_class_info, SndcpMleAdapterError::NegativeDataClassInfo)?;
    optional_nonnegative_todo(options.scheduled_data_status, SndcpMleAdapterError::NegativeScheduledDataStatus)?;
    optional_nonnegative_todo(options.max_schedule_interval, SndcpMleAdapterError::NegativeMaxScheduleInterval)?;
    Ok(())
}

fn validate_ltpd_packet_data_unitdata_req(
    req: &LtpdMleUnitdataReq,
    tla_handle: MleHandle,
    allocation: Option<&SndcpLowerChannelAllocation>,
) -> Result<(), SndcpMleAdapterError> {
    validate_options(&req.sdu, tla_handle, ltpd_options_for_validation(req))?;
    if req.layer2service != Layer2Service::Unacknowledged {
        return Err(SndcpMleAdapterError::UnsupportedLayer2Service(req.layer2service));
    }
    if !req.packet_data_flag {
        return Err(SndcpMleAdapterError::PacketDataFlagRequired);
    }
    if req.stealing_permission {
        return Err(SndcpMleAdapterError::PacketDataStealingNotPermitted);
    }
    if req.address.ssi_type != SsiType::Issi {
        return Err(SndcpMleAdapterError::NonIssiPacketDataAddress(req.address));
    }
    if let Some(allocation) = allocation {
        validate_lower_channel_allocation(allocation)?;
        if allocation.issi != req.address.ssi {
            return Err(SndcpMleAdapterError::LowerAllocationIssiMismatch {
                address_issi: req.address.ssi,
                allocation_issi: allocation.issi,
            });
        }
    }
    Ok(())
}

fn ltpd_options_for_validation(req: &LtpdMleUnitdataReq) -> SndcpLtpdUnitdataOptions {
    SndcpLtpdUnitdataOptions {
        address: req.address,
        endpoint_id: req.endpoint_id,
        link_id: req.link_id,
        layer2service: req.layer2service,
        pdu_priority: u8::try_from(req.pdu_prio).unwrap_or(u8::MAX),
        data_priority: None,
        nsapi_data_priority: None,
        ms_default_data_priority: None,
        data_scheduling: SndcpDataScheduling::NonScheduled,
        mle_data_priority_signalling_required: false,
        unacked_bl_repetitions: todo_to_optional_u8(req.unacked_bl_repetitions),
        stealing_permission: req.stealing_permission,
        stealing_repeats_flag: req.stealing_repeats_flag,
        channel_advice_flag: req.channel_advice_flag,
        data_class_info: todo_to_optional_todo(req.data_class_info),
        scheduled_data_status: todo_to_optional_todo(req.scheduled_data_status),
        max_schedule_interval: todo_to_optional_todo(req.max_schedule_interval),
        fcs_flag: req.fcs_flag,
        packet_data_flag: req.packet_data_flag,
    }
}

fn handle_to_todo(handle: MleHandle) -> Result<Todo, SndcpMleAdapterError> {
    Todo::try_from(handle).map_err(|_| SndcpMleAdapterError::HandleOutOfRange(handle))
}

fn validate_tracker_handle(handle: MleHandle) -> Result<(), SndcpDeliveryTrackerError> {
    Todo::try_from(handle)
        .map(|_| ())
        .map_err(|_| SndcpDeliveryTrackerError::HandleOutOfRange(handle))
}

fn todo_to_tracker_handle(handle: Todo) -> Result<MleHandle, SndcpDeliveryTrackerError> {
    if handle >= 0 {
        Ok(handle as MleHandle)
    } else {
        Err(SndcpDeliveryTrackerError::NegativeHandle(handle))
    }
}

fn todo_to_optional_u8(value: Todo) -> Option<u8> {
    u8::try_from(value).ok()
}

fn todo_to_optional_todo(value: Todo) -> Option<Todo> {
    (value >= 0).then_some(value)
}

fn optional_u8_to_todo(value: Option<u8>) -> Todo {
    value.map(Todo::from).unwrap_or(-1)
}

fn optional_nonnegative_todo(value: Option<Todo>, error: fn(Todo) -> SndcpMleAdapterError) -> Result<Todo, SndcpMleAdapterError> {
    match value {
        Some(value) if value < 0 => Err(error(value)),
        Some(value) => Ok(value),
        None => Ok(-1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sndcp::ip::build_ipv4_udp_npdu;
    use crate::sndcp::pdch::{
        SndcpChannelAdviceRequest, SndcpLowerChannelAllocation, SndcpMacChannelAllocationPlacement, SndcpPacketDataAllocationDecision,
        SndcpPacketDataPlanInput, SndcpPacketDataResourceRequest, SndcpPdchAllocationPolicy, SndcpPdchError, SndcpPdchManager,
        packet_data_plan_to_lower_channel_allocation,
    };
    use crate::sndcp::pdp::{SndcpActivateAddressDemand, SndcpActivatePdpContextDemand, encode_activate_pdp_context_demand};
    use crate::sndcp::state::SwmiSndcpState;
    use crate::sndcp::unitdata::decode_sn_unitdata_pdu;
    use tetra_core::{SsiType, TetraAddress};
    use tetra_saps::lcmc::{
        enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment},
        fields::chan_alloc_req::CmceChanAllocReq,
    };
    use tetra_saps::sn::{SnPacketDataMsType, sn_unitdata_req};

    const ISSI: u32 = 2_260_618;
    const HANDLE: MleHandle = 77;

    fn address() -> TetraAddress {
        TetraAddress::new(ISSI, SsiType::Issi)
    }

    fn control_options() -> SndcpLtpdUnitdataOptions {
        SndcpLtpdUnitdataOptions::control_acknowledged(address(), 1, 0)
    }

    fn packet_data_options() -> SndcpLtpdUnitdataOptions {
        SndcpLtpdUnitdataOptions::packet_data_unacknowledged(address(), 1, 0, 4)
    }

    fn packet_ltpd_req() -> LtpdMleUnitdataReq {
        let req =
            sn_unitdata_req(2, HANDLE, BitBuffer::from_bytes(&[0x45, 0x00]), Some(3), Some(5)).expect("SN-UNITDATA request should build");
        sn_unitdata_req_to_ltpd_mle_unitdata_req(&req, HANDLE, packet_data_options()).expect("SN-UNITDATA request should map to LTPD")
    }

    fn lower_pdch_allocation(issi: u32) -> SndcpLowerChannelAllocation {
        SndcpLowerChannelAllocation {
            issi,
            allocation: SndcpPacketDataAllocationDecision::NewPdchAllocation,
            placement: SndcpMacChannelAllocationPlacement::MacResource,
            chan_alloc: CmceChanAllocReq {
                usage: None,
                carrier: None,
                timeslots: [false, true, true, true],
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
            },
        }
    }

    fn packet_plan_input() -> SndcpPacketDataPlanInput {
        SndcpPacketDataPlanInput {
            issi: ISSI,
            nsapi: 2,
            endpoint_id: 1,
            link_id: 0,
            swmi_state: SwmiSndcpState::Ready,
            packet_data_ms_type: SnPacketDataMsType::TypeAParallel,
            layer2service: Layer2Service::Unacknowledged,
            pdu_priority: 3,
            data_priority: Some(5),
            unacked_bl_repetitions: Some(0),
            scheduled_data_status: None,
            fcs_flag: false,
            current_channel_packet_data_suitable: false,
            allow_common_control_packet_data: false,
            pdch_available: true,
            channel_advice_request: SndcpChannelAdviceRequest::None,
            resource_request: SndcpPacketDataResourceRequest::None,
            downlink_sdu_bits: 19,
            nonfragmented_sdu_capacity_bits: Some(124),
            fragmented_channel_allocation_mac_end_supported: false,
            active_circuit_mode_service: false,
            parallel_voice_data_permitted: false,
        }
    }

    fn assert_mle_sndcp_prefix(tl_sdu: &BitBuffer, sndcp_sdu: &BitBuffer) {
        let mut tl_sdu_reader = BitBuffer::from_bitbuffer(tl_sdu);
        tl_sdu_reader.seek(0);
        assert_eq!(tl_sdu_reader.read_bits(3), Some(MleProtocolDiscriminator::Sndcp.into_raw()));

        let tl_bits = tl_sdu.to_bitstr();
        let sndcp_bits = sndcp_sdu.to_bitstr();
        assert_eq!(tl_sdu.get_len(), sndcp_sdu.get_len() + 3);
        assert_eq!(&tl_bits[3..], sndcp_bits.as_str());
    }

    #[test]
    fn control_sndcp_pdu_maps_to_acknowledged_ltpd_mle_unitdata_request() {
        let demand = encode_activate_pdp_context_demand(&SndcpActivatePdpContextDemand {
            sndcp_version: 1,
            nsapi: 2,
            address: SndcpActivateAddressDemand::Ipv4Dynamic,
            packet_data_ms_type: SnPacketDataMsType::TypeAParallel,
            pcomp_negotiation: 0,
        })
        .expect("activation demand should encode");

        let prim = sndcp_pdu_to_ltpd_mle_unitdata_req(demand.clone(), HANDLE, control_options())
            .expect("SNDCP control PDU should map to LTPD MLE-UNITDATA.req");

        assert_eq!(prim.sdu.to_bitstr(), demand.to_bitstr());
        assert_eq!(prim.handle, HANDLE as Todo);
        assert_eq!(prim.address, address());
        assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
        assert_eq!(prim.pdu_prio, SNDCP_CONTROL_PDU_PRIORITY as Todo);
        assert_eq!(prim.endpoint_id, 1);
        assert_eq!(prim.link_id, 0);
        assert_eq!(prim.unacked_bl_repetitions, -1);
        assert!(!prim.stealing_permission);
        assert!(!prim.stealing_repeats_flag);
        assert!(!prim.channel_advice_flag);
        assert_eq!(prim.data_class_info, -1);
        assert_eq!(prim.data_prio, -1);
        assert!(!prim.mle_data_prio_flag);
        assert!(!prim.packet_data_flag);
        assert_eq!(prim.scheduled_data_status, -1);
        assert_eq!(prim.max_schedule_interval, -1);
        assert!(!prim.fcs_flag);
    }

    #[test]
    fn sn_unitdata_request_maps_priority_and_data_priority_to_ltpd() {
        let npdu = build_ipv4_udp_npdu([10, 0, 0, 1], [10, 0, 0, 2], 9200, 49_152, b"wap", 1, 32).expect("IPv4/UDP N-PDU should build");
        let req = sn_unitdata_req(2, HANDLE, BitBuffer::from_bytes(&npdu), Some(3), Some(5)).expect("SN-UNITDATA request should build");

        let prim =
            sn_unitdata_req_to_ltpd_mle_unitdata_req(&req, HANDLE, packet_data_options()).expect("SN-UNITDATA request should map to LTPD");

        assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
        assert_eq!(prim.unacked_bl_repetitions, 0);
        assert_eq!(prim.pdu_prio, 3);
        assert_eq!(prim.data_prio, 5);
        assert!(!prim.mle_data_prio_flag);
        assert!(prim.packet_data_flag);

        let decoded = decode_sn_unitdata_pdu(&prim.sdu).expect("encoded SNDCP PDU should decode");
        assert_eq!(decoded.nsapi, 2);
        assert_eq!(decoded.n_pdu.to_bitstr(), BitBuffer::from_bytes(&npdu).to_bitstr());
    }

    #[test]
    fn sn_unitdata_request_uses_options_priority_when_primitive_omits_it() {
        let req = sn_unitdata_req(2, HANDLE, BitBuffer::from_bytes(&[0x45, 0x00]), None, None).expect("SN-UNITDATA request should build");

        let prim = sn_unitdata_req_to_ltpd_mle_unitdata_req(&req, HANDLE, packet_data_options().with_pdu_priority(6))
            .expect("missing SN priority should use context/options priority");

        assert_eq!(prim.pdu_prio, 6);
        assert_eq!(prim.data_prio, 2);
        assert!(!prim.mle_data_prio_flag);
    }

    #[test]
    fn sn_unitdata_request_caps_sn_priority_to_context_max() {
        let req =
            sn_unitdata_req(2, HANDLE, BitBuffer::from_bytes(&[0x45, 0x00]), Some(7), None).expect("SN-UNITDATA request should build");

        let prim = sn_unitdata_req_to_ltpd_mle_unitdata_req(&req, HANDLE, packet_data_options().with_pdu_priority(3))
            .expect("SN priority above context max should be capped");

        assert_eq!(prim.pdu_prio, 3);
        assert_eq!(prim.data_prio, 2);
        assert!(!prim.mle_data_prio_flag);
    }

    #[test]
    fn sn_unitdata_uses_nsapi_data_priority_before_ms_default() {
        let req = sn_unitdata_req(2, HANDLE, BitBuffer::from_bytes(&[0x45, 0x00]), None, None).expect("SN-UNITDATA request should build");

        let prim = sn_unitdata_req_to_ltpd_mle_unitdata_req(
            &req,
            HANDLE,
            packet_data_options()
                .with_nsapi_data_priority(Some(6))
                .with_ms_default_data_priority(Some(1)),
        )
        .expect("NSAPI data priority should resolve");

        assert_eq!(prim.data_prio, 6);
        assert!(!prim.mle_data_prio_flag);
    }

    #[test]
    fn sn_unitdata_uses_ms_default_data_priority_when_nsapi_is_undefined() {
        let req = sn_unitdata_req(2, HANDLE, BitBuffer::from_bytes(&[0x45, 0x00]), None, None).expect("SN-UNITDATA request should build");

        let prim = sn_unitdata_req_to_ltpd_mle_unitdata_req(&req, HANDLE, packet_data_options().with_ms_default_data_priority(Some(4)))
            .expect("MS default data priority should resolve");

        assert_eq!(prim.data_prio, 4);
        assert!(!prim.mle_data_prio_flag);
    }

    #[test]
    fn scheduled_sn_unitdata_data_priority_is_undefined() {
        let req =
            sn_unitdata_req(2, HANDLE, BitBuffer::from_bytes(&[0x45, 0x00]), None, Some(7)).expect("SN-UNITDATA request should build");

        let prim = sn_unitdata_req_to_ltpd_mle_unitdata_req(
            &req,
            HANDLE,
            packet_data_options().with_data_scheduling(SndcpDataScheduling::Scheduled),
        )
        .expect("scheduled SN-UNITDATA should map with undefined data priority");

        assert_eq!(prim.data_prio, -1);
        assert!(!prim.mle_data_prio_flag);
    }

    #[test]
    fn packet_data_ltpd_request_maps_to_tla_unitdata_with_pdch_allocation() {
        let req = packet_ltpd_req();
        let allocation = lower_pdch_allocation(ISSI);

        let tla = ltpd_unitdata_req_to_tla_unitdata_req_with_allocation(&req, 91, 6, Some(&allocation))
            .expect("packet-data LTPD should map to TLA with lower allocation");

        assert_eq!(tla.main_address, address());
        assert_eq!(tla.link_id, req.link_id);
        assert_eq!(tla.endpoint_id, req.endpoint_id);
        assert_eq!(tla.pdu_prio, req.pdu_prio);
        assert!(!tla.stealing_permission);
        assert_eq!(tla.subscriber_class, 6);
        assert_eq!(tla.fcs_flag, req.fcs_flag);
        assert_eq!(tla.air_interface_encryption, None);
        assert_eq!(tla.stealing_repeats_flag, Some(req.stealing_repeats_flag));
        assert!(tla.packet_data_flag);
        assert_eq!(tla.n_tlsdu_repeats, Some(0));
        assert_eq!(tla.data_class_info, None);
        assert_eq!(tla.req_handle, 91);
        assert!(tla.tx_reporter.is_none());

        let chan_alloc = tla.chan_alloc.expect("PDCH allocation should be passed to lower layers");
        assert_eq!(chan_alloc.usage, None);
        assert_eq!(chan_alloc.carrier, None);
        assert_eq!(chan_alloc.timeslots, [false, true, true, true]);
        assert_eq!(chan_alloc.alloc_type, ChanAllocType::Replace);
        assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Both);

        let mut tl_sdu = BitBuffer::from_bitbuffer(&tla.tl_sdu);
        tl_sdu.seek(0);
        assert_eq!(tl_sdu.read_bits(3), Some(MleProtocolDiscriminator::Sndcp.into_raw()));
        let tl_bits = tla.tl_sdu.to_bitstr();
        let req_bits = req.sdu.to_bitstr();
        assert_eq!(tla.tl_sdu.get_len(), req.sdu.get_len() + 3);
        assert_eq!(&tl_bits[3..], req_bits.as_str());
    }

    #[test]
    fn packet_data_plan_lower_allocation_maps_to_tla_unitdata_boundary() {
        let req = packet_ltpd_req();
        let manager = SndcpPdchManager::new();
        let plan = manager
            .plan_swmi_unitdata_channel(packet_plan_input())
            .expect("READY packet-data context should produce a PDCH plan");
        let allocation =
            packet_data_plan_to_lower_channel_allocation(&plan, SndcpPdchAllocationPolicy::timeslots([false, true, true, true], None))
                .expect("PDCH plan should validate lower allocation policy")
                .expect("new PDCH plan should carry lower allocation");

        let tla = ltpd_unitdata_req_to_tla_unitdata_req_with_allocation(&req, 93, 6, Some(&allocation))
            .expect("planned lower allocation should map to TLA");

        assert_eq!(tla.main_address, address());
        assert_eq!(tla.endpoint_id, req.endpoint_id);
        assert_eq!(tla.link_id, req.link_id);
        assert_eq!(tla.req_handle, 93);
        assert!(!tla.stealing_permission);
        assert!(tla.packet_data_flag);
        assert_mle_sndcp_prefix(&tla.tl_sdu, &req.sdu);

        let chan_alloc = tla.chan_alloc.expect("planned PDCH allocation should reach the pure TLA primitive");
        assert_eq!(chan_alloc.usage, None);
        assert_eq!(chan_alloc.carrier, None);
        assert_eq!(chan_alloc.timeslots, [false, true, true, true]);
        assert_eq!(chan_alloc.alloc_type, ChanAllocType::Replace);
        assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Both);
    }

    #[test]
    fn packet_data_ltpd_request_can_map_to_tla_without_new_allocation() {
        let req = packet_ltpd_req();

        let tla = ltpd_unitdata_req_to_tla_unitdata_req_with_allocation(&req, 92, 6, None)
            .expect("existing PDCH or common-control packet data may omit new lower allocation");

        assert!(tla.chan_alloc.is_none());
        assert!(tla.packet_data_flag);
        assert_eq!(tla.req_handle, 92);
    }

    #[test]
    fn packet_data_ltpd_to_tla_rejects_non_packet_or_stealing_paths() {
        let mut req = packet_ltpd_req();
        req.packet_data_flag = false;
        assert_eq!(
            ltpd_unitdata_req_to_tla_unitdata_req_with_allocation(&req, 91, 6, None).expect_err("packet-data flag must be explicit"),
            SndcpMleAdapterError::PacketDataFlagRequired
        );

        let mut req = packet_ltpd_req();
        req.stealing_permission = true;
        assert_eq!(
            ltpd_unitdata_req_to_tla_unitdata_req_with_allocation(&req, 91, 6, None)
                .expect_err("WAP/IP PDCH MVP must not steal FACCH/STCH resources"),
            SndcpMleAdapterError::PacketDataStealingNotPermitted
        );
    }

    #[test]
    fn packet_data_ltpd_to_tla_rejects_group_or_mismatched_allocation_address() {
        let mut req = packet_ltpd_req();
        req.address = TetraAddress::new(110, SsiType::Gssi);
        assert_eq!(
            ltpd_unitdata_req_to_tla_unitdata_req_with_allocation(&req, 91, 6, None)
                .expect_err("packet data must remain ISSI/PDP-context scoped"),
            SndcpMleAdapterError::NonIssiPacketDataAddress(TetraAddress::new(110, SsiType::Gssi))
        );

        let req = packet_ltpd_req();
        assert_eq!(
            ltpd_unitdata_req_to_tla_unitdata_req_with_allocation(&req, 91, 6, Some(&lower_pdch_allocation(ISSI + 1)))
                .expect_err("lower allocation ISSI must match the LTPD packet-data address"),
            SndcpMleAdapterError::LowerAllocationIssiMismatch {
                address_issi: ISSI,
                allocation_issi: ISSI + 1,
            }
        );
    }

    #[test]
    fn packet_data_ltpd_to_tla_rejects_invalid_lower_allocation() {
        let req = packet_ltpd_req();
        let mut allocation = lower_pdch_allocation(ISSI);
        allocation.chan_alloc.timeslots = [true, false, false, false];

        assert_eq!(
            ltpd_unitdata_req_to_tla_unitdata_req_with_allocation(&req, 91, 6, Some(&allocation))
                .expect_err("lower allocation must validate before handoff"),
            SndcpMleAdapterError::LowerAllocation(SndcpPdchError::McchTimeslotRequiresExplicitPolicy(1))
        );
    }

    #[test]
    fn adapter_rejects_unsafe_or_placeholder_fields() {
        let sdu = BitBuffer::from_bytes(&[0x40, 0x20]);

        assert_eq!(
            sndcp_pdu_to_ltpd_mle_unitdata_req(sdu.clone(), HANDLE, control_options().with_layer2service(Layer2Service::Todo))
                .expect_err("Layer2Service::Todo should reject"),
            SndcpMleAdapterError::UnsupportedLayer2Service(Layer2Service::Todo)
        );
        assert_eq!(
            sndcp_pdu_to_ltpd_mle_unitdata_req(sdu.clone(), HANDLE, control_options().with_pdu_priority(8))
                .expect_err("PDU priority outside 0..7 should reject"),
            SndcpMleAdapterError::PduPriorityOutOfRange(8)
        );
        assert_eq!(
            sndcp_pdu_to_ltpd_mle_unitdata_req(sdu.clone(), HANDLE, control_options().with_data_priority(Some(8)))
                .expect_err("data priority outside 0..7 should reject"),
            SndcpMleAdapterError::DataPriorityOutOfRange(8)
        );
        assert_eq!(
            sndcp_pdu_to_ltpd_mle_unitdata_req(
                sdu.clone(),
                HANDLE,
                control_options().with_mle_data_priority_signalling_required(true)
            )
            .expect_err("MLE data priority signalling requires a defined data priority"),
            SndcpMleAdapterError::MleDataPriorityFlagWithoutDefinedPriority
        );
        assert_eq!(
            sndcp_pdu_to_ltpd_mle_unitdata_req(BitBuffer::new(0), HANDLE, control_options()).expect_err("empty SDU should reject"),
            SndcpMleAdapterError::EmptySdu
        );
        assert_eq!(
            sndcp_pdu_to_ltpd_mle_unitdata_req(sdu, i32::MAX as u32 + 1, control_options()).expect_err("handle beyond Todo should reject"),
            SndcpMleAdapterError::HandleOutOfRange(i32::MAX as u32 + 1)
        );
    }

    #[test]
    fn adapter_rejects_negative_optional_todo_fields() {
        let sdu = BitBuffer::from_bytes(&[0x40, 0x20]);
        let mut bad = control_options();
        bad.data_class_info = Some(-2);
        assert_eq!(
            sndcp_pdu_to_ltpd_mle_unitdata_req(sdu.clone(), HANDLE, bad).expect_err("negative data class should reject"),
            SndcpMleAdapterError::NegativeDataClassInfo(-2)
        );

        let mut bad = control_options();
        bad.scheduled_data_status = Some(-2);
        assert_eq!(
            sndcp_pdu_to_ltpd_mle_unitdata_req(sdu.clone(), HANDLE, bad).expect_err("negative scheduled status should reject"),
            SndcpMleAdapterError::NegativeScheduledDataStatus(-2)
        );

        let mut bad = control_options();
        bad.max_schedule_interval = Some(-2);
        assert_eq!(
            sndcp_pdu_to_ltpd_mle_unitdata_req(sdu, HANDLE, bad).expect_err("negative max schedule should reject"),
            SndcpMleAdapterError::NegativeMaxScheduleInterval(-2)
        );
    }

    #[test]
    fn delivery_tracker_unitdata_success_removes_handle_without_sn_delivery() {
        let npdu = build_ipv4_udp_npdu([10, 0, 0, 1], [10, 0, 0, 2], 9200, 49_152, b"wap", 1, 32).expect("IPv4/UDP N-PDU should build");
        let req = sn_unitdata_req(2, HANDLE, BitBuffer::from_bytes(&npdu), Some(3), Some(5)).expect("SN-UNITDATA request should build");
        let mut tracker = SndcpDeliveryTracker::new();

        tracker.track_unitdata_request(&req).expect("SN request handle should track");
        assert!(tracker.contains_handle(HANDLE));

        let outcome = tracker
            .handle_mle_report(&LtpdMleReportInd {
                handle: HANDLE as Todo,
                transfer_result: TLA_REPORT_SUCCESSFUL_TRANSFER,
            })
            .expect("successful terminal MLE report should complete SN-UNITDATA tracking");

        assert_eq!(outcome, SndcpDeliveryReportOutcome::CompletedWithoutSnDelivery);
        assert!(tracker.is_empty());
        assert_eq!(
            tracker.handle_mle_report(&LtpdMleReportInd {
                handle: HANDLE as Todo,
                transfer_result: TLA_REPORT_SUCCESSFUL_TRANSFER,
            }),
            Err(SndcpDeliveryTrackerError::UnknownHandle(HANDLE))
        );
    }

    #[test]
    fn delivery_tracker_keeps_handle_for_progress_report_and_failure_delivers() {
        let mut tracker = SndcpDeliveryTracker::new();
        tracker.track_handle(HANDLE).expect("handle should track");

        assert_eq!(
            tracker.handle_mle_report(&LtpdMleReportInd {
                handle: HANDLE as Todo,
                transfer_result: TLA_REPORT_FIRST_COMPLETE_TRANSMISSION,
            }),
            Ok(SndcpDeliveryReportOutcome::Progress)
        );
        assert!(tracker.contains_handle(HANDLE));

        assert_eq!(
            tracker.handle_mle_report(&LtpdMleReportInd {
                handle: HANDLE as Todo,
                transfer_result: TLA_REPORT_FAILED_TRANSFER,
            }),
            Ok(SndcpDeliveryReportOutcome::Delivered(SnDeliveryInd {
                handle: HANDLE,
                status: SnDeliveryStatus::Failure
            }))
        );
        assert!(tracker.is_empty());
    }

    #[test]
    fn delivery_tracker_can_cancel_pending_unitdata() {
        let mut tracker = SndcpDeliveryTracker::new();
        tracker.track_handle(HANDLE).expect("handle should track");

        assert_eq!(
            tracker.cancel(HANDLE),
            Ok(SnDeliveryInd {
                handle: HANDLE,
                status: SnDeliveryStatus::DeletedOrCancelledBySndcp
            })
        );
        assert!(tracker.is_empty());
    }

    #[test]
    fn delivery_tracker_rejects_duplicate_unknown_and_invalid_handles() {
        let mut tracker = SndcpDeliveryTracker::new();
        tracker.track_handle(HANDLE).expect("handle should track");

        assert_eq!(
            tracker.track_handle(HANDLE),
            Err(SndcpDeliveryTrackerError::DuplicateHandle(HANDLE))
        );
        assert_eq!(
            tracker.cancel(HANDLE + 1),
            Err(SndcpDeliveryTrackerError::UnknownHandle(HANDLE + 1))
        );
        assert_eq!(
            tracker.handle_mle_report(&LtpdMleReportInd {
                handle: -1,
                transfer_result: TLA_REPORT_SUCCESSFUL_TRANSFER,
            }),
            Err(SndcpDeliveryTrackerError::NegativeHandle(-1))
        );
        assert_eq!(
            tracker.handle_mle_report(&LtpdMleReportInd {
                handle: HANDLE as Todo,
                transfer_result: 99,
            }),
            Err(SndcpDeliveryTrackerError::UnsupportedTransferResult(99))
        );
        assert!(tracker.contains_handle(HANDLE));
    }
}
