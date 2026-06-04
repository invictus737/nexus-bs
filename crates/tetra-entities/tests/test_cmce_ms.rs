mod common;

use common::ComponentTest;
use tetra_config::bluestation::StackMode;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, BurstType, Layer2Service, PhyBlockNum, PhyBlockType, Sap, SsiType, TetraAddress, TrainingSequence, debug};
use tetra_entities::lmac::components::errorcontrol;
use tetra_entities::umac::subcomp::fillbits;
use tetra_pdus::cmce::enums::call_timeout::CallTimeout;
use tetra_pdus::cmce::enums::disconnect_cause::DisconnectCause;
use tetra_pdus::cmce::enums::transmission_grant::TransmissionGrant;
use tetra_pdus::cmce::fields::basic_service_information::BasicServiceInformation;
use tetra_pdus::cmce::pdus::d_connect::DConnect;
use tetra_pdus::cmce::pdus::d_connect_acknowledge::DConnectAcknowledge;
use tetra_pdus::cmce::pdus::d_disconnect::DDisconnect;
use tetra_pdus::cmce::pdus::d_release::DRelease;
use tetra_pdus::cmce::pdus::d_setup::DSetup;
use tetra_pdus::cmce::pdus::d_tx_granted::DTxGranted;
use tetra_pdus::cmce::pdus::u_connect::UConnect;
use tetra_pdus::cmce::pdus::u_disconnect::UDisconnect;
use tetra_pdus::cmce::pdus::u_release::URelease;
use tetra_pdus::llc::pdus::bl_udata::BlUdata;
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;
use tetra_pdus::umac::pdus::mac_access::MacAccess;
use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
use tetra_saps::control::enums::communication_type::CommunicationType;
use tetra_saps::lcmc::{LcmcMleConfigureReq, LcmcMleUnitdataInd, LcmcMleUnitdataReq};
use tetra_saps::tmv::{TmvUnitdataReq, enums::logical_chans::LogicalChannel};
use tetra_saps::tp::{TpUnitdataInd, TpUnitdataReqSlot};
use tetra_saps::{SapMsg, SapMsgInner};

const LOCAL_ISSI: u32 = 1000001;
const CALL_ID: u16 = 0x234;
const SCH_HU_TYPE1_CAP_BITS: usize = 92;

