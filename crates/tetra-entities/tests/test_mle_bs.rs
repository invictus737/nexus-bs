// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

mod common;

use common::ComponentTest;
use tetra_config::bluestation::StackMode;
use tetra_config::bluestation::sec_cell::{CfgBsServiceDetails, CfgNeighborCellCa};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Layer2Service, Sap, SsiType, TdmaTime, TetraAddress};
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;
use tetra_pdus::mle::pdus::d_nwrk_broadcast::DNwrkBroadcast;
use tetra_saps::lcmc::{
    LcmcMleUnitdataReq,
    enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment},
    fields::chan_alloc_req::CmceChanAllocReq,
};
use tetra_saps::lmm::LmmMleUnitdataReq;
use tetra_saps::ltpd::LtpdMleUnitdataReq;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};
use tetra_saps::tla::{
    TLA_REPORT_FAILED_TRANSFER, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION, TLA_REPORT_SUCCESSFUL_TRANSFER, TlDataConfBl, TlaTlDataIndBl,
    TlaTlReportInd, TlaTlUnitdataIndBl,
};

const TEST_ISSI: u32 = 0x0012_3456;
const TEST_GSSI: u32 = 0x0000_4321;
const TEST_BITS: &str = "10101100";

fn issi_addr() -> TetraAddress {
    TetraAddress {
        ssi: TEST_ISSI,
        ssi_type: SsiType::Issi,
    }
}

fn gssi_addr() -> TetraAddress {
    TetraAddress {
        ssi: TEST_GSSI,
        ssi_type: SsiType::Gssi,
    }
}

fn route_through_mle(message: SapMsg) -> Vec<SapMsg> {
    let mut test = ComponentTest::new(StackMode::Bs, None);
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Llc]);
    test.submit_message(message);
    test.deliver_all_messages();
    test.dump_sinks()
}

fn route_through_mle_ms(message: SapMsg) -> Vec<SapMsg> {
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Llc]);
    test.submit_message(message);
    test.deliver_all_messages();
    test.dump_sinks()
}

fn route_through_mle_with_subscriber_class(message: SapMsg, subscriber_class: u16) -> Vec<SapMsg> {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.subscriber_class = subscriber_class;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Llc]);
    test.submit_message(message);
    test.deliver_all_messages();
    test.dump_sinks()
}

fn route_sndcp_outbound_through_mle_with_direct_service_flag(message: SapMsg) -> Vec<SapMsg> {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Llc]);
    test.submit_message(message);
    test.deliver_all_messages();
    test.dump_sinks()
}

fn outbound_subscriber_class(msgs: &[SapMsg]) -> i32 {
    assert_eq!(msgs.len(), 1);
    match &msgs[0].msg {
        SapMsgInner::TlaTlDataReqBl(prim) => prim.subscriber_class,
        SapMsgInner::TlaTlDataRespBl(prim) => prim.subscriber_class,
        SapMsgInner::TlaTlUnitdataReqBl(prim) => prim.subscriber_class,
        _ => panic!("expected outbound TLA basic-link primitive"),
    }
}

fn parse_d_nwrk_broadcast(prim: &tetra_saps::tla::TlaTlUnitdataReqBl) -> DNwrkBroadcast {
    let mut tl_sdu = BitBuffer::from_bitbuffer(&prim.tl_sdu);
    assert_eq!(
        tl_sdu.read_bits(3),
        Some(MleProtocolDiscriminator::Mle.into_raw()),
        "D-NWRK-BROADCAST must carry the MLE protocol discriminator"
    );
    DNwrkBroadcast::from_bitbuf(&mut tl_sdu).expect("D-NWRK-BROADCAST should decode")
}

fn build_lmm_req(layer2service: Layer2Service) -> SapMsg {
    SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mm,
        dest: TetraEntity::Mle,
        msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
            sdu: BitBuffer::from_bitstr(TEST_BITS),
            handle: 7,
            address: issi_addr(),
            layer2service,
            stealing_permission: true,
            stealing_repeats_flag: true,
            encryption_flag: false,
            is_null_pdu: false,
            tx_reporter: None,
        }),
    }
}

fn build_lcmc_req(layer2service: Layer2Service) -> SapMsg {
    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Mle,
        msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
            sdu: BitBuffer::from_bitstr(TEST_BITS),
            handle: 11,
            endpoint_id: 2,
            link_id: 3,
            layer2service,
            pdu_prio: 5,
            layer2_qos: 0,
            stealing_permission: true,
            stealing_repeats_flag: true,
            unacked_bl_repetitions: None,
            main_address: gssi_addr(),
            chan_alloc: None,
            tx_reporter: None,
        }),
    }
}

fn build_ltpd_req(layer2service: Layer2Service) -> SapMsg {
    build_ltpd_req_with_chan_alloc(layer2service, None)
}

