// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

/// Clause 21.4.4.3 ACCESS-DEFINE
#[derive(Debug, Clone)]
pub struct AccessDefine {
    // 1
    pub common_or_assigned_control: bool,
    // 2
    pub access_code: u8,
    // 4
    pub imm: u8,
    // 4
    pub wt: u8,
    // 4
    pub nu: u8,
    // 1
    pub frame_len_factor: bool,
    // 4
    pub ts_pointer: u8,
    // 3
    pub min_pdu_prio: u8,
    // 2
    pub opt_field_flag: u8,
    // 16 opt
    pub subscriber_class: Option<u16>,
    // 24 opt
    pub gssi: Option<u32>,
}

impl AccessDefine {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let mut s = AccessDefine {
            common_or_assigned_control: false,
            access_code: 0,
            imm: 0,
            wt: 0,
            nu: 0,
            frame_len_factor: false,
            ts_pointer: 0,
            min_pdu_prio: 0,
            opt_field_flag: 0,
            subscriber_class: None,
            gssi: None,
        };

        // required constant mac_pdu_type
        let mac_pdu_type = buf.read_field(2, "mac_pdu_type")?;
        if mac_pdu_type != 2 {
            return Err(PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: mac_pdu_type,
            });
        }
        // required constant broadcast_type
        let broadcast_type = buf.read_field(2, "broadcast_type")?;
        if broadcast_type != 1 {
            return Err(PduParseErr::InvalidValue {
                field: "broadcast_type",
                value: broadcast_type,
            });
        }
        s.common_or_assigned_control = buf.read_field(1, "common_or_assigned_control")? != 0;
        s.access_code = buf.read_field(2, "access_code")? as u8;
        s.imm = buf.read_field(4, "imm")? as u8;
        s.wt = buf.read_field(4, "wt")? as u8;
        s.nu = buf.read_field(4, "nu")? as u8;
        s.frame_len_factor = buf.read_field(1, "frame_len_factor")? != 0;
        s.ts_pointer = buf.read_field(4, "ts_pointer")? as u8;
        s.min_pdu_prio = buf.read_field(3, "min_pdu_prio")? as u8;
        s.opt_field_flag = buf.read_field(2, "opt_field_flag")? as u8;
        // TODO REVIEW: conditional read of subscriber_class
        if s.opt_field_flag == 1 {
            s.subscriber_class = Some(buf.read_field(16, "subscriber_class")? as u16);
        }
        // TODO REVIEW: conditional read of gssi
        if s.opt_field_flag == 2 {
            s.gssi = Some(buf.read_field(24, "gssi")? as u32);
        }
        // required constant FILLER
        let filler = buf.read_field(3, "filler")?;
        if filler != 4 {
            return Err(PduParseErr::InvalidValue {
                field: "filler",
                value: filler,
            });
        }

        Ok(s)
    }

    pub fn try_to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        if self.access_code > 0x03 {
            return Err(PduParseErr::InvalidValue {
                field: "access_code",
                value: self.access_code as u64,
            });
        }
        if self.imm > 0x0f {
            return Err(PduParseErr::InvalidValue {
                field: "imm",
                value: self.imm as u64,
            });
        }
        if self.wt > 0x0f {
            return Err(PduParseErr::InvalidValue {
                field: "wt",
                value: self.wt as u64,
            });
        }
        if self.nu > 0x0f {
            return Err(PduParseErr::InvalidValue {
                field: "nu",
                value: self.nu as u64,
            });
        }
        if self.ts_pointer > 0x0f {
            return Err(PduParseErr::InvalidValue {
                field: "ts_pointer",
                value: self.ts_pointer as u64,
            });
        }
        if self.min_pdu_prio > 0x07 {
            return Err(PduParseErr::InvalidValue {
                field: "min_pdu_prio",
                value: self.min_pdu_prio as u64,
            });
        }
        if self.opt_field_flag > 0x03 {
            return Err(PduParseErr::InvalidValue {
                field: "opt_field_flag",
                value: self.opt_field_flag as u64,
            });
        }
        if let Some(gssi) = self.gssi {
            if gssi > 0x00ff_ffff {
                return Err(PduParseErr::InvalidValue {
                    field: "gssi",
                    value: gssi as u64,
                });
            }
        }
        match self.opt_field_flag {
            0 | 3 => {
                if self.subscriber_class.is_some() || self.gssi.is_some() {
                    return Err(PduParseErr::Inconsistency {
                        field: "opt_field_flag",
                        reason: "optional field flag 00/11 shall not add subscriber class or GSSI",
                    });
                }
            }
            1 => {
                if self.subscriber_class.is_none() {
                    return Err(PduParseErr::FieldNotPresent {
                        field: Some("subscriber_class"),
                    });
                }
                if self.gssi.is_some() {
                    return Err(PduParseErr::Inconsistency {
                        field: "gssi",
                        reason: "optional field flag 01 selects subscriber class, not GSSI",
                    });
                }
            }
            2 => {
                if self.gssi.is_none() {
                    return Err(PduParseErr::FieldNotPresent { field: Some("gssi") });
                }
                if self.subscriber_class.is_some() {
                    return Err(PduParseErr::Inconsistency {
                        field: "subscriber_class",
                        reason: "optional field flag 10 selects GSSI, not subscriber class",
                    });
                }
            }
            _ => unreachable!("opt_field_flag width already checked"),
        }

        // write required constant mac_pdu_type
        buf.write_bits(2, 2);
        // write required constant broadcast_type
        buf.write_bits(1, 2);
        buf.write_bits(self.common_or_assigned_control as u8 as u64, 1);
        buf.write_bits(self.access_code as u64, 2);
        buf.write_bits(self.imm as u64, 4);
        buf.write_bits(self.wt as u64, 4);
        buf.write_bits(self.nu as u64, 4);
        buf.write_bits(self.frame_len_factor as u8 as u64, 1);
        buf.write_bits(self.ts_pointer as u64, 4);
        buf.write_bits(self.min_pdu_prio as u64, 3);
        buf.write_bits(self.opt_field_flag as u64, 2);
        // TODO REVIEW: conditional write of subscriber_class
        if let Some(v) = self.subscriber_class {
            buf.write_bits(v as u64, 16);
        }
        // TODO REVIEW: conditional write of gssi
        if let Some(v) = self.gssi {
            buf.write_bits(v as u64, 24);
        }
        // write required constant FILLER
        buf.write_bits(4, 3);
        Ok(())
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        if let Err(err) = self.try_to_bitbuf(buf) {
            tracing::error!("invalid ACCESS-DEFINE serialization request: {:?}", err);
        }
    }
}

