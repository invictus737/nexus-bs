use std::collections::VecDeque;

use tetra_core::{Direction, PhyBlockNum};
use tetra_saps::control::call_control::Circuit;

pub const MAX_TX_DATA_BLOCKS_PER_TIMESLOT: usize = 18;

#[derive(Debug)]
pub enum CircuitTxBlock {
    AcElp(Vec<u8>),
    RawTchSHalfSlot { block_num: PhyBlockNum, type5_bits: Vec<u8> },
}

pub struct CircuitMgr {
    pub dl: [Option<Circuit>; 4],
    pub ul: [Option<Circuit>; 4],

    /// Data blocks queued to be transmitted, per timeslot
    pub tx_data: [VecDeque<CircuitTxBlock>; 4],
}

impl CircuitMgr {
    pub fn new() -> Self {
        Self {
            dl: [None, None, None, None],
            ul: [None, None, None, None],
            tx_data: [VecDeque::new(), VecDeque::new(), VecDeque::new(), VecDeque::new()],
        }
    }

    pub fn is_active(&self, dir: Direction, ts: u8) -> bool {
        if !(1..=4).contains(&ts) {
            tracing::error!("UMAC CircuitMgr::is_active: invalid timeslot {}", ts);
            return false;
        }

        match dir {
            Direction::Dl => self.dl[ts as usize - 1].is_some(),
            Direction::Ul => self.ul[ts as usize - 1].is_some(),
            _ => {
                tracing::error!("UMAC CircuitMgr: called with non-specific direction {:?}", dir);
                return Default::default();
            }
        }
    }

    pub fn get_usage(&self, dir: Direction, ts: u8) -> Option<u8> {
        if !(1..=4).contains(&ts) {
            tracing::error!("UMAC CircuitMgr::get_usage: invalid timeslot {}", ts);
            return None;
        }

        match dir {
            Direction::Dl => {
                if let Some(circuit) = &self.dl[ts as usize - 1] {
                    Some(circuit.usage)
                } else {
                    None
                }
            }
            Direction::Ul => {
                if let Some(circuit) = &self.ul[ts as usize - 1] {
                    Some(circuit.usage)
                } else {
                    None
                }
            }
            _ => {
                tracing::error!("UMAC CircuitMgr: called with non-specific direction {:?}", dir);
                return Default::default();
            }
        }
    }

    /// Closes an active circuit, and return the Circuit to the caller
    pub fn close_circuit(&mut self, dir: Direction, ts: u8) -> Option<Circuit> {
        if !(1..=4).contains(&ts) {
            tracing::error!("UMAC CircuitMgr::close_circuit: invalid timeslot {}", ts);
            return None;
        }

        match dir {
            Direction::Dl => {
                self.tx_data[ts as usize - 1].clear();
                self.dl[ts as usize - 1].take()
            }
            Direction::Ul => self.ul[ts as usize - 1].take(),
            _ => {
                tracing::error!("UMAC CircuitMgr: called with non-specific direction {:?}", dir);
                return Default::default();
            }
        }
    }

    /// Creates a new circuit on the given direction and timeslot
    /// This channel should be free, if not, warnings will be issued and the existing circuit will be closed first
    pub fn create_circuit(&mut self, dir: Direction, circuit: Circuit) {
        let ts = circuit.ts;
        if !(1..=4).contains(&ts) {
            tracing::error!("UMAC CircuitMgr::create_circuit: invalid timeslot {}", ts);
            return;
        }

        // Sanity check
        if self.is_active(dir, ts) {
            tracing::warn!("CircuitMgr::create had still active circuit on {:?} {}", dir, ts);
            self.close_circuit(dir, ts);
        }

        match dir {
            Direction::Dl => {
                if !self.tx_data[ts as usize - 1].is_empty() {
                    tracing::warn!("CircuitMgr::create had pending tx_data on Dl {}", ts);
                    self.tx_data[ts as usize - 1].clear();
                }
                self.dl[ts as usize - 1] = Some(circuit);
            }
            Direction::Ul => self.ul[ts as usize - 1] = Some(circuit),
            _ => {
                tracing::error!("UMAC CircuitMgr: called with non-specific direction {:?}", dir);
                return Default::default();
            }
        }
    }

    /// Put a block in the queue for transmission on an associated channel
    pub fn put_block(&mut self, ts: u8, block: Vec<u8>) {
        if !(1..=4).contains(&ts) {
            tracing::error!("CircuitMgr::put_block on invalid timeslot {}", ts);
            return;
        }
        if !self.is_active(Direction::Dl, ts) {
            tracing::warn!("CircuitMgr::put_block on inactive circuit {:?} {}", Direction::Dl, ts);
            return;
        }
        self.push_tx_data_bounded(ts, CircuitTxBlock::AcElp(block), "ACELP");
    }

