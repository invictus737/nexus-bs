mod common;

use std::collections::BTreeSet;

use tetra_config::bluestation::{CfgBrew, EnergySavingAssignment, StackConfig, StackMode, from_toml_str};
use tetra_core::ranges::SortedDisjointSsiRanges;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::typed_pdu_fields::Type3FieldGeneric;
use tetra_core::{BitBuffer, Layer2Service, Sap, SsiType, TdmaTime, TetraAddress, debug};
use tetra_entities::mm::mm_bs::MmBs;
use tetra_entities::net_dashboard::server::DashboardServer;
use tetra_entities::net_telemetry::{
    TelemetryEvent,
    channel::{TelemetrySource, telemetry_channel},
};
use tetra_pdus::mm::enums::energy_saving_mode::EnergySavingMode;
use tetra_pdus::mm::enums::location_update_accept_type::LocationUpdateAcceptType;
use tetra_pdus::mm::enums::location_update_type::LocationUpdateType;
use tetra_pdus::mm::enums::mm_pdu_type_dl::MmPduTypeDl;
use tetra_pdus::mm::enums::mm_pdu_type_ul::MmPduTypeUl;
use tetra_pdus::mm::enums::reject_cause::RejectCause;
use tetra_pdus::mm::enums::status_downlink::StatusDownlink;
use tetra_pdus::mm::enums::status_uplink::StatusUplink;
use tetra_pdus::mm::enums::type34_elem_id_ul::MmType34ElemIdUl;
use tetra_pdus::mm::fields::class_of_ms::ClassOfMs;
use tetra_pdus::mm::fields::group_identity_attachment::GroupIdentityAttachment;
use tetra_pdus::mm::fields::group_identity_downlink::GroupIdentityDownlink;
use tetra_pdus::mm::fields::group_identity_location_demand::GroupIdentityLocationDemand;
use tetra_pdus::mm::fields::group_identity_uplink::GroupIdentityUplink;
use tetra_pdus::mm::pdus::d_attach_detach_group_identity::DAttachDetachGroupIdentity;
use tetra_pdus::mm::pdus::d_attach_detach_group_identity_acknowledgement::DAttachDetachGroupIdentityAcknowledgement;
use tetra_pdus::mm::pdus::d_location_update_accept::DLocationUpdateAccept;
use tetra_pdus::mm::pdus::d_location_update_command::DLocationUpdateCommand;
use tetra_pdus::mm::pdus::d_location_update_reject::DLocationUpdateReject;
use tetra_pdus::mm::pdus::d_mm_status::DMmStatus;
use tetra_pdus::mm::pdus::mm_pdu_function_not_supported::MmPduFunctionNotSupported;
use tetra_pdus::mm::pdus::u_attach_detach_group_identity::UAttachDetachGroupIdentity;
use tetra_pdus::mm::pdus::u_attach_detach_group_identity_acknowledgement::UAttachDetachGroupIdentityAcknowledgement;
use tetra_pdus::mm::pdus::u_itsi_detach::UItsiDetach;
use tetra_pdus::mm::pdus::u_location_update_demand::ULocationUpdateDemand;
use tetra_pdus::mm::pdus::u_mm_status::UMmStatus;
use tetra_pdus::mm::pdus::u_tei_provide::UTeiProvide;
use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
use tetra_saps::lmm::{LmmMleReportInd, LmmMleUnitdataInd};
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};
use tetra_saps::tla::TLA_REPORT_FAILED_TRANSFER;

use crate::common::ComponentTest;

const LARGE_RESTART_RECOVERY_MEMBER_COUNT: u32 = 4096;

#[test]
fn test_u_mm_status_energy_saving() {
    // Motorola requesting power management (ChangeOfEnergySavingModeRequest)
    debug::setup_logging_verbose();
    let dltime_vec1 = TdmaTime::default().add_timeslots(2); // Downlink time: 0/1/1/3
    let issi = 2040814;

    // Setup testing stack
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime_vec1));
    let components = vec![TetraEntity::Mm];
    let sinks: Vec<TetraEntity> = vec![TetraEntity::Mle];
    test.populate_entities(components, sinks);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // Submit and process message
    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeRequest,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // Energy saving mode requests now get a D-MM-STATUS ChangeOfEnergySavingModeResponse
    assert_eq!(sink_msgs.len(), 1);

    // Parse the response and verify it's a D-MM-STATUS
    let SapMsgInner::LmmMleUnitdataReq(ref resp_prim) = sink_msgs[0].msg else {
        panic!("Expected LmmMleUnitdataReq");
    };
    let mut resp_sdu = BitBuffer::from_bitstr(&resp_prim.sdu.to_bitstr());
    let resp_pdu = DMmStatus::from_bitbuf(&mut resp_sdu).expect("Failed parsing D-MM-STATUS response");
    assert_eq!(
        resp_pdu.status_downlink,
        tetra_pdus::mm::enums::status_downlink::StatusDownlink::ChangeOfEnergySavingModeResponse
    );
    let esi = resp_pdu
        .energy_saving_information
        .expect("D-MM-STATUS response must carry energy saving information");
    assert_eq!(esi.energy_saving_mode, EnergySavingMode::StayAlive);
    assert_eq!(esi.frame_number, None);
    assert_eq!(esi.multiframe_number, None);
}

#[test]
fn test_u_mm_status_energy_saving_request_activates_configured_eg_when_enabled() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.energy_saving_mode = EnergySavingMode::Eg3 as u8;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    // Register the MS while explicitly staying alive so this test isolates the
    // MS-initiated U-CHANGE OF ENERGY SAVING MODE REQUEST path.
    submit_location_update(&mut test, issi, Some(EnergySavingMode::StayAlive));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 clause 16.7.1 allows the BS to allocate a different
    // energy economy mode than requested. Clauses 16.10.9/16.10.10 require the
    // allocated mode plus frame/multiframe start point in the response.
    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeRequest,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::LmmMleUnitdataReq(ref resp_prim) = sink_msgs[0].msg else {
        panic!("Expected LmmMleUnitdataReq");
    };
    let mut resp_sdu = BitBuffer::from_bitstr(&resp_prim.sdu.to_bitstr());
    let resp_pdu = DMmStatus::from_bitbuf(&mut resp_sdu).expect("Failed parsing D-MM-STATUS response");
    assert_eq!(resp_pdu.status_downlink, StatusDownlink::ChangeOfEnergySavingModeResponse);
    let esi = resp_pdu
        .energy_saving_information
        .expect("D-MM-STATUS response must carry allocated energy saving information");
    assert_eq!(esi.energy_saving_mode, EnergySavingMode::Eg3);
    assert!(esi.frame_number.is_some());
    assert!(esi.multiframe_number.is_some());
    assert_ne!(esi.frame_number, Some(18));

    let state = test.config.state_read();
    let assignment = state
        .energy_saving
        .get(&issi)
        .expect("MS-requested EG should activate configured assignment after D-MM-STATUS response");
    assert_eq!(assignment.mode, EnergySavingMode::Eg3 as u8);
    assert_eq!(assignment.frame, esi.frame_number);
    assert_eq!(assignment.multiframe, esi.multiframe_number);
    assert!(assignment.awake_until.is_some());
}

#[test]
fn test_u_mm_status_energy_saving_request_does_not_allocate_frame_18_start() {
    debug::setup_logging_verbose();
    let issi_that_would_spread_to_frame_18 = 17;

    for mode in energy_economy_modes_for_test() {
        let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
        config.cell.energy_saving_mode = mode as u8;

        let mut test = ComponentTest::from_config(
            config,
            Some(dltime_for_frame_18_energy_start(issi_that_would_spread_to_frame_18, mode)),
        );
        test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

        submit_location_update(&mut test, issi_that_would_spread_to_frame_18, Some(EnergySavingMode::StayAlive));
        test.run_stack(Some(1));
        let _ = test.dump_sinks();

        // EN 300 392-2 clauses 16.7.1, 16.10.9 and 16.10.10 carry the
        // negotiated energy economy mode and start point in D-MM-STATUS. Clause
        // 23.7.6 derives the continuing receive cycle from that start point, so
        // the MS-initiated change path must use the same frame-18 guard as the
        // registration and BS-initiated paths.
        submit_u_mm_status_energy_saving(
            &mut test,
            issi_that_would_spread_to_frame_18,
            StatusUplink::ChangeOfEnergySavingModeRequest,
            EnergySavingMode::Eg1,
        );
        test.run_stack(Some(1));
        let status_msgs = test.dump_sinks();

        let status = extract_d_mm_status(&status_msgs);
        assert_eq!(status.status_downlink, StatusDownlink::ChangeOfEnergySavingModeResponse);
        let esi = status
            .energy_saving_information
            .expect("D-MM-STATUS response must carry allocated energy saving information");
        assert_eq!(esi.energy_saving_mode, mode);
        assert_energy_saving_start_avoids_frame_18(mode, esi.frame_number, esi.multiframe_number);

        let state = test.config.state_read();
        let assignment = state
            .energy_saving
            .get(&issi_that_would_spread_to_frame_18)
            .expect("MS-requested EG assignment should activate");
        assert_eq!(assignment.mode, mode as u8);
        assert_eq!(assignment.frame, esi.frame_number);
        assert_eq!(assignment.multiframe, esi.multiframe_number);
    }
}

#[test]
fn test_u_mm_status_stay_alive_request_clears_active_energy_saving_assignment() {
    debug::setup_logging_verbose();
    let issi = 2040815;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.energy_saving_mode = EnergySavingMode::Eg3 as u8;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update(&mut test, issi, Some(EnergySavingMode::StayAlive));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeRequest,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));
    let eg_msgs = test.dump_sinks();
    let eg_status = extract_d_mm_status(&eg_msgs);
    let eg_esi = eg_status
        .energy_saving_information
        .expect("EG response must carry energy saving information");
    assert_eq!(eg_status.status_downlink, StatusDownlink::ChangeOfEnergySavingModeResponse);
    assert_eq!(eg_esi.energy_saving_mode, EnergySavingMode::Eg3);
    assert!(
        test.config.state_read().energy_saving.contains_key(&issi),
        "configured EG should be active before the StayAlive request"
    );

    // EN 300 392-2 clauses 16.7.1, 16.10.9 and 16.10.10 allow an MS to
    // request StayAlive. Per clause 23.7.6, StayAlive ends the MAC sleep cycle
    // immediately; the registration itself remains valid.
    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeRequest,
        EnergySavingMode::StayAlive,
    );
    test.run_stack(Some(1));
    let stay_alive_msgs = test.dump_sinks();
    let stay_alive_status = extract_d_mm_status(&stay_alive_msgs);
    assert_eq!(stay_alive_status.status_downlink, StatusDownlink::ChangeOfEnergySavingModeResponse);
    let stay_alive_esi = stay_alive_status
        .energy_saving_information
        .expect("StayAlive response must carry energy saving information");
    assert_eq!(stay_alive_esi.energy_saving_mode, EnergySavingMode::StayAlive);
    assert_eq!(stay_alive_esi.frame_number, None);
    assert_eq!(stay_alive_esi.multiframe_number, None);

    let state = test.config.state_read();
    assert!(
        !state.energy_saving.contains_key(&issi),
        "StayAlive request must clear the active EG assignment"
    );
    assert!(
        state.subscribers.is_registered(issi),
        "exiting energy economy must not deregister the MS"
    );
}

#[test]
fn test_u_mm_status_energy_saving_accepts_type3_absent_terminator() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 tables 16.20/16.21 carry the 3-bit energy saving mode
    // followed by optional Type 3 Proprietary. A zero m-bit after the mode
    // explicitly terminates the absent Type 3 list.
    let dep_info = (EnergySavingMode::Eg1 as u64) << 1;
    submit_u_mm_status(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeRequest,
        Some(dep_info),
        Some(4),
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let status = extract_d_mm_status(&sink_msgs);
    assert_eq!(status.status_downlink, StatusDownlink::ChangeOfEnergySavingModeResponse);
}

#[test]
fn test_u_mm_status_energy_saving_rejects_malformed_trailing_dependent_data() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // Same leading EnergySavingMode::Eg1 bits as a valid request, but the
    // trailing bit is an unterminated Type 3 m-bit with no field identifier.
    let dep_info = ((EnergySavingMode::Eg1 as u64) << 1) | 1;
    submit_u_mm_status(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeRequest,
        Some(dep_info),
        Some(4),
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        sink_msgs.is_empty(),
        "malformed U-CHANGE OF ENERGY SAVING MODE REQUEST must not emit D-MM STATUS"
    );
    assert!(
        test.config.state_read().energy_saving.get(&issi).is_none(),
        "malformed U-MM STATUS must not activate energy saving state"
    );
}

#[test]
fn test_unsupported_u_mm_status_returns_function_not_supported_with_sub_pdu_type() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    // EN 300 392-2 clause 16.9.3.5.1 note 3 and table 16.27 note 2:
    // when U-MM STATUS is recognized but its status-uplink sub-PDU is not
    // supported, MM PDU/FUNCTION NOT SUPPORTED carries that sub-PDU selector.
    submit_u_mm_status(&mut test, issi, StatusUplink::DualWatchModeRequest, None, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let unsupported = extract_mm_pdu_function_not_supported(&sink_msgs);
    assert_eq!(
        unsupported.not_supported_pdu_type,
        tetra_pdus::mm::enums::mm_pdu_type_ul::MmPduTypeUl::UMmStatus as u8
    );
    assert_eq!(
        unsupported.not_supported_sub_pdu_type,
        Some((6, StatusUplink::DualWatchModeRequest.into()))
    );
    assert_eq!(
        extract_mm_pdu_function_not_supported_layer2service(&sink_msgs),
        Layer2Service::Acknowledged
    );
    assert!(!test.config.state_read().subscribers.is_registered(issi));
}

#[test]
fn test_unsupported_non_security_mm_pdu_types_return_function_not_supported_without_registration() {
    debug::setup_logging_verbose();

    for (idx, pdu_type) in [MmPduTypeUl::UInformationProvide, MmPduTypeUl::UDisableStatus]
        .into_iter()
        .enumerate()
    {
        let issi = 2040814 + idx as u32;
        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
        test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

        submit_raw_mm_pdu_type(&mut test, issi, pdu_type);
        test.run_stack(Some(1));
        let sink_msgs = test.dump_sinks();

        // EN 300 392-2 clause 16.8.8 and table 16.27 allow the SwMI to
        // answer an individually addressed unsupported MM PDU with
        // MM PDU/FUNCTION NOT SUPPORTED. These are whole-PDU non-security
        // cases, so no sub-PDU selector is present.
        let unsupported = extract_mm_pdu_function_not_supported(&sink_msgs);
        assert_eq!(unsupported.not_supported_pdu_type, pdu_type.into_raw() as u8);
        assert_eq!(unsupported.not_supported_sub_pdu_type, None);
        assert_eq!(
            extract_mm_pdu_function_not_supported_layer2service(&sink_msgs),
            Layer2Service::Acknowledged
        );
        assert!(
            !test.config.state_read().subscribers.is_registered(issi),
            "{pdu_type:?} must not synthesize registration"
        );
        assert!(subscriber_updates(&sink_msgs).is_empty());

        let response_address = sink_msgs
            .iter()
            .find_map(|msg| match &msg.msg {
                SapMsgInner::LmmMleUnitdataReq(prim) => {
                    let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                    MmPduFunctionNotSupported::from_bitbuf(&mut sdu).ok().map(|_| prim.address)
                }
                _ => None,
            })
            .expect("expected addressed MM PDU/FUNCTION NOT SUPPORTED");
        assert_eq!(response_address, TetraAddress::issi(issi));
    }
}

#[test]
fn test_non_issi_unsupported_mm_pdu_drops_without_function_not_supported() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    for received_address in invalid_mm_source_addresses(issi) {
        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
        test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

        // EN 300 392-2 clause 16.8.8 allows MM PDU/FUNCTION NOT SUPPORTED
        // only for an individually addressed MM PDU. Non-ISSI RF sources are
        // rejected at the source-address guard and must not receive an MM
        // response or synthesize local registration state.
        submit_raw_mm_pdu_type_with_received_address(&mut test, MmPduTypeUl::UInformationProvide, received_address);
        test.run_stack(Some(1));
        let sink_msgs = test.dump_sinks();

        assert!(
            sink_msgs.is_empty(),
            "non-ISSI unsupported MM PDU source {received_address} should be dropped"
        );
        assert!(!test.config.state_read().subscribers.is_registered(issi));
        assert!(subscriber_updates(&sink_msgs).is_empty());
    }
}

#[test]
fn test_stale_stay_alive_response_does_not_clear_active_energy_saving_assignment() {
    debug::setup_logging_verbose();
    let issi = 2040816;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.energy_saving_mode = EnergySavingMode::Eg3 as u8;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update(&mut test, issi, Some(EnergySavingMode::StayAlive));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeRequest,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    {
        let state = test.config.state_read();
        let assignment = state
            .energy_saving
            .get(&issi)
            .expect("MS-requested EG should be active before stale response");
        assert_eq!(assignment.mode, EnergySavingMode::Eg3 as u8);
    }

    // EN 300 392-2 clause 16.7.1 makes U-CHANGE OF ENERGY SAVING MODE
    // RESPONSE the answer to a SwMI D-CHANGE request. Without a pending SwMI
    // request, a StayAlive response is stale and must not cancel active EG.
    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::StayAlive,
    );
    test.run_stack(Some(1));

    assert_eq!(
        test.dump_sinks().len(),
        0,
        "stale StayAlive response should not produce a downlink status"
    );
    let state = test.config.state_read();
    let assignment = state
        .energy_saving
        .get(&issi)
        .expect("stale StayAlive response must not clear active EG assignment");
    assert_eq!(assignment.mode, EnergySavingMode::Eg3 as u8);
}

#[test]
fn test_mm_energy_saving_reconfiguration_preserves_assigned_channel_suspension() {
    debug::setup_logging_verbose();
    let issi = 2040817;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.energy_saving_mode = EnergySavingMode::Eg1 as u8;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update(&mut test, issi, Some(EnergySavingMode::Eg1));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    {
        let mut state = test.config.state_write();
        let assignment = state
            .energy_saving
            .get_mut(&issi)
            .expect("registration-carried EG assignment should be active");
        assignment.suspension_count = 2;
    }

    // EN 300 392-2 clause 23.7.6 suspends the sleep cycle while an MS obeys an
    // assigned channel or is active in a call. MM energy-economy renegotiation
    // must not clear that MAC-owned suspension counter.
    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeRequest,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));

    let state = test.config.state_read();
    let assignment = state
        .energy_saving
        .get(&issi)
        .expect("MM-requested EG assignment should remain active");
    assert_eq!(assignment.mode, EnergySavingMode::Eg1 as u8);
    assert_eq!(assignment.suspension_count, 2);
}

#[test]
fn test_unknown_u_mm_status_energy_saving_does_not_create_phantom_assignment() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeRequest,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));

    assert_eq!(test.dump_sinks().len(), 0);
    assert!(!test.config.state_read().energy_saving.contains_key(&issi));
}

#[test]
fn test_unknown_u_tei_provide_does_not_create_phantom_mm_state() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_u_tei_provide(&mut test, issi, 0x1234_5678_9abc_de);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    assert_eq!(sink_msgs.len(), 0);
    assert!(subscriber_updates(&sink_msgs).is_empty());
    {
        let state = test.config.state_read();
        assert!(!state.subscribers.is_registered(issi));
        assert!(!state.energy_saving.contains_key(&issi));
    }
    assert_eq!(debug_mm_client_tei(&mut test, issi), None);
}

#[test]
fn test_known_u_tei_provide_updates_only_registered_client_tei() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let tei = 0x1234_5678_9abc_de;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert!(test.config.state_read().subscribers.is_registered(issi));
    assert_eq!(debug_mm_client_tei(&mut test, issi), Some(None));

    submit_u_tei_provide(&mut test, issi, tei);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    assert_eq!(sink_msgs.len(), 0);
    assert!(subscriber_updates(&sink_msgs).is_empty());
    {
        let state = test.config.state_read();
        assert!(state.subscribers.is_registered(issi));
        assert!(!state.energy_saving.contains_key(&issi));
    }
    assert_eq!(debug_mm_client_tei(&mut test, issi), Some(Some(tei)));
}

#[test]
fn test_u_location_update_demand_energy_saving_gets_stay_alive() {
    assert_location_update_response_stay_alive(Some(EnergySavingMode::Eg1));
}

#[test]
fn test_u_location_update_demand_without_energy_saving_gets_stay_alive() {
    assert_location_update_response_stay_alive(None);
}

#[test]
fn test_location_update_accept_preserves_supported_request_type() {
    debug::setup_logging_verbose();
    let supported_types = [
        LocationUpdateType::RoamingLocationUpdating,
        LocationUpdateType::PeriodicLocationUpdating,
        LocationUpdateType::ItsiAttach,
        LocationUpdateType::ServiceRestorationRoamingLocationUpdating,
        LocationUpdateType::DemandLocationUpdating,
    ];

    for (idx, location_update_type) in supported_types.into_iter().enumerate() {
        let issi = 2041000 + idx as u32;
        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
        test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

        submit_location_update_with_type(&mut test, issi, location_update_type, None);
        test.run_stack(Some(1));
        let sink_msgs = test.dump_sinks();

        let accept = sink_msgs
            .iter()
            .find_map(|msg| match &msg.msg {
                SapMsgInner::LmmMleUnitdataReq(prim) => {
                    let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                    DLocationUpdateAccept::from_bitbuf(&mut sdu).ok()
                }
                _ => None,
            })
            .expect("expected D-LOCATION UPDATE ACCEPT");

        assert_eq!(
            accept.location_update_accept_type,
            expected_location_update_accept_type(location_update_type)
        );
        assert_eq!(
            accept.location_update_accept_type.into_raw(),
            location_update_type.into_raw(),
            "D-LOCATION UPDATE ACCEPT must preserve the raw U-LOCATION UPDATE DEMAND type"
        );
    }
}

#[test]
fn test_location_update_accept_preserves_type_with_energy_saving_request() {
    debug::setup_logging_verbose();
    let supported_types = [
        LocationUpdateType::RoamingLocationUpdating,
        LocationUpdateType::PeriodicLocationUpdating,
        LocationUpdateType::ItsiAttach,
        LocationUpdateType::ServiceRestorationRoamingLocationUpdating,
        LocationUpdateType::DemandLocationUpdating,
    ];

    for (idx, location_update_type) in supported_types.into_iter().enumerate() {
        let issi = 2042000 + idx as u32;
        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
        test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

        // EN 300 392-2 clauses 16.10.35a and 16.7.1 are independent fields:
        // answering an energy-saving request with safe-default StayAlive must
        // not rewrite the location update accept type.
        submit_location_update_with_type(&mut test, issi, location_update_type, Some(EnergySavingMode::Eg1));
        test.run_stack(Some(1));
        let sink_msgs = test.dump_sinks();
        let accept = extract_location_update_accept(&sink_msgs);

        assert_eq!(
            accept.location_update_accept_type,
            expected_location_update_accept_type(location_update_type)
        );
        assert_eq!(
            accept.location_update_accept_type.into_raw(),
            location_update_type.into_raw(),
            "D-LOCATION UPDATE ACCEPT must preserve the raw U-LOCATION UPDATE DEMAND type when ESI is present"
        );
        let esi = accept
            .energy_saving_information
            .expect("energy saving request should be answered in D-LOCATION UPDATE ACCEPT");
        assert_eq!(esi.energy_saving_mode, EnergySavingMode::StayAlive);
        assert_eq!(esi.frame_number, None);
        assert_eq!(esi.multiframe_number, None);
    }
}

#[test]
fn test_location_update_accept_scch_frame18_distribution_uses_slot1_for_class_of_ms() {
    debug::setup_logging_verbose();

    let issi = 2043000;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    let mut pdu = base_location_update_demand(LocationUpdateType::ItsiAttach, None);
    pdu.class_of_ms = Some(ClassOfMs {
        freq_simplex_duplex: true,
        multislot_phase_mod: false,
        concurrent_multicarrier: false,
        voice: true,
        e2e_encryption_not_supported: true,
        circuit_mode_data: false,
        tetra_packet_data: false,
        fast_switching: false,
        dck_encryption: false,
        clch_needed: true,
        concurrent_circuit_mode: false,
        original_advanced_link: false,
        minimum_mode: true,
        carrier_specific_signalling: false,
        authentication: false,
        sck_encryption: false,
        air_interface_version: 0,
        common_scch: true,
        reserved_21: false,
        mac_d_blck: false,
        extended_advanced_link: false,
        d8psk: false,
    });

    submit_location_update_demand_with_handle(&mut test, issi, pdu, 0);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let accept = extract_location_update_accept(&sink_msgs);

    // EN 300 392-2 clauses 16.10.46 and 16.10.8: table 16.90 encodes
    // SCCH information in the upper 4 bits and distribution in the lower
    // 2 bits; distribution 00 means frame-18 time slot 1.
    assert_eq!(accept.scch_information_and_distribution_on_18th_frame, Some(0x00));
}

#[test]
fn test_bs_initiated_energy_saving_stays_pending_until_ms_response() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.energy_saving_mode = EnergySavingMode::Eg1 as u8;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert_eq!(sink_msgs.len(), 2);
    let SapMsgInner::LmmMleUnitdataReq(ref accept_prim) = sink_msgs[0].msg else {
        panic!("Expected D-LOCATION UPDATE ACCEPT");
    };
    let mut accept_sdu = BitBuffer::from_bitstr(&accept_prim.sdu.to_bitstr());
    let accept = DLocationUpdateAccept::from_bitbuf(&mut accept_sdu).expect("Failed parsing D-LOCATION UPDATE ACCEPT");
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::ItsiAttach);
    assert!(accept.energy_saving_information.is_none());

    let SapMsgInner::LmmMleUnitdataReq(ref status_prim) = sink_msgs[1].msg else {
        panic!("Expected D-MM STATUS");
    };
    let mut status_sdu = BitBuffer::from_bitstr(&status_prim.sdu.to_bitstr());
    let status = DMmStatus::from_bitbuf(&mut status_sdu).expect("Failed parsing D-MM STATUS");
    assert_eq!(status.status_downlink, StatusDownlink::ChangeOfEnergySavingModeRequest);
    let esi = status
        .energy_saving_information
        .expect("D-MM STATUS request must carry energy saving information");
    assert_eq!(esi.energy_saving_mode, EnergySavingMode::Eg1);
    assert!(esi.frame_number.is_some());
    assert!(esi.multiframe_number.is_some());

    assert!(
        !test.config.state_read().energy_saving.contains_key(&issi),
        "BS-initiated EG must not become active before the MS response"
    );

    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));

    let state = test.config.state_read();
    let assignment = state
        .energy_saving
        .get(&issi)
        .expect("matching U-CHANGE response must activate pending EG assignment");
    assert_eq!(assignment.mode, EnergySavingMode::Eg1 as u8);
    assert_eq!(assignment.frame, esi.frame_number);
    assert_eq!(assignment.multiframe, esi.multiframe_number);
}

#[test]
fn test_example_config_default_eg3_stays_pending_until_ms_response() {
    debug::setup_logging_verbose();
    let issi = 2040819;

    let config_toml = include_str!("../../../example_config/config.toml");
    let config = from_toml_str(config_toml).expect("example config should parse");
    assert_eq!(
        config.cell.energy_saving_mode,
        EnergySavingMode::Eg3 as u8,
        "example config must exercise the Nexus-BS EG3 operator default"
    );

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&sink_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::ItsiAttach);
    assert!(
        accept.energy_saving_information.is_none(),
        "BS-initiated EG3 allocation must not be marked accepted in D-LOCATION UPDATE ACCEPT before MS response"
    );

    let status = extract_d_mm_status(&sink_msgs);
    assert_eq!(status.status_downlink, StatusDownlink::ChangeOfEnergySavingModeRequest);
    let esi = status
        .energy_saving_information
        .expect("BS-initiated D-MM STATUS request must carry EG3 energy saving information");
    assert_eq!(esi.energy_saving_mode, EnergySavingMode::Eg3);
    assert_energy_saving_start_avoids_frame_18(esi.energy_saving_mode, esi.frame_number, esi.multiframe_number);

    // EN 300 392-2 clause 16.7.1 allows BS-initiated energy economy changes,
    // while clauses 16.10.9/16.10.10 carry the requested mode/start point. The
    // MS response is what activates the assignment; until then lower layers
    // must keep scheduling this MS as continuously reachable.
    assert!(
        !test.config.state_read().energy_saving.contains_key(&issi),
        "default EG3 must stay pending until the MS accepts the BS-initiated request"
    );

    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::Eg3,
    );
    test.run_stack(Some(1));
    assert_eq!(test.dump_sinks().len(), 0);

    let state = test.config.state_read();
    let assignment = state
        .energy_saving
        .get(&issi)
        .expect("matching U-CHANGE response must activate the example-config EG3 assignment");
    assert_eq!(assignment.mode, EnergySavingMode::Eg3 as u8);
    assert_eq!(assignment.frame, esi.frame_number);
    assert_eq!(assignment.multiframe, esi.multiframe_number);
}

