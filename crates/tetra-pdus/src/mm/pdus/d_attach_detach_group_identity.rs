use core::fmt;

use tetra_core::expect_pdu_type;
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mm::enums::mm_pdu_type_dl::MmPduTypeDl;
use crate::mm::enums::type34_elem_id_dl::MmType34ElemIdDl;
use crate::mm::fields::group_identity_downlink::GroupIdentityDownlink;
use crate::mm::pdus::attach_detach_group_identity_validation::{
    parse_d_attach_detach_group_identity_options, validate_group_identity_downlink_collection, validate_group_report_response,
    validate_type3_generic_field, validate_type4_generic_field,
};

/// Representation of the D-ATTACH/DETACH GROUP IDENTITY PDU (Clause 16.9.2.1).
/// The infrastructure sends this message to the MS to indicate attachment/detachment of group identities for the MS or to initiate a group report request or give a group report response.
/// Response expected: -/U-ATTACH/DETACH GROUP IDENTITY ACKNOWLEDGEMENT
/// Response to: -/U-ATTACH/DETACH GROUP IDENTITY (report request)

// note 1: The MS shall accept the type 3/4 information elements both in the numerical order as described in annex E and in the order shown in this table.
#[derive(Debug)]
pub struct DAttachDetachGroupIdentity {
    /// Type1, 1 bits, Group identity report
    pub group_identity_report: bool,
    /// Type1, 1 bits, Group identity acknowledgement request
    pub group_identity_acknowledgement_request: bool,
    /// Type1, 1 bits, Group identity attach/detach mode
    pub group_identity_attach_detach_mode: bool,
    /// Type3, See note,
    pub proprietary: Option<Type3FieldGeneric>,
    /// Type3, See note,
    pub group_report_response: Option<Type3FieldGeneric>,
    /// Type4, See note,
    pub group_identity_downlink: Option<Vec<GroupIdentityDownlink>>,
    /// Type4, See ETSI EN 300 392-7 [8] and note,
    pub group_identity_security_related_information: Option<Type4FieldGeneric>,
}

