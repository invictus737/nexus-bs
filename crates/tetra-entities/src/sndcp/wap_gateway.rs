// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original TETRA SNDCP WAP/IP status gateway primitive.

use super::bearer_policy::SndcpPacketDataBearerProfile;
use super::context::{SndcpContextKey, SndcpContextTable};
use super::ip::{IpPrimitiveError, bitbuffer_npdu_octets, parse_ipv4_packet};
use super::priority::SndcpDataScheduling;
use super::wap_ip::{WapIpEndpoint, WapIpError, WapIpServicePolicy, build_wap_status_response_npdu_optional_with_npdu_budget};
use super::wap_status::{WapStatusError, WapStatusSnapshot};
use tetra_core::MleHandle;
use tetra_saps::sn::{SnAddress, SnPdpType, SnPrimitiveError, SnUnitdataInd, SnUnitdataReq, sn_unitdata_req};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WapGatewayError {
    ReservedNsapi(u8),
    IssiNotAllowed(u32),
    MissingContext(SndcpContextKey),
    UnsupportedPdpType { pdp_type: SnPdpType },
    ContextAddressNotIpv4 { address: SnAddress },
    SourceAddressMismatch { expected: [u8; 4], actual: [u8; 4] },
    FragmentedIpv4Unsupported { flags_fragment: u16 },
    Ip(IpPrimitiveError),
    Wap(WapIpError),
    ResponseNpduTooLarge { len: usize, max: u16 },
    MissingPduPriorityMax(SndcpContextKey),
    NoResponseRequired,
    Sn(SnPrimitiveError),
}

impl From<IpPrimitiveError> for WapGatewayError {
    fn from(value: IpPrimitiveError) -> Self {
        WapGatewayError::Ip(value)
    }
}

impl From<WapIpError> for WapGatewayError {
    fn from(value: WapIpError) -> Self {
        WapGatewayError::Wap(value)
    }
}

impl From<SnPrimitiveError> for WapGatewayError {
    fn from(value: SnPrimitiveError) -> Self {
        WapGatewayError::Sn(value)
    }
}

#[derive(Debug, Clone)]
pub struct WapStatusUnitdataResponse {
    pub unitdata: SnUnitdataReq,
    pub pdu_priority_max: u8,
    pub nsapi_data_priority: Option<u8>,
    pub ms_default_data_priority: Option<u8>,
    pub scheduling: SndcpDataScheduling,
    pub bearer_profile: SndcpPacketDataBearerProfile,
}

pub fn build_wap_status_unitdata_response(
    contexts: &SndcpContextTable,
    issi: u32,
    handle: MleHandle,
    unitdata: &SnUnitdataInd,
    endpoint: WapIpEndpoint,
    policy: &WapIpServicePolicy,
    snapshot: &WapStatusSnapshot,
) -> Result<WapStatusUnitdataResponse, WapGatewayError> {
    build_wap_status_unitdata_response_optional(contexts, issi, handle, unitdata, endpoint, policy, snapshot)?
        .ok_or(WapGatewayError::NoResponseRequired)
}

