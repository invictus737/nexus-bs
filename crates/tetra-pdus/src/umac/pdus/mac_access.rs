// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::{BitBuffer, SsiType, TetraAddress, pdu_parse_error::PduParseErr};

use crate::umac::{enums::reservation_requirement::ReservationRequirement, fields::EventLabel};

/// Clause 21.4.2.1 MAC-ACCESS
#[derive(Debug, Clone)]
pub struct MacAccess {
    // 1
    pub fill_bits: bool,
    // 1
    pub encrypted: bool,
    // 2
    // pub addr_type: u8,
    pub addr: Option<TetraAddress>,
    // 24 opt
    // pub ssi: Option<u32>,
    // 10 opt
    pub event_label: Option<EventLabel>,
    // 1
    // pub optional_field_flag: bool,
    // 1 opt
    // pub length_ind_or_cap_req: Option<bool>,

    // 5 opt
    pub length_ind: Option<u8>,
    // 1 opt
    pub frag_flag: Option<bool>,
    // 4 opt
    pub reservation_req: Option<ReservationRequirement>,
}

impl MacAccess {
    const EVENT_LABEL_ALL_ONES: EventLabel = 0x03ff;
    const SSI_MAX: u32 = 0x00ff_ffff;
    const LENGTH_IND_MAX: u8 = 0x1f;

    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        // required constant mac_pdu_type
        let mac_pdu_type = buf.read_field(1, "mac_pdu_type")?;
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
                let address = TetraAddress {
                    ssi_type: SsiType::Issi, // Uplink, always ISSI
                    ssi: buf.read_field(24, "ssi")? as u32,
                };
                (Some(address), None)
            }
            1 => {
                let ev_label = buf.read_field(10, "event_label")? as u16;
                // EN 300 392-2 clause 23.4.1.2.3.2/.3: event label 0 and
                // all-ones are not valid in uplink MAC-ACCESS address type 01.
                if ev_label == 0 || ev_label == Self::EVENT_LABEL_ALL_ONES {
                    return Err(PduParseErr::InvalidValue {
                        field: "event_label",
                        value: ev_label as u64,
                    });
                }
                (None, Some(ev_label))
            }
            2 => {
                let address = TetraAddress {
                    ssi_type: SsiType::Ussi,
                    ssi: buf.read_field(24, "ussi")? as u32,
                };
                (Some(address), None)
            }
            3 => {
                let address = TetraAddress {
                    ssi_type: SsiType::Smi,
                    ssi: buf.read_field(24, "smi")? as u32,
                };
                (Some(address), None)
            }
            _ => {
                return Err(PduParseErr::InvalidValue {
                    field: "addr_type",
                    value: addr_type as u64,
                });
            }
        };

        let optional_field_flag = buf.read_field(1, "optional_field_flag")? != 0;
        let (length_ind, frag_flag, reservation_req) = if optional_field_flag {
            let length_ind_or_cap_req = buf.read_field(1, "length_ind_or_cap_req")?;
            if length_ind_or_cap_req == 0 {
                let len = buf.read_field(5, "length_ind")? as u8;
                (Some(len), None, None)
            } else {
                let frag = buf.read_field(1, "frag_flag")? != 0;
                let val = buf.read_field(4, "reservation_req")?;
                // 4-bit field, ReservationRequirement covers all 16 values. Propagate
                // instead of unwrap to stay panic-free on any future change.
                let res_req = ReservationRequirement::try_from(val).map_err(|_| PduParseErr::InvalidValue {
                    field: "reservation_req",
                    value: val,
                })?;
                (None, Some(frag), Some(res_req))
            }
        } else {
            (None, None, None)
        };

        Ok(MacAccess {
            fill_bits,
            encrypted,
            addr,
            event_label,
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
                    reason: "MAC-ACCESS shall contain one address field selected by address type",
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
                        field: Some("encrypted_mac_access_address"),
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
                    field: Some("encrypted_mac_access_address"),
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
                reason: "MAC-ACCESS may contain length indication or capacity request, not both",
            });
        }

        if has_frag_flag != has_reservation_req {
            return Err(PduParseErr::Inconsistency {
                field: "capacity_request",
                reason: "capacity request requires fragmentation flag and reservation requirement",
            });
        }

        Ok(())
    }

    pub fn try_to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate_for_serialization()?;

        // write required constant mac_pdu_type
        buf.write_bits(0, 1);
        buf.write_bits(self.fill_bits as u8 as u64, 1);
        buf.write_bits(self.encrypted as u8 as u64, 1);

        // Derive addr_type from addr and write type and field
        if let Some(addr) = self.addr {
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
                        field: Some("encrypted_mac_access_address"),
                    });
                }
                SsiType::Unknown | SsiType::EventLabel => {
                    return Err(PduParseErr::InvalidValue {
                        field: "ssi_type",
                        value: addr.ssi_type as u64,
                    });
                }
            }
        } else if let Some(event_label) = self.event_label {
            buf.write_bits(1, 2);
            buf.write_bits(event_label as u64, 10);
        }

        if self.length_ind.is_some() || self.frag_flag.is_some() || self.reservation_req.is_some() {
            buf.write_bits(1, 1); // optional field flag
            if let Some(length_ind) = self.length_ind {
                buf.write_bits(0, 1); // length_ind_or_cap_req
                buf.write_bits(length_ind as u64, 5);
            } else if let (Some(frag_flag), Some(reservation_req)) = (self.frag_flag, self.reservation_req) {
                buf.write_bits(1, 1); // length_ind_or_cap_req
                buf.write_bits(frag_flag as u64, 1);
                buf.write_bits(reservation_req as u64, 4);
            } else {
                return Err(PduParseErr::Inconsistency {
                    field: "capacity_request",
                    reason: "capacity request requires fragmentation flag and reservation requirement",
                });
            }
        } else {
            buf.write_bits(0, 1); // optional field flag
        }

        Ok(())
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        if let Err(err) = self.try_to_bitbuf(buf) {
            tracing::error!("invalid MAC-ACCESS serialization request: {:?}", err);
        }
    }

    pub fn is_null_pdu(&self) -> bool {
        self.length_ind.unwrap_or(1) == 0
    }

    pub fn is_frag_start(&self) -> bool {
        self.frag_flag.unwrap_or(false)
    }
}

