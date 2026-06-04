use core::fmt;

use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

const GIAT_GSSI: u8 = 0;
const GIAT_GTSI: u8 = 1;
const GIAT_VGSSI: u8 = 2;
const GIAT_RESERVED: u8 = 3;
const MAX_U2: u8 = 0x03;
const MAX_U3: u8 = 0x07;
const MAX_U24: u32 = 0x00ff_ffff;

/// 16.10.27 Group identity uplink
#[derive(Debug, Clone)]
pub struct GroupIdentityUplink {
    // 1
    // pub attach_detach_type_identifier: bool,
    // 3 opt
    pub class_of_usage: Option<u8>,
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

impl GroupIdentityUplink {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 table 16.58 note 1: GIADTI selects exactly one
        // attachment/detachment subfield.
        match (self.class_of_usage, self.group_identity_detachment_uplink) {
            (Some(class_of_usage), None) if class_of_usage <= MAX_U3 => {}
            (Some(class_of_usage), None) => {
                return Err(PduParseErr::InvalidValue {
                    field: "class_of_usage",
                    value: class_of_usage as u64,
                });
            }
            (None, Some(detachment)) if detachment <= MAX_U2 => {}
            (None, Some(detachment)) => {
                return Err(PduParseErr::InvalidValue {
                    field: "group_identity_detachment_uplink",
                    value: detachment as u64,
                });
            }
            (None, None) => {
                return Err(PduParseErr::FieldNotPresent {
                    field: Some("class_of_usage_or_group_identity_detachment_uplink"),
                });
            }
            (Some(_), Some(_)) => {
                return Err(PduParseErr::Inconsistency {
                    field: "group_identity_detachment_uplink",
                    reason: "class_of_usage and group_identity_detachment_uplink are mutually exclusive",
                });
            }
        }

        // EN 300 392-2 table 16.58 note 2: uplink GIAT=3 is reserved.
        if let Some(gssi) = self.gssi {
            if gssi > MAX_U24 {
                return Err(PduParseErr::InvalidValue {
                    field: "gssi",
                    value: gssi as u64,
                });
            }
            if let Some(address_extension) = self.address_extension {
                if address_extension > MAX_U24 {
                    return Err(PduParseErr::InvalidValue {
                        field: "address_extension",
                        value: address_extension as u64,
                    });
                }
            }
            if self.vgssi.is_some() {
                return Err(PduParseErr::Inconsistency {
                    field: "vgssi",
                    reason: "uplink GIAT=3 is reserved",
                });
            }
        } else {
            if self.address_extension.is_some() {
                return Err(PduParseErr::Inconsistency {
                    field: "address_extension",
                    reason: "address_extension is valid only with GSSI",
                });
            }
            let vgssi = self.vgssi.ok_or(PduParseErr::FieldNotPresent { field: Some("vgssi") })?;
            if vgssi > MAX_U24 {
                return Err(PduParseErr::InvalidValue {
                    field: "vgssi",
                    value: vgssi as u64,
                });
            }
        }

        Ok(())
    }

    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let mut s = GroupIdentityUplink {
            // attach_detach_type_identifier: false,
            class_of_usage: None,
            group_identity_detachment_uplink: None,
            // group_identity_address_type: 0,
            gssi: None,
            address_extension: None,
            vgssi: None,
        };

        let attach_detach_type_identifier = buf.read_field(1, "attach_detach_type_identifier")?;
        if attach_detach_type_identifier == 0 {
            s.class_of_usage = Some(buf.read_field(3, "class_of_usage")? as u8);
        }
        if attach_detach_type_identifier == 1 {
            s.group_identity_detachment_uplink = Some(buf.read_field(2, "group_identity_detachment_uplink")? as u8);
        }

