// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use crate::cmce::fields::sds_short_report::SdsShortReport;

/// Clause 14.8.34 Pre-coded status
/// The pre-coded status information element shall define general purpose status messages known to all TETRA systems as
/// defined in table 14.72 and shall provide support for the SDS-TL "short reporting" protocol.
/// Bits: 2
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum PreCodedStatus {
    Emergency,
    Reserved(u16),
    SdsTl(SdsShortReport),
    NetworkUserSpecific(u16),
}

impl From<u16> for PreCodedStatus {
    fn from(x: u16) -> Self {
        // ETSI EN 300 392-2 Table 14.72:
        //   0           = Emergency
        //   1..=31743   = Reserved
        //   31744..=32767 = SDS-TL short report (pdu_type bits 15..10 == 0b011111)
        //   32768..=65535 = Network/User Specific
        //
        // SDS-TL parsing can fail (expect_value on pdu_type bits, plus future
        // additions to ShortReportType), so fall back to Reserved(x) on Err
        // rather than panic on an unwrap. Wire traffic is never trusted input.
        match x {
            0 => PreCodedStatus::Emergency,
            1..=31743 => PreCodedStatus::Reserved(x),
            31744..=32767 => match SdsShortReport::from_u16(x) {
                Ok(report) => PreCodedStatus::SdsTl(report),
                Err(_) => PreCodedStatus::Reserved(x),
            },
            32768..=65535 => PreCodedStatus::NetworkUserSpecific(x),
        }
    }
}

impl PreCodedStatus {
    /// Convert this enum back into the raw integer value
    pub fn into_raw(self) -> u16 {
        match self {
            PreCodedStatus::Emergency => 0,
            PreCodedStatus::Reserved(x) => x,
            PreCodedStatus::SdsTl(x) => x.to_u16(),
            PreCodedStatus::NetworkUserSpecific(x) => x,
        }
    }
}

impl From<PreCodedStatus> for u16 {
    fn from(e: PreCodedStatus) -> Self {
        e.into_raw()
    }
}

impl core::fmt::Display for PreCodedStatus {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PreCodedStatus::Emergency => write!(f, "Emergency"),
            PreCodedStatus::Reserved(x) => write!(f, "Reserved({})", x),
            PreCodedStatus::SdsTl(x) => write!(f, "SdsTl({})", x),
            PreCodedStatus::NetworkUserSpecific(x) => write!(f, "NetworkUserSpecific({})", x),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_coded_status_all_ones_is_network_user_specific() {
        // EN 300 392-2 table 14.72 defines the 16-bit range
        // 32768..=65535 as TETRA network and user specific status values.
        let status = PreCodedStatus::from(0xFFFF);

        assert_eq!(status, PreCodedStatus::NetworkUserSpecific(0xFFFF));
        assert_eq!(status.into_raw(), 0xFFFF);
    }
}
