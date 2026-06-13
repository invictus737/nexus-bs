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
use crate::mm::fields::group_identity_downlink::GroupIdentityDownlink;
use crate::mm::pdus::attach_detach_group_identity_validation::{
    parse_d_attach_detach_group_identity_ack_options, validate_group_identity_downlink_collection, validate_type3_generic_field,
    validate_type4_generic_field,
};

/// Representation of the D-ATTACH/DETACH GROUP IDENTITY ACKNOWLEDGEMENT PDU (Clause 16.9.2.2).
/// The infrastructure sends this message to the MS to acknowledge MS-initiated attachment/detachment of group identities.
/// Response expected: -
/// Response to: U-ATTACH/DETACH GROUP IDENTITY

// Note: The MS shall accept the type 3/4 information elements both in the numerical order as described in annex E and in the order shown in this table.
#[derive(Debug)]
pub struct DAttachDetachGroupIdentityAcknowledgement {
    /// Type1, 1 bits, Group identity accept/reject
    pub group_identity_accept_reject: u8,
    /// Type1, 1 bits, Reserved
    pub reserved: bool,
    /// Type3, See note,
    pub proprietary: Option<Type3FieldGeneric>,
    /// Type4, See note,
    pub group_identity_downlink: Option<Vec<GroupIdentityDownlink>>,
    /// Type4, See ETSI EN 300 392-7 [8] and note,
    pub group_identity_security_related_information: Option<Type4FieldGeneric>,
}

impl DAttachDetachGroupIdentityAcknowledgement {
    fn validate(&self) -> Result<(), PduParseErr> {
        if self.group_identity_accept_reject > 1 {
            return Err(PduParseErr::InvalidValue {
                field: "group_identity_accept_reject",
                value: self.group_identity_accept_reject as u64,
            });
        }
        if self.reserved {
            return Err(PduParseErr::InvalidValue {
                field: "reserved",
                value: 1,
            });
        }
        validate_type3_generic_field("proprietary", &self.proprietary, MmType34ElemIdDl::Proprietary.into_raw(), None)?;
        validate_group_identity_downlink_collection("group_identity_downlink", &self.group_identity_downlink)?;
        validate_type4_generic_field(
            "group_identity_security_related_information",
            &self.group_identity_security_related_information,
            MmType34ElemIdDl::GroupIdentitySecurityRelatedInformation.into_raw(),
        )?;
        Ok(())
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(4, "pdu_type")?;
        expect_pdu_type!(pdu_type, MmPduTypeDl::DAttachDetachGroupIdentityAcknowledgement)?;

        // Type1
        let group_identity_accept_reject = buffer.read_field(1, "group_identity_accept_reject")? as u8;
        // Type1
        let reserved = buffer.read_field(1, "reserved")? != 0;

        // EN 300 392-2 Annex E orders Type3/4 IEs numerically. Clause
        // 16.9.2.2 note also requires accepting the table order.
        let obit = delimiters::read_obit(buffer)?;
        let (proprietary, group_identity_downlink, group_identity_security_related_information) =
            parse_d_attach_detach_group_identity_ack_options(obit, buffer)?;

        let pdu = DAttachDetachGroupIdentityAcknowledgement {
            group_identity_accept_reject,
            reserved,
            proprietary,
            group_identity_downlink,
            group_identity_security_related_information,
        };
        pdu.validate()?;
        Ok(pdu)
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // PDU Type
        buffer.write_bits(MmPduTypeDl::DAttachDetachGroupIdentityAcknowledgement.into_raw(), 4);
        // Type1
        buffer.write_bits(self.group_identity_accept_reject as u64, 1);
        // Type1
        buffer.write_bits(self.reserved as u64, 1);

        // Check if any optional field present and place o-bit
        let obit = self.proprietary.is_some()
            || self.group_identity_downlink.is_some()
            || self.group_identity_security_related_information.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type3/4 elements are emitted in Annex E numerical order.
        typed::write_type4_struct(
            obit,
            buffer,
            &self.group_identity_downlink,
            MmType34ElemIdDl::GroupIdentityDownlink,
            GroupIdentityDownlink::to_bitbuf,
        )?;

        // Type4
        typed::write_type4_todo(
            obit,
            buffer,
            &self.group_identity_security_related_information,
            MmType34ElemIdDl::GroupIdentitySecurityRelatedInformation,
        )?;

        // Type3
        typed::write_type3_generic(obit, buffer, &self.proprietary, MmType34ElemIdDl::Proprietary)?;

        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for DAttachDetachGroupIdentityAcknowledgement {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DAttachDetachGroupIdentityAcknowledgement {{ group_identity_accept_reject: {:?} reserved: {:?} proprietary: {:?} group_identity_downlink: {:?} group_identity_security_related_information: {:?} }}",
            self.group_identity_accept_reject,
            self.reserved,
            self.proprietary,
            self.group_identity_downlink,
            self.group_identity_security_related_information,
        )
    }
}

#[cfg(test)]
mod tests {
    use tetra_core::debug;

    use super::*;

