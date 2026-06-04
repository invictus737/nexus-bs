/// Clause 21.5.6 Basic slot granting, Capacity Allocation element
/// Bits: 4
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BasicSlotgrantCapAlloc {
    FirstSubslotGranted = 0,
    Grant1Slot = 1,
    Grant2Slots = 2,
    Grant3Slots = 3,
    Grant4Slots = 4,
    Grant5Slots = 5,
    Grant6Slots = 6,
    Grant8Slots = 7,
    Grant10Slots = 8,
    Grant13Slots = 9,
    Grant17Slots = 10,
    Grant24Slots = 11,
    Grant34Slots = 12,
    Grant51Slots = 13,
    Grant68Slots = 14,
    SecondSubslotGranted = 15,
}

impl std::convert::TryFrom<u64> for BasicSlotgrantCapAlloc {
    type Error = ();
    fn try_from(x: u64) -> Result<Self, Self::Error> {
        match x {
            0 => Ok(BasicSlotgrantCapAlloc::FirstSubslotGranted),
            1 => Ok(BasicSlotgrantCapAlloc::Grant1Slot),
            2 => Ok(BasicSlotgrantCapAlloc::Grant2Slots),
            3 => Ok(BasicSlotgrantCapAlloc::Grant3Slots),
            4 => Ok(BasicSlotgrantCapAlloc::Grant4Slots),
            5 => Ok(BasicSlotgrantCapAlloc::Grant5Slots),
            6 => Ok(BasicSlotgrantCapAlloc::Grant6Slots),
            7 => Ok(BasicSlotgrantCapAlloc::Grant8Slots),
            8 => Ok(BasicSlotgrantCapAlloc::Grant10Slots),
            9 => Ok(BasicSlotgrantCapAlloc::Grant13Slots),
            10 => Ok(BasicSlotgrantCapAlloc::Grant17Slots),
            11 => Ok(BasicSlotgrantCapAlloc::Grant24Slots),
            12 => Ok(BasicSlotgrantCapAlloc::Grant34Slots),
            13 => Ok(BasicSlotgrantCapAlloc::Grant51Slots),
            14 => Ok(BasicSlotgrantCapAlloc::Grant68Slots),
            15 => Ok(BasicSlotgrantCapAlloc::SecondSubslotGranted),
            _ => Err(()),
        }
    }
}

impl BasicSlotgrantCapAlloc {
    /// Convert this enum back into the raw integer value
    pub fn into_raw(self) -> u64 {
        match self {
            BasicSlotgrantCapAlloc::FirstSubslotGranted => 0,
            BasicSlotgrantCapAlloc::Grant1Slot => 1,
            BasicSlotgrantCapAlloc::Grant2Slots => 2,
            BasicSlotgrantCapAlloc::Grant3Slots => 3,
            BasicSlotgrantCapAlloc::Grant4Slots => 4,
            BasicSlotgrantCapAlloc::Grant5Slots => 5,
            BasicSlotgrantCapAlloc::Grant6Slots => 6,
            BasicSlotgrantCapAlloc::Grant8Slots => 7,
            BasicSlotgrantCapAlloc::Grant10Slots => 8,
            BasicSlotgrantCapAlloc::Grant13Slots => 9,
            BasicSlotgrantCapAlloc::Grant17Slots => 10,
            BasicSlotgrantCapAlloc::Grant24Slots => 11,
            BasicSlotgrantCapAlloc::Grant34Slots => 12,
            BasicSlotgrantCapAlloc::Grant51Slots => 13,
            BasicSlotgrantCapAlloc::Grant68Slots => 14,
            BasicSlotgrantCapAlloc::SecondSubslotGranted => 15,
        }
    }

