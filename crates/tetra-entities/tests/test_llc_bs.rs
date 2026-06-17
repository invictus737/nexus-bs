// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

mod common;

use common::ComponentTest;
use tetra_config::bluestation::StackMode;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Sap, SsiType, TdmaTime, TetraAddress, TxReporter, TxState, debug};
use tetra_entities::llc::components::fcs;
use tetra_pdus::llc::consts::consts::N252_BL_MAX_TLSDU_RETRANSMITS_ACKED;
use tetra_pdus::llc::consts::timers::{T251_SENDER_RETRY_TIMER, T252_ACK_WAITING_TIMER};
use tetra_pdus::llc::enums::llc_pdu_type::LlcPduType;
use tetra_pdus::llc::pdus::al_ack::AlAck;
use tetra_pdus::llc::pdus::al_data::AlData;
use tetra_pdus::llc::pdus::al_setup::AlSetup;
use tetra_pdus::llc::pdus::bl_ack::BlAck;
use tetra_pdus::llc::pdus::bl_adata::BlAdata;
use tetra_pdus::llc::pdus::bl_data::BlData;
use tetra_pdus::llc::pdus::bl_udata::BlUdata;
use tetra_saps::lcmc::enums::alloc_type::ChanAllocType;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};
use tetra_saps::tla::{
    TLA_REPORT_FAILED_TRANSFER, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION, TLA_REPORT_NO_SPECIFIC_REPORT, TLA_REPORT_SUCCESSFUL_TRANSFER,
    TlDataRespBl, TlaTlDataReqBl, TlaTlUnitdataReqBl,
};
use tetra_saps::tma::{TmaReport, TmaReportInd, TmaUnitdataInd};

const LLC_INBOUND_DUPLICATE_SUPPRESSION_HORIZON_TICKS: usize =
    (N252_BL_MAX_TLSDU_RETRANSMITS_ACKED as usize + 1) * T251_SENDER_RETRY_TIMER as usize;

#[test]
fn test_bl_data_with_unanswered_tl_sdu_sends_standalone_ack() {
    debug::setup_logging_verbose();

    // BL-DATA without FCS, N(S)=1, followed by an intentionally incomplete
    // upper-layer SDU. The point of this vector is the LLC ACK behaviour when
    // no TL-DATA response is available to piggyback into BL-ACK.
    let test_vec = "00011001011100111000000011111100001000010000000000000000";
    let dltime_vec = TdmaTime::default().add_timeslots(2); // Downlink time: 0/1/1/3
    let test_prim = TmaUnitdataInd {
        pdu: Some(BitBuffer::from_bitstr(test_vec)),
        main_address: TetraAddress {
            ssi: 2065022,
            ssi_type: SsiType::Issi,
        },
        scrambling_code: 864282631,
        endpoint_id: 0,
        new_endpoint_id: None,
        css_endpoint_id: None,
        air_interface_encryption: 0,
        chan_change_response_req: false,
        chan_change_handle: None,
        chan_info: None,
    };
    let test_sapmsg = SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(test_prim),
    };

    // Setup testing stack
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime_vec));
    let components = vec![TetraEntity::Llc, TetraEntity::Mle, TetraEntity::Mm];
    let sinks: Vec<TetraEntity> = vec![TetraEntity::Umac];
    test.populate_entities(components, sinks);

    // Submit and process message
    test.submit_message(test_sapmsg);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert_eq!(sink_msgs.len(), 1);
    let ack_msg = sink_msgs
        .iter()
        .find(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlAck))
        .expect("expected standalone BL-ACK");
    let SapMsgInner::TmaUnitdataReq(ack) = &ack_msg.msg else {
        panic!("expected TMA-UNITDATA.req");
    };

    // EN 300 392-2 clauses 22.3.2.3(d) and 22.3.1.2: without a matching
    // TL-DATA response/request, MAC-ready LLC sends a standalone BL-ACK and
    // copies the received basic-link address/context to TMA.
    assert_eq!(ack_msg.sap, Sap::TmaSap);
    assert_eq!(ack_msg.src, TetraEntity::Llc);
    assert_eq!(ack_msg.dest, TetraEntity::Umac);
    assert_eq!(bl_ack_nr_and_payload_bits(ack_msg), Some((1, String::new())));
    assert_eq!(ack.req_handle, -1);
    assert_eq!(ack.main_address, TetraAddress::new(2065022, SsiType::Issi));
    assert_eq!(ack.endpoint_id, 0);
    assert_eq!(ack.pdu_prio, 5);
    assert!(!ack.stealing_permission);
    assert_eq!(ack.subscriber_class, 0);
    assert_eq!(ack.air_interface_encryption, Some(0));
    assert!(ack.stealing_repeats_flag.is_none());
    assert!(ack.data_category.is_none());
    assert!(ack.chan_alloc.is_none());
    assert!(ack.tx_reporter.is_none());
}

fn build_tl_data_req(addr: TetraAddress) -> SapMsg {
    build_tl_data_req_with_handle(addr, 0)
}

fn build_tl_data_req_with_timeslot(addr: TetraAddress, timeslot: u8) -> SapMsg {
    let mut msg = build_tl_data_req(addr);
    set_tl_data_req_timeslot(&mut msg, timeslot);
    msg
}

fn build_tl_data_req_with_handle_timeslot(addr: TetraAddress, req_handle: i32, timeslot: u8) -> SapMsg {
    let mut msg = build_tl_data_req_with_handle(addr, req_handle);
    set_tl_data_req_timeslot(&mut msg, timeslot);
    msg
}

fn build_tl_data_req_with_payload_handle_timeslot(addr: TetraAddress, payload: &[u8], req_handle: i32, timeslot: u8) -> SapMsg {
    let mut msg = build_tl_data_req_with_handle_timeslot(addr, req_handle, timeslot);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut msg.msg else {
        unreachable!("build_tl_data_req_with_handle_timeslot must return TlaTlDataReqBl");
    };
    prim.tl_sdu = BitBuffer::from_bytes(payload);
    msg
}

fn build_tl_data_req_with_handle_fcs_timeslot(addr: TetraAddress, req_handle: i32, fcs_flag: bool, timeslot: u8) -> SapMsg {
    let mut msg = build_tl_data_req_with_handle_and_fcs(addr, req_handle, fcs_flag);
    set_tl_data_req_timeslot(&mut msg, timeslot);
    msg
}

fn set_tl_data_req_timeslot(msg: &mut SapMsg, timeslot: u8) {
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut msg.msg else {
        unreachable!("build_tl_data_req must return TlaTlDataReqBl");
    };
    let mut timeslots = [false; 4];
    timeslots[(timeslot - 1) as usize] = true;
    prim.chan_alloc = Some(CmceChanAllocReq {
        usage: None,
        carrier: None,
        timeslots,
        alloc_type: ChanAllocType::Replace,
        ul_dl_assigned: UlDlAssignment::Both,
    });
}

fn build_tl_data_req_with_handle(addr: TetraAddress, req_handle: i32) -> SapMsg {
    build_tl_data_req_with_handle_and_fcs(addr, req_handle, false)
}

fn build_tl_data_req_with_handle_and_fcs(addr: TetraAddress, req_handle: i32, fcs_flag: bool) -> SapMsg {
    build_tl_data_req_with_endpoint_handle_and_fcs(addr, 0, req_handle, fcs_flag)
}

fn build_tl_data_req_with_endpoint_handle(addr: TetraAddress, endpoint_id: u32, req_handle: i32) -> SapMsg {
    build_tl_data_req_with_endpoint_handle_and_fcs(addr, endpoint_id, req_handle, false)
}

fn build_tl_data_req_with_endpoint_handle_and_fcs(addr: TetraAddress, endpoint_id: u32, req_handle: i32, fcs_flag: bool) -> SapMsg {
    SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
            main_address: addr,
            link_id: 0,
            endpoint_id,
            tl_sdu: BitBuffer::from_bytes(&[0x55]),
            pdu_prio: 0,
            stealing_permission: false,
            subscriber_class: 0,
            fcs_flag,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_class_info: None,
            req_handle,
            graceful_degradation: None,
            chan_alloc: None,
            tx_reporter: None,
        }),
    }
}

fn build_tl_data_resp(addr: TetraAddress, payload: &[u8]) -> SapMsg {
    build_tl_data_resp_with_endpoint_and_fcs(addr, 0, payload, false)
}

fn build_tl_data_resp_with_handle(addr: TetraAddress, req_handle: i32, payload: &[u8]) -> SapMsg {
    build_tl_data_resp_with_endpoint_handle_and_fcs(addr, 0, req_handle, payload, false)
}

fn build_tl_data_resp_with_endpoint_and_fcs(addr: TetraAddress, endpoint_id: u32, payload: &[u8], fcs_flag: bool) -> SapMsg {
    build_tl_data_resp_with_endpoint_handle_and_fcs(addr, endpoint_id, 0, payload, fcs_flag)
}

fn build_tl_data_resp_with_endpoint_handle_and_fcs(
    addr: TetraAddress,
    endpoint_id: u32,
    req_handle: i32,
    payload: &[u8],
    fcs_flag: bool,
) -> SapMsg {
    SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TlaTlDataRespBl(TlDataRespBl {
            main_address: addr,
            link_id: 0,
            endpoint_id,
            tl_sdu: BitBuffer::from_bytes(payload),
            scrambling_code: 0,
            pdu_prio: 0,
            stealing_permission: false,
            subscriber_class: 0,
            fcs_flag,
            air_interface_encryption: 0,
            stealing_repeats_flag: None,
            data_class_info: None,
            req_handle,
        }),
    }
}

fn build_tl_unitdata_req_with_fcs(addr: TetraAddress, payload: &[u8], fcs_flag: bool) -> SapMsg {
    build_tl_unitdata_req_with_repeats_handle_and_fcs(addr, payload, 0, Some(0), fcs_flag)
}

fn build_tl_unitdata_req_with_repeats_handle(addr: TetraAddress, payload: &[u8], req_handle: i32, n_tlsdu_repeats: u8) -> SapMsg {
    build_tl_unitdata_req_with_repeats_handle_and_fcs(addr, payload, req_handle, Some(n_tlsdu_repeats), false)
}

fn build_tl_unitdata_req_without_repeats_handle(addr: TetraAddress, payload: &[u8], req_handle: i32) -> SapMsg {
    build_tl_unitdata_req_with_repeats_handle_and_fcs(addr, payload, req_handle, None, false)
}

fn build_tl_unitdata_req_with_repeats_handle_and_fcs(
    addr: TetraAddress,
    payload: &[u8],
    req_handle: i32,
    n_tlsdu_repeats: Option<u8>,
    fcs_flag: bool,
) -> SapMsg {
    SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TlaTlUnitdataReqBl(TlaTlUnitdataReqBl {
            main_address: addr,
            link_id: 0,
            endpoint_id: 0,
            tl_sdu: BitBuffer::from_bytes(payload),
            pdu_prio: 0,
            stealing_permission: false,
            subscriber_class: 0,
            fcs_flag,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            packet_data_flag: false,
            n_tlsdu_repeats,
            data_class_info: None,
            req_handle,
            chan_alloc: None,
            tx_reporter: None,
        }),
    }
}

fn build_tma_report_ind(req_handle: i32, report: TmaReport) -> SapMsg {
    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaReportInd(TmaReportInd { req_handle, report }),
    }
}

fn build_bl_ack_ind(addr: TetraAddress, nr: u8) -> SapMsg {
    build_bl_ack_ind_with_payload(addr, nr, &[])
}

fn build_bl_ack_ind_with_payload(addr: TetraAddress, nr: u8, payload: &[u8]) -> SapMsg {
    build_bl_ack_ind_with_payload_and_fcs(addr, nr, payload, false)
}

fn build_bl_ack_ind_with_payload_bits(addr: TetraAddress, nr: u8, payload_bits: &str) -> SapMsg {
    build_bl_ack_ind_with_payload_bits_and_fcs(addr, nr, payload_bits, false)
}

fn build_bl_ack_ind_with_payload_and_fcs(addr: TetraAddress, nr: u8, payload: &[u8], has_fcs: bool) -> SapMsg {
    build_bl_ack_ind_with_endpoint_payload_and_fcs(addr, 0, nr, payload, has_fcs)
}

fn build_bl_ack_ind_with_payload_bits_and_fcs(addr: TetraAddress, nr: u8, payload_bits: &str, has_fcs: bool) -> SapMsg {
    let bl_ack = BlAck { has_fcs, nr };
    let mut pdu = BitBuffer::new_autoexpand(8);
    bl_ack.to_bitbuf(&mut pdu);
    append_payload_bits_and_optional_fcs_for_test(&mut pdu, payload_bits, has_fcs);

    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
            pdu: Some(pdu),
            main_address: addr,
            scrambling_code: 0,
            endpoint_id: 0,
            new_endpoint_id: None,
            css_endpoint_id: None,
            air_interface_encryption: 0,
            chan_change_response_req: false,
            chan_change_handle: None,
            chan_info: None,
        }),
    }
}

fn build_bl_ack_ind_with_endpoint(addr: TetraAddress, endpoint_id: u32, nr: u8) -> SapMsg {
    build_bl_ack_ind_with_endpoint_payload_and_fcs(addr, endpoint_id, nr, &[], false)
}

fn build_bl_ack_ind_with_endpoint_payload_and_fcs(addr: TetraAddress, endpoint_id: u32, nr: u8, payload: &[u8], has_fcs: bool) -> SapMsg {
    let bl_ack = BlAck { has_fcs, nr };
    let mut pdu = BitBuffer::new_autoexpand(8);
    bl_ack.to_bitbuf(&mut pdu);
    append_payload_and_optional_fcs_for_test(&mut pdu, payload, has_fcs);

    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
            pdu: Some(pdu),
            main_address: addr,
            scrambling_code: 0,
            endpoint_id,
            new_endpoint_id: None,
            css_endpoint_id: None,
            air_interface_encryption: 0,
            chan_change_response_req: false,
            chan_change_handle: None,
            chan_info: None,
        }),
    }
}

fn build_bl_data_ind(addr: TetraAddress, ns: u8) -> SapMsg {
    build_bl_data_ind_with_endpoint_payload_and_fcs(addr, 0, ns, &[], false)
}

fn build_bl_data_ind_with_payload_and_fcs(addr: TetraAddress, ns: u8, payload: &[u8], has_fcs: bool) -> SapMsg {
    build_bl_data_ind_with_endpoint_payload_and_fcs(addr, 0, ns, payload, has_fcs)
}

fn build_bl_data_ind_with_endpoint(addr: TetraAddress, endpoint_id: u32, ns: u8) -> SapMsg {
    build_bl_data_ind_with_endpoint_payload_and_fcs(addr, endpoint_id, ns, &[], false)
}

fn build_bl_data_ind_with_endpoint_payload_and_fcs(addr: TetraAddress, endpoint_id: u32, ns: u8, payload: &[u8], has_fcs: bool) -> SapMsg {
    let bl_data = BlData { has_fcs, ns };
    let mut pdu = BitBuffer::new_autoexpand(8);
    bl_data.to_bitbuf(&mut pdu);
    append_payload_and_optional_fcs_for_test(&mut pdu, payload, has_fcs);

    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
            pdu: Some(pdu),
            main_address: addr,
            scrambling_code: 0,
            endpoint_id,
            new_endpoint_id: None,
            css_endpoint_id: None,
            air_interface_encryption: 0,
            chan_change_response_req: false,
            chan_change_handle: None,
            chan_info: None,
        }),
    }
}

fn build_bl_adata_ind_with_payload_and_fcs(addr: TetraAddress, nr: u8, ns: u8, payload: &[u8], has_fcs: bool) -> SapMsg {
    let bl_adata = BlAdata { has_fcs, nr, ns };
    let mut pdu = BitBuffer::new_autoexpand(8);
    bl_adata.to_bitbuf(&mut pdu);
    append_payload_and_optional_fcs_for_test(&mut pdu, payload, has_fcs);

    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
            pdu: Some(pdu),
            main_address: addr,
            scrambling_code: 0,
            endpoint_id: 0,
            new_endpoint_id: None,
            css_endpoint_id: None,
            air_interface_encryption: 0,
            chan_change_response_req: false,
            chan_change_handle: None,
            chan_info: None,
        }),
    }
}

fn build_al_setup_ind(addr: TetraAddress, endpoint_id: u32, al_number: u8) -> SapMsg {
    let mut setup = default_al_setup();
    setup.advanced_link_number = al_number;
    build_al_setup_ind_with_setup(addr, endpoint_id, setup)
}

fn default_al_setup() -> AlSetup {
    AlSetup {
        acknowledged_service: true,
        advanced_link_number: 0,
        max_tl_sdu_len_code: 6,
        connection_width: false,
        advanced_link_symmetry: false,
        uplink_timeslots: None,
        downlink_timeslots: None,
        throughput_code: 6,
        window_size_code: 1,
        max_tl_sdu_retransmissions: 3,
        max_segment_retransmissions: 3,
        setup_report: AlSetup::SETUP_REPORT_SERVICE_DEFINITION,
        ns: None,
        augmented: None,
    }
}

fn build_al_setup_ind_with_setup(addr: TetraAddress, endpoint_id: u32, setup: AlSetup) -> SapMsg {
    let mut pdu = BitBuffer::new_autoexpand(32);
    setup.to_bitbuf(&mut pdu);
    pdu.seek(0);

    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
            pdu: Some(pdu),
            main_address: addr,
            scrambling_code: 0,
            endpoint_id,
            new_endpoint_id: None,
            css_endpoint_id: None,
            air_interface_encryption: 0,
            chan_change_response_req: false,
            chan_change_handle: None,
            chan_info: None,
        }),
    }
}

fn build_al_final_ar_ind(addr: TetraAddress, endpoint_id: u32, ns: u8, payload: &[u8]) -> SapMsg {
    let data = AlData {
        final_segment: true,
        acknowledgement_requested: true,
        ns,
        ss: 0,
    };
    let mut pdu = BitBuffer::new_autoexpand(64);
    data.to_bitbuf(&mut pdu);
    append_al_payload_and_fcs_for_test(&mut pdu, payload);

    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
            pdu: Some(pdu),
            main_address: addr,
            scrambling_code: 0,
            endpoint_id,
            new_endpoint_id: None,
            css_endpoint_id: None,
            air_interface_encryption: 0,
            chan_change_response_req: false,
            chan_change_handle: None,
            chan_info: None,
        }),
    }
}

fn build_al_data_ar_ind(addr: TetraAddress, endpoint_id: u32, ns: u8, ss: u8, payload: &[u8]) -> SapMsg {
    let data = AlData {
        final_segment: false,
        acknowledgement_requested: true,
        ns,
        ss,
    };
    let mut pdu = BitBuffer::new_autoexpand(64);
    data.to_bitbuf(&mut pdu);
    append_payload_and_optional_fcs_for_test(&mut pdu, payload, false);
    pdu.seek(0);

    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
            pdu: Some(pdu),
            main_address: addr,
            scrambling_code: 0,
            endpoint_id,
            new_endpoint_id: None,
            css_endpoint_id: None,
            air_interface_encryption: 0,
            chan_change_response_req: false,
            chan_change_handle: None,
            chan_info: None,
        }),
    }
}

fn build_al_ack_ind(addr: TetraAddress, endpoint_id: u32, nr: u8) -> SapMsg {
    let mut pdu = BitBuffer::new_autoexpand(16);
    AlAck::complete(nr).to_bitbuf(&mut pdu);
    pdu.seek(0);

    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
            pdu: Some(pdu),
            main_address: addr,
            scrambling_code: 0,
            endpoint_id,
            new_endpoint_id: None,
            css_endpoint_id: None,
            air_interface_encryption: 0,
            chan_change_response_req: false,
            chan_change_handle: None,
            chan_info: None,
        }),
    }
}

fn build_al_rnr_complete_ind(addr: TetraAddress, endpoint_id: u32, nr: u8) -> SapMsg {
    let mut pdu = BitBuffer::new_autoexpand(16);
    let mut ack = AlAck::complete(nr);
    ack.receiver_ready = false;
    ack.to_bitbuf(&mut pdu);
    pdu.seek(0);

    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
            pdu: Some(pdu),
            main_address: addr,
            scrambling_code: 0,
            endpoint_id,
            new_endpoint_id: None,
            css_endpoint_id: None,
            air_interface_encryption: 0,
            chan_change_response_req: false,
            chan_change_handle: None,
            chan_info: None,
        }),
    }
}

fn build_al_selective_ack_ind(addr: TetraAddress, endpoint_id: u32, nr: u8, sr: u8) -> SapMsg {
    build_al_selective_ack_ind_with_bitmap(addr, endpoint_id, nr, sr, 0, 1)
}

fn build_al_selective_ack_ind_with_bitmap(
    addr: TetraAddress,
    endpoint_id: u32,
    nr: u8,
    sr: u8,
    acknowledgement_bitmap: u64,
    acknowledgement_length: u8,
) -> SapMsg {
    let mut pdu = BitBuffer::new_autoexpand(32);
    AlAck::selective(true, nr, sr, acknowledgement_bitmap, acknowledgement_length).to_bitbuf(&mut pdu);
    pdu.seek(0);

    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
            pdu: Some(pdu),
            main_address: addr,
            scrambling_code: 0,
            endpoint_id,
            new_endpoint_id: None,
            css_endpoint_id: None,
            air_interface_encryption: 0,
            chan_change_response_req: false,
            chan_change_handle: None,
            chan_info: None,
        }),
    }
}

fn append_al_payload_and_fcs_for_test(pdu: &mut BitBuffer, payload: &[u8]) {
    let payload_start = pdu.get_len_written();
    let mut payload_buf = BitBuffer::from_bytes(payload);
    let payload_len = payload_buf.get_len_remaining();
    pdu.copy_bits(&mut payload_buf, payload_len);
    let fcs_value = fcs::compute_fcs(pdu, payload_start, pdu.get_len());
    pdu.write_bits(fcs_value as u64, 32);
    pdu.seek(0);
}

fn append_payload_and_optional_fcs_for_test(pdu: &mut BitBuffer, payload: &[u8], has_fcs: bool) {
    let payload_start = pdu.get_len_written();
    let mut payload_buf = BitBuffer::from_bytes(payload);
    let payload_len = payload_buf.get_len_remaining();
    pdu.copy_bits(&mut payload_buf, payload_len);
    if has_fcs {
        let fcs_value = fcs::compute_fcs(pdu, payload_start, pdu.get_len());
        pdu.write_bits(fcs_value as u64, 32);
    }
    pdu.seek(0);
}

fn append_payload_bits_and_optional_fcs_for_test(pdu: &mut BitBuffer, payload_bits: &str, has_fcs: bool) {
    let payload_start = pdu.get_len_written();
    let mut payload_buf = BitBuffer::from_bitstr(payload_bits);
    let payload_len = payload_buf.get_len_remaining();
    pdu.copy_bits(&mut payload_buf, payload_len);
    if has_fcs {
        let fcs_value = fcs::compute_fcs(pdu, payload_start, pdu.get_len());
        pdu.write_bits(fcs_value as u64, 32);
    }
    pdu.seek(0);
}