fn build_lcmc_ind(local_issi: u32, sdu: BitBuffer, endpoint_id: u32) -> SapMsg {
    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 7,
            endpoint_id,
            link_id: 3,
            received_tetra_address: TetraAddress::new(local_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_d_disconnect_msg(endpoint_id: u32) -> SapMsg {
    let pdu = DDisconnect {
        call_identifier: CALL_ID,
        disconnect_cause: DisconnectCause::SwmiRequestedDisconnection,
        notification_indicator: None,
        facility: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(32);
    pdu.to_bitbuf(&mut sdu).expect("failed to serialize D-DISCONNECT");
    sdu.seek(0);
    build_lcmc_ind(LOCAL_ISSI, sdu, endpoint_id)
}

fn build_d_release_msg(endpoint_id: u32) -> SapMsg {
    let pdu = DRelease {
        call_identifier: CALL_ID,
        disconnect_cause: DisconnectCause::SwmiRequestedDisconnection,
        notification_indicator: None,
        facility: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(32);
    pdu.to_bitbuf(&mut sdu).expect("failed to serialize D-RELEASE");
    sdu.seek(0);
    build_lcmc_ind(LOCAL_ISSI, sdu, endpoint_id)
}

fn with_channel_change_request(mut msg: SapMsg, handle: i32) -> SapMsg {
    let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut msg.msg else {
        panic!("expected LCMC-MLE-UNITDATA indication");
    };
    prim.chan_change_resp_req = true;
    prim.chan_change_handle = Some(handle);
    msg
}

fn build_d_connect_msg(endpoint_id: u32) -> SapMsg {
    build_d_connect_msg_with_grant(endpoint_id, TransmissionGrant::Granted)
}

fn build_d_connect_msg_with_grant(endpoint_id: u32, transmission_grant: TransmissionGrant) -> SapMsg {
    let pdu = DConnect {
        call_identifier: CALL_ID,
        call_time_out: CallTimeout::T5m,
        hook_method_selection: false,
        simplex_duplex_selection: false,
        transmission_grant,
        transmission_request_permission: true,
        call_ownership: true,
        call_priority: None,
        basic_service_information: None,
        temporary_address: None,
        notification_indicator: None,
        facility: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(40);
    pdu.to_bitbuf(&mut sdu).expect("failed to serialize D-CONNECT");
    sdu.seek(0);
    build_lcmc_ind(LOCAL_ISSI, sdu, endpoint_id)
}

fn build_d_connect_acknowledge_msg(endpoint_id: u32) -> SapMsg {
    build_d_connect_acknowledge_msg_with_grant(endpoint_id, TransmissionGrant::Granted)
}

fn build_d_connect_acknowledge_msg_with_grant(endpoint_id: u32, transmission_grant: TransmissionGrant) -> SapMsg {
    let pdu = DConnectAcknowledge {
        call_identifier: CALL_ID,
        call_time_out: CallTimeout::T5m,
        transmission_grant,
        transmission_request_permission: true,
        notification_indicator: None,
        facility: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(32);
    pdu.to_bitbuf(&mut sdu).expect("failed to serialize D-CONNECT-ACKNOWLEDGE");
    sdu.seek(0);
    build_lcmc_ind(LOCAL_ISSI, sdu, endpoint_id)
}

fn build_d_tx_granted_msg(endpoint_id: u32, transmission_grant: TransmissionGrant) -> SapMsg {
    build_d_tx_granted_msg_with_call_id(endpoint_id, CALL_ID, transmission_grant)
}

fn build_d_tx_granted_msg_with_call_id(endpoint_id: u32, call_identifier: u16, transmission_grant: TransmissionGrant) -> SapMsg {
    let pdu = DTxGranted {
        call_identifier,
        transmission_grant: transmission_grant.into_raw() as u8,
        transmission_request_permission: true,
        encryption_control: false,
        reserved: false,
        notification_indicator: None,
        transmitting_party_type_identifier: None,
        transmitting_party_address_ssi: None,
        transmitting_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(40);
    pdu.to_bitbuf(&mut sdu).expect("failed to serialize D-TX GRANTED");
    sdu.seek(0);
    build_lcmc_ind(LOCAL_ISSI, sdu, endpoint_id)
}

fn build_d_setup_msg_with_bsi(endpoint_id: u32, hook_method_selection: bool, basic_service_information: BasicServiceInformation) -> SapMsg {
    let pdu = DSetup {
        call_identifier: CALL_ID,
        call_time_out: CallTimeout::T5m,
        hook_method_selection,
        simplex_duplex_selection: false,
        basic_service_information,
        transmission_grant: TransmissionGrant::GrantedToOtherUser,
        transmission_request_permission: true,
        call_priority: 0,
        notification_indicator: None,
        temporary_address: None,
        calling_party_address_ssi: Some(2000001),
        calling_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(80);
    pdu.to_bitbuf(&mut sdu).expect("failed to serialize D-SETUP");
    sdu.seek(0);
    build_lcmc_ind(LOCAL_ISSI, sdu, endpoint_id)
}

fn build_d_setup_msg(endpoint_id: u32, hook_method_selection: bool) -> SapMsg {
    build_d_setup_msg_with_bsi(
        endpoint_id,
        hook_method_selection,
        BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2p,
            slots_per_frame: None,
            speech_service: Some(0),
        },
    )
}

fn has_lcmc_mle_unitdata_req(msgs: &[SapMsg]) -> bool {
    msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(_)))
}

fn extract_tmv_unitdata_req(msgs: &[SapMsg]) -> &TmvUnitdataReq {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) => slot.blk1.as_ref(),
            _ => None,
        })
        .expect("expected TMV-UNITDATA request toward LMAC")
}

fn extract_tp_unitdata_req(msgs: &[SapMsg]) -> &TpUnitdataReqSlot {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TpUnitdataReq(prim) => Some(prim),
            _ => None,
        })
        .expect("expected TP-UNITDATA request toward PHY")
}

