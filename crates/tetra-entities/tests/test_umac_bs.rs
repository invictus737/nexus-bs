mod common;

use tetra_config::bluestation::{EnergySavingAssignment, SharedConfig, StackMode};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{
    BitBuffer, BurstType, Direction, Layer2Service, PhyBlockNum, PhyBlockType, PhysicalChannel, Sap, SsiType, TdmaTime, TetraAddress,
    TrainingSequence, TxReporter, TxState, debug,
};
use tetra_entities::lmac::components::{errorcontrol, scrambler};
use tetra_entities::umac::umac_bs::UmacBs;
use tetra_pdus::cmce::enums::{call_timeout::CallTimeout, transmission_grant::TransmissionGrant};
use tetra_pdus::cmce::fields::basic_service_information::BasicServiceInformation;
use tetra_pdus::cmce::pdus::d_connect::DConnect;
use tetra_pdus::cmce::pdus::d_setup::DSetup;
use tetra_pdus::cmce::pdus::d_tx_ceased::DTxCeased;
use tetra_pdus::cmce::pdus::d_tx_granted::DTxGranted;
use tetra_pdus::llc::enums::llc_pdu_type::LlcPduType;
use tetra_pdus::llc::pdus::bl_ack::BlAck;
use tetra_pdus::llc::pdus::bl_adata::BlAdata;
use tetra_pdus::llc::pdus::bl_data::BlData;
use tetra_pdus::llc::pdus::bl_udata::BlUdata;
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;
use tetra_pdus::umac::enums::basic_slotgrant_cap_alloc::BasicSlotgrantCapAlloc;
use tetra_pdus::umac::enums::basic_slotgrant_granting_delay::BasicSlotgrantGrantingDelay;
use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
use tetra_pdus::umac::pdus::mac_access::MacAccess;
use tetra_pdus::umac::pdus::mac_end_dl::MacEndDl;
use tetra_pdus::umac::pdus::mac_frag_dl::MacFragDl;
use tetra_pdus::umac::pdus::mac_resource::MacResource;
use tetra_pdus::umac::pdus::mac_u_blck::MacUBlck;
use tetra_pdus::umac::pdus::mac_u_signal::MacUSignal;
use tetra_saps::control::call_control::{CallControl, Circuit, CircuitDlMediaSource};
use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
use tetra_saps::control::enums::communication_type::CommunicationType;
use tetra_saps::lcmc::enums::alloc_type::ChanAllocType;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::lmm::LmmMleUnitdataReq;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};
use tetra_saps::tlmc::{TlmcConfigureReq, TlmcEnergyEconomyStartpoint};
use tetra_saps::tma::{TmaCancelReq, TmaReport, TmaUnitdataReq};
use tetra_saps::tmd::{TmdCircuitDataInd, TmdCircuitDataReq};
use tetra_saps::tmv::{TmvUnitdataInd, TmvUnitdataReq, enums::logical_chans::LogicalChannel};
use tetra_saps::tp::TpUnitdataInd;

use crate::common::ComponentTest;

fn eg_assignment(start: TdmaTime) -> EnergySavingAssignment {
    EnergySavingAssignment {
        mode: 7,
        frame: Some(start.f),
        multiframe: Some(start.m),
        awake_until: None,
        suspension_count: 0,
    }
}

fn alternating_bits(len: usize) -> String {
    (0..len).map(|idx| if idx % 2 == 0 { '1' } else { '0' }).collect()
}

fn group_call_open_msg_for_direction(gssi: u32, ts: u8, direction: Direction) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Open(Circuit {
            direction,
            ts,
            peer_ts: None,
            usage: 4,
            circuit_mode: CircuitModeType::TchS,
            speech_service: Some(0),
            etee_encrypted: false,
            dl_media_source: CircuitDlMediaSource::LocalLoopback,
            active_addr: Some(TetraAddress::new(gssi, SsiType::Gssi)),
            active_secondary_addrs: Vec::new(),
        })),
    }
}

fn group_call_open_msg(gssi: u32, ts: u8) -> SapMsg {
    group_call_open_msg_for_direction(gssi, ts, Direction::Both)
}

fn group_call_open_msg_with_secondary_speaker(gssi: u32, speaker_issi: u32, ts: u8) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Open(Circuit {
            direction: Direction::Both,
            ts,
            peer_ts: None,
            usage: 4,
            circuit_mode: CircuitModeType::TchS,
            speech_service: Some(0),
            etee_encrypted: false,
            dl_media_source: CircuitDlMediaSource::LocalLoopback,
            active_addr: Some(TetraAddress::new(gssi, SsiType::Gssi)),
            active_secondary_addrs: vec![TetraAddress::issi(speaker_issi)],
        })),
    }
}

fn tlmc_configure_req() -> TlmcConfigureReq {
    TlmcConfigureReq {
        threshold_values: None,
        distribution_on_18th_frame: None,
        scch_information: None,
        energy_economy_issi: None,
        energy_economy_group: None,
        energy_economy_startpoint: None,
        dual_watch_energy_economy_group: None,
        dual_watch_startpoint: None,
        mle_activity_indicator: None,
        channel_change_accepted: None,
        channel_change_handle: None,
        operating_mode: None,
        call_release: None,
        valid_addresses: None,
        ms_default_data_priority: None,
        layer_2_data_priority_lifetime: None,
        layer_2_data_priority_signalling_delay: None,
        data_priority_random_access_delay_factor: None,
        schedule_repetition_information: None,
        data_class_activity_information: None,
        endpoint_id: None,
        periodic_reporting_timer: None,
        graceful_service_degradation_mode_control: None,
    }
}

fn submit_tlmc_configure_req(test: &mut ComponentTest, prim: TlmcConfigureReq) {
    test.submit_message(SapMsg {
        sap: Sap::TlmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TlmcConfigureReq(prim),
    });
}

#[test]
fn test_frame_18_extension_is_not_advertised_without_full_frame18_receive_support() {
    debug::setup_logging_verbose();

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.frame_18_ext = true;
    let shared_config = SharedConfig::from_parts(config, None);
    let precomps = UmacBs::generate_precomps(&shared_config);

    // EN 300 392-2 clause 21.4.6.5: frame 18 extension allows MSs to
    // receive downlink information on all slots of frame 18. This BS only
    // schedules SCH/F on legal non-fixed frame-18 opportunities, so MAC-SYNC
    // must not advertise all-slot frame-18 reception.
    assert!(!precomps.mac_sync.frame_18_ext);
}

#[test]
fn test_mle_sysinfo_does_not_advertise_sndcp_service_until_bearer_is_implemented() {
    debug::setup_logging_verbose();

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sndcp_service = true;
    let shared_config = SharedConfig::from_parts(config, None);
    let precomps = UmacBs::generate_precomps(&shared_config);

    // EN 300 392-2 clauses 18.5.2.1 and 18.5.21 expose packet-data/SNDCP
    // availability via local BS service details. WAP/IP depends on this bearer,
    // and the current SNDCP entity is fail-closed, so a direct StackConfig must
    // not bypass parser validation and advertise SNDCP on-air.
    assert!(
        !precomps.mle_sysinfo.bs_service_details.sndcp_service,
        "local D-MLE-SYSINFO must not advertise SNDCP/WAP until the bearer is implemented"
    );
}

#[test]
fn test_tlmc_configure_without_energy_economy_is_ignored_without_panic() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Umac], vec![]);

    // EN 300 392-2 clause 20.4.3 defines TLMC/TMC configuration primitives
    // for lower-layer resource and MAC management. Unsupported/empty fields
    // should degrade to a no-op rather than panic.
    submit_tlmc_configure_req(&mut test, tlmc_configure_req());
    test.run_stack(Some(1));
}

#[test]
fn test_tlmc_energy_economy_configures_and_clears_umac_assignment() {
    debug::setup_logging_verbose();

    let start = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
    let issi = 1234;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![]);

    // EN 300 392-2 clauses 20.3.5.4.1c and 23.7.6 route the negotiated
    // energy economy group/startpoint to MAC through TL/TMC-CONFIGURE.
    submit_tlmc_configure_req(
        &mut test,
        TlmcConfigureReq {
            energy_economy_issi: Some(issi),
            energy_economy_group: Some(1),
            energy_economy_startpoint: Some(TlmcEnergyEconomyStartpoint { frame: 3, multiframe: 1 }),
            ..tlmc_configure_req()
        },
    );
    test.run_stack(Some(1));
    {
        let state = test.config.state_read();
        let assignment = state.energy_saving.get(&issi).expect("TLMC EG assignment should be tracked");
        assert_eq!(assignment.mode, 1);
        assert_eq!(assignment.frame, Some(3));
        assert_eq!(assignment.multiframe, Some(1));
        assert!(
            assignment.awake_until.is_some(),
            "assignment must keep MS awake until EG start/T.210 guard"
        );
    }

    submit_tlmc_configure_req(
        &mut test,
        TlmcConfigureReq {
            energy_economy_issi: Some(issi),
            energy_economy_group: Some(0),
            energy_economy_startpoint: None,
            ..tlmc_configure_req()
        },
    );
    test.run_stack(Some(1));
    assert!(
        !test.config.state_read().energy_saving.contains_key(&issi),
        "TLMC StayAlive group should clear UMAC EG assignment"
    );
}

#[test]
fn test_tlmc_energy_economy_ignores_incomplete_or_invalid_assignments() {
    debug::setup_logging_verbose();

    let issi = 1234;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![]);

    for prim in [
        TlmcConfigureReq {
            energy_economy_group: Some(1),
            energy_economy_startpoint: Some(TlmcEnergyEconomyStartpoint { frame: 3, multiframe: 1 }),
            ..tlmc_configure_req()
        },
        TlmcConfigureReq {
            energy_economy_issi: Some(issi),
            energy_economy_group: Some(1),
            energy_economy_startpoint: None,
            ..tlmc_configure_req()
        },
        TlmcConfigureReq {
            energy_economy_issi: Some(issi),
            energy_economy_group: Some(8),
            energy_economy_startpoint: Some(TlmcEnergyEconomyStartpoint { frame: 3, multiframe: 1 }),
            ..tlmc_configure_req()
        },
        TlmcConfigureReq {
            energy_economy_issi: Some(issi),
            energy_economy_group: Some(1),
            energy_economy_startpoint: Some(TlmcEnergyEconomyStartpoint { frame: 18, multiframe: 1 }),
            ..tlmc_configure_req()
        },
        TlmcConfigureReq {
            energy_economy_issi: Some(issi),
            energy_economy_group: Some(1),
            energy_economy_startpoint: Some(TlmcEnergyEconomyStartpoint { frame: 16, multiframe: 1 }),
            ..tlmc_configure_req()
        },
    ] {
        submit_tlmc_configure_req(&mut test, prim);
        test.run_stack(Some(1));
        assert!(
            !test.config.state_read().energy_saving.contains_key(&issi),
            "invalid TLMC EG assignment must not create scheduler state"
        );
    }
}

#[test]
fn test_tlmc_energy_economy_rejects_startpoint_with_unsupported_frame_18_receive_recurrence() {
    debug::setup_logging_verbose();

    for (group, frame) in [(1, 16), (2, 15), (3, 12), (4, 9)] {
        let issi = 0x4100 + group as u32;
        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { t: 1, f: 1, m: 1, h: 0 }));
        test.populate_entities(vec![TetraEntity::Umac], vec![]);

        assert!(
            EnergySavingAssignment::receive_cycle_uses_frame(group, frame, 1, 18),
            "test vector must recur on frame 18"
        );
        // EN 300 392-2 clauses 16.10.10 and 23.7.6 bind TLMC EG startpoint to
        // later receive frames. Until this BS supports the full frame-18
        // receive model, UMAC must reject recurrences that would require
        // sleeping MSs to rely on frame-18 grants.
        submit_tlmc_configure_req(
            &mut test,
            TlmcConfigureReq {
                energy_economy_issi: Some(issi),
                energy_economy_group: Some(group),
                energy_economy_startpoint: Some(TlmcEnergyEconomyStartpoint { frame, multiframe: 1 }),
                ..tlmc_configure_req()
            },
        );
        test.run_stack(Some(1));

        assert!(
            !test.config.state_read().energy_saving.contains_key(&issi),
            "EG{group} startpoint frame {frame} recurs on frame 18 and must be rejected"
        );
    }
}

fn mac_u_blck_pdu_with_event_label(event_label: u16, reservation_req: u8) -> BitBuffer {
    let mut pdu = BitBuffer::new_autoexpand(32);
    MacUBlck {
        fill_bits: false,
        encrypted: false,
        event_label,
        reservation_req,
    }
    .to_bitbuf(&mut pdu);
    pdu.seek(0);
    pdu
}

fn submit_mac_u_blck_with_event_label(test: &mut ComponentTest, event_label: u16, reservation_req: u8) {
    test.submit_message(SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            pdu: mac_u_blck_pdu_with_event_label(event_label, reservation_req),
            block_num: PhyBlockNum::Both,
            logical_channel: LogicalChannel::SchF,
            crc_pass: true,
            scrambling_code: 864282631,
            rssi_dbfs: f32::NAN,
        }),
    });
}

fn submit_mac_u_blck(test: &mut ComponentTest, reservation_req: u8) {
    submit_mac_u_blck_with_event_label(test, 17, reservation_req);
}

fn mac_access_pdu_with_reservation(issi: u32, reservation_req: ReservationRequirement) -> BitBuffer {
    let mut pdu = BitBuffer::new_autoexpand(40);
    MacAccess {
        fill_bits: false,
        encrypted: false,
        addr: Some(TetraAddress::issi(issi)),
        event_label: None,
        length_ind: None,
        frag_flag: Some(false),
        reservation_req: Some(reservation_req),
    }
    .to_bitbuf(&mut pdu);
    pdu.seek(0);
    pdu
}

fn submit_mac_access_with_reservation(test: &mut ComponentTest, issi: u32, reservation_req: ReservationRequirement) {
    test.submit_message(SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            pdu: mac_access_pdu_with_reservation(issi, reservation_req),
            block_num: PhyBlockNum::Block1,
            logical_channel: LogicalChannel::SchHu,
            crc_pass: true,
            scrambling_code: 864282631,
            rssi_dbfs: f32::NAN,
        }),
    });
}

fn private_call_open_msg(caller_issi: u32, called_issi: u32, ts: u8) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Open(Circuit {
            direction: Direction::Both,
            ts,
            peer_ts: None,
            usage: 4,
            circuit_mode: CircuitModeType::TchS,
            speech_service: Some(0),
            etee_encrypted: false,
            dl_media_source: CircuitDlMediaSource::LocalLoopback,
            active_addr: Some(TetraAddress::issi(caller_issi)),
            active_secondary_addrs: vec![TetraAddress::issi(called_issi)],
        })),
    }
}

fn floor_released_msg(call_id: u16, ts: u8) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id, ts }),
    }
}

fn floor_granted_msg(call_id: u16, source_issi: u32, dest_gssi: u32, ts: u8) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
            call_id,
            source_issi,
            dest_gssi,
            ts,
        }),
    }
}

fn d_tx_granted_sdu(call_id: u16, transmission_grant: TransmissionGrant) -> BitBuffer {
    let mut sdu = BitBuffer::new_autoexpand(40);
    DTxGranted {
        call_identifier: call_id,
        transmission_grant: transmission_grant.into_raw() as u8,
        transmission_request_permission: false,
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
    }
    .to_bitbuf(&mut sdu)
    .expect("serialize D-TX GRANTED");
    sdu.seek(0);
    sdu
}

fn d_tx_ceased_sdu(call_id: u16) -> BitBuffer {
    let mut sdu = BitBuffer::new_autoexpand(40);
    DTxCeased {
        call_identifier: call_id,
        transmission_request_permission: false,
        notification_indicator: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    }
    .to_bitbuf(&mut sdu)
    .expect("serialize D-TX CEASED");
    sdu.seek(0);
    sdu
}

fn private_caller_d_connect_sdu(call_id: u16, notification_indicator: Option<u64>) -> BitBuffer {
    let mut sdu = BitBuffer::new_autoexpand(64);
    DConnect {
        call_identifier: call_id,
        call_time_out: CallTimeout::T5m,
        hook_method_selection: false,
        simplex_duplex_selection: false,
        transmission_grant: TransmissionGrant::Granted,
        transmission_request_permission: false,
        call_ownership: false,
        call_priority: None,
        basic_service_information: None,
        temporary_address: None,
        notification_indicator,
        facility: None,
        proprietary: None,
    }
    .to_bitbuf(&mut sdu)
    .expect("serialize private caller D-CONNECT");
    sdu.seek(0);
    sdu
}

fn llc_wrapped_cmce_sdu(mut cmce_sdu: BitBuffer) -> BitBuffer {
    let mut sdu = BitBuffer::new_autoexpand(64);
    BlUdata { has_fcs: false }.to_bitbuf(&mut sdu);
    sdu.write_bits(MleProtocolDiscriminator::Cmce.into_raw(), 3);
    let cmce_sdu_len = cmce_sdu.get_len();
    sdu.copy_bits(&mut cmce_sdu, cmce_sdu_len);
    sdu.seek(0);
    sdu
}

fn llc_ack_wrapped_cmce_sdu(mut cmce_sdu: BitBuffer) -> BitBuffer {
    let mut sdu = BitBuffer::new_autoexpand(64);
    BlData { has_fcs: false, ns: 0 }.to_bitbuf(&mut sdu);
    sdu.write_bits(MleProtocolDiscriminator::Cmce.into_raw(), 3);
    let cmce_sdu_len = cmce_sdu.get_len();
    sdu.copy_bits(&mut cmce_sdu, cmce_sdu_len);
    sdu.seek(0);
    sdu
}

fn llc_wrapped_mle_sdu(payload_bits: usize) -> BitBuffer {
    let mut sdu = BitBuffer::new_autoexpand(16 + payload_bits);
    BlUdata { has_fcs: false }.to_bitbuf(&mut sdu);
    sdu.write_bits(MleProtocolDiscriminator::Mle.into_raw(), 3);
    sdu.write_zeroes(payload_bits);
    sdu.seek(0);
    sdu
}

fn private_call_open_msg_with_peer(active_issi: u32, peer_issi: u32, ts: u8, peer_ts: u8) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Open(Circuit {
            direction: Direction::Both,
            ts,
            peer_ts: Some(peer_ts),
            usage: 4,
            circuit_mode: CircuitModeType::TchS,
            speech_service: Some(0),
            etee_encrypted: false,
            dl_media_source: CircuitDlMediaSource::LocalLoopback,
            active_addr: Some(TetraAddress::issi(active_issi)),
            active_secondary_addrs: vec![TetraAddress::issi(peer_issi)],
        })),
    }
}

fn local_parrot_open_msg(caller_issi: u32, ts: u8) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Open(Circuit {
            direction: Direction::Both,
            ts,
            peer_ts: None,
            usage: 4,
            circuit_mode: CircuitModeType::TchS,
            speech_service: Some(0),
            etee_encrypted: false,
            dl_media_source: CircuitDlMediaSource::LocalParrot,
            active_addr: Some(TetraAddress::issi(caller_issi)),
            active_secondary_addrs: vec![TetraAddress::issi(99_999)],
        })),
    }
}

fn submit_ul_voice_frame(test: &mut ComponentTest, ts: u8, data: Vec<u8>) {
    test.submit_message(SapMsg {
        sap: Sap::TmdSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmdCircuitDataInd(TmdCircuitDataInd {
            ts,
            data,
            raw_tch_s_block: None,
        }),
    });
}

fn submit_ul_raw_tch_s_block2(test: &mut ComponentTest, ts: u8, data: Vec<u8>) {
    test.submit_message(SapMsg {
        sap: Sap::TmdSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmdCircuitDataInd(TmdCircuitDataInd {
            ts,
            data,
            raw_tch_s_block: Some(PhyBlockNum::Block2),
        }),
    });
}

fn submit_dl_tmd_req(test: &mut ComponentTest, ts: u8, data: Vec<u8>, raw_tch_s_block: Option<PhyBlockNum>) {
    test.submit_message(SapMsg {
        sap: Sap::TmdSap,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmdCircuitDataReq(TmdCircuitDataReq { ts, data, raw_tch_s_block }),
    });
}

fn acelp_test_bits() -> Vec<u8> {
    (0..274)
        .map(|idx| match idx % 7 {
            0 | 3 | 5 => 1,
            _ => 0,
        })
        .collect()
}

fn test_scrambling_code() -> u32 {
    scrambler::tetra_scramb_get_init(204, 1337, 1)
}

fn encoded_tch_s(codec_bits: &[u8], blk_num: u8) -> BitBuffer {
    errorcontrol::encode_tp(
        TmvUnitdataReq {
            mac_block: BitBuffer::from_bitarr(codec_bits),
            logical_channel: LogicalChannel::TchS,
            scrambling_code: test_scrambling_code(),
        },
        blk_num,
    )
    .expect("TCH/S test frame should encode")
}

fn build_uplink_tch_s_ind(train_type: TrainingSequence, block_num: PhyBlockNum, block: BitBuffer) -> SapMsg {
    SapMsg {
        sap: Sap::TpSap,
        src: TetraEntity::Phy,
        dest: TetraEntity::Lmac,
        msg: SapMsgInner::TpUnitdataInd(TpUnitdataInd {
            train_type,
            burst_type: BurstType::NUB,
            block_type: PhyBlockType::NUB,
            block_num,
            block,
            rssi_dbfs: f32::NAN,
        }),
    }
}

fn collect_dl_tch_bits(msgs: &[SapMsg], ts: u8) -> Vec<Vec<u8>> {
    msgs.iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmvUnitdataReq(prim) if prim.ts.t == ts => Some(prim),
            _ => None,
        })
        .flat_map(|prim| [prim.blk1.as_ref(), prim.blk2.as_ref()].into_iter().flatten())
        .filter(|blk| blk.logical_channel == LogicalChannel::TchS)
        .map(|blk| {
            let mut bits = vec![0u8; 274];
            let mut block = blk.mac_block.clone();
            block.seek(0);
            block.to_bitarr(&mut bits);
            bits
        })
        .collect()
}

fn assert_dl_tch_contains_bits(msgs: &[SapMsg], ts: u8, expected_bits: &[u8], context: &str) {
    let observed = collect_dl_tch_bits(msgs, ts);
    assert!(
        observed.iter().any(|bits| bits == expected_bits),
        "{context}: expected ACELP bit pattern on DL TCH/S ts {ts}, observed {} TCH/S blocks",
        observed.len()
    );
}

fn collect_dl_raw_tch_block2_bits(msgs: &[SapMsg], ts: u8) -> Vec<Vec<u8>> {
    msgs.iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmvUnitdataReq(prim) if prim.ts.t == ts => Some(prim),
            _ => None,
        })
        .filter(|prim| {
            prim.blk1.as_ref().is_some_and(|blk| blk.logical_channel == LogicalChannel::Stch)
                && prim
                    .blk2
                    .as_ref()
                    .is_some_and(|blk| blk.logical_channel == LogicalChannel::TchS && blk.mac_block.get_len() == 216)
        })
        .filter_map(|prim| prim.blk2.as_ref())
        .map(|blk| {
            let mut bits = vec![0u8; 216];
            let mut block = blk.mac_block.clone();
            block.seek(0);
            block.to_bitarr(&mut bits);
            bits
        })
        .collect()
}

fn mac_u_signal_pdu_for_test(second_half_stolen: bool) -> BitBuffer {
    let mut pdu = BitBuffer::new_autoexpand(124);
    MacUSignal { second_half_stolen }.to_bitbuf(&mut pdu);
    // Clause 21.4.5 fixes the TM-SDU in MAC-U-SIGNAL at 121 bits. The test
    // payload only needs to be syntactically present; LLC parsing is outside
    // this UMAC identity regression.
    pdu.write_ones(121);
    pdu.seek(0);
    pdu
}

fn mac_u_signal_bl_ack_pdu_for_test(nr: u8) -> BitBuffer {
    let mut pdu = BitBuffer::new_autoexpand(16);
    MacUSignal { second_half_stolen: false }.to_bitbuf(&mut pdu);
    BlAck { has_fcs: false, nr }.to_bitbuf(&mut pdu);
    pdu.seek(0);
    pdu
}

