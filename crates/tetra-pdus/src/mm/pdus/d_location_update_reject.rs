use core::fmt;

use tetra_core::expect_pdu_type;
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mm::enums::location_update_type::LocationUpdateType;
use crate::mm::enums::mm_pdu_type_dl::MmPduTypeDl;
use crate::mm::enums::type34_elem_id_dl::MmType34ElemIdDl;

/// Representation of the D-LOCATION UPDATE REJECT PDU (Clause 16.9.2.9).
/// The infrastructure sends this message to the MS to indicate that updating in the network is not accepted.
/// Response expected: -
/// Response to: U-LOCATION UPDATE DEMAND

// note 1: Information element "Ciphering parameters" is not present if "Cipher control" is set to "0", "ciphering off".
// note 2: Information element "Ciphering parameters" is present if "Cipher control" is set to "1", "ciphering on".
#[derive(Debug)]
pub struct DLocationUpdateReject {
    /// Type1, 3 bits, Location update type
    pub location_update_type: LocationUpdateType,
    /// Type1, 5 bits, Reject cause
    pub reject_cause: u8,
    /// Type1, 1 bits, Cipher control
    pub cipher_control: bool,
    /// Conditional 10 bits, See note,
    pub ciphering_parameters: Option<u64>,
    /// Type2, 24 bits, MNI of the MS,
    pub address_extension: Option<u64>,
    /// Type3, Cell type control
    pub cell_type_control: Option<Type3FieldGeneric>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl DLocationUpdateReject {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(4, "pdu_type")?;
        expect_pdu_type!(pdu_type, MmPduTypeDl::DLocationUpdateReject)?;

        // Type1
        let location_update_type = buffer.read_field(3, "location_update_type")?;
        let location_update_type = LocationUpdateType::try_from(location_update_type).unwrap(); // never fails
        // Type1
        let reject_cause = buffer.read_field(5, "reject_cause")? as u8;
        // Type1
        let cipher_control = buffer.read_field(1, "cipher_control")? != 0;
        // Conditional 10-bit ciphering parameters; EN 300 392-2 table 16.13
        // includes it only when Cipher control is set.
        let ciphering_parameters = if cipher_control {
            Some(buffer.read_field(10, "ciphering_parameters")?)
        } else {
            None
        };

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type2
        let address_extension = typed::parse_type2_generic(obit, buffer, 24, "address_extension")?;

        // Type3
        let cell_type_control = typed::parse_type3_generic(obit, buffer, MmType34ElemIdDl::CellTypeControl)?;

        // Type3
        let proprietary = typed::parse_type3_generic(obit, buffer, MmType34ElemIdDl::Proprietary)?;
        // Read trailing mbit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        Ok(DLocationUpdateReject {
            location_update_type,
            reject_cause,
            cipher_control,
            ciphering_parameters,
            address_extension,
            cell_type_control,
            proprietary,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        // PDU Type
        buffer.write_bits(MmPduTypeDl::DLocationUpdateReject.into_raw(), 4);
        // Type1
        buffer.write_bits(self.location_update_type as u64, 3);
        // Type1
        buffer.write_bits(self.reject_cause as u64, 5);
        // Type1
        buffer.write_bits(self.cipher_control as u64, 1);
        // Conditional
        match (self.cipher_control, self.ciphering_parameters) {
            (true, Some(value)) if value <= 0x03ff => buffer.write_bits(value, 10),
            (true, Some(value)) => {
                return Err(PduParseErr::InvalidValue {
                    field: "ciphering_parameters",
                    value,
                });
            }
            (true, None) => {
                return Err(PduParseErr::Inconsistency {
                    field: "ciphering_parameters",
                    reason: "missing when cipher_control is set",
                });
            }
            (false, Some(_)) => {
                return Err(PduParseErr::Inconsistency {
                    field: "ciphering_parameters",
                    reason: "present when cipher_control is not set",
                });
            }
            (false, None) => {}
        }

        // Check if any optional field present and place o-bit
        let obit = self.address_extension.is_some() || self.cell_type_control.is_some() || self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2
        typed::write_type2_generic(obit, buffer, self.address_extension, 24);

        // Type3
        typed::write_type3_generic(obit, buffer, &self.cell_type_control, MmType34ElemIdDl::CellTypeControl)?;

        // Type3
        typed::write_type3_generic(obit, buffer, &self.proprietary, MmType34ElemIdDl::Proprietary)?;

        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for DLocationUpdateReject {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DLocationUpdateReject {{ location_update_type: {:?} reject_cause: {:?} cipher_control: {:?} ciphering_parameters: {:?} address_extension: {:?} cell_type_control: {:?} proprietary: {:?} }}",
            self.location_update_type,
            self.reject_cause,
            self.cipher_control,
            self.ciphering_parameters,
            self.address_extension,
            self.cell_type_control,
            self.proprietary,
        )
    }
}

