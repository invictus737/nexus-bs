// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original pure TETRA SNDCP LTPD WAP/IP pipeline primitive.

use super::mle_adapter::{
    SndcpLtpdUnitdataOptions, SndcpMleAdapterError, sn_unitdata_req_to_ltpd_mle_unitdata_req, sndcp_pdu_to_ltpd_mle_unitdata_req,
};
use super::pdch::{
    SndcpChannelAdviceRequest, SndcpEndOfDataPlanInput, SndcpPacketDataAllocationDecision, SndcpPacketDataPlanInput,
    SndcpPdchAllocationPolicy, SndcpPdchError, SndcpPdchManager, end_of_data_plan_to_lower_channel_allocation,
    packet_data_plan_to_lower_channel_allocation,
};
use super::state::SwmiSndcpState;
use super::transfer::SndcpDataTransmitRequest;
use super::wap_gateway::WapStatusUnitdataResponse;
use super::wap_session::{SndcpWapSession, SndcpWapSessionError, SndcpWapSessionResponse};
use super::wap_status::WapStatusSnapshot;
use tetra_core::{EndpointId, Layer2Service, LinkId, MleHandle, SsiType, TetraAddress};
use tetra_saps::lcmc::{enums::alloc_type::ChanAllocType, fields::chan_alloc_req::CmceChanAllocReq};
use tetra_saps::ltpd::{LtpdMleUnitdataInd, LtpdMleUnitdataReq};
use tetra_saps::sn::SnPacketDataMsType;

pub const SNDCP_MLE_HANDLE_MIN: MleHandle = 1;
pub const SNDCP_MLE_HANDLE_MAX: MleHandle = i32::MAX as MleHandle;
const WAP_IP_MVP_NONFRAGMENTED_MAC_CAPACITY_BITS: usize = 124;