fn mac_u_signal_bl_adata_pdu_for_test(nr: u8, ns: u8) -> BitBuffer {
    let mut pdu = BitBuffer::new_autoexpand(32);
    MacUSignal { second_half_stolen: false }.to_bitbuf(&mut pdu);
    BlAdata { has_fcs: false, nr, ns }.to_bitbuf(&mut pdu);
    // A small payload proves pre-floor routing strips ambiguous TL-SDU data
    // instead of duplicating it under both candidate ISSIs.
    pdu.write_bits(0b10101, 5);
    pdu.seek(0);
    pdu
}

fn bl_ack_tma_sdu_for_test(nr: u8) -> BitBuffer {
    let mut pdu = BitBuffer::new_autoexpand(8);
    BlAck { has_fcs: false, nr }.to_bitbuf(&mut pdu);
    pdu.seek(0);
    pdu
}

fn submit_stch_mac_u_signal(test: &mut ComponentTest) {
    submit_stch_mac_u_signal_with_second_half(test, false);
}

fn submit_stch_mac_u_signal_with_second_half(test: &mut ComponentTest, second_half_stolen: bool) {
    test.submit_message(SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            pdu: mac_u_signal_pdu_for_test(second_half_stolen),
            block_num: PhyBlockNum::Block1,
            logical_channel: LogicalChannel::Stch,
            crc_pass: true,
            scrambling_code: 864282631,
            rssi_dbfs: f32::NAN,
        }),
    });
}

fn submit_stch_mac_u_signal_bl_ack(test: &mut ComponentTest, nr: u8) {
    test.submit_message(SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            pdu: mac_u_signal_bl_ack_pdu_for_test(nr),
            block_num: PhyBlockNum::Block1,
            logical_channel: LogicalChannel::Stch,
            crc_pass: true,
            scrambling_code: 864282631,
            rssi_dbfs: f32::NAN,
        }),
    });
}

fn submit_stch_mac_u_signal_bl_adata(test: &mut ComponentTest, nr: u8, ns: u8) {
    test.submit_message(SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            pdu: mac_u_signal_bl_adata_pdu_for_test(nr, ns),
            block_num: PhyBlockNum::Block1,
            logical_channel: LogicalChannel::Stch,
            crc_pass: true,
            scrambling_code: 864282631,
            rssi_dbfs: f32::NAN,
        }),
    });
}

fn tma_unitdata_ind_addresses(msgs: &[SapMsg]) -> Vec<TetraAddress> {
    msgs.iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmaUnitdataInd(prim) => Some(prim.main_address),
            _ => None,
        })
        .collect()
}

fn tma_unitdata_ind_pdu_types_and_lengths(msgs: &[SapMsg]) -> Vec<(LlcPduType, usize)> {
    msgs.iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmaUnitdataInd(prim) => {
                let pdu = prim.pdu.as_ref()?;
                let pdu_type = pdu.peek_bits(4).and_then(|bits| LlcPduType::try_from(bits).ok())?;
                Some((pdu_type, pdu.get_len()))
            }
            _ => None,
        })
        .collect()
}

fn has_lmac_blk2_stolen_configure(msgs: &[SapMsg]) -> bool {
    msgs.iter().any(|msg| {
        msg.dest == TetraEntity::Lmac
            && matches!(
                &msg.msg,
                SapMsgInner::TmvConfigureReq(prim) if prim.blk2_stolen == Some(true)
            )
    })
}

fn count_ul_inactivity_timeouts(msgs: &[SapMsg], ts: u8) -> usize {
    msgs.iter()
        .filter(|msg| {
            matches!(
                &msg.msg,
                SapMsgInner::CmceCallControl(CallControl::UlInactivityTimeout { ts: timeout_ts })
                    if *timeout_ts == ts
            )
        })
        .count()
}

fn reserve_current_uplink_for_mac_u_blck(test: &mut ComponentTest, start: TdmaTime, issi: u32) {
    let msg_dltime = start.add_timeslots(-2);
    let umac = test
        .router
        .get_entity(TetraEntity::Umac)
        .expect("UMAC entity should be registered")
        .as_any_mut()
        .downcast_mut::<UmacBs>()
        .expect("registered UMAC should be UmacBs");
    umac.channel_scheduler
        .ul_process_cap_req_from(
            msg_dltime,
            msg_dltime.t,
            TetraAddress::issi(issi),
            &ReservationRequirement::Req1Slot,
        )
        .expect("test setup should reserve the uplink slot that carries MAC-U-BLCK");
}

#[test]
fn test_group_ul_voice_loopback_preserves_tch_s_bits() {
    debug::setup_logging_verbose();

    let gssi = 0x1201;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg(gssi, traffic_ts));
    test.run_stack(Some(1));

    let ul_bits = acelp_test_bits();
    submit_ul_voice_frame(&mut test, traffic_ts, ul_bits.clone());
    test.run_stack(Some(12));

    let msgs = test.dump_sinks();
    assert_dl_tch_contains_bits(
        &msgs,
        traffic_ts,
        &ul_bits,
        "EN 300 392-2 clauses 14.5.2.1.3, 14.5.2.2.1 and 23.5: group-call UL speech must be reflected to the assigned DL TCH/S without bit corruption",
    );
}

#[test]
fn test_local_parrot_ul_acelp_forwards_to_cmce_without_dl_loopback() {
    debug::setup_logging_verbose();

    let caller_issi = 0x2201;
    let traffic_ts = 2;
    let call_id = 99;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Cmce, TetraEntity::Lmac]);

    test.submit_message(local_parrot_open_msg(caller_issi, traffic_ts));
    test.submit_message(floor_granted_msg(call_id, caller_issi, 99_999, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let ul_bits = acelp_test_bits();
    submit_ul_voice_frame(&mut test, traffic_ts, ul_bits.clone());
    test.run_stack(Some(12));

    let msgs = test.dump_sinks();
    let parrot_frames: Vec<_> = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmdCircuitDataInd(ind) if msg.dest == TetraEntity::Cmce => Some(ind),
            _ => None,
        })
        .collect();
    assert_eq!(parrot_frames.len(), 1);
    assert_eq!(parrot_frames[0].ts, traffic_ts);
    assert_eq!(parrot_frames[0].data, ul_bits);
    assert_eq!(parrot_frames[0].raw_tch_s_block, None);
    assert!(
        collect_dl_tch_bits(&msgs, traffic_ts).is_empty(),
        "LocalParrot must not immediately loop caller UL speech back to DL"
    );
}

#[test]
fn test_local_parrot_ul_raw_block2_forwards_to_cmce_without_dl_loopback() {
    debug::setup_logging_verbose();

    let caller_issi = 0x2201;
    let traffic_ts = 2;
    let call_id = 100;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Cmce, TetraEntity::Lmac]);

    test.submit_message(local_parrot_open_msg(caller_issi, traffic_ts));
    test.submit_message(floor_granted_msg(call_id, caller_issi, 99_999, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 5 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, traffic_ts, raw_block2.clone());
    test.run_stack(Some(12));

    let msgs = test.dump_sinks();
    let parrot_frames: Vec<_> = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmdCircuitDataInd(ind) if msg.dest == TetraEntity::Cmce => Some(ind),
            _ => None,
        })
        .collect();
    assert_eq!(parrot_frames.len(), 1);
    assert_eq!(parrot_frames[0].ts, traffic_ts);
    assert_eq!(parrot_frames[0].data, raw_block2);
    assert_eq!(parrot_frames[0].raw_tch_s_block, Some(PhyBlockNum::Block2));
    assert!(
        collect_dl_raw_tch_block2_bits(&msgs, traffic_ts).is_empty(),
        "LocalParrot must not immediately loop raw caller UL speech back to DL"
    );
}

#[test]
fn test_tmd_dl_req_acelp_parrot_playback_preserves_tch_s_bits() {
    debug::setup_logging_verbose();

    let caller_issi = 0x2201;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(local_parrot_open_msg(caller_issi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let acelp_bits = acelp_test_bits();
    submit_dl_tmd_req(&mut test, traffic_ts, acelp_bits.clone(), None);
    test.run_stack(Some(12));

    let msgs = test.dump_sinks();
    assert_dl_tch_contains_bits(
        &msgs,
        traffic_ts,
        &acelp_bits,
        "LocalParrot TmdCircuitDataReq playback must preserve complete ACELP TCH/S bits",
    );
}

#[test]
fn test_tmd_dl_req_raw_block2_playback_preserves_tch_s_halfslot() {
    debug::setup_logging_verbose();

    let caller_issi = 0x2201;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(local_parrot_open_msg(caller_issi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 7 + 1) % 2) as u8).collect();
    submit_dl_tmd_req(&mut test, traffic_ts, raw_block2.clone(), Some(PhyBlockNum::Block2));
    test.run_stack(Some(12));

    let observed = collect_dl_raw_tch_block2_bits(&test.dump_sinks(), traffic_ts);
    assert!(
        observed.iter().any(|bits| bits == &raw_block2),
        "TmdCircuitDataReq raw Block2 playback must preserve exact 216-bit TCH/S half-slot"
    );
}

#[test]
fn test_group_ul_raw_block2_loopback_preserves_tch_s_halfslot() {
    debug::setup_logging_verbose();

    let gssi = 0x1201;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg(gssi, traffic_ts));
    test.run_stack(Some(1));

    let raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 5 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, traffic_ts, raw_block2.clone());
    test.run_stack(Some(12));

    let msgs = test.dump_sinks();
    let observed = collect_dl_raw_tch_block2_bits(&msgs, traffic_ts);
    assert!(
        observed.iter().any(|bits| bits == &raw_block2),
        "EN 300 392-2 clauses 23.8.4.1.4 and 23.8.5 require group-call raw TCH/S Block2 to be preserved on downlink after STCH first half; observed {} candidates",
        observed.len()
    );
}

#[test]
fn test_group_ul_raw_block2_is_dropped_during_hangtime() {
    debug::setup_logging_verbose();

    let gssi = 0x1201;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg(gssi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(floor_released_msg(7, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 3 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, traffic_ts, raw_block2.clone());
    test.run_stack(Some(12));

    let msgs = test.dump_sinks();
    let observed = collect_dl_raw_tch_block2_bits(&msgs, traffic_ts);
    assert!(
        !observed.iter().any(|bits| bits == &raw_block2),
        "EN 300 392-2 clauses 14.5.2.2.1 and 14.5.2.4: U-plane media received after D-TX CEASED/floor release must not be looped during hangtime"
    );
}

#[test]
fn test_group_floor_release_purges_queued_raw_block2_media() {
    debug::setup_logging_verbose();

    let gssi = 0x1201;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg(gssi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let stale_raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 5 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, traffic_ts, stale_raw_block2.clone());
    test.submit_message(floor_released_msg(7, traffic_ts));
    test.run_stack(Some(12));

    let msgs = test.dump_sinks();
    let observed = collect_dl_raw_tch_block2_bits(&msgs, traffic_ts);
    assert!(
        !observed.iter().any(|bits| bits == &stale_raw_block2),
        "EN 300 392-2 clauses 14.5.2.2.1 and 14.5.2.4: queued old-speaker TCH/S must be purged when the floor is released"
    );
}

#[test]
fn test_group_floor_grant_purges_stale_raw_block2_but_allows_new_media() {
    debug::setup_logging_verbose();

    let gssi = 0x1201;
    let new_speaker = 0x2201;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg(gssi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let stale_raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 7 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, traffic_ts, stale_raw_block2.clone());
    test.submit_message(floor_granted_msg(7, new_speaker, gssi, traffic_ts));
    test.run_stack(Some(12));

    let stale_msgs = test.dump_sinks();
    let stale_observed = collect_dl_raw_tch_block2_bits(&stale_msgs, traffic_ts);
    assert!(
        !stale_observed.iter().any(|bits| bits == &stale_raw_block2),
        "EN 300 392-2 clauses 14.5.2.2.1 and 14.5.2.4: queued media from the previous floor epoch must not survive a new D-TX GRANTED"
    );

    let fresh_raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 11 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, traffic_ts, fresh_raw_block2.clone());
    test.run_stack(Some(12));

    let fresh_msgs = test.dump_sinks();
    let fresh_observed = collect_dl_raw_tch_block2_bits(&fresh_msgs, traffic_ts);
    assert!(
        fresh_observed.iter().any(|bits| bits == &fresh_raw_block2),
        "fresh media after the new floor grant should still be routed to DL TCH/S"
    );
}

#[test]
fn test_group_floor_grant_preserves_first_hangtime_requester_raw_block2() {
    debug::setup_logging_verbose();

    let gssi = 226333;
    let first_speaker = 2260616;
    let mtp3550_issi = 2260082;
    let call_id = 78;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg_with_secondary_speaker(gssi, first_speaker, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let first_raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 13 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, traffic_ts, first_raw_block2.clone());
    test.run_stack(Some(12));
    let first_observed = collect_dl_raw_tch_block2_bits(&test.dump_sinks(), traffic_ts);
    assert!(
        first_observed.iter().any(|bits| bits == &first_raw_block2),
        "test setup should prove the initial group speaker media is routed before hangtime"
    );

    test.submit_message(floor_released_msg(call_id, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let early_mtp3550_block2: Vec<u8> = (0..216).map(|idx| ((idx * 17 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, traffic_ts, early_mtp3550_block2.clone());
    test.submit_message(floor_granted_msg(call_id, mtp3550_issi, gssi, traffic_ts));
    test.run_stack(Some(12));

    let observed = collect_dl_raw_tch_block2_bits(&test.dump_sinks(), traffic_ts);
    assert!(
        observed.iter().any(|bits| bits == &early_mtp3550_block2),
        "EN 300 392-2 clauses 14.5.2.2.1, 23.5 and 23.8.5: first group TCH/S Block2 from ISSI 2260082 must survive the hangtime-to-D-TX-GRANTED transition; observed {} candidates",
        observed.len()
    );
}

#[test]
fn test_group_floor_handoff_reopens_ul_traffic_for_lmac_tch_s_decode() {
    debug::setup_logging_verbose();

    let gssi = 226333;
    let first_speaker = 2260616;
    let second_speaker = 2260082;
    let call_id = 77;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut umac_test = ComponentTest::new(StackMode::Bs, Some(start));
    umac_test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    umac_test.submit_message(group_call_open_msg_with_secondary_speaker(gssi, first_speaker, traffic_ts));
    umac_test.run_stack(Some(2));
    let _ = umac_test.dump_sinks();

    umac_test.submit_message(floor_released_msg(call_id, traffic_ts));
    umac_test.run_stack(Some(2));
    let _ = umac_test.dump_sinks();

    umac_test.submit_message(floor_granted_msg(call_id, second_speaker, gssi, traffic_ts));
    umac_test.run_stack(Some(10));
    let umac_msgs = umac_test.dump_sinks();

    let granted_traffic_slot = umac_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) if slot.ts.t == traffic_ts => Some(slot),
            _ => None,
        })
        .find(|slot| slot.ul_phy_chan == PhysicalChannel::Tp)
        .cloned()
        .expect("FloorGranted after hangtime must schedule the group UL timeslot as traffic/TP");

    assert_eq!(
        granted_traffic_slot.ul_phy_chan,
        PhysicalChannel::Tp,
        "EN 300 392-2 clauses 14.5.2.2.1 and 23.5: once D-TX GRANTED moves the floor, the assigned UL slot must be TP/TCH, not CP signalling"
    );

    let mut lmac_test = ComponentTest::new(StackMode::Bs, Some(granted_traffic_slot.ts.add_timeslots(2)));
    lmac_test.populate_entities(vec![TetraEntity::Lmac], vec![TetraEntity::Umac]);
    lmac_test.run_stack(Some(1));
    let _ = lmac_test.dump_sinks();

    lmac_test.submit_message(SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Lmac,
        msg: SapMsgInner::TmvUnitdataReq(granted_traffic_slot),
    });
    lmac_test.deliver_all_messages();
    let _ = lmac_test.dump_sinks();

    let codec_bits = acelp_test_bits();
    lmac_test.submit_message(build_uplink_tch_s_ind(
        TrainingSequence::NormalTrainSeq1,
        PhyBlockNum::Both,
        encoded_tch_s(&codec_bits, 1),
    ));
    lmac_test.deliver_all_messages();

    let lmac_msgs = lmac_test.dump_sinks();
    let traffic: Vec<_> = lmac_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmdCircuitDataInd(ind) => Some(ind),
            _ => None,
        })
        .collect();
    assert_eq!(
        traffic.len(),
        1,
        "valid TCH/S from the newly granted group speaker should reach UMAC once, not be treated as FACCH/SCH"
    );
    assert_eq!(traffic[0].ts, traffic_ts);
    assert_eq!(traffic[0].data, codec_bits);
    assert_eq!(
        traffic[0].raw_tch_s_block, None,
        "full-slot TCH/S speech should be decoded as a complete ACELP frame"
    );
    assert_ne!(first_speaker, second_speaker, "test setup must exercise a real speaker handoff");
}

#[test]
fn test_group_same_speaker_floor_retake_reopens_ul_traffic_for_lmac_tch_s_decode() {
    debug::setup_logging_verbose();

    let gssi = 226333;
    let speaker = 2260082;
    let call_id = 79;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut umac_test = ComponentTest::new(StackMode::Bs, Some(start));
    umac_test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    umac_test.submit_message(group_call_open_msg_with_secondary_speaker(gssi, speaker, traffic_ts));
    umac_test.run_stack(Some(2));
    let _ = umac_test.dump_sinks();

    umac_test.submit_message(floor_released_msg(call_id, traffic_ts));
    umac_test.run_stack(Some(2));
    let _ = umac_test.dump_sinks();

    umac_test.submit_message(floor_granted_msg(call_id, speaker, gssi, traffic_ts));
    umac_test.run_stack(Some(10));
    let umac_msgs = umac_test.dump_sinks();

    let granted_traffic_slot = umac_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) if slot.ts.t == traffic_ts => Some(slot),
            _ => None,
        })
        .find(|slot| slot.ul_phy_chan == PhysicalChannel::Tp)
        .cloned()
        .expect("same-speaker FloorGranted after hangtime must schedule the group UL timeslot as traffic/TP");

    let mut lmac_test = ComponentTest::new(StackMode::Bs, Some(granted_traffic_slot.ts.add_timeslots(2)));
    lmac_test.populate_entities(vec![TetraEntity::Lmac], vec![TetraEntity::Umac]);
    lmac_test.run_stack(Some(1));
    let _ = lmac_test.dump_sinks();

    lmac_test.submit_message(SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Lmac,
        msg: SapMsgInner::TmvUnitdataReq(granted_traffic_slot),
    });
    lmac_test.deliver_all_messages();
    let _ = lmac_test.dump_sinks();

    let codec_bits = acelp_test_bits();
    lmac_test.submit_message(build_uplink_tch_s_ind(
        TrainingSequence::NormalTrainSeq1,
        PhyBlockNum::Both,
        encoded_tch_s(&codec_bits, 1),
    ));
    lmac_test.deliver_all_messages();

    let lmac_msgs = lmac_test.dump_sinks();
    let traffic: Vec<_> = lmac_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmdCircuitDataInd(ind) => Some(ind),
            _ => None,
        })
        .collect();
    assert_eq!(
        traffic.len(),
        1,
        "same-speaker group retake must leave LMAC in traffic mode so valid TCH/S reaches UMAC"
    );
    assert_eq!(traffic[0].ts, traffic_ts);
    assert_eq!(traffic[0].data, codec_bits);
    assert_eq!(traffic[0].raw_tch_s_block, None);
}

#[test]
fn test_private_simplex_ul_voice_loopback_preserves_tch_s_bits() {
    debug::setup_logging_verbose();

    let caller_issi = 0x3201;
    let called_issi = 0x3202;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(private_call_open_msg(caller_issi, called_issi, traffic_ts));
    test.submit_message(floor_granted_msg(1, caller_issi, 0, traffic_ts));
    test.run_stack(Some(1));

    let ul_bits = acelp_test_bits();
    submit_ul_voice_frame(&mut test, traffic_ts, ul_bits.clone());
    test.run_stack(Some(12));

    let msgs = test.dump_sinks();
    assert_dl_tch_contains_bits(
        &msgs,
        traffic_ts,
        &ul_bits,
        "EN 300 392-2 clauses 14.5.1.2.1 and 23.5: private simplex speech on an assigned channel must remain a valid TCH/S frame after UMAC loopback",
    );
}

#[test]
fn test_private_simplex_pre_floor_voice_waits_for_floor_granted() {
    debug::setup_logging_verbose();

    let caller_issi = 0x3231;
    let called_issi = 0x3232;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(private_call_open_msg(caller_issi, called_issi, traffic_ts));
    test.run_stack(Some(1));

    let ul_bits = acelp_test_bits();
    submit_ul_voice_frame(&mut test, traffic_ts, ul_bits.clone());
    test.run_stack(Some(2));

    let pre_floor_msgs = test.dump_sinks();
    let pre_floor_tch = collect_dl_tch_bits(&pre_floor_msgs, traffic_ts);
    assert!(
        pre_floor_tch.iter().all(|bits| bits != &ul_bits),
        "EN 300 392-2 Annex D.4 and clause 14.5.1.2.1: private-simplex Open creates the bearer, but must not route first TCH/S before CMCE FloorGranted identifies the authorized speaker"
    );

    test.submit_message(floor_granted_msg(1, caller_issi, called_issi, traffic_ts));
    test.run_stack(Some(12));

    let post_floor_msgs = test.dump_sinks();
    assert_dl_tch_contains_bits(
        &post_floor_msgs,
        traffic_ts,
        &ul_bits,
        "EN 300 392-2 Annex D.4 plus clauses 14.5.1.2.1 and 23.5.2.2.1: after called-leg L2 ACK lets CMCE issue FloorGranted, the retained first private-simplex TCH/S burst must route",
    );
}