fn build_ltpd_req_with_chan_alloc(layer2service: Layer2Service, chan_alloc: Option<CmceChanAllocReq>) -> SapMsg {
    SapMsg {
        sap: Sap::TlpdSap,
        src: TetraEntity::Sndcp,
        dest: TetraEntity::Mle,
        msg: SapMsgInner::LtpdMleUnitdataReq(LtpdMleUnitdataReq {
            sdu: BitBuffer::from_bitstr(TEST_BITS),
            handle: 17,
            address: issi_addr(),
            layer2service,
            unacked_bl_repetitions: 2,
            pdu_prio: 6,
            endpoint_id: 4,
            link_id: 5,
            stealing_permission: true,
            stealing_repeats_flag: true,
            channel_advice_flag: false,
            data_class_info: 3,
            data_prio: 0,
            mle_data_prio_flag: false,
            packet_data_flag: true,
            scheduled_data_status: 0,
            max_schedule_interval: 0,
            fcs_flag: true,
            chan_alloc,
        }),
    }
}

fn pdch_chan_alloc() -> CmceChanAllocReq {
    CmceChanAllocReq {
        usage: None,
        carrier: None,
        timeslots: [false, true, true, true],
        alloc_type: ChanAllocType::Replace,
        ul_dl_assigned: UlDlAssignment::Both,
    }
}

fn build_tl_data_conf(req_handle: i32, addr: TetraAddress, link_id: u32, endpoint_id: u32) -> SapMsg {
    SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Mle,
        msg: SapMsgInner::TlaTlDataConfBl(TlDataConfBl {
            main_address: addr,
            link_id,
            endpoint_id,
            new_endpoint_id: None,
            css_endpoint_id: None,
            tl_sdu: None,
            scrambling_code: 0,
            fcs_flag: false,
            air_interface_encryption: 0,
            chan_change_resp_req: false,
            chan_change_handle: None,
            chan_info: None,
            req_handle,
            report: TLA_REPORT_SUCCESSFUL_TRANSFER,
        }),
    }
}

fn build_tl_report(req_handle: i32, report: i32) -> SapMsg {
    SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Mle,
        msg: SapMsgInner::TlaTlReportInd(TlaTlReportInd {
            req_handle: Some(req_handle),
            report,
            chan_change_resp_req: None,
            chan_change_handle: None,
            chan_info: None,
            endpoint_id: Some(2),
        }),
    }
}

fn build_tl_data_ind(discriminator: MleProtocolDiscriminator, addr: TetraAddress, link_id: u32, endpoint_id: u32) -> SapMsg {
    let mut sdu = BitBuffer::new(3 + TEST_BITS.len());
    sdu.write_bits(discriminator.into_raw(), 3);
    sdu.copy_bits(&mut BitBuffer::from_bitstr(TEST_BITS), TEST_BITS.len());
    sdu.seek(0);

    SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Mle,
        msg: SapMsgInner::TlaTlDataIndBl(TlaTlDataIndBl {
            main_address: addr,
            link_id,
            endpoint_id,
            new_endpoint_id: None,
            css_endpoint_id: None,
            tl_sdu: Some(sdu),
            scrambling_code: 0,
            fcs_flag: false,
            air_interface_encryption: 0,
            chan_change_resp_req: false,
            chan_change_handle: None,
            chan_info: None,
            req_handle: 23,
        }),
    }
}

