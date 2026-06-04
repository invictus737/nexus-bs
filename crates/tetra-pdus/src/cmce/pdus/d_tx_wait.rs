use core::fmt;

use crate::cmce::enums::{cmce_pdu_type_dl::CmcePduTypeDl, type3_elem_id::CmceType3ElemId};
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

const MAX_U14: u64 = 0x3fff;
const MAX_U6: u64 = 0x3f;

/// Representation of the D-TX WAIT PDU (Clause 14.7.1.17).
/// This PDU shall be a message from the SwMI that the call is being interrupted.
/// Response expected: -
/// Response to: U-TX DEMAND

#[derive(Debug)]
pub struct DTxWait {
    /// Type1, 14 bits, Call identifier
    pub call_identifier: u16,
    /// Type1, 1 bits, Transmission request permission
    /// Set to true to signal MSes they are allowed to send a U-TX DEMAND
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

impl DTxWait {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 clause 14.7.1.17 / table 14.20 uses a 14-bit
        // Call identifier and a 6-bit Notification indicator. Reject
        // over-wide host values instead of silently truncating them on air.
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
        expect_pdu_type!(pdu_type, CmcePduTypeDl::DTxWait)?;

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

        Ok(DTxWait {
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
        buffer.write_bits(CmcePduTypeDl::DTxWait.into_raw(), 5);
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
    use tetra_core::BitBuffer;

    fn base_d_tx_wait() -> DTxWait {
        DTxWait {
            call_identifier: 0x1234,
            transmission_request_permission: true,
            notification_indicator: Some(0x21),
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        }
    }

    fn serialize_err(pdu: &DTxWait) -> PduParseErr {
        let mut buf = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut buf).unwrap_err()
    }

    #[test]
    fn d_tx_wait_roundtrips_type1_and_notification_indicator() {
        let pdu = base_d_tx_wait();
        let mut buf = BitBuffer::new_autoexpand(64);

        pdu.to_bitbuf(&mut buf).expect("serialize D-TX WAIT");
        buf.seek(0);
        let decoded = DTxWait::from_bitbuf(&mut buf).expect("parse D-TX WAIT");

        assert_eq!(decoded.call_identifier, 0x1234);
        assert!(decoded.transmission_request_permission);
        assert_eq!(decoded.notification_indicator, Some(0x21));
    }

    #[test]
    fn d_tx_wait_rejects_overwide_call_identifier() {
        let pdu = DTxWait {
            call_identifier: 0x4000,
            ..base_d_tx_wait()
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
    fn d_tx_wait_rejects_overwide_notification_indicator() {
        let pdu = DTxWait {
            notification_indicator: Some(0x40),
            ..base_d_tx_wait()
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

impl fmt::Display for DTxWait {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DTxWait {{ call_identifier: {:?} transmission_request_permission: {:?} notification_indicator: {:?} facility: {:?} dm_ms_address: {:?} proprietary: {:?} }}",
            self.call_identifier,
            self.transmission_request_permission,
            self.notification_indicator,
            self.facility,
            self.dm_ms_address,
            self.proprietary,
        )
    }
}
