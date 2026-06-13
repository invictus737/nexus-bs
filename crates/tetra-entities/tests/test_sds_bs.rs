// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

mod common;

use std::time::Duration;

use tetra_config::bluestation::{
    CfgBrew, CfgHomeModeDisplay, CfgSdsCommandControl, CfgSdsCommandEntry, HomeModeDisplaySdsTextCodingScheme, LIVE_SDS_QUEUE_MAX_LEN,
    SharedConfig, StackMode,
};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Layer2Service, Sap, SsiType, TdmaTime, TetraAddress, TxReporter, TxState, debug};
use tetra_entities::cmce::cmce_bs::CmceBs;
use tetra_entities::cmce::subentities::sds_bs::{SdsBsSubentity, SdsPendingAction};
use tetra_entities::net_control::commands::{
    WAP_MVP_COLOR_PAGE_TEXT, WAP_MVP_MESSAGE_TEXT, WAP_MVP_PAGE_TEXT, WAP_SDS_TL_PROTOCOL_ID, WAP_SDS_TL_TRANSFER_FLAGS_NO_REPORT,
    WAP_SDS_TYPE4_MAX_BYTE_ALIGNED_PAYLOAD_BYTES, WAP_WDP_PROTOCOL_ID, wap_sds_tl_transfer_type4_payload, wap_sds_type4_payload,
};
use tetra_entities::net_control::{ControlCommand, ControlResponse, make_control_link};
use tetra_entities::{MessageQueue, TetraEntityTrait};
use tetra_pdus::cmce::enums::cmce_pdu_type_ul::CmcePduTypeUl;
use tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier;
use tetra_pdus::cmce::enums::pre_coded_status::PreCodedStatus;
use tetra_pdus::cmce::pdus::cmce_function_not_supported::CmceFunctionNotSupported;
use tetra_pdus::cmce::pdus::d_sds_data::DSdsData;
use tetra_pdus::cmce::pdus::d_status::DStatus;
use tetra_pdus::cmce::pdus::u_sds_data::USdsData;
use tetra_pdus::cmce::pdus::u_status::UStatus;
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;
use tetra_saps::control::enums::sds_user_data::SdsUserData;
use tetra_saps::control::sds::{CmceSdsData, CmceSdsStatus};
use tetra_saps::lcmc::LcmcMleUnitdataInd;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};

use crate::common::ComponentTest;

const LARGE_LOCAL_GROUP_MEMBER_COUNT: u32 = 1024;

/// Helper: register a subscriber ISSI in the StackState subscriber registry
fn register_subscriber(test: &mut ComponentTest, issi: u32) {
    test.config.state_write().subscribers.register(issi);
}

/// Helper: affiliate a subscriber with a GSSI in the StackState subscriber registry
fn affiliate_subscriber(test: &mut ComponentTest, issi: u32, gssi: u32) {
    test.config.state_write().subscribers.affiliate(issi, gssi);
}

fn register_affiliated_group_members(test: &mut ComponentTest, first_issi: u32, count: u32, gssi: u32) {
    let mut state = test.config.state_write();
    for offset in 0..count {
        let issi = first_issi + offset;
        state.subscribers.register(issi);
        assert!(state.subscribers.affiliate(issi, gssi));
    }
    assert_eq!(state.subscribers.group_members(gssi).len(), count as usize);
}

fn register_shared_subscriber(config: &SharedConfig, issi: u32) {
    config.state_write().subscribers.register(issi);
}

fn affiliate_shared_subscriber(config: &SharedConfig, issi: u32, gssi: u32) {
    config.state_write().subscribers.affiliate(issi, gssi);
}

fn local_mni(config: &SharedConfig) -> u64 {
    let config = config.config();
    ((config.net.mcc as u64) << 14) | config.net.mnc as u64
}

