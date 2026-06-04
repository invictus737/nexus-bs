use core::fmt;

use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use crate::mle::enums::mle_pdu_type_ul::MlePduTypeUl;
use crate::mle::pdus::raw_sdu;

/// Representation of the U-RESTORE PDU (Clause 18.4.1.4.7).
/// The message shall be sent by the MS-MLE, when restoration of the C-Plane towards a new cell is in progress.
/// Response expected: D-RESTORE-ACK/D-RESTORE-FAIL
/// Response to: -

// note 1: The element is present in the PDU if its value on the new cell is different from that on the old cell.
// note 2: When included, this element gives the value for the old cell.
// note 3: This PDU shall carry a CMCE U-CALL RESTORE PDU which shall be used to restore a call after cell reselection. There shall be no P-bit in the PDU coding preceding the "SDU" information element.
#[derive(Debug)]
pub struct URestore {
    /// Type2, 10 bits, See notes 1 and 2,
    pub mcc: Option<u64>,
    /// Type2, 14 bits, See notes 1 and 2,
    pub mnc: Option<u64>,
    /// Type2, 14 bits, See notes 1 and 2,
    pub la: Option<u64>,
    /// Conditional This PDU shall carry a CMCE U-CALL RESTORE PDU which shall be used to restore a call after cell reselection. The SDU is coded according to the CMCE protocol. There shall be no P-bit in the PDU coding preceding the "SDU" information element.,
    pub sdu: Option<u64>,
}

impl URestore {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeUl::URestore)?;

        // obit designates presence of any further type2, type3 or type4 fields
        let obit = delimiters::read_obit(buffer)?;

        // Type2
        let mcc = typed::parse_type2_generic(obit, buffer, 10, "mcc")?;
        // Type2
        let mnc = typed::parse_type2_generic(obit, buffer, 14, "mnc")?;
        // Type2
        let la = typed::parse_type2_generic(obit, buffer, 14, "la")?;
        // EN 300 392-2 table 18.13: the CMCE U-CALL RESTORE SDU has no
        // preceding P-bit and follows the optional old-cell elements.
        let sdu = raw_sdu::read_remaining_u64(buffer, "u_restore_sdu")?;

        Ok(URestore { mcc, mnc, la, sdu })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        // PDU Type
        buffer.write_bits(MlePduTypeUl::URestore.into_raw(), 3);

        raw_sdu::reject_write_if_present(self.sdu, "u_restore_sdu")?;

        // Check if any optional field present and place o-bit
        let obit = self.mcc.is_some() || self.mnc.is_some() || self.la.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2
        typed::write_type2_generic(obit, buffer, self.mcc, 10);

        // Type2
        typed::write_type2_generic(obit, buffer, self.mnc, 14);

        // Type2
        typed::write_type2_generic(obit, buffer, self.la, 14);

        Ok(())
    }
}

impl fmt::Display for URestore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "URestore {{ mcc: {:?} mnc: {:?} la: {:?} sdu: {:?} }}",
            self.mcc, self.mnc, self.la, self.sdu,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u_restore_round_trips_optional_old_cell_fields_without_sdu() {
        let pdu = URestore {
            mcc: Some(204),
            mnc: Some(1337),
            la: Some(2),
            sdu: None,
        };
        let mut buf = BitBuffer::new_autoexpand(48);

        pdu.to_bitbuf(&mut buf).expect("serialize U-RESTORE without SDU");
        buf.seek(0);
        let parsed = URestore::from_bitbuf(&mut buf).expect("parse U-RESTORE without SDU");

        assert_eq!(parsed.mcc, Some(204));
        assert_eq!(parsed.mnc, Some(1337));
        assert_eq!(parsed.la, Some(2));
        assert_eq!(parsed.sdu, None);
        assert_eq!(buf.get_len_remaining(), 0);
    }

    #[test]
    fn u_restore_parses_cmce_sdu_after_absent_optional_fields() {
        let mut buf = BitBuffer::new_autoexpand(16);
        buf.write_bits(MlePduTypeUl::URestore.into_raw(), 3);
        buf.write_bits(0, 1);
        buf.write_bits(0b11011, 5);
        buf.seek(0);

        let parsed = URestore::from_bitbuf(&mut buf).expect("parse U-RESTORE with SDU");

        assert_eq!(parsed.mcc, None);
        assert_eq!(parsed.mnc, None);
        assert_eq!(parsed.la, None);
        assert_eq!(parsed.sdu, Some(0b11011));
        assert_eq!(buf.get_len_remaining(), 0);
    }

    #[test]
    fn u_restore_rejects_serializing_raw_sdu_until_length_is_modelled() {
        let pdu = URestore {
            mcc: None,
            mnc: None,
            la: None,
            sdu: Some(0b11011),
        };
        let mut buf = BitBuffer::new_autoexpand(16);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::NotImplemented {
                field: Some("u_restore_sdu"),
            })
        );
        assert_eq!(buf.get_len(), 3);
    }
}