#[test]
fn test_plain_location_update_does_not_restart_same_active_energy_saving_assignment() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.energy_saving_mode = EnergySavingMode::Eg1 as u8;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();
    let status = extract_d_mm_status(&initial_msgs);
    let initial_esi = status
        .energy_saving_information
        .expect("BS-initiated D-MM STATUS must carry energy saving information");

    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 clause 16.7.1 lets the BS allocate/change EG with
    // D-MM STATUS and clause 16.11.1.2 starts T352 for that request. If the
    // same valid EG assignment is already active, a plain location update must
    // not create a duplicate BS-initiated request whose later T352 expiry could
    // clear the working assignment.
    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let reattach_msgs = test.dump_sinks();
    assert!(
        !reattach_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::LmmMleUnitdataReq(prim) if {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                DMmStatus::from_bitbuf(&mut sdu)
                    .map(|status| status.status_downlink == StatusDownlink::ChangeOfEnergySavingModeRequest)
                    .unwrap_or(false)
            })),
        "plain LU for an already-active EG assignment must not queue another D-MM STATUS request"
    );

    test.run_stack(Some((30 * 18 * 4 + 1) as usize));
    let _ = test.dump_sinks();
    let state = test.config.state_read();
    let assignment = state
        .energy_saving
        .get(&issi)
        .expect("active EG assignment must survive the duplicate-LU window");
    assert_eq!(assignment.mode, EnergySavingMode::Eg1 as u8);
    assert_eq!(assignment.frame, initial_esi.frame_number);
    assert_eq!(assignment.multiframe, initial_esi.multiframe_number);
}

#[test]
fn test_registration_energy_request_supersedes_pending_bs_initiated_energy_assignment() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.energy_saving_mode = EnergySavingMode::Eg1 as u8;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let pending_msgs = test.dump_sinks();

    let SapMsgInner::LmmMleUnitdataReq(ref status_prim) = pending_msgs[1].msg else {
        panic!("Expected D-MM STATUS");
    };
    let mut status_sdu = BitBuffer::from_bitstr(&status_prim.sdu.to_bitstr());
    let status = DMmStatus::from_bitbuf(&mut status_sdu).expect("Failed parsing D-MM STATUS");
    let pending_esi = status
        .energy_saving_information
        .expect("D-MM STATUS request must carry energy saving information");

    test.run_stack(Some(4));
    assert_eq!(test.dump_sinks().len(), 0);

    // EN 300 392-2 clauses 16.7.1 and 16.10.10: registration-carried energy
    // saving allocation includes its own absolute start point. It supersedes a
    // previous BS-initiated D-CHANGE request that has not been answered yet.
    submit_location_update(&mut test, issi, Some(EnergySavingMode::Eg1));
    test.run_stack(Some(1));
    let accept_msgs = test.dump_sinks();
    let accept = extract_location_update_accept(&accept_msgs);
    let accepted_esi = accept
        .energy_saving_information
        .expect("D-LOCATION UPDATE ACCEPT must answer requested energy saving mode");
    assert_ne!(
        (accepted_esi.frame_number, accepted_esi.multiframe_number),
        (pending_esi.frame_number, pending_esi.multiframe_number),
        "test setup must create a newer EG start point"
    );

    {
        let state = test.config.state_read();
        let assignment = state
            .energy_saving
            .get(&issi)
            .expect("registration-carried energy saving must activate immediately");
        assert_eq!(assignment.frame, accepted_esi.frame_number);
        assert_eq!(assignment.multiframe, accepted_esi.multiframe_number);
    }

    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));
    assert_eq!(test.dump_sinks().len(), 0);

    let state = test.config.state_read();
    let assignment = state
        .energy_saving
        .get(&issi)
        .expect("stale U-CHANGE response must not clear active assignment");
    assert_eq!(assignment.frame, accepted_esi.frame_number);
    assert_eq!(assignment.multiframe, accepted_esi.multiframe_number);
}

#[test]
fn test_bs_initiated_energy_saving_response_after_t352_expiry_keeps_stay_alive() {
    debug::setup_logging_verbose();
    let issi = 2040815;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.energy_saving_mode = EnergySavingMode::Eg1 as u8;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let SapMsgInner::LmmMleUnitdataReq(ref status_prim) = sink_msgs[1].msg else {
        panic!("Expected D-MM STATUS");
    };
    let mut status_sdu = BitBuffer::from_bitstr(&status_prim.sdu.to_bitstr());
    let status = DMmStatus::from_bitbuf(&mut status_sdu).expect("Failed parsing D-MM STATUS");
    assert_eq!(status.status_downlink, StatusDownlink::ChangeOfEnergySavingModeRequest);

    // EN 300 392-2 clauses 16.7.3 and 16.11.1.2 define T352 as the 30 s
    // energy-mode response timer. After expiry, a late response must not
    // activate the stale BS-initiated EG assignment.
    test.run_stack(Some((30 * 18 * 4 + 1) as usize));
    assert_eq!(test.dump_sinks().len(), 0);
    assert!(
        !test.config.state_read().energy_saving.contains_key(&issi),
        "T352 expiry must leave the MS in StayAlive until a fresh negotiation"
    );

    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));

    assert_eq!(
        test.dump_sinks().len(),
        0,
        "late U-CHANGE response should not produce a downlink status"
    );
    let state = test.config.state_read();
    assert!(
        state.subscribers.is_registered(issi),
        "T352 expiry must not disturb registration state"
    );
    assert!(
        !state.energy_saving.contains_key(&issi),
        "late U-CHANGE response after T352 expiry must not activate EG"
    );
}

#[test]
fn test_bs_initiated_energy_saving_replacement_t352_expiry_preserves_previous_active_assignment() {
    debug::setup_logging_verbose();
    let issi = 2040816;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.energy_saving_mode = EnergySavingMode::Eg2 as u8;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    let previous_assignment = EnergySavingAssignment {
        mode: EnergySavingMode::Eg1 as u8,
        frame: Some(3),
        multiframe: Some(1),
        awake_until: Some(TdmaTime { t: 1, f: 3, m: 1, h: 0 }),
        suspension_count: 0,
    };
    test.config.state_write().energy_saving.insert(issi, previous_assignment);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let status = extract_d_mm_status(&sink_msgs);
    assert_eq!(status.status_downlink, StatusDownlink::ChangeOfEnergySavingModeRequest);
    let requested_esi = status
        .energy_saving_information
        .expect("BS-initiated replacement request must carry energy saving information");
    assert_eq!(requested_esi.energy_saving_mode, EnergySavingMode::Eg2);

    // EN 300 392-2 clause 16.7.1 permits a BS-initiated EG change and
    // clause 16.7.3 makes T352 expiry a failure of the requested service.
    // A failed replacement negotiation must not erase the previously
    // negotiated EG cycle, which remains valid within the RA per clause 23.7.6.
    test.run_stack(Some((30 * 18 * 4 + 1) as usize));
    assert_eq!(test.dump_sinks().len(), 0);

    let state = test.config.state_read();
    let assignment = state
        .energy_saving
        .get(&issi)
        .expect("T352 expiry of an EG replacement must preserve the previous active assignment");
    assert_eq!(assignment.mode, previous_assignment.mode);
    assert_eq!(assignment.frame, previous_assignment.frame);
    assert_eq!(assignment.multiframe, previous_assignment.multiframe);
    assert_eq!(assignment.awake_until, previous_assignment.awake_until);
    assert_eq!(assignment.suspension_count, previous_assignment.suspension_count);
    drop(state);

    assert_eq!(
        debug_mm_client_energy(&mut test, issi),
        Some((EnergySavingMode::Eg1, previous_assignment.frame, previous_assignment.multiframe)),
        "MM client state must also return to the previous EG assignment"
    );
}

#[test]
fn test_bs_initiated_energy_saving_replacement_mismatched_response_preserves_previous_active_assignment() {
    debug::setup_logging_verbose();
    let issi = 2040817;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.energy_saving_mode = EnergySavingMode::Eg2 as u8;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    let previous_assignment = EnergySavingAssignment {
        mode: EnergySavingMode::Eg1 as u8,
        frame: Some(5),
        multiframe: Some(1),
        awake_until: Some(TdmaTime { t: 1, f: 5, m: 1, h: 0 }),
        suspension_count: 0,
    };
    test.config.state_write().energy_saving.insert(issi, previous_assignment);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let status = extract_d_mm_status(&sink_msgs);
    assert_eq!(status.status_downlink, StatusDownlink::ChangeOfEnergySavingModeRequest);
    let requested_esi = status
        .energy_saving_information
        .expect("BS-initiated replacement request must carry energy saving information");
    assert_eq!(requested_esi.energy_saving_mode, EnergySavingMode::Eg2);

    // EN 300 392-2 clause 16.7.1 says the MS response to a BS-initiated
    // change uses the same requested energy mode or StayAlive rejection. A
    // mismatched response fails only this replacement negotiation; it must not
    // erase the previous active EG cycle still valid in the current RA.
    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));
    assert_eq!(test.dump_sinks().len(), 0);

    let state = test.config.state_read();
    let assignment = state
        .energy_saving
        .get(&issi)
        .expect("mismatched EG replacement response must preserve the previous active assignment");
    assert_eq!(assignment.mode, previous_assignment.mode);
    assert_eq!(assignment.frame, previous_assignment.frame);
    assert_eq!(assignment.multiframe, previous_assignment.multiframe);
    assert_eq!(assignment.awake_until, previous_assignment.awake_until);
    assert_eq!(assignment.suspension_count, previous_assignment.suspension_count);
    drop(state);

    assert_eq!(
        debug_mm_client_energy(&mut test, issi),
        Some((EnergySavingMode::Eg1, previous_assignment.frame, previous_assignment.multiframe)),
        "MM client state must continue advertising the previous EG assignment"
    );

    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::Eg2,
    );
    test.run_stack(Some(1));
    assert_eq!(test.dump_sinks().len(), 0);

    let state = test.config.state_read();
    let assignment = state
        .energy_saving
        .get(&issi)
        .expect("stale matching response after mismatch must not replace the restored assignment");
    assert_eq!(assignment.mode, previous_assignment.mode);
    assert_eq!(assignment.frame, previous_assignment.frame);
    assert_eq!(assignment.multiframe, previous_assignment.multiframe);
}

#[test]
fn test_bs_initiated_energy_saving_stay_alive_rejection_clears_previous_active_assignment() {
    debug::setup_logging_verbose();
    let issi = 2040818;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.energy_saving_mode = EnergySavingMode::Eg2 as u8;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    let previous_assignment = EnergySavingAssignment {
        mode: EnergySavingMode::Eg1 as u8,
        frame: Some(5),
        multiframe: Some(1),
        awake_until: Some(TdmaTime { t: 1, f: 5, m: 1, h: 0 }),
        suspension_count: 0,
    };
    test.config.state_write().energy_saving.insert(issi, previous_assignment);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let status = extract_d_mm_status(&sink_msgs);
    assert_eq!(status.status_downlink, StatusDownlink::ChangeOfEnergySavingModeRequest);
    assert_eq!(
        status
            .energy_saving_information
            .expect("BS-initiated replacement request must carry energy saving information")
            .energy_saving_mode,
        EnergySavingMode::Eg2
    );

    // EN 300 392-2 clause 16.7.1 allows the MS to reject a BS-initiated
    // energy economy change by responding StayAlive. Honour that as the
    // terminal's current mode instead of restoring an older EG assignment.
    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::StayAlive,
    );
    test.run_stack(Some(1));
    assert_eq!(test.dump_sinks().len(), 0);

    assert!(
        !test.config.state_read().energy_saving.contains_key(&issi),
        "StayAlive rejection must clear any previous active EG assignment"
    );
    assert_eq!(
        debug_mm_client_energy(&mut test, issi),
        Some((EnergySavingMode::StayAlive, None, None)),
        "MM client state must track the terminal's StayAlive rejection"
    );
}

#[test]
fn test_mismatched_bs_initiated_energy_saving_response_clears_pending_and_keeps_stay_alive() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.energy_saving_mode = EnergySavingMode::Eg1 as u8;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let SapMsgInner::LmmMleUnitdataReq(ref status_prim) = sink_msgs[1].msg else {
        panic!("Expected D-MM STATUS");
    };
    let mut status_sdu = BitBuffer::from_bitstr(&status_prim.sdu.to_bitstr());
    let status = DMmStatus::from_bitbuf(&mut status_sdu).expect("Failed parsing D-MM STATUS");
    assert_eq!(status.status_downlink, StatusDownlink::ChangeOfEnergySavingModeRequest);
    let esi = status
        .energy_saving_information
        .expect("D-MM STATUS request must carry energy saving information");
    assert_eq!(esi.energy_saving_mode, EnergySavingMode::Eg1);

    // EN 300 392-2 clauses 16.7.1/16.10.9/16.10.10 make the BS-initiated
    // response mode part of a single negotiation. A mismatched response must
    // clear the pending assignment so a later stale matching response cannot
    // activate EG after the negotiation has failed.
    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::Eg2,
    );
    test.run_stack(Some(1));
    assert_eq!(test.dump_sinks().len(), 0);
    assert!(
        !test.config.state_read().energy_saving.contains_key(&issi),
        "mismatched U-CHANGE response must keep the MS in StayAlive"
    );

    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));
    assert_eq!(test.dump_sinks().len(), 0);
    assert!(
        !test.config.state_read().energy_saving.contains_key(&issi),
        "stale matching response after mismatch must not reactivate cleared pending EG"
    );
}

#[test]
fn test_stale_energy_saving_response_for_known_issi_keeps_stay_alive() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert!(test.config.state_read().subscribers.is_registered(issi));

    // EN 300 392-2 clauses 16.7.1/16.10.9/16.10.10 negotiate EG state.
    // A response without a matching SwMI request is stale and must not activate EG.
    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));

    assert_eq!(
        test.dump_sinks().len(),
        0,
        "stale U-CHANGE response should not produce a downlink status"
    );
    let state = test.config.state_read();
    assert!(
        !state.energy_saving.contains_key(&issi),
        "stale U-CHANGE response must not create an EG assignment"
    );
    assert!(
        state.subscribers.is_registered(issi),
        "stale U-CHANGE response must not disturb attach state"
    );
}

#[test]
fn test_periodic_registration_command_uses_last_l2_handle() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let handle = 0x4321;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.periodic_registration_secs = 1;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update_with_type_and_handle(&mut test, issi, LocationUpdateType::ItsiAttach, None, handle);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    std::thread::sleep(std::time::Duration::from_millis(1100));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let (command_prim, command_pdu) = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                let pdu_type = sdu.read_field(4, "pdu_type").ok()?;
                if pdu_type != MmPduTypeDl::DLocationUpdateCommand.into_raw() {
                    return None;
                }

                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                let pdu = DLocationUpdateCommand::from_bitbuf(&mut sdu).ok()?;
                Some((prim, pdu))
            }
            _ => None,
        })
        .expect("expected D-LOCATION UPDATE COMMAND after periodic registration expiry");

    assert_eq!(
        command_prim.handle, handle,
        "ETSI 16.9.2.8 command must be sent on the stored L2 handle for this MS"
    );
    assert!(
        command_pdu.group_identity_report,
        "ETSI 16.9.2.8 group identity report request should stay enabled for periodic registration"
    );
    assert!(!command_pdu.cipher_control);
    assert_eq!(command_pdu.ciphering_parameters, None);
    assert_eq!(command_pdu.address_extension, None);
    assert_eq!(command_pdu.cell_type_control, None);
    assert_eq!(command_pdu.proprietary, None);
}

#[test]
fn test_demand_location_update_after_periodic_command_reregisters_shared_subscriber_before_group_affiliation() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let gssi = 3001;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.periodic_registration_secs = 1;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![gssi]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert!(test.config.state_read().subscribers.is_registered(issi));
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    backdate_mm_registration(&mut test, issi, 2);
    test.run_stack(Some(1));
    let command_msgs = test.dump_sinks();
    assert!(contains_location_update_command(&command_msgs));
    {
        let state = test.config.state_read();
        assert!(
            !state.subscribers.is_registered(issi),
            "first periodic expiry removes the shared subscriber while waiting for DemandLocationUpdating"
        );
        assert!(state.subscribers.group_members(gssi).is_empty());
    }

    // EN 300 392-2 clauses 16.9.2.8 and 16.9.3.4: the MS answers a
    // D-LOCATION-UPDATE-COMMAND with U-LOCATION-UPDATE-DEMAND. If the SwMI
    // accepts DemandLocationUpdating, shared subscriber state must exist before
    // group identity location demand is applied.
    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::DemandLocationUpdating, vec![gssi]);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&demand_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(issi));
    assert_eq!(state.subscribers.group_members(gssi), vec![issi]);
}

#[test]
fn test_demand_location_update_after_periodic_command_without_group_report_reaffiliates_cached_groups() {
    debug::setup_logging_verbose();
    let issi = 2040815;
    let gssi = 3002;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.periodic_registration_secs = 1;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![gssi]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    backdate_mm_registration(&mut test, issi, 2);
    test.run_stack(Some(1));
    let command_msgs = test.dump_sinks();
    assert!(contains_location_update_command(&command_msgs));
    assert!(!test.config.state_read().subscribers.is_registered(issi));

    // Some terminals return from coverage believing persistent group
    // affiliations are still valid and send DemandLocationUpdating without a
    // group report. Keep the cached groups coherent with the re-created shared
    // registration instead of ACKing while the local registry remains empty.
    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DemandLocationUpdating, None);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&demand_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(issi));
    assert_eq!(state.subscribers.group_members(gssi), vec![issi]);
}

#[test]
fn test_group_less_coverage_return_publishes_dashboard_group_snapshot() {
    debug::setup_logging_verbose();
    let issi = 2040815;
    let gssi = 3002;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.periodic_registration_secs = 1;

    let (mut test, telemetry) = mm_test_with_telemetry(config);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![gssi]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    let _ = drain_telemetry(&telemetry);
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    backdate_mm_registration(&mut test, issi, 2);
    test.run_stack(Some(1));
    let command_msgs = test.dump_sinks();
    assert!(contains_location_update_command(&command_msgs));
    assert!(!test.config.state_read().subscribers.is_registered(issi));
    let _ = drain_telemetry(&telemetry);

    // EN 300 392-2 clauses 16.9.2.8/16.9.3.4 let the MS answer the
    // BS-commanded location update without repeating its group list. Clause
    // 16.8.0 keeps the already accepted persistent group identity valid until
    // a real detach/replacement. When MM restores that local affiliation for
    // CMCE, dashboard telemetry must also receive the final group list.
    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DemandLocationUpdating, None);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&demand_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    let events = drain_telemetry(&telemetry);
    assert!(
        events.iter().any(|event| matches!(
            event,
            TelemetryEvent::MsGroupsSnapshot {
                issi: event_issi,
                gssis
            } if *event_issi == issi && gssis == &vec![gssi]
        )),
        "group-less coverage return must publish a final dashboard group snapshot, got {events:?}"
    );
    assert_eq!(dashboard_groups_after(&events, issi), vec![gssi]);
}

#[test]
fn test_standalone_group_attach_after_periodic_command_restores_shared_subscriber_before_affiliation() {
    debug::setup_logging_verbose();
    let issi = 2040816;
    let initial_group = 3003;
    let new_group = 3004;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.periodic_registration_secs = 1;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![initial_group]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert!(test.config.state_read().subscribers.is_registered(issi));
    assert_eq!(test.config.state_read().subscribers.group_members(initial_group), vec![issi]);

    backdate_mm_registration(&mut test, issi, 2);
    test.run_stack(Some(1));
    let command_msgs = test.dump_sinks();
    assert!(contains_location_update_command(&command_msgs));
    {
        let state = test.config.state_read();
        assert!(
            !state.subscribers.is_registered(issi),
            "first periodic expiry keeps the MM client known but clears shared routing state"
        );
        assert!(state.subscribers.group_members(initial_group).is_empty());
    }

    // EN 300 392-2 clauses 16.4.3 and 16.8.2 make standalone group
    // attach/detach an MM procedure for an attached MS. After the local
    // periodic watchdog command from clause 16.9.2.8, a still-known client in
    // the grace window must be restored in the shared registry before the
    // accepted group affiliation is advertised.
    submit_attach_detach_group_identity(&mut test, issi, false, Some(vec![new_group]));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);
    assert_eq!(ack.group_identity_accept_reject, 0);
    let downlink = ack
        .group_identity_downlink
        .expect("accepted standalone attach should acknowledge the requested group");
    assert_eq!(downlink.len(), 1);
    assert_eq!(downlink[0].gssi, Some(new_group));
    assert!(downlink[0].group_identity_attachment.is_some());

    let updates = subscriber_updates(&sink_msgs);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].action, BrewSubscriberAction::Register);
    assert_eq!(updates[0].issi, issi);
    assert!(updates[0].groups.is_empty());
    assert_eq!(updates[1].action, BrewSubscriberAction::Affiliate);
    assert_eq!(updates[1].issi, issi);
    assert_eq!(updates[1].groups, vec![new_group]);

    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(issi));
    assert!(state.subscribers.group_members(initial_group).is_empty());
    assert_eq!(state.subscribers.group_members(new_group), vec![issi]);
}

#[test]
fn test_bs_initiated_energy_saving_does_not_allocate_frame_18_start() {
    debug::setup_logging_verbose();
    let issi_that_would_spread_to_frame_18 = 17;

    for mode in energy_economy_modes_for_test() {
        let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
        config.cell.energy_saving_mode = mode as u8;

        let mut test = ComponentTest::from_config(
            config,
            Some(dltime_for_frame_18_energy_start(issi_that_would_spread_to_frame_18, mode)),
        );
        test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

        submit_location_update(&mut test, issi_that_would_spread_to_frame_18, None);
        test.run_stack(Some(1));
        let sink_msgs = test.dump_sinks();

        let SapMsgInner::LmmMleUnitdataReq(ref status_prim) = sink_msgs[1].msg else {
            panic!("Expected D-MM STATUS");
        };
        let mut status_sdu = BitBuffer::from_bitstr(&status_prim.sdu.to_bitstr());
        let status = DMmStatus::from_bitbuf(&mut status_sdu).expect("Failed parsing D-MM STATUS");
        let esi = status
            .energy_saving_information
            .expect("D-MM STATUS request must carry energy saving information");

        // EN 300 392-2 clauses 16.7.1 and 16.10.10 make the ESI frame/MF
        // the negotiated start point, and clause 23.7.6 makes that start point
        // the first receive frame before the sleeping cycle. Clause 23.5.2.2.7
        // requires the BS to send downlink PDUs where the MS should listen, so
        // a stack with no SCH/F scheduling on frame 18 must not allocate that
        // frame as the start or recurring receive frame.
        assert_energy_saving_start_avoids_frame_18(mode, esi.frame_number, esi.multiframe_number);
    }
}

#[test]
fn test_registration_energy_saving_accept_does_not_allocate_frame_18_start() {
    debug::setup_logging_verbose();
    let issi_that_would_spread_to_frame_18 = 17;

    for mode in energy_economy_modes_for_test() {
        let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
        config.cell.energy_saving_mode = mode as u8;

        let mut test = ComponentTest::from_config(
            config,
            Some(dltime_for_frame_18_energy_start(issi_that_would_spread_to_frame_18, mode)),
        );
        test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

        submit_location_update(&mut test, issi_that_would_spread_to_frame_18, Some(mode));
        test.run_stack(Some(1));
        let sink_msgs = test.dump_sinks();

        let accept = extract_location_update_accept(&sink_msgs);
        assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::ItsiAttach);
        let esi = accept
            .energy_saving_information
            .expect("D-LOCATION UPDATE ACCEPT must carry requested energy saving information");
        assert_eq!(esi.energy_saving_mode, mode);

        // EN 300 392-2 clauses 16.7.1/16.10.10 allow the registration accept
        // path to carry the same energy economy start point as D-MM STATUS.
        // Per clause 23.7.6 and timer T.210, the MS remains awake through the
        // negotiated start and later returns to this receive cycle after
        // signalling activity; keep the cycle away from this stack's unscheduled
        // frame 18 SCH/F resources.
        assert_energy_saving_start_avoids_frame_18(mode, esi.frame_number, esi.multiframe_number);

        let state = test.config.state_read();
        let assignment = state
            .energy_saving
            .get(&issi_that_would_spread_to_frame_18)
            .expect("registration-carried EG assignment should activate immediately");
        assert_eq!(assignment.mode, mode as u8);
        assert_eq!(assignment.frame, esi.frame_number);
        assert_eq!(assignment.multiframe, esi.multiframe_number);
        assert!(assignment.awake_until.is_some());
    }
}

#[test]
fn test_malformed_u_mm_status_energy_saving_is_not_reinterpreted_as_stay_alive() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let test_prim = LmmMleUnitdataInd {
        sdu: BitBuffer::from_bitstr("0011000001"),
        handle: 0,
        received_address: TetraAddress::issi(issi),
    };
    let test_sapmsg = SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(test_prim),
    };

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    test.submit_message(test_sapmsg);
    test.run_stack(Some(1));

    assert_eq!(test.dump_sinks().len(), 0);
    assert!(!test.config.state_read().energy_saving.contains_key(&issi));
}

#[test]
fn test_non_issi_location_update_demand_drops_without_registration() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    for received_address in invalid_mm_source_addresses(issi) {
        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
        test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

        // EN 300 392-2 clauses 16.4.3 and 16.9.3.4 make location update an
        // MS/ITSI registration procedure. A non-individual RF source must not
        // create MM registration state or receive a D-LOCATION UPDATE response.
        submit_location_update_demand_with_handle_and_received_address(
            &mut test,
            base_location_update_demand(LocationUpdateType::ItsiAttach, None),
            0,
            received_address,
        );
        test.run_stack(Some(1));
        let sink_msgs = test.dump_sinks();

        assert!(sink_msgs.is_empty(), "non-ISSI LU source {received_address} should be dropped");
        assert!(!test.config.state_read().subscribers.is_registered(issi));
        assert!(subscriber_updates(&sink_msgs).is_empty());
    }
}

#[test]
fn test_non_issi_u_mm_status_energy_saving_drops_without_energy_mutation() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    for received_address in invalid_mm_source_addresses(issi) {
        let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
        config.cell.energy_saving_mode = EnergySavingMode::Eg1 as u8;
        let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
        test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

        submit_location_update(&mut test, issi, None);
        test.run_stack(Some(1));
        let _ = test.dump_sinks();
        assert!(test.config.state_read().subscribers.is_registered(issi));

        // EN 300 392-2 clauses 16.7.1 and 16.10.35a carry energy-economy MM
        // changes for a registered individual MS. The same numeric SSI with a
        // non-ISSI RF address must not activate EG state.
        submit_u_mm_status_energy_saving_with_received_address(
            &mut test,
            StatusUplink::ChangeOfEnergySavingModeResponse,
            EnergySavingMode::Eg1,
            received_address,
        );
        test.run_stack(Some(1));
        let sink_msgs = test.dump_sinks();

        assert!(
            sink_msgs.is_empty(),
            "non-ISSI U-MM STATUS source {received_address} should be dropped"
        );
        assert!(!test.config.state_read().energy_saving.contains_key(&issi));
    }
}

#[test]
fn test_non_issi_standalone_group_attach_drops_without_affiliation() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let gssi = 3000;

    for received_address in invalid_mm_source_addresses(issi) {
        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
        test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

        submit_location_update(&mut test, issi, None);
        test.run_stack(Some(1));
        let _ = test.dump_sinks();
        assert!(test.config.state_read().subscribers.is_registered(issi));

        // EN 300 392-2 clauses 16.8.2 and 16.8.4 define group
        // attach/detach/report as procedures for an attached individual MS.
        // A non-ISSI RF source must not affiliate the numeric SSI to a group.
        submit_attach_detach_group_identity_with_received_address(&mut test, false, Some(vec![gssi]), received_address);
        test.run_stack(Some(1));
        let sink_msgs = test.dump_sinks();

        assert!(
            sink_msgs.is_empty(),
            "non-ISSI group attach source {received_address} should be dropped"
        );
        assert!(test.config.state_read().subscribers.group_members(gssi).is_empty());
    }
}

#[test]
fn test_location_update_group_report_is_capped_before_affiliation() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let groups: Vec<u32> = (1000..1013).collect();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, groups.clone());
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let accept_prim = sink_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => Some(prim),
            _ => None,
        })
        .expect("expected D-LOCATION UPDATE ACCEPT");
    let mut accept_sdu = BitBuffer::from_bitstr(&accept_prim.sdu.to_bitstr());
    let accept = DLocationUpdateAccept::from_bitbuf(&mut accept_sdu).expect("Failed parsing D-LOCATION UPDATE ACCEPT");
    let gila = accept.group_identity_location_accept.expect("expected GroupIdentityLocationAccept");
    assert_eq!(
        gila.group_identity_accept_reject, 1,
        "over-cap group reports must not claim aggregate acceptance"
    );
    let rejected = gila
        .group_identity_downlink
        .expect("over-cap group reports must explicitly list rejected identities");
    assert_eq!(rejected.len(), groups.len());
    for (rejected_group, requested_gssi) in rejected.iter().zip(groups.iter()) {
        assert_eq!(rejected_group.gssi, Some(*requested_gssi));
        assert_eq!(rejected_group.group_identity_detachment_uplink, Some(0));
        assert!(rejected_group.group_identity_attachment.is_none());
    }

    let state = test.config.state_read();
    assert!(
        state.subscribers.group_members(1000).is_empty(),
        "first group in an over-cap report must not be partially affiliated"
    );
    assert!(
        state.subscribers.group_members(1012).is_empty(),
        "last group in an over-cap report must not be affiliated on the BS side"
    );
}

