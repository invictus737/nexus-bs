mod common;

use tetra_config::bluestation::{EnergySavingAssignment, SharedConfig, StackMode};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Direction, Layer2Service, PhyBlockNum, Sap, SsiType, TdmaTime, TetraAddress, TxReporter, TxState, debug};
use tetra_entities::umac::umac_bs::UmacBs;
use tetra_pdus::cmce::enums::transmission_grant::TransmissionGrant;
use tetra_pdus::cmce::pdus::d_tx_granted::DTxGranted;
use tetra_pdus::umac::enums::basic_slotgrant_cap_alloc::BasicSlotgrantCapAlloc;
use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
use tetra_pdus::umac::pdus::mac_access::MacAccess;
use tetra_pdus::umac::pdus::mac_end_dl::MacEndDl;
use tetra_pdus::umac::pdus::mac_frag_dl::MacFragDl;
use tetra_pdus::umac::pdus::mac_resource::MacResource;
use tetra_pdus::umac::pdus::mac_u_blck::MacUBlck;
use tetra_pdus::umac::pdus::mac_u_signal::MacUSignal;
use tetra_saps::control::call_control::{CallControl, Circuit, CircuitDlMediaSource};
use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
use tetra_saps::lcmc::enums::alloc_type::ChanAllocType;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::lmm::LmmMleUnitdataReq;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};
use tetra_saps::tlmc::{TlmcConfigureReq, TlmcEnergyEconomyStartpoint};
use tetra_saps::tma::{TmaCancelReq, TmaReport, TmaUnitdataReq};
use tetra_saps::tmv::{TmvUnitdataInd, TmvUnitdataReq, enums::logical_chans::LogicalChannel};

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

fn tma_unitdata_ind_addresses(msgs: &[SapMsg]) -> Vec<TetraAddress> {
    msgs.iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmaUnitdataInd(prim) => Some(prim.main_address),
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
fn test_stch_mac_u_signal_uses_current_ul_speaker_from_private_open_circuit() {
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
    assert_eq!(
        addresses,
        vec![TetraAddress::issi(caller_issi)],
        "EN 300 392-2 clauses 21.4.5 and 14.5.1.2.1 require STCH U-plane signalling to inherit the current private-call speaker, not ISSI 0"
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
    assert_eq!(
        addresses,
        vec![TetraAddress::issi(caller_issi)],
        "EN 300 392-2 clauses 14.5.1.2.1 and 21.4.5: STCH private-call signalling has no SSI field, so UMAC must not let a non-participant FloorGranted rewrite the inferred speaker"
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
    test.run_stack(Some(8));
    let sink_msgs = test.dump_sinks();
    let mac_resource =
        first_mac_resource_for_addr(&sink_msgs, TetraAddress::issi(target_issi)).expect("expected MAC-RESOURCE addressed to target ISSI");

    assert!(
        !mac_resource.random_access_flag,
        "EN 300 392-2 clause 21.4.3.1 random_access_flag must only acknowledge actual random access"
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
        .find(|(logical_channel, resource)| *logical_channel == LogicalChannel::Stch && resource.chan_alloc_element.is_none())
        .expect("expected first ACK-only STCH for the requester");
    assert!(
        !ack_only.1.random_access_flag,
        "ACK-only STCH must not consume the random-access ACK preserved from hangtime"
    );

    let grant = resources
        .iter()
        .find(|(logical_channel, resource)| {
            *logical_channel == LogicalChannel::Stch && resource.chan_alloc_element.is_some()
        })
        .unwrap_or_else(|| panic!(
            "expected D-TX GRANTED STCH with channel allocation; ack_reporter={:?}; grant_reporter={:?}; resources={resources:?}; all_pdus={all_pdus:?}",
            ack_reporter.get_state(),
            grant_reporter.get_state()
        ));
    assert!(
        grant.1.random_access_flag,
        "EN 300 392-2 clauses 21.4.3.1, 14.5.1.2.1 b) and 23.5.2.2.1: the private-call floor grant STCH should carry the random-access ACK that lets the requesting MS continue onto the assigned channel"
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
