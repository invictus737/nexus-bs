// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeslotOwner {
    Brew,
    Cmce,
    PacketData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeslotAllocErr {
    InvalidTimeslot(u8),
    InUse {
        ts: u8,
        owner: TimeslotOwner,
    },
    NotAllocated {
        ts: u8,
    },
    OwnerMismatch {
        ts: u8,
        owner: TimeslotOwner,
        actual: TimeslotOwner,
    },
}

#[derive(Debug, Clone)]
pub struct TimeslotAllocator {
    // Index 0 = TS2, 1 = TS3, 2 = TS4
    owners: [Option<TimeslotOwner>; 3],
}

impl Default for TimeslotAllocator {
    fn default() -> Self {
        Self {
            owners: [None, None, None],
        }
    }
}

impl TimeslotAllocator {
    fn idx(ts: u8) -> Result<usize, TimeslotAllocErr> {
        if (2..=4).contains(&ts) {
            Ok((ts - 2) as usize)
        } else {
            Err(TimeslotAllocErr::InvalidTimeslot(ts))
        }
    }

    pub fn allocate_any(&mut self, owner: TimeslotOwner) -> Option<u8> {
        for (i, slot) in self.owners.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(owner);
                return Some(i as u8 + 2);
            }
        }
        None
    }

    pub fn can_allocate_preempting(&self, needed: usize, preemptible_owner: TimeslotOwner) -> bool {
        needed <= self.owners.len()
            && self
                .owners
                .iter()
                .filter(|owner| owner.is_none() || **owner == Some(preemptible_owner))
                .count()
                >= needed
    }

    pub fn allocate_many_preempting(&mut self, owner: TimeslotOwner, needed: usize, preemptible_owner: TimeslotOwner) -> Option<Vec<u8>> {
        if needed == 0 {
            return Some(Vec::new());
        }
        if !self.can_allocate_preempting(needed, preemptible_owner) {
            return None;
        }

        let mut selected = Vec::with_capacity(needed);
        for (idx, slot_owner) in self.owners.iter().enumerate() {
            if slot_owner.is_none() {
                selected.push(idx);
                if selected.len() == needed {
                    break;
                }
            }
        }
        if selected.len() < needed {
            for (idx, slot_owner) in self.owners.iter().enumerate() {
                if *slot_owner == Some(preemptible_owner) {
                    selected.push(idx);
                    if selected.len() == needed {
                        break;
                    }
                }
            }
        }

        for idx in &selected {
            self.owners[*idx] = Some(owner);
        }
        Some(selected.into_iter().map(|idx| idx as u8 + 2).collect())
    }

    pub fn allocate_any_preempting(&mut self, owner: TimeslotOwner, preemptible_owner: TimeslotOwner) -> Option<u8> {
        self.allocate_many_preempting(owner, 1, preemptible_owner)
            .and_then(|mut slots| slots.pop())
    }

    pub fn reserve(&mut self, owner: TimeslotOwner, ts: u8) -> Result<(), TimeslotAllocErr> {
        let idx = Self::idx(ts)?;
        match self.owners[idx] {
            None => {
                self.owners[idx] = Some(owner);
                Ok(())
            }
            Some(existing) => Err(TimeslotAllocErr::InUse { ts, owner: existing }),
        }
    }

    pub fn release(&mut self, owner: TimeslotOwner, ts: u8) -> Result<(), TimeslotAllocErr> {
        let idx = Self::idx(ts)?;
        match self.owners[idx] {
            None => Err(TimeslotAllocErr::NotAllocated { ts }),
            Some(existing) if existing != owner => Err(TimeslotAllocErr::OwnerMismatch {
                ts,
                owner,
                actual: existing,
            }),
            Some(_) => {
                self.owners[idx] = None;
                Ok(())
            }
        }
    }

    pub fn owner(&self, ts: u8) -> Option<TimeslotOwner> {
        Self::idx(ts).ok().and_then(|idx| self.owners[idx])
    }

    pub fn is_free(&self, ts: u8) -> bool {
        self.owner(ts).is_none()
    }

    pub fn release_all(&mut self, owner: TimeslotOwner) {
        for slot in &mut self.owners {
            if *slot == Some(owner) {
                *slot = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_prefers_free_slot_before_preempting_packet_data() {
        let mut alloc = TimeslotAllocator::default();
        alloc.reserve(TimeslotOwner::PacketData, 2).expect("TS2 data reserve");

        let ts = alloc
            .allocate_any_preempting(TimeslotOwner::Cmce, TimeslotOwner::PacketData)
            .expect("voice should allocate a free slot");

        assert_eq!(ts, 3);
        assert_eq!(alloc.owner(2), Some(TimeslotOwner::PacketData));
        assert_eq!(alloc.owner(3), Some(TimeslotOwner::Cmce));
        assert_eq!(alloc.owner(4), None);
    }

    #[test]
    fn one_slot_voice_reclaims_only_one_packet_data_slot_on_shortage() {
        let mut alloc = TimeslotAllocator::default();
        for ts in 2..=4 {
            alloc.reserve(TimeslotOwner::PacketData, ts).expect("data reserve");
        }

        let ts = alloc
            .allocate_any_preempting(TimeslotOwner::Cmce, TimeslotOwner::PacketData)
            .expect("voice should reclaim one PDCH slot");

        assert_eq!(ts, 2);
        assert_eq!(alloc.owner(2), Some(TimeslotOwner::Cmce));
        assert_eq!(alloc.owner(3), Some(TimeslotOwner::PacketData));
        assert_eq!(alloc.owner(4), Some(TimeslotOwner::PacketData));
    }

    #[test]
    fn local_duplex_voice_reclaims_two_packet_data_slots_when_required() {
        let mut alloc = TimeslotAllocator::default();
        for ts in 2..=4 {
            alloc.reserve(TimeslotOwner::PacketData, ts).expect("data reserve");
        }

        let slots = alloc
            .allocate_many_preempting(TimeslotOwner::Cmce, 2, TimeslotOwner::PacketData)
            .expect("local duplex should reclaim two slots");

        assert_eq!(slots, vec![2, 3]);
        assert_eq!(alloc.owner(2), Some(TimeslotOwner::Cmce));
        assert_eq!(alloc.owner(3), Some(TimeslotOwner::Cmce));
        assert_eq!(alloc.owner(4), Some(TimeslotOwner::PacketData));
    }

    #[test]
    fn preempting_allocation_fails_without_mutation_when_capacity_is_insufficient() {
        let mut alloc = TimeslotAllocator::default();
        alloc.reserve(TimeslotOwner::Cmce, 2).expect("voice reserve");
        alloc.reserve(TimeslotOwner::Brew, 3).expect("brew reserve");
        alloc.reserve(TimeslotOwner::PacketData, 4).expect("data reserve");

        assert!(
            alloc
                .allocate_many_preempting(TimeslotOwner::Cmce, 2, TimeslotOwner::PacketData)
                .is_none()
        );
        assert_eq!(alloc.owner(2), Some(TimeslotOwner::Cmce));
        assert_eq!(alloc.owner(3), Some(TimeslotOwner::Brew));
        assert_eq!(alloc.owner(4), Some(TimeslotOwner::PacketData));
    }
}
