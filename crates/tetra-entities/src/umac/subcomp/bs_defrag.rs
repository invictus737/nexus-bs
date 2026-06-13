// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::collections::HashMap;

use tetra_core::{BitBuffer, TdmaTime, TetraAddress, Todo};

use crate::umac::subcomp::defrag::{DefragBuffer, DefragBufferState};

const DEFRAG_BUF_MAX_LEN: usize = 4096;
const DEFRAG_TS_BEFORE_TIMEOUT: i32 = 9 * 4;

/// Defragmenter suitable for BS use
/// Maintains a set of DefragBuffers per timeslot, indexed by full TETRA address.
/// This allows multiple MSes to send fragmented data in the same timeslot.
pub struct BsDefrag {
    pub buffers: [HashMap<TetraAddress, DefragBuffer>; 4],
}

impl BsDefrag {
    pub fn new() -> Self {
        Self {
            buffers: [HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new()],
        }
    }

    pub fn reset(&mut self) {
        for map in &mut self.buffers {
            map.clear();
        }
    }

    pub fn age_buffers(&mut self, t: TdmaTime) {
        for map in &mut self.buffers {
            map.retain(|_, buffer| {
                if buffer.state != DefragBufferState::Inactive && t.diff(buffer.t_last) >= DEFRAG_TS_BEFORE_TIMEOUT {
                    tracing::info!("defrag_buffer for {} timed out", buffer.t_last.t);
                    false
                } else {
                    true
                }
            });
        }
    }

    pub fn discard_incomplete_for_addr(&mut self, t: TdmaTime, addr: TetraAddress, reason: &str) -> bool {
        let ts = (t.t - 1) as usize;
        let Some(buffer) = self.buffers[ts].get(&addr) else {
            return false;
        };
        if buffer.state == DefragBufferState::Inactive {
            return false;
        }

        tracing::debug!(
            "defrag_buffer: discarding incomplete TM-SDU for ts {} addr {} at {}: {}",
            t.t,
            addr,
            t,
            reason
        );
        self.buffers[ts].remove(&addr);
        true
    }

    /// Inserts a first fragment into a fragbuffer.
    pub fn insert_first(&mut self, bitbuffer: &mut BitBuffer, t: TdmaTime, addr: TetraAddress, aie_info: Option<Todo>) {
        // Check if buffer already exists for this address/timeslot
        // Remove and discard, if so.
        let ts = (t.t - 1) as usize;
        let mut buf = if let Some(mut buf) = self.buffers[ts].remove(&addr) {
            // MS sent a new burst before the previous one completed — normal under RF loss.
            // Drop the incomplete burst silently and start fresh.
            tracing::debug!(
                "defrag_buffer: ts {} addr {} started new burst before previous completed, resetting",
                t.t,
                addr
            );
            buf.reset();
            buf
        } else {
            DefragBuffer::new()
        };

        // Initialize target buffer
        buf.state = DefragBufferState::Active;
        buf.addr = addr;
        buf.t_first = t;
        buf.t_last = t;
        buf.num_frags = 1;
        buf.aie_info = aie_info;

        // Copy the bitbuffer data from pos to end into our fragbuffer
        buf.buffer.copy_bits(bitbuffer, bitbuffer.get_len_remaining());

        tracing::debug!(
            "defrag_buffer for ts {} addr: {}, t: {}-{}, frags: {}: {}",
            t.t,
            buf.addr,
            buf.t_first,
            buf.t_last,
            buf.num_frags,
            buf.buffer.dump_bin()
        );

        self.buffers[ts].insert(addr, buf);
    }

