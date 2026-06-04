mod common;

use common::ComponentTest;
use tetra_config::bluestation::StackMode;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, BurstType, PhyBlockNum, PhyBlockType, PhysicalChannel, Sap, TdmaTime, TrainingSequence, debug};
use tetra_entities::lmac::components::errorcontrol;
use tetra_saps::tmv::{TmvUnitdataReq, TmvUnitdataReqSlot, enums::logical_chans::LogicalChannel};
use tetra_saps::tp::TpUnitdataInd;
use tetra_saps::{SapMsg, SapMsgInner};

const TEST_SCRAMBLING_CODE: u32 = 864282631;

fn build_downlink_traffic_req(logical_channel: LogicalChannel, type1_bits: usize) -> SapMsg {
    SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Lmac,
        msg: SapMsgInner::TmvUnitdataReq(TmvUnitdataReqSlot {
            ts: TdmaTime::default(),
            ul_phy_chan: PhysicalChannel::Tp,
            blk1: Some(TmvUnitdataReq {
                mac_block: BitBuffer::new(type1_bits),
                logical_channel,
                scrambling_code: TEST_SCRAMBLING_CODE,
            }),
            blk2: None,
            bbk: Some(TmvUnitdataReq {
                mac_block: BitBuffer::new(14),
                logical_channel: LogicalChannel::Aach,
                scrambling_code: TEST_SCRAMBLING_CODE,
            }),
        }),
    }
}

fn build_corrupt_uplink_tch_s_ind() -> SapMsg {
    let codec_bits: Vec<u8> = (0..274).map(|idx| (idx % 2) as u8).collect();
    let type1 = BitBuffer::from_bitarr(&codec_bits);
    let encoded = errorcontrol::encode_tp(
        TmvUnitdataReq {
            mac_block: type1,
            logical_channel: LogicalChannel::TchS,
            scrambling_code: TEST_SCRAMBLING_CODE,
        },
        1,
    );

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
        let (_, crc_ok) = errorcontrol::decode_tp(LogicalChannel::TchS, candidate.clone(), TEST_SCRAMBLING_CODE);
        if !crc_ok {
            corrupt_block = Some(candidate);
            break;
        }
    }

    SapMsg {
        sap: Sap::TpSap,
        src: TetraEntity::Phy,
        dest: TetraEntity::Lmac,
        msg: SapMsgInner::TpUnitdataInd(TpUnitdataInd {
            train_type: TrainingSequence::NormalTrainSeq1,
            burst_type: BurstType::NUB,
            block_type: PhyBlockType::NUB,
            block_num: PhyBlockNum::Both,
            block: corrupt_block.expect("test setup should create a TCH/S frame with bad speech CRC"),
            rssi_dbfs: f32::NAN,
        }),
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
fn bs_lmac_drops_bad_crc_tch_s_instead_of_forwarding_static_speech() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 4 }));
    test.populate_entities(vec![TetraEntity::Lmac], vec![TetraEntity::Umac]);

    test.submit_message(build_downlink_traffic_req(LogicalChannel::TchS, 274));
    test.deliver_all_messages();
    let _ = test.dump_sinks();

    test.submit_message(build_corrupt_uplink_tch_s_ind());
    test.deliver_all_messages();

    assert!(
        test.dump_sinks()
            .iter()
            .all(|msg| !matches!(msg.msg, SapMsgInner::TmdCircuitDataInd(_))),
        "EN 300 392-2 clauses 23.8.3 and 23.8.3.2 permit undecodable TCH delivery only with a bad half-slot condition; this SAP cannot carry that condition, so LMAC-BS must not forward it as speech"
    );
}
