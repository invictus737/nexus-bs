// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

mod common;

use tetra_config::bluestation::StackMode;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, PhyBlockNum, Sap, SsiType, TdmaTime, TetraAddress, TxReporter, debug};
use tetra_entities::umac::subcomp::fillbits;
use tetra_pdus::cmce::pdus::u_connect::UConnect;
use tetra_pdus::llc::pdus::bl_udata::BlUdata;
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;
use tetra_pdus::mle::pdus::d_mle_sync::DMleSync;
use tetra_pdus::umac::enums::sysinfo_opt_field_flag::SysinfoOptFieldFlag;
use tetra_pdus::umac::pdus::mac_access::MacAccess;
use tetra_pdus::umac::pdus::mac_end_dl::MacEndDl;
use tetra_pdus::umac::pdus::mac_resource::MacResource;
use tetra_pdus::umac::pdus::mac_sync::MacSync;
use tetra_pdus::umac::pdus::mac_sysinfo::MacSysinfo;
use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};
use tetra_saps::tlmb::TlmbSysinfoInd;
use tetra_saps::tma::{TmaReport, TmaUnitdataReq};
use tetra_saps::tmv::{TmvUnitdataInd, TmvUnitdataReq, enums::logical_chans::LogicalChannel};

use crate::common::ComponentTest;

const TEST_RSSI_DBFS: f32 = -42.0;
const TEST_LOCAL_ISSI: u32 = 1000001;
const TEST_CALL_ID: u16 = 0x234;
const SCH_HU_TYPE1_CAP_BITS: usize = 92;

fn build_sch_f_msg(bitstr: &str) -> SapMsg {
    SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            pdu: BitBuffer::from_bitstr(bitstr),
            block_num: PhyBlockNum::Both,
            logical_channel: LogicalChannel::SchF,
            crc_pass: true,
            scrambling_code: 0,
            rssi_dbfs: TEST_RSSI_DBFS,
        }),
    }
}

fn build_tmv_msg(logical_channel: LogicalChannel, bitstr: &str) -> SapMsg {
    let block_num = match logical_channel {
        LogicalChannel::SchF => PhyBlockNum::Both,
        _ => PhyBlockNum::Block1,
    };

    SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            pdu: BitBuffer::from_bitstr(bitstr),
            block_num,
            logical_channel,
            crc_pass: true,
            scrambling_code: 0,
            rssi_dbfs: TEST_RSSI_DBFS,
        }),
    }
}

fn build_u_connect_bl_udata() -> BitBuffer {
    let u_connect = UConnect {
        call_identifier: TEST_CALL_ID,
        hook_method_selection: false,
        simplex_duplex_selection: false,
        basic_service_information: None,
        facility: None,
        proprietary: None,
    };
    let mut cmce_sdu = BitBuffer::new_autoexpand(32);
    u_connect.to_bitbuf(&mut cmce_sdu).expect("failed to serialize U-CONNECT");
    cmce_sdu.seek(0);

    let mut tl_sdu = BitBuffer::new_autoexpand(40);
    tl_sdu.write_bits(MleProtocolDiscriminator::Cmce.into_raw(), 3);
    let cmce_sdu_len = cmce_sdu.get_len();
    tl_sdu.copy_bits(&mut cmce_sdu, cmce_sdu_len);
    tl_sdu.seek(0);

    let mut tm_sdu = BitBuffer::new_autoexpand(48);
    BlUdata { has_fcs: false }.to_bitbuf(&mut tm_sdu);
    let tl_sdu_len = tl_sdu.get_len();
    tm_sdu.copy_bits(&mut tl_sdu, tl_sdu_len);
    tm_sdu.seek(0);
    tm_sdu
}

fn build_tma_unitdata_req_to_ms_umac(pdu: BitBuffer) -> SapMsg {
    build_tma_unitdata_req_to_ms_umac_with_reporter(pdu, 77, None)
}

fn build_tma_unitdata_req_to_ms_umac_with_reporter(pdu: BitBuffer, req_handle: i32, tx_reporter: Option<TxReporter>) -> SapMsg {
    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle,
            pdu,
            main_address: TetraAddress::new(TEST_LOCAL_ISSI, SsiType::Issi),
            endpoint_id: 2,
            pdu_prio: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: None,
            tx_reporter,
        }),
    }
}