/// Helper: build a U-SDS-DATA message from a source ISSI to a dest SSI with 16-bit payload
fn build_u_sds_data_msg(source_issi: u32, dest_ssi: u32, payload: u16) -> SapMsg {
    let u_sds = USdsData {
        area_selection: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_short_number_address: None,
        called_party_ssi: Some(dest_ssi as u64),
        called_party_extension: None,
        user_defined_data: SdsUserData::Type1(payload),
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_sds.to_bitbuf(&mut sdu).expect("Failed to serialize U-SDS-DATA");
    sdu.seek(0);

    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(source_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

/// Helper: build a U-STATUS message from a source ISSI to a dest SSI.
fn build_u_status_msg(source_issi: u32, dest_ssi: u32, pre_coded_status: PreCodedStatus) -> SapMsg {
    let u_status = UStatus {
        area_selection: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_short_number_address: None,
        called_party_ssi: Some(dest_ssi as u64),
        called_party_extension: None,
        pre_coded_status,
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_status.to_bitbuf(&mut sdu).expect("Failed to serialize U-STATUS");
    sdu.seek(0);

    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(source_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn with_received_tetra_address(mut msg: SapMsg, address: TetraAddress) -> SapMsg {
    let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut msg.msg else {
        panic!("expected LCMC-MLE-UNITDATA indication");
    };
    prim.received_tetra_address = address;
    msg
}

/// Helper: build a U-SDS-DATA message using TSI destination addressing.
fn build_u_sds_data_tsi_msg(source_issi: u32, dest_ssi: u32, dest_extension: u64, payload: u16) -> SapMsg {
    let u_sds = USdsData {
        area_selection: 0,
        called_party_type_identifier: PartyTypeIdentifier::Tsi,
        called_party_short_number_address: None,
        called_party_ssi: Some(dest_ssi as u64),
        called_party_extension: Some(dest_extension),
        user_defined_data: SdsUserData::Type1(payload),
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(96);
    u_sds.to_bitbuf(&mut sdu).expect("Failed to serialize U-SDS-DATA TSI");
    sdu.seek(0);

    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(source_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

/// Helper: build a U-SDS-DATA message using short-number destination addressing.
fn build_u_sds_data_short_number_msg(source_issi: u32, short_number: u64, payload: u16) -> SapMsg {
    let u_sds = USdsData {
        area_selection: 0,
        called_party_type_identifier: PartyTypeIdentifier::Sna,
        called_party_short_number_address: Some(short_number),
        called_party_ssi: None,
        called_party_extension: None,
        user_defined_data: SdsUserData::Type1(payload),
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_sds.to_bitbuf(&mut sdu).expect("Failed to serialize U-SDS-DATA SNA");
    sdu.seek(0);

    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(source_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

/// Helper: build a U-STATUS message using TSI destination addressing.
fn build_u_status_tsi_msg(source_issi: u32, dest_ssi: u32, dest_extension: u64, pre_coded_status: PreCodedStatus) -> SapMsg {
    let u_status = UStatus {
        area_selection: 0,
        called_party_type_identifier: PartyTypeIdentifier::Tsi,
        called_party_short_number_address: None,
        called_party_ssi: Some(dest_ssi as u64),
        called_party_extension: Some(dest_extension),
        pre_coded_status,
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(96);
    u_status.to_bitbuf(&mut sdu).expect("Failed to serialize U-STATUS TSI");
    sdu.seek(0);

    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(source_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

/// Helper: build a U-STATUS message using short-number destination addressing.
fn build_u_status_short_number_msg(source_issi: u32, short_number: u64, pre_coded_status: PreCodedStatus) -> SapMsg {
    let u_status = UStatus {
        area_selection: 0,
        called_party_type_identifier: PartyTypeIdentifier::Sna,
        called_party_short_number_address: Some(short_number),
        called_party_ssi: None,
        called_party_extension: None,
        pre_coded_status,
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_status.to_bitbuf(&mut sdu).expect("Failed to serialize U-STATUS SNA");
    sdu.seek(0);

    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(source_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

/// Count D-SDS-DATA messages (LcmcMleUnitdataReq to Mle) in sink output
fn count_d_sds_data(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|m| m.dest == TetraEntity::Mle && matches!(&m.msg, SapMsgInner::LcmcMleUnitdataReq(_)))
        .count()
}

/// Count CmceSdsData messages to Brew in sink output
fn count_brew_sds(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|m| m.dest == TetraEntity::Brew && matches!(&m.msg, SapMsgInner::CmceSdsData(_)))
        .count()
}

fn brew_sds_enabled_config() -> tetra_config::bluestation::StackConfig {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(CfgBrew {
        host: "test.local".into(),
        port: 3000,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        feature_rssi_export: false,
        whitelisted_ssis: None,
    });
    config
}

fn assert_u_status_sds_tl_short_report_to_brew(raw_status: u16, message_reference: u8, delivery_status: u8) {
    assert_u_status_sds_tl_short_report_to_brew_with_pid(0x82, raw_status, message_reference, delivery_status);
}

fn assert_u_status_sds_tl_short_report_to_brew_with_pid(protocol_id: u8, raw_status: u16, message_reference: u8, delivery_status: u8) {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(brew_sds_enabled_config(), Some(dltime));
    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);
    register_subscriber(&mut test, 1000001);

    // EN 300 392-2 clauses 29.4.1 and 29.4.2.4: SDS-TL Type4 carries the
    // protocol identifier and message reference in the original transfer.
    // The later SDS-SHORT REPORT repeats only the message reference, so the BS
    // must remember the PID from the downlink transfer it sent.
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 5000001,
            dest_issi: 1000001,
            dest_ssi_type: None,
            user_defined_data: SdsUserData::Type4(40, vec![protocol_id, 0x04, message_reference, 0x01, b'A']),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    assert_eq!(count_d_sds_data(&setup_msgs), 1, "expected setup D-SDS-DATA to local MS");

    let msg = build_u_status_msg(1000001, 5000001, PreCodedStatus::from(raw_status));
    test.submit_message(msg);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let brew_msg = sink_msgs
        .iter()
        .find_map(|m| match &m.msg {
            SapMsgInner::CmceSdsData(sds) if m.dest == TetraEntity::Brew => Some(sds),
            _ => None,
        })
        .expect("expected SDS short report to be forwarded to Brew");
    assert_eq!(brew_msg.source_issi, 1000001);
    assert_eq!(brew_msg.dest_issi, 5000001);
    assert_eq!(
        brew_msg.user_defined_data,
        SdsUserData::Type4(32, vec![protocol_id, 0x10, delivery_status, message_reference])
    );
    assert_eq!(
        count_d_sds_data(&sink_msgs),
        0,
        "non-local status report should not be delivered on RF"
    );
}

fn assert_u_status_sds_tl_short_report_without_context_to_brew(raw_status: u16) {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(brew_sds_enabled_config(), Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, 1000001);

    test.submit_message(build_u_status_msg(1000001, 5000001, PreCodedStatus::from(raw_status)));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let brew_msg = sink_msgs
        .iter()
        .find_map(|m| match &m.msg {
            SapMsgInner::CmceSdsData(sds) if m.dest == TetraEntity::Brew => Some(sds),
            _ => None,
        })
        .expect("expected SDS short report to be forwarded to Brew");
    assert_eq!(
        brew_msg.user_defined_data,
        SdsUserData::Type1(raw_status),
        "without a prior SDS-TL transfer context, the BS must not fabricate a Type4 report with guessed PID"
    );
}

fn assert_u_status_sds_tl_short_report_ignores_non_sds_tl_pid_context(protocol_id: u8) {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(brew_sds_enabled_config(), Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, 1000001);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 5000001,
            dest_issi: 1000001,
            dest_ssi_type: None,
            user_defined_data: SdsUserData::Type4(40, vec![protocol_id, 0x04, 0x44, 0x01, b'A']),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    assert_eq!(count_d_sds_data(&setup_msgs), 1, "expected setup D-SDS-DATA to local MS");

    test.submit_message(build_u_status_msg(1000001, 5000001, PreCodedStatus::from(0x7E44)));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let brew_msg = sink_msgs
        .iter()
        .find_map(|m| match &m.msg {
            SapMsgInner::CmceSdsData(sds) if m.dest == TetraEntity::Brew => Some(sds),
            _ => None,
        })
        .expect("expected SDS short report to be forwarded to Brew");
    assert_eq!(
        brew_msg.user_defined_data,
        SdsUserData::Type1(0x7E44),
        "PID 0x{protocol_id:02X} must not create SDS-TL report context"
    );
}

fn extract_d_sds_data(msgs: &[SapMsg]) -> (&tetra_saps::lcmc::LcmcMleUnitdataReq, DSdsData) {
    msgs.iter()
        .find_map(|m| match &m.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => {
                let mut sdu = prim.sdu.clone();
                DSdsData::from_bitbuf(&mut sdu).ok().map(|pdu| (prim, pdu))
            }
            _ => None,
        })
        .expect("expected D-SDS-DATA")
}

fn extract_d_status(msgs: &[SapMsg]) -> (&tetra_saps::lcmc::LcmcMleUnitdataReq, DStatus) {
    msgs.iter()
        .find_map(|m| match &m.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => {
                let mut sdu = prim.sdu.clone();
                DStatus::from_bitbuf(&mut sdu).ok().map(|pdu| (prim, pdu))
            }
            _ => None,
        })
        .expect("expected D-STATUS")
}

fn count_d_sds_pdus(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => {
                let mut sdu = prim.sdu.clone();
                DSdsData::from_bitbuf(&mut sdu).ok()
            }
            _ => None,
        })
        .count()
}

fn count_d_status_pdus(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => {
                let mut sdu = prim.sdu.clone();
                DStatus::from_bitbuf(&mut sdu).ok()
            }
            _ => None,
        })
        .count()
}

fn extract_cmce_function_not_supported(msgs: &[SapMsg]) -> (&tetra_saps::lcmc::LcmcMleUnitdataReq, CmceFunctionNotSupported) {
    msgs.iter()
        .find_map(|m| match &m.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => {
                let mut sdu = prim.sdu.clone();
                CmceFunctionNotSupported::from_bitbuf(&mut sdu).ok().map(|pdu| (prim, pdu))
            }
            _ => None,
        })
        .expect("expected CMCE FUNCTION NOT SUPPORTED")
}

fn assert_sds_cmce_function_not_supported(msgs: &[SapMsg], target_issi: u32, pdu_type: CmcePduTypeUl) {
    let (prim, pdu) = extract_cmce_function_not_supported(msgs);
    assert_eq!(pdu.not_supported_pdu_type, pdu_type.into_raw() as u8);
    assert!(!pdu.call_identifier_present);
    assert_eq!(pdu.call_identifier, None);
    assert_eq!(pdu.function_not_supported_pointer, 0);
    assert_eq!(pdu.length_of_received_pdu_extract, None);
    assert!(pdu.received_pdu_extract.is_none());
    assert_eq!(prim.main_address.ssi, target_issi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert!(prim.chan_alloc.is_none());
}

fn extract_tla_d_status(sdu: &BitBuffer) -> DStatus {
    let mut sdu = sdu.clone();
    assert_eq!(sdu.read_bits(3), Some(MleProtocolDiscriminator::Cmce.into_raw()));
    DStatus::from_bitbuf(&mut sdu).expect("expected CMCE D-STATUS after MLE discriminator")
}

fn extract_tla_d_sds_data(sdu: &BitBuffer) -> DSdsData {
    let mut sdu = sdu.clone();
    assert_eq!(sdu.read_bits(3), Some(MleProtocolDiscriminator::Cmce.into_raw()));
    DSdsData::from_bitbuf(&mut sdu).expect("expected CMCE D-SDS-DATA after MLE discriminator")
}

fn broadcast_cfg(text: &str, protocol_id: u8) -> CfgHomeModeDisplay {
    CfgHomeModeDisplay {
        source_issi: 0x00FF_FFFF,
        interval_multiframes: 1,
        protocol_id,
        text_coding_scheme: HomeModeDisplaySdsTextCodingScheme::LATIN,
        text: text.into(),
    }
}

fn drain_queue(queue: &mut MessageQueue) -> Vec<SapMsg> {
    let mut msgs = Vec::new();
    while let Some(msg) = queue.pop_front() {
        msgs.push(msg);
    }
    msgs
}

#[test]
fn test_sds_local_delivery() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(brew_sds_enabled_config(), Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Register source and dest ISSIs in StackState
    register_subscriber(&mut test, 1000001);
    register_subscriber(&mut test, 2000001);

    // Send U-SDS-DATA from source ISSI to registered dest ISSI
    let msg = build_u_sds_data_msg(1000001, 2000001, 0xABCD);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let d_sds_count = count_d_sds_data(&sink_msgs);
    assert_eq!(d_sds_count, 1, "Expected 1 D-SDS-DATA at Mle sink for local delivery");

    // Verify the address is ISSI
    for m in &sink_msgs {
        if m.dest == TetraEntity::Mle {
            if let SapMsgInner::LcmcMleUnitdataReq(ref prim) = m.msg {
                assert_eq!(prim.main_address.ssi, 2000001);
                assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
            }
        }
    }
}

#[test]
fn test_u_sds_to_local_issi_uses_acknowledged_l2_and_preserves_fields() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let dest_issi = 2000001;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(brew_sds_enabled_config(), Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);
    register_subscriber(&mut test, dest_issi);

    test.submit_message(build_u_sds_data_msg(source_issi, dest_issi, 0xABCD));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (prim, pdu) = extract_d_sds_data(&sink_msgs);

    // EN 300 392-2 clause 18.3.5.3.1 maps acknowledged-request L2 service to a
    // TL-DATA request. Local ISSI SDS delivery should therefore keep
    // acknowledged basic-link transfer and preserve the D-SDS-DATA fields.
    assert_eq!(prim.main_address.ssi, dest_issi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert_eq!(pdu.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(pdu.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(pdu.calling_party_extension, None);
    assert!(matches!(pdu.user_defined_data, SdsUserData::Type1(0xABCD)));
}

#[test]
fn test_u_sds_to_registered_9999_is_not_absorbed_by_dashboard_shortcut() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let dest_issi = 9999;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);
    register_subscriber(&mut test, dest_issi);

    test.submit_message(build_u_sds_data_msg(source_issi, dest_issi, 0xABCD));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (prim, pdu) = extract_d_sds_data(&sink_msgs);

    // ISSI 9999 is a local dashboard/control convention in this stack, not an
    // ETSI reserved air-interface address. EN 300 392-2 SSI-addressed SDS
    // routing must still deliver it when registered as a normal ISSI.
    assert_eq!(prim.main_address.ssi, dest_issi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert_eq!(pdu.calling_party_address_ssi, Some(source_issi as u64));
    assert!(matches!(pdu.user_defined_data, SdsUserData::Type1(0xABCD)));
}

#[test]
fn test_home_mode_display_default_source_issi_is_all_ones_on_air() {
    debug::setup_logging_verbose();

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.home_mode_display = Some(broadcast_cfg("HMD", 0xDC));
    config.cell.sds_broadcast = None;
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    sds.tick_start(&mut queue, TdmaTime::default());
    assert!(queue.pop_front().is_none());
    sds.tick_start(&mut queue, TdmaTime::default().add_timeslots(96 * 4));

    let msgs = drain_queue(&mut queue);
    let (prim, pdu) = extract_d_sds_data(&msgs);

    // EN 300 392-2 29.3.3.8.2 says SwMI SDS-TL system broadcast
    // messages may use broadcast destination 0xFFFFFF and should use
    // broadcast source 0xFFFFFF so MSs can recognize system broadcasts.
    assert_eq!(prim.main_address.ssi, 0x00FF_FFFF);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(pdu.calling_party_address_ssi, Some(0x00FF_FFFF));
    assert_eq!(pdu.calling_party_extension, None);
    let SdsUserData::Type4(len_bits, data) = pdu.user_defined_data else {
        panic!("home-mode display should use SDS-TL Type4");
    };
    assert_eq!(len_bits as usize, data.len() * 8);
    assert_eq!(&data[..4], &[0xDC, 0x00, 0x00, 0x01]);
    assert_eq!(&data[4..], b"HMD");
}

#[test]
fn test_home_mode_display_configured_source_issi_is_normalized_for_system_broadcast() {
    debug::setup_logging_verbose();

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    let mut hmd = broadcast_cfg("HMD", 0xDC);
    hmd.source_issi = 5000001;
    config.cell.home_mode_display = Some(hmd);
    config.cell.sds_broadcast = None;
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    sds.tick_start(&mut queue, TdmaTime::default());
    assert!(queue.pop_front().is_none());
    sds.tick_start(&mut queue, TdmaTime::default().add_timeslots(96 * 4));

    let msgs = drain_queue(&mut queue);
    let (prim, pdu) = extract_d_sds_data(&msgs);

    // EN 300 392-2 clause 29.3.3.8.2 recommends all-ones as the
    // source address for SwMI system broadcast, independent of local
    // configuration identity.
    assert_eq!(prim.main_address.ssi, 0x00FF_FFFF);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_address_ssi, Some(0x00FF_FFFF));
    let SdsUserData::Type4(_, data) = pdu.user_defined_data else {
        panic!("home-mode display should use SDS-TL Type4");
    };
    assert_eq!(&data[4..], b"HMD");
}

#[test]
fn test_home_mode_display_utf16_truncation_preserves_complete_code_units() {
    debug::setup_logging_verbose();

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    let mut hmd = broadcast_cfg(&format!("{}😀B", "A".repeat(124)), 0xDC);
    hmd.text_coding_scheme = HomeModeDisplaySdsTextCodingScheme::UTF16;
    config.cell.home_mode_display = Some(hmd);
    config.cell.sds_broadcast = None;
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    sds.tick_start(&mut queue, TdmaTime::default());
    assert!(queue.pop_front().is_none());
    sds.tick_start(&mut queue, TdmaTime::default().add_timeslots(96 * 4));

    let msgs = drain_queue(&mut queue);
    let (_, pdu) = extract_d_sds_data(&msgs);
    let SdsUserData::Type4(len_bits, data) = pdu.user_defined_data else {
        panic!("home-mode display should use SDS-TL Type4");
    };
    assert_eq!(len_bits as usize, data.len() * 8);
    assert_eq!(&data[..4], &[0xDC, 0x00, 0x00, 0x1A]);

    let text = &data[4..];
    // EN 300 392-2 table 29.29 and clause 29.5.4.1 define coding scheme
    // 0x1A as UTF-16BE. Truncation must not leave an odd byte or the high
    // half of a surrogate pair at the end of the Type 4 user data.
    assert_eq!(text.len(), 248);
    assert_eq!(text.len() % 2, 0);
    let last_unit = u16::from_be_bytes([text[text.len() - 2], text[text.len() - 1]]);
    assert!(!(0xD800..=0xDBFF).contains(&last_unit));
    assert!(text.chunks_exact(2).all(|unit| unit == [0x00, b'A']));
}

#[test]
fn test_sds_broadcast_default_source_issi_is_all_ones_on_air() {
    debug::setup_logging_verbose();

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.home_mode_display = None;
    config.cell.sds_broadcast = Some(broadcast_cfg("SYS", 0x82));
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    sds.tick_start(&mut queue, TdmaTime::default());
    assert!(queue.pop_front().is_none());
    sds.tick_start(&mut queue, TdmaTime::default().add_timeslots(96 * 4));

    let msgs = drain_queue(&mut queue);
    let (prim, pdu) = extract_d_sds_data(&msgs);

    assert_eq!(prim.main_address.ssi, 0x00FF_FFFF);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_address_ssi, Some(0x00FF_FFFF));
    let SdsUserData::Type4(len_bits, data) = pdu.user_defined_data else {
        panic!("periodic SDS broadcast should use SDS-TL Type4");
    };
    assert_eq!(len_bits as usize, data.len() * 8);
    assert_eq!(&data[..4], &[0x82, 0x00, 0x00, 0x01]);
    assert_eq!(&data[4..], b"SYS");
}

#[test]
fn test_live_sds_control_broadcast_uses_all_ones_source_on_air() {
    debug::setup_logging_verbose();

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.home_mode_display = None;
    config.cell.sds_broadcast = None;
    let shared_config = SharedConfig::from_parts(config, None);
    let (dispatcher, endpoint) = make_control_link();
    let mut cmce = CmceBs::new(shared_config.clone(), None, Some(endpoint));
    let mut queue = MessageQueue::new();

    dispatcher.send(ControlCommand::AddLiveSds {
        text: "LIVE".to_string(),
        protocol_id: 0xDC,
        source_issi: 5000001,
        repeat_count: 1,
    });

    cmce.tick_start(&mut queue, TdmaTime::default());
    assert!(queue.pop_front().is_none());
    cmce.tick_start(&mut queue, TdmaTime::default().add_timeslots(96 * 4 * 2));

    let msgs = drain_queue(&mut queue);
    let (prim, pdu) = extract_d_sds_data(&msgs);

    assert_eq!(prim.main_address.ssi, 0x00FF_FFFF);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    // EN 300 392-2 clause 29.3.3.8.2 recommends all-ones as the
    // source address for SwMI system broadcast, even when the control
    // API supplied a local source identity.
    assert_eq!(pdu.calling_party_address_ssi, Some(0x00FF_FFFF));
    let SdsUserData::Type4(len_bits, data) = pdu.user_defined_data else {
        panic!("live SDS broadcast should use SDS-TL Type4");
    };
    assert_eq!(len_bits as usize, data.len() * 8);
    assert_eq!(&data[..4], &[0xDC, 0x00, 0x00, 0x01]);
    assert_eq!(&data[4..], b"LIVE");
    assert!(shared_config.state_read().live_sds_queue.is_empty());
}

#[test]
fn test_live_sds_control_rejects_source_issi_above_24_bits() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    let (dispatcher, endpoint) = make_control_link();
    let mut cmce = CmceBs::new(shared_config.clone(), None, Some(endpoint));
    let mut queue = MessageQueue::new();

    dispatcher.send(ControlCommand::AddLiveSds {
        text: "BAD".to_string(),
        protocol_id: 0xDC,
        source_issi: 0x0100_0000,
        repeat_count: 1,
    });
    cmce.tick_start(&mut queue, TdmaTime::default());

    assert!(queue.pop_front().is_none());
    assert!(shared_config.state_read().live_sds_queue.is_empty());
}

#[test]
fn test_live_sds_control_rejects_non_sds_tl_protocol_id() {
    debug::setup_logging_verbose();

    // EN 300 392-2 clause 29.4.1 and table 29.21 reserve 0x00..=0x7F for
    // applications that shall not use SDS-TL transport; 0xFF is the extension
    // PID, which this stack does not implement for SDS-TL TRANSFER yet.
    for protocol_id in [0x02, 0xFF] {
        let config = ComponentTest::get_default_test_config(StackMode::Bs);
        let shared_config = SharedConfig::from_parts(config, None);
        let (dispatcher, endpoint) = make_control_link();
        let mut cmce = CmceBs::new(shared_config.clone(), None, Some(endpoint));
        let mut queue = MessageQueue::new();

        dispatcher.send(ControlCommand::AddLiveSds {
            text: "BADPID".to_string(),
            protocol_id,
            source_issi: 0x00FF_FFFF,
            repeat_count: 1,
        });
        cmce.tick_start(&mut queue, TdmaTime::default());

        assert!(queue.pop_front().is_none());
        assert!(
            shared_config.state_read().live_sds_queue.is_empty(),
            "PID 0x{protocol_id:02X} must not enter the live SDS queue"
        );
    }
}

#[test]
fn test_live_sds_control_rejects_wap_sds_tl_protocol_id_0x84() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    let (dispatcher, endpoint) = make_control_link();
    let mut cmce = CmceBs::new(shared_config.clone(), None, Some(endpoint));
    let mut queue = MessageQueue::new();

    dispatcher.send(ControlCommand::AddLiveSds {
        text: "WAP".to_string(),
        protocol_id: 0x84,
        source_issi: 0x00FF_FFFF,
        repeat_count: 1,
    });
    cmce.tick_start(&mut queue, TdmaTime::default());

    // EN 300 392-2 table 29.21 assigns PID 0x84 to WAP with SDS-TL
    // transfer service. Live SDS only builds text-style SDS-TL transfer
    // envelopes, so it must reject WAP rather than transmit a misleading
    // non-text application PID through the text path.
    assert!(queue.pop_front().is_none());
    assert!(shared_config.state_read().live_sds_queue.is_empty());
}

#[test]
fn test_live_sds_control_queue_is_bounded() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    let (dispatcher, endpoint) = make_control_link();
    let mut cmce = CmceBs::new(shared_config.clone(), None, Some(endpoint));
    let mut queue = MessageQueue::new();

    for idx in 0..=LIVE_SDS_QUEUE_MAX_LEN {
        dispatcher.send(ControlCommand::AddLiveSds {
            text: format!("LIVE-{idx}"),
            protocol_id: 0x82,
            source_issi: 0x00FF_FFFF,
            repeat_count: 1,
        });
    }
    cmce.tick_start(&mut queue, TdmaTime::default());

    let state = shared_config.state_read();
    assert_eq!(
        state.live_sds_queue.len(),
        LIVE_SDS_QUEUE_MAX_LEN,
        "live SDS admission must be bounded before RF scheduling"
    );
    let expected_last = format!("LIVE-{}", LIVE_SDS_QUEUE_MAX_LEN - 1);
    assert_eq!(
        state.live_sds_queue.back().map(|m| m.text.as_str()),
        Some(expected_last.as_str()),
        "overflow item must be rejected, not evict an accepted broadcast silently"
    );
    assert_eq!(
        state.next_live_sds_id,
        LIVE_SDS_QUEUE_MAX_LEN as u32 + 1,
        "rejected overflow must not consume a live SDS id"
    );
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_periodic_sds_broadcast_invalid_protocol_id_is_not_sent() {
    debug::setup_logging_verbose();

    // Runtime guard for programmatic config: parser/dashboard validation should
    // normally reject this before TX, but sender must still not put a non-SDS-TL
    // PID on air as an SDS-TL TRANSFER.
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.home_mode_display = Some(broadcast_cfg("BADPID", 0x02));
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    sds.tick_start(&mut queue, TdmaTime::default());
    sds.tick_start(&mut queue, TdmaTime::default().add_timeslots(96 * 4));

    assert!(queue.pop_front().is_none(), "non-SDS-TL PID must not be sent as SDS-TL TRANSFER");
}

#[test]
fn test_periodic_sds_broadcast_rejects_wap_sds_tl_protocol_id_0x84() {
    debug::setup_logging_verbose();

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.home_mode_display = Some(broadcast_cfg("WAP", 0x84));
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    sds.tick_start(&mut queue, TdmaTime::default());
    sds.tick_start(&mut queue, TdmaTime::default().add_timeslots(96 * 4));

    assert!(
        queue.pop_front().is_none(),
        "WAP PID 0x84 must stay fail-closed in the text-style periodic SDS path"
    );
}

#[test]
fn test_u_sds_local_tsi_to_registered_issi_routes_as_acknowledged_d_sds() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let dest_issi = 2000001;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);
    register_subscriber(&mut test, dest_issi);

    // EN 300 392-2 clauses 13.3.2.3 and 14.7.2.8/table 14.28:
    // CPTI=2 carries called-party SSI plus extension. When the extension is
    // this SwMI MNI, the destination is the local TSI and may be routed by its
    // local 24-bit SSI without discarding the message as unsupported.
    let msg = build_u_sds_data_tsi_msg(source_issi, dest_issi, local_mni(&test.config), 0xABCD);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let (prim, pdu) = extract_d_sds_data(&sink_msgs);
    assert_eq!(prim.main_address.ssi, dest_issi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert_eq!(pdu.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(pdu.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(pdu.calling_party_extension, None);
    assert!(matches!(pdu.user_defined_data, SdsUserData::Type1(0xABCD)));
    assert_eq!(count_d_sds_pdus(&sink_msgs), 1);
    assert_eq!(count_d_status_pdus(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_u_sds_local_tsi_to_gssi_routes_as_unacknowledged_d_sds() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let member_issi = 1000002;
    let gssi = 226333;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);
    register_subscriber(&mut test, member_issi);
    affiliate_subscriber(&mut test, member_issi, gssi);

    // EN 300 392-2 clauses 13.2 and 13.3.2.3 include user-defined group SDS.
    // A local GTSI-form destination keeps group routing and unacknowledged L2.
    let msg = build_u_sds_data_tsi_msg(source_issi, gssi, local_mni(&test.config), 0xBEEF);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let (prim, pdu) = extract_d_sds_data(&sink_msgs);
    assert_eq!(prim.main_address.ssi, gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(pdu.calling_party_extension, None);
    assert!(matches!(pdu.user_defined_data, SdsUserData::Type1(0xBEEF)));
    assert_eq!(count_d_sds_pdus(&sink_msgs), 1);
    assert_eq!(count_d_status_pdus(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_u_sds_foreign_tsi_extension_is_not_rewritten_to_registered_issi() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, 2000001);
    register_subscriber(&mut test, 1000001);

    // EN 300 392-2 clauses 13.3.2.3, 14.7.2.8 and 14.7.3.2/table
    // 14.33: TSI addressing includes an extension/MNI. A foreign MNI is not
    // this local SwMI, so reject explicitly instead of routing by base SSI
    // alone.
    let msg = build_u_sds_data_tsi_msg(1000001, 2000001, 0x12_3456, 0xABCD);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    assert_sds_cmce_function_not_supported(&sink_msgs, 1000001, CmcePduTypeUl::USdsData);
    assert_eq!(count_d_sds_pdus(&sink_msgs), 0);
    assert_eq!(count_d_status_pdus(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_u_sds_short_number_address_is_rejected_for_ssi_gssi_router() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, 1000001);

    // EN 300 392-2 clauses 14.7.2.8 and 14.7.3.2/table 14.33: U-SDS-DATA
    // permits short-number addressing, but this router only implements
    // SSI/GSSI routing. Do not reinterpret a short number as an SSI.
    let msg = build_u_sds_data_short_number_msg(1000001, 42, 0xABCD);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    assert_sds_cmce_function_not_supported(&sink_msgs, 1000001, CmcePduTypeUl::USdsData);
    assert_eq!(count_d_sds_pdus(&sink_msgs), 0);
    assert_eq!(count_d_status_pdus(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_sds_brew_forward() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(CfgBrew {
        host: "test.local".into(),
        port: 3000,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        feature_rssi_export: false,
        whitelisted_ssis: None,
    });
    let mut test = ComponentTest::from_config(config, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Do NOT register dest ISSI — should forward to Brew
    register_subscriber(&mut test, 1000001);
    let msg = build_u_sds_data_msg(1000001, 5000001, 0x1234);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let brew_count = count_brew_sds(&sink_msgs);
    assert!(brew_count > 0, "Expected CmceSdsData at Brew sink for non-local ISSI");

    let d_sds_count = count_d_sds_data(&sink_msgs);
    assert_eq!(d_sds_count, 0, "Should not deliver locally when dest is not registered");
}

#[test]
fn test_u_sds_data_rejects_unregistered_rf_source_before_local_or_brew_routing() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let dest_issi = 2000001;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(brew_sds_enabled_config(), Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, dest_issi);

    // EN 300 392-2 clauses 13.3.2.1, 13.3.2.3 and 14.7.2.7 carry U-SDS-DATA
    // as an MS-originated service request. This SwMI must not route local SDS
    // or forward to Brew until MM registration state exists for the calling ISSI.
    test.submit_message(build_u_sds_data_msg(source_issi, dest_issi, 0xABCD));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert_eq!(count_brew_sds(&sink_msgs), 0);
    assert_eq!(count_d_sds_data(&sink_msgs), 0);
}

#[test]
fn test_u_sds_data_rejects_invalid_rf_source_before_brew_forwarding() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };

    for invalid_source in [
        TetraAddress::new(0x0100_0000, SsiType::Issi),
        TetraAddress::new(1000001, SsiType::Gssi),
        TetraAddress::new(1000001, SsiType::Unknown),
    ] {
        let mut test = ComponentTest::from_config(brew_sds_enabled_config(), Some(dltime));
        test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);

        // EN 300 392-2 clauses 13.2 and 14.7.2.7 define mobile-originated
        // SDS as an individual MS service. A non-ISSI or out-of-range RF
        // source must not be accepted as a Brew source identity.
        let msg = with_received_tetra_address(build_u_sds_data_msg(invalid_source.ssi, 5000001, 0xABCD), invalid_source);
        test.submit_message(msg);
        test.run_stack(Some(1));
        let sink_msgs = test.dump_sinks();

        assert_eq!(count_brew_sds(&sink_msgs), 0);
        assert_eq!(count_d_sds_data(&sink_msgs), 0);
    }
}

#[test]
fn test_sds_from_brew_to_local() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Register dest ISSI in StackState
    register_subscriber(&mut test, 2000001);

    // Submit CmceSdsData from Brew on Control SAP
    let msg = SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 3000001,
            dest_issi: 2000001,
            dest_ssi_type: None,
            user_defined_data: SdsUserData::Type1(0xCAFE),
            tx_reporter: None,
        }),
    };
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let d_sds_count = count_d_sds_data(&sink_msgs);
    assert_eq!(d_sds_count, 1, "Expected D-SDS-DATA at Mle sink from Brew");
}

#[test]
fn test_sds_from_brew_to_local_preserves_tx_reporter_for_air_delivery() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, 2000001);

    let reporter = TxReporter::new();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 3000001,
            dest_issi: 2000001,
            dest_ssi_type: None,
            user_defined_data: SdsUserData::Type1(0xCAFE),
            tx_reporter: Some(reporter.clone()),
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let (prim, pdu) = extract_d_sds_data(&sink_msgs);
    assert_eq!(prim.main_address.ssi, 2000001);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert!(matches!(pdu.user_defined_data, SdsUserData::Type1(0xCAFE)));

    let air_reporter = prim.tx_reporter.as_ref().expect("Brew SDS should carry TxReporter").clone();
    assert_eq!(reporter.get_state(), TxState::Pending);
    air_reporter.mark_transmitted();
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    air_reporter.mark_acknowledged();
    assert_eq!(reporter.get_state(), TxState::Acknowledged);
}

#[test]
fn test_sds_from_brew_invalid_destination_discards_tx_reporter() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);

    let reporter = TxReporter::new();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 3000001,
            dest_issi: 2000001,
            dest_ssi_type: Some(SsiType::Issi),
            user_defined_data: SdsUserData::Type1(0xCAFE),
            tx_reporter: Some(reporter.clone()),
        }),
    });
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 13.3.2.2 and 18.3.5.3.1 define reporting of SDS
    // delivery to the transport/user side. If CMCE rejects before air
    // submission, the caller's reporter must leave Pending as a failed
    // delivery instead of waiting forever.
    assert_eq!(count_d_sds_data(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
    assert_eq!(reporter.get_state(), TxState::Discarded);
}

#[test]
fn test_sds_from_brew_invalid_destination_ignores_late_discard_after_transmit() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);

    let reporter = TxReporter::new();
    reporter.mark_transmitted();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 3000001,
            dest_issi: 2000001,
            dest_ssi_type: Some(SsiType::Issi),
            user_defined_data: SdsUserData::Type1(0xCAFE),
            tx_reporter: Some(reporter.clone()),
        }),
    });
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clause 13.3.2.2 gives one transfer result for a status/SDS
    // handle. A stale local reject after an async transmit report must not turn
    // into a second conflicting result or panic the service.
    assert_eq!(count_d_sds_data(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
    assert_eq!(reporter.get_state(), TxState::Transmitted);
}

#[test]
fn test_sds_from_brew_invalid_type4_discards_tx_reporter() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, 2000001);

    let reporter = TxReporter::new();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 3000001,
            dest_issi: 2000001,
            dest_ssi_type: Some(SsiType::Issi),
            user_defined_data: SdsUserData::Type4(7, vec![0x82]),
            tx_reporter: Some(reporter.clone()),
        }),
    });
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.8.52 bounds Type4 SDS user data to a protocol
    // identifier plus payload bits. A sub-8-bit Type4 cannot serialize as
    // D-SDS-DATA, so the transmit reporter is failed locally.
    assert_eq!(count_d_sds_data(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
    assert_eq!(reporter.get_state(), TxState::Discarded);
}

#[test]
fn test_sds_from_brew_to_local_group_uses_gssi_unacknowledged_l2() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let source_issi = 3000001;
    let member_issi = 2000001;
    let gssi = 100;
    register_subscriber(&mut test, member_issi);
    affiliate_subscriber(&mut test, member_issi, gssi);

    // EN 300 392-2 clause 13.2 includes mobile-terminated user-defined group
    // short messages. Brew-origin SDS carries the 24-bit destination SSI; when
    // that SSI has local group members, CMCE must transmit one GSSI-addressed
    // unacknowledged D-SDS-DATA rather than dropping it as an unregistered ISSI.
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi,
            dest_issi: gssi,
            dest_ssi_type: None,
            user_defined_data: SdsUserData::Type1(0xCAFE),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let (prim, pdu) = extract_d_sds_data(&sink_msgs);
    assert_eq!(prim.main_address.ssi, gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_address_ssi, Some(source_issi as u64));
    assert!(matches!(pdu.user_defined_data, SdsUserData::Type1(0xCAFE)));
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_sds_from_brew_all_ones_dest_uses_gssi_unacknowledged_all_ones_source_no_report() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // EN 300 392-2 clause 29.3.3.8.2 allows SDS-TL system broadcast to
    // all-ones 0xFFFFFF. It should not depend on local group membership, should
    // use all-ones source on air, and shall request no delivery report.
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 3000001,
            dest_issi: 0x00FF_FFFF,
            dest_ssi_type: None,
            user_defined_data: SdsUserData::Type4(40, vec![0x82, 0x04, 0x44, 0x01, b'A']),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let (prim, pdu) = extract_d_sds_data(&sink_msgs);
    assert_eq!(prim.main_address.ssi, 0x00FF_FFFF);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_address_ssi, Some(0x00FF_FFFF));
    assert_eq!(pdu.user_defined_data.to_arr(), vec![0x82, 0x00, 0x44, 0x01, b'A']);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_sds_from_brew_all_ones_vendor_pid_transfer_clears_delivery_report_only() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 3000001,
            dest_issi: 0x00FF_FFFF,
            dest_ssi_type: None,
            user_defined_data: SdsUserData::Type4(40, vec![0xDC, 0x04, 0x44, 0x01, b'A']),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let (prim, pdu) = extract_d_sds_data(&sink_msgs);
    assert_eq!(prim.main_address.ssi, 0x00FF_FFFF);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_address_ssi, Some(0x00FF_FFFF));

    // EN 300 392-2 clause 29.3.3.8.2 forbids delivery-report requests for
    // all-ones system broadcast. The SDS-TL PID may be vendor/user-defined, so
    // only clear the transfer report bits and preserve PID/MR/payload.
    assert_eq!(pdu.user_defined_data.to_arr(), vec![0xDC, 0x00, 0x44, 0x01, b'A']);
}

