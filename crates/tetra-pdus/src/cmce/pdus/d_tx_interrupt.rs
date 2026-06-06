use core::fmt;

use crate::cmce::enums::{cmce_pdu_type_dl::CmcePduTypeDl, type3_elem_id::CmceType3ElemId};
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

const TPTI_SSI: u64 = 1;
const TPTI_TSI: u64 = 2;
const MAX_U14: u64 = 0x3fff;
const MAX_U24: u64 = 0x00ff_ffff;
const MAX_U6: u64 = 0x3f;

/// Representation of the D-TX INTERRUPT PDU (Clause 14.7.1.16).
/// This PDU shall be a message from the SwMI indicating that a permission to transmit has been withdrawn.
/// Response expected: -
/// Response to: -

// note 1: This information element is not used in this version of the present document and its value shall be set to "0".
// note 2: Shall be conditional on the value of Transmitting Party Type Identifier (TPTI): TPTI = 1; Transmitting Party SSI; TPTI = 2; Transmitting Party SSI + Transmitting Party Extension.
#[derive(Debug)]
pub struct DTxInterrupt {
    /// Type1, 14 bits, Call identifier
    pub call_identifier: u16,
    /// Type1, 2 bits, Transmission grant
    pub transmission_grant: u8,
    /// Type1, 1 bits, Transmission request permission
    /// EN 300 392-2 14.8.43/table 14.81 bit: false/0 = allowed to
    /// request transmission, true/1 = not allowed to request transmission.
    pub transmission_request_permission: bool,
    /// Type1, 1 bits, Encryption control
    pub encryption_control: bool,
    /// Type1, 1 bits, See note 1,
    pub reserved: bool,
    /// Type2, 6 bits, Notification indicator
    pub notification_indicator: Option<u64>,
    /// Type2, 2 bits, Transmitting party type identifier
    pub transmitting_party_type_identifier: Option<u64>,
    /// Type2, 24 bits, See note 2,
    pub transmitting_party_address_ssi: Option<u64>,
    /// Type2, 24 bits, See note 2,
    pub transmitting_party_extension: Option<u64>,
    /// Type3, External subscriber number
    pub external_subscriber_number: Option<Type3FieldGeneric>,
    /// Type3, Facility
    pub facility: Option<Type3FieldGeneric>,
    /// Type3, DM-MS address
    pub dm_ms_address: Option<Type3FieldGeneric>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl DTxInterrupt {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 table 14.19 note 1: reserved bit shall be set to 0.
        if self.reserved {
            return Err(PduParseErr::InvalidValue {
                field: "reserved",
                value: 1,
            });
        }
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
        if let Some(notification_indicator) = self.notification_indicator {
            if notification_indicator > MAX_U6 {
                return Err(PduParseErr::InvalidValue {
                    field: "notification_indicator",
                    value: notification_indicator,
                });
            }
        }
        if let Some(ssi) = self.transmitting_party_address_ssi {
            if ssi > MAX_U24 {
                return Err(PduParseErr::InvalidValue {
                    field: "transmitting_party_address_ssi",
                    value: ssi,
                });
            }
        }
        if let Some(extension) = self.transmitting_party_extension {
            if extension > MAX_U24 {
                return Err(PduParseErr::InvalidValue {
                    field: "transmitting_party_extension",
                    value: extension,
                });
            }
        }

        // EN 300 392-2 table 14.19 note 2 and table 14.82: TPTI controls
        // which address fields follow; values 0 and 3 are reserved.
        match self.transmitting_party_type_identifier {
            None => {
                if self.transmitting_party_address_ssi.is_some() {
                    return Err(PduParseErr::Inconsistency {
                        field: "transmitting_party_address_ssi",
                        reason: "not valid without transmitting party type identifier",
                    });
                }
                if self.transmitting_party_extension.is_some() {
                    return Err(PduParseErr::Inconsistency {
                        field: "transmitting_party_extension",
                        reason: "not valid without transmitting party type identifier",
                    });
                }
            }
            Some(TPTI_SSI) => {
                if self.transmitting_party_address_ssi.is_none() {
                    return Err(PduParseErr::FieldNotPresent {
                        field: Some("transmitting_party_address_ssi"),
                    });
                }
                if self.transmitting_party_extension.is_some() {
                    return Err(PduParseErr::Inconsistency {
                        field: "transmitting_party_extension",
                        reason: "not valid for transmitting party type identifier",
                    });
                }
            }
            Some(TPTI_TSI) => {
                if self.transmitting_party_address_ssi.is_none() {
                    return Err(PduParseErr::FieldNotPresent {
                        field: Some("transmitting_party_address_ssi"),
                    });
                }
                if self.transmitting_party_extension.is_none() {
                    return Err(PduParseErr::FieldNotPresent {
                        field: Some("transmitting_party_extension"),
                    });
                }
            }
            Some(value) => {
                return Err(PduParseErr::InvalidValue {
                    field: "transmitting_party_type_identifier",
                    value,
                });
            }
        }

        Ok(())
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(5, "pdu_type")?;
        expect_pdu_type!(pdu_type, CmcePduTypeDl::DTxInterrupt)?;

        // Type1
        let call_identifier = buffer.read_field(14, "call_identifier")? as u16;
        // Type1
        let transmission_grant = buffer.read_field(2, "transmission_grant")? as u8;
        // Type1
        let transmission_request_permission = buffer.read_field(1, "transmission_request_permission")? != 0;
        // Type1
        let encryption_control = buffer.read_field(1, "encryption_control")? != 0;
        // Type1
        let reserved = buffer.read_field(1, "reserved")? != 0;
        if reserved {
            return Err(PduParseErr::InvalidValue {
                field: "reserved",
                value: 1,
            });
        }

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type2
        let notification_indicator = typed::parse_type2_generic(obit, buffer, 6, "notification_indicator")?;
        // Type2
        let transmitting_party_type_identifier = typed::parse_type2_generic(obit, buffer, 2, "transmitting_party_type_identifier")?;
        let (transmitting_party_address_ssi, transmitting_party_extension) = match transmitting_party_type_identifier {
            None => (None, None),
            Some(TPTI_SSI) => (Some(buffer.read_field(24, "transmitting_party_address_ssi")?), None),
            Some(TPTI_TSI) => (
                Some(buffer.read_field(24, "transmitting_party_address_ssi")?),
                Some(buffer.read_field(24, "transmitting_party_extension")?),
            ),
            Some(value) => {
                return Err(PduParseErr::InvalidValue {
                    field: "transmitting_party_type_identifier",
                    value,
                });
            }
        };

        // Type3
        let external_subscriber_number = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::ExtSubscriberNum)?;

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

