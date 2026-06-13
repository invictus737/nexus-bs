// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::collections::{HashMap, HashSet, VecDeque};
use tetra_core::{TdmaTime, TimeslotAllocator};

/// Bounded live SDS admission. These entries are operator-injected dashboard
/// broadcasts; without a cap, a disconnected or scripted dashboard could grow
/// process memory unbounded while RF can only drain one item per interval.
pub const LIVE_SDS_QUEUE_MAX_LEN: usize = 256;

/// A one-shot or repeating SDS broadcast message injected at runtime via the dashboard.
///
/// Each message is broadcast to all MSs on the cell (GSSI 0xFFFFFF) using the same
/// SDS-TL TRANSFER mechanism as Home Mode Display. Messages are transmitted at the
/// `home_mode_display` interval (or `sds_broadcast` interval if that is configured),
/// round-robining with any static SDS broadcast so neither displaces the other.
///
/// - `repeat_count = 0` → repeats indefinitely until explicitly deleted.
/// - `repeat_count > 0` → auto-removed after that many transmissions.
#[derive(Debug, Clone)]
pub struct LiveSdsMessage {
    /// Unique ID (monotonically incrementing, assigned by the stack).
    pub id: u32,
    /// Text to broadcast (UTF-8; encoded as ISO-8859-1 on TX, unknown chars → '?').
    pub text: String,
    /// SDS protocol ID. Defaults to 0x82 (ETSI SDS-TL text messaging); vendor PIDs
    /// such as 0xDC can still be supplied explicitly.
    pub protocol_id: u8,
    /// Source ISSI shown on the radio. Defaults to 16777215 (0xFFFFFF, "network").
    pub source_issi: u32,
    /// 0 = repeat forever; >0 = auto-remove after this many transmissions.
    pub repeat_count: u32,
    /// Number of times this message has been transmitted so far.
    pub sent_count: u32,
}

#[derive(Debug, Clone)]
pub struct Subscriber {
    pub issi: u32,
    // Set of attached GSSIs
    pub attached_groups: HashSet<u32>,
}

/// Per-MS energy economy allocation visible to lower layers.
///
/// `mode` follows EN 300 392-2 table 16.39: 0=StayAlive, 1..=7=EG1..EG7.
/// `frame` and `multiframe` are the absolute TDMA start point from table 16.40
/// and are meaningful only for EG1..EG7.
#[derive(Debug, Clone, Copy)]
pub struct EnergySavingAssignment {
    pub mode: u8,
    pub frame: Option<u8>,
    pub multiframe: Option<u8>,
    /// Latest instant until which the MS should remain awake before returning
    /// to its negotiated EG cycle. This covers the initial start-point guard and
    /// later T.210 activity windows.
    pub awake_until: Option<TdmaTime>,
    /// Number of active assigned-channel/call contexts currently suspending
    /// this MS's sleep cycle. EN 300 392-2 clause 23.7.6 suspends EG while
    /// the MS obeys assigned-channel allocation or is active in a call; a
    /// counter avoids prematurely resuming during overlapping contexts.
    pub suspension_count: u16,
}

impl EnergySavingAssignment {
    pub fn stay_alive() -> Self {
        Self {
            mode: 0,
            frame: None,
            multiframe: None,
            awake_until: None,
            suspension_count: 0,
        }
    }

    pub fn sleep_frames(mode: u8) -> Option<u16> {
        match mode {
            1 => Some(1),
            2 => Some(2),
            3 => Some(5),
            4 => Some(8),
            5 => Some(17),
            6 => Some(71),
            7 => Some(359),
            _ => None,
        }
    }

