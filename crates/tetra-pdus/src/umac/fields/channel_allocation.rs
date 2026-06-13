// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

// Clause 21.5.2

use core::fmt;

use tetra_core::{BitBuffer, Todo, pdu_parse_error::PduParseErr};
use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};

#[derive(Debug, Clone)]
pub struct ChanAllocElement {
    // 2
    pub alloc_type: ChanAllocType,
    // 4-bit field, each bit represents a timeslot (TS1 to TS4)
    pub ts_assigned: [bool; 4],
    // 2 bits. 0 = Augmented, 1 = Dl only, 2 = Ul only, 3 = Both
    pub ul_dl_assigned: UlDlAssignment,
    // 1
    pub clch_permission: bool,
    // 1
    pub cell_change_flag: bool,
    // 12
    pub carrier_num: u16,
    // 1
    // pub ext_carrier_num_flag: bool,
    // 4 opt
    // pub ext_freq_band: Option<u8>,
    // 2 opt
    // pub ext_offset: Option<u8>,
    // 3 opt
    // pub ext_duplex_spacing: Option<u8>,
    // 1 opt
    // pub ext_reverse_operation: Option<bool>,
    pub ext: Option<Todo>,
    // 2
    pub mon_pattern: u8,
    // 2 opt
    pub frame18_mon_pattern: Option<u8>,
    // Below is for extended channel allocation which is unsupported for now
    // // 2 opt
    // pub aug_uldl_ass: Option<u8>,
    // // 3 opt
    // pub aug_bandwidth: Option<u8>,
    // // 3 opt
    // pub aug_mod: Option<u8>,
    // // 3 opt
    // pub QAM: Option<u8>,
    // // 3 opt
    // pub RESERVED: Option<u8>,
    // // 3 opt
    // pub aug_conf_chan_stat: Option<u8>,
    // // 4 opt
    // pub bs_link_imbalance: Option<u8>,
    // // 5 opt
    // pub bs_tx_pow_rel: Option<u8>,
    // // 2 opt
    // pub napping_status: Option<u8>,
    // // 2 opt
    // pub napping_info: Option<u8>,
    // // 4 opt
    // pub RESERVED: Option<u8>,
    // // 1
    // pub cond_a_flag: bool,
    // // 16 opt
    // pub cond_a_elem: Option<u16>,
    // // 1
    // pub cond_b_flag: bool,
    // // 16 opt
    // pub cond_b_elem: Option<u16>,
    // // 1
    // pub further_aug_flag: bool,
}

impl ChanAllocElement {
    pub fn is_supported_by_this_stack(&self) -> bool {
        self.ext.is_none() && self.ul_dl_assigned != UlDlAssignment::Augmented
    }

    fn unsupported(field: &'static str) -> PduParseErr {
        PduParseErr::NotImplemented { field: Some(field) }
    }

    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let val = buf.read_field(2, "alloc_type")?;
        // 2-bit field, ChanAllocType covers all 4 values, so this cannot fail today.
        // Propagate as InvalidValue rather than unwrap so a future field-width or enum
        // change can't panic the worker on wire data.
        let alloc_type = ChanAllocType::try_from(val).map_err(|_| PduParseErr::InvalidValue {
            field: "alloc_type",
            value: val,
        })?;

        let bitmap = buf.read_field(4, "ts_assigned")? as u8;
        let ts_assigned = [
            (bitmap & 0b1000) != 0,
            (bitmap & 0b0100) != 0,
            (bitmap & 0b0010) != 0,
            (bitmap & 0b0001) != 0,
        ];

        let val = buf.read_field(2, "ul_dl_assigned")?;
        let ul_dl_assigned = UlDlAssignment::try_from(val).map_err(|_| PduParseErr::InvalidValue {
            field: "ul_dl_assigned",
            value: val,
        })?;
        if ul_dl_assigned == UlDlAssignment::Augmented {
            // EN 300 392-2 clause 21.5.2 notes 1 and 5: an MS that does not
            // support augmented channel allocations shall discard the channel
            // allocation and any TM-SDU carried in the containing
            // MAC-RESOURCE or MAC-END. This stack does not implement the
            // augmented conditional field set yet, so return an explicit parse
            // error instead of panicking or accepting a partial allocation.
            return Err(Self::unsupported("augmented_channel_allocation"));
        }

