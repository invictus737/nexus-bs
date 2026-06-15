// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original TETRA SNDCP SN-UNITDATA primitive bridge.

use tetra_core::BitBuffer;
use tetra_saps::sn::{SnPrimitiveError, SnUnitdataInd, SnUnitdataReq, sn_unitdata_ind};

pub const SN_PDU_TYPE_UNITDATA: u8 = 4;
pub const SNDCP_NO_COMPRESSION: u8 = 0;

const IPV4_VERSION: u8 = 4;
const IPV6_VERSION: u8 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPduKind {
    Ipv4,
    Ipv6,
    Other(u8),
    TooShort,
}

#[derive(Debug, Clone)]
pub struct SnUnitdata {
    pub nsapi: u8,
    pub pcomp: u8,
    pub dcomp: u8,
    pub n_pdu: BitBuffer,
    pub network_pdu_kind: NetworkPduKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SndcpUnitdataError {
    UnsupportedPduType(u8),
    UnsupportedNsapi(u8),
    UnsupportedCompression { pcomp: u8, dcomp: u8 },
    EmptyNPdu,
    Malformed(&'static str),
    Sn(SnPrimitiveError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SndcpEncodeError {
    UnsupportedNsapi(u8),
    UnsupportedCompression { pcomp: u8, dcomp: u8 },
    EmptyNPdu,
}

impl From<SnPrimitiveError> for SndcpUnitdataError {
    fn from(value: SnPrimitiveError) -> Self {
        SndcpUnitdataError::Sn(value)
    }
}

pub fn decode_sn_unitdata_pdu(sdu: &BitBuffer) -> Result<SnUnitdata, SndcpUnitdataError> {
    let mut sdu = BitBuffer::from_bitbuffer(sdu);
    if sdu.get_pos() != 0 {
        sdu.seek(0);
    }

    let Some(sn_pdu_type) = sdu.read_bits(4) else {
        return Err(SndcpUnitdataError::Malformed("sn_pdu_type"));
    };
    let sn_pdu_type = sn_pdu_type as u8;
    if sn_pdu_type != SN_PDU_TYPE_UNITDATA {
        return Err(SndcpUnitdataError::UnsupportedPduType(sn_pdu_type));
    }

    decode_sn_unitdata_body(&sdu)
}

pub fn decode_sn_unitdata_body(sdu: &BitBuffer) -> Result<SnUnitdata, SndcpUnitdataError> {
    // The caller has already consumed the SN PDU type. Table 28.43 then
    // carries NSAPI, PCOMP, DCOMP and the variable length N-PDU.
    let mut sdu = BitBuffer::from_bitbuffer_pos(sdu);

    let Some(nsapi) = sdu.read_bits(4) else {
        return Err(SndcpUnitdataError::Malformed("nsapi"));
    };
    let nsapi = nsapi as u8;
    if !(1..=14).contains(&nsapi) {
        return Err(SndcpUnitdataError::UnsupportedNsapi(nsapi));
    }

    let Some(pcomp) = sdu.read_bits(4) else {
        return Err(SndcpUnitdataError::Malformed("pcomp"));
    };
    let Some(dcomp) = sdu.read_bits(4) else {
        return Err(SndcpUnitdataError::Malformed("dcomp"));
    };
    let pcomp = pcomp as u8;
    let dcomp = dcomp as u8;

    if pcomp != SNDCP_NO_COMPRESSION || dcomp != SNDCP_NO_COMPRESSION {
        return Err(SndcpUnitdataError::UnsupportedCompression { pcomp, dcomp });
    }

    if sdu.get_len_remaining() == 0 {
        return Err(SndcpUnitdataError::EmptyNPdu);
    }

    let n_pdu = BitBuffer::from_bitbuffer_pos(&sdu);
    let network_pdu_kind = classify_network_pdu(&n_pdu);

    Ok(SnUnitdata {
        nsapi,
        pcomp,
        dcomp,
        n_pdu,
        network_pdu_kind,
    })
}

pub fn encode_sn_unitdata(nsapi: u8, pcomp: u8, dcomp: u8, n_pdu: &BitBuffer) -> Result<BitBuffer, SndcpEncodeError> {
    // EN 300 392-2 clause 28.4.4.14/table 28.43: SN-UNITDATA is
    // SN PDU type(4), NSAPI(4), PCOMP(4), DCOMP(4), then a variable N-PDU
    // whose length is defined by the lower layer PDU length.
    if !(1..=14).contains(&nsapi) {
        return Err(SndcpEncodeError::UnsupportedNsapi(nsapi));
    }
    if pcomp != SNDCP_NO_COMPRESSION || dcomp != SNDCP_NO_COMPRESSION {
        return Err(SndcpEncodeError::UnsupportedCompression { pcomp, dcomp });
    }
    if n_pdu.get_len() == 0 {
        return Err(SndcpEncodeError::EmptyNPdu);
    }

    let mut pdu = BitBuffer::new(16 + n_pdu.get_len());
    pdu.write_bits(SN_PDU_TYPE_UNITDATA as u64, 4);
    pdu.write_bits(nsapi as u64, 4);
    pdu.write_bits(pcomp as u64, 4);
    pdu.write_bits(dcomp as u64, 4);

    let mut n_pdu = BitBuffer::from_bitbuffer(n_pdu);
    n_pdu.seek(0);
    while let Some(bit) = n_pdu.read_bits(1) {
        pdu.write_bits(bit, 1);
    }
    pdu.seek(0);
    Ok(pdu)
}

pub fn sn_unitdata_ind_from_pdu(sdu: &BitBuffer) -> Result<SnUnitdataInd, SndcpUnitdataError> {
    let unitdata = decode_sn_unitdata_pdu(sdu)?;
    sn_unitdata_ind_from_decoded(unitdata)
}

pub fn sn_unitdata_ind_from_decoded(unitdata: SnUnitdata) -> Result<SnUnitdataInd, SndcpUnitdataError> {
    Ok(sn_unitdata_ind(unitdata.nsapi, unitdata.n_pdu)?)
}

pub fn sn_unitdata_req_to_pdu(req: &SnUnitdataReq) -> Result<BitBuffer, SndcpEncodeError> {
    encode_sn_unitdata(req.nsapi, SNDCP_NO_COMPRESSION, SNDCP_NO_COMPRESSION, &req.n_pdu)
}

fn classify_network_pdu(n_pdu: &BitBuffer) -> NetworkPduKind {
    let Some(version) = n_pdu.peek_bits(4) else {
        return NetworkPduKind::TooShort;
    };

    match version as u8 {
        IPV4_VERSION => NetworkPduKind::Ipv4,
        IPV6_VERSION => NetworkPduKind::Ipv6,
        other => NetworkPduKind::Other(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_saps::sn::{sn_unitdata_ind, sn_unitdata_req};

    #[test]
    fn sn_unitdata_pdu_round_trips_to_sn_sap_indication() {
        let n_pdu = BitBuffer::from_bytes(&[0x45, 0x00, 0x00, 0x14]);
        let pdu = encode_sn_unitdata(2, 0, 0, &n_pdu).expect("SN-UNITDATA should encode");

        let decoded = decode_sn_unitdata_pdu(&pdu).expect("SN-UNITDATA should decode");
        assert_eq!(decoded.nsapi, 2);
        assert_eq!(decoded.pcomp, 0);
        assert_eq!(decoded.dcomp, 0);
        assert_eq!(decoded.network_pdu_kind, NetworkPduKind::Ipv4);

        let ind = sn_unitdata_ind_from_pdu(&pdu).expect("decoded PDU should map to SN-SAP indication");
        assert_eq!(ind.nsapi, 2);
        assert_eq!(ind.n_pdu.to_bitstr(), n_pdu.to_bitstr());
    }

    #[test]
    fn sn_sap_request_encodes_to_sn_unitdata_pdu() {
        let n_pdu = BitBuffer::from_bytes(&[0x60, 0x00, 0x00, 0x00]);
        let req = sn_unitdata_req(3, 77, n_pdu.clone(), Some(2), None).expect("SN-SAP request should be valid");

        let pdu = sn_unitdata_req_to_pdu(&req).expect("SN-SAP request should encode");
        let decoded = decode_sn_unitdata_pdu(&pdu).expect("SN-UNITDATA should decode");

        assert_eq!(decoded.nsapi, 3);
        assert_eq!(decoded.network_pdu_kind, NetworkPduKind::Ipv6);
        assert_eq!(decoded.n_pdu.to_bitstr(), n_pdu.to_bitstr());
    }

    #[test]
    fn sn_unitdata_rejects_reserved_nsapi_compression_and_empty_npdu() {
        let n_pdu = BitBuffer::from_bytes(&[0x45]);

        assert_eq!(
            encode_sn_unitdata(0, 0, 0, &n_pdu).expect_err("reserved NSAPI should reject"),
            SndcpEncodeError::UnsupportedNsapi(0)
        );
        assert_eq!(
            encode_sn_unitdata(1, 1, 0, &n_pdu).expect_err("compression should reject"),
            SndcpEncodeError::UnsupportedCompression { pcomp: 1, dcomp: 0 }
        );
        assert_eq!(
            encode_sn_unitdata(1, 0, 0, &BitBuffer::new(0)).expect_err("empty N-PDU should reject"),
            SndcpEncodeError::EmptyNPdu
        );
    }

    #[test]
    fn sn_unitdata_decoder_rejects_unsupported_fields() {
        let n_pdu = BitBuffer::from_bytes(&[0x45]);

        let mut wrong_type = encode_sn_unitdata(1, 0, 0, &n_pdu).unwrap();
        wrong_type.write_bits(5, 4);
        wrong_type.seek(0);
        assert_eq!(
            decode_sn_unitdata_pdu(&wrong_type).expect_err("wrong SN PDU type should reject"),
            SndcpUnitdataError::UnsupportedPduType(5)
        );

        let mut reserved_nsapi = BitBuffer::new(24);
        reserved_nsapi.write_bits(SN_PDU_TYPE_UNITDATA as u64, 4);
        reserved_nsapi.write_bits(15, 4);
        reserved_nsapi.write_bits(0, 4);
        reserved_nsapi.write_bits(0, 4);
        reserved_nsapi.write_bits(0x45, 8);
        reserved_nsapi.seek(0);
        assert_eq!(
            decode_sn_unitdata_pdu(&reserved_nsapi).expect_err("reserved NSAPI should reject"),
            SndcpUnitdataError::UnsupportedNsapi(15)
        );

        let mut compressed = BitBuffer::new(24);
        compressed.write_bits(SN_PDU_TYPE_UNITDATA as u64, 4);
        compressed.write_bits(1, 4);
        compressed.write_bits(1, 4);
        compressed.write_bits(0, 4);
        compressed.write_bits(0x45, 8);
        compressed.seek(0);
        assert_eq!(
            decode_sn_unitdata_pdu(&compressed).expect_err("compression should reject"),
            SndcpUnitdataError::UnsupportedCompression { pcomp: 1, dcomp: 0 }
        );
    }

    #[test]
    fn decoded_sn_unitdata_maps_to_sn_sap_validation() {
        let ind = sn_unitdata_ind(4, BitBuffer::from_bytes(&[0x10])).expect("SN-SAP indication should build");
        let pdu = encode_sn_unitdata(ind.nsapi, 0, 0, &ind.n_pdu).expect("SN-UNITDATA should encode");

        let remapped = sn_unitdata_ind_from_pdu(&pdu).expect("PDU should map back to SN-SAP");

        assert_eq!(remapped.nsapi, ind.nsapi);
        assert_eq!(remapped.n_pdu.to_bitstr(), ind.n_pdu.to_bitstr());
    }
}