#[test]
fn test_private_simplex_cross_route_floor_release_purges_peer_dl_media() {
    debug::setup_logging_verbose();

    let caller_issi = 0x3201;
    let called_issi = 0x3202;
    let caller_ts = 2;
    let called_ts = 3;
    let call_id = 1;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(private_call_open_msg_with_peer(caller_issi, called_issi, caller_ts, called_ts));
    test.submit_message(private_call_open_msg_with_peer(called_issi, caller_issi, called_ts, caller_ts));
    test.submit_message(floor_granted_msg(call_id, caller_issi, called_issi, caller_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let stale_raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 7 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, caller_ts, stale_raw_block2.clone());
    test.submit_message(floor_released_msg(call_id, caller_ts));
    test.run_stack(Some(12));

    let msgs = test.dump_sinks();
    let observed = collect_dl_raw_tch_block2_bits(&msgs, called_ts);
    assert!(
        !observed.iter().any(|bits| bits == &stale_raw_block2),
        "EN 300 392-2 clauses 14.5.1.2.1 e), 23.5 and 23.8.5: private simplex floor release must purge queued old-speaker TCH/S on the crossed peer DL timeslot"
    );
}

#[test]
fn test_private_simplex_cross_route_floor_grant_keeps_new_peer_audio() {
    debug::setup_logging_verbose();

    let caller_issi = 0x3211;
    let called_issi = 0x3212;
    let caller_ts = 2;
    let called_ts = 3;
    let call_id = 1;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(private_call_open_msg_with_peer(caller_issi, called_issi, caller_ts, called_ts));
    test.submit_message(private_call_open_msg_with_peer(called_issi, caller_issi, called_ts, caller_ts));
    test.submit_message(floor_granted_msg(call_id, called_issi, caller_issi, called_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let fresh_raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 11 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, called_ts, fresh_raw_block2.clone());
    test.run_stack(Some(12));

    let msgs = test.dump_sinks();
    let observed = collect_dl_raw_tch_block2_bits(&msgs, caller_ts);
    assert!(
        observed.iter().any(|bits| bits == &fresh_raw_block2),
        "EN 300 392-2 clauses 14.5.1.2.1 b) and 23.5: private simplex P2P on separate assigned timeslots must cross-route the granted speaker's TCH/S to the peer DL timeslot"
    );
}

#[test]
fn test_private_simplex_floor_grant_preserves_first_requester_raw_block2() {
    debug::setup_logging_verbose();

    let hytera_issi = 2_260_616;
    let mxp600_issi = 2_260_618;
    let hytera_ts = 2;
    let mxp600_ts = 3;
    let call_id = 55;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(private_call_open_msg_with_peer(hytera_issi, mxp600_issi, hytera_ts, mxp600_ts));
    test.submit_message(private_call_open_msg_with_peer(mxp600_issi, hytera_issi, mxp600_ts, hytera_ts));
    test.submit_message(floor_granted_msg(call_id, mxp600_issi, hytera_issi, mxp600_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let first_raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 13 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, hytera_ts, first_raw_block2.clone());
    test.submit_message(floor_granted_msg(call_id, hytera_issi, mxp600_issi, hytera_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.run_stack(Some(12));
    let msgs = test.dump_sinks();
    let observed = collect_dl_raw_tch_block2_bits(&msgs, mxp600_ts);
    assert!(
        observed.iter().any(|bits| bits == &first_raw_block2),
        "EN 300 392-2 clauses 14.5.1.2.1 b), 23.5 and 23.8.5: a private-simplex FloorGranted must not purge the first valid requester TCH/S Block2 while clearing stale previous-speaker media; observed {} candidates",
        observed.len()
    );
}

#[test]
fn test_private_simplex_shared_ts_floor_grant_preserves_first_requester_raw_block2() {
    debug::setup_logging_verbose();

    let hytera_issi = 2_260_616;
    let mxp600_issi = 2_260_618;
    let traffic_ts = 2;
    let call_id = 56;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(private_call_open_msg(hytera_issi, mxp600_issi, traffic_ts));
    test.submit_message(floor_granted_msg(call_id, mxp600_issi, hytera_issi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let first_raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 17 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, traffic_ts, first_raw_block2.clone());
    test.submit_message(floor_granted_msg(call_id, hytera_issi, mxp600_issi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.run_stack(Some(12));
    let observed = collect_dl_raw_tch_block2_bits(&test.dump_sinks(), traffic_ts);
    assert!(
        observed.iter().any(|bits| bits == &first_raw_block2),
        "EN 300 392-2 clauses 14.5.1.2.1 b), 14.5.1.4.2, 23.5 and 23.8.5: private simplex on one shared assigned timeslot must not tag the first requester TCH/S Block2 with the previous floor holder and purge it during D-TX GRANTED; observed {} candidates",
        observed.len()
    );
}

#[test]
fn test_private_simplex_same_speaker_raw_block2_reentry_survives_hangtime() {
    debug::setup_logging_verbose();

    let mtp3550_issi = 2_260_082;
    let mxp600_issi = 2_260_618;
    let traffic_ts = 2;
    let call_id = 57;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(private_call_open_msg(mtp3550_issi, mxp600_issi, traffic_ts));
    test.submit_message(floor_granted_msg(call_id, mtp3550_issi, mxp600_issi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let first_raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 19 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, traffic_ts, first_raw_block2.clone());
    test.run_stack(Some(12));
    let first_observed = collect_dl_raw_tch_block2_bits(&test.dump_sinks(), traffic_ts);
    assert!(
        first_observed.iter().any(|bits| bits == &first_raw_block2),
        "test setup should prove the initial Motorola private-simplex speaker media is routed before hangtime"
    );

    test.submit_message(floor_released_msg(call_id, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let reentry_raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 23 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, traffic_ts, reentry_raw_block2.clone());
    test.submit_message(floor_granted_msg(call_id, mtp3550_issi, mxp600_issi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.run_stack(Some(12));
    let observed = collect_dl_raw_tch_block2_bits(&test.dump_sinks(), traffic_ts);
    assert!(
        observed.iter().any(|bits| bits == &reentry_raw_block2),
        "EN 300 392-2 clauses 14.5.1.2.1 b), 14.5.1.4.2, 23.5 and 23.8.5: private-simplex raw TCH/S Block2 from the same ISSI must survive the hangtime-to-D-TX-GRANTED transition; observed {} candidates",
        observed.len()
    );
}

#[test]
fn test_private_simplex_raw_block2_waits_for_delayed_floor_grant() {
    debug::setup_logging_verbose();

    let mtp3550_issi = 2_260_082;
    let mxp600_issi = 2_260_618;
    let traffic_ts = 2;
    let call_id = 58;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(private_call_open_msg(mtp3550_issi, mxp600_issi, traffic_ts));
    test.submit_message(floor_granted_msg(call_id, mtp3550_issi, mxp600_issi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(floor_released_msg(call_id, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let early_raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 29 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, traffic_ts, early_raw_block2.clone());
    test.run_stack(Some(2));
    let _ = test.dump_sinks();

    test.submit_message(floor_granted_msg(call_id, mtp3550_issi, mxp600_issi, traffic_ts));
    test.run_stack(Some(12));
    let observed = collect_dl_raw_tch_block2_bits(&test.dump_sinks(), traffic_ts);
    assert!(
        observed.iter().any(|bits| bits == &early_raw_block2),
        "EN 300 392-2 clauses 14.5.1.2.1 b), 14.5.1.4.2, 23.5 and 23.8.5: private-simplex raw TCH/S received just before the internal FloorGranted must be retained briefly and routed after the grant; observed {} candidates",
        observed.len()
    );
}

#[test]
fn test_private_simplex_acelp_waits_for_delayed_floor_grant() {
    debug::setup_logging_verbose();

    let hytera_issi = 2_260_616;
    let mxp600_issi = 2_260_618;
    let traffic_ts = 2;
    let call_id = 59;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(private_call_open_msg(hytera_issi, mxp600_issi, traffic_ts));
    test.submit_message(floor_granted_msg(call_id, hytera_issi, mxp600_issi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(floor_released_msg(call_id, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let early_acelp = acelp_test_bits();
    submit_ul_voice_frame(&mut test, traffic_ts, early_acelp.clone());
    test.run_stack(Some(2));
    let _ = test.dump_sinks();

    test.submit_message(floor_granted_msg(call_id, hytera_issi, mxp600_issi, traffic_ts));
    test.run_stack(Some(12));
    let msgs = test.dump_sinks();
    assert_dl_tch_contains_bits(
        &msgs,
        traffic_ts,
        &early_acelp,
        "EN 300 392-2 clauses 14.5.1.2.1 b), 14.5.1.4.2, 23.5 and 23.8.5: private-simplex ACELP TCH/S received just before the internal FloorGranted must be retained briefly and routed after the grant",
    );
}

#[test]
fn test_private_simplex_deferred_media_expires_without_floor_grant() {
    debug::setup_logging_verbose();

    let caller_issi = 2_260_616;
    let called_issi = 2_260_618;
    let traffic_ts = 2;
    let call_id = 60;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(private_call_open_msg(caller_issi, called_issi, traffic_ts));
    test.submit_message(floor_granted_msg(call_id, caller_issi, called_issi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(floor_released_msg(call_id, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let stale_raw_block2: Vec<u8> = (0..216).map(|idx| ((idx * 31 + 1) % 2) as u8).collect();
    submit_ul_raw_tch_s_block2(&mut test, traffic_ts, stale_raw_block2.clone());
    test.run_stack(Some(24));
    let _ = test.dump_sinks();

    test.submit_message(floor_granted_msg(call_id, caller_issi, called_issi, traffic_ts));
    test.run_stack(Some(12));
    let observed = collect_dl_raw_tch_block2_bits(&test.dump_sinks(), traffic_ts);
    assert!(
        observed.iter().all(|bits| bits != &stale_raw_block2),
        "EN 300 392-2 clauses 14.5.1.2.1 e) and 14.5.1.4.2: stale private TCH/S must not be replayed after the bounded hangtime guard expires"
    );
}

#[test]
fn test_unsupported_ul_voice_does_not_refresh_inactivity_timer() {
    debug::setup_logging_verbose();

    let caller_issi = 0x3221;
    let called_issi = 0x3222;
    let traffic_ts = 2;
    let call_id = 1;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.ul_inactivity_secs = 1;
    let mut test = ComponentTest::from_config(config, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Cmce, TetraEntity::Lmac]);

    test.submit_message(private_call_open_msg(caller_issi, called_issi, traffic_ts));
    test.submit_message(floor_granted_msg(call_id, caller_issi, called_issi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.run_stack(Some(60));
    let _ = test.dump_sinks();

    submit_ul_voice_frame(&mut test, traffic_ts, vec![1; 13]);
    test.run_stack(Some(20));

    let msgs = test.dump_sinks();
    assert_eq!(
        count_ul_inactivity_timeouts(&msgs, traffic_ts),
        1,
        "EN 300 392-2 clauses 14.5.1.2.1 and 23.8.3/23.8.5: unsupported TCH/S must not refresh the BS-side UL voice timer for a simplex private floor holder"
    );
    assert!(
        collect_dl_tch_bits(&msgs, traffic_ts).is_empty(),
        "unsupported UL voice must not be emitted as clean downlink TCH/S"
    );
}

#[test]
fn test_private_duplex_ul_voice_cross_route_preserves_tch_s_bits() {
    debug::setup_logging_verbose();

    let first_issi = 0x3211;
    let second_issi = 0x3212;
    let first_ts = 2;
    let second_ts = 3;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(private_call_open_msg_with_peer(first_issi, second_issi, first_ts, second_ts));
    test.submit_message(private_call_open_msg_with_peer(second_issi, first_issi, second_ts, first_ts));
    test.run_stack(Some(1));

    let ul_bits = acelp_test_bits();
    submit_ul_voice_frame(&mut test, second_ts, ul_bits.clone());
    test.run_stack(Some(12));

    let msgs = test.dump_sinks();
    assert_dl_tch_contains_bits(
        &msgs,
        first_ts,
        &ul_bits,
        "EN 300 392-2 clauses 14.5.1.2.1 and 23.5: duplex private-call UL speech must be cross-routed to the peer DL TCH/S without bit corruption",
    );
    assert!(
        !collect_dl_tch_bits(&msgs, second_ts).iter().any(|bits| bits == &ul_bits),
        "duplex private-call UL speech from ts {second_ts} must not be looped back to the transmitting party"
    );
}

#[test]
fn test_stch_mac_u_signal_waits_for_private_floor_granted() {
    debug::setup_logging_verbose();

    let caller_issi = 0x3101;
    let called_issi = 0x3102;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    test.submit_message(private_call_open_msg(caller_issi, called_issi, traffic_ts));
    submit_stch_mac_u_signal(&mut test);
    test.run_stack(Some(1));

    let addresses = tma_unitdata_ind_addresses(&test.dump_sinks());
    assert!(
        addresses.is_empty(),
        "EN 300 392-2 clauses 14.5.1.2.1 and 14.5.1.4: private-simplex Open only establishes the bearer; FloorGranted authorizes the U-plane speaker"
    );

    let mut granted_test = ComponentTest::new(StackMode::Bs, Some(start));
    granted_test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);
    granted_test.submit_message(private_call_open_msg(caller_issi, called_issi, traffic_ts));
    granted_test.submit_message(floor_granted_msg(1, caller_issi, called_issi, traffic_ts));
    submit_stch_mac_u_signal(&mut granted_test);
    granted_test.run_stack(Some(1));

    let addresses = tma_unitdata_ind_addresses(&granted_test.dump_sinks());
    assert_eq!(
        addresses,
        vec![TetraAddress::issi(caller_issi)],
        "EN 300 392-2 clauses 21.4.5 and 14.5.1.2.1 require STCH U-plane signalling to inherit the FloorGranted private-call speaker, not ISSI 0"
    );
}

#[test]
fn test_stch_bl_ack_before_private_floor_granted_routes_to_private_participants() {
    debug::setup_logging_verbose();

    let caller_issi = 2_260_616;
    let called_issi = 2_260_618;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    // CMCE seeds the called ISSI as primary on same-timeslot private simplex
    // during setup, but MAC-U-SIGNAL/STCH BL-ACK has no address field. Some
    // radios send the caller D-CONNECT BL-ACK on the assigned channel before
    // FloorGranted; route pure ACKs to both participants and let LLC match the
    // pending acknowledged transfer by SSI/N(S).
    test.submit_message(private_call_open_msg(called_issi, caller_issi, traffic_ts));
    submit_stch_mac_u_signal_bl_ack(&mut test, 1);
    test.run_stack(Some(1));

    let addresses = tma_unitdata_ind_addresses(&test.dump_sinks());
    assert_eq!(
        addresses,
        vec![TetraAddress::issi(called_issi), TetraAddress::issi(caller_issi)],
        "EN 300 392-2 Annex D.4 and clauses 21.4.5/22.3.2.3: addressless pre-floor private BL-ACK on assigned-channel STCH must reach LLC for both candidate ISSI links before FloorGranted"
    );
}

#[test]
fn test_stch_bl_adata_before_private_floor_granted_routes_ack_only_to_private_participants() {
    debug::setup_logging_verbose();

    let caller_issi = 2_260_082;
    let called_issi = 2_260_618;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    test.submit_message(private_call_open_msg(called_issi, caller_issi, traffic_ts));
    submit_stch_mac_u_signal_bl_adata(&mut test, 1, 0);
    test.run_stack(Some(1));

    let msgs = test.dump_sinks();
    let addresses = tma_unitdata_ind_addresses(&msgs);
    assert_eq!(
        addresses,
        vec![TetraAddress::issi(called_issi), TetraAddress::issi(caller_issi)],
        "EN 300 392-2 Annex D.4 and clauses 21.4.5/22.3.2.3: addressless pre-floor private BL-ADATA must expose its ACK to both candidate ISSI links before FloorGranted"
    );
    assert_eq!(
        tma_unitdata_ind_pdu_types_and_lengths(&msgs),
        vec![(LlcPduType::BlAck, 5), (LlcPduType::BlAck, 5)],
        "pre-floor BL-ADATA payload is sender-ambiguous on STCH, so UMAC must strip it to ACK-only copies instead of duplicating TL-SDU data under both ISSIs"
    );
}

#[test]
fn test_private_simplex_open_replacement_clears_stale_ul_speaker_before_floor() {
    debug::setup_logging_verbose();

    let old_caller_issi = 0x4101;
    let old_called_issi = 0x4102;
    let new_called_issi = 0x4202;
    let new_caller_issi = 0x4201;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    test.submit_message(private_call_open_msg(old_caller_issi, old_called_issi, traffic_ts));
    test.submit_message(floor_granted_msg(1, old_caller_issi, old_called_issi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // New Annex D.4 private setup replaces the same assigned channel. Before
    // CMCE sends FloorGranted for the new call, ordinary addressless STCH must
    // not inherit the stale speaker from the old call.
    test.submit_message(private_call_open_msg(new_called_issi, new_caller_issi, traffic_ts));
    submit_stch_mac_u_signal(&mut test);
    test.run_stack(Some(1));

    let addresses = tma_unitdata_ind_addresses(&test.dump_sinks());
    assert!(
        addresses.is_empty(),
        "EN 300 392-2 clauses 14.5.1.2.1 and 21.4.5: a replaced private-simplex bearer must clear stale UL speaker state until the new FloorGranted arrives"
    );
}

#[test]
fn test_stch_mac_u_signal_uses_secondary_speaker_from_group_open_circuit() {
    debug::setup_logging_verbose();

    let gssi = 0x3100;
    let first_speaker = 0x3101;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    test.submit_message(group_call_open_msg_with_secondary_speaker(gssi, first_speaker, traffic_ts));
    submit_stch_mac_u_signal(&mut test);
    test.run_stack(Some(1));

    let addresses = tma_unitdata_ind_addresses(&test.dump_sinks());
    assert_eq!(
        addresses,
        vec![TetraAddress::issi(first_speaker)],
        "EN 300 392-2 clauses 14.5.2.1, 14.5.2.2.1 and 21.4.5: group Open is GSSI-scoped, but the first speaker ISSI carried as secondary must seed STCH signalling before a later FloorGranted"
    );
}

#[test]
fn test_stch_mac_u_signal_tracks_floor_granted_handoff() {
    debug::setup_logging_verbose();

    let caller_issi = 0x3111;
    let called_issi = 0x3112;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    test.submit_message(private_call_open_msg(caller_issi, called_issi, traffic_ts));
    test.submit_message(floor_granted_msg(1, called_issi, 0, traffic_ts));
    submit_stch_mac_u_signal(&mut test);
    test.run_stack(Some(1));

    let addresses = tma_unitdata_ind_addresses(&test.dump_sinks());
    assert_eq!(
        addresses,
        vec![TetraAddress::issi(called_issi)],
        "UMAC must follow CMCE floor-control state so later U-TX DEMAND/U-TX CEASED signalling is attributed to the granted MS"
    );
}

#[test]
fn test_group_floor_grant_accepts_new_speaker_when_initial_speaker_is_secondary() {
    debug::setup_logging_verbose();

    let gssi = 0x3110;
    let first_speaker = 0x3111;
    let second_speaker = 0x3112;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    test.submit_message(group_call_open_msg_with_secondary_speaker(gssi, first_speaker, traffic_ts));
    test.submit_message(floor_granted_msg(1, second_speaker, gssi, traffic_ts));
    submit_stch_mac_u_signal(&mut test);
    test.run_stack(Some(1));

    let addresses = tma_unitdata_ind_addresses(&test.dump_sinks());
    assert_eq!(
        addresses,
        vec![TetraAddress::issi(second_speaker)],
        "EN 300 392-2 clauses 14.5.2.2.1 and 21.4.5: a group circuit is GSSI-scoped even when the initial speaker ISSI is tracked as secondary, so a later group FloorGranted must not be rejected as a private-call non-participant"
    );
}

#[test]
fn test_stch_mac_u_signal_ignores_floor_granted_for_non_participant_private_speaker() {
    debug::setup_logging_verbose();

    let caller_issi = 0x3121;
    let called_issi = 0x3122;
    let attacker_issi = 0x3123;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    test.submit_message(private_call_open_msg(caller_issi, called_issi, traffic_ts));
    test.submit_message(floor_granted_msg(1, attacker_issi, 0, traffic_ts));
    submit_stch_mac_u_signal(&mut test);
    test.run_stack(Some(1));

    let addresses = tma_unitdata_ind_addresses(&test.dump_sinks());
    assert!(
        addresses.is_empty(),
        "EN 300 392-2 clauses 14.5.1.2.1 and 21.4.5: STCH private-call signalling has no SSI field, so an invalid non-participant FloorGranted must not create or rewrite a private speaker"
    );
}

#[test]
fn test_stch_mac_u_signal_without_current_speaker_is_dropped() {
    debug::setup_logging_verbose();

    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    submit_stch_mac_u_signal(&mut test);
    test.run_stack(Some(1));

    assert!(
        tma_unitdata_ind_addresses(&test.dump_sinks()).is_empty(),
        "MAC-U-SIGNAL on STCH has no SSI field; without an active ISSI floor holder UMAC must drop instead of fabricating ISSI 0"
    );
}

#[test]
fn test_stch_mac_u_signal_second_half_stolen_forwards_first_half_and_marks_lmac_blk2_stolen() {
    debug::setup_logging_verbose();

    let caller_issi = 0x3131;
    let called_issi = 0x3132;
    let traffic_ts = 2;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc, TetraEntity::Lmac]);

    test.submit_message(private_call_open_msg(caller_issi, called_issi, traffic_ts));
    test.submit_message(floor_granted_msg(1, caller_issi, called_issi, traffic_ts));
    submit_stch_mac_u_signal_with_second_half(&mut test, true);
    test.run_stack(Some(1));

    let msgs = test.dump_sinks();
    assert_eq!(
        tma_unitdata_ind_addresses(&msgs),
        vec![TetraAddress::issi(caller_issi)],
        "EN 300 392-2 clause 21.4.5: first-half MAC-U-SIGNAL still carries a 121-bit TM-SDU even when the second half is stolen"
    );
    assert!(
        has_lmac_blk2_stolen_configure(&msgs),
        "EN 300 392-2 clauses 21.4.5 and 23.8.4.2.2: UMAC must tell LMAC to decode the second half as STCH, not TCH"
    );
}

#[test]
fn test_mac_u_blck_is_dropped_without_panic_until_tmsdu_delivery_is_implemented() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default().add_timeslots(2)));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc, TetraEntity::Mle]);

    // EN 300 392-2 clause 21.4.2.5 allows MAC-U-BLCK on SCH/F with an
    // implicit TM-SDU. Until the UMAC model carries that TM-SDU and maps the
    // event label, the BS should drop it explicitly rather than aborting.
    submit_mac_u_blck(&mut test, 1);
    test.run_stack(Some(1));

    assert!(
        test.dump_sinks().is_empty(),
        "MAC-U-BLCK TM-SDU delivery is intentionally not emitted until event-label handling is implemented"
    );
}

#[test]
fn test_mac_u_blck_reservation_requirement_enqueues_grant_for_known_slot_owner() {
    debug::setup_logging_verbose();

    let start = TdmaTime::default().add_timeslots(2);
    let issi = 0x3021;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);
    reserve_current_uplink_for_mac_u_blck(&mut test, start, issi);

    // EN 300 392-2 clauses 21.4.2.5 and 21.5.4/table 21.94 make the
    // MAC-U-BLCK reservation field mandatory; value 0001 means one slot.
    submit_mac_u_blck(&mut test, 1);
    test.run_stack(Some(4));

    let sink_msgs = test.dump_sinks();
    let grant = first_mac_resource_for_addr(&sink_msgs, TetraAddress::issi(issi))
        .expect("MAC-U-BLCK reservation should enqueue a grant for the slot owner");
    let slot_grant = grant
        .slot_granting_element
        .expect("MAC-U-BLCK reservation should be represented as a slot grant");
    assert_eq!(slot_grant.capacity_allocation, BasicSlotgrantCapAlloc::Grant1Slot);
    assert!(
        !sink_msgs.iter().any(|msg| matches!(msg.msg, SapMsgInner::TmaUnitdataInd(_))),
        "MAC-U-BLCK TM-SDU payload remains blocked until event-label delivery is implemented"
    );
}

#[test]
fn test_stale_frame_18_energy_saving_assignment_does_not_starve_private_reservation_grant() {
    debug::setup_logging_verbose();

    let start = TdmaTime::default().add_timeslots(2);
    let caller_issi = 0x3031;
    let called_issi = 0x3032;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        state.energy_saving.insert(
            caller_issi,
            EnergySavingAssignment {
                mode: 5,
                frame: Some(18),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
        state.energy_saving.insert(
            called_issi,
            EnergySavingAssignment {
                mode: 2,
                frame: Some(15),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);

    test.submit_message(private_call_open_msg(caller_issi, called_issi, 2));
    test.deliver_all_messages();
    {
        let state = test.config.state_read();
        for issi in [caller_issi, called_issi] {
            let assignment = state
                .energy_saving
                .get(&issi)
                .expect("test should inject a stale/external EG assignment");
            assert!(
                !assignment.is_energy_economy(),
                "stale/external frame-18 EG assignment must fail open before scheduling"
            );
        }
    }

    reserve_current_uplink_for_mac_u_blck(&mut test, start, caller_issi);

    // EN 300 392-2 clauses 21.4.6.5, 21.4.7.2, 23.5.2.2.7 and 23.7.6/table
    // 23.9: UMAC may gate downlink grants only for valid EG listen cycles.
    // Stale/external assignments that require unsupported frame-18 receive
    // must behave as StayAlive so a private-call participant can still receive
    // the next assigned-channel reservation grant.
    submit_mac_u_blck(&mut test, 1);
    test.run_stack(Some(4));

    let sink_msgs = test.dump_sinks();
    let grant = first_mac_resource_for_addr(&sink_msgs, TetraAddress::issi(caller_issi))
        .expect("private-call participant should receive reservation grant despite stale frame-18 EG state");
    let slot_grant = grant
        .slot_granting_element
        .expect("MAC-U-BLCK reservation should be represented as a slot grant");
    assert_eq!(slot_grant.capacity_allocation, BasicSlotgrantCapAlloc::Grant1Slot);
    assert!(
        !sink_msgs.iter().any(|msg| matches!(msg.msg, SapMsgInner::TmaUnitdataInd(_))),
        "MAC-U-BLCK TM-SDU payload remains blocked until event-label delivery is implemented"
    );
}

#[test]
fn test_mac_u_blck_reserved_event_labels_are_ignored() {
    debug::setup_logging_verbose();

    for event_label in [0, 0x03ff] {
        let start = TdmaTime::default().add_timeslots(2);
        let issi = 0x3120 + event_label as u32;
        let mut test = ComponentTest::new(StackMode::Bs, Some(start));
        test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);
        reserve_current_uplink_for_mac_u_blck(&mut test, start, issi);

        // EN 300 392-2 clause 23.4.1.2.3.2/.3 says event label all-zero and
        // all-ones are not valid in MAC-U-BLCK; the BS shall ignore the PDU.
        submit_mac_u_blck_with_event_label(&mut test, event_label, 1);
        test.run_stack(Some(4));

        let sink_msgs = test.dump_sinks();
        assert!(
            first_mac_resource_for_addr(&sink_msgs, TetraAddress::issi(issi)).is_none(),
            "reserved MAC-U-BLCK event label {event_label} must not enqueue a slot grant"
        );
        assert!(
            !sink_msgs.iter().any(|msg| matches!(msg.msg, SapMsgInner::TmaUnitdataInd(_))),
            "reserved MAC-U-BLCK event label {event_label} must not deliver TM-SDU payload"
        );
    }
}

#[test]
fn test_mac_u_blck_no_reservation_requirement_does_not_enqueue_grant() {
    debug::setup_logging_verbose();

    let start = TdmaTime::default().add_timeslots(2);
    let issi = 0x3022;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);
    reserve_current_uplink_for_mac_u_blck(&mut test, start, issi);

    // EN 300 392-2 table 21.94 gives MAC-U-BLCK value 1111 as "No
    // reservation requirement"; it must not be treated as ReqOver68.
    submit_mac_u_blck(&mut test, 15);
    test.run_stack(Some(4));

    let sink_msgs = test.dump_sinks();
    assert!(
        first_mac_resource_for_addr(&sink_msgs, TetraAddress::issi(issi)).is_none(),
        "MAC-U-BLCK no-reservation value must not enqueue a slot grant"
    );
    assert!(
        !sink_msgs.iter().any(|msg| matches!(msg.msg, SapMsgInner::TmaUnitdataInd(_))),
        "MAC-U-BLCK TM-SDU payload remains blocked until event-label delivery is implemented"
    );
}

#[test]
fn test_in_fragmented_sch_hu_and_sch_f() {
    // Receive SCH/HU containing MAC-ACCESS with fragmentation start
    // Then receive SCH-F containing MAC-END (UL)
    debug::setup_logging_verbose();
    let test_vec1 = "00000000111111000001001111110111000100011001011100111000000011111100001000010000000000000000";
    let test_vec2 = "0110001110000000000010010000000000000000000000000100010000000000000000000000000110010000000000000000000000001000001000000111111000001001111110000000010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
    let dltime_vec1 = TdmaTime::default().add_timeslots(2); // Downlink time: 0/1/1/3
    // let ultime_vec1 = dltime_vec1.add_timeslots(-2); // Uplink time: 0/1/1/1
    let test_prim1 = TmvUnitdataInd {
        pdu: BitBuffer::from_bitstr(test_vec1),
        block_num: PhyBlockNum::Block1,
        logical_channel: LogicalChannel::SchHu,
        crc_pass: true,
        scrambling_code: 864282631,
        rssi_dbfs: f32::NAN,
    };
    let test_sapmsg1 = SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(test_prim1),
    };
    let test_prim2 = TmvUnitdataInd {
        pdu: BitBuffer::from_bitstr(test_vec2),
        block_num: PhyBlockNum::Both,
        logical_channel: LogicalChannel::SchF,
        crc_pass: true,
        scrambling_code: 864282631,
        rssi_dbfs: f32::NAN,
    };
    let test_sapmsg2 = SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(test_prim2),
    };

    // Setup testing stack
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime_vec1));
    let components = vec![TetraEntity::Umac, TetraEntity::Llc, TetraEntity::Mle];
    let sinks: Vec<TetraEntity> = vec![
        // TetraEntity::Lmac, // Simply discard
        TetraEntity::Mm,
    ];
    test.populate_entities(components, sinks);

    // Submit and process message
    test.submit_message(test_sapmsg1);
    test.run_stack(Some(4));
    test.submit_message(test_sapmsg2);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::LmmMleUnitdataInd(prim) = &sink_msgs[0].msg else {
        panic!("expected defragmented uplink to be routed to MM");
    };

    // EN 300 392-2 clauses 21.4.2.1, 21.4.4.4 and 21.4.5.2: MAC-ACCESS
    // starts the fragmented uplink TM-SDU and MAC-END completes it. After LLC
    // removes BL-DATA and MLE removes the protocol discriminator, MM receives
    // the reconstructed management SDU from the original ISSI.
    assert_eq!(sink_msgs[0].sap, Sap::LmmSap);
    assert_eq!(sink_msgs[0].src, TetraEntity::Mle);
    assert_eq!(sink_msgs[0].dest, TetraEntity::Mm);
    assert_eq!(prim.received_address, TetraAddress::new(2065022, SsiType::Issi));
    assert_eq!(prim.handle, 0);
    assert_eq!(
        prim.sdu.to_bitstr(),
        "001011100111000000011111100001000010000000000000000000000000010010000000000000000000000000100010000000000000000000000000110010000000000000000000000001000"
    );
}

#[test]
fn test_in_fragmented_sch_hu_and_sch_hu() {
    // Receive SCH/HU containing MAC-ACCESS with fragmentation start
    // Then receive SCH-HU containing MAC-END-HU
    // Message ultimately contains CMCE SDS message
    debug::setup_logging_verbose();
    let test_vec1 = "00000000111110010001111101110111000000010010011110000010000001100010001001001111100001010100";
    let test_vec2 = "10011000000101000110000000000000000000000000000000000000000000000000111111111111110100000010";
    let dltime_vec1 = TdmaTime::default().add_timeslots(2); // Downlink time: 0/1/1/3
    // let ultime_vec1 = dltime_vec1.add_timeslots(-2); // Uplink time: 0/1/1/1
    let test_prim1 = TmvUnitdataInd {
        pdu: BitBuffer::from_bitstr(test_vec1),
        block_num: PhyBlockNum::Block1,
        logical_channel: LogicalChannel::SchHu,
        crc_pass: true,
        scrambling_code: 864282631,
        rssi_dbfs: f32::NAN,
    };
    let test_sapmsg1 = SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(test_prim1),
    };
    let test_prim2 = TmvUnitdataInd {
        pdu: BitBuffer::from_bitstr(test_vec2),
        block_num: PhyBlockNum::Block1,
        logical_channel: LogicalChannel::SchHu,
        crc_pass: true,
        scrambling_code: 864282631,
        rssi_dbfs: f32::NAN,
    };
    let test_sapmsg2 = SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(test_prim2),
    };

    // Setup testing stack
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime_vec1));
    let components = vec![TetraEntity::Umac, TetraEntity::Llc, TetraEntity::Mle];
    let sinks: Vec<TetraEntity> = vec![
        // TetraEntity::Lmac, // Simply discard
        TetraEntity::Cmce,
    ];
    test.populate_entities(components, sinks);

    // Submit and process message
    test.submit_message(test_sapmsg1);
    test.run_stack(Some(4));
    test.submit_message(test_sapmsg2);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::LcmcMleUnitdataInd(prim) = &sink_msgs[0].msg else {
        panic!("expected defragmented uplink to be routed to CMCE");
    };

    // EN 300 392-2 clauses 21.4.2.1, 21.4.4.3 and 21.4.5.2: SCH/HU
    // MAC-ACCESS plus MAC-END-HU complete one fragmented signalling TM-SDU.
    // LLC/MLE must preserve source address and endpoint context while routing
    // the CMCE SDU after the MLE protocol discriminator.
    assert_eq!(sink_msgs[0].sap, Sap::LcmcSap);
    assert_eq!(sink_msgs[0].src, TetraEntity::Mle);
    assert_eq!(sink_msgs[0].dest, TetraEntity::Cmce);
    assert_eq!(prim.received_tetra_address, TetraAddress::new(2040814, SsiType::Issi));
    assert_eq!(prim.handle, 0);
    assert_eq!(prim.endpoint_id, 0);
    assert_eq!(prim.link_id, 0);
    assert_eq!(
        prim.sdu.to_bitstr(),
        "0100111100000100000011000100010010011111000010101000000101000110000000000000000000000000000000000000000000000000111111111111110100000010"
    );
    assert!(!prim.chan_change_resp_req);
    assert!(prim.chan_change_handle.is_none());
}

#[test]
fn test_out_fragmented_resource() {
    // Test for UMAC (and LLC/MLE)
    // The vector is an MM DAttachDetachGroupIdentityAcknowledgement which contains a lot of groups.
    // As it is very large, it needs to be fragmented at the MAC layer.
    debug::setup_logging_verbose();
    let test_vec = "10110011011100110100110001101011100000000000011101010011001110110100000000000111010100111111101101000000000001110101010000000011010000000000011101010100000010110100000000000111010101000001001101000000000001110101010000011011010000000000011101010100001000110100000000000111010101000010101101000000000001110101010000110011010000000000011101010100001110110100000000000111010101000100001101000000000001110101010001001011010000000000011101010100010100";
    let dltime_vec = TdmaTime::default().add_timeslots(2); // Downlink time: 0/1/1/3
    // let ultime_vec = dltime_vec.add_timeslots(-2); // Uplink time: 0/1/1/1
    let tx_reporter = TxReporter::new_unacked();
    let test_prim = LmmMleUnitdataReq {
        sdu: BitBuffer::from_bitstr(test_vec),
        handle: 0,
        address: TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 30128,
        },
        layer2service: Layer2Service::Acknowledged,
        stealing_permission: false,
        stealing_repeats_flag: false,
        encryption_flag: false,
        is_null_pdu: false,
        tx_reporter: Some(tx_reporter.clone()),
    };
    let test_sapmsg = SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mm,
        dest: TetraEntity::Mle,
        msg: SapMsgInner::LmmMleUnitdataReq(test_prim),
    };

    // Setup testing stack
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime_vec));
    let components = vec![TetraEntity::Umac, TetraEntity::Llc, TetraEntity::Mle];
    let sinks: Vec<TetraEntity> = vec![TetraEntity::Lmac];
    test.populate_entities(components, sinks);

    // Submit and process message.  EN 300 392-2 clauses 21.1.4 and
    // 21.4.3.1-21.4.3.3 require long downlink TM-SDUs to start
    // fragmentation with MAC-RESOURCE length_ind 111111b, continue with
    // MAC-FRAG, and terminate with MAC-END.
    test.submit_message(test_sapmsg);
    let mut sink_msgs = Vec::new();
    let mut saw_fragment_start = false;
    for _ in 0..8 {
        test.run_stack(Some(1));
        let new_msgs = test.dump_sinks();
        if !saw_fragment_start
            && downlink_mac_pdus(&new_msgs).iter().any(|pdu| {
                matches!(
                    pdu,
                    DownlinkMacPdu::Resource(_, resource)
                        if resource.addr.is_some_and(|addr| addr.ssi == 30128)
                            && resource.length_ind == 0b111111
                )
            })
        {
            saw_fragment_start = true;
            assert!(
                !tx_reporter.is_transmitted(),
                "MAC transmission must not be reported before the MAC-END fragment"
            );
        }
        sink_msgs.extend(new_msgs);
    }

    let mac_pdus = downlink_mac_pdus(&sink_msgs);
    let start_idx = mac_pdus
        .iter()
        .position(|pdu| {
            matches!(
                pdu,
                DownlinkMacPdu::Resource(LogicalChannel::SchF, resource)
                    if resource.addr.is_some_and(|addr| addr.ssi == 30128)
                        && resource.length_ind == 0b111111
            )
        })
        .expect("expected SCH/F MAC-RESOURCE fragmentation start addressed to target ISSI");
    assert!(
        saw_fragment_start,
        "fragmentation start should be observed during stepwise execution"
    );

    assert!(
        mac_pdus[start_idx + 1..].iter().any(|pdu| matches!(
            pdu,
            DownlinkMacPdu::Frag(LogicalChannel::SchF) | DownlinkMacPdu::End(LogicalChannel::SchF, _)
        )),
        "fragmented downlink should contain a SCH/F MAC-FRAG or MAC-END continuation"
    );
    let end_idx = mac_pdus[start_idx + 1..]
        .iter()
        .position(|pdu| matches!(pdu, DownlinkMacPdu::End(LogicalChannel::SchF, _)))
        .map(|idx| start_idx + 1 + idx)
        .expect("fragmented downlink should terminate with SCH/F MAC-END");
    assert!(end_idx > start_idx);
    let DownlinkMacPdu::End(_, mac_end) = &mac_pdus[end_idx] else {
        unreachable!("end_idx points to MAC-END");
    };
    assert!(mac_end.length_ind > 0, "MAC-END should carry a non-zero length indication");
    assert!(
        tx_reporter.is_transmitted(),
        "MAC transmission should only be reported after the MAC-END fragment is emitted"
    );
}

#[test]
fn test_fragmented_tma_with_chan_alloc_places_allocation_in_mac_end_not_mac_resource() {
    debug::setup_logging_verbose();

    let target_issi = 30133;
    let mut timeslots = [false; 4];
    timeslots[1] = true;
    let tx_reporter = TxReporter::new_unacked();
    let test_prim = TmaUnitdataReq {
        req_handle: 9,
        pdu: BitBuffer::from_bitstr(&alternating_bits(300)),
        main_address: TetraAddress::new(target_issi, SsiType::Issi),
        endpoint_id: 1,
        pdu_prio: 0,
        stealing_permission: false,
        subscriber_class: 0,
        air_interface_encryption: None,
        stealing_repeats_flag: None,
        data_category: None,
        chan_alloc: Some(CmceChanAllocReq {
            usage: Some(4),
            timeslots,
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
            carrier: None,
        }),
        tx_reporter: Some(tx_reporter.clone()),
    };

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default().add_timeslots(2)));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(test_prim),
    });

    test.run_stack(Some(12));
    let sink_msgs = test.dump_sinks();
    let mac_pdus = downlink_mac_pdus(&sink_msgs);
    let target_addr = TetraAddress::issi(target_issi);
    let start_idx = mac_pdus
        .iter()
        .position(|pdu| {
            matches!(
                pdu,
                DownlinkMacPdu::Resource(LogicalChannel::SchF, resource)
                    if resource
                        .addr
                        .is_some_and(|resource_addr| mac_resource_matches_addr(resource_addr, target_addr))
                        && resource.length_ind == 0b111111
            )
        })
        .expect("expected SCH/F MAC-RESOURCE fragmentation start addressed to target ISSI");
    let DownlinkMacPdu::Resource(_, start_resource) = &mac_pdus[start_idx] else {
        unreachable!("start_idx points to MAC-RESOURCE");
    };
    assert!(
        start_resource.chan_alloc_element.is_none(),
        "EN 300 392-2 clause 23.4.2.1.1 forbids channel allocation in fragmented MAC-RESOURCE"
    );

    let end_idx = mac_pdus[start_idx + 1..]
        .iter()
        .position(|pdu| matches!(pdu, DownlinkMacPdu::End(LogicalChannel::SchF, _)))
        .map(|idx| start_idx + 1 + idx)
        .expect("fragmented channel-allocation downlink should terminate with SCH/F MAC-END");
    for pdu in &mac_pdus[start_idx..end_idx] {
        if let DownlinkMacPdu::Resource(_, resource) = pdu
            && resource
                .addr
                .is_some_and(|resource_addr| mac_resource_matches_addr(resource_addr, target_addr))
        {
            assert!(
                resource.chan_alloc_element.is_none(),
                "no MAC-RESOURCE in this fragmented message may carry channel allocation"
            );
        }
    }

    let DownlinkMacPdu::End(_, mac_end) = &mac_pdus[end_idx] else {
        unreachable!("end_idx points to MAC-END");
    };
    let chan_alloc = mac_end
        .chan_alloc_element
        .as_ref()
        .expect("fragmented channel allocation must be carried in MAC-END");
    assert_eq!(chan_alloc.alloc_type, ChanAllocType::Replace);
    assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Both);
    assert_eq!(chan_alloc.ts_assigned, timeslots);
    assert!(chan_alloc.clch_permission);
    assert_eq!(chan_alloc.carrier_num, test.config.config().cell.main_carrier);
    assert!(
        tx_reporter.is_transmitted(),
        "MAC transmission should report success only after the MAC-END carrying channel allocation"
    );
}

#[derive(Debug)]
enum DownlinkMacPdu {
    Resource(LogicalChannel, MacResource),
    Frag(LogicalChannel),
    End(LogicalChannel, MacEndDl),
}

fn downlink_mac_pdus(msgs: &[SapMsg]) -> Vec<DownlinkMacPdu> {
    let mut pdus = Vec::new();
    for msg in msgs {
        let SapMsgInner::TmvUnitdataReq(slot) = &msg.msg else {
            continue;
        };

        for block in [&slot.blk1, &slot.blk2].into_iter().flatten() {
            pdus.extend(parse_downlink_mac_pdus_from_block(block));
        }
    }
    pdus
}

fn downlink_mac_pdus_at(msgs: &[SapMsg], ts: TdmaTime) -> Vec<DownlinkMacPdu> {
    let mut pdus = Vec::new();
    for msg in msgs {
        let SapMsgInner::TmvUnitdataReq(slot) = &msg.msg else {
            continue;
        };
        if slot.ts != ts {
            continue;
        }

        for block in [&slot.blk1, &slot.blk2].into_iter().flatten() {
            pdus.extend(parse_downlink_mac_pdus_from_block(block));
        }
    }
    pdus
}

fn parse_downlink_mac_pdus_from_block(block: &TmvUnitdataReq) -> Vec<DownlinkMacPdu> {
    let mut pdus = Vec::new();
    if block.logical_channel != LogicalChannel::SchF && block.logical_channel != LogicalChannel::Stch {
        return pdus;
    }

    let mut mac_block = block.mac_block.clone();
    mac_block.seek(0);

    while mac_block.get_len_remaining() >= 2 {
        let start_pos = mac_block.get_pos();
        let Some(mac_pdu_type) = mac_block.peek_bits(2) else {
            break;
        };

        match mac_pdu_type {
            0b00 => {
                let Ok(resource) = MacResource::from_bitbuf(&mut mac_block) else {
                    break;
                };
                let length_bits = resource.length_ind as usize * 8;
                let next_pos = start_pos + length_bits;
                let is_fragment_start = resource.length_ind == 0b111111;
                pdus.push(DownlinkMacPdu::Resource(block.logical_channel, resource));

                if is_fragment_start || length_bits == 0 || next_pos <= mac_block.get_pos() || next_pos > mac_block.get_len() {
                    break;
                }
                mac_block.seek(next_pos);
            }
            0b01 => {
                let pdu_subtype = mac_block.peek_bits_posoffset(2, 1);
                if pdu_subtype == Some(0) {
                    if MacFragDl::from_bitbuf(&mut mac_block).is_ok() {
                        pdus.push(DownlinkMacPdu::Frag(block.logical_channel));
                    }
                } else if pdu_subtype == Some(1) {
                    if let Ok(pdu) = MacEndDl::from_bitbuf(&mut mac_block) {
                        pdus.push(DownlinkMacPdu::End(block.logical_channel, pdu));
                    }
                }
                break;
            }
            _ => break,
        }
    }

    pdus
}

fn first_mac_resource_for_addr(msgs: &[SapMsg], target_addr: TetraAddress) -> Option<MacResource> {
    mac_resources_for_addr(msgs, target_addr)
        .into_iter()
        .map(|(_, resource)| resource)
        .next()
}

fn mac_resource_matches_addr(resource_addr: TetraAddress, target_addr: TetraAddress) -> bool {
    resource_addr == target_addr
        || (resource_addr.ssi == target_addr.ssi
            && resource_addr.ssi_type == SsiType::Ssi
            && matches!(target_addr.ssi_type, SsiType::Issi | SsiType::Gssi))
}

fn mac_resources_for_addr(msgs: &[SapMsg], target_addr: TetraAddress) -> Vec<(LogicalChannel, MacResource)> {
    downlink_mac_pdus(msgs)
        .into_iter()
        .filter_map(|pdu| match pdu {
            DownlinkMacPdu::Resource(logical_channel, resource)
                if resource
                    .addr
                    .is_some_and(|resource_addr| mac_resource_matches_addr(resource_addr, target_addr)) =>
            {
                Some((logical_channel, resource))
            }
            _ => None,
        })
        .collect()
}

fn tma_report_for_handle(msgs: &[SapMsg], req_handle: i32) -> Option<TmaReport> {
    msgs.iter().find_map(|msg| match &msg.msg {
        SapMsgInner::TmaReportInd(report) if report.req_handle == req_handle => Some(report.report.clone()),
        _ => None,
    })
}

fn build_tma_cancel_req(req_handle: i32) -> SapMsg {
    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaCancelReq(TmaCancelReq { req_handle }),
    }
}