    pub fn receive_cycle_uses_frame(mode: u8, frame: u8, multiframe: u8, target_frame: u8) -> bool {
        let Some(sleep_frames) = Self::sleep_frames(mode) else {
            return false;
        };
        if !matches!(frame, 1..=18) || !matches!(multiframe, 1..=60) || !matches!(target_frame, 1..=18) {
            return false;
        }

        let start_index = (multiframe as i32 - 1) * 18 + (frame as i32 - 1);
        let cycle = sleep_frames as i32 + 1;
        let full_cycle = 18 * 60;
        let mut offset = 0;
        while offset < full_cycle {
            let current_index = (start_index + offset).rem_euclid(full_cycle);
            if (current_index % 18) + 1 == target_frame as i32 {
                return true;
            }
            offset += cycle;
        }
        false
    }

    pub fn is_energy_economy(self) -> bool {
        if Self::sleep_frames(self.mode).is_none() {
            return false;
        }
        let Some(frame) = self.frame else {
            return false;
        };
        let Some(multiframe) = self.multiframe else {
            return false;
        };

        // This BS does not yet advertise full frame-18 receive support for EG
        // sleep cycles. MM allocation avoids EG cycles requiring frame 18; this
        // guard keeps stale or externally injected assignments fail-open until
        // frame-18 EG receive support is complete.
        matches!(frame, 1..=17) && matches!(multiframe, 1..=60) && !Self::receive_cycle_uses_frame(self.mode, frame, multiframe, 18)
    }

    pub fn listens_at(self, ts: TdmaTime) -> bool {
        if !self.is_energy_economy() {
            return true;
        }

        if self.suspension_count > 0 {
            return true;
        }

        if let Some(awake_until) = self.awake_until {
            if ts.diff(awake_until) <= 0 {
                return true;
            }
        }

        let Some(sleep_frames) = Self::sleep_frames(self.mode) else {
            return true;
        };
        let Some(frame) = self.frame else {
            return true;
        };
        let Some(multiframe) = self.multiframe else {
            return true;
        };

        let start_index = (multiframe as i32 - 1) * 18 + (frame as i32 - 1);
        let current_index = (ts.m as i32 - 1) * 18 + (ts.f as i32 - 1);
        let cycle = sleep_frames as i32 + 1;
        (current_index - start_index).rem_euclid(18 * 60) % cycle == 0
    }

    pub fn mark_awake_from_signalling_activity(&mut self, activity_time: TdmaTime) {
        if self.is_energy_economy() {
            // EN 300 392-2 T.210 default is 18 TDMA frames.
            let t210_until = activity_time.add_timeslots(18 * 4);
            self.awake_until = Some(match self.awake_until {
                Some(existing) if existing.diff(t210_until) >= 0 => existing,
                _ => t210_until,
            });
        }
    }

    pub fn suspend_for_assigned_channel(&mut self) {
        if self.is_energy_economy() {
            self.suspension_count = self.suspension_count.saturating_add(1);
        }
    }

    pub fn resume_from_assigned_channel(&mut self, activity_time: TdmaTime) {
        if self.suspension_count > 0 {
            self.suspension_count -= 1;
        }
        if self.suspension_count == 0 {
            self.mark_awake_from_signalling_activity(activity_time);
        }
    }
}

/// Centralized subscriber registry tracking locally registered ISSIs and their group affiliations.
#[derive(Debug, Clone)]
pub struct SubscriberRegistry {
    /// Registered ISSIs → Subscriber information
    subscribers: HashMap<u32, Subscriber>,
    /// GSSI → registered ISSIs currently affiliated to that group.
    group_members_by_gssi: HashMap<u32, HashSet<u32>>,
    /// Set of all GSSIs with at least one local affiliate
    all_attached_groups: HashSet<u32>,
}

impl SubscriberRegistry {
    pub fn new() -> Self {
        Self {
            subscribers: HashMap::new(),
            group_members_by_gssi: HashMap::new(),
            all_attached_groups: HashSet::new(),
        }
    }

    pub fn is_registered(&self, issi: u32) -> bool {
        self.subscribers.contains_key(&issi)
    }

    /// Tolerant registration; if ISSI already registered, we overwrite it with a fresh Subscriber struct
    pub fn register(&mut self, issi: u32) {
        self.deregister(issi); // Clean up any existing registration to prevent stale affiliations
        self.subscribers.insert(
            issi,
            Subscriber {
                issi,
                attached_groups: HashSet::new(),
            },
        );
    }

