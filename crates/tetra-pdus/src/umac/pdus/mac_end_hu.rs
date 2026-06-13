// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::BitBuffer;
use tetra_core::pdu_parse_error::PduParseErr;

use crate::umac::enums::reservation_requirement::ReservationRequirement;

/// Clause 21.4.2.2 MAC-END-HU
#[derive(Debug, Clone)]
pub struct MacEndHu {
    // 1
    pub fill_bits: bool,
    // 1
    // pub length_ind_or_cap_req: bool,
    // 4 opt
    pub length_ind: Option<u8>,
    // 4 opt
    pub reservation_req: Option<ReservationRequirement>,
}

impl MacEndHu {
    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        // required constant mac_pdu_type
        let mac_pdu_type = buf.read_field(1, "mac_pdu_type")?;
        if mac_pdu_type != 1 {
            return Err(PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: mac_pdu_type,
            });
        }
        let fill_bits = buf.read_field(1, "fill_bits")? != 0;

        let length_ind_or_cap_req = buf.read_field(1, "length_ind_or_cap_req")?;
        let (length_ind, reservation_req) = if length_ind_or_cap_req == 0 {
            let len = buf.read_field(4, "length_ind")? as u8;
            if len == 0 {
                return Err(PduParseErr::InvalidValue {
                    field: "length_ind",
                    value: len as u64,
                });
            }
            (Some(len), None)
        } else {
            let val = buf.read_field(4, "reservation_req")?;
            let res_req = ReservationRequirement::try_from(val).map_err(|_| PduParseErr::InvalidValue {
                field: "reservation_req",
                value: val,
            })?;
            (None, Some(res_req))
        };

        Ok(MacEndHu {
            fill_bits,
            length_ind,
            reservation_req,
        })
    }

    pub fn try_to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        match (self.length_ind, self.reservation_req) {
            (Some(_), Some(_)) => {
                return Err(PduParseErr::Inconsistency {
                    field: "length_ind/reservation_req",
                    reason: "MAC-END-HU shall contain only one of length indication or reservation requirement",
                });
            }
            (None, None) => {
                return Err(PduParseErr::FieldNotPresent {
                    field: Some("length_ind_or_reservation_req"),
                });
            }
            _ => {}
        }
        if let Some(v) = self.length_ind {
            if v == 0 || v > 0x0f {
                return Err(PduParseErr::InvalidValue {
                    field: "length_ind",
                    value: v as u64,
                });
            }
        }

        // write required constant mac_pdu_type
        buf.write_bits(1, 1);
        buf.write_bits(self.fill_bits as u8 as u64, 1);

        if let Some(v) = self.length_ind {
            buf.write_bits(0, 1); // length_ind_or_cap_req
            buf.write_bits(v as u64, 4);
        } else if let Some(reservation_req) = self.reservation_req {
            buf.write_bits(1, 1); // length_ind_or_cap_req
            buf.write_bits(reservation_req as u64, 4);
        }
        Ok(())
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        if let Err(err) = self.try_to_bitbuf(buf) {
            tracing::error!("invalid MAC-END-HU serialization request: {:?}", err);
        }
    }
}

impl fmt::Display for MacEndHu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MacEndHu {{ fill_bits: {}", self.fill_bits)?;
        if let Some(v) = self.length_ind {
            write!(f, "  length_ind: {}", v)?;
        }
        if let Some(v) = self.reservation_req {
            write!(f, "  reservation_req: {}", v)?;
        }
        write!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_rejects_wrong_mac_pdu_type_without_panic() {
        let mut buf = BitBuffer::from_bitstr("0000000");

        assert_eq!(
            MacEndHu::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: 0,
            }
        );
    }

    #[test]
    fn parser_rejects_reserved_zero_length_indication() {
        let mut buf = BitBuffer::from_bitstr("1000000");

        assert_eq!(
            MacEndHu::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "length_ind",
                value: 0,
            }
        );
    }

    #[test]
    fn serializer_rejects_missing_length_or_reservation_without_panic() {
        let pdu = MacEndHu {
            fill_bits: false,
            length_ind: None,
            reservation_req: None,
        };
        let mut buf = BitBuffer::new(7);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::FieldNotPresent {
                field: Some("length_ind_or_reservation_req"),
            }
        );

        pdu.to_bitbuf(&mut buf);
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_rejects_both_length_and_reservation() {
        let pdu = MacEndHu {
            fill_bits: false,
            length_ind: Some(1),
            reservation_req: Some(ReservationRequirement::Req1Slot),
        };
        let mut buf = BitBuffer::new(7);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::Inconsistency {
                field: "length_ind/reservation_req",
                reason: "MAC-END-HU shall contain only one of length indication or reservation requirement",
            }
        );
    }

    #[test]
    fn serializer_rejects_reserved_zero_length_indication() {
        let pdu = MacEndHu {
            fill_bits: false,
            length_ind: Some(0),
            reservation_req: None,
        };
        let mut buf = BitBuffer::new(7);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "length_ind",
                value: 0,
            }
        );
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_rejects_length_indication_above_four_bits() {
        let pdu = MacEndHu {
            fill_bits: false,
            length_ind: Some(16),
            reservation_req: None,
        };
        let mut buf = BitBuffer::new(7);

        assert_eq!(
            pdu.try_to_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "length_ind",
                value: 16,
            }
        );
        assert_eq!(buf.get_len_written(), 0);
    }

    #[test]
    fn serializer_round_trips_reservation_requirement() {
        let pdu = MacEndHu {
            fill_bits: true,
            length_ind: None,
            reservation_req: Some(ReservationRequirement::Req8Slots),
        };
        let mut buf = BitBuffer::new(7);

        pdu.try_to_bitbuf(&mut buf).unwrap();
        buf.seek(0);
        let parsed = MacEndHu::from_bitbuf(&mut buf).unwrap();

        assert!(parsed.fill_bits);
        assert_eq!(parsed.length_ind, None);
        assert_eq!(parsed.reservation_req, Some(ReservationRequirement::Req8Slots));
    }
}