impl fmt::Display for AccessDefine {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "access_define {{ common_or_assigned_control: {}, access_code: {}, imm: {}, wt: {}, nu: {}, frame_len_factor: {}, ts_pointer: {}, min_pdu_prio: {}, opt_field_flag: {}",
            self.common_or_assigned_control,
            self.access_code,
            self.imm,
            self.wt,
            self.nu,
            self.frame_len_factor,
            self.ts_pointer,
            self.min_pdu_prio,
            self.opt_field_flag
        )?;

        if let Some(v) = self.subscriber_class {
            write!(f, "  subscriber_class: {}", v)?;
        };
        if let Some(v) = self.gssi {
            write!(f, "  gssi: {}", v)?;
        };
        write!(f, " }}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_access_define() -> AccessDefine {
        AccessDefine {
            common_or_assigned_control: false,
            access_code: 0,
            imm: 1,
            wt: 2,
            nu: 3,
            frame_len_factor: false,
            ts_pointer: 4,
            min_pdu_prio: 5,
            opt_field_flag: 0,
            subscriber_class: None,
            gssi: None,
        }
    }

    #[test]
    fn parser_rejects_wrong_mac_pdu_type_without_panic() {
        let mut buf = BitBuffer::from_bitstr("00000000000000000000000000000000");

        assert_eq!(
            AccessDefine::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: 0,
            }
        );
    }

    #[test]
    fn parser_rejects_wrong_broadcast_type_without_panic() {
        let mut buf = BitBuffer::from_bitstr("10000000000000000000000000000000");

        assert_eq!(
            AccessDefine::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "broadcast_type",
                value: 0,
            }
        );
    }

    #[test]
    fn parser_rejects_wrong_filler_without_panic() {
        let mut buf = BitBuffer::from_bitstr("10010000000000000000000000000000");

        assert_eq!(
            AccessDefine::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue { field: "filler", value: 0 }
        );
    }

    #[test]
    fn serializer_rejects_width_overflow_without_writing() {
        let pdu = AccessDefine {
            access_code: 4,
            ..base_access_define()
        };
        let mut buf = BitBuffer::new(32);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "access_code",
                value: 4,
            }
        );
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_rejects_missing_subscriber_class() {
        let pdu = AccessDefine {
            opt_field_flag: 1,
            ..base_access_define()
        };
        let mut buf = BitBuffer::new(48);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::FieldNotPresent {
                field: Some("subscriber_class"),
            }
        );
    }

    #[test]
    fn serializer_rejects_gssi_above_twenty_four_bits_without_writing() {
        let pdu = AccessDefine {
            opt_field_flag: 2,
            gssi: Some(0x0100_0000),
            ..base_access_define()
        };
        let mut buf = BitBuffer::new(56);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "gssi",
                value: 0x0100_0000,
            }
        );
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_round_trips_gssi_optional_field() {
        let pdu = AccessDefine {
            opt_field_flag: 2,
            gssi: Some(0x00ab_cdef),
            ..base_access_define()
        };
        let mut buf = BitBuffer::new(56);

        pdu.try_to_bitbuf(&mut buf).unwrap();
        buf.seek(0);
        let parsed = AccessDefine::from_bitbuf(&mut buf).unwrap();

        assert_eq!(parsed.opt_field_flag, 2);
        assert_eq!(parsed.gssi, Some(0x00ab_cdef));
        assert_eq!(parsed.subscriber_class, None);
    }

    #[test]
    fn serializer_round_trips_no_optional_field_flag_three() {
        let pdu = AccessDefine {
            opt_field_flag: 3,
            ..base_access_define()
        };
        let mut buf = BitBuffer::new(32);

        pdu.try_to_bitbuf(&mut buf).unwrap();
        buf.seek(0);
        let parsed = AccessDefine::from_bitbuf(&mut buf).unwrap();

        assert_eq!(parsed.opt_field_flag, 3);
        assert_eq!(parsed.subscriber_class, None);
        assert_eq!(parsed.gssi, None);
    }
}
