use std::collections::HashMap;

use tetra_config::bluestation::{EnergySavingAssignment, SubscriberRegistry};
use tetra_core::{
    BitBuffer, Direction, PhyBlockNum, PhysicalChannel, SsiType, TdmaTime, TetraAddress, Todo, TxReporter, unimplemented_log,
};
use tetra_saps::{
    control::call_control::{Circuit, CircuitDlMediaSource},
    tmv::{TmvUnitdataReq, TmvUnitdataReqSlot, enums::logical_chans::LogicalChannel},
};

use tetra_pdus::{
    mle::pdus::{d_mle_sync::DMleSync, d_mle_sysinfo::DMleSysinfo},
    umac::{
        enums::{
            access_assign_dl_usage::AccessAssignDlUsage, access_assign_ul_usage::AccessAssignUlUsage,
            basic_slotgrant_cap_alloc::BasicSlotgrantCapAlloc, basic_slotgrant_granting_delay::BasicSlotgrantGrantingDelay,
            reservation_requirement::ReservationRequirement,
        },
        fields::basic_slotgrant::BasicSlotgrant,
        pdus::{
            access_assign::{AccessAssign, AccessField},
            access_assign_fr18::AccessAssignFr18,
            mac_resource::MacResource,
            mac_sync::MacSync,
            mac_sysinfo::MacSysinfo,
        },
    },
};

use crate::{
    lmac::components::scrambler,
    umac::subcomp::{
        bs_frag::BsFragger,
        circuit_mgr::{CircuitMgr, CircuitTxBlock},
    },
};

/// We submit this many TX timeslots ahead of the current time
pub const MACSCHED_TX_AHEAD: usize = 1;

// We schedule up to this many frames ahead
pub const MACSCHED_NUM_FRAMES: usize = 18;

const NULL_PDU_LEN_BITS: usize = 16;

pub const SCH_HD_CAP: usize = 124;
pub const SCH_F_CAP: usize = 268;
pub const TCH_S_CAP: usize = 274;

/// Number of timeslots the scheduler operates on. May become larger when secondary carriers are supported.
pub const NUM_TIMESLOTS: usize = 4;

const PREDEFINED_BROADCAST_GSSI: u32 = 0xFF_FFFF;

enum DlTchBlock {
    AcElp(BitBuffer),
    RawTchSHalfSlot { block_num: PhyBlockNum, type5_bits: BitBuffer },
}

#[derive(Debug)]
pub struct PrecomputedUmacPdus {
    pub mac_sysinfo1: MacSysinfo,
    pub mac_sysinfo2: MacSysinfo,
    pub mle_sysinfo: DMleSysinfo,
    pub mac_sync: MacSync,
    pub mle_sync: DMleSync,
}

#[derive(Debug)]
pub struct TimeslotSchedule {
    pub ul1: Option<u32>,
    pub ul2: Option<u32>,
    /// Usage marker (4-62) issued to an MS that received a multi-slot grant.
    /// When set, AACH for this slot signals `Traffic(marker)` so the MS knows
    /// the slot is reserved for it. The marker remains until both ul1 and ul2
    /// are consumed/freed.
    ///
    /// Per ETSI TS 100 392-2 §23.5.1: usage markers 0 (= unallocated) and
    /// 1-3 are reserved. 63 (= common linearisation channel) is reserved.
    /// Valid range for BS-assigned reservations is 4..=62.
    pub usage_marker: Option<u8>,
    // pub dl: Option<TmvUnitdataReq>,
}

// #[derive(Debug)]
pub struct BsChannelScheduler {
    pub cur_dltime: TdmaTime,
    scrambling_code: u32,
    precomps: PrecomputedUmacPdus,
    /// Collect dltx traffic here that can't be sent this slot.
    /// Swapped back into the dltx_queues method at the end of the tick.
    dltx_next_slot_queue: Vec<DlSchedElem>,
    /// Four queues for scheduled downlink traffic, one per timeslot
    dltx_queues: [Vec<DlSchedElem>; 4],
    ulsched: [[TimeslotSchedule; MACSCHED_NUM_FRAMES]; 4],

    circuits: CircuitMgr,

    /// When true, the given timeslot is in call hangtime: keep circuit allocated but stop
    /// sending traffic-plane TCH blocks. Instead, transmit signalling-plane idle (Null PDUs)
    /// and signal UL usage as AssignedOnly so MS can request the floor.
    hangtime: [bool; 4],

    /// Per-timeslot set of addresses whose RandomAccessAck was dropped by dl_drop_all_except_stolen.
    /// The next STCH built for a matching SSI should carry random_access_flag=true to properly
    /// acknowledge the random access per ETSI 21.4.3.1.
    pending_ra_acks: [Vec<TetraAddress>; 4],

    /// True if a MAC-RESOURCE PDU with a chan_alloc element has already been enqueued for ts1
    /// in the current frame. The second such PDU (e.g. DConnectAck MCCH) must be deferred to
    /// the next frame to avoid exceeding the 216-bit slot capacity (DConnect+DConnectAck=223 bits).
    mcch_chan_alloc_sent_this_frame: bool,

    /// Per-timeslot rotating cursor for allocating usage markers to multi-slot
    /// uplink reservations. Wraps in the valid range [4, 62] (0 = unallocated,
    /// 1-3 reserved, 63 = common linearisation; per ETSI TS 100 392-2 §23.5.1).
    ///
    /// A multi-slot grant without a usage_marker leaves the MS unable to
    /// associate AACH slot signalling with its own reservation — empirically
    /// MS-side stacks (MXP600 etc.) abandon the burst after the first slot and
    /// fall back to repeated random access, which never completes a
    /// fragmented MM PDU (e.g. ULocationUpdate when re-entering coverage).
    /// Issuing a real marker fixes that.
    next_usage_marker: [u8; 4],
}

#[derive(Debug)]
pub enum DlSchedElem {
    /// A SYSINFO or neighboring cells info block. The integer determines which of the precomputed blocks to use (SYSINFO1, SYSINFO2, NEIGHBORING_CELLS
    Broadcast(Todo),

    /// A received MAC-ACCESS PDU still has to be acknowledged
    RandomAccessAck(TetraAddress),

    /// A slotgrant response, which has to be transmitted with high priority or the delay numbers will be off.
    /// ssi, BasicSlotgrant, and an optional usage_marker are provided. When the grant covers >1 slot the
    /// scheduler allocates a usage marker so AACH and the MacResource ACK can identify the reservation
    /// (per ETSI TS 100 392-2 §21.4.3.2 and §23.5.1); single-slot grants don't need one.
    Grant(TetraAddress, BasicSlotgrant, Option<u8>),

    /// A grant requested by an uplink reservation requirement. Capacity is
    /// reserved only when this element is integrated into the actual
    /// MAC-RESOURCE transmission, so EG-delayed downlink grants do not point at
    /// already-expired uplink opportunities.
    PendingGrant(TetraAddress, ReservationRequirement),

    /// A MAC-RESOURCE PDU. May be split into fragments upon processing, in which case a FragBuf will be inserted after processing the resource.
    Resource(MacResource, BitBuffer, Option<TxReporter>, Option<GroupDeliveryState>),

    /// A FragBuf containing remaining non-transmitted information after a MAC-RESOURCE start has been transmitted
    FragBuf(BsFragger, Option<GroupDeliveryState>),

    /// Pre-built STCH block for FACCH/stealing a half-slot from traffic channel.
    /// Contains MAC-U-SIGNAL (3 bits) + TM-SDU = 124 type1 bits.
    /// Delivers time-critical signaling (D-TX CEASED, D-TX GRANTED) per EN 300 392-2, clause 23.5.
    Stealing(BitBuffer, TetraAddress, Option<TxReporter>, Option<GroupStealingState>),
}

/// Delivery state for GSSI-addressed signalling while members use Energy Economy.
///
/// ETSI EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6 require the BS to account for
/// an MS's energy economy receive windows when sending downlink PDUs. A single
/// GSSI MAC-RESOURCE may be missed by sleeping affiliates, so the scheduler keeps
/// transmitting the same GSSI-addressed PDU until every known affiliated ISSI has
/// had a listening opportunity. For the predefined all-ones broadcast GSSI we use
/// the registered ISSI set for coverage, but we do not extend T.210 because clause
/// 23.7.6 explicitly excludes that address from sleep-cycle suspension.
#[derive(Debug, Clone)]
pub struct GroupDeliveryState {
    original_pdu: MacResource,
    original_sdu: BitBuffer,
    targets: Vec<u32>,
    covered: Vec<u32>,
    active_batch: Vec<u32>,
    tx_reporter: Option<TxReporter>,
    suspend_t210: bool,
}

impl GroupDeliveryState {
    fn new(
        original_pdu: MacResource,
        original_sdu: BitBuffer,
        targets: Vec<u32>,
        tx_reporter: Option<TxReporter>,
        suspend_t210: bool,
    ) -> Self {
        Self {
            original_pdu,
            original_sdu,
            targets,
            covered: Vec::new(),
            active_batch: Vec::new(),
            tx_reporter,
            suspend_t210,
        }
    }

    fn is_complete(&self) -> bool {
        self.targets.iter().all(|issi| self.covered.contains(issi))
    }

    fn is_final_batch(&self) -> bool {
        self.targets
            .iter()
            .all(|issi| self.covered.contains(issi) || self.active_batch.contains(issi))
    }

    fn uncovered_listeners(&self, ts: TdmaTime, energy_saving: &HashMap<u32, EnergySavingAssignment>) -> Vec<u32> {
        self.targets
            .iter()
            .copied()
            .filter(|issi| !self.covered.contains(issi))
            .filter(|issi| Self::ms_listens_at(energy_saving, *issi, ts))
            .collect()
    }

    fn active_batch_listens(&self, ts: TdmaTime, energy_saving: &HashMap<u32, EnergySavingAssignment>) -> bool {
        self.active_batch
            .iter()
            .copied()
            .all(|issi| Self::ms_listens_at(energy_saving, issi, ts))
    }

    fn ms_listens_at(energy_saving: &HashMap<u32, EnergySavingAssignment>, issi: u32, ts: TdmaTime) -> bool {
        energy_saving.get(&issi).map(|assignment| assignment.listens_at(ts)).unwrap_or(true)
    }

    fn retain_targets(&mut self, current_targets: &[u32]) {
        self.targets.retain(|issi| current_targets.binary_search(issi).is_ok());
        self.covered.retain(|issi| current_targets.binary_search(issi).is_ok());
        self.active_batch.retain(|issi| current_targets.binary_search(issi).is_ok());
    }

    fn begin_batch_if_needed(&mut self, ts: TdmaTime, energy_saving: &HashMap<u32, EnergySavingAssignment>) {
        if self.active_batch.is_empty() {
            self.active_batch = self.uncovered_listeners(ts, energy_saving);
        }
    }

    fn mark_batch_covered(&mut self) {
        for issi in self.active_batch.drain(..) {
            if !self.covered.contains(&issi) {
                self.covered.push(issi);
            }
        }
    }
}

/// Delivery coverage for GSSI-addressed FACCH/STCH stealing while members use
/// Energy Economy. Unlike SCH/F resources, the STCH block is already encoded,
/// so this state only tracks which affiliated ISSIs have had a listening
/// opportunity.
///
/// ETSI EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6 require the BS to account
/// for energy economy on downlink signalling. Clause 20.4.1.1.3 means the
/// retained TMA reporter should not complete until the GSSI FACCH transfer is
/// covered for all locally known listening batches.
#[derive(Debug, Clone)]
pub struct GroupStealingState {
    targets: Vec<u32>,
    covered: Vec<u32>,
    active_batch: Vec<u32>,
    tx_reporter: Option<TxReporter>,
    suspend_t210: bool,
}

impl GroupStealingState {
    fn new(targets: Vec<u32>, tx_reporter: Option<TxReporter>, suspend_t210: bool) -> Self {
        Self {
            targets,
            covered: Vec::new(),
            active_batch: Vec::new(),
            tx_reporter,
            suspend_t210,
        }
    }

    fn is_complete(&self) -> bool {
        self.targets.iter().all(|issi| self.covered.contains(issi))
    }

    fn uncovered_listeners(&self, ts: TdmaTime, energy_saving: &HashMap<u32, EnergySavingAssignment>) -> Vec<u32> {
        self.targets
            .iter()
            .copied()
            .filter(|issi| !self.covered.contains(issi))
            .filter(|issi| GroupDeliveryState::ms_listens_at(energy_saving, *issi, ts))
            .collect()
    }

    fn active_batch_listens(&self, ts: TdmaTime, energy_saving: &HashMap<u32, EnergySavingAssignment>) -> bool {
        self.active_batch
            .iter()
            .copied()
            .all(|issi| GroupDeliveryState::ms_listens_at(energy_saving, issi, ts))
    }

    fn retain_targets(&mut self, current_targets: &[u32]) {
        self.targets.retain(|issi| current_targets.binary_search(issi).is_ok());
        self.covered.retain(|issi| current_targets.binary_search(issi).is_ok());
        self.active_batch.retain(|issi| current_targets.binary_search(issi).is_ok());
    }

    fn begin_batch_if_needed(&mut self, ts: TdmaTime, energy_saving: &HashMap<u32, EnergySavingAssignment>) {
        if self.active_batch.is_empty() {
            self.active_batch = self.uncovered_listeners(ts, energy_saving);
        }
    }

    fn mark_batch_covered(&mut self) {
        for issi in self.active_batch.drain(..) {
            if !self.covered.contains(&issi) {
                self.covered.push(issi);
            }
        }
    }
}

const EMPTY_SCHED_ELEM: TimeslotSchedule = TimeslotSchedule {
    ul1: None,
    ul2: None,
    usage_marker: None,
    // dl: None,
};
const EMPTY_SCHED_CHANNEL: [TimeslotSchedule; MACSCHED_NUM_FRAMES] = [EMPTY_SCHED_ELEM; MACSCHED_NUM_FRAMES];
const EMPTY_SCHED: [[TimeslotSchedule; MACSCHED_NUM_FRAMES]; 4] = [EMPTY_SCHED_CHANNEL; 4];

impl BsChannelScheduler {
    pub fn new(scrambling_code: u32, precomps: PrecomputedUmacPdus) -> Self {
        BsChannelScheduler {
            cur_dltime: TdmaTime { t: 0, f: 0, m: 0, h: 0 }, // Intentionally invalid, updated in tick function
            scrambling_code,
            precomps,
            dltx_next_slot_queue: Vec::new(),
            dltx_queues: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            ulsched: EMPTY_SCHED,
            circuits: CircuitMgr::new(),
            hangtime: [false, false, false, false],
            pending_ra_acks: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            mcch_chan_alloc_sent_this_frame: false,
            // Start each timeslot's marker cursor at 4 (first valid value).
            next_usage_marker: [4, 4, 4, 4],
        }
    }

    fn frame18_can_carry_scheduled_schf(ts: TdmaTime) -> bool {
        ts.f == 18 && !ts.is_mandatory_bsch() && !ts.is_mandatory_bnch()
    }

    fn can_carry_scheduled_schf(ts: TdmaTime) -> bool {
        ts.f != 18 || Self::frame18_can_carry_scheduled_schf(ts)
    }

    /// Enter/leave hangtime for a traffic timeslot (2..=4).
    pub fn set_hangtime(&mut self, ts: u8, active: bool) {
        if !(1..=4).contains(&ts) {
            tracing::warn!("BsChannelScheduler::set_hangtime: invalid ts {}", ts);
            return;
        }

        let idx = ts as usize - 1;
        self.hangtime[idx] = active;

        // When leaving hangtime, drain stale signaling items that can only be consumed
        // in signaling mode. Keep Stealing items — they carry D-TX GRANTED/CEASED
        // that still need FACCH delivery.
        if !active {
            self.dl_drop_all_except_stolen(ts);
        }

        tracing::info!(
            "BsChannelScheduler: hangtime {} for ts {}",
            if active { "ENABLED" } else { "DISABLED" },
            ts,
        );
    }

    pub fn is_hangtime(&self, ts: u8) -> bool {
        // Defensive bounds check: ts must be 1..=4. Without this, a caller
        // accidentally passing ts=0 would underflow `ts as usize - 1` to
        // usize::MAX and panic on the array index. set_hangtime already has
        // this guard; mirror it here. Credit to proxiboi69 in
        // MidnightBlueLabs/tetra-bluestation PR #85.
        if !(1..=4).contains(&ts) {
            tracing::warn!("BsChannelScheduler::is_hangtime: invalid ts {}", ts);
            return false;
        }
        self.hangtime[ts as usize - 1]
    }

    pub fn clear_dl_media_queue(&mut self, ts: u8, reason: &str) {
        let dropped = self.circuits.clear_tx_data(ts);
        if dropped > 0 {
            tracing::info!(
                "BsChannelScheduler: dropped {} queued DL media block(s) on ts {}: {}",
                dropped,
                ts,
                reason
            );
        }
    }

    fn is_hangtime_effective(&self, ts: u8) -> bool {
        if !(1..=4).contains(&ts) {
            tracing::warn!("BsChannelScheduler::is_hangtime_effective: invalid ts {}", ts);
            return false;
        }
        let idx = ts as usize - 1;
        if !self.hangtime[idx] {
            return false;
        }
        // If a stealing block is still queued for this slot, keep traffic mode
        // so it can be delivered via FACCH.
        !self.has_pending_stealing(ts)
    }

    fn has_pending_stealing(&self, ts: u8) -> bool {
        let slot = ts as usize - 1;
        self.dltx_queues
            .get(slot)
            .map(|q| q.iter().any(|e| matches!(e, DlSchedElem::Stealing(..))))
            .unwrap_or(false)
    }

    fn generate_hangtime_idle_schf(&self) -> BitBuffer {
        // Full-slot SCH/F carrying a Null PDU (idle).
        let mut buf = BitBuffer::new(SCH_F_CAP);
        let pdu = MacResource::null_pdu();
        pdu.to_bitbuf(&mut buf);
        buf
    }

    // pub fn set_scrambling_code(&mut self, scrambling_code: u32) {
    //     self.scrambling_code = scrambling_code;
    //     unimplemented!("need to refresh some msgs possibly");
    // }

    // pub fn set_precomputed_msgs(&mut self, precomps: PrecomputedUmacPdus) {
    //     self.precomps = precomps;
    //     unimplemented!("need to refresh some msgs possibly");
    // }

    /// Update the System Wide Services flag in the broadcast SYSINFO.
    pub fn set_system_wide_services_state(&mut self, enabled: bool) {
        if self.precomps.mle_sysinfo.bs_service_details.system_wide_services != enabled {
            self.precomps.mle_sysinfo.bs_service_details.system_wide_services = enabled;
            // Should already be signalled at SwMI interface level
            tracing::debug!(
                "BsChannelScheduler: system_wide_services {}",
                if enabled { "ENABLED" } else { "DISABLED" }
            );
        }
    }

    /// Fully wipe the schedule
    pub fn purge_schedule(&mut self) {
        self.dltx_queues = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        self.ulsched = EMPTY_SCHED;
    }

    /// Sets the current downlink time to the given TdmaTime
    /// Wipes the schedule, as it can no longer be guaranteed to be valid
    pub fn set_dl_time(&mut self, new_ts: TdmaTime) {
        self.cur_dltime = new_ts;
        self.purge_schedule();
    }

    pub fn ul_ts_to_sched_index(&self, ts: &TdmaTime) -> usize {
        let to_index = (ts.f as usize - 1) + ((ts.m as usize - 1) * 18) + (ts.h as usize * 18 * 60);
        to_index % MACSCHED_NUM_FRAMES
    }

    ///////// UPLINK GRANT PROCESSING /////////

    /// Finds a grant opportunity for uplink transmission
    /// If num_slots is 1, is_halfslot may specifiy whether only a half slot is needed
    /// Returns (opportunities_to_skip, Vec<timestamps_of_granted_slots>)
    /// Returns None if no suitable opportunity is found in the schedule
    pub fn ul_find_grant_opportunity(&self, t: u8, num_slots: usize, is_halfslot: bool) -> Option<(usize, Vec<TdmaTime>)> {
        self.ul_find_grant_opportunity_from(self.cur_dltime, t, num_slots, is_halfslot)
    }

    pub fn ul_find_grant_opportunity_from(
        &self,
        base_dltime: TdmaTime,
        t: u8,
        num_slots: usize,
        is_halfslot: bool,
    ) -> Option<(usize, Vec<TdmaTime>)> {
        let first_opportunity = base_dltime.forward_to_timeslot(t);
        let mut grant_timeslots = Vec::with_capacity(num_slots);
        let mut opportunities_skipped = 0;

        assert!(!is_halfslot || num_slots == 1, "is_halfslot set for num_slots > 1");

        for dist in 0..MACSCHED_NUM_FRAMES - 1 {
            // let candidate_t = self.cur_ts.add_timeslots(dist as i32 * 4);
            // Base off of internal perception of time, convert to UL time
            // Below may crash someday, but I'd want to investigate that situation
            let candidate_t = first_opportunity.add_timeslots(dist as i32 * 4);
            assert!(
                candidate_t.t == first_opportunity.t,
                "ul_find_grant_opportunity: candidate_t.ts {} does not match requested ts {}. Please report this to developer. ",
                candidate_t.t,
                first_opportunity.t
            );

            tracing::debug!(
                "ul_find_grant_opportunity: considering candidate ul_ts {}, have {:?}",
                candidate_t,
                grant_timeslots
            );

            if candidate_t.is_mandatory_clch() {
                // EN 300 392-2 clause 23.5.2.2.2 counts frame-18 predefined
                // common-linearization opportunities in the granting delay,
                // while clauses 23.5.2.2.1 and 23.5.2.2.7 require reserved
                // access to jump over only those frame-18 slots.
                if grant_timeslots.is_empty() {
                    opportunities_skipped += 1;
                }
                continue;
            }

            let index = self.ul_ts_to_sched_index(&candidate_t);
            let elem = &self.ulsched[t as usize - 1][index];
            // tracing::debug!("ul_find_grant_opportunity: sched[{}] ts {}: {:?}", index, candidate_t, elem);
            if (elem.ul1.is_none() && elem.ul2.is_none()) || (is_halfslot && (elem.ul1.is_none() || elem.ul2.is_none())) {
                // Free UL slot, add this timeslot to result vec
                grant_timeslots.push(candidate_t);
                // continue;
            } else {
                // Something is here, clear our grant timeslots
                opportunities_skipped += grant_timeslots.len() + 1;
                grant_timeslots.clear();
            }

            // Check if done
            if grant_timeslots.len() == num_slots {
                return Some((opportunities_skipped, grant_timeslots));
            }
        }

        // If we get here, we did not find a suitable grant opportunity
        None
    }