fn corrupt_last_bit(mut msg: SapMsg) -> SapMsg {
    let SapMsgInner::TmaUnitdataInd(prim) = &mut msg.msg else {
        panic!("expected TMA-UNITDATA.ind");
    };
    let pdu = prim.pdu.as_mut().expect("expected LLC PDU");
    let end = pdu.get_raw_end();
    pdu.set_raw_pos(end - 1);
    pdu.xor_bit(1);
    pdu.seek(0);
    msg
}

fn take_first_tma_req_reporter(msgs: &mut [SapMsg]) -> tetra_core::TxReporter {
    msgs.iter_mut()
        .find_map(|msg| match &mut msg.msg {
            SapMsgInner::TmaUnitdataReq(prim) => prim.tx_reporter.take(),
            _ => None,
        })
        .expect("expected TMA-UNITDATA request with TxReporter")
}

fn take_tma_req_reporter_for_endpoint(msgs: &mut [SapMsg], endpoint_id: u32) -> tetra_core::TxReporter {
    msgs.iter_mut()
        .find_map(|msg| match &mut msg.msg {
            SapMsgInner::TmaUnitdataReq(prim) if prim.endpoint_id == endpoint_id => prim.tx_reporter.take(),
            _ => None,
        })
        .expect("expected TMA-UNITDATA request with TxReporter for endpoint")
}

fn attach_tl_data_req_reporter(msg: &mut SapMsg, reporter: TxReporter) {
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut msg.msg else {
        panic!("expected TL-DATA request");
    };
    prim.tx_reporter = Some(reporter);
}

fn llc_pdu_type(msg: &SapMsg) -> Option<LlcPduType> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    prim.pdu.peek_bits(4).and_then(|bits| LlcPduType::try_from(bits).ok())
}

fn tma_cancel_req_handle(msg: &SapMsg) -> Option<i32> {
    let SapMsgInner::TmaCancelReq(prim) = &msg.msg else {
        return None;
    };
    Some(prim.req_handle)
}

fn bl_data_ns(msg: &SapMsg) -> Option<u8> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    let mut pdu = prim.pdu.clone();
    BlData::from_bitbuf(&mut pdu).ok().map(|pdu| pdu.ns)
}

fn bl_adata_nr_ns(msg: &SapMsg) -> Option<(u8, u8)> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    let mut pdu = prim.pdu.clone();
    BlAdata::from_bitbuf(&mut pdu).ok().map(|pdu| (pdu.nr, pdu.ns))
}

fn bl_ack_nr_and_payload_bits(msg: &SapMsg) -> Option<(u8, String)> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    let mut pdu = prim.pdu.clone();
    let ack = BlAck::from_bitbuf(&mut pdu).ok()?;
    pdu.set_raw_start(pdu.get_raw_pos());
    Some((ack.nr, pdu.to_bitstr()))
}

fn bl_ack_prio_nr_and_payload_bits(msg: &SapMsg) -> Option<(i32, u8, String)> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    let (_, payload) = bl_ack_nr_and_payload_bits(msg)?;
    let mut pdu = prim.pdu.clone();
    let ack = BlAck::from_bitbuf(&mut pdu).ok()?;
    Some((prim.pdu_prio, ack.nr, payload))
}

fn al_ack_nr(msg: &SapMsg) -> Option<u8> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    let mut pdu = prim.pdu.clone();
    AlAck::from_bitbuf(&mut pdu).ok().map(|ack| ack.nr)
}

fn al_ack_from_tma_req(msg: &SapMsg) -> Option<AlAck> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    let mut pdu = prim.pdu.clone();
    AlAck::from_bitbuf(&mut pdu).ok()
}

fn al_setup_report(msg: &SapMsg) -> Option<u8> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    let mut pdu = prim.pdu.clone();
    AlSetup::from_bitbuf(&mut pdu).ok().map(|setup| setup.setup_report)
}

fn al_setup_from_tma_req(msg: &SapMsg) -> Option<AlSetup> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    let mut pdu = prim.pdu.clone();
    AlSetup::from_bitbuf(&mut pdu).ok()
}

fn al_data_header_and_fcs_ok(msg: &SapMsg) -> Option<(bool, bool, u8, u8, bool)> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    let mut pdu = prim.pdu.clone();
    let header = AlData::from_bitbuf(&mut pdu).ok()?;
    Some((
        header.final_segment,
        header.acknowledgement_requested,
        header.ns,
        header.ss,
        fcs::check_fcs(&pdu),
    ))
}

fn al_data_header_payload_bits(msg: &SapMsg) -> Option<(bool, bool, u8, u8, usize)> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    let mut pdu = prim.pdu.clone();
    let header = AlData::from_bitbuf(&mut pdu).ok()?;
    Some((
        header.final_segment,
        header.acknowledgement_requested,
        header.ns,
        header.ss,
        pdu.get_len_remaining(),
    ))
}

fn tl_data_ind_handle_and_payload_bits(msg: &SapMsg) -> Option<(i32, String)> {
    let SapMsgInner::TlaTlDataIndBl(prim) = &msg.msg else {
        return None;
    };
    Some((
        prim.req_handle,
        prim.tl_sdu.as_ref().map(|tl_sdu| tl_sdu.to_bitstr()).unwrap_or_default(),
    ))
}

fn tl_data_ind_endpoint_handle_and_payload_bits(msg: &SapMsg) -> Option<(u32, i32, String)> {
    let SapMsgInner::TlaTlDataIndBl(prim) = &msg.msg else {
        return None;
    };
    Some((
        prim.endpoint_id,
        prim.req_handle,
        prim.tl_sdu.as_ref().map(|tl_sdu| tl_sdu.to_bitstr()).unwrap_or_default(),
    ))
}

fn tma_req_pdu(msg: &SapMsg) -> Option<BitBuffer> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    Some(prim.pdu.clone())
}

fn fcs_payload_bits_after_header(mut pdu: BitBuffer, payload_end_raw: usize) -> String {
    pdu.set_raw_end(payload_end_raw);
    pdu.set_raw_start(pdu.get_raw_pos());
    pdu.to_bitstr()
}

fn find_tla_report(msgs: &[SapMsg], req_handle: i32, report: i32) -> bool {
    msgs.iter().any(|msg| {
        matches!(&msg.msg, SapMsgInner::TlaTlReportInd(prim)
            if prim.req_handle == Some(req_handle) && prim.report == report)
    })
}

fn tma_req_handle(msg: &SapMsg) -> Option<i32> {
    let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
        return None;
    };
    Some(prim.req_handle)
}

fn al_segment_headers(msgs: &[SapMsg]) -> Vec<(bool, bool, u8, u8, usize)> {
    msgs.iter()
        .filter_map(|msg| {
            if llc_pdu_type(msg) == Some(LlcPduType::AlDataAlFinal) {
                al_data_header_payload_bits(msg)
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn test_n251_oversized_bl_data_reports_failed_without_tma_req() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 25101;
    let reporter = TxReporter::new();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_endpoint_handle_and_fcs(addr, 0, req_handle, true);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.tl_sdu = BitBuffer::from_bytes(&[0xA5; 325]);
    prim.tx_reporter = Some(reporter.clone());

    // EN 300 392-2 Annex A N.251 limits a basic-link TL-SDU to 2595 bits
    // when FCS is present. Reject before assigning N(S) or queueing TMA.
    test.submit_message(req);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        find_tla_report(&sink_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "oversized TL-DATA.req should report failed transfer immediately"
    );
    assert_eq!(reporter.get_state(), TxState::Discarded);
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "oversized TL-DATA.req must not reach TMA/MAC"
    );
}

#[test]
fn test_n251_oversized_bl_udata_reports_failed_without_tma_req() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 25102;
    let reporter = TxReporter::new_unacked();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_unitdata_req_with_repeats_handle_and_fcs(addr, &[0x5A; 325], req_handle, Some(0), true);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-UNITDATA request");
    };
    prim.tx_reporter = Some(reporter.clone());

    // EN 300 392-2 Annex A N.251 applies to unacknowledged basic-link
    // TL-UNITDATA as well as acknowledged TL-DATA.
    test.submit_message(req);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        find_tla_report(&sink_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "oversized TL-UNITDATA.req should report failed transfer immediately"
    );
    assert_eq!(reporter.get_state(), TxState::Discarded);
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "oversized TL-UNITDATA.req must not reach TMA/MAC"
    );
}

#[test]
fn test_n251_oversized_tl_data_response_preserves_pending_standalone_ack() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind(addr, 1));
    test.deliver_all_messages();
    let data_ind_msgs = test.dump_sinks();
    let (ind_handle, _) = data_ind_msgs
        .iter()
        .find_map(tl_data_ind_handle_and_payload_bits)
        .expect("incoming BL-DATA should produce TL-DATA.ind with retained handle");
    assert!(
        data_ind_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "incoming BL-DATA acknowledgement should still be pending before MAC-ready tick"
    );

    // EN 300 392-2 clause 22.3.2.3(b/c) allows response payload in BL-ACK,
    // but Annex A N.251 still bounds that TL-SDU. An oversized response must
    // not consume the waiting ACK for the received BL-DATA.
    test.submit_message(build_tl_data_resp_with_endpoint_handle_and_fcs(
        addr,
        0,
        ind_handle,
        &[0xCC; 325],
        true,
    ));
    test.deliver_all_messages();
    let rejected_resp_msgs = test.dump_sinks();

    assert!(
        find_tla_report(&rejected_resp_msgs, ind_handle, TLA_REPORT_FAILED_TRANSFER),
        "oversized TL-DATA.response should report failed transfer"
    );
    assert!(
        rejected_resp_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "oversized TL-DATA.response must not emit BL-ACK-with-payload or fallback BL-DATA"
    );

    test.run_stack(Some(1));
    let ack_msgs = test.dump_sinks();
    assert_eq!(
        ack_msgs.iter().find_map(bl_ack_nr_and_payload_bits),
        Some((1, String::new())),
        "pending ACK should remain available for standalone BL-ACK"
    );
}

#[test]
fn test_n251_no_fcs_allows_four_extra_octets() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 25104;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    // EN 300 392-2 Annex A N.251: without the optional FCS, the TL-SDU part
    // may use the four octets otherwise occupied by the FCS. 328 octets is
    // 2624 bits, within 2595 + 32 bits.
    test.submit_message(build_tl_data_req_with_payload_handle_timeslot(addr, &[0x5A; 328], req_handle, 1));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        find_tla_report(&sink_msgs, req_handle, TLA_REPORT_NO_SPECIFIC_REPORT),
        "in-range no-FCS TL-DATA.req should be accepted"
    );
    assert!(
        !find_tla_report(&sink_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "in-range no-FCS TL-DATA.req must not be rejected by N.251"
    );
    assert!(
        sink_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlData)),
        "in-range no-FCS TL-DATA.req should reach TMA/MAC as BL-DATA"
    );
}

#[test]
fn test_al_setup_success_response_establishes_original_acknowledged_link() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_al_setup_ind(addr, 0, 0));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    let setup_response = sink_msgs
        .iter()
        .find(|msg| llc_pdu_type(msg) == Some(LlcPduType::AlSetup))
        .expect("supported AL-SETUP should produce AL-SETUP success response");
    assert_eq!(al_setup_report(setup_response), Some(AlSetup::SETUP_REPORT_SUCCESS));
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "AL-SETUP establishes LLC link state; it must not be delivered to MLE as SNDCP data"
    );
}

#[test]
fn test_al_setup_four_slot_phase_mod_request_is_negotiated_down_before_data_transfer() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 2, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let request = AlSetup {
        acknowledged_service: true,
        advanced_link_number: 0,
        max_tl_sdu_len_code: 6,
        connection_width: true,
        advanced_link_symmetry: false,
        uplink_timeslots: Some(3),
        downlink_timeslots: None,
        throughput_code: 6,
        window_size_code: 1,
        max_tl_sdu_retransmissions: 3,
        max_segment_retransmissions: 3,
        setup_report: AlSetup::SETUP_REPORT_SERVICE_DEFINITION,
        ns: None,
        augmented: None,
    };

    test.submit_message(build_al_setup_ind_with_setup(addr, 0, request));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();
    let setup_response = sink_msgs
        .iter()
        .find_map(al_setup_from_tma_req)
        .expect("4-slot phase-mod AL-SETUP should produce a negotiated response");
    assert_eq!(setup_response.setup_report, AlSetup::SETUP_REPORT_SERVICE_CHANGE);
    assert_eq!(
        setup_response.uplink_timeslots,
        Some(0),
        "N.264 response must advertise the single-slot PDCH fallback, not echo a 4-slot request"
    );
    assert_eq!(setup_response.throughput_code, 6);

    test.submit_message(build_al_final_ar_ind(addr, 0, 0, &[0xA5]));
    test.deliver_all_messages();
    assert!(
        test.dump_sinks()
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "AL-DATA before the MS accepts lower QoS must not be delivered as established-link SNDCP data"
    );

    let mut accepted = setup_response;
    accepted.setup_report = AlSetup::SETUP_REPORT_SUCCESS;
    test.submit_message(build_al_setup_ind_with_setup(addr, 0, accepted));
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_al_final_ar_ind(addr, 0, 0, &[0xA5]));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();
    assert!(
        sink_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::TlaTlDataIndBl(prim) if prim.link_id == 1 && prim.tl_sdu.is_some()
        )),
        "accepted lower-QoS original AL should deliver AL-FINAL-AR as link_id=1 TL-DATA.ind"
    );
}

#[test]
fn test_inbound_al_final_ar_delivers_tldata_with_link_id_and_ack() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_al_setup_ind(addr, 0, 0));
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_al_final_ar_ind(addr, 0, 0, &[0xA5]));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    let data_ind = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataIndBl(prim) => Some(prim),
            _ => None,
        })
        .expect("complete AL-FINAL-AR should deliver TL-DATA.ind to MLE");
    assert_eq!(data_ind.main_address, addr);
    assert_eq!(data_ind.link_id, 1, "AL number 1 maps to non-basic link_id 1");
    assert_eq!(data_ind.endpoint_id, 0);
    assert!(data_ind.fcs_flag);
    assert_eq!(
        data_ind.tl_sdu.as_ref().map(BitBuffer::to_bitstr),
        Some("10100101".to_string()),
        "LLC must strip the AL FCS before delivering TL-SDU to MLE"
    );

    let ack = sink_msgs
        .iter()
        .find(|msg| llc_pdu_type(msg) == Some(LlcPduType::AlAckAlRnr))
        .expect("AL-FINAL-AR should be acknowledged");
    assert_eq!(al_ack_nr(ack), Some(0));
    let SapMsgInner::TmaUnitdataReq(ack_prim) = &ack.msg else {
        panic!("expected AL-ACK as TMA-UNITDATA.req");
    };
    assert_eq!(ack_prim.pdu_prio, 5);
    assert!(ack_prim.stealing_permission);
}

#[test]
fn test_inbound_incomplete_al_data_ar_sends_selective_ack_not_whole_repeat() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_al_setup_ind(addr, 0, 0));
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_al_data_ar_ind(addr, 0, 0, 1, &[0xA5]));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "incomplete AL TL-SDU must not be delivered to MLE before missing segment 0 arrives"
    );

    let ack = sink_msgs
        .iter()
        .find(|msg| llc_pdu_type(msg) == Some(LlcPduType::AlAckAlRnr))
        .and_then(al_ack_from_tma_req)
        .expect("AL-DATA-AR with a missing older segment should be selectively acknowledged");
    assert_eq!(ack.nr, 0);
    assert_eq!(ack.sr, Some(0));
    assert_eq!(ack.acknowledgement_length, 2);
    assert_eq!(ack.acknowledgement_bitmap, 1);
    assert!(
        !ack.requests_repeat_entire_tl_sdu(),
        "EN 300 392-2 22.3.3.2.3 uses selective ACK for missing segments; whole repeat is for TL-SDU FCS failure"
    );
}

#[test]
fn test_outbound_nonzero_link_tldata_uses_al_final_ar_and_completes_on_al_ack() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 7101;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_al_setup_ind(addr, 0, 0));
    test.deliver_all_messages();
    test.dump_sinks();

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.link_id = 1;
    prim.pdu_prio = 4;
    prim.fcs_flag = false;

    test.submit_message(req);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        find_tla_report(&sink_msgs, req_handle, TLA_REPORT_NO_SPECIFIC_REPORT),
        "accepted AL TL-DATA.req should report no-specific-report first"
    );
    assert!(
        sink_msgs.iter().all(|msg| llc_pdu_type(msg) != Some(LlcPduType::BlData)),
        "nonzero link_id must not fall back to basic-link BL-DATA"
    );
    let al_data = sink_msgs
        .iter()
        .find(|msg| llc_pdu_type(msg) == Some(LlcPduType::AlDataAlFinal))
        .expect("nonzero link_id TL-DATA.req should emit AL-FINAL-AR");
    assert_eq!(
        al_data_header_and_fcs_ok(al_data),
        Some((true, true, 0, 0, true)),
        "outbound WAP/SNDCP AL response should be a single AL-FINAL-AR with valid mandatory AL FCS"
    );

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessReservedOrStealing));
    test.deliver_all_messages();
    let progress_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&progress_msgs, req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION),
        "MAC completion should surface as first-complete TL report before peer AL-ACK"
    );

    test.submit_message(build_al_ack_ind(addr, 0, 0));
    test.deliver_all_messages();
    let complete_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&complete_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
        "complete AL-ACK should finish the SNDCP/MLE lower-layer transfer"
    );
}

#[test]
fn test_same_link_al_setup_clears_pending_outbound_before_ns_reset() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let first_req_handle = 7116;
    let second_req_handle = 7117;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_al_setup_ind(addr, 0, 0));
    test.deliver_all_messages();
    test.dump_sinks();

    let mut first_req = build_tl_data_req_with_handle(addr, first_req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut first_req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.link_id = 1;
    prim.pdu_prio = 4;
    prim.fcs_flag = false;

    test.submit_message(first_req);
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    assert!(
        al_segment_headers(&first_msgs).iter().any(|(_, _, ns, _, _)| *ns == 0),
        "first transfer should use N(S)=0 after initial AL setup"
    );

    test.submit_message(build_al_setup_ind(addr, 0, 0));
    test.deliver_all_messages();
    let reset_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&reset_msgs, first_req_handle, TLA_REPORT_FAILED_TRANSFER),
        "same-link AL setup/reset must fail and clear the pending old transfer before N(S) restarts"
    );
    assert!(
        reset_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::AlSetup)),
        "same-link setup should still be answered with AL-SETUP"
    );

    let mut second_req = build_tl_data_req_with_handle(addr, second_req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut second_req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.link_id = 1;
    prim.pdu_prio = 4;
    prim.fcs_flag = false;

    test.submit_message(second_req);
    test.run_stack(Some(1));
    let second_msgs = test.dump_sinks();
    assert!(
        al_segment_headers(&second_msgs).iter().any(|(_, _, ns, _, _)| *ns == 0),
        "new transfer should also start from N(S)=0 after same-link AL reset"
    );
    for handle in second_msgs.iter().filter_map(tma_req_handle) {
        test.submit_message(build_tma_report_ind(handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_al_ack_ind(addr, 0, 0));
    test.deliver_all_messages();
    let complete_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&complete_msgs, second_req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
        "AL-ACK N(R)=0 after reset must match the new transfer, not become ambiguous with stale outbound state"
    );
}

#[test]
fn test_outbound_nonzero_link_tldata_completes_on_complete_al_rnr() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 7104;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_al_setup_ind(addr, 0, 0));
    test.deliver_all_messages();
    test.dump_sinks();

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.link_id = 1;
    prim.pdu_prio = 4;
    prim.fcs_flag = false;

    test.submit_message(req);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let al_data = sink_msgs
        .iter()
        .find(|msg| llc_pdu_type(msg) == Some(LlcPduType::AlDataAlFinal))
        .expect("nonzero link_id TL-DATA.req should emit AL-FINAL-AR");
    assert_eq!(al_data_header_and_fcs_ok(al_data), Some((true, true, 0, 0, true)));

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessReservedOrStealing));
    test.deliver_all_messages();
    let progress_msgs = test.dump_sinks();
    assert!(find_tla_report(&progress_msgs, req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION));

    test.submit_message(build_al_rnr_complete_ind(addr, 0, 0));
    test.deliver_all_messages();
    let complete_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&complete_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
        "complete AL-RNR acknowledges the TL-SDU even while applying receiver-not-ready flow control"
    );
}

#[test]
fn test_outbound_nonzero_link_tldata_waits_t252_before_al_retransmission() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 7105;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_al_setup_ind(addr, 0, 0));
    test.deliver_all_messages();
    test.dump_sinks();

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.link_id = 1;
    prim.pdu_prio = 4;
    prim.fcs_flag = false;

    test.submit_message(req);
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    assert_eq!(
        first_msgs
            .iter()
            .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::AlDataAlFinal))
            .count(),
        1,
        "initial AL-FINAL-AR should be submitted once"
    );

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let progress_msgs = test.dump_sinks();
    assert!(find_tla_report(&progress_msgs, req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION));

    test.run_stack(Some((T251_SENDER_RETRY_TIMER + 4) as usize));
    let t251_window_msgs = test.dump_sinks();
    assert!(
        t251_window_msgs
            .iter()
            .all(|msg| llc_pdu_type(msg) != Some(LlcPduType::AlDataAlFinal)),
        "AL ACK wait is T.252, so T.251 expiry must not retransmit AL-FINAL-AR"
    );

    test.run_stack(Some(T252_ACK_WAITING_TIMER as usize));
    let retry_msgs = test.dump_sinks();
    assert!(
        retry_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::AlDataAlFinal)),
        "missing peer AL-ACK after T.252 should retransmit AL-FINAL-AR"
    );
}

#[test]
fn test_outbound_nonzero_link_late_al_ack_after_n273_zero_completes_during_pdch_grace() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let endpoint_id = 1;
    let req_handle = 7106;
    let service_reporter = TxReporter::new();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut setup = default_al_setup();
    setup.max_tl_sdu_retransmissions = 0;
    test.submit_message(build_al_setup_ind_with_setup(addr, endpoint_id, setup));
    test.deliver_all_messages();
    test.dump_sinks();

    let mut req = build_tl_data_req_with_handle_timeslot(addr, req_handle, 2);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.endpoint_id = endpoint_id;
    prim.link_id = 1;
    prim.tx_reporter = Some(service_reporter.clone());

    test.submit_message(req);
    test.run_stack(Some(1));
    let mut first_msgs = test.dump_sinks();
    assert_eq!(
        first_msgs
            .iter()
            .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::AlDataAlFinal))
            .count(),
        1,
        "initial WAP/SNDCP AL-FINAL-AR should be submitted once on the assigned PDCH"
    );
    let reporter = take_first_tma_req_reporter(&mut first_msgs);
    reporter.mark_transmitted();

    test.run_stack(Some(T252_ACK_WAITING_TIMER as usize));
    let grace_msgs = test.dump_sinks();
    assert!(
        !find_tla_report(&grace_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "late-ACK grace should retain an AL transfer after MAC success before reporting failed transfer"
    );
    assert!(
        grace_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "N.273=0 grace must not queue extra AL retransmissions"
    );
    assert_eq!(service_reporter.get_state(), TxState::Transmitted);

    test.submit_message(build_al_ack_ind(addr, endpoint_id, 0));
    test.run_stack(Some(1));
    let complete_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&complete_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
        "matching AL-ACK inside the late grace should complete the WAP/SNDCP AL transfer"
    );
    assert_eq!(service_reporter.get_state(), TxState::Acknowledged);
}