fn build_tl_unitdata_ind(discriminator: MleProtocolDiscriminator, addr: TetraAddress, link_id: u32, endpoint_id: u32) -> SapMsg {
    let mut sdu = BitBuffer::new(3 + TEST_BITS.len());
    sdu.write_bits(discriminator.into_raw(), 3);
    sdu.copy_bits(&mut BitBuffer::from_bitstr(TEST_BITS), TEST_BITS.len());
    sdu.seek(0);

    SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Mle,
        msg: SapMsgInner::TlaTlUnitdataIndBl(TlaTlUnitdataIndBl {
            main_address: addr,
            link_id,
            endpoint_id,
            new_endpoint_id: None,
            css_endpoint_id: None,
            tl_sdu: Some(sdu),
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

fn assert_mle_prefixed_sdu(sdu: &BitBuffer, discriminator: MleProtocolDiscriminator) {
    assert_eq!(sdu.get_len(), 3 + TEST_BITS.len());
    assert_eq!(sdu.peek_bits(3), Some(discriminator.into_raw()));
    assert_eq!(
        sdu.peek_bits_startoffset(3, TEST_BITS.len()),
        Some(u64::from_str_radix(TEST_BITS, 2).unwrap())
    );
}

#[test]
fn test_cmce_prefixed_tl_unitdata_ind_routes_to_lcmc_sap() {
    let mut test = ComponentTest::new(StackMode::Bs, None);
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Cmce]);

    // EN 300 392-2 clauses 18.3.5.3.1 and 20.3.5.1.9 require incoming
    // unacknowledged basic-link TL-SDUs to be delivered to the service user
    // selected by the MLE protocol discriminator.
    test.submit_message(build_tl_unitdata_ind(MleProtocolDiscriminator::Cmce, gssi_addr(), 7, 8));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    assert_eq!(sink_msgs.len(), 1);
    assert_eq!(sink_msgs[0].sap, Sap::LcmcSap);
    assert_eq!(sink_msgs[0].dest, TetraEntity::Cmce);
    let SapMsgInner::LcmcMleUnitdataInd(prim) = &sink_msgs[0].msg else {
        panic!("expected CMCE MLE-UNITDATA indication");
    };
    assert_eq!(prim.received_tetra_address, gssi_addr());
    assert_eq!(prim.link_id, 7);
    assert_eq!(prim.endpoint_id, 8);
    assert_eq!(
        prim.sdu.peek_bits(TEST_BITS.len()),
        Some(u64::from_str_radix(TEST_BITS, 2).unwrap())
    );
}

#[test]
fn test_lmm_acknowledged_response_uses_tl_data_response() {
    let sink_msgs = route_through_mle(build_lmm_req(Layer2Service::AcknowledgedResponse));

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::TlaTlDataRespBl(prim) = &sink_msgs[0].msg else {
        panic!("expected TL-DATA response for acknowledged-response MM service");
    };
    assert_eq!(prim.main_address, issi_addr());
    assert_eq!(prim.link_id, 0);
    assert_eq!(prim.endpoint_id, 0);
    assert_eq!(prim.req_handle, 7);
    assert!(prim.stealing_permission);
    assert_eq!(prim.stealing_repeats_flag, Some(true));
    assert_mle_prefixed_sdu(&prim.tl_sdu, MleProtocolDiscriminator::Mm);
}

#[test]
fn test_lmm_acknowledged_request_preserves_stealing_flags() {
    let sink_msgs = route_through_mle(build_lmm_req(Layer2Service::Acknowledged));

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::TlaTlDataReqBl(prim) = &sink_msgs[0].msg else {
        panic!("expected TL-DATA request for acknowledged-request MM service");
    };
    assert_eq!(prim.main_address, issi_addr());
    assert_eq!(prim.link_id, 0);
    assert_eq!(prim.endpoint_id, 0);
    assert_ne!(prim.req_handle, 0);
    // EN 300 392-2 clause 18.3.5.3.1 requires MLE to pass layer-3
    // stealing parameters through to LLC instead of applying local defaults.
    assert!(prim.stealing_permission);
    assert_eq!(prim.stealing_repeats_flag, Some(true));
    assert_mle_prefixed_sdu(&prim.tl_sdu, MleProtocolDiscriminator::Mm);
}

#[test]
fn test_lmm_unacknowledged_request_uses_tl_unitdata_request() {
    let sink_msgs = route_through_mle(build_lmm_req(Layer2Service::Unacknowledged));

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &sink_msgs[0].msg else {
        panic!("expected TL-UNITDATA request for unacknowledged MM service");
    };
    assert_eq!(prim.main_address, issi_addr());
    assert_eq!(prim.link_id, 0);
    assert_eq!(prim.endpoint_id, 0);
    assert_eq!(prim.req_handle, 7);
    assert!(prim.stealing_permission);
    assert_eq!(prim.stealing_repeats_flag, Some(true));
    assert_mle_prefixed_sdu(&prim.tl_sdu, MleProtocolDiscriminator::Mm);
}

#[test]
fn test_lcmc_acknowledged_response_uses_tl_data_response() {
    let sink_msgs = route_through_mle(build_lcmc_req(Layer2Service::AcknowledgedResponse));

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::TlaTlDataRespBl(prim) = &sink_msgs[0].msg else {
        panic!("expected TL-DATA response for acknowledged-response CMCE service");
    };
    assert_eq!(prim.main_address, gssi_addr());
    assert_eq!(prim.link_id, 3);
    assert_eq!(prim.endpoint_id, 2);
    assert_eq!(prim.pdu_prio, 5);
    assert_eq!(prim.stealing_repeats_flag, Some(true));
    assert_eq!(prim.req_handle, 11);
    assert_mle_prefixed_sdu(&prim.tl_sdu, MleProtocolDiscriminator::Cmce);
}

#[test]
fn test_lcmc_acknowledged_request_stays_tl_data_request() {
    let sink_msgs = route_through_mle(build_lcmc_req(Layer2Service::Acknowledged));

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::TlaTlDataReqBl(prim) = &sink_msgs[0].msg else {
        panic!("expected TL-DATA request for acknowledged-request CMCE service");
    };
    assert_eq!(prim.main_address, gssi_addr());
    assert_eq!(prim.link_id, 3);
    assert_eq!(prim.endpoint_id, 2);
    assert_eq!(prim.pdu_prio, 5);
    assert!(prim.stealing_permission);
    assert_eq!(prim.stealing_repeats_flag, Some(true));
    assert_ne!(prim.req_handle, 0);
    assert_mle_prefixed_sdu(&prim.tl_sdu, MleProtocolDiscriminator::Cmce);
}

#[test]
fn test_sndcp_unacknowledged_request_uses_packet_data_tl_unitdata() {
    let sink_msgs = route_sndcp_outbound_through_mle_with_direct_service_flag(build_ltpd_req(Layer2Service::Unacknowledged));

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &sink_msgs[0].msg else {
        panic!("expected TL-UNITDATA request for unacknowledged SNDCP service");
    };
    // EN 300 392-2 clauses 17.3.5 and 18.3.5.3.1: SNDCP uses LTPD-SAP and
    // MLE prefixes the SN-PDU with the SNDCP protocol discriminator before
    // passing explicit packet-data service parameters to LLC.
    assert_eq!(prim.main_address, issi_addr());
    assert_eq!(prim.link_id, 5);
    assert_eq!(prim.endpoint_id, 4);
    assert_ne!(prim.req_handle, 0);
    assert_eq!(prim.pdu_prio, 6);
    assert!(prim.stealing_permission);
    assert_eq!(prim.stealing_repeats_flag, Some(true));
    assert!(prim.packet_data_flag);
    assert_eq!(prim.n_tlsdu_repeats, Some(2));
    assert_eq!(prim.data_class_info, Some(3));
    assert!(prim.chan_alloc.is_none());
    assert_mle_prefixed_sdu(&prim.tl_sdu, MleProtocolDiscriminator::Sndcp);
}

#[test]
fn test_sndcp_unacknowledged_request_preserves_pdch_channel_allocation() {
    let sink_msgs = route_sndcp_outbound_through_mle_with_direct_service_flag(build_ltpd_req_with_chan_alloc(
        Layer2Service::Unacknowledged,
        Some(pdch_chan_alloc()),
    ));

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &sink_msgs[0].msg else {
        panic!("expected TL-UNITDATA request for unacknowledged SNDCP service");
    };
    // EN 300 392-2 clause 28.3.5.2/2b permits the SwMI to include MAC
    // channel allocation when accepting packet-data transfer. MLE must carry
    // this SNDCP-owned allocation without changing CMCE/SDS service routing.
    let chan_alloc = prim.chan_alloc.as_ref().expect("SNDCP PDCH allocation should reach LLC");
    assert_eq!(chan_alloc.usage, None);
    assert_eq!(chan_alloc.carrier, None);
    assert_eq!(chan_alloc.timeslots, [false, true, true, true]);
    assert_eq!(chan_alloc.alloc_type, ChanAllocType::Replace);
    assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Both);
    assert_mle_prefixed_sdu(&prim.tl_sdu, MleProtocolDiscriminator::Sndcp);
}

