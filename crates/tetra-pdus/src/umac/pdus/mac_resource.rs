use core::fmt;

use tetra_core::{BitBuffer, SsiType, TetraAddress, pdu_parse_error::PduParseErr};

use crate::umac::{
    enums::mac_resource_addr_type::MacResourceAddrType,
    fields::{EventLabel, basic_slotgrant::BasicSlotgrant, channel_allocation::ChanAllocElement},
};

/// Clause 21.4.3.1 MAC-RESOURCE
#[derive(Debug, Clone, Default)]
pub struct MacResource {
    /// 1 bit, designates if SDU is followed by fill bits to obtain 8-bit alignment.
    /// May be initially set to 0 and updated through MacResource::update_len_and_fill_ind
    /// Carries no meaning if Null PDU
    pub fill_bits: bool,
    /// 1 bit, only relevant if slot granting element present.
    /// 0 -> current chan, 1 -> grant on allocated chan
    /// Carries no meaning if Null PDU
    pub pos_of_grant: u8,
    /// 2 bits. upper bit = encryption enabled, lower bit = cck parity
    /// Carries no meaning if Null PDU
    pub encryption_mode: u8,
    /// 1 bit. If true, random access acknowledged
    /// Carries no meaning if Null PDU
    pub random_access_flag: bool,
    /// 6 bits, 0b111111 = FRAG START, 0b111110 = 2ND SLOT STOLEN
    /// May be left as 0 and updated through MacResource::update_len_and_fill_ind
    pub length_ind: u8,

    /// 3 bits.
    /// If not present, this is a null PDU
    pub addr: Option<TetraAddress>,
    // pub addr_type: MacResourceAddrType,
    // // 24 opt
    // pub ssi: Option<u32>,
    // // 10 opt
    pub event_label: Option<EventLabel>,
    // // 6 opt
    pub usage_marker: Option<u8>,
    // 1
    // pub power_control_flag: bool,
    /// 4 opt
    pub power_control_element: Option<u8>,
    // 1
    // pub slot_granting_flag: bool,
    /// 8 opt
    pub slot_granting_element: Option<BasicSlotgrant>,
    // 1
    // pub chan_alloc_flag: bool,
    pub chan_alloc_element: Option<ChanAllocElement>,
}

impl MacResource {
    pub fn null_pdu() -> Self {
        MacResource {
            fill_bits: false,
            pos_of_grant: 0,
            encryption_mode: 0,
            random_access_flag: false,
            length_ind: 2,
            addr: None,
            event_label: None,
            usage_marker: None,
            power_control_element: None,
            slot_granting_element: None,
            chan_alloc_element: None,
        }
    }

    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let mut s = MacResource {
            fill_bits: false,
            pos_of_grant: 0,
            encryption_mode: 0,
            random_access_flag: false,
            length_ind: 0,
            addr: None,
            event_label: None,
            usage_marker: None,
            power_control_element: None,
            slot_granting_element: None,
            chan_alloc_element: None,
        };

