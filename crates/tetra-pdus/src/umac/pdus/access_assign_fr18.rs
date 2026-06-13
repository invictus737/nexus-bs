// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::umac::{enums::access_assign_ul_usage::AccessAssignUlUsage, pdus::access_assign::AccessField};

/// Clause 21.4.7.2 ACCESS-ASSIGN
/// TODO FIXME technically not part of this SAP, but part of the MAC
#[derive(Debug)]
pub struct AccessAssignFr18 {
    // 2, kept for debugging purposes
    pub _header: u8,
    // 6
    // pub dl_usage: AccessAssignDlUsage,
    pub ul_usage: AccessAssignUlUsage,

    /// Populated when header == 0, 1 or 2
    /// Provides access rights on UL subslot 1
    pub f1_af1: Option<AccessField>,

    /// Populated when header == 3
    pub f1_traf_um: Option<AccessAssignUlUsage>,

    /// Populated when header == 0, 1 or 2
    /// Provides access rights on UL subslot 2
    pub f2_af2: Option<AccessField>,

    /// Populated when header == 3
    /// Provides access rights on both UL subslots
    pub f2_af: Option<AccessField>,
    // pub f2_ul_um: Option<AccessAssignUlUsage>,
}

impl Default for AccessAssignFr18 {
    fn default() -> Self {
        AccessAssignFr18 {
            _header: 0,

            ul_usage: AccessAssignUlUsage::CommonOnly,

            f1_af1: None,
            f1_traf_um: None,
            f2_af2: None,
            f2_af: None,
        }
    }
}

impl AccessAssignFr18 {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let mut s = AccessAssignFr18 {
            _header: buf.read_field(2, "_header")? as u8,
            ..Default::default()
        };

        let field1 = buf.read_field(6, "field1")? as u8;
        let field2 = buf.read_field(6, "field2")? as u8;

        match s._header {
            0 => {
                s.ul_usage = AccessAssignUlUsage::CommonOnly;
                s.f1_af1 = Some(AccessField {
                    access_code: (field1 >> 4) & 0x3,
                    base_frame_len: field1 & 0xF,
                });
                s.f2_af2 = Some(AccessField {
                    access_code: (field2 >> 4) & 0x3,
                    base_frame_len: field2 & 0xF,
                });
            }
            1 => {
                s.ul_usage = AccessAssignUlUsage::CommonAndAssigned;
                s.f1_af1 = Some(AccessField {
                    access_code: (field1 >> 4) & 0x3,
                    base_frame_len: field1 & 0xF,
                });
                s.f2_af2 = Some(AccessField {
                    access_code: (field2 >> 4) & 0x3,
                    base_frame_len: field2 & 0xF,
                });
            }
            2 => {
                s.ul_usage = AccessAssignUlUsage::AssignedOnly;
                s.f1_af1 = Some(AccessField {
                    access_code: (field1 >> 4) & 0x3,
                    base_frame_len: field1 & 0xF,
                });
                s.f2_af2 = Some(AccessField {
                    access_code: (field2 >> 4) & 0x3,
                    base_frame_len: field2 & 0xF,
                });
            }
            3 => {
                // UL usage counts as CommonAndAssigned, but with traffic marker (UMt).
                //
                // Per ETSI EN 300 392-2 Table 21.82, header=11 on frame 18 requires a
                // *traffic* usage marker. Non-traffic values (Unallocated, AssignedOnly,
                // CommonOnly, CommonAndAssigned) are forbidden here. Previously we
                // accepted the parse and then `assert!`-ed is_traffic, which crashed the
                // worker on the first malformed AACH burst we received (could happen on
                // interference, a buggy MS, or hostile traffic). Return InvalidValue
                // instead so the caller can drop the block and keep running. Credit to
                // proxiboi69 in MidnightBlueLabs/tetra-bluestation PR #85 for spotting it.
                let ul_usage = AccessAssignUlUsage::from_usage_marker(field1).ok_or(PduParseErr::InvalidValue {
                    field: "ul_usage",
                    value: field1 as u64,
                })?;
                if !ul_usage.is_traffic() {
                    return Err(PduParseErr::InvalidValue {
                        field: "ul_usage",
                        value: field1 as u64,
                    });
                }
                s.ul_usage = ul_usage;

                s.f2_af = Some(AccessField {
                    access_code: (field2 >> 4) & 0x3,
                    base_frame_len: field2 & 0xF,
                });
            }
            _ => {
                return Err(PduParseErr::InvalidValue {
                    field: "header",
                    value: s._header as u64,
                });
            }
        }