#[test]
fn test_sndcp_unadvertised_request_does_not_reach_llc_packet_data() {
    let sink_msgs = route_through_mle(build_ltpd_req(Layer2Service::Unacknowledged));

    // EN 300 392-2 table 18.26 maps SNDCP service advertisement to packet-data
    // availability. With local service unavailable, accidental WAP/SNDCP
    // runtime wiring must not emit packet-data TL-UNITDATA to LLC.
    assert!(sink_msgs.is_empty());
}

#[test]
fn test_sndcp_terminal_report_routes_back_to_ltpd_sap() {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Sndcp]);

    test.submit_message(build_ltpd_req(Layer2Service::Unacknowledged));
    test.deliver_all_messages();
    let outbound = test.dump_sinks();
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &outbound[0].msg else {
        panic!("expected SNDCP TL-UNITDATA request");
    };
    let req_handle = prim.req_handle;

    test.submit_message(build_tl_report(req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER));
    test.deliver_all_messages();
    let reports = test.dump_sinks();

    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].sap, Sap::TlpdSap);
    assert_eq!(reports[0].dest, TetraEntity::Sndcp);
    let SapMsgInner::LtpdMleReportInd(report) = &reports[0].msg else {
        panic!("expected SNDCP MLE-REPORT indication");
    };
    assert_eq!(report.handle, 17);
    assert_eq!(report.transfer_result, TLA_REPORT_SUCCESSFUL_TRANSFER);
}

#[test]
fn test_lcmc_unacknowledged_request_stays_tl_unitdata_request() {
    let sink_msgs = route_through_mle(build_lcmc_req(Layer2Service::Unacknowledged));

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &sink_msgs[0].msg else {
        panic!("expected TL-UNITDATA request for unacknowledged CMCE service");
    };
    assert_eq!(prim.main_address, gssi_addr());
    assert_eq!(prim.link_id, 3);
    assert_eq!(prim.endpoint_id, 2);
    assert_eq!(prim.pdu_prio, 5);
    // EN 300 392-2 clauses 20.4.1.1.3 and 22.3.2.4.1 rely on request
    // handles to correlate MAC/LLC progress reports. BS MLE allocates a
    // unique lower-layer handle for CMCE TL-UNITDATA because CMCE FACCH
    // signalling often uses upper-layer handle 0.
    assert_ne!(prim.req_handle, 11);
    assert!(prim.stealing_permission);
    assert_eq!(prim.stealing_repeats_flag, Some(true));
    assert_ne!(prim.req_handle, 0);
    assert_eq!(prim.n_tlsdu_repeats, None);
    assert_mle_prefixed_sdu(&prim.tl_sdu, MleProtocolDiscriminator::Cmce);
}

