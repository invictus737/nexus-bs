mod common;

use common::ComponentTest;
use tetra_config::bluestation::StackMode;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, BurstType, PhyBlockNum, PhyBlockType, PhysicalChannel, Sap, TdmaTime, TrainingSequence, debug};
use tetra_entities::lmac::components::{errorcontrol, scrambler};
use tetra_saps::tmv::{TmvUnitdataReq, TmvUnitdataReqSlot, enums::logical_chans::LogicalChannel};
use tetra_saps::tp::TpUnitdataInd;
use tetra_saps::{SapMsg, SapMsgInner};

fn test_scrambling_code() -> u32 {
    scrambler::tetra_scramb_get_init(204, 1337, 1)
}

fn acelp_test_bits() -> Vec<u8> {
    (0..274).map(|idx| ((idx * 7 + 3) % 2) as u8).collect()
}

fn build_downlink_traffic_req_for_ul_ts(logical_channel: LogicalChannel, type1_bits: usize, ul_ts: u8) -> SapMsg {
    SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Lmac,
        msg: SapMsgInner::TmvUnitdataReq(TmvUnitdataReqSlot {
            ts: TdmaTime {
                h: 0,
                m: 1,
                f: 1,
                t: ul_ts,
            },
            ul_phy_chan: PhysicalChannel::Tp,
            blk1: Some(TmvUnitdataReq {
                mac_block: BitBuffer::new(type1_bits),
                logical_channel,
                scrambling_code: test_scrambling_code(),
            }),
            blk2: None,
            bbk: Some(TmvUnitdataReq {
                mac_block: BitBuffer::new(14),
                logical_channel: LogicalChannel::Aach,
                scrambling_code: test_scrambling_code(),
            }),
        }),
    }
}

fn build_downlink_traffic_req(logical_channel: LogicalChannel, type1_bits: usize) -> SapMsg {
    build_downlink_traffic_req_for_ul_ts(logical_channel, type1_bits, TdmaTime::default().t)
}

