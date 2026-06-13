// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use crate::mle::enums::mle_pdu_type_dl::MlePduTypeDl;

/// Representation of the D-RESTORE-FAIL PDU (Clause 18.4.1.4.5).
/// Upon receipt from the SwMI, the message shall indicate to the MS-MLE a failure in the restoration of the C-Plane on the new selected cell.
/// Response expected: -
/// Response to: U-RESTORE

#[derive(Debug)]
pub struct DRestoreFail {
    /// Type1, 2 bits, Fail cause
    pub fail_cause: u8,
}

impl DRestoreFail {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 clause 18.4.1.4.5/table 18.9 and clause 18.5.7/table
        // 18.49 encode Fail cause as a 2-bit Type-1 information element.
        if self.fail_cause > 0x03 {
            return Err(PduParseErr::InvalidValue {
                field: "fail_cause",
                value: self.fail_cause as u64,
            });
        }
        Ok(())
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeDl::DRestoreFail)?;

        // Type1
        let fail_cause = buffer.read_field(2, "fail_cause")? as u8;

        // Table E.24 fixes the optional-elements-present bit to zero.
        let obit = delimiters::read_obit(buffer)?;
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        let pdu = DRestoreFail { fail_cause };
        pdu.validate()?;
        Ok(pdu)
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // PDU Type
        buffer.write_bits(MlePduTypeDl::DRestoreFail.into_raw(), 3);
        // Type1
        buffer.write_bits(self.fail_cause as u64, 2);
        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for DRestoreFail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DRestoreFail {{ fail_cause: {:?} }}", self.fail_cause)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d_restore_fail_roundtrips_with_o_bit_zero() {
        let pdu = DRestoreFail { fail_cause: 3 };
        let mut buf = BitBuffer::new_autoexpand(8);

        pdu.to_bitbuf(&mut buf).expect("serialize D-RESTORE-FAIL");
        assert_eq!(buf.get_len(), 6);
        buf.seek(0);
        let parsed = DRestoreFail::from_bitbuf(&mut buf).expect("parse D-RESTORE-FAIL");

        assert_eq!(parsed.fail_cause, 3);
        assert_eq!(buf.get_len_remaining(), 0);
    }

    #[test]
    fn d_restore_fail_rejects_optional_elements_present_bit() {
        let mut buf = BitBuffer::new_autoexpand(8);
        buf.write_bits(MlePduTypeDl::DRestoreFail.into_raw(), 3);
        buf.write_bits(3, 2);
        delimiters::write_obit(&mut buf, 1);
        delimiters::write_mbit(&mut buf, 0);
        buf.seek(0);

        assert_eq!(
            DRestoreFail::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidTrailingMbitValue
        );
    }

    #[test]
    fn d_restore_fail_rejects_overwide_fail_cause() {
        let pdu = DRestoreFail { fail_cause: 4 };
        let mut buf = BitBuffer::new_autoexpand(8);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "fail_cause",
                value: 4,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }
}