fn build_tma_unitdata_req(req_handle: i32, target_issi: u32) -> SapMsg {
    build_tma_unitdata_req_with_payload(req_handle, target_issi, BitBuffer::from_bitstr("10101010"), None)
}

fn build_tma_unitdata_req_with_payload(req_handle: i32, target_issi: u32, pdu: BitBuffer, tx_reporter: Option<TxReporter>) -> SapMsg {
    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle,
            pdu,
            main_address: TetraAddress::issi(target_issi),
            endpoint_id: 1,
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

fn build_p2p_d_setup_tma_req(req_handle: i32, calling_issi: u32, called_issi: u32, tx_reporter: TxReporter) -> SapMsg {
    let mut cmce_sdu = BitBuffer::new_autoexpand(96);
    DSetup {
        call_identifier: 6,
        call_time_out: CallTimeout::T60s,
        hook_method_selection: false,
        simplex_duplex_selection: false,
        basic_service_information: BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2p,
            slots_per_frame: None,
            speech_service: Some(0),
        },
        transmission_grant: TransmissionGrant::NotGranted,
        transmission_request_permission: false,
        call_priority: 0,
        notification_indicator: None,
        temporary_address: None,
        calling_party_address_ssi: Some(calling_issi),
        calling_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    }
    .to_bitbuf(&mut cmce_sdu)
    .expect("serialize P2P D-SETUP");
    cmce_sdu.seek(0);

    build_tma_unitdata_req_with_payload(req_handle, called_issi, llc_wrapped_cmce_sdu(cmce_sdu), Some(tx_reporter))
}

#[test]
fn test_unsolicited_issi_downlink_does_not_set_random_access_ack_flag() {
    debug::setup_logging_verbose();

    let target_issi = 30128;
    let test_prim = TmaUnitdataReq {
        req_handle: 1,
        pdu: BitBuffer::from_bitstr("10101010"),
        main_address: TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: target_issi,
        },
        endpoint_id: 1,
        pdu_prio: 0,
        stealing_permission: false,
        subscriber_class: 0,
        air_interface_encryption: None,
        stealing_repeats_flag: None,
        data_category: None,
        chan_alloc: None,
        tx_reporter: None,
    };
    let test_sapmsg = SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(test_prim),
    };

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default().add_timeslots(2)));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(test_sapmsg);
    test.run_stack(Some(80));
    let sink_msgs = test.dump_sinks();
    let mac_resource =
        first_mac_resource_for_addr(&sink_msgs, TetraAddress::issi(target_issi)).expect("expected MAC-RESOURCE addressed to target ISSI");

    assert!(
        !mac_resource.random_access_flag,
        "EN 300 392-2 clause 21.4.3.1 random_access_flag must only acknowledge actual random access"
    );
}

#[test]
fn test_acked_channel_allocation_tma_carries_current_channel_ack_grant() {
    debug::setup_logging_verbose();

    let target_issi = 0x3318;
    let mut llc_pdu = BitBuffer::new_autoexpand(16);
    BlData { has_fcs: false, ns: 0 }.to_bitbuf(&mut llc_pdu);
    llc_pdu.write_bits(0b101010, 6);
    llc_pdu.seek(0);

    let mut assigned = [false; 4];
    assigned[1] = true;
    let test_prim = TmaUnitdataReq {
        req_handle: 33,
        pdu: llc_pdu,
        main_address: TetraAddress::issi(target_issi),
        endpoint_id: 1,
        pdu_prio: 6,
        stealing_permission: false,
        subscriber_class: 0,
        air_interface_encryption: None,
        stealing_repeats_flag: None,
        data_category: None,
        chan_alloc: Some(CmceChanAllocReq {
            usage: Some(4),
            timeslots: assigned,
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
            carrier: None,
        }),
        tx_reporter: None,
    };

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default().add_timeslots(2)));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(test_prim),
    });
    test.run_stack(Some(12));

    let sink_msgs = test.dump_sinks();
    let mac_resource = first_mac_resource_for_addr(&sink_msgs, TetraAddress::issi(target_issi))
        .expect("acknowledged channel-allocation transfer should emit MAC-RESOURCE");
    assert!(
        mac_resource.chan_alloc_element.is_some(),
        "sanity check: test vector must carry a channel allocation"
    );
    assert_eq!(
        mac_resource.pos_of_grant, 0,
        "EN 300 392-2 23.5.4.3 permits the BL-ACK slot grant on the current MCCH before the channel change"
    );
    let slot_grant = mac_resource
        .slot_granting_element
        .expect("acknowledged channel-allocation transfer must grant an uplink subslot for BL-ACK");
    assert!(
        matches!(
            slot_grant.capacity_allocation,
            BasicSlotgrantCapAlloc::FirstSubslotGranted | BasicSlotgrantCapAlloc::SecondSubslotGranted
        ),
        "a BL-ACK fits in one reserved subslot"
    );
    assert_ne!(
        slot_grant.granting_delay,
        BasicSlotgrantGrantingDelay::WaitForAnotherSlotgrantMessage,
        "D-CONNECT ACK/D-CONNECT setup progress must carry a real ACK opportunity"
    );
}

