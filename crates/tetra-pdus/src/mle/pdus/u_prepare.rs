use core::fmt;

use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use crate::mle::enums::mle_pdu_type_ul::MlePduTypeUl;
use crate::mle::pdus::raw_sdu;

/// Representation of the U-PREPARE PDU (Clause 18.4.1.4.6).
/// The message shall be sent on the serving cell to the SwMI by the MS-MLE, when preparation of cell reselection to a neighbour cell is in progress.
/// Response expected: D-NEW-CELL / D-NWRK-BROADCAST / D-PREPARE-FAIL
/// Response to: -

// note 1: The SDU may carry an MM registration PDU which is used to forward register to a new CA cell during announced type 1 cell reselection or a U-OTAR CCK DEMAND PDU which is used to request the Common Cipher Key (CCK) of the new cell. The SDU is coded according to the MM protocol description. There shall be no P-bit in the PDU coding preceding the SDU information element.
#[derive(Debug)]
pub struct UPrepare {
    /// Type2, 5 bits, Cell identifier CA
    pub cell_identifier_ca: Option<u64>,
    /// Conditional See note,
    pub sdu: Option<u64>,
}

impl UPrepare {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeUl::UPrepare)?;

        // obit designates presence of any further type2, type3 or type4 fields
        let obit = delimiters::read_obit(buffer)?;

        // Type2
        let cell_identifier_ca = typed::parse_type2_generic(obit, buffer, 5, "cell_identifier_ca")?;

        // EN 300 392-2 table 18.11: the SDU has no preceding P-bit and is the
        // remaining MM-coded payload, when present.
        let sdu = raw_sdu::read_remaining_u64(buffer, "u_prepare_sdu")?;

        Ok(UPrepare { cell_identifier_ca, sdu })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        // PDU Type
        buffer.write_bits(MlePduTypeUl::UPrepare.into_raw(), 3);

        // Check if any optional field present and place o-bit
        raw_sdu::reject_write_if_present(self.sdu, "u_prepare_sdu")?;

        let obit = self.cell_identifier_ca.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2
        typed::write_type2_generic(obit, buffer, self.cell_identifier_ca, 5);

        Ok(())
    }
}

impl fmt::Display for UPrepare {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "UPrepare {{ cell_identifier_ca: {:?} sdu: {:?} }}",
            self.cell_identifier_ca, self.sdu,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u_prepare_round_trips_cell_identifier_without_sdu() {
        let pdu = UPrepare {
            cell_identifier_ca: Some(17),
            sdu: None,
        };
        let mut buf = BitBuffer::new_autoexpand(16);

        pdu.to_bitbuf(&mut buf).expect("serialize U-PREPARE without SDU");
        buf.seek(0);
        let parsed = UPrepare::from_bitbuf(&mut buf).expect("parse U-PREPARE without SDU");

        assert_eq!(parsed.cell_identifier_ca, Some(17));
        assert_eq!(parsed.sdu, None);
        assert_eq!(buf.get_len_remaining(), 0);
    }

    #[test]
    fn u_prepare_parses_no_pbit_sdu_as_remaining_payload() {
        let mut buf = BitBuffer::new_autoexpand(16);
        buf.write_bits(MlePduTypeUl::UPrepare.into_raw(), 3);
        buf.write_bits(1, 1);
        buf.write_bits(1, 1);
        buf.write_bits(3, 5);
        buf.write_bits(0b1010, 4);
        buf.seek(0);

        let parsed = UPrepare::from_bitbuf(&mut buf).expect("parse U-PREPARE with SDU");

        assert_eq!(parsed.cell_identifier_ca, Some(3));
        assert_eq!(parsed.sdu, Some(0b1010));
        assert_eq!(buf.get_len_remaining(), 0);
    }

    #[test]
    fn u_prepare_rejects_serializing_raw_sdu_until_length_is_modelled() {
        let pdu = UPrepare {
            cell_identifier_ca: Some(3),
            sdu: Some(0b1010),
        };
        let mut buf = BitBuffer::new_autoexpand(16);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::NotImplemented {
                field: Some("u_prepare_sdu"),
            })
        );
        assert_eq!(buf.get_len(), 3);
    }
}
