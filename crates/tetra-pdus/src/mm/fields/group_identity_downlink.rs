use core::fmt;

use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mm::fields::group_identity_attachment::GroupIdentityAttachment;

const GIAT_GSSI: u8 = 0;
const GIAT_GTSI: u8 = 1;
const GIAT_VGSSI: u8 = 2;
const GIAT_GTSI_VGSSI: u8 = 3;
const MAX_U2: u8 = 0x03;
const MAX_U3: u8 = 0x07;
const MAX_U24: u32 = 0x00ff_ffff;

/// 16.10.22 Group identity downlink
#[derive(Debug, Clone)]
pub struct GroupIdentityDownlink {
    // 1
    // pub attach_detach_type_identifier: u8,
    // 5 opt
    pub group_identity_attachment: Option<GroupIdentityAttachment>,
    // 2 opt
    pub group_identity_detachment_uplink: Option<u8>,
    // 2
    // pub group_identity_address_type: u8,
    // 24 opt
    pub gssi: Option<u32>,
    // 24 opt
    pub address_extension: Option<u32>,
    // 24 opt
    pub vgssi: Option<u32>,
}

impl GroupIdentityDownlink {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 table 16.54 note 1: GIADTI selects exactly one
        // attachment/detachment subfield.
        match (&self.group_identity_attachment, self.group_identity_detachment_uplink) {
            (Some(attachment), None) if attachment.group_identity_attachment_lifetime <= MAX_U2 && attachment.class_of_usage <= MAX_U3 => {}
            (Some(attachment), None) if attachment.group_identity_attachment_lifetime > MAX_U2 => {
                return Err(PduParseErr::InvalidValue {
                    field: "group_identity_attachment_lifetime",
                    value: attachment.group_identity_attachment_lifetime as u64,
                });
            }
            (Some(attachment), None) => {
                return Err(PduParseErr::InvalidValue {
                    field: "class_of_usage",
                    value: attachment.class_of_usage as u64,
                });
            }
            (None, Some(detachment)) if detachment <= MAX_U2 => {}
            (None, Some(detachment)) => {
                return Err(PduParseErr::InvalidValue {
                    field: "group_identity_detachment_downlink",
                    value: detachment as u64,
                });
            }
            (None, None) => {
                return Err(PduParseErr::FieldNotPresent {
                    field: Some("group_identity_attachment_or_group_identity_detachment_downlink"),
                });
            }
            (Some(_), Some(_)) => {
                return Err(PduParseErr::Inconsistency {
                    field: "group_identity_detachment_downlink",
                    reason: "group_identity_attachment and group_identity_detachment_downlink are mutually exclusive",
                });
            }
        }

        // EN 300 392-2 table 16.54 note 2 defines all four downlink GIAT
        // address shapes, including GTSI-V(GSSI).
        if let Some(gssi) = self.gssi {
            if gssi > MAX_U24 {
                return Err(PduParseErr::InvalidValue {
                    field: "gssi",
                    value: gssi as u64,
                });
            }
        }
        if let Some(extension) = self.address_extension {
            if extension > MAX_U24 {
                return Err(PduParseErr::InvalidValue {
                    field: "address_extension",
                    value: extension as u64,
                });
            }
        }
        if let Some(vgssi) = self.vgssi {
            if vgssi > MAX_U24 {
                return Err(PduParseErr::InvalidValue {
                    field: "vgssi",
                    value: vgssi as u64,
                });
            }
        }
        match (self.gssi.is_some(), self.address_extension.is_some(), self.vgssi.is_some()) {
            (true, false, false) | (true, true, false) | (false, false, true) | (true, true, true) => {}
            (true, false, true) => {
                return Err(PduParseErr::Inconsistency {
                    field: "vgssi",
                    reason: "vgssi with GSSI also requires address_extension",
                });
            }
            (false, true, _) => {
                return Err(PduParseErr::Inconsistency {
                    field: "address_extension",
                    reason: "address_extension is valid only with GSSI",
                });
            }
            (false, false, false) => {
                return Err(PduParseErr::FieldNotPresent {
                    field: Some("gssi_or_vgssi"),
                });
            }
        }

