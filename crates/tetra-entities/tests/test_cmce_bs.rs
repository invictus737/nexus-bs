mod common;

use tetra_config::bluestation::{CfgBrew, ENERGY_SAVING_MODE_AUTO, SharedConfig, StackMode, from_toml_str};
use tetra_core::ranges::SortedDisjointSsiRanges;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::typed_pdu_fields::Type3FieldGeneric;
use tetra_core::{BitBuffer, Direction, Layer2Service, PhyBlockNum, Sap, SsiType, TdmaTime, TetraAddress, TimeslotOwner, TxState, debug};
use tetra_entities::cmce::cmce_bs::CmceBs;
use tetra_entities::net_control::{ControlCommand, make_control_link};
use tetra_entities::net_dashboard::DashboardServer;
use tetra_entities::net_telemetry::{TelemetryEvent, TelemetrySource, telemetry_channel};
use tetra_entities::{MessageQueue, TetraEntityTrait};
use tetra_pdus::cmce::enums::call_timeout::CallTimeout;
use tetra_pdus::cmce::enums::cmce_pdu_type_dl::CmcePduTypeDl;
use tetra_pdus::cmce::enums::cmce_pdu_type_ul::CmcePduTypeUl;
use tetra_pdus::cmce::enums::disconnect_cause::DisconnectCause;
use tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier;
use tetra_pdus::cmce::enums::transmission_grant::TransmissionGrant;
use tetra_pdus::cmce::fields::basic_service_information::BasicServiceInformation;
use tetra_pdus::cmce::pdus::cmce_function_not_supported::CmceFunctionNotSupported;
use tetra_pdus::cmce::pdus::d_alert::DAlert;
use tetra_pdus::cmce::pdus::d_call_proceeding::DCallProceeding;
use tetra_pdus::cmce::pdus::d_connect::DConnect;
use tetra_pdus::cmce::pdus::d_connect_acknowledge::DConnectAcknowledge;
use tetra_pdus::cmce::pdus::d_disconnect::DDisconnect;
use tetra_pdus::cmce::pdus::d_info::DInfo;
use tetra_pdus::cmce::pdus::d_release::DRelease;
use tetra_pdus::cmce::pdus::d_setup::DSetup;
use tetra_pdus::cmce::pdus::d_tx_ceased::DTxCeased;
use tetra_pdus::cmce::pdus::d_tx_granted::DTxGranted;
use tetra_pdus::cmce::pdus::d_tx_interrupt::DTxInterrupt;
use tetra_pdus::cmce::pdus::u_alert::UAlert;
use tetra_pdus::cmce::pdus::u_call_restore::UCallRestore;
use tetra_pdus::cmce::pdus::u_connect::UConnect;
use tetra_pdus::cmce::pdus::u_disconnect::UDisconnect;
use tetra_pdus::cmce::pdus::u_facility::UFacility;
use tetra_pdus::cmce::pdus::u_info::UInfo;
use tetra_pdus::cmce::pdus::u_release::URelease;
use tetra_pdus::cmce::pdus::u_setup::USetup;
use tetra_pdus::cmce::pdus::u_tx_ceased::UTxCeased;
use tetra_pdus::cmce::pdus::u_tx_demand::UTxDemand;
use tetra_pdus::llc::pdus::bl_udata::BlUdata;
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;
use tetra_pdus::mm::enums::energy_saving_mode::EnergySavingMode;
use tetra_pdus::mm::enums::location_update_type::LocationUpdateType;
use tetra_pdus::mm::fields::class_of_ms::ClassOfMs;
use tetra_pdus::mm::fields::group_identity_location_demand::GroupIdentityLocationDemand;
use tetra_pdus::mm::fields::group_identity_uplink::GroupIdentityUplink;
use tetra_pdus::mm::pdus::d_location_update_accept::DLocationUpdateAccept;
use tetra_pdus::mm::pdus::u_attach_detach_group_identity_acknowledgement::UAttachDetachGroupIdentityAcknowledgement;
use tetra_pdus::mm::pdus::u_location_update_demand::ULocationUpdateDemand;
use tetra_pdus::umac::pdus::mac_resource::MacResource;
use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
use tetra_saps::control::call_control::{CallControl, CircuitDlMediaSource, NetworkCircuitCall};
use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
use tetra_saps::control::enums::communication_type::CommunicationType;
use tetra_saps::lcmc::enums::alloc_type::ChanAllocType;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::lcmc::{LcmcMleUnitdataInd, LcmcMleUnitdataReq};
use tetra_saps::lmm::LmmMleUnitdataInd;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};
use tetra_saps::tmv::{TmvUnitdataReq, enums::logical_chans::LogicalChannel};

use crate::common::ComponentTest;

const TEST_GSSI: u32 = 91;
const TEST_ISSI: u32 = 1000001;
const TEST_CALLED_GSSI: u32 = 92;
const TEST_CALLED_ISSI: u32 = 1000002;
const TEST_OTHER_ISSI: u32 = 1000003;
const LAB_GROUP_GSSI: u32 = 226333;
const LAB_ISSI_A: u32 = 2260616;
const LAB_ISSI_B: u32 = 2260082;
const LAB_ISSI_MXP600: u32 = 2260618;
const LARGE_GSSI_MEMBER_COUNT: u32 = 4096;
const TETRA_TIMESLOTS_PER_SECOND: i32 = 18 * 4;
const PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS: i32 = (4 - 1) * 4;
const GROUP_TX_CEASED_TAIL_DRAIN_TIMESLOTS: i32 = (4 - 1) * 4;
const PRIVATE_RELEASE_DELIVERY_GUARD_TIMESLOTS: i32 = 2 * TETRA_TIMESLOTS_PER_SECOND;
const PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS: i32 = 16;
const PRIVATE_SIMPLEX_CONNECT_ACK_UNACKED_REPETITIONS: u8 = 3;

#[test]
fn test_cmce_forwards_rf_carrier_inhibit_to_mm() {
    debug::setup_logging_verbose();
    let (dispatcher, endpoint) = make_control_link();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.register_entity(CmceBs::new(test.config.clone(), None, Some(endpoint)));
    test.populate_entities(vec![], vec![TetraEntity::Mm]);

    dispatcher.send(ControlCommand::SetRfCarrierInhibit { inhibited: true });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    assert!(
        msgs.iter()
            .any(|msg| { msg.dest == TetraEntity::Mm && matches!(&msg.msg, SapMsgInner::RfCarrierInhibit { inhibited } if *inhibited) }),
        "CMCE must forward legacy RF carrier commands to MM for registry cleanup"
    );
    assert!(
        !test.config.state_read().carrier_inhibited,
        "CMCE must not hard-inhibit RF directly before MM can notify registered MS"
    );
}

fn unique_restart_recovery_path(label: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after UNIX_EPOCH")
        .as_nanos();
    let mut path = std::env::temp_dir();
    path.push(format!("nexus-bs-cmce-restart-recovery-{label}-{}-{nanos}.txt", std::process::id()));
    path.to_string_lossy().into_owned()
}

fn type3_marker() -> Type3FieldGeneric {
    Type3FieldGeneric {
        field_id: 0,
        len: 8,
        data: 0xA5,
    }
}

fn frequency_simplex_voice_class_of_ms() -> ClassOfMs {
    ClassOfMs {
        freq_simplex_duplex: false,
        multislot_phase_mod: true,
        concurrent_multicarrier: false,
        voice: true,
        e2e_encryption_not_supported: true,
        circuit_mode_data: false,
        tetra_packet_data: true,
        fast_switching: false,
        dck_encryption: false,
        clch_needed: false,
        concurrent_circuit_mode: false,
        original_advanced_link: true,
        minimum_mode: false,
        carrier_specific_signalling: false,
        authentication: false,
        sck_encryption: false,
        air_interface_version: 3,
        common_scch: true,
        reserved_21: false,
        mac_d_blck: false,
        extended_advanced_link: false,
        d8psk: false,
    }
}

/// Helper: register a subscriber on a GSSI so CMCE accepts calls for that group.
fn register_subscriber(test: &mut ComponentTest, issi: u32, gssi: u32) {
    let register = SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi,
            groups: vec![],
            action: BrewSubscriberAction::Register,
        }),
    };
    test.submit_message(register);
    test.run_stack(Some(1));

    let affiliate = SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi,
            groups: vec![gssi],
            action: BrewSubscriberAction::Affiliate,
        }),
    };
    test.submit_message(affiliate);
    test.run_stack(Some(1));
    test.dump_sinks();
}

fn submit_subscriber_update(test: &mut ComponentTest, issi: u32, groups: Vec<u32>, action: BrewSubscriberAction) {
    submit_subscriber_update_from(test, TetraEntity::Mm, issi, groups, action);
}

fn submit_subscriber_update_from(test: &mut ComponentTest, src: TetraEntity, issi: u32, groups: Vec<u32>, action: BrewSubscriberAction) {
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate { issi, groups, action }),
    });
}

fn submit_subscriber_update_to_cmce(
    cmce: &mut CmceBs,
    queue: &mut MessageQueue,
    issi: u32,
    groups: Vec<u32>,
    action: BrewSubscriberAction,
) {
    cmce.rx_prim(
        queue,
        SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Mm,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate { issi, groups, action }),
        },
    );
}

fn register_subscriber_to_cmce(cmce: &mut CmceBs, queue: &mut MessageQueue, issi: u32, gssi: u32) {
    submit_subscriber_update_to_cmce(cmce, queue, issi, vec![], BrewSubscriberAction::Register);
    drain_message_queue(queue);
    submit_subscriber_update_to_cmce(cmce, queue, issi, vec![gssi], BrewSubscriberAction::Affiliate);
    drain_message_queue(queue);
}

fn drain_message_queue(queue: &mut MessageQueue) -> Vec<SapMsg> {
    let mut msgs = Vec::new();
    while let Some(msg) = queue.pop_front() {
        msgs.push(msg);
    }
    msgs
}

fn drain_telemetry(source: &TelemetrySource) -> Vec<TelemetryEvent> {
    std::iter::from_fn(|| source.try_recv()).collect()
}

fn cmce_bs_mut(test: &mut ComponentTest) -> &mut CmceBs {
    test.router
        .get_entity(TetraEntity::Cmce)
        .expect("CMCE entity should be registered")
        .as_any_mut()
        .downcast_mut::<CmceBs>()
        .expect("registered CMCE entity should be CmceBs")
}

fn force_cmce_next_call_identifier(test: &mut ComponentTest, next_call_identifier: u16) {
    cmce_bs_mut(test).debug_force_next_call_identifier(next_call_identifier);
}

fn cmce_debug_active_call_ids(test: &mut ComponentTest) -> Vec<u16> {
    cmce_bs_mut(test).debug_active_call_ids()
}

fn cmce_debug_subscriber_groups_for(test: &mut ComponentTest, issi: u32) -> Vec<u32> {
    cmce_bs_mut(test).debug_subscriber_groups_for(issi)
}

fn drain_private_simplex_tail(test: &mut ComponentTest, dltime: TdmaTime) {
    test.router
        .set_dl_time(dltime.add_timeslots(PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
}

fn drain_group_tx_ceased_tail(test: &mut ComponentTest, dltime: TdmaTime) {
    test.router
        .set_dl_time(dltime.add_timeslots(GROUP_TX_CEASED_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
}

fn drain_group_tx_ceased_tail_after_large_stress(test: &mut ComponentTest, dltime: TdmaTime) {
    test.router.set_dl_time(dltime.add_timeslots(1_000_000));
    test.run_stack(Some(1));
}

fn run_group_late_entry_resend_tick(test: &mut ComponentTest, dltime: TdmaTime) {
    let base = dltime.add_timeslots(5 * TETRA_TIMESLOTS_PER_SECOND);
    for frame_offset in 0..=18 {
        test.router.set_dl_time(base.add_timeslots(frame_offset * 4));
        test.run_stack(Some(1));
    }
}

/// Helper: submit a real MM U-LOCATION UPDATE DEMAND carrying group affiliation.
fn submit_location_update_with_group_identity_location_demand(test: &mut ComponentTest, issi: u32, gssi: u32) {
    submit_location_update_with_type_and_group_identity_location_demand(test, issi, gssi, LocationUpdateType::ItsiAttach);
}

fn submit_location_update_without_group_identity_location_demand(
    test: &mut ComponentTest,
    issi: u32,
    location_update_type: LocationUpdateType,
) {
    let pdu = ULocationUpdateDemand {
        location_update_type,
        request_to_append_la: false,
        cipher_control: false,
        ciphering_parameters: None,
        class_of_ms: None,
        energy_saving_mode: None,
        la_information: None,
        ssi: None,
        address_extension: None,
        group_identity_location_demand: None,
        group_report_response: None,
        authentication_uplink: None,
        extended_capabilities: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(64);
    pdu.to_bitbuf(&mut sdu)
        .expect("Failed to serialize group-less ULocationUpdateDemand");
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

fn submit_swmi_group_refresh_ack(test: &mut ComponentTest, issi: u32, handle: u32) {
    let pdu = UAttachDetachGroupIdentityAcknowledgement {
        group_identity_acknowledgement_type: false,
        group_identity_uplink: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(64);
    pdu.to_bitbuf(&mut sdu)
        .expect("Failed to serialize UAttachDetachGroupIdentityAcknowledgement");
    sdu.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
            sdu,
            handle,
            received_address: TetraAddress::issi(issi),
        }),
    });
}

fn submit_location_update_with_type_and_group_identity_location_demand(
    test: &mut ComponentTest,
    issi: u32,
    gssi: u32,
    location_update_type: LocationUpdateType,
) {
    let pdu = ULocationUpdateDemand {
        location_update_type,
        request_to_append_la: false,
        cipher_control: false,
        ciphering_parameters: None,
        class_of_ms: None,
        energy_saving_mode: None,
        la_information: None,
        ssi: None,
        address_extension: None,
        group_identity_location_demand: Some(GroupIdentityLocationDemand {
            group_identity_attach_detach_mode: 1,
            group_identity_uplink: Some(vec![GroupIdentityUplink {
                class_of_usage: Some(0),
                group_identity_detachment_uplink: None,
                gssi: Some(gssi),
                address_extension: None,
                vgssi: None,
            }]),
        }),
        group_report_response: None,
        authentication_uplink: None,
        extended_capabilities: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(256);
    pdu.to_bitbuf(&mut sdu).expect("Failed to serialize ULocationUpdateDemand");
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

fn submit_location_update_with_group_and_class_of_ms(test: &mut ComponentTest, issi: u32, gssi: u32, class_of_ms: ClassOfMs) {
    let pdu = ULocationUpdateDemand {
        location_update_type: LocationUpdateType::ItsiAttach,
        request_to_append_la: false,
        cipher_control: false,
        ciphering_parameters: None,
        class_of_ms: Some(class_of_ms),
        energy_saving_mode: None,
        la_information: None,
        ssi: None,
        address_extension: None,
        group_identity_location_demand: Some(GroupIdentityLocationDemand {
            group_identity_attach_detach_mode: 1,
            group_identity_uplink: Some(vec![GroupIdentityUplink {
                class_of_usage: Some(0),
                group_identity_detachment_uplink: None,
                gssi: Some(gssi),
                address_extension: None,
                vgssi: None,
            }]),
        }),
        group_report_response: None,
        authentication_uplink: None,
        extended_capabilities: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(64);
    pdu.to_bitbuf(&mut sdu)
        .expect("Failed to serialize class-carrying ULocationUpdateDemand");
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

fn build_mm_deregister_msg(issi: u32) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi,
            groups: vec![],
            action: BrewSubscriberAction::Deregister,
        }),
    }
}

fn build_mm_release_individual_calls_msg(issi: u32) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi,
            groups: vec![],
            action: BrewSubscriberAction::ReleaseIndividualCalls,
        }),
    }
}

fn build_mm_deaffiliate_msg(issi: u32, gssi: u32) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi,
            groups: vec![gssi],
            action: BrewSubscriberAction::Deaffiliate,
        }),
    }
}

fn test_brew_config() -> CfgBrew {
    CfgBrew {
        host: "test-brew.local".to_string(),
        port: 443,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: std::time::Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        feature_rssi_export: false,
        whitelisted_ssis: None,
    }
}

fn default_network_circuit_call(source_issi: u32, destination: u32) -> NetworkCircuitCall {
    NetworkCircuitCall {
        source_issi,
        destination,
        number: destination.to_string(),
        priority: 0,
        service: 0,
        mode: CircuitModeType::TchS.into_raw() as u8,
        duplex: 0,
        method: 0,
        communication: CommunicationType::P2p.into_raw() as u8,
        grant: TransmissionGrant::Granted.into_raw() as u8,
        permission: 0,
        timeout: CallTimeout::T5m.into_raw() as u8,
        ownership: 1,
        queued: 0,
    }
}

fn tsi_extension(mcc: u16, mnc: u16) -> u64 {
    ((u64::from(mcc) & 0x03ff) << 14) | (u64::from(mnc) & 0x3fff)
}

fn default_group_u_setup(dest_gssi: u32) -> USetup {
    USetup {
        area_selection: 0,
        hook_method_selection: false,
        simplex_duplex_selection: false,
        basic_service_information: BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2Mp,
            slots_per_frame: None,
            speech_service: Some(0),
        },
        request_to_transmit_send_data: false,
        call_priority: 0,
        clir_control: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_ssi: Some(dest_gssi as u64),
        called_party_short_number_address: None,
        called_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    }
}

/// Helper: build a U-SETUP SAP message for a group call.
fn build_u_setup_msg(calling_issi: u32, dest_gssi: u32) -> SapMsg {
    build_u_setup_custom_msg(calling_issi, default_group_u_setup(dest_gssi))
}

fn serialize_u_setup_for_test(u_setup: &USetup, sdu: &mut BitBuffer, context: &str) {
    if u_setup.called_party_type_identifier == PartyTypeIdentifier::Reserved {
        sdu.write_bits(CmcePduTypeUl::USetup.into_raw(), 5);
        sdu.write_bits(u_setup.area_selection as u64, 4);
        sdu.write_bits(u_setup.hook_method_selection as u64, 1);
        sdu.write_bits(u_setup.simplex_duplex_selection as u64, 1);
        u_setup
            .basic_service_information
            .to_bitbuf(sdu)
            .expect("test U-SETUP basic service information must serialize");
        sdu.write_bits(u_setup.request_to_transmit_send_data as u64, 1);
        sdu.write_bits(u_setup.call_priority as u64, 4);
        sdu.write_bits(u_setup.clir_control as u64, 2);
        sdu.write_bits(PartyTypeIdentifier::Reserved.into_raw(), 2);
        sdu.write_bits(0, 1);
    } else {
        u_setup.to_bitbuf(sdu).expect(context);
    }
}

fn build_u_setup_custom_msg(calling_issi: u32, u_setup: USetup) -> SapMsg {
    let mut sdu = BitBuffer::new_autoexpand(80);
    serialize_u_setup_for_test(&u_setup, &mut sdu, "Failed to serialize USetup");
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
            received_tetra_address: TetraAddress::new(calling_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

/// Helper: build a U-SETUP SAP message for a local individual call.
fn build_u_setup_p2p_msg(calling_issi: u32, called_issi: u32) -> SapMsg {
    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(called_issi as u64);
    build_u_setup_p2p_custom_msg(calling_issi, u_setup)
}

fn build_u_setup_p2p_custom_msg(calling_issi: u32, u_setup: USetup) -> SapMsg {
    let mut sdu = BitBuffer::new_autoexpand(80);
    serialize_u_setup_for_test(&u_setup, &mut sdu, "Failed to serialize USetup P2P");
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
            received_tetra_address: TetraAddress::new(calling_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_u_call_restore_msg(calling_issi: u32, call_id: u16, other_party_issi: u32) -> SapMsg {
    let pdu = UCallRestore {
        call_identifier: call_id,
        request_to_transmit_send_data: false,
        other_party_type_identifier: 1,
        other_party_short_number_address: None,
        other_party_ssi: Some(other_party_issi as u64),
        other_party_extension: None,
        basic_service_information: Some(BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2p,
            slots_per_frame: None,
            speech_service: Some(0),
        }),
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(80);
    pdu.to_bitbuf(&mut sdu).expect("Failed to serialize UCallRestore");
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
            received_tetra_address: TetraAddress::new(calling_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_u_facility_msg(calling_issi: u32) -> SapMsg {
    let pdu = UFacility {};
    let mut sdu = BitBuffer::new_autoexpand(16);
    pdu.to_bitbuf(&mut sdu).expect("Failed to serialize UFacility");
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
            received_tetra_address: TetraAddress::new(calling_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn default_p2p_u_setup() -> USetup {
    USetup {
        area_selection: 0,
        hook_method_selection: false,
        simplex_duplex_selection: false,
        basic_service_information: BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2p,
            slots_per_frame: None,
            speech_service: Some(0),
        },
        request_to_transmit_send_data: false,
        call_priority: 0,
        clir_control: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_ssi: Some(TEST_CALLED_ISSI as u64),
        called_party_short_number_address: None,
        called_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    }
}

fn build_u_connect_msg(called_issi: u32, call_id: u16) -> SapMsg {
    build_u_connect_custom_msg(called_issi, call_id, false)
}

fn build_u_connect_custom_msg(called_issi: u32, call_id: u16, simplex_duplex_selection: bool) -> SapMsg {
    build_u_connect_custom_msg_with_hook(called_issi, call_id, simplex_duplex_selection, simplex_duplex_selection)
}

fn build_u_connect_custom_msg_with_hook(
    called_issi: u32,
    call_id: u16,
    hook_method_selection: bool,
    simplex_duplex_selection: bool,
) -> SapMsg {
    build_u_connect_pdu_msg(
        called_issi,
        UConnect {
            call_identifier: call_id,
            hook_method_selection,
            simplex_duplex_selection,
            basic_service_information: None,
            facility: None,
            proprietary: None,
        },
    )
}

fn build_u_connect_pdu_msg(called_issi: u32, u_connect: UConnect) -> SapMsg {
    let mut sdu = BitBuffer::new_autoexpand(64);
    u_connect.to_bitbuf(&mut sdu).expect("Failed to serialize UConnect");
    sdu.seek(0);

    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 2,
            endpoint_id: 2,
            link_id: 2,
            received_tetra_address: TetraAddress::new(called_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_u_connect_with_unsupported_feature_msg(called_issi: u32, call_id: u16, unsupported: &str) -> SapMsg {
    let mut u_connect = UConnect {
        call_identifier: call_id,
        hook_method_selection: false,
        simplex_duplex_selection: false,
        basic_service_information: None,
        facility: None,
        proprietary: None,
    };

    match unsupported {
        "basic_service_information" => {
            u_connect.basic_service_information = Some(BasicServiceInformation {
                circuit_mode_type: CircuitModeType::TchS,
                encryption_flag: false,
                communication_type: CommunicationType::P2p,
                slots_per_frame: None,
                speech_service: Some(3),
            });
        }
        "facility" => u_connect.facility = Some(type3_marker()),
        "proprietary" => u_connect.proprietary = Some(type3_marker()),
        _ => unreachable!(),
    }

    build_u_connect_pdu_msg(called_issi, u_connect)
}

fn build_u_alert_msg(called_issi: u32, call_id: u16) -> SapMsg {
    build_u_alert_pdu_msg(
        called_issi,
        UAlert {
            call_identifier: call_id,
            reserved: true,
            simplex_duplex_selection: false,
            basic_service_information: None,
            facility: None,
            proprietary: None,
        },
    )
}

fn build_u_alert_pdu_msg(called_issi: u32, u_alert: UAlert) -> SapMsg {
    let mut sdu = BitBuffer::new_autoexpand(32);
    u_alert.to_bitbuf(&mut sdu).expect("Failed to serialize UAlert");
    sdu.seek(0);

    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 2,
            endpoint_id: 2,
            link_id: 2,
            received_tetra_address: TetraAddress::new(called_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_u_alert_with_unsupported_feature_msg(called_issi: u32, call_id: u16, unsupported: &str) -> SapMsg {
    if unsupported == "reserved" {
        let mut sdu = BitBuffer::new_autoexpand(32);
        sdu.write_bits(CmcePduTypeUl::UAlert.into_raw(), 5);
        sdu.write_bits(call_id as u64, 14);
        sdu.write_bits(0, 1);
        sdu.write_bits(0, 1);
        sdu.write_bits(0, 1);
        sdu.seek(0);

        return SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Mle,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
                sdu,
                handle: 2,
                endpoint_id: 2,
                link_id: 2,
                received_tetra_address: TetraAddress::new(called_issi, SsiType::Issi),
                chan_change_resp_req: false,
                chan_change_handle: None,
            }),
        };
    }

    let mut u_alert = UAlert {
        call_identifier: call_id,
        reserved: true,
        simplex_duplex_selection: false,
        basic_service_information: None,
        facility: None,
        proprietary: None,
    };

    match unsupported {
        "basic_service_information" => {
            u_alert.basic_service_information = Some(BasicServiceInformation {
                circuit_mode_type: CircuitModeType::TchS,
                encryption_flag: false,
                communication_type: CommunicationType::P2p,
                slots_per_frame: None,
                speech_service: Some(3),
            });
        }
        "facility" => u_alert.facility = Some(type3_marker()),
        "proprietary" => u_alert.proprietary = Some(type3_marker()),
        _ => unreachable!(),
    }

    build_u_alert_pdu_msg(called_issi, u_alert)
}

fn build_u_release_msg(calling_issi: u32, call_id: u16) -> SapMsg {
    build_u_release_pdu_msg(
        calling_issi,
        URelease {
            call_identifier: call_id,
            disconnect_cause: DisconnectCause::UserRequestedDisconnection,
            facility: None,
            proprietary: None,
        },
    )
}

fn build_u_release_pdu_msg(calling_issi: u32, u_release: URelease) -> SapMsg {
    let mut sdu = BitBuffer::new_autoexpand(32);
    u_release.to_bitbuf(&mut sdu).expect("Failed to serialize URelease");
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
            received_tetra_address: TetraAddress::new(calling_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_u_release_with_unsupported_feature_msg(calling_issi: u32, call_id: u16, unsupported: &str) -> SapMsg {
    let mut u_release = URelease {
        call_identifier: call_id,
        disconnect_cause: DisconnectCause::UserRequestedDisconnection,
        facility: None,
        proprietary: None,
    };

    match unsupported {
        "facility" => u_release.facility = Some(type3_marker()),
        "proprietary" => u_release.proprietary = Some(type3_marker()),
        _ => unreachable!(),
    }

    build_u_release_pdu_msg(calling_issi, u_release)
}

fn build_u_disconnect_msg(calling_issi: u32, call_id: u16) -> SapMsg {
    build_u_disconnect_with_cause_msg(calling_issi, call_id, DisconnectCause::UserRequestedDisconnection)
}

fn build_u_disconnect_with_cause_msg(calling_issi: u32, call_id: u16, disconnect_cause: DisconnectCause) -> SapMsg {
    build_u_disconnect_pdu_msg(
        calling_issi,
        UDisconnect {
            call_identifier: call_id,
            disconnect_cause,
            facility: None,
            proprietary: None,
        },
    )
}

fn build_u_disconnect_pdu_msg(calling_issi: u32, u_disconnect: UDisconnect) -> SapMsg {
    let mut sdu = BitBuffer::new_autoexpand(32);
    u_disconnect.to_bitbuf(&mut sdu).expect("Failed to serialize UDisconnect");
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
            received_tetra_address: TetraAddress::new(calling_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_u_disconnect_with_unsupported_feature_msg(calling_issi: u32, call_id: u16, unsupported: &str) -> SapMsg {
    let mut u_disconnect = UDisconnect {
        call_identifier: call_id,
        disconnect_cause: DisconnectCause::UserRequestedDisconnection,
        facility: None,
        proprietary: None,
    };

    match unsupported {
        "facility" => u_disconnect.facility = Some(type3_marker()),
        "proprietary" => u_disconnect.proprietary = Some(type3_marker()),
        _ => unreachable!(),
    }

    build_u_disconnect_pdu_msg(calling_issi, u_disconnect)
}

fn build_u_info_pdu_msg(calling_issi: u32, u_info: UInfo) -> SapMsg {
    let mut sdu = BitBuffer::new_autoexpand(64);
    u_info.to_bitbuf(&mut sdu).expect("Failed to serialize UInfo");
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
            received_tetra_address: TetraAddress::new(calling_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_u_info_with_unsupported_feature_msg(calling_issi: u32, call_id: u16, unsupported: &str) -> SapMsg {
    let mut u_info = UInfo {
        call_identifier: call_id,
        poll_response: false,
        modify: None,
        dtmf: None,
        facility: None,
        proprietary: None,
    };

    match unsupported {
        "modify" => u_info.modify = Some(0x1FF),
        "facility" => u_info.facility = Some(type3_marker()),
        "proprietary" => u_info.proprietary = Some(type3_marker()),
        _ => unreachable!(),
    }

    build_u_info_pdu_msg(calling_issi, u_info)
}

fn build_u_tx_demand_msg(calling_issi: u32, call_id: u16) -> SapMsg {
    build_u_tx_demand_msg_with_priority(calling_issi, call_id, 0)
}

fn build_u_tx_demand_msg_with_priority(calling_issi: u32, call_id: u16, tx_demand_priority: u8) -> SapMsg {
    build_u_tx_demand_custom_msg(
        calling_issi,
        UTxDemand {
            call_identifier: call_id,
            tx_demand_priority,
            encryption_control: false,
            reserved: false,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        },
    )
}

fn build_u_tx_demand_custom_msg(calling_issi: u32, u_tx_demand: UTxDemand) -> SapMsg {
    let mut sdu = BitBuffer::new_autoexpand(64);
    u_tx_demand.to_bitbuf(&mut sdu).expect("Failed to serialize UTxDemand");
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
            received_tetra_address: TetraAddress::new(calling_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_u_tx_demand_reserved_bit_msg(calling_issi: u32, call_id: u16) -> SapMsg {
    let mut sdu = BitBuffer::new_autoexpand(32);
    sdu.write_bits(CmcePduTypeUl::UTxDemand.into_raw(), 5);
    sdu.write_bits(call_id as u64, 14);
    sdu.write_bits(0, 2);
    sdu.write_bits(0, 1);
    sdu.write_bits(1, 1);
    sdu.write_bits(0, 1);
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
            received_tetra_address: TetraAddress::new(calling_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_u_tx_ceased_msg(calling_issi: u32, call_id: u16) -> SapMsg {
    let u_tx_ceased = UTxCeased {
        call_identifier: call_id,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(32);
    u_tx_ceased.to_bitbuf(&mut sdu).expect("Failed to serialize UTxCeased");
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
            received_tetra_address: TetraAddress::new(calling_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_tmd_ind_to_cmce(ts: u8, data: Vec<u8>, raw_tch_s_block: Option<PhyBlockNum>) -> SapMsg {
    SapMsg {
        sap: Sap::TmdSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::TmdCircuitDataInd(tetra_saps::tmd::TmdCircuitDataInd { ts, data, raw_tch_s_block }),
    }
}

fn dl_pdu_type(prim: &LcmcMleUnitdataReq) -> Option<CmcePduTypeDl> {
    prim.sdu.peek_bits(5).and_then(|raw| CmcePduTypeDl::try_from(raw).ok())
}

fn is_dl_pdu(prim: &LcmcMleUnitdataReq, pdu_type: CmcePduTypeDl) -> bool {
    dl_pdu_type(prim) == Some(pdu_type)
}

fn parse_d_setup(prim: &LcmcMleUnitdataReq) -> Option<DSetup> {
    if !is_dl_pdu(prim, CmcePduTypeDl::DSetup) {
        return None;
    }
    let mut sdu = prim.sdu.clone();
    sdu.seek(0);
    DSetup::from_bitbuf(&mut sdu).ok()
}

fn parse_d_call_proceeding(prim: &LcmcMleUnitdataReq) -> Option<DCallProceeding> {
    if !is_dl_pdu(prim, CmcePduTypeDl::DCallProceeding) {
        return None;
    }
    let mut sdu = prim.sdu.clone();
    sdu.seek(0);
    DCallProceeding::from_bitbuf(&mut sdu).ok()
}

fn parse_d_connect(prim: &LcmcMleUnitdataReq) -> Option<DConnect> {
    if !is_dl_pdu(prim, CmcePduTypeDl::DConnect) {
        return None;
    }
    let mut sdu = prim.sdu.clone();
    sdu.seek(0);
    DConnect::from_bitbuf(&mut sdu).ok()
}

fn parse_d_alert(prim: &LcmcMleUnitdataReq) -> Option<DAlert> {
    if !is_dl_pdu(prim, CmcePduTypeDl::DAlert) {
        return None;
    }
    let mut sdu = prim.sdu.clone();
    sdu.seek(0);
    DAlert::from_bitbuf(&mut sdu).ok()
}

fn parse_d_connect_acknowledge(prim: &LcmcMleUnitdataReq) -> Option<DConnectAcknowledge> {
    if !is_dl_pdu(prim, CmcePduTypeDl::DConnectAcknowledge) {
        return None;
    }
    let mut sdu = prim.sdu.clone();
    sdu.seek(0);
    DConnectAcknowledge::from_bitbuf(&mut sdu).ok()
}

fn parse_d_tx_granted(prim: &LcmcMleUnitdataReq) -> Option<DTxGranted> {
    if !is_dl_pdu(prim, CmcePduTypeDl::DTxGranted) {
        return None;
    }
    let mut sdu = prim.sdu.clone();
    sdu.seek(0);
    DTxGranted::from_bitbuf(&mut sdu).ok()
}

#[derive(Debug)]
struct WrappedMacDTxGranted {
    resource_sequence: usize,
    logical_channel: LogicalChannel,
    resource: MacResource,
    bl_udata: BlUdata,
    grant: DTxGranted,
}

fn wrapped_d_tx_granted_from_lmac_msgs(msgs: &[SapMsg]) -> Vec<WrappedMacDTxGranted> {
    let mut decoded = Vec::new();
    let mut resource_sequence = 0;

    for msg in msgs {
        let SapMsgInner::TmvUnitdataReq(slot) = &msg.msg else {
            continue;
        };

        for block in [&slot.blk1, &slot.blk2].into_iter().flatten() {
            wrapped_d_tx_granted_from_tmv_block(block, &mut resource_sequence, &mut decoded);
        }
    }

    decoded
}

fn wrapped_d_tx_granted_from_tmv_block(block: &TmvUnitdataReq, resource_sequence: &mut usize, decoded: &mut Vec<WrappedMacDTxGranted>) {
    if block.logical_channel != LogicalChannel::Stch && block.logical_channel != LogicalChannel::SchF {
        return;
    }

    let mut mac_block = block.mac_block.clone();
    mac_block.seek(0);

    while mac_block.get_len_remaining() >= 2 {
        let start_pos = mac_block.get_pos();
        let Some(mac_pdu_type) = mac_block.peek_bits(2) else {
            break;
        };
        if mac_pdu_type != 0b00 {
            break;
        }

        let Ok(resource) = MacResource::from_bitbuf(&mut mac_block) else {
            break;
        };
        let sequence = *resource_sequence;
        *resource_sequence += 1;

        let total_len_bits = resource.length_ind as usize * 8;
        let next_pos = start_pos + total_len_bits;
        if total_len_bits == 0 || next_pos <= mac_block.get_pos() || next_pos > mac_block.get_len() {
            break;
        }

        let mut payload = BitBuffer::from_bitbuffer_pos(&mac_block);
        payload.set_raw_end(payload.get_raw_start() + (next_pos - mac_block.get_pos()));

        if let Some((bl_udata, grant)) = parse_wrapped_d_tx_granted_payload(payload) {
            decoded.push(WrappedMacDTxGranted {
                resource_sequence: sequence,
                logical_channel: block.logical_channel,
                resource,
                bl_udata,
                grant,
            });
        }

        mac_block.seek(next_pos);
    }
}

fn parse_wrapped_d_tx_granted_payload(mut payload: BitBuffer) -> Option<(BlUdata, DTxGranted)> {
    let bl_udata = BlUdata::from_bitbuf(&mut payload).ok()?;
    let discriminator = payload
        .read_field(3, "mle_protocol_discriminator")
        .ok()
        .and_then(|bits| MleProtocolDiscriminator::try_from(bits).ok())?;
    if discriminator != MleProtocolDiscriminator::Cmce {
        return None;
    }
    if payload.peek_bits(5).and_then(|bits| CmcePduTypeDl::try_from(bits).ok()) != Some(CmcePduTypeDl::DTxGranted) {
        return None;
    }

    DTxGranted::from_bitbuf(&mut payload).ok().map(|grant| (bl_udata, grant))
}

fn assert_compact_d_tx_granted_facch(prim: &LcmcMleUnitdataReq, grant: &DTxGranted) {
    if prim.main_address.ssi_type == SsiType::Gssi && grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8 {
        assert_eq!(
            grant.transmitting_party_type_identifier,
            Some(1),
            "GSSI D-TX GRANTED/GrantedToOtherUser should identify the current speaker SSI"
        );
        assert!(
            grant.transmitting_party_address_ssi.is_some(),
            "GSSI D-TX GRANTED/GrantedToOtherUser should carry the current speaker SSI"
        );
        assert!(
            prim.sdu.get_len() > 25,
            "speaker-qualified GSSI D-TX GRANTED should include optional transmitting-party IEs"
        );
    } else {
        assert_eq!(grant.transmitting_party_type_identifier, None);
        assert_eq!(grant.transmitting_party_address_ssi, None);
        assert_eq!(
            prim.sdu.get_len(),
            25,
            "non-GSSI D-TX GRANTED should omit optional transmitting-party IEs so it fits assigned-channel FACCH/STCH"
        );
    }
    assert_eq!(
        prim.unacked_bl_repetitions,
        Some(0),
        "D-TX GRANTED FACCH is time-sensitive floor control and must not repeat stale BL-UDATA over a later floor state"
    );
}

fn assert_d_tx_granted_facch_allocation(
    prim: &LcmcMleUnitdataReq,
    grant: &DTxGranted,
    ts: u8,
    usage: u8,
    ul_dl_assigned: UlDlAssignment,
    context: &str,
) {
    assert_compact_d_tx_granted_facch(prim, grant);
    assert!(
        prim.stealing_permission,
        "{context}: D-TX GRANTED must use assigned-channel FACCH/STCH"
    );
    let chan_alloc = prim
        .chan_alloc
        .as_ref()
        .unwrap_or_else(|| panic!("{context}: FACCH/STCH D-TX GRANTED must carry channel allocation"));
    assert_chan_alloc_matches_circuit(chan_alloc, ts, usage, context);
    assert_eq!(
        chan_alloc.ul_dl_assigned, ul_dl_assigned,
        "{context}: channel allocation direction must match floor state"
    );
}

fn parse_d_tx_interrupt(prim: &LcmcMleUnitdataReq) -> Option<DTxInterrupt> {
    if !is_dl_pdu(prim, CmcePduTypeDl::DTxInterrupt) {
        return None;
    }
    let mut sdu = prim.sdu.clone();
    sdu.seek(0);
    DTxInterrupt::from_bitbuf(&mut sdu).ok()
}

fn parse_d_tx_ceased(prim: &LcmcMleUnitdataReq) -> Option<DTxCeased> {
    if !is_dl_pdu(prim, CmcePduTypeDl::DTxCeased) {
        return None;
    }
    let mut sdu = prim.sdu.clone();
    sdu.seek(0);
    DTxCeased::from_bitbuf(&mut sdu).ok()
}

fn parse_d_disconnect(prim: &LcmcMleUnitdataReq) -> Option<DDisconnect> {
    if !is_dl_pdu(prim, CmcePduTypeDl::DDisconnect) {
        return None;
    }
    let mut sdu = prim.sdu.clone();
    sdu.seek(0);
    DDisconnect::from_bitbuf(&mut sdu).ok()
}

fn parse_d_info(prim: &LcmcMleUnitdataReq) -> Option<DInfo> {
    if !is_dl_pdu(prim, CmcePduTypeDl::DInfo) {
        return None;
    }
    let mut sdu = prim.sdu.clone();
    sdu.seek(0);
    DInfo::from_bitbuf(&mut sdu).ok()
}

fn parse_d_release(prim: &LcmcMleUnitdataReq) -> Option<DRelease> {
    if !is_dl_pdu(prim, CmcePduTypeDl::DRelease) {
        return None;
    }
    let mut sdu = prim.sdu.clone();
    sdu.seek(0);
    DRelease::from_bitbuf(&mut sdu).ok()
}

fn parse_cmce_function_not_supported(prim: &LcmcMleUnitdataReq) -> Option<CmceFunctionNotSupported> {
    if !is_dl_pdu(prim, CmcePduTypeDl::CmceFunctionNotSupported) {
        return None;
    }
    let mut sdu = prim.sdu.clone();
    sdu.seek(0);
    CmceFunctionNotSupported::from_bitbuf(&mut sdu).ok()
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

fn first_d_setup_call_id(msgs: &[SapMsg]) -> u16 {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| pdu.call_identifier),
            _ => None,
        })
        .expect("expected D-SETUP call identifier")
}

/// Extract tx_reporters from D-SETUP messages in the sink output.
fn extract_d_setup_reporters(msgs: &mut Vec<SapMsg>) -> Vec<tetra_core::TxReporter> {
    let mut reporters = vec![];
    for msg in msgs.iter_mut() {
        if msg.dest == TetraEntity::Mle {
            if let SapMsgInner::LcmcMleUnitdataReq(ref mut prim) = msg.msg {
                if is_dl_pdu(prim, CmcePduTypeDl::DSetup) {
                    if let Some(reporter) = prim.tx_reporter.take() {
                        reporters.push(reporter);
                    }
                }
            }
        }
    }
    reporters
}

/// Count D-SETUP messages in sink output without taking reporters.
fn count_d_setups(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|msg| {
            msg.dest == TetraEntity::Mle
                && matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim)
                    if is_dl_pdu(prim, CmcePduTypeDl::DSetup))
        })
        .count()
}

fn count_d_call_proceedings(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_call_proceeding(prim).is_some()))
        .count()
}

fn count_d_connects(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some()))
        .count()
}

fn extract_d_release_reporters(msgs: &mut Vec<SapMsg>) -> Vec<tetra_core::TxReporter> {
    let mut reporters = vec![];
    for msg in msgs.iter_mut() {
        if msg.dest == TetraEntity::Mle {
            if let SapMsgInner::LcmcMleUnitdataReq(ref mut prim) = msg.msg {
                if is_dl_pdu(prim, CmcePduTypeDl::DRelease) {
                    if let Some(reporter) = prim.tx_reporter.take() {
                        reporters.push(reporter);
                    }
                }
            }
        }
    }
    reporters
}

fn extract_d_release_reporters_to(msgs: &mut Vec<SapMsg>, expected_issi: u32) -> Vec<tetra_core::TxReporter> {
    let mut reporters = vec![];
    for msg in msgs.iter_mut() {
        if msg.dest == TetraEntity::Mle {
            if let SapMsgInner::LcmcMleUnitdataReq(ref mut prim) = msg.msg {
                if prim.main_address.ssi == expected_issi && is_dl_pdu(prim, CmcePduTypeDl::DRelease) {
                    if let Some(reporter) = prim.tx_reporter.take() {
                        reporters.push(reporter);
                    }
                }
            }
        }
    }
    reporters
}

fn count_umac_call_ended_or_close(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|msg| {
            msg.dest == TetraEntity::Umac
                && matches!(
                    &msg.msg,
                    SapMsgInner::CmceCallControl(CallControl::CallEnded { .. }) | SapMsgInner::CmceCallControl(CallControl::Close(_, _))
                )
        })
        .count()
}

fn count_umac_open(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|msg| msg.dest == TetraEntity::Umac && matches!(&msg.msg, SapMsgInner::CmceCallControl(CallControl::Open(_))))
        .count()
}

fn assert_chan_alloc_matches_circuit(chan_alloc: &CmceChanAllocReq, ts: u8, usage: u8, context: &str) {
    assert_eq!(
        chan_alloc.usage,
        Some(usage),
        "{context}: channel usage must match opened UMAC circuit"
    );
    assert_eq!(
        chan_alloc.timeslots.iter().filter(|enabled| **enabled).count(),
        1,
        "{context}: exactly one traffic timeslot should be allocated"
    );
    assert!(
        (1..=4).contains(&ts),
        "{context}: opened UMAC circuit timeslot should be in TETRA timeslot range"
    );
    assert!(
        chan_alloc.timeslots[(ts - 1) as usize],
        "{context}: allocated timeslot must match opened UMAC circuit"
    );
}

fn count_umac_floor_granted(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|msg| msg.dest == TetraEntity::Umac && matches!(&msg.msg, SapMsgInner::CmceCallControl(CallControl::FloorGranted { .. })))
        .count()
}

fn count_umac_floor_released(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|msg| msg.dest == TetraEntity::Umac && matches!(&msg.msg, SapMsgInner::CmceCallControl(CallControl::FloorReleased { .. })))
        .count()
}

fn count_d_tx_granted(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_tx_granted(prim).is_some()))
        .count()
}

fn d_tx_granted_to_issi(msgs: &[SapMsg], issi: u32) -> Vec<DTxGranted> {
    msgs.iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) if prim.main_address.ssi == issi => parse_d_tx_granted(prim),
            _ => None,
        })
        .collect()
}

fn d_tx_granted_reporter(msgs: &[SapMsg], target_addr: TetraAddress, transmission_grant: TransmissionGrant) -> tetra_core::TxReporter {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) if prim.main_address == target_addr => parse_d_tx_granted(prim).and_then(|pdu| {
                (pdu.transmission_grant == transmission_grant.into_raw() as u8)
                    .then(|| prim.tx_reporter.as_ref().expect("D-TX GRANTED should carry TxReporter").clone())
            }),
            _ => None,
        })
        .expect("expected D-TX GRANTED reporter")
}

fn first_d_setup_reporter(msgs: &[SapMsg]) -> tetra_core::TxReporter {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).and_then(|_| prim.tx_reporter.clone()),
            _ => None,
        })
        .expect("expected D-SETUP reporter")
}

fn network_group_ready_tuple(msgs: &[SapMsg], brew_uuid: uuid::Uuid) -> Option<(u16, u8)> {
    msgs.iter().find_map(|msg| match &msg.msg {
        SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
            brew_uuid: ready_uuid,
            call_id,
            ts,
            ..
        }) if *ready_uuid == brew_uuid => Some((*call_id, *ts)),
        _ => None,
    })
}

fn transmit_network_group_setup_and_drain(
    test: &mut ComponentTest,
    setup_msgs: &[SapMsg],
    brew_uuid: uuid::Uuid,
) -> (u16, u8, Vec<SapMsg>) {
    assert!(
        network_group_ready_tuple(setup_msgs, brew_uuid).is_none(),
        "network-origin group setup must wait for RF D-SETUP transmission before Brew ready"
    );
    let reporter = first_d_setup_reporter(setup_msgs);
    reporter.mark_transmitted();
    test.run_stack(Some(1));
    let ready_msgs = test.dump_sinks();
    let (call_id, ts) = network_group_ready_tuple(&ready_msgs, brew_uuid)
        .expect("network-origin group setup should report ready after D-SETUP transmission");
    (call_id, ts, ready_msgs)
}

fn transmit_positive_group_grants_and_drain(test: &mut ComponentTest, msgs: &[SapMsg]) -> Vec<SapMsg> {
    let mut transmitted = 0;
    for msg in msgs {
        let SapMsgInner::LcmcMleUnitdataReq(prim) = &msg.msg else {
            continue;
        };
        let Some(grant) = parse_d_tx_granted(prim) else {
            continue;
        };
        if grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8 {
            prim.tx_reporter
                .as_ref()
                .expect("positive group D-TX GRANTED should carry TxReporter")
                .mark_transmitted();
            transmitted += 1;
        }
    }
    assert!(transmitted > 0, "expected at least one positive group D-TX GRANTED reporter");
    test.run_stack(Some(1));
    test.dump_sinks()
}

fn transmit_positive_group_grant_and_assert_floor(
    test: &mut ComponentTest,
    msgs: &[SapMsg],
    requester_addr: TetraAddress,
    call_id: u16,
    source_issi: u32,
    dest_gssi: u32,
    ts: u8,
) -> Vec<SapMsg> {
    let requester_reporter = d_tx_granted_reporter(msgs, requester_addr, TransmissionGrant::Granted);
    assert_eq!(requester_reporter.get_state(), TxState::Pending);
    requester_reporter.mark_transmitted();
    test.run_stack(Some(1));
    let activation_msgs = test.dump_sinks();
    assert_eq!(count_umac_floor_granted(&activation_msgs), 1);
    assert!(activation_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi: got_source_issi,
                dest_gssi: got_dest_gssi,
                ts: got_ts,
            }) if *got_call_id == call_id
                && *got_source_issi == source_issi
                && *got_dest_gssi == dest_gssi
                && *got_ts == ts
        )
    }));
    activation_msgs
}

fn count_d_tx_interrupt(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_tx_interrupt(prim).is_some()))
        .count()
}

fn count_d_tx_ceased(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_tx_ceased(prim).is_some()))
        .count()
}

fn count_d_tx_ceased_to_issi(msgs: &[SapMsg], issi: u32) -> usize {
    msgs.iter()
        .filter(|msg| {
            matches!(
                &msg.msg,
                SapMsgInner::LcmcMleUnitdataReq(prim) if prim.main_address.ssi == issi && parse_d_tx_ceased(prim).is_some()
            )
        })
        .count()
}

fn d_info_reset_t310_prims(msgs: &[SapMsg]) -> Vec<(&LcmcMleUnitdataReq, DInfo)> {
    msgs.iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_info(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .filter(|(_, pdu)| pdu.reset_call_time_out_timer_t310_)
        .collect()
}

fn assert_no_group_d_info_reset_t310(msgs: &[SapMsg], context: &str) {
    let resets = d_info_reset_t310_prims(msgs);
    assert!(
        resets.is_empty(),
        "{context}: floor grant must not emit timer-only group D-INFO reset T310 on FACCH"
    );
}

fn count_d_releases(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_release(prim).is_some()))
        .count()
}

fn count_d_disconnects(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_disconnect(prim).is_some()))
        .count()
}

fn assert_established_p2p_release_pdus(msgs: &[SapMsg], call_id: u16, disconnect_cause: DisconnectCause) {
    assert_established_p2p_release_pdus_to(msgs, call_id, disconnect_cause, &[TEST_ISSI, TEST_CALLED_ISSI]);
}

fn assert_established_p2p_release_pdus_to(msgs: &[SapMsg], call_id: u16, disconnect_cause: DisconnectCause, expected_ssis: &[u32]) {
    let releases: Vec<_> = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();

    assert_eq!(
        releases.len(),
        expected_ssis.len(),
        "Established P2P release should send one reporter-tracked FACCH/STCH D-RELEASE to the expected MS leg(s)"
    );

    let mut facch_ssis = Vec::new();
    for (prim, d_release) in releases {
        assert_eq!(d_release.call_identifier, call_id);
        assert_eq!(d_release.disconnect_cause, disconnect_cause);
        assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
        assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
        assert!(
            prim.stealing_permission,
            "established-call D-RELEASE must use assigned-channel FACCH/STCH, not duplicate MCCH fallback"
        );
        facch_ssis.push(prim.main_address.ssi);
        assert!(prim.tx_reporter.is_some(), "FACCH/STCH D-RELEASE must be reporter-tracked");
        let chan_alloc = prim
            .chan_alloc
            .as_ref()
            .expect("FACCH/STCH D-RELEASE should preserve assigned-channel allocation");
        assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Dl);
        assert!(chan_alloc.usage.is_some());
        assert!(chan_alloc.timeslots.iter().any(|enabled| *enabled));
    }

    facch_ssis.sort_unstable();
    let mut expected = expected_ssis.to_vec();
    expected.sort_unstable();
    assert_eq!(facch_ssis, expected);
}

fn assert_no_d_info(msgs: &[SapMsg]) {
    assert!(
        !msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_info(prim).is_some())),
        "message set should not include D-INFO"
    );
}

fn assert_release_notification_to(msgs: &[SapMsg], expected_issi: u32, expected_notification: Option<u64>) {
    let release = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .find(|(prim, _)| prim.main_address.ssi == expected_issi)
        .unwrap_or_else(|| panic!("expected D-RELEASE to ISSI {expected_issi}"));

    assert_eq!(
        release.1.notification_indicator, expected_notification,
        "unexpected D-RELEASE notification indicator for ISSI {expected_issi}"
    );
}

fn assert_p2p_setup_rejected_with_dummy_call_id(msgs: &[SapMsg], calling_issi: u32) {
    assert_p2p_setup_rejected_with_dummy_call_id_and_cause(msgs, calling_issi, DisconnectCause::RequestedServiceNotAvailable);
}

fn assert_p2p_setup_rejected_with_dummy_call_id_and_cause(msgs: &[SapMsg], calling_issi: u32, disconnect_cause: DisconnectCause) {
    let releases: Vec<_> = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(releases.len(), 1, "unsupported P2P U-SETUP should receive one D-RELEASE");
    let (release_prim, release) = &releases[0];
    assert_eq!(release.call_identifier, 0);
    assert_eq!(release.disconnect_cause, disconnect_cause);
    assert_eq!(release_prim.main_address.ssi, calling_issi);
    assert_eq!(release_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(release_prim.layer2service, Layer2Service::Unacknowledged);
    assert!(release_prim.chan_alloc.is_none());
    assert_eq!(count_d_setups(msgs), 0);
    assert_eq!(count_umac_open(msgs), 0);
}

fn assert_one_cmce_function_not_supported(
    msgs: &[SapMsg],
    target_issi: u32,
    pdu_type: CmcePduTypeUl,
    call_id: Option<u16>,
    field_level: bool,
) {
    let unsupported: Vec<_> = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_cmce_function_not_supported(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(unsupported.len(), 1, "expected one CMCE FUNCTION NOT SUPPORTED for {pdu_type:?}");
    let (prim, pdu) = &unsupported[0];
    assert_eq!(pdu.not_supported_pdu_type, pdu_type.into_raw() as u8);
    assert_eq!(pdu.call_identifier_present, call_id.is_some());
    assert_eq!(pdu.call_identifier, call_id.map(u64::from));
    assert_eq!(prim.main_address.ssi, target_issi);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert!(prim.chan_alloc.is_none());
    if field_level {
        assert_ne!(
            pdu.function_not_supported_pointer, 0,
            "field-level unsupported response must point at the unsupported element"
        );
        let len = pdu
            .length_of_received_pdu_extract
            .expect("field-level unsupported response should include received-PDU extract");
        assert!(len > pdu.function_not_supported_pointer as u64);
        assert!(pdu.received_pdu_extract.is_some());
    } else {
        assert_eq!(pdu.function_not_supported_pointer, 0);
        assert_eq!(pdu.length_of_received_pdu_extract, None);
        assert!(pdu.received_pdu_extract.is_none());
    }
}

fn count_network_call_end(msgs: &[SapMsg], brew_uuid: uuid::Uuid) -> usize {
    msgs.iter()
        .filter(|msg| {
            msg.dest == TetraEntity::Brew
                && matches!(
                    &msg.msg,
                    SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid: got_uuid }) if *got_uuid == brew_uuid
                )
        })
        .count()
}

fn count_network_circuit_release(msgs: &[SapMsg], brew_uuid: uuid::Uuid) -> usize {
    msgs.iter()
        .filter(|msg| {
            msg.dest == TetraEntity::Brew
                && matches!(
                    &msg.msg,
                    SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease {
                        brew_uuid: got_uuid,
                        ..
                    }) if *got_uuid == brew_uuid
                )
        })
        .count()
}

fn count_network_circuit_media_ready(msgs: &[SapMsg], brew_uuid: uuid::Uuid) -> usize {
    msgs.iter()
        .filter(|msg| {
            msg.dest == TetraEntity::Brew
                && matches!(
                    &msg.msg,
                    SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady {
                        brew_uuid: got_uuid,
                        ..
                    }) if *got_uuid == brew_uuid
                )
        })
        .count()
}

fn count_network_circuit_connect_confirm(msgs: &[SapMsg], brew_uuid: uuid::Uuid) -> usize {
    msgs.iter()
        .filter(|msg| {
            msg.dest == TetraEntity::Brew
                && matches!(
                    &msg.msg,
                    SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
                        brew_uuid: got_uuid,
                        ..
                    }) if *got_uuid == brew_uuid
                )
        })
        .count()
}

fn count_brew_floor_granted(msgs: &[SapMsg], call_id: u16, source_issi: u32, dest_ssi: u32) -> usize {
    msgs.iter()
        .filter(|msg| {
            msg.dest == TetraEntity::Brew
                && matches!(
                    &msg.msg,
                    SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                        call_id: got_call_id,
                        source_issi: got_source,
                        dest_gssi: got_dest,
                        ..
                    }) if *got_call_id == call_id && *got_source == source_issi && *got_dest == dest_ssi
                )
        })
        .count()
}

fn count_brew_floor_released(msgs: &[SapMsg], call_id: u16) -> usize {
    msgs.iter()
        .filter(|msg| {
            msg.dest == TetraEntity::Brew
                && matches!(
                    &msg.msg,
                    SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id: got_call_id, .. }) if *got_call_id == call_id
                )
        })
        .count()
}

fn count_network_circuit_simplex_granted(msgs: &[SapMsg], brew_uuid: uuid::Uuid, grant: TransmissionGrant) -> usize {
    msgs.iter()
        .filter(|msg| {
            msg.dest == TetraEntity::Brew
                && matches!(
                    &msg.msg,
                    SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexGranted {
                        brew_uuid: got_uuid,
                        grant: got_grant,
                        ..
                    }) if *got_uuid == brew_uuid && *got_grant == grant.into_raw() as u8
                )
        })
        .count()
}

fn d_connect_reporter(msgs: &[SapMsg], issi: u32) -> tetra_core::TxReporter {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) if prim.main_address.ssi == issi && parse_d_connect(prim).is_some() => {
                prim.tx_reporter.clone()
            }
            _ => None,
        })
        .expect("expected D-CONNECT with TxReporter")
}

fn first_d_connect_reporter(msgs: &[SapMsg]) -> tetra_core::TxReporter {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some() => prim.tx_reporter.clone(),
            _ => None,
        })
        .expect("expected D-CONNECT with TxReporter")
}

fn acknowledge_d_connect(msgs: &[SapMsg], issi: u32) {
    let reporter = d_connect_reporter(msgs, issi);
    reporter.mark_transmitted();
    reporter.mark_acknowledged();
}

fn acknowledge_first_d_connect(msgs: &[SapMsg]) {
    let reporter = first_d_connect_reporter(msgs);
    reporter.mark_transmitted();
    reporter.mark_acknowledged();
}

fn build_network_call_end_msg(brew_uuid: uuid::Uuid) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
    }
}

fn start_network_group_call(
    test: &mut ComponentTest,
    brew_uuid: uuid::Uuid,
    source_issi: u32,
    dest_gssi: u32,
    priority: u8,
) -> (u16, u8, Vec<SapMsg>) {
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi,
            dest_gssi,
            priority,
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let (call_id, ts, ready_msgs) = transmit_network_group_setup_and_drain(test, &setup_msgs, brew_uuid);
    let mut combined_msgs = setup_msgs;
    combined_msgs.extend(ready_msgs);
    (call_id, ts, combined_msgs)
}

fn start_group_call(test: &mut ComponentTest) -> u16 {
    let u_setup_msg = build_u_setup_msg(TEST_ISSI, TEST_GSSI);
    test.submit_message(u_setup_msg);
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();
    let initial_setups = count_d_setups(&initial_msgs);
    assert!(initial_setups > 0, "Expected initial D-SETUP after U-SETUP");
    first_d_setup_call_id(&initial_msgs)
}

fn start_group_call_with_circuit(test: &mut ComponentTest) -> (u16, u8, u8) {
    start_group_call_with_circuit_for(test, TEST_ISSI, TEST_GSSI)
}

fn start_group_call_with_circuit_for(test: &mut ComponentTest, calling_issi: u32, dest_gssi: u32) -> (u16, u8, u8) {
    let u_setup_msg = build_u_setup_msg(calling_issi, dest_gssi);
    test.submit_message(u_setup_msg);
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();
    let initial_setups = count_d_setups(&initial_msgs);
    assert!(initial_setups > 0, "Expected initial D-SETUP after U-SETUP");
    let call_id = first_d_setup_call_id(&initial_msgs);
    let circuit = initial_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .expect("group U-SETUP should open a traffic circuit");
    assert_eq!(
        circuit.active_addr,
        Some(TetraAddress::new(dest_gssi, SsiType::Gssi)),
        "group traffic circuit should be scoped to the destination GSSI"
    );
    assert_eq!(
        circuit.active_secondary_addrs,
        vec![TetraAddress::issi(calling_issi)],
        "group traffic circuit should carry only the first speaker ISSI as secondary without changing the primary GSSI scope"
    );
    (call_id, circuit.ts, circuit.usage)
}

fn start_group_call_with_u_setup(test: &mut ComponentTest, u_setup: USetup) -> u16 {
    test.submit_message(build_u_setup_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();
    let initial_setups = count_d_setups(&initial_msgs);
    assert!(initial_setups > 0, "Expected initial D-SETUP after U-SETUP");
    first_d_setup_call_id(&initial_msgs)
}

#[test]
fn test_large_group_setup_uses_one_gssi_d_setup_and_one_umac_open() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    let member_count = LARGE_GSSI_MEMBER_COUNT + 2;
    let first_issi = 420_000_u32;
    let speaker_issi = first_issi;
    for offset in 0..member_count {
        let issi = first_issi + offset;
        submit_subscriber_update(&mut test, issi, Vec::new(), BrewSubscriberAction::Register);
        submit_subscriber_update(&mut test, issi, vec![TEST_GSSI], BrewSubscriberAction::Affiliate);
    }
    test.run_stack(Some((member_count as usize * 2) + 16));
    let _ = test.dump_sinks();

    test.submit_message(build_u_setup_msg(speaker_issi, TEST_GSSI));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.2.1 keeps normal group setup addressed to the
    // group identity. A large GSSI must not fan out one D-SETUP per affiliate.
    let setups: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(setups.len(), 1, "large group setup must emit one GSSI D-SETUP");
    assert_eq!(setups[0].0.main_address, TetraAddress::new(TEST_GSSI, SsiType::Gssi));

    let opens: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .collect();
    assert_eq!(opens.len(), 1, "large group setup must open one GSSI traffic circuit");
    assert_eq!(opens[0].active_addr, Some(TetraAddress::new(TEST_GSSI, SsiType::Gssi)));
    assert_eq!(opens[0].active_secondary_addrs, vec![TetraAddress::issi(speaker_issi)]);
    assert_eq!(count_umac_open(&setup_msgs), 1);
    assert_eq!(count_d_releases(&setup_msgs), 0);
}

#[test]
fn test_group_call_id_wrap_skips_live_group_call_and_preserves_ptt() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let (first_call_id, first_ts, first_usage) = start_group_call_with_circuit(&mut test);

    let second_group_caller = TEST_OTHER_ISSI;
    let second_group_listener = TEST_OTHER_ISSI + 10;
    register_subscriber(&mut test, second_group_caller, TEST_CALLED_GSSI);
    register_subscriber(&mut test, second_group_listener, TEST_CALLED_GSSI);

    force_cmce_next_call_identifier(&mut test, first_call_id);
    let (second_call_id, _, _) = start_group_call_with_circuit_for(&mut test, second_group_caller, TEST_CALLED_GSSI);

    // EN 300 392-2 clause 14.2.3 uses the SwMI call identifier as the
    // reference for call handling, and table 14.36 gives only 14 real bits.
    // After wrap, a fresh group setup must skip a still-live group call id.
    assert_ne!(
        second_call_id, first_call_id,
        "new group call must not overwrite an existing active group call when call-id wraps"
    );
    let active_ids = cmce_debug_active_call_ids(&mut test);
    assert!(active_ids.contains(&first_call_id), "first group call id should remain live");
    assert!(active_ids.contains(&second_call_id), "second group call id should be live");

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, first_call_id));
    test.run_stack(Some(1));
    let ptt_msgs = test.dump_sinks();
    let queued_grant = ptt_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .find(|(prim, grant)| prim.main_address == TetraAddress::issi(TEST_CALLED_ISSI) && grant.call_identifier == first_call_id)
        .expect("original group call must still queue return PTT after another wrapped allocation");
    assert_eq!(queued_grant.1.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        queued_grant.0,
        &queued_grant.1,
        first_ts,
        first_usage,
        UlDlAssignment::Dl,
        "group call-id wrap queued PTT",
    );
    assert_eq!(count_d_releases(&ptt_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&ptt_msgs), 0);
}

#[test]
fn test_group_setup_call_id_wrap_skips_live_private_call() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    submit_subscriber_update(&mut test, TEST_ISSI, Vec::new(), BrewSubscriberAction::Register);
    submit_subscriber_update(&mut test, TEST_CALLED_ISSI, Vec::new(), BrewSubscriberAction::Register);
    test.run_stack(Some(4));
    let _ = test.dump_sinks();
    let (private_call_id, _private_setup_msgs) = start_p2p_setup(&mut test);

    let group_caller = TEST_OTHER_ISSI;
    let group_listener = TEST_OTHER_ISSI + 20;
    register_subscriber(&mut test, group_caller, TEST_GSSI);
    register_subscriber(&mut test, group_listener, TEST_GSSI);

    force_cmce_next_call_identifier(&mut test, private_call_id);
    let (group_call_id, _, _) = start_group_call_with_circuit_for(&mut test, group_caller, TEST_GSSI);

    // EN 300 392-2 clauses 14.5.1.1.2/14.5.1.2.1 keep the private-call
    // identifier as the reference for subsequent individual-call PDUs. A
    // simultaneous group setup must not reuse it after wrap.
    assert_ne!(
        group_call_id, private_call_id,
        "group setup must skip a live private-call identifier"
    );
    let active_ids = cmce_debug_active_call_ids(&mut test);
    assert!(active_ids.contains(&private_call_id), "private call id should remain live");
    assert!(active_ids.contains(&group_call_id), "group call id should be live");
}

fn start_p2p_setup(test: &mut ComponentTest) -> (u16, Vec<SapMsg>) {
    start_p2p_setup_between(test, TEST_ISSI, TEST_CALLED_ISSI)
}

fn start_p2p_setup_between(test: &mut ComponentTest, caller_issi: u32, called_issi: u32) -> (u16, Vec<SapMsg>) {
    test.submit_message(build_u_setup_p2p_msg(caller_issi, called_issi));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&msgs);
    (call_id, msgs)
}

fn start_p2p_setup_with_u_setup(test: &mut ComponentTest, u_setup: USetup) -> (u16, Vec<SapMsg>) {
    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&msgs);
    (call_id, msgs)
}

fn start_active_p2p_call(test: &mut ComponentTest) -> u16 {
    let (call_id, _connect_msgs) = start_active_p2p_call_with_connect_msgs(test);
    call_id
}

fn start_active_p2p_call_with_connect_msgs(test: &mut ComponentTest) -> (u16, Vec<SapMsg>) {
    start_active_p2p_call_between_with_connect_msgs(test, TEST_ISSI, TEST_CALLED_ISSI)
}

fn start_active_p2p_call_between_with_connect_msgs(test: &mut ComponentTest, caller_issi: u32, called_issi: u32) -> (u16, Vec<SapMsg>) {
    let (call_id, _setup_msgs) = start_p2p_setup_between(test, caller_issi, called_issi);
    let (mut connect_msgs, after_called_ack_msgs) =
        submit_p2p_connect_and_ack_called(test, build_u_connect_msg(called_issi, call_id), called_issi);
    connect_msgs.extend(after_called_ack_msgs);
    assert!(count_umac_open(&connect_msgs) >= 1, "U-CONNECT should open the P2P traffic circuit");
    let floor_msgs = grant_initial_p2p_floor(test, caller_issi, call_id);
    connect_msgs.extend(floor_msgs);
    (call_id, connect_msgs)
}

fn grant_initial_p2p_floor(test: &mut ComponentTest, speaker_issi: u32, call_id: u16) -> Vec<SapMsg> {
    test.submit_message(build_u_tx_demand_msg(speaker_issi, call_id));
    test.run_stack(Some(1));
    let floor_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_floor_granted(&floor_msgs),
        1,
        "private simplex test helper should produce one floor grant for the requested PTT"
    );
    floor_msgs
}

fn submit_p2p_connect_and_ack_called(test: &mut ComponentTest, u_connect_msg: SapMsg, called_issi: u32) -> (Vec<SapMsg>, Vec<SapMsg>) {
    test.submit_message(u_connect_msg);
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();
    acknowledge_called_d_connect_ack(&connect_msgs, called_issi);
    test.run_stack(Some(1));
    let mut after_called_ack_msgs = test.dump_sinks();
    acknowledge_first_d_connect(&after_called_ack_msgs);
    test.run_stack(Some(1));
    after_called_ack_msgs.extend(test.dump_sinks());
    (connect_msgs, after_called_ack_msgs)
}

fn direct_private_simplex_connect_msgs(u_setup: USetup) -> (u16, Vec<SapMsg>) {
    direct_private_simplex_connect_msgs_with_config(u_setup, ComponentTest::get_default_test_config(StackMode::Bs))
}

fn direct_private_simplex_connect_msgs_with_config(u_setup: USetup, config: tetra_config::bluestation::StackConfig) -> (u16, Vec<SapMsg>) {
    let (call_id, mut ack_msgs, after_called_ack_msgs, after_caller_ack_msgs) = direct_private_simplex_connect_phases(u_setup, config);
    ack_msgs.extend(after_called_ack_msgs);
    ack_msgs.extend(after_caller_ack_msgs);
    (call_id, ack_msgs)
}

fn direct_private_simplex_connect_phases(
    u_setup: USetup,
    config: tetra_config::bluestation::StackConfig,
) -> (u16, Vec<SapMsg>, Vec<SapMsg>, Vec<SapMsg>) {
    let shared = SharedConfig::from_parts(config, None);
    let mut cmce = CmceBs::new(shared, None, None);
    let mut queue = MessageQueue::new();

    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_ISSI, TEST_GSSI);
    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    cmce.rx_prim(&mut queue, build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    let setup_msgs = drain_message_queue(&mut queue);
    let call_id = first_d_setup_call_id(&setup_msgs);

    cmce.rx_prim(&mut queue, build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    let ack_msgs = drain_message_queue(&mut queue);
    acknowledge_called_d_connect_ack(&ack_msgs, TEST_CALLED_ISSI);
    cmce.tick_start(&mut queue, TdmaTime { h: 0, m: 1, f: 1, t: 1 });
    let after_ack_msgs = drain_message_queue(&mut queue);
    acknowledge_first_d_connect(&after_ack_msgs);
    cmce.tick_start(&mut queue, TdmaTime { h: 0, m: 1, f: 1, t: 2 });
    let after_caller_ack_msgs = drain_message_queue(&mut queue);
    (call_id, ack_msgs, after_ack_msgs, after_caller_ack_msgs)
}

fn acknowledge_called_d_connect_ack(msgs: &[SapMsg], called_issi: u32) {
    transmit_called_d_connect_ack(msgs, called_issi);
    let reporter = called_d_connect_ack_reporter(msgs, called_issi);
    if !reporter.is_in_final_state() {
        reporter.mark_acknowledged();
    }
}

fn transmit_called_d_connect_ack(msgs: &[SapMsg], called_issi: u32) {
    let reporter = called_d_connect_ack_reporter(msgs, called_issi);
    reporter.mark_transmitted();
}

fn called_d_connect_ack_reporter(msgs: &[SapMsg], called_issi: u32) -> tetra_core::TxReporter {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim)
                if prim.main_address.ssi == called_issi && parse_d_connect_acknowledge(prim).is_some() =>
            {
                prim.tx_reporter.clone()
            }
            _ => None,
        })
        .expect("called MS should receive D-CONNECT ACKNOWLEDGE before caller D-CONNECT")
}

fn discard_called_d_connect_ack(msgs: &[SapMsg], called_issi: u32) {
    let reporter = called_d_connect_ack_reporter(msgs, called_issi);
    reporter.mark_discarded();
}

fn count_d_connect_acknowledges(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect_acknowledge(prim).is_some()))
        .count()
}

fn assert_private_simplex_caller_d_connect_with_setup_floor(msgs: &[SapMsg], source_issi: u32, dest_issi: u32) {
    let open_idx = msgs
        .iter()
        .position(|msg| matches!(&msg.msg, SapMsgInner::CmceCallControl(CallControl::Open(_))))
        .expect("P2P U-CONNECT should open the UMAC bearer");
    let caller_connect_idx = msgs
        .iter()
        .position(|msg| {
            matches!(
                &msg.msg,
                SapMsgInner::LcmcMleUnitdataReq(prim)
                    if prim.main_address.ssi == TEST_ISSI && parse_d_connect(prim).is_some()
            )
        })
        .expect("private simplex setup should emit caller D-CONNECT after called ACK");

    assert!(
        open_idx < caller_connect_idx,
        "EN 300 392-2 14.5.1.1.2: caller D-CONNECT must stay after the called D-CONNECT ACK path"
    );
    assert_eq!(
        count_umac_floor_granted(msgs),
        1,
        "EN 300 392-2 14.5.1.1.1/14.5.1.1.2 and 14.5.1.2.1 b): the setup TransmissionGrant defines the initial U-plane floor"
    );
    assert!(
        msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                source_issi: got_source,
                dest_gssi,
                ..
            }) if *got_source == source_issi && *dest_gssi == dest_issi
        )),
        "initial setup floor should follow the MS that received TransmissionGrant::Granted"
    );
}

fn start_duplex_called_party_disconnect_with_peer_d_release(
    test: &mut ComponentTest,
    call_id: u16,
) -> (Vec<tetra_core::TxReporter>, Vec<tetra_core::TxReporter>) {
    test.submit_message(build_u_disconnect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let mut disconnect_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus_to(
        &disconnect_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_CALLED_ISSI, TEST_ISSI],
    );
    assert_no_d_info(&disconnect_msgs);
    assert_release_notification_to(&disconnect_msgs, TEST_CALLED_ISSI, None);
    assert_release_notification_to(&disconnect_msgs, TEST_ISSI, None);
    assert_eq!(
        count_d_disconnects(&disconnect_msgs),
        0,
        "Duplex local RF P2P clear must use D-RELEASE, not peer D-DISCONNECT"
    );
    assert_eq!(count_umac_call_ended_or_close(&disconnect_msgs), 0);

    let release_ack_reporters = extract_d_release_reporters_to(&mut disconnect_msgs, TEST_CALLED_ISSI);
    assert_eq!(release_ack_reporters.len(), 1);
    let peer_release_reporters = extract_d_release_reporters_to(&mut disconnect_msgs, TEST_ISSI);
    assert_eq!(
        peer_release_reporters.len(),
        1,
        "Duplex assigned-channel peer D-RELEASE must carry one TxReporter"
    );
    (release_ack_reporters, peer_release_reporters)
}

fn start_called_party_disconnect_with_peer_d_release(
    test: &mut ComponentTest,
    _dltime: TdmaTime,
    call_id: u16,
) -> (Vec<tetra_core::TxReporter>, Vec<tetra_core::TxReporter>) {
    start_duplex_called_party_disconnect_with_peer_d_release(test, call_id)
}

fn p2p_open_ts_for(msgs: &[SapMsg], issi: u32) -> u8 {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit))
                if circuit.active_addr == Some(TetraAddress::new(issi, SsiType::Issi))
                    || circuit.active_secondary_addrs.contains(&TetraAddress::new(issi, SsiType::Issi)) =>
            {
                Some(circuit.ts)
            }
            _ => None,
        })
        .expect("expected UMAC Open circuit for P2P participant")
}

fn build_ul_inactivity_timeout_msg(ts: u8) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Umac,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::UlInactivityTimeout { ts }),
    }
}

fn start_active_duplex_p2p_call(test: &mut ComponentTest) -> u16 {
    let mut u_setup = default_p2p_u_setup();
    u_setup.simplex_duplex_selection = true;
    let (call_id, _setup_msgs) = start_p2p_setup_with_u_setup(test, u_setup);
    let (mut connect_msgs, after_called_ack_msgs) =
        submit_p2p_connect_and_ack_called(test, build_u_connect_custom_msg(TEST_CALLED_ISSI, call_id, true), TEST_CALLED_ISSI);
    connect_msgs.extend(after_called_ack_msgs);
    assert!(
        count_umac_open(&connect_msgs) >= 1,
        "duplex U-CONNECT should open the P2P traffic circuit"
    );
    call_id
}

#[test]
fn test_network_group_call_start_propagates_priority_to_d_setup() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(CfgBrew {
        host: "test-brew.local".to_string(),
        port: 443,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: std::time::Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        feature_rssi_export: false,
        whitelisted_ssis: None,
    });

    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let brew_uuid = uuid::Uuid::new_v4();
    // EN 300 392-2 table 14.46: priority 11 is the highest ordinary
    // non-pre-emptive call priority. It must remain a normal setup while
    // transmission interruption is default-off.
    let priority = 11;
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: TEST_CALLED_ISSI,
            dest_gssi: TEST_GSSI,
            priority,
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();

    let open_circuit = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .expect("network-origin group setup should open a UMAC traffic circuit");
    assert_eq!(open_circuit.peer_ts, None);
    assert_eq!(open_circuit.active_addr, Some(TetraAddress::new(TEST_GSSI, SsiType::Gssi)));
    assert_eq!(
        open_circuit.active_secondary_addrs,
        vec![TetraAddress::issi(TEST_CALLED_ISSI)],
        "network-origin group Open should keep the group GSSI primary and carry the current speaker ISSI only as secondary"
    );
    assert_eq!(
        open_circuit.dl_media_source,
        CircuitDlMediaSource::SwMI,
        "network-origin group calls carry downlink speech from Brew/SwMI, not from local RF loopback"
    );

    let setups: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(setups.len(), 1, "Expected one network-origin D-SETUP to the called group");

    let (setup_prim, setup) = &setups[0];
    assert_eq!(setup.call_priority, priority);
    assert_eq!(setup.calling_party_address_ssi, Some(TEST_CALLED_ISSI));
    assert_eq!(setup_prim.main_address.ssi, TEST_GSSI);
    assert_eq!(setup_prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(setup_prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(setup.transmission_grant, TransmissionGrant::GrantedToOtherUser);
    assert!(!setup.transmission_request_permission);
    assert!(setup_prim.chan_alloc.is_some(), "group D-SETUP should carry traffic allocation");
    assert!(
        setup_prim.tx_reporter.is_some(),
        "network-origin group D-SETUP should be reporter-tracked before Brew media is opened"
    );

    assert!(
        network_group_ready_tuple(&setup_msgs, brew_uuid).is_none(),
        "network-origin group setup must not notify Brew ready before RF D-SETUP transmission"
    );

    setup_prim.tx_reporter.as_ref().unwrap().mark_transmitted();
    test.run_stack(Some(1));
    let ready_msgs = test.dump_sinks();
    let swmi_update_pos = ready_msgs
        .iter()
        .position(|msg| {
            matches!(
                &msg.msg,
                SapMsgInner::CmceCallControl(CallControl::SetDlMediaSource {
                    ts: update_ts,
                    dl_media_source: CircuitDlMediaSource::SwMI,
                }) if *update_ts == open_circuit.ts
            )
        })
        .expect("network-origin group ready should switch UMAC DL media source to SwMI before granting the floor");
    let floor_granted_pos = ready_msgs
        .iter()
        .position(|msg| matches!(&msg.msg, SapMsgInner::CmceCallControl(CallControl::FloorGranted { .. })))
        .expect("network-origin group ready should grant the RF floor");
    assert!(
        swmi_update_pos < floor_granted_pos,
        "UMAC must switch to Brew/SwMI media before FloorGranted opens U-plane"
    );
    assert!(
        network_group_ready_tuple(&ready_msgs, brew_uuid).is_some(),
        "network-origin group setup should notify Brew after RF D-SETUP transmission"
    );
}

#[test]
fn test_brew_external_affiliate_counts_as_listener_without_shared_rf_registration() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    submit_subscriber_update(&mut test, TEST_ISSI, Vec::new(), BrewSubscriberAction::Register);
    test.run_stack(Some(1));
    test.dump_sinks();

    let external_issi = 2_261_313;
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi: external_issi,
            groups: vec![TEST_GSSI],
            action: BrewSubscriberAction::Affiliate,
        }),
    });
    test.run_stack(Some(1));
    test.dump_sinks();

    {
        let state = test.config.state_read();
        assert!(
            !state.subscribers.is_registered(external_issi),
            "Brew/outside subscribers must not be registered as local RF subscribers"
        );
        assert!(
            !state.subscribers.group_members(TEST_GSSI).contains(&external_issi),
            "Brew/outside subscribers must not enter the RF/EG group-member registry"
        );
    }

    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    assert!(
        count_umac_open(&setup_msgs) >= 1,
        "external Brew affiliation should still count as a CMCE listener for group setup"
    );
    assert_eq!(
        count_d_releases(&setup_msgs),
        0,
        "external Brew listener accounting must prevent a false no-listener rejection"
    );
}

#[test]
fn test_brew_external_only_affiliate_does_not_open_network_rf_downlink() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    submit_subscriber_update_from(
        &mut test,
        TetraEntity::Brew,
        2_261_313,
        vec![TEST_GSSI],
        BrewSubscriberAction::Affiliate,
    );
    test.run_stack(Some(1));
    test.dump_sinks();

    let brew_uuid = uuid::Uuid::new_v4();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: TEST_OTHER_ISSI,
            dest_gssi: TEST_GSSI,
            priority: 7,
        }),
    });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // Brew subscriber events are interconnect state, not EN 300 392-2 MM
    // group affiliation on the local air interface. With no local RF listener,
    // a Brew-origin group call must not allocate a local traffic channel.
    assert_eq!(count_network_call_end(&msgs, brew_uuid), 1);
    assert_eq!(count_d_setups(&msgs), 0);
    assert_eq!(count_umac_open(&msgs), 0);
    assert!(
        network_group_ready_tuple(&msgs, brew_uuid).is_none(),
        "external-only listener state must not report RF media ready"
    );
}

#[test]
fn test_brew_echo_deaffiliate_does_not_remove_local_rf_affiliation() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    assert!(
        cmce_debug_subscriber_groups_for(&mut test, TEST_ISSI).contains(&TEST_GSSI),
        "test fixture should start with a real MM/RF affiliation"
    );

    submit_subscriber_update_from(
        &mut test,
        TetraEntity::Brew,
        TEST_ISSI,
        vec![TEST_GSSI],
        BrewSubscriberAction::Affiliate,
    );
    test.run_stack(Some(1));
    test.dump_sinks();
    submit_subscriber_update_from(
        &mut test,
        TetraEntity::Brew,
        TEST_ISSI,
        vec![TEST_GSSI],
        BrewSubscriberAction::Deaffiliate,
    );
    test.run_stack(Some(1));
    test.dump_sinks();

    assert!(
        cmce_debug_subscriber_groups_for(&mut test, TEST_ISSI).contains(&TEST_GSSI),
        "Brew echo deaffiliate for the same ISSI must not clear the local RF affiliation"
    );

    let brew_uuid = uuid::Uuid::new_v4();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: TEST_OTHER_ISSI,
            dest_gssi: TEST_GSSI,
            priority: 7,
        }),
    });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    assert_eq!(count_network_call_end(&msgs, brew_uuid), 0);
    assert!(
        count_d_setups(&msgs) >= 1,
        "local RF affiliation must still allow a Brew-origin group call to be set up"
    );
    assert!(count_umac_open(&msgs) >= 1);
}

#[test]
fn test_network_group_speaker_change_updates_dashboard_after_rf_grant() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let (telemetry_sink, telemetry_source) = telemetry_channel();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());

    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.register_entity(CmceBs::new(test.config.clone(), Some(telemetry_sink), None));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew]);
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let first_uuid = uuid::Uuid::new_v4();
    let (call_id, _ts, _start_msgs) = start_network_group_call(&mut test, first_uuid, TEST_CALLED_ISSI, TEST_GSSI, 7);

    let dashboard = DashboardServer::new("test.toml".to_string());
    for event in drain_telemetry(&telemetry_source) {
        dashboard.handle_telemetry(event);
    }

    let second_uuid = uuid::Uuid::new_v4();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid: second_uuid,
            source_issi: TEST_OTHER_ISSI,
            dest_gssi: TEST_GSSI,
            priority: 7,
        }),
    });
    test.run_stack(Some(1));
    let grant_msgs = test.dump_sinks();
    assert!(
        network_group_ready_tuple(&grant_msgs, second_uuid).is_none(),
        "network speaker change must wait until RF D-TX GRANTED is transmitted before Brew ready/dashboard update"
    );

    let grant_reporter = d_tx_granted_reporter(
        &grant_msgs,
        TetraAddress::new(TEST_GSSI, SsiType::Gssi),
        TransmissionGrant::GrantedToOtherUser,
    );
    grant_reporter.mark_transmitted();
    test.run_stack(Some(1));
    let ready_msgs = test.dump_sinks();
    assert!(
        network_group_ready_tuple(&ready_msgs, second_uuid).is_some(),
        "network speaker change should notify Brew ready after RF D-TX GRANTED transmission"
    );

    let events = drain_telemetry(&telemetry_source);
    assert!(
        events.iter().any(|event| matches!(
            event,
            TelemetryEvent::GroupCallSpeakerChanged {
                call_id: changed_call_id,
                gssi,
                speaker_issi,
            } if *changed_call_id == call_id && *gssi == TEST_GSSI && *speaker_issi == TEST_OTHER_ISSI
        )),
        "dashboard telemetry must publish the Brew network speaker change"
    );
    for event in events {
        dashboard.handle_telemetry(event);
    }

    let state = dashboard.state.read().unwrap();
    let call = state
        .snapshot_calls()
        .into_iter()
        .find(|call| call.call_id == call_id)
        .expect("dashboard should retain the reused network group call");
    assert_eq!(call.active_speaker, Some(TEST_OTHER_ISSI));
    assert_eq!(call.caller_issi, TEST_OTHER_ISSI);
}

#[test]
fn test_network_group_preemptive_start_default_off_rejects_without_setup() {
    debug::setup_logging_verbose();

    // EN 300 392-2 table 14.46 makes call priorities 12..=15 pre-emptive;
    // clause 14.5.2.2.1 f) permits interruption only when supported by the
    // SwMI. With the default-off config, do not downgrade the request into a
    // normal group call.
    for priority in 12..=15 {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
        config.brew = Some(test_brew_config());
        let mut test = ComponentTest::from_config(config, Some(dltime));
        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

        let brew_uuid = uuid::Uuid::new_v4();
        test.submit_message(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
                brew_uuid,
                source_issi: TEST_CALLED_ISSI,
                dest_gssi: TEST_GSSI,
                priority,
            }),
        });
        test.run_stack(Some(1));
        let msgs = test.dump_sinks();

        assert_eq!(count_network_call_end(&msgs, brew_uuid), 1, "priority {priority}");
        assert_eq!(count_d_setups(&msgs), 0, "priority {priority}");
        assert_eq!(count_umac_open(&msgs), 0, "priority {priority}");
        assert!(
            msgs.iter().all(|msg| {
                !matches!(
                    &msg.msg,
                    SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                        brew_uuid: ready_uuid,
                        ..
                    }) if *ready_uuid == brew_uuid
                )
            }),
            "default-off pre-emptive network group setup must not report ready for priority {priority}"
        );
    }
}

#[test]
fn test_network_group_call_end_from_active_network_speaker_enters_hangtime_without_release() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());

    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let brew_uuid = uuid::Uuid::new_v4();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: TEST_CALLED_ISSI,
            dest_gssi: TEST_GSSI,
            priority: 7,
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let (call_id, ts, _ready_msgs) = transmit_network_group_setup_and_drain(&mut test, &setup_msgs, brew_uuid);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
    });
    test.run_stack(Some(1));
    let end_msgs = test.dump_sinks();

    // Brew GROUP_IDLE means the network speaker ceased transmitting. EN 300
    // 392-2 clause 14.5.2.2.1 uses D-TX-CEASED for floor release; clause
    // 14.5.2.3 D-RELEASE is reserved for actual group-call teardown.
    let ceased = end_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_ceased(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .expect("active network speaker end should emit D-TX-CEASED");
    assert_eq!(ceased.0.main_address.ssi, TEST_GSSI);
    assert_eq!(ceased.0.main_address.ssi_type, SsiType::Gssi);
    assert!(ceased.0.stealing_permission);
    assert_eq!(ceased.1.call_identifier, call_id);
    assert!(!ceased.1.transmission_request_permission);
    assert_eq!(count_d_releases(&end_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&end_msgs), 0);
    assert!(end_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorReleased {
                call_id: released_call_id,
                ts: released_ts,
            }) if *released_call_id == call_id && *released_ts == ts
        )
    }));

    test.submit_message(build_u_tx_demand_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&demand_msgs), 0);
    assert!(count_d_tx_granted(&demand_msgs) >= 1);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
    let _activation_msgs = transmit_positive_group_grant_and_assert_floor(
        &mut test,
        &demand_msgs,
        TetraAddress::issi(TEST_ISSI),
        call_id,
        TEST_ISSI,
        TEST_GSSI,
        ts,
    );
}

#[test]
fn test_network_group_call_end_before_rf_ready_releases_without_false_floor_idle() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());

    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let brew_uuid = uuid::Uuid::new_v4();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: TEST_CALLED_ISSI,
            dest_gssi: TEST_GSSI,
            priority: 7,
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    assert!(
        network_group_ready_tuple(&setup_msgs, brew_uuid).is_none(),
        "Brew media must not be ready until RF D-SETUP transmission is reported"
    );
    let call_id = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| pdu.call_identifier),
            _ => None,
        })
        .expect("network-origin group setup should emit D-SETUP");

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
    });
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();

    assert_eq!(network_group_ready_tuple(&release_msgs, brew_uuid), None);
    assert_eq!(
        count_d_tx_ceased(&release_msgs),
        0,
        "not-yet-ready network calls must not emit false floor idle"
    );
    assert_eq!(count_umac_floor_released(&release_msgs), 0);
    assert_eq!(
        count_d_releases(&release_msgs),
        2,
        "early GROUP_IDLE should release the reserved group circuit with FACCH plus MCCH fallback D-RELEASE"
    );
    assert_eq!(count_network_call_end(&release_msgs, brew_uuid), 0);

    let releases: Vec<_> = release_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim),
            _ => None,
        })
        .collect();
    assert!(releases.iter().all(|pdu| pdu.call_identifier == call_id));
    assert!(
        releases
            .iter()
            .all(|pdu| pdu.disconnect_cause == DisconnectCause::SwmiRequestedDisconnection)
    );

    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 1, "Only FACCH D-RELEASE should be reporter-tracked");
    reporters[0].mark_transmitted();

    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(count_umac_call_ended_or_close(&closed_msgs) >= 2);
    assert_eq!(count_network_call_end(&closed_msgs, brew_uuid), 1);
    assert!(!cmce_debug_active_call_ids(&mut test).contains(&call_id));
}

#[test]
fn test_network_group_hangtime_release_waits_for_reporter_before_network_call_end() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    config.cell.hangtime_secs = 0;

    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let brew_uuid = uuid::Uuid::new_v4();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: TEST_CALLED_ISSI,
            dest_gssi: TEST_GSSI,
            priority: 7,
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let (call_id, _ts, _ready_msgs) = transmit_network_group_setup_and_drain(&mut test, &setup_msgs, brew_uuid);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
    });
    test.run_stack(Some(1));
    let end_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&end_msgs), 0);
    assert_eq!(count_network_call_end(&end_msgs, brew_uuid), 0);

    test.run_stack(Some(2));
    let mut release_msgs = test.dump_sinks();
    let releases: Vec<_> = release_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(releases.len(), 2, "hangtime expiry should emit FACCH D-RELEASE plus MCCH fallback");
    for (_, release) in &releases {
        assert_eq!(release.call_identifier, call_id);
        assert_eq!(release.disconnect_cause, DisconnectCause::ExpiryOfTimer);
    }
    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 1, "Only FACCH D-RELEASE should be reporter-tracked");

    test.run_stack(Some(2));
    let duplicate_timer_msgs = test.dump_sinks();
    assert_eq!(
        count_d_releases(&duplicate_timer_msgs),
        0,
        "pending hangtime release must not resend D-RELEASE on every CMCE timer tick"
    );
    assert_eq!(
        count_network_call_end(&duplicate_timer_msgs, brew_uuid),
        0,
        "pending hangtime release must wait for reporter completion before notifying Brew"
    );

    // EN 300 392-2 clauses 14.5.2.3.2 and 14.5.2.3.3: SwMI sends
    // D-RELEASE to the group, receives no MS response, and subsequently
    // releases the call. The local circuit/Brew cleanup must wait until the
    // D-RELEASE delivery is reported or the guard expires.
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);
    assert_eq!(count_network_call_end(&release_msgs, brew_uuid), 0);

    reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "reporter completion should close the assigned group circuit"
    );
    assert_eq!(count_network_call_end(&closed_msgs, brew_uuid), 1);
}

#[test]
fn test_network_group_duplicate_network_call_end_during_hangtime_is_ignored() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    config.cell.hangtime_secs = 30;

    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let brew_uuid = uuid::Uuid::new_v4();
    let (call_id, ts, setup_msgs) = start_network_group_call(&mut test, brew_uuid, TEST_CALLED_ISSI, TEST_GSSI, 7);
    assert_eq!(count_network_call_end(&setup_msgs, brew_uuid), 0);

    test.submit_message(build_network_call_end_msg(brew_uuid));
    test.run_stack(Some(1));
    let end_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&end_msgs), 0);
    assert_eq!(count_network_call_end(&end_msgs, brew_uuid), 0);
    assert!(end_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorReleased {
                call_id: released_call_id,
                ts: released_ts,
            }) if *released_call_id == call_id && *released_ts == ts
        )
    }));

    test.submit_message(build_network_call_end_msg(brew_uuid));
    test.run_stack(Some(1));
    let duplicate_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 14.5.2.2.1 and 14.5.2.3.2 separate floor release
    // from call release. Once the network speaker has ceased and the call is in
    // hangtime, a duplicate external end for the same speaker must not create a
    // second D-TX-CEASED, D-RELEASE, or Brew end notification.
    assert_eq!(count_d_tx_ceased(&duplicate_msgs), 0);
    assert_eq!(count_d_releases(&duplicate_msgs), 0);
    assert_eq!(count_network_call_end(&duplicate_msgs, brew_uuid), 0);
    assert_eq!(count_umac_call_ended_or_close(&duplicate_msgs), 0);
}

#[test]
fn test_network_group_local_retake_after_network_end_does_not_transfer_call_ownership() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    config.cell.hangtime_secs = 30;

    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let brew_uuid = uuid::Uuid::new_v4();
    let (call_id, ts, setup_msgs) = start_network_group_call(&mut test, brew_uuid, TEST_CALLED_ISSI, TEST_GSSI, 7);
    assert_eq!(count_network_call_end(&setup_msgs, brew_uuid), 0);

    test.submit_message(build_network_call_end_msg(brew_uuid));
    test.run_stack(Some(1));
    let end_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&end_msgs), 0);
    assert_eq!(count_network_call_end(&end_msgs, brew_uuid), 0);

    test.submit_message(build_u_tx_demand_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert!(count_d_tx_granted(&demand_msgs) >= 1);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
    let activation_msgs = transmit_positive_group_grant_and_assert_floor(
        &mut test,
        &demand_msgs,
        TetraAddress::issi(TEST_ISSI),
        call_id,
        TEST_ISSI,
        TEST_GSSI,
        ts,
    );
    let loopback_update_pos = activation_msgs
        .iter()
        .position(|msg| {
            matches!(
                &msg.msg,
                SapMsgInner::CmceCallControl(CallControl::SetDlMediaSource {
                    ts: update_ts,
                    dl_media_source: CircuitDlMediaSource::LocalLoopback,
                }) if *update_ts == ts
            )
        })
        .expect("local retake after a network speaker should switch the bearer back to local loopback");
    let floor_granted_pos = activation_msgs
        .iter()
        .position(|msg| matches!(&msg.msg, SapMsgInner::CmceCallControl(CallControl::FloorGranted { .. })))
        .expect("local retake should grant the RF floor");
    assert!(
        loopback_update_pos < floor_granted_pos,
        "UMAC must switch to local loopback before FloorGranted opens local group U-plane"
    );
    assert_eq!(count_network_call_end(&demand_msgs, brew_uuid), 0);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let release_msgs = test.dump_sinks();
    let releases: Vec<_> = release_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        releases.len(),
        1,
        "non-owner U-DISCONNECT should receive one direct D-RELEASE rejection"
    );
    let (release_prim, release) = &releases[0];
    assert_eq!(release_prim.main_address.ssi, TEST_ISSI);
    assert_eq!(release_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(release.call_identifier, call_id);
    assert_eq!(release.disconnect_cause, DisconnectCause::RequestedServiceNotAvailable);

    // EN 300 392-2 clauses 14.5.2.3.1 and 14.5.2.7: a local MS that is granted
    // the floor after a network-origin speaker ceases does not become call
    // owner unless SwMI explicitly transfers ownership with D-INFO.
    assert_eq!(count_network_call_end(&release_msgs, brew_uuid), 0);
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);
    assert_eq!(count_d_releases(&release_msgs), 1);
}

#[test]
fn test_network_group_no_listener_release_reports_network_end_once_after_d_release_delivery() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());

    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let brew_uuid = uuid::Uuid::new_v4();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: TEST_CALLED_ISSI,
            dest_gssi: TEST_GSSI,
            priority: 7,
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let (call_id, _ts, _ready_msgs) = transmit_network_group_setup_and_drain(&mut test, &setup_msgs, brew_uuid);
    assert_eq!(count_network_call_end(&setup_msgs, brew_uuid), 0);

    test.submit_message(build_mm_deaffiliate_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();
    let releases: Vec<_> = release_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        releases.len(),
        2,
        "last-listener departure should emit FACCH D-RELEASE plus MCCH fallback"
    );
    for (_, release) in &releases {
        assert_eq!(release.call_identifier, call_id);
        assert_eq!(release.disconnect_cause, DisconnectCause::SwmiRequestedDisconnection);
    }
    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 1, "Only FACCH D-RELEASE should be reporter-tracked");

    // EN 300 392-2 clauses 14.5.2.2.7 and 14.5.2.3.2: external-to-group
    // calls use group-call signalling, and SwMI sends D-RELEASE before it
    // subsequently releases the call. The external NetworkCallEnd therefore
    // must not be emitted before D-RELEASE delivery is reported.
    assert_eq!(count_network_call_end(&release_msgs, brew_uuid), 0);
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);

    test.submit_message(build_network_call_end_msg(brew_uuid));
    test.run_stack(Some(1));
    let duplicate_pending_msgs = test.dump_sinks();
    assert_eq!(
        count_d_releases(&duplicate_pending_msgs),
        0,
        "duplicate NetworkCallEnd while group release is pending must not repeat D-RELEASE"
    );
    assert_eq!(count_network_call_end(&duplicate_pending_msgs, brew_uuid), 0);
    assert_eq!(count_umac_call_ended_or_close(&duplicate_pending_msgs), 0);

    reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "reporter completion should close the assigned group circuit"
    );
    assert_eq!(count_network_call_end(&closed_msgs, brew_uuid), 1);

    test.run_stack(Some(1));
    let extra_msgs = test.dump_sinks();
    assert_eq!(
        count_network_call_end(&extra_msgs, brew_uuid),
        0,
        "no duplicate NetworkCallEnd after pending release is complete"
    );
}

#[test]
fn test_network_group_call_start_reports_end_when_no_traffic_circuit_is_free() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(CfgBrew {
        host: "test-brew.local".to_string(),
        port: 443,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: std::time::Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        feature_rssi_export: false,
        whitelisted_ssis: None,
    });

    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    {
        let mut state = test.config.state_write();
        for ts in 2..=4 {
            state
                .timeslot_alloc
                .reserve(TimeslotOwner::Brew, ts)
                .expect("test should be able to reserve all traffic timeslots");
        }
    }

    let brew_uuid = uuid::Uuid::new_v4();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: TEST_CALLED_ISSI,
            dest_gssi: TEST_GSSI,
            priority: 7,
        }),
    });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.2.2.7 maps external-subscriber-to-group calls
    // onto normal group-call signalling. If no traffic circuit can be
    // allocated, the SwMI must not emit a partial D-SETUP/Open and must close
    // the Brew-side attempt explicitly.
    assert_eq!(count_network_call_end(&msgs, brew_uuid), 1);
    assert_eq!(count_d_setups(&msgs), 0);
    assert_eq!(count_umac_open(&msgs), 0);
    assert!(
        msgs.iter().all(|msg| {
            !matches!(
                &msg.msg,
                SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                    brew_uuid: ready_uuid,
                    ..
                }) if *ready_uuid == brew_uuid
            )
        }),
        "network-origin group call without a traffic circuit must not report ready"
    );
}

#[test]
fn test_network_group_preemption_default_off_rejects_without_interrupt_or_grant() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    let _call_id = start_group_call(&mut test);

    let brew_uuid = uuid::Uuid::new_v4();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: TEST_CALLED_ISSI,
            dest_gssi: TEST_GSSI,
            priority: 15,
        }),
    });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.2.2.1 f) is conditional on SwMI support for
    // transmission interruption. The default config does not advertise/enable
    // that support, so an active local group speaker is left untouched.
    assert_eq!(count_network_call_end(&msgs, brew_uuid), 1);
    assert_eq!(count_d_tx_interrupt(&msgs), 0);
    assert_eq!(count_d_tx_granted(&msgs), 0);
    assert_eq!(count_umac_floor_granted(&msgs), 0);
    assert!(
        msgs.iter().all(|msg| {
            !matches!(
                &msg.msg,
                SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                    brew_uuid: ready_uuid,
                    ..
                }) if *ready_uuid == brew_uuid
            )
        }),
        "default-off preemption must not report network floor readiness"
    );
}

#[test]
fn test_network_group_preemption_non_preemptive_priority_rejects_without_interrupt_or_grant() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    config.cell.transmission_interruption_enabled = true;
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    let _call_id = start_group_call(&mut test);

    let brew_uuid = uuid::Uuid::new_v4();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: TEST_CALLED_ISSI,
            dest_gssi: TEST_GSSI,
            priority: 11,
        }),
    });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // EN 300 392-2 table 14.46 reserves 12..=15 for pre-emptive call
    // priorities. Priority 11 is the highest non-pre-emptive priority and
    // must not withdraw an active MS floor.
    assert_eq!(count_network_call_end(&msgs, brew_uuid), 1);
    assert_eq!(count_d_tx_interrupt(&msgs), 0);
    assert_eq!(count_d_tx_granted(&msgs), 0);
    assert_eq!(count_umac_floor_granted(&msgs), 0);
}

#[test]
fn test_network_group_preemption_equal_preemptive_priority_rejects_without_interrupt_or_grant() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    config.cell.transmission_interruption_enabled = true;
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    let mut u_setup = default_group_u_setup(TEST_GSSI);
    u_setup.call_priority = 12;
    let _call_id = start_group_call_with_u_setup(&mut test, u_setup);

    let brew_uuid = uuid::Uuid::new_v4();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: TEST_CALLED_ISSI,
            dest_gssi: TEST_GSSI,
            priority: 12,
        }),
    });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // Clause 14.5.2.2.1 f) permits pre-emptive interruption only for the
    // higher-priority transmission; equal pre-emptive priority leaves the
    // existing local floor intact.
    assert_eq!(count_network_call_end(&msgs, brew_uuid), 1);
    assert_eq!(count_d_tx_interrupt(&msgs), 0);
    assert_eq!(count_d_tx_granted(&msgs), 0);
    assert_eq!(count_umac_floor_granted(&msgs), 0);
}

#[test]
fn test_network_group_preemption_emits_d_tx_interrupt_before_d_tx_granted() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    config.cell.transmission_interruption_enabled = true;
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit(&mut test);

    let brew_uuid = uuid::Uuid::new_v4();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: TEST_CALLED_ISSI,
            dest_gssi: TEST_GSSI,
            priority: 12,
        }),
    });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    let interrupts: Vec<_> = msgs
        .iter()
        .enumerate()
        .filter_map(|(idx, msg)| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_interrupt(prim).map(|pdu| (idx, prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        interrupts.len(),
        2,
        "preempting a local group speaker should send individual and group D-TX-INTERRUPT"
    );

    let individual_interrupt = interrupts
        .iter()
        .find(|(_, prim, _)| prim.main_address.ssi == TEST_ISSI && prim.main_address.ssi_type == SsiType::Issi)
        .expect("expected D-TX-INTERRUPT addressed to the interrupted local speaker");
    assert_eq!(individual_interrupt.2.call_identifier, call_id);

    let group_interrupt = interrupts
        .iter()
        .find(|(_, prim, _)| prim.main_address.ssi == TEST_GSSI && prim.main_address.ssi_type == SsiType::Gssi)
        .expect("expected group-addressed D-TX-INTERRUPT for listeners");
    assert_eq!(group_interrupt.2.call_identifier, call_id);

    for (_, prim, interrupt) in &interrupts {
        assert_eq!(interrupt.call_identifier, call_id);
        assert_eq!(interrupt.transmission_grant, TransmissionGrant::GrantedToOtherUser.into_raw() as u8);
        assert!(!interrupt.transmission_request_permission);
        assert_eq!(interrupt.transmitting_party_type_identifier, Some(1));
        assert_eq!(interrupt.transmitting_party_address_ssi, Some(TEST_CALLED_ISSI as u64));
        assert!(prim.stealing_permission);
        let chan_alloc = prim
            .chan_alloc
            .as_ref()
            .expect("FACCH D-TX-INTERRUPT should carry channel allocation");
        assert_chan_alloc_matches_circuit(chan_alloc, active_ts, active_usage, "group D-TX-INTERRUPT");
        assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Dl);
    }

    let grant = msgs
        .iter()
        .enumerate()
        .find_map(|(idx, msg)| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (idx, prim, pdu)),
            _ => None,
        })
        .expect("expected group D-TX-GRANTED after D-TX-INTERRUPT");
    assert!(
        interrupts.iter().all(|(idx, _, _)| *idx < grant.0),
        "D-TX-INTERRUPT must withdraw the old floor before D-TX-GRANTED advertises the new one"
    );
    assert_eq!(grant.1.main_address.ssi, TEST_GSSI);
    assert_eq!(grant.1.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(grant.2.call_identifier, call_id);
    assert_eq!(grant.2.transmission_grant, TransmissionGrant::GrantedToOtherUser.into_raw() as u8);
    assert_compact_d_tx_granted_facch(grant.1, &grant.2);
    assert!(grant.1.stealing_permission);
    assert_eq!(
        grant
            .1
            .chan_alloc
            .as_ref()
            .expect("FACCH D-TX-GRANTED should carry channel allocation")
            .ul_dl_assigned,
        UlDlAssignment::Dl
    );

    assert_eq!(
        count_umac_floor_granted(&msgs),
        0,
        "network preemption must wait for RF D-TX-GRANTED transmission before U-plane activation"
    );
    assert!(
        network_group_ready_tuple(&msgs, brew_uuid).is_none(),
        "network preemption must not report ready before RF D-TX-GRANTED transmission"
    );
    let grant_reporter = d_tx_granted_reporter(
        &msgs,
        TetraAddress::new(TEST_GSSI, SsiType::Gssi),
        TransmissionGrant::GrantedToOtherUser,
    );
    grant_reporter.mark_transmitted();
    test.run_stack(Some(1));
    let activation_msgs = test.dump_sinks();
    assert_eq!(count_umac_floor_granted(&activation_msgs), 1);
    assert!(activation_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ..
            }) if *got_call_id == call_id && *source_issi == TEST_CALLED_ISSI && *dest_gssi == TEST_GSSI
        )
    }));
    assert_eq!(network_group_ready_tuple(&activation_msgs, brew_uuid), Some((call_id, active_ts)));
    assert_eq!(count_network_call_end(&msgs, brew_uuid), 0);
}

#[test]
fn test_network_origin_private_preemptive_setup_default_off_rejects_without_d_setup() {
    debug::setup_logging_verbose();

    // EN 300 392-2 table 14.46 assigns 12..=15 to pre-emptive call priority.
    // Clause 14.5.1.2.1 f) is conditional on SwMI interruption support; the
    // default-off config must reject instead of silently downgrading to an
    // ordinary individual call.
    for priority in 12..=15 {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
        config.brew = Some(test_brew_config());
        let mut test = ComponentTest::from_config(config, Some(dltime));
        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

        let brew_uuid = uuid::Uuid::new_v4();
        let mut call = default_network_circuit_call(TEST_ISSI, TEST_CALLED_ISSI);
        call.priority = priority;

        test.submit_message(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, call }),
        });
        test.run_stack(Some(1));
        let msgs = test.dump_sinks();

        let rejects: Vec<_> = msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject {
                    brew_uuid: reject_uuid,
                    cause,
                }) if *reject_uuid == brew_uuid => Some(*cause),
                _ => None,
            })
            .collect();
        assert_eq!(
            rejects,
            vec![DisconnectCause::RequestedServiceNotAvailable.into_raw() as u8],
            "priority {priority}"
        );
        assert!(
            msgs.iter().all(|msg| !matches!(
                &msg.msg,
                SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept {
                    brew_uuid: accepted_uuid,
                }) if *accepted_uuid == brew_uuid
            )),
            "default-off pre-emptive network private setup must not be accepted for priority {priority}"
        );
        assert_eq!(count_d_setups(&msgs), 0, "priority {priority}");
        assert_eq!(count_umac_open(&msgs), 0, "priority {priority}");
    }
}

#[test]
fn test_network_origin_private_preemptive_setup_group_interruption_enabled_still_rejects() {
    debug::setup_logging_verbose();

    // EN 300 392-2 clause 14.5.1.2.1 f) makes private-call interruption a
    // supported-SwMI capability. The configured interruption path here is the
    // group-call D-TX-INTERRUPT path, so enabling it must not partially enable
    // private pre-emption.
    for priority in 12..=15 {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
        config.cell.transmission_interruption_enabled = true;
        config.brew = Some(test_brew_config());
        let mut test = ComponentTest::from_config(config, Some(dltime));
        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

        let brew_uuid = uuid::Uuid::new_v4();
        let mut call = default_network_circuit_call(TEST_ISSI, TEST_CALLED_ISSI);
        call.priority = priority;

        test.submit_message(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, call }),
        });
        test.run_stack(Some(1));
        let msgs = test.dump_sinks();

        assert!(
            msgs.iter().any(|msg| matches!(
                &msg.msg,
                SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject {
                    brew_uuid: reject_uuid,
                    cause,
                }) if *reject_uuid == brew_uuid
                    && *cause == DisconnectCause::RequestedServiceNotAvailable.into_raw() as u8
            )),
            "priority {priority}"
        );
        assert_eq!(count_d_setups(&msgs), 0, "priority {priority}");
        assert_eq!(count_umac_open(&msgs), 0, "priority {priority}");
    }
}

#[test]
fn test_network_origin_private_call_preserves_method_and_timeout_fields() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let brew_uuid = uuid::Uuid::new_v4();
    let mut call = default_network_circuit_call(TEST_ISSI, TEST_CALLED_ISSI);
    call.priority = 11;
    call.method = 0;
    call.timeout = CallTimeout::T10m.into_raw() as u8;

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest {
            brew_uuid,
            call: call.clone(),
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();

    let setup = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .expect("network-origin private call should emit D-SETUP to the called MS");
    assert_eq!(setup.0.main_address.ssi, TEST_CALLED_ISSI);
    // EN 300 392-2 tables 14.50 and 14.62: network-origin private setup
    // must preserve the selected call timeout and hook method in D-SETUP.
    // Table 14.46 keeps priority 11 below the pre-emptive 12..=15 range, so
    // it remains a normal private call when interruption support is default-off.
    assert_eq!(setup.1.call_priority, 11);
    assert_eq!(setup.1.call_time_out, CallTimeout::T10m);
    assert!(!setup.1.hook_method_selection);
    assert!(!setup.1.simplex_duplex_selection);
    assert!(setup_msgs.iter().any(|msg| matches!(
        &msg.msg,
        SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { brew_uuid: accepted_uuid })
            if *accepted_uuid == brew_uuid
    )));

    let call_id = setup.1.call_identifier;
    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let connect_request_msgs = test.dump_sinks();
    let connect_request = connect_request_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest {
                brew_uuid: request_uuid,
                call,
            }) if *request_uuid == brew_uuid => Some(call),
            _ => None,
        })
        .expect("called MS U-CONNECT should be forwarded to Brew");
    assert_eq!(connect_request.timeout, CallTimeout::T10m.into_raw() as u8);
    assert_eq!(connect_request.method, 0);
    assert_eq!(connect_request.grant, TransmissionGrant::Granted.into_raw() as u8);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
            brew_uuid,
            grant: TransmissionGrant::Granted.into_raw() as u8,
            permission: 0,
        }),
    });
    test.run_stack(Some(1));
    let confirm_msgs = test.dump_sinks();

    let connect_ack = confirm_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect_acknowledge(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .expect("network connect confirm should emit D-CONNECT-ACKNOWLEDGE to the called MS");
    assert_eq!(connect_ack.0.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(connect_ack.0.layer2service, Layer2Service::Acknowledged);
    assert!(connect_ack.0.tx_reporter.is_some());
    assert_eq!(connect_ack.1.call_identifier, call_id);
    assert_eq!(connect_ack.1.call_time_out, CallTimeout::T10m);
    assert_eq!(connect_ack.1.transmission_grant, TransmissionGrant::GrantedToOtherUser);
    assert!(count_umac_open(&confirm_msgs) >= 1);
    assert_eq!(
        count_umac_floor_granted(&confirm_msgs),
        0,
        "Brew-origin private floor waits until the called D-CONNECT ACK is L2-acknowledged"
    );
    assert_eq!(
        count_network_circuit_media_ready(&confirm_msgs, brew_uuid),
        0,
        "Brew media must wait for local D-CONNECT ACK L2 ACK"
    );

    acknowledge_called_d_connect_ack(&confirm_msgs, TEST_CALLED_ISSI);
    test.run_stack(Some(1));
    let after_ack_msgs = test.dump_sinks();
    assert_eq!(count_umac_floor_granted(&after_ack_msgs), 0);
    assert_eq!(count_network_circuit_media_ready(&after_ack_msgs, brew_uuid), 1);
}

#[test]
fn test_network_origin_private_release_sends_d_release_without_brew_echo() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let brew_uuid = uuid::Uuid::new_v4();
    let call = default_network_circuit_call(TEST_ISSI, TEST_CALLED_ISSI);
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, call }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| pdu.call_identifier),
            _ => None,
        })
        .expect("network-origin private setup should emit D-SETUP");

    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
            brew_uuid,
            grant: TransmissionGrant::Granted.into_raw() as u8,
            permission: 0,
        }),
    });
    test.run_stack(Some(1));
    let confirm_msgs = test.dump_sinks();
    assert!(count_umac_open(&confirm_msgs) >= 1);
    assert_eq!(count_network_circuit_media_ready(&confirm_msgs, brew_uuid), 0);
    acknowledge_called_d_connect_ack(&confirm_msgs, TEST_CALLED_ISSI);
    test.run_stack(Some(1));
    let ready_msgs = test.dump_sinks();
    assert_eq!(count_network_circuit_media_ready(&ready_msgs, brew_uuid), 1);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease {
            brew_uuid,
            cause: DisconnectCause::UserRequestedDisconnection.into_raw() as u8,
        }),
    });
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.3.2: for SwMI/network initiated private
    // release, the SwMI sends D-RELEASE to the MS and then clears local call
    // state. The network/Brew side initiated this release, so CMCE must not
    // echo NetworkCircuitRelease back to the same originator.
    let releases: Vec<_> = release_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        releases.len(),
        1,
        "network-origin private release should send one assigned-channel D-RELEASE to the local MS"
    );
    for (prim, release) in releases {
        assert_eq!(prim.main_address.ssi, TEST_CALLED_ISSI);
        assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
        assert!(
            prim.stealing_permission,
            "established-call D-RELEASE should stay on the assigned channel"
        );
        assert_eq!(release.call_identifier, call_id);
        assert_eq!(release.disconnect_cause, DisconnectCause::UserRequestedDisconnection);
    }
    assert_eq!(
        count_d_disconnects(&release_msgs),
        0,
        "network-initiated release uses D-RELEASE, not D-DISCONNECT"
    );
    assert_eq!(count_network_circuit_release(&release_msgs, brew_uuid), 0);

    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 1, "only FACCH D-RELEASE is reporter-tracked");
    reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 1,
        "local circuit should close after D-RELEASE transmission is reported"
    );
    assert_eq!(
        count_network_circuit_release(&closed_msgs, brew_uuid),
        0,
        "Brew-origin release must not be echoed back after cleanup"
    );
}

#[test]
fn test_local_origin_brew_private_connect_preserves_method_and_timeout_fields() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    let mut test = ComponentTest::from_config(config, Some(dltime));
    {
        let mut state = test.config.state_write();
        state.network_connected = true;
    }
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let remote_issi = 7_000_001;
    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(remote_issi as u64);
    u_setup.hook_method_selection = true;
    u_setup.simplex_duplex_selection = false;

    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let (brew_uuid, mut network_call) = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, call }) => Some((*brew_uuid, call.clone())),
            _ => None,
        })
        .expect("local private U-SETUP should be forwarded to Brew");
    assert_eq!(network_call.method, 1);
    assert_eq!(network_call.duplex, 0);

    network_call.timeout = CallTimeout::T10m.into_raw() as u8;
    network_call.method = 1;
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest {
            brew_uuid,
            call: network_call,
        }),
    });
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

    let connect = connect_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .expect("Brew connect request should emit D-CONNECT to the local caller");
    // EN 300 392-2 tables 14.50 and 14.62: D-CONNECT carries the selected
    // call timeout and hook method; these are independent of simplex/duplex.
    assert_eq!(connect.0.main_address.ssi, TEST_ISSI);
    assert_eq!(connect.0.layer2service, Layer2Service::Acknowledged);
    assert!(connect.0.tx_reporter.is_some());
    assert_eq!(connect.1.call_time_out, CallTimeout::T10m);
    assert!(connect.1.hook_method_selection);
    assert!(!connect.1.simplex_duplex_selection);
    assert!(count_umac_open(&connect_msgs) >= 1);
    assert_eq!(
        count_network_circuit_connect_confirm(&connect_msgs, brew_uuid),
        0,
        "Brew connect confirm must wait for local caller D-CONNECT L2 ACK"
    );
    assert_eq!(
        count_network_circuit_media_ready(&connect_msgs, brew_uuid),
        0,
        "Brew media must wait for local caller D-CONNECT L2 ACK"
    );

    acknowledge_d_connect(&connect_msgs, TEST_ISSI);
    test.run_stack(Some(1));
    let after_ack_msgs = test.dump_sinks();
    assert_eq!(count_network_circuit_connect_confirm(&after_ack_msgs, brew_uuid), 1);
    assert_eq!(count_network_circuit_media_ready(&after_ack_msgs, brew_uuid), 1);
}

#[test]
fn test_local_origin_brew_private_simplex_connect_sets_initial_floor() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    let mut test = ComponentTest::from_config(config, Some(dltime));
    {
        let mut state = test.config.state_write();
        state.network_connected = true;
    }
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let remote_issi = 7_000_101;
    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(remote_issi as u64);
    u_setup.simplex_duplex_selection = false;

    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let (brew_uuid, mut network_call) = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, call }) => Some((*brew_uuid, call.clone())),
            _ => None,
        })
        .expect("local private U-SETUP should be forwarded to Brew");
    network_call.grant = TransmissionGrant::Granted.into_raw() as u8;
    network_call.permission = 0;

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest {
            brew_uuid,
            call: network_call,
        }),
    });
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

    let connect = connect_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .expect("Brew connect request should emit D-CONNECT to the local caller");
    assert_eq!(connect.0.main_address.ssi, TEST_ISSI);
    assert_eq!(connect.0.layer2service, Layer2Service::Acknowledged);
    assert!(connect.0.tx_reporter.is_some());
    assert_eq!(connect.1.transmission_grant, TransmissionGrant::Granted);
    let call_id = connect.1.call_identifier;

    assert_eq!(
        count_umac_floor_granted(&connect_msgs),
        0,
        "Annex D.4-compatible Brew private setup must wait for local D-CONNECT L2 ACK before initial floor"
    );
    assert_eq!(
        count_network_circuit_connect_confirm(&connect_msgs, brew_uuid),
        0,
        "Brew connect confirm must wait for local caller D-CONNECT L2 ACK"
    );
    assert_eq!(
        count_network_circuit_media_ready(&connect_msgs, brew_uuid),
        0,
        "Brew media must wait for local caller D-CONNECT L2 ACK"
    );

    acknowledge_d_connect(&connect_msgs, TEST_ISSI);
    test.run_stack(Some(1));
    let after_ack_msgs = test.dump_sinks();

    assert_eq!(count_umac_floor_granted(&after_ack_msgs), 1);
    assert!(
        after_ack_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ..
            }) if *got_call_id == call_id && *source_issi == TEST_ISSI && *dest_gssi == remote_issi
        )),
        "EN 300 392-2 clause 14.5.1.2.1 plus Annex D.4: Brew-routed simplex D-CONNECT grant seeds the local floor after L2 ACK"
    );
    assert_eq!(count_network_circuit_connect_confirm(&after_ack_msgs, brew_uuid), 1);
    assert_eq!(count_network_circuit_media_ready(&after_ack_msgs, brew_uuid), 1);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_start_msgs = test.dump_sinks();
    assert_eq!(count_umac_floor_released(&ceased_start_msgs), 0);

    test.router
        .set_dl_time(dltime.add_timeslots(PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
    let tail_msgs = test.dump_sinks();
    assert!(
        count_umac_floor_released(&tail_msgs) >= 1,
        "U-TX CEASED from the granted local Brew-private speaker must not be ignored as floor_holder=None"
    );
}

#[test]
fn test_local_origin_brew_private_simplex_ptt_notifies_brew_without_rf_to_external_peer() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    let mut test = ComponentTest::from_config(config, Some(dltime));
    {
        let mut state = test.config.state_write();
        state.network_connected = true;
    }
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let remote_issi = 7_000_103;
    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(remote_issi as u64);
    u_setup.simplex_duplex_selection = false;

    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let (brew_uuid, mut network_call) = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, call }) => Some((*brew_uuid, call.clone())),
            _ => None,
        })
        .expect("local private U-SETUP should be forwarded to Brew");
    network_call.grant = TransmissionGrant::NotGranted.into_raw() as u8;
    network_call.permission = 0;

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest {
            brew_uuid,
            call: network_call,
        }),
    });
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();
    let call_id = connect_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| pdu.call_identifier),
            _ => None,
        })
        .expect("Brew connect request should emit D-CONNECT");
    acknowledge_d_connect(&connect_msgs, TEST_ISSI);
    test.run_stack(Some(1));
    let after_ack_msgs = test.dump_sinks();
    assert_eq!(count_network_circuit_media_ready(&after_ack_msgs, brew_uuid), 1);
    assert_eq!(count_umac_floor_granted(&after_ack_msgs), 0);

    test.submit_message(build_u_tx_demand_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let local_grants = d_tx_granted_to_issi(&demand_msgs, TEST_ISSI);
    assert_eq!(local_grants.len(), 1);
    assert_eq!(local_grants[0].transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_eq!(
        d_tx_granted_to_issi(&demand_msgs, remote_issi).len(),
        0,
        "Brew external peer must not receive RF D-TX GRANTED"
    );
    assert_eq!(count_umac_floor_granted(&demand_msgs), 1);
    assert_eq!(count_brew_floor_granted(&demand_msgs, call_id, TEST_ISSI, remote_issi), 1);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_start_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&ceased_start_msgs), 0);
    test.router
        .set_dl_time(dltime.add_timeslots(PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
    let tail_msgs = test.dump_sinks();

    assert_eq!(count_d_tx_ceased_to_issi(&tail_msgs, TEST_ISSI), 1);
    assert_eq!(
        count_d_tx_ceased_to_issi(&tail_msgs, remote_issi),
        0,
        "Brew external peer must not receive RF D-TX CEASED"
    );
    assert_eq!(count_umac_floor_released(&tail_msgs), 1);
    assert_eq!(count_brew_floor_released(&tail_msgs, call_id), 1);
}

#[test]
fn test_brew_private_simplex_remote_floor_idle_grants_queued_local_ptt() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    let mut test = ComponentTest::from_config(config, Some(dltime));
    {
        let mut state = test.config.state_write();
        state.network_connected = true;
    }
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let remote_issi = 7_000_104;
    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(remote_issi as u64);
    u_setup.simplex_duplex_selection = false;

    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let (brew_uuid, mut network_call) = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, call }) => Some((*brew_uuid, call.clone())),
            _ => None,
        })
        .expect("local private U-SETUP should be forwarded to Brew");
    network_call.grant = TransmissionGrant::NotGranted.into_raw() as u8;
    network_call.permission = 0;

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest {
            brew_uuid,
            call: network_call,
        }),
    });
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();
    let call_id = connect_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| pdu.call_identifier),
            _ => None,
        })
        .expect("Brew connect request should emit D-CONNECT");
    acknowledge_d_connect(&connect_msgs, TEST_ISSI);
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexGranted {
            brew_uuid,
            grant: TransmissionGrant::GrantedToOtherUser.into_raw() as u8,
            permission: 0,
        }),
    });
    test.run_stack(Some(1));
    let remote_grant_msgs = test.dump_sinks();
    let local_listener_grants = d_tx_granted_to_issi(&remote_grant_msgs, TEST_ISSI);
    assert_eq!(local_listener_grants.len(), 1);
    assert_eq!(
        local_listener_grants[0].transmission_grant,
        TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    );
    assert_eq!(d_tx_granted_to_issi(&remote_grant_msgs, remote_issi).len(), 0);
    assert_eq!(count_umac_floor_granted(&remote_grant_msgs), 1);

    test.submit_message(build_u_tx_demand_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let queued_msgs = test.dump_sinks();
    let queued_grants = d_tx_granted_to_issi(&queued_msgs, TEST_ISSI);
    assert_eq!(queued_grants.len(), 1);
    assert_eq!(
        queued_grants[0].transmission_grant,
        TransmissionGrant::RequestQueued.into_raw() as u8
    );

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexIdle {
            brew_uuid,
            grant: TransmissionGrant::NotGranted.into_raw() as u8,
            permission: 0,
        }),
    });
    test.run_stack(Some(1));
    let idle_msgs = test.dump_sinks();
    let local_grants = d_tx_granted_to_issi(&idle_msgs, TEST_ISSI);
    assert_eq!(local_grants.len(), 1);
    assert_eq!(local_grants[0].transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_eq!(count_umac_floor_granted(&idle_msgs), 1);
    assert_eq!(
        count_network_circuit_simplex_granted(&idle_msgs, brew_uuid, TransmissionGrant::GrantedToOtherUser),
        1,
        "queued local floor must be reflected back to Brew as SIMPLEX_GRANTED for the external peer"
    );
}

#[test]
fn test_local_origin_brew_private_d_connect_transmitted_without_l2_ack_does_not_open_media() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    let mut test = ComponentTest::from_config(config, Some(dltime));
    {
        let mut state = test.config.state_write();
        state.network_connected = true;
    }
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let remote_issi = 7_000_102;
    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(remote_issi as u64);
    u_setup.simplex_duplex_selection = false;

    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let (brew_uuid, mut network_call) = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, call }) => Some((*brew_uuid, call.clone())),
            _ => None,
        })
        .expect("local private U-SETUP should be forwarded to Brew");
    network_call.grant = TransmissionGrant::Granted.into_raw() as u8;
    network_call.permission = 0;

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest {
            brew_uuid,
            call: network_call,
        }),
    });
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

    let connect_reporter = d_connect_reporter(&connect_msgs, TEST_ISSI);
    connect_reporter.mark_transmitted();
    test.run_stack(Some(1));
    let after_transmit_only_msgs = test.dump_sinks();

    assert_eq!(
        count_umac_floor_granted(&after_transmit_only_msgs),
        0,
        "Annex D.4/D.5: local D-CONNECT transmission alone must not authorize first simplex traffic"
    );
    assert_eq!(
        count_network_circuit_connect_confirm(&after_transmit_only_msgs, brew_uuid),
        0,
        "Brew connect confirm waits for local D-CONNECT L2 ACK, not only MAC transmission"
    );
    assert_eq!(
        count_network_circuit_media_ready(&after_transmit_only_msgs, brew_uuid),
        0,
        "Brew media waits for local D-CONNECT L2 ACK, not only MAC transmission"
    );
}

#[test]
fn test_network_origin_brew_private_simplex_connect_confirm_grants_external_caller_first() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let brew_uuid = uuid::Uuid::new_v4();
    let mut call = default_network_circuit_call(TEST_ISSI, TEST_CALLED_ISSI);
    call.priority = 11;
    call.duplex = 0;

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest {
            brew_uuid,
            call: call.clone(),
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| pdu.call_identifier),
            _ => None,
        })
        .expect("network-origin private setup should emit D-SETUP");

    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
            brew_uuid,
            grant: TransmissionGrant::Granted.into_raw() as u8,
            permission: 0,
        }),
    });
    test.run_stack(Some(1));
    let confirm_msgs = test.dump_sinks();

    let connect_ack = confirm_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect_acknowledge(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .expect("network connect confirm should emit D-CONNECT-ACKNOWLEDGE");
    assert_eq!(connect_ack.0.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(connect_ack.0.layer2service, Layer2Service::Acknowledged);
    assert!(connect_ack.0.tx_reporter.is_some());
    assert_eq!(connect_ack.1.transmission_grant, TransmissionGrant::GrantedToOtherUser);

    assert_eq!(
        count_umac_floor_granted(&confirm_msgs),
        0,
        "Annex D.4-compatible Brew private setup must wait for local D-CONNECT ACK L2 ACK before initial floor"
    );
    assert_eq!(
        count_network_circuit_media_ready(&confirm_msgs, brew_uuid),
        0,
        "Brew media must wait for local D-CONNECT ACK L2 ACK"
    );

    acknowledge_called_d_connect_ack(&confirm_msgs, TEST_CALLED_ISSI);
    test.run_stack(Some(1));
    let after_ack_msgs = test.dump_sinks();

    assert_eq!(
        count_umac_floor_granted(&after_ack_msgs),
        0,
        "external caller-first grant must not seed local UL floor"
    );
    assert_eq!(count_network_circuit_media_ready(&after_ack_msgs, brew_uuid), 1);

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let queued_msgs = test.dump_sinks();
    let queued_grants = d_tx_granted_to_issi(&queued_msgs, TEST_CALLED_ISSI);
    assert_eq!(queued_grants.len(), 1);
    assert_eq!(
        queued_grants[0].transmission_grant,
        TransmissionGrant::RequestQueued.into_raw() as u8,
        "local PTT while the Brew caller owns the initial floor must queue, not steal floor"
    );
}

#[test]
fn test_network_origin_brew_private_d_connect_ack_transmitted_without_l2_ack_does_not_open_media() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let brew_uuid = uuid::Uuid::new_v4();
    let mut call = default_network_circuit_call(TEST_ISSI, TEST_CALLED_ISSI);
    call.priority = 11;
    call.duplex = 0;

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest {
            brew_uuid,
            call: call.clone(),
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| pdu.call_identifier),
            _ => None,
        })
        .expect("network-origin private setup should emit D-SETUP");

    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
            brew_uuid,
            grant: TransmissionGrant::Granted.into_raw() as u8,
            permission: 0,
        }),
    });
    test.run_stack(Some(1));
    let confirm_msgs = test.dump_sinks();

    let connect_ack_reporter = called_d_connect_ack_reporter(&confirm_msgs, TEST_CALLED_ISSI);
    connect_ack_reporter.mark_transmitted();
    test.run_stack(Some(1));
    let after_transmit_only_msgs = test.dump_sinks();

    assert_eq!(
        count_umac_floor_granted(&after_transmit_only_msgs),
        0,
        "Annex D.4/D.5: local D-CONNECT ACK transmission alone must not authorize first simplex traffic"
    );
    assert_eq!(
        count_network_circuit_media_ready(&after_transmit_only_msgs, brew_uuid),
        0,
        "Brew media waits for local D-CONNECT ACK L2 ACK, not only MAC transmission"
    );
}

/// Test that late-entry D-SETUP re-sends are throttled when the previous
/// D-SETUP's TxReceipt is still in Pending state (UMAC hasn't transmitted it yet),
/// and that they resume once the receipt reaches a final state.
#[test]
fn test_dsetup_late_entry_throttle() {
    debug::setup_logging_verbose();

    // Start at timeslot 1 so circuit creation aligns cleanly with tick_start checks
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    start_group_call(&mut test);

    // Run a few more ticks to get through the D_SETUP_REPEATS backup window.
    // The backup send goes through (receipt is None) and creates a tracked receipt.
    test.run_stack(Some(8));
    let mut backup_msgs = test.dump_sinks();
    let backup_reporters = extract_d_setup_reporters(&mut backup_msgs);

    // We should have at least one reporter from the backup send
    assert!(
        !backup_reporters.is_empty(),
        "Expected backup D-SETUP with tx_reporter in initial window"
    );
    let last_reporter = &backup_reporters[backup_reporters.len() - 1];
    assert_eq!(last_reporter.get_state(), TxState::Pending);

    // Run for 2 full late-entry intervals (720 ticks). With the receipt still Pending,
    // ALL late-entry D-SETUPs should be suppressed.
    test.run_stack(Some(720));
    let throttled_msgs = test.dump_sinks();
    let throttled_count = count_d_setups(&throttled_msgs);
    assert_eq!(
        throttled_count, 0,
        "Late-entry D-SETUPs should be suppressed while receipt is Pending"
    );

    // Now mark the previous D-SETUP as transmitted (simulating UMAC sending it over the air)
    last_reporter.mark_transmitted();

    // Run for 2 more late-entry intervals. Now D-SETUPs should go through.
    test.run_stack(Some(720));
    let mut unthrottled_msgs = test.dump_sinks();
    let unthrottled_count = count_d_setups(&unthrottled_msgs);
    assert!(
        unthrottled_count > 0,
        "Late-entry D-SETUPs should resume once receipt reaches final state"
    );

    // Each re-send that went through should have created a fresh reporter
    let new_reporters = extract_d_setup_reporters(&mut unthrottled_msgs);
    assert_eq!(
        new_reporters.len(),
        unthrottled_count,
        "Each re-sent D-SETUP should carry a fresh tx_reporter"
    );
}

#[test]
fn test_group_setup_sends_proceeding_connect_and_group_setup_with_allocations() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    // EN 300 392-2 clause 14.5.2.1 normal group call setup: the
    // requesting MS receives call proceeding/connect while the group receives
    // D-SETUP on the allocated traffic channel.
    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs);

    let open_circuit = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .expect("group setup should open a UMAC traffic circuit");
    assert_eq!(open_circuit.peer_ts, None);
    assert_eq!(
        open_circuit.active_addr,
        Some(TetraAddress::new(TEST_GSSI, SsiType::Gssi)),
        "group CallControl::Open must identify the GSSI so UMAC can apply EG assigned-channel suspension"
    );
    assert_eq!(
        open_circuit.active_secondary_addrs,
        vec![TetraAddress::issi(TEST_ISSI)],
        "initial group speaker is tracked as a secondary ISSI, while the primary GSSI keeps UMAC group-scoped rather than private/P2P-scoped"
    );
    assert_eq!(
        open_circuit.dl_media_source,
        tetra_saps::control::call_control::CircuitDlMediaSource::LocalLoopback
    );
    assert!(
        (2..=4).contains(&open_circuit.ts),
        "group traffic should use an assignable traffic timeslot"
    );
    assert!(open_circuit.usage > 0, "group traffic should carry a usage marker");

    let floor_grants_to_umac: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id,
                source_issi,
                dest_gssi,
                ts,
            }) if msg.dest == TetraEntity::Umac => Some((*call_id, *source_issi, *dest_gssi, *ts)),
            _ => None,
        })
        .collect();
    assert_eq!(
        floor_grants_to_umac,
        vec![(call_id, TEST_ISSI, TEST_GSSI, open_circuit.ts)],
        "initial group setup must tell UMAC which ISSI owns the first floor because STCH MAC-U-SIGNAL has no SSI field"
    );

    let proceedings: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_call_proceeding(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(proceedings.len(), 1, "Expected one D-CALL-PROCEEDING to the caller");
    let (proceeding_prim, proceeding) = &proceedings[0];
    assert_eq!(proceeding.call_identifier, call_id);
    assert_eq!(proceeding_prim.main_address.ssi, TEST_ISSI);
    assert_eq!(proceeding_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(proceeding_prim.layer2service, Layer2Service::Unacknowledged);
    assert!(proceeding_prim.chan_alloc.is_none());

    let connects: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(connects.len(), 1, "Expected one D-CONNECT to the caller");
    let (connect_prim, connect) = &connects[0];
    assert_eq!(connect.call_identifier, call_id);
    assert_eq!(connect.transmission_grant, TransmissionGrant::Granted);
    assert!(!connect.transmission_request_permission);
    assert!(connect.call_ownership);
    assert_eq!(connect_prim.main_address.ssi, TEST_ISSI);
    assert_eq!(connect_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(connect_prim.layer2service, Layer2Service::Unacknowledged);
    let connect_alloc = connect_prim.chan_alloc.as_ref().expect("D-CONNECT should carry channel allocation");
    assert_eq!(connect_alloc.alloc_type, ChanAllocType::Replace);
    assert_eq!(connect_alloc.usage, Some(open_circuit.usage));
    assert!(connect_alloc.timeslots[(open_circuit.ts - 1) as usize]);
    assert_eq!(connect_alloc.ul_dl_assigned, UlDlAssignment::Both);

    let setups: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(setups.len(), 1, "Expected one GSSI-addressed D-SETUP");
    let (setup_prim, setup) = &setups[0];
    assert_eq!(setup.call_identifier, call_id);
    assert_eq!(setup.calling_party_address_ssi, Some(TEST_ISSI));
    assert_eq!(setup.basic_service_information.communication_type, CommunicationType::P2Mp);
    assert_eq!(setup.basic_service_information.circuit_mode_type, CircuitModeType::TchS);
    assert_eq!(setup.transmission_grant, TransmissionGrant::GrantedToOtherUser);
    assert!(!setup.transmission_request_permission);
    assert_eq!(setup_prim.main_address.ssi, TEST_GSSI);
    assert_eq!(setup_prim.main_address.ssi_type, SsiType::Gssi);
    assert_eq!(setup_prim.layer2service, Layer2Service::Unacknowledged);
    let setup_alloc = setup_prim.chan_alloc.as_ref().expect("D-SETUP should carry channel allocation");
    assert_eq!(setup_alloc.alloc_type, ChanAllocType::Replace);
    assert_eq!(setup_alloc.usage, Some(open_circuit.usage));
    assert!(setup_alloc.timeslots[(open_circuit.ts - 1) as usize]);
    assert_eq!(setup_alloc.ul_dl_assigned, UlDlAssignment::Both);
}

#[test]
fn test_group_u_setup_numeric_collision_routes_to_gssi_not_registered_issi() {
    debug::setup_logging_verbose();

    let collision = TEST_CALLED_ISSI;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, collision);
    register_subscriber(&mut test, collision, TEST_CALLED_GSSI);

    // EN 300 392-2 clause 14.5.2.1 normal group call setup is P2MP. The
    // called-party numeric value is a GSSI in this path even if an ISSI with
    // the same 24-bit value is also registered.
    test.submit_message(build_u_setup_msg(TEST_ISSI, collision));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();

    let open_circuit = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .expect("group U-SETUP should open a GSSI traffic circuit");
    assert_eq!(open_circuit.active_addr, Some(TetraAddress::new(collision, SsiType::Gssi)));
    assert_eq!(
        open_circuit.active_secondary_addrs,
        vec![TetraAddress::issi(TEST_ISSI)],
        "P2MP destination numeric collision must not be added as a private/P2P ISSI participant"
    );

    let setups: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(setups.len(), 1, "group setup should emit one GSSI-addressed D-SETUP");
    let (setup_prim, setup) = &setups[0];
    assert_eq!(setup.basic_service_information.communication_type, CommunicationType::P2Mp);
    assert_eq!(setup_prim.main_address.ssi, collision);
    assert_eq!(setup_prim.main_address.ssi_type, SsiType::Gssi);
    assert_ne!(
        setup_prim.main_address.ssi_type,
        SsiType::Issi,
        "P2MP setup must not be rerouted to the registered ISSI with the same numeric value"
    );
}

#[test]
fn test_repeated_group_u_setup_same_active_gssi_uses_existing_call_without_service_unavailable() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let (active_call_id, active_ts, active_usage) = start_group_call_with_circuit(&mut test);

    // EN 300 392-2 clause 14.5.2.1 covers setup. Once the same GSSI call is
    // already maintained, clause 14.5.2.2.1 floor control applies: a field
    // radio's repeated same-GSSI U-SETUP is treated as a floor request alias,
    // not as a second setup transaction.
    test.submit_message(build_u_setup_msg(TEST_CALLED_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let duplicate_msgs = test.dump_sinks();

    assert_eq!(
        count_d_releases(&duplicate_msgs),
        0,
        "same-GSSI active-call rejoin must not emit D-RELEASE RequestedServiceNotAvailable"
    );
    assert_eq!(
        count_d_call_proceedings(&duplicate_msgs),
        0,
        "active same-GSSI U-SETUP alias must not restart setup with D-CALL PROCEEDING"
    );
    assert_eq!(
        count_d_connects(&duplicate_msgs),
        0,
        "active same-GSSI U-SETUP alias must not resend D-CONNECT"
    );

    let floor_answers: Vec<_> = duplicate_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        floor_answers.len(),
        1,
        "repeated setup from a non-speaker should be handled as one floor-control response"
    );
    let (grant_prim, grant) = &floor_answers[0];
    assert_eq!(grant.call_identifier, active_call_id);
    assert_eq!(grant_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(grant_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(grant.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        grant_prim,
        grant,
        active_ts,
        active_usage,
        UlDlAssignment::Dl,
        "queued repeated U-SETUP floor response",
    );

    assert_eq!(count_d_setups(&duplicate_msgs), 0, "duplicate setup must not send a second D-SETUP");
    assert_eq!(
        count_umac_open(&duplicate_msgs),
        0,
        "duplicate setup must not open a second circuit"
    );
    assert_eq!(
        count_umac_call_ended_or_close(&duplicate_msgs),
        0,
        "repeated setup must not close the active group call"
    );
    assert_eq!(
        count_umac_floor_granted(&duplicate_msgs),
        0,
        "queued repeated setup must not hand off the floor before U-TX CEASED"
    );
}

#[test]
fn test_large_group_repeated_u_setup_floor_alias_is_bounded_without_setup_fanout() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    let member_count = LARGE_GSSI_MEMBER_COUNT;
    let first_issi = 740_000_u32;
    let current_speaker = first_issi;
    let queued_requester = first_issi + 1;
    for offset in 0..member_count {
        let issi = first_issi + offset;
        submit_subscriber_update(&mut test, issi, Vec::new(), BrewSubscriberAction::Register);
        submit_subscriber_update(&mut test, issi, vec![TEST_GSSI], BrewSubscriberAction::Affiliate);
    }
    test.run_stack(Some((member_count as usize * 2) + 16));
    let _ = test.dump_sinks();

    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, current_speaker, TEST_GSSI);

    test.submit_message(build_u_setup_msg(queued_requester, TEST_GSSI));
    test.run_stack(Some(1));
    let first_waiter_msgs = test.dump_sinks();
    let first_waiter_grants: Vec<_> = first_waiter_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(first_waiter_grants.len(), 1);
    assert_eq!(first_waiter_grants[0].0.main_address, TetraAddress::issi(queued_requester));
    assert_eq!(first_waiter_grants[0].1.call_identifier, call_id);
    assert_eq!(
        first_waiter_grants[0].1.transmission_grant,
        TransmissionGrant::RequestQueued.into_raw() as u8
    );
    assert_eq!(count_d_call_proceedings(&first_waiter_msgs), 0);
    assert_eq!(count_d_connects(&first_waiter_msgs), 0);
    assert_eq!(count_d_setups(&first_waiter_msgs), 0);
    assert_eq!(count_d_releases(&first_waiter_msgs), 0);
    assert_eq!(count_umac_open(&first_waiter_msgs), 0);
    assert_eq!(count_umac_floor_granted(&first_waiter_msgs), 0);

    for issi in (first_issi + 2)..(first_issi + member_count) {
        test.submit_message(build_u_setup_msg(issi, TEST_GSSI));
    }
    test.deliver_all_messages();
    let busy_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.2.1 covers group setup. Once the same GSSI
    // call is already maintained, clause 14.5.2.2.1 floor control applies.
    // Treat repeated same-GSSI U-SETUPs as bounded floor requests: do not fan
    // out setup transactions, and keep affiliated contenders in FIFO order.
    assert_eq!(count_d_call_proceedings(&busy_msgs), 0);
    assert_eq!(count_d_connects(&busy_msgs), 0);
    assert_eq!(count_d_setups(&busy_msgs), 0);
    assert_eq!(count_d_releases(&busy_msgs), 0);
    assert_eq!(count_umac_open(&busy_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&busy_msgs), 0);
    assert_eq!(count_umac_floor_granted(&busy_msgs), 0);

    let busy_grants: Vec<_> = busy_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(busy_grants.len(), member_count as usize - 2);
    for (prim, grant) in &busy_grants {
        assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
        assert!(
            prim.main_address.ssi >= first_issi + 2 && prim.main_address.ssi < first_issi + member_count,
            "only same-GSSI repeated setup contenders should receive busy floor responses"
        );
        assert_eq!(grant.call_identifier, call_id);
        assert_eq!(
            grant.transmission_grant,
            TransmissionGrant::RequestQueued.into_raw() as u8,
            "affiliated repeated U-SETUP aliases inside the bounded FIFO should wait their turn"
        );
        assert_d_tx_granted_facch_allocation(
            prim,
            grant,
            active_ts,
            active_usage,
            UlDlAssignment::Dl,
            "large group repeated U-SETUP busy floor requester",
        );
    }

    test.submit_message(build_u_tx_ceased_msg(current_speaker, call_id));
    test.run_stack(Some(1));
    let handoff_msgs = test.dump_sinks();
    let handoff_grants: Vec<_> = handoff_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(handoff_grants.len(), 2);
    let queued_handoff = handoff_grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(queued_requester))
        .expect("the first repeated U-SETUP alias should retain the queued floor");
    assert_eq!(queued_handoff.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert!(
        handoff_grants.iter().all(|(prim, _)| {
            prim.main_address.ssi == queued_requester || (prim.main_address.ssi == TEST_GSSI && prim.main_address.ssi_type == SsiType::Gssi)
        }),
        "later repeated U-SETUP contenders must not jump ahead of the first queued floor requester"
    );
    assert_eq!(count_umac_floor_granted(&handoff_msgs), 0);
    assert_eq!(count_d_releases(&handoff_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&handoff_msgs), 0);

    let second_queued_requester = first_issi + 2;
    test.submit_message(build_u_tx_ceased_msg(queued_requester, call_id));
    test.run_stack(Some(1));
    let second_handoff_msgs = test.dump_sinks();
    let second_handoff_grants: Vec<_> = second_handoff_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    let second_handoff = second_handoff_grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(second_queued_requester))
        .expect("FIFO should hand the next repeated U-SETUP alias the floor");
    assert_eq!(second_handoff.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert!(
        second_handoff_grants.iter().all(|(prim, _)| {
            prim.main_address.ssi == second_queued_requester
                || (prim.main_address.ssi == TEST_GSSI && prim.main_address.ssi_type == SsiType::Gssi)
        }),
        "second FIFO handoff must still produce only requester and GSSI listener grants"
    );
    assert_eq!(count_umac_floor_granted(&second_handoff_msgs), 0);
    assert_eq!(count_d_releases(&second_handoff_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&second_handoff_msgs), 0);
}

#[test]
fn test_repeated_group_u_setup_from_current_speaker_answers_setup_reentry() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let (active_call_id, active_ts, active_usage) = start_group_call_with_circuit(&mut test);

    // Some Motorola-class terminals repeat U-SETUP when the user presses PTT
    // again, even though the SwMI still has that MS as current speaker. Treat
    // that as setup re-entry on the maintained call so the MS receives the
    // D-CONNECT setup response it expects; listener floor state still uses
    // normal maintenance signalling.
    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let repeated_msgs = test.dump_sinks();

    assert_eq!(count_d_releases(&repeated_msgs), 0);
    assert_eq!(count_d_setups(&repeated_msgs), 0);
    assert_eq!(count_d_call_proceedings(&repeated_msgs), 1);
    assert_eq!(count_d_connects(&repeated_msgs), 1);
    assert_eq!(count_umac_open(&repeated_msgs), 0);

    let connects: Vec<_> = repeated_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    let (connect_prim, connect) = &connects[0];
    assert_eq!(connect.call_identifier, active_call_id);
    assert_eq!(connect.transmission_grant, TransmissionGrant::Granted);
    assert_eq!(connect_prim.main_address, TetraAddress::issi(TEST_ISSI));
    let connect_alloc = connect_prim
        .chan_alloc
        .as_ref()
        .expect("current-speaker setup re-entry D-CONNECT should carry channel allocation");
    assert_chan_alloc_matches_circuit(connect_alloc, active_ts, active_usage, "current-speaker setup re-entry D-CONNECT");
    assert_eq!(connect_alloc.ul_dl_assigned, UlDlAssignment::Both);

    let grants: Vec<_> = repeated_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        grants.len(),
        1,
        "D-CONNECT grants the current speaker; only local listeners need D-TX GRANTED"
    );
    let (listener_prim, listener_grant) = &grants[0];
    assert_eq!(listener_prim.main_address, TetraAddress::issi(TEST_CALLED_ISSI));
    assert_eq!(listener_grant.call_identifier, active_call_id);
    assert_eq!(
        listener_grant.transmission_grant,
        TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    );
    for (prim, grant) in &grants {
        assert_d_tx_granted_facch_allocation(
            prim,
            grant,
            active_ts,
            active_usage,
            UlDlAssignment::Dl,
            "current-speaker setup re-entry listener floor update",
        );
    }

    assert_eq!(
        count_umac_floor_granted(&repeated_msgs),
        1,
        "current-speaker setup re-entry refreshes the existing UMAC floor"
    );
    assert_eq!(count_umac_call_ended_or_close(&repeated_msgs), 0);
}

#[test]
fn test_repeated_group_u_setup_same_gssi_during_hangtime_answers_setup_reentry() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let (active_call_id, active_ts, active_usage) = start_group_call_with_circuit(&mut test);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, active_call_id));
    test.run_stack(Some(1));
    let _hangtime_msgs = test.dump_sinks();
    drain_group_tx_ceased_tail(&mut test, dltime);
    let _tail_msgs = test.dump_sinks();

    // Nexus-BS hangtime is local call-retention between transmissions. A
    // terminal that sends U-TX DEMAND is handled as in-call floor control, but
    // a Motorola-class terminal may send a fresh U-SETUP after leaving the
    // maintained context. EN 300 392-2 clauses 14.5.2.1.2 and 14.7.2.10 make
    // that a setup primitive, so answer with setup-phase PDUs while reusing the
    // existing call id/circuit.
    test.submit_message(build_u_setup_msg(TEST_CALLED_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let repeated_msgs = test.dump_sinks();

    assert_eq!(
        count_d_releases(&repeated_msgs),
        0,
        "same-GSSI hangtime rejoin must not emit D-RELEASE RequestedServiceNotAvailable"
    );
    assert_eq!(
        count_d_call_proceedings(&repeated_msgs),
        1,
        "hangtime U-SETUP re-entry must answer the setup primitive"
    );
    assert_eq!(count_d_connects(&repeated_msgs), 1, "hangtime U-SETUP re-entry must send D-CONNECT");

    let proceedings: Vec<_> = repeated_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_call_proceeding(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    let (proceeding_prim, proceeding) = &proceedings[0];
    assert_eq!(proceeding.call_identifier, active_call_id);
    assert_eq!(proceeding_prim.main_address, TetraAddress::issi(TEST_CALLED_ISSI));
    assert!(proceeding_prim.chan_alloc.is_none());

    let connects: Vec<_> = repeated_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    let (connect_prim, connect) = &connects[0];
    assert_eq!(connect.call_identifier, active_call_id);
    assert_eq!(connect.transmission_grant, TransmissionGrant::Granted);
    assert!(!connect.transmission_request_permission);
    assert_eq!(connect_prim.main_address, TetraAddress::issi(TEST_CALLED_ISSI));
    assert_eq!(connect_prim.layer2service, Layer2Service::Unacknowledged);
    let connect_alloc = connect_prim
        .chan_alloc
        .as_ref()
        .expect("maintained group D-CONNECT should carry the existing channel allocation");
    assert_chan_alloc_matches_circuit(connect_alloc, active_ts, active_usage, "hangtime U-SETUP re-entry D-CONNECT");
    assert_eq!(connect_alloc.ul_dl_assigned, UlDlAssignment::Both);

    let grants: Vec<_> = repeated_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        grants.len(),
        1,
        "D-CONNECT grants the requester; only listeners need maintenance floor signalling"
    );
    let (listener_prim, listener_grant) = &grants[0];
    assert_eq!(listener_prim.main_address, TetraAddress::issi(TEST_ISSI));
    assert_eq!(listener_grant.call_identifier, active_call_id);
    assert_eq!(
        listener_grant.transmission_grant,
        TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    );
    for (prim, grant) in &grants {
        assert_d_tx_granted_facch_allocation(
            prim,
            grant,
            active_ts,
            active_usage,
            UlDlAssignment::Dl,
            "hangtime U-SETUP re-entry listener floor update",
        );
    }

    assert_eq!(
        count_d_setups(&repeated_msgs),
        0,
        "hangtime retake must not inject an immediate back-up D-SETUP over the first speech frames"
    );
    assert_eq!(count_umac_open(&repeated_msgs), 0, "hangtime retake must not open a second circuit");
    assert_eq!(
        count_umac_call_ended_or_close(&repeated_msgs),
        0,
        "hangtime retake must not close the maintained group call"
    );
    assert_eq!(
        count_umac_floor_granted(&repeated_msgs),
        1,
        "D-CONNECT setup re-entry grants the requester and reopens the existing U-plane floor"
    );
    let floor_grants_to_umac: Vec<_> = repeated_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id,
                source_issi,
                dest_gssi,
                ts,
            }) if msg.dest == TetraEntity::Umac => Some((*call_id, *source_issi, *dest_gssi, *ts)),
            _ => None,
        })
        .collect();
    assert_eq!(floor_grants_to_umac, vec![(active_call_id, TEST_CALLED_ISSI, TEST_GSSI, active_ts)]);

    run_group_late_entry_resend_tick(&mut test, dltime);
    let backup_msgs = test.dump_sinks();
    let setup_refresh = backup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .next()
        .expect("deferred back-up D-SETUP should still advertise the maintained call");
    assert_eq!(setup_refresh.1.call_identifier, active_call_id);
    assert_eq!(setup_refresh.1.calling_party_address_ssi, Some(TEST_CALLED_ISSI));
    assert_eq!(setup_refresh.0.main_address, TetraAddress::new(TEST_GSSI, SsiType::Gssi));
}

#[test]
fn test_group_u_setup_same_gssi_during_pending_release_starts_fresh_call() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let (old_call_id, old_ts, old_usage) = start_group_call_with_circuit(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, old_call_id));
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();
    let old_release_reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(old_release_reporters.len(), 1, "old group D-RELEASE should be pending on FACCH");
    assert_eq!(old_release_reporters[0].get_state(), TxState::Pending);
    assert_eq!(
        count_umac_call_ended_or_close(&release_msgs),
        0,
        "old release must still be in the pending-release guard before replacement setup"
    );

    // EN 300 392-2 clause 14.5.2.3 covers release of the old group call, and
    // clause 14.5.2.1 covers the next normal group setup. A stale local
    // D-RELEASE delivery guard for the same GSSI must not turn the first
    // replacement U-SETUP into RequestedServiceNotAvailable.
    test.submit_message(build_u_setup_msg(TEST_CALLED_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let replacement_msgs = test.dump_sinks();

    assert_eq!(
        count_d_releases(&replacement_msgs),
        0,
        "same-GSSI replacement setup during pending release must not emit service-unavailable D-RELEASE"
    );
    assert_eq!(count_d_call_proceedings(&replacement_msgs), 1);
    assert_eq!(count_d_connects(&replacement_msgs), 1);
    assert_eq!(count_d_setups(&replacement_msgs), 1);
    assert_eq!(
        count_umac_open(&replacement_msgs),
        1,
        "replacement setup should open a fresh group circuit"
    );
    assert_eq!(
        count_umac_call_ended_or_close(&replacement_msgs),
        0,
        "stale pending release must stay on its old circuit while replacement setup starts"
    );
    let replacement_circuit = replacement_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .expect("replacement setup should open a fresh group circuit");
    assert_ne!(
        replacement_circuit.ts, old_ts,
        "replacement call must not reuse the old pending-release traffic slot"
    );
    assert_ne!(
        replacement_circuit.usage, old_usage,
        "replacement call must not reuse the old pending-release usage marker"
    );

    let new_call_id = first_d_setup_call_id(&replacement_msgs);
    assert_ne!(new_call_id, old_call_id, "replacement setup should allocate a fresh group call id");

    let replacement_setup = replacement_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .expect("replacement setup should emit D-SETUP");
    assert_eq!(replacement_setup.0.main_address, TetraAddress::new(TEST_GSSI, SsiType::Gssi));
    assert_eq!(replacement_setup.1.call_identifier, new_call_id);
    assert_eq!(replacement_setup.1.calling_party_address_ssi, Some(TEST_CALLED_ISSI));

    old_release_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let old_release_closed_msgs = test.dump_sinks();
    assert!(
        old_release_closed_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::Close(Direction::Both, ts)) if *ts == old_ts
        )),
        "old D-RELEASE reporter completion should close the old traffic slot"
    );
    assert!(
        old_release_closed_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::CallEnded { call_id, ts }) if *call_id == old_call_id && *ts == old_ts
        )),
        "old D-RELEASE reporter completion should end only the old call"
    );
    assert!(
        !old_release_closed_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::CallEnded { call_id, .. }) if *call_id == new_call_id
        )),
        "old D-RELEASE reporter completion must not close the fresh replacement call"
    );
    assert_eq!(count_d_releases(&old_release_closed_msgs), 0);
}

#[test]
fn test_group_preemptive_u_setup_default_off_rejects_without_circuit() {
    debug::setup_logging_verbose();

    // EN 300 392-2 table 14.46 defines call priorities 12..=15 as
    // pre-emptive. Clause 14.5.2.2.1 f) allows transmission interruption only
    // when supported, so the default-off SwMI rejects before call-id
    // allocation instead of starting a normal group call.
    for priority in 12..=15 {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);

        let mut u_setup = default_group_u_setup(TEST_GSSI);
        u_setup.call_priority = priority;
        test.submit_message(build_u_setup_custom_msg(TEST_ISSI, u_setup));
        test.run_stack(Some(1));
        let msgs = test.dump_sinks();

        let releases: Vec<_> = msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(
            releases.len(),
            1,
            "pre-emptive group U-SETUP should receive one D-RELEASE for priority {priority}"
        );
        let (release_prim, release) = &releases[0];
        assert_eq!(release.call_identifier, 0, "priority {priority}");
        assert_eq!(
            release.disconnect_cause,
            DisconnectCause::RequestedServiceNotAvailable,
            "priority {priority}"
        );
        assert_eq!(release_prim.main_address.ssi, TEST_ISSI, "priority {priority}");
        assert_eq!(release_prim.main_address.ssi_type, SsiType::Issi, "priority {priority}");
        assert_eq!(count_d_setups(&msgs), 0, "priority {priority}");
        assert_eq!(count_umac_open(&msgs), 0, "priority {priority}");
    }
}

#[test]
fn test_group_priority_11_default_off_starts_call_setup() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);

    let mut u_setup = default_group_u_setup(TEST_GSSI);
    u_setup.call_priority = 11;

    // EN 300 392-2 table 14.46 keeps priority 11 below the pre-emptive
    // 12..=15 range. It must still form a normal group setup with the
    // default-off interruption guard.
    test.submit_message(build_u_setup_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    let setup = msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim),
            _ => None,
        })
        .expect("priority 11 group U-SETUP should emit D-SETUP");
    assert_eq!(setup.call_priority, 11);
    assert_eq!(count_d_releases(&msgs), 0);
    assert!(count_d_setups(&msgs) >= 1);
    assert!(count_umac_open(&msgs) >= 1);
}

#[test]
fn test_u_setup_with_unsupported_feature_rejects_with_dummy_call_id() {
    debug::setup_logging_verbose();

    for area_selection in [1, 2] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
        test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);

        let mut u_setup = default_p2p_u_setup();
        u_setup.area_selection = area_selection;

        test.submit_message(build_u_setup_custom_msg(TEST_ISSI, u_setup));
        test.run_stack(Some(1));
        let msgs = test.dump_sinks();

        // EN 300 392-2 table 14.30 marks area selection as SS-AS, and
        // clause 14.8.1/table 14.34 defines only value 0 as area not defined.
        // This SwMI does not implement SS-AS, so non-zero area selection is an
        // unsupported setup feature. As first response before SwMI call
        // identity allocation, clause 14.5.1.1.2 uses the dummy reference.
        let releases: Vec<_> = msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(
            releases.len(),
            1,
            "unsupported setup area_selection={area_selection} should receive one D-RELEASE"
        );
        let (release_prim, release) = &releases[0];
        assert_eq!(release.call_identifier, 0);
        assert_eq!(release.disconnect_cause, DisconnectCause::IncompatibleTrafficCase);
        assert_eq!(release_prim.main_address.ssi, TEST_ISSI);
        assert_eq!(release_prim.main_address.ssi_type, SsiType::Issi);
        assert_eq!(count_d_setups(&msgs), 0);
        assert_eq!(count_umac_open(&msgs), 0);
    }
}

#[test]
fn test_u_setup_with_unsupported_optional_features_rejects_with_dummy_call_id() {
    debug::setup_logging_verbose();

    for unsupported in ["clir_control", "facility", "dm_ms_address", "proprietary"] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
        test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);

        let mut u_setup = default_p2p_u_setup();
        match unsupported {
            "clir_control" => u_setup.clir_control = 1,
            "facility" => u_setup.facility = Some(type3_marker()),
            "dm_ms_address" => u_setup.dm_ms_address = Some(type3_marker()),
            "proprietary" => u_setup.proprietary = Some(type3_marker()),
            _ => unreachable!(),
        }

        test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
        test.run_stack(Some(1));
        let msgs = test.dump_sinks();

        // EN 300 392-2 table 14.30 includes these optional U-SETUP fields,
        // but CLIR/Facility/DM-MS/Proprietary require functions this SwMI does
        // not implement. Reject before call-id allocation instead of silently
        // accepting a request whose semantics were not honoured.
        assert_p2p_setup_rejected_with_dummy_call_id_and_cause(&msgs, TEST_ISSI, DisconnectCause::IncompatibleTrafficCase);
    }
}

#[test]
fn test_u_call_restore_unsupported_returns_function_not_supported_without_circuit() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    test.submit_message(build_u_call_restore_msg(TEST_ISSI, 0x123, TEST_CALLED_ISSI));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.7.3.2 / table 14.33 permits CMCE FUNCTION NOT
    // SUPPORTED as the SwMI response to an individually addressed CMCE PDU.
    // Pointer 0 means the whole U-CALL RESTORE PDU type is unsupported.
    let unsupported: Vec<_> = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_cmce_function_not_supported(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        unsupported.len(),
        1,
        "unsupported U-CALL RESTORE should receive one CMCE FUNCTION NOT SUPPORTED"
    );
    let (prim, pdu) = &unsupported[0];
    assert_eq!(pdu.not_supported_pdu_type, CmcePduTypeUl::UCallRestore.into_raw() as u8);
    assert!(!pdu.call_identifier_present);
    assert_eq!(pdu.call_identifier, None);
    assert_eq!(pdu.function_not_supported_pointer, 0);
    assert_eq!(pdu.length_of_received_pdu_extract, None);
    assert!(pdu.received_pdu_extract.is_none());
    assert_eq!(prim.main_address.ssi, TEST_ISSI);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert!(prim.chan_alloc.is_none());
    assert_eq!(count_d_setups(&msgs), 0);
    assert_eq!(count_umac_open(&msgs), 0);
}

#[test]
fn test_u_facility_unsupported_returns_function_not_supported_without_circuit() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);

    test.submit_message(build_u_facility_msg(TEST_ISSI));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.7.2.5 defines U-FACILITY as call-unrelated SS
    // transport. This SwMI does not implement SS handling yet, so clause
    // 14.7.3.2/table 14.33 is used to reject the unsupported PDU type.
    let unsupported: Vec<_> = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_cmce_function_not_supported(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        unsupported.len(),
        1,
        "unsupported U-FACILITY should receive one CMCE FUNCTION NOT SUPPORTED"
    );
    let (prim, pdu) = &unsupported[0];
    assert_eq!(pdu.not_supported_pdu_type, CmcePduTypeUl::UFacility.into_raw() as u8);
    assert!(!pdu.call_identifier_present);
    assert_eq!(pdu.call_identifier, None);
    assert_eq!(pdu.function_not_supported_pointer, 0);
    assert_eq!(pdu.length_of_received_pdu_extract, None);
    assert!(pdu.received_pdu_extract.is_none());
    assert_eq!(prim.main_address.ssi, TEST_ISSI);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert!(prim.chan_alloc.is_none());
    assert_eq!(count_d_setups(&msgs), 0);
    assert_eq!(count_umac_open(&msgs), 0);
}

#[test]
fn test_group_u_setup_without_called_party_rejects_with_dummy_call_id() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);

    let u_setup = USetup {
        area_selection: 0,
        hook_method_selection: false,
        simplex_duplex_selection: false,
        basic_service_information: BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2Mp,
            slots_per_frame: None,
            speech_service: Some(0),
        },
        request_to_transmit_send_data: false,
        call_priority: 0,
        clir_control: 0,
        called_party_type_identifier: PartyTypeIdentifier::Reserved,
        called_party_ssi: None,
        called_party_short_number_address: None,
        called_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };

    test.submit_message(build_u_setup_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.2.3.2 requires D-RELEASE when the SwMI cannot
    // support a group-call request. With no allocated SwMI call identity yet,
    // the dummy call identity is zero per clause 3.1.
    let releases: Vec<_> = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(releases.len(), 1, "malformed group setup should receive one D-RELEASE");
    let (release_prim, release) = &releases[0];
    assert_eq!(release.call_identifier, 0);
    assert_eq!(release.disconnect_cause, DisconnectCause::RequestedServiceNotAvailable);
    assert_eq!(release_prim.main_address.ssi, TEST_ISSI);
    assert_eq!(release_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(release_prim.layer2service, Layer2Service::Unacknowledged);
    assert!(release_prim.chan_alloc.is_none());
    assert_eq!(count_d_setups(&msgs), 0);
    assert_eq!(count_umac_open(&msgs), 0);
}

#[test]
fn test_group_u_setup_without_listeners_rejects_with_dummy_call_id() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);

    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.2.3.2 requires D-RELEASE when the SwMI cannot
    // support the group-call request. With no allocated SwMI call identity yet,
    // the dummy call identity is zero per clause 3.1.
    let releases: Vec<_> = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(releases.len(), 1, "unsupported group setup should receive one D-RELEASE");
    let (release_prim, release) = &releases[0];
    assert_eq!(release.call_identifier, 0);
    assert_eq!(release.disconnect_cause, DisconnectCause::RequestedServiceNotAvailable);
    assert_eq!(release_prim.main_address.ssi, TEST_ISSI);
    assert_eq!(release_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(release_prim.layer2service, Layer2Service::Unacknowledged);
    assert!(release_prim.chan_alloc.is_none());
    assert_eq!(count_d_setups(&msgs), 0);
    assert_eq!(count_umac_open(&msgs), 0);
}

#[test]
fn test_group_tx_demand_from_non_speaker_is_queued_without_floor_handoff() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    // EN 300 392-2 clause 14.5.2.2.1 lets D-TX GRANTED inform a
    // requesting MS that its floor request is queued while the current group
    // speaker remains active. Table 14.18 makes transmitting-party IEs
    // optional; keeping the PDU compact lets it fit assigned-channel FACCH.
    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(grants.len(), 1, "queued U-TX DEMAND should answer only the requester");
    let (grant_prim, grant) = &grants[0];
    assert_eq!(grant.call_identifier, call_id);
    assert_eq!(grant.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert!(!grant.transmission_request_permission);
    assert_compact_d_tx_granted_facch(grant_prim, grant);
    assert_eq!(grant_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(grant_prim.main_address.ssi_type, SsiType::Issi);
    assert!(grant_prim.stealing_permission);
    assert_eq!(
        count_umac_floor_granted(&demand_msgs),
        0,
        "queued floor request must not notify UMAC of a floor handoff"
    );
    assert!(
        d_info_reset_t310_prims(&demand_msgs).is_empty(),
        "queued-only U-TX DEMAND must not reset T310"
    );
}

#[test]
fn test_group_preemptive_u_tx_demand_default_off_queues_without_interrupt() {
    debug::setup_logging_verbose();

    // EN 300 392-2 clause 14.5.2.2.1 f) permits pre-emptive U-TX DEMAND
    // handling only when the SwMI supports transmission interruption. The
    // default config keeps the compatibility guard off. Table 14.85 maps
    // priorities 2 and 3 to pre-emptive/emergency TX demand.
    for tx_demand_priority in [2, 3] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

        test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
        let call_id = start_group_call(&mut test);

        test.submit_message(build_u_tx_demand_msg_with_priority(TEST_CALLED_ISSI, call_id, tx_demand_priority));
        test.run_stack(Some(1));
        let demand_msgs = test.dump_sinks();

        assert_eq!(count_d_tx_interrupt(&demand_msgs), 0, "priority {tx_demand_priority}");
        assert_eq!(count_umac_floor_granted(&demand_msgs), 0, "priority {tx_demand_priority}");
        let grants: Vec<_> = demand_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(grants.len(), 1, "priority {tx_demand_priority}");
        assert_eq!(grants[0].0.main_address.ssi, TEST_CALLED_ISSI, "priority {tx_demand_priority}");
        assert_eq!(
            grants[0].1.transmission_grant,
            TransmissionGrant::RequestQueued.into_raw() as u8,
            "priority {tx_demand_priority}"
        );
        assert_compact_d_tx_granted_facch(grants[0].0, &grants[0].1);
    }
}

#[test]
fn test_group_preemptive_u_tx_demand_enabled_interrupts_current_speaker_before_grant() {
    debug::setup_logging_verbose();

    // EN 300 392-2 table 14.85 marks TX demand priorities 2 and 3 as
    // pre-emptive/emergency. Clause 14.5.2.2.1 f) permits interruption only
    // when SwMI support is explicitly enabled.
    for tx_demand_priority in [2, 3] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
        config.cell.transmission_interruption_enabled = true;
        let mut test = ComponentTest::from_config(config, Some(dltime));

        test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
        let call_id = start_group_call(&mut test);

        test.submit_message(build_u_tx_demand_msg_with_priority(TEST_CALLED_ISSI, call_id, tx_demand_priority));
        test.run_stack(Some(1));
        let demand_msgs = test.dump_sinks();

        let interrupts: Vec<_> = demand_msgs
            .iter()
            .enumerate()
            .filter_map(|(idx, msg)| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_interrupt(prim).map(|pdu| (idx, prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(
            interrupts.len(),
            2,
            "enabled local pre-emption should send individual and group D-TX-INTERRUPT for priority {tx_demand_priority}"
        );
        assert!(interrupts.iter().any(|(_, prim, interrupt)| {
            prim.main_address.ssi == TEST_ISSI
                && prim.main_address.ssi_type == SsiType::Issi
                && interrupt.transmitting_party_address_ssi == Some(TEST_CALLED_ISSI as u64)
                && interrupt.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8
        }));
        assert!(interrupts.iter().any(|(_, prim, interrupt)| {
            prim.main_address.ssi == TEST_GSSI
                && prim.main_address.ssi_type == SsiType::Gssi
                && interrupt.transmitting_party_address_ssi == Some(TEST_CALLED_ISSI as u64)
                && interrupt.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8
        }));

        let grants: Vec<_> = demand_msgs
            .iter()
            .enumerate()
            .filter_map(|(idx, msg)| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (idx, prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(
            grants.len(),
            2,
            "pre-emptive handoff should grant requester and inform local listeners for priority {tx_demand_priority}"
        );
        assert!(
            interrupts
                .iter()
                .all(|(interrupt_idx, _, _)| grants.iter().all(|(grant_idx, _, _)| interrupt_idx < grant_idx)),
            "D-TX-INTERRUPT must withdraw current permission before D-TX-GRANTED advertises the new speaker"
        );
        assert!(grants.iter().any(|(_, prim, grant)| {
            prim.main_address.ssi == TEST_CALLED_ISSI
                && prim.main_address.ssi_type == SsiType::Issi
                && grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8
        }));
        assert!(grants.iter().any(|(_, prim, grant)| {
            prim.main_address == TetraAddress::issi(TEST_ISSI)
                && grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8
        }));
        for (_, prim, grant) in &grants {
            assert_compact_d_tx_granted_facch(prim, grant);
        }

        assert_eq!(count_umac_floor_granted(&demand_msgs), 0, "priority {tx_demand_priority}");
        let activation_msgs = transmit_positive_group_grants_and_drain(&mut test, &demand_msgs);
        assert_eq!(count_umac_floor_granted(&activation_msgs), 1, "priority {tx_demand_priority}");
        assert!(activation_msgs.iter().any(|msg| {
            matches!(
                &msg.msg,
                SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                    call_id: got_call_id,
                    source_issi,
                    dest_gssi,
                    ..
                }) if *got_call_id == call_id && *source_issi == TEST_CALLED_ISSI && *dest_gssi == TEST_GSSI
            )
        }));
    }
}

#[test]
fn test_large_group_preemptive_grant_removes_requester_from_fifo_before_next_handoff() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.transmission_interruption_enabled = true;
    let mut test = ComponentTest::from_config(config, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    let member_count = LARGE_GSSI_MEMBER_COUNT;
    let first_issi = 880_000_u32;
    let current_speaker = first_issi;
    let preemptive_requester = first_issi + 1;
    let next_fifo_requester = first_issi + 2;

    for offset in 0..member_count {
        let issi = first_issi + offset;
        submit_subscriber_update(&mut test, issi, Vec::new(), BrewSubscriberAction::Register);
        submit_subscriber_update(&mut test, issi, vec![TEST_GSSI], BrewSubscriberAction::Affiliate);
    }
    test.run_stack(Some((member_count as usize * 2) + 16));
    let _ = test.dump_sinks();

    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, current_speaker, TEST_GSSI);

    for issi in (first_issi + 1)..(first_issi + member_count) {
        test.submit_message(build_u_tx_demand_msg(issi, call_id));
    }
    test.run_stack(Some(member_count as usize + 16));
    let queued_msgs = test.dump_sinks();
    assert_eq!(
        queued_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim),
                _ => None,
            })
            .filter(|grant| grant.transmission_grant == TransmissionGrant::RequestQueued.into_raw() as u8)
            .count(),
        member_count as usize - 1,
        "large group should queue every in-cap floor contender before preemption"
    );

    // EN 300 392-2 clause 14.5.2.2.1 f) permits an explicitly configured
    // SwMI to interrupt the current speaker for a pre-emptive U-TX DEMAND.
    // The pre-empting ISSI must also be removed from the local FIFO, otherwise
    // its later U-TX CEASED grants the floor back to itself instead of the
    // next large-group waiter.
    test.submit_message(build_u_tx_demand_msg_with_priority(preemptive_requester, call_id, 3));
    test.run_stack(Some(1));
    let preempt_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_interrupt(&preempt_msgs), 2);
    assert_eq!(count_umac_floor_granted(&preempt_msgs), 0);
    let preempt_activation = transmit_positive_group_grants_and_drain(&mut test, &preempt_msgs);
    assert_eq!(count_umac_floor_granted(&preempt_activation), 1);
    assert!(preempt_activation.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ..
            }) if *got_call_id == call_id && *source_issi == preemptive_requester && *dest_gssi == TEST_GSSI
        )
    }));

    test.submit_message(build_u_tx_ceased_msg(preemptive_requester, call_id));
    test.run_stack(Some(1));
    let handoff_msgs = test.dump_sinks();
    let handoff_grants: Vec<_> = handoff_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();

    let next_handoff = handoff_grants
        .iter()
        .find(|(prim, grant)| {
            prim.main_address == TetraAddress::issi(next_fifo_requester)
                && grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8
        })
        .expect("next FIFO requester should receive the post-preemption floor handoff");
    assert_d_tx_granted_facch_allocation(
        next_handoff.0,
        &next_handoff.1,
        active_ts,
        active_usage,
        UlDlAssignment::Both,
        "large group post-preemption FIFO handoff",
    );
    assert!(
        handoff_grants.iter().all(|(prim, grant)| {
            prim.main_address != TetraAddress::issi(preemptive_requester)
                || grant.transmission_grant != TransmissionGrant::Granted.into_raw() as u8
        }),
        "pre-empting requester must not remain queued and be granted to itself again"
    );
    assert!(handoff_grants.iter().any(|(prim, grant)| {
        prim.main_address == TetraAddress::new(TEST_GSSI, SsiType::Gssi)
            && grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    }));
    assert_eq!(count_d_tx_ceased(&handoff_msgs), 0);
    assert_eq!(count_umac_floor_released(&handoff_msgs), 0);
    assert_eq!(count_umac_floor_granted(&handoff_msgs), 0);
    let handoff_activation = transmit_positive_group_grants_and_drain(&mut test, &handoff_msgs);
    assert_eq!(count_umac_floor_granted(&handoff_activation), 1);
}

#[test]
fn test_private_call_cleanup_preserves_group_floor_membership() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit(&mut test);

    test.submit_message(build_mm_release_individual_calls_msg(TEST_CALLED_ISSI));
    test.run_stack(Some(1));
    let cleanup_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&cleanup_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&cleanup_msgs), 0);

    // EN 300 392-2 clauses 16.8.0/16.8.4 keep accepted group identities
    // valid until an explicit group detach/replacement. A local private-call
    // cleanup must therefore not remove the MS from CMCE's GSSI listener set.
    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    let queued_grant = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(TEST_CALLED_ISSI))
        .expect("group-affiliated MS should still receive a queued return PTT grant after private cleanup");
    assert_eq!(queued_grant.1.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        queued_grant.0,
        &queued_grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Dl,
        "private cleanup preserved group listener",
    );
}

#[test]
fn test_group_tx_demand_from_unaffiliated_issi_is_not_queued_or_granted() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.2.2.1 floor requests apply to MSs
    // participating in the group call. A registered but unaffiliated ISSI must
    // not be queued for, or granted, the active GSSI floor.
    let grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(grants.len(), 1, "unaffiliated requester should receive one NotGranted response");
    let (grant_prim, grant) = &grants[0];
    assert_eq!(grant_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(grant_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(grant.transmission_grant, TransmissionGrant::NotGranted.into_raw() as u8);
    assert_compact_d_tx_granted_facch(grant_prim, grant);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_floor_granted(&ceased_msgs),
        0,
        "unaffiliated requester must not be queued for later floor handoff"
    );
}

#[test]
fn test_group_tx_ceased_hands_floor_to_queued_requester() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let (call_id, _, _) = start_group_call_with_circuit(&mut test);

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_msgs = test.dump_sinks();

    let grants: Vec<_> = ceased_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        grants.len(),
        2,
        "queued requester handoff should send D-TX-GRANTED to requester and local listeners"
    );

    let requester_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == TEST_CALLED_ISSI && prim.main_address.ssi_type == SsiType::Issi)
        .expect("expected individual D-TX-GRANTED to queued requester");
    assert_eq!(requester_grant.1.call_identifier, call_id);
    assert_eq!(requester_grant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_compact_d_tx_granted_facch(requester_grant.0, &requester_grant.1);

    let listener_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(TEST_ISSI))
        .expect("expected listener FACCH D-TX-GRANTED");
    assert_eq!(listener_grant.1.call_identifier, call_id);
    assert_eq!(
        listener_grant.1.transmission_grant,
        TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    );
    assert_compact_d_tx_granted_facch(listener_grant.0, &listener_grant.1);
    let listener_alloc = listener_grant
        .0
        .chan_alloc
        .as_ref()
        .expect("FACCH listener grant should carry channel allocation");
    assert_eq!(listener_alloc.ul_dl_assigned, UlDlAssignment::Dl);

    assert_eq!(
        count_d_setups(&ceased_msgs),
        0,
        "queued group floor handoff must not inject an immediate back-up D-SETUP over the first speech frames"
    );
    assert_no_group_d_info_reset_t310(&ceased_msgs, "queued U-TX-CEASED handoff");

    assert!(
        ceased_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_tx_ceased(prim).is_some())),
        "queued handoff should grant the next speaker instead of entering no-speaker hangtime"
    );
    assert_eq!(count_umac_floor_released(&ceased_msgs), 0);
    assert_eq!(count_umac_floor_granted(&ceased_msgs), 0);
    let activation_msgs = transmit_positive_group_grants_and_drain(&mut test, &ceased_msgs);
    assert_eq!(count_umac_floor_granted(&activation_msgs), 1);
    assert!(activation_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ..
            }) if *got_call_id == call_id && *source_issi == TEST_CALLED_ISSI && *dest_gssi == TEST_GSSI
        )
    }));
}

#[test]
fn test_large_group_floor_handoff_uses_one_gssi_listener_grant() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    let member_count = LARGE_GSSI_MEMBER_COUNT;
    let first_issi = 500_000_u32;
    for offset in 0..member_count {
        let issi = first_issi + offset;
        submit_subscriber_update(&mut test, issi, Vec::new(), BrewSubscriberAction::Register);
        submit_subscriber_update(&mut test, issi, vec![TEST_GSSI], BrewSubscriberAction::Affiliate);
    }
    test.run_stack(Some((member_count as usize * 2) + 16));
    let _ = test.dump_sinks();

    let speaker_count = 32_u32;
    let speakers: Vec<u32> = (0..speaker_count).map(|offset| first_issi + offset).collect();
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, speakers[0], TEST_GSSI);

    let mut current_speaker = speakers[0];
    for (cycle, next_speaker) in speakers.iter().copied().enumerate().skip(1) {
        test.submit_message(build_u_tx_demand_msg(next_speaker, call_id));
        test.run_stack(Some(1));
        let queued_msgs = test.dump_sinks();
        let queued_grants: Vec<_> = queued_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(
            queued_grants.len(),
            1,
            "large-group queued PTT should answer only requester on cycle {cycle}"
        );
        assert_eq!(queued_grants[0].0.main_address, TetraAddress::issi(next_speaker), "cycle {cycle}");
        assert_eq!(
            queued_grants[0].1.transmission_grant,
            TransmissionGrant::RequestQueued.into_raw() as u8,
            "cycle {cycle}"
        );
        assert_ne!(
            queued_grants[0].1.transmission_grant,
            TransmissionGrant::NotGranted.into_raw() as u8,
            "large-group queued PTT must not degrade to PTT denied on cycle {cycle}"
        );
        assert_eq!(count_umac_floor_granted(&queued_msgs), 0, "cycle {cycle}");

        test.submit_message(build_u_tx_ceased_msg(current_speaker, call_id));
        test.run_stack(Some(1));
        let handoff_msgs = test.dump_sinks();

        // EN 300 392-2 clause 14.5.2.2.1 uses group-addressed D-TX GRANTED
        // for listeners. A large GSSI must not create one listener grant per
        // affiliate, even across repeated back-and-forth PTT handoffs.
        let grants: Vec<_> = handoff_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(
            grants.len(),
            2,
            "large-group handoff should produce one requester grant plus one GSSI listener grant on cycle {cycle}"
        );

        let requester_grant = grants
            .iter()
            .find(|(prim, _)| prim.main_address == TetraAddress::issi(next_speaker))
            .expect("queued requester should get one individual grant");
        assert_eq!(
            requester_grant.1.transmission_grant,
            TransmissionGrant::Granted.into_raw() as u8,
            "cycle {cycle}"
        );
        assert_d_tx_granted_facch_allocation(
            requester_grant.0,
            &requester_grant.1,
            active_ts,
            active_usage,
            UlDlAssignment::Both,
            "large group requester handoff",
        );

        let listener_grants: Vec<_> = grants
            .iter()
            .filter(|(prim, _)| prim.main_address == TetraAddress::new(TEST_GSSI, SsiType::Gssi))
            .collect();
        assert_eq!(
            listener_grants.len(),
            1,
            "listeners must be notified once via GSSI on cycle {cycle}"
        );
        assert_eq!(
            listener_grants[0].1.transmission_grant,
            TransmissionGrant::GrantedToOtherUser.into_raw() as u8,
            "cycle {cycle}"
        );
        assert_d_tx_granted_facch_allocation(
            listener_grants[0].0,
            &listener_grants[0].1,
            active_ts,
            active_usage,
            UlDlAssignment::Dl,
            "large group listener grant",
        );
        assert_eq!(count_umac_floor_granted(&handoff_msgs), 0, "cycle {cycle}");
        let activation_msgs = transmit_positive_group_grants_and_drain(&mut test, &handoff_msgs);
        assert_eq!(count_umac_floor_granted(&activation_msgs), 1, "cycle {cycle}");
        assert_eq!(count_d_releases(&handoff_msgs), 0, "cycle {cycle}");
        assert_eq!(count_umac_call_ended_or_close(&handoff_msgs), 0, "cycle {cycle}");

        current_speaker = next_speaker;
    }
}

#[test]
fn test_large_group_duplicate_queued_u_tx_demand_is_idempotent_before_handoff() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    let member_count = LARGE_GSSI_MEMBER_COUNT;
    let first_issi = 540_000_u32;
    let current_speaker = first_issi;
    let duplicate_requester = first_issi + 1;
    let next_requester = first_issi + 2;
    for offset in 0..member_count {
        let issi = first_issi + offset;
        submit_subscriber_update(&mut test, issi, Vec::new(), BrewSubscriberAction::Register);
        submit_subscriber_update(&mut test, issi, vec![TEST_GSSI], BrewSubscriberAction::Affiliate);
    }
    test.run_stack(Some((member_count as usize * 2) + 16));
    let _ = test.dump_sinks();

    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, current_speaker, TEST_GSSI);

    let duplicate_count = 8;
    for _ in 0..duplicate_count {
        test.submit_message(build_u_tx_demand_msg(duplicate_requester, call_id));
    }
    test.submit_message(build_u_tx_demand_msg(next_requester, call_id));
    test.run_stack(Some(duplicate_count + 4));
    let demand_msgs = test.dump_sinks();
    let demand_grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        demand_grants
            .iter()
            .filter(|(prim, grant)| {
                prim.main_address == TetraAddress::issi(duplicate_requester)
                    && grant.transmission_grant == TransmissionGrant::RequestQueued.into_raw() as u8
            })
            .count(),
        duplicate_count,
        "duplicate queued requester should receive explicit queued responses without adding duplicate FIFO entries"
    );
    assert_eq!(
        demand_grants
            .iter()
            .filter(|(prim, grant)| {
                prim.main_address == TetraAddress::issi(next_requester)
                    && grant.transmission_grant == TransmissionGrant::RequestQueued.into_raw() as u8
            })
            .count(),
        1,
        "next unique requester should still enter the FIFO after duplicate pressure"
    );
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
    assert_eq!(count_d_releases(&demand_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&demand_msgs), 0);

    // EN 300 392-2 clause 14.5.2.2.1 allows a queued request-to-transmit
    // response while another group member has the floor. Nexus-BS must treat
    // repeated same-ISSI U-TX DEMAND as idempotent queue pressure: the repeated
    // radio gets queued responses, but only one FIFO entry is retained.
    test.submit_message(build_u_tx_ceased_msg(current_speaker, call_id));
    test.run_stack(Some(1));
    let first_handoff_msgs = test.dump_sinks();
    let first_handoff_grants: Vec<_> = first_handoff_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(first_handoff_grants.len(), 2);
    let duplicate_handoff = first_handoff_grants
        .iter()
        .find(|(prim, grant)| {
            prim.main_address == TetraAddress::issi(duplicate_requester)
                && grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8
        })
        .expect("duplicate requester should receive exactly one granted handoff");
    assert_d_tx_granted_facch_allocation(
        duplicate_handoff.0,
        &duplicate_handoff.1,
        active_ts,
        active_usage,
        UlDlAssignment::Both,
        "large group duplicate requester handoff",
    );
    assert!(
        first_handoff_grants.iter().any(|(prim, grant)| {
            prim.main_address == TetraAddress::new(TEST_GSSI, SsiType::Gssi)
                && grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8
        }),
        "duplicate requester handoff should notify listeners once via GSSI"
    );
    assert_eq!(count_umac_floor_granted(&first_handoff_msgs), 0);
    let first_activation = transmit_positive_group_grants_and_drain(&mut test, &first_handoff_msgs);
    assert_eq!(count_umac_floor_granted(&first_activation), 1);
    assert_eq!(count_d_releases(&first_handoff_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&first_handoff_msgs), 0);

    test.submit_message(build_u_tx_ceased_msg(duplicate_requester, call_id));
    test.run_stack(Some(1));
    let second_handoff_msgs = test.dump_sinks();
    let second_handoff_grants: Vec<_> = second_handoff_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(second_handoff_grants.len(), 2);
    let next_handoff = second_handoff_grants
        .iter()
        .find(|(prim, grant)| {
            prim.main_address == TetraAddress::issi(next_requester)
                && grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8
        })
        .expect("next unique requester should receive the second handoff");
    assert_d_tx_granted_facch_allocation(
        next_handoff.0,
        &next_handoff.1,
        active_ts,
        active_usage,
        UlDlAssignment::Both,
        "large group next unique requester handoff after duplicate pressure",
    );
    assert!(
        second_handoff_grants
            .iter()
            .all(|(prim, grant)| !(prim.main_address == TetraAddress::issi(duplicate_requester)
                && grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8)),
        "duplicate requester must not remain in the FIFO for a second self-handoff"
    );
    assert_eq!(count_umac_floor_granted(&second_handoff_msgs), 0);
    let second_activation = transmit_positive_group_grants_and_drain(&mut test, &second_handoff_msgs);
    assert_eq!(count_umac_floor_granted(&second_activation), 1);
    assert_eq!(count_d_releases(&second_handoff_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&second_handoff_msgs), 0);
}

#[test]
fn test_large_group_floor_queue_is_bounded_fifo_for_thousands_of_waiters() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    let member_count = LARGE_GSSI_MEMBER_COUNT;
    let first_issi = 700_000_u32;
    let current_speaker = first_issi;
    let queued_requester = first_issi + 1;
    for offset in 0..member_count {
        let issi = first_issi + offset;
        submit_subscriber_update(&mut test, issi, Vec::new(), BrewSubscriberAction::Register);
        submit_subscriber_update(&mut test, issi, vec![TEST_GSSI], BrewSubscriberAction::Affiliate);
    }
    test.run_stack(Some((member_count as usize * 2) + 16));
    let _ = test.dump_sinks();

    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, current_speaker, TEST_GSSI);

    test.submit_message(build_u_tx_demand_msg(queued_requester, call_id));
    test.run_stack(Some(1));
    let first_waiter_msgs = test.dump_sinks();
    let first_waiter_grants: Vec<_> = first_waiter_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(first_waiter_grants.len(), 1);
    assert_eq!(first_waiter_grants[0].0.main_address, TetraAddress::issi(queued_requester));
    assert_eq!(
        first_waiter_grants[0].1.transmission_grant,
        TransmissionGrant::RequestQueued.into_raw() as u8
    );
    assert_eq!(count_umac_floor_granted(&first_waiter_msgs), 0);

    for issi in (first_issi + 2)..(first_issi + member_count) {
        test.submit_message(build_u_tx_demand_msg(issi, call_id));
    }
    test.run_stack(Some(member_count as usize + 16));
    let busy_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.2.2.1 lets the SwMI answer a floor request
    // with queued/granted/not-granted state. Nexus-BS keeps a bounded FIFO so
    // thousands of affiliated contenders can wait their turn without replacing
    // the first queued MS.
    let busy_grants: Vec<_> = busy_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .filter(|(prim, _)| prim.main_address.ssi >= first_issi + 2 && prim.main_address.ssi < first_issi + member_count)
        .collect();
    assert_eq!(busy_grants.len(), member_count as usize - 2);
    for (prim, grant) in &busy_grants {
        assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
        assert!(
            prim.main_address.ssi >= first_issi + 2 && prim.main_address.ssi < first_issi + member_count,
            "only busy requesters should receive busy queue responses"
        );
        assert!(
            grant.transmission_grant == TransmissionGrant::RequestQueued.into_raw() as u8,
            "large group contenders inside the bounded FIFO should be queued"
        );
        assert_d_tx_granted_facch_allocation(
            prim,
            grant,
            active_ts,
            active_usage,
            UlDlAssignment::Dl,
            "large group busy floor requester",
        );
    }
    assert_eq!(count_umac_floor_granted(&busy_msgs), 0);
    assert_eq!(count_d_releases(&busy_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&busy_msgs), 0);

    test.submit_message(build_u_tx_ceased_msg(current_speaker, call_id));
    test.run_stack(Some(1));
    let handoff_msgs = test.dump_sinks();
    let handoff_grants: Vec<_> = handoff_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(handoff_grants.len(), 2);
    let queued_handoff = handoff_grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(queued_requester))
        .expect("only the first queued requester should receive the handoff floor");
    assert_eq!(queued_handoff.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert!(
        handoff_grants.iter().all(|(prim, _)| {
            prim.main_address.ssi == queued_requester || (prim.main_address.ssi == TEST_GSSI && prim.main_address.ssi_type == SsiType::Gssi)
        }),
        "later requesters must not jump ahead of the first queued floor requester"
    );
    assert_eq!(count_umac_floor_granted(&handoff_msgs), 0);
    let handoff_activation = transmit_positive_group_grants_and_drain(&mut test, &handoff_msgs);
    assert_eq!(count_umac_floor_granted(&handoff_activation), 1);
    assert_eq!(count_d_releases(&handoff_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&handoff_msgs), 0);

    let second_queued_requester = first_issi + 2;
    test.submit_message(build_u_tx_ceased_msg(queued_requester, call_id));
    test.run_stack(Some(1));
    let second_handoff_msgs = test.dump_sinks();
    let second_handoff_grants: Vec<_> = second_handoff_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    let second_handoff = second_handoff_grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(second_queued_requester))
        .expect("FIFO should hand the next floor to the second queued requester");
    assert_eq!(second_handoff.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert!(
        second_handoff_grants.iter().all(|(prim, _)| {
            prim.main_address.ssi == second_queued_requester
                || (prim.main_address.ssi == TEST_GSSI && prim.main_address.ssi_type == SsiType::Gssi)
        }),
        "FIFO handoff must not skip to later large-group requesters"
    );
    assert_eq!(count_umac_floor_granted(&second_handoff_msgs), 0);
    let second_handoff_activation = transmit_positive_group_grants_and_drain(&mut test, &second_handoff_msgs);
    assert_eq!(count_umac_floor_granted(&second_handoff_activation), 1);

    let mut current_fifo_speaker = second_queued_requester;
    for expected_next_speaker in (first_issi + 3)..(first_issi + member_count) {
        test.submit_message(build_u_tx_ceased_msg(current_fifo_speaker, call_id));
        test.run_stack(Some(1));
        let drain_msgs = test.dump_sinks();
        let drain_grants: Vec<_> = drain_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();

        assert_eq!(
            drain_grants.len(),
            2,
            "FIFO drain should emit one requester grant and one GSSI listener grant for expected ISSI {expected_next_speaker}"
        );
        let requester_grant = drain_grants
            .iter()
            .find(|(prim, grant)| {
                prim.main_address == TetraAddress::issi(expected_next_speaker)
                    && grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8
            })
            .expect("next FIFO requester should receive the floor during full drain");
        assert_d_tx_granted_facch_allocation(
            requester_grant.0,
            &requester_grant.1,
            active_ts,
            active_usage,
            UlDlAssignment::Both,
            "large group full FIFO drain requester handoff",
        );
        assert!(
            drain_grants.iter().any(|(prim, grant)| {
                prim.main_address == TetraAddress::new(TEST_GSSI, SsiType::Gssi)
                    && grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8
            }),
            "full FIFO drain should notify listeners once via GSSI"
        );
        assert!(
            drain_grants
                .iter()
                .all(|(_, grant)| grant.transmission_grant != TransmissionGrant::NotGranted.into_raw() as u8),
            "accepted FIFO waiters must not degrade to NotGranted during full drain"
        );
        assert_eq!(count_d_releases(&drain_msgs), 0);
        assert_eq!(count_d_tx_ceased(&drain_msgs), 0);
        assert_eq!(count_umac_call_ended_or_close(&drain_msgs), 0);
        assert_eq!(count_umac_floor_released(&drain_msgs), 0);
        assert_eq!(count_umac_floor_granted(&drain_msgs), 0);
        let drain_activation = transmit_positive_group_grants_and_drain(&mut test, &drain_msgs);
        assert_eq!(count_umac_floor_granted(&drain_activation), 1);
        current_fifo_speaker = expected_next_speaker;
    }

    test.submit_message(build_u_tx_ceased_msg(current_fifo_speaker, call_id));
    test.run_stack(Some(1));
    let final_ceased_start_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_granted(&final_ceased_start_msgs), 0);
    assert_eq!(count_d_tx_ceased(&final_ceased_start_msgs), 0);
    assert_eq!(count_umac_floor_released(&final_ceased_start_msgs), 0);
    assert_eq!(count_umac_floor_granted(&final_ceased_start_msgs), 0);

    drain_group_tx_ceased_tail_after_large_stress(&mut test, dltime);
    let final_ceased_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_granted(&final_ceased_msgs), 0);
    assert_eq!(count_d_tx_ceased(&final_ceased_msgs), 1);
    assert_eq!(count_umac_floor_released(&final_ceased_msgs), 1);
    assert_eq!(count_umac_floor_granted(&final_ceased_msgs), 0);
}

#[test]
fn test_large_group_floor_fifo_overflow_returns_not_granted_after_4096_waiters() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    let member_count = LARGE_GSSI_MEMBER_COUNT + 2;
    let first_issi = 760_000_u32;
    let current_speaker = first_issi;
    let first_waiter = first_issi + 1;
    let second_waiter = first_issi + 2;
    let overflow_waiter = first_issi + member_count - 1;
    for offset in 0..member_count {
        let issi = first_issi + offset;
        submit_subscriber_update(&mut test, issi, Vec::new(), BrewSubscriberAction::Register);
        submit_subscriber_update(&mut test, issi, vec![TEST_GSSI], BrewSubscriberAction::Affiliate);
    }
    test.run_stack(Some((member_count as usize * 2) + 16));
    let _ = test.dump_sinks();

    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, current_speaker, TEST_GSSI);

    for issi in (first_issi + 1)..(first_issi + member_count) {
        test.submit_message(build_u_tx_demand_msg(issi, call_id));
    }
    test.run_stack(Some(member_count as usize + 16));
    let demand_msgs = test.dump_sinks();
    let grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .filter(|(prim, _)| prim.main_address.ssi > current_speaker && prim.main_address.ssi < first_issi + member_count)
        .collect();
    assert_eq!(grants.len(), member_count as usize - 1);
    assert_eq!(
        grants
            .iter()
            .filter(|(_, grant)| grant.transmission_grant == TransmissionGrant::RequestQueued.into_raw() as u8)
            .count(),
        LARGE_GSSI_MEMBER_COUNT as usize,
        "bounded FIFO should accept exactly 4096 group floor waiters"
    );
    let overflow = grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(overflow_waiter))
        .expect("overflow requester should receive an explicit floor response");
    assert_eq!(
        overflow.1.transmission_grant,
        TransmissionGrant::NotGranted.into_raw() as u8,
        "requester beyond the bounded FIFO must be explicitly denied"
    );

    test.submit_message(build_u_tx_ceased_msg(current_speaker, call_id));
    test.run_stack(Some(1));
    let handoff_msgs = test.dump_sinks();
    let handoff_grants: Vec<_> = handoff_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    let first_handoff = handoff_grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(first_waiter))
        .expect("overflow denial must not disturb the head of the FIFO");
    assert_eq!(first_handoff.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_eq!(count_umac_floor_granted(&handoff_msgs), 0);
    let first_activation = transmit_positive_group_grants_and_drain(&mut test, &handoff_msgs);
    assert_eq!(count_umac_floor_granted(&first_activation), 1);

    // Once the head waiter is granted, the bounded FIFO has capacity again.
    // A previously denied requester may retry and enter the tail, but it must
    // not jump ahead of already queued affiliated users.
    test.submit_message(build_u_tx_demand_msg(overflow_waiter, call_id));
    test.run_stack(Some(1));
    let retry_msgs = test.dump_sinks();
    let retry_grant = retry_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(overflow_waiter))
        .expect("overflow requester should be allowed to retry after one FIFO slot frees");
    assert_eq!(
        retry_grant.1.transmission_grant,
        TransmissionGrant::RequestQueued.into_raw() as u8,
        "overflow retry should enter the tail once FIFO capacity is available"
    );
    assert_d_tx_granted_facch_allocation(
        retry_grant.0,
        &retry_grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Dl,
        "large group overflow retry queued at FIFO tail",
    );

    test.submit_message(build_u_tx_ceased_msg(first_waiter, call_id));
    test.run_stack(Some(1));
    let second_handoff_msgs = test.dump_sinks();
    let second_handoff_grants: Vec<_> = second_handoff_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    let second_handoff = second_handoff_grants
        .iter()
        .find(|(prim, grant)| {
            prim.main_address == TetraAddress::issi(second_waiter)
                && grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8
        })
        .expect("overflow retry must not jump ahead of the original FIFO tail");
    assert_d_tx_granted_facch_allocation(
        second_handoff.0,
        &second_handoff.1,
        active_ts,
        active_usage,
        UlDlAssignment::Both,
        "large group overflow retry preserves FIFO order",
    );
    assert!(
        second_handoff_grants.iter().all(|(prim, grant)| {
            prim.main_address != TetraAddress::issi(overflow_waiter)
                || grant.transmission_grant != TransmissionGrant::Granted.into_raw() as u8
        }),
        "overflow retry must wait behind the already queued FIFO users"
    );
    assert_eq!(count_umac_floor_granted(&second_handoff_msgs), 0);
    let second_activation = transmit_positive_group_grants_and_drain(&mut test, &second_handoff_msgs);
    assert_eq!(count_umac_floor_granted(&second_activation), 1);
}

#[test]
fn test_ten_thousand_member_group_floor_overflow_is_explicit_and_private_call_still_works() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.call_timeout_secs = 0;
    let mut test = ComponentTest::from_config(config, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    let member_count = 10_000_u32;
    let first_issi = 900_000_u32;
    let current_speaker = first_issi;
    let first_waiter = first_issi + 1;
    let fifo_capacity = LARGE_GSSI_MEMBER_COUNT as usize;

    for offset in 0..member_count {
        let issi = first_issi + offset;
        submit_subscriber_update(&mut test, issi, Vec::new(), BrewSubscriberAction::Register);
        submit_subscriber_update(&mut test, issi, vec![TEST_GSSI], BrewSubscriberAction::Affiliate);
    }
    test.run_stack(Some((member_count as usize * 2) + 16));
    let _ = test.dump_sinks();

    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, current_speaker, TEST_GSSI);

    for issi in (first_issi + 1)..(first_issi + member_count) {
        test.submit_message(build_u_tx_demand_msg(issi, call_id));
    }
    test.run_stack(Some(member_count as usize + 64));
    let demand_msgs = test.dump_sinks();
    let grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .filter(|(prim, _)| prim.main_address.ssi > current_speaker && prim.main_address.ssi < first_issi + member_count)
        .collect();

    // EN 300 392-2 clause 14.5.2.2.1 defines explicit floor-control
    // outcomes. At 10k affiliated members, Nexus-BS must not silently drop
    // over-cap PTT contenders: in-cap contenders are queued, over-cap
    // contenders are denied explicitly, and the active call remains usable.
    assert_eq!(grants.len(), member_count as usize - 1);
    assert_eq!(
        grants
            .iter()
            .filter(|(_, grant)| grant.transmission_grant == TransmissionGrant::RequestQueued.into_raw() as u8)
            .count(),
        fifo_capacity,
        "bounded FIFO should queue exactly {fifo_capacity} floor contenders"
    );
    assert_eq!(
        grants
            .iter()
            .filter(|(_, grant)| grant.transmission_grant == TransmissionGrant::NotGranted.into_raw() as u8)
            .count(),
        member_count as usize - 1 - fifo_capacity,
        "over-cap floor contenders must receive explicit NotGranted responses"
    );
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
    assert_eq!(count_d_releases(&demand_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&demand_msgs), 0);

    test.submit_message(build_u_tx_ceased_msg(current_speaker, call_id));
    test.run_stack(Some(1));
    let handoff_msgs = test.dump_sinks();
    let handoff_grants: Vec<_> = handoff_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    let first_handoff = handoff_grants
        .iter()
        .find(|(prim, grant)| {
            prim.main_address == TetraAddress::issi(first_waiter) && grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8
        })
        .expect("10k overflow storm must not disturb the head FIFO handoff");
    assert_d_tx_granted_facch_allocation(
        first_handoff.0,
        &first_handoff.1,
        active_ts,
        active_usage,
        UlDlAssignment::Both,
        "10k group floor overflow first handoff",
    );
    assert!(
        handoff_grants.iter().all(|(prim, grant)| {
            prim.main_address == TetraAddress::issi(first_waiter)
                || (prim.main_address == TetraAddress::new(TEST_GSSI, SsiType::Gssi)
                    && grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8)
        }),
        "only the FIFO head and GSSI listeners should receive the first handoff after 10k overflow"
    );
    assert_eq!(count_umac_floor_granted(&handoff_msgs), 0);
    let handoff_activation = transmit_positive_group_grants_and_drain(&mut test, &handoff_msgs);
    assert_eq!(count_umac_floor_granted(&handoff_activation), 1);
    assert_eq!(count_d_releases(&handoff_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&handoff_msgs), 0);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let (private_call_id, private_connect_msgs) = start_active_p2p_call_with_connect_msgs(&mut test);
    assert_ne!(
        private_call_id, call_id,
        "private call aftercare must allocate a distinct live call identifier after the 10k group storm"
    );
    assert_eq!(
        count_umac_open(&private_connect_msgs),
        1,
        "simple private call should still open its shared traffic circuit after the 10k group storm"
    );
    assert!(
        private_connect_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some())),
        "simple private call should still send D-CONNECT after the 10k group storm"
    );
    assert!(
        private_connect_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect_acknowledge(prim).is_some())),
        "simple private call should still send D-CONNECT-ACKNOWLEDGE after the 10k group storm"
    );
}

#[test]
fn test_cross_layer_large_group_floor_grant_survives_wrapped_ptt_storm_to_lmac() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce, TetraEntity::Mle, TetraEntity::Llc, TetraEntity::Umac],
        vec![TetraEntity::Lmac, TetraEntity::Brew],
    );

    let member_count = LARGE_GSSI_MEMBER_COUNT;
    let first_issi = 820_000_u32;
    let current_speaker = first_issi;
    let queued_requester = first_issi + 1;
    let call_id = 0x1234;
    force_cmce_next_call_identifier(&mut test, call_id);

    for offset in 0..member_count {
        let issi = first_issi + offset;
        submit_subscriber_update(&mut test, issi, Vec::new(), BrewSubscriberAction::Register);
        submit_subscriber_update(&mut test, issi, vec![TEST_GSSI], BrewSubscriberAction::Affiliate);
    }
    test.run_stack(Some((member_count as usize * 2) + 16));
    let _ = test.dump_sinks();

    test.submit_message(build_u_setup_msg(current_speaker, TEST_GSSI));
    test.run_stack(Some(8));
    let _ = test.dump_sinks();
    assert!(
        cmce_debug_active_call_ids(&mut test).contains(&call_id),
        "large-group setup should maintain the forced call identifier before the PTT storm"
    );

    test.submit_message(build_u_tx_demand_msg(queued_requester, call_id));
    for issi in (first_issi + 2)..(first_issi + member_count) {
        test.submit_message(build_u_tx_demand_msg(issi, call_id));
    }
    test.submit_message(build_u_tx_ceased_msg(current_speaker, call_id));
    test.run_stack(Some(256));
    let storm_msgs = test.dump_sinks();
    let wrapped_grants = wrapped_d_tx_granted_from_lmac_msgs(&storm_msgs);

    // EN 300 392-2 clauses 14.5.2.2.1, 20.4.1.1.3, 22.3.2.4.1 and 23.5:
    // the requester floor grant must survive the real CMCE->MLE->LLC->UMAC
    // wrapping path and reach assigned-channel STCH as BL-UDATA/MLE(CMCE).
    let requester_positive = wrapped_grants
        .iter()
        .find(|decoded| {
            decoded.logical_channel == LogicalChannel::Stch
                && decoded.resource.addr.is_some_and(|addr| addr.ssi == queued_requester)
                && decoded
                    .resource
                    .chan_alloc_element
                    .as_ref()
                    .is_some_and(|alloc| alloc.ul_dl_assigned == UlDlAssignment::Both)
                && decoded.grant.call_identifier == call_id
                && decoded.grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8
        })
        .unwrap_or_else(|| panic!("expected wrapped positive requester D-TX GRANTED at LMAC; decoded grants={wrapped_grants:?}"));
    assert!(
        !requester_positive.bl_udata.has_fcs,
        "CMCE floor-control BL-UDATA should use the unacknowledged no-FCS path in this fixture"
    );

    let listener_notification = wrapped_grants
        .iter()
        .find(|decoded| {
            decoded.logical_channel == LogicalChannel::Stch
                && decoded.resource.addr.is_some_and(|addr| addr.ssi == TEST_GSSI)
                && decoded.grant.call_identifier == call_id
                && decoded.grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8
        })
        .unwrap_or_else(|| panic!("expected wrapped GSSI D-TX GRANTED/GrantedToOtherUser at LMAC; decoded grants={wrapped_grants:?}"));
    assert!(
        requester_positive.resource_sequence < listener_notification.resource_sequence,
        "requester positive grant should reach STCH before the listener floor notification"
    );

    let lower_value_grant_count = wrapped_grants
        .iter()
        .filter(|decoded| {
            decoded.resource.addr.is_some_and(|addr| {
                (first_issi + 2..first_issi + member_count).contains(&addr.ssi)
                    && decoded.grant.call_identifier == call_id
                    && matches!(
                        TransmissionGrant::try_from(decoded.grant.transmission_grant as u64),
                        Ok(TransmissionGrant::RequestQueued | TransmissionGrant::NotGranted)
                    )
            })
        })
        .count();
    assert!(
        lower_value_grant_count > 0,
        "fixture should emit at least one wrapped queued/not-granted response so the PTT storm is observable at LMAC"
    );

    if let Some(first_lower_value) = wrapped_grants.iter().find(|decoded| {
        decoded.resource.addr.is_some_and(|addr| {
            (first_issi + 2..first_issi + member_count).contains(&addr.ssi)
                && decoded.grant.call_identifier == call_id
                && matches!(
                    TransmissionGrant::try_from(decoded.grant.transmission_grant as u64),
                    Ok(TransmissionGrant::RequestQueued | TransmissionGrant::NotGranted)
                )
        })
    }) {
        assert!(
            requester_positive.resource_sequence < first_lower_value.resource_sequence,
            "positive requester floor grant must be emitted before lower-value storm queue/denial responses"
        );
        assert!(
            listener_notification.resource_sequence < first_lower_value.resource_sequence,
            "listener floor notification must stay ahead of lower-value storm queue/denial responses"
        );
    }
}

#[test]
fn test_group_call_uses_shared_registry_when_cmce_listener_mirror_is_empty() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    {
        let mut state = test.config.state_write();
        for issi in [LAB_ISSI_A, LAB_ISSI_B] {
            state.subscribers.register(issi);
            assert!(state.subscribers.affiliate(issi, LAB_GROUP_GSSI));
        }
    }

    // CMCE has not received MmSubscriberUpdate messages in this fixture, so
    // its local subscriber_groups/group_listeners mirror is empty. The shared
    // MM registry is authoritative after restart recovery/resync; group setup
    // and floor requests must not fail as "no listener"/unaffiliated.
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);

    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    let grant = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_B))
        .expect("shared-registry group member should receive queued PTT response");

    assert_eq!(grant.1.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        grant.0,
        &grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Dl,
        "shared-registry group floor request",
    );
    assert_eq!(count_d_releases(&demand_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&demand_msgs), 0);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
}

#[test]
fn test_group_ul_inactivity_hands_floor_to_queued_requester() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);

    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let queued_msgs = test.dump_sinks();
    let queued_grant = queued_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_B))
        .expect("second MS should be queued while first MS owns group floor");
    assert_eq!(queued_grant.1.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);

    test.submit_message(build_ul_inactivity_timeout_msg(active_ts));
    test.run_stack(Some(1));
    let timeout_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.2.2.1 permits the SwMI to grant a queued
    // U-TX DEMAND when the current group transmission ceases. A local UL
    // inactivity guard is the BS-side cease event; the waiting MS must not
    // need a second PTT attempt.
    assert_eq!(count_d_tx_ceased(&timeout_msgs), 0);
    assert_eq!(count_umac_floor_released(&timeout_msgs), 0);

    let grants: Vec<_> = timeout_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        grants.len(),
        2,
        "queued group timeout handoff should notify requester and group listeners"
    );

    let requester_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_B))
        .expect("queued requester should get the group floor");
    assert_eq!(requester_grant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        requester_grant.0,
        &requester_grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Both,
        "group UL inactivity handoff requester grant",
    );

    let listener_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_A))
        .expect("local listener should be told which MS now has the floor");
    assert_eq!(
        listener_grant.1.transmission_grant,
        TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    );
    assert_d_tx_granted_facch_allocation(
        listener_grant.0,
        &listener_grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Dl,
        "group UL inactivity listener grant",
    );

    assert_eq!(
        count_d_setups(&timeout_msgs),
        0,
        "queued timeout handoff must not inject an immediate back-up D-SETUP over the first speech frames"
    );
    assert_no_group_d_info_reset_t310(&timeout_msgs, "UL inactivity queued handoff");

    assert_eq!(
        count_umac_floor_granted(&timeout_msgs),
        0,
        "queued timeout handoff must wait until the positive D-TX GRANTED is transmitted"
    );
    let requester_reporter = d_tx_granted_reporter(&timeout_msgs, TetraAddress::issi(LAB_ISSI_B), TransmissionGrant::Granted);
    assert_eq!(requester_reporter.get_state(), TxState::Pending);
    requester_reporter.mark_transmitted();
    test.run_stack(Some(1));
    let activation_msgs = test.dump_sinks();
    assert_eq!(count_umac_floor_granted(&activation_msgs), 1);
    assert!(activation_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ts,
            }) if *got_call_id == call_id
                && *source_issi == LAB_ISSI_B
                && *dest_gssi == LAB_GROUP_GSSI
                && *ts == active_ts
        )
    }));
}

#[test]
fn test_group_ul_inactivity_regrants_current_speaker_once_before_tx_ceased() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);

    test.submit_message(build_ul_inactivity_timeout_msg(active_ts));
    test.run_stack(Some(1));
    let first_timeout_msgs = test.dump_sinks();

    // EN 300 392-2 clause 23.5.2.2.7 allows a bounded regrant if no uplink
    // arrives after the individual grant. For field radios, this avoids
    // turning one missed/corrupted D-TX GRANTED into an immediate floor loss.
    assert_eq!(count_d_tx_ceased(&first_timeout_msgs), 0);
    assert_eq!(count_umac_floor_released(&first_timeout_msgs), 0);

    let regrant = first_timeout_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_B))
        .expect("first inactivity timeout should regrant the current group speaker");
    assert_eq!(regrant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        regrant.0,
        &regrant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Both,
        "group inactivity current-speaker regrant",
    );
    assert_eq!(
        first_timeout_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .count(),
        1,
        "regrant must not repeat listener notifications while the floor owner is unchanged"
    );
    assert_eq!(
        count_umac_floor_granted(&first_timeout_msgs),
        0,
        "current-speaker regrant must wait until the positive D-TX GRANTED is transmitted"
    );
    let regrant_reporter = d_tx_granted_reporter(&first_timeout_msgs, TetraAddress::issi(LAB_ISSI_B), TransmissionGrant::Granted);
    assert_eq!(regrant_reporter.get_state(), TxState::Pending);
    regrant_reporter.mark_transmitted();
    test.run_stack(Some(1));
    let activation_msgs = test.dump_sinks();
    assert_eq!(count_umac_floor_granted(&activation_msgs), 1);
    assert!(activation_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ts,
            }) if *got_call_id == call_id
                && *source_issi == LAB_ISSI_B
                && *dest_gssi == LAB_GROUP_GSSI
                && *ts == active_ts
        )
    }));

    test.submit_message(build_ul_inactivity_timeout_msg(active_ts));
    test.run_stack(Some(1));
    let second_timeout_msgs = test.dump_sinks();

    assert_eq!(
        count_d_tx_granted(&second_timeout_msgs),
        0,
        "second inactivity timeout in the same floor epoch must not regrant forever"
    );
    assert_eq!(count_d_tx_ceased(&second_timeout_msgs), 1);
    assert_eq!(count_umac_floor_released(&second_timeout_msgs), 1);
}

#[test]
fn test_group_226333_alternating_ptt_round_trip_queues_not_denies() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);

    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let first_return_msgs = test.dump_sinks();
    let first_return_grants: Vec<_> = first_return_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(first_return_grants.len(), 1);
    assert_eq!(first_return_grants[0].0.main_address, TetraAddress::issi(LAB_ISSI_B));
    assert_eq!(
        first_return_grants[0].1.transmission_grant,
        TransmissionGrant::RequestQueued.into_raw() as u8
    );
    assert_d_tx_granted_facch_allocation(
        first_return_grants[0].0,
        &first_return_grants[0].1,
        active_ts,
        active_usage,
        UlDlAssignment::Dl,
        "226333 first return PTT while another MS has floor",
    );
    assert_eq!(count_d_releases(&first_return_msgs), 0);
    assert_eq!(count_d_tx_ceased(&first_return_msgs), 0);
    assert_eq!(count_umac_floor_released(&first_return_msgs), 0);
    assert_eq!(count_umac_floor_granted(&first_return_msgs), 0);

    test.submit_message(build_u_tx_ceased_msg(LAB_ISSI_A, call_id));
    test.run_stack(Some(1));
    let handoff_to_b_msgs = test.dump_sinks();
    let handoff_to_b_grants: Vec<_> = handoff_to_b_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(handoff_to_b_grants.len(), 2);
    let grant_to_b = handoff_to_b_grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_B))
        .expect("queued ISSI should get the floor");
    assert_eq!(grant_to_b.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        grant_to_b.0,
        &grant_to_b.1,
        active_ts,
        active_usage,
        UlDlAssignment::Both,
        "226333 handoff to queued requester",
    );
    assert!(
        handoff_to_b_grants.iter().all(|(prim, grant)| {
            prim.main_address != TetraAddress::new(LAB_GROUP_GSSI, SsiType::Gssi)
                && (prim.main_address != TetraAddress::issi(LAB_ISSI_B)
                    || grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8)
        }),
        "2260082 must not hear a GSSI/self GrantedToOtherUser immediately after its positive floor grant"
    );
    assert_eq!(count_d_tx_ceased(&handoff_to_b_msgs), 0);
    assert_eq!(count_umac_floor_released(&handoff_to_b_msgs), 0);
    assert_eq!(count_umac_floor_granted(&handoff_to_b_msgs), 0);
    let handoff_to_b_activation = transmit_positive_group_grants_and_drain(&mut test, &handoff_to_b_msgs);
    assert!(handoff_to_b_activation.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ..
            }) if *got_call_id == call_id && *source_issi == LAB_ISSI_B && *dest_gssi == LAB_GROUP_GSSI
        )
    }));

    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_A, call_id));
    test.run_stack(Some(1));
    let queued_back_msgs = test.dump_sinks();
    let queued_back_grants: Vec<_> = queued_back_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(queued_back_grants.len(), 1);
    assert_eq!(queued_back_grants[0].0.main_address, TetraAddress::issi(LAB_ISSI_A));
    assert_eq!(
        queued_back_grants[0].1.transmission_grant,
        TransmissionGrant::RequestQueued.into_raw() as u8
    );
    assert_ne!(
        queued_back_grants[0].1.transmission_grant,
        TransmissionGrant::NotGranted.into_raw() as u8
    );
    assert_d_tx_granted_facch_allocation(
        queued_back_grants[0].0,
        &queued_back_grants[0].1,
        active_ts,
        active_usage,
        UlDlAssignment::Dl,
        "226333 return request while second MS has floor",
    );
    assert_eq!(count_d_releases(&queued_back_msgs), 0);
    assert_eq!(count_d_tx_ceased(&queued_back_msgs), 0);
    assert_eq!(count_umac_floor_released(&queued_back_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&queued_back_msgs), 0);
    assert_eq!(count_umac_floor_granted(&queued_back_msgs), 0);

    test.submit_message(build_u_tx_ceased_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let handoff_to_a_msgs = test.dump_sinks();
    let handoff_to_a_grants: Vec<_> = handoff_to_a_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(handoff_to_a_grants.len(), 2);
    let grant_to_a = handoff_to_a_grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_A))
        .expect("original speaker should get the floor back");
    assert_eq!(grant_to_a.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        grant_to_a.0,
        &grant_to_a.1,
        active_ts,
        active_usage,
        UlDlAssignment::Both,
        "226333 handoff back to original speaker",
    );
    assert!(
        handoff_to_a_grants.iter().all(|(prim, grant)| {
            prim.main_address != TetraAddress::new(LAB_GROUP_GSSI, SsiType::Gssi)
                && (prim.main_address != TetraAddress::issi(LAB_ISSI_A)
                    || grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8)
        }),
        "local group speaker must not receive listener-only GrantedToOtherUser after its positive floor grant"
    );
    assert_eq!(count_d_tx_ceased(&handoff_to_a_msgs), 0);
    assert_eq!(count_umac_floor_released(&handoff_to_a_msgs), 0);
    assert_eq!(count_umac_floor_granted(&handoff_to_a_msgs), 0);
    let handoff_to_a_activation = transmit_positive_group_grants_and_drain(&mut test, &handoff_to_a_msgs);
    assert!(handoff_to_a_activation.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ..
            }) if *got_call_id == call_id && *source_issi == LAB_ISSI_A && *dest_gssi == LAB_GROUP_GSSI
        )
    }));

    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_A, call_id));
    test.run_stack(Some(1));
    let current_speaker_retry_msgs = test.dump_sinks();
    let current_speaker_grants: Vec<_> = current_speaker_retry_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        current_speaker_grants.len(),
        2,
        "current-speaker U-TX DEMAND should reassert permission instead of being silently ignored"
    );
    let current_individual = current_speaker_grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_A))
        .expect("current speaker should receive explicit D-TX-GRANTED");
    assert_eq!(current_individual.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        current_individual.0,
        &current_individual.1,
        active_ts,
        active_usage,
        UlDlAssignment::Both,
        "226333 current speaker retry",
    );
    assert_eq!(count_d_releases(&current_speaker_retry_msgs), 0);
    assert_eq!(count_d_tx_ceased(&current_speaker_retry_msgs), 0);
    assert_eq!(count_umac_floor_released(&current_speaker_retry_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&current_speaker_retry_msgs), 0);
    assert_eq!(count_umac_floor_granted(&current_speaker_retry_msgs), 0);
    let retry_activation = transmit_positive_group_grants_and_drain(&mut test, &current_speaker_retry_msgs);
    assert_eq!(count_umac_floor_granted(&retry_activation), 1);
}

#[test]
fn test_group_226333_three_local_members_listener_grants_exclude_new_speaker() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_MXP600, LAB_GROUP_GSSI);
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);

    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let queued_msgs = test.dump_sinks();
    let queued_grants: Vec<_> = queued_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(queued_grants.len(), 1);
    assert_eq!(queued_grants[0].0.main_address, TetraAddress::issi(LAB_ISSI_B));
    assert_eq!(
        queued_grants[0].1.transmission_grant,
        TransmissionGrant::RequestQueued.into_raw() as u8
    );

    test.submit_message(build_u_tx_ceased_msg(LAB_ISSI_A, call_id));
    test.run_stack(Some(1));
    let handoff_msgs = test.dump_sinks();
    let handoff_grants: Vec<_> = handoff_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();

    assert_eq!(
        handoff_grants.len(),
        3,
        "three-member local 226333 handoff should grant requester and notify the two other local listeners individually"
    );
    assert!(
        handoff_grants
            .iter()
            .all(|(prim, _)| prim.main_address != TetraAddress::new(LAB_GROUP_GSSI, SsiType::Gssi)),
        "local 226333 handoff must not send a GSSI listener grant that the new speaker can hear"
    );

    let requester_grant = handoff_grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_B))
        .expect("2260082 should receive the positive floor grant");
    assert_eq!(requester_grant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        requester_grant.0,
        &requester_grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Both,
        "226333 three-member handoff to 2260082",
    );

    for listener_issi in [LAB_ISSI_A, LAB_ISSI_MXP600] {
        let listener_grant = handoff_grants
            .iter()
            .find(|(prim, _)| prim.main_address == TetraAddress::issi(listener_issi))
            .unwrap_or_else(|| panic!("listener ISSI {listener_issi} should receive GrantedToOtherUser"));
        assert_eq!(
            listener_grant.1.transmission_grant,
            TransmissionGrant::GrantedToOtherUser.into_raw() as u8
        );
        assert_d_tx_granted_facch_allocation(
            listener_grant.0,
            &listener_grant.1,
            active_ts,
            active_usage,
            UlDlAssignment::Dl,
            "226333 three-member local listener grant",
        );
    }

    assert_eq!(count_umac_floor_granted(&handoff_msgs), 0);
    let activation_msgs = transmit_positive_group_grants_and_drain(&mut test, &handoff_msgs);
    assert_eq!(count_umac_floor_granted(&activation_msgs), 1);
    assert_eq!(count_d_tx_ceased(&handoff_msgs), 0);
}

#[test]
fn test_group_local_listener_floor_grant_fanout_threshold() {
    debug::setup_logging_verbose();

    for (member_count, expect_individual_listeners) in [(101_u32, true), (102_u32, false)] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );

        let first_issi = 700_000_u32 + member_count * 1_000;
        let current_speaker = first_issi;
        let next_speaker = first_issi + 1;
        for offset in 0..member_count {
            let issi = first_issi + offset;
            submit_subscriber_update(&mut test, issi, Vec::new(), BrewSubscriberAction::Register);
            submit_subscriber_update(&mut test, issi, vec![TEST_GSSI], BrewSubscriberAction::Affiliate);
        }
        test.run_stack(Some((member_count as usize * 2) + 16));
        let _ = test.dump_sinks();

        let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, current_speaker, TEST_GSSI);

        test.submit_message(build_u_tx_demand_msg(next_speaker, call_id));
        test.run_stack(Some(1));
        let queued_msgs = test.dump_sinks();
        let queued_grants: Vec<_> = queued_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(queued_grants.len(), 1, "member_count={member_count}");
        assert_eq!(queued_grants[0].0.main_address, TetraAddress::issi(next_speaker));
        assert_eq!(
            queued_grants[0].1.transmission_grant,
            TransmissionGrant::RequestQueued.into_raw() as u8,
            "member_count={member_count}"
        );

        test.submit_message(build_u_tx_ceased_msg(current_speaker, call_id));
        test.run_stack(Some(1));
        let handoff_msgs = test.dump_sinks();
        let handoff_grants: Vec<_> = handoff_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();

        let requester_grant = handoff_grants
            .iter()
            .find(|(prim, _)| prim.main_address == TetraAddress::issi(next_speaker))
            .expect("queued requester should receive positive floor grant");
        assert_eq!(
            requester_grant.1.transmission_grant,
            TransmissionGrant::Granted.into_raw() as u8,
            "member_count={member_count}"
        );
        assert_d_tx_granted_facch_allocation(
            requester_grant.0,
            &requester_grant.1,
            active_ts,
            active_usage,
            UlDlAssignment::Both,
            "local listener threshold requester grant",
        );

        if expect_individual_listeners {
            assert_eq!(handoff_grants.len(), member_count as usize, "member_count={member_count}");
            assert!(
                handoff_grants
                    .iter()
                    .all(|(prim, _)| prim.main_address != TetraAddress::new(TEST_GSSI, SsiType::Gssi)),
                "100 local listeners should not use GSSI fallback"
            );
            let listener_grants = handoff_grants
                .iter()
                .filter(|(prim, grant)| {
                    prim.main_address.ssi_type == SsiType::Issi
                        && prim.main_address.ssi != next_speaker
                        && grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8
                })
                .count();
            assert_eq!(listener_grants, 100, "member_count={member_count}");
        } else {
            assert_eq!(handoff_grants.len(), 2, "member_count={member_count}");
            let gssi_grant = handoff_grants
                .iter()
                .find(|(prim, _)| prim.main_address == TetraAddress::new(TEST_GSSI, SsiType::Gssi))
                .expect("101 local listeners should use bounded GSSI fallback");
            assert_eq!(
                gssi_grant.1.transmission_grant,
                TransmissionGrant::GrantedToOtherUser.into_raw() as u8,
                "member_count={member_count}"
            );
            assert_d_tx_granted_facch_allocation(
                gssi_grant.0,
                &gssi_grant.1,
                active_ts,
                active_usage,
                UlDlAssignment::Dl,
                "local listener threshold GSSI fallback grant",
            );
        }

        assert!(
            handoff_grants.iter().all(|(prim, grant)| {
                prim.main_address != TetraAddress::issi(next_speaker)
                    || grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8
            }),
            "new local speaker must not receive GrantedToOtherUser as an ISSI copy"
        );
        assert_eq!(count_umac_floor_granted(&handoff_msgs), 0);
        let activation_msgs = transmit_positive_group_grants_and_drain(&mut test, &handoff_msgs);
        assert_eq!(count_umac_floor_granted(&activation_msgs), 1);
        assert_eq!(count_d_tx_ceased(&handoff_msgs), 0);
    }
}

#[test]
fn test_restart_recovery_cached_226333_group_restores_cmce_listeners_after_unrouted_ack() {
    debug::setup_logging_verbose();

    let path = unique_restart_recovery_path("cmce-cached-226333-unrouted-ack");
    std::fs::write(
        &path,
        format!(
            "{} {}:0:4\n{} {}:0:4\n{} {}:0:4\n",
            LAB_ISSI_A, LAB_GROUP_GSSI, LAB_ISSI_B, LAB_GROUP_GSSI, LAB_ISSI_MXP600, LAB_GROUP_GSSI
        ),
    )
    .expect("failed to seed restart recovery cache");

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2260000, 2269999)]);
    config.cell.energy_saving_mode = EnergySavingMode::Eg7 as u8;
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(
        vec![TetraEntity::Mm, TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    test.run_stack(Some(73));
    let _ = test.dump_sinks();

    for issi in [LAB_ISSI_A, LAB_ISSI_B, LAB_ISSI_MXP600] {
        submit_location_update_without_group_identity_location_demand(&mut test, issi, LocationUpdateType::DemandLocationUpdating);
        test.run_stack(Some(1));
        let attach_msgs = test.dump_sinks();
        assert!(
            contains_location_update_accept(&attach_msgs),
            "restart recovery LU for ISSI {issi} should still get D-LOCATION UPDATE ACCEPT"
        );
        assert!(
            test.config.state_read().subscribers.group_members(LAB_GROUP_GSSI).contains(&issi),
            "cached GSSI should be provisionally restored for ISSI {issi}"
        );

        // EN 300 392-2 clause 16.8.1 requires an MS ACK for the SwMI group
        // refresh. The MLE handle is local plumbing, so this deliberately uses
        // a non-matching handle to exercise the restart path seen in field logs.
        submit_swmi_group_refresh_ack(&mut test, issi, 123_000 + issi);
        test.run_stack(Some(1));
        let ack_msgs = test.dump_sinks();
        assert_eq!(
            count_d_releases(&ack_msgs),
            0,
            "group refresh ACK for ISSI {issi} must not cause a CMCE release"
        );
    }

    test.run_stack(Some(721));
    let after_t353_msgs = test.dump_sinks();
    assert_eq!(
        count_d_releases(&after_t353_msgs),
        0,
        "accepted restart refresh ACKs must prevent later T353 rollback/release"
    );

    let mut members = test.config.state_read().subscribers.group_members(LAB_GROUP_GSSI);
    members.sort_unstable();
    assert_eq!(
        members,
        vec![LAB_ISSI_B, LAB_ISSI_A, LAB_ISSI_MXP600],
        "restart recovery should leave every lab ISSI affiliated to 226333"
    );

    // CMCE consumes the MM Register/Affiliate updates. A valid group call plus
    // queued return PTT proves the restored GSSI is usable beyond dashboard
    // state and local MM cache bookkeeping.
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);
    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let ptt_msgs = test.dump_sinks();
    let queued_grant = ptt_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_B))
        .expect("restored 226333 listener should receive queued return PTT grant");
    assert_eq!(queued_grant.1.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        queued_grant.0,
        &queued_grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Dl,
        "restart-restored 226333 return PTT while another MS has floor",
    );
    assert_eq!(count_d_releases(&ptt_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&ptt_msgs), 0);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_restart_recovery_large_cached_gssi_restores_cmce_listeners_and_turn_taking() {
    debug::setup_logging_verbose();

    let path = unique_restart_recovery_path("cmce-large-cached-gssi-unrouted-ack");
    let member_count = LARGE_GSSI_MEMBER_COUNT;
    let first_issi = 2_264_000_u32;
    let gssi = LAB_GROUP_GSSI;
    let cache: String = (0..member_count)
        .map(|offset| format!("{} {}:0:4\n", first_issi + offset, gssi))
        .collect();
    std::fs::write(&path, cache).expect("failed to seed large restart recovery cache");

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2_260_000, 2_269_999)]);
    config.security.issi_whitelist.clear();

    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.config.state_write().subscriber_recovery_path = Some(path.clone());
    test.populate_entities(
        vec![TetraEntity::Mm, TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    test.run_stack(Some(73));
    let _ = test.dump_sinks();

    for offset in 0..member_count {
        let issi = first_issi + offset;
        submit_location_update_without_group_identity_location_demand(&mut test, issi, LocationUpdateType::DemandLocationUpdating);
        test.run_stack(Some(1));
        let attach_msgs = test.dump_sinks();
        assert!(
            contains_location_update_accept(&attach_msgs),
            "restart recovery LU for ISSI {issi} should still get D-LOCATION UPDATE ACCEPT"
        );

        // EN 300 392-2 clause 16.8.1 keeps the SwMI group refresh ACK as the
        // confirmation point. The non-matching handle models unrouted field
        // ACKs and must still converge to the restored cached affiliation.
        submit_swmi_group_refresh_ack(&mut test, issi, 900_000 + offset);
        test.run_stack(Some(1));
        let ack_msgs = test.dump_sinks();
        assert_eq!(
            count_d_releases(&ack_msgs),
            0,
            "large restart ACK for ISSI {issi} must not release CMCE state"
        );
    }

    let mut members = test.config.state_read().subscribers.group_members(gssi);
    members.sort_unstable();
    assert_eq!(members.len(), member_count as usize);
    assert_eq!(members.first().copied(), Some(first_issi));
    assert_eq!(members.last().copied(), Some(first_issi + member_count - 1));

    // CMCE must receive enough restored listener state to run a real group
    // floor exchange after restart. This is the field symptom guard: the
    // return PTT must be queued/grantable, not denied because the BS forgot
    // the listener's group affiliation after restart.
    let speaker_issi = first_issi;
    let requester_issi = first_issi + member_count - 1;
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, speaker_issi, gssi);
    test.submit_message(build_u_tx_demand_msg(requester_issi, call_id));
    test.run_stack(Some(1));
    let ptt_msgs = test.dump_sinks();
    let queued_grant = ptt_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(requester_issi))
        .expect("large restart-restored listener should receive queued return PTT grant");
    assert_eq!(queued_grant.1.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        queued_grant.0,
        &queued_grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Dl,
        "large restart-restored GSSI return PTT while another MS has floor",
    );
    assert_eq!(count_d_releases(&ptt_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&ptt_msgs), 0);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(format!("{path}.tmp"));
}

#[test]
fn test_group_tx_ceased_does_not_grant_deaffiliated_queued_requester() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    let queued_grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(queued_grants.len(), 1);
    assert_eq!(
        queued_grants[0].1.transmission_grant,
        TransmissionGrant::RequestQueued.into_raw() as u8
    );

    test.submit_message(build_mm_deaffiliate_msg(TEST_CALLED_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 clause 14.5.2.2.1 scopes group floor control to MSs
    // involved in the call. A requester that loses GSSI affiliation after a
    // queued U-TX DEMAND must not receive a later D-TX-GRANTED handoff.
    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_start_msgs = test.dump_sinks();

    assert_eq!(count_d_tx_granted(&ceased_start_msgs), 0);
    assert_eq!(count_umac_floor_granted(&ceased_start_msgs), 0);
    assert_eq!(count_d_tx_ceased(&ceased_start_msgs), 0);
    assert_eq!(count_umac_floor_released(&ceased_start_msgs), 0);
    assert!(ceased_start_msgs.iter().all(|msg| {
        !matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                source_issi,
                dest_gssi,
                ..
            }) if *source_issi == TEST_CALLED_ISSI && *dest_gssi == TEST_GSSI
        )
    }));

    drain_group_tx_ceased_tail(&mut test, dltime);
    let ceased_msgs = test.dump_sinks();

    assert_eq!(count_d_tx_granted(&ceased_msgs), 0);
    assert_eq!(count_umac_floor_granted(&ceased_msgs), 0);
    assert_eq!(count_d_tx_ceased(&ceased_msgs), 1);
    assert_eq!(count_umac_floor_released(&ceased_msgs), 1);
    assert!(ceased_msgs.iter().all(|msg| {
        !matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                source_issi,
                dest_gssi,
                ..
            }) if *source_issi == TEST_CALLED_ISSI && *dest_gssi == TEST_GSSI
        )
    }));
}

#[test]
fn test_group_tx_ceased_skips_deaffiliated_front_waiter_and_grants_next_fifo_waiter() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_OTHER_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.submit_message(build_u_tx_demand_msg(TEST_OTHER_ISSI, call_id));
    test.run_stack(Some(2));
    let demand_msgs = test.dump_sinks();
    let queued_grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .filter(|(_, grant)| grant.transmission_grant == TransmissionGrant::RequestQueued.into_raw() as u8)
        .collect();
    assert_eq!(queued_grants.len(), 2);

    test.submit_message(build_mm_deaffiliate_msg(TEST_CALLED_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // EN 300 392-2 clause 14.5.2.2.1 scopes queued floor requests to MSs
    // still involved in the group call. A deaffiliated head waiter is stale;
    // the SwMI policy may grant the next valid queued requester.
    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_msgs = test.dump_sinks();
    let handoff_grants: Vec<_> = ceased_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(handoff_grants.len(), 2);
    let next_handoff = handoff_grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(TEST_OTHER_ISSI))
        .expect("next affiliated FIFO waiter should receive the floor");
    assert_eq!(next_handoff.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert!(
        handoff_grants.iter().all(|(prim, _)| {
            prim.main_address == TetraAddress::issi(TEST_OTHER_ISSI) || prim.main_address == TetraAddress::issi(TEST_ISSI)
        }),
        "deaffiliated front waiter must not receive the handoff"
    );
    assert_eq!(count_umac_floor_granted(&ceased_msgs), 0);
    let activation_msgs = transmit_positive_group_grants_and_drain(&mut test, &ceased_msgs);
    assert_eq!(count_umac_floor_granted(&activation_msgs), 1);
    assert_eq!(count_d_tx_ceased(&ceased_msgs), 0);
}

#[test]
fn test_group_queued_requester_u_tx_ceased_withdraws_before_handoff() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_OTHER_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.submit_message(build_u_tx_demand_msg(TEST_OTHER_ISSI, call_id));
    test.run_stack(Some(2));
    let demand_msgs = test.dump_sinks();
    assert_eq!(
        demand_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim),
                _ => None,
            })
            .filter(|grant| grant.transmission_grant == TransmissionGrant::RequestQueued.into_raw() as u8)
            .count(),
        2
    );

    // EN 300 392-2 clause 14.5.2.2.1 a) states that a queued request-to-
    // transmit may be withdrawn with U-TX CEASED and no CC protocol response
    // shall be received from the SwMI for that message.
    test.submit_message(build_u_tx_ceased_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let withdraw_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_granted(&withdraw_msgs), 0);
    assert_eq!(count_d_tx_ceased(&withdraw_msgs), 0);
    assert_eq!(count_umac_floor_granted(&withdraw_msgs), 0);
    assert_eq!(count_umac_floor_released(&withdraw_msgs), 0);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let handoff_msgs = test.dump_sinks();
    let handoff_grants: Vec<_> = handoff_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(handoff_grants.len(), 3);
    let next_handoff = handoff_grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(TEST_OTHER_ISSI))
        .expect("withdrawn queued requester must be skipped in favour of next FIFO waiter");
    assert_eq!(next_handoff.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert!(
        handoff_grants.iter().all(|(prim, grant)| {
            prim.main_address != TetraAddress::issi(TEST_CALLED_ISSI)
                || grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8
        }),
        "withdrawn queued requester may remain a listener, but must not receive a positive floor grant"
    );
    assert!(
        handoff_grants.iter().all(|(prim, _)| {
            prim.main_address == TetraAddress::issi(TEST_OTHER_ISSI)
                || prim.main_address == TetraAddress::issi(TEST_ISSI)
                || prim.main_address == TetraAddress::issi(TEST_CALLED_ISSI)
        }),
        "withdrawn queued requester must not receive a later floor grant"
    );
    assert_eq!(count_umac_floor_granted(&handoff_msgs), 0);
    let activation_msgs = transmit_positive_group_grants_and_drain(&mut test, &handoff_msgs);
    assert_eq!(count_umac_floor_granted(&activation_msgs), 1);
    assert_eq!(count_d_tx_ceased(&handoff_msgs), 0);
}

#[test]
fn test_group_current_speaker_deaffiliate_releases_floor_without_handoff() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    // EN 300 392-2 clause 14.5.2.2.1 makes the SwMI responsible for the
    // transmitting MS. If the current speaker loses group affiliation while
    // other listeners remain, the old floor must be withdrawn instead of
    // leaving UMAC configured for an MS outside the GSSI.
    test.submit_message(build_mm_deaffiliate_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let deaffiliate_msgs = test.dump_sinks();

    assert_eq!(count_d_tx_granted(&deaffiliate_msgs), 0);
    assert_eq!(count_umac_floor_granted(&deaffiliate_msgs), 0);
    assert_eq!(count_d_tx_ceased(&deaffiliate_msgs), 1);
    assert_eq!(count_umac_floor_released(&deaffiliate_msgs), 1);
    assert_eq!(count_umac_call_ended_or_close(&deaffiliate_msgs), 0);
    assert!(deaffiliate_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorReleased {
                call_id: got_call_id,
                ..
            }) if *got_call_id == call_id
        )
    }));
}

#[test]
fn test_group_tx_ceased_without_queue_releases_floor_to_hangtime() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit(&mut test);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_start_msgs = test.dump_sinks();

    assert_eq!(
        count_d_tx_ceased(&ceased_start_msgs),
        0,
        "group no-queue U-TX CEASED must wait for bearer-tail drain before D-TX CEASED"
    );
    assert_eq!(
        count_umac_floor_released(&ceased_start_msgs),
        0,
        "group no-queue U-TX CEASED must not put UMAC into hangtime before TCH/S tail drain"
    );

    drain_group_tx_ceased_tail(&mut test, dltime);
    let ceased_msgs = test.dump_sinks();

    let ceased: Vec<_> = ceased_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_ceased(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(ceased.len(), 1, "current speaker U-TX-CEASED should emit one group D-TX-CEASED");
    let (ceased_prim, d_tx_ceased) = &ceased[0];
    assert_eq!(d_tx_ceased.call_identifier, call_id);
    assert!(!d_tx_ceased.transmission_request_permission);
    assert_eq!(ceased_prim.main_address.ssi, TEST_GSSI);
    assert_eq!(ceased_prim.main_address.ssi_type, SsiType::Gssi);
    assert!(ceased_prim.stealing_permission);
    let ceased_alloc = ceased_prim
        .chan_alloc
        .as_ref()
        .expect("FACCH D-TX-CEASED should carry channel allocation");
    assert_chan_alloc_matches_circuit(ceased_alloc, active_ts, active_usage, "group D-TX-CEASED");
    assert_eq!(ceased_alloc.ul_dl_assigned, UlDlAssignment::Dl);

    assert_eq!(count_umac_floor_granted(&ceased_msgs), 0);
    assert_eq!(count_umac_floor_released(&ceased_msgs), 1);
    assert!(
        d_info_reset_t310_prims(&ceased_msgs).is_empty(),
        "no-handoff U-TX-CEASED must not reset T310"
    );
    assert!(ceased_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorReleased {
                call_id: got_call_id,
                ..
            }) if *got_call_id == call_id
        )
    }));
}

#[test]
fn test_legacy_gssi_group_tx_ceased_without_queue_releases_call_after_floor_ceased() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.legacy_gssi_group_call = true;
    let mut test = ComponentTest::from_config(config, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);
    let (call_id, _active_ts, _active_usage) = start_group_call_with_circuit_for(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);

    test.submit_message(build_u_tx_ceased_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let ceased_start_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&ceased_start_msgs), 0);
    assert_eq!(count_d_releases(&ceased_start_msgs), 0);
    assert_eq!(count_umac_floor_released(&ceased_start_msgs), 0);

    drain_group_tx_ceased_tail(&mut test, dltime);
    let release_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 14.5.2.2.1(e) and 14.5.2.3: legacy mode first
    // sends the normal end-of-transmission edge, then clears the maintained
    // local group call so older MSs use fresh setup for the next over.
    assert_eq!(count_d_tx_ceased(&release_msgs), 1);
    assert_eq!(count_umac_floor_released(&release_msgs), 1);
    assert_eq!(
        count_d_releases(&release_msgs),
        2,
        "legacy no-handoff over should send FACCH and MCCH D-RELEASE"
    );
    assert_eq!(
        count_d_tx_granted(&release_msgs),
        0,
        "legacy no-handoff over must not fast-regrant the old speaker"
    );
    assert_eq!(
        count_umac_call_ended_or_close(&release_msgs),
        0,
        "D-RELEASE reporter/guard must close the old circuit later, not before release delivery"
    );

    test.submit_message(build_u_setup_msg(LAB_ISSI_B, LAB_GROUP_GSSI));
    test.run_stack(Some(1));
    let fresh_setup_msgs = test.dump_sinks();

    assert_eq!(
        count_d_releases(&fresh_setup_msgs),
        0,
        "fresh same-GSSI setup during old release drain must not be rejected"
    );
    assert_eq!(count_d_call_proceedings(&fresh_setup_msgs), 1);
    assert_eq!(count_d_connects(&fresh_setup_msgs), 1);
    assert_eq!(count_d_setups(&fresh_setup_msgs), 1);
    assert_eq!(count_umac_open(&fresh_setup_msgs), 1);
}

#[test]
fn test_legacy_gssi_same_speaker_retake_during_tail_releases_instead_of_positive_fast_grant() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.legacy_gssi_group_call = true;
    let mut test = ComponentTest::from_config(config, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_MXP600, LAB_GROUP_GSSI);
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);

    test.submit_message(build_u_tx_ceased_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let ceased_start_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&ceased_start_msgs), 0);
    assert_eq!(count_d_releases(&ceased_start_msgs), 0);

    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let retake_msgs = test.dump_sinks();

    let queued_grant = retake_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_B))
        .expect("same-speaker legacy retake during tail should be acknowledged as queued until tail clears");
    assert_eq!(queued_grant.1.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        queued_grant.0,
        &queued_grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Dl,
        "legacy same-speaker group retake queued during TX-CEASED tail",
    );
    assert_eq!(count_umac_floor_granted(&retake_msgs), 0);
    assert_eq!(count_d_tx_ceased(&retake_msgs), 0);
    assert_eq!(count_d_releases(&retake_msgs), 0);

    drain_group_tx_ceased_tail(&mut test, dltime);
    let release_msgs = test.dump_sinks();

    assert_eq!(
        count_d_tx_granted(&release_msgs),
        0,
        "legacy same-speaker tail retake must not send the positive fast grant that old Motorola terminals fail to transmit on"
    );
    assert_eq!(count_d_tx_ceased(&release_msgs), 1);
    assert_eq!(count_umac_floor_released(&release_msgs), 1);
    assert_eq!(count_d_releases(&release_msgs), 2);
    assert_eq!(count_umac_floor_granted(&release_msgs), 0);
}

#[test]
fn test_legacy_gssi_group_keeps_different_speaker_tail_handoff() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.legacy_gssi_group_call = true;
    let mut test = ComponentTest::from_config(config, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);
    let (call_id, _active_ts, _active_usage) = start_group_call_with_circuit_for(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);

    test.submit_message(build_u_tx_ceased_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let _ceased_start_msgs = test.dump_sinks();

    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_A, call_id));
    test.run_stack(Some(1));
    let _queue_msgs = test.dump_sinks();

    drain_group_tx_ceased_tail(&mut test, dltime);
    let handoff_msgs = test.dump_sinks();

    assert!(
        handoff_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::LcmcMleUnitdataReq(prim)
                if prim.main_address == TetraAddress::issi(LAB_ISSI_A)
                    && parse_d_tx_granted(prim).is_some_and(|pdu| pdu.transmission_grant == TransmissionGrant::Granted.into_raw() as u8)
        )),
        "legacy GSSI mode must keep ETSI queued handoff to a different speaker"
    );
    assert_eq!(count_d_tx_ceased(&handoff_msgs), 0);
    assert_eq!(count_d_releases(&handoff_msgs), 0);
}

#[test]
fn test_legacy_gssi_group_skips_stale_same_speaker_retake_when_later_speaker_is_queued() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.legacy_gssi_group_call = true;
    let mut test = ComponentTest::from_config(config, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_MXP600, LAB_GROUP_GSSI);
    let (call_id, _active_ts, _active_usage) = start_group_call_with_circuit_for(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);

    test.submit_message(build_u_tx_ceased_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let _ceased_start_msgs = test.dump_sinks();

    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let stale_retake_msgs = test.dump_sinks();
    assert!(
        stale_retake_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::LcmcMleUnitdataReq(prim)
                if prim.main_address == TetraAddress::issi(LAB_ISSI_B)
                    && parse_d_tx_granted(prim).is_some_and(|pdu| pdu.transmission_grant == TransmissionGrant::RequestQueued.into_raw() as u8)
        )),
        "legacy same-speaker retake should be held as queued during the tail"
    );

    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_A, call_id));
    test.run_stack(Some(1));
    let later_speaker_queue_msgs = test.dump_sinks();
    assert!(
        later_speaker_queue_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::LcmcMleUnitdataReq(prim)
                if prim.main_address == TetraAddress::issi(LAB_ISSI_A)
                    && parse_d_tx_granted(prim).is_some_and(|pdu| pdu.transmission_grant == TransmissionGrant::RequestQueued.into_raw() as u8)
        )),
        "later different speaker should also be queued while the old tail drains"
    );

    drain_group_tx_ceased_tail(&mut test, dltime);
    let handoff_msgs = test.dump_sinks();

    assert!(
        handoff_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::LcmcMleUnitdataReq(prim)
                if prim.main_address == TetraAddress::issi(LAB_ISSI_A)
                    && parse_d_tx_granted(prim).is_some_and(|pdu| pdu.transmission_grant == TransmissionGrant::Granted.into_raw() as u8)
        )),
        "legacy GSSI mode must skip stale same-speaker retake and preserve handoff to the later speaker"
    );
    assert!(
        !handoff_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::LcmcMleUnitdataReq(prim)
                if prim.main_address == TetraAddress::issi(LAB_ISSI_B)
                    && parse_d_tx_granted(prim).is_some_and(|pdu| pdu.transmission_grant == TransmissionGrant::Granted.into_raw() as u8)
        )),
        "legacy GSSI mode must not positively regrant the stale same-speaker retake"
    );
    assert_eq!(count_d_tx_ceased(&handoff_msgs), 0);
    assert_eq!(count_d_releases(&handoff_msgs), 0);
}

#[test]
fn test_group_tx_ceased_tail_drain_then_grants_requester_queued_during_tail() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit(&mut test);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_start_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&ceased_start_msgs), 0);
    assert_eq!(count_umac_floor_released(&ceased_start_msgs), 0);

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let queued_grant = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(TEST_CALLED_ISSI))
        .expect("requester PTT during group tail drain should be queued");
    assert_eq!(queued_grant.1.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        queued_grant.0,
        &queued_grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Dl,
        "group tail-drain requester queue response",
    );
    assert_eq!(count_umac_floor_released(&demand_msgs), 0);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);

    drain_group_tx_ceased_tail(&mut test, dltime);
    let tail_msgs = test.dump_sinks();

    let grants: Vec<_> = tail_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        grants.len(),
        2,
        "tail-drained group handoff should grant requester and inform listeners"
    );
    let requester_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(TEST_CALLED_ISSI))
        .expect("queued requester should receive the floor after tail drain");
    assert_eq!(requester_grant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        requester_grant.0,
        &requester_grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Both,
        "group tail-drain queued requester handoff",
    );
    assert_eq!(count_d_tx_ceased(&tail_msgs), 0);
    assert_eq!(count_umac_floor_released(&tail_msgs), 0);
    assert_eq!(
        count_umac_floor_granted(&tail_msgs),
        0,
        "tail-drained queued group handoff must wait until positive D-TX GRANTED is transmitted"
    );

    let requester_reporter = d_tx_granted_reporter(&tail_msgs, TetraAddress::issi(TEST_CALLED_ISSI), TransmissionGrant::Granted);
    assert_eq!(requester_reporter.get_state(), TxState::Pending);
    requester_reporter.mark_transmitted();
    test.run_stack(Some(1));
    let activation_msgs = test.dump_sinks();
    assert_eq!(count_umac_floor_granted(&activation_msgs), 1);
    assert!(activation_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ts,
            }) if *got_call_id == call_id
                && *source_issi == TEST_CALLED_ISSI
                && *dest_gssi == TEST_GSSI
                && *ts == active_ts
        )
    }));
}

#[test]
fn test_group_226333_same_speaker_retake_during_tx_ceased_tail_defers_positive_grant() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_MXP600, LAB_GROUP_GSSI);
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);

    test.submit_message(build_u_tx_ceased_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let ceased_start_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&ceased_start_msgs), 0);
    assert_eq!(count_umac_floor_released(&ceased_start_msgs), 0);

    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let retake_msgs = test.dump_sinks();

    let queued_grant = retake_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_B))
        .expect("same-speaker fast retake during group tail drain should receive a queued response");
    assert_eq!(queued_grant.1.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        queued_grant.0,
        &queued_grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Dl,
        "same-speaker group retake queued during TX-CEASED tail",
    );
    assert_eq!(
        count_umac_floor_granted(&retake_msgs),
        0,
        "same-speaker retake must not reopen U-plane before the previous U-TX CEASED tail settles"
    );
    assert_eq!(count_umac_floor_released(&retake_msgs), 0);
    assert_eq!(count_d_tx_ceased(&retake_msgs), 0);
    assert_eq!(count_d_setups(&retake_msgs), 0);
    assert_eq!(count_d_call_proceedings(&retake_msgs), 0);
    assert_eq!(count_d_connects(&retake_msgs), 0);
    assert_eq!(count_umac_open(&retake_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&retake_msgs), 0);

    drain_group_tx_ceased_tail(&mut test, dltime);
    let grant_msgs = test.dump_sinks();
    assert_eq!(
        count_d_setups(&grant_msgs),
        0,
        "same-speaker tail retake must not inject late-entry D-SETUP in the same burst as the deferred positive floor grant"
    );
    let grants: Vec<_> = grant_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();

    let requester_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_B))
        .expect("same-speaker retake should receive positive floor after tail drain");
    assert_eq!(requester_grant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        requester_grant.0,
        &requester_grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Both,
        "same-speaker group retake positive grant after TX-CEASED tail",
    );
    for listener_issi in [LAB_ISSI_A, LAB_ISSI_MXP600] {
        let listener_grant = grants
            .iter()
            .find(|(prim, _)| prim.main_address == TetraAddress::issi(listener_issi))
            .unwrap_or_else(|| panic!("listener ISSI {listener_issi} should be informed after same-speaker retake"));
        assert_eq!(
            listener_grant.1.transmission_grant,
            TransmissionGrant::GrantedToOtherUser.into_raw() as u8
        );
        assert_d_tx_granted_facch_allocation(
            listener_grant.0,
            &listener_grant.1,
            active_ts,
            active_usage,
            UlDlAssignment::Dl,
            "same-speaker group retake listener grant after TX-CEASED tail",
        );
    }
    assert_eq!(count_d_tx_ceased(&grant_msgs), 0);
    assert_eq!(count_umac_floor_released(&grant_msgs), 0);
    assert_eq!(
        count_umac_floor_granted(&grant_msgs),
        0,
        "positive group grant must not open U-plane until the D-TX GRANTED reporter is transmitted"
    );

    let requester_reporter = d_tx_granted_reporter(&grant_msgs, TetraAddress::issi(LAB_ISSI_B), TransmissionGrant::Granted);
    assert_eq!(requester_reporter.get_state(), TxState::Pending);
    requester_reporter.mark_transmitted();
    test.run_stack(Some(1));
    let activation_msgs = test.dump_sinks();
    assert_eq!(count_umac_floor_granted(&activation_msgs), 1);
    assert!(activation_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ts,
            }) if *got_call_id == call_id
                && *source_issi == LAB_ISSI_B
                && *dest_gssi == LAB_GROUP_GSSI
                && *ts == active_ts
        )
    }));

    test.run_stack(Some(2));
    let stale_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&stale_msgs), 0);
    assert_eq!(count_umac_floor_released(&stale_msgs), 0);
}

#[test]
fn test_group_226333_same_speaker_repeated_setup_during_tx_ceased_tail_defers_positive_grant() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit_for(&mut test, LAB_ISSI_B, LAB_GROUP_GSSI);

    test.submit_message(build_u_tx_ceased_msg(LAB_ISSI_B, call_id));
    test.run_stack(Some(1));
    let _ceased_start_msgs = test.dump_sinks();

    test.submit_message(build_u_setup_msg(LAB_ISSI_B, LAB_GROUP_GSSI));
    test.run_stack(Some(1));
    let repeated_setup_msgs = test.dump_sinks();

    let queued_grant = repeated_setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_B))
        .expect("same-speaker repeated U-SETUP during group tail drain should receive a queued response");
    assert_eq!(queued_grant.1.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        queued_grant.0,
        &queued_grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Dl,
        "same-speaker repeated U-SETUP queued during group TX-CEASED tail",
    );
    assert_eq!(count_d_setups(&repeated_setup_msgs), 0);
    assert_eq!(count_d_call_proceedings(&repeated_setup_msgs), 0);
    assert_eq!(count_d_connects(&repeated_setup_msgs), 0);
    assert_eq!(count_umac_floor_granted(&repeated_setup_msgs), 0);
    assert_eq!(count_umac_floor_released(&repeated_setup_msgs), 0);

    drain_group_tx_ceased_tail(&mut test, dltime);
    let grant_msgs = test.dump_sinks();
    let requester_grant = grant_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .find(|(prim, _)| prim.main_address == TetraAddress::issi(LAB_ISSI_B))
        .expect("same-speaker repeated U-SETUP should receive positive floor after tail drain");
    assert_eq!(requester_grant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_d_tx_granted_facch_allocation(
        requester_grant.0,
        &requester_grant.1,
        active_ts,
        active_usage,
        UlDlAssignment::Both,
        "same-speaker repeated U-SETUP positive grant after group TX-CEASED tail",
    );
    assert_eq!(count_d_tx_ceased(&grant_msgs), 0);
    assert_eq!(count_umac_floor_released(&grant_msgs), 0);
    assert_eq!(
        count_umac_floor_granted(&grant_msgs),
        0,
        "positive repeated-setup group grant must wait for RF transmission before U-plane activation"
    );

    let requester_reporter = d_tx_granted_reporter(&grant_msgs, TetraAddress::issi(LAB_ISSI_B), TransmissionGrant::Granted);
    assert_eq!(requester_reporter.get_state(), TxState::Pending);
    requester_reporter.mark_transmitted();
    test.run_stack(Some(1));
    let activation_msgs = test.dump_sinks();
    assert_eq!(count_umac_floor_granted(&activation_msgs), 1);
}

#[test]
fn test_group_hangtime_tx_demand_defers_late_entry_d_setup_refresh() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let (call_id, active_ts, active_usage) = start_group_call_with_circuit(&mut test);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let _hangtime_msgs = test.dump_sinks();
    drain_group_tx_ceased_tail(&mut test, dltime);
    let _tail_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 14.5.2.1.1/14.5.2.1.2 and Annex D allow back-up
    // D-SETUP for group-call setup/late entry; clause 14.5.2.2.1 moves the
    // active floor with D-TX GRANTED. A new U-TX DEMAND after hangtime must
    // not inject D-SETUP in the immediate floor-grant burst, but the cached
    // back-up D-SETUP still has to advertise the new speaker when the late
    // entry scheduler sends it later.
    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    assert_eq!(
        count_d_setups(&demand_msgs),
        0,
        "hangtime floor retake must not inject an immediate back-up D-SETUP over the first speech frames"
    );

    let grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(grants.len(), 2);
    assert!(grants.iter().any(|(prim, grant)| {
        prim.main_address == TetraAddress::issi(TEST_CALLED_ISSI) && grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8
    }));
    assert!(grants.iter().any(|(prim, grant)| {
        prim.main_address == TetraAddress::issi(TEST_ISSI)
            && grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    }));
    assert_no_group_d_info_reset_t310(&demand_msgs, "hangtime floor retake");
    assert_eq!(
        count_umac_floor_granted(&demand_msgs),
        0,
        "hangtime floor retake must wait until positive D-TX GRANTED is transmitted"
    );

    let requester_reporter = d_tx_granted_reporter(&demand_msgs, TetraAddress::issi(TEST_CALLED_ISSI), TransmissionGrant::Granted);
    assert_eq!(requester_reporter.get_state(), TxState::Pending);
    requester_reporter.mark_transmitted();
    test.run_stack(Some(1));
    let activation_msgs = test.dump_sinks();
    assert_eq!(count_umac_floor_granted(&activation_msgs), 1);

    run_group_late_entry_resend_tick(&mut test, dltime);
    let backup_msgs = test.dump_sinks();
    let setup_refreshes: Vec<_> = backup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert!(
        !setup_refreshes.is_empty(),
        "deferred late-entry D-SETUP should still be sent after the immediate floor-grant burst"
    );
    let (setup_refresh_prim, setup_refresh) = &setup_refreshes[0];
    assert_eq!(setup_refresh.call_identifier, call_id);
    assert_eq!(setup_refresh.calling_party_address_ssi, Some(TEST_CALLED_ISSI));
    assert_eq!(setup_refresh.transmission_grant, TransmissionGrant::GrantedToOtherUser);
    assert!(!setup_refresh.transmission_request_permission);
    assert_eq!(setup_refresh_prim.main_address, TetraAddress::new(TEST_GSSI, SsiType::Gssi));
    let setup_refresh_alloc = setup_refresh_prim
        .chan_alloc
        .as_ref()
        .expect("deferred group D-SETUP refresh should carry channel allocation");
    assert_chan_alloc_matches_circuit(
        setup_refresh_alloc,
        active_ts,
        active_usage,
        "deferred hangtime retake D-SETUP refresh",
    );
    assert_eq!(setup_refresh_alloc.ul_dl_assigned, UlDlAssignment::Both);
}

#[test]
fn test_group_release_sends_facch_release_and_mcch_fallback_before_close() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let release_msgs = test.dump_sinks();

    let d_release_prims: Vec<_> = release_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_release(prim).is_some() => Some(prim),
            _ => None,
        })
        .collect();
    assert_eq!(d_release_prims.len(), 2, "Expected FACCH D-RELEASE plus MCCH fallback");
    for prim in &d_release_prims {
        let d_release = parse_d_release(prim).expect("D-RELEASE should parse");
        assert_eq!(d_release.call_identifier, call_id);
        assert_eq!(d_release.disconnect_cause, DisconnectCause::UserRequestedDisconnection);
        assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    }

    let facch = d_release_prims
        .iter()
        .find(|prim| prim.stealing_permission && prim.chan_alloc.is_some())
        .expect("Expected FACCH/STCH D-RELEASE");
    assert_eq!(facch.main_address.ssi, TEST_GSSI);
    assert_eq!(facch.main_address.ssi_type, SsiType::Gssi);
    assert!(facch.tx_reporter.is_some(), "FACCH release must be reporter-tracked");
    let chan_alloc = facch.chan_alloc.as_ref().expect("FACCH release needs channel allocation");
    assert_eq!(chan_alloc.usage, Some(4));
    assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Dl);

    let mcch = d_release_prims
        .iter()
        .find(|prim| !prim.stealing_permission && prim.chan_alloc.is_none())
        .expect("Expected MCCH D-RELEASE fallback");
    assert_eq!(mcch.main_address.ssi, TEST_GSSI);
    assert_eq!(mcch.main_address.ssi_type, SsiType::Gssi);
    assert!(mcch.tx_reporter.is_none());

    assert_eq!(
        count_umac_call_ended_or_close(&release_msgs),
        0,
        "Group circuit must stay open until D-RELEASE is reported or guard timeout expires"
    );
}

#[test]
fn test_group_call_timeout_sends_expiry_of_timer_release_cause() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.call_timeout_secs = 1;
    let mut test = ComponentTest::from_config(config, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    // call_timeout_secs=1 maps to CallTimeout::T30s. EN 300 392-2
    // clause 14.5.2.3.5 T310 expiry requires "expiry of timer".
    test.router.set_dl_time(dltime.add_timeslots(30 * 18 * 4 + 1));
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();

    let releases: Vec<_> = release_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        releases.len(),
        2,
        "group call timeout should emit FACCH D-RELEASE plus MCCH fallback"
    );
    for (prim, release) in &releases {
        assert_eq!(release.call_identifier, call_id);
        assert_eq!(release.disconnect_cause, DisconnectCause::ExpiryOfTimer);
        assert_eq!(prim.main_address.ssi, TEST_GSSI);
        assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
        assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    }
    assert!(
        releases
            .iter()
            .any(|(prim, _)| prim.stealing_permission && prim.chan_alloc.is_some() && prim.tx_reporter.is_some()),
        "FACCH/STCH timeout D-RELEASE should stay reporter-tracked"
    );
    assert!(
        releases
            .iter()
            .any(|(prim, _)| !prim.stealing_permission && prim.chan_alloc.is_none() && prim.tx_reporter.is_none()),
        "timeout release should keep an untracked MCCH fallback"
    );
    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 1, "Only FACCH D-RELEASE should carry a reporter");
    assert_eq!(
        count_umac_call_ended_or_close(&release_msgs),
        0,
        "Group circuit must stay open until timeout D-RELEASE delivery is reported"
    );
}

#[test]
fn test_network_group_call_timeout_reports_network_end_after_expiry_release_delivery() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(test_brew_config());
    config.cell.call_timeout_secs = 1;
    let mut test = ComponentTest::from_config(config, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    let brew_uuid = uuid::Uuid::new_v4();
    let (call_id, _ts, _setup_msgs) = start_network_group_call(&mut test, brew_uuid, TEST_CALLED_ISSI, TEST_GSSI, 7);

    // T310/call timeout is a network-side release for this SwMI. The external
    // network callback must wait until the group D-RELEASE is reported sent.
    test.router.set_dl_time(dltime.add_timeslots(30 * 18 * 4 + 1));
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();

    let releases: Vec<_> = release_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        releases.len(),
        2,
        "network group timeout should emit FACCH D-RELEASE plus MCCH fallback"
    );
    for (_, release) in &releases {
        assert_eq!(release.call_identifier, call_id);
        assert_eq!(release.disconnect_cause, DisconnectCause::ExpiryOfTimer);
    }

    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 1, "Only FACCH D-RELEASE should be reporter-tracked");
    assert_eq!(count_network_call_end(&release_msgs, brew_uuid), 0);
    assert_eq!(
        count_umac_call_ended_or_close(&release_msgs),
        0,
        "Network-origin group timeout must not close before D-RELEASE is reported"
    );

    test.run_stack(Some(2));
    let duplicate_timer_msgs = test.dump_sinks();
    assert_eq!(
        count_d_releases(&duplicate_timer_msgs),
        0,
        "pending call-timeout release must not resend D-RELEASE on every CMCE timer tick"
    );
    assert_eq!(
        count_network_call_end(&duplicate_timer_msgs, brew_uuid),
        0,
        "pending call-timeout release must wait for reporter completion before notifying Brew"
    );
    assert_eq!(
        count_umac_call_ended_or_close(&duplicate_timer_msgs),
        0,
        "pending call-timeout release must keep the traffic circuit open until reporter completion"
    );

    reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "Reporter completion should close the timed-out network group circuit"
    );
    assert_eq!(count_network_call_end(&closed_msgs, brew_uuid), 1);
}

#[test]
fn test_group_release_waits_for_release_reporter_before_circuit_close() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();
    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 1, "Only FACCH D-RELEASE should carry a reporter");
    assert_eq!(reporters[0].get_state(), TxState::Pending);
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);

    test.run_stack(Some(3));
    let pending_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_call_ended_or_close(&pending_msgs),
        0,
        "Pending group release should not close before reporter completion"
    );

    reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "Reporter completion should close the circuit and signal CallEnded"
    );
}

#[test]
fn test_group_release_discarded_reporter_does_not_close_before_guard_timeout() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();
    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 1, "Only FACCH D-RELEASE should carry a reporter");

    // EN 300 392-2 clause 14.5.2.3.2 says the SwMI sends D-RELEASE and then
    // releases the call. A local discard is not evidence that D-RELEASE was
    // sent, so it must not close the assigned circuit before the guard expires.
    reporters[0].mark_discarded();
    test.run_stack(Some(1));
    let discarded_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_call_ended_or_close(&discarded_msgs),
        0,
        "Discarded D-RELEASE reporter must not be treated as delivered"
    );

    test.run_stack(Some(20));
    let still_pending_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_call_ended_or_close(&still_pending_msgs),
        0,
        "Energy-economy-aware group release guard must not close after the old short timeout"
    );

    test.run_stack(Some(1420));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "Local guard timeout should eventually close a discarded group release"
    );
}

#[test]
fn test_group_release_closes_after_bounded_pending_release_timeout() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let release_msgs = test.dump_sinks();
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);

    test.run_stack(Some(20));
    let still_pending_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_call_ended_or_close(&still_pending_msgs),
        0,
        "Energy-economy-aware group release guard must not close after the old short timeout"
    );

    test.run_stack(Some(1420));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "Local guard timeout should eventually close a stuck pending group release"
    );
}

#[test]
fn test_group_release_pending_ignores_duplicate_release_without_extra_signalling() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let release_msgs = test.dump_sinks();
    assert_eq!(
        count_d_releases(&release_msgs),
        2,
        "initial group release should send FACCH plus MCCH D-RELEASE"
    );
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);

    // EN 300 392-2 clause 14.5.2.3 D-RELEASE receives no response from the
    // MS. Once SwMI release is pending, duplicate uplink release/disconnect
    // indications must not create duplicate D-RELEASEs or close the circuit early.
    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let duplicate_disconnect_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&duplicate_disconnect_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&duplicate_disconnect_msgs), 0);

    test.submit_message(build_u_release_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let duplicate_release_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&duplicate_release_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&duplicate_release_msgs), 0);
}

#[test]
fn test_group_pending_release_ignores_non_owner_disconnect_release_without_extra_signalling() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let release_msgs = test.dump_sinks();
    assert_eq!(
        count_d_releases(&release_msgs),
        2,
        "initial group release should send FACCH plus MCCH D-RELEASE"
    );
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);

    // EN 300 392-2 clause 14.5.2.3 clears the group call with D-RELEASE and
    // does not require an MS response. While the old D-RELEASE is pending, late
    // non-owner release/disconnect PDUs for the same call id are stale local
    // traffic and must not create service-unavailable D-RELEASEs.
    test.submit_message(build_u_disconnect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let non_owner_disconnect_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&non_owner_disconnect_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&non_owner_disconnect_msgs), 0);

    test.submit_message(build_u_release_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let non_owner_release_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&non_owner_release_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&non_owner_release_msgs), 0);
}

#[test]
fn test_group_release_pending_allows_same_gssi_restart_while_old_release_drains() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let (call_id, old_ts, _old_usage) = start_group_call_with_circuit(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let release_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&release_msgs), 2);
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);

    // EN 300 392-2 clause 14.5.2.3 clears the old group call with D-RELEASE,
    // and clause 14.5.2.1 permits the next normal group setup. While the
    // local FACCH D-RELEASE is still draining, a same-GSSI restart must not be
    // rejected as unsupported or inherit stale release state.
    test.submit_message(build_u_setup_msg(TEST_CALLED_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let restart_msgs = test.dump_sinks();

    assert_eq!(
        count_d_releases(&restart_msgs),
        0,
        "same-GSSI restart during stale pending release must not receive service-unavailable D-RELEASE"
    );
    assert_eq!(count_d_call_proceedings(&restart_msgs), 1);
    assert_eq!(count_d_connects(&restart_msgs), 1);
    assert_eq!(count_d_setups(&restart_msgs), 1, "restart should send a fresh group D-SETUP");
    assert_eq!(count_umac_open(&restart_msgs), 1, "restart should open one replacement circuit");
    assert_eq!(
        count_umac_call_ended_or_close(&restart_msgs),
        0,
        "restart must not close the stale pending-release circuit before D-RELEASE reporter/guard completion"
    );
    let replacement_circuit = restart_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .expect("restart should open one replacement circuit");
    assert_ne!(
        replacement_circuit.ts, old_ts,
        "restart must use a different traffic slot from the pending release"
    );

    let new_call_id = first_d_setup_call_id(&restart_msgs);
    assert_ne!(new_call_id, call_id, "restart should allocate a fresh group call id");
}

#[test]
fn test_group_pending_release_call_id_wrap_skips_old_release_id() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let (old_call_id, _old_ts, _old_usage) = start_group_call_with_circuit(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, old_call_id));
    test.run_stack(Some(1));
    let release_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&release_msgs), 2);
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);

    force_cmce_next_call_identifier(&mut test, old_call_id);
    test.submit_message(build_u_setup_msg(TEST_CALLED_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let restart_msgs = test.dump_sinks();

    let new_call_id = first_d_setup_call_id(&restart_msgs);
    assert_ne!(
        new_call_id, old_call_id,
        "EN 300 392-2 clause 14.2.3/table 14.36: a fresh group setup must skip an old call id while D-RELEASE is still pending"
    );
    assert_eq!(count_d_releases(&restart_msgs), 0);
    assert_eq!(count_umac_open(&restart_msgs), 1);

    let occupied_ids = cmce_debug_active_call_ids(&mut test);
    assert!(
        occupied_ids.contains(&old_call_id),
        "old pending group release id should stay occupied"
    );
    assert!(occupied_ids.contains(&new_call_id), "replacement group call id should be occupied");
}

#[test]
fn test_group_release_pending_ignores_late_floor_pdus_without_extra_signalling() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let release_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&release_msgs), 2);
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);

    // EN 300 392-2 14.5.2.3 clears the group call with D-RELEASE and expects no
    // response. During the local FACCH delivery drain, do not emit new floor
    // signalling that would contradict the pending release.
    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_granted(&demand_msgs), 0);
    assert_eq!(count_d_tx_ceased(&demand_msgs), 0);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
    assert_eq!(count_umac_floor_released(&demand_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&demand_msgs), 0);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_granted(&ceased_msgs), 0);
    assert_eq!(count_d_tx_ceased(&ceased_msgs), 0);
    assert_eq!(count_umac_floor_granted(&ceased_msgs), 0);
    assert_eq!(count_umac_floor_released(&ceased_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&ceased_msgs), 0);
}

#[test]
fn test_group_pending_release_large_ptt_flood_is_ignored_without_signalling() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    let member_count = LARGE_GSSI_MEMBER_COUNT;
    let first_issi = 430_000_u32;
    for offset in 0..member_count {
        let issi = first_issi + offset;
        submit_subscriber_update(&mut test, issi, Vec::new(), BrewSubscriberAction::Register);
        submit_subscriber_update(&mut test, issi, vec![TEST_GSSI], BrewSubscriberAction::Affiliate);
    }
    test.run_stack(Some((member_count as usize * 2) + 16));
    let _ = test.dump_sinks();

    let speaker_issi = first_issi;
    let (call_id, _ts, _usage) = start_group_call_with_circuit_for(&mut test, speaker_issi, TEST_GSSI);

    test.submit_message(build_u_disconnect_msg(speaker_issi, call_id));
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();
    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 1, "large pending release should track FACCH delivery");
    assert_eq!(count_d_releases(&release_msgs), 2);
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);

    for offset in 1..member_count {
        test.submit_message(build_u_tx_demand_msg(first_issi + offset, call_id));
    }
    test.deliver_all_messages();
    let flood_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.2.3 clears the group call with D-RELEASE.
    // While that release is pending, clause 14.5.2.2.1 floor-control traffic
    // for the old call must not restart turn taking or emit contradictory
    // queued/not-granted responses for thousands of late contenders.
    assert_eq!(count_d_tx_granted(&flood_msgs), 0);
    assert_eq!(count_d_tx_ceased(&flood_msgs), 0);
    assert_eq!(count_d_releases(&flood_msgs), 0);
    assert_eq!(count_umac_floor_granted(&flood_msgs), 0);
    assert_eq!(count_umac_floor_released(&flood_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&flood_msgs), 0);

    let occupied_ids = cmce_debug_active_call_ids(&mut test);
    assert!(
        occupied_ids.contains(&call_id),
        "large late-PTT flood must not evict the pending group release call id"
    );

    reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "large late-PTT flood must not prevent pending D-RELEASE completion"
    );
    assert!(
        !cmce_debug_active_call_ids(&mut test).contains(&call_id),
        "pending group release call id should be freed after reporter completion"
    );
}

#[test]
fn test_group_release_pending_allows_network_restart_while_old_release_drains() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(CfgBrew {
        host: "test-brew.local".to_string(),
        port: 443,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: std::time::Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        feature_rssi_export: false,
        whitelisted_ssis: None,
    });
    let mut test = ComponentTest::from_config(config, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    let (old_call_id, old_ts, _old_usage) = start_group_call_with_circuit(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, old_call_id));
    test.run_stack(Some(1));
    let release_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&release_msgs), 2);
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);

    let brew_uuid = uuid::Uuid::new_v4();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: TEST_CALLED_ISSI,
            dest_gssi: TEST_GSSI,
            priority: 7,
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();

    assert_eq!(
        count_network_call_end(&setup_msgs, brew_uuid),
        0,
        "Brew restart during stale pending release must not be dropped"
    );
    assert_eq!(count_d_releases(&setup_msgs), 0);
    assert_eq!(count_d_setups(&setup_msgs), 1, "restart should send a fresh group D-SETUP");
    assert_eq!(count_umac_open(&setup_msgs), 1, "restart should open one replacement circuit");
    assert_eq!(
        count_umac_call_ended_or_close(&setup_msgs),
        0,
        "restart must not close the stale pending-release circuit before D-RELEASE reporter/guard completion"
    );
    assert!(
        network_group_ready_tuple(&setup_msgs, brew_uuid).is_none(),
        "network restart must wait for fresh RF D-SETUP transmission before Brew media ready"
    );

    let replacement_circuit = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .expect("network restart should open one replacement circuit");
    assert_ne!(
        replacement_circuit.ts, old_ts,
        "restart must use a different traffic slot from the pending release"
    );

    let new_call_id = first_d_setup_call_id(&setup_msgs);
    assert_ne!(new_call_id, old_call_id, "restart should allocate a fresh group call id");

    let reporter = first_d_setup_reporter(&setup_msgs);
    reporter.mark_transmitted();
    test.run_stack(Some(1));
    let ready_msgs = test.dump_sinks();
    assert_eq!(
        network_group_ready_tuple(&ready_msgs, brew_uuid),
        Some((new_call_id, replacement_circuit.ts)),
        "Brew media should become ready after the fresh D-SETUP reaches RF"
    );
}

#[test]
fn test_group_u_release_from_non_owner_is_rejected_without_group_release() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_release_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let non_owner_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.2.3.1 permits the SwMI to reject a
    // non-call-owner disconnection request without clearing the group call.
    let d_release_prims: Vec<_> = non_owner_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_release(prim).is_some() => Some(prim),
            _ => None,
        })
        .collect();
    assert_eq!(d_release_prims.len(), 1, "Expected one individual D-RELEASE rejection");
    let prim = d_release_prims[0];
    assert_eq!(prim.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert!(!prim.stealing_permission);
    assert!(prim.chan_alloc.is_none());
    let d_release = parse_d_release(prim).expect("D-RELEASE should parse");
    assert_eq!(d_release.call_identifier, call_id);
    assert_eq!(d_release.disconnect_cause, DisconnectCause::RequestedServiceNotAvailable);
    assert_eq!(count_umac_call_ended_or_close(&non_owner_msgs), 0);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let owner_msgs = test.dump_sinks();
    assert_eq!(
        count_d_releases(&owner_msgs),
        2,
        "Owner U-DISCONNECT should still clear the active group call after non-owner rejection"
    );
}

#[test]
fn test_group_floor_holder_without_d_info_ownership_cannot_disconnect_call() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_start_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&ceased_start_msgs), 0);
    assert_eq!(count_umac_floor_released(&ceased_start_msgs), 0);

    drain_group_tx_ceased_tail(&mut test, dltime);
    let ceased_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&ceased_msgs), 1);
    assert_eq!(count_umac_floor_released(&ceased_msgs), 1);

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert!(count_d_tx_granted(&demand_msgs) >= 1);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
    let activation_msgs = transmit_positive_group_grants_and_drain(&mut test, &demand_msgs);
    assert_eq!(count_umac_floor_granted(&activation_msgs), 1);

    test.submit_message(build_u_disconnect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let non_owner_msgs = test.dump_sinks();
    let releases: Vec<_> = non_owner_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        releases.len(),
        1,
        "floor holder without ownership should receive one direct rejection"
    );
    let (release_prim, release) = &releases[0];
    assert_eq!(release_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(release_prim.main_address.ssi_type, SsiType::Issi);
    assert!(!release_prim.stealing_permission);
    assert!(release_prim.chan_alloc.is_none());
    assert_eq!(release.call_identifier, call_id);
    assert_eq!(release.disconnect_cause, DisconnectCause::RequestedServiceNotAvailable);
    assert_eq!(count_umac_call_ended_or_close(&non_owner_msgs), 0);

    // EN 300 392-2 clauses 14.5.2.3.1 and 14.5.2.7: transmission permission
    // and call ownership are separate. Without D-INFO ownership transfer, the
    // original MO group-call owner remains the party allowed to clear the call.
    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let owner_msgs = test.dump_sinks();
    assert_eq!(
        count_d_releases(&owner_msgs),
        2,
        "original call owner U-DISCONNECT should still initiate group D-RELEASE"
    );
}

#[test]
fn test_unsolicited_group_owner_u_release_does_not_disconnect_active_group_call() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    let call_id = start_group_call(&mut test);

    // EN 300 392-2 clause 14.5.2.3.1 uses U-DISCONNECT for owner-initiated
    // group disconnection. U-RELEASE is not the active-call release request.
    test.submit_message(build_u_release_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let release_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&release_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let disconnect_msgs = test.dump_sinks();
    assert_eq!(
        count_d_releases(&disconnect_msgs),
        2,
        "U-DISCONNECT should still initiate group D-RELEASE after unsolicited U-RELEASE was ignored"
    );
}

#[test]
fn test_p2p_preemptive_u_setup_default_off_starts_call_setup() {
    debug::setup_logging_verbose();

    // EN 300 392-2 table 14.46 defines call priorities 12..=15 as
    // pre-emptive. Accepting the call priority in U-SETUP is not the same as
    // active transmission interruption; default-off still only affects later
    // U-TX DEMAND priority 2/3 interruption.
    for priority in 12..=15 {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
        test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

        let mut u_setup = default_p2p_u_setup();
        u_setup.call_priority = priority;
        u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);

        let (_call_id, msgs) = start_p2p_setup_with_u_setup(&mut test, u_setup);

        let setup = msgs
            .iter()
            .find_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim),
                _ => None,
            })
            .expect("pre-emptive-priority P2P U-SETUP should emit D-SETUP");
        assert_eq!(setup.call_priority, priority);
        assert_eq!(count_d_releases(&msgs), 0, "priority {priority}");
        assert_eq!(count_d_setups(&msgs), 1, "priority {priority}");
        assert_eq!(count_umac_open(&msgs), 0, "priority {priority}");
    }
}

#[test]
fn test_p2p_preemptive_u_setup_interruption_enabled_starts_call_setup() {
    debug::setup_logging_verbose();

    // EN 300 392-2 table 14.46 pre-emptive call priority is preserved in the
    // private D-SETUP. Actual stop-transmission pre-emption is driven later by
    // U-TX DEMAND priority 2/3 under clause 14.5.1.2.1 f).
    for priority in 12..=15 {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
        config.cell.transmission_interruption_enabled = true;
        let mut test = ComponentTest::from_config(config, Some(dltime));
        test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

        let mut u_setup = default_p2p_u_setup();
        u_setup.call_priority = priority;
        u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);

        let (_call_id, msgs) = start_p2p_setup_with_u_setup(&mut test, u_setup);

        let setup = msgs
            .iter()
            .find_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim),
                _ => None,
            })
            .expect("pre-emptive-priority P2P U-SETUP should emit D-SETUP");
        assert_eq!(setup.call_priority, priority);
        assert_eq!(count_d_releases(&msgs), 0, "priority {priority}");
        assert_eq!(count_d_setups(&msgs), 1, "priority {priority}");
        assert_eq!(count_umac_open(&msgs), 0, "priority {priority}");
    }
}

#[test]
fn test_p2p_priority_11_default_off_starts_call_setup() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.call_priority = 11;
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);

    // EN 300 392-2 table 14.46 keeps priority 11 below the pre-emptive
    // 12..=15 range. Default-off interruption support must not block a normal
    // private call at this boundary.
    let (_call_id, msgs) = start_p2p_setup_with_u_setup(&mut test, u_setup);

    let setup = msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim),
            _ => None,
        })
        .expect("priority 11 P2P U-SETUP should emit D-SETUP");
    assert_eq!(setup.call_priority, 11);
    assert_eq!(count_d_releases(&msgs), 0);
    assert_eq!(count_d_setups(&msgs), 1);
}

#[test]
fn test_p2p_sna_u_setup_with_brew_disabled_rejects_with_dummy_call_id() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_type_identifier = PartyTypeIdentifier::Sna;
    u_setup.called_party_ssi = None;
    u_setup.called_party_short_number_address = Some(42);

    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.3.2 says the SwMI should send D-RELEASE
    // when it cannot support an individual call request. As first response,
    // clause 14.5.1.1.2 uses the dummy call reference, defined as zero in
    // clause 3.1.
    assert_p2p_setup_rejected_with_dummy_call_id(&msgs, TEST_ISSI);
}

#[test]
fn test_p2p_tsi_u_setup_with_foreign_mni_rejects_without_local_routing() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_type_identifier = PartyTypeIdentifier::Tsi;
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    u_setup.called_party_extension = Some(tsi_extension(205, 1337));

    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // EN 300 392-2 table 14.41 makes the TSI extension the MCC+MNC
    // component. A foreign TSI must not be collapsed onto a local ISSI that
    // happens to share the same 24-bit SSI.
    assert_p2p_setup_rejected_with_dummy_call_id(&msgs, TEST_ISSI);
    assert!(
        !msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { .. })
        )),
        "foreign TSI should not be forwarded through legacy Brew routing without full TSI support"
    );
}

#[test]
fn test_p2p_tsi_u_setup_with_local_mni_routes_to_local_issi() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_type_identifier = PartyTypeIdentifier::Tsi;
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    u_setup.called_party_extension = Some(tsi_extension(204, 1337));

    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    assert!(count_d_setups(&msgs) > 0, "local TSI should route to the local called ISSI");
    assert_eq!(count_umac_open(&msgs), 0, "P2P setup must not open traffic before U-CONNECT");
    assert!(
        !msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { .. })
        )),
        "local TSI should not be forwarded to Brew when the local ISSI is registered"
    );
}

#[test]
fn test_p2p_reserved_u_setup_without_called_party_rejects_with_dummy_call_id() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(CfgBrew {
        host: "test-brew.local".to_string(),
        port: 443,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: std::time::Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        feature_rssi_export: false,
        whitelisted_ssis: None,
    });
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_type_identifier = PartyTypeIdentifier::Reserved;
    u_setup.called_party_ssi = None;
    u_setup.called_party_short_number_address = None;
    u_setup.called_party_extension = None;

    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // CPTI=Reserved carries no called-party SSI or number after parsing. Even
    // with Brew configured, this is still an unsupported first-response setup
    // case and should not silently vanish.
    assert_p2p_setup_rejected_with_dummy_call_id(&msgs, TEST_ISSI);
}

#[test]
fn test_p2p_busy_called_party_rejects_with_dummy_call_id() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    register_subscriber(&mut test, TEST_OTHER_ISSI, TEST_GSSI);
    let (active_call_id, _setup_msgs) = start_p2p_setup(&mut test);

    test.submit_message(build_u_setup_p2p_msg(TEST_OTHER_ISSI, TEST_CALLED_ISSI));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.3.2 says the SwMI should send D-RELEASE
    // when it cannot support an individual call request. Before a new SwMI
    // call identity exists, clause 14.5.1.1.2 uses the dummy call reference
    // rather than borrowing the busy call's active identity.
    assert_p2p_setup_rejected_with_dummy_call_id_and_cause(&msgs, TEST_OTHER_ISSI, DisconnectCause::CalledPartyBusy);
    let release = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim),
            _ => None,
        })
        .next()
        .expect("busy reject should include D-RELEASE");
    assert_ne!(release.call_identifier, active_call_id);
}

#[test]
fn test_p2p_busy_calling_party_rejects_with_dummy_call_id() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    register_subscriber(&mut test, TEST_OTHER_ISSI, TEST_CALLED_GSSI);
    let _call_id = start_p2p_setup(&mut test).0;

    test.submit_message(build_u_setup_p2p_msg(TEST_ISSI, TEST_OTHER_ISSI));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.1.2 starts outgoing private setup from an
    // idle CC sub-entity. A caller already in an individual call has no idle
    // entity for a second setup, so reject before allocating another call id.
    assert_p2p_setup_rejected_with_dummy_call_id_and_cause(&msgs, TEST_ISSI, DisconnectCause::NoIdleCcEntity);
}

#[test]
fn test_p2p_pending_individual_release_remains_busy_until_reporter_completion() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    register_subscriber(&mut test, TEST_OTHER_ISSI, TEST_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut initiator_release_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus_to(
        &initiator_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_ISSI],
    );
    assert_no_d_info(&initiator_release_msgs);
    assert_release_notification_to(&initiator_release_msgs, TEST_ISSI, None);
    assert_eq!(count_d_disconnects(&initiator_release_msgs), 0);
    let release_ack_reporters = extract_d_release_reporters(&mut initiator_release_msgs);
    assert_eq!(release_ack_reporters.len(), 1);

    drain_private_simplex_tail(&mut test, dltime);
    let mut peer_release_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&peer_release_msgs), 0);
    assert_established_p2p_release_pdus_to(
        &peer_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_CALLED_ISSI],
    );
    assert_no_d_info(&peer_release_msgs);
    assert_release_notification_to(&peer_release_msgs, TEST_CALLED_ISSI, None);
    let peer_release_reporters = extract_d_release_reporters(&mut peer_release_msgs);
    assert_eq!(peer_release_reporters.len(), 1);

    peer_release_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let delivery_msgs = test.dump_sinks();
    assert_eq!(
        count_d_disconnects(&delivery_msgs),
        0,
        "Peer D-RELEASE delivery must not emit D-DISCONNECT"
    );
    assert_eq!(
        count_umac_call_ended_or_close(&delivery_msgs),
        0,
        "P2P circuit must stay open while initiator D-RELEASE delivery is pending"
    );

    // EN 300 392-2 clauses 14.5.1.1.2 and 14.5.1.3.1: while
    // the established private call is being released with pending
    // assigned-channel D-RELEASE delivery, neither party has an idle CC
    // sub-entity for another private setup.
    test.submit_message(build_u_setup_p2p_msg(TEST_OTHER_ISSI, TEST_CALLED_ISSI));
    test.run_stack(Some(1));
    let called_busy_msgs = test.dump_sinks();
    assert_p2p_setup_rejected_with_dummy_call_id_and_cause(&called_busy_msgs, TEST_OTHER_ISSI, DisconnectCause::CalledPartyBusy);

    test.submit_message(build_u_setup_p2p_msg(TEST_ISSI, TEST_OTHER_ISSI));
    test.run_stack(Some(1));
    let caller_busy_msgs = test.dump_sinks();
    assert_p2p_setup_rejected_with_dummy_call_id_and_cause(&caller_busy_msgs, TEST_ISSI, DisconnectCause::NoIdleCcEntity);

    for reporter in &release_ack_reporters {
        reporter.mark_transmitted();
    }
    test.run_stack(Some(8));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "Reporter completion should close the pending P2P release"
    );

    test.submit_message(build_u_setup_p2p_msg(TEST_OTHER_ISSI, TEST_CALLED_ISSI));
    test.run_stack(Some(1));
    let next_setup_msgs = test.dump_sinks();
    assert_eq!(count_d_setups(&next_setup_msgs), 1);
    assert_eq!(count_d_releases(&next_setup_msgs), 0);
}

#[test]
fn test_p2p_pending_release_call_id_wrap_skips_old_release_id() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let new_caller = TEST_OTHER_ISSI + 50;
    let new_called = TEST_OTHER_ISSI + 51;
    submit_subscriber_update(&mut test, new_caller, Vec::new(), BrewSubscriberAction::Register);
    submit_subscriber_update(&mut test, new_called, Vec::new(), BrewSubscriberAction::Register);
    test.run_stack(Some(4));
    let _ = test.dump_sinks();

    let old_call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, old_call_id));
    test.run_stack(Some(1));
    let initiator_release_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus_to(
        &initiator_release_msgs,
        old_call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_ISSI],
    );
    assert_eq!(count_umac_call_ended_or_close(&initiator_release_msgs), 0);

    drain_private_simplex_tail(&mut test, dltime);
    let peer_release_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&peer_release_msgs), 0);
    assert_established_p2p_release_pdus_to(
        &peer_release_msgs,
        old_call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_CALLED_ISSI],
    );
    assert_eq!(count_umac_call_ended_or_close(&peer_release_msgs), 0);

    force_cmce_next_call_identifier(&mut test, old_call_id);
    let (new_call_id, setup_msgs) = start_p2p_setup_between(&mut test, new_caller, new_called);

    assert_ne!(
        new_call_id, old_call_id,
        "EN 300 392-2 clauses 14.2.3/table 14.36 and 14.5.1.3: a fresh P2P setup must skip an old call id while private release is pending"
    );
    assert_eq!(count_d_setups(&setup_msgs), 1);
    assert_eq!(count_umac_open(&setup_msgs), 0);
    assert_eq!(count_d_releases(&setup_msgs), 0);

    let occupied_ids = cmce_debug_active_call_ids(&mut test);
    assert!(
        occupied_ids.contains(&old_call_id),
        "old pending P2P release id should stay occupied"
    );
    assert!(occupied_ids.contains(&new_call_id), "fresh P2P setup id should be occupied");
}

#[test]
fn test_p2p_busy_calling_party_echo_setup_rejects_with_dummy_call_id() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let _call_id = start_p2p_setup(&mut test).0;

    let mut echo_setup = default_p2p_u_setup();
    echo_setup.called_party_ssi = Some(999);
    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, echo_setup));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // The local echo ISSI is still a private-call setup target. EN 300 392-2
    // clause 14.5.1.1.2 idle-state gating applies before local service
    // routing, so echo must not create a parallel call for a busy caller.
    assert_p2p_setup_rejected_with_dummy_call_id_and_cause(&msgs, TEST_ISSI, DisconnectCause::NoIdleCcEntity);
}

#[test]
fn test_p2p_setup_to_parrot_99999_opens_separate_local_simplex_service() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let (telemetry_sink, telemetry_source) = telemetry_channel();
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.register_entity(CmceBs::new(test.config.clone(), Some(telemetry_sink), None));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew]);
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let mut setup = default_p2p_u_setup();
    setup.called_party_ssi = Some(99_999);
    setup.hook_method_selection = true;
    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, setup));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    assert_eq!(count_d_call_proceedings(&msgs), 1);
    let d_call_proceeding = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) if prim.main_address.ssi == TEST_ISSI => parse_d_call_proceeding(prim),
            _ => None,
        })
        .next()
        .expect("parrot setup should answer caller with D-CALL PROCEEDING");
    assert!(
        d_call_proceeding.hook_method_selection,
        "parrot should preserve the caller hook method so Hytera-class terminals do not render call modified"
    );
    let d_connect = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) if prim.main_address.ssi == TEST_ISSI => parse_d_connect(prim),
            _ => None,
        })
        .next()
        .expect("parrot setup should answer caller with D-CONNECT");
    assert_eq!(d_connect.transmission_grant, TransmissionGrant::Granted);
    assert!(
        d_connect.hook_method_selection,
        "parrot should preserve the caller hook method so the setup is not signalled as modified"
    );
    assert!(!d_connect.simplex_duplex_selection, "parrot service is simplex-only");

    let open = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .next()
        .expect("parrot setup should open a UMAC circuit");
    assert_eq!(open.peer_ts, None);
    assert_eq!(open.dl_media_source, CircuitDlMediaSource::LocalParrot);
    assert_eq!(open.active_addr, Some(TetraAddress::issi(TEST_ISSI)));
    assert_eq!(open.active_secondary_addrs, vec![TetraAddress::issi(99_999)]);
    let parrot_ts = open.ts;
    assert_eq!(count_umac_floor_granted(&msgs), 1);
    assert!(
        !msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { .. })
        )),
        "parrot is a local CMCE test service and must not route through Brew"
    );

    assert!(cmce_debug_active_call_ids(&mut test).contains(&d_connect.call_identifier));

    let events = drain_telemetry(&telemetry_source);
    let start_event = events
        .iter()
        .find_map(|event| match event {
            TelemetryEvent::IndividualCallStarted {
                call_id,
                calling_issi,
                called_issi,
                simplex,
                ts,
                ..
            } => Some((*call_id, *calling_issi, *called_issi, *simplex, *ts)),
            _ => None,
        })
        .expect("parrot setup should publish individual-call dashboard telemetry");
    assert_eq!(start_event, (d_connect.call_identifier, TEST_ISSI, 99_999, true, parrot_ts));

    let dashboard = DashboardServer::new("test.toml".to_string());
    for event in events {
        dashboard.handle_telemetry(event);
    }
    let state = dashboard.state.read().unwrap();
    let calls = state.snapshot_calls();
    let call = calls
        .iter()
        .find(|call| call.call_id == d_connect.call_identifier)
        .expect("parrot call should be visible in dashboard Calls");
    assert_eq!(call.call_type, "individual");
    assert_eq!(call.caller_issi, TEST_ISSI);
    assert_eq!(call.called_issi, 99_999);
    assert!(call.simplex);
    assert_eq!(call.ts, parrot_ts);
    let last_heard = state
        .last_heard
        .front()
        .expect("parrot call should be visible in dashboard Last Heard");
    assert_eq!(last_heard.issi, TEST_ISSI);
    assert_eq!(last_heard.activity, "call_individual");
    assert_eq!(last_heard.dest, 99_999);
}

#[test]
fn test_p2p_setup_to_parrot_99999_rejects_duplex_without_opening_circuit() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let mut setup = default_p2p_u_setup();
    setup.called_party_ssi = Some(99_999);
    setup.simplex_duplex_selection = true;
    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, setup));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    assert_p2p_setup_rejected_with_dummy_call_id_and_cause(&msgs, TEST_ISSI, DisconnectCause::IncompatibleTrafficCase);
    assert_eq!(count_umac_open(&msgs), 0);
}

#[test]
fn test_parrot_99999_records_replays_exact_frames_then_releases_caller() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    test.submit_message(build_u_setup_p2p_msg(TEST_ISSI, 99_999));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let d_connect = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim),
            _ => None,
        })
        .next()
        .expect("parrot setup should answer with D-CONNECT");
    let call_id = d_connect.call_identifier;
    let (traffic_ts, traffic_usage) = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some((circuit.ts, circuit.usage)),
            _ => None,
        })
        .next()
        .expect("parrot setup should open a circuit");

    let acelp_frame: Vec<u8> = (0..274).map(|idx| (idx % 2) as u8).collect();
    let raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 5 + 1) % 2) as u8).collect();
    test.submit_message(build_tmd_ind_to_cmce(traffic_ts, acelp_frame.clone(), None));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();
    test.submit_message(build_tmd_ind_to_cmce(traffic_ts, raw_block2.clone(), Some(PhyBlockNum::Block2)));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let playback_start_msgs = test.dump_sinks();
    assert!(
        !playback_start_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::TmdCircuitDataReq(_))),
        "parrot playback must not inject frames in the same router drain as U-TX CEASED"
    );
    assert!(
        playback_start_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted { source_issi: 99_999, .. })
        )),
        "parrot playback starts with one virtual floor grant"
    );
    let (playback_grant_prim, playback_grant) = playback_start_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) if prim.main_address.ssi == TEST_ISSI => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .next()
        .expect("parrot playback should notify the real caller that the virtual peer owns the floor");
    assert_eq!(
        TransmissionGrant::try_from(playback_grant.transmission_grant as u64),
        Ok(TransmissionGrant::GrantedToOtherUser)
    );
    assert_d_tx_granted_facch_allocation(
        playback_grant_prim,
        &playback_grant,
        traffic_ts,
        traffic_usage,
        UlDlAssignment::Dl,
        "parrot playback virtual peer grant",
    );
    assert_eq!(
        count_d_releases(&playback_start_msgs),
        0,
        "parrot must not release before paced playback has drained"
    );

    test.submit_message(build_tmd_ind_to_cmce(traffic_ts, vec![1; 274], None));
    test.deliver_all_messages();
    let late_ul_msgs = test.dump_sinks();
    assert!(
        !late_ul_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::TmdCircuitDataInd(prim) if prim.ts == traffic_ts
        )),
        "late UL media on a Parrot-owned timeslot must be consumed locally, not forwarded to Brew"
    );

    test.run_stack(Some(48));
    let mut release_msgs = test.dump_sinks();
    let playback: Vec<_> = release_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmdCircuitDataReq(prim) => Some((prim.ts, prim.data.clone(), prim.raw_tch_s_block)),
            _ => None,
        })
        .collect();
    assert_eq!(
        playback,
        vec![(traffic_ts, acelp_frame, None), (traffic_ts, raw_block2, Some(PhyBlockNum::Block2))],
        "parrot playback must preserve recorded frame order and raw TCH/S block metadata"
    );
    assert_established_p2p_release_pdus_to(&release_msgs, call_id, DisconnectCause::SwmiRequestedDisconnection, &[TEST_ISSI]);
    assert_release_notification_to(&release_msgs, TEST_ISSI, None);
    assert!(
        !release_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::LcmcMleUnitdataReq(prim)
                if prim.main_address.ssi == 99_999 && parse_d_release(prim).is_some()
        )),
        "parrot release must not address a non-existent RF peer 99999"
    );

    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 1);
    reporters[0].mark_transmitted();
    test.run_stack(Some(8));
    let cleanup_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&cleanup_msgs) >= 2,
        "parrot release reporter completion should close the local UMAC circuit"
    );
    assert!(!cmce_debug_active_call_ids(&mut test).contains(&call_id));
}

#[test]
fn test_parrot_99999_rf_length_recording_does_not_flood_floor_and_releases() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    test.submit_message(build_u_setup_p2p_msg(TEST_ISSI, 99_999));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| pdu.call_identifier),
            _ => None,
        })
        .next()
        .expect("parrot setup should answer with D-CONNECT");
    let traffic_ts = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit.ts),
            _ => None,
        })
        .next()
        .expect("parrot setup should open a circuit");

    const RF_RECORDED_FRAMES: usize = 141;
    for seq in 0..RF_RECORDED_FRAMES {
        let frame: Vec<u8> = (0..274).map(|idx| ((idx + seq) % 2) as u8).collect();
        test.submit_message(build_tmd_ind_to_cmce(traffic_ts, frame, None));
    }
    test.run_stack(Some(1));
    let recording_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_floor_granted(&recording_msgs),
        0,
        "parrot recording must not emit one FloorGranted per recorded TCH/S frame"
    );

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let playback_start_msgs = test.dump_sinks();
    let playback_count = playback_start_msgs
        .iter()
        .filter(|msg| matches!(&msg.msg, SapMsgInner::TmdCircuitDataReq(prim) if prim.ts == traffic_ts))
        .count();
    assert_eq!(
        playback_count, 0,
        "parrot must not inject RF-length playback frames in the same router drain as U-TX CEASED"
    );
    assert_eq!(
        count_d_releases(&playback_start_msgs),
        0,
        "parrot must not release before RF-length playback has drained"
    );

    test.run_stack(Some(16));
    let paced_msgs = test.dump_sinks();
    let paced_playback_count = paced_msgs
        .iter()
        .filter(|msg| matches!(&msg.msg, SapMsgInner::TmdCircuitDataReq(prim) if prim.ts == traffic_ts))
        .count();
    assert!(
        paced_playback_count <= 4,
        "parrot playback must be TDMA-paced, not a busy-loop flood; got {paced_playback_count} frames in 16 ticks"
    );

    let mut release_msgs = Vec::new();
    for _ in 0..220 {
        test.run_stack(Some(4));
        release_msgs.extend(test.dump_sinks());
        if count_d_releases(&release_msgs) > 0 {
            break;
        }
    }
    assert_established_p2p_release_pdus_to(&release_msgs, call_id, DisconnectCause::SwmiRequestedDisconnection, &[TEST_ISSI]);
    assert_eq!(
        count_umac_call_ended_or_close(&release_msgs),
        0,
        "parrot RF-length release should keep the local circuit open until D-RELEASE is transmitted or guard expires"
    );

    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 1);
    reporters[0].mark_transmitted();
    test.run_stack(Some(8));
    let cleanup_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&cleanup_msgs) >= 2,
        "parrot RF-length fail-safe must close the local UMAC circuit after release"
    );
    assert!(!cmce_debug_active_call_ids(&mut test).contains(&call_id));
}

#[test]
fn test_p2p_setup_to_configured_local_unregistered_issi_rejects_without_brew_fallback() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(2_260_000, 2_269_999)]);
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    test.submit_message(build_u_setup_p2p_msg(TEST_ISSI, 2_260_616));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // EN 300 392-2 clauses 14.5.1.1.2 and 14.5.1.3.2 scope this as an
    // individual-call setup rejection before a SwMI call identity exists. The
    // configured local SSI range is policy, not an ETSI address rule: the
    // destination remains local and unreachable instead of falling through to
    // the external Brew path.
    assert_p2p_setup_rejected_with_dummy_call_id_and_cause(&msgs, TEST_ISSI, DisconnectCause::CalledPartyNotReachable);
    assert!(
        !msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { .. })
        )),
        "configured-local unregistered ISSI must not be forwarded as a Brew network setup"
    );
}

#[test]
fn test_p2p_setup_between_runtime_registered_local_issis_stays_local_without_config_ranges() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(Vec::new());
    config.brew = Some(test_brew_config());
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.config.state_write().network_connected = true;
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    submit_subscriber_update(&mut test, LAB_ISSI_A, Vec::new(), BrewSubscriberAction::Register);
    submit_subscriber_update(&mut test, LAB_ISSI_MXP600, Vec::new(), BrewSubscriberAction::Register);
    test.run_stack(Some(2));
    let _ = test.dump_sinks();

    let (_call_id, msgs) = start_p2p_setup_between(&mut test, LAB_ISSI_A, LAB_ISSI_MXP600);

    // EN 300 392-2 clause 14.5.1 local individual-call setup is selected from
    // current SwMI subscriber state. Static local_ssi_ranges is deployment
    // policy for unregistered/offline fallback only; two runtime-registered
    // local MSs must not be routed through Brew just because the config has no
    // explicit ISSI range.
    assert_eq!(count_d_call_proceedings(&msgs), 1);
    assert_eq!(count_d_setups(&msgs), 1);
    assert_eq!(count_d_releases(&msgs), 0);
    assert!(
        !msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { .. })
        )),
        "runtime-registered local P2P must stay inside the BS without Brew fallback"
    );
}

#[test]
fn test_p2p_brew_backhaul_down_rejects_with_dummy_call_id() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(CfgBrew {
        host: "test-brew.local".to_string(),
        port: 443,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: std::time::Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        feature_rssi_export: false,
        whitelisted_ssis: None,
    });
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    test.submit_message(build_u_setup_p2p_msg(TEST_ISSI, 7_000_001));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // The Brew route is configured but not connected. This is still a
    // first-response call rejection, so the on-air D-RELEASE uses dummy call
    // identity 0 and does not allocate/open a local traffic circuit.
    assert_p2p_setup_rejected_with_dummy_call_id(&msgs, TEST_ISSI);
    assert!(
        msgs.iter().all(|msg| !matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { .. })
        )),
        "backhaul-down reject must not forward a NetworkCircuitSetupRequest"
    );
}

#[test]
fn test_p2p_setup_sends_proceeding_and_setup_without_opening_circuit() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    // EN 300 392-2 clause 14.7.1/14.7.2 P2P setup remains on common
    // signalling until the called MS returns U-CONNECT.
    let (call_id, setup_msgs) = start_p2p_setup(&mut test);

    let proceedings: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_call_proceeding(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(proceedings.len(), 1, "Expected one D-CALL-PROCEEDING to the caller");
    let proceeding_prim = proceedings[0].0;
    let proceeding = &proceedings[0].1;
    assert_eq!(proceeding.call_identifier, call_id);
    assert_eq!(proceeding_prim.main_address.ssi, TEST_ISSI);
    assert_eq!(proceeding_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(proceeding_prim.layer2service, Layer2Service::Unacknowledged);
    assert!(proceeding_prim.chan_alloc.is_none());
    assert!(!proceeding_prim.stealing_permission);

    let setups: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(setups.len(), 1, "Expected one D-SETUP to the called MS");
    let setup_prim = setups[0].0;
    let setup = &setups[0].1;
    assert_eq!(setup.call_identifier, call_id);
    assert_eq!(setup.calling_party_address_ssi, Some(TEST_ISSI));
    assert_eq!(setup.basic_service_information.communication_type, CommunicationType::P2p);
    assert_eq!(setup.basic_service_information.circuit_mode_type, CircuitModeType::TchS);
    assert_eq!(setup.transmission_grant, TransmissionGrant::NotGranted);
    assert!(!setup.transmission_request_permission);
    assert_eq!(setup_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(setup_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(setup_prim.layer2service, Layer2Service::Unacknowledged);
    assert!(setup_prim.chan_alloc.is_none());
    assert!(!setup_prim.stealing_permission);

    assert_eq!(
        count_umac_open(&setup_msgs),
        0,
        "P2P setup must not open UMAC traffic before U-CONNECT"
    );
}

#[test]
fn test_simple_private_call_after_real_mm_registration_and_group_affiliation() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Mm, TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    // EN 300 392-2 clauses 16.4/16.7/16.8: registration and group
    // affiliation are MM procedures, so feed CMCE via real U-LOCATION UPDATE
    // DEMAND instead of injecting MmSubscriberUpdate directly.
    submit_location_update_with_group_identity_location_demand(&mut test, TEST_ISSI, TEST_GSSI);
    test.run_stack(Some(1));
    let caller_mm_msgs = test.dump_sinks();
    assert!(
        contains_location_update_accept(&caller_mm_msgs),
        "caller registration should receive D-LOCATION UPDATE ACCEPT"
    );

    submit_location_update_with_group_identity_location_demand(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    test.run_stack(Some(1));
    let called_mm_msgs = test.dump_sinks();
    assert!(
        contains_location_update_accept(&called_mm_msgs),
        "called registration should receive D-LOCATION UPDATE ACCEPT"
    );
    {
        let state = test.config.state_read();
        assert!(state.subscribers.is_registered(TEST_ISSI));
        assert!(state.subscribers.is_registered(TEST_CALLED_ISSI));
        assert_eq!(state.subscribers.group_members(TEST_GSSI), vec![TEST_ISSI]);
        assert_eq!(state.subscribers.group_members(TEST_CALLED_GSSI), vec![TEST_CALLED_ISSI]);
    }

    // EN 300 392-2 clause 14.5.1.1.2: outgoing individual setup sends
    // D-CALL PROCEEDING to the caller and assigns a SwMI call id before
    // completion.
    let (call_id, setup_msgs) = start_p2p_setup(&mut test);
    let proceedings: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_call_proceeding(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(proceedings.len(), 1, "U-SETUP should receive one D-CALL PROCEEDING");
    let (proceeding_prim, proceeding) = &proceedings[0];
    assert_eq!(proceeding.call_identifier, call_id);
    assert_eq!(proceeding_prim.main_address.ssi, TEST_ISSI);
    assert_eq!(proceeding_prim.main_address.ssi_type, SsiType::Issi);

    // EN 300 392-2 clause 14.5.1.1.1: the incoming individual-call leg is
    // D-SETUP to the called ISSI; traffic is not opened until U-CONNECT.
    let setups: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(setups.len(), 1, "U-SETUP should emit one D-SETUP to the called MS");
    let (setup_prim, setup) = &setups[0];
    assert_eq!(setup.call_identifier, call_id);
    assert_eq!(setup.calling_party_address_ssi, Some(TEST_ISSI));
    assert_eq!(setup.basic_service_information.communication_type, CommunicationType::P2p);
    assert_eq!(setup_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(setup_prim.main_address.ssi_type, SsiType::Issi);
    assert!(setup_prim.chan_alloc.is_none());
    assert_eq!(count_umac_open(&setup_msgs), 0, "P2P setup must not open traffic before U-CONNECT");

    let (mut connect_msgs, after_called_ack_msgs) =
        submit_p2p_connect_and_ack_called(&mut test, build_u_connect_msg(TEST_CALLED_ISSI, call_id), TEST_CALLED_ISSI);

    // EN 300 392-2 clauses 14.5.1.1.1/14.5.1.1.2: U-CONNECT completes the
    // called leg. Annex D.4 keeps caller D-CONNECT blocked until the called
    // D-CONNECT ACKNOWLEDGE is L2-acknowledged.
    assert_eq!(
        count_umac_open(&connect_msgs),
        1,
        "U-CONNECT should open one shared simplex assigned-channel circuit"
    );
    assert!(
        connect_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some())),
        "caller D-CONNECT must wait for called D-CONNECT ACK BL-ACK"
    );
    assert!(
        connect_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect_acknowledge(prim).is_some())),
        "U-CONNECT should first produce D-CONNECT ACKNOWLEDGE to the called MS"
    );
    let open_ts = p2p_open_ts_for(&connect_msgs, TEST_ISSI);
    assert!(
        (1..=4).contains(&open_ts),
        "assigned-channel open should use a valid TETRA timeslot"
    );
    connect_msgs.extend(after_called_ack_msgs);
    assert!(
        connect_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some())),
        "called D-CONNECT ACK BL-ACK should release D-CONNECT to the caller"
    );
}

#[test]
fn test_simple_private_call_after_mm_soft_roaming_reattach_releases_stale_call() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Mm, TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    submit_location_update_with_group_identity_location_demand(&mut test, TEST_ISSI, TEST_GSSI);
    test.run_stack(Some(1));
    let caller_mm_msgs = test.dump_sinks();
    assert!(
        contains_location_update_accept(&caller_mm_msgs),
        "caller registration should receive D-LOCATION UPDATE ACCEPT"
    );

    submit_location_update_with_group_identity_location_demand(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    test.run_stack(Some(1));
    let called_mm_msgs = test.dump_sinks();
    assert!(
        contains_location_update_accept(&called_mm_msgs),
        "called registration should receive D-LOCATION UPDATE ACCEPT"
    );

    let call_id = start_active_p2p_call(&mut test);

    // ETSI EN 300 392-2 clause 16.4.1.1 keeps RoamingLocationUpdating in MM.
    // The CMCE reset is a bounded compatibility guard for a recently known MS:
    // if the terminal lost local call state, the SwMI still follows the
    // established individual-call release rule in clause 14.5.1.3.3 before
    // allowing the next clean U-SETUP.
    submit_location_update_with_type_and_group_identity_location_demand(
        &mut test,
        TEST_ISSI,
        TEST_GSSI,
        LocationUpdateType::RoamingLocationUpdating,
    );
    test.run_stack(Some(1));
    let mut reattach_msgs = test.dump_sinks();

    assert!(
        contains_location_update_accept(&reattach_msgs),
        "soft roaming reattach should still receive D-LOCATION UPDATE ACCEPT"
    );
    assert_established_p2p_release_pdus(&reattach_msgs, call_id, DisconnectCause::UserRequestedDisconnection);
    let release_reporters = extract_d_release_reporters(&mut reattach_msgs);
    assert_eq!(
        release_reporters.len(),
        2,
        "soft roaming reattach should reporter-track both assigned-channel D-RELEASEs"
    );
    assert_eq!(
        count_umac_call_ended_or_close(&reattach_msgs),
        0,
        "soft reattach must keep the P2P traffic circuit until D-RELEASE transmission is known"
    );

    for reporter in &release_reporters {
        reporter.mark_transmitted();
    }
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "D-RELEASE reporter completion should close the stale soft-reattach P2P circuit"
    );

    let (fresh_call_id, fresh_setup_msgs) = start_p2p_setup(&mut test);
    assert_ne!(fresh_call_id, call_id, "fresh private call should get a new call identifier");
    assert_eq!(
        count_umac_open(&fresh_setup_msgs),
        0,
        "fresh P2P setup must not open traffic before U-CONNECT"
    );
    assert!(
        fresh_setup_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_call_proceeding(prim).is_some())),
        "fresh U-SETUP should receive D-CALL PROCEEDING after soft roaming reattach"
    );
    assert!(
        fresh_setup_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_setup(prim).is_some())),
        "fresh U-SETUP should deliver D-SETUP to the called ISSI after soft roaming reattach"
    );

    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, fresh_call_id));
    test.run_stack(Some(1));
    let fresh_connect_msgs = test.dump_sinks();
    assert!(
        count_umac_open(&fresh_connect_msgs) >= 1,
        "fresh U-CONNECT should reopen the simple private-call traffic circuit"
    );
}

#[test]
fn test_p2p_u_setup_numeric_collision_routes_to_registered_issi_not_gssi() {
    debug::setup_logging_verbose();

    let collision = TEST_CALLED_ISSI;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, collision);
    register_subscriber(&mut test, collision, TEST_CALLED_GSSI);

    // EN 300 392-2 clause 14.5.1.0 requires individual calls to be set up as
    // point-to-point. A P2P U-SETUP therefore targets the registered ISSI,
    // not a same-number GSSI that also has listeners.
    test.submit_message(build_u_setup_p2p_msg(TEST_ISSI, collision));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();

    let setups: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(setups.len(), 1, "P2P setup should emit one ISSI-addressed D-SETUP");
    let (setup_prim, setup) = &setups[0];
    assert_eq!(setup.basic_service_information.communication_type, CommunicationType::P2p);
    assert_eq!(setup.calling_party_address_ssi, Some(TEST_ISSI));
    assert_eq!(setup_prim.main_address.ssi, collision);
    assert_eq!(setup_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(
        count_umac_open(&setup_msgs),
        0,
        "P2P setup must not be treated as an immediately connected group call"
    );
    assert!(
        setup_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some())),
        "P2P setup must wait for U-CONNECT before sending D-CONNECT"
    );
}

#[test]
fn test_p2p_pending_setup_does_not_duplicate_while_initial_reporter_pending() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let (call_id, mut setup_msgs) = start_p2p_setup(&mut test);

    let setups: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(setups.len(), 1, "P2P setup should emit one initial D-SETUP");
    let (setup_prim, setup) = &setups[0];
    assert_eq!(setup.call_identifier, call_id);
    assert_eq!(setup_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert!(setup_prim.chan_alloc.is_none(), "setup-phase D-SETUP remains on MCCH");
    assert!(
        setup_prim.tx_reporter.is_some(),
        "initial D-SETUP should be reporter-tracked so backup/retry paths wait for MAC completion"
    );
    assert_eq!(count_umac_open(&setup_msgs), 0, "D-SETUP must not open traffic before U-CONNECT");

    let reporters = extract_d_setup_reporters(&mut setup_msgs);
    assert_eq!(reporters.len(), 1, "initial setup should expose exactly one D-SETUP TxReporter");
    assert_eq!(reporters[0].get_state(), TxState::Pending);

    // EN 300 392-2 clause 14.5.1.1.1 requires D-SETUP to the called MS before
    // U-CONNECT. While MAC has not completed that first transfer, neither the
    // generic circuit backup path nor the EE retry path may enqueue a second
    // same-call D-SETUP.
    test.run_stack(Some(70));
    let backup_window_msgs = test.dump_sinks();
    assert_eq!(
        count_d_setups(&backup_window_msgs),
        0,
        "pending P2P setup must not send a generic backup D-SETUP while the initial reporter is still pending"
    );

    test.run_stack(Some(720));
    let pending_retry_msgs = test.dump_sinks();
    assert_eq!(
        count_d_setups(&pending_retry_msgs),
        0,
        "pending P2P setup must not send an EE retry while the initial reporter is still pending"
    );
}

#[test]
fn test_p2p_setup_timeout_sends_expiry_release_and_cleans_pending_call() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let (call_id, _setup_msgs) = start_p2p_setup(&mut test);

    // CallTimeoutSetupPhase::T60s maps to 4235 TETRA timeslots; expiry uses
    // strict age > limit, so the release occurs on the 4236th following tick.
    test.run_stack(Some(4236));
    let timeout_msgs = test.dump_sinks();
    let releases: Vec<_> = timeout_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();

    assert_eq!(
        releases.len(),
        6,
        "setup timeout should repeat MCCH D-RELEASE to both private-call parties"
    );
    for (prim, release) in &releases {
        assert_eq!(release.call_identifier, call_id);
        assert_eq!(release.disconnect_cause, DisconnectCause::ExpiryOfTimer);
        assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
        assert!(
            prim.chan_alloc.is_none(),
            "setup-timeout release stays on MCCH because no traffic circuit is active"
        );
        assert!(
            prim.main_address.ssi == TEST_ISSI || prim.main_address.ssi == TEST_CALLED_ISSI,
            "release should target one of the two P2P participants"
        );
    }
    assert!(
        count_umac_call_ended_or_close(&timeout_msgs) >= 1,
        "setup timeout cleanup must notify UMAC/control state that the pending call ended"
    );

    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let late_connect_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_open(&late_connect_msgs),
        0,
        "late U-CONNECT after setup timeout must not resurrect a cleaned-up private call"
    );
}

#[test]
fn test_p2p_setup_phase_called_u_disconnect_releases_without_d_disconnect() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let (call_id, _setup_msgs) = start_p2p_setup(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let reject_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.1.5: during individual call setup, a
    // called-user rejection is U-DISCONNECT followed by SwMI D-RELEASE.
    // D-DISCONNECT is the peer handshake for established calls, not setup
    // rejection before the traffic circuit exists.
    assert_eq!(count_d_disconnects(&reject_msgs), 0);
    assert_eq!(count_umac_open(&reject_msgs), 0);
    assert!(
        count_umac_call_ended_or_close(&reject_msgs) >= 1,
        "setup rejection cleanup should notify lower-layer/control state that the pending call ended"
    );

    let releases: Vec<_> = reject_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        releases.len(),
        6,
        "setup rejection should repeat MCCH D-RELEASE to both private-call parties"
    );
    for (prim, release) in releases {
        assert_eq!(release.call_identifier, call_id);
        assert_eq!(release.disconnect_cause, DisconnectCause::UserRequestedDisconnection);
        assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
        assert!(prim.main_address.ssi == TEST_ISSI || prim.main_address.ssi == TEST_CALLED_ISSI);
        assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
        assert!(prim.chan_alloc.is_none());
    }
}

#[test]
fn test_p2p_setup_phase_reject_busy_and_no_answer_causes_are_preserved() {
    debug::setup_logging_verbose();

    for disconnect_cause in [
        DisconnectCause::CallRejectedByTheCalledParty,
        DisconnectCause::CalledPartyBusy,
        DisconnectCause::ExpiryOfTimer,
        DisconnectCause::InvalidCallIdentifier,
    ] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
        let (call_id, _setup_msgs) = start_p2p_setup(&mut test);

        test.submit_message(build_u_disconnect_pdu_msg(
            TEST_CALLED_ISSI,
            UDisconnect {
                call_identifier: call_id,
                disconnect_cause,
                facility: None,
                proprietary: None,
            },
        ));
        test.run_stack(Some(1));
        let reject_msgs = test.dump_sinks();

        assert_eq!(
            count_d_disconnects(&reject_msgs),
            0,
            "setup-phase private reject/no-answer/busy cause {disconnect_cause:?} must not use established D-DISCONNECT"
        );
        let releases: Vec<_> = reject_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(
            releases.len(),
            6,
            "setup-phase private reject/no-answer/busy cause {disconnect_cause:?} should repeat MCCH D-RELEASE to both parties"
        );
        for (prim, release) in releases {
            assert_eq!(release.call_identifier, call_id);
            assert_eq!(release.disconnect_cause, disconnect_cause);
            assert!(prim.main_address.ssi == TEST_ISSI || prim.main_address.ssi == TEST_CALLED_ISSI);
            assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
            assert!(prim.chan_alloc.is_none());
        }
    }
}

#[test]
fn test_p2p_u_connect_waits_for_called_delivery_then_caller_d_connect_before_setup_floor() {
    debug::setup_logging_verbose();

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let (_call_id, ack_msgs, after_called_delivery_msgs, after_caller_delivery_msgs) =
        direct_private_simplex_connect_phases(u_setup, config);

    assert!(
        ack_msgs.iter().all(|msg| {
            !matches!(
                &msg.msg,
                SapMsgInner::LcmcMleUnitdataReq(prim)
                    if prim.main_address.ssi == TEST_ISSI && parse_d_connect(prim).is_some()
            )
        }),
        "ETSI EN 300 392-2 14.5.1.1.1/14.5.1.1.2: caller D-CONNECT must wait until called D-CONNECT ACKNOWLEDGE is delivered"
    );
    assert_eq!(
        count_umac_floor_granted(&ack_msgs),
        0,
        "private floor must not be released before called D-CONNECT ACKNOWLEDGE and caller D-CONNECT delivery"
    );

    let d_connect_acks: Vec<_> = ack_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect_acknowledge(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(d_connect_acks.len(), 1);
    assert_eq!(d_connect_acks[0].0.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(d_connect_acks[0].0.layer2service, Layer2Service::Unacknowledged);
    assert!(
        !d_connect_acks[0].0.stealing_permission,
        "first called D-CONNECT ACK with late assignment starts on the current channel; assigned-channel recovery remains for local MAC failure retries"
    );
    assert_eq!(
        d_connect_acks[0].0.unacked_bl_repetitions,
        Some(PRIVATE_SIMPLEX_CONNECT_ACK_UNACKED_REPETITIONS)
    );
    let chan_alloc = d_connect_acks[0]
        .0
        .chan_alloc
        .as_ref()
        .expect("called D-CONNECT ACK must carry channel allocation");
    assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Both);
    assert!(d_connect_acks[0].0.tx_reporter.is_some());

    assert_eq!(
        count_d_connects(&after_called_delivery_msgs),
        1,
        "called D-CONNECT ACK local unacknowledged delivery should release caller D-CONNECT"
    );
    assert_eq!(
        count_umac_floor_granted(&after_called_delivery_msgs),
        0,
        "called D-CONNECT ACK local delivery should send caller D-CONNECT without enabling floor"
    );
    assert_eq!(
        count_umac_floor_granted(&after_caller_delivery_msgs),
        1,
        "caller D-CONNECT L2 ACK completes private simplex setup and opens the ETSI setup-granted U-plane floor"
    );

    let mut connect_msgs = ack_msgs;
    connect_msgs.extend(after_called_delivery_msgs);
    connect_msgs.extend(after_caller_delivery_msgs);
    assert_private_simplex_caller_d_connect_with_setup_floor(&connect_msgs, TEST_ISSI, TEST_CALLED_ISSI);
}

#[test]
fn test_p2p_called_d_connect_ack_pending_local_delivery_does_not_authorize_caller() {
    debug::setup_logging_verbose();

    let shared = SharedConfig::from_parts(ComponentTest::get_default_test_config(StackMode::Bs), None);
    let mut cmce = CmceBs::new(shared, None, None);
    let mut queue = MessageQueue::new();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };

    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_ISSI, TEST_GSSI);
    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    cmce.rx_prim(&mut queue, build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    let setup_msgs = drain_message_queue(&mut queue);
    let call_id = first_d_setup_call_id(&setup_msgs);

    cmce.rx_prim(&mut queue, build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    let ack_msgs = drain_message_queue(&mut queue);
    let reporter = called_d_connect_ack_reporter(&ack_msgs, TEST_CALLED_ISSI);
    assert_eq!(reporter.get_state(), TxState::Pending);

    cmce.tick_start(&mut queue, dltime.add_timeslots(4));
    let pending_delivery_msgs = drain_message_queue(&mut queue);

    assert_eq!(
        count_d_connects(&pending_delivery_msgs),
        0,
        "caller D-CONNECT must not be queued until called D-CONNECT ACK BL-ACK is reported"
    );
    assert_eq!(
        count_umac_floor_granted(&pending_delivery_msgs),
        0,
        "private-simplex floor must remain blocked while called D-CONNECT ACK BL-ACK is pending"
    );
    assert_eq!(
        count_d_releases(&pending_delivery_msgs),
        0,
        "CMCE should keep waiting/retrying instead of releasing before the delivery guard expires"
    );
}

#[test]
fn test_p2p_called_d_connect_ack_unack_transmission_authorizes_caller_d_connect() {
    debug::setup_logging_verbose();

    let shared = SharedConfig::from_parts(ComponentTest::get_default_test_config(StackMode::Bs), None);
    let mut cmce = CmceBs::new(shared, None, None);
    let mut queue = MessageQueue::new();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };

    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_ISSI, TEST_GSSI);
    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    cmce.rx_prim(&mut queue, build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    let setup_msgs = drain_message_queue(&mut queue);
    let call_id = first_d_setup_call_id(&setup_msgs);

    cmce.rx_prim(&mut queue, build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    let ack_msgs = drain_message_queue(&mut queue);
    transmit_called_d_connect_ack(&ack_msgs, TEST_CALLED_ISSI);
    cmce.tick_start(&mut queue, dltime.add_timeslots(4));
    let transmitted_only_msgs = drain_message_queue(&mut queue);

    assert_eq!(
        count_d_connects(&transmitted_only_msgs),
        1,
        "EN 300 392-2 Annex D.4: simplex called D-CONNECT ACK may use repeated unacknowledged service, so local transmission authorizes caller D-CONNECT"
    );
    assert_eq!(
        count_umac_floor_granted(&transmitted_only_msgs),
        0,
        "private-simplex U-plane must remain blocked until caller D-CONNECT is delivered"
    );
    assert_eq!(
        count_d_releases(&transmitted_only_msgs),
        0,
        "simplex unacknowledged called-leg delivery must not release for missing BL-ACK"
    );
}

#[test]
fn test_p2p_called_d_connect_ack_unack_repeat_delivery_does_not_wait_for_bl_ack() {
    debug::setup_logging_verbose();

    let shared = SharedConfig::from_parts(ComponentTest::get_default_test_config(StackMode::Bs), None);
    let mut cmce = CmceBs::new(shared, None, None);
    let mut queue = MessageQueue::new();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };

    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_ISSI, TEST_GSSI);
    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    cmce.rx_prim(&mut queue, build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    let setup_msgs = drain_message_queue(&mut queue);
    let call_id = first_d_setup_call_id(&setup_msgs);

    cmce.rx_prim(&mut queue, build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    let ack_msgs = drain_message_queue(&mut queue);
    let d_connect_acks: Vec<_> = ack_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect_acknowledge(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(d_connect_acks.len(), 1);
    let (ack_prim, ack_pdu) = &d_connect_acks[0];
    assert_eq!(ack_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(ack_pdu.call_identifier, call_id);
    assert_eq!(ack_prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(
        ack_prim.unacked_bl_repetitions,
        Some(PRIVATE_SIMPLEX_CONNECT_ACK_UNACKED_REPETITIONS)
    );
    assert!(ack_prim.chan_alloc.is_some());
    assert_eq!(count_d_connects(&ack_msgs), 0);
    assert_eq!(count_umac_floor_granted(&ack_msgs), 0);

    transmit_called_d_connect_ack(&ack_msgs, TEST_CALLED_ISSI);
    cmce.tick_start(&mut queue, dltime.add_timeslots(4));
    let after_delivery_msgs = drain_message_queue(&mut queue);

    assert_eq!(
        count_d_connects(&after_delivery_msgs),
        1,
        "Annex D.4 repeat signalling may proceed after unacknowledged local delivery"
    );
    assert_eq!(
        count_umac_floor_granted(&after_delivery_msgs),
        0,
        "private floor must still wait for caller D-CONNECT delivery"
    );
    assert_eq!(
        count_d_releases(&after_delivery_msgs),
        0,
        "unacknowledged repeated called-leg setup delivery must not fail the call for missing BL-ACK"
    );
}

#[test]
fn test_p2p_caller_d_connect_transmitted_without_l2_ack_does_not_seed_initial_floor() {
    debug::setup_logging_verbose();

    let shared = SharedConfig::from_parts(ComponentTest::get_default_test_config(StackMode::Bs), None);
    let mut cmce = CmceBs::new(shared, None, None);
    let mut queue = MessageQueue::new();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };

    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_ISSI, TEST_GSSI);
    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    cmce.rx_prim(&mut queue, build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    let setup_msgs = drain_message_queue(&mut queue);
    let call_id = first_d_setup_call_id(&setup_msgs);

    cmce.rx_prim(&mut queue, build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    let ack_msgs = drain_message_queue(&mut queue);
    acknowledge_called_d_connect_ack(&ack_msgs, TEST_CALLED_ISSI);
    cmce.tick_start(&mut queue, dltime);
    let caller_connect_msgs = drain_message_queue(&mut queue);

    assert_eq!(
        count_d_connects(&caller_connect_msgs),
        1,
        "called D-CONNECT ACK BL-ACK should send one caller D-CONNECT"
    );
    assert_eq!(
        count_umac_floor_granted(&caller_connect_msgs),
        0,
        "caller D-CONNECT must be locally delivered before the first simplex floor"
    );

    let reporter = d_connect_reporter(&caller_connect_msgs, TEST_ISSI);
    reporter.mark_transmitted();
    cmce.tick_start(&mut queue, dltime.add_timeslots(4));
    let no_ack_msgs = drain_message_queue(&mut queue);

    assert_eq!(
        count_umac_floor_granted(&no_ack_msgs),
        0,
        "caller D-CONNECT local transmission must not seed private-simplex U-plane without L2 ACK"
    );
    assert_eq!(
        count_d_releases(&no_ack_msgs),
        0,
        "CMCE must not release simply because the caller D-CONNECT L2 ACK has not arrived inside this short guard"
    );

    cmce.rx_prim(&mut queue, build_u_tx_demand_msg(TEST_ISSI, call_id));
    let premature_ptt_msgs = drain_message_queue(&mut queue);
    assert_eq!(
        count_d_tx_granted(&premature_ptt_msgs),
        0,
        "caller U-TX DEMAND must not receive D-TX GRANTED until caller D-CONNECT is L2-ACKed"
    );
    assert_eq!(
        count_umac_floor_granted(&premature_ptt_msgs),
        0,
        "caller U-TX DEMAND must not open U-plane while caller D-CONNECT ACK is still pending"
    );

    reporter.mark_acknowledged();
    cmce.tick_start(&mut queue, dltime.add_timeslots(8));
    let activated_msgs = drain_message_queue(&mut queue);
    assert_eq!(
        count_umac_floor_granted(&activated_msgs),
        1,
        "caller D-CONNECT BL-ACK should activate the ETSI setup-granted private-simplex U-plane floor"
    );

    cmce.rx_prim(&mut queue, build_u_tx_demand_msg(TEST_ISSI, call_id));
    let first_ptt_msgs = drain_message_queue(&mut queue);
    assert_eq!(
        count_d_tx_granted(&first_ptt_msgs),
        2,
        "explicit U-TX DEMAND from the setup floor holder should notify both private-call parties"
    );
    assert_eq!(
        count_umac_floor_granted(&first_ptt_msgs),
        1,
        "explicit U-TX DEMAND from the setup floor holder should refresh exactly one U-plane floor"
    );
    assert!(
        first_ptt_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ..
            }) if *got_call_id == call_id && *source_issi == TEST_ISSI && *dest_gssi == TEST_CALLED_ISSI
        )),
        "explicit PTT should grant the requesting caller floor"
    );
}

#[test]
fn test_p2p_caller_d_connect_missing_l2_ack_retries_on_assigned_channel_before_floor() {
    debug::setup_logging_verbose();

    let shared = SharedConfig::from_parts(ComponentTest::get_default_test_config(StackMode::Bs), None);
    let mut cmce = CmceBs::new(shared, None, None);
    let mut queue = MessageQueue::new();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };

    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_ISSI, TEST_GSSI);
    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    cmce.rx_prim(&mut queue, build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    let setup_msgs = drain_message_queue(&mut queue);
    let call_id = first_d_setup_call_id(&setup_msgs);

    cmce.rx_prim(&mut queue, build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    let ack_msgs = drain_message_queue(&mut queue);
    acknowledge_called_d_connect_ack(&ack_msgs, TEST_CALLED_ISSI);
    cmce.tick_start(&mut queue, dltime);
    let first_caller_connect_msgs = drain_message_queue(&mut queue);

    let first_d_connects: Vec<_> = first_caller_connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(first_d_connects.len(), 1);
    assert_eq!(first_d_connects[0].0.main_address.ssi, TEST_ISSI);
    assert_eq!(
        first_d_connects[0].1.notification_indicator, None,
        "caller D-CONNECT must remain compact for assigned-channel retry compatibility"
    );
    assert!(
        !first_d_connects[0].0.stealing_permission,
        "ETSI EN 300 392-2 Annex D.4: first caller D-CONNECT with channel allocation uses current-channel ACK grant"
    );
    assert_eq!(first_d_connects[0].0.layer2service, Layer2Service::Acknowledged);
    assert!(first_d_connects[0].0.chan_alloc.is_some());
    assert_eq!(
        count_umac_floor_granted(&first_caller_connect_msgs),
        0,
        "first caller D-CONNECT must not open private-simplex media until L2 ACK"
    );

    let first_reporter = d_connect_reporter(&first_caller_connect_msgs, TEST_ISSI);
    first_reporter.mark_transmitted();
    first_reporter.mark_lost();
    cmce.tick_start(&mut queue, dltime.add_timeslots(4));
    let retry_msgs = drain_message_queue(&mut queue);

    let retry_d_connects: Vec<_> = retry_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(retry_d_connects.len(), 1);
    assert_eq!(retry_d_connects[0].0.main_address.ssi, TEST_ISSI);
    assert_eq!(
        retry_d_connects[0].1.notification_indicator, None,
        "assigned-channel recovery D-CONNECT must fit FACCH/STCH with MAC-RESOURCE"
    );
    assert!(
        retry_d_connects[0].0.stealing_permission,
        "if the caller missed the current-channel ACK window after channel allocation, retry D-CONNECT on the assigned traffic channel using FACCH/STCH"
    );
    assert_eq!(retry_d_connects[0].0.layer2service, Layer2Service::Acknowledged);
    assert_eq!(retry_d_connects[0].1.call_identifier, call_id);
    assert!(retry_d_connects[0].0.chan_alloc.is_some());
    assert_eq!(
        count_umac_floor_granted(&retry_msgs),
        0,
        "assigned-channel recovery D-CONNECT still waits for the caller BL-ACK before FloorGranted"
    );
    assert_eq!(count_d_releases(&retry_msgs), 0);

    let retry_reporter = d_connect_reporter(&retry_msgs, TEST_ISSI);
    retry_reporter.mark_transmitted();
    cmce.tick_start(&mut queue, dltime.add_timeslots(8));
    let retry_no_ack_msgs = drain_message_queue(&mut queue);
    assert_eq!(
        count_umac_floor_granted(&retry_no_ack_msgs),
        0,
        "local FACCH transmission alone must not open private-simplex media"
    );

    retry_reporter.mark_acknowledged();
    cmce.tick_start(&mut queue, dltime.add_timeslots(12));
    let activated_msgs = drain_message_queue(&mut queue);
    assert_eq!(
        count_umac_floor_granted(&activated_msgs),
        1,
        "caller D-CONNECT BL-ACK on assigned-channel recovery activates the ETSI setup-granted floor"
    );
}

#[test]
fn test_p2p_caller_invalid_call_identifier_during_caller_connect_ack_pending_releases_without_active_teardown() {
    debug::setup_logging_verbose();

    let shared = SharedConfig::from_parts(ComponentTest::get_default_test_config(StackMode::Bs), None);
    let mut cmce = CmceBs::new(shared, None, None);
    let mut queue = MessageQueue::new();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };

    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_ISSI, TEST_GSSI);
    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    cmce.rx_prim(&mut queue, build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    let setup_msgs = drain_message_queue(&mut queue);
    let call_id = first_d_setup_call_id(&setup_msgs);

    cmce.rx_prim(&mut queue, build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    let ack_msgs = drain_message_queue(&mut queue);
    acknowledge_called_d_connect_ack(&ack_msgs, TEST_CALLED_ISSI);
    cmce.tick_start(&mut queue, dltime);
    let caller_connect_msgs = drain_message_queue(&mut queue);
    let caller_reporter = d_connect_reporter(&caller_connect_msgs, TEST_ISSI);
    caller_reporter.mark_transmitted();

    cmce.rx_prim(
        &mut queue,
        build_u_disconnect_pdu_msg(
            TEST_ISSI,
            UDisconnect {
                call_identifier: call_id,
                disconnect_cause: DisconnectCause::InvalidCallIdentifier,
                facility: None,
                proprietary: None,
            },
        ),
    );
    let mut release_msgs = drain_message_queue(&mut queue);

    assert_eq!(
        count_d_disconnects(&release_msgs),
        0,
        "caller-connect abort must not use established-call D-DISCONNECT"
    );
    assert_eq!(
        count_d_tx_granted(&release_msgs),
        0,
        "caller-connect abort must not grant a private floor"
    );
    assert_eq!(
        count_umac_floor_granted(&release_msgs),
        0,
        "caller-connect abort must not open U-plane"
    );

    let releases: Vec<_> = release_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        releases.len(),
        2,
        "caller-connect abort should release caller on current signalling and called leg on assigned signalling"
    );
    let caller_release = releases
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == TEST_ISSI)
        .expect("caller must receive D-RELEASE");
    assert_eq!(caller_release.1.call_identifier, call_id);
    assert_eq!(caller_release.1.disconnect_cause, DisconnectCause::InvalidCallIdentifier);
    assert!(!caller_release.0.stealing_permission);
    assert!(caller_release.0.chan_alloc.is_none());

    let called_release = releases
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == TEST_CALLED_ISSI)
        .expect("called MS must receive D-RELEASE");
    assert_eq!(called_release.1.call_identifier, call_id);
    assert_eq!(called_release.1.disconnect_cause, DisconnectCause::InvalidCallIdentifier);
    assert!(called_release.0.stealing_permission);
    assert!(called_release.0.chan_alloc.is_some());
    assert!(called_release.0.tx_reporter.is_some());

    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 1);
    assert_eq!(
        count_umac_call_ended_or_close(&release_msgs),
        0,
        "assigned bearer must stay open until called-leg D-RELEASE is locally transmitted"
    );

    reporters[0].mark_transmitted();
    cmce.tick_start(&mut queue, dltime.add_timeslots(4));
    let cleanup_msgs = drain_message_queue(&mut queue);
    assert!(
        count_umac_call_ended_or_close(&cleanup_msgs) >= 1,
        "called-leg D-RELEASE transmission should close the pending connect-abort bearer"
    );

    cmce.rx_prim(&mut queue, build_u_setup_p2p_msg(TEST_ISSI, TEST_CALLED_ISSI));
    let fresh_setup_msgs = drain_message_queue(&mut queue);
    assert_eq!(
        count_d_setups(&fresh_setup_msgs),
        1,
        "connect-abort cleanup must not leave either P2P party busy for the next setup"
    );
    assert_eq!(count_d_releases(&fresh_setup_msgs), 0);
}

#[test]
fn test_p2p_preemptive_disconnect_during_caller_connect_pending_is_sanitized() {
    debug::setup_logging_verbose();

    let shared = SharedConfig::from_parts(ComponentTest::get_default_test_config(StackMode::Bs), None);
    let mut cmce = CmceBs::new(shared, None, None);
    let mut queue = MessageQueue::new();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };

    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_ISSI, TEST_GSSI);
    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    cmce.rx_prim(&mut queue, build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    let setup_msgs = drain_message_queue(&mut queue);
    let call_id = first_d_setup_call_id(&setup_msgs);

    cmce.rx_prim(&mut queue, build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    let ack_msgs = drain_message_queue(&mut queue);
    acknowledge_called_d_connect_ack(&ack_msgs, TEST_CALLED_ISSI);
    cmce.tick_start(&mut queue, dltime);
    let caller_connect_msgs = drain_message_queue(&mut queue);
    let caller_reporter = d_connect_reporter(&caller_connect_msgs, TEST_ISSI);
    caller_reporter.mark_transmitted();

    cmce.rx_prim(
        &mut queue,
        build_u_disconnect_with_cause_msg(TEST_ISSI, call_id, DisconnectCause::PreEmptiveUseOfResource),
    );
    let release_msgs = drain_message_queue(&mut queue);

    assert_eq!(count_d_disconnects(&release_msgs), 0);
    assert_eq!(count_d_tx_interrupt(&release_msgs), 0);
    assert_eq!(
        count_umac_floor_granted(&release_msgs),
        0,
        "unsupported private pre-emption abort must not activate the simplex floor"
    );

    let releases: Vec<_> = release_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(releases.len(), 2);
    assert!(
        releases
            .iter()
            .all(|(_, pdu)| pdu.disconnect_cause == DisconnectCause::UserRequestedDisconnection),
        "private-call U-DISCONNECT PreEmptiveUseOfResource is a terminal-local abort here; BS must not echo it as SwMI pre-emption"
    );
    assert!(releases.iter().any(|(prim, _)| prim.main_address.ssi == TEST_ISSI));
    assert!(releases.iter().any(|(prim, _)| prim.main_address.ssi == TEST_CALLED_ISSI));
}

#[test]
fn test_p2p_duplex_u_connect_waits_for_called_delivery_before_caller_d_connect() {
    debug::setup_logging_verbose();

    let shared = SharedConfig::from_parts(ComponentTest::get_default_test_config(StackMode::Bs), None);
    let mut cmce = CmceBs::new(shared, None, None);
    let mut queue = MessageQueue::new();

    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_ISSI, TEST_GSSI);
    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    u_setup.simplex_duplex_selection = true;
    cmce.rx_prim(&mut queue, build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    let setup_msgs = drain_message_queue(&mut queue);
    let call_id = first_d_setup_call_id(&setup_msgs);

    cmce.rx_prim(
        &mut queue,
        build_u_connect_custom_msg_with_hook(TEST_CALLED_ISSI, call_id, false, true),
    );
    let ack_msgs = drain_message_queue(&mut queue);

    assert_eq!(
        count_d_connects(&ack_msgs),
        0,
        "duplex caller D-CONNECT stays blocked until called D-CONNECT ACK BL-ACK"
    );
    assert_eq!(
        count_umac_floor_granted(&ack_msgs),
        0,
        "duplex direct setup must not synthesize simplex floor before called delivery"
    );

    let d_connect_acks: Vec<_> = ack_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect_acknowledge(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(d_connect_acks.len(), 1);
    assert_eq!(d_connect_acks[0].0.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(d_connect_acks[0].0.main_address.ssi_type, SsiType::Issi);
    assert_eq!(d_connect_acks[0].0.layer2service, Layer2Service::Acknowledged);
    assert_eq!(d_connect_acks[0].0.unacked_bl_repetitions, None);
    assert!(d_connect_acks[0].0.chan_alloc.is_some());
    assert!(d_connect_acks[0].0.tx_reporter.is_some());
    assert_eq!(d_connect_acks[0].1.call_identifier, call_id);
    assert_eq!(d_connect_acks[0].1.transmission_grant, TransmissionGrant::Granted);

    acknowledge_called_d_connect_ack(&ack_msgs, TEST_CALLED_ISSI);
    cmce.tick_start(&mut queue, TdmaTime { h: 0, m: 1, f: 1, t: 1 });
    let after_called_ack_msgs = drain_message_queue(&mut queue);

    let d_connects: Vec<_> = after_called_ack_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        d_connects.len(),
        1,
        "called D-CONNECT ACK BL-ACK should release exactly one duplex D-CONNECT to the caller"
    );
    let (connect_prim, d_connect) = &d_connects[0];
    assert_eq!(connect_prim.main_address.ssi, TEST_ISSI);
    assert_eq!(connect_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(connect_prim.layer2service, Layer2Service::Acknowledged);
    assert!(connect_prim.chan_alloc.is_some());
    assert!(connect_prim.tx_reporter.is_some());
    assert_eq!(d_connect.call_identifier, call_id);
    assert!(d_connect.simplex_duplex_selection);
    assert_eq!(d_connect.transmission_grant, TransmissionGrant::Granted);
    assert_eq!(count_umac_floor_granted(&after_called_ack_msgs), 0);
}

#[test]
fn test_p2p_duplex_from_frequency_simplex_ms_keeps_two_local_bearers() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Mm, TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    // EN 300 392-2 clause 16.10.5 / table 16.31 reports RF frequency
    // simplex/duplex capability. Clause 23.1.3.1 still permits a frequency
    // half-duplex MS to use single-slot duplex call service on the
    // corresponding uplink/downlink slot pair, so CMCE must not translate this
    // RF bit into a forced CMCE simplex private call.
    submit_location_update_with_group_and_class_of_ms(&mut test, TEST_ISSI, TEST_GSSI, frequency_simplex_voice_class_of_ms());
    test.run_stack(Some(1));
    let caller_mm_msgs = test.dump_sinks();
    assert!(
        contains_location_update_accept(&caller_mm_msgs),
        "caller frequency-simplex ClassOfMs registration should be accepted"
    );

    submit_location_update_with_group_and_class_of_ms(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI, frequency_simplex_voice_class_of_ms());
    test.run_stack(Some(1));
    let called_mm_msgs = test.dump_sinks();
    assert!(
        contains_location_update_accept(&called_mm_msgs),
        "called frequency-simplex ClassOfMs registration should be accepted"
    );

    let mut u_setup = default_p2p_u_setup();
    u_setup.hook_method_selection = true;
    u_setup.simplex_duplex_selection = true;
    let (call_id, setup_msgs) = start_p2p_setup_with_u_setup(&mut test, u_setup);

    let proceeding = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_call_proceeding(prim),
            _ => None,
        })
        .expect("caller should receive D-CALL PROCEEDING");
    assert!(
        proceeding.simplex_duplex_selection,
        "ClassOfMs frequency-simplex bit must not downgrade caller D-CALL PROCEEDING to simplex"
    );

    let setup = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim),
            _ => None,
        })
        .expect("called MS should receive D-SETUP");
    assert!(
        setup.simplex_duplex_selection,
        "ClassOfMs frequency-simplex bit must not downgrade called D-SETUP to simplex"
    );
    assert_eq!(setup.call_time_out, CallTimeout::Infinite);
    assert_eq!(count_umac_open(&setup_msgs), 0, "P2P setup must not open traffic before U-CONNECT");

    let (connect_msgs, _after_called_ack_msgs) = submit_p2p_connect_and_ack_called(
        &mut test,
        build_u_connect_custom_msg(TEST_CALLED_ISSI, call_id, true),
        TEST_CALLED_ISSI,
    );

    let open_circuits: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .collect();
    assert_eq!(
        open_circuits.len(),
        2,
        "local MS-MS private duplex must open one assigned bearer per MS"
    );
    assert!(
        open_circuits.iter().all(|circuit| circuit.peer_ts.is_some()),
        "duplex local bearers must be cross-routed with peer_ts on both legs"
    );
    assert_ne!(
        open_circuits[0].ts, open_circuits[1].ts,
        "local MS-MS private duplex needs two distinct traffic timeslots"
    );
}

#[test]
fn test_p2p_called_d_connect_ack_local_discard_retries_before_release() {
    debug::setup_logging_verbose();

    let shared = SharedConfig::from_parts(ComponentTest::get_default_test_config(StackMode::Bs), None);
    let mut cmce = CmceBs::new(shared, None, None);
    let mut queue = MessageQueue::new();
    let mut dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };

    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_ISSI, TEST_GSSI);
    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    cmce.rx_prim(&mut queue, build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    let setup_msgs = drain_message_queue(&mut queue);
    let call_id = first_d_setup_call_id(&setup_msgs);

    cmce.rx_prim(&mut queue, build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    let mut ack_msgs = drain_message_queue(&mut queue);
    assert_eq!(count_umac_open(&ack_msgs), 1);

    let expected_stealing_by_attempt = [false, false, true, false, true];
    for (idx, expected_stealing) in expected_stealing_by_attempt.iter().copied().enumerate() {
        let attempt = idx + 1;
        assert_eq!(
            count_d_connect_acknowledges(&ack_msgs),
            1,
            "attempt {attempt}: called MS should receive one repeated D-CONNECT ACKNOWLEDGE delivery attempt"
        );
        let d_connect_acks: Vec<_> = ack_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect_acknowledge(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(d_connect_acks[0].0.main_address.ssi, TEST_CALLED_ISSI);
        assert_eq!(d_connect_acks[0].0.layer2service, Layer2Service::Unacknowledged);
        assert_eq!(
            d_connect_acks[0].0.unacked_bl_repetitions,
            Some(PRIVATE_SIMPLEX_CONNECT_ACK_UNACKED_REPETITIONS)
        );
        assert_eq!(
            d_connect_acks[0].0.stealing_permission, expected_stealing,
            "attempt {attempt}: local-discard retry should alternate MCCH/current-channel and assigned-channel STCH/FACCH recovery"
        );
        assert_eq!(
            d_connect_acks[0]
                .0
                .chan_alloc
                .as_ref()
                .expect("called D-CONNECT ACK must carry late channel allocation")
                .ul_dl_assigned,
            UlDlAssignment::Both
        );
        assert_eq!(d_connect_acks[0].1.call_identifier, call_id);
        assert!(d_connect_acks[0].0.tx_reporter.is_some());
        assert_eq!(
            count_d_connects(&ack_msgs),
            0,
            "attempt {attempt}: caller D-CONNECT must remain blocked until called D-CONNECT ACK is locally transmitted"
        );
        assert_eq!(
            count_umac_floor_granted(&ack_msgs),
            0,
            "attempt {attempt}: UMAC floor must remain blocked until called D-CONNECT ACK is locally transmitted and caller D-CONNECT follows"
        );
        assert_eq!(
            count_d_releases(&ack_msgs),
            0,
            "attempt {attempt}: release should wait until retries are exhausted"
        );

        discard_called_d_connect_ack(&ack_msgs, TEST_CALLED_ISSI);
        dltime = dltime.add_timeslots(4);
        cmce.tick_start(&mut queue, dltime);
        ack_msgs = drain_message_queue(&mut queue);
    }
    let fail_msgs = ack_msgs;

    assert_eq!(
        count_d_connect_acknowledges(&fail_msgs),
        0,
        "after retry exhaustion CMCE should stop retrying called D-CONNECT ACKNOWLEDGE"
    );
    assert_eq!(
        count_d_connects(&fail_msgs),
        0,
        "caller D-CONNECT must never be sent when called D-CONNECT ACK was never locally transmitted"
    );
    assert_eq!(
        count_umac_floor_granted(&fail_msgs),
        0,
        "private floor must never be seeded when called D-CONNECT ACK was never locally transmitted"
    );

    let releases: Vec<_> = fail_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert!(
        !releases.is_empty(),
        "called D-CONNECT ACK retry exhaustion should release the private setup"
    );
    for (prim, release) in releases {
        assert_eq!(release.call_identifier, call_id);
        assert_eq!(release.disconnect_cause, DisconnectCause::AcknowledgedServiceNotComplete);
        assert!(prim.main_address.ssi == TEST_ISSI || prim.main_address.ssi == TEST_CALLED_ISSI);
        assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
        assert!(prim.chan_alloc.is_none());
    }
}

#[test]
fn test_p2p_hook_other_ms_connect_waits_for_called_ack_then_seeds_called_floor() {
    debug::setup_logging_verbose();

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    u_setup.hook_method_selection = true;
    u_setup.request_to_transmit_send_data = true;
    let (_call_id, connect_msgs) = direct_private_simplex_connect_msgs(u_setup);

    assert_private_simplex_caller_d_connect_with_setup_floor(&connect_msgs, TEST_CALLED_ISSI, TEST_ISSI);

    let d_connect = connect_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) if prim.main_address.ssi == TEST_ISSI => parse_d_connect(prim),
            _ => None,
        })
        .expect("caller should receive D-CONNECT");
    assert_eq!(
        d_connect.transmission_grant,
        TransmissionGrant::GrantedToOtherUser,
        "called-first hook setup must preserve ETSI connect grant polarity while BS seeds the called-side setup floor after both setup legs are delivered"
    );

    let d_connect_ack = connect_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) if prim.main_address.ssi == TEST_CALLED_ISSI => parse_d_connect_acknowledge(prim),
            _ => None,
        })
        .expect("called MS should receive D-CONNECT-ACKNOWLEDGE");
    assert_eq!(
        d_connect_ack.transmission_grant,
        TransmissionGrant::Granted,
        "called-first hook setup must preserve called MS ETSI transmit grant while BS UMAC floor remains silent"
    );
}

#[test]
fn test_p2p_hook_override_offers_on_off_hook_for_direct_setup() {
    debug::setup_logging_verbose();

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.force_private_p2p_hook_signalling = true;
    let shared = SharedConfig::from_parts(config, None);
    let (telemetry_sink, telemetry_source) = telemetry_channel();
    let mut cmce = CmceBs::new(shared, Some(telemetry_sink), None);
    let mut queue = MessageQueue::new();

    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_ISSI, TEST_GSSI);
    register_subscriber_to_cmce(&mut cmce, &mut queue, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    assert!(!u_setup.hook_method_selection, "test input models Motorola direct setup");

    cmce.rx_prim(&mut queue, build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    let setup_msgs = drain_message_queue(&mut queue);
    let call_id = first_d_setup_call_id(&setup_msgs);

    let d_call_proceeding = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) if prim.main_address.ssi == TEST_ISSI => parse_d_call_proceeding(prim),
            _ => None,
        })
        .expect("caller should receive D-CALL PROCEEDING");
    assert!(
        d_call_proceeding.hook_method_selection,
        "compatibility override should offer on/off-hook signalling to the caller"
    );

    let d_setup = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) if prim.main_address.ssi == TEST_CALLED_ISSI => parse_d_setup(prim),
            _ => None,
        })
        .expect("called MS should receive D-SETUP");
    assert!(
        d_setup.hook_method_selection,
        "compatibility override should offer on/off-hook signalling to the called MS"
    );
    assert_eq!(
        d_setup.transmission_grant,
        TransmissionGrant::GrantedToOtherUser,
        "direct-setup caller first-floor polarity must be preserved when only the hook method is overridden"
    );
    let start_event = drain_telemetry(&telemetry_source)
        .into_iter()
        .find_map(|event| match event {
            TelemetryEvent::IndividualCallStarted {
                call_id,
                calling_issi,
                called_issi,
                simplex,
                secondary_ts,
                ..
            } => Some((call_id, calling_issi, called_issi, simplex, secondary_ts)),
            _ => None,
        })
        .expect("local P2P setup should publish dashboard telemetry");
    assert_eq!(
        start_event,
        (call_id, TEST_ISSI, TEST_CALLED_ISSI, true, None),
        "dashboard simplex/duplex state must come from simplex_duplex_selection, not hook_method_selection"
    );

    cmce.rx_prim(&mut queue, build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    let ack_msgs = drain_message_queue(&mut queue);
    acknowledge_called_d_connect_ack(&ack_msgs, TEST_CALLED_ISSI);
    cmce.tick_start(&mut queue, TdmaTime { h: 0, m: 1, f: 1, t: 1 });
    let after_called_ack_msgs = drain_message_queue(&mut queue);

    let d_connect = after_called_ack_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) if prim.main_address.ssi == TEST_ISSI => parse_d_connect(prim),
            _ => None,
        })
        .expect("caller should receive D-CONNECT after called ACK");
    assert!(
        d_connect.hook_method_selection,
        "caller D-CONNECT should preserve the offered on/off-hook method"
    );
    assert_eq!(d_connect.transmission_grant, TransmissionGrant::Granted);
}

#[test]
fn test_p2p_u_connect_opens_circuit_and_sends_connect_pair_with_allocations() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let (call_id, _setup_msgs) = start_p2p_setup(&mut test);

    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();
    acknowledge_called_d_connect_ack(&connect_msgs, TEST_CALLED_ISSI);
    test.run_stack(Some(1));
    let after_called_ack_msgs = test.dump_sinks();
    acknowledge_first_d_connect(&after_called_ack_msgs);
    test.run_stack(Some(1));
    let after_caller_ack_msgs = test.dump_sinks();

    assert!(
        count_umac_open(&connect_msgs) >= 1,
        "U-CONNECT should be the point where P2P traffic circuit(s) open"
    );
    let open_circuits: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .collect();
    assert_eq!(
        open_circuits.len(),
        1,
        "simple local private simplex call should open one shared assigned-channel circuit"
    );
    let simplex_open = open_circuits
        .iter()
        .find(|circuit| circuit.active_addr == Some(TetraAddress::new(TEST_CALLED_ISSI, SsiType::Issi)))
        .expect("simplex P2P should open a called-leg primary UMAC traffic circuit for pre-floor ACK attribution");
    // EN 300 392-2 clause 14.5.1.2.1: simple private setup keeps one
    // simplex traffic channel; peer_ts is reserved for duplex cross-routing.
    assert_eq!(simplex_open.peer_ts, None);
    assert_eq!(simplex_open.dl_media_source, CircuitDlMediaSource::LocalLoopback);
    assert_eq!(
        simplex_open.active_secondary_addrs,
        vec![TetraAddress::issi(TEST_ISSI)],
        "simplex P2P shared assigned channel must identify both ISSIs so UMAC suspends EG for both active MSs"
    );
    assert_eq!(
        count_umac_floor_granted(&connect_msgs),
        0,
        "local private simplex U-CONNECT must not open U-plane before caller D-CONNECT is delivered"
    );
    assert_eq!(
        count_umac_floor_granted(&after_called_ack_msgs),
        0,
        "called D-CONNECT ACK local delivery should send caller D-CONNECT without enabling floor yet"
    );
    assert_eq!(
        count_umac_floor_granted(&after_caller_ack_msgs),
        1,
        "caller D-CONNECT L2 ACK should open the ETSI setup-granted U-plane floor"
    );
    assert_eq!(count_umac_floor_released(&connect_msgs), 0);
    assert_eq!(count_umac_floor_released(&after_called_ack_msgs), 0);
    assert_eq!(count_umac_floor_released(&after_caller_ack_msgs), 0);

    let d_connects: Vec<_> = after_called_ack_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        d_connects.len(),
        1,
        "direct setup sends caller D-CONNECT only after called D-CONNECT ACK local delivery"
    );
    for (prim, pdu) in &d_connects {
        assert_eq!(pdu.call_identifier, call_id);
        assert_eq!(pdu.simplex_duplex_selection, false);
        assert_eq!(pdu.transmission_grant, TransmissionGrant::Granted);
        assert!(!pdu.transmission_request_permission);
        assert!(pdu.call_ownership);
        assert_eq!(prim.main_address.ssi, TEST_ISSI);
        assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
        assert_eq!(prim.layer2service, Layer2Service::Acknowledged);
        assert!(!prim.stealing_permission);
        assert!(prim.tx_reporter.is_some());
        let chan_alloc = prim.chan_alloc.as_ref().expect("D-CONNECT should carry channel allocation");
        assert_chan_alloc_matches_circuit(chan_alloc, simplex_open.ts, simplex_open.usage, "D-CONNECT");
        assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Both);
    }

    let d_connect_acks: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect_acknowledge(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        d_connect_acks.len(),
        1,
        "U-CONNECT should first send one repeated unacknowledged D-CONNECT-ACKNOWLEDGE to the called MS"
    );
    for (prim, pdu) in &d_connect_acks {
        assert_eq!(pdu.call_identifier, call_id);
        assert_eq!(pdu.transmission_grant, TransmissionGrant::GrantedToOtherUser);
        assert!(!pdu.transmission_request_permission);
        assert_eq!(prim.main_address.ssi, TEST_CALLED_ISSI);
        assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
        assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
        assert_eq!(prim.unacked_bl_repetitions, Some(PRIVATE_SIMPLEX_CONNECT_ACK_UNACKED_REPETITIONS));
        assert!(
            !prim.stealing_permission,
            "called D-CONNECT-ACKNOWLEDGE with channel allocation starts on current-channel signalling; STCH-only stealing is reserved for recovery retry"
        );
        assert!(prim.tx_reporter.is_some());
        let chan_alloc = prim
            .chan_alloc
            .as_ref()
            .expect("D-CONNECT-ACKNOWLEDGE should carry channel allocation");
        assert_chan_alloc_matches_circuit(chan_alloc, simplex_open.ts, simplex_open.usage, "D-CONNECT-ACKNOWLEDGE");
        assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Both);
    }
}

#[test]
fn test_simple_private_call_full_direct_setup_and_release_workflow() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    assert!(
        !test.config.config().cell.transmission_interruption_enabled,
        "call_preemptive/transmission_interruption_enabled must remain default-off"
    );

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    // EN 300 392-2 clauses 14.5.1.1.2 and 14.7.2.3: a direct simple
    // individual call starts on common signalling with D-CALL PROCEEDING to
    // the caller and D-SETUP to the called ISSI. The assigned traffic circuit
    // is opened only after the called MS accepts with U-CONNECT.
    let (call_id, setup_msgs) = start_p2p_setup(&mut test);
    assert_eq!(
        setup_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_call_proceeding(prim).is_some()))
            .count(),
        1,
        "U-SETUP should receive one D-CALL PROCEEDING"
    );
    assert_eq!(count_d_setups(&setup_msgs), 1, "U-SETUP should emit one D-SETUP to the called MS");
    assert_eq!(count_umac_open(&setup_msgs), 0, "P2P setup phase must not open traffic");

    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let mut connect_msgs = test.dump_sinks();
    acknowledge_called_d_connect_ack(&connect_msgs, TEST_CALLED_ISSI);
    test.run_stack(Some(1));
    connect_msgs.extend(test.dump_sinks());
    acknowledge_first_d_connect(&connect_msgs);
    test.run_stack(Some(1));
    connect_msgs.extend(test.dump_sinks());

    assert_eq!(
        count_umac_open(&connect_msgs),
        1,
        "U-CONNECT should open one shared simplex circuit"
    );
    assert_eq!(
        connect_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some()))
            .count(),
        1,
        "simple private setup should send caller D-CONNECT after called D-CONNECT ACK local delivery"
    );
    assert_eq!(
        connect_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect_acknowledge(prim).is_some()))
            .count(),
        1,
        "simple private setup should send one repeated unacknowledged D-CONNECT-ACKNOWLEDGE to the called MS"
    );
    let caller_connect = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim),
            _ => None,
        })
        .next()
        .expect("simple private setup should include caller D-CONNECT");
    let called_connect_ack = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect_acknowledge(prim),
            _ => None,
        })
        .next()
        .expect("simple private setup should include called D-CONNECT ACKNOWLEDGE");
    assert_eq!(
        caller_connect.notification_indicator, None,
        "caller D-CONNECT stays compact so assigned-channel recovery fits FACCH/STCH"
    );
    assert_eq!(
        called_connect_ack.notification_indicator,
        Some(19),
        "called D-CONNECT ACKNOWLEDGE should mark the direct private setup as connected"
    );
    assert_eq!(
        count_d_tx_interrupt(&connect_msgs),
        0,
        "default simple private setup must not use pre-emptive interruption"
    );

    // EN 300 392-2 clause 14.5.1.3.1: after one party sends U-DISCONNECT it
    // waits for D-RELEASE. The same clause allows the SwMI to inform the local
    // simplex peer by D-RELEASE after bearer tail drain; no U-RELEASE response
    // is expected on this peer-clear path.
    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut initiator_release_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus_to(
        &initiator_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_ISSI],
    );
    assert_eq!(
        count_d_disconnects(&initiator_release_msgs),
        0,
        "simplex floor-holder U-DISCONNECT must tail-drain before peer D-RELEASE"
    );
    assert_eq!(
        count_umac_call_ended_or_close(&initiator_release_msgs),
        0,
        "traffic circuit must remain open while peer clear is tail-draining"
    );
    let release_ack_reporters = extract_d_release_reporters(&mut initiator_release_msgs);
    assert_eq!(
        release_ack_reporters.len(),
        1,
        "U-DISCONNECT initiator must receive one prompt assigned-channel D-RELEASE"
    );

    test.router
        .set_dl_time(dltime.add_timeslots(PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
    let mut peer_release_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&peer_release_msgs), 0);
    assert_established_p2p_release_pdus_to(
        &peer_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_CALLED_ISSI],
    );
    assert_eq!(
        count_umac_call_ended_or_close(&peer_release_msgs),
        0,
        "traffic circuit must remain open while called ISSI D-RELEASE is pending"
    );
    let peer_release_reporters = extract_d_release_reporters(&mut peer_release_msgs);
    assert_eq!(peer_release_reporters.len(), 1);
    assert_eq!(
        peer_release_reporters[0].get_state(),
        TxState::Pending,
        "called ISSI D-RELEASE must be reporter-tracked before circuit close"
    );
    peer_release_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert_eq!(
        count_umac_call_ended_or_close(&test.dump_sinks()),
        0,
        "peer D-RELEASE must still wait for caller D-RELEASE delivery"
    );

    for reporter in &release_ack_reporters {
        reporter.mark_transmitted();
    }
    test.run_stack(Some(1));
    assert!(
        count_umac_call_ended_or_close(&test.dump_sinks()) >= 2,
        "peer D-RELEASE and initiator D-RELEASE delivery should close the simple private call"
    );
}

#[test]
fn test_p2p_u_connect_with_unsupported_optional_function_returns_function_not_supported_without_opening_circuit() {
    debug::setup_logging_verbose();

    for unsupported in ["basic_service_information", "facility", "proprietary"] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
        let (call_id, _setup_msgs) = start_p2p_setup(&mut test);

        test.submit_message(build_u_connect_with_unsupported_feature_msg(TEST_CALLED_ISSI, call_id, unsupported));
        test.run_stack(Some(1));
        let connect_msgs = test.dump_sinks();

        // EN 300 392-2 table 14.23 allows U-CONNECT to carry optional basic
        // service/facility/proprietary elements. This SwMI only supports the
        // already-negotiated private speech service and no SS/proprietary call
        // setup functions here, so clause 14.7.3.2/table 14.33 is used before
        // any D-CONNECT ACKNOWLEDGE or traffic-channel open.
        assert_one_cmce_function_not_supported(&connect_msgs, TEST_CALLED_ISSI, CmcePduTypeUl::UConnect, Some(call_id), true);
        assert_eq!(count_umac_open(&connect_msgs), 0);
        assert_eq!(count_umac_call_ended_or_close(&connect_msgs), 0);
        assert_eq!(count_d_releases(&connect_msgs), 0);
        assert!(
            !connect_msgs
                .iter()
                .any(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some())),
            "unsupported U-CONNECT must not complete the calling side"
        );
        assert!(
            !connect_msgs
                .iter()
                .any(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect_acknowledge(prim).is_some())),
            "unsupported U-CONNECT must not acknowledge the called side"
        );
    }
}

#[test]
fn test_p2p_u_alert_with_unsupported_optional_function_returns_function_not_supported_without_alerting_caller() {
    debug::setup_logging_verbose();

    for unsupported in ["reserved", "basic_service_information", "facility", "proprietary"] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
        let (call_id, _setup_msgs) = start_p2p_setup(&mut test);

        test.submit_message(build_u_alert_with_unsupported_feature_msg(TEST_CALLED_ISSI, call_id, unsupported));
        test.run_stack(Some(1));
        let alert_msgs = test.dump_sinks();

        // EN 300 392-2 table 14.21 defines U-ALERT optional basic service,
        // facility and proprietary elements, and requires the reserved bit to
        // be 1. This SwMI only supports the already-negotiated alert path, so
        // clause 14.7.3.2/table 14.33 is used before alerting the caller.
        assert_one_cmce_function_not_supported(&alert_msgs, TEST_CALLED_ISSI, CmcePduTypeUl::UAlert, Some(call_id), true);
        assert!(
            alert_msgs
                .iter()
                .all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_alert(prim).is_some())),
            "unsupported U-ALERT must not alert the calling MS"
        );
        assert_eq!(count_umac_open(&alert_msgs), 0);
        assert_eq!(count_umac_call_ended_or_close(&alert_msgs), 0);
    }
}

#[test]
fn test_simple_private_call_works_with_transmission_interruption_enabled() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.transmission_interruption_enabled = true;
    let mut test = ComponentTest::from_config(config, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let (call_id, _setup_msgs) = start_p2p_setup(&mut test);
    let (mut connect_msgs, after_called_ack_msgs) =
        submit_p2p_connect_and_ack_called(&mut test, build_u_connect_msg(TEST_CALLED_ISSI, call_id), TEST_CALLED_ISSI);
    assert_eq!(
        connect_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some()))
            .count(),
        0,
        "caller D-CONNECT stays blocked until called D-CONNECT ACK local delivery"
    );
    connect_msgs.extend(after_called_ack_msgs);

    // EN 300 392-2 clause 14.5.1.2.1 f) only uses transmission interruption
    // for pre-emptive priority requests during an active transmission. Enabling
    // that optional SwMI support must not change ordinary simple private setup.
    assert_eq!(count_d_tx_interrupt(&connect_msgs), 0);
    assert_eq!(count_umac_open(&connect_msgs), 1);
    assert_eq!(
        connect_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some()))
            .count(),
        1,
        "simple private call should send one acknowledged caller D-CONNECT after called D-CONNECT ACK local delivery"
    );
    assert_eq!(
        connect_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect_acknowledge(prim).is_some()))
            .count(),
        1,
        "simple private call should send one repeated unacknowledged D-CONNECT ACKNOWLEDGE to the called MS"
    );
}

#[test]
fn test_simple_private_call_works_with_preemption_default_off() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    assert!(
        !test.config.config().cell.transmission_interruption_enabled,
        "call_preemptive/transmission_interruption_enabled must remain default-off"
    );

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let (call_id, _setup_msgs) = start_p2p_setup(&mut test);
    let (mut connect_msgs, after_called_ack_msgs) =
        submit_p2p_connect_and_ack_called(&mut test, build_u_connect_msg(TEST_CALLED_ISSI, call_id), TEST_CALLED_ISSI);
    assert_eq!(
        connect_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some()))
            .count(),
        0,
        "caller D-CONNECT stays blocked until called D-CONNECT ACK local delivery"
    );
    connect_msgs.extend(after_called_ack_msgs);

    // EN 300 392-2 clauses 14.5.1.2.1 and 14.7.2.3: a simple private
    // U-CONNECT first sends D-CONNECT-ACKNOWLEDGE to the called MS and, after
    // called-leg local delivery, sends D-CONNECT to the caller. Optional
    // transmission interruption/pre-emption is not part of this ordinary path.
    assert_eq!(count_d_tx_interrupt(&connect_msgs), 0);
    assert_eq!(count_umac_open(&connect_msgs), 1);
    assert_eq!(
        connect_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some()))
            .count(),
        1,
        "default-off simple private call should send one acknowledged caller D-CONNECT"
    );
    assert_eq!(
        connect_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect_acknowledge(prim).is_some()))
            .count(),
        1,
        "default-off simple private call should send one repeated unacknowledged D-CONNECT ACKNOWLEDGE"
    );
}

#[test]
fn test_simplex_p2p_preemptive_disconnect_cause_is_sanitized_when_unsupported() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    assert!(
        !test.config.config().cell.transmission_interruption_enabled,
        "call_preemptive/transmission_interruption_enabled must remain default-off"
    );

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_u_disconnect_with_cause_msg(
        TEST_ISSI,
        call_id,
        DisconnectCause::PreEmptiveUseOfResource,
    ));
    test.run_stack(Some(1));
    let mut initiator_release_msgs = test.dump_sinks();

    assert_eq!(
        count_d_tx_interrupt(&initiator_release_msgs),
        0,
        "private P2P release must not emit D-TX INTERRUPT when pre-emption is unsupported"
    );
    assert_established_p2p_release_pdus_to(
        &initiator_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_ISSI],
    );
    let release_ack_reporters = extract_d_release_reporters(&mut initiator_release_msgs);
    assert_eq!(release_ack_reporters.len(), 1);

    test.router
        .set_dl_time(dltime.add_timeslots(PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
    let peer_release_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&peer_release_msgs), 0);
    assert_established_p2p_release_pdus_to(
        &peer_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_CALLED_ISSI],
    );
}

#[test]
fn test_example_config_simple_private_call_works_with_preemption_default_off() {
    debug::setup_logging_verbose();

    let config_toml = include_str!("../../../example_config/config.toml");
    let config = from_toml_str(config_toml).expect("example config should parse");
    assert!(config_toml.contains("call_preemptive = false"));
    assert!(config_toml.contains("force_private_p2p_hook_signalling = false"));
    assert!(
        !config.cell.transmission_interruption_enabled,
        "example config must keep call_preemptive/transmission_interruption_enabled default-off"
    );
    assert!(
        !config.cell.force_private_p2p_hook_signalling,
        "example config must keep the private P2P hook override default-off"
    );
    assert!(
        config.cell.legacy_gssi_group_call,
        "example config should explicitly enable the lab legacy GSSI compatibility profile"
    );
    assert_eq!(
        config.cell.energy_saving_mode, ENERGY_SAVING_MODE_AUTO,
        "example config must keep EE on auto while keeping ordinary private call setup available"
    );

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(config, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    // EN 300 392-2 clauses 14.5.1.1.2 and 14.5.1.2.1: ordinary private
    // call setup proceeds through D-CALL PROCEEDING/D-SETUP and opens the
    // assigned channel only after U-CONNECT. Table 14.46 pre-emptive setup
    // remains outside this normal priority path when config keeps support off.
    let (call_id, setup_msgs) = start_p2p_setup(&mut test);
    assert_eq!(count_d_setups(&setup_msgs), 1);
    assert_eq!(count_d_tx_interrupt(&setup_msgs), 0);

    let (connect_msgs, _after_called_ack_msgs) =
        submit_p2p_connect_and_ack_called(&mut test, build_u_connect_msg(TEST_CALLED_ISSI, call_id), TEST_CALLED_ISSI);

    assert_eq!(
        count_umac_open(&connect_msgs),
        1,
        "example config private call should open normally"
    );
    assert_eq!(
        count_d_tx_interrupt(&connect_msgs),
        0,
        "example config simple private call must not use transmission interruption"
    );
}

#[test]
fn test_p2p_local_private_call_preserves_hook_method_and_config_timeout_fields() {
    debug::setup_logging_verbose();

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.call_timeout_secs = 800;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(config, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.hook_method_selection = true;
    let (call_id, setup_msgs) = start_p2p_setup_with_u_setup(&mut test, u_setup);

    let setups: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(setups.len(), 1, "Expected one D-SETUP to the called MS");
    let setup = &setups[0].1;
    assert_eq!(setup.call_identifier, call_id);
    // EN 300 392-2 tables 14.50 and 14.62: local private setup carries the
    // configured call timeout and selected hook method into D-SETUP.
    assert_eq!(setup.call_time_out, CallTimeout::T10m);
    assert!(setup.hook_method_selection);
    // EN 300 392-2 table 14.74: raw bit 0 means the calling MS requests
    // transmit/send data. The called MS is therefore told that the other user
    // has the setup-phase permission.
    assert_eq!(setup.transmission_grant, TransmissionGrant::GrantedToOtherUser);

    let (mut connect_msgs, after_called_ack_msgs) =
        submit_p2p_connect_and_ack_called(&mut test, build_u_connect_msg(TEST_CALLED_ISSI, call_id), TEST_CALLED_ISSI);

    let open_circuits: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .collect();
    assert_eq!(open_circuits.len(), 1, "simplex private call should open one shared traffic bearer");
    let open = open_circuits[0];
    // Called-leg D-CONNECT-ACKNOWLEDGE delivery precedes caller activation.
    // The shared bearer therefore keeps the called ISSI primary for setup
    // assigned-channel signalling attribution; U-plane opens only after the
    // caller D-CONNECT delivery completes the setup grant.
    assert_eq!(open.peer_ts, None);
    assert_eq!(open.active_addr, Some(TetraAddress::new(TEST_CALLED_ISSI, SsiType::Issi)));
    assert!(
        open.active_secondary_addrs.contains(&TetraAddress::new(TEST_ISSI, SsiType::Issi)),
        "shared simplex private bearer must still keep the calling MS active for assigned-channel listening"
    );
    assert_eq!(
        count_umac_floor_granted(&connect_msgs),
        0,
        "private simplex setup must not enable U-plane before caller D-CONNECT L2 ACK"
    );
    connect_msgs.extend(after_called_ack_msgs);
    assert_eq!(
        count_umac_floor_granted(&connect_msgs),
        1,
        "private simplex connect completion should open the ETSI setup-granted U-plane floor"
    );

    let d_connects: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        d_connects.len(),
        1,
        "Expected one caller D-CONNECT after called D-CONNECT ACK local delivery"
    );
    for (_, pdu) in &d_connects {
        // EN 300 392-2 clauses 14.7.1.4/14.7.2.3 keep the same timeout and
        // hook method on D-CONNECT when the simple private call is accepted.
        assert_eq!(pdu.call_identifier, call_id);
        assert_eq!(pdu.call_time_out, CallTimeout::T10m);
        assert!(pdu.hook_method_selection);
        assert!(!pdu.simplex_duplex_selection);
        assert_eq!(pdu.transmission_grant, TransmissionGrant::Granted);
    }

    let d_connect_acks: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect_acknowledge(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        d_connect_acks.len(),
        1,
        "Expected one repeated unacknowledged D-CONNECT-ACKNOWLEDGE"
    );
    for (_, pdu) in &d_connect_acks {
        assert_eq!(pdu.call_identifier, call_id);
        assert_eq!(pdu.call_time_out, CallTimeout::T10m);
        assert_eq!(pdu.transmission_grant, TransmissionGrant::GrantedToOtherUser);
    }
}

#[test]
fn test_p2p_duplex_request_accepts_called_simplex_offer() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.simplex_duplex_selection = true;
    let (call_id, _setup_msgs) = start_p2p_setup_with_u_setup(&mut test, u_setup);

    let (mut connect_msgs, after_called_ack_msgs) = submit_p2p_connect_and_ack_called(
        &mut test,
        build_u_connect_custom_msg_with_hook(TEST_CALLED_ISSI, call_id, false, false),
        TEST_CALLED_ISSI,
    );

    // EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 allow a called MS
    // that cannot support requested duplex to offer simplex in U-CONNECT.
    // The SwMI must not reject that valid simple private-call answer.
    assert_eq!(count_d_releases(&connect_msgs), 0);
    assert!(
        connect_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some())),
        "caller D-CONNECT stays blocked until called D-CONNECT ACK local delivery"
    );

    let open_circuits: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .collect();
    assert_eq!(
        open_circuits.len(),
        1,
        "duplex-to-simplex offer should release the unused second bearer before UMAC open"
    );
    let open = open_circuits[0];
    assert!(
        open.peer_ts.is_none(),
        "downgraded simplex private call must not cross-route to a second bearer"
    );
    assert_eq!(open.active_addr, Some(TetraAddress::new(TEST_CALLED_ISSI, SsiType::Issi)));
    assert!(
        open.active_secondary_addrs.contains(&TetraAddress::new(TEST_ISSI, SsiType::Issi)),
        "downgraded simplex private call must keep both MS awake on the shared assigned channel"
    );

    connect_msgs.extend(after_called_ack_msgs);
    let d_connects: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        d_connects.len(),
        1,
        "Expected one caller D-CONNECT after called D-CONNECT ACK local delivery"
    );
    for (prim, pdu) in &d_connects {
        assert_eq!(prim.main_address.ssi, TEST_ISSI);
        assert_eq!(pdu.call_identifier, call_id);
        assert!(!pdu.simplex_duplex_selection);
        assert_eq!(pdu.transmission_grant, TransmissionGrant::Granted);
    }

    let d_connect_acks: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect_acknowledge(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        d_connect_acks.len(),
        1,
        "Expected one repeated unacknowledged D-CONNECT-ACKNOWLEDGE"
    );
    for (prim, pdu) in &d_connect_acks {
        assert_eq!(prim.main_address.ssi, TEST_CALLED_ISSI);
        assert_eq!(pdu.call_identifier, call_id);
        assert_eq!(pdu.transmission_grant, TransmissionGrant::GrantedToOtherUser);
    }
}

#[test]
fn test_p2p_duplex_request_accepts_called_simplex_offer_in_u_alert() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.simplex_duplex_selection = true;
    let (call_id, _setup_msgs) = start_p2p_setup_with_u_setup(&mut test, u_setup);

    test.submit_message(build_u_alert_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let alert_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 allow a called MS
    // that cannot support requested duplex to offer simplex in U-ALERT.
    // The alert phase must propagate that offered service without opening
    // traffic yet; UMAC circuits are opened only after U-CONNECT.
    assert_eq!(count_d_releases(&alert_msgs), 0);
    assert_eq!(count_umac_open(&alert_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&alert_msgs), 0);

    let d_alerts: Vec<_> = alert_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_alert(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(d_alerts.len(), 1, "Expected one D-ALERT to the calling MS");
    assert_eq!(d_alerts[0].0.main_address.ssi, TEST_ISSI);
    assert_eq!(d_alerts[0].1.call_identifier, call_id);
    assert!(
        !d_alerts[0].1.simplex_duplex_selection,
        "D-ALERT must carry the called MS simplex offer to the caller"
    );

    let (mut connect_msgs, after_called_ack_msgs) = submit_p2p_connect_and_ack_called(
        &mut test,
        build_u_connect_custom_msg_with_hook(TEST_CALLED_ISSI, call_id, false, false),
        TEST_CALLED_ISSI,
    );

    assert_eq!(count_d_releases(&connect_msgs), 0);
    assert!(
        connect_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some())),
        "caller D-CONNECT stays blocked until called D-CONNECT ACK local delivery"
    );
    let open_circuits: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .collect();
    assert_eq!(
        open_circuits.len(),
        1,
        "U-ALERT simplex offer should leave one shared bearer for U-CONNECT"
    );
    assert!(
        open_circuits[0].peer_ts.is_none(),
        "simplex-offered private call must not keep duplex cross-routing"
    );
    assert!(
        open_circuits[0]
            .active_secondary_addrs
            .contains(&TetraAddress::new(TEST_ISSI, SsiType::Issi)),
        "simplex-offered private call must keep both MS awake on the shared assigned channel"
    );

    connect_msgs.extend(after_called_ack_msgs);
    let d_connects: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        d_connects.len(),
        1,
        "Expected one caller D-CONNECT after called D-CONNECT ACK local delivery"
    );
    for (prim, pdu) in &d_connects {
        assert_eq!(prim.main_address.ssi, TEST_ISSI);
        assert_eq!(pdu.call_identifier, call_id);
        assert!(!pdu.simplex_duplex_selection);
        assert_eq!(pdu.transmission_grant, TransmissionGrant::Granted);
    }

    let d_connect_acks: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect_acknowledge(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        d_connect_acks.len(),
        1,
        "Expected one repeated unacknowledged D-CONNECT-ACKNOWLEDGE"
    );
    for (prim, pdu) in &d_connect_acks {
        assert_eq!(prim.main_address.ssi, TEST_CALLED_ISSI);
        assert_eq!(pdu.call_identifier, call_id);
        assert_eq!(pdu.transmission_grant, TransmissionGrant::GrantedToOtherUser);
    }
}

#[test]
fn test_p2p_hook_setup_other_ms_request_seeds_called_setup_floor() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

    let mut u_setup = default_p2p_u_setup();
    u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);
    u_setup.hook_method_selection = true;
    u_setup.request_to_transmit_send_data = true;
    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs);
    let d_setups: Vec<_> = setup_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(d_setups.len(), 1, "Expected one D-SETUP to the called MS");
    assert_eq!(d_setups[0].0.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(d_setups[0].1.call_identifier, call_id);
    assert_eq!(d_setups[0].1.transmission_grant, TransmissionGrant::Granted);

    let (mut connect_msgs, after_called_ack_msgs) =
        submit_p2p_connect_and_ack_called(&mut test, build_u_connect_msg(TEST_CALLED_ISSI, call_id), TEST_CALLED_ISSI);

    let open_circuits: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .collect();
    assert_eq!(
        open_circuits.len(),
        1,
        "hook private simplex call should open one shared traffic bearer"
    );
    let open = open_circuits[0];
    // EN 300 392-2 clause 14.5.1.2.1 and table 14.74: with on/off-hook
    // signalling, raw bit 1 asks that the other MS may transmit/send data.
    // The connect grant gives the called MS the setup-phase floor. Later
    // U-TX DEMAND is only a floor refresh/change procedure.
    assert_eq!(open.peer_ts, None);
    assert_eq!(open.active_addr, Some(TetraAddress::new(TEST_CALLED_ISSI, SsiType::Issi)));
    let open_ts = open.ts;
    assert_eq!(
        open.active_secondary_addrs,
        vec![TetraAddress::new(TEST_ISSI, SsiType::Issi)],
        "shared simplex private bearer must still keep exactly the calling MS active for assigned-channel listening"
    );
    assert_eq!(
        count_umac_floor_granted(&connect_msgs),
        0,
        "called-first hook setup must not open U-plane before caller D-CONNECT L2 ACK"
    );
    connect_msgs.extend(after_called_ack_msgs);
    assert_eq!(
        count_umac_floor_granted(&connect_msgs),
        1,
        "called-first hook connect completion should open the called MS setup-granted floor"
    );

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let called_ptt_msgs = test.dump_sinks();
    assert_eq!(
        count_d_tx_granted(&called_ptt_msgs),
        2,
        "called MS explicit U-TX DEMAND should refresh/confirm both private-call parties"
    );
    assert_eq!(
        count_umac_floor_granted(&called_ptt_msgs),
        1,
        "called MS explicit U-TX DEMAND should refresh one U-plane floor"
    );
    assert!(called_ptt_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ts,
            }) if *got_call_id == call_id
                && *source_issi == TEST_CALLED_ISSI
                && *dest_gssi == TEST_ISSI
                && *ts == open_ts
        )
    }));

    let d_connects: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        d_connects.len(),
        1,
        "Expected one caller D-CONNECT after called D-CONNECT ACK local delivery"
    );
    for (prim, pdu) in &d_connects {
        assert_eq!(prim.main_address.ssi, TEST_ISSI);
        assert_eq!(pdu.call_identifier, call_id);
        assert_eq!(pdu.simplex_duplex_selection, false);
        assert_eq!(pdu.transmission_grant, TransmissionGrant::GrantedToOtherUser);
    }

    let d_connect_acks: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect_acknowledge(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        d_connect_acks.len(),
        1,
        "Expected one repeated unacknowledged D-CONNECT-ACKNOWLEDGE"
    );
    for (prim, pdu) in &d_connect_acks {
        assert_eq!(prim.main_address.ssi, TEST_CALLED_ISSI);
        assert_eq!(pdu.call_identifier, call_id);
        assert_eq!(pdu.transmission_grant, TransmissionGrant::Granted);
    }
}

#[test]
fn test_active_p2p_call_does_not_emit_late_entry_d_setup() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    // EN 300 392-2 Annex D describes back-up D-SETUP for group call
    // listeners, while the individual-call example repeats D-CONNECT
    // ACK/paging for the called MS. Once a private call is active, the cached
    // D-SETUP must not leak through the generic circuit late-entry path.
    test.run_stack(Some(8));
    let backup_window_msgs = test.dump_sinks();
    assert_eq!(
        count_d_setups(&backup_window_msgs),
        0,
        "active individual call_id={call_id} must not emit backup D-SETUP"
    );

    test.run_stack(Some(720));
    let late_entry_msgs = test.dump_sinks();
    assert_eq!(
        count_d_setups(&late_entry_msgs),
        0,
        "active individual call_id={call_id} must not emit late-entry D-SETUP"
    );
}

#[test]
fn test_p2p_u_connect_from_unexpected_issi_does_not_open_circuit() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let attacker_issi = 1000003;
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    register_subscriber(&mut test, attacker_issi, TEST_CALLED_GSSI);
    let (call_id, _setup_msgs) = start_p2p_setup(&mut test);

    test.submit_message(build_u_connect_msg(attacker_issi, call_id));
    test.run_stack(Some(1));
    let attacker_msgs = test.dump_sinks();

    // EN 300 392-2 private call setup binds D-CONNECT/U-CONNECT to the called
    // party leg. An unrelated ISSI must not activate the circuit or emit
    // D-CONNECT/D-CONNECT-ACKNOWLEDGE for the pending call.
    assert_eq!(count_umac_open(&attacker_msgs), 0);
    assert!(
        attacker_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim)
                if parse_d_connect(prim).is_some() || parse_d_connect_acknowledge(prim).is_some()))
    );

    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let called_msgs = test.dump_sinks();
    assert!(
        count_umac_open(&called_msgs) >= 1,
        "legitimate called-party U-CONNECT should still open the pending P2P circuit"
    );
}

#[test]
fn test_p2p_u_release_from_non_participant_is_rejected_without_teardown() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_u_release_msg(TEST_OTHER_ISSI, call_id));
    test.run_stack(Some(1));
    let release_msgs = test.dump_sinks();

    let releases: Vec<_> = release_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(releases.len(), 1, "non-participant U-RELEASE should receive a direct D-RELEASE");
    let (release_prim, release) = &releases[0];
    assert_eq!(release.call_identifier, call_id);
    assert_eq!(release.disconnect_cause, DisconnectCause::RequestedServiceNotAvailable);
    assert_eq!(release_prim.main_address.ssi, TEST_OTHER_ISSI);
    assert_eq!(release_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);
}

#[test]
fn test_p2p_u_disconnect_from_non_participant_is_rejected_without_teardown() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_OTHER_ISSI, call_id));
    test.run_stack(Some(1));
    let disconnect_msgs = test.dump_sinks();

    let releases: Vec<_> = disconnect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(releases.len(), 1, "non-participant U-DISCONNECT should receive a direct D-RELEASE");
    let (release_prim, release) = &releases[0];
    assert_eq!(release.call_identifier, call_id);
    assert_eq!(release.disconnect_cause, DisconnectCause::RequestedServiceNotAvailable);
    assert_eq!(release_prim.main_address.ssi, TEST_OTHER_ISSI);
    assert_eq!(release_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(count_umac_call_ended_or_close(&disconnect_msgs), 0);
}

#[test]
fn test_p2p_u_tx_demand_from_non_participant_is_denied_without_floor_handoff() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    register_subscriber(&mut test, TEST_OTHER_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    // EN 300 392-2 clause 14.5.1.2.1 scopes U-TX DEMAND to a user
    // application within the active call. A third ISSI must not be granted
    // the individual-call floor or cause a handoff between the two parties.
    test.submit_message(build_u_tx_demand_msg(TEST_OTHER_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        grants.len(),
        1,
        "non-participant private floor request should receive one explicit denial"
    );
    let (grant_prim, grant) = &grants[0];
    assert_eq!(grant.call_identifier, call_id);
    assert_eq!(grant.transmission_grant, TransmissionGrant::NotGranted.into_raw() as u8);
    assert!(!grant.transmission_request_permission);
    assert_eq!(grant.transmitting_party_type_identifier, None);
    assert_eq!(grant.transmitting_party_address_ssi, None);
    assert_eq!(grant_prim.main_address.ssi, TEST_OTHER_ISSI);
    assert_eq!(grant_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(grant_prim.layer2service, Layer2Service::Unacknowledged);
    assert!(grant_prim.chan_alloc.is_none());
    assert_eq!(count_d_tx_ceased(&demand_msgs), 0);
    assert_eq!(count_d_releases(&demand_msgs), 0);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
    assert_eq!(count_umac_floor_released(&demand_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&demand_msgs), 0);
}

#[test]
fn test_simplex_p2p_current_floor_holder_u_tx_demand_is_granted_not_denied() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let (call_id, connect_msgs) = start_active_p2p_call_with_connect_msgs(&mut test);
    let caller_ts = p2p_open_ts_for(&connect_msgs, TEST_ISSI);

    // EN 300 392-2 clause 14.5.1.2.1 b): the MS already holding the private
    // simplex floor must not be denied when its user application sends a PTT
    // demand around through-connection. Confirm or preserve the grant and keep
    // UMAC on the same active speaker.
    test.submit_message(build_u_tx_demand_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(grants.len(), 2, "current private floor holder PTT should notify both MSs");
    assert!(
        grants
            .iter()
            .all(|(_, grant)| grant.transmission_grant != TransmissionGrant::NotGranted.into_raw() as u8),
        "current private floor holder must not receive PTT denied"
    );
    let requester_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == TEST_ISSI && prim.main_address.ssi_type == SsiType::Issi)
        .expect("expected grant to current floor holder");
    assert_eq!(requester_grant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_eq!(
        requester_grant
            .0
            .chan_alloc
            .as_ref()
            .expect("current holder grant should carry FACCH allocation")
            .ul_dl_assigned,
        UlDlAssignment::Both
    );
    assert!(demand_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ts,
            }) if *got_call_id == call_id
                && *source_issi == TEST_ISSI
                && *dest_gssi == TEST_CALLED_ISSI
                && *ts == caller_ts
        )
    }));
    assert_eq!(count_d_tx_ceased(&demand_msgs), 0);
    assert_eq!(count_d_releases(&demand_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&demand_msgs), 0);
}

#[test]
fn test_simplex_p2p_same_speaker_rekey_during_tx_ceased_tail_suppresses_stale_floor_release() {
    debug::setup_logging_verbose();

    let caller_issi = LAB_ISSI_B;
    let called_issi = LAB_ISSI_MXP600;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, caller_issi, LAB_GROUP_GSSI);
    register_subscriber(&mut test, called_issi, LAB_GROUP_GSSI);

    test.submit_message(build_u_setup_p2p_msg(caller_issi, called_issi));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs);

    let (connect_msgs, _after_called_ack_msgs) =
        submit_p2p_connect_and_ack_called(&mut test, build_u_connect_msg(called_issi, call_id), called_issi);
    let caller_ts = p2p_open_ts_for(&connect_msgs, caller_issi);

    test.submit_message(build_u_tx_ceased_msg(caller_issi, call_id));
    test.run_stack(Some(1));
    let ceased_start_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&ceased_start_msgs), 0);
    assert_eq!(count_umac_floor_released(&ceased_start_msgs), 0);

    test.submit_message(build_u_tx_demand_msg(caller_issi, call_id));
    test.run_stack(Some(1));
    let rekey_msgs = test.dump_sinks();

    let grants: Vec<_> = rekey_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        grants.len(),
        2,
        "same-speaker private rekey should refresh both MSs with D-TX GRANTED"
    );
    let requester_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == caller_issi && prim.main_address.ssi_type == SsiType::Issi)
        .expect("same-speaker rekey should grant the requester");
    assert_eq!(requester_grant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_eq!(
        requester_grant
            .0
            .chan_alloc
            .as_ref()
            .expect("requester rekey grant should carry FACCH allocation")
            .ul_dl_assigned,
        UlDlAssignment::Both
    );
    let listener_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == called_issi && prim.main_address.ssi_type == SsiType::Issi)
        .expect("same-speaker rekey should refresh the listener");
    assert_eq!(
        listener_grant.1.transmission_grant,
        TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    );
    assert_eq!(
        listener_grant
            .0
            .chan_alloc
            .as_ref()
            .expect("listener rekey grant should carry FACCH allocation")
            .ul_dl_assigned,
        UlDlAssignment::Both
    );
    assert_eq!(count_d_tx_ceased(&rekey_msgs), 0);
    assert_eq!(count_umac_floor_released(&rekey_msgs), 0);
    assert_eq!(count_umac_floor_granted(&rekey_msgs), 1);
    assert!(rekey_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ts,
            }) if *got_call_id == call_id
                && *source_issi == caller_issi
                && *dest_gssi == called_issi
                && *ts == caller_ts
        )
    }));

    test.router
        .set_dl_time(dltime.add_timeslots(PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
    let stale_tail_msgs = test.dump_sinks();

    assert_eq!(
        count_d_tx_ceased(&stale_tail_msgs),
        0,
        "EN 300 392-2 clause 14.5.1.4.2: a stale D-TX CEASED must not switch U-plane off after the same MS was regranted"
    );
    assert_eq!(count_umac_floor_released(&stale_tail_msgs), 0);
    assert_eq!(count_umac_floor_granted(&stale_tail_msgs), 0);
}

#[test]
fn test_p2p_u_tx_demand_for_stale_private_call_id_is_explicitly_denied() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    let caller_issi = 2_260_082;
    let called_issi = 2_260_616;
    register_subscriber(&mut test, caller_issi, TEST_GSSI);
    register_subscriber(&mut test, called_issi, TEST_CALLED_GSSI);
    let stale_call_id = 0x1234;

    // EN 300 392-2 clause 14.5.1.2.1 b): a rejected request-to-transmit is
    // explicitly answered with D-TX GRANTED / transmission not granted. A
    // stale private PTT may arrive after BS call state is gone, so the denial
    // must go back on the requester's signalling link and must not allocate a
    // traffic channel or synthesize a group floor event.
    test.submit_message(build_u_tx_demand_msg(called_issi, stale_call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(grants.len(), 1, "stale private PTT should receive one explicit denial");
    let (grant_prim, grant) = &grants[0];
    assert_eq!(grant.call_identifier, stale_call_id);
    assert_eq!(grant.transmission_grant, TransmissionGrant::NotGranted.into_raw() as u8);
    assert!(!grant.transmission_request_permission);
    assert_eq!(grant.transmitting_party_type_identifier, None);
    assert_eq!(grant.transmitting_party_address_ssi, None);
    assert_eq!(grant_prim.main_address.ssi, called_issi);
    assert_eq!(grant_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(grant_prim.layer2service, Layer2Service::Unacknowledged);
    assert!(grant_prim.chan_alloc.is_none());
    assert_eq!(count_d_tx_ceased(&demand_msgs), 0);
    assert_eq!(count_d_releases(&demand_msgs), 0);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
    assert_eq!(count_umac_floor_released(&demand_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&demand_msgs), 0);
}

#[test]
fn test_p2p_u_tx_demand_with_unsupported_optional_function_returns_function_not_supported_without_floor_handoff() {
    debug::setup_logging_verbose();

    for unsupported in ["encryption_control", "reserved", "facility", "dm_ms_address", "proprietary"] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
        let call_id = start_active_p2p_call(&mut test);

        let msg = if unsupported == "reserved" {
            build_u_tx_demand_reserved_bit_msg(TEST_CALLED_ISSI, call_id)
        } else {
            let mut u_tx_demand = UTxDemand {
                call_identifier: call_id,
                tx_demand_priority: 0,
                encryption_control: false,
                reserved: false,
                facility: None,
                dm_ms_address: None,
                proprietary: None,
            };
            match unsupported {
                "encryption_control" => u_tx_demand.encryption_control = true,
                "facility" => u_tx_demand.facility = Some(type3_marker()),
                "dm_ms_address" => u_tx_demand.dm_ms_address = Some(type3_marker()),
                "proprietary" => u_tx_demand.proprietary = Some(type3_marker()),
                _ => unreachable!(),
            }
            build_u_tx_demand_custom_msg(TEST_CALLED_ISSI, u_tx_demand)
        };

        test.submit_message(msg);
        test.run_stack(Some(1));
        let demand_msgs = test.dump_sinks();

        // EN 300 392-2 table 14.32 defines U-TX DEMAND. The reserved bit
        // shall be 0, and facility/DM-MS/proprietary/encryption-control
        // handling is not implemented in this SwMI floor-control path. Clause
        // 14.7.3.2/table 14.33 gives an explicit unsupported-function response
        // for individually addressed CMCE PDUs; no floor state changes first.
        assert_one_cmce_function_not_supported(&demand_msgs, TEST_CALLED_ISSI, CmcePduTypeUl::UTxDemand, Some(call_id), true);
        assert_eq!(count_d_tx_granted(&demand_msgs), 0);
        assert_eq!(count_d_tx_ceased(&demand_msgs), 0);
        assert_eq!(count_d_releases(&demand_msgs), 0);
        assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
        assert_eq!(count_umac_floor_released(&demand_msgs), 0);
        assert_eq!(count_umac_call_ended_or_close(&demand_msgs), 0);
    }
}

#[test]
fn test_simplex_p2p_u_tx_demand_from_non_holder_is_queued_without_floor_handoff() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.2.1 b) says that if the other MS is
    // transmitting, SwMI should normally wait for U-TX CEASED before
    // granting transmission. The requester gets an explicit queued response.
    let grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(grants.len(), 1, "non-holder U-TX DEMAND should only answer the requester");
    let (grant_prim, grant) = &grants[0];
    assert_eq!(grant_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(grant_prim.main_address.ssi_type, SsiType::Issi);
    assert_eq!(grant.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert_eq!(grant.transmitting_party_type_identifier, None);
    assert_eq!(grant.transmitting_party_address_ssi, None);
    assert_eq!(
        grant_prim.sdu.get_len(),
        25,
        "P2P D-TX GRANTED should omit optional transmitting-party address so it fits FACCH"
    );
    assert!(!grant.transmission_request_permission);
    assert_eq!(count_d_tx_ceased(&demand_msgs), 0);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
}

#[test]
fn test_simplex_p2p_preemptive_u_tx_demand_default_off_is_queued_without_interrupt() {
    debug::setup_logging_verbose();

    // EN 300 392-2 clause 14.5.1.2.1 b) is the baseline when the SwMI does
    // not support private-call transmission interruption: wait for U-TX CEASED
    // and explicitly queue/reject the request. EN 300 392-2 table 14.85
    // marks priorities 2 and 3 as pre-emptive/emergency, but the local config
    // keeps D-TX INTERRUPT support default-off.
    for tx_demand_priority in [2, 3] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
        let call_id = start_active_p2p_call(&mut test);

        test.submit_message(build_u_tx_demand_msg_with_priority(TEST_CALLED_ISSI, call_id, tx_demand_priority));
        test.run_stack(Some(1));
        let demand_msgs = test.dump_sinks();

        assert_eq!(count_d_tx_interrupt(&demand_msgs), 0, "priority {tx_demand_priority}");
        let grants: Vec<_> = demand_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(
            grants.len(),
            1,
            "pre-emptive P2P request should only answer the requester for priority {tx_demand_priority}"
        );
        let (grant_prim, grant) = &grants[0];
        assert_eq!(grant_prim.main_address.ssi, TEST_CALLED_ISSI, "priority {tx_demand_priority}");
        assert_eq!(
            grant.transmission_grant,
            TransmissionGrant::RequestQueued.into_raw() as u8,
            "priority {tx_demand_priority}"
        );
        assert_eq!(grant.transmitting_party_address_ssi, None, "priority {tx_demand_priority}");
        assert_eq!(grant.transmitting_party_type_identifier, None, "priority {tx_demand_priority}");
        assert_eq!(grant_prim.sdu.get_len(), 25, "priority {tx_demand_priority}");
        assert_eq!(count_d_tx_ceased(&demand_msgs), 0, "priority {tx_demand_priority}");
        assert_eq!(count_umac_floor_granted(&demand_msgs), 0, "priority {tx_demand_priority}");
        assert_eq!(count_umac_floor_released(&demand_msgs), 0, "priority {tx_demand_priority}");
    }
}

#[test]
fn test_simplex_p2p_preemptive_u_tx_demand_enabled_interrupts_current_speaker_before_grant() {
    debug::setup_logging_verbose();

    // EN 300 392-2 clause 14.5.1.2.1 f) and table 14.85: with SwMI
    // transmission interruption support enabled, priority 2/3 U-TX DEMAND
    // interrupts the current simplex private transmitter and grants the
    // requester.
    for tx_demand_priority in [2, 3] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
        config.cell.transmission_interruption_enabled = true;
        let mut test = ComponentTest::from_config(config, Some(dltime));

        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
        let call_id = start_active_p2p_call(&mut test);

        test.submit_message(build_u_tx_demand_msg_with_priority(TEST_CALLED_ISSI, call_id, tx_demand_priority));
        test.run_stack(Some(1));
        let demand_msgs = test.dump_sinks();

        let interrupts: Vec<_> = demand_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_interrupt(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(
            interrupts.len(),
            1,
            "pre-emptive P2P request should interrupt exactly the current speaker for priority {tx_demand_priority}"
        );
        let (interrupt_prim, interrupt) = &interrupts[0];
        assert_eq!(interrupt_prim.main_address.ssi, TEST_ISSI, "priority {tx_demand_priority}");
        assert_eq!(interrupt_prim.main_address.ssi_type, SsiType::Issi, "priority {tx_demand_priority}");
        assert!(interrupt_prim.stealing_permission, "priority {tx_demand_priority}");
        assert_eq!(
            interrupt_prim
                .chan_alloc
                .as_ref()
                .expect("D-TX INTERRUPT must carry assigned-channel allocation")
                .ul_dl_assigned,
            UlDlAssignment::Dl,
            "priority {tx_demand_priority}"
        );
        assert_eq!(
            interrupt.transmission_grant,
            TransmissionGrant::GrantedToOtherUser.into_raw() as u8,
            "priority {tx_demand_priority}"
        );
        assert_eq!(
            interrupt.transmitting_party_type_identifier,
            Some(1),
            "priority {tx_demand_priority}"
        );
        assert_eq!(
            interrupt.transmitting_party_address_ssi,
            Some(TEST_CALLED_ISSI as u64),
            "priority {tx_demand_priority}"
        );

        let grants: Vec<_> = demand_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(
            grants.len(),
            1,
            "pre-emptive P2P request should grant exactly the requester for priority {tx_demand_priority}"
        );
        let (grant_prim, grant) = &grants[0];
        assert_eq!(grant_prim.main_address.ssi, TEST_CALLED_ISSI, "priority {tx_demand_priority}");
        assert_eq!(
            grant.transmission_grant,
            TransmissionGrant::Granted.into_raw() as u8,
            "priority {tx_demand_priority}"
        );
        assert_eq!(grant.transmitting_party_type_identifier, None, "priority {tx_demand_priority}");
        assert_eq!(grant.transmitting_party_address_ssi, None, "priority {tx_demand_priority}");
        assert_eq!(grant_prim.sdu.get_len(), 25, "priority {tx_demand_priority}");
        assert_eq!(count_d_tx_ceased(&demand_msgs), 0, "priority {tx_demand_priority}");
        assert_eq!(count_umac_floor_granted(&demand_msgs), 1, "priority {tx_demand_priority}");
        assert_eq!(count_umac_floor_released(&demand_msgs), 0, "priority {tx_demand_priority}");
    }
}

#[test]
fn test_simplex_p2p_u_tx_ceased_hands_floor_to_queued_requester() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.2.1 e): when a request was already queued,
    // SwMI should hand over with D-TX GRANTED and without an explicit
    // D-TX CEASED.
    assert_eq!(count_d_tx_ceased(&ceased_msgs), 0);
    let grants: Vec<_> = ceased_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(grants.len(), 2, "queued private floor handoff should notify both MSs");

    let requester_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == TEST_CALLED_ISSI && prim.main_address.ssi_type == SsiType::Issi)
        .expect("expected queued requester grant");
    assert_eq!(requester_grant.1.call_identifier, call_id);
    assert_eq!(requester_grant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_eq!(requester_grant.1.transmitting_party_type_identifier, None);
    assert_eq!(requester_grant.1.transmitting_party_address_ssi, None);
    assert_eq!(requester_grant.0.unacked_bl_repetitions, Some(0));
    assert_eq!(requester_grant.0.sdu.get_len(), 25);
    let requester_alloc = requester_grant
        .0
        .chan_alloc
        .as_ref()
        .expect("requester grant should carry FACCH channel allocation");
    assert_eq!(requester_alloc.ul_dl_assigned, UlDlAssignment::Both);

    let listener_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == TEST_ISSI && prim.main_address.ssi_type == SsiType::Issi)
        .expect("expected former speaker listener grant");
    assert_eq!(
        listener_grant.1.transmission_grant,
        TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    );
    assert_eq!(listener_grant.1.transmitting_party_type_identifier, None);
    assert_eq!(listener_grant.1.transmitting_party_address_ssi, None);
    assert_eq!(listener_grant.0.unacked_bl_repetitions, Some(0));
    assert_eq!(listener_grant.0.sdu.get_len(), 25);
    let listener_alloc = listener_grant
        .0
        .chan_alloc
        .as_ref()
        .expect("listener grant should carry FACCH channel allocation");
    assert_eq!(listener_alloc.ul_dl_assigned, UlDlAssignment::Both);

    assert_eq!(count_umac_floor_granted(&ceased_msgs), 1);
    assert!(ceased_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: got_call_id,
                source_issi,
                dest_gssi,
                ..
            }) if *got_call_id == call_id && *source_issi == TEST_CALLED_ISSI && *dest_gssi == TEST_ISSI
        )
    }));
}

#[test]
fn test_simplex_p2p_u_tx_demand_after_idle_floor_grants_with_bidirectional_allocation() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_start_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&ceased_start_msgs), 0);

    test.router
        .set_dl_time(dltime.add_timeslots(PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
    let tail_msgs = test.dump_sinks();
    assert_eq!(
        count_d_tx_ceased(&tail_msgs),
        2,
        "idle simplex private floor should emit D-TX CEASED only after bearer-tail drain"
    );

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.2.1 b) uses the transmission grant IE to
    // switch the granted MS to transmit and the peer to receive. The attached
    // channel allocation is kept Both (clause 21.5.2) so an already assigned
    // simplex private channel remains coherent while UMAC gates the active UL
    // speaker by ISSI.
    let grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(grants.len(), 2, "idle private floor request should notify both MSs");

    let requester_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == TEST_CALLED_ISSI && prim.main_address.ssi_type == SsiType::Issi)
        .expect("expected requester grant");
    assert_eq!(requester_grant.1.call_identifier, call_id);
    assert_eq!(requester_grant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert!(!requester_grant.1.transmission_request_permission);
    assert_eq!(requester_grant.0.unacked_bl_repetitions, Some(0));
    let requester_alloc = requester_grant
        .0
        .chan_alloc
        .as_ref()
        .expect("requester grant should carry FACCH channel allocation");
    assert_eq!(requester_alloc.ul_dl_assigned, UlDlAssignment::Both);

    let listener_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == TEST_ISSI && prim.main_address.ssi_type == SsiType::Issi)
        .expect("expected listener grant");
    assert_eq!(
        listener_grant.1.transmission_grant,
        TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    );
    assert!(!listener_grant.1.transmission_request_permission);
    assert_eq!(listener_grant.0.unacked_bl_repetitions, Some(0));
    let listener_alloc = listener_grant
        .0
        .chan_alloc
        .as_ref()
        .expect("listener grant should carry FACCH channel allocation");
    assert_eq!(listener_alloc.ul_dl_assigned, UlDlAssignment::Both);

    assert_eq!(count_umac_floor_granted(&demand_msgs), 1);
}

#[test]
fn test_simplex_p2p_field_issis_u_tx_demand_after_idle_floor_uses_bidirectional_grants() {
    debug::setup_logging_verbose();

    let caller_issi = 2_260_082;
    let called_issi = 2_260_616;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, caller_issi, TEST_GSSI);
    register_subscriber(&mut test, called_issi, TEST_CALLED_GSSI);

    test.submit_message(build_u_setup_p2p_msg(caller_issi, called_issi));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs);

    let _ = submit_p2p_connect_and_ack_called(&mut test, build_u_connect_msg(called_issi, call_id), called_issi);
    let _ = grant_initial_p2p_floor(&mut test, caller_issi, call_id);

    test.submit_message(build_u_tx_ceased_msg(caller_issi, call_id));
    test.run_stack(Some(1));
    let ceased_start_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&ceased_start_msgs), 0);

    test.router
        .set_dl_time(dltime.add_timeslots(PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
    let tail_msgs = test.dump_sinks();
    assert_eq!(
        count_d_tx_ceased(&tail_msgs),
        2,
        "field private floor should become idle only after bearer-tail D-TX CEASED"
    );

    test.submit_message(build_u_tx_demand_msg(called_issi, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.2.1 b) makes the transmission-grant IE the
    // PTT decision. Table 21.5.2 permits "Both" for channel allocation; keeping
    // both directions avoids reassigning an active field radio to UL-only/DL-only
    // while UMAC FloorGranted still gates the single transmitting ISSI.
    let grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(grants.len(), 2, "field ISSI idle private floor request should notify both MSs");
    assert!(
        grants
            .iter()
            .all(|(_, grant)| grant.transmission_grant != TransmissionGrant::NotGranted.into_raw() as u8),
        "field ISSI private floor request must not be denied"
    );

    let requester_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == called_issi && prim.main_address.ssi_type == SsiType::Issi)
        .expect("expected grant to field requester");
    assert_eq!(requester_grant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_eq!(
        requester_grant
            .0
            .chan_alloc
            .as_ref()
            .expect("requester field grant should carry FACCH allocation")
            .ul_dl_assigned,
        UlDlAssignment::Both
    );

    let listener_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == caller_issi && prim.main_address.ssi_type == SsiType::Issi)
        .expect("expected grant-to-other-user to field listener");
    assert_eq!(
        listener_grant.1.transmission_grant,
        TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    );
    assert_eq!(
        listener_grant
            .0
            .chan_alloc
            .as_ref()
            .expect("listener field grant should carry FACCH allocation")
            .ul_dl_assigned,
        UlDlAssignment::Both
    );
    assert_eq!(count_umac_floor_granted(&demand_msgs), 1);
}

#[test]
fn test_simplex_p2p_field_issis_non_holder_u_tx_demand_is_queued_not_denied() {
    debug::setup_logging_verbose();

    let caller_issi = 2_260_082;
    let called_issi = 2_260_616;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, caller_issi, TEST_GSSI);
    register_subscriber(&mut test, called_issi, TEST_CALLED_GSSI);

    test.submit_message(build_u_setup_p2p_msg(caller_issi, called_issi));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs);

    let _ = submit_p2p_connect_and_ack_called(&mut test, build_u_connect_msg(called_issi, call_id), called_issi);
    let _ = grant_initial_p2p_floor(&mut test, caller_issi, call_id);

    test.submit_message(build_u_tx_demand_msg(called_issi, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.2.1 b): when the other private-call party
    // already has the floor, a normal non-preemptive request is queued or
    // rejected explicitly. For a legitimate two-party field call, the first
    // non-holder request should be queued, not denied.
    let grants: Vec<_> = demand_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(grants.len(), 1, "field non-holder PTT should answer only the requester");
    let (grant_prim, grant) = &grants[0];
    assert_eq!(grant_prim.main_address.ssi, called_issi);
    assert_eq!(grant.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert_ne!(grant.transmission_grant, TransmissionGrant::NotGranted.into_raw() as u8);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
}

#[test]
fn test_simplex_p2p_field_issis_queued_handoff_uses_requester_source_both_directions() {
    debug::setup_logging_verbose();

    for (caller_issi, called_issi) in [(2_260_082, 2_260_616), (2_260_616, 2_260_082)] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, caller_issi, TEST_GSSI);
        register_subscriber(&mut test, called_issi, TEST_CALLED_GSSI);

        test.submit_message(build_u_setup_p2p_msg(caller_issi, called_issi));
        test.run_stack(Some(1));
        let setup_msgs = test.dump_sinks();
        let call_id = first_d_setup_call_id(&setup_msgs);

        let (connect_msgs, _after_called_ack_msgs) =
            submit_p2p_connect_and_ack_called(&mut test, build_u_connect_msg(called_issi, call_id), called_issi);
        let caller_ts = p2p_open_ts_for(&connect_msgs, caller_issi);
        let _ = grant_initial_p2p_floor(&mut test, caller_issi, call_id);

        test.submit_message(build_u_tx_demand_msg(called_issi, call_id));
        test.run_stack(Some(1));
        let demand_msgs = test.dump_sinks();
        let queued_grants: Vec<_> = demand_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(queued_grants.len(), 1, "field non-holder PTT should queue for caller {caller_issi}");
        assert_eq!(queued_grants[0].0.main_address.ssi, called_issi);
        assert_eq!(
            queued_grants[0].1.transmission_grant,
            TransmissionGrant::RequestQueued.into_raw() as u8
        );
        assert_eq!(count_umac_floor_granted(&demand_msgs), 0);

        test.submit_message(build_u_tx_ceased_msg(caller_issi, call_id));
        test.run_stack(Some(1));
        let ceased_msgs = test.dump_sinks();

        // EN 300 392-2 clause 14.5.1.2.1 e): with a queued request present,
        // U-TX CEASED hands the floor directly to that requester using
        // D-TX GRANTED to both MSs. The UMAC source/ts must name the requester,
        // independent of which field ISSI was caller or called.
        assert_eq!(count_d_tx_ceased(&ceased_msgs), 0);
        let handoff_grants: Vec<_> = ceased_msgs
            .iter()
            .filter_map(|msg| match &msg.msg {
                SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
                _ => None,
            })
            .collect();
        assert_eq!(handoff_grants.len(), 2, "queued field handoff should notify both MSs");

        let requester_grant = handoff_grants
            .iter()
            .find(|(prim, _)| prim.main_address.ssi == called_issi && prim.main_address.ssi_type == SsiType::Issi)
            .expect("expected queued requester grant");
        assert_eq!(requester_grant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
        assert_eq!(
            requester_grant
                .0
                .chan_alloc
                .as_ref()
                .expect("requester handoff grant should carry FACCH allocation")
                .ul_dl_assigned,
            UlDlAssignment::Both
        );

        let listener_grant = handoff_grants
            .iter()
            .find(|(prim, _)| prim.main_address.ssi == caller_issi && prim.main_address.ssi_type == SsiType::Issi)
            .expect("expected former speaker listener grant");
        assert_eq!(
            listener_grant.1.transmission_grant,
            TransmissionGrant::GrantedToOtherUser.into_raw() as u8
        );
        assert_eq!(
            listener_grant
                .0
                .chan_alloc
                .as_ref()
                .expect("listener handoff grant should carry FACCH allocation")
                .ul_dl_assigned,
            UlDlAssignment::Both
        );

        assert_eq!(count_umac_floor_granted(&ceased_msgs), 1);
        assert!(ceased_msgs.iter().any(|msg| {
            matches!(
                &msg.msg,
                SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                    call_id: got_call_id,
                    source_issi,
                    dest_gssi,
                    ts,
                }) if *got_call_id == call_id
                    && *source_issi == called_issi
                    && *dest_gssi == caller_issi
                    && *ts == caller_ts
            )
        }));
    }
}

#[test]
fn test_simplex_p2p_field_issis_queued_u_tx_ceased_withdraws_before_handoff() {
    debug::setup_logging_verbose();

    for (caller_issi, called_issi) in [(2_260_082, 2_260_616), (2_260_616, 2_260_082)] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, caller_issi, TEST_GSSI);
        register_subscriber(&mut test, called_issi, TEST_CALLED_GSSI);

        test.submit_message(build_u_setup_p2p_msg(caller_issi, called_issi));
        test.run_stack(Some(1));
        let setup_msgs = test.dump_sinks();
        let call_id = first_d_setup_call_id(&setup_msgs);

        let (connect_msgs, _after_called_ack_msgs) =
            submit_p2p_connect_and_ack_called(&mut test, build_u_connect_msg(called_issi, call_id), called_issi);
        let caller_ts = p2p_open_ts_for(&connect_msgs, caller_issi);
        let _ = grant_initial_p2p_floor(&mut test, caller_issi, call_id);

        test.submit_message(build_u_tx_demand_msg(called_issi, call_id));
        test.run_stack(Some(1));
        let demand_msgs = test.dump_sinks();
        assert_eq!(count_d_tx_granted(&demand_msgs), 1);

        test.submit_message(build_u_tx_ceased_msg(called_issi, call_id));
        test.run_stack(Some(1));
        let withdraw_msgs = test.dump_sinks();

        // EN 300 392-2 clause 14.5.1.2.1 a): an MS may withdraw a queued
        // request before it has been granted by sending U-TX CEASED. That must
        // clear the queued requester without granting a stale reverse floor.
        assert_eq!(count_d_tx_granted(&withdraw_msgs), 0);
        assert_eq!(count_d_tx_ceased(&withdraw_msgs), 0);
        assert_eq!(count_umac_floor_granted(&withdraw_msgs), 0);
        assert_eq!(count_umac_floor_released(&withdraw_msgs), 0);

        test.submit_message(build_u_tx_ceased_msg(caller_issi, call_id));
        test.run_stack(Some(1));
        let cease_after_withdraw_start_msgs = test.dump_sinks();

        assert_eq!(count_d_tx_granted(&cease_after_withdraw_start_msgs), 0);
        assert_eq!(
            count_d_tx_ceased(&cease_after_withdraw_start_msgs),
            0,
            "withdrawn field queue must not emit D-TX CEASED before bearer-tail drain"
        );
        assert_eq!(count_umac_floor_granted(&cease_after_withdraw_start_msgs), 0);
        assert_eq!(count_umac_floor_released(&cease_after_withdraw_start_msgs), 0);

        test.router
            .set_dl_time(dltime.add_timeslots(PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
        test.run_stack(Some(1));
        let cease_after_withdraw_msgs = test.dump_sinks();

        assert_eq!(count_d_tx_granted(&cease_after_withdraw_msgs), 0);
        assert_eq!(
            count_d_tx_ceased(&cease_after_withdraw_msgs),
            2,
            "withdrawn field queue must not be granted when the original speaker ceases"
        );
        assert_eq!(count_umac_floor_granted(&cease_after_withdraw_msgs), 0);
        assert_eq!(count_umac_floor_released(&cease_after_withdraw_msgs), 1);
        assert!(cease_after_withdraw_msgs.iter().any(|msg| {
            matches!(
                &msg.msg,
                SapMsgInner::CmceCallControl(CallControl::FloorReleased {
                    call_id: got_call_id,
                    ts,
                }) if *got_call_id == call_id && *ts == caller_ts
            )
        }));
    }
}

#[test]
fn test_simplex_p2p_u_tx_ceased_without_queued_request_does_not_grant_peer() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let (call_id, connect_msgs) = start_active_p2p_call_with_connect_msgs(&mut test);
    let caller_ts = p2p_open_ts_for(&connect_msgs, TEST_ISSI);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_start_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.2.1 b/e) forbids unsolicited D-TX
    // GRANTED, but allows D-TX CEASED to each MS so both CC entities leave
    // the active transmission state. Peer-facing cease is delayed by the
    // bearer-tail drain before it is sent.
    assert_eq!(count_d_tx_granted(&ceased_start_msgs), 0);
    assert_eq!(count_d_tx_ceased(&ceased_start_msgs), 0);
    assert_eq!(count_umac_floor_granted(&ceased_start_msgs), 0);
    assert_eq!(count_umac_floor_released(&ceased_start_msgs), 0);

    test.router
        .set_dl_time(dltime.add_timeslots(PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
    let ceased_msgs = test.dump_sinks();

    assert_eq!(count_d_tx_granted(&ceased_msgs), 0);
    let ceased: Vec<_> = ceased_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_ceased(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(ceased.len(), 2, "end of simplex private transmission should notify both MSs");
    for (prim, pdu) in &ceased {
        assert_eq!(pdu.call_identifier, call_id);
        assert!(!pdu.transmission_request_permission);
        assert_eq!(
            prim.unacked_bl_repetitions,
            Some(0),
            "D-TX CEASED FACCH must be single-shot so it cannot repeat over a later D-TX GRANTED"
        );
    }
    assert!(ceased.iter().any(|(prim, _)| prim.main_address.ssi == TEST_ISSI));
    assert!(ceased.iter().any(|(prim, _)| prim.main_address.ssi == TEST_CALLED_ISSI));
    assert_eq!(count_umac_floor_granted(&ceased_msgs), 0);
    assert_eq!(count_umac_floor_released(&ceased_msgs), 1);
    assert!(ceased_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorReleased {
                call_id: got_call_id,
                ts,
            }) if *got_call_id == call_id && *ts == caller_ts
        )
    }));
}

#[test]
fn test_simplex_p2p_ul_inactivity_without_queued_request_does_not_grant_peer() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let (call_id, connect_msgs) = start_active_p2p_call_with_connect_msgs(&mut test);
    let caller_ts = p2p_open_ts_for(&connect_msgs, TEST_ISSI);

    test.submit_message(build_ul_inactivity_timeout_msg(caller_ts));
    test.run_stack(Some(1));
    let timeout_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.2.1 b/e) forbids unsolicited D-TX
    // GRANTED. A local UL inactivity guard may force the old speaker off the
    // floor and send D-TX CEASED to both MSs, but it must not grant the peer
    // unless that peer already requested transmission.
    assert_eq!(count_d_tx_granted(&timeout_msgs), 0);
    let ceased: Vec<_> = timeout_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_ceased(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        ceased.len(),
        2,
        "UL inactivity floor release should notify both private-call parties"
    );
    for (_, pdu) in &ceased {
        assert_eq!(pdu.call_identifier, call_id);
        assert!(!pdu.transmission_request_permission);
    }
    assert!(ceased.iter().any(|(prim, _)| prim.main_address.ssi == TEST_ISSI));
    assert!(ceased.iter().any(|(prim, _)| prim.main_address.ssi == TEST_CALLED_ISSI));
    assert_eq!(count_umac_floor_granted(&timeout_msgs), 0);
    assert_eq!(count_umac_floor_released(&timeout_msgs), 1);
    assert!(timeout_msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorReleased {
                call_id: got_call_id,
                ts,
            }) if *got_call_id == call_id && *ts == caller_ts
        )
    }));
}

#[test]
fn test_simplex_p2p_ul_inactivity_hands_floor_to_queued_requester() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let (call_id, connect_msgs) = start_active_p2p_call_with_connect_msgs(&mut test);
    let caller_ts = p2p_open_ts_for(&connect_msgs, TEST_ISSI);

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_ul_inactivity_timeout_msg(caller_ts));
    test.run_stack(Some(1));
    let timeout_msgs = test.dump_sinks();

    // A queued U-TX DEMAND is the scoped exception in EN 300 392-2
    // clause 14.5.1.2.1 e): SwMI may hand over with D-TX GRANTED to both MSs
    // and without an explicit D-TX CEASED.
    assert_eq!(count_d_tx_ceased(&timeout_msgs), 0);
    let grants: Vec<_> = timeout_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(grants.len(), 2, "queued timeout handoff should notify both MSs");

    let requester_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == TEST_CALLED_ISSI && prim.main_address.ssi_type == SsiType::Issi)
        .expect("expected queued requester grant");
    assert_eq!(requester_grant.1.call_identifier, call_id);
    assert_eq!(requester_grant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_eq!(requester_grant.1.transmitting_party_type_identifier, None);
    assert_eq!(requester_grant.1.transmitting_party_address_ssi, None);
    assert_eq!(requester_grant.0.sdu.get_len(), 25);
    let requester_alloc = requester_grant
        .0
        .chan_alloc
        .as_ref()
        .expect("requester grant should carry FACCH channel allocation");
    assert_eq!(requester_alloc.ul_dl_assigned, UlDlAssignment::Both);

    let listener_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == TEST_ISSI && prim.main_address.ssi_type == SsiType::Issi)
        .expect("expected former speaker listener grant");
    assert_eq!(
        listener_grant.1.transmission_grant,
        TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    );
    assert_eq!(listener_grant.1.transmitting_party_type_identifier, None);
    assert_eq!(listener_grant.1.transmitting_party_address_ssi, None);
    assert_eq!(listener_grant.0.sdu.get_len(), 25);
    let listener_alloc = listener_grant
        .0
        .chan_alloc
        .as_ref()
        .expect("listener grant should carry FACCH channel allocation");
    assert_eq!(listener_alloc.ul_dl_assigned, UlDlAssignment::Both);

    assert_eq!(count_umac_floor_granted(&timeout_msgs), 1);
}

#[test]
fn test_p2p_u_tx_ceased_from_non_participant_is_ignored_without_floor_handoff() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    register_subscriber(&mut test, TEST_OTHER_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    // EN 300 392-2 clause 14.5.1.2.1 permits D-TX GRANTED only for the
    // granted MS and the other MS involved in the call. A non-participant
    // U-TX CEASED must not switch floor ownership to either call party.
    test.submit_message(build_u_tx_ceased_msg(TEST_OTHER_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_msgs = test.dump_sinks();

    assert_eq!(count_d_tx_granted(&ceased_msgs), 0);
    assert_eq!(count_d_tx_ceased(&ceased_msgs), 0);
    assert_eq!(count_d_releases(&ceased_msgs), 0);
    assert_eq!(count_umac_floor_granted(&ceased_msgs), 0);
    assert_eq!(count_umac_floor_released(&ceased_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&ceased_msgs), 0);
}

#[test]
fn test_duplex_p2p_ignores_tx_floor_pdus_without_channel_rewrite() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_duplex_p2p_call(&mut test);

    // EN 300 392-2 clause 14.5.1.2.1 grants full-duplex individual calls to
    // both parties during D-CONNECT/D-CONNECT ACKNOWLEDGE. Later PTT floor
    // messages must not downgrade the traffic assignment to simplex UL/DL.
    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_granted(&demand_msgs), 0);
    assert_eq!(count_d_tx_ceased(&demand_msgs), 0);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
    assert_eq!(count_umac_floor_released(&demand_msgs), 0);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_granted(&ceased_msgs), 0);
    assert_eq!(count_d_tx_ceased(&ceased_msgs), 0);
    assert_eq!(count_umac_floor_granted(&ceased_msgs), 0);
    assert_eq!(count_umac_floor_released(&ceased_msgs), 0);
}

#[test]
fn test_unsolicited_private_u_release_does_not_start_disconnect_pending() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    // EN 300 392-2 clause 14.5.1.3.3 makes U-RELEASE the MS response to a
    // BS D-DISCONNECT. Without a pending D-DISCONNECT, it must not start the
    // private-call disconnect handshake or release the traffic circuit.
    test.submit_message(build_u_release_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let release_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&release_msgs), 0);
    assert_eq!(
        release_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_disconnect(prim).is_some()))
            .count(),
        0
    );
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let disconnect_start_msgs = test.dump_sinks();
    assert_eq!(
        disconnect_start_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_disconnect(prim).is_some()))
            .count(),
        0,
        "simplex caller U-DISCONNECT should tail-drain before called-peer D-RELEASE"
    );
    assert_established_p2p_release_pdus_to(
        &disconnect_start_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_ISSI],
    );
    assert_eq!(count_umac_call_ended_or_close(&disconnect_start_msgs), 0);

    test.router
        .set_dl_time(dltime.add_timeslots(PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
    let mut peer_release_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&peer_release_msgs), 0);
    assert_established_p2p_release_pdus_to(
        &peer_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_CALLED_ISSI],
    );
    assert_eq!(count_umac_call_ended_or_close(&peer_release_msgs), 0);
    let peer_release_reporters = extract_d_release_reporters(&mut peer_release_msgs);
    assert_eq!(peer_release_reporters.len(), 1);
    peer_release_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert_eq!(count_umac_call_ended_or_close(&test.dump_sinks()), 0);
}

#[test]
fn test_p2p_u_disconnect_with_unsupported_optional_function_returns_function_not_supported_without_disconnect_handshake() {
    debug::setup_logging_verbose();

    for unsupported in ["facility", "proprietary"] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
        let call_id = start_active_p2p_call(&mut test);

        test.submit_message(build_u_disconnect_with_unsupported_feature_msg(TEST_ISSI, call_id, unsupported));
        test.run_stack(Some(1));
        let disconnect_msgs = test.dump_sinks();

        // EN 300 392-2 table 14.24 allows Facility/Proprietary on
        // U-DISCONNECT. This SwMI does not implement those call-control
        // functions, so clause 14.7.3.2/table 14.33 is used before starting the
        // D-DISCONNECT/D-RELEASE clearing handshake.
        assert_one_cmce_function_not_supported(&disconnect_msgs, TEST_ISSI, CmcePduTypeUl::UDisconnect, Some(call_id), true);
        assert!(
            disconnect_msgs
                .iter()
                .all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_disconnect(prim).is_some())),
            "unsupported U-DISCONNECT must not start D-DISCONNECT"
        );
        assert_eq!(count_d_releases(&disconnect_msgs), 0);
        assert_eq!(count_umac_call_ended_or_close(&disconnect_msgs), 0);
    }
}

#[test]
fn test_p2p_u_release_with_unsupported_optional_function_returns_function_not_supported_without_clearing_call() {
    debug::setup_logging_verbose();

    for unsupported in ["facility", "proprietary"] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
        let call_id = start_active_p2p_call(&mut test);

        test.submit_message(build_u_release_with_unsupported_feature_msg(TEST_ISSI, call_id, unsupported));
        test.run_stack(Some(1));
        let release_msgs = test.dump_sinks();

        // EN 300 392-2 table 14.29 allows Facility/Proprietary on U-RELEASE.
        // Without support for those functions, the BS responds with CMCE
        // FUNCTION NOT SUPPORTED and leaves the active call intact.
        assert_one_cmce_function_not_supported(&release_msgs, TEST_ISSI, CmcePduTypeUl::URelease, Some(call_id), true);
        assert_eq!(count_d_releases(&release_msgs), 0);
        assert!(
            release_msgs
                .iter()
                .all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_disconnect(prim).is_some())),
            "unsupported U-RELEASE must not start D-DISCONNECT"
        );
        assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);
    }
}

#[test]
fn test_p2p_u_info_with_unsupported_optional_function_returns_function_not_supported_without_side_effects() {
    debug::setup_logging_verbose();

    for unsupported in ["modify", "facility", "proprietary"] {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
        let call_id = start_active_p2p_call(&mut test);

        test.submit_message(build_u_info_with_unsupported_feature_msg(TEST_ISSI, call_id, unsupported));
        test.run_stack(Some(1));
        let info_msgs = test.dump_sinks();

        // EN 300 392-2 table 14.26 defines U-INFO Modify, Facility and
        // Proprietary information elements. This stack only implements the DTMF
        // subset today, so unsupported call-modification/SS/proprietary
        // functions are rejected before any local side effect.
        assert_one_cmce_function_not_supported(&info_msgs, TEST_ISSI, CmcePduTypeUl::UInfo, Some(call_id), true);
        assert!(
            info_msgs
                .iter()
                .all(|msg| !matches!(&msg.msg, SapMsgInner::CmceCallControl(CallControl::NetworkCircuitDtmf { .. }))),
            "unsupported U-INFO must not emit DTMF/control side effects"
        );
        assert_eq!(count_d_releases(&info_msgs), 0);
        assert_eq!(count_umac_call_ended_or_close(&info_msgs), 0);
    }
}

#[test]
fn test_p2p_u_alert_from_unexpected_issi_does_not_alert_caller() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let attacker_issi = 1000003;
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    register_subscriber(&mut test, attacker_issi, TEST_CALLED_GSSI);
    let (call_id, _setup_msgs) = start_p2p_setup(&mut test);

    test.submit_message(build_u_alert_msg(attacker_issi, call_id));
    test.run_stack(Some(1));
    let attacker_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.7.2.1 U-ALERT is the called MS response to
    // D-SETUP. An unrelated ISSI must not alert the caller for this call.
    assert!(
        attacker_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_alert(prim).is_some()))
    );

    test.submit_message(build_u_alert_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let called_msgs = test.dump_sinks();
    let alerts: Vec<_> = called_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_alert(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(alerts.len(), 1, "legitimate called-party U-ALERT should emit one D-ALERT");
    assert_eq!(alerts[0].0.main_address.ssi, TEST_ISSI);
    assert_eq!(alerts[0].1.call_identifier, call_id);
}

#[test]
fn test_p2p_u_disconnect_tail_drains_peer_release_before_circuit_close() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    // EN 300 392-2 clause 14.5.1.3.1: the MS that sent U-DISCONNECT
    // receives D-RELEASE promptly. The same clause permits the peer leg to be
    // informed by D-RELEASE; unlike D-DISCONNECT, it expects no U-RELEASE and
    // avoids the MXP600 peer-D-DISCONNECT reboot path found in RF testing.
    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut initiator_release_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus_to(
        &initiator_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_ISSI],
    );
    assert_eq!(
        count_d_disconnects(&initiator_release_msgs),
        0,
        "simplex caller U-DISCONNECT must tail-drain before peer D-RELEASE"
    );
    assert_eq!(
        count_umac_call_ended_or_close(&initiator_release_msgs),
        0,
        "P2P circuit must stay open while peer clear is tail-draining"
    );
    let release_ack_reporters = extract_d_release_reporters(&mut initiator_release_msgs);
    assert_eq!(
        release_ack_reporters.len(),
        1,
        "U-DISCONNECT initiator should receive one prompt assigned-channel D-RELEASE"
    );

    test.run_stack(Some(3));
    let early_tail_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&early_tail_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&early_tail_msgs), 0);

    test.router
        .set_dl_time(dltime.add_timeslots(PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
    let mut peer_release_msgs = test.dump_sinks();

    assert_eq!(count_d_disconnects(&peer_release_msgs), 0);
    assert_eq!(count_d_setups(&peer_release_msgs), 0);
    assert_established_p2p_release_pdus_to(
        &peer_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_CALLED_ISSI],
    );
    assert_eq!(
        count_umac_call_ended_or_close(&peer_release_msgs),
        0,
        "P2P circuit must stay open while peer D-RELEASE is pending"
    );
    let peer_release_reporters = extract_d_release_reporters(&mut peer_release_msgs);
    assert_eq!(
        peer_release_reporters.len(),
        1,
        "Assigned-channel peer D-RELEASE must carry one TxReporter"
    );
    assert_eq!(peer_release_reporters[0].get_state(), TxState::Pending);
    test.run_stack(Some(3));
    let pending_peer_release_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_call_ended_or_close(&pending_peer_release_msgs),
        0,
        "P2P circuit must stay open while peer D-RELEASE delivery is still pending"
    );

    peer_release_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let pending_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_call_ended_or_close(&pending_msgs),
        0,
        "Pending individual release should not close before initiator D-RELEASE reporter completion"
    );

    for reporter in &release_ack_reporters {
        reporter.mark_transmitted();
    }
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "peer D-RELEASE and initiator D-RELEASE delivery should close the P2P traffic circuit"
    );
}

#[test]
fn test_p2p_disconnect_tail_drain_ignores_late_tx_demands_without_not_granted() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut initiator_release_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus_to(
        &initiator_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_ISSI],
    );
    let release_ack_reporters = extract_d_release_reporters(&mut initiator_release_msgs);
    assert_eq!(release_ack_reporters.len(), 1);

    // EN 300 392-2 clauses 14.5.1.3.1/14.5.1.3.3 put the established
    // individual call into disconnection clearance. Floor requests arriving
    // during the bearer tail drain are stale and must not be answered with
    // D-TX GRANTED/NotGranted that a terminal can render as PTT denied.
    for issi in [TEST_CALLED_ISSI, TEST_ISSI] {
        test.submit_message(build_u_tx_demand_msg(issi, call_id));
        test.run_stack(Some(1));
        let demand_msgs = test.dump_sinks();
        assert_eq!(count_d_tx_granted(&demand_msgs), 0, "late U-TX DEMAND from ISSI {issi}");
        assert_eq!(count_umac_floor_granted(&demand_msgs), 0, "late U-TX DEMAND from ISSI {issi}");
        assert_eq!(count_umac_floor_released(&demand_msgs), 0, "late U-TX DEMAND from ISSI {issi}");
        assert_eq!(count_d_disconnects(&demand_msgs), 0, "late U-TX DEMAND from ISSI {issi}");
        assert_eq!(count_d_releases(&demand_msgs), 0, "late U-TX DEMAND from ISSI {issi}");
        assert_eq!(count_umac_call_ended_or_close(&demand_msgs), 0, "late U-TX DEMAND from ISSI {issi}");
    }

    drain_private_simplex_tail(&mut test, dltime);
    let mut peer_release_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&peer_release_msgs), 0);
    assert_established_p2p_release_pdus_to(
        &peer_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_CALLED_ISSI],
    );
    let peer_release_reporters = extract_d_release_reporters(&mut peer_release_msgs);
    assert_eq!(peer_release_reporters.len(), 1);
    peer_release_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert_eq!(count_umac_call_ended_or_close(&test.dump_sinks()), 0);

    release_ack_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert!(
        count_umac_call_ended_or_close(&test.dump_sinks()) >= 2,
        "P2P circuit should close after peer D-RELEASE and initiator D-RELEASE delivery"
    );
}

#[test]
fn test_p2p_disconnect_pending_ignores_tx_demands_before_peer_release_delivery() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut initiator_release_msgs = test.dump_sinks();
    let release_ack_reporters = extract_d_release_reporters(&mut initiator_release_msgs);
    assert_eq!(release_ack_reporters.len(), 1);

    drain_private_simplex_tail(&mut test, dltime);
    let mut peer_release_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus_to(
        &peer_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_CALLED_ISSI],
    );
    let peer_release_reporters = extract_d_release_reporters(&mut peer_release_msgs);
    assert_eq!(peer_release_reporters.len(), 1);

    // During reporter-tracked peer D-RELEASE delivery, additional PTT requests
    // are stale floor control and must not become terminal-visible NotGranted
    // responses.
    for issi in [TEST_CALLED_ISSI, TEST_ISSI] {
        test.submit_message(build_u_tx_demand_msg(issi, call_id));
        test.run_stack(Some(1));
        let demand_msgs = test.dump_sinks();
        assert_eq!(
            count_d_tx_granted(&demand_msgs),
            0,
            "disconnect-pending U-TX DEMAND from ISSI {issi}"
        );
        assert_eq!(
            count_umac_floor_granted(&demand_msgs),
            0,
            "disconnect-pending U-TX DEMAND from ISSI {issi}"
        );
        assert_eq!(
            count_umac_floor_released(&demand_msgs),
            0,
            "disconnect-pending U-TX DEMAND from ISSI {issi}"
        );
        assert_eq!(
            count_d_disconnects(&demand_msgs),
            0,
            "disconnect-pending U-TX DEMAND from ISSI {issi}"
        );
        assert_eq!(count_d_releases(&demand_msgs), 0, "disconnect-pending U-TX DEMAND from ISSI {issi}");
        assert_eq!(
            count_umac_call_ended_or_close(&demand_msgs),
            0,
            "disconnect-pending U-TX DEMAND from ISSI {issi}"
        );
    }

    peer_release_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert_eq!(count_umac_call_ended_or_close(&test.dump_sinks()), 0);

    release_ack_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert!(
        count_umac_call_ended_or_close(&test.dump_sinks()) >= 2,
        "P2P circuit should close after peer D-RELEASE and initiator D-RELEASE delivery"
    );
}

#[test]
fn test_p2p_caller_disconnect_tail_drains_when_mxp600_peer_holds_floor() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_MXP600, LAB_GROUP_GSSI);
    let (call_id, _connect_msgs) = start_active_p2p_call_between_with_connect_msgs(&mut test, LAB_ISSI_A, LAB_ISSI_MXP600);

    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_MXP600, call_id));
    test.run_stack(Some(1));
    let queued_msgs = test.dump_sinks();
    let queued_grants: Vec<_> = queued_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(queued_grants.len(), 1);
    assert_eq!(queued_grants[0].0.main_address, TetraAddress::issi(LAB_ISSI_MXP600));
    assert_eq!(
        queued_grants[0].1.transmission_grant,
        TransmissionGrant::RequestQueued.into_raw() as u8
    );

    test.submit_message(build_u_tx_ceased_msg(LAB_ISSI_A, call_id));
    test.run_stack(Some(1));
    let handoff_msgs = test.dump_sinks();
    let handoff_grants: Vec<_> = handoff_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(handoff_grants.len(), 2, "queued private floor handoff should grant both legs");
    assert!(handoff_grants.iter().any(|(prim, grant)| {
        prim.main_address == TetraAddress::issi(LAB_ISSI_MXP600) && grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8
    }));
    assert!(handoff_grants.iter().any(|(prim, grant)| {
        prim.main_address == TetraAddress::issi(LAB_ISSI_A)
            && grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    }));
    assert_eq!(count_umac_floor_granted(&handoff_msgs), 1);

    // Field regression for 2260616 -> 2260618: if the MXP600 peer is the
    // current simplex floor holder and the caller presses the red key, keep the
    // caller D-RELEASE prompt but drain the peer-facing D-RELEASE so bearer
    // tail bits finish before the terminal receives the final clear.
    test.submit_message(build_u_disconnect_msg(LAB_ISSI_A, call_id));
    test.run_stack(Some(1));
    let mut initiator_release_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus_to(
        &initiator_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[LAB_ISSI_A],
    );
    assert_no_d_info(&initiator_release_msgs);
    assert_release_notification_to(&initiator_release_msgs, LAB_ISSI_A, None);
    assert_eq!(
        count_d_disconnects(&initiator_release_msgs),
        0,
        "caller hangup must tail-drain before peer clear when the MXP600 peer holds the floor"
    );
    assert_eq!(count_umac_call_ended_or_close(&initiator_release_msgs), 0);
    let release_ack_reporters = extract_d_release_reporters(&mut initiator_release_msgs);
    assert_eq!(release_ack_reporters.len(), 1);

    test.run_stack(Some(3));
    let early_tail_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&early_tail_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&early_tail_msgs), 0);

    drain_private_simplex_tail(&mut test, dltime);
    let mut peer_release_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&peer_release_msgs), 0);
    assert_established_p2p_release_pdus_to(
        &peer_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[LAB_ISSI_MXP600],
    );
    assert_no_d_info(&peer_release_msgs);
    assert_release_notification_to(&peer_release_msgs, LAB_ISSI_MXP600, None);
    assert_eq!(count_umac_call_ended_or_close(&peer_release_msgs), 0);

    let peer_release_reporters = extract_d_release_reporters(&mut peer_release_msgs);
    assert_eq!(peer_release_reporters.len(), 1);
    peer_release_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert_eq!(count_umac_call_ended_or_close(&test.dump_sinks()), 0);

    release_ack_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert!(
        count_umac_call_ended_or_close(&test.dump_sinks()) >= 2,
        "P2P circuit closes only after peer D-RELEASE and caller D-RELEASE delivery"
    );
}

#[test]
fn test_p2p_caller_disconnect_clears_mxp600_peer_with_release_after_peer_ceased_last_floor() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_MXP600, LAB_GROUP_GSSI);
    let (call_id, _connect_msgs) = start_active_p2p_call_between_with_connect_msgs(&mut test, LAB_ISSI_A, LAB_ISSI_MXP600);

    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_MXP600, call_id));
    test.run_stack(Some(1));
    let queued_msgs = test.dump_sinks();
    assert_eq!(
        count_d_tx_granted(&queued_msgs),
        1,
        "MXP600 peer floor request should be queued while caller still has the simplex floor"
    );

    test.submit_message(build_u_tx_ceased_msg(LAB_ISSI_A, call_id));
    test.run_stack(Some(1));
    let handoff_msgs = test.dump_sinks();
    let handoff_grants: Vec<_> = handoff_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(handoff_grants.len(), 2, "queued floor handoff should grant both private legs");
    assert!(handoff_grants.iter().any(|(prim, grant)| {
        prim.main_address == TetraAddress::issi(LAB_ISSI_MXP600) && grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8
    }));

    test.submit_message(build_u_tx_ceased_msg(LAB_ISSI_MXP600, call_id));
    test.run_stack(Some(1));
    let peer_cease_msgs = test.dump_sinks();
    assert_eq!(count_d_releases(&peer_cease_msgs), 0);
    assert_eq!(count_d_disconnects(&peer_cease_msgs), 0);
    assert_eq!(
        count_umac_call_ended_or_close(&peer_cease_msgs),
        0,
        "peer U-TX CEASED must not close the private bearer"
    );

    let after_peer_cease_tail = dltime.add_timeslots(PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS);
    test.router.set_dl_time(after_peer_cease_tail);
    test.run_stack(Some(1));
    let peer_ceased_tail_msgs = test.dump_sinks();
    assert_eq!(
        count_d_tx_ceased(&peer_ceased_tail_msgs),
        2,
        "tail-drained peer U-TX CEASED should notify both private-call legs"
    );
    assert_eq!(count_umac_floor_released(&peer_ceased_tail_msgs), 1);
    assert_eq!(count_umac_call_ended_or_close(&peer_ceased_tail_msgs), 0);

    // Field regression for the live MXP600 reboot close: 2260618 was the last
    // simplex speaker, sent U-TX CEASED, and then 2260616 cleared the call.
    // Keep a bearer-tail drain and use the ETSI-allowed peer D-RELEASE
    // alternative; peer D-DISCONNECT on this path repeatedly rebooted MXP600.
    test.submit_message(build_u_disconnect_msg(LAB_ISSI_A, call_id));
    test.run_stack(Some(1));
    let mut initiator_release_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus_to(
        &initiator_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[LAB_ISSI_A],
    );
    assert_no_d_info(&initiator_release_msgs);
    assert_release_notification_to(&initiator_release_msgs, LAB_ISSI_A, None);
    assert_eq!(
        count_d_disconnects(&initiator_release_msgs),
        0,
        "caller hangup must not send peer clear while peer release is tail-draining"
    );
    assert_eq!(count_umac_call_ended_or_close(&initiator_release_msgs), 0);
    let release_ack_reporters = extract_d_release_reporters(&mut initiator_release_msgs);
    assert_eq!(release_ack_reporters.len(), 1);

    let after_disconnect_tail =
        after_peer_cease_tail.add_timeslots(PRIVATE_SIMPLEX_TAIL_DRAIN_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS);
    test.router.set_dl_time(after_disconnect_tail);
    test.run_stack(Some(1));
    let mut peer_release_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&peer_release_msgs), 0);
    assert_established_p2p_release_pdus_to(
        &peer_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[LAB_ISSI_MXP600],
    );
    assert_no_d_info(&peer_release_msgs);
    assert_release_notification_to(&peer_release_msgs, LAB_ISSI_MXP600, None);
    assert_eq!(count_umac_call_ended_or_close(&peer_release_msgs), 0);

    let peer_release_reporters = extract_d_release_reporters(&mut peer_release_msgs);
    assert_eq!(peer_release_reporters.len(), 1);
    peer_release_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert_eq!(count_umac_call_ended_or_close(&test.dump_sinks()), 0);

    release_ack_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert!(
        count_umac_call_ended_or_close(&test.dump_sinks()) >= 2,
        "P2P circuit closes only after peer D-RELEASE and caller D-RELEASE delivery"
    );
}

#[test]
fn test_p2p_caller_disconnect_cancels_pending_peer_tx_ceased_tail() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, LAB_ISSI_A, LAB_GROUP_GSSI);
    register_subscriber(&mut test, LAB_ISSI_MXP600, LAB_GROUP_GSSI);
    let (call_id, _connect_msgs) = start_active_p2p_call_between_with_connect_msgs(&mut test, LAB_ISSI_A, LAB_ISSI_MXP600);

    test.submit_message(build_u_tx_demand_msg(LAB_ISSI_MXP600, call_id));
    test.run_stack(Some(1));
    let queued_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_granted(&queued_msgs), 1);

    test.submit_message(build_u_tx_ceased_msg(LAB_ISSI_A, call_id));
    test.run_stack(Some(1));
    let handoff_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_granted(&handoff_msgs), 2);
    assert_eq!(count_umac_floor_granted(&handoff_msgs), 1);

    test.submit_message(build_u_tx_ceased_msg(LAB_ISSI_MXP600, call_id));
    test.run_stack(Some(1));
    let peer_cease_start_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&peer_cease_start_msgs), 0);
    assert_eq!(count_umac_floor_released(&peer_cease_start_msgs), 0);
    assert_eq!(count_d_disconnects(&peer_cease_start_msgs), 0);
    assert_eq!(count_d_releases(&peer_cease_start_msgs), 0);

    // Regression for 2260616 -> 2260618 when the MXP600 releases PTT and the
    // caller presses red before the peer TX-CEASED tail drain expires. The
    // disconnect clear supersedes floor-idle signalling, so no stale
    // D-TX CEASED may leak before the peer D-RELEASE.
    test.submit_message(build_u_disconnect_msg(LAB_ISSI_A, call_id));
    test.run_stack(Some(1));
    let mut initiator_release_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&initiator_release_msgs), 0);
    assert_eq!(count_umac_floor_released(&initiator_release_msgs), 0);
    assert_eq!(count_d_disconnects(&initiator_release_msgs), 0);
    assert_established_p2p_release_pdus_to(
        &initiator_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[LAB_ISSI_A],
    );
    let release_ack_reporters = extract_d_release_reporters(&mut initiator_release_msgs);
    assert_eq!(release_ack_reporters.len(), 1);

    drain_private_simplex_tail(&mut test, dltime);
    let mut peer_release_msgs = test.dump_sinks();
    assert_eq!(
        count_d_tx_ceased(&peer_release_msgs),
        0,
        "disconnect must suppress stale peer U-TX CEASED completion"
    );
    assert_eq!(count_umac_floor_released(&peer_release_msgs), 0);
    assert_eq!(count_d_disconnects(&peer_release_msgs), 0);
    assert_established_p2p_release_pdus_to(
        &peer_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[LAB_ISSI_MXP600],
    );

    let peer_release_reporters = extract_d_release_reporters(&mut peer_release_msgs);
    assert_eq!(peer_release_reporters.len(), 1);
    peer_release_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert_eq!(count_umac_call_ended_or_close(&test.dump_sinks()), 0);

    release_ack_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert!(
        count_umac_call_ended_or_close(&test.dump_sinks()) >= 2,
        "P2P circuit closes after caller D-RELEASE and peer D-RELEASE"
    );
}

#[test]
fn test_p2p_called_party_u_disconnect_waits_for_caller_release_before_circuit_close() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    // EN 300 392-2 clause 14.5.1.3.1 permits either user application to
    // initiate individual-call disconnection. If the called MS disconnects, it
    // receives D-RELEASE promptly. Because the calling peer did not request
    // release, the peer-facing clear waits for the bounded bearer drain and
    // then uses D-RELEASE, the clause 14.5.1.3.1 peer-clear alternative that
    // expects no U-RELEASE response.
    test.submit_message(build_u_disconnect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let mut called_release_msgs = test.dump_sinks();

    assert_established_p2p_release_pdus_to(
        &called_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_CALLED_ISSI],
    );
    assert_eq!(
        count_d_disconnects(&called_release_msgs),
        0,
        "called-party U-DISCONNECT must tail-drain before clearing the floor-holding caller"
    );
    assert_eq!(count_umac_call_ended_or_close(&called_release_msgs), 0);
    let release_ack_reporters = extract_d_release_reporters(&mut called_release_msgs);
    assert_eq!(
        release_ack_reporters.len(),
        1,
        "Called-party U-DISCONNECT initiator must receive one prompt assigned-channel D-RELEASE"
    );

    drain_private_simplex_tail(&mut test, dltime);
    let mut peer_release_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&peer_release_msgs), 0);
    assert_established_p2p_release_pdus_to(
        &peer_release_msgs,
        call_id,
        DisconnectCause::UserRequestedDisconnection,
        &[TEST_ISSI],
    );
    assert_eq!(count_umac_call_ended_or_close(&peer_release_msgs), 0);

    let peer_release_reporters = extract_d_release_reporters(&mut peer_release_msgs);
    assert_eq!(
        peer_release_reporters.len(),
        1,
        "Assigned-channel D-RELEASE to caller peer must carry one TxReporter"
    );
    assert_eq!(peer_release_reporters[0].get_state(), TxState::Pending);

    peer_release_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let transmitted_delivery_msgs = test.dump_sinks();
    assert_eq!(count_umac_call_ended_or_close(&transmitted_delivery_msgs), 0);

    for reporter in &release_ack_reporters {
        reporter.mark_transmitted();
    }
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "Reporter completion should close the called-party-disconnected P2P traffic circuit"
    );
}

#[test]
fn test_duplex_p2p_peer_u_disconnect_or_u_release_does_not_duplicate_pending_release() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_duplex_p2p_call(&mut test);

    let (requester_release_reporters, peer_release_reporters) =
        start_called_party_disconnect_with_peer_d_release(&mut test, dltime, call_id);

    // EN 300 392-2 clause 14.5.1.3.1 permits the peer leg to be informed by
    // D-RELEASE. This local RF path deliberately has no peer U-RELEASE
    // handshake, so stale peer release/disconnect indications must not create
    // duplicate clear PDUs or close before both D-RELEASE reporters complete.
    for msg in [build_u_disconnect_msg(TEST_ISSI, call_id), build_u_release_msg(TEST_ISSI, call_id)] {
        test.submit_message(msg);
        test.run_stack(Some(1));
        let duplicate_msgs = test.dump_sinks();
        assert_eq!(count_d_disconnects(&duplicate_msgs), 0);
        assert_eq!(count_d_releases(&duplicate_msgs), 0);
        assert_eq!(count_umac_call_ended_or_close(&duplicate_msgs), 0);
    }

    peer_release_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert_eq!(
        count_umac_call_ended_or_close(&test.dump_sinks()),
        0,
        "duplex peer D-RELEASE delivery must still wait for requester D-RELEASE delivery"
    );

    requester_release_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert!(
        count_umac_call_ended_or_close(&test.dump_sinks()) >= 2,
        "duplex P2P circuit should close after requester and peer D-RELEASE delivery"
    );
}

#[test]
fn test_p2p_pending_release_ignores_duplicate_u_disconnect_and_tx_demand() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_duplex_p2p_call(&mut test);

    let (requester_release_reporters, peer_release_reporters) =
        start_called_party_disconnect_with_peer_d_release(&mut test, dltime, call_id);

    // EN 300 392-2 clause 14.5.1.3.1 clears this user-initiated established
    // individual call with D-RELEASE. During the local FACCH/STCH delivery
    // drain, duplicate disconnects or PTT floor requests must not create new
    // call-maintenance signalling for the same call identifier.
    test.submit_message(build_u_disconnect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let duplicate_disconnect_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&duplicate_disconnect_msgs), 0);
    assert_eq!(count_d_releases(&duplicate_disconnect_msgs), 0);
    assert_eq!(count_d_tx_granted(&duplicate_disconnect_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&duplicate_disconnect_msgs), 0);

    test.submit_message(build_u_tx_demand_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&demand_msgs), 0);
    assert_eq!(count_d_releases(&demand_msgs), 0);
    assert_eq!(count_d_tx_granted(&demand_msgs), 0);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&demand_msgs), 0);

    for reporter in requester_release_reporters.iter().chain(peer_release_reporters.iter()) {
        reporter.mark_transmitted();
    }
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "Reporter completion should still close the pending P2P release"
    );
}

#[test]
fn test_p2p_pending_release_large_duplicate_disconnect_ptt_flood_is_ignored_and_closes() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_duplex_p2p_call(&mut test);

    let (requester_release_reporters, peer_release_reporters) =
        start_called_party_disconnect_with_peer_d_release(&mut test, dltime, call_id);

    for offset in 0..LARGE_GSSI_MEMBER_COUNT {
        if offset % 2 == 0 {
            test.submit_message(build_u_disconnect_msg(TEST_CALLED_ISSI, call_id));
        } else {
            test.submit_message(build_u_tx_demand_msg(TEST_ISSI, call_id));
        }
    }
    test.deliver_all_messages();
    let flood_msgs = test.dump_sinks();

    // While the D-RELEASE clear is pending, stale duplicate disconnects and
    // PTT floor requests must not create new maintenance signalling or close
    // early.
    assert_eq!(count_d_disconnects(&flood_msgs), 0);
    assert_eq!(count_d_releases(&flood_msgs), 0);
    assert_eq!(count_d_tx_granted(&flood_msgs), 0);
    assert_eq!(count_umac_floor_granted(&flood_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&flood_msgs), 0);
    assert!(
        cmce_debug_active_call_ids(&mut test).contains(&call_id),
        "large stale P2P release flood must not evict the pending call id"
    );

    for reporter in requester_release_reporters.iter().chain(peer_release_reporters.iter()) {
        reporter.mark_transmitted();
    }
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert_eq!(
        count_d_releases(&closed_msgs),
        0,
        "reporter completion must not duplicate either D-RELEASE"
    );
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "large stale P2P release flood must not prevent reporter-driven close"
    );
    assert!(
        !cmce_debug_active_call_ids(&mut test).contains(&call_id),
        "pending P2P call id should be freed after D-RELEASE reporter completion"
    );
}

#[test]
fn test_p2p_pending_release_suppresses_d_setup_resend() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_duplex_p2p_call(&mut test);

    let (_requester_release_reporters, _peer_release_reporters) =
        start_called_party_disconnect_with_peer_d_release(&mut test, dltime, call_id);

    // The call is already in release clearance by assigned-channel D-RELEASEs.
    // A cached D-SETUP resend would contradict that disconnection phase and can
    // re-open UI state on real terminals.
    test.run_stack(Some(8));
    let backup_window_msgs = test.dump_sinks();
    assert_eq!(
        count_d_setups(&backup_window_msgs),
        0,
        "D-RELEASE-pending individual call_id={call_id} must not emit backup D-SETUP"
    );
    assert_eq!(count_umac_call_ended_or_close(&backup_window_msgs), 0);
}

#[test]
fn test_p2p_pending_peer_release_suppresses_floor_pdus() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_duplex_p2p_call(&mut test);

    let (_requester_release_reporters, peer_release_reporters) =
        start_called_party_disconnect_with_peer_d_release(&mut test, dltime, call_id);
    assert_eq!(peer_release_reporters[0].get_state(), TxState::Pending);

    // EN 300 392-2 clause 14.5.1.3.1 moves the call into release clearance.
    // Until D-RELEASE delivery is resolved, floor control must not race the
    // shutdown with D-TX GRANTED/D-TX CEASED.
    test.submit_message(build_u_tx_demand_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_granted(&demand_msgs), 0);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
    assert_eq!(count_d_disconnects(&demand_msgs), 0);
    assert_eq!(count_d_releases(&demand_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&demand_msgs), 0);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let ceased_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&ceased_msgs), 0);
    assert_eq!(count_umac_floor_released(&ceased_msgs), 0);
    assert_eq!(count_d_disconnects(&ceased_msgs), 0);
    assert_eq!(count_d_releases(&ceased_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&ceased_msgs), 0);
}

#[test]
fn test_duplex_p2p_discarded_peer_d_release_waits_for_guard_before_close() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_duplex_p2p_call(&mut test);

    let (requester_release_reporters, peer_release_reporters) =
        start_called_party_disconnect_with_peer_d_release(&mut test, dltime, call_id);

    // A discarded local TxReporter is not evidence that the peer received the
    // D-RELEASE. Keep the assigned circuit open until the bounded release guard
    // expires; do not fall back to D-DISCONNECT.
    peer_release_reporters[0].mark_discarded();
    requester_release_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let discarded_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&discarded_msgs), 0);
    assert_eq!(count_d_releases(&discarded_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&discarded_msgs), 0);

    test.router
        .set_dl_time(dltime.add_timeslots(PRIVATE_RELEASE_DELIVERY_GUARD_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&closed_msgs), 0);
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "release guard timeout should close the duplex P2P circuit without peer D-DISCONNECT"
    );
}

#[test]
fn test_active_p2p_mm_deregister_waits_for_release_reporters_before_circuit_close() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    // EN 300 392-2 clause 14.5.1.3.3 uses D-RELEASE without a peer
    // response after the SwMI releases an established individual call. MM
    // deregistration must still keep the assigned channel alive until the
    // FACCH/STCH D-RELEASE reports final transmission or times out.
    test.submit_message(build_mm_deregister_msg(TEST_ISSI));
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();

    assert_established_p2p_release_pdus(&release_msgs, call_id, DisconnectCause::UserRequestedDisconnection);
    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 2, "Only assigned-channel D-RELEASEs should carry TxReporters");
    for reporter in &reporters {
        assert_eq!(reporter.get_state(), TxState::Pending);
    }
    assert_eq!(
        count_umac_call_ended_or_close(&release_msgs),
        0,
        "P2P circuit must stay open until deregister-triggered D-RELEASE transmission is reported"
    );

    test.run_stack(Some(3));
    let pending_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_call_ended_or_close(&pending_msgs),
        0,
        "Pending deregister release should not close before reporter completion"
    );

    for reporter in &reporters {
        reporter.mark_transmitted();
    }
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "Reporter completion should close the deregistered P2P traffic circuit"
    );
}

#[test]
fn test_active_p2p_discarded_release_reporters_do_not_close_before_guard_timeout() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_mm_deregister_msg(TEST_ISSI));
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();

    assert_established_p2p_release_pdus(&release_msgs, call_id, DisconnectCause::UserRequestedDisconnection);
    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 2, "Assigned-channel private D-RELEASEs should carry TxReporters");

    // EN 300 392-2 clause 14.5.1.3.2 requires D-RELEASE before releasing an
    // established individual call. Local UMAC discard is not transmission, so
    // both private-call legs must remain open until guard timeout.
    for reporter in &reporters {
        reporter.mark_discarded();
    }
    test.run_stack(Some(1));
    let discarded_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_call_ended_or_close(&discarded_msgs),
        0,
        "Discarded private D-RELEASE reporters must not close the traffic circuit immediately"
    );

    test.run_stack(Some(20));
    let closed_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_call_ended_or_close(&closed_msgs),
        0,
        "Discarded private D-RELEASE reporters must not close before the two-second delivery guard"
    );

    test.router
        .set_dl_time(dltime.add_timeslots(PRIVATE_RELEASE_DELIVERY_GUARD_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "Local guard timeout should eventually close a discarded private release"
    );
}

#[test]
fn test_stale_p2p_circuit_expiry_waits_for_release_reporters_before_umac_close() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.call_timeout_secs = 0;
    let mut test = ComponentTest::from_config(config, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    // EN 300 392-2 clause 14.5.1.3.2 releases an established individual
    // call with D-RELEASE before the traffic circuit is released. This forces
    // only the CircuitMgr stale-circuit safety timeout; the configured CMCE
    // call timeout is disabled above so the normal release path cannot mask
    // this expiry path.
    test.router.set_dl_time(dltime.add_timeslots(6 * 60 * 18 * 4 + 80));
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();

    assert_established_p2p_release_pdus(&release_msgs, call_id, DisconnectCause::ExpiryOfTimer);
    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 2, "Only assigned-channel D-RELEASEs should carry TxReporters");
    for reporter in &reporters {
        assert_eq!(reporter.get_state(), TxState::Pending);
    }
    assert_eq!(
        count_umac_call_ended_or_close(&release_msgs),
        0,
        "CircuitMgr expiry must not close the P2P traffic circuit before D-RELEASE delivery is reported"
    );

    test.run_stack(Some(3));
    let pending_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_call_ended_or_close(&pending_msgs),
        0,
        "Pending stale-circuit individual release should not close before reporter completion"
    );

    for reporter in &reporters {
        reporter.mark_transmitted();
    }
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "Reporter completion should close the stale P2P traffic circuit"
    );
}

#[test]
fn test_duplex_p2p_pending_release_closes_after_bounded_timeout() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_duplex_p2p_call(&mut test);

    let (_requester_release_reporters, _peer_release_reporters) =
        start_called_party_disconnect_with_peer_d_release(&mut test, dltime, call_id);

    test.run_stack(Some(80));
    let early_wait_msgs = test.dump_sinks();
    assert_eq!(
        count_d_releases(&early_wait_msgs),
        0,
        "Pending D-RELEASE guard must not duplicate release PDUs before the delivery window expires"
    );
    assert_eq!(count_d_disconnects(&early_wait_msgs), 0);
    assert_eq!(
        count_umac_call_ended_or_close(&early_wait_msgs),
        0,
        "Pending D-RELEASE guard must keep the assigned circuit open"
    );

    test.router
        .set_dl_time(dltime.add_timeslots(PRIVATE_RELEASE_DELIVERY_GUARD_TIMESLOTS + PRIVATE_TEST_TIME_JUMP_MARGIN_TIMESLOTS));
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert_eq!(
        count_d_releases(&closed_msgs),
        0,
        "D-RELEASE delivery timeout must not emit another peer clear PDU"
    );
    assert_eq!(
        count_d_disconnects(&closed_msgs),
        0,
        "D-RELEASE delivery timeout must not fall back to D-DISCONNECT"
    );
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "D-RELEASE delivery timeout should close the stuck P2P circuit locally"
    );
}
