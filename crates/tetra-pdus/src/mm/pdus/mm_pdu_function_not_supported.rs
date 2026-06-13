// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::expect_pdu_type;
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mm::enums::mm_pdu_type_dl::MmPduTypeDl;

/// Representation of the MM PDU/FUNCTION NOT SUPPORTED PDU (Clause 16.9.4.1).
/// This PDU may be sent by the MS/LS or SwMI to indicate that the received MM PDU or the function indicated in the PDU is not supported.
/// Response expected: -
/// Response to: Any individually addressed MM PDU

// note 1: This information element shall identify the received PDU which contains the function which cannot be supported.
// note 2: In case the receiving party recognizes the PDU and the PDU contains a sub-PDU field (like in U/M-MM STATUS PDU, U/D-OTAR, U/D-ENABLE, etc.) this element contains the element indicating which sub-PDU this is.
// note 3: The length of this element is indicated by the Length of the copied PDU element. This element is not present if the Length of the copied PDU element is not present.
// note 4: This element contains the received PDU beginning from and excluding the PDU type element.
#[derive(Debug)]
pub struct MmPduFunctionNotSupported {
    /// Type1, 4 bits, See note 1,
    pub not_supported_pdu_type: u8,
    /// Type2, See note 2. Holds (len_bits, value)
    pub not_supported_sub_pdu_type: Option<(usize, u64)>,
    // //// Type2, 8 bits, Length of the copied PDU
    // pub length_of_the_copied_pdu: Option<u64>,
    // /// Conditional See notes 3 and 4,
    // pub received_pdu_contents: Option<u64>,
}

impl MmPduFunctionNotSupported {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(4, "pdu_type")?;
        expect_pdu_type!(pdu_type, MmPduTypeDl::MmPduFunctionNotSupported)?;

        // Type1
        let not_supported_pdu_type = buffer.read_field(4, "not_supported_pdu_type")? as u8;

        // obit designates presence of any further type2, type3 or type4 fields
        let obit = delimiters::read_obit(buffer)?;

        // Type2
        if !obit {
            return Ok(MmPduFunctionNotSupported {
                not_supported_pdu_type,
                not_supported_sub_pdu_type: None,
            });
        }

        let not_supported_sub_pdu_type = match delimiters::read_pbit(buffer)? {
            true => {
                let len = buffer.get_len_remaining().checked_sub(1).ok_or(PduParseErr::BufferEnded {
                    field: Some("not_supported_sub_pdu_type"),
                })?;
                if len == 0 {
                    return Err(PduParseErr::InvalidValue {
                        field: "not_supported_sub_pdu_type_len",
                        value: 0,
                    });
                }
                let value = buffer.read_field(len, "not_supported_sub_pdu_type")?;
                Some((len, value))
            }
            false => None,
        };

        if delimiters::read_mbit(buffer)? {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }
        if buffer.get_len_remaining() != 0 {
            return Err(PduParseErr::NotImplemented {
                field: Some("received_pdu_contents"),
            });
        }

        Ok(MmPduFunctionNotSupported {
            not_supported_pdu_type,
            not_supported_sub_pdu_type,
        })
        // // Type2
        // let length_of_the_copied_pdu = typed::parse_type2_generic(obit, buffer, 8, "length_of_the_copied_pdu")?;
        // // Conditional
        // unimplemented!(); let received_pdu_contents = if obit { Some(0) } else { None };

        // // Read trailing obit (if not previously encountered)
        // obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        // if obit {
        //     return Err(PduParseErr::InvalidTrailingMbitValue);
        // }

        // Ok(MmPduFunctionNotSupported {
        //     not_supported_pdu_type,
        //     not_supported_sub_pdu_type,
        //     length_of_the_copied_pdu,
        //     received_pdu_contents
        // })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        // PDU Type
        buffer.write_bits(MmPduTypeDl::MmPduFunctionNotSupported.into_raw(), 4);
        // Type1
        buffer.write_bits(self.not_supported_pdu_type as u64, 4);

        // Check if any optional field present and place o-bit
        let obit = self.not_supported_sub_pdu_type.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type2. EN 300 392-2 table 16.27 makes the sub-PDU selector length
        // depend on the received PDU; callers carry that bit length explicitly.
        let Some((len, val)) = self.not_supported_sub_pdu_type else {
            return Err(PduParseErr::Inconsistency {
                field: "not_supported_sub_pdu_type",
                reason: "missing while optional-bit is set",
            });
        };
        if len > 64 {
            return Err(PduParseErr::InvalidValue {
                field: "not_supported_sub_pdu_type_len",
                value: len as u64,
            });
        }
        if len < 64 && val >> len != 0 {
            return Err(PduParseErr::InvalidValue {
                field: "not_supported_sub_pdu_type",
                value: val,
            });
        }
        typed::write_type2_generic(obit, buffer, Some(val), len);

        // let obit = self.not_supported_sub_pdu_type.is_some() || self.length_of_the_copied_pdu.is_some() ;
        // delimiters::write_obit(buffer, obit as u8);
        // if !obit { return Ok(()); }

        // // Type2
        // unimplemented!();
        //     typed::write_type2_generic(obit, buffer, self.not_supported_sub_pdu_type, 999);