pub fn build_wap_status_unitdata_response_optional(
    contexts: &SndcpContextTable,
    issi: u32,
    handle: MleHandle,
    unitdata: &SnUnitdataInd,
    endpoint: WapIpEndpoint,
    policy: &WapIpServicePolicy,
    snapshot: &WapStatusSnapshot,
) -> Result<Option<WapStatusUnitdataResponse>, WapGatewayError> {
    // EN 300 392-2 clause 28 maps each user N-PDU to an active PDP context
    // selected by NSAPI. Keep this as a pure SN-SAP primitive until the SNDCP
    // bearer state machine and MLE handoff are fully implemented.
    let key = SndcpContextKey::new(issi, unitdata.nsapi).map_err(|_| WapGatewayError::ReservedNsapi(unitdata.nsapi))?;
    if !policy.allows_issi(issi) {
        return Err(WapGatewayError::IssiNotAllowed(issi));
    }
    let context = contexts.get(key).ok_or(WapGatewayError::MissingContext(key))?;
    let pdu_priority_max = context.pdu_priority.ok_or(WapGatewayError::MissingPduPriorityMax(key))?;

    if context.pdp_type != SnPdpType::Ipv4 {
        return Err(WapGatewayError::UnsupportedPdpType {
            pdp_type: context.pdp_type,
        });
    }

    let expected_source = match context.address {
        SnAddress::Ipv4(address) => address,
        address => return Err(WapGatewayError::ContextAddressNotIpv4 { address }),
    };

    let request_npdu = bitbuffer_npdu_octets(&unitdata.n_pdu)?;
    let request_ip = parse_ipv4_packet(&request_npdu)?;
    if is_fragmented_ipv4(request_ip.flags_fragment) {
        return Err(WapGatewayError::FragmentedIpv4Unsupported {
            flags_fragment: request_ip.flags_fragment,
        });
    }
    if request_ip.source != expected_source {
        return Err(WapGatewayError::SourceAddressMismatch {
            expected: expected_source,
            actual: request_ip.source,
        });
    }

    let response_npdu = match build_wap_status_response_npdu_optional_with_npdu_budget(
        &request_npdu,
        endpoint,
        policy,
        snapshot,
        context.max_npdu_len.map(usize::from),
    ) {
        Ok(Some(response_npdu)) => response_npdu,
        Ok(None) => return Ok(None),
        Err(WapIpError::Status(WapStatusError::RenderedTooLarge { len, .. })) => {
            let max = context.max_npdu_len.unwrap_or(u16::MAX);
            return Err(WapGatewayError::ResponseNpduTooLarge { len, max });
        }
        Err(err) => return Err(WapGatewayError::Wap(err)),
    };
    if let Some(max) = context.max_npdu_len {
        if response_npdu.len() > max as usize {
            return Err(WapGatewayError::ResponseNpduTooLarge {
                len: response_npdu.len(),
                max,
            });
        }
    }

    Ok(Some(WapStatusUnitdataResponse {
        unitdata: sn_unitdata_req(
            unitdata.nsapi,
            handle,
            tetra_core::BitBuffer::from_bytes(&response_npdu),
            None,
            None,
        )?,
        pdu_priority_max,
        nsapi_data_priority: context.data_priority,
        ms_default_data_priority: None,
        scheduling: SndcpDataScheduling::NonScheduled,
        bearer_profile: context.bearer_profile,
    }))
}

