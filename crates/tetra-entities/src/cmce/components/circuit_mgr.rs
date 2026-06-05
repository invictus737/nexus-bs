use std::collections::{HashSet, VecDeque};

use tetra_core::{Direction, TdmaTime, TimeslotAllocator, TimeslotOwner, frames, multiframes};
use tetra_pdus::cmce::structs::cmce_circuit::CmceCircuit;
use tetra_saps::{
    control::enums::{circuit_mode_type::CircuitModeType, communication_type::CommunicationType},
    lcmc::CallId,
};

const D_SETUP_REPEATS: i32 = 1;
const LATE_ENTRY_INTERVAL_TIMESLOTS: i32 = multiframes!(5);
const MAX_CALL_IDENTIFIER: u16 = 0x3FFF;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitErr {
    NoCircuitFree,
    CircuitAlreadyInUse,
    CircuitNotActive,
    CallIdentifierExhausted,
}

pub enum CircuitMgrCmd {
    SendDSetup(CallId, u8, u8), // call id, usage number, timeslot
    SendClose(CallId, CmceCircuit),
}

pub struct CircuitMgr {
    pub dltime: TdmaTime,

    /// Holds any Dl and Dl+Ul circuits
    pub dl: [Option<CmceCircuit>; 4],
    /// Holds any Ul-only circuits, with no recipients on this cell
    pub ul_only: [Option<CmceCircuit>; 4],

    /// Data blocks queued to be transmitted, per timeslot
    pub tx_data: [VecDeque<Vec<u8>>; 4],

    /// 14-bit call identifier. Zero value is reserved.
    pub next_call_identifier: u16,
    /// 5-bit usage number. Values 0-3 are reserved.
    pub next_usage_number: u8,
}

impl CircuitMgr {
    pub fn new() -> Self {
        Self {
            dltime: TdmaTime::default(),
            dl: [None, None, None, None],
            ul_only: [None, None, None, None],
            tx_data: [VecDeque::new(), VecDeque::new(), VecDeque::new(), VecDeque::new()],
            next_call_identifier: 4,
            next_usage_number: 4,
        }
    }

    /// Checks if a circuit is active on the given timeslot
    /// Returns (dl_active, ul_active)
    pub fn is_active(&self, ts: u8) -> (bool, bool) {
        match &self.dl[ts as usize - 1] {
            Some(dl) => {
                if dl.direction == Direction::Both {
                    (true, true)
                } else {
                    (true, self.ul_only[ts as usize - 1].is_some())
                }
            }
            None => (false, self.ul_only[ts as usize - 1].is_some()),
        }
    }

    /// Checks if a circuit is active on the given timeslot and direction
    /// Direction must be Dl or Ul
    pub fn is_active_dir(&self, ts: u8, dir: Direction) -> bool {
        match dir {
            Direction::Dl => self.dl[ts as usize - 1].is_some(),
            Direction::Ul => {
                let dl_is_both = if let Some(dl) = &self.dl[ts as usize - 1] {
                    if self.ul_only[ts as usize - 1].is_some() {
                        tracing::warn!(
                            "CMCE: circuit_mgr ts={} has both dl and ul_only set simultaneously (invariant violation)",
                            ts
                        );
                    }
                    dl.direction == Direction::Both
                } else {
                    false
                };
                self.ul_only[ts as usize - 1].is_some() || dl_is_both
            }

            _ => {
                tracing::error!("CMCE: is_active_dir called with non-specific direction {:?}, returning false", dir);
                false
            }
        }
    }

    /// Gets the usage number of an active circuit, (Option<dl_usage>, Option<ul_usage>)
    pub fn get_usage(&self, ts: u8) -> (Option<u8>, Option<u8>) {
        let (dl_usage, dl_is_both) = if let Some(dl) = &self.dl[ts as usize - 1] {
            (Some(dl.usage), dl.direction == Direction::Both)
        } else {
            (None, false)
        };
        let ul_usage = if dl_is_both {
            assert!(self.ul_only[ts as usize - 1].is_none());
            dl_usage
        } else if let Some(ul) = &self.ul_only[ts as usize - 1] {
            Some(ul.usage)
        } else {
            None
        };
        (dl_usage, ul_usage)
    }