fn parse_u_connect_from_mac_access(prim: &TmvUnitdataReq) -> (MacAccess, BlUdata, UConnect) {
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

fn parse_u_connect_from_encoded_sch_hu(block: &BitBuffer, scrambling_code: u32) -> (MacAccess, BlUdata, UConnect) {
    let (decoded, crc_pass) = errorcontrol::decode_cp(
        LogicalChannel::SchHu,
        TpUnitdataInd {
            train_type: TrainingSequence::ExtendedTrainSeq,
            burst_type: BurstType::CUB,
            block_type: PhyBlockType::SSN1,
            block_num: PhyBlockNum::Block1,
            block: BitBuffer::from_bitbuffer(block),
            rssi_dbfs: 0.0,
        },
        Some(scrambling_code),
    );
    assert!(crc_pass);
    parse_u_connect_from_mac_access(&TmvUnitdataReq {
        mac_block: decoded.expect("decoded SCH/HU MAC block"),
        logical_channel: LogicalChannel::SchHu,
        scrambling_code,
    })
}

fn extract_u_release_req(msgs: &[SapMsg]) -> (&LcmcMleUnitdataReq, URelease) {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                URelease::from_bitbuf(&mut sdu).ok().map(|pdu| (prim, pdu))
            }
            _ => None,
        })
        .expect("expected U-RELEASE toward MLE")
}

fn extract_u_connect_req(msgs: &[SapMsg]) -> (&LcmcMleUnitdataReq, UConnect) {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                UConnect::from_bitbuf(&mut sdu).ok().map(|pdu| (prim, pdu))
            }
            _ => None,
        })
        .expect("expected U-CONNECT toward MLE")
}

fn extract_u_disconnect_req(msgs: &[SapMsg]) -> (&LcmcMleUnitdataReq, UDisconnect) {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim) => {
                let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
                UDisconnect::from_bitbuf(&mut sdu).ok().map(|pdu| (prim, pdu))
            }
            _ => None,
        })
        .expect("expected U-DISCONNECT toward MLE")
}

fn extract_lcmc_configure_req(msgs: &[SapMsg]) -> &LcmcMleConfigureReq {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleConfigureReq(prim) => Some(prim),
            _ => None,
        })
        .expect("expected LCMC-MLE-CONFIGURE request toward MLE")
}

fn has_lcmc_configure_req(msgs: &[SapMsg]) -> bool {
    msgs.iter().any(|msg| matches!(&msg.msg, SapMsgInner::LcmcMleConfigureReq(_)))
}

#[test]
fn test_ms_private_direct_d_setup_reaches_phy_as_sch_hu_cub_u_connect() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(
        vec![
            TetraEntity::Cmce,
            TetraEntity::Mle,
            TetraEntity::Llc,
            TetraEntity::Umac,
            TetraEntity::Lmac,
        ],
        vec![TetraEntity::Phy],
    );

    test.submit_message(build_d_setup_msg(2, false));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let tp = extract_tp_unitdata_req(&sink_msgs);
    assert_eq!(tp.burst_type, BurstType::CUB);
    assert_eq!(tp.train_type, TrainingSequence::ExtendedTrainSeq);
    assert!(tp.bbk.is_none());
    assert!(tp.blk2.is_none());
    let encoded = tp.blk1.as_ref().expect("expected encoded SCH/HU block");
    assert_eq!(encoded.get_len(), 168);

    // EN 300 392-2 clauses 14.5.1.1.1, 14.7.2.3, 20.4.1.1.4,
    // 21.4.2.1, 23.5.2.4 and 8.3.1.4.3: direct private setup acceptance
    // produces U-CONNECT, which the MS can carry in a SCH/HU MAC-ACCESS
    // random-access PDU and encode as a CUB for PHY. This is still not a
    // formal conformance or RF certification claim; PHY CUB slotting remains
    // a separate boundary.
    let (mac_access, bl_udata, u_connect) = parse_u_connect_from_encoded_sch_hu(encoded, 0);
    assert_eq!(mac_access.addr, Some(TetraAddress::new(LOCAL_ISSI, SsiType::Issi)));
    assert!(!mac_access.encrypted);
    assert!(!bl_udata.has_fcs);
    assert_eq!(u_connect.call_identifier, CALL_ID);
}

