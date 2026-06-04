use core::fmt;

use crate::cmce::enums::{cmce_pdu_type_dl::CmcePduTypeDl, type3_elem_id::CmceType3ElemId};
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

const MAX_U3: u64 = 0x07;
const MAX_U4: u64 = 0x0f;
const MAX_U6: u64 = 0x3f;
const MAX_U9: u64 = 0x01ff;
const MAX_U14: u64 = 0x3fff;

/// Representation of the D-CALL RESTORE PDU (Clause 14.7.1.3).
/// This PDU shall indicate to the MS that a call has been restored after a temporary break of the call.
/// Response expected: -
/// Response to: U-CALL RESTORE

#[derive(Debug)]
pub struct DCallRestore {
    /// Type1, 14 bits, Call identifier
    pub call_identifier: u16,
    /// Type1, 2 bits, Transmission grant
    pub transmission_grant: u8,
    /// Type1, 1 bits, Transmission request permission
    /// ETSI 14.8.43: 0 = allowed to request transmission, 1 = not allowed.
    pub transmission_request_permission: bool,
    /// Type1, 1 bits, Reset call time-out timer (T310)
    pub reset_call_time_out_timer_t310_: bool,
    /// Type2, 14 bits, New call identifier
    pub new_call_identifier: Option<u64>,
    /// Type2, 4 bits, Call time-out
    pub call_time_out: Option<u64>,
    /// Type2, 3 bits, Call status
    pub call_status: Option<u64>,
    /// Type2, 9 bits, Modify
    pub modify: Option<u64>,
    /// Type2, 6 bits, Notification indicator
    pub notification_indicator: Option<u64>,
    /// Type3, Facility
    pub facility: Option<Type3FieldGeneric>,
    /// Type3, Temporary address
    pub temporary_address: Option<Type3FieldGeneric>,
    /// Type3, DM-MS address
    pub dm_ms_address: Option<Type3FieldGeneric>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl DCallRestore {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 table 14.6 gives the exact Type1/Type2 widths.
        if self.call_identifier as u64 > MAX_U14 {
            return Err(PduParseErr::InvalidValue {
                field: "call_identifier",
                value: self.call_identifier as u64,
            });
        }
        if self.transmission_grant > 0x03 {
            return Err(PduParseErr::InvalidValue {
                field: "transmission_grant",
                value: self.transmission_grant as u64,
            });
        }

        if let Some(new_call_identifier) = self.new_call_identifier {
            if new_call_identifier > MAX_U14 {
                return Err(PduParseErr::InvalidValue {
                    field: "new_call_identifier",
                    value: new_call_identifier,
                });
            }
        }
        if let Some(call_time_out) = self.call_time_out {
            if call_time_out > MAX_U4 {
                return Err(PduParseErr::InvalidValue {
                    field: "call_time_out",
                    value: call_time_out,
                });
            }
        }
        if let Some(call_status) = self.call_status {
            if call_status > MAX_U3 {
                return Err(PduParseErr::InvalidValue {
                    field: "call_status",
                    value: call_status,
                });
            }
        }
        if let Some(modify) = self.modify {
            if modify > MAX_U9 {
                return Err(PduParseErr::InvalidValue {
                    field: "modify",
                    value: modify,
                });
            }
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
        expect_pdu_type!(pdu_type, CmcePduTypeDl::DCallRestore)?;

        // Type1
        let call_identifier = buffer.read_field(14, "call_identifier")? as u16;
        // Type1
        let transmission_grant = buffer.read_field(2, "transmission_grant")? as u8;
        // Type1
        let transmission_request_permission = buffer.read_field(1, "transmission_request_permission")? != 0;
        // Type1
        let reset_call_time_out_timer_t310_ = buffer.read_field(1, "reset_call_time_out_timer_t310_")? != 0;

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type2
        let new_call_identifier = typed::parse_type2_generic(obit, buffer, 14, "new_call_identifier")?;
        // Type2
        let call_time_out = typed::parse_type2_generic(obit, buffer, 4, "call_time_out")?;
        // Type2
        let call_status = typed::parse_type2_generic(obit, buffer, 3, "call_status")?;
        // Type2
        let modify = typed::parse_type2_generic(obit, buffer, 9, "modify")?;
        // Type2
        let notification_indicator = typed::parse_type2_generic(obit, buffer, 6, "notification_indicator")?;

        // Type3
        let facility = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::Facility)?;
        // Type3
        let temporary_address = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::TempAddr)?;
        // Type3
        let dm_ms_address = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::DmMsAddr)?;
        // Type3
        let proprietary = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::Proprietary)?;

        // Read trailing mbit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        let pdu = DCallRestore {
            call_identifier,
            transmission_grant,
            transmission_request_permission,
            reset_call_time_out_timer_t310_,
            new_call_identifier,
            call_time_out,
            call_status,
            modify,
            notification_indicator,
            facility,
            temporary_address,
            dm_ms_address,
            proprietary,
        };
        pdu.validate()?;
        Ok(pdu)
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // PDU Type
        buffer.write_bits(CmcePduTypeDl::DCallRestore.into_raw(), 5);
        // Type1
        buffer.write_bits(self.call_identifier as u64, 14);
        // Type1
        buffer.write_bits(self.transmission_grant as u64, 2);
        // Type1
        buffer.write_bits(self.transmission_request_permission as u64, 1);
        // Type1
        buffer.write_bits(self.reset_call_time_out_timer_t310_ as u64, 1);

        // Check if any optional field present and place o-bit
        let obit = self.new_call_identifier.is_some()
            || self.call_time_out.is_some()
            || self.call_status.is_some()
            || self.modify.is_some()
            || self.notification_indicator.is_some()
            || self.facility.is_some()
            || self.temporary_address.is_some()
            || self.dm_ms_address.is_some()
            || self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2
        typed::write_type2_generic(obit, buffer, self.new_call_identifier, 14);

        // Type2
        typed::write_type2_generic(obit, buffer, self.call_time_out, 4);

        // Type2
        typed::write_type2_generic(obit, buffer, self.call_status, 3);

        // Type2
        typed::write_type2_generic(obit, buffer, self.modify, 9);

        // Type2
        typed::write_type2_generic(obit, buffer, self.notification_indicator, 6);

        // Type3
        typed::write_type3_generic(obit, buffer, &self.facility, CmceType3ElemId::Facility)?;

        // Type3
        typed::write_type3_generic(obit, buffer, &self.temporary_address, CmceType3ElemId::TempAddr)?;

        // Type3
        typed::write_type3_generic(obit, buffer, &self.dm_ms_address, CmceType3ElemId::DmMsAddr)?;

        // Type3
        typed::write_type3_generic(obit, buffer, &self.proprietary, CmceType3ElemId::Proprietary)?;

        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for DCallRestore {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DCallRestore {{ call_identifier: {:?} transmission_grant: {:?} transmission_request_permission: {:?} reset_call_time_out_timer_t310_: {:?} new_call_identifier: {:?} call_time_out: {:?} call_status: {:?} modify: {:?} notification_indicator: {:?} facility: {:?} temporary_address: {:?} dm_ms_address: {:?} proprietary: {:?} }}",
            self.call_identifier,
            self.transmission_grant,
            self.transmission_request_permission,
            self.reset_call_time_out_timer_t310_,
            self.new_call_identifier,
            self.call_time_out,
            self.call_status,
            self.modify,
            self.notification_indicator,
            self.facility,
            self.temporary_address,
            self.dm_ms_address,
            self.proprietary,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_core::BitBuffer;

    fn base_d_call_restore() -> DCallRestore {
        DCallRestore {
            call_identifier: 0x1234,
            transmission_grant: 0,
            transmission_request_permission: false,
            reset_call_time_out_timer_t310_: false,
            new_call_identifier: None,
            call_time_out: None,
            call_status: None,
            modify: None,
            notification_indicator: None,
            facility: None,
            temporary_address: None,
            dm_ms_address: None,
            proprietary: None,
        }
    }

    fn round_trip(pdu: &DCallRestore) -> DCallRestore {
        let mut buf = BitBuffer::new_autoexpand(128);
        pdu.to_bitbuf(&mut buf).expect("serialize D-CALL RESTORE");
        buf.seek(0);
        DCallRestore::from_bitbuf(&mut buf).expect("parse D-CALL RESTORE")
    }

    #[test]
    fn d_call_restore_roundtrips_type1_only() {
        let decoded = round_trip(&base_d_call_restore());

        assert_eq!(decoded.call_identifier, 0x1234);
        assert_eq!(decoded.transmission_grant, 0);
        assert!(!decoded.transmission_request_permission);
        assert!(!decoded.reset_call_time_out_timer_t310_);
        assert!(decoded.new_call_identifier.is_none());
    }

    #[test]
    fn d_call_restore_roundtrips_type2_fields() {
        let pdu = DCallRestore {
            transmission_grant: 3,
            transmission_request_permission: true,
            reset_call_time_out_timer_t310_: true,
            new_call_identifier: Some(0x0102),
            call_time_out: Some(0x0f),
            call_status: Some(0x07),
            modify: Some(0x01ff),
            notification_indicator: Some(0x3f),
            ..base_d_call_restore()
        };
        let decoded = round_trip(&pdu);

        assert_eq!(decoded.transmission_grant, 3);
        assert!(decoded.transmission_request_permission);
        assert!(decoded.reset_call_time_out_timer_t310_);
        assert_eq!(decoded.new_call_identifier, Some(0x0102));
        assert_eq!(decoded.call_time_out, Some(0x0f));
        assert_eq!(decoded.call_status, Some(0x07));
        assert_eq!(decoded.modify, Some(0x01ff));
        assert_eq!(decoded.notification_indicator, Some(0x3f));
    }

    #[test]
    fn d_call_restore_rejects_overwide_transmission_grant() {
        let pdu = DCallRestore {
            transmission_grant: 4,
            ..base_d_call_restore()
        };
        let mut buf = BitBuffer::new_autoexpand(64);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "transmission_grant",
                value: 4,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_call_restore_rejects_overwide_type2_fields() {
        let cases = [
            (
                DCallRestore {
                    new_call_identifier: Some(0x4000),
                    ..base_d_call_restore()
                },
                "new_call_identifier",
                0x4000,
            ),
            (
                DCallRestore {
                    call_time_out: Some(0x10),
                    ..base_d_call_restore()
                },
                "call_time_out",
                0x10,
            ),
            (
                DCallRestore {
                    call_status: Some(0x08),
                    ..base_d_call_restore()
                },
                "call_status",
                0x08,
            ),
            (
                DCallRestore {
                    modify: Some(0x0200),
                    ..base_d_call_restore()
                },
                "modify",
                0x0200,
            ),
            (
                DCallRestore {
                    notification_indicator: Some(0x40),
                    ..base_d_call_restore()
                },
                "notification_indicator",
                0x40,
            ),
        ];

        for (pdu, field, value) in cases {
            let mut buf = BitBuffer::new_autoexpand(64);
            assert_eq!(pdu.to_bitbuf(&mut buf), Err(PduParseErr::InvalidValue { field, value }));
            assert_eq!(buf.get_len(), 0);
        }
    }
}