    /// Pass 0 when the first subslot should be granted.
    /// Pass 99 when the second subslot should be granted.
    pub fn try_from_req_slotcount(req: usize) -> Option<Self> {
        match req {
            0 => Some(BasicSlotgrantCapAlloc::FirstSubslotGranted),
            1 => Some(BasicSlotgrantCapAlloc::Grant1Slot),
            2 => Some(BasicSlotgrantCapAlloc::Grant2Slots),
            3 => Some(BasicSlotgrantCapAlloc::Grant3Slots),
            4 => Some(BasicSlotgrantCapAlloc::Grant4Slots),
            5 => Some(BasicSlotgrantCapAlloc::Grant5Slots),
            6 => Some(BasicSlotgrantCapAlloc::Grant6Slots),
            7..=8 => Some(BasicSlotgrantCapAlloc::Grant8Slots),
            9..=10 => Some(BasicSlotgrantCapAlloc::Grant10Slots),
            11..=13 => Some(BasicSlotgrantCapAlloc::Grant13Slots),
            14..=17 => Some(BasicSlotgrantCapAlloc::Grant17Slots),
            18..=24 => Some(BasicSlotgrantCapAlloc::Grant24Slots),
            25..=34 => Some(BasicSlotgrantCapAlloc::Grant34Slots),
            35..=51 => Some(BasicSlotgrantCapAlloc::Grant51Slots),
            52..=68 => Some(BasicSlotgrantCapAlloc::Grant68Slots),
            99 => Some(BasicSlotgrantCapAlloc::SecondSubslotGranted),
            _ => None,
        }
    }

    /// Pass 0 when the first subslot should be granted.
    /// Pass 99 when the second subslot should be granted.
    ///
    /// EN 300 392-2 table 21.96 defines capacity allocation values only up to
    /// 68 full slots. Keep callers panic-free by saturating larger full-slot
    /// requests to the largest encodable basic grant; callers that need strict
    /// validation can use `try_from_req_slotcount`.
    pub fn from_req_slotcount(req: usize) -> Self {
        if let Some(cap_alloc) = Self::try_from_req_slotcount(req) {
            return cap_alloc;
        }

        tracing::warn!(
            "BasicSlotgrantCapAlloc::from_req_slotcount({}) exceeds ETSI basic grant range; capping at 68 slots",
            req
        );
        BasicSlotgrantCapAlloc::Grant68Slots
    }

    /// Returns 0 when the first subslot is granted
    /// Returns 99 when the second subslot is granted
    pub fn to_req_slotcount(&self) -> usize {
        match self {
            BasicSlotgrantCapAlloc::FirstSubslotGranted => 0,
            BasicSlotgrantCapAlloc::Grant1Slot => 1,
            BasicSlotgrantCapAlloc::Grant2Slots => 2,
            BasicSlotgrantCapAlloc::Grant3Slots => 3,
            BasicSlotgrantCapAlloc::Grant4Slots => 4,
            BasicSlotgrantCapAlloc::Grant5Slots => 5,
            BasicSlotgrantCapAlloc::Grant6Slots => 6,
            BasicSlotgrantCapAlloc::Grant8Slots => 8,
            BasicSlotgrantCapAlloc::Grant10Slots => 10,
            BasicSlotgrantCapAlloc::Grant13Slots => 13,
            BasicSlotgrantCapAlloc::Grant17Slots => 17,
            BasicSlotgrantCapAlloc::Grant24Slots => 24,
            BasicSlotgrantCapAlloc::Grant34Slots => 34,
            BasicSlotgrantCapAlloc::Grant51Slots => 51,
            BasicSlotgrantCapAlloc::Grant68Slots => 68,
            BasicSlotgrantCapAlloc::SecondSubslotGranted => 99,
        }
    }
}

impl From<BasicSlotgrantCapAlloc> for u64 {
    fn from(e: BasicSlotgrantCapAlloc) -> Self {
        e.into_raw()
    }
}

