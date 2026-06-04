use core::fmt;

use crate::umac::enums::reservation_requirement::ReservationRequirement;
use tetra_core::BitBuffer;
use tetra_core::pdu_parse_error::PduParseErr;

/// Clause 21.4.2.6 MAC-U-BLCK
#[derive(Debug, Clone)]
pub struct MacUBlck {
    // 1
    pub fill_bits: bool,
    // 1
    pub encrypted: bool,
    // 10
    pub event_label: u16,
    // 4
    pub reservation_req: u8, // WARNING don't use the regular ReservationRequirement enum, as there is a caveat in the highest two values
}

impl MacUBlck {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        // required constant mac_pdu_type
        let mac_pdu_type = buf.read_field(2, "mac_pdu_type")?;
        if mac_pdu_type != 3 {
            return Err(PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: mac_pdu_type,
            });
        }
        // required constant supp_pdu_subtype
        let supp_pdu_subtype = buf.read_field(1, "supp_pdu_subtype")?;
        if supp_pdu_subtype != 0 {
            return Err(PduParseErr::InvalidValue {
                field: "supp_pdu_subtype",
                value: supp_pdu_subtype,
            });
        }
        let fill_bits = buf.read_field(1, "fill_bits")? != 0;
        let encrypted = buf.read_field(1, "encrypted")? != 0;
        let event_label = buf.read_field(10, "event_label")? as u16;
        let reservation_req = buf.read_field(4, "reservation_req")? as u8;

        Ok(MacUBlck {
            fill_bits,
            encrypted,
            event_label,
            reservation_req,
        })
    }

    pub fn try_to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        if self.event_label > 0x03ff {
            return Err(PduParseErr::InvalidValue {
                field: "event_label",
                value: self.event_label as u64,
            });
        }
        if self.reservation_req > 0x0f {
            return Err(PduParseErr::InvalidValue {
                field: "reservation_req",
                value: self.reservation_req as u64,
            });
        }

        // write required constant mac_pdu_type
        buf.write_bits(3, 2);
        // write required constant supp_pdu_subtype
        buf.write_bits(0, 1);
        buf.write_bits(self.fill_bits as u8 as u64, 1);
        buf.write_bits(self.encrypted as u8 as u64, 1);
        buf.write_bits(self.event_label as u64, 10);
        buf.write_bits(self.reservation_req as u64, 4);
        Ok(())
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        if let Err(err) = self.try_to_bitbuf(buf) {
            tracing::error!("invalid MAC-U-BLCK serialization request: {:?}", err);
        }
    }

    pub fn reservation_requirement(&self) -> Option<ReservationRequirement> {
        match self.reservation_req {
            0..=13 => ReservationRequirement::try_from(self.reservation_req as u64).ok(),
            14 => Some(ReservationRequirement::ReqOver68),
            15 => None,
            _ => None,
        }
    }
}

impl fmt::Display for MacUBlck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MacUBlck {{ fill_bits: {}", self.fill_bits)?;
        write!(f, "  encrypted: {}", self.encrypted)?;
        write!(f, "  addr: {}", self.event_label)?;
        write!(f, "  reservation_req: {}", self.reservation_req)?;
        write!(f, " }}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_rejects_wrong_mac_pdu_type_without_panic() {
        let mut buf = BitBuffer::from_bitstr("0100000000000000000");

        assert_eq!(
            MacUBlck::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: 1,
            }
        );
    }

    #[test]
    fn parser_rejects_wrong_supp_pdu_subtype_without_panic() {
        let mut buf = BitBuffer::from_bitstr("1110000000000000000");

        assert_eq!(
            MacUBlck::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "supp_pdu_subtype",
                value: 1,
            }
        );
    }

    #[test]
    fn serializer_rejects_event_label_above_ten_bits_without_writing() {
        let pdu = MacUBlck {
            fill_bits: false,
            encrypted: false,
            event_label: 0x0400,
            reservation_req: 1,
        };
        let mut buf = BitBuffer::new(19);

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
    fn serializer_rejects_reservation_req_above_four_bits_without_writing() {
        let pdu = MacUBlck {
            fill_bits: false,
            encrypted: false,
            event_label: 1,
            reservation_req: 16,
        };
        let mut buf = BitBuffer::new(19);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "reservation_req",
                value: 16,
            }
        );
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_round_trips_boundary_values() {
        let pdu = MacUBlck {
            fill_bits: true,
            encrypted: false,
            event_label: 0x03ff,
            reservation_req: 15,
        };
        let mut buf = BitBuffer::new(19);

        pdu.try_to_bitbuf(&mut buf).unwrap();
        buf.seek(0);
        let parsed = MacUBlck::from_bitbuf(&mut buf).unwrap();

        assert!(parsed.fill_bits);
        assert!(!parsed.encrypted);
        assert_eq!(parsed.event_label, 0x03ff);
        assert_eq!(parsed.reservation_req, 15);
    }

    #[test]
    fn mac_u_blck_reservation_req_fourteen_means_over_68_slots() {
        // EN 300 392-2 table 21.94 is specific to MAC-U-BLCK: raw value
        // 1110 means "68 or more slots required", unlike the general table.
        let pdu = MacUBlck {
            fill_bits: false,
            encrypted: false,
            event_label: 1,
            reservation_req: 14,
        };

        assert_eq!(pdu.reservation_requirement(), Some(ReservationRequirement::ReqOver68));
    }

    #[test]
    fn mac_u_blck_reservation_req_fifteen_means_no_reservation() {
        // EN 300 392-2 table 21.94 reserves raw value 1111 in MAC-U-BLCK for
        // "No reservation requirement"; it must not become ReqOver68.
        let pdu = MacUBlck {
            fill_bits: false,
            encrypted: false,
            event_label: 1,
            reservation_req: 15,
        };

        assert_eq!(pdu.reservation_requirement(), None);
    }
}
