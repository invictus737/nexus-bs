// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original TETRA SNDCP/IP primitive helpers.

use tetra_core::BitBuffer;

pub const IPV4_PROTOCOL_UDP: u8 = 17;
const IPV4_MIN_HEADER_LEN: usize = 20;
const UDP_HEADER_LEN: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpPrimitiveError {
    NpduNotOctetAligned { bits: usize },
    Ipv4TooShort { len: usize },
    UnsupportedIpv4Version { version: u8 },
    Ipv4HeaderTooShort { ihl_words: u8 },
    Ipv4TotalLengthTooShort { total_length: usize, header_len: usize },
    Ipv4TotalLengthExceedsBuffer { total_length: usize, buffer_len: usize },
    UdpTooShort { len: usize },
    UdpLengthTooShort { udp_length: usize },
    UdpLengthExceedsPayload { udp_length: usize, payload_len: usize },
    PayloadTooLarge { len: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv4Packet<'a> {
    pub dscp_ecn: u8,
    pub identification: u16,
    pub flags_fragment: u16,
    pub ttl: u8,
    pub protocol: u8,
    pub source: [u8; 4],
    pub destination: [u8; 4],
    pub payload: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpDatagram<'a> {
    pub source_port: u16,
    pub destination_port: u16,
    pub checksum: u16,
    pub payload: &'a [u8],
}

pub fn bitbuffer_npdu_octets(n_pdu: &BitBuffer) -> Result<Vec<u8>, IpPrimitiveError> {
    let bits = n_pdu.get_len();
    if bits % 8 != 0 {
        return Err(IpPrimitiveError::NpduNotOctetAligned { bits });
    }

    let mut n_pdu = BitBuffer::from_bitbuffer(n_pdu);
    let mut octets = Vec::with_capacity(bits / 8);
    for _ in 0..(bits / 8) {
        let byte = n_pdu
            .read_bits(8)
            .expect("byte-aligned BitBuffer length was checked before reading");
        octets.push(byte as u8);
    }
    Ok(octets)
}

pub fn parse_ipv4_packet(packet: &[u8]) -> Result<Ipv4Packet<'_>, IpPrimitiveError> {
    if packet.len() < IPV4_MIN_HEADER_LEN {
        return Err(IpPrimitiveError::Ipv4TooShort { len: packet.len() });
    }

    let version = packet[0] >> 4;
    if version != 4 {
        return Err(IpPrimitiveError::UnsupportedIpv4Version { version });
    }

    let ihl_words = packet[0] & 0x0f;
    if ihl_words < 5 {
        return Err(IpPrimitiveError::Ipv4HeaderTooShort { ihl_words });
    }
    let header_len = ihl_words as usize * 4;
    if packet.len() < header_len {
        return Err(IpPrimitiveError::Ipv4TooShort { len: packet.len() });
    }

    let total_length = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    if total_length < header_len {
        return Err(IpPrimitiveError::Ipv4TotalLengthTooShort { total_length, header_len });
    }
    if total_length > packet.len() {
        return Err(IpPrimitiveError::Ipv4TotalLengthExceedsBuffer {
            total_length,
            buffer_len: packet.len(),
        });
    }

    Ok(Ipv4Packet {
        dscp_ecn: packet[1],
        identification: u16::from_be_bytes([packet[4], packet[5]]),
        flags_fragment: u16::from_be_bytes([packet[6], packet[7]]),
        ttl: packet[8],
        protocol: packet[9],
        source: [packet[12], packet[13], packet[14], packet[15]],
        destination: [packet[16], packet[17], packet[18], packet[19]],
        payload: &packet[header_len..total_length],
    })
}

pub fn parse_udp_datagram(payload: &[u8]) -> Result<UdpDatagram<'_>, IpPrimitiveError> {
    if payload.len() < UDP_HEADER_LEN {
        return Err(IpPrimitiveError::UdpTooShort { len: payload.len() });
    }

    let udp_length = u16::from_be_bytes([payload[4], payload[5]]) as usize;
    if udp_length < UDP_HEADER_LEN {
        return Err(IpPrimitiveError::UdpLengthTooShort { udp_length });
    }
    if udp_length > payload.len() {
        return Err(IpPrimitiveError::UdpLengthExceedsPayload {
            udp_length,
            payload_len: payload.len(),
        });
    }

    Ok(UdpDatagram {
        source_port: u16::from_be_bytes([payload[0], payload[1]]),
        destination_port: u16::from_be_bytes([payload[2], payload[3]]),
        checksum: u16::from_be_bytes([payload[6], payload[7]]),
        payload: &payload[UDP_HEADER_LEN..udp_length],
    })
}