#[test]
fn test_outbound_nonzero_link_al_reports_failed_after_late_ack_grace_expires() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let endpoint_id = 1;
    let req_handle = 7107;
    let service_reporter = TxReporter::new();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut setup = default_al_setup();
    setup.max_tl_sdu_retransmissions = 0;
    let max_segment_retransmissions = setup.max_segment_retransmissions as usize;
    test.submit_message(build_al_setup_ind_with_setup(addr, endpoint_id, setup));
    test.deliver_all_messages();
    test.dump_sinks();

    let mut req = build_tl_data_req_with_handle_timeslot(addr, req_handle, 2);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.endpoint_id = endpoint_id;
    prim.link_id = 1;
    prim.tx_reporter = Some(service_reporter.clone());

    test.submit_message(req);
    test.run_stack(Some(1));
    let mut first_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut first_msgs);
    reporter.mark_transmitted();

    test.run_stack(Some(T252_ACK_WAITING_TIMER as usize));
    let grace_msgs = test.dump_sinks();
    assert!(!find_tla_report(&grace_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER));

    let mut ack_probe_count = 0usize;
    for _ in 0..8 {
        test.run_stack(Some(T252_ACK_WAITING_TIMER as usize));
        let mut retry_msgs = test.dump_sinks();
        let retry_segments = al_segment_headers(&retry_msgs);
        if retry_segments.is_empty() {
            continue;
        }
        assert_eq!(
            retry_segments
                .iter()
                .map(|(final_segment, acknowledgement_requested, _, ss, _)| (*final_segment, *acknowledgement_requested, *ss))
                .collect::<Vec<_>>(),
            vec![(true, true, 0)],
            "T.252 should repeat the single AL-FINAL-AR ACK request while N.274 remains"
        );
        ack_probe_count += retry_segments.len();
        let retry_reporter = take_first_tma_req_reporter(&mut retry_msgs);
        retry_reporter.mark_transmitted();
        if ack_probe_count >= max_segment_retransmissions {
            break;
        }
    }
    assert_eq!(
        ack_probe_count, max_segment_retransmissions,
        "test must exhaust negotiated N.274 ACK-request probes before late-failure grace"
    );

    test.run_stack(Some(T252_ACK_WAITING_TIMER as usize));
    let exhausted_msgs = test.dump_sinks();
    assert!(
        !find_tla_report(&exhausted_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "N.274 exhaustion should start late-ACK grace before reporting failed transfer"
    );

    test.run_stack(Some((72 * 4 + 8) as usize));
    let failed_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&failed_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "EN 300 392-2 22.3.3.2.4 requires failed transfer after N.273 is exceeded and late-ACK grace expires"
    );
    assert!(
        failed_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "late-ACK grace expiry must not queue retransmissions after N.273=0"
    );
    assert_eq!(service_reporter.get_state(), TxState::Lost);
}

#[test]
fn test_outbound_nonzero_link_al_accepts_delayed_ack_inside_extended_grace() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let endpoint_id = 1;
    let req_handle = 7113;
    let service_reporter = TxReporter::new();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut setup = default_al_setup();
    setup.max_tl_sdu_retransmissions = 0;
    test.submit_message(build_al_setup_ind_with_setup(addr, endpoint_id, setup));
    test.deliver_all_messages();
    test.dump_sinks();

    let mut req = build_tl_data_req_with_handle_timeslot(addr, req_handle, 2);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.endpoint_id = endpoint_id;
    prim.link_id = 1;
    prim.tx_reporter = Some(service_reporter.clone());

    test.submit_message(req);
    test.run_stack(Some(1));
    let mut first_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut first_msgs);
    reporter.mark_transmitted();

    test.run_stack(Some(T252_ACK_WAITING_TIMER as usize));
    test.dump_sinks();
    test.run_stack(Some((18 * 4 + 8) as usize));
    let delayed_window_msgs = test.dump_sinks();
    assert!(
        !find_tla_report(&delayed_window_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "AL late-ACK grace must cover delayed terminal ACKs observed on WAP PDCH without RF retransmission"
    );
    assert!(
        al_segment_headers(&delayed_window_msgs)
            .iter()
            .all(|(final_segment, acknowledgement_requested, _, _, _)| *final_segment && *acknowledgement_requested),
        "any delayed-window AL retry must only repeat an ACK-request segment"
    );

    test.submit_message(build_al_ack_ind(addr, endpoint_id, 0));
    test.deliver_all_messages();
    let complete_msgs = test.dump_sinks();
    assert!(find_tla_report(&complete_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER));
    assert_eq!(service_reporter.get_state(), TxState::Acknowledged);
}

#[test]
fn test_outbound_nonzero_link_al_no_late_ack_grace_after_mac_discard() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let endpoint_id = 1;
    let req_handle = 7108;
    let service_reporter = TxReporter::new();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut setup = default_al_setup();
    setup.max_tl_sdu_retransmissions = 0;
    test.submit_message(build_al_setup_ind_with_setup(addr, endpoint_id, setup));
    test.deliver_all_messages();
    test.dump_sinks();

    let mut req = build_tl_data_req_with_handle_timeslot(addr, req_handle, 2);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.endpoint_id = endpoint_id;
    prim.link_id = 1;
    prim.tx_reporter = Some(service_reporter.clone());

    test.submit_message(req);
    test.run_stack(Some(1));
    let mut first_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut first_msgs);
    reporter.mark_discarded();

    test.run_stack(Some(T252_ACK_WAITING_TIMER as usize + 4));
    let failed_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&failed_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "discarded MAC transfer must fail after T.252/N.273 instead of waiting in late-ACK grace"
    );
    assert_eq!(service_reporter.get_state(), TxState::Discarded);
}

#[test]
fn test_outbound_nonzero_link_tldata_segments_large_tl_sdu_and_completes_on_al_ack() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 7102;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_al_setup_ind(addr, 0, 0));
    test.deliver_all_messages();
    test.dump_sinks();

    let payload: Vec<u8> = (0..180).map(|idx| idx as u8).collect();
    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.link_id = 1;
    prim.pdu_prio = 4;
    prim.fcs_flag = false;
    prim.tl_sdu = BitBuffer::from_bytes(&payload);

    test.submit_message(req);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        find_tla_report(&sink_msgs, req_handle, TLA_REPORT_NO_SPECIFIC_REPORT),
        "accepted segmented AL TL-DATA.req should report no-specific-report first"
    );
    let al_segments: Vec<&SapMsg> = sink_msgs
        .iter()
        .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::AlDataAlFinal))
        .collect();
    assert!(al_segments.len() > 1, "large AL TL-SDU should be segmented");

    let mut segment_req_handles = Vec::new();
    for (idx, msg) in al_segments.iter().enumerate() {
        let (final_segment, acknowledgement_requested, ns, ss, payload_bits) =
            al_data_header_payload_bits(msg).expect("AL segment should parse");
        assert_eq!(ns, 0);
        assert_eq!(ss as usize, idx);
        assert!(
            payload_bits <= 208,
            "AL segment payload must stay inside the SCH/F MAC-RESOURCE budget"
        );
        assert_eq!(final_segment, idx == al_segments.len() - 1);
        assert_eq!(acknowledgement_requested, idx == al_segments.len() - 1 || (idx + 1) % 4 == 0);

        let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
            panic!("expected AL segment as TMA-UNITDATA.req");
        };
        segment_req_handles.push(prim.req_handle);
    }
    assert!(
        segment_req_handles.iter().all(|handle| *handle != req_handle),
        "multi-segment AL uses internal TMA handles while preserving service req_handle"
    );

    for segment_req_handle in segment_req_handles {
        test.submit_message(build_tma_report_ind(segment_req_handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    let progress_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&progress_msgs, req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION),
        "first-complete report should be emitted after all AL segments reached MAC"
    );

    test.submit_message(build_al_ack_ind(addr, 0, 0));
    test.deliver_all_messages();
    let complete_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&complete_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
        "complete AL-ACK should finish the segmented SNDCP/MLE lower-layer transfer"
    );
}

#[test]
fn test_outbound_segmented_al_requests_periodic_ack_and_retries_selective_missing_segment() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 7103;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_al_setup_ind(addr, 0, 0));
    test.deliver_all_messages();
    test.dump_sinks();

    let payload: Vec<u8> = (0..560).map(|idx| idx as u8).collect();
    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.link_id = 1;
    prim.pdu_prio = 4;
    prim.fcs_flag = false;
    prim.tl_sdu = BitBuffer::from_bytes(&payload);

    test.submit_message(req);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let al_segments: Vec<&SapMsg> = sink_msgs
        .iter()
        .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::AlDataAlFinal))
        .collect();
    assert!(al_segments.len() > 16, "test vector must cross the periodic AL-DATA-AR boundary");

    let mut segment_req_handles = Vec::new();
    for (idx, msg) in al_segments.iter().enumerate() {
        let (final_segment, acknowledgement_requested, _ns, ss, _payload_bits) =
            al_data_header_payload_bits(msg).expect("AL segment should parse");
        assert_eq!(ss as usize, idx);
        assert_eq!(
            acknowledgement_requested,
            final_segment || (idx + 1) % 4 == 0,
            "LLC should request AL-ACK periodically and on AL-FINAL"
        );

        let SapMsgInner::TmaUnitdataReq(prim) = &msg.msg else {
            panic!("expected AL segment as TMA-UNITDATA.req");
        };
        segment_req_handles.push(prim.req_handle);
    }

    for segment_req_handle in segment_req_handles {
        test.submit_message(build_tma_report_ind(segment_req_handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_al_selective_ack_ind(addr, 0, 0, 8));
    test.run_stack(Some(1));
    let retry_msgs = test.dump_sinks();
    let retry_segments: Vec<_> = retry_msgs
        .iter()
        .filter_map(|msg| {
            if llc_pdu_type(msg) == Some(LlcPduType::AlDataAlFinal) {
                al_data_header_payload_bits(msg)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(
        retry_segments.iter().map(|(_, _, _, ss, _)| *ss).collect::<Vec<_>>(),
        vec![8],
        "selective AL-ACK with S(R)=8 should requeue only the missing segment"
    );
    assert!(
        !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
        "partial AL-ACK must not complete the TL-SDU before a complete AL-ACK"
    );
}

#[test]
fn test_outbound_segmented_al_t252_repeats_ack_request_before_full_tl_sdu_retransmit() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 7115;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut setup = default_al_setup();
    setup.max_tl_sdu_retransmissions = 1;
    setup.max_segment_retransmissions = 3;
    test.submit_message(build_al_setup_ind_with_setup(addr, 0, setup));
    test.deliver_all_messages();
    test.dump_sinks();

    let payload = [0x5a; 50];
    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.link_id = 1;
    prim.pdu_prio = 4;
    prim.fcs_flag = false;
    prim.tl_sdu = BitBuffer::from_bytes(&payload);

    test.submit_message(req);
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    let first_segments = al_segment_headers(&first_msgs);
    assert_eq!(
        first_segments.iter().map(|(_, _, _, ss, _)| *ss).collect::<Vec<_>>(),
        vec![0, 1, 2],
        "test vector should create three AL segments"
    );

    for handle in first_msgs.iter().filter_map(tma_req_handle) {
        test.submit_message(build_tma_report_ind(handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    test.dump_sinks();

    test.run_stack(Some(T252_ACK_WAITING_TIMER as usize));
    let retry_msgs = test.dump_sinks();
    let retry_segments = al_segment_headers(&retry_msgs);
    assert_eq!(
        retry_segments
            .iter()
            .map(|(final_segment, acknowledgement_requested, _, ss, _)| (*final_segment, *acknowledgement_requested, *ss))
            .collect::<Vec<_>>(),
        vec![(true, true, 2)],
        "T.252 expiry should repeat the AL-FINAL-AR ACK request, not restart the complete TL-SDU"
    );
    assert!(
        !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "missing AL-ACK after one T.252 must not fail or restart the WAP TL-SDU while N.274 remains"
    );
}

#[test]
fn test_outbound_segmented_al_selective_retry_requests_ack_for_nonfinal_segment() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 7114;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut setup = default_al_setup();
    setup.max_tl_sdu_retransmissions = 0;
    setup.max_segment_retransmissions = 3;
    test.submit_message(build_al_setup_ind_with_setup(addr, 0, setup));
    test.deliver_all_messages();
    test.dump_sinks();

    let payload = [0x5a; 25];
    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.link_id = 1;
    prim.pdu_prio = 4;
    prim.fcs_flag = false;
    prim.tl_sdu = BitBuffer::from_bytes(&payload);

    test.submit_message(req);
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    let first_segments = al_segment_headers(&first_msgs);
    assert_eq!(
        first_segments
            .iter()
            .map(|(_, acknowledgement_requested, _, ss, _)| (*acknowledgement_requested, *ss))
            .collect::<Vec<_>>(),
        vec![(false, 0), (true, 1)],
        "test vector should model a two-segment WAP ConnectReply where only the final segment asks for AL-ACK"
    );

    for handle in first_msgs.iter().filter_map(tma_req_handle) {
        test.submit_message(build_tma_report_ind(handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_al_selective_ack_ind_with_bitmap(addr, 0, 0, 0, 1, 2));
    test.run_stack(Some(1));
    let retry_msgs = test.dump_sinks();
    let retry_segments = al_segment_headers(&retry_msgs);
    assert_eq!(
        retry_segments
            .iter()
            .map(|(final_segment, acknowledgement_requested, _, ss, _)| (*final_segment, *acknowledgement_requested, *ss))
            .collect::<Vec<_>>(),
        vec![(false, true, 0)],
        "selective retransmission of a non-final missing segment must request AL-ACK so the peer can complete the TL-SDU"
    );
}

#[test]
fn test_outbound_segmented_al_t252_retries_selectively_requested_segments_before_n273_failure() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 7109;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut setup = default_al_setup();
    setup.max_tl_sdu_retransmissions = 0;
    setup.max_segment_retransmissions = 2;
    test.submit_message(build_al_setup_ind_with_setup(addr, 0, setup));
    test.deliver_all_messages();
    test.dump_sinks();

    let payload = [0x5a; 50];
    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.link_id = 1;
    prim.pdu_prio = 4;
    prim.fcs_flag = false;
    prim.tl_sdu = BitBuffer::from_bytes(&payload);

    test.submit_message(req);
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    let first_segments: Vec<_> = first_msgs
        .iter()
        .filter_map(|msg| {
            if llc_pdu_type(msg) == Some(LlcPduType::AlDataAlFinal) {
                al_data_header_payload_bits(msg)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        first_segments.iter().map(|(_, _, _, ss, _)| *ss).collect::<Vec<_>>(),
        vec![0, 1, 2],
        "50-byte WAP-like AL payload should require three original AL segments"
    );

    for handle in first_msgs.iter().filter_map(tma_req_handle) {
        test.submit_message(build_tma_report_ind(handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    let progress_msgs = test.dump_sinks();
    assert!(find_tla_report(&progress_msgs, req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION));

    test.submit_message(build_al_selective_ack_ind_with_bitmap(addr, 0, 0, 1, 0, 3));
    test.run_stack(Some(1));
    let first_retry_msgs = test.dump_sinks();
    let first_retry_segments = al_segment_headers(&first_retry_msgs);
    assert_eq!(
        first_retry_segments.iter().map(|(_, _, _, ss, _)| *ss).collect::<Vec<_>>(),
        vec![1, 2],
        "selective AL-ACK should request only the missing segments"
    );

    for handle in first_retry_msgs.iter().filter_map(tma_req_handle) {
        test.submit_message(build_tma_report_ind(handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    test.dump_sinks();

    test.run_stack(Some(T252_ACK_WAITING_TIMER as usize));
    let second_retry_msgs = test.dump_sinks();
    let second_retry_segments = al_segment_headers(&second_retry_msgs);
    assert_eq!(
        second_retry_segments.iter().map(|(_, _, _, ss, _)| *ss).collect::<Vec<_>>(),
        vec![1, 2],
        "T.252 expiry after selective ACK must use N.274 segment retries before any N.273 TL-SDU failure"
    );
    assert!(
        !find_tla_report(&second_retry_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "N.273=0 must not fail a partially acknowledged TL-SDU while N.274 segment retries remain"
    );

    for handle in second_retry_msgs.iter().filter_map(tma_req_handle) {
        test.submit_message(build_tma_report_ind(handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_al_ack_ind(addr, 0, 0));
    test.deliver_all_messages();
    let complete_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&complete_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
        "complete AL-ACK after selective N.274 retries must complete the TL-SDU"
    );

    test.run_stack(Some(T252_ACK_WAITING_TIMER as usize));
    let post_complete_msgs = test.dump_sinks();
    assert!(
        al_segment_headers(&post_complete_msgs).is_empty(),
        "completed AL TL-SDU must be removed and not retried again"
    );
}

#[test]
fn test_outbound_segmented_al_repeated_selective_ack_does_not_exhaust_n274_while_retry_inflight() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 7113;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut setup = default_al_setup();
    setup.max_tl_sdu_retransmissions = 0;
    setup.max_segment_retransmissions = 1;
    test.submit_message(build_al_setup_ind_with_setup(addr, 0, setup));
    test.deliver_all_messages();
    test.dump_sinks();

    let payload = [0x4d; 50];
    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.link_id = 1;
    prim.pdu_prio = 4;
    prim.fcs_flag = false;
    prim.tl_sdu = BitBuffer::from_bytes(&payload);

    test.submit_message(req);
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    assert_eq!(
        al_segment_headers(&first_msgs)
            .iter()
            .map(|(_, _, _, ss, _)| *ss)
            .collect::<Vec<_>>(),
        vec![0, 1, 2],
        "test vector should create three original AL segments"
    );

    for handle in first_msgs.iter().filter_map(tma_req_handle) {
        test.submit_message(build_tma_report_ind(handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_al_selective_ack_ind_with_bitmap(addr, 0, 0, 1, 0, 1));
    test.run_stack(Some(1));
    let retry_msgs = test.dump_sinks();
    assert_eq!(
        al_segment_headers(&retry_msgs)
            .iter()
            .map(|(_, _, _, ss, _)| *ss)
            .collect::<Vec<_>>(),
        vec![1],
        "first selective ACK should queue one S(S)=1 retransmission"
    );

    test.submit_message(build_al_selective_ack_ind_with_bitmap(addr, 0, 0, 1, 0, 1));
    test.run_stack(Some(1));
    let duplicate_ack_msgs = test.dump_sinks();
    assert!(
        al_segment_headers(&duplicate_ack_msgs).is_empty(),
        "duplicate selective ACK must not resubmit a segment retry that is already in flight"
    );
    assert!(
        !find_tla_report(&duplicate_ack_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "duplicate selective ACK must not spend another N.274 retry before MAC progress"
    );

    test.submit_message(build_al_ack_ind(addr, 0, 0));
    test.deliver_all_messages();
    let complete_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&complete_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
        "complete AL-ACK after a duplicate selective ACK should still finish the transfer"
    );
}

#[test]
fn test_outbound_segmented_al_selective_n274_exhaustion_waits_for_late_ack_without_resubmitting() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 7112;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut setup = default_al_setup();
    setup.max_tl_sdu_retransmissions = 0;
    setup.max_segment_retransmissions = 1;
    test.submit_message(build_al_setup_ind_with_setup(addr, 0, setup));
    test.deliver_all_messages();
    test.dump_sinks();

    let payload = [0x6b; 50];
    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.link_id = 1;
    prim.pdu_prio = 4;
    prim.fcs_flag = false;
    prim.tl_sdu = BitBuffer::from_bytes(&payload);

    test.submit_message(req);
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    assert_eq!(
        al_segment_headers(&first_msgs)
            .iter()
            .map(|(_, _, _, ss, _)| *ss)
            .collect::<Vec<_>>(),
        vec![0, 1, 2],
        "50-byte WAP-like AL payload should require three original AL segments"
    );

    for handle in first_msgs.iter().filter_map(tma_req_handle) {
        test.submit_message(build_tma_report_ind(handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_al_selective_ack_ind_with_bitmap(addr, 0, 0, 1, 0, 1));
    test.run_stack(Some(1));
    let retry_msgs = test.dump_sinks();
    assert_eq!(
        al_segment_headers(&retry_msgs)
            .iter()
            .map(|(_, _, _, ss, _)| *ss)
            .collect::<Vec<_>>(),
        vec![1],
        "first selective ACK should spend the single allowed N.274 retry on the requested segment"
    );

    for handle in retry_msgs.iter().filter_map(tma_req_handle) {
        test.submit_message(build_tma_report_ind(handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    test.dump_sinks();

    test.run_stack(Some(T252_ACK_WAITING_TIMER as usize));
    let grace_msgs = test.dump_sinks();
    assert!(
        !find_tla_report(&grace_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "N.273=0 must not fail a MAC-successful AL TL-SDU before late-ACK grace expires"
    );
    assert!(
        al_segment_headers(&grace_msgs).is_empty(),
        "N.274 exhaustion with N.273=0 must not resubmit AL segments while waiting for late AL-ACK"
    );

    test.run_stack(Some(8));
    let quiet_grace_msgs = test.dump_sinks();
    assert!(
        quiet_grace_msgs.is_empty(),
        "late-ACK grace must not re-run selective N.274 exhaustion on every scheduler tick"
    );

    test.submit_message(build_al_ack_ind(addr, 0, 0));
    test.deliver_all_messages();
    let complete_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&complete_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
        "complete AL-ACK inside late grace should still finish the WAP/SNDCP AL transfer"
    );
}

#[test]
fn test_outbound_segmented_al_t252_retries_only_segments_marked_bad_in_last_selective_ack() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 7110;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut setup = default_al_setup();
    setup.max_tl_sdu_retransmissions = 0;
    setup.max_segment_retransmissions = 3;
    test.submit_message(build_al_setup_ind_with_setup(addr, 0, setup));
    test.deliver_all_messages();
    test.dump_sinks();

    let payload = [0x63; 50];
    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.link_id = 1;
    prim.pdu_prio = 4;
    prim.fcs_flag = false;
    prim.tl_sdu = BitBuffer::from_bytes(&payload);

    test.submit_message(req);
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    assert_eq!(
        al_segment_headers(&first_msgs)
            .iter()
            .map(|(_, _, _, ss, _)| *ss)
            .collect::<Vec<_>>(),
        vec![0, 1, 2],
        "test vector should create three AL segments"
    );

    for handle in first_msgs.iter().filter_map(tma_req_handle) {
        test.submit_message(build_tma_report_ind(handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_al_selective_ack_ind_with_bitmap(addr, 0, 0, 1, 0, 1));
    test.run_stack(Some(1));
    let first_retry_msgs = test.dump_sinks();
    assert_eq!(
        al_segment_headers(&first_retry_msgs)
            .iter()
            .map(|(_, _, _, ss, _)| *ss)
            .collect::<Vec<_>>(),
        vec![1],
        "selective ACK with ack_len=1 marks only S(R) as bad; later segments are outside the block"
    );

    for handle in first_retry_msgs.iter().filter_map(tma_req_handle) {
        test.submit_message(build_tma_report_ind(handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    test.dump_sinks();

    test.run_stack(Some(T252_ACK_WAITING_TIMER as usize));
    let t252_retry_msgs = test.dump_sinks();
    assert_eq!(
        al_segment_headers(&t252_retry_msgs)
            .iter()
            .map(|(_, _, _, ss, _)| *ss)
            .collect::<Vec<_>>(),
        vec![1],
        "T.252 must retry only segments marked bad in the last selective ACK, not every unacknowledged segment"
    );
}

#[test]
fn test_outbound_segmented_al_selective_n274_exhaustion_restarts_complete_tl_sdu_using_n273() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 7111;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut setup = default_al_setup();
    setup.max_tl_sdu_retransmissions = 1;
    setup.max_segment_retransmissions = 1;
    test.submit_message(build_al_setup_ind_with_setup(addr, 0, setup));
    test.deliver_all_messages();
    test.dump_sinks();

    let payload = [0x73; 50];
    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.link_id = 1;
    prim.pdu_prio = 4;
    prim.fcs_flag = false;
    prim.tl_sdu = BitBuffer::from_bytes(&payload);

    test.submit_message(req);
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    assert_eq!(
        al_segment_headers(&first_msgs)
            .iter()
            .map(|(_, _, _, ss, _)| *ss)
            .collect::<Vec<_>>(),
        vec![0, 1, 2],
        "test vector should create three AL segments"
    );

    for handle in first_msgs.iter().filter_map(tma_req_handle) {
        test.submit_message(build_tma_report_ind(handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_al_selective_ack_ind_with_bitmap(addr, 0, 0, 1, 0, 1));
    test.run_stack(Some(1));
    let first_retry_msgs = test.dump_sinks();
    assert_eq!(
        al_segment_headers(&first_retry_msgs)
            .iter()
            .map(|(_, _, _, ss, _)| *ss)
            .collect::<Vec<_>>(),
        vec![1],
        "first selective ACK should consume the one allowed N.274 segment retry"
    );

    for handle in first_retry_msgs.iter().filter_map(tma_req_handle) {
        test.submit_message(build_tma_report_ind(handle, TmaReport::SuccessReservedOrStealing));
    }
    test.deliver_all_messages();
    test.dump_sinks();

    test.submit_message(build_al_selective_ack_ind_with_bitmap(addr, 0, 0, 1, 0, 1));
    test.run_stack(Some(1));
    let full_retry_msgs = test.dump_sinks();
    assert_eq!(
        al_segment_headers(&full_retry_msgs)
            .iter()
            .map(|(_, _, _, ss, _)| *ss)
            .collect::<Vec<_>>(),
        vec![0, 1, 2],
        "when N.274 is exhausted but N.273 remains, LLC must restart the complete TL-SDU with original segmentation"
    );
    assert!(
        !find_tla_report(&full_retry_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "N.274 exhaustion alone must not fail the TL-SDU while N.273 remains"
    );
}

#[test]
fn test_outbound_bl_data_with_fcs_appends_32_bit_fcs() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac]);

    // EN 300 392-2 clauses 21.1.2.3 and 21.2.2.3 table 21.9:
    // BL-DATA with FCS carries TL-SDU followed by a 32-bit Frame Check Sequence.
    test.submit_message(build_tl_data_req_with_handle_and_fcs(addr, 101, true));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let data_msg = sink_msgs
        .iter()
        .find(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlDataFcs))
        .expect("expected BL-DATA with FCS");

    let mut pdu = tma_req_pdu(data_msg).expect("expected TMA-UNITDATA.req PDU");
    let header = BlData::from_bitbuf(&mut pdu).expect("expected BL-DATA header");
    assert!(header.has_fcs);
    assert!(fcs::check_fcs(&pdu));
    let payload_end = pdu.get_raw_end() - 32;
    assert_eq!(fcs_payload_bits_after_header(pdu, payload_end), "01010101");
}

#[test]
fn test_outbound_bl_adata_with_fcs_appends_32_bit_fcs() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac]);

    test.submit_message(build_bl_data_ind(addr, 1));
    // The inbound BL-DATA was received two timeslots before the current DL
    // time, so clause 22.3.2.3(d) piggybacking applies only to TS3 here.
    test.submit_message(build_tl_data_req_with_handle_fcs_timeslot(addr, 102, true, 3));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let data_msg = sink_msgs
        .iter()
        .find(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlAdataFcs))
        .expect("expected BL-ADATA with FCS");

    let mut pdu = tma_req_pdu(data_msg).expect("expected TMA-UNITDATA.req PDU");
    let header = BlAdata::from_bitbuf(&mut pdu).expect("expected BL-ADATA header");
    assert!(header.has_fcs);
    assert_eq!(header.nr, 1);
    assert_eq!(header.ns, 0);
    assert!(fcs::check_fcs(&pdu));
    let payload_end = pdu.get_raw_end() - 32;
    assert_eq!(fcs_payload_bits_after_header(pdu, payload_end), "01010101");
}

#[test]
fn test_outbound_bl_ack_response_with_fcs_appends_32_bit_fcs() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind(addr, 1));
    test.deliver_all_messages();
    let data_ind_msgs = test.dump_sinks();
    let (ind_handle, _) = data_ind_msgs
        .iter()
        .find_map(tl_data_ind_handle_and_payload_bits)
        .expect("incoming BL-DATA should produce TL-DATA.ind with retained handle");

    // EN 300 392-2 clause 21.2.2.1 table 21.5: the FCS BL-ACK variant is
    // valid when the acknowledgement carries TL-DATA response payload.
    test.submit_message(build_tl_data_resp_with_endpoint_handle_and_fcs(addr, 0, ind_handle, &[0xCC], true));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();
    let ack_msg = sink_msgs
        .iter()
        .find(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlAckFcs))
        .expect("expected BL-ACK response with FCS");

    let mut pdu = tma_req_pdu(ack_msg).expect("expected TMA-UNITDATA.req PDU");
    let header = BlAck::from_bitbuf(&mut pdu).expect("expected BL-ACK header");
    assert!(header.has_fcs);
    assert_eq!(header.nr, 1);
    assert!(fcs::check_fcs(&pdu));
    let payload_end = pdu.get_raw_end() - 32;
    assert_eq!(fcs_payload_bits_after_header(pdu, payload_end), "11001100");
}

#[test]
fn test_outbound_bl_udata_with_fcs_appends_32_bit_fcs() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac]);

    // EN 300 392-2 clause 21.2.2.4 table 21.11: BL-UDATA with FCS carries
    // the unacknowledged TL-SDU followed by a 32-bit Frame Check Sequence.
    test.submit_message(build_tl_unitdata_req_with_fcs(addr, &[0x77], true));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let data_msg = sink_msgs
        .iter()
        .find(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdataFcs))
        .expect("expected BL-UDATA with FCS");

    let mut pdu = tma_req_pdu(data_msg).expect("expected TMA-UNITDATA.req PDU");
    let header = BlUdata::from_bitbuf(&mut pdu).expect("expected BL-UDATA header");
    assert!(header.has_fcs);
    assert!(fcs::check_fcs(&pdu));
    let payload_end = pdu.get_raw_end() - 32;
    assert_eq!(fcs_payload_bits_after_header(pdu, payload_end), "01110111");
}

#[test]
fn test_outbound_bl_udata_preserves_stealing_parameters_to_umac() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac]);

    let mut req = build_tl_unitdata_req_with_repeats_handle(addr, &[0x55], 191, 0);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-UNITDATA request");
    };
    prim.stealing_permission = true;
    prim.stealing_repeats_flag = Some(true);
    prim.subscriber_class = 7;
    prim.data_class_info = Some(3);
    prim.pdu_prio = 6;

    // EN 300 392-2 tables 20.23 and 20.54 carry PDU priority plus layer-3
    // stealing parameters from TL-UNITDATA.req to TMA-UNITDATA.req.
    test.submit_message(req);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let data_msg = sink_msgs
        .iter()
        .find(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata))
        .expect("expected BL-UDATA TMA-UNITDATA.req");
    let SapMsgInner::TmaUnitdataReq(prim) = &data_msg.msg else {
        panic!("expected TMA-UNITDATA request");
    };

    assert!(prim.stealing_permission);
    assert_eq!(prim.stealing_repeats_flag, Some(true));
    assert_eq!(prim.subscriber_class, 7);
    assert_eq!(prim.data_category, Some(3));
    assert_eq!(prim.pdu_prio, 6);
}

#[test]
fn test_outbound_bl_data_preserves_stealing_parameters_to_umac() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 199;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.stealing_permission = true;
    prim.stealing_repeats_flag = Some(true);
    prim.subscriber_class = 7;
    prim.data_class_info = Some(3);
    prim.pdu_prio = 6;

    // EN 300 392-2 table 20.20 includes stealing permission and stealing
    // repeats on acknowledged TL-DATA.req, while clause 22.3.2.3 requires the
    // LLC to confirm the handle and put the TL-SDU into the transmission
    // buffer. These flags are scheduling hints for MAC, not a drop condition.
    test.submit_message(req);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        find_tla_report(&sink_msgs, req_handle, TLA_REPORT_NO_SPECIFIC_REPORT),
        "TL-DATA.req must emit immediate no-specific TL-REPORT even when stealing is permitted"
    );
    let data_msg = sink_msgs
        .iter()
        .find(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlData))
        .expect("expected BL-DATA TMA-UNITDATA.req");
    let SapMsgInner::TmaUnitdataReq(prim) = &data_msg.msg else {
        panic!("expected TMA-UNITDATA request");
    };

    assert_eq!(prim.req_handle, req_handle);
    assert!(prim.stealing_permission);
    assert_eq!(prim.stealing_repeats_flag, Some(true));
    assert_eq!(prim.subscriber_class, 7);
    assert_eq!(prim.data_category, Some(3));
    assert_eq!(prim.pdu_prio, 6);
    assert!(prim.tx_reporter.is_some());
}

#[test]
fn test_outbound_bl_data_preserves_pdu_priority_to_tma() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 198;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-DATA request");
    };
    prim.pdu_prio = 7;

    // EN 300 392-2 table 20.20 includes PDU priority on TL-DATA.req, and
    // table 20.54 makes it a TMA-UNITDATA.req parameter for MAC scheduling.
    test.submit_message(req);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let data = sink_msgs.iter().find_map(|msg| match &msg.msg {
        SapMsgInner::TmaUnitdataReq(prim)
            if prim.main_address == addr && prim.req_handle == req_handle && llc_pdu_type(msg) == Some(LlcPduType::BlData) =>
        {
            Some(prim)
        }
        _ => None,
    });
    let data = data.expect("expected BL-DATA TMA-UNITDATA.req");
    assert_eq!(data.pdu_prio, 7);
}

#[test]
fn test_bl_data_mac_ready_submits_highest_pdu_priority_first_without_reordering_ns() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let low_handle = 183;
    let high_handle = 184;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut low = build_tl_data_req_with_handle(addr, low_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut low.msg else {
        panic!("expected TL-DATA request");
    };
    prim.pdu_prio = 1;

    let mut high = build_tl_data_req_with_handle(addr, high_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut high.msg else {
        panic!("expected TL-DATA request");
    };
    prim.pdu_prio = 7;

    test.submit_message(low);
    test.submit_message(high);
    test.run_stack(Some(1));
    let mut first_msgs = test.dump_sinks();

    let first_data: Vec<_> = first_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmaUnitdataReq(prim) if llc_pdu_type(msg) == Some(LlcPduType::BlData) => {
                bl_data_ns(msg).map(|ns| (prim.req_handle, prim.pdu_prio, ns))
            }
            _ => None,
        })
        .collect();

    // EN 300 392-2 clauses 22.3.2.2 and 22.3.2.3(a/d): TL-DATA is stored in
    // PDU-priority order, and N(S) is the sequence number of the TL-SDU that
    // is actually selected at MAC-ready time.
    assert_eq!(first_data, vec![(high_handle, 7, 0)]);

    let reporter = take_first_tma_req_reporter(&mut first_msgs);
    reporter.mark_transmitted();
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_bl_ack_ind(addr, 0));
    test.run_stack(Some(1));
    let second_msgs = test.dump_sinks();
    let second_data: Vec<_> = second_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmaUnitdataReq(prim) if llc_pdu_type(msg) == Some(LlcPduType::BlData) => {
                bl_data_ns(msg).map(|ns| (prim.req_handle, prim.pdu_prio, ns))
            }
            _ => None,
        })
        .collect();

    assert_eq!(
        second_data,
        vec![(low_handle, 1, 1)],
        "lower-priority queued TL-DATA should follow with the next N(S)"
    );
}