    pub fn get_next_call_id(&mut self) -> CallId {
        let call_id = Self::normal_call_identifier(self.next_call_identifier);
        self.next_call_identifier = Self::call_identifier_after(call_id);
        call_id
    }

    pub fn get_next_call_id_avoiding(&mut self, occupied: &HashSet<u16>) -> Result<CallId, CircuitErr> {
        for _ in 0..MAX_CALL_IDENTIFIER {
            let call_id = self.get_next_call_id();
            // EN 300 392-2 table 14.36 and clause 14.2.3 require the
            // SwMI-allocated call identifier to remain the reference for one
            // specific call. Skip live/pending identifiers when the 14-bit
            // namespace wraps; value 0 remains the dummy identifier.
            if !occupied.contains(&call_id) {
                return Ok(call_id);
            }
        }

        Err(CircuitErr::CallIdentifierExhausted)
    }

    fn normal_call_identifier(call_id: u16) -> u16 {
        if (1..=MAX_CALL_IDENTIFIER).contains(&call_id) { call_id } else { 1 }
    }

    fn call_identifier_after(call_id: u16) -> u16 {
        if call_id >= MAX_CALL_IDENTIFIER { 1 } else { call_id + 1 }
    }

    pub fn get_next_usage_number(&mut self) -> u8 {
        let usage = self.next_usage_number;
        self.next_usage_number += 1;
        if self.next_usage_number > 63 {
            self.next_usage_number = 4; // Wrap around, skip reserved values
        }
        usage
    }

    /// Finds a free timeslot for the given direction (Ul, Dl or Both)
    fn get_free_ts(&self, dir: Direction) -> Result<u8, CircuitErr> {
        // TODO FIXME we may do a bit smarter allocation here
        for ts in 2..=4 {
            let (dl_active, ul_active) = self.is_active(ts);
            match (dir, dl_active, ul_active) {
                (Direction::Dl, false, _) => return Ok(ts),
                (Direction::Ul, false, false) => return Ok(ts),
                (Direction::Ul, true, false) => {
                    // Check if dl circuit covers Dl+Ul
                    let dl = self.dl[ts as usize - 1].as_ref().unwrap();
                    if dl.direction != Direction::Both {
                        return Ok(ts);
                    }
                }
                (Direction::Both, false, false) => return Ok(ts),
                _ => {}
            }
        }
        Err(CircuitErr::NoCircuitFree)
    }

    pub fn allocate_circuit(&mut self, dir: Direction, comm_type: CommunicationType) -> Result<&CmceCircuit, CircuitErr> {
        self.allocate_circuit_duplex(dir, comm_type, false)
    }

    pub fn allocate_circuit_duplex(
        &mut self,
        dir: Direction,
        comm_type: CommunicationType,
        simplex_duplex: bool,
    ) -> Result<&CmceCircuit, CircuitErr> {
        // Get timeslot, call_id and usage
        let ts = self.get_free_ts(dir)?;
        let call_id = self.get_next_call_id();
        let usage = self.get_next_usage_number();

        // Create circuit
        let circuit = CmceCircuit {
            ts_created: self.dltime,
            direction: dir,
            ts,
            call_id,
            usage,
            circuit_mode: CircuitModeType::TchS,
            comm_type,
            simplex_duplex,
            speech_service: Some(0),
            etee_encrypted: false,
        };

        // Register circuit and return
        Ok(self.open_circuit(dir, circuit)?)
    }

    /// Allocate circuit using centralized timeslot allocator
    pub fn allocate_circuit_with_allocator(
        &mut self,
        dir: Direction,
        comm_type: CommunicationType,
        timeslot_alloc: &mut TimeslotAllocator,
        owner: TimeslotOwner,
    ) -> Result<&CmceCircuit, CircuitErr> {
        self.allocate_circuit_with_allocator_duplex(dir, comm_type, false, timeslot_alloc, owner)
    }

    /// Allocate circuit using centralized timeslot allocator with explicit duplex flag.
    pub fn allocate_circuit_with_allocator_duplex(
        &mut self,
        dir: Direction,
        comm_type: CommunicationType,
        simplex_duplex: bool,
        timeslot_alloc: &mut TimeslotAllocator,
        owner: TimeslotOwner,
    ) -> Result<&CmceCircuit, CircuitErr> {
        self.allocate_circuit_with_allocator_duplex_avoiding(dir, comm_type, simplex_duplex, timeslot_alloc, owner, &HashSet::new())
    }

