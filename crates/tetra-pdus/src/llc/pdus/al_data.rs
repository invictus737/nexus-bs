// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original LLC Advanced Link PDU support.

use core::fmt;

use tetra_core::pdu_parse_error::*;
use tetra_core::{BitBuffer, expect_value, let_field};

/// EN 300 392-2 clauses 21.2.3.2 and 21.2.3.3 original AL-DATA/AL-FINAL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlData {
    pub final_segment: bool,
    pub acknowledgement_requested: bool,
    pub ns: u8,
    pub ss: u8,
}

impl AlData {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let_field!(buf, llc_pdu_type, 4);
        expect_value!(llc_pdu_type, 9)?;
        let_field!(buf, final_segment, 1);
        let_field!(buf, acknowledgement_requested, 1);
        let_field!(buf, ns, 3);
        let_field!(buf, ss, 8);
        Ok(Self {
            final_segment: final_segment != 0,
            acknowledgement_requested: acknowledgement_requested != 0,
            ns: ns as u8,
            ss: ss as u8,
        })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        buf.write_bits(9, 4);
        buf.write_bits(self.final_segment as u64, 1);
        buf.write_bits(self.acknowledgement_requested as u64, 1);
        buf.write_bits(self.ns as u64, 3);
        buf.write_bits(self.ss as u64, 8);
    }
}

impl fmt::Display for AlData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match (self.final_segment, self.acknowledgement_requested) {
            (false, false) => "al_data",
            (false, true) => "al_data_ar",
            (true, false) => "al_final",
            (true, true) => "al_final_ar",
        };
        write!(f, "{} {{ ns: {}, ss: {} }}", name, self.ns, self.ss)
    }
}