#[test]
fn test_ms_private_encrypted_d_setup_rejected_with_u_disconnect() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_setup_msg_with_bsi(
        2,
        false,
        BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: true,
            communication_type: CommunicationType::P2p,
            slots_per_frame: None,
            speech_service: Some(0),
        },
    ));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let (prim, u_disconnect) = extract_u_disconnect_req(&sink_msgs);

    // EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.5: when the called MS
    // cannot accept the requested encryption state and cannot offer a service
    // alternative, it rejects the call with U-DISCONNECT and an encryption
    // disconnect cause. Crypto itself is intentionally outside this patch.
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(prim.main_address, TetraAddress::new(LOCAL_ISSI, SsiType::Issi));
    assert_eq!(u_disconnect.call_identifier, CALL_ID);
    assert_eq!(u_disconnect.disconnect_cause, DisconnectCause::CalledPartyDoesNotSupportEncryption);
}

#[test]
fn test_ms_private_unsupported_basic_service_rejected_with_u_disconnect() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_setup_msg_with_bsi(
        2,
        false,
        BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2Mp,
            slots_per_frame: None,
            speech_service: Some(0),
        },
    ));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let (prim, u_disconnect) = extract_u_disconnect_req(&sink_msgs);

    // EN 300 392-2 clause 14.5.1.1.1: if the requested basic service is not
    // acceptable and no alternative is offered, called-side CC rejects with
    // U-DISCONNECT. The headless MS shim currently auto-accepts only direct
    // unencrypted P2P TCH/S speech.
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(u_disconnect.call_identifier, CALL_ID);
    assert_eq!(u_disconnect.disconnect_cause, DisconnectCause::CallRejectedByTheCalledParty);
}

#[test]
fn test_ms_private_direct_d_setup_reaches_umac_as_sch_hu_u_connect() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(
        vec![TetraEntity::Cmce, TetraEntity::Mle, TetraEntity::Llc, TetraEntity::Umac],
        vec![TetraEntity::Lmac],
    );

    test.submit_message(build_d_setup_msg(2, false));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let tmv = extract_tmv_unitdata_req(&sink_msgs);
    assert_eq!(tmv.logical_channel, LogicalChannel::SchHu);
    let (mac_access, bl_udata, u_connect) = parse_u_connect_from_mac_access(tmv);

    // EN 300 392-2 clauses 14.5.1.1.1, 14.7.2.3, 20.4.1.1.4 and
    // 21.4.2.1: direct private setup acceptance emits U-CONNECT, which the
    // MS lower layers can carry as a small unacknowledged CMCE TM-SDU in
    // SCH/HU MAC-ACCESS. This is not a physical-air certification claim:
    // LMAC-MS transmit encoding is a separate implementation boundary.
    assert_eq!(mac_access.addr, Some(TetraAddress::new(LOCAL_ISSI, SsiType::Issi)));
    assert!(!mac_access.encrypted);
    assert!(!bl_udata.has_fcs);
    assert_eq!(u_connect.call_identifier, CALL_ID);
}