#[test]
fn test_lcmc_unacknowledged_request_preserves_explicit_n253_zero() {
    let mut msg = build_lcmc_req(Layer2Service::Unacknowledged);
    let SapMsgInner::LcmcMleUnitdataReq(prim) = &mut msg.msg else {
        panic!("expected CMCE MLE-UNITDATA request");
    };
    prim.unacked_bl_repetitions = Some(0);

    let sink_msgs = route_through_mle(msg);

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &sink_msgs[0].msg else {
        panic!("expected TL-UNITDATA request for unacknowledged CMCE service");
    };
    assert_eq!(
        prim.n_tlsdu_repeats,
        Some(0),
        "EN 300 392-2 clause 22.3.2.4.1 defines N.253 + 1 BL-UDATA transmissions; explicit zero means one complete floor-control transmission"
    );
}

#[test]
fn test_lcmc_unacknowledged_requests_get_unique_lower_handles() {
    let mut first = build_lcmc_req(Layer2Service::Unacknowledged);
    let mut second = build_lcmc_req(Layer2Service::Unacknowledged);
    if let SapMsgInner::LcmcMleUnitdataReq(prim) = &mut first.msg {
        prim.handle = 0;
    }
    if let SapMsgInner::LcmcMleUnitdataReq(prim) = &mut second.msg {
        prim.handle = 0;
    }

    let mut test = ComponentTest::new(StackMode::Bs, None);
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Llc]);
    test.submit_message(first);
    test.submit_message(second);
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    assert_eq!(sink_msgs.len(), 2);
    let handles: Vec<i32> = sink_msgs
        .iter()
        .map(|msg| match &msg.msg {
            SapMsgInner::TlaTlUnitdataReqBl(prim) => prim.req_handle,
            _ => panic!("expected CMCE TL-UNITDATA request"),
        })
        .collect();

    assert_ne!(handles[0], 0);
    assert_ne!(handles[1], 0);
    assert_ne!(
        handles[0], handles[1],
        "CMCE TL-UNITDATA requests with the same upper handle must not collide at LLC/TMA"
    );
}

#[test]
fn test_lcmc_unacknowledged_terminal_report_routes_back_to_cmce() {
    let mut test = ComponentTest::new(StackMode::Bs, None);
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Cmce]);

    test.submit_message(build_lcmc_req(Layer2Service::Unacknowledged));
    test.deliver_all_messages();
    let outbound = test.dump_sinks();
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &outbound[0].msg else {
        panic!("expected CMCE TL-UNITDATA request");
    };
    let lower_req_handle = prim.req_handle;

    test.submit_message(build_tl_report(lower_req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER));
    test.deliver_all_messages();
    let reports = test.dump_sinks();

    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].sap, Sap::LcmcSap);
    assert_eq!(reports[0].dest, TetraEntity::Cmce);
    let SapMsgInner::LcmcMleReportInd(report) = &reports[0].msg else {
        panic!("expected CMCE MLE-REPORT indication");
    };
    assert_eq!(report.handle, 11);
    assert_eq!(report.transfer_result, TLA_REPORT_SUCCESSFUL_TRANSFER);
}

#[test]
fn test_bs_mle_rejects_unspecified_layer2service_todo() {
    // EN 300 392-2 clause 18.3.5.3.1 enumerates the LLC service selection
    // values. The legacy Todo sentinel is not an ETSI service and must not be
    // silently promoted to acknowledged transfer.
    assert!(
        route_through_mle(build_lmm_req(Layer2Service::Todo)).is_empty(),
        "BS MLE must reject MM Layer2Service::Todo before LLC"
    );
    assert!(
        route_through_mle(build_lcmc_req(Layer2Service::Todo)).is_empty(),
        "BS MLE must reject CMCE Layer2Service::Todo before LLC"
    );
}

#[test]
fn test_bs_mle_uses_configured_subscriber_class_for_upper_layer_requests() {
    let subscriber_class = 0x0F0F;
    let expected = subscriber_class as i32;

    // EN 300 392-2 clauses 18.3.5.1.4 and 18.5.22 define subscriber class as
    // the bit mask of classes allowed on the cell. BS MLE should pass the
    // configured cell mask down to LLC instead of hardcoding an empty mask.
    for msgs in [
        route_through_mle_with_subscriber_class(build_lmm_req(Layer2Service::Acknowledged), subscriber_class),
        route_through_mle_with_subscriber_class(build_lmm_req(Layer2Service::AcknowledgedResponse), subscriber_class),
        route_through_mle_with_subscriber_class(build_lmm_req(Layer2Service::Unacknowledged), subscriber_class),
        route_through_mle_with_subscriber_class(build_lcmc_req(Layer2Service::Acknowledged), subscriber_class),
        route_through_mle_with_subscriber_class(build_lcmc_req(Layer2Service::AcknowledgedResponse), subscriber_class),
        route_through_mle_with_subscriber_class(build_lcmc_req(Layer2Service::Unacknowledged), subscriber_class),
    ] {
        assert_eq!(outbound_subscriber_class(&msgs), expected);
    }
}