        // EN 300 392-2 clause 21.4.3.1: MAC-RESOURCE has MAC PDU type 00.
        let mac_pdu_type = buf.read_field(2, "mac_pdu_type")?;
        if mac_pdu_type != 0 {
            return Err(PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: mac_pdu_type,
            });
        }
        s.fill_bits = buf.read_field(1, "fill_bits")? != 0;
        s.pos_of_grant = buf.read_field(1, "pos_of_grant")? as u8;
        s.encryption_mode = buf.read_field(2, "encryption_mode")? as u8;
        s.random_access_flag = buf.read_field(1, "random_access_flag")? != 0;
        s.length_ind = buf.read_field(6, "length_ind")? as u8;

        // Parse address type and fields
        let bits = buf.read_field(3, "addr_type")?;
        // MacResourceAddrType covers all 8 values of a 3-bit field, so this conversion
        // cannot fail today — but propagate as InvalidValue rather than .expect() so
        // a future enum change (or a corrupted bit-stream) doesn't crash the worker.
        let addr_type = MacResourceAddrType::try_from(bits).map_err(|_| PduParseErr::InvalidValue {
            field: "addr_type",
            value: bits,
        })?;

        match addr_type {
            MacResourceAddrType::NullPdu => {
                // Other fields don't carry meaning in null PDU, so for clarity we set them to defaults
                // While this deviates from the truly received message, it may prevent a bug or two
                s.fill_bits = false;
                s.pos_of_grant = 0;
                s.encryption_mode = 0;
                s.random_access_flag = false;
            }

            MacResourceAddrType::Ssi => {
                s.addr = Some(TetraAddress {
                    ssi: buf.read_field(24, "ssi")? as u32,
                    // encrypted: s.encryption_mode != 0,
                    ssi_type: SsiType::Ssi,
                });
            }
            MacResourceAddrType::EventLabel => {
                s.event_label = Some(buf.read_field(10, "event_label")? as u16);
            }
            MacResourceAddrType::Ussi => {
                s.addr = Some(TetraAddress {
                    ssi: buf.read_field(24, "ussi")? as u32,
                    // encrypted: s.encryption_mode != 0,
                    ssi_type: SsiType::Ussi,
                });
            }
            MacResourceAddrType::Smi => {
                s.addr = Some(TetraAddress {
                    ssi: buf.read_field(24, "smi")? as u32,
                    // encrypted: s.encryption_mode != 0,
                    ssi_type: SsiType::Smi,
                });
            }
            MacResourceAddrType::SsiAndEventLabel => {
                s.addr = Some(TetraAddress {
                    ssi: buf.read_field(24, "ssi")? as u32,
                    // encrypted: s.encryption_mode != 0,
                    ssi_type: SsiType::Ssi,
                });
                s.event_label = Some(buf.read_field(10, "event_label")? as u16);
            }
            MacResourceAddrType::SsiAndUsageMarker => {
                s.addr = Some(TetraAddress {
                    ssi: buf.read_field(24, "ssi")? as u32,
                    // encrypted: s.encryption_mode != 0,
                    ssi_type: SsiType::Ssi,
                });
                s.usage_marker = Some(buf.read_field(6, "usage_marker")? as u8);
            }
            MacResourceAddrType::SmiAndEventLabel => {
                s.addr = Some(TetraAddress {
                    ssi: buf.read_field(24, "smi")? as u32,
                    // encrypted: s.encryption_mode != 0,
                    ssi_type: SsiType::Smi,
                });
                s.event_label = Some(buf.read_field(10, "event_label")? as u16);
            }
        }

        if addr_type == MacResourceAddrType::NullPdu {
            s.encryption_mode = 0;
            return Ok(s);
        }

        let power_control_flag = buf.read_field(1, "power_control_flag")?;
        if power_control_flag == 1 {
            s.power_control_element = Some(buf.read_field(4, "power_control_element")? as u8);
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

    fn validate_for_serialization(&self) -> Result<MacResourceAddrType, PduParseErr> {
        if self.pos_of_grant > 1 {
            return Err(PduParseErr::InvalidValue {
                field: "pos_of_grant",
                value: self.pos_of_grant as u64,
            });
        }
        if self.encryption_mode > 0x03 {
            return Err(PduParseErr::InvalidValue {
                field: "encryption_mode",
                value: self.encryption_mode as u64,
            });
        }
        if self.length_ind == 0 || self.length_ind > 0x3f {
            return Err(PduParseErr::InvalidValue {
                field: "length_ind",
                value: self.length_ind as u64,
            });
        }
        if let Some(addr) = self.addr {
            if addr.ssi > 0x00ff_ffff {
                return Err(PduParseErr::InvalidValue {
                    field: "addr.ssi",
                    value: addr.ssi as u64,
                });
            }
        }
        if let Some(event_label) = self.event_label {
            if event_label > 0x03ff {
                return Err(PduParseErr::InvalidValue {
                    field: "event_label",
                    value: event_label as u64,
                });
            }
        }
        if let Some(usage_marker) = self.usage_marker {
            if usage_marker > 0x3f {
                return Err(PduParseErr::InvalidValue {
                    field: "usage_marker",
                    value: usage_marker as u64,
                });
            }
        }
        if let Some(power_control) = self.power_control_element {
            if power_control > 0x0f {
                return Err(PduParseErr::InvalidValue {
                    field: "power_control_element",
                    value: power_control as u64,
                });
            }
        }

        if self.is_null_pdu() {
            return Ok(MacResourceAddrType::NullPdu);
        } else if let Some(addr) = self.addr {
            if addr.ssi_type == SsiType::Ssi || addr.ssi_type == SsiType::Gssi || addr.ssi_type == SsiType::Issi {
                if self.event_label.is_none() && self.usage_marker.is_none() {
                    Ok(MacResourceAddrType::Ssi)
                } else if self.event_label.is_some() && self.usage_marker.is_none() {
                    Ok(MacResourceAddrType::SsiAndEventLabel)
                } else if self.usage_marker.is_some() && self.event_label.is_none() {
                    Ok(MacResourceAddrType::SsiAndUsageMarker)
                } else {
                    Err(PduParseErr::Inconsistency {
                        field: "event_label/usage_marker",
                        reason: "MAC-RESOURCE SSI address may carry event label or usage marker, not both",
                    })
                }
            } else if addr.ssi_type == SsiType::Ussi && self.event_label.is_none() && self.usage_marker.is_none() {
                Ok(MacResourceAddrType::Ussi)
            } else if addr.ssi_type == SsiType::Smi {
                if self.usage_marker.is_some() {
                    return Err(PduParseErr::Inconsistency {
                        field: "usage_marker",
                        reason: "MAC-RESOURCE SMI address may not carry a usage marker",
                    });
                }
                if self.event_label.is_some() {
                    Ok(MacResourceAddrType::SmiAndEventLabel)
                } else {
                    Ok(MacResourceAddrType::Smi)
                }
            } else {
                Err(PduParseErr::Inconsistency {
                    field: "addr.ssi_type",
                    reason: "MAC-RESOURCE address type must be SSI/GSSI/ISSI, USSI, or SMI",
                })
            }
        } else {
            if self.usage_marker.is_some() {
                return Err(PduParseErr::Inconsistency {
                    field: "usage_marker",
                    reason: "MAC-RESOURCE usage marker requires an SSI address",
                });
            }
            if self.event_label.is_some() {
                Ok(MacResourceAddrType::EventLabel)
            } else {
                Ok(MacResourceAddrType::NullPdu)
            }
        }
    }

    pub fn try_to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        let addr_type = self.validate_for_serialization()?;
        if addr_type != MacResourceAddrType::NullPdu && self.encryption_mode != 0 {
            return Err(PduParseErr::NotImplemented {
                field: Some("encryption_mode"),
            });
        }

        buf.write_bits(0, 2);
        buf.write_bits(self.fill_bits as u8 as u64, 1);
        buf.write_bits(self.pos_of_grant as u64, 1);
        buf.write_bits(self.encryption_mode as u64, 2);
        buf.write_bits(self.random_access_flag as u8 as u64, 1);
        buf.write_bits(self.length_ind as u64, 6);

        // Write address type and fields
        buf.write_bits(addr_type as u64, 3);
        match addr_type {
            MacResourceAddrType::NullPdu => {}
            MacResourceAddrType::Ssi | MacResourceAddrType::Ussi | MacResourceAddrType::Smi => {
                let Some(addr) = self.addr else {
                    return Err(PduParseErr::FieldNotPresent { field: Some("addr") });
                };
                buf.write_bits(addr.ssi as u64, 24);
            }
            MacResourceAddrType::EventLabel => {
                let Some(event_label) = self.event_label else {
                    return Err(PduParseErr::FieldNotPresent {
                        field: Some("event_label"),
                    });
                };
                buf.write_bits(event_label as u64, 10);
            }
            MacResourceAddrType::SsiAndEventLabel | MacResourceAddrType::SmiAndEventLabel => {
                let Some(addr) = self.addr else {
                    return Err(PduParseErr::FieldNotPresent { field: Some("addr") });
                };
                let Some(event_label) = self.event_label else {
                    return Err(PduParseErr::FieldNotPresent {
                        field: Some("event_label"),
                    });
                };
                buf.write_bits(addr.ssi as u64, 24);
                buf.write_bits(event_label as u64, 10);
            }
            MacResourceAddrType::SsiAndUsageMarker => {
                let Some(addr) = self.addr else {
                    return Err(PduParseErr::FieldNotPresent { field: Some("addr") });
                };
                let Some(usage_marker) = self.usage_marker else {
                    return Err(PduParseErr::FieldNotPresent {
                        field: Some("usage_marker"),
                    });
                };
                buf.write_bits(addr.ssi as u64, 24);
                buf.write_bits(usage_marker as u64, 6);
            }
        }

        if addr_type == MacResourceAddrType::NullPdu {
            // No additional fields
            return Ok(());
        }

        if let Some(v) = self.power_control_element {
            buf.write_bits(1, 1);
            buf.write_bits(v as u64, 4);
        } else {
            buf.write_bits(0, 1);
        }

        if let Some(v) = &self.slot_granting_element {
            buf.write_bits(1, 1);
            v.to_bitbuf(buf); // 8-bit BasicSlotgrant element
        } else {
            buf.write_bits(0, 1);
        }

        if let Some(v) = &self.chan_alloc_element {
            if v.is_supported_by_this_stack() {
                buf.write_bits(1, 1); // Chan alloc flag
                v.to_bitbuf(buf);
            } else {
                tracing::error!("MAC-RESOURCE requested unsupported channel allocation; omitting channel allocation element");
                buf.write_bits(0, 1);
            }
        } else {
            buf.write_bits(0, 1);
        }
        Ok(())
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        if let Err(err) = self.try_to_bitbuf(buf) {
            tracing::error!("invalid MAC-RESOURCE serialization request: {:?}", err);
        }
    }

    pub fn is_null_pdu(&self) -> bool {
        self.addr.is_none() && self.event_label.is_none() && self.usage_marker.is_none()
    }

    pub fn compute_header_len(&self) -> usize {
        let mut ret = 16;
        if self.is_null_pdu() {
            return ret;
        }

        if self.event_label.is_some() {
            ret += 10
        };
        if self.usage_marker.is_some() {
            ret += 6
        };
        if self.addr.is_some() {
            ret += 24
        };

        ret += 1;
        if self.power_control_element.is_some() {
            ret += 4
        };
        ret += 1;
        if self.slot_granting_element.is_some() {
            ret += 8
        };
        ret += 1;
        if let Some(chan_alloc) = self.chan_alloc_element.as_ref() {
            if chan_alloc.is_supported_by_this_stack() {
                ret += chan_alloc.compute_len();
            }
        };

        ret
    }

    /// Updates the length_ind and fill_bits fields based on the computed header lenght and provided SDU length
    /// Returns the number of fill bits that need to be added to the PDU
    pub fn update_len_and_fill_ind(&mut self, sdu_len: usize) -> usize {
        let hdr_len = self.compute_header_len();
        let total_len = hdr_len + sdu_len;
        let total_len_bytes = (total_len + 7) / 8;
        let num_fill_bits = (8 - (total_len % 8)) % 8;

        self.length_ind = total_len_bytes as u8;
        self.fill_bits = num_fill_bits != 0;
        num_fill_bits
    }
}