    /// Allocate circuit using centralized timeslot allocator with explicit duplex flag,
    /// while avoiding live CMCE call identifiers held by higher-layer state.
    pub fn allocate_circuit_with_allocator_duplex_avoiding(
        &mut self,
        dir: Direction,
        comm_type: CommunicationType,
        simplex_duplex: bool,
        timeslot_alloc: &mut TimeslotAllocator,
        owner: TimeslotOwner,
        occupied_call_ids: &HashSet<u16>,
    ) -> Result<&CmceCircuit, CircuitErr> {
        // Get timeslot from centralized allocator
        let ts = timeslot_alloc.allocate_any(owner).ok_or(CircuitErr::NoCircuitFree)?;

        let call_id = match self.get_next_call_id_avoiding(occupied_call_ids) {
            Ok(call_id) => call_id,
            Err(err) => {
                let _ = timeslot_alloc.release(owner, ts);
                return Err(err);
            }
        };
        let usage = self.get_next_usage_number();

        // Create circuit
        let circuit = CmceCircuit {
            ts_created: self.dltime,
            direction: dir,
            ts,
            call_id,
            usage,
            circuit_mode: CircuitModeType::TchS,
            comm_type,
            simplex_duplex,
            speech_service: Some(0),
            etee_encrypted: false,
        };

        // Register circuit and return
        match self.open_circuit(dir, circuit) {
            Ok(circuit) => Ok(circuit),
            Err(err) => {
                let _ = timeslot_alloc.release(owner, ts);
                Err(err)
            }
        }
    }

    /// Allocate an additional circuit for an existing call_id using the centralized allocator.
    /// Used for duplex P2P calls where calling and called MS need separate timeslots.
    pub fn allocate_circuit_for_call_with_allocator(
        &mut self,
        call_id: u16,
        dir: Direction,
        comm_type: CommunicationType,
        simplex_duplex: bool,
        timeslot_alloc: &mut TimeslotAllocator,
        owner: TimeslotOwner,
    ) -> Result<&CmceCircuit, CircuitErr> {
        let ts = timeslot_alloc.allocate_any(owner).ok_or(CircuitErr::NoCircuitFree)?;
        let usage = self.get_next_usage_number();

        let circuit = CmceCircuit {
            ts_created: self.dltime,
            direction: dir,
            ts,
            call_id,
            usage,
            circuit_mode: CircuitModeType::TchS,
            comm_type,
            simplex_duplex,
            speech_service: Some(0),
            etee_encrypted: false,
        };

        Ok(self.open_circuit(dir, circuit)?)
    }

    /// Closes any active circuits for given timeslot and direction.
    /// Returns the CmceCircuit
    /// When direction is Both, closes both directions
    pub fn close_circuit(&mut self, dir: Direction, ts: u8) -> Result<CmceCircuit, CircuitErr> {
        match dir {
            Direction::Dl | Direction::Both => {
                self.tx_data[ts as usize - 1].clear();
                if dir == Direction::Both && self.ul_only[ts as usize - 1].is_some() {
                    tracing::warn!("Closing Dl+Ul circuit on ts {} while Ul-only circuit exists", ts);
                }
                let circuit = self.dl[ts as usize - 1].take();
                circuit.ok_or(CircuitErr::CircuitNotActive)
            }
            Direction::Ul => {
                let circuit = self.ul_only[ts as usize - 1].take();
                circuit.ok_or(CircuitErr::CircuitNotActive)
            }
            _ => unreachable!("BUG: unhandled match variant -- should never be reached"),
        }
    }