    /// Gets mutable ref to subscriber only after MM explicitly registered it.
    pub fn get_subscriber_mut(&mut self, issi: u32) -> Option<&mut Subscriber> {
        self.subscribers.get_mut(&issi)
    }

    /// Deregister an ISSI, removing it from the registry and cleaning up any group affiliations
    pub fn deregister(&mut self, issi: u32) {
        if let Some(subscriber) = self.subscribers.remove(&issi) {
            for gssi in &subscriber.attached_groups {
                let remove_group = if let Some(members) = self.group_members_by_gssi.get_mut(gssi) {
                    members.remove(&issi);
                    members.is_empty()
                } else {
                    false
                };
                if remove_group {
                    self.group_members_by_gssi.remove(gssi);
                }
                if !self.group_members_by_gssi.contains_key(gssi) {
                    self.all_attached_groups.remove(gssi);
                }
            }
        }
    }

    /// Add GSSI to subscriber's attached groups and global set
    pub fn affiliate(&mut self, issi: u32, gssi: u32) -> bool {
        let Some(subscriber) = self.get_subscriber_mut(issi) else {
            return false;
        };
        subscriber.attached_groups.insert(gssi);
        self.group_members_by_gssi.entry(gssi).or_default().insert(issi);
        self.all_attached_groups.insert(gssi);
        true
    }

    /// Remove GSSI from subscriber's attached groups. Update global set if no more subscribers are affiliated with this GSSI.
    pub fn deaffiliate(&mut self, issi: u32, gssi: u32) -> bool {
        let Some(subscriber) = self.get_subscriber_mut(issi) else {
            return false;
        };
        if subscriber.attached_groups.remove(&gssi) {
            let remove_group = if let Some(members) = self.group_members_by_gssi.get_mut(&gssi) {
                members.remove(&issi);
                members.is_empty()
            } else {
                false
            };
            if remove_group {
                self.group_members_by_gssi.remove(&gssi);
            }
            if !self.group_members_by_gssi.contains_key(&gssi) {
                self.all_attached_groups.remove(&gssi);
            }
            return true;
        }
        false
    }

    /// Check if any subscriber is affiliated with the given GSSI
    pub fn has_group_members(&self, gssi: u32) -> bool {
        self.all_attached_groups.contains(&gssi)
    }

    /// Check if a registered ISSI is affiliated with the given GSSI.
    pub fn contains_group_member(&self, gssi: u32, issi: u32) -> bool {
        self.group_members_by_gssi.get(&gssi).is_some_and(|members| members.contains(&issi))
    }

    /// Iterate all currently registered ISSIs affiliated with the given GSSI
    /// without allocating a temporary Vec.
    pub fn group_member_issis(&self, gssi: u32) -> impl Iterator<Item = u32> + '_ {
        self.group_members_by_gssi
            .get(&gssi)
            .into_iter()
            .flat_map(|members| members.iter().copied())
    }

    /// Returns all currently registered ISSIs affiliated with the given GSSI.
    pub fn group_members(&self, gssi: u32) -> Vec<u32> {
        self.group_member_issis(gssi).collect()
    }

    /// Returns all currently registered ISSIs.
    ///
    /// Used by BrewEntity after Brew reconnection to issue D-LOCATION-UPDATE-COMMAND
    /// to all locally registered MS, forcing them to re-affiliate with the BS.
    /// Without this, MS units that were registered before a Brew disconnect believe
    /// they are still affiliated and do not re-register, causing PTT denial until
    /// they are manually power-cycled or the BS service is restarted.
    pub fn all_registered_issis(&self) -> impl Iterator<Item = u32> + '_ {
        self.subscribers.keys().copied()
    }
}

