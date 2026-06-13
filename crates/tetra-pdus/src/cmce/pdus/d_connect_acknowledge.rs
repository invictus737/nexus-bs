// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use crate::cmce::enums::call_timeout::CallTimeout;
use crate::cmce::enums::transmission_grant::TransmissionGrant;
use crate::cmce::enums::{cmce_pdu_type_dl::CmcePduTypeDl, type3_elem_id::CmceType3ElemId};
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

const MAX_U14: u64 = 0x3fff;
const MAX_U6: u64 = 0x3f;

/// Representation of the D-CONNECT ACKNOWLEDGE PDU (Clause 14.7.1.5).
/// This PDU shall be the order to the called MS to through-connect.
/// Response expected: -
/// Response to: U-CONNECT

#[derive(Debug)]
pub struct DConnectAcknowledge {
    /// Type1, 14 bits, Call identifier
    pub call_identifier: u16,
    /// Type1, 4 bits, Call time-out (clause 14.8.16)
    pub call_time_out: CallTimeout,
    /// Type1, 2 bits, Transmission grant (clause 14.8.42)
    pub transmission_grant: TransmissionGrant,
    /// Type1, 1 bits, Transmission request permission
    /// EN 300 392-2 14.8.43/table 14.81 bit: false/0 = allowed to
    /// request transmission, true/1 = not allowed to request transmission.
    pub transmission_request_permission: bool,
    /// Type2, 6 bits, Notification indicator
    pub notification_indicator: Option<u64>,
    /// Type3, Facility
    pub facility: Option<Type3FieldGeneric>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl DConnectAcknowledge {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 clause 14.7.1.5 / table 14.8 uses a 14-bit
        // Call identifier and a 6-bit Notification indicator.
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
        expect_pdu_type!(pdu_type, CmcePduTypeDl::DConnectAcknowledge)?;

        // Type1
        let call_identifier = buffer.read_field(14, "call_identifier")? as u16;
        // Type1
        let call_time_out = CallTimeout::try_from(buffer.read_field(4, "call_time_out")?)
            .expect("4-bit D-CONNECT ACKNOWLEDGE call_time_out is always an ETSI table 14.8.16 value");
        // Type1
        let transmission_grant = TransmissionGrant::try_from(buffer.read_field(2, "transmission_grant")?)
            .expect("2-bit D-CONNECT ACKNOWLEDGE transmission_grant is always an ETSI table 14.8.42 value");
        // Type1
        let transmission_request_permission = buffer.read_field(1, "transmission_request_permission")? != 0;

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

        Ok(DConnectAcknowledge {
            call_identifier,
            call_time_out,
            transmission_grant,
            transmission_request_permission,
            notification_indicator,
            facility,
            proprietary,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // PDU Type
        buffer.write_bits(CmcePduTypeDl::DConnectAcknowledge.into_raw(), 5);
        // Type1
        buffer.write_bits(self.call_identifier as u64, 14);
        // Type1
        buffer.write_bits(self.call_time_out.into_raw(), 4);
        // Type1
        buffer.write_bits(self.transmission_grant.into_raw(), 2);
        // Type1
        buffer.write_bits(self.transmission_request_permission as u64, 1);

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

impl fmt::Display for DConnectAcknowledge {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DConnectAcknowledge {{ call_identifier: {:?} call_time_out: {:?} transmission_grant: {:?} transmission_request_permission: {:?} notification_indicator: {:?} facility: {:?} proprietary: {:?} }}",
            self.call_identifier,
            self.call_time_out,
            self.transmission_grant,
            self.transmission_request_permission,
            self.notification_indicator,
            self.facility,
            self.proprietary,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_d_connect_acknowledge() -> DConnectAcknowledge {
        DConnectAcknowledge {
            call_identifier: 0x1234,
            call_time_out: CallTimeout::T10m,
            transmission_grant: TransmissionGrant::GrantedToOtherUser,
            transmission_request_permission: true,
            notification_indicator: Some(0x21),
            facility: None,
            proprietary: None,
        }
    }

    fn serialize_err(pdu: &DConnectAcknowledge) -> PduParseErr {
        let mut buf = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut buf).unwrap_err()
    }

    #[test]
    fn d_connect_acknowledge_transmission_request_permission_false_serializes_etsi_allowed_zero() {
        let pdu = DConnectAcknowledge {
            transmission_request_permission: false,
            notification_indicator: None,
            ..base_d_connect_acknowledge()
        };

        let mut encoded = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut encoded)
            .expect("serialize D-CONNECT ACKNOWLEDGE with ETSI transmission request permission");

        // EN 300 392-2 14.8.43/table 14.81: raw bit 0 means transmission requests are allowed.
        assert_eq!(encoded.to_bitstr().chars().nth(25), Some('0'));
        encoded.seek(0);
        let decoded = DConnectAcknowledge::from_bitbuf(&mut encoded).expect("parse D-CONNECT ACKNOWLEDGE");
        assert!(!decoded.transmission_request_permission);
    }

    #[test]
    fn d_connect_acknowledge_transmission_request_permission_true_serializes_etsi_not_allowed_one() {
        let pdu = DConnectAcknowledge {
            transmission_request_permission: true,
            notification_indicator: None,
            ..base_d_connect_acknowledge()
        };

        let mut encoded = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut encoded)
            .expect("serialize D-CONNECT ACKNOWLEDGE with ETSI transmission request permission");

        // EN 300 392-2 14.8.43/table 14.81: raw bit 1 means transmission requests are not allowed.
        assert_eq!(encoded.to_bitstr().chars().nth(25), Some('1'));
        encoded.seek(0);
        let decoded = DConnectAcknowledge::from_bitbuf(&mut encoded).expect("parse D-CONNECT ACKNOWLEDGE");
        assert!(decoded.transmission_request_permission);
    }

    #[test]
    fn d_connect_acknowledge_roundtrip_uses_typed_timeout_and_grant() {
        let pdu = DConnectAcknowledge {
            notification_indicator: None,
            ..base_d_connect_acknowledge()
        };

        let mut encoded = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut encoded)
            .expect("serialize D-CONNECT ACKNOWLEDGE with ETSI table values");
        encoded.seek(0);

        // EN 300 392-2 table 14.8 carries Call time-out and Transmission
        // grant; clauses 14.8.16 and 14.8.42 define their enumerated values.
        let decoded = DConnectAcknowledge::from_bitbuf(&mut encoded).expect("parse encoded D-CONNECT ACKNOWLEDGE");
        assert_eq!(decoded.call_identifier, 0x1234);
        assert_eq!(decoded.call_time_out, CallTimeout::T10m);
        assert_eq!(decoded.transmission_grant, TransmissionGrant::GrantedToOtherUser);
        assert!(decoded.transmission_request_permission);
    }

    #[test]
    fn d_connect_acknowledge_rejects_overwide_call_identifier() {
        let pdu = DConnectAcknowledge {
            call_identifier: 0x4000,
            ..base_d_connect_acknowledge()
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
    fn d_connect_acknowledge_rejects_overwide_notification_indicator() {
        let pdu = DConnectAcknowledge {
            notification_indicator: Some(0x40),
            ..base_d_connect_acknowledge()
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