#[test]
fn test_standalone_group_attach_over_cap_marks_partial_reject_before_affiliation() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let groups: Vec<u32> = (4000..4013).collect();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 Annex G makes the aggregate accept/reject bit cover the
    // requested identities. If local ACK capacity cannot represent all rejected
    // identities, the BS must not commit a partial local affiliation set.
    submit_attach_detach_group_identity(&mut test, issi, false, Some(groups.clone()));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);

    assert_eq!(ack.group_identity_accept_reject, 1);
    let rejected = ack
        .group_identity_downlink
        .expect("over-cap standalone attach must explicitly list rejected identities");
    assert_eq!(rejected.len(), groups.len());
    for (rejected_group, requested_gssi) in rejected.iter().zip(groups.iter()) {
        assert_eq!(rejected_group.gssi, Some(*requested_gssi));
        assert_eq!(rejected_group.group_identity_detachment_uplink, Some(0));
        assert!(rejected_group.group_identity_attachment.is_none());
    }

    let state = test.config.state_read();
    assert!(
        state.subscribers.group_members(4000).is_empty(),
        "first group in an over-cap standalone attach must not be partially affiliated"
    );
    assert!(
        state.subscribers.group_members(4012).is_empty(),
        "last group in an over-cap standalone attach must not be affiliated on the BS side"
    );
}

#[test]
fn test_location_update_rejects_unprovisioned_group_without_affiliation() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let allowed_gssi = 3000;
    let rejected_gssi = 4000;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.allowed_gssi_ranges = Some(SortedDisjointSsiRanges::from_vec_tuple(vec![(allowed_gssi, allowed_gssi)]));
    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![allowed_gssi, rejected_gssi]);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&sink_msgs);
    let gila = accept.group_identity_location_accept.expect("expected GroupIdentityLocationAccept");
    assert_eq!(
        gila.group_identity_accept_reject, 1,
        "unprovisioned GSSI should make group-location accept partial"
    );
    let downlink = gila
        .group_identity_downlink
        .expect("partial group-location accept should list accepted/rejected groups");
    assert!(downlink.iter().any(|group| {
        group.gssi == Some(allowed_gssi)
            && group
                .group_identity_attachment
                .as_ref()
                .is_some_and(|attachment| attachment.group_identity_attachment_lifetime == 0)
    }));
    assert!(downlink.iter().any(|group| {
        group.gssi == Some(rejected_gssi) && group.group_identity_detachment_uplink == Some(0) && group.group_identity_attachment.is_none()
    }));

    let state = test.config.state_read();
    assert_eq!(state.subscribers.group_members(allowed_gssi), vec![issi]);
    assert!(
        state.subscribers.group_members(rejected_gssi).is_empty(),
        "unprovisioned GSSI must not enter the shared subscriber registry"
    );
}

#[test]
fn test_standalone_group_attach_rejects_unprovisioned_group_without_affiliation() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let allowed_gssi = 3000;
    let rejected_gssi = 4000;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.allowed_gssi_ranges = Some(SortedDisjointSsiRanges::from_vec_tuple(vec![(allowed_gssi, allowed_gssi)]));
    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_attach_detach_group_identity(&mut test, issi, false, Some(vec![allowed_gssi, rejected_gssi]));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);

    assert_eq!(
        ack.group_identity_accept_reject, 1,
        "unprovisioned GSSI should make standalone group ACK partial"
    );
    let downlink = ack
        .group_identity_downlink
        .expect("partial standalone group ACK should list accepted/rejected groups");
    assert!(downlink.iter().any(|group| {
        group.gssi == Some(allowed_gssi)
            && group
                .group_identity_attachment
                .as_ref()
                .is_some_and(|attachment| attachment.group_identity_attachment_lifetime == 0)
    }));
    assert!(downlink.iter().any(|group| {
        group.gssi == Some(rejected_gssi) && group.group_identity_detachment_uplink == Some(0) && group.group_identity_attachment.is_none()
    }));

    let state = test.config.state_read();
    assert_eq!(state.subscribers.group_members(allowed_gssi), vec![issi]);
    assert!(
        state.subscribers.group_members(rejected_gssi).is_empty(),
        "unprovisioned GSSI must not enter the shared subscriber registry"
    );
}

#[test]
fn test_standalone_over_cap_mode_one_preserves_existing_groups_without_partial_detach() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let over_cap_groups: Vec<u32> = (6000..6013).collect();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![3000, 3001]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert_eq!(test.config.state_read().subscribers.group_members(3000), vec![issi]);
    assert_eq!(test.config.state_read().subscribers.group_members(3001), vec![issi]);

    // EN 300 392-2 Annex G does not allow local state to move to a partial
    // replacement when the ACK cannot coherently represent the oversized
    // request. Standalone mode=1 must therefore validate capacity before
    // detaching current groups.
    submit_attach_detach_group_identity(&mut test, issi, true, Some(over_cap_groups.clone()));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);
    assert_eq!(ack.group_identity_accept_reject, 1);
    let rejected = ack
        .group_identity_downlink
        .expect("over-cap mode=1 standalone attach must explicitly list rejected identities");
    assert_eq!(rejected.len(), over_cap_groups.len());
    for (rejected_group, requested_gssi) in rejected.iter().zip(over_cap_groups.iter()) {
        assert_eq!(rejected_group.gssi, Some(*requested_gssi));
        assert_eq!(rejected_group.group_identity_detachment_uplink, Some(0));
        assert!(rejected_group.group_identity_attachment.is_none());
    }

    let state = test.config.state_read();
    assert_eq!(state.subscribers.group_members(3000), vec![issi]);
    assert_eq!(state.subscribers.group_members(3001), vec![issi]);
    assert!(
        state.subscribers.group_members(6000).is_empty(),
        "oversized standalone replacement set must not be partially affiliated"
    );
}

#[test]
fn test_location_update_over_cap_mode_one_preserves_existing_groups_without_partial_detach() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let old_groups = vec![2000, 2001];
    let over_cap_groups: Vec<u32> = (5000..5013).collect();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, old_groups.clone());
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert_eq!(test.config.state_read().subscribers.group_members(2000), vec![issi]);
    assert_eq!(test.config.state_read().subscribers.group_members(2001), vec![issi]);

    // EN 300 392-2 Annex G requires reject ACKs to represent rejected groups
    // coherently. If local ACK capacity cannot represent the result, mode=1
    // must not first detach the old groups and then reject the oversized new set.
    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, over_cap_groups.clone());
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let accept = extract_location_update_accept(&sink_msgs);
    let gila = accept.group_identity_location_accept.expect("expected GroupIdentityLocationAccept");
    assert_eq!(gila.group_identity_accept_reject, 1);
    let rejected = gila
        .group_identity_downlink
        .expect("over-cap location update must explicitly list rejected identities");
    assert_eq!(rejected.len(), over_cap_groups.len());
    for (rejected_group, requested_gssi) in rejected.iter().zip(over_cap_groups.iter()) {
        assert_eq!(rejected_group.gssi, Some(*requested_gssi));
        assert_eq!(rejected_group.group_identity_detachment_uplink, Some(0));
        assert!(rejected_group.group_identity_attachment.is_none());
    }

    let state = test.config.state_read();
    assert_eq!(state.subscribers.group_members(2000), vec![issi]);
    assert_eq!(state.subscribers.group_members(2001), vec![issi]);
    assert!(
        state.subscribers.group_members(5000).is_empty(),
        "oversized replacement set must not be partially affiliated"
    );
}

#[test]
fn test_location_update_mode_one_detaches_old_groups_before_affiliating_ack_groups() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![2000, 2001]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert!(test.config.state_read().subscribers.has_group_members(2000));
    assert!(test.config.state_read().subscribers.has_group_members(2001));

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![2001, 2002]);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let accept = extract_location_update_accept(&sink_msgs);
    let accepted_groups = accept
        .group_identity_location_accept
        .and_then(|gila| gila.group_identity_downlink)
        .expect("expected GroupIdentityLocationAccept");
    let accepted_gssis: Vec<u32> = accepted_groups.iter().filter_map(|group| group.gssi).collect();
    assert_eq!(accepted_gssis, vec![2001, 2002]);
    for group in &accepted_groups {
        let attachment = group
            .group_identity_attachment
            .as_ref()
            .expect("mode=1 attach list should acknowledge attachments");
        assert_eq!(attachment.group_identity_attachment_lifetime, 0);
        assert_eq!(attachment.class_of_usage, 0);
        assert!(group.group_identity_detachment_uplink.is_none());
    }

    let state = test.config.state_read();
    assert!(
        state.subscribers.group_members(2000).is_empty(),
        "mode=1 must detach groups absent from the new accepted report"
    );
    assert_eq!(state.subscribers.group_members(2001), vec![issi]);
    assert_eq!(state.subscribers.group_members(2002), vec![issi]);
}

#[test]
fn test_location_update_mode_one_ignores_explicit_detachment_entries() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![2000, 2001]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 16.10.17 mode=1 already detaches all active groups and
    // attaches groups defined in the uplink element. Detachment entries in
    // that element must not be processed or ACKed as separate detachments.
    submit_location_update_with_group_identity_uplink(
        &mut test,
        issi,
        LocationUpdateType::ItsiAttach,
        vec![
            GroupIdentityUplink {
                class_of_usage: None,
                group_identity_detachment_uplink: Some(0),
                gssi: Some(2000),
                address_extension: None,
                vgssi: None,
            },
            GroupIdentityUplink {
                class_of_usage: Some(0),
                group_identity_detachment_uplink: None,
                gssi: Some(2002),
                address_extension: None,
                vgssi: None,
            },
        ],
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let accept = extract_location_update_accept(&sink_msgs);
    let accepted_groups = accept
        .group_identity_location_accept
        .and_then(|gila| gila.group_identity_downlink)
        .expect("expected GroupIdentityLocationAccept");

    assert_eq!(accepted_groups.len(), 1);
    assert_eq!(accepted_groups[0].gssi, Some(2002));
    assert!(accepted_groups[0].group_identity_attachment.is_some());
    assert!(accepted_groups[0].group_identity_detachment_uplink.is_none());

    let state = test.config.state_read();
    assert!(state.subscribers.group_members(2000).is_empty());
    assert!(state.subscribers.group_members(2001).is_empty());
    assert_eq!(state.subscribers.group_members(2002), vec![issi]);
}

#[test]
fn test_attach_detach_group_identity_without_uplink_groups_acks_empty_current_groups() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_attach_detach_group_identity(&mut test, issi, false, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);

    assert_eq!(ack.group_identity_accept_reject, 0);
    assert!(!ack.reserved);
    assert!(
        ack.group_identity_downlink.is_none(),
        "empty current group set should be acknowledged without stale downlink groups"
    );
}

#[test]
fn test_standalone_mode_zero_without_uplink_groups_does_not_echo_current_groups() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let gssi = 3000;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![gssi]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    submit_attach_detach_group_identity(&mut test, issi, false, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);

    // EN 300 392-2 Annex G requirement 9 gives no-group PDUs a detach-all
    // meaning only for mode=1, except for solicited group-report responses.
    // A bare mode=0 no-op must not re-advertise local affiliations as if the
    // MS had requested them in this transaction.
    assert_eq!(ack.group_identity_accept_reject, 0);
    assert!(
        ack.group_identity_downlink.is_none(),
        "mode=0 no-group ACK must not echo current GSSI affiliations"
    );
    assert!(
        subscriber_updates(&sink_msgs).is_empty(),
        "mode=0 no-group ACK must not mutate affiliation state"
    );
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);
}

#[test]
fn test_standalone_mode_one_without_uplink_groups_detaches_all_current_groups() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let groups = vec![3000, 3001];

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, groups.clone());
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_attach_detach_group_identity(&mut test, issi, true, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);

    // EN 300 392-2 clause 16.8.2 and Annex G requirement 9 allow mode=1
    // without a group list to mean "detach all currently active group
    // identities". The acknowledgement should therefore accept the request
    // without re-advertising the groups that were just removed.
    assert_eq!(ack.group_identity_accept_reject, 0);
    assert!(
        ack.group_identity_downlink.is_none(),
        "detach-all ACK should not report stale current groups"
    );

    let updates = subscriber_updates(&sink_msgs);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].action, BrewSubscriberAction::Deaffiliate);
    let mut update_groups = updates[0].groups.clone();
    update_groups.sort_unstable();
    assert_eq!(update_groups, groups);

    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(issi));
    assert!(state.subscribers.group_members(3000).is_empty());
    assert!(state.subscribers.group_members(3001).is_empty());
}

#[test]
fn test_standalone_group_detach_ack_omits_accepted_detachment_group() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let gssi = 3000;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![gssi]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    submit_attach_detach_group_identity_uplink(
        &mut test,
        issi,
        false,
        Some(vec![GroupIdentityUplink {
            class_of_usage: None,
            group_identity_detachment_uplink: Some(0),
            gssi: Some(gssi),
            address_extension: None,
            vgssi: None,
        }]),
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);

    // EN 300 392-2 Annex G requirements 6d and 8d say an accepted group
    // detachment is acknowledged implicitly: accept/reject=0 and no detached
    // group is echoed in the acknowledgement.
    assert_eq!(ack.group_identity_accept_reject, 0);
    assert!(
        ack.group_identity_downlink.is_none(),
        "accepted detachment should be implicit and omit the detached GSSI"
    );

    let updates = subscriber_updates(&sink_msgs);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].action, BrewSubscriberAction::Deaffiliate);
    assert_eq!(updates[0].groups, vec![gssi]);

    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(issi));
    assert!(state.subscribers.group_members(gssi).is_empty());
}

#[test]
fn test_standalone_group_report_complete_clears_stale_groups_without_reacknowledging_them() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let groups = vec![3050, 3051];

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, groups.clone());
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_attach_detach_group_report_response(&mut test, issi, 1, 0);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);

    // EN 300 392-2 clauses 16.4.3 and 16.10.27a define group report complete
    // as the MS reporting no attached groups. It must not ACK stale local
    // groups as if the MS had re-sent them in GroupIdentityUplink.
    assert_eq!(ack.group_identity_accept_reject, 0);
    assert!(
        ack.group_identity_downlink.is_none(),
        "standalone group-report-complete ACK should not report stale current groups"
    );

    let updates = subscriber_updates(&sink_msgs);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].action, BrewSubscriberAction::Deaffiliate);
    let mut update_groups = updates[0].groups.clone();
    update_groups.sort_unstable();
    assert_eq!(update_groups, groups);

    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(issi));
    assert!(state.subscribers.group_members(3050).is_empty());
    assert!(state.subscribers.group_members(3051).is_empty());
}

#[test]
fn test_group_report_response_preserves_ms_accepted_class_of_usage() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let gssi = 3055;
    let class_of_usage = 5;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_group_identity_uplink(
        &mut test,
        issi,
        LocationUpdateType::ItsiAttach,
        vec![GroupIdentityUplink {
            class_of_usage: Some(class_of_usage),
            group_identity_detachment_uplink: None,
            gssi: Some(gssi),
            address_extension: None,
            vgssi: None,
        }],
    );
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_attach_detach_group_report_request(&mut test, issi);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (report, layer2service) = extract_d_attach_detach_group_identity(&sink_msgs);

    // EN 300 392-2 clauses 16.8.4 and 16.10.19 require reported downlink
    // group identities to carry the Group identity attachment sub-elements.
    // Clause 16.10.6 defines Class of usage as the group priority, so the BS
    // must not synthesize a generic priority during group-report refresh.
    assert_eq!(layer2service, Layer2Service::AcknowledgedResponse);
    assert!(report.group_identity_attach_detach_mode);
    let reported = report.group_identity_downlink.expect("group report should include attached group");
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].gssi, Some(gssi));
    let attachment = reported[0]
        .group_identity_attachment
        .as_ref()
        .expect("reported group should include attachment information");
    assert_eq!(attachment.group_identity_attachment_lifetime, 0);
    assert_eq!(attachment.class_of_usage, class_of_usage);
}

#[test]
fn test_group_report_response_preserves_swmi_assigned_class_of_usage() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let gssi = 3056;
    let handle = 72;
    let class_of_usage = 6;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    begin_swmi_group_transaction_for_test(
        &mut test,
        issi,
        handle,
        vec![GroupIdentityDownlink {
            group_identity_attachment: Some(GroupIdentityAttachment {
                group_identity_attachment_lifetime: 0,
                class_of_usage,
            }),
            group_identity_detachment_uplink: None,
            gssi: Some(gssi),
            address_extension: None,
            vgssi: None,
        }],
        false,
    );
    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    submit_attach_detach_group_report_request(&mut test, issi);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (report, layer2service) = extract_d_attach_detach_group_identity(&sink_msgs);

    assert_eq!(layer2service, Layer2Service::AcknowledgedResponse);
    let reported = report
        .group_identity_downlink
        .expect("group report should include SwMI-assigned group");
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].gssi, Some(gssi));
    let attachment = reported[0]
        .group_identity_attachment
        .as_ref()
        .expect("reported SwMI group should include attachment information");
    assert_eq!(attachment.group_identity_attachment_lifetime, 0);
    assert_eq!(attachment.class_of_usage, class_of_usage);
}

#[test]
fn test_standalone_group_report_response_reserved_value_rejects_without_group_mutation() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 3060;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![group]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_attach_detach_group_report_response(&mut test, issi, 1, 1);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);

    assert_eq!(ack.group_identity_accept_reject, 1);
    assert!(ack.group_identity_downlink.is_none());
    assert!(subscriber_updates(&sink_msgs).is_empty());
    assert_eq!(test.config.state_read().subscribers.group_members(group), vec![issi]);
}

#[test]
fn test_mixed_group_report_response_and_attach_list_rejects_without_affiliation() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let requested_group = 3070;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_attach_detach_group_identity_with_report_response(
        &mut test,
        issi,
        false,
        vec![GroupIdentityUplink {
            class_of_usage: Some(0),
            group_identity_detachment_uplink: None,
            gssi: Some(requested_group),
            address_extension: None,
            vgssi: None,
        }],
        1,
        0,
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);

    // EN 300 392-2 clause 16.8.2 says an MS-initiated attach/detach group
    // identity request shall not include group report response. Annex G then
    // requires the rejected requested group to be listed in the reject ACK.
    assert_eq!(ack.group_identity_accept_reject, 1);
    let rejected = ack
        .group_identity_downlink
        .expect("mixed report response plus attach list must explicitly reject requested groups");
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].gssi, Some(requested_group));
    assert_eq!(rejected[0].group_identity_detachment_uplink, Some(0));
    assert!(rejected[0].group_identity_attachment.is_none());
    assert!(
        subscriber_updates(&sink_msgs).is_empty(),
        "mixed report response plus attach list must not emit affiliation updates"
    );
    assert!(test.config.state_read().subscribers.group_members(requested_group).is_empty());
}

#[test]
fn test_mixed_group_report_response_and_mode_one_preserves_existing_groups() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let existing_group = 3075;
    let requested_group = 3076;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![existing_group]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert_eq!(test.config.state_read().subscribers.group_members(existing_group), vec![issi]);

    submit_attach_detach_group_identity_with_report_response(
        &mut test,
        issi,
        true,
        vec![GroupIdentityUplink {
            class_of_usage: Some(0),
            group_identity_detachment_uplink: None,
            gssi: Some(requested_group),
            address_extension: None,
            vgssi: None,
        }],
        1,
        0,
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);

    // Outside a SwMI-requested group-report window, EN 300 392-2 clause
    // 16.8.2 does not define group_report_response in an MS-initiated
    // attach/detach request. Reject before mode=1 detach-all is applied.
    assert_eq!(ack.group_identity_accept_reject, 1);
    let rejected = ack
        .group_identity_downlink
        .expect("mixed mode=1 request must explicitly reject requested groups");
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].gssi, Some(requested_group));
    assert_eq!(rejected[0].group_identity_detachment_uplink, Some(0));
    assert!(rejected[0].group_identity_attachment.is_none());
    assert!(subscriber_updates(&sink_msgs).is_empty());

    let state = test.config.state_read();
    assert_eq!(state.subscribers.group_members(existing_group), vec![issi]);
    assert!(state.subscribers.group_members(requested_group).is_empty());
}

#[test]
fn test_restart_recovery_keeps_group_report_pending_until_attach_detach_complete() {
    debug::setup_logging_verbose();
    let issi = 2260618;
    let gssi = 226333;
    let path = unique_restart_recovery_path("demand-groups-then-attach-detach-complete");
    std::fs::write(&path, format!("{issi}\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let command_msgs = test.dump_sinks();
    let command_details = location_update_command_details(&command_msgs);
    assert_eq!(command_details.len(), 1);
    assert_eq!(command_details[0].0, issi);
    assert!(command_details[0].3.group_identity_report);
    assert!(debug_mm_solicited_group_report_pending(&mut test, issi));

    // This mirrors the field log: the terminal first registers and includes
    // its group list in U-LOCATION UPDATE DEMAND, but omits the
    // group-report-complete IE. Per EN 300 392-2 clause 16.4.4, the BS must
    // keep the group-report window open because a final U-ATTACH/DETACH may
    // follow.
    submit_location_update_with_group_identity_uplink(
        &mut test,
        issi,
        LocationUpdateType::DemandLocationUpdating,
        vec![GroupIdentityUplink {
            class_of_usage: Some(4),
            group_identity_detachment_uplink: None,
            gssi: Some(gssi),
            address_extension: None,
            vgssi: None,
        }],
    );
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    let accept = extract_location_update_accept(&demand_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    assert!(debug_mm_solicited_group_report_pending(&mut test, issi));
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    submit_attach_detach_group_identity_with_report_response(
        &mut test,
        issi,
        true,
        vec![GroupIdentityUplink {
            class_of_usage: Some(4),
            group_identity_detachment_uplink: None,
            gssi: Some(gssi),
            address_extension: None,
            vgssi: None,
        }],
        1,
        0,
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);

    assert_eq!(ack.group_identity_accept_reject, 0);
    let accepted = ack
        .group_identity_downlink
        .expect("final complete group report should acknowledge the reported GSSI");
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].gssi, Some(gssi));
    assert_eq!(
        accepted[0]
            .group_identity_attachment
            .as_ref()
            .expect("accepted group should carry attachment information")
            .class_of_usage,
        4
    );

    let updates = subscriber_updates(&sink_msgs);
    assert!(
        updates.is_empty(),
        "final complete report retaining the same GSSI must not create a transient CMCE No Group window: {updates:?}"
    );
    assert!(!debug_mm_solicited_group_report_pending(&mut test, issi));
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_location_update_group_report_complete_clears_stale_groups_without_reaffiliation() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let groups = vec![3100, 3101];

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, groups.clone());
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert_eq!(test.config.state_read().subscribers.group_members(3100), vec![issi]);
    assert_eq!(test.config.state_read().subscribers.group_members(3101), vec![issi]);

    // EN 300 392-2 clause 16.4.3 says an MS with no attached groups answers a
    // SwMI group-report request with group report complete. Clause 16.10.27a
    // defines value 0 as complete.
    submit_location_update_with_group_report_response(&mut test, issi, LocationUpdateType::DemandLocationUpdating, 1, 0);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&sink_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    assert!(
        accept.group_identity_location_accept.is_none(),
        "empty complete group report must not advertise stale GSSI entries"
    );
    assert!(
        !contains_location_update_command(&sink_msgs),
        "group-report-complete is already a completed report"
    );

    let updates = subscriber_updates(&sink_msgs);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].action, BrewSubscriberAction::Deaffiliate);
    let mut update_groups = updates[0].groups.clone();
    update_groups.sort_unstable();
    assert_eq!(update_groups, groups);

    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(issi));
    assert!(state.subscribers.group_members(3100).is_empty());
    assert!(state.subscribers.group_members(3101).is_empty());
}

#[test]
fn test_group_report_complete_with_energy_saving_preserves_demand_accept_and_clears_groups() {
    debug::setup_logging_verbose();
    let issi = 2040815;
    let groups = vec![3110, 3111];

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, groups.clone());
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert_eq!(test.config.state_read().subscribers.group_members(3110), vec![issi]);
    assert_eq!(test.config.state_read().subscribers.group_members(3111), vec![issi]);

    // EN 300 392-2 clauses 16.4.3 and 16.10.27a define a complete
    // group-report response as no attached groups. Clause 16.7.1 permits the
    // SwMI to answer an MS energy-economy request with StayAlive, and clause
    // 16.10.35a requires the accept type to echo the LU procedure.
    submit_location_update_with_group_report_response_and_energy(
        &mut test,
        issi,
        LocationUpdateType::DemandLocationUpdating,
        1,
        0,
        Some(EnergySavingMode::Eg1),
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&sink_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    let esi = accept
        .energy_saving_information
        .expect("energy-saving request should be answered explicitly");
    assert_eq!(esi.energy_saving_mode, EnergySavingMode::StayAlive);
    assert_eq!(esi.frame_number, None);
    assert_eq!(esi.multiframe_number, None);
    assert!(
        accept.group_identity_location_accept.is_none(),
        "group-report-complete must not ACK stale local affiliations"
    );
    assert!(
        !contains_location_update_command(&sink_msgs),
        "group-report-complete must not trigger a follow-up group report command"
    );

    let updates = subscriber_updates(&sink_msgs);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].action, BrewSubscriberAction::Deaffiliate);
    let mut update_groups = updates[0].groups.clone();
    update_groups.sort_unstable();
    assert_eq!(update_groups, groups);

    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(issi));
    assert!(state.subscribers.group_members(3110).is_empty());
    assert!(state.subscribers.group_members(3111).is_empty());
    assert!(
        !state.energy_saving.contains_key(&issi),
        "StayAlive LU response must not create an EG assignment"
    );
}

#[test]
fn test_new_roaming_group_report_complete_does_not_trigger_another_group_report_command() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_group_report_response(&mut test, issi, LocationUpdateType::RoamingLocationUpdating, 1, 0);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&sink_msgs);
    assert_eq!(
        accept.location_update_accept_type,
        LocationUpdateAcceptType::RoamingLocationUpdating
    );
    assert!(accept.group_identity_location_accept.is_none());
    assert!(
        !contains_location_update_command(&sink_msgs),
        "a complete empty group report must not be followed by another D-LOCATION UPDATE COMMAND"
    );

    let updates = subscriber_updates(&sink_msgs);
    assert!(updates.iter().any(|update| update.action == BrewSubscriberAction::Register));
    assert!(test.config.state_read().subscribers.is_registered(issi));
}

#[test]
fn test_location_update_group_report_response_reserved_value_is_rejected() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_group_report_response(&mut test, issi, LocationUpdateType::DemandLocationUpdating, 1, 1);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let reject = extract_location_update_reject(&sink_msgs);
    assert_eq!(reject.location_update_type, LocationUpdateType::DemandLocationUpdating);
    assert_eq!(reject.reject_cause, RejectCause::MessageConsistencyError as u8);
    assert!(subscriber_updates(&sink_msgs).is_empty());
    assert!(!test.config.state_read().subscribers.is_registered(issi));
}

#[test]
fn test_group_report_request_from_known_ms_returns_d_attach_detach_group_identity_report_complete() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 3000;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![group]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_attach_detach_group_report_request(&mut test, issi);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clause 16.8.4: if SwMI accepts an MS-initiated group
    // report request, it answers with D-ATTACH/DETACH GROUP IDENTITY, mode=1
    // and group-report-complete when the groups fit in one PDU.
    let (report, layer2service) = extract_d_attach_detach_group_identity(&sink_msgs);
    assert!(!report.group_identity_report);
    assert!(!report.group_identity_acknowledgement_request);
    assert!(report.group_identity_attach_detach_mode);
    assert_eq!(layer2service, Layer2Service::AcknowledgedResponse);
    let response = report.group_report_response.expect("group report complete IE should be present");
    assert_eq!(response.len, 1);
    assert_eq!(response.data, 0);
    let downlink = report
        .group_identity_downlink
        .expect("known MS with local group should get downlink group report");
    assert_eq!(downlink.len(), 1);
    assert_eq!(downlink[0].gssi, Some(group));
    assert!(downlink[0].group_identity_attachment.is_some());
    assert!(!contains_attach_detach_ack(&sink_msgs));
    assert!(subscriber_updates(&sink_msgs).is_empty());

    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(issi));
    assert_eq!(state.subscribers.group_members(group), vec![issi]);
}

#[test]
fn test_group_report_request_restores_shared_state_before_reporting_cached_groups() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 3000;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.periodic_registration_secs = 1;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![group]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert_eq!(test.config.state_read().subscribers.group_members(group), vec![issi]);

    backdate_mm_registration(&mut test, issi, 2);
    test.run_stack(Some(1));
    let command_msgs = test.dump_sinks();
    assert!(contains_location_update_command(&command_msgs));
    {
        let state = test.config.state_read();
        assert!(
            !state.subscribers.is_registered(issi),
            "first periodic expiry should clear shared routing state while MM waits for re-registration"
        );
        assert!(state.subscribers.group_members(group).is_empty());
    }

    submit_attach_detach_group_report_request(&mut test, issi);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 16.8.0 and 16.8.4 make accepted reported groups
    // valid attached identities. Before MM advertises cached groups back to
    // the MS, the shared local routing registry must be coherent again.
    let (report, _) = extract_d_attach_detach_group_identity(&sink_msgs);
    let downlink = report
        .group_identity_downlink
        .expect("cached group report should include locally valid groups");
    assert_eq!(downlink.len(), 1);
    assert_eq!(downlink[0].gssi, Some(group));

    let updates = subscriber_updates(&sink_msgs);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].action, BrewSubscriberAction::Register);
    assert_eq!(updates[0].issi, issi);
    assert!(updates[0].groups.is_empty());
    assert_eq!(updates[1].action, BrewSubscriberAction::Affiliate);
    assert_eq!(updates[1].issi, issi);
    assert_eq!(updates[1].groups, vec![group]);

    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(issi));
    assert_eq!(state.subscribers.group_members(group), vec![issi]);
}

