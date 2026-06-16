// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

mod common;

use common::ComponentTest;
use tetra_config::bluestation::{CfgWapIp, StackMode};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Layer2Service, Sap, SsiType, TetraAddress, debug};
use tetra_entities::sndcp::ip::{bitbuffer_npdu_octets, build_ipv4_udp_npdu, parse_ipv4_packet, parse_udp_datagram};
use tetra_entities::sndcp::pdp::{
    SndcpActivateAddressDemand, SndcpActivatePdpContextDemand, SndcpDeactivation, decode_activate_pdp_context_accept,
    decode_deactivate_pdp_context_demand, encode_activate_pdp_context_demand, encode_deactivate_pdp_context_accept,
    encode_deactivate_pdp_context_demand,
};
use tetra_entities::sndcp::sndcp_bs::{
    NetworkPduKind, SndcpDecode, SndcpEncodeError, SndcpRuntimeHandoffDecision, SndcpRuntimeHandoffPolicy, SndcpRuntimePduClass,
    decode_ltpd_sdu, encode_sn_unitdata,
};
use tetra_entities::sndcp::transfer::{
    SN_PDU_TYPE_END_OF_DATA, SndcpDataTransmitRequest, SndcpDataTransmitResponseResult, SndcpEndOfData, SndcpNotSupported,
    SndcpTransferControl, decode_data_transmit_response, encode_data_transmit_request, encode_end_of_data, encode_not_supported,
};
use tetra_entities::sndcp::unitdata::decode_sn_unitdata_pdu;
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;
use tetra_saps::SapMsg;
use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
use tetra_saps::ltpd::{LtpdMleConfigureInd, LtpdMleConfigureReq, LtpdMleReportInd, LtpdMleUnitdataInd, LtpdMleUnitdataReq};
use tetra_saps::sapmsg::SapMsgInner;
use tetra_saps::sn::{SnAddress, SnPacketDataMsType};
use tetra_saps::tla::TlaTlUnitdataIndBl;

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

fn build_ltpd_report_ind(handle: i32, transfer_result: i32) -> SapMsg {
    SapMsg {
        sap: Sap::TlpdSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Sndcp,
        msg: SapMsgInner::LtpdMleReportInd(LtpdMleReportInd { handle, transfer_result }),
    }
}

fn build_ltpd_configure_req() -> SapMsg {
    SapMsg {
        sap: Sap::TlpdSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Sndcp,
        msg: SapMsgInner::LtpdMleConfigureReq(LtpdMleConfigureReq {
            chan_change_accepted: Some(false),
            chan_change_handle: 9,
            call_release: -1,
            endpoint_id: 1,
            encryption_flag: false,
            ms_default_data_prio: -1,
            layer2_data_prio_lifetime: -1,
            layer2_data_prio_signalling_delay: -1,
            data_prio_random_access_delay_factor: -1,
            data_class_info: -1,
            schedule_repetition_info: -1,
            sndcp_status: 0,
        }),
    }
}

