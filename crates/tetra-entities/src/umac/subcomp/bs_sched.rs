// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::collections::{HashMap, HashSet};

use tetra_config::bluestation::{EnergySavingAssignment, SubscriberRegistry};
use tetra_core::{
    BitBuffer, Direction, PhyBlockNum, PhysicalChannel, SsiType, TdmaTime, TetraAddress, TimeslotAllocator, TimeslotOwner, Todo,
    TxReporter, unimplemented_log,
};
use tetra_saps::{
    control::call_control::{Circuit, CircuitDlMediaSource},
    lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment},
    tmv::{TmvUnitdataReq, TmvUnitdataReqSlot, enums::logical_chans::LogicalChannel},
};

use tetra_pdus::{
    cmce::{
        enums::{cmce_pdu_type_dl::CmcePduTypeDl, transmission_grant::TransmissionGrant},
        pdus::d_tx_granted::DTxGranted,
    },
    llc::enums::llc_pdu_type::LlcPduType,
    llc::pdus::{al_data::AlData, bl_adata::BlAdata, bl_data::BlData, bl_udata::BlUdata},
    mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator,
    mle::pdus::{d_mle_sync::DMleSync, d_mle_sysinfo::DMleSysinfo},
    umac::{
        enums::{
            access_assign_dl_usage::AccessAssignDlUsage, access_assign_ul_usage::AccessAssignUlUsage,
            basic_slotgrant_cap_alloc::BasicSlotgrantCapAlloc, basic_slotgrant_granting_delay::BasicSlotgrantGrantingDelay,
            reservation_requirement::ReservationRequirement,
        },
        fields::{basic_slotgrant::BasicSlotgrant, channel_allocation::ChanAllocElement},
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

// We schedule enough same-timeslot uplink opportunities to represent the
// largest ETSI basic slot grant. Table 21.95 reaches 68 slots, and reserved
// access must jump mandatory frame-18 CLCH opportunities, so one multiframe is
// not enough for large WAP/AL fragments.
pub const MACSCHED_NUM_FRAMES: usize = 90;

const NULL_PDU_LEN_BITS: usize = 16;

pub const SCH_HD_CAP: usize = 124;
pub const SCH_F_CAP: usize = 268;
pub const TCH_S_CAP: usize = 274;

/// Number of timeslots the scheduler operates on. May become larger when secondary carriers are supported.
pub const NUM_TIMESLOTS: usize = 4;

const MAX_RESERVED_ACCESS_GRANT_CHUNK_SLOTS: usize = 4;
const BASIC_GRANT_FULL_SLOT_COUNTS_DESC: &[usize] = &[68, 51, 34, 24, 17, 13, 10, 8, 6, 5, 4, 3, 2, 1];
const PREDEFINED_BROADCAST_GSSI: u32 = 0xFF_FFFF;
const MAX_PENDING_RA_ACKS_PER_TIMESLOT: usize = 8192;
const MAX_DLSCHED_ELEMS_PER_TIMESLOT: usize = 4096;
const MAX_DLSCHED_NEXT_SLOT_ELEMS: usize = 4096;
// PDCH assignment lifetime is controlled by explicit channel allocation updates
// (Replace/QuitAndGo) and voice pre-emption. Keep this as a stale-state
// fail-safe only; a short lease makes radios visibly lose assigned SCCH slots
// between valid SNDCP data bursts.
const PACKET_DATA_ASSIGNMENT_TTL_TIMESLOTS: i32 = 4 * 18 * 300;

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

#[derive(Debug, Clone, Copy, PartialEq)]
struct PacketDataAssignment {
    addr: TetraAddress,
    ul_dl_assigned: UlDlAssignment,
    carrier_num: u16,
    expires_at: TdmaTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PacketDataAssignmentUpdate {
    Replace {
        addr: TetraAddress,
        ts_assigned: [bool; NUM_TIMESLOTS],
        ul_dl_assigned: UlDlAssignment,
        carrier_num: u16,
    },
    Additional {
        addr: TetraAddress,
        ts_assigned: [bool; NUM_TIMESLOTS],
        ul_dl_assigned: UlDlAssignment,
        carrier_num: u16,
    },
    QuitAndGo {
        addr: TetraAddress,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingGrantDebt {
    pub res_req: ReservationRequirement,
    pub remaining_slots: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingGrantOrigin {
    CommonControl,
    PacketData { assigned_ts: u8 },
}

impl PendingGrantDebt {
    fn new(res_req: ReservationRequirement) -> Self {
        let remaining_slots = match res_req {
            ReservationRequirement::Req1Subslot => 1,
            _ => res_req.to_req_slotcount(),
        };
        Self { res_req, remaining_slots }
    }

    fn is_halfslot(self) -> bool {
        self.res_req == ReservationRequirement::Req1Subslot
    }

    fn next_chunk_slots(self) -> usize {
        if self.is_halfslot() {
            1
        } else {
            self.remaining_slots.min(MAX_RESERVED_ACCESS_GRANT_CHUNK_SLOTS)
        }
    }

    fn remaining_after_grant(self, granted_slots: usize) -> Option<Self> {
        if self.is_halfslot() || granted_slots >= self.remaining_slots {
            return None;
        }
        let remaining_slots = self.remaining_slots - granted_slots;
        Some(Self {
            res_req: ReservationRequirement::from_req_slotcount(remaining_slots),
            remaining_slots,
        })
    }

    fn wait_capacity_allocation(self) -> BasicSlotgrantCapAlloc {
        if self.is_halfslot() {
            BasicSlotgrantCapAlloc::FirstSubslotGranted
        } else {
            BasicSlotgrantCapAlloc::from_req_slotcount(self.remaining_slots)
        }
    }

    fn coalesce_max(self, other: Self) -> Self {
        if other.remaining_slots > self.remaining_slots {
            other
        } else {
            self
        }
    }
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

    /// Dynamic packet-data assigned SCCH/PDCH state for SNDCP/WAP only.
    /// This is deliberately separate from CircuitMgr so CMCE voice/simplex/
    /// private/group/SDS traffic state remains the sole owner of traffic
    /// channels.
    packet_data_assignments: [Option<PacketDataAssignment>; 4],

    /// Round-robin cursor for downlink SNDCP/AL data over active PDCH slots.
    packet_data_downlink_cursor: usize,
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
    PendingGrant(TetraAddress, PendingGrantDebt, PendingGrantOrigin),

    /// A MAC-RESOURCE PDU. May be split into fragments upon processing, in which case a FragBuf will be inserted after processing the resource.
    Resource(MacResource, BitBuffer, Option<TxReporter>, Option<GroupDeliveryState>),

    /// A FragBuf containing remaining non-transmitted information after a MAC-RESOURCE start has been transmitted
    FragBuf(BsFragger, Option<GroupDeliveryState>),

    /// Pre-built STCH block for FACCH/stealing a half-slot from traffic channel.
    /// Contains MAC-U-SIGNAL (3 bits) + TM-SDU = 124 type1 bits.
    /// Delivers time-critical signaling (D-TX CEASED, D-TX GRANTED) per EN 300 392-2, clause 23.5.
    Stealing(BitBuffer, TetraAddress, Option<TxReporter>, Option<GroupStealingState>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum StealingSchedPriority {
    Ordinary,
    ChannelAllocation,
    CmceChannelAllocation,
    ListenerFloorGrant,
    PositiveFloorGrant,
    FloorWithdraw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DlBackpressurePriority {
    Ordinary,
    CmceCallControl,
    GrantOrAck,
    ChannelAllocation,
    ListenerFloorGrant,
    PositiveFloorGrant,
    FloorWithdraw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FloorWithdrawKey {
    addr: TetraAddress,
    call_id: u16,
    pdu_type: CmcePduTypeDl,
}

/// Delivery state for GSSI-addressed signalling while members use Energy Economy.
///
/// ETSI EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6 require the BS to account for
/// an MS's energy economy receive windows when sending downlink PDUs. A single
/// GSSI MAC-RESOURCE may be missed by sleeping affiliates, so the scheduler keeps
/// transmitting the same GSSI-addressed PDU until every affiliated ISSI with a
/// valid Energy Economy assignment has had a listening opportunity. StayAlive or
/// fail-open affiliates listen to the ordinary group-addressed transmission and
/// do not need per-member repeat state. For the predefined all-ones broadcast
/// GSSI we use the registered ISSI set for EG coverage, but we do not extend
/// T.210 because clause 23.7.6 explicitly excludes that address from sleep-cycle
/// suspension.
#[derive(Debug, Clone)]
pub struct GroupDeliveryState {
    original_pdu: MacResource,
    original_sdu: BitBuffer,
    targets: Vec<u32>,
    covered: HashSet<u32>,
    active_batch: HashSet<u32>,
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
            covered: HashSet::new(),
            active_batch: HashSet::new(),
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

    fn has_uncovered_listener(&self, ts: TdmaTime, energy_saving: &HashMap<u32, EnergySavingAssignment>) -> bool {
        self.targets
            .iter()
            .copied()
            .any(|issi| !self.covered.contains(&issi) && Self::ms_listens_at(energy_saving, issi, ts))
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
            self.active_batch = self.uncovered_listeners(ts, energy_saving).into_iter().collect();
        }
    }

    fn mark_batch_covered(&mut self) {
        for issi in self.active_batch.drain() {
            self.covered.insert(issi);
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
    covered: HashSet<u32>,
    active_batch: HashSet<u32>,
    tx_reporter: Option<TxReporter>,
    suspend_t210: bool,
}

impl GroupStealingState {
    fn new(targets: Vec<u32>, tx_reporter: Option<TxReporter>, suspend_t210: bool) -> Self {
        Self {
            targets,
            covered: HashSet::new(),
            active_batch: HashSet::new(),
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

    fn has_uncovered_listener(&self, ts: TdmaTime, energy_saving: &HashMap<u32, EnergySavingAssignment>) -> bool {
        self.targets
            .iter()
            .copied()
            .any(|issi| !self.covered.contains(&issi) && GroupDeliveryState::ms_listens_at(energy_saving, issi, ts))
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
            self.active_batch = self.uncovered_listeners(ts, energy_saving).into_iter().collect();
        }
    }

    fn mark_batch_covered(&mut self) {
        for issi in self.active_batch.drain() {
            self.covered.insert(issi);
        }
    }
}

#[derive(Debug, Default)]
struct GroupReadinessCache {
    targets_by_addr: HashMap<TetraAddress, Vec<u32>>,
    any_listens_by_addr: HashMap<TetraAddress, bool>,
}

impl GroupReadinessCache {
    fn targets_for(&mut self, addr: TetraAddress, subscribers: &SubscriberRegistry) -> &[u32] {
        self.targets_by_addr
            .entry(addr)
            .or_insert_with(|| BsChannelScheduler::group_targets(addr, subscribers))
            .as_slice()
    }

    fn any_target_listens(
        &mut self,
        addr: TetraAddress,
        ts: TdmaTime,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
        subscribers: &SubscriberRegistry,
    ) -> bool {
        if let Some(listens) = self.any_listens_by_addr.get(&addr) {
            return *listens;
        }

        let listens = {
            let targets = self.targets_for(addr, subscribers);
            targets.is_empty()
                || targets
                    .iter()
                    .copied()
                    .any(|issi| BsChannelScheduler::ms_listens_at(energy_saving, issi, ts))
        };
        self.any_listens_by_addr.insert(addr, listens);
        listens
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
            packet_data_assignments: [None, None, None, None],
            packet_data_downlink_cursor: 0,
        }
    }

    pub fn health_stats(&self) -> crate::health::UmacHealthStats {
        let dl_queue_total = self.dltx_queues.iter().map(Vec::len).sum();
        let dl_queue_max_per_ts = self.dltx_queues.iter().map(Vec::len).max().unwrap_or_default();
        let pending_ra_ack_total = self.pending_ra_acks.iter().map(Vec::len).sum();
        let pending_ra_ack_max_per_ts = self.pending_ra_acks.iter().map(Vec::len).max().unwrap_or_default();

        crate::health::UmacHealthStats {
            dl_queue_total,
            dl_queue_max_per_ts,
            next_slot_queue_len: self.dltx_next_slot_queue.len(),
            pending_ra_ack_total,
            pending_ra_ack_max_per_ts,
            ..crate::health::UmacHealthStats::default()
        }
    }

    fn dl_slot_index(ts: u8, context: &str) -> Option<usize> {
        if (1..=NUM_TIMESLOTS as u8).contains(&ts) {
            Some(ts as usize - 1)
        } else {
            tracing::warn!("{}: invalid downlink timeslot {}", context, ts);
            None
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

    pub fn clear_dl_media_queue_except_source(&mut self, ts: u8, source_ul_ts: u8, source_addr: TetraAddress, reason: &str) {
        let dropped = self.circuits.clear_tx_data_except_source(ts, source_ul_ts, source_addr);
        if dropped > 0 {
            tracing::info!(
                "BsChannelScheduler: dropped {} queued stale DL media block(s) on ts {} while preserving source {} on UL ts {}: {}",
                dropped,
                ts,
                source_addr,
                source_ul_ts,
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

    fn has_pending_group_positive_floor_grant_stealing(&self, ts: u8) -> bool {
        let Some(slot) = Self::dl_slot_index(ts, "has_pending_group_positive_floor_grant_stealing") else {
            return false;
        };
        if self.ul_circuit_is_private_participant_scoped(ts) {
            return false;
        }
        if !self.ul_circuit_primary_addr(ts).is_some_and(|addr| addr.ssi_type == SsiType::Gssi) {
            return false;
        }

        self.dltx_queues[slot].iter().any(Self::stealing_is_group_positive_floor_grant)
    }

    fn stealing_is_group_positive_floor_grant(elem: &DlSchedElem) -> bool {
        let DlSchedElem::Stealing(block, addr, _, _) = elem else {
            return false;
        };
        if addr.ssi_type != SsiType::Issi {
            return false;
        }

        let mut mac_probe = BitBuffer::from_bitbuffer(block);
        let Ok(resource) = MacResource::from_bitbuf(&mut mac_probe) else {
            return false;
        };
        let Some(chan_alloc) = resource.chan_alloc_element.as_ref() else {
            return false;
        };
        if !matches!(chan_alloc.ul_dl_assigned, UlDlAssignment::Ul | UlDlAssignment::Both) {
            return false;
        }

        let mac_payload = BitBuffer::from_bitbuffer_pos(&mac_probe);
        let Some(mut cmce_payload) = Self::cmce_dl_payload_from_tma_sdu(&mac_payload) else {
            return false;
        };
        DTxGranted::from_bitbuf(&mut cmce_payload)
            .is_ok_and(|grant| grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8)
    }

    fn log_selected_stch_d_tx_granted_diag(&self, tx_ts: TdmaTime, block: &BitBuffer, fallback_addr: TetraAddress, tch_buf_present: bool) {
        let mut mac_probe = BitBuffer::from_bitbuffer(block);
        let Ok(resource) = MacResource::from_bitbuf(&mut mac_probe) else {
            return;
        };
        let addr = resource.addr.unwrap_or(fallback_addr);
        let mac_payload = BitBuffer::from_bitbuffer_pos(&mac_probe);
        let Some(mut cmce_payload) = Self::cmce_dl_payload_from_tma_sdu(&mac_payload) else {
            return;
        };
        let Ok(grant) = DTxGranted::from_bitbuf(&mut cmce_payload) else {
            return;
        };
        if grant.transmission_grant != TransmissionGrant::Granted.into_raw() as u8 {
            return;
        }

        let priority = Self::stealing_sched_priority(&DlSchedElem::Stealing(block.clone(), fallback_addr, None, None));
        tracing::info!(
            "UMAC RF diag: selected STCH D-TX GRANTED tx_ts={} addr={} call_id={} grant={} ra_ack={} usage={:?} chan_alloc={:?} ul_primary={:?} priority={:?} tch_buf_present={}",
            tx_ts,
            addr,
            grant.call_identifier,
            grant.transmission_grant,
            resource.random_access_flag,
            resource.usage_marker,
            resource
                .chan_alloc_element
                .as_ref()
                .map(|ca| (ca.ts_assigned, ca.ul_dl_assigned, ca.mon_pattern, ca.frame18_mon_pattern)),
            self.ul_circuit_primary_addr(tx_ts.t),
            priority,
            tch_buf_present
        );
    }

    fn generate_hangtime_idle_schf(&self) -> BitBuffer {
        // Full-slot SCH/F carrying a Null PDU (idle).
        let mut buf = BitBuffer::new(SCH_F_CAP);
        let pdu = MacResource::null_pdu();
        pdu.to_bitbuf(&mut buf);
        buf
    }

    pub(crate) fn sdu_is_sndcp_packet_data(sdu: &BitBuffer) -> bool {
        let mut wrapped = BitBuffer::from_bitbuffer(sdu);
        let llc_pdu_type = wrapped.peek_bits(4).and_then(|bits| LlcPduType::try_from(bits).ok());

        let parsed = match llc_pdu_type {
            Some(LlcPduType::BlAdata | LlcPduType::BlAdataFcs) => BlAdata::from_bitbuf(&mut wrapped).is_ok(),
            Some(LlcPduType::BlData | LlcPduType::BlDataFcs) => BlData::from_bitbuf(&mut wrapped).is_ok(),
            Some(LlcPduType::BlUdata | LlcPduType::BlUdataFcs) => BlUdata::from_bitbuf(&mut wrapped).is_ok(),
            Some(LlcPduType::AlDataAlFinal) => AlData::from_bitbuf(&mut wrapped).is_ok(),
            _ => false,
        };
        if !parsed {
            return false;
        }

        wrapped
            .read_field(3, "mle_protocol_discriminator")
            .ok()
            .and_then(|bits| MleProtocolDiscriminator::try_from(bits).ok())
            == Some(MleProtocolDiscriminator::Sndcp)
    }

    pub(crate) fn sdu_is_llc_advanced_link(sdu: &BitBuffer) -> bool {
        let wrapped = BitBuffer::from_bitbuffer(sdu);
        wrapped
            .peek_bits(4)
            .and_then(|bits| LlcPduType::try_from(bits).ok())
            .is_some_and(|pdu_type| matches!(pdu_type, LlcPduType::AlSetup | LlcPduType::AlDataAlFinal | LlcPduType::AlAckAlRnr))
    }

    fn sdu_is_first_original_al_data_segment(sdu: &BitBuffer) -> bool {
        let mut wrapped = BitBuffer::from_bitbuffer(sdu);
        AlData::from_bitbuf(&mut wrapped).is_ok_and(|al| al.ss == 0)
    }

    fn packet_data_assignment_update_from_resource(pdu: &MacResource, sdu: &BitBuffer) -> Option<PacketDataAssignmentUpdate> {
        let addr = pdu.addr?;
        if addr.ssi_type != SsiType::Issi || !Self::sdu_is_sndcp_packet_data(sdu) {
            return None;
        }

        let chan_alloc = pdu.chan_alloc_element.as_ref()?;
        match chan_alloc.alloc_type {
            ChanAllocType::Replace | ChanAllocType::Additional => {
                if chan_alloc.ul_dl_assigned == UlDlAssignment::Augmented {
                    return None;
                }
                if !chan_alloc.ts_assigned.iter().any(|assigned| *assigned) {
                    return Some(PacketDataAssignmentUpdate::QuitAndGo { addr });
                }
                match chan_alloc.alloc_type {
                    ChanAllocType::Replace => Some(PacketDataAssignmentUpdate::Replace {
                        addr,
                        ts_assigned: chan_alloc.ts_assigned,
                        ul_dl_assigned: chan_alloc.ul_dl_assigned,
                        carrier_num: chan_alloc.carrier_num,
                    }),
                    ChanAllocType::Additional => Some(PacketDataAssignmentUpdate::Additional {
                        addr,
                        ts_assigned: chan_alloc.ts_assigned,
                        ul_dl_assigned: chan_alloc.ul_dl_assigned,
                        carrier_num: chan_alloc.carrier_num,
                    }),
                    _ => None,
                }
            }
            ChanAllocType::QuitAndGo => Some(PacketDataAssignmentUpdate::QuitAndGo { addr }),
            ChanAllocType::ReplaceWithCarrierSignalling => None,
        }
    }

    fn packet_data_allocator_blocks_assignment(timeslot_alloc: Option<&TimeslotAllocator>, ts: u8) -> bool {
        timeslot_alloc
            .and_then(|allocator| allocator.owner(ts))
            .is_some_and(|owner| owner != TimeslotOwner::PacketData)
    }

    fn reserve_packet_data_allocator_slot(timeslot_alloc: &mut Option<&mut TimeslotAllocator>, ts: u8) -> bool {
        let Some(allocator) = timeslot_alloc.as_deref_mut() else {
            return true;
        };
        match allocator.owner(ts) {
            None => allocator.reserve(TimeslotOwner::PacketData, ts).is_ok(),
            Some(TimeslotOwner::PacketData) => true,
            Some(owner) => {
                tracing::debug!(
                    "packet-data assignment skipped TS{} because central allocator owner is {:?}",
                    ts,
                    owner
                );
                false
            }
        }
    }

    fn release_packet_data_allocator_slot(timeslot_alloc: &mut Option<&mut TimeslotAllocator>, ts: u8) {
        let Some(allocator) = timeslot_alloc.as_deref_mut() else {
            return;
        };
        if allocator.owner(ts) == Some(TimeslotOwner::PacketData) {
            let _ = allocator.release(TimeslotOwner::PacketData, ts);
        }
    }

    fn packet_data_slot_has_other_assignment(&self, ts: u8, addr: TetraAddress) -> bool {
        if !(2..=NUM_TIMESLOTS as u8).contains(&ts) {
            return false;
        }
        self.packet_data_assignments[ts as usize - 1].is_some_and(|assignment| assignment.addr != addr)
    }

    fn packet_data_has_active_assignment_for_other_addr(&self, addr: TetraAddress, timeslot_alloc: Option<&TimeslotAllocator>) -> bool {
        (2..=NUM_TIMESLOTS as u8).any(|ts| {
            self.packet_data_assignments[ts as usize - 1].is_some_and(|assignment| {
                assignment.addr != addr
                    && !self.packet_data_slot_has_voice_circuit(ts)
                    && !Self::packet_data_allocator_blocks_assignment(timeslot_alloc, ts)
            })
        })
    }

    fn packet_data_slot_available_for_assignment(&self, ts: u8, addr: TetraAddress, timeslot_alloc: Option<&TimeslotAllocator>) -> bool {
        !self.packet_data_slot_has_voice_circuit(ts)
            && !Self::packet_data_allocator_blocks_assignment(timeslot_alloc, ts)
            && !self.packet_data_slot_has_other_assignment(ts, addr)
    }

    fn apply_packet_data_assignment_update(&mut self, update: PacketDataAssignmentUpdate, now: TdmaTime) {
        let mut timeslot_alloc = None;
        self.apply_packet_data_assignment_update_with_allocator(update, now, &mut timeslot_alloc);
    }

    fn apply_packet_data_assignment_update_with_allocator(
        &mut self,
        update: PacketDataAssignmentUpdate,
        now: TdmaTime,
        timeslot_alloc: &mut Option<&mut TimeslotAllocator>,
    ) {
        let expires_at = now.add_timeslots(PACKET_DATA_ASSIGNMENT_TTL_TIMESLOTS);

        match update {
            PacketDataAssignmentUpdate::Replace {
                addr,
                ts_assigned,
                ul_dl_assigned,
                carrier_num,
            } => {
                let requested_count = Self::packet_data_requested_traffic_slot_count(ts_assigned);
                let planned = self.plan_packet_data_traffic_slots_with_allocator(requested_count, addr, timeslot_alloc.as_deref());
                if planned != ts_assigned {
                    tracing::info!(
                        "packet-data PDCH Replace for {} normalized {:?} -> {:?} (single first-free traffic slot)",
                        addr,
                        ts_assigned,
                        planned
                    );
                }
                for ts in 2..=NUM_TIMESLOTS {
                    let slot = ts - 1;
                    if planned[slot]
                        && self.packet_data_slot_available_for_assignment(ts as u8, addr, timeslot_alloc.as_deref())
                        && Self::reserve_packet_data_allocator_slot(timeslot_alloc, ts as u8)
                    {
                        self.packet_data_assignments[slot] = Some(PacketDataAssignment {
                            addr,
                            ul_dl_assigned,
                            carrier_num,
                            expires_at,
                        });
                    } else if planned[slot] {
                        tracing::debug!(
                            "packet-data assignment for {} skipped TS{} because a voice/circuit bearer owns it",
                            addr,
                            ts
                        );
                        self.packet_data_assignments[slot] = None;
                        Self::release_packet_data_allocator_slot(timeslot_alloc, ts as u8);
                    } else if self.packet_data_assignments[slot].is_some_and(|assignment| assignment.addr == addr) {
                        self.packet_data_assignments[slot] = None;
                        Self::release_packet_data_allocator_slot(timeslot_alloc, ts as u8);
                    }
                }
                if ts_assigned[0] {
                    tracing::debug!(
                        "packet-data assignment for {} included TS1; scheduler leaves TS1 common-control",
                        addr
                    );
                }
            }
            PacketDataAssignmentUpdate::Additional {
                addr,
                ts_assigned,
                ul_dl_assigned,
                carrier_num,
            } => {
                let requested_count = Self::packet_data_requested_traffic_slot_count(ts_assigned);
                let planned = self.plan_packet_data_traffic_slots_with_allocator(requested_count, addr, timeslot_alloc.as_deref());
                if planned != ts_assigned {
                    tracing::info!(
                        "packet-data PDCH Additional for {} normalized {:?} -> {:?} (single first-free traffic slot)",
                        addr,
                        ts_assigned,
                        planned
                    );
                }
                for ts in 2..=NUM_TIMESLOTS {
                    let slot = ts - 1;
                    if planned[slot]
                        && self.packet_data_slot_available_for_assignment(ts as u8, addr, timeslot_alloc.as_deref())
                        && Self::reserve_packet_data_allocator_slot(timeslot_alloc, ts as u8)
                    {
                        self.packet_data_assignments[slot] = Some(PacketDataAssignment {
                            addr,
                            ul_dl_assigned,
                            carrier_num,
                            expires_at,
                        });
                    } else if planned[slot] {
                        tracing::debug!(
                            "packet-data additional assignment for {} skipped TS{} because a voice/circuit bearer owns it",
                            addr,
                            ts
                        );
                        self.packet_data_assignments[slot] = None;
                        Self::release_packet_data_allocator_slot(timeslot_alloc, ts as u8);
                    } else if self.packet_data_assignments[slot].is_some_and(|assignment| assignment.addr == addr) {
                        self.packet_data_assignments[slot] = None;
                        Self::release_packet_data_allocator_slot(timeslot_alloc, ts as u8);
                    }
                }
                if ts_assigned[0] {
                    tracing::debug!(
                        "packet-data additional assignment for {} included TS1; scheduler leaves TS1 common-control",
                        addr
                    );
                }
            }
            PacketDataAssignmentUpdate::QuitAndGo { addr } => {
                for slot in 1..NUM_TIMESLOTS {
                    if self.packet_data_assignments[slot].is_some_and(|assignment| assignment.addr == addr) {
                        self.packet_data_assignments[slot] = None;
                        Self::release_packet_data_allocator_slot(timeslot_alloc, (slot + 1) as u8);
                    }
                }
            }
        }
    }

    fn expire_packet_data_assignments(&mut self, now: TdmaTime) {
        let mut timeslot_alloc = None;
        self.expire_packet_data_assignments_with_allocator(now, &mut timeslot_alloc);
    }

    fn expire_packet_data_assignments_with_allocator(&mut self, now: TdmaTime, timeslot_alloc: &mut Option<&mut TimeslotAllocator>) {
        for slot in 1..NUM_TIMESLOTS {
            if self.packet_data_assignments[slot].is_some_and(|assignment| assignment.expires_at.diff(now) <= 0) {
                self.packet_data_assignments[slot] = None;
                Self::release_packet_data_allocator_slot(timeslot_alloc, (slot + 1) as u8);
            }
        }
    }

    fn packet_data_assignment_for_timeslot(&self, ts: TdmaTime) -> Option<PacketDataAssignment> {
        self.packet_data_assignment_for_timeslot_with_allocator(ts, None)
    }

    fn packet_data_assignment_for_timeslot_with_allocator(
        &self,
        ts: TdmaTime,
        timeslot_alloc: Option<&TimeslotAllocator>,
    ) -> Option<PacketDataAssignment> {
        if !(2..=NUM_TIMESLOTS as u8).contains(&ts.t) || ts.f == 18 {
            return None;
        }
        if timeslot_alloc
            .and_then(|allocator| allocator.owner(ts.t))
            .is_some_and(|owner| owner != TimeslotOwner::PacketData)
        {
            return None;
        }
        if self.packet_data_slot_has_voice_circuit(ts.t) {
            return None;
        }
        self.packet_data_assignments[ts.t as usize - 1]
    }

    fn packet_data_slot_has_voice_circuit(&self, ts: u8) -> bool {
        if !(2..=NUM_TIMESLOTS as u8).contains(&ts) {
            return false;
        }
        self.circuits.is_active(Direction::Dl, ts) || self.circuits.is_active(Direction::Ul, ts)
    }

    fn clear_future_uplink_reservations_for_timeslot(&mut self, ts: u8, reason: &str) {
        let Some(slot) = Self::dl_slot_index(ts, "clear_future_uplink_reservations_for_timeslot") else {
            return;
        };
        let mut cleared = 0usize;
        for elem in &mut self.ulsched[slot] {
            if elem.ul1.is_some() || elem.ul2.is_some() || elem.usage_marker.is_some() {
                elem.ul1 = None;
                elem.ul2 = None;
                elem.usage_marker = None;
                cleared += 1;
            }
        }
        if cleared > 0 {
            tracing::info!(
                "packet-data/reserved-access UL reservations cleared on TS{} for voice priority: {} entries ({})",
                ts,
                cleared,
                reason
            );
        }
    }

    fn packet_data_assignment_supports_downlink(assignment: PacketDataAssignment) -> bool {
        matches!(assignment.ul_dl_assigned, UlDlAssignment::Dl | UlDlAssignment::Both)
    }

    fn packet_data_assignment_supports_uplink(assignment: PacketDataAssignment) -> bool {
        matches!(assignment.ul_dl_assigned, UlDlAssignment::Ul | UlDlAssignment::Both)
    }

    fn packet_data_pending_grant_origin_is_valid(
        &self,
        addr: TetraAddress,
        origin: PendingGrantOrigin,
        timeslot_alloc: Option<&TimeslotAllocator>,
    ) -> bool {
        let PendingGrantOrigin::PacketData { assigned_ts } = origin else {
            return true;
        };
        if !(2..=NUM_TIMESLOTS as u8).contains(&assigned_ts) {
            return false;
        }
        if self.packet_data_slot_has_voice_circuit(assigned_ts) {
            return false;
        }
        if timeslot_alloc
            .and_then(|allocator| allocator.owner(assigned_ts))
            .is_some_and(|owner| owner != TimeslotOwner::PacketData)
        {
            return false;
        }
        self.packet_data_assignments[assigned_ts as usize - 1]
            .is_some_and(|assignment| assignment.addr == addr && Self::packet_data_assignment_supports_uplink(assignment))
    }

    fn packet_data_requested_traffic_slot_count(ts_assigned: [bool; NUM_TIMESLOTS]) -> usize {
        let requested_non_mcch = ts_assigned[1..].iter().filter(|assigned| **assigned).count();
        if requested_non_mcch > 0 {
            requested_non_mcch
        } else if ts_assigned[0] {
            1
        } else {
            0
        }
    }

    fn plan_packet_data_traffic_slots_with_allocator(
        &self,
        requested_count: usize,
        addr: TetraAddress,
        timeslot_alloc: Option<&TimeslotAllocator>,
    ) -> [bool; NUM_TIMESLOTS] {
        let mut planned = [false; NUM_TIMESLOTS];
        if requested_count == 0 {
            return planned;
        }

        if self.packet_data_has_active_assignment_for_other_addr(addr, timeslot_alloc) {
            return planned;
        }

        for ts in 2..=NUM_TIMESLOTS as u8 {
            if !self.packet_data_slot_available_for_assignment(ts, addr, timeslot_alloc) {
                continue;
            }
            planned[ts as usize - 1] = true;
            break;
        }
        planned
    }

    fn adapt_packet_data_channel_allocation_for_voice_priority(&self, pdu: &mut MacResource, sdu: &BitBuffer) -> bool {
        self.adapt_packet_data_channel_allocation_for_voice_priority_with_allocator(pdu, sdu, None)
    }

    fn adapt_packet_data_channel_allocation_for_voice_priority_with_allocator(
        &self,
        pdu: &mut MacResource,
        sdu: &BitBuffer,
        timeslot_alloc: Option<&TimeslotAllocator>,
    ) -> bool {
        if pdu.addr.map_or(true, |addr| addr.ssi_type != SsiType::Issi) || !Self::sdu_is_sndcp_packet_data(sdu) {
            return true;
        }

        let Some(chan_alloc) = pdu.chan_alloc_element.as_mut() else {
            return true;
        };
        if chan_alloc.ul_dl_assigned == UlDlAssignment::Augmented {
            return true;
        }
        if !matches!(chan_alloc.alloc_type, ChanAllocType::Replace | ChanAllocType::Additional) {
            return true;
        }

        let addr = pdu.addr.expect("checked above");
        let requested_count = Self::packet_data_requested_traffic_slot_count(chan_alloc.ts_assigned);
        if requested_count == 0 {
            return true;
        }

        let planned = self.plan_packet_data_traffic_slots_with_allocator(requested_count, addr, timeslot_alloc);
        if planned == chan_alloc.ts_assigned {
            return true;
        }

        if planned.iter().any(|assigned| *assigned) {
            tracing::info!(
                "packet-data PDCH allocation for {} dynamically mapped {:?} -> {:?} as single first-free traffic slot around active voice/circuit bearers",
                addr,
                chan_alloc.ts_assigned,
                planned
            );
            chan_alloc.ts_assigned = planned;
            chan_alloc.clch_permission = matches!(chan_alloc.ul_dl_assigned, UlDlAssignment::Ul | UlDlAssignment::Both);
        } else {
            tracing::info!(
                "packet-data PDCH allocation for {} deferred because no single traffic slot is available without parallel packet-data or voice/circuit conflict",
                addr
            );
            return false;
        }
        pdu.update_len_and_fill_ind(sdu.get_len());
        true
    }

    fn packet_data_active_downlink_slots_for_addr(&self, addr: TetraAddress) -> Vec<u8> {
        (2..=NUM_TIMESLOTS as u8)
            .find(|ts| {
                self.packet_data_assignments[*ts as usize - 1].is_some_and(|assignment| {
                    assignment.addr == addr
                        && Self::packet_data_assignment_supports_downlink(assignment)
                        && !self.packet_data_slot_has_voice_circuit(*ts)
                })
            })
            .into_iter()
            .collect()
    }

    fn packet_data_active_uplink_slots_for_addr_with_allocator(
        &self,
        addr: TetraAddress,
        timeslot_alloc: Option<&TimeslotAllocator>,
    ) -> Vec<u8> {
        (2..=NUM_TIMESLOTS as u8)
            .find(|ts| {
                self.packet_data_assignments[*ts as usize - 1].is_some_and(|assignment| {
                    assignment.addr == addr
                        && Self::packet_data_assignment_supports_uplink(assignment)
                        && !self.packet_data_slot_has_voice_circuit(*ts)
                        && !timeslot_alloc
                            .and_then(|allocator| allocator.owner(*ts))
                            .is_some_and(|owner| owner != TimeslotOwner::PacketData)
                })
            })
            .into_iter()
            .collect()
    }

    fn packet_data_downlink_timeslots_for_resource(&mut self, pdu: &MacResource, sdu: &BitBuffer) -> Option<[u8; NUM_TIMESLOTS]> {
        if pdu.chan_alloc_element.is_some() || (!Self::sdu_is_sndcp_packet_data(sdu) && !Self::sdu_is_llc_advanced_link(sdu)) {
            return None;
        }
        let addr = pdu.addr?;
        if addr.ssi_type != SsiType::Issi {
            return None;
        }

        let slots = self.packet_data_active_downlink_slots_for_addr(addr);
        if slots.is_empty() {
            return None;
        }

        let (selected, affinity) = if Self::sdu_is_first_original_al_data_segment(sdu) {
            // Segment 0 is the reassembly anchor for original AL TL-SDUs.
            // Keep multislot use for the rest of the transfer, but put this
            // anchor on the first active PDCH slot so half-duplex, non-fast
            // switching MSs do not repeatedly miss only S(S)=0.
            self.packet_data_downlink_cursor = 1;
            (slots[0], "first_al_segment")
        } else {
            let selected = slots[self.packet_data_downlink_cursor % slots.len()];
            self.packet_data_downlink_cursor = self.packet_data_downlink_cursor.wrapping_add(1);
            (selected, "round_robin")
        };
        tracing::info!(
            "packet-data PDCH downlink for {} selected TS{} from active {:?} sdu_kind={} affinity={}",
            addr,
            selected,
            slots,
            if Self::sdu_is_sndcp_packet_data(sdu) { "sndcp" } else { "al" },
            affinity
        );
        let mut timeslots = [0; NUM_TIMESLOTS];
        timeslots[0] = selected;
        Some(timeslots)
    }

    fn packet_data_resource_requires_packet_data_owner(pdu: &MacResource, sdu: &BitBuffer) -> bool {
        pdu.chan_alloc_element.is_none()
            && pdu.addr.is_some_and(|addr| addr.ssi_type == SsiType::Issi)
            && (Self::sdu_is_sndcp_packet_data(sdu) || Self::sdu_is_llc_advanced_link(sdu))
    }

    fn packet_data_resource_allowed_on_allocated_slot(
        ts: u8,
        pdu: &MacResource,
        sdu: &BitBuffer,
        timeslot_alloc: Option<&TimeslotAllocator>,
    ) -> bool {
        if ts == 1 || !Self::packet_data_resource_requires_packet_data_owner(pdu, sdu) {
            return true;
        }

        let Some(alloc) = timeslot_alloc else {
            return true;
        };

        match alloc.owner(ts) {
            Some(TimeslotOwner::PacketData) => true,
            owner => {
                tracing::info!(
                    "packet-data resource for {:?} dropped on TS{} because allocator owner is {:?}",
                    pdu.addr,
                    ts,
                    owner
                );
                false
            }
        }
    }

    fn packet_data_channel_allocation(
        assignment: PacketDataAssignment,
        ts_assigned: [bool; NUM_TIMESLOTS],
        alloc_type: ChanAllocType,
    ) -> ChanAllocElement {
        ChanAllocElement {
            alloc_type,
            ts_assigned,
            ul_dl_assigned: assignment.ul_dl_assigned,
            clch_permission: matches!(assignment.ul_dl_assigned, UlDlAssignment::Ul | UlDlAssignment::Both),
            cell_change_flag: false,
            carrier_num: assignment.carrier_num,
            ext: None,
            mon_pattern: 0,
            frame18_mon_pattern: Some(0),
        }
    }

    fn purge_packet_data_pending_grants_for_slot(&mut self, addr: TetraAddress, assigned_ts: u8, reason: &str) {
        fn retain_not_matching(queue: &mut Vec<DlSchedElem>, addr: TetraAddress, assigned_ts: u8) -> usize {
            let before = queue.len();
            queue.retain(|elem| {
                !matches!(
                    elem,
                    DlSchedElem::PendingGrant(
                        pending_addr,
                        _,
                        PendingGrantOrigin::PacketData {
                            assigned_ts: pending_ts
                        }
                    ) if *pending_addr == addr && *pending_ts == assigned_ts
                )
            });
            before - queue.len()
        }

        let mut purged = retain_not_matching(&mut self.dltx_next_slot_queue, addr, assigned_ts);
        for queue in &mut self.dltx_queues {
            purged += retain_not_matching(queue, addr, assigned_ts);
        }
        if purged > 0 {
            tracing::info!(
                "packet-data PDCH pending grants purged for {} TS{}: {} entries ({})",
                addr,
                assigned_ts,
                purged,
                reason
            );
        }
    }

    fn preempt_packet_data_assignment_for_voice(&mut self, ts: u8) {
        let Some(slot) = Self::dl_slot_index(ts, "preempt_packet_data_assignment_for_voice") else {
            return;
        };
        let Some(assignment) = self.packet_data_assignments[slot] else {
            return;
        };

        self.purge_packet_data_pending_grants_for_slot(assignment.addr, ts, "voice/circuit preemption");
        self.packet_data_assignments[slot] = None;
        let mut remaining = [false; NUM_TIMESLOTS];
        for next_slot in 1..NUM_TIMESLOTS {
            if self.packet_data_assignments[next_slot].is_some_and(|candidate| {
                candidate.addr == assignment.addr && !self.packet_data_slot_has_voice_circuit((next_slot + 1) as u8)
            }) {
                remaining[next_slot] = true;
            }
        }

        let Some(target_slot) = remaining.iter().enumerate().find_map(|(idx, assigned)| assigned.then_some(idx)) else {
            let mut pdu = Self::dl_make_minimal_resource(&assignment.addr, None, false);
            pdu.chan_alloc_element = Some(Self::packet_data_channel_allocation(
                assignment,
                [false; NUM_TIMESLOTS],
                ChanAllocType::QuitAndGo,
            ));
            pdu.update_len_and_fill_ind(0);
            self.push_sched_queue_bounded(
                0,
                DlSchedElem::Resource(pdu, BitBuffer::new(0), None, None),
                "dltx_queues:pdch_voice_preempt_release",
            );
            tracing::info!(
                "packet-data PDCH assignment for {} lost final TS{} to voice/circuit; queued common-control release",
                assignment.addr,
                ts
            );
            return;
        };

        for next_slot in 1..NUM_TIMESLOTS {
            if next_slot != target_slot
                && self.packet_data_assignments[next_slot].is_some_and(|candidate| candidate.addr == assignment.addr)
            {
                self.packet_data_assignments[next_slot] = None;
            }
        }
        let mut single_remaining = [false; NUM_TIMESLOTS];
        single_remaining[target_slot] = true;

        let mut pdu = Self::dl_make_minimal_resource(&assignment.addr, None, false);
        pdu.chan_alloc_element = Some(Self::packet_data_channel_allocation(
            assignment,
            single_remaining,
            ChanAllocType::Replace,
        ));
        pdu.update_len_and_fill_ind(0);
        self.push_sched_queue_bounded(
            target_slot,
            DlSchedElem::Resource(pdu, BitBuffer::new(0), None, None),
            "dltx_queues:pdch_voice_preempt_resize",
        );
        tracing::info!(
            "packet-data PDCH assignment for {} shrinks away from voice TS{} to single-slot bitmap {:?}",
            assignment.addr,
            ts,
            single_remaining
        );
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
        self.packet_data_assignments = [None, None, None, None];
        self.packet_data_downlink_cursor = 0;
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
        self.ul_find_grant_opportunity_on_channel_from(base_dltime, t, &[t], num_slots, is_halfslot, None)
    }

    fn ul_find_grant_opportunity_for_addr_from(
        &self,
        base_dltime: TdmaTime,
        timeslot: u8,
        addr: TetraAddress,
        num_slots: usize,
        is_halfslot: bool,
        timeslot_alloc: Option<&TimeslotAllocator>,
    ) -> Option<(usize, Vec<TdmaTime>)> {
        let packet_data_uplink_slots = self.packet_data_active_uplink_slots_for_addr_with_allocator(addr, timeslot_alloc);
        if packet_data_uplink_slots.contains(&timeslot) {
            // EN 300 392-2 clause 23.5.2.2.2: on a multi-slot assigned
            // channel, slot-grant delay opportunities are counted across all
            // uplink timeslots assigned to that channel, not just the
            // same-numbered timeslot carrying the grant.
            return self.ul_find_grant_opportunity_on_channel_from(
                base_dltime,
                timeslot,
                &packet_data_uplink_slots,
                num_slots,
                is_halfslot,
                Some(addr.ssi),
            );
        }
        if !packet_data_uplink_slots.is_empty() {
            tracing::debug!(
                "ul_find_grant_opportunity: addr {} pending grant on TS{} but active PDCH uplink channel is {:?}",
                addr,
                timeslot,
                packet_data_uplink_slots
            );
            return None;
        }

        self.ul_find_grant_opportunity_from(base_dltime, timeslot, num_slots, is_halfslot)
    }

    fn ul_find_grant_opportunity_on_channel_from(
        &self,
        base_dltime: TdmaTime,
        grant_timeslot: u8,
        channel_timeslots: &[u8],
        num_slots: usize,
        is_halfslot: bool,
        reusable_owner: Option<u32>,
    ) -> Option<(usize, Vec<TdmaTime>)> {
        if channel_timeslots.is_empty() || !channel_timeslots.contains(&grant_timeslot) {
            return None;
        }
        let first_opportunity = base_dltime.forward_to_timeslot(grant_timeslot);
        let mut grant_timeslots = Vec::with_capacity(num_slots);
        let mut opportunities_skipped = 0;

        assert!(!is_halfslot || num_slots == 1, "is_halfslot set for num_slots > 1");

        for dist in 0..(MACSCHED_NUM_FRAMES - 1) * NUM_TIMESLOTS {
            // let candidate_t = self.cur_ts.add_timeslots(dist as i32 * 4);
            // Base off of internal perception of time, convert to UL time
            // Below may crash someday, but I'd want to investigate that situation
            let candidate_t = first_opportunity.add_timeslots(dist as i32);
            if !channel_timeslots.contains(&candidate_t.t) {
                continue;
            }

            tracing::debug!(
                "ul_find_grant_opportunity: considering candidate ul_ts {} on channel {:?}, have {:?}",
                candidate_t,
                channel_timeslots,
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
            let elem = &self.ulsched[candidate_t.t as usize - 1][index];
            // tracing::debug!("ul_find_grant_opportunity: sched[{}] ts {}: {:?}", index, candidate_t, elem);
            // Packet-data terminals can ask for more reserved access while
            // earlier grants for the same assigned PDCH are still pending.
            // Re-granting capacity that is already reserved for the same ISSI
            // is conflict-free and follows EN 300 392-2 clause 23.5.2.2.7's
            // recommendation to re-grant remaining capacity when needed.
            let same_owner_full_slot =
                reusable_owner.is_some_and(|owner| !is_halfslot && elem.ul1 == Some(owner) && elem.ul2 == Some(owner));
            if (elem.ul1.is_none() && elem.ul2.is_none())
                || same_owner_full_slot
                || (is_halfslot && (elem.ul1.is_none() || elem.ul2.is_none()))
            {
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
                // Same-ISSI full-slot PDCH re-grants are already present in
                // the UL schedule; keep the owner and refresh only the marker.
                if elem.ul1 == Some(ssi) && elem.ul2 == Some(ssi) {
                    continue;
                }
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
        self.ul_process_pending_grant_from(base_dltime, timeslot, addr, PendingGrantDebt::new(*res_req))
            .map(|(grant, usage_marker, _granted_slots)| (grant, usage_marker))
    }

    fn ul_process_pending_grant_from(
        &mut self,
        base_dltime: TdmaTime,
        timeslot: u8,
        addr: TetraAddress,
        debt: PendingGrantDebt,
    ) -> Option<(BasicSlotgrant, Option<u8>, usize)> {
        self.ul_process_pending_grant_from_with_allocator(base_dltime, timeslot, addr, debt, None)
    }

    fn ul_process_pending_grant_from_with_allocator(
        &mut self,
        base_dltime: TdmaTime,
        timeslot: u8,
        addr: TetraAddress,
        debt: PendingGrantDebt,
        timeslot_alloc: Option<&TimeslotAllocator>,
    ) -> Option<(BasicSlotgrant, Option<u8>, usize)> {
        let is_halfslot = debt.is_halfslot();
        let requested_cap = debt.next_chunk_slots();

        let mut too_late_op = None;
        let mut grant_op = None;
        let candidate_caps: &[usize] = if is_halfslot { &[1] } else { BASIC_GRANT_FULL_SLOT_COUNTS_DESC };
        for candidate_cap in candidate_caps.iter().copied().filter(|cap| *cap <= requested_cap) {
            let candidate_op =
                self.ul_find_grant_opportunity_for_addr_from(base_dltime, timeslot, addr, candidate_cap, is_halfslot, timeslot_alloc);
            if let Some((skips, grant_timestamps)) = candidate_op {
                if skips <= 13 {
                    grant_op = Some((candidate_cap, skips, grant_timestamps));
                    break;
                }
                too_late_op.get_or_insert((candidate_cap, skips));
            }
        }

        tracing::debug!(
            "ul_process_cap_req: addr {}, debt {:?}, chunk_cap {}, is_halfslot {}, grant_op: {:?}",
            addr,
            debt,
            requested_cap,
            is_halfslot,
            grant_op
        );

        // If found, reserve the slots and return a BasicSlotgrant + optional usage_marker.
        if let Some((granted_cap, skips, grant_timestamps)) = grant_op {
            // For multi-slot full grants, allocate a usage marker. We do this
            // BEFORE reserving so the marker can be embedded in the schedule.
            // Single-slot or half-slot grants don't need a marker — the MS
            // either has nothing to fragment (subslot) or completes the burst
            // in the one slot (single full slot).
            let usage_marker = if !is_halfslot && granted_cap >= 2 {
                Some(self.alloc_usage_marker(timeslot))
            } else {
                None
            };

            if granted_cap < requested_cap {
                tracing::debug!(
                    "ul_process_cap_req: addr {} debt {:?} reduced grant chunk {} -> {} to keep delay {} representable",
                    addr,
                    debt,
                    requested_cap,
                    granted_cap,
                    skips
                );
            }

            // Reserve the target granting opportunity. Get subslot (only relevant for halfslot reservation)
            let subslot = self.ul_reserve_grant(addr.ssi, grant_timestamps, is_halfslot, usage_marker);

            // tracing::info!("After grant:")
            // self.dump_ul_schedule_full(false);

            // Build BasicSlotgrant response element
            let cap_alloc = if is_halfslot {
                match subslot {
                    1 => BasicSlotgrantCapAlloc::FirstSubslotGranted,
                    2 => BasicSlotgrantCapAlloc::SecondSubslotGranted,
                    _ => unreachable!("ul_process_cap_req: subslot must be 1 or 2, got {}", subslot),
                }
            } else {
                BasicSlotgrantCapAlloc::from_req_slotcount(granted_cap)
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
                granted_cap,
            ))
        } else {
            if let Some((candidate_cap, skips)) = too_late_op {
                // EN 300 392-2 clause 21.5.6 defines delay opportunities only
                // for raw values 1..=13. Raw 14/15 are special meanings, so do
                // not reserve capacity that cannot be represented as a valid
                // basic slot-grant delay.
                tracing::warn!(
                    "ul_process_cap_req: grant opportunity for addr {} debt {:?} chunk {} needs {} delay opportunities",
                    addr,
                    debt,
                    candidate_cap,
                    skips
                );
                return None;
            }
            tracing::warn!(
                "ul_process_cap_req: no suitable grant opportunity found for addr {}, debt {:?}",
                addr,
                debt
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

    pub fn packet_data_uplink_owner(&self, ts: TdmaTime, slot: PhyBlockNum) -> Option<TetraAddress> {
        if slot != PhyBlockNum::Both {
            return None;
        }
        let assignment = self.packet_data_assignment_for_timeslot(ts)?;
        matches!(assignment.ul_dl_assigned, UlDlAssignment::Ul | UlDlAssignment::Both).then_some(assignment.addr)
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
        let Some(slot) = Self::dl_slot_index(ts, "dl_enqueue_grant") else {
            return;
        };
        tracing::debug!(
            "dl_enqueue_grant: ts {} enqueueing PDU {:?} for addr {} marker {:?}",
            ts,
            grant,
            addr,
            usage_marker
        );
        let elem = DlSchedElem::Grant(addr, grant, usage_marker);
        self.push_sched_queue_bounded(slot, elem, "dltx_queues:dl_enqueue_grant");
    }

    pub fn dl_enqueue_reservation_grant(&mut self, ts: u8, addr: TetraAddress, res_req: ReservationRequirement) {
        self.dl_enqueue_reservation_grant_with_origin(ts, addr, res_req, PendingGrantOrigin::CommonControl);
    }

    pub fn dl_enqueue_packet_data_reservation_grant(&mut self, ts: u8, addr: TetraAddress, res_req: ReservationRequirement) {
        self.dl_enqueue_reservation_grant_with_origin(ts, addr, res_req, PendingGrantOrigin::PacketData { assigned_ts: ts });
    }

    fn dl_enqueue_reservation_grant_with_origin(
        &mut self,
        ts: u8,
        addr: TetraAddress,
        res_req: ReservationRequirement,
        origin: PendingGrantOrigin,
    ) {
        let Some(slot) = Self::dl_slot_index(ts, "dl_enqueue_reservation_grant") else {
            return;
        };
        tracing::debug!(
            "dl_enqueue_reservation_grant: ts {} enqueueing reservation {:?} for addr {} origin {:?}",
            ts,
            res_req,
            addr,
            origin
        );
        let elem = DlSchedElem::PendingGrant(addr, PendingGrantDebt::new(res_req), origin);
        self.push_sched_queue_bounded(slot, elem, "dltx_queues:dl_enqueue_reservation_grant");
    }

    pub fn dl_enqueue_random_access_ack(&mut self, ts: u8, addr: TetraAddress) {
        let Some(slot) = Self::dl_slot_index(ts, "dl_enqueue_random_access_ack") else {
            return;
        };
        tracing::debug!(
            "dl_enqueue_random_access_ack: ts {} enqueueing random access acknowledgementfor addr {}",
            ts,
            addr
        );
        let elem = DlSchedElem::RandomAccessAck(addr);
        self.push_sched_queue_with_cap(
            slot,
            elem,
            MAX_PENDING_RA_ACKS_PER_TIMESLOT,
            "dltx_queues:dl_enqueue_random_access_ack",
        );
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

    fn elem_protected_from_backpressure(elem: &DlSchedElem) -> bool {
        Self::elem_backpressure_priority(elem) > DlBackpressurePriority::Ordinary
    }

    fn elem_backpressure_priority(elem: &DlSchedElem) -> DlBackpressurePriority {
        if let DlSchedElem::Stealing(..) = elem {
            return match Self::stealing_sched_priority(elem) {
                StealingSchedPriority::Ordinary => DlBackpressurePriority::Ordinary,
                StealingSchedPriority::ChannelAllocation | StealingSchedPriority::CmceChannelAllocation => {
                    DlBackpressurePriority::ChannelAllocation
                }
                StealingSchedPriority::ListenerFloorGrant => DlBackpressurePriority::ListenerFloorGrant,
                StealingSchedPriority::PositiveFloorGrant => DlBackpressurePriority::PositiveFloorGrant,
                StealingSchedPriority::FloorWithdraw => DlBackpressurePriority::FloorWithdraw,
            };
        }

        if Self::elem_has_channel_allocation(elem) {
            return DlBackpressurePriority::ChannelAllocation;
        }

        if Self::elem_has_integrated_grant_or_ack(elem) {
            return DlBackpressurePriority::GrantOrAck;
        }

        match elem {
            DlSchedElem::Resource(_, sdu, _, _) if Self::resource_is_cmce_setup_call_control(sdu) => {
                DlBackpressurePriority::CmceCallControl
            }
            DlSchedElem::Grant(..) | DlSchedElem::PendingGrant(..) | DlSchedElem::RandomAccessAck(_) => DlBackpressurePriority::GrantOrAck,
            _ => DlBackpressurePriority::Ordinary,
        }
    }

    fn mark_sched_elem_discarded_if_reported(elem: DlSchedElem) {
        match elem {
            DlSchedElem::Resource(_, _, tx_reporter, group_state) => {
                Self::mark_reporter_discarded_if_pending(tx_reporter.or_else(|| group_state.and_then(|state| state.tx_reporter)));
            }
            DlSchedElem::FragBuf(_, group_state) => {
                Self::mark_reporter_discarded_if_pending(group_state.and_then(|state| state.tx_reporter));
            }
            DlSchedElem::Stealing(_, _, tx_reporter, group_state) => {
                Self::mark_reporter_discarded_if_pending(tx_reporter.or_else(|| group_state.and_then(|state| state.tx_reporter)));
            }
            DlSchedElem::Broadcast(_) | DlSchedElem::RandomAccessAck(_) | DlSchedElem::Grant(..) | DlSchedElem::PendingGrant(..) => {}
        }
    }

    fn enforce_sched_queue_cap(queue: &mut Vec<DlSchedElem>, cap: usize, label: &str) {
        while queue.len() > cap {
            let protected_drop = queue
                .iter()
                .enumerate()
                .min_by_key(|(_, elem)| Self::elem_backpressure_priority(elem))
                .map(|(index, elem)| (index, Self::elem_backpressure_priority(elem)));
            let Some((pos, priority)) = protected_drop else {
                return;
            };

            if priority > DlBackpressurePriority::Ordinary {
                tracing::warn!(
                    "UMAC scheduler: {} has {} protected element(s), discarding oldest {:?} element to preserve local cap {}",
                    label,
                    queue.len(),
                    priority,
                    cap
                );
            } else {
                tracing::warn!(
                    "UMAC scheduler: discarding queued downlink element from {} because queue length exceeded cap {}",
                    label,
                    cap
                );
            }
            let elem = queue.remove(pos);
            Self::mark_sched_elem_discarded_if_reported(elem);
        }
    }

    fn floor_withdraw_key(elem: &DlSchedElem) -> Option<FloorWithdrawKey> {
        let DlSchedElem::Stealing(block, fallback_addr, _, _) = elem else {
            return None;
        };
        let mut mac_probe = BitBuffer::from_bitbuffer(block);
        let resource = MacResource::from_bitbuf(&mut mac_probe).ok()?;
        let addr = resource.addr.unwrap_or(*fallback_addr);
        let mac_payload = BitBuffer::from_bitbuffer_pos(&mac_probe);
        let mut cmce_payload = Self::cmce_dl_payload_from_tma_sdu(&mac_payload)?;
        let pdu_type = cmce_payload
            .read_field(5, "cmce_pdu_type_dl")
            .ok()
            .and_then(|bits| CmcePduTypeDl::try_from(bits).ok())?;

        match pdu_type {
            CmcePduTypeDl::DTxCeased | CmcePduTypeDl::DTxInterrupt => {
                let call_id = cmce_payload.read_field(14, "call_identifier").ok()? as u16;
                Some(FloorWithdrawKey { addr, call_id, pdu_type })
            }
            _ => None,
        }
    }

    fn coalesce_floor_withdraw(queue: &mut Vec<DlSchedElem>, elem: &DlSchedElem, label: &str) {
        let Some(incoming_key) = Self::floor_withdraw_key(elem) else {
            return;
        };

        let mut idx = 0;
        while idx < queue.len() {
            if Self::floor_withdraw_key(&queue[idx]) == Some(incoming_key) {
                let old = queue.remove(idx);
                tracing::debug!(
                    "UMAC scheduler: coalescing older {:?} floor-control for call_id={} addr={} in {}",
                    incoming_key.pdu_type,
                    incoming_key.call_id,
                    incoming_key.addr,
                    label
                );
                Self::mark_sched_elem_discarded_if_reported(old);
            } else {
                idx += 1;
            }
        }
    }

    fn coalesce_ready_grant_or_ack(queue: &mut [DlSchedElem], elem: DlSchedElem) -> Option<DlSchedElem> {
        match elem {
            DlSchedElem::Grant(addr, grant, usage_marker) => {
                for queued in queue.iter_mut().rev() {
                    match queued {
                        DlSchedElem::Resource(pdu, _, _, _) if pdu.addr == Some(addr) => {
                            pdu.slot_granting_element = Some(grant);
                            if pdu.usage_marker.is_none() {
                                pdu.usage_marker = usage_marker;
                            }
                            return None;
                        }
                        DlSchedElem::RandomAccessAck(ack_addr) if *ack_addr == addr => {
                            let mut pdu = Self::dl_make_minimal_resource(&addr, Some(grant), true);
                            pdu.usage_marker = usage_marker;
                            *queued = DlSchedElem::Resource(pdu, BitBuffer::new(0), None, None);
                            return None;
                        }
                        DlSchedElem::Grant(grant_addr, queued_grant, queued_marker) if *grant_addr == addr => {
                            *queued_grant = grant;
                            if queued_marker.is_none() {
                                *queued_marker = usage_marker;
                            }
                            return None;
                        }
                        _ => {}
                    }
                }
                Some(DlSchedElem::Grant(addr, grant, usage_marker))
            }
            DlSchedElem::RandomAccessAck(addr) => {
                for queued in queue.iter_mut().rev() {
                    match queued {
                        DlSchedElem::Resource(pdu, _, _, _) if pdu.addr == Some(addr) => {
                            pdu.random_access_flag = true;
                            return None;
                        }
                        DlSchedElem::Grant(grant_addr, grant, usage_marker) if *grant_addr == addr => {
                            let mut pdu = Self::dl_make_minimal_resource(&addr, Some(grant.clone()), true);
                            pdu.usage_marker = *usage_marker;
                            *queued = DlSchedElem::Resource(pdu, BitBuffer::new(0), None, None);
                            return None;
                        }
                        DlSchedElem::RandomAccessAck(ack_addr) if *ack_addr == addr => {
                            return None;
                        }
                        _ => {}
                    }
                }
                Some(DlSchedElem::RandomAccessAck(addr))
            }
            DlSchedElem::PendingGrant(addr, debt, origin) => {
                for queued in queue.iter_mut().rev() {
                    if let DlSchedElem::PendingGrant(pending_addr, pending_debt, pending_origin) = queued
                        && *pending_addr == addr
                        && *pending_origin == origin
                    {
                        *pending_debt = pending_debt.coalesce_max(debt);
                        return None;
                    }
                }
                Some(DlSchedElem::PendingGrant(addr, debt, origin))
            }
            elem => Some(elem),
        }
    }

    fn push_sched_queue_with_cap(&mut self, slot: usize, elem: DlSchedElem, cap: usize, label: &str) {
        Self::coalesce_floor_withdraw(&mut self.dltx_queues[slot], &elem, label);
        if let Some(elem) = Self::coalesce_ready_grant_or_ack(&mut self.dltx_queues[slot], elem) {
            self.dltx_queues[slot].push(elem);
        }
        Self::enforce_sched_queue_cap(&mut self.dltx_queues[slot], cap, label);
    }

    fn push_sched_queue_bounded(&mut self, slot: usize, elem: DlSchedElem, label: &str) {
        self.push_sched_queue_with_cap(slot, elem, MAX_DLSCHED_ELEMS_PER_TIMESLOT, label);
    }

    fn push_next_slot_queue_bounded(&mut self, elem: DlSchedElem, label: &str) {
        Self::coalesce_floor_withdraw(&mut self.dltx_next_slot_queue, &elem, label);
        if let Some(elem) = Self::coalesce_ready_grant_or_ack(&mut self.dltx_next_slot_queue, elem) {
            self.dltx_next_slot_queue.push(elem);
        }
        Self::enforce_sched_queue_cap(&mut self.dltx_next_slot_queue, MAX_DLSCHED_NEXT_SLOT_ELEMS, label);
    }

    pub fn dl_enqueue_tma(&mut self, pdu: MacResource, sdu: BitBuffer, tx_reporter: Option<TxReporter>) {
        self.dl_enqueue_tma_inner(pdu, sdu, tx_reporter, None);
    }

    pub fn dl_enqueue_tma_with_current_channel_ack_grant(
        &mut self,
        pdu: MacResource,
        sdu: BitBuffer,
        tx_reporter: Option<TxReporter>,
        grant_addr: TetraAddress,
        res_req: ReservationRequirement,
    ) {
        self.dl_enqueue_tma_inner(pdu, sdu, tx_reporter, Some((grant_addr, res_req)));
    }

    fn dl_enqueue_tma_inner(
        &mut self,
        mut pdu: MacResource,
        sdu: BitBuffer,
        tx_reporter: Option<TxReporter>,
        current_channel_ack_grant: Option<(TetraAddress, ReservationRequirement)>,
    ) {
        if !self.adapt_packet_data_channel_allocation_for_voice_priority(&mut pdu, &sdu) {
            Self::mark_reporter_discarded_if_pending(tx_reporter);
            return;
        }

        let timeslots = if current_channel_ack_grant.is_none() {
            self.packet_data_downlink_timeslots_for_resource(&pdu, &sdu)
                .unwrap_or_else(Self::common_control_downlink_timeslots)
        } else {
            Self::common_control_downlink_timeslots()
        };

        // Queue the message for all timeslots on which we should transmit this message.
        // The loop basically prevents cloning the last element.
        for i in 0..NUM_TIMESLOTS {
            let ts = timeslots[i];
            let next_ts = if i < NUM_TIMESLOTS - 1 { timeslots[i + 1] } else { 0 };
            assert!(ts > 0);

            // If this PDU carries a chan_alloc element (DConnect/DConnectAck MCCH), check if we
            // already sent one this frame. DConnect MCCH (113 bits) + DConnectAck MCCH (110 bits)
            // = 223 bits > 216-bit slot capacity. Defer the second one to the next frame.
            let deferred = if ts == 1 && pdu.chan_alloc_element.is_some() {
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
                self.push_next_slot_queue_bounded(elem, "dltx_next_slot_queue:dl_enqueue_tma");
                if let Some((grant_addr, res_req)) = current_channel_ack_grant {
                    let elem = DlSchedElem::PendingGrant(grant_addr, PendingGrantDebt::new(res_req), PendingGrantOrigin::CommonControl);
                    self.push_next_slot_queue_bounded(elem, "dltx_next_slot_queue:dl_enqueue_tma_ack_grant");
                }
                break;
            } else if next_ts > 0 {
                // There is another ts for which we need to transmit this message.
                // Clone the message now and push it to the current ts.
                let elem = DlSchedElem::Resource(pdu.clone(), sdu.clone(), tx_reporter.clone(), None);
                self.push_sched_queue_bounded(ts as usize - 1, elem, "dltx_queues:dl_enqueue_tma");
                if let Some((grant_addr, res_req)) = current_channel_ack_grant {
                    let elem = DlSchedElem::PendingGrant(grant_addr, PendingGrantDebt::new(res_req), PendingGrantOrigin::CommonControl);
                    self.push_sched_queue_bounded(ts as usize - 1, elem, "dltx_queues:dl_enqueue_tma_ack_grant");
                }
            } else {
                // This is the last ts on which we need to transmit this message
                let elem = DlSchedElem::Resource(pdu, sdu, tx_reporter, None);
                self.push_sched_queue_bounded(ts as usize - 1, elem, "dltx_queues:dl_enqueue_tma");
                if let Some((grant_addr, res_req)) = current_channel_ack_grant {
                    let elem = DlSchedElem::PendingGrant(grant_addr, PendingGrantDebt::new(res_req), PendingGrantOrigin::CommonControl);
                    self.push_sched_queue_bounded(ts as usize - 1, elem, "dltx_queues:dl_enqueue_tma_ack_grant");
                }
                break;
            }
        }
    }

    /// Consumes and returns true if a pending random access ack exists for the given address on
    /// this timeslot. Used when building STCH blocks so the MAC-RESOURCE can carry
    /// random_access_flag=true per ETSI 21.4.3.1.
    pub fn take_pending_ra_ack(&mut self, ts: u8, addr: TetraAddress) -> bool {
        let Some(slot) = Self::dl_slot_index(ts, "take_pending_ra_ack") else {
            return false;
        };
        let pending = &mut self.pending_ra_acks[slot];
        if let Some(pos) = pending
            .iter()
            .position(|pending_addr| pending_addr.ssi == addr.ssi && pending_addr.ssi_type == addr.ssi_type)
        {
            pending.swap_remove(pos);
            true
        } else {
            false
        }
    }

    fn store_pending_ra_ack(&mut self, slot: usize, ts: u8, addr: TetraAddress) {
        let pending = &mut self.pending_ra_acks[slot];
        if pending
            .iter()
            .any(|pending_addr| pending_addr.ssi == addr.ssi && pending_addr.ssi_type == addr.ssi_type)
        {
            tracing::debug!(
                "store_pending_ra_ack: random-access ACK for {} on ts {} is already pending",
                addr,
                ts
            );
            return;
        }

        if pending.len() >= MAX_PENDING_RA_ACKS_PER_TIMESLOT {
            let dropped = pending.swap_remove(0);
            tracing::warn!(
                "store_pending_ra_ack: dropping deferred random-access ACK {} on ts {} because {} ACK(s) are already pending",
                dropped,
                ts,
                MAX_PENDING_RA_ACKS_PER_TIMESLOT
            );
        }

        pending.push(addr);
    }

    /// Returns whether an STCH MAC-RESOURCE should carry random_access_flag for
    /// this address. ACK-only STCH mirrors the acknowledgement but keeps it
    /// pending for the following channel-allocation STCH.
    ///
    /// EN 300 392-2 clause 21.4.3.1 defines the random access flag as the BS
    /// acknowledgement of successful random access, so the first MAC-RESOURCE
    /// after a U-TX DEMAND random access should not suppress it. Clauses
    /// 14.5.1.2.1 b), 14.5.2.2.1 b), and 23.5.2.2.1 make the following
    /// channel-allocation D-TX GRANTED the response that lets the requesting MS
    /// enter the assigned-channel U-plane; keep the acknowledgement pending for
    /// that PDU as well.
    pub fn take_pending_ra_ack_for_stch(&mut self, ts: u8, addr: TetraAddress, carries_channel_allocation: bool) -> bool {
        let Some(slot) = Self::dl_slot_index(ts, "take_pending_ra_ack_for_stch") else {
            return false;
        };
        let has_pending = self.pending_ra_acks[slot]
            .iter()
            .any(|pending_addr| pending_addr.ssi == addr.ssi && pending_addr.ssi_type == addr.ssi_type);
        if !has_pending {
            return false;
        }

        if carries_channel_allocation {
            self.take_pending_ra_ack(ts, addr)
        } else {
            tracing::debug!(
                "take_pending_ra_ack_for_stch: mirroring pending RA ACK for {} on ts {} and preserving it until channel-allocation STCH",
                addr,
                ts
            );
            true
        }
    }

    /// Same STCH random-access acknowledgement decision as
    /// `take_pending_ra_ack_for_stch`, but also consumes a ready ACK that is
    /// still queued on the real downlink scheduler path. This is intentionally
    /// used for group requester positive D-TX GRANTED only: CMCE gates UMAC
    /// FloorGranted until that STCH is transmitted, so hangtime cleanup may not
    /// have moved the ACK into `pending_ra_acks` yet.
    pub fn take_pending_or_ready_ra_ack_for_stch(&mut self, ts: u8, addr: TetraAddress, carries_channel_allocation: bool) -> bool {
        if self.take_pending_ra_ack_for_stch(ts, addr, carries_channel_allocation) {
            return true;
        }

        let Some(slot) = Self::dl_slot_index(ts, "take_pending_or_ready_ra_ack_for_stch") else {
            return false;
        };
        let Some(pos) = self.dltx_queues[slot].iter().position(
            |elem| matches!(elem, DlSchedElem::RandomAccessAck(ack_addr) if ack_addr.ssi == addr.ssi && ack_addr.ssi_type == addr.ssi_type),
        ) else {
            return false;
        };

        if carries_channel_allocation {
            self.dltx_queues[slot].remove(pos);
        } else {
            tracing::debug!(
                "take_pending_or_ready_ra_ack_for_stch: mirroring ready RA ACK for {} on ts {} and preserving it until channel-allocation STCH",
                addr,
                ts
            );
        }
        true
    }

    /// Enqueue a pre-built STCH block for FACCH/stealing on a traffic timeslot.
    /// The block must be 124 type1 bits containing MAC-U-SIGNAL header + TM-SDU.
    pub fn dl_enqueue_stealing(&mut self, ts: u8, block: BitBuffer, addr: TetraAddress, tx_reporter: Option<TxReporter>) {
        let Some(slot) = Self::dl_slot_index(ts, "dl_enqueue_stealing") else {
            Self::mark_reporter_discarded_if_pending(tx_reporter);
            return;
        };
        tracing::info!("dl_enqueue_stealing: ts {} enqueueing STCH block ({} bits)", ts, block.get_len());
        self.push_sched_queue_bounded(
            slot,
            DlSchedElem::Stealing(block, addr, tx_reporter, None),
            "dltx_queues:dl_enqueue_stealing",
        );
    }

    fn dl_requeue_group_stealing(&mut self, ts: u8, block: BitBuffer, addr: TetraAddress, group_state: GroupStealingState) {
        let Some(slot) = Self::dl_slot_index(ts, "dl_requeue_group_stealing") else {
            Self::mark_reporter_discarded_if_pending(group_state.tx_reporter);
            return;
        };
        tracing::debug!(
            "dl_requeue_group_stealing: GSSI {} covered {}/{} on ts {}",
            addr.ssi,
            group_state.covered.len(),
            group_state.targets.len(),
            ts
        );
        self.push_sched_queue_bounded(
            slot,
            DlSchedElem::Stealing(block, addr, group_state.tx_reporter.clone(), Some(group_state)),
            "dltx_queues:dl_requeue_group_stealing",
        );
    }

    fn dl_enqueue_tma_frag_next_frame_with_group_state(&mut self, fragger: BsFragger, group_state: Option<GroupDeliveryState>) {
        tracing::debug!("dl_enqueue_tma_frag_next_frame: enqueueing {:?}", fragger);
        let elem = DlSchedElem::FragBuf(fragger, group_state);
        self.push_next_slot_queue_bounded(elem, "dltx_next_slot_queue:dl_enqueue_tma_frag_next_frame");
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
        self.push_next_slot_queue_bounded(elem, "dltx_next_slot_queue:dl_enqueue_group_repeat_next_frame");
    }

    fn dl_defer_pending_grant_next_frame(&mut self, addr: TetraAddress, debt: PendingGrantDebt, origin: PendingGrantOrigin) {
        tracing::debug!(
            "dl_defer_pending_grant_next_frame: requeueing reservation debt {:?} for addr {} origin {:?}",
            debt,
            addr,
            origin
        );
        self.push_next_slot_queue_bounded(
            DlSchedElem::PendingGrant(addr, debt, origin),
            "dltx_next_slot_queue:dl_defer_pending_grant_next_frame",
        );
    }

    fn elem_addr(elem: &DlSchedElem) -> Option<TetraAddress> {
        match elem {
            DlSchedElem::RandomAccessAck(addr) | DlSchedElem::Grant(addr, _, _) | DlSchedElem::PendingGrant(addr, _, _) => Some(*addr),
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
            subscribers.group_member_issis(addr.ssi).collect()
        };
        targets.sort_unstable();
        targets.dedup();
        targets
    }

    fn is_predefined_broadcast_gssi(addr: Option<TetraAddress>) -> bool {
        matches!(
            addr,
            Some(TetraAddress {
                ssi: PREDEFINED_BROADCAST_GSSI,
                ssi_type: SsiType::Gssi,
            })
        )
    }

    fn retain_current_group_delivery_targets(
        state: &mut GroupDeliveryState,
        addr: TetraAddress,
        subscribers: &SubscriberRegistry,
        readiness_cache: &mut GroupReadinessCache,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
    ) {
        // EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6 require delivery to match
        // current EG listening opportunities. If MM removes a registration or
        // group affiliation while a repeated GSSI transfer is pending, stale
        // snapshot targets are no longer valid local addresses.
        let current_targets = readiness_cache.targets_for(addr, subscribers);
        let current_targets = Self::energy_economy_targets(current_targets, energy_saving);
        state.retain_targets(&current_targets);
    }

    fn retain_current_group_stealing_targets(
        state: &mut GroupStealingState,
        addr: TetraAddress,
        subscribers: &SubscriberRegistry,
        readiness_cache: &mut GroupReadinessCache,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
    ) {
        // Same current-address pruning as MAC-RESOURCE delivery, applied to
        // FACCH/STCH repeats whose block is already encoded.
        let current_targets = readiness_cache.targets_for(addr, subscribers);
        let current_targets = Self::energy_economy_targets(current_targets, energy_saving);
        state.retain_targets(&current_targets);
    }

    fn prune_completed_stale_group_states_for_slot(
        &mut self,
        slot: usize,
        subscribers: &SubscriberRegistry,
        readiness_cache: &mut GroupReadinessCache,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
    ) {
        let Some(queue) = self.dltx_queues.get_mut(slot) else {
            return;
        };

        queue.retain_mut(|elem| {
            let completed_reporter = match elem {
                DlSchedElem::Resource(pdu, _, _, Some(state)) => pdu.addr.and_then(|addr| {
                    Self::retain_current_group_delivery_targets(state, addr, subscribers, readiness_cache, energy_saving);
                    state.is_complete().then(|| state.tx_reporter.clone())
                }),
                DlSchedElem::FragBuf(fragger, Some(state)) => fragger.addr().and_then(|addr| {
                    Self::retain_current_group_delivery_targets(state, addr, subscribers, readiness_cache, energy_saving);
                    state.is_complete().then(|| state.tx_reporter.clone())
                }),
                DlSchedElem::Stealing(_, addr, _, Some(state)) => {
                    Self::retain_current_group_stealing_targets(state, *addr, subscribers, readiness_cache, energy_saving);
                    state.is_complete().then(|| state.tx_reporter.clone())
                }
                _ => None,
            };

            if let Some(reporter) = completed_reporter {
                if let Some(reporter) = reporter {
                    if !reporter.try_mark_transmitted() {
                        tracing::debug!(
                            "BsChannelScheduler: ignoring late group complete-transmission report for reporter already in {:?}",
                            reporter.get_state()
                        );
                    }
                }
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
        readiness_cache: &mut GroupReadinessCache,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
    ) -> Option<GroupDeliveryState> {
        if addr.ssi_type != SsiType::Gssi {
            return None;
        }

        let targets = readiness_cache.targets_for(addr, subscribers);
        if targets.is_empty() {
            return None;
        }
        let energy_economy_targets = Self::energy_economy_targets(targets, energy_saving);
        if energy_economy_targets.is_empty() {
            return None;
        }

        Some(GroupDeliveryState::new(
            pdu.clone(),
            sdu.clone(),
            energy_economy_targets,
            tx_reporter,
            addr.ssi != PREDEFINED_BROADCAST_GSSI,
        ))
    }

    fn energy_economy_targets(targets: &[u32], energy_saving: &HashMap<u32, EnergySavingAssignment>) -> Vec<u32> {
        targets
            .iter()
            .copied()
            .filter(|issi| energy_saving.get(issi).is_some_and(|assignment| assignment.is_energy_economy()))
            .collect()
    }

    fn group_state_ready_for_tx(
        state: Option<&GroupDeliveryState>,
        addr: TetraAddress,
        ts: TdmaTime,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
        subscribers: &SubscriberRegistry,
        readiness_cache: &mut GroupReadinessCache,
    ) -> bool {
        if let Some(state) = state {
            if !state.active_batch.is_empty() {
                return state.active_batch_listens(ts, energy_saving);
            }
            return state.has_uncovered_listener(ts, energy_saving);
        }

        readiness_cache.any_target_listens(addr, ts, energy_saving, subscribers)
    }

    fn group_stealing_state_ready_for_tx(
        state: Option<&GroupStealingState>,
        addr: TetraAddress,
        ts: TdmaTime,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
        subscribers: &SubscriberRegistry,
        readiness_cache: &mut GroupReadinessCache,
    ) -> bool {
        if let Some(state) = state {
            if !state.active_batch.is_empty() {
                return state.active_batch_listens(ts, energy_saving);
            }
            return state.has_uncovered_listener(ts, energy_saving);
        }

        readiness_cache.any_target_listens(addr, ts, energy_saving, subscribers)
    }

    fn elem_is_ready_for_tx(
        elem: &DlSchedElem,
        ts: TdmaTime,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
        subscribers: &SubscriberRegistry,
        readiness_cache: &mut GroupReadinessCache,
    ) -> bool {
        let Some(addr) = Self::elem_addr(elem) else {
            return true;
        };

        match addr.ssi_type {
            SsiType::Issi => Self::ms_listens_at(energy_saving, addr.ssi, ts),
            SsiType::Gssi => match elem {
                DlSchedElem::Resource(_, _, _, group_state) | DlSchedElem::FragBuf(_, group_state) => {
                    Self::group_state_ready_for_tx(group_state.as_ref(), addr, ts, energy_saving, subscribers, readiness_cache)
                }
                DlSchedElem::Stealing(_, _, _, group_state) => {
                    Self::group_stealing_state_ready_for_tx(group_state.as_ref(), addr, ts, energy_saving, subscribers, readiness_cache)
                }
                _ => Self::group_state_ready_for_tx(None, addr, ts, energy_saving, subscribers, readiness_cache),
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

    fn cmce_dl_payload_from_tma_sdu(sdu: &BitBuffer) -> Option<BitBuffer> {
        let direct = BitBuffer::from_bitbuffer(sdu);
        if matches!(
            direct.peek_bits(5).and_then(|bits| CmcePduTypeDl::try_from(bits).ok()),
            Some(CmcePduTypeDl::DTxGranted | CmcePduTypeDl::DTxCeased | CmcePduTypeDl::DTxInterrupt)
        ) {
            // EN 300 392-2 clauses 14.5.2.2.1 and 23.5 allow assigned-channel
            // floor control directly on STCH. D-TX INTERRUPT can start with the
            // same first four bits as LLC BL-UDATA-FCS, so classify direct
            // floor-control before trying the LLC wrapper.
            return Some(direct);
        }

        let mut wrapped = BitBuffer::from_bitbuffer(sdu);
        if BlUdata::from_bitbuf(&mut wrapped).is_ok() {
            let discriminator = wrapped
                .read_field(3, "mle_protocol_discriminator")
                .ok()
                .and_then(|bits| MleProtocolDiscriminator::try_from(bits).ok());
            if discriminator == Some(MleProtocolDiscriminator::Cmce) {
                return Some(wrapped);
            }
            return None;
        }

        if matches!(
            direct.peek_bits(4).and_then(|bits| LlcPduType::try_from(bits).ok()),
            Some(LlcPduType::BlAck | LlcPduType::BlAckFcs | LlcPduType::BlAdata | LlcPduType::BlAdataFcs)
        ) {
            return None;
        }
        if direct.get_len_remaining() < 10 {
            return None;
        }
        direct
            .peek_bits(5)
            .and_then(|bits| CmcePduTypeDl::try_from(bits).ok())
            .map(|_| direct)
    }

    fn cmce_dl_pdu_type_from_tma_sdu(sdu: &BitBuffer) -> Option<CmcePduTypeDl> {
        let mut payload = Self::cmce_dl_payload_from_tma_sdu(sdu)?;
        payload
            .read_field(5, "cmce_pdu_type_dl")
            .ok()
            .and_then(|bits| CmcePduTypeDl::try_from(bits).ok())
    }

    fn cmce_setup_call_control_pdu(pdu_type: CmcePduTypeDl) -> bool {
        matches!(
            pdu_type,
            CmcePduTypeDl::DAlert
                | CmcePduTypeDl::DCallProceeding
                | CmcePduTypeDl::DConnect
                | CmcePduTypeDl::DConnectAcknowledge
                | CmcePduTypeDl::DDisconnect
                | CmcePduTypeDl::DRelease
                | CmcePduTypeDl::DSetup
                | CmcePduTypeDl::DCallRestore
                | CmcePduTypeDl::CmceFunctionNotSupported
        )
    }

    fn resource_is_cmce_setup_call_control(sdu: &BitBuffer) -> bool {
        Self::cmce_dl_pdu_type_from_tma_sdu(sdu).is_some_and(Self::cmce_setup_call_control_pdu)
    }

    fn stealing_sched_priority(elem: &DlSchedElem) -> StealingSchedPriority {
        let DlSchedElem::Stealing(block, _, _, _) = elem else {
            return StealingSchedPriority::Ordinary;
        };
        let mut mac_probe = BitBuffer::from_bitbuffer(block);
        let Ok(resource) = MacResource::from_bitbuf(&mut mac_probe) else {
            return StealingSchedPriority::Ordinary;
        };
        let uplink_allocated = resource
            .chan_alloc_element
            .as_ref()
            .is_some_and(|chan_alloc| matches!(chan_alloc.ul_dl_assigned, UlDlAssignment::Ul | UlDlAssignment::Both));
        let has_channel_allocation = resource.chan_alloc_element.is_some();

        let mac_payload = BitBuffer::from_bitbuffer_pos(&mac_probe);
        let Some(mut cmce_type_probe) = Self::cmce_dl_payload_from_tma_sdu(&mac_payload) else {
            return if has_channel_allocation {
                StealingSchedPriority::ChannelAllocation
            } else {
                StealingSchedPriority::Ordinary
            };
        };
        let pdu_type = cmce_type_probe
            .read_field(5, "cmce_pdu_type_dl")
            .ok()
            .and_then(|bits| CmcePduTypeDl::try_from(bits).ok());

        match pdu_type {
            Some(CmcePduTypeDl::DTxInterrupt | CmcePduTypeDl::DTxCeased) => {
                // EN 300 392-2 clause 14.5.2.2.1 floor withdrawal must not
                // sit behind lower-value queued floor responses on STCH.
                StealingSchedPriority::FloorWithdraw
            }
            Some(CmcePduTypeDl::DTxGranted) => {
                let Some(mut grant_probe) = Self::cmce_dl_payload_from_tma_sdu(&mac_payload) else {
                    return StealingSchedPriority::Ordinary;
                };
                let Ok(grant) = DTxGranted::from_bitbuf(&mut grant_probe) else {
                    return StealingSchedPriority::Ordinary;
                };
                if grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8 && (uplink_allocated || !has_channel_allocation)
                {
                    // The positive D-TX GRANTED with an uplink channel
                    // allocation, or the already-assigned private-channel
                    // equivalent, is the response that lets the requester
                    // transmit; keep it ahead of RequestQueued/NotGranted storm
                    // traffic in large GSSI cells.
                    StealingSchedPriority::PositiveFloorGrant
                } else if grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8 {
                    // EN 300 392-2 clause 14.5.2.2.1 b): group listeners
                    // must be told when another MS is granted transmit
                    // permission. Keep the GSSI notification near the
                    // positive grant, but below the requester grant itself.
                    StealingSchedPriority::ListenerFloorGrant
                } else {
                    StealingSchedPriority::Ordinary
                }
            }
            _ if has_channel_allocation => StealingSchedPriority::CmceChannelAllocation,
            _ => StealingSchedPriority::Ordinary,
        }
    }

    fn dl_select_ready_stealing_index(
        queue: &[DlSchedElem],
        ts: TdmaTime,
        subscribers: &SubscriberRegistry,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
        readiness_cache: &mut GroupReadinessCache,
    ) -> Option<usize> {
        let mut selected = None;
        let mut selected_priority = StealingSchedPriority::Ordinary;

        for (index, elem) in queue.iter().enumerate() {
            if !matches!(elem, DlSchedElem::Stealing(..))
                || !Self::elem_is_ready_for_tx(elem, ts, energy_saving, subscribers, readiness_cache)
            {
                continue;
            }

            let priority = Self::stealing_sched_priority(elem);
            if selected.is_none() || priority > selected_priority {
                selected = Some(index);
                selected_priority = priority;
                if priority == StealingSchedPriority::FloorWithdraw {
                    break;
                }
            }
        }

        selected
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
                for issi in subscribers.group_member_issis(addr.ssi) {
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
        if let Some(tx_reporter) = tx_reporter {
            tx_reporter.try_mark_discarded();
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

    fn elem_group_repeat_addr(elem: &DlSchedElem) -> Option<(TetraAddress, Option<TxReporter>, Option<TxReporter>)> {
        match elem {
            DlSchedElem::Resource(pdu, _, tx_reporter, Some(group_state)) => {
                pdu.addr.map(|addr| (addr, tx_reporter.clone(), group_state.tx_reporter.clone()))
            }
            DlSchedElem::FragBuf(fragger, Some(group_state)) => fragger.addr().map(|addr| (addr, None, group_state.tx_reporter.clone())),
            DlSchedElem::Stealing(_, addr, tx_reporter, Some(group_state)) => {
                Some((*addr, tx_reporter.clone(), group_state.tx_reporter.clone()))
            }
            _ => None,
        }
    }

    fn dl_drop_queued_gssi_repeats_in_queue(queue: &mut Vec<DlSchedElem>, group_addr: TetraAddress, reason: &str) -> usize {
        let before = queue.len();
        queue.retain(|elem| {
            let Some((addr, tx_reporter, group_reporter)) = Self::elem_group_repeat_addr(elem) else {
                return true;
            };
            if addr != group_addr {
                return true;
            }

            // EN 300 392-2 clauses 14.5.2.2.1, 23.5.2.2.7, and 23.7.6:
            // when the floor changes, retained late EG repeats for the old
            // GSSI signalling snapshot may advertise stale transmitting-party
            // state to a later receive batch. Drop only already-created repeat
            // state; leave fresh group_state=None signalling in the queue.
            tracing::debug!("UMAC: dropping queued GSSI repeat for {} ({})", group_addr, reason);
            Self::mark_reporter_discarded_if_pending(tx_reporter);
            Self::mark_reporter_discarded_if_pending(group_reporter);
            false
        });
        before - queue.len()
    }

    pub fn dl_drop_queued_gssi_repeats(&mut self, group_addr: TetraAddress, reason: &str) -> usize {
        if group_addr.ssi_type != SsiType::Gssi {
            return 0;
        }

        let mut removed = Self::dl_drop_queued_gssi_repeats_in_queue(&mut self.dltx_next_slot_queue, group_addr, reason);
        for queue in &mut self.dltx_queues {
            removed += Self::dl_drop_queued_gssi_repeats_in_queue(queue, group_addr, reason);
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
        self.push_next_slot_queue_bounded(elem, "dltx_next_slot_queue:dl_enqueue_tma_next_frame");
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

    pub fn dl_schedule_tmd_from_ul(&mut self, ts: u8, source_ul_ts: u8, speaker_addr: Option<TetraAddress>, block: Vec<u8>) {
        self.circuits.put_block_from_ul(ts, source_ul_ts, speaker_addr, block);
    }

    pub fn dl_schedule_raw_tch_s_half_slot(&mut self, ts: u8, block_num: PhyBlockNum, type5_bits: Vec<u8>) {
        self.circuits.put_raw_tch_s_half_slot(ts, block_num, type5_bits);
    }

    pub fn dl_schedule_raw_tch_s_half_slot_from_ul(
        &mut self,
        ts: u8,
        source_ul_ts: u8,
        speaker_addr: Option<TetraAddress>,
        block_num: PhyBlockNum,
        type5_bits: Vec<u8>,
    ) {
        self.circuits
            .put_raw_tch_s_half_slot_from_ul(ts, source_ul_ts, speaker_addr, block_num, type5_bits);
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

    /// Participant-scoped means an individual/private bearer. A group circuit
    /// may carry the first speaker ISSI as secondary metadata for EG/listening,
    /// but the GSSI primary keeps the floor-control guard group-scoped.
    pub fn ul_circuit_is_private_participant_scoped(&self, ts: u8) -> bool {
        if !(1..=4).contains(&ts) {
            return false;
        }

        self.circuits.ul[ts as usize - 1]
            .as_ref()
            .is_some_and(|circuit| circuit.is_primary_issi_scoped())
    }

    pub fn ul_circuit_primary_addr(&self, ts: u8) -> Option<TetraAddress> {
        if !(1..=4).contains(&ts) {
            return None;
        }

        self.circuits.ul[ts as usize - 1].as_ref().and_then(|circuit| circuit.active_addr)
    }

    pub fn ul_circuit_issi_participants(&self, ts: u8) -> Vec<TetraAddress> {
        if !(1..=4).contains(&ts) {
            return Vec::new();
        }

        self.circuits.ul[ts as usize - 1]
            .as_ref()
            .map(|circuit| circuit.active_addresses().filter(|addr| addr.ssi_type == SsiType::Issi).collect())
            .unwrap_or_default()
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

    pub fn dl_circuit_peer_ts(&self, ts: u8) -> Option<u8> {
        if !(1..=4).contains(&ts) {
            return None;
        }
        self.circuits.dl[ts as usize - 1].as_ref().and_then(|c| c.peer_ts)
    }

    /// Return the DL media source policy for the UL circuit on `ts`.
    /// `LocalLoopback` = reflect UL back to DL (group/simplex calls).
    /// `LocalParrot` = CMCE records UL media and feeds DL playback later.
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

    pub fn set_circuit_dl_media_source(&mut self, ts: u8, dl_media_source: CircuitDlMediaSource) -> bool {
        self.circuits.set_dl_media_source(ts, dl_media_source)
    }

    pub fn close_circuit(&mut self, dir: Direction, ts: u8) -> Option<Circuit> {
        // Clearing hangtime here is safe: if the circuit is gone, this timeslot is no longer in use.
        if (1..=4).contains(&ts) {
            self.hangtime[ts as usize - 1] = false;
        }
        self.circuits.close_circuit(dir, ts)
    }

    pub fn dl_discard_pending_stealing(&mut self, ts: u8, reason: &str) -> usize {
        let Some(slot) = Self::dl_slot_index(ts, "dl_discard_pending_stealing") else {
            return 0;
        };

        let old_queue = std::mem::take(&mut self.dltx_queues[slot]);
        let mut retained = Vec::with_capacity(old_queue.len());
        let mut discarded = 0;
        for elem in old_queue {
            if matches!(elem, DlSchedElem::Stealing(..)) {
                discarded += 1;
                Self::mark_sched_elem_discarded_if_reported(elem);
            } else {
                retained.push(elem);
            }
        }
        self.dltx_queues[slot] = retained;

        if discarded > 0 {
            tracing::debug!(
                "BsChannelScheduler: discarded {} queued STCH stealing block(s) on closed DL ts {}: {}",
                discarded,
                ts,
                reason
            );
        }
        discarded
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

        self.dl_drop_all_except_stolen(circuit.ts);
        self.preempt_packet_data_assignment_for_voice(circuit.ts);
        self.clear_future_uplink_reservations_for_timeslot(circuit.ts, "voice circuit open");

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
        let slot = Self::dl_slot_index(ts.t, "dl_get_scheduled_resource_for_addr")?;
        let queue = &mut self.dltx_queues[slot];

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
        let Some(slot) = Self::dl_slot_index(ts.t, "dl_take_all_ready_grants_and_acks") else {
            return Vec::new();
        };
        let queue = &mut self.dltx_queues[slot];
        let mut taken = Vec::new();
        let mut retained = Vec::with_capacity(queue.len());
        let mut readiness_cache = GroupReadinessCache::default();

        for elem in std::mem::take(queue) {
            if matches!(
                elem,
                DlSchedElem::Grant(..) | DlSchedElem::PendingGrant(..) | DlSchedElem::RandomAccessAck(_)
            ) && Self::elem_is_ready_for_tx(&elem, ts, energy_saving, subscribers, &mut readiness_cache)
            {
                taken.push(elem);
            } else {
                retained.push(elem);
            }
        }

        *queue = retained;
        taken
    }

    /// Removes all elements from the schedule, except stolen blocks. This function is used
    /// when leaving hangtime to clear out any stale grants, resources, etc that can only be processed in signaling mode,
    /// while keeping stealing blocks that may still need to be transmitted via FACCH.
    /// Discarded elements are reported as such via tx_reporter if available. Returns true if elements were discarded.
    pub fn dl_drop_all_except_stolen(&mut self, timeslot: u8) -> bool {
        let Some(slot) = Self::dl_slot_index(timeslot, "dl_drop_all_except_stolen") else {
            return false;
        };
        let dropped_grant_addrs: HashSet<TetraAddress> = self.dltx_queues[slot]
            .iter()
            .filter_map(|elem| match elem {
                DlSchedElem::Grant(addr, _, _) | DlSchedElem::PendingGrant(addr, _, _) => Some(*addr),
                _ => None,
            })
            .collect();
        let mut item_was_discarded = false;

        let old_queue = std::mem::take(&mut self.dltx_queues[slot]);
        let mut retained = Vec::with_capacity(old_queue.len());
        for elem in old_queue {
            if matches!(elem, DlSchedElem::Stealing(..)) {
                retained.push(elem);
                continue;
            }

            // Found a to-be-discarded element.
            // Log and call tx_reporter::mark_discarded() if applicable.
            // Logged at debug because this fires during normal hangtime entry/exit
            // races and isn't an anomaly worth surfacing as a warning. Per
            // proxiboi69 in MidnightBlueLabs/tetra-bluestation PR #85.
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
                    if dropped_grant_addrs.contains(&addr) {
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
                        // random_access_flag=true (ETSI 21.4.3.1). Keep the
                        // local queue deduplicated and bounded so repeated
                        // random access from thousands of group listeners cannot
                        // grow scheduler state without limit.
                        self.store_pending_ra_ack(slot, timeslot, addr);
                    }
                }

                DlSchedElem::Grant(..) | DlSchedElem::PendingGrant(..) | DlSchedElem::Broadcast(_) => {
                    // Silently dropped as internal or not equipped with a tx_reporter
                }
                _ => unreachable!(),
            }
        }
        self.dltx_queues[slot] = retained;

        item_was_discarded
    }

    pub fn dl_integrate_sched_elems_for_timeslot(
        &mut self,
        ts: TdmaTime,
        subscribers: &SubscriberRegistry,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
    ) {
        self.dl_integrate_sched_elems_for_timeslot_with_allocator(ts, subscribers, energy_saving, None);
    }

    fn dl_integrate_sched_elems_for_timeslot_with_allocator(
        &mut self,
        ts: TdmaTime,
        subscribers: &SubscriberRegistry,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
        timeslot_alloc: Option<&TimeslotAllocator>,
    ) {
        let Some(slot) = Self::dl_slot_index(ts.t, "dl_integrate_sched_elems_for_timeslot") else {
            return;
        };
        if !Self::can_carry_scheduled_schf(ts) {
            // EN 300 392-2 clauses 9.5.2 and 9.5.3 reserve fixed frame-18
            // BSCH/BNCH positions. Keep pending MAC-RESOURCE grants queued
            // until a frame-18 SCH/F opportunity or an ordinary frame.
            return;
        }

        // Remove all grants and acks from queue and collect them into a vec
        let grants_and_acks = self.dl_take_all_ready_grants_and_acks(ts, subscribers, energy_saving);
        let mut resource_indexes: HashMap<TetraAddress, usize> = self.dltx_queues[slot]
            .iter()
            .enumerate()
            .filter_map(|(index, elem)| match elem {
                DlSchedElem::Resource(pdu, _, _, _) => pdu.addr.map(|addr| (addr, index)),
                _ => None,
            })
            .collect();

        // Process grants and acks
        for elem in grants_and_acks {
            let elem = match elem {
                DlSchedElem::PendingGrant(addr, debt, origin) => {
                    if !self.packet_data_pending_grant_origin_is_valid(addr, origin, timeslot_alloc) {
                        tracing::warn!(
                            "dl_integrate_sched_elems_for_timeslot: dropping stale packet-data pending grant for addr {} debt {:?} origin {:?} at tx {}",
                            addr,
                            debt,
                            origin,
                            ts
                        );
                        continue;
                    }
                    match self.ul_process_pending_grant_from_with_allocator(ts, ts.t, addr, debt, timeslot_alloc) {
                        Some((grant, usage_marker, granted_slots)) => {
                            if let Some(remaining_debt) = debt.remaining_after_grant(granted_slots) {
                                self.dl_defer_pending_grant_next_frame(addr, remaining_debt, origin);
                            }
                            DlSchedElem::Grant(addr, grant, usage_marker)
                        }
                        None => {
                            tracing::warn!(
                                "dl_integrate_sched_elems_for_timeslot: no grant opportunity for addr {} debt {:?} at tx {}",
                                addr,
                                debt,
                                ts
                            );
                            // EN 300 392-2 23.5.2.2.2 defines granting delay 1111 as
                            // "Wait for another slot grant", which restarts T.206
                            // without granting slots. Keep the EG-gated grant pending
                            // until a transmit window also has usable uplink capacity.
                            self.dl_defer_pending_grant_next_frame(addr, debt, origin);
                            let wait_grant = BasicSlotgrant {
                                capacity_allocation: debt.wait_capacity_allocation(),
                                granting_delay: BasicSlotgrantGrantingDelay::WaitForAnotherSlotgrantMessage,
                            };
                            DlSchedElem::Grant(addr, wait_grant, None)
                        }
                    }
                }
                elem => elem,
            };

            // Try to find existing resource for this address
            let addr = match &elem {
                DlSchedElem::Grant(addr, _, _) | DlSchedElem::PendingGrant(addr, _, _) => *addr,
                DlSchedElem::RandomAccessAck(addr) => *addr,
                _ => unreachable!("BUG: unhandled match variant -- should never be reached"),
            };
            match resource_indexes.get(&addr).copied() {
                Some(index) => {
                    let DlSchedElem::Resource(pdu, _sdu, _repeat, _) = &mut self.dltx_queues[slot][index] else {
                        unreachable!("BUG: resource index no longer points to a MAC-RESOURCE");
                    };
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
                            let mut pdu = Self::dl_make_minimal_resource(&addr, Some(grant.clone()), false);
                            pdu.usage_marker = *usage_marker;
                            pdu
                        }
                        DlSchedElem::RandomAccessAck(_) => {
                            tracing::debug!(
                                "dl_integrate_sched_elems_for_timeslot: Creating new resource for addr {} with ack",
                                addr
                            );
                            Self::dl_make_minimal_resource(&addr, None, true)
                        }
                        _ => unreachable!("BUG: unhandled match variant -- should never be reached"),
                    };

                    // Push new resource into the queue. These do not need a tx_reporter
                    let dlsched_res = DlSchedElem::Resource(pdu, BitBuffer::new(0), None, None);
                    self.push_sched_queue_bounded(slot, dlsched_res, "dltx_queues:dl_integrate_sched_elems_for_timeslot");
                    if let Some(index) = self.dltx_queues[slot]
                        .iter()
                        .rposition(|elem| matches!(elem, DlSchedElem::Resource(pdu, _, _, _) if pdu.addr == Some(addr)))
                    {
                        resource_indexes.insert(addr, index);
                    }
                }
            }
        }
    }

    fn dl_build_block_from_signalling_schedule(
        &mut self,
        ts: TdmaTime,
        subscribers: &SubscriberRegistry,
        energy_saving: &mut HashMap<u32, EnergySavingAssignment>,
        timeslot_alloc: &mut Option<&mut TimeslotAllocator>,
    ) -> Option<BitBuffer> {
        let mut buf_opt = None;
        let mut readiness_cache = GroupReadinessCache::default();

        while !self.dltx_queues[ts.t as usize - 1].is_empty() {
            let opt = self.dl_take_prioritized_sched_item_with_cache(ts, subscribers, energy_saving, &mut readiness_cache);

            match opt {
                Some(sched_elem) => {
                    match sched_elem {
                        DlSchedElem::Broadcast(_) => {
                            unimplemented_log!("finalize_ts_for_tick: Broadcast scheduling not implemented");
                        }

                        DlSchedElem::Resource(mut pdu, sdu, tx_reporter, group_state) => {
                            if !Self::packet_data_resource_allowed_on_allocated_slot(ts.t, &pdu, &sdu, timeslot_alloc.as_deref()) {
                                Self::mark_reporter_discarded_if_pending(tx_reporter);
                                continue;
                            }
                            if !self.adapt_packet_data_channel_allocation_for_voice_priority_with_allocator(
                                &mut pdu,
                                &sdu,
                                timeslot_alloc.as_deref(),
                            ) {
                                Self::mark_reporter_discarded_if_pending(tx_reporter);
                                continue;
                            }
                            let addr = pdu.addr;
                            let packet_data_assignment_update = Self::packet_data_assignment_update_from_resource(&pdu, &sdu);
                            let is_all_ones_broadcast = Self::is_predefined_broadcast_gssi(addr);
                            let mut group_state = group_state.or_else(|| {
                                addr.and_then(|addr| {
                                    Self::group_state_for_resource(
                                        addr,
                                        &pdu,
                                        &sdu,
                                        tx_reporter.clone(),
                                        subscribers,
                                        &mut readiness_cache,
                                        energy_saving,
                                    )
                                })
                            });
                            if let Some(state) = group_state.as_mut() {
                                if let Some(addr) = addr {
                                    Self::retain_current_group_delivery_targets(
                                        state,
                                        addr,
                                        subscribers,
                                        &mut readiness_cache,
                                        energy_saving,
                                    );
                                }
                                state.begin_batch_if_needed(ts, energy_saving);
                            }
                            let fragger_reporter = match group_state.as_ref() {
                                Some(state) if is_all_ones_broadcast => state.tx_reporter.clone(),
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
                                if is_all_ones_broadcast && state.tx_reporter.as_ref().is_some_and(TxReporter::is_transmitted) {
                                    // TMA-REPORT is local MAC progress per
                                    // EN 300 392-2 20.4.1.1.3. For the
                                    // predefined all-ones broadcast address,
                                    // report once this TM-SDU has been fully
                                    // emitted, while keeping EG repeats as
                                    // best-effort receive coverage.
                                    state.tx_reporter = None;
                                }
                            }
                            if !fully_transmitted {
                                // Fragmentation was started and we have more chunks to send
                                // Enqueue fragger with remaining data for retrieval next frame
                                self.dl_enqueue_tma_frag_next_frame_with_group_state(fragger, group_state);
                            } else if let Some(state) = group_state
                                && !state.is_complete()
                            {
                                self.dl_enqueue_group_repeat_next_frame(state);
                            } else if let Some(update) = packet_data_assignment_update {
                                self.apply_packet_data_assignment_update_with_allocator(update, ts, timeslot_alloc);
                            }
                            buf_opt = Some(buf);
                        }

                        DlSchedElem::FragBuf(mut fragger, mut group_state) => {
                            let addr = fragger.addr();
                            let is_all_ones_broadcast = Self::is_predefined_broadcast_gssi(addr);
                            if let Some(state) = group_state.as_mut() {
                                if let Some(addr) = addr {
                                    Self::retain_current_group_delivery_targets(
                                        state,
                                        addr,
                                        subscribers,
                                        &mut readiness_cache,
                                        energy_saving,
                                    );
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
                                if is_all_ones_broadcast && state.tx_reporter.as_ref().is_some_and(TxReporter::is_transmitted) {
                                    state.tx_reporter = None;
                                }
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
            Self::enforce_sched_queue_cap(current, MAX_DLSCHED_ELEMS_PER_TIMESLOT, "dltx_queues:next_slot_merge");
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
            CircuitTxBlock::AcElp { block, .. } => {
                let mut buf = BitBuffer::from_vec(block);
                // Raw ACELP speech (274 bits for TCH/S).
                // Clamp to TCH_S_CAP as Vec may be larger (e.g. 280 bits).
                buf.set_raw_end(buf.get_raw_start() + TCH_S_CAP);
                Some(DlTchBlock::AcElp(buf))
            }
            CircuitTxBlock::RawTchSHalfSlot { block_num, type5_bits, .. } => {
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

        let mut readiness_cache = GroupReadinessCache::default();

        // Check for FACCH/stealing: take a queued Stealing item (highest priority signaling)
        let (stch_opt, stealing_addr_opt, tx_reporter_opt, group_state_opt) = {
            if ts.t >= 1 && (ts.t as usize) <= self.dltx_queues.len() {
                self.prune_completed_stale_group_states_for_slot(ts.t as usize - 1, subscribers, &mut readiness_cache, energy_saving);
            }
            let q = &mut self.dltx_queues[ts.t as usize - 1];
            if let Some(i) = Self::dl_select_ready_stealing_index(q, ts, subscribers, energy_saving, &mut readiness_cache) {
                match q.remove(i) {
                    DlSchedElem::Stealing(buf, addr, tx_reporter, group_state) => (Some(buf), Some(addr), tx_reporter, group_state),
                    _ => unreachable!(),
                }
            } else {
                (None, None, None, None)
            }
        };

        if let (Some(block), Some(addr)) = (stch_opt.as_ref(), stealing_addr_opt) {
            self.log_selected_stch_d_tx_granted_diag(ts, block, addr, tch_buf.is_some());
        }

        // Trace queued signaling that is not yet eligible for stealing.
        if stch_opt.is_none() && !self.dltx_queues[ts.t as usize - 1].is_empty() {
            tracing::debug!("dl_build_traffic_block: queued signaling on ts {} but no stealing item", ts.t);
        }

        let mut should_report_transmitted = stch_opt.is_some();
        if let (Some(block), Some(addr)) = (stch_opt.as_ref(), stealing_addr_opt)
            && addr.ssi_type == SsiType::Gssi
        {
            let is_all_ones_broadcast = Self::is_predefined_broadcast_gssi(Some(addr));
            let targets = readiness_cache.targets_for(addr, subscribers);
            let energy_economy_targets = Self::energy_economy_targets(targets, energy_saving);
            if energy_economy_targets.is_empty() {
                Self::mark_stealing_signalling_activity(addr, None, ts, subscribers, energy_saving);
            } else {
                let mut group_state = group_state_opt.unwrap_or_else(|| {
                    GroupStealingState::new(
                        energy_economy_targets,
                        tx_reporter_opt.clone(),
                        addr.ssi != PREDEFINED_BROADCAST_GSSI,
                    )
                });
                if !group_state.targets.is_empty() {
                    Self::retain_current_group_stealing_targets(&mut group_state, addr, subscribers, &mut readiness_cache, energy_saving);
                }
                if !group_state.targets.is_empty() {
                    group_state.begin_batch_if_needed(ts, energy_saving);
                    Self::mark_stealing_signalling_activity(addr, Some(&group_state), ts, subscribers, energy_saving);
                    group_state.mark_batch_covered();
                    let coverage_complete = group_state.is_complete();
                    should_report_transmitted = coverage_complete || is_all_ones_broadcast;
                    if is_all_ones_broadcast {
                        group_state.tx_reporter = None;
                    }
                    if !coverage_complete {
                        self.dl_requeue_group_stealing(ts.t, block.clone(), addr, group_state);
                    }
                } else {
                    Self::mark_stealing_signalling_activity(addr, None, ts, subscribers, energy_saving);
                }
            }
        } else if let Some(addr) = stealing_addr_opt {
            Self::mark_stealing_signalling_activity(addr, None, ts, subscribers, energy_saving);
        }

        if should_report_transmitted && let Some(tx_reporter) = tx_reporter_opt {
            if !tx_reporter.try_mark_transmitted() {
                tracing::debug!(
                    "BsChannelScheduler: ignoring late stealing complete-transmission report for reporter already in {:?}",
                    tx_reporter.get_state()
                );
            }
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
        let mut readiness_cache = GroupReadinessCache::default();
        self.dl_take_prioritized_sched_item_with_cache(ts, subscribers, energy_saving, &mut readiness_cache)
    }

    fn dl_take_prioritized_sched_item_with_cache(
        &mut self,
        ts: TdmaTime,
        subscribers: &SubscriberRegistry,
        energy_saving: &HashMap<u32, EnergySavingAssignment>,
        readiness_cache: &mut GroupReadinessCache,
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
        self.prune_completed_stale_group_states_for_slot(slot, subscribers, readiness_cache, energy_saving);
        let Some(q) = self.dltx_queues.get_mut(slot) else {
            return None;
        };

        // Return grants first, but only when the addressed MS should be listening.
        if let Some(i) = q.iter().position(|e| {
            matches!(e, DlSchedElem::Grant(..) | DlSchedElem::PendingGrant(..))
                && Self::elem_is_ready_for_tx(e, ts, energy_saving, subscribers, readiness_cache)
        }) {
            return Some(q.remove(i));
        }

        // Channel allocations carry call-control resource assignment. Keep
        // them ahead of ready EG grant traffic so a private-call setup is not
        // held behind ordinary reservation churn once the addressed MS can
        // receive it (EN 300 392-2 clauses 14, 21.5.2 and 23.5.2.2.7).
        if let Some(i) = q.iter().position(|e| {
            Self::elem_has_channel_allocation(e) && Self::elem_is_ready_for_tx(e, ts, energy_saving, subscribers, readiness_cache)
        }) {
            return Some(q.remove(i));
        }

        // Grants and random-access ACKs are integrated into MAC-RESOURCE before
        // SCH/F building. Keep those resources ahead of fragmentation backlog so
        // EN 300 392-2 21.4.3.1 ACK/grant timing is not delayed by ordinary data.
        if let Some(i) = q.iter().position(|e| {
            Self::elem_has_integrated_grant_or_ack(e) && Self::elem_is_ready_for_tx(e, ts, energy_saving, subscribers, readiness_cache)
        }) {
            return Some(q.remove(i));
        }

        // EN 300 392-2 clause 14.5.1 call setup/release progress is CMCE
        // control-plane signalling. Clause 23.5.2.2.7 still gates delivery to
        // the addressed MS's EG receive opportunity; once that opportunity is
        // present, keep D-SETUP/D-RELEASE style control ahead of ordinary
        // SDS/data and fragmented backlog.
        if let Some(i) = q.iter().position(|e| {
            matches!(e, DlSchedElem::Resource(_, sdu, _, _) if Self::resource_is_cmce_setup_call_control(sdu))
                && Self::elem_is_ready_for_tx(e, ts, energy_saving, subscribers, readiness_cache)
        }) {
            return Some(q.remove(i));
        }

        // Return FragBufs next, but only when the addressed MS should be listening.
        if let Some(i) = q.iter().position(|e| {
            matches!(e, DlSchedElem::FragBuf(_, _)) && Self::elem_is_ready_for_tx(e, ts, energy_saving, subscribers, readiness_cache)
        }) {
            return Some(q.remove(i));
        }

        // Return Resources last, but only when the addressed MS should be listening.
        if let Some(i) = q.iter().position(|e| {
            matches!(e, DlSchedElem::Resource(_, _, _, _)) && Self::elem_is_ready_for_tx(e, ts, energy_saving, subscribers, readiness_cache)
        }) {
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
        let mut timeslot_alloc = None;
        self.finalize_ts_for_tick_inner(subscribers, energy_saving, &mut timeslot_alloc)
    }

    pub fn finalize_ts_for_tick_with_timeslot_allocator(
        &mut self,
        subscribers: &SubscriberRegistry,
        energy_saving: &mut HashMap<u32, EnergySavingAssignment>,
        timeslot_alloc: &mut TimeslotAllocator,
    ) -> TmvUnitdataReqSlot {
        let mut timeslot_alloc = Some(timeslot_alloc);
        self.finalize_ts_for_tick_inner(subscribers, energy_saving, &mut timeslot_alloc)
    }

    fn finalize_ts_for_tick_inner(
        &mut self,
        subscribers: &SubscriberRegistry,
        energy_saving: &mut HashMap<u32, EnergySavingAssignment>,
        timeslot_alloc: &mut Option<&mut TimeslotAllocator>,
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
        self.expire_packet_data_assignments_with_allocator(ts, timeslot_alloc);

        let dl_circuit_active = self.circuits.is_active(Direction::Dl, ts.t) && ts.f != 18;
        let ul_circuit_active = self.circuits.is_active(Direction::Ul, ts.t) && ts.f != 18;
        let packet_data_assignment_active = self
            .packet_data_assignment_for_timeslot_with_allocator(ts, timeslot_alloc.as_deref())
            .is_some();

        // During hangtime we stop sending traffic frames and switch to signalling mode.
        // Keep traffic mode while FACCH/stealing is still queued for delivery.
        let hang_effective = if (2..=4).contains(&ts.t) {
            self.is_hangtime_effective(ts.t)
        } else {
            false
        };

        let dl_is_traffic = dl_circuit_active && !hang_effective;
        let ul_is_traffic = ul_circuit_active && !hang_effective;
        let group_positive_floor_grant_pending = (2..=4).contains(&ts.t) && self.has_pending_group_positive_floor_grant_stealing(ts.t);

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
                tracing::debug!(
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
                            tracing::debug!(
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
                        tracing::debug!(
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
            self.dl_integrate_sched_elems_for_timeslot_with_allocator(ts, subscribers, energy_saving, timeslot_alloc.as_deref());

            // Fill our signalling block with scheduled items (if any)
            let buf = self.dl_build_block_from_signalling_schedule(ts, subscribers, energy_saving, timeslot_alloc);
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
                } else if packet_data_assignment_active && Self::can_carry_scheduled_schf(ts) {
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
        elem.bbk = Some(self.generate_bbk_block_with_group_floor_grant(ts, group_positive_floor_grant_pending, timeslot_alloc.as_deref()));

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

            // Keep the primary MCCH BNCH (TS1) on SYSINFO1 so cold MSs always
            // see the default definition for access code A. Auxiliary BNCH
            // opportunities on even timeslots carry SYSINFO2/extended services.
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
        let group_positive_floor_grant_pending = (2..=4).contains(&ts.t) && self.has_pending_group_positive_floor_grant_stealing(ts.t);
        self.generate_bbk_block_with_group_floor_grant(ts, group_positive_floor_grant_pending, None)
    }

    fn generate_bbk_block_with_group_floor_grant(
        &self,
        ts: TdmaTime,
        group_positive_floor_grant_pending: bool,
        timeslot_alloc: Option<&TimeslotAllocator>,
    ) -> TmvUnitdataReq {
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
                    // The individually addressed group D-TX GRANTED is the
                    // opposite edge: it switches the granted MS U-plane on, so
                    // AACH must advertise traffic while that STCH is delivered.
                    let in_hangtime = (2..=4).contains(&ts.t) && self.hangtime[ts.t as usize - 1] && !group_positive_floor_grant_pending;

                    if in_hangtime && (dl_traffic_usage.is_some() || ul_traffic_usage.is_some()) {
                        aach.dl_usage = AccessAssignDlUsage::AssignedControl;
                        // AssignedOnly (Header 2) allows random access for MSs on
                        // the assigned channel while blocking common control MSs.
                        aach.ul_usage = AccessAssignUlUsage::AssignedOnly;
                        aach.f2_af = Some(AccessField {
                            access_code: 0,
                            base_frame_len: 4,
                        });
                    } else if dl_traffic_usage.is_none()
                        && ul_traffic_usage.is_none()
                        && let Some(packet_data_assignment) = self.packet_data_assignment_for_timeslot_with_allocator(ts, timeslot_alloc)
                    {
                        aach.dl_usage = AccessAssignDlUsage::AssignedControl;
                        aach.ul_usage = if matches!(packet_data_assignment.ul_dl_assigned, UlDlAssignment::Ul | UlDlAssignment::Both) {
                            AccessAssignUlUsage::AssignedOnly
                        } else {
                            AccessAssignUlUsage::CommonAndAssigned
                        };
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

            if group_positive_floor_grant_pending && (2..=4).contains(&ts.t) {
                tracing::info!(
                    "UMAC RF diag: AACH group-positive-grant pending ts={} dl_usage={} ul_usage={} hangtime={} circuit_usage={{dl:{:?},ul:{:?}}}",
                    ts,
                    aach.dl_usage,
                    aach.ul_usage,
                    self.hangtime[ts.t as usize - 1],
                    dl_traffic_usage,
                    ul_traffic_usage
                );
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
        TxReporter, TxState,
        address::{SsiType, TetraAddress},
        debug::setup_logging_default,
    };

    use tetra_pdus::{
        cmce::{
            enums::call_timeout::CallTimeout,
            pdus::{d_connect_acknowledge::DConnectAcknowledge, d_tx_ceased::DTxCeased, d_tx_interrupt::DTxInterrupt},
        },
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

    fn test_sndcp_llc_sdu() -> BitBuffer {
        let mut sdu = BitBuffer::new_autoexpand(16);
        BlUdata { has_fcs: false }.to_bitbuf(&mut sdu);
        sdu.write_bits(MleProtocolDiscriminator::Sndcp.into_raw(), 3);
        sdu.write_bits(0b0100, 4);
        sdu.seek(0);
        sdu
    }

    fn test_sndcp_al_data_sdu(ss: u8) -> BitBuffer {
        let mut sdu = BitBuffer::new_autoexpand(32);
        AlData {
            final_segment: false,
            acknowledgement_requested: true,
            ns: 0,
            ss,
        }
        .to_bitbuf(&mut sdu);
        sdu.write_bits(MleProtocolDiscriminator::Sndcp.into_raw(), 3);
        sdu.write_bits(0b0100, 4);
        sdu.seek(0);
        sdu
    }

    fn test_cmce_llc_sdu() -> BitBuffer {
        let mut sdu = BitBuffer::new_autoexpand(16);
        BlUdata { has_fcs: false }.to_bitbuf(&mut sdu);
        sdu.write_bits(MleProtocolDiscriminator::Cmce.into_raw(), 3);
        sdu.write_bits(0b0100, 4);
        sdu.seek(0);
        sdu
    }

    fn queue_has_resource_for_addr_on_ts(sched: &BsChannelScheduler, ts: u8, addr: TetraAddress) -> bool {
        sched.dltx_queues[ts as usize - 1]
            .iter()
            .any(|elem| matches!(elem, DlSchedElem::Resource(pdu, _, _, _) if pdu.addr == Some(addr)))
    }

    fn test_packet_data_channel_allocation_resource(
        issi: u32,
        ts_assigned: [bool; 4],
        usage_marker: Option<u8>,
        alloc_type: ChanAllocType,
        ul_dl_assigned: UlDlAssignment,
    ) -> (MacResource, BitBuffer) {
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: issi,
        };
        let sdu = test_sndcp_llc_sdu();
        let mut pdu = MacResource {
            fill_bits: false,
            pos_of_grant: 0,
            encryption_mode: 0,
            random_access_flag: false,
            length_ind: 0,
            addr: Some(addr),
            event_label: None,
            usage_marker,
            power_control_element: None,
            slot_granting_element: None,
            chan_alloc_element: Some(ChanAllocElement {
                alloc_type,
                ts_assigned,
                ul_dl_assigned,
                clch_permission: matches!(ul_dl_assigned, UlDlAssignment::Ul | UlDlAssignment::Both),
                cell_change_flag: false,
                carrier_num: 1001,
                ext: None,
                mon_pattern: 0,
                frame18_mon_pattern: Some(0),
            }),
        };
        pdu.update_len_and_fill_ind(sdu.get_len());
        (pdu, sdu)
    }

    fn test_cmce_stch_block(addr: TetraAddress, mut sdu: BitBuffer, ul_dl_assigned: UlDlAssignment) -> BitBuffer {
        const STCH_CAP: usize = 124;

        sdu.seek(0);

        let mut timeslots = [false; 4];
        timeslots[1] = true;
        let mut mac_pdu = MacResource {
            fill_bits: false,
            pos_of_grant: 0,
            encryption_mode: 0,
            random_access_flag: false,
            length_ind: 0,
            addr: Some(addr),
            event_label: None,
            usage_marker: Some(6),
            power_control_element: None,
            slot_granting_element: None,
            chan_alloc_element: Some(ChanAllocElement {
                alloc_type: ChanAllocType::Replace,
                ts_assigned: timeslots,
                ul_dl_assigned,
                clch_permission: matches!(ul_dl_assigned, UlDlAssignment::Ul | UlDlAssignment::Both),
                cell_change_flag: false,
                carrier_num: 1001,
                ext: None,
                mon_pattern: 1,
                frame18_mon_pattern: None,
            }),
        };
        let sdu_len = sdu.get_len();
        let header_len = mac_pdu.compute_header_len();
        let fill_bits = crate::umac::subcomp::fillbits::addition::compute_required(header_len + sdu_len, STCH_CAP);
        let total_len = header_len + sdu_len + fill_bits;
        assert!(total_len <= STCH_CAP, "test D-TX GRANTED STCH must fit in one stealing block");
        mac_pdu.length_ind = (total_len / 8) as u8;
        mac_pdu.fill_bits = fill_bits > 0;

        let mut stch_block = BitBuffer::new(STCH_CAP);
        mac_pdu.to_bitbuf(&mut stch_block);
        stch_block.copy_bits(&mut sdu, sdu_len);
        crate::umac::subcomp::fillbits::addition::write(&mut stch_block, Some(fill_bits));
        stch_block
    }

    fn test_d_tx_granted_stch_block(
        addr: TetraAddress,
        transmission_grant: TransmissionGrant,
        ul_dl_assigned: UlDlAssignment,
    ) -> BitBuffer {
        let mut sdu = BitBuffer::new_autoexpand(40);
        DTxGranted {
            call_identifier: 7,
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
        .expect("serialize compact D-TX GRANTED");
        test_cmce_stch_block(addr, sdu, ul_dl_assigned)
    }

    fn test_llc_wrapped_d_tx_granted_stch_block(
        addr: TetraAddress,
        transmission_grant: TransmissionGrant,
        ul_dl_assigned: UlDlAssignment,
    ) -> BitBuffer {
        let mut cmce_sdu = BitBuffer::new_autoexpand(40);
        DTxGranted {
            call_identifier: 7,
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
        .to_bitbuf(&mut cmce_sdu)
        .expect("serialize compact D-TX GRANTED");
        cmce_sdu.seek(0);

        let mut sdu = BitBuffer::new_autoexpand(64);
        BlUdata { has_fcs: false }.to_bitbuf(&mut sdu);
        sdu.write_bits(MleProtocolDiscriminator::Cmce.into_raw(), 3);
        let cmce_sdu_len = cmce_sdu.get_len();
        sdu.copy_bits(&mut cmce_sdu, cmce_sdu_len);
        sdu.seek(0);

        test_cmce_stch_block(addr, sdu, ul_dl_assigned)
    }

    fn test_llc_wrapped_d_connect_ack_stch_block(addr: TetraAddress, ul_dl_assigned: UlDlAssignment) -> BitBuffer {
        let d_connect_ack = DConnectAcknowledge {
            call_identifier: 0x22,
            call_time_out: CallTimeout::T2m,
            transmission_grant: TransmissionGrant::Granted,
            transmission_request_permission: false,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(64);
        BlUdata { has_fcs: false }.to_bitbuf(&mut sdu);
        sdu.write_bits(MleProtocolDiscriminator::Cmce.into_raw(), 3);
        d_connect_ack.to_bitbuf(&mut sdu).expect("test D-CONNECT ACK should serialize");
        sdu.seek(0);

        test_cmce_stch_block(addr, sdu, ul_dl_assigned)
    }

    #[test]
    fn test_mle_bl_udata_is_not_classified_as_cmce_d_info() {
        let mut sdu = BitBuffer::new_autoexpand(16);
        BlUdata { has_fcs: false }.to_bitbuf(&mut sdu);
        sdu.write_bits(MleProtocolDiscriminator::Mle.into_raw(), 3);
        sdu.write_bits(0, 8);
        sdu.seek(0);

        // BL-UDATA(false) starts with 0010 and the MLE discriminator starts
        // with 1, so the first five bits are 00101, the raw CMCE D-INFO type.
        // EN 300 392-2 clause 18.5.21 still makes this an MLE payload, not a
        // direct CMCE PDU; classifier code must not fall through after reading
        // a non-CMCE discriminator.
        assert!(BsChannelScheduler::cmce_dl_payload_from_tma_sdu(&sdu).is_none());
    }

    fn test_d_tx_interrupt_stch_block(addr: TetraAddress, ul_dl_assigned: UlDlAssignment) -> BitBuffer {
        let mut sdu = BitBuffer::new_autoexpand(40);
        DTxInterrupt {
            call_identifier: 7,
            transmission_grant: TransmissionGrant::GrantedToOtherUser.into_raw() as u8,
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
        .expect("serialize compact D-TX INTERRUPT");
        test_cmce_stch_block(addr, sdu, ul_dl_assigned)
    }

    fn test_d_tx_ceased_stch_block(addr: TetraAddress, call_identifier: u16, ul_dl_assigned: UlDlAssignment) -> BitBuffer {
        let mut sdu = BitBuffer::new_autoexpand(32);
        DTxCeased {
            call_identifier,
            transmission_request_permission: false,
            notification_indicator: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        }
        .to_bitbuf(&mut sdu)
        .expect("serialize compact D-TX CEASED");
        test_cmce_stch_block(addr, sdu, ul_dl_assigned)
    }

    fn test_ordinary_resource_for_issi(issi: u32, sdu_bits: usize) -> (MacResource, BitBuffer) {
        let (mut pdu, sdu) = test_resource_for_issi(issi, sdu_bits);
        pdu.random_access_flag = false;
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

    #[test]
    fn test_downlink_scheduler_discards_reported_ordinary_resource_when_queue_cap_is_reached() {
        let mut sched = get_testing_slotter();
        let first_reporter = TxReporter::new_unacked();

        for offset in 0..=MAX_DLSCHED_ELEMS_PER_TIMESLOT {
            let (pdu, sdu) = test_ordinary_resource_for_issi(300_000 + offset as u32, 8);
            let reporter = (offset == 0).then(|| first_reporter.clone());
            sched.dl_enqueue_tma(pdu, sdu, reporter);
        }

        // Local BS robustness guard: ordinary downlink signalling backlog is
        // bounded so a large group/recovery storm cannot grow scheduler memory
        // without limit. This does not define an over-air ETSI PDU change; the
        // discarded TMA request is reported through TxReporter.
        assert_eq!(sched.dltx_queues[0].len(), MAX_DLSCHED_ELEMS_PER_TIMESLOT);
        assert_eq!(first_reporter.get_state(), TxState::Discarded);
    }

    #[test]
    fn test_downlink_scheduler_discards_reported_deferred_resource_when_next_slot_cap_is_reached() {
        let mut sched = get_testing_slotter();
        let first_reporter = TxReporter::new_unacked();

        for offset in 0..=MAX_DLSCHED_NEXT_SLOT_ELEMS {
            let (pdu, sdu) = test_ordinary_resource_for_issi(320_000 + offset as u32, 8);
            let reporter = (offset == 0).then(|| first_reporter.clone());
            sched.dl_enqueue_tma_next_frame(pdu, sdu, reporter);
        }

        // Same local robustness guard as the live downlink queue, applied to
        // deferred next-frame signalling before it can be merged back into TS1.
        assert_eq!(sched.dltx_next_slot_queue.len(), MAX_DLSCHED_NEXT_SLOT_ELEMS);
        assert_eq!(first_reporter.get_state(), TxState::Discarded);
    }

    #[test]
    fn test_downlink_scheduler_backpressure_preserves_grants_over_ordinary_resources() {
        let mut sched = get_testing_slotter();
        for offset in 0..MAX_DLSCHED_ELEMS_PER_TIMESLOT {
            let (pdu, sdu) = test_ordinary_resource_for_issi(310_000 + offset as u32, 8);
            sched.dl_enqueue_tma(pdu, sdu, None);
        }

        let grant_addr = TetraAddress::issi(399_999);
        let grant = BasicSlotgrant {
            capacity_allocation: BasicSlotgrantCapAlloc::FirstSubslotGranted,
            granting_delay: BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity,
        };
        sched.dl_enqueue_grant(1, grant_addr, grant, None);

        // EN 300 392-2 clauses 21.4.3.1 and 23.5.2.2.2 make random-access
        // ACK/grant timing critical. Backpressure may discard ordinary queued
        // downlinks, but it must preserve the grant lane.
        assert_eq!(sched.dltx_queues[0].len(), MAX_DLSCHED_ELEMS_PER_TIMESLOT);
        assert!(
            sched.dltx_queues[0]
                .iter()
                .any(|elem| matches!(elem, DlSchedElem::Grant(addr, _, _) if *addr == grant_addr)),
            "critical grant must survive downlink queue backpressure"
        );
    }

    #[test]
    fn test_floor_withdraw_duplicate_coalesces_and_keeps_latest_reporter() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let traffic_ts = 2;
        let gssi = TetraAddress::new(226_333, SsiType::Gssi);
        let old_reporter = TxReporter::new_unacked();
        let latest_reporter = TxReporter::new_unacked();

        sched.dl_enqueue_stealing(
            traffic_ts,
            test_d_tx_ceased_stch_block(gssi, 7, UlDlAssignment::Dl),
            gssi,
            Some(old_reporter.clone()),
        );
        sched.dl_enqueue_stealing(
            traffic_ts,
            test_d_tx_ceased_stch_block(gssi, 7, UlDlAssignment::Dl),
            gssi,
            Some(latest_reporter.clone()),
        );

        // Local UMAC robustness guard: repeated same-call group floor withdraw
        // PDUs should not occupy multiple protected slots while the assigned
        // channel drains. This preserves the newest reporter and marks the
        // older duplicate as locally discarded.
        assert_eq!(sched.dltx_queues[traffic_ts as usize - 1].len(), 1);
        assert_eq!(old_reporter.get_state(), TxState::Discarded);
        assert_eq!(latest_reporter.get_state(), TxState::Pending);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let (_tch, stch) = sched.dl_build_traffic_block(
            TdmaTime {
                t: traffic_ts,
                f: 2,
                m: 1,
                h: 0,
            },
            &subscribers,
            &mut energy_saving,
        );
        let stch = stch.expect("coalesced floor withdraw should remain queued");
        let mut parsed = BitBuffer::from_bitbuffer(&stch);
        let resource = MacResource::from_bitbuf(&mut parsed).expect("selected STCH should carry MAC-RESOURCE");
        assert_eq!(resource.addr.map(|addr| addr.ssi), Some(gssi.ssi));
        let ceased = DTxCeased::from_bitbuf(&mut parsed).expect("selected STCH should carry D-TX CEASED");
        assert_eq!(ceased.call_identifier, 7);
        assert_eq!(latest_reporter.get_state(), TxState::Transmitted);
    }

    #[test]
    fn test_protected_floor_withdraw_backlog_stays_bounded_and_retains_newest() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let traffic_ts = 2;
        let first_reporter = TxReporter::new_unacked();
        for offset in 0..MAX_DLSCHED_ELEMS_PER_TIMESLOT {
            let addr = TetraAddress::issi(2_260_000 + offset as u32);
            let reporter = (offset == 0).then(|| first_reporter.clone());
            sched.dl_enqueue_stealing(
                traffic_ts,
                test_d_tx_ceased_stch_block(addr, offset as u16, UlDlAssignment::Dl),
                addr,
                reporter,
            );
        }

        let latest_addr = TetraAddress::new(226_333, SsiType::Gssi);
        let latest_reporter = TxReporter::new_unacked();
        sched.dl_enqueue_stealing(
            traffic_ts,
            test_d_tx_ceased_stch_block(latest_addr, 0x1234, UlDlAssignment::Dl),
            latest_addr,
            Some(latest_reporter.clone()),
        );

        // EN 300 392-2 clauses 14.5.2.2.1 and 23.5 make floor-withdraw STCH
        // time critical, but the local scheduler still has to stay bounded in
        // a pathological storm. When every queued item is protected, the oldest
        // lowest-priority protected element is discarded and the newest floor
        // withdraw remains queued.
        assert_eq!(sched.dltx_queues[traffic_ts as usize - 1].len(), MAX_DLSCHED_ELEMS_PER_TIMESLOT);
        assert_eq!(first_reporter.get_state(), TxState::Discarded);
        assert_eq!(latest_reporter.get_state(), TxState::Pending);
        assert!(
            sched.dltx_queues[traffic_ts as usize - 1]
                .iter()
                .any(|elem| matches!(elem, DlSchedElem::Stealing(_, addr, reporter, _)
                    if *addr == latest_addr && reporter.as_ref().is_some_and(|r| r.shares_state_with(&latest_reporter)))),
            "newest protected floor withdraw should remain queued after cap enforcement"
        );
    }

    #[test]
    fn test_invalid_downlink_timeslot_enqueue_and_drop_apis_do_not_panic_or_mutate() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress::issi(1234);
        let grant = BasicSlotgrant {
            capacity_allocation: BasicSlotgrantCapAlloc::FirstSubslotGranted,
            granting_delay: BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity,
        };

        for invalid_ts in [0, 5] {
            let steal_reporter = TxReporter::new_unacked();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sched.dl_enqueue_grant(invalid_ts, addr, grant.clone(), None);
                sched.dl_enqueue_reservation_grant(invalid_ts, addr, ReservationRequirement::Req1Slot);
                sched.dl_enqueue_random_access_ack(invalid_ts, addr);
                assert!(!sched.take_pending_ra_ack(invalid_ts, addr));
                assert!(!sched.take_pending_ra_ack_for_stch(invalid_ts, addr, true));
                sched.dl_enqueue_stealing(invalid_ts, BitBuffer::new(SCH_HD_CAP), addr, Some(steal_reporter.clone()));
                assert!(!sched.dl_drop_all_except_stolen(invalid_ts));
                assert!(
                    sched
                        .dl_take_all_ready_grants_and_acks(
                            TdmaTime {
                                t: invalid_ts,
                                f: 1,
                                m: 1,
                                h: 0
                            },
                            &SubscriberRegistry::new(),
                            &HashMap::new(),
                        )
                        .is_empty()
                );
                assert!(
                    sched
                        .dl_get_scheduled_resource_for_addr(
                            TdmaTime {
                                t: invalid_ts,
                                f: 1,
                                m: 1,
                                h: 0
                            },
                            &addr
                        )
                        .is_none()
                );
                sched.dl_integrate_sched_elems_for_timeslot(
                    TdmaTime {
                        t: invalid_ts,
                        f: 1,
                        m: 1,
                        h: 0,
                    },
                    &SubscriberRegistry::new(),
                    &HashMap::new(),
                );
            }));

            // Internal robustness guard: invalid local scheduling state must
            // not crash the BS process or leave an impossible TMA request
            // pending. This does not alter any valid over-air ETSI PDU.
            assert!(result.is_ok(), "invalid downlink timeslot {invalid_ts} must not panic");
            assert_eq!(steal_reporter.get_state(), TxState::Discarded);
            assert!(
                sched.dltx_queues.iter().all(Vec::is_empty),
                "invalid downlink timeslot {invalid_ts} must not mutate DL queues"
            );
            assert!(
                sched.pending_ra_acks.iter().all(Vec::is_empty),
                "invalid downlink timeslot {invalid_ts} must not leave pending RA ACK state"
            );
        }
    }

    #[test]
    fn test_ul_private_scope_uses_primary_addr_not_secondary_group_speaker() {
        let mut sched = get_testing_slotter();
        let ts = 2;
        let gssi = 0x226333;
        let first_speaker = 0x2260616;
        let second_speaker = 0x2260618;

        sched.create_circuit(
            Direction::Ul,
            Circuit {
                direction: Direction::Ul,
                ts,
                peer_ts: None,
                usage: 4,
                circuit_mode: tetra_saps::control::enums::circuit_mode_type::CircuitModeType::TchS,
                speech_service: Some(0),
                etee_encrypted: false,
                dl_media_source: CircuitDlMediaSource::LocalLoopback,
                active_addr: Some(TetraAddress::new(gssi, SsiType::Gssi)),
                active_secondary_addrs: vec![TetraAddress::issi(first_speaker)],
            },
        );

        assert!(
            sched.ul_circuit_has_issi_participants(ts),
            "the first group speaker is still tracked as an active ISSI for EG/listening state"
        );
        assert!(
            !sched.ul_circuit_is_private_participant_scoped(ts),
            "EN 300 392-2 clause 14.5.2: a GSSI-primary group bearer must not use private/P2P participant filtering"
        );
        assert!(sched.circuit_is_active_for_addr(Direction::Ul, ts, TetraAddress::new(gssi, SsiType::Gssi)));
        assert!(sched.circuit_is_active_for_addr(Direction::Ul, ts, TetraAddress::issi(first_speaker)));
        assert!(
            !sched.circuit_is_active_for_addr(Direction::Ul, ts, TetraAddress::issi(second_speaker)),
            "later group speakers are admitted by group floor-control, not by preloading every ISSI as a private participant"
        );
    }

    #[test]
    fn test_ul_private_scope_remains_strict_for_p2p_primary_issi() {
        let mut sched = get_testing_slotter();
        let ts = 2;
        let caller_issi = 0x2260616;
        let called_issi = 0x2260618;
        let outsider_issi = 0x2260082;

        sched.create_circuit(
            Direction::Ul,
            Circuit {
                direction: Direction::Ul,
                ts,
                peer_ts: None,
                usage: 4,
                circuit_mode: tetra_saps::control::enums::circuit_mode_type::CircuitModeType::TchS,
                speech_service: Some(0),
                etee_encrypted: false,
                dl_media_source: CircuitDlMediaSource::LocalLoopback,
                active_addr: Some(TetraAddress::issi(caller_issi)),
                active_secondary_addrs: vec![TetraAddress::issi(called_issi)],
            },
        );

        assert!(
            sched.ul_circuit_is_private_participant_scoped(ts),
            "EN 300 392-2 clause 14.5.1: private/P2P calls use ISSI-primary bearers and keep strict participant filtering"
        );
        assert!(sched.circuit_is_active_for_addr(Direction::Ul, ts, TetraAddress::issi(caller_issi)));
        assert!(sched.circuit_is_active_for_addr(Direction::Ul, ts, TetraAddress::issi(called_issi)));
        assert!(
            !sched.circuit_is_active_for_addr(Direction::Ul, ts, TetraAddress::issi(outsider_issi)),
            "a third ISSI must not become a private-call participant through floor-control"
        );
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

    fn assign_test_packet_data_uplink_slots(sched: &mut BsChannelScheduler, addr: TetraAddress, slots: &[u8]) {
        for ts in slots {
            sched.packet_data_assignments[*ts as usize - 1] = Some(PacketDataAssignment {
                addr,
                ul_dl_assigned: UlDlAssignment::Both,
                carrier_num: 1001,
                expires_at: TdmaTime { t: *ts, f: 1, m: 2, h: 0 }.add_timeslots(PACKET_DATA_ASSIGNMENT_TTL_TIMESLOTS),
            });
        }
    }

    fn block_test_uplink_slot(sched: &mut BsChannelScheduler, ts: TdmaTime, owner: u32) {
        let index = sched.ul_ts_to_sched_index(&ts);
        let elem = &mut sched.ulsched[ts.t as usize - 1][index];
        elem.ul1 = Some(owner);
        elem.ul2 = Some(owner);
    }

    fn open_test_group_circuit(sched: &mut BsChannelScheduler, ts: u8, gssi: u32, speaker_issi: u32, usage: u8) {
        for direction in [Direction::Dl, Direction::Ul] {
            sched.create_circuit(
                direction,
                Circuit {
                    direction,
                    ts,
                    peer_ts: None,
                    usage,
                    circuit_mode: tetra_saps::control::enums::circuit_mode_type::CircuitModeType::TchS,
                    speech_service: Some(0),
                    etee_encrypted: false,
                    dl_media_source: CircuitDlMediaSource::LocalLoopback,
                    active_addr: Some(TetraAddress::new(gssi, SsiType::Gssi)),
                    active_secondary_addrs: vec![TetraAddress::issi(speaker_issi)],
                },
            );
        }
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

    fn assert_schf_null_block(block: &TmvUnitdataReq) {
        assert_eq!(block.logical_channel, LogicalChannel::SchF);

        let mut mac_block = block.mac_block.clone();
        mac_block.seek(0);
        let resource = MacResource::from_bitbuf(&mut mac_block).expect("SCH/F idle block must carry a MAC-RESOURCE");
        assert!(
            resource.is_null_pdu(),
            "idle assigned PDCH maintenance must use a Null MAC-RESOURCE"
        );
    }

    fn first_schf_channel_allocation(slot: &TmvUnitdataReqSlot) -> ChanAllocElement {
        let block = slot.blk1.as_ref().expect("scheduled SCH/F block should be present");
        assert_eq!(block.logical_channel, LogicalChannel::SchF);

        let mut mac_block = block.mac_block.clone();
        mac_block.seek(0);
        let resource = MacResource::from_bitbuf(&mut mac_block).expect("SCH/F block should carry a MAC-RESOURCE");
        resource
            .chan_alloc_element
            .expect("SCH/F MAC-RESOURCE should carry channel allocation")
    }

    #[test]
    fn test_sndcp_packet_data_chan_alloc_activates_assigned_pdch_maintenance() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });
        let issi = 0x5101;
        let (pdu, sdu) = test_packet_data_channel_allocation_resource(
            issi,
            [true, true, false, false],
            None,
            ChanAllocType::Replace,
            UlDlAssignment::Both,
        );
        sched.dl_enqueue_tma(pdu, sdu, None);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let allocated_on_mcch = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(allocated_on_mcch.ts, TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        assert!(
            sched.packet_data_assignments[0].is_none(),
            "packet-data assignment must not alter TS1"
        );
        assert!(
            sched.packet_data_assignments[1].is_some(),
            "SNDCP MAC-RESOURCE channel allocation should activate TS2 packet-data assignment"
        );

        let mut ts1_aach = allocated_on_mcch.bbk.expect("TS1 should carry AACH").mac_block;
        ts1_aach.seek(0);
        let ts1_aach = AccessAssign::from_bitbuf(&mut ts1_aach).expect("TS1 AACH should parse");
        assert_eq!(ts1_aach.dl_usage, AccessAssignDlUsage::CommonControl);

        sched.tick_start(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let maintained = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(maintained.ts, TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        assert_schf_null_block(
            maintained
                .blk1
                .as_ref()
                .expect("idle assigned PDCH should transmit SCH/F Null for channel maintenance"),
        );

        let mut aach = maintained.bbk.expect("assigned PDCH should carry AACH").mac_block;
        aach.seek(0);
        let aach = AccessAssign::from_bitbuf(&mut aach).expect("assigned PDCH AACH should parse");
        assert_eq!(aach.dl_usage, AccessAssignDlUsage::AssignedControl);
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::AssignedOnly);
    }

    #[test]
    fn test_packet_data_dynamic_single_slot_allocation_moves_away_from_voice() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let issi = 0x5110;
        let (pdu, sdu) = test_packet_data_channel_allocation_resource(
            issi,
            [false, true, false, false],
            None,
            ChanAllocType::Replace,
            UlDlAssignment::Both,
        );
        sched.dl_enqueue_tma(pdu, sdu, None);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let allocated_on_mcch = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);

        let chan_alloc = first_schf_channel_allocation(&allocated_on_mcch);
        assert_eq!(chan_alloc.ts_assigned, [false, false, true, false]);
        assert!(
            sched.packet_data_assignments[1].is_none(),
            "voice-owned TS2 must not become a packet-data assigned channel"
        );
        assert!(
            sched.packet_data_assignments[2].is_some(),
            "single-slot packet data should move to the first free traffic slot"
        );
    }

    #[test]
    fn test_packet_data_dynamic_single_slot_allocation_uses_ts4_when_ts2_ts3_are_voice() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);
        open_test_dl_circuit(&mut sched, 3);

        let issi = 0x511a;
        let (pdu, sdu) = test_packet_data_channel_allocation_resource(
            issi,
            [false, true, false, false],
            None,
            ChanAllocType::Replace,
            UlDlAssignment::Both,
        );
        sched.dl_enqueue_tma(pdu, sdu, None);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let allocated_on_mcch = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);

        let chan_alloc = first_schf_channel_allocation(&allocated_on_mcch);
        assert_eq!(chan_alloc.ts_assigned, [false, false, false, true]);
        assert_eq!(
            chan_alloc.ts_assigned.iter().filter(|assigned| **assigned).count(),
            1,
            "single-slot packet-data fallback must not expand to parallel TS"
        );
        assert!(
            sched.packet_data_assignments[1].is_none() && sched.packet_data_assignments[2].is_none(),
            "voice-owned TS2/TS3 must not become packet-data assigned channels"
        );
        assert!(
            sched.packet_data_assignments[3].is_some(),
            "single-slot packet data should use TS4 when it is the first free traffic slot"
        );
    }

    #[test]
    fn test_packet_data_single_slot_policy_defers_second_parallel_pdch() {
        let mut sched = get_testing_slotter();
        let now = TdmaTime { t: 4, f: 1, m: 1, h: 0 };
        sched.set_dl_time(now);

        let first_addr = TetraAddress::issi(0x511c);
        let second_issi = 0x511d;
        sched.apply_packet_data_assignment_update(
            PacketDataAssignmentUpdate::Replace {
                addr: first_addr,
                ts_assigned: [false, true, false, false],
                ul_dl_assigned: UlDlAssignment::Both,
                carrier_num: 1001,
            },
            now,
        );
        assert!(sched.packet_data_assignments[1].is_some());

        let (pdu, sdu) = test_packet_data_channel_allocation_resource(
            second_issi,
            [false, true, false, false],
            None,
            ChanAllocType::Replace,
            UlDlAssignment::Both,
        );
        sched.dl_enqueue_tma(pdu, sdu, None);

        assert!(
            sched.dltx_queues.iter().all(Vec::is_empty),
            "a second packet-data ISSI must not receive a parallel PDCH on TS3/TS4 while one data slot is active"
        );
        assert!(
            sched.packet_data_assignments[1].is_some_and(|assignment| assignment.addr == first_addr)
                && sched.packet_data_assignments[2].is_none()
                && sched.packet_data_assignments[3].is_none(),
            "single-slot packet-data state must remain globally capped to one traffic TS"
        );

        sched.apply_packet_data_assignment_update(PacketDataAssignmentUpdate::QuitAndGo { addr: first_addr }, now.add_timeslots(4));
        let (pdu, sdu) = test_packet_data_channel_allocation_resource(
            second_issi,
            [false, true, false, false],
            None,
            ChanAllocType::Replace,
            UlDlAssignment::Both,
        );
        sched.dl_enqueue_tma(pdu, sdu, None);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let allocated_on_mcch = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        let chan_alloc = first_schf_channel_allocation(&allocated_on_mcch);
        assert_eq!(chan_alloc.ts_assigned, [false, true, false, false]);
        assert_eq!(
            chan_alloc.ts_assigned.iter().filter(|assigned| **assigned).count(),
            1,
            "after the first PDCH is released, the next data session must still advertise exactly one slot"
        );
    }

    #[test]
    fn test_packet_data_dynamic_single_slot_allocation_defers_when_all_traffic_slots_are_voice() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);
        open_test_dl_circuit(&mut sched, 3);
        open_test_dl_circuit(&mut sched, 4);

        let issi = 0x511b;
        let (pdu, sdu) = test_packet_data_channel_allocation_resource(
            issi,
            [false, true, false, false],
            None,
            ChanAllocType::Replace,
            UlDlAssignment::Both,
        );
        sched.dl_enqueue_tma(pdu, sdu, None);

        assert!(
            sched.dltx_queues.iter().all(|queue| queue.is_empty()),
            "when voice owns TS2/TS3/TS4, packet data must not queue a channel allocation on any slot"
        );
        assert!(
            sched.packet_data_assignments[1..].iter().all(Option::is_none),
            "when no traffic slot is free, packet data must not create a stale assigned-PDCH owner"
        );
    }

    #[test]
    fn test_packet_data_assignment_marks_central_allocator_and_voice_avoids_it() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });
        let issi = 0x5114;
        let (pdu, sdu) = test_packet_data_channel_allocation_resource(
            issi,
            [false, true, false, false],
            None,
            ChanAllocType::Replace,
            UlDlAssignment::Both,
        );
        sched.dl_enqueue_tma(pdu, sdu, None);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let mut timeslot_alloc = TimeslotAllocator::default();
        let allocated_on_mcch = sched.finalize_ts_for_tick_with_timeslot_allocator(&subscribers, &mut energy_saving, &mut timeslot_alloc);

        let chan_alloc = first_schf_channel_allocation(&allocated_on_mcch);
        assert_eq!(chan_alloc.ts_assigned, [false, true, false, false]);
        assert_eq!(timeslot_alloc.owner(2), Some(TimeslotOwner::PacketData));

        let voice_ts = timeslot_alloc
            .allocate_any_preempting(TimeslotOwner::Cmce, TimeslotOwner::PacketData)
            .expect("voice should use a real free slot before reclaiming PDCH");
        assert_eq!(voice_ts, 3);
        assert_eq!(
            timeslot_alloc.owner(2),
            Some(TimeslotOwner::PacketData),
            "data must keep TS2 while TS3/TS4 are free for voice"
        );
        assert_eq!(timeslot_alloc.owner(3), Some(TimeslotOwner::Cmce));
    }

    #[test]
    fn test_sndcp_mac_resource_pdch_stall_cannot_block_voice_preemption_or_release() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });
        let addr = TetraAddress::issi(0x5118);
        let (pdu, sdu) = test_packet_data_channel_allocation_resource(
            addr.ssi,
            [false, true, true, true],
            None,
            ChanAllocType::Replace,
            UlDlAssignment::Both,
        );
        sched.dl_enqueue_tma(pdu, sdu, None);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let mut timeslot_alloc = TimeslotAllocator::default();
        let allocated_on_mcch = sched.finalize_ts_for_tick_with_timeslot_allocator(&subscribers, &mut energy_saving, &mut timeslot_alloc);
        let chan_alloc = first_schf_channel_allocation(&allocated_on_mcch);
        assert_eq!(chan_alloc.ts_assigned, [false, true, false, false]);
        assert_eq!(timeslot_alloc.owner(2), Some(TimeslotOwner::PacketData));
        assert_eq!(timeslot_alloc.owner(3), None);
        assert_eq!(timeslot_alloc.owner(4), None);
        assert!(
            sched
                .packet_data_assignment_for_timeslot(TdmaTime { t: 2, f: 2, m: 1, h: 0 })
                .is_some(),
            "SNDCP MAC-RESOURCE should create exactly one PDCH assignment on TS2"
        );
        assert_eq!(
            sched.packet_data_uplink_owner(TdmaTime { t: 2, f: 2, m: 1, h: 0 }, PhyBlockNum::Both),
            Some(addr),
            "bidirectional PDCH assignment should own uplink packet data on TS2"
        );

        let voice_slots = timeslot_alloc
            .allocate_many_preempting(TimeslotOwner::Cmce, 2, TimeslotOwner::PacketData)
            .expect("local duplex voice must use free slots before reclaiming stalled PDCH");
        assert_eq!(voice_slots, vec![3, 4]);
        for ts in &voice_slots {
            open_test_dl_circuit(&mut sched, *ts);
        }

        for ts in [3, 4] {
            assert_eq!(timeslot_alloc.owner(ts), Some(TimeslotOwner::Cmce));
            assert!(
                sched
                    .packet_data_assignment_for_timeslot(TdmaTime { t: ts, f: 2, m: 1, h: 0 })
                    .is_none(),
                "voice open must clear the stalled SNDCP PDCH assignment on reclaimed TS{ts}"
            );
            assert_eq!(
                sched.packet_data_uplink_owner(TdmaTime { t: ts, f: 2, m: 1, h: 0 }, PhyBlockNum::Both),
                None,
                "voice open must clear stale uplink packet-data ownership on reclaimed TS{ts}"
            );
        }
        assert_eq!(timeslot_alloc.owner(2), Some(TimeslotOwner::PacketData));
        assert_eq!(
            sched.packet_data_uplink_owner(TdmaTime { t: 2, f: 2, m: 1, h: 0 }, PhyBlockNum::Both),
            Some(addr),
            "voice must preserve the existing single-slot PDCH while free voice slots exist"
        );

        sched.tick_start(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let _pdch_slot = sched.finalize_ts_for_tick_with_timeslot_allocator(&subscribers, &mut energy_saving, &mut timeslot_alloc);

        sched.tick_start(TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        let voice_slot = sched.finalize_ts_for_tick_with_timeslot_allocator(&subscribers, &mut energy_saving, &mut timeslot_alloc);
        assert_eq!(voice_slot.ts, TdmaTime { t: 3, f: 2, m: 1, h: 0 });
        let mut voice_aach = voice_slot.bbk.expect("voice slot should carry AACH").mac_block;
        voice_aach.seek(0);
        let voice_aach = AccessAssign::from_bitbuf(&mut voice_aach).expect("voice AACH should parse");
        assert_eq!(voice_aach.dl_usage, AccessAssignDlUsage::Traffic(4));
        assert_ne!(voice_aach.dl_usage, AccessAssignDlUsage::AssignedControl);
        assert_eq!(
            voice_slot.blk1.as_ref().map(|block| block.logical_channel),
            Some(LogicalChannel::Stch),
            "free voice slot must schedule traffic/STCH instead of assigned PDCH maintenance"
        );

        for ts in &voice_slots {
            sched.close_circuit(Direction::Dl, *ts);
            timeslot_alloc
                .release(TimeslotOwner::Cmce, *ts)
                .expect("voice release should free only CMCE-owned scheduler slots");
        }
        assert_eq!(timeslot_alloc.owner(2), Some(TimeslotOwner::PacketData));
        assert_eq!(timeslot_alloc.owner(3), None);
        assert_eq!(timeslot_alloc.owner(4), None);
        assert!(
            sched
                .packet_data_assignment_for_timeslot(TdmaTime { t: 2, f: 2, m: 1, h: 0 })
                .is_some()
                && sched
                    .packet_data_assignment_for_timeslot(TdmaTime { t: 3, f: 2, m: 1, h: 0 })
                    .is_none()
                && sched
                    .packet_data_assignment_for_timeslot(TdmaTime { t: 4, f: 2, m: 1, h: 0 })
                    .is_none(),
            "voice release must preserve only the single original PDCH slot"
        );
    }

    #[test]
    fn test_packet_data_final_allocation_respects_cmce_reserved_slot_before_voice_open() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });
        let issi = 0x5115;
        let (pdu, sdu) = test_packet_data_channel_allocation_resource(
            issi,
            [false, true, false, false],
            None,
            ChanAllocType::Replace,
            UlDlAssignment::Both,
        );
        sched.dl_enqueue_tma(pdu, sdu, None);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let mut timeslot_alloc = TimeslotAllocator::default();
        timeslot_alloc
            .reserve(TimeslotOwner::Cmce, 2)
            .expect("CMCE may reserve TS2 before UMAC opens the circuit");
        let allocated_on_mcch = sched.finalize_ts_for_tick_with_timeslot_allocator(&subscribers, &mut energy_saving, &mut timeslot_alloc);

        let chan_alloc = first_schf_channel_allocation(&allocated_on_mcch);
        assert_eq!(
            chan_alloc.ts_assigned,
            [false, false, true, false],
            "PDCH must move away from a centrally reserved voice slot before RF transmission"
        );
        assert_eq!(timeslot_alloc.owner(2), Some(TimeslotOwner::Cmce));
        assert_eq!(timeslot_alloc.owner(3), Some(TimeslotOwner::PacketData));
        assert!(
            sched.packet_data_assignments[1].is_none(),
            "TS2 must not remain data-assigned after CMCE reserved it"
        );
        assert!(sched.packet_data_assignments[2].is_some());
    }

    #[test]
    fn test_idle_pdch_maintenance_respects_cmce_reserved_slot_before_voice_open() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress::issi(0x5116);
        let now = TdmaTime { t: 1, f: 2, m: 1, h: 0 };
        let mut timeslot_alloc = TimeslotAllocator::default();
        sched.set_dl_time(now);

        {
            let mut allocator_ref = Some(&mut timeslot_alloc);
            sched.apply_packet_data_assignment_update_with_allocator(
                PacketDataAssignmentUpdate::Replace {
                    addr,
                    ts_assigned: [false, true, true, true],
                    ul_dl_assigned: UlDlAssignment::Both,
                    carrier_num: 1001,
                },
                now,
                &mut allocator_ref,
            );
        }
        assert_eq!(timeslot_alloc.owner(2), Some(TimeslotOwner::PacketData));
        assert_eq!(timeslot_alloc.owner(3), None);
        assert_eq!(timeslot_alloc.owner(4), None);
        assert!(
            sched
                .packet_data_assignment_for_timeslot(TdmaTime { t: 2, f: 2, m: 1, h: 0 })
                .is_some(),
            "test setup should have an idle PDCH assignment on TS2"
        );
        timeslot_alloc
            .reserve(TimeslotOwner::Brew, 3)
            .expect("test setup occupies TS3 so voice must reclaim data");
        timeslot_alloc
            .reserve(TimeslotOwner::Brew, 4)
            .expect("test setup occupies TS4 so voice must reclaim data");

        let voice_ts = timeslot_alloc
            .allocate_any_preempting(TimeslotOwner::Cmce, TimeslotOwner::PacketData)
            .expect("voice must be able to reserve a packet-data slot when no free traffic slot remains");
        assert_eq!(voice_ts, 2);
        assert_eq!(timeslot_alloc.owner(2), Some(TimeslotOwner::Cmce));
        assert!(
            sched
                .packet_data_assignment_for_timeslot_with_allocator(TdmaTime { t: 2, f: 2, m: 1, h: 0 }, Some(&timeslot_alloc),)
                .is_none(),
            "central CMCE reservation must suppress stale PDCH ownership before circuit-open"
        );

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let reserved_voice_slot = sched.finalize_ts_for_tick_with_timeslot_allocator(&subscribers, &mut energy_saving, &mut timeslot_alloc);
        assert_eq!(reserved_voice_slot.ts, TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        assert_ne!(
            reserved_voice_slot.blk1.as_ref().map(|block| block.logical_channel),
            Some(LogicalChannel::SchF),
            "CMCE-reserved TS2 must not transmit idle assigned-PDCH SCH/F maintenance"
        );
        let mut aach = reserved_voice_slot.bbk.expect("reserved voice slot should carry AACH").mac_block;
        aach.seek(0);
        let aach = AccessAssign::from_bitbuf(&mut aach).expect("reserved voice slot AACH should parse");
        assert_eq!(
            aach.dl_usage,
            AccessAssignDlUsage::Unallocated,
            "until the voice circuit opens, TS2 must stop advertising assigned-PDCH downlink usage"
        );
        assert_eq!(
            aach.ul_usage,
            AccessAssignUlUsage::Unallocated,
            "until the voice circuit opens, TS2 must stop advertising assigned-PDCH uplink usage"
        );
    }

    #[test]
    fn test_packet_data_multi_slot_request_normalizes_to_single_slot_around_voice() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 3);

        let issi = 0x5111;
        let (pdu, sdu) = test_packet_data_channel_allocation_resource(
            issi,
            [false, true, true, true],
            None,
            ChanAllocType::Replace,
            UlDlAssignment::Both,
        );
        sched.dl_enqueue_tma(pdu, sdu, None);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let allocated_on_mcch = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);

        let chan_alloc = first_schf_channel_allocation(&allocated_on_mcch);
        assert_eq!(chan_alloc.ts_assigned, [false, true, false, false]);
        assert_eq!(
            chan_alloc.ts_assigned.iter().filter(|assigned| **assigned).count(),
            1,
            "packet-data RF allocation must never advertise parallel PDCH slots"
        );
        assert!(sched.packet_data_assignments[1].is_some());
        assert!(
            sched.packet_data_assignments[2].is_none(),
            "voice-owned TS3 must be withheld from packet data"
        );
        assert!(
            sched.packet_data_assignments[3].is_none(),
            "multi-slot packet-data request must be capped to one first-free traffic slot"
        );
    }

    #[test]
    fn test_packet_data_downlink_uses_single_assigned_pdch_slot() {
        let mut sched = get_testing_slotter();
        let now = TdmaTime { t: 2, f: 1, m: 1, h: 0 };
        sched.set_dl_time(now);
        let addr = TetraAddress::issi(0x5119);
        sched.apply_packet_data_assignment_update(
            PacketDataAssignmentUpdate::Replace {
                addr,
                ts_assigned: [false, true, true, true],
                ul_dl_assigned: UlDlAssignment::Both,
                carrier_num: 1001,
            },
            now,
        );

        let pdu = BsChannelScheduler::dl_make_minimal_resource(&addr, None, false);
        let sdu = test_sndcp_llc_sdu();
        let selected: Vec<u8> = (0..4)
            .map(|_| {
                sched
                    .packet_data_downlink_timeslots_for_resource(&pdu, &sdu)
                    .expect("active single-slot PDCH should route packet data")[0]
            })
            .collect();

        assert_eq!(
            selected,
            vec![2, 2, 2, 2],
            "packet-data downlink must stay on the single assigned PDCH slot"
        );
    }

    #[test]
    fn test_packet_data_downlink_keeps_first_al_segment_on_first_active_pdch_slot() {
        let mut sched = get_testing_slotter();
        let now = TdmaTime { t: 2, f: 1, m: 1, h: 0 };
        sched.set_dl_time(now);
        let addr = TetraAddress::issi(0x511a);
        sched.apply_packet_data_assignment_update(
            PacketDataAssignmentUpdate::Replace {
                addr,
                ts_assigned: [false, true, true, true],
                ul_dl_assigned: UlDlAssignment::Both,
                carrier_num: 1001,
            },
            now,
        );

        let pdu = BsChannelScheduler::dl_make_minimal_resource(&addr, None, false);
        let ss0 = test_sndcp_al_data_sdu(0);
        let ss1 = test_sndcp_al_data_sdu(1);

        assert_eq!(
            sched
                .packet_data_downlink_timeslots_for_resource(&pdu, &ss0)
                .expect("first AL segment should route on active PDCH")[0],
            2,
            "AL S(S)=0 is the reassembly anchor and should use the first active PDCH slot"
        );
        assert_eq!(
            sched
                .packet_data_downlink_timeslots_for_resource(&pdu, &ss1)
                .expect("following AL segment should still use assigned PDCH")[0],
            2,
            "after S(S)=0, subsequent AL data must stay on the single assigned PDCH slot"
        );
    }

    #[test]
    fn test_packet_data_dynamic_allocation_fails_closed_when_voice_owns_all_traffic_slots() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);
        open_test_dl_circuit(&mut sched, 3);
        open_test_dl_circuit(&mut sched, 4);

        let issi = 0x5112;
        let (pdu, sdu) = test_packet_data_channel_allocation_resource(
            issi,
            [false, true, true, true],
            None,
            ChanAllocType::Replace,
            UlDlAssignment::Both,
        );
        let reporter = TxReporter::new_unacked();
        sched.dl_enqueue_tma(pdu, sdu, Some(reporter.clone()));

        assert_eq!(
            reporter.get_state(),
            TxState::Discarded,
            "packet-data request should be deferred instead of sent on MCCH when no PDCH slot exists"
        );
        assert!(
            sched.packet_data_assignments[1..].iter().all(Option::is_none),
            "packet data must not steal any slot when voice owns all traffic capacity"
        );
        assert!(
            sched.dltx_queues.iter().all(Vec::is_empty),
            "no SNDCP MAC-RESOURCE should be queued without a free assigned PDCH slot"
        );
    }

    #[test]
    fn test_packet_data_assignment_dl_only_uses_common_and_assigned_uplink() {
        let mut sched = get_testing_slotter();
        let now = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
        sched.set_dl_time(now);
        sched.apply_packet_data_assignment_update(
            PacketDataAssignmentUpdate::Replace {
                addr: TetraAddress::issi(0x5102),
                ts_assigned: [false, false, true, false],
                ul_dl_assigned: UlDlAssignment::Dl,
                carrier_num: 1001,
            },
            now,
        );

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let maintained = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(maintained.ts, TdmaTime { t: 2, f: 1, m: 1, h: 0 });

        let mut aach = maintained.bbk.expect("assigned PDCH should carry AACH").mac_block;
        aach.seek(0);
        let aach = AccessAssign::from_bitbuf(&mut aach).expect("assigned PDCH AACH should parse");
        assert_eq!(aach.dl_usage, AccessAssignDlUsage::AssignedControl);
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::CommonAndAssigned);
        assert_schf_null_block(
            maintained
                .blk1
                .as_ref()
                .expect("idle DL-only assigned PDCH should still transmit SCH/F Null"),
        );
    }

    #[test]
    fn test_packet_data_assignment_shrinks_deactivates_and_expires_without_circuit_state() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress::issi(0x5103);
        let now = TdmaTime { t: 1, f: 2, m: 1, h: 0 };

        sched.apply_packet_data_assignment_update(
            PacketDataAssignmentUpdate::Replace {
                addr,
                ts_assigned: [false, true, true, false],
                ul_dl_assigned: UlDlAssignment::Both,
                carrier_num: 1001,
            },
            now,
        );
        assert!(sched.packet_data_assignments[1].is_some());
        assert!(
            sched.packet_data_assignments[2].is_none(),
            "multi-slot request must be normalized to one packet-data assignment"
        );

        sched.apply_packet_data_assignment_update(
            PacketDataAssignmentUpdate::Replace {
                addr,
                ts_assigned: [false, false, true, false],
                ul_dl_assigned: UlDlAssignment::Both,
                carrier_num: 1001,
            },
            now.add_timeslots(4),
        );
        assert!(
            sched.packet_data_assignments[1].is_some(),
            "Replace should refresh the single first-free packet-data timeslot for the same ISSI"
        );
        assert!(
            sched.packet_data_assignments[2].is_none(),
            "Replace must not move or expand packet data while TS2 remains the first free traffic slot"
        );

        sched.apply_packet_data_assignment_update(PacketDataAssignmentUpdate::QuitAndGo { addr }, now.add_timeslots(8));
        assert!(sched.packet_data_assignments[1..].iter().all(Option::is_none));

        sched.apply_packet_data_assignment_update(
            PacketDataAssignmentUpdate::Replace {
                addr,
                ts_assigned: [false, false, false, true],
                ul_dl_assigned: UlDlAssignment::Both,
                carrier_num: 1001,
            },
            now,
        );
        assert!(
            sched.packet_data_assignments[1].is_some(),
            "fresh packet-data assignment should use TS2 as the first free traffic slot even if the request bitmap names TS4"
        );
        assert!(sched.packet_data_assignments[2].is_none());
        assert!(sched.packet_data_assignments[3].is_none());
        sched.expire_packet_data_assignments(now.add_timeslots(PACKET_DATA_ASSIGNMENT_TTL_TIMESLOTS));
        assert!(
            sched.packet_data_assignments[1].is_none(),
            "stale packet-data assignment should expire locally"
        );
    }

    #[test]
    fn test_packet_data_assignment_survives_short_ready_gap_without_voice_or_quit_and_go() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress::issi(0x5108);
        let now = TdmaTime { t: 1, f: 2, m: 1, h: 0 };

        sched.apply_packet_data_assignment_update(
            PacketDataAssignmentUpdate::Replace {
                addr,
                ts_assigned: [false, true, true, true],
                ul_dl_assigned: UlDlAssignment::Both,
                carrier_num: 1001,
            },
            now,
        );
        sched.expire_packet_data_assignments(now.add_timeslots(4 * 18 * 4));

        assert!(
            sched.packet_data_assignments[1].is_some()
                && sched.packet_data_assignments[2].is_none()
                && sched.packet_data_assignments[3].is_none(),
            "single-slot PDCH must survive a normal READY/STANDBY gap without expanding to TS3/TS4"
        );
    }

    #[test]
    fn test_stuck_sndcp_packet_data_cannot_block_voice_allocation_and_release() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress::issi(0x5109);
        let now = TdmaTime { t: 2, f: 2, m: 1, h: 0 };
        let mut timeslot_alloc = TimeslotAllocator::default();
        sched.set_dl_time(now);

        {
            let mut allocator_ref = Some(&mut timeslot_alloc);
            sched.apply_packet_data_assignment_update_with_allocator(
                PacketDataAssignmentUpdate::Replace {
                    addr,
                    ts_assigned: [false, true, true, true],
                    ul_dl_assigned: UlDlAssignment::Both,
                    carrier_num: 1001,
                },
                now,
                &mut allocator_ref,
            );
        }
        assert_eq!(timeslot_alloc.owner(2), Some(TimeslotOwner::PacketData));
        assert_eq!(timeslot_alloc.owner(3), None);
        assert_eq!(timeslot_alloc.owner(4), None);
        assert!(
            sched.packet_data_assignments[1].is_some()
                && sched.packet_data_assignments[2].is_none()
                && sched.packet_data_assignments[3].is_none(),
            "stuck SNDCP must hold exactly one PDCH slot until voice pressure, QuitAndGo, or expiry"
        );

        let duplex_voice_slots = timeslot_alloc
            .allocate_many_preempting(TimeslotOwner::Cmce, 2, TimeslotOwner::PacketData)
            .expect("local duplex voice must use free slots before touching stale PDCH");
        assert_eq!(duplex_voice_slots, vec![3, 4]);
        assert_eq!(timeslot_alloc.owner(2), Some(TimeslotOwner::PacketData));
        assert_eq!(timeslot_alloc.owner(3), Some(TimeslotOwner::Cmce));
        assert_eq!(timeslot_alloc.owner(4), Some(TimeslotOwner::Cmce));

        for ts in &duplex_voice_slots {
            open_test_dl_circuit(&mut sched, *ts);
        }

        assert!(sched.circuit_is_active(Direction::Dl, 3));
        assert!(sched.circuit_is_active(Direction::Dl, 4));
        assert!(
            sched.packet_data_assignments[1].is_some()
                && sched.packet_data_assignments[2].is_none()
                && sched.packet_data_assignments[3].is_none(),
            "voice on free TS3/TS4 must not kill the unrelated single-slot PDCH on TS2"
        );

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let voice_slot = sched.finalize_ts_for_tick_with_timeslot_allocator(&subscribers, &mut energy_saving, &mut timeslot_alloc);
        assert_eq!(voice_slot.ts, TdmaTime { t: 3, f: 2, m: 1, h: 0 });
        assert_eq!(
            voice_slot.blk1.as_ref().map(|block| block.logical_channel),
            Some(LogicalChannel::Stch),
            "free voice slot must schedule traffic/STCH instead of assigned PDCH maintenance"
        );
        let mut voice_aach = voice_slot.bbk.expect("voice slot should carry AACH").mac_block;
        voice_aach.seek(0);
        let voice_aach = AccessAssign::from_bitbuf(&mut voice_aach).expect("voice AACH should parse");
        assert_eq!(voice_aach.dl_usage, AccessAssignDlUsage::Traffic(4));
        assert_ne!(voice_aach.dl_usage, AccessAssignDlUsage::AssignedControl);

        for ts in &duplex_voice_slots {
            sched.close_circuit(Direction::Dl, *ts);
            timeslot_alloc
                .release(TimeslotOwner::Cmce, *ts)
                .expect("voice release should free the central allocator slot");
        }
        assert_eq!(timeslot_alloc.owner(2), Some(TimeslotOwner::PacketData));
        assert_eq!(timeslot_alloc.owner(3), None);
        assert_eq!(timeslot_alloc.owner(4), None);
        assert!(
            sched
                .packet_data_assignment_for_timeslot(TdmaTime { t: 2, f: 2, m: 1, h: 0 })
                .is_some()
                && sched
                    .packet_data_assignment_for_timeslot(TdmaTime { t: 3, f: 2, m: 1, h: 0 })
                    .is_none()
                && sched
                    .packet_data_assignment_for_timeslot(TdmaTime { t: 4, f: 2, m: 1, h: 0 })
                    .is_none(),
            "voice release must preserve only the original single-slot PDCH"
        );

        let simplex_voice_ts = timeslot_alloc
            .allocate_any_preempting(TimeslotOwner::Cmce, TimeslotOwner::PacketData)
            .expect("next voice call should use a free slot before touching remaining PDCH");
        assert_eq!(simplex_voice_ts, 3);
        assert_eq!(
            timeslot_alloc.owner(2),
            Some(TimeslotOwner::PacketData),
            "remaining data slot must survive while free TS3/TS4 can carry voice"
        );
        open_test_dl_circuit(&mut sched, simplex_voice_ts);
        assert!(sched.circuit_is_active(Direction::Dl, simplex_voice_ts));
        assert!(
            sched.packet_data_assignments[1].is_some(),
            "opening voice on a free slot must not kill unrelated single-slot PDCH"
        );
    }

    #[test]
    fn test_packet_data_resource_does_not_transmit_on_cmce_reserved_slot_before_circuit_open() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress::issi(0x5114);
        let now = TdmaTime { t: 1, f: 2, m: 1, h: 0 };
        let mut timeslot_alloc = TimeslotAllocator::default();
        sched.set_dl_time(now);

        sched.apply_packet_data_assignment_update(
            PacketDataAssignmentUpdate::Replace {
                addr,
                ts_assigned: [false, true, false, false],
                ul_dl_assigned: UlDlAssignment::Both,
                carrier_num: 1001,
            },
            now,
        );
        timeslot_alloc
            .reserve(TimeslotOwner::Cmce, 2)
            .expect("CMCE may reserve TS2 before UMAC circuit state opens");

        let (mut pdu, _) = test_resource_for_issi(addr.ssi, 0);
        let sdu = test_sndcp_llc_sdu();
        pdu.update_len_and_fill_ind(sdu.get_len());
        sched.dl_enqueue_tma(pdu, sdu, None);
        assert!(
            queue_has_resource_for_addr_on_ts(&sched, 2, addr),
            "test setup should queue ordinary packet-data resource on stale TS2 PDCH"
        );

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let slot = sched.finalize_ts_for_tick_with_timeslot_allocator(&subscribers, &mut energy_saving, &mut timeslot_alloc);

        assert_eq!(slot.ts, TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        assert!(
            !matches!(slot.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::SchF)),
            "packet data must not transmit SCH/F on a TS already reserved for CMCE"
        );
        assert!(
            !queue_has_resource_for_addr_on_ts(&sched, 2, addr),
            "reserved voice slot must not keep stale queued packet-data resources"
        );
        assert_eq!(timeslot_alloc.owner(2), Some(TimeslotOwner::Cmce));
    }

    #[test]
    fn test_packet_data_replace_clears_voice_owned_slot_instead_of_resurrecting_after_voice_close() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress::issi(0x5106);
        let now = TdmaTime { t: 1, f: 2, m: 1, h: 0 };
        open_test_dl_circuit(&mut sched, 2);
        sched.packet_data_assignments[1] = Some(PacketDataAssignment {
            addr,
            ul_dl_assigned: UlDlAssignment::Both,
            carrier_num: 1001,
            expires_at: now.add_timeslots(PACKET_DATA_ASSIGNMENT_TTL_TIMESLOTS),
        });

        sched.apply_packet_data_assignment_update(
            PacketDataAssignmentUpdate::Replace {
                addr,
                ts_assigned: [false, true, true, false],
                ul_dl_assigned: UlDlAssignment::Both,
                carrier_num: 1001,
            },
            now.add_timeslots(4),
        );

        assert!(
            sched.packet_data_assignments[1].is_none(),
            "voice-owned TS2 must not retain a stale PDCH assignment"
        );
        assert!(
            sched.packet_data_assignments[2].is_some(),
            "free TS3 may still be used for the refreshed PDCH assignment"
        );

        sched.close_circuit(Direction::Dl, 2);
        assert!(
            sched
                .packet_data_assignment_for_timeslot(TdmaTime { t: 2, f: 2, m: 1, h: 0 })
                .is_none(),
            "closing voice must not resurrect the cleared TS2 packet-data assignment"
        );
    }

    #[test]
    fn test_packet_data_multi_slot_request_preempted_as_single_final_slot() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        let addr = TetraAddress::issi(0x5113);
        let now = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
        sched.apply_packet_data_assignment_update(
            PacketDataAssignmentUpdate::Replace {
                addr,
                ts_assigned: [false, true, true, true],
                ul_dl_assigned: UlDlAssignment::Both,
                carrier_num: 1001,
            },
            now,
        );

        open_test_dl_circuit(&mut sched, 2);

        assert!(
            sched.packet_data_assignments[1..].iter().all(Option::is_none),
            "multi-slot request is normalized to one PDCH slot, so voice on TS2 is final packet-data preemption"
        );

        let release = sched.dltx_queues[0]
            .iter()
            .find_map(|elem| match elem {
                DlSchedElem::Resource(pdu, sdu, _, _) if pdu.addr == Some(addr) && sdu.get_len() == 0 => pdu.chan_alloc_element.as_ref(),
                _ => None,
            })
            .expect("final voice preemption should queue a common-control PDCH release");
        assert_eq!(release.alloc_type, ChanAllocType::QuitAndGo);
        assert_eq!(release.ts_assigned, [false, false, false, false]);
        assert_eq!(release.ul_dl_assigned, UlDlAssignment::Both);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let slot = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(slot.ts, TdmaTime { t: 2, f: 1, m: 1, h: 0 });

        let mut aach = slot.bbk.expect("traffic slot should carry AACH").mac_block;
        aach.seek(0);
        let aach = AccessAssign::from_bitbuf(&mut aach).expect("traffic AACH should parse");
        assert_eq!(aach.dl_usage, AccessAssignDlUsage::Traffic(4));
        assert_ne!(aach.dl_usage, AccessAssignDlUsage::AssignedControl);
    }

    #[test]
    fn test_packet_data_final_voice_preemption_queues_common_control_release() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress::issi(0x5107);
        let now = TdmaTime { t: 1, f: 2, m: 1, h: 0 };
        sched.apply_packet_data_assignment_update(
            PacketDataAssignmentUpdate::Replace {
                addr,
                ts_assigned: [false, true, false, false],
                ul_dl_assigned: UlDlAssignment::Both,
                carrier_num: 1001,
            },
            now,
        );

        open_test_dl_circuit(&mut sched, 2);

        assert!(
            sched.packet_data_assignments[1..].iter().all(Option::is_none),
            "final voice preemption should clear local packet-data assignment state"
        );
        let release = sched.dltx_queues[0]
            .iter()
            .find_map(|elem| match elem {
                DlSchedElem::Resource(pdu, sdu, _, _) if pdu.addr == Some(addr) && sdu.get_len() == 0 => Some(pdu),
                _ => None,
            })
            .expect("final PDCH loss should queue a common-control release resource on TS1");
        let chan_alloc = release
            .chan_alloc_element
            .as_ref()
            .expect("common-control release resource must carry channel allocation");
        assert_eq!(chan_alloc.alloc_type, ChanAllocType::QuitAndGo);
        assert_eq!(chan_alloc.ts_assigned, [false, false, false, false]);
        assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Both);
    }

    #[test]
    fn test_voice_preemption_purges_packet_data_pending_grants_for_that_slot() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress::issi(0x510a);
        let now = TdmaTime { t: 1, f: 2, m: 1, h: 0 };
        sched.apply_packet_data_assignment_update(
            PacketDataAssignmentUpdate::Replace {
                addr,
                ts_assigned: [false, true, false, false],
                ul_dl_assigned: UlDlAssignment::Both,
                carrier_num: 1001,
            },
            now,
        );

        sched.dl_enqueue_packet_data_reservation_grant(2, addr, ReservationRequirement::Req10Slots);
        sched.dltx_queues[0].push(DlSchedElem::PendingGrant(
            addr,
            PendingGrantDebt::new(ReservationRequirement::Req4Slots),
            PendingGrantOrigin::PacketData { assigned_ts: 2 },
        ));
        sched.dltx_next_slot_queue.push(DlSchedElem::PendingGrant(
            addr,
            PendingGrantDebt::new(ReservationRequirement::Req1Slot),
            PendingGrantOrigin::PacketData { assigned_ts: 2 },
        ));
        sched.dltx_next_slot_queue.push(DlSchedElem::PendingGrant(
            addr,
            PendingGrantDebt::new(ReservationRequirement::Req1Slot),
            PendingGrantOrigin::CommonControl,
        ));

        open_test_dl_circuit(&mut sched, 2);

        assert!(
            sched
                .dltx_queues
                .iter()
                .chain(std::iter::once(&sched.dltx_next_slot_queue))
                .flat_map(|queue| queue.iter())
                .all(|elem| !matches!(
                    elem,
                    DlSchedElem::PendingGrant(
                        pending_addr,
                        _,
                        PendingGrantOrigin::PacketData {
                            assigned_ts: 2
                        }
                    ) if *pending_addr == addr
                )),
            "voice preemption on TS2 must purge stale packet-data reservation debt for that assigned slot"
        );
        assert!(
            sched.dltx_next_slot_queue.iter().any(
                |elem| matches!(elem, DlSchedElem::PendingGrant(pending_addr, _, PendingGrantOrigin::CommonControl)
                    if *pending_addr == addr)
            ),
            "common-control reservation grants are not tied to the preempted PDCH slot"
        );
    }

    #[test]
    fn test_non_sndcp_channel_allocation_does_not_activate_packet_data_assignment() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });
        let (mut pdu, _) = test_packet_data_channel_allocation_resource(
            0x5104,
            [false, true, false, false],
            Some(4),
            ChanAllocType::Replace,
            UlDlAssignment::Both,
        );
        let sdu = test_cmce_llc_sdu();
        pdu.update_len_and_fill_ind(sdu.get_len());
        sched.dl_enqueue_tma(pdu, sdu, None);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let _ = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert!(
            sched.packet_data_assignments[1..].iter().all(Option::is_none),
            "CMCE/SDS/non-SNDCP MAC-RESOURCE channel allocations must not drive PDCH maintenance state"
        );
    }

    #[test]
    fn test_packet_data_assignment_does_not_override_active_voice_circuit_aach() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        sched.apply_packet_data_assignment_update(
            PacketDataAssignmentUpdate::Replace {
                addr: TetraAddress::issi(0x5105),
                ts_assigned: [false, true, false, false],
                ul_dl_assigned: UlDlAssignment::Both,
                carrier_num: 1001,
            },
            TdmaTime { t: 1, f: 1, m: 1, h: 0 },
        );
        open_test_dl_circuit(&mut sched, 2);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let slot = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(slot.ts, TdmaTime { t: 2, f: 1, m: 1, h: 0 });
        assert_eq!(slot.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::Stch));

        let mut aach = slot.bbk.expect("traffic slot should carry AACH").mac_block;
        aach.seek(0);
        let aach = AccessAssign::from_bitbuf(&mut aach).expect("traffic AACH should parse");
        assert_eq!(aach.dl_usage, AccessAssignDlUsage::Traffic(4));
        assert_ne!(aach.dl_usage, AccessAssignDlUsage::AssignedControl);
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
            !sched.dltx_queues[0]
                .iter()
                .any(|elem| matches!(elem, DlSchedElem::PendingGrant(pending_addr, debt, _)
                    if *pending_addr == addr
                        && debt.res_req == ReservationRequirement::Req1Slot
                        && debt.remaining_slots == 1)),
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
                    |elem| matches!(elem, DlSchedElem::PendingGrant(pending_addr, debt, _)
                        if *pending_addr == addr
                            && debt.res_req == ReservationRequirement::Req1Slot
                            && debt.remaining_slots == 1)
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
            sched.dltx_queues[0]
                .iter()
                .any(|elem| matches!(elem, DlSchedElem::PendingGrant(pending_addr, debt, _)
                    if *pending_addr == addr
                        && debt.res_req == ReservationRequirement::Req1Slot
                        && debt.remaining_slots == 1)),
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
    fn test_large_group_positive_floor_grant_stch_preempts_busy_response_backlog() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let traffic_ts = 2;
        let requester = TetraAddress::issi(2_260_082);
        for offset in 0..MAX_DLSCHED_ELEMS_PER_TIMESLOT {
            let busy_requester = TetraAddress::issi(2_500_000 + offset as u32);
            sched.dl_enqueue_stealing(
                traffic_ts,
                test_d_tx_granted_stch_block(busy_requester, TransmissionGrant::RequestQueued, UlDlAssignment::Dl),
                busy_requester,
                None,
            );
        }

        let grant_reporter = TxReporter::new_unacked();
        sched.dl_enqueue_stealing(
            traffic_ts,
            test_d_tx_granted_stch_block(requester, TransmissionGrant::Granted, UlDlAssignment::Both),
            requester,
            Some(grant_reporter.clone()),
        );
        assert_eq!(
            sched.dltx_queues[traffic_ts as usize - 1].len(),
            MAX_DLSCHED_ELEMS_PER_TIMESLOT,
            "low-value DL-only busy/queued STCH responses may be shed so a positive floor grant does not grow the queue"
        );

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let (_tch, stch) = sched.dl_build_traffic_block(
            TdmaTime {
                t: traffic_ts,
                f: 2,
                m: 1,
                h: 0,
            },
            &subscribers,
            &mut energy_saving,
        );
        let stch = stch.expect("assigned traffic channel should send one STCH block");

        let mut parsed = BitBuffer::from_bitbuffer(&stch);
        let resource = MacResource::from_bitbuf(&mut parsed).expect("selected STCH should carry MAC-RESOURCE");
        assert_eq!(resource.addr.map(|addr| addr.ssi), Some(requester.ssi));
        assert_eq!(
            resource
                .chan_alloc_element
                .as_ref()
                .expect("positive floor grant must carry channel allocation")
                .ul_dl_assigned,
            UlDlAssignment::Both
        );
        let granted = DTxGranted::from_bitbuf(&mut parsed).expect("selected STCH should carry D-TX GRANTED");
        assert_eq!(granted.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
        assert_eq!(grant_reporter.get_state(), TxState::Transmitted);

        // EN 300 392-2 clauses 14.5.2.2.1 b) and 23.5: the positive
        // D-TX GRANTED with UL+DL allocation is the floor-control response
        // that lets the queued requester enter U-plane. It must not be FIFO
        // delayed behind thousands of DL-only RequestQueued/NotGranted STCH
        // responses in a large GSSI cell.
        assert!(
            sched.dltx_queues[traffic_ts as usize - 1]
                .iter()
                .any(|elem| matches!(elem, DlSchedElem::Stealing(_, addr, _, _) if addr.ssi != requester.ssi)),
            "lower-priority busy responses should remain queued after the requester floor grant is sent"
        );
    }

    #[test]
    fn test_hangtime_group_positive_floor_grant_aach_advertises_traffic_for_older_ms() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });

        let traffic_ts = 2;
        let usage = 4;
        let gssi = 226_333;
        let first_speaker = 2_260_616;
        let mtp3550_requester = TetraAddress::issi(2_260_082);
        open_test_group_circuit(&mut sched, traffic_ts, gssi, first_speaker, usage);
        sched.set_hangtime(traffic_ts, true);
        sched.dl_enqueue_stealing(
            traffic_ts,
            test_d_tx_granted_stch_block(mtp3550_requester, TransmissionGrant::Granted, UlDlAssignment::Both),
            mtp3550_requester,
            None,
        );

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let slot = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(
            slot.ts,
            TdmaTime {
                t: traffic_ts,
                f: 2,
                m: 1,
                h: 0
            }
        );
        assert_eq!(
            slot.blk1.as_ref().map(|block| block.logical_channel),
            Some(LogicalChannel::Stch),
            "positive group D-TX GRANTED should be delivered as assigned-channel FACCH/STCH"
        );
        assert_eq!(
            slot.ul_phy_chan,
            PhysicalChannel::Tp,
            "group requester grant should reopen the uplink traffic path for TCH/S"
        );

        let mut bbk = slot.bbk.expect("traffic slot should carry AACH").mac_block;
        bbk.seek(0);
        let aach = AccessAssign::from_bitbuf(&mut bbk).expect("AACH should parse");
        assert_eq!(
            aach.dl_usage,
            AccessAssignDlUsage::Traffic(usage),
            "EN 300 392-2 clauses 14.5.2.2.1 b) and 23.5: D-TX GRANTED/Granted switches U-plane on, so its FACCH slot must not advertise AssignedControl to older Motorola MSs"
        );
        assert_eq!(
            aach.ul_usage,
            AccessAssignUlUsage::Traffic(usage),
            "EN 300 392-2 clauses 14.5.2.2.1 b) and 23.5: the granted requester must see traffic uplink on the same slot as the positive grant"
        );
    }

    #[test]
    fn test_hangtime_group_d_tx_ceased_aach_stays_assigned_control() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });

        let traffic_ts = 2;
        let gssi = 226_333;
        let first_speaker = 2_260_082;
        let group_addr = TetraAddress::new(gssi, SsiType::Gssi);
        open_test_group_circuit(&mut sched, traffic_ts, gssi, first_speaker, 4);
        sched.set_hangtime(traffic_ts, true);
        sched.dl_enqueue_stealing(
            traffic_ts,
            test_d_tx_ceased_stch_block(group_addr, 7, UlDlAssignment::Dl),
            group_addr,
            None,
        );

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let slot = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);

        let mut bbk = slot.bbk.expect("traffic slot should carry AACH").mac_block;
        bbk.seek(0);
        let aach = AccessAssign::from_bitbuf(&mut bbk).expect("AACH should parse");
        assert_eq!(
            aach.dl_usage,
            AccessAssignDlUsage::AssignedControl,
            "EN 300 392-2 clause 14.5.2.2.1 e): D-TX CEASED remains the floor-withdraw edge and should keep hangtime AACH in assigned-control mode"
        );
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::AssignedOnly);
    }

    #[test]
    fn test_large_group_positive_floor_grant_stch_preempts_llc_wrapped_busy_response_backlog() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let traffic_ts = 2;
        let requester = TetraAddress::issi(2_260_082);
        for offset in 0..MAX_DLSCHED_ELEMS_PER_TIMESLOT {
            let busy_requester = TetraAddress::issi(2_600_000 + offset as u32);
            sched.dl_enqueue_stealing(
                traffic_ts,
                test_llc_wrapped_d_tx_granted_stch_block(busy_requester, TransmissionGrant::RequestQueued, UlDlAssignment::Dl),
                busy_requester,
                None,
            );
        }

        let grant_reporter = TxReporter::new_unacked();
        sched.dl_enqueue_stealing(
            traffic_ts,
            test_llc_wrapped_d_tx_granted_stch_block(requester, TransmissionGrant::Granted, UlDlAssignment::Both),
            requester,
            Some(grant_reporter.clone()),
        );

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let (_tch, stch) = sched.dl_build_traffic_block(
            TdmaTime {
                t: traffic_ts,
                f: 2,
                m: 1,
                h: 0,
            },
            &subscribers,
            &mut energy_saving,
        );
        let stch = stch.expect("assigned traffic channel should send one STCH block");

        let mut parsed = BitBuffer::from_bitbuffer(&stch);
        let resource = MacResource::from_bitbuf(&mut parsed).expect("selected STCH should carry MAC-RESOURCE");
        assert_eq!(resource.addr.map(|addr| addr.ssi), Some(requester.ssi));
        assert_eq!(
            resource
                .chan_alloc_element
                .as_ref()
                .expect("positive floor grant must carry channel allocation")
                .ul_dl_assigned,
            UlDlAssignment::Both
        );
        let stch_payload = BitBuffer::from_bitbuffer_pos(&parsed);
        let mut cmce_payload =
            BsChannelScheduler::cmce_dl_payload_from_tma_sdu(&stch_payload).expect("STCH should carry BL-UDATA/MLE/CMCE payload");
        let granted = DTxGranted::from_bitbuf(&mut cmce_payload).expect("selected STCH should carry D-TX GRANTED");
        assert_eq!(granted.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
        assert_eq!(grant_reporter.get_state(), TxState::Transmitted);

        // Real CMCE->MLE->LLC traffic reaches UMAC wrapped in BL-UDATA with
        // an MLE CMCE discriminator. The scheduler priority must decode that
        // shape as the same clause 14.5.2.2.1 positive floor grant, otherwise
        // the first usable PTT can sit behind thousands of busy responses.
        assert!(
            sched.dltx_queues[traffic_ts as usize - 1]
                .iter()
                .any(|elem| matches!(elem, DlSchedElem::Stealing(_, addr, _, _) if addr.ssi != requester.ssi)),
            "lower-priority wrapped busy responses should remain queued after the requester floor grant is sent"
        );
    }

    #[test]
    fn test_large_group_listener_floor_grant_stch_preempts_wrapped_busy_response_backlog_after_requester() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let traffic_ts = 2;
        let requester = TetraAddress::issi(2_260_082);
        let gssi = TetraAddress::new(226_333, SsiType::Gssi);
        for offset in 0..MAX_DLSCHED_ELEMS_PER_TIMESLOT {
            let busy_requester = TetraAddress::issi(2_700_000 + offset as u32);
            sched.dl_enqueue_stealing(
                traffic_ts,
                test_llc_wrapped_d_tx_granted_stch_block(busy_requester, TransmissionGrant::RequestQueued, UlDlAssignment::Dl),
                busy_requester,
                None,
            );
        }

        let listener_reporter = TxReporter::new_unacked();
        sched.dl_enqueue_stealing(
            traffic_ts,
            test_llc_wrapped_d_tx_granted_stch_block(gssi, TransmissionGrant::GrantedToOtherUser, UlDlAssignment::Dl),
            gssi,
            Some(listener_reporter.clone()),
        );

        let grant_reporter = TxReporter::new_unacked();
        sched.dl_enqueue_stealing(
            traffic_ts,
            test_llc_wrapped_d_tx_granted_stch_block(requester, TransmissionGrant::Granted, UlDlAssignment::Both),
            requester,
            Some(grant_reporter.clone()),
        );

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let (_tch, first_stch) = sched.dl_build_traffic_block(
            TdmaTime {
                t: traffic_ts,
                f: 2,
                m: 1,
                h: 0,
            },
            &subscribers,
            &mut energy_saving,
        );
        let first_stch = first_stch.expect("requester positive grant should be sent first");
        let mut parsed = BitBuffer::from_bitbuffer(&first_stch);
        let first_resource = MacResource::from_bitbuf(&mut parsed).expect("first STCH should carry MAC-RESOURCE");
        assert_eq!(first_resource.addr.map(|addr| addr.ssi), Some(requester.ssi));
        assert_eq!(
            first_resource
                .chan_alloc_element
                .as_ref()
                .expect("positive floor grant must carry channel allocation")
                .ul_dl_assigned,
            UlDlAssignment::Both
        );
        let first_payload = BitBuffer::from_bitbuffer_pos(&parsed);
        let mut first_cmce =
            BsChannelScheduler::cmce_dl_payload_from_tma_sdu(&first_payload).expect("first STCH should carry BL-UDATA/MLE/CMCE payload");
        let first_grant = DTxGranted::from_bitbuf(&mut first_cmce).expect("first STCH should carry D-TX GRANTED");
        assert_eq!(first_grant.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
        assert_eq!(grant_reporter.get_state(), TxState::Transmitted);
        assert_eq!(listener_reporter.get_state(), TxState::Pending);

        let (_tch, second_stch) = sched.dl_build_traffic_block(
            TdmaTime {
                t: traffic_ts,
                f: 2,
                m: 1,
                h: 0,
            },
            &subscribers,
            &mut energy_saving,
        );
        let second_stch = second_stch.expect("listener floor notification should follow requester grant");
        let mut parsed = BitBuffer::from_bitbuffer(&second_stch);
        let second_resource = MacResource::from_bitbuf(&mut parsed).expect("second STCH should carry MAC-RESOURCE");
        assert_eq!(second_resource.addr.map(|addr| addr.ssi), Some(gssi.ssi));
        assert_eq!(
            second_resource
                .chan_alloc_element
                .as_ref()
                .expect("listener floor grant should carry DL assignment")
                .ul_dl_assigned,
            UlDlAssignment::Dl
        );
        let second_payload = BitBuffer::from_bitbuffer_pos(&parsed);
        let mut second_cmce =
            BsChannelScheduler::cmce_dl_payload_from_tma_sdu(&second_payload).expect("second STCH should carry BL-UDATA/MLE/CMCE payload");
        let second_grant = DTxGranted::from_bitbuf(&mut second_cmce).expect("second STCH should carry D-TX GRANTED");
        assert_eq!(
            second_grant.transmission_grant,
            TransmissionGrant::GrantedToOtherUser.into_raw() as u8
        );
        assert_eq!(listener_reporter.get_state(), TxState::Transmitted);

        // EN 300 392-2 clause 14.5.2.2.1 b) requires the SwMI to inform
        // listeners when another MS is granted the floor. The group-addressed
        // notification must stay close to the positive grant and ahead of
        // lower-value storm responses.
        assert!(
            sched.dltx_queues[traffic_ts as usize - 1]
                .iter()
                .any(|elem| matches!(elem, DlSchedElem::Stealing(_, addr, _, _) if addr.ssi != requester.ssi && addr.ssi != gssi.ssi)),
            "lower-priority wrapped busy responses should remain queued after requester and listener floor grants are sent"
        );
    }

    #[test]
    fn test_explicit_channel_allocation_stch_recovery_preempts_ack_only_facch() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let traffic_ts = 2;
        let called = TetraAddress::issi(2_260_618);
        let ack_only = {
            let mut sdu = BitBuffer::new_autoexpand(5);
            sdu.write_bits(0, 5);
            sdu.seek(0);
            test_cmce_stch_block(called, sdu, UlDlAssignment::Both)
        };
        sched.dl_enqueue_stealing(traffic_ts, ack_only, called, None);

        let connect_ack_reporter = TxReporter::new_unacked();
        sched.dl_enqueue_stealing(
            traffic_ts,
            test_llc_wrapped_d_connect_ack_stch_block(called, UlDlAssignment::Both),
            called,
            Some(connect_ack_reporter.clone()),
        );

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let (_tch, stch) = sched.dl_build_traffic_block(
            TdmaTime {
                t: traffic_ts,
                f: 2,
                m: 1,
                h: 0,
            },
            &subscribers,
            &mut energy_saving,
        );
        let stch = stch.expect("assigned traffic channel should send one STCH block");

        let mut parsed = BitBuffer::from_bitbuffer(&stch);
        let resource = MacResource::from_bitbuf(&mut parsed).expect("selected STCH should carry MAC-RESOURCE");
        assert_eq!(resource.addr.map(|addr| addr.ssi), Some(called.ssi));
        assert_eq!(
            resource
                .chan_alloc_element
                .as_ref()
                .expect("explicit channel-allocation STCH recovery must carry channel allocation")
                .ul_dl_assigned,
            UlDlAssignment::Both
        );
        let stch_payload = BitBuffer::from_bitbuffer_pos(&parsed);
        let mut cmce_payload =
            BsChannelScheduler::cmce_dl_payload_from_tma_sdu(&stch_payload).expect("STCH should carry BL-UDATA/MLE/CMCE payload");
        DConnectAcknowledge::from_bitbuf(&mut cmce_payload).expect("selected STCH should carry D-CONNECT ACKNOWLEDGE");
        assert_eq!(connect_ack_reporter.get_state(), TxState::Transmitted);

        // EN 300 392-2 clause 23.8.2.2 permits ambiguity-safe recovery on an
        // assigned channel after a receive-authorization ACK is missed. If such
        // recovery is explicitly enqueued as STCH, a small ACK-only FACCH block
        // must not occupy the next stealing opportunity ahead of the recovery
        // PDU. The first authoritative P2P setup leg remains current-channel
        // acknowledged signalling per clauses 14.5.3.1 and 23.5.4.3.1.
        assert!(
            sched.dltx_queues[traffic_ts as usize - 1]
                .iter()
                .any(|elem| matches!(elem, DlSchedElem::Stealing(_, addr, _, _) if *addr == called)),
            "ACK-only FACCH should remain queued after explicit channel-allocation STCH recovery is sent"
        );
    }

    #[test]
    fn test_preemptive_floor_interrupt_stch_stays_ahead_of_positive_grant() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let traffic_ts = 2;
        let new_speaker = TetraAddress::issi(2_260_616);
        let old_speaker = TetraAddress::issi(2_260_082);
        let grant_reporter = TxReporter::new_unacked();
        let interrupt_reporter = TxReporter::new_unacked();

        sched.dl_enqueue_stealing(
            traffic_ts,
            test_d_tx_granted_stch_block(new_speaker, TransmissionGrant::Granted, UlDlAssignment::Both),
            new_speaker,
            Some(grant_reporter.clone()),
        );
        sched.dl_enqueue_stealing(
            traffic_ts,
            test_d_tx_interrupt_stch_block(old_speaker, UlDlAssignment::Dl),
            old_speaker,
            Some(interrupt_reporter.clone()),
        );

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let (_tch, first_stch) = sched.dl_build_traffic_block(
            TdmaTime {
                t: traffic_ts,
                f: 2,
                m: 1,
                h: 0,
            },
            &subscribers,
            &mut energy_saving,
        );
        let first_stch = first_stch.expect("preemptive handoff should send D-TX INTERRUPT first");

        let mut parsed = BitBuffer::from_bitbuffer(&first_stch);
        let _resource = MacResource::from_bitbuf(&mut parsed).expect("selected STCH should carry MAC-RESOURCE");
        let pdu_type = parsed
            .read_field(5, "cmce_pdu_type_dl")
            .ok()
            .and_then(|bits| CmcePduTypeDl::try_from(bits).ok());
        assert_eq!(pdu_type, Some(CmcePduTypeDl::DTxInterrupt));
        assert_eq!(interrupt_reporter.get_state(), TxState::Transmitted);
        assert_eq!(grant_reporter.get_state(), TxState::Pending);

        let (_tch, second_stch) = sched.dl_build_traffic_block(
            TdmaTime {
                t: traffic_ts,
                f: 2,
                m: 1,
                h: 0,
            },
            &subscribers,
            &mut energy_saving,
        );
        let second_stch = second_stch.expect("positive grant should remain queued after interrupt");
        let mut parsed = BitBuffer::from_bitbuffer(&second_stch);
        let _resource = MacResource::from_bitbuf(&mut parsed).expect("selected STCH should carry MAC-RESOURCE");
        let granted = DTxGranted::from_bitbuf(&mut parsed).expect("second STCH should carry D-TX GRANTED");
        assert_eq!(granted.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
        assert_eq!(grant_reporter.get_state(), TxState::Transmitted);

        // EN 300 392-2 clause 14.5.2.2.1 f): interruption withdraws the
        // current permission before the new floor is advertised. This protects
        // the UMAC air path, not only CMCE's message production order.
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
                            && state.covered.len() == 1
                            && state.covered.contains(&first_issi)
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
    fn test_floor_change_drops_only_requeued_gssi_repeat_state() {
        let mut sched = get_testing_slotter();
        let gssi = 91;
        let other_gssi = 92;
        let old_reporter = TxReporter::new_unacked();
        let fresh_reporter = TxReporter::new_unacked();
        let other_reporter = TxReporter::new_unacked();
        let (old_pdu, old_sdu) = test_resource_for_gssi(gssi, 8);
        let (fresh_pdu, fresh_sdu) = test_resource_for_gssi(gssi, 8);
        let (other_pdu, other_sdu) = test_resource_for_gssi(other_gssi, 8);

        sched.dltx_queues[0].push(DlSchedElem::Resource(
            old_pdu.clone(),
            old_sdu.clone(),
            None,
            Some(GroupDeliveryState::new(
                old_pdu,
                old_sdu,
                vec![1001],
                Some(old_reporter.clone()),
                true,
            )),
        ));
        sched.dltx_queues[0].push(DlSchedElem::Resource(fresh_pdu, fresh_sdu, Some(fresh_reporter.clone()), None));
        sched.dltx_queues[0].push(DlSchedElem::Resource(
            other_pdu.clone(),
            other_sdu.clone(),
            None,
            Some(GroupDeliveryState::new(
                other_pdu,
                other_sdu,
                vec![2001],
                Some(other_reporter.clone()),
                true,
            )),
        ));

        // EN 300 392-2 clause 14.5.2.2.1 moves the active group floor with
        // D-TX GRANTED. Local EG repeats created for old GSSI signalling must
        // not survive that floor change, but fresh unsent signalling for the
        // same GSSI has no repeat snapshot yet and must remain eligible.
        assert_eq!(
            sched.dl_drop_queued_gssi_repeats(TetraAddress::new(gssi, SsiType::Gssi), "test floor change"),
            1
        );
        assert_eq!(old_reporter.get_state(), TxState::Discarded);
        assert_eq!(fresh_reporter.get_state(), TxState::Pending);
        assert_eq!(other_reporter.get_state(), TxState::Pending);
        assert_eq!(sched.dltx_queues[0].len(), 2);
        assert!(
            sched.dltx_queues[0].iter().any(
                |elem| matches!(elem, DlSchedElem::Resource(_, _, Some(reporter), None) if reporter.shares_state_with(&fresh_reporter))
            ),
            "fresh group_state=None signalling for the new floor must stay queued"
        );
        assert!(
            sched.dltx_queues[0].iter().any(|elem| matches!(elem, DlSchedElem::Resource(
                    MacResource {
                        addr: Some(TetraAddress {
                            ssi,
                            ssi_type: SsiType::Gssi
                        }),
                        ..
                    },
                    _,
                    _,
                    Some(_)
                ) if *ssi == other_gssi)),
            "repeat state for a different GSSI must not be dropped"
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
        assert_eq!(reporter.get_state(), TxState::Transmitted);
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
        assert_eq!(reporter.get_state(), TxState::Transmitted);

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
    fn test_all_ones_resource_reports_after_first_complete_transmission_but_still_repeats_for_eg_targets() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });

        let first_issi = 1001;
        let second_issi = 1002;
        let reporter = TxReporter::new_unacked();
        let (pdu, sdu) = test_resource_for_gssi(PREDEFINED_BROADCAST_GSSI, 8);
        sched.dl_enqueue_tma(pdu, sdu, Some(reporter.clone()));

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
        assert_eq!(first.ts, TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        assert_eq!(first.blk1.as_ref().map(|block| block.logical_channel), Some(LogicalChannel::SchF));
        assert_eq!(
            reporter.get_state(),
            TxState::Transmitted,
            "TMA-REPORT is local MAC progress and must complete after the first all-ones TM-SDU is sent"
        );
        assert_eq!(energy_saving.get(&first_issi).and_then(|a| a.awake_until), None);
        assert_eq!(energy_saving.get(&second_issi).and_then(|a| a.awake_until), None);

        for ts in [
            TdmaTime { t: 1, f: 2, m: 1, h: 0 },
            TdmaTime { t: 2, f: 2, m: 1, h: 0 },
            TdmaTime { t: 3, f: 2, m: 1, h: 0 },
            TdmaTime { t: 4, f: 2, m: 1, h: 0 },
        ] {
            sched.tick_start(ts);
        }

        let second = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(second.ts, TdmaTime { t: 1, f: 3, m: 1, h: 0 });
        assert_eq!(
            second.blk1.as_ref().map(|block| block.logical_channel),
            Some(LogicalChannel::SchF),
            "all-ones repeat coverage should continue for later EG receive batches after TMA report completion"
        );
        assert_eq!(reporter.get_state(), TxState::Transmitted);
        assert_eq!(energy_saving.get(&first_issi).and_then(|a| a.awake_until), None);
        assert_eq!(energy_saving.get(&second_issi).and_then(|a| a.awake_until), None);
    }

    #[test]
    fn test_large_stayalive_gssi_resource_skips_group_delivery_state_snapshot() {
        let gssi = 91;
        let member_count = 4096;
        let (pdu, sdu) = test_resource_for_gssi(gssi, 8);
        let addr = pdu.addr.expect("test GSSI resource must be addressed");

        let mut subscribers = SubscriberRegistry::new();
        for offset in 0..member_count {
            let issi = 40_000 + offset;
            subscribers.register(issi);
            assert!(subscribers.affiliate(issi, gssi));
        }

        let mut readiness_cache = GroupReadinessCache::default();
        let energy_saving = HashMap::new();
        let state = BsChannelScheduler::group_state_for_resource(
            addr,
            &pdu,
            &sdu,
            Some(TxReporter::new_unacked()),
            &subscribers,
            &mut readiness_cache,
            &energy_saving,
        );

        // Local scale guard: when no affiliate has an Energy Economy
        // assignment, all members are continuously listening from the
        // scheduler's point of view. EN 300 392-2 clauses 23.5.2.2.7 and
        // 23.7.6 do not require a per-member repeat tracker in that case.
        assert!(
            state.is_none(),
            "StayAlive GSSI signalling must not allocate a per-member repeat snapshot"
        );
    }

    #[test]
    fn test_mixed_stayalive_eg_gssi_resource_tracks_only_energy_economy_targets() {
        let gssi = 91;
        let stayalive_count = 4096;
        let eg_issi = 61_000;
        let (pdu, sdu) = test_resource_for_gssi(gssi, 8);
        let addr = pdu.addr.expect("test GSSI resource must be addressed");

        let mut subscribers = SubscriberRegistry::new();
        for offset in 0..stayalive_count {
            let issi = 56_000 + offset;
            subscribers.register(issi);
            assert!(subscribers.affiliate(issi, gssi));
        }
        subscribers.register(eg_issi);
        assert!(subscribers.affiliate(eg_issi, gssi));

        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            eg_issi,
            EnergySavingAssignment {
                mode: 7,
                frame: Some(3),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let mut readiness_cache = GroupReadinessCache::default();
        let state = BsChannelScheduler::group_state_for_resource(
            addr,
            &pdu,
            &sdu,
            Some(TxReporter::new_unacked()),
            &subscribers,
            &mut readiness_cache,
            &energy_saving,
        )
        .expect("one valid EG member should require repeat tracking");

        // EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6 require EG receive
        // windows to be covered. StayAlive affiliates listen to the ordinary
        // GSSI transmission and must not inflate the retained repeat snapshot.
        assert_eq!(state.targets, vec![eg_issi]);
        assert_eq!(subscribers.group_members(gssi).len(), stayalive_count as usize + 1);
    }

    #[test]
    fn test_fail_open_energy_assignment_does_not_create_gssi_repeat_snapshot() {
        let gssi = 91;
        let issi = 62_000;
        let (pdu, sdu) = test_resource_for_gssi(gssi, 8);
        let addr = pdu.addr.expect("test GSSI resource must be addressed");

        let mut subscribers = SubscriberRegistry::new();
        subscribers.register(issi);
        assert!(subscribers.affiliate(issi, gssi));

        let mut energy_saving = HashMap::new();
        energy_saving.insert(
            issi,
            EnergySavingAssignment {
                mode: 7,
                frame: Some(18),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
        );

        let mut readiness_cache = GroupReadinessCache::default();
        let state = BsChannelScheduler::group_state_for_resource(
            addr,
            &pdu,
            &sdu,
            Some(TxReporter::new_unacked()),
            &subscribers,
            &mut readiness_cache,
            &energy_saving,
        );

        // Frame-18 EG is fail-open in this scheduler. Treat it as StayAlive
        // for repeat-state sizing instead of retaining a stale per-member EG
        // target that can never receive on the unsupported frame.
        assert!(
            state.is_none(),
            "invalid/fail-open EG assignments must not allocate GSSI repeat snapshots"
        );
    }

    #[test]
    fn test_large_stayalive_gssi_resource_transmits_once_without_per_member_repeat() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });

        let gssi = 91;
        let member_count = 2048;
        let reporter = TxReporter::new_unacked();
        let (pdu, sdu) = test_resource_for_gssi(gssi, 8);
        sched.dl_enqueue_tma(pdu, sdu, Some(reporter.clone()));

        let mut subscribers = SubscriberRegistry::new();
        for offset in 0..member_count {
            let issi = 50_000 + offset;
            subscribers.register(issi);
            assert!(subscribers.affiliate(issi, gssi));
        }

        let mut energy_saving = HashMap::new();
        let delivered = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);

        // EN 300 392-2 clauses 14.5.2.1, 23.5.2.2.7, and 23.7.6 keep the
        // downlink address GSSI-scoped. When every affiliate is awake, one
        // group-addressed resource covers the whole local listener set.
        assert_eq!(delivered.ts, TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        assert_eq!(reporter.get_state(), TxState::Transmitted);
        assert!(
            sched.dltx_queues[0].is_empty(),
            "StayAlive GSSI delivery must not create one queued repeat per affiliated ISSI"
        );
        assert_eq!(subscribers.group_members(gssi).len(), member_count as usize);
    }

    #[test]
    fn test_large_stayalive_gssi_facch_transmits_once_without_group_stealing_state() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let gssi = 91;
        let member_count = 4096;
        let reporter = TxReporter::new_unacked();
        sched.dl_enqueue_stealing(
            2,
            BitBuffer::new(SCH_HD_CAP),
            TetraAddress::new(gssi, SsiType::Gssi),
            Some(reporter.clone()),
        );

        let mut subscribers = SubscriberRegistry::new();
        for offset in 0..member_count {
            let issi = 70_000 + offset;
            subscribers.register(issi);
            assert!(subscribers.affiliate(issi, gssi));
        }

        let mut energy_saving = HashMap::new();
        let delivered = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);

        // Same scale guard as MAC-RESOURCE delivery, applied to already encoded
        // FACCH/STCH group signalling. With no EG assignments, there is no
        // member-batch repeat state to retain.
        assert_eq!(delivered.ts, TdmaTime { t: 2, f: 1, m: 1, h: 0 });
        assert_eq!(
            delivered.blk1.as_ref().map(|block| block.logical_channel),
            Some(LogicalChannel::Stch)
        );
        assert_eq!(reporter.get_state(), TxState::Transmitted);
        assert!(
            sched.dltx_queues[1]
                .iter()
                .all(|elem| !matches!(elem, DlSchedElem::Stealing(_, addr, _, Some(_)) if addr.ssi == gssi)),
            "StayAlive GSSI FACCH must not keep a per-member stealing repeat state"
        );
    }

    #[test]
    fn test_large_mixed_eg7_gssi_resource_repeats_by_receive_batch_not_member() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });

        let gssi = 91;
        let stayalive_count = 1024;
        let eg7_count = 1024;
        let reporter = TxReporter::new_unacked();
        let (pdu, sdu) = test_resource_for_gssi(gssi, 8);
        sched.dl_enqueue_tma(pdu, sdu, Some(reporter.clone()));

        let mut subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        for offset in 0..stayalive_count {
            let issi = 80_000 + offset;
            subscribers.register(issi);
            assert!(subscribers.affiliate(issi, gssi));
        }
        for offset in 0..eg7_count {
            let issi = 90_000 + offset;
            subscribers.register(issi);
            assert!(subscribers.affiliate(issi, gssi));
            energy_saving.insert(
                issi,
                EnergySavingAssignment {
                    mode: 7,
                    frame: Some(3),
                    multiframe: Some(1),
                    awake_until: None,
                    suspension_count: 0,
                },
            );
        }

        let first = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(first.ts, TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        assert_eq!(reporter.get_state(), TxState::Pending);
        assert_eq!(
            sched.dltx_queues[0]
                .iter()
                .filter(|elem| matches!(elem, DlSchedElem::Resource(_, _, _, Some(_))))
                .count(),
            1,
            "GSSI repeat must stay one queued resource for the EG7 receive batch, not one per member"
        );
        let repeat_state = sched.dltx_queues[0]
            .iter()
            .find_map(|elem| match elem {
                DlSchedElem::Resource(_, _, _, Some(state)) => Some(state),
                _ => None,
            })
            .expect("mixed EG7 GSSI resource should retain repeat state");
        assert_eq!(
            repeat_state.targets.len(),
            eg7_count as usize,
            "repeat snapshot must track only EG7 listeners, not the StayAlive half of the group"
        );
        assert!(
            energy_saving.values().all(|assignment| assignment.awake_until.is_none()),
            "sleeping EG7 members must not get T.210 from the StayAlive batch"
        );

        sched.tick_start(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        sched.tick_start(TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        sched.tick_start(TdmaTime { t: 3, f: 2, m: 1, h: 0 });
        sched.tick_start(TdmaTime { t: 4, f: 2, m: 1, h: 0 });
        let second = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(second.ts, TdmaTime { t: 1, f: 3, m: 1, h: 0 });
        assert_eq!(reporter.get_state(), TxState::Transmitted);
        assert!(
            sched.dltx_queues[0].is_empty(),
            "GSSI resource should leave the queue after all large receive batches are covered"
        );
        assert!(
            energy_saving
                .values()
                .all(|assignment| assignment.awake_until == Some(TdmaTime { t: 1, f: 3, m: 2, h: 0 })),
            "EG7 receive batch should get T.210 only after its own downlink transmit"
        );
    }

    #[test]
    fn test_large_mixed_eg7_gssi_facch_stealing_repeats_by_receive_batch_not_member() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        open_test_dl_circuit(&mut sched, 2);

        let gssi = 91;
        let stayalive_count = 1024;
        let eg7_count = 1024;
        let reporter = TxReporter::new_unacked();
        sched.dl_enqueue_stealing(
            2,
            BitBuffer::new(SCH_HD_CAP),
            TetraAddress::new(gssi, SsiType::Gssi),
            Some(reporter.clone()),
        );

        let mut subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        for offset in 0..stayalive_count {
            let issi = 110_000 + offset;
            subscribers.register(issi);
            assert!(subscribers.affiliate(issi, gssi));
        }
        for offset in 0..eg7_count {
            let issi = 120_000 + offset;
            subscribers.register(issi);
            assert!(subscribers.affiliate(issi, gssi));
            energy_saving.insert(
                issi,
                EnergySavingAssignment {
                    mode: 7,
                    frame: Some(2),
                    multiframe: Some(1),
                    awake_until: None,
                    suspension_count: 0,
                },
            );
        }

        let first = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(first.ts, TdmaTime { t: 2, f: 1, m: 1, h: 0 });
        assert_eq!(reporter.get_state(), TxState::Pending);
        assert_eq!(
            sched.dltx_queues[1]
                .iter()
                .filter(|elem| matches!(elem, DlSchedElem::Stealing(_, _, _, Some(_))))
                .count(),
            1,
            "GSSI FACCH repeat must remain one queued STCH block for the EG7 receive batch, not one per member"
        );
        let repeat_state = sched.dltx_queues[1]
            .iter()
            .find_map(|elem| match elem {
                DlSchedElem::Stealing(_, _, _, Some(state)) => Some(state),
                _ => None,
            })
            .expect("mixed EG7 GSSI FACCH should retain repeat state");
        assert_eq!(
            repeat_state.targets.len(),
            eg7_count as usize,
            "FACCH repeat snapshot must track only EG7 listeners, not the StayAlive half of the group"
        );
        assert!(
            energy_saving.values().all(|assignment| assignment.awake_until.is_none()),
            "sleeping EG7 members must not get T.210 from the StayAlive FACCH batch"
        );

        sched.tick_start(TdmaTime { t: 2, f: 1, m: 1, h: 0 });
        sched.tick_start(TdmaTime { t: 3, f: 1, m: 1, h: 0 });
        sched.tick_start(TdmaTime { t: 4, f: 1, m: 1, h: 0 });
        sched.tick_start(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let second = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(second.ts, TdmaTime { t: 2, f: 2, m: 1, h: 0 });

        // EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6 require assigned-channel
        // signalling to respect EG receive windows. The scalable behaviour is
        // one GSSI-addressed FACCH repeat per receive batch, not per affiliate.
        assert_eq!(reporter.get_state(), TxState::Transmitted);
        assert!(
            sched.dltx_queues[1].is_empty(),
            "GSSI FACCH delivery should leave the queue after all large receive batches are covered"
        );
        assert!(
            energy_saving
                .values()
                .all(|assignment| assignment.awake_until == Some(TdmaTime { t: 2, f: 2, m: 2, h: 0 })),
            "EG7 FACCH receive batch should get T.210 only after its own downlink transmit"
        );
    }

    #[test]
    fn test_large_gssi_readiness_cache_is_slot_scoped_across_queued_resources() {
        let mut sched = get_testing_slotter();
        let ts_sleeping = TdmaTime { t: 1, f: 2, m: 1, h: 0 };
        let ts_awake = TdmaTime { t: 1, f: 3, m: 1, h: 0 };
        let gssi = 91;
        let group_addr = TetraAddress::new(gssi, SsiType::Gssi);
        let member_count = 4096;
        let resource_count = 64;

        for _ in 0..resource_count {
            let (pdu, sdu) = test_resource_for_gssi(gssi, 8);
            sched.dl_enqueue_tma(pdu, sdu, None);
        }

        let mut subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        for offset in 0..member_count {
            let issi = 130_000 + offset;
            subscribers.register(issi);
            assert!(subscribers.affiliate(issi, gssi));
            energy_saving.insert(
                issi,
                EnergySavingAssignment {
                    mode: 7,
                    frame: Some(ts_awake.f),
                    multiframe: Some(ts_awake.m),
                    awake_until: None,
                    suspension_count: 0,
                },
            );
        }

        let mut sleeping_cache = GroupReadinessCache::default();
        assert!(
            sched
                .dl_take_prioritized_sched_item_with_cache(ts_sleeping, &subscribers, &energy_saving, &mut sleeping_cache)
                .is_none(),
            "no EG7 affiliate listens on the sleeping frame"
        );
        assert_eq!(
            sleeping_cache.targets_by_addr.get(&group_addr).map(Vec::len),
            Some(member_count as usize),
            "large GSSI target list should be built once for the scheduling opportunity"
        );
        assert_eq!(
            sleeping_cache.any_listens_by_addr.get(&group_addr),
            Some(&false),
            "the sleeping-frame GSSI readiness result should be cached across all queued resources"
        );
        assert_eq!(sched.dltx_queues[0].len(), resource_count as usize);

        let mut awake_cache = GroupReadinessCache::default();
        assert!(
            sched
                .dl_take_prioritized_sched_item_with_cache(ts_awake, &subscribers, &energy_saving, &mut awake_cache)
                .is_some(),
            "the same queued GSSI resources should become ready on the EG7 receive frame"
        );
        assert_eq!(
            awake_cache.targets_by_addr.get(&group_addr).map(Vec::len),
            Some(member_count as usize)
        );
        assert_eq!(
            awake_cache.any_listens_by_addr.get(&group_addr),
            Some(&true),
            "the awake-frame readiness result should be cached for the current slot"
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
    fn test_packet_data_single_slot_grant_does_not_count_adjacent_uplink_opportunities() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 2, f: 1, m: 1, h: 0 };
        sched.set_dl_time(grant_base);
        let addr = TetraAddress::issi(0x2260618);
        let blocker = 0x2260001;
        assign_test_packet_data_uplink_slots(&mut sched, addr, &[2, 3, 4]);

        for dist in 0..14 {
            block_test_uplink_slot(&mut sched, grant_base.add_timeslots(dist * 4), blocker);
        }

        let grant = sched.ul_process_cap_req_from(grant_base, 2, addr, &ReservationRequirement::Req1Slot);

        assert!(
            grant.is_none(),
            "single-slot PDCH must not count adjacent TS3/TS4 as assigned-channel opportunities"
        );
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 3, f: 1, m: 1, h: 0 }, PhyBlockNum::Both),
            None,
            "single-slot PDCH must not reserve TS3 when TS2 delay is unencodable"
        );
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 4, f: 1, m: 1, h: 0 }, PhyBlockNum::Both),
            None,
            "single-slot PDCH must not reserve TS4 when TS2 delay is unencodable"
        );
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 2, f: 15, m: 1, h: 0 }, PhyBlockNum::Both),
            None,
            "same-TS search must not reserve an unencodable later TS2 frame"
        );
    }

    #[test]
    fn test_packet_data_single_slot_grant_stays_on_assigned_slot_when_adjacent_voice_exists() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 2, f: 1, m: 1, h: 0 };
        sched.set_dl_time(grant_base);
        let addr = TetraAddress::issi(0x2260618);
        let blocker = 0x2260001;
        assign_test_packet_data_uplink_slots(&mut sched, addr, &[2, 3, 4]);
        open_test_dl_circuit(&mut sched, 3);
        block_test_uplink_slot(&mut sched, grant_base, blocker);

        let (grant, _) = sched
            .ul_process_cap_req_from(grant_base, 2, addr, &ReservationRequirement::Req1Slot)
            .expect("single-slot PDCH grant should use the next opportunity on its assigned slot");

        // Opening voice on TS3 collapses a legacy/stale multislot PDCH state
        // back to one slot. With TS2 busy, reserved access waits for the next
        // TS2 opportunity instead of jumping to an unassigned adjacent slot.
        assert_eq!(grant.granting_delay, BasicSlotgrantGrantingDelay::DelayNOpportunities(1));
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 3, f: 1, m: 1, h: 0 }, PhyBlockNum::Both),
            None,
            "packet data must not reserve uplink capacity on the voice-owned slot"
        );
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 4, f: 1, m: 1, h: 0 }, PhyBlockNum::Both),
            None,
            "single-slot packet data must not reserve adjacent TS4 uplink capacity"
        );
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 2, f: 2, m: 1, h: 0 }, PhyBlockNum::Both),
            Some(addr.ssi),
            "single-slot packet data should reserve the next opportunity on TS2"
        );
    }

    #[test]
    fn test_packet_data_single_slot_pending_grant_waits_when_delay_unrepresentable() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 2, f: 1, m: 1, h: 0 };
        sched.set_dl_time(grant_base);
        let addr = TetraAddress::issi(0x2260618);
        let blocker = 0x2260001;
        assign_test_packet_data_uplink_slots(&mut sched, addr, &[2, 3, 4]);

        for dist in 0..14 {
            block_test_uplink_slot(&mut sched, grant_base.add_timeslots(dist * 4), blocker);
        }

        sched.dl_enqueue_reservation_grant(2, addr, ReservationRequirement::Req10Slots);
        sched.dl_integrate_sched_elems_for_timeslot(grant_base, &SubscriberRegistry::new(), &HashMap::new());

        let slot_grant = sched.dltx_queues[1]
            .iter()
            .find_map(|elem| match elem {
                DlSchedElem::Resource(pdu, _, _, _) if pdu.addr == Some(addr) => pdu.slot_granting_element.as_ref(),
                _ => None,
            })
            .expect("scheduler should emit a wait grant when the single PDCH slot has no representable capacity");

        assert_eq!(slot_grant.capacity_allocation, BasicSlotgrantCapAlloc::Grant10Slots);
        assert_eq!(
            slot_grant.granting_delay,
            BasicSlotgrantGrantingDelay::WaitForAnotherSlotgrantMessage
        );
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 3, f: 1, m: 1, h: 0 }, PhyBlockNum::Both),
            None,
            "single-slot PDCH pending grant must not reserve TS3"
        );
        assert_eq!(
            sched.ul_get_slot_owner(TdmaTime { t: 4, f: 1, m: 1, h: 0 }, PhyBlockNum::Both),
            None,
            "single-slot PDCH pending grant must not reserve TS4"
        );
    }

    #[test]
    fn test_packet_data_single_slot_grant_reuses_same_issi_reserved_capacity() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 2, f: 1, m: 1, h: 0 };
        sched.set_dl_time(grant_base);
        let addr = TetraAddress::issi(0x2260618);
        assign_test_packet_data_uplink_slots(&mut sched, addr, &[2, 3, 4]);

        for dist in 0..14 {
            let ts = grant_base.add_timeslots(dist);
            if [2, 3, 4].contains(&ts.t) && !ts.is_mandatory_clch() {
                block_test_uplink_slot(&mut sched, ts, addr.ssi);
            }
        }

        sched.dl_enqueue_reservation_grant(2, addr, ReservationRequirement::Req10Slots);
        sched.dl_integrate_sched_elems_for_timeslot(grant_base, &SubscriberRegistry::new(), &HashMap::new());

        let slot_grant = sched.dltx_queues[1]
            .iter()
            .find_map(|elem| match elem {
                DlSchedElem::Resource(pdu, _, _, _) if pdu.addr == Some(addr) => pdu.slot_granting_element.as_ref(),
                _ => None,
            })
            .expect("same-ISSI reserved PDCH capacity should be re-grantable");

        assert_eq!(slot_grant.capacity_allocation, BasicSlotgrantCapAlloc::Grant4Slots);
        assert_eq!(slot_grant.granting_delay, BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity);
        assert!(
            sched
                .dltx_next_slot_queue
                .iter()
                .any(|elem| matches!(elem, DlSchedElem::PendingGrant(pending_addr, debt, _)
                    if *pending_addr == addr && debt.remaining_slots == 6)),
            "re-granting same-ISSI PDCH capacity must still preserve remaining WAP/AL reservation debt"
        );
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
    fn test_basic_slotgrant_large_wap_reservation_is_sliced_and_skips_clch() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 2, f: 16, m: 1, h: 0 };
        sched.set_dl_time(grant_base);
        let addr = TetraAddress::issi(0x2260618);

        let (grant, marker) = sched
            .ul_process_cap_req_from(grant_base, 2, addr, &ReservationRequirement::Req51Slots)
            .expect("large WAP/AL reservation should emit a bounded first grant slice");

        assert_eq!(grant.capacity_allocation, BasicSlotgrantCapAlloc::Grant4Slots);
        assert_eq!(grant.granting_delay, BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity);
        assert_eq!(marker, Some(4));

        let mut reserved = 0usize;
        let mut saw_mandatory_clch_gap = false;
        for dist in 0..8 {
            let ts = grant_base.add_timeslots(dist * 4);
            let owner = sched.ul_get_slot_owner(ts, PhyBlockNum::Both);
            if ts.is_mandatory_clch() {
                assert_eq!(owner, None, "mandatory CLCH must not be consumed by a sliced reserved-access grant");
                saw_mandatory_clch_gap = true;
            } else if owner == Some(addr.ssi) {
                reserved += 1;
            }
        }

        assert!(
            saw_mandatory_clch_gap,
            "test vector must cross a mandatory frame-18 CLCH opportunity"
        );
        assert_eq!(
            reserved, MAX_RESERVED_ACCESS_GRANT_CHUNK_SLOTS,
            "Req51Slots must reserve only the bounded first grant slice"
        );
    }

    #[test]
    fn test_pending_large_wap_reservation_requeues_remaining_grant_debt() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 15, m: 1, h: 0 });
        let tx_ts = TdmaTime { t: 1, f: 16, m: 1, h: 0 };
        let addr = TetraAddress::issi(0x2260618);
        sched.dl_enqueue_reservation_grant(tx_ts.t, addr, ReservationRequirement::Req51Slots);

        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();
        let granted = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);

        assert_eq!(granted.ts, tx_ts);
        assert_eq!(
            (0..8)
                .filter(|dist| {
                    let ts = tx_ts.add_timeslots(dist * 4);
                    sched.ul_get_slot_owner(ts, PhyBlockNum::Both) == Some(addr.ssi)
                })
                .count(),
            MAX_RESERVED_ACCESS_GRANT_CHUNK_SLOTS,
            "finalization should reserve only the bounded first grant slice"
        );

        assert!(
            sched.dltx_queues[tx_ts.t as usize - 1]
                .iter()
                .any(|elem| matches!(elem, DlSchedElem::PendingGrant(pending_addr, debt, _)
                    if *pending_addr == addr
                        && debt.remaining_slots == 51 - MAX_RESERVED_ACCESS_GRANT_CHUNK_SLOTS
                        && debt.res_req == ReservationRequirement::Req51Slots)),
            "large reservation must keep debt for the remaining WAP/AL fragments"
        );
    }

    #[test]
    fn test_primary_mcch_bnch_keeps_default_access_sysinfo() {
        let mut sched = get_testing_slotter();
        let subscribers = SubscriberRegistry::new();
        let mut energy_saving = HashMap::new();

        let ts1_bnch = TdmaTime { t: 1, f: 2, m: 1, h: 0 };
        sched.set_dl_time(ts1_bnch.add_timeslots(-1));
        let slot = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(slot.ts, ts1_bnch);
        let mut bnch = slot.blk2.expect("TS1 SCH/HD half-slot should carry BNCH in second half-slot");
        assert_eq!(bnch.logical_channel, LogicalChannel::Bnch);

        let decoded_sysinfo = MacSysinfo::from_bitbuf(&mut bnch.mac_block).expect("BNCH should start with MAC-SYSINFO");
        let _decoded_mle = DMleSysinfo::from_bitbuf(&mut bnch.mac_block).expect("BNCH should carry D-MLE-SYSINFO after MAC-SYSINFO");
        assert_eq!(decoded_sysinfo.option_field, SysinfoOptFieldFlag::DefaultDefForAccCodeA);
        assert!(
            decoded_sysinfo.default_access_code.is_some(),
            "primary MCCH SYSINFO must carry access code A for cold random access"
        );
        assert!(
            decoded_sysinfo.ext_services.is_none(),
            "primary MCCH SYSINFO must not replace access parameters with extended services"
        );

        let ts2_bnch = TdmaTime { t: 2, f: 2, m: 1, h: 0 };
        sched.set_dl_time(ts2_bnch.add_timeslots(-1));
        let slot = sched.finalize_ts_for_tick(&subscribers, &mut energy_saving);
        assert_eq!(slot.ts, ts2_bnch);
        let mut bnch = slot.blk2.expect("TS2 BSCH half-slot should carry BNCH in second half-slot");
        assert_eq!(bnch.logical_channel, LogicalChannel::Bnch);

        let decoded_sysinfo = MacSysinfo::from_bitbuf(&mut bnch.mac_block).expect("BNCH should start with MAC-SYSINFO");
        let _decoded_mle = DMleSysinfo::from_bitbuf(&mut bnch.mac_block).expect("BNCH should carry D-MLE-SYSINFO after MAC-SYSINFO");
        assert_eq!(decoded_sysinfo.option_field, SysinfoOptFieldFlag::ExtServicesBroadcast);
        assert!(decoded_sysinfo.default_access_code.is_none());
        assert!(
            decoded_sysinfo.ext_services.is_some(),
            "auxiliary BNCH should still carry SYSINFO2/extended services for WAP/IP profile advertisement"
        );
    }

    #[test]
    fn test_pending_large_wap_reservation_wait_grant_preserves_debt_without_capacity() {
        for (res_req, expected_cap, expected_slots) in [
            (ReservationRequirement::Req34Slots, BasicSlotgrantCapAlloc::Grant34Slots, 34usize),
            (ReservationRequirement::Req51Slots, BasicSlotgrantCapAlloc::Grant51Slots, 51usize),
        ] {
            let mut sched = get_testing_slotter();
            let tx_ts = TdmaTime { t: 1, f: 3, m: 1, h: 0 };
            let addr = TetraAddress::issi(0x2260618);
            let blocker = 0x2260001;
            sched.set_dl_time(tx_ts);
            sched.dl_enqueue_reservation_grant(tx_ts.t, addr, res_req);

            let first_opportunity = tx_ts.forward_to_timeslot(tx_ts.t);
            for dist in 0..MACSCHED_NUM_FRAMES - 1 {
                let candidate_t = first_opportunity.add_timeslots(dist as i32 * 4);
                if candidate_t.is_mandatory_clch() {
                    continue;
                }
                let index = sched.ul_ts_to_sched_index(&candidate_t);
                let elem = &mut sched.ulsched[candidate_t.t as usize - 1][index];
                elem.ul1 = Some(blocker);
                elem.ul2 = Some(blocker);
            }

            sched.dl_integrate_sched_elems_for_timeslot(tx_ts, &SubscriberRegistry::new(), &HashMap::new());

            let wait_grant = sched.dltx_queues[tx_ts.t as usize - 1]
                .iter()
                .find_map(|elem| match elem {
                    DlSchedElem::Resource(pdu, _, _, _) if pdu.addr == Some(addr) => pdu.slot_granting_element.as_ref(),
                    _ => None,
                })
                .expect("no-capacity large reservation should emit a wait-grant MAC-RESOURCE");
            assert_eq!(wait_grant.capacity_allocation, expected_cap);
            assert_eq!(
                wait_grant.granting_delay,
                BasicSlotgrantGrantingDelay::WaitForAnotherSlotgrantMessage
            );
            assert!(
                (0..MACSCHED_NUM_FRAMES - 1).all(|dist| {
                    let ts = first_opportunity.add_timeslots(dist as i32 * 4);
                    sched.ul_get_slot_owner(ts, PhyBlockNum::Both) != Some(addr.ssi)
                }),
                "wait-grant must not reserve bogus uplink capacity"
            );
            assert!(
                sched
                    .dltx_next_slot_queue
                    .iter()
                    .any(|elem| matches!(elem, DlSchedElem::PendingGrant(pending_addr, debt, _)
                        if *pending_addr == addr
                            && debt.res_req == res_req
                            && debt.remaining_slots == expected_slots)),
                "wait-grant must preserve pending debt for {res_req:?}"
            );
        }
    }

    #[test]
    fn test_voice_open_clears_future_packet_data_reserved_access_on_that_timeslot() {
        let mut sched = get_testing_slotter();
        let grant_base = TdmaTime { t: 2, f: 1, m: 1, h: 0 };
        sched.set_dl_time(grant_base);
        let addr = TetraAddress::issi(0x2260618);

        sched
            .ul_process_cap_req_from(grant_base, 2, addr, &ReservationRequirement::Req34Slots)
            .expect("data reservation should initially occupy future TS2 uplink opportunities");
        assert!(
            (0..45).any(|dist| {
                let ts = grant_base.add_timeslots(dist * 4);
                sched.ul_get_slot_owner(ts, PhyBlockNum::Both) == Some(addr.ssi)
            }),
            "test setup must create future reserved-access ownership"
        );

        open_test_dl_circuit(&mut sched, 2);

        for dist in 0..45 {
            let ts = grant_base.add_timeslots(dist * 4);
            assert_eq!(
                sched.ul_get_slot_owner(ts, PhyBlockNum::Both),
                None,
                "voice on TS2 must clear stale packet-data reserved access at {ts}"
            );
        }
        assert!(sched.circuit_is_active(Direction::Dl, 2));
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

        assert_eq!(
            sched.dltx_queues[ts.t as usize - 1].len(),
            1,
            "ready grant and ACK should coalesce into the existing MAC-RESOURCE before integration"
        );

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
    fn test_mass_random_access_grant_ack_integration_uses_one_resource_per_issi() {
        let mut sched = get_testing_slotter();
        let ts = TdmaTime { t: 1, f: 2, m: 1, h: 0 };
        let member_count = MAX_DLSCHED_ELEMS_PER_TIMESLOT;
        let grant = BasicSlotgrant {
            capacity_allocation: BasicSlotgrantCapAlloc::FirstSubslotGranted,
            granting_delay: BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity,
        };

        for offset in 0..member_count {
            let addr = TetraAddress::issi(70_000 + offset as u32);
            sched.dl_enqueue_grant(ts.t, addr, grant.clone(), None);
            sched.dl_enqueue_random_access_ack(ts.t, addr);
        }
        assert_eq!(
            sched.dltx_queues[ts.t as usize - 1].len(),
            member_count,
            "4096 grant+RA pairs must coalesce before integration instead of creating 8192 protected queue entries"
        );

        let subscribers = SubscriberRegistry::new();
        let energy_saving = HashMap::new();
        sched.dl_integrate_sched_elems_for_timeslot(ts, &subscribers, &energy_saving);

        // EN 300 392-2 clauses 21.4.3.1 and 23.5.2.2.2 bind the MAC
        // random-access acknowledgement and slot grant to the addressed MS.
        // A mass access burst should collapse to one MAC-RESOURCE per ISSI,
        // not leave independent ACK/grant queue entries behind.
        let queue = &sched.dltx_queues[ts.t as usize - 1];
        assert_eq!(queue.len(), member_count);
        assert!(
            queue
                .iter()
                .all(|elem| !matches!(elem, DlSchedElem::Grant(..) | DlSchedElem::RandomAccessAck(_))),
            "ready grant/ACK elements should be integrated into MAC-RESOURCEs"
        );
        for elem in queue {
            let DlSchedElem::Resource(pdu, _sdu, _reporter, _group_state) = elem else {
                panic!("mass grant/ACK integration should produce only MAC-RESOURCE elements");
            };
            assert_eq!(pdu.addr.expect("resource should stay addressed").ssi_type, SsiType::Issi);
            assert!(pdu.random_access_flag);
            assert!(pdu.slot_granting_element.is_some());
        }
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
        let other_addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 5678,
        };

        sched.dl_enqueue_random_access_ack(1, addr);
        assert!(sched.dl_drop_all_except_stolen(1));

        // EN 300 392-2 clause 21.4.3.1 defines random_access_flag as the
        // successful random-access ACK. In a private/group floor transition,
        // the channel-allocation D-TX GRANTED response described by clauses
        // 14.5.1.2.1 b), 14.5.2.2.1 b) and 23.5.2.2.1 is the response that
        // moves the requesting MS into U-plane. A preceding ACK-only STCH may
        // acknowledge random access but must keep the ACK pending for the
        // channel-allocation STCH, and another ISSI must not consume it.
        assert!(
            !sched.take_pending_ra_ack_for_stch(1, other_addr, false),
            "another ISSI in a large group must not mirror this random-access ACK"
        );
        assert!(
            sched.take_pending_ra_ack_for_stch(1, addr, false),
            "ACK-only STCH should acknowledge random access while leaving it pending"
        );
        assert!(
            !sched.take_pending_ra_ack_for_stch(1, other_addr, true),
            "another ISSI in a large group must not consume this random-access ACK"
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
    fn test_pending_random_access_ack_deduplicates_same_issi() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        for _ in 0..16 {
            sched.dl_enqueue_random_access_ack(1, addr);
        }
        assert!(sched.dl_drop_all_except_stolen(1));

        // EN 300 392-2 clause 21.4.3.1 acknowledges random access per
        // addressed MS. Repeated local preservation of the same ACK after a
        // hangtime cleanup must not create unbounded duplicate scheduler state.
        assert_eq!(sched.pending_ra_acks[0].len(), 1);
        assert!(sched.take_pending_ra_ack(1, addr));
        assert!(!sched.take_pending_ra_ack(1, addr));
    }

    #[test]
    fn test_pending_random_access_ack_queue_is_bounded_for_large_group_churn() {
        let mut sched = get_testing_slotter();
        let base_issi = 200_000;
        let total = MAX_PENDING_RA_ACKS_PER_TIMESLOT + 32;

        for offset in 0..total {
            sched.dl_enqueue_random_access_ack(1, TetraAddress::issi(base_issi + offset as u32));
        }
        assert!(sched.dl_drop_all_except_stolen(1));

        // Local robustness guard for large GSSI cells: preserving ACKs across
        // hangtime cleanup is clause 21.4.3.1 compatible, but the BS must not
        // retain unbounded per-timeslot state when thousands of affiliates
        // repeatedly contend for access.
        assert_eq!(sched.pending_ra_acks[0].len(), MAX_PENDING_RA_ACKS_PER_TIMESLOT);
        assert!(
            !sched.take_pending_ra_ack(1, TetraAddress::issi(base_issi)),
            "overflow should shed at least the earliest retained ACK instead of growing without bound"
        );
        assert!(
            sched.take_pending_ra_ack(1, TetraAddress::issi(base_issi + total as u32 - 1)),
            "bounded queue should still retain a recent ACK for consumption by the next STCH"
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