    /// Reserves all slots designated in a grant option
    /// If only one halfslot is needed, returns 1 or 2 designating which slot was reserved
    pub fn ul_reserve_grant(&mut self, ssi: u32, grant_timestamps: Vec<TdmaTime>, is_halfslot: bool, usage_marker: Option<u8>) -> u8 {
        assert!(!grant_timestamps.is_empty());
        assert!(!is_halfslot || grant_timestamps.len() == 1);
        // let ts = grant_timestamps[0].t as usize;
        for ts in grant_timestamps {
            let index = self.ul_ts_to_sched_index(&ts);

            let elem: &mut TimeslotSchedule = &mut self.ulsched[ts.t as usize - 1][index];
            // Stamp the usage marker on the slot. AACH generation for this
            // slot will then emit Traffic(marker) per ETSI §23.5.2, which tells
            // the MS that holds the reservation it can transmit here.
            if let Some(m) = usage_marker {
                elem.usage_marker = Some(m);
            }
            if is_halfslot {
                if elem.ul1.is_none() {
                    elem.ul1 = Some(ssi);
                    return 1;
                } else {
                    assert!(elem.ul2.is_none(), "ul_reserve_grant: ul2 already set for ts {:?}, ssi {}", ts, ssi);
                    elem.ul2 = Some(ssi);
                    return 2;
                }
            } else {
                assert!(elem.ul1.is_none(), "ul_reserve_grant: ul1 already set for ts {:?}, ssi {}", ts, ssi);
                assert!(elem.ul2.is_none(), "ul_reserve_grant: ul2 already set for ts {:?}, ssi {}", ts, ssi);
                elem.ul1 = Some(ssi);
                elem.ul2 = Some(ssi);
            }
        }

        // Full slots reserved
        0
    }

    /// Tries to find a way to satisfy a granting request, and reserves the slots in the schedule.
    /// On success returns a `BasicSlotgrant` plus an optional `usage_marker`. The marker is
    /// `Some(m)` only when the grant covers more than one slot — single-slot grants don't need
    /// one. The marker is stored on each reserved `TimeslotSchedule` entry so AACH generation
    /// for those slots emits `Traffic(m)` and the MS can identify its reservation.
    pub fn ul_process_cap_req(
        &mut self,
        timeslot: u8,
        addr: TetraAddress,
        res_req: &ReservationRequirement,
    ) -> Option<(BasicSlotgrant, Option<u8>)> {
        self.ul_process_cap_req_from(self.cur_dltime, timeslot, addr, res_req)
    }

    pub fn ul_process_cap_req_from(
        &mut self,
        base_dltime: TdmaTime,
        timeslot: u8,
        addr: TetraAddress,
        res_req: &ReservationRequirement,
    ) -> Option<(BasicSlotgrant, Option<u8>)> {
        let is_halfslot = res_req == &ReservationRequirement::Req1Subslot;
        let requested_cap = if is_halfslot { 1 } else { res_req.to_req_slotcount() };

        // Find a suitable grant opportunity
        let grant_op = self.ul_find_grant_opportunity_from(base_dltime, timeslot, requested_cap, is_halfslot);

        tracing::debug!(
            "ul_process_cap_req: addr {}, res_req {:?}, requested_cap {}, is_halfslot {}, grant_op: {:?}",
            addr,
            res_req,
            requested_cap,
            is_halfslot,
            grant_op
        );

        // If found, reserve the slots and return a BasicSlotgrant + optional usage_marker.
        if let Some((skips, grant_timestamps)) = grant_op {
            if skips > 13 {
                // EN 300 392-2 clause 21.5.6 defines delay opportunities only
                // for raw values 1..=13. Raw 14/15 are special meanings, so do
                // not reserve capacity that cannot be represented as a valid
                // basic slot-grant delay.
                tracing::warn!(
                    "ul_process_cap_req: grant opportunity for addr {} res_req {:?} needs {} delay opportunities",
                    addr,
                    res_req,
                    skips
                );
                return None;
            }

            // For multi-slot full grants, allocate a usage marker. We do this
            // BEFORE reserving so the marker can be embedded in the schedule.
            // Single-slot or half-slot grants don't need a marker — the MS
            // either has nothing to fragment (subslot) or completes the burst
            // in the one slot (single full slot).
            let usage_marker = if !is_halfslot && requested_cap >= 2 {
                Some(self.alloc_usage_marker(timeslot))
            } else {
                None
            };

            // Reserve the target granting opportunity. Get subslot (only relevant for halfslot reservation)
            let subslot = self.ul_reserve_grant(addr.ssi, grant_timestamps, is_halfslot, usage_marker);

            // tracing::info!("After grant:")
            // self.dump_ul_schedule_full(false);

            // Build BasicSlotgrant response element
            let cap_alloc = if res_req == &ReservationRequirement::Req1Subslot {
                match subslot {
                    1 => BasicSlotgrantCapAlloc::FirstSubslotGranted,
                    2 => BasicSlotgrantCapAlloc::SecondSubslotGranted,
                    _ => unreachable!("ul_process_cap_req: subslot must be 1 or 2, got {}", subslot),
                }
            } else {
                BasicSlotgrantCapAlloc::from_req_slotcount(requested_cap)
            };
            let grant_delay = if skips == 0 {
                BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity
            } else {
                BasicSlotgrantGrantingDelay::DelayNOpportunities(skips as u8)
            };
            Some((
                BasicSlotgrant {
                    capacity_allocation: cap_alloc,
                    granting_delay: grant_delay,
                },
                usage_marker,
            ))
        } else {
            tracing::warn!(
                "ul_process_cap_req: no suitable grant opportunity found for addr {}, res_req {:?}",
                addr,
                res_req
            );
            None
        }
    }

    /// Returns schedule info for the given uplink timeslot and full-or-subslot
    /// If Both is requested, schedule is assumed to have matching allocation for two subslots
    /// If not, a warning is issued and None is returned.
    pub fn ul_get_slot_owner(&self, ts: TdmaTime, slot: PhyBlockNum) -> Option<u32> {
        let sched = &self.ulsched[ts.t as usize - 1][self.ul_ts_to_sched_index(&ts)];
        match slot {
            PhyBlockNum::Block1 => sched.ul1,
            PhyBlockNum::Block2 => sched.ul2,
            PhyBlockNum::Both => {
                if sched.ul1 != sched.ul2 {
                    tracing::warn!("ul_get_slot_owner: requested Both but ul1 {:?} != ul2 {:?}", sched.ul1, sched.ul2);
                    return None;
                }
                sched.ul1
            }
            _ => unreachable!(),
        }
    }

    fn ul_get_usage(&self, ts: TdmaTime) -> AccessAssignUlUsage {
        let ul_sched = &self.ulsched[ts.t as usize - 1][self.ul_ts_to_sched_index(&ts)];
        match (ul_sched.ul1, ul_sched.ul2) {
            // A reserved slot with a usage_marker gets `Traffic(marker)` so the
            // MS that holds the reservation can identify its slot from AACH and
            // continue a fragmented uplink burst (MacFragUl → MacEndUl). Without
            // a marker, the MS abandons the burst after one slot — see the
            // comment on `next_usage_marker` for the failure mode this fixes.
            (Some(_), Some(_)) => {
                if let Some(marker) = ul_sched.usage_marker {
                    AccessAssignUlUsage::Traffic(marker)
                } else {
                    AccessAssignUlUsage::AssignedOnly
                }
            }
            (Some(_), None) => {
                if let Some(marker) = ul_sched.usage_marker {
                    AccessAssignUlUsage::Traffic(marker)
                } else {
                    AccessAssignUlUsage::CommonAndAssigned
                }
            }
            (None, None) => AccessAssignUlUsage::CommonOnly,
            _ => unreachable!("ul2 can't be set with ul1 None"),
        }
    }

    /// Allocate a fresh usage marker for a multi-slot reservation in `timeslot`.
    /// The marker is taken from the per-timeslot rotating cursor in the valid
    /// range [4, 62]. ETSI reserves 0 (Unallocated), 1-3, and 63 (Common
    /// linearisation), so we skip those.
    ///
    /// We don't track outstanding markers — the cursor just wraps. With only
    /// a handful of in-flight reservations per timeslot at any moment and 59
    /// valid markers to choose from, accidental reuse is improbable, and even
    /// if it happens the consequence is benign (the other MS would see its
    /// marker re-issued in a different slot and re-attempt).
    fn alloc_usage_marker(&mut self, timeslot: u8) -> u8 {
        let idx = (timeslot as usize - 1).min(3);
        let marker = self.next_usage_marker[idx];
        // Advance cursor, wrapping in [4, 62].
        let next = if marker >= 62 { 4 } else { marker + 1 };
        self.next_usage_marker[idx] = next;
        marker
    }

    ////////// DOWNLINK SCHEDULING /////////

    /// Registers that we should transmit a MAC-RESOURCE or similar with a grant, somewhere this tick.
    /// `usage_marker` is set when the grant covers >1 slot — the MS uses it to identify the reservation
    /// when continuing the burst on the second slot (per ETSI §21.4.3.2). Single-slot grants pass None.
    pub fn dl_enqueue_grant(&mut self, ts: u8, addr: TetraAddress, grant: BasicSlotgrant, usage_marker: Option<u8>) {
        tracing::debug!(
            "dl_enqueue_grant: ts {} enqueueing PDU {:?} for addr {} marker {:?}",
            ts,
            grant,
            addr,
            usage_marker
        );
        let elem = DlSchedElem::Grant(addr, grant, usage_marker);
        self.dltx_queues[ts as usize - 1].push(elem);
    }

    pub fn dl_enqueue_reservation_grant(&mut self, ts: u8, addr: TetraAddress, res_req: ReservationRequirement) {
        tracing::debug!(
            "dl_enqueue_reservation_grant: ts {} enqueueing reservation {:?} for addr {}",
            ts,
            res_req,
            addr
        );
        let elem = DlSchedElem::PendingGrant(addr, res_req);
        self.dltx_queues[ts as usize - 1].push(elem);
    }

    pub fn dl_enqueue_random_access_ack(&mut self, ts: u8, addr: TetraAddress) {
        tracing::debug!(
            "dl_enqueue_random_access_ack: ts {} enqueueing random access acknowledgementfor addr {}",
            ts,
            addr
        );
        let elem = DlSchedElem::RandomAccessAck(addr);
        self.dltx_queues[ts as usize - 1].push(elem);
    }

    fn common_control_downlink_timeslots() -> [u8; NUM_TIMESLOTS] {
        // EN 300 392-2 clauses 21.4.6.5 and 23.5.2.2.7: this normal
        // TMA path emits MCCH/SCH/F common-control signalling on the main
        // control-channel timeslot used by this single-carrier scheduler.
        // Assigned-channel signalling on traffic timeslots is handled by
        // `dl_enqueue_stealing`, so do not pretend to discover per-SSI traffic
        // slots here.
        [1, 0, 0, 0]
    }

    pub fn dl_enqueue_tma(&mut self, pdu: MacResource, sdu: BitBuffer, tx_reporter: Option<TxReporter>) {
        let timeslots = Self::common_control_downlink_timeslots();

        // Queue the message for all timeslots on which we should transmit this message.
        // The loop basically prevents cloning the last element.
        for i in 0..NUM_TIMESLOTS {
            let ts = timeslots[i];
            let next_ts = if i < NUM_TIMESLOTS - 1 { timeslots[i + 1] } else { 0 };
            assert!(ts > 0);

            // If this PDU carries a chan_alloc element (DConnect/DConnectAck MCCH), check if we
            // already sent one this frame. DConnect MCCH (113 bits) + DConnectAck MCCH (110 bits)
            // = 223 bits > 216-bit slot capacity. Defer the second one to the next frame.
            let deferred = if pdu.chan_alloc_element.is_some() {
                if self.mcch_chan_alloc_sent_this_frame {
                    true // Defer this one to next frame
                } else {
                    self.mcch_chan_alloc_sent_this_frame = true;
                    false // First one goes normally
                }
            } else {
                false
            };

            tracing::debug!(
                "dl_enqueue_tma: ts {}{} enqueueing PDU {:?} SDU {}",
                ts,
                if tx_reporter.is_some() { " reported" } else { "" },
                pdu,
                sdu.dump_bin(),
            );

            if deferred {
                tracing::debug!("dl_enqueue_tma: ts {} deferring chan_alloc PDU to next frame (slot capacity)", ts);
                let elem = DlSchedElem::Resource(pdu, sdu, tx_reporter, None);
                self.dltx_next_slot_queue.push(elem);
                break;
            } else if next_ts > 0 {
                // There is another ts for which we need to transmit this message.
                // Clone the message now and push it to the current ts.
                let elem = DlSchedElem::Resource(pdu.clone(), sdu.clone(), tx_reporter.clone(), None);
                self.dltx_queues[ts as usize - 1].push(elem);
            } else {
                // This is the last ts on which we need to transmit this message
                let elem = DlSchedElem::Resource(pdu, sdu, tx_reporter, None);
                self.dltx_queues[ts as usize - 1].push(elem);
                break;
            }
        }
    }

    /// Consumes and returns true if a pending random access ack exists for the given address on
    /// this timeslot. Used when building STCH blocks so the MAC-RESOURCE can carry
    /// random_access_flag=true per ETSI 21.4.3.1.
    pub fn take_pending_ra_ack(&mut self, ts: u8, addr: TetraAddress) -> bool {
        let pending = &mut self.pending_ra_acks[ts as usize - 1];
        if let Some(pos) = pending
            .iter()
            .position(|pending_addr| pending_addr.ssi == addr.ssi && pending_addr.ssi_type == addr.ssi_type)
        {
            pending.remove(pos);
            true
        } else {
            false
        }
    }

    /// Consumes a hangtime-preserved random-access acknowledgement only when the
    /// STCH also carries a channel allocation. In a simplex private-call floor
    /// change, LLC can enqueue a short BL-ACK before CMCE's D-TX GRANTED. EN 300
    /// 392-2 clause 21.4.3.1 defines the random access flag as the BS
    /// acknowledgement of successful random access, while clauses 14.5.1.2.1 b)
    /// and 23.5.2.2.1 make the following channel-allocation D-TX GRANTED the
    /// response that lets the requesting MS enter the assigned-channel U-plane.
    /// Keep the preserved ACK for that response instead of spending it on an
    /// ACK-only STCH.
    pub fn take_pending_ra_ack_for_stch(&mut self, ts: u8, addr: TetraAddress, carries_channel_allocation: bool) -> bool {
        if !carries_channel_allocation {
            if self.pending_ra_acks[ts as usize - 1]
                .iter()
                .any(|pending_addr| pending_addr.ssi == addr.ssi && pending_addr.ssi_type == addr.ssi_type)
            {
                tracing::debug!(
                    "take_pending_ra_ack_for_stch: preserving pending RA ACK for {} on ts {} until channel-allocation STCH",
                    addr,
                    ts
                );
            }
            return false;
        }
        self.take_pending_ra_ack(ts, addr)
    }

    /// Enqueue a pre-built STCH block for FACCH/stealing on a traffic timeslot.
    /// The block must be 124 type1 bits containing MAC-U-SIGNAL header + TM-SDU.
    pub fn dl_enqueue_stealing(&mut self, ts: u8, block: BitBuffer, addr: TetraAddress, tx_reporter: Option<TxReporter>) {
        tracing::info!("dl_enqueue_stealing: ts {} enqueueing STCH block ({} bits)", ts, block.get_len());
        self.dltx_queues[ts as usize - 1].push(DlSchedElem::Stealing(block, addr, tx_reporter, None));
    }

    fn dl_requeue_group_stealing(&mut self, ts: u8, block: BitBuffer, addr: TetraAddress, group_state: GroupStealingState) {
        tracing::debug!(
            "dl_requeue_group_stealing: GSSI {} covered {}/{} on ts {}",
            addr.ssi,
            group_state.covered.len(),
            group_state.targets.len(),
            ts
        );
        self.dltx_queues[ts as usize - 1].push(DlSchedElem::Stealing(
            block,
            addr,
            group_state.tx_reporter.clone(),
            Some(group_state),
        ));
    }

    fn dl_enqueue_tma_frag_next_frame_with_group_state(&mut self, fragger: BsFragger, group_state: Option<GroupDeliveryState>) {
        tracing::debug!("dl_enqueue_tma_frag_next_frame: enqueueing {:?}", fragger);
        let elem = DlSchedElem::FragBuf(fragger, group_state);
        self.dltx_next_slot_queue.push(elem);
    }

    fn dl_enqueue_group_repeat_next_frame(&mut self, group_state: GroupDeliveryState) {
        tracing::debug!(
            "dl_enqueue_group_repeat_next_frame: GSSI {:?} covered {}/{}",
            group_state.original_pdu.addr,
            group_state.covered.len(),
            group_state.targets.len()
        );
        let elem = DlSchedElem::Resource(
            group_state.original_pdu.clone(),
            group_state.original_sdu.clone(),
            None,
            Some(group_state),
        );
        self.dltx_next_slot_queue.push(elem);
    }

    fn dl_defer_pending_grant_next_frame(&mut self, addr: TetraAddress, res_req: ReservationRequirement) {
        tracing::debug!(
            "dl_defer_pending_grant_next_frame: requeueing reservation {:?} for addr {}",
            res_req,
            addr
        );
        self.dltx_next_slot_queue.push(DlSchedElem::PendingGrant(addr, res_req));
    }

    fn elem_addr(elem: &DlSchedElem) -> Option<TetraAddress> {
        match elem {
            DlSchedElem::RandomAccessAck(addr) | DlSchedElem::Grant(addr, _, _) | DlSchedElem::PendingGrant(addr, _) => Some(*addr),
            DlSchedElem::Resource(pdu, _, _, _) => pdu.addr,
            DlSchedElem::FragBuf(fragger, _) => fragger.addr(),
            DlSchedElem::Stealing(_, addr, _, _) => Some(*addr),
            _ => None,
        }
    }

    fn group_targets(addr: TetraAddress, subscribers: &SubscriberRegistry) -> Vec<u32> {
        if addr.ssi_type != SsiType::Gssi {
            return Vec::new();
        }

        let mut targets: Vec<u32> = if addr.ssi == PREDEFINED_BROADCAST_GSSI {
            subscribers.all_registered_issis().collect()
        } else {
            subscribers.group_members(addr.ssi)
        };
        targets.sort_unstable();
        targets.dedup();
        targets
    }

    fn retain_current_group_delivery_targets(state: &mut GroupDeliveryState, addr: TetraAddress, subscribers: &SubscriberRegistry) {
        // EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6 require delivery to match
        // current EG listening opportunities. If MM removes a registration or
        // group affiliation while a repeated GSSI transfer is pending, stale
        // snapshot targets are no longer valid local addresses.
        let current_targets = Self::group_targets(addr, subscribers);
        state.retain_targets(&current_targets);
    }

    fn retain_current_group_stealing_targets(state: &mut GroupStealingState, addr: TetraAddress, subscribers: &SubscriberRegistry) {
        // Same current-address pruning as MAC-RESOURCE delivery, applied to
        // FACCH/STCH repeats whose block is already encoded.
        let current_targets = Self::group_targets(addr, subscribers);
        state.retain_targets(&current_targets);
    }

    fn prune_completed_stale_group_states_for_slot(&mut self, slot: usize, subscribers: &SubscriberRegistry) {
        let Some(queue) = self.dltx_queues.get_mut(slot) else {
            return;
        };

        queue.retain_mut(|elem| {
            let completed_reporter = match elem {
                DlSchedElem::Resource(pdu, _, _, Some(state)) => pdu.addr.and_then(|addr| {
                    Self::retain_current_group_delivery_targets(state, addr, subscribers);
                    state.is_complete().then(|| state.tx_reporter.clone()).flatten()
                }),
                DlSchedElem::FragBuf(fragger, Some(state)) => fragger.addr().and_then(|addr| {
                    Self::retain_current_group_delivery_targets(state, addr, subscribers);
                    state.is_complete().then(|| state.tx_reporter.clone()).flatten()
                }),
                DlSchedElem::Stealing(_, addr, _, Some(state)) => {
                    Self::retain_current_group_stealing_targets(state, *addr, subscribers);
                    state.is_complete().then(|| state.tx_reporter.clone()).flatten()
                }
                _ => None,
            };

            if let Some(reporter) = completed_reporter {
                reporter.mark_transmitted();
                return false;
            }

            true
        });
    }

    fn ms_listens_at(energy_saving: &HashMap<u32, EnergySavingAssignment>, issi: u32, ts: TdmaTime) -> bool {
        energy_saving.get(&issi).map(|assignment| assignment.listens_at(ts)).unwrap_or(true)
    }

    fn group_state_for_resource(
        addr: TetraAddress,
        pdu: &MacResource,
        sdu: &BitBuffer,
        tx_reporter: Option<TxReporter>,
        subscribers: &SubscriberRegistry,
    ) -> Option<GroupDeliveryState> {
        if addr.ssi_type != SsiType::Gssi {
            return None;
        }

        let targets = Self::group_targets(addr, subscribers);
        if targets.is_empty() {
            return None;
        }

        Some(GroupDeliveryState::new(
            pdu.clone(),
            sdu.clone(),
            targets,
            tx_reporter,
            addr.ssi != PREDEFINED_BROADCAST_GSSI,
        ))
    }

    fn group_state_ready_for_tx(
        state: Option<&GroupDeliveryState>,
        addr: TetraAddress,
        ts: TdmaTime,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
        subscribers: &SubscriberRegistry,
    ) -> bool {
        if let Some(state) = state {
            if !state.active_batch.is_empty() {
                return state.active_batch_listens(ts, energy_saving);
            }
            return !state.uncovered_listeners(ts, energy_saving).is_empty();
        }

        let targets = Self::group_targets(addr, subscribers);
        if targets.is_empty() {
            return true;
        }

        targets.iter().copied().any(|issi| Self::ms_listens_at(energy_saving, issi, ts))
    }

    fn group_stealing_state_ready_for_tx(
        state: Option<&GroupStealingState>,
        addr: TetraAddress,
        ts: TdmaTime,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
        subscribers: &SubscriberRegistry,
    ) -> bool {
        if let Some(state) = state {
            if !state.active_batch.is_empty() {
                return state.active_batch_listens(ts, energy_saving);
            }
            return !state.uncovered_listeners(ts, energy_saving).is_empty();
        }

        let targets = Self::group_targets(addr, subscribers);
        if targets.is_empty() {
            return true;
        }

        targets.iter().copied().any(|issi| Self::ms_listens_at(energy_saving, issi, ts))
    }

    fn elem_is_ready_for_tx(
        elem: &DlSchedElem,
        ts: TdmaTime,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
        subscribers: &SubscriberRegistry,
    ) -> bool {
        let Some(addr) = Self::elem_addr(elem) else {
            return true;
        };

        match addr.ssi_type {
            SsiType::Issi => Self::ms_listens_at(energy_saving, addr.ssi, ts),
            SsiType::Gssi => match elem {
                DlSchedElem::Resource(_, _, _, group_state) | DlSchedElem::FragBuf(_, group_state) => {
                    Self::group_state_ready_for_tx(group_state.as_ref(), addr, ts, energy_saving, subscribers)
                }
                DlSchedElem::Stealing(_, _, _, group_state) => {
                    Self::group_stealing_state_ready_for_tx(group_state.as_ref(), addr, ts, energy_saving, subscribers)
                }
                _ => Self::group_state_ready_for_tx(None, addr, ts, energy_saving, subscribers),
            },
            _ => true,
        }
    }