#[test]
fn test_highest_priority_bl_data_cancels_untransmitted_lower_priority_tma() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let low_handle = 1831;
    let high_handle = 1832;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut low = build_tl_data_req_with_handle(addr, low_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut low.msg else {
        panic!("expected TL-DATA request");
    };
    prim.pdu_prio = 1;
    test.submit_message(low);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&initial_msgs, low_handle, TLA_REPORT_NO_SPECIFIC_REPORT),
        "initial lower-priority TL-DATA should still get the retained service handle"
    );
    let low_reporter = take_first_tma_req_reporter(&mut initial_msgs);
    assert_eq!(low_reporter.get_state(), TxState::Pending);

    let mut high = build_tl_data_req_with_handle(addr, high_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut high.msg else {
        panic!("expected TL-DATA request");
    };
    prim.pdu_prio = 7;

    // EN 300 392-2 clause 20.4.1.1.1 gives LLC TMA-CANCEL for a submitted
    // TMA-UNITDATA.req, table 20.54 carries the TMA priority, and clause
    // 22.3.2.3 keeps acknowledged TL-SDUs buffered. A highest-priority
    // TL-DATA may therefore cancel an untransmitted lower-priority TMA
    // request without dropping that lower-priority TL-SDU from LLC state.
    test.submit_message(high);
    test.run_stack(Some(1));
    let preempt_msgs = test.dump_sinks();

    assert_eq!(
        preempt_msgs.iter().filter_map(tma_cancel_req_handle).collect::<Vec<_>>(),
        vec![low_handle],
        "LLC must cancel the lower-priority TMA request before submitting the highest-priority one"
    );
    assert_eq!(
        preempt_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::TmaUnitdataReq(prim) if llc_pdu_type(msg) == Some(LlcPduType::BlData) => {
                    bl_data_ns(msg).map(|ns| (prim.req_handle, prim.pdu_prio, ns))
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![(high_handle, 7, 0)],
        "highest-priority BL-DATA should take the first outstanding N(S)"
    );
    assert!(
        preempt_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(prim)
            if prim.req_handle == low_handle)),
        "the cancelled lower-priority BL-DATA must not be resubmitted in the same MAC-ready turn"
    );

    test.submit_message(build_tma_report_ind(high_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_bl_ack_ind(addr, 0));
    test.run_stack(Some(1));
    let after_high_ack_msgs = test.dump_sinks();

    assert!(
        after_high_ack_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataConfBl(prim)
            if prim.main_address == addr
                && prim.req_handle == high_handle
                && prim.report == TLA_REPORT_SUCCESSFUL_TRANSFER)),
        "highest-priority transfer should complete normally after peer BL-ACK"
    );
    assert_eq!(
        after_high_ack_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::TmaUnitdataReq(prim) if prim.req_handle == low_handle && llc_pdu_type(msg) == Some(LlcPduType::BlData) => {
                    bl_data_ns(msg).map(|ns| (prim.req_handle, prim.pdu_prio, ns))
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![(low_handle, 1, 1)],
        "cancelled lower-priority TL-SDU should remain buffered and transmit next with the following N(S)"
    );
}

#[test]
fn test_highest_priority_bl_data_cancels_pending_bl_adata_and_preserves_nr() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let low_handle = 1835;
    let high_handle = 1836;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut low = build_tl_data_req_with_handle(addr, low_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut low.msg else {
        panic!("expected TL-DATA request");
    };
    prim.pdu_prio = 1;

    test.submit_message(build_bl_data_ind(addr, 1));
    test.submit_message(low);
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();

    assert_eq!(
        initial_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::TmaUnitdataReq(prim) if prim.req_handle == low_handle => {
                    bl_adata_nr_ns(msg).map(|(nr, ns)| (prim.req_handle, prim.pdu_prio, nr, ns))
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![(low_handle, 1, 1, 0)],
        "sanity check: lower-priority transfer starts as BL-ADATA carrying the pending N(R)"
    );

    let mut high = build_tl_data_req_with_handle(addr, high_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut high.msg else {
        panic!("expected TL-DATA request");
    };
    prim.pdu_prio = 7;

    // EN 300 392-2 clause 22.3.2.3(a)(v): when a lower-priority BL-ADATA is
    // cancelled before MAC transmission, LLC memorizes the ACK N(R). Priority
    // reordering may rewrite the DATA N(S), but the N(R) must remain embedded.
    test.submit_message(high);
    test.run_stack(Some(1));
    let preempt_msgs = test.dump_sinks();
    assert_eq!(
        preempt_msgs.iter().filter_map(tma_cancel_req_handle).collect::<Vec<_>>(),
        vec![low_handle]
    );
    assert!(
        preempt_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(prim)
            if prim.req_handle == low_handle)),
        "cancelled BL-ADATA must not be resubmitted in the same MAC-ready turn"
    );

    test.submit_message(build_tma_report_ind(high_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_bl_ack_ind(addr, 0));
    test.run_stack(Some(1));
    let after_high_ack_msgs = test.dump_sinks();

    assert_eq!(
        after_high_ack_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::TmaUnitdataReq(prim) if prim.req_handle == low_handle => {
                    bl_adata_nr_ns(msg).map(|(nr, ns)| (prim.req_handle, prim.pdu_prio, nr, ns))
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![(low_handle, 1, 1, 1)],
        "cancelled lower-priority BL-ADATA should preserve N(R)=1 while taking rewritten N(S)=1"
    );
    assert!(
        after_high_ack_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(prim)
            if prim.req_handle == low_handle && llc_pdu_type(msg) == Some(LlcPduType::BlData))),
        "memorized N(R) must not be dropped by rebuilding the cancelled BL-ADATA as BL-DATA"
    );
}

#[test]
fn test_non_high_priority_bl_data_does_not_cancel_submitted_tma() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let low_handle = 1833;
    let mid_handle = 1834;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut low = build_tl_data_req_with_handle(addr, low_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut low.msg else {
        panic!("expected TL-DATA request");
    };
    prim.pdu_prio = 1;
    test.submit_message(low);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let mut mid = build_tl_data_req_with_handle(addr, mid_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut mid.msg else {
        panic!("expected TL-DATA request");
    };
    prim.pdu_prio = 6;

    // EN 300 392-2 table 20.54 defines 7 as the highest TMA priority. Keep
    // ordinary higher-but-not-highest BL-DATA behind the submitted transfer so
    // simple private-call signalling is not reordered by a broad local policy.
    test.submit_message(mid);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        sink_msgs.iter().all(|msg| tma_cancel_req_handle(msg).is_none()),
        "non-highest priority must not issue TMA-CANCEL for an already submitted BL-DATA"
    );
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(prim)
            if prim.req_handle == mid_handle)),
        "non-highest priority BL-DATA should wait until the active basic-link transfer completes"
    );
}

