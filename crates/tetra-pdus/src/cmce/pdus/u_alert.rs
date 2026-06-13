// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use crate::cmce::enums::{cmce_pdu_type_ul::CmcePduTypeUl, type3_elem_id::CmceType3ElemId};
use crate::cmce::fields::basic_service_information::BasicServiceInformation;
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

const MAX_U14: u64 = 0x3fff;

/// Representation of the U-ALERT PDU (Clause 14.7.2.1).
/// This PDU shall be an acknowledgement from the called MS that the called user has been alerted.
/// Response expected: -
/// Response to: D-SETUP

// note 1: This information element is not used in this edition of the present document and its value shall be set to "1" (equivalent to "Hook on/Hook off signalling" for backwards compatibility with edition 1 of the present document – refer to table 14.62).
#[derive(Debug)]
pub struct UAlert {
    /// Type1, 14 bits, Call identifier
    pub call_identifier: u16,
    /// Type1, 1 bits, See note,
    pub reserved: bool,
    /// Type1, 1 bits, Simplex/duplex selection
    pub simplex_duplex_selection: bool,
    /// Type2, 8 bits, Basic service information
    pub basic_service_information: Option<BasicServiceInformation>,
    /// Type3, Facility
    pub facility: Option<Type3FieldGeneric>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl UAlert {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 clause 14.7.2.1 / table 14.21 fixes the Call
        // identifier to 14 bits, and the note requires the reserved bit to be 1.
        if self.call_identifier as u64 > MAX_U14 {
            return Err(PduParseErr::InvalidValue {
                field: "call_identifier",
                value: self.call_identifier as u64,
            });
        }
        if !self.reserved {
            return Err(PduParseErr::InvalidValue {
                field: "reserved",
                value: 0,
            });
        }
        if let Some(bsi) = &self.basic_service_information {
            bsi.validate()?;
        }

        Ok(())
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(5, "pdu_type")?;
        expect_pdu_type!(pdu_type, CmcePduTypeUl::UAlert)?;

        // Type1
        let call_identifier = buffer.read_field(14, "call_identifier")? as u16;
        // Type1
        let reserved = buffer.read_field(1, "reserved")? != 0;
        // Type1
        let simplex_duplex_selection = buffer.read_field(1, "simplex_duplex_selection")? != 0;

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type2
        let basic_service_information = typed::parse_type2_struct(obit, buffer, BasicServiceInformation::from_bitbuf)?;

        // Type3
        let facility = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::Facility)?;

        // Type3
        let proprietary = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::Proprietary)?;

        // Read trailing mbit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        Ok(UAlert {
            call_identifier,
            reserved,
            simplex_duplex_selection,
            basic_service_information,
            facility,
            proprietary,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // PDU Type
        buffer.write_bits(CmcePduTypeUl::UAlert.into_raw(), 5);
        // Type1
        buffer.write_bits(self.call_identifier as u64, 14);
        // Type1
        buffer.write_bits(self.reserved as u64, 1);
        // Type1
        buffer.write_bits(self.simplex_duplex_selection as u64, 1);

        // Check if any optional field present and place o-bit
        let obit = self.basic_service_information.is_some() || self.facility.is_some() || self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2
        typed::write_type2_struct(obit, buffer, &self.basic_service_information, BasicServiceInformation::to_bitbuf)?;

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

    fn base_u_alert() -> UAlert {
        UAlert {
            call_identifier: 0x1234,
            reserved: true,
            simplex_duplex_selection: false,
            basic_service_information: None,
            facility: None,
            proprietary: None,
        }
    }

    fn serialize_err(pdu: &UAlert) -> PduParseErr {
        let mut buf = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut buf).unwrap_err()
    }

    #[test]
    fn u_alert_rejects_overwide_call_identifier() {
        let pdu = UAlert {
            call_identifier: 0x4000,
            ..base_u_alert()
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
    fn u_alert_requires_reserved_bit_set() {
        let pdu = UAlert {
            reserved: false,
            ..base_u_alert()
        };

        assert!(matches!(
            serialize_err(&pdu),
            PduParseErr::InvalidValue {
                field: "reserved",
                value: 0
            }
        ));
    }
}

impl fmt::Display for UAlert {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "UAlert {{ call_identifier: {:?} reserved: {:?} simplex_duplex_selection: {:?} basic_service_information: {:?} facility: {:?} proprietary: {:?} }}",
            self.call_identifier,
            self.reserved,
            self.simplex_duplex_selection,
            self.basic_service_information,
            self.facility,
            self.proprietary,
        )
    }
}
