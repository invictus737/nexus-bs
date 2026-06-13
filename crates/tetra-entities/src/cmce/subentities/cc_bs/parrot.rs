// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

//! Local Parrot/Papagal simplex test service — ISSI 99999.
//!
//! The service records validated UL TCH/S frames from one caller, plays the
//! exact frame payloads back after the caller releases PTT, then clears the
//! private call. Playback is paced by TDMA ticks and intentionally separate
//! from normal local P2P handling.

use std::collections::VecDeque;

use tetra_core::{PhyBlockNum, Sap, TdmaTime, TetraAddress, tetra_entities::TetraEntity};
use tetra_saps::{SapMsg, SapMsgInner, control::call_control::CallControl, tmd::TmdCircuitDataReq};

pub const PARROT_ISSI: u32 = 99_999;
const PARROT_MAX_RECORD_SECONDS: usize = 20;
const TETRA_FRAMES_PER_SECOND: i32 = 18;
const TCH_S_TRAFFIC_FRAMES_PER_SECOND: usize = 17;
const TETRA_TIMESLOTS_PER_SECOND: i32 = TETRA_FRAMES_PER_SECOND * 4;
const PARROT_MAX_FRAMES: usize = PARROT_MAX_RECORD_SECONDS * TCH_S_TRAFFIC_FRAMES_PER_SECOND;
const PARROT_PLAYBACK_START_GUARD_TIMESLOTS: i32 = 12;
const PARROT_PLAYBACK_DRAIN_TIMESLOTS: i32 = 16;
const PARROT_PLAYBACK_GUARD_SECONDS: i32 = 2;
const PARROT_PLAYBACK_DEADLINE_TIMESLOTS: i32 =
    ((PARROT_MAX_RECORD_SECONDS as i32) + PARROT_PLAYBACK_GUARD_SECONDS) * TETRA_TIMESLOTS_PER_SECOND;

#[derive(Clone, Debug)]
struct ParrotFrame {
    data: Vec<u8>,
    raw_tch_s_block: Option<PhyBlockNum>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParrotState {
    Recording,
    Playing,
    Releasing,
}

#[derive(Debug)]
pub struct ParrotSession {
    pub ts: u8,
    pub call_id: u16,
    caller: TetraAddress,
    frames: VecDeque<ParrotFrame>,
    state: ParrotState,
    last_frame_at: Option<TdmaTime>,
    playback_started_at: Option<TdmaTime>,
    playback_release_started_at: Option<TdmaTime>,
    playback_finished: bool,
}

impl ParrotSession {
    pub fn new(ts: u8, call_id: u16, caller: TetraAddress) -> Self {
        Self {
            ts,
            call_id,
            caller,
            frames: VecDeque::new(),
            state: ParrotState::Recording,
            last_frame_at: None,
            playback_started_at: None,
            playback_release_started_at: None,
            playback_finished: false,
        }
    }

    pub fn record_ul_frame(&mut self, ts: u8, data: Vec<u8>, raw_tch_s_block: Option<PhyBlockNum>) -> bool {
        if self.ts != ts || self.state != ParrotState::Recording {
            return false;
        }
        if self.frames.len() >= PARROT_MAX_FRAMES {
            tracing::debug!(
                "CMCE: parrot recording limit reached call_id={} max_frames={}",
                self.call_id,
                PARROT_MAX_FRAMES
            );
            return true;
        }
        self.frames.push_back(ParrotFrame { data, raw_tch_s_block });
        true
    }

    pub fn recorded_len(&self) -> usize {
        self.frames.len()
    }

    pub fn caller_issi(&self) -> u32 {
        self.caller.ssi
    }

    pub fn owns_ts(&self, ts: u8) -> bool {
        self.ts == ts
    }

    pub fn start_playback(&mut self, now: TdmaTime) -> bool {
        if self.state != ParrotState::Recording {
            return false;
        }
        self.state = ParrotState::Playing;
        self.last_frame_at = None;
        self.playback_started_at = Some(now);
        self.playback_release_started_at = None;
        self.playback_finished = false;
        true
    }

    pub fn finish_without_playback(&mut self) -> Option<usize> {
        if self.state != ParrotState::Recording {
            return None;
        }
        let recorded = self.frames.len();
        self.frames.clear();
        self.state = ParrotState::Releasing;
        self.playback_release_started_at = None;
        self.playback_finished = false;
        Some(recorded)
    }

    pub fn is_playing(&self) -> bool {
        self.state == ParrotState::Playing
    }

