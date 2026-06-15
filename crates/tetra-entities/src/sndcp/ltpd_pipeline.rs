// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original pure TETRA SNDCP LTPD WAP/IP pipeline primitive.

use super::mle_adapter::{
    SndcpLtpdUnitdataOptions, SndcpMleAdapterError, sn_unitdata_req_to_ltpd_mle_unitdata_req, sndcp_pdu_to_ltpd_mle_unitdata_req,
};
use super::pdch::{SndcpPdchError, SndcpPdchManager};
use super::wap_gateway::WapStatusUnitdataResponse;
use super::wap_session::{SndcpWapSession, SndcpWapSessionError, SndcpWapSessionResponse};
use super::wap_status::WapStatusSnapshot;
use tetra_core::{EndpointId, LinkId, MleHandle, SsiType, TetraAddress};
use tetra_saps::ltpd::{LtpdMleUnitdataInd, LtpdMleUnitdataReq};

pub const SNDCP_MLE_HANDLE_MIN: MleHandle = 1;
pub const SNDCP_MLE_HANDLE_MAX: MleHandle = i32::MAX as MleHandle;

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

    pub fn handle_ltpd_mle_unitdata_ind_allocating(
        &mut self,
        ind: &LtpdMleUnitdataInd,
        snapshot: &WapStatusSnapshot,
    ) -> Result<LtpdMleUnitdataReq, SndcpWapLtpdPipelineError> {
        let issi = issi_from_ltpd_ind(ind)?;
        self.pdch.observe_ltpd_unitdata_ind(issi, ind)?;
        let handle = self.handles.allocate();
        self.handle_validated_ltpd_mle_unitdata_ind(ind, issi, handle, snapshot)
    }

    pub fn handle_ltpd_mle_unitdata_ind(
        &mut self,
        ind: &LtpdMleUnitdataInd,
        handle: MleHandle,
        snapshot: &WapStatusSnapshot,
    ) -> Result<LtpdMleUnitdataReq, SndcpWapLtpdPipelineError> {
        let issi = issi_from_ltpd_ind(ind)?;
        self.pdch.observe_ltpd_unitdata_ind(issi, ind)?;
        self.handle_validated_ltpd_mle_unitdata_ind(ind, issi, handle, snapshot)
    }

    fn handle_validated_ltpd_mle_unitdata_ind(
        &mut self,
        ind: &LtpdMleUnitdataInd,
        issi: u32,
        handle: MleHandle,
        snapshot: &WapStatusSnapshot,
    ) -> Result<LtpdMleUnitdataReq, SndcpWapLtpdPipelineError> {
        let response = self.session.handle_inbound_pdu_response(issi, handle, &ind.sdu, snapshot)?;

        match response {
            SndcpWapSessionResponse::Control(pdu) => Ok(sndcp_pdu_to_ltpd_mle_unitdata_req(
                pdu,
                handle,
                control_response_options_from_ind(ind),
            )?),
            SndcpWapSessionResponse::Unitdata(response) => {
                self.pdch.ensure_packet_data_ready(issi, ind.endpoint_id, ind.link_id)?;
                Ok(sn_unitdata_req_to_ltpd_mle_unitdata_req(
                    &response.unitdata,
                    handle,
                    packet_data_response_options_from_ind(ind, &response),
                )?)
            }
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
    debug_assert_eq!(bearer.layer2service, tetra_core::Layer2Service::Unacknowledged);
    let options = SndcpLtpdUnitdataOptions::packet_data_unacknowledged(
        ind.received_tetra_address,
        ind.endpoint_id,
        ind.link_id,
        response.pdu_priority_max,
    );
    options
        .with_unacked_bl_repetitions(bearer.unacked_bl_repetitions)
        .with_nsapi_data_priority(response.nsapi_data_priority)
        .with_ms_default_data_priority(response.ms_default_data_priority)
        .with_data_scheduling(response.scheduling)
        .with_fcs(bearer.fcs_flag)
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
    use crate::sndcp::pdch::{SndcpPdchError, SndcpPdchState};
    use crate::sndcp::pdp::{
        SndcpActivateAddressDemand, SndcpActivatePdpContextDemand, SndcpActivationRejectCause, SndcpDeactivation,
        decode_activate_pdp_context_accept, decode_activate_pdp_context_reject, decode_deactivate_pdp_context_accept,
        encode_activate_pdp_context_demand, encode_deactivate_pdp_context_demand,
    };
    use crate::sndcp::pdp_service::SndcpPdpPolicy;
    use crate::sndcp::state::SwmiSndcpState;
    use crate::sndcp::transfer::{
        SndcpDataTransmitRequest, SndcpDataTransmitResponseResult, decode_data_transmit_response, encode_data_transmit_request,
    };
    use crate::sndcp::unitdata::{decode_sn_unitdata_pdu, encode_sn_unitdata};
    use crate::sndcp::wap_gateway::WapGatewayError;
    use crate::sndcp::wap_ip::{WapIpEndpoint, WapIpServicePolicy};
    use tetra_core::{BitBuffer, Layer2Service, Todo};
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
            stack_version: "v0.1.68_dev-test".to_string(),
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
        encode_activate_pdp_context_demand(&SndcpActivatePdpContextDemand {
            sndcp_version: 1,
            nsapi,
            address: SndcpActivateAddressDemand::Ipv4Dynamic,
            packet_data_ms_type: SnPacketDataMsType::TypeAParallel,
            pcomp_negotiation: 0,
        })
        .expect("activation demand should encode")
    }

    fn ltpd_ind(address: TetraAddress, sdu: BitBuffer) -> LtpdMleUnitdataInd {
        LtpdMleUnitdataInd {
            sdu,
            endpoint_id: 3,
            link_id: 7,
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
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode")
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
        let response_unitdata = decode_sn_unitdata_pdu(&req.sdu).expect("response SN-UNITDATA should decode");
        let response_npdu = bitbuffer_npdu_octets(&response_unitdata.n_pdu).expect("response N-PDU should be octet aligned");
        let response_ip = parse_ipv4_packet(&response_npdu).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");

        assert_eq!(req.address, address);
        assert_eq!(req.endpoint_id, 3);
        assert_eq!(req.link_id, 7);
        assert_eq!(req.layer2service, Layer2Service::Unacknowledged);
        assert_eq!(req.unacked_bl_repetitions, 0);
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
        assert_eq!(req.layer2service, Layer2Service::Unacknowledged);
        assert_eq!(req.unacked_bl_repetitions, 0);
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
        pipeline.mark_pdch_ready(ISSI, 3, 7);

        let request_npdu = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        let unitdata = encode_sn_unitdata(2, 0, 0, &BitBuffer::from_bytes(&request_npdu)).expect("SN-UNITDATA should encode");

        let req = pipeline
            .handle_ltpd_mle_unitdata_ind(&ltpd_ind(address, unitdata), HANDLE, &snapshot())
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
        pdu.write_bits(9, 4);
        pdu.seek(0);
        let mut pipeline = pipeline();

        assert_eq!(
            pipeline
                .handle_ltpd_mle_unitdata_ind(&ltpd_ind(TetraAddress::issi(ISSI), pdu), HANDLE, &snapshot())
                .expect_err("unsupported PDU should reject before MLE request"),
            SndcpWapLtpdPipelineError::Session(SndcpWapSessionError::UnsupportedInboundPduType(9))
        );
        assert_eq!(pipeline.session().pdp().contexts().len(), 0);
    }
}