#[test]
fn test_highest_priority_bl_data_cancels_submitted_lower_priority_bl_udata_same_basic_link() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let low_handle = 1837;
    let high_handle = 1838;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut low = build_tl_unitdata_req_with_repeats_handle(addr, &[0x11], low_handle, 0);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &mut low.msg else {
        panic!("expected TL-UNITDATA request");
    };
    prim.pdu_prio = 1;
    test.submit_message(low);
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();
    assert_eq!(
        initial_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::TmaUnitdataReq(prim) if llc_pdu_type(msg) == Some(LlcPduType::BlUdata) => {
                    Some((prim.req_handle, prim.pdu_prio))
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![(low_handle, 1)]
    );

    let mut high = build_tl_data_req_with_handle(addr, high_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut high.msg else {
        panic!("expected TL-DATA request");
    };
    prim.pdu_prio = 7;

    // EN 300 392-2 clause 22.3.2.3(a)(v) allows LLC to cancel lower-priority
    // TL-DATA or TL-UNITDATA already submitted to MAC when a highest-priority
    // TL-DATA is ready on the same basic link. Clause 20.4.1.1.1 supplies
    // TMA-CANCEL for the already submitted TMA-UNITDATA.req.
    test.submit_message(high);
    test.run_stack(Some(1));
    let preempt_msgs = test.dump_sinks();

    assert_eq!(
        preempt_msgs.iter().filter_map(tma_cancel_req_handle).collect::<Vec<_>>(),
        vec![low_handle],
        "LLC must cancel the lower-priority BL-UDATA TMA request"
    );
    assert_eq!(
        preempt_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::TmaUnitdataReq(prim) if llc_pdu_type(msg) == Some(LlcPduType::BlData) => {
                    bl_data_ns(msg).map(|ns| (prim.req_handle, prim.pdu_prio, ns))
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![(high_handle, 7, 0)],
        "highest-priority BL-DATA should be submitted after cancelling BL-UDATA"
    );
    assert!(
        preempt_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(prim)
            if prim.req_handle == low_handle)),
        "cancelled BL-UDATA must not be re-submitted in the same MAC-ready turn"
    );

    test.submit_message(build_tma_report_ind(high_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let after_high_mac_report_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&after_high_mac_report_msgs, high_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION),
        "highest-priority BL-DATA should still report first complete transmission"
    );
    assert_eq!(
        after_high_mac_report_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::TmaUnitdataReq(prim) if llc_pdu_type(msg) == Some(LlcPduType::BlUdata) => {
                    Some((prim.req_handle, prim.pdu_prio))
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![(low_handle, 1)],
        "cancelled BL-UDATA should remain buffered and become MAC-ready after the cancelled turn"
    );
}

#[test]
fn test_emergency_cancelled_bl_udata_with_tx_reporter_resubmits_with_fresh_pending_reporter() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let low_handle = 1841;
    let high_handle = 1842;
    let service_reporter = TxReporter::new_unacked();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut low = build_tl_unitdata_req_with_repeats_handle(addr, &[0x11], low_handle, 0);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &mut low.msg else {
        panic!("expected TL-UNITDATA request");
    };
    prim.pdu_prio = 1;
    prim.tx_reporter = Some(service_reporter.clone());
    test.submit_message(low);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let first_mac_reporter = take_first_tma_req_reporter(&mut initial_msgs);
    assert_eq!(service_reporter.get_state(), TxState::Pending);
    assert_eq!(first_mac_reporter.get_state(), TxState::Pending);
    assert!(!first_mac_reporter.shares_state_with(&service_reporter));

    let mut high = build_tl_data_req_with_handle(addr, high_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut high.msg else {
        panic!("expected TL-DATA request");
    };
    prim.pdu_prio = 7;

    test.submit_message(high);
    test.run_stack(Some(1));
    let preempt_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 20.4.1.1.1 and 22.3.2.3(a)(v): the TMA-CANCEL
    // aborts the submitted lower-priority MAC request, but the stored
    // BL-UDATA TL-SDU and its service reporter stay pending for resubmission.
    assert_eq!(
        preempt_msgs.iter().filter_map(tma_cancel_req_handle).collect::<Vec<_>>(),
        vec![low_handle]
    );
    assert_eq!(first_mac_reporter.get_state(), TxState::Discarded);
    assert_eq!(service_reporter.get_state(), TxState::Pending);

    test.submit_message(build_tma_report_ind(high_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let mut after_high_mac_report_msgs = test.dump_sinks();
    assert!(find_tla_report(
        &after_high_mac_report_msgs,
        high_handle,
        TLA_REPORT_FIRST_COMPLETE_TRANSMISSION
    ));
    let retry_mac_reporter = take_first_tma_req_reporter(&mut after_high_mac_report_msgs);
    assert_eq!(retry_mac_reporter.get_state(), TxState::Pending);
    assert!(!retry_mac_reporter.shares_state_with(&first_mac_reporter));
    assert!(!retry_mac_reporter.shares_state_with(&service_reporter));
    assert_eq!(service_reporter.get_state(), TxState::Pending);

    test.submit_message(build_tma_report_ind(low_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let final_msgs = test.dump_sinks();
    assert!(find_tla_report(&final_msgs, low_handle, TLA_REPORT_SUCCESSFUL_TRANSFER));
    assert_eq!(retry_mac_reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Transmitted);
}

#[test]
fn test_highest_priority_bl_data_does_not_cancel_bl_udata_on_different_basic_link() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let low_handle = 1839;
    let high_handle = 1840;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut low = build_tl_unitdata_req_with_repeats_handle(addr, &[0x11], low_handle, 0);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &mut low.msg else {
        panic!("expected TL-UNITDATA request");
    };
    prim.pdu_prio = 1;
    test.submit_message(low);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let mut high = build_tl_data_req_with_endpoint_handle(addr, 1, high_handle);
    let SapMsgInner::TlaTlDataReqBl(prim) = &mut high.msg else {
        panic!("expected TL-DATA request");
    };
    prim.pdu_prio = 7;

    // EN 300 392-2 clause 22.3.2.3 scopes the sending buffer and cancellation
    // decision to the basic link. A distinct endpoint models a distinct MAC
    // resource and must not cancel the already submitted BL-UDATA.
    test.submit_message(high);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        sink_msgs.iter().all(|msg| tma_cancel_req_handle(msg).is_none()),
        "highest priority on a different basic link must not issue TMA-CANCEL for BL-UDATA"
    );
    assert_eq!(
        sink_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::TmaUnitdataReq(prim) if prim.endpoint_id == 1 && llc_pdu_type(msg) == Some(LlcPduType::BlData) => {
                    bl_data_ns(msg).map(|ns| (prim.req_handle, prim.pdu_prio, ns))
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![(high_handle, 7, 0)]
    );
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(prim)
            if prim.req_handle == low_handle)),
        "already submitted BL-UDATA should not be duplicated while a different basic link sends priority 7"
    );
}

#[test]
fn test_bl_udata_mac_ready_submits_highest_pdu_priority_first() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let low_handle = 181;
    let high_handle = 182;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut low = build_tl_unitdata_req_with_repeats_handle(addr, &[0x11], low_handle, 0);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &mut low.msg else {
        panic!("expected TL-UNITDATA request");
    };
    prim.pdu_prio = 1;

    let mut high = build_tl_unitdata_req_with_repeats_handle(addr, &[0x77], high_handle, 0);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &mut high.msg else {
        panic!("expected TL-UNITDATA request");
    };
    prim.pdu_prio = 7;

    test.submit_message(low);
    test.submit_message(high);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let submitted: Vec<_> = sink_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmaUnitdataReq(prim) if llc_pdu_type(msg) == Some(LlcPduType::BlUdata) => Some((prim.req_handle, prim.pdu_prio)),
            _ => None,
        })
        .collect();

    // EN 300 392-2 clauses 22.3.1.7.2 and 22.3.2.4.1: on MAC-ready,
    // unacknowledged basic-link data is selected by highest PDU priority.
    assert_eq!(submitted, vec![(high_handle, 7), (low_handle, 1)]);
}

#[test]
fn test_bl_udata_mac_ready_keeps_fifo_order_for_equal_pdu_priority() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let first_handle = 177;
    let second_handle = 178;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut first = build_tl_unitdata_req_with_repeats_handle(addr, &[0x11], first_handle, 0);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &mut first.msg else {
        panic!("expected TL-UNITDATA request");
    };
    prim.pdu_prio = 4;

    let mut second = build_tl_unitdata_req_with_repeats_handle(addr, &[0x22], second_handle, 0);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &mut second.msg else {
        panic!("expected TL-UNITDATA request");
    };
    prim.pdu_prio = 4;

    test.submit_message(first);
    test.submit_message(second);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let submitted_handles: Vec<_> = sink_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmaUnitdataReq(prim) if llc_pdu_type(msg) == Some(LlcPduType::BlUdata) => Some(prim.req_handle),
            _ => None,
        })
        .collect();

    assert_eq!(submitted_handles, vec![first_handle, second_handle]);
}

#[test]
fn test_bl_udata_reserved_success_reports_completed_after_n253_plus_one_reports() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 190;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    // EN 300 392-2 clause 22.3.2.4.1(e): reserved/stealing BL-UDATA
    // transfer completes only after N.253 + 1 complete transmissions.
    test.submit_message(build_tl_unitdata_req_with_repeats_handle(addr, &[0x55], req_handle, 1));
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();
    assert_eq!(
        initial_msgs
            .iter()
            .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata))
            .count(),
        1
    );

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let retry_msgs = test.dump_sinks();
    assert!(
        !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
        "first complete transmission is not enough when N.253=1"
    );
    assert_eq!(
        retry_msgs
            .iter()
            .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata))
            .count(),
        1,
        "LLC should re-submit BL-UDATA for the second complete transmission"
    );

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let final_msgs = test.dump_sinks();
    assert!(find_tla_report(&final_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER));
    assert!(
        !final_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata)),
        "no BL-UDATA retry should remain after N.253+1 complete transmissions"
    );
}

#[test]
fn test_bl_udata_with_tx_reporter_reserved_repeats_use_fresh_mac_reporters() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 191;
    let service_reporter = TxReporter::new_unacked();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_unitdata_req_with_repeats_handle(addr, &[0x55], req_handle, 1);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-UNITDATA request");
    };
    prim.tx_reporter = Some(service_reporter.clone());

    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let first_mac_reporter = take_first_tma_req_reporter(&mut initial_msgs);
    assert_eq!(service_reporter.get_state(), TxState::Pending);
    assert_eq!(first_mac_reporter.get_state(), TxState::Pending);
    assert!(!first_mac_reporter.shares_state_with(&service_reporter));

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let mut retry_msgs = test.dump_sinks();
    assert!(
        !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
        "service-level reporter must not complete after the first of N.253+1 reserved transmissions"
    );
    assert_eq!(first_mac_reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Pending);

    let second_mac_reporter = take_first_tma_req_reporter(&mut retry_msgs);
    assert_eq!(second_mac_reporter.get_state(), TxState::Pending);
    assert!(!second_mac_reporter.shares_state_with(&first_mac_reporter));
    assert!(!second_mac_reporter.shares_state_with(&service_reporter));

    // EN 300 392-2 clauses 20.4.1.1.3 and 22.3.2.4.1: each MAC request
    // reports its own progress, but the BL-UDATA service transfer completes
    // only after all N.253 + 1 reserved/stealing transmissions are complete.
    test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let final_msgs = test.dump_sinks();
    assert!(find_tla_report(&final_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER));
    assert_eq!(second_mac_reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Transmitted);
}

#[test]
fn test_bl_udata_with_tx_reporter_random_access_success_completes_immediately() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 192;
    let service_reporter = TxReporter::new_unacked();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_unitdata_req_with_repeats_handle(addr, &[0x55], req_handle, 5);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-UNITDATA request");
    };
    prim.tx_reporter = Some(service_reporter.clone());

    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let mac_reporter = take_first_tma_req_reporter(&mut initial_msgs);
    assert_eq!(mac_reporter.get_state(), TxState::Pending);
    assert_eq!(service_reporter.get_state(), TxState::Pending);
    assert!(!mac_reporter.shares_state_with(&service_reporter));

    // EN 300 392-2 clauses 20.4.1.1.3 and 22.3.2.4.1(d): successful random
    // access completes unacknowledged basic-link transfer immediately, without
    // waiting for N.253 reserved/stealing repeats.
    test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessRandomAccess));
    test.run_stack(Some(1));
    let final_msgs = test.dump_sinks();

    assert!(find_tla_report(&final_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER));
    assert_eq!(mac_reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Transmitted);
    assert!(
        !final_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata)),
        "random-access success must remove BL-UDATA from the sending buffer"
    );
}

#[test]
fn test_bl_udata_with_tx_reporter_random_access_failure_discards_service_reporter() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 193;
    let service_reporter = TxReporter::new_unacked();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_unitdata_req_with_repeats_handle(addr, &[0x55], req_handle, 5);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-UNITDATA request");
    };
    prim.tx_reporter = Some(service_reporter.clone());

    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let mac_reporter = take_first_tma_req_reporter(&mut initial_msgs);
    assert_eq!(mac_reporter.get_state(), TxState::Pending);
    assert_eq!(service_reporter.get_state(), TxState::Pending);

    // EN 300 392-2 clauses 20.4.1.1.3 and 22.3.2.4.1(g): random-access
    // failure removes the TL-SDU and reports failed transfer to the service
    // user, so the service-level reporter must be terminally discarded.
    test.submit_message(build_tma_report_ind(req_handle, TmaReport::RandomAccessFailure));
    test.run_stack(Some(1));
    let final_msgs = test.dump_sinks();

    assert!(find_tla_report(&final_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER));
    assert_eq!(mac_reporter.get_state(), TxState::Discarded);
    assert_eq!(service_reporter.get_state(), TxState::Discarded);
    assert!(
        !final_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata)),
        "random-access failure must remove BL-UDATA from the sending buffer"
    );
}

#[test]
fn test_bl_udata_with_tx_reporter_fragmentation_failure_exhaustion_discards_service_reporter() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 194;
    let service_reporter = TxReporter::new_unacked();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_unitdata_req_with_repeats_handle(addr, &[0x55], req_handle, 1);
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &mut req.msg else {
        panic!("expected TL-UNITDATA request");
    };
    prim.tx_reporter = Some(service_reporter.clone());

    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let first_mac_reporter = take_first_tma_req_reporter(&mut initial_msgs);
    assert_eq!(first_mac_reporter.get_state(), TxState::Pending);
    assert_eq!(service_reporter.get_state(), TxState::Pending);

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::FragmentationFailure));
    test.run_stack(Some(1));
    let mut retry_msgs = test.dump_sinks();
    assert!(
        !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "first fragmentation failure must retry while N.253 failure allowance remains"
    );
    assert_eq!(first_mac_reporter.get_state(), TxState::Discarded);
    assert_eq!(service_reporter.get_state(), TxState::Pending);
    let retry_mac_reporter = take_first_tma_req_reporter(&mut retry_msgs);
    assert_eq!(retry_mac_reporter.get_state(), TxState::Pending);
    assert!(!retry_mac_reporter.shares_state_with(&first_mac_reporter));
    assert!(!retry_mac_reporter.shares_state_with(&service_reporter));

    // EN 300 392-2 clauses 20.4.1.1.3 and 22.3.2.4.1(f): after the allowed
    // fragmentation failures are exhausted before N.253 + 1 complete
    // transmissions, LLC removes BL-UDATA and reports failed transfer.
    test.submit_message(build_tma_report_ind(req_handle, TmaReport::FragmentationFailure));
    test.run_stack(Some(1));
    let final_msgs = test.dump_sinks();

    assert!(find_tla_report(&final_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER));
    assert_eq!(retry_mac_reporter.get_state(), TxState::Discarded);
    assert_eq!(service_reporter.get_state(), TxState::Discarded);
    assert!(
        !final_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata)),
        "exhausted fragmentation failures must not schedule another BL-UDATA retry"
    );
}

#[test]
fn test_bl_udata_without_indicated_n253_uses_designer_default() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 189;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    // EN 300 392-2 clause 22.3.2.4.1 note 1 and Annex A.2: when the
    // service user does not indicate N.253, the MS designer value applies.
    // This repo's Annex A.2 designer value is 3, so BL-UDATA completes after
    // four complete reserved/stealing transmissions.
    test.submit_message(build_tl_unitdata_req_without_repeats_handle(addr, &[0x55], req_handle));
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();
    assert_eq!(
        initial_msgs
            .iter()
            .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata))
            .count(),
        1
    );

    for complete in 1..=3 {
        test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessReservedOrStealing));
        test.run_stack(Some(1));
        let retry_msgs = test.dump_sinks();
        assert!(
            !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
            "complete transmission {complete} must not finish designer-default N.253=3"
        );
        assert_eq!(
            retry_msgs
                .iter()
                .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata))
                .count(),
            1,
            "complete transmission {complete} should schedule the next BL-UDATA repeat"
        );
    }

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let final_msgs = test.dump_sinks();
    assert!(find_tla_report(&final_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER));
    assert!(
        !final_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata)),
        "designer-default N.253=3 should not schedule a fifth complete transmission"
    );
}

#[test]
fn test_bl_udata_random_access_failure_reports_failed_and_drops_pending() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 191;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_tl_unitdata_req_with_repeats_handle(addr, &[0x55], req_handle, 1));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 clause 22.3.2.4.1(g): random-access failure removes the
    // TL-SDU and reports failed transfer.
    test.submit_message(build_tma_report_ind(req_handle, TmaReport::RandomAccessFailure));
    test.run_stack(Some(1));
    let failed_msgs = test.dump_sinks();
    assert!(find_tla_report(&failed_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER));

    test.run_stack(Some(1));
    assert!(
        !test.dump_sinks().iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata)),
        "random-access failure must drop the pending BL-UDATA"
    );
}

#[test]
fn test_bl_udata_fragmentation_failure_n253_zero_allows_one_retry_then_fails() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 192;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_tl_unitdata_req_with_repeats_handle(addr, &[0x55], req_handle, 0));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 clause 22.3.2.4.1(f): when N.253=0, retry until at most
    // two failed transmissions or one complete transmission.
    test.submit_message(build_tma_report_ind(req_handle, TmaReport::FragmentationFailure));
    test.run_stack(Some(1));
    let retry_msgs = test.dump_sinks();
    assert!(
        !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "first fragmentation failure with N.253=0 should retry"
    );
    assert_eq!(
        retry_msgs
            .iter()
            .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata))
            .count(),
        1
    );

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::FragmentationFailure));
    test.run_stack(Some(1));
    let failed_msgs = test.dump_sinks();
    assert!(find_tla_report(&failed_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER));
    assert!(
        !failed_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata)),
        "second failed transmission with N.253=0 should terminate the BL-UDATA transfer"
    );
}

#[test]
fn test_bl_udata_service_user_n253_above_annex_range_is_clamped_to_five() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 193;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    // EN 300 392-2 22.3.2.4.1 note 1 lets the service user indicate
    // N.253, while Annex A.2 bounds N.253 to range 0..=5. Values above the
    // ETSI range are clamped to five, so completion requires six complete
    // reserved/stealing transmissions.
    test.submit_message(build_tl_unitdata_req_with_repeats_handle(addr, &[0x55], req_handle, u8::MAX));
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();
    assert_eq!(
        initial_msgs
            .iter()
            .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata))
            .count(),
        1
    );

    for complete in 1..=5 {
        test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessReservedOrStealing));
        test.run_stack(Some(1));
        let retry_msgs = test.dump_sinks();
        assert!(
            !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
            "complete transmission {complete} must not finish clamped N.253=5"
        );
        assert_eq!(
            retry_msgs
                .iter()
                .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata))
                .count(),
            1,
            "complete transmission {complete} should schedule the next BL-UDATA repeat"
        );
    }

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let final_msgs = test.dump_sinks();
    assert!(find_tla_report(&final_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER));
    assert!(
        !final_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata)),
        "clamped N.253=5 should not schedule a seventh complete transmission"
    );
}

#[test]
fn test_bl_data_tma_report_with_duplicate_req_handle_is_ignored_as_ambiguous() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let duplicate_req_handle = 194;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    // EN 300 392-2 22.3.2.3(e/f) ties a TMA-REPORT to the handle for one
    // MAC request. If the local service user reuses a handle across two
    // submitted BL-DATA requests, LLC cannot safely infer which PDU completed.
    test.submit_message(build_tl_data_req_with_endpoint_handle(addr, 1, duplicate_req_handle));
    test.submit_message(build_tl_data_req_with_endpoint_handle(addr, 2, duplicate_req_handle));
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    assert_eq!(
        initial_msgs
            .iter()
            .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlData))
            .count(),
        2
    );
    let reporter_ep1 = take_tma_req_reporter_for_endpoint(&mut initial_msgs, 1);
    let reporter_ep2 = take_tma_req_reporter_for_endpoint(&mut initial_msgs, 2);

    test.submit_message(build_tma_report_ind(duplicate_req_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let report_msgs = test.dump_sinks();

    assert!(
        !find_tla_report(&report_msgs, duplicate_req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION),
        "ambiguous duplicate-handle TMA report must not complete either BL-DATA request"
    );
    assert_eq!(reporter_ep1.get_state(), TxState::Pending);
    assert_eq!(reporter_ep2.get_state(), TxState::Pending);
}

#[test]
fn test_bl_udata_tma_report_with_duplicate_req_handle_is_ignored_as_ambiguous() {
    debug::setup_logging_verbose();

    let first_addr = TetraAddress::new(2065022, SsiType::Issi);
    let second_addr = TetraAddress::new(2065023, SsiType::Issi);
    let duplicate_req_handle = 195;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    // EN 300 392-2 22.3.2.4.1(c/e) uses TMA-REPORT to count complete
    // BL-UDATA transmissions for one TL-SDU. A duplicate local handle across
    // two queued TL-SDUs is ambiguous and must not advance either counter.
    test.submit_message(build_tl_unitdata_req_with_repeats_handle(
        first_addr,
        &[0x55],
        duplicate_req_handle,
        1,
    ));
    test.submit_message(build_tl_unitdata_req_with_repeats_handle(
        second_addr,
        &[0xAA],
        duplicate_req_handle,
        1,
    ));
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();
    assert_eq!(
        initial_msgs
            .iter()
            .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata))
            .count(),
        2
    );

    test.submit_message(build_tma_report_ind(duplicate_req_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let report_msgs = test.dump_sinks();

    assert!(
        !find_tla_report(&report_msgs, duplicate_req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
        "ambiguous duplicate-handle TMA report must not complete either BL-UDATA request"
    );
    assert!(
        !report_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata)),
        "ambiguous report must not make either already-submitted BL-UDATA look ready for the next repeat"
    );
}

#[test]
fn test_cross_service_tma_report_with_duplicate_req_handle_is_ignored_as_ambiguous() {
    debug::setup_logging_verbose();

    let data_addr = TetraAddress::new(2065022, SsiType::Issi);
    let udata_addr = TetraAddress::new(2065023, SsiType::Issi);
    let duplicate_req_handle = 196;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    // EN 300 392-2 clauses 22.3.2.3(e/f) and 22.3.2.4.1(c/e) apply
    // TMA-REPORT side effects to different LLC services. Since TMA-REPORT.ind
    // carries only req_handle, a handle collision across submitted BL-DATA and
    // BL-UDATA cannot be resolved safely.
    test.submit_message(build_tl_data_req_with_endpoint_handle(data_addr, 1, duplicate_req_handle));
    test.submit_message(build_tl_unitdata_req_with_repeats_handle(
        udata_addr,
        &[0x55],
        duplicate_req_handle,
        1,
    ));
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    assert_eq!(
        initial_msgs
            .iter()
            .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlData))
            .count(),
        1
    );
    assert_eq!(
        initial_msgs
            .iter()
            .filter(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata))
            .count(),
        1
    );
    let data_reporter = take_tma_req_reporter_for_endpoint(&mut initial_msgs, 1);

    test.submit_message(build_tma_report_ind(duplicate_req_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let report_msgs = test.dump_sinks();

    assert!(
        !find_tla_report(&report_msgs, duplicate_req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION),
        "cross-service ambiguous report must not complete BL-DATA"
    );
    assert!(
        !find_tla_report(&report_msgs, duplicate_req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER),
        "cross-service ambiguous report must not complete BL-UDATA"
    );
    assert_eq!(data_reporter.get_state(), TxState::Pending);
    assert!(
        !report_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlUdata)),
        "ambiguous report must not make submitted BL-UDATA ready for the next repeat"
    );
}

#[test]
fn test_inbound_basic_link_fcs_is_validated_and_stripped_before_delivery() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind_with_payload_and_fcs(addr, 0, &[0xA5], true));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let data_ind = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataIndBl(prim) if prim.main_address == addr => Some(prim),
            _ => None,
        })
        .expect("expected TL-DATA.ind from valid BL-DATA with FCS");

    assert!(data_ind.fcs_flag);
    let payload = data_ind.tl_sdu.as_ref().expect("expected delivered TL-SDU");
    assert_eq!(payload.to_bitstr(), "10100101");
}

#[test]
fn test_inbound_bl_ack_fcs_response_payload_is_stripped_before_confirm() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_tl_data_req(addr));
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    reporter.mark_transmitted();
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_bl_ack_ind_with_payload_and_fcs(addr, 0, &[0xA5], true));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let data_conf = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataConfBl(prim) if prim.main_address == addr => Some(prim),
            _ => None,
        })
        .expect("expected TL-DATA.conf from valid BL-ACK with FCS");

    assert!(data_conf.fcs_flag);
    let payload = data_conf.tl_sdu.as_ref().expect("expected response TL-SDU");
    assert_eq!(payload.to_bitstr(), "10100101");
}

#[test]
fn test_bl_ack_fcs_failure_still_acknowledges_downlink_but_drops_response_payload() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 183;
    let service_reporter = TxReporter::new();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    reporter.mark_transmitted();
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 clause 22.3.2.3(j): BL-ACK acknowledgement N(R) is
    // independent from the optional response TL-SDU, whose delivery depends on
    // the FCS. A corrupt response payload must not suppress the valid ACK.
    test.submit_message(corrupt_last_bit(build_bl_ack_ind_with_payload_and_fcs(addr, 0, &[0xA5], true)));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let conf = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataConfBl(prim) if prim.main_address == addr => Some(prim),
            _ => None,
        })
        .expect("BL-ACK with valid N(R) should still confirm the downlink transfer");
    assert_eq!(conf.req_handle, req_handle);
    assert_eq!(conf.report, TLA_REPORT_SUCCESSFUL_TRANSFER);
    assert!(conf.tl_sdu.is_none(), "bad FCS response payload must be dropped");
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Acknowledged);
    assert!(
        !sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "bad FCS response payload must not be delivered as TL-DATA.ind"
    );
}

