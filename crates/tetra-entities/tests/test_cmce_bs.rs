mod common;

use tetra_config::bluestation::{CfgBrew, StackMode, from_toml_str};
use tetra_core::ranges::SortedDisjointSsiRanges;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::typed_pdu_fields::Type3FieldGeneric;
use tetra_core::{BitBuffer, Layer2Service, Sap, SsiType, TdmaTime, TetraAddress, TimeslotOwner, TxState, debug};
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
use tetra_pdus::mm::enums::location_update_type::LocationUpdateType;
use tetra_pdus::mm::fields::group_identity_location_demand::GroupIdentityLocationDemand;
use tetra_pdus::mm::fields::group_identity_uplink::GroupIdentityUplink;
use tetra_pdus::mm::pdus::d_location_update_accept::DLocationUpdateAccept;
use tetra_pdus::mm::pdus::u_location_update_demand::ULocationUpdateDemand;
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

use crate::common::ComponentTest;

const TEST_GSSI: u32 = 91;
const TEST_ISSI: u32 = 1000001;
const TEST_CALLED_GSSI: u32 = 92;
const TEST_CALLED_ISSI: u32 = 1000002;
const TEST_OTHER_ISSI: u32 = 1000003;

fn type3_marker() -> Type3FieldGeneric {
    Type3FieldGeneric {
        field_id: 0,
        len: 8,
        data: 0xA5,
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

/// Helper: submit a real MM U-LOCATION UPDATE DEMAND carrying group affiliation.
fn submit_location_update_with_group_identity_location_demand(test: &mut ComponentTest, issi: u32, gssi: u32) {
    submit_location_update_with_type_and_group_identity_location_demand(test, issi, gssi, LocationUpdateType::ItsiAttach);
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
    build_u_disconnect_pdu_msg(
        calling_issi,
        UDisconnect {
            call_identifier: call_id,
            disconnect_cause: DisconnectCause::UserRequestedDisconnection,
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

fn assert_compact_d_tx_granted_facch(prim: &LcmcMleUnitdataReq, grant: &DTxGranted) {
    assert_eq!(grant.transmitting_party_type_identifier, None);
    assert_eq!(grant.transmitting_party_address_ssi, None);
    assert_eq!(
        prim.sdu.get_len(),
        25,
        "D-TX GRANTED must omit optional transmitting-party IEs so it fits assigned-channel FACCH/STCH"
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

fn extract_d_disconnect_reporters(msgs: &mut Vec<SapMsg>) -> Vec<tetra_core::TxReporter> {
    let mut reporters = vec![];
    for msg in msgs.iter_mut() {
        if msg.dest == TetraEntity::Mle {
            if let SapMsgInner::LcmcMleUnitdataReq(ref mut prim) = msg.msg {
                if is_dl_pdu(prim, CmcePduTypeDl::DDisconnect) {
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
    let releases: Vec<_> = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();

    assert_eq!(
        releases.len(),
        4,
        "Established P2P release should send FACCH/STCH D-RELEASE plus MCCH fallback to both MSs"
    );

    let mut facch_ssis = Vec::new();
    let mut mcch_ssis = Vec::new();
    for (prim, d_release) in releases {
        assert_eq!(d_release.call_identifier, call_id);
        assert_eq!(d_release.disconnect_cause, disconnect_cause);
        assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
        assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);

        if prim.stealing_permission {
            facch_ssis.push(prim.main_address.ssi);
            assert!(prim.tx_reporter.is_some(), "FACCH/STCH D-RELEASE must be reporter-tracked");
            let chan_alloc = prim
                .chan_alloc
                .as_ref()
                .expect("FACCH/STCH D-RELEASE should preserve assigned-channel allocation");
            assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Dl);
            assert!(chan_alloc.usage.is_some());
            assert!(chan_alloc.timeslots.iter().any(|enabled| *enabled));
        } else {
            mcch_ssis.push(prim.main_address.ssi);
            assert!(prim.tx_reporter.is_none(), "MCCH fallback should not carry a reporter");
            assert!(prim.chan_alloc.is_none(), "MCCH fallback should not carry channel allocation");
        }
    }

    facch_ssis.sort_unstable();
    mcch_ssis.sort_unstable();
    assert_eq!(facch_ssis, vec![TEST_ISSI, TEST_CALLED_ISSI]);
    assert_eq!(mcch_ssis, vec![TEST_ISSI, TEST_CALLED_ISSI]);
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
    let (call_id, ts) = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid: ready_uuid,
                call_id,
                ts,
                ..
            }) if *ready_uuid == brew_uuid => Some((*call_id, *ts)),
            _ => None,
        })
        .expect("network-origin group setup should report ready to Brew");
    (call_id, ts, setup_msgs)
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

fn start_group_call_with_u_setup(test: &mut ComponentTest, u_setup: USetup) -> u16 {
    test.submit_message(build_u_setup_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();
    let initial_setups = count_d_setups(&initial_msgs);
    assert!(initial_setups > 0, "Expected initial D-SETUP after U-SETUP");
    first_d_setup_call_id(&initial_msgs)
}

fn start_p2p_setup(test: &mut ComponentTest) -> (u16, Vec<SapMsg>) {
    test.submit_message(build_u_setup_p2p_msg(TEST_ISSI, TEST_CALLED_ISSI));
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
    let (call_id, _setup_msgs) = start_p2p_setup(test);
    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();
    assert!(count_umac_open(&connect_msgs) >= 1, "U-CONNECT should open the P2P traffic circuit");
    (call_id, connect_msgs)
}

fn p2p_open_ts_for(msgs: &[SapMsg], issi: u32) -> u8 {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit))
                if circuit.active_addr == Some(TetraAddress::new(issi, SsiType::Issi)) =>
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
    test.submit_message(build_u_connect_custom_msg(TEST_CALLED_ISSI, call_id, true));
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();
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
        setup_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid: ready_uuid,
                ..
            }) if *ready_uuid == brew_uuid
        )),
        "network-origin group setup should notify Brew when the call is ready"
    );
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
    let (call_id, ts) = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid: ready_uuid,
                call_id,
                ts,
                ..
            }) if *ready_uuid == brew_uuid => Some((*call_id, *ts)),
            _ => None,
        })
        .expect("network-origin group setup should report ready to Brew");

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
    assert_eq!(count_umac_floor_granted(&demand_msgs), 1);
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
    let call_id = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid: ready_uuid,
                call_id,
                ..
            }) if *ready_uuid == brew_uuid => Some(*call_id),
            _ => None,
        })
        .expect("network-origin group setup should report ready to Brew");

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
    let (call_id, _, setup_msgs) = start_network_group_call(&mut test, brew_uuid, TEST_CALLED_ISSI, TEST_GSSI, 7);
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
    assert_eq!(count_umac_floor_granted(&demand_msgs), 1);
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
    let call_id = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid: ready_uuid,
                call_id,
                ..
            }) if *ready_uuid == brew_uuid => Some(*call_id),
            _ => None,
        })
        .expect("network-origin group setup should report ready to Brew");
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
    let call_id = start_group_call(&mut test);

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

    assert_eq!(count_umac_floor_granted(&msgs), 1);
    assert!(msgs.iter().any(|msg| {
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
    assert!(msgs.iter().any(|msg| {
        matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid: ready_uuid,
                call_id: ready_call_id,
                ..
            }) if *ready_uuid == brew_uuid && *ready_call_id == call_id
        )
    }));
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
    assert_eq!(connect_ack.1.call_identifier, call_id);
    assert_eq!(connect_ack.1.call_time_out, CallTimeout::T10m);
    assert_eq!(connect_ack.1.transmission_grant, TransmissionGrant::Granted);
    assert!(count_umac_open(&confirm_msgs) >= 1);
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
        2,
        "network-origin private release should send FACCH plus MCCH D-RELEASE to the local MS"
    );
    for (prim, release) in releases {
        assert_eq!(prim.main_address.ssi, TEST_CALLED_ISSI);
        assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
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
    assert_eq!(connect.1.call_time_out, CallTimeout::T10m);
    assert!(connect.1.hook_method_selection);
    assert!(!connect.1.simplex_duplex_selection);
    assert!(count_umac_open(&connect_msgs) >= 1);
    assert!(connect_msgs.iter().any(|msg| matches!(
        &msg.msg,
        SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
            brew_uuid: confirm_uuid,
            ..
        }) if *confirm_uuid == brew_uuid
    )));
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
    let active_call_id = start_group_call(&mut test);

    // EN 300 392-2 clause 14.5.2.1.3 same-group setup collisions are tied
    // back to the SwMI call identifier, while clause 14.5.2.2.1 keeps later
    // PTT attempts as floor control. A compatible U-SETUP for an already
    // active GSSI must not be reported to the MS as an unavailable service.
    test.submit_message(build_u_setup_msg(TEST_CALLED_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let duplicate_msgs = test.dump_sinks();

    assert_eq!(
        count_d_releases(&duplicate_msgs),
        0,
        "same-GSSI active-call rejoin must not emit D-RELEASE RequestedServiceNotAvailable"
    );

    let proceedings: Vec<_> = duplicate_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_call_proceeding(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        proceedings.len(),
        1,
        "repeated U-SETUP should receive D-CALL PROCEEDING for the existing call"
    );
    let (proceeding_prim, proceeding) = &proceedings[0];
    assert_eq!(
        proceeding.call_identifier, active_call_id,
        "repeated setup must be bound to the active SwMI call id"
    );
    assert_eq!(proceeding_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(proceeding_prim.main_address.ssi_type, SsiType::Issi);

    let connects: Vec<_> = duplicate_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(connects.len(), 1, "repeated U-SETUP should receive D-CONNECT for the existing call");
    let (connect_prim, connect) = &connects[0];
    assert_eq!(connect.call_identifier, active_call_id);
    assert_eq!(
        connect.transmission_grant,
        TransmissionGrant::GrantedToOtherUser,
        "a non-speaker rejoining while another MS transmits should enter receive state"
    );
    assert!(!connect.call_ownership);
    assert_eq!(connect_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(connect_prim.main_address.ssi_type, SsiType::Issi);
    assert!(
        connect_prim.chan_alloc.is_some(),
        "D-CONNECT for active-call rejoin must carry the existing traffic allocation"
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
    assert_compact_d_tx_granted_facch(grant_prim, grant);

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
fn test_repeated_group_u_setup_from_current_speaker_reasserts_existing_floor() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let active_call_id = start_group_call(&mut test);

    // Some terminals repeat U-SETUP when the user presses PTT again even
    // though the SwMI still has that MS as current speaker. Treating that as
    // a duplicate with only D-CONNECT is too weak in the field: clause
    // 14.5.2.2.1 floor control needs an explicit D-TX GRANTED response for
    // transmit permission, and UMAC must keep the current speaker mapped.
    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let repeated_msgs = test.dump_sinks();

    assert_eq!(count_d_releases(&repeated_msgs), 0);
    assert_eq!(count_d_setups(&repeated_msgs), 0);
    assert_eq!(count_umac_open(&repeated_msgs), 0);

    let connect = repeated_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .expect("current-speaker repeated U-SETUP should receive D-CONNECT for the existing call");
    assert_eq!(connect.1.call_identifier, active_call_id);
    assert_eq!(connect.1.transmission_grant, TransmissionGrant::Granted);
    assert!(connect.1.call_ownership);
    assert_eq!(connect.0.main_address.ssi, TEST_ISSI);
    assert!(connect.0.chan_alloc.is_some());

    let grants: Vec<_> = repeated_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        grants.len(),
        2,
        "current-speaker repeated setup must explicitly reassert floor to the MS and group"
    );
    assert!(grants.iter().any(|(prim, grant)| {
        prim.main_address.ssi == TEST_ISSI
            && prim.main_address.ssi_type == SsiType::Issi
            && grant.call_identifier == active_call_id
            && grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8
    }));
    assert!(grants.iter().any(|(prim, grant)| {
        prim.main_address.ssi == TEST_GSSI
            && prim.main_address.ssi_type == SsiType::Gssi
            && grant.call_identifier == active_call_id
            && grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    }));
    for (prim, grant) in &grants {
        assert_compact_d_tx_granted_facch(prim, grant);
    }

    assert_eq!(
        count_umac_floor_granted(&repeated_msgs),
        1,
        "current-speaker repeated setup must refresh the UMAC speaker mapping"
    );
    assert_eq!(count_umac_call_ended_or_close(&repeated_msgs), 0);
}

#[test]
fn test_repeated_group_u_setup_same_gssi_during_hangtime_grants_existing_call_floor() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_GSSI);
    let active_call_id = start_group_call(&mut test);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, active_call_id));
    test.run_stack(Some(1));
    let _hangtime_msgs = test.dump_sinks();

    // Nexus-BS hangtime is local call-retention between transmissions. While
    // the call is still maintained, EN 300 392-2 clause 14.5.2.2.1 floor
    // control applies on the existing call id instead of starting or rejecting
    // a parallel same-GSSI call.
    test.submit_message(build_u_setup_msg(TEST_CALLED_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let repeated_msgs = test.dump_sinks();

    assert_eq!(
        count_d_releases(&repeated_msgs),
        0,
        "same-GSSI hangtime rejoin must not emit D-RELEASE RequestedServiceNotAvailable"
    );

    let connect = repeated_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .expect("hangtime repeated U-SETUP should receive D-CONNECT for the existing call");
    assert_eq!(connect.1.call_identifier, active_call_id);
    assert_eq!(connect.1.transmission_grant, TransmissionGrant::Granted);
    assert_eq!(connect.0.main_address.ssi, TEST_CALLED_ISSI);
    assert!(connect.0.chan_alloc.is_some());

    let grants: Vec<_> = repeated_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_granted(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(
        grants.len(),
        2,
        "hangtime retake should grant the requester and inform group listeners"
    );
    assert!(grants.iter().any(|(prim, grant)| {
        prim.main_address.ssi == TEST_CALLED_ISSI
            && prim.main_address.ssi_type == SsiType::Issi
            && grant.call_identifier == active_call_id
            && grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8
    }));
    assert!(grants.iter().any(|(prim, grant)| {
        prim.main_address.ssi == TEST_GSSI
            && prim.main_address.ssi_type == SsiType::Gssi
            && grant.call_identifier == active_call_id
            && grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    }));
    for (prim, grant) in &grants {
        assert_compact_d_tx_granted_facch(prim, grant);
    }

    assert_eq!(count_d_setups(&repeated_msgs), 0, "hangtime retake must not send a second D-SETUP");
    assert_eq!(count_umac_open(&repeated_msgs), 0, "hangtime retake must not open a second circuit");
    assert_eq!(
        count_umac_call_ended_or_close(&repeated_msgs),
        0,
        "hangtime retake must not close the maintained group call"
    );
    assert_eq!(
        count_umac_floor_granted(&repeated_msgs),
        1,
        "hangtime retake must hand the existing traffic floor to the requester"
    );
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
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);

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
            "pre-emptive handoff should grant requester and inform group for priority {tx_demand_priority}"
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
            prim.main_address.ssi == TEST_GSSI
                && prim.main_address.ssi_type == SsiType::Gssi
                && grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8
        }));
        for (_, prim, grant) in &grants {
            assert_compact_d_tx_granted_facch(prim, grant);
        }

        assert_eq!(count_umac_floor_granted(&demand_msgs), 1, "priority {tx_demand_priority}");
        assert!(demand_msgs.iter().any(|msg| {
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
    let call_id = start_group_call(&mut test);

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
        "queued requester handoff should send D-TX-GRANTED to requester and group listeners"
    );

    let requester_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == TEST_CALLED_ISSI && prim.main_address.ssi_type == SsiType::Issi)
        .expect("expected individual D-TX-GRANTED to queued requester");
    assert_eq!(requester_grant.1.call_identifier, call_id);
    assert_eq!(requester_grant.1.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_compact_d_tx_granted_facch(requester_grant.0, &requester_grant.1);

    let group_grant = grants
        .iter()
        .find(|(prim, _)| prim.main_address.ssi == TEST_GSSI && prim.main_address.ssi_type == SsiType::Gssi)
        .expect("expected group FACCH D-TX-GRANTED");
    assert_eq!(group_grant.1.call_identifier, call_id);
    assert_eq!(
        group_grant.1.transmission_grant,
        TransmissionGrant::GrantedToOtherUser.into_raw() as u8
    );
    assert_compact_d_tx_granted_facch(group_grant.0, &group_grant.1);
    let group_alloc = group_grant
        .0
        .chan_alloc
        .as_ref()
        .expect("FACCH group grant should carry channel allocation");
    assert_eq!(group_alloc.ul_dl_assigned, UlDlAssignment::Dl);

    assert!(
        ceased_msgs
            .iter()
            .all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_tx_ceased(prim).is_some())),
        "queued handoff should grant the next speaker instead of entering no-speaker hangtime"
    );
    assert_eq!(count_umac_floor_released(&ceased_msgs), 0);
    assert_eq!(count_umac_floor_granted(&ceased_msgs), 1);
    assert!(ceased_msgs.iter().any(|msg| {
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
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
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
    assert_eq!(ceased_alloc.ul_dl_assigned, UlDlAssignment::Dl);

    assert_eq!(count_umac_floor_granted(&ceased_msgs), 0);
    assert_eq!(count_umac_floor_released(&ceased_msgs), 1);
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
fn test_group_release_pending_rejects_same_gssi_restart_without_second_circuit() {
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

    // EN 300 392-2 clause 14.5.2.3.2 says the SwMI sends D-RELEASE and
    // subsequently releases the call. While the local FACCH D-RELEASE is still
    // draining, keep the call occupied so a same-GSSI restart cannot allocate
    // a second traffic circuit over the pending release.
    test.submit_message(build_u_setup_msg(TEST_CALLED_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let restart_msgs = test.dump_sinks();

    let releases: Vec<_> = restart_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_release(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(releases.len(), 1, "same-GSSI restart should receive one direct D-RELEASE");
    let (release_prim, release) = &releases[0];
    assert_eq!(
        release.call_identifier, 0,
        "restart rejection before a new SwMI call identity exists must use the dummy call identity"
    );
    assert_ne!(
        release.call_identifier, call_id,
        "restart rejection must not release the still-pending group call"
    );
    assert_eq!(release.disconnect_cause, DisconnectCause::RequestedServiceNotAvailable);
    assert_eq!(release_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(release_prim.main_address.ssi_type, SsiType::Issi);
    assert!(release_prim.chan_alloc.is_none());

    assert_eq!(count_d_setups(&restart_msgs), 0, "restart must not send a new group D-SETUP");
    assert_eq!(count_umac_open(&restart_msgs), 0, "restart must not open a second circuit");
    assert_eq!(
        count_umac_call_ended_or_close(&restart_msgs),
        0,
        "restart rejection must not close the pending release early"
    );
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
fn test_group_release_pending_rejects_network_speaker_change_without_floor_signalling() {
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
    let call_id = start_group_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
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
    let network_msgs = test.dump_sinks();

    assert_eq!(count_network_call_end(&network_msgs, brew_uuid), 1);
    assert_eq!(count_d_tx_interrupt(&network_msgs), 0);
    assert_eq!(count_d_tx_granted(&network_msgs), 0);
    assert_eq!(count_d_tx_ceased(&network_msgs), 0);
    assert_eq!(count_umac_floor_granted(&network_msgs), 0);
    assert_eq!(count_umac_floor_released(&network_msgs), 0);
    assert!(
        network_msgs.iter().all(|msg| {
            !matches!(
                &msg.msg,
                SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                    brew_uuid: ready_uuid,
                    ..
                }) if *ready_uuid == brew_uuid
            )
        }),
        "pending group release must not report network floor readiness"
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
    let ceased_msgs = test.dump_sinks();
    assert_eq!(count_d_tx_ceased(&ceased_msgs), 1);
    assert_eq!(count_umac_floor_released(&ceased_msgs), 1);

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert!(count_d_tx_granted(&demand_msgs) >= 1);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 1);

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
fn test_p2p_preemptive_u_setup_default_off_rejects_without_call_setup() {
    debug::setup_logging_verbose();

    // EN 300 392-2 table 14.46 defines call priorities 12..=15 as
    // pre-emptive. Clause 14.5.1.2.1 f) is conditional on SwMI interruption
    // support, so default-off handling rejects before allocating an
    // individual call identity.
    for priority in 12..=15 {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
        test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac]);
        register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
        register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);

        let mut u_setup = default_p2p_u_setup();
        u_setup.call_priority = priority;
        u_setup.called_party_ssi = Some(TEST_CALLED_ISSI as u64);

        test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
        test.run_stack(Some(1));
        let msgs = test.dump_sinks();

        assert_p2p_setup_rejected_with_dummy_call_id(&msgs, TEST_ISSI);
    }
}

#[test]
fn test_p2p_preemptive_u_setup_group_interruption_enabled_still_rejects_without_call_setup() {
    debug::setup_logging_verbose();

    // EN 300 392-2 clause 14.5.1.2.1 f) is specific to private-call
    // interruption support. Group transmission interruption being configured
    // does not make P2P pre-emption supported.
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

        test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
        test.run_stack(Some(1));
        let msgs = test.dump_sinks();

        assert_p2p_setup_rejected_with_dummy_call_id(&msgs, TEST_ISSI);
        assert_eq!(count_d_setups(&msgs), 0, "priority {priority}");
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
    let mut disconnect_msgs = test.dump_sinks();
    let disconnect_reporters = extract_d_disconnect_reporters(&mut disconnect_msgs);
    assert_eq!(disconnect_reporters.len(), 1);

    disconnect_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert_eq!(count_umac_call_ended_or_close(&test.dump_sinks()), 0);

    test.submit_message(build_u_release_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus(&release_msgs, call_id, DisconnectCause::UserRequestedDisconnection);
    let release_reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(release_reporters.len(), 2);
    assert_eq!(
        count_umac_call_ended_or_close(&release_msgs),
        0,
        "P2P circuit must stay open while final D-RELEASE reporters are pending"
    );

    // EN 300 392-2 clauses 14.5.1.1.2 and 14.5.1.3.2/14.5.1.3.3: while
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

    for reporter in &release_reporters {
        reporter.mark_transmitted();
    }
    test.run_stack(Some(1));
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

    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 14.5.1.1.1/14.5.1.1.2: U-CONNECT completes the
    // called leg, after which the SwMI sends D-CONNECT/D-CONNECT ACKNOWLEDGE
    // and opens the assigned channel.
    assert_eq!(
        count_umac_open(&connect_msgs),
        1,
        "U-CONNECT should open one shared simplex assigned-channel circuit"
    );
    assert!(
        connect_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some())),
        "U-CONNECT should produce D-CONNECT to the caller"
    );
    assert!(
        connect_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect_acknowledge(prim).is_some())),
        "U-CONNECT should produce D-CONNECT ACKNOWLEDGE to the called MS"
    );
    let open_ts = p2p_open_ts_for(&connect_msgs, TEST_ISSI);
    assert!(
        (1..=4).contains(&open_ts),
        "assigned-channel open should use a valid TETRA timeslot"
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
fn test_p2p_pending_setup_retry_is_reporter_throttled_before_timeout() {
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

    // The CircuitMgr path emits an early backup D-SETUP; later EE retries use
    // the same reporter throttle so an untransmitted retry does not turn into
    // repeated setup spam before T302 expires.
    test.run_stack(Some(70));
    let mut retry_msgs = test.dump_sinks();
    let setups: Vec<_> = retry_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_setup(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(setups.len(), 1, "pending P2P setup should retry D-SETUP before setup timeout");
    let (setup_prim, setup) = &setups[0];
    assert_eq!(setup.call_identifier, call_id);
    assert_eq!(setup_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert!(setup_prim.chan_alloc.is_none(), "setup-phase retry remains on MCCH");
    assert!(
        setup_prim.tx_reporter.is_some(),
        "retry should be reporter-tracked so later resends are throttled until MAC reports completion"
    );
    assert_eq!(
        count_umac_open(&retry_msgs),
        0,
        "D-SETUP retry must not open traffic before U-CONNECT"
    );

    let reporters = extract_d_setup_reporters(&mut retry_msgs);
    assert_eq!(reporters.len(), 1, "retry should expose exactly one D-SETUP TxReporter");
    assert_eq!(reporters[0].get_state(), TxState::Pending);

    test.run_stack(Some(720));
    let pending_retry_msgs = test.dump_sinks();
    assert_eq!(
        count_d_setups(&pending_retry_msgs),
        0,
        "pending P2P setup must not send another D-SETUP while the previous retry reporter is still pending"
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
        .find(|circuit| circuit.active_addr == Some(TetraAddress::new(TEST_ISSI, SsiType::Issi)))
        .expect("simplex P2P should open a caller-owned UMAC traffic circuit");
    // EN 300 392-2 clause 14.5.1.2.1: simple private setup keeps one
    // simplex traffic channel; peer_ts is reserved for duplex cross-routing.
    assert_eq!(simplex_open.peer_ts, None);
    assert_eq!(simplex_open.dl_media_source, CircuitDlMediaSource::LocalLoopback);
    assert!(
        simplex_open
            .active_secondary_addrs
            .contains(&TetraAddress::new(TEST_CALLED_ISSI, SsiType::Issi)),
        "simplex P2P shared assigned channel must identify both ISSIs so UMAC suspends EG for both active MSs"
    );

    let d_connects: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(d_connects.len(), 2, "Expected FACCH D-CONNECT plus MCCH fallback");
    assert!(
        d_connects
            .iter()
            .any(|(prim, _)| prim.stealing_permission && prim.chan_alloc.is_some()),
        "One D-CONNECT should be sent with FACCH stealing and channel allocation"
    );
    assert!(
        d_connects
            .iter()
            .any(|(prim, _)| !prim.stealing_permission && prim.chan_alloc.is_some()),
        "One D-CONNECT fallback should be sent on MCCH with the same allocation"
    );
    for (prim, pdu) in &d_connects {
        assert_eq!(pdu.call_identifier, call_id);
        assert_eq!(pdu.simplex_duplex_selection, false);
        assert_eq!(pdu.transmission_grant, TransmissionGrant::Granted);
        assert!(!pdu.transmission_request_permission);
        assert!(pdu.call_ownership);
        assert_eq!(prim.main_address.ssi, TEST_ISSI);
        assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
        assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
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
    assert_eq!(d_connect_acks.len(), 2, "Expected FACCH D-CONNECT-ACKNOWLEDGE plus MCCH fallback");
    assert!(
        d_connect_acks
            .iter()
            .any(|(prim, _)| prim.stealing_permission && prim.chan_alloc.is_some()),
        "One D-CONNECT-ACKNOWLEDGE should be sent with FACCH stealing and channel allocation"
    );
    assert!(
        d_connect_acks
            .iter()
            .any(|(prim, _)| !prim.stealing_permission && prim.chan_alloc.is_some()),
        "One D-CONNECT-ACKNOWLEDGE fallback should be sent on MCCH with the same allocation"
    );
    for (prim, pdu) in &d_connect_acks {
        assert_eq!(pdu.call_identifier, call_id);
        assert_eq!(pdu.transmission_grant, TransmissionGrant::GrantedToOtherUser);
        assert!(!pdu.transmission_request_permission);
        assert_eq!(prim.main_address.ssi, TEST_CALLED_ISSI);
        assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
        assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
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
    let connect_msgs = test.dump_sinks();

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
        2,
        "simple private setup should send FACCH and MCCH D-CONNECT to the caller"
    );
    assert_eq!(
        connect_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect_acknowledge(prim).is_some()))
            .count(),
        2,
        "simple private setup should send FACCH and MCCH D-CONNECT-ACKNOWLEDGE to the called MS"
    );
    assert_eq!(
        count_d_tx_interrupt(&connect_msgs),
        0,
        "default simple private setup must not use pre-emptive interruption"
    );

    // EN 300 392-2 clauses 14.5.1.3.1 and 14.5.1.3.3: after one party sends
    // U-DISCONNECT, the SwMI requests peer clearance with D-DISCONNECT and
    // waits for U-RELEASE before sending final D-RELEASE and closing UMAC.
    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut disconnect_msgs = test.dump_sinks();

    let d_disconnects: Vec<_> = disconnect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_disconnect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(d_disconnects.len(), 1, "U-DISCONNECT should send one D-DISCONNECT to the peer");
    let (disconnect_prim, d_disconnect) = &d_disconnects[0];
    assert_eq!(d_disconnect.call_identifier, call_id);
    assert_eq!(d_disconnect.disconnect_cause, DisconnectCause::UserRequestedDisconnection);
    assert_eq!(disconnect_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(disconnect_prim.main_address.ssi_type, SsiType::Issi);
    assert!(disconnect_prim.stealing_permission);
    assert!(disconnect_prim.chan_alloc.is_some());
    assert_eq!(count_d_releases(&disconnect_msgs), 0, "D-RELEASE must wait for peer U-RELEASE");
    assert_eq!(
        count_umac_call_ended_or_close(&disconnect_msgs),
        0,
        "traffic circuit must remain open while peer release is pending"
    );
    let disconnect_reporters = extract_d_disconnect_reporters(&mut disconnect_msgs);
    assert_eq!(disconnect_reporters.len(), 1);
    disconnect_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    assert_eq!(
        count_umac_call_ended_or_close(&test.dump_sinks()),
        0,
        "D-DISCONNECT transmission alone must not close the private circuit"
    );

    test.submit_message(build_u_release_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus(&release_msgs, call_id, DisconnectCause::UserRequestedDisconnection);
    let release_reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(release_reporters.len(), 2, "assigned-channel D-RELEASEs should be reporter-tracked");
    assert_eq!(
        count_umac_call_ended_or_close(&release_msgs),
        0,
        "final release must not close UMAC until D-RELEASE delivery is known"
    );

    for reporter in &release_reporters {
        reporter.mark_transmitted();
    }
    test.run_stack(Some(1));
    assert!(
        count_umac_call_ended_or_close(&test.dump_sinks()) >= 2,
        "D-RELEASE reporter completion should close the simple private call"
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
    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

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
        2,
        "simple private call should still send FACCH and MCCH D-CONNECT"
    );
    assert_eq!(
        connect_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect_acknowledge(prim).is_some()))
            .count(),
        2,
        "simple private call should still send FACCH and MCCH D-CONNECT-ACKNOWLEDGE"
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
    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 14.5.1.2.1 and 14.7.2.3: a simple private
    // U-CONNECT completes with D-CONNECT to the caller and
    // D-CONNECT-ACKNOWLEDGE to the called MS. Optional transmission
    // interruption/pre-emption is not part of this ordinary setup path.
    assert_eq!(count_d_tx_interrupt(&connect_msgs), 0);
    assert_eq!(count_umac_open(&connect_msgs), 1);
    assert_eq!(
        connect_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect(prim).is_some()))
            .count(),
        2,
        "default-off simple private call should send FACCH and MCCH D-CONNECT"
    );
    assert_eq!(
        connect_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_connect_acknowledge(prim).is_some()))
            .count(),
        2,
        "default-off simple private call should send FACCH and MCCH D-CONNECT-ACKNOWLEDGE"
    );
}

#[test]
fn test_example_config_simple_private_call_works_with_preemption_default_off() {
    debug::setup_logging_verbose();

    let config_toml = include_str!("../../../example_config/config.toml");
    let config = from_toml_str(config_toml).expect("example config should parse");
    assert!(config_toml.contains("call_preemptive = false"));
    assert!(
        !config.cell.transmission_interruption_enabled,
        "example config must keep call_preemptive/transmission_interruption_enabled default-off"
    );
    assert_eq!(
        config.cell.energy_saving_mode, 3,
        "example config must exercise Nexus-BS EG3 default while keeping ordinary private call setup available"
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

    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

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

    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

    let open_circuits: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .collect();
    assert_eq!(open_circuits.len(), 1, "simplex private call should open one shared traffic bearer");
    let open = open_circuits[0];
    // EN 300 392-2 clause 14.5.1.2.1: during call set-up, the MS given
    // permission to transmit starts the transmission-control timer. Keep the
    // CMCE grant state and UMAC current UL speaker aligned.
    assert_eq!(open.peer_ts, None);
    assert_eq!(open.active_addr, Some(TetraAddress::new(TEST_ISSI, SsiType::Issi)));
    assert!(
        open.active_secondary_addrs
            .contains(&TetraAddress::new(TEST_CALLED_ISSI, SsiType::Issi)),
        "shared simplex private bearer must still keep the called MS active for assigned-channel listening"
    );

    let d_connects: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(d_connects.len(), 2, "Expected FACCH D-CONNECT plus MCCH fallback");
    for (_, pdu) in &d_connects {
        // EN 300 392-2 clauses 14.7.1.4/14.7.2.3 keep the same timeout and
        // hook method on D-CONNECT when the simple private call is accepted.
        assert_eq!(pdu.call_identifier, call_id);
        assert_eq!(pdu.call_time_out, CallTimeout::T10m);
        assert!(pdu.hook_method_selection);
        assert!(!pdu.simplex_duplex_selection);
    }

    let d_connect_acks: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect_acknowledge(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(d_connect_acks.len(), 2, "Expected FACCH D-CONNECT-ACKNOWLEDGE plus MCCH fallback");
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

    test.submit_message(build_u_connect_custom_msg_with_hook(TEST_CALLED_ISSI, call_id, false, false));
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

    // EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 allow a called MS
    // that cannot support requested duplex to offer simplex in U-CONNECT.
    // The SwMI must not reject that valid simple private-call answer.
    assert_eq!(count_d_releases(&connect_msgs), 0);

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
    assert_eq!(open.active_addr, Some(TetraAddress::new(TEST_ISSI, SsiType::Issi)));
    assert!(
        open.active_secondary_addrs
            .contains(&TetraAddress::new(TEST_CALLED_ISSI, SsiType::Issi)),
        "downgraded simplex private call must keep both MS awake on the shared assigned channel"
    );

    let d_connects: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(d_connects.len(), 2, "Expected FACCH D-CONNECT plus MCCH fallback");
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
    assert_eq!(d_connect_acks.len(), 2, "Expected FACCH D-CONNECT-ACKNOWLEDGE plus MCCH fallback");
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

    test.submit_message(build_u_connect_custom_msg_with_hook(TEST_CALLED_ISSI, call_id, false, false));
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

    assert_eq!(count_d_releases(&connect_msgs), 0);
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
            .contains(&TetraAddress::new(TEST_CALLED_ISSI, SsiType::Issi)),
        "simplex-offered private call must keep both MS awake on the shared assigned channel"
    );

    let d_connects: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(d_connects.len(), 2, "Expected FACCH D-CONNECT plus MCCH fallback");
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
    assert_eq!(d_connect_acks.len(), 2, "Expected FACCH D-CONNECT-ACKNOWLEDGE plus MCCH fallback");
    for (prim, pdu) in &d_connect_acks {
        assert_eq!(prim.main_address.ssi, TEST_CALLED_ISSI);
        assert_eq!(pdu.call_identifier, call_id);
        assert_eq!(pdu.transmission_grant, TransmissionGrant::GrantedToOtherUser);
    }
}

#[test]
fn test_p2p_u_connect_honors_request_to_transmit_other_ms_first() {
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
    u_setup.request_to_transmit_send_data = true;
    test.submit_message(build_u_setup_p2p_custom_msg(TEST_ISSI, u_setup));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs);

    test.submit_message(build_u_connect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

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
        "called-MS-first simplex private call should open one shared traffic bearer"
    );
    let open = open_circuits[0];
    // EN 300 392-2 clause 14.5.1.2.1: during call set-up, the MS given
    // permission to transmit starts the transmission-control timer. Keep the
    // CMCE grant state and UMAC current UL speaker aligned.
    assert_eq!(open.peer_ts, None);
    assert_eq!(open.active_addr, Some(TetraAddress::new(TEST_CALLED_ISSI, SsiType::Issi)));
    assert!(
        open.active_secondary_addrs.contains(&TetraAddress::new(TEST_ISSI, SsiType::Issi)),
        "shared simplex private bearer must still keep the calling MS active for assigned-channel listening"
    );

    let d_connects: Vec<_> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_connect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(d_connects.len(), 2, "Expected FACCH D-CONNECT plus MCCH fallback");
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
    assert_eq!(d_connect_acks.len(), 2, "Expected FACCH D-CONNECT-ACKNOWLEDGE plus MCCH fallback");
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
fn test_simplex_p2p_preemptive_u_tx_demand_with_group_interruption_enabled_is_queued_without_interrupt() {
    debug::setup_logging_verbose();

    // EN 300 392-2 clause 14.5.1.2.1 b) is the baseline when the SwMI does
    // not support private-call transmission interruption: wait for U-TX CEASED
    // and explicitly queue/reject the request. EN 300 392-2 table 14.85
    // marks priorities 2 and 3 as pre-emptive/emergency, but the local config
    // flag is scoped to group-call D-TX INTERRUPT support and must not
    // silently enable P2P pre-emption.
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
    let _ = test.dump_sinks();

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

    test.submit_message(build_u_connect_msg(called_issi, call_id));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_u_tx_ceased_msg(caller_issi, call_id));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

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

    test.submit_message(build_u_connect_msg(called_issi, call_id));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

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

        test.submit_message(build_u_connect_msg(called_issi, call_id));
        test.run_stack(Some(1));
        let connect_msgs = test.dump_sinks();
        let caller_ts = p2p_open_ts_for(&connect_msgs, caller_issi);

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

        test.submit_message(build_u_connect_msg(called_issi, call_id));
        test.run_stack(Some(1));
        let connect_msgs = test.dump_sinks();
        let caller_ts = p2p_open_ts_for(&connect_msgs, caller_issi);

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
    let ceased_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.2.1 b/e) forbids unsolicited D-TX
    // GRANTED, but allows D-TX CEASED to each MS so both CC entities leave
    // the active transmission state.
    assert_eq!(count_d_tx_granted(&ceased_msgs), 0);
    let ceased: Vec<_> = ceased_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_tx_ceased(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(ceased.len(), 2, "end of simplex private transmission should notify both MSs");
    for (_, pdu) in &ceased {
        assert_eq!(pdu.call_identifier, call_id);
        assert!(!pdu.transmission_request_permission);
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
    let disconnect_msgs = test.dump_sinks();
    assert_eq!(
        disconnect_msgs
            .iter()
            .filter(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if parse_d_disconnect(prim).is_some()))
            .count(),
        1,
        "U-DISCONNECT should still start the private disconnect handshake after unsolicited U-RELEASE was ignored"
    );
    assert_eq!(count_umac_call_ended_or_close(&disconnect_msgs), 0);
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
fn test_p2p_u_disconnect_waits_for_peer_release_before_circuit_close() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    // EN 300 392-2 clause 14.7.1.6: after D-DISCONNECT, the called peer
    // responds with U-RELEASE. The BS must not collapse that exchange into an
    // immediate circuit close while the peer still needs the disconnect PDU.
    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut disconnect_msgs = test.dump_sinks();

    let d_disconnects: Vec<_> = disconnect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_disconnect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(d_disconnects.len(), 1, "Expected one D-DISCONNECT to the peer");
    let (disconnect_prim, d_disconnect) = &d_disconnects[0];
    assert_eq!(d_disconnect.call_identifier, call_id);
    assert_eq!(d_disconnect.disconnect_cause, DisconnectCause::UserRequestedDisconnection);
    assert_eq!(disconnect_prim.main_address.ssi, TEST_CALLED_ISSI);
    assert_eq!(disconnect_prim.main_address.ssi_type, SsiType::Issi);
    assert!(disconnect_prim.stealing_permission);
    assert!(disconnect_prim.chan_alloc.is_some());
    assert_eq!(disconnect_prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(
        count_d_releases(&disconnect_msgs),
        0,
        "D-RELEASE should wait for peer U-RELEASE or local guard timeout"
    );
    assert_eq!(
        count_umac_call_ended_or_close(&disconnect_msgs),
        0,
        "P2P circuit must stay open while waiting for peer U-RELEASE"
    );
    let disconnect_reporters = extract_d_disconnect_reporters(&mut disconnect_msgs);
    assert_eq!(
        disconnect_reporters.len(),
        1,
        "Assigned-channel D-DISCONNECT must carry one TxReporter"
    );
    assert_eq!(disconnect_reporters[0].get_state(), TxState::Pending);

    test.run_stack(Some(3));
    let pending_delivery_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_call_ended_or_close(&pending_delivery_msgs),
        0,
        "P2P circuit must stay open while D-DISCONNECT delivery is still pending"
    );

    disconnect_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let transmitted_delivery_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_call_ended_or_close(&transmitted_delivery_msgs),
        0,
        "D-DISCONNECT transmission report should start peer U-RELEASE wait, not close the circuit"
    );

    test.submit_message(build_u_release_msg(TEST_CALLED_ISSI, call_id));
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
        "P2P circuit must stay open until D-RELEASE transmission is reported"
    );

    test.run_stack(Some(3));
    let pending_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_call_ended_or_close(&pending_msgs),
        0,
        "Pending individual release should not close before reporter completion"
    );

    for reporter in &reporters {
        reporter.mark_transmitted();
    }
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "Reporter completion should close the P2P traffic circuit"
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
    // initiate individual-call disconnection. If the called MS disconnects,
    // the SwMI must still deliver D-DISCONNECT to the calling MS and wait for
    // its U-RELEASE response before releasing the assigned channel.
    test.submit_message(build_u_disconnect_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let mut disconnect_msgs = test.dump_sinks();

    let d_disconnects: Vec<_> = disconnect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => parse_d_disconnect(prim).map(|pdu| (prim, pdu)),
            _ => None,
        })
        .collect();
    assert_eq!(d_disconnects.len(), 1, "Expected one D-DISCONNECT to the calling peer");
    let (disconnect_prim, d_disconnect) = &d_disconnects[0];
    assert_eq!(d_disconnect.call_identifier, call_id);
    assert_eq!(d_disconnect.disconnect_cause, DisconnectCause::UserRequestedDisconnection);
    assert_eq!(disconnect_prim.main_address.ssi, TEST_ISSI);
    assert_eq!(disconnect_prim.main_address.ssi_type, SsiType::Issi);
    assert!(disconnect_prim.stealing_permission);
    assert!(disconnect_prim.chan_alloc.is_some());
    assert_eq!(disconnect_prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(count_d_releases(&disconnect_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&disconnect_msgs), 0);

    let disconnect_reporters = extract_d_disconnect_reporters(&mut disconnect_msgs);
    assert_eq!(
        disconnect_reporters.len(),
        1,
        "Assigned-channel D-DISCONNECT to caller must carry one TxReporter"
    );
    assert_eq!(disconnect_reporters[0].get_state(), TxState::Pending);

    disconnect_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let transmitted_delivery_msgs = test.dump_sinks();
    assert_eq!(count_umac_call_ended_or_close(&transmitted_delivery_msgs), 0);

    test.submit_message(build_u_release_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus(&release_msgs, call_id, DisconnectCause::UserRequestedDisconnection);
    let reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(reporters.len(), 2, "Only assigned-channel D-RELEASEs should carry TxReporters");
    assert_eq!(
        count_umac_call_ended_or_close(&release_msgs),
        0,
        "P2P circuit must stay open until D-RELEASE transmission is reported"
    );

    for reporter in &reporters {
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
fn test_p2p_pending_release_ignores_duplicate_u_disconnect_and_tx_demand() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut disconnect_msgs = test.dump_sinks();
    let disconnect_reporters = extract_d_disconnect_reporters(&mut disconnect_msgs);
    assert_eq!(disconnect_reporters.len(), 1);

    disconnect_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let pending_disconnect_msgs = test.dump_sinks();
    assert_eq!(count_umac_call_ended_or_close(&pending_disconnect_msgs), 0);

    test.submit_message(build_u_release_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus(&release_msgs, call_id, DisconnectCause::UserRequestedDisconnection);
    let release_reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(release_reporters.len(), 2);
    assert_eq!(count_umac_call_ended_or_close(&release_msgs), 0);

    // EN 300 392-2 clause 14.5.1.3.2 releases an established individual call
    // with D-RELEASE and then clears the call. During the local FACCH delivery
    // drain, duplicate disconnects or PTT floor requests must not create new
    // call-maintenance signalling for the same call identifier.
    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let duplicate_disconnect_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&duplicate_disconnect_msgs), 0);
    assert_eq!(count_d_releases(&duplicate_disconnect_msgs), 0);
    assert_eq!(count_d_tx_granted(&duplicate_disconnect_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&duplicate_disconnect_msgs), 0);

    test.submit_message(build_u_tx_demand_msg(TEST_CALLED_ISSI, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    assert_eq!(count_d_disconnects(&demand_msgs), 0);
    assert_eq!(count_d_releases(&demand_msgs), 0);
    assert_eq!(count_d_tx_granted(&demand_msgs), 0);
    assert_eq!(count_umac_floor_granted(&demand_msgs), 0);
    assert_eq!(count_umac_call_ended_or_close(&demand_msgs), 0);

    for reporter in &release_reporters {
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
fn test_p2p_disconnect_pending_suppresses_d_setup_resend() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut disconnect_msgs = test.dump_sinks();
    let disconnect_reporters = extract_d_disconnect_reporters(&mut disconnect_msgs);
    assert_eq!(disconnect_reporters.len(), 1);

    disconnect_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let pending_disconnect_msgs = test.dump_sinks();
    assert_eq!(count_umac_call_ended_or_close(&pending_disconnect_msgs), 0);

    // EN 300 392-2 clause 14.5.1.3.3 puts the peer into a release response
    // path after D-DISCONNECT. A cached D-SETUP resend would contradict that
    // disconnection phase and can re-open UI state on real terminals.
    test.run_stack(Some(8));
    let backup_window_msgs = test.dump_sinks();
    assert_eq!(
        count_d_setups(&backup_window_msgs),
        0,
        "DisconnectPending individual call_id={call_id} must not emit backup D-SETUP"
    );
}

#[test]
fn test_p2p_u_disconnect_delivery_guard_falls_back_to_release_without_peer_wait() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    // EN 300 392-2 clause 14.5.1.3.3 makes U-RELEASE the MS response to
    // D-DISCONNECT. If local delivery is not reported, the BS must not start a
    // peer-response wait for a PDU that may never have reached the MS; it falls
    // back to the established-call D-RELEASE clearing path.
    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut disconnect_msgs = test.dump_sinks();
    let disconnect_reporters = extract_d_disconnect_reporters(&mut disconnect_msgs);
    assert_eq!(
        disconnect_reporters.len(),
        1,
        "Assigned-channel D-DISCONNECT must carry one TxReporter"
    );
    assert_eq!(disconnect_reporters[0].get_state(), TxState::Pending);

    test.run_stack(Some(17));
    let mut guard_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus(&guard_msgs, call_id, DisconnectCause::UserRequestedDisconnection);
    let release_reporters = extract_d_release_reporters(&mut guard_msgs);
    assert_eq!(
        release_reporters.len(),
        2,
        "Assigned-channel fallback D-RELEASEs should carry TxReporters"
    );
    assert_eq!(
        count_umac_call_ended_or_close(&guard_msgs),
        0,
        "P2P circuit must stay open until fallback D-RELEASE transmission is reported"
    );

    for reporter in &release_reporters {
        reporter.mark_transmitted();
    }
    test.run_stack(Some(1));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "Reporter completion should close the fallback-released P2P traffic circuit"
    );
}

#[test]
fn test_p2p_discarded_d_disconnect_falls_back_to_d_release() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut disconnect_msgs = test.dump_sinks();
    let disconnect_reporters = extract_d_disconnect_reporters(&mut disconnect_msgs);
    assert_eq!(
        disconnect_reporters.len(),
        1,
        "Assigned-channel D-DISCONNECT must carry one TxReporter"
    );

    // EN 300 392-2 clause 14.7.1.6 expects U-RELEASE only after a
    // D-DISCONNECT reaches the MS. A local UMAC discard is not transmission, so
    // the SwMI uses the clause 14.5.1.3.2 D-RELEASE path instead of waiting for
    // an impossible peer response.
    disconnect_reporters[0].mark_discarded();
    test.run_stack(Some(1));
    let mut release_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus(&release_msgs, call_id, DisconnectCause::UserRequestedDisconnection);
    let release_reporters = extract_d_release_reporters(&mut release_msgs);
    assert_eq!(
        release_reporters.len(),
        2,
        "Assigned-channel fallback D-RELEASEs should carry TxReporters"
    );
    assert_eq!(
        count_umac_call_ended_or_close(&release_msgs),
        0,
        "P2P circuit must stay open until fallback D-RELEASE transmission is reported"
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
fn test_p2p_pending_disconnect_closes_after_bounded_timeout() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, TEST_CALLED_ISSI, TEST_CALLED_GSSI);
    let call_id = start_active_p2p_call(&mut test);

    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let mut disconnect_msgs = test.dump_sinks();
    assert_eq!(count_umac_call_ended_or_close(&disconnect_msgs), 0);
    let disconnect_reporters = extract_d_disconnect_reporters(&mut disconnect_msgs);
    assert_eq!(
        disconnect_reporters.len(),
        1,
        "Assigned-channel D-DISCONNECT must carry one TxReporter"
    );

    disconnect_reporters[0].mark_transmitted();
    test.run_stack(Some(1));
    let delivered_disconnect_msgs = test.dump_sinks();
    assert_eq!(
        count_umac_call_ended_or_close(&delivered_disconnect_msgs),
        0,
        "D-DISCONNECT delivery report starts peer U-RELEASE wait without closing the circuit"
    );

    test.run_stack(Some(20));
    let release_msgs = test.dump_sinks();
    assert_established_p2p_release_pdus(&release_msgs, call_id, DisconnectCause::UserRequestedDisconnection);
    assert_eq!(
        count_umac_call_ended_or_close(&release_msgs),
        0,
        "Disconnect timeout should emit D-RELEASE before closing the assigned channel"
    );

    test.run_stack(Some(20));
    let closed_msgs = test.dump_sinks();
    assert!(
        count_umac_call_ended_or_close(&closed_msgs) >= 2,
        "Local release guard timeout should eventually close a stuck P2P release"
    );
}