impl fmt::Display for MacResource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MacResource {{ fill_bits: {}, pos_of_grant: {}, encryption_mode: {}, random_access_flag: {}, length_ind: {}",
            self.fill_bits, self.pos_of_grant, self.encryption_mode, self.random_access_flag, self.length_ind
        )?;

        if let Some(addr) = &self.addr {
            write!(f, "  addr {{ ssi: {} }}", addr.ssi)?;
            if let Some(v) = self.event_label {
                write!(f, "    event_label: {}", v)?;
            }
            if let Some(v) = self.usage_marker {
                write!(f, "    usage_marker: {}", v)?;
            }
            write!(f, "  }}")?;
        } else {
            write!(f, "  addr: Null PDU")?;
        }

        if let Some(v) = self.power_control_element {
            write!(f, "  power_control_element: {}", v)?;
        }
        if let Some(v) = &self.slot_granting_element {
            write!(f, "  slot_granting_element: {}", v)?;
        }
        if let Some(v) = &self.chan_alloc_element {
            write!(f, "  chan_alloc_element: {:?}", v)?;
        }
        write!(f, " }}")
    }
}

#[cfg(test)]
mod tests {

    use tetra_core::debug;

    use super::*;

    #[test]
    fn test_mac_resource_with_chanalloc() {
        debug::setup_logging_verbose();

        let mut buffer = BitBuffer::from_bitstr("00000000100111100000000000000000110011001111100010100101100010111111000011");
        let pdu = MacResource::from_bitbuf(&mut buffer).unwrap();
        println!("Parsed MacResource: {:?}", pdu);

        assert!(buffer.get_len_remaining() == 0);
        assert_eq!(pdu.chan_alloc_element.as_ref().unwrap().carrier_num, 1528);

        let mut new = BitBuffer::new_autoexpand(buffer.get_len());
        pdu.to_bitbuf(&mut new);
        assert_eq!(new.to_bitstr(), buffer.to_bitstr());
    }

