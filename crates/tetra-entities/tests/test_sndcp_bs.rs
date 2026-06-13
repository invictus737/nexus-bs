// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

mod common;

use common::ComponentTest;
use tetra_config::bluestation::StackMode;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Sap, SsiType, TetraAddress, debug};
use tetra_entities::sndcp::sndcp_bs::{NetworkPduKind, SndcpDecode, SndcpEncodeError, decode_ltpd_sdu, encode_sn_unitdata};
use tetra_saps::SapMsg;
use tetra_saps::ltpd::LtpdMleUnitdataInd;
use tetra_saps::sapmsg::SapMsgInner;

fn build_ltpd_ind(sap: Sap, sdu: BitBuffer) -> SapMsg {
    SapMsg {
        sap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Sndcp,
        msg: SapMsgInner::LtpdMleUnitdataInd(LtpdMleUnitdataInd {
            sdu,
            endpoint_id: 1,
            link_id: 2,
            received_tetra_address: TetraAddress::new(1000001, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_sn_unitdata(nsapi: u8, pcomp: u8, dcomp: u8, n_pdu: &[u8]) -> BitBuffer {
    let mut sdu = BitBuffer::new(16 + n_pdu.len() * 8);
    sdu.write_bits(4, 4);
    sdu.write_bits(nsapi as u64, 4);
    sdu.write_bits(pcomp as u64, 4);
    sdu.write_bits(dcomp as u64, 4);
    for byte in n_pdu {
        sdu.write_bits(*byte as u64, 8);
    }
    sdu.seek(0);
    sdu
}

fn build_sn_pdu(sn_pdu_type: u8) -> BitBuffer {
    let mut sdu = BitBuffer::new(8);
    sdu.write_bits(sn_pdu_type as u64, 4);
    sdu.write_bits(0, 4);
    sdu.seek(0);
    sdu
}

#[test]
fn sndcp_encode_sn_unitdata_no_compression_round_trips_decoder() {
    // EN 300 392-2 clause 28.4.4.14/table 28.43 defines SN-UNITDATA as
    // SN PDU type, NSAPI, PCOMP, DCOMP and a lower-layer-length N-PDU with no
    // trailing O-bit. Keep the outbound helper aligned with the inbound
    // decoder before advertising a packet-data/WAP bearer.
    let mut n_pdu = BitBuffer::new(32);
    for byte in [0x45, 0x00, 0x00, 0x14] {
        n_pdu.write_bits(byte, 8);
    }
    n_pdu.seek(0);

    let encoded = encode_sn_unitdata(3, 0, 0, &n_pdu).expect("SN-UNITDATA encode should succeed");

    let SndcpDecode::Unitdata(unitdata) = decode_ltpd_sdu(&encoded) else {
        panic!("expected encoded SN-UNITDATA to decode");
    };

    assert_eq!(unitdata.nsapi, 3);
    assert_eq!(unitdata.pcomp, 0);
    assert_eq!(unitdata.dcomp, 0);
    assert_eq!(unitdata.network_pdu_kind, NetworkPduKind::Ipv4);
    assert_eq!(unitdata.n_pdu.to_bitstr(), n_pdu.to_bitstr());
}

#[test]
fn sndcp_encode_sn_unitdata_rejects_unsupported_fields() {
    let mut n_pdu = BitBuffer::new(8);
    n_pdu.write_bits(0x45, 8);
    n_pdu.seek(0);

    assert_encode_error(encode_sn_unitdata(0, 0, 0, &n_pdu), SndcpEncodeError::UnsupportedNsapi(0));
    assert_encode_error(encode_sn_unitdata(15, 0, 0, &n_pdu), SndcpEncodeError::UnsupportedNsapi(15));
    assert_encode_error(
        encode_sn_unitdata(3, 1, 0, &n_pdu),
        SndcpEncodeError::UnsupportedCompression { pcomp: 1, dcomp: 0 },
    );
    assert_encode_error(encode_sn_unitdata(3, 0, 0, &BitBuffer::new(0)), SndcpEncodeError::EmptyNPdu);
}

fn assert_encode_error(result: Result<BitBuffer, SndcpEncodeError>, expected: SndcpEncodeError) {
    match result {
        Err(err) => assert_eq!(err, expected),
        Ok(_) => panic!("expected encode error {expected:?}"),
    }
}

#[test]
fn sndcp_decode_sn_unitdata_no_compression_ipv4_npdu() {
    // EN 300 392-2 clause 28.4.4.14 defines SN-UNITDATA as SN PDU type,
    // NSAPI, PCOMP, DCOMP, then a lower-layer-length N-PDU. This is the
    // smallest WAP-capable bearer step SNDCP can safely implement: decode an
    // uncompressed IP N-PDU, without claiming any UDP/WAP handoff.
    let sdu = build_sn_unitdata(3, 0, 0, &[0x45, 0x00, 0x00, 0x14]);

    let SndcpDecode::Unitdata(unitdata) = decode_ltpd_sdu(&sdu) else {
        panic!("expected decoded SN-UNITDATA");
    };

    assert_eq!(unitdata.nsapi, 3);
    assert_eq!(unitdata.pcomp, 0);
    assert_eq!(unitdata.dcomp, 0);
    assert_eq!(unitdata.network_pdu_kind, NetworkPduKind::Ipv4);
    assert_eq!(unitdata.n_pdu.get_len(), 32);

    let mut n_pdu = BitBuffer::from_bitbuffer(&unitdata.n_pdu);
    assert_eq!(n_pdu.read_bits(8), Some(0x45));
}

#[test]
fn sndcp_decode_distinguishes_unsupported_packet_data_cases() {
    match decode_ltpd_sdu(&build_sn_pdu(5)) {
        SndcpDecode::UnsupportedPduType(5) => {}
        other => panic!("expected unsupported SN-DATA PDU type, got {:?}", other),
    }

    match decode_ltpd_sdu(&build_sn_unitdata(15, 0, 0, &[0x45])) {
        SndcpDecode::UnsupportedNsapi(15) => {}
        other => panic!("expected reserved NSAPI rejection, got {:?}", other),
    }

    match decode_ltpd_sdu(&build_sn_unitdata(3, 1, 0, &[0x45])) {
        SndcpDecode::UnsupportedCompression { pcomp: 1, dcomp: 0 } => {}
        other => panic!("expected unsupported compression rejection, got {:?}", other),
    }
}

#[test]
fn sndcp_ltpd_unitdata_drops_without_output_when_service_is_not_advertised() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Bs, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle]);

    // Table 18.26 service advertising remains false until a full bearer is
    // available, so even a syntactically supported SN-UNITDATA is dropped.
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_sn_unitdata(3, 0, 0, &[0x45])));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn sndcp_ltpd_unitdata_decodes_but_drops_without_sn_sap_handoff() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle]);

    // Direct config can exercise the local decoder, but SNDCP still has no
    // SN-SAP/IP/WAP handoff in this stack. The fail-safe result is no output.
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_sn_unitdata(3, 0, 0, &[0x45])));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn sndcp_unexpected_sap_drops_without_panic() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Bs, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle]);

    // A malformed internal route must not crash SNDCP or synthesize packet
    // data output while the bearer is unsupported.
    test.submit_message(build_ltpd_ind(Sap::LmmSap, build_sn_unitdata(3, 0, 0, &[0x45])));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}
