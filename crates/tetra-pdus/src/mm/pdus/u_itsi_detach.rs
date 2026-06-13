// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::expect_pdu_type;
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mm::enums::mm_pdu_type_ul::MmPduTypeUl;
use crate::mm::enums::type34_elem_id_ul::MmType34ElemIdUl;
use crate::mm::pdus::attach_detach_group_identity_validation::validate_type3_generic_field;

const MAX_U24: u64 = 0x00ff_ffff;

/// Representation of the U-ITSI DETACH PDU (Clause 16.9.3.3).
/// The MS sends this message to the infrastructure to announce that the MS will be de-activated.
/// Response expected: -/D-MM STATUS
/// Response to: -

#[derive(Debug)]
pub struct UItsiDetach {
    /// Type2, 24 bits, MNI of the MS (MCC followed by MNC)
    pub address_extension: Option<u64>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl UItsiDetach {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 clause 16.9.3.3/table 16.17 encodes Address
        // extension (MNI) as a 24-bit Type-2 information element.
        if let Some(address_extension) = self.address_extension {
            if address_extension > MAX_U24 {
                return Err(PduParseErr::InvalidValue {
                    field: "address_extension",
                    value: address_extension,
                });
            }
        }
        validate_type3_generic_field("proprietary", &self.proprietary, MmType34ElemIdUl::Proprietary.into_raw(), None)?;
        Ok(())
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(4, "pdu_type")?;
        expect_pdu_type!(pdu_type, MmPduTypeUl::UItsiDetach)?;

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type2
        let address_extension = typed::parse_type2_generic(obit, buffer, 24, "address_extension")?;

        // Type3
        let proprietary = typed::parse_type3_generic(obit, buffer, MmType34ElemIdUl::Proprietary)?;

        // Read trailing mbit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        let pdu = UItsiDetach {
            address_extension,
            proprietary,
        };
        pdu.validate()?;
        Ok(pdu)
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // PDU Type
        buffer.write_bits(MmPduTypeUl::UItsiDetach.into_raw(), 4);

        // Check if any optional field present and place o-bit
        let obit = self.address_extension.is_some() || self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2
        typed::write_type2_generic(obit, buffer, self.address_extension, 24);

        // Type3
        typed::write_type3_generic(obit, buffer, &self.proprietary, MmType34ElemIdUl::Proprietary)?;

        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for UItsiDetach {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "UItsiDetach {{ address_extension: {:?} proprietary: {:?} }}",
            self.address_extension, self.proprietary,
        )
    }
}

#[cfg(test)]
mod tests {
    use tetra_core::debug;

    use super::*;

    #[test]
    fn test_u_itsi_detach() {
        debug::setup_logging_verbose();
        let test_vec = "0001110011001100000101001110010";
        let mut buf_in = BitBuffer::from_bitstr(test_vec);
        let pdu = UItsiDetach::from_bitbuf(&mut buf_in).expect("Failed parsing");

        tracing::info!("Parsed: {:?}", pdu);
        tracing::info!("Buf at end: {}", buf_in.dump_bin());

        assert!(buf_in.get_len_remaining() == 0, "Buffer not fully consumed");

        let mut buf_out = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut buf_out).unwrap();
        tracing::info!("Serialized: {}", buf_out.dump_bin());
        assert_eq!(buf_out.to_bitstr(), test_vec);
    }

    #[test]
    fn u_itsi_detach_rejects_overwide_address_extension() {
        let pdu = UItsiDetach {
            address_extension: Some(0x0100_0000),
            proprietary: None,
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "address_extension",
                value: 0x0100_0000,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn u_itsi_detach_rejects_wrong_proprietary_id() {
        let pdu = UItsiDetach {
            address_extension: None,
            proprietary: Some(Type3FieldGeneric {
                field_id: MmType34ElemIdUl::GroupReportResponse.into_raw(),
                len: 1,
                data: 0,
            }),
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "proprietary",
                value: MmType34ElemIdUl::GroupReportResponse.into_raw(),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn u_itsi_detach_rejects_zero_length_proprietary_ie() {
        let pdu = UItsiDetach {
            address_extension: None,
            proprietary: Some(Type3FieldGeneric {
                field_id: MmType34ElemIdUl::Proprietary.into_raw(),
                len: 0,
                data: 0,
            }),
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InconsistentLength { expected: 1, found: 0 })
        );
        assert_eq!(buf.get_len(), 0);
    }
}