        let clch_permission = buf.read_field(1, "clch_permission")? != 0;
        let cell_change_flag = buf.read_field(1, "cell_change_flag")? != 0;
        let carrier_num = buf.read_field(12, "carrier_num")? as u16;

        let ext_carrier_num_flag = buf.read_field(1, "ext_carrier_num_flag")? == 1;
        let ext = if ext_carrier_num_flag {
            // EN 300 392-2 clause 21.5.2 note 2 makes the frequency-band,
            // offset, duplex-spacing and reverse-operation fields conditional
            // on extended carrier numbering. The current data model does not
            // store those fields, so fail closed rather than applying the
            // wrong carrier.
            return Err(Self::unsupported("extended_carrier_numbering"));
            // let (ext_freq_band, ext_offset, ext_duplex_spacing, ext_reverse_operation) = match ext_carrier_num_flag {
            //     false => (None, None, None, None),
            //     true => {
            //         let ext_freq_band = buf.read_field(4, "ext_freq_band")? as u8;
            //         let ext_offset = buf.read_field(2, "ext_offset")? as u8;
            //         let ext_duplex_spacing = buf.read_field(3, "ext_duplex_spacing")? as u8;
            //         let ext_reverse_operation = buf.read_field(1, "ext_reverse_operation")? != 0;
            //         (Some(ext_freq_band), Some(ext_offset), Some(ext_duplex_spacing), Some(ext_reverse_operation))
            //     },
            // };
        } else {
            None
        };

        let mon_pattern = buf.read_field(2, "mon_pattern")? as u8;
        let frame18_mon_pattern = match mon_pattern {
            0 => Some(buf.read_field(2, "frame18_mon_pattern")? as u8),
            _ => None,
        };

        Ok(ChanAllocElement {
            alloc_type,
            ts_assigned,
            ul_dl_assigned,
            clch_permission,
            cell_change_flag,
            carrier_num,
            ext,
            // ext_freq_band,
            // ext_offset,
            // ext_duplex_spacing,
            // ext_reverse_operation,
            mon_pattern,
            frame18_mon_pattern,
        })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        if !self.is_supported_by_this_stack() {
            tracing::error!(
                "unsupported channel allocation requested for serialization: ext_present={} ul_dl_assigned={:?}",
                self.ext.is_some(),
                self.ul_dl_assigned
            );
            return;
        }

        buf.write_bits(self.alloc_type as u64, 2);
        for &bit in &self.ts_assigned {
            buf.write_bits(bit as u8 as u64, 1);
        }
        buf.write_bits(self.ul_dl_assigned as u64, 2);
        buf.write_bits(self.clch_permission as u8 as u64, 1);
        buf.write_bits(self.cell_change_flag as u8 as u64, 1);
        buf.write_bits(self.carrier_num as u64, 12);

        // If ext_freq_band supplied, we assume all four fields are there
        if let Some(_ext) = self.ext {
            buf.write_bits(1, 1); // Extended carrier number flag
            tracing::error!("extended carrier numbering serialization is not implemented");
            return;
            // buf.write_bits(ext_freq_band as u64, 4);
            // buf.write_bits(self.ext_offset.unwrap() as u64, 2);
            // buf.write_bits(self.ext_duplex_spacing.unwrap() as u64, 3);
            // buf.write_bits(self.ext_reverse_operation.unwrap() as u8 as u64, 1);
        } else {
            buf.write_bits(0, 1); // Extended carrier number flag
        }

        buf.write_bits(self.mon_pattern as u64, 2);
        if self.mon_pattern == 0 {
            buf.write_bits(self.frame18_mon_pattern.unwrap() as u64, 2);
        }

        if self.ul_dl_assigned == UlDlAssignment::Augmented {
            tracing::error!("augmented channel allocation serialization is not implemented");
            return;
        }