#[test]
fn test_sds_from_brew_all_ones_non_sds_tl_pid_payload_is_not_rewritten() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 3000001,
            dest_issi: 0x00FF_FFFF,
            dest_ssi_type: None,
            user_defined_data: SdsUserData::Type4(40, vec![0x02, 0x04, 0x44, 0x01, b'A']),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let (prim, pdu) = extract_d_sds_data(&sink_msgs);
    assert_eq!(prim.main_address.ssi, 0x00FF_FFFF);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_address_ssi, Some(0x00FF_FFFF));

    // EN 300 392-2 clause 29.4.1 says PID 0x02 is outside the SDS-TL
    // transport-PDU range, so byte 1 is payload, not a delivery-report flag.
    assert_eq!(pdu.user_defined_data.to_arr(), vec![0x02, 0x04, 0x44, 0x01, b'A']);
}

#[test]
fn test_sds_from_brew_rejects_source_issi_above_24_bits() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, 2000001);

    // EN 300 392-2 table 14.13 carries D-SDS-DATA Calling party address SSI
    // in 24 bits. Network-origin SDS must reject sources that cannot fit.
    let msg = SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 0x0100_0000,
            dest_issi: 2000001,
            dest_ssi_type: None,
            user_defined_data: SdsUserData::Type1(0xCAFE),
            tx_reporter: None,
        }),
    };
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    assert_eq!(count_d_sds_data(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_sds_from_brew_rejects_dest_issi_above_24_bits() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // EN 300 392-2 clause 18.3.5.1.1 carries the MLE main address SSI on the
    // air interface; reject destinations outside the 24-bit SSI range.
    let msg = SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 3000001,
            dest_issi: 0x0100_0000,
            dest_ssi_type: None,
            user_defined_data: SdsUserData::Type1(0xCAFE),
            tx_reporter: None,
        }),
    };
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    assert_eq!(count_d_sds_data(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_sds_from_brew_unregistered() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Do NOT register dest ISSI
    let msg = SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 3000001,
            dest_issi: 9999999,
            dest_ssi_type: None,
            user_defined_data: SdsUserData::Type1(0xDEAD),
            tx_reporter: None,
        }),
    };
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let d_sds_count = count_d_sds_data(&sink_msgs);
    assert_eq!(d_sds_count, 0, "Should not deliver D-SDS-DATA when dest is not registered");
}

#[test]
fn test_sds_group_delivery() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let gssi = 100;

    // Register 3 ISSIs and affiliate them with the GSSI in StackState
    for issi in [1000001, 1000002, 1000003] {
        register_subscriber(&mut test, issi);
        affiliate_subscriber(&mut test, issi, gssi);
    }

    // Send U-SDS-DATA to the GSSI
    let msg = build_u_sds_data_msg(1000001, gssi, 0xBEEF);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let d_sds_count = count_d_sds_data(&sink_msgs);
    assert_eq!(d_sds_count, 1, "Expected exactly 1 GSSI-addressed D-SDS-DATA (not per-member)");

    // Verify the address is GSSI
    for m in &sink_msgs {
        if m.dest == TetraEntity::Mle {
            if let SapMsgInner::LcmcMleUnitdataReq(ref prim) = m.msg {
                assert_eq!(prim.main_address.ssi, gssi);
                assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
            }
        }
    }
}