impl DAttachDetachGroupIdentity {
    fn validate(&self) -> Result<(), PduParseErr> {
        validate_type3_generic_field("proprietary", &self.proprietary, MmType34ElemIdDl::Proprietary.into_raw(), None)?;
        validate_group_report_response(&self.group_report_response, MmType34ElemIdDl::GroupReportResponse.into_raw())?;
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
        expect_pdu_type!(pdu_type, MmPduTypeDl::DAttachDetachGroupIdentity)?;

        // Type1
        let group_identity_report = buffer.read_field(1, "group_identity_report")? != 0;
        // Type1
        let group_identity_acknowledgement_request = buffer.read_field(1, "group_identity_acknowledgement_request")? != 0;
        // Type1
        let group_identity_attach_detach_mode = buffer.read_field(1, "group_identity_attach_detach_mode")? != 0;

        // EN 300 392-2 Annex E orders Type3/4 IEs numerically. Clause
        // 16.9.2.1 note also requires accepting the table order.
        let obit = delimiters::read_obit(buffer)?;
        let (proprietary, group_report_response, group_identity_downlink, group_identity_security_related_information) =
            parse_d_attach_detach_group_identity_options(obit, buffer)?;

        let pdu = DAttachDetachGroupIdentity {
            group_identity_report,
            group_identity_acknowledgement_request,
            group_identity_attach_detach_mode,
            proprietary,
            group_report_response,
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
        buffer.write_bits(MmPduTypeDl::DAttachDetachGroupIdentity.into_raw(), 4);
        // Type1
        buffer.write_bits(self.group_identity_report as u64, 1);
        // Type1
        buffer.write_bits(self.group_identity_acknowledgement_request as u64, 1);
        // Type1
        buffer.write_bits(self.group_identity_attach_detach_mode as u64, 1);

        // Check if any optional field present and place o-bit
        let obit = self.proprietary.is_some()
            || self.group_report_response.is_some()
            || self.group_identity_downlink.is_some()
            || self.group_identity_security_related_information.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type3/4 elements are emitted in Annex E numerical order.
        typed::write_type3_generic(obit, buffer, &self.group_report_response, MmType34ElemIdDl::GroupReportResponse)?;

        // Type4
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

impl fmt::Display for DAttachDetachGroupIdentity {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DAttachDetachGroupIdentity {{ group_identity_report: {:?} group_identity_acknowledgement_request: {:?} group_identity_attach_detach_mode: {:?} proprietary: {:?} group_report_response: {:?} group_identity_downlink: {:?} group_identity_security_related_information: {:?} }}",
            self.group_identity_report,
            self.group_identity_acknowledgement_request,
            self.group_identity_attach_detach_mode,
            self.proprietary,
            self.group_report_response,
            self.group_identity_downlink,
            self.group_identity_security_related_information,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mm::fields::group_identity_attachment::GroupIdentityAttachment;

    fn downlink_group() -> GroupIdentityDownlink {
        GroupIdentityDownlink {
            group_identity_attachment: Some(GroupIdentityAttachment {
                group_identity_attachment_lifetime: 0,
                class_of_usage: 0,
            }),
            group_identity_detachment_uplink: None,
            gssi: Some(0x0012_3456),
            address_extension: None,
            vgssi: None,
        }
    }

    #[test]
    fn d_attach_detach_group_identity_rejects_wrong_group_report_response_id() {
        let pdu = DAttachDetachGroupIdentity {
            group_identity_report: false,
            group_identity_acknowledgement_request: false,
            group_identity_attach_detach_mode: true,
            proprietary: None,
            group_report_response: Some(Type3FieldGeneric {
                field_id: MmType34ElemIdDl::Proprietary.into_raw(),
                len: 1,
                data: 0,
            }),
            group_identity_downlink: None,
            group_identity_security_related_information: None,
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "group_report_response",
                value: MmType34ElemIdDl::Proprietary.into_raw(),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_attach_detach_group_identity_rejects_empty_downlink_collection() {
        let pdu = DAttachDetachGroupIdentity {
            group_identity_report: false,
            group_identity_acknowledgement_request: false,
            group_identity_attach_detach_mode: true,
            proprietary: None,
            group_report_response: None,
            group_identity_downlink: Some(vec![]),
            group_identity_security_related_information: None,
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "group_identity_downlink",
                value: 0,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_attach_detach_group_identity_roundtrips_valid_group_report_complete() {
        let pdu = DAttachDetachGroupIdentity {
            group_identity_report: false,
            group_identity_acknowledgement_request: false,
            group_identity_attach_detach_mode: true,
            proprietary: None,
            group_report_response: Some(Type3FieldGeneric {
                field_id: MmType34ElemIdDl::GroupReportResponse.into_raw(),
                len: 1,
                data: 0,
            }),
            group_identity_downlink: Some(vec![downlink_group()]),
            group_identity_security_related_information: None,
        };
        let mut buf = BitBuffer::new_autoexpand(128);

        pdu.to_bitbuf(&mut buf).expect("serialize D-ATTACH/DETACH GROUP IDENTITY");
        buf.seek(0);
        let parsed = DAttachDetachGroupIdentity::from_bitbuf(&mut buf).expect("parse D-ATTACH/DETACH GROUP IDENTITY");

        assert!(parsed.group_identity_attach_detach_mode);
        let response = parsed.group_report_response.expect("group report response should be present");
        assert_eq!(response.len, 1);
        assert_eq!(response.data, 0);
        assert_eq!(parsed.group_identity_downlink.expect("downlink group should be present").len(), 1);
    }

    #[test]
    fn d_attach_detach_group_identity_accepts_table_order_type34_elements() {
        let proprietary = Type3FieldGeneric {
            field_id: MmType34ElemIdDl::Proprietary.into_raw(),
            len: 3,
            data: 0b101,
        };
        let group_report_response = Type3FieldGeneric {
            field_id: MmType34ElemIdDl::GroupReportResponse.into_raw(),
            len: 1,
            data: 0,
        };
        let security = Type4FieldGeneric {
            field_id: MmType34ElemIdDl::GroupIdentitySecurityRelatedInformation.into_raw(),
            len: 3,
            elems: 1,
            data: 0b110,
        };
        let downlink = vec![downlink_group()];
        let mut buf = BitBuffer::new_autoexpand(160);
        buf.write_bits(MmPduTypeDl::DAttachDetachGroupIdentity.into_raw(), 4);
        buf.write_bits(0, 1);
        buf.write_bits(0, 1);
        buf.write_bits(1, 1);
        delimiters::write_obit(&mut buf, 1);
        typed::write_type3_generic(true, &mut buf, &Some(proprietary), MmType34ElemIdDl::Proprietary).unwrap();
        typed::write_type3_generic(true, &mut buf, &Some(group_report_response), MmType34ElemIdDl::GroupReportResponse).unwrap();
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

        let parsed = DAttachDetachGroupIdentity::from_bitbuf(&mut buf).expect("parse table-order D-ATTACH/DETACH");

        assert!(parsed.proprietary.is_some());
        assert_eq!(parsed.group_report_response.expect("group report response").data, 0);
        assert_eq!(parsed.group_identity_downlink.expect("group identity downlink").len(), 1);
        assert!(parsed.group_identity_security_related_information.is_some());
    }

    #[test]
    fn d_attach_detach_group_identity_serializes_type34_elements_in_numeric_order() {
        let pdu = DAttachDetachGroupIdentity {
            group_identity_report: false,
            group_identity_acknowledgement_request: false,
            group_identity_attach_detach_mode: true,
            proprietary: Some(Type3FieldGeneric {
                field_id: MmType34ElemIdDl::Proprietary.into_raw(),
                len: 3,
                data: 0b101,
            }),
            group_report_response: Some(Type3FieldGeneric {
                field_id: MmType34ElemIdDl::GroupReportResponse.into_raw(),
                len: 1,
                data: 0,
            }),
            group_identity_downlink: Some(vec![downlink_group()]),
            group_identity_security_related_information: None,
        };
        let mut buf = BitBuffer::new_autoexpand(128);

        pdu.to_bitbuf(&mut buf).expect("serialize D-ATTACH/DETACH");

        assert_eq!(
            buf.peek_bits_startoffset(9, 4),
            Some(MmType34ElemIdDl::GroupReportResponse.into_raw())
        );
    }
}