    #[test]
    fn parser_rejects_non_mac_resource_type_without_panic() {
        let mut buffer = BitBuffer::from_bitstr("01");

        assert_eq!(
            MacResource::from_bitbuf(&mut buffer).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: 0b01
            }
        );
    }

    #[test]
    fn serializer_rejects_zero_length_indication_without_panic() {
        let pdu = MacResource {
            length_ind: 0,
            ..MacResource::null_pdu()
        };
        let mut buf = BitBuffer::new(16);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "length_ind",
                value: 0,
            }
        );

        pdu.to_bitbuf(&mut buf);
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_rejects_ssi_event_label_and_usage_marker_mix() {
        let pdu = MacResource {
            length_ind: 8,
            addr: Some(TetraAddress::issi(0x123456)),
            event_label: Some(1),
            usage_marker: Some(2),
            ..Default::default()
        };
        let mut buf = BitBuffer::new(64);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::Inconsistency {
                field: "event_label/usage_marker",
                reason: "MAC-RESOURCE SSI address may carry event label or usage marker, not both",
            }
        );
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_rejects_address_above_twenty_four_bits() {
        let pdu = MacResource {
            length_ind: 8,
            addr: Some(TetraAddress::issi(0x0100_0000)),
            ..Default::default()
        };
        let mut buf = BitBuffer::new(64);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "addr.ssi",
                value: 0x0100_0000,
            }
        );
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_returns_not_implemented_for_encrypted_resource() {
        let pdu = MacResource {
            length_ind: 8,
            encryption_mode: 1,
            addr: Some(TetraAddress::issi(0x123456)),
            ..Default::default()
        };
        let mut buf = BitBuffer::new(64);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::NotImplemented {
                field: Some("encryption_mode"),
            }
        );
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_round_trips_ssi_usage_marker() {
        let pdu = MacResource {
            length_ind: 8,
            addr: Some(TetraAddress::issi(0x123456)),
            usage_marker: Some(0x3f),
            ..Default::default()
        };
        let mut buf = BitBuffer::new(49);

        pdu.try_to_bitbuf(&mut buf).unwrap();
        buf.seek(0);
        let parsed = MacResource::from_bitbuf(&mut buf).unwrap();

        assert_eq!(parsed.addr, Some(TetraAddress::new(0x123456, SsiType::Ssi)));
        assert_eq!(parsed.usage_marker, Some(0x3f));
        assert_eq!(parsed.event_label, None);
    }

    #[test]
    fn serializer_allows_null_pdu_ignored_flags_without_panic() {
        let pdu = MacResource {
            fill_bits: true,
            pos_of_grant: 1,
            encryption_mode: 3,
            random_access_flag: true,
            length_ind: 2,
            ..MacResource::null_pdu()
        };
        let mut buf = BitBuffer::new(16);

        pdu.try_to_bitbuf(&mut buf).unwrap();

        assert_eq!(buf.get_len_written(), 16);
    }
}
