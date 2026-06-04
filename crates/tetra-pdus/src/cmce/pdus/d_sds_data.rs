use core::fmt;

use crate::cmce::enums::{cmce_pdu_type_dl::CmcePduTypeDl, party_type_identifier::PartyTypeIdentifier, type3_elem_id::CmceType3ElemId};
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};
use tetra_saps::control::enums::sds_user_data::SdsUserData;

use super::sds_user_data_codec::{read_sds_type4_user_data, write_sds_user_data};

/// Representation of the D-SDS-DATA PDU (Clause 14.7.1.10).
/// This PDU shall be for receiving user defined SDS data.
/// Response expected: -
/// Response to: -

// note 1: Shall be conditional on the value of Calling Party Type Identifier (CPTI): CPTI = 1: Calling Party SSI; CPTI = 2: Calling Party SSI + Calling Party Extension.
// note 2: Shall be conditional on the value of Short Data Type Identifier (SDTI): SDTI = 0: User Defined Data-1; SDTI = 1: User Defined Data-2; SDTI = 2: User Defined Data-3; SDTI = 3: Length Indicator + User Defined Data-4.
// Clause 14.8.52 defines Type4 content as an 8-bit protocol identifier followed by 0..2039 protocol-dependent bits.
#[derive(Debug)]
pub struct DSdsData {
    /// Type1, 2 bits, Calling party type identifier
    pub calling_party_type_identifier: PartyTypeIdentifier,
    /// Conditional 24 bits, See note 1, condition: calling_party_type_identifier == 1 || calling_party_type_identifier == 2
    pub calling_party_address_ssi: Option<u64>,
    /// Conditional 24 bits, See note 1, condition: calling_party_type_identifier == 2
    pub calling_party_extension: Option<u64>,
    /// Either type1, type2, type3 or type4 user data field.
    pub user_defined_data: SdsUserData,
    /// Type3, External subscriber number
    pub external_subscriber_number: Option<Type3FieldGeneric>,
    /// Type3, DM-MS address
    pub dm_ms_address: Option<Type3FieldGeneric>,
}

impl DSdsData {
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
        expect_pdu_type!(pdu_type, CmcePduTypeDl::DSdsData)?;

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

