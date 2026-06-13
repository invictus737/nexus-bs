// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::expect_pdu_type;
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mm::enums::mm_pdu_type_dl::MmPduTypeDl;
use crate::mm::enums::type34_elem_id_dl::MmType34ElemIdDl;

/// Representation of the D-LOCATION UPDATE COMMAND PDU (Clause 16.9.2.8).
/// The infrastructure sends this message to the MS to initiate a location update demand in the MS.
/// Response expected: U-LOCATION UPDATE DEMAND
/// Response to: -

// note 1: Ciphering parameters element is not present if Cipher control is set to ‘0’ and is present if set to ‘1’.
#[derive(Debug)]
pub struct DLocationUpdateCommand {
    /// Type1, 1 bits, Group identity report
    pub group_identity_report: bool,
    /// Type1, 1 bits, Cipher control
    pub cipher_control: bool,
    /// Conditional 10 bits, Conditional: present only if Cipher control = 1 (on); absent if Cipher control = 0 (off),
    pub ciphering_parameters: Option<u64>,
    /// Type2, 24 bits, MNI of the MS,
    pub address_extension: Option<u64>,
    /// Type3, Cell type control
    pub cell_type_control: Option<Type3FieldGeneric>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl DLocationUpdateCommand {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(4, "pdu_type")?;
        expect_pdu_type!(pdu_type, MmPduTypeDl::DLocationUpdateCommand)?;

        // Type1
        let group_identity_report = buffer.read_field(1, "group_identity_report")? != 0;
        // Type1
        let cipher_control = buffer.read_field(1, "cipher_control")? != 0;
        // Conditional 10-bit ciphering parameters; EN 300 392-2 table 16.12
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

        // Read trailing obit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        Ok(DLocationUpdateCommand {
            group_identity_report,
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
        buffer.write_bits(MmPduTypeDl::DLocationUpdateCommand.into_raw(), 4);
        // Type1
        buffer.write_bits(self.group_identity_report as u64, 1);
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

impl fmt::Display for DLocationUpdateCommand {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DLocationUpdateCommand {{ group_identity_report: {:?} cipher_control: {:?} ciphering_parameters: {:?} address_extension: {:?} cell_type_control: {:?} proprietary: {:?} }}",
            self.group_identity_report,
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
    fn test_d_location_update_command_without_cipher_parameters_round_trips() {
        debug::setup_logging_verbose();

        // EN 300 392-2 clause 16.9.2.8 table 16.12: when Cipher control is
        // zero, the 10-bit Ciphering parameters field is absent. Final zero is
        // the no-more-fields o-bit.
        let test_vec = "0110100";
        let mut buf_in = BitBuffer::from_bitstr(test_vec);
        let pdu = DLocationUpdateCommand::from_bitbuf(&mut buf_in).expect("Failed parsing D-LOCATION UPDATE COMMAND");

        assert!(pdu.group_identity_report);
        assert!(!pdu.cipher_control);
        assert_eq!(pdu.ciphering_parameters, None);
        assert_eq!(pdu.address_extension, None);
        assert_eq!(pdu.cell_type_control, None);
        assert_eq!(pdu.proprietary, None);
        assert_eq!(buf_in.get_len_remaining(), 0);

        let mut buf_out = BitBuffer::new_autoexpand(16);
        pdu.to_bitbuf(&mut buf_out).unwrap();
        assert_eq!(buf_out.to_bitstr(), test_vec);
    }

    #[test]
    fn test_d_location_update_command_with_cipher_parameters_round_trips() {
        debug::setup_logging_verbose();

        let pdu = DLocationUpdateCommand {
            group_identity_report: false,
            cipher_control: true,
            ciphering_parameters: Some(0b10_1010_1010),
            address_extension: None,
            cell_type_control: None,
            proprietary: None,
        };
        let mut buf_out = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut buf_out).unwrap();

        let mut buf_in = BitBuffer::from_bitstr(&buf_out.to_bitstr());
        let parsed = DLocationUpdateCommand::from_bitbuf(&mut buf_in).expect("Failed parsing D-LOCATION UPDATE COMMAND");

        assert!(!parsed.group_identity_report);
        assert!(parsed.cipher_control);
        assert_eq!(parsed.ciphering_parameters, Some(0b10_1010_1010));
        assert_eq!(parsed.address_extension, None);
        assert_eq!(parsed.cell_type_control, None);
        assert_eq!(parsed.proprietary, None);
        assert_eq!(buf_in.get_len_remaining(), 0);
    }

    #[test]
    fn test_d_location_update_command_with_type3_optionals_round_trips() {
        debug::setup_logging_verbose();

        let pdu = DLocationUpdateCommand {
            group_identity_report: true,
            cipher_control: false,
            ciphering_parameters: None,
            address_extension: Some(0x00AB_CDEF),
            cell_type_control: Some(Type3FieldGeneric {
                field_id: MmType34ElemIdDl::CellTypeControl.into_raw(),
                len: 3,
                data: 0b101,
            }),
            proprietary: Some(Type3FieldGeneric {
                field_id: MmType34ElemIdDl::Proprietary.into_raw(),
                len: 3,
                data: 0b011,
            }),
        };
        let mut buf_out = BitBuffer::new_autoexpand(96);
        pdu.to_bitbuf(&mut buf_out).unwrap();

        let mut buf_in = BitBuffer::from_bitstr(&buf_out.to_bitstr());
        let parsed = DLocationUpdateCommand::from_bitbuf(&mut buf_in).expect("Failed parsing D-LOCATION UPDATE COMMAND");

        assert!(parsed.group_identity_report);
        assert!(!parsed.cipher_control);
        assert_eq!(parsed.ciphering_parameters, None);
        assert_eq!(parsed.address_extension, Some(0x00AB_CDEF));
        assert_eq!(
            parsed.cell_type_control,
            Some(Type3FieldGeneric {
                field_id: MmType34ElemIdDl::CellTypeControl.into_raw(),
                len: 3,
                data: 0b101,
            })
        );
        assert_eq!(
            parsed.proprietary,
            Some(Type3FieldGeneric {
                field_id: MmType34ElemIdDl::Proprietary.into_raw(),
                len: 3,
                data: 0b011,
            })
        );
        assert_eq!(buf_in.get_len_remaining(), 0);
    }

    #[test]
    fn test_d_location_update_command_rejects_inconsistent_cipher_presence() {
        let missing = DLocationUpdateCommand {
            group_identity_report: true,
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

        let unexpected = DLocationUpdateCommand {
            group_identity_report: true,
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

        let too_large = DLocationUpdateCommand {
            group_identity_report: true,
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