#[test]
fn test_acked_channel_allocation_stealing_tma_uses_assigned_channel_stch() {
    debug::setup_logging_verbose();

    let caller_issi = 2_260_082;
    let called_issi = 2_260_618;
    let traffic_ts = 2;
    let req_handle = 34;
    let mut llc_pdu = BitBuffer::new_autoexpand(24);
    BlData { has_fcs: false, ns: 0 }.to_bitbuf(&mut llc_pdu);
    llc_pdu.write_bits(0b10101010, 8);
    llc_pdu.seek(0);

    let mut assigned = [false; 4];
    assigned[traffic_ts as usize - 1] = true;
    let tx_reporter = TxReporter::new_unacked();
    let test_prim = TmaUnitdataReq {
        req_handle,
        pdu: llc_pdu,
        main_address: TetraAddress::issi(called_issi),
        endpoint_id: 1,
        pdu_prio: 6,
        stealing_permission: true,
        subscriber_class: 0,
        air_interface_encryption: None,
        stealing_repeats_flag: None,
        data_category: None,
        chan_alloc: Some(CmceChanAllocReq {
            usage: Some(4),
            timeslots: assigned,
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
            carrier: None,
        }),
        tx_reporter: Some(tx_reporter.clone()),
    };

    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);
    test.submit_message(private_call_open_msg(called_issi, caller_issi, traffic_ts));
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(test_prim),
    });
    test.run_stack(Some(4));

    let sink_msgs = test.dump_sinks();
    let stch_resource = mac_resources_for_addr(&sink_msgs, TetraAddress::issi(called_issi))
        .into_iter()
        .find(|(logical_channel, resource)| *logical_channel == LogicalChannel::Stch && resource.chan_alloc_element.is_some())
        .map(|(_, resource)| resource)
        .expect("assigned-channel recovery should emit STCH MAC-RESOURCE to called ISSI");
    assert_eq!(
        stch_resource
            .chan_alloc_element
            .as_ref()
            .expect("STCH recovery must preserve channel allocation")
            .ul_dl_assigned,
        UlDlAssignment::Both
    );
    assert_eq!(
        stch_resource.usage_marker,
        Some(4),
        "EN 300 392-2 clauses 14.5.3.1, 23.5 and 23.8.2.2: assigned-channel D-CONNECT ACK recovery must carry the traffic usage marker on STCH"
    );
    assert_eq!(tx_reporter.get_state(), TxState::Transmitted);
    assert!(
        matches!(
            tma_report_for_handle(&sink_msgs, req_handle),
            Some(TmaReport::SuccessReservedOrStealing)
        ),
        "complete assigned-channel STCH recovery should report reserved/stealing success"
    );
}

#[test]
fn test_private_caller_d_connect_assigned_channel_recovery_fits_stch_when_compact() {
    debug::setup_logging_verbose();

    let caller_issi = 2_260_082;
    let called_issi = 2_260_618;
    let traffic_ts = 2;
    let req_handle = 36;
    let compact_sdu = llc_ack_wrapped_cmce_sdu(private_caller_d_connect_sdu(4, None));
    let notified_sdu = llc_ack_wrapped_cmce_sdu(private_caller_d_connect_sdu(4, Some(19)));

    let mut assigned = [false; 4];
    assigned[traffic_ts as usize - 1] = true;
    let tx_reporter = TxReporter::new_unacked();
    let test_prim = TmaUnitdataReq {
        req_handle,
        pdu: compact_sdu.clone(),
        main_address: TetraAddress::issi(caller_issi),
        endpoint_id: 1,
        pdu_prio: 6,
        stealing_permission: true,
        subscriber_class: 0,
        air_interface_encryption: None,
        stealing_repeats_flag: None,
        data_category: None,
        chan_alloc: Some(CmceChanAllocReq {
            usage: Some(4),
            timeslots: assigned,
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
            carrier: None,
        }),
        tx_reporter: Some(tx_reporter.clone()),
    };

    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);
    test.submit_message(private_call_open_msg(called_issi, caller_issi, traffic_ts));
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(test_prim),
    });
    test.run_stack(Some(4));

    let sink_msgs = test.dump_sinks();
    let stch_resource = mac_resources_for_addr(&sink_msgs, TetraAddress::issi(caller_issi))
        .into_iter()
        .find(|(logical_channel, resource)| *logical_channel == LogicalChannel::Stch && resource.chan_alloc_element.is_some())
        .map(|(_, resource)| resource)
        .expect("compact caller D-CONNECT recovery should emit STCH MAC-RESOURCE");
    let stch_header_len = stch_resource.compute_header_len();
    assert!(
        stch_header_len + compact_sdu.get_len() <= 124,
        "compact caller D-CONNECT must fit FACCH/STCH with MAC-RESOURCE channel allocation"
    );
    assert!(
        stch_header_len + notified_sdu.get_len() > 124,
        "adding optional caller notification would reproduce the RF failure: MAC-RESOURCE header plus SDU no longer fits STCH"
    );
    assert_eq!(
        stch_resource
            .chan_alloc_element
            .as_ref()
            .expect("caller D-CONNECT recovery must preserve channel allocation")
            .ul_dl_assigned,
        UlDlAssignment::Both
    );
    assert_eq!(
        stch_resource.usage_marker,
        Some(4),
        "EN 300 392-2 clauses 14.5.1.1.2 and Annex D.4: caller D-CONNECT assigned-channel recovery keeps the traffic usage marker"
    );
    assert_eq!(tx_reporter.get_state(), TxState::Transmitted);
    assert!(
        matches!(
            tma_report_for_handle(&sink_msgs, req_handle),
            Some(TmaReport::SuccessReservedOrStealing)
        ),
        "complete compact caller D-CONNECT STCH recovery should report reserved/stealing success"
    );
}

#[test]
fn test_assigned_channel_stch_recovery_survives_frame_18_gap() {
    debug::setup_logging_verbose();

    let caller_issi = 2_260_618;
    let called_issi = 2_260_082;
    let traffic_ts = 2;
    let req_handle = 35;
    let mut llc_pdu = BitBuffer::new_autoexpand(24);
    BlData { has_fcs: false, ns: 0 }.to_bitbuf(&mut llc_pdu);
    llc_pdu.write_bits(0b10101010, 8);
    llc_pdu.seek(0);

    let mut assigned = [false; 4];
    assigned[traffic_ts as usize - 1] = true;
    let tx_reporter = TxReporter::new_unacked();
    let test_prim = TmaUnitdataReq {
        req_handle,
        pdu: llc_pdu,
        main_address: TetraAddress::issi(called_issi),
        endpoint_id: 1,
        pdu_prio: 6,
        stealing_permission: true,
        subscriber_class: 0,
        air_interface_encryption: None,
        stealing_repeats_flag: None,
        data_category: None,
        chan_alloc: Some(CmceChanAllocReq {
            usage: Some(4),
            timeslots: assigned,
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
            carrier: None,
        }),
        tx_reporter: Some(tx_reporter.clone()),
    };

    // MACSCHED_TX_AHEAD is one timeslot, so starting at f=18,t=1 makes the
    // first finalized target f=18,t=2: the observed P2P TS2 recovery boundary.
    let start = TdmaTime { h: 0, m: 1, f: 18, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);
    test.submit_message(private_call_open_msg(called_issi, caller_issi, traffic_ts));
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(test_prim),
    });

    test.run_stack(Some(1));
    let frame18_msgs = test.dump_sinks();
    assert!(
        mac_resources_for_addr(&frame18_msgs, TetraAddress::issi(called_issi))
            .into_iter()
            .all(|(logical_channel, _)| logical_channel != LogicalChannel::Stch),
        "EN 300 392-2 fixed frame-18 handling must not emit assigned-channel STCH on f=18 TS2 without frame-18 receive support"
    );
    assert_eq!(
        tx_reporter.get_state(),
        TxState::Pending,
        "queued assigned-channel recovery must survive the frame-18 traffic gap without a false transmission report"
    );
    assert!(
        tma_report_for_handle(&frame18_msgs, req_handle).is_none(),
        "UMAC must not report STCH recovery success until the queued block is actually emitted"
    );

    test.run_stack(Some(4));
    let recovery_msgs = test.dump_sinks();
    let stch_resource = mac_resources_for_addr(&recovery_msgs, TetraAddress::issi(called_issi))
        .into_iter()
        .find(|(logical_channel, resource)| *logical_channel == LogicalChannel::Stch && resource.chan_alloc_element.is_some())
        .map(|(_, resource)| resource)
        .expect("assigned-channel recovery should be retained and emitted on the next legal TS2 traffic opportunity");
    assert_eq!(
        stch_resource
            .chan_alloc_element
            .as_ref()
            .expect("retained STCH recovery must preserve channel allocation")
            .ul_dl_assigned,
        UlDlAssignment::Both
    );
    assert_eq!(
        stch_resource.usage_marker,
        Some(4),
        "retained D-CONNECT ACK recovery must keep the assigned traffic usage marker"
    );
    assert_eq!(tx_reporter.get_state(), TxState::Transmitted);
    assert!(
        matches!(
            tma_report_for_handle(&recovery_msgs, req_handle),
            Some(TmaReport::SuccessReservedOrStealing)
        ),
        "UMAC should report success once the retained STCH recovery reaches the legal traffic slot"
    );
}

#[test]
fn test_decoded_same_numeric_ssi_preserves_ra_ack_and_grant() {
    debug::setup_logging_verbose();

    let shared_ssi = 0x3021;
    let group_member_issi = 0x3022;
    let start = TdmaTime::default().add_timeslots(2);
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        state.subscribers.register(group_member_issi);
        state.subscribers.affiliate(group_member_issi, shared_ssi);
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);

    // EN 300 392-2 clause 21.4.3.1 defines random_access_flag as an ACK for
    // the specific MS random access. Clause 23.5.2.2.7 requires the slot grant
    // to be addressed to the intended listener. A numerically equal GSSI must
    // not absorb an ISSI acknowledgement or reservation grant.
    submit_mac_access_with_reservation(&mut test, shared_ssi, ReservationRequirement::Req1Slot);
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 50,
            pdu: BitBuffer::from_bitstr("10101010"),
            main_address: TetraAddress::new(shared_ssi, SsiType::Gssi),
            endpoint_id: 1,
            pdu_prio: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: None,
            tx_reporter: None,
        }),
    });

    test.run_stack(Some(24));
    let sink_msgs = test.dump_sinks();
    let resources = mac_resources_for_addr(&sink_msgs, TetraAddress::issi(shared_ssi));
    assert!(
        resources.len() >= 2,
        "decoded same-numeric SSI resources should remain separately observable; MAC-RESOURCE table 21.55 carries SSI generically on air: {resources:?}"
    );
    assert!(
        resources
            .iter()
            .all(|(_, resource)| resource.addr == Some(TetraAddress::new(shared_ssi, SsiType::Ssi))),
        "decoded MAC-RESOURCE should expose generic SSI on air; scheduler unit tests cover the pre-encoding ISSI/GSSI split"
    );
    assert!(
        resources
            .iter()
            .any(|(_, resource)| resource.random_access_flag && resource.slot_granting_element.is_some()),
        "one decoded MAC-RESOURCE should preserve the ISSI random access ACK and reservation grant"
    );
    assert!(
        resources
            .iter()
            .any(|(_, resource)| !resource.random_access_flag && resource.slot_granting_element.is_none()),
        "the numerically equal GSSI downlink should stay as a separate plain MAC-RESOURCE"
    );
}

#[test]
fn test_tma_unitdata_complete_transmission_emits_tma_report_ind() {
    debug::setup_logging_verbose();

    let target_issi = 30131;
    let req_handle = 47;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default().add_timeslots(2)));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);

    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle,
            pdu: BitBuffer::from_bitstr("10101010"),
            main_address: TetraAddress::new(target_issi, SsiType::Issi),
            endpoint_id: 1,
            pdu_prio: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: None,
            tx_reporter: None,
        }),
    });

    test.run_stack(Some(8));
    let sink_msgs = test.dump_sinks();

    assert!(
        first_mac_resource_for_addr(&sink_msgs, TetraAddress::issi(target_issi)).is_some(),
        "sanity check: the TM-SDU should have been put in a MAC-RESOURCE"
    );
    assert!(
        matches!(
            tma_report_for_handle(&sink_msgs, req_handle),
            Some(TmaReport::SuccessReservedOrStealing)
        ),
        "EN 300 392-2 clauses 20.4.1.1.3 and 23.1.2.1.1 require MAC to report complete TM-SDU transmission"
    );
}

#[test]
fn test_tma_report_tracking_is_bounded_under_stalled_downlink_completion() {
    debug::setup_logging_verbose();

    let base_handle = 30_000;
    let base_issi = 500_000;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default().add_timeslots(2)));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    let cap = {
        let umac = test
            .router
            .get_entity(TetraEntity::Umac)
            .expect("UMAC entity should be registered")
            .as_any_mut()
            .downcast_mut::<UmacBs>()
            .expect("registered UMAC should be UmacBs");
        umac.debug_max_pending_tma_reports_for_test()
    };

    for offset in 0..=cap {
        test.submit_message(build_tma_unitdata_req(base_handle + offset as i32, base_issi + offset as u32));
    }
    test.deliver_all_messages();

    let pending_count = {
        let umac = test
            .router
            .get_entity(TetraEntity::Umac)
            .expect("UMAC entity should be registered")
            .as_any_mut()
            .downcast_mut::<UmacBs>()
            .expect("registered UMAC should be UmacBs");
        umac.debug_pending_tma_report_count_for_test()
    };
    assert_eq!(
        pending_count, cap,
        "UMAC must cap retained TMA reports when downlink completion is stalled"
    );

    let sink_msgs = test.dump_sinks();
    assert!(
        matches!(
            tma_report_for_handle(&sink_msgs, base_handle + cap as i32),
            Some(TmaReport::FragmentationFailure)
        ),
        "EN 300 392-2 clause 20.4.1.1.3: overflowed local MAC request should fail via TMA-REPORT instead of growing pending state"
    );
    assert!(
        tma_report_for_handle(&sink_msgs, base_handle).is_none(),
        "requests inside the cap should remain pending until transmitted, cancelled, discarded, or timeout guarded"
    );
}

#[test]
fn test_tma_report_timeout_cancels_queued_mac_resource() {
    debug::setup_logging_verbose();

    let target_handle = 30_100;
    let trigger_handle = 30_101;
    let target_issi = 2_260_618;
    let trigger_issi = 2_260_619;
    let start = TdmaTime::default().add_timeslots(2);
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);

    test.submit_message(build_tma_unitdata_req(target_handle, target_issi));
    test.deliver_all_messages();

    {
        let umac = test
            .router
            .get_entity(TetraEntity::Umac)
            .expect("UMAC entity should be registered")
            .as_any_mut()
            .downcast_mut::<UmacBs>()
            .expect("registered UMAC should be UmacBs");
        assert_eq!(umac.debug_pending_tma_report_count_for_test(), 1);
        let timeout = umac.debug_tma_report_pending_timeout_timeslots_for_test();
        umac.debug_force_pending_tma_report_age_for_test(timeout + 1);
    }

    // Processing a new request drains timed-out retained reports before the
    // scheduler gets another chance to emit the old one. EN 300 392-2 clause
    // 20.4.1.1.3 reports the failed TMA request; the matching queued RF
    // message must be cancelled as part of the same local failure handling.
    test.submit_message(build_tma_unitdata_req(trigger_handle, trigger_issi));
    test.deliver_all_messages();
    let timeout_msgs = test.dump_sinks();
    assert!(
        matches!(
            tma_report_for_handle(&timeout_msgs, target_handle),
            Some(TmaReport::FragmentationFailure)
        ),
        "timed-out TMA request should report MAC fragmentation failure"
    );
    assert!(
        first_mac_resource_for_addr(&timeout_msgs, TetraAddress::issi(target_issi)).is_none(),
        "draining the timeout must not itself transmit the stale target request"
    );

    test.run_stack(Some(8));
    let later_msgs = test.dump_sinks();
    assert!(
        first_mac_resource_for_addr(&later_msgs, TetraAddress::issi(target_issi)).is_none(),
        "a TMA request reported failed must not remain queued for delayed RF transmission"
    );
    assert!(
        first_mac_resource_for_addr(&later_msgs, TetraAddress::issi(trigger_issi)).is_some(),
        "sanity check: later non-timed-out TMA traffic should still transmit normally"
    );
}

#[test]
fn test_standalone_ack_only_bl_ack_without_reporter_does_not_retain_tma_report() {
    debug::setup_logging_verbose();

    let req_handle = 30_150;
    let target_issi = 2_260_620;
    let start = TdmaTime::default().add_timeslots(2);
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);

    test.submit_message(build_tma_unitdata_req_with_payload(
        req_handle,
        target_issi,
        bl_ack_tma_sdu_for_test(0),
        None,
    ));
    test.deliver_all_messages();

    let pending_count = {
        let umac = test
            .router
            .get_entity(TetraEntity::Umac)
            .expect("UMAC entity should be registered")
            .as_any_mut()
            .downcast_mut::<UmacBs>()
            .expect("registered UMAC should be UmacBs");
        umac.debug_pending_tma_report_count_for_test()
    };
    assert_eq!(
        pending_count, 0,
        "standalone ACK-only BL-ACK has no service reporter and must not retain local pending TMA state"
    );

    test.run_stack(Some(8));
    let sink_msgs = test.dump_sinks();
    assert!(
        tma_report_for_handle(&sink_msgs, req_handle).is_none(),
        "ACK-only BL-ACK without a TxReporter must not synthesize a later TMA report"
    );
}

#[test]
fn test_eg7_p2p_d_setup_preempts_ordinary_backlog_at_receive_window() {
    debug::setup_logging_verbose();

    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let receive_window = TdmaTime { h: 0, m: 1, f: 3, t: 1 };
    let calling_issi = 2_260_082;
    let called_issi = 2_260_618;
    let ordinary_issi = 9_000_001;
    let d_setup_handle = 30_202;
    let d_setup_reporter = TxReporter::new_unacked();
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        state.energy_saving.insert(
            called_issi,
            EnergySavingAssignment {
                mode: 7,
                frame: Some(receive_window.f),
                multiframe: Some(receive_window.m),
                awake_until: None,
                suspension_count: 0,
            },
        );
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);

    test.submit_message(build_tma_unitdata_req_with_payload(
        30_201,
        ordinary_issi,
        BitBuffer::from_bitstr(&alternating_bits(4096)),
        None,
    ));
    test.submit_message(build_p2p_d_setup_tma_req(
        d_setup_handle,
        calling_issi,
        called_issi,
        d_setup_reporter.clone(),
    ));

    test.run_stack(Some(5));
    let sink_msgs = test.dump_sinks();
    let receive_window_pdus = downlink_mac_pdus_at(&sink_msgs, receive_window);
    let first_resource = receive_window_pdus
        .iter()
        .find_map(|pdu| match pdu {
            DownlinkMacPdu::Resource(LogicalChannel::SchF, resource) => Some(resource),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!("expected a SCH/F MAC-RESOURCE at the EG7 receive window {receive_window}; observed {receive_window_pdus:?}")
        });

    assert!(
        first_resource
            .addr
            .is_some_and(|addr| mac_resource_matches_addr(addr, TetraAddress::issi(called_issi))),
        "EN 300 392-2 clauses 14.5.1.1.1 and 23.5.2.2.7: once the EG7 called MS is listening, P2P D-SETUP must preempt ordinary SCH/F backlog; observed first resource {first_resource:?}"
    );
    assert_eq!(
        d_setup_reporter.get_state(),
        TxState::Transmitted,
        "D-SETUP reporter should complete when the MAC-RESOURCE is emitted at the called MS receive window"
    );
    assert!(
        matches!(
            tma_report_for_handle(&sink_msgs, d_setup_handle),
            Some(TmaReport::SuccessReservedOrStealing)
        ),
        "complete D-SETUP transmission should emit TMA success before the local pending-report guard"
    );
}

#[test]
fn test_tma_report_cap_admits_higher_priority_floor_control_over_protected_backlog() {
    debug::setup_logging_verbose();

    let call_id = 6;
    let traffic_ts = 2;
    let mut timeslots = [false; 4];
    timeslots[(traffic_ts - 1) as usize] = true;
    let listener_base_handle = 100_000;
    let positive_handle = 200_000;
    let floor_withdraw_handle = 200_001;
    let requester_issi = 2_260_082;
    let gssi = 226_333;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default().add_timeslots(2)));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    let cap = {
        let umac = test
            .router
            .get_entity(TetraEntity::Umac)
            .expect("UMAC entity should be registered")
            .as_any_mut()
            .downcast_mut::<UmacBs>()
            .expect("registered UMAC should be UmacBs");
        umac.debug_max_pending_tma_reports_for_test()
    };

    let make_floor_tma = |req_handle: i32, pdu: BitBuffer, main_address: TetraAddress, chan_alloc: Option<CmceChanAllocReq>| -> SapMsg {
        SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
                req_handle,
                pdu,
                main_address,
                endpoint_id: 0,
                pdu_prio: 0,
                stealing_permission: true,
                subscriber_class: 0,
                air_interface_encryption: None,
                stealing_repeats_flag: None,
                data_category: None,
                chan_alloc,
                tx_reporter: None,
            }),
        }
    };

    let listener_alloc = Some(CmceChanAllocReq {
        usage: Some(6),
        timeslots,
        alloc_type: ChanAllocType::Replace,
        ul_dl_assigned: UlDlAssignment::Dl,
        carrier: None,
    });
    for offset in 0..cap {
        test.submit_message(make_floor_tma(
            listener_base_handle + offset as i32,
            d_tx_granted_sdu(call_id, TransmissionGrant::GrantedToOtherUser),
            TetraAddress::new(gssi, SsiType::Gssi),
            listener_alloc.clone(),
        ));
    }

    let positive_alloc = Some(CmceChanAllocReq {
        usage: Some(6),
        timeslots,
        alloc_type: ChanAllocType::Replace,
        ul_dl_assigned: UlDlAssignment::Both,
        carrier: None,
    });
    test.submit_message(make_floor_tma(
        positive_handle,
        d_tx_granted_sdu(call_id, TransmissionGrant::Granted),
        TetraAddress::issi(requester_issi),
        positive_alloc,
    ));
    test.submit_message(make_floor_tma(
        floor_withdraw_handle,
        d_tx_ceased_sdu(call_id),
        TetraAddress::new(gssi, SsiType::Gssi),
        None,
    ));

    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    let pending_count = {
        let umac = test
            .router
            .get_entity(TetraEntity::Umac)
            .expect("UMAC entity should be registered")
            .as_any_mut()
            .downcast_mut::<UmacBs>()
            .expect("registered UMAC should be UmacBs");
        umac.debug_pending_tma_report_count_for_test()
    };
    assert_eq!(pending_count, cap);

    // EN 300 392-2 clause 14.5.2.2.1 floor-control notifications are all
    // protected signalling, but they are not equally urgent. UMAC admission
    // must preserve the requester positive grant and floor withdrawal by
    // evicting lower-priority listener grants under the local pending-report
    // cap, with explicit TMA failures for the evicted requests.
    assert!(
        matches!(
            tma_report_for_handle(&sink_msgs, listener_base_handle),
            Some(TmaReport::FragmentationFailure)
        ),
        "positive requester grant should evict the oldest lower-priority listener grant"
    );
    assert!(
        matches!(
            tma_report_for_handle(&sink_msgs, listener_base_handle + 1),
            Some(TmaReport::FragmentationFailure)
        ),
        "floor withdrawal should evict the next lower-priority listener grant"
    );
    assert!(
        tma_report_for_handle(&sink_msgs, positive_handle).is_none(),
        "positive requester grant must remain pending, not fail admission"
    );
    assert!(
        tma_report_for_handle(&sink_msgs, floor_withdraw_handle).is_none(),
        "floor withdrawal must remain pending, not fail admission"
    );
}