fn extract_tmv_unitdata_req(msgs: &[SapMsg]) -> &TmvUnitdataReq {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) => slot.blk1.as_ref(),
            _ => None,
        })
        .expect("expected TMV-UNITDATA request toward LMAC")
}

fn tma_report_for_handle(msgs: &[SapMsg], req_handle: i32) -> Option<TmaReport> {
    msgs.iter().find_map(|msg| match &msg.msg {
        SapMsgInner::TmaReportInd(report) if report.req_handle == req_handle => Some(report.report.clone()),
        _ => None,
    })
}

fn parse_mac_access_u_connect(prim: &TmvUnitdataReq) -> (MacAccess, BlUdata, UConnect) {
    let mut mac_block = BitBuffer::from_bitstr(&prim.mac_block.to_bitstr());
    let mac_access = MacAccess::from_bitbuf(&mut mac_block).expect("expected MAC-ACCESS");
    let fill_bits = if mac_access.fill_bits {
        fillbits::removal::get_num_fill_bits(&mac_block, SCH_HU_TYPE1_CAP_BITS, false)
    } else {
        0
    };
    mac_block.set_raw_end(mac_block.get_raw_start() + SCH_HU_TYPE1_CAP_BITS - fill_bits);

    let bl_udata = BlUdata::from_bitbuf(&mut mac_block).expect("expected BL-UDATA");
    let discriminator =
        MleProtocolDiscriminator::try_from(mac_block.read_bits(3).expect("expected MLE discriminator")).expect("valid MLE discriminator");
    assert_eq!(discriminator, MleProtocolDiscriminator::Cmce);
    let u_connect = UConnect::from_bitbuf(&mut mac_block).expect("expected U-CONNECT");

    (mac_access, bl_udata, u_connect)
}

#[test]
fn ms_tma_unitdata_req_emits_sch_hu_mac_access_for_small_unencrypted_cmce_pdu() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(build_tma_unitdata_req_to_ms_umac(build_u_connect_bl_udata()));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    let tmv = extract_tmv_unitdata_req(&sink_msgs);
    assert_eq!(tmv.logical_channel, LogicalChannel::SchHu);

    let (mac_access, bl_udata, u_connect) = parse_mac_access_u_connect(tmv);
    // EN 300 392-2 clauses 20.4.1.1.4, 21.4.2.1 and 23.5.2.4:
    // a small unencrypted C-plane TM-SDU may be sent by the MS in a SCH/HU
    // MAC-ACCESS random-access PDU when no reserved capacity is active.
    assert_eq!(mac_access.addr, Some(TetraAddress::new(TEST_LOCAL_ISSI, SsiType::Issi)));
    assert!(!mac_access.encrypted);
    assert!(mac_access.fill_bits);
    assert!(!bl_udata.has_fcs);
    assert_eq!(u_connect.call_identifier, TEST_CALL_ID);
}

#[test]
fn ms_tma_unitdata_req_reports_random_access_completion_to_llc() {
    debug::setup_logging_verbose();
    let req_handle = 901;
    let tx_reporter = TxReporter::new_unacked();
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);

    // EN 300 392-2 clauses 20.4.1.1.3 and 20.4.1.1.4 require MAC to report
    // progress for a TMA-UNITDATA request. This MS path emits the small TM-SDU
    // as SCH/HU MAC-ACCESS random access, so LLC must see SuccessRandomAccess
    // and the shared TxReporter must move to transmitted.
    test.submit_message(build_tma_unitdata_req_to_ms_umac_with_reporter(
        build_u_connect_bl_udata(),
        req_handle,
        Some(tx_reporter.clone()),
    ));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    let tmv = extract_tmv_unitdata_req(&sink_msgs);
    assert_eq!(tmv.logical_channel, LogicalChannel::SchHu);
    assert!(tx_reporter.is_transmitted());
    assert!(matches!(
        tma_report_for_handle(&sink_msgs, req_handle),
        Some(TmaReport::SuccessRandomAccess)
    ));
}