    #[test]
    fn test_d_attach_detach_group_identity_ack() {
        // 10110011011100000100110000001011100000000110101000110011100000
        // |--|         identifier
        //     |        accept/reject
        //      |       reserved
        //       ||                                                         obit, mbit
        //         |--|                                                     identifier: 0x7 GroupIdentityDownlink
        //             |---------|                                          len: 38
        //                        |------------------------------------|    field
        //                                                              |   closing mbit
        //
        // 000001 01110010000001010100011001110000
        // |----|           num elems: 1
        //        |         attach/detach type identifier
        //         ||       fetime: until next location update
        //           |-|    class of usage: 4
        //              ||  type identifier
        //                |----------------------| gssi: 0x000000

        // Vec from lab
        debug::setup_logging_verbose();
        let test_vec = "10110011011100000100110000001011100000000110101000110011100000";
        let mut buf_in = BitBuffer::from_bitstr(test_vec);
        let pdu = DAttachDetachGroupIdentityAcknowledgement::from_bitbuf(&mut buf_in).expect("Failed parsing");

        tracing::info!("Parsed: {:?}", pdu);
        tracing::info!("Buf at end: {}", buf_in.dump_bin());

        assert!(buf_in.get_len_remaining() == 0, "Buffer not fully consumed");

        let mut buf_out = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut buf_out).unwrap();
        tracing::info!("Serialized: {}", buf_out.dump_bin());
        assert_eq!(buf_out.to_bitstr(), test_vec);
    }

    #[test]
    fn d_attach_detach_group_identity_ack_rejects_reserved_bit_on_serialize() {
        let pdu = DAttachDetachGroupIdentityAcknowledgement {
            group_identity_accept_reject: 0,
            reserved: true,
            proprietary: None,
            group_identity_downlink: None,
            group_identity_security_related_information: None,
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "reserved",
                value: 1,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_attach_detach_group_identity_ack_rejects_overwide_accept_reject_on_serialize() {
        let pdu = DAttachDetachGroupIdentityAcknowledgement {
            group_identity_accept_reject: 2,
            reserved: false,
            proprietary: None,
            group_identity_downlink: None,
            group_identity_security_related_information: None,
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "group_identity_accept_reject",
                value: 2,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_attach_detach_group_identity_ack_accepts_table_order_type34_elements() {
        let proprietary = Type3FieldGeneric {
            field_id: MmType34ElemIdDl::Proprietary.into_raw(),
            len: 3,
            data: 0b101,
        };
        let downlink = vec![GroupIdentityDownlink {
            group_identity_attachment: Some(crate::mm::fields::group_identity_attachment::GroupIdentityAttachment {
                group_identity_attachment_lifetime: 0,
                class_of_usage: 0,
            }),
            group_identity_detachment_uplink: None,
            gssi: Some(0x0012_3456),
            address_extension: None,
            vgssi: None,
        }];
        let security = Type4FieldGeneric {
            field_id: MmType34ElemIdDl::GroupIdentitySecurityRelatedInformation.into_raw(),
            len: 3,
            elems: 1,
            data: 0b110,
        };
        let mut buf = BitBuffer::new_autoexpand(160);
        buf.write_bits(MmPduTypeDl::DAttachDetachGroupIdentityAcknowledgement.into_raw(), 4);
        buf.write_bits(0, 1);
        buf.write_bits(0, 1);
        delimiters::write_obit(&mut buf, 1);
        typed::write_type3_generic(true, &mut buf, &Some(proprietary), MmType34ElemIdDl::Proprietary).unwrap();
        typed::write_type4_struct(
            true,
            &mut buf,
            &Some(downlink),
            MmType34ElemIdDl::GroupIdentityDownlink,
            GroupIdentityDownlink::to_bitbuf,
        )
        .unwrap();
        typed::write_type4_todo(
            true,
            &mut buf,
            &Some(security),
            MmType34ElemIdDl::GroupIdentitySecurityRelatedInformation,
        )
        .unwrap();
        delimiters::write_mbit(&mut buf, 0);
        buf.seek(0);

        let parsed = DAttachDetachGroupIdentityAcknowledgement::from_bitbuf(&mut buf).expect("parse table-order D-ATTACH/DETACH ACK");

        assert!(parsed.proprietary.is_some());
        assert_eq!(parsed.group_identity_downlink.expect("group identity downlink").len(), 1);
        assert!(parsed.group_identity_security_related_information.is_some());
    }

    #[test]
    fn d_attach_detach_group_identity_ack_serializes_type34_elements_in_numeric_order() {
        let pdu = DAttachDetachGroupIdentityAcknowledgement {
            group_identity_accept_reject: 1,
            reserved: false,
            proprietary: Some(Type3FieldGeneric {
                field_id: MmType34ElemIdDl::Proprietary.into_raw(),
                len: 3,
                data: 0b101,
            }),
            group_identity_downlink: Some(vec![GroupIdentityDownlink {
                group_identity_attachment: Some(crate::mm::fields::group_identity_attachment::GroupIdentityAttachment {
                    group_identity_attachment_lifetime: 0,
                    class_of_usage: 0,
                }),
                group_identity_detachment_uplink: None,
                gssi: Some(0x0012_3456),
                address_extension: None,
                vgssi: None,
            }]),
            group_identity_security_related_information: None,
        };
        let mut buf = BitBuffer::new_autoexpand(128);

        pdu.to_bitbuf(&mut buf).expect("serialize D-ATTACH/DETACH ACK");

        assert_eq!(
            buf.peek_bits_startoffset(8, 4),
            Some(MmType34ElemIdDl::GroupIdentityDownlink.into_raw())
        );
    }
}
