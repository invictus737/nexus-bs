// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::BitBuffer;
use tetra_core::pdu_parse_error::PduParseErr;

/// Clause 21.4.3.2 MAC-FRAG (downlink)
#[derive(Debug, Clone)]
pub struct MacFragDl {
    // 1
    pub fill_bits: bool,
}

impl MacFragDl {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        // EN 300 392-2 clause 21.4.3.2: MAC-FRAG downlink has type 01,
        // subtype 0. Reject corrupt bits instead of panicking the worker.
        let mac_pdu_type = buf.read_field(2, "mac_pdu_type")?;
        if mac_pdu_type != 1 {
            return Err(PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: mac_pdu_type,
            });
        }
        let pdu_subtype = buf.read_field(1, "pdu_subtype")?;
        if pdu_subtype != 0 {
            return Err(PduParseErr::InvalidValue {
                field: "pdu_subtype",
                value: pdu_subtype,
            });
        }
        let fill_bits = buf.read_field(1, "fill_bits")? != 0;

        Ok(MacFragDl { fill_bits })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        // write required constant mac_pdu_type
        buf.write_bits(1, 2);
        // write required constant pdu_subtype
        buf.write_bits(0, 1);
        buf.write_bits(self.fill_bits as u8 as u64, 1);
    }
}

impl fmt::Display for MacFragDl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MacFragDl {{ fill_bits: {} }}", self.fill_bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_rejects_non_mac_frag_type_without_panic() {
        let mut buffer = BitBuffer::from_bitstr("00");

        assert_eq!(
            MacFragDl::from_bitbuf(&mut buffer).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: 0b00
            }
        );
    }

    #[test]
    fn parser_rejects_non_mac_frag_subtype_without_panic() {
        let mut buffer = BitBuffer::from_bitstr("011");

        assert_eq!(
            MacFragDl::from_bitbuf(&mut buffer).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "pdu_subtype",
                value: 0b1
            }
        );
    }
}