fn build_ltpd_configure_ind() -> SapMsg {
    SapMsg {
        sap: Sap::TlpdSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Sndcp,
        msg: SapMsgInner::LtpdMleConfigureInd(LtpdMleConfigureInd {
            endpoint_id: 1,
            chan_change_responce_required: true,
            chan_change_handle: 10,
            reason_for_config_indication: 3,
            conflicting_endpoint_id: 2,
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

fn build_tla_sndcp_unitdata_ind(sndcp_sdu: BitBuffer) -> SapMsg {
    let sdu_len = sndcp_sdu.get_len();
    let mut tl_sdu = BitBuffer::new(3 + sdu_len);
    tl_sdu.write_bits(MleProtocolDiscriminator::Sndcp.into_raw(), 3);
    let mut sndcp_sdu = BitBuffer::from_bitbuffer(&sndcp_sdu);
    sndcp_sdu.seek(0);
    tl_sdu.copy_bits(&mut sndcp_sdu, sdu_len);
    tl_sdu.seek(0);

    SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Mle,
        msg: SapMsgInner::TlaTlUnitdataIndBl(TlaTlUnitdataIndBl {
            main_address: TetraAddress::new(1000001, SsiType::Issi),
            link_id: 2,
            endpoint_id: 1,
            new_endpoint_id: None,
            css_endpoint_id: None,
            tl_sdu: Some(tl_sdu),
            scrambling_code: 0,
            fcs_flag: false,
            air_interface_encryption: 0,
            chan_change_resp_req: false,
            chan_change_handle: None,
            chan_info: None,
            report: None,
        }),
    }
}

fn build_sn_pdu(sn_pdu_type: u8) -> BitBuffer {
    let mut sdu = BitBuffer::new(8);
    sdu.write_bits(sn_pdu_type as u64, 4);
    sdu.write_bits(0, 4);
    sdu.seek(0);
    sdu
}

fn build_dynamic_ipv4_activation_demand(nsapi: u8) -> BitBuffer {
    encode_activate_pdp_context_demand(&SndcpActivatePdpContextDemand {
        sndcp_version: 1,
        nsapi,
        address: SndcpActivateAddressDemand::Ipv4Dynamic,
        packet_data_ms_type: SnPacketDataMsType::TypeAParallel,
        pcomp_negotiation: 0,
    })
    .expect("minimal IPv4 PDP activation demand should encode")
}

fn build_wap_status_sn_unitdata(nsapi: u8) -> BitBuffer {
    let n_pdu =
        build_ipv4_udp_npdu([10, 0, 0, 82], [10, 0, 0, 1], 49_152, 9200, b"", 0x2260, 32).expect("WAP IPv4/UDP probe N-PDU should build");
    build_sn_unitdata(nsapi, 0, 0, &n_pdu)
}

fn build_wap_status_sn_unitdata_from(nsapi: u8, source: [u8; 4], payload: &[u8]) -> BitBuffer {
    let n_pdu =
        build_ipv4_udp_npdu(source, [10, 0, 0, 1], 49_152, 9200, payload, 0x2260, 32).expect("WAP IPv4/UDP probe N-PDU should build");
    build_sn_unitdata(nsapi, 0, 0, &n_pdu)
}

fn enable_wap_ip_status_mvp(config: &mut tetra_config::bluestation::StackConfig) {
    config.cell.sndcp_service = true;
    config.cell.wap_ip = Some(CfgWapIp {
        enabled: true,
        address: [10, 0, 0, 1],
        port: 9200,
        response_ttl: 32,
        dynamic_pool_prefix: [10, 0, 0],
        dynamic_pool_first_host: 2,
        dynamic_pool_last_host: 254,
        allow_static_ipv4: true,
        accept_empty_probe: true,
        accept_root_path: true,
        accept_status_path: true,
        accept_status_wml_path: true,
        max_request_payload_bytes: 128,
        assume_pdch_ready_after_data_transmit: true,
    });
}

fn submit_wap_packet_data_sequence(test: &mut ComponentTest) {
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_dynamic_ipv4_activation_demand(2)));
    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
    ));
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_wap_status_sn_unitdata(2)));
}

fn submit_wap_packet_data_sequence_through_mle(test: &mut ComponentTest) {
    test.submit_message(build_tla_sndcp_unitdata_ind(build_dynamic_ipv4_activation_demand(2)));
    test.submit_message(build_tla_sndcp_unitdata_ind(
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
    ));
    test.submit_message(build_tla_sndcp_unitdata_ind(build_wap_status_sn_unitdata(2)));
}

