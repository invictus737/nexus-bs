use core::fmt;

use crate::cmce::enums::{cmce_pdu_type_ul::CmcePduTypeUl, type3_elem_id::CmceType3ElemId};
use crate::cmce::fields::basic_service_information::BasicServiceInformation;
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

const OPTI_SNA: u8 = 0;
const OPTI_SSI: u8 = 1;
const OPTI_TSI: u8 = 2;
const MAX_U8: u64 = 0xff;
const MAX_U14: u64 = 0x3fff;
const MAX_U24: u64 = 0x00ff_ffff;

/// Representation of the U-CALL RESTORE PDU (Clause 14.7.2.2).
/// This PDU shall be the order from the MS for restoration of a specific call after a temporary break of the call.
/// Response expected: D-CALL RESTORE
/// Response to: None

// note 1: Shall be conditional on the value of Other Party Type Identifier (OPTI): OPTI = 0; Other Party SNA; OPTI = 1; Other Party SSI; OPTI = 2; Other Party SSI + Other Party Extension.
// note 2: A use of SNA in call restoration is strongly discouraged as SS-SNA may not be supported in all networks.
// note 3: Although coded as a type 2 element, this information element is mandatory to inform the new cell of the basic service of the current call.
#[derive(Debug)]
pub struct UCallRestore {
    /// Type1, 14 bits, Call identifier
    pub call_identifier: u16,
    /// Type1, 1 bits, Request to transmit/send data
    pub request_to_transmit_send_data: bool,
    /// Type1, 2 bits, Other party type identifier
    pub other_party_type_identifier: u8,
    /// Conditional 8 bits, See notes 1 and 2, condition: other_party_type_identifier == 0
    pub other_party_short_number_address: Option<u64>,
    /// Conditional 24 bits, Other party SSI condition: other_party_type_identifier == 1 || other_party_type_identifier == 2
    pub other_party_ssi: Option<u64>,
    /// Conditional 24 bits, See note 1, condition: other_party_type_identifier == 2
    pub other_party_extension: Option<u64>,
    /// Type2, 8 bits, See note 3,
    pub basic_service_information: Option<BasicServiceInformation>,
    /// Type3, Facility
    pub facility: Option<Type3FieldGeneric>,
    /// Type3, DM-MS address
    pub dm_ms_address: Option<Type3FieldGeneric>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl UCallRestore {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 table 14.22: call identifier is 14 bits.
        if self.call_identifier as u64 > MAX_U14 {
            return Err(PduParseErr::InvalidValue {
                field: "call_identifier",
                value: self.call_identifier as u64,
            });
        }

        // EN 300 392-2 table 14.22 note 1: OPTI selects exactly which other
        // party address fields are present. OPTI=3 is reserved here.
        match self.other_party_type_identifier {
            OPTI_SNA => {
                let short = self.other_party_short_number_address.ok_or(PduParseErr::FieldNotPresent {
                    field: Some("other_party_short_number_address"),
                })?;
                if short > MAX_U8 {
                    return Err(PduParseErr::InvalidValue {
                        field: "other_party_short_number_address",
                        value: short,
                    });
                }
                if self.other_party_ssi.is_some() {
                    return Err(PduParseErr::Inconsistency {
                        field: "other_party_ssi",
                        reason: "not valid for OPTI=SNA",
                    });
                }
                if self.other_party_extension.is_some() {
                    return Err(PduParseErr::Inconsistency {
                        field: "other_party_extension",
                        reason: "not valid for OPTI=SNA",
                    });
                }
            }
            OPTI_SSI => {
                let ssi = self.other_party_ssi.ok_or(PduParseErr::FieldNotPresent {
                    field: Some("other_party_ssi"),
                })?;
                if ssi > MAX_U24 {
                    return Err(PduParseErr::InvalidValue {
                        field: "other_party_ssi",
                        value: ssi,
                    });
                }
                if self.other_party_short_number_address.is_some() {
                    return Err(PduParseErr::Inconsistency {
                        field: "other_party_short_number_address",
                        reason: "not valid for OPTI=SSI",
                    });
                }
                if self.other_party_extension.is_some() {
                    return Err(PduParseErr::Inconsistency {
                        field: "other_party_extension",
                        reason: "not valid for OPTI=SSI",
                    });
                }
            }
            OPTI_TSI => {
                let ssi = self.other_party_ssi.ok_or(PduParseErr::FieldNotPresent {
                    field: Some("other_party_ssi"),
                })?;
                if ssi > MAX_U24 {
                    return Err(PduParseErr::InvalidValue {
                        field: "other_party_ssi",
                        value: ssi,
                    });
                }
                let extension = self.other_party_extension.ok_or(PduParseErr::FieldNotPresent {
                    field: Some("other_party_extension"),
                })?;
                if extension > MAX_U24 {
                    return Err(PduParseErr::InvalidValue {
                        field: "other_party_extension",
                        value: extension,
                    });
                }
                if self.other_party_short_number_address.is_some() {
                    return Err(PduParseErr::Inconsistency {
                        field: "other_party_short_number_address",
                        reason: "not valid for OPTI=TSI",
                    });
                }
            }
            value => {
                return Err(PduParseErr::InvalidValue {
                    field: "other_party_type_identifier",
                    value: value as u64,
                });
            }
        }

        // EN 300 392-2 table 14.22 note 3: BSI is mandatory even though the
        // information element is encoded as Type2.
        let basic_service_information = self.basic_service_information.as_ref().ok_or(PduParseErr::FieldNotPresent {
            field: Some("basic_service_information"),
        })?;
        basic_service_information.validate()?;

        Ok(())
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(5, "pdu_type")?;
        expect_pdu_type!(pdu_type, CmcePduTypeUl::UCallRestore)?;

        // Type1
        let call_identifier = buffer.read_field(14, "call_identifier")? as u16;
        // Type1
        let request_to_transmit_send_data = buffer.read_field(1, "request_to_transmit_send_data")? != 0;
        // Type1
        let other_party_type_identifier = buffer.read_field(2, "other_party_type_identifier")? as u8;
        // Conditional
        let other_party_short_number_address = if other_party_type_identifier == 0 {
            Some(buffer.read_field(8, "other_party_short_number_address")?)
        } else {
            None
        };
        // Conditional
        let other_party_ssi = if other_party_type_identifier == 1 || other_party_type_identifier == 2 {
            Some(buffer.read_field(24, "other_party_ssi")?)
        } else {
            None
        };
        // Conditional
        let other_party_extension = if other_party_type_identifier == 2 {
            Some(buffer.read_field(24, "other_party_extension")?)
        } else {
            None
        };

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type2
        let basic_service_information = typed::parse_type2_struct(obit, buffer, BasicServiceInformation::from_bitbuf)?;

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

        let pdu = UCallRestore {
            call_identifier,
            request_to_transmit_send_data,
            other_party_type_identifier,
            other_party_short_number_address,
            other_party_ssi,
            other_party_extension,
            basic_service_information,
            facility,
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
        buffer.write_bits(CmcePduTypeUl::UCallRestore.into_raw(), 5);
        // Type1
        buffer.write_bits(self.call_identifier as u64, 14);
        // Type1
        buffer.write_bits(self.request_to_transmit_send_data as u64, 1);
        // Type1
        buffer.write_bits(self.other_party_type_identifier as u64, 2);
        // Conditional
        if let Some(ref value) = self.other_party_short_number_address {
            buffer.write_bits(*value, 8);
        }
        // Conditional
        if let Some(ref value) = self.other_party_ssi {
            buffer.write_bits(*value, 24);
        }
        // Conditional
        if let Some(ref value) = self.other_party_extension {
            buffer.write_bits(*value, 24);
        }

        // Check if any optional field present and place o-bit
        let obit = self.basic_service_information.is_some()
            || self.facility.is_some()
            || self.dm_ms_address.is_some()
            || self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2
        typed::write_type2_struct(obit, buffer, &self.basic_service_information, BasicServiceInformation::to_bitbuf)?;

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

impl fmt::Display for UCallRestore {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "UCallRestore {{ call_identifier: {:?} request_to_transmit_send_data: {:?} other_party_type_identifier: {:?} other_party_short_number_address: {:?} other_party_ssi: {:?} other_party_extension: {:?} basic_service_information: {:?} facility: {:?} dm_ms_address: {:?} proprietary: {:?} }}",
            self.call_identifier,
            self.request_to_transmit_send_data,
            self.other_party_type_identifier,
            self.other_party_short_number_address,
            self.other_party_ssi,
            self.other_party_extension,
            self.basic_service_information,
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
    use tetra_saps::control::enums::{circuit_mode_type::CircuitModeType, communication_type::CommunicationType};

    fn bsi() -> BasicServiceInformation {
        BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2p,
            slots_per_frame: None,
            speech_service: Some(0),
        }
    }

    fn base_u_call_restore() -> UCallRestore {
        UCallRestore {
            call_identifier: 0x1234,
            request_to_transmit_send_data: false,
            other_party_type_identifier: OPTI_SSI,
            other_party_short_number_address: None,
            other_party_ssi: Some(0x00aa_bbcc),
            other_party_extension: None,
            basic_service_information: Some(bsi()),
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        }
    }

    fn round_trip(pdu: &UCallRestore) -> UCallRestore {
        let mut buf = BitBuffer::new_autoexpand(128);
        pdu.to_bitbuf(&mut buf).expect("serialize U-CALL RESTORE");
        buf.seek(0);
        UCallRestore::from_bitbuf(&mut buf).expect("parse U-CALL RESTORE")
    }

    #[test]
    fn u_call_restore_roundtrips_ssi_other_party_with_mandatory_bsi() {
        let decoded = round_trip(&base_u_call_restore());

        assert_eq!(decoded.call_identifier, 0x1234);
        assert_eq!(decoded.other_party_type_identifier, OPTI_SSI);
        assert_eq!(decoded.other_party_ssi, Some(0x00aa_bbcc));
        assert!(decoded.other_party_short_number_address.is_none());
        assert!(decoded.other_party_extension.is_none());
        assert!(decoded.basic_service_information.is_some());
    }

    #[test]
    fn u_call_restore_roundtrips_tsi_other_party() {
        let pdu = UCallRestore {
            other_party_type_identifier: OPTI_TSI,
            other_party_ssi: Some(0x0012_3456),
            other_party_extension: Some(0x0065_4321),
            ..base_u_call_restore()
        };
        let decoded = round_trip(&pdu);

        assert_eq!(decoded.other_party_type_identifier, OPTI_TSI);
        assert_eq!(decoded.other_party_ssi, Some(0x0012_3456));
        assert_eq!(decoded.other_party_extension, Some(0x0065_4321));
    }

    #[test]
    fn u_call_restore_requires_basic_service_information_on_serialize() {
        let pdu = UCallRestore {
            basic_service_information: None,
            ..base_u_call_restore()
        };
        let mut buf = BitBuffer::new_autoexpand(64);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::FieldNotPresent {
                field: Some("basic_service_information"),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn u_call_restore_requires_basic_service_information_on_parse() {
        let mut buf = BitBuffer::new_autoexpand(64);
        buf.write_bits(CmcePduTypeUl::UCallRestore.into_raw(), 5);
        buf.write_bits(1, 14);
        buf.write_bits(0, 1);
        buf.write_bits(OPTI_SSI as u64, 2);
        buf.write_bits(0x123456, 24);
        buf.write_bits(0, 1);
        buf.seek(0);

        assert_eq!(
            UCallRestore::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::FieldNotPresent {
                field: Some("basic_service_information"),
            }
        );
    }

    #[test]
    fn u_call_restore_rejects_reserved_opti_on_parse() {
        let mut buf = BitBuffer::new_autoexpand(64);
        buf.write_bits(CmcePduTypeUl::UCallRestore.into_raw(), 5);
        buf.write_bits(1, 14);
        buf.write_bits(0, 1);
        buf.write_bits(3, 2);
        buf.write_bits(1, 1);
        buf.write_bits(1, 1);
        bsi().to_bitbuf(&mut buf).expect("BSI should serialize");
        buf.write_bits(0, 1);
        buf.seek(0);

        assert_eq!(
            UCallRestore::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "other_party_type_identifier",
                value: 3,
            }
        );
    }

    #[test]
    fn u_call_restore_rejects_opti_address_inconsistency() {
        let pdu = UCallRestore {
            other_party_extension: Some(0x12),
            ..base_u_call_restore()
        };
        let mut buf = BitBuffer::new_autoexpand(64);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::Inconsistency {
                field: "other_party_extension",
                reason: "not valid for OPTI=SSI",
            })
        );
        assert_eq!(buf.get_len(), 0);
    }
}