#[test]
fn test_group_report_request_with_uplink_groups_rejects_without_affiliation_mutation() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let existing_group = 3000;
    let requested_group = 3001;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![existing_group]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_malformed_attach_detach_group_report_request(
        &mut test,
        issi,
        false,
        Some(vec![GroupIdentityUplink {
            class_of_usage: Some(0),
            group_identity_detachment_uplink: None,
            gssi: Some(requested_group),
            address_extension: None,
            vgssi: None,
        }]),
        None,
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clause 16.8.4 says a report request uses amendment mode
    // and shall not include group identity uplink elements. Do not accept it
    // as a report or process the attached GSSI list.
    let unsupported = extract_mm_pdu_function_not_supported(&sink_msgs);
    assert_eq!(
        unsupported.not_supported_pdu_type,
        MmPduTypeUl::UAttachDetachGroupIdentity.into_raw() as u8
    );
    assert!(extract_d_attach_detach_group_identities(&sink_msgs).is_empty());
    assert!(!contains_attach_detach_ack(&sink_msgs));
    assert!(subscriber_updates(&sink_msgs).is_empty());

    let state = test.config.state_read();
    assert_eq!(state.subscribers.group_members(existing_group), vec![issi]);
    assert!(state.subscribers.group_members(requested_group).is_empty());
}

#[test]
fn test_group_report_request_with_group_report_response_rejects_without_report_downlink() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 3000;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![group]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_malformed_attach_detach_group_report_request(
        &mut test,
        issi,
        false,
        None,
        Some(Type3FieldGeneric {
            field_id: 0,
            len: 1,
            data: 0,
        }),
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // group_report_response is the completion indicator in the report answer,
    // not in the MS report request. A mixed request must not be accepted as a
    // valid report and must not trigger a D-ATTACH/DETACH GROUP IDENTITY report.
    let unsupported = extract_mm_pdu_function_not_supported(&sink_msgs);
    assert_eq!(
        unsupported.not_supported_pdu_type,
        MmPduTypeUl::UAttachDetachGroupIdentity.into_raw() as u8
    );
    assert!(extract_d_attach_detach_group_identities(&sink_msgs).is_empty());
    assert!(!contains_attach_detach_ack(&sink_msgs));
    assert!(subscriber_updates(&sink_msgs).is_empty());
    assert_eq!(test.config.state_read().subscribers.group_members(group), vec![issi]);
}

#[test]
fn test_group_report_request_from_known_ms_without_groups_reports_complete_empty() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_attach_detach_group_report_request(&mut test, issi);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clause 16.8.4: when SwMI has no groups to report, it sends
    // group-report-complete with no group identity downlink IE.
    let (report, layer2service) = extract_d_attach_detach_group_identity(&sink_msgs);
    assert_eq!(layer2service, Layer2Service::AcknowledgedResponse);
    assert!(report.group_identity_attach_detach_mode);
    let response = report.group_report_response.expect("group report complete IE should be present");
    assert_eq!(response.len, 1);
    assert_eq!(response.data, 0);
    assert!(report.group_identity_downlink.is_none());
    assert!(!contains_attach_detach_ack(&sink_msgs));
    assert!(subscriber_updates(&sink_msgs).is_empty());
}

#[test]
fn test_group_report_request_from_known_ms_segments_large_group_list() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let groups: Vec<u32> = (0..14).map(|idx| 3000 + idx).collect();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(
        &mut test,
        issi,
        LocationUpdateType::ItsiAttach,
        groups.iter().copied().take(12).collect(),
    );
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_attach_detach_group_identity(&mut test, issi, false, Some(groups.iter().copied().skip(12).collect()));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_attach_detach_group_report_request(&mut test, issi);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clause 16.8.4: when reported groups do not fit one
    // D-ATTACH/DETACH GROUP IDENTITY PDU, the first PDU omits group-report-
    // complete and subsequent PDUs use amendment mode; only the last PDU
    // carries group-report-complete.
    let reports = extract_d_attach_detach_group_identities(&sink_msgs);
    assert_eq!(reports.len(), 2);

    let (first, first_l2) = &reports[0];
    assert!(!first.group_identity_report);
    assert!(!first.group_identity_acknowledgement_request);
    assert!(first.group_identity_attach_detach_mode);
    assert!(first.group_report_response.is_none());
    assert_eq!(*first_l2, Layer2Service::AcknowledgedResponse);
    let first_groups: Vec<u32> = first
        .group_identity_downlink
        .as_ref()
        .expect("first segment should carry groups")
        .iter()
        .map(|gid| gid.gssi.expect("GSSI should be present"))
        .collect();
    assert_eq!(first_groups, groups[..12].to_vec());

    let (last, last_l2) = &reports[1];
    assert!(!last.group_identity_report);
    assert!(!last.group_identity_acknowledgement_request);
    assert!(!last.group_identity_attach_detach_mode);
    assert_eq!(*last_l2, Layer2Service::Acknowledged);
    let response = last
        .group_report_response
        .as_ref()
        .expect("last segment should complete the report");
    assert_eq!(response.len, 1);
    assert_eq!(response.data, 0);
    let last_groups: Vec<u32> = last
        .group_identity_downlink
        .as_ref()
        .expect("last segment should carry remaining groups")
        .iter()
        .map(|gid| gid.gssi.expect("GSSI should be present"))
        .collect();
    assert_eq!(last_groups, groups[12..].to_vec());

    assert!(!contains_attach_detach_ack(&sink_msgs));
    assert!(subscriber_updates(&sink_msgs).is_empty());
}

#[test]
fn test_group_report_request_from_unknown_ms_returns_function_not_supported_without_registration() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_attach_detach_group_report_request(&mut test, issi);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let unsupported = extract_mm_pdu_function_not_supported(&sink_msgs);
    assert_eq!(
        unsupported.not_supported_pdu_type,
        tetra_pdus::mm::enums::mm_pdu_type_ul::MmPduTypeUl::UAttachDetachGroupIdentity as u8
    );
    assert_eq!(
        extract_mm_pdu_function_not_supported_layer2service(&sink_msgs),
        Layer2Service::Acknowledged
    );
    assert!(!contains_attach_detach_ack(&sink_msgs));
    assert!(subscriber_updates(&sink_msgs).is_empty());
    assert!(!test.config.state_read().subscribers.is_registered(issi));
}

#[test]
fn test_unknown_group_attach_rejects_without_synthesizing_registration() {
    debug::setup_logging_verbose();

    for (idx, group_identity_attach_detach_mode) in [false, true].into_iter().enumerate() {
        let issi = 2040814 + idx as u32;
        let group = 3000 + idx as u32;

        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
        test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Cmce, TetraEntity::Mle]);

        // EN 300 392-2 clauses 16.4.3 and 16.9.3.4 keep group attachment behind
        // the MS registration/location-update path. A standalone group attach from
        // an unknown ISSI must be rejected, not used to synthesize registration.
        submit_attach_detach_group_identity(&mut test, issi, group_identity_attach_detach_mode, Some(vec![group]));
        test.run_stack(Some(1));
        let sink_msgs = test.dump_sinks();
        let ack = extract_attach_detach_ack(&sink_msgs);

        assert_eq!(ack.group_identity_accept_reject, 1);
        let rejected = ack
            .group_identity_downlink
            .expect("rejected group attach must list the rejected identity");
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].gssi, Some(group));
        assert_eq!(rejected[0].group_identity_detachment_uplink, Some(0));
        assert!(rejected[0].group_identity_attachment.is_none());
        assert!(
            subscriber_updates(&sink_msgs).is_empty(),
            "unknown standalone group attach must not emit Register or Affiliate updates"
        );

        let state = test.config.state_read();
        assert!(!state.subscribers.is_registered(issi));
        assert!(state.subscribers.group_members(group).is_empty());
    }
}

#[test]
fn test_standalone_mode_one_ignores_explicit_detachment_entries() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Cmce, TetraEntity::Mle]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![3000, 3001]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_attach_detach_group_identity_uplink(
        &mut test,
        issi,
        true,
        Some(vec![
            GroupIdentityUplink {
                class_of_usage: None,
                group_identity_detachment_uplink: Some(0),
                gssi: Some(3000),
                address_extension: None,
                vgssi: None,
            },
            GroupIdentityUplink {
                class_of_usage: Some(0),
                group_identity_detachment_uplink: None,
                gssi: Some(3002),
                address_extension: None,
                vgssi: None,
            },
        ]),
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);

    assert_eq!(ack.group_identity_accept_reject, 0);
    let downlink = ack.group_identity_downlink.expect("expected acknowledgement for attached group");
    assert_eq!(downlink.len(), 1);
    assert_eq!(downlink[0].gssi, Some(3002));
    assert!(downlink[0].group_identity_attachment.is_some());
    assert!(downlink[0].group_identity_detachment_uplink.is_none());

    let state = test.config.state_read();
    assert!(state.subscribers.group_members(3000).is_empty());
    assert!(state.subscribers.group_members(3001).is_empty());
    assert_eq!(state.subscribers.group_members(3002), vec![issi]);
}

#[test]
fn test_standalone_mode_one_retains_same_group_without_cmce_deaffiliate() {
    debug::setup_logging_verbose();
    let issi = 2260616;
    let gssi = 226333;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Cmce, TetraEntity::Mle]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![gssi]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 clause 16.10.17 mode=1 replaces the current group list.
    // If the replacement list retains the same accepted GSSI, local CMCE
    // routing must not see a transient Deaffiliate/No Group window.
    submit_attach_detach_group_identity(&mut test, issi, true, Some(vec![gssi]));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let ack = extract_attach_detach_ack(&sink_msgs);
    assert_eq!(ack.group_identity_accept_reject, 0);
    let downlink = ack.group_identity_downlink.expect("retained group should be acknowledged");
    assert_eq!(downlink.len(), 1);
    assert_eq!(downlink[0].gssi, Some(gssi));

    let updates = subscriber_updates(&sink_msgs);
    assert!(
        updates.is_empty(),
        "retaining the same GSSI must not churn CMCE affiliation updates: {updates:?}"
    );
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);
}

#[test]
fn test_location_update_mode_one_retains_same_group_without_cmce_deaffiliate() {
    debug::setup_logging_verbose();
    let issi = 2260616;
    let gssi = 226333;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Cmce, TetraEntity::Mle]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![gssi]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_location_update_with_groups_and_group_report_response(
        &mut test,
        issi,
        LocationUpdateType::DemandLocationUpdating,
        vec![gssi],
        1,
        0,
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&sink_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    let gila = accept
        .group_identity_location_accept
        .expect("retained group should be acknowledged in location update accept");
    assert_eq!(gila.group_identity_accept_reject, 0);
    let downlink = gila.group_identity_downlink.expect("accepted group should be listed");
    assert_eq!(downlink.len(), 1);
    assert_eq!(downlink[0].gssi, Some(gssi));

    let updates = subscriber_updates(&sink_msgs);
    assert!(
        updates.is_empty(),
        "mode=1 restart refresh retaining the same GSSI must not churn CMCE affiliation updates: {updates:?}"
    );
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);
}

#[test]
fn test_u_itsi_detach_deaffiliates_deregisters_and_clears_energy_saving() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let gssi = 3000;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.energy_saving_mode = EnergySavingMode::Eg1 as u8;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![gssi]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    {
        let state = test.config.state_read();
        assert!(state.subscribers.is_registered(issi));
        assert_eq!(state.subscribers.group_members(gssi), vec![issi]);
        assert!(state.energy_saving.contains_key(&issi));
    }

    // EN 300 392-2 clause 16.9.3.3 U-ITSI DETACH announces MS
    // de-activation. The BS-side lifecycle must therefore clear the registered
    // subscriber, its group affiliations, and any negotiated energy economy
    // assignment instead of leaving stale routing/listen-window state.
    submit_u_itsi_detach(&mut test, issi);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let state = test.config.state_read();
    assert!(!state.subscribers.is_registered(issi));
    assert!(state.subscribers.group_members(gssi).is_empty());
    assert!(!state.energy_saving.contains_key(&issi));
}

#[test]
fn test_hard_roaming_reregistration_resets_shared_groups_and_energy_saving() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let gssi = 3000;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.energy_saving_mode = EnergySavingMode::Eg1 as u8;

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![gssi]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    {
        let state = test.config.state_read();
        assert_eq!(state.subscribers.group_members(gssi), vec![issi]);
        assert!(state.energy_saving.contains_key(&issi));
    }

    backdate_mm_registration(&mut test, issi, 121);

    // EN 300 392-2 clauses 16.4.1.1 and 16.7.1: once the old MS is treated
    // as a hard roaming re-registration, accepted group and energy-economy
    // state must be rebuilt from the new procedure rather than inherited from
    // the stale local registration.
    submit_location_update_with_type(&mut test, issi, LocationUpdateType::RoamingLocationUpdating, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&sink_msgs);
    assert_eq!(
        accept.location_update_accept_type,
        LocationUpdateAcceptType::RoamingLocationUpdating
    );
    let status = extract_d_mm_status(&sink_msgs);
    assert_eq!(status.status_downlink, StatusDownlink::ChangeOfEnergySavingModeRequest);
    assert_eq!(
        status
            .energy_saving_information
            .expect("fresh BS-initiated EG request should carry ESI")
            .energy_saving_mode,
        EnergySavingMode::Eg1
    );

    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(issi));
    assert!(
        state.subscribers.group_members(gssi).is_empty(),
        "hard roaming re-registration without group list must clear stale shared GSSI membership"
    );
    assert!(
        !state.energy_saving.contains_key(&issi),
        "new BS-initiated EG request must stay pending until the MS response"
    );
    drop(state);
    assert_eq!(
        debug_mm_client_energy(&mut test, issi),
        Some((EnergySavingMode::StayAlive, None, None)),
        "hard roaming re-registration must clear stale client-manager EG mode/window until the fresh response arrives"
    );
}

#[test]
fn test_soft_roaming_reattach_releases_private_calls_without_group_churn() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let gssi = 3000;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(CfgBrew {
        host: "127.0.0.1".to_string(),
        port: 443,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: std::time::Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: false,
        feature_rssi_export: false,
        whitelisted_ssis: None,
    });

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![gssi]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_location_update_with_type(&mut test, issi, LocationUpdateType::RoamingLocationUpdating, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 16.9.3.4 and 16.10.35a keep this as a
    // location-registration update accepted as RoamingLocationUpdating. The
    // local CMCE cleanup is private-call state repair only; accepted group
    // affiliations from clauses 16.8.0/16.8.4 must not be withdrawn and
    // replayed as a transient "No Group" window.
    let accept = extract_location_update_accept(&sink_msgs);
    assert_eq!(
        accept.location_update_accept_type,
        LocationUpdateAcceptType::RoamingLocationUpdating
    );

    let cmce_updates: Vec<&MmSubscriberUpdate> = sink_msgs
        .iter()
        .filter(|msg| msg.dest == TetraEntity::Cmce)
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::MmSubscriberUpdate(update) => Some(update),
            _ => None,
        })
        .collect();
    assert_eq!(cmce_updates.len(), 1);
    assert_eq!(cmce_updates[0].action, BrewSubscriberAction::ReleaseIndividualCalls);
    assert_eq!(cmce_updates[0].groups, Vec::<u32>::new());

    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    assert!(
        sink_msgs
            .iter()
            .filter(|msg| msg.dest == TetraEntity::Brew)
            .all(|msg| !matches!(msg.msg, SapMsgInner::MmSubscriberUpdate(_))),
        "soft roaming re-attach must not replay cached affiliation toward Brew"
    );
}

#[test]
fn test_location_update_with_groups_emits_register_before_affiliate() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let groups = vec![3000, 3001];

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Cmce, TetraEntity::Mle]);

    // EN 300 392-2 clauses 16.10.35/16.10.35a keep registration and group
    // attachment as separate MM results; CMCE must learn the ISSI before its
    // group affiliations are made visible.
    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, groups.clone());
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let updates = subscriber_updates(&sink_msgs);

    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].issi, issi);
    assert_eq!(updates[0].action, BrewSubscriberAction::Register);
    assert!(updates[0].groups.is_empty());
    assert_eq!(updates[1].issi, issi);
    assert_eq!(updates[1].action, BrewSubscriberAction::Affiliate);
    assert_eq!(updates[1].groups, groups);
}

#[test]
fn test_duplicate_group_attach_retry_acks_without_duplicate_affiliate() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 3000;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Cmce, TetraEntity::Mle]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_attach_detach_group_identity(&mut test, issi, false, Some(vec![group]));
    test.run_stack(Some(1));
    let first_attach_msgs = test.dump_sinks();
    assert!(
        subscriber_updates(&first_attach_msgs)
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Affiliate && update.groups == vec![group])
    );

    // EN 300 392-2 clause 16.4.3 allows the MS to retry group attachment
    // procedures. A duplicate attach must be ACKed but must not inflate local
    // group-listener state with another affiliate event.
    submit_attach_detach_group_identity(&mut test, issi, false, Some(vec![group]));
    test.run_stack(Some(1));
    let retry_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&retry_msgs);
    let ack_groups: Vec<u32> = ack
        .group_identity_downlink
        .unwrap_or_default()
        .iter()
        .filter_map(|group| group.gssi)
        .collect();
    assert_eq!(ack_groups, vec![group]);
    assert!(
        subscriber_updates(&retry_msgs)
            .iter()
            .all(|update| update.action != BrewSubscriberAction::Affiliate),
        "duplicate attach retry must not emit another affiliate update"
    );
    assert_eq!(test.config.state_read().subscribers.group_members(group), vec![issi]);
}

#[test]
fn test_swmi_group_ack_accepts_pending_attach_without_downlink_response() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 91;
    let handle = 77;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_attach_group(group)], false);
    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let updates = subscriber_updates(&msgs);

    assert!(
        updates
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Affiliate && update.groups == vec![group]),
        "accepted SwMI-initiated group attachment should affiliate the MS"
    );
    assert!(
        test.config.state_read().subscribers.group_members(group).contains(&issi),
        "accepted SwMI-initiated group attachment should update subscriber registry"
    );
    assert!(
        !contains_attach_detach_ack(&msgs),
        "U-ATTACH/DETACH GROUP IDENTITY ACK expects no downlink MM response"
    );
}

#[test]
fn test_non_issi_swmi_group_ack_drops_without_completing_pending_transaction() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 92;
    let handle = 78;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_attach_group(group)], false);

    for received_address in invalid_mm_source_addresses(issi) {
        // EN 300 392-2 clauses 16.8.6, 16.8.8 and Annex G bind SwMI group
        // acknowledgement to an individual MS/ITSI procedure. A GSSI,
        // unknown, or out-of-range RF source must not complete the pending
        // SwMI-initiated group transaction.
        submit_swmi_group_ack_with_received_address(&mut test, issi, handle, false, vec![], received_address);
        test.run_stack(Some(1));
        let msgs = test.dump_sinks();

        assert!(
            msgs.is_empty(),
            "non-ISSI SwMI group ACK source {received_address} should be dropped"
        );
        assert!(!test.config.state_read().subscribers.group_members(group).contains(&issi));
    }

    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let valid_ack_msgs = test.dump_sinks();
    assert!(
        subscriber_updates(&valid_ack_msgs)
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Affiliate && update.groups == vec![group]),
        "valid ACK after invalid RF sources should still complete the pending SwMI attachment"
    );
    assert!(test.config.state_read().subscribers.group_members(group).contains(&issi));
}

#[test]
fn test_wrong_handle_swmi_group_ack_keeps_pending_transaction_until_valid_ack() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 101;
    let handle = 87;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_attach_group(group)], false);
    submit_swmi_group_ack(&mut test, issi, handle + 1, false, vec![]);
    test.run_stack(Some(1));
    let wrong_handle_msgs = test.dump_sinks();

    // EN 300 392-2 clause 16.11.1.3 binds the SwMI-initiated group
    // attach/detach transaction to T353. A mismatched ACK handle is not the
    // solicited response and must not consume the pending transaction.
    assert!(
        subscriber_updates(&wrong_handle_msgs)
            .iter()
            .all(|update| update.action != BrewSubscriberAction::Affiliate),
        "mismatched handle ACK must not affiliate the pending SwMI attachment"
    );
    assert!(!test.config.state_read().subscribers.group_members(group).contains(&issi));
    assert!(!contains_attach_detach_ack(&wrong_handle_msgs));

    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let valid_ack_msgs = test.dump_sinks();
    assert!(
        subscriber_updates(&valid_ack_msgs)
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Affiliate && update.groups == vec![group]),
        "valid ACK after mismatched handle should still complete the pending SwMI attachment"
    );
    assert!(test.config.state_read().subscribers.group_members(group).contains(&issi));
}

#[test]
fn test_swmi_group_ack_rejects_explicit_attachment_without_affiliation() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 92;
    let handle = 78;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_attach_group(group)], false);
    submit_swmi_group_ack(&mut test, issi, handle, true, vec![group]);
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let updates = subscriber_updates(&msgs);

    assert!(
        updates.iter().all(|update| update.action != BrewSubscriberAction::Affiliate),
        "explicitly rejected SwMI-requested group attachment must not affiliate the MS"
    );
    assert!(
        !test.config.state_read().subscribers.group_members(group).contains(&issi),
        "explicit rejection must leave subscriber registry unattached"
    );
    assert!(!contains_attach_detach_ack(&msgs));
}

#[test]
fn test_swmi_group_ack_reject_without_group_list_does_not_affiliate() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 96;
    let handle = 82;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_attach_group(group)], false);
    submit_swmi_group_ack(&mut test, issi, handle, true, vec![]);
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // EN 300 392-2 Annex G requirement 7 and clause 16.10.14/table 16.46:
    // reject means at least one attachment was rejected, and all rejected
    // groups must be present in the ACK identity list.
    assert!(
        subscriber_updates(&msgs)
            .iter()
            .all(|update| update.action != BrewSubscriberAction::Affiliate),
        "malformed reject ACK without rejected groups must not accept the pending SwMI attachment"
    );
    assert!(
        !test.config.state_read().subscribers.group_members(group).contains(&issi),
        "malformed reject ACK without rejected groups must leave subscriber registry unattached"
    );
    assert!(
        !contains_attach_detach_ack(&msgs),
        "U-ATTACH/DETACH GROUP IDENTITY ACK still expects no downlink MM response"
    );

    // A malformed ACK is ignored, not treated as terminal. EN 300 392-2
    // Annex G requirement 7 forbids implicit rejection, so the pending
    // SwMI-initiated transaction remains open until a valid ACK or T353 expiry.
    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let valid_ack_msgs = test.dump_sinks();
    assert!(
        subscriber_updates(&valid_ack_msgs)
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Affiliate && update.groups == vec![group]),
        "valid ACK after malformed reject should still complete the pending SwMI attachment"
    );
    assert!(test.config.state_read().subscribers.group_members(group).contains(&issi));
}

#[test]
fn test_swmi_group_ack_reject_wrong_address_type_does_not_consume_pending() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 103;
    let handle = 89;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_attach_group(group)], false);
    submit_swmi_group_ack_uplink(&mut test, issi, handle, true, Some(vec![swmi_ack_vgssi_detach_entry(group)]));
    test.run_stack(Some(1));
    let wrong_type_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 16.10.22/16.10.27 and Annex G keep the group
    // identity address form significant. A VGSSI rejection is not a GSSI
    // rejection merely because the numeric value matches.
    assert!(
        subscriber_updates(&wrong_type_msgs)
            .iter()
            .all(|update| update.action != BrewSubscriberAction::Affiliate),
        "wrong-address-type reject ACK must not affiliate or complete the pending GSSI attachment"
    );
    assert!(!test.config.state_read().subscribers.group_members(group).contains(&issi));
    assert!(!contains_attach_detach_ack(&wrong_type_msgs));

    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let valid_ack_msgs = test.dump_sinks();
    assert!(
        subscriber_updates(&valid_ack_msgs)
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Affiliate && update.groups == vec![group]),
        "valid ACK after wrong-address-type reject should still complete the pending GSSI attachment"
    );
    assert!(test.config.state_read().subscribers.group_members(group).contains(&issi));
}

#[test]
fn test_swmi_group_ack_accept_vgssi_pending_does_not_affiliate_plain_gssi() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let vgssi = 104;
    let handle = 90;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_attach_vgssi(vgssi)], false);
    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // This stack does not yet model VGSSI/GTSI affiliation in the local
    // subscriber registry. Annex G acknowledgement handling must therefore
    // fail closed instead of coercing VGSSI 104 into plain GSSI 104.
    assert!(
        subscriber_updates(&msgs)
            .iter()
            .all(|update| update.action != BrewSubscriberAction::Affiliate),
        "accepted VGSSI transaction must not be reported as a plain GSSI affiliation"
    );
    assert!(!test.config.state_read().subscribers.group_members(vgssi).contains(&issi));
    assert!(!contains_attach_detach_ack(&msgs));
}

#[test]
fn test_swmi_group_ack_reject_with_accepted_attachment_entry_still_affiliates_that_group() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let accepted_group = 97;
    let rejected_group = 98;
    let handle = 83;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    begin_swmi_group_transaction_for_test(
        &mut test,
        issi,
        handle,
        vec![swmi_attach_group(accepted_group), swmi_attach_group(rejected_group)],
        false,
    );
    submit_swmi_group_ack_uplink(
        &mut test,
        issi,
        handle,
        true,
        Some(vec![swmi_ack_attach_entry(accepted_group), swmi_ack_detach_entry(rejected_group)]),
    );
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let updates = subscriber_updates(&msgs);

    // EN 300 392-2 Annex G requirements 7, 8a and 8b: a reject ACK may list
    // accepted attachments as attachment entries, while rejected attachments
    // are detachment entries. Do not treat every listed GSSI as rejected.
    assert!(
        updates
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Affiliate && update.groups == vec![accepted_group]),
        "explicitly accepted attachment in a mixed reject ACK should affiliate"
    );
    assert!(
        test.config.state_read().subscribers.group_members(accepted_group).contains(&issi),
        "accepted group should be present in subscriber registry"
    );
    assert!(
        !test.config.state_read().subscribers.group_members(rejected_group).contains(&issi),
        "detachment entry in reject ACK should leave the requested attachment rejected"
    );
    assert!(!contains_attach_detach_ack(&msgs));
}

#[test]
fn test_swmi_group_ack_reject_with_only_attachment_entries_does_not_affiliate() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 99;
    let handle = 84;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_attach_group(group)], false);
    submit_swmi_group_ack_uplink(&mut test, issi, handle, true, Some(vec![swmi_ack_attach_entry(group)]));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // EN 300 392-2 Annex G requirement 7 forbids implicit rejection. If the
    // reject bit is set but no detachment-form rejected attachment is present,
    // do not reinterpret the ACK as acceptance.
    assert!(
        subscriber_updates(&msgs)
            .iter()
            .all(|update| update.action != BrewSubscriberAction::Affiliate),
        "malformed reject ACK without rejection entries must not affiliate"
    );
    assert!(
        !test.config.state_read().subscribers.group_members(group).contains(&issi),
        "malformed reject ACK must leave subscriber registry unattached"
    );
    assert!(!contains_attach_detach_ack(&msgs));

    // Attachment-form entries in a reject ACK can be explicit acceptances
    // (Annex G requirement 8a), but they are not rejection entries. The ACK is
    // therefore malformed for a single requested attachment and must not
    // consume the pending transaction.
    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let valid_ack_msgs = test.dump_sinks();
    assert!(
        subscriber_updates(&valid_ack_msgs)
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Affiliate && update.groups == vec![group]),
        "valid ACK after malformed reject should still complete the pending SwMI attachment"
    );
    assert!(test.config.state_read().subscribers.group_members(group).contains(&issi));
}

#[test]
fn test_swmi_group_ack_cannot_reject_requested_detach() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 93;
    let handle = 79;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    submit_attach_detach_group_identity(&mut test, issi, false, Some(vec![group]));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert!(test.config.state_read().subscribers.group_members(group).contains(&issi));

    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_detach_group(group)], false);
    submit_swmi_group_ack_uplink(&mut test, issi, handle, true, Some(vec![swmi_ack_attach_entry(group)]));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let updates = subscriber_updates(&msgs);

    assert!(
        updates
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Deaffiliate && update.groups == vec![group]),
        "MS cannot reject a SwMI-requested group detachment"
    );
    assert!(
        !test.config.state_read().subscribers.group_members(group).contains(&issi),
        "SwMI-requested detachment must be applied despite ACK reject list"
    );
}

#[test]
fn test_swmi_group_ack_accepts_pending_detach_without_downlink_response() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 102;
    let handle = 88;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    submit_attach_detach_group_identity(&mut test, issi, false, Some(vec![group]));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert!(test.config.state_read().subscribers.group_members(group).contains(&issi));

    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_detach_group(group)], false);
    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let updates = subscriber_updates(&msgs);

    // EN 300 392-2 Annex G requirements 4, 6d and G.3: an accepted
    // SwMI-requested detachment may omit the group list, and the uplink ACK
    // expects no additional downlink MM response.
    assert!(
        updates
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Deaffiliate && update.groups == vec![group]),
        "accepted SwMI-requested group detachment should deaffiliate the MS"
    );
    assert!(
        !test.config.state_read().subscribers.group_members(group).contains(&issi),
        "accepted SwMI-requested detachment should update subscriber registry"
    );
    assert!(!contains_attach_detach_ack(&msgs));
}

