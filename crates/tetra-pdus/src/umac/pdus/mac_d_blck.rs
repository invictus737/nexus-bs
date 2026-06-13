// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::BitBuffer;
use tetra_core::pdu_parse_error::PduParseErr;

use crate::umac::fields::basic_slotgrant::BasicSlotgrant;

/// Clause 21.4.3.4 MAC-D-BLCK
#[derive(Debug, Clone)]
pub struct MacDBlck {
    // 1
    pub fill_bits: bool,
    // 2
    pub encryption_mode: u8,
    // 10
    pub event_label: u16,
    // 1
    pub imm_napping_permission: bool,
    // 1
    // pub slot_granting_flag: bool,
    // 8 opt
    pub slot_granting_element: Option<BasicSlotgrant>,
}

impl MacDBlck {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let mut s = MacDBlck {
            fill_bits: false,
            encryption_mode: 0,
            event_label: 0,
            imm_napping_permission: false,
            // slot_granting_flag: false,
            slot_granting_element: None,
        };

        // required constant mac_pdu_type
        let mac_pdu_type = buf.read_field(2, "mac_pdu_type")?;
        if mac_pdu_type != 3 {
            return Err(PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: mac_pdu_type,
            });
        }
        // required constant pdu_subtype
        let pdu_subtype = buf.read_field(1, "pdu_subtype")?;
        if pdu_subtype != 0 {
            return Err(PduParseErr::InvalidValue {
                field: "pdu_subtype",
                value: pdu_subtype,
            });
        }

        s.fill_bits = buf.read_field(1, "fill_bits")? != 0;
        s.encryption_mode = buf.read_field(2, "encryption_mode")? as u8;
        s.event_label = buf.read_field(10, "event_label")? as u16;
        s.imm_napping_permission = buf.read_field(1, "imm_napping_permission")? != 0;

        let slot_granting_flag = buf.read_field(1, "slot_granting_flag")?;
        if slot_granting_flag == 1 {
            // 8-bit BasicSlotgrant element
            s.slot_granting_element = Some(BasicSlotgrant::from_bitbuf(buf)?);
        }

        Ok(s)
    }

    pub fn try_to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        if self.encryption_mode > 0x03 {
            return Err(PduParseErr::InvalidValue {
                field: "encryption_mode",
                value: self.encryption_mode as u64,
            });
        }
        if self.event_label > 0x03ff {
            return Err(PduParseErr::InvalidValue {
                field: "event_label",
                value: self.event_label as u64,
            });
        }

        // write required constant mac_pdu_type and pdu_subtype
        buf.write_bits(3, 2);
        buf.write_bits(0, 1);

        buf.write_bits(self.fill_bits as u8 as u64, 1);
        buf.write_bits(self.encryption_mode as u64, 2);
        buf.write_bits(self.event_label as u64, 10);
        buf.write_bits(self.imm_napping_permission as u8 as u64, 1);

        if let Some(v) = &self.slot_granting_element {
            buf.write_bits(1, 1);
            v.to_bitbuf(buf); // 8-bit BasicSlotgrant element
        } else {
            buf.write_bits(0, 1);
        }
        Ok(())
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        if let Err(err) = self.try_to_bitbuf(buf) {
            tracing::error!("invalid MAC-D-BLCK serialization request: {:?}", err);
        }
    }
}

impl fmt::Display for MacDBlck {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "MacDBlck {{ fill_bits: {}", self.fill_bits)?;
        write!(f, " encryption_mode: {}", self.encryption_mode)?;
        write!(f, " event_label: {}", self.event_label)?;
        write!(f, " imm_napping_permission: {}", self.imm_napping_permission)?;
        if let Some(v) = &self.slot_granting_element {
            write!(f, "  slot_granting_element: {}", v)?;
        }
        write!(f, " }}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::umac::enums::basic_slotgrant_cap_alloc::BasicSlotgrantCapAlloc;
    use crate::umac::enums::basic_slotgrant_granting_delay::BasicSlotgrantGrantingDelay;

    #[test]
    fn parser_rejects_wrong_mac_pdu_type_without_panic() {
        let mut buf = BitBuffer::from_bitstr("010000000000000000");

        assert_eq!(
            MacDBlck::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: 1,
            }
        );
    }

    #[test]
    fn parser_rejects_wrong_pdu_subtype_without_panic() {
        let mut buf = BitBuffer::from_bitstr("111000000000000000");

        assert_eq!(
            MacDBlck::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "pdu_subtype",
                value: 1,
            }
        );
    }

    #[test]
    fn serializer_rejects_encryption_mode_above_two_bits_without_writing() {
        let pdu = MacDBlck {
            fill_bits: false,
            encryption_mode: 4,
            event_label: 1,
            imm_napping_permission: false,
            slot_granting_element: None,
        };
        let mut buf = BitBuffer::new(18);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "encryption_mode",
                value: 4,
            }
        );
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_rejects_event_label_above_ten_bits_without_writing() {
        let pdu = MacDBlck {
            fill_bits: false,
            encryption_mode: 0,
            event_label: 0x0400,
            imm_napping_permission: false,
            slot_granting_element: None,
        };
        let mut buf = BitBuffer::new(18);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "event_label",
                value: 0x0400,
            }
        );
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_round_trips_with_basic_slotgrant() {
        let pdu = MacDBlck {
            fill_bits: true,
            encryption_mode: 0,
            event_label: 0x03ff,
            imm_napping_permission: true,
            slot_granting_element: Some(BasicSlotgrant {
                capacity_allocation: BasicSlotgrantCapAlloc::Grant1Slot,
                granting_delay: BasicSlotgrantGrantingDelay::DelayNOpportunities(2),
            }),
        };
        let mut buf = BitBuffer::new(26);

        pdu.try_to_bitbuf(&mut buf).unwrap();
        buf.seek(0);
        let parsed = MacDBlck::from_bitbuf(&mut buf).unwrap();

        assert!(parsed.fill_bits);
        assert_eq!(parsed.encryption_mode, 0);
        assert_eq!(parsed.event_label, 0x03ff);
        assert!(parsed.imm_napping_permission);
        assert!(parsed.slot_granting_element.is_some());
    }
}