#[test]
fn test_bl_adata_fcs_failure_processes_nr_but_does_not_ack_or_deliver_bad_ns() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 184;
    let service_reporter = TxReporter::new();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle_timeslot(addr, req_handle, 3);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    reporter.mark_transmitted();
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 clause 22.3.2.3(l): BL-ADATA is handled as BL-ACK first,
    // then BL-DATA. A bad optional FCS on the contained TL-SDU must not block
    // N(R), but it must suppress delivery and ACKing of the corrupt N(S).
    test.submit_message(corrupt_last_bit(build_bl_adata_ind_with_payload_and_fcs(addr, 0, 1, &[0xA5], true)));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let conf = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataConfBl(prim) if prim.main_address == addr => Some(prim),
            _ => None,
        })
        .expect("BL-ADATA N(R) should still confirm the downlink transfer");
    assert_eq!(conf.req_handle, req_handle);
    assert_eq!(conf.report, TLA_REPORT_SUCCESSFUL_TRANSFER);
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Acknowledged);
    assert!(
        !sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "bad FCS BL-ADATA payload must not be delivered"
    );
    assert!(
        !sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(prim)
            if prim.main_address == addr && llc_pdu_type(msg) == Some(LlcPduType::BlAck))),
        "bad FCS BL-ADATA N(S) must not be acknowledged"
    );
}

#[test]
fn test_pending_ack_does_not_piggyback_across_ssi_types() {
    debug::setup_logging_verbose();

    let numeric_ssi = 2065022;
    let issi_addr = TetraAddress::new(numeric_ssi, SsiType::Issi);
    let gssi_addr = TetraAddress::new(numeric_ssi, SsiType::Gssi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac]);

    // EN 300 392-2 clause 22.3.2.3 combines ACKs only on the same basic link.
    // A GSSI ACK must not be consumed by an ISSI BL-DATA with the same numeric SSI.
    test.submit_message(build_bl_data_ind(gssi_addr, 1));
    test.submit_message(build_tl_data_req(issi_addr));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let saw_issi_bl_data = sink_msgs.iter().any(|msg| {
        matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(prim)
            if prim.main_address == issi_addr && llc_pdu_type(msg) == Some(LlcPduType::BlData))
    });
    let saw_issi_bl_adata = sink_msgs.iter().any(|msg| {
        matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(prim)
            if prim.main_address == issi_addr && llc_pdu_type(msg) == Some(LlcPduType::BlAdata))
    });
    let saw_gssi_bl_ack = sink_msgs.iter().any(|msg| {
        matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(prim)
            if prim.main_address == gssi_addr && llc_pdu_type(msg) == Some(LlcPduType::BlAck))
    });

    assert!(saw_issi_bl_data, "ISSI downlink should remain BL-DATA");
    assert!(!saw_issi_bl_adata, "ISSI downlink must not consume the GSSI pending ACK");
    assert!(
        saw_gssi_bl_ack,
        "GSSI ACK should remain separate and be sent as standalone BL-ACK at MAC-ready time"
    );
}

#[test]
fn test_pending_ack_piggybacks_by_endpoint_not_receive_timeslot() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 4, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    // The inbound BL-DATA is treated as received on UL timeslot 2
    // (current DL time minus two slots). EN 300 392-2 clause 22.3.1.1 makes
    // endpoint_id the local MAC-resource/basic-link identifier, so a later
    // TL-DATA.req on the same endpoint may consume that waiting ACK even if a
    // local helper supplies a different channel-allocation timeslot.
    test.submit_message(build_bl_data_ind(addr, 1));
    test.submit_message(build_tl_data_req_with_timeslot(addr, 3));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        !sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(prim)
            if prim.main_address == addr && llc_pdu_type(msg) == Some(LlcPduType::BlData))),
        "same-endpoint downlink should not stay BL-DATA when a waiting ACK can form BL-ADATA"
    );
    assert!(
        sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(prim)
            if prim.main_address == addr && llc_pdu_type(msg) == Some(LlcPduType::BlAdata))),
        "same-endpoint pending ACK should be piggybacked as BL-ADATA"
    );
    assert!(
        !sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(prim)
            if prim.main_address == addr && llc_pdu_type(msg) == Some(LlcPduType::BlAck))),
        "same-endpoint pending ACK should be consumed by BL-ADATA, not sent separately"
    );
}

#[test]
fn test_pending_ack_piggybacks_when_same_endpoint_transfer_fits() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 4, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind(addr, 1));
    test.submit_message(build_tl_data_req_with_timeslot(addr, 2));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let nr_ns: Vec<(u8, u8)> = sink_msgs.iter().filter_map(bl_adata_nr_ns).collect();
    assert_eq!(
        nr_ns,
        vec![(1, 0)],
        "same-endpoint downlink should combine pending ACK and TL-DATA as BL-ADATA"
    );
    assert!(
        !sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(prim)
            if prim.main_address == addr && llc_pdu_type(msg) == Some(LlcPduType::BlAck))),
        "same-endpoint pending ACK should be consumed by BL-ADATA, not sent twice"
    );
}

#[test]
fn test_pending_ack_falls_back_to_bl_ack_when_bl_adata_exceeds_single_mac_resource_capacity() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 212;
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 4, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    // The inbound BL-DATA is received on UL timeslot 2. With the same-slot
    // channel allocation, a 25-octet TL-SDU would make BL-ADATA exceed the
    // fresh SCH/F MAC-RESOURCE TM-SDU capacity. EN 300 392-2 clause
    // 22.3.2.3(d) requires a standalone BL-ACK and separate BL-DATA instead
    // of handing oversized BL-ADATA to MAC fragmentation.
    test.submit_message(build_bl_data_ind(addr, 1));
    let mut req = build_tl_data_req_with_payload_handle_timeslot(addr, &[0xA5; 25], req_handle, 2);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut sink_msgs = test.dump_sinks();

    let ack_payloads: Vec<(i32, u8, String)> = sink_msgs.iter().filter_map(bl_ack_prio_nr_and_payload_bits).collect();
    assert_eq!(
        ack_payloads,
        vec![(5, 1, String::new())],
        "oversized BL-ADATA should emit only a standalone BL-ACK with ETSI BL-ACK PDU priority 5"
    );
    assert!(
        sink_msgs.iter().all(|msg| llc_pdu_type(msg) != Some(LlcPduType::BlAdata)),
        "oversized same-slot transfer must not be submitted as BL-ADATA"
    );

    let bl_data_ns_values: Vec<u8> = sink_msgs.iter().filter_map(bl_data_ns).collect();
    assert_eq!(bl_data_ns_values, vec![0], "TL-SDU should be sent as tracked BL-DATA with N(S)=0");
    let reporter = take_first_tma_req_reporter(&mut sink_msgs);

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let report_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&report_msgs, req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION),
        "fallback BL-DATA should start the normal acknowledged-transfer report path"
    );

    test.submit_message(build_bl_ack_ind(addr, 0));
    test.run_stack(Some(1));
    let conf_msgs = test.dump_sinks();
    assert!(
        conf_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataConfBl(prim)
            if prim.main_address == addr
                && prim.req_handle == req_handle
                && prim.report == TLA_REPORT_SUCCESSFUL_TRANSFER)),
        "fallback BL-DATA must remain in acknowledged-transfer state and confirm on peer BL-ACK"
    );
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Acknowledged);
}

#[test]
fn test_pending_ack_is_not_consumed_by_bl_adata_when_same_link_transfer_is_blocked() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 4, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle_timeslot(addr, 301, 2);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    assert_eq!(
        initial_msgs.iter().filter_map(bl_data_ns).collect::<Vec<_>>(),
        vec![0],
        "first TL-DATA.req should submit the outstanding BL-DATA"
    );
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    test.submit_message(build_tma_report_ind(301, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // Same basic-link transfer 301 is still awaiting peer BL-ACK. A new
    // inbound BL-DATA must not lose its acknowledgement by being baked into a
    // queued BL-ADATA for transfer 302, because that queued transfer is not
    // MAC-ready while the previous BL-DATA blocks the link.
    test.submit_message(build_bl_data_ind(addr, 1));
    test.submit_message(build_tl_data_req_with_handle_timeslot(addr, 302, 2));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let ack_payloads: Vec<(u8, String)> = sink_msgs.iter().filter_map(bl_ack_nr_and_payload_bits).collect();
    assert_eq!(
        ack_payloads,
        vec![(1, String::new())],
        "blocked same-link TL-DATA is not MAC-ready, so the waiting ACK is sent as standalone BL-ACK"
    );
    assert!(
        sink_msgs.iter().all(|msg| llc_pdu_type(msg) != Some(LlcPduType::BlAdata)),
        "blocked same-link TL-DATA must not consume the pending ACK into BL-ADATA"
    );
    assert!(
        sink_msgs.iter().all(|msg| bl_data_ns(msg).is_none()),
        "second TL-DATA should remain queued until the first transfer is acknowledged"
    );

    test.submit_message(build_bl_ack_ind(addr, 0));
    test.run_stack(Some(1));
    let after_first_ack_msgs = test.dump_sinks();
    assert!(
        after_first_ack_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataConfBl(prim)
            if prim.main_address == addr
                && prim.req_handle == 301
                && prim.report == TLA_REPORT_SUCCESSFUL_TRANSFER)),
        "first transfer should confirm when its peer BL-ACK arrives"
    );
    assert_eq!(
        after_first_ack_msgs.iter().filter_map(bl_adata_nr_ns).collect::<Vec<_>>(),
        Vec::<(u8, u8)>::new(),
        "the waiting ACK was already sent before the queued second TL-DATA became MAC-ready"
    );
    assert!(
        after_first_ack_msgs.iter().any(|msg| bl_data_ns(msg) == Some(1)),
        "queued second TL-DATA should be sent as BL-DATA when no same-link ACK remains waiting"
    );
    assert!(
        after_first_ack_msgs.iter().all(|msg| llc_pdu_type(msg) != Some(LlcPduType::BlAck)),
        "already-sent ACK must not be emitted a second time"
    );
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Acknowledged);
}

#[test]
fn test_tl_data_req_and_first_complete_emit_tl_reports() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 91;
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_tl_data_req_with_handle(addr, req_handle));
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&initial_msgs, req_handle, TLA_REPORT_NO_SPECIFIC_REPORT),
        "EN 300 392-2 22.3.2.3(a) requires immediate TL-REPORT handle confirmation"
    );
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    assert_eq!(reporter.get_state(), TxState::Pending);

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    let report_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&report_msgs, req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION),
        "EN 300 392-2 22.3.2.3(f) requires first-complete TL-REPORT"
    );
}

#[test]
fn test_tx_reporter_transmitted_emits_first_complete_tl_report() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 96;
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_tl_data_req_with_handle(addr, req_handle));
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    assert_eq!(reporter.get_state(), TxState::Pending);

    reporter.mark_transmitted();
    test.run_stack(Some(1));
    let report_msgs = test.dump_sinks();

    // EN 300 392-2 clause 22.3.2.3(f): the first complete BL-DATA
    // transmission starts T.251 and reports first-complete transfer even when
    // UMAC reports completion through TxReporter instead of TMA-REPORT.ind.
    assert!(
        find_tla_report(&report_msgs, req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION),
        "TxReporter transmission must emit first-complete TL-REPORT"
    );
    assert!(
        report_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "first-complete report must not retransmit before T.251 expires"
    );
}

#[test]
fn test_tma_success_report_marks_bl_data_transmitted_and_allows_ack() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 77;
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac]);

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    assert_eq!(reporter.get_state(), TxState::Pending);

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::SuccessReservedOrStealing));
    test.run_stack(Some(1));
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Transmitted);

    test.submit_message(build_bl_ack_ind(addr, 0));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Acknowledged);
}

#[test]
fn test_matching_bl_ack_without_payload_emits_tl_data_confirm() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 93;
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    reporter.mark_transmitted();
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_bl_ack_ind(addr, 0));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let conf = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataConfBl(prim) if prim.main_address == addr => Some(prim),
            _ => None,
        })
        .expect("matching BL-ACK should produce TL-DATA.conf");
    assert_eq!(conf.req_handle, req_handle);
    assert_eq!(conf.report, TLA_REPORT_SUCCESSFUL_TRANSFER);
    assert!(conf.tl_sdu.is_none(), "empty BL-ACK should confirm without response TL-SDU");
    assert!(
        !sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "matching BL-ACK without payload must not be reported as TL-DATA.ind"
    );
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Acknowledged);
}

#[test]
fn test_matching_bl_ack_reconciles_tx_reporter_before_periodic_tick() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 94;
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    reporter.mark_transmitted();

    // EN 300 392-2 clause 22.3.2.3(f,j): a peer BL-ACK can arrive after
    // UMAC has completed BL-DATA transmission but before LLC's periodic
    // retransmission scan has observed TxReporter. ACK processing must first
    // reconcile that TxReporter completion, then accept the matching N(R).
    test.submit_message(build_bl_ack_ind(addr, 0));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        find_tla_report(&sink_msgs, req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION),
        "early BL-ACK must not suppress the first-complete TL-REPORT"
    );

    let conf = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataConfBl(prim) if prim.main_address == addr => Some(prim),
            _ => None,
        })
        .expect("matching BL-ACK after TxReporter completion should produce TL-DATA.conf");
    assert_eq!(conf.req_handle, req_handle);
    assert_eq!(conf.report, TLA_REPORT_SUCCESSFUL_TRANSFER);
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Acknowledged);
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "matching early BL-ACK must not requeue the acknowledged BL-DATA"
    );
}

#[test]
fn test_matching_bl_ack_before_local_umac_completion_completes_transfer_without_retransmit() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 95;
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    assert_eq!(reporter.get_state(), TxState::Pending);

    // EN 300 392-2 clause 22.3.2.3(j): a matching BL-ACK acknowledges the
    // BL-DATA transfer. In production the uplink ACK can be decoded before the
    // asynchronous local UMAC completion reporter is observed; accepting that
    // matching ACK avoids a duplicate acknowledged downlink PDU during CMCE
    // call setup while preserving N(R) validation.
    test.submit_message(build_bl_ack_ind(addr, 0));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        find_tla_report(&sink_msgs, req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION),
        "matching BL-ACK must synthesize the first-complete TL-REPORT when it beats local UMAC completion"
    );
    let conf = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataConfBl(prim) if prim.main_address == addr => Some(prim),
            _ => None,
        })
        .expect("matching BL-ACK before local UMAC completion should produce TL-DATA.conf");
    assert_eq!(conf.req_handle, req_handle);
    assert_eq!(conf.report, TLA_REPORT_SUCCESSFUL_TRANSFER);
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Acknowledged);
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "matching BL-ACK before local UMAC completion must not requeue or retransmit the acknowledged BL-DATA"
    );
}

#[test]
fn test_unexpected_bl_ack_with_payload_is_delivered_as_tl_data_ind() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    // EN 300 392-2 clause 22.3.2.3(j): if no TL-SDU is waiting for
    // retransmission, a valid contained TL-SDU is delivered using
    // TL-DATA.ind.
    test.submit_message(build_bl_ack_ind_with_payload(addr, 0, &[0xA5]));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let data_ind = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataIndBl(prim) if prim.main_address == addr => Some(prim),
            _ => None,
        })
        .expect("unexpected BL-ACK payload should be delivered as TL-DATA.ind");
    assert_eq!(
        data_ind
            .tl_sdu
            .as_ref()
            .expect("TL-DATA.ind should carry the BL-ACK payload")
            .to_bitstr(),
        "10100101"
    );
    assert!(
        !sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataConfBl(_))),
        "unexpected BL-ACK payload must not confirm a transfer"
    );
    assert!(
        !sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "unexpected BL-ACK payload must not schedule a retry without an outstanding TL-SDU"
    );
}

#[test]
fn test_unexpected_empty_bl_ack_without_outstanding_downlink_is_noop() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    // EN 300 392-2 clause 22.3.2.3(j): when no TL-SDU is waiting for
    // retransmission and the BL-ACK carries no optional TL-SDU, there is no
    // downlink state to mutate and nothing to deliver upward.
    test.submit_message(build_bl_ack_ind(addr, 0));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        sink_msgs.is_empty(),
        "empty BL-ACK without an outstanding downlink must not confirm, retry, or deliver data"
    );
}

#[test]
fn test_wrong_bl_ack_with_payload_retries_and_delivers_payload_as_tl_data_ind() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 94;
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    reporter.mark_transmitted();
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 clause 22.3.2.3(j): wrong N(R) is not a successful
    // ACK, so the BS keeps the TL-SDU for retransmission; the valid contained
    // TL-SDU is nevertheless delivered using TL-DATA.ind.
    test.submit_message(build_bl_ack_ind_with_payload(addr, 1, &[0xA5]));
    test.run_stack(Some(1));
    let mut sink_msgs = test.dump_sinks();

    let retry_ns: Vec<u8> = sink_msgs.iter().filter_map(bl_data_ns).collect();
    assert_eq!(retry_ns, vec![0], "wrong BL-ACK should retry the original N(S)");
    let retry_reporter = take_first_tma_req_reporter(&mut sink_msgs);
    assert_eq!(retry_reporter.get_state(), TxState::Pending);
    let data_ind = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataIndBl(prim) if prim.main_address == addr => Some(prim),
            _ => None,
        })
        .expect("wrong BL-ACK payload should be delivered as TL-DATA.ind");
    assert!(
        data_ind
            .tl_sdu
            .as_ref()
            .expect("TL-DATA.ind should carry the BL-ACK payload")
            .to_bitstr()
            == "10100101"
    );
    assert!(
        !sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataConfBl(_))),
        "wrong BL-ACK payload must not confirm the transfer"
    );
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Transmitted);
}

#[test]
fn test_wrong_bl_adata_retransmission_piggybacks_ack_for_contained_ns() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req(addr);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    reporter.mark_transmitted();
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 clause 22.3.2.3(l) handles BL-ADATA as BL-ACK first,
    // then BL-DATA. Clause 22.3.2.3(k) keeps the old TL-SDU for retry when
    // N(R) is wrong, and clause 22.3.2.3(d) allows the contained DATA ACK to
    // be folded into that retry as BL-ADATA at MAC-ready time.
    test.submit_message(build_bl_adata_ind_with_payload_and_fcs(addr, 1, 1, &[0xA5], false));
    test.run_stack(Some(1));
    let mut sink_msgs = test.dump_sinks();

    let data_ind = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataIndBl(prim) if prim.main_address == addr => Some(prim),
            _ => None,
        })
        .expect("BL-ADATA contained N(S) should be delivered as TL-DATA.ind");
    assert_eq!(
        data_ind
            .tl_sdu
            .as_ref()
            .expect("TL-DATA.ind should carry the BL-ADATA payload")
            .to_bitstr(),
        "10100101"
    );
    assert!(
        !sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataConfBl(_))),
        "wrong BL-ADATA N(R) must not confirm the outstanding downlink transfer"
    );
    assert_eq!(
        sink_msgs.iter().filter_map(bl_adata_nr_ns).collect::<Vec<_>>(),
        vec![(1, 0)],
        "retry should piggyback ACK for contained N(S)=1 while retransmitting original N(S)=0"
    );
    let retry_reporter = take_first_tma_req_reporter(&mut sink_msgs);
    assert_eq!(retry_reporter.get_state(), TxState::Pending);
    assert!(
        sink_msgs.iter().filter_map(bl_data_ns).collect::<Vec<_>>().is_empty(),
        "retry should not be emitted as standalone BL-DATA when BL-ADATA fits"
    );
    assert!(
        !sink_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlAck)),
        "contained N(S) ACK should be consumed by retry BL-ADATA, not sent separately"
    );
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Transmitted);
}

#[test]
fn test_tma_failed_transfer_report_terminates_bl_data_without_t251_retry() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 88;
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_tl_data_req_with_handle(addr, req_handle));
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::FailedTransfer));
    test.run_stack(Some(1));
    let failed_msgs = test.dump_sinks();
    assert_eq!(reporter.get_state(), TxState::Discarded);
    assert!(
        find_tla_report(&failed_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "generic TMA failed-transfer should report failed transfer immediately"
    );

    test.run_stack(Some(20));
    let retry_msgs = test.dump_sinks();
    let retry_ns: Vec<u8> = retry_msgs.iter().filter_map(bl_data_ns).collect();
    assert!(
        retry_ns.is_empty(),
        "generic TMA failed-transfer must not be retried through the T.251 path"
    );
    assert_eq!(reporter.get_state(), TxState::Discarded);
}

#[test]
fn test_bl_data_fragmentation_failure_retries_immediately_with_same_ns() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 188;
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let initial_ns: Vec<u8> = initial_msgs.iter().filter_map(bl_data_ns).collect();
    assert_eq!(initial_ns, vec![0]);
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);

    // EN 300 392-2 clause 22.3.2.3(h): fragmentation failure for
    // BL-DATA/BL-ADATA signals DATA_IN_BUFFER immediately while N.252 permits.
    test.submit_message(build_tma_report_ind(req_handle, TmaReport::FragmentationFailure));
    test.run_stack(Some(1));
    let retry_msgs = test.dump_sinks();
    let retry_ns: Vec<u8> = retry_msgs.iter().filter_map(bl_data_ns).collect();
    assert_eq!(retry_ns, vec![0], "fragmentation failure should retry the same N(S) immediately");
    assert!(
        !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "first fragmentation failure must keep the TL-SDU while N.252 permits"
    );
    assert_eq!(reporter.get_state(), TxState::Discarded);
    assert_eq!(service_reporter.get_state(), TxState::Pending);
}

#[test]
fn test_bl_data_fragmentation_failure_retries_with_fresh_mac_reporter_and_pending_service_reporter() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 1881;
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());

    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let first_mac_reporter = take_first_tma_req_reporter(&mut initial_msgs);
    assert_eq!(first_mac_reporter.get_state(), TxState::Pending);
    assert_eq!(service_reporter.get_state(), TxState::Pending);
    assert!(
        !first_mac_reporter.shares_state_with(&service_reporter),
        "acknowledged BL-DATA must not reuse the service reporter as the per-attempt MAC reporter"
    );

    // EN 300 392-2 clause 22.3.2.3(h): a fragmentation failure retries the
    // stored TL-SDU while N.252 permits. The failed MAC attempt is discarded,
    // but the service-level SDS/LLC transaction remains pending.
    test.submit_message(build_tma_report_ind(req_handle, TmaReport::FragmentationFailure));
    test.run_stack(Some(1));
    let mut retry_msgs = test.dump_sinks();
    assert_eq!(first_mac_reporter.get_state(), TxState::Discarded);
    assert_eq!(service_reporter.get_state(), TxState::Pending);
    assert!(
        !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "first fragmentation failure must not fail the service reporter while retry is still allowed"
    );

    let retry_mac_reporter = take_first_tma_req_reporter(&mut retry_msgs);
    assert_eq!(retry_mac_reporter.get_state(), TxState::Pending);
    assert!(!retry_mac_reporter.shares_state_with(&first_mac_reporter));
    assert!(!retry_mac_reporter.shares_state_with(&service_reporter));
}

