// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::pdu_parse_error::PduParseErr;
use tetra_core::{BitBuffer, SsiType, TetraAddress};

use crate::umac::enums::reservation_requirement::ReservationRequirement;

/// Clause 21.4.2.3 MAC-DATA
#[derive(Debug, Clone)]
pub struct MacData {
    // 1
    pub fill_bits: bool,
    // 1
    pub encrypted: bool,
    // 2
    // pub addr_type: u8,
    // 24 opt, if addr_type in [0,2,3]
    pub addr: Option<TetraAddress>,
    // 10 opt, if addr_type == 1
    pub event_label: Option<u16>,

    /// 6 bit, optional. If not provided, frag_flag and reservation_req must be provided
    pub length_ind: Option<u8>,
    /// 1 bit, optional. If not provided, length_ind must be provided
    pub frag_flag: Option<bool>,
    /// 4 opt, optional. If not provided, length_ind must be provided
    pub reservation_req: Option<ReservationRequirement>,
}

impl MacData {
    const EVENT_LABEL_ALL_ONES: u16 = 0x03ff;
    const SSI_MAX: u32 = 0x00ff_ffff;
    const LENGTH_IND_MAX: u8 = 0x3f;

    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        // required constant mac_pdu_type — MacData is type 0. A corrupted or misrouted
        // burst could carry a different value; return InvalidValue instead of asserting
        // so a single bad PDU can't panic the UMAC worker and take down the cell.
        let mac_pdu_type = buf.read_field(2, "mac_pdu_type")?;
        if mac_pdu_type != 0 {
            return Err(PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: mac_pdu_type,
            });
        }
        let fill_bits = buf.read_field(1, "fill_bits")? != 0;
        let encrypted = buf.read_field(1, "encrypted")? != 0;
        let addr_type = buf.read_field(2, "addr_type")? as u8;
        let (addr, event_label) = match addr_type {
            0 => {
                let ssi = buf.read_field(24, "ssi")? as u32;
                let addr = TetraAddress {
                    ssi,
                    ssi_type: SsiType::Issi, // Uplink, always ISSI
                };
                (Some(addr), None)
            }
            1 => {
                let event_label = buf.read_field(10, "event_label")? as u16;
                // EN 300 392-2 clause 23.4.1.2.3.2/.3: event label 0 and
                // all-ones are not valid in uplink MAC-DATA address type 01.
                if event_label == 0 || event_label == Self::EVENT_LABEL_ALL_ONES {
                    return Err(PduParseErr::InvalidValue {
                        field: "event_label",
                        value: event_label as u64,
                    });
                }
                (None, Some(event_label))
            }
            2 => {
                let ssi = buf.read_field(24, "ssi")? as u32;
                let addr = TetraAddress {
                    ssi,
                    ssi_type: SsiType::Ussi,
                };
                (Some(addr), None)
            }
            3 => {
                let ssi = buf.read_field(24, "ssi")? as u32;
                let addr = TetraAddress {
                    ssi,
                    ssi_type: SsiType::Smi,
                };
                (Some(addr), None)
            }
            _ => {
                return Err(PduParseErr::InvalidValue {
                    field: "addr_type",
                    value: addr_type as u64,
                });
            }
        };

        let length_ind_or_cap_req = buf.read_field(1, "length_ind_or_cap_req")?;
        let (length_ind, frag_flag, reservation_req) = match length_ind_or_cap_req {
            0 => (Some(buf.read_field(6, "length_ind")? as u8), None, None),
            1 => {
                let frag_flag = buf.read_field(1, "frag_flag")? != 0;
                let val = buf.read_field(4, "reservation_requirement")?;
                // 4-bit field fully covered by ReservationRequirement; propagate rather
                // than unwrap to stay panic-free.
                let res_req = ReservationRequirement::try_from(val).map_err(|_| PduParseErr::InvalidValue {
                    field: "reservation_requirement",
                    value: val,
                })?;
                buf.read_bits(1); // Reserved bit
                (None, Some(frag_flag), Some(res_req))
            }
            _ => {
                return Err(PduParseErr::InvalidValue {
                    field: "length_ind_or_cap_req",
                    value: length_ind_or_cap_req,
                });
            }
        };

        Ok(MacData {
            fill_bits,
            encrypted,
            event_label,
            addr,
            length_ind,
            frag_flag,
            reservation_req,
        })
    }

    fn validate_for_serialization(&self) -> Result<(), PduParseErr> {
        match (self.addr, self.event_label) {
            (Some(_), Some(_)) => {
                return Err(PduParseErr::Inconsistency {
                    field: "addr/event_label",
                    reason: "MAC-DATA shall contain one address field selected by address type",
                });
            }
            (None, None) => {
                return Err(PduParseErr::FieldNotPresent {
                    field: Some("addr_or_event_label"),
                });
            }
            _ => {}
        }

        if let Some(addr) = self.addr {
            if addr.ssi > Self::SSI_MAX {
                return Err(PduParseErr::InvalidValue {
                    field: "ssi",
                    value: addr.ssi as u64,
                });
            }
            match addr.ssi_type {
                SsiType::Ssi | SsiType::Issi | SsiType::Gssi | SsiType::Ussi | SsiType::Smi => {}
                SsiType::Esi => {
                    return Err(PduParseErr::NotImplemented {
                        field: Some("encrypted_mac_data_address"),
                    });
                }
                SsiType::Unknown | SsiType::EventLabel => {
                    return Err(PduParseErr::InvalidValue {
                        field: "ssi_type",
                        value: addr.ssi_type as u64,
                    });
                }
            }

            if self.encrypted {
                return Err(PduParseErr::NotImplemented {
                    field: Some("encrypted_mac_data_address"),
                });
            }
        }

        if let Some(event_label) = self.event_label {
            if event_label == 0 || event_label == Self::EVENT_LABEL_ALL_ONES || event_label > Self::EVENT_LABEL_ALL_ONES {
                return Err(PduParseErr::InvalidValue {
                    field: "event_label",
                    value: event_label as u64,
                });
            }
        }

        let has_length_ind = self.length_ind.is_some();
        let has_frag_flag = self.frag_flag.is_some();
        let has_reservation_req = self.reservation_req.is_some();

        if let Some(length_ind) = self.length_ind {
            if length_ind > Self::LENGTH_IND_MAX {
                return Err(PduParseErr::InvalidValue {
                    field: "length_ind",
                    value: length_ind as u64,
                });
            }
        }

        if has_length_ind && (has_frag_flag || has_reservation_req) {
            return Err(PduParseErr::Inconsistency {
                field: "length_ind_or_capacity_request",
                reason: "MAC-DATA may contain length indication or capacity request, not both",
            });
        }

        if !has_length_ind && !(has_frag_flag && has_reservation_req) {
            return Err(PduParseErr::Inconsistency {
                field: "capacity_request",
                reason: "MAC-DATA requires either length indication or a complete capacity request",
            });
        }

        Ok(())
    }

    pub fn try_to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate_for_serialization()?;

        // write required constant mac_pdu_type
        buf.write_bits(0, 2);
        buf.write_bits(self.fill_bits as u8 as u64, 1);
        buf.write_bits(self.encrypted as u8 as u64, 1);

        // If addr is given; we write one of three address types followed by the 24-bit addr
        if let Some(addr) = &self.addr {
            match addr.ssi_type {
                SsiType::Ssi | SsiType::Issi | SsiType::Gssi => {
                    buf.write_bits(0, 2);
                    buf.write_bits(addr.ssi as u64, 24);
                }
                SsiType::Ussi => {
                    buf.write_bits(2, 2);
                    buf.write_bits(addr.ssi as u64, 24);
                }
                SsiType::Smi => {
                    buf.write_bits(3, 2);
                    buf.write_bits(addr.ssi as u64, 24);
                }
                SsiType::Esi => {
                    return Err(PduParseErr::NotImplemented {
                        field: Some("encrypted_mac_data_address"),
                    });
                }
                SsiType::Unknown | SsiType::EventLabel => {
                    return Err(PduParseErr::InvalidValue {
                        field: "ssi_type",
                        value: addr.ssi_type as u64,
                    });
                }
            };
        } else if let Some(event_label) = self.event_label {
            // We must have an event label
            buf.write_bits(1, 2);
            buf.write_bits(event_label as u64, 10);
        }

        // Check if we have a length indication or if we start fragmentation
        if let Some(length_ind) = self.length_ind {
            buf.write_bits(0, 1); // length_ind_or_cap_req
            buf.write_bits(length_ind as u64, 6);
        } else if let (Some(frag_flag), Some(reservation_req)) = (self.frag_flag, self.reservation_req) {
            buf.write_bits(1, 1); // length_ind_or_cap_req
            buf.write_bits(frag_flag as u64, 1);
            buf.write_bits(reservation_req as u64, 4);
            buf.write_bits(0, 1); // Reserved bit
        } else {
            return Err(PduParseErr::Inconsistency {
                field: "capacity_request",
                reason: "MAC-DATA requires either length indication or a complete capacity request",
            });
        }

        Ok(())
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        if let Err(err) = self.try_to_bitbuf(buf) {
            tracing::error!("invalid MAC-DATA serialization request: {:?}", err);
        }
    }
}