#[test]
fn test_mle_broadcast_uses_retained_nonzero_llc_handle() {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.timezone = Some("UTC".to_string());
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 20, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Mm, TetraEntity::Cmce]);

    test.run_stack(Some(1));
    let broadcast_msgs = test.dump_sinks();
    let req_handle = broadcast_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlUnitdataReqBl(prim) if prim.main_address.ssi == 0x00FF_FFFF => Some(prim.req_handle),
            _ => None,
        })
        .expect("D-NWRK-BROADCAST should be emitted as TL-UNITDATA.req");

    // EN 300 392-2 clause 22.3.1.1 requires the MLE/LLC service request
    // handle to be retained for related MAC/LLC reports; broadcast uses a
    // local MLE handle even though there is no upper-layer report recipient.
    assert_ne!(req_handle, 0);

    test.submit_message(build_tl_report(req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER));
    test.deliver_all_messages();
    assert!(
        test.dump_sinks()
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::LmmMleReportInd(_) | SapMsgInner::LcmcMleReportInd(_))),
        "broadcast transfer reports should be consumed inside MLE"
    );
}

#[test]
fn test_mle_broadcast_uses_configured_subscriber_class() {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.timezone = Some("UTC".to_string());
    config.cell.subscriber_class = 0x0F0F;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 20, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Llc]);

    test.run_stack(Some(1));
    let broadcast_msgs = test.dump_sinks();
    let subscriber_class = broadcast_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlUnitdataReqBl(prim) if prim.main_address.ssi == 0x00FF_FFFF => Some(prim.subscriber_class),
            _ => None,
        })
        .expect("D-NWRK-BROADCAST should be emitted as TL-UNITDATA.req");

    assert_eq!(subscriber_class, 0x0F0F);
}

#[test]
fn test_mle_network_time_broadcast_uses_fresh_single_llc_transmission() {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.timezone = Some("UTC".to_string());
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 20, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Llc]);

    test.run_stack(Some(1));
    let broadcast_msgs = test.dump_sinks();
    let n253 = broadcast_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlUnitdataReqBl(prim) if prim.main_address.ssi == 0x00FF_FFFF => Some(prim.n_tlsdu_repeats),
            _ => None,
        })
        .expect("D-NWRK-BROADCAST should be emitted as TL-UNITDATA.req");

    assert_eq!(
        n253,
        Some(0),
        "EN 300 392-2 clause 18.5.24 network time is sampled at PDU construction; do not repeat stale timestamp TL-SDUs via default N.253"
    );
}

#[test]
fn test_mle_broadcast_forces_neighbor_sndcp_service_unavailable() {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.neighbor_cell_broadcast = 2;
    config.cell.neighbor_cells_ca = vec![CfgNeighborCellCa {
        cell_identifier_ca: 1,
        cell_reselection_types_supported: 0,
        neighbor_cell_synchronized: false,
        cell_load_ca: 0,
        main_carrier_number: 1585,
        main_carrier_number_extension: None,
        mcc: None,
        mnc: None,
        location_area: None,
        maximum_ms_transmit_power: None,
        minimum_rx_access_level: None,
        subscriber_class: None,
        bs_service_details: Some(CfgBsServiceDetails {
            system_wide_services: true,
            voice_service: true,
            sndcp_service: true,
            ..CfgBsServiceDetails::default()
        }),
        timeshare_cell_information_or_security_parameters: None,
        tdma_frame_offset: None,
    }];

    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 20, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Llc]);
    test.run_stack(Some(1));
    let broadcast_msgs = test.dump_sinks();
    let pdu = broadcast_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlUnitdataReqBl(prim) if prim.main_address.ssi == 0x00FF_FFFF => Some(parse_d_nwrk_broadcast(prim)),
            _ => None,
        })
        .expect("D-NWRK-BROADCAST should be emitted as TL-UNITDATA.req");

    // EN 300 392-2 clause 18.5.17 permits neighbour BS service details, and
    // table 18.26 defines SNDCP service=1 as packet-data availability. The
    // Nexus-BS WAP MVP is SDS-based, so MLE must fail-closed on the on-air bit.
    assert_eq!(pdu.number_of_ca_neighbour_cells, Some(1));
    let details = pdu.neighbour_cell_information_for_ca[0]
        .bs_service_details
        .as_ref()
        .expect("neighbour BS service details should be present");
    assert!(details.system_wide_services);
    assert!(details.voice_service);
    assert!(!details.sndcp_service);
}

#[test]
fn test_ms_mle_lmm_acknowledged_request_preserves_upper_handle() {
    let sink_msgs = route_through_mle_ms(build_lmm_req(Layer2Service::Acknowledged));

    // EN 300 392-2 clause 22.3.1.1 requires request handles to identify
    // subsequent related primitives across the MLE/LLC boundary.
    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::TlaTlDataReqBl(prim) = &sink_msgs[0].msg else {
        panic!("MS MLE should emit TL-DATA request for acknowledged MM service");
    };
    assert_eq!(prim.req_handle, 7);
    assert_ne!(prim.req_handle, 0);
    assert!(prim.stealing_permission);
    assert_eq!(prim.stealing_repeats_flag, Some(true));
    assert_mle_prefixed_sdu(&prim.tl_sdu, MleProtocolDiscriminator::Mm);
}