#[test]
/// A test containing a single Lmac frame with MAC-RESOURCE control-only contents.
fn mac_resource_without_llc_sdu_drops_without_llc_delivery() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    let mac_resource_bits =
        "0010001000110001011010110000101010001010000100000000110000010000100000000000000000000000000000000000000000000000000000000000";
    let mut parsed_resource = BitBuffer::from_bitstr(mac_resource_bits);
    let resource = MacResource::from_bitbuf(&mut parsed_resource).expect("test vector should contain MAC-RESOURCE");
    assert!(!resource.is_null_pdu());
    assert_eq!(resource.addr, Some(TetraAddress::new(7015050, SsiType::Ssi)));
    assert_eq!(resource.length_ind, 6);
    assert!(resource.random_access_flag);
    assert_eq!(resource.encryption_mode, 0);
    assert!(resource.power_control_element.is_none());
    assert!(resource.slot_granting_element.is_none());
    assert!(resource.chan_alloc_element.is_none());

    // EN 300 392-2 clause 21.4.3.1 permits MAC-RESOURCE without a TM-SDU for
    // control purposes such as random-access acknowledgement. Clause
    // 20.4.1.1.4 only delivers a TMA-UNITDATA.ind when a TM-SDU is present.
    let m = SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            pdu: BitBuffer::from_bitstr(mac_resource_bits),
            block_num: PhyBlockNum::Block1,
            logical_channel: LogicalChannel::SchHd,
            crc_pass: true,
            scrambling_code: 0,
            rssi_dbfs: TEST_RSSI_DBFS,
        }),
    };

    // Submit and process message
    test.submit_message(m);
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    // Evaluate results
    assert!(
        sink_msgs.is_empty(),
        "control-only MAC-RESOURCE must not synthesize LLC/TMA delivery"
    );
}

#[test]
fn mac_end_dl_reserved_length_zero_drops_without_panic() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    // EN 300 392-2 table 21.59 reserves MAC-END DL length indication 000000.
    // Corrupt air input must be dropped, not asserted in the receiver.
    test.submit_message(build_sch_f_msg("0110000000000"));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn mac_end_dl_oversized_length_with_fill_bits_drops_without_panic() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    // length_ind=111111 declares far more bits than this synthetic block
    // contains. UMAC must reject before fill-bit removal reads outside the
    // buffer window.
    test.submit_message(build_sch_f_msg("0111011111100"));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn mac_resource_oversized_length_drops_without_truncated_llc_delivery() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    let mut mac_resource = BitBuffer::new_autoexpand(64);
    MacResource {
        fill_bits: false,
        pos_of_grant: 0,
        encryption_mode: 0,
        random_access_flag: false,
        length_ind: 8,
        addr: Some(TetraAddress::new(TEST_LOCAL_ISSI, SsiType::Issi)),
        event_label: None,
        usage_marker: None,
        power_control_element: None,
        slot_granting_element: None,
        chan_alloc_element: None,
    }
    .to_bitbuf(&mut mac_resource);
    mac_resource.write_bits(0xaa, 8);

    // EN 300 392-2 clause 21.4.3.1 makes length_ind the MAC-RESOURCE PDU
    // length. Clauses 20.4.1.1.3 and 20.4.1.1.4 then rely on MAC not
    // delivering a partial TM-SDU upward as if random-access reception had
    // succeeded.
    test.submit_message(build_tmv_msg(LogicalChannel::SchF, &mac_resource.to_bitstr()));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn mac_u_signal_stch_drops_without_panic_until_u_plane_is_implemented() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    // EN 300 392-2 clause 21.4.5 fixes MAC-U-SIGNAL to STCH with a 121-bit
    // U-plane TM-SDU. This MS shim currently has no U-plane application, so it
    // must drop unsupported STCH signalling without panicking or delivering it
    // as clear C-plane data.
    let mac_u_signal = format!("110{}", "0".repeat(121));
    test.submit_message(build_tmv_msg(LogicalChannel::Stch, &mac_u_signal));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn supplementary_mac_pdu_schf_drops_without_panic_until_event_labels_are_implemented() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    // EN 300 392-2 clause 21.4.2.5 defines supplementary MAC-U-BLCK style
    // event-label signalling outside STCH. Without event-label mapping, the
    // MS must fail closed and avoid forwarding undecoded payload.
    let supplementary = format!("110{}", "0".repeat(265));
    test.submit_message(build_tmv_msg(LogicalChannel::SchF, &supplementary));
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
fn unexpected_tlmb_primitive_to_ms_umac_drops_without_panic() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Mle]);

    // EN 300 392-2 clauses 20.3.5.3.2 and 20.4.4: on an MS, TLMB/TMB
    // SYSINFO indications are generated by MAC and passed upward to MLE.
    // If a local router sends one back to UMAC, the MS stack must drop it
    // instead of panicking.
    test.submit_message(SapMsg {
        sap: Sap::TlmbSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TlmbSysinfoInd(TlmbSysinfoInd {
            endpoint_id: 0,
            tl_sdu: BitBuffer::new_autoexpand(8),
            mac_broadcast_info: None,
        }),
    });
    test.deliver_all_messages();

    assert!(test.dump_sinks().is_empty());
}