    fn elem_has_integrated_grant_or_ack(elem: &DlSchedElem) -> bool {
        matches!(
            elem,
            DlSchedElem::Resource(pdu, _, _, _) if pdu.slot_granting_element.is_some() || pdu.random_access_flag
        )
    }

    fn elem_has_channel_allocation(elem: &DlSchedElem) -> bool {
        match elem {
            DlSchedElem::Resource(pdu, _, _, _) => pdu.chan_alloc_element.is_some(),
            DlSchedElem::FragBuf(fragger, _) => fragger.carries_channel_allocation(),
            _ => false,
        }
    }

    fn mark_addr_signalling_activity(addr: Option<TetraAddress>, ts: TdmaTime, energy_saving: &mut HashMap<u32, EnergySavingAssignment>) {
        let Some(addr) = addr else {
            return;
        };
        if addr.ssi_type != SsiType::Issi {
            return;
        }
        if let Some(assignment) = energy_saving.get_mut(&addr.ssi) {
            assignment.mark_awake_from_signalling_activity(ts);
        }
    }

    fn mark_group_signalling_activity(
        group_state: Option<&GroupDeliveryState>,
        ts: TdmaTime,
        energy_saving: &mut HashMap<u32, EnergySavingAssignment>,
    ) {
        let Some(group_state) = group_state else {
            return;
        };
        if !group_state.suspend_t210 {
            return;
        }
        for &issi in &group_state.active_batch {
            if let Some(assignment) = energy_saving.get_mut(&issi) {
                assignment.mark_awake_from_signalling_activity(ts);
            }
        }
    }

    fn mark_stealing_signalling_activity(
        addr: TetraAddress,
        group_state: Option<&GroupStealingState>,
        ts: TdmaTime,
        subscribers: &SubscriberRegistry,
        energy_saving: &mut HashMap<u32, EnergySavingAssignment>,
    ) {
        if let Some(group_state) = group_state {
            if group_state.suspend_t210 {
                for &issi in &group_state.active_batch {
                    if let Some(assignment) = energy_saving.get_mut(&issi) {
                        assignment.mark_awake_from_signalling_activity(ts);
                    }
                }
            }
            return;
        }

        match addr.ssi_type {
            SsiType::Issi => Self::mark_addr_signalling_activity(Some(addr), ts, energy_saving),
            SsiType::Gssi if addr.ssi != PREDEFINED_BROADCAST_GSSI => {
                // EN 300 392-2 clause 23.7.6 temporarily suspends sleep after
                // TMA-SAP signalling for a valid address, excluding only the
                // predefined all-ones broadcast group address. For GSSI
                // stealing, extend T.210 only for affiliates that were
                // actually listening at this TDMA instant; one awake group
                // member must not manufacture activity for sleeping EG peers.
                for issi in Self::group_targets(addr, subscribers) {
                    let listens_at_tx = energy_saving.get(&issi).map(|assignment| assignment.listens_at(ts)).unwrap_or(true);
                    if !listens_at_tx {
                        continue;
                    }
                    if let Some(assignment) = energy_saving.get_mut(&issi) {
                        assignment.mark_awake_from_signalling_activity(ts);
                    }
                }
            }
            _ => {}
        }
    }

    fn mark_reporter_discarded_if_pending(tx_reporter: Option<TxReporter>) {
        if let Some(tx_reporter) = tx_reporter
            && tx_reporter.get_state() == tetra_core::TxState::Pending
        {
            tx_reporter.mark_discarded();
        }
    }

    fn reporter_matches(tx_reporter: Option<&TxReporter>, target: &TxReporter) -> bool {
        tx_reporter.is_some_and(|tx_reporter| tx_reporter.shares_state_with(target))
    }

    fn group_state_matches_reporter(group_state: Option<&GroupDeliveryState>, target: &TxReporter) -> bool {
        group_state
            .and_then(|state| state.tx_reporter.as_ref())
            .is_some_and(|tx_reporter| tx_reporter.shares_state_with(target))
    }

    fn elem_matches_reporter(elem: &DlSchedElem, target: &TxReporter) -> bool {
        match elem {
            DlSchedElem::Resource(_, _, tx_reporter, group_state) => {
                Self::reporter_matches(tx_reporter.as_ref(), target) || Self::group_state_matches_reporter(group_state.as_ref(), target)
            }
            DlSchedElem::FragBuf(fragger, group_state) => {
                fragger.tx_reporter_shares_state_with(target) || Self::group_state_matches_reporter(group_state.as_ref(), target)
            }
            DlSchedElem::Stealing(_, _, tx_reporter, group_state) => {
                Self::reporter_matches(tx_reporter.as_ref(), target)
                    || group_state
                        .as_ref()
                        .and_then(|state| state.tx_reporter.as_ref())
                        .is_some_and(|tx_reporter| tx_reporter.shares_state_with(target))
            }
            _ => false,
        }
    }

    fn dl_cancel_by_reporter_in_queue(queue: &mut Vec<DlSchedElem>, target: &TxReporter) -> usize {
        let before = queue.len();
        queue.retain(|elem| !Self::elem_matches_reporter(elem, target));
        before - queue.len()
    }

    /// Cancel queued TMA-SAP signalling that has not yet been transmitted.
    ///
    /// EN 300 392-2 clause 20.4.1.1.1 defines TMA-CANCEL as cancellation of a
    /// submitted TMA-UNITDATA request. Matching by the retained TxReporter
    /// clone removes only the specific request associated with LLC's handle.
    pub fn dl_cancel_by_reporter(&mut self, target: &TxReporter) -> usize {
        let mut removed = Self::dl_cancel_by_reporter_in_queue(&mut self.dltx_next_slot_queue, target);
        for queue in &mut self.dltx_queues {
            removed += Self::dl_cancel_by_reporter_in_queue(queue, target);
        }
        if removed > 0 {
            Self::mark_reporter_discarded_if_pending(Some(target.clone()));
        }
        removed
    }

    /// Enqueue a TMA PDU to be transmitted on the NEXT frame (ts1, frame N+1).
    /// Use this to deliberately separate two MCCH messages that would overflow the slot
    /// if sent together (e.g. DConnect MCCH + DConnectAck MCCH = 223 bits > 216-bit slot).
    pub fn dl_enqueue_tma_next_frame(&mut self, pdu: MacResource, sdu: BitBuffer, tx_reporter: Option<TxReporter>) {
        tracing::debug!(
            "dl_enqueue_tma_next_frame: deferring PDU {:?} SDU {} to next frame",
            pdu,
            sdu.dump_bin()
        );
        let elem = DlSchedElem::Resource(pdu, sdu, tx_reporter, None);
        self.dltx_next_slot_queue.push(elem);
    }

    pub fn dl_schedule_tmb(&mut self, traffic: BitBuffer, ts: &TdmaTime) {
        // EN 300 392-2 clauses 20.56 and 23 define TMB-SAP as the MAC path
        // for unaddressed system broadcast messages. The current BS sends
        // D-NWRK-BROADCAST through the modelled TLA/TMA path; raw TLMB/TMB
        // scheduling is not yet modelled, so fail closed instead of panicking.
        tracing::error!(
            "TMB-SAP broadcast scheduling is not implemented; dropping {} bits requested for {:?}",
            traffic.get_len(),
            ts
        );
    }

    // pub fn dl_schedule_tmd(&mut self, _traffic: BitBuffer, _ts: &TdmaTime) {
    //     unimplemented!("Traffic scheduling not implemented yet");
    // }

    pub fn dl_schedule_tmd(&mut self, ts: u8, block: Vec<u8>) {
        self.circuits.put_block(ts, block);
    }

    pub fn dl_schedule_raw_tch_s_half_slot(&mut self, ts: u8, block_num: PhyBlockNum, type5_bits: Vec<u8>) {
        self.circuits.put_raw_tch_s_half_slot(ts, block_num, type5_bits);
    }

    pub fn circuit_is_active(&self, dir: Direction, ts: u8) -> bool {
        self.circuits.is_active(dir, ts)
    }

    pub fn circuit_is_active_for_addr(&self, dir: Direction, ts: u8, addr: TetraAddress) -> bool {
        if !(1..=4).contains(&ts) {
            return false;
        }

        let circuit = match dir {
            Direction::Dl => &self.circuits.dl[ts as usize - 1],
            Direction::Ul => &self.circuits.ul[ts as usize - 1],
            _ => {
                tracing::error!("UMAC CircuitMgr: called with non-specific direction {:?}", dir);
                return false;
            }
        };

        circuit.as_ref().is_some_and(|circuit| circuit.is_active_for_addr(addr))
    }

    pub fn ul_circuit_has_issi_participants(&self, ts: u8) -> bool {
        if !(1..=4).contains(&ts) {
            return false;
        }

        self.circuits.ul[ts as usize - 1]
            .as_ref()
            .is_some_and(|circuit| circuit.active_addresses().any(|addr| addr.ssi_type == SsiType::Issi))
    }

    /// Return the peer timeslot for the UL circuit on `ts`, if any.
    /// Used for full-duplex P2P cross-routing: UL voice on `ts` must be played out
    /// on the peer MS's DL timeslot. Returns `None` for simplex/group calls
    /// (where UL→DL stays on the same timeslot, classic loopback).
    pub fn ul_circuit_peer_ts(&self, ts: u8) -> Option<u8> {
        if !(1..=4).contains(&ts) {
            return None;
        }
        self.circuits.ul[ts as usize - 1].as_ref().and_then(|c| c.peer_ts)
    }

    /// Return the DL media source policy for the UL circuit on `ts`.
    /// `LocalLoopback` = reflect UL back to DL (group/simplex calls).
    /// `SwMI` = DL audio comes from Brew/TetraPack; suppress local loopback.
    pub fn ul_circuit_dl_media_source(&self, ts: u8) -> CircuitDlMediaSource {
        if !(1..=4).contains(&ts) {
            return CircuitDlMediaSource::LocalLoopback;
        }
        self.circuits.ul[ts as usize - 1]
            .as_ref()
            .map(|c| c.dl_media_source)
            .unwrap_or(CircuitDlMediaSource::LocalLoopback)
    }

    pub fn close_circuit(&mut self, dir: Direction, ts: u8) -> Option<Circuit> {
        // Clearing hangtime here is safe: if the circuit is gone, this timeslot is no longer in use.
        if (1..=4).contains(&ts) {
            self.hangtime[ts as usize - 1] = false;
        }
        self.circuits.close_circuit(dir, ts)
    }

    pub fn create_circuit(&mut self, dir: Direction, circuit: Circuit) {
        if !(1..=4).contains(&circuit.ts) {
            tracing::error!(
                "BsChannelScheduler::create_circuit: rejecting invalid traffic timeslot {} for {:?}",
                circuit.ts,
                dir
            );
            return;
        }
        if circuit.ts == 1 {
            // This BS scheduler uses TS1 as the MCCH/SCH-F common-control
            // carrier. EN 300 392-2 clause 23.5.2.2.7 still allows reserved
            // UL access on TS1 via ACCESS-ASSIGN, but assigned-channel voice
            // traffic is modelled only on TS2..TS4. Rejecting here keeps a bad
            // CMCE/UMAC allocation from becoming a process-killing AACH assert.
            tracing::error!("BsChannelScheduler::create_circuit: rejecting traffic circuit on TS1 common control");
            return;
        }

        // New/updated circuit implies traffic mode.
        self.hangtime[circuit.ts as usize - 1] = false;
        self.circuits.create_circuit(dir, circuit);
    }

    /// Takes a block or None value.
    /// If block is present and some signalling channel, and space is available,
    /// adds a trailing Null PDU.
    /// If blk is None, returns None.
    /// Otherwise, returns blk unchanged (eg. for SYNC, broadcast, etc).
    pub fn try_add_null_pdus(&mut self, blk: Option<TmvUnitdataReq>) -> Option<TmvUnitdataReq> {
        // A null pdu in a slot:
        // 0000000000010000100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000
        // Oddly, the fill_bits ind is set to 0, while a fill bit is indeed present to fill the slot.
        // We replicate that behavior here.
        if let Some(mut b) = blk {
            // STCH: MAC-U-SIGNAL occupies entire half-slot (3-bit header + 121-bit TM-SDU).
            // No additional MAC PDUs may be concatenated; receiver passes all bits after header to LLC.
            // Adding a null PDU would corrupt TM-SDU (misinterpreted as optional CMCE element flags).
            if b.logical_channel == LogicalChannel::SchHd || b.logical_channel == LogicalChannel::SchF {
                if b.mac_block.get_len_remaining() >= NULL_PDU_LEN_BITS {
                    tracing::trace!("try_add_null_pdus: closing blk with Null PDU");

                    // We have room for a Null PDU
                    let mut null_pdu = MacResource::null_pdu();
                    null_pdu.length_ind = 2; // Null PDU is 16 bits
                    let _ = null_pdu.update_len_and_fill_ind(0);
                    null_pdu.to_bitbuf(&mut b.mac_block);

                    // TODO FIXME: it's possibly the best idea to still add fill bits trailing this null pdu.
                    // Check real-world captures.
                } else {
                    tracing::debug!(
                        "try_add_null_pdus: not enough space for Null PDU in block, got {} bits remaining",
                        b.mac_block.get_len_remaining()
                    );
                }
            }

            Some(b)
        } else {
            None
        }
    }

    /// Returns a mutable reference to the first scheduled resource for the given timeslot and address.
    pub fn dl_get_scheduled_resource_for_addr(&mut self, ts: TdmaTime, addr: &TetraAddress) -> Option<&mut DlSchedElem> {
        let queue = &mut self.dltx_queues[ts.t as usize - 1];

        for index in 0..queue.len() {
            let elem = &mut queue[index];
            if let DlSchedElem::Resource(pdu, _sdu, _repeat, _) = elem {
                if let Some(pdu_addr) = pdu.addr {
                    if pdu_addr == *addr {
                        // Found a resource for this address
                        return queue.get_mut(index);
                    }
                }
            }
        }
        // No resource for this address was found
        None
    }

    /// Make a minimal resource to contain a grant or a random access acknowledgement
    pub fn dl_make_minimal_resource(addr: &TetraAddress, grant: Option<BasicSlotgrant>, random_access_ack: bool) -> MacResource {
        let mut pdu = MacResource {
            fill_bits: false, // updated later
            pos_of_grant: 0,
            encryption_mode: 0,
            random_access_flag: random_access_ack,
            length_ind: 0, // updated later
            addr: Some(*addr),
            event_label: None,
            usage_marker: None,
            power_control_element: None,
            slot_granting_element: grant,
            chan_alloc_element: None,
        };
        pdu.update_len_and_fill_ind(0);
        pdu
    }

    /// Takes and removes ready grants and random access acknowledgements from
    /// the given timeslot's queue, returning them as a vec.
    pub fn dl_take_all_ready_grants_and_acks(
        &mut self,
        ts: TdmaTime,
        subscribers: &SubscriberRegistry,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
    ) -> Vec<DlSchedElem> {
        let queue = &mut self.dltx_queues[ts.t as usize - 1];
        let mut taken = Vec::new();

        let mut i = 0;
        while i < queue.len() {
            if matches!(
                queue[i],
                DlSchedElem::Grant(..) | DlSchedElem::PendingGrant(..) | DlSchedElem::RandomAccessAck(_)
            ) && Self::elem_is_ready_for_tx(&queue[i], ts, energy_saving, subscribers)
            {
                let elem = queue.remove(i);
                taken.push(elem);
            } else {
                i += 1;
            }
        }
        taken
    }

    /// Removes all elements from the schedule, except stolen blocks. This function is used
    /// when leaving hangtime to clear out any stale grants, resources, etc that can only be processed in signaling mode,
    /// while keeping stealing blocks that may still need to be transmitted via FACCH.
    /// Discarded elements are reported as such via tx_reporter if available. Returns true if elements were discarded.
    pub fn dl_drop_all_except_stolen(&mut self, timeslot: u8) -> bool {
        let queue = &mut self.dltx_queues[timeslot as usize - 1];
        let dropped_grant_addrs: Vec<TetraAddress> = queue
            .iter()
            .filter_map(|elem| match elem {
                DlSchedElem::Grant(addr, _, _) | DlSchedElem::PendingGrant(addr, _) => Some(*addr),
                _ => None,
            })
            .collect();
        let mut i = 0;
        let mut item_was_discarded = false;
        while i < queue.len() {
            if matches!(queue[i], DlSchedElem::Stealing(..)) {
                i += 1;
            } else {
                // Found a to-be-discarded element.
                // Remove, log, and call tx_reporter::mark_discarded() if applicable.
                // Logged at debug because this fires during normal hangtime entry/exit
                // races and isn't an anomaly worth surfacing as a warning. Per
                // proxiboi69 in MidnightBlueLabs/tetra-bluestation PR #85.
                let elem = queue.remove(i);
                item_was_discarded = true;
                tracing::debug!("dl_drop_all_except_stolen: discarding scheduled {:?} on ts {}", elem, timeslot);

                match elem {
                    DlSchedElem::Resource(_, _, tx_reporter, group_state) => {
                        // Report as discarded manually
                        Self::mark_reporter_discarded_if_pending(tx_reporter.or_else(|| group_state.and_then(|s| s.tx_reporter)));
                    }

                    DlSchedElem::FragBuf(_, group_state) => {
                        // Fragger self-marks any unsent fragments as discarded when dropped, so we don't need to do anything here.
                        Self::mark_reporter_discarded_if_pending(group_state.and_then(|s| s.tx_reporter));
                    }

                    DlSchedElem::RandomAccessAck(addr) => {
                        if dropped_grant_addrs
                            .iter()
                            .any(|grant_addr| grant_addr.ssi == addr.ssi && grant_addr.ssi_type == addr.ssi_type)
                        {
                            // ETSI EN 300 392-2 clauses 21.4.3.1 and 23.5.1.3.3
                            // tie a reserved random access acknowledgement to the
                            // corresponding slot grant. If hangtime cleanup drops
                            // the grant, do not later transmit an ACK-only STCH.
                            tracing::debug!(
                                "dl_drop_all_except_stolen: dropping RA ACK for {} because its grant is also discarded on ts {}",
                                addr,
                                timeslot
                            );
                        } else {
                            // Save the SSI so the next STCH for this address can carry
                            // random_access_flag=true (ETSI 21.4.3.1)
                            self.pending_ra_acks[timeslot as usize - 1].push(addr);
                        }
                    }

                    DlSchedElem::Grant(..) | DlSchedElem::PendingGrant(..) | DlSchedElem::Broadcast(_) => {
                        // Silently dropped as internal or not equipped with a tx_reporter
                    }
                    _ => unreachable!(),
                }
            }
        }

        item_was_discarded
    }

    pub fn dl_integrate_sched_elems_for_timeslot(
        &mut self,
        ts: TdmaTime,
        subscribers: &SubscriberRegistry,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
    ) {
        if !Self::can_carry_scheduled_schf(ts) {
            // EN 300 392-2 clauses 9.5.2 and 9.5.3 reserve fixed frame-18
            // BSCH/BNCH positions. Keep pending MAC-RESOURCE grants queued
            // until a frame-18 SCH/F opportunity or an ordinary frame.
            return;
        }

        // Remove all grants and acks from queue and collect them into a vec
        let grants_and_acks = self.dl_take_all_ready_grants_and_acks(ts, subscribers, energy_saving);

        // Process grants and acks
        for elem in grants_and_acks {
            let elem = match elem {
                DlSchedElem::PendingGrant(addr, res_req) => match self.ul_process_cap_req_from(ts, ts.t, addr, &res_req) {
                    Some((grant, usage_marker)) => DlSchedElem::Grant(addr, grant, usage_marker),
                    None => {
                        tracing::warn!(
                            "dl_integrate_sched_elems_for_timeslot: no grant opportunity for addr {} res_req {:?} at tx {}",
                            addr,
                            res_req,
                            ts
                        );
                        // EN 300 392-2 23.5.2.2.2 defines granting delay 1111 as
                        // "Wait for another slot grant", which restarts T.206
                        // without granting slots. Keep the EG-gated grant pending
                        // until a transmit window also has usable uplink capacity.
                        self.dl_defer_pending_grant_next_frame(addr, res_req);
                        let cap_alloc = if res_req == ReservationRequirement::Req1Subslot {
                            BasicSlotgrantCapAlloc::FirstSubslotGranted
                        } else {
                            BasicSlotgrantCapAlloc::from_req_slotcount(res_req.to_req_slotcount())
                        };
                        let wait_grant = BasicSlotgrant {
                            capacity_allocation: cap_alloc,
                            granting_delay: BasicSlotgrantGrantingDelay::WaitForAnotherSlotgrantMessage,
                        };
                        DlSchedElem::Grant(addr, wait_grant, None)
                    }
                },
                elem => elem,
            };

            // Try to find existing resource for this address
            let addr = match &elem {
                DlSchedElem::Grant(addr, _, _) | DlSchedElem::PendingGrant(addr, _) => addr,
                DlSchedElem::RandomAccessAck(addr) => addr,
                _ => unreachable!("BUG: unhandled match variant -- should never be reached"),
            };
            let mac_resource = self.dl_get_scheduled_resource_for_addr(ts, addr);
            match mac_resource {
                Some(DlSchedElem::Resource(pdu, _sdu, _repeat, _)) => {
                    // Integrate grant into the resource
                    match &elem {
                        DlSchedElem::Grant(_, grant, usage_marker) => {
                            tracing::debug!(
                                "dl_integrate_sched_elems_for_timeslot: Integrating grant {:?} into resource for addr {} marker {:?}",
                                grant,
                                addr,
                                usage_marker,
                            );
                            pdu.slot_granting_element = Some(grant.clone());
                            // Carry the marker through so the MS knows what to
                            // tag its reservation with on the next UL slot.
                            // Don't overwrite a marker we already set (e.g.
                            // when the grant came after an ACK that already
                            // populated it).
                            if pdu.usage_marker.is_none() {
                                pdu.usage_marker = *usage_marker;
                            }
                        }
                        DlSchedElem::RandomAccessAck(_) => {
                            tracing::debug!(
                                "dl_integrate_sched_elems_for_timeslot: Integrating ack into resource for addr {}",
                                addr
                            );
                            pdu.random_access_flag = true;
                        }
                        _ => unreachable!("BUG: unhandled match variant -- should never be reached"),
                    }
                }
                None => {
                    // No resource for this address was found, create a new one

                    let pdu = match &elem {
                        DlSchedElem::Grant(_, grant, usage_marker) => {
                            tracing::debug!(
                                "dl_integrate_sched_elems_for_timeslot: Creating new resource for addr {} with grant {:?} marker {:?}",
                                addr,
                                grant,
                                usage_marker,
                            );
                            let mut pdu = Self::dl_make_minimal_resource(addr, Some(grant.clone()), false);
                            pdu.usage_marker = *usage_marker;
                            pdu
                        }
                        DlSchedElem::RandomAccessAck(_) => {
                            tracing::debug!(
                                "dl_integrate_sched_elems_for_timeslot: Creating new resource for addr {} with ack",
                                addr
                            );
                            Self::dl_make_minimal_resource(addr, None, true)
                        }
                        _ => unreachable!("BUG: unhandled match variant -- should never be reached"),
                    };

                    // Push new resource into the queue. These do not need a tx_reporter
                    let dlsched_res = DlSchedElem::Resource(pdu, BitBuffer::new(0), None, None);
                    self.dltx_queues[ts.t as usize - 1].push(dlsched_res);
                }
                _ => unreachable!("BUG: unhandled match variant -- should never be reached"),
            }
        }
    }