#[test]
fn test_u_sds_to_local_group_uses_unacknowledged_l2_and_preserves_fields() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let gssi = 100;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);
    affiliate_subscriber(&mut test, source_issi, gssi);

    test.submit_message(build_u_sds_data_msg(source_issi, gssi, 0xBEEF));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (prim, pdu) = extract_d_sds_data(&sink_msgs);

    // EN 300 392-2 clause 18.3.5.3.1 maps unacknowledged L2 service to
    // TL-UNITDATA. GSSI SDS is group delivery and must not request per-MS ACKs.
    assert_eq!(prim.main_address.ssi, gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(pdu.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(pdu.calling_party_extension, None);
    assert!(matches!(pdu.user_defined_data, SdsUserData::Type1(0xBEEF)));
}

#[test]
fn test_u_sds_to_large_local_group_routes_once_as_unacknowledged_gssi() {
    debug::setup_logging_verbose();

    let first_issi = 1000001;
    let source_issi = first_issi + LARGE_LOCAL_GROUP_MEMBER_COUNT - 1;
    let gssi = 226333;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_affiliated_group_members(&mut test, first_issi, LARGE_LOCAL_GROUP_MEMBER_COUNT, gssi);

    test.submit_message(build_u_sds_data_msg(source_issi, gssi, 0xBEEF));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (prim, pdu) = extract_d_sds_data(&sink_msgs);

    // EN 300 392-2 clauses 13.2 and 18.3.5.3.1 include user-defined group
    // SDS over unacknowledged delivery. A large local affiliate set must still
    // route as one GSSI-addressed D-SDS-DATA, not per-member ISSI fan-out.
    assert_eq!(count_d_sds_pdus(&sink_msgs), 1);
    assert_eq!(count_d_status_pdus(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
    assert_eq!(prim.main_address.ssi, gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(pdu.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(pdu.calling_party_extension, None);
    assert!(matches!(pdu.user_defined_data, SdsUserData::Type1(0xBEEF)));
}

#[test]
fn test_u_sds_to_all_ones_group_without_affiliation_uses_gssi_unitdata() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let broadcast_gssi = 0x00FF_FFFF;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);

    test.submit_message(build_u_sds_data_msg(source_issi, broadcast_gssi, 0xBEEF));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (prim, pdu) = extract_d_sds_data(&sink_msgs);

    // EN 300 392-2 clause 23.4.1.2.1 note 3 defines the all-ones
    // predefined broadcast GSSI as a group to which all MS belong. Mobile
    // originated SDS to that address is therefore local group delivery even
    // when the subscriber registry has no explicit affiliation entry.
    assert_eq!(count_brew_sds(&sink_msgs), 0);
    assert_eq!(prim.main_address.ssi, broadcast_gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(pdu.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(pdu.calling_party_extension, None);
    assert!(matches!(pdu.user_defined_data, SdsUserData::Type1(0xBEEF)));
}

#[test]
fn test_u_sds_ambiguous_issi_gssi_destination_is_dropped() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let ambiguous_ssi = 100;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);
    register_subscriber(&mut test, ambiguous_ssi);
    affiliate_subscriber(&mut test, source_issi, ambiguous_ssi);

    test.submit_message(build_u_sds_data_msg(source_issi, ambiguous_ssi, 0xBEEF));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clause 13.2 has distinct individual and group SDS
    // services. With only the numeric SSI and no explicit destination kind,
    // the local router must not silently choose ISSI over GSSI.
    assert_eq!(count_d_sds_data(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_brew_sds_ambiguous_issi_gssi_destination_is_dropped() {
    debug::setup_logging_verbose();

    let source_issi = 3000001;
    let group_member = 1000001;
    let ambiguous_ssi = 100;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, ambiguous_ssi);
    register_subscriber(&mut test, group_member);
    affiliate_subscriber(&mut test, group_member, ambiguous_ssi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi,
            dest_issi: ambiguous_ssi,
            dest_ssi_type: None,
            user_defined_data: SdsUserData::Type1(0xCAFE),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert_eq!(count_d_sds_data(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_u_status_forwarded_as_d_status() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Register both source and dest
    register_subscriber(&mut test, 1000001);
    register_subscriber(&mut test, 2000001);

    let status = PreCodedStatus::try_from(0x8210).unwrap();
    let msg = build_u_status_msg(1000001, 2000001, status);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();

    // Should produce exactly 1 D-STATUS at Mle sink
    let mle_msgs: Vec<_> = sink_msgs
        .iter()
        .filter(|m| m.dest == TetraEntity::Mle && matches!(&m.msg, SapMsgInner::LcmcMleUnitdataReq(_)))
        .collect();
    assert_eq!(mle_msgs.len(), 1, "Expected 1 D-STATUS at Mle sink");

    let (prim, d_status) = extract_d_status(&sink_msgs);
    assert_eq!(prim.main_address.ssi, 2000001);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);

    // EN 300 392-2 clauses 14.7.1.11 and 14.7.2.7 carry a pre-coded
    // status with the calling party as SSI and no optional external/DM fields.
    assert_eq!(d_status.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(d_status.calling_party_address_ssi, Some(1000001));
    assert_eq!(d_status.calling_party_extension, None);
    assert_eq!(d_status.pre_coded_status, status);
    assert!(d_status.external_subscriber_number.is_none());
    assert!(d_status.dm_ms_address.is_none());
}

#[test]
fn test_u_status_all_ones_status_to_local_issi_emits_d_status() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let dest_issi = 2000001;
    let status = PreCodedStatus::NetworkUserSpecific(0xFFFF);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);
    register_subscriber(&mut test, dest_issi);

    test.submit_message(build_u_status_msg(source_issi, dest_issi, status));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (prim, d_status) = extract_d_status(&sink_msgs);

    // EN 300 392-2 tables 14.27 and 14.72 make 0xFFFF a 16-bit
    // network/user-specific pre-coded status. It must remain a D-STATUS
    // value, distinct from the 24-bit all-ones broadcast GSSI.
    assert_eq!(prim.main_address.ssi, dest_issi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.pre_coded_status, status);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_u_status_ambiguous_issi_gssi_destination_is_dropped() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let ambiguous_ssi = 100;
    let status = PreCodedStatus::try_from(0x8210).unwrap();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);
    register_subscriber(&mut test, ambiguous_ssi);
    affiliate_subscriber(&mut test, source_issi, ambiguous_ssi);

    test.submit_message(build_u_status_msg(source_issi, ambiguous_ssi, status));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 13.2 and 14.5.5 preserve separate predefined
    // individual and group status services. A numeric collision is ambiguous
    // until the internal/Brew SAP carries the address kind explicitly.
    assert_eq!(count_d_sds_data(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_u_status_to_registered_9999_without_command_config_is_d_status() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let dest_issi = 9999;
    let status = PreCodedStatus::try_from(0x8210).unwrap();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);
    register_subscriber(&mut test, dest_issi);

    test.submit_message(build_u_status_msg(source_issi, dest_issi, status));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (prim, d_status) = extract_d_status(&sink_msgs);

    assert_eq!(prim.main_address.ssi, dest_issi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.pre_coded_status, status);
}

#[test]
fn test_u_status_to_registered_9999_with_unmatched_command_config_is_d_status() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let dest_issi = 9999;
    let status = PreCodedStatus::NetworkUserSpecific(0x9002);
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sds_command_control = Some(CfgSdsCommandControl {
        authorized_issis: vec![source_issi],
        commands: vec![CfgSdsCommandEntry {
            status_code: 0x9001,
            action: "kick_all".into(),
        }],
    });
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(config, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);
    register_subscriber(&mut test, dest_issi);

    test.submit_message(build_u_status_msg(source_issi, dest_issi, status));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (prim, d_status) = extract_d_status(&sink_msgs);

    assert_eq!(prim.main_address.ssi, dest_issi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.pre_coded_status, status);
}

#[test]
fn test_u_status_to_9999_sds_tl_short_report_does_not_trigger_command_control() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let dest_issi = 9999;
    let status = PreCodedStatus::try_from(32001).unwrap();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sds_command_control = Some(CfgSdsCommandControl {
        authorized_issis: vec![source_issi],
        commands: vec![CfgSdsCommandEntry {
            status_code: 32001,
            action: "kick_all".into(),
        }],
    });
    let shared_config = SharedConfig::from_parts(config, None);
    {
        let mut state = shared_config.state_write();
        state.subscribers.register(source_issi);
        state.subscribers.register(dest_issi);
    }
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    // EN 300 392-2 clause 14.8.34 table 14.72 maps 31744..=32767 to SDS-TL
    // short reporting, not network/user-specific local commands. Even a direct
    // programmatic config must not let those values trigger local actions.
    sds.route_status_deliver(&mut queue, build_u_status_msg(source_issi, dest_issi, status));

    assert!(sds.pending_actions.is_empty());
    let msg = queue.pop_front().expect("SDS-TL status should continue as normal D-STATUS");
    assert!(matches!(msg.msg, SapMsgInner::LcmcMleUnitdataReq(_)));
}

#[test]
fn test_u_status_to_9999_matching_command_control_is_consumed_locally() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let command_status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sds_command_control = Some(CfgSdsCommandControl {
        authorized_issis: vec![source_issi],
        commands: vec![CfgSdsCommandEntry {
            status_code: command_status.into_raw() as u16,
            action: "kick_all".into(),
        }],
    });
    let shared_config = SharedConfig::from_parts(config, None);
    {
        let mut state = shared_config.state_write();
        state.subscribers.register(source_issi);
        state.subscribers.register(9999);
    }
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    // EN 300 392-2 clauses 13.3.2.1 and 14.7.2.8 model this as a registered
    // MS-originated U-STATUS; the local kick_all action is a stack extension
    // layered after that RF-source registration check.
    sds.route_status_deliver(&mut queue, build_u_status_msg(source_issi, 9999, command_status));

    assert!(
        queue.pop_front().is_none(),
        "matching SDS command control must not also emit D-STATUS"
    );
    assert_eq!(sds.pending_actions.len(), 1);
    assert!(matches!(sds.pending_actions.first(), Some(SdsPendingAction::KickAll)));
}

#[test]
fn test_u_status_to_local_issi_reaches_llc_as_tl_data_request() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let dest_issi = 2000001;
    let status = PreCodedStatus::try_from(0x8210).unwrap();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce, TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);
    register_subscriber(&mut test, dest_issi);

    test.submit_message(build_u_status_msg(source_issi, dest_issi, status));
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let llc_msgs: Vec<_> = sink_msgs.iter().filter(|m| m.dest == TetraEntity::Llc).collect();
    assert_eq!(llc_msgs.len(), 1, "Expected one U-STATUS delivery to LLC");
    let SapMsgInner::TlaTlDataReqBl(prim) = &llc_msgs[0].msg else {
        panic!("ISSI D-STATUS should use TL-DATA request at TLA-SAP");
    };

    // EN 300 392-2 clause 18.3.5.3.1 maps acknowledged request to TL-DATA
    // request. ISSI D-STATUS keeps per-MS basic-link acknowledgement.
    assert_eq!(prim.main_address.ssi, dest_issi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    let d_status = extract_tla_d_status(&prim.tl_sdu);
    assert_eq!(d_status.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.calling_party_extension, None);
    assert_eq!(d_status.pre_coded_status, status);
    assert!(d_status.external_subscriber_number.is_none());
    assert!(d_status.dm_ms_address.is_none());
}

#[test]
fn test_network_origin_sds_status_to_local_issi_reaches_llc_as_tl_data_request() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let dest_issi = 2000001;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce, TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Brew]);
    register_subscriber(&mut test, dest_issi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsStatus(CmceSdsStatus {
            source_issi,
            dest_issi,
            dest_ssi_type: SsiType::Issi,
            status_number: status.into_raw(),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let llc_msgs: Vec<_> = sink_msgs.iter().filter(|m| m.dest == TetraEntity::Llc).collect();
    assert_eq!(llc_msgs.len(), 1, "Expected one network-origin D-STATUS delivery to LLC");
    let SapMsgInner::TlaTlDataReqBl(prim) = &llc_msgs[0].msg else {
        panic!("network-origin ISSI D-STATUS should use TL-DATA request at TLA-SAP");
    };

    // EN 300 392-2 clauses 14.7.1.11 and 18.3.5.3.1: individual D-STATUS
    // delivery uses acknowledged service and therefore maps to TL-DATA.
    assert_eq!(prim.main_address.ssi, dest_issi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    let d_status = extract_tla_d_status(&prim.tl_sdu);
    assert_eq!(d_status.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.pre_coded_status, status);
}

#[test]
fn test_network_origin_sds_status_to_local_issi_preserves_tx_reporter_for_air_delivery() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let dest_issi = 2000001;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce, TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Brew]);
    register_subscriber(&mut test, dest_issi);

    let reporter = TxReporter::new();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsStatus(CmceSdsStatus {
            source_issi,
            dest_issi,
            dest_ssi_type: SsiType::Issi,
            status_number: status.into_raw(),
            tx_reporter: Some(reporter.clone()),
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let llc_msgs: Vec<_> = sink_msgs.iter().filter(|m| m.dest == TetraEntity::Llc).collect();
    assert_eq!(llc_msgs.len(), 1, "Expected one network-origin D-STATUS delivery to LLC");
    let SapMsgInner::TlaTlDataReqBl(prim) = &llc_msgs[0].msg else {
        panic!("network-origin ISSI D-STATUS should use TL-DATA request at TLA-SAP");
    };

    // EN 300 392-2 clauses 13.3.2.2 and 18.3.5.3.1 define reportable
    // TNSDS-STATUS over acknowledged TL-DATA for individual delivery.
    let air_reporter = prim
        .tx_reporter
        .as_ref()
        .expect("network-origin ISSI D-STATUS should carry TxReporter")
        .clone();
    assert_eq!(reporter.get_state(), TxState::Pending);
    air_reporter.mark_transmitted();
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    air_reporter.mark_acknowledged();
    assert_eq!(reporter.get_state(), TxState::Acknowledged);
}

#[test]
fn test_network_origin_sds_status_numeric_collision_explicit_issi_routes_individual() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let collided_ssi = 2000001;
    let group_member = 2000002;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce, TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Brew]);
    register_subscriber(&mut test, collided_ssi);
    register_subscriber(&mut test, group_member);
    affiliate_subscriber(&mut test, group_member, collided_ssi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsStatus(CmceSdsStatus {
            source_issi,
            dest_issi: collided_ssi,
            dest_ssi_type: SsiType::Issi,
            status_number: status.into_raw(),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let llc_msgs: Vec<_> = sink_msgs.iter().filter(|m| m.dest == TetraEntity::Llc).collect();
    assert_eq!(
        llc_msgs.len(),
        1,
        "Expected explicit ISSI network-origin D-STATUS delivery despite numeric GSSI collision"
    );
    let SapMsgInner::TlaTlDataReqBl(prim) = &llc_msgs[0].msg else {
        panic!("explicit ISSI D-STATUS should use TL-DATA request at TLA-SAP");
    };

    // EN 300 392-2 clause 13.2 keeps pre-defined individual and group status
    // services distinct. The control SAP address kind selects the individual
    // service when the numeric SSI is also a local GSSI.
    assert_eq!(prim.main_address.ssi, collided_ssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    let d_status = extract_tla_d_status(&prim.tl_sdu);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.pre_coded_status, status);
}

#[test]
fn test_network_origin_sds_data_numeric_collision_explicit_issi_routes_individual() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let collided_ssi = 2000001;
    let group_member = 2000002;
    let payload = SdsUserData::Type1(0x1234);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce, TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Brew]);
    register_subscriber(&mut test, collided_ssi);
    register_subscriber(&mut test, group_member);
    affiliate_subscriber(&mut test, group_member, collided_ssi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi,
            dest_issi: collided_ssi,
            dest_ssi_type: Some(SsiType::Issi),
            user_defined_data: payload.clone(),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let llc_msgs: Vec<_> = sink_msgs.iter().filter(|m| m.dest == TetraEntity::Llc).collect();
    assert_eq!(
        llc_msgs.len(),
        1,
        "Expected explicit ISSI network-origin D-SDS-DATA delivery despite numeric GSSI collision"
    );
    let SapMsgInner::TlaTlDataReqBl(prim) = &llc_msgs[0].msg else {
        panic!("explicit ISSI D-SDS-DATA should use TL-DATA request at TLA-SAP");
    };

    // EN 300 392-2 clause 13.2 keeps individual and group SDS services
    // distinct. The internal SAP destination kind selects the individual
    // service when the numeric SSI is also a local GSSI.
    assert_eq!(prim.main_address.ssi, collided_ssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    let d_sds = extract_tla_d_sds_data(&prim.tl_sdu);
    assert_eq!(d_sds.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_sds.user_defined_data, payload);
}

#[test]
fn test_u_status_local_tsi_to_registered_issi_routes_as_acknowledged_d_status() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let dest_issi = 2000001;
    let status = PreCodedStatus::try_from(0x8210).unwrap();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);
    register_subscriber(&mut test, dest_issi);

    // EN 300 392-2 clauses 13.3.2.1 and 14.7.2.7/table 14.27:
    // CPTI=2 carries called-party SSI plus extension. A local MNI extension
    // is a local TSI, so predefined status routes like SSI-addressed status.
    let msg = build_u_status_tsi_msg(source_issi, dest_issi, local_mni(&test.config), status);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let (prim, d_status) = extract_d_status(&sink_msgs);
    assert_eq!(prim.main_address.ssi, dest_issi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert_eq!(d_status.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.calling_party_extension, None);
    assert_eq!(d_status.pre_coded_status, status);
    assert_eq!(count_d_sds_pdus(&sink_msgs), 0);
    assert_eq!(count_d_status_pdus(&sink_msgs), 1);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_u_status_local_tsi_to_gssi_routes_as_unacknowledged_d_status() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let member_issi = 1000002;
    let gssi = 226333;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);
    register_subscriber(&mut test, member_issi);
    affiliate_subscriber(&mut test, member_issi, gssi);

    // EN 300 392-2 clauses 13.2 and 13.3.2.1 include predefined group
    // status. A local GTSI-form destination keeps GSSI routing and
    // unacknowledged L2 service.
    let msg = build_u_status_tsi_msg(source_issi, gssi, local_mni(&test.config), status);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let (prim, d_status) = extract_d_status(&sink_msgs);
    assert_eq!(prim.main_address.ssi, gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.calling_party_extension, None);
    assert_eq!(d_status.pre_coded_status, status);
    assert_eq!(count_d_sds_pdus(&sink_msgs), 0);
    assert_eq!(count_d_status_pdus(&sink_msgs), 1);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_u_status_foreign_tsi_extension_is_not_rewritten_to_registered_issi() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, 1000001);
    register_subscriber(&mut test, 2000001);

    // EN 300 392-2 clauses 13.3.2.1, 14.7.2.7 and 14.7.3.2/table
    // 14.33: TSI addressing includes an extension/MNI. A foreign MNI is not
    // this local SwMI, so reject explicitly instead of routing by base SSI
    // alone.
    let status = PreCodedStatus::try_from(0x8210).unwrap();
    let msg = build_u_status_tsi_msg(1000001, 2000001, 0x12_3456, status);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    assert_sds_cmce_function_not_supported(&sink_msgs, 1000001, CmcePduTypeUl::UStatus);
    assert_eq!(count_d_sds_pdus(&sink_msgs), 0);
    assert_eq!(count_d_status_pdus(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_u_status_short_number_address_is_rejected_for_ssi_gssi_router() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, 1000001);

    let status = PreCodedStatus::try_from(0x8210).unwrap();
    let msg = build_u_status_short_number_msg(1000001, 42, status);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    // EN 300 392-2 clauses 14.7.2.7 and 14.7.3.2/table 14.33: do not
    // reinterpret short-number status addressing as a local SSI/GSSI route.
    assert_sds_cmce_function_not_supported(&sink_msgs, 1000001, CmcePduTypeUl::UStatus);
    assert_eq!(count_d_sds_pdus(&sink_msgs), 0);
    assert_eq!(count_d_status_pdus(&sink_msgs), 0);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_u_status_to_local_group_is_unacknowledged_d_status() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let gssi = 100;
    register_subscriber(&mut test, 1000001);
    register_subscriber(&mut test, 1000002);
    affiliate_subscriber(&mut test, 1000002, gssi);

    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let msg = build_u_status_msg(1000001, gssi, status);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let mle_msgs: Vec<_> = sink_msgs
        .iter()
        .filter(|m| m.dest == TetraEntity::Mle && matches!(&m.msg, SapMsgInner::LcmcMleUnitdataReq(_)))
        .collect();
    assert_eq!(mle_msgs.len(), 1, "Expected 1 GSSI-addressed D-STATUS at Mle sink");

    let (prim, d_status) = extract_d_status(&sink_msgs);
    assert_eq!(prim.main_address.ssi, gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);

    // EN 300 392-2 clauses 14.7.1.11 and 14.7.2.7 use the same D-STATUS
    // field shape for local group delivery; only the MLE/L2 destination changes.
    assert_eq!(d_status.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(d_status.calling_party_address_ssi, Some(1000001));
    assert_eq!(d_status.calling_party_extension, None);
    assert_eq!(d_status.pre_coded_status, status);
    assert!(d_status.external_subscriber_number.is_none());
    assert!(d_status.dm_ms_address.is_none());
}

#[test]
fn test_u_status_to_local_group_reaches_llc_as_tl_unitdata_request() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let member_issi = 1000002;
    let gssi = 100;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce, TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);
    register_subscriber(&mut test, member_issi);
    affiliate_subscriber(&mut test, member_issi, gssi);

    test.submit_message(build_u_status_msg(source_issi, gssi, status));
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let llc_msgs: Vec<_> = sink_msgs.iter().filter(|m| m.dest == TetraEntity::Llc).collect();
    assert_eq!(llc_msgs.len(), 1, "Expected one GSSI U-STATUS delivery to LLC");
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &llc_msgs[0].msg else {
        panic!("GSSI D-STATUS should use TL-UNITDATA request at TLA-SAP");
    };

    // EN 300 392-2 clause 18.3.5.3.1 maps unacknowledged service to
    // TL-UNITDATA request. GSSI D-STATUS must not request per-MS ACKs.
    assert_eq!(prim.main_address.ssi, gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    let d_status = extract_tla_d_status(&prim.tl_sdu);
    assert_eq!(d_status.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.calling_party_extension, None);
    assert_eq!(d_status.pre_coded_status, status);
    assert!(d_status.external_subscriber_number.is_none());
    assert!(d_status.dm_ms_address.is_none());
}

