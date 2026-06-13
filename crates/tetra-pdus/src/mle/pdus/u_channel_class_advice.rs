// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

use crate::mle::enums::mle_pdu_type_ul::MlePduTypeUl;
use crate::mle::pdus::raw_sdu;

/// Representation of the U-CHANNEL CLASS ADVICE PDU (Clause 18.4.1.4.8).
/// The message advises the SwMI of usable channel classes and the data priority of SN PDUs awaiting access to a packet data channel.
/// Response expected: -
/// Response to: -

// note 1: Shall indicate the number of “channel class identifier” information elements: 002 means one (4 bits); 012 means two (8 bits); 102 means three (12 bits); 112 means four (16 bits).
// note 2: Shall be present as many times as indicated by the “number of channel class identifiers” element; no P-bit preceding each element.
// note 3: There shall be no P-bit in the PDU coding preceding the “SDU” information element.
// note 4: If value is 0, the SwMI shall decode the SDU using the SNDCP protocol; if 1, using the protocol indicated by “protocol discriminator.”
// note 5: This instance of “protocol discriminator” shall be present only if “discriminator for SDU protocol present” is set to 1.
// note 6: If present, this instance of “protocol discriminator” indicates the SDU protocol.
#[derive(Debug)]
pub struct UChannelClassAdvice {
    /// Type1, 2 bits, See note 1,
    pub number_of_channel_class_identifiers: u8,
    /// Conditional 4 bits, Repeatable; see note 2,
    pub channel_class_identifier: Option<u64>,
    /// Type1, 1 bits, See note 4,
    pub discriminator_for_sdu_protocol_present: bool,
    /// Conditional 3 bits, See notes 5 and 6,
    pub protocol_discriminator: Option<u64>,
    /// Type2, 3 bits, Data priority
    pub data_priority: Option<u64>,
    /// Conditional See note 3,
    pub sdu: Option<u64>,
}

impl UChannelClassAdvice {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(3, "pdu_type")?;
        expect_pdu_type!(pdu_type, MlePduTypeUl::UChannelClassAdvice)?;

        // Type1
        let number_of_channel_class_identifiers = buffer.read_field(2, "number_of_channel_class_identifiers")? as u8;
        let channel_class_identifier_len = (usize::from(number_of_channel_class_identifiers) + 1) * 4;
        let channel_class_identifier = Some(buffer.read_field(channel_class_identifier_len, "channel_class_identifier")?);
        // Type1
        let discriminator_for_sdu_protocol_present = buffer.read_field(1, "discriminator_for_sdu_protocol_present")? != 0;
        let protocol_discriminator = if discriminator_for_sdu_protocol_present {
            Some(buffer.read_field(3, "protocol_discriminator")?)
        } else {
            None
        };

        // obit designates presence of any further type2, type3 or type4 fields
        let obit = delimiters::read_obit(buffer)?;

        // Type2
        let data_priority = typed::parse_type2_generic(obit, buffer, 3, "data_priority")?;
        // EN 300 392-2 table 18.14: the SN-PDU SDU has no preceding P-bit and
        // occupies the remaining payload, if present.
        let sdu = raw_sdu::read_remaining_u64(buffer, "u_channel_class_advice_sdu")?;