/// Runtime override for the built-in WX/METAR service, edited from the dashboard.
///
/// Mirrors the editable subset of `[wx_service]` config. When `Some`, it takes precedence
/// over the config so toggles/edits apply immediately without a restart; the dashboard
/// also writes the new values back to the TOML so they persist. `None` means "no override
/// — use the config value".
#[derive(Debug, Clone, Default)]
pub struct WxRuntimeOverride {
    pub enabled: bool,
    pub service_issi: u32,
    pub periodic_enabled: bool,
    pub periodic_issi: u32,
    pub periodic_is_group: bool,
    pub periodic_icao: String,
    pub periodic_interval_secs: u64,
}

/// Mutable, stack-editable state (mutex-protected).
#[derive(Debug, Clone)]
pub struct StackState {
    pub timeslot_alloc: TimeslotAllocator,
    /// Backhaul/network connection to SwMI (e.g., Brew/TetraPack). False -> fallback mode.
    pub network_connected: bool,
    /// Operator RF carrier inhibit latch. Runtime-only; defaults to carrier active after restart.
    pub carrier_inhibited: bool,
    /// Centralized subscriber registry for local-first routing decisions.
    pub subscribers: SubscriberRegistry,
    /// Optional sidecar file storing local ISSIs that successfully registered.
    /// Nexus-BS uses this after a process restart to trigger ETSI
    /// infrastructure-initiated registration for stations that are still
    /// camped on the cell but absent from fresh in-memory MM state.
    pub subscriber_recovery_path: Option<String>,
    /// Energy economy allocations by local ISSI, consumed by lower layers when
    /// scheduling downlink signalling for sleeping MSs.
    pub energy_saving: HashMap<u32, EnergySavingAssignment>,
    /// Queue of live SDS messages injected at runtime via the dashboard.
    /// Transmitted round-robin alongside the static Home Mode Display text.
    pub live_sds_queue: VecDeque<LiveSdsMessage>,
    /// Monotonically incrementing ID counter for live SDS messages.
    pub next_live_sds_id: u32,
    /// Runtime ISSI whitelist override edited from the dashboard. When `Some`, it takes
    /// precedence over the config file's `[security] issi_whitelist` so changes apply
    /// immediately without a restart. An empty Vec here means "open network" (all ISSIs
    /// allowed), exactly like an empty whitelist in config. `None` means "no override —
    /// fall back to the config value". The dashboard also writes the new list back to the
    /// TOML so it survives a restart.
    pub issi_whitelist_override: Option<Vec<u32>>,
    /// Runtime override for the WX/METAR service (dashboard toggle). See WxRuntimeOverride.
    pub wx_override: Option<WxRuntimeOverride>,
}

#[cfg(test)]
mod energy_saving_tests {
    use super::*;

    #[test]
    fn test_register_deregister() {
        let mut reg = SubscriberRegistry::new();
        assert!(!reg.is_registered(1001));
        reg.register(1001);
        assert!(reg.is_registered(1001));
        reg.deregister(1001);
        assert!(!reg.is_registered(1001));
    }

    #[test]
    fn test_affiliate_deaffiliate() {
        let mut reg = SubscriberRegistry::new();
        reg.register(1001);
        reg.affiliate(1001, 91);
        assert!(reg.has_group_members(91));
        reg.deaffiliate(1001, 91);
        assert!(!reg.has_group_members(91));
    }

    #[test]
    fn test_has_group_members() {
        let mut reg = SubscriberRegistry::new();
        reg.register(1001);
        reg.register(1002);
        reg.register(1003);
        reg.affiliate(1001, 100);
        reg.affiliate(1002, 100);
        reg.affiliate(1003, 100);
        assert!(reg.has_group_members(100));

        // Deaffiliate one, should still have members
        reg.deaffiliate(1001, 100);
        assert!(reg.has_group_members(100));

        // Deregister a user, should still have members
        reg.deregister(1002);
        assert!(reg.has_group_members(100));

        // Deregister last user, should have no members
        reg.deregister(1003);
        assert!(!reg.has_group_members(100));
    }

