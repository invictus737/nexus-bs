use core::fmt;

use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use crate::mle::enums::mle_pdu_type_dl::MlePduTypeDl;

/// Representation of the D-CHANNEL RESPONSE PDU (Clause 18.4.1.4.5a).
/// The message shall be sent by the SwMI in response to an MS request for an assigned channel replacement.
/// Response expected: -
/// Response to: U-CHANNEL REQUEST

// note 1: In the present document, this element shall not be included.
#[derive(Debug)]
pub struct DChannelResponse {
    /// Type1, 1 bits, Channel response type
    pub channel_response_type: bool,
    /// Type1, 3 bits, Reason for the channel request
    pub reason_for_the_channel_request: u8,
    /// Type1, 4 bits, Channel request retry delay
    pub channel_request_retry_delay: u8,
    /// Type2, 8 bits, See note,
    pub reserved1: Option<u64>,
    /// Type2, 8 bits, See note,
    pub reserved2: Option<u64>,
}

impl DChannelResponse {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 clause 18.5.21a/table 18.88 reserves reason values
        // 4..=7. Clause 18.5.6b/table 18.45 reserves retry delay values
        // 13 and 14, while value 15 means retransmission not permitted.
        if self.reason_for_the_channel_request > 3 {
            return Err(PduParseErr::InvalidValue {
                field: "reason_for_the_channel_request",
                value: self.reason_for_the_channel_request as u64,
            });
        }
        if self.channel_request_retry_delay > 15 || (13..=14).contains(&self.channel_request_retry_delay) {
            return Err(PduParseErr::InvalidValue {
                field: "channel_request_retry_delay",
                value: self.channel_request_retry_delay as u64,
            });
        }
        // EN 300 392-2 table 18.10 note: in the present document these
        // reserved Type-2 elements shall not be included.
        if self.reserved1.is_some() {
            return Err(PduParseErr::Inconsistency {
                field: "reserved1",
                reason: "reserved element shall not be included",
            });
        }
        if self.reserved2.is_some() {
            return Err(PduParseErr::Inconsistency {
                field: "reserved2",
                reason: "reserved element shall not be included",
            });
        }
        Ok(())
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeDl::DChannelResponse)?;

        // Type1
        let channel_response_type = buffer.read_field(1, "channel_response_type")? != 0;
        // Type1
        let reason_for_the_channel_request = buffer.read_field(3, "reason_for_the_channel_request")? as u8;
        // Type1
        let channel_request_retry_delay = buffer.read_field(4, "channel_request_retry_delay")? as u8;

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type2
        let reserved1 = typed::parse_type2_generic(obit, buffer, 8, "reserved1")?;
        // Type2
        let reserved2 = typed::parse_type2_generic(obit, buffer, 8, "reserved2")?;

        // Read trailing obit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        let pdu = DChannelResponse {
            channel_response_type,
            reason_for_the_channel_request,
            channel_request_retry_delay,
            reserved1,
            reserved2,
        };
        pdu.validate()?;
        Ok(pdu)
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // PDU Type
        buffer.write_bits(MlePduTypeDl::DChannelResponse.into_raw(), 3);
        // Type1
        buffer.write_bits(self.channel_response_type as u64, 1);
        // Type1
        buffer.write_bits(self.reason_for_the_channel_request as u64, 3);
        // Type1
        buffer.write_bits(self.channel_request_retry_delay as u64, 4);

        // Check if any optional field present and place o-bit
        let obit = self.reserved1.is_some() || self.reserved2.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2
        typed::write_type2_generic(obit, buffer, self.reserved1, 8);

        // Type2
        typed::write_type2_generic(obit, buffer, self.reserved2, 8);

        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for DChannelResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DChannelResponse {{ channel_response_type: {:?} reason_for_the_channel_request: {:?} channel_request_retry_delay: {:?} reserved1: {:?} reserved2: {:?} }}",
            self.channel_response_type,
            self.reason_for_the_channel_request,
            self.channel_request_retry_delay,
            self.reserved1,
            self.reserved2,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_channel_response() -> DChannelResponse {
        DChannelResponse {
            channel_response_type: false,
            reason_for_the_channel_request: 3,
            channel_request_retry_delay: 15,
            reserved1: None,
            reserved2: None,
        }
    }

    #[test]
    fn d_channel_response_roundtrips_without_reserved_elements() {
        let pdu = base_channel_response();
        let mut buf = BitBuffer::new_autoexpand(16);

        pdu.to_bitbuf(&mut buf).expect("serialize D-CHANNEL RESPONSE");
        buf.seek(0);
        let parsed = DChannelResponse::from_bitbuf(&mut buf).expect("parse D-CHANNEL RESPONSE");

        assert!(!parsed.channel_response_type);
        assert_eq!(parsed.reason_for_the_channel_request, 3);
        assert_eq!(parsed.channel_request_retry_delay, 15);
        assert_eq!(parsed.reserved1, None);
        assert_eq!(parsed.reserved2, None);
        assert_eq!(buf.get_len_remaining(), 0);
    }

    #[test]
    fn d_channel_response_rejects_reserved_reason() {
        let pdu = DChannelResponse {
            reason_for_the_channel_request: 4,
            ..base_channel_response()
        };
        let mut buf = BitBuffer::new_autoexpand(16);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "reason_for_the_channel_request",
                value: 4,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_channel_response_rejects_reserved_retry_delay() {
        let pdu = DChannelResponse {
            channel_request_retry_delay: 13,
            ..base_channel_response()
        };
        let mut buf = BitBuffer::new_autoexpand(16);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::InvalidValue {
                field: "channel_request_retry_delay",
                value: 13,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn d_channel_response_rejects_reserved_type2_elements_on_parse() {
        let mut buf = BitBuffer::new_autoexpand(32);
        buf.write_bits(MlePduTypeDl::DChannelResponse.into_raw(), 3);
        buf.write_bits(1, 1);
        buf.write_bits(0, 3);
        buf.write_bits(0, 4);
        delimiters::write_obit(&mut buf, 1);
        typed::write_type2_generic(true, &mut buf, Some(0), 8);
        typed::write_type2_generic(true, &mut buf, None, 8);
        delimiters::write_mbit(&mut buf, 0);
        buf.seek(0);

        assert_eq!(
            DChannelResponse::from_bitbuf(&mut buf).unwrap_err(),
            PduParseErr::Inconsistency {
                field: "reserved1",
                reason: "reserved element shall not be included",
            }
        );
    }

    #[test]
    fn d_channel_response_rejects_reserved_type2_elements_on_serialize() {
        let pdu = DChannelResponse {
            reserved2: Some(0),
            ..base_channel_response()
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::Inconsistency {
                field: "reserved2",
                reason: "reserved element shall not be included",
            })
        );
        assert_eq!(buf.get_len(), 0);
    }
}
