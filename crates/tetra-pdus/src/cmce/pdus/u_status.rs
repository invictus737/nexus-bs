// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use crate::cmce::enums::pre_coded_status::PreCodedStatus;
use crate::cmce::enums::{cmce_pdu_type_ul::CmcePduTypeUl, party_type_identifier::PartyTypeIdentifier, type3_elem_id::CmceType3ElemId};
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

/// Representation of the U-STATUS PDU (Clause 14.7.2.7).
/// This PDU shall be used for sending a pre-coded status message.
/// Response expected: -
/// Response to: -

// note 1: This information element is used by SS-AS, refer to ETSI EN 300 392-12-8 [14].
// note 2: Shall be conditional on the value of Called Party Type Identifier (CPTI): CPTI = 0 → Called Party SNA (see ETS 300 392-12-7 [13]); CPTI = 1 → Called Party SSI; CPTI = 2 → Called Party SSI + Called Party Extension.
#[derive(Debug)]
pub struct UStatus {
    /// Type1, 4 bits, See note 1,
    pub area_selection: u8,
    /// Type1, 2 bits, Called party type identifier
    pub called_party_type_identifier: PartyTypeIdentifier,
    /// Conditional 8 bits, See note 2, condition: called_party_type_identifier == 0
    pub called_party_short_number_address: Option<u64>,
    /// Conditional 24 bits, See note 2, condition: called_party_type_identifier == 1 || called_party_type_identifier == 2
    pub called_party_ssi: Option<u64>,
    /// Conditional 24 bits, See note 2, condition: called_party_type_identifier == 2
    pub called_party_extension: Option<u64>,
    /// Type1, 16 bits, Pre-coded status
    pub pre_coded_status: PreCodedStatus,
    /// Type3, External subscriber number
    pub external_subscriber_number: Option<Type3FieldGeneric>,
    /// Type3, DM-MS address
    pub dm_ms_address: Option<Type3FieldGeneric>,
}