#[test]
fn test_bl_data_fragmentation_failure_exhaustion_reports_failed_transfer() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 189;
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_tl_data_req_with_handle(addr, req_handle));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    for attempt in 1..=N252_BL_MAX_TLSDU_RETRANSMITS_ACKED {
        test.submit_message(build_tma_report_ind(req_handle, TmaReport::FragmentationFailure));
        test.run_stack(Some(1));
        let retry_msgs = test.dump_sinks();
        let retry_ns: Vec<u8> = retry_msgs.iter().filter_map(bl_data_ns).collect();
        assert_eq!(retry_ns, vec![0], "N.252 fragmentation retry attempt {attempt} should reuse N(S)");
        assert!(
            !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
            "retry attempt {attempt} should not fail the transfer before N.252 is exhausted"
        );
    }

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::FragmentationFailure));
    test.run_stack(Some(1));
    let failed_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&failed_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "EN 300 392-2 22.3.2.3(h) requires failed-transfer report after N.252 is exceeded"
    );
    assert!(
        !failed_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "no further BL-DATA retransmission may be queued after N.252 fragmentation exhaustion"
    );
}

#[test]
fn test_bl_data_t251_exhaustion_after_complete_transmission_marks_service_reporter_lost() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 1891;
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let mut mac_reporter = take_first_tma_req_reporter(&mut initial_msgs);
    mac_reporter.mark_transmitted();

    test.run_stack(Some(1));
    let first_complete_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&first_complete_msgs, req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION),
        "first complete MAC transmission should surface to the service reporter"
    );
    assert_eq!(service_reporter.get_state(), TxState::Transmitted);

    for attempt in 1..=N252_BL_MAX_TLSDU_RETRANSMITS_ACKED {
        let mut retry_msgs = Vec::new();
        for _ in 0..32 {
            test.run_stack(Some(1));
            retry_msgs.extend(test.dump_sinks());
            if retry_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))) {
                break;
            }
        }
        assert!(
            !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
            "T.251 retry attempt {attempt} must not fail before N.252 is exceeded"
        );
        mac_reporter = take_first_tma_req_reporter(&mut retry_msgs);
        mac_reporter.mark_transmitted();
        test.run_stack(Some(1));
        let _ = test.dump_sinks();
        assert_eq!(service_reporter.get_state(), TxState::Transmitted);
    }

    let mut failed_msgs = Vec::new();
    for _ in 0..32 {
        test.run_stack(Some(1));
        failed_msgs.extend(test.dump_sinks());
        if find_tla_report(&failed_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER) {
            break;
        }
    }

    assert!(
        find_tla_report(&failed_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "T.251/N.252 exhaustion after complete transmission should fail the transfer"
    );
    assert_eq!(
        service_reporter.get_state(),
        TxState::Lost,
        "after at least one complete MAC transmission, no peer BL-ACK means Lost, not Discarded"
    );
    assert_eq!(mac_reporter.get_state(), TxState::Transmitted);
}

#[test]
fn test_bl_adata_fragmentation_failure_retries_immediately_with_same_nr_ns() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 187;
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind(addr, 1));
    test.submit_message(build_tl_data_req_with_handle_timeslot(addr, req_handle, 3));
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();
    let initial_nr_ns: Vec<(u8, u8)> = initial_msgs.iter().filter_map(bl_adata_nr_ns).collect();
    assert_eq!(initial_nr_ns, vec![(1, 0)]);

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::FragmentationFailure));
    test.run_stack(Some(1));
    let retry_msgs = test.dump_sinks();
    let retry_nr_ns: Vec<(u8, u8)> = retry_msgs.iter().filter_map(bl_adata_nr_ns).collect();
    assert_eq!(
        retry_nr_ns,
        vec![(1, 0)],
        "fragmentation retry should preserve BL-ADATA N(R) and N(S)"
    );
    assert!(
        !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "first BL-ADATA fragmentation failure must keep the TL-SDU while N.252 permits"
    );
}

#[test]
fn test_bl_adata_fragmentation_retry_uses_latest_waiting_ack() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 186;
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind(addr, 1));
    test.submit_message(build_tl_data_req_with_handle_timeslot(addr, req_handle, 3));
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();
    assert_eq!(
        initial_msgs.iter().filter_map(bl_adata_nr_ns).collect::<Vec<_>>(),
        vec![(1, 0)],
        "initial MAC-ready TL-DATA should consume the first waiting ACK as BL-ADATA"
    );

    // EN 300 392-2 clause 22.3.2.3 note 2 stops acknowledgement actions for
    // an older received BL-DATA when a newer BL-DATA arrives before it is
    // acknowledged. The retransmission buffer must therefore refresh the
    // embedded BL-ADATA N(R), rather than replaying the stale N(R)=1.
    test.submit_message(build_bl_data_ind(addr, 0));
    test.deliver_all_messages();
    let newer_ind_msgs = test.dump_sinks();
    assert!(
        newer_ind_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(prim) if prim.main_address == addr)),
        "newer inbound BL-DATA should be delivered while its ACK remains pending"
    );
    assert!(
        newer_ind_msgs
            .iter()
            .all(|msg| !matches!(llc_pdu_type(msg), Some(LlcPduType::BlAck | LlcPduType::BlAdata))),
        "newer ACK must remain pending until the retry can fold it into BL-ADATA"
    );

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::FragmentationFailure));
    test.run_stack(Some(1));
    let retry_msgs = test.dump_sinks();
    assert_eq!(
        retry_msgs.iter().filter_map(bl_adata_nr_ns).collect::<Vec<_>>(),
        vec![(0, 0)],
        "fragmentation retry must carry the latest waiting N(R)=0, not stale N(R)=1"
    );
    assert!(
        retry_msgs
            .iter()
            .all(|msg| !matches!(llc_pdu_type(msg), Some(LlcPduType::BlAck | LlcPduType::BlData))),
        "latest ACK should be folded into the BL-ADATA retry without an extra standalone BL-ACK"
    );
}

#[test]
fn test_t251_retransmission_exhaustion_emits_failed_transfer_report() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 95;
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let mut reporter = take_first_tma_req_reporter(&mut initial_msgs);
    reporter.mark_transmitted();

    for attempt in 1..=N252_BL_MAX_TLSDU_RETRANSMITS_ACKED {
        test.run_stack(Some(20));
        let mut retry_msgs = test.dump_sinks();
        let retry_ns: Vec<u8> = retry_msgs.iter().filter_map(bl_data_ns).collect();
        assert_eq!(retry_ns, vec![0], "N.252 retry attempt {attempt} should retransmit the same N(S)");
        reporter = take_first_tma_req_reporter(&mut retry_msgs);
        assert_eq!(reporter.get_state(), TxState::Pending);
        reporter.mark_transmitted();
    }

    test.run_stack(Some(20));
    let final_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&final_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "EN 300 392-2 22.3.2.3(i/k) requires TL-REPORT failed transfer after N.252 is exhausted"
    );
    assert!(
        !final_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "no further BL-DATA retransmission may be queued after N.252 exhaustion"
    );
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Lost);
}

#[test]
fn test_channel_allocation_t251_exhaustion_keeps_late_ack_grace() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2260616, SsiType::Issi);
    let req_handle = 951;
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle_timeslot(addr, req_handle, 2);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let mut reporter = take_first_tma_req_reporter(&mut initial_msgs);
    reporter.mark_transmitted();

    for attempt in 1..=N252_BL_MAX_TLSDU_RETRANSMITS_ACKED {
        test.run_stack(Some(20));
        let mut retry_msgs = test.dump_sinks();
        let retry_ns: Vec<u8> = retry_msgs.iter().filter_map(bl_data_ns).collect();
        assert_eq!(
            retry_ns,
            vec![0],
            "channel-allocation retry attempt {attempt} should retain the original N(S)"
        );
        reporter = take_first_tma_req_reporter(&mut retry_msgs);
        reporter.mark_transmitted();
    }

    test.run_stack(Some(20));
    let grace_msgs = test.dump_sinks();
    assert!(
        !find_tla_report(&grace_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "EN 300 392-2 Annex D.4 channel-allocation setup keeps a bounded late-ACK grace before failing"
    );
    assert!(
        grace_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "late-ACK grace must not queue extra retransmissions after N.252 is exhausted"
    );
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Transmitted);

    test.submit_message(build_bl_ack_ind(addr, 0));
    test.run_stack(Some(1));
    let ack_msgs = test.dump_sinks();
    assert!(
        ack_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::TlaTlDataConfBl(prim)
                if prim.req_handle == req_handle && prim.report == TLA_REPORT_SUCCESSFUL_TRANSFER
        )),
        "a matching BL-ACK inside the channel-allocation grace should still confirm the TL-DATA transfer"
    );
    assert_eq!(service_reporter.get_state(), TxState::Acknowledged);
}

#[test]
fn test_t251_retry_waits_until_four_target_signalling_frames() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 96;
    let target_timeslot = 3;
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle_timeslot(addr, req_handle, target_timeslot);
    if let SapMsgInner::TlaTlDataReqBl(prim) = &mut req.msg {
        // This test is about assigned-channel recovery timing. The first
        // non-stealing late-assignment D-CONNECT ACK path intentionally waits
        // for BL-ACK on the current control channel.
        prim.stealing_permission = true;
    }
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    reporter.mark_transmitted();

    let mut retry_tick_after_report = None;
    for tick_after_report in 1..=20 {
        test.run_stack(Some(1));
        let retry_msgs = test.dump_sinks();
        let retry_ns: Vec<u8> = retry_msgs.iter().filter_map(bl_data_ns).collect();
        assert!(
            !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
            "T.251 boundary test must not exhaust N.252 before the first retry"
        );

        if retry_ns.is_empty() {
            assert!(
                tick_after_report < 14,
                "EN 300 392-2 Annex A.1/T.251 should have retransmitted at the fourth target signalling frame"
            );
            continue;
        }

        assert_eq!(retry_ns, vec![0], "first T.251 retry should retransmit the same BL-DATA N(S)");
        retry_tick_after_report = Some(tick_after_report);
        break;
    }

    assert_eq!(
        retry_tick_after_report,
        Some(14),
        "EN 300 392-2 Annex A.1 counts T.251 in downlink signalling frames for the target timeslot"
    );
}

#[test]
fn test_random_access_failure_emits_failed_transfer_report() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 94;
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_tl_data_req_with_handle(addr, req_handle));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_tma_report_ind(req_handle, TmaReport::RandomAccessFailure));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        find_tla_report(&sink_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "EN 300 392-2 22.3.2.3(g) requires failed-transfer TL-REPORT on random-access failure"
    );
}

#[test]
fn test_random_access_failure_discards_without_retransmission() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 89;
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac]);

    test.submit_message(build_tl_data_req_with_handle(addr, req_handle));
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);

    // EN 300 392-2 clause 22.3.2.3(g): random-access failure for a PDU
    // containing service-user data is a failed transfer and is not retried by
    // the LLC N.252/T.251 retransmission path.
    test.submit_message(build_tma_report_ind(req_handle, TmaReport::RandomAccessFailure));
    test.run_stack(Some(1));
    assert_eq!(reporter.get_state(), TxState::Discarded);
    assert!(test.dump_sinks().is_empty());

    test.run_stack(Some(20));
    assert!(
        test.dump_sinks().is_empty(),
        "random-access failure should discard the pending BL-DATA without retransmission"
    );
    assert_eq!(reporter.get_state(), TxState::Discarded);
}

#[test]
fn test_wrong_bl_ack_retransmits_immediately_after_transmit_report() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac]);

    test.submit_message(build_tl_data_req(addr));
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let initial_data = initial_msgs
        .iter_mut()
        .find_map(|msg| match &mut msg.msg {
            SapMsgInner::TmaUnitdataReq(prim) => Some(prim),
            _ => None,
        })
        .expect("expected initial BL-DATA transmission");
    assert_eq!(
        bl_data_ns(&SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmaUnitdataReq(initial_data.clone()),
        }),
        Some(0)
    );
    let reporter = initial_data.tx_reporter.take().expect("initial BL-DATA should have TxReporter");
    reporter.mark_transmitted();

    test.run_stack(Some(1));
    assert!(
        test.dump_sinks().is_empty(),
        "transmit report alone should not retransmit before T.251"
    );

    test.submit_message(build_bl_ack_ind(addr, 1));
    test.run_stack(Some(1));
    let retry_msgs = test.dump_sinks();
    let retry_ns: Vec<u8> = retry_msgs.iter().filter_map(bl_data_ns).collect();
    assert_eq!(retry_ns, vec![0], "wrong BL-ACK N(R) should trigger immediate retry with same N(S)");
}

#[test]
fn test_wrong_bl_ack_before_first_complete_is_ignored_and_keeps_pending_transfer() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 191;
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    assert_eq!(reporter.get_state(), TxState::Pending);

    // EN 300 392-2 clause 22.3.2.3(f/k) starts ACK evaluation after the
    // first complete transmission report. A premature wrong BL-ACK must not
    // confirm, fail, retransmit, or consume an N.252 retry attempt.
    test.submit_message(build_bl_ack_ind(addr, 1));
    test.run_stack(Some(1));
    let premature_ack_msgs = test.dump_sinks();
    assert_eq!(reporter.get_state(), TxState::Pending);
    assert!(
        premature_ack_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "premature wrong BL-ACK must not queue a retransmission"
    );
    assert!(
        !find_tla_report(&premature_ack_msgs, req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION)
            && !find_tla_report(&premature_ack_msgs, req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER)
            && !find_tla_report(&premature_ack_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "premature wrong BL-ACK must not produce transfer reports"
    );

    reporter.mark_transmitted();
    test.submit_message(build_bl_ack_ind(addr, 0));
    test.run_stack(Some(1));
    let confirmed_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&confirmed_msgs, req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION),
        "matching ACK after first complete should still reconcile the pending transfer"
    );
    assert!(
        confirmed_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::TlaTlDataConfBl(prim)
                if prim.req_handle == req_handle && prim.report == TLA_REPORT_SUCCESSFUL_TRANSFER
        )),
        "matching ACK after premature wrong ACK should confirm the TL-DATA request"
    );
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Acknowledged);
}

#[test]
fn test_wrong_bl_ack_exhaustion_reports_failed_transfer_and_drops_pending() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let req_handle = 190;
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req_with_handle(addr, req_handle);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let mut reporter = take_first_tma_req_reporter(&mut initial_msgs);
    reporter.mark_transmitted();

    for attempt in 1..=N252_BL_MAX_TLSDU_RETRANSMITS_ACKED {
        test.submit_message(build_bl_ack_ind(addr, 1));
        test.run_stack(Some(1));
        let mut retry_msgs = test.dump_sinks();
        let retry_ns: Vec<u8> = retry_msgs.iter().filter_map(bl_data_ns).collect();
        assert_eq!(
            retry_ns,
            vec![0],
            "wrong BL-ACK N(R) should retry same N(S) while N.252 permits; attempt {attempt}"
        );
        assert!(
            !find_tla_report(&retry_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
            "wrong BL-ACK attempt {attempt} must not fail before N.252 is exceeded"
        );
        reporter = take_first_tma_req_reporter(&mut retry_msgs);
        assert_eq!(reporter.get_state(), TxState::Pending);
        reporter.mark_transmitted();
    }

    test.submit_message(build_bl_ack_ind(addr, 1));
    test.run_stack(Some(1));
    let failed_msgs = test.dump_sinks();

    // EN 300 392-2 clause 22.3.2.3(j): if BL-ACK N(R) does not equal V(S),
    // LLC retransmits while N.252 permits, then reports failed transfer and
    // discards the TL-SDU from the sending buffer.
    assert!(
        find_tla_report(&failed_msgs, req_handle, TLA_REPORT_FAILED_TRANSFER),
        "wrong BL-ACK after N.252 is exceeded must report failed transfer"
    );
    assert!(
        !failed_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "no further BL-DATA retransmission may be queued after wrong-ACK N.252 exhaustion"
    );
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Lost);

    test.submit_message(build_bl_ack_ind(addr, 1));
    test.run_stack(Some(1));
    assert!(
        test.dump_sinks().is_empty(),
        "wrong-ACK exhaustion must drop the pending TL-SDU so later duplicate ACKs have no side effects"
    );
}

#[test]
fn test_bl_ack_with_response_payload_acknowledges_downlink_and_delivers_tl_data_conf() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req(addr);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    reporter.mark_transmitted();
    test.run_stack(Some(1));
    let report_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&report_msgs, 0, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION),
        "transmitted BL-DATA should emit first-complete TL-REPORT"
    );
    assert!(
        report_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "transmitted BL-DATA should not retransmit before peer BL-ACK"
    );

    // EN 300 392-2 clause 22.3.2.3(j): a BL-ACK may both acknowledge
    // the pending TL-SDU and carry the peer's TL-DATA response payload.
    test.submit_message(build_bl_ack_ind_with_payload(addr, 0, &[0xA5]));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Acknowledged);

    let data_conf = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataConfBl(prim) if prim.main_address == addr => Some(prim),
            _ => None,
        })
        .expect("matching BL-ACK response payload should be delivered as TL-DATA.conf");
    assert_eq!(data_conf.report, TLA_REPORT_SUCCESSFUL_TRANSFER);
    let payload_bits = data_conf
        .tl_sdu
        .as_ref()
        .expect("TL-DATA.conf should carry response TL-SDU")
        .to_bitstr();
    assert_eq!(payload_bits, "10100101");
    assert!(
        !sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "matching BL-ACK response payload must not be delivered as TL-DATA.ind"
    );

    assert!(
        !sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "matching BL-ACK with response payload must not schedule a retry"
    );
}

#[test]
fn test_bl_ack_with_short_response_payload_delivers_tl_data_conf_bits() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let service_reporter = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req = build_tl_data_req(addr);
    attach_tl_data_req_reporter(&mut req, service_reporter.clone());
    test.submit_message(req);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter = take_first_tma_req_reporter(&mut initial_msgs);
    reporter.mark_transmitted();
    test.run_stack(Some(1));
    let report_msgs = test.dump_sinks();
    assert!(
        find_tla_report(&report_msgs, 0, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION),
        "transmitted BL-DATA should emit first-complete TL-REPORT"
    );
    assert!(
        report_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TmaUnitdataReq(_))),
        "transmitted BL-DATA should not retransmit before peer BL-ACK"
    );

    // EN 300 392-2 clause 22.3.2.3(j) does not restrict the BL-ACK response
    // TL-SDU to whole octets; preserve 1..=4 bit responses instead of
    // treating them as absent.
    test.submit_message(build_bl_ack_ind_with_payload_bits(addr, 0, "1010"));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter.get_state(), TxState::Acknowledged);
    let data_conf = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataConfBl(prim) if prim.main_address == addr => Some(prim),
            _ => None,
        })
        .expect("matching BL-ACK short response payload should be delivered as TL-DATA.conf");
    assert_eq!(data_conf.report, TLA_REPORT_SUCCESSFUL_TRANSFER);
    let payload_bits = data_conf
        .tl_sdu
        .as_ref()
        .expect("TL-DATA.conf should carry short response TL-SDU")
        .to_bitstr();
    assert_eq!(payload_bits, "1010");
    assert!(
        !sink_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "matching BL-ACK short response payload must not be delivered as TL-DATA.ind"
    );
}

#[test]
fn test_tl_data_response_before_ack_is_sent_as_bl_ack_payload() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind(addr, 1));
    test.deliver_all_messages();
    let data_ind_msgs = test.dump_sinks();
    let (ind_handle, _) = data_ind_msgs
        .iter()
        .find_map(tl_data_ind_handle_and_payload_bits)
        .expect("incoming BL-DATA should produce TL-DATA.ind with retained handle");
    assert!(
        data_ind_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(prim) if prim.main_address == addr)),
        "incoming BL-DATA should be delivered before the response is formed"
    );

    // EN 300 392-2 clause 22.3.2.3(b): response before the waiting
    // acknowledgement is sent is carried as service-user data in BL-ACK.
    test.submit_message(build_tl_data_resp_with_handle(addr, ind_handle, &[0xCC]));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    let ack = sink_msgs
        .iter()
        .find_map(bl_ack_prio_nr_and_payload_bits)
        .expect("TL-DATA.resp should consume pending ACK and emit BL-ACK with payload");
    assert_eq!(ack, (5, 1, "11001100".to_owned()));
    assert!(
        !sink_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlData)),
        "response-before-ACK must not be sent as a separate BL-DATA"
    );
}

#[test]
fn test_new_bl_data_replaces_pending_ack_before_mac_ready_standalone_ack() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind(addr, 0));
    test.deliver_all_messages();
    let first_msgs = test.dump_sinks();
    assert!(
        first_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(prim) if prim.main_address == addr)),
        "first incoming BL-DATA should be delivered to the service user"
    );
    assert!(
        first_msgs.iter().all(|msg| llc_pdu_type(msg) != Some(LlcPduType::BlAck)),
        "first BL-DATA acknowledgement is still pending before MAC-ready tick_end"
    );

    // EN 300 392-2 clause 22.3.2.3 note 2: a new BL-DATA before the
    // previous BL-DATA is acknowledged stops all acknowledgement actions for
    // the previous TL-SDU, independent of N(S).
    test.submit_message(build_bl_data_ind_with_payload_and_fcs(addr, 1, &[0xB1], false));
    test.deliver_all_messages();
    let second_msgs = test.dump_sinks();
    assert!(
        second_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(prim)
            if prim.main_address == addr
                && prim.tl_sdu.as_ref().map(|tl_sdu| tl_sdu.to_bitstr()).as_deref() == Some("10110001"))),
        "second incoming BL-DATA should still be delivered to the service user"
    );
    assert!(
        second_msgs.iter().all(|msg| llc_pdu_type(msg) != Some(LlcPduType::BlAck)),
        "replacement acknowledgement is still pending before MAC-ready tick_end"
    );

    test.run_stack(Some(1));
    let ack_msgs = test.dump_sinks();
    let acks: Vec<(u8, String)> = ack_msgs.iter().filter_map(bl_ack_nr_and_payload_bits).collect();
    assert_eq!(
        acks,
        vec![(1, String::new())],
        "only the latest waiting BL-DATA should be acknowledged at MAC-ready tick_end"
    );
}