#[test]
fn test_itsi_detach_clears_pending_swmi_group_transaction_before_issi_reuse() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let stale_group = 105;
    let handle = 91;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_attach_group(stale_group)], false);

    // EN 300 392-2 clauses 16.8.6 and 16.9.3.3: U-ITSI DETACH terminates
    // the registered MM context, so a later ACK from the detached/reused ISSI
    // must not complete an older SwMI group attach transaction.
    submit_u_itsi_detach(&mut test, issi);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    assert!(msgs.is_empty(), "stale SwMI group ACK after detach/reuse should be ignored");
    assert!(!test.config.state_read().subscribers.group_members(stale_group).contains(&issi));
}

#[test]
fn test_periodic_registration_command_abandons_pending_swmi_group_transaction() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let stale_group = 106;
    let handle = 92;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.periodic_registration_secs = 1;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_attach_group(stale_group)], false);

    backdate_mm_registration(&mut test, issi, 2);
    test.run_stack(Some(1));
    let command_msgs = test.dump_sinks();
    assert!(contains_location_update_command(&command_msgs));

    // EN 300 392-2 clauses 16.8.6 and 16.9.2.8: a SwMI registration command
    // starts a fresh registration procedure, so any old Annex G group ACK must
    // not attach groups while the MS is in the re-registration window.
    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    assert!(msgs.is_empty(), "stale SwMI group ACK after registration command should be ignored");
    assert!(!test.config.state_read().subscribers.group_members(stale_group).contains(&issi));
}

#[test]
fn test_periodic_registration_grace_expiry_rejects_and_removes_groups_energy_and_stale_swmi_ack() {
    debug::setup_logging_verbose();
    let issi = 2040818;
    let active_group = 3007;
    let stale_group = 3008;
    let handle = 94;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.periodic_registration_secs = 1;
    config.cell.energy_saving_mode = EnergySavingMode::Eg1 as u8;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![active_group]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::Eg1,
    );
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    {
        let state = test.config.state_read();
        assert!(state.subscribers.is_registered(issi));
        assert_eq!(state.subscribers.group_members(active_group), vec![issi]);
        assert!(state.energy_saving.contains_key(&issi));
    }

    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_attach_group(stale_group)], false);

    backdate_mm_registration(&mut test, issi, 2);
    test.run_stack(Some(1));
    let command_msgs = test.dump_sinks();
    assert!(contains_location_update_command(&command_msgs));
    {
        let state = test.config.state_read();
        assert!(
            !state.subscribers.is_registered(issi),
            "first periodic expiry clears shared routing while the SwMI waits for re-registration"
        );
        assert!(state.subscribers.group_members(active_group).is_empty());
        assert!(
            state.energy_saving.contains_key(&issi),
            "grace period keeps the cached EG assignment available for a timely DemandLocationUpdating response"
        );
    }

    // EN 300 392-2 clauses 16.9.2.8 and 16.9.3.4: the SwMI first prompts a
    // fresh registration with D-LOCATION UPDATE COMMAND. If the local watchdog
    // grace period then expires without the expected U-LOCATION UPDATE DEMAND,
    // clause 16.11.1.1 timer-expiry semantics map to REJECT(ExpiryOfTimer).
    // Clause 16.8.6 keeps stale group ACKs from completing after registration
    // has overridden the old group procedure.
    expire_mm_registration_grace(&mut test, issi);
    test.run_stack(Some(1));
    let reject_msgs = test.dump_sinks();
    let reject = extract_location_update_reject(&reject_msgs);
    assert_eq!(reject.location_update_type, LocationUpdateType::PeriodicLocationUpdating);
    assert_eq!(reject.reject_cause, RejectCause::ExpiryOfTimer as u8);

    let updates = subscriber_updates(&reject_msgs);
    assert!(
        updates
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Deaffiliate && update.groups == vec![active_group]),
        "final removal must publish deaffiliation for cached groups"
    );
    assert!(
        updates
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Deregister && update.issi == issi),
        "final removal must publish deregistration"
    );

    {
        let state = test.config.state_read();
        assert!(!state.subscribers.is_registered(issi));
        assert!(state.subscribers.group_members(active_group).is_empty());
        assert!(!state.energy_saving.contains_key(&issi));
    }
    assert_eq!(
        debug_mm_client_energy(&mut test, issi),
        None,
        "second expiry removes the MM client and its cached EG mode"
    );

    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let stale_ack_msgs = test.dump_sinks();
    assert!(
        stale_ack_msgs.is_empty(),
        "stale SwMI group ACK after final registration expiry must be ignored"
    );
    assert!(!test.config.state_read().subscribers.group_members(stale_group).contains(&issi));
}

#[test]
fn test_brew_reconnected_marks_registration_pending_and_abandons_swmi_group_transaction() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let existing_group = 107;
    let stale_group = 108;
    let handle = 93;
    let registration_handle = 0x4321;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update_with_type_and_handle(&mut test, issi, LocationUpdateType::ItsiAttach, None, registration_handle);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    submit_attach_detach_group_identity(&mut test, issi, false, Some(vec![existing_group]));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_attach_group(stale_group)], false);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::BrewReconnected,
    });
    test.run_stack(Some(1));
    let command_msgs = test.dump_sinks();
    assert!(contains_location_update_command(&command_msgs));

    // EN 300 392-2 clauses 16.4.3 and 16.8.6: Brew reconnect uses
    // D-LOCATION UPDATE COMMAND as a SwMI-initiated registration refresh.
    // The old SwMI group transaction is abandoned, while the subsequent
    // DemandLocationUpdating response is still treated as a pending-command
    // registration and replays Register/Affiliate toward CMCE/Brew.
    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let stale_ack_msgs = test.dump_sinks();
    assert!(
        stale_ack_msgs.is_empty(),
        "stale SwMI group ACK after Brew reconnect should be ignored"
    );
    assert!(!test.config.state_read().subscribers.group_members(stale_group).contains(&issi));

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::DemandLocationUpdating, vec![existing_group]);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    let updates = subscriber_updates(&demand_msgs);
    assert!(
        updates
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Register && update.issi == issi),
        "Brew reconnect registration refresh must replay Register after DemandLocationUpdating"
    );
    assert!(
        updates
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Affiliate && update.groups == vec![existing_group]),
        "Brew reconnect registration refresh must replay group affiliation"
    );
    assert_eq!(test.config.state_read().subscribers.group_members(existing_group), vec![issi]);
}

#[test]
fn test_unmatched_swmi_group_ack_is_ignored() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 94;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    submit_swmi_group_ack(&mut test, issi, 80, false, vec![group]);
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    assert!(msgs.is_empty(), "unmatched SwMI group ACK should not emit responses or updates");
    assert!(!test.config.state_read().subscribers.group_members(group).contains(&issi));
}

#[test]
fn test_swmi_group_pending_expiry_blocks_late_ack() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 95;
    let handle = 81;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_attach_group(group)], false);
    test.run_stack(Some(730));
    let _ = test.dump_sinks();
    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    assert!(msgs.is_empty(), "late ACK after T353 expiry should be ignored");
    assert!(!test.config.state_read().subscribers.group_members(group).contains(&issi));
}

#[test]
fn test_standalone_group_attach_marks_reject_for_unsupported_address_form() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let accepted_gssi = 3000;
    let rejected_vgssi = 4000;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Cmce, TetraEntity::Mle]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 16.10.12 says accept/reject=1 when at least one requested
    // attachment/detachment is rejected. This implementation only supports
    // plain GSSI attachment; a VGSSI entry must not be silently skipped while
    // the ACK claims that all requested identities were accepted.
    submit_attach_detach_group_identity_uplink(
        &mut test,
        issi,
        false,
        Some(vec![
            GroupIdentityUplink {
                class_of_usage: Some(0),
                group_identity_detachment_uplink: None,
                gssi: Some(accepted_gssi),
                address_extension: None,
                vgssi: None,
            },
            GroupIdentityUplink {
                class_of_usage: Some(0),
                group_identity_detachment_uplink: None,
                gssi: None,
                address_extension: None,
                vgssi: Some(rejected_vgssi),
            },
        ]),
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let ack = extract_attach_detach_ack(&sink_msgs);

    assert_eq!(ack.group_identity_accept_reject, 1);
    let downlink = ack
        .group_identity_downlink
        .expect("partial rejection should identify affected groups");
    assert!(downlink.iter().any(|group| {
        group.gssi == Some(accepted_gssi)
            && group
                .group_identity_attachment
                .as_ref()
                .is_some_and(|attachment| attachment.group_identity_attachment_lifetime == 0)
    }));
    assert!(downlink.iter().any(|group| {
        group.vgssi == Some(rejected_vgssi)
            && group.group_identity_attachment.is_none()
            && group.group_identity_detachment_uplink == Some(0)
    }));
    assert_eq!(test.config.state_read().subscribers.group_members(accepted_gssi), vec![issi]);
}

#[test]
fn test_location_update_group_demand_marks_reject_for_unsupported_address_form() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let accepted_gssi = 3000;
    let rejected_vgssi = 4000;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Cmce, TetraEntity::Mle]);

    submit_location_update_with_group_identity_uplink(
        &mut test,
        issi,
        LocationUpdateType::ItsiAttach,
        vec![
            GroupIdentityUplink {
                class_of_usage: Some(0),
                group_identity_detachment_uplink: None,
                gssi: Some(accepted_gssi),
                address_extension: None,
                vgssi: None,
            },
            GroupIdentityUplink {
                class_of_usage: Some(0),
                group_identity_detachment_uplink: None,
                gssi: None,
                address_extension: None,
                vgssi: Some(rejected_vgssi),
            },
        ],
    );
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let accept = extract_location_update_accept(&sink_msgs);
    let gila = accept
        .group_identity_location_accept
        .expect("location update should include GroupIdentityLocationAccept");

    assert_eq!(gila.group_identity_accept_reject, 1);
    let downlink = gila
        .group_identity_downlink
        .expect("partial rejection should identify affected groups in GILA");
    assert!(downlink.iter().any(|group| group.gssi == Some(accepted_gssi)));
    assert!(downlink.iter().any(|group| {
        group.vgssi == Some(rejected_vgssi)
            && group.group_identity_attachment.is_none()
            && group.group_identity_detachment_uplink == Some(0)
    }));
    assert_eq!(test.config.state_read().subscribers.group_members(accepted_gssi), vec![issi]);
}

#[test]
fn test_unsupported_location_update_types_do_not_emit_accept() {
    debug::setup_logging_verbose();
    let unsupported_types = [
        LocationUpdateType::MigratingLocationUpdating,
        LocationUpdateType::ServiceRestorationMigratingLocationUpdating,
        LocationUpdateType::DisabledMsUpdating,
    ];

    for (idx, location_update_type) in unsupported_types.into_iter().enumerate() {
        let issi = 2043000 + idx as u32;
        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
        test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

        // EN 300 392-2 table 16.67 defines raw LU types 1, 5, and 7, but this
        // SwMI implementation does not support accepting those procedures. They
        // must not fall through to D-LOCATION UPDATE ACCEPT table 16.68 values.
        submit_location_update_with_type(&mut test, issi, location_update_type, None);
        test.run_stack(Some(1));
        let sink_msgs = test.dump_sinks();

        assert!(
            !contains_location_update_accept(&sink_msgs),
            "{location_update_type} must not produce D-LOCATION UPDATE ACCEPT"
        );
        assert!(
            subscriber_updates(&sink_msgs).is_empty(),
            "{location_update_type} must not register or affiliate the subscriber"
        );
        let reject = extract_location_update_reject(&sink_msgs);
        assert_eq!(
            reject.location_update_type, location_update_type,
            "D-LOCATION UPDATE REJECT must preserve the unsupported LU demand type"
        );
        let expected_cause = match location_update_type {
            LocationUpdateType::MigratingLocationUpdating | LocationUpdateType::ServiceRestorationMigratingLocationUpdating => {
                RejectCause::MigrationNotSupported
            }
            LocationUpdateType::DisabledMsUpdating => RejectCause::ServiceNotSubscribed,
            _ => unreachable!("test only covers unsupported LU types"),
        };
        assert_eq!(reject.reject_cause, expected_cause as u8);
    }
}

#[test]
fn test_unsupported_location_update_features_emit_reject() {
    debug::setup_logging_verbose();

    let unsupported_requests = vec![
        {
            let mut pdu = base_location_update_demand(LocationUpdateType::ItsiAttach, None);
            pdu.request_to_append_la = true;
            (pdu, RejectCause::LaNotAllowed)
        },
        {
            let mut pdu = base_location_update_demand(LocationUpdateType::ItsiAttach, None);
            pdu.la_information = Some(0x1234);
            (pdu, RejectCause::LaNotAllowed)
        },
        {
            let mut pdu = base_location_update_demand(LocationUpdateType::ItsiAttach, None);
            pdu.cipher_control = true;
            pdu.ciphering_parameters = Some(0);
            (pdu, RejectCause::NoCipherKsg)
        },
        {
            let mut pdu = base_location_update_demand(LocationUpdateType::ItsiAttach, None);
            pdu.authentication_uplink = Some(type3_field(MmType34ElemIdUl::AuthenticationUplink, 8, 0));
            (pdu, RejectCause::MessageConsistencyError)
        },
        {
            let mut pdu = base_location_update_demand(LocationUpdateType::ItsiAttach, None);
            pdu.proprietary = Some(type3_field(MmType34ElemIdUl::Proprietary, 8, 0));
            (pdu, RejectCause::MessageConsistencyError)
        },
    ];

    for (idx, (pdu, expected_cause)) in unsupported_requests.into_iter().enumerate() {
        let issi = 2044000 + idx as u32;
        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
        test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

        // EN 300 392-2 clause 16.9.3.4 expects D-LOCATION UPDATE ACCEPT or
        // REJECT. Unsupported critical LU features must not be silently dropped.
        submit_location_update_demand_with_handle(&mut test, issi, pdu, 0);
        test.run_stack(Some(1));
        let sink_msgs = test.dump_sinks();

        assert!(!contains_location_update_accept(&sink_msgs));
        assert!(subscriber_updates(&sink_msgs).is_empty());
        let reject = extract_location_update_reject(&sink_msgs);
        assert_eq!(reject.location_update_type, LocationUpdateType::ItsiAttach);
        assert_eq!(reject.reject_cause, expected_cause as u8);
        assert!(!test.config.state_read().subscribers.is_registered(issi));
    }
}

#[test]
fn test_location_update_accepts_extended_capabilities_without_acting_on_them() {
    debug::setup_logging_verbose();

    let issi = 2260082;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    let mut pdu = base_location_update_demand(LocationUpdateType::ItsiAttach, None);
    pdu.extended_capabilities = Some(type3_field(MmType34ElemIdUl::ExtendedCapabilities, 8, 0));

    // EN 300 392-2 clause 16.4.4 permits/requires this IE in U-LOCATION
    // UPDATE DEMAND when supported extended features are present. Nexus-BS does
    // not consume the bits yet, but accepting the registration keeps restart
    // recovery and normal attach from failing on standards-compliant radios.
    submit_location_update_demand_with_handle(&mut test, issi, pdu, 0);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&sink_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::ItsiAttach);
    let updates = subscriber_updates(&sink_msgs);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].action, BrewSubscriberAction::Register);
    assert_eq!(updates[0].issi, issi);
}

#[test]
fn test_location_update_accepts_matching_optional_ssi_identity() {
    debug::setup_logging_verbose();

    let issi = 2040814;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    // EN 300 392-2 clause 16.9.3.4 table 16.18 defines optional SSI as the
    // ISSI of the MS. A matching value is valid and remains compatible with
    // radios that include the identity explicitly.
    let mut pdu = base_location_update_demand(LocationUpdateType::ItsiAttach, None);
    pdu.ssi = Some(issi as u64);
    submit_location_update_demand_with_handle(&mut test, issi, pdu, 0);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(contains_location_update_accept(&sink_msgs));
    assert!(test.config.state_read().subscribers.is_registered(issi));
}

#[test]
fn test_location_update_accepts_matching_optional_mni_identity() {
    debug::setup_logging_verbose();

    let issi = 2040814;
    let matching_mni = (204u64 << 14) | 1337u64;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    // EN 300 392-2 clause 16.9.3.4 table 16.18 defines address extension as
    // the MS MNI. A matching MCC/MNC extension is valid, while absence remains
    // accepted for radios that omit the optional field.
    let mut pdu = base_location_update_demand(LocationUpdateType::ItsiAttach, None);
    pdu.address_extension = Some(matching_mni);
    submit_location_update_demand_with_handle(&mut test, issi, pdu, 0);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(contains_location_update_accept(&sink_msgs));
    assert!(test.config.state_read().subscribers.is_registered(issi));
}

#[test]
fn test_location_update_rejects_mismatched_optional_mni_identity() {
    debug::setup_logging_verbose();

    let issi = 2040814;
    let mismatched_mni = (204u64 << 14) | 1338u64;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    // EN 300 392-2 clauses 16.4.1.1 and 16.9.3.4 require a present address
    // extension to identify the MS MNI. A different MNC is inconsistent with
    // this cell and must not create subscriber or energy-economy state.
    let mut pdu = base_location_update_demand(LocationUpdateType::ItsiAttach, None);
    pdu.address_extension = Some(mismatched_mni);
    submit_location_update_demand_with_handle(&mut test, issi, pdu, 0);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(!contains_location_update_accept(&sink_msgs));
    assert!(subscriber_updates(&sink_msgs).is_empty());
    let reject = extract_location_update_reject(&sink_msgs);
    assert_eq!(reject.location_update_type, LocationUpdateType::ItsiAttach);
    assert_eq!(reject.reject_cause, RejectCause::MessageConsistencyError as u8);
    assert!(!test.config.state_read().subscribers.is_registered(issi));
}

#[test]
fn test_location_update_rejects_mismatched_optional_ssi_identity() {
    debug::setup_logging_verbose();

    let issi = 2040814;
    let claimed_issi = issi + 1;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    // EN 300 392-2 clause 16.4.1.1 and table 16.18 require the optional SSI
    // in U-LOCATION UPDATE DEMAND to identify the registering MS. It must not
    // contradict the lower-layer address that delivered the MM procedure.
    let mut pdu = base_location_update_demand(LocationUpdateType::ItsiAttach, None);
    pdu.ssi = Some(claimed_issi as u64);
    submit_location_update_demand_with_handle(&mut test, issi, pdu, 0);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(!contains_location_update_accept(&sink_msgs));
    assert!(subscriber_updates(&sink_msgs).is_empty());
    let reject = extract_location_update_reject(&sink_msgs);
    assert_eq!(reject.location_update_type, LocationUpdateType::ItsiAttach);
    assert_eq!(reject.reject_cause, RejectCause::MessageConsistencyError as u8);
    assert!(!test.config.state_read().subscribers.is_registered(issi));
    assert!(!test.config.state_read().subscribers.is_registered(claimed_issi));
}

#[test]
fn test_whitelist_rejection_uses_policy_cause_not_migration() {
    debug::setup_logging_verbose();
    let issi = 2040814;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.security.issi_whitelist = vec![999_999];

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(!contains_location_update_accept(&sink_msgs));
    assert!(subscriber_updates(&sink_msgs).is_empty());
    let reject = extract_location_update_reject(&sink_msgs);
    assert_eq!(reject.location_update_type, LocationUpdateType::ItsiAttach);
    assert_eq!(reject.reject_cause, RejectCause::ServiceNotSubscribed as u8);
    assert!(!test.config.state_read().subscribers.is_registered(issi));
}

#[test]
fn test_known_migrating_location_update_deaffiliates_and_deregisters_without_accept() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let group = 3000;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Cmce, TetraEntity::Mle]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![group]);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert_eq!(test.config.state_read().subscribers.group_members(group), vec![issi]);

    // EN 300 392-2 table 16.67 defines migrating LU types, while table 16.68
    // defines accept encodings. This SwMI does not implement migration identity
    // exchange, so a known migrating MS must be released from local affiliation
    // state and must not be accepted as still registered here.
    submit_location_update_with_type(&mut test, issi, LocationUpdateType::MigratingLocationUpdating, None);
    test.run_stack(Some(1));
    let migration_msgs = test.dump_sinks();
    let updates = subscriber_updates(&migration_msgs);

    assert!(
        !contains_location_update_accept(&migration_msgs),
        "unsupported migration must not produce D-LOCATION UPDATE ACCEPT"
    );
    let reject = extract_location_update_reject(&migration_msgs);
    assert_eq!(reject.location_update_type, LocationUpdateType::MigratingLocationUpdating);
    assert_eq!(reject.reject_cause, RejectCause::MigrationNotSupported as u8);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].action, BrewSubscriberAction::Deaffiliate);
    assert_eq!(updates[0].groups, vec![group]);
    assert_eq!(updates[1].action, BrewSubscriberAction::Deregister);
    assert!(updates[1].groups.is_empty());

    let state = test.config.state_read();
    assert!(!state.subscribers.is_registered(issi));
    assert!(state.subscribers.group_members(group).is_empty());
}