#[test]
fn test_ms_private_direct_d_setup_sends_u_connect() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_setup_msg(2, false));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let (prim, u_connect) = extract_u_connect_req(&sink_msgs);

    // EN 300 392-2 clause 14.5.1.1.1 and table 14.23: direct incoming
    // private setup is accepted with U-CONNECT, then the MS waits for
    // D-CONNECT ACKNOWLEDGE. This test covers only the headless direct P2P
    // accept path, not full TNCC/user application or U-plane configuration.
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(prim.main_address, TetraAddress::new(LOCAL_ISSI, SsiType::Issi));
    assert_eq!(prim.handle, 7);
    assert_eq!(prim.endpoint_id, 2);
    assert_eq!(prim.link_id, 3);
    assert!(prim.stealing_permission);
    assert_eq!(u_connect.call_identifier, CALL_ID);
    assert!(!u_connect.hook_method_selection);
    assert!(!u_connect.simplex_duplex_selection);
    assert!(u_connect.basic_service_information.is_none());
    assert!(u_connect.facility.is_none());
    assert!(u_connect.proprietary.is_none());
}

#[test]
fn test_ms_private_d_connect_acknowledge_completes_called_side_without_response() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_setup_msg(2, false));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let (_, u_connect) = extract_u_connect_req(&setup_msgs);
    assert_eq!(u_connect.call_identifier, CALL_ID);

    test.submit_message(build_d_connect_acknowledge_msg(2));
    test.run_stack(Some(1));
    let ack_msgs = test.dump_sinks();
    let configure = extract_lcmc_configure_req(&ack_msgs);

    // EN 300 392-2 clauses 14.5.1.1.1, 14.7.1.5 and 14.5.1.4.1: after
    // U-CONNECT, D-CONNECT ACKNOWLEDGE orders the called MS to
    // through-connect and has no CMCE response PDU, but CC shall configure
    // lower layers according to the transmission grant.
    assert!(
        !has_lcmc_mle_unitdata_req(&ack_msgs),
        "D-CONNECT-ACKNOWLEDGE must not produce an uplink CMCE response"
    );
    assert_eq!(configure.endpoint_id, 2);
    assert_eq!(configure.call_release, None);
    assert!(configure.switch_u_plane);
    assert!(configure.tx_grant);
}

#[test]
fn test_ms_private_d_connect_acknowledge_without_setup_rejects_invalid_call_id() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_connect_acknowledge_msg(2));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (prim, u_disconnect) = extract_u_disconnect_req(&sink_msgs);

    // EN 300 392-2 clause 14.5.6.5.2: an individually addressed downlink PDU
    // other than D-SETUP/D-RELEASE with an unrecognized call identifier is
    // rejected with U-DISCONNECT/Invalid call identifier. D-CONNECT-ACKNOWLEDGE
    // is only valid for the called-side setup context after D-SETUP/U-CONNECT.
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(u_disconnect.call_identifier, CALL_ID);
    assert_eq!(u_disconnect.disconnect_cause, DisconnectCause::InvalidCallIdentifier);
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleConfigureReq(_))),
        "invalid D-CONNECT-ACKNOWLEDGE must not configure lower layers"
    );
}

#[test]
fn test_ms_private_d_connect_completes_calling_side_without_response() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_connect_msg(2));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let configure = extract_lcmc_configure_req(&sink_msgs);

    // EN 300 392-2 clauses 14.5.1.2.1, 14.7.1.4 and 14.5.1.4.1:
    // D-CONNECT orders the calling MS to through-connect and has no CMCE
    // response PDU, but CC shall configure lower layers according to the
    // transmission grant.
    assert!(
        !has_lcmc_mle_unitdata_req(&sink_msgs),
        "D-CONNECT must not produce an uplink CMCE response"
    );
    assert_eq!(configure.endpoint_id, 2);
    assert_eq!(configure.call_release, None);
    assert!(configure.switch_u_plane);
    assert!(configure.tx_grant);
}

