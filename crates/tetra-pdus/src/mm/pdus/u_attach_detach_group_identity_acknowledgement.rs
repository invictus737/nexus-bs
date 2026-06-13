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
use crate::mm::fields::group_identity_uplink::GroupIdentityUplink;
use crate::mm::pdus::attach_detach_group_identity_validation::{validate_group_identity_uplink_collection, validate_type3_generic_field};

/// Representation of the U-ATTACH/DETACH GROUP IDENTITY ACKNOWLEDGEMENT PDU (Clause 16.9.3.2).
/// The MS sends this message to the infrastructure to acknowledge SwMI initiated attachment/detachment of group identities.
/// Response expected: -
/// Response to: D-ATTACH/DETACH GROUP IDENTITY

#[derive(Debug)]
pub struct UAttachDetachGroupIdentityAcknowledgement {
    /// Type1, 1 bits, Group identity acknowledgement type
    pub group_identity_acknowledgement_type: bool,
    /// Type4, Group identity uplink
    pub group_identity_uplink: Option<Vec<GroupIdentityUplink>>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl UAttachDetachGroupIdentityAcknowledgement {
    fn validate(&self) -> Result<(), PduParseErr> {
        validate_group_identity_uplink_collection("group_identity_uplink", &self.group_identity_uplink)?;
        validate_type3_generic_field("proprietary", &self.proprietary, MmType34ElemIdUl::Proprietary.into_raw(), None)?;
        Ok(())
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(4, "pdu_type")?;
        expect_pdu_type!(pdu_type, MmPduTypeUl::UAttachDetachGroupIdentityAcknowledgement)?;

        // Type1
        let group_identity_acknowledgement_type = buffer.read_field(1, "group_identity_acknowledgement_type")? != 0;

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type4
        let group_identity_uplink = typed::parse_type4_struct(
            obit,
            buffer,
            MmType34ElemIdUl::GroupIdentityUplink,
            GroupIdentityUplink::from_bitbuf,
        )?;

        // Type3
        let proprietary = typed::parse_type3_generic(obit, buffer, MmType34ElemIdUl::Proprietary)?;

        // Read trailing mbit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        let pdu = UAttachDetachGroupIdentityAcknowledgement {
            group_identity_acknowledgement_type,
            group_identity_uplink,
            proprietary,
        };
        pdu.validate()?;
        Ok(pdu)
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // PDU Type
        buffer.write_bits(MmPduTypeUl::UAttachDetachGroupIdentityAcknowledgement.into_raw(), 4);
        // Type1
        buffer.write_bits(self.group_identity_acknowledgement_type as u64, 1);

        // Check if any optional field present and place o-bit
        let obit = self.group_identity_uplink.is_some() || self.proprietary.is_some();
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

        // Type3
        typed::write_type3_generic(obit, buffer, &self.proprietary, MmType34ElemIdUl::Proprietary)?;

        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for UAttachDetachGroupIdentityAcknowledgement {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "UAttachDetachGroupIdentityAcknowledgement {{ group_identity_acknowledgement_type: {:?} group_identity_uplink: {:?} proprietary: {:?} }}",
            self.group_identity_acknowledgement_type, self.group_identity_uplink, self.proprietary,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u_attach_detach_group_identity_ack_rejects_empty_uplink_collection() {
        let pdu = UAttachDetachGroupIdentityAcknowledgement {
            group_identity_acknowledgement_type: false,
            group_identity_uplink: Some(vec![]),
            proprietary: None,
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "group_identity_uplink",
                value: 0,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn u_attach_detach_group_identity_ack_rejects_wrong_proprietary_id() {
        let pdu = UAttachDetachGroupIdentityAcknowledgement {
            group_identity_acknowledgement_type: false,
            group_identity_uplink: None,
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
}