#[test]
fn test_tma_cancel_removes_pending_unitdata_without_report_or_transmission() {
    debug::setup_logging_verbose();

    let target_issi = 30133;
    let req_handle = 49;
    let reporter = TxReporter::new_unacked();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default().add_timeslots(2)));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);

    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle,
            pdu: BitBuffer::from_bitstr("10101010"),
            main_address: TetraAddress::new(target_issi, SsiType::Issi),
            endpoint_id: 1,
            pdu_prio: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: None,
            tx_reporter: Some(reporter.clone()),
        }),
    });
    test.deliver_all_messages();

    // EN 300 392-2 clause 20.4.1.1.1: TMA-CANCEL cancels a previously
    // submitted TMA-UNITDATA request. Cancel before the scheduler builds
    // SCH/F, so no over-air MAC-RESOURCE and no progress/failure TMA-REPORT
    // should be emitted for this handle.
    test.submit_message(build_tma_cancel_req(req_handle));
    test.deliver_all_messages();
    assert_eq!(reporter.get_state(), TxState::Discarded);

    test.run_stack(Some(8));
    let sink_msgs = test.dump_sinks();
    assert!(
        first_mac_resource_for_addr(&sink_msgs, TetraAddress::issi(target_issi)).is_none(),
        "cancelled TMA-UNITDATA must not be transmitted"
    );
    assert!(
        tma_report_for_handle(&sink_msgs, req_handle).is_none(),
        "explicit TMA-CANCEL must remove the pending request instead of reporting a MAC failure"
    );
}

#[test]
fn test_tma_cancel_after_transmission_does_not_retract_success_report() {
    debug::setup_logging_verbose();

    let target_issi = 30134;
    let req_handle = 51;
    let reporter = TxReporter::new_unacked();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default().add_timeslots(2)));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);

    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle,
            pdu: BitBuffer::from_bitstr("10101010"),
            main_address: TetraAddress::new(target_issi, SsiType::Issi),
            endpoint_id: 1,
            pdu_prio: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: None,
            tx_reporter: Some(reporter.clone()),
        }),
    });
    test.run_stack(Some(8));
    let transmitted_msgs = test.dump_sinks();
    assert!(
        first_mac_resource_for_addr(&transmitted_msgs, TetraAddress::issi(target_issi)).is_some(),
        "sanity check: the TMA-UNITDATA should be transmitted before cancel"
    );
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert!(
        matches!(
            tma_report_for_handle(&transmitted_msgs, req_handle),
            Some(TmaReport::SuccessReservedOrStealing)
        ),
        "complete transmission should report success before a late cancel"
    );

    test.submit_message(build_tma_cancel_req(req_handle));
    test.run_stack(Some(1));
    let cancel_msgs = test.dump_sinks();
    assert_eq!(reporter.get_state(), TxState::Transmitted);
    assert!(
        tma_report_for_handle(&cancel_msgs, req_handle).is_none(),
        "late TMA-CANCEL must not synthesize a failure after success was reported"
    );
}

#[test]
fn test_tma_unitdata_scheduler_discard_emits_fragmentation_failure_report() {
    debug::setup_logging_verbose();

    let target_issi = 30132;
    let req_handle = 48;
    let reporter = TxReporter::new_unacked();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default().add_timeslots(2)));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);

    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle,
            pdu: BitBuffer::from_bitstr("10101010"),
            main_address: TetraAddress::new(target_issi, SsiType::Issi),
            endpoint_id: 1,
            pdu_prio: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: None,
            tx_reporter: Some(reporter.clone()),
        }),
    });
    test.deliver_all_messages();

    let umac = test
        .router
        .get_entity(TetraEntity::Umac)
        .expect("UMAC entity should be registered")
        .as_any_mut()
        .downcast_mut::<UmacBs>()
        .expect("registered UMAC should be UmacBs");
    assert!(
        umac.channel_scheduler.dl_drop_all_except_stolen(1),
        "test setup should discard the queued downlink request"
    );
    assert_eq!(reporter.get_state(), TxState::Discarded);

    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert!(
        matches!(tma_report_for_handle(&sink_msgs, req_handle), Some(TmaReport::FragmentationFailure)),
        "EN 300 392-2 clauses 20.4.1.1.3 and 22.3.2.3(h) use TMA fragmentation failure when the TM-SDU is not completely sent"
    );
}

#[test]
fn test_dl_circuit_close_discards_pending_stch_stealing_reporter() {
    debug::setup_logging_verbose();

    let target_issi = 30135;
    let peer_issi = 30136;
    let req_handle = 52;
    let reporter = TxReporter::new_unacked();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default().add_timeslots(2)));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);

    test.submit_message(private_call_open_msg(target_issi, peer_issi, 2));
    test.deliver_all_messages();

    let mut stealing_req =
        build_tma_unitdata_req_with_payload(req_handle, target_issi, BitBuffer::from_bitstr("10101010"), Some(reporter.clone()));
    let SapMsgInner::TmaUnitdataReq(prim) = &mut stealing_req.msg else {
        panic!("expected TMA-UNITDATA.req");
    };
    prim.stealing_permission = true;
    prim.pdu_prio = 5;

    test.submit_message(stealing_req);
    test.deliver_all_messages();
    assert_eq!(
        reporter.get_state(),
        TxState::Pending,
        "test setup should leave the STCH stealing block queued but not yet emitted"
    );

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Close(Direction::Dl, 2)),
    });
    test.deliver_all_messages();
    assert_eq!(
        reporter.get_state(),
        TxState::Discarded,
        "closing the DL circuit must discard queued STCH stealing instead of leaving it for pending-report timeout"
    );

    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();
    assert!(
        matches!(tma_report_for_handle(&sink_msgs, req_handle), Some(TmaReport::FragmentationFailure)),
        "discarded queued STCH stealing should surface as the standard MAC fragmentation failure report"
    );
}

#[test]
fn test_all_ones_broadcast_fragments_wait_for_full_active_eg_batch_without_t210() {
    debug::setup_logging_verbose();

    let start = TdmaTime { t: 3, f: 2, m: 1, h: 0 };
    let all_ones_gssi = 0xFF_FFFF;
    let first_issi = 1401;
    let second_issi = 1402;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        state.subscribers.register(first_issi);
        state.subscribers.register(second_issi);
        state.energy_saving.insert(
            first_issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
        state.energy_saving.insert(
            second_issi,
            EnergySavingAssignment {
                mode: 3,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 49,
            pdu: BitBuffer::new(600),
            main_address: TetraAddress::new(all_ones_gssi, SsiType::Gssi),
            endpoint_id: 1,
            pdu_prio: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: None,
            tx_reporter: None,
        }),
    });

    test.run_stack(Some(28));
    let sink_msgs = test.dump_sinks();
    let observed_mac_times: Vec<TdmaTime> = sink_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmvUnitdataReq(slot)
                if [&slot.blk1, &slot.blk2]
                    .into_iter()
                    .flatten()
                    .any(|block| !parse_downlink_mac_pdus_from_block(block).is_empty()) =>
            {
                Some(slot.ts)
            }
            _ => None,
        })
        .collect();
    assert!(
        downlink_mac_pdus_at(&sink_msgs, TdmaTime { t: 1, f: 3, m: 1, h: 0 })
            .iter()
            .any(|pdu| {
                matches!(
                    pdu,
                    DownlinkMacPdu::Resource(LogicalChannel::SchF, resource)
                        if resource.addr.is_some_and(|addr| addr.ssi == all_ones_gssi)
                            && resource.length_ind == 0b111111
                )
            }),
        "fragmented all-ones broadcast should start when all active EG listeners share the receive frame; observed MAC PDU times: {:?}",
        observed_mac_times
    );
    let has_fragment_continuation_at = |ts| {
        downlink_mac_pdus_at(&sink_msgs, ts).iter().any(|pdu| {
            matches!(
                pdu,
                DownlinkMacPdu::Frag(LogicalChannel::SchF) | DownlinkMacPdu::End(LogicalChannel::SchF, _)
            )
        })
    };
    assert!(
        !has_fragment_continuation_at(TdmaTime { t: 1, f: 4, m: 1, h: 0 }),
        "all-ones broadcast fragments must not transmit outside the active EG receive frame"
    );
    assert!(
        !has_fragment_continuation_at(TdmaTime { t: 1, f: 5, m: 1, h: 0 }),
        "all-ones broadcast fragments must wait when only one active-batch ISSI is listening"
    );
    assert!(
        !has_fragment_continuation_at(TdmaTime { t: 1, f: 7, m: 1, h: 0 }),
        "all-ones broadcast fragments must keep waiting through the next partial active-batch receive frame"
    );
    assert!(
        has_fragment_continuation_at(TdmaTime { t: 1, f: 9, m: 1, h: 0 }),
        "all-ones broadcast fragments should resume when the active EG batch shares a receive frame again"
    );

    let state = test.config.state_read();
    for issi in [first_issi, second_issi] {
        assert_eq!(
            state.energy_saving.get(&issi).and_then(|assignment| assignment.awake_until),
            None,
            "EN 300 392-2 clause 23.7.6 excludes all-ones broadcast from T.210 suspension"
        );
    }
}

#[test]
fn test_all_ones_mle_bl_udata_reports_success_after_active_eg_batch() {
    debug::setup_logging_verbose();

    let start = TdmaTime { t: 3, f: 2, m: 1, h: 0 };
    let req_handle = 58;
    let all_ones_gssi = 0xFF_FFFF;
    let issi = 1401;
    let tx_reporter = TxReporter::new_unacked();
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        state.subscribers.register(issi);
        state.energy_saving.insert(
            issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);

    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle,
            pdu: llc_wrapped_mle_sdu(75),
            main_address: TetraAddress::new(all_ones_gssi, SsiType::Gssi),
            endpoint_id: 0,
            pdu_prio: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: None,
            tx_reporter: Some(tx_reporter.clone()),
        }),
    });

    test.run_stack(Some(8));
    let sink_msgs = test.dump_sinks();

    assert!(
        downlink_mac_pdus_at(&sink_msgs, TdmaTime { t: 1, f: 3, m: 1, h: 0 })
            .iter()
            .any(|pdu| matches!(
                pdu,
                DownlinkMacPdu::Resource(LogicalChannel::SchF, resource)
                    if resource.addr.is_some_and(|addr| addr.ssi == all_ones_gssi)
            )),
        "all-ones MLE BL-UDATA should transmit in the active EG receive frame"
    );
    assert_eq!(tx_reporter.get_state(), TxState::Transmitted);
    assert!(
        matches!(
            tma_report_for_handle(&sink_msgs, req_handle),
            Some(TmaReport::SuccessReservedOrStealing)
        ),
        "EN 300 392-2 clauses 20.4.1.1.3 and 22.3.2.4.1 require a complete-transmission report after the BL-UDATA MAC request is sent"
    );
}

#[test]
fn test_all_ones_fragmented_channel_allocation_mac_end_waits_for_full_active_eg_batch() {
    debug::setup_logging_verbose();

    let start = TdmaTime { t: 3, f: 2, m: 1, h: 0 };
    let all_ones_gssi = 0xFF_FFFF;
    let first_issi = 1401;
    let second_issi = 1402;
    let mut timeslots = [false; 4];
    timeslots[1] = true;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        state.subscribers.register(first_issi);
        state.subscribers.register(second_issi);
        state.energy_saving.insert(
            first_issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
        state.energy_saving.insert(
            second_issi,
            EnergySavingAssignment {
                mode: 3,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 50,
            pdu: BitBuffer::from_bitstr(&alternating_bits(300)),
            main_address: TetraAddress::new(all_ones_gssi, SsiType::Gssi),
            endpoint_id: 1,
            pdu_prio: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: Some(CmceChanAllocReq {
                usage: Some(4),
                timeslots,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
                carrier: None,
            }),
            tx_reporter: None,
        }),
    });

    test.run_stack(Some(28));
    let sink_msgs = test.dump_sinks();
    let observed_mac_times: Vec<TdmaTime> = sink_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmvUnitdataReq(slot)
                if [&slot.blk1, &slot.blk2]
                    .into_iter()
                    .flatten()
                    .any(|block| !parse_downlink_mac_pdus_from_block(block).is_empty()) =>
            {
                Some(slot.ts)
            }
            _ => None,
        })
        .collect();

    let start_pdus = downlink_mac_pdus_at(&sink_msgs, TdmaTime { t: 1, f: 3, m: 1, h: 0 });
    let start_resource = start_pdus
        .iter()
        .find_map(|pdu| match pdu {
            DownlinkMacPdu::Resource(LogicalChannel::SchF, resource)
                if resource.addr.is_some_and(|addr| addr.ssi == all_ones_gssi) && resource.length_ind == 0b111111 =>
            {
                Some(resource)
            }
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "fragmented all-ones broadcast should start in the shared EG receive frame; observed MAC PDU times: {:?}",
                observed_mac_times
            )
        });
    assert!(
        start_resource.chan_alloc_element.is_none(),
        "EN 300 392-2 clause 23.4.2.1.1 carries fragmented channel allocation in MAC-END, not MAC-RESOURCE"
    );

    let has_fragment_continuation_at = |ts| {
        downlink_mac_pdus_at(&sink_msgs, ts).iter().any(|pdu| {
            matches!(
                pdu,
                DownlinkMacPdu::Frag(LogicalChannel::SchF) | DownlinkMacPdu::End(LogicalChannel::SchF, _)
            )
        })
    };
    assert!(
        !has_fragment_continuation_at(TdmaTime { t: 1, f: 4, m: 1, h: 0 }),
        "fragment continuations must not transmit outside the active EG receive frame"
    );
    assert!(
        !has_fragment_continuation_at(TdmaTime { t: 1, f: 5, m: 1, h: 0 }),
        "fragment continuations must wait when only one active-batch ISSI is listening"
    );
    assert!(
        !has_fragment_continuation_at(TdmaTime { t: 1, f: 7, m: 1, h: 0 }),
        "fragment continuations must keep waiting through the next partial active-batch receive frame"
    );

    let end_pdus = downlink_mac_pdus_at(&sink_msgs, TdmaTime { t: 1, f: 9, m: 1, h: 0 });
    let mac_end = end_pdus
        .iter()
        .find_map(|pdu| match pdu {
            DownlinkMacPdu::End(LogicalChannel::SchF, mac_end) => Some(mac_end),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "fragmented all-ones broadcast should emit MAC-END in the next shared EG receive frame; observed MAC PDU times: {:?}",
                observed_mac_times
            )
        });
    let chan_alloc = mac_end
        .chan_alloc_element
        .as_ref()
        .expect("fragmented all-ones channel allocation must be carried in MAC-END");
    assert_eq!(chan_alloc.alloc_type, ChanAllocType::Replace);
    assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Both);
    assert_eq!(chan_alloc.ts_assigned, timeslots);
    assert!(chan_alloc.clch_permission);
    assert_eq!(chan_alloc.carrier_num, test.config.config().cell.main_carrier);

    let state = test.config.state_read();
    for issi in [first_issi, second_issi] {
        assert_eq!(
            state.energy_saving.get(&issi).and_then(|assignment| assignment.awake_until),
            None,
            "EN 300 392-2 clause 23.7.6 excludes all-ones broadcast from T.210 suspension"
        );
    }
}

#[test]
fn test_facch_stealing_preserves_channel_allocation_in_stch_mac_resource() {
    debug::setup_logging_verbose();

    let target_issi = 30129;
    let traffic_ts = 2;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default().add_timeslots(2)));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg(91, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let mut timeslots = [false; 4];
    timeslots[(traffic_ts - 1) as usize] = true;
    let test_prim = TmaUnitdataReq {
        req_handle: 7,
        pdu: BitBuffer::from_bitstr("10101010"),
        main_address: TetraAddress::new(target_issi, SsiType::Issi),
        endpoint_id: 1,
        pdu_prio: 0,
        stealing_permission: true,
        subscriber_class: 0,
        air_interface_encryption: None,
        stealing_repeats_flag: None,
        data_category: None,
        chan_alloc: Some(CmceChanAllocReq {
            usage: Some(4),
            timeslots,
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
            carrier: None,
        }),
        tx_reporter: None,
    };
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(test_prim),
    });

    test.run_stack(Some(4));
    let sink_msgs = test.dump_sinks();
    let mac_resource = first_mac_resource_for_addr(&sink_msgs, TetraAddress::issi(target_issi))
        .expect("expected STCH MAC-RESOURCE addressed to target ISSI");
    let chan_alloc = mac_resource
        .chan_alloc_element
        .expect("FACCH/STCH MAC-RESOURCE should preserve CMCE channel allocation");

    // EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6: assigned-channel
    // downlink signalling must carry the allocation that keeps an EG MS active
    // on the assigned channel rather than relying on common-control listening.
    assert_eq!(chan_alloc.alloc_type, ChanAllocType::Replace);
    assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Both);
    assert_eq!(chan_alloc.ts_assigned, timeslots);
    assert!(chan_alloc.clch_permission);
    assert_eq!(chan_alloc.carrier_num, test.config.config().cell.main_carrier);
    assert_eq!(mac_resource.usage_marker, Some(4));
}

#[test]
fn test_group_listener_floor_grant_with_speaker_id_stays_on_stch() {
    debug::setup_logging_verbose();

    let speaker_issi = 2_260_082;
    let gssi = 226_333;
    let traffic_ts = 2;
    let call_id = 6;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default().add_timeslots(2)));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg_with_secondary_speaker(gssi, speaker_issi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let d_tx_granted = DTxGranted {
        call_identifier: call_id,
        transmission_grant: TransmissionGrant::GrantedToOtherUser.into_raw() as u8,
        transmission_request_permission: false,
        encryption_control: false,
        reserved: false,
        notification_indicator: None,
        transmitting_party_type_identifier: Some(1),
        transmitting_party_address_ssi: Some(speaker_issi as u64),
        transmitting_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };
    let mut pdu = BitBuffer::new_autoexpand(64);
    d_tx_granted.to_bitbuf(&mut pdu).expect("serialize speaker-qualified D-TX GRANTED");
    pdu.seek(0);

    let mut timeslots = [false; 4];
    timeslots[(traffic_ts - 1) as usize] = true;
    let reporter = TxReporter::new_unacked();
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 74,
            pdu,
            main_address: TetraAddress::new(gssi, SsiType::Gssi),
            endpoint_id: 0,
            pdu_prio: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: Some(CmceChanAllocReq {
                usage: Some(6),
                timeslots,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Dl,
                carrier: None,
            }),
            tx_reporter: Some(reporter.clone()),
        }),
    });

    test.run_stack(Some(8));
    let sink_msgs = test.dump_sinks();
    let resources = mac_resources_for_addr(&sink_msgs, TetraAddress::new(gssi, SsiType::Gssi));
    let group_stch = resources
        .iter()
        .find(|(logical_channel, _)| *logical_channel == LogicalChannel::Stch)
        .unwrap_or_else(|| {
            panic!(
                "expected speaker-qualified GSSI D-TX GRANTED to remain on assigned-channel STCH; reporter={:?}; resources={resources:?}",
                reporter.get_state()
            )
        });

    assert_eq!(
        group_stch.1.usage_marker,
        Some(6),
        "EN 300 392-2 clauses 14.5.2.2.1 and 23.8.1: GSSI listener grant should keep the active traffic usage marker"
    );
    assert!(
        group_stch.1.chan_alloc_element.is_none(),
        "speaker-qualified GSSI D-TX GRANTED must omit redundant MAC channel allocation so it fits STCH for assigned-channel listeners"
    );
    assert!(
        !group_stch.1.random_access_flag,
        "a GSSI listener grant must not acknowledge one ISSI's random access to every member of a large group"
    );
}

#[test]
fn test_private_floor_grant_stch_carries_preserved_random_access_ack() {
    debug::setup_logging_verbose();

    let caller_issi = 2_260_618;
    let called_issi = 2_260_616;
    let traffic_ts = 2;
    let call_id = 5;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(private_call_open_msg(caller_issi, called_issi, traffic_ts));
    test.submit_message(floor_released_msg(call_id, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    {
        let umac = test
            .router
            .get_entity(TetraEntity::Umac)
            .expect("UMAC entity should be registered")
            .as_any_mut()
            .downcast_mut::<UmacBs>()
            .expect("registered UMAC should be UmacBs");
        umac.channel_scheduler
            .dl_enqueue_random_access_ack(traffic_ts, TetraAddress::issi(called_issi));
        assert!(
            umac.channel_scheduler.dl_drop_all_except_stolen(traffic_ts),
            "test setup should preserve a hangtime random-access ACK for the next STCH"
        );
    }

    let mut grant_sdu = BitBuffer::new_autoexpand(40);
    DTxGranted {
        call_identifier: call_id,
        transmission_grant: TransmissionGrant::Granted.into_raw() as u8,
        transmission_request_permission: false,
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
    }
    .to_bitbuf(&mut grant_sdu)
    .expect("serialize compact D-TX GRANTED");
    grant_sdu.seek(0);

    let mut timeslots = [false; 4];
    timeslots[(traffic_ts - 1) as usize] = true;
    let ack_reporter = TxReporter::new_unacked();
    let grant_reporter = TxReporter::new_unacked();

    test.submit_message(floor_granted_msg(call_id, called_issi, caller_issi, traffic_ts));
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 70,
            pdu: BitBuffer::from_bitstr("00110"),
            main_address: TetraAddress::issi(called_issi),
            endpoint_id: 0,
            pdu_prio: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: None,
            tx_reporter: Some(ack_reporter.clone()),
        }),
    });
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 71,
            pdu: grant_sdu,
            main_address: TetraAddress::issi(called_issi),
            endpoint_id: 0,
            pdu_prio: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: Some(CmceChanAllocReq {
                usage: Some(5),
                timeslots,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
                carrier: None,
            }),
            tx_reporter: Some(grant_reporter.clone()),
        }),
    });

    test.run_stack(Some(24));
    let sink_msgs = test.dump_sinks();
    let all_pdus = downlink_mac_pdus(&sink_msgs);
    let resources = mac_resources_for_addr(&sink_msgs, TetraAddress::issi(called_issi));

    let ack_only = resources
        .iter()
        .find(|(logical_channel, resource)| {
            *logical_channel == LogicalChannel::Stch && resource.chan_alloc_element.is_none() && resource.usage_marker.is_none()
        })
        .expect("expected first ACK-only STCH for the requester");
    assert!(
        ack_only.1.random_access_flag,
        "EN 300 392-2 clause 21.4.3.1: first STCH MAC-RESOURCE after random access should acknowledge the requester without consuming the preserved floor-grant ACK"
    );

    let grant = resources
        .iter()
        .find(|(logical_channel, resource)| {
            *logical_channel == LogicalChannel::Stch
                && resource.chan_alloc_element.is_none()
                && resource.random_access_flag
                && resource.usage_marker == Some(5)
        })
        .unwrap_or_else(|| {
            panic!(
                "expected private D-TX GRANTED STCH without redundant MAC channel allocation; ack_reporter={:?}; grant_reporter={:?}; resources={resources:?}; all_pdus={all_pdus:?}",
                ack_reporter.get_state(),
                grant_reporter.get_state()
            )
        });
    assert!(
        grant.1.random_access_flag,
        "EN 300 392-2 clauses 21.4.3.1, 14.5.1.2.1 b) and 23.5.2.2.1: the private-call floor grant STCH should carry the random-access ACK that lets the requesting MS continue onto the assigned channel"
    );
    assert!(
        grant.1.chan_alloc_element.is_none(),
        "EN 300 392-2 clauses 14.5.1.2.1 b), 14.5.1.4.2 and 23.5: when the private traffic channel is already assigned, D-TX GRANTED may switch U-plane without repeating a MAC channel allocation"
    );
}