#[test]
fn test_ms_private_d_connect_acknowledge_granted_to_other_user_configures_receive_only() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_setup_msg(2, false));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_d_connect_acknowledge_msg_with_grant(2, TransmissionGrant::GrantedToOtherUser));
    test.run_stack(Some(1));
    let ack_msgs = test.dump_sinks();
    let configure = extract_lcmc_configure_req(&ack_msgs);

    // EN 300 392-2 clause 14.5.1.4.1: "transmission granted to another
    // user" switches U-plane on for receive but leaves Tx grant false.
    assert!(!has_lcmc_mle_unitdata_req(&ack_msgs));
    assert_eq!(configure.endpoint_id, 2);
    assert!(configure.switch_u_plane);
    assert!(!configure.tx_grant);
}

#[test]
fn test_ms_private_d_connect_not_granted_keeps_u_plane_off() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_connect_msg_with_grant(2, TransmissionGrant::NotGranted));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let configure = extract_lcmc_configure_req(&sink_msgs);

    // EN 300 392-2 clause 14.5.1.4.1: for grant values other than
    // "transmission granted" or "transmission granted to another user", the
    // U-plane shall not be switched on.
    assert!(!has_lcmc_mle_unitdata_req(&sink_msgs));
    assert_eq!(configure.endpoint_id, 2);
    assert!(!configure.switch_u_plane);
    assert!(!configure.tx_grant);
}

#[test]
fn test_ms_private_d_connect_acknowledge_request_queued_keeps_u_plane_off() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_setup_msg(2, false));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_d_connect_acknowledge_msg_with_grant(2, TransmissionGrant::RequestQueued));
    test.run_stack(Some(1));
    let ack_msgs = test.dump_sinks();
    let configure = extract_lcmc_configure_req(&ack_msgs);

    // EN 300 392-2 clause 14.5.1.4.1: queued/not-granted transmission
    // permission does not through-connect the U-plane.
    assert!(!has_lcmc_mle_unitdata_req(&ack_msgs));
    assert_eq!(configure.endpoint_id, 2);
    assert!(!configure.switch_u_plane);
    assert!(!configure.tx_grant);
}

#[test]
fn test_ms_private_d_tx_granted_granted_switches_tx_on() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_connect_msg(2));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_d_tx_granted_msg(2, TransmissionGrant::Granted));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let configure = extract_lcmc_configure_req(&sink_msgs);

    // EN 300 392-2 clause 14.5.1.4.2 and table 14.80:
    // D-TX GRANTED / transmission granted switches U-plane on and grants Tx.
    assert!(!has_lcmc_mle_unitdata_req(&sink_msgs));
    assert_eq!(configure.endpoint_id, 2);
    assert!(configure.switch_u_plane);
    assert!(configure.tx_grant);
}

#[test]
fn test_ms_private_d_tx_granted_granted_to_other_user_switches_receive_only() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_connect_msg(2));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_d_tx_granted_msg(2, TransmissionGrant::GrantedToOtherUser));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let configure = extract_lcmc_configure_req(&sink_msgs);

    // EN 300 392-2 clause 14.5.1.4.2 and table 14.80:
    // granted-to-other-user switches U-plane on for receive, with Tx grant off.
    assert!(!has_lcmc_mle_unitdata_req(&sink_msgs));
    assert_eq!(configure.endpoint_id, 2);
    assert!(configure.switch_u_plane);
    assert!(!configure.tx_grant);
}

#[test]
fn test_ms_private_d_tx_granted_not_granted_does_not_reconfigure_or_release() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_connect_msg(2));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_d_tx_granted_msg(2, TransmissionGrant::NotGranted));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.4.2: for transmission-not-granted, the
    // U-plane state shall not be changed and no release is implied.
    assert!(!has_lcmc_configure_req(&sink_msgs));
    assert!(!has_lcmc_mle_unitdata_req(&sink_msgs));
}

#[test]
fn test_ms_private_d_tx_granted_request_queued_does_not_reconfigure_or_release() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_connect_msg(2));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(build_d_tx_granted_msg(2, TransmissionGrant::RequestQueued));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.4.2: queued request status is only a
    // response to the request-to-transmit; it must not switch U-plane state.
    assert!(!has_lcmc_configure_req(&sink_msgs));
    assert!(!has_lcmc_mle_unitdata_req(&sink_msgs));
}

