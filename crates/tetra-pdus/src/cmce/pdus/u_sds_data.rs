// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use crate::cmce::enums::{cmce_pdu_type_ul::CmcePduTypeUl, party_type_identifier::PartyTypeIdentifier, type3_elem_id::CmceType3ElemId};
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};
use tetra_saps::control::enums::sds_user_data::SdsUserData;

use super::sds_user_data_codec::{read_sds_type4_user_data, write_sds_user_data};

/// Representation of the U-SDS-DATA PDU (Clause 14.7.2.8).
/// This PDU shall be for sending user defined SDS data.
/// Response expected: -
/// Response to: -

// note 1: This information element is used by SS-AS, refer to ETSI EN 300 392-12-8 [14].
// note 2: Shall be conditional on the value of Called Party Type Identifier (CPTI): CPTI=0 → Called Party SNA; CPTI=1 → Called Party SSI; CPTI=2 → Called Party SSI+Called Party Extension.
// note 3: Shall be conditional on the value of Short Data Type Identifier (SDTI): SDTI=0 → User Defined Data-1; SDTI=1 → User Defined Data-2; SDTI=2 → User Defined Data-3; SDTI=3 → Length indicator + User Defined Data-4.
// note 4: Any combination of address and user defined data type is allowed; recommended to choose the shortest appropriate user defined data type to fit one sub-slot when possible.
// note 5: The length of User Defined Data-4 is between 0 and 2 047 bits (longest recommended: 1 017 bits on basic link with Short SSI and FCS on π/4-DQPSK).
// Clause 14.8.52 defines those bits as an 8-bit protocol identifier followed by 0..2039 protocol-dependent bits.
#[derive(Debug)]
pub struct USdsData {
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
    /// Either type1, type2, type3 or type4 user data field.
    pub user_defined_data: SdsUserData,
    /// Type3, External subscriber number
    pub external_subscriber_number: Option<Type3FieldGeneric>,
    /// Type3, DM-MS address
    pub dm_ms_address: Option<Type3FieldGeneric>,
}

impl USdsData {
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
        expect_pdu_type!(pdu_type, CmcePduTypeUl::USdsData)?;

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
        let short_data_type_identifier = buffer.read_field(2, "short_data_type_identifier")? as u8;
        let user_defined_data = match short_data_type_identifier {
            0 => SdsUserData::Type1(buffer.read_field(16, "user_defined_data_1")? as u16),
            1 => SdsUserData::Type2(buffer.read_field(32, "user_defined_data_2")? as u32),
            2 => SdsUserData::Type3(buffer.read_field(64, "user_defined_data_3")?),
            3 => {
                let len_bits = buffer.read_field(11, "length_indicator")? as u16;
                read_sds_type4_user_data(buffer, len_bits)?
            }
            _ => unreachable!(),
        };

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