impl core::fmt::Display for BasicSlotgrantCapAlloc {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BasicSlotgrantCapAlloc::FirstSubslotGranted => write!(f, "FirstSubslotGranted"),
            BasicSlotgrantCapAlloc::Grant1Slot => write!(f, "Grant1Slot"),
            BasicSlotgrantCapAlloc::Grant2Slots => write!(f, "Grant2Slots"),
            BasicSlotgrantCapAlloc::Grant3Slots => write!(f, "Grant3Slots"),
            BasicSlotgrantCapAlloc::Grant4Slots => write!(f, "Grant4Slots"),
            BasicSlotgrantCapAlloc::Grant5Slots => write!(f, "Grant5Slots"),
            BasicSlotgrantCapAlloc::Grant6Slots => write!(f, "Grant6Slots"),
            BasicSlotgrantCapAlloc::Grant8Slots => write!(f, "Grant8Slots"),
            BasicSlotgrantCapAlloc::Grant10Slots => write!(f, "Grant10Slots"),
            BasicSlotgrantCapAlloc::Grant13Slots => write!(f, "Grant13Slots"),
            BasicSlotgrantCapAlloc::Grant17Slots => write!(f, "Grant17Slots"),
            BasicSlotgrantCapAlloc::Grant24Slots => write!(f, "Grant24Slots"),
            BasicSlotgrantCapAlloc::Grant34Slots => write!(f, "Grant34Slots"),
            BasicSlotgrantCapAlloc::Grant51Slots => write!(f, "Grant51Slots"),
            BasicSlotgrantCapAlloc::Grant68Slots => write!(f, "Grant68Slots"),
            BasicSlotgrantCapAlloc::SecondSubslotGranted => write!(f, "SecondSubslotGranted"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subslot_capacity_allocations_have_documented_sentinel_counts() {
        // EN 300 392-2 table 21.96 defines raw values 0 and 15 as first and
        // second subslot allocations, not full-slot counts. Keep the existing
        // sentinel convention panic-free for callers that need to inspect a
        // decoded basic slot grant before branching on the subslot variants.
        assert_eq!(BasicSlotgrantCapAlloc::FirstSubslotGranted.to_req_slotcount(), 0);
        assert_eq!(BasicSlotgrantCapAlloc::SecondSubslotGranted.to_req_slotcount(), 99);
    }

    #[test]
    fn capacity_allocation_slot_count_roundtrips_known_values() {
        for (requested, expected) in [
            (0, BasicSlotgrantCapAlloc::FirstSubslotGranted),
            (1, BasicSlotgrantCapAlloc::Grant1Slot),
            (2, BasicSlotgrantCapAlloc::Grant2Slots),
            (3, BasicSlotgrantCapAlloc::Grant3Slots),
            (4, BasicSlotgrantCapAlloc::Grant4Slots),
            (5, BasicSlotgrantCapAlloc::Grant5Slots),
            (6, BasicSlotgrantCapAlloc::Grant6Slots),
            (8, BasicSlotgrantCapAlloc::Grant8Slots),
            (10, BasicSlotgrantCapAlloc::Grant10Slots),
            (13, BasicSlotgrantCapAlloc::Grant13Slots),
            (17, BasicSlotgrantCapAlloc::Grant17Slots),
            (24, BasicSlotgrantCapAlloc::Grant24Slots),
            (34, BasicSlotgrantCapAlloc::Grant34Slots),
            (51, BasicSlotgrantCapAlloc::Grant51Slots),
            (68, BasicSlotgrantCapAlloc::Grant68Slots),
            (99, BasicSlotgrantCapAlloc::SecondSubslotGranted),
        ] {
            assert_eq!(BasicSlotgrantCapAlloc::try_from_req_slotcount(requested), Some(expected));
            assert_eq!(BasicSlotgrantCapAlloc::from_req_slotcount(requested), expected);
            assert_eq!(expected.to_req_slotcount(), requested);
        }
    }

    #[test]
    fn overrange_full_slot_requests_are_panic_free_and_saturate() {
        // EN 300 392-2 table 21.96 has no basic capacity allocation above
        // 68 full slots. The scheduler may still see larger local counts after
        // capping an over-68 reservation requirement; never panic in that path.
        for requested in [69, 70, 98, 100, usize::MAX] {
            assert_eq!(BasicSlotgrantCapAlloc::try_from_req_slotcount(requested), None);
            assert_eq!(
                BasicSlotgrantCapAlloc::from_req_slotcount(requested),
                BasicSlotgrantCapAlloc::Grant68Slots
            );
        }

        assert_eq!(
            BasicSlotgrantCapAlloc::from_req_slotcount(99),
            BasicSlotgrantCapAlloc::SecondSubslotGranted
        );
    }
}
