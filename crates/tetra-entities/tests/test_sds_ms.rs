mod common;

use common::ComponentTest;
use tetra_config::bluestation::StackMode;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Sap, SsiType, TetraAddress, debug};
use tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier;
use tetra_pdus::cmce::enums::pre_coded_status::PreCodedStatus;
use tetra_pdus::cmce::pdus::d_sds_data::DSdsData;
use tetra_pdus::cmce::pdus::d_status::DStatus;
use tetra_saps::control::enums::sds_user_data::SdsUserData;
use tetra_saps::lcmc::LcmcMleUnitdataInd;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};

const LOCAL_ISSI: u32 = 1000001;
const LOCAL_GSSI: u32 = 3000003;
const NETWORK_SOURCE_ISSI: u32 = 2000002;

fn build_d_sds_data_msg(local_issi: u32, source_issi: u32, user_defined_data: SdsUserData) -> SapMsg {
    build_d_sds_data_msg_with_dest_type(local_issi, SsiType::Issi, source_issi, user_defined_data)
}

fn build_d_sds_data_msg_with_dest_type(
    local_issi: u32,
    dest_ssi_type: SsiType,
    source_issi: u32,
    user_defined_data: SdsUserData,
) -> SapMsg {
    let pdu = DSdsData {
        calling_party_type_identifier: PartyTypeIdentifier::Ssi,
        calling_party_address_ssi: Some(source_issi as u64),
        calling_party_extension: None,
        user_defined_data,
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(128);
    pdu.to_bitbuf(&mut sdu).expect("Failed to serialize D-SDS-DATA");
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
            received_tetra_address: TetraAddress::new(local_issi, dest_ssi_type),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_d_sds_data_tsi_msg(local_issi: u32, source_issi: u32, calling_party_extension: u32, user_defined_data: SdsUserData) -> SapMsg {
    let pdu = DSdsData {
        calling_party_type_identifier: PartyTypeIdentifier::Tsi,
        calling_party_address_ssi: Some(source_issi as u64),
        calling_party_extension: Some(calling_party_extension as u64),
        user_defined_data,
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(128);
    pdu.to_bitbuf(&mut sdu).expect("Failed to serialize D-SDS-DATA");
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
            received_tetra_address: TetraAddress::new(local_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_d_status_msg(local_issi: u32, source_issi: u32, pre_coded_status: PreCodedStatus) -> SapMsg {
    build_d_status_msg_with_dest_type(local_issi, SsiType::Issi, source_issi, pre_coded_status)
}

fn build_d_status_msg_with_dest_type(
    local_issi: u32,
    dest_ssi_type: SsiType,
    source_issi: u32,
    pre_coded_status: PreCodedStatus,
) -> SapMsg {
    let pdu = DStatus {
        calling_party_type_identifier: PartyTypeIdentifier::Ssi,
        calling_party_address_ssi: Some(source_issi as u64),
        calling_party_extension: None,
        pre_coded_status,
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    pdu.to_bitbuf(&mut sdu).expect("Failed to serialize D-STATUS");
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
            received_tetra_address: TetraAddress::new(local_issi, dest_ssi_type),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_d_status_tsi_msg(local_issi: u32, source_issi: u32, calling_party_extension: u32, pre_coded_status: PreCodedStatus) -> SapMsg {
    let pdu = DStatus {
        calling_party_type_identifier: PartyTypeIdentifier::Tsi,
        calling_party_address_ssi: Some(source_issi as u64),
        calling_party_extension: Some(calling_party_extension as u64),
        pre_coded_status,
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    pdu.to_bitbuf(&mut sdu).expect("Failed to serialize D-STATUS");
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
            received_tetra_address: TetraAddress::new(local_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

#[test]
fn test_ms_d_sds_data_is_delivered_to_user() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::User]);

    let payload = SdsUserData::Type4(16, vec![0x12, 0x34]);
    test.submit_message(build_d_sds_data_msg(LOCAL_ISSI, NETWORK_SOURCE_ISSI, payload.clone()));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let (msg, sds) = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceSdsData(sds) => Some((msg, sds)),
            _ => None,
        })
        .expect("D-SDS-DATA should be delivered to the MS user entity");

    // EN 300 392-2 clause 14.7.1.10: D-SDS-DATA carries user-defined SDS
    // data to the MS and expects no CMCE response.
    assert_eq!(msg.sap, Sap::TnsdsSap);
    assert_eq!(msg.src, TetraEntity::Cmce);
    assert_eq!(msg.dest, TetraEntity::User);
    assert_eq!(sds.source_issi, NETWORK_SOURCE_ISSI);
    assert_eq!(sds.dest_issi, LOCAL_ISSI);
    assert_eq!(sds.dest_ssi_type, Some(SsiType::Issi));
    assert_eq!(sds.user_defined_data, payload);
}

#[test]
fn test_ms_d_sds_data_group_destination_preserves_gssi_type() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::User]);

    let payload = SdsUserData::Type4(16, vec![0x12, 0x34]);
    test.submit_message(build_d_sds_data_msg_with_dest_type(
        LOCAL_GSSI,
        SsiType::Gssi,
        NETWORK_SOURCE_ISSI,
        payload.clone(),
    ));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let (msg, sds) = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceSdsData(sds) => Some((msg, sds)),
            _ => None,
        })
        .expect("group D-SDS-DATA should be delivered to the MS user entity");

    // EN 300 392-2 clause 13.2 distinguishes individual and group user-defined
    // short message reception. Keep the GSSI address type at TNSDS-UNITDATA.
    assert_eq!(msg.sap, Sap::TnsdsSap);
    assert_eq!(msg.src, TetraEntity::Cmce);
    assert_eq!(msg.dest, TetraEntity::User);
    assert_eq!(sds.source_issi, NETWORK_SOURCE_ISSI);
    assert_eq!(sds.dest_issi, LOCAL_GSSI);
    assert_eq!(sds.dest_ssi_type, Some(SsiType::Gssi));
    assert_eq!(sds.user_defined_data, payload);
}

