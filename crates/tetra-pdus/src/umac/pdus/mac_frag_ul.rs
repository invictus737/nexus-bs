use core::fmt;

use tetra_core::BitBuffer;
use tetra_core::pdu_parse_error::PduParseErr;

/// Clause 21.4.2.4 MAC-FRAG (uplink)
#[derive(Debug, Clone)]
pub struct MacFragUl {
    // 1
    pub fill_bits: bool,
}

impl MacFragUl {
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
        if pdu_subtype != 0 {
            return Err(PduParseErr::InvalidValue {
                field: "pdu_subtype",
                value: pdu_subtype,
            });
        }
        let fill_bits = buf.read_field(1, "fill_bits")? != 0;

        Ok(MacFragUl { fill_bits })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        // write required constant mac_pdu_type
        buf.write_bits(1, 2);
        // write required constant pdu_subtype
        buf.write_bits(0, 1);
        buf.write_bits(self.fill_bits as u8 as u64, 1);
    }
}

impl fmt::Display for MacFragUl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MacFragUl {{ fill_bits: {} }}", self.fill_bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_rejects_wrong_mac_pdu_type_without_panic() {
        let mut buf = BitBuffer::from_bitstr("0000");

        assert_eq!(
            MacFragUl::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "mac_pdu_type",
                value: 0,
            }
        );
    }

    #[test]
    fn parser_rejects_wrong_pdu_subtype_without_panic() {
        let mut buf = BitBuffer::from_bitstr("0110");

        assert_eq!(
            MacFragUl::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::InvalidValue {
                field: "pdu_subtype",
                value: 1,
            }
        );
    }

    #[test]
    fn serializer_round_trips_fill_bits() {
        let pdu = MacFragUl { fill_bits: true };
        let mut buf = BitBuffer::new(4);

        pdu.to_bitbuf(&mut buf);
        buf.seek(0);
        let parsed = MacFragUl::from_bitbuf(&mut buf).unwrap();

        assert!(parsed.fill_bits);
    }
}
