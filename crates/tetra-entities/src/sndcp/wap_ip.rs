// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original WAP-over-UDP/IP adapter primitives for TETRA SNDCP experiments.

use super::ip::{IPV4_PROTOCOL_UDP, IpPrimitiveError, build_ipv4_udp_npdu, parse_ipv4_packet, parse_udp_datagram};
use super::wap_status::{
    DEFAULT_WAP_STATUS_MAX_BYTES, WAP_STATUS_HTML_PATH, WAP_STATUS_LEGACY_WML_PATH, WAP_STATUS_REFRESH_PATH, WapStatusError,
    WapStatusSnapshot, render_wml2_status,
};

pub const DEFAULT_WAP_UDP_REQUEST_MAX_BYTES: usize = 128;
pub const IPV4_UDP_HEADER_BYTES: usize = 28;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WapIpEndpoint {
    pub address: [u8; 4],
    pub port: u16,
    pub response_ttl: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WapIpServicePolicy {
    pub status_enabled: bool,
    pub accept_empty_probe: bool,
    pub accept_root_path: bool,
    pub accept_status_path: bool,
    pub accept_status_wml_path: bool,
    pub max_request_payload_bytes: usize,
    pub allowed_issis: Option<Vec<u32>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WapUdpRequestKind {
    Empty,
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WapIpError {
    Ip(IpPrimitiveError),
    Status(WapStatusError),
    UnsupportedIpProtocol { protocol: u8 },
    WrongDestination { expected: [u8; 4], actual: [u8; 4] },
    WrongUdpPort { expected: u16, actual: u16 },
    StatusServiceDisabled,
    EmptyProbeDisabled,
    UdpPayloadTooLarge { len: usize, max: usize },
    UnsupportedWapUdpPayload { len: usize },
    UnsupportedWapPath { path: String },
}

impl From<IpPrimitiveError> for WapIpError {
    fn from(value: IpPrimitiveError) -> Self {
        WapIpError::Ip(value)
    }
}

impl From<WapStatusError> for WapIpError {
    fn from(value: WapStatusError) -> Self {
        WapIpError::Status(value)
    }
}

impl Default for WapIpServicePolicy {
    fn default() -> Self {
        Self {
            status_enabled: false,
            accept_empty_probe: false,
            accept_root_path: false,
            accept_status_path: false,
            accept_status_wml_path: false,
            max_request_payload_bytes: DEFAULT_WAP_UDP_REQUEST_MAX_BYTES,
            allowed_issis: None,
        }
    }
}

impl WapIpServicePolicy {
    pub fn experimental_status() -> Self {
        Self {
            status_enabled: true,
            accept_empty_probe: true,
            accept_root_path: true,
            accept_status_path: true,
            accept_status_wml_path: true,
            ..Self::default()
        }
    }

    pub fn experimental_status_for_issis(allowed_issis: Vec<u32>) -> Self {
        Self {
            allowed_issis: Some(allowed_issis),
            ..Self::experimental_status()
        }
    }

    pub fn allows_issi(&self, issi: u32) -> bool {
        self.allowed_issis.as_ref().map(|allowed| allowed.contains(&issi)).unwrap_or(true)
    }
}

pub fn build_wap_status_response_npdu(
    request_npdu: &[u8],
    endpoint: WapIpEndpoint,
    policy: &WapIpServicePolicy,
    snapshot: &WapStatusSnapshot,
) -> Result<Vec<u8>, WapIpError> {
    build_wap_status_response_npdu_with_wml2_budget(request_npdu, endpoint, policy, snapshot, DEFAULT_WAP_STATUS_MAX_BYTES)
}

pub fn build_wap_status_response_npdu_with_wml2_budget(
    request_npdu: &[u8],
    endpoint: WapIpEndpoint,
    policy: &WapIpServicePolicy,
    snapshot: &WapStatusSnapshot,
    max_wml2_bytes: usize,
) -> Result<Vec<u8>, WapIpError> {
    let request_ip = parse_ipv4_packet(request_npdu)?;
    if request_ip.protocol != IPV4_PROTOCOL_UDP {
        return Err(WapIpError::UnsupportedIpProtocol {
            protocol: request_ip.protocol,
        });
    }
    if request_ip.destination != endpoint.address {
        return Err(WapIpError::WrongDestination {
            expected: endpoint.address,
            actual: request_ip.destination,
        });
    }

    let request_udp = parse_udp_datagram(request_ip.payload)?;
    if request_udp.destination_port != endpoint.port {
        return Err(WapIpError::WrongUdpPort {
            expected: endpoint.port,
            actual: request_udp.destination_port,
        });
    }
    let _request_kind = parse_wap_udp_request(request_udp.payload, policy)?;

    let page = render_wml2_status(snapshot, max_wml2_bytes)?;
    let response = build_ipv4_udp_npdu(
        endpoint.address,
        request_ip.source,
        endpoint.port,
        request_udp.source_port,
        page.as_bytes(),
        request_ip.identification.wrapping_add(1),
        endpoint.response_ttl,
    )?;

    Ok(response)
}

pub fn parse_wap_udp_request(payload: &[u8], policy: &WapIpServicePolicy) -> Result<WapUdpRequestKind, WapIpError> {
    // ETSI TS 100 392-2 clause 29.5.8 delegates WAP protocol details to WAP
    // 2.0. This lab primitive intentionally admits only explicit diagnostic
    // status probes until a full WAP/WSP profile is implemented.
    if !policy.status_enabled {
        return Err(WapIpError::StatusServiceDisabled);
    }
    if payload.len() > policy.max_request_payload_bytes {
        return Err(WapIpError::UdpPayloadTooLarge {
            len: payload.len(),
            max: policy.max_request_payload_bytes,
        });
    }
    if payload.is_empty() {
        if policy.accept_empty_probe {
            return Ok(WapUdpRequestKind::Empty);
        }
        return Err(WapIpError::EmptyProbeDisabled);
    }

    let Ok(text) = std::str::from_utf8(payload) else {
        return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
    };
    let text = text.trim();

    if let Some(path) = text.strip_prefix("GET ") {
        let path = path.split_whitespace().next().unwrap_or("/");
        let path = normalize_get_path(path);
        if is_status_path(&path, policy) {
            return Ok(WapUdpRequestKind::Status);
        }
        return Err(WapIpError::UnsupportedWapPath { path });
    }

    Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() })
}

fn normalize_get_path(path: &str) -> String {
    let path = path.trim();
    let path = path.split(['?', '#']).next().unwrap_or(path).trim();
    if path.is_empty() { "/".to_string() } else { path.to_string() }
}

fn is_status_path(path: &str, policy: &WapIpServicePolicy) -> bool {
    match path {
        "/" => policy.accept_root_path,
        "/status" => policy.accept_status_path,
        WAP_STATUS_REFRESH_PATH | WAP_STATUS_HTML_PATH => policy.accept_status_path,
        WAP_STATUS_LEGACY_WML_PATH => policy.accept_status_wml_path,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sndcp::ip::{parse_ipv4_packet, parse_udp_datagram};

    fn snapshot() -> WapStatusSnapshot {
        WapStatusSnapshot {
            title: "Nexus-BS".to_string(),
            stack_version: "v0.1.68_dev-test".to_string(),
            service_state: "ON AIR".to_string(),
            registered_ms: 4,
            active_calls: 0,
            queued_sds: 1,
            uptime_secs: 125,
            last_activity: None,
            health_summary: Some("OK".to_string()),
            health_lines: vec!["CORE OK".to_string(), "RF OK".to_string(), "SDS OK".to_string()],
            radio_lines: vec!["MS 2260618 -47dB G1 SA".to_string()],
            call_lines: Vec::new(),
        }
    }

    fn policy() -> WapIpServicePolicy {
        WapIpServicePolicy::experimental_status()
    }

    #[test]
    fn wap_udp_request_classifier_accepts_only_mvp_safe_requests() {
        assert_eq!(parse_wap_udp_request(b"", &policy()), Ok(WapUdpRequestKind::Empty));
        assert_eq!(
            parse_wap_udp_request(b"GET / HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status)
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /status HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status)
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /status.xhtml?refresh=1 HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status)
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /status.html HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status)
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /status.wml?refresh=1 HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status)
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /admin HTTP/1.0\r\n\r\n", &policy()),
            Err(WapIpError::UnsupportedWapPath {
                path: "/admin".to_string()
            })
        );
        assert_eq!(
            parse_wap_udp_request(&[0x01, 0x40, 0x00], &policy()),
            Err(WapIpError::UnsupportedWapUdpPayload { len: 3 })
        );
        assert_eq!(
            parse_wap_udp_request(b"POST /", &policy()),
            Err(WapIpError::UnsupportedWapUdpPayload { len: 6 })
        );
    }

    #[test]
    fn wap_udp_request_policy_is_default_deny_and_path_scoped() {
        assert_eq!(
            parse_wap_udp_request(b"GET /status.wml HTTP/1.0\r\n\r\n", &WapIpServicePolicy::default()),
            Err(WapIpError::StatusServiceDisabled)
        );

        let status_wml_only = WapIpServicePolicy {
            status_enabled: true,
            accept_status_wml_path: true,
            ..WapIpServicePolicy::default()
        };
        assert_eq!(parse_wap_udp_request(b"", &status_wml_only), Err(WapIpError::EmptyProbeDisabled));
        assert_eq!(
            parse_wap_udp_request(b"GET / HTTP/1.0\r\n\r\n", &status_wml_only),
            Err(WapIpError::UnsupportedWapPath { path: "/".to_string() })
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /status.wml#s HTTP/1.0\r\n\r\n", &status_wml_only),
            Ok(WapUdpRequestKind::Status)
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /status.xhtml HTTP/1.0\r\n\r\n", &status_wml_only),
            Err(WapIpError::UnsupportedWapPath {
                path: "/status.xhtml".to_string()
            })
        );
    }

    #[test]
    fn wap_udp_request_policy_bounds_probe_size() {
        let tiny = WapIpServicePolicy {
            status_enabled: true,
            accept_status_wml_path: true,
            max_request_payload_bytes: 8,
            ..WapIpServicePolicy::default()
        };

        assert_eq!(
            parse_wap_udp_request(b"GET /status.wml HTTP/1.0\r\n\r\n", &tiny),
            Err(WapIpError::UdpPayloadTooLarge { len: 28, max: 8 })
        );
    }

    #[test]
    fn wap_policy_can_allowlist_status_issis() {
        let open = WapIpServicePolicy::experimental_status();
        assert!(open.allows_issi(2_260_618));
        assert!(open.allows_issi(2_260_082));

        let restricted = WapIpServicePolicy::experimental_status_for_issis(vec![2_260_618]);
        assert!(restricted.allows_issi(2_260_618));
        assert!(!restricted.allows_issi(2_260_082));
    }

    #[test]
    fn wap_status_response_swaps_ipv4_udp_endpoints() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_udp_npdu([10, 0, 0, 226], endpoint.address, 49152, endpoint.port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");

        let response = build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()).expect("WAP response should build");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");

        assert_eq!(response_ip.source, endpoint.address);
        assert_eq!(response_ip.destination, [10, 0, 0, 226]);
        assert_eq!(response_ip.identification, 0x2223);
        assert_eq!(response_ip.ttl, 32);
        assert_eq!(response_udp.source_port, endpoint.port);
        assert_eq!(response_udp.destination_port, 49152);
        let page = std::str::from_utf8(response_udp.payload).expect("WAP status page should be UTF-8");
        assert!(page.contains("http://www.w3.org/1999/xhtml"));
        assert!(page.contains("-//WAPFORUM//DTD XHTML Mobile 1.0//EN"));
        assert!(page.contains("Welcome to Nexus-BS"));
        assert!(page.contains("WAP 2.0 / WML2"));
        assert!(!page.contains("<wml"));
        assert!(!page.contains("<card"));
        assert!(page.contains("Nexus-BS"));
        assert!(page.contains("MS</span> 4") || page.contains("MS:4"));
    }

    #[test]
    fn wap_status_response_accepts_empty_udp_probe_for_mvp() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_udp_npdu([10, 0, 0, 226], endpoint.address, 49152, endpoint.port, b"", 0x2222, 64)
            .expect("request N-PDU should build");

        let response = build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot())
            .expect("empty WAP UDP probe should build status response");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");
        let page = std::str::from_utf8(response_udp.payload).unwrap();
        assert!(page.contains("http://www.w3.org/1999/xhtml"));
        assert!(page.contains("Welcome"));
    }

    #[test]
    fn wap_status_response_rejects_when_policy_disables_status() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_udp_npdu([10, 0, 0, 226], endpoint.address, 49152, endpoint.port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");

        assert_eq!(
            build_wap_status_response_npdu(&request, endpoint, &WapIpServicePolicy::default(), &snapshot()),
            Err(WapIpError::StatusServiceDisabled)
        );
    }

    #[test]
    fn wap_status_response_rejects_unsupported_udp_payloads() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_udp_npdu(
            [10, 0, 0, 226],
            endpoint.address,
            49152,
            endpoint.port,
            &[0x01, 0x40, 0x00],
            0x2222,
            64,
        )
        .expect("request N-PDU should build");

        assert_eq!(
            build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()),
            Err(WapIpError::UnsupportedWapUdpPayload { len: 3 })
        );
    }

    #[test]
    fn wap_status_response_rejects_non_wap_destination() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let wrong_address =
            build_ipv4_udp_npdu([10, 0, 0, 226], [10, 0, 0, 2], 49152, endpoint.port, b"GET /", 1, 64).expect("request N-PDU should build");
        assert_eq!(
            build_wap_status_response_npdu(&wrong_address, endpoint, &policy(), &snapshot()),
            Err(WapIpError::WrongDestination {
                expected: endpoint.address,
                actual: [10, 0, 0, 2]
            })
        );

        let wrong_port =
            build_ipv4_udp_npdu([10, 0, 0, 226], endpoint.address, 49152, 9201, b"GET /", 1, 64).expect("request N-PDU should build");
        assert_eq!(
            build_wap_status_response_npdu(&wrong_port, endpoint, &policy(), &snapshot()),
            Err(WapIpError::WrongUdpPort {
                expected: endpoint.port,
                actual: 9201
            })
        );
    }
}