#[test]
fn test_ms_mle_lmm_unacknowledged_request_preserves_upper_handle() {
    let sink_msgs = route_through_mle_ms(build_lmm_req(Layer2Service::Unacknowledged));

    // EN 300 392-2 clause 18.3.5.3.1 maps an unacknowledged MM
    // MLE-UNITDATA request onto a TL-UNITDATA request after MLE prefixes the
    // MM protocol discriminator.
    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &sink_msgs[0].msg else {
        panic!("MS MLE should emit TL-UNITDATA request for unacknowledged MM service");
    };
    assert_eq!(prim.main_address, issi_addr());
    assert_eq!(prim.link_id, 0);
    assert_eq!(prim.endpoint_id, 0);
    assert_eq!(prim.req_handle, 7);
    assert!(prim.stealing_permission);
    assert_eq!(prim.stealing_repeats_flag, Some(true));
    assert_mle_prefixed_sdu(&prim.tl_sdu, MleProtocolDiscriminator::Mm);
}

#[test]
fn test_ms_mle_lcmc_service_selection_preserves_handle() {
    let ack_resp = route_through_mle_ms(build_lcmc_req(Layer2Service::AcknowledgedResponse));
    let SapMsgInner::TlaTlDataRespBl(resp) = &ack_resp[0].msg else {
        panic!("MS MLE should emit TL-DATA response for acknowledged-response CMCE service");
    };
    assert_eq!(resp.req_handle, 11);
    assert_eq!(resp.endpoint_id, 2);
    assert_eq!(resp.link_id, 3);
    assert_eq!(resp.stealing_repeats_flag, Some(true));
    assert_mle_prefixed_sdu(&resp.tl_sdu, MleProtocolDiscriminator::Cmce);

    let unacked = route_through_mle_ms(build_lcmc_req(Layer2Service::Unacknowledged));
    let SapMsgInner::TlaTlUnitdataReqBl(unitdata) = &unacked[0].msg else {
        panic!("MS MLE should emit TL-UNITDATA request for unacknowledged CMCE service");
    };
    assert_eq!(unitdata.req_handle, 11);
    assert_eq!(unitdata.endpoint_id, 2);
    assert_eq!(unitdata.link_id, 3);
    assert_eq!(unitdata.pdu_prio, 5);
    assert!(unitdata.stealing_permission);
    assert_eq!(unitdata.stealing_repeats_flag, Some(true));
    assert_mle_prefixed_sdu(&unitdata.tl_sdu, MleProtocolDiscriminator::Cmce);
}

#[test]
fn test_ms_mle_rejects_unspecified_layer2service_todo() {
    // EN 300 392-2 clause 18.3.5.3.1 requires an explicit layer 2 service
    // selection. The MS-side MLE must not guess the service for Todo either.
    assert!(
        route_through_mle_ms(build_lmm_req(Layer2Service::Todo)).is_empty(),
        "MS MLE must reject MM Layer2Service::Todo before LLC"
    );
    assert!(
        route_through_mle_ms(build_lcmc_req(Layer2Service::Todo)).is_empty(),
        "MS MLE must reject CMCE Layer2Service::Todo before LLC"
    );
}

#[test]
fn test_sndcp_prefixed_tl_data_ind_routes_to_tlpd_sap() {
    let mut test = ComponentTest::new(StackMode::Bs, None);
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Sndcp]);

    test.submit_message(build_tl_data_ind(MleProtocolDiscriminator::Sndcp, issi_addr(), 9, 4));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    assert_eq!(sink_msgs.len(), 1);
    assert_eq!(sink_msgs[0].sap, Sap::TlpdSap);
    assert_eq!(sink_msgs[0].dest, TetraEntity::Sndcp);
    let SapMsgInner::LtpdMleUnitdataInd(prim) = &sink_msgs[0].msg else {
        panic!("expected SNDCP MLE-UNITDATA indication");
    };
    assert_eq!(prim.received_tetra_address, issi_addr());
    assert_eq!(prim.link_id, 9);
    assert_eq!(prim.endpoint_id, 4);
    assert_eq!(
        prim.sdu.peek_bits(TEST_BITS.len()),
        Some(u64::from_str_radix(TEST_BITS, 2).unwrap())
    );
}

#[test]
fn test_ms_sndcp_prefixed_tl_data_ind_routes_to_tlpd_sap() {
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Sndcp]);

    test.submit_message(build_tl_data_ind(MleProtocolDiscriminator::Sndcp, issi_addr(), 9, 4));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 17.2 and 18.5.21 map SNDCP to LTPD-SAP; clause
    // 18.3.5.3.1 d) requires incoming basic-link data to be routed to the SAP
    // indicated by the MLE protocol discriminator.
    assert_eq!(sink_msgs.len(), 1);
    assert_eq!(sink_msgs[0].sap, Sap::TlpdSap);
    assert_eq!(sink_msgs[0].dest, TetraEntity::Sndcp);
    let SapMsgInner::LtpdMleUnitdataInd(prim) = &sink_msgs[0].msg else {
        panic!("expected MS SNDCP MLE-UNITDATA indication");
    };
    assert_eq!(prim.received_tetra_address, issi_addr());
    assert_eq!(prim.link_id, 9);
    assert_eq!(prim.endpoint_id, 4);
    assert_eq!(
        prim.sdu.peek_bits(TEST_BITS.len()),
        Some(u64::from_str_radix(TEST_BITS, 2).unwrap())
    );
}

