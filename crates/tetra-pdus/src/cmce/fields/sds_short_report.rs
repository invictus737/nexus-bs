// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::{PduParseErr, expect_value};

use crate::cmce::enums::short_report_type::ShortReportType;

/// Clause 29.4.2.3 SDS-SHORT REPORT
/// This PDU shall be used to report on the progress of previously received SDS data
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SdsShortReport {
    /// 2 bits
    short_report_type: ShortReportType,
    /// 8 bits. The same value as in the corresponding request PDU
    message_reference: u8,
}

impl SdsShortReport {
    pub fn short_report_type(&self) -> ShortReportType {
        self.short_report_type
    }

    pub fn message_reference(&self) -> u8 {
        self.message_reference
    }

    // No from_bitbuf, to_bitbuf functions, as we'll parse this in a bit of a different way originating from an enum field in the U-STATUS PDU pre-coded status field
    pub fn from_u16(val: u16) -> Result<Self, PduParseErr> {
        // EN 300 392-2 table 14.72 reserves the 0b011111xxxxxx_xxxxxxxx
        // pre-coded status range for SDS-TL short reporting. The low byte is
        // the original message reference and bits 9..8 are the short report
        // type from table 29.23.
        let pdu_type = val >> 10;
        expect_value!(pdu_type, 0b011111)?;
        let raw = ((val >> 8) & 0x3) as u64;
        // raw is masked to 2 bits, ShortReportType covers all 4 values today,
        // but propagate as InvalidValue to stay panic-free if the enum changes.
        let short_report_type = ShortReportType::try_from(raw).map_err(|_| PduParseErr::InvalidValue {
            field: "short_report_type",
            value: raw,
        })?;
        let message_reference = (val & 0xFF) as u8;

        Ok(SdsShortReport {
            short_report_type,
            message_reference,
        })
    }

    pub fn to_u16(&self) -> u16 {
        assert!(self.short_report_type.into_raw() <= 0b11, "short_report_type must be 2 bits");
        (0b011111 << 10) | ((self.short_report_type.into_raw() as u16) << 8) | (self.message_reference as u16 & 0xFF)
    }
}

impl fmt::Display for SdsShortReport {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "SdsShortReport {{ short_report_type: {:?}, message_reference: {:?} }}",
            self.short_report_type, self.message_reference,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_u16_roundtrip() {
        // Synthetic test, not real world data
        let sds = SdsShortReport {
            short_report_type: ShortReportType::MessageReceived,
            message_reference: 0b00000001,
        };

        let converted = sds.to_u16();
        assert_eq!(converted, 0b0111111000000001);

        let parsed = SdsShortReport::from_u16(converted).unwrap();
        assert_eq!(parsed.short_report_type, sds.short_report_type);
        assert_eq!(parsed.message_reference, sds.message_reference);
    }
}
