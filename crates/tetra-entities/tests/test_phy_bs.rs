#[path = "common/default_stack.rs"]
#[allow(dead_code)]
mod default_stack;

use std::sync::{Arc, Mutex};

use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, BurstType, Sap, TdmaTime, TrainingSequence};
use tetra_entities::phy::phy_bs::PhyBs;
use tetra_entities::{MessageQueue, TetraEntityTrait};
use tetra_pdus::phy::traits::rxtx_dev::{RxSlotBits, RxTxDev, RxTxDevError, TxSlotBits};
use tetra_saps::tp::TpUnitdataReqSlot;
use tetra_saps::{SapMsg, SapMsgInner};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TxCall {
    slots: usize,
    has_payload: bool,
}

#[derive(Clone)]
struct CapturingRxTxDev {
    calls: Arc<Mutex<Vec<TxCall>>>,
    tx_inhibit_calls: Arc<Mutex<Vec<bool>>>,
}

impl RxTxDev for CapturingRxTxDev {
    fn set_tx_inhibited(&mut self, inhibited: bool) -> Result<(), RxTxDevError> {
        self.tx_inhibit_calls.lock().expect("inhibit lock").push(inhibited);
        Ok(())
    }

    fn rxtx_timeslot(&mut self, tx_slot: &[TxSlotBits]) -> Result<Vec<Option<RxSlotBits<'_>>>, RxTxDevError> {
        self.calls.lock().expect("capture lock").push(TxCall {
            slots: tx_slot.len(),
            has_payload: tx_slot.iter().any(|slot| slot.slot.is_some()),
        });
        Ok(Vec::new())
    }
}

fn downlink_tp_req() -> SapMsg {
    SapMsg {
        sap: Sap::TpSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Phy,
        msg: SapMsgInner::TpUnitdataReq(TpUnitdataReqSlot {
            train_type: TrainingSequence::NormalTrainSeq1,
            burst_type: BurstType::NDB,
            bbk: Some(BitBuffer::new(30)),
            blk1: Some(BitBuffer::new(432)),
            blk2: None,
        }),
    }
}

#[test]
fn carrier_inhibit_hard_gates_tx_stream_but_keeps_phy_rx_tick() {
    let config = SharedConfig::from_parts(default_stack::default_test_config_bs(), None);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let tx_inhibit_calls = Arc::new(Mutex::new(Vec::new()));
    let dev = CapturingRxTxDev {
        calls: Arc::clone(&calls),
        tx_inhibit_calls: Arc::clone(&tx_inhibit_calls),
    };
    let mut phy = PhyBs::new(config.clone(), dev);
    let mut queue = MessageQueue::new();

    phy.tick_start(&mut queue, TdmaTime::default());
    phy.rx_prim(&mut queue, downlink_tp_req());
    {
        let captured = calls.lock().expect("capture lock");
        assert_eq!(
            captured.as_slice(),
            &[TxCall {
                slots: 1,
                has_payload: true
            }]
        );
    }
    assert!(tx_inhibit_calls.lock().expect("inhibit lock").is_empty());

    config.state_write().carrier_inhibited = true;
    phy.rx_prim(&mut queue, downlink_tp_req());
    phy.rx_prim(&mut queue, downlink_tp_req());

    let captured = calls.lock().expect("capture lock");
    assert_eq!(
        captured.as_slice(),
        &[
            TxCall {
                slots: 1,
                has_payload: true
            },
            TxCall {
                slots: 0,
                has_payload: false,
            },
            TxCall {
                slots: 0,
                has_payload: false,
            },
        ],
        "carrier inhibit must keep the RX/timing tick while suppressing all downlink slot payloads"
    );
    assert_eq!(
        tx_inhibit_calls.lock().expect("inhibit lock").as_slice(),
        &[true],
        "carrier inhibit must hard-gate the RF device once, not just omit TX slot payloads"
    );

    drop(captured);
    config.state_write().carrier_inhibited = false;
    phy.rx_prim(&mut queue, downlink_tp_req());

    assert_eq!(
        tx_inhibit_calls.lock().expect("inhibit lock").as_slice(),
        &[true, false],
        "carrier enable must re-enable the RF TX path once"
    );
    assert_eq!(
        calls.lock().expect("capture lock").last().copied(),
        Some(TxCall {
            slots: 1,
            has_payload: true,
        }),
        "carrier enable must resume normal downlink TX slots"
    );
}