#[test]
fn test_ms_d_sds_data_tsi_calling_party_is_not_collapsed_to_ssi() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::User]);

    let payload = SdsUserData::Type4(16, vec![0x12, 0x34]);
    test.submit_message(build_d_sds_data_tsi_msg(LOCAL_ISSI, NETWORK_SOURCE_ISSI, 0x12_3456, payload));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 14.7.1.10 and 13.3.2.3 preserve Calling Party
    // Extension for CPTI=TSI. The current User SAP container cannot carry it,
    // so SDS-MS must not rewrite TSI to a plain source_issi.
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::CmceSdsData(_))),
        "D-SDS-DATA with TSI calling party must not be delivered as plain SSI"
    );
}

#[test]
fn test_ms_d_status_is_delivered_to_user_as_status_indication() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::User]);

    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    test.submit_message(build_d_status_msg(LOCAL_ISSI, NETWORK_SOURCE_ISSI, status));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let (msg, sds) = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceSdsStatus(status) => Some((msg, status)),
            _ => None,
        })
        .expect("D-STATUS should be delivered to the MS user entity");

    // EN 300 392-2 clauses 13.3.2.1 and 14.7.1.11: D-STATUS carries a
    // pre-coded status to the MS user at TNSDS-STATUS, not user-defined
    // TNSDS-UNITDATA.
    assert_eq!(msg.sap, Sap::TnsdsSap);
    assert_eq!(msg.src, TetraEntity::Cmce);
    assert_eq!(msg.dest, TetraEntity::User);
    assert_eq!(sds.source_issi, NETWORK_SOURCE_ISSI);
    assert_eq!(sds.dest_issi, LOCAL_ISSI);
    assert_eq!(sds.dest_ssi_type, SsiType::Issi);
    assert_eq!(sds.status_number, status.into_raw());
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::CmceSdsData(_))),
        "D-STATUS must not be delivered as user-defined SDS data"
    );
}

#[test]
fn test_ms_d_status_group_destination_preserves_gssi_type() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::User]);

    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    test.submit_message(build_d_status_msg_with_dest_type(
        LOCAL_GSSI,
        SsiType::Gssi,
        NETWORK_SOURCE_ISSI,
        status,
    ));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let (msg, sds) = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceSdsStatus(status) => Some((msg, status)),
            _ => None,
        })
        .expect("group D-STATUS should be delivered to the MS user entity");

    // EN 300 392-2 clauses 13.2, 13.3.2.1 and 14.7.1.11 distinguish
    // individual and group pre-coded status reception. Keep the GSSI address
    // type at TNSDS-STATUS.
    assert_eq!(msg.sap, Sap::TnsdsSap);
    assert_eq!(msg.src, TetraEntity::Cmce);
    assert_eq!(msg.dest, TetraEntity::User);
    assert_eq!(sds.source_issi, NETWORK_SOURCE_ISSI);
    assert_eq!(sds.dest_issi, LOCAL_GSSI);
    assert_eq!(sds.dest_ssi_type, SsiType::Gssi);
    assert_eq!(sds.status_number, status.into_raw());
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::CmceSdsData(_))),
        "group D-STATUS must not be delivered as user-defined SDS data"
    );
}

#[test]
fn test_ms_d_status_all_ones_status_number_is_delivered_to_user() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::User]);

    let status = PreCodedStatus::NetworkUserSpecific(0xFFFF);
    test.submit_message(build_d_status_msg(LOCAL_ISSI, NETWORK_SOURCE_ISSI, status));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let (msg, sds) = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceSdsStatus(status) => Some((msg, status)),
            _ => None,
        })
        .expect("D-STATUS should be delivered to the MS user entity");

    // EN 300 392-2 tables 14.14 and 14.72 define Pre-coded status as a
    // 16-bit field, with 0xFFFF available for network/user definitions.
    // Keep it on the TNSDS-STATUS path, not as user-defined SDS Type1 data.
    assert_eq!(msg.sap, Sap::TnsdsSap);
    assert_eq!(msg.src, TetraEntity::Cmce);
    assert_eq!(msg.dest, TetraEntity::User);
    assert_eq!(sds.source_issi, NETWORK_SOURCE_ISSI);
    assert_eq!(sds.dest_issi, LOCAL_ISSI);
    assert_eq!(sds.dest_ssi_type, SsiType::Issi);
    assert_eq!(sds.status_number, status.into_raw());
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::CmceSdsData(_))),
        "D-STATUS 0xFFFF must not be delivered as user-defined SDS data"
    );
}

#[test]
fn test_ms_d_status_tsi_calling_party_is_not_collapsed_to_ssi() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::User]);

    let status = PreCodedStatus::NetworkUserSpecific(0x9001);
    test.submit_message(build_d_status_tsi_msg(LOCAL_ISSI, NETWORK_SOURCE_ISSI, 0x12_3456, status));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 14.7.1.11 and 13.3.2.1 preserve Calling Party
    // Extension for CPTI=TSI. The current User SAP container cannot carry it,
    // so SDS-MS must not rewrite TSI to a plain source_issi.
    assert!(
        sink_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::CmceSdsData(_) | SapMsgInner::CmceSdsStatus(_))),
        "D-STATUS with TSI calling party must not be delivered as plain SSI"
    );
}
