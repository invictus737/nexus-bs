// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

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

fn sch_hu_type1_block() -> BitBuffer {
    // 92 deterministic bits. The exact MAC payload is not important here:
    // this test is scoped to lower-MAC SCH/HU channel coding.
    BitBuffer::from_bitstr("10110011100011110000111100001111000011110000111100001111000011110000111100001111000011110000")
}

fn build_sch_hu_req(mac_block: BitBuffer) -> SapMsg {
    SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Lmac,
        msg: SapMsgInner::TmvUnitdataReq(TmvUnitdataReqSlot {
            ts: TdmaTime::default(),
            ul_phy_chan: PhysicalChannel::Cp,
            blk1: Some(TmvUnitdataReq {
                mac_block,
                logical_channel: LogicalChannel::SchHu,
                scrambling_code: TEST_SCRAMBLING_CODE,
            }),
            blk2: None,
            bbk: None,
        }),
    }
}

fn extract_tp_unitdata_req(msgs: &[SapMsg]) -> &tetra_saps::tp::TpUnitdataReqSlot {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::TpUnitdataReq(prim) => Some(prim),
            _ => None,
        })
        .expect("expected TP-UNITDATA request toward PHY")
}

#[test]
fn ms_lmac_encodes_sch_hu_tmv_request_as_cub_for_phy() {
    debug::setup_logging_verbose();
    let original = sch_hu_type1_block();

    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Lmac], vec![TetraEntity::Phy]);

    test.submit_message(build_sch_hu_req(BitBuffer::from_bitbuffer(&original)));
    test.deliver_all_messages();
    let sink_msgs = test.dump_sinks();

    let prim = extract_tp_unitdata_req(&sink_msgs);

    // EN 300 392-2 clauses 8.3.1.4.3, 9.4.4.3.3, 9.5.3 and 23.5.2.4:
    // a SCH/HU MAC block is a 92-bit uplink type-1 control block encoded
    // to 168 type-5 bits, then mapped as a control uplink burst with the
    // extended training sequence. This validates only the LMAC-MS boundary;
    // actual RF transmit slotting remains PHY work.
    assert_eq!(prim.burst_type, BurstType::CUB);
    assert_eq!(prim.train_type, TrainingSequence::ExtendedTrainSeq);
    assert!(prim.bbk.is_none());
    assert!(prim.blk2.is_none());

    let encoded = prim.blk1.as_ref().expect("CUB needs encoded SCH/HU block");
    assert_eq!(encoded.get_len(), 168);

    let (decoded, crc_pass) = errorcontrol::decode_cp(
        LogicalChannel::SchHu,
        TpUnitdataInd {
            train_type: TrainingSequence::ExtendedTrainSeq,
            burst_type: BurstType::CUB,
            block_type: PhyBlockType::NUB,
            block_num: PhyBlockNum::Block1,
            block: BitBuffer::from_bitbuffer(encoded),
            rssi_dbfs: 0.0,
        },
        Some(TEST_SCRAMBLING_CODE),
    );
    assert!(crc_pass);
    assert_eq!(decoded.expect("decoded SCH/HU block").to_bitstr(), original.to_bitstr());
}

#[test]
fn ms_lmac_rejects_non_sch_hu_uplink_tmv_request() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Ms, None);
    test.populate_entities(vec![TetraEntity::Lmac], vec![TetraEntity::Phy]);

    test.submit_message(SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Lmac,
        msg: SapMsgInner::TmvUnitdataReq(TmvUnitdataReqSlot {
            ts: TdmaTime::default(),
            ul_phy_chan: PhysicalChannel::Cp,
            blk1: Some(TmvUnitdataReq {
                mac_block: BitBuffer::new(268),
                logical_channel: LogicalChannel::SchF,
                scrambling_code: TEST_SCRAMBLING_CODE,
            }),
            blk2: None,
            bbk: None,
        }),
    });
    test.deliver_all_messages();

    assert!(
        test.dump_sinks().is_empty(),
        "LMAC-MS must not guess wider uplink transmit mappings before they are implemented"
    );
}
