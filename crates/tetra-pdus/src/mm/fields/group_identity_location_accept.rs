// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use crate::mm::{enums::type34_elem_id_dl::MmType34ElemIdDl, fields::group_identity_downlink::GroupIdentityDownlink};
use tetra_core::expect_value;
use tetra_core::typed_pdu_fields::{delimiters, typed};
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

/// Representation of the Group identity location accept PDU (Clause 16.10.23).
/// The group identity location accept information element shall be a collection of sub elements.
#[derive(Debug)]
pub struct GroupIdentityLocationAccept {
    /// Type1, 1 bit. 0 = accept, 1 = reject
    pub group_identity_accept_reject: u8,
    /// Type1, 1 bits, reserved
    // pub reserved: bool,
    /// Type4, Group identity downlink
    pub group_identity_downlink: Option<Vec<GroupIdentityDownlink>>,
}

impl GroupIdentityLocationAccept {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 table 16.55: accept/reject and reserved are one-bit
        // fields; reserved is serialized as zero and rejected on parse.
        if self.group_identity_accept_reject > 1 {
            return Err(PduParseErr::InvalidValue {
                field: "group_identity_accept_reject",
                value: self.group_identity_accept_reject as u64,
            });
        }
        if self.group_identity_downlink.as_ref().is_some_and(Vec::is_empty) {
            return Err(PduParseErr::InvalidValue {
                field: "group_identity_downlink",
                value: 0,
            });
        }
        Ok(())
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        // Type1
        let group_identity_accept_reject = buffer.read_field(1, "group_identity_accept_reject")? as u8;

        // Type1
        let reserved = buffer.read_field(1, "reserved")?;
        expect_value!(reserved, 0)?;

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type4
        let group_identity_downlink = typed::parse_type4_struct(
            obit,
            buffer,
            MmType34ElemIdDl::GroupIdentityDownlink,
            GroupIdentityDownlink::from_bitbuf,
        )?;

        // Read trailing mbit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }
        let pdu = GroupIdentityLocationAccept {
            group_identity_accept_reject,
            // reserved: reserved,
            group_identity_downlink,
        };
        pdu.validate()?;
        Ok(pdu)
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // Type1
        buffer.write_bits(self.group_identity_accept_reject as u64, 1);
        // Type1, reserved
        buffer.write_bits(0, 1);

        // Check if any optional field present and place o-bit
        let obit = self.group_identity_downlink.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type4
        typed::write_type4_struct(
            obit,
            buffer,
            &self.group_identity_downlink,
            MmType34ElemIdDl::GroupIdentityDownlink,
            GroupIdentityDownlink::to_bitbuf,
        )?;

        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for GroupIdentityLocationAccept {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "GroupIdentityLocationAccept {{ group_identity_accept_reject: {:?} group_identity_downlink: {:?} }}",
            self.group_identity_accept_reject,
            // self.reserved,
            self.group_identity_downlink,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_core::BitBuffer;

    #[test]
    fn group_identity_location_accept_roundtrips_without_downlink_collection() {
        let pdu = GroupIdentityLocationAccept {
            group_identity_accept_reject: 0,
            group_identity_downlink: None,
        };
        let mut buffer = BitBuffer::new_autoexpand(8);

        pdu.to_bitbuf(&mut buffer).expect("serialize GroupIdentityLocationAccept");
        assert_eq!(buffer.get_len(), 3);
        buffer.seek(0);

        let decoded = GroupIdentityLocationAccept::from_bitbuf(&mut buffer).expect("parse GroupIdentityLocationAccept");
        assert_eq!(decoded.group_identity_accept_reject, 0);
        assert!(decoded.group_identity_downlink.is_none());
    }

    #[test]
    fn group_identity_location_accept_rejects_reserved_bit_on_parse() {
        let mut buffer = BitBuffer::new_autoexpand(8);
        buffer.write_bits(0, 1);
        buffer.write_bits(1, 1);
        buffer.write_bits(0, 1);
        buffer.seek(0);

        assert_eq!(
            GroupIdentityLocationAccept::from_bitbuf(&mut buffer).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "reserved",
                value: 1,
            }
        );
    }

    #[test]
    fn group_identity_location_accept_rejects_overwide_accept_reject_on_serialize() {
        let pdu = GroupIdentityLocationAccept {
            group_identity_accept_reject: 2,
            group_identity_downlink: None,
        };
        let mut buffer = BitBuffer::new_autoexpand(8);

        assert_eq!(
            pdu.to_bitbuf(&mut buffer),
            Err(PduParseErr::InvalidValue {
                field: "group_identity_accept_reject",
                value: 2,
            })
        );
        assert_eq!(buffer.get_len(), 0);
    }

    #[test]
    fn group_identity_location_accept_rejects_empty_downlink_collection() {
        let pdu = GroupIdentityLocationAccept {
            group_identity_accept_reject: 0,
            group_identity_downlink: Some(vec![]),
        };
        let mut buffer = BitBuffer::new_autoexpand(8);

        assert_eq!(
            pdu.to_bitbuf(&mut buffer),
            Err(PduParseErr::InvalidValue {
                field: "group_identity_downlink",
                value: 0,
            })
        );
        assert_eq!(buffer.get_len(), 0);
    }
}