#[test]
fn test_ms_sndcp_prefixed_tl_unitdata_ind_routes_to_tlpd_sap() {
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Sndcp]);

    test.submit_message(build_tl_unitdata_ind(MleProtocolDiscriminator::Sndcp, issi_addr(), 5, 6));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    // Same routing rule as TL-DATA.ind, but through the unacknowledged LLC
    // service primitive.
    assert_eq!(sink_msgs.len(), 1);
    assert_eq!(sink_msgs[0].sap, Sap::TlpdSap);
    assert_eq!(sink_msgs[0].dest, TetraEntity::Sndcp);
    let SapMsgInner::LtpdMleUnitdataInd(prim) = &sink_msgs[0].msg else {
        panic!("expected MS SNDCP MLE-UNITDATA indication");
    };
    assert_eq!(prim.received_tetra_address, issi_addr());
    assert_eq!(prim.link_id, 5);
    assert_eq!(prim.endpoint_id, 6);
    assert_eq!(
        prim.sdu.peek_bits(TEST_BITS.len()),
        Some(u64::from_str_radix(TEST_BITS, 2).unwrap())
    );
}

#[test]
fn test_lmm_tl_data_confirm_routes_mle_report_to_original_handle() {
    let mut test = ComponentTest::new(StackMode::Bs, None);
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Mm]);

    test.submit_message(build_lmm_req(Layer2Service::Acknowledged));
    test.deliver_all_messages();
    let outbound = test.dump_sinks();
    let lower_req_handle = outbound
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataReqBl(prim) => Some(prim.req_handle),
            _ => None,
        })
        .expect("MLE should emit a TL-DATA request");
    assert_ne!(lower_req_handle, 0);

    test.submit_message(build_tl_data_conf(lower_req_handle, issi_addr(), 0, 0));
    test.deliver_all_messages();
    let reports = test.dump_sinks();

    let report = reports
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleReportInd(prim) => Some(prim),
            _ => None,
        })
        .expect("successful TL-DATA confirm should produce MM MLE-REPORT.ind");
    assert_eq!(report.handle, 7);
    assert_eq!(report.transfer_result, TLA_REPORT_SUCCESSFUL_TRANSFER);
}

#[test]
fn test_lmm_success_tl_report_routes_once_and_clears_handle() {
    let mut test = ComponentTest::new(StackMode::Bs, None);
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Mm]);

    test.submit_message(build_lmm_req(Layer2Service::Acknowledged));
    test.deliver_all_messages();
    let outbound = test.dump_sinks();
    let lower_req_handle = outbound
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataReqBl(prim) => Some(prim.req_handle),
            _ => None,
        })
        .expect("MLE should emit a TL-DATA request");

    test.submit_message(build_tl_report(lower_req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER));
    test.deliver_all_messages();
    let reports = test.dump_sinks();
    let report = reports
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleReportInd(prim) => Some(prim),
            _ => None,
        })
        .expect("successful terminal TL-REPORT should produce MM MLE-REPORT.ind");
    assert_eq!(report.handle, 7);
    assert_eq!(report.transfer_result, TLA_REPORT_SUCCESSFUL_TRANSFER);

    test.submit_message(build_tl_data_conf(lower_req_handle, issi_addr(), 0, 0));
    test.deliver_all_messages();
    assert!(
        test.dump_sinks()
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::LmmMleReportInd(_))),
        "terminal TL-REPORT must clear the pending handle"
    );
}

#[test]
fn test_lcmc_failed_tl_report_routes_once_and_clears_handle() {
    let mut test = ComponentTest::new(StackMode::Bs, None);
    test.populate_entities(vec![TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Cmce]);

    test.submit_message(build_lcmc_req(Layer2Service::Acknowledged));
    test.deliver_all_messages();
    let outbound = test.dump_sinks();
    let lower_req_handle = outbound
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataReqBl(prim) => Some(prim.req_handle),
            _ => None,
        })
        .expect("MLE should emit a TL-DATA request");
    assert_ne!(lower_req_handle, 0);

    test.submit_message(build_tl_report(lower_req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION));
    test.deliver_all_messages();
    assert!(
        test.dump_sinks()
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleReportInd(_))),
        "first-complete TL-REPORT is progress, not completion"
    );

    test.submit_message(build_tl_report(lower_req_handle, TLA_REPORT_FAILED_TRANSFER));
    test.deliver_all_messages();
    let reports = test.dump_sinks();
    let report = reports
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleReportInd(prim) => Some(prim),
            _ => None,
        })
        .expect("failed TL-REPORT should produce CMCE MLE-REPORT.ind");
    assert_eq!(report.handle, 11);
    assert_eq!(report.transfer_result, TLA_REPORT_FAILED_TRANSFER);

    test.submit_message(build_tl_data_conf(lower_req_handle, gssi_addr(), 3, 2));
    test.deliver_all_messages();
    assert!(
        test.dump_sinks()
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleReportInd(_))),
        "failed terminal TL-REPORT must clear the pending handle"
    );
}