    /// Creates a new circuit on the given direction and timeslot
    /// This channel should be free, if not, warnings will be issued and existing circuit will be closed first
    /// Consumes the circuit but returns a reference
    fn open_circuit(&mut self, dir: Direction, circuit: CmceCircuit) -> Result<&CmceCircuit, CircuitErr> {
        // Sanity check, close circuit and issue warning if exists
        let ts = circuit.ts;
        let (dl_active, ul_active) = self.is_active(ts);
        if dir.includes_dl() && dl_active {
            return Err(CircuitErr::CircuitAlreadyInUse);
        }
        if dir.includes_ul() && ul_active {
            return Err(CircuitErr::CircuitAlreadyInUse);
        }

        match dir {
            Direction::Dl | Direction::Both => {
                if !self.tx_data[ts as usize - 1].is_empty() {
                    tracing::warn!("CircuitMgr::create had pending tx_data on Dl {}", ts);
                    self.tx_data[ts as usize - 1].clear();
                }
                self.dl[ts as usize - 1] = Some(circuit);
                Ok(self.dl[ts as usize - 1].as_ref().unwrap())
            }
            Direction::Ul => {
                self.ul_only[ts as usize - 1] = Some(circuit);
                Ok(self.ul_only[ts as usize - 1].as_ref().unwrap())
            }
            _ => unreachable!("BUG: unhandled match variant -- should never be reached"),
        }
    }

    /// Put a block in the queue for transmission on an associated channel
    pub fn put_block(&mut self, ts: u8, block: Vec<u8>) -> Result<(), CircuitErr> {
        if !self.is_active_dir(ts, Direction::Dl) {
            Err(CircuitErr::CircuitNotActive)
        } else {
            self.tx_data[ts as usize - 1].push_back(block);
            Ok(())
        }
    }

    /// Take a to-be-transmitted block from the queue
    pub fn take_block(&mut self, ts: u8) -> Result<Option<Vec<u8>>, CircuitErr> {
        if !self.is_active_dir(ts, Direction::Dl) {
            return Err(CircuitErr::CircuitNotActive);
        } else {
            Ok(self.tx_data[ts as usize - 1].pop_front())
        }
    }

    pub fn active_call_ids(&self) -> Vec<u16> {
        let mut ids = Vec::new();
        for circuit in self.dl.iter().chain(self.ul_only.iter()).flatten() {
            if !ids.contains(&circuit.call_id) {
                ids.push(circuit.call_id);
            }
        }
        ids
    }

    /// Closes any circuits that have expired.
    /// Safety timeout for simplex (HDX/PTT) circuits: 6 minutes (beyond T5m).
    /// Full-duplex (FDX) circuits — normal voice calls — have no timeout here;
    /// they are released by normal call signalling (U-DISCONNECT / CALL_RELEASE).
    /// ETSI EN 300 392-2 §14.9: call timeout does not apply to FDX individual calls.
    fn close_expired_circuits(&mut self, mut tasks: Option<Vec<CircuitMgrCmd>>) -> Option<Vec<CircuitMgrCmd>> {
        const CIRCUIT_EXPIRY_TIMESLOTS: i32 = 6 * 60 * 18 * 4; // 6 minutes for simplex

        let mut to_close: Vec<_> = self
            .dl
            .iter()
            .filter_map(|circuit| circuit.as_ref())
            // FDX circuits (simplex_duplex=true) have no safety timeout — skip them.
            .filter(|circuit| !circuit.simplex_duplex)
            .filter(|circuit| circuit.ts_created.age(self.dltime) > CIRCUIT_EXPIRY_TIMESLOTS)
            .map(|circuit| (circuit.direction, circuit.ts, circuit.call_id))
            .collect();
        to_close.extend(
            self.ul_only
                .iter()
                .filter_map(|circuit| circuit.as_ref())
                .filter(|circuit| !circuit.simplex_duplex)
                .filter(|circuit| circuit.ts_created.age(self.dltime) > CIRCUIT_EXPIRY_TIMESLOTS)
                .map(|circuit| (circuit.direction, circuit.ts, circuit.call_id)),
        );
        for (dir, ts, call_id) in to_close {
            match self.close_circuit(dir, ts) {
                Ok(circuit) => {
                    tasks.get_or_insert_with(Vec::new).push(CircuitMgrCmd::SendClose(call_id, circuit));
                }
                Err(_) => {
                    // Already closed by normal release path racing with the expiry timer — safe to ignore.
                    tracing::debug!(
                        "circuit_mgr: expiry close skipped for call_id={} ts={} dir={:?} (already closed)",
                        call_id,
                        ts,
                        dir
                    );
                }
            }
        }
        tasks
    }