#[test]
fn test_duplicate_inbound_bl_data_after_ack_is_not_delivered_again_but_is_acked() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind_with_payload_and_fcs(addr, 0, &[0xA0], false));
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    assert_eq!(
        first_msgs
            .iter()
            .filter_map(tl_data_ind_handle_and_payload_bits)
            .map(|(_, payload)| payload)
            .collect::<Vec<_>>(),
        vec!["10100000".to_owned()],
        "first valid BL-DATA should be delivered as TL-DATA.ind"
    );
    assert_eq!(
        first_msgs.iter().filter_map(bl_ack_nr_and_payload_bits).collect::<Vec<_>>(),
        vec![(0, String::new())],
        "first valid BL-DATA should be acknowledged"
    );

    // EN 300 392-2 clause 22.3.2.3 with Annex A.1 retransmission behaviour:
    // if the MS repeats the same valid N(S), LLC must ACK the duplicate while
    // suppressing a second service-user TL-DATA indication.
    test.submit_message(build_bl_data_ind_with_payload_and_fcs(addr, 0, &[0xA1], false));
    test.deliver_all_messages();
    let duplicate_delivery_msgs = test.dump_sinks();
    assert!(
        duplicate_delivery_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "duplicate BL-DATA N(S) must not emit another TL-DATA.ind"
    );
    assert!(
        duplicate_delivery_msgs
            .iter()
            .all(|msg| llc_pdu_type(msg) != Some(LlcPduType::BlAck)),
        "duplicate BL-DATA ACK should be scheduled for MAC-ready, not emitted immediately"
    );

    test.run_stack(Some(1));
    let duplicate_ack_msgs = test.dump_sinks();
    assert_eq!(
        duplicate_ack_msgs.iter().filter_map(bl_ack_nr_and_payload_bits).collect::<Vec<_>>(),
        vec![(0, String::new())],
        "duplicate BL-DATA N(S) should still be acknowledged"
    );
    assert!(
        duplicate_ack_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "duplicate BL-DATA ACK tick must not include a delayed duplicate TL-DATA.ind"
    );
}

#[test]
fn test_inbound_bl_data_same_ns_after_retry_horizon_is_delivered_again() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind_with_payload_and_fcs(addr, 0, &[0xA0], false));
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    assert_eq!(
        first_msgs
            .iter()
            .filter_map(tl_data_ind_handle_and_payload_bits)
            .map(|(_, payload)| payload)
            .collect::<Vec<_>>(),
        vec!["10100000".to_owned()],
        "first valid BL-DATA should be delivered as TL-DATA.ind"
    );
    assert_eq!(
        first_msgs.iter().filter_map(bl_ack_nr_and_payload_bits).collect::<Vec<_>>(),
        vec![(0, String::new())],
        "first valid BL-DATA should be acknowledged"
    );

    // EN 300 392-2 clause 22.3.2.3 note 3 says N(S) alone is not a safe
    // indefinite duplicate-suppression mechanism. After the full Annex A.1
    // T.251/N.252 retry envelope, the same N(S) may be a new transfer.
    test.run_stack(Some(LLC_INBOUND_DUPLICATE_SUPPRESSION_HORIZON_TICKS + 8));
    let idle_msgs = test.dump_sinks();
    assert!(
        idle_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "idle expiry ticks must not synthesize service-user data"
    );

    test.submit_message(build_bl_data_ind_with_payload_and_fcs(addr, 0, &[0xA2], false));
    test.run_stack(Some(1));
    let later_msgs = test.dump_sinks();
    assert_eq!(
        later_msgs
            .iter()
            .filter_map(tl_data_ind_handle_and_payload_bits)
            .map(|(_, payload)| payload)
            .collect::<Vec<_>>(),
        vec!["10100010".to_owned()],
        "same BL-DATA N(S) after the retry horizon should be delivered as a new TL-DATA.ind"
    );
    assert_eq!(
        later_msgs.iter().filter_map(bl_ack_nr_and_payload_bits).collect::<Vec<_>>(),
        vec![(0, String::new())],
        "same BL-DATA N(S) after the retry horizon should still be acknowledged"
    );
}

#[test]
fn test_duplicate_inbound_bl_adata_after_ack_is_not_delivered_again_but_is_acked() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_adata_ind_with_payload_and_fcs(addr, 0, 1, &[0xB0], false));
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    assert_eq!(
        first_msgs
            .iter()
            .filter_map(tl_data_ind_handle_and_payload_bits)
            .map(|(_, payload)| payload)
            .collect::<Vec<_>>(),
        vec!["10110000".to_owned()],
        "first valid BL-ADATA data half should be delivered as TL-DATA.ind"
    );
    assert_eq!(
        first_msgs.iter().filter_map(bl_ack_nr_and_payload_bits).collect::<Vec<_>>(),
        vec![(1, String::new())],
        "first valid BL-ADATA data half should be acknowledged"
    );

    // BL-ADATA carries an independent N(R) and N(S). Duplicate suppression is
    // only for the receive-side N(S) service indication; the duplicate N(S)
    // still requires a BL-ACK.
    test.submit_message(build_bl_adata_ind_with_payload_and_fcs(addr, 0, 1, &[0xB1], false));
    test.deliver_all_messages();
    let duplicate_delivery_msgs = test.dump_sinks();
    assert!(
        duplicate_delivery_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "duplicate BL-ADATA N(S) must not emit another TL-DATA.ind"
    );
    assert!(
        duplicate_delivery_msgs
            .iter()
            .all(|msg| llc_pdu_type(msg) != Some(LlcPduType::BlAck)),
        "duplicate BL-ADATA ACK should be scheduled for MAC-ready, not emitted immediately"
    );

    test.run_stack(Some(1));
    let duplicate_ack_msgs = test.dump_sinks();
    assert_eq!(
        duplicate_ack_msgs.iter().filter_map(bl_ack_nr_and_payload_bits).collect::<Vec<_>>(),
        vec![(1, String::new())],
        "duplicate BL-ADATA N(S) should still be acknowledged"
    );
    assert!(
        duplicate_ack_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "duplicate BL-ADATA ACK tick must not include a delayed duplicate TL-DATA.ind"
    );
}

#[test]
fn test_inbound_bl_adata_same_ns_after_retry_horizon_is_delivered_again() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_adata_ind_with_payload_and_fcs(addr, 0, 1, &[0xB0], false));
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    assert_eq!(
        first_msgs
            .iter()
            .filter_map(tl_data_ind_handle_and_payload_bits)
            .map(|(_, payload)| payload)
            .collect::<Vec<_>>(),
        vec!["10110000".to_owned()],
        "first valid BL-ADATA data half should be delivered as TL-DATA.ind"
    );
    assert_eq!(
        first_msgs.iter().filter_map(bl_ack_nr_and_payload_bits).collect::<Vec<_>>(),
        vec![(1, String::new())],
        "first valid BL-ADATA data half should be acknowledged"
    );

    test.run_stack(Some(LLC_INBOUND_DUPLICATE_SUPPRESSION_HORIZON_TICKS + 8));
    let idle_msgs = test.dump_sinks();
    assert!(
        idle_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "idle expiry ticks must not synthesize service-user data"
    );

    test.submit_message(build_bl_adata_ind_with_payload_and_fcs(addr, 0, 1, &[0xB2], false));
    test.run_stack(Some(1));
    let later_msgs = test.dump_sinks();
    assert_eq!(
        later_msgs
            .iter()
            .filter_map(tl_data_ind_handle_and_payload_bits)
            .map(|(_, payload)| payload)
            .collect::<Vec<_>>(),
        vec!["10110010".to_owned()],
        "same BL-ADATA N(S) after the retry horizon should be delivered as a new TL-DATA.ind"
    );
    assert_eq!(
        later_msgs.iter().filter_map(bl_ack_nr_and_payload_bits).collect::<Vec<_>>(),
        vec![(1, String::new())],
        "same BL-ADATA N(S) after the retry horizon should still be acknowledged"
    );
}

#[test]
fn test_corrupt_new_bl_data_fcs_cancels_prior_pending_ack_without_delivery() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind_with_payload_and_fcs(addr, 0, &[0xA0], false));
    test.deliver_all_messages();
    let first_msgs = test.dump_sinks();
    assert!(
        first_msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(prim)
            if prim.main_address == addr
                && prim.tl_sdu.as_ref().map(|tl_sdu| tl_sdu.to_bitstr()).as_deref() == Some("10100000"))),
        "first valid BL-DATA should be delivered to the service user"
    );
    assert!(
        first_msgs.iter().all(|msg| llc_pdu_type(msg) != Some(LlcPduType::BlAck)),
        "first BL-DATA acknowledgement is still pending before MAC-ready tick_end"
    );

    // EN 300 392-2 clause 22.3.2.3(k) and note 2: reception of a new
    // BL-DATA before the previous received TL-SDU is acknowledged stops all
    // acknowledgement actions for the previous TL-SDU independently of N(S).
    // A bad FCS still suppresses delivery and ACKing of the corrupt TL-SDU.
    test.submit_message(corrupt_last_bit(build_bl_data_ind_with_payload_and_fcs(addr, 1, &[0xB1], true)));
    test.deliver_all_messages();
    let corrupt_msgs = test.dump_sinks();
    assert!(
        corrupt_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(_))),
        "corrupt replacement BL-DATA must not be delivered"
    );
    assert!(
        corrupt_msgs.iter().all(|msg| llc_pdu_type(msg) != Some(LlcPduType::BlAck)),
        "corrupt replacement BL-DATA must not be acknowledged immediately"
    );

    test.run_stack(Some(1));
    let ack_msgs = test.dump_sinks();
    assert!(
        ack_msgs.iter().all(|msg| llc_pdu_type(msg) != Some(LlcPduType::BlAck)),
        "old pending BL-ACK must be cancelled by the corrupt newer BL-DATA"
    );
}

#[test]
fn test_zero_handle_tl_data_response_does_not_match_pending_ack() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind_with_payload_and_fcs(addr, 1, &[0xAB], false));
    test.deliver_all_messages();
    let data_ind_msgs = test.dump_sinks();
    let (ind_handle, payload_bits) = data_ind_msgs
        .iter()
        .find_map(tl_data_ind_handle_and_payload_bits)
        .expect("incoming BL-DATA should produce TL-DATA.ind");
    assert_ne!(ind_handle, 0, "LLC-generated TL-DATA.ind handle must be non-zero");
    assert_eq!(payload_bits, "10101011");

    // EN 300 392-2 clauses 22.3.1.1 and 22.3.2.3(b/c) require a
    // TL-DATA.response to carry the corresponding TL-DATA.ind handle. A zero
    // handle is not one of this LLC's generated handles and must not consume
    // the pending acknowledgement as an immediate BL-ACK response payload.
    test.submit_message(build_tl_data_resp(addr, &[0xCC]));
    test.deliver_all_messages();
    let immediate_msgs = test.dump_sinks();
    assert!(
        !immediate_msgs
            .iter()
            .any(|msg| matches!(llc_pdu_type(msg), Some(LlcPduType::BlAck | LlcPduType::BlAckFcs))),
        "zero-handle response must not be matched to the waiting ACK"
    );

    test.run_stack(Some(1));
    let tick_msgs = test.dump_sinks();
    assert_eq!(
        tick_msgs.iter().filter_map(bl_adata_nr_ns).collect::<Vec<_>>(),
        vec![(1, 0)],
        "zero-handle response falls back to a new acknowledged transfer and may combine with a waiting ACK at MAC-ready time"
    );
}

#[test]
fn test_tl_data_response_uses_matching_indication_handle_for_pending_ack() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind_with_payload_and_fcs(addr, 1, &[0xAB], false));
    test.deliver_all_messages();
    let data_ind_msgs = test.dump_sinks();
    let (ind_handle, payload_bits) = data_ind_msgs
        .iter()
        .find_map(tl_data_ind_handle_and_payload_bits)
        .expect("incoming BL-DATA should produce TL-DATA.ind");
    assert_ne!(ind_handle, 0, "LLC-generated TL-DATA.ind handle must be non-zero");
    assert_eq!(payload_bits, "10101011");

    // EN 300 392-2 clauses 22.3.1.1 and 22.3.2.3(b) require the
    // TL-DATA.response handle to identify the corresponding TL-DATA.ind. A
    // matching handle may consume that indication's still-waiting BL-ACK.
    test.submit_message(build_tl_data_resp_with_handle(addr, ind_handle, &[0xCC]));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    let ack = sink_msgs
        .iter()
        .find_map(bl_ack_nr_and_payload_bits)
        .expect("matching TL-DATA.response handle should consume pending ACK");
    assert_eq!(ack, (1, "11001100".to_owned()));
    assert!(
        !sink_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlData)),
        "response-before-ACK must not be sent as a separate BL-DATA"
    );
}

#[test]
fn test_delayed_tl_data_response_handle_does_not_consume_newer_ack() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind_with_payload_and_fcs(addr, 0, &[0xA0], false));
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    let (first_handle, first_payload) = first_msgs
        .iter()
        .find_map(tl_data_ind_handle_and_payload_bits)
        .expect("first BL-DATA should produce TL-DATA.ind");
    assert_ne!(first_handle, 0);
    assert_eq!(first_payload, "10100000");
    assert_eq!(
        first_msgs.iter().find_map(bl_ack_nr_and_payload_bits),
        Some((0, String::new())),
        "first BL-DATA should be acknowledged at MAC-ready tick_end"
    );

    test.submit_message(build_bl_data_ind_with_payload_and_fcs(addr, 1, &[0xB0], false));
    test.deliver_all_messages();
    let second_msgs = test.dump_sinks();
    let (second_handle, second_payload) = second_msgs
        .iter()
        .find_map(tl_data_ind_handle_and_payload_bits)
        .expect("second BL-DATA should produce TL-DATA.ind");
    assert_ne!(second_handle, first_handle);
    assert_eq!(second_payload, "10110000");

    // EN 300 392-2 clause 22.3.2.3(c) treats a response as a new acknowledged
    // transfer after its corresponding BL-ACK has already gone out. The stale
    // handle must not consume the newer BL-DATA's still-waiting acknowledgement
    // as an immediate BL-ACK response payload.
    test.submit_message(build_tl_data_resp_with_handle(addr, first_handle, &[0x33]));
    test.deliver_all_messages();
    let response_delivery_msgs = test.dump_sinks();
    assert!(
        !response_delivery_msgs
            .iter()
            .any(|msg| matches!(llc_pdu_type(msg), Some(LlcPduType::BlAck | LlcPduType::BlAckFcs))),
        "delayed response for the first indication must not consume the second indication's ACK"
    );

    test.run_stack(Some(1));
    let tick_msgs = test.dump_sinks();
    assert_eq!(
        tick_msgs.iter().filter_map(bl_adata_nr_ns).collect::<Vec<_>>(),
        vec![(1, 0)],
        "delayed response should be queued as a new sequenced transfer and may carry the second indication's ACK as BL-ADATA"
    );
    assert!(
        tick_msgs
            .iter()
            .all(|msg| !matches!(llc_pdu_type(msg), Some(LlcPduType::BlAck | LlcPduType::BlAckFcs))),
        "second indication's ACK should not be sent twice after BL-ADATA consumes it"
    );
}

#[test]
fn test_tl_data_response_consumes_pending_ack_for_matching_endpoint() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind_with_endpoint(addr, 1, 0));
    test.submit_message(build_bl_data_ind_with_endpoint(addr, 2, 1));
    test.deliver_all_messages();
    let data_ind_msgs = test.dump_sinks();
    let endpoint_2_handle = data_ind_msgs
        .iter()
        .find_map(|msg| match tl_data_ind_endpoint_handle_and_payload_bits(msg) {
            Some((2, handle, _)) => Some(handle),
            _ => None,
        })
        .expect("endpoint 2 incoming BL-DATA should produce TL-DATA.ind");
    assert_eq!(
        data_ind_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::TlaTlDataIndBl(prim) if prim.main_address == addr))
            .count(),
        2,
        "incoming BL-DATA on both endpoints should be delivered before responses are formed"
    );

    // EN 300 392-2 clause 22.3.2.3(b/c) combines a TL-DATA.response with the
    // corresponding waiting ACK. "Corresponding" is endpoint-scoped at the
    // TLA/TMA boundary; do not consume the first queued ACK for the same SSI.
    test.submit_message(build_tl_data_resp_with_endpoint_handle_and_fcs(
        addr,
        2,
        endpoint_2_handle,
        &[0xCC],
        false,
    ));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    let ack = sink_msgs
        .iter()
        .find_map(bl_ack_nr_and_payload_bits)
        .expect("endpoint 2 TL-DATA.resp should consume endpoint 2 pending ACK");
    assert_eq!(ack, (1, "11001100".to_owned()));
}

#[test]
fn test_outbound_bl_data_uses_endpoint_scoped_basic_links() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    // EN 300 392-2 clauses 22.3.1.1 and 23.1.2.5.2 make endpoint_id part
    // of the local basic-link context. Two MAC resources for the same ISSI
    // must not block each other or share V(S).
    test.submit_message(build_tl_data_req_with_endpoint_handle(addr, 1, 201));
    test.submit_message(build_tl_data_req_with_endpoint_handle(addr, 2, 202));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let mut submitted: Vec<(u32, u8)> = sink_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmaUnitdataReq(prim) if prim.main_address == addr => bl_data_ns(msg).map(|ns| (prim.endpoint_id, ns)),
            _ => None,
        })
        .collect();
    submitted.sort();

    assert_eq!(submitted, vec![(1, 0), (2, 0)]);
}

#[test]
fn test_bl_ack_matches_endpoint_scoped_expected_ack() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let service_reporter_ep1 = TxReporter::new();
    let service_reporter_ep2 = TxReporter::new();
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    let mut req_ep1 = build_tl_data_req_with_endpoint_handle(addr, 1, 201);
    attach_tl_data_req_reporter(&mut req_ep1, service_reporter_ep1.clone());
    let mut req_ep2 = build_tl_data_req_with_endpoint_handle(addr, 2, 202);
    attach_tl_data_req_reporter(&mut req_ep2, service_reporter_ep2.clone());
    test.submit_message(req_ep1);
    test.submit_message(req_ep2);
    test.run_stack(Some(1));
    let mut initial_msgs = test.dump_sinks();
    let reporter_ep1 = take_tma_req_reporter_for_endpoint(&mut initial_msgs, 1);
    let reporter_ep2 = take_tma_req_reporter_for_endpoint(&mut initial_msgs, 2);
    reporter_ep1.mark_transmitted();
    reporter_ep2.mark_transmitted();
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_bl_ack_ind_with_endpoint(addr, 2, 0));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let conf = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlaTlDataConfBl(prim) if prim.main_address == addr => Some(prim),
            _ => None,
        })
        .expect("endpoint 2 BL-ACK should confirm endpoint 2 TL-DATA.req");
    assert_eq!(conf.endpoint_id, 2);
    assert_eq!(conf.req_handle, 202);
    assert_eq!(reporter_ep1.get_state(), TxState::Transmitted);
    assert_eq!(reporter_ep2.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter_ep1.get_state(), TxState::Transmitted);
    assert_eq!(service_reporter_ep2.get_state(), TxState::Acknowledged);
}

#[test]
fn test_standalone_bl_ack_preserves_received_endpoint_and_handle() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind_with_endpoint(addr, 2, 1));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (ind_handle, _) = sink_msgs
        .iter()
        .find_map(tl_data_ind_handle_and_payload_bits)
        .expect("incoming BL-DATA should produce TL-DATA.ind with retained handle");

    let ack = sink_msgs.iter().find_map(|msg| match &msg.msg {
        SapMsgInner::TmaUnitdataReq(prim) if prim.main_address == addr && llc_pdu_type(msg) == Some(LlcPduType::BlAck) => {
            Some((prim.endpoint_id, prim.req_handle, prim.pdu_prio, prim.air_interface_encryption))
        }
        _ => None,
    });

    assert_eq!(
        ack,
        Some((2, ind_handle, 5, Some(0))),
        "standalone BL-ACK should use the retained context and ETSI BL-ACK PDU priority 5"
    );
}

#[test]
fn test_standalone_bl_ack_from_traffic_slot_uses_facch_channel_context() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    // The TMA indication is inferred as received two timeslots before the
    // current DL time, so DL TS4 corresponds to a received traffic-slot TS2.
    let dltime = TdmaTime { t: 4, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind_with_endpoint(addr, 7, 1));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (ind_handle, _) = sink_msgs
        .iter()
        .find_map(tl_data_ind_handle_and_payload_bits)
        .expect("incoming BL-DATA should produce TL-DATA.ind with retained handle");

    let ack = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TmaUnitdataReq(prim) if prim.main_address == addr && llc_pdu_type(msg) == Some(LlcPduType::BlAck) => Some(prim),
            _ => None,
        })
        .expect("traffic-slot BL-DATA should produce standalone BL-ACK");

    let chan_alloc = ack
        .chan_alloc
        .as_ref()
        .expect("traffic-slot BL-ACK must carry FACCH channel allocation");

    // EN 300 392-2 clauses 22.3.2.3(d), 22.3.1.1/22.3.1.2 and
    // 23.5/23.5.2.2.7: a standalone BL-ACK for a traffic-channel BL-DATA
    // keeps the received endpoint/handle and uses stealing/FACCH on that slot.
    assert_eq!(ack.endpoint_id, 7);
    assert_eq!(ack.req_handle, ind_handle);
    assert_eq!(ack.pdu_prio, 5);
    assert!(ack.stealing_permission);
    assert_eq!(ack.air_interface_encryption, Some(0));
    assert!(ack.tx_reporter.is_none());
    assert_eq!(chan_alloc.timeslots, [false, true, false, false]);
    assert_eq!(chan_alloc.alloc_type, ChanAllocType::Replace);
    assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Both);
    assert!(chan_alloc.usage.is_none());
    assert!(chan_alloc.carrier.is_none());
}

#[test]
fn test_tl_data_response_after_ack_is_sent_as_bl_data() {
    debug::setup_logging_verbose();

    let addr = TetraAddress::new(2065022, SsiType::Issi);
    let dltime = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);

    test.submit_message(build_bl_data_ind(addr, 0));
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();
    assert!(
        initial_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlAck)),
        "first tick should send standalone BL-ACK when no TL-DATA response/request is available"
    );

    // EN 300 392-2 clause 22.3.2.3(c): once the corresponding BL-ACK has
    // already been sent, a late TL-DATA response is transmitted as BL-DATA.
    let mut resp = build_tl_data_resp(addr, &[0x33]);
    let SapMsgInner::TlaTlDataRespBl(prim) = &mut resp.msg else {
        panic!("expected TL-DATA response");
    };
    prim.pdu_prio = 4;
    prim.stealing_permission = true;
    prim.stealing_repeats_flag = Some(true);
    prim.subscriber_class = 7;
    prim.data_class_info = Some(3);

    test.submit_message(resp);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let data_msg = sink_msgs
        .iter()
        .find(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlData))
        .expect("late TL-DATA.resp should be sent as BL-DATA");
    assert_eq!(bl_data_ns(data_msg), Some(0));
    let SapMsgInner::TmaUnitdataReq(prim) = &data_msg.msg else {
        panic!("expected TMA-UNITDATA request");
    };
    assert_eq!(
        prim.pdu_prio, 4,
        "late TL-DATA.resp fallback to BL-DATA should preserve TL PDU priority"
    );
    assert!(
        prim.stealing_permission,
        "late TL-DATA.resp fallback to BL-DATA should preserve stealing permission"
    );
    assert_eq!(prim.stealing_repeats_flag, Some(true));
    assert_eq!(prim.subscriber_class, 7);
    assert_eq!(prim.data_category, Some(3));
    assert!(prim.tx_reporter.is_some());
    assert!(
        !sink_msgs.iter().any(|msg| llc_pdu_type(msg) == Some(LlcPduType::BlAck)),
        "late TL-DATA.resp should not create a second BL-ACK"
    );
}
