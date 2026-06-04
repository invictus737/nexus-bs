use core::fmt;

use tetra_core::BitBuffer;
use tetra_core::pdu_parse_error::PduParseErr;

use crate::umac::enums::reservation_requirement::ReservationRequirement;

/// Clause 21.4.2.5 MAC-END (uplink)
#[derive(Debug, Clone)]
pub struct MacEndUl {
    // 1
    pub fill_bits: bool,
    // 6
    // If 2-bits length_ind_cap_req < 0b11, field holds 6-bit length indication
    pub length_ind: Option<u8>,
    // If 2-bits length_ind_cap_req == 0b11, then reservation_req field holds 4  data bits
    pub reservation_req: Option<ReservationRequirement>,
}

impl MacEndUl {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        // required constant mac_pdu_type
        let mac_pdu_type = buf.read_field(2, "mac_pdu_type")?;
        if mac_pdu_type != 1 {
            return Err(PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: mac_pdu_type,
            });
        }
        // required constant pdu_subtype
        let pdu_subtype = buf.read_field(1, "pdu_subtype")?;
        if pdu_subtype != 1 {
            return Err(PduParseErr::InvalidValue {
                field: "pdu_subtype",
                value: pdu_subtype,
            });
        }
        let fill_bits = buf.read_field(1, "fill_bits")? != 0;
        let length_ind_cap_req = buf.read_field(6, "length_ind_cap_req")?;
        let (length_ind, reservation_req) = if length_ind_cap_req == 0 {
            // Reserved value
            return Err(PduParseErr::InvalidValue {
                field: "length_ind_cap_req",
                value: length_ind_cap_req,
            });
        } else if length_ind_cap_req <= 0b101110 {
            // Length indication
            (Some(length_ind_cap_req as u8), None)
        } else if length_ind_cap_req < 0b110000 {
            // reserved value, return error
            return Err(PduParseErr::InvalidValue {
                field: "length_ind_cap_req",
                value: length_ind_cap_req,
            });
        } else {
            // 0x110000 or higher, cap req
            let val = length_ind_cap_req & 0b001111;
            let res_req = ReservationRequirement::try_from(val).map_err(|_| PduParseErr::InvalidValue {
                field: "reservation_req",
                value: val,
            })?;
            (None, Some(res_req))
        };

        Ok(MacEndUl {
            fill_bits,
            length_ind,
            reservation_req,
        })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        match (self.length_ind, self.reservation_req) {
            (Some(_), Some(_)) => {
                return Err(PduParseErr::Inconsistency {
                    field: "length_ind/reservation_req",
                    reason: "MAC-END uplink shall contain one length indication or reservation requirement",
                });
            }
            (None, None) => {
                return Err(PduParseErr::FieldNotPresent {
                    field: Some("length_ind_or_reservation_req"),
                });
            }
            _ => {}
        }
        if let Some(length_ind) = self.length_ind {
            if length_ind == 0 || length_ind > 0b101110 {
                return Err(PduParseErr::InvalidValue {
                    field: "length_ind",
                    value: length_ind as u64,
                });
            }
        }

        // write required constant mac_pdu_type
        buf.write_bits(1, 2);
        // write required constant pdu_subtype
        buf.write_bits(1, 1);
        buf.write_bits(self.fill_bits as u8 as u64, 1);
        if let Some(length_ind) = self.length_ind {
            buf.write_bits(length_ind as u64, 6);
        } else if let Some(reservation_req) = self.reservation_req {
            buf.write_bits(0b11, 2);
            buf.write_bits(reservation_req as u64, 4);
        }
        Ok(())
    }
}

impl fmt::Display for MacEndUl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MacEndUl {{ fill_bits: {}", self.fill_bits)?;
        if let Some(length_ind) = self.length_ind {
            write!(f, "  length_ind: {}", length_ind)?;
        }
        if let Some(reservation_req) = self.reservation_req {
            write!(f, "  reservation_req: {}", reservation_req)?;
        }
        write!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_rejects_wrong_mac_pdu_type_without_panic() {
        let mut buf = BitBuffer::from_bitstr("0010000001");

        assert_eq!(
            MacEndUl::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: 0,
            }
        );
    }

    #[test]
    fn parser_rejects_wrong_pdu_subtype_without_panic() {
        let mut buf = BitBuffer::from_bitstr("0100000001");

        assert_eq!(
            MacEndUl::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "pdu_subtype",
                value: 0,
            }
        );
    }

    #[test]
    fn parser_rejects_reserved_zero_length_indication() {
        let mut buf = BitBuffer::from_bitstr("0110000000");

        assert_eq!(
            MacEndUl::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "length_ind_cap_req",
                value: 0,
            }
        );
    }

    #[test]
    fn parser_rejects_reserved_101111_value() {
        let mut buf = BitBuffer::from_bitstr("0110101111");

        assert_eq!(
            MacEndUl::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "length_ind_cap_req",
                value: 0b101111,
            }
        );
    }

    #[test]
    fn parser_accepts_11xxxx_reservation_requirement() {
        let mut buf = BitBuffer::from_bitstr("0111110111");
        let pdu = MacEndUl::from_bitbuf(&mut buf).unwrap();

        assert!(pdu.fill_bits);
        assert_eq!(pdu.length_ind, None);
        assert_eq!(pdu.reservation_req, Some(ReservationRequirement::Req8Slots));
    }

    #[test]
    fn serializer_rejects_missing_length_or_reservation_without_writing() {
        let pdu = MacEndUl {
            fill_bits: false,
            length_ind: None,
            reservation_req: None,
        };
        let mut buf = BitBuffer::new(10);

        assert_eq!(
            pdu.to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::FieldNotPresent {
                field: Some("length_ind_or_reservation_req"),
            }
        );
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_rejects_both_length_and_reservation() {
        let pdu = MacEndUl {
            fill_bits: false,
            length_ind: Some(1),
            reservation_req: Some(ReservationRequirement::Req1Slot),
        };
        let mut buf = BitBuffer::new(10);

        assert_eq!(
            pdu.to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::Inconsistency {
                field: "length_ind/reservation_req",
                reason: "MAC-END uplink shall contain one length indication or reservation requirement",
            }
        );
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_rejects_reserved_zero_length_indication_without_writing() {
        let pdu = MacEndUl {
            fill_bits: false,
            length_ind: Some(0),
            reservation_req: None,
        };
        let mut buf = BitBuffer::new(10);

        assert_eq!(
            pdu.to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "length_ind",
                value: 0,
            }
        );
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_allows_highest_length_indication() {
        let pdu = MacEndUl {
            fill_bits: false,
            length_ind: Some(0b101110),
            reservation_req: None,
        };
        let mut buf = BitBuffer::new(10);

        pdu.to_bitbuf(&mut buf).unwrap();
        buf.seek(0);
        let parsed = MacEndUl::from_bitbuf(&mut buf).unwrap();

        assert_eq!(parsed.length_ind, Some(0b101110));
        assert_eq!(parsed.reservation_req, None);
    }
}