    pub fn tick_start(&mut self, dltime: TdmaTime) -> Option<Vec<CircuitMgrCmd>> {
        self.dltime = dltime;
        let mut tasks = None;

        if dltime.t == 1 {
            // First, close any expired circuits
            tasks = self.close_expired_circuits(tasks);

            // Next, go through channels, see if D-SETUPs need to be sent
            // Late entry: resend D-SETUP every 5 seconds
            for circuit in self.dl.iter() {
                if let Some(circuit) = circuit {
                    let age = circuit.ts_created.age(dltime);

                    // Send D-SETUP for the initial frame + 1 backup frame after circuit creation.
                    // Matches ETSI EN 300 392-2 Annex D Figure D.2: 1 initial + 1 back-up
                    // on MCCH. Include the exact one-frame boundary because this check
                    // only runs on t1; otherwise a circuit created on t1 skips the backup.
                    if (1..=frames!(D_SETUP_REPEATS)).contains(&age) {
                        tasks
                            .get_or_insert_with(Vec::new)
                            .push(CircuitMgrCmd::SendDSetup(circuit.call_id, circuit.usage, circuit.ts));
                    }
                    // Late entry: resend every 5 seconds.
                    // Compare in frames (age/4) since tick_start only fires on t==1
                    // but ts_created may have any timeslot value.
                    else if (age / 4) % (LATE_ENTRY_INTERVAL_TIMESLOTS / 4) == 0 {
                        tasks
                            .get_or_insert_with(Vec::new)
                            .push(CircuitMgrCmd::SendDSetup(circuit.call_id, circuit.usage, circuit.ts));
                    }
                }
            }
            return tasks;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_d_setup_is_sent_at_exact_one_frame_boundary() {
        let mut mgr = CircuitMgr::new();
        let created = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
        mgr.dltime = created;
        let mut timeslot_alloc = TimeslotAllocator::default();
        let circuit = mgr
            .allocate_circuit_with_allocator(Direction::Dl, CommunicationType::P2Mp, &mut timeslot_alloc, TimeslotOwner::Cmce)
            .expect("test circuit should allocate")
            .clone();

        // EN 300 392-2 Annex D permits a back-up D-SETUP during group setup.
        // The scheduler only checks on t1, so a circuit created on t1 reaches
        // the backup check exactly one TDMA frame later.
        let tasks = mgr
            .tick_start(created.add_timeslots(4))
            .expect("exact one-frame boundary should queue tasks");

        assert!(
            tasks.iter().any(|cmd| matches!(
                cmd,
                CircuitMgrCmd::SendDSetup(call_id, usage, ts)
                    if *call_id == circuit.call_id && *usage == circuit.usage && *ts == circuit.ts
            )),
            "backup D-SETUP should be queued at the exact one-frame boundary"
        );
    }

    #[test]
    fn call_identifier_uses_full_14_bit_range_and_skips_dummy_zero() {
        let mut mgr = CircuitMgr::new();
        mgr.next_call_identifier = 0x3FFE;

        // EN 300 392-2 table 14.36 gives a 14-bit call identifier field.
        // Value 0 is the dummy call identifier; real SwMI allocations use
        // 1..=16383 and then wrap back to 1.
        assert_eq!(mgr.get_next_call_id(), 0x3FFE);
        assert_eq!(mgr.get_next_call_id(), 0x3FFF);
        assert_eq!(mgr.get_next_call_id(), 1);
        assert_eq!(mgr.next_call_identifier, 2);
    }

    #[test]
    fn call_identifier_wrap_skips_occupied_live_ids() {
        let mut mgr = CircuitMgr::new();
        mgr.next_call_identifier = 0x3FFE;
        let occupied = HashSet::from([0x3FFE, 0x3FFF, 1, 2]);

        assert_eq!(
            mgr.get_next_call_id_avoiding(&occupied),
            Ok(3),
            "wrapped allocation must skip live CMCE call identifiers"
        );
        assert_eq!(mgr.next_call_identifier, 4);
    }

    #[test]
    fn call_identifier_allocator_reports_exhaustion_when_all_real_ids_are_live() {
        let mut mgr = CircuitMgr::new();
        let occupied: HashSet<_> = (1..=MAX_CALL_IDENTIFIER).collect();

        assert_eq!(mgr.get_next_call_id_avoiding(&occupied), Err(CircuitErr::CallIdentifierExhausted));
    }
}