pub fn build_ipv4_udp_npdu(
    source: [u8; 4],
    destination: [u8; 4],
    source_port: u16,
    destination_port: u16,
    payload: &[u8],
    identification: u16,
    ttl: u8,
) -> Result<Vec<u8>, IpPrimitiveError> {
    let udp_len = UDP_HEADER_LEN
        .checked_add(payload.len())
        .ok_or(IpPrimitiveError::PayloadTooLarge { len: payload.len() })?;
    let total_len = IPV4_MIN_HEADER_LEN
        .checked_add(udp_len)
        .ok_or(IpPrimitiveError::PayloadTooLarge { len: payload.len() })?;
    if udp_len > u16::MAX as usize || total_len > u16::MAX as usize {
        return Err(IpPrimitiveError::PayloadTooLarge { len: payload.len() });
    }

    let mut packet = Vec::with_capacity(total_len);
    packet.push(0x45);
    packet.push(0);
    packet.extend_from_slice(&(total_len as u16).to_be_bytes());
    packet.extend_from_slice(&identification.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.push(ttl);
    packet.push(IPV4_PROTOCOL_UDP);
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(&source);
    packet.extend_from_slice(&destination);

    let checksum = ipv4_header_checksum(&packet[..IPV4_MIN_HEADER_LEN]);
    packet[10..12].copy_from_slice(&checksum.to_be_bytes());

    packet.extend_from_slice(&source_port.to_be_bytes());
    packet.extend_from_slice(&destination_port.to_be_bytes());
    packet.extend_from_slice(&(udp_len as u16).to_be_bytes());
    // IPv4 permits UDP checksum zero. Keep this primitive deterministic until
    // a terminal profile requires pseudo-header checksum generation.
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(payload);
    Ok(packet)
}

pub fn ipv4_header_checksum(header: &[u8]) -> u16 {
    let mut sum = 0u32;
    for chunk in header.chunks(2) {
        let word = match chunk {
            [hi, lo] => u16::from_be_bytes([*hi, *lo]) as u32,
            [hi] => (*hi as u32) << 8,
            _ => 0,
        };
        sum = sum.wrapping_add(word);
        while sum > 0xffff {
            sum = (sum & 0xffff) + (sum >> 16);
        }
    }
    !(sum as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitbuffer_npdu_octets_requires_byte_alignment() {
        let mut bits = BitBuffer::new(9);
        bits.write_bits(0x45, 8);
        bits.write_bits(1, 1);
        bits.seek(0);

        assert_eq!(bitbuffer_npdu_octets(&bits), Err(IpPrimitiveError::NpduNotOctetAligned { bits: 9 }));
    }

    #[test]
    fn ipv4_udp_npdu_round_trips_wap_payload_bytes() {
        let payload = b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>Nexus-BS</p></body></html>";
        let packet =
            build_ipv4_udp_npdu([10, 0, 0, 1], [10, 0, 0, 226], 9200, 9200, payload, 0x1234, 64).expect("IPv4/UDP N-PDU should build");

        let ipv4 = parse_ipv4_packet(&packet).expect("IPv4 packet should parse");
        assert_eq!(ipv4.source, [10, 0, 0, 1]);
        assert_eq!(ipv4.destination, [10, 0, 0, 226]);
        assert_eq!(ipv4.identification, 0x1234);
        assert_eq!(ipv4.ttl, 64);
        assert_eq!(ipv4.protocol, IPV4_PROTOCOL_UDP);
        assert_eq!(ipv4_header_checksum(&packet[..IPV4_MIN_HEADER_LEN]), 0);

        let udp = parse_udp_datagram(ipv4.payload).expect("UDP datagram should parse");
        assert_eq!(udp.source_port, 9200);
        assert_eq!(udp.destination_port, 9200);
        assert_eq!(udp.checksum, 0);
        assert_eq!(udp.payload, payload);
    }

    #[test]
    fn bitbuffer_npdu_octets_feeds_ipv4_parser() {
        let packet = build_ipv4_udp_npdu([192, 0, 2, 1], [192, 0, 2, 2], 49152, 9200, b"wap", 7, 32).expect("IPv4/UDP N-PDU should build");
        let mut n_pdu = BitBuffer::new(packet.len() * 8);
        for byte in &packet {
            n_pdu.write_bits(*byte as u64, 8);
        }
        n_pdu.seek(0);

        let octets = bitbuffer_npdu_octets(&n_pdu).expect("byte-aligned N-PDU should become octets");
        let ipv4 = parse_ipv4_packet(&octets).expect("IPv4 packet should parse from N-PDU octets");
        let udp = parse_udp_datagram(ipv4.payload).expect("UDP payload should parse");
        assert_eq!(udp.payload, b"wap");
    }

    #[test]
    fn ipv4_parser_rejects_invalid_lengths() {
        assert_eq!(parse_ipv4_packet(&[0u8; 19]), Err(IpPrimitiveError::Ipv4TooShort { len: 19 }));

        let mut packet = build_ipv4_udp_npdu([1, 1, 1, 1], [2, 2, 2, 2], 1, 2, b"x", 0, 1).unwrap();
        packet[2..4].copy_from_slice(&19u16.to_be_bytes());
        assert_eq!(
            parse_ipv4_packet(&packet),
            Err(IpPrimitiveError::Ipv4TotalLengthTooShort {
                total_length: 19,
                header_len: 20
            })
        );
    }

    #[test]
    fn udp_parser_rejects_invalid_lengths() {
        assert_eq!(parse_udp_datagram(&[0u8; 7]), Err(IpPrimitiveError::UdpTooShort { len: 7 }));

        let mut udp = vec![0u8; UDP_HEADER_LEN];
        udp[4..6].copy_from_slice(&7u16.to_be_bytes());
        assert_eq!(parse_udp_datagram(&udp), Err(IpPrimitiveError::UdpLengthTooShort { udp_length: 7 }));
    }
}