impl fmt::Display for MacData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "MacData {{ fill_bits: {} encrypted: {}", self.fill_bits, self.encrypted)?;
        if let Some(v) = &self.addr {
            write!(f, " addr: {}", v)?;
        }
        if let Some(v) = self.event_label {
            write!(f, " event_label: {}", v)?;
        }
        if let Some(v) = self.length_ind {
            write!(f, " length_ind: {}", v)?;
        }
        if let Some(v) = self.frag_flag {
            write!(f, " frag_flag: {}", v)?;
        }
        if let Some(v) = self.reservation_req {
            write!(f, " reservation_req: {}", v)?;
        }
        write!(f, " }}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_mac_data() -> MacData {
        MacData {
            fill_bits: false,
            encrypted: false,
            addr: Some(TetraAddress::issi(0x123456)),
            event_label: None,
            length_ind: Some(2),
            frag_flag: None,
            reservation_req: None,
        }
    }

    fn mac_data_event_label_bits(event_label: u16) -> BitBuffer {
        let mut buf = BitBuffer::new_autoexpand(24);
        buf.write_bits(0, 2); // MAC-DATA
        buf.write_bits(0, 1); // no fill bits
        buf.write_bits(0, 1); // not encrypted
        buf.write_bits(1, 2); // event label address type
        buf.write_bits(event_label as u64, 10);
        buf.write_bits(0, 1); // length indication
        buf.write_bits(2, 6);
        buf.seek(0);
        buf
    }

    #[test]
    fn parser_rejects_non_mac_data_type_without_panic() {
        let mut buf = BitBuffer::from_bitstr("01");

        assert_eq!(
            MacData::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: 0b01,
            }
        );
    }

    #[test]
    fn parser_rejects_reserved_zero_event_label_without_panic() {
        let mut buf = mac_data_event_label_bits(0);

        assert_eq!(
            MacData::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "event_label",
                value: 0,
            }
        );
    }

    #[test]
    fn parser_rejects_reserved_all_ones_event_label_without_panic() {
        let mut buf = mac_data_event_label_bits(MacData::EVENT_LABEL_ALL_ONES);

        assert_eq!(
            MacData::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "event_label",
                value: MacData::EVENT_LABEL_ALL_ONES as u64,
            }
        );
    }

    #[test]
    fn try_to_bitbuf_rejects_missing_address_selector_without_panic() {
        let mut pdu = base_mac_data();
        pdu.addr = None;
        let mut buf = BitBuffer::new_autoexpand(40);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::FieldNotPresent {
                field: Some("addr_or_event_label"),
            }
        );

        pdu.to_bitbuf(&mut buf);
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn try_to_bitbuf_rejects_length_indication_and_capacity_request_mix() {
        let mut pdu = base_mac_data();
        pdu.frag_flag = Some(false);
        pdu.reservation_req = Some(ReservationRequirement::Req1Slot);
        let mut buf = BitBuffer::new_autoexpand(48);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::Inconsistency {
                field: "length_ind_or_capacity_request",
                reason: "MAC-DATA may contain length indication or capacity request, not both",
            }
        );
    }

    #[test]
    fn try_to_bitbuf_rejects_incomplete_capacity_request() {
        let mut pdu = base_mac_data();
        pdu.length_ind = None;
        pdu.frag_flag = Some(false);
        let mut buf = BitBuffer::new_autoexpand(48);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::Inconsistency {
                field: "capacity_request",
                reason: "MAC-DATA requires either length indication or a complete capacity request",
            }
        );
    }

    #[test]
    fn try_to_bitbuf_rejects_encrypted_address_until_crypto_supported() {
        let mut pdu = base_mac_data();
        pdu.encrypted = true;
        let mut buf = BitBuffer::new_autoexpand(48);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::NotImplemented {
                field: Some("encrypted_mac_data_address"),
            }
        );
    }

    #[test]
    fn try_to_bitbuf_round_trips_capacity_request() {
        let mut pdu = base_mac_data();
        pdu.length_ind = None;
        pdu.frag_flag = Some(false);
        pdu.reservation_req = Some(ReservationRequirement::Req1Slot);
        let mut buf = BitBuffer::new_autoexpand(48);

        pdu.try_to_bitbuf(&mut buf).expect("serialize MAC-DATA capacity request");
        buf.seek(0);
        let parsed = MacData::from_bitbuf(&mut buf).expect("parse MAC-DATA capacity request");

        assert_eq!(parsed.addr, pdu.addr);
        assert_eq!(parsed.frag_flag, Some(false));
        assert_eq!(parsed.reservation_req, Some(ReservationRequirement::Req1Slot));
    }
}