fn is_fragmented_ipv4(flags_fragment: u16) -> bool {
    const IPV4_MORE_FRAGMENTS: u16 = 0x2000;
    const IPV4_FRAGMENT_OFFSET_MASK: u16 = 0x1fff;

    flags_fragment & (IPV4_MORE_FRAGMENTS | IPV4_FRAGMENT_OFFSET_MASK) != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sndcp::context::SndcpPdpContext;
    use crate::sndcp::ip::{build_ipv4_udp_npdu, parse_udp_datagram};
    use tetra_saps::sn::{SnPacketDataMsType, sn_unitdata_ind};

    const ISSI: u32 = 2_260_618;
    const NSAPI: u8 = 2;
    const HANDLE: MleHandle = 55;
    const MS_IP: [u8; 4] = [10, 0, 0, 18];

    fn endpoint() -> WapIpEndpoint {
        WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        }
    }

    fn policy() -> WapIpServicePolicy {
        WapIpServicePolicy::experimental_status()
    }

    fn snapshot() -> WapStatusSnapshot {
        WapStatusSnapshot {
            title: "Nexus-BS".to_string(),
            stack_version: "v0.1.69_dev-test".to_string(),
            service_state: "ON AIR".to_string(),
            registered_ms: 3,
            active_calls: 1,
            queued_sds: 2,
            uptime_secs: 61,
            last_activity: None,
            health_summary: Some("OK".to_string()),
            health_lines: vec!["CORE OK".to_string(), "RF OK".to_string(), "SDS OK".to_string()],
            radio_lines: vec!["MS 2260618 -47dB G1 SA".to_string()],
            call_lines: vec!["G91 S2260618 TS2".to_string()],
        }
    }

    fn detailed_snapshot() -> WapStatusSnapshot {
        WapStatusSnapshot {
            title: "Nexus-BS WAP &&&&&&&&&&&&".to_string(),
            stack_version: "v0.1.69_dev-with-long-build-id".to_string(),
            service_state: "DEGRADED &&&&&&".to_string(),
            registered_ms: 6,
            active_calls: 2,
            queued_sds: 4,
            uptime_secs: 3661,
            last_activity: Some("SDS 2260082>2260618 &&&&&&".to_string()),
            health_summary: Some("DEGRADED C0 D2".to_string()),
            health_lines: vec![
                "CORE OK".to_string(),
                "RF WARN".to_string(),
                "VOICE OK".to_string(),
                "P2P OK".to_string(),
                "SDS BAD".to_string(),
            ],
            radio_lines: vec![
                "MS 2260082 -52dB G1 EG3 &&&&&&".to_string(),
                "MS 2260616 -41dB G1 EG1 &&&&&&".to_string(),
                "MS 2260618 -47dB G1 SA &&&&&&".to_string(),
            ],
            call_lines: vec![
                "G91 S2260616 TS4 &&&&&&".to_string(),
                "P2P-D 2260618>2260082 TS2/3 &&&&&&".to_string(),
            ],
        }
    }

    fn contexts(max_npdu_len: Option<u16>) -> SndcpContextTable {
        let mut contexts = SndcpContextTable::default();
        let context = SndcpPdpContext::primary_ipv4(ISSI, NSAPI, SnAddress::Ipv4(MS_IP), SnPacketDataMsType::TypeAParallel)
            .expect("test context should be valid")
            .with_qos(Some(3), Some(1), max_npdu_len);
        contexts.activate(context).expect("test context should activate");
        contexts
    }

    fn unitdata_from_request(source: [u8; 4], destination_port: u16) -> SnUnitdataInd {
        let request = build_ipv4_udp_npdu(source, endpoint().address, 49_152, destination_port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        sn_unitdata_ind(NSAPI, tetra_core::BitBuffer::from_bytes(&request)).expect("SN-UNITDATA indication should build")
    }

    #[test]
    fn active_ipv4_context_generates_status_unitdata_response() {
        let contexts = contexts(Some(576));
        let unitdata = unitdata_from_request(MS_IP, endpoint().port);

        let response = build_wap_status_unitdata_response(&contexts, ISSI, HANDLE, &unitdata, endpoint(), &policy(), &snapshot())
            .expect("active WAP context should produce response SN-UNITDATA");

        assert_eq!(response.unitdata.nsapi, NSAPI);
        assert_eq!(response.unitdata.handle, HANDLE);
        assert_eq!(response.unitdata.pdu_priority, None);
        assert_eq!(response.unitdata.data_priority, None);
        assert_eq!(response.pdu_priority_max, 3);
        assert_eq!(response.nsapi_data_priority, Some(1));
        assert_eq!(response.ms_default_data_priority, None);
        assert_eq!(response.scheduling, SndcpDataScheduling::NonScheduled);
        assert_eq!(response.bearer_profile, SndcpPacketDataBearerProfile::default());

        let response_octets = bitbuffer_npdu_octets(&response.unitdata.n_pdu).expect("response N-PDU should be byte aligned");
        let response_ip = parse_ipv4_packet(&response_octets).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");

        assert_eq!(response_ip.source, endpoint().address);
        assert_eq!(response_ip.destination, MS_IP);
        assert_eq!(response_udp.source_port, endpoint().port);
        assert_eq!(response_udp.destination_port, 49_152);
        assert!(std::str::from_utf8(response_udp.payload).unwrap().contains("Nexus-BS"));
    }

    #[test]
    fn detailed_status_compacts_to_negotiated_576_byte_npdu() {
        let contexts = contexts(Some(576));
        let unitdata = unitdata_from_request(MS_IP, endpoint().port);

        let response = build_wap_status_unitdata_response(&contexts, ISSI, HANDLE, &unitdata, endpoint(), &policy(), &detailed_snapshot())
            .expect("detailed dashboard snapshot should compact under negotiated N-PDU");
        let response_octets = bitbuffer_npdu_octets(&response.unitdata.n_pdu).expect("response N-PDU should be byte aligned");

        assert!(response_octets.len() <= 576);
        let response_ip = parse_ipv4_packet(&response_octets).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");
        let page = std::str::from_utf8(response_udp.payload).expect("WAP status page should be UTF-8");
        assert!(page.contains("http://www.w3.org/1999/xhtml"));
        assert!(page.contains("Nexus-BS"));
        assert!(
            page.len() > 128,
            "negotiated 576-byte N-PDU should not collapse to the legacy 128-byte tiny page"
        );
    }

    #[test]
    fn missing_context_is_rejected_before_wap_response() {
        let contexts = SndcpContextTable::default();
        let unitdata = unitdata_from_request(MS_IP, endpoint().port);
        let key = SndcpContextKey::new(ISSI, NSAPI).unwrap();

        let error = build_wap_status_unitdata_response(&contexts, ISSI, HANDLE, &unitdata, endpoint(), &policy(), &snapshot())
            .expect_err("missing context should be rejected");

        assert_eq!(error, WapGatewayError::MissingContext(key));
    }

    #[test]
    fn status_policy_can_restrict_issi_access_before_wap_response() {
        let contexts = contexts(Some(576));
        let unitdata = unitdata_from_request(MS_IP, endpoint().port);
        let policy = WapIpServicePolicy::experimental_status_for_issis(vec![2_260_082]);

        let error = build_wap_status_unitdata_response(&contexts, ISSI, HANDLE, &unitdata, endpoint(), &policy, &snapshot())
            .expect_err("ISSI outside allowlist should reject WAP status");

        assert_eq!(error, WapGatewayError::IssiNotAllowed(ISSI));
    }

    #[test]
    fn request_source_must_match_active_context_ipv4_address() {
        let contexts = contexts(Some(576));
        let unitdata = unitdata_from_request([10, 0, 0, 99], endpoint().port);

        let error = build_wap_status_unitdata_response(&contexts, ISSI, HANDLE, &unitdata, endpoint(), &policy(), &snapshot())
            .expect_err("source IP mismatch should be rejected");

        assert_eq!(
            error,
            WapGatewayError::SourceAddressMismatch {
                expected: MS_IP,
                actual: [10, 0, 0, 99]
            }
        );
    }

    #[test]
    fn fragmented_ipv4_is_rejected_until_reassembly_exists() {
        let contexts = contexts(Some(576));
        let mut request = build_ipv4_udp_npdu(MS_IP, endpoint().address, 49_152, endpoint().port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");
        request[6..8].copy_from_slice(&0x2000u16.to_be_bytes());
        let unitdata = sn_unitdata_ind(NSAPI, tetra_core::BitBuffer::from_bytes(&request)).expect("SN-UNITDATA indication should build");

        let error = build_wap_status_unitdata_response(&contexts, ISSI, HANDLE, &unitdata, endpoint(), &policy(), &snapshot())
            .expect_err("fragmented IPv4 should be rejected");

        assert_eq!(error, WapGatewayError::FragmentedIpv4Unsupported { flags_fragment: 0x2000 });
    }

    #[test]
    fn response_must_fit_negotiated_context_mtu() {
        let contexts = contexts(Some(32));
        let unitdata = unitdata_from_request(MS_IP, endpoint().port);

        let error = build_wap_status_unitdata_response(&contexts, ISSI, HANDLE, &unitdata, endpoint(), &policy(), &snapshot())
            .expect_err("tiny negotiated MTU should reject response");

        assert!(matches!(error, WapGatewayError::ResponseNpduTooLarge { max: 32, .. }));
    }

    #[test]
    fn wap_destination_errors_are_preserved() {
        let contexts = contexts(Some(576));
        let unitdata = unitdata_from_request(MS_IP, 9201);

        let error = build_wap_status_unitdata_response(&contexts, ISSI, HANDLE, &unitdata, endpoint(), &policy(), &snapshot())
            .expect_err("wrong WAP UDP port should be preserved");

        assert_eq!(
            error,
            WapGatewayError::Wap(WapIpError::WrongUdpPort {
                expected: endpoint().port,
                actual: 9201
            })
        );
    }
}