#[test]
fn test_group_floor_grant_stch_repeats_preserved_random_access_ack_for_requester() {
    debug::setup_logging_verbose();

    let first_speaker_issi = 2_260_618;
    let requester_issi = 2_260_082;
    let gssi = 226_333;
    let traffic_ts = 2;
    let call_id = 6;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg_with_secondary_speaker(gssi, first_speaker_issi, traffic_ts));
    test.submit_message(floor_released_msg(call_id, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    {
        let umac = test
            .router
            .get_entity(TetraEntity::Umac)
            .expect("UMAC entity should be registered")
            .as_any_mut()
            .downcast_mut::<UmacBs>()
            .expect("registered UMAC should be UmacBs");
        umac.channel_scheduler
            .dl_enqueue_random_access_ack(traffic_ts, TetraAddress::issi(requester_issi));
        assert!(
            umac.channel_scheduler.dl_drop_all_except_stolen(traffic_ts),
            "test setup should preserve the requester's hangtime random-access ACK for STCH"
        );
    }

    let mut grant_sdu = BitBuffer::new_autoexpand(40);
    DTxGranted {
        call_identifier: call_id,
        transmission_grant: TransmissionGrant::Granted.into_raw() as u8,
        transmission_request_permission: false,
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
    }
    .to_bitbuf(&mut grant_sdu)
    .expect("serialize compact D-TX GRANTED");
    grant_sdu.seek(0);

    let mut timeslots = [false; 4];
    timeslots[(traffic_ts - 1) as usize] = true;
    let ack_reporter = TxReporter::new_unacked();
    let grant_reporter = TxReporter::new_unacked();

    test.submit_message(floor_granted_msg(call_id, requester_issi, gssi, traffic_ts));
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 72,
            pdu: BitBuffer::from_bitstr("00111"),
            main_address: TetraAddress::issi(requester_issi),
            endpoint_id: 0,
            pdu_prio: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: None,
            tx_reporter: Some(ack_reporter.clone()),
        }),
    });
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 73,
            pdu: grant_sdu,
            main_address: TetraAddress::issi(requester_issi),
            endpoint_id: 0,
            pdu_prio: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: Some(CmceChanAllocReq {
                usage: Some(6),
                timeslots,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
                carrier: None,
            }),
            tx_reporter: Some(grant_reporter.clone()),
        }),
    });

    test.run_stack(Some(24));
    let sink_msgs = test.dump_sinks();
    let all_pdus = downlink_mac_pdus(&sink_msgs);
    let resources = mac_resources_for_addr(&sink_msgs, TetraAddress::issi(requester_issi));

    let ack_only = resources
        .iter()
        .find(|(logical_channel, resource)| *logical_channel == LogicalChannel::Stch && resource.chan_alloc_element.is_none())
        .expect("expected first ACK-only STCH for the group requester");
    assert!(
        ack_only.1.random_access_flag,
        "EN 300 392-2 clauses 21.4.3.1 and 14.5.2.2.1 b): group requester should see MAC random-access acknowledgement before the floor-grant STCH"
    );

    let grant = resources
        .iter()
        .find(|(logical_channel, resource)| {
            *logical_channel == LogicalChannel::Stch && resource.chan_alloc_element.is_some()
        })
        .unwrap_or_else(|| panic!(
            "expected requester D-TX GRANTED STCH with channel allocation; ack_reporter={:?}; grant_reporter={:?}; resources={resources:?}; all_pdus={all_pdus:?}",
            ack_reporter.get_state(),
            grant_reporter.get_state()
        ));
    assert!(
        grant.1.random_access_flag,
        "EN 300 392-2 clauses 21.4.3.1, 14.5.2.2.1 b) and 23.5.2.2.1: group D-TX GRANTED should repeat the preserved random-access ACK for the MS entering U-plane"
    );
    assert_eq!(
        grant
            .1
            .chan_alloc_element
            .as_ref()
            .expect("grant should keep channel allocation")
            .ul_dl_assigned,
        UlDlAssignment::Both
    );
}

#[test]
fn test_group_requester_d_tx_granted_stch_consumes_ready_random_access_ack() {
    debug::setup_logging_verbose();

    let first_speaker_issi = 2_260_616;
    let requester_issi = 2_260_082;
    let gssi = 226_333;
    let traffic_ts = 2;
    let call_id = 6;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg_with_secondary_speaker(gssi, first_speaker_issi, traffic_ts));
    test.submit_message(floor_released_msg(call_id, traffic_ts));
    test.run_stack(Some(2));
    let _ = test.dump_sinks();

    {
        let umac = test
            .router
            .get_entity(TetraEntity::Umac)
            .expect("UMAC entity should be registered")
            .as_any_mut()
            .downcast_mut::<UmacBs>()
            .expect("registered UMAC should be UmacBs");
        umac.channel_scheduler
            .dl_enqueue_random_access_ack(traffic_ts, TetraAddress::issi(requester_issi));
    }

    let mut timeslots = [false; 4];
    timeslots[(traffic_ts - 1) as usize] = true;
    let grant_reporter = TxReporter::new_unacked();
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 72,
            pdu: d_tx_granted_sdu(call_id, TransmissionGrant::Granted),
            main_address: TetraAddress::issi(requester_issi),
            endpoint_id: 0,
            pdu_prio: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: Some(CmceChanAllocReq {
                usage: Some(6),
                timeslots,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
                carrier: None,
            }),
            tx_reporter: Some(grant_reporter.clone()),
        }),
    });

    test.run_stack(Some(16));
    let sink_msgs = test.dump_sinks();
    let resources = mac_resources_for_addr(&sink_msgs, TetraAddress::issi(requester_issi));
    let grant = resources
        .iter()
        .find(|(logical_channel, resource)| *logical_channel == LogicalChannel::Stch && resource.chan_alloc_element.is_some())
        .unwrap_or_else(|| {
            panic!(
                "expected requester positive D-TX GRANTED STCH with channel allocation; reporter={:?}; resources={resources:?}",
                grant_reporter.get_state()
            )
        });
    assert!(
        grant.1.random_access_flag,
        "EN 300 392-2 clauses 21.4.3.1 and 14.5.2.2.1 b): real-ordering group requester D-TX GRANTED must acknowledge the U-TX DEMAND random access even before UMAC FloorGranted clears hangtime"
    );
    assert_eq!(
        grant
            .1
            .chan_alloc_element
            .as_ref()
            .expect("grant should keep channel allocation")
            .ul_dl_assigned,
        UlDlAssignment::Both
    );
    assert_eq!(grant_reporter.get_state(), TxState::Transmitted);
}

#[test]
fn test_large_group_ptt_storm_prioritizes_requester_grant_with_preserved_ra_ack() {
    debug::setup_logging_verbose();

    let first_speaker_issi = 2_260_618;
    let requester_issi = 2_260_082;
    let gssi = 226_333;
    let traffic_ts = 2;
    let call_id = 6;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg_with_secondary_speaker(gssi, first_speaker_issi, traffic_ts));
    test.submit_message(floor_released_msg(call_id, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    {
        let umac = test
            .router
            .get_entity(TetraEntity::Umac)
            .expect("UMAC entity should be registered")
            .as_any_mut()
            .downcast_mut::<UmacBs>()
            .expect("registered UMAC should be UmacBs");
        umac.channel_scheduler
            .dl_enqueue_random_access_ack(traffic_ts, TetraAddress::issi(requester_issi));
        assert!(
            umac.channel_scheduler.dl_drop_all_except_stolen(traffic_ts),
            "test setup should preserve the requester's random-access ACK through hangtime cleanup"
        );
    }

    let mut timeslots = [false; 4];
    timeslots[(traffic_ts - 1) as usize] = true;
    let make_d_tx_granted_sdu = |transmission_grant: TransmissionGrant| {
        let mut sdu = BitBuffer::new_autoexpand(40);
        DTxGranted {
            call_identifier: call_id,
            transmission_grant: transmission_grant.into_raw() as u8,
            transmission_request_permission: false,
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
        }
        .to_bitbuf(&mut sdu)
        .expect("serialize D-TX GRANTED");
        sdu.seek(0);
        sdu
    };

    // EN 300 392-2 clause 14.5.2.2.1 allows queued/not-granted floor
    // responses while another MS owns the floor. Under a large GSSI storm,
    // those lower-value responses must not delay the positive grant that lets
    // the requester enter the assigned-channel U-plane.
    let busy_count = 4096;
    for offset in 0..busy_count {
        let busy_issi = 3_100_000 + offset;
        test.submit_message(SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
                req_handle: 10_000 + offset as i32,
                pdu: make_d_tx_granted_sdu(TransmissionGrant::NotGranted),
                main_address: TetraAddress::issi(busy_issi),
                endpoint_id: 0,
                pdu_prio: 0,
                stealing_permission: true,
                subscriber_class: 0,
                air_interface_encryption: None,
                stealing_repeats_flag: None,
                data_category: None,
                chan_alloc: None,
                tx_reporter: None,
            }),
        });
    }

    let grant_reporter = TxReporter::new_unacked();
    test.submit_message(floor_granted_msg(call_id, requester_issi, gssi, traffic_ts));
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 20_000,
            pdu: make_d_tx_granted_sdu(TransmissionGrant::Granted),
            main_address: TetraAddress::issi(requester_issi),
            endpoint_id: 0,
            pdu_prio: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: Some(CmceChanAllocReq {
                usage: Some(6),
                timeslots,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
                carrier: None,
            }),
            tx_reporter: Some(grant_reporter.clone()),
        }),
    });

    test.run_stack(Some(48));
    let sink_msgs = test.dump_sinks();
    let all_pdus = downlink_mac_pdus(&sink_msgs);

    let requester_grant_index = all_pdus
        .iter()
        .position(|pdu| match pdu {
            DownlinkMacPdu::Resource(LogicalChannel::Stch, resource)
                if resource
                    .addr
                    .is_some_and(|addr| mac_resource_matches_addr(addr, TetraAddress::issi(requester_issi)))
                    && resource.chan_alloc_element.is_some() =>
            {
                true
            }
            _ => false,
        })
        .unwrap_or_else(|| {
            panic!(
                "expected requester positive D-TX GRANTED STCH with channel allocation; reporter={:?}; pdus={all_pdus:?}",
                grant_reporter.get_state()
            )
        });
    let first_busy_index = all_pdus.iter().position(|pdu| match pdu {
        DownlinkMacPdu::Resource(LogicalChannel::Stch, resource)
            if resource.addr.is_some_and(|addr| {
                matches!(addr.ssi_type, SsiType::Issi | SsiType::Ssi) && (3_100_000..3_100_000 + busy_count).contains(&addr.ssi)
            }) =>
        {
            true
        }
        _ => false,
    });

    if let Some(first_busy_index) = first_busy_index {
        assert!(
            requester_grant_index < first_busy_index,
            "positive requester floor grant must transmit before lower-value busy floor responses"
        );
    }

    let DownlinkMacPdu::Resource(_, requester_grant) = &all_pdus[requester_grant_index] else {
        unreachable!("requester_grant_index was selected from Resource variants");
    };
    assert!(
        requester_grant.random_access_flag,
        "EN 300 392-2 clause 21.4.3.1: positive group floor grant must preserve the requester's random-access ACK"
    );
    assert_eq!(
        requester_grant
            .chan_alloc_element
            .as_ref()
            .expect("requester grant should carry assigned-channel allocation")
            .ul_dl_assigned,
        UlDlAssignment::Both
    );
}

#[test]
fn test_large_group_ptt_storm_prioritizes_llc_wrapped_requester_grant_with_preserved_ra_ack() {
    debug::setup_logging_verbose();

    let first_speaker_issi = 2_260_618;
    let requester_issi = 2_260_082;
    let gssi = 226_333;
    let traffic_ts = 2;
    let call_id = 6;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg_with_secondary_speaker(gssi, first_speaker_issi, traffic_ts));
    test.submit_message(floor_released_msg(call_id, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    {
        let umac = test
            .router
            .get_entity(TetraEntity::Umac)
            .expect("UMAC entity should be registered")
            .as_any_mut()
            .downcast_mut::<UmacBs>()
            .expect("registered UMAC should be UmacBs");
        umac.channel_scheduler
            .dl_enqueue_random_access_ack(traffic_ts, TetraAddress::issi(requester_issi));
        assert!(
            umac.channel_scheduler.dl_drop_all_except_stolen(traffic_ts),
            "test setup should preserve the requester's random-access ACK through hangtime cleanup"
        );
    }

    let mut timeslots = [false; 4];
    timeslots[(traffic_ts - 1) as usize] = true;
    let make_wrapped_d_tx_granted_sdu =
        |transmission_grant: TransmissionGrant| llc_wrapped_cmce_sdu(d_tx_granted_sdu(call_id, transmission_grant));

    // EN 300 392-2 clauses 20.4.1.1.3, 22.3.2.4.1 and 14.5.2.2.1:
    // real CMCE floor control reaches UMAC as LLC BL-UDATA plus an MLE CMCE
    // discriminator. The bounded TMA queue must still admit the positive
    // floor grant that lets the requester transmit when thousands of lower
    // priority responses are already pending.
    let busy_count = 4096;
    for offset in 0..busy_count {
        let busy_issi = 3_200_000 + offset;
        test.submit_message(SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
                req_handle: 30_000 + offset as i32,
                pdu: make_wrapped_d_tx_granted_sdu(TransmissionGrant::NotGranted),
                main_address: TetraAddress::issi(busy_issi),
                endpoint_id: 0,
                pdu_prio: 0,
                stealing_permission: true,
                subscriber_class: 0,
                air_interface_encryption: None,
                stealing_repeats_flag: None,
                data_category: None,
                chan_alloc: None,
                tx_reporter: None,
            }),
        });
    }

    let grant_reporter = TxReporter::new_unacked();
    test.submit_message(floor_granted_msg(call_id, requester_issi, gssi, traffic_ts));
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 40_000,
            pdu: make_wrapped_d_tx_granted_sdu(TransmissionGrant::Granted),
            main_address: TetraAddress::issi(requester_issi),
            endpoint_id: 0,
            pdu_prio: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: Some(CmceChanAllocReq {
                usage: Some(6),
                timeslots,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
                carrier: None,
            }),
            tx_reporter: Some(grant_reporter.clone()),
        }),
    });

    test.run_stack(Some(48));
    let sink_msgs = test.dump_sinks();
    let all_pdus = downlink_mac_pdus(&sink_msgs);

    let requester_grant_index = all_pdus
        .iter()
        .position(|pdu| match pdu {
            DownlinkMacPdu::Resource(LogicalChannel::Stch, resource)
                if resource
                    .addr
                    .is_some_and(|addr| mac_resource_matches_addr(addr, TetraAddress::issi(requester_issi)))
                    && resource.chan_alloc_element.is_some() =>
            {
                true
            }
            _ => false,
        })
        .unwrap_or_else(|| {
            panic!(
                "expected wrapped requester positive D-TX GRANTED STCH with channel allocation; reporter={:?}; pdus={all_pdus:?}",
                grant_reporter.get_state()
            )
        });
    let first_busy_index = all_pdus.iter().position(|pdu| match pdu {
        DownlinkMacPdu::Resource(LogicalChannel::Stch, resource)
            if resource.addr.is_some_and(|addr| {
                matches!(addr.ssi_type, SsiType::Issi | SsiType::Ssi) && (3_200_000..3_200_000 + busy_count).contains(&addr.ssi)
            }) =>
        {
            true
        }
        _ => false,
    });

    if let Some(first_busy_index) = first_busy_index {
        assert!(
            requester_grant_index < first_busy_index,
            "wrapped positive requester floor grant must transmit before lower-value busy floor responses"
        );
    }

    let DownlinkMacPdu::Resource(_, requester_grant) = &all_pdus[requester_grant_index] else {
        unreachable!("requester_grant_index was selected from Resource variants");
    };
    assert!(
        requester_grant.random_access_flag,
        "EN 300 392-2 clause 21.4.3.1: wrapped positive group floor grant must preserve the requester's random-access ACK"
    );
    assert_eq!(
        requester_grant
            .chan_alloc_element
            .as_ref()
            .expect("wrapped requester grant should carry assigned-channel allocation")
            .ul_dl_assigned,
        UlDlAssignment::Both
    );
    assert_eq!(grant_reporter.get_state(), TxState::Transmitted);
}

#[test]
fn test_large_group_ptt_storm_admits_llc_wrapped_listener_floor_grant() {
    debug::setup_logging_verbose();

    let first_speaker_issi = 2_260_618;
    let new_speaker_issi = 2_260_082;
    let gssi = 226_333;
    let traffic_ts = 2;
    let call_id = 6;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg_with_secondary_speaker(gssi, first_speaker_issi, traffic_ts));
    test.submit_message(floor_granted_msg(call_id, new_speaker_issi, gssi, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let make_wrapped_d_tx_granted_sdu =
        |transmission_grant: TransmissionGrant| llc_wrapped_cmce_sdu(d_tx_granted_sdu(call_id, transmission_grant));

    let busy_count = 4096;
    for offset in 0..busy_count {
        let busy_issi = 3_300_000 + offset;
        test.submit_message(SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
                req_handle: 50_000 + offset as i32,
                pdu: make_wrapped_d_tx_granted_sdu(TransmissionGrant::NotGranted),
                main_address: TetraAddress::issi(busy_issi),
                endpoint_id: 0,
                pdu_prio: 0,
                stealing_permission: true,
                subscriber_class: 0,
                air_interface_encryption: None,
                stealing_repeats_flag: None,
                data_category: None,
                chan_alloc: None,
                tx_reporter: None,
            }),
        });
    }

    let mut timeslots = [false; 4];
    timeslots[(traffic_ts - 1) as usize] = true;
    let listener_reporter = TxReporter::new_unacked();
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 60_000,
            pdu: make_wrapped_d_tx_granted_sdu(TransmissionGrant::GrantedToOtherUser),
            main_address: TetraAddress::new(gssi, SsiType::Gssi),
            endpoint_id: 0,
            pdu_prio: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: Some(CmceChanAllocReq {
                usage: Some(6),
                timeslots,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Dl,
                carrier: None,
            }),
            tx_reporter: Some(listener_reporter.clone()),
        }),
    });

    test.run_stack(Some(48));
    let sink_msgs = test.dump_sinks();
    let all_pdus = downlink_mac_pdus(&sink_msgs);
    let listener_grant = all_pdus
        .iter()
        .find_map(|pdu| match pdu {
            DownlinkMacPdu::Resource(LogicalChannel::Stch, resource)
                if resource.addr.is_some_and(|addr| addr.ssi == gssi) && resource.chan_alloc_element.is_some() =>
            {
                Some(resource)
            }
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "expected wrapped GSSI D-TX GRANTED/GrantedToOtherUser STCH under full TMA backlog; reporter={:?}; pdus={all_pdus:?}",
                listener_reporter.get_state()
            )
        });
    assert_eq!(
        listener_grant
            .chan_alloc_element
            .as_ref()
            .expect("listener floor grant should carry DL channel allocation")
            .ul_dl_assigned,
        UlDlAssignment::Dl
    );
    assert_eq!(listener_reporter.get_state(), TxState::Transmitted);

    // EN 300 392-2 clause 14.5.2.2.1 b): group listeners need the
    // GrantedToOtherUser notification for coherent floor state. Admission must
    // not drop it behind thousands of lower-value busy responses before the
    // scheduler can transmit it.
}

#[test]
fn test_large_group_ptt_storm_mixed_eg7_stayalive_keeps_requester_and_listener_floor_grants() {
    debug::setup_logging_verbose();

    let gssi = 226_333;
    let first_issi = 3_400_000;
    let first_speaker_issi = first_issi;
    let requester_issi = first_issi + 2;
    let stayalive_listener_issi = first_issi + 1;
    let traffic_ts = 2;
    let call_id = 6;
    let member_count = 4096;
    let start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        for offset in 0..member_count {
            let issi = first_issi + offset;
            state.subscribers.register(issi);
            assert!(state.subscribers.affiliate(issi, gssi));
            if offset % 2 == 0 {
                state.energy_saving.insert(issi, eg_assignment(start));
            }
        }
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg_with_secondary_speaker(gssi, first_speaker_issi, traffic_ts));
    test.submit_message(floor_released_msg(call_id, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    {
        let state = test.config.state_read();
        assert_eq!(state.subscribers.group_members(gssi).len(), member_count as usize);
        assert_eq!(
            state
                .energy_saving
                .get(&requester_issi)
                .expect("requester should be an EG7 member in this mixed group")
                .suspension_count,
            1,
            "assigned-channel group call should suspend the EG7 requester before floor-control STCH"
        );
        assert!(
            !state.energy_saving.contains_key(&stayalive_listener_issi),
            "odd-offset member intentionally represents StayAlive/no EG scheduler state"
        );
    }

    {
        let umac = test
            .router
            .get_entity(TetraEntity::Umac)
            .expect("UMAC entity should be registered")
            .as_any_mut()
            .downcast_mut::<UmacBs>()
            .expect("registered UMAC should be UmacBs");
        umac.channel_scheduler
            .dl_enqueue_random_access_ack(traffic_ts, TetraAddress::issi(requester_issi));
        assert!(
            umac.channel_scheduler.dl_drop_all_except_stolen(traffic_ts),
            "test setup should preserve the requester's random-access ACK through hangtime cleanup"
        );
    }

    let mut timeslots = [false; 4];
    timeslots[(traffic_ts - 1) as usize] = true;
    let make_wrapped_d_tx_granted_sdu =
        |transmission_grant: TransmissionGrant| llc_wrapped_cmce_sdu(d_tx_granted_sdu(call_id, transmission_grant));

    // EN 300 392-2 clauses 14.5.2.2.1, 21.4.3.1, 23.5, and 23.7.6:
    // group floor-control must keep the requester grant and the GSSI listener
    // notification deliverable even when thousands of lower-value busy
    // responses are queued and half the GSSI is in EG7.
    let busy_count = 4096;
    for offset in 0..busy_count {
        let busy_issi = 3_500_000 + offset;
        test.submit_message(SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
                req_handle: 70_000 + offset as i32,
                pdu: make_wrapped_d_tx_granted_sdu(TransmissionGrant::NotGranted),
                main_address: TetraAddress::issi(busy_issi),
                endpoint_id: 0,
                pdu_prio: 0,
                stealing_permission: true,
                subscriber_class: 0,
                air_interface_encryption: None,
                stealing_repeats_flag: None,
                data_category: None,
                chan_alloc: None,
                tx_reporter: None,
            }),
        });
    }

    let requester_reporter = TxReporter::new_unacked();
    let listener_reporter = TxReporter::new_unacked();
    test.submit_message(floor_granted_msg(call_id, requester_issi, gssi, traffic_ts));
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 80_000,
            pdu: make_wrapped_d_tx_granted_sdu(TransmissionGrant::Granted),
            main_address: TetraAddress::issi(requester_issi),
            endpoint_id: 0,
            pdu_prio: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: Some(CmceChanAllocReq {
                usage: Some(6),
                timeslots,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
                carrier: None,
            }),
            tx_reporter: Some(requester_reporter.clone()),
        }),
    });
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 80_001,
            pdu: make_wrapped_d_tx_granted_sdu(TransmissionGrant::GrantedToOtherUser),
            main_address: TetraAddress::new(gssi, SsiType::Gssi),
            endpoint_id: 0,
            pdu_prio: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: Some(CmceChanAllocReq {
                usage: Some(6),
                timeslots,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Dl,
                carrier: None,
            }),
            tx_reporter: Some(listener_reporter.clone()),
        }),
    });

    test.run_stack(Some(64));
    let sink_msgs = test.dump_sinks();
    let all_pdus = downlink_mac_pdus(&sink_msgs);

    let requester_grant_index = all_pdus
        .iter()
        .position(|pdu| match pdu {
            DownlinkMacPdu::Resource(LogicalChannel::Stch, resource)
                if resource
                    .addr
                    .is_some_and(|addr| mac_resource_matches_addr(addr, TetraAddress::issi(requester_issi)))
                    && resource.chan_alloc_element.is_some() =>
            {
                true
            }
            _ => false,
        })
        .unwrap_or_else(|| {
            panic!(
                "expected mixed-EG requester D-TX GRANTED STCH with channel allocation; reporter={:?}; pdus={all_pdus:?}",
                requester_reporter.get_state()
            )
        });
    let listener_grant_index = all_pdus
        .iter()
        .position(|pdu| match pdu {
            DownlinkMacPdu::Resource(LogicalChannel::Stch, resource)
                if resource
                    .addr
                    .is_some_and(|addr| mac_resource_matches_addr(addr, TetraAddress::new(gssi, SsiType::Gssi)))
                    && resource.chan_alloc_element.is_some() =>
            {
                true
            }
            _ => false,
        })
        .unwrap_or_else(|| {
            panic!(
                "expected mixed-EG GSSI D-TX GRANTED/GrantedToOtherUser STCH; reporter={:?}; pdus={all_pdus:?}",
                listener_reporter.get_state()
            )
        });

    let first_busy_index = all_pdus.iter().position(|pdu| match pdu {
        DownlinkMacPdu::Resource(LogicalChannel::Stch, resource)
            if resource.addr.is_some_and(|addr| {
                matches!(addr.ssi_type, SsiType::Issi | SsiType::Ssi) && (3_500_000..3_500_000 + busy_count).contains(&addr.ssi)
            }) =>
        {
            true
        }
        _ => false,
    });
    if let Some(first_busy_index) = first_busy_index {
        assert!(
            requester_grant_index < first_busy_index && listener_grant_index < first_busy_index,
            "requester and listener floor grants must transmit before lower-value busy responses"
        );
    }
    assert!(
        requester_grant_index < listener_grant_index,
        "positive requester grant should stay ahead of the listener notification"
    );

    let DownlinkMacPdu::Resource(_, requester_grant) = &all_pdus[requester_grant_index] else {
        unreachable!("requester_grant_index was selected from Resource variants");
    };
    assert!(
        requester_grant.random_access_flag,
        "requester floor grant should repeat the preserved random-access ACK"
    );
    assert_eq!(
        requester_grant
            .chan_alloc_element
            .as_ref()
            .expect("requester grant should carry channel allocation")
            .ul_dl_assigned,
        UlDlAssignment::Both
    );

    let DownlinkMacPdu::Resource(_, listener_grant) = &all_pdus[listener_grant_index] else {
        unreachable!("listener_grant_index was selected from Resource variants");
    };
    assert!(
        !listener_grant.random_access_flag,
        "GSSI listener grant must not acknowledge one requester's random access for the whole group"
    );
    assert_eq!(
        listener_grant
            .chan_alloc_element
            .as_ref()
            .expect("listener grant should carry channel allocation")
            .ul_dl_assigned,
        UlDlAssignment::Dl
    );
    assert_eq!(requester_reporter.get_state(), TxState::Transmitted);
    assert_eq!(listener_reporter.get_state(), TxState::Transmitted);
}

