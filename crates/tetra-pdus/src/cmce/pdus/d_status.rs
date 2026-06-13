// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use crate::cmce::enums::pre_coded_status::PreCodedStatus;
use crate::cmce::enums::{cmce_pdu_type_dl::CmcePduTypeDl, party_type_identifier::PartyTypeIdentifier, type3_elem_id::CmceType3ElemId};
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

/// Representation of the D-STATUS PDU (Clause 14.7.1.11).
/// This PDU shall be the PDU for receiving a pre-coded status message.
/// Response expected: None
/// Response to: None

// Note 1: Shall be conditional on the value of Calling Party Type Identifier (CPTI): CPTI = 1 → include Calling Party SSI only; CPTI = 2 → include both SSI and Calling Party Extension.
#[derive(Debug)]
pub struct DStatus {
    /// Type1, 2 bits, Calling party type identifier
    pub calling_party_type_identifier: PartyTypeIdentifier,
    /// Conditional 24 bits, Calling party address SSI condition: calling_party_type_identifier == 1 || calling_party_type_identifier == 2
    pub calling_party_address_ssi: Option<u64>,
    /// Conditional 24 bits, Calling party extension condition: calling_party_type_identifier == 2
    pub calling_party_extension: Option<u64>,
    /// Type1, 16 bits, Pre-coded status
    pub pre_coded_status: PreCodedStatus,
    /// Type3, External subscriber number
    pub external_subscriber_number: Option<Type3FieldGeneric>,
    /// Type3, DM-MS address
    pub dm_ms_address: Option<Type3FieldGeneric>,
}

impl DStatus {
    fn validate_for_serialization(&self) -> Result<(), PduParseErr> {
        fn require_24_bit(field: &'static str, value: u64) -> Result<(), PduParseErr> {
            if value <= 0x00FF_FFFF {
                Ok(())
            } else {
                Err(PduParseErr::InvalidValue { field, value })
            }
        }

        fn require_absent(field: &'static str, present: bool) -> Result<(), PduParseErr> {
            if present {
                Err(PduParseErr::Inconsistency {
                    field,
                    reason: "not valid for calling party type identifier",
                })
            } else {
                Ok(())
            }
        }

        match self.calling_party_type_identifier {
            PartyTypeIdentifier::Ssi => {
                let ssi = self.calling_party_address_ssi.ok_or(PduParseErr::FieldNotPresent {
                    field: Some("calling_party_address_ssi"),
                })?;
                require_24_bit("calling_party_address_ssi", ssi)?;
                require_absent("calling_party_extension", self.calling_party_extension.is_some())
            }
            PartyTypeIdentifier::Tsi => {
                let ssi = self.calling_party_address_ssi.ok_or(PduParseErr::FieldNotPresent {
                    field: Some("calling_party_address_ssi"),
                })?;
                let extension = self.calling_party_extension.ok_or(PduParseErr::FieldNotPresent {
                    field: Some("calling_party_extension"),
                })?;
                require_24_bit("calling_party_address_ssi", ssi)?;
                require_24_bit("calling_party_extension", extension)
            }
            PartyTypeIdentifier::Sna | PartyTypeIdentifier::Reserved => Err(PduParseErr::InvalidValue {
                field: "calling_party_type_identifier",
                value: self.calling_party_type_identifier.into_raw(),
            }),
        }
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(5, "pdu_type")?;
        expect_pdu_type!(pdu_type, CmcePduTypeDl::DStatus)?;

        // Type1
        let cpti_raw = buffer.read_field(2, "calling_party_type_identifier")?;
        let calling_party_type_identifier = PartyTypeIdentifier::try_from(cpti_raw).map_err(|_| PduParseErr::InvalidValue {
            field: "calling_party_type_identifier",
            value: cpti_raw,
        })?;
        if matches!(
            calling_party_type_identifier,
            PartyTypeIdentifier::Sna | PartyTypeIdentifier::Reserved
        ) {
            return Err(PduParseErr::InvalidValue {
                field: "calling_party_type_identifier",
                value: cpti_raw,
            });
        }
        // Conditional
        let calling_party_address_ssi =
            if calling_party_type_identifier == PartyTypeIdentifier::Ssi || calling_party_type_identifier == PartyTypeIdentifier::Tsi {
                Some(buffer.read_field(24, "calling_party_address_ssi")?)
            } else {
                None
            };
        // Conditional
        let calling_party_extension = if calling_party_type_identifier == PartyTypeIdentifier::Tsi {
            Some(buffer.read_field(24, "calling_party_extension")?)
        } else {
            None
        };
        // Type1
        let val = buffer.read_field(16, "pre_coded_status")? as u16;
        let pre_coded_status = PreCodedStatus::from(val);

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type3
        let external_subscriber_number = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::ExtSubscriberNum)?;

