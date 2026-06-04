use core::fmt;

use crate::cmce::enums::{cmce_pdu_type_dl::CmcePduTypeDl, type3_elem_id::CmceType3ElemId};
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

const MAX_U6: u64 = 0x3f;
const MAX_U14: u64 = 0x3fff;

/// Representation of the D-TX CEASED PDU (Clause 14.7.1.13).
/// This PDU shall be the PDU from the SwMI to all MS within a call that a transmission has ceased.
/// Response expected: -
/// Response to: U-TX CEASED

#[derive(Debug)]
pub struct DTxCeased {
    /// Type1, 14 bits, Call identifier
    pub call_identifier: u16,
    /// Type1, 1 bits, Transmission request permission
    /// ETSI 14.8.43: 0 = allowed to request transmission, 1 = not allowed.
    pub transmission_request_permission: bool,
    /// Type2, 6 bits, Notification indicator
    pub notification_indicator: Option<u64>,
    /// Type3, Facility
    pub facility: Option<Type3FieldGeneric>,
    /// Type3, DM-MS address
    pub dm_ms_address: Option<Type3FieldGeneric>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl DTxCeased {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 clause 14.7.1.13 / table 14.16 fixes the Call
        // identifier to 14 bits and Notification indicator to 6 bits.
        if self.call_identifier as u64 > MAX_U14 {
            return Err(PduParseErr::InvalidValue {
                field: "call_identifier",
                value: self.call_identifier as u64,
            });
        }
        if let Some(notification_indicator) = self.notification_indicator {
            if notification_indicator > MAX_U6 {
                return Err(PduParseErr::InvalidValue {
                    field: "notification_indicator",
                    value: notification_indicator,
                });
            }
        }

        Ok(())
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(5, "pdu_type")?;
        expect_pdu_type!(pdu_type, CmcePduTypeDl::DTxCeased)?;

        // Type1
        let call_identifier = buffer.read_field(14, "call_identifier")? as u16;
        // Type1
        let transmission_request_permission = buffer.read_field(1, "transmission_request_permission")? != 0;

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type2
        let notification_indicator = typed::parse_type2_generic(obit, buffer, 6, "notification_indicator")?;

        // Type3
        let facility = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::Facility)?;

        // Type3
        let dm_ms_address = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::DmMsAddr)?;

        // Type3
        let proprietary = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::Proprietary)?;

        // Read trailing mbit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        Ok(DTxCeased {
            call_identifier,
            transmission_request_permission,
            notification_indicator,
            facility,
            dm_ms_address,
            proprietary,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // PDU Type
        buffer.write_bits(CmcePduTypeDl::DTxCeased.into_raw(), 5);
        // Type1
        buffer.write_bits(self.call_identifier as u64, 14);
        // Type1
        buffer.write_bits(self.transmission_request_permission as u64, 1);

        // Check if any optional field present and place o-bit
        let obit =
            self.notification_indicator.is_some() || self.facility.is_some() || self.dm_ms_address.is_some() || self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2
        typed::write_type2_generic(obit, buffer, self.notification_indicator, 6);

        // Type3
        typed::write_type3_generic(obit, buffer, &self.facility, CmceType3ElemId::Facility)?;

        // Type3
        typed::write_type3_generic(obit, buffer, &self.dm_ms_address, CmceType3ElemId::DmMsAddr)?;

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

    fn base_d_tx_ceased() -> DTxCeased {
        DTxCeased {
            call_identifier: 0x1234,
            transmission_request_permission: false,
            notification_indicator: Some(0x21),
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        }
    }

    fn serialize_err(pdu: &DTxCeased) -> PduParseErr {
        let mut buf = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut buf).unwrap_err()
    }

    #[test]
    fn d_tx_ceased_rejects_overwide_call_identifier() {
        let pdu = DTxCeased {
            call_identifier: 0x4000,
            ..base_d_tx_ceased()
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
    fn d_tx_ceased_rejects_overwide_notification_indicator() {
        let pdu = DTxCeased {
            notification_indicator: Some(0x40),
            ..base_d_tx_ceased()
        };

        assert!(matches!(
            serialize_err(&pdu),
            PduParseErr::InvalidValue {
                field: "notification_indicator",
                value: 0x40
            }
        ));
    }
}

impl fmt::Display for DTxCeased {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DTxCeased {{ call_identifier: {:?} transmission_request_permission: {:?} notification_indicator: {:?} facility: {:?} dm_ms_address: {:?} proprietary: {:?} }}",
            self.call_identifier,
            self.transmission_request_permission,
            self.notification_indicator,
            self.facility,
            self.dm_ms_address,
            self.proprietary,
        )
    }
}