#[test]
fn test_u_status_to_large_local_group_routes_once_as_unacknowledged_gssi() {
    debug::setup_logging_verbose();

    let first_issi = 1000001;
    let source_issi = first_issi;
    let gssi = 226333;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_affiliated_group_members(&mut test, first_issi, LARGE_LOCAL_GROUP_MEMBER_COUNT, gssi);

    test.submit_message(build_u_status_msg(source_issi, gssi, status));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (prim, d_status) = extract_d_status(&sink_msgs);

    // EN 300 392-2 clauses 13.2, 14.7.1.11 and 14.7.2.7 include
    // predefined group status. With 1024 local affiliates, the BS should
    // still emit a single GSSI-addressed D-STATUS using unacknowledged L2.
    assert_eq!(count_d_sds_pdus(&sink_msgs), 0);
    assert_eq!(count_d_status_pdus(&sink_msgs), 1);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
    assert_eq!(prim.main_address.ssi, gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(d_status.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.calling_party_extension, None);
    assert_eq!(d_status.pre_coded_status, status);
    assert!(d_status.external_subscriber_number.is_none());
    assert!(d_status.dm_ms_address.is_none());
}

#[test]
fn test_u_status_to_all_ones_group_without_affiliation_uses_gssi_unitdata() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let broadcast_gssi = 0x00FF_FFFF;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);

    test.submit_message(build_u_status_msg(source_issi, broadcast_gssi, status));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (prim, d_status) = extract_d_status(&sink_msgs);

    // EN 300 392-2 table 14.27 sends U-STATUS to a called party SSI, and
    // clause 23.4.1.2.1 note 3 makes all-ones a predefined broadcast group.
    // The downlink D-STATUS must therefore be group-addressed and use
    // unacknowledged L2 service, not Brew forwarding or per-ISSI ACK.
    assert_eq!(count_brew_sds(&sink_msgs), 0);
    assert_eq!(prim.main_address.ssi, broadcast_gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(d_status.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.calling_party_extension, None);
    assert_eq!(d_status.pre_coded_status, status);
    assert!(d_status.external_subscriber_number.is_none());
    assert!(d_status.dm_ms_address.is_none());
}

#[test]
fn test_u_status_all_ones_status_to_all_ones_group_emits_unacknowledged_d_status() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let broadcast_gssi = 0x00FF_FFFF;
    let status = PreCodedStatus::NetworkUserSpecific(0xFFFF);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(brew_sds_enabled_config(), Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);

    test.submit_message(build_u_status_msg(source_issi, broadcast_gssi, status));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (prim, d_status) = extract_d_status(&sink_msgs);

    // EN 300 392-2 clause 23.4.1.2.1 note 3 defines the 24-bit all-ones
    // destination as broadcast GSSI, while table 14.72 separately allows the
    // 16-bit all-ones status number. Both all-ones fields must be preserved.
    assert_eq!(prim.main_address.ssi, broadcast_gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.pre_coded_status, status);
    assert_eq!(count_brew_sds(&sink_msgs), 0);
}

#[test]
fn test_network_origin_sds_status_to_local_group_reaches_llc_as_tl_unitdata_request() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let member_issi = 1000002;
    let gssi = 100;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce, TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Brew]);
    register_subscriber(&mut test, member_issi);
    affiliate_subscriber(&mut test, member_issi, gssi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsStatus(CmceSdsStatus {
            source_issi,
            dest_issi: gssi,
            dest_ssi_type: SsiType::Gssi,
            status_number: status.into_raw(),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let llc_msgs: Vec<_> = sink_msgs.iter().filter(|m| m.dest == TetraEntity::Llc).collect();
    assert_eq!(llc_msgs.len(), 1, "Expected one network-origin GSSI D-STATUS delivery to LLC");
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &llc_msgs[0].msg else {
        panic!("network-origin GSSI D-STATUS should use TL-UNITDATA request at TLA-SAP");
    };

    // EN 300 392-2 clauses 13.2 and 18.3.5.3.1: group status delivery is a
    // group short status service and must not request a per-MS basic-link ACK.
    assert_eq!(prim.main_address.ssi, gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    let d_status = extract_tla_d_status(&prim.tl_sdu);
    assert_eq!(d_status.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.pre_coded_status, status);
}

#[test]
fn test_network_origin_sds_status_to_local_group_preserves_unacked_tx_reporter() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let member_issi = 1000002;
    let gssi = 100;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce, TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Brew]);
    register_subscriber(&mut test, member_issi);
    affiliate_subscriber(&mut test, member_issi, gssi);

    let reporter = TxReporter::new_unacked();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsStatus(CmceSdsStatus {
            source_issi,
            dest_issi: gssi,
            dest_ssi_type: SsiType::Gssi,
            status_number: status.into_raw(),
            tx_reporter: Some(reporter.clone()),
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let llc_msgs: Vec<_> = sink_msgs.iter().filter(|m| m.dest == TetraEntity::Llc).collect();
    assert_eq!(llc_msgs.len(), 1, "Expected one network-origin GSSI D-STATUS delivery to LLC");
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &llc_msgs[0].msg else {
        panic!("network-origin GSSI D-STATUS should use TL-UNITDATA request at TLA-SAP");
    };

    // EN 300 392-2 clauses 13.3.2.2 and 18.3.5.3.1 allow reportable
    // TNSDS-STATUS while group delivery still uses unacknowledged TL-UNITDATA.
    let air_reporter = prim
        .tx_reporter
        .as_ref()
        .expect("network-origin GSSI D-STATUS should carry TxReporter")
        .clone();
    assert_eq!(reporter.get_state(), TxState::Pending);
    air_reporter.mark_transmitted();
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert!(reporter.is_in_final_state());
}

#[test]
fn test_network_origin_sds_status_invalid_destination_discards_tx_reporter() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let unregistered_dest_issi = 2000001;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce, TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Brew]);

    let reporter = TxReporter::new();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsStatus(CmceSdsStatus {
            source_issi,
            dest_issi: unregistered_dest_issi,
            dest_ssi_type: SsiType::Issi,
            status_number: status.into_raw(),
            tx_reporter: Some(reporter.clone()),
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    assert!(
        sink_msgs.iter().all(|m| m.dest != TetraEntity::Llc),
        "invalid D-STATUS destination must not be handed to LLC"
    );
    assert_eq!(reporter.get_state(), TxState::Discarded);
}

#[test]
fn test_network_origin_sds_status_invalid_destination_ignores_late_discard_after_transmit() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let unregistered_dest_issi = 2000001;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce, TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Brew]);

    let reporter = TxReporter::new();
    reporter.mark_transmitted();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsStatus(CmceSdsStatus {
            source_issi,
            dest_issi: unregistered_dest_issi,
            dest_ssi_type: SsiType::Issi,
            status_number: status.into_raw(),
            tx_reporter: Some(reporter.clone()),
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    assert!(
        sink_msgs.iter().all(|m| m.dest != TetraEntity::Llc),
        "invalid D-STATUS destination must not be handed to LLC"
    );
    assert_eq!(reporter.get_state(), TxState::Transmitted);
}

#[test]
fn test_network_origin_sds_status_to_all_ones_normalizes_source_and_uses_group_unitdata() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let broadcast_gssi = 0x00FF_FFFF;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce, TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Brew]);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsStatus(CmceSdsStatus {
            source_issi,
            dest_issi: broadcast_gssi,
            dest_ssi_type: SsiType::Gssi,
            status_number: status.into_raw(),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let llc_msgs: Vec<_> = sink_msgs.iter().filter(|m| m.dest == TetraEntity::Llc).collect();
    assert_eq!(llc_msgs.len(), 1, "Expected one all-ones GSSI D-STATUS delivery to LLC");
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &llc_msgs[0].msg else {
        panic!("all-ones GSSI D-STATUS should use TL-UNITDATA request at TLA-SAP");
    };

    // EN 300 392-2 table 14.14 defines D-STATUS as a one-way pre-coded status
    // PDU. For predefined all-ones broadcast GSSI, keep group unacknowledged
    // delivery and normalize the over-air source to all ones, matching the
    // SDS-TL broadcast source convention.
    assert_eq!(prim.main_address.ssi, broadcast_gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    let d_status = extract_tla_d_status(&prim.tl_sdu);
    assert_eq!(d_status.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(d_status.calling_party_address_ssi, Some(broadcast_gssi as u64));
    assert_eq!(d_status.pre_coded_status, status);
}

#[test]
fn test_network_origin_sds_status_numeric_collision_explicit_gssi_routes_group() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let collided_ssi = 2000001;
    let group_member = 2000002;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce, TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Brew]);
    register_subscriber(&mut test, collided_ssi);
    register_subscriber(&mut test, group_member);
    affiliate_subscriber(&mut test, group_member, collided_ssi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsStatus(CmceSdsStatus {
            source_issi,
            dest_issi: collided_ssi,
            dest_ssi_type: SsiType::Gssi,
            status_number: status.into_raw(),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let llc_msgs: Vec<_> = sink_msgs.iter().filter(|m| m.dest == TetraEntity::Llc).collect();
    assert_eq!(
        llc_msgs.len(),
        1,
        "Expected explicit GSSI network-origin D-STATUS delivery despite numeric ISSI collision"
    );
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &llc_msgs[0].msg else {
        panic!("explicit GSSI D-STATUS should use TL-UNITDATA request at TLA-SAP");
    };

    // EN 300 392-2 clauses 13.2 and 18.3.5.3.1: group status uses the
    // group service and unacknowledged layer-2 delivery, even when the GSSI
    // value also exists as a registered ISSI.
    assert_eq!(prim.main_address.ssi, collided_ssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    let d_status = extract_tla_d_status(&prim.tl_sdu);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.pre_coded_status, status);
}

#[test]
fn test_network_origin_sds_data_numeric_collision_explicit_gssi_routes_group() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let collided_ssi = 2000001;
    let group_member = 2000002;
    let payload = SdsUserData::Type1(0x1234);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce, TetraEntity::Mle], vec![TetraEntity::Llc, TetraEntity::Brew]);
    register_subscriber(&mut test, collided_ssi);
    register_subscriber(&mut test, group_member);
    affiliate_subscriber(&mut test, group_member, collided_ssi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi,
            dest_issi: collided_ssi,
            dest_ssi_type: Some(SsiType::Gssi),
            user_defined_data: payload.clone(),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let llc_msgs: Vec<_> = sink_msgs.iter().filter(|m| m.dest == TetraEntity::Llc).collect();
    assert_eq!(
        llc_msgs.len(),
        1,
        "Expected explicit GSSI network-origin D-SDS-DATA delivery despite numeric ISSI collision"
    );
    let SapMsgInner::TlaTlUnitdataReqBl(prim) = &llc_msgs[0].msg else {
        panic!("explicit GSSI D-SDS-DATA should use TL-UNITDATA request at TLA-SAP");
    };

    // EN 300 392-2 clauses 13.2 and 18.3.5.3.1: group SDS data uses the
    // group service and unacknowledged layer-2 delivery, even when the GSSI
    // value also exists as a registered ISSI.
    assert_eq!(prim.main_address.ssi, collided_ssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    let d_sds = extract_tla_d_sds_data(&prim.tl_sdu);
    assert_eq!(d_sds.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_sds.user_defined_data, payload);
}

#[test]
fn test_control_sds_rejects_non_byte_aligned_len_bits() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_sds_from_control(
        &mut queue,
        ControlCommand::SendSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            len_bits: 7,
            payload: vec![0x41],
        },
    );

    assert!(!ok);
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_control_sds_latin_text_is_wrapped_with_single_text_coding_octet() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();
    let payload = b"HI".to_vec();

    let ok = sds.rx_sds_from_control(
        &mut queue,
        ControlCommand::SendSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            len_bits: (payload.len() * 8) as u16,
            payload,
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected D-SDS-DATA")];
    let (_, pdu) = extract_d_sds_data(&msgs);
    let SdsUserData::Type4(len_bits, data) = pdu.user_defined_data else {
        panic!("control SDS text should use SDS-TL Type4");
    };
    assert_eq!(len_bits as usize, data.len() * 8);
    assert_eq!(data[0], 0x82, "SDS-TL text protocol id");
    assert_eq!(data[1], 0x04, "existing SDS-TRANSFER report-request byte");
    assert_ne!(data[2], 0, "message reference should not be zero");
    assert_eq!(&data[3..], &[0x01, b'H', b'I']);
}

#[test]
fn test_control_sds_unicode_text_uses_sds_tl_utf16be_coding() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();
    let payload = "€".as_bytes().to_vec();

    let ok = sds.rx_sds_from_control(
        &mut queue,
        ControlCommand::SendSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            len_bits: (payload.len() * 8) as u16,
            payload,
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected D-SDS-DATA")];
    let (_, pdu) = extract_d_sds_data(&msgs);
    let SdsUserData::Type4(_, data) = pdu.user_defined_data else {
        panic!("control SDS text should use SDS-TL Type4");
    };
    // EN 300 392-2 table 29.29 uses 0x1A for ISO/IEC 10646-1 UCS-2.
    assert_eq!(&data[3..], &[0x1A, 0x20, 0xAC]);
}

#[test]
fn test_control_sds_all_ones_dest_uses_gssi_unacknowledged_no_report() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();
    let payload = b"BCAST".to_vec();

    let ok = sds.rx_sds_from_control(
        &mut queue,
        ControlCommand::SendSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 0x00FF_FFFF,
            dest_is_group: false,
            len_bits: (payload.len() * 8) as u16,
            payload,
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected D-SDS-DATA")];
    let (prim, pdu) = extract_d_sds_data(&msgs);
    assert_eq!(prim.main_address.ssi, 0x00FF_FFFF);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_address_ssi, Some(0x00FF_FFFF));
    let SdsUserData::Type4(_, data) = pdu.user_defined_data else {
        panic!("control SDS text should use SDS-TL Type4");
    };
    // EN 300 392-2 29.3.3.8.2: broadcast SDS uses no delivery report.
    assert_eq!(data[0], 0x82);
    assert_eq!(data[1], 0x00);
    assert_ne!(data[2], 0);
}