#[test]
fn test_restart_recovery_cache_sends_location_update_command_on_startup() {
    debug::setup_logging_verbose();
    let cached_issi = 2260082;
    let seeded_issi = 2260616;
    let path = unique_restart_recovery_path("startup");
    std::fs::write(&path, format!("{cached_issi}\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.cell.restart_recovery_issis = vec![seeded_issi];
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    // EN 300 392-2 clause 16.4.4 permits the SwMI to initiate registration at
    // any time with D-LOCATION UPDATE COMMAND. Nexus-BS uses that procedure
    // after process restart for locally known ISSIs that may still be camped.
    // Keep the local restart probe behind a short startup guard and pace
    // recovered ISSIs so the first RF frames are not overloaded with several
    // acknowledged MM commands at once.
    test.run_stack(Some(72));
    assert!(
        location_update_commands(&test.dump_sinks()).is_empty(),
        "restart recovery must not blast commands during the startup RF guard"
    );

    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let commands = location_update_commands(&sink_msgs);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].0, cached_issi);
    assert_eq!(commands[0].1, 0);
    assert!(commands[0].2.group_identity_report);

    test.run_stack(Some(71));
    assert!(
        location_update_commands(&test.dump_sinks()).is_empty(),
        "restart recovery must pace configured/cached ISSIs instead of sending a burst"
    );

    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let commands = location_update_commands(&sink_msgs);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].0, seeded_issi);
    assert_eq!(commands[0].1, 0);
    assert!(commands[0].2.group_identity_report);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_large_cache_paces_one_command_per_interval_and_restores_groups() {
    debug::setup_logging_verbose();
    let member_count = LARGE_RESTART_RECOVERY_MEMBER_COUNT;
    let first_issi = 2_264_000_u32;
    let gssi = 226333;
    let path = unique_restart_recovery_path("large-paced-groups");
    let cache: String = (0..member_count)
        .map(|offset| format!("{} {}:0:0\n", first_issi + offset, gssi))
        .collect();
    std::fs::write(&path, cache).expect("failed to seed large recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2_260_000, 2_269_999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    // EN 300 392-2 clause 16.4.4 permits SwMI-commanded registration with
    // D-LOCATION UPDATE COMMAND. The inter-ISSI delay is Nexus-BS local RF
    // robustness policy: after a BS restart, thousands of still-camped MSs must
    // be re-probed one at a time, without creating a first-frame burst or
    // dropping their previously accepted group affiliations.
    test.run_stack(Some(72));
    assert!(
        location_update_commands(&test.dump_sinks()).is_empty(),
        "large restart recovery must hold the startup guard before probing"
    );

    for offset in 0..member_count {
        let issi = first_issi + offset;
        test.run_stack(Some(1));
        let command_msgs = test.dump_sinks();
        let commands = location_update_command_details(&command_msgs);
        assert_eq!(commands.len(), 1, "ISSI {issi} should receive exactly one restart command");
        assert_eq!(commands[0].0, issi);
        assert_eq!(commands[0].1, 0);
        assert_eq!(commands[0].2, Layer2Service::Acknowledged);
        assert!(commands[0].3.group_identity_report);

        submit_location_update_with_groups_and_group_report_response(
            &mut test,
            issi,
            LocationUpdateType::DemandLocationUpdating,
            vec![gssi],
            1,
            0,
        );
        test.run_stack(Some(1));
        let response_msgs = test.dump_sinks();
        assert!(
            contains_location_update_accept(&response_msgs),
            "ISSI {issi} should complete commanded DemandLocationUpdating"
        );
        assert!(
            !contains_location_update_command(&response_msgs),
            "successful ISSI {issi} recovery response must remove the pending probe"
        );

        if offset + 1 < member_count {
            test.run_stack(Some(70));
            assert!(
                location_update_commands(&test.dump_sinks()).is_empty(),
                "large restart recovery must not send an early or burst command after ISSI {issi}"
            );
        }
    }

    let mut members = test.config.state_read().subscribers.group_members(gssi);
    members.sort_unstable();
    assert_eq!(members.len(), member_count as usize);
    assert_eq!(members.first().copied(), Some(first_issi));
    assert_eq!(members.last().copied(), Some(first_issi + member_count - 1));

    test.run_stack(Some(144));
    assert!(
        location_update_commands(&test.dump_sinks()).is_empty(),
        "all recovered ISSIs should be removed from the restart probe queue"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_large_cache_first_sweep_reaches_every_issi_before_retry() {
    debug::setup_logging_verbose();
    let member_count = LARGE_RESTART_RECOVERY_MEMBER_COUNT + 1;
    let first_issi = 2_264_000_u32;
    let gssi = 226333;
    let path = unique_restart_recovery_path("large-first-sweep");
    let cache: String = (0..member_count)
        .map(|offset| format!("{} {}:0:0\n", first_issi + offset, gssi))
        .collect();
    std::fs::write(&path, cache).expect("failed to seed large recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2_260_000, 2_269_999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    test.run_stack(Some(72));
    assert!(
        location_update_commands(&test.dump_sinks()).is_empty(),
        "large first sweep must keep the startup guard quiet"
    );

    let mut seen = BTreeSet::new();
    for offset in 0..member_count {
        let expected_issi = first_issi + offset;
        test.run_stack(Some(1));
        let command_msgs = test.dump_sinks();
        let commands = location_update_command_details(&command_msgs);
        assert_eq!(commands.len(), 1, "first sweep tick {offset} should emit exactly one command");
        assert_eq!(commands[0].0, expected_issi);
        assert_eq!(commands[0].1, 0);
        assert_eq!(commands[0].2, Layer2Service::Acknowledged);
        assert!(commands[0].3.group_identity_report);
        assert!(
            seen.insert(commands[0].0),
            "ISSI {} was retried before every cached ISSI received a first probe",
            commands[0].0
        );

        if offset + 1 < member_count {
            test.run_stack(Some(71));
            assert!(
                location_update_commands(&test.dump_sinks()).is_empty(),
                "first sweep must keep inter-ISSI spacing quiet after ISSI {}",
                commands[0].0
            );
        }
    }

    assert_eq!(seen.len(), member_count as usize);
    assert!(seen.contains(&first_issi));
    assert!(seen.contains(&(first_issi + member_count - 1)));

    test.run_stack(Some(72));
    let retry_msgs = test.dump_sinks();
    let retry_commands = location_update_command_details(&retry_msgs);
    assert_eq!(retry_commands.len(), 1);
    assert_eq!(retry_commands[0].0, first_issi);
    assert!(retry_commands[0].3.group_identity_report);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_successful_location_update_persists_restart_recovery_cache() {
    debug::setup_logging_verbose();
    let issi = 2260082;
    let path = unique_restart_recovery_path("persist");
    let _ = std::fs::remove_file(&path);

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(contains_location_update_accept(&sink_msgs));
    let cache = std::fs::read_to_string(&path).expect("registration should persist restart recovery cache");
    assert!(
        cache.lines().any(|line| line.trim() == issi.to_string()),
        "cache should contain ISSI {issi}, got {cache:?}"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_successful_location_update_persists_restart_recovery_groups() {
    debug::setup_logging_verbose();
    let issi = 2260082;
    let gssi = 226333;
    let path = unique_restart_recovery_path("persist-groups");
    let _ = std::fs::remove_file(&path);

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_groups(&mut test, issi, LocationUpdateType::ItsiAttach, vec![gssi]);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(contains_location_update_accept(&sink_msgs));
    let cache = std::fs::read_to_string(&path).expect("registration should persist restart recovery cache");
    assert!(
        cache.lines().any(|line| line.trim() == format!("{issi} {gssi}:0:0")),
        "cache should contain ISSI {issi} and GSSI {gssi}, got {cache:?}"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_cache_coalesces_multiple_updates_until_flush() {
    debug::setup_logging_verbose();
    let issis = [2260082, 2260616, 2260618];
    let path = unique_restart_recovery_path("coalesce-updates");
    let _ = std::fs::remove_file(&path);

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    for issi in issis {
        submit_location_update(&mut test, issi, None);
    }
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    assert_eq!(
        debug_mm_restart_recovery_cache_len(&mut test),
        issis.len(),
        "MM restart recovery cache should update in memory for every ISSI without rereading the file per update"
    );
    assert!(
        debug_mm_restart_recovery_cache_dirty(&mut test),
        "multiple same-window updates should be coalesced instead of forcing a full-file write per ISSI"
    );

    // EN 300 392-2 clause 16.4.4 permits SwMI-initiated registration after
    // restart; this assertion is only about local persistence scaling. Force a
    // flush to prove the coalesced cache still writes all recovered ISSIs.
    debug_mm_flush_restart_recovery_cache(&mut test);
    assert!(!debug_mm_restart_recovery_cache_dirty(&mut test));
    let cache = std::fs::read_to_string(&path).expect("forced flush should persist restart recovery cache");
    for issi in issis {
        assert!(
            cache.lines().any(|line| line.trim() == issi.to_string()),
            "coalesced restart recovery cache should contain ISSI {issi}, got {cache:?}"
        );
    }

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_unsolicited_itsi_attach_without_groups_restores_cached_affiliation() {
    debug::setup_logging_verbose();
    let issi = 2260618;
    let gssi = 226333;
    let path = unique_restart_recovery_path("unsolicited-itsi-cached-group");
    std::fs::write(&path, format!("{issi} {gssi}:0:4\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    // EN 300 392-2 clause 16.4.4 gives the SwMI a group-report command path,
    // but a still-camped MS may also self-attach before the startup command is
    // sent. If it omits group identities, restore only the cached, previously
    // accepted group locally and send a separate SwMI group refresh instead of
    // colliding with an immediate group-report command.
    submit_location_update_with_type(&mut test, issi, LocationUpdateType::ItsiAttach, None);
    test.run_stack(Some(1));
    let attach_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&attach_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::ItsiAttach);
    assert!(
        accept.group_identity_location_accept.is_none(),
        "cached restart restoration must not fabricate a GroupIdentityLocationAccept for a group-less ITSI attach"
    );

    assert!(
        !contains_location_update_command(&attach_msgs),
        "cached restart group refresh must not immediately collide with a group-report command"
    );
    assert!(!debug_mm_solicited_group_report_pending(&mut test, issi));

    let updates = subscriber_updates(&attach_msgs);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].action, BrewSubscriberAction::Register);
    assert_eq!(updates[0].issi, issi);
    assert!(updates[0].groups.is_empty());
    assert_eq!(updates[1].action, BrewSubscriberAction::Affiliate);
    assert_eq!(updates[1].issi, issi);
    assert_eq!(updates[1].groups, vec![gssi]);
    assert_swmi_group_attach_refresh(&attach_msgs, gssi, 4, "unsolicited restart ITSI attach");
    assert!(debug_mm_swmi_group_transaction_pending(&mut test, issi));

    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(issi));
    assert_eq!(state.subscribers.group_members(gssi), vec![issi]);
    drop(state);

    let cache = std::fs::read_to_string(&path).expect("restored affiliation should keep cached group");
    assert!(
        cache.lines().any(|line| line.trim() == format!("{issi} {gssi}:0:4")),
        "cache should preserve restored GSSI/class, got {cache:?}"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_unsolicited_itsi_attach_eg7_requests_group_report_before_bs_eg() {
    debug::setup_logging_verbose();
    let issi = 2260616;
    let path = unique_restart_recovery_path("unsolicited-itsi-eg7-no-group");
    std::fs::write(&path, format!("{issi}\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.cell.energy_saving_mode = EnergySavingMode::Eg7 as u8;
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_type(&mut test, issi, LocationUpdateType::ItsiAttach, None);
    test.run_stack(Some(1));
    let attach_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&attach_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::ItsiAttach);
    assert!(accept.group_identity_location_accept.is_none());
    assert!(
        test.config.state_read().subscribers.group_members(226333).is_empty(),
        "bare restart cache must not invent a GSSI"
    );
    assert!(
        !test.config.state_read().energy_saving.contains_key(&issi),
        "BS-initiated EG7 request must remain pending until the MS explicitly responds"
    );

    let downlink_types = mm_downlink_pdu_types(&attach_msgs);
    let command_idx = downlink_types
        .iter()
        .position(|pdu| *pdu == MmPduTypeDl::DLocationUpdateCommand)
        .expect("group-report command should be queued for restart candidate");
    let status_idx = downlink_types
        .iter()
        .position(|pdu| *pdu == MmPduTypeDl::DMmStatus)
        .expect("configured EG7 should still be requested");
    assert!(
        command_idx < status_idx,
        "group report command must be queued before BS-initiated EG7 request, got {downlink_types:?}"
    );
    assert!(debug_mm_solicited_group_report_pending(&mut test, issi));

    let cache = std::fs::read_to_string(&path).expect("bare ISSI cache should remain readable");
    assert!(
        cache.lines().any(|line| line.trim() == issi.to_string()),
        "bare cache should keep ISSI and no fabricated GSSI, got {cache:?}"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_unsolicited_itsi_attach_eg7_refreshes_cached_group_before_bs_eg() {
    debug::setup_logging_verbose();
    let issi = 2260616;
    let gssi = 226333;
    let path = unique_restart_recovery_path("unsolicited-itsi-eg7-cached-group");
    std::fs::write(&path, format!("{issi} {gssi}:0:4\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.cell.energy_saving_mode = EnergySavingMode::Eg7 as u8;
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    submit_location_update_with_type(&mut test, issi, LocationUpdateType::ItsiAttach, None);
    test.run_stack(Some(1));
    let attach_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&attach_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::ItsiAttach);
    assert!(
        accept.group_identity_location_accept.is_none(),
        "cached EG7 restart restore must not fake a group report inside D-LOCATION UPDATE ACCEPT"
    );
    assert!(
        !contains_location_update_command(&attach_msgs),
        "cached EG7 restart restore uses SwMI group refresh instead of an immediate group-report command"
    );
    assert_swmi_group_attach_refresh(&attach_msgs, gssi, 4, "EG7 cached restart restore");

    let downlink_types = mm_downlink_pdu_types(&attach_msgs);
    let refresh_idx = downlink_types
        .iter()
        .position(|pdu| *pdu == MmPduTypeDl::DAttachDetachGroupIdentity)
        .expect("cached group refresh should be queued");
    let status_idx = downlink_types
        .iter()
        .position(|pdu| *pdu == MmPduTypeDl::DMmStatus)
        .expect("configured EG7 should still be requested");
    assert!(
        refresh_idx < status_idx,
        "cached group refresh must be queued before BS-initiated EG7 request, got {downlink_types:?}"
    );
    assert!(
        !test.config.state_read().energy_saving.contains_key(&issi),
        "EG7 must remain pending until the MS explicitly responds"
    );
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);
    assert!(debug_mm_swmi_group_transaction_pending(&mut test, issi));

    // The local MLE handle is not an over-air field in the group-identity ACK.
    // Restart refresh accepts a same-ISSI ACK even when LLC/MLE reports an
    // unrouted non-zero handle, preventing a false T353 rollback to No Group.
    submit_swmi_group_ack(&mut test, issi, 123_456, false, vec![]);
    test.run_stack(Some(1));
    let ack_msgs = test.dump_sinks();
    assert!(!contains_attach_detach_ack(&ack_msgs));
    assert!(
        subscriber_updates(&ack_msgs)
            .iter()
            .all(|update| update.action != BrewSubscriberAction::Affiliate),
        "ACK for already-restored cached EG7 group must not duplicate affiliation"
    );
    assert!(!debug_mm_swmi_group_transaction_pending(&mut test, issi));
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    test.run_stack(Some(721));
    let after_t353_msgs = test.dump_sinks();
    assert!(
        subscriber_updates(&after_t353_msgs)
            .iter()
            .all(|update| update.action != BrewSubscriberAction::Deaffiliate),
        "accepted unrouted restart ACK must not roll back after T353"
    );
    assert!(
        !contains_location_update_command(&after_t353_msgs),
        "accepted unrouted restart ACK must not reprobe after T353"
    );
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    submit_u_mm_status_energy_saving(
        &mut test,
        issi,
        StatusUplink::ChangeOfEnergySavingModeResponse,
        EnergySavingMode::Eg7,
    );
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);
    let assignment = test
        .config
        .state_read()
        .energy_saving
        .get(&issi)
        .copied()
        .expect("matching EG7 response must activate pending assignment");
    assert_eq!(assignment.mode, EnergySavingMode::Eg7 as u8);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_large_cached_group_eg7_activates_assignments_for_all_members() {
    debug::setup_logging_verbose();
    let member_count = LARGE_RESTART_RECOVERY_MEMBER_COUNT;
    let first_issi = 2_264_000_u32;
    let gssi = 226333;
    let path = unique_restart_recovery_path("large-unsolicited-itsi-eg7-cached-group");
    let cache: String = (0..member_count)
        .map(|offset| format!("{} {}:0:4\n", first_issi + offset, gssi))
        .collect();
    std::fs::write(&path, cache).expect("failed to seed large recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2_260_000, 2_269_999)]);
    config.cell.energy_saving_mode = EnergySavingMode::Eg7 as u8;
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    for offset in 0..member_count {
        let issi = first_issi + offset;
        submit_location_update_with_type(&mut test, issi, LocationUpdateType::ItsiAttach, None);
        test.run_stack(Some(1));
        let attach_msgs = test.dump_sinks();
        assert!(
            contains_location_update_accept(&attach_msgs),
            "restart recovery LU for ISSI {issi} should still get D-LOCATION UPDATE ACCEPT"
        );
        if offset == 0 || offset == member_count - 1 {
            assert_swmi_group_attach_refresh(&attach_msgs, gssi, 4, "large EG7 cached restart restore");
        }
        assert!(
            !test.config.state_read().energy_saving.contains_key(&issi),
            "EG7 must remain pending until ISSI {issi} explicitly responds"
        );

        submit_swmi_group_ack(&mut test, issi, 800_000 + offset, false, vec![]);
        test.run_stack(Some(1));
        let _ = test.dump_sinks();

        submit_u_mm_status_energy_saving(
            &mut test,
            issi,
            StatusUplink::ChangeOfEnergySavingModeResponse,
            EnergySavingMode::Eg7,
        );
        test.run_stack(Some(1));
        let _ = test.dump_sinks();
    }

    let mut members = test.config.state_read().subscribers.group_members(gssi);
    members.sort_unstable();
    assert_eq!(members.len(), member_count as usize);
    assert_eq!(members.first().copied(), Some(first_issi));
    assert_eq!(members.last().copied(), Some(first_issi + member_count - 1));

    let state = test.config.state_read();
    for offset in 0..member_count {
        let issi = first_issi + offset;
        let assignment = state
            .energy_saving
            .get(&issi)
            .copied()
            .expect("large restart-restored member must activate EG7 after matching response");
        assert_eq!(assignment.mode, EnergySavingMode::Eg7 as u8);
        assert_eq!(assignment.suspension_count, 0);
    }

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_group_less_demand_restores_cached_affiliation() {
    debug::setup_logging_verbose();
    let issi = 2260618;
    let gssi = 226333;
    let path = unique_restart_recovery_path("cached-group-restore");
    std::fs::write(&path, format!("{issi} {gssi}:0:4\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let command_msgs = test.dump_sinks();
    let command_details = location_update_command_details(&command_msgs);
    assert_eq!(command_details.len(), 1);
    assert_eq!(command_details[0].0, issi);
    assert!(command_details[0].3.group_identity_report);

    // EN 300 392-2 clause 16.4.4 permits BS-commanded registration.
    // Clause 16.8.0 keeps previously accepted group identities valid while
    // their lifetime remains valid. If the MS answers the recovery command
    // without a fresh group report, restore only the cached accepted groups for
    // local routing; the D-LOCATION UPDATE ACCEPT itself must not pretend the
    // group was reported in this PDU.
    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DemandLocationUpdating, None);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&demand_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    assert!(
        accept.group_identity_location_accept.is_none(),
        "cached group restoration must not fabricate a GroupIdentityLocationAccept entry"
    );
    assert!(
        !contains_location_update_command(&demand_msgs),
        "cached group restoration must not trigger an immediate duplicate recovery command"
    );
    assert!(
        debug_mm_solicited_group_report_pending(&mut test, issi),
        "the solicited group-report window remains pending until an explicit complete report or expiry"
    );

    let updates = subscriber_updates(&demand_msgs);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].action, BrewSubscriberAction::Register);
    assert_eq!(updates[0].issi, issi);
    assert!(updates[0].groups.is_empty());
    assert_eq!(updates[1].action, BrewSubscriberAction::Affiliate);
    assert_eq!(updates[1].issi, issi);
    assert_eq!(updates[1].groups, vec![gssi]);
    assert_swmi_group_attach_refresh(&demand_msgs, gssi, 4, "group-less DemandLocationUpdating cached restart restore");

    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(issi));
    assert_eq!(state.subscribers.group_members(gssi), vec![issi]);
    drop(state);

    let cache = std::fs::read_to_string(&path).expect("restored affiliation should keep cached group");
    assert!(
        cache.lines().any(|line| line.trim() == format!("{issi} {gssi}:0:4")),
        "cache should preserve restored GSSI/class, got {cache:?}"
    );

    submit_swmi_group_ack(&mut test, issi, 0, false, vec![]);
    test.run_stack(Some(1));
    let ack_msgs = test.dump_sinks();
    assert!(
        !contains_attach_detach_ack(&ack_msgs),
        "U-ATTACH/DETACH GROUP IDENTITY ACK must not get a downlink MM response"
    );
    assert!(
        subscriber_updates(&ack_msgs)
            .iter()
            .all(|update| update.action != BrewSubscriberAction::Affiliate),
        "ACK for already-restored cached group refresh must not duplicate CMCE/Brew affiliation"
    );
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_group_less_update_preserves_pending_swmi_refresh_until_ack() {
    debug::setup_logging_verbose();
    let issi = 2260616;
    let gssi = 226333;
    let path = unique_restart_recovery_path("cached-group-refresh-survives-group-less-lu");
    std::fs::write(&path, format!("{issi} {gssi}:0:4\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.cell.energy_saving_mode = EnergySavingMode::Eg7 as u8;
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let command_msgs = test.dump_sinks();
    assert_eq!(location_update_commands(&command_msgs).len(), 1);

    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DemandLocationUpdating, None);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert_swmi_group_attach_refresh(&demand_msgs, gssi, 4, "group-less DemandLocationUpdating cached restart restore");
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);
    assert!(debug_mm_swmi_group_transaction_pending(&mut test, issi));

    // EN 300 392-2 clause 16.8.6 handles collision of group attachment with
    // other MM procedures. A later U-LOCATION UPDATE DEMAND with no group
    // state is not an ACK/reject for the already pending SwMI
    // D-ATTACH/DETACH GROUP IDENTITY, so T353 or the real ACK must remain
    // authoritative. This avoids a post-restart BS/MS split where local MM has
    // 226333 but the terminal still shows "No Group".
    submit_location_update_with_type(&mut test, issi, LocationUpdateType::PeriodicLocationUpdating, None);
    test.run_stack(Some(1));
    let periodic_msgs = test.dump_sinks();
    let accept = extract_location_update_accept(&periodic_msgs);
    assert_eq!(
        accept.location_update_accept_type,
        LocationUpdateAcceptType::PeriodicLocationUpdating
    );
    assert!(accept.group_identity_location_accept.is_none());
    assert!(
        swmi_group_attach_refresh_details(&periodic_msgs).is_empty(),
        "group-less follow-up LU must not duplicate the pending SwMI group refresh"
    );
    assert!(
        subscriber_updates(&periodic_msgs)
            .iter()
            .all(|update| update.action != BrewSubscriberAction::Deaffiliate),
        "group-less follow-up LU must not deaffiliate the provisionally restored restart group"
    );
    assert!(
        debug_mm_swmi_group_transaction_pending(&mut test, issi),
        "group-less follow-up LU must preserve the pending SwMI group refresh until ACK/T353"
    );
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    submit_swmi_group_ack(&mut test, issi, 123_456, false, vec![]);
    test.run_stack(Some(1));
    let ack_msgs = test.dump_sinks();
    assert!(!contains_attach_detach_ack(&ack_msgs));
    assert!(!debug_mm_swmi_group_transaction_pending(&mut test, issi));
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    test.run_stack(Some(721));
    let after_t353_msgs = test.dump_sinks();
    assert!(
        subscriber_updates(&after_t353_msgs)
            .iter()
            .all(|update| update.action != BrewSubscriberAction::Deaffiliate),
        "accepted SwMI group refresh must not roll back after T353"
    );
    assert!(
        !contains_location_update_command(&after_t353_msgs),
        "accepted SwMI group refresh must not re-request group report after T353"
    );
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_group_less_update_preserves_pending_swmi_refresh_until_t353() {
    debug::setup_logging_verbose();
    let issi = 2260616;
    let gssi = 226333;
    let path = unique_restart_recovery_path("cached-group-refresh-survives-group-less-lu-until-t353");
    std::fs::write(&path, format!("{issi} {gssi}:0:4\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.cell.energy_saving_mode = EnergySavingMode::Eg7 as u8;
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let _ = test.dump_sinks();

    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DemandLocationUpdating, None);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert_swmi_group_attach_refresh(&demand_msgs, gssi, 4, "group-less DemandLocationUpdating cached restart restore");
    assert!(debug_mm_swmi_group_transaction_pending(&mut test, issi));

    submit_location_update_with_type(&mut test, issi, LocationUpdateType::PeriodicLocationUpdating, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    assert!(debug_mm_swmi_group_transaction_pending(&mut test, issi));

    test.run_stack(Some(721));
    let timeout_msgs = test.dump_sinks();
    assert!(
        subscriber_updates(&timeout_msgs)
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Deaffiliate && update.groups == vec![gssi]),
        "T353 must still roll back the unconfirmed cached group after the group-less LU interleaving"
    );
    assert!(
        contains_location_update_command(&timeout_msgs),
        "T353 after the group-less LU interleaving should request a fresh group report"
    );
    assert!(!debug_mm_swmi_group_transaction_pending(&mut test, issi));
    assert!(test.config.state_read().subscribers.group_members(gssi).is_empty());

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_complete_group_report_abandons_pending_swmi_refresh() {
    debug::setup_logging_verbose();
    let issi = 2260616;
    let gssi = 226333;
    let path = unique_restart_recovery_path("complete-report-abandons-cached-swmi-refresh");
    std::fs::write(&path, format!("{issi} {gssi}:0:4\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.cell.energy_saving_mode = EnergySavingMode::Eg7 as u8;
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let _ = test.dump_sinks();

    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DemandLocationUpdating, None);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert_swmi_group_attach_refresh(&demand_msgs, gssi, 4, "group-less DemandLocationUpdating cached restart restore");
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);
    assert!(debug_mm_swmi_group_transaction_pending(&mut test, issi));

    // EN 300 392-2 clauses 16.4.4 and 16.10.27a make the explicit complete
    // group report authoritative. Unlike a group-less location update, it
    // does carry group state and must abandon the older cached SwMI refresh so
    // a late ACK cannot re-affiliate a group the MS just reported as absent.
    submit_location_update_with_group_report_response(&mut test, issi, LocationUpdateType::DemandLocationUpdating, 1, 0);
    test.run_stack(Some(1));
    let complete_msgs = test.dump_sinks();
    let accept = extract_location_update_accept(&complete_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    assert!(accept.group_identity_location_accept.is_none());
    assert!(!debug_mm_swmi_group_transaction_pending(&mut test, issi));
    assert!(
        subscriber_updates(&complete_msgs)
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Deaffiliate && update.groups == vec![gssi]),
        "explicit empty complete group report must clear the provisionally restored cached group"
    );
    assert!(test.config.state_read().subscribers.group_members(gssi).is_empty());

    submit_swmi_group_ack(&mut test, issi, 123_456, false, vec![]);
    test.run_stack(Some(1));
    let stale_ack_msgs = test.dump_sinks();
    assert!(
        stale_ack_msgs.is_empty()
            || subscriber_updates(&stale_ack_msgs)
                .iter()
                .all(|update| update.action != BrewSubscriberAction::Affiliate),
        "late ACK for abandoned cached SwMI refresh must not re-affiliate the cleared group"
    );
    assert!(test.config.state_read().subscribers.group_members(gssi).is_empty());

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_hard_roaming_location_update_abandons_pending_restart_group_refresh() {
    debug::setup_logging_verbose();
    let issi = 2260616;
    let gssi = 226333;
    let path = unique_restart_recovery_path("hard-roaming-abandons-cached-swmi-refresh");
    std::fs::write(&path, format!("{issi} {gssi}:0:4\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let _ = test.dump_sinks();

    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DemandLocationUpdating, None);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert_swmi_group_attach_refresh(&demand_msgs, gssi, 4, "group-less DemandLocationUpdating cached restart restore");
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);
    assert!(debug_mm_swmi_group_transaction_pending(&mut test, issi));

    backdate_mm_registration(&mut test, issi, 121);
    submit_location_update_with_type(&mut test, issi, LocationUpdateType::RoamingLocationUpdating, None);
    test.run_stack(Some(1));
    let roaming_msgs = test.dump_sinks();
    let accept = extract_location_update_accept(&roaming_msgs);
    assert_eq!(
        accept.location_update_accept_type,
        LocationUpdateAcceptType::RoamingLocationUpdating
    );
    assert!(!debug_mm_swmi_group_transaction_pending(&mut test, issi));
    assert!(
        subscriber_updates(&roaming_msgs)
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Deaffiliate && update.groups == vec![gssi]),
        "hard group-less roaming re-registration must clear the old restart group"
    );
    assert!(test.config.state_read().subscribers.group_members(gssi).is_empty());

    submit_swmi_group_ack(&mut test, issi, 123_456, false, vec![]);
    test.run_stack(Some(1));
    let stale_ack_msgs = test.dump_sinks();
    assert!(
        subscriber_updates(&stale_ack_msgs)
            .iter()
            .all(|update| update.action != BrewSubscriberAction::Affiliate),
        "late ACK from the abandoned restart refresh must not re-affiliate after hard roaming re-registration"
    );
    assert!(test.config.state_read().subscribers.group_members(gssi).is_empty());

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_rejected_location_update_abandons_pending_swmi_group_transaction() {
    debug::setup_logging_verbose();
    let issi = 2040814;
    let stale_group = 226333;
    let handle = 97;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Brew]);

    submit_location_update(&mut test, issi, None);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    begin_swmi_group_transaction_for_test(&mut test, issi, handle, vec![swmi_attach_group(stale_group)], false);

    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DisabledMsUpdating, None);
    test.run_stack(Some(1));
    let reject_msgs = test.dump_sinks();
    let reject = extract_location_update_reject(&reject_msgs);
    assert_eq!(reject.location_update_type, LocationUpdateType::DisabledMsUpdating);
    assert_eq!(reject.reject_cause, RejectCause::ServiceNotSubscribed as u8);
    assert!(!debug_mm_swmi_group_transaction_pending(&mut test, issi));

    submit_swmi_group_ack(&mut test, issi, handle, false, vec![]);
    test.run_stack(Some(1));
    let stale_ack_msgs = test.dump_sinks();
    assert!(stale_ack_msgs.is_empty(), "stale ACK after rejected LU should be ignored");
    assert!(!test.config.state_read().subscribers.group_members(stale_group).contains(&issi));
}

#[test]
fn test_restart_recovery_group_less_demand_segments_cached_scan_list_refresh() {
    debug::setup_logging_verbose();
    let issi = 2260618;
    let groups: Vec<u32> = (226300..=226312).collect();
    let path = unique_restart_recovery_path("cached-scan-list-segments");
    let cached_groups = groups.iter().map(|gssi| format!("{gssi}:0:4")).collect::<Vec<String>>().join(",");
    std::fs::write(&path, format!("{issi} {cached_groups}\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let _ = test.dump_sinks();

    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DemandLocationUpdating, None);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    let first_refreshes = swmi_group_attach_refresh_details(&demand_msgs);
    assert_eq!(first_refreshes.len(), 1);
    assert_eq!(
        first_refreshes[0].groups,
        groups[..12].iter().map(|gssi| (*gssi, 0, 4)).collect::<Vec<_>>()
    );
    assert_eq!(first_refreshes[0].layer2service, Layer2Service::Acknowledged);
    assert_ne!(first_refreshes[0].handle, 0);

    for gssi in &groups[..12] {
        assert_eq!(
            test.config.state_read().subscribers.group_members(*gssi),
            vec![issi],
            "first batch GSSI {gssi} should be provisionally restored"
        );
    }
    assert!(
        test.config.state_read().subscribers.group_members(groups[12]).is_empty(),
        "unsent cached scan-list group must not be locally restored before its over-air refresh"
    );
    let cache_after_first = std::fs::read_to_string(&path).expect("pending segmented refresh should preserve cache");
    for gssi in &groups {
        assert!(
            cache_after_first.lines().any(|line| line.contains(&gssi.to_string())),
            "pending segmented refresh should keep cached GSSI {gssi}, got {cache_after_first:?}"
        );
    }

    submit_swmi_group_ack(&mut test, issi, 123_456, false, vec![]);
    test.run_stack(Some(1));
    let second_msgs = test.dump_sinks();
    let second_refreshes = swmi_group_attach_refresh_details(&second_msgs);
    assert_eq!(second_refreshes.len(), 1);
    assert_eq!(second_refreshes[0].groups, vec![(groups[12], 0, 4)]);
    assert!(
        subscriber_updates(&second_msgs)
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Affiliate && update.groups == vec![groups[12]]),
        "ACK for first scan-list batch should restore and advertise the next batch"
    );
    assert_eq!(test.config.state_read().subscribers.group_members(groups[12]), vec![issi]);
    assert!(debug_mm_swmi_group_transaction_pending(&mut test, issi));

    submit_swmi_group_ack(&mut test, issi, 123_457, false, vec![]);
    test.run_stack(Some(1));
    let final_ack_msgs = test.dump_sinks();
    assert!(swmi_group_attach_refresh_details(&final_ack_msgs).is_empty());
    assert!(!debug_mm_swmi_group_transaction_pending(&mut test, issi));
    for gssi in &groups {
        assert_eq!(test.config.state_read().subscribers.group_members(*gssi), vec![issi]);
    }

    let cache = std::fs::read_to_string(&path).expect("segmented refresh should persist restored scan list");
    for gssi in &groups {
        assert!(
            cache.lines().any(|line| line.contains(&gssi.to_string())),
            "final segmented refresh cache should retain GSSI {gssi}, got {cache:?}"
        );
    }

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_segmented_group_refresh_t353_preserves_unsent_cached_groups() {
    debug::setup_logging_verbose();
    let issi = 2260618;
    let groups: Vec<u32> = (226300..=226312).collect();
    let path = unique_restart_recovery_path("cached-scan-list-segment-t353-preserves-remaining");
    let cached_groups = groups.iter().map(|gssi| format!("{gssi}:0:4")).collect::<Vec<String>>().join(",");
    std::fs::write(&path, format!("{issi} {cached_groups}\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let _ = test.dump_sinks();

    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DemandLocationUpdating, None);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    let first_refreshes = swmi_group_attach_refresh_details(&demand_msgs);
    assert_eq!(first_refreshes.len(), 1);
    assert_eq!(
        first_refreshes[0].groups,
        groups[..12].iter().map(|gssi| (*gssi, 0, 4)).collect::<Vec<_>>()
    );
    assert!(debug_mm_swmi_group_transaction_pending(&mut test, issi));

    test.run_stack(Some(721));
    let expired_msgs = test.dump_sinks();
    assert!(
        subscriber_updates(&expired_msgs)
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Deaffiliate && update.groups.as_slice() == &groups[..12]),
        "T353 expiry should roll back only the unconfirmed first scan-list batch"
    );
    assert!(
        contains_location_update_command(&expired_msgs),
        "T353 expiry should request a fresh authoritative group report"
    );
    for gssi in &groups {
        assert!(
            test.config.state_read().subscribers.group_members(*gssi).is_empty(),
            "GSSI {gssi} should not remain provisionally affiliated after T353 expiry"
        );
    }

    let cache = std::fs::read_to_string(&path).expect("T353 segmented rollback should persist cache");
    for gssi in &groups[..12] {
        assert!(
            !cache.lines().any(|line| line.contains(&gssi.to_string())),
            "unconfirmed first-batch GSSI {gssi} should be removed after T353, got {cache:?}"
        );
    }
    assert!(
        cache.lines().any(|line| line.contains(&groups[12].to_string())),
        "unsent cached scan-list GSSI {} should stay in restart cache, got {cache:?}",
        groups[12]
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_group_refresh_reject_rolls_back_cached_affiliation_and_reprobes() {
    debug::setup_logging_verbose();
    let issi = 2260618;
    let gssi = 226333;
    let path = unique_restart_recovery_path("cached-group-refresh-reject");
    std::fs::write(&path, format!("{issi} {gssi}:0:4\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let _ = test.dump_sinks();

    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DemandLocationUpdating, None);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert_swmi_group_attach_refresh(&demand_msgs, gssi, 4, "group-less DemandLocationUpdating cached restart restore");
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);
    assert!(debug_mm_swmi_group_transaction_pending(&mut test, issi));

    submit_swmi_group_ack(&mut test, issi, 0, true, vec![gssi]);
    test.run_stack(Some(1));
    let reject_msgs = test.dump_sinks();

    assert!(!contains_attach_detach_ack(&reject_msgs));
    assert!(
        subscriber_updates(&reject_msgs)
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Deaffiliate && update.groups == vec![gssi]),
        "rejected restart refresh must roll back the provisional cached affiliation"
    );
    assert!(
        contains_location_update_command(&reject_msgs),
        "rejected restart refresh should request a fresh group report"
    );
    assert!(debug_mm_solicited_group_report_pending(&mut test, issi));
    assert!(!debug_mm_swmi_group_transaction_pending(&mut test, issi));
    assert!(test.config.state_read().subscribers.group_members(gssi).is_empty());

    let cache = std::fs::read_to_string(&path).expect("reject rollback should persist cache");
    assert!(
        cache.lines().any(|line| line.trim() == issi.to_string()),
        "reject rollback should keep bare ISSI recovery entry, got {cache:?}"
    );
    assert!(
        !cache.lines().any(|line| line.contains(&gssi.to_string())),
        "reject rollback must remove unconfirmed cached GSSI {gssi}, got {cache:?}"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_group_refresh_accepts_unrouted_nonzero_ack_without_t353_purge() {
    debug::setup_logging_verbose();
    let issi = 2260616;
    let gssi = 226333;
    let path = unique_restart_recovery_path("cached-group-refresh-unrouted-nonzero-ack");
    std::fs::write(&path, format!("{issi} {gssi}:0:4\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let _ = test.dump_sinks();

    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DemandLocationUpdating, None);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert_swmi_group_attach_refresh(&demand_msgs, gssi, 4, "group-less DemandLocationUpdating cached restart restore");
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);
    assert!(debug_mm_swmi_group_transaction_pending(&mut test, issi));

    // EN 300 392-2 clause 16.8.1 defines the ACK procedure over air, but the
    // MLE primitive handle is local plumbing. Restart refresh must accept the
    // same-ISSI ACK even when that local handle is not routed back.
    submit_swmi_group_ack(&mut test, issi, 123_456, false, vec![]);
    test.run_stack(Some(1));
    let ack_msgs = test.dump_sinks();
    assert!(!contains_attach_detach_ack(&ack_msgs));
    assert!(
        subscriber_updates(&ack_msgs)
            .iter()
            .all(|update| update.action != BrewSubscriberAction::Deaffiliate),
        "accepted unrouted ACK must not deaffiliate cached restart group"
    );
    assert!(
        !contains_location_update_command(&ack_msgs),
        "accepted unrouted ACK must not request a fresh group report"
    );
    assert!(!debug_mm_swmi_group_transaction_pending(&mut test, issi));
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    test.run_stack(Some(721));
    let after_t353_msgs = test.dump_sinks();
    assert!(
        subscriber_updates(&after_t353_msgs)
            .iter()
            .all(|update| update.action != BrewSubscriberAction::Deaffiliate),
        "accepted unrouted ACK must prevent later T353 rollback"
    );
    assert!(
        !contains_location_update_command(&after_t353_msgs),
        "accepted unrouted ACK must prevent later T353 reprobe"
    );
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    let cache = std::fs::read_to_string(&path).expect("accepted ACK should preserve cache");
    assert!(
        cache.lines().any(|line| line.trim() == format!("{issi} {gssi}:0:4")),
        "accepted ACK should keep cached GSSI {gssi}, got {cache:?}"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_group_refresh_t353_expiry_rolls_back_cached_affiliation_and_reprobes() {
    debug::setup_logging_verbose();
    let issi = 2260618;
    let gssi = 226333;
    let path = unique_restart_recovery_path("cached-group-refresh-t353");
    std::fs::write(&path, format!("{issi} {gssi}:0:4\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let _ = test.dump_sinks();

    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DemandLocationUpdating, None);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert_swmi_group_attach_refresh(&demand_msgs, gssi, 4, "group-less DemandLocationUpdating cached restart restore");
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);
    assert!(debug_mm_swmi_group_transaction_pending(&mut test, issi));

    test.run_stack(Some(721));
    let timeout_msgs = test.dump_sinks();
    assert!(
        subscriber_updates(&timeout_msgs)
            .iter()
            .any(|update| update.action == BrewSubscriberAction::Deaffiliate && update.groups == vec![gssi]),
        "T353 expiry must roll back the provisional cached affiliation"
    );
    assert!(
        contains_location_update_command(&timeout_msgs),
        "T353 expiry should request a fresh group report"
    );
    assert!(debug_mm_solicited_group_report_pending(&mut test, issi));
    assert!(!debug_mm_swmi_group_transaction_pending(&mut test, issi));
    assert!(test.config.state_read().subscribers.group_members(gssi).is_empty());

    let cache = std::fs::read_to_string(&path).expect("T353 rollback should persist cache");
    assert!(
        cache.lines().any(|line| line.trim() == issi.to_string()),
        "T353 rollback should keep bare ISSI recovery entry, got {cache:?}"
    );
    assert!(
        !cache.lines().any(|line| line.contains(&gssi.to_string())),
        "T353 rollback must remove unconfirmed cached GSSI {gssi}, got {cache:?}"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_empty_complete_report_clears_cached_affiliation() {
    debug::setup_logging_verbose();
    let issi = 2260618;
    let gssi = 226333;
    let path = unique_restart_recovery_path("cached-group-empty");
    std::fs::write(&path, format!("{issi} {gssi}:0:4\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    assert_eq!(location_update_commands(&test.dump_sinks()).len(), 1);

    submit_location_update_with_group_report_response(&mut test, issi, LocationUpdateType::DemandLocationUpdating, 1, 0);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&demand_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    assert!(accept.group_identity_location_accept.is_none());
    assert!(test.config.state_read().subscribers.group_members(gssi).is_empty());

    let cache = std::fs::read_to_string(&path).expect("empty report should keep ISSI cache without group");
    assert!(
        cache.lines().any(|line| line.trim() == issi.to_string()),
        "cache should keep ISSI for future recovery, got {cache:?}"
    );
    assert!(
        !cache.lines().any(|line| line.contains(&gssi.to_string())),
        "explicit empty complete report must clear cached GSSI {gssi}, got {cache:?}"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_explicit_group_report_replaces_cached_affiliation() {
    debug::setup_logging_verbose();
    let issi = 2260618;
    let old_gssi = 226333;
    let new_gssi = 226444;
    let path = unique_restart_recovery_path("cached-group-replace");
    std::fs::write(&path, format!("{issi} {old_gssi}:0:4\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    assert_eq!(location_update_commands(&test.dump_sinks()).len(), 1);

    submit_location_update_with_groups_and_group_report_response(
        &mut test,
        issi,
        LocationUpdateType::DemandLocationUpdating,
        vec![new_gssi],
        1,
        0,
    );
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&demand_msgs);
    let gila = accept
        .group_identity_location_accept
        .as_ref()
        .expect("explicit group report should be acknowledged");
    let accepted_groups = gila.group_identity_downlink.as_ref().expect("reported group should be listed");
    assert!(accepted_groups.iter().any(|group| group.gssi == Some(new_gssi)));

    let state = test.config.state_read();
    assert!(state.subscribers.group_members(old_gssi).is_empty());
    assert_eq!(state.subscribers.group_members(new_gssi), vec![issi]);
    drop(state);

    let cache = std::fs::read_to_string(&path).expect("explicit report should replace cached group");
    assert!(cache.lines().any(|line| line.contains(&format!("{new_gssi}:0:0"))));
    assert!(
        !cache.lines().any(|line| line.contains(&format!("{old_gssi}:"))),
        "explicit report must replace old cached GSSI {old_gssi}, got {cache:?}"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_demand_location_update_restores_affiliation_and_eg3() {
    debug::setup_logging_verbose();
    let issi = 2260618;
    let gssi = 226333;
    let path = unique_restart_recovery_path("demand-groups-eg3");
    std::fs::write(&path, format!("{issi}\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.cell.energy_saving_mode = EnergySavingMode::Eg3 as u8;
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let command_msgs = test.dump_sinks();
    let commands = location_update_commands(&command_msgs);
    let command_details = location_update_command_details(&command_msgs);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].0, issi);
    assert!(commands[0].2.group_identity_report);
    assert_eq!(command_details.len(), 1);
    assert_eq!(command_details[0].0, issi);
    assert_eq!(command_details[0].2, Layer2Service::Acknowledged);
    {
        let state = test.config.state_read();
        assert!(
            !state.subscribers.is_registered(issi),
            "restart recovery command alone must not fabricate a registered terminal"
        );
        assert!(state.subscribers.group_members(gssi).is_empty());
        assert!(
            !state.energy_saving.contains_key(&issi),
            "restart recovery command alone must not install stale EG state before the MS confirms"
        );
    }

    // EN 300 392-2 clause 16.4.4 lets the SwMI command a still-camped MS to
    // perform location update. Clauses 16.9.3.4, 16.10.23 and 16.10.35a keep
    // the accepted DemandLocationUpdating response and group identity result
    // coherent. Clauses 16.7.1, 16.10.9, 16.10.10, and 23.7.6/T.210 cover the
    // rebuilt energy economy assignment and receive-window timing.
    submit_location_update_with_groups_and_energy(
        &mut test,
        issi,
        LocationUpdateType::DemandLocationUpdating,
        vec![gssi],
        Some(EnergySavingMode::Eg1),
    );
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&demand_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    let gila = accept
        .group_identity_location_accept
        .as_ref()
        .expect("DemandLocationUpdating accept should acknowledge reported groups");
    let accepted_groups = gila
        .group_identity_downlink
        .as_ref()
        .expect("DemandLocationUpdating accept should list accepted groups");
    assert_eq!(gila.group_identity_accept_reject, 0);
    assert!(accepted_groups.iter().any(|group| {
        group.gssi == Some(gssi)
            && group
                .group_identity_attachment
                .as_ref()
                .is_some_and(|attachment| attachment.group_identity_attachment_lifetime == 0 && attachment.class_of_usage == 0)
            && group.group_identity_detachment_uplink.is_none()
    }));
    let esi = accept
        .energy_saving_information
        .expect("DemandLocationUpdating accept should carry allocated EG3");
    assert_eq!(esi.energy_saving_mode, EnergySavingMode::Eg3);
    assert_energy_saving_start_avoids_frame_18(EnergySavingMode::Eg3, esi.frame_number, esi.multiframe_number);
    assert!(
        debug_mm_solicited_group_report_pending(&mut test, issi),
        "reported group identities without group-report-complete must keep the solicited restart group-report window open"
    );

    let updates = subscriber_updates(&demand_msgs);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].action, BrewSubscriberAction::Register);
    assert_eq!(updates[0].issi, issi);
    assert!(updates[0].groups.is_empty());
    assert_eq!(updates[1].action, BrewSubscriberAction::Affiliate);
    assert_eq!(updates[1].issi, issi);
    assert_eq!(updates[1].groups, vec![gssi]);

    {
        let state = test.config.state_read();
        assert!(state.subscribers.is_registered(issi));
        assert_eq!(state.subscribers.group_members(gssi), vec![issi]);
        let assignment = state
            .energy_saving
            .get(&issi)
            .expect("accepted EG3 assignment should be active after DemandLocationUpdating");
        assert_eq!(assignment.mode, EnergySavingMode::Eg3 as u8);
        assert_eq!(assignment.frame, esi.frame_number);
        assert_eq!(assignment.multiframe, esi.multiframe_number);
        assert!(assignment.awake_until.is_some());
    }

    test.run_stack(Some(2));
    let followup_msgs = test.dump_sinks();
    assert!(
        location_update_commands(&followup_msgs).is_empty(),
        "successful restart recovery response must stop further D-LOCATION-UPDATE-COMMAND probes"
    );

    let cache = std::fs::read_to_string(&path).expect("registration should keep recovery cache");
    assert!(
        cache
            .lines()
            .any(|line| line.trim().starts_with(&issi.to_string()) && line.contains(&format!("{gssi}:0:0"))),
        "cache should keep recovered ISSI/GSSI, got {cache:?}"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_failed_location_update_accept_reprobes_registration() {
    debug::setup_logging_verbose();
    let issi = 2260618;
    let gssi = 226333;
    let path = unique_restart_recovery_path("failed-accept-reprobe");
    std::fs::write(&path, format!("{issi}\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.cell.energy_saving_mode = EnergySavingMode::Eg3 as u8;
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    assert_eq!(location_update_commands(&test.dump_sinks()).len(), 1);

    submit_location_update_with_groups_and_energy(
        &mut test,
        issi,
        LocationUpdateType::DemandLocationUpdating,
        vec![gssi],
        Some(EnergySavingMode::Eg1),
    );
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    let accept_details = location_update_accept_details(&demand_msgs);
    assert_eq!(accept_details.len(), 1);
    let accept_handle = accept_details[0].1;
    assert!(
        accept_handle >= 0x8000_0000,
        "D-LOCATION UPDATE ACCEPT should use a tracked local MLE handle"
    );
    assert_eq!(accept_details[0].2, Layer2Service::Acknowledged);
    assert_eq!(
        accept_details[0].3.location_update_accept_type,
        LocationUpdateAcceptType::DemandLocationUpdating
    );
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);
    assert!(test.config.state_read().energy_saving.contains_key(&issi));

    // LLC/MLE reports are local SAP confirmations. EN 300 392-2 clause 16.4.4
    // lets the SwMI repeat D-LOCATION UPDATE COMMAND when the accepted
    // registration was not confirmed at layer 2. The reprobe must not
    // immediately deregister the already observed subscriber, because the MS
    // may still be transmitting MAC access while it answers the new command.
    submit_lmm_mle_report(&mut test, accept_handle, TLA_REPORT_FAILED_TRANSFER);
    test.run_stack(Some(1));
    let failed_msgs = test.dump_sinks();

    let commands = location_update_commands(&failed_msgs);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].0, issi);
    assert!(commands[0].2.group_identity_report);

    let updates = subscriber_updates(&failed_msgs);
    assert!(
        updates.is_empty(),
        "failed D-LOCATION UPDATE ACCEPT transfer should reprobe without dropping the local CMCE/Brew subscriber route"
    );

    let state = test.config.state_read();
    assert!(
        state.subscribers.is_registered(issi),
        "registration reprobe should preserve the local subscriber until timeout/detach/reject"
    );
    assert_eq!(
        state.subscribers.group_members(gssi),
        vec![issi],
        "registration reprobe should preserve group affiliation for PTT while the MS answers D-LOCATION-UPDATE-COMMAND"
    );
    assert!(
        !state.energy_saving.contains_key(&issi),
        "failed registration accept must fail open to StayAlive before re-probing"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_demand_location_update_accepts_complete_group_report_with_groups() {
    debug::setup_logging_verbose();
    let issi = 2260616;
    let gssi = 226333;
    let path = unique_restart_recovery_path("demand-groups-complete");
    std::fs::write(&path, format!("{issi}\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let command_msgs = test.dump_sinks();
    let command_details = location_update_command_details(&command_msgs);
    assert_eq!(command_details.len(), 1);
    assert_eq!(command_details[0].0, issi);
    assert!(command_details[0].3.group_identity_report);

    // EN 300 392-2 clause 16.4.4 says a BS-commanded group report may carry
    // the reported groups in U-LOCATION UPDATE DEMAND. If all reported groups
    // fit in that PDU, the group report response IE shall also indicate
    // "group report complete" (clause 16.10.27a value 0).
    submit_location_update_with_groups_and_group_report_response(
        &mut test,
        issi,
        LocationUpdateType::DemandLocationUpdating,
        vec![gssi],
        1,
        0,
    );
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&demand_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    let gila = accept
        .group_identity_location_accept
        .as_ref()
        .expect("complete group report with groups should be acknowledged");
    assert_eq!(gila.group_identity_accept_reject, 0);
    let accepted_groups = gila
        .group_identity_downlink
        .as_ref()
        .expect("accepted group should be listed in GroupIdentityLocationAccept");
    assert_eq!(accepted_groups.len(), 1);
    assert_eq!(accepted_groups[0].gssi, Some(gssi));
    assert!(accepted_groups[0].group_identity_attachment.is_some());
    assert!(
        !contains_location_update_command(&demand_msgs),
        "complete solicited group report must not trigger another D-LOCATION-UPDATE-COMMAND"
    );

    let updates = subscriber_updates(&demand_msgs);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].action, BrewSubscriberAction::Register);
    assert_eq!(updates[0].issi, issi);
    assert!(updates[0].groups.is_empty());
    assert_eq!(updates[1].action, BrewSubscriberAction::Affiliate);
    assert_eq!(updates[1].issi, issi);
    assert_eq!(updates[1].groups, vec![gssi]);

    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(issi));
    assert_eq!(state.subscribers.group_members(gssi), vec![issi]);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_accepts_solicited_attach_detach_group_report_completion() {
    debug::setup_logging_verbose();
    let issi = 2260082;
    let gssi = 226333;
    let path = unique_restart_recovery_path("demand-followup-attach-complete");
    std::fs::write(&path, format!("{issi}\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let command_msgs = test.dump_sinks();
    let command_details = location_update_command_details(&command_msgs);
    assert_eq!(command_details.len(), 1);
    assert_eq!(command_details[0].0, issi);
    assert!(command_details[0].3.group_identity_report);

    // Some terminals answer D-LOCATION UPDATE COMMAND first with the demanded
    // registration PDU, then report the desired groups in a following
    // U-ATTACH/DETACH GROUP IDENTITY. EN 300 392-2 clause 16.4.4 permits that
    // follow-up path; while the report is pending, the BS must not start a new
    // location-update-command loop.
    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DemandLocationUpdating, None);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    let accept = extract_location_update_accept(&demand_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    assert!(
        !contains_location_update_command(&demand_msgs),
        "pending solicited group report should prevent an immediate duplicate command"
    );
    let updates = subscriber_updates(&demand_msgs);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].action, BrewSubscriberAction::Register);
    assert!(test.config.state_read().subscribers.group_members(gssi).is_empty());

    submit_attach_detach_group_identity_with_report_response(
        &mut test,
        issi,
        true,
        vec![GroupIdentityUplink {
            class_of_usage: Some(0),
            group_identity_detachment_uplink: None,
            gssi: Some(gssi),
            address_extension: None,
            vgssi: None,
        }],
        1,
        0,
    );
    test.run_stack(Some(1));
    let report_msgs = test.dump_sinks();

    let ack = extract_attach_detach_ack(&report_msgs);
    assert_eq!(ack.group_identity_accept_reject, 0);
    let accepted_groups = ack
        .group_identity_downlink
        .as_ref()
        .expect("solicited group report completion should ACK accepted groups");
    assert_eq!(accepted_groups.len(), 1);
    assert_eq!(accepted_groups[0].gssi, Some(gssi));
    assert!(accepted_groups[0].group_identity_attachment.is_some());

    let updates = subscriber_updates(&report_msgs);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].action, BrewSubscriberAction::Affiliate);
    assert_eq!(updates[0].issi, issi);
    assert_eq!(updates[0].groups, vec![gssi]);
    assert_eq!(test.config.state_read().subscribers.group_members(gssi), vec![issi]);

    test.run_stack(Some(2));
    let followup_msgs = test.dump_sinks();
    assert!(
        location_update_commands(&followup_msgs).is_empty(),
        "completed solicited attach/detach report must stop restart recovery probes"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_group_report_complete_keeps_groups_empty() {
    debug::setup_logging_verbose();
    let issi = 2260618;
    let gssi = 226333;
    let path = unique_restart_recovery_path("demand-no-groups");
    std::fs::write(&path, format!("{issi}\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    let command_msgs = test.dump_sinks();
    let command_details = location_update_command_details(&command_msgs);
    assert_eq!(command_details.len(), 1);
    assert_eq!(command_details[0].0, issi);
    assert_eq!(command_details[0].2, Layer2Service::Acknowledged);
    assert!(command_details[0].3.group_identity_report);

    // EN 300 392-2 clause 16.4.4 allows SwMI-driven location updating after
    // restart. Clauses 16.9.3.4 and 16.10.27a define the terminal's complete
    // empty group report; the SwMI must rebuild active state from that response
    // and must not restore stale groups from the restart cache.
    submit_location_update_with_group_report_response(&mut test, issi, LocationUpdateType::DemandLocationUpdating, 1, 0);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let accept = extract_location_update_accept(&demand_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    assert!(
        accept.group_identity_location_accept.is_none(),
        "empty complete group report must not advertise stale GSSI entries after restart recovery"
    );

    let updates = subscriber_updates(&demand_msgs);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].action, BrewSubscriberAction::Register);
    assert_eq!(updates[0].issi, issi);
    assert!(updates[0].groups.is_empty());

    {
        let state = test.config.state_read();
        assert!(state.subscribers.is_registered(issi));
        assert!(state.subscribers.group_members(gssi).is_empty());
        assert!(!state.energy_saving.contains_key(&issi));
    }

    test.run_stack(Some(2));
    let followup_msgs = test.dump_sinks();
    assert!(
        location_update_commands(&followup_msgs).is_empty(),
        "successful empty restart recovery response must stop further D-LOCATION-UPDATE-COMMAND probes"
    );

    let cache = std::fs::read_to_string(&path).expect("registration should keep recovery cache");
    assert!(cache.lines().any(|line| line.trim() == issi.to_string()));

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_re_requests_group_report_when_recovered_without_groups() {
    debug::setup_logging_verbose();
    let issi = 2260082;
    let gssi = 226333;
    let path = unique_restart_recovery_path("demand-no-followup-groups");
    std::fs::write(&path, format!("{issi}\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    test.run_stack(Some(73));
    assert_eq!(location_update_commands(&test.dump_sinks()).len(), 1);

    // EN 300 392-2 clause 16.4.4 lets the SwMI request a group report with
    // D-LOCATION UPDATE COMMAND. If the terminal answers the location update
    // but does not include groups and does not send the promised follow-up
    // U-ATTACH/DETACH GROUP IDENTITY, local restart recovery must continue
    // requesting the group report instead of leaving CMCE with no listener for
    // the terminal's scan group.
    submit_location_update_with_type(&mut test, issi, LocationUpdateType::DemandLocationUpdating, None);
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    let accept = extract_location_update_accept(&demand_msgs);
    assert_eq!(accept.location_update_accept_type, LocationUpdateAcceptType::DemandLocationUpdating);
    assert!(!contains_location_update_command(&demand_msgs));
    assert!(debug_mm_solicited_group_report_pending(&mut test, issi));

    {
        let state = test.config.state_read();
        assert!(state.subscribers.is_registered(issi));
        assert!(state.subscribers.group_members(gssi).is_empty());
    }

    test.run_stack(Some(60 * 18 * 4 + 1));
    let retry_msgs = test.dump_sinks();
    let retry_commands = location_update_commands(&retry_msgs);
    assert_eq!(retry_commands.len(), 1);
    assert_eq!(retry_commands[0].0, issi);
    assert!(retry_commands[0].2.group_identity_report);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_retries_are_long_lived_and_paced() {
    debug::setup_logging_verbose();
    let issi = 2260618;
    let path = unique_restart_recovery_path("retry-paced");
    std::fs::write(&path, format!("{issi}\n")).expect("failed to seed recovery cache");

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    test.run_stack(Some(73));
    let first_msgs = test.dump_sinks();
    let first_commands = location_update_commands(&first_msgs);
    assert_eq!(first_commands.len(), 1);
    assert_eq!(first_commands[0].0, issi);

    // EN 300 392-2 clause 16.4.4 gives the SwMI the registration command
    // procedure; the retry cadence here is local RF policy. Keep retries
    // long-lived enough for post-restart radios that miss early commands, but
    // leave a quiet gap after LLC retransmission exhaustion before retrying.
    test.run_stack(Some(2 * 18 * 4 - 1));
    assert!(
        location_update_commands(&test.dump_sinks()).is_empty(),
        "restart recovery retry must wait for the paced local retry interval"
    );

    test.run_stack(Some(1));
    let retry_msgs = test.dump_sinks();
    let retry_commands = location_update_commands(&retry_msgs);
    assert_eq!(retry_commands.len(), 1);
    assert_eq!(retry_commands[0].0, issi);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

fn assert_location_update_response_stay_alive(energy_saving_mode: Option<EnergySavingMode>) {
    debug::setup_logging_verbose();

    let pdu = ULocationUpdateDemand {
        location_update_type: LocationUpdateType::ItsiAttach,
        request_to_append_la: false,
        cipher_control: false,
        ciphering_parameters: None,
        class_of_ms: None,
        energy_saving_mode,
        la_information: None,
        ssi: None,
        address_extension: None,
        group_identity_location_demand: None,
        group_report_response: None,
        authentication_uplink: None,
        extended_capabilities: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(32);
    pdu.to_bitbuf(&mut sdu).unwrap();
    sdu.seek(0);

    let test_prim = LmmMleUnitdataInd {
        sdu,
        handle: 0,
        received_address: TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 2040814,
        },
    };
    let test_sapmsg = SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(test_prim),
    };

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    let components = vec![TetraEntity::Mm];
    let sinks: Vec<TetraEntity> = vec![TetraEntity::Mle];
    test.populate_entities(components, sinks);

    test.submit_message(test_sapmsg);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::LmmMleUnitdataReq(ref resp_prim) = sink_msgs[0].msg else {
        panic!("Expected LmmMleUnitdataReq");
    };
    let mut resp_sdu = BitBuffer::from_bitstr(&resp_prim.sdu.to_bitstr());
    let resp_pdu = DLocationUpdateAccept::from_bitbuf(&mut resp_sdu).expect("Failed parsing D-LOCATION UPDATE ACCEPT response");
    assert_eq!(resp_pdu.location_update_accept_type, LocationUpdateAcceptType::ItsiAttach);
    let esi = resp_pdu
        .energy_saving_information
        .expect("D-LOCATION UPDATE ACCEPT must answer requested energy saving mode");
    assert_eq!(esi.energy_saving_mode, EnergySavingMode::StayAlive);
    assert_eq!(esi.frame_number, None);
    assert_eq!(esi.multiframe_number, None);
}

fn energy_economy_modes_for_test() -> [EnergySavingMode; 7] {
    [
        EnergySavingMode::Eg1,
        EnergySavingMode::Eg2,
        EnergySavingMode::Eg3,
        EnergySavingMode::Eg4,
        EnergySavingMode::Eg5,
        EnergySavingMode::Eg6,
        EnergySavingMode::Eg7,
    ]
}

fn dltime_for_frame_18_energy_start(issi: u32, mode: EnergySavingMode) -> TdmaTime {
    let cycle_frames = EnergySavingAssignment::sleep_frames(mode as u8).expect("EG mode should have table 23.9 sleep frames") + 1;
    let spread_frames = (issi % cycle_frames as u32) as u8;
    assert!(
        spread_frames < 18,
        "test ISSI must spread each EG start within the current multiframe"
    );
    let dltime = TdmaTime {
        t: 1,
        f: 18 - spread_frames,
        m: 1,
        h: 0,
    };
    let raw_start = dltime.add_timeslots((2 * 18 + spread_frames as i32) * 4);
    assert_eq!(raw_start.f, 18, "test setup must force an unguarded EG start onto frame 18");
    dltime
}

fn assert_energy_saving_start_avoids_frame_18(mode: EnergySavingMode, frame: Option<u8>, multiframe: Option<u8>) {
    assert_ne!(frame, Some(18), "mode {mode:?} must not start EG on frame 18");
    let start = TdmaTime {
        t: 1,
        f: frame.expect("EG start should carry frame number"),
        m: multiframe.expect("EG start should carry multiframe number"),
        h: 0,
    };
    let cycle_frames = EnergySavingAssignment::sleep_frames(mode as u8).expect("EG mode should have table 23.9 sleep frames") + 1;
    let receive_opportunities_in_full_multiframe_cycle = (18 * 60) / cycle_frames as i32;
    for n in 1..=receive_opportunities_in_full_multiframe_cycle {
        let receive = start.add_timeslots((cycle_frames as i32) * n * 4);
        assert_ne!(receive.f, 18, "mode {mode:?} receive opportunity {n} must not recur on frame 18");
    }
}

fn extract_location_update_accept(msgs: &[SapMsg]) -> DLocationUpdateAccept {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                DLocationUpdateAccept::from_bitbuf(&mut sdu).ok()
            }
            _ => None,
        })
        .expect("expected D-LOCATION UPDATE ACCEPT")
}

fn location_update_accept_details(msgs: &[SapMsg]) -> Vec<(u32, u32, Layer2Service, DLocationUpdateAccept)> {
    msgs.iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                let pdu = DLocationUpdateAccept::from_bitbuf(&mut sdu).ok()?;
                Some((prim.address.ssi, prim.handle, prim.layer2service.clone(), pdu))
            }
            _ => None,
        })
        .collect()
}

fn contains_location_update_accept(msgs: &[SapMsg]) -> bool {
    msgs.iter().any(|msg| match &msg.msg {
        SapMsgInner::LmmMleUnitdataReq(prim) => {
            let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
            DLocationUpdateAccept::from_bitbuf(&mut sdu).is_ok()
        }
        _ => false,
    })
}

fn contains_location_update_command(msgs: &[SapMsg]) -> bool {
    msgs.iter().any(|msg| match &msg.msg {
        SapMsgInner::LmmMleUnitdataReq(prim) => {
            let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
            DLocationUpdateCommand::from_bitbuf(&mut sdu).is_ok()
        }
        _ => false,
    })
}

fn location_update_commands(msgs: &[SapMsg]) -> Vec<(u32, u32, DLocationUpdateCommand)> {
    msgs.iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                let pdu = DLocationUpdateCommand::from_bitbuf(&mut sdu).ok()?;
                Some((prim.address.ssi, prim.handle, pdu))
            }
            _ => None,
        })
        .collect()
}

fn location_update_command_details(msgs: &[SapMsg]) -> Vec<(u32, u32, Layer2Service, DLocationUpdateCommand)> {
    msgs.iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                let pdu = DLocationUpdateCommand::from_bitbuf(&mut sdu).ok()?;
                Some((prim.address.ssi, prim.handle, prim.layer2service.clone(), pdu))
            }
            _ => None,
        })
        .collect()
}

fn mm_downlink_pdu_types(msgs: &[SapMsg]) -> Vec<MmPduTypeDl> {
    msgs.iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => {
                let sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                MmPduTypeDl::try_from(sdu.peek_bits(4)?).ok()
            }
            _ => None,
        })
        .collect()
}

fn extract_location_update_reject(msgs: &[SapMsg]) -> DLocationUpdateReject {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                DLocationUpdateReject::from_bitbuf(&mut sdu).ok()
            }
            _ => None,
        })
        .expect("expected D-LOCATION UPDATE REJECT")
}

fn extract_attach_detach_ack(msgs: &[SapMsg]) -> DAttachDetachGroupIdentityAcknowledgement {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                DAttachDetachGroupIdentityAcknowledgement::from_bitbuf(&mut sdu).ok()
            }
            _ => None,
        })
        .expect("expected D-ATTACH-DETACH GROUP IDENTITY ACKNOWLEDGEMENT")
}

fn extract_d_attach_detach_group_identity(msgs: &[SapMsg]) -> (DAttachDetachGroupIdentity, Layer2Service) {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                DAttachDetachGroupIdentity::from_bitbuf(&mut sdu)
                    .ok()
                    .map(|pdu| (pdu, prim.layer2service))
            }
            _ => None,
        })
        .expect("expected D-ATTACH-DETACH GROUP IDENTITY")
}

fn extract_d_attach_detach_group_identities(msgs: &[SapMsg]) -> Vec<(DAttachDetachGroupIdentity, Layer2Service)> {
    msgs.iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                DAttachDetachGroupIdentity::from_bitbuf(&mut sdu)
                    .ok()
                    .map(|pdu| (pdu, prim.layer2service))
            }
            _ => None,
        })
        .collect()
}

#[derive(Debug, PartialEq)]
struct SwmiGroupAttachRefreshDetails {
    groups: Vec<(u32, u8, u8)>,
    layer2service: Layer2Service,
    handle: u32,
}

fn swmi_group_attach_refresh_details(msgs: &[SapMsg]) -> Vec<SwmiGroupAttachRefreshDetails> {
    msgs.iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                DAttachDetachGroupIdentity::from_bitbuf(&mut sdu)
                    .ok()
                    .map(|pdu| (pdu, prim.layer2service, prim.handle))
            }
            _ => None,
        })
        .filter(|(pdu, _, _)| {
            !pdu.group_identity_report
                && pdu.group_identity_acknowledgement_request
                && !pdu.group_identity_attach_detach_mode
                && pdu.group_report_response.is_none()
        })
        .map(|(pdu, layer2service, handle)| SwmiGroupAttachRefreshDetails {
            groups: pdu
                .group_identity_downlink
                .unwrap_or_default()
                .into_iter()
                .filter_map(|group| {
                    let attachment = group.group_identity_attachment?;
                    Some((
                        group.gssi?,
                        attachment.group_identity_attachment_lifetime,
                        attachment.class_of_usage,
                    ))
                })
                .collect(),
            layer2service,
            handle,
        })
        .collect()
}

fn assert_swmi_group_attach_refresh(msgs: &[SapMsg], gssi: u32, class_of_usage: u8, context: &str) {
    let refreshes = swmi_group_attach_refresh_details(msgs);
    assert_eq!(refreshes.len(), 1, "{context}: expected one SwMI group attach refresh");

    let refresh = &refreshes[0];
    assert_ne!(
        refresh.handle, 0,
        "{context}: SwMI group attach refresh should use a non-zero local downlink handle"
    );
    assert_eq!(
        refresh.layer2service,
        Layer2Service::Acknowledged,
        "{context}: SwMI group attach refresh should use acknowledged service"
    );
    assert_eq!(refresh.groups, vec![(gssi, 0, class_of_usage)], "{context}: refreshed cached group");
}

fn extract_d_mm_status(msgs: &[SapMsg]) -> DMmStatus {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                DMmStatus::from_bitbuf(&mut sdu).ok()
            }
            _ => None,
        })
        .expect("expected D-MM-STATUS")
}

fn extract_mm_pdu_function_not_supported(msgs: &[SapMsg]) -> MmPduFunctionNotSupported {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                MmPduFunctionNotSupported::from_bitbuf(&mut sdu).ok()
            }
            _ => None,
        })
        .expect("expected MM PDU/FUNCTION NOT SUPPORTED")
}

fn extract_mm_pdu_function_not_supported_layer2service(msgs: &[SapMsg]) -> Layer2Service {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LmmMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                MmPduFunctionNotSupported::from_bitbuf(&mut sdu).ok().map(|_| prim.layer2service)
            }
            _ => None,
        })
        .expect("expected MM PDU/FUNCTION NOT SUPPORTED")
}