        Ok(UChannelClassAdvice {
            number_of_channel_class_identifiers,
            channel_class_identifier,
            discriminator_for_sdu_protocol_present,
            protocol_discriminator,
            data_priority,
            sdu,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        raw_sdu::reject_write_if_present(self.sdu, "u_channel_class_advice_sdu")?;
        if self.number_of_channel_class_identifiers > 3 {
            return Err(PduParseErr::InvalidValue {
                field: "number_of_channel_class_identifiers",
                value: self.number_of_channel_class_identifiers as u64,
            });
        }
        let channel_class_identifier_len = (usize::from(self.number_of_channel_class_identifiers) + 1) * 4;
        let channel_class_identifier = self.channel_class_identifier.ok_or(PduParseErr::FieldNotPresent {
            field: Some("channel_class_identifier"),
        })?;
        if channel_class_identifier_len < 64 && channel_class_identifier >= (1u64 << channel_class_identifier_len) {
            return Err(PduParseErr::InvalidValue {
                field: "channel_class_identifier",
                value: channel_class_identifier,
            });
        }
        if self.discriminator_for_sdu_protocol_present {
            let protocol_discriminator = self.protocol_discriminator.ok_or(PduParseErr::FieldNotPresent {
                field: Some("protocol_discriminator"),
            })?;
            if protocol_discriminator > 7 {
                return Err(PduParseErr::InvalidValue {
                    field: "protocol_discriminator",
                    value: protocol_discriminator,
                });
            }
        } else if self.protocol_discriminator.is_some() {
            return Err(PduParseErr::InvalidValue {
                field: "protocol_discriminator",
                value: self.protocol_discriminator.unwrap_or_default(),
            });
        }
        if let Some(data_priority) = self.data_priority
            && data_priority > 7
        {
            return Err(PduParseErr::InvalidValue {
                field: "data_priority",
                value: data_priority,
            });
        }

        // PDU Type
        buffer.write_bits(MlePduTypeUl::UChannelClassAdvice.into_raw(), 3);
        // Type1
        buffer.write_bits(self.number_of_channel_class_identifiers as u64, 2);
        // Conditional
        buffer.write_bits(channel_class_identifier, channel_class_identifier_len);
        // Type1
        buffer.write_bits(self.discriminator_for_sdu_protocol_present as u64, 1);
        // Conditional
        if let Some(ref value) = self.protocol_discriminator {
            buffer.write_bits(*value, 3);
        }

        // Check if any optional field present and place o-bit
        let obit = self.data_priority.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2
        typed::write_type2_generic(obit, buffer, self.data_priority, 3);

        Ok(())
    }
}

impl fmt::Display for UChannelClassAdvice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "UChannelClassAdvice {{ number_of_channel_class_identifiers: {:?} channel_class_identifier: {:?} discriminator_for_sdu_protocol_present: {:?} protocol_discriminator: {:?} data_priority: {:?} sdu: {:?} }}",
            self.number_of_channel_class_identifiers,
            self.channel_class_identifier,
            self.discriminator_for_sdu_protocol_present,
            self.protocol_discriminator,
            self.data_priority,
            self.sdu,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u_channel_class_advice_round_trips_identifiers_and_priority_without_sdu() {
        let pdu = UChannelClassAdvice {
            number_of_channel_class_identifiers: 1,
            channel_class_identifier: Some(0xab),
            discriminator_for_sdu_protocol_present: true,
            protocol_discriminator: Some(5),
            data_priority: Some(6),
            sdu: None,
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        pdu.to_bitbuf(&mut buf).expect("serialize U-CHANNEL CLASS ADVICE");
        buf.seek(0);
        let parsed = UChannelClassAdvice::from_bitbuf(&mut buf).expect("parse U-CHANNEL CLASS ADVICE");

        assert_eq!(parsed.number_of_channel_class_identifiers, 1);
        assert_eq!(parsed.channel_class_identifier, Some(0xab));
        assert_eq!(parsed.discriminator_for_sdu_protocol_present, true);
        assert_eq!(parsed.protocol_discriminator, Some(5));
        assert_eq!(parsed.data_priority, Some(6));
        assert_eq!(parsed.sdu, None);
        assert_eq!(buf.get_len_remaining(), 0);
    }

    #[test]
    fn u_channel_class_advice_parses_no_pbit_sdu_after_optional_priority() {
        let mut buf = BitBuffer::new_autoexpand(32);
        buf.write_bits(MlePduTypeUl::UChannelClassAdvice.into_raw(), 3);
        buf.write_bits(0, 2);
        buf.write_bits(0x5, 4);
        buf.write_bits(0, 1);
        buf.write_bits(1, 1);
        buf.write_bits(1, 1);
        buf.write_bits(4, 3);
        buf.write_bits(0b101011, 6);
        buf.seek(0);

        let parsed = UChannelClassAdvice::from_bitbuf(&mut buf).expect("parse U-CHANNEL CLASS ADVICE with SDU");

        assert_eq!(parsed.number_of_channel_class_identifiers, 0);
        assert_eq!(parsed.channel_class_identifier, Some(0x5));
        assert_eq!(parsed.discriminator_for_sdu_protocol_present, false);
        assert_eq!(parsed.protocol_discriminator, None);
        assert_eq!(parsed.data_priority, Some(4));
        assert_eq!(parsed.sdu, Some(0b101011));
        assert_eq!(buf.get_len_remaining(), 0);
    }

    #[test]
    fn u_channel_class_advice_rejects_serializing_raw_sdu_until_length_is_modelled() {
        let pdu = UChannelClassAdvice {
            number_of_channel_class_identifiers: 0,
            channel_class_identifier: Some(0x5),
            discriminator_for_sdu_protocol_present: false,
            protocol_discriminator: None,
            data_priority: None,
            sdu: Some(0b101011),
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::NotImplemented {
                field: Some("u_channel_class_advice_sdu"),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }
}
