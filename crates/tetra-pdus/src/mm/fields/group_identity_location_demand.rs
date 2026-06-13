// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::expect_value;
use tetra_core::typed_pdu_fields::{delimiters, typed};
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mm::enums::type34_elem_id_ul::MmType34ElemIdUl;
use crate::mm::fields::group_identity_uplink::GroupIdentityUplink;

/// Representation of the Group identity location demand PDU (Clause 16.10.24).
/// The group identity location demand information element shall be a collection of sub elements.
/// Response expected:
/// Response to:

#[derive(Debug)]
pub struct GroupIdentityLocationDemand {
    /// Type1, 1 bits, reserved
    // pub reserved: bool,
    /// Type1, 1 bits, Group identity attach/detach mode
    pub group_identity_attach_detach_mode: u8,
    /// Type4, Group identity uplink
    pub group_identity_uplink: Option<Vec<GroupIdentityUplink>>,
}

impl GroupIdentityLocationDemand {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 table 16.56: attach/detach mode is one bit.
        if self.group_identity_attach_detach_mode > 1 {
            return Err(PduParseErr::InvalidValue {
                field: "group_identity_attach_detach_mode",
                value: self.group_identity_attach_detach_mode as u64,
            });
        }
        if self.group_identity_uplink.as_ref().is_some_and(Vec::is_empty) {
            return Err(PduParseErr::InvalidValue {
                field: "group_identity_uplink",
                value: 0,
            });
        }
        Ok(())
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let reserved = buffer.read_field(1, "reserved")?;
        expect_value!(reserved, 0)?;

        // Type1
        let group_identity_attach_detach_mode = buffer.read_field(1, "group_identity_attach_detach_mode")? as u8;

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type4
        let group_identity_uplink = typed::parse_type4_struct(
            obit,
            buffer,
            MmType34ElemIdUl::GroupIdentityUplink,
            GroupIdentityUplink::from_bitbuf,
        )?;

        // Read trailing mbit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        let pdu = GroupIdentityLocationDemand {
            group_identity_attach_detach_mode,
            group_identity_uplink,
        };
        pdu.validate()?;
        Ok(pdu)
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // Type1
        buffer.write_bits(0, 1);

        // Type1
        buffer.write_bits(self.group_identity_attach_detach_mode as u64, 1);

        // Check if any optional field present and place o-bit
        let obit = self.group_identity_uplink.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type4
        typed::write_type4_struct(
            obit,
            buffer,
            &self.group_identity_uplink,
            MmType34ElemIdUl::GroupIdentityUplink,
            GroupIdentityUplink::to_bitbuf,
        )?;

        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for GroupIdentityLocationDemand {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "GroupIdentityLocationDemand {{ group_identity_attach_detach_mode: {:?} group_identity_uplink: {:?} }}",
            self.group_identity_attach_detach_mode, self.group_identity_uplink,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_from_bitstring() {
        // Same vec as under U-LOCATION UPDATE DEMAND subfield test
        let bitstring = "00111000000001001000000010100000000000000000000000110100";
        let mut buffer = BitBuffer::from_bitstr(bitstring);
        let pdu = GroupIdentityLocationDemand::from_bitbuf(&mut buffer);

        println!("Parsed: {:?}", pdu);
        println!("Buf at end: {}", buffer.dump_bin());

        assert!(pdu.is_ok());
        let parsed = pdu.unwrap();
        assert_eq!(parsed.group_identity_attach_detach_mode, 0);
        assert!(parsed.group_identity_uplink.is_some());
        assert!(buffer.get_len_remaining() == 0);
    }

    #[test]
    fn group_identity_location_demand_rejects_overwide_mode_on_serialize() {
        let pdu = GroupIdentityLocationDemand {
            group_identity_attach_detach_mode: 2,
            group_identity_uplink: None,
        };
        let mut buffer = BitBuffer::new_autoexpand(16);

        assert_eq!(
            pdu.to_bitbuf(&mut buffer),
            Err(PduParseErr::InvalidValue {
                field: "group_identity_attach_detach_mode",
                value: 2,
            })
        );
        assert_eq!(buffer.get_len(), 0);
    }

    #[test]
    fn group_identity_location_demand_rejects_reserved_bit_on_parse() {
        let mut buffer = BitBuffer::new_autoexpand(8);
        buffer.write_bits(1, 1);
        buffer.write_bits(0, 1);
        buffer.write_bits(0, 1);
        buffer.seek(0);

        assert_eq!(
            GroupIdentityLocationDemand::from_bitbuf(&mut buffer).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "reserved",
                value: 1,
            }
        );
    }

    #[test]
    fn group_identity_location_demand_rejects_empty_uplink_collection() {
        let pdu = GroupIdentityLocationDemand {
            group_identity_attach_detach_mode: 1,
            group_identity_uplink: Some(vec![]),
        };
        let mut buffer = BitBuffer::new_autoexpand(16);

        assert_eq!(
            pdu.to_bitbuf(&mut buffer),
            Err(PduParseErr::InvalidValue {
                field: "group_identity_uplink",
                value: 0,
            })
        );
        assert_eq!(buffer.get_len(), 0);
    }
}