fn contains_attach_detach_ack(msgs: &[SapMsg]) -> bool {
    msgs.iter().any(|msg| match &msg.msg {
        SapMsgInner::LmmMleUnitdataReq(prim) => {
            let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
            DAttachDetachGroupIdentityAcknowledgement::from_bitbuf(&mut sdu).is_ok()
        }
        _ => false,
    })
}

fn subscriber_updates(msgs: &[SapMsg]) -> Vec<&MmSubscriberUpdate> {
    msgs.iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::MmSubscriberUpdate(update) => Some(update),
            _ => None,
        })
        .collect()
}

fn backdate_mm_registration(test: &mut ComponentTest, issi: u32, elapsed_secs: u64) {
    let mm = test
        .router
        .get_entity(TetraEntity::Mm)
        .expect("MM entity should be registered")
        .as_any_mut()
        .downcast_mut::<MmBs>()
        .expect("registered MM entity should be MmBs");
    assert!(
        mm.debug_backdate_registration_for_test(issi, std::time::Duration::from_secs(elapsed_secs)),
        "expected registered ISSI to backdate"
    );
}

fn expire_mm_registration_grace(test: &mut ComponentTest, issi: u32) {
    let mm = test
        .router
        .get_entity(TetraEntity::Mm)
        .expect("MM entity should be registered")
        .as_any_mut()
        .downcast_mut::<MmBs>()
        .expect("registered MM entity should be MmBs");
    assert!(
        mm.debug_expire_registration_grace_for_test(issi),
        "expected registered ISSI to have an expirable periodic-registration grace window"
    );
}