        Ok(USdsData {
            area_selection,
            called_party_type_identifier,
            called_party_short_number_address,
            called_party_ssi,
            called_party_extension,
            user_defined_data,
            external_subscriber_number,
            dm_ms_address,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate_for_serialization()?;

        // PDU Type
        buffer.write_bits(CmcePduTypeUl::USdsData.into_raw(), 5);
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

        // Type1 + conditional user data. EN 300 392-2 14.7.2.8 / 14.8.38
        // bounds Type 4 user data with an 11-bit length indicator.
        write_sds_user_data(buffer, &self.user_defined_data)?;

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

impl fmt::Display for USdsData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "USdsData {{ area_selection: {:?} called_party_type_identifier: {:?} called_party_short_number_address: {:?} called_party_ssi: {:?} called_party_extension: {:?} user_defined_data: {:?} external_subscriber_number: {:?} dm_ms_address: {:?} }}",
            self.area_selection,
            self.called_party_type_identifier,
            self.called_party_short_number_address,
            self.called_party_ssi,
            self.called_party_extension,
            self.user_defined_data,
            self.external_subscriber_number,
            self.dm_ms_address,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_core::BitBuffer;

    fn round_trip(pdu: &USdsData) -> USdsData {
        let mut buf = BitBuffer::new_autoexpand(256);
        pdu.to_bitbuf(&mut buf).expect("serialize failed");
        buf.seek(0);
        USdsData::from_bitbuf(&mut buf).expect("parse failed")
    }

    fn base_u_sds_data() -> USdsData {
        USdsData {
            area_selection: 0,
            called_party_type_identifier: PartyTypeIdentifier::Ssi,
            called_party_short_number_address: None,
            called_party_ssi: Some(0x00AA_BBCC),
            called_party_extension: None,
            user_defined_data: SdsUserData::Type1(0xCAFE),
            external_subscriber_number: None,
            dm_ms_address: None,
        }
    }

    #[test]
    fn test_u_sds_data_sdti0_cpti1() {
        let pdu = USdsData {
            area_selection: 0,
            called_party_type_identifier: PartyTypeIdentifier::Ssi,
            called_party_short_number_address: None,
            called_party_ssi: Some(1000001),
            called_party_extension: None,
            user_defined_data: SdsUserData::Type1(0xCAFE),
            external_subscriber_number: None,
            dm_ms_address: None,
        };
        let parsed = round_trip(&pdu);
        assert_eq!(parsed.area_selection, 0);
        assert_eq!(parsed.called_party_type_identifier, PartyTypeIdentifier::Ssi);
        assert_eq!(parsed.called_party_ssi, Some(1000001));
        assert_eq!(parsed.called_party_extension, None);
        assert_eq!(parsed.user_defined_data, SdsUserData::Type1(0xCAFE));
    }

    #[test]
    fn test_u_sds_data_sdti3_cpti1() {
        let payload = vec![0x01, 0x02, 0x03];
        let pdu = USdsData {
            area_selection: 5,
            called_party_type_identifier: PartyTypeIdentifier::Ssi,
            called_party_short_number_address: None,
            called_party_ssi: Some(2000002),
            called_party_extension: None,
            user_defined_data: SdsUserData::Type4(24, payload.clone()),
            external_subscriber_number: None,
            dm_ms_address: None,
        };
        let parsed = round_trip(&pdu);
        assert_eq!(parsed.area_selection, 5);
        assert_eq!(parsed.called_party_ssi, Some(2000002));
        assert_eq!(parsed.user_defined_data, SdsUserData::Type4(24, payload));
    }

    #[test]
    fn test_u_sds_data_type4_rejects_missing_protocol_identifier() {
        let mut pdu = base_u_sds_data();
        pdu.user_defined_data = SdsUserData::Type4(0, Vec::new());
        let mut buf = BitBuffer::new_autoexpand(64);

        // EN 300 392-2 clause 14.8.52: generated Type4 SDS must include the
        // 8-bit protocol identifier before the protocol-dependent payload.
        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "length_indicator",
                value: 0,
            })
        );
    }

    #[test]
    fn test_u_sds_data_type4_rejects_sub_pid_payload() {
        let payload = vec![0b1010_1000];
        let mut pdu = base_u_sds_data();
        pdu.user_defined_data = SdsUserData::Type4(5, payload.clone());
        let mut buf = BitBuffer::new_autoexpand(64);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "length_indicator",
                value: 5,
            })
        );
    }

    #[test]
    fn test_u_sds_data_type4_allows_2047_bit_payload() {
        let mut payload: Vec<u8> = (0..256).map(|idx| (idx ^ 0xA5) as u8).collect();
        payload[255] &= 0xFE;
        let mut pdu = base_u_sds_data();
        pdu.user_defined_data = SdsUserData::Type4(2047, payload.clone());

        let parsed = round_trip(&pdu);

        // EN 300 392-2 clauses 14.7.2.8 and 14.8.52: U-SDS-DATA Type4 uses
        // an 11-bit length indicator and may carry 0..2047 user-data bits.
        assert_eq!(parsed.user_defined_data, SdsUserData::Type4(2047, payload));
    }

    #[test]
    fn test_u_sds_data_type4_rejects_invalid_payload_shape() {
        let mut buf = BitBuffer::new_autoexpand(64);
        let over_width = USdsData {
            area_selection: 0,
            called_party_type_identifier: PartyTypeIdentifier::Ssi,
            called_party_short_number_address: None,
            called_party_ssi: Some(2000002),
            called_party_extension: None,
            user_defined_data: SdsUserData::Type4(2048, vec![0u8; 256]),
            external_subscriber_number: None,
            dm_ms_address: None,
        };
        assert_eq!(
            over_width.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "length_indicator",
                value: 2048,
            })
        );

        let missing_pid = USdsData {
            area_selection: 0,
            called_party_type_identifier: PartyTypeIdentifier::Ssi,
            called_party_short_number_address: None,
            called_party_ssi: Some(2000002),
            called_party_extension: None,
            user_defined_data: SdsUserData::Type4(7, vec![0xAA]),
            external_subscriber_number: None,
            dm_ms_address: None,
        };
        let mut buf = BitBuffer::new_autoexpand(64);
        assert_eq!(
            missing_pid.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "length_indicator",
                value: 7,
            })
        );

        let short_payload = USdsData {
            area_selection: 0,
            called_party_type_identifier: PartyTypeIdentifier::Ssi,
            called_party_short_number_address: None,
            called_party_ssi: Some(2000002),
            called_party_extension: None,
            user_defined_data: SdsUserData::Type4(17, vec![0xAA, 0xBB]),
            external_subscriber_number: None,
            dm_ms_address: None,
        };
        let mut buf = BitBuffer::new_autoexpand(64);
        assert_eq!(
            short_payload.to_bitbuf(&mut buf),
            Err(PduParseErr::InconsistentLength { expected: 3, found: 2 })
        );
    }

    #[test]
    fn test_u_sds_data_parser_rejects_type4_below_protocol_identifier_width() {
        for len_bits in [0u16, 7] {
            let mut buf = BitBuffer::new_autoexpand(64);
            buf.write_bits(CmcePduTypeUl::USdsData.into_raw(), 5);
            buf.write_bits(0, 4);
            buf.write_bits(PartyTypeIdentifier::Ssi.into_raw(), 2);
            buf.write_bits(2000002, 24);
            buf.write_bits(3, 2);
            buf.write_bits(len_bits as u64, 11);
            if len_bits > 0 {
                buf.write_bits(0x55, len_bits as usize);
            }
            buf.write_bits(0, 1);
            buf.seek(0);

            // EN 300 392-2 clause 14.8.52: inbound Type4 SDS must contain
            // the 8-bit protocol identifier before protocol-dependent bits.
            assert_eq!(
                USdsData::from_bitbuf(&mut buf).unwrap_err(),
                PduParseErr::InvalidValue {
                    field: "length_indicator",
                    value: len_bits as u64,
                }
            );
        }
    }

    #[test]
    fn test_u_sds_data_parser_canonicalizes_type4_tail_bits() {
        let mut buf = BitBuffer::new_autoexpand(64);
        buf.write_bits(CmcePduTypeUl::USdsData.into_raw(), 5);
        buf.write_bits(0, 4);
        buf.write_bits(PartyTypeIdentifier::Ssi.into_raw(), 2);
        buf.write_bits(2000002, 24);
        buf.write_bits(3, 2);
        buf.write_bits(17, 11);
        buf.write_bits(0xAA, 8);
        buf.write_bits(0xBB, 8);
        buf.write_bits(1, 1);
        buf.write_bits(0, 1);
        buf.seek(0);

        let parsed = USdsData::from_bitbuf(&mut buf).expect("expected valid 17-bit Type4 U-SDS-DATA");

        assert_eq!(parsed.user_defined_data, SdsUserData::Type4(17, vec![0xAA, 0xBB, 0x80]));
    }

    #[test]
    fn test_u_sds_data_cpti0_sna() {
        let pdu = USdsData {
            area_selection: 0,
            called_party_type_identifier: PartyTypeIdentifier::Sna,
            called_party_short_number_address: Some(42),
            called_party_ssi: None,
            called_party_extension: None,
            user_defined_data: SdsUserData::Type2(0x12345678),
            external_subscriber_number: None,
            dm_ms_address: None,
        };
        let parsed = round_trip(&pdu);
        assert_eq!(parsed.called_party_type_identifier, PartyTypeIdentifier::Sna);
        assert_eq!(parsed.called_party_short_number_address, Some(42));
        assert_eq!(parsed.called_party_ssi, None);
        assert_eq!(parsed.user_defined_data, SdsUserData::Type2(0x12345678));
    }

    #[test]
    fn test_u_sds_data_cpti2_extension() {
        let pdu = USdsData {
            area_selection: 0,
            called_party_type_identifier: PartyTypeIdentifier::Tsi,
            called_party_short_number_address: None,
            called_party_ssi: Some(3000003),
            called_party_extension: Some(0xABCDEF),
            user_defined_data: SdsUserData::Type3(0x0102030405060708),
            external_subscriber_number: None,
            dm_ms_address: None,
        };
        let parsed = round_trip(&pdu);
        assert_eq!(parsed.called_party_type_identifier, PartyTypeIdentifier::Tsi);
        assert_eq!(parsed.called_party_ssi, Some(3000003));
        assert_eq!(parsed.called_party_extension, Some(0xABCDEF));
        assert_eq!(parsed.user_defined_data, SdsUserData::Type3(0x0102030405060708));
    }

    #[test]
    fn test_u_sds_data_rejects_reserved_called_party_type() {
        let pdu = USdsData {
            called_party_type_identifier: PartyTypeIdentifier::Reserved,
            ..base_u_sds_data()
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
    fn test_u_sds_data_parser_rejects_reserved_called_party_type() {
        let mut buf = BitBuffer::new_autoexpand(32);
        buf.write_bits(CmcePduTypeUl::USdsData.into_raw(), 5);
        buf.write_bits(0, 4);
        buf.write_bits(PartyTypeIdentifier::Reserved.into_raw(), 2);
        buf.write_bits(0, 2);
        buf.write_bits(0, 16);
        buf.write_bits(0, 1);
        buf.seek(0);

        assert_eq!(
            USdsData::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "called_party_type_identifier",
                value: PartyTypeIdentifier::Reserved.into_raw(),
            }
        );
    }

    #[test]
    fn test_u_sds_data_rejects_area_selection_above_4_bits() {
        let pdu = USdsData {
            area_selection: 0x10,
            ..base_u_sds_data()
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
    fn test_u_sds_data_requires_called_party_fields_for_cpti() {
        let missing_sna = USdsData {
            called_party_type_identifier: PartyTypeIdentifier::Sna,
            called_party_short_number_address: None,
            called_party_ssi: None,
            called_party_extension: None,
            ..base_u_sds_data()
        };
        let mut buf = BitBuffer::new_autoexpand(32);
        assert_eq!(
            missing_sna.to_bitbuf(&mut buf),
            Err(PduParseErr::FieldNotPresent {
                field: Some("called_party_short_number_address"),
            })
        );
        assert_eq!(buf.get_len(), 0);

        let missing_ssi = USdsData {
            called_party_ssi: None,
            ..base_u_sds_data()
        };
        let mut buf = BitBuffer::new_autoexpand(32);
        assert_eq!(
            missing_ssi.to_bitbuf(&mut buf),
            Err(PduParseErr::FieldNotPresent {
                field: Some("called_party_ssi"),
            })
        );
        assert_eq!(buf.get_len(), 0);

        let missing_tsi_extension = USdsData {
            called_party_type_identifier: PartyTypeIdentifier::Tsi,
            called_party_ssi: Some(0x00AA_BBCC),
            called_party_extension: None,
            ..base_u_sds_data()
        };
        let mut buf = BitBuffer::new_autoexpand(32);
        assert_eq!(
            missing_tsi_extension.to_bitbuf(&mut buf),
            Err(PduParseErr::FieldNotPresent {
                field: Some("called_party_extension"),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn test_u_sds_data_rejects_forbidden_fields_for_cpti() {
        let sna_with_ssi = USdsData {
            called_party_type_identifier: PartyTypeIdentifier::Sna,
            called_party_short_number_address: Some(42),
            called_party_ssi: Some(0x00AA_BBCC),
            called_party_extension: None,
            ..base_u_sds_data()
        };
        let mut buf = BitBuffer::new_autoexpand(32);
        assert_eq!(
            sna_with_ssi.to_bitbuf(&mut buf),
            Err(PduParseErr::Inconsistency {
                field: "called_party_ssi",
                reason: "not valid for called party type identifier",
            })
        );
        assert_eq!(buf.get_len(), 0);

        let ssi_with_sna = USdsData {
            called_party_type_identifier: PartyTypeIdentifier::Ssi,
            called_party_short_number_address: Some(42),
            called_party_ssi: Some(0x00AA_BBCC),
            called_party_extension: None,
            ..base_u_sds_data()
        };
        let mut buf = BitBuffer::new_autoexpand(32);
        assert_eq!(
            ssi_with_sna.to_bitbuf(&mut buf),
            Err(PduParseErr::Inconsistency {
                field: "called_party_short_number_address",
                reason: "not valid for called party type identifier",
            })
        );
        assert_eq!(buf.get_len(), 0);

        let ssi_with_extension = USdsData {
            called_party_extension: Some(0x123456),
            ..base_u_sds_data()
        };
        let mut buf = BitBuffer::new_autoexpand(32);
        assert_eq!(
            ssi_with_extension.to_bitbuf(&mut buf),
            Err(PduParseErr::Inconsistency {
                field: "called_party_extension",
                reason: "not valid for called party type identifier",
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn test_u_sds_data_rejects_called_address_values_above_field_width() {
        let over_sna = USdsData {
            called_party_type_identifier: PartyTypeIdentifier::Sna,
            called_party_short_number_address: Some(0x100),
            called_party_ssi: None,
            called_party_extension: None,
            ..base_u_sds_data()
        };
        let mut buf = BitBuffer::new_autoexpand(32);
        assert_eq!(
            over_sna.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "called_party_short_number_address",
                value: 0x100,
            })
        );
        assert_eq!(buf.get_len(), 0);

        let over_ssi = USdsData {
            called_party_ssi: Some(0x0100_0000),
            ..base_u_sds_data()
        };
        let mut buf = BitBuffer::new_autoexpand(32);
        assert_eq!(
            over_ssi.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "called_party_ssi",
                value: 0x0100_0000,
            })
        );
        assert_eq!(buf.get_len(), 0);

        let over_extension = USdsData {
            called_party_type_identifier: PartyTypeIdentifier::Tsi,
            called_party_ssi: Some(0x00AA_BBCC),
            called_party_extension: Some(0x0100_0000),
            ..base_u_sds_data()
        };
        let mut buf = BitBuffer::new_autoexpand(32);
        assert_eq!(
            over_extension.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "called_party_extension",
                value: 0x0100_0000,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }
}