fn encoded_tch_s(codec_bits: &[u8], blk_num: u8) -> BitBuffer {
    let type1 = BitBuffer::from_bitarr(&codec_bits);
    errorcontrol::encode_tp(
        TmvUnitdataReq {
            mac_block: type1,
            logical_channel: LogicalChannel::TchS,
            scrambling_code: test_scrambling_code(),
        },
        blk_num,
    )
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

fn build_corrupt_uplink_tch_s_ind() -> SapMsg {
    let codec_bits = acelp_test_bits();
    let encoded = encoded_tch_s(&codec_bits, 1);
    let mut encoded_bits = vec![0u8; encoded.get_len()];
    let mut encoded_for_read = encoded;
    encoded_for_read.to_bitarr(&mut encoded_bits);

    let mut corrupt_block = None;
    for step in [1usize, 2, 3, 5, 7, 11] {
        let mut corrupted = encoded_bits.clone();
        for bit in (0..corrupted.len()).step_by(step) {
            corrupted[bit] ^= 1;
        }

        let candidate = BitBuffer::from_bitarr(&corrupted);
        let (_, crc_ok) = errorcontrol::decode_tp(LogicalChannel::TchS, candidate.clone(), test_scrambling_code());
        if !crc_ok {
            corrupt_block = Some(candidate);
            break;
        }
    }

    build_uplink_tch_s_ind(
        TrainingSequence::NormalTrainSeq1,
        PhyBlockNum::Both,
        corrupt_block.expect("test setup should create a TCH/S frame with bad speech CRC"),
    )
}

fn mark_all_ul_timeslots_as_traffic(test: &mut ComponentTest) {
    for ul_ts in 1..=4 {
        test.submit_message(build_downlink_traffic_req_for_ul_ts(LogicalChannel::TchS, 274, ul_ts));
        test.deliver_all_messages();
        let _ = test.dump_sinks();
    }
}

#[test]
fn bs_lmac_drops_unsupported_circuit_mode_data_tch_without_phy_output() {
    debug::setup_logging_verbose();

    for (logical_channel, type1_bits) in [
        // EN 300 392-2 clauses 8.3.1.3.2 to 8.3.1.3.4 define TCH/4.8,
        // TCH/2.4 and TCH/7.2 as circuit-mode data channels with distinct
        // type-1 sizes and coding from TCH/S speech. Until those encoders are
        // implemented, the lower MAC must fail closed and emit no PHY burst.
        (LogicalChannel::Tch24, 144),
        (LogicalChannel::Tch48, 288),
        (LogicalChannel::Tch72, 432),
    ] {
        let mut test = ComponentTest::new(StackMode::Bs, None);
        test.populate_entities(vec![TetraEntity::Lmac], vec![TetraEntity::Phy]);

        test.submit_message(build_downlink_traffic_req(logical_channel, type1_bits));
        test.deliver_all_messages();

        assert!(
            test.dump_sinks().is_empty(),
            "LMAC-BS must not guess circuit-mode data TCH coding for {logical_channel:?}"
        );
    }
}

#[test]
fn bs_lmac_forwards_valid_fullslot_tch_s_to_umac() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 3 }));
    test.populate_entities(vec![TetraEntity::Lmac], vec![TetraEntity::Umac]);

    mark_all_ul_timeslots_as_traffic(&mut test);

    let codec_bits = acelp_test_bits();
    let encoded = encoded_tch_s(&codec_bits, 1);
    let (decoded, crc_ok) = errorcontrol::decode_tp(LogicalChannel::TchS, encoded.clone(), test_scrambling_code());
    assert!(crc_ok, "test setup should create a valid full-slot TCH/S frame");
    let decoded = decoded.expect("valid TCH/S should decode");
    assert_eq!(decoded.to_bitstr(), BitBuffer::from_bitarr(&codec_bits).to_bitstr());

    test.submit_message(build_uplink_tch_s_ind(
        TrainingSequence::NormalTrainSeq1,
        PhyBlockNum::Both,
        encoded,
    ));
    test.deliver_all_messages();

    let sinks = test.dump_sinks();
    let traffic: Vec<_> = sinks
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmdCircuitDataInd(ind) => Some(ind),
            _ => None,
        })
        .collect();
    assert_eq!(traffic.len(), 1, "valid full-slot TCH/S should be forwarded once");
    assert_eq!(traffic[0].ts, TdmaTime::default().add_timeslots(-2).t);
    assert_eq!(traffic[0].data, codec_bits);
}

#[test]
fn bs_lmac_drops_normal_seq2_block2_tch_s_without_forwarding_clean_speech() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 3 }));
    test.populate_entities(vec![TetraEntity::Lmac], vec![TetraEntity::Umac]);

    mark_all_ul_timeslots_as_traffic(&mut test);

    let codec_bits = acelp_test_bits();
    test.submit_message(build_uplink_tch_s_ind(
        TrainingSequence::NormalTrainSeq2,
        PhyBlockNum::Block2,
        encoded_tch_s(&codec_bits, 2),
    ));
    test.deliver_all_messages();

    assert!(
        test.dump_sinks()
            .iter()
            .all(|msg| !matches!(msg.msg, SapMsgInner::TmdCircuitDataInd(_))),
        "EN 300 392-2 clauses 23.8.3 and 23.8.3.2 require bad/partial speech frame state to be preserved; this SAP cannot carry BFI/half-slot condition, so LMAC-BS must not forward Block2-only TCH/S as clean speech"
    );
}

#[test]
fn bs_lmac_drops_bad_crc_tch_s_instead_of_forwarding_static_speech() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 3 }));
    test.populate_entities(vec![TetraEntity::Lmac], vec![TetraEntity::Umac]);

    mark_all_ul_timeslots_as_traffic(&mut test);

    test.submit_message(build_corrupt_uplink_tch_s_ind());
    test.deliver_all_messages();

    assert!(
        test.dump_sinks()
            .iter()
            .all(|msg| !matches!(msg.msg, SapMsgInner::TmdCircuitDataInd(_))),
        "EN 300 392-2 clauses 23.8.3 and 23.8.3.2 permit undecodable TCH delivery only with a bad half-slot condition; this SAP cannot carry that condition, so LMAC-BS must not forward it as speech"
    );
}