#[test]
fn test_control_sds_group_disables_delivery_report_request() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    affiliate_shared_subscriber(&shared_config, 2000001, 91);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();
    let payload = b"GROUP".to_vec();

    let ok = sds.rx_sds_from_control(
        &mut queue,
        ControlCommand::SendSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 91,
            dest_is_group: true,
            len_bits: (payload.len() * 8) as u16,
            payload,
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected D-SDS-DATA")];
    let (prim, pdu) = extract_d_sds_data(&msgs);
    assert_eq!(prim.main_address.ssi, 91);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_address_ssi, Some(9999));
    let SdsUserData::Type4(_, data) = pdu.user_defined_data else {
        panic!("control SDS text should use SDS-TL Type4");
    };
    assert_eq!(data[0], 0x82);
    assert_eq!(data[1], 0x00);
    assert_ne!(data[2], 0);
}

#[test]
fn test_control_sds_rejects_source_ssi_above_24_bits() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_sds_from_control(
        &mut queue,
        ControlCommand::SendSds {
            handle: 1,
            source_ssi: 0x0100_0000,
            dest_ssi: 2000001,
            dest_is_group: false,
            len_bits: 8,
            payload: vec![0x41],
        },
    );

    assert!(!ok);
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_control_sds_rejects_dest_ssi_above_24_bits() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_sds_from_control(
        &mut queue,
        ControlCommand::SendSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 0x0100_0000,
            dest_is_group: false,
            len_bits: 8,
            payload: vec![0x41],
        },
    );

    // EN 300 392-2 table 14.13 carries D-SDS-DATA SSI addresses in
    // 24-bit air-interface fields; reject before serializing.
    assert!(!ok);
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_control_sds_to_unknown_issi_is_rejected() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();
    let payload = b"HI".to_vec();

    let ok = sds.rx_sds_from_control(
        &mut queue,
        ControlCommand::SendSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            len_bits: (payload.len() * 8) as u16,
            payload,
        },
    );

    // EN 300 392-2 clauses 13.2 and 13.3.2.1 distinguish individual SDS
    // delivery. A control-origin ISSI destination must be registered locally
    // before the SwMI emits a D-SDS-DATA over RF.
    assert!(!ok);
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_control_sds_to_unknown_gssi_is_rejected() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();
    let payload = b"GROUP".to_vec();

    let ok = sds.rx_sds_from_control(
        &mut queue,
        ControlCommand::SendSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 91,
            dest_is_group: true,
            len_bits: (payload.len() * 8) as u16,
            payload,
        },
    );

    // EN 300 392-2 clause 13.2 includes group SDS, but local non-broadcast
    // GSSI delivery requires an affiliated listener. The predefined all-ones
    // GSSI remains the separate broadcast case.
    assert!(!ok);
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_control_sds_rejects_type4_payload_overflow_after_text_wrapper() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let payload = vec![0x41; 252];
    let ok = sds.rx_sds_from_control(
        &mut queue,
        ControlCommand::SendSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            len_bits: (payload.len() * 8) as u16,
            payload,
        },
    );

    assert!(!ok);
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_control_raw_sds_type1_status_like_payload() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            sdti: 0,
            len_bits: 16,
            payload: vec![0x82, 0x10],
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected raw D-SDS-DATA")];
    let (prim, pdu) = extract_d_sds_data(&msgs);
    assert_eq!(prim.main_address.ssi, 2000001);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert_eq!(pdu.calling_party_address_ssi, Some(9999));
    assert_eq!(pdu.user_defined_data, SdsUserData::Type1(0x8210));
}