fn assert_no_runtime_side_effects(test: &mut ComponentTest) {
    assert_eq!(test.router.get_msgqueue_len(), 0);
    assert!(test.dump_sinks().is_empty());
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
fn sndcp_decode_activate_pdp_context_demand_dynamic_ipv4() {
    // EN 300 392-2 clause 28.3.3.5 requires PDP context activation before
    // packet data transfer. The BS-side decoder can recognize the activation
    // demand, while runtime handling remains fail-closed until the full bearer
    // is implemented.
    let sdu = build_dynamic_ipv4_activation_demand(2);

    let SndcpDecode::ActivatePdpContextDemand(demand) = decode_ltpd_sdu(&sdu) else {
        panic!("expected decoded SN-ACTIVATE PDP CONTEXT DEMAND");
    };

    assert_eq!(demand.nsapi, 2);
    assert_eq!(demand.sndcp_version, 1);
    assert_eq!(demand.address, SndcpActivateAddressDemand::Ipv4Dynamic);
    assert_eq!(demand.packet_data_ms_type, SnPacketDataMsType::TypeAParallel);
}

#[test]
fn sndcp_decode_ltpd_sdu_accepts_mle_demux_cursor() {
    // EN 300 392-2 clause 18.4.1.3 routes service PDUs by a 3-bit MLE
    // protocol discriminator. The MLE entity consumes that discriminator
    // before delivering the SNDCP SDU on LTPD-SAP, so SNDCP decoding must be
    // relative to the current BitBuffer cursor.
    let sndcp_sdu = build_dynamic_ipv4_activation_demand(2);
    let mut tl_sdu = BitBuffer::new(3 + sndcp_sdu.get_len());
    tl_sdu.write_bits(MleProtocolDiscriminator::Sndcp.into_raw(), 3);
    let mut sndcp_sdu = BitBuffer::from_bitbuffer(&sndcp_sdu);
    sndcp_sdu.seek(0);
    let sndcp_len = sndcp_sdu.get_len();
    tl_sdu.copy_bits(&mut sndcp_sdu, sndcp_len);
    tl_sdu.seek(0);
    assert_eq!(tl_sdu.read_bits(3), Some(MleProtocolDiscriminator::Sndcp.into_raw() as u64));

    let SndcpDecode::ActivatePdpContextDemand(demand) = decode_ltpd_sdu(&tl_sdu) else {
        panic!("expected cursor-relative SN-ACTIVATE PDP CONTEXT DEMAND");
    };

    assert_eq!(demand.nsapi, 2);
}

#[test]
fn sndcp_decode_deactivate_pdp_context_demand_single_nsapi() {
    let deactivation = SndcpDeactivation::Nsapi(2);
    let sdu = encode_deactivate_pdp_context_demand(&deactivation).expect("deactivation demand should encode");

    let SndcpDecode::DeactivatePdpContextDemand(decoded) = decode_ltpd_sdu(&sdu) else {
        panic!("expected decoded SN-DEACTIVATE PDP CONTEXT DEMAND");
    };

    assert_eq!(decoded, deactivation);
    assert_eq!(decode_deactivate_pdp_context_demand(&sdu), Ok(deactivation));
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
fn sndcp_runtime_handoff_policy_is_separate_and_disabled_by_default() {
    // EN 300 392-2 table 18.26 service advertising is not enough to make a
    // packet-data bearer safe. Runtime WAP/IP handoff needs a separate gate so
    // future PDP/SN-SAP/PDCH plumbing cannot emit traffic by accident.
    let policy = SndcpRuntimeHandoffPolicy::default();

    let activation = decode_ltpd_sdu(&build_dynamic_ipv4_activation_demand(2));
    assert_eq!(
        policy.decide_ltpd_unitdata_ind(false, &activation),
        SndcpRuntimeHandoffDecision::DropServiceUnavailable
    );
    assert_eq!(
        policy.decide_ltpd_unitdata_ind(true, &activation),
        SndcpRuntimeHandoffDecision::DropRuntimeHandoffDisabled {
            pdu: SndcpRuntimePduClass::PdpActivationDemand
        }
    );

    let ready = decode_ltpd_sdu(
        &encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
    );
    assert_eq!(
        policy.decide_ltpd_unitdata_ind(true, &ready),
        SndcpRuntimeHandoffDecision::DropRuntimeHandoffDisabled {
            pdu: SndcpRuntimePduClass::TransferControl
        }
    );

    let unitdata = decode_ltpd_sdu(&build_wap_status_sn_unitdata(2));
    assert_eq!(
        policy.decide_ltpd_unitdata_ind(true, &unitdata),
        SndcpRuntimeHandoffDecision::DropRuntimeHandoffDisabled {
            pdu: SndcpRuntimePduClass::Unitdata
        }
    );
}

#[test]
fn sndcp_ltpd_wap_packet_data_sequence_drops_without_service_advertising() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Bs, None);
    test.populate_entities(
        vec![TetraEntity::Sndcp],
        vec![
            TetraEntity::Mle,
            TetraEntity::Llc,
            TetraEntity::Cmce,
            TetraEntity::User,
            TetraEntity::Brew,
        ],
    );

    // EN 300 392-2 clauses 17.3.5, 28.3.3.5, 28.4.4.5 and 28.4.4.14
    // describe the LTPD/SNDCP activation, READY request and SN-UNITDATA path.
    // With table 18.26 SNDCP service still unadvertised, the live runtime must
    // not synthesize PDP activation responses, WAP/IP output or lower-channel
    // allocation side effects.
    submit_wap_packet_data_sequence(&mut test);
    test.deliver_all_messages();

    assert_no_runtime_side_effects(&mut test);
}

