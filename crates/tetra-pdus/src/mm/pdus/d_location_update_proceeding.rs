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
use crate::mm::pdus::attach_detach_group_identity_validation::validate_type3_generic_field;

const MAX_U24: u32 = 0x00ff_ffff;

/// Representation of the D-LOCATION UPDATE PROCEEDING PDU (Clause 16.9.2.10).
/// The infrastructure sends this message to the MS on registration at accepted migration to assign a (V)ASSI.
/// Response expected: -
/// Response to: U-LOCATION UPDATE DEMAND

#[derive(Debug)]
pub struct DLocationUpdateProceeding {
    /// Type1, 24 bits, (V)ASSI of the MS,
    pub ssi: u32,
    /// Type1, 24 bits, MNI of the MS,
    pub address_extension: u32,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl DLocationUpdateProceeding {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 clause 16.9.2.10/table 16.14 encodes both identity
        // fields as 24-bit Type-1 information elements.
        if self.ssi > MAX_U24 {
            return Err(PduParseErr::InvalidValue {
                field: "ssi",
                value: self.ssi as u64,
            });
        }
        if self.address_extension > MAX_U24 {
            return Err(PduParseErr::InvalidValue {
                field: "address_extension",
                value: self.address_extension as u64,
            });
        }
        validate_type3_generic_field("proprietary", &self.proprietary, MmType34ElemIdDl::Proprietary.into_raw(), None)?;
        Ok(())
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(4, "pdu_type")?;
        expect_pdu_type!(pdu_type, MmPduTypeDl::DLocationUpdateProceeding)?;

        // Type1
        let ssi = buffer.read_field(24, "ssi")? as u32;
        // Type1
        let address_extension = buffer.read_field(24, "address_extension")? as u32;

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type3
        let proprietary = typed::parse_type3_generic(obit, buffer, MmType34ElemIdDl::Proprietary)?;

        // Read trailing mbit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        let pdu = DLocationUpdateProceeding {
            ssi,
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
        buffer.write_bits(MmPduTypeDl::DLocationUpdateProceeding.into_raw(), 4);
        // Type1
        buffer.write_bits(self.ssi as u64, 24);
        // Type1
        buffer.write_bits(self.address_extension as u64, 24);

        // Check if any optional field present and place o-bit
        let obit = self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type3
        typed::write_type3_generic(obit, buffer, &self.proprietary, MmType34ElemIdDl::Proprietary)?;

        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for DLocationUpdateProceeding {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DLocationUpdateProceeding {{ ssi: {:?} address_extension: {:?} proprietary: {:?} }}",
            self.ssi, self.address_extension, self.proprietary,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d_location_update_proceeding_roundtrips_type1_fields() {
        let pdu = DLocationUpdateProceeding {
            ssi: 0x0012_3456,
            address_extension: 0x00ab_cdef,
            proprietary: None,
        };
        let mut buf = BitBuffer::new_autoexpand(64);

        pdu.to_bitbuf(&mut buf).expect("serialize D-LOCATION UPDATE PROCEEDING");
        buf.seek(0);
        let parsed = DLocationUpdateProceeding::from_bitbuf(&mut buf).expect("parse D-LOCATION UPDATE PROCEEDING");

        assert_eq!(parsed.ssi, 0x0012_3456);
        assert_eq!(parsed.address_extension, 0x00ab_cdef);
        assert!(parsed.proprietary.is_none());
        assert_eq!(buf.get_len_remaining(), 0);
    }

    #[test]
    fn d_location_update_proceeding_rejects_overwide_ssi() {
        let pdu = DLocationUpdateProceeding {
            ssi: 0x0100_0000,
            address_extension: 0,
            proprietary: None,
        };
        let mut buf = BitBuffer::new_autoexpand(64);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "ssi",
                value: 0x0100_0000,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_location_update_proceeding_rejects_overwide_address_extension() {
        let pdu = DLocationUpdateProceeding {
            ssi: 0,
            address_extension: 0x0100_0000,
            proprietary: None,
        };
        let mut buf = BitBuffer::new_autoexpand(64);

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
    fn d_location_update_proceeding_rejects_wrong_proprietary_id() {
        let pdu = DLocationUpdateProceeding {
            ssi: 0,
            address_extension: 0,
            proprietary: Some(Type3FieldGeneric {
                field_id: MmType34ElemIdDl::GroupReportResponse.into_raw(),
                len: 1,
                data: 0,
            }),
        };
        let mut buf = BitBuffer::new_autoexpand(64);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "proprietary",
                value: MmType34ElemIdDl::GroupReportResponse.into_raw(),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }
}
