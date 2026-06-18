// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original pure TETRA SNDCP WAP/IP session primitive.

use super::bearer::{SndcpBearerActivationOutcome, SndcpBearerError, SndcpBearerManager};
use super::pdp::{
    SndcpActivateAddressDemand, SndcpActivatePdpContextDemand, SndcpPdpError, decode_activate_pdp_context_demand,
    decode_deactivate_pdp_context_demand, encode_activate_pdp_context_accept, encode_activate_pdp_context_reject,
    encode_deactivate_pdp_context_accept,
};
use super::pdp_service::{SndcpPdpPolicy, SndcpPdpService};
use super::state::SwmiSndcpState;
use super::transfer::{
    SN_PDU_TYPE_DATA, SN_PDU_TYPE_DATA_TRANSMIT_REQUEST, SN_PDU_TYPE_END_OF_DATA, SN_PDU_TYPE_RECONNECT, SndcpTransferError,
    decode_data_transmit_request, decode_end_of_data, decode_reconnect,
};
use super::unitdata::{SndcpEncodeError, SndcpUnitdataError, sn_unitdata_req_to_pdu, sn_user_data_ind_from_pdu};
use super::wap_gateway::{WapGatewayError, WapStatusUnitdataResponse, build_wap_status_unitdata_response_optional};
use super::wap_ip::{WapIpEndpoint, WapIpServicePolicy};
use super::wap_status::WapStatusSnapshot;
use tetra_core::{BitBuffer, MleHandle};
use tetra_saps::sn::{SnPacketDataMsType, SnUnitdataReq};

const SN_PDU_TYPE_ACTIVATE_PDP_CONTEXT_DEMAND: u8 = 0;
const SN_PDU_TYPE_DEACTIVATE_PDP_CONTEXT_DEMAND: u8 = 2;
const SN_PDU_TYPE_UNITDATA: u8 = 4;

