// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original WAP-over-UDP/IP adapter primitives for TETRA SNDCP experiments.

use super::ip::{
    IPV4_PROTOCOL_TCP, IPV4_PROTOCOL_UDP, IpPrimitiveError, TCP_FLAG_ACK, TCP_FLAG_FIN, TCP_FLAG_PSH, TCP_FLAG_RST, TCP_FLAG_SYN,
    TcpSegment, build_ipv4_tcp_npdu, build_ipv4_udp_npdu, parse_ipv4_packet, parse_tcp_segment, parse_udp_datagram,
};
use super::wap_status::{
    DEFAULT_WAP_STATUS_MAX_BYTES, WAP_STATUS_HTML_PATH, WAP_STATUS_LEGACY_WML_PATH, WAP_STATUS_REFRESH_PATH, WapStatusError,
    WapStatusSnapshot, render_wml2_status,
};

pub const DEFAULT_WAP_UDP_REQUEST_MAX_BYTES: usize = 1024;
pub const DEFAULT_WAP_WSP_STATUS_MAX_BYTES: usize = WSP_CONNECT_REPLY_CLIENT_SDU_SIZE_BYTES - WSP_REPLY_FIXED_HEADER_BYTES;
pub const IPV4_UDP_HEADER_BYTES: usize = 28;
pub const IPV4_TCP_HEADER_BYTES: usize = 40;
const TCP_HTTP_DEFAULT_MAX_PAYLOAD_BYTES: usize = 536;
const TCP_HTTP_RESPONSE_WINDOW_BYTES: u16 = 4096;
const WTP_CON_FLAG: u8 = 0x80;
const WTP_RID_FLAG: u8 = 0x01;
const WTP_PDU_INVOKE: u8 = 1;
const WTP_PDU_RESULT: u8 = 2;
const WTP_PDU_ACK: u8 = 3;
const WTP_PDU_ABORT: u8 = 4;
const WTP_RESULT_GTR_TTR: u8 = (WTP_PDU_RESULT << 3) | 0x06;
const WTP_TID_RESPONSE_FLAG: u16 = 0x8000;
const WTP_TID_VALUE_MASK: u16 = 0x7fff;
const WSP_PDU_CONNECT: u8 = 0x01;
const WSP_PDU_CONNECT_REPLY: u8 = 0x02;
const WSP_PDU_REPLY: u8 = 0x04;
const WSP_PDU_GET: u8 = 0x40;
const WSP_STATUS_OK: u8 = 0x20;
const WSP_SHORT_INTEGER_FLAG: u8 = 0x80;
const WSP_CT_APP_VND_WAP_XHTML_XML_ASSIGNED_NUMBER: u8 = 0x45;
const WSP_CT_APP_VND_WAP_XHTML_XML: u8 = WSP_SHORT_INTEGER_FLAG | WSP_CT_APP_VND_WAP_XHTML_XML_ASSIGNED_NUMBER;
const WSP_CAP_CLIENT_SDU_SIZE: u8 = 0x80;
const WSP_CAP_SERVER_SDU_SIZE: u8 = 0x81;
const WSP_CAP_PROTOCOL_OPTIONS: u8 = 0x82;
const WSP_CAP_METHOD_MOR: u8 = 0x83;
const WSP_CAP_EXTENDED_METHODS: u8 = 0x85;
const WSP_CAP_HEADER_CODE_PAGES: u8 = 0x86;
const WSP_CONNECT_REPLY_CLIENT_SDU_SIZE_BYTES: usize = 1400;
const WSP_CONNECT_REPLY_SERVER_SDU_SIZE_BYTES: usize = 1400;
const WSP_REPLY_FIXED_HEADER_BYTES: usize = 4;

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
    WtpWspConnect {
        transaction_id: u16,
        retransmission: bool,
    },
    WtpWspStatus {
        transaction_id: u16,
        retransmission: bool,
    },
    WtpControlNoResponse {
        transaction_id: u16,
        pdu_type: u8,
        abort: Option<WtpAbortInfo>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WtpAbortInfo {
    pub abort_type: u8,
    pub reason: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WapIpError {
    Ip(IpPrimitiveError),
    Status(WapStatusError),
    UnsupportedIpProtocol { protocol: u8 },
    WrongDestination { expected: [u8; 4], actual: [u8; 4] },
    WrongUdpPort { expected: u16, actual: u16 },
    WrongTcpPort { expected: u16, actual: u16 },
    StatusServiceDisabled,
    EmptyProbeDisabled,
    UdpPayloadTooLarge { len: usize, max: usize },
    TcpPayloadTooLarge { len: usize, max: usize },
    UnsupportedWapUdpPayload { len: usize },
    UnsupportedHttpTcpPayload { len: usize },
    UnsupportedTcpSegment { flags: u16, payload_len: usize },
    UnsupportedWapPath { path: String },
    NoResponseRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WspCapability {
    id: u8,
    parameters: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WspConnectRequest {
    version: u8,
    capabilities: Vec<WspCapability>,
}

#[derive(Debug, Clone, Copy)]
struct WtpInvoke<'a> {
    transaction_id: u16,
    retransmission: bool,
    wsp: &'a [u8],
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
    required_response(build_wap_status_response_npdu_optional_with_npdu_budget(
        request_npdu,
        endpoint,
        policy,
        snapshot,
        None,
    )?)
}

pub fn build_wap_status_response_npdu_with_wml2_budget(
    request_npdu: &[u8],
    endpoint: WapIpEndpoint,
    policy: &WapIpServicePolicy,
    snapshot: &WapStatusSnapshot,
    max_wml2_bytes: usize,
) -> Result<Vec<u8>, WapIpError> {
    required_response(build_wap_status_response_npdu_optional_with_npdu_budget(
        request_npdu,
        endpoint,
        policy,
        snapshot,
        Some(max_wml2_bytes.saturating_add(IPV4_UDP_HEADER_BYTES)),
    )?)
}

pub fn build_wap_status_response_npdu_optional_with_npdu_budget(
    request_npdu: &[u8],
    endpoint: WapIpEndpoint,
    policy: &WapIpServicePolicy,
    snapshot: &WapStatusSnapshot,
    max_npdu_bytes: Option<usize>,
) -> Result<Option<Vec<u8>>, WapIpError> {
    let request_ip = parse_ipv4_packet(request_npdu)?;
    if request_ip.destination != endpoint.address {
        return Err(WapIpError::WrongDestination {
            expected: endpoint.address,
            actual: request_ip.destination,
        });
    }

    match request_ip.protocol {
        IPV4_PROTOCOL_UDP => build_udp_status_response(&request_ip, endpoint, policy, snapshot, max_npdu_bytes),
        IPV4_PROTOCOL_TCP => build_tcp_status_response(&request_ip, endpoint, policy, snapshot, max_npdu_bytes),
        protocol => Err(WapIpError::UnsupportedIpProtocol { protocol }),
    }
}

fn required_response(response: Option<Vec<u8>>) -> Result<Vec<u8>, WapIpError> {
    response.ok_or(WapIpError::NoResponseRequired)
}

fn build_udp_status_response(
    request_ip: &super::ip::Ipv4Packet<'_>,
    endpoint: WapIpEndpoint,
    policy: &WapIpServicePolicy,
    snapshot: &WapStatusSnapshot,
    max_npdu_bytes: Option<usize>,
) -> Result<Option<Vec<u8>>, WapIpError> {
    let request_udp = parse_udp_datagram(request_ip.payload)?;
    if request_udp.destination_port != endpoint.port {
        return Err(WapIpError::WrongUdpPort {
            expected: endpoint.port,
            actual: request_udp.destination_port,
        });
    }
    let request_kind = parse_wap_udp_request(request_udp.payload, policy)?;
    tracing::info!(
        "WAP/IP diag: IPv4/UDP request src={:?}:{} dst={:?}:{} kind={:?} payload_len={}",
        request_ip.source,
        request_udp.source_port,
        request_ip.destination,
        request_udp.destination_port,
        request_kind,
        request_udp.payload.len()
    );

    let response_payload = match request_kind {
        WapUdpRequestKind::Empty | WapUdpRequestKind::Status => {
            let max_wml2_bytes = max_npdu_bytes
                .map(|max| max.saturating_sub(IPV4_UDP_HEADER_BYTES))
                .unwrap_or(DEFAULT_WAP_STATUS_MAX_BYTES);
            render_wml2_status(snapshot, max_wml2_bytes)?.into_bytes()
        }
        WapUdpRequestKind::WtpWspConnect {
            transaction_id,
            retransmission: _,
        } => {
            let connect = parse_wtp_wsp_connect_request(request_udp.payload)?.ok_or(WapIpError::UnsupportedWapUdpPayload {
                len: request_udp.payload.len(),
            })?;
            tracing::info!(
                "WAP/IP diag: WSP Connect version={:#x} requested_capabilities={:?}",
                connect.version,
                connect.capabilities
            );
            build_wtp_wsp_result(transaction_id, &build_wsp_connect_reply(&connect))
        }
        WapUdpRequestKind::WtpWspStatus {
            transaction_id,
            retransmission: _,
        } => {
            let max_wsp_page_bytes = max_npdu_bytes
                .map(|max| max.saturating_sub(IPV4_UDP_HEADER_BYTES + 3 + WSP_REPLY_FIXED_HEADER_BYTES))
                .unwrap_or(DEFAULT_WAP_WSP_STATUS_MAX_BYTES)
                .min(DEFAULT_WAP_WSP_STATUS_MAX_BYTES);
            let page = render_wml2_status(snapshot, max_wsp_page_bytes)?;
            build_wtp_wsp_result(transaction_id, &build_wsp_reply(page.as_bytes()))
        }
        WapUdpRequestKind::WtpControlNoResponse {
            transaction_id,
            pdu_type,
            abort,
        } => {
            tracing::info!(
                "WAP/IP diag: IPv4/UDP WTP control no-response transaction_id={} pdu_type={} abort={:?} abort_type={} abort_reason={} abort_reason_name={}",
                transaction_id,
                pdu_type,
                abort,
                abort.map(|info| info.abort_type).unwrap_or_default(),
                abort.map(|info| info.reason).unwrap_or_default(),
                abort.map(wtp_abort_reason_name).unwrap_or("none")
            );
            return Ok(None);
        }
    };
    let response = build_ipv4_udp_npdu(
        endpoint.address,
        request_ip.source,
        endpoint.port,
        request_udp.source_port,
        &response_payload,
        request_ip.identification.wrapping_add(1),
        endpoint.response_ttl,
    )?;
    tracing::info!(
        "WAP/IP diag: IPv4/UDP response dst={:?}:{} payload_len={} npdu_len={}",
        request_ip.source,
        request_udp.source_port,
        response_payload.len(),
        response.len()
    );

    Ok(Some(response))
}

fn build_tcp_status_response(
    request_ip: &super::ip::Ipv4Packet<'_>,
    endpoint: WapIpEndpoint,
    policy: &WapIpServicePolicy,
    snapshot: &WapStatusSnapshot,
    max_npdu_bytes: Option<usize>,
) -> Result<Option<Vec<u8>>, WapIpError> {
    let request_tcp = parse_tcp_segment(request_ip.payload)?;
    if request_tcp.destination_port != endpoint.port {
        return Err(WapIpError::WrongTcpPort {
            expected: endpoint.port,
            actual: request_tcp.destination_port,
        });
    }
    if !policy.status_enabled {
        return Err(WapIpError::StatusServiceDisabled);
    }
    if request_tcp.payload.len() > policy.max_request_payload_bytes {
        return Err(WapIpError::TcpPayloadTooLarge {
            len: request_tcp.payload.len(),
            max: policy.max_request_payload_bytes,
        });
    }
    if request_tcp.flags & TCP_FLAG_RST != 0 {
        tracing::debug!(
            "WAP/IP diag: IPv4/TCP RST src={:?}:{} seq={} ack={}",
            request_ip.source,
            request_tcp.source_port,
            request_tcp.sequence_number,
            request_tcp.acknowledgement_number
        );
        return Ok(None);
    }

    let server_iss = tcp_server_iss(request_ip, &request_tcp);
    if request_tcp.flags & TCP_FLAG_SYN != 0 {
        tracing::info!(
            "WAP/IP diag: IPv4/TCP SYN src={:?}:{} dst={:?}:{} seq={} -> SYN-ACK seq={} ack={}",
            request_ip.source,
            request_tcp.source_port,
            request_ip.destination,
            request_tcp.destination_port,
            request_tcp.sequence_number,
            server_iss,
            request_tcp.sequence_number.wrapping_add(1)
        );
        return build_tcp_npdu_response(
            request_ip,
            &request_tcp,
            endpoint,
            server_iss,
            request_tcp.sequence_number.wrapping_add(1),
            TCP_FLAG_SYN | TCP_FLAG_ACK,
            b"",
        )
        .map(Some);
    }

    if request_tcp.payload.is_empty() {
        tracing::debug!(
            "WAP/IP diag: IPv4/TCP ACK-only/no-payload src={:?}:{} flags={:#x} seq={} ack={}; no response",
            request_ip.source,
            request_tcp.source_port,
            request_tcp.flags,
            request_tcp.sequence_number,
            request_tcp.acknowledgement_number
        );
        return Ok(None);
    }

    let request_path = parse_http_tcp_status_get(request_tcp.payload, policy)?;
    let max_tcp_payload_bytes = max_npdu_bytes
        .map(|max| max.saturating_sub(IPV4_TCP_HEADER_BYTES))
        .unwrap_or(TCP_HTTP_DEFAULT_MAX_PAYLOAD_BYTES)
        .min(TCP_HTTP_DEFAULT_MAX_PAYLOAD_BYTES);
    let response_payload = build_http_status_payload(snapshot, max_tcp_payload_bytes)?;
    let response_sequence = if request_tcp.flags & TCP_FLAG_ACK != 0 && request_tcp.acknowledgement_number != 0 {
        request_tcp.acknowledgement_number
    } else {
        server_iss.wrapping_add(1)
    };
    let response_ack = tcp_ack_number(&request_tcp);

    let response = build_tcp_npdu_response(
        request_ip,
        &request_tcp,
        endpoint,
        response_sequence,
        response_ack,
        TCP_FLAG_ACK | TCP_FLAG_PSH | TCP_FLAG_FIN,
        &response_payload,
    )?;
    tracing::info!(
        "WAP/IP diag: IPv4/TCP HTTP GET src={:?}:{} path={} req_payload_len={} -> 200 XHTML payload_len={} npdu_len={} seq={} ack={}",
        request_ip.source,
        request_tcp.source_port,
        request_path,
        request_tcp.payload.len(),
        response_payload.len(),
        response.len(),
        response_sequence,
        response_ack
    );
    Ok(Some(response))
}

fn build_tcp_npdu_response(
    request_ip: &super::ip::Ipv4Packet<'_>,
    request_tcp: &TcpSegment<'_>,
    endpoint: WapIpEndpoint,
    sequence_number: u32,
    acknowledgement_number: u32,
    flags: u16,
    payload: &[u8],
) -> Result<Vec<u8>, WapIpError> {
    Ok(build_ipv4_tcp_npdu(
        endpoint.address,
        request_ip.source,
        endpoint.port,
        request_tcp.source_port,
        sequence_number,
        acknowledgement_number,
        flags,
        TCP_HTTP_RESPONSE_WINDOW_BYTES,
        payload,
        request_ip.identification.wrapping_add(1),
        endpoint.response_ttl,
    )?)
}

fn parse_http_tcp_status_get(payload: &[u8], policy: &WapIpServicePolicy) -> Result<String, WapIpError> {
    let Ok(text) = std::str::from_utf8(payload) else {
        return Err(WapIpError::UnsupportedHttpTcpPayload { len: payload.len() });
    };
    let first_line = text.lines().next().unwrap_or("").trim_end_matches('\r').trim();
    let Some(path) = first_line.strip_prefix("GET ") else {
        return Err(WapIpError::UnsupportedHttpTcpPayload { len: payload.len() });
    };
    let path = path.split_whitespace().next().unwrap_or("/");
    let path = normalize_get_path(uri_path(path));
    if is_status_path(&path, policy) {
        return Ok(path);
    }
    Err(WapIpError::UnsupportedWapPath { path })
}

fn build_http_status_payload(snapshot: &WapStatusSnapshot, max_tcp_payload_bytes: usize) -> Result<Vec<u8>, WapIpError> {
    let mut body_budget = max_tcp_payload_bytes.saturating_sub(build_http_status_header(0).len() + 5);
    for _ in 0..4 {
        let body = render_wml2_status(snapshot, body_budget)?;
        let header = build_http_status_header(body.len());
        let total = header.len() + body.len();
        if total <= max_tcp_payload_bytes {
            let mut payload = Vec::with_capacity(total);
            payload.extend_from_slice(header.as_bytes());
            payload.extend_from_slice(body.as_bytes());
            return Ok(payload);
        }
        let next_budget = max_tcp_payload_bytes.saturating_sub(header.len());
        if next_budget >= body_budget {
            return Err(WapIpError::Status(WapStatusError::RenderedTooLarge {
                len: total,
                max: max_tcp_payload_bytes,
            }));
        }
        body_budget = next_budget;
    }

    Err(WapIpError::Status(WapStatusError::RenderedTooLarge {
        len: max_tcp_payload_bytes.saturating_add(1),
        max: max_tcp_payload_bytes,
    }))
}

fn build_http_status_header(content_len: usize) -> String {
    format!(
        "HTTP/1.0 200 OK\r\nContent-Type: application/vnd.wap.xhtml+xml\r\nContent-Length: {content_len}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n"
    )
}

fn tcp_ack_number(segment: &TcpSegment<'_>) -> u32 {
    let mut ack = segment.sequence_number.wrapping_add(segment.payload.len() as u32);
    if segment.flags & TCP_FLAG_SYN != 0 {
        ack = ack.wrapping_add(1);
    }
    if segment.flags & TCP_FLAG_FIN != 0 {
        ack = ack.wrapping_add(1);
    }
    ack
}

fn tcp_server_iss(request_ip: &super::ip::Ipv4Packet<'_>, request_tcp: &TcpSegment<'_>) -> u32 {
    let mut hash = 0x4e42_5300u32;
    for byte in request_ip
        .source
        .iter()
        .copied()
        .chain(request_ip.destination.iter().copied())
        .chain(request_tcp.source_port.to_be_bytes())
        .chain(request_tcp.destination_port.to_be_bytes())
    {
        hash ^= byte as u32;
        hash = hash.rotate_left(5).wrapping_mul(0x0100_0193);
    }
    hash
}

pub fn parse_wap_udp_request(payload: &[u8], policy: &WapIpServicePolicy) -> Result<WapUdpRequestKind, WapIpError> {
    // ETSI TS 100 392-2 clause 29.5.8 delegates WAP protocol details to WAP
    // 2.0. This diagnostic gateway intentionally admits only the configured
    // status resources over the WSP/WTP/UDP packet-data bearer.
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

    if let Some(kind) = parse_wtp_wsp_request(payload, policy)? {
        return Ok(kind);
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

fn parse_wtp_wsp_request(payload: &[u8], policy: &WapIpServicePolicy) -> Result<Option<WapUdpRequestKind>, WapIpError> {
    if payload.len() < 3 {
        return Ok(None);
    }

    let wtp_pdu_type = (payload[0] >> 3) & 0x0f;
    let transaction_id = u16::from_be_bytes([payload[1], payload[2]]) & WTP_TID_VALUE_MASK;
    if matches!(wtp_pdu_type, WTP_PDU_ACK | WTP_PDU_ABORT) {
        return Ok(Some(WapUdpRequestKind::WtpControlNoResponse {
            transaction_id,
            pdu_type: wtp_pdu_type,
            abort: parse_wtp_abort_info(payload, wtp_pdu_type),
        }));
    }
    if wtp_pdu_type != WTP_PDU_INVOKE {
        return Ok(None);
    }

    let Some(invoke) = parse_wtp_invoke(payload)? else {
        return Ok(None);
    };
    let wsp = invoke.wsp;
    match wsp.first().copied() {
        Some(WSP_PDU_CONNECT) => Ok(Some(WapUdpRequestKind::WtpWspConnect {
            transaction_id: invoke.transaction_id,
            retransmission: invoke.retransmission,
        })),
        Some(pdu_type) if pdu_type == WSP_PDU_GET || (0x50..=0x5f).contains(&pdu_type) => {
            let (uri_len, len_octets) = read_uintvar(&wsp[1..]).ok_or(WapIpError::UnsupportedWapUdpPayload { len: payload.len() })?;
            let uri_start = 1 + len_octets;
            let uri_end = uri_start + uri_len;
            if uri_end > wsp.len() {
                return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
            }
            let Ok(uri) = std::str::from_utf8(&wsp[uri_start..uri_end]) else {
                return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
            };
            let path = normalize_get_path(uri_path(uri));
            if is_status_path(&path, policy) {
                return Ok(Some(WapUdpRequestKind::WtpWspStatus {
                    transaction_id: invoke.transaction_id,
                    retransmission: invoke.retransmission,
                }));
            }
            Err(WapIpError::UnsupportedWapPath { path })
        }
        _ => Ok(None),
    }
}

fn parse_wtp_wsp_connect_request(payload: &[u8]) -> Result<Option<WspConnectRequest>, WapIpError> {
    let Some(invoke) = parse_wtp_invoke(payload)? else {
        return Ok(None);
    };
    let wsp = invoke.wsp;
    if wsp.first().copied() != Some(WSP_PDU_CONNECT) {
        return Ok(None);
    }
    parse_wsp_connect_request(wsp).map(Some)
}

fn parse_wtp_invoke(payload: &[u8]) -> Result<Option<WtpInvoke<'_>>, WapIpError> {
    if payload.len() < 4 || ((payload[0] >> 3) & 0x0f) != WTP_PDU_INVOKE {
        return Ok(None);
    }
    let invoke_header = payload[3];
    let version = (invoke_header >> 6) & 0x03;
    let reserved = invoke_header & 0x0c;
    let transaction_class = invoke_header & 0x03;
    if version != 0 || reserved != 0 || transaction_class != 2 {
        return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
    }

    let wsp_start = if payload[0] & WTP_CON_FLAG != 0 {
        parse_wtp_variable_header_end(payload, 4)?
    } else {
        4
    };
    if wsp_start >= payload.len() {
        return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
    }

    Ok(Some(WtpInvoke {
        transaction_id: u16::from_be_bytes([payload[1], payload[2]]) & WTP_TID_VALUE_MASK,
        retransmission: payload[0] & WTP_RID_FLAG != 0,
        wsp: &payload[wsp_start..],
    }))
}

fn parse_wtp_variable_header_end(payload: &[u8], mut offset: usize) -> Result<usize, WapIpError> {
    loop {
        let Some(tpi_header) = payload.get(offset).copied() else {
            return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
        };
        let tpi_continues = tpi_header & WTP_CON_FLAG != 0;
        let tpi_is_long = tpi_header & 0x04 != 0;
        offset = if tpi_is_long {
            let Some(tpi_len) = payload.get(offset + 1).copied() else {
                return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
            };
            offset + 2 + tpi_len as usize
        } else {
            offset + 1 + (tpi_header & 0x03) as usize
        };
        if offset > payload.len() {
            return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
        }
        if !tpi_continues {
            return Ok(offset);
        }
    }
}

fn parse_wsp_connect_request(wsp: &[u8]) -> Result<WspConnectRequest, WapIpError> {
    if wsp.len() < 4 || wsp[0] != WSP_PDU_CONNECT {
        return Err(WapIpError::UnsupportedWapUdpPayload { len: wsp.len() });
    }

    let version = wsp[1];
    let (capabilities_len, cap_len_octets) = read_uintvar(&wsp[2..]).ok_or(WapIpError::UnsupportedWapUdpPayload { len: wsp.len() })?;
    let headers_len_start = 2 + cap_len_octets;
    let (headers_len, headers_len_octets) =
        read_uintvar(&wsp[headers_len_start..]).ok_or(WapIpError::UnsupportedWapUdpPayload { len: wsp.len() })?;
    let capabilities_start = headers_len_start + headers_len_octets;
    let capabilities_end = capabilities_start + capabilities_len;
    let headers_end = capabilities_end + headers_len;
    if headers_end > wsp.len() {
        return Err(WapIpError::UnsupportedWapUdpPayload { len: wsp.len() });
    }

    let capabilities = parse_wsp_capabilities(&wsp[capabilities_start..capabilities_end])?;
    Ok(WspConnectRequest { version, capabilities })
}

fn parse_wsp_capabilities(mut buf: &[u8]) -> Result<Vec<WspCapability>, WapIpError> {
    let mut capabilities = Vec::new();
    while !buf.is_empty() {
        let (len, len_octets) = read_uintvar(buf).ok_or(WapIpError::UnsupportedWapUdpPayload { len: buf.len() })?;
        if len == 0 || len_octets + len > buf.len() {
            return Err(WapIpError::UnsupportedWapUdpPayload { len: buf.len() });
        }
        let body = &buf[len_octets..len_octets + len];
        capabilities.push(WspCapability {
            id: body[0],
            parameters: body[1..].to_vec(),
        });
        buf = &buf[len_octets + len..];
    }
    Ok(capabilities)
}

fn build_wtp_wsp_result(transaction_id: u16, wsp_payload: &[u8]) -> Vec<u8> {
    let response_tid = (transaction_id & WTP_TID_VALUE_MASK) | WTP_TID_RESPONSE_FLAG;
    let mut payload = Vec::with_capacity(3 + wsp_payload.len());
    payload.push(WTP_RESULT_GTR_TTR);
    payload.extend_from_slice(&response_tid.to_be_bytes());
    payload.extend_from_slice(wsp_payload);
    payload
}

fn build_wsp_connect_reply(connect: &WspConnectRequest) -> Vec<u8> {
    let capabilities = build_wsp_connect_reply_capabilities(connect);
    let mut payload = Vec::with_capacity(4 + capabilities.len());
    payload.push(WSP_PDU_CONNECT_REPLY);
    payload.push(0x01); // Server session id.
    write_uintvar(capabilities.len(), &mut payload);
    write_uintvar(0, &mut payload); // Headers length.
    payload.extend_from_slice(&capabilities);
    payload
}

fn build_wsp_connect_reply_capabilities(connect: &WspConnectRequest) -> Vec<u8> {
    let mut capabilities = Vec::new();
    if let Some(requested) = wsp_capability_uintvar(connect, WSP_CAP_CLIENT_SDU_SIZE) {
        push_wsp_uintvar_capability(
            &mut capabilities,
            WSP_CAP_CLIENT_SDU_SIZE,
            requested.min(WSP_CONNECT_REPLY_CLIENT_SDU_SIZE_BYTES),
        );
    }
    if let Some(requested) = wsp_capability_uintvar(connect, WSP_CAP_SERVER_SDU_SIZE) {
        push_wsp_uintvar_capability(
            &mut capabilities,
            WSP_CAP_SERVER_SDU_SIZE,
            requested.min(WSP_CONNECT_REPLY_SERVER_SDU_SIZE_BYTES),
        );
    }
    if wsp_capability(connect, WSP_CAP_PROTOCOL_OPTIONS).is_some() {
        push_wsp_octets_capability(&mut capabilities, WSP_CAP_PROTOCOL_OPTIONS, &[0x00]);
    }
    if let Some(requested) = wsp_capability(connect, WSP_CAP_METHOD_MOR).and_then(|capability| capability.parameters.first().copied()) {
        let accepted = requested.min(1);
        if accepted > 0 {
            push_wsp_octets_capability(&mut capabilities, WSP_CAP_METHOD_MOR, &[accepted]);
        }
    }
    if let Some(requested) = wsp_capability(connect, WSP_CAP_EXTENDED_METHODS) {
        let accepted: Vec<u8> = requested
            .parameters
            .split(|octet| *octet == 0)
            .filter_map(|entry| entry.first().copied())
            .filter(|pdu_type| (0x50..=0x5f).contains(pdu_type))
            .collect();
        if !accepted.is_empty() {
            push_wsp_octets_capability(&mut capabilities, WSP_CAP_EXTENDED_METHODS, &accepted);
        }
    }
    if wsp_capability(connect, WSP_CAP_HEADER_CODE_PAGES).is_some() {
        push_wsp_octets_capability(&mut capabilities, WSP_CAP_HEADER_CODE_PAGES, &[]);
    }
    capabilities
}

fn parse_wtp_abort_info(payload: &[u8], pdu_type: u8) -> Option<WtpAbortInfo> {
    if pdu_type != WTP_PDU_ABORT || payload.len() < 4 {
        return None;
    }
    // OMA WTP puts the Abort type in the low bits of octet 1 and the Abort
    // reason in octet 4. WSP user abort reasons use the full octet value.
    Some(WtpAbortInfo {
        abort_type: payload[0] & 0x07,
        reason: payload[3],
    })
}

fn wtp_abort_reason_name(info: WtpAbortInfo) -> &'static str {
    match (info.abort_type, info.reason) {
        (0, 0) => "provider/UNKNOWN",
        (0, 1) => "provider/PROTOERR",
        (0, 2) => "provider/INVALIDTID",
        (0, 3) => "provider/NOTIMPLEMENTEDCL2",
        (0, 4) => "provider/NOTIMPLEMENTEDSAR",
        (0, 5) => "provider/NOTIMPLEMENTEDUACK",
        (0, 6) => "provider/WTPVERSIONONE",
        (0, 7) => "provider/CAPTEMPEXCEEDED",
        (0, 8) => "provider/NORESPONSE",
        (0, 9) => "provider/MESSAGETOOLARGE",
        (0, 10) => "provider/NOTIMPLEMENTEDESAR",
        (1, 0xe0) => "user/WSP_PROTOERR",
        (1, 0xe1) => "user/WSP_DISCONNECT",
        (1, 0xe2) => "user/WSP_SUSPEND",
        (1, 0xe3) => "user/WSP_RESUME",
        (1, 0xe4) => "user/WSP_CONGESTION",
        (1, 0xe5) => "user/WSP_CONNECTERR",
        (1, 0xe6) => "user/WSP_MRUEXCEEDED",
        (1, 0xe7) => "user/WSP_MOREXCEEDED",
        (1, 0xe8) => "user/WSP_PEERREQ",
        (1, 0xe9) => "user/WSP_NETERR",
        (1, 0xea) => "user/WSP_USERREQ",
        (1, 0xeb) => "user/WSP_USERRFS",
        (1, 0xec) => "user/WSP_USERPND",
        (1, 0xed) => "user/WSP_USERDCR",
        (1, 0xee) => "user/WSP_USERDCU",
        (1, _) => "user/WSP_OR_APPLICATION",
        _ => "unknown",
    }
}

fn wsp_capability(connect: &WspConnectRequest, id: u8) -> Option<&WspCapability> {
    connect.capabilities.iter().find(|capability| capability.id == id)
}

fn wsp_capability_uintvar(connect: &WspConnectRequest, id: u8) -> Option<usize> {
    let capability = wsp_capability(connect, id)?;
    let (value, len) = read_uintvar(&capability.parameters)?;
    (len == capability.parameters.len()).then_some(value)
}

fn push_wsp_uintvar_capability(out: &mut Vec<u8>, capability_id: u8, value: usize) {
    let mut encoded_value = Vec::new();
    write_uintvar(value, &mut encoded_value);
    push_wsp_octets_capability(out, capability_id, &encoded_value);
}

fn push_wsp_octets_capability(out: &mut Vec<u8>, capability_id: u8, value: &[u8]) {
    write_uintvar(1 + value.len(), out);
    out.push(capability_id);
    out.extend_from_slice(value);
}

fn build_wsp_reply(page: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(WSP_REPLY_FIXED_HEADER_BYTES + page.len());
    payload.push(WSP_PDU_REPLY);
    payload.push(WSP_STATUS_OK);
    write_uintvar(1, &mut payload);
    payload.push(WSP_CT_APP_VND_WAP_XHTML_XML);
    payload.extend_from_slice(page);
    payload
}

fn read_uintvar(buf: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0usize;
    for (idx, octet) in buf.iter().copied().enumerate().take(5) {
        value = value.checked_shl(7)?.checked_add((octet & 0x7f) as usize)?;
        if octet & 0x80 == 0 {
            return Some((value, idx + 1));
        }
    }
    None
}

fn write_uintvar(mut value: usize, out: &mut Vec<u8>) {
    let mut stack = [0u8; 5];
    let mut idx = stack.len();
    loop {
        idx -= 1;
        stack[idx] = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            break;
        }
    }
    let continuation_end = stack.len() - 1;
    for byte in &mut stack[idx..continuation_end] {
        *byte |= 0x80;
    }
    out.extend_from_slice(&stack[idx..]);
}

fn normalize_get_path(path: &str) -> String {
    let path = path.trim();
    let path = path.split(['?', '#']).next().unwrap_or(path).trim();
    let path = path.strip_prefix("./").unwrap_or(path);
    if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn uri_path(uri: &str) -> &str {
    let uri = uri.trim();
    if let Some(rest) = uri.strip_prefix("http://").or_else(|| uri.strip_prefix("https://")) {
        return rest.find('/').map(|idx| &rest[idx..]).unwrap_or("/");
    }
    uri
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
    use crate::sndcp::ip::{parse_ipv4_packet, parse_tcp_segment, parse_udp_datagram};

    fn snapshot() -> WapStatusSnapshot {
        WapStatusSnapshot {
            title: "Nexus-BS".to_string(),
            stack_version: "v0.1.69_dev-test".to_string(),
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

    fn hex_octets(hex: &str) -> Vec<u8> {
        assert_eq!(hex.len() % 2, 0);
        hex.as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let hi = (pair[0] as char).to_digit(16).expect("test hex high nibble");
                let lo = (pair[1] as char).to_digit(16).expect("test hex low nibble");
                ((hi << 4) | lo) as u8
            })
            .collect()
    }

    fn mxp600_wtp_wsp_connect_payload() -> Vec<u8> {
        hex_octets(concat!(
            "0b13cc1201101d8264048094800004819480000282f0028303028401098610782d75702d3100",
            "456e636f64696e672d76657273696f6e00312e33008094809580b380a3806170706c696361",
            "74696f6e2f766e642e70686f6e65636f6d2e6d6d632d7762786d6c00806170706c696361",
            "74696f6e2f6f637465742d73747265616d00808380746578742f6373730080696d616765",
            "2f626d7000809d809e80a080a180ae80b080b2806170706c69636174696f6e2f766e642e",
            "7761702e7868746d6c2b786d6c00801f3e6170706c69636174696f6e2f7868746d6c2b",
            "786d6c0070726f66696c650022687474703a2f2f7777772e776170666f72756d2e6f7267",
            "2f7868746d6c2200808280696d6167652f782d75702d77706e6700806170706c69636174",
            "696f6e2f766e642e75706c616e65742e6265617265722d63686f6963652d7762786d6c00",
            "01a94d4f542d4d58503630305c4d52323032362e312055502e42726f777365722f362e",
            "332e302e31202847554929204d4d502f322e3000bbea83656e2d67620083028001"
        ))
    }

    fn build_wtp_wsp_get_payload(transaction_id: u16, uri: &str) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.push(0x0a);
        payload.extend_from_slice(&(transaction_id & WTP_TID_VALUE_MASK).to_be_bytes());
        payload.push(0x12);
        payload.push(WSP_PDU_GET);
        write_uintvar(uri.len(), &mut payload);
        payload.extend_from_slice(uri.as_bytes());
        payload
    }

    fn build_wtp_wsp_connect_payload(transaction_id: u16, capabilities: &[u8]) -> Vec<u8> {
        let mut wsp = Vec::new();
        wsp.push(WSP_PDU_CONNECT);
        wsp.push(0x10);
        write_uintvar(capabilities.len(), &mut wsp);
        write_uintvar(0, &mut wsp);
        wsp.extend_from_slice(capabilities);

        let mut payload = Vec::new();
        payload.push(0x0a);
        payload.extend_from_slice(&(transaction_id & WTP_TID_VALUE_MASK).to_be_bytes());
        payload.push(0x12);
        payload.extend_from_slice(&wsp);
        payload
    }

    fn prepend_short_wtp_tpi(mut payload: Vec<u8>) -> Vec<u8> {
        assert!(payload.len() >= 4, "test helper expects a WTP Invoke fixed header");
        payload[0] |= WTP_CON_FLAG;
        payload.splice(4..4, [0x01, 0x00]);
        payload
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
            parse_wap_udp_request(b"GET status.xhtml?refresh=1 HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status)
        );
        assert_eq!(
            parse_wap_udp_request(b"GET ./status.xhtml HTTP/1.0\r\n\r\n", &policy()),
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
    fn wap_udp_request_classifier_accepts_terminal_browser_headers() {
        let mut request = b"GET /status.xhtml HTTP/1.1\r\nHost: 10.0.0.1:9200\r\nUser-Agent: MXP600 WAP Browser\r\nAccept: ".to_vec();
        request.resize(392, b'a');
        request.extend_from_slice(b"\r\n");
        assert_eq!(request.len(), 394);

        assert_eq!(parse_wap_udp_request(&request, &policy()), Ok(WapUdpRequestKind::Status));
    }

    #[test]
    fn wap_udp_request_classifier_accepts_mxp600_wtp_wsp_connect() {
        assert_eq!(
            parse_wap_udp_request(&mxp600_wtp_wsp_connect_payload(), &policy()),
            Ok(WapUdpRequestKind::WtpWspConnect {
                transaction_id: 0x13cc,
                retransmission: true
            })
        );
    }

    #[test]
    fn wap_udp_request_classifier_skips_wtp_tpis_before_wsp() {
        let connect = prepend_short_wtp_tpi(build_wtp_wsp_connect_payload(0x1234, &[]));
        assert_eq!(
            parse_wap_udp_request(&connect, &policy()),
            Ok(WapUdpRequestKind::WtpWspConnect {
                transaction_id: 0x1234,
                retransmission: false
            })
        );

        let get = prepend_short_wtp_tpi(build_wtp_wsp_get_payload(0x1235, "status.xhtml"));
        assert_eq!(
            parse_wap_udp_request(&get, &policy()),
            Ok(WapUdpRequestKind::WtpWspStatus {
                transaction_id: 0x1235,
                retransmission: false
            })
        );
    }

    #[test]
    fn wap_udp_request_classifier_accepts_wtp_wsp_get_status_path() {
        assert_eq!(
            parse_wap_udp_request(
                &build_wtp_wsp_get_payload(0x1234, "http://10.0.0.1:9200/status.xhtml?refresh=1"),
                &policy()
            ),
            Ok(WapUdpRequestKind::WtpWspStatus {
                transaction_id: 0x1234,
                retransmission: false
            })
        );
        assert_eq!(
            parse_wap_udp_request(&build_wtp_wsp_get_payload(0x1234, "status.xhtml"), &policy()),
            Ok(WapUdpRequestKind::WtpWspStatus {
                transaction_id: 0x1234,
                retransmission: false
            })
        );
    }

    #[test]
    fn wap_udp_request_classifier_treats_wtp_ack_as_no_response() {
        assert_eq!(
            parse_wap_udp_request(&[0x18, 0x13, 0xcc], &policy()),
            Ok(WapUdpRequestKind::WtpControlNoResponse {
                transaction_id: 0x13cc,
                pdu_type: WTP_PDU_ACK,
                abort: None,
            })
        );
        assert_eq!(
            parse_wap_udp_request(&[0x1c, 0x13, 0xcc], &policy()),
            Ok(WapUdpRequestKind::WtpControlNoResponse {
                transaction_id: 0x13cc,
                pdu_type: WTP_PDU_ACK,
                abort: None,
            })
        );
        assert_eq!(
            parse_wap_udp_request(&[0x20, 0x13, 0xcc, 0x01], &policy()),
            Ok(WapUdpRequestKind::WtpControlNoResponse {
                transaction_id: 0x13cc,
                pdu_type: WTP_PDU_ABORT,
                abort: Some(WtpAbortInfo { abort_type: 0, reason: 1 }),
            })
        );
        assert_eq!(
            parse_wap_udp_request(&[0x21, 0x13, 0xcc, 0xe0], &policy()),
            Ok(WapUdpRequestKind::WtpControlNoResponse {
                transaction_id: 0x13cc,
                pdu_type: WTP_PDU_ABORT,
                abort: Some(WtpAbortInfo {
                    abort_type: 1,
                    reason: 0xe0,
                }),
            })
        );
        assert_eq!(
            parse_wap_udp_request(&[0x27, 0x13, 0xcc, 0xe0], &policy()),
            Ok(WapUdpRequestKind::WtpControlNoResponse {
                transaction_id: 0x13cc,
                pdu_type: WTP_PDU_ABORT,
                abort: Some(WtpAbortInfo {
                    abort_type: 7,
                    reason: 0xe0,
                }),
            })
        );
        assert_eq!(
            wtp_abort_reason_name(WtpAbortInfo { abort_type: 0, reason: 1 }),
            "provider/PROTOERR"
        );
        assert_eq!(
            wtp_abort_reason_name(WtpAbortInfo {
                abort_type: 1,
                reason: 0xe0,
            }),
            "user/WSP_PROTOERR"
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
    fn wap_status_response_answers_tcp_syn_with_syn_ack() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_tcp_npdu(
            [10, 0, 0, 226],
            endpoint.address,
            49152,
            endpoint.port,
            0x0102_0304,
            0,
            TCP_FLAG_SYN,
            2048,
            b"",
            0x3333,
            64,
        )
        .expect("TCP SYN N-PDU should build");

        let response = build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()).expect("TCP SYN response should build");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_tcp = parse_tcp_segment(response_ip.payload).expect("response TCP should parse");

        assert_eq!(response_ip.source, endpoint.address);
        assert_eq!(response_ip.destination, [10, 0, 0, 226]);
        assert_eq!(response_ip.identification, 0x3334);
        assert_eq!(response_ip.ttl, 32);
        assert_eq!(response_tcp.source_port, endpoint.port);
        assert_eq!(response_tcp.destination_port, 49152);
        assert_eq!(response_tcp.flags, TCP_FLAG_SYN | TCP_FLAG_ACK);
        assert_eq!(response_tcp.acknowledgement_number, 0x0102_0305);
        assert!(response_tcp.payload.is_empty());
    }

    #[test]
    fn wap_status_response_suppresses_tcp_ack_only_segments() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_tcp_npdu(
            [10, 0, 0, 226],
            endpoint.address,
            49152,
            endpoint.port,
            0x0102_0305,
            0x9000_0001,
            TCP_FLAG_ACK,
            2048,
            b"",
            0x3334,
            64,
        )
        .expect("TCP ACK N-PDU should build");

        assert_eq!(
            build_wap_status_response_npdu_optional_with_npdu_budget(&request, endpoint, &policy(), &snapshot(), Some(576)),
            Ok(None)
        );
        assert_eq!(
            build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()),
            Err(WapIpError::NoResponseRequired)
        );
    }

    #[test]
    fn wap_status_response_answers_tcp_http_get_with_xhtml_and_fin() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let syn = build_ipv4_tcp_npdu(
            [10, 0, 0, 2],
            endpoint.address,
            49152,
            endpoint.port,
            0x1000_0000,
            0,
            TCP_FLAG_SYN,
            2048,
            b"",
            0x3333,
            64,
        )
        .expect("TCP SYN N-PDU should build");
        let syn_ack = build_wap_status_response_npdu(&syn, endpoint, &policy(), &snapshot()).expect("TCP SYN response should build");
        let syn_ack_ip = parse_ipv4_packet(&syn_ack).expect("SYN-ACK IPv4 should parse");
        let syn_ack_tcp = parse_tcp_segment(syn_ack_ip.payload).expect("SYN-ACK TCP should parse");
        let request_payload = b"GET /status.xhtml HTTP/1.1\r\nHost: 10.0.0.1:9200\r\nConnection: close\r\n\r\n";
        let get = build_ipv4_tcp_npdu(
            [10, 0, 0, 2],
            endpoint.address,
            49152,
            endpoint.port,
            0x1000_0001,
            syn_ack_tcp.sequence_number.wrapping_add(1),
            TCP_FLAG_ACK | TCP_FLAG_PSH,
            2048,
            request_payload,
            0x3334,
            64,
        )
        .expect("TCP GET N-PDU should build");

        let response = build_wap_status_response_npdu_optional_with_npdu_budget(&get, endpoint, &policy(), &snapshot(), Some(576))
            .expect("TCP GET response should build")
            .expect("TCP GET should require a response");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_tcp = parse_tcp_segment(response_ip.payload).expect("response TCP should parse");
        let http = std::str::from_utf8(response_tcp.payload).expect("HTTP response should be UTF-8");

        assert_eq!(response_ip.source, endpoint.address);
        assert_eq!(response_ip.destination, [10, 0, 0, 2]);
        assert_eq!(response_tcp.source_port, endpoint.port);
        assert_eq!(response_tcp.destination_port, 49152);
        assert_eq!(response_tcp.sequence_number, syn_ack_tcp.sequence_number.wrapping_add(1));
        assert_eq!(
            response_tcp.acknowledgement_number,
            0x1000_0001u32.wrapping_add(request_payload.len() as u32)
        );
        assert_eq!(response_tcp.flags, TCP_FLAG_ACK | TCP_FLAG_PSH | TCP_FLAG_FIN);
        assert!(http.starts_with("HTTP/1.0 200 OK\r\n"));
        assert!(http.contains("Content-Type: application/vnd.wap.xhtml+xml\r\n"));
        assert!(http.contains("Content-Length: "));
        assert!(http.contains("Connection: close\r\n"));
        assert!(http.contains("http://www.w3.org/1999/xhtml"));
        assert!(http.contains("Welcome to Nexus-BS"));
        assert!(
            response.len() <= 576,
            "TCP HTTP response should respect the negotiated N-PDU budget"
        );
    }

    #[test]
    fn wap_status_response_repeats_tcp_responses_for_duplicate_segments() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let syn = build_ipv4_tcp_npdu(
            [10, 0, 0, 2],
            endpoint.address,
            49152,
            endpoint.port,
            0x1000_0000,
            0,
            TCP_FLAG_SYN,
            2048,
            b"",
            0x3333,
            64,
        )
        .expect("TCP SYN N-PDU should build");

        let first_syn_ack = build_wap_status_response_npdu_optional_with_npdu_budget(&syn, endpoint, &policy(), &snapshot(), Some(576))
            .expect("first TCP SYN response should build")
            .expect("TCP SYN should require a response");
        let duplicate_syn_ack = build_wap_status_response_npdu_optional_with_npdu_budget(&syn, endpoint, &policy(), &snapshot(), Some(576))
            .expect("duplicate TCP SYN response should build")
            .expect("duplicate TCP SYN should require a response");
        assert_eq!(duplicate_syn_ack, first_syn_ack);

        let syn_ack_ip = parse_ipv4_packet(&first_syn_ack).expect("SYN-ACK IPv4 should parse");
        let syn_ack_tcp = parse_tcp_segment(syn_ack_ip.payload).expect("SYN-ACK TCP should parse");
        assert_eq!(syn_ack_tcp.flags, TCP_FLAG_SYN | TCP_FLAG_ACK);
        assert_eq!(syn_ack_tcp.acknowledgement_number, 0x1000_0001);

        let request_payload = b"GET /status.xhtml HTTP/1.1\r\nHost: 10.0.0.1:9200\r\nConnection: close\r\n\r\n";
        let get = build_ipv4_tcp_npdu(
            [10, 0, 0, 2],
            endpoint.address,
            49152,
            endpoint.port,
            0x1000_0001,
            syn_ack_tcp.sequence_number.wrapping_add(1),
            TCP_FLAG_ACK | TCP_FLAG_PSH,
            2048,
            request_payload,
            0x3334,
            64,
        )
        .expect("TCP GET N-PDU should build");

        let first_get_response =
            build_wap_status_response_npdu_optional_with_npdu_budget(&get, endpoint, &policy(), &snapshot(), Some(576))
                .expect("first TCP GET response should build")
                .expect("TCP GET should require a response");
        let duplicate_get_response =
            build_wap_status_response_npdu_optional_with_npdu_budget(&get, endpoint, &policy(), &snapshot(), Some(576))
                .expect("duplicate TCP GET response should build")
                .expect("duplicate TCP GET should require a response");
        assert_eq!(duplicate_get_response, first_get_response);
        assert!(
            first_get_response.len() <= 576,
            "duplicate-safe TCP HTTP response should respect the negotiated N-PDU budget"
        );

        let response_ip = parse_ipv4_packet(&first_get_response).expect("HTTP response IPv4 should parse");
        let response_tcp = parse_tcp_segment(response_ip.payload).expect("HTTP response TCP should parse");
        assert_eq!(response_tcp.sequence_number, syn_ack_tcp.sequence_number.wrapping_add(1));
        assert_eq!(
            response_tcp.acknowledgement_number,
            0x1000_0001u32.wrapping_add(request_payload.len() as u32)
        );
        assert_eq!(response_tcp.flags, TCP_FLAG_ACK | TCP_FLAG_PSH | TCP_FLAG_FIN);
    }

    #[test]
    fn wap_status_response_answers_mxp600_wtp_wsp_connect() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_udp_npdu(
            [10, 0, 0, 2],
            endpoint.address,
            49152,
            endpoint.port,
            &mxp600_wtp_wsp_connect_payload(),
            0x2222,
            64,
        )
        .expect("WSP Connect request N-PDU should build");

        let response =
            build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()).expect("WSP Connect response should build");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");

        assert_eq!(response_ip.source, endpoint.address);
        assert_eq!(response_ip.destination, [10, 0, 0, 2]);
        assert_eq!(response_udp.source_port, endpoint.port);
        assert_eq!(response_udp.destination_port, 49152);
        assert_eq!(&response_udp.payload[..3], &[WTP_RESULT_GTR_TTR, 0x93, 0xcc]);
        assert_eq!(response_udp.payload[3], WSP_PDU_CONNECT_REPLY);
        assert_eq!(response_udp.payload[4], 0x01);
        assert_eq!(
            &response_udp.payload[5..],
            &[
                0x10,
                0x00,
                0x03,
                WSP_CAP_CLIENT_SDU_SIZE,
                0x8a,
                0x78,
                0x03,
                WSP_CAP_SERVER_SDU_SIZE,
                0x8a,
                0x78,
                0x02,
                WSP_CAP_PROTOCOL_OPTIONS,
                0x00,
                0x02,
                WSP_CAP_METHOD_MOR,
                0x01,
                0x01,
                WSP_CAP_HEADER_CODE_PAGES,
            ]
        );
    }

    #[test]
    fn wap_status_response_negotiates_wsp_connect_reply_capabilities() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let mut capabilities = Vec::new();
        push_wsp_uintvar_capability(&mut capabilities, WSP_CAP_CLIENT_SDU_SIZE, 2000);
        push_wsp_uintvar_capability(&mut capabilities, WSP_CAP_SERVER_SDU_SIZE, 1600);
        push_wsp_octets_capability(&mut capabilities, WSP_CAP_PROTOCOL_OPTIONS, &[0xf8]);
        push_wsp_octets_capability(&mut capabilities, WSP_CAP_METHOD_MOR, &[3]);
        push_wsp_octets_capability(&mut capabilities, WSP_CAP_EXTENDED_METHODS, b"\x50STATUS\0\x70POSTX\0");
        push_wsp_octets_capability(&mut capabilities, WSP_CAP_HEADER_CODE_PAGES, b"\x78x-up-1\0");
        let request_payload = build_wtp_wsp_connect_payload(0x1234, &capabilities);
        let request = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint.address, 49152, endpoint.port, &request_payload, 0x2222, 64)
            .expect("WSP Connect request N-PDU should build");

        let response =
            build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()).expect("WSP Connect response should build");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");

        assert_eq!(&response_udp.payload[..3], &[WTP_RESULT_GTR_TTR, 0x92, 0x34]);
        assert_eq!(response_udp.payload[3], WSP_PDU_CONNECT_REPLY);
        assert_eq!(response_udp.payload[4], 0x01);
        let (capabilities_len, cap_len_octets) =
            read_uintvar(&response_udp.payload[5..]).expect("ConnectReply capabilities length should parse");
        let headers_len_start = 5 + cap_len_octets;
        let (headers_len, headers_len_octets) =
            read_uintvar(&response_udp.payload[headers_len_start..]).expect("ConnectReply headers length should parse");
        let capabilities_start = headers_len_start + headers_len_octets;
        let capabilities_end = capabilities_start + capabilities_len;
        assert_eq!(headers_len, 0);
        assert_eq!(capabilities_end, response_udp.payload.len());
        let negotiated = parse_wsp_capabilities(&response_udp.payload[capabilities_start..capabilities_end])
            .expect("ConnectReply capabilities should parse");

        assert_eq!(
            negotiated
                .iter()
                .find(|cap| cap.id == WSP_CAP_CLIENT_SDU_SIZE)
                .and_then(|cap| read_uintvar(&cap.parameters).map(|(value, _)| value)),
            Some(WSP_CONNECT_REPLY_CLIENT_SDU_SIZE_BYTES)
        );
        assert_eq!(
            negotiated
                .iter()
                .find(|cap| cap.id == WSP_CAP_SERVER_SDU_SIZE)
                .and_then(|cap| read_uintvar(&cap.parameters).map(|(value, _)| value)),
            Some(WSP_CONNECT_REPLY_SERVER_SDU_SIZE_BYTES)
        );
        assert_eq!(
            negotiated
                .iter()
                .find(|cap| cap.id == WSP_CAP_PROTOCOL_OPTIONS)
                .map(|cap| cap.parameters.as_slice()),
            Some(&[0x00][..])
        );
        assert_eq!(
            negotiated
                .iter()
                .find(|cap| cap.id == WSP_CAP_METHOD_MOR)
                .map(|cap| cap.parameters.as_slice()),
            Some(&[0x01][..])
        );
        assert_eq!(
            negotiated
                .iter()
                .find(|cap| cap.id == WSP_CAP_EXTENDED_METHODS)
                .map(|cap| cap.parameters.as_slice()),
            Some(&[0x50][..])
        );
        assert_eq!(
            negotiated
                .iter()
                .find(|cap| cap.id == WSP_CAP_HEADER_CODE_PAGES)
                .map(|cap| cap.parameters.as_slice()),
            Some(&[][..]),
            "ConnectReply should explicitly decline requested extension header code pages"
        );
    }

    #[test]
    fn wap_status_response_suppresses_wtp_ack_control_pdu() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_udp_npdu(
            [10, 0, 0, 2],
            endpoint.address,
            49152,
            endpoint.port,
            &[0x18, 0x13, 0xcc],
            0x2222,
            64,
        )
        .expect("WTP ACK request N-PDU should build");

        assert_eq!(
            build_wap_status_response_npdu_optional_with_npdu_budget(&request, endpoint, &policy(), &snapshot(), Some(576)),
            Ok(None)
        );
        assert_eq!(
            build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()),
            Err(WapIpError::NoResponseRequired)
        );
    }

    #[test]
    fn wap_status_response_suppresses_wtp_abort_control_pdu() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        for abort_payload in [&[0x20, 0x13, 0xcc, 0x01][..], &[0x21, 0x13, 0xcc, 0xe0][..]] {
            let request = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint.address, 49152, endpoint.port, abort_payload, 0x2222, 64)
                .expect("WTP Abort request N-PDU should build");

            assert_eq!(
                build_wap_status_response_npdu_optional_with_npdu_budget(&request, endpoint, &policy(), &snapshot(), Some(576)),
                Ok(None)
            );
            assert_eq!(
                build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()),
                Err(WapIpError::NoResponseRequired)
            );
        }
    }

    #[test]
    fn wap_status_response_answers_wtp_wsp_get_with_xhtml_reply() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request_payload = build_wtp_wsp_get_payload(0x1234, "/status.xhtml");
        let request = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint.address, 49152, endpoint.port, &request_payload, 0x2222, 64)
            .expect("WSP GET request N-PDU should build");

        let response = build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()).expect("WSP GET response should build");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");

        assert_eq!(
            &response_udp.payload[..7],
            [
                WTP_RESULT_GTR_TTR,
                0x92,
                0x34,
                WSP_PDU_REPLY,
                WSP_STATUS_OK,
                0x01,
                WSP_CT_APP_VND_WAP_XHTML_XML,
            ]
        );
        assert_eq!(response_udp.payload[5], 1, "WSP Reply HeadersLen should contain only ContentType");
        assert_eq!(
            response_udp.payload[6], WSP_CT_APP_VND_WAP_XHTML_XML,
            "WSP Reply ContentType should be application/vnd.wap.xhtml+xml as a short-integer"
        );
        assert!(
            response_udp.payload[3..].len() <= WSP_CONNECT_REPLY_CLIENT_SDU_SIZE_BYTES,
            "WSP Reply SDU should not exceed negotiated Client-SDU-Size: {} bytes",
            response_udp.payload[3..].len()
        );
        assert!(
            response_udp.payload.len() <= DEFAULT_WAP_WSP_STATUS_MAX_BYTES + 3 + WSP_REPLY_FIXED_HEADER_BYTES,
            "WSP response payload should fit current AL delivery budget: {} bytes",
            response_udp.payload.len()
        );
        assert!(
            response.len() <= IPV4_UDP_HEADER_BYTES + DEFAULT_WAP_WSP_STATUS_MAX_BYTES + 3 + WSP_REPLY_FIXED_HEADER_BYTES,
            "WSP response N-PDU should stay small enough for AL delivery: {} bytes",
            response.len()
        );
        let page = std::str::from_utf8(&response_udp.payload[7..]).expect("WSP XHTML body should be UTF-8");
        assert!(page.contains("http://www.w3.org/1999/xhtml"));
        assert!(page.contains("Welcome to Nexus-BS"));
        assert!(!page.contains("<wml"));
    }

    #[test]
    fn wap_status_response_keeps_wtp_result_rid_clear_for_retransmitted_get() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let mut request_payload = build_wtp_wsp_get_payload(0x1234, "/status.xhtml");
        request_payload[0] |= WTP_RID_FLAG;
        let request = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint.address, 49152, endpoint.port, &request_payload, 0x2222, 64)
            .expect("WSP GET request N-PDU should build");

        let response = build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()).expect("WSP GET response should build");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");

        assert_eq!(
            &response_udp.payload[..7],
            [
                WTP_RESULT_GTR_TTR,
                0x92,
                0x34,
                WSP_PDU_REPLY,
                WSP_STATUS_OK,
                0x01,
                WSP_CT_APP_VND_WAP_XHTML_XML,
            ]
        );
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