#[test]
fn test_ms_private_d_tx_granted_unknown_call_id_sends_invalid_call_u_disconnect() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_tx_granted_msg_with_call_id(2, CALL_ID + 1, TransmissionGrant::Granted));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let (prim, u_disconnect) = extract_u_disconnect_req(&sink_msgs);

    // EN 300 392-2 clause 14.5.6.5.2: an individually addressed downlink PDU
    // with an unrecognized call identifier is rejected with
    // U-DISCONNECT/Invalid call identifier.
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(u_disconnect.call_identifier, CALL_ID + 1);
    assert_eq!(u_disconnect.disconnect_cause, DisconnectCause::InvalidCallIdentifier);
    assert!(!has_lcmc_configure_req(&sink_msgs));
}

#[test]
fn test_ms_private_on_off_hook_d_setup_does_not_auto_connect_without_tncc() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_setup_msg(2, true));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // EN 300 392-2 clause 14.5.1.1.1: on/off-hook setup needs user/TNCC
    // progress before U-ALERT or U-CONNECT. The direct setup shim must not
    // auto-answer that signalling method.
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(_))),
        "on/off-hook D-SETUP must not produce an uplink CMCE PDU without TNCC"
    );
}

#[test]
fn test_ms_private_d_disconnect_sends_u_release() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(with_channel_change_request(build_d_disconnect_msg(2), 99));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let (prim, u_release) = extract_u_release_req(&sink_msgs);
    let configure = extract_lcmc_configure_req(&sink_msgs);

    // EN 300 392-2 clause 14.5.1.3.3: D-DISCONNECT is acknowledged by
    // U-RELEASE. In both D-DISCONNECT and D-RELEASE cases, CC must also send
    // lower-layer CONFIGURE to switch U-plane off and accept any channel
    // change required by the release PDU.
    assert_eq!(prim.layer2service, Layer2Service::Unacknowledged);
    assert_eq!(prim.main_address, TetraAddress::new(LOCAL_ISSI, SsiType::Issi));
    assert_eq!(prim.handle, 7);
    assert_eq!(prim.endpoint_id, 2);
    assert_eq!(prim.link_id, 3);
    assert!(prim.stealing_permission);
    assert_eq!(u_release.call_identifier, CALL_ID);
    assert_eq!(u_release.disconnect_cause, DisconnectCause::SwmiRequestedDisconnection);
    assert_eq!(configure.endpoint_id, 2);
    assert_eq!(configure.chan_change_accepted, Some(true));
    assert_eq!(configure.chan_change_handle, 99);
    assert_eq!(configure.call_release, Some(CALL_ID as i32));
    assert!(!configure.switch_u_plane);
    assert!(!configure.tx_grant);
}

#[test]
fn test_ms_private_d_release_cleans_without_uplink_response() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle]);

    test.submit_message(build_d_setup_msg(2, false));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let (_, u_connect) = extract_u_connect_req(&setup_msgs);
    assert_eq!(u_connect.call_identifier, CALL_ID);

    test.submit_message(with_channel_change_request(build_d_release_msg(2), 100));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    let configure = extract_lcmc_configure_req(&sink_msgs);

    // EN 300 392-2 clause 14.5.1.3.3: D-RELEASE shall not trigger a CMCE
    // response. It still clears call state and sends lower-layer CONFIGURE to
    // switch U-plane off and leave the assigned channel.
    assert!(
        sink_msgs.iter().all(|msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(_))),
        "D-RELEASE must not produce an uplink CMCE PDU"
    );
    assert_eq!(configure.endpoint_id, 2);
    assert_eq!(configure.chan_change_accepted, Some(true));
    assert_eq!(configure.chan_change_handle, 100);
    assert_eq!(configure.call_release, Some(CALL_ID as i32));
    assert_eq!(configure.circuit_mode_type, CircuitModeType::TchS);
    assert!(!configure.encryption_flag);
    assert!(!configure.switch_u_plane);
    assert!(!configure.tx_grant);
}