#[derive(Debug, Clone)]
pub struct SndcpWapSession {
    bearer: SndcpBearerManager,
    endpoint: WapIpEndpoint,
    wap_policy: WapIpServicePolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SndcpWapSessionError {
    TooShort(&'static str),
    UnsupportedInboundPduType(u8),
    Pdp(SndcpPdpError),
    Unitdata(SndcpUnitdataError),
    UnitdataEncode(SndcpEncodeError),
    Wap(WapGatewayError),
    Bearer(SndcpBearerError),
    Transfer(SndcpTransferError),
    MissingControlPdu(&'static str),
}

#[derive(Debug, Clone)]
pub enum SndcpWapSessionResponse {
    Control(BitBuffer),
    Unitdata(WapStatusUnitdataResponse),
    NoResponse,
}

impl From<SndcpPdpError> for SndcpWapSessionError {
    fn from(value: SndcpPdpError) -> Self {
        SndcpWapSessionError::Pdp(value)
    }
}

impl From<SndcpUnitdataError> for SndcpWapSessionError {
    fn from(value: SndcpUnitdataError) -> Self {
        SndcpWapSessionError::Unitdata(value)
    }
}

impl From<SndcpEncodeError> for SndcpWapSessionError {
    fn from(value: SndcpEncodeError) -> Self {
        SndcpWapSessionError::UnitdataEncode(value)
    }
}

impl From<WapGatewayError> for SndcpWapSessionError {
    fn from(value: WapGatewayError) -> Self {
        SndcpWapSessionError::Wap(value)
    }
}

impl From<SndcpBearerError> for SndcpWapSessionError {
    fn from(value: SndcpBearerError) -> Self {
        SndcpWapSessionError::Bearer(value)
    }
}

impl From<SndcpTransferError> for SndcpWapSessionError {
    fn from(value: SndcpTransferError) -> Self {
        SndcpWapSessionError::Transfer(value)
    }
}

impl SndcpWapSession {
    pub fn new(policy: SndcpPdpPolicy, endpoint: WapIpEndpoint, wap_policy: WapIpServicePolicy) -> Self {
        Self {
            bearer: SndcpBearerManager::new(policy),
            endpoint,
            wap_policy,
        }
    }

    pub fn with_bearer_manager(bearer: SndcpBearerManager, endpoint: WapIpEndpoint, wap_policy: WapIpServicePolicy) -> Self {
        Self {
            bearer,
            endpoint,
            wap_policy,
        }
    }

    pub fn pdp(&self) -> &SndcpPdpService {
        self.bearer.pdp()
    }

    pub fn bearer(&self) -> &SndcpBearerManager {
        &self.bearer
    }

    pub fn state_for_issi(&self, issi: u32) -> SwmiSndcpState {
        self.bearer.state_for_issi(issi)
    }

    pub fn mark_ready_bearer_temporarily_broken(&mut self, issi: u32) -> Result<(), SndcpWapSessionError> {
        if let Some(transition) = self.bearer.mark_ready_bearer_temporarily_broken(issi)? {
            tracing::info!(
                "SNDCP/WAP-IP: packet-data bearer temporarily broken issi={} {:?}->{:?}",
                issi,
                transition.previous_state,
                transition.new_state
            );
        }
        Ok(())
    }

    pub fn deregister_issi(&mut self, issi: u32) -> Result<(), SndcpWapSessionError> {
        let transition = self.bearer.deregister_issi(issi)?;
        tracing::info!(
            "SNDCP/WAP-IP: deregistered issi={} state={:?}->{:?}",
            issi,
            transition.previous_state,
            transition.new_state
        );
        Ok(())
    }

    pub fn endpoint(&self) -> WapIpEndpoint {
        self.endpoint
    }

    pub fn wap_policy(&self) -> &WapIpServicePolicy {
        &self.wap_policy
    }

    pub fn handle_inbound_pdu(
        &mut self,
        issi: u32,
        handle: MleHandle,
        pdu: &BitBuffer,
        snapshot: &WapStatusSnapshot,
    ) -> Result<BitBuffer, SndcpWapSessionError> {
        self.handle_inbound_pdu_response(issi, handle, pdu, snapshot)?.into_pdu()
    }

    pub fn handle_inbound_pdu_response(
        &mut self,
        issi: u32,
        handle: MleHandle,
        pdu: &BitBuffer,
        snapshot: &WapStatusSnapshot,
    ) -> Result<SndcpWapSessionResponse, SndcpWapSessionError> {
        match sn_pdu_type(pdu)? {
            SN_PDU_TYPE_ACTIVATE_PDP_CONTEXT_DEMAND => self
                .handle_activate_pdp_context_demand(issi, pdu)
                .map(SndcpWapSessionResponse::Control),
            SN_PDU_TYPE_DEACTIVATE_PDP_CONTEXT_DEMAND => self
                .handle_deactivate_pdp_context_demand(issi, pdu)
                .map(SndcpWapSessionResponse::Control),
            SN_PDU_TYPE_DATA_TRANSMIT_REQUEST => self.handle_data_transmit_request(issi, pdu).map(SndcpWapSessionResponse::Control),
            SN_PDU_TYPE_END_OF_DATA => self.handle_end_of_data_response(issi, pdu),
            SN_PDU_TYPE_RECONNECT => self.handle_reconnect_response(issi, pdu),
            SN_PDU_TYPE_UNITDATA | SN_PDU_TYPE_DATA => self.handle_unitdata_session_response(issi, handle, pdu, snapshot),
            other => Err(SndcpWapSessionError::UnsupportedInboundPduType(other)),
        }
    }

    pub fn handle_activate_pdp_context_demand(&mut self, issi: u32, pdu: &BitBuffer) -> Result<BitBuffer, SndcpWapSessionError> {
        let demand = decode_activate_pdp_context_demand(pdu)?;

        match self.bearer.handle_activate_demand(issi, demand)? {
            SndcpBearerActivationOutcome::Accepted { accept, .. } => {
                tracing::info!(
                    "SNDCP/WAP-IP: PDP activation accepted issi={} nsapi={} assigned_address={:?}",
                    issi,
                    accept.nsapi,
                    accept.assigned_address
                );
                Ok(encode_activate_pdp_context_accept(&accept)?)
            }
            SndcpBearerActivationOutcome::Rejected(reject) => {
                tracing::info!(
                    "SNDCP/WAP-IP: PDP activation rejected issi={} nsapi={} cause={:?}",
                    issi,
                    reject.nsapi,
                    reject.cause
                );
                Ok(encode_activate_pdp_context_reject(&reject)?)
            }
        }
    }

    pub fn handle_deactivate_pdp_context_demand(&mut self, issi: u32, pdu: &BitBuffer) -> Result<BitBuffer, SndcpWapSessionError> {
        let deactivation = decode_deactivate_pdp_context_demand(pdu)?;
        let result = self.bearer.handle_deactivate_demand(issi, deactivation);
        Ok(encode_deactivate_pdp_context_accept(&result.deactivation.accept)?)
    }

    pub fn handle_data_transmit_request(&mut self, issi: u32, pdu: &BitBuffer) -> Result<BitBuffer, SndcpWapSessionError> {
        let request = decode_data_transmit_request(pdu)?;
        let _ = self.recover_missing_wap_ipv4_context_for_ms_transfer(issi, request.nsapi)?;
        let outcome = self.bearer.handle_ms_data_transmit_request(issi, request)?;
        outcome
            .control_pdu
            .ok_or(SndcpWapSessionError::MissingControlPdu("sn_data_transmit_response"))
    }

    pub fn handle_end_of_data_response(&mut self, issi: u32, pdu: &BitBuffer) -> Result<SndcpWapSessionResponse, SndcpWapSessionError> {
        let end_of_data = decode_end_of_data(pdu)?;
        let outcome = self.bearer.handle_end_of_data_received(issi, end_of_data, false)?;
        Ok(match outcome.control_pdu {
            Some(pdu) => SndcpWapSessionResponse::Control(pdu),
            None => SndcpWapSessionResponse::NoResponse,
        })
    }

    pub fn handle_reconnect(&mut self, issi: u32, pdu: &BitBuffer) -> Result<BitBuffer, SndcpWapSessionError> {
        self.handle_reconnect_response(issi, pdu)?.into_pdu()
    }

    pub fn handle_reconnect_response(&mut self, issi: u32, pdu: &BitBuffer) -> Result<SndcpWapSessionResponse, SndcpWapSessionError> {
        let reconnect = decode_reconnect(pdu)?;
        if reconnect.nsapi.is_none()
            && self.bearer.state_for_issi(issi) == SwmiSndcpState::Idle
            && self.bearer.pdp().contexts().contexts_for_issi(issi).next().is_none()
        {
            tracing::info!(
                "SNDCP/WAP-IP: ignoring SN-RECONNECT without data for issi={} because no local PDP context survived",
                issi
            );
            return Ok(SndcpWapSessionResponse::NoResponse);
        }
        if let Some(nsapi) = reconnect.nsapi {
            let recovered = self.recover_missing_wap_ipv4_context_for_ms_transfer(issi, nsapi)?;
            if recovered {
                let outcome = self.bearer.handle_ms_data_transmit_request(
                    issi,
                    super::transfer::SndcpDataTransmitRequest {
                        nsapi,
                        logical_link_status: false,
                        resource_request: reconnect.resource_request,
                    },
                )?;
                return Ok(match outcome.control_pdu {
                    Some(pdu) => SndcpWapSessionResponse::Control(pdu),
                    None => SndcpWapSessionResponse::NoResponse,
                });
            }
        }
        let outcome = self.bearer.handle_reconnect_received(issi, reconnect)?;
        Ok(match outcome.control_pdu {
            Some(pdu) => SndcpWapSessionResponse::Control(pdu),
            None => SndcpWapSessionResponse::NoResponse,
        })
    }

    fn recover_missing_wap_ipv4_context_for_ms_transfer(&mut self, issi: u32, nsapi: u8) -> Result<bool, SndcpWapSessionError> {
        if self.bearer.pdp().contexts().get_issi_nsapi(issi, nsapi).ok().flatten().is_some() {
            return Ok(false);
        }

        // Clause 28.2.1 requires PDP activation before normal SNDCP data, but
        // after a SwMI restart a terminal can still believe its previously
        // activated PDP context exists and immediately request data transfer.
        // Keep this recovery local to the WAP/IP endpoint and reconstruct the
        // minimal dynamic IPv4 primary context needed to answer or reject the
        // following SN-DATA TRANSMIT REQUEST through the normal bearer path.
        let demand = SndcpActivatePdpContextDemand {
            sndcp_version: 1,
            nsapi,
            address: SndcpActivateAddressDemand::Ipv4Dynamic,
            packet_data_ms_type: SnPacketDataMsType::TypeAParallel,
            pcomp_negotiation: 0,
        };

        match self.bearer.handle_activate_demand(issi, demand)? {
            SndcpBearerActivationOutcome::Accepted { accept, .. } => {
                tracing::warn!(
                    "SNDCP/WAP-IP: recovered missing dynamic IPv4 PDP context for issi={} nsapi={} assigned_address={:?} before MS data transfer",
                    issi,
                    nsapi,
                    accept.assigned_address
                );
            }
            SndcpBearerActivationOutcome::Rejected(reject) => {
                tracing::warn!(
                    "SNDCP/WAP-IP: unable to recover missing PDP context for issi={} nsapi={} before MS data transfer: {:?}",
                    issi,
                    nsapi,
                    reject.cause
                );
            }
        }

        Ok(true)
    }

    pub fn handle_unitdata(
        &mut self,
        issi: u32,
        handle: MleHandle,
        pdu: &BitBuffer,
        snapshot: &WapStatusSnapshot,
    ) -> Result<BitBuffer, SndcpWapSessionError> {
        Ok(sn_unitdata_req_to_pdu(&self.handle_unitdata_req(issi, handle, pdu, snapshot)?)?)
    }

    pub fn handle_unitdata_req(
        &mut self,
        issi: u32,
        handle: MleHandle,
        pdu: &BitBuffer,
        snapshot: &WapStatusSnapshot,
    ) -> Result<SnUnitdataReq, SndcpWapSessionError> {
        Ok(self.handle_unitdata_response(issi, handle, pdu, snapshot)?.unitdata)
    }

    pub fn handle_unitdata_response(
        &mut self,
        issi: u32,
        handle: MleHandle,
        pdu: &BitBuffer,
        snapshot: &WapStatusSnapshot,
    ) -> Result<WapStatusUnitdataResponse, SndcpWapSessionError> {
        match self.handle_unitdata_session_response(issi, handle, pdu, snapshot)? {
            SndcpWapSessionResponse::Unitdata(response) => Ok(response),
            SndcpWapSessionResponse::NoResponse => Err(SndcpWapSessionError::MissingControlPdu("no_response")),
            SndcpWapSessionResponse::Control(_) => Err(SndcpWapSessionError::MissingControlPdu("unitdata_response")),
        }
    }

    fn handle_unitdata_session_response(
        &mut self,
        issi: u32,
        handle: MleHandle,
        pdu: &BitBuffer,
        snapshot: &WapStatusSnapshot,
    ) -> Result<SndcpWapSessionResponse, SndcpWapSessionError> {
        let unitdata = sn_user_data_ind_from_pdu(pdu)?;
        let Some(response) = build_wap_status_unitdata_response_optional(
            self.bearer.pdp().contexts(),
            issi,
            handle,
            &unitdata,
            self.endpoint,
            &self.wap_policy,
            snapshot,
        )?
        else {
            return Ok(SndcpWapSessionResponse::NoResponse);
        };
        self.bearer.prepare_swmi_unitdata_transfer(issi, unitdata.nsapi)?;
        Ok(SndcpWapSessionResponse::Unitdata(response))
    }
}

impl SndcpWapSessionResponse {
    pub fn into_pdu(self) -> Result<BitBuffer, SndcpWapSessionError> {
        match self {
            SndcpWapSessionResponse::Control(pdu) => Ok(pdu),
            SndcpWapSessionResponse::Unitdata(response) => Ok(sn_unitdata_req_to_pdu(&response.unitdata)?),
            SndcpWapSessionResponse::NoResponse => Err(SndcpWapSessionError::MissingControlPdu("no_response")),
        }
    }
}

fn sn_pdu_type(pdu: &BitBuffer) -> Result<u8, SndcpWapSessionError> {
    let mut pdu = BitBuffer::from_bitbuffer(pdu);
    pdu.seek(0);
    pdu.read_bits(4)
        .map(|value| value as u8)
        .ok_or(SndcpWapSessionError::TooShort("sn_pdu_type"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sndcp::ip::{bitbuffer_npdu_octets, build_ipv4_udp_npdu, parse_ipv4_packet, parse_udp_datagram};
    use crate::sndcp::pdch::SndcpPacketDataResourceRequest;
    use crate::sndcp::pdp::{
        SndcpActivateAddressDemand, SndcpActivatePdpContextDemand, SndcpActivationRejectCause, SndcpDeactivation,
        SndcpMaximumTransmissionUnit, decode_activate_pdp_context_accept, decode_activate_pdp_context_reject,
        encode_activate_pdp_context_demand, encode_deactivate_pdp_context_demand,
    };
    use crate::sndcp::state::SwmiSndcpState;
    use crate::sndcp::transfer::{
        SndcpDataTransmitRequest, SndcpDataTransmitResponseResult, SndcpEndOfData, SndcpReconnect, decode_data_transmit_response,
        decode_end_of_data, encode_data_transmit_request, encode_end_of_data, encode_reconnect,
    };
    use crate::sndcp::unitdata::{decode_sn_unitdata_pdu, encode_sn_data, encode_sn_unitdata};
    use tetra_saps::sn::{SnAddress, SnPacketDataMsType};

    const ISSI: u32 = 2_260_618;
    const HANDLE: MleHandle = 88;

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
            active_group_calls: 1,
            active_private_calls: 0,
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

    fn session() -> SndcpWapSession {
        SndcpWapSession::new(SndcpPdpPolicy::experimental_wap_ipv4(), endpoint(), wap_policy())
    }

    fn data_transmit_request(nsapi: u8) -> BitBuffer {
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi,
            logical_link_status: false,
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode")
    }

    fn reconnect(nsapi: u8) -> BitBuffer {
        encode_reconnect(&SndcpReconnect {
            nsapi: Some(nsapi),
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .expect("SN-RECONNECT should encode")
    }

    fn reconnect_without_data() -> BitBuffer {
        encode_reconnect(&SndcpReconnect {
            nsapi: None,
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .expect("SN-RECONNECT without data should encode")
    }

    fn activate_context(session: &mut SndcpWapSession, nsapi: u8) {
        session
            .handle_inbound_pdu(ISSI, HANDLE, &dynamic_ipv4_demand(nsapi), &snapshot())
            .expect("activation should produce accept PDU");
        assert_eq!(session.state_for_issi(ISSI), SwmiSndcpState::Standby);
    }

    fn enter_ready(session: &mut SndcpWapSession, nsapi: u8) {
        let response = session
            .handle_inbound_pdu(ISSI, HANDLE, &data_transmit_request(nsapi), &snapshot())
            .expect("SN-DATA TRANSMIT REQUEST should produce response");
        let response = decode_data_transmit_response(&response).expect("SN-DATA TRANSMIT RESPONSE should decode");
        assert_eq!(response.nsapi, nsapi);
        assert_eq!(response.result, SndcpDataTransmitResponseResult::Accepted);
        assert_eq!(session.state_for_issi(ISSI), SwmiSndcpState::Ready);
    }

    #[test]
    fn dynamic_pdp_activation_then_wap_unitdata_returns_status_pdu() {
        let mut session = session();
        let accept_pdu = session
            .handle_inbound_pdu(ISSI, HANDLE, &dynamic_ipv4_demand(2), &snapshot())
            .expect("activation should produce accept PDU");
        let accept = decode_activate_pdp_context_accept(&accept_pdu).expect("activation accept should decode");
        assert_eq!(accept.assigned_address, Some(SnAddress::Ipv4([10, 0, 0, 2])));
        assert_eq!(accept.maximum_transmission_unit, SndcpMaximumTransmissionUnit::Octets576);
        assert_eq!(session.state_for_issi(ISSI), SwmiSndcpState::Standby);
        enter_ready(&mut session, 2);

        let request_npdu = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        let unitdata_pdu = encode_sn_unitdata(2, 0, 0, &BitBuffer::from_bytes(&request_npdu)).expect("SN-UNITDATA should encode");

        let response_pdu = session
            .handle_inbound_pdu(ISSI, HANDLE, &unitdata_pdu, &snapshot())
            .expect("active PDP context should serve WAP status");
        let response_unitdata = decode_sn_unitdata_pdu(&response_pdu).expect("response SN-UNITDATA should decode");
        assert_eq!(response_unitdata.nsapi, 2);

        let response_npdu = bitbuffer_npdu_octets(&response_unitdata.n_pdu).expect("response N-PDU should be byte aligned");
        let response_ip = parse_ipv4_packet(&response_npdu).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");

        assert_eq!(response_ip.source, endpoint().address);
        assert_eq!(response_ip.destination, [10, 0, 0, 2]);
        assert_eq!(response_udp.source_port, endpoint().port);
        assert_eq!(response_udp.destination_port, 49_152);
        assert!(std::str::from_utf8(response_udp.payload).unwrap().contains("Nexus-BS"));
    }

    #[test]
    fn dynamic_pdp_ready_then_wap_sn_data_returns_status_pdu() {
        let mut session = session();
        activate_context(&mut session, 2);
        enter_ready(&mut session, 2);

        let request_npdu = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        let data_pdu = encode_sn_data(2, 0, 0, &BitBuffer::from_bytes(&request_npdu)).expect("SN-DATA should encode");

        let response_pdu = session
            .handle_inbound_pdu(ISSI, HANDLE, &data_pdu, &snapshot())
            .expect("active PDP context should serve WAP status from SN-DATA");
        let response_unitdata = decode_sn_unitdata_pdu(&response_pdu).expect("response SN-UNITDATA should decode");
        assert_eq!(response_unitdata.nsapi, 2);

        let response_npdu = bitbuffer_npdu_octets(&response_unitdata.n_pdu).expect("response N-PDU should be byte aligned");
        let response_ip = parse_ipv4_packet(&response_npdu).expect("response IPv4 should parse");
        assert_eq!(response_ip.source, endpoint().address);
        assert_eq!(response_ip.destination, [10, 0, 0, 2]);
    }

    #[test]
    fn data_transmit_request_after_activation_enters_ready() {
        let mut session = session();
        activate_context(&mut session, 2);

        enter_ready(&mut session, 2);
    }

    #[test]
    fn data_transmit_request_without_local_context_recovers_wap_ipv4_context_and_enters_ready() {
        let mut session = session();

        let response = session
            .handle_inbound_pdu(ISSI, HANDLE, &data_transmit_request(1), &snapshot())
            .expect("WAP/IP recovery should accept MS data-transfer request after local PDP loss");
        let response = decode_data_transmit_response(&response).expect("SN-DATA TRANSMIT RESPONSE should decode");

        assert_eq!(response.nsapi, 1);
        assert_eq!(response.result, SndcpDataTransmitResponseResult::Accepted);
        assert_eq!(session.state_for_issi(ISSI), SwmiSndcpState::Ready);
        let context = session
            .pdp()
            .contexts()
            .get_issi_nsapi(ISSI, 1)
            .expect("recovered NSAPI should be valid")
            .expect("missing WAP/IP context should be rebuilt for the ISSI");
        assert_eq!(context.address, SnAddress::Ipv4([10, 0, 0, 2]));
    }

    #[test]
    fn reconnect_with_data_without_local_context_recovers_wap_ipv4_context_and_enters_ready() {
        let mut session = session();

        let response = session
            .handle_inbound_pdu(ISSI, HANDLE, &reconnect(1), &snapshot())
            .expect("WAP/IP recovery should accept SN-RECONNECT with data after local PDP loss");
        let response = decode_data_transmit_response(&response).expect("SN-DATA TRANSMIT RESPONSE should decode");

        assert_eq!(response.nsapi, 1);
        assert_eq!(response.result, SndcpDataTransmitResponseResult::Accepted);
        assert_eq!(session.state_for_issi(ISSI), SwmiSndcpState::Ready);
    }

    #[test]
    fn reconnect_with_data_to_send_after_ready_returns_data_transmit_response() {
        let mut session = session();
        activate_context(&mut session, 2);
        enter_ready(&mut session, 2);

        let response = session
            .handle_inbound_pdu(ISSI, HANDLE, &reconnect(2), &snapshot())
            .expect("SN-RECONNECT with data to send should produce response");
        let response = decode_data_transmit_response(&response).expect("SN-DATA TRANSMIT RESPONSE should decode");

        assert_eq!(response.nsapi, 2);
        assert_eq!(response.result, SndcpDataTransmitResponseResult::Accepted);
        assert_eq!(session.state_for_issi(ISSI), SwmiSndcpState::Ready);
    }

    #[test]
    fn end_of_data_after_ready_is_valid_and_returns_standby() {
        let mut session = session();
        activate_context(&mut session, 2);
        enter_ready(&mut session, 2);
        let end_of_data = encode_end_of_data(&SndcpEndOfData {
            immediate_service_change: false,
        })
        .expect("SN-END OF DATA should encode");

        let response = session
            .handle_inbound_pdu(ISSI, HANDLE, &end_of_data, &snapshot())
            .expect("SN-END OF DATA should be handled as a valid transfer-control PDU");
        let response = decode_end_of_data(&response).expect("SwMI SN-END OF DATA echo should decode");

        assert!(!response.immediate_service_change);
        assert_eq!(session.state_for_issi(ISSI), SwmiSndcpState::Standby);
    }

    #[test]
    fn reconnect_without_data_after_end_of_data_is_no_response_in_standby() {
        let mut session = session();
        activate_context(&mut session, 2);
        enter_ready(&mut session, 2);
        let end_of_data = encode_end_of_data(&SndcpEndOfData {
            immediate_service_change: false,
        })
        .expect("SN-END OF DATA should encode");
        session
            .handle_inbound_pdu(ISSI, HANDLE, &end_of_data, &snapshot())
            .expect("SN-END OF DATA should return to STANDBY");

        let response = session
            .handle_inbound_pdu_response(ISSI, HANDLE, &reconnect_without_data(), &snapshot())
            .expect("SN-RECONNECT without data should be accepted in STANDBY");

        assert!(matches!(response, SndcpWapSessionResponse::NoResponse));
        assert_eq!(session.state_for_issi(ISSI), SwmiSndcpState::Standby);
    }

    #[test]
    fn unitdata_after_activation_before_ready_is_rejected_fail_closed() {
        let mut session = session();
        activate_context(&mut session, 2);

        let request_npdu = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        let unitdata_pdu = encode_sn_unitdata(2, 0, 0, &BitBuffer::from_bytes(&request_npdu)).expect("SN-UNITDATA should encode");

        let error = session
            .handle_unitdata(ISSI, HANDLE, &unitdata_pdu, &snapshot())
            .expect_err("READY state is required before WAP SN-UNITDATA response");

        assert_eq!(
            error,
            SndcpWapSessionError::Bearer(SndcpBearerError::PacketDataTransferNotReady {
                issi: ISSI,
                state: SwmiSndcpState::Standby
            })
        );
    }

    #[test]
    fn unsupported_activation_returns_encoded_reject_without_context() {
        let mut session = session();
        let demand = encode_activate_pdp_context_demand(&SndcpActivatePdpContextDemand {
            sndcp_version: 1,
            nsapi: 3,
            address: SndcpActivateAddressDemand::Ipv6,
            packet_data_ms_type: SnPacketDataMsType::TypeAParallel,
            pcomp_negotiation: 0,
        })
        .expect("IPv6 activation demand should encode");

        let reject_pdu = session
            .handle_inbound_pdu(ISSI, HANDLE, &demand, &snapshot())
            .expect("unsupported activation should produce reject PDU");
        let reject = decode_activate_pdp_context_reject(&reject_pdu).expect("activation reject should decode");

        assert_eq!(reject.nsapi, 3);
        assert_eq!(reject.cause, SndcpActivationRejectCause::Ipv6NotSupported);
        assert!(session.pdp().contexts().get_issi_nsapi(ISSI, 3).unwrap().is_none());
    }

    #[test]
    fn unitdata_before_activation_is_rejected_fail_closed() {
        let mut session = session();
        let request_npdu = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        let unitdata_pdu = encode_sn_unitdata(2, 0, 0, &BitBuffer::from_bytes(&request_npdu)).expect("SN-UNITDATA should encode");

        let error = session
            .handle_unitdata(ISSI, HANDLE, &unitdata_pdu, &snapshot())
            .expect_err("missing PDP context should reject WAP response");

        assert!(matches!(error, SndcpWapSessionError::Wap(WapGatewayError::MissingContext(_))));
    }

    #[test]
    fn wap_status_policy_must_be_enabled_after_pdp_activation() {
        let mut session = SndcpWapSession::new(SndcpPdpPolicy::experimental_wap_ipv4(), endpoint(), WapIpServicePolicy::default());
        activate_context(&mut session, 2);
        enter_ready(&mut session, 2);

        let request_npdu = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        let unitdata_pdu = encode_sn_unitdata(2, 0, 0, &BitBuffer::from_bytes(&request_npdu)).expect("SN-UNITDATA should encode");

        let error = session
            .handle_unitdata(ISSI, HANDLE, &unitdata_pdu, &snapshot())
            .expect_err("WAP status endpoint should remain disabled by default");

        assert!(matches!(
            error,
            SndcpWapSessionError::Wap(WapGatewayError::Wap(crate::sndcp::wap_ip::WapIpError::StatusServiceDisabled))
        ));
    }

    #[test]
    fn deactivation_removes_context_before_later_unitdata() {
        let mut session = session();
        activate_context(&mut session, 2);

        let deactivation = encode_deactivate_pdp_context_demand(&SndcpDeactivation::Nsapi(2)).expect("deactivation demand should encode");
        let deactivation_accept = session
            .handle_inbound_pdu(ISSI, HANDLE, &deactivation, &snapshot())
            .expect("deactivation should produce accept PDU");
        assert!(deactivation_accept.get_len() > 0);
        assert!(session.pdp().contexts().get_issi_nsapi(ISSI, 2).unwrap().is_none());

        let request_npdu = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        let unitdata_pdu = encode_sn_unitdata(2, 0, 0, &BitBuffer::from_bytes(&request_npdu)).expect("SN-UNITDATA should encode");

        let error = session
            .handle_unitdata(ISSI, HANDLE, &unitdata_pdu, &snapshot())
            .expect_err("deactivated PDP context should reject WAP response");

        assert!(matches!(error, SndcpWapSessionError::Wap(WapGatewayError::MissingContext(_))));
    }

    #[test]
    fn unsupported_inbound_pdu_type_is_rejected_without_mutation() {
        let mut session = session();
        let mut pdu = BitBuffer::new(4);
        pdu.write_bits(12, 4);
        pdu.seek(0);

        assert_eq!(
            session
                .handle_inbound_pdu(ISSI, HANDLE, &pdu, &snapshot())
                .expect_err("unsupported inbound PDU type should reject"),
            SndcpWapSessionError::UnsupportedInboundPduType(12)
        );
        assert_eq!(session.pdp().contexts().contexts_for_issi(ISSI).count(), 0);
        assert_eq!(session.state_for_issi(ISSI), SwmiSndcpState::Idle);
    }
}