    pub fn insert_next(&mut self, bitbuffer: &mut BitBuffer, addr: TetraAddress, t: TdmaTime) {
        let ts = (t.t - 1) as usize;
        let buf = match self.buffers[ts].get_mut(&addr) {
            Some(b) => b,
            None => {
                tracing::warn!("defrag_buffer for ts {} addr {} not found", t.t, addr);
                return;
            }
        };

        if buf.state != DefragBufferState::Active {
            tracing::warn!("defrag_buffer for ts {} addr {} not active", t.t, addr);
            return;
        }

        if buf.buffer.get_len() + bitbuffer.get_len_remaining() > DEFRAG_BUF_MAX_LEN {
            tracing::warn!("defrag_buffer for ts {} addr {} would exceed max len", t.t, addr);
            buf.reset();
            return;
        }

        buf.t_last = t;
        buf.num_frags += 1;

        // Copy the bitbuffer data from pos to end into our fragbuffer
        buf.buffer.copy_bits(bitbuffer, bitbuffer.get_len_remaining());

        tracing::debug!(
            "defrag_buffer for ts {} addr: {}, t: {}-{}, frags: {}: {}",
            t.t,
            addr,
            buf.t_first,
            buf.t_last,
            buf.num_frags,
            buf.buffer.dump_bin()
        );
    }

    /// Inserts the last fragment into a DefragBuffer, and returns the completed object
    pub fn insert_last(&mut self, bitbuffer: &mut BitBuffer, addr: TetraAddress, t: TdmaTime) -> Option<DefragBuffer> {
        // First, insert the last fragment, then reset buffer pos to start
        self.insert_next(bitbuffer, addr, t);

        // Now take the buffer out of the map
        let ts = (t.t - 1) as usize;
        let mut buf = match self.buffers[ts].remove(&addr) {
            Some(b) => b,
            None => {
                tracing::warn!("defrag_buffer for ts {} addr {} not found", t.t, addr);
                return None;
            }
        };

        if buf.state != DefragBufferState::Active {
            tracing::warn!("defrag_buffer for ts {} addr {} not active at MAC-END", t.t, addr);
            return None;
        }

        // Update state to complete and return
        buf.state = DefragBufferState::Complete;
        buf.buffer.set_raw_pos(0);
        Some(buf)
    }