#[cfg(test)]
mod tests {
    use tetra_core::debug;

    use super::*;

    #[test]
    fn test_d_location_update_reject_without_cipher_parameters_round_trips() {
        debug::setup_logging_verbose();

        // EN 300 392-2 clause 16.9.2.9 table 16.13: when Cipher control is
        // zero, the 10-bit Ciphering parameters field is absent. Final zero is
        // the no-more-fields o-bit.
        let test_vec = "01110110110000";
        let mut buf_in = BitBuffer::from_bitstr(test_vec);
        let pdu = DLocationUpdateReject::from_bitbuf(&mut buf_in).expect("Failed parsing D-LOCATION UPDATE REJECT");

        assert_eq!(pdu.location_update_type, LocationUpdateType::ItsiAttach);
        assert_eq!(pdu.reject_cause, 12);
        assert!(!pdu.cipher_control);
        assert_eq!(pdu.ciphering_parameters, None);
        assert_eq!(buf_in.get_len_remaining(), 0);

        let mut buf_out = BitBuffer::new_autoexpand(16);
        pdu.to_bitbuf(&mut buf_out).unwrap();
        assert_eq!(buf_out.to_bitstr(), test_vec);
    }

    #[test]
    fn test_d_location_update_reject_with_cipher_parameters_round_trips() {
        debug::setup_logging_verbose();

        let pdu = DLocationUpdateReject {
            location_update_type: LocationUpdateType::DemandLocationUpdating,
            reject_cause: 18,
            cipher_control: true,
            ciphering_parameters: Some(0b10_1010_1010),
            address_extension: None,
            cell_type_control: None,
            proprietary: None,
        };
        let mut buf_out = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut buf_out).unwrap();

        let mut buf_in = BitBuffer::from_bitstr(&buf_out.to_bitstr());
        let parsed = DLocationUpdateReject::from_bitbuf(&mut buf_in).expect("Failed parsing D-LOCATION UPDATE REJECT");

        assert_eq!(parsed.location_update_type, LocationUpdateType::DemandLocationUpdating);
        assert_eq!(parsed.reject_cause, 18);
        assert!(parsed.cipher_control);
        assert_eq!(parsed.ciphering_parameters, Some(0b10_1010_1010));
        assert_eq!(buf_in.get_len_remaining(), 0);
    }

    #[test]
    fn test_d_location_update_reject_rejects_inconsistent_cipher_presence() {
        let missing = DLocationUpdateReject {
            location_update_type: LocationUpdateType::ItsiAttach,
            reject_cause: 12,
            cipher_control: true,
            ciphering_parameters: None,
            address_extension: None,
            cell_type_control: None,
            proprietary: None,
        };
        let mut missing_buf = BitBuffer::new_autoexpand(16);
        assert!(matches!(
            missing.to_bitbuf(&mut missing_buf),
            Err(PduParseErr::Inconsistency {
                field: "ciphering_parameters",
                ..
            })
        ));

        let unexpected = DLocationUpdateReject {
            location_update_type: LocationUpdateType::ItsiAttach,
            reject_cause: 12,
            cipher_control: false,
            ciphering_parameters: Some(0),
            address_extension: None,
            cell_type_control: None,
            proprietary: None,
        };
        let mut unexpected_buf = BitBuffer::new_autoexpand(16);
        assert!(matches!(
            unexpected.to_bitbuf(&mut unexpected_buf),
            Err(PduParseErr::Inconsistency {
                field: "ciphering_parameters",
                ..
            })
        ));

        let too_large = DLocationUpdateReject {
            location_update_type: LocationUpdateType::ItsiAttach,
            reject_cause: 12,
            cipher_control: true,
            ciphering_parameters: Some(0x0400),
            address_extension: None,
            cell_type_control: None,
            proprietary: None,
        };
        let mut too_large_buf = BitBuffer::new_autoexpand(16);
        assert!(matches!(
            too_large.to_bitbuf(&mut too_large_buf),
            Err(PduParseErr::InvalidValue {
                field: "ciphering_parameters",
                value: 0x0400,
            })
        ));
    }
}
