use core::fmt;

use crate::cmce::enums::{cmce_pdu_type_ul::CmcePduTypeUl, type3_elem_id::CmceType3ElemId};
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

const MAX_U9: u64 = 0x01ff;
const MAX_U14: u64 = 0x3fff;

/// Representation of the U-INFO PDU (Clause 14.7.2.6).
/// This PDU shall be the general information message from the MS.
/// Response expected: -
/// Response to: -

// note 1: If the message is sent connectionless then the call identifier shall be equal to the dummy call identifier.
// note 2: Shall be valid for acknowledged group call only. For other types of call it shall be set equal to zero.
#[derive(Debug)]
pub struct UInfo {
    /// Type1, 14 bits, See note 1,
    pub call_identifier: u16,
    /// Type1, 1 bits, See note 2,
    pub poll_response: bool,
    /// Type2, 9 bits, Modify
    pub modify: Option<u64>,
    /// Type3, DTMF
    pub dtmf: Option<Type3FieldGeneric>,
    /// Type3, Facility
    pub facility: Option<Type3FieldGeneric>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl UInfo {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 clause 14.7.2.6 / table 14.26 fixes the Call
        // identifier to 14 bits and Modify to 9 bits. The poll-response
        // context rule depends on the active call type and is handled by CMCE.
        if self.call_identifier as u64 > MAX_U14 {
            return Err(PduParseErr::InvalidValue {
                field: "call_identifier",
                value: self.call_identifier as u64,
            });
        }
        if let Some(modify) = self.modify {
            if modify > MAX_U9 {
                return Err(PduParseErr::InvalidValue {
                    field: "modify",
                    value: modify,
                });
            }
        }

        Ok(())
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(5, "pdu_type")?;
        expect_pdu_type!(pdu_type, CmcePduTypeUl::UInfo)?;

        // Type1
        let call_identifier = buffer.read_field(14, "call_identifier")? as u16;
        // Type1
        let poll_response = buffer.read_field(1, "poll_response")? != 0;

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type2
        let modify = typed::parse_type2_generic(obit, buffer, 9, "modify")?;

        // Type3
        let dtmf = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::Dtmf)?;

        // Type3
        let facility = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::Facility)?;

        // Type3
        let proprietary = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::Proprietary)?;

        // Read trailing mbit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        Ok(UInfo {
            call_identifier,
            poll_response,
            modify,
            dtmf,
            facility,
            proprietary,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // PDU Type
        buffer.write_bits(CmcePduTypeUl::UInfo.into_raw(), 5);
        // Type1
        buffer.write_bits(self.call_identifier as u64, 14);
        // Type1
        buffer.write_bits(self.poll_response as u64, 1);

        // Check if any optional field present and place o-bit
        let obit = self.modify.is_some() || self.dtmf.is_some() || self.facility.is_some() || self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2
        typed::write_type2_generic(obit, buffer, self.modify, 9);

        // Type3
        typed::write_type3_generic(obit, buffer, &self.dtmf, CmceType3ElemId::Dtmf)?;

        // Type3
        typed::write_type3_generic(obit, buffer, &self.facility, CmceType3ElemId::Facility)?;

        // Type3
        typed::write_type3_generic(obit, buffer, &self.proprietary, CmceType3ElemId::Proprietary)?;

        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_u_info() -> UInfo {
        UInfo {
            call_identifier: 0x1234,
            poll_response: false,
            modify: Some(0x01ff),
            dtmf: None,
            facility: None,
            proprietary: None,
        }
    }

    fn serialize_err(pdu: &UInfo) -> PduParseErr {
        let mut buf = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut buf).unwrap_err()
    }

    #[test]
    fn u_info_rejects_overwide_call_identifier() {
        let pdu = UInfo {
            call_identifier: 0x4000,
            ..base_u_info()
        };

        assert!(matches!(
            serialize_err(&pdu),
            PduParseErr::InvalidValue {
                field: "call_identifier",
                value: 0x4000
            }
        ));
    }

    #[test]
    fn u_info_rejects_overwide_modify() {
        let pdu = UInfo {
            modify: Some(0x0200),
            ..base_u_info()
        };

        assert!(matches!(
            serialize_err(&pdu),
            PduParseErr::InvalidValue {
                field: "modify",
                value: 0x0200
            }
        ));
    }
}

impl fmt::Display for UInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "UInfo {{ call_identifier: {:?} poll_response: {:?} modify: {:?} dtmf: {:?} facility: {:?} proprietary: {:?} }}",
            self.call_identifier, self.poll_response, self.modify, self.dtmf, self.facility, self.proprietary,
        )
    }
}