        // Type3
        let dm_ms_address = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::DmMsAddr)?;

        // Read trailing mbit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        Ok(DStatus {
            calling_party_type_identifier,
            calling_party_address_ssi,
            calling_party_extension,
            pre_coded_status,
            external_subscriber_number,
            dm_ms_address,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate_for_serialization()?;

        // PDU Type
        buffer.write_bits(CmcePduTypeDl::DStatus.into_raw(), 5);
        // Type1
        buffer.write_bits(self.calling_party_type_identifier.into_raw(), 2);
        // Conditional
        if let Some(ref value) = self.calling_party_address_ssi {
            buffer.write_bits(*value, 24);
        }
        // Conditional
        if let Some(ref value) = self.calling_party_extension {
            buffer.write_bits(*value, 24);
        }
        // Type1
        buffer.write_bits(self.pre_coded_status.into_raw().into(), 16);

        // Check if any optional field present and place o-bit
        let obit = self.external_subscriber_number.is_some() || self.dm_ms_address.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type3
        typed::write_type3_generic(obit, buffer, &self.external_subscriber_number, CmceType3ElemId::ExtSubscriberNum)?;

        // Type3
        typed::write_type3_generic(obit, buffer, &self.dm_ms_address, CmceType3ElemId::DmMsAddr)?;

        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for DStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DStatus {{ calling_party_type_identifier: {:?} calling_party_address_ssi: {:?} calling_party_extension: {:?} pre_coded_status: {:?} external_subscriber_number: {:?} dm_ms_address: {:?} }}",
            self.calling_party_type_identifier,
            self.calling_party_address_ssi,
            self.calling_party_extension,
            self.pre_coded_status,
            self.external_subscriber_number,
            self.dm_ms_address,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_core::BitBuffer;

    fn base_d_status() -> DStatus {
        DStatus {
            calling_party_type_identifier: PartyTypeIdentifier::Ssi,
            calling_party_address_ssi: Some(0x00AA_BBCC),
            calling_party_extension: None,
            pre_coded_status: PreCodedStatus::Emergency,
            external_subscriber_number: None,
            dm_ms_address: None,
        }
    }

    #[test]
    fn d_status_rejects_reserved_calling_party_type() {
        let pdu = DStatus {
            calling_party_type_identifier: PartyTypeIdentifier::Reserved,
            ..base_d_status()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "calling_party_type_identifier",
                value: PartyTypeIdentifier::Reserved.into_raw(),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_status_rejects_sna_calling_party_type() {
        let pdu = DStatus {
            calling_party_type_identifier: PartyTypeIdentifier::Sna,
            calling_party_address_ssi: None,
            calling_party_extension: None,
            ..base_d_status()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "calling_party_type_identifier",
                value: PartyTypeIdentifier::Sna.into_raw(),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_status_parser_rejects_reserved_calling_party_type() {
        let mut buf = BitBuffer::new_autoexpand(32);
        buf.write_bits(CmcePduTypeDl::DStatus.into_raw(), 5);
        buf.write_bits(PartyTypeIdentifier::Reserved.into_raw(), 2);
        buf.write_bits(0, 16);
        buf.write_bits(0, 1);
        buf.seek(0);

        assert_eq!(
            DStatus::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "calling_party_type_identifier",
                value: PartyTypeIdentifier::Reserved.into_raw(),
            }
        );
    }

    #[test]
    fn d_status_parser_rejects_sna_calling_party_type() {
        let mut buf = BitBuffer::new_autoexpand(32);
        buf.write_bits(CmcePduTypeDl::DStatus.into_raw(), 5);
        buf.write_bits(PartyTypeIdentifier::Sna.into_raw(), 2);
        buf.write_bits(0, 16);
        buf.write_bits(0, 1);
        buf.seek(0);

        assert_eq!(
            DStatus::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "calling_party_type_identifier",
                value: PartyTypeIdentifier::Sna.into_raw(),
            }
        );
    }

    #[test]
    fn d_status_requires_ssi_for_cpti_ssi() {
        let pdu = DStatus {
            calling_party_address_ssi: None,
            ..base_d_status()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::FieldNotPresent {
                field: Some("calling_party_address_ssi"),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_status_rejects_extension_for_cpti_ssi() {
        let pdu = DStatus {
            calling_party_extension: Some(1),
            ..base_d_status()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::Inconsistency {
                field: "calling_party_extension",
                reason: "not valid for calling party type identifier",
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_status_requires_extension_for_cpti_tsi() {
        let pdu = DStatus {
            calling_party_type_identifier: PartyTypeIdentifier::Tsi,
            calling_party_extension: None,
            ..base_d_status()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::FieldNotPresent {
                field: Some("calling_party_extension"),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_status_rejects_ssi_above_24_bits() {
        let pdu = DStatus {
            calling_party_address_ssi: Some(0x0100_0000),
            ..base_d_status()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "calling_party_address_ssi",
                value: 0x0100_0000,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_status_round_trips_all_ones_network_specific_status() {
        let pdu = DStatus {
            calling_party_address_ssi: Some(0x00FF_FFFF),
            pre_coded_status: PreCodedStatus::NetworkUserSpecific(0xFFFF),
            ..base_d_status()
        };
        let mut buf = BitBuffer::new_autoexpand(80);

        pdu.to_bitbuf(&mut buf).expect("D-STATUS should serialize");
        buf.seek(0);
        let parsed = DStatus::from_bitbuf(&mut buf).expect("D-STATUS should parse");

        // EN 300 392-2 table 14.14 carries Calling Party SSI as a 24-bit
        // address and Pre-coded status as a separate 16-bit field. The
        // all-ones values in those fields must not be conflated.
        assert_eq!(parsed.calling_party_address_ssi, Some(0x00FF_FFFF));
        assert_eq!(parsed.pre_coded_status, PreCodedStatus::NetworkUserSpecific(0xFFFF));
    }
}