        // // Type2
        // typed::write_type2_generic(obit, buffer, self.length_of_the_copied_pdu, 8);

        // // Conditional
        // if let Some(ref _value) = self.received_pdu_contents {
        //     unimplemented!();
        //     buffer.write_bits(*_value, 999);
        // }
        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

impl fmt::Display for MmPduFunctionNotSupported {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // write!(f, "MmPduFunctionNotSupported {{ not_supported_pdu_type: {:?} not_supported_sub_pdu_type: {:?} length_of_the_copied_pdu: {:?} received_pdu_contents: {:?} }}",
        write!(
            f,
            "MmPduFunctionNotSupported {{ not_supported_pdu_type: {:?} not_supported_sub_pdu_type: {:?} }}",
            self.not_supported_pdu_type,
            self.not_supported_sub_pdu_type,
            // self.length_of_the_copied_pdu,
            // self.received_pdu_contents,
        )
    }
}

#[cfg(test)]
mod tests {
    use tetra_core::debug;

    use crate::mm::enums::{mm_pdu_type_ul::MmPduTypeUl, status_uplink::StatusUplink};

    use super::*;

    #[test]
    fn test_mm_pdu_function_not_supported_parse() {
        // Self-generated vec!!!
        debug::setup_logging_verbose();
        let test_vec = "111100110";
        let mut buf_in = BitBuffer::from_bitstr(test_vec);
        let pdu = MmPduFunctionNotSupported::from_bitbuf(&mut buf_in).expect("Failed parsing");

        tracing::info!("Parsed: {:?}", pdu);
        tracing::info!("Buf at end: {}", buf_in.dump_bin());

        assert!(buf_in.get_len_remaining() == 0, "Buffer not fully consumed");

        let mut buf_out = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut buf_out).unwrap();
        tracing::info!("Serialized: {}", buf_out.dump_bin());
        assert_eq!(buf_out.to_bitstr(), test_vec);
    }

    #[test]
    fn test_mm_pdu_function_not_support_write() {
        // Self-generated vec!!!
        // 1111 0011 1 10000010
        // |--|                     pdu type
        //      |--|                unsupported pdu type = UMmStatus (0x3)
        //           |              obit
        //             |-----|      unsupported sub pdu type = ChangeOfEnergySavingModeRequest
        //                     |    trailing obit

        debug::setup_logging_verbose();
        let pdu = MmPduFunctionNotSupported {
            not_supported_pdu_type: MmPduTypeUl::UMmStatus as u8,
            not_supported_sub_pdu_type: Some((6, StatusUplink::ChangeOfEnergySavingModeRequest.into())),
        };
        let mut test_buf = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut test_buf).unwrap();

        tracing::info!("Buf at end: {}", test_buf.dump_bin());
        let test_vec = "11110011110000010";

        assert_eq!(test_buf.to_bitstr(), test_vec);
    }

    #[test]
    fn test_mm_pdu_function_not_support_parse_sub_pdu_type() {
        debug::setup_logging_verbose();

        // EN 300 392-2 table 16.27: when the unsupported PDU contains a
        // sub-PDU field, the function-not-supported PDU carries that
        // not-supported sub-PDU type. U-MM STATUS status uplink is 6 bits.
        let test_vec = "11110011110000010";
        let mut buf_in = BitBuffer::from_bitstr(test_vec);
        let pdu = MmPduFunctionNotSupported::from_bitbuf(&mut buf_in).expect("Failed parsing sub-PDU form");

        assert_eq!(pdu.not_supported_pdu_type, MmPduTypeUl::UMmStatus as u8);
        assert_eq!(
            pdu.not_supported_sub_pdu_type,
            Some((6, StatusUplink::ChangeOfEnergySavingModeRequest.into()))
        );
        assert_eq!(buf_in.get_len_remaining(), 0);

        let mut buf_out = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut buf_out).unwrap();
        assert_eq!(buf_out.to_bitstr(), test_vec);
    }

    #[test]
    fn test_mm_pdu_function_not_support_rejects_sub_pdu_type_len_above_64() {
        let pdu = MmPduFunctionNotSupported {
            not_supported_pdu_type: MmPduTypeUl::UMmStatus as u8,
            not_supported_sub_pdu_type: Some((65, 0)),
        };
        let mut test_buf = BitBuffer::new_autoexpand(32);

        assert!(matches!(
            pdu.to_bitbuf(&mut test_buf),
            Err(PduParseErr::InvalidValue {
                field: "not_supported_sub_pdu_type_len",
                ..
            })
        ));
    }

    #[test]
    fn test_mm_pdu_function_not_support_rejects_sub_pdu_type_value_that_exceeds_len() {
        let pdu = MmPduFunctionNotSupported {
            not_supported_pdu_type: MmPduTypeUl::UMmStatus as u8,
            not_supported_sub_pdu_type: Some((6, 0b1_000000)),
        };
        let mut test_buf = BitBuffer::new_autoexpand(32);

        assert!(matches!(
            pdu.to_bitbuf(&mut test_buf),
            Err(PduParseErr::InvalidValue {
                field: "not_supported_sub_pdu_type",
                ..
            })
        ));
    }
}