        // TODO REVIEW: conditional write of aug_uldl_ass
        // if let Some(v) = self.aug_uldl_ass { buf.write_bits(v as u64, 2); }
        // // TODO REVIEW: conditional write of aug_bandwidth
        // if let Some(v) = self.aug_bandwidth { buf.write_bits(v as u64, 3); }
        // // TODO REVIEW: conditional write of aug_mod
        // if let Some(v) = self.aug_mod { buf.write_bits(v as u64, 3); }
        // // TODO REVIEW: conditional write of QAM
        // if let Some(v) = self.QAM { buf.write_bits(v as u64, 3); }
        // // TODO REVIEW: conditional write of RESERVED
        // if let Some(v) = self.RESERVED { buf.write_bits(v as u64, 3); }
        // // TODO REVIEW: conditional write of aug_conf_chan_stat
        // if let Some(v) = self.aug_conf_chan_stat { buf.write_bits(v as u64, 3); }
        // // TODO REVIEW: conditional write of bs_link_imbalance
        // if let Some(v) = self.bs_link_imbalance { buf.write_bits(v as u64, 4); }
        // // TODO REVIEW: conditional write of bs_tx_pow_rel
        // if let Some(v) = self.bs_tx_pow_rel { buf.write_bits(v as u64, 5); }
        // // TODO REVIEW: conditional write of napping_status
        // if let Some(v) = self.napping_status { buf.write_bits(v as u64, 2); }
        // // TODO REVIEW: conditional write of napping_info
        // if let Some(v) = self.napping_info { buf.write_bits(v as u64, 2); }
        // // TODO REVIEW: conditional write of RESERVED
        // if let Some(v) = self.RESERVED { buf.write_bits(v as u64, 4); }
        // buf.write_bits(self.cond_a_flag as u8 as u64, 1);
        // // TODO REVIEW: conditional write of cond_a_elem
        // if let Some(v) = self.cond_a_elem { buf.write_bits(v as u64, 16); }
        // buf.write_bits(self.cond_b_flag as u8 as u64, 1);
        // // TODO REVIEW: conditional write of cond_b_elem
        // if let Some(v) = self.cond_b_elem { buf.write_bits(v as u64, 16); }
        // buf.write_bits(self.further_aug_flag as u8 as u64, 1);
    }

    pub fn compute_len(&self) -> usize {
        if !self.is_supported_by_this_stack() {
            tracing::error!(
                "unsupported channel allocation requested for length calculation: ext_present={} ul_dl_assigned={:?}",
                self.ext.is_some(),
                self.ul_dl_assigned
            );
            return 0;
        }

        // Until and including ext carrier numbering flag
        let mut len = 2 + 4 + 2 + 1 + 1 + 12 + 1;

        if let Some(_ext) = &self.ext {
            tracing::error!("extended carrier numbering length calculation is not implemented");
            return 0;
        }

        len += 2;
        if self.mon_pattern == 0 {
            len += 2;
        }

        if self.ul_dl_assigned == UlDlAssignment::Augmented {
            tracing::error!("augmented channel allocation length calculation is not implemented");
            return 0;
        }

        len
    }
}