#[test]
fn test_control_raw_sds_to_unknown_issi_is_rejected() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            sdti: 0,
            len_bits: 16,
            payload: vec![0x82, 0x10],
        },
    );

    // Keep raw SDS data aligned with D-STATUS routing: an individual RF
    // destination must be known from MM registration before control can emit
    // a D-SDS-DATA to that ISSI.
    assert!(!ok);
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_control_raw_sds_to_unknown_gssi_is_rejected() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 91,
            dest_is_group: true,
            sdti: 3,
            len_bits: 16,
            payload: vec![0xDC, 0xAA],
        },
    );

    assert!(!ok);
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_control_send_status_to_issi_emits_d_status_not_d_sds_data() {
    debug::setup_logging_verbose();

    let source_issi = 9999;
    let dest_issi = 2000001;
    let status = PreCodedStatus::try_from(0x8210).unwrap();
    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    shared_config.state_write().subscribers.register(dest_issi);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_status_command_from_control(
        &mut queue,
        ControlCommand::SendStatus {
            handle: 2,
            source_ssi: source_issi,
            dest_ssi: dest_issi,
            dest_is_group: false,
            status_number: status.into_raw(),
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected control D-STATUS")];
    let (prim, d_status) = extract_d_status(&msgs);
    assert_eq!(prim.main_address.ssi, dest_issi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert_eq!(d_status.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.calling_party_extension, None);
    assert_eq!(d_status.pre_coded_status, status);

    if let SapMsgInner::LcmcMleUnitdataReq(prim) = &msgs[0].msg {
        let mut sdu = prim.sdu.clone();
        assert!(
            DSdsData::from_bitbuf(&mut sdu).is_err(),
            "Control SendStatus must serialize D-STATUS, not D-SDS-DATA Type1"
        );
    }
}

#[test]
fn test_control_send_status_all_ones_status_number_emits_d_status() {
    debug::setup_logging_verbose();

    let source_issi = 9999;
    let dest_issi = 2000001;
    let status = PreCodedStatus::NetworkUserSpecific(0xFFFF);
    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    shared_config.state_write().subscribers.register(dest_issi);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_status_command_from_control(
        &mut queue,
        ControlCommand::SendStatus {
            handle: 2,
            source_ssi: source_issi,
            dest_ssi: dest_issi,
            dest_is_group: false,
            status_number: status.into_raw(),
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected control D-STATUS")];
    let (prim, d_status) = extract_d_status(&msgs);
    assert_eq!(prim.main_address.ssi, dest_issi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.pre_coded_status, status);

    if let SapMsgInner::LcmcMleUnitdataReq(prim) = &msgs[0].msg {
        let mut sdu = prim.sdu.clone();
        assert!(
            DSdsData::from_bitbuf(&mut sdu).is_err(),
            "Control SendStatus with status 0xFFFF must serialize D-STATUS, not D-SDS-DATA Type1"
        );
    }
}

#[test]
fn test_control_send_status_to_group_emits_unacknowledged_gssi_d_status() {
    debug::setup_logging_verbose();

    let source_issi = 9999;
    let member_issi = 2000001;
    let gssi = 100;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    {
        let mut state = shared_config.state_write();
        state.subscribers.register(member_issi);
        state.subscribers.affiliate(member_issi, gssi);
    }
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_status_command_from_control(
        &mut queue,
        ControlCommand::SendStatus {
            handle: 3,
            source_ssi: source_issi,
            dest_ssi: gssi,
            dest_is_group: true,
            status_number: status.into_raw(),
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected control group D-STATUS")];
    let (prim, d_status) = extract_d_status(&msgs);
    assert_eq!(prim.main_address.ssi, gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);

    // EN 300 392-2 clauses 13.2, 14.7.1.11 and 18.3.5.3.1: predefined
    // group status uses D-STATUS and unacknowledged TL-UNITDATA semantics,
    // not per-MS acknowledged Type1 SDS.
    assert_eq!(d_status.calling_party_address_ssi, Some(source_issi as u64));
    assert_eq!(d_status.pre_coded_status, status);
}

#[test]
fn test_control_send_status_to_all_ones_emits_unacknowledged_gssi_d_status() {
    debug::setup_logging_verbose();

    let source_issi = 9999;
    let broadcast_gssi = 0x00FF_FFFF;
    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_status_command_from_control(
        &mut queue,
        ControlCommand::SendStatus {
            handle: 4,
            source_ssi: source_issi,
            dest_ssi: broadcast_gssi,
            dest_is_group: false,
            status_number: status.into_raw(),
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected broadcast D-STATUS")];
    let (prim, d_status) = extract_d_status(&msgs);

    // EN 300 392-2 clauses 13.2 and 18.3.5.3.1: a pre-defined group
    // status addressed to the predefined all-ones group is not a per-MS
    // acknowledged transfer.
    assert_eq!(prim.main_address.ssi, broadcast_gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(d_status.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(d_status.calling_party_address_ssi, Some(broadcast_gssi as u64));
    assert_eq!(d_status.pre_coded_status, status);
}

#[test]
fn test_control_send_status_all_ones_dest_and_status_preserves_both_fields() {
    debug::setup_logging_verbose();

    let broadcast_gssi = 0x00FF_FFFF;
    let status = PreCodedStatus::NetworkUserSpecific(0xFFFF);
    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_status_command_from_control(
        &mut queue,
        ControlCommand::SendStatus {
            handle: 4,
            source_ssi: broadcast_gssi,
            dest_ssi: broadcast_gssi,
            dest_is_group: false,
            status_number: status.into_raw(),
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected all-ones broadcast D-STATUS")];
    let (prim, d_status) = extract_d_status(&msgs);

    // EN 300 392-2 clause 23.4.1.2.1 note 3 defines the 24-bit all-ones
    // destination as broadcast GSSI, while table 14.27 carries a separate
    // 16-bit pre-coded status. Preserve both all-ones fields independently.
    assert_eq!(prim.main_address.ssi, broadcast_gssi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(d_status.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(d_status.calling_party_address_ssi, Some(broadcast_gssi as u64));
    assert_eq!(d_status.pre_coded_status, status);
}

#[test]
fn test_control_send_status_to_unknown_issi_is_rejected() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_status_command_from_control(
        &mut queue,
        ControlCommand::SendStatus {
            handle: 4,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            status_number: 0x8210,
        },
    );

    assert!(!ok);
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_cmce_control_send_status_to_unknown_issi_returns_failed_status_response() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    let (dispatcher, endpoint) = make_control_link();
    let mut cmce = CmceBs::new(shared_config, None, Some(endpoint));
    let mut queue = MessageQueue::new();

    dispatcher.send(ControlCommand::SendStatus {
        handle: 76,
        source_ssi: 9999,
        dest_ssi: 2000001,
        dest_is_group: false,
        status_number: 0x8210,
    });
    cmce.tick_start(&mut queue, TdmaTime::default());

    // EN 300 392-2 clauses 13.2 and 18.3.5.3.1 keep D-STATUS as a local
    // air-interface delivery. A control-origin individual status to an
    // unregistered ISSI must fail without emitting D-STATUS or D-SDS-DATA.
    assert!(queue.pop_front().is_none());
    let response = dispatcher.try_recv_response().expect("expected failed status control response");
    assert!(matches!(
        response,
        ControlResponse::SendStatusResponse {
            handle: 76,
            success: false
        }
    ));
}

#[test]
fn test_control_raw_sds_type4_preserves_pid_and_non_byte_aligned_bits() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();
    let payload = vec![0xDC, 0b1010_1100, 0b1100_1111, 0x55];
    let expected_payload = vec![0xDC, 0b1010_1100, 0b1100_0000];

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            sdti: 3,
            len_bits: 20,
            payload: payload.clone(),
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected raw D-SDS-DATA")];
    let (_, pdu) = extract_d_sds_data(&msgs);
    assert_eq!(pdu.user_defined_data, SdsUserData::Type4(20, expected_payload));
}

#[test]
fn test_wap_mvp_text_variants_keep_exact_requested_message_and_type4_budget() {
    let requested = "Hello! You are running Nexus-BS. Gretings and 73! from Chris YO3TCO!";
    assert_eq!(WAP_MVP_MESSAGE_TEXT, requested);

    for (name, page_text) in [
        ("plain", WAP_MVP_MESSAGE_TEXT),
        ("dynamic WML", WAP_MVP_PAGE_TEXT),
        ("color/blink", WAP_MVP_COLOR_PAGE_TEXT),
    ] {
        assert_eq!(
            page_text.match_indices(requested).count(),
            1,
            "{name} WAP payload must carry the requested message exactly once"
        );
        let payload = wap_sds_type4_payload(page_text);
        assert_eq!(payload[0], WAP_WDP_PROTOCOL_ID, "{name} WAP payload must use direct WAP PID 0x04");
        assert_eq!(
            &payload[1..],
            page_text.as_bytes(),
            "{name} WAP payload body must be opaque page bytes"
        );
        assert!(
            payload.len() <= WAP_SDS_TYPE4_MAX_BYTE_ALIGNED_PAYLOAD_BYTES,
            "{name} WAP payload must fit byte-aligned SDS Type4"
        );
    }

    let sds_tl_payload = wap_sds_tl_transfer_type4_payload(WAP_MVP_PAGE_TEXT, 73);
    assert_eq!(sds_tl_payload[0], WAP_SDS_TL_PROTOCOL_ID);
    assert_eq!(sds_tl_payload[1], WAP_SDS_TL_TRANSFER_FLAGS_NO_REPORT);
    assert_eq!(sds_tl_payload[2], 73);
    assert!(
        sds_tl_payload.len() <= WAP_SDS_TYPE4_MAX_BYTE_ALIGNED_PAYLOAD_BYTES,
        "WAP/SDS-TL dynamic WML payload must also fit byte-aligned SDS Type4"
    );
}

#[test]
fn test_control_raw_sds_type4_delivers_exact_requested_wap_text() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();
    let payload = wap_sds_type4_payload(WAP_MVP_MESSAGE_TEXT);

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 73,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            sdti: 3,
            len_bits: (payload.len() * 8) as u16,
            payload: payload.clone(),
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected exact-text WAP D-SDS-DATA")];
    let (prim, pdu) = extract_d_sds_data(&msgs);
    // EN 300 392-2 table 29.21 PID 0x04 scopes this as WAP-over-SDS Type4.
    // This test covers the MVP delivery boundary only: direct WAP payload
    // bytes after the PID, not an SNDCP/IP WAP bearer.
    assert_eq!(prim.main_address.ssi, 2000001);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert_eq!(
        pdu.user_defined_data,
        SdsUserData::Type4((payload.len() * 8) as u16, payload.clone())
    );
    assert_eq!(payload[0], WAP_WDP_PROTOCOL_ID);
    assert_eq!(&payload[1..], WAP_MVP_MESSAGE_TEXT.as_bytes());
    assert_eq!(
        std::str::from_utf8(&payload[1..]).expect("MVP WAP text is ASCII"),
        "Hello! You are running Nexus-BS. Gretings and 73! from Chris YO3TCO!"
    );
}

#[test]
fn test_control_raw_sds_type4_delivers_wap_mvp_page() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();
    let payload = wap_sds_type4_payload(WAP_MVP_PAGE_TEXT);

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 73,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            sdti: 3,
            len_bits: (payload.len() * 8) as u16,
            payload: payload.clone(),
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected WAP MVP D-SDS-DATA")];
    let (prim, pdu) = extract_d_sds_data(&msgs);
    // EN 300 392-2 table 29.21 assigns PID 0x04 to WAP without SDS-TL
    // transfer service. Clause 29.5.8.2 leaves WAP application data to WAP
    // itself, so the raw Type4 command carries the WAP page body opaquely
    // after the PID.
    assert_eq!(prim.main_address.ssi, 2000001);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert_eq!(
        pdu.user_defined_data,
        SdsUserData::Type4((payload.len() * 8) as u16, payload.clone())
    );
    assert_eq!(payload[0], WAP_WDP_PROTOCOL_ID);
    assert_ne!(payload[0], WAP_SDS_TL_PROTOCOL_ID);
    assert_eq!(&payload[1..], WAP_MVP_PAGE_TEXT.as_bytes());
    assert_eq!(WAP_MVP_PAGE_TEXT.match_indices(WAP_MVP_MESSAGE_TEXT).count(), 1);
    assert!(WAP_MVP_PAGE_TEXT.contains("ontimer=\"#b\""));
    assert!(WAP_MVP_PAGE_TEXT.contains("ontimer=\"#a\""));
    assert!(WAP_MVP_PAGE_TEXT.contains("timer value=\"6\""));
    assert!(WAP_MVP_PAGE_TEXT.contains("*** FLASH ***"));
    assert!(payload.len() <= WAP_SDS_TYPE4_MAX_BYTE_ALIGNED_PAYLOAD_BYTES);
}

#[test]
fn test_control_raw_sds_type4_delivers_wap_color_blink_page_variant() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();
    let payload = wap_sds_type4_payload(WAP_MVP_COLOR_PAGE_TEXT);

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 74,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            sdti: 3,
            len_bits: (payload.len() * 8) as u16,
            payload: payload.clone(),
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected color WAP D-SDS-DATA")];
    let (prim, pdu) = extract_d_sds_data(&msgs);
    assert_eq!(prim.main_address.ssi, 2000001);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert_eq!(
        pdu.user_defined_data,
        SdsUserData::Type4((payload.len() * 8) as u16, payload.clone())
    );
    assert_eq!(payload[0], WAP_WDP_PROTOCOL_ID);
    assert_eq!(&payload[1..], WAP_MVP_COLOR_PAGE_TEXT.as_bytes());
    assert_eq!(WAP_MVP_COLOR_PAGE_TEXT.match_indices(WAP_MVP_MESSAGE_TEXT).count(), 1);
    assert!(WAP_MVP_COLOR_PAGE_TEXT.contains("bgcolor=\"#000\""));
    assert!(WAP_MVP_COLOR_PAGE_TEXT.contains("color=\"red\""));
    assert!(WAP_MVP_COLOR_PAGE_TEXT.contains("<blink>"));
    assert!(payload.len() <= WAP_SDS_TYPE4_MAX_BYTE_ALIGNED_PAYLOAD_BYTES);
}

#[test]
fn test_cmce_control_raw_sds_wap_mvp_returns_raw_sds_response() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let (dispatcher, endpoint) = make_control_link();
    let mut cmce = CmceBs::new(shared_config, None, Some(endpoint));
    let mut queue = MessageQueue::new();
    let payload = wap_sds_type4_payload(WAP_MVP_PAGE_TEXT);

    dispatcher.send(ControlCommand::SendRawSds {
        handle: 73,
        source_ssi: 9999,
        dest_ssi: 2000001,
        dest_is_group: false,
        sdti: 3,
        len_bits: (payload.len() * 8) as u16,
        payload: payload.clone(),
    });
    cmce.tick_start(&mut queue, TdmaTime::default());

    let msgs = vec![queue.pop_front().expect("expected CMCE-controlled WAP MVP D-SDS-DATA")];
    let (_, pdu) = extract_d_sds_data(&msgs);
    // EN 300 392-2 table 29.21 assigns PID 0x04 to WAP carried as raw
    // Type4 SDS user data. The control response is therefore raw-SDS scoped,
    // not a text-SDS response and not proof of an SNDCP/IP WAP bearer.
    assert_eq!(pdu.user_defined_data, SdsUserData::Type4((payload.len() * 8) as u16, payload));
    let response = dispatcher.try_recv_response().expect("expected raw SDS control response");
    assert!(matches!(
        response,
        ControlResponse::SendRawSdsResponse { handle: 73, success: true }
    ));
}

#[test]
fn test_cmce_control_raw_sds_wap_mvp_accepts_dashboard_all_ones_source() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2_260_616);
    let (dispatcher, endpoint) = make_control_link();
    let mut cmce = CmceBs::new(shared_config, None, Some(endpoint));
    let mut queue = MessageQueue::new();
    let payload = wap_sds_type4_payload(WAP_MVP_PAGE_TEXT);

    dispatcher.send(ControlCommand::SendRawSds {
        handle: 76,
        source_ssi: 0x00FF_FFFF,
        dest_ssi: 2_260_616,
        dest_is_group: false,
        sdti: 3,
        len_bits: (payload.len() * 8) as u16,
        payload: payload.clone(),
    });
    cmce.tick_start(&mut queue, TdmaTime::default());

    let msgs = vec![queue.pop_front().expect("expected dashboard-source WAP MVP D-SDS-DATA")];
    let (prim, pdu) = extract_d_sds_data(&msgs);

    // EN 300 392-2 table 29.21 assigns PID 0x04 to direct WAP-over-SDS Type4.
    // The dashboard uses the all-ones infrastructure source SSI; CMCE must
    // preserve that source and carry the WML page opaquely as raw SDS data.
    assert_eq!(prim.main_address.ssi, 2_260_616);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(pdu.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
    assert_eq!(pdu.calling_party_address_ssi, Some(0x00FF_FFFF));
    assert_eq!(pdu.user_defined_data, SdsUserData::Type4((payload.len() * 8) as u16, payload));

    let response = dispatcher
        .try_recv_response()
        .expect("expected dashboard-source raw SDS control response");
    assert!(matches!(
        response,
        ControlResponse::SendRawSdsResponse { handle: 76, success: true }
    ));
}

#[test]
fn test_cmce_control_raw_sds_wap_overflow_returns_failed_raw_sds_response() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let (dispatcher, endpoint) = make_control_link();
    let mut cmce = CmceBs::new(shared_config, None, Some(endpoint));
    let mut queue = MessageQueue::new();
    let payload = vec![WAP_WDP_PROTOCOL_ID; WAP_SDS_TYPE4_MAX_BYTE_ALIGNED_PAYLOAD_BYTES + 1];

    dispatcher.send(ControlCommand::SendRawSds {
        handle: 75,
        source_ssi: 9999,
        dest_ssi: 2000001,
        dest_is_group: false,
        sdti: 3,
        len_bits: (payload.len() * 8) as u16,
        payload,
    });
    cmce.tick_start(&mut queue, TdmaTime::default());

    // EN 300 392-2 Type4 SDS has an 11-bit length indicator. Byte-aligned
    // WAP payloads therefore top out at 255 octets including the PID.
    assert!(queue.pop_front().is_none());
    let response = dispatcher.try_recv_response().expect("expected failed raw SDS control response");
    assert!(matches!(
        response,
        ControlResponse::SendRawSdsResponse {
            handle: 75,
            success: false
        }
    ));
}

#[test]
fn test_control_raw_sds_type4_delivers_wap_sds_tl_transfer_page() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();
    let payload = wap_sds_tl_transfer_type4_payload(WAP_MVP_PAGE_TEXT, 73);

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 73,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            sdti: 3,
            len_bits: (payload.len() * 8) as u16,
            payload: payload.clone(),
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected WAP/SDS-TL D-SDS-DATA")];
    let (prim, pdu) = extract_d_sds_data(&msgs);
    // EN 300 392-2 table 29.21 assigns PID 0x84 to WAP with SDS-TL
    // transfer service. The payload therefore starts with the SDS-TRANSFER
    // flags and message-reference octets before the WAP application data.
    assert_eq!(prim.main_address.ssi, 2000001);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
    assert_eq!(
        pdu.user_defined_data,
        SdsUserData::Type4((payload.len() * 8) as u16, payload.clone())
    );
    assert_eq!(payload[0], WAP_SDS_TL_PROTOCOL_ID);
    assert_eq!(payload[1], WAP_SDS_TL_TRANSFER_FLAGS_NO_REPORT);
    assert_eq!(payload[2], 73);
    assert_eq!(&payload[3..], WAP_MVP_PAGE_TEXT.as_bytes());
    assert_eq!(WAP_MVP_PAGE_TEXT.match_indices(WAP_MVP_MESSAGE_TEXT).count(), 1);
    assert!(payload.len() <= WAP_SDS_TYPE4_MAX_BYTE_ALIGNED_PAYLOAD_BYTES);
}

#[test]
fn test_control_raw_sds_type4_accepts_direct_wap_and_wcmp_pids() {
    debug::setup_logging_verbose();

    for protocol_id in [0x04, 0x05] {
        let config = ComponentTest::get_default_test_config(StackMode::Bs);
        let shared_config = SharedConfig::from_parts(config, None);
        register_shared_subscriber(&shared_config, 2000001);
        let mut sds = SdsBsSubentity::new(shared_config);
        let mut queue = MessageQueue::new();
        let payload = vec![protocol_id, 0x00, 0x44, 0xAA];

        let ok = sds.rx_raw_sds_from_control(
            &mut queue,
            ControlCommand::SendRawSds {
                handle: 1,
                source_ssi: 9999,
                dest_ssi: 2000001,
                dest_is_group: false,
                sdti: 3,
                len_bits: 32,
                payload: payload.clone(),
            },
        );

        assert!(ok, "direct WAP/WCMP PID 0x{protocol_id:02X} should be raw-SDS deliverable");
        let msgs = vec![queue.pop_front().expect("expected raw WAP/WCMP D-SDS-DATA")];
        let (_, pdu) = extract_d_sds_data(&msgs);
        assert_eq!(pdu.user_defined_data, SdsUserData::Type4(32, payload), "PID 0x{protocol_id:02X}");
    }
}

#[test]
fn test_control_raw_sds_type4_rejects_zero_bit_payload_without_pid() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            sdti: 3,
            len_bits: 0,
            payload: Vec::new(),
        },
    );

    // EN 300 392-2 clauses 13.3.3 and 14.8.52: Type4 user-defined data
    // starts with an 8-bit protocol identifier, followed by 0..2039
    // protocol-dependent bits. A zero-bit Type4 payload cannot carry the PID.
    assert!(!ok);
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_control_raw_sds_type4_rejects_sub_pid_payload() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            sdti: 3,
            len_bits: 3,
            payload: vec![0b1010_1100],
        },
    );

    // A Type4 SDS may have a sub-octet protocol-dependent body, but not a
    // sub-octet total length: the first 8 bits are always the protocol ID.
    assert!(!ok);
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_control_raw_sds_rejects_invalid_sdti() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            sdti: 4,
            len_bits: 16,
            payload: vec![0x12, 0x34],
        },
    );

    assert!(!ok);
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_control_raw_sds_rejects_mismatched_fixed_lengths() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            sdti: 1,
            len_bits: 16,
            payload: vec![0x12, 0x34, 0x56, 0x78],
        },
    );

    assert!(!ok);
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_control_raw_sds_rejects_extra_fixed_type_bytes() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    register_shared_subscriber(&shared_config, 2000001);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 2000001,
            dest_is_group: false,
            sdti: 0,
            len_bits: 16,
            payload: vec![0x12, 0x34, 0x56],
        },
    );

    // EN 300 392-2 table 14.76 maps SDTI 0/1/2 to fixed User Defined
    // Data-1/2/3 fields; tables 14.87-14.89 define exact 16/32/64-bit
    // lengths. Do not silently ignore extra control-plane bytes.
    assert!(!ok);
    assert!(queue.pop_front().is_none());
}

#[test]
fn test_control_raw_sds_all_ones_dest_uses_gssi_unacknowledged_all_ones_source() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 0x00FF_FFFF,
            dest_is_group: false,
            sdti: 3,
            len_bits: 16,
            payload: vec![0xDC, 0xAA],
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected raw broadcast D-SDS-DATA")];
    let (prim, pdu) = extract_d_sds_data(&msgs);
    assert_eq!(prim.main_address.ssi, 0x00FF_FFFF);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_address_ssi, Some(0x00FF_FFFF));
    assert_eq!(pdu.user_defined_data, SdsUserData::Type4(16, vec![0xDC, 0xAA]));
}

#[test]
fn test_control_raw_sds_all_ones_clears_wap_sds_tl_delivery_report_request() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 0x00FF_FFFF,
            dest_is_group: false,
            sdti: 3,
            len_bits: 40,
            payload: vec![0x82, 0x04, 0x44, 0x01, b'A'],
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected raw broadcast D-SDS-DATA")];
    let (prim, pdu) = extract_d_sds_data(&msgs);
    assert_eq!(prim.main_address.ssi, 0x00FF_FFFF);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(pdu.calling_party_address_ssi, Some(0x00FF_FFFF));
    // EN 300 392-2 clause 29.3.3.8.2: system broadcast SDS-TL shall request
    // no delivery report. Preserve PID/MR/user data while clearing the report
    // request bits in the SDS-TRANSFER flags octet.
    assert_eq!(pdu.user_defined_data, SdsUserData::Type4(40, vec![0x82, 0x00, 0x44, 0x01, b'A']));
}