        Ok(DTxInterrupt {
            call_identifier,
            transmission_grant,
            transmission_request_permission,
            encryption_control,
            reserved,
            notification_indicator,
            transmitting_party_type_identifier,
            transmitting_party_address_ssi,
            transmitting_party_extension,
            external_subscriber_number,
            facility,
            dm_ms_address,
            proprietary,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // PDU Type
        buffer.write_bits(CmcePduTypeDl::DTxInterrupt.into_raw(), 5);
        // Type1
        buffer.write_bits(self.call_identifier as u64, 14);
        // Type1
        buffer.write_bits(self.transmission_grant as u64, 2);
        // Type1
        buffer.write_bits(self.transmission_request_permission as u64, 1);
        // Type1
        buffer.write_bits(self.encryption_control as u64, 1);
        // Type1
        buffer.write_bits(self.reserved as u64, 1);

        // Check if any optional field present and place o-bit
        let obit = self.notification_indicator.is_some()
            || self.transmitting_party_type_identifier.is_some()
            || self.external_subscriber_number.is_some()
            || self.facility.is_some()
            || self.dm_ms_address.is_some()
            || self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2
        typed::write_type2_generic(obit, buffer, self.notification_indicator, 6);

        // Type2
        typed::write_type2_generic(obit, buffer, self.transmitting_party_type_identifier, 2);

        if let Some(value) = self.transmitting_party_address_ssi {
            buffer.write_bits(value, 24);
        }
        if let Some(value) = self.transmitting_party_extension {
            buffer.write_bits(value, 24);
        }

        // Type3
        typed::write_type3_generic(obit, buffer, &self.external_subscriber_number, CmceType3ElemId::ExtSubscriberNum)?;

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

impl fmt::Display for DTxInterrupt {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DTxInterrupt {{ call_identifier: {:?} transmission_grant: {:?} transmission_request_permission: {:?} encryption_control: {:?} reserved: {:?} notification_indicator: {:?} transmitting_party_type_identifier: {:?} transmitting_party_address_ssi: {:?} transmitting_party_extension: {:?} external_subscriber_number: {:?} facility: {:?} dm_ms_address: {:?} proprietary: {:?} }}",
            self.call_identifier,
            self.transmission_grant,
            self.transmission_request_permission,
            self.encryption_control,
            self.reserved,
            self.notification_indicator,
            self.transmitting_party_type_identifier,
            self.transmitting_party_address_ssi,
            self.transmitting_party_extension,
            self.external_subscriber_number,
            self.facility,
            self.dm_ms_address,
            self.proprietary,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_core::BitBuffer;

    fn base_d_tx_interrupt() -> DTxInterrupt {
        DTxInterrupt {
            call_identifier: 0x1234,
            transmission_grant: 3,
            transmission_request_permission: false,
            encryption_control: false,
            reserved: false,
            notification_indicator: None,
            transmitting_party_type_identifier: Some(TPTI_SSI),
            transmitting_party_address_ssi: Some(0x00aa_bbcc),
            transmitting_party_extension: None,
            external_subscriber_number: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        }
    }

    fn round_trip(pdu: &DTxInterrupt) -> (DTxInterrupt, usize) {
        let mut buf = BitBuffer::new_autoexpand(128);
        pdu.to_bitbuf(&mut buf).expect("serialize D-TX INTERRUPT");
        let len = buf.get_len();
        buf.seek(0);
        let decoded = DTxInterrupt::from_bitbuf(&mut buf).expect("parse D-TX INTERRUPT");
        (decoded, len)
    }

    #[test]
    fn d_tx_interrupt_roundtrips_tpti_ssi_without_address_pbits() {
        let pdu = base_d_tx_interrupt();
        let (decoded, len) = round_trip(&pdu);

        assert_eq!(len, 54);
        assert_eq!(decoded.call_identifier, 0x1234);
        assert_eq!(decoded.transmission_grant, 3);
        assert_eq!(decoded.transmitting_party_type_identifier, Some(TPTI_SSI));
        assert_eq!(decoded.transmitting_party_address_ssi, Some(0x00aa_bbcc));
        assert_eq!(decoded.transmitting_party_extension, None);
    }

    #[test]
    fn d_tx_interrupt_roundtrips_tpti_tsi_without_address_pbits() {
        let pdu = DTxInterrupt {
            transmitting_party_type_identifier: Some(TPTI_TSI),
            transmitting_party_address_ssi: Some(0x0012_3456),
            transmitting_party_extension: Some(0x0076_5432),
            ..base_d_tx_interrupt()
        };
        let (decoded, len) = round_trip(&pdu);

        assert_eq!(len, 78);
        assert_eq!(decoded.transmitting_party_type_identifier, Some(TPTI_TSI));
        assert_eq!(decoded.transmitting_party_address_ssi, Some(0x0012_3456));
        assert_eq!(decoded.transmitting_party_extension, Some(0x0076_5432));
    }

    #[test]
    fn d_tx_interrupt_parser_rejects_reserved_bit_set() {
        let mut buf = BitBuffer::new_autoexpand(32);
        buf.write_bits(CmcePduTypeDl::DTxInterrupt.into_raw(), 5);
        buf.write_bits(1, 14);
        buf.write_bits(0, 2);
        buf.write_bits(0, 1);
        buf.write_bits(0, 1);
        buf.write_bits(1, 1);
        buf.write_bits(0, 1);
        buf.seek(0);

        assert_eq!(
            DTxInterrupt::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "reserved",
                value: 1,
            }
        );
    }

    #[test]
    fn d_tx_interrupt_rejects_reserved_bit_on_serialize() {
        let pdu = DTxInterrupt {
            reserved: true,
            ..base_d_tx_interrupt()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "reserved",
                value: 1,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_tx_interrupt_rejects_reserved_tpti_values() {
        for value in [0, 3] {
            let pdu = DTxInterrupt {
                transmitting_party_type_identifier: Some(value),
                transmitting_party_address_ssi: None,
                transmitting_party_extension: None,
                ..base_d_tx_interrupt()
            };
            let mut buf = BitBuffer::new_autoexpand(32);

            assert_eq!(
                pdu.to_bitbuf(&mut buf),
                Err(PduParseErr::InvalidValue {
                    field: "transmitting_party_type_identifier",
                    value,
                })
            );
            assert_eq!(buf.get_len(), 0);
        }
    }

    #[test]
    fn d_tx_interrupt_requires_address_for_tpti_ssi() {
        let pdu = DTxInterrupt {
            transmitting_party_address_ssi: None,
            ..base_d_tx_interrupt()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::FieldNotPresent {
                field: Some("transmitting_party_address_ssi"),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_tx_interrupt_requires_extension_for_tpti_tsi() {
        let pdu = DTxInterrupt {
            transmitting_party_type_identifier: Some(TPTI_TSI),
            transmitting_party_address_ssi: Some(0x00aa_bbcc),
            transmitting_party_extension: None,
            ..base_d_tx_interrupt()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::FieldNotPresent {
                field: Some("transmitting_party_extension"),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_tx_interrupt_rejects_address_fields_without_tpti() {
        for pdu in [
            DTxInterrupt {
                transmitting_party_type_identifier: None,
                transmitting_party_address_ssi: Some(0x00aa_bbcc),
                transmitting_party_extension: None,
                ..base_d_tx_interrupt()
            },
            DTxInterrupt {
                transmitting_party_type_identifier: None,
                transmitting_party_address_ssi: None,
                transmitting_party_extension: Some(0x0012_3456),
                ..base_d_tx_interrupt()
            },
        ] {
            let mut buf = BitBuffer::new_autoexpand(32);
            assert!(matches!(
                pdu.to_bitbuf(&mut buf),
                Err(PduParseErr::Inconsistency {
                    field: "transmitting_party_address_ssi" | "transmitting_party_extension",
                    reason: "not valid without transmitting party type identifier",
                })
            ));
            assert_eq!(buf.get_len(), 0);
        }
    }

    #[test]
    fn d_tx_interrupt_rejects_tpti_tsi_without_extension_on_parse() {
        let mut buf = BitBuffer::new_autoexpand(64);
        buf.write_bits(CmcePduTypeDl::DTxInterrupt.into_raw(), 5);
        buf.write_bits(1, 14);
        buf.write_bits(0, 2);
        buf.write_bits(0, 1);
        buf.write_bits(0, 1);
        buf.write_bits(0, 1);
        buf.write_bits(1, 1);
        buf.write_bits(0, 1);
        buf.write_bits(1, 1);
        buf.write_bits(TPTI_TSI, 2);
        buf.write_bits(0x00aa_bbcc, 24);
        buf.seek(0);

        assert_eq!(
            DTxInterrupt::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::BufferEnded {
                field: Some("transmitting_party_extension"),
            }
        );
    }
}