        Ok(s)
    }

    fn validate_access_field(field_name: &'static str, access_field: AccessField) -> Result<(), PduParseErr> {
        if access_field.access_code > 0x03 {
            return Err(PduParseErr::InvalidValue {
                field: field_name,
                value: access_field.access_code as u64,
            });
        }
        if access_field.base_frame_len > 0x0f {
            return Err(PduParseErr::InvalidValue {
                field: field_name,
                value: access_field.base_frame_len as u64,
            });
        }
        Ok(())
    }

    fn validate_access_field_pair(&self) -> Result<(AccessField, AccessField), PduParseErr> {
        let Some(f1_af1) = self.f1_af1 else {
            return Err(PduParseErr::FieldNotPresent { field: Some("f1_af1") });
        };
        let Some(f2_af2) = self.f2_af2 else {
            return Err(PduParseErr::FieldNotPresent { field: Some("f2_af2") });
        };
        if self.f2_af.is_some() {
            return Err(PduParseErr::Inconsistency {
                field: "f2_af",
                reason: "frame-18 headers 00/01/10 use access field 1 and access field 2, not shared access field",
            });
        }
        Self::validate_access_field("f1_af1", f1_af1)?;
        Self::validate_access_field("f2_af2", f2_af2)?;
        Ok((f1_af1, f2_af2))
    }

    fn write_access_field(buf: &mut BitBuffer, access_field: AccessField) {
        buf.write_bits(access_field.access_code as u64, 2);
        buf.write_bits(access_field.base_frame_len as u64, 4);
    }

    pub fn try_to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        if self.ul_usage == AccessAssignUlUsage::CommonOnly {
            let (f1_af1, f2_af2) = self.validate_access_field_pair()?;
            let header = 0;
            buf.write_bits(header as u64, 2);
            Self::write_access_field(buf, f1_af1);
            Self::write_access_field(buf, f2_af2);
        } else if self.ul_usage == AccessAssignUlUsage::CommonAndAssigned {
            let (f1_af1, f2_af2) = self.validate_access_field_pair()?;
            let header = 1;
            buf.write_bits(header as u64, 2);
            Self::write_access_field(buf, f1_af1);
            Self::write_access_field(buf, f2_af2);
        } else if self.ul_usage == AccessAssignUlUsage::AssignedOnly {
            let (f1_af1, f2_af2) = self.validate_access_field_pair()?;
            let header = 2;
            buf.write_bits(header as u64, 2);
            Self::write_access_field(buf, f1_af1);
            Self::write_access_field(buf, f2_af2);
        } else if self.ul_usage.is_traffic() {
            let Some(f2_af) = self.f2_af else {
                return Err(PduParseErr::FieldNotPresent { field: Some("f2_af") });
            };
            if self.f1_af1.is_some() || self.f2_af2.is_some() {
                return Err(PduParseErr::Inconsistency {
                    field: "f1_af1/f2_af2",
                    reason: "frame-18 header 11 uses traffic marker plus shared access field",
                });
            }
            Self::validate_access_field("f2_af", f2_af)?;

            // UL usage counts as common and assigned, but with traffic marker
            let header = 3;
            buf.write_bits(header as u64, 2);
            let Some(ul_usage) = self.ul_usage.to_usage_marker() else {
                return Err(PduParseErr::Inconsistency {
                    field: "ul_usage",
                    reason: "frame-18 header 11 requires a traffic usage marker",
                });
            };
            if ul_usage > 0x3f {
                return Err(PduParseErr::InvalidValue {
                    field: "ul_usage",
                    value: ul_usage as u64,
                });
            }
            buf.write_bits(ul_usage as u64, 6);
            Self::write_access_field(buf, f2_af);
        } else {
            return Err(PduParseErr::Inconsistency {
                field: "ul_usage",
                reason: "frame-18 ACCESS-ASSIGN supports common-only, common-and-assigned, assigned-only, or traffic",
            });
        }

        Ok(())
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        if let Err(err) = self.try_to_bitbuf(buf) {
            tracing::error!("invalid frame-18 ACCESS-ASSIGN serialization request: {:?}", err);
        }
    }
}