#[test]
/// A test containing a fragmented downlink message, which is reassembled by UMAC.
fn mac_resource_fragment_start_and_mac_end_reassembles_to_llc() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    let first_fragment_bits =
        "0000000111111001011010110000101001100011000000110100111101011010111110000100110000110000100100011000000000001100010101000000";
    let end_fragment_bits =
        "0111000100110000000000010011001000110000001101000010110000110001010000000000110000010000100000000000000000000000000000000000";

    let mut parsed_first = BitBuffer::from_bitstr(first_fragment_bits);
    let first_resource = MacResource::from_bitbuf(&mut parsed_first).expect("first vector should contain MAC-RESOURCE");
    assert_eq!(first_resource.addr, Some(TetraAddress::new(7015011, SsiType::Ssi)));
    assert_eq!(first_resource.length_ind, 0b111111);

    let mut parsed_end = BitBuffer::from_bitstr(end_fragment_bits);
    let end = MacEndDl::from_bitbuf(&mut parsed_end).expect("second vector should contain MAC-END");
    assert!(end.fill_bits);
    assert_eq!(end.length_ind, 9);

    test.submit_message(build_tmv_msg(LogicalChannel::SchHd, first_fragment_bits));
    test.deliver_all_messages();
    assert!(
        test.dump_sinks().is_empty(),
        "fragment start must be stored and not delivered before MAC-END"
    );

    test.submit_message(build_tmv_msg(LogicalChannel::SchHd, end_fragment_bits));
    test.deliver_all_messages();
    let msgs = test.dump_sinks();

    assert_eq!(msgs.len(), 1);
    let SapMsgInner::TmaUnitdataInd(prim) = &msgs[0].msg else {
        panic!("expected reconstructed TM-SDU toward LLC");
    };
    // EN 300 392-2 clause 21.4.3.1 marks the first MAC-RESOURCE with
    // length_ind=111111 as a fragmentation start; clause 23.4.3.1.1 requires
    // the MS-MAC to append MAC-END and deliver the reconstructed TM-SDU to LLC.
    assert_eq!(msgs[0].sap, Sap::TmaSap);
    assert_eq!(msgs[0].src, TetraEntity::Umac);
    assert_eq!(msgs[0].dest, TetraEntity::Llc);
    assert_eq!(prim.main_address, TetraAddress::new(7015011, SsiType::Ssi));
    assert_eq!(
        prim.pdu.as_ref().expect("expected reconstructed TM-SDU").to_bitstr(),
        "00011010011110101101011111000010011000011000010010001100000000000110001010100000000100110010001100000011010000101100001100010"
    );
}