impl UStatus {
    fn validate_for_serialization(&self) -> Result<(), PduParseErr> {
        fn require_8_bit(field: &'static str, value: u64) -> Result<(), PduParseErr> {
            if value <= 0xFF {
                Ok(())
            } else {
                Err(PduParseErr::InvalidValue { field, value })
            }
        }

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
                    reason: "not valid for called party type identifier",
                })
            } else {
                Ok(())
            }
        }

        if self.area_selection > 0x0F {
            return Err(PduParseErr::InvalidValue {
                field: "area_selection",
                value: self.area_selection as u64,
            });
        }

        match self.called_party_type_identifier {
            PartyTypeIdentifier::Sna => {
                let sna = self.called_party_short_number_address.ok_or(PduParseErr::FieldNotPresent {
                    field: Some("called_party_short_number_address"),
                })?;
                require_8_bit("called_party_short_number_address", sna)?;
                require_absent("called_party_ssi", self.called_party_ssi.is_some())?;
                require_absent("called_party_extension", self.called_party_extension.is_some())
            }
            PartyTypeIdentifier::Ssi => {
                let ssi = self.called_party_ssi.ok_or(PduParseErr::FieldNotPresent {
                    field: Some("called_party_ssi"),
                })?;
                require_24_bit("called_party_ssi", ssi)?;
                require_absent(
                    "called_party_short_number_address",
                    self.called_party_short_number_address.is_some(),
                )?;
                require_absent("called_party_extension", self.called_party_extension.is_some())
            }
            PartyTypeIdentifier::Tsi => {
                let ssi = self.called_party_ssi.ok_or(PduParseErr::FieldNotPresent {
                    field: Some("called_party_ssi"),
                })?;
                let extension = self.called_party_extension.ok_or(PduParseErr::FieldNotPresent {
                    field: Some("called_party_extension"),
                })?;
                require_24_bit("called_party_ssi", ssi)?;
                require_24_bit("called_party_extension", extension)?;
                require_absent(
                    "called_party_short_number_address",
                    self.called_party_short_number_address.is_some(),
                )
            }
            PartyTypeIdentifier::Reserved => Err(PduParseErr::InvalidValue {
                field: "called_party_type_identifier",
                value: self.called_party_type_identifier.into_raw(),
            }),
        }
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(5, "pdu_type")?;
        expect_pdu_type!(pdu_type, CmcePduTypeUl::UStatus)?;

        // Type1
        let area_selection = buffer.read_field(4, "area_selection")? as u8;
        // Type1
        let cpti_raw = buffer.read_field(2, "called_party_type_identifier")?;
        let called_party_type_identifier = PartyTypeIdentifier::try_from(cpti_raw).map_err(|_| PduParseErr::InvalidValue {
            field: "called_party_type_identifier",
            value: cpti_raw,
        })?;
        if called_party_type_identifier == PartyTypeIdentifier::Reserved {
            return Err(PduParseErr::InvalidValue {
                field: "called_party_type_identifier",
                value: cpti_raw,
            });
        }
        // Conditional
        let called_party_short_number_address = if called_party_type_identifier == PartyTypeIdentifier::Sna {
            Some(buffer.read_field(8, "called_party_short_number_address")?)
        } else {
            None
        };
        // Conditional
        let called_party_ssi =
            if called_party_type_identifier == PartyTypeIdentifier::Ssi || called_party_type_identifier == PartyTypeIdentifier::Tsi {
                Some(buffer.read_field(24, "called_party_ssi")?)
            } else {
                None
            };
        // Conditional
        let called_party_extension = if called_party_type_identifier == PartyTypeIdentifier::Tsi {
            Some(buffer.read_field(24, "called_party_extension")?)
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

        Ok(UStatus {
            area_selection,
            called_party_type_identifier,
            called_party_short_number_address,
            called_party_ssi,
            called_party_extension,
            pre_coded_status,
            external_subscriber_number,
            dm_ms_address,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate_for_serialization()?;

        // PDU Type
        buffer.write_bits(CmcePduTypeUl::UStatus.into_raw(), 5);
        // Type1
        buffer.write_bits(self.area_selection as u64, 4);
        // Type1
        buffer.write_bits(self.called_party_type_identifier.into_raw(), 2);
        // Conditional
        if let Some(ref value) = self.called_party_short_number_address {
            buffer.write_bits(*value, 8);
        }
        // Conditional
        if let Some(ref value) = self.called_party_ssi {
            buffer.write_bits(*value, 24);
        }
        // Conditional
        if let Some(ref value) = self.called_party_extension {
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

impl fmt::Display for UStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "UStatus {{ area_selection: {:?} called_party_type_identifier: {:?} called_party_short_number_address: {:?} called_party_ssi: {:?} called_party_extension: {:?} pre_coded_status: {:?} external_subscriber_number: {:?} dm_ms_address: {:?} }}",
            self.area_selection,
            self.called_party_type_identifier,
            self.called_party_short_number_address,
            self.called_party_ssi,
            self.called_party_extension,
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

    fn base_u_status() -> UStatus {
        UStatus {
            area_selection: 0,
            called_party_type_identifier: PartyTypeIdentifier::Ssi,
            called_party_short_number_address: None,
            called_party_ssi: Some(0x00AA_BBCC),
            called_party_extension: None,
            pre_coded_status: PreCodedStatus::Emergency,
            external_subscriber_number: None,
            dm_ms_address: None,
        }
    }

    #[test]
    fn u_status_rejects_reserved_called_party_type() {
        let pdu = UStatus {
            called_party_type_identifier: PartyTypeIdentifier::Reserved,
            ..base_u_status()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "called_party_type_identifier",
                value: PartyTypeIdentifier::Reserved.into_raw(),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn u_status_parser_rejects_reserved_called_party_type() {
        let mut buf = BitBuffer::new_autoexpand(32);
        buf.write_bits(CmcePduTypeUl::UStatus.into_raw(), 5);
        buf.write_bits(0, 4);
        buf.write_bits(PartyTypeIdentifier::Reserved.into_raw(), 2);
        buf.write_bits(0, 16);
        buf.write_bits(0, 1);
        buf.seek(0);

        assert_eq!(
            UStatus::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "called_party_type_identifier",
                value: PartyTypeIdentifier::Reserved.into_raw(),
            }
        );
    }

    #[test]
    fn u_status_requires_sna_for_cpti_sna() {
        let pdu = UStatus {
            called_party_type_identifier: PartyTypeIdentifier::Sna,
            called_party_ssi: None,
            ..base_u_status()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::FieldNotPresent {
                field: Some("called_party_short_number_address"),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn u_status_rejects_ssi_for_cpti_sna() {
        let pdu = UStatus {
            called_party_type_identifier: PartyTypeIdentifier::Sna,
            called_party_short_number_address: Some(7),
            ..base_u_status()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::Inconsistency {
                field: "called_party_ssi",
                reason: "not valid for called party type identifier",
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn u_status_requires_ssi_for_cpti_ssi() {
        let pdu = UStatus {
            called_party_ssi: None,
            ..base_u_status()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::FieldNotPresent {
                field: Some("called_party_ssi"),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn u_status_rejects_extension_for_cpti_ssi() {
        let pdu = UStatus {
            called_party_extension: Some(1),
            ..base_u_status()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::Inconsistency {
                field: "called_party_extension",
                reason: "not valid for called party type identifier",
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn u_status_requires_extension_for_cpti_tsi() {
        let pdu = UStatus {
            called_party_type_identifier: PartyTypeIdentifier::Tsi,
            called_party_extension: None,
            ..base_u_status()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::FieldNotPresent {
                field: Some("called_party_extension"),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn u_status_rejects_area_selection_above_4_bits() {
        let pdu = UStatus {
            area_selection: 0x10,
            ..base_u_status()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "area_selection",
                value: 0x10,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn u_status_rejects_ssi_above_24_bits() {
        let pdu = UStatus {
            called_party_ssi: Some(0x0100_0000),
            ..base_u_status()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "called_party_ssi",
                value: 0x0100_0000,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn u_status_round_trips_all_ones_network_specific_status() {
        let pdu = UStatus {
            called_party_ssi: Some(0x00FF_FFFF),
            pre_coded_status: PreCodedStatus::NetworkUserSpecific(0xFFFF),
            ..base_u_status()
        };
        let mut buf = BitBuffer::new_autoexpand(80);

        pdu.to_bitbuf(&mut buf).expect("U-STATUS should serialize");
        buf.seek(0);
        let parsed = UStatus::from_bitbuf(&mut buf).expect("U-STATUS should parse");

        // EN 300 392-2 table 14.27 carries Called Party SSI as a 24-bit
        // address and Pre-coded status as a separate 16-bit field. The
        // all-ones values in those fields must not be conflated.
        assert_eq!(parsed.called_party_ssi, Some(0x00FF_FFFF));
        assert_eq!(parsed.pre_coded_status, PreCodedStatus::NetworkUserSpecific(0xFFFF));
    }
}