impl fmt::Display for AccessAssignFr18 {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "access_assign {{ ul_usage: {}", self.ul_usage)?;
        if let Some(af) = &self.f2_af {
            write!(f, "  AF {}/{}", af.access_code, af.base_frame_len)?;
        };
        if let Some(af) = &self.f1_af1 {
            write!(f, "  AF1 {}/{}", af.access_code, af.base_frame_len)?;
        };
        if let Some(af) = &self.f2_af2 {
            write!(f, "  AF2 {}/{}", af.access_code, af.base_frame_len)?;
        };
        write!(f, " }}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access_field() -> AccessField {
        AccessField {
            access_code: 0,
            base_frame_len: 4,
        }
    }

    #[test]
    fn parser_rejects_header_11_nontraffic_usage_marker_without_panic() {
        let mut buf = BitBuffer::from_bitstr("11000000000100");

        assert_eq!(
            AccessAssignFr18::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "ul_usage",
                value: 0,
            }
        );
    }

    #[test]
    fn serializer_rejects_missing_access_fields_without_panic() {
        let pdu = AccessAssignFr18 {
            ul_usage: AccessAssignUlUsage::CommonOnly,
            ..Default::default()
        };
        let mut buf = BitBuffer::new(14);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::FieldNotPresent { field: Some("f1_af1") }
        );

        pdu.to_bitbuf(&mut buf);
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_rejects_frame18_unallocated_usage_without_panic() {
        let pdu = AccessAssignFr18 {
            ul_usage: AccessAssignUlUsage::Unallocated,
            ..Default::default()
        };
        let mut buf = BitBuffer::new(14);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::Inconsistency {
                field: "ul_usage",
                reason: "frame-18 ACCESS-ASSIGN supports common-only, common-and-assigned, assigned-only, or traffic",
            }
        );
    }

    #[test]
    fn serializer_rejects_traffic_marker_above_six_bits() {
        let pdu = AccessAssignFr18 {
            ul_usage: AccessAssignUlUsage::Traffic(64),
            f2_af: Some(access_field()),
            ..Default::default()
        };
        let mut buf = BitBuffer::new(14);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "ul_usage",
                value: 64,
            }
        );
    }

    #[test]
    fn serializer_round_trips_traffic_marker() {
        let pdu = AccessAssignFr18 {
            ul_usage: AccessAssignUlUsage::Traffic(17),
            f2_af: Some(access_field()),
            ..Default::default()
        };
        let mut buf = BitBuffer::new(14);

        pdu.try_to_bitbuf(&mut buf)
            .expect("serialize frame-18 ACCESS-ASSIGN traffic marker");
        buf.seek(0);
        let parsed = AccessAssignFr18::from_bitbuf(&mut buf).expect("parse frame-18 ACCESS-ASSIGN traffic marker");

        assert_eq!(parsed.ul_usage, AccessAssignUlUsage::Traffic(17));
        assert_eq!(parsed.f2_af.expect("shared access field").base_frame_len, 4);
    }
}