#[test]
fn test_control_raw_sds_all_ones_clears_sds_tl_delivery_report_request() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let shared_config = SharedConfig::from_parts(config, None);
    let mut sds = SdsBsSubentity::new(shared_config);
    let mut queue = MessageQueue::new();

    let ok = sds.rx_raw_sds_from_control(
        &mut queue,
        ControlCommand::SendRawSds {
            handle: 1,
            source_ssi: 9999,
            dest_ssi: 0x00FF_FFFF,
            dest_is_group: false,
            sdti: 3,
            len_bits: 32,
            payload: vec![WAP_SDS_TL_PROTOCOL_ID, 0x04, 0x44, 0xAA],
        },
    );

    assert!(ok);
    let msgs = vec![queue.pop_front().expect("expected raw broadcast SDS-TL D-SDS-DATA")];
    let (prim, pdu) = extract_d_sds_data(&msgs);
    assert_eq!(prim.main_address.ssi, 0x00FF_FFFF);
    assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(
        pdu.user_defined_data,
        SdsUserData::Type4(32, vec![WAP_SDS_TL_PROTOCOL_ID, 0x00, 0x44, 0xAA])
    );
}

#[test]
fn test_u_status_sds_tl_short_report_to_brew_maps_received_to_delivery_status_0x00() {
    // EN 300 392-2 table 29.23 short report type 2 = message received;
    // table 29.16 standard SDS-REPORT delivery status 0x00 = receipt acknowledged.
    assert_u_status_sds_tl_short_report_to_brew(0x7E44, 0x44, 0x00);
}

#[test]
fn test_u_status_sds_tl_short_report_to_brew_maps_consumed_to_delivery_status_0x02() {
    // EN 300 392-2 table 29.23 short report type 3 = message consumed;
    // table 29.16 standard SDS-REPORT delivery status 0x02 = consumed by destination.
    assert_u_status_sds_tl_short_report_to_brew(0x7F45, 0x45, 0x02);
}

#[test]
fn test_u_status_sds_tl_both_reports_preserve_context_for_second_report() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(brew_sds_enabled_config(), Some(dltime));
    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let protocol_id = 0xDC;
    let message_reference = 0x66;
    let reporting_issi = 1000001;
    let peer_issi = 5000001;
    register_subscriber(&mut test, reporting_issi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: peer_issi,
            dest_issi: reporting_issi,
            dest_ssi_type: None,
            user_defined_data: SdsUserData::Type4(40, vec![protocol_id, 0x0C, message_reference, 0x01, b'A']),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    assert_eq!(count_d_sds_data(&setup_msgs), 1, "expected D-SDS-DATA to local MS");

    // EN 300 392-2 clauses 29.3.2.2 and table 29.17 allow the originator to
    // request both received and consumed end-to-end reports for one message
    // reference. The first short report must not drop the PID context needed
    // to expand the second short report.
    test.submit_message(build_u_status_msg(
        reporting_issi,
        peer_issi,
        PreCodedStatus::from(0x7E00 | message_reference as u16),
    ));
    test.run_stack(Some(1));
    let received_msgs = test.dump_sinks();
    let received_brew = received_msgs
        .iter()
        .find_map(|m| match &m.msg {
            SapMsgInner::CmceSdsData(sds) if m.dest == TetraEntity::Brew => Some(sds),
            _ => None,
        })
        .expect("expected received short report to be forwarded to Brew");
    assert_eq!(
        received_brew.user_defined_data,
        SdsUserData::Type4(32, vec![protocol_id, 0x10, 0x00, message_reference])
    );

    test.submit_message(build_u_status_msg(
        reporting_issi,
        peer_issi,
        PreCodedStatus::from(0x7F00 | message_reference as u16),
    ));
    test.run_stack(Some(1));
    let consumed_msgs = test.dump_sinks();
    let consumed_brew = consumed_msgs
        .iter()
        .find_map(|m| match &m.msg {
            SapMsgInner::CmceSdsData(sds) if m.dest == TetraEntity::Brew => Some(sds),
            _ => None,
        })
        .expect("expected consumed short report to be forwarded to Brew");
    assert_eq!(
        consumed_brew.user_defined_data,
        SdsUserData::Type4(32, vec![protocol_id, 0x10, 0x02, message_reference])
    );
}

#[test]
fn test_u_status_sds_tl_short_report_to_brew_maps_dest_mem_full_to_delivery_status_0x52() {
    // EN 300 392-2 table 29.23 short report type 1 = destination memory full;
    // table 29.16 standard SDS-REPORT delivery status 0x52 = destination memory full.
    assert_u_status_sds_tl_short_report_to_brew(0x7D46, 0x46, 0x52);
}

#[test]
fn test_u_status_sds_tl_short_report_to_brew_maps_protocol_error_to_delivery_status_0x50() {
    // EN 300 392-2 table 29.23 short report type 0 combines protocol/encoding
    // not supported; table 29.16 separates protocol 0x50 and coding 0x51.
    assert_u_status_sds_tl_short_report_to_brew(0x7C47, 0x47, 0x50);
}

#[test]
fn test_u_status_sds_tl_short_report_to_brew_preserves_cached_vendor_pid() {
    // EN 300 392-2 table 29.21 reserves 0xC0..=0xFE for user application
    // definition in the SDS-TL protocol identifier space. A short report has
    // no PID field, so the SwMI must expand it with the PID from the matching
    // outbound transfer instead of assuming text PID 0x82.
    assert_u_status_sds_tl_short_report_to_brew_with_pid(0xDC, 0x7E44, 0x44, 0x00);
}

#[test]
fn test_u_status_sds_tl_short_report_context_is_bounded_and_evicts_oldest() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(brew_sds_enabled_config(), Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);

    let reporting_issi = 1000001;
    let first_peer = 5000001;
    let newest_peer = first_peer + 256;
    let protocol_id = 0xDC;
    let message_reference = 0x00;
    register_subscriber(&mut test, reporting_issi);

    for index in 0..=256 {
        let peer_issi = first_peer + index;
        test.submit_message(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceSdsData(CmceSdsData {
                source_issi: peer_issi,
                dest_issi: reporting_issi,
                dest_ssi_type: None,
                user_defined_data: SdsUserData::Type4(40, vec![protocol_id, 0x04, index as u8, 0x01, b'A']),
                tx_reporter: None,
            }),
        });
        test.run_stack(Some(1));
        let _ = test.dump_sinks();
    }

    // EN 300 392-2 clause 29.4.3.11 gives SDS-SHORT REPORT only an 8-bit
    // message reference, not the original Type4 PID. Keep the remembered PID
    // context bounded; after more than one full reference space, the oldest
    // context must not be used to fabricate an SDS-REPORT.
    test.submit_message(build_u_status_msg(
        reporting_issi,
        first_peer,
        PreCodedStatus::from(0x7E00 | message_reference as u16),
    ));
    test.run_stack(Some(1));
    let first_msgs = test.dump_sinks();
    let first_brew_msg = first_msgs
        .iter()
        .find_map(|m| match &m.msg {
            SapMsgInner::CmceSdsData(sds) if m.dest == TetraEntity::Brew && sds.dest_issi == first_peer => Some(sds),
            _ => None,
        })
        .expect("expected evicted short report to be forwarded to Brew");
    assert_eq!(first_brew_msg.user_defined_data, SdsUserData::Type1(0x7E00));

    test.submit_message(build_u_status_msg(
        reporting_issi,
        newest_peer,
        PreCodedStatus::from(0x7E00 | message_reference as u16),
    ));
    test.run_stack(Some(1));
    let newest_msgs = test.dump_sinks();
    let newest_brew_msg = newest_msgs
        .iter()
        .find_map(|m| match &m.msg {
            SapMsgInner::CmceSdsData(sds) if m.dest == TetraEntity::Brew && sds.dest_issi == newest_peer => Some(sds),
            _ => None,
        })
        .expect("expected newest short report to be forwarded to Brew");
    assert_eq!(
        newest_brew_msg.user_defined_data,
        SdsUserData::Type4(32, vec![protocol_id, 0x10, 0x00, message_reference])
    );
}

#[test]
fn test_u_status_sds_tl_short_report_without_pid_context_stays_precoded_for_brew() {
    // EN 300 392-2 clause 29.4.3.11: SDS-SHORT REPORT carries the message
    // reference, but not the Type4 Protocol Identifier. Without a matching
    // transfer context, fabricating PID 0x82 would corrupt non-text SDS-TL.
    assert_u_status_sds_tl_short_report_without_context_to_brew(0x7E44);
}

#[test]
fn test_u_status_short_report_ignores_non_sds_tl_pid_0x02_context() {
    // EN 300 392-2 clause 29.4.1 and table 29.21: PID 0x02 is simple text
    // messaging outside the SDS-TL transport range, so byte 1 must not be
    // interpreted as SDS-TL TRANSFER report-request flags.
    assert_u_status_sds_tl_short_report_ignores_non_sds_tl_pid_context(0x02);
}

#[test]
fn test_u_status_short_report_ignores_extension_pid_0xff_context() {
    // EN 300 392-2 table 29.21 reserves PID 0xFF for extension. This stack
    // does not implement extension PID parsing, so it must not cache 0xFF as
    // a concrete SDS-TL transport context for later STATUS conversion.
    assert_u_status_sds_tl_short_report_ignores_non_sds_tl_pid_context(0xFF);
}

#[test]
fn test_u_status_brew_forward() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(CfgBrew {
        host: "test.local".into(),
        port: 3000,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        feature_rssi_export: false,
        whitelisted_ssis: None,
    });
    let mut test = ComponentTest::from_config(config, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Only register source, NOT dest — should forward to Brew
    register_subscriber(&mut test, 1000001);

    let u_status = UStatus {
        area_selection: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_short_number_address: None,
        called_party_ssi: Some(5000001),
        called_party_extension: None,
        pre_coded_status: PreCodedStatus::from(0x8210),
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_status.to_bitbuf(&mut sdu).expect("Failed to serialize U-STATUS");
    sdu.seek(0);

    let msg = SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(1000001, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    };
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();

    // Should forward to Brew as CmceSdsData with Type1 payload
    let brew_count = count_brew_sds(&sink_msgs);
    assert_eq!(brew_count, 1, "Expected 1 CmceSdsData at Brew sink for U-STATUS");

    // Verify the payload is Type1 with the original pre-coded status value
    let brew_msg = sink_msgs.iter().find(|m| m.dest == TetraEntity::Brew).unwrap();
    if let SapMsgInner::CmceSdsData(ref sds) = brew_msg.msg {
        assert_eq!(sds.source_issi, 1000001);
        assert_eq!(sds.dest_issi, 5000001);
        assert_eq!(sds.user_defined_data, SdsUserData::Type1(0x8210));
    } else {
        panic!("Expected CmceSdsData message at Brew sink");
    }

    // Should NOT deliver locally
    let d_sds_count = count_d_sds_data(&sink_msgs);
    assert_eq!(d_sds_count, 0, "Should not deliver locally when dest is not registered");
}

#[test]
fn test_u_status_rejects_invalid_rf_source_before_brew_forwarding() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };

    for invalid_source in [
        TetraAddress::new(0x0100_0000, SsiType::Issi),
        TetraAddress::new(1000001, SsiType::Gssi),
        TetraAddress::new(1000001, SsiType::Unknown),
    ] {
        let mut test = ComponentTest::from_config(brew_sds_enabled_config(), Some(dltime));
        test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);

        // EN 300 392-2 clauses 13.2 and 14.7.2.8 define mobile-originated
        // status as an individual MS service. Reject invalid RF source
        // identities before converting/forwarding status to Brew SDS data.
        let msg = with_received_tetra_address(
            build_u_status_msg(invalid_source.ssi, 5000001, PreCodedStatus::from(0x8210)),
            invalid_source,
        );
        test.submit_message(msg);
        test.run_stack(Some(1));
        let sink_msgs = test.dump_sinks();

        assert_eq!(count_brew_sds(&sink_msgs), 0);
        assert_eq!(count_d_sds_data(&sink_msgs), 0);
    }
}

#[test]
fn test_u_status_rejects_unregistered_rf_source_before_local_or_brew_routing() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let dest_issi = 2000001;
    let status = PreCodedStatus::from(0x8210);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(brew_sds_enabled_config(), Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, dest_issi);

    // EN 300 392-2 clauses 13.3.2.1, 13.3.2.3 and 14.7.2.8 carry U-STATUS
    // as an MS-originated service request. A known destination is not enough:
    // the RF calling party must also be in the SwMI registration state.
    test.submit_message(build_u_status_msg(source_issi, dest_issi, status));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert_eq!(count_brew_sds(&sink_msgs), 0);
    assert_eq!(count_d_sds_data(&sink_msgs), 0);
}

#[test]
fn test_u_status_all_ones_status_to_brew_stays_type1_without_local_delivery() {
    debug::setup_logging_verbose();

    let source_issi = 1000001;
    let dest_issi = 5000001;
    let status = PreCodedStatus::NetworkUserSpecific(0xFFFF);
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(brew_sds_enabled_config(), Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, source_issi);

    test.submit_message(build_u_status_msg(source_issi, dest_issi, status));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let brew_msg = sink_msgs
        .iter()
        .find_map(|m| match &m.msg {
            SapMsgInner::CmceSdsData(sds) if m.dest == TetraEntity::Brew => Some(sds),
            _ => None,
        })
        .expect("expected non-local all-ones status to be forwarded to Brew");

    // EN 300 392-2 table 14.27 carries U-STATUS as a 16-bit pre-coded
    // status. Without SDS-TL short-report context, 0xffff remains Type1 and
    // must not be expanded into SDS-TL Type4 user data.
    assert_eq!(brew_msg.source_issi, source_issi);
    assert_eq!(brew_msg.dest_issi, dest_issi);
    assert_eq!(brew_msg.user_defined_data, SdsUserData::Type1(0xFFFF));
    assert_eq!(count_d_sds_data(&sink_msgs), 0);
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(_))),
        "non-local Brew-forwarded U-STATUS must not also deliver a local D-STATUS"
    );
}

#[test]
fn test_u_status_brew_forward_respects_feature_sds_enabled() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(CfgBrew {
        host: "test.local".into(),
        port: 3000,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: false,
        feature_rssi_export: false,
        whitelisted_ssis: None,
    });
    let mut test = ComponentTest::from_config(config, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, 1000001);

    // EN 300 392-2 U-STATUS/D-STATUS is part of the status/SDS service
    // surface. If the local Brew SDS bridge is disabled, this stack must not
    // route a non-local U-STATUS to Brew just because Brew itself is configured.
    let msg = build_u_status_msg(1000001, 5000001, PreCodedStatus::from(0x8210));
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    assert_eq!(count_brew_sds(&sink_msgs), 0);
    assert_eq!(count_d_sds_data(&sink_msgs), 0);
}

#[test]
fn test_u_status_unregistered_dest_dropped() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Only register source, NOT dest
    register_subscriber(&mut test, 1000001);

    let u_status = UStatus {
        area_selection: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_short_number_address: None,
        called_party_ssi: Some(9999999),
        called_party_extension: None,
        pre_coded_status: PreCodedStatus::from(0x8210),
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_status.to_bitbuf(&mut sdu).expect("Failed to serialize U-STATUS");
    sdu.seek(0);

    let msg = SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(1000001, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    };
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let d_status_count = count_d_sds_data(&sink_msgs);
    assert_eq!(d_status_count, 0, "Should not deliver D-STATUS when dest is not registered");
}