#[test]
fn sndcp_ltpd_wap_packet_data_sequence_drops_even_with_direct_service_flag() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(
        vec![TetraEntity::Sndcp],
        vec![
            TetraEntity::Mle,
            TetraEntity::Llc,
            TetraEntity::Cmce,
            TetraEntity::User,
            TetraEntity::Brew,
        ],
    );

    // A direct StackConfig can exercise decoders behind the parser/sysinfo
    // fail-closed gates, but runtime SNDCP still has no PDP table, SN-SAP,
    // WAP/IP handoff or MLE channel-allocation wiring.
    submit_wap_packet_data_sequence(&mut test);
    test.deliver_all_messages();

    assert_no_runtime_side_effects(&mut test);
}

#[test]
fn sndcp_wap_ip_mvp_answers_activation_ready_and_wml_unitdata_when_enabled() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    // EN 300 392-2 clauses 28.3.3.5, 28.4.4.5 and 28.4.4.14: PDP
    // activation establishes the IPv4 context, SN-DATA TRANSMIT moves the
    // subscriber into READY, and SN-UNITDATA carries the WAP/IP N-PDU.
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_dynamic_ipv4_activation_demand(2)));
    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
    ));
    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        build_wap_status_sn_unitdata_from(2, [10, 0, 0, 2], b"GET /status.xhtml HTTP/1.0\r\n\r\n"),
    ));
    test.deliver_all_messages();

    let mut ltpd_reqs: Vec<LtpdMleUnitdataReq> = test
        .dump_sinks()
        .into_iter()
        .filter(|msg| msg.sap == Sap::TlpdSap && msg.src == TetraEntity::Sndcp && msg.dest == TetraEntity::Mle)
        .filter_map(|msg| match msg.msg {
            SapMsgInner::LtpdMleUnitdataReq(req) => Some(req),
            _ => None,
        })
        .collect();

    assert_eq!(ltpd_reqs.len(), 3);

    let accept = decode_activate_pdp_context_accept(&ltpd_reqs[0].sdu).expect("activation response should decode");
    assert_eq!(accept.nsapi, 2);
    assert_eq!(accept.assigned_address, Some(SnAddress::Ipv4([10, 0, 0, 2])));
    assert_eq!(ltpd_reqs[0].layer2service, Layer2Service::Acknowledged);
    assert!(!ltpd_reqs[0].packet_data_flag);

    let ready = decode_data_transmit_response(&ltpd_reqs[1].sdu).expect("ready response should decode");
    assert_eq!(ready.nsapi, 2);
    assert_eq!(ready.result, SndcpDataTransmitResponseResult::Accepted);
    assert_eq!(ltpd_reqs[1].layer2service, Layer2Service::Acknowledged);
    assert!(!ltpd_reqs[1].packet_data_flag);
    let ready_alloc = ltpd_reqs[1]
        .chan_alloc
        .as_ref()
        .expect("accepted SN-DATA TRANSMIT RESPONSE should carry the MVP PDCH allocation");
    assert_eq!(ready_alloc.usage, Some(4));
    assert_eq!(ready_alloc.carrier, None);
    assert_eq!(ready_alloc.timeslots, [false, true, false, false]);
    assert_eq!(ready_alloc.alloc_type, ChanAllocType::Replace);
    assert_eq!(ready_alloc.ul_dl_assigned, UlDlAssignment::Both);

    assert_eq!(ltpd_reqs[2].layer2service, Layer2Service::Unacknowledged);
    assert!(ltpd_reqs[2].packet_data_flag);
    assert!(ltpd_reqs[2].chan_alloc.is_none());
    let unitdata = decode_sn_unitdata_pdu(&ltpd_reqs.remove(2).sdu).expect("WAP response SN-UNITDATA should decode");
    let response_octets = bitbuffer_npdu_octets(&unitdata.n_pdu).expect("response N-PDU should be byte aligned");
    let response_ip = parse_ipv4_packet(&response_octets).expect("response IPv4 should parse");
    let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");
    assert_eq!(response_ip.source, [10, 0, 0, 1]);
    assert_eq!(response_ip.destination, [10, 0, 0, 2]);
    assert_eq!(response_udp.source_port, 9200);
    assert_eq!(response_udp.destination_port, 49_152);
    let page = std::str::from_utf8(response_udp.payload).unwrap();
    assert!(page.contains("http://www.w3.org/1999/xhtml"));
    assert!(page.contains("Welcome to Nexus-BS"));
    assert!(page.contains("WAP 2.0 / WML2"));
    assert!(!page.contains("<wml"));
    assert!(!page.contains("<card"));
}