        Ok(DSdsData {
            calling_party_type_identifier,
            calling_party_address_ssi,
            calling_party_extension,
            user_defined_data,
            external_subscriber_number,
            dm_ms_address,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate_for_serialization()?;

        // PDU Type
        buffer.write_bits(CmcePduTypeDl::DSdsData.into_raw(), 5);
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

        // Type1 + conditional user data. EN 300 392-2 14.7.1.10 / 14.8.38
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

impl fmt::Display for DSdsData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DSdsData {{ calling_party_type_identifier: {:?} calling_party_address_ssi: {:?} calling_party_extension: {:?} user_defined_data: {:?} external_subscriber_number: {:?} dm_ms_address: {:?} }}",
            self.calling_party_type_identifier,
            self.calling_party_address_ssi,
            self.calling_party_extension,
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

    fn round_trip(pdu: &DSdsData) -> DSdsData {
        let mut buf = BitBuffer::new_autoexpand(256);
        pdu.to_bitbuf(&mut buf).expect("serialize failed");
        buf.seek(0);
        DSdsData::from_bitbuf(&mut buf).expect("parse failed")
    }

    fn base_d_sds_data() -> DSdsData {
        DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Ssi,
            calling_party_address_ssi: Some(0x00AA_BBCC),
            calling_party_extension: None,
            user_defined_data: SdsUserData::Type1(0xABCD),
            external_subscriber_number: None,
            dm_ms_address: None,
        }
    }

    #[test]
    fn test_d_sds_data_sdti0_cpti1() {
        let pdu = DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Ssi,
            calling_party_address_ssi: Some(1000001),
            calling_party_extension: None,
            user_defined_data: SdsUserData::Type1(0xABCD),
            external_subscriber_number: None,
            dm_ms_address: None,
        };
        let parsed = round_trip(&pdu);
        assert_eq!(parsed.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
        assert_eq!(parsed.calling_party_address_ssi, Some(1000001));
        assert_eq!(parsed.calling_party_extension, None);
        assert_eq!(parsed.user_defined_data, SdsUserData::Type1(0xABCD));
    }

    #[test]
    fn test_d_sds_data_sdti3_cpti1() {
        let payload = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA];
        let pdu = DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Ssi,
            calling_party_address_ssi: Some(2000002),
            calling_party_extension: None,
            user_defined_data: SdsUserData::Type4(40, payload.clone()), // 5 bytes = 40 bits
            external_subscriber_number: None,
            dm_ms_address: None,
        };
        let parsed = round_trip(&pdu);
        assert_eq!(parsed.calling_party_type_identifier, PartyTypeIdentifier::Ssi);
        assert_eq!(parsed.calling_party_address_ssi, Some(2000002));
        assert_eq!(parsed.user_defined_data, SdsUserData::Type4(40, payload));
    }

    #[test]
    fn test_d_sds_data_type4_rejects_length_above_2047_bits() {
        let pdu = DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Ssi,
            calling_party_address_ssi: Some(2000002),
            calling_party_extension: None,
            user_defined_data: SdsUserData::Type4(2048, vec![0u8; 256]),
            external_subscriber_number: None,
            dm_ms_address: None,
        };
        let mut buf = BitBuffer::new_autoexpand(256);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "length_indicator",
                value: 2048,
            })
        );
    }

    #[test]
    fn test_d_sds_data_type4_rejects_missing_protocol_identifier() {
        let pdu = DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Ssi,
            calling_party_address_ssi: Some(2000002),
            calling_party_extension: None,
            user_defined_data: SdsUserData::Type4(0, Vec::new()),
            external_subscriber_number: None,
            dm_ms_address: None,
        };
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
    fn test_d_sds_data_type4_rejects_short_payload() {
        let pdu = DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Ssi,
            calling_party_address_ssi: Some(2000002),
            calling_party_extension: None,
            user_defined_data: SdsUserData::Type4(17, vec![0xAA, 0xBB]),
            external_subscriber_number: None,
            dm_ms_address: None,
        };
        let mut buf = BitBuffer::new_autoexpand(64);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InconsistentLength { expected: 3, found: 2 })
        );
    }

    #[test]
    fn test_d_sds_data_parser_rejects_type4_below_protocol_identifier_width() {
        for len_bits in [0u16, 7] {
            let mut buf = BitBuffer::new_autoexpand(64);
            buf.write_bits(CmcePduTypeDl::DSdsData.into_raw(), 5);
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
                DSdsData::from_bitbuf(&mut buf).unwrap_err(),
                PduParseErr::InvalidValue {
                    field: "length_indicator",
                    value: len_bits as u64,
                }
            );
        }
    }

    #[test]
    fn test_d_sds_data_parser_canonicalizes_type4_tail_bits() {
        let mut buf = BitBuffer::new_autoexpand(64);
        buf.write_bits(CmcePduTypeDl::DSdsData.into_raw(), 5);
        buf.write_bits(PartyTypeIdentifier::Ssi.into_raw(), 2);
        buf.write_bits(2000002, 24);
        buf.write_bits(3, 2);
        buf.write_bits(17, 11);
        buf.write_bits(0xAA, 8);
        buf.write_bits(0xBB, 8);
        buf.write_bits(1, 1);
        buf.write_bits(0, 1);
        buf.seek(0);

        let parsed = DSdsData::from_bitbuf(&mut buf).expect("expected valid 17-bit Type4 D-SDS-DATA");

        assert_eq!(parsed.user_defined_data, SdsUserData::Type4(17, vec![0xAA, 0xBB, 0x80]));
    }

    #[test]
    fn test_d_sds_data_type4_allows_2047_bit_payload() {
        let mut payload = vec![0xA5; 256];
        payload[255] = 0xFE;
        let pdu = DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Ssi,
            calling_party_address_ssi: Some(2000002),
            calling_party_extension: None,
            user_defined_data: SdsUserData::Type4(2047, payload.clone()),
            external_subscriber_number: None,
            dm_ms_address: None,
        };

        let parsed = round_trip(&pdu);
        assert_eq!(parsed.user_defined_data, SdsUserData::Type4(2047, payload));
    }

    #[test]
    fn test_d_sds_data_cpti2_extension() {
        let pdu = DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Tsi,
            calling_party_address_ssi: Some(3000003),
            calling_party_extension: Some(0x123456),
            user_defined_data: SdsUserData::Type1(0x1234),
            external_subscriber_number: None,
            dm_ms_address: None,
        };
        let parsed = round_trip(&pdu);
        assert_eq!(parsed.calling_party_type_identifier, PartyTypeIdentifier::Tsi);
        assert_eq!(parsed.calling_party_address_ssi, Some(3000003));
        assert_eq!(parsed.calling_party_extension, Some(0x123456));
        assert_eq!(parsed.user_defined_data, SdsUserData::Type1(0x1234));
    }

    #[test]
    fn test_d_sds_data_rejects_sna_calling_party_type() {
        let pdu = DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Sna,
            calling_party_address_ssi: None,
            calling_party_extension: None,
            ..base_d_sds_data()
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
    fn test_d_sds_data_rejects_reserved_calling_party_type() {
        let pdu = DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Reserved,
            ..base_d_sds_data()
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
    fn test_d_sds_data_parser_rejects_reserved_calling_party_type() {
        let mut buf = BitBuffer::new_autoexpand(32);
        buf.write_bits(CmcePduTypeDl::DSdsData.into_raw(), 5);
        buf.write_bits(PartyTypeIdentifier::Reserved.into_raw(), 2);
        buf.write_bits(0, 2);
        buf.write_bits(0, 16);
        buf.write_bits(0, 1);
        buf.seek(0);

        assert_eq!(
            DSdsData::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "calling_party_type_identifier",
                value: PartyTypeIdentifier::Reserved.into_raw(),
            }
        );
    }

    #[test]
    fn test_d_sds_data_parser_rejects_sna_calling_party_type() {
        let mut buf = BitBuffer::new_autoexpand(32);
        buf.write_bits(CmcePduTypeDl::DSdsData.into_raw(), 5);
        buf.write_bits(PartyTypeIdentifier::Sna.into_raw(), 2);
        buf.write_bits(0, 2);
        buf.write_bits(0, 16);
        buf.write_bits(0, 1);
        buf.seek(0);

        assert_eq!(
            DSdsData::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "calling_party_type_identifier",
                value: PartyTypeIdentifier::Sna.into_raw(),
            }
        );
    }

    #[test]
    fn test_d_sds_data_requires_calling_party_ssi_for_ssi_and_tsi() {
        for cpti in [PartyTypeIdentifier::Ssi, PartyTypeIdentifier::Tsi] {
            let pdu = DSdsData {
                calling_party_type_identifier: cpti,
                calling_party_address_ssi: None,
                calling_party_extension: if cpti == PartyTypeIdentifier::Tsi { Some(0x123456) } else { None },
                ..base_d_sds_data()
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
    }

    #[test]
    fn test_d_sds_data_requires_calling_party_extension_for_tsi() {
        let pdu = DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Tsi,
            calling_party_address_ssi: Some(0x00AA_BBCC),
            calling_party_extension: None,
            ..base_d_sds_data()
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
    fn test_d_sds_data_rejects_forbidden_calling_party_extension_for_ssi() {
        let pdu = DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Ssi,
            calling_party_address_ssi: Some(0x00AA_BBCC),
            calling_party_extension: Some(0x123456),
            ..base_d_sds_data()
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
    fn test_d_sds_data_rejects_calling_address_values_above_24_bits() {
        let over_ssi = DSdsData {
            calling_party_address_ssi: Some(0x0100_0000),
            ..base_d_sds_data()
        };
        let mut buf = BitBuffer::new_autoexpand(32);
        assert_eq!(
            over_ssi.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "calling_party_address_ssi",
                value: 0x0100_0000,
            })
        );
        assert_eq!(buf.get_len(), 0);

        let over_extension = DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Tsi,
            calling_party_address_ssi: Some(0x00AA_BBCC),
            calling_party_extension: Some(0x0100_0000),
            ..base_d_sds_data()
        };
        let mut buf = BitBuffer::new_autoexpand(32);
        assert_eq!(
            over_extension.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "calling_party_extension",
                value: 0x0100_0000,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }
}