    #[test]
    fn test_has_group_members_empty() {
        let reg = SubscriberRegistry::new();
        assert!(!reg.has_group_members(999));
    }

    #[test]
    fn test_group_members_lists_affiliated_issis() {
        let mut reg = SubscriberRegistry::new();
        reg.register(1001);
        reg.register(1002);
        reg.register(1003);
        reg.affiliate(1001, 91);
        reg.affiliate(1003, 91);
        reg.affiliate(1002, 92);

        assert!(reg.contains_group_member(91, 1001));
        assert!(reg.contains_group_member(91, 1003));
        assert!(!reg.contains_group_member(91, 1002));
        let mut iter_members: Vec<u32> = reg.group_member_issis(91).collect();
        iter_members.sort_unstable();
        assert_eq!(iter_members, vec![1001, 1003]);
        let mut members = reg.group_members(91);
        members.sort_unstable();
        assert_eq!(members, vec![1001, 1003]);
        assert!(reg.group_members(999).is_empty());
    }

    #[test]
    fn test_group_members_index_survives_large_group_churn() {
        let mut reg = SubscriberRegistry::new();
        let gssi = 91;
        let first_issi = 10_000;
        let members = 4096;

        for offset in 0..members {
            let issi = first_issi + offset;
            reg.register(issi);
            assert!(reg.affiliate(issi, gssi));
        }

        assert!(reg.has_group_members(gssi));
        assert_eq!(reg.group_members(gssi).len(), members as usize);

        assert!(reg.deaffiliate(first_issi, gssi));
        reg.deregister(first_issi + 1);
        assert!(
            !reg.is_registered(first_issi + 1),
            "deregister should remove indexed group membership"
        );

        let mut remaining = reg.group_members(gssi);
        remaining.sort_unstable();
        assert_eq!(remaining.len(), members as usize - 2);
        assert!(!remaining.contains(&first_issi));
        assert!(!remaining.contains(&(first_issi + 1)));

        for offset in 2..members {
            assert!(reg.deaffiliate(first_issi + offset, gssi));
        }
        assert!(!reg.has_group_members(gssi));
        assert!(reg.group_members(gssi).is_empty());
    }

    #[test]
    fn test_register_overwrites_existing_subscriber() {
        let mut reg = SubscriberRegistry::new();
        reg.register(1001);
        reg.affiliate(1001, 91);
        assert!(reg.has_group_members(91));

        reg.register(1001);

        assert!(reg.is_registered(1001));
        reg.deaffiliate(1001, 91);
        assert!(!reg.has_group_members(91));
    }

    #[test]
    fn test_all_registered_issis() {
        let mut reg = SubscriberRegistry::new();
        reg.register(1001);
        reg.register(1002);
        reg.register(1003);
        let mut issis: Vec<u32> = reg.all_registered_issis().collect();
        issis.sort_unstable();
        assert_eq!(issis, vec![1001, 1002, 1003]);

        reg.deregister(1002);
        let mut issis: Vec<u32> = reg.all_registered_issis().collect();
        issis.sort_unstable();
        assert_eq!(issis, vec![1001, 1003]);
    }

    #[test]
    fn test_unknown_affiliation_does_not_register_subscriber() {
        let mut reg = SubscriberRegistry::new();

        assert!(!reg.affiliate(1001, 91));
        assert!(!reg.deaffiliate(1001, 91));
        assert!(!reg.is_registered(1001));
        assert!(reg.all_registered_issis().next().is_none());
        assert!(!reg.has_group_members(91));
    }
}

