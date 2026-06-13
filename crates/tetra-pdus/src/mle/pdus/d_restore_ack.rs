// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use crate::mle::enums::mle_pdu_type_dl::MlePduTypeDl;
use crate::mle::pdus::raw_sdu;

/// Representation of the D-RESTORE-ACK PDU (Clause 18.4.1.4.4).
/// Upon receipt from the SwMI, the message shall indicate to the MS-MLE an acknowledgement of the C-Plane restoration on the new selected cell.
/// Response expected: -
/// Response to: U-RESTORE

// note 1: This PDU shall carry a CMCE D-CALL RESTORE PDU which can be used to restore a call after cell reselection. The SDU is coded according to the CMCE protocol description. There shall be no P-bit in the PDU coding preceding the SDU information element.
#[derive(Debug)]
pub struct DRestoreAck {
    /// Conditional See note,
    pub sdu: Option<u64>,
}

impl DRestoreAck {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeDl::DRestoreAck)?;

        // EN 300 392-2 table 18.8: the CMCE D-CALL RESTORE SDU has no
        // preceding P-bit and consumes the remaining payload.
        let sdu = raw_sdu::read_remaining_u64(buffer, "d_restore_ack_sdu")?;

        Ok(DRestoreAck { sdu })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        // PDU Type
        buffer.write_bits(MlePduTypeDl::DRestoreAck.into_raw(), 3);
        raw_sdu::reject_write_if_present(self.sdu, "d_restore_ack_sdu")?;
        Ok(())
    }
}

impl fmt::Display for DRestoreAck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DRestoreAck {{ sdu: {:?} }}", self.sdu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d_restore_ack_parses_remaining_payload_as_sdu() {
        let mut buf = BitBuffer::new_autoexpand(16);
        buf.write_bits(MlePduTypeDl::DRestoreAck.into_raw(), 3);
        buf.write_bits(0b10101, 5);
        buf.seek(0);

        let parsed = DRestoreAck::from_bitbuf(&mut buf).expect("parse D-RESTORE-ACK");

        assert_eq!(parsed.sdu, Some(0b10101));
        assert_eq!(buf.get_len_remaining(), 0);
    }

    #[test]
    fn d_restore_ack_rejects_serializing_raw_sdu_until_length_is_modelled() {
        let pdu = DRestoreAck { sdu: Some(0b10101) };
        let mut buf = BitBuffer::new_autoexpand(16);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::NotImplemented {
                field: Some("d_restore_ack_sdu"),
            })
        );
    }
}