#[derive(Debug, Clone)]
pub struct SndcpWapLtpdPipeline {
    session: SndcpWapSession,
    handles: SndcpWapLtpdHandleAllocator,
    pdch: SndcpPdchManager,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SndcpWapLtpdHandleAllocator {
    next: MleHandle,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SndcpWapLtpdPipelineError {
    NonIssiAddress(TetraAddress),
    Session(SndcpWapSessionError),
    Mle(SndcpMleAdapterError),
    Pdch(SndcpPdchError),
}

impl From<SndcpWapSessionError> for SndcpWapLtpdPipelineError {
    fn from(value: SndcpWapSessionError) -> Self {
        SndcpWapLtpdPipelineError::Session(value)
    }
}

impl From<SndcpMleAdapterError> for SndcpWapLtpdPipelineError {
    fn from(value: SndcpMleAdapterError) -> Self {
        SndcpWapLtpdPipelineError::Mle(value)
    }
}

impl From<SndcpPdchError> for SndcpWapLtpdPipelineError {
    fn from(value: SndcpPdchError) -> Self {
        SndcpWapLtpdPipelineError::Pdch(value)
    }
}

impl SndcpWapLtpdPipeline {
    pub fn new(session: SndcpWapSession) -> Self {
        Self {
            session,
            handles: SndcpWapLtpdHandleAllocator::default(),
            pdch: SndcpPdchManager::default(),
        }
    }

    pub fn with_handle_allocator(mut self, handles: SndcpWapLtpdHandleAllocator) -> Self {
        self.handles = handles;
        self
    }

    pub fn session(&self) -> &SndcpWapSession {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut SndcpWapSession {
        &mut self.session
    }

    pub fn pdch(&self) -> &SndcpPdchManager {
        &self.pdch
    }

    pub fn pdch_mut(&mut self) -> &mut SndcpPdchManager {
        &mut self.pdch
    }

    pub fn mark_pdch_ready(&mut self, issi: u32, endpoint_id: EndpointId, link_id: LinkId) {
        self.pdch.mark_pdch_ready(issi, endpoint_id, link_id);
    }

    pub fn deregister_issi(&mut self, issi: u32) -> Result<(), SndcpWapLtpdPipelineError> {
        self.session.deregister_issi(issi)?;
        self.pdch.deregister_issi(issi);
        Ok(())
    }

    pub fn attach_mvp_pdch_allocation_for_data_transmit_response(
        &mut self,
        req: &mut LtpdMleUnitdataReq,
        ind: &LtpdMleUnitdataInd,
        issi: u32,
        data_transmit: &SndcpDataTransmitRequest,
        active_circuit_mode_service: bool,
        parallel_voice_data_permitted: bool,
    ) -> Result<(), SndcpWapLtpdPipelineError> {
        let packet_data_ms_type = self
            .session
            .bearer()
            .pdp()
            .contexts()
            .get_issi_nsapi(issi, data_transmit.nsapi)
            .ok()
            .flatten()
            .map(|context| context.packet_data_ms_type)
            .unwrap_or(SnPacketDataMsType::TypeAParallel);
        let plan = self.pdch.plan_swmi_unitdata_channel(SndcpPacketDataPlanInput {
            issi,
            nsapi: data_transmit.nsapi,
            endpoint_id: ind.endpoint_id,
            link_id: ind.link_id,
            swmi_state: self.session.state_for_issi(issi),
            packet_data_ms_type,
            layer2service: req.layer2service,
            pdu_priority: req.pdu_prio as u8,
            data_priority: None,
            unacked_bl_repetitions: None,
            scheduled_data_status: None,
            fcs_flag: req.fcs_flag,
            current_channel_packet_data_suitable: false,
            allow_common_control_packet_data: false,
            pdch_available: true,
            channel_advice_request: SndcpChannelAdviceRequest::None,
            resource_request: data_transmit.resource_request,
            downlink_sdu_bits: req.sdu.get_len(),
            nonfragmented_sdu_capacity_bits: Some(WAP_IP_MVP_NONFRAGMENTED_MAC_CAPACITY_BITS),
            fragmented_channel_allocation_mac_end_supported: false,
            active_circuit_mode_service,
            parallel_voice_data_permitted,
        })?;
        let pdch_policy = SndcpPdchAllocationPolicy::assigned_scch_for_resource_request(data_transmit.resource_request);
        if let Some(allocation) = packet_data_plan_to_lower_channel_allocation(&plan, pdch_policy)? {
            tracing::info!(
                "SNDCP/WAP-IP: attaching MVP PDCH allocation issi={} nsapi={} endpoint={} link={} resource_request={:?} chan_alloc={:?}",
                issi,
                data_transmit.nsapi,
                ind.endpoint_id,
                ind.link_id,
                data_transmit.resource_request,
                allocation.chan_alloc
            );
            req.chan_alloc = Some(allocation.chan_alloc);
        } else if plan.allocation == SndcpPacketDataAllocationDecision::ExistingPdch {
            let chan_alloc = CmceChanAllocReq {
                usage: pdch_policy.usage_marker,
                carrier: pdch_policy.carrier,
                timeslots: pdch_policy.timeslots,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: pdch_policy.ul_dl_assignment,
            };
            tracing::info!(
                "SNDCP/WAP-IP: refreshing existing MVP PDCH allocation issi={} nsapi={} endpoint={} link={} resource_request={:?} chan_alloc={:?}",
                issi,
                data_transmit.nsapi,
                ind.endpoint_id,
                ind.link_id,
                data_transmit.resource_request,
                chan_alloc
            );
            req.chan_alloc = Some(chan_alloc);
        }
        Ok(())
    }

    pub fn attach_common_control_allocation_for_end_of_data_response(
        &mut self,
        req: &mut LtpdMleUnitdataReq,
        ind: &LtpdMleUnitdataInd,
        issi: u32,
    ) -> Result<bool, SndcpWapLtpdPipelineError> {
        if self.pdch.ensure_packet_data_ready(issi, ind.endpoint_id, ind.link_id).is_err() {
            return Ok(false);
        }

        let plan = self.pdch.plan_swmi_end_of_data_channel(SndcpEndOfDataPlanInput {
            issi,
            endpoint_id: ind.endpoint_id,
            link_id: ind.link_id,
            swmi_state: SwmiSndcpState::Ready,
            layer2service: req.layer2service,
            pdu_priority: req.pdu_prio as u8,
            downlink_sdu_bits: req.sdu.get_len(),
            nonfragmented_sdu_capacity_bits: Some(WAP_IP_MVP_NONFRAGMENTED_MAC_CAPACITY_BITS),
            fragmented_channel_allocation_mac_end_supported: false,
        })?;
        let allocation = end_of_data_plan_to_lower_channel_allocation(&plan)?;
        tracing::info!(
            "SNDCP/WAP-IP: attaching SN-END OF DATA common-control allocation issi={} endpoint={} link={} chan_alloc={:?}",
            issi,
            ind.endpoint_id,
            ind.link_id,
            allocation.chan_alloc
        );
        req.chan_alloc = Some(allocation.chan_alloc);
        self.pdch.mark_return_to_common_control_transmitted(issi)?;
        Ok(true)
    }

    pub fn handle_ltpd_mle_unitdata_ind_allocating(
        &mut self,
        ind: &LtpdMleUnitdataInd,
        snapshot: &WapStatusSnapshot,
    ) -> Result<LtpdMleUnitdataReq, SndcpWapLtpdPipelineError> {
        require_ltpd_response(self.handle_ltpd_mle_unitdata_ind_allocating_optional(ind, snapshot)?)
    }

    pub fn handle_ltpd_mle_unitdata_ind_allocating_optional(
        &mut self,
        ind: &LtpdMleUnitdataInd,
        snapshot: &WapStatusSnapshot,
    ) -> Result<Option<LtpdMleUnitdataReq>, SndcpWapLtpdPipelineError> {
        let issi = issi_from_ltpd_ind(ind)?;
        self.pdch.observe_ltpd_unitdata_ind(issi, ind)?;
        let handle = self.handles.allocate();
        self.handle_validated_ltpd_mle_unitdata_ind_optional(ind, issi, handle, snapshot)
    }

    pub fn handle_ltpd_mle_unitdata_ind(
        &mut self,
        ind: &LtpdMleUnitdataInd,
        handle: MleHandle,
        snapshot: &WapStatusSnapshot,
    ) -> Result<LtpdMleUnitdataReq, SndcpWapLtpdPipelineError> {
        let issi = issi_from_ltpd_ind(ind)?;
        self.pdch.observe_ltpd_unitdata_ind(issi, ind)?;
        require_ltpd_response(self.handle_validated_ltpd_mle_unitdata_ind_optional(ind, issi, handle, snapshot)?)
    }

    fn handle_validated_ltpd_mle_unitdata_ind_optional(
        &mut self,
        ind: &LtpdMleUnitdataInd,
        issi: u32,
        handle: MleHandle,
        snapshot: &WapStatusSnapshot,
    ) -> Result<Option<LtpdMleUnitdataReq>, SndcpWapLtpdPipelineError> {
        let response = self.session.handle_inbound_pdu_response(issi, handle, &ind.sdu, snapshot)?;

        match response {
            SndcpWapSessionResponse::Control(pdu) => Ok(Some(sndcp_pdu_to_ltpd_mle_unitdata_req(
                pdu,
                handle,
                control_response_options_from_ind(ind),
            )?)),
            SndcpWapSessionResponse::Unitdata(response) => {
                self.pdch.ensure_packet_data_ready(issi, ind.endpoint_id, ind.link_id)?;
                Ok(Some(sn_unitdata_req_to_ltpd_mle_unitdata_req(
                    &response.unitdata,
                    handle,
                    packet_data_response_options_from_ind(ind, &response),
                )?))
            }
            SndcpWapSessionResponse::NoResponse => Ok(None),
        }
    }
}

impl SndcpWapLtpdHandleAllocator {
    pub fn new(first_handle: MleHandle) -> Self {
        Self {
            next: normalize_handle(first_handle),
        }
    }

    pub fn next_handle(&self) -> MleHandle {
        self.next
    }

    pub fn allocate(&mut self) -> MleHandle {
        let handle = self.next;
        self.next = next_handle_after(handle);
        handle
    }
}

impl Default for SndcpWapLtpdHandleAllocator {
    fn default() -> Self {
        Self::new(SNDCP_MLE_HANDLE_MIN)
    }
}

pub fn issi_from_ltpd_ind(ind: &LtpdMleUnitdataInd) -> Result<u32, SndcpWapLtpdPipelineError> {
    match ind.received_tetra_address.ssi_type {
        SsiType::Issi => Ok(ind.received_tetra_address.ssi),
        _ => Err(SndcpWapLtpdPipelineError::NonIssiAddress(ind.received_tetra_address)),
    }
}

fn control_response_options_from_ind(ind: &LtpdMleUnitdataInd) -> SndcpLtpdUnitdataOptions {
    SndcpLtpdUnitdataOptions::control_acknowledged(ind.received_tetra_address, ind.endpoint_id, ind.link_id)
}

fn packet_data_response_options_from_ind(ind: &LtpdMleUnitdataInd, response: &WapStatusUnitdataResponse) -> SndcpLtpdUnitdataOptions {
    let bearer = response.bearer_profile.resolve_swmi_unitdata_downlink();
    let options = if ind.link_id != 0 {
        SndcpLtpdUnitdataOptions::packet_data_acknowledged(
            ind.received_tetra_address,
            ind.endpoint_id,
            ind.link_id,
            response.pdu_priority_max,
        )
    } else {
        debug_assert_eq!(bearer.layer2service, Layer2Service::Unacknowledged);
        SndcpLtpdUnitdataOptions::packet_data_unacknowledged(
            ind.received_tetra_address,
            ind.endpoint_id,
            ind.link_id,
            response.pdu_priority_max,
        )
        .with_unacked_bl_repetitions(bearer.unacked_bl_repetitions)
    };
    options
        .with_nsapi_data_priority(response.nsapi_data_priority)
        .with_ms_default_data_priority(response.ms_default_data_priority)
        .with_data_scheduling(response.scheduling)
        .with_fcs(bearer.fcs_flag)
}

fn require_ltpd_response(response: Option<LtpdMleUnitdataReq>) -> Result<LtpdMleUnitdataReq, SndcpWapLtpdPipelineError> {
    response.ok_or(SndcpWapLtpdPipelineError::Session(SndcpWapSessionError::MissingControlPdu(
        "no_response",
    )))
}

fn normalize_handle(handle: MleHandle) -> MleHandle {
    if (SNDCP_MLE_HANDLE_MIN..=SNDCP_MLE_HANDLE_MAX).contains(&handle) {
        handle
    } else {
        SNDCP_MLE_HANDLE_MIN
    }
}

fn next_handle_after(handle: MleHandle) -> MleHandle {
    if handle >= SNDCP_MLE_HANDLE_MAX {
        SNDCP_MLE_HANDLE_MIN
    } else {
        handle + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sndcp::bearer::SndcpBearerError;
    use crate::sndcp::bearer_policy::SndcpPacketDataBearerProfile;
    use crate::sndcp::ip::{bitbuffer_npdu_octets, build_ipv4_udp_npdu, parse_ipv4_packet, parse_udp_datagram};
    use crate::sndcp::mle_adapter::SNDCP_CONTROL_PDU_PRIORITY;
    use crate::sndcp::pdch::{
        SNDCP_BASIC_LINK_ID, SndcpPacketDataResourceRequest, SndcpPdchError, SndcpPdchState, SndcpPhaseModulationResourceRequest,
    };
    use crate::sndcp::pdp::{
        SndcpActivateAddressDemand, SndcpActivatePdpContextDemand, SndcpActivationRejectCause, SndcpDeactivation,
        decode_activate_pdp_context_accept, decode_activate_pdp_context_reject, decode_deactivate_pdp_context_accept,
        encode_activate_pdp_context_demand, encode_deactivate_pdp_context_demand,
    };
    use crate::sndcp::pdp_service::SndcpPdpPolicy;
    use crate::sndcp::state::SwmiSndcpState;
    use crate::sndcp::transfer::{
        SndcpDataTransmitRequest, SndcpDataTransmitResponseResult, SndcpEndOfData, SndcpReconnect, decode_data_transmit_response,
        encode_data_transmit_request, encode_end_of_data, encode_reconnect,
    };
    use crate::sndcp::unitdata::{decode_sn_user_data_pdu, encode_sn_unitdata};
    use crate::sndcp::wap_gateway::WapGatewayError;
    use crate::sndcp::wap_ip::{WapIpEndpoint, WapIpServicePolicy};
    use tetra_core::{BitBuffer, Layer2Service, LinkId, Todo};
    use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
    use tetra_saps::sn::{SnAddress, SnPacketDataMsType};

    const ISSI: u32 = 2_260_618;
    const GSSI: u32 = 91;
    const HANDLE: MleHandle = 99;

    fn endpoint() -> WapIpEndpoint {
        WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        }
    }

    fn wap_policy() -> WapIpServicePolicy {
        WapIpServicePolicy::experimental_status()
    }

    fn snapshot() -> WapStatusSnapshot {
        WapStatusSnapshot {
            title: "Nexus-BS".to_string(),
            stack_version: "v0.1.69_dev-test".to_string(),
            service_state: "ON AIR".to_string(),
            registered_ms: 2,
            active_calls: 1,
            queued_sds: 0,
            uptime_secs: 125,
            last_activity: None,
            health_summary: Some("OK".to_string()),
            health_lines: vec!["CORE OK".to_string(), "RF OK".to_string(), "SDS OK".to_string()],
            radio_lines: vec!["MS 2260618 -47dB G1 SA".to_string()],
            call_lines: vec!["G91 S2260618 TS2".to_string()],
        }
    }

    fn dynamic_ipv4_demand(nsapi: u8) -> BitBuffer {
        dynamic_ipv4_demand_with_ms_type(nsapi, SnPacketDataMsType::TypeAParallel)
    }

    fn dynamic_ipv4_demand_with_ms_type(nsapi: u8, packet_data_ms_type: SnPacketDataMsType) -> BitBuffer {
        encode_activate_pdp_context_demand(&SndcpActivatePdpContextDemand {
            sndcp_version: 1,
            nsapi,
            address: SndcpActivateAddressDemand::Ipv4Dynamic,
            packet_data_ms_type,
            pcomp_negotiation: 0,
        })
        .expect("activation demand should encode")
    }

    fn ltpd_ind(address: TetraAddress, sdu: BitBuffer) -> LtpdMleUnitdataInd {
        ltpd_ind_on_link(address, sdu, 7)
    }

    fn ltpd_ind_on_link(address: TetraAddress, sdu: BitBuffer, link_id: LinkId) -> LtpdMleUnitdataInd {
        LtpdMleUnitdataInd {
            sdu,
            endpoint_id: 3,
            link_id,
            received_tetra_address: address,
            chan_change_resp_req: false,
            chan_change_handle: None,
        }
    }

    fn pipeline() -> SndcpWapLtpdPipeline {
        pipeline_with_policy(SndcpPdpPolicy::experimental_wap_ipv4())
    }

    fn pipeline_with_policy(policy: SndcpPdpPolicy) -> SndcpWapLtpdPipeline {
        SndcpWapLtpdPipeline::new(SndcpWapSession::new(policy, endpoint(), wap_policy()))
    }

    fn data_transmit_request(nsapi: u8) -> BitBuffer {
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi,
            logical_link_status: false,
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode")
    }

    fn single_slot_phase_modulation_resource_request() -> SndcpPacketDataResourceRequest {
        SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: 1,
            downlink_timeslots: 1,
            full_phase_modulation_capability_timeslots: 1,
            unspecified_phase_modulation_resource: false,
        })
    }

    fn unspecified_four_slot_phase_modulation_resource_request() -> SndcpPacketDataResourceRequest {
        SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: 4,
            downlink_timeslots: 4,
            full_phase_modulation_capability_timeslots: 4,
            unspecified_phase_modulation_resource: true,
        })
    }

    fn specific_four_slot_phase_modulation_resource_request() -> SndcpPacketDataResourceRequest {
        SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: 4,
            downlink_timeslots: 4,
            full_phase_modulation_capability_timeslots: 4,
            unspecified_phase_modulation_resource: false,
        })
    }

    fn data_transmit_request_with_resource(nsapi: u8, resource_request: SndcpPacketDataResourceRequest) -> BitBuffer {
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi,
            logical_link_status: false,
            resource_request,
        })
        .expect("SN-DATA TRANSMIT REQUEST with resource request should encode")
    }

    fn reconnect_with_resource(nsapi: u8, resource_request: SndcpPacketDataResourceRequest) -> BitBuffer {
        encode_reconnect(&SndcpReconnect {
            nsapi: Some(nsapi),
            resource_request,
        })
        .expect("SN-RECONNECT with resource request should encode")
    }

    fn enter_ready(pipeline: &mut SndcpWapLtpdPipeline, address: TetraAddress) -> LtpdMleUnitdataReq {
        let req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, data_transmit_request(2)), HANDLE, &snapshot())
            .expect("SN-DATA TRANSMIT REQUEST should produce response");
        let response = decode_data_transmit_response(&req.sdu).expect("SN-DATA TRANSMIT RESPONSE should decode");
        assert_eq!(response.nsapi, 2);
        assert_eq!(response.result, SndcpDataTransmitResponseResult::Accepted);
        assert_eq!(req.layer2service, Layer2Service::Acknowledged);
        assert!(!req.packet_data_flag);
        assert_eq!(pipeline.session().state_for_issi(ISSI), SwmiSndcpState::Ready);
        req
    }

    fn data_transmit() -> SndcpDataTransmitRequest {
        SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
            resource_request: SndcpPacketDataResourceRequest::None,
        }
    }

    fn data_transmit_with_resource(resource_request: SndcpPacketDataResourceRequest) -> SndcpDataTransmitRequest {
        SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
            resource_request,
        }
    }

    #[test]
    fn handle_allocator_stays_inside_ltpd_todo_range() {
        let mut allocator = SndcpWapLtpdHandleAllocator::new(0);
        assert_eq!(allocator.allocate(), SNDCP_MLE_HANDLE_MIN);
        assert_eq!(allocator.allocate(), SNDCP_MLE_HANDLE_MIN + 1);

        let mut allocator = SndcpWapLtpdHandleAllocator::new(SNDCP_MLE_HANDLE_MAX);
        assert_eq!(allocator.allocate(), SNDCP_MLE_HANDLE_MAX);
        assert_eq!(allocator.allocate(), SNDCP_MLE_HANDLE_MIN);

        let mut allocator = SndcpWapLtpdHandleAllocator::new(SNDCP_MLE_HANDLE_MAX + 1);
        assert_eq!(allocator.next_handle(), SNDCP_MLE_HANDLE_MIN);
        assert_eq!(allocator.allocate(), SNDCP_MLE_HANDLE_MIN);
    }

    #[test]
    fn allocating_pipeline_assigns_fresh_outbound_handles() {
        let mut pipeline = pipeline().with_handle_allocator(SndcpWapLtpdHandleAllocator::new(41));
        let address = TetraAddress::issi(ISSI);

        let activation = pipeline
            .handle_ltpd_mle_unitdata_ind_allocating(&ltpd_ind(address, dynamic_ipv4_demand(2)), &snapshot())
            .expect("activation should produce accept");
        assert_eq!(activation.handle, 41);
        let ready = pipeline
            .handle_ltpd_mle_unitdata_ind_allocating(&ltpd_ind(address, data_transmit_request(2)), &snapshot())
            .expect("SN-DATA TRANSMIT REQUEST should produce accept response");
        assert_eq!(ready.handle, 42);
        pipeline.mark_pdch_ready(ISSI, 3, 7);

        let request_npdu = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        let unitdata = encode_sn_unitdata(2, 0, 0, &BitBuffer::from_bytes(&request_npdu)).expect("SN-UNITDATA should encode");

        let response = pipeline
            .handle_ltpd_mle_unitdata_ind_allocating(&ltpd_ind(address, unitdata), &snapshot())
            .expect("WAP request should produce response");
        assert_eq!(response.handle, 43);
        assert!(response.packet_data_flag);
    }

    #[test]
    fn local_rejections_do_not_consume_allocated_handles() {
        let mut pipeline = pipeline().with_handle_allocator(SndcpWapLtpdHandleAllocator::new(55));

        let gssi_ind = ltpd_ind(TetraAddress::new(GSSI, SsiType::Gssi), dynamic_ipv4_demand(2));
        assert!(matches!(
            pipeline.handle_ltpd_mle_unitdata_ind_allocating(&gssi_ind, &snapshot()),
            Err(SndcpWapLtpdPipelineError::NonIssiAddress(_))
        ));

        let mut channel_change_ind = ltpd_ind(TetraAddress::issi(ISSI), dynamic_ipv4_demand(2));
        channel_change_ind.chan_change_resp_req = true;
        assert_eq!(
            pipeline
                .handle_ltpd_mle_unitdata_ind_allocating(&channel_change_ind, &snapshot())
                .expect_err("channel-change response is not implemented"),
            SndcpWapLtpdPipelineError::Pdch(SndcpPdchError::MissingChannelChangeHandle { issi: ISSI })
        );

        let activation = pipeline
            .handle_ltpd_mle_unitdata_ind_allocating(&ltpd_ind(TetraAddress::issi(ISSI), dynamic_ipv4_demand(2)), &snapshot())
            .expect("next valid response should use first configured handle");
        assert_eq!(activation.handle, 55);
    }

    #[test]
    fn activation_demand_maps_to_ltpd_unitdata_request() {
        let mut pipeline = pipeline();
        let ind = ltpd_ind(TetraAddress::issi(ISSI), dynamic_ipv4_demand(2));

        let req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ind, HANDLE, &snapshot())
            .expect("activation should produce MLE-UNITDATA request");
        let accept = decode_activate_pdp_context_accept(&req.sdu).expect("activation accept should decode");

        assert_eq!(accept.assigned_address, Some(SnAddress::Ipv4([10, 0, 0, 2])));
        assert_eq!(req.handle, HANDLE as Todo);
        assert_eq!(req.address, TetraAddress::issi(ISSI));
        assert_eq!(req.endpoint_id, 3);
        assert_eq!(req.link_id, 7);
        assert_eq!(req.layer2service, Layer2Service::Acknowledged);
        assert_eq!(req.pdu_prio, SNDCP_CONTROL_PDU_PRIORITY as Todo);
        assert_eq!(req.unacked_bl_repetitions, -1);
        assert!(!req.packet_data_flag);
        assert!(!req.fcs_flag);
    }

    #[test]
    fn activation_demand_accepts_type_b_and_type_c_mxp600_profiles() {
        for packet_data_ms_type in [SnPacketDataMsType::TypeBAlternating, SnPacketDataMsType::TypeCIpSingleMode] {
            let mut pipeline = pipeline();
            let ind = ltpd_ind(TetraAddress::issi(ISSI), dynamic_ipv4_demand_with_ms_type(2, packet_data_ms_type));

            let req = pipeline
                .handle_ltpd_mle_unitdata_ind(&ind, HANDLE, &snapshot())
                .expect("activation should produce MLE-UNITDATA request");
            let accept = decode_activate_pdp_context_accept(&req.sdu).expect("activation accept should decode");

            assert_eq!(accept.nsapi, 2);
            assert_eq!(accept.assigned_address, Some(SnAddress::Ipv4([10, 0, 0, 2])));
            assert_eq!(
                pipeline
                    .session()
                    .pdp()
                    .contexts()
                    .get_issi_nsapi(ISSI, 2)
                    .unwrap()
                    .map(|context| context.packet_data_ms_type),
                Some(packet_data_ms_type)
            );
        }
    }

    #[test]
    fn dynamic_activation_after_recovered_context_refreshes_mxp600_ms_type() {
        let mut pipeline = pipeline();
        let address = TetraAddress::issi(ISSI);
        let recovered_ready = pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, data_transmit_request(1)), HANDLE, &snapshot())
            .expect("missing WAP/IP context should be recovered from MS data-transfer request");
        let recovered_response = decode_data_transmit_response(&recovered_ready.sdu).expect("SN-DATA TRANSMIT RESPONSE should decode");
        assert_eq!(recovered_response.nsapi, 1);
        assert_eq!(recovered_response.result, SndcpDataTransmitResponseResult::Accepted);
        assert_eq!(
            pipeline
                .session()
                .pdp()
                .contexts()
                .get_issi_nsapi(ISSI, 1)
                .unwrap()
                .map(|context| context.packet_data_ms_type),
            Some(SnPacketDataMsType::TypeAParallel)
        );

        pipeline
            .handle_ltpd_mle_unitdata_ind(
                &ltpd_ind(
                    address,
                    encode_end_of_data(&SndcpEndOfData {
                        immediate_service_change: false,
                    })
                    .expect("SN-END OF DATA should encode"),
                ),
                HANDLE,
                &snapshot(),
            )
            .expect("SN-END OF DATA should return recovered context to STANDBY");

        let reactivation = pipeline
            .handle_ltpd_mle_unitdata_ind(
                &ltpd_ind(address, dynamic_ipv4_demand_with_ms_type(1, SnPacketDataMsType::TypeBAlternating)),
                HANDLE,
                &snapshot(),
            )
            .expect("MXP600 Type B reactivation should refresh the recovered context");
        let accept = decode_activate_pdp_context_accept(&reactivation.sdu).expect("reactivation should decode as accept, not reject");
        assert_eq!(accept.nsapi, 1);
        assert_eq!(accept.assigned_address, Some(SnAddress::Ipv4([10, 0, 0, 2])));
        assert_eq!(
            pipeline
                .session()
                .pdp()
                .contexts()
                .get_issi_nsapi(ISSI, 1)
                .unwrap()
                .map(|context| context.packet_data_ms_type),
            Some(SnPacketDataMsType::TypeBAlternating)
        );
    }

    #[test]
    fn activation_reject_uses_acknowledged_control_link() {
        let mut pipeline = pipeline();
        let demand = encode_activate_pdp_context_demand(&SndcpActivatePdpContextDemand {
            sndcp_version: 1,
            nsapi: 2,
            address: SndcpActivateAddressDemand::Ipv6,
            packet_data_ms_type: SnPacketDataMsType::TypeAParallel,
            pcomp_negotiation: 0,
        })
        .expect("IPv6 demand should encode");

        let req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(TetraAddress::issi(ISSI), demand), HANDLE, &snapshot())
            .expect("unsupported activation should produce reject");
        let reject = decode_activate_pdp_context_reject(&req.sdu).expect("activation reject should decode");

        assert_eq!(reject.cause, SndcpActivationRejectCause::Ipv6NotSupported);
        assert_eq!(req.layer2service, Layer2Service::Acknowledged);
        assert_eq!(req.unacked_bl_repetitions, -1);
        assert!(!req.packet_data_flag);
        assert_eq!(pipeline.session().pdp().contexts().len(), 0);
    }

    #[test]
    fn active_wap_unitdata_maps_to_ltpd_response_with_context_priority() {
        let mut pipeline = pipeline();
        let address = TetraAddress::issi(ISSI);
        pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, dynamic_ipv4_demand(2)), HANDLE, &snapshot())
            .expect("activation should produce accept");
        enter_ready(&mut pipeline, address);
        pipeline.mark_pdch_ready(ISSI, 3, 7);

        let request_npdu = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        let unitdata = encode_sn_unitdata(2, 0, 0, &BitBuffer::from_bytes(&request_npdu)).expect("SN-UNITDATA should encode");

        let req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, unitdata), HANDLE, &snapshot())
            .expect("WAP request should produce MLE-UNITDATA response");
        let response_unitdata = decode_sn_user_data_pdu(&req.sdu).expect("response SN user data should decode");
        let response_npdu = bitbuffer_npdu_octets(&response_unitdata.n_pdu).expect("response N-PDU should be octet aligned");
        let response_ip = parse_ipv4_packet(&response_npdu).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");

        assert_eq!(req.address, address);
        assert_eq!(req.endpoint_id, 3);
        assert_eq!(req.link_id, 7);
        assert_eq!(req.layer2service, Layer2Service::Acknowledged);
        assert_eq!(req.unacked_bl_repetitions, -1);
        assert_eq!(req.pdu_prio, 4);
        assert_eq!(req.data_prio, 2);
        assert!(!req.mle_data_prio_flag);
        assert!(req.packet_data_flag);
        assert_eq!(response_ip.source, endpoint().address);
        assert_eq!(response_ip.destination, [10, 0, 0, 2]);
        assert_eq!(response_udp.source_port, endpoint().port);
        assert_eq!(response_udp.destination_port, 49_152);
        assert!(std::str::from_utf8(response_udp.payload).unwrap().contains("Nexus-BS"));
    }

    #[test]
    fn accepted_data_transmit_response_gets_mvp_pdch_allocation() {
        let mut pipeline = pipeline();
        let address = TetraAddress::issi(ISSI);
        pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, dynamic_ipv4_demand(2)), HANDLE, &snapshot())
            .expect("activation should produce accept");
        let ind = ltpd_ind(address, data_transmit_request(2));
        let mut req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ind, HANDLE, &snapshot())
            .expect("SN-DATA TRANSMIT REQUEST should produce response");

        pipeline
            .attach_mvp_pdch_allocation_for_data_transmit_response(&mut req, &ind, ISSI, &data_transmit(), false, false)
            .expect("MVP PDCH allocation should attach without active circuit-mode service");

        let allocation = req.chan_alloc.expect("PDCH allocation should be present");
        assert_eq!(allocation.usage, None);
        assert_eq!(allocation.carrier, None);
        assert_eq!(allocation.timeslots, [false, false, false, true]);
        assert_eq!(allocation.alloc_type, tetra_saps::lcmc::enums::alloc_type::ChanAllocType::Replace);
        assert_eq!(
            allocation.ul_dl_assigned,
            tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment::Both
        );
    }

    #[test]
    fn mxp600_type_b_single_slot_resource_request_gets_mvp_pdch_allocation() {
        let mut pipeline = pipeline();
        let address = TetraAddress::issi(ISSI);
        pipeline
            .handle_ltpd_mle_unitdata_ind(
                &ltpd_ind(address, dynamic_ipv4_demand_with_ms_type(2, SnPacketDataMsType::TypeBAlternating)),
                HANDLE,
                &snapshot(),
            )
            .expect("Type B activation should produce accept");

        let resource_request = single_slot_phase_modulation_resource_request();
        let request_pdu = data_transmit_request_with_resource(2, resource_request);
        assert_eq!(request_pdu.get_len(), 21);
        let ind = ltpd_ind(address, request_pdu);
        let mut req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ind, HANDLE, &snapshot())
            .expect("SN-DATA TRANSMIT REQUEST with single-slot resource should produce response");
        let response = decode_data_transmit_response(&req.sdu).expect("SN-DATA TRANSMIT RESPONSE should decode");
        assert_eq!(response.nsapi, 2);
        assert_eq!(response.result, SndcpDataTransmitResponseResult::Accepted);

        pipeline
            .attach_mvp_pdch_allocation_for_data_transmit_response(
                &mut req,
                &ind,
                ISSI,
                &data_transmit_with_resource(resource_request),
                false,
                false,
            )
            .expect("single-slot phase modulation resource should attach MVP PDCH allocation");

        let allocation = req.chan_alloc.expect("PDCH allocation should be present");
        assert_eq!(allocation.usage, None);
        assert_eq!(allocation.timeslots, [false, false, false, true]);
        assert!(!allocation.timeslots[0], "single-slot PDCH must preserve MCCH TS1");
        assert!(
            !allocation.timeslots[1] && !allocation.timeslots[2],
            "single-slot resource/capability must not advertise TS2/TS3"
        );
        assert_eq!(allocation.alloc_type, tetra_saps::lcmc::enums::alloc_type::ChanAllocType::Replace);
        assert_eq!(
            allocation.ul_dl_assigned,
            tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment::Both
        );
    }

    #[test]
    fn mxp600_type_b_unspecified_four_slot_capability_gets_mvp_pdch_allocation() {
        let mut pipeline = pipeline();
        let address = TetraAddress::issi(ISSI);
        pipeline
            .handle_ltpd_mle_unitdata_ind(
                &ltpd_ind(address, dynamic_ipv4_demand_with_ms_type(1, SnPacketDataMsType::TypeBAlternating)),
                HANDLE,
                &snapshot(),
            )
            .expect("Type B activation should produce accept");

        let resource_request = unspecified_four_slot_phase_modulation_resource_request();
        let request_pdu = data_transmit_request_with_resource(1, resource_request);
        assert_eq!(request_pdu.get_len(), 21);
        let ind = ltpd_ind(address, request_pdu);
        let mut req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ind, HANDLE, &snapshot())
            .expect("SN-DATA TRANSMIT REQUEST with unspecified resource should produce response");
        let response = decode_data_transmit_response(&req.sdu).expect("SN-DATA TRANSMIT RESPONSE should decode");
        assert_eq!(response.nsapi, 1);
        assert_eq!(response.result, SndcpDataTransmitResponseResult::Accepted);

        pipeline
            .attach_mvp_pdch_allocation_for_data_transmit_response(
                &mut req,
                &ind,
                ISSI,
                &SndcpDataTransmitRequest {
                    nsapi: 1,
                    logical_link_status: false,
                    resource_request,
                },
                false,
                false,
            )
            .expect("unspecified phase modulation resource should attach MVP PDCH allocation");

        let allocation = req.chan_alloc.expect("PDCH allocation should be present");
        assert_eq!(allocation.usage, None);
        assert_eq!(allocation.timeslots, [false, false, false, true]);
        assert_eq!(allocation.alloc_type, tetra_saps::lcmc::enums::alloc_type::ChanAllocType::Replace);
        assert_eq!(
            allocation.ul_dl_assigned,
            tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment::Both
        );
    }

    #[test]
    fn mxp600_type_b_specific_four_slot_request_gets_mvp_pdch_fallback_allocation() {
        let mut pipeline = pipeline();
        let address = TetraAddress::issi(ISSI);
        pipeline
            .handle_ltpd_mle_unitdata_ind(
                &ltpd_ind(address, dynamic_ipv4_demand_with_ms_type(1, SnPacketDataMsType::TypeBAlternating)),
                HANDLE,
                &snapshot(),
            )
            .expect("Type B activation should produce accept");

        let resource_request = specific_four_slot_phase_modulation_resource_request();
        let request_pdu = data_transmit_request_with_resource(1, resource_request);
        assert_eq!(request_pdu.get_len(), 21);
        let ind = ltpd_ind(address, request_pdu);
        let mut req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ind, HANDLE, &snapshot())
            .expect("SN-DATA TRANSMIT REQUEST with specific four-slot resource should produce response");
        let response = decode_data_transmit_response(&req.sdu).expect("SN-DATA TRANSMIT RESPONSE should decode");
        assert_eq!(response.nsapi, 1);
        assert_eq!(response.result, SndcpDataTransmitResponseResult::Accepted);

        pipeline
            .attach_mvp_pdch_allocation_for_data_transmit_response(
                &mut req,
                &ind,
                ISSI,
                &SndcpDataTransmitRequest {
                    nsapi: 1,
                    logical_link_status: false,
                    resource_request,
                },
                false,
                false,
            )
            .expect("specific symmetric phase modulation resource should attach resource-aware PDCH allocation");

        let allocation = req.chan_alloc.expect("PDCH allocation should be present");
        assert_eq!(allocation.usage, None);
        assert_eq!(allocation.timeslots, [false, false, false, true]);
        assert_eq!(allocation.alloc_type, tetra_saps::lcmc::enums::alloc_type::ChanAllocType::Replace);
        assert_eq!(
            allocation.ul_dl_assigned,
            tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment::Both
        );
    }

    #[test]
    fn mxp600_type_b_specific_four_slot_reconnect_returns_data_transmit_response() {
        let mut pipeline = pipeline();
        let address = TetraAddress::issi(ISSI);
        pipeline
            .handle_ltpd_mle_unitdata_ind(
                &ltpd_ind(address, dynamic_ipv4_demand_with_ms_type(1, SnPacketDataMsType::TypeBAlternating)),
                HANDLE,
                &snapshot(),
            )
            .expect("Type B activation should produce accept");
        pipeline
            .handle_ltpd_mle_unitdata_ind(
                &ltpd_ind(
                    address,
                    data_transmit_request_with_resource(1, specific_four_slot_phase_modulation_resource_request()),
                ),
                HANDLE,
                &snapshot(),
            )
            .expect("initial SN-DATA TRANSMIT REQUEST should enter READY");

        let reconnect_pdu = reconnect_with_resource(1, specific_four_slot_phase_modulation_resource_request());
        assert_eq!(reconnect_pdu.get_len(), 21);
        let req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, reconnect_pdu), HANDLE, &snapshot())
            .expect("SN-RECONNECT with data to send should produce response");
        let response = decode_data_transmit_response(&req.sdu).expect("SN-DATA TRANSMIT RESPONSE should decode");

        assert_eq!(response.nsapi, 1);
        assert_eq!(response.result, SndcpDataTransmitResponseResult::Accepted);
        assert_eq!(pipeline.session().state_for_issi(ISSI), SwmiSndcpState::Ready);
    }

    #[test]
    fn repeated_specific_four_slot_request_refreshes_existing_pdch_allocation_after_end_of_data() {
        let mut pipeline = pipeline();
        let address = TetraAddress::issi(ISSI);
        let resource_request = specific_four_slot_phase_modulation_resource_request();
        pipeline
            .handle_ltpd_mle_unitdata_ind(
                &ltpd_ind(address, dynamic_ipv4_demand_with_ms_type(1, SnPacketDataMsType::TypeBAlternating)),
                HANDLE,
                &snapshot(),
            )
            .expect("Type B activation should produce accept");
        let first_ind = ltpd_ind(address, data_transmit_request_with_resource(1, resource_request));
        let mut first_req = pipeline
            .handle_ltpd_mle_unitdata_ind(&first_ind, HANDLE, &snapshot())
            .expect("initial SN-DATA TRANSMIT REQUEST should produce response");
        pipeline
            .attach_mvp_pdch_allocation_for_data_transmit_response(
                &mut first_req,
                &first_ind,
                ISSI,
                &SndcpDataTransmitRequest {
                    nsapi: 1,
                    logical_link_status: false,
                    resource_request,
                },
                false,
                false,
            )
            .expect("initial specific four-slot request should attach a channel allocation");
        pipeline.mark_pdch_ready(ISSI, first_ind.endpoint_id, first_ind.link_id);

        pipeline
            .handle_ltpd_mle_unitdata_ind(
                &ltpd_ind(
                    address,
                    encode_end_of_data(&SndcpEndOfData {
                        immediate_service_change: false,
                    })
                    .expect("SN-END OF DATA should encode"),
                ),
                HANDLE,
                &snapshot(),
            )
            .expect("SN-END OF DATA should return SNDCP to STANDBY");
        assert_eq!(pipeline.session().state_for_issi(ISSI), SwmiSndcpState::Standby);

        let refresh_ind = ltpd_ind(address, data_transmit_request_with_resource(1, resource_request));
        let mut refresh_req = pipeline
            .handle_ltpd_mle_unitdata_ind(&refresh_ind, HANDLE, &snapshot())
            .expect("new SN-DATA TRANSMIT REQUEST should produce response");
        pipeline
            .attach_mvp_pdch_allocation_for_data_transmit_response(
                &mut refresh_req,
                &refresh_ind,
                ISSI,
                &SndcpDataTransmitRequest {
                    nsapi: 1,
                    logical_link_status: false,
                    resource_request,
                },
                false,
                false,
            )
            .expect("existing PDCH should be refreshed with the requested channel allocation");

        let allocation = refresh_req
            .chan_alloc
            .expect("existing PDCH refresh must carry a replacement channel allocation");
        assert_eq!(allocation.usage, None);
        assert_eq!(allocation.timeslots, [false, false, false, true]);
        assert_eq!(allocation.alloc_type, ChanAllocType::Replace);
        assert_eq!(allocation.ul_dl_assigned, UlDlAssignment::Both);
    }

    #[test]
    fn end_of_data_response_can_attach_quit_and_go_common_control_allocation() {
        let mut pipeline = pipeline();
        let address = TetraAddress::issi(ISSI);
        let resource_request = specific_four_slot_phase_modulation_resource_request();
        pipeline
            .handle_ltpd_mle_unitdata_ind(
                &ltpd_ind(address, dynamic_ipv4_demand_with_ms_type(1, SnPacketDataMsType::TypeBAlternating)),
                HANDLE,
                &snapshot(),
            )
            .expect("Type B activation should produce accept");
        let data_ind = ltpd_ind(address, data_transmit_request_with_resource(1, resource_request));
        let mut data_req = pipeline
            .handle_ltpd_mle_unitdata_ind(&data_ind, HANDLE, &snapshot())
            .expect("SN-DATA TRANSMIT REQUEST should produce response");
        pipeline
            .attach_mvp_pdch_allocation_for_data_transmit_response(
                &mut data_req,
                &data_ind,
                ISSI,
                &SndcpDataTransmitRequest {
                    nsapi: 1,
                    logical_link_status: false,
                    resource_request,
                },
                false,
                false,
            )
            .expect("PDCH allocation should attach");
        pipeline.mark_pdch_ready(ISSI, data_ind.endpoint_id, data_ind.link_id);

        let end_ind = ltpd_ind(
            address,
            encode_end_of_data(&SndcpEndOfData {
                immediate_service_change: false,
            })
            .expect("SN-END OF DATA should encode"),
        );
        let mut end_req = pipeline
            .handle_ltpd_mle_unitdata_ind(&end_ind, HANDLE, &snapshot())
            .expect("SN-END OF DATA should produce response");
        assert!(
            pipeline
                .attach_common_control_allocation_for_end_of_data_response(&mut end_req, &end_ind, ISSI)
                .expect("SN-END OF DATA common-control allocation should attach")
        );

        let allocation = end_req.chan_alloc.expect("SN-END OF DATA should carry common-control allocation");
        assert_eq!(allocation.timeslots, [false, false, false, false]);
        assert_eq!(allocation.alloc_type, ChanAllocType::QuitAndGo);
        assert_eq!(
            pipeline
                .pdch()
                .ensure_packet_data_ready(ISSI, end_ind.endpoint_id, SNDCP_BASIC_LINK_ID),
            Err(SndcpPdchError::PacketDataBearerNotReady {
                issi: ISSI,
                state: SndcpPdchState::CommonControl
            })
        );
    }

    #[test]
    fn mvp_pdch_allocation_refuses_active_circuit_mode_service() {
        let mut pipeline = pipeline();
        let address = TetraAddress::issi(ISSI);
        pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, dynamic_ipv4_demand(2)), HANDLE, &snapshot())
            .expect("activation should produce accept");
        let ind = ltpd_ind(address, data_transmit_request(2));
        let mut req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ind, HANDLE, &snapshot())
            .expect("SN-DATA TRANSMIT REQUEST should produce response");

        assert_eq!(
            pipeline.attach_mvp_pdch_allocation_for_data_transmit_response(&mut req, &ind, ISSI, &data_transmit(), true, false),
            Err(SndcpWapLtpdPipelineError::Pdch(SndcpPdchError::CircuitModeConflict { issi: ISSI }))
        );
        assert!(req.chan_alloc.is_none());
    }

    #[test]
    fn mvp_pdch_allocation_allows_parallel_voice_data_when_lower_capacity_exists() {
        let mut pipeline = pipeline();
        let address = TetraAddress::issi(ISSI);
        pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, dynamic_ipv4_demand(2)), HANDLE, &snapshot())
            .expect("activation should produce accept");
        let ind = ltpd_ind(address, data_transmit_request(2));
        let mut req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ind, HANDLE, &snapshot())
            .expect("SN-DATA TRANSMIT REQUEST should produce response");

        pipeline
            .attach_mvp_pdch_allocation_for_data_transmit_response(&mut req, &ind, ISSI, &data_transmit(), true, true)
            .expect("parallel voice/data policy should allow PDCH allocation when lower capacity exists");

        assert!(req.chan_alloc.is_some());
    }

    #[test]
    fn wap_unitdata_before_ready_is_rejected_without_mle_response() {
        let mut pipeline = pipeline();
        let address = TetraAddress::issi(ISSI);
        pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, dynamic_ipv4_demand(2)), HANDLE, &snapshot())
            .expect("activation should produce accept");

        let request_npdu = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        let unitdata = encode_sn_unitdata(2, 0, 0, &BitBuffer::from_bytes(&request_npdu)).expect("SN-UNITDATA should encode");

        let error = pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, unitdata), HANDLE, &snapshot())
            .expect_err("READY state should be required before WAP response");

        assert_eq!(
            error,
            SndcpWapLtpdPipelineError::Session(SndcpWapSessionError::Bearer(SndcpBearerError::PacketDataTransferNotReady {
                issi: ISSI,
                state: SwmiSndcpState::Standby
            }))
        );
    }

    #[test]
    fn wap_unitdata_after_ready_without_pdch_ready_is_rejected_without_mle_response() {
        let mut pipeline = pipeline();
        let address = TetraAddress::issi(ISSI);
        pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, dynamic_ipv4_demand(2)), HANDLE, &snapshot())
            .expect("activation should produce accept");
        enter_ready(&mut pipeline, address);

        let request_npdu = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        let unitdata = encode_sn_unitdata(2, 0, 0, &BitBuffer::from_bytes(&request_npdu)).expect("SN-UNITDATA should encode");

        let error = pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, unitdata), HANDLE, &snapshot())
            .expect_err("PDCH readiness should be required before WAP response");

        assert_eq!(
            error,
            SndcpWapLtpdPipelineError::Pdch(SndcpPdchError::PacketDataBearerNotReady {
                issi: ISSI,
                state: SndcpPdchState::CommonControl
            })
        );
    }

    #[test]
    fn active_wap_unitdata_uses_negotiated_context_pdu_priority_max() {
        let mut pipeline = pipeline_with_policy(SndcpPdpPolicy {
            pdu_priority_max: 3,
            ..SndcpPdpPolicy::experimental_wap_ipv4()
        });
        let address = TetraAddress::issi(ISSI);
        pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, dynamic_ipv4_demand(2)), HANDLE, &snapshot())
            .expect("activation should produce accept");
        enter_ready(&mut pipeline, address);
        pipeline.mark_pdch_ready(ISSI, 3, 7);

        let request_npdu = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        let unitdata = encode_sn_unitdata(2, 0, 0, &BitBuffer::from_bytes(&request_npdu)).expect("SN-UNITDATA should encode");

        let req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, unitdata), HANDLE, &snapshot())
            .expect("WAP request should produce MLE-UNITDATA response");

        assert_eq!(req.pdu_prio, 3);
        assert_eq!(req.layer2service, Layer2Service::Acknowledged);
        assert_eq!(req.unacked_bl_repetitions, -1);
        assert_eq!(req.data_prio, 2);
        assert!(!req.mle_data_prio_flag);
        assert!(req.packet_data_flag);
    }

    #[test]
    fn active_wap_unitdata_can_use_unacknowledged_basic_link_when_realtime_qos_is_negotiated() {
        let mut pipeline = pipeline_with_policy(SndcpPdpPolicy {
            default_bearer_profile: SndcpPacketDataBearerProfile::negotiated_realtime_unacknowledged(1, true),
            ..SndcpPdpPolicy::experimental_wap_ipv4()
        });
        let address = TetraAddress::issi(ISSI);
        pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, dynamic_ipv4_demand(2)), HANDLE, &snapshot())
            .expect("activation should produce accept");
        enter_ready(&mut pipeline, address);
        pipeline.mark_pdch_ready(ISSI, 3, 0);

        let request_npdu = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        let unitdata = encode_sn_unitdata(2, 0, 0, &BitBuffer::from_bytes(&request_npdu)).expect("SN-UNITDATA should encode");

        let req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind_on_link(address, unitdata, 0), HANDLE, &snapshot())
            .expect("WAP request should produce MLE-UNITDATA response");

        assert_eq!(req.layer2service, Layer2Service::Unacknowledged);
        assert_eq!(req.unacked_bl_repetitions, 1);
        assert!(req.fcs_flag);
        assert!(req.packet_data_flag);
    }

    #[test]
    fn deactivation_removes_context_before_later_ltpd_unitdata() {
        let mut pipeline = pipeline();
        let address = TetraAddress::issi(ISSI);
        pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, dynamic_ipv4_demand(2)), HANDLE, &snapshot())
            .expect("activation should produce accept");

        let deactivation = encode_deactivate_pdp_context_demand(&SndcpDeactivation::Nsapi(2)).expect("deactivation demand should encode");
        let deactivation_req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, deactivation), HANDLE, &snapshot())
            .expect("deactivation should produce accept");
        assert_eq!(
            decode_deactivate_pdp_context_accept(&deactivation_req.sdu).expect("deactivation accept should decode"),
            SndcpDeactivation::Nsapi(2)
        );
        assert_eq!(deactivation_req.layer2service, Layer2Service::Acknowledged);
        assert_eq!(deactivation_req.unacked_bl_repetitions, -1);
        assert!(!deactivation_req.packet_data_flag);

        let request_npdu = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        let unitdata = encode_sn_unitdata(2, 0, 0, &BitBuffer::from_bytes(&request_npdu)).expect("SN-UNITDATA should encode");

        let error = pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, unitdata), HANDLE, &snapshot())
            .expect_err("deactivated context should reject WAP response");

        assert!(matches!(
            error,
            SndcpWapLtpdPipelineError::Session(SndcpWapSessionError::Wap(WapGatewayError::MissingContext(_)))
        ));
    }

    #[test]
    fn non_issi_ltpd_address_rejects_without_context_mutation() {
        let mut pipeline = pipeline();
        let ind = ltpd_ind(TetraAddress::new(GSSI, SsiType::Gssi), dynamic_ipv4_demand(2));

        assert_eq!(
            pipeline
                .handle_ltpd_mle_unitdata_ind(&ind, HANDLE, &snapshot())
                .expect_err("GSSI packet-data source should reject"),
            SndcpWapLtpdPipelineError::NonIssiAddress(TetraAddress::new(GSSI, SsiType::Gssi))
        );
        assert_eq!(pipeline.session().pdp().contexts().len(), 0);
    }

    #[test]
    fn channel_change_request_rejects_until_supported() {
        let mut pipeline = pipeline();
        let mut ind = ltpd_ind(TetraAddress::issi(ISSI), dynamic_ipv4_demand(2));
        ind.chan_change_resp_req = true;
        ind.chan_change_handle = Some(11);

        assert_eq!(
            pipeline
                .handle_ltpd_mle_unitdata_ind(&ind, HANDLE, &snapshot())
                .expect_err("channel-change response is not implemented"),
            SndcpWapLtpdPipelineError::Pdch(SndcpPdchError::ChannelChangeResponseRequired { issi: ISSI, handle: 11 })
        );
        assert_eq!(pipeline.session().pdp().contexts().len(), 0);
    }

    #[test]
    fn unsupported_sndcp_pdu_type_does_not_emit_ltpd_request() {
        let mut pdu = BitBuffer::new(4);
        pdu.write_bits(12, 4);
        pdu.seek(0);
        let mut pipeline = pipeline();

        assert_eq!(
            pipeline
                .handle_ltpd_mle_unitdata_ind(&ltpd_ind(TetraAddress::issi(ISSI), pdu), HANDLE, &snapshot())
                .expect_err("unsupported PDU should reject before MLE request"),
            SndcpWapLtpdPipelineError::Session(SndcpWapSessionError::UnsupportedInboundPduType(12))
        );
        assert_eq!(pipeline.session().pdp().contexts().len(), 0);
    }
}