impl Default for StackState {
    fn default() -> Self {
        Self {
            timeslot_alloc: TimeslotAllocator::default(),
            network_connected: false,
            carrier_inhibited: false,
            subscribers: SubscriberRegistry::new(),
            subscriber_recovery_path: None,
            energy_saving: HashMap::new(),
            live_sds_queue: VecDeque::new(),
            next_live_sds_id: 1,
            issi_whitelist_override: None,
            wx_override: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_saving_sleep_frames_match_etsi_table_23_9() {
        assert_eq!(EnergySavingAssignment::sleep_frames(1), Some(1));
        assert_eq!(EnergySavingAssignment::sleep_frames(2), Some(2));
        assert_eq!(EnergySavingAssignment::sleep_frames(3), Some(5));
        assert_eq!(EnergySavingAssignment::sleep_frames(4), Some(8));
        assert_eq!(EnergySavingAssignment::sleep_frames(5), Some(17));
        assert_eq!(EnergySavingAssignment::sleep_frames(6), Some(71));
        assert_eq!(EnergySavingAssignment::sleep_frames(7), Some(359));
        assert_eq!(EnergySavingAssignment::sleep_frames(0), None);
        assert_eq!(EnergySavingAssignment::sleep_frames(8), None);
    }

    #[test]
    fn energy_saving_listens_on_absolute_start_point_and_cycle() {
        let start = TdmaTime::default();
        let assignment = EnergySavingAssignment {
            mode: 3,
            frame: Some(start.f),
            multiframe: Some(start.m),
            awake_until: None,
            suspension_count: 0,
        };

        assert!(assignment.listens_at(start));
        assert!(!assignment.listens_at(start.add_timeslots(4)));
        assert!(assignment.listens_at(start.add_timeslots(6 * 4)));
    }

    #[test]
    fn energy_saving_invalid_start_point_fails_open_to_stay_alive() {
        for assignment in [
            EnergySavingAssignment {
                mode: 1,
                frame: Some(0),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
            EnergySavingAssignment {
                mode: 1,
                frame: Some(19),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            },
            EnergySavingAssignment {
                mode: 1,
                frame: Some(1),
                multiframe: Some(0),
                awake_until: None,
                suspension_count: 0,
            },
            EnergySavingAssignment {
                mode: 1,
                frame: Some(1),
                multiframe: Some(61),
                awake_until: None,
                suspension_count: 0,
            },
        ] {
            assert!(!assignment.is_energy_economy());
            assert!(assignment.listens_at(TdmaTime::default()));
        }
    }

    #[test]
    fn energy_saving_frame_18_assignment_fails_open_for_this_bs_scheduler() {
        let assignment = EnergySavingAssignment {
            mode: 5,
            frame: Some(18),
            multiframe: Some(1),
            awake_until: None,
            suspension_count: 0,
        };

        // EN 300 392-2 clause 23.7.6 defines EG start point as frame plus
        // multiframe. This BS scheduler does not emit scheduled MAC-RESOURCE on
        // frame 18, so stale/external frame-18 assignments must not activate EG
        // gating and silently starve downlink signalling.
        assert!(!assignment.is_energy_economy());
        assert!(assignment.listens_at(TdmaTime { t: 1, f: 17, m: 1, h: 0 }));
        assert!(assignment.listens_at(TdmaTime { t: 1, f: 18, m: 1, h: 0 }));
        assert!(assignment.listens_at(TdmaTime { t: 1, f: 1, m: 2, h: 0 }));
    }

    #[test]
    fn energy_saving_assignment_cycle_that_reaches_frame_18_fails_open_for_this_bs_scheduler() {
        for (mode, frame) in [(1, 16), (2, 15), (3, 12), (4, 9)] {
            let assignment = EnergySavingAssignment {
                mode,
                frame: Some(frame),
                multiframe: Some(1),
                awake_until: None,
                suspension_count: 0,
            };

            // EN 300 392-2 clause 23.7.6 derives later EG receive frames from
            // the negotiated start point and table 23.9 sleep interval. This
            // scheduler does not place scheduled SCH/F resources on frame 18,
            // so assignments whose recurrence reaches frame 18 fail open to
            // StayAlive behaviour.
            assert!(EnergySavingAssignment::receive_cycle_uses_frame(mode, frame, 1, 18));
            assert!(!assignment.is_energy_economy());
            assert!(assignment.listens_at(TdmaTime { t: 1, f: 18, m: 1, h: 0 }));
        }
    }

    #[test]
    fn energy_saving_eg6_eg7_cycles_wrap_across_multiframe_boundary() {
        let eg6_start = TdmaTime { t: 1, f: 17, m: 1, h: 0 };
        let eg6 = EnergySavingAssignment {
            mode: 6,
            frame: Some(eg6_start.f),
            multiframe: Some(eg6_start.m),
            awake_until: None,
            suspension_count: 0,
        };
        assert!(eg6.listens_at(eg6_start));
        assert!(!eg6.listens_at(eg6_start.add_timeslots(4)));
        assert!(eg6.listens_at(eg6_start.add_timeslots((71 + 1) * 4)));
        assert!(eg6.listens_at(eg6_start.add_timeslots((71 + 1) * 2 * 4)));

        let eg7_start = TdmaTime { t: 1, f: 10, m: 50, h: 0 };
        let eg7 = EnergySavingAssignment {
            mode: 7,
            frame: Some(eg7_start.f),
            multiframe: Some(eg7_start.m),
            awake_until: None,
            suspension_count: 0,
        };
        assert!(eg7.listens_at(eg7_start));
        assert!(!eg7.listens_at(eg7_start.add_timeslots(4)));
        assert!(eg7.listens_at(eg7_start.add_timeslots((359 + 1) * 4)));
        assert!(eg7.listens_at(eg7_start.add_timeslots((359 + 1) * 2 * 4)));
    }

    #[test]
    fn energy_saving_t210_activity_temporarily_keeps_ms_awake() {
        let start = TdmaTime::default();
        let mut assignment = EnergySavingAssignment {
            mode: 7,
            frame: Some(start.f),
            multiframe: Some(start.m),
            awake_until: None,
            suspension_count: 0,
        };

        assert!(!assignment.listens_at(start.add_timeslots(4)));
        assignment.mark_awake_from_signalling_activity(start);
        assert!(assignment.listens_at(start.add_timeslots(4)));
        assert!(assignment.listens_at(start.add_timeslots(18 * 4)));
        assert!(!assignment.listens_at(start.add_timeslots(18 * 4 + 4)));
    }

    #[test]
    fn energy_saving_t210_does_not_shorten_future_start_guard() {
        let start = TdmaTime::default();
        let future_start = start.add_timeslots(36 * 4);
        let mut assignment = EnergySavingAssignment {
            mode: 7,
            frame: Some(future_start.f),
            multiframe: Some(future_start.m),
            awake_until: Some(future_start),
            suspension_count: 0,
        };

        assignment.mark_awake_from_signalling_activity(start);

        assert_eq!(assignment.awake_until, Some(future_start));
        assert!(assignment.listens_at(start.add_timeslots(30 * 4)));
        assert!(assignment.listens_at(future_start));
    }

    #[test]
    fn energy_saving_assigned_channel_suspension_resumes_with_t210() {
        let start = TdmaTime::default();
        let mut assignment = EnergySavingAssignment {
            mode: 7,
            frame: Some(start.f),
            multiframe: Some(start.m),
            awake_until: None,
            suspension_count: 0,
        };
        let sleeping_frame = start.add_timeslots(4);

        assert!(!assignment.listens_at(sleeping_frame));
        assignment.suspend_for_assigned_channel();
        assignment.suspend_for_assigned_channel();
        assert!(assignment.listens_at(sleeping_frame));

        assignment.resume_from_assigned_channel(sleeping_frame);
        assert_eq!(assignment.suspension_count, 1);
        assert!(assignment.listens_at(sleeping_frame.add_timeslots(18 * 4 + 4)));

        assignment.resume_from_assigned_channel(sleeping_frame);
        assert_eq!(assignment.suspension_count, 0);
        assert!(assignment.listens_at(sleeping_frame.add_timeslots(18 * 4)));
        assert!(!assignment.listens_at(sleeping_frame.add_timeslots(18 * 4 + 4)));
    }
}