#[test]
fn test_oversized_facch_stealing_falls_back_to_schf_instead_of_overflowing_stch() {
    debug::setup_logging_verbose();

    let target_issi = 30130;
    let traffic_ts = 2;
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default().add_timeslots(2)));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(group_call_open_msg(91, traffic_ts));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let d_tx_granted = DTxGranted {
        call_identifier: 1,
        transmission_grant: TransmissionGrant::Granted.into_raw() as u8,
        transmission_request_permission: false,
        encryption_control: false,
        reserved: false,
        notification_indicator: None,
        transmitting_party_type_identifier: Some(1),
        transmitting_party_address_ssi: Some(target_issi as u64),
        transmitting_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };
    let mut pdu = BitBuffer::new_autoexpand(128);
    d_tx_granted.to_bitbuf(&mut pdu).expect("serialize D-TX GRANTED");
    pdu.seek(0);

    let mut timeslots = [false; 4];
    timeslots[(traffic_ts - 1) as usize] = true;
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 8,
            pdu,
            main_address: TetraAddress::new(target_issi, SsiType::Issi),
            endpoint_id: 1,
            pdu_prio: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: Some(CmceChanAllocReq {
                usage: Some(4),
                timeslots,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
                carrier: None,
            }),
            tx_reporter: None,
        }),
    });

    test.run_stack(Some(8));
    let sink_msgs = test.dump_sinks();
    let resources = mac_resources_for_addr(&sink_msgs, TetraAddress::issi(target_issi));

    assert!(
        resources
            .iter()
            .any(|(logical_channel, _)| *logical_channel == LogicalChannel::SchF),
        "oversized FACCH/STCH message should fall back to SCH/F where the fragger owns capacity"
    );
    assert!(
        resources
            .iter()
            .all(|(logical_channel, _)| *logical_channel != LogicalChannel::Stch),
        "oversized FACCH/STCH message must not be written into a 124-bit STCH block"
    );
}

#[test]
fn test_call_control_open_suspends_group_energy_saving_until_close_plus_t210() {
    debug::setup_logging_verbose();

    let start = TdmaTime::default();
    let gssi = 91;
    let first_issi = 1001;
    let second_issi = 1002;
    let unrelated_issi = 1003;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        for issi in [first_issi, second_issi, unrelated_issi] {
            state.subscribers.register(issi);
            state.energy_saving.insert(issi, eg_assignment(start));
        }
        state.subscribers.affiliate(first_issi, gssi);
        state.subscribers.affiliate(second_issi, gssi);
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![]);

    test.submit_message(group_call_open_msg(gssi, 2));
    test.run_stack(Some(1));

    {
        let state = test.config.state_read();
        for issi in [first_issi, second_issi] {
            let assignment = state.energy_saving.get(&issi).expect("affiliated EG MS should remain tracked");
            assert_eq!(assignment.suspension_count, 1);
            assert!(
                assignment.listens_at(start.add_timeslots(4)),
                "assigned-channel group call must suspend the EG sleep cycle"
            );
        }
        assert_eq!(
            state
                .energy_saving
                .get(&unrelated_issi)
                .expect("unrelated EG MS should remain tracked")
                .suspension_count,
            0
        );
    }

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Close(Direction::Both, 2)),
    });
    test.run_stack(Some(1));

    let state = test.config.state_read();
    for issi in [first_issi, second_issi] {
        let assignment = state.energy_saving.get(&issi).expect("affiliated EG MS should remain tracked");
        assert_eq!(assignment.suspension_count, 0);
        assert!(
            assignment.awake_until.is_some(),
            "EG resume after assigned-channel close should keep T.210 awake window"
        );
    }
}

#[test]
fn test_late_group_eg_activation_joins_active_assigned_channel_suspension() {
    debug::setup_logging_verbose();

    let start = TdmaTime::default();
    let gssi = 91;
    let initial_issi = 1001;
    let late_issi = 1002;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        state.subscribers.register(initial_issi);
        state.subscribers.affiliate(initial_issi, gssi);
        state.energy_saving.insert(initial_issi, eg_assignment(start));
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![]);

    test.submit_message(group_call_open_msg(gssi, 2));
    test.run_stack(Some(1));

    {
        let mut state = test.config.state_write();
        state.subscribers.register(late_issi);
        state.subscribers.affiliate(late_issi, gssi);
    }
    submit_tlmc_configure_req(
        &mut test,
        TlmcConfigureReq {
            energy_economy_issi: Some(late_issi),
            energy_economy_group: Some(7),
            energy_economy_startpoint: Some(TlmcEnergyEconomyStartpoint { frame: 3, multiframe: 1 }),
            ..tlmc_configure_req()
        },
    );
    test.run_stack(Some(1));

    {
        let state = test.config.state_read();
        let assignment = state
            .energy_saving
            .get(&late_issi)
            .expect("late affiliated EG MS should be tracked");
        assert_eq!(
            assignment.suspension_count, 1,
            "late EG activation in an active GSSI call must inherit assigned-channel suspension"
        );
        assert!(
            assignment.listens_at(start.add_timeslots(4)),
            "late EG member must stay awake while the assigned group channel is active"
        );
    }

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Close(Direction::Both, 2)),
    });
    test.run_stack(Some(1));

    let state = test.config.state_read();
    let assignment = state
        .energy_saving
        .get(&late_issi)
        .expect("late affiliated EG MS should remain tracked after group close");
    assert_eq!(assignment.suspension_count, 0);
    assert!(
        assignment.awake_until.is_some(),
        "late EG resume after assigned-channel close should keep T.210 awake window"
    );
}

#[test]
fn test_large_eg7_group_call_open_suspends_all_members_once_and_resumes_after_close() {
    debug::setup_logging_verbose();

    let start = TdmaTime::default();
    let gssi = 226_333;
    let first_issi = 300_000;
    let member_count = 4096;
    let unrelated_issi = 399_999;
    let ts = 2;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        for offset in 0..member_count {
            let issi = first_issi + offset;
            state.subscribers.register(issi);
            assert!(state.subscribers.affiliate(issi, gssi));
            state.energy_saving.insert(issi, eg_assignment(start));
        }
        state.subscribers.register(unrelated_issi);
        state.energy_saving.insert(unrelated_issi, eg_assignment(start));
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![]);

    test.submit_message(group_call_open_msg(gssi, ts));
    test.run_stack(Some(1));

    {
        let state = test.config.state_read();
        assert_eq!(state.subscribers.group_members(gssi).len(), member_count as usize);
        for offset in 0..member_count {
            let issi = first_issi + offset;
            let assignment = state
                .energy_saving
                .get(&issi)
                .expect("large-group EG7 member should remain tracked");
            assert_eq!(
                assignment.suspension_count, 1,
                "EN 300 392-2 clause 23.7.6: assigned-channel GSSI call should suspend each affiliated EG member exactly once"
            );
            assert!(
                assignment.listens_at(start.add_timeslots(4)),
                "assigned-channel suspension should keep every EG7 group member awake while the call is active"
            );
        }
        assert_eq!(
            state
                .energy_saving
                .get(&unrelated_issi)
                .expect("unrelated EG7 member should remain tracked")
                .suspension_count,
            0,
            "unrelated EG7 ISSI must not be suspended by another GSSI's group call"
        );
    }

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Close(Direction::Both, ts)),
    });
    test.run_stack(Some(1));

    let state = test.config.state_read();
    for offset in 0..member_count {
        let issi = first_issi + offset;
        let assignment = state
            .energy_saving
            .get(&issi)
            .expect("large-group EG7 member should remain tracked after close");
        assert_eq!(assignment.suspension_count, 0);
        assert!(
            assignment.awake_until.is_some(),
            "EG7 resume after assigned-channel group close should keep the T.210 awake guard"
        );
    }
    assert_eq!(
        state
            .energy_saving
            .get(&unrelated_issi)
            .expect("unrelated EG7 member should remain tracked after close")
            .suspension_count,
        0
    );
}

#[test]
fn test_group_secondary_speaker_does_not_double_suspend_energy_saving() {
    debug::setup_logging_verbose();

    let start = TdmaTime::default();
    let gssi = 92;
    let first_issi = 1101;
    let second_issi = 1102;
    let unrelated_issi = 1103;
    let ts = 2;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        for issi in [first_issi, second_issi, unrelated_issi] {
            state.subscribers.register(issi);
            state.energy_saving.insert(issi, eg_assignment(start));
        }
        state.subscribers.affiliate(first_issi, gssi);
        state.subscribers.affiliate(second_issi, gssi);
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![]);

    test.submit_message(group_call_open_msg_with_secondary_speaker(gssi, first_issi, ts));
    test.run_stack(Some(1));

    {
        let state = test.config.state_read();
        for issi in [first_issi, second_issi] {
            let assignment = state.energy_saving.get(&issi).expect("affiliated EG MS should remain tracked");
            assert_eq!(
                assignment.suspension_count, 1,
                "EN 300 392-2 clause 23.7.6: a group speaker ISSI already covered by the GSSI assigned channel must not be double-counted as a private/P2P participant"
            );
        }
        assert_eq!(
            state
                .energy_saving
                .get(&unrelated_issi)
                .expect("unrelated EG MS should remain tracked")
                .suspension_count,
            0
        );
    }

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Close(Direction::Both, ts)),
    });
    test.run_stack(Some(1));

    let state = test.config.state_read();
    for issi in [first_issi, second_issi] {
        assert_eq!(
            state
                .energy_saving
                .get(&issi)
                .expect("affiliated EG MS should remain tracked")
                .suspension_count,
            0
        );
    }
}

#[test]
fn test_tlmc_energy_economy_reconfiguration_preserves_assigned_channel_suspension() {
    debug::setup_logging_verbose();

    let start = TdmaTime::default();
    let caller_issi = 1301;
    let called_issi = 1302;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        state.energy_saving.insert(caller_issi, eg_assignment(start));
        state.energy_saving.insert(called_issi, eg_assignment(start));
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![]);

    test.submit_message(private_call_open_msg(caller_issi, called_issi, 2));
    test.run_stack(Some(1));
    assert_eq!(
        test.config
            .state_read()
            .energy_saving
            .get(&caller_issi)
            .expect("caller EG assignment should remain tracked")
            .suspension_count,
        1
    );

    // EN 300 392-2 clause 23.7.6 allows the BS to change EG parameters, but
    // an MS may sleep only on its common control channel. A live assigned
    // private-call channel keeps the sleep cycle suspended across TLMC
    // reconfiguration until the channel is released.
    submit_tlmc_configure_req(
        &mut test,
        TlmcConfigureReq {
            energy_economy_issi: Some(caller_issi),
            energy_economy_group: Some(7),
            energy_economy_startpoint: Some(TlmcEnergyEconomyStartpoint { frame: 1, multiframe: 1 }),
            ..tlmc_configure_req()
        },
    );
    test.run_stack(Some(1));
    {
        let state = test.config.state_read();
        let assignment = state
            .energy_saving
            .get(&caller_issi)
            .expect("reconfigured caller EG assignment should remain tracked");
        assert_eq!(assignment.mode, 7);
        assert_eq!(assignment.suspension_count, 1);
        assert!(
            assignment.listens_at(start.add_timeslots(4)),
            "assigned-channel suspension must keep the reconfigured EG MS awake on a sleeping frame"
        );
    }

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Close(Direction::Both, 2)),
    });
    test.run_stack(Some(1));

    let state = test.config.state_read();
    let assignment = state
        .energy_saving
        .get(&caller_issi)
        .expect("caller EG assignment should remain after close");
    assert_eq!(assignment.suspension_count, 0);
    assert!(
        assignment.awake_until.is_some(),
        "EG resume after reconfigured assigned-channel close should keep T.210 awake window"
    );
}

#[test]
fn test_tlmc_energy_economy_activation_during_private_call_starts_suspended() {
    debug::setup_logging_verbose();

    let start = TdmaTime::default();
    let caller_issi = 1303;
    let called_issi = 1304;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    test.populate_entities(vec![TetraEntity::Umac], vec![]);

    test.submit_message(private_call_open_msg(caller_issi, called_issi, 2));
    test.run_stack(Some(1));
    assert!(
        !test.config.state_read().energy_saving.contains_key(&caller_issi),
        "test setup starts without a pre-existing EG assignment"
    );

    // EN 300 392-2 clause 23.7.6 suspends energy economy while the MS is
    // active on an assigned channel. If TLMC activates EG after the private
    // call channel is already open, the new assignment must inherit that
    // active suspension instead of letting the MS sleep mid-call.
    submit_tlmc_configure_req(
        &mut test,
        TlmcConfigureReq {
            energy_economy_issi: Some(caller_issi),
            energy_economy_group: Some(7),
            energy_economy_startpoint: Some(TlmcEnergyEconomyStartpoint { frame: 1, multiframe: 1 }),
            ..tlmc_configure_req()
        },
    );
    test.run_stack(Some(1));
    {
        let state = test.config.state_read();
        let assignment = state
            .energy_saving
            .get(&caller_issi)
            .expect("TLMC should activate caller EG assignment");
        assert_eq!(assignment.mode, 7);
        assert_eq!(assignment.suspension_count, 1);
        assert!(
            assignment.listens_at(start.add_timeslots(4)),
            "active private-call assigned channel must keep a newly activated EG MS awake on a sleeping frame"
        );
    }

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Close(Direction::Both, 2)),
    });
    test.run_stack(Some(1));

    let state = test.config.state_read();
    let assignment = state
        .energy_saving
        .get(&caller_issi)
        .expect("caller EG assignment should remain after close");
    assert_eq!(assignment.suspension_count, 0);
    assert!(
        assignment.awake_until.is_some(),
        "EG resume after assigned-channel close should keep T.210 awake window"
    );
}

#[test]
fn test_call_control_open_suspends_all_ones_broadcast_energy_saving_until_close_plus_t210() {
    debug::setup_logging_verbose();

    let start = TdmaTime::default();
    let first_issi = 1001;
    let second_issi = 1002;
    let unregistered_issi = 1003;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        for issi in [first_issi, second_issi] {
            state.subscribers.register(issi);
            state.energy_saving.insert(issi, eg_assignment(start));
        }
        state.energy_saving.insert(unregistered_issi, eg_assignment(start));
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![]);

    test.submit_message(group_call_open_msg(0xFF_FFFF, 2));
    test.run_stack(Some(1));

    {
        let state = test.config.state_read();
        for issi in [first_issi, second_issi] {
            let assignment = state.energy_saving.get(&issi).expect("registered EG MS should remain tracked");
            assert_eq!(assignment.suspension_count, 1);
            assert!(
                assignment.listens_at(start.add_timeslots(4)),
                "all-ones assigned-channel call must suspend every registered EG MS"
            );
        }
        assert_eq!(
            state
                .energy_saving
                .get(&unregistered_issi)
                .expect("unregistered EG assignment should remain tracked")
                .suspension_count,
            0
        );
    }

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Close(Direction::Both, 2)),
    });
    test.run_stack(Some(1));

    let state = test.config.state_read();
    for issi in [first_issi, second_issi] {
        let assignment = state.energy_saving.get(&issi).expect("registered EG MS should remain tracked");
        assert_eq!(assignment.suspension_count, 0);
        assert!(
            assignment.awake_until.is_some(),
            "EG resume after all-ones assigned-channel close should keep T.210 awake window"
        );
    }
}

#[test]
fn test_call_control_open_suspends_multiple_active_issi_addresses_until_close_plus_t210() {
    debug::setup_logging_verbose();

    let start = TdmaTime::default();
    let first_issi = 1001;
    let second_issi = 1002;
    let unrelated_issi = 1003;
    let ts = 2;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        for issi in [first_issi, second_issi, unrelated_issi] {
            state.subscribers.register(issi);
            state.energy_saving.insert(issi, eg_assignment(start));
        }
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![]);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Open(Circuit {
            direction: Direction::Both,
            ts,
            peer_ts: None,
            usage: 4,
            circuit_mode: CircuitModeType::TchS,
            speech_service: Some(0),
            etee_encrypted: false,
            dl_media_source: CircuitDlMediaSource::LocalLoopback,
            active_addr: Some(TetraAddress::new(first_issi, SsiType::Issi)),
            active_secondary_addrs: vec![TetraAddress::new(second_issi, SsiType::Issi)],
        })),
    });
    test.run_stack(Some(1));

    {
        let state = test.config.state_read();
        for issi in [first_issi, second_issi] {
            let assignment = state.energy_saving.get(&issi).expect("EG MS should remain tracked");
            assert_eq!(assignment.suspension_count, 1);
            assert!(
                assignment.listens_at(start.add_timeslots(4)),
                "assigned-channel private call must suspend every active ISSI sleep cycle"
            );
        }
        assert_eq!(
            state
                .energy_saving
                .get(&unrelated_issi)
                .expect("unrelated EG MS should remain tracked")
                .suspension_count,
            0
        );
    }

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Close(Direction::Both, ts)),
    });
    test.run_stack(Some(1));

    let state = test.config.state_read();
    for issi in [first_issi, second_issi] {
        let assignment = state.energy_saving.get(&issi).expect("EG MS should remain tracked");
        assert_eq!(assignment.suspension_count, 0);
        assert!(
            assignment.awake_until.is_some(),
            "closing an assigned-channel private call should start the T.210 awake guard"
        );
    }
}

#[test]
fn test_group_energy_saving_suspension_resumes_original_members_after_deaffiliate() {
    debug::setup_logging_verbose();

    let start = TdmaTime::default();
    let gssi = 92;
    let first_issi = 1101;
    let second_issi = 1102;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        for issi in [first_issi, second_issi] {
            state.subscribers.register(issi);
            state.subscribers.affiliate(issi, gssi);
            state.energy_saving.insert(issi, eg_assignment(start));
        }
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![]);

    test.submit_message(group_call_open_msg(gssi, 2));
    test.run_stack(Some(1));
    {
        let mut state = test.config.state_write();
        state.subscribers.deaffiliate(first_issi, gssi);
    }

    // EN 300 392-2 clause 23.7.6 suspends the MAC sleep cycle while the MS is
    // active in the assigned-channel call. Closing must resume the exact ISSIs
    // suspended at open time, even if group affiliation changed meanwhile.
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Close(Direction::Both, 2)),
    });
    test.run_stack(Some(1));

    let state = test.config.state_read();
    assert!(state.subscribers.group_members(gssi).iter().all(|issi| *issi != first_issi));
    for issi in [first_issi, second_issi] {
        let assignment = state.energy_saving.get(&issi).expect("EG MS should remain tracked");
        assert_eq!(assignment.suspension_count, 0);
        assert!(assignment.awake_until.is_some(), "resume should start the T.210 awake guard");
    }
}

#[test]
fn test_group_energy_saving_suspension_survives_partial_circuit_close() {
    debug::setup_logging_verbose();

    let start = TdmaTime::default();
    let gssi = 93;
    let issi = 1201;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        state.subscribers.register(issi);
        state.subscribers.affiliate(issi, gssi);
        state.energy_saving.insert(issi, eg_assignment(start));
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![]);

    test.submit_message(group_call_open_msg(gssi, 2));
    test.run_stack(Some(1));

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Close(Direction::Dl, 2)),
    });
    test.run_stack(Some(1));
    assert_eq!(
        test.config
            .state_read()
            .energy_saving
            .get(&issi)
            .expect("EG MS should remain tracked")
            .suspension_count,
        1,
        "UL side is still active, so assigned-channel sleep suspension must remain"
    );

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Close(Direction::Ul, 2)),
    });
    test.run_stack(Some(1));

    let state = test.config.state_read();
    let assignment = state.energy_saving.get(&issi).expect("EG MS should remain tracked");
    assert_eq!(assignment.suspension_count, 0);
    assert!(assignment.awake_until.is_some(), "final close should start the T.210 awake guard");
}

#[test]
fn test_group_energy_saving_partial_replacement_keeps_old_owner_suspended() {
    debug::setup_logging_verbose();

    let start = TdmaTime::default();
    let old_gssi = 94;
    let new_gssi = 95;
    let old_issi = 1301;
    let new_issi = 1302;
    let mut test = ComponentTest::new(StackMode::Bs, Some(start));
    {
        let mut state = test.config.state_write();
        state.subscribers.register(old_issi);
        state.subscribers.affiliate(old_issi, old_gssi);
        state.energy_saving.insert(old_issi, eg_assignment(start));
        state.subscribers.register(new_issi);
        state.subscribers.affiliate(new_issi, new_gssi);
        state.energy_saving.insert(new_issi, eg_assignment(start));
    }
    test.populate_entities(vec![TetraEntity::Umac], vec![]);

    test.submit_message(group_call_open_msg(old_gssi, 2));
    test.run_stack(Some(1));

    test.submit_message(group_call_open_msg_for_direction(new_gssi, 2, Direction::Dl));
    test.run_stack(Some(1));

    {
        let state = test.config.state_read();
        assert_eq!(
            state
                .energy_saving
                .get(&old_issi)
                .expect("old EG MS should remain tracked")
                .suspension_count,
            1,
            "old group still owns the UL circuit, so EG sleep must remain suspended"
        );
        assert_eq!(
            state
                .energy_saving
                .get(&new_issi)
                .expect("new EG MS should remain tracked")
                .suspension_count,
            1,
            "new group owns the replacement DL circuit and must also be suspended"
        );
    }

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Close(Direction::Ul, 2)),
    });
    test.run_stack(Some(1));

    {
        let state = test.config.state_read();
        assert_eq!(
            state
                .energy_saving
                .get(&old_issi)
                .expect("old EG MS should remain tracked")
                .suspension_count,
            0
        );
        assert_eq!(
            state
                .energy_saving
                .get(&new_issi)
                .expect("new EG MS should remain tracked")
                .suspension_count,
            1
        );
    }

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Close(Direction::Dl, 2)),
    });
    test.run_stack(Some(1));

    let state = test.config.state_read();
    assert_eq!(
        state
            .energy_saving
            .get(&new_issi)
            .expect("new EG MS should remain tracked")
            .suspension_count,
        0
    );
}