#[test]
fn sndcp_wap_packet_data_sequence_through_live_mle_has_no_lower_output() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(
        vec![TetraEntity::Mle, TetraEntity::Sndcp],
        vec![TetraEntity::Llc, TetraEntity::Cmce, TetraEntity::User, TetraEntity::Brew],
    );

    // This exercises the real MLE protocol-discriminator demux plus the real
    // SNDCP runtime entity. Even when the raw service bit is forced for a
    // decoder test, SNDCP must not emit WAP/IP, CMCE/SDS, or lower LLC output
    // until the PDP/READY/PDCH bearer is wired deliberately.
    submit_wap_packet_data_sequence_through_mle(&mut test);
    test.deliver_all_messages();

    assert_no_runtime_side_effects(&mut test);
}

#[test]
fn sndcp_wap_ip_mvp_routes_through_live_mle_to_llc_when_enabled() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    enable_wap_ip_status_mvp(&mut config);
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(
        vec![TetraEntity::Mle, TetraEntity::Sndcp],
        vec![TetraEntity::Llc, TetraEntity::Cmce, TetraEntity::Brew, TetraEntity::User],
    );

    test.submit_message(build_tla_sndcp_unitdata_ind(build_dynamic_ipv4_activation_demand(2)));
    test.submit_message(build_tla_sndcp_unitdata_ind(
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
    ));
    test.submit_message(build_tla_sndcp_unitdata_ind(build_wap_status_sn_unitdata_from(
        2,
        [10, 0, 0, 2],
        b"",
    )));
    test.deliver_all_messages();

    let sinks = test.dump_sinks();
    assert!(
        sinks
            .iter()
            .all(|msg| msg.dest != TetraEntity::Cmce && msg.dest != TetraEntity::Brew && msg.dest != TetraEntity::User)
    );

    let llc_msgs: Vec<_> = sinks
        .iter()
        .filter(|msg| msg.sap == Sap::TlaSap && msg.dest == TetraEntity::Llc)
        .collect();
    assert_eq!(llc_msgs.len(), 3, "unexpected sink messages: {sinks:#?}");
    let ready_alloc = llc_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataReqBl(req) => req.chan_alloc.as_ref(),
            _ => None,
        })
        .expect("SN-DATA TRANSMIT RESPONSE should route to LLC with a PDCH allocation");
    assert_eq!(ready_alloc.usage, Some(4));
    assert_eq!(ready_alloc.carrier, None);
    assert_eq!(ready_alloc.timeslots, [false, true, false, false]);
    assert_eq!(ready_alloc.alloc_type, ChanAllocType::Replace);
    assert_eq!(ready_alloc.ul_dl_assigned, UlDlAssignment::Both);
    let packet_data = llc_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlUnitdataReqBl(req) if req.packet_data_flag => Some(req),
            _ => None,
        })
        .expect("WAP SN-UNITDATA response should route as packet-data TLA UNITDATA");

    let mut tl_sdu = BitBuffer::from_bitbuffer(&packet_data.tl_sdu);
    tl_sdu.seek(0);
    assert_eq!(tl_sdu.read_bits(3), Some(MleProtocolDiscriminator::Sndcp.into_raw() as u64));
    let remaining_bits = tl_sdu.get_len() - 3;
    let mut sndcp_sdu = BitBuffer::new(remaining_bits);
    sndcp_sdu.copy_bits(&mut tl_sdu, remaining_bits);
    sndcp_sdu.seek(0);
    let unitdata = decode_sn_unitdata_pdu(&sndcp_sdu).expect("TLA payload should carry SN-UNITDATA");
    let response_octets = bitbuffer_npdu_octets(&unitdata.n_pdu).expect("response N-PDU should be byte aligned");
    let response_ip = parse_ipv4_packet(&response_octets).expect("response IPv4 should parse");
    assert_eq!(response_ip.source, [10, 0, 0, 1]);
    assert_eq!(response_ip.destination, [10, 0, 0, 2]);
}