    fn dl_build_block_from_signalling_schedule(
        &mut self,
        ts: TdmaTime,
        subscribers: &SubscriberRegistry,
        energy_saving: &mut HashMap<u32, EnergySavingAssignment>,
    ) -> Option<BitBuffer> {
        let mut buf_opt = None;

        while !self.dltx_queues[ts.t as usize - 1].is_empty() {
            let opt = self.dl_take_prioritized_sched_item(ts, subscribers, energy_saving);

            match opt {
                Some(sched_elem) => {
                    match sched_elem {
                        DlSchedElem::Broadcast(_) => {
                            unimplemented_log!("finalize_ts_for_tick: Broadcast scheduling not implemented");
                        }

                        DlSchedElem::Resource(pdu, sdu, tx_reporter, group_state) => {
                            let addr = pdu.addr;
                            let mut group_state = group_state.or_else(|| {
                                addr.and_then(|addr| Self::group_state_for_resource(addr, &pdu, &sdu, tx_reporter.clone(), subscribers))
                            });
                            if let Some(state) = group_state.as_mut() {
                                if let Some(addr) = addr {
                                    Self::retain_current_group_delivery_targets(state, addr, subscribers);
                                }
                                state.begin_batch_if_needed(ts, energy_saving);
                            }
                            let fragger_reporter = match group_state.as_ref() {
                                Some(state) if !state.is_final_batch() => None,
                                Some(state) => state.tx_reporter.clone(),
                                None => tx_reporter,
                            };
                            // Allocate bitbuf if not already done
                            let mut buf = buf_opt.unwrap_or_else(|| BitBuffer::new(SCH_F_CAP));
                            // Create fragger, either to send the whole PDU or to start fragmentation
                            let mut fragger = BsFragger::new(pdu, sdu, fragger_reporter);
                            let before_len = buf.get_len_written();
                            let fully_transmitted = fragger.get_next_chunk(&mut buf);
                            if buf.get_len_written() > before_len {
                                Self::mark_addr_signalling_activity(addr, ts, energy_saving);
                                Self::mark_group_signalling_activity(group_state.as_ref(), ts, energy_saving);
                            }
                            if let Some(state) = group_state.as_mut()
                                && fully_transmitted
                            {
                                state.mark_batch_covered();
                            }
                            if !fully_transmitted {
                                // Fragmentation was started and we have more chunks to send
                                // Enqueue fragger with remaining data for retrieval next frame
                                self.dl_enqueue_tma_frag_next_frame_with_group_state(fragger, group_state);
                            } else if let Some(state) = group_state
                                && !state.is_complete()
                            {
                                self.dl_enqueue_group_repeat_next_frame(state);
                            }
                            buf_opt = Some(buf);
                        }

                        DlSchedElem::FragBuf(mut fragger, mut group_state) => {
                            let addr = fragger.addr();
                            if let Some(state) = group_state.as_mut() {
                                if let Some(addr) = addr {
                                    Self::retain_current_group_delivery_targets(state, addr, subscribers);
                                }
                                state.begin_batch_if_needed(ts, energy_saving);
                            }
                            // Allocate bitbuf if not already done
                            let mut buf = buf_opt.unwrap_or_else(|| BitBuffer::new(SCH_F_CAP));
                            let before_len = buf.get_len_written();
                            let fully_transmitted = fragger.get_next_chunk(&mut buf);
                            if buf.get_len_written() > before_len {
                                Self::mark_addr_signalling_activity(addr, ts, energy_saving);
                                Self::mark_group_signalling_activity(group_state.as_ref(), ts, energy_saving);
                            }
                            if let Some(state) = group_state.as_mut()
                                && fully_transmitted
                            {
                                state.mark_batch_covered();
                            }
                            if !fully_transmitted {
                                // Fragmentation was continued and we still have more chunks to send
                                // Re-enqueue fragger with remaining data for retrieval next frame
                                self.dl_enqueue_tma_frag_next_frame_with_group_state(fragger, group_state);
                            } else if let Some(state) = group_state
                                && !state.is_complete()
                            {
                                self.dl_enqueue_group_repeat_next_frame(state);
                            }
                            buf_opt = Some(buf);
                        }

                        DlSchedElem::Stealing(_, _, tx_reporter, group_state) => {
                            // Stealing items should only appear on traffic timeslots; discard if found here
                            tracing::warn!(
                                "dl_build_block_from_signalling_schedule: Stealing item found on non-traffic ts {}, discarding",
                                ts.t
                            );
                            Self::mark_reporter_discarded_if_pending(
                                tx_reporter.or_else(|| group_state.and_then(|state| state.tx_reporter)),
                            );
                        }

                        _ => {
                            tracing::error!("UMAC: finalize_ts_for_tick: unexpected DlSchedElem type {:?}, skipping", sched_elem);
                        }
                    }
                }
                None => {
                    // No more items to process, we can finalize this timeslot
                    break;
                }
            }
        }

        // If any signalling could not be sent this slot, it should be in the next slot queue.
        // Drain next_slot_queue into the front of the current slot queue so deferred PDUs are
        // sent before any newly-arriving ones in the next frame.  Using extend instead of swap
        // avoids a panic when the current queue already contains items (e.g. two back-to-back
        // P2P calls each deferring a chan_alloc PDU within the same tick).
        if !self.dltx_next_slot_queue.is_empty() {
            let current = &mut self.dltx_queues[ts.t as usize - 1];
            // Prepend: move deferred items to front, then re-append any items already queued.
            let mut merged = std::mem::take(&mut self.dltx_next_slot_queue);
            merged.extend(current.drain(..));
            *current = merged;
        }

        buf_opt
    }

    /// Build traffic block for active circuit. Returns (optional_tch_block, optional_stch_block):
    /// - tch_block: queued ACELP speech or raw half-slot TCH/S, or None when no uplink speech was received
    /// - stch_block: STCH signaling (124 bits) for FACCH stealing (EN 300 392-2, clause 23.5)
    /// Also reports transmission, if a TxReporter was attached to the DlSchedElem::Stealing element
    fn dl_build_traffic_block(
        &mut self,
        ts: TdmaTime,
        subscribers: &SubscriberRegistry,
        energy_saving: &mut HashMap<u32, EnergySavingAssignment>,
    ) -> (Option<DlTchBlock>, Option<BitBuffer>) {
        let tch_buf = self.circuits.take_block(ts.t).and_then(|block| match block {
            CircuitTxBlock::AcElp(block) => {
                let mut buf = BitBuffer::from_vec(block);
                // Raw ACELP speech (274 bits for TCH/S).
                // Clamp to TCH_S_CAP as Vec may be larger (e.g. 280 bits).
                buf.set_raw_end(buf.get_raw_start() + TCH_S_CAP);
                Some(DlTchBlock::AcElp(buf))
            }
            CircuitTxBlock::RawTchSHalfSlot { block_num, type5_bits } => {
                if block_num != PhyBlockNum::Block2 {
                    tracing::warn!(
                        "dl_build_traffic_block: dropping unsupported raw TCH/S half-slot {:?} on ts {}",
                        block_num,
                        ts.t
                    );
                    return None;
                }
                if type5_bits.len() != 216 {
                    tracing::warn!(
                        "dl_build_traffic_block: dropping raw TCH/S Block2 with {} bits on ts {}",
                        type5_bits.len(),
                        ts.t
                    );
                    return None;
                }
                Some(DlTchBlock::RawTchSHalfSlot {
                    block_num,
                    type5_bits: BitBuffer::from_bitarr(&type5_bits),
                })
            }
        });

        // Check for FACCH/stealing: take a queued Stealing item (highest priority signaling)
        let (stch_opt, stealing_addr_opt, tx_reporter_opt, group_state_opt) = {
            if ts.t >= 1 && (ts.t as usize) <= self.dltx_queues.len() {
                self.prune_completed_stale_group_states_for_slot(ts.t as usize - 1, subscribers);
            }
            let q = &mut self.dltx_queues[ts.t as usize - 1];
            if let Some(i) = q
                .iter()
                .position(|e| matches!(e, DlSchedElem::Stealing(..)) && Self::elem_is_ready_for_tx(e, ts, energy_saving, subscribers))
            {
                match q.remove(i) {
                    DlSchedElem::Stealing(buf, addr, tx_reporter, group_state) => (Some(buf), Some(addr), tx_reporter, group_state),
                    _ => unreachable!(),
                }
            } else {
                (None, None, None, None)
            }
        };

        // Warn about other queued signaling that can't be sent via stealing yet
        if stch_opt.is_none() && !self.dltx_queues[ts.t as usize - 1].is_empty() {
            tracing::warn!("dl_build_traffic_block: queued signaling on ts {} but no stealing item", ts.t);
        }

        let mut should_report_transmitted = stch_opt.is_some();
        if let (Some(block), Some(addr)) = (stch_opt.as_ref(), stealing_addr_opt)
            && addr.ssi_type == SsiType::Gssi
        {
            let mut group_state = group_state_opt.unwrap_or_else(|| {
                GroupStealingState::new(
                    Self::group_targets(addr, subscribers),
                    tx_reporter_opt.clone(),
                    addr.ssi != PREDEFINED_BROADCAST_GSSI,
                )
            });
            if !group_state.targets.is_empty() {
                Self::retain_current_group_stealing_targets(&mut group_state, addr, subscribers);
            }
            if !group_state.targets.is_empty() {
                group_state.begin_batch_if_needed(ts, energy_saving);
                Self::mark_stealing_signalling_activity(addr, Some(&group_state), ts, subscribers, energy_saving);
                group_state.mark_batch_covered();
                should_report_transmitted = group_state.is_complete();
                if !should_report_transmitted {
                    self.dl_requeue_group_stealing(ts.t, block.clone(), addr, group_state);
                }
            } else {
                Self::mark_stealing_signalling_activity(addr, None, ts, subscribers, energy_saving);
            }
        } else if let Some(addr) = stealing_addr_opt {
            Self::mark_stealing_signalling_activity(addr, None, ts, subscribers, energy_saving);
        }

        if should_report_transmitted && let Some(tx_reporter) = tx_reporter_opt {
            tx_reporter.mark_transmitted();
        }

        (tch_buf, stch_opt)
    }

    fn generate_stch_null_block(&self) -> BitBuffer {
        let mut buf = BitBuffer::new(SCH_HD_CAP);
        MacResource::null_pdu().to_bitbuf(&mut buf);
        buf
    }

    /// Return first queued grant.
    /// If none; return first in-progress fragmented message.
    /// If none; return first to-be-transmitted resource.
    /// If none, return None.
    pub fn dl_take_prioritized_sched_item(
        &mut self,
        ts: TdmaTime,
        subscribers: &SubscriberRegistry,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
    ) -> Option<DlSchedElem> {
        if !Self::can_carry_scheduled_schf(ts) {
            return None;
        }

        // Map 1-based ts to 0-based index, bail on 0 or out of range.
        // (ts.t should always be 1..=4, but guard rather than unwrap so a bad ts can't
        // panic the scheduler.)
        if ts.t < 1 || (ts.t as usize) > self.dltx_queues.len() {
            tracing::warn!("dl_take_prioritized_sched_item: ts.t={} out of range, no item", ts.t);
            return None;
        }
        let slot = ts.t as usize - 1;
        self.prune_completed_stale_group_states_for_slot(slot, subscribers);
        let Some(q) = self.dltx_queues.get_mut(slot) else {
            return None;
        };

        // Return grants first, but only when the addressed MS should be listening.
        if let Some(i) = q.iter().position(|e| {
            matches!(e, DlSchedElem::Grant(..) | DlSchedElem::PendingGrant(..))
                && Self::elem_is_ready_for_tx(e, ts, energy_saving, subscribers)
        }) {
            return Some(q.remove(i));
        }

        // Channel allocations carry call-control resource assignment. Keep
        // them ahead of ready EG grant traffic so a private-call setup is not
        // held behind ordinary reservation churn once the addressed MS can
        // receive it (EN 300 392-2 clauses 14, 21.5.2 and 23.5.2.2.7).
        if let Some(i) = q
            .iter()
            .position(|e| Self::elem_has_channel_allocation(e) && Self::elem_is_ready_for_tx(e, ts, energy_saving, subscribers))
        {
            return Some(q.remove(i));
        }

        // Grants and random-access ACKs are integrated into MAC-RESOURCE before
        // SCH/F building. Keep those resources ahead of fragmentation backlog so
        // EN 300 392-2 21.4.3.1 ACK/grant timing is not delayed by ordinary data.
        if let Some(i) = q
            .iter()
            .position(|e| Self::elem_has_integrated_grant_or_ack(e) && Self::elem_is_ready_for_tx(e, ts, energy_saving, subscribers))
        {
            return Some(q.remove(i));
        }

        // Return FragBufs next, but only when the addressed MS should be listening.
        if let Some(i) = q
            .iter()
            .position(|e| matches!(e, DlSchedElem::FragBuf(_, _)) && Self::elem_is_ready_for_tx(e, ts, energy_saving, subscribers))
        {
            return Some(q.remove(i));
        }

        // Return Resources last, but only when the addressed MS should be listening.
        if let Some(i) = q
            .iter()
            .position(|e| matches!(e, DlSchedElem::Resource(_, _, _, _)) && Self::elem_is_ready_for_tx(e, ts, energy_saving, subscribers))
        {
            return Some(q.remove(i));
        }

        None
    }

    pub fn tick_start(&mut self, ts: TdmaTime) {
        // Increment current time
        self.cur_dltime = self.cur_dltime.add_timeslots(1);
        assert!(
            ts == self.cur_dltime,
            "BsChannelScheduler tick_start: ts mismatch, expected {}, got {}",
            self.cur_dltime,
            ts
        );
    }

    /// Prepares a scheduled FUTURE timeslot for transfer to lmac and transmission
    /// Generates BBK block
    /// If the timeslot is not full, generates SYNC SB1/SB2 blocks.
    /// Increments cur_ts by one timeslot.
    /// Caller should check timestamp of returned DlTxElem to prevent desync
    pub fn finalize_ts_for_tick(
        &mut self,
        subscribers: &SubscriberRegistry,
        energy_saving: &mut HashMap<u32, EnergySavingAssignment>,
    ) -> TmvUnitdataReqSlot {
        // Reset the per-frame chan_alloc flag when we start processing ts1 (MCCH slot).
        // This allows the next DConnect MCCH to go normally while the subsequent DConnectAck
        // MCCH is deferred to the following frame.
        if self.cur_dltime.add_timeslots(MACSCHED_TX_AHEAD as i32).t == 1 {
            self.mcch_chan_alloc_sent_this_frame = false;
        }

        // We finalize a FUTURE slot: cur_ts plus some number of timeslots
        let ts = self.cur_dltime.add_timeslots(MACSCHED_TX_AHEAD as i32);
        self.precomps.mac_sync.time = ts;
        self.precomps.mac_sysinfo1.hyperframe_number = Some(ts.h);
        self.precomps.mac_sysinfo2.hyperframe_number = Some(ts.h);

        let dl_circuit_active = self.circuits.is_active(Direction::Dl, ts.t) && ts.f != 18;
        let ul_circuit_active = self.circuits.is_active(Direction::Ul, ts.t) && ts.f != 18;

        // During hangtime we stop sending traffic frames and switch to signalling mode.
        // Keep traffic mode while FACCH/stealing is still queued for delivery.
        let hang_effective = if (2..=4).contains(&ts.t) {
            self.is_hangtime_effective(ts.t)
        } else {
            false
        };

        let dl_is_traffic = dl_circuit_active && !hang_effective;
        let ul_is_traffic = ul_circuit_active && !hang_effective;

        // Build the block for this timeslot with anything scheduled (traffic or signalling)
        // For traffic timeslots, also check for FACCH/stealing (STCH half-slot)
        let ul_phy = if ul_is_traffic { PhysicalChannel::Tp } else { PhysicalChannel::Cp };

        let mut elem = if dl_is_traffic {
            let (tch_buf_opt, stch_opt) = self.dl_build_traffic_block(ts, subscribers, energy_saving);

            if let Some(stch_buf) = stch_opt {
                // FACCH/Stealing: 1st half = STCH signaling. If uplink speech
                // is available, keep it in the second half. If not, EN 300 392-2
                // clause 23.8.5 permits filling the channel with C-plane Null
                // PDUs instead of fabricating an unproven all-zero speech frame.
                // NDB uses NormalTrainSeq2 for independent half-slot demodulation (EN 300 392-2, clause 23.5).
                tracing::info!(
                    "finalize_ts_for_tick: FACCH stealing on ts {} (stch={} bits, speech_present={})",
                    ts.t,
                    stch_buf.get_len(),
                    tch_buf_opt.is_some()
                );
                TmvUnitdataReqSlot {
                    ts,
                    blk1: Some(TmvUnitdataReq {
                        logical_channel: LogicalChannel::Stch,
                        mac_block: stch_buf,
                        scrambling_code: self.scrambling_code,
                    }),
                    blk2: Some(match tch_buf_opt {
                        Some(DlTchBlock::AcElp(tch_buf)) => TmvUnitdataReq {
                            logical_channel: LogicalChannel::TchS,
                            mac_block: tch_buf,
                            scrambling_code: self.scrambling_code,
                        },
                        Some(DlTchBlock::RawTchSHalfSlot { block_num, type5_bits }) => {
                            tracing::info!(
                                "finalize_ts_for_tick: preserving raw TCH/S {:?} after FACCH on ts {}",
                                block_num,
                                ts.t
                            );
                            TmvUnitdataReq {
                                logical_channel: LogicalChannel::TchS,
                                mac_block: type5_bits,
                                scrambling_code: self.scrambling_code,
                            }
                        }
                        None => TmvUnitdataReq {
                            logical_channel: LogicalChannel::Stch,
                            mac_block: self.generate_stch_null_block(),
                            scrambling_code: self.scrambling_code,
                        },
                    }),
                    bbk: None,
                    ul_phy_chan: ul_phy,
                }
            } else if let Some(tch_buf) = tch_buf_opt {
                match tch_buf {
                    DlTchBlock::AcElp(tch_buf) => {
                        // Normal traffic: full-slot TCH
                        TmvUnitdataReqSlot {
                            ts,
                            blk1: Some(TmvUnitdataReq {
                                logical_channel: LogicalChannel::TchS,
                                mac_block: tch_buf,
                                scrambling_code: self.scrambling_code,
                            }),
                            blk2: None,
                            bbk: None,
                            ul_phy_chan: ul_phy,
                        }
                    }
                    DlTchBlock::RawTchSHalfSlot { block_num, type5_bits } => {
                        // EN 300 392-2 clause 23.8.5 requires the BS to preserve
                        // the timing and half-slot position of U-plane TCH. With no
                        // local FACCH pending, fill the stolen first half with a
                        // C-plane Null PDU and keep the received TCH/S in Block2.
                        tracing::info!(
                            "finalize_ts_for_tick: preserving raw TCH/S {:?} with STCH Null first half on ts {}",
                            block_num,
                            ts.t
                        );
                        TmvUnitdataReqSlot {
                            ts,
                            blk1: Some(TmvUnitdataReq {
                                logical_channel: LogicalChannel::Stch,
                                mac_block: self.generate_stch_null_block(),
                                scrambling_code: self.scrambling_code,
                            }),
                            blk2: Some(TmvUnitdataReq {
                                logical_channel: LogicalChannel::TchS,
                                mac_block: type5_bits,
                                scrambling_code: self.scrambling_code,
                            }),
                            bbk: None,
                            ul_phy_chan: ul_phy,
                        }
                    }
                }
            } else {
                // No uplink speech was received for this active circuit. Per
                // EN 300 392-2 clause 23.8.5, keep transmitting on the assigned
                // channel using C-plane Null PDUs rather than an all-zero ACELP
                // frame that is not proven to be a valid silence/substitution frame.
                TmvUnitdataReqSlot {
                    ts,
                    blk1: Some(TmvUnitdataReq {
                        logical_channel: LogicalChannel::Stch,
                        mac_block: self.generate_stch_null_block(),
                        scrambling_code: self.scrambling_code,
                    }),
                    blk2: Some(TmvUnitdataReq {
                        logical_channel: LogicalChannel::Stch,
                        mac_block: self.generate_stch_null_block(),
                        scrambling_code: self.scrambling_code,
                    }),
                    bbk: None,
                    ul_phy_chan: ul_phy,
                }
            }
        } else {
            // Signalling mode (either no circuit, or hangtime on an allocated timeslot)
            // Integrate all grants and random access acks into resources (either existing or new)
            self.dl_integrate_sched_elems_for_timeslot(ts, subscribers, energy_saving);

            // Fill our signalling block with scheduled items (if any)
            let buf = self.dl_build_block_from_signalling_schedule(ts, subscribers, energy_saving);
            if let Some(buf) = buf {
                TmvUnitdataReqSlot {
                    ts,
                    blk1: Some(TmvUnitdataReq {
                        logical_channel: LogicalChannel::SchF,
                        mac_block: buf,
                        scrambling_code: self.scrambling_code,
                    }),
                    blk2: None,
                    bbk: None,
                    ul_phy_chan: ul_phy,
                }
            } else {
                // If this is an allocated traffic slot in hangtime, keep it alive with an idle SCH/F (Null PDU).
                // Otherwise, fall back to default SYNC/SYSINFO.
                if hang_effective && dl_circuit_active {
                    TmvUnitdataReqSlot {
                        ts,
                        blk1: Some(TmvUnitdataReq {
                            logical_channel: LogicalChannel::SchF,
                            mac_block: self.generate_hangtime_idle_schf(),
                            scrambling_code: self.scrambling_code,
                        }),
                        blk2: None,
                        bbk: None,
                        ul_phy_chan: ul_phy,
                    }
                } else {
                    // Put default SYNC/SYSINFO frame
                    TmvUnitdataReqSlot {
                        ts,
                        blk1: None,
                        blk2: None,
                        bbk: None,
                        ul_phy_chan: ul_phy,
                    }
                }
            }
        };

        // Sanity check: frame 18 carries scheduled SCH/F only outside fixed
        // BSCH/BNCH positions (EN 300 392-2 clauses 9.5.2 and 9.5.3).
        if elem.blk1.is_some() {
            assert!(
                Self::can_carry_scheduled_schf(ts),
                "scheduled SCH/F is not allowed on fixed frame-18 slot {}",
                ts
            );
        }

        // Construct the BBK block to reflect UL/DL usage
        assert!(elem.bbk.is_none(), "BBK block already set");
        elem.bbk = Some(self.generate_bbk_block(ts));

        // tracing::trace!("finalize_ts_for_tick: have {}{}{}",
        //     if elem.bbk.is_some() { "bbk " } else { "" },
        //     if elem.blk1.is_some() { "blk1 " } else { "" },
        //     if elem.blk2.is_some() { "blk2 " } else { "" });

        // Populate blk1 if empty: BSCH on frame 18, SCH/HD on other frames
        if elem.blk1.is_none() {
            elem.blk1 = Some(self.generate_default_blks(ts));
        };

        // Check if second block may still be populated (blk1 is half-slot and blk2 is None)
        let blk1_lchan = elem.blk1.as_ref().unwrap().logical_channel;

        if blk1_lchan == LogicalChannel::Stch {
            // FACCH/Stealing: blk1 = STCH signaling, blk2 = TCH speech (already set above)
            assert!(elem.blk2.is_some(), "STCH blk1 must have blk2 (TCH half-slot)");
        } else if elem.blk2.is_none() && (blk1_lchan == LogicalChannel::Bsch || blk1_lchan == LogicalChannel::SchHd) {
            // Populate blk2 with SYSINFO if blk1 is half-slot (not STCH)
            // Check blk1 is indeed short (124 for half-slot or 60 for SYNC)
            assert!(elem.blk1.as_ref().unwrap().mac_block.get_len() <= 124);

            let mut buf = BitBuffer::new(124);

            // Write MAC-SYSINFO (alternating sysinfo1/sysinfo2), followed by MLE-SYSINFO
            if ts.t % 2 == 1 {
                self.precomps.mac_sysinfo1.to_bitbuf(&mut buf);
            } else {
                self.precomps.mac_sysinfo2.to_bitbuf(&mut buf);
            }
            self.precomps.mle_sysinfo.to_bitbuf(&mut buf);

            elem.blk2 = Some(TmvUnitdataReq {
                logical_channel: LogicalChannel::Bnch,
                mac_block: buf,
                scrambling_code: self.scrambling_code,
            })
        } else if elem.blk2.is_none() {
            // Full-slot block (TCH or SCH/F): just verify it fills both half slots
            assert!(
                elem.blk1.as_ref().unwrap().mac_block.get_len() >= 268,
                "blk1 should be full-slot but is too short"
            );
        }

        assert!(elem.bbk.is_some(), "BBK block is not set, this should not happen");
        assert!(elem.blk1.is_some(), "blk1 block is not set, this should not happen");

        // If signalling channels are here, and there is spare room, we need to close them with a Null pdu
        elem.blk1 = self.try_add_null_pdus(elem.blk1);
        elem.blk2 = self.try_add_null_pdus(elem.blk2);

        // Move all BitBuffer positions to the start of the window
        elem.bbk.as_mut().unwrap().mac_block.seek(0);
        elem.blk1.as_mut().unwrap().mac_block.seek(0);
        if let Some(blk2) = elem.blk2.as_mut() {
            blk2.mac_block.seek(0);
        }

        // tracing::warn!("start finalize");
        // self.dump_ul_schedule_full(true);

        // Clear UL schedule for this timeslot. Releasing the usage_marker
        // alongside ul1/ul2 keeps the marker pool from leaking — once both
        // slots of a reservation have been consumed, the marker is free to
        // be re-issued. (If a reservation extends over multiple frames this
        // gets called once per consumed slot pair, which is correct.)
        let index = self.ul_ts_to_sched_index(&ts.add_timeslots(-4));
        self.ulsched[ts.t as usize - 1][index].ul1 = None;
        self.ulsched[ts.t as usize - 1][index].ul2 = None;
        self.ulsched[ts.t as usize - 1][index].usage_marker = None;

        // tracing::warn!("end finalize");
        // self.dump_ul_schedule_full(true);

        // We now have our bbk, blk1 and (optional) blk2
        elem
    }

