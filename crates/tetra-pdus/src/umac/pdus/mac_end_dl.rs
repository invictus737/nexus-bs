use core::fmt;

use tetra_core::BitBuffer;
use tetra_core::pdu_parse_error::PduParseErr;

use crate::umac::fields::basic_slotgrant::BasicSlotgrant;
use crate::umac::fields::channel_allocation::ChanAllocElement;

/// Clause 21.4.3.3 MAC-END (downlink)
#[derive(Debug, Clone)]
pub struct MacEndDl {
    // 1
    pub fill_bits: bool,
    // 1
    pub pos_of_grant: u8,
    /// 6 bits, depending on modulation some interesting length calculation may need to be applied
    pub length_ind: u8,
    // 1
    // pub slot_granting_flag: bool,
    // 8 opt
    pub slot_granting_element: Option<BasicSlotgrant>,
    // 1
    // pub chan_alloc_flag: bool,
    // 999 opt
    pub chan_alloc_element: Option<ChanAllocElement>,
}

impl MacEndDl {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let mut s = MacEndDl {
            fill_bits: false,
            pos_of_grant: 0,
            length_ind: 0,
            slot_granting_element: None,
            chan_alloc_element: None,
        };

        // EN 300 392-2 clause 21.4.3.3: MAC-END downlink has type 01,
        // subtype 1. Reject corrupt bits instead of panicking the worker.
        let mac_pdu_type = buf.read_field(2, "mac_pdu_type")?;
        if mac_pdu_type != 1 {
            return Err(PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: mac_pdu_type,
            });
        }
        let pdu_subtype = buf.read_field(1, "pdu_subtype")?;
        if pdu_subtype != 1 {
            return Err(PduParseErr::InvalidValue {
                field: "pdu_subtype",
                value: pdu_subtype,
            });
        }
        s.fill_bits = buf.read_field(1, "fill_bits")? != 0;
        s.pos_of_grant = buf.read_field(1, "pos_of_grant")? as u8;
        s.length_ind = buf.read_field(6, "length_ind")? as u8;
        if s.length_ind == 0 {
            return Err(PduParseErr::InvalidValue {
                field: "length_ind",
                value: 0,
            });
        }

        let slot_granting_flag = buf.read_field(1, "slot_granting_flag")?;
        if slot_granting_flag == 1 {
            // Read 8-bit BasicSlotgrant element
            s.slot_granting_element = Some(BasicSlotgrant::from_bitbuf(buf)?);
        }

        let chan_alloc_flag = buf.read_field(1, "chan_alloc_flag")?;
        if chan_alloc_flag == 1 {
            s.chan_alloc_element = Some(ChanAllocElement::from_bitbuf(buf)?);
        }

        Ok(s)
    }

    pub fn compute_hdr_len(has_slotgrant: bool, chan_alloc_element: Option<&ChanAllocElement>) -> usize {
        let mut len = 2 + 1 + 1 + 1 + 6 + 1 + (if has_slotgrant { 8 } else { 0 }) + 1;
        if let Some(chan_alloc) = chan_alloc_element {
            if chan_alloc.is_supported_by_this_stack() {
                len += chan_alloc.compute_len();
            }
        }
        len
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        // write required constant mac_pdu_type and pdu_subtype
        buf.write_bits(1, 2);
        buf.write_bits(1, 1);

        buf.write_bits(self.fill_bits as u8 as u64, 1);
        buf.write_bits(self.pos_of_grant as u64, 1);
        buf.write_bits(self.length_ind as u64, 6);

        if let Some(v) = &self.slot_granting_element {
            buf.write_bits(1, 1);
            // Write 8-bit BasicSlotgrant element
            v.to_bitbuf(buf);
        } else {
            buf.write_bits(0, 1);
        }

        if let Some(v) = &self.chan_alloc_element {
            if v.is_supported_by_this_stack() {
                buf.write_bits(1, 1); // Chan alloc flag
                v.to_bitbuf(buf);
            } else {
                tracing::error!("MAC-END DL requested unsupported channel allocation; omitting channel allocation element");
                buf.write_bits(0, 1);
            }
        } else {
            buf.write_bits(0, 1);
        }
    }
}

