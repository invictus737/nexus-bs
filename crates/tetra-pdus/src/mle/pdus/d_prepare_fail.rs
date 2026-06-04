use core::fmt;

use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use crate::mle::enums::mle_pdu_type_dl::MlePduTypeDl;
use crate::mle::pdus::raw_sdu;

/// Representation of the D-PREPARE-FAIL PDU (Clause 18.4.1.4.3).
/// Upon receipt from the SwMI the message shall be used by the MS-MLE as a preparation failure, while announcing cell reselection to the old cell.
/// Response expected: -
/// Response to: U-PREPARE/U-PREPARE-DA

// note 1: The SDU may carry an MM registration PDU. The SDU is coded according to the MM protocol description. There shall be no P-bit in the PDU coding preceding the SDU information element.
#[derive(Debug)]
pub struct DPrepareFail {
    /// Type1, 2 bits, Fail cause
    pub fail_cause: u8,
    /// Conditional See note,
    pub sdu: Option<u64>,
}

impl DPrepareFail {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeDl::DPrepareFail)?;

        // Type1
        let fail_cause = buffer.read_field(2, "fail_cause")? as u8;
        // EN 300 392-2 table 18.7: optional MM SDU has no preceding P-bit and
        // consumes the remaining payload.
        let sdu = raw_sdu::read_remaining_u64(buffer, "d_prepare_fail_sdu")?;

        Ok(DPrepareFail { fail_cause, sdu })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        // PDU Type
        buffer.write_bits(MlePduTypeDl::DPrepareFail.into_raw(), 3);
        // Type1
        buffer.write_bits(self.fail_cause as u64, 2);
        raw_sdu::reject_write_if_present(self.sdu, "d_prepare_fail_sdu")?;
        Ok(())
    }
}

impl fmt::Display for DPrepareFail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DPrepareFail {{ fail_cause: {:?} sdu: {:?} }}", self.fail_cause, self.sdu,)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d_prepare_fail_parses_remaining_payload_as_sdu() {
        let mut buf = BitBuffer::new_autoexpand(16);
        buf.write_bits(MlePduTypeDl::DPrepareFail.into_raw(), 3);
        buf.write_bits(1, 2);
        buf.write_bits(0b10101, 5);
        buf.seek(0);

        let parsed = DPrepareFail::from_bitbuf(&mut buf).expect("parse D-PREPARE-FAIL");

        assert_eq!(parsed.fail_cause, 1);
        assert_eq!(parsed.sdu, Some(0b10101));
        assert_eq!(buf.get_len_remaining(), 0);
    }

    #[test]
    fn d_prepare_fail_rejects_serializing_raw_sdu_until_length_is_modelled() {
        let pdu = DPrepareFail {
            fail_cause: 1,
            sdu: Some(0b10101),
        };
        let mut buf = BitBuffer::new_autoexpand(16);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::NotImplemented {
                field: Some("d_prepare_fail_sdu"),
            })
        );
    }
}