    fn generate_bbk_block(&self, ts: TdmaTime) -> TmvUnitdataReq {
        let (ul_traffic_usage, dl_traffic_usage) = if ts.f == 18 {
            (None, None)
        } else {
            (
                self.circuits.get_usage(Direction::Ul, ts.t),
                self.circuits.get_usage(Direction::Dl, ts.t),
            )
        };

        // Generate BBK block
        let mut aach_bb = BitBuffer::new(14);
        if ts.f != 18 {
            let mut aach = AccessAssign::default();

            match ts.t {
                1 => {
                    assert!(dl_traffic_usage.is_none(), "DL ts 1 can't be traffic");
                    assert!(ul_traffic_usage.is_none(), "UL ts 1 can't be traffic (is this allowed?"); // TODO FIXME check spec

                    // TS1 (MCCH) DL is always CommonControl — that doesn't
                    // change for individual reservations.
                    aach.dl_usage = AccessAssignDlUsage::CommonControl;

                    // UL behaviour: when this slot has an active uplink
                    // reservation with a usage_marker (i.e. a multi-slot grant
                    // we issued previously), the AACH must announce
                    // `Traffic(marker)` per ETSI TS 100 392-2 §23.5.2 so the
                    // MS holding the reservation can identify "its" slot and
                    // continue the fragmented burst with MacEndUl. Without
                    // this, the MS sees CommonOnly, treats the slot as random
                    // access, and abandons the burst after the first frag —
                    // leaving location updates / re-attaches stuck in an
                    // infinite random-access loop (the symptom we observed
                    // when an MS re-entered coverage and couldn't TX/RX).
                    let ul_sched = &self.ulsched[0][self.ul_ts_to_sched_index(&ts)];
                    let ul_usage_for_slot = self.ul_get_usage(ts);
                    match ul_usage_for_slot {
                        AccessAssignUlUsage::Traffic(_) => {
                            // Reservation in flight: hand the marker through
                            // AACH so the MS commits to its assigned slot.
                            aach.ul_usage = ul_usage_for_slot;
                            // For Traffic UL usage we don't emit f1/f2 access
                            // fields — the slot is fully allocated.
                        }
                        _ => {
                            // EN 300 392-2 clauses 23.5.1.3.3 and 23.5.2.2.7:
                            // keep TS1 DL as CommonControl, but mark each
                            // granted uplink subslot unavailable for random
                            // access. A base frame length of 0 encodes a
                            // reserved subslot per clause 21.5.1.
                            aach.ul_usage = AccessAssignUlUsage::CommonOnly;
                            aach.f1_af1 = Some(AccessField {
                                access_code: 0,
                                base_frame_len: if ul_sched.ul1.is_some() { 0 } else { 4 },
                            });
                            aach.f2_af2 = Some(AccessField {
                                access_code: 0,
                                base_frame_len: if ul_sched.ul2.is_some() { 0 } else { 4 },
                            });
                        }
                    }
                }
                2..=4 => {
                    // Additional channels (TS2..TS4).
                    // Normal operation: Traffic(usage) when a circuit is active, else Unallocated.
                    // Hangtime: immediately switch AACH to AssignedControl so radios
                    // detect the end of traffic in the same frame as D-TX CEASED.
                    // The timeslot may still be in traffic mode (for STCH delivery) but
                    // the AACH reflects the new channel state.
                    let in_hangtime = (2..=4).contains(&ts.t) && self.hangtime[ts.t as usize - 1];

                    if in_hangtime && (dl_traffic_usage.is_some() || ul_traffic_usage.is_some()) {
                        aach.dl_usage = AccessAssignDlUsage::AssignedControl;
                        // AssignedOnly (Header 2) allows random access for MSs on
                        // the assigned channel while blocking common control MSs.
                        aach.ul_usage = AccessAssignUlUsage::AssignedOnly;
                        aach.f2_af = Some(AccessField {
                            access_code: 0,
                            base_frame_len: 4,
                        });
                    } else {
                        aach.dl_usage = if let Some(usage) = dl_traffic_usage {
                            AccessAssignDlUsage::Traffic(usage)
                        } else {
                            AccessAssignDlUsage::Unallocated
                        };
                        aach.ul_usage = if let Some(usage) = ul_traffic_usage {
                            AccessAssignUlUsage::Traffic(usage)
                        } else {
                            AccessAssignUlUsage::Unallocated
                        };
                    }
                }
                _ => {
                    tracing::error!("UMAC: generate_bbk_block: invalid timeslot {} (expected 1-4)", ts.t);
                    return TmvUnitdataReq {
                        logical_channel: LogicalChannel::Aach,
                        mac_block: BitBuffer::new(14),
                        scrambling_code: self.scrambling_code,
                    };
                }
            }

            aach.to_bitbuf(&mut aach_bb);
        } else {
            // Fr18. EN 300 392-2 clauses 23.5.2.2.1 and 23.5.2.2.7 allow
            // reserved uplink access in frame 18 except for predefined common
            // linearization opportunities. The AACH for frame 18 carries only
            // uplink access rights, so reflect any legal reservation here.
            assert!(ul_traffic_usage.is_none() && dl_traffic_usage.is_none());
            let ul_usage_for_slot = if ts.is_mandatory_clch() {
                AccessAssignUlUsage::CommonOnly
            } else {
                self.ul_get_usage(ts)
            };
            let ul_sched = &self.ulsched[ts.t as usize - 1][self.ul_ts_to_sched_index(&ts)];
            let frame18_access_field_for = |reserved: bool| AccessField {
                access_code: 0,
                base_frame_len: if reserved { 0 } else { 4 },
            };

            let aach = match ul_usage_for_slot {
                AccessAssignUlUsage::Traffic(_) => AccessAssignFr18 {
                    ul_usage: ul_usage_for_slot,
                    f2_af: Some(AccessField {
                        access_code: 0,
                        base_frame_len: 0,
                    }),
                    ..Default::default()
                },
                AccessAssignUlUsage::CommonAndAssigned | AccessAssignUlUsage::AssignedOnly => AccessAssignFr18 {
                    ul_usage: ul_usage_for_slot,
                    // EN 300 392-2 clause 23.5.2.2.7: granted uplink
                    // subslots must be marked reserved in ACCESS-ASSIGN.
                    // Table 21.83 keeps separate access fields for frame-18
                    // headers 01/10; table 21.86 encodes reserved as
                    // base-frame-length 0.
                    f1_af1: Some(frame18_access_field_for(ul_sched.ul1.is_some())),
                    f2_af2: Some(frame18_access_field_for(ul_sched.ul2.is_some())),
                    ..Default::default()
                },
                _ => AccessAssignFr18 {
                    ul_usage: AccessAssignUlUsage::CommonOnly,
                    f1_af1: Some(AccessField {
                        access_code: 0,
                        base_frame_len: 1,
                    }),
                    f2_af2: Some(AccessField {
                        access_code: 0,
                        base_frame_len: 0,
                    }),
                    ..Default::default()
                },
            };
            aach.to_bitbuf(&mut aach_bb);
        }

        TmvUnitdataReq {
            logical_channel: LogicalChannel::Aach,
            mac_block: aach_bb,
            scrambling_code: self.scrambling_code,
        }
    }

    fn generate_default_blks(&self, ts: TdmaTime) -> TmvUnitdataReq {
        match (ts.f, ts.t) {
            (1..=17, 1) => {
                // Two options: [Blk1: SCH/HD Null | Blk2: BNCH SYSINFO] or [Both: SCH/F Null]
                // Alternate every frame
                match ts.f % 2 {
                    0 => {
                        // Half-slot Null PDU on SCH/HD, SYSINFO gets added later as BNCH blk2
                        let mut buf1 = BitBuffer::new(SCH_HD_CAP);
                        let blk1 = MacResource::null_pdu();
                        blk1.to_bitbuf(&mut buf1);
                        TmvUnitdataReq {
                            logical_channel: LogicalChannel::SchHd,
                            mac_block: buf1,
                            scrambling_code: self.scrambling_code,
                        }
                    }
                    1 => {
                        // Full-slot Null PDU
                        let mut buf = BitBuffer::new(SCH_F_CAP);
                        let blk = MacResource::null_pdu();
                        blk.to_bitbuf(&mut buf);
                        TmvUnitdataReq {
                            logical_channel: LogicalChannel::SchF,
                            mac_block: buf,
                            scrambling_code: self.scrambling_code,
                        }
                    }
                    _ => unreachable!("BUG: unhandled match variant -- should never be reached"), // never happens
                }
            }
            (1..=17, 2..=4) | (18, _) => {
                // SYNC + SYSINFO (added later)
                let mut buf = BitBuffer::new(60);
                self.precomps.mac_sync.to_bitbuf(&mut buf);
                self.precomps.mle_sync.to_bitbuf(&mut buf);
                TmvUnitdataReq {
                    logical_channel: LogicalChannel::Bsch,
                    mac_block: buf,
                    scrambling_code: scrambler::SCRAMB_INIT,
                }
            }
            _ => unreachable!("BUG: unhandled match variant -- should never be reached"), // never happens
        }
    }

    pub fn dump_ul_schedule(&self, skip_empty: bool) {
        let ts = self.cur_dltime;
        tracing::info!("Dumping uplink schedule for {}:", ts);
        for dist in 0..MACSCHED_NUM_FRAMES - 1 {
            let ts = ts.add_timeslots(dist as i32 * 4);
            let index = self.ul_ts_to_sched_index(&ts);
            let elem = &self.ulsched[ts.t as usize - 1][index];
            if skip_empty && elem.ul1.is_none() && elem.ul2.is_none() {
                continue;
            }
            tracing::info!("  Schedule {}: {:?}", ts, elem);
        }
    }

    pub fn dump_ul_schedule_full(&self, skip_empty: bool) {
        tracing::info!("Dumping uplink schedule for {}:", self.cur_dltime);

        for dist in 0..MACSCHED_NUM_FRAMES - 1 {
            let ts = self.cur_dltime.add_timeslots(dist as i32 * 4);
            let index = self.ul_ts_to_sched_index(&ts);
            if skip_empty
                && self.ulsched[0][index].ul1.is_none()
                && self.ulsched[0][index].ul2.is_none()
                && self.ulsched[1][index].ul1.is_none()
                && self.ulsched[1][index].ul2.is_none()
                && self.ulsched[2][index].ul1.is_none()
                && self.ulsched[2][index].ul2.is_none()
                && self.ulsched[3][index].ul1.is_none()
                && self.ulsched[3][index].ul2.is_none()
            {
                continue;
            }
            tracing::info!(
                "  Schedule {}: ({} / {})  ({} / {})  ({} / {})  ({} / {})",
                ts,
                self.ulsched[0][index].ul1.map_or("-".to_string(), |v| v.to_string()),
                self.ulsched[0][index].ul2.map_or("-".to_string(), |v| v.to_string()),
                self.ulsched[1][index].ul1.map_or("-".to_string(), |v| v.to_string()),
                self.ulsched[1][index].ul2.map_or("-".to_string(), |v| v.to_string()),
                self.ulsched[2][index].ul1.map_or("-".to_string(), |v| v.to_string()),
                self.ulsched[2][index].ul2.map_or("-".to_string(), |v| v.to_string()),
                self.ulsched[3][index].ul1.map_or("-".to_string(), |v| v.to_string()),
                self.ulsched[3][index].ul2.map_or("-".to_string(), |v| v.to_string())
            );
        }
    }

    pub fn dump_dl_queue(&self) {
        tracing::info!("Dumping downlink queue:");
        for (index, elem) in self.dltx_queues.iter().enumerate() {
            for e in elem {
                tracing::trace!("  ts[{}] {:?}", index, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use tetra_core::{
        TxState,
        address::{SsiType, TetraAddress},
        debug::setup_logging_default,
    };

    use tetra_pdus::{
        mle::{
            fields::bs_service_details::BsServiceDetails,
            pdus::{d_mle_sync::DMleSync, d_mle_sysinfo::DMleSysinfo},
        },
        umac::{
            enums::sysinfo_opt_field_flag::SysinfoOptFieldFlag,
            fields::{
                channel_allocation::ChanAllocElement, sysinfo_default_def_for_access_code_a::SysinfoDefaultDefForAccessCodeA,
                sysinfo_ext_services::SysinfoExtendedServices,
            },
            pdus::{mac_sync::MacSync, mac_sysinfo::MacSysinfo},
        },
    };
    use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};

    use super::*;

    pub fn get_testing_slotter() -> BsChannelScheduler {
        let _guard = setup_logging_default(None);
        let ext_services = SysinfoExtendedServices {
            auth_required: false,
            class1_supported: true,
            class2_supported: true,
            class3_supported: false,
            sck_n: Some(0),
            dck_retrieval_during_cell_select: None,
            dck_retrieval_during_cell_reselect: None,
            linked_gck_crypto_periods: None,
            short_gck_vn: None,
            sdstl_addressing_method: 2,
            gck_supported: false,
            section: 0,
            section_data: 0,
        };

        let def_access = SysinfoDefaultDefForAccessCodeA {
            imm: 8,
            wt: 5,
            nu: 5,
            fl_factor: false,
            ts_ptr: 0,
            min_pdu_prio: 0,
        };

        let sysinfo1 = MacSysinfo {
            main_carrier: 1001,
            freq_band: 4,
            freq_offset_index: 0,
            duplex_spacing: 0,
            reverse_operation: false,
            num_of_csch: 0,
            ms_txpwr_max_cell: 5,
            rxlev_access_min: 3,
            access_parameter: 7,
            radio_dl_timeout: 3,
            cck_id: None,
            hyperframe_number: Some(0),
            option_field: SysinfoOptFieldFlag::DefaultDefForAccCodeA,
            ts_common_frames: None,
            default_access_code: Some(def_access),
            ext_services: None,
        };

        let sysinfo2 = MacSysinfo {
            main_carrier: sysinfo1.main_carrier,
            freq_band: sysinfo1.freq_band,
            freq_offset_index: sysinfo1.freq_offset_index,
            duplex_spacing: sysinfo1.duplex_spacing,
            reverse_operation: sysinfo1.reverse_operation,
            num_of_csch: sysinfo1.num_of_csch,
            ms_txpwr_max_cell: sysinfo1.ms_txpwr_max_cell,
            rxlev_access_min: sysinfo1.rxlev_access_min,
            access_parameter: sysinfo1.access_parameter,
            radio_dl_timeout: sysinfo1.radio_dl_timeout,
            cck_id: sysinfo1.cck_id,
            hyperframe_number: sysinfo1.hyperframe_number,
            option_field: SysinfoOptFieldFlag::ExtServicesBroadcast,
            ts_common_frames: None,
            default_access_code: None,
            ext_services: Some(ext_services),
        };

        let mle_sysinfo_pdu = DMleSysinfo {
            location_area: 2,
            subscriber_class: 65535, // All subscriber classes allowed
            bs_service_details: BsServiceDetails {
                registration: true,
                deregistration: true,
                priority_cell: false,
                no_minimum_mode: true,
                migration: false,
                system_wide_services: true,
                voice_service: true,
                circuit_mode_data_service: false,
                sndcp_service: false,
                aie_service: false,
                advanced_link: false,
            },
        };

        let mac_sync_pdu = MacSync {
            system_code: 1,
            colour_code: 1,
            time: TdmaTime::default(),
            sharing_mode: 0, // Continuous transmission
            ts_reserved_frames: 0,
            u_plane_dtx: false,
            frame_18_ext: false,
        };

        let mle_sync_pdu = DMleSync {
            mcc: 204,
            mnc: 1337,
            neighbor_cell_broadcast: 2,
            cell_load_ca: 0,
            late_entry_supported: true,
        };

        let precomps = PrecomputedUmacPdus {
            mac_sysinfo1: sysinfo1,
            mac_sysinfo2: sysinfo2,
            mle_sysinfo: mle_sysinfo_pdu,
            mac_sync: mac_sync_pdu,
            mle_sync: mle_sync_pdu,
        };

        let mut sched = BsChannelScheduler::new(1, precomps);
        sched.set_dl_time(TdmaTime::default().add_timeslots(2));
        sched
    }

    #[test]
    fn test_tmb_broadcast_scheduler_fails_closed_without_panic() {
        let mut sched = get_testing_slotter();
        let ts = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut traffic = BitBuffer::new_autoexpand(8);
        traffic.write_bits(0b1010_1010, 8);
        traffic.seek(0);

        sched.dl_schedule_tmb(traffic, &ts);

        assert!(
            sched.dltx_queues.iter().all(Vec::is_empty),
            "unsupported raw TMB-SAP scheduling must not enqueue partial broadcast state"
        );
    }

    fn test_resource_for_issi(issi: u32, sdu_bits: usize) -> (MacResource, BitBuffer) {
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: issi,
        };
        let mut pdu = MacResource {
            fill_bits: false,
            pos_of_grant: 0,
            encryption_mode: 0,
            random_access_flag: true,
            length_ind: 0,
            addr: Some(addr),
            event_label: None,
            usage_marker: None,
            power_control_element: None,
            slot_granting_element: None,
            chan_alloc_element: None,
        };
        let sdu = BitBuffer::new(sdu_bits);
        pdu.update_len_and_fill_ind(sdu.get_len());
        (pdu, sdu)
    }

    fn test_channel_allocation_resource_for_issi(issi: u32, sdu_bits: usize) -> (MacResource, BitBuffer) {
        let (mut pdu, sdu) = test_resource_for_issi(issi, sdu_bits);
        pdu.random_access_flag = false;
        pdu.chan_alloc_element = Some(ChanAllocElement {
            alloc_type: ChanAllocType::Replace,
            ts_assigned: [false, true, false, false],
            ul_dl_assigned: UlDlAssignment::Both,
            clch_permission: true,
            cell_change_flag: false,
            carrier_num: 1001,
            ext: None,
            mon_pattern: 1,
            frame18_mon_pattern: None,
        });
        pdu.update_len_and_fill_ind(sdu.get_len());
        (pdu, sdu)
    }

    fn test_resource_for_gssi(gssi: u32, sdu_bits: usize) -> (MacResource, BitBuffer) {
        let addr = TetraAddress {
            ssi_type: SsiType::Gssi,
            ssi: gssi,
        };
        let mut pdu = MacResource {
            fill_bits: false,
            pos_of_grant: 0,
            encryption_mode: 0,
            random_access_flag: false,
            length_ind: 0,
            addr: Some(addr),
            event_label: None,
            usage_marker: None,
            power_control_element: None,
            slot_granting_element: None,
            chan_alloc_element: None,
        };
        let sdu = BitBuffer::new(sdu_bits);
        pdu.update_len_and_fill_ind(sdu.get_len());
        (pdu, sdu)
    }

    #[test]
    fn test_common_control_tma_path_queues_only_ts1() {
        let mut sched = get_testing_slotter();
        let (pdu, sdu) = test_resource_for_issi(1234, 8);

        sched.dl_enqueue_tma(pdu, sdu, None);

        // EN 300 392-2 clauses 21.4.6.5 and 23.5.2.2.7: in this single
        // carrier scheduler, normal TMA common-control signalling is queued on
        // the TS1 MCCH/SCH-F path. Assigned-channel FACCH/STCH signalling uses
        // `dl_enqueue_stealing` with an explicit traffic timeslot instead.
        assert_eq!(sched.dltx_queues[0].len(), 1);
        assert!(sched.dltx_queues[1].is_empty());
        assert!(sched.dltx_queues[2].is_empty());
        assert!(sched.dltx_queues[3].is_empty());
    }

    fn open_test_dl_circuit(sched: &mut BsChannelScheduler, ts: u8) {
        sched.create_circuit(
            Direction::Dl,
            Circuit {
                direction: Direction::Dl,
                ts,
                peer_ts: None,
                usage: 4,
                circuit_mode: tetra_saps::control::enums::circuit_mode_type::CircuitModeType::TchS,
                speech_service: Some(0),
                etee_encrypted: false,
                dl_media_source: CircuitDlMediaSource::LocalLoopback,
                active_addr: None,
                active_secondary_addrs: Vec::new(),
            },
        );
    }

    #[test]
    fn test_ts1_traffic_circuit_request_is_rejected_without_panic() {
        let mut sched = get_testing_slotter();

        open_test_dl_circuit(&mut sched, 1);
        assert!(
            !sched.circuit_is_active(Direction::Dl, 1),
            "TS1 is the MCCH/SCH-F common-control path in this scheduler, not an assigned traffic channel"
        );

        sched.create_circuit(
            Direction::Dl,
            Circuit {
                direction: Direction::Dl,
                ts: 0,
                peer_ts: None,
                usage: 4,
                circuit_mode: tetra_saps::control::enums::circuit_mode_type::CircuitModeType::TchS,
                speech_service: Some(0),
                etee_encrypted: false,
                dl_media_source: CircuitDlMediaSource::LocalLoopback,
                active_addr: None,
                active_secondary_addrs: Vec::new(),
            },
        );
        assert!(!sched.circuit_is_active(Direction::Dl, 0));

        let mut bbk = sched.generate_bbk_block(TdmaTime { t: 1, f: 2, m: 1, h: 0 }).mac_block;
        bbk.seek(0);
        let aach = AccessAssign::from_bitbuf(&mut bbk).expect("TS1 AACH should remain parseable");

        // EN 300 392-2 clauses 21.4.6.5 and 23.5.2.2.7 keep this single
        // carrier scheduler's TS1 as common-control downlink. Uplink
        // reservations may still be represented by ACCESS-ASSIGN fields, but
        // an erroneous assigned-channel voice circuit must not turn TS1 into
        // traffic or crash the BS.
        assert_eq!(aach.dl_usage, AccessAssignDlUsage::CommonControl);
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::CommonOnly);
    }

    fn assert_stch_null_block(block: &TmvUnitdataReq) {
        assert_eq!(block.logical_channel, LogicalChannel::Stch);

        let mut mac_block = block.mac_block.clone();
        mac_block.seek(0);
        let resource = MacResource::from_bitbuf(&mut mac_block).expect("STCH idle block must carry a MAC-RESOURCE");
        assert!(
            resource.is_null_pdu(),
            "EN 300 392-2 clause 23.8.5 permits C-plane Null PDUs when no uplink speech was received"
        );
    }

    #[test]
    fn test_active_traffic_slot_without_voice_uses_stch_null_not_zero_tch() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();

        let elem = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);

