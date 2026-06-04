use core::fmt;

use crate::cmce::enums::disconnect_cause::DisconnectCause;
use crate::cmce::enums::{cmce_pdu_type_dl::CmcePduTypeDl, type3_elem_id::CmceType3ElemId};
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

const MAX_U14: u64 = 0x3fff;
const MAX_U6: u64 = 0x3f;

/// Representation of the D-RELEASE PDU (Clause 14.7.1.9).
/// This PDU shall be a message from the infrastructure to the MS to inform that the connection has been released.
/// Response expected: -
/// Response to: -/U-DISCONNECT

#[derive(Debug)]
pub struct DRelease {
    /// Type1, 14 bits, Call identifier
    pub call_identifier: u16,
    /// Type1, 5 bits, Disconnect cause
    pub disconnect_cause: DisconnectCause,
    /// Type2, 6 bits, Notification indicator
    pub notification_indicator: Option<u64>,
    /// Type3, Facility
    pub facility: Option<Type3FieldGeneric>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl DRelease {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 clause 14.7.1.9 / table 14.13 define the call
        // identifier as a 14-bit Type1 element and notification indicator as
        // a 6-bit Type2 element. Reject over-wide values instead of silently
        // truncating on serialization.
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
        expect_pdu_type!(pdu_type, CmcePduTypeDl::DRelease)?;

        // Type1
        let call_identifier = buffer.read_field(14, "call_identifier")? as u16;

        // Type1
        let val = buffer.read_field(5, "disconnect_cause")?;
        let disconnect_cause = DisconnectCause::try_from(val).map_err(|_| PduParseErr::InvalidValue {
            field: "disconnect_cause",
            value: val,
        })?;

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type2
        let notification_indicator = typed::parse_type2_generic(obit, buffer, 6, "notification_indicator")?;

        // Type3
        let facility = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::Facility)?;

        // Type3
        let proprietary = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::Proprietary)?;

        // Read trailing mbit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        Ok(DRelease {
            call_identifier,
            disconnect_cause,
            notification_indicator,
            facility,
            proprietary,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // PDU Type
        buffer.write_bits(CmcePduTypeDl::DRelease.into_raw(), 5);
        // Type1
        buffer.write_bits(self.call_identifier as u64, 14);
        // Type1
        buffer.write_bits(self.disconnect_cause as u64, 5);

        // Check if any optional field present and place o-bit
        let obit = self.notification_indicator.is_some() || self.facility.is_some() || self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2
        typed::write_type2_generic(obit, buffer, self.notification_indicator, 6);

        // Type3
        typed::write_type3_generic(obit, buffer, &self.facility, CmceType3ElemId::Facility)?;

        // Type3
        typed::write_type3_generic(obit, buffer, &self.proprietary, CmceType3ElemId::Proprietary)?;

        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for DRelease {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DRelease {{ call_identifier: {:?} disconnect_cause: {:?} notification_indicator: {:?} facility: {:?} proprietary: {:?} }}",
            self.call_identifier, self.disconnect_cause, self.notification_indicator, self.facility, self.proprietary,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_core::debug;

    #[test]
    fn test_parse_d_release() {
        debug::setup_logging_verbose();
        let bitstr = "0011000000011011001011010";
        let mut buffer = BitBuffer::from_bitstr(bitstr);
        let result = DRelease::from_bitbuf(&mut buffer).unwrap();

        tracing::info!("Parsed DRelease: {:?}", result);
        tracing::info!("buf:        {}", buffer.dump_bin());
        assert_eq!(result.call_identifier, 217);
        assert_eq!(result.disconnect_cause, DisconnectCause::ExpiryOfTimer);

        let mut buffer_out = BitBuffer::new_autoexpand(30);
        let _ = result.to_bitbuf(&mut buffer_out);
        tracing::info!("Serialized: {}", buffer_out.dump_bin());
        assert_eq!(bitstr, buffer_out.to_bitstr());
        assert!(buffer.get_len_remaining() == 0);
    }

    #[test]
    fn d_release_rejects_overwide_call_identifier() {
        let pdu = DRelease {
            call_identifier: 0x4000,
            disconnect_cause: DisconnectCause::ExpiryOfTimer,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };
        let mut buffer = BitBuffer::new_autoexpand(32);

        // EN 300 392-2 clause 14.7.1.9: D-RELEASE call identifier is 14 bits.
        assert_eq!(
            pdu.to_bitbuf(&mut buffer),
            Err(PduParseErr::InvalidValue {
                field: "call_identifier",
                value: 0x4000
            })
        );
    }

    #[test]
    fn d_release_rejects_overwide_notification_indicator() {
        let pdu = DRelease {
            call_identifier: 1,
            disconnect_cause: DisconnectCause::ExpiryOfTimer,
            notification_indicator: Some(0x40),
            facility: None,
            proprietary: None,
        };
        let mut buffer = BitBuffer::new_autoexpand(32);

        // EN 300 392-2 table 14.13: notification indicator is 6 bits.
        assert_eq!(
            pdu.to_bitbuf(&mut buffer),
            Err(PduParseErr::InvalidValue {
                field: "notification_indicator",
                value: 0x40
            })
        );
    }
}