    /// Retrieves a read-only reference to the AIE info associated with a DefragBuffer
    pub fn get_aie_info(&self, addr: TetraAddress, t: TdmaTime) -> Option<&Todo> {
        let ts = (t.t - 1) as usize;
        let buf = match self.buffers[ts].get(&addr) {
            Some(b) => b,
            None => {
                tracing::warn!("defrag_buffer for ts {} addr {} not found", t.t, addr);
                return None;
            }
        };
        if buf.state == DefragBufferState::Inactive {
            tracing::warn!("defrag_buffer for ts {} addr {} not active", t.t, addr);
            return None;
        };
        buf.aie_info.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_core::{address::SsiType, bitbuffer::BitBuffer, debug};

    #[test]
    fn test_3_chunks() {
        debug::setup_logging_verbose();

        let ssi = 1234;
        let mut buf1 = BitBuffer::from_bitstr("000");
        let t1 = TdmaTime::default().add_timeslots(2); // UL time 0
        let mut buf2 = BitBuffer::from_bitstr("111");
        let t2 = t1.add_timeslots(4);
        let mut buf3 = BitBuffer::from_bitstr("0011");
        let t3 = t2.add_timeslots(4);

        let mut defragger = BsDefrag::new();
        let addr = TetraAddress {
            ssi,
            ssi_type: SsiType::Issi,
        };
        defragger.insert_first(&mut buf1, t1, addr, None);
        defragger.insert_next(&mut buf2, addr, t2);
        let out = defragger.insert_last(&mut buf3, addr, t3).unwrap();
        assert_eq!(out.buffer.to_bitstr(), "0001110011");
        assert_eq!(out.buffer.get_pos(), 0);
    }

    #[test]
    fn test_complete_mac_pdu_discards_incomplete_fragment() {
        debug::setup_logging_verbose();

        let addr = TetraAddress {
            ssi: 1234,
            ssi_type: SsiType::Issi,
        };
        let t = TdmaTime::default().add_timeslots(2);
        let mut first = BitBuffer::from_bitstr("1010");
        let mut stale_end = BitBuffer::from_bitstr("1111");
        let mut defragger = BsDefrag::new();

        defragger.insert_first(&mut first, t, addr, None);
        assert!(defragger.discard_incomplete_for_addr(t, addr, "new MAC-DATA"));

        // EN 300 392-2 clause 23.4.3.1.2: after a MAC-DATA or MAC-U-BLCK
        // interrupts a fragmented TM-SDU, later MAC-END for the old burst is
        // discarded instead of completing stale user data.
        assert!(defragger.insert_last(&mut stale_end, addr, t.add_timeslots(4)).is_none());
    }

    #[test]
    fn test_discard_incomplete_requires_same_tetra_address() {
        debug::setup_logging_verbose();

        let issi_addr = TetraAddress {
            ssi: 1234,
            ssi_type: SsiType::Issi,
        };
        let gssi_addr = TetraAddress {
            ssi: 1234,
            ssi_type: SsiType::Gssi,
        };
        let t = TdmaTime::default().add_timeslots(2);
        let mut first = BitBuffer::from_bitstr("1010");
        let mut end = BitBuffer::from_bitstr("1111");
        let mut defragger = BsDefrag::new();

        defragger.insert_first(&mut first, t, issi_addr, None);
        assert!(
            !defragger.discard_incomplete_for_addr(t, gssi_addr, "same numeric SSI but different address type"),
            "defrag discard must be scoped to the full TETRA address"
        );

        let out = defragger
            .insert_last(&mut end, issi_addr, t.add_timeslots(4))
            .expect("ISSI buffer should survive a GSSI discard attempt");
        assert_eq!(out.addr, issi_addr);
        assert_eq!(out.buffer.to_bitstr(), "10101111");
    }

    #[test]
    fn test_insert_last_requires_same_tetra_address() {
        debug::setup_logging_verbose();

        let issi_addr = TetraAddress {
            ssi: 1234,
            ssi_type: SsiType::Issi,
        };
        let gssi_addr = TetraAddress {
            ssi: 1234,
            ssi_type: SsiType::Gssi,
        };
        let t = TdmaTime::default().add_timeslots(2);
        let mut first = BitBuffer::from_bitstr("1010");
        let mut wrong_end = BitBuffer::from_bitstr("0000");
        let mut right_end = BitBuffer::from_bitstr("1111");
        let mut defragger = BsDefrag::new();

        defragger.insert_first(&mut first, t, issi_addr, None);
        assert!(
            defragger.insert_last(&mut wrong_end, gssi_addr, t.add_timeslots(4)).is_none(),
            "MAC-END for a different address type must not complete the ISSI fragment buffer"
        );

        let out = defragger
            .insert_last(&mut right_end, issi_addr, t.add_timeslots(8))
            .expect("ISSI buffer should remain available for the matching MAC-END");
        assert_eq!(out.addr, issi_addr);
        assert_eq!(out.buffer.to_bitstr(), "10101111");
    }

    #[test]
    fn test_t202_discards_incomplete_fragment() {
        debug::setup_logging_verbose();

        let addr = TetraAddress {
            ssi: 1234,
            ssi_type: SsiType::Issi,
        };
        let t = TdmaTime::default().add_timeslots(2);
        let mut first = BitBuffer::from_bitstr("1010");
        let mut stale_end = BitBuffer::from_bitstr("1111");
        let mut defragger = BsDefrag::new();

        defragger.insert_first(&mut first, t, addr, None);
        defragger.age_buffers(t.add_timeslots(DEFRAG_TS_BEFORE_TIMEOUT));

        // EN 300 392-2 Annex A.1 default T.202 is 9 downlink signalling
        // frames. Once it elapses, the partially reconstructed TM-SDU is gone.
        assert!(
            defragger
                .insert_last(&mut stale_end, addr, t.add_timeslots(DEFRAG_TS_BEFORE_TIMEOUT + 4))
                .is_none()
        );
    }
}