impl fmt::Display for MacAccess {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "MacAccess {{ fill_bits: {} encrypted: {}", self.fill_bits, self.encrypted)?;
        if let Some(addr) = self.addr {
            write!(f, " addr: {}", addr)?;
        }
        if let Some(event_label) = self.event_label {
            write!(f, " event_label: {:?}", event_label)?;
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

    fn base_mac_access() -> MacAccess {
        MacAccess {
            fill_bits: false,
            encrypted: false,
            addr: Some(TetraAddress::issi(0x123456)),
            event_label: None,
            length_ind: None,
            frag_flag: None,
            reservation_req: None,
        }
    }

    fn mac_access_event_label_bits(event_label: EventLabel) -> BitBuffer {
        let mut buf = BitBuffer::new_autoexpand(16);
        buf.write_bits(0, 1); // MAC-ACCESS
        buf.write_bits(0, 1); // no fill bits
        buf.write_bits(0, 1); // not encrypted
        buf.write_bits(1, 2); // event label address type
        buf.write_bits(event_label as u64, 10);
        buf.write_bits(0, 1); // no optional field
        buf.seek(0);
        buf
    }

    #[test]
    fn parser_rejects_non_mac_access_type_without_panic() {
        let mut buf = BitBuffer::new_autoexpand(8);
        buf.write_bits(1, 1);
        buf.seek(0);

        assert_eq!(
            MacAccess::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: 1,
            }
        );
    }

    #[test]
    fn parser_rejects_truncated_ssi_address_without_panic() {
        let mut buf = BitBuffer::new_autoexpand(24);
        buf.write_bits(0, 1); // MAC-ACCESS
        buf.write_bits(0, 1); // no fill bits
        buf.write_bits(0, 1); // not encrypted
        buf.write_bits(0, 2); // SSI address type
        buf.write_bits(0, 16); // truncated before the 24-bit SSI field completes
        buf.seek(0);

        assert_eq!(
            MacAccess::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::BufferEnded { field: Some("ssi") }
        );
    }

    #[test]
    fn parser_rejects_reserved_zero_event_label_without_panic() {
        let mut buf = mac_access_event_label_bits(0);

        assert_eq!(
            MacAccess::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "event_label",
                value: 0,
            }
        );
    }

    #[test]
    fn parser_rejects_reserved_all_ones_event_label_without_panic() {
        let mut buf = mac_access_event_label_bits(MacAccess::EVENT_LABEL_ALL_ONES);

        assert_eq!(
            MacAccess::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "event_label",
                value: MacAccess::EVENT_LABEL_ALL_ONES as u64,
            }
        );
    }

    #[test]
    fn try_to_bitbuf_rejects_missing_address_selector_without_panic() {
        let mut pdu = base_mac_access();
        pdu.addr = None;
        let mut buf = BitBuffer::new_autoexpand(32);

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
    fn try_to_bitbuf_rejects_reserved_event_label_without_panic() {
        let mut pdu = base_mac_access();
        pdu.addr = None;
        pdu.event_label = Some(0);
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "event_label",
                value: 0,
            }
        );
    }

    #[test]
    fn try_to_bitbuf_rejects_length_indication_and_capacity_request_mix() {
        let mut pdu = base_mac_access();
        pdu.length_ind = Some(1);
        pdu.frag_flag = Some(false);
        pdu.reservation_req = Some(ReservationRequirement::Req1Slot);
        let mut buf = BitBuffer::new_autoexpand(40);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::Inconsistency {
                field: "length_ind_or_capacity_request",
                reason: "MAC-ACCESS may contain length indication or capacity request, not both",
            }
        );
    }

    #[test]
    fn try_to_bitbuf_rejects_incomplete_capacity_request() {
        let mut pdu = base_mac_access();
        pdu.frag_flag = Some(false);
        let mut buf = BitBuffer::new_autoexpand(40);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::Inconsistency {
                field: "capacity_request",
                reason: "capacity request requires fragmentation flag and reservation requirement",
            }
        );
    }

    #[test]
    fn try_to_bitbuf_rejects_encrypted_address_until_crypto_supported() {
        let mut pdu = base_mac_access();
        pdu.encrypted = true;
        let mut buf = BitBuffer::new_autoexpand(40);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::NotImplemented {
                field: Some("encrypted_mac_access_address"),
            }
        );
    }

    #[test]
    fn try_to_bitbuf_round_trips_capacity_request() {
        let mut pdu = base_mac_access();
        pdu.frag_flag = Some(false);
        pdu.reservation_req = Some(ReservationRequirement::Req1Slot);
        let mut buf = BitBuffer::new_autoexpand(40);

        pdu.try_to_bitbuf(&mut buf).expect("serialize MAC-ACCESS capacity request");
        buf.seek(0);
        let parsed = MacAccess::from_bitbuf(&mut buf).expect("parse MAC-ACCESS capacity request");

        assert_eq!(parsed.addr, pdu.addr);
        assert_eq!(parsed.frag_flag, Some(false));
        assert_eq!(parsed.reservation_req, Some(ReservationRequirement::Req1Slot));
    }
}