impl fmt::Display for ChanAllocElement {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "ChanAllocElement {{ alloc_type: {} ts_assigned: {:?} ul_dl_assigned: {} clch_permission: {} cell_change_flag: {} carrier_num: {}",
            self.alloc_type, self.ts_assigned, self.ul_dl_assigned, self.clch_permission, self.cell_change_flag, self.carrier_num,
        )?;

        if let Some(v) = self.ext {
            write!(f, "  ext: {}", v)?;
        }
        write!(f, " mon_pattern: {}", self.mon_pattern)?;
        if let Some(v) = self.frame18_mon_pattern {
            write!(f, "  frame18_mon_pattern: {}", v)?;
        }
        write!(f, " }}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_core::debug;

    #[test]
    fn test_parse_chanalloc_replace_lab() {
        debug::setup_logging_verbose();
        let bitstr = "0001001110001111101001011";
        let mut buffer = BitBuffer::from_bitstr(bitstr);
        let result: ChanAllocElement = ChanAllocElement::from_bitbuf(&mut buffer).unwrap();

        tracing::info!("Parsed ChanAllocElement: {:?}", result);
        tracing::info!("buf:        {}", buffer.dump_bin());
        assert!(buffer.get_len_remaining() == 0);
        assert_eq!(result.carrier_num, 1001);

        let mut buffer_out = BitBuffer::new_autoexpand(30);
        result.to_bitbuf(&mut buffer_out);
        tracing::info!("Serialized: {}", buffer_out.dump_bin());
        assert_eq!(bitstr, buffer_out.to_bitstr());
        assert_eq!(bitstr.len(), result.compute_len());
    }

    #[test]
    fn test_parse_chanalloc_additional() {
        debug::setup_logging_verbose();
        let bitstr = "0100101100010111111000011";
        let mut buffer = BitBuffer::from_bitstr(bitstr);
        let result: ChanAllocElement = ChanAllocElement::from_bitbuf(&mut buffer).unwrap();

        tracing::info!("Parsed ChanAllocElement: {:?}", result);
        tracing::info!("buf:        {}", buffer.dump_bin());
        assert!(buffer.get_len_remaining() == 0);
        assert_eq!(result.carrier_num, 1528);
        // let freq = 400000000 + result.carrier_num as u32 * 25000;
        // tracing::info!("Frequency: {} Hz", freq);
        let mut buffer_out = BitBuffer::new_autoexpand(30);
        result.to_bitbuf(&mut buffer_out);
        tracing::info!("Serialized: {}", buffer_out.dump_bin());
        assert_eq!(bitstr, buffer_out.to_bitstr());
        assert_eq!(bitstr.len(), result.compute_len());
    }

    #[test]
    fn test_parse_chanalloc_replace() {
        debug::setup_logging_verbose();
        let bitstr = "0000101100010111111000011";
        let mut buffer = BitBuffer::from_bitstr(bitstr);
        let result: ChanAllocElement = ChanAllocElement::from_bitbuf(&mut buffer).unwrap();

        tracing::info!("Parsed ChanAllocElement: {:?}", result);
        tracing::info!("buf:        {}", buffer.dump_bin());
        assert!(buffer.get_len_remaining() == 0);
        assert_eq!(result.carrier_num, 1528);
        // let freq = 400000000 + result.carrier_num as u32 * 25000;
        // tracing::info!("Frequency: {} Hz", freq);
        let mut buffer_out = BitBuffer::new_autoexpand(30);
        result.to_bitbuf(&mut buffer_out);
        tracing::info!("Serialized: {}", buffer_out.dump_bin());
        assert_eq!(bitstr, buffer_out.to_bitstr());
        assert_eq!(bitstr.len(), result.compute_len());
    }

    #[test]
    fn test_parse_chanalloc_quitandgo() {
        debug::setup_logging_verbose();
        let bitstr = "1000001100010111111000011";
        let mut buffer = BitBuffer::from_bitstr(bitstr);
        let result: ChanAllocElement = ChanAllocElement::from_bitbuf(&mut buffer).unwrap();

        tracing::info!("Parsed ChanAllocElement: {:?}", result);
        tracing::info!("buf:        {}", buffer.dump_bin());
        assert!(buffer.get_len_remaining() == 0);
        assert_eq!(result.carrier_num, 1528);
        let mut buffer_out = BitBuffer::new_autoexpand(30);
        result.to_bitbuf(&mut buffer_out);
        tracing::info!("Serialized: {}", buffer_out.dump_bin());
        assert_eq!(bitstr, buffer_out.to_bitstr());
        assert_eq!(bitstr.len(), result.compute_len());
    }

    #[test]
    fn parse_augmented_channel_allocation_returns_not_implemented_without_panic() {
        debug::setup_logging_verbose();
        let bitstr = concat!(
            "00",   // Replace allocation type
            "0001", // TS4 assignment bit map
            "00",   // Up/downlink assigned = augmented channel allocation
        );
        let mut buffer = BitBuffer::from_bitstr(bitstr);

        // EN 300 392-2 clause 21.5.2 notes 1 and 5 require an MS without
        // augmented channel-allocation support to discard the allocation and
        // any TM-SDU in the containing MAC-RESOURCE or MAC-END PDU.
        assert_eq!(
            ChanAllocElement::from_bitbuf(&mut buffer).unwrap_err(),
            PduParseErr::NotImplemented {
                field: Some("augmented_channel_allocation")
            }
        );
    }

    #[test]
    fn parse_extended_carrier_numbering_returns_not_implemented_without_panic() {
        debug::setup_logging_verbose();
        let bitstr = concat!(
            "00",           // Replace allocation type
            "0001",         // TS4 assignment bit map
            "11",           // Up/downlink assigned = both
            "0",            // No immediate CLCH permission
            "0",            // No cell change
            "000000000001", // Carrier number
            "1",            // Extended carrier numbering flag
        );
        let mut buffer = BitBuffer::from_bitstr(bitstr);

        // EN 300 392-2 clause 21.5.2 note 2 makes four carrier-extension
        // fields conditional on this flag. The stack does not store them yet,
        // so parsing must fail closed instead of applying the base carrier.
        assert_eq!(
            ChanAllocElement::from_bitbuf(&mut buffer).unwrap_err(),
            PduParseErr::NotImplemented {
                field: Some("extended_carrier_numbering")
            }
        );
    }
}