fn debug_mm_client_energy(test: &mut ComponentTest, issi: u32) -> Option<(EnergySavingMode, Option<u8>, Option<u8>)> {
    test.router
        .get_entity(TetraEntity::Mm)
        .expect("MM entity should be registered")
        .as_any_mut()
        .downcast_mut::<MmBs>()
        .expect("registered MM entity should be MmBs")
        .debug_client_energy_for_test(issi)
}

fn debug_mm_client_tei(test: &mut ComponentTest, issi: u32) -> Option<Option<u64>> {
    test.router
        .get_entity(TetraEntity::Mm)
        .expect("MM entity should be registered")
        .as_any_mut()
        .downcast_mut::<MmBs>()
        .expect("registered MM entity should be MmBs")
        .debug_client_tei_for_test(issi)
}

fn debug_mm_solicited_group_report_pending(test: &mut ComponentTest, issi: u32) -> bool {
    test.router
        .get_entity(TetraEntity::Mm)
        .expect("MM entity should be registered")
        .as_any_mut()
        .downcast_mut::<MmBs>()
        .expect("registered MM entity should be MmBs")
        .debug_solicited_group_report_pending_for_test(issi)
}

fn debug_mm_restart_recovery_cache_dirty(test: &mut ComponentTest) -> bool {
    test.router
        .get_entity(TetraEntity::Mm)
        .expect("MM entity should be registered")
        .as_any_mut()
        .downcast_mut::<MmBs>()
        .expect("registered MM entity should be MmBs")
        .debug_restart_recovery_cache_dirty_for_test()
}

fn debug_mm_restart_recovery_cache_len(test: &mut ComponentTest) -> usize {
    test.router
        .get_entity(TetraEntity::Mm)
        .expect("MM entity should be registered")
        .as_any_mut()
        .downcast_mut::<MmBs>()
        .expect("registered MM entity should be MmBs")
        .debug_restart_recovery_cache_len_for_test()
}

fn debug_mm_flush_restart_recovery_cache(test: &mut ComponentTest) {
    test.router
        .get_entity(TetraEntity::Mm)
        .expect("MM entity should be registered")
        .as_any_mut()
        .downcast_mut::<MmBs>()
        .expect("registered MM entity should be MmBs")
        .debug_flush_restart_recovery_cache_for_test()
}

fn debug_mm_swmi_group_transaction_pending(test: &mut ComponentTest, issi: u32) -> bool {
    test.router
        .get_entity(TetraEntity::Mm)
        .expect("MM entity should be registered")
        .as_any_mut()
        .downcast_mut::<MmBs>()
        .expect("registered MM entity should be MmBs")
        .debug_swmi_group_transaction_pending_for_test(issi)
}

fn begin_swmi_group_transaction_for_test(
    test: &mut ComponentTest,
    issi: u32,
    handle: u32,
    group_identity_downlink: Vec<GroupIdentityDownlink>,
    detach_all_then_attach: bool,
) {
    let mm = test
        .router
        .get_entity(TetraEntity::Mm)
        .expect("MM entity should be registered")
        .as_any_mut()
        .downcast_mut::<MmBs>()
        .expect("registered MM entity should be MmBs");
    assert!(
        mm.debug_begin_swmi_group_transaction_for_test(issi, handle, group_identity_downlink, detach_all_then_attach),
        "expected registered ISSI to accept pending SwMI group transaction"
    );
}

fn swmi_attach_group(gssi: u32) -> GroupIdentityDownlink {
    GroupIdentityDownlink {
        group_identity_attachment: Some(GroupIdentityAttachment {
            group_identity_attachment_lifetime: 0,
            class_of_usage: 4,
        }),
        group_identity_detachment_uplink: None,
        gssi: Some(gssi),
        address_extension: None,
        vgssi: None,
    }
}

fn swmi_attach_vgssi(vgssi: u32) -> GroupIdentityDownlink {
    GroupIdentityDownlink {
        group_identity_attachment: Some(GroupIdentityAttachment {
            group_identity_attachment_lifetime: 0,
            class_of_usage: 4,
        }),
        group_identity_detachment_uplink: None,
        gssi: None,
        address_extension: None,
        vgssi: Some(vgssi),
    }
}

fn swmi_detach_group(gssi: u32) -> GroupIdentityDownlink {
    GroupIdentityDownlink {
        group_identity_attachment: None,
        group_identity_detachment_uplink: Some(0),
        gssi: Some(gssi),
        address_extension: None,
        vgssi: None,
    }
}

fn swmi_ack_attach_entry(gssi: u32) -> GroupIdentityUplink {
    GroupIdentityUplink {
        class_of_usage: Some(0),
        group_identity_detachment_uplink: None,
        gssi: Some(gssi),
        address_extension: None,
        vgssi: None,
    }
}

fn swmi_ack_detach_entry(gssi: u32) -> GroupIdentityUplink {
    GroupIdentityUplink {
        class_of_usage: None,
        group_identity_detachment_uplink: Some(0),
        gssi: Some(gssi),
        address_extension: None,
        vgssi: None,
    }
}

fn swmi_ack_vgssi_detach_entry(vgssi: u32) -> GroupIdentityUplink {
    GroupIdentityUplink {
        class_of_usage: None,
        group_identity_detachment_uplink: Some(0),
        gssi: None,
        address_extension: None,
        vgssi: Some(vgssi),
    }
}

fn expected_location_update_accept_type(location_update_type: LocationUpdateType) -> LocationUpdateAcceptType {
    match location_update_type {
        LocationUpdateType::RoamingLocationUpdating => LocationUpdateAcceptType::RoamingLocationUpdating,
        LocationUpdateType::PeriodicLocationUpdating => LocationUpdateAcceptType::PeriodicLocationUpdating,
        LocationUpdateType::ItsiAttach => LocationUpdateAcceptType::ItsiAttach,
        LocationUpdateType::ServiceRestorationRoamingLocationUpdating => {
            LocationUpdateAcceptType::ServiceRestorationRoamingLocationUpdating
        }
        LocationUpdateType::DemandLocationUpdating => LocationUpdateAcceptType::DemandLocationUpdating,
        LocationUpdateType::MigratingLocationUpdating
        | LocationUpdateType::ServiceRestorationMigratingLocationUpdating
        | LocationUpdateType::DisabledMsUpdating => {
            panic!("unsupported location update type should not produce D-LOCATION UPDATE ACCEPT")
        }
    }
}

fn unique_restart_recovery_path(label: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after UNIX_EPOCH")
        .as_nanos();
    let mut path = std::env::temp_dir();
    path.push(format!("nexus-bs-mm-restart-recovery-{label}-{}-{nanos}.txt", std::process::id()));
    path.to_string_lossy().into_owned()
}

fn mm_test_with_telemetry(config: StackConfig) -> (ComponentTest, TelemetrySource) {
    let (sink, source) = telemetry_channel();
    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.register_entity(MmBs::new(test.config.clone(), Some(sink), None));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    (test, source)
}

fn drain_telemetry(source: &TelemetrySource) -> Vec<TelemetryEvent> {
    std::iter::from_fn(|| source.try_recv()).collect()
}

fn dashboard_groups_after(events: &[TelemetryEvent], issi: u32) -> Vec<u32> {
    let dashboard = DashboardServer::new("test.toml".to_string());
    for event in events {
        dashboard.handle_telemetry(event.clone());
    }
    dashboard
        .state
        .read()
        .unwrap()
        .snapshot_ms()
        .into_iter()
        .find(|ms| ms.issi == issi)
        .map(|ms| ms.groups)
        .unwrap_or_default()
}

fn submit_location_update(test: &mut ComponentTest, issi: u32, energy_saving_mode: Option<EnergySavingMode>) {
    submit_location_update_with_type(test, issi, LocationUpdateType::ItsiAttach, energy_saving_mode);
}

fn submit_location_update_with_type(
    test: &mut ComponentTest,
    issi: u32,
    location_update_type: LocationUpdateType,
    energy_saving_mode: Option<EnergySavingMode>,
) {
    submit_location_update_with_type_and_handle(test, issi, location_update_type, energy_saving_mode, 0);
}

fn submit_location_update_with_type_and_handle(
    test: &mut ComponentTest,
    issi: u32,
    location_update_type: LocationUpdateType,
    energy_saving_mode: Option<EnergySavingMode>,
    handle: u32,
) {
    let pdu = base_location_update_demand(location_update_type, energy_saving_mode);
    submit_location_update_demand_with_handle(test, issi, pdu, handle);
}

fn base_location_update_demand(
    location_update_type: LocationUpdateType,
    energy_saving_mode: Option<EnergySavingMode>,
) -> ULocationUpdateDemand {
    ULocationUpdateDemand {
        location_update_type,
        request_to_append_la: false,
        cipher_control: false,
        ciphering_parameters: None,
        class_of_ms: None,
        energy_saving_mode,
        la_information: None,
        ssi: None,
        address_extension: None,
        group_identity_location_demand: None,
        group_report_response: None,
        authentication_uplink: None,
        extended_capabilities: None,
        proprietary: None,
    }
}

fn type3_field(field_id: MmType34ElemIdUl, len: usize, data: u128) -> Type3FieldGeneric {
    Type3FieldGeneric {
        field_id: field_id.into_raw(),
        len,
        data,
    }
}

fn submit_location_update_demand_with_handle(test: &mut ComponentTest, issi: u32, pdu: ULocationUpdateDemand, handle: u32) {
    submit_location_update_demand_with_handle_and_received_address(test, pdu, handle, TetraAddress::issi(issi));
}

fn submit_location_update_demand_with_handle_and_received_address(
    test: &mut ComponentTest,
    pdu: ULocationUpdateDemand,
    handle: u32,
    received_address: TetraAddress,
) {
    let mut sdu = BitBuffer::new_autoexpand(32);
    pdu.to_bitbuf(&mut sdu).unwrap();
    sdu.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
            sdu,
            handle,
            received_address,
        }),
    });
}

fn submit_lmm_mle_report(test: &mut ComponentTest, handle: u32, transfer_result: i32) {
    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleReportInd(LmmMleReportInd { handle, transfer_result }),
    });
}

fn submit_u_itsi_detach(test: &mut ComponentTest, issi: u32) {
    let pdu = UItsiDetach {
        address_extension: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(8);
    pdu.to_bitbuf(&mut sdu).unwrap();
    sdu.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
            sdu,
            handle: 0,
            received_address: TetraAddress::issi(issi),
        }),
    });
}

fn submit_u_tei_provide(test: &mut ComponentTest, issi: u32, tei: u64) {
    let pdu = UTeiProvide { tei, proprietary: None };
    let mut sdu = BitBuffer::new_autoexpand(16);
    pdu.to_bitbuf(&mut sdu).unwrap();
    sdu.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
            sdu,
            handle: 0,
            received_address: TetraAddress::issi(issi),
        }),
    });
}

fn submit_location_update_with_groups(test: &mut ComponentTest, issi: u32, location_update_type: LocationUpdateType, groups: Vec<u32>) {
    submit_location_update_with_groups_and_energy(test, issi, location_update_type, groups, None);
}

fn submit_location_update_with_groups_and_energy(
    test: &mut ComponentTest,
    issi: u32,
    location_update_type: LocationUpdateType,
    groups: Vec<u32>,
    energy_saving_mode: Option<EnergySavingMode>,
) {
    let group_identity_uplink = groups
        .into_iter()
        .map(|gssi| GroupIdentityUplink {
            class_of_usage: Some(0),
            group_identity_detachment_uplink: None,
            gssi: Some(gssi),
            address_extension: None,
            vgssi: None,
        })
        .collect();
    submit_location_update_with_group_identity_uplink_and_energy(
        test,
        issi,
        location_update_type,
        group_identity_uplink,
        energy_saving_mode,
    );
}

fn submit_location_update_with_groups_and_group_report_response(
    test: &mut ComponentTest,
    issi: u32,
    location_update_type: LocationUpdateType,
    groups: Vec<u32>,
    len: usize,
    data: u128,
) {
    let group_identity_uplink = groups
        .into_iter()
        .map(|gssi| GroupIdentityUplink {
            class_of_usage: Some(0),
            group_identity_detachment_uplink: None,
            gssi: Some(gssi),
            address_extension: None,
            vgssi: None,
        })
        .collect();
    let mut pdu = base_location_update_demand(location_update_type, None);
    pdu.group_identity_location_demand = Some(GroupIdentityLocationDemand {
        group_identity_attach_detach_mode: 1,
        group_identity_uplink: Some(group_identity_uplink),
    });
    pdu.group_report_response = Some(Type3FieldGeneric { field_id: 0, len, data });
    submit_location_update_demand_with_handle(test, issi, pdu, 0);
}

fn submit_location_update_with_group_identity_uplink(
    test: &mut ComponentTest,
    issi: u32,
    location_update_type: LocationUpdateType,
    group_identity_uplink: Vec<GroupIdentityUplink>,
) {
    submit_location_update_with_group_identity_uplink_and_energy(test, issi, location_update_type, group_identity_uplink, None);
}

fn submit_location_update_with_group_identity_uplink_and_energy(
    test: &mut ComponentTest,
    issi: u32,
    location_update_type: LocationUpdateType,
    group_identity_uplink: Vec<GroupIdentityUplink>,
    energy_saving_mode: Option<EnergySavingMode>,
) {
    let pdu = ULocationUpdateDemand {
        location_update_type,
        request_to_append_la: false,
        cipher_control: false,
        ciphering_parameters: None,
        class_of_ms: None,
        energy_saving_mode,
        la_information: None,
        ssi: None,
        address_extension: None,
        group_identity_location_demand: Some(GroupIdentityLocationDemand {
            group_identity_attach_detach_mode: 1,
            group_identity_uplink: Some(group_identity_uplink),
        }),
        group_report_response: None,
        authentication_uplink: None,
        extended_capabilities: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(256);
    pdu.to_bitbuf(&mut sdu).unwrap();
    sdu.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
            sdu,
            handle: 0,
            received_address: TetraAddress::issi(issi),
        }),
    });
}

fn submit_location_update_with_group_report_response(
    test: &mut ComponentTest,
    issi: u32,
    location_update_type: LocationUpdateType,
    len: usize,
    data: u128,
) {
    submit_location_update_with_group_report_response_and_energy(test, issi, location_update_type, len, data, None);
}

fn submit_location_update_with_group_report_response_and_energy(
    test: &mut ComponentTest,
    issi: u32,
    location_update_type: LocationUpdateType,
    len: usize,
    data: u128,
    energy_saving_mode: Option<EnergySavingMode>,
) {
    let mut pdu = base_location_update_demand(location_update_type, energy_saving_mode);
    pdu.group_report_response = Some(Type3FieldGeneric { field_id: 0, len, data });
    submit_location_update_demand_with_handle(test, issi, pdu, 0);
}

fn submit_u_mm_status_energy_saving(test: &mut ComponentTest, issi: u32, status: StatusUplink, mode: EnergySavingMode) {
    submit_u_mm_status(test, issi, status, Some(mode as u64), Some(3));
}

fn submit_u_mm_status_energy_saving_with_received_address(
    test: &mut ComponentTest,
    status: StatusUplink,
    mode: EnergySavingMode,
    received_address: TetraAddress,
) {
    submit_u_mm_status_with_received_address(test, status, Some(mode as u64), Some(3), received_address);
}

fn submit_u_mm_status(
    test: &mut ComponentTest,
    issi: u32,
    status: StatusUplink,
    dependent_information: Option<u64>,
    dependent_information_len: Option<usize>,
) {
    submit_u_mm_status_with_received_address(
        test,
        status,
        dependent_information,
        dependent_information_len,
        TetraAddress::issi(issi),
    );
}

fn submit_u_mm_status_with_received_address(
    test: &mut ComponentTest,
    status: StatusUplink,
    dependent_information: Option<u64>,
    dependent_information_len: Option<usize>,
    received_address: TetraAddress,
) {
    let pdu = UMmStatus {
        status_uplink: status,
        status_uplink_dependent_information: dependent_information,
        status_uplink_dependent_information_len: dependent_information_len,
    };
    let mut sdu = BitBuffer::new_autoexpand(16);
    pdu.to_bitbuf(&mut sdu).unwrap();
    sdu.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
            sdu,
            handle: 0,
            received_address,
        }),
    });
}

fn submit_raw_mm_pdu_type(test: &mut ComponentTest, issi: u32, pdu_type: MmPduTypeUl) {
    submit_raw_mm_pdu_type_with_received_address(test, pdu_type, TetraAddress::issi(issi));
}

fn submit_raw_mm_pdu_type_with_received_address(test: &mut ComponentTest, pdu_type: MmPduTypeUl, received_address: TetraAddress) {
    let mut sdu = BitBuffer::new_autoexpand(4);
    sdu.write_bits(pdu_type.into_raw(), 4);
    sdu.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
            sdu,
            handle: 0,
            received_address,
        }),
    });
}

fn submit_attach_detach_group_identity(
    test: &mut ComponentTest,
    issi: u32,
    group_identity_attach_detach_mode: bool,
    groups: Option<Vec<u32>>,
) {
    submit_attach_detach_group_identity_with_received_address(test, group_identity_attach_detach_mode, groups, TetraAddress::issi(issi));
}

fn submit_attach_detach_group_identity_with_received_address(
    test: &mut ComponentTest,
    group_identity_attach_detach_mode: bool,
    groups: Option<Vec<u32>>,
    received_address: TetraAddress,
) {
    let group_identity_uplink = groups.map(|groups| {
        groups
            .into_iter()
            .map(|gssi| GroupIdentityUplink {
                class_of_usage: Some(0),
                group_identity_detachment_uplink: None,
                gssi: Some(gssi),
                address_extension: None,
                vgssi: None,
            })
            .collect()
    });
    submit_attach_detach_group_identity_uplink_with_received_address(
        test,
        group_identity_attach_detach_mode,
        group_identity_uplink,
        received_address,
    );
}

fn submit_attach_detach_group_identity_uplink(
    test: &mut ComponentTest,
    issi: u32,
    group_identity_attach_detach_mode: bool,
    group_identity_uplink: Option<Vec<GroupIdentityUplink>>,
) {
    submit_attach_detach_group_identity_uplink_with_response(test, issi, group_identity_attach_detach_mode, group_identity_uplink, None);
}

fn submit_attach_detach_group_identity_with_report_response(
    test: &mut ComponentTest,
    issi: u32,
    group_identity_attach_detach_mode: bool,
    group_identity_uplink: Vec<GroupIdentityUplink>,
    len: usize,
    data: u128,
) {
    submit_attach_detach_group_identity_uplink_with_response(
        test,
        issi,
        group_identity_attach_detach_mode,
        Some(group_identity_uplink),
        Some(Type3FieldGeneric { field_id: 0, len, data }),
    );
}

fn submit_attach_detach_group_identity_uplink_with_response(
    test: &mut ComponentTest,
    issi: u32,
    group_identity_attach_detach_mode: bool,
    group_identity_uplink: Option<Vec<GroupIdentityUplink>>,
    group_report_response: Option<Type3FieldGeneric>,
) {
    submit_attach_detach_group_identity_uplink_with_response_and_received_address(
        test,
        group_identity_attach_detach_mode,
        group_identity_uplink,
        group_report_response,
        TetraAddress::issi(issi),
    );
}

fn submit_attach_detach_group_identity_uplink_with_received_address(
    test: &mut ComponentTest,
    group_identity_attach_detach_mode: bool,
    group_identity_uplink: Option<Vec<GroupIdentityUplink>>,
    received_address: TetraAddress,
) {
    submit_attach_detach_group_identity_uplink_with_response_and_received_address(
        test,
        group_identity_attach_detach_mode,
        group_identity_uplink,
        None,
        received_address,
    );
}

fn submit_attach_detach_group_identity_uplink_with_response_and_received_address(
    test: &mut ComponentTest,
    group_identity_attach_detach_mode: bool,
    group_identity_uplink: Option<Vec<GroupIdentityUplink>>,
    group_report_response: Option<Type3FieldGeneric>,
    received_address: TetraAddress,
) {
    let pdu = UAttachDetachGroupIdentity {
        group_identity_report: false,
        group_identity_attach_detach_mode,
        group_report_response,
        group_identity_uplink,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(128);
    pdu.to_bitbuf(&mut sdu).unwrap();
    sdu.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
            sdu,
            handle: 0,
            received_address,
        }),
    });
}

fn invalid_mm_source_addresses(issi: u32) -> [TetraAddress; 3] {
    [
        TetraAddress::new(issi, SsiType::Gssi),
        TetraAddress::new(issi, SsiType::Unknown),
        TetraAddress::new(0x0100_0000, SsiType::Issi),
    ]
}

fn submit_swmi_group_ack(test: &mut ComponentTest, issi: u32, handle: u32, rejected: bool, rejected_groups: Vec<u32>) {
    let group_identity_uplink = (!rejected_groups.is_empty()).then(|| rejected_groups.into_iter().map(swmi_ack_detach_entry).collect());
    submit_swmi_group_ack_uplink(test, issi, handle, rejected, group_identity_uplink);
}

fn submit_swmi_group_ack_with_received_address(
    test: &mut ComponentTest,
    issi: u32,
    handle: u32,
    rejected: bool,
    rejected_groups: Vec<u32>,
    received_address: TetraAddress,
) {
    let group_identity_uplink = (!rejected_groups.is_empty()).then(|| rejected_groups.into_iter().map(swmi_ack_detach_entry).collect());
    submit_swmi_group_ack_uplink_with_received_address(test, issi, handle, rejected, group_identity_uplink, received_address);
}

fn submit_swmi_group_ack_uplink(
    test: &mut ComponentTest,
    issi: u32,
    handle: u32,
    rejected: bool,
    group_identity_uplink: Option<Vec<GroupIdentityUplink>>,
) {
    submit_swmi_group_ack_uplink_with_received_address(test, issi, handle, rejected, group_identity_uplink, TetraAddress::issi(issi));
}

fn submit_swmi_group_ack_uplink_with_received_address(
    test: &mut ComponentTest,
    _issi: u32,
    handle: u32,
    rejected: bool,
    group_identity_uplink: Option<Vec<GroupIdentityUplink>>,
    received_address: TetraAddress,
) {
    let pdu = UAttachDetachGroupIdentityAcknowledgement {
        group_identity_acknowledgement_type: rejected,
        group_identity_uplink,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(128);
    pdu.to_bitbuf(&mut sdu).unwrap();
    sdu.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
            sdu,
            handle,
            received_address,
        }),
    });
}

fn submit_attach_detach_group_report_request(test: &mut ComponentTest, issi: u32) {
    let pdu = UAttachDetachGroupIdentity {
        group_identity_report: true,
        group_identity_attach_detach_mode: false,
        group_report_response: None,
        group_identity_uplink: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(32);
    pdu.to_bitbuf(&mut sdu).unwrap();
    sdu.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
            sdu,
            handle: 0,
            received_address: TetraAddress::issi(issi),
        }),
    });
}

fn submit_malformed_attach_detach_group_report_request(
    test: &mut ComponentTest,
    issi: u32,
    group_identity_attach_detach_mode: bool,
    group_identity_uplink: Option<Vec<GroupIdentityUplink>>,
    group_report_response: Option<Type3FieldGeneric>,
) {
    let pdu = UAttachDetachGroupIdentity {
        group_identity_report: true,
        group_identity_attach_detach_mode,
        group_report_response,
        group_identity_uplink,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(128);
    pdu.to_bitbuf(&mut sdu).unwrap();
    sdu.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
            sdu,
            handle: 0,
            received_address: TetraAddress::issi(issi),
        }),
    });
}

fn submit_attach_detach_group_report_response(test: &mut ComponentTest, issi: u32, len: usize, data: u128) {
    let pdu = UAttachDetachGroupIdentity {
        group_identity_report: false,
        group_identity_attach_detach_mode: false,
        group_report_response: Some(Type3FieldGeneric { field_id: 0, len, data }),
        group_identity_uplink: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(32);
    pdu.to_bitbuf(&mut sdu).unwrap();
    sdu.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
            sdu,
            handle: 0,
            received_address: TetraAddress::issi(issi),
        }),
    });
}