#[test]
/// A test containing a SYSINFO frame, parsed by UMAC and MLE
fn test_sysinfo() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Mle]);

    let sysinfo_bits =
        "1000010000111111010001000000100001101001111100000000000000011101000011100000000000000000000000101111111111100101110101110111";
    let mut parsed_sysinfo = BitBuffer::from_bitstr(sysinfo_bits);
    let sysinfo = MacSysinfo::from_bitbuf(&mut parsed_sysinfo).expect("test vector should contain MAC-SYSINFO");
    assert_eq!(sysinfo.main_carrier, 1087);
    assert_eq!(sysinfo.freq_band, 4);
    assert_eq!(sysinfo.freq_offset_index, 1);
    assert_eq!(sysinfo.duplex_spacing, 0);
    assert!(!sysinfo.reverse_operation);
    assert_eq!(sysinfo.ms_txpwr_max_cell, 4);
    assert_eq!(sysinfo.rxlev_access_min, 3);
    assert_eq!(sysinfo.access_parameter, 4);
    assert_eq!(sysinfo.radio_dl_timeout, 15);
    assert_eq!(sysinfo.cck_id, Some(1));
    assert_eq!(sysinfo.option_field, SysinfoOptFieldFlag::ExtServicesBroadcast);

    let m = SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            pdu: BitBuffer::from_bitstr(sysinfo_bits),
            block_num: PhyBlockNum::Block2,
            logical_channel: LogicalChannel::Bnch,
            crc_pass: true,
            scrambling_code: 0,
            rssi_dbfs: TEST_RSSI_DBFS,
        }),
    };
    test.submit_message(m);
    test.deliver_all_messages();
    let msgs = test.dump_sinks();

    assert_eq!(msgs.len(), 1);
    let SapMsgInner::TlmbSysinfoInd(prim) = &msgs[0].msg else {
        panic!("expected TLMB-SYSINFO.ind toward MLE");
    };
    // EN 300 392-2 clauses 21.4.4.1 and 23.7.2: after decoding MAC-SYSINFO,
    // MS-MAC passes the remaining broadcast TL-SDU to MLE over TLMB-SAP.
    assert_eq!(msgs[0].sap, Sap::TlmbSap);
    assert_eq!(msgs[0].src, TetraEntity::Umac);
    assert_eq!(msgs[0].dest, TetraEntity::Mle);
    assert_eq!(prim.endpoint_id, 0);
    assert!(prim.mac_broadcast_info.is_none());
    assert_eq!(prim.tl_sdu.to_bitstr(), "000000000000101111111111100101110101110111");
}

#[test]
/// A test containing a SYNC frame, parsed by UMAC and MLE
fn test_sync() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Mle]);

    // SB1 09/11/4/000 type1: 000100000111010110010010000000001101001000000100010101110011
    // TMB-SAP SYNC CC 000001(0x01) TN 11(4) FN 01011(11) MN 001001( 9) MCC 0110100100(420) MNC 00001000101011(555)
    let sync_bits = "000100000111010110010010000000001101001000000100010101110011";
    let mut parsed_sync = BitBuffer::from_bitstr(sync_bits);
    let mac_sync = MacSync::from_bitbuf(&mut parsed_sync).expect("test vector should contain MAC-SYNC");
    assert_eq!(mac_sync.system_code, 1);
    assert_eq!(mac_sync.colour_code, 1);
    assert_eq!(mac_sync.time, TdmaTime { t: 4, f: 11, m: 9, h: 0 });
    assert_eq!(mac_sync.sharing_mode, 0);
    assert_eq!(mac_sync.ts_reserved_frames, 0);
    assert!(!mac_sync.u_plane_dtx);
    assert!(!mac_sync.frame_18_ext);
    let dle_sync = DMleSync::from_bitbuf(&mut parsed_sync).expect("test vector should contain D-MLE-SYNC");
    assert_eq!(dle_sync.mcc, 420);
    assert_eq!(dle_sync.mnc, 555);
    assert_eq!(dle_sync.neighbor_cell_broadcast, 2);
    assert_eq!(dle_sync.cell_load_ca, 1);
    assert!(dle_sync.late_entry_supported);

    let m = SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            pdu: BitBuffer::from_bitstr(sync_bits),
            block_num: PhyBlockNum::Block1,
            logical_channel: LogicalChannel::Bsch,
            crc_pass: true,
            scrambling_code: 0,
            rssi_dbfs: TEST_RSSI_DBFS,
        }),
    };
    test.submit_message(m);
    test.deliver_all_messages();
    let msgs = test.dump_sinks();

    let lmac_config = msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TmvConfigureReq(prim) if msg.dest == TetraEntity::Lmac => Some((msg, prim)),
            _ => None,
        })
        .expect("MAC-SYNC should configure LMAC time");
    // EN 300 392-2 clauses 21.4.4.2 and 23.7.2: after decoding MAC-SYNC,
    // MS-MAC updates lower MAC timing and passes the associated D-MLE-SYNC
    // TL-SDU to MLE.
    assert_eq!(lmac_config.0.sap, Sap::TmvSap);
    assert_eq!(lmac_config.0.src, TetraEntity::Umac);
    assert_eq!(lmac_config.1.time, Some(TdmaTime { t: 4, f: 11, m: 9, h: 0 }));

    let mle_sync = msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TlmbSyncInd(prim) if msg.dest == TetraEntity::Mle => Some((msg, prim)),
            _ => None,
        })
        .expect("MAC-SYNC should deliver D-MLE-SYNC to MLE");
    assert_eq!(mle_sync.0.sap, Sap::TlmbSap);
    assert_eq!(mle_sync.0.src, TetraEntity::Umac);
    assert_eq!(mle_sync.1.endpoint_id, 0);
    assert_eq!(mle_sync.1.tl_sdu.to_bitstr(), "01101001000000100010101110011");
}