    pub fn next_playback_msg(&mut self, now: TdmaTime) -> Option<SapMsg> {
        if self.state == ParrotState::Releasing {
            if self
                .playback_release_started_at
                .is_some_and(|release_started_at| release_started_at.age(now) >= PARROT_PLAYBACK_DRAIN_TIMESLOTS)
            {
                self.playback_release_started_at = None;
                self.playback_finished = true;
            }
            return None;
        }
        if self.state != ParrotState::Playing {
            return None;
        }
        if self
            .playback_started_at
            .is_some_and(|started_at| started_at.age(now) >= PARROT_PLAYBACK_DEADLINE_TIMESLOTS)
        {
            let remaining = self.frames.len();
            self.frames.clear();
            self.state = ParrotState::Releasing;
            self.playback_release_started_at = None;
            self.playback_finished = true;
            tracing::warn!(
                "CMCE: parrot playback guard expired call_id={} remaining_frames={}",
                self.call_id,
                remaining
            );
            return None;
        }
        if now.t != self.ts || now.f == 18 {
            return None;
        }
        if self
            .playback_started_at
            .is_some_and(|started_at| started_at.age(now) < PARROT_PLAYBACK_START_GUARD_TIMESLOTS)
        {
            return None;
        }
        if self.last_frame_at == Some(now) {
            return None;
        }

        let Some(frame) = self.frames.pop_front() else {
            self.state = ParrotState::Releasing;
            self.playback_release_started_at = Some(self.last_frame_at.unwrap_or(now));
            return None;
        };
        self.last_frame_at = Some(now);

        Some(SapMsg {
            sap: Sap::TmdSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmdCircuitDataReq(TmdCircuitDataReq {
                ts: self.ts,
                data: frame.data,
                raw_tch_s_block: frame.raw_tch_s_block,
            }),
        })
    }

    pub fn take_playback_finished(&mut self) -> bool {
        std::mem::take(&mut self.playback_finished)
    }

    pub fn floor_granted_msg(&self) -> SapMsg {
        SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: self.call_id,
                source_issi: self.caller.ssi,
                dest_gssi: PARROT_ISSI,
                ts: self.ts,
            }),
        }
    }

    pub fn playback_floor_granted_msg(&self) -> SapMsg {
        SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: self.call_id,
                source_issi: PARROT_ISSI,
                dest_gssi: self.caller.ssi,
                ts: self.ts,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parrot_recording_is_limited_to_twenty_seconds() {
        let mut session = ParrotSession::new(2, 42, TetraAddress::issi(1001));

        for seq in 0..(PARROT_MAX_FRAMES + 5) {
            assert!(session.record_ul_frame(2, vec![seq as u8; 35], None));
        }

        assert_eq!(session.recorded_len(), PARROT_MAX_FRAMES);
    }

    #[test]
    fn parrot_playback_guard_forces_completion() {
        let mut session = ParrotSession::new(2, 42, TetraAddress::issi(1001));
        assert!(session.record_ul_frame(2, vec![0; 35], None));

        let start = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        assert!(session.start_playback(start));
        let after_deadline = start.add_timeslots(PARROT_PLAYBACK_DEADLINE_TIMESLOTS);

        assert!(session.next_playback_msg(after_deadline).is_none());
        assert!(session.take_playback_finished());
        assert_eq!(session.recorded_len(), 0);
    }

    #[test]
    fn parrot_playback_skips_frame_18_and_drains_before_finish() {
        let mut session = ParrotSession::new(2, 42, TetraAddress::issi(1001));
        assert!(session.record_ul_frame(2, vec![1; 35], None));

        let start = TdmaTime { h: 0, m: 1, f: 14, t: 2 };
        assert!(session.start_playback(start));
        let frame_18 = TdmaTime { h: 0, m: 1, f: 18, t: 2 };
        assert!(
            session.next_playback_msg(frame_18).is_none(),
            "frame 18 is not RF-sendable traffic in the current UMAC scheduler"
        );
        assert_eq!(session.recorded_len(), 1);

        let next_multiframe_t2 = frame_18.add_timeslots(4);
        assert!(session.next_playback_msg(next_multiframe_t2).is_some());
        assert_eq!(session.recorded_len(), 0);

        let empty_probe = next_multiframe_t2.add_timeslots(4);
        assert!(session.next_playback_msg(empty_probe).is_none());
        assert!(
            !session.take_playback_finished(),
            "release should wait for a small RF drain guard after the last queued playback frame"
        );

        let after_drain = next_multiframe_t2.add_timeslots(PARROT_PLAYBACK_DRAIN_TIMESLOTS + 4);
        assert!(session.next_playback_msg(after_drain).is_none());
        assert!(session.take_playback_finished());
    }
}