#[test]
fn sndcp_ltpd_configure_primitives_drop_without_output() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    // The pure PDCH planner models LTPD-MLE-CONFIGURE, but the runtime SNDCP
    // entity has no live PDCH/session owner yet. Until that boundary exists,
    // configure primitives must be logged/dropped without side effects.
    test.submit_message(build_ltpd_configure_req());
    test.submit_message(build_ltpd_configure_ind());
    test.deliver_all_messages();

    assert_no_runtime_side_effects(&mut test);
}

#[test]
fn sndcp_ltpd_activation_demand_decodes_but_drops_without_pdp_handler() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle]);

    // This exercises only the clause 28 activation decoder. The runtime entity
    // intentionally emits no response until PDP activation/context routing is
    // implemented end-to-end and SNDCP advertising is enabled deliberately.
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_dynamic_ipv4_activation_demand(2)));
    test.deliver_all_messages();

    assert_no_runtime_side_effects(&mut test);
}

#[test]
fn sndcp_ltpd_deactivation_demand_decodes_but_drops_without_pdp_handler() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    // Runtime deactivation handling remains deliberately fail-closed: decode
    // and log only, with no MLE response or CMCE/SDS side effect.
    let pdu = encode_deactivate_pdp_context_demand(&SndcpDeactivation::Nsapi(2)).expect("deactivation demand should encode");
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, pdu));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn sndcp_ltpd_deactivation_accept_drops_without_pending_context() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle]);

    let pdu = encode_deactivate_pdp_context_accept(&SndcpDeactivation::AllNsapis).expect("deactivation accept should encode");
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, pdu));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
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
fn sndcp_decode_transfer_control_pdus_without_runtime_handoff() {
    let request = encode_data_transmit_request(&SndcpDataTransmitRequest {
        nsapi: 2,
        logical_link_status: false,
    })
    .expect("SN-DATA TRANSMIT REQUEST should encode");
    let SndcpDecode::TransferControl(SndcpTransferControl::DataTransmitRequest(decoded_request)) = decode_ltpd_sdu(&request) else {
        panic!("expected decoded SN-DATA TRANSMIT REQUEST");
    };
    assert_eq!(decoded_request.nsapi, 2);

    let end = encode_end_of_data(&SndcpEndOfData {
        immediate_service_change: true,
    })
    .expect("SN-END OF DATA should encode");
    let SndcpDecode::TransferControl(SndcpTransferControl::EndOfData(decoded_end)) = decode_ltpd_sdu(&end) else {
        panic!("expected decoded SN-END OF DATA");
    };
    assert!(decoded_end.immediate_service_change);

    let not_supported = encode_not_supported(&SndcpNotSupported {
        not_supported_pdu_type: SN_PDU_TYPE_END_OF_DATA,
    })
    .expect("SN-NOT SUPPORTED should encode");
    let SndcpDecode::TransferControl(SndcpTransferControl::NotSupported(decoded_not_supported)) = decode_ltpd_sdu(&not_supported) else {
        panic!("expected decoded SN-NOT SUPPORTED");
    };
    assert_eq!(decoded_not_supported.not_supported_pdu_type, SN_PDU_TYPE_END_OF_DATA);
}

#[test]
fn sndcp_ltpd_reserved_nsapi_and_compression_drop_without_output() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_sn_unitdata(15, 0, 0, &[0x45])));
    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_sn_unitdata(3, 1, 0, &[0x45])));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn sndcp_ltpd_transfer_control_decodes_but_drops_without_bearer_state_machine() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
        })
        .expect("SN-DATA TRANSMIT REQUEST should encode"),
    ));
    test.submit_message(build_ltpd_ind(
        Sap::TlpdSap,
        encode_end_of_data(&SndcpEndOfData {
            immediate_service_change: false,
        })
        .expect("SN-END OF DATA should encode"),
    ));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn sndcp_ltpd_malformed_control_pdu_drops_without_output() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_ind(Sap::TlpdSap, build_sn_pdu(0)));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
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

#[test]
fn sndcp_ltpd_report_indication_drops_without_pending_request() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Sndcp], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.submit_message(build_ltpd_report_ind(7, 0));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}