#[test]
fn mac_resource_full_slot_delivers_tmsdu_to_llc() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    let mac_resource_bits = "0010000010001110000000000000000001100101110110001000100110001001010001101100100100011110001110010011000000000001001100111110000000001000000000000001000010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
    let expected_tm_sdu = "0010010001111000111001001100000000000100110011111000000000";

    let mut parsed_resource = BitBuffer::from_bitstr(mac_resource_bits);
    let resource = MacResource::from_bitbuf(&mut parsed_resource).expect("test vector should contain MAC-RESOURCE");
    assert_eq!(resource.addr, Some(TetraAddress::new(101, SsiType::Ssi)));
    assert_eq!(resource.usage_marker, Some(54));
    assert_eq!(resource.length_ind, 17);
    assert!(resource.fill_bits);
    let chan_alloc = resource
        .chan_alloc_element
        .as_ref()
        .expect("test vector should contain channel allocation");
    assert_eq!(chan_alloc.alloc_type, ChanAllocType::Replace);
    assert_eq!(chan_alloc.ts_assigned, [false, true, false, false]);
    assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Both);
    assert_eq!(chan_alloc.carrier_num, 1187);
    assert_eq!(chan_alloc.mon_pattern, 3);

    let mut parsed_tm_sdu = BitBuffer::from_bitstr(expected_tm_sdu);
    let bl_udata = BlUdata::from_bitbuf(&mut parsed_tm_sdu).expect("expected BL-UDATA TM-SDU");
    assert!(!bl_udata.has_fcs);
    let discriminator = MleProtocolDiscriminator::try_from(parsed_tm_sdu.read_bits(3).expect("expected MLE discriminator"))
        .expect("valid MLE discriminator");
    assert_eq!(discriminator, MleProtocolDiscriminator::Cmce);

    test.submit_message(build_tmv_msg(LogicalChannel::SchF, mac_resource_bits));
    test.deliver_all_messages();
    let msgs = test.dump_sinks();

    assert_eq!(msgs.len(), 1);
    let SapMsgInner::TmaUnitdataInd(prim) = &msgs[0].msg else {
        panic!("expected TM-SDU toward LLC");
    };
    // EN 300 392-2 clause 21.4.3.1 defines MAC-RESOURCE on SCH/F and clause
    // 23.4.3.1.1 requires non-fragmented TM-SDU delivery to LLC when one of
    // the MS valid addresses is present.
    assert_eq!(msgs[0].sap, Sap::TmaSap);
    assert_eq!(msgs[0].src, TetraEntity::Umac);
    assert_eq!(msgs[0].dest, TetraEntity::Llc);
    assert_eq!(prim.main_address, TetraAddress::new(101, SsiType::Ssi));
    assert_eq!(prim.pdu.as_ref().expect("expected TM-SDU").to_bitstr(), expected_tm_sdu);
}