    pub fn put_raw_tch_s_half_slot(&mut self, ts: u8, block_num: PhyBlockNum, type5_bits: Vec<u8>) {
        if !(1..=4).contains(&ts) {
            tracing::error!("CircuitMgr::put_raw_tch_s_half_slot on invalid timeslot {}", ts);
            return;
        }
        if !self.is_active(Direction::Dl, ts) {
            tracing::warn!("CircuitMgr::put_raw_tch_s_half_slot on inactive circuit {:?} {}", Direction::Dl, ts);
            return;
        }
        self.push_tx_data_bounded(ts, CircuitTxBlock::RawTchSHalfSlot { block_num, type5_bits }, "raw TCH/S");
    }

    fn push_tx_data_bounded(&mut self, ts: u8, block: CircuitTxBlock, label: &str) {
        let queue = &mut self.tx_data[ts as usize - 1];
        if queue.len() >= MAX_TX_DATA_BLOCKS_PER_TIMESLOT {
            queue.pop_front();
            tracing::warn!(
                "CircuitMgr: dropping oldest queued DL {} block on ts {} because media queue reached {} block(s)",
                label,
                ts,
                MAX_TX_DATA_BLOCKS_PER_TIMESLOT
            );
        }
        queue.push_back(block);
    }

    pub fn clear_tx_data(&mut self, ts: u8) -> usize {
        if !(1..=4).contains(&ts) {
            tracing::warn!("CircuitMgr::clear_tx_data on invalid timeslot {}", ts);
            return 0;
        }
        let queue = &mut self.tx_data[ts as usize - 1];
        let dropped = queue.len();
        queue.clear();
        dropped
    }

    /// Take a to-be-transmitted block from the queue
    pub fn take_block(&mut self, ts: u8) -> Option<CircuitTxBlock> {
        if !self.is_active(Direction::Dl, ts) {
            tracing::warn!("CircuitMgr::take_block on inactive circuit {:?} {}", Direction::Dl, ts);
            return None;
        }
        self.tx_data[ts as usize - 1].pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_saps::control::call_control::CircuitDlMediaSource;
    use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;

    fn test_dl_circuit(ts: u8) -> Circuit {
        Circuit {
            direction: Direction::Dl,
            ts,
            peer_ts: None,
            usage: 4,
            circuit_mode: CircuitModeType::TchS,
            speech_service: Some(0),
            etee_encrypted: false,
            dl_media_source: CircuitDlMediaSource::LocalLoopback,
            active_addr: None,
            active_secondary_addrs: Vec::new(),
        }
    }

    #[test]
    fn dl_media_queue_drops_oldest_acelp_blocks_when_bounded() {
        let mut circuits = CircuitMgr::new();
        circuits.create_circuit(Direction::Dl, test_dl_circuit(2));

        for seq in 0..(MAX_TX_DATA_BLOCKS_PER_TIMESLOT + 3) {
            circuits.put_block(2, vec![seq as u8]);
        }

        assert_eq!(circuits.tx_data[1].len(), MAX_TX_DATA_BLOCKS_PER_TIMESLOT);
        match circuits.take_block(2).expect("bounded queue should retain latest ACELP blocks") {
            CircuitTxBlock::AcElp(block) => assert_eq!(block, vec![3], "oldest overfed ACELP frames should be dropped before newer speech"),
            other => panic!("expected ACELP block, got {other:?}"),
        }
    }

    #[test]
    fn dl_media_queue_drops_oldest_raw_tch_s_blocks_when_bounded() {
        let mut circuits = CircuitMgr::new();
        circuits.create_circuit(Direction::Dl, test_dl_circuit(3));

        for seq in 0..(MAX_TX_DATA_BLOCKS_PER_TIMESLOT + 2) {
            circuits.put_raw_tch_s_half_slot(3, PhyBlockNum::Block2, vec![seq as u8; 216]);
        }

        assert_eq!(circuits.tx_data[2].len(), MAX_TX_DATA_BLOCKS_PER_TIMESLOT);
        match circuits.take_block(3).expect("bounded queue should retain latest raw TCH/S blocks") {
            CircuitTxBlock::RawTchSHalfSlot { type5_bits, .. } => assert_eq!(
                type5_bits[0], 2,
                "oldest overfed raw TCH/S frames should be dropped before newer speech"
            ),
            other => panic!("expected raw TCH/S block, got {other:?}"),
        }
    }
}
