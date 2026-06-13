// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use crate::mle::enums::mle_pdu_type_dl::MlePduTypeDl;
use crate::mle::pdus::raw_sdu;

/// Representation of the D-NEW-CELL PDU (Clause 18.4.1.4.2).
/// Upon receipt from the SwMI the message shall inform the MS-MLE that it can select a new cell as previously indicated in the U-PREPARE or U-PREPARE-DA PDU.
/// Response expected: -
/// Response to: U-PREPARE/U-PREPARE-DA

// note 1: The SDU may carry an MM registration PDU which is used to forward register to a new cell during announced type 1 cell reselection or a D-OTAR CCK PROVIDE PDU which is used to identify the current CCK; it may also provide the future CCK for the LA which the MS has indicated in the U-OTAR CCK DEMAND PDU and whether the CCK provided is in use in other LAs or is used throughout the SwMI. The SDU is coded according to the MM protocol description. There shall be no P-bit in the PDU coding preceding the SDU information element.
#[derive(Debug)]
pub struct DNewCell {
    /// Type1, 2 bits, Channel command valid
    pub channel_command_valid: u8,
    /// Conditional SDU
    pub sdu: Option<u64>,
}

impl DNewCell {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeDl::DNewCell)?;

        // Type1
        let channel_command_valid = buffer.read_field(2, "channel_command_valid")? as u8;
        // EN 300 392-2 table 18.6: optional MM/OTAR SDU has no preceding P-bit
        // and consumes the remaining payload.
        let sdu = raw_sdu::read_remaining_u64(buffer, "d_new_cell_sdu")?;

        Ok(DNewCell {
            channel_command_valid,
            sdu,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        // PDU Type
        buffer.write_bits(MlePduTypeDl::DNewCell.into_raw(), 3);
        // Type1
        buffer.write_bits(self.channel_command_valid as u64, 2);
        raw_sdu::reject_write_if_present(self.sdu, "d_new_cell_sdu")?;
        Ok(())
    }
}

impl fmt::Display for DNewCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DNewCell {{ channel_command_valid: {:?} sdu: {:?} }}",
            self.channel_command_valid, self.sdu,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d_new_cell_parses_remaining_payload_as_sdu() {
        let mut buf = BitBuffer::new_autoexpand(16);
        buf.write_bits(MlePduTypeDl::DNewCell.into_raw(), 3);
        buf.write_bits(2, 2);
        buf.write_bits(0b10101, 5);
        buf.seek(0);

        let parsed = DNewCell::from_bitbuf(&mut buf).expect("parse D-NEW-CELL");

        assert_eq!(parsed.channel_command_valid, 2);
        assert_eq!(parsed.sdu, Some(0b10101));
        assert_eq!(buf.get_len_remaining(), 0);
    }

    #[test]
    fn d_new_cell_rejects_serializing_raw_sdu_until_length_is_modelled() {
        let pdu = DNewCell {
            channel_command_valid: 2,
            sdu: Some(0b10101),
        };
        let mut buf = BitBuffer::new_autoexpand(16);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::NotImplemented {
                field: Some("d_new_cell_sdu"),
            })
        );
    }
}