        assert_eq!(elem.ts, TdmaTime { t: 2, f: 1, m: 1, h: 0 });
        assert_stch_null_block(
            elem.blk1
                .as_ref()
                .expect("active idle circuit should transmit first Null half-slot"),
        );
        assert_stch_null_block(
            elem.blk2
                .as_ref()
                .expect("active idle circuit should transmit second Null half-slot"),
        );
    }

    #[test]
    fn test_facch_without_voice_replaces_second_half_with_stch_null() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);
        sched.dl_enqueue_stealing(2, BitBuffer::new(SCH_HD_CAP), TetraAddress::new(1234, SsiType::Issi), None);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();

        let elem = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);

        assert_eq!(elem.ts, TdmaTime { t: 2, f: 1, m: 1, h: 0 });
        assert_eq!(elem.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::Stch));
        assert_stch_null_block(
            elem.blk2
                .as_ref()
                .expect("FACCH without speech should Null-fill the second half-slot"),
        );
    }

    #[test]
    fn test_facch_with_voice_keeps_second_half_tch_s() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);
        sched.dl_enqueue_stealing(2, BitBuffer::new(SCH_HD_CAP), TetraAddress::new(1234, SsiType::Issi), None);
        sched.dl_schedule_tmd(2, vec![0xA5; 35]);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();

        let elem = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);

        assert_eq!(elem.ts, TdmaTime { t: 2, f: 1, m: 1, h: 0 });
        assert_eq!(elem.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::Stch));
        assert_eq!(elem.blk2.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::TchS));
    }

    #[test]
    fn test_energy_saving_defers_issi_resource_until_monitoring_window() {
        let mut sched = get_testing_slotter();
        let issi = 1234;
        let (pdu, sdu) = test_resource_for_issi(issi, 8);
        sched.dl_enqueue_tma(pdu, sdu, None);

        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(1),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let subscribers = SubscriberRegistry::new();
        assert!(
            sched
                .dl_take_prioritized_sched_item(TdmaTime { t: 1, f: 2, m: 1, h: 0 }, &subscribers, &energy_saving)
                .is_none()
        );
        assert!(
            sched
                .dl_take_prioritized_sched_item(TdmaTime { t: 1, f: 3, m: 1, h: 0 }, &subscribers, &energy_saving)
                .is_some()
        );
    }

    #[test]
    fn test_energy_saving_pending_slotgrant_reserves_ul_after_actual_grant_transmit() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });

        let issi = 1234;
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: issi,
        };
        sched.dl_enqueue_reservation_grant(1, addr, ReservationRequirement::Req1Slot);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        // EN 300 392-2 clause 23.7.6 lets an EG1 MS sleep on frame 2 here.
        // The pending grant must therefore stay queued and must not reserve
        // frame-2 UL capacity before the MS can receive the MAC-RESOURCE.
        let asleep = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(asleep.ts, TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 1, f: 2, m: 1, h: 0 }, PhyBlockNum::Both),
            None
        );

        sched.tick_start(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        sched.tick_start(TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        sched.tick_start(TdmaTime { t: 3, f: 2, m: 1, h: 0 });
        sched.tick_start(TdmaTime { t: 4, f: 2, m: 1, h: 0 });

        let listening = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(listening.ts, TdmaTime { t: 1, f: 3, m: 1, h: 0 });
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 1, f: 3, m: 1, h: 0 }, PhyBlockNum::Both),
            Some(issi)
        );

        let mut aach_block = listening.bbk.as_ref().expect("TS1 should carry AACH").mac_block.clone();
        aach_block.seek(0);
        let aach = AccessAssign::from_bitbuf(&mut aach_block).expect("pending grant AACH should parse");
        // EN 300 392-2 clauses 23.5.1.3.3 and 23.5.2.2.7: the pending grant
        // path must mark granted subslots unavailable for random access in the
        // same transmitted slot that carries the MAC-RESOURCE grant.
        assert_eq!(aach.dl_usage, AccessAssignDlUsage::CommonControl);
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::CommonOnly);
        assert_eq!(aach.f1_af1.expect("first subslot access field").base_frame_len, 0);
        assert_eq!(aach.f2_af2.expect("second subslot access field").base_frame_len, 0);

        let mut mac_block = listening.blk1.expect("listening frame should carry MAC-RESOURCE").mac_block;
        let mac_resource = MacResource::from_bitbuf(&mut mac_block).expect("pending grant should emit MAC-RESOURCE");
        let grant = mac_resource
            .slot_granting_element
            .expect("pending reservation grant should be integrated into MAC-RESOURCE");
        assert_eq!(grant.capacity_allocation, BasicSlotgrantCapAlloc::Grant1Slot);
        assert_eq!(grant.granting_delay, BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity);
    }

    #[test]
    fn test_invalid_frame_18_energy_saving_assignment_fails_open_for_pending_slotgrant() {
        for assignment in [
            EnergySavingAssignment {
                mode: 5,
                frame: Some(18),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
            EnergySavingAssignment {
                mode: 2,
                frame: Some(15),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        ] {
            let mut sched = get_testing_slotter();
            sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });

            let issi = 1234;
            let addr = TetraAddress::issi(issi);
            sched.dl_enqueue_reservation_grant(1, addr, ReservationRequirement::Req1Slot);

            let subscribers = SubscriberRegistry::new();
            let mut energy_saving = HashMap::new();
            energy_saving.insert(issi, assignment);
            assert!(
                !assignment.is_energy_economy(),
                "test vector must be a stale/external EG assignment that requires unsupported frame-18 receive"
            );

            // EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6/table 23.9 require
            // the scheduler to honour valid EG listen windows. This BS does
            // not advertise full frame-18 receive support, so stale/external
            // assignments whose start or recurrence reaches frame 18 must fail
            // open and not gate slot grants behind an unreachable listen cycle.
            let granted = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
            assert_eq!(granted.ts, TdmaTime { t: 1, f: 2, m: 1, h: 0 });
            assert_eq!(
                sched.ul_get_slot_owner(TdmaTime { t: 1, f: 2, m: 1, h: 0 }, PhyBlockNum::Both),
                Some(issi),
                "invalid EG assignment must not starve a pending reservation grant"
            );

            let mut mac_block = granted
                .blk1
                .expect("fail-open assignment should allow immediate MAC-RESOURCE grant")
                .mac_block;
            let mac_resource = MacResource::from_bitbuf(&mut mac_block).expect("pending grant should emit MAC-RESOURCE");
            let grant = mac_resource
                .slot_granting_element
                .expect("pending reservation grant should be integrated into MAC-RESOURCE");
            assert_eq!(grant.capacity_allocation, BasicSlotgrantCapAlloc::Grant1Slot);
            assert_eq!(grant.granting_delay, BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity);
        }
    }

    #[test]
    fn test_pending_slotgrant_uses_non_fixed_frame_18_schf_opportunity() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 17, m: 1, h: 0 });

        let issi = 1234;
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: issi,
        };
        sched.dl_enqueue_reservation_grant(1, addr, ReservationRequirement::Req1Slot);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();

        // EN 300 392-2 clauses 9.5.2 and 9.5.3 reserve only the fixed
        // frame-18 BSCH/BNCH positions. Other frame-18 downlink opportunities
        // may carry SCH/F signalling, so MAC-RESOURCE grants can be sent there.
        let frame18 = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(frame18.ts, TdmaTime { t: 1, f: 18, m: 1, h: 0 });
        assert!(BsChannelScheduler::frame18_can_carry_scheduled_schf(frame18.ts));
        assert_eq!(
            frame18
                .blk1
                .as_ref()
                .expect("frame 18 should carry scheduled SCH/F")
                .logical_channel,
            LogicalChannel::SchF
        );
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 1, f: 18, m: 1, h: 0 }, PhyBlockNum::Both),
            Some(issi)
        );
        assert!(
            !sched.dltx_queues[0].iter().any(
                |elem| matches!(elem, DlSchedElem::PendingGrant(pending_addr, ReservationRequirement::Req1Slot)
                    if *pending_addr == addr)
            ),
            "legal frame-18 SCH/F finalization must consume the pending grant"
        );

        let mut aach_block = frame18.bbk.as_ref().expect("frame 18 should carry AACH").mac_block.clone();
        aach_block.seek(0);
        let aach = AccessAssignFr18::from_bitbuf(&mut aach_block).expect("frame-18 AACH should parse");
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::AssignedOnly);

        let mut mac_block = frame18.blk1.expect("frame 18 should carry MAC-RESOURCE").mac_block;
        let mac_resource = MacResource::from_bitbuf(&mut mac_block).expect("pending grant should emit MAC-RESOURCE");
        let grant = mac_resource
            .slot_granting_element
            .expect("pending reservation grant should be integrated into MAC-RESOURCE");
        assert_eq!(grant.capacity_allocation, BasicSlotgrantCapAlloc::Grant1Slot);
        assert_eq!(grant.granting_delay, BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity);
    }

    #[test]
    fn test_pending_slotgrant_waits_through_fixed_frame_18_broadcast_slots() {
        for (cur, fixed_ts) in [
            (TdmaTime { t: 1, f: 18, m: 1, h: 0 }, TdmaTime { t: 2, f: 18, m: 1, h: 0 }),
            (TdmaTime { t: 3, f: 18, m: 1, h: 0 }, TdmaTime { t: 4, f: 18, m: 1, h: 0 }),
        ] {
            let mut sched = get_testing_slotter();
            sched.set_dl_time(cur);

            let addr = TetraAddress {
                ssi_type: SsiType::Issi,
                ssi: 1234,
            };
            sched.dl_enqueue_reservation_grant(fixed_ts.t, addr, ReservationRequirement::Req1Slot);

            let subscribers = SubscriberRegistry::new();
            let mut energy_saving = HashMap::new();

            // EN 300 392-2 clauses 9.5.2 and 9.5.3 protect fixed frame-18
            // broadcast positions. Do not replace BSCH/BNCH with full SCH/F.
            let fixed = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
            assert_eq!(fixed.ts, fixed_ts);
            assert!(!BsChannelScheduler::frame18_can_carry_scheduled_schf(fixed.ts));
            assert_eq!(
                fixed.blk1.expect("fixed frame-18 slot should keep default BSCH").logical_channel,
                LogicalChannel::Bsch
            );
            assert_eq!(sched.ul_get_slot_owner(fixed_ts, PhyBlockNum::Both), None);
            assert!(
                sched.dltx_queues[fixed_ts.t as usize - 1].iter().any(
                    |elem| matches!(elem, DlSchedElem::PendingGrant(pending_addr, ReservationRequirement::Req1Slot)
                        if *pending_addr == addr)
                ),
                "fixed frame-18 finalization must leave the pending grant queued"
            );
        }
    }

    #[test]
    fn test_energy_saving_pending_slotgrant_survives_when_listen_frame_has_no_ul_capacity() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 2, m: 1, h: 0 });

        let issi = 1234;
        let blocking_issi = 0x00AB_CD00;
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: issi,
        };
        sched.dl_enqueue_reservation_grant(1, addr, ReservationRequirement::Req1Slot);

        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let subscribers = SubscriberRegistry::new();
        let first_listen_ts = TdmaTime { t: 1, f: 3, m: 1, h: 0 };
        let first_opportunity = first_listen_ts.forward_to_timeslot(1);
        for dist in 0..MACSCHED_NUM_FRAMES - 1 {
            let candidate_t = first_opportunity.add_timeslots(dist as i32 * 4);
            if candidate_t.is_mandatory_clch() {
                continue;
            }
            let index = sched.ul_ts_to_sched_index(&candidate_t);
            let elem = &mut sched.ulsched[candidate_t.t as usize - 1][index];
            elem.ul1 = Some(blocking_issi);
            elem.ul2 = Some(blocking_issi);
        }

        // EN 300 392-2 23.5.2.2.7 requires the BS to account for energy
        // economy when sending a slot grant. EN 300 392-2 23.5.2.2.2 gives
        // delay 1111 as a no-capacity "wait for another slot grant" signal
        // that keeps the MS from reverting to random access while the pending
        // grant remains queued for real capacity.
        let congested = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(congested.ts, first_listen_ts);
        assert_eq!(sched.ul_get_slot_owner(first_listen_ts, PhyBlockNum::Both), Some(blocking_issi));
        let mut mac_block = congested
            .blk1
            .expect("congested EG listen window should still emit a wait-grant MAC-RESOURCE")
            .mac_block;
        let mac_resource = MacResource::from_bitbuf(&mut mac_block).expect("wait grant should emit MAC-RESOURCE");
        let wait_grant = mac_resource
            .slot_granting_element
            .expect("congested pending reservation should carry a wait slotgrant");
        assert_eq!(wait_grant.capacity_allocation, BasicSlotgrantCapAlloc::Grant1Slot);
        assert_eq!(
            wait_grant.granting_delay,
            BasicSlotgrantGrantingDelay::WaitForAnotherSlotgrantMessage
        );
        assert!(
            sched.dltx_queues[0].iter().any(
                |elem| matches!(elem, DlSchedElem::PendingGrant(pending_addr, ReservationRequirement::Req1Slot)
                    if *pending_addr == addr)
            ),
            "pending grant should be requeued instead of dropped when no UL capacity exists"
        );

        let next_listen_ts = TdmaTime { t: 1, f: 5, m: 1, h: 0 };
        let index = sched.ul_ts_to_sched_index(&next_listen_ts);
        sched.ulsched[0][index].ul1 = None;
        sched.ulsched[0][index].ul2 = None;
        sched.ulsched[0][index].usage_marker = None;

        for ts in [
            TdmaTime { t: 1, f: 3, m: 1, h: 0 },
            TdmaTime { t: 2, f: 3, m: 1, h: 0 },
            TdmaTime { t: 3, f: 3, m: 1, h: 0 },
            TdmaTime { t: 4, f: 3, m: 1, h: 0 },
            TdmaTime { t: 1, f: 4, m: 1, h: 0 },
            TdmaTime { t: 2, f: 4, m: 1, h: 0 },
            TdmaTime { t: 3, f: 4, m: 1, h: 0 },
            TdmaTime { t: 4, f: 4, m: 1, h: 0 },
        ] {
            sched.tick_start(ts);
        }

        let delivered = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(delivered.ts, next_listen_ts);
        assert_eq!(sched.ul_get_slot_owner(next_listen_ts, PhyBlockNum::Both), Some(issi));

        let mut mac_block = delivered.blk1.expect("requeued pending grant should emit MAC-RESOURCE").mac_block;
        let mac_resource = MacResource::from_bitbuf(&mut mac_block).expect("pending grant should emit MAC-RESOURCE");
        let grant = mac_resource
            .slot_granting_element
            .expect("pending reservation grant should be integrated into MAC-RESOURCE after capacity frees");
        assert_eq!(grant.capacity_allocation, BasicSlotgrantCapAlloc::Grant1Slot);
        assert_eq!(grant.granting_delay, BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity);
    }

    #[test]
    fn test_energy_saving_marks_t210_after_actual_downlink_transmit() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });

        let issi = 1234;
        let (pdu, sdu) = test_resource_for_issi(issi, 8);
        sched.dl_enqueue_tma(pdu, sdu, None);

        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            issi,
            EnergySavingAssignment {
                mode: 5,
                frame: Some(2),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let subscribers = SubscriberRegistry::new();
        let elem = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(elem.ts, TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        assert!(elem.blk1.is_some());

        let assignment = energy_saving.get(&issi).expect("assignment must remain present");
        assert_eq!(assignment.awake_until, Some(TdmaTime { t: 1, f: 2, m: 2, h: 0 }));
    }

    #[test]
    fn test_energy_saving_marks_t210_after_facch_issi_transmit() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let issi = 1234;
        sched.dl_enqueue_stealing(2, BitBuffer::new(124), TetraAddress::new(issi, SsiType::Issi), None);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            issi,
            EnergySavingAssignment {
                mode: 5,
                frame: Some(2),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let elem = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(elem.ts, TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        assert_eq!(elem.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::Stch));

        // EN 300 392-2 clause 23.7.6 keeps an EG MS awake for T.210 after it
        // receives TMA-SAP signalling from the BS for one of its valid addresses.
        assert_eq!(
            energy_saving.get(&issi).and_then(|assignment| assignment.awake_until),
            Some(TdmaTime { t: 2, f: 2, m: 2, h: 0 })
        );
    }

    #[test]
    fn test_energy_saving_defers_facch_issi_stealing_until_monitoring_window() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let issi = 1234;
        let reporter = TxReporter::new_unacked();
        sched.dl_enqueue_stealing(
            2,
            BitBuffer::new(124),
            TetraAddress::new(issi, SsiType::Issi),
            Some(reporter.clone()),
        );

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let asleep = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(asleep.ts, TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        assert_stch_null_block(
            asleep
                .blk1
                .as_ref()
                .expect("idle assigned channel should carry first Null half-slot"),
        );
        assert_stch_null_block(
            asleep
                .blk2
                .as_ref()
                .expect("idle assigned channel should carry second Null half-slot"),
        );
        assert_eq!(reporter.get_state(), TxState::Pending);
        assert_eq!(energy_saving.get(&issi).and_then(|assignment| assignment.awake_until), None);
        assert!(
            sched.dltx_queues[1]
                .iter()
                .any(|elem| matches!(elem, DlSchedElem::Stealing(_, addr, _, _) if addr.ssi == issi && addr.ssi_type == SsiType::Issi)),
            "FACCH/STCH item should remain queued while the addressed MS is asleep"
        );

        for ts in [
            TdmaTime { t: 2, f: 2, m: 1, h: 0 },
            TdmaTime { t: 3, f: 2, m: 1, h: 0 },
            TdmaTime { t: 4, f: 2, m: 1, h: 0 },
            TdmaTime { t: 1, f: 3, m: 1, h: 0 },
        ] {
            sched.tick_start(ts);
        }

        let listening = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(listening.ts, TdmaTime { t: 2, f: 3, m: 1, h: 0 });
        assert_eq!(
            listening.blk1.as_ref().map(|block| block.logical_channel),
            Some(LogicalChannel::Stch)
        );
        assert_eq!(reporter.get_state(), TxState::Transmitted);
        assert_eq!(
            energy_saving.get(&issi).and_then(|assignment| assignment.awake_until),
            Some(TdmaTime { t: 2, f: 3, m: 2, h: 0 })
        );
    }

    #[test]
    fn test_energy_saving_marks_t210_after_facch_gssi_transmit() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let gssi = 91;
        let first_issi = 1001;
        let second_issi = 1002;
        sched.dl_enqueue_stealing(2, BitBuffer::new(124), TetraAddress::new(gssi, SsiType::Gssi), None);

        let mut subscribers = SubscriberRegistry::new();
        subscribers.register(first_issi);
        subscribers.register(second_issi);
        subscribers.affiliate(first_issi, gssi);
        subscribers.affiliate(second_issi, gssi);

        let mut energy_saving = HashMap::new();
        for issi in [first_issi, second_issi] {
            energy_saving.insert(
                issi,
                EnergySavingAssignment {
                    mode: 5,
                    frame: Some(2),
                    multiframe: Some(1),
                    awake_until: None,
                    suspension_count: 0,
                },
            );
        }

        let elem = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(elem.ts, TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        assert_eq!(elem.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::Stch));
        for issi in [first_issi, second_issi] {
            assert_eq!(
                energy_saving.get(&issi).and_then(|assignment| assignment.awake_until),
                Some(TdmaTime { t: 2, f: 2, m: 2, h: 0 })
            );
        }
    }

    #[test]
    fn test_energy_saving_facch_gssi_marks_t210_only_for_listening_affiliates() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let gssi = 91;
        let first_issi = 1001;
        let second_issi = 1002;
        sched.dl_enqueue_stealing(2, BitBuffer::new(124), TetraAddress::new(gssi, SsiType::Gssi), None);

        let mut subscribers = SubscriberRegistry::new();
        subscribers.register(first_issi);
        subscribers.register(second_issi);
        subscribers.affiliate(first_issi, gssi);
        subscribers.affiliate(second_issi, gssi);

        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            first_issi,
            EnergySavingAssignment {
                mode: 5,
                frame: Some(2),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
        energy_saving.insert(
            second_issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let elem = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(elem.ts, TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        assert_eq!(elem.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::Stch));
        assert_eq!(
            energy_saving.get(&first_issi).and_then(|assignment| assignment.awake_until),
            Some(TdmaTime { t: 2, f: 2, m: 2, h: 0 })
        );
        assert_eq!(
            energy_saving.get(&second_issi).and_then(|assignment| assignment.awake_until),
            None,
            "GSSI FACCH transmit must not extend T.210 for a sleeping affiliate that did not listen at this frame"
        );
    }

    #[test]
    fn test_energy_saving_repeats_facch_gssi_stealing_until_affiliates_are_covered() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let gssi = 91;
        let first_issi = 1001;
        let second_issi = 1002;
        let reporter = TxReporter::new_unacked();
        sched.dl_enqueue_stealing(
            2,
            BitBuffer::new(124),
            TetraAddress::new(gssi, SsiType::Gssi),
            Some(reporter.clone()),
        );

        let mut subscribers = SubscriberRegistry::new();
        subscribers.register(first_issi);
        subscribers.register(second_issi);
        subscribers.affiliate(first_issi, gssi);
        subscribers.affiliate(second_issi, gssi);

        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            first_issi,
            EnergySavingAssignment {
                mode: 5,
                frame: Some(2),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
        energy_saving.insert(
            second_issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let first = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(first.ts, TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        assert_eq!(first.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::Stch));
        assert_eq!(
            reporter.get_state(),
            TxState::Pending,
            "GSSI FACCH TMA report must wait until all EG affiliate batches are covered"
        );
        assert_eq!(
            energy_saving.get(&first_issi).and_then(|assignment| assignment.awake_until),
            Some(TdmaTime { t: 2, f: 2, m: 2, h: 0 })
        );
        assert_eq!(
            energy_saving.get(&second_issi).and_then(|assignment| assignment.awake_until),
            None,
            "sleeping affiliate must not get T.210 from another member's STCH batch"
        );
        assert!(
            sched.dltx_queues[1].iter().any(|elem| {
                matches!(
                    elem,
                    DlSchedElem::Stealing(_, addr, _, Some(state))
                        if addr.ssi == gssi
                            && addr.ssi_type == SsiType::Gssi
                            && state.covered == vec![first_issi]
                )
            }),
            "GSSI FACCH/STCH must remain queued after only the first EG batch is covered"
        );

        for ts in [
            TdmaTime { t: 2, f: 2, m: 1, h: 0 },
            TdmaTime { t: 3, f: 2, m: 1, h: 0 },
            TdmaTime { t: 4, f: 2, m: 1, h: 0 },
            TdmaTime { t: 1, f: 3, m: 1, h: 0 },
        ] {
            sched.tick_start(ts);
        }

        let second = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(second.ts, TdmaTime { t: 2, f: 3, m: 1, h: 0 });
        assert_eq!(second.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::Stch));
        assert_eq!(reporter.get_state(), TxState::Transmitted);
        assert_eq!(
            energy_saving.get(&second_issi).and_then(|assignment| assignment.awake_until),
            Some(TdmaTime { t: 2, f: 3, m: 2, h: 0 })
        );
        assert!(
            sched.dltx_queues[1]
                .iter()
                .all(|elem| !matches!(elem, DlSchedElem::Stealing(_, addr, _, _) if addr.ssi == gssi)),
            "GSSI FACCH/STCH should leave the queue once all affiliates are covered"
        );
    }

    #[test]
    fn test_tma_cancel_removes_requeued_gssi_facch_stealing_before_final_batch() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let gssi = 91;
        let first_issi = 1001;
        let second_issi = 1002;
        let reporter = TxReporter::new_unacked();
        sched.dl_enqueue_stealing(
            2,
            BitBuffer::new(124),
            TetraAddress::new(gssi, SsiType::Gssi),
            Some(reporter.clone()),
        );

        let mut subscribers = SubscriberRegistry::new();
        subscribers.register(first_issi);
        subscribers.register(second_issi);
        subscribers.affiliate(first_issi, gssi);
        subscribers.affiliate(second_issi, gssi);

        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            first_issi,
            EnergySavingAssignment {
                mode: 5,
                frame: Some(2),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
        energy_saving.insert(
            second_issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let first = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(first.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::Stch));
        assert_eq!(reporter.get_state(), TxState::Pending);

        // EN 300 392-2 clause 20.4.1.1.1: TMA-CANCEL can still remove a
        // submitted TMA-UNITDATA while the GSSI FACCH transfer is only
        // partially covered and no final TMA-REPORT is due.
        assert_eq!(sched.dl_cancel_by_reporter(&reporter), 1);
        assert_eq!(reporter.get_state(), TxState::Discarded);
        assert!(
            sched.dltx_queues[1]
                .iter()
                .all(|elem| !matches!(elem, DlSchedElem::Stealing(_, addr, _, _) if addr.ssi == gssi)),
            "cancel must remove the requeued GSSI FACCH/STCH final batch"
        );
    }

    #[test]
    fn test_energy_saving_facch_broadcast_gssi_does_not_mark_t210() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let issi = 1001;
        sched.dl_enqueue_stealing(
            2,
            BitBuffer::new(124),
            TetraAddress::new(PREDEFINED_BROADCAST_GSSI, SsiType::Gssi),
            None,
        );

        let mut subscribers = SubscriberRegistry::new();
        subscribers.register(issi);

        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            issi,
            EnergySavingAssignment {
                mode: 5,
                frame: Some(2),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let elem = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(elem.ts, TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        assert_eq!(elem.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::Stch));

        // EN 300 392-2 clause 23.7.6 excludes the predefined broadcast group
        // address (all ones) from sleep-cycle suspension.
        assert_eq!(energy_saving.get(&issi).and_then(|assignment| assignment.awake_until), None);
    }

    #[test]
    fn test_facch_all_ones_broadcast_repeats_without_t210_until_registered_eg_targets_covered() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let first_issi = 1001;
        let second_issi = 1002;
        let reporter = TxReporter::new_unacked();
        sched.dl_enqueue_stealing(
            2,
            BitBuffer::new(124),
            TetraAddress::new(PREDEFINED_BROADCAST_GSSI, SsiType::Gssi),
            Some(reporter.clone()),
        );

        let mut subscribers = SubscriberRegistry::new();
        subscribers.register(first_issi);
        subscribers.register(second_issi);

        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            first_issi,
            EnergySavingAssignment {
                mode: 5,
                frame: Some(2),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
        energy_saving.insert(
            second_issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let first = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(first.ts, TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        assert_eq!(first.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::Stch));
        assert_eq!(reporter.get_state(), TxState::Pending);
        assert_eq!(
            energy_saving.get(&first_issi).and_then(|assignment| assignment.awake_until),
            None,
            "EN 300 392-2 23.7.6 excludes all-ones GSSI from T.210 suspension"
        );
        assert_eq!(energy_saving.get(&second_issi).and_then(|assignment| assignment.awake_until), None);

        for ts in [
            TdmaTime { t: 2, f: 2, m: 1, h: 0 },
            TdmaTime { t: 3, f: 2, m: 1, h: 0 },
            TdmaTime { t: 4, f: 2, m: 1, h: 0 },
            TdmaTime { t: 1, f: 3, m: 1, h: 0 },
        ] {
            sched.tick_start(ts);
        }

        let second = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(second.ts, TdmaTime { t: 2, f: 3, m: 1, h: 0 });
        assert_eq!(second.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::Stch));
        assert_eq!(reporter.get_state(), TxState::Transmitted);
        assert_eq!(energy_saving.get(&first_issi).and_then(|assignment| assignment.awake_until), None);
        assert_eq!(energy_saving.get(&second_issi).and_then(|assignment| assignment.awake_until), None);
    }

    #[test]
    fn test_facch_all_ones_prunes_deregistered_target_before_repeat() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let first_issi = 1001;
        let second_issi = 1002;
        let reporter = TxReporter::new_unacked();
        sched.dl_enqueue_stealing(
            2,
            BitBuffer::new(124),
            TetraAddress::new(PREDEFINED_BROADCAST_GSSI, SsiType::Gssi),
            Some(reporter.clone()),
        );

        let mut subscribers = SubscriberRegistry::new();
        subscribers.register(first_issi);
        subscribers.register(second_issi);

        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            first_issi,
            EnergySavingAssignment {
                mode: 5,
                frame: Some(2),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
        energy_saving.insert(
            second_issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let first = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(first.ts, TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        assert_eq!(first.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::Stch));
        assert_eq!(reporter.get_state(), TxState::Pending);

        subscribers.deregister(second_issi);

        for ts in [
            TdmaTime { t: 2, f: 2, m: 1, h: 0 },
            TdmaTime { t: 3, f: 2, m: 1, h: 0 },
            TdmaTime { t: 4, f: 2, m: 1, h: 0 },
            TdmaTime { t: 1, f: 3, m: 1, h: 0 },
        ] {
            sched.tick_start(ts);
        }

        // EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6 require downlink
        // delivery to follow current valid addresses and EG receive windows.
        // Once the second MS deregisters, the all-ones repeat is already
        // covered for every currently registered target and must not wait for
        // the stale snapshot.
        let second = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(second.ts, TdmaTime { t: 2, f: 3, m: 1, h: 0 });
        assert_stch_null_block(
            second
                .blk1
                .as_ref()
                .expect("idle assigned channel should carry first Null half-slot"),
        );
        assert_stch_null_block(
            second
                .blk2
                .as_ref()
                .expect("idle assigned channel should carry second Null half-slot"),
        );
        assert_eq!(reporter.get_state(), TxState::Transmitted);
        assert!(
            sched.dltx_queues[1]
                .iter()
                .all(|elem| !matches!(elem, DlSchedElem::Stealing(_, addr, _, _) if addr.ssi == PREDEFINED_BROADCAST_GSSI)),
            "all-ones FACCH/STCH must not remain queued for a deregistered target"
        );
    }

    #[test]
    fn test_energy_saving_repeats_gssi_resource_until_affiliated_members_are_covered() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });

        let gssi = 91;
        let first_issi = 1001;
        let second_issi = 1002;
        let (pdu, sdu) = test_resource_for_gssi(gssi, 8);
        sched.dl_enqueue_tma(pdu, sdu, None);

        let mut subscribers = SubscriberRegistry::new();
        subscribers.register(first_issi);
        subscribers.register(second_issi);
        subscribers.affiliate(first_issi, gssi);
        subscribers.affiliate(second_issi, gssi);

        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            first_issi,
            EnergySavingAssignment {
                mode: 5,
                frame: Some(2),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
        energy_saving.insert(
            second_issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let first = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(first.ts, TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        assert_eq!(
            energy_saving.get(&first_issi).and_then(|a| a.awake_until),
            Some(TdmaTime { t: 1, f: 2, m: 2, h: 0 })
        );
        assert_eq!(energy_saving.get(&second_issi).and_then(|a| a.awake_until), None);

        sched.tick_start(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        sched.tick_start(TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        sched.tick_start(TdmaTime { t: 3, f: 2, m: 1, h: 0 });
        sched.tick_start(TdmaTime { t: 4, f: 2, m: 1, h: 0 });
        let second = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(second.ts, TdmaTime { t: 1, f: 3, m: 1, h: 0 });
        assert_eq!(
            energy_saving.get(&second_issi).and_then(|a| a.awake_until),
            Some(TdmaTime { t: 1, f: 3, m: 2, h: 0 })
        );
    }

    #[test]
    fn test_gssi_resource_prunes_deaffiliated_target_before_final_batch() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });

        let gssi = 91;
        let first_issi = 1001;
        let second_issi = 1002;
        let reporter = TxReporter::new_unacked();
        let (pdu, sdu) = test_resource_for_gssi(gssi, 8);
        sched.dl_enqueue_tma(pdu, sdu, Some(reporter.clone()));

        let mut subscribers = SubscriberRegistry::new();
        subscribers.register(first_issi);
        subscribers.register(second_issi);
        subscribers.affiliate(first_issi, gssi);
        subscribers.affiliate(second_issi, gssi);

        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            first_issi,
            EnergySavingAssignment {
                mode: 5,
                frame: Some(2),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
        energy_saving.insert(
            second_issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let first = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(first.ts, TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        assert_eq!(reporter.get_state(), TxState::Pending);

        subscribers.deaffiliate(second_issi, gssi);

        // EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6 bind repeated GSSI
        // delivery to current affiliates' receive opportunities. A removed
        // affiliate must not keep a completed local transfer pending.
        assert!(
            sched
                .dl_take_prioritized_sched_item(TdmaTime { t: 1, f: 3, m: 1, h: 0 }, &subscribers, &energy_saving)
                .is_none(),
            "GSSI resource repeat should be pruned once all current affiliates are already covered"
        );
        assert_eq!(reporter.get_state(), TxState::Transmitted);
        assert!(
            sched.dltx_queues[0].iter().all(|elem| {
                !matches!(
                    elem,
                    DlSchedElem::Resource(
                        MacResource {
                            addr: Some(TetraAddress {
                                ssi,
                                ssi_type: SsiType::Gssi
                            }),
                            ..
                        },
                        _,
                        _,
                        _
                    ) if *ssi == gssi
                )
            }),
            "GSSI resource must not remain queued for a deaffiliated target"
        );
    }

    #[test]
    fn test_grant_integration_keeps_issi_and_gssi_resources_separate() {
        let mut sched = get_testing_slotter();
        let ts = TdmaTime::default();
        let issi_addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };
        let gssi_addr = TetraAddress {
            ssi_type: SsiType::Gssi,
            ssi: 1234,
        };
        let (gssi_pdu, gssi_sdu) = test_resource_for_gssi(gssi_addr.ssi, 8);
        sched.dl_enqueue_tma(gssi_pdu, gssi_sdu, None);

        let grant = BasicSlotgrant {
            capacity_allocation: BasicSlotgrantCapAlloc::FirstSubslotGranted,
            granting_delay: BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity,
        };
        sched.dl_enqueue_grant(ts.t, issi_addr, grant, None);
        sched.dl_enqueue_random_access_ack(ts.t, issi_addr);

        let subscribers = SubscriberRegistry::new();
        let energy_saving = HashMap::new();
        sched.dl_integrate_sched_elems_for_timeslot(ts, &subscribers, &energy_saving);

        assert_eq!(sched.dltx_queues[ts.t as usize - 1].len(), 2);
        assert!(
            sched.dltx_queues[ts.t as usize - 1].iter().any(|elem| {
                matches!(
                    elem,
                    DlSchedElem::Resource(
                        MacResource {
                            addr: Some(TetraAddress {
                                ssi: 1234,
                                ssi_type: SsiType::Issi
                            }),
                            ..
                        },
                        _,
                        _,
                        _
                    )
                )
            }),
            "ISSI grant/RA ack must integrate into an ISSI-addressed resource"
        );
        assert!(
            sched.dltx_queues[ts.t as usize - 1].iter().any(|elem| {
                matches!(
                    elem,
                    DlSchedElem::Resource(
                        MacResource {
                            addr: Some(TetraAddress {
                                ssi: 1234,
                                ssi_type: SsiType::Gssi
                            }),
                            ..
                        },
                        _,
                        _,
                        _
                    )
                )
            }),
            "same numeric GSSI resource must not absorb ISSI grant/RA ack"
        );
    }

    #[test]
    fn test_all_ones_broadcast_fragments_wait_for_entire_active_eg_batch() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 2, m: 1, h: 0 });

        let first_issi = 1001;
        let second_issi = 1002;
        let (pdu, sdu) = test_resource_for_gssi(PREDEFINED_BROADCAST_GSSI, 600);
        sched.dl_enqueue_tma(pdu, sdu, None);

        let mut subscribers = SubscriberRegistry::new();
        subscribers.register(first_issi);
        subscribers.register(second_issi);

        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            first_issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
        energy_saving.insert(
            second_issi,
            EnergySavingAssignment {
                mode: 3,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );
        assert!(energy_saving.get(&first_issi).copied().unwrap().is_energy_economy());
        assert!(energy_saving.get(&second_issi).copied().unwrap().is_energy_economy());

        let first = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(first.ts, TdmaTime { t: 1, f: 3, m: 1, h: 0 });
        assert!(first.blk1.is_some());
        assert_eq!(
            energy_saving.get(&first_issi).and_then(|assignment| assignment.awake_until),
            None,
            "EN 300 392-2 23.7.6 excludes all-ones broadcast from T.210 suspension"
        );
        assert_eq!(
            energy_saving.get(&second_issi).and_then(|assignment| assignment.awake_until),
            None,
            "EN 300 392-2 23.7.6 excludes all-ones broadcast from T.210 suspension"
        );

        assert!(
            sched
                .dl_take_prioritized_sched_item(TdmaTime { t: 1, f: 4, m: 1, h: 0 }, &subscribers, &energy_saving)
                .is_none(),
            "remaining broadcast fragments must not transmit outside the active EG receive frame"
        );
        assert!(
            sched
                .dl_take_prioritized_sched_item(TdmaTime { t: 1, f: 5, m: 1, h: 0 }, &subscribers, &energy_saving)
                .is_none(),
            "remaining broadcast fragments must wait until every active-batch ISSI is listening"
        );
        assert!(
            sched
                .dl_take_prioritized_sched_item(TdmaTime { t: 1, f: 9, m: 1, h: 0 }, &subscribers, &energy_saving)
                .is_some(),
            "remaining broadcast fragments should resume when the active batch shares a receive frame"
        );
    }

    #[test]
    fn test_halfslot_grants() {
        let mut sched = get_testing_slotter();
        let resreq = ReservationRequirement::Req1Subslot;
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };
        let grant1 = sched.ul_process_cap_req(1, addr, &resreq);
        tracing::info!("grant1: {:?}", grant1);
        assert!(grant1.is_some(), "ul_process_cap_req should return Some, but got None");

        sched.dump_ul_schedule(false);

        let u1 = sched.ul_get_usage(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        let u2 = sched.ul_get_usage(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let u3 = sched.ul_get_usage(TdmaTime { t: 1, f: 3, m: 1, h: 0 });
        tracing::info!("usage ts 1/2/3: {:?}/{:?}/{:?}", u1, u2, u3);

        let cap_alloc1 = grant1.unwrap().0.capacity_allocation;
        assert_eq!(
            cap_alloc1,
            BasicSlotgrantCapAlloc::FirstSubslotGranted,
            "ul_process_cap_req should return FirstSubslotGranted, but got {:?}",
            cap_alloc1
        );
        let grant2 = sched.ul_process_cap_req(1, addr, &resreq);
        tracing::info!("grant2: {:?}", grant2);
        assert!(grant2.is_some(), "ul_process_cap_req should return Some, but got None");
        let cap_alloc2 = grant2.unwrap().0.capacity_allocation;
        assert_eq!(
            cap_alloc2,
            BasicSlotgrantCapAlloc::SecondSubslotGranted,
            "ul_process_cap_req should return SecondSubslotGranted, but got {:?}",
            cap_alloc2
        );

        sched.dump_ul_schedule(false);

        let u1 = sched.ul_get_usage(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        let u2 = sched.ul_get_usage(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let u3 = sched.ul_get_usage(TdmaTime { t: 1, f: 3, m: 1, h: 0 });
        tracing::info!("usage ts 1/2/3: {:?}/{:?}/{:?}", u1, u2, u3);

        sched.dump_ul_schedule(false);
    }

    #[test]
    fn test_halfslot_and_fullslot_grant() {
        let mut sched = get_testing_slotter();
        let resreq1 = ReservationRequirement::Req1Subslot;
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        sched.dump_ul_schedule(true);
        let grant1 = sched.ul_process_cap_req(1, addr, &resreq1);
        tracing::info!("grant1: {:?}", grant1);

        let u1 = sched.ul_get_usage(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        let u2 = sched.ul_get_usage(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let u3 = sched.ul_get_usage(TdmaTime { t: 1, f: 3, m: 1, h: 0 });
        tracing::info!("usage ts 1/2/3: {:?}/{:?}/{:?}", u1, u2, u3);

        assert!(grant1.is_some());
        let cap_alloc1 = grant1.unwrap().0.capacity_allocation;
        assert_eq!(cap_alloc1, BasicSlotgrantCapAlloc::FirstSubslotGranted);

        sched.dump_ul_schedule(true);
        let resreq2 = ReservationRequirement::Req3Slots;
        let Some((grant2, _marker)) = sched.ul_process_cap_req(1, addr, &resreq2) else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        tracing::info!("grant2: {:?}", grant2);
        sched.dump_ul_schedule(true);

        let u1 = sched.ul_get_usage(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        let u2 = sched.ul_get_usage(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let u3 = sched.ul_get_usage(TdmaTime { t: 1, f: 3, m: 1, h: 0 });
        tracing::info!("usage ts 1/2/3: {:?}/{:?}/{:?}", u1, u2, u3);

        assert_eq!(grant2.capacity_allocation, BasicSlotgrantCapAlloc::Grant3Slots);
        assert_eq!(grant2.granting_delay, BasicSlotgrantGrantingDelay::DelayNOpportunities(1));
    }

    #[test]
    fn test_basic_slotgrant_does_not_encode_delay_above_thirteen_opportunities() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };
        let blocker = 5678;

        for dist in 0..14 {
            let ts = TdmaTime { t: 1, f: 1, m: 1, h: 0 }.add_timeslots(dist * 4);
            let index = sched.ul_ts_to_sched_index(&ts);
            sched.ulsched[0][index].ul1 = Some(blocker);
            sched.ulsched[0][index].ul2 = Some(blocker);
        }

        // EN 300 392-2 clause 21.5.6 only defines basic slot-grant delay
        // opportunities 1..=13. Raw values 14 and 15 are special encodings,
        // not DelayNOpportunities(14/15), so the scheduler must retry later.
        let grant = sched.ul_process_cap_req_from(TdmaTime { t: 1, f: 1, m: 1, h: 0 }, 1, addr, &ReservationRequirement::Req1Slot);
        assert!(grant.is_none(), "scheduler must not encode delay opportunity > 13");

        let free_ts = TdmaTime { t: 1, f: 15, m: 1, h: 0 };
        assert_eq!(sched.ul_get_slot_owner(free_ts, PhyBlockNum::Both), None);
    }

    #[test]
    fn test_basic_slotgrant_delay_counts_mandatory_frame_18_clch_before_grant_start() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 2, f: 17, m: 1, h: 0 };
        sched.set_dl_time(grant_base);
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };
        let blocker = 5678;

        let index = sched.ul_ts_to_sched_index(&grant_base);
        sched.ulsched[1][index].ul1 = Some(blocker);
        sched.ulsched[1][index].ul2 = Some(blocker);

        // EN 300 392-2 23.5.2.2.2 counts successive delay opportunities,
        // including frame-18 predefined common-linearization opportunities.
        // Clause 23.5.2.2.7 requires reserved access to jump over those slots,
        // so the first free post-boundary slot is encoded as delay 2.
        let (grant, _) = sched
            .ul_process_cap_req_from(grant_base, 2, addr, &ReservationRequirement::Req1Slot)
            .expect("frame-1 grant after frame-18 skip should be encodable");
        assert_eq!(grant.granting_delay, BasicSlotgrantGrantingDelay::DelayNOpportunities(2));
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 2, f: 1, m: 2, h: 0 }, PhyBlockNum::Both),
            Some(addr.ssi)
        );
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 2, f: 18, m: 1, h: 0 }, PhyBlockNum::Both),
            None
        );
    }

    #[test]
    fn test_basic_slotgrant_frame_18_count_can_push_delay_above_thirteen() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 2, f: 17, m: 1, h: 0 };
        sched.set_dl_time(grant_base);
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };
        let blocker = 5678;

        for ts in [grant_base].into_iter().chain((1..=12).map(|f| TdmaTime { t: 2, f, m: 2, h: 0 })) {
            let index = sched.ul_ts_to_sched_index(&ts);
            sched.ulsched[1][index].ul1 = Some(blocker);
            sched.ulsched[1][index].ul2 = Some(blocker);
        }

        // Occupied frame 17 + counted mandatory frame-18 CLCH + occupied
        // frames 1..12 means the first free slot would require delay 14. Raw
        // grant delay values 14/15 are special meanings, so defer instead.
        let grant = sched.ul_process_cap_req_from(grant_base, 2, addr, &ReservationRequirement::Req1Slot);
        assert!(
            grant.is_none(),
            "scheduler must include mandatory frame-18 CLCH when enforcing the >13 delay guard"
        );
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 2, f: 13, m: 2, h: 0 }, PhyBlockNum::Both),
            None
        );
    }

    #[test]
    fn test_basic_slotgrant_multislot_uses_non_clch_frame_18_reserved_access() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 1, f: 16, m: 1, h: 0 };
        sched.set_dl_time(grant_base);
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        let (grant, marker) = sched
            .ul_process_cap_req_from(grant_base, 1, addr, &ReservationRequirement::Req4Slots)
            .expect("non-CLCH frame 18 should be usable reserved uplink access");

        assert_eq!(grant.capacity_allocation, BasicSlotgrantCapAlloc::Grant4Slots);
        assert_eq!(grant.granting_delay, BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity);
        assert_eq!(marker, Some(4));
        for ts in [
            TdmaTime { t: 1, f: 16, m: 1, h: 0 },
            TdmaTime { t: 1, f: 17, m: 1, h: 0 },
            TdmaTime { t: 1, f: 18, m: 1, h: 0 },
            TdmaTime { t: 1, f: 1, m: 2, h: 0 },
        ] {
            assert_eq!(sched.ul_get_slot_owner(ts, PhyBlockNum::Both), Some(addr.ssi));
        }
    }

    #[test]
    fn test_basic_slotgrant_multislot_jumps_mandatory_frame_18_clch() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 2, f: 16, m: 1, h: 0 };
        sched.set_dl_time(grant_base);
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        let (grant, marker) = sched
            .ul_process_cap_req_from(grant_base, 2, addr, &ReservationRequirement::Req4Slots)
            .expect("mandatory CLCH should be jumped inside a multi-slot grant");

        assert_eq!(grant.capacity_allocation, BasicSlotgrantCapAlloc::Grant4Slots);
        assert_eq!(grant.granting_delay, BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity);
        assert_eq!(marker, Some(4));
        for ts in [
            TdmaTime { t: 2, f: 16, m: 1, h: 0 },
            TdmaTime { t: 2, f: 17, m: 1, h: 0 },
            TdmaTime { t: 2, f: 1, m: 2, h: 0 },
            TdmaTime { t: 2, f: 2, m: 2, h: 0 },
        ] {
            assert_eq!(sched.ul_get_slot_owner(ts, PhyBlockNum::Both), Some(addr.ssi));
        }
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 2, f: 18, m: 1, h: 0 }, PhyBlockNum::Both),
            None
        );
    }

    #[test]
    fn test_frame_18_aach_advertises_reserved_uplink_usage_marker() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 1, f: 16, m: 1, h: 0 };
        sched.set_dl_time(grant_base);
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        let (_grant, marker) = sched
            .ul_process_cap_req_from(grant_base, 1, addr, &ReservationRequirement::Req4Slots)
            .expect("grant should reserve a non-CLCH frame-18 slot");
        let marker = marker.expect("multi-slot reservation should get a usage marker");

        let mut bbk = sched.generate_bbk_block(TdmaTime { t: 1, f: 18, m: 1, h: 0 }).mac_block;
        bbk.seek(0);
        let aach = AccessAssignFr18::from_bitbuf(&mut bbk).expect("frame-18 AACH should parse");

        // EN 300 392-2 clause 23.5.2.2.7 says the BS should mark granted
        // subslots as reserved in ACCESS-ASSIGN. On frame 18, clause 21.4.7.2
        // carries that as uplink access rights only.
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::Traffic(marker));
    }

    #[test]
    fn test_frame_18_aach_marks_full_slot_grant_as_reserved_for_random_access() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 1, f: 18, m: 1, h: 0 };
        sched.set_dl_time(grant_base);
        assert!(!grant_base.is_mandatory_clch());
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        sched
            .ul_process_cap_req_from(grant_base, 1, addr, &ReservationRequirement::Req1Slot)
            .expect("full-slot grant should reserve frame-18 TS1 uplink");

        let mut bbk = sched.generate_bbk_block(grant_base).mac_block;
        bbk.seek(0);
        let aach = AccessAssignFr18::from_bitbuf(&mut bbk).expect("frame-18 AACH should parse");

        // EN 300 392-2 clause 23.5.2.2.7: after granting one slot, the BS
        // should mark the two equivalent uplink subslots as reserved in
        // ACCESS-ASSIGN. Clause 21.5.1 encodes a reserved subslot as
        // base-frame-length 0.
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::AssignedOnly);
        assert_eq!(aach.f1_af1.expect("first subslot access field").base_frame_len, 0);
        assert_eq!(aach.f2_af2.expect("second subslot access field").base_frame_len, 0);
    }

    #[test]
    fn test_frame_18_aach_marks_first_subslot_grant_as_reserved_for_random_access() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 1, f: 18, m: 1, h: 0 };
        sched.set_dl_time(grant_base);
        assert!(!grant_base.is_mandatory_clch());
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        sched
            .ul_process_cap_req_from(grant_base, 1, addr, &ReservationRequirement::Req1Subslot)
            .expect("subslot grant should reserve frame-18 TS1 first uplink subslot");

        let mut bbk = sched.generate_bbk_block(grant_base).mac_block;
        bbk.seek(0);
        let aach = AccessAssignFr18::from_bitbuf(&mut bbk).expect("frame-18 AACH should parse");

        assert_eq!(aach.ul_usage, AccessAssignUlUsage::CommonAndAssigned);
        assert_eq!(aach.f1_af1.expect("first subslot access field").base_frame_len, 0);
        assert_eq!(aach.f2_af2.expect("second subslot access field").base_frame_len, 4);
    }

    #[test]
    fn test_ts1_aach_marks_full_slot_grant_as_reserved_for_random_access() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        sched
            .ul_process_cap_req_from(grant_base, 1, addr, &ReservationRequirement::Req1Slot)
            .expect("full-slot grant should reserve TS1 uplink");

        let mut bbk = sched.generate_bbk_block(grant_base).mac_block;
        bbk.seek(0);
        let aach = AccessAssign::from_bitbuf(&mut bbk).expect("TS1 AACH should parse");

        // EN 300 392-2 clauses 23.5.1.3.3 and 23.5.2.2.7: granted uplink
        // capacity must be advertised as unavailable for random access.
        assert_eq!(aach.dl_usage, AccessAssignDlUsage::CommonControl);
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::CommonOnly);
        assert_eq!(aach.f1_af1.expect("first subslot access field").base_frame_len, 0);
        assert_eq!(aach.f2_af2.expect("second subslot access field").base_frame_len, 0);
    }

    #[test]
    fn test_ts1_aach_marks_first_subslot_grant_as_reserved_for_random_access() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        sched
            .ul_process_cap_req_from(grant_base, 1, addr, &ReservationRequirement::Req1Subslot)
            .expect("first subslot grant should reserve TS1 uplink");

        let mut bbk = sched.generate_bbk_block(grant_base).mac_block;
        bbk.seek(0);
        let aach = AccessAssign::from_bitbuf(&mut bbk).expect("TS1 AACH should parse");

        assert_eq!(aach.dl_usage, AccessAssignDlUsage::CommonControl);
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::CommonOnly);
        assert_eq!(aach.f1_af1.expect("first subslot access field").base_frame_len, 0);
        assert_eq!(aach.f2_af2.expect("second subslot access field").base_frame_len, 4);
    }

    #[test]
    fn test_ts1_aach_marks_second_subslot_grant_as_reserved_for_random_access() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        sched
            .ul_process_cap_req_from(grant_base, 1, addr, &ReservationRequirement::Req1Subslot)
            .expect("first subslot grant should reserve TS1 uplink");
        sched
            .ul_process_cap_req_from(grant_base, 1, addr, &ReservationRequirement::Req1Subslot)
            .expect("second subslot grant should reserve remaining TS1 uplink");

        let mut bbk = sched.generate_bbk_block(grant_base).mac_block;
        bbk.seek(0);
        let aach = AccessAssign::from_bitbuf(&mut bbk).expect("TS1 AACH should parse");

        assert_eq!(aach.dl_usage, AccessAssignDlUsage::CommonControl);
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::CommonOnly);
        assert_eq!(aach.f1_af1.expect("first subslot access field").base_frame_len, 0);
        assert_eq!(aach.f2_af2.expect("second subslot access field").base_frame_len, 0);
    }

    #[test]
    fn test_dl_grant_and_ack_integration() {
        let mut sched = get_testing_slotter();
        let ts = TdmaTime::default();
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&addr, None, false);
        let sdu = BitBuffer::new(0);
        sched.dl_enqueue_tma(pdu, sdu, None);

        let grant = BasicSlotgrant {
            capacity_allocation: BasicSlotgrantCapAlloc::FirstSubslotGranted,
            granting_delay: BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity,
        };

        sched.dl_enqueue_grant(ts.t, addr, grant.clone(), None);
        sched.dl_enqueue_random_access_ack(ts.t, addr);

        sched.dump_ul_schedule(true);
        sched.dump_dl_queue();

        assert!(sched.dltx_queues[ts.t as usize - 1].len() == 3);

        tracing::info!("Integrating queue");
        let subscribers = SubscriberRegistry::new();
        let energy_saving = HashMap::new();
        sched.dl_integrate_sched_elems_for_timeslot(ts, &subscribers, &energy_saving);

        sched.dump_ul_schedule(true);
        sched.dump_dl_queue();

        assert!(sched.dltx_queues[ts.t as usize - 1].len() == 1);
        let DlSchedElem::Resource(pdu, _sdu, _reporter, _) = &sched.dltx_queues[ts.t as usize - 1][0] else {
            panic!("expected integrated resource");
        };
        assert_eq!(pdu.addr.expect("integrated resource should keep address").ssi, addr.ssi);
        assert!(pdu.random_access_flag, "explicit RandomAccessAck should set MAC-RESOURCE RA flag");
        let integrated_grant = pdu.slot_granting_element.as_ref().expect("grant should be integrated");
        assert_eq!(integrated_grant.capacity_allocation, grant.capacity_allocation);
        assert_eq!(integrated_grant.granting_delay, grant.granting_delay);
    }

    #[test]
    fn test_dl_grant_without_ack_does_not_set_random_access_flag() {
        let mut sched = get_testing_slotter();
        let ts = TdmaTime::default();
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        let grant = BasicSlotgrant {
            capacity_allocation: BasicSlotgrantCapAlloc::FirstSubslotGranted,
            granting_delay: BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity,
        };

        sched.dl_enqueue_grant(ts.t, addr, grant, None);
        let subscribers = SubscriberRegistry::new();
        let energy_saving = HashMap::new();
        sched.dl_integrate_sched_elems_for_timeslot(ts, &subscribers, &energy_saving);

        let DlSchedElem::Resource(pdu, _sdu, _reporter, _) = &sched.dltx_queues[ts.t as usize - 1][0] else {
            panic!("expected grant resource");
        };
        assert!(!pdu.random_access_flag, "slot grants alone must not acknowledge random access");
    }

    #[test]
    fn test_random_access_ack_resource_preempts_fragment_backlog() {
        let mut sched = get_testing_slotter();
        let ts = TdmaTime { t: 1, f: 2, m: 1, h: 0 };
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        let (frag_pdu, frag_sdu) = test_resource_for_issi(9999, 600);
        sched.dltx_queues[ts.t as usize - 1].push(DlSchedElem::FragBuf(BsFragger::new(frag_pdu, frag_sdu, None), None));
        sched.dl_enqueue_random_access_ack(ts.t, addr);

        let subscribers = SubscriberRegistry::new();
        let energy_saving = HashMap::new();
        sched.dl_integrate_sched_elems_for_timeslot(ts, &subscribers, &energy_saving);

        let Some(DlSchedElem::Resource(pdu, _sdu, _reporter, _group_state)) =
            sched.dl_take_prioritized_sched_item(ts, &subscribers, &energy_saving)
        else {
            panic!("integrated RA ACK resource should preempt fragment backlog");
        };
        assert_eq!(pdu.addr, Some(addr));
        assert!(
            pdu.random_access_flag,
            "EN 300 392-2 21.4.3.1 random-access ACK must transmit before ordinary fragmentation backlog"
        );
    }

    #[test]
    fn test_slotgrant_resource_preempts_fragment_backlog() {
        let mut sched = get_testing_slotter();
        let ts = TdmaTime { t: 1, f: 2, m: 1, h: 0 };
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };
        let grant = BasicSlotgrant {
            capacity_allocation: BasicSlotgrantCapAlloc::FirstSubslotGranted,
            granting_delay: BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity,
        };

        let (frag_pdu, frag_sdu) = test_resource_for_issi(9999, 600);
        sched.dltx_queues[ts.t as usize - 1].push(DlSchedElem::FragBuf(BsFragger::new(frag_pdu, frag_sdu, None), None));
        sched.dl_enqueue_grant(ts.t, addr, grant.clone(), None);

        let subscribers = SubscriberRegistry::new();
        let energy_saving = HashMap::new();
        sched.dl_integrate_sched_elems_for_timeslot(ts, &subscribers, &energy_saving);

        let Some(DlSchedElem::Resource(pdu, _sdu, _reporter, _group_state)) =
            sched.dl_take_prioritized_sched_item(ts, &subscribers, &energy_saving)
        else {
            panic!("integrated slotgrant resource should preempt fragment backlog");
        };
        assert_eq!(pdu.addr, Some(addr));
        let integrated_grant = pdu.slot_granting_element.expect("integrated resource should carry the slot grant");
        assert_eq!(integrated_grant.capacity_allocation, grant.capacity_allocation);
        assert_eq!(integrated_grant.granting_delay, grant.granting_delay);
    }

    #[test]
    fn test_private_chan_alloc_resource_not_starved_by_ready_eg_grants() {
        let mut sched = get_testing_slotter();
        let ts = TdmaTime { t: 1, f: 3, m: 1, h: 0 };
        let private_call_issi = 0x4101;
        let (chan_alloc_pdu, chan_alloc_sdu) = test_channel_allocation_resource_for_issi(private_call_issi, 16);
        sched.dl_enqueue_tma(chan_alloc_pdu, chan_alloc_sdu, None);

        let grant = BasicSlotgrant {
            capacity_allocation: BasicSlotgrantCapAlloc::FirstSubslotGranted,
            granting_delay: BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity,
        };
        let ready_eg_issis = [0x4201, 0x4202, 0x4203];
        for issi in ready_eg_issis {
            sched.dl_enqueue_grant(ts.t, TetraAddress::issi(issi), grant.clone(), None);
        }

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            private_call_issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(ts.f),
                multiframe: Some(ts.m),
                awake_until: None,
                suspension_count: 0,
            },
        );
        for issi in ready_eg_issis {
            energy_saving.insert(
                issi,
                EnergySavingAssignment {
                    mode: 1,
                    frame: Some(ts.f),
                    multiframe: Some(ts.m),
                    awake_until: None,
                    suspension_count: 0,
                },
            );
        }

        sched.dl_integrate_sched_elems_for_timeslot(ts, &subscribers, &energy_saving);

        let Some(DlSchedElem::Resource(pdu, _sdu, _reporter, _group_state)) =
            sched.dl_take_prioritized_sched_item(ts, &subscribers, &energy_saving)
        else {
            panic!("private-call channel allocation resource should be selected before ready EG grants");
        };
        assert_eq!(pdu.addr, Some(TetraAddress::issi(private_call_issi)));
        assert!(
            pdu.chan_alloc_element.is_some(),
            "EN 300 392-2 clauses 14, 21.5.2 and 23.5.2.2.7: call channel allocation must not be starved by ready EG grants"
        );
    }

    #[test]
    fn test_fragmented_private_chan_alloc_not_starved_by_ready_eg_grants() {
        let mut sched = get_testing_slotter();
        let ts = TdmaTime { t: 1, f: 3, m: 1, h: 0 };
        let private_call_issi = 0x4301;
        let (chan_alloc_pdu, chan_alloc_sdu) = test_channel_allocation_resource_for_issi(private_call_issi, 300);
        let mut fragger = BsFragger::new(chan_alloc_pdu, chan_alloc_sdu, None);
        let mut first_chunk = BitBuffer::new(SCH_F_CAP);
        assert!(
            !fragger.get_next_chunk(&mut first_chunk),
            "test vector must create a fragmented channel-allocation transfer"
        );
        assert!(
            fragger.carries_channel_allocation(),
            "EN 300 392-2 clause 23.4.2.1.1 moves fragmented channel allocation into the pending MAC-END"
        );
        sched.dltx_queues[ts.t as usize - 1].push(DlSchedElem::FragBuf(fragger, None));

        let grant = BasicSlotgrant {
            capacity_allocation: BasicSlotgrantCapAlloc::FirstSubslotGranted,
            granting_delay: BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity,
        };
        let ready_eg_issis = [0x4401, 0x4402, 0x4403];
        for issi in ready_eg_issis {
            sched.dl_enqueue_grant(ts.t, TetraAddress::issi(issi), grant.clone(), None);
        }

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            private_call_issi,
            EnergySavingAssignment {
                mode: 1,
                frame: Some(ts.f),
                multiframe: Some(ts.m),
                awake_until: None,
                suspension_count: 0,
            },
        );
        for issi in ready_eg_issis {
            energy_saving.insert(
                issi,
                EnergySavingAssignment {
                    mode: 1,
                    frame: Some(ts.f),
                    multiframe: Some(ts.m),
                    awake_until: None,
                    suspension_count: 0,
                },
            );
        }

        sched.dl_integrate_sched_elems_for_timeslot(ts, &subscribers, &energy_saving);

        let Some(DlSchedElem::FragBuf(fragger, _group_state)) = sched.dl_take_prioritized_sched_item(ts, &subscribers, &energy_saving)
        else {
            panic!("fragmented private-call channel allocation should continue before ready EG grants");
        };
        assert!(
            fragger.carries_channel_allocation(),
            "EN 300 392-2 clauses 14, 21.5.2, 23.4.2.1.1 and 23.5.2.2.7: pending MAC-END channel allocation must not be starved"
        );
    }

    #[test]
    fn test_pending_random_access_ack_distinguishes_ssi_type() {
        let mut sched = get_testing_slotter();
        let issi_addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };
        let gssi_addr = TetraAddress {
            ssi_type: SsiType::Gssi,
            ssi: 1234,
        };

        sched.dl_enqueue_random_access_ack(1, issi_addr);
        assert!(sched.dl_drop_all_except_stolen(1));

        assert!(
            !sched.take_pending_ra_ack(1, gssi_addr),
            "same numeric SSI with GSSI type must not consume an ISSI random-access ACK"
        );
        assert!(
            sched.take_pending_ra_ack(1, issi_addr),
            "original ISSI random-access ACK should still be pending"
        );
    }

    #[test]
    fn test_pending_random_access_ack_for_stch_waits_for_channel_allocation() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        sched.dl_enqueue_random_access_ack(1, addr);
        assert!(sched.dl_drop_all_except_stolen(1));

        // EN 300 392-2 clause 21.4.3.1 defines random_access_flag as the
        // successful random-access ACK. In a private-call floor transition, the
        // grant that matters is the channel-allocation D-TX GRANTED response
        // described by clauses 14.5.1.2.1 b) and 23.5.2.2.1; an intervening
        // ACK-only STCH must not consume the preserved MAC ACK.
        assert!(
            !sched.take_pending_ra_ack_for_stch(1, addr, false),
            "ACK-only STCH must leave the preserved random-access ACK pending"
        );
        assert!(
            sched.take_pending_ra_ack_for_stch(1, addr, true),
            "channel-allocation STCH should consume and carry the preserved random-access ACK"
        );
        assert!(
            !sched.take_pending_ra_ack_for_stch(1, addr, true),
            "random-access ACK should be consumed only once"
        );
    }

    #[test]
    fn test_dropped_ra_ack_with_pending_grant_does_not_leave_ack_only_stch() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        sched.dl_enqueue_random_access_ack(1, addr);
        sched.dl_enqueue_reservation_grant(1, addr, ReservationRequirement::Req1Slot);

        // EN 300 392-2 clauses 21.4.3.1 and 23.5.1.3.3 make the
        // random-access ACK and reserved uplink grant one coherent response.
        // If hangtime cleanup discards the grant, preserving the RA ACK would
        // later build an ACK-only STCH for an MS that asked for reserved
        // capacity.
        assert!(sched.dl_drop_all_except_stolen(1));

        assert!(
            !sched.take_pending_ra_ack(1, addr),
            "dropping a matching PendingGrant must not preserve an ACK-only response"
        );
        assert!(
            sched.dltx_queues[0].is_empty(),
            "hangtime cleanup should discard the queued RA ACK and PendingGrant together"
        );
    }
}