        Ok(())
    }

    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let mut s = GroupIdentityDownlink {
            // attach_detach_type_identifier: 0,
            group_identity_attachment: None,
            group_identity_detachment_uplink: None,
            // group_identity_address_type: 0,
            gssi: None,
            address_extension: None,
            vgssi: None,
        };

        let attach_detach_type_identifier = buf.read_field(1, "attach_detach_type_identifier")? as u8;
        if attach_detach_type_identifier == 0 {
            s.group_identity_attachment = Some(GroupIdentityAttachment::from_bitbuf(buf)?);
        }
        if attach_detach_type_identifier == 1 {
            s.group_identity_detachment_uplink = Some(buf.read_field(2, "attach_detach_type_identifier")? as u8);
        }

        let address_type = buf.read_field(2, "address_type")? as u8;
        if address_type == GIAT_GSSI || address_type == GIAT_GTSI || address_type == GIAT_GTSI_VGSSI {
            s.gssi = Some(buf.read_field(24, "gssi")? as u32);
        }
        if address_type == GIAT_GTSI || address_type == GIAT_GTSI_VGSSI {
            s.address_extension = Some(buf.read_field(24, "address_extension")? as u32);
        }
        if address_type == GIAT_VGSSI || address_type == GIAT_GTSI_VGSSI {
            s.vgssi = Some(buf.read_field(24, "vgssi")? as u32);
        }

        s.validate()?;
        Ok(s)
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        buf.write_bits(if self.group_identity_attachment.is_some() { 0 } else { 1 }, 1);
        if let Some(v) = &self.group_identity_attachment {
            v.to_bitbuf(buf);
        }
        if let Some(v) = self.group_identity_detachment_uplink {
            buf.write_bits(v as u64, 2);
        }

        let address_type = if self.gssi.is_some() {
            if self.address_extension.is_some() {
                if self.vgssi.is_some() { GIAT_GTSI_VGSSI } else { GIAT_GTSI }
            } else {
                GIAT_GSSI
            }
        } else {
            GIAT_VGSSI
        };

        buf.write_bits(address_type as u64, 2);
        if let Some(v) = self.gssi {
            buf.write_bits(v as u64, 24);
        }
        if let Some(v) = self.address_extension {
            buf.write_bits(v as u64, 24);
        }
        if let Some(v) = self.vgssi {
            buf.write_bits(v as u64, 24);
        }

        Ok(())
    }
}

impl fmt::Display for GroupIdentityDownlink {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "group_identity_downlink {{ group_identity_attachment: {:?} group_identity_detachment_uplink: {:?} gssi: {:?} address_extension: {:?} vgssi: {:?} }}",
            self.group_identity_attachment, self.group_identity_detachment_uplink, self.gssi, self.address_extension, self.vgssi
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment() -> GroupIdentityAttachment {
        GroupIdentityAttachment {
            group_identity_attachment_lifetime: 3,
            class_of_usage: 0,
        }
    }

    fn base_group_identity_downlink() -> GroupIdentityDownlink {
        GroupIdentityDownlink {
            group_identity_attachment: Some(attachment()),
            group_identity_detachment_uplink: None,
            gssi: Some(0x00aa_bbcc),
            address_extension: None,
            vgssi: None,
        }
    }

    #[test]
    fn group_identity_downlink_roundtrips_gtsi_vgssi_shape() {
        let pdu = GroupIdentityDownlink {
            address_extension: Some(0x0012_3456),
            vgssi: Some(0x0065_4321),
            ..base_group_identity_downlink()
        };
        let mut buf = BitBuffer::new_autoexpand(96);

        pdu.to_bitbuf(&mut buf).expect("serialize GroupIdentityDownlink");
        buf.seek(0);
        let decoded = GroupIdentityDownlink::from_bitbuf(&mut buf).expect("parse GroupIdentityDownlink");

        assert!(decoded.group_identity_attachment.is_some());
        assert_eq!(decoded.gssi, Some(0x00aa_bbcc));
        assert_eq!(decoded.address_extension, Some(0x0012_3456));
        assert_eq!(decoded.vgssi, Some(0x0065_4321));
    }

    #[test]
    fn group_identity_downlink_rejects_vgssi_with_gssi_without_extension() {
        let pdu = GroupIdentityDownlink {
            vgssi: Some(0x0012_3456),
            ..base_group_identity_downlink()
        };
        let mut buf = BitBuffer::new_autoexpand(96);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::Inconsistency {
                field: "vgssi",
                reason: "vgssi with GSSI also requires address_extension",
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn group_identity_downlink_rejects_missing_attachment_or_detachment_selector() {
        let pdu = GroupIdentityDownlink {
            group_identity_attachment: None,
            ..base_group_identity_downlink()
        };
        let mut buf = BitBuffer::new_autoexpand(64);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::FieldNotPresent {
                field: Some("group_identity_attachment_or_group_identity_detachment_downlink"),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn group_identity_downlink_rejects_overwide_attachment_class() {
        let pdu = GroupIdentityDownlink {
            group_identity_attachment: Some(GroupIdentityAttachment {
                group_identity_attachment_lifetime: 3,
                class_of_usage: 8,
            }),
            ..base_group_identity_downlink()
        };
        let mut buf = BitBuffer::new_autoexpand(64);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "class_of_usage",
                value: 8,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }
}