impl fmt::Display for MacEndDl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MacEndDl {{ fill_bits: {}", self.fill_bits)?;
        write!(f, "  pos_of_grant: {}", self.pos_of_grant)?;
        write!(f, "  length_ind: {}", self.length_ind)?;
        if let Some(v) = &self.slot_granting_element {
            write!(f, "  slot_granting_element: {}", v)?;
        }
        if let Some(v) = &self.chan_alloc_element {
            write!(f, "  chan_alloc_element: {:?}", v)?;
        }
        write!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_rejects_non_mac_end_type_without_panic() {
        let mut buffer = BitBuffer::from_bitstr("00");

        assert_eq!(
            MacEndDl::from_bitbuf(&mut buffer).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: 0b00
            }
        );
    }

    #[test]
    fn parser_rejects_non_mac_end_subtype_without_panic() {
        let mut buffer = BitBuffer::from_bitstr("010");

        assert_eq!(
            MacEndDl::from_bitbuf(&mut buffer).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "pdu_subtype",
                value: 0b0
            }
        );
    }

    #[test]
    fn parser_rejects_reserved_zero_length_indicator_without_panic() {
        let mut buffer = BitBuffer::from_bitstr("01100000000");

        assert_eq!(
            MacEndDl::from_bitbuf(&mut buffer).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "length_ind",
                value: 0
            }
        );
    }

    #[test]
    fn compute_hdr_len_accounts_for_basic_slotgrant() {
        assert_eq!(MacEndDl::compute_hdr_len(false, None), 13);
        assert_eq!(MacEndDl::compute_hdr_len(true, None), 21);
    }

    #[test]
    fn compute_hdr_len_accounts_for_channel_allocation() {
        let chan_alloc = ChanAllocElement {
            alloc_type: tetra_saps::lcmc::enums::alloc_type::ChanAllocType::Replace,
            ts_assigned: [false, true, false, false],
            ul_dl_assigned: tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment::Both,
            clch_permission: true,
            cell_change_flag: false,
            carrier_num: 1528,
            ext: None,
            mon_pattern: 0,
            frame18_mon_pattern: Some(0),
        };

        assert_eq!(MacEndDl::compute_hdr_len(false, Some(&chan_alloc)), 13 + chan_alloc.compute_len());
    }

    #[test]
    fn mac_end_dl_roundtrips_channel_allocation() {
        let chan_alloc = ChanAllocElement {
            alloc_type: tetra_saps::lcmc::enums::alloc_type::ChanAllocType::Replace,
            ts_assigned: [false, true, false, false],
            ul_dl_assigned: tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment::Both,
            clch_permission: true,
            cell_change_flag: false,
            carrier_num: 1528,
            ext: None,
            mon_pattern: 0,
            frame18_mon_pattern: Some(0),
        };
        let pdu = MacEndDl {
            fill_bits: true,
            pos_of_grant: 0,
            length_ind: ((MacEndDl::compute_hdr_len(false, Some(&chan_alloc)) + 7) / 8) as u8,
            slot_granting_element: None,
            chan_alloc_element: Some(chan_alloc),
        };
        let mut buffer = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut buffer);
        buffer.seek(0);

        let parsed = MacEndDl::from_bitbuf(&mut buffer).expect("parse MAC-END with channel allocation");
        let parsed_chan_alloc = parsed
            .chan_alloc_element
            .as_ref()
            .expect("channel allocation should round-trip in MAC-END");

        assert!(parsed.fill_bits);
        assert_eq!(parsed.pos_of_grant, 0);
        assert_eq!(parsed.length_ind, pdu.length_ind);
        assert_eq!(
            buffer.get_len_written(),
            MacEndDl::compute_hdr_len(false, pdu.chan_alloc_element.as_ref())
        );
        assert_eq!(
            parsed_chan_alloc.alloc_type,
            tetra_saps::lcmc::enums::alloc_type::ChanAllocType::Replace
        );
        assert_eq!(parsed_chan_alloc.ts_assigned, [false, true, false, false]);
        assert_eq!(
            parsed_chan_alloc.ul_dl_assigned,
            tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment::Both
        );
        assert!(parsed_chan_alloc.clch_permission);
        assert!(!parsed_chan_alloc.cell_change_flag);
        assert_eq!(parsed_chan_alloc.carrier_num, 1528);
        assert_eq!(parsed_chan_alloc.mon_pattern, 0);
        assert_eq!(parsed_chan_alloc.frame18_mon_pattern, Some(0));
    }
}