        let address_type = buf.read_field(2, "address_type")? as u8;
        if address_type == GIAT_RESERVED {
            return Err(PduParseErr::InvalidValue {
                field: "address_type",
                value: address_type as u64,
            });
        }
        if address_type == GIAT_GSSI || address_type == GIAT_GTSI {
            s.gssi = Some(buf.read_field(24, "gssi")? as u32);
        }
        if address_type == GIAT_GTSI {
            s.address_extension = Some(buf.read_field(24, "address_extension")? as u32);
        }
        if address_type == GIAT_VGSSI {
            s.vgssi = Some(buf.read_field(24, "vgssi")? as u32);
        }

        s.validate()?;
        Ok(s)
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        buf.write_bits(if self.class_of_usage.is_some() { 0 } else { 1 }, 1);
        if let Some(v) = self.class_of_usage {
            buf.write_bits(v as u64, 3);
        }
        if let Some(v) = self.group_identity_detachment_uplink {
            buf.write_bits(v as u64, 2);
        }

        let address_type = if self.gssi.is_some() {
            if self.address_extension.is_some() { GIAT_GTSI } else { GIAT_GSSI }
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

impl fmt::Display for GroupIdentityUplink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "group_identity_uplink {{\n  class_of_usage: {:?}\n  group_identity_detachment_uplink: {:?}\n  gssi: {:?}\n  address_extension: {:?}\n  vgssi: {:?}\n}}\n",
            self.class_of_usage, self.group_identity_detachment_uplink, self.gssi, self.address_extension, self.vgssi,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_core::BitBuffer;

    fn base_group_identity_uplink() -> GroupIdentityUplink {
        GroupIdentityUplink {
            class_of_usage: Some(0),
            group_identity_detachment_uplink: None,
            gssi: Some(0x00aa_bbcc),
            address_extension: None,
            vgssi: None,
        }
    }

    #[test]
    fn group_identity_uplink_roundtrips_gssi_attachment() {
        let pdu = base_group_identity_uplink();
        let mut buf = BitBuffer::new_autoexpand(64);

        pdu.to_bitbuf(&mut buf).expect("serialize GroupIdentityUplink");
        buf.seek(0);
        let decoded = GroupIdentityUplink::from_bitbuf(&mut buf).expect("parse GroupIdentityUplink");

        assert_eq!(decoded.class_of_usage, Some(0));
        assert_eq!(decoded.group_identity_detachment_uplink, None);
        assert_eq!(decoded.gssi, Some(0x00aa_bbcc));
        assert_eq!(decoded.address_extension, None);
        assert_eq!(decoded.vgssi, None);
    }

    #[test]
    fn group_identity_uplink_rejects_reserved_giat_on_parse() {
        let mut buf = BitBuffer::new_autoexpand(16);
        buf.write_bits(0, 1);
        buf.write_bits(0, 3);
        buf.write_bits(GIAT_RESERVED as u64, 2);
        buf.seek(0);

        assert_eq!(
            GroupIdentityUplink::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "address_type",
                value: GIAT_RESERVED as u64,
            }
        );
    }

    #[test]
    fn group_identity_uplink_rejects_reserved_giat_shape_on_serialize() {
        let pdu = GroupIdentityUplink {
            vgssi: Some(0x0012_3456),
            ..base_group_identity_uplink()
        };
        let mut buf = BitBuffer::new_autoexpand(64);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::Inconsistency {
                field: "vgssi",
                reason: "uplink GIAT=3 is reserved",
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn group_identity_uplink_rejects_missing_attachment_or_detachment_selector() {
        let pdu = GroupIdentityUplink {
            class_of_usage: None,
            ..base_group_identity_uplink()
        };
        let mut buf = BitBuffer::new_autoexpand(64);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::FieldNotPresent {
                field: Some("class_of_usage_or_group_identity_detachment_uplink"),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn group_identity_uplink_rejects_overwide_class_of_usage() {
        let pdu = GroupIdentityUplink {
            class_of_usage: Some(8),
            ..base_group_identity_uplink()
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
