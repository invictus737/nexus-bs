#[derive(Debug, PartialEq, Eq)]
pub struct Type4FieldGeneric {
    pub field_id: u64,
    pub len: usize,
    pub elems: usize,
    /// Up to 64 bits of data (later bits are discarded)
    pub data: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Type3FieldGeneric {
    pub field_id: u64,
    pub len: usize,
    /// Up to 128 bits of data (later bits are discarded).
    /// Sized to fit External Subscriber Number (up to 96 bits = 24 BCD digits)
    /// and any future long IEs without further refactor.
    pub data: u128,
}

/// Helper functions for dealing with type2, type3 and type4 fields for MLE, CMCE, MM and SNDCP PDUs.
pub mod delimiters {
    use crate::{bitbuffer::BitBuffer, pdu_parse_error::PduParseErr};

    /// Read the o-bit between type1 and type2/type3 elements
    pub fn read_obit(buffer: &mut BitBuffer) -> Result<bool, PduParseErr> {
        Ok(buffer.read_field(1, "obit")? == 1)
    }

    /// Write the o-bit between type1 and type2/type3 elements
    pub fn write_obit(buffer: &mut BitBuffer, val: u8) {
        buffer.write_bit(val);
    }

    /// Read a p-bit preceding a type2 element
    pub fn read_pbit(buffer: &mut BitBuffer) -> Result<bool, PduParseErr> {
        Ok(buffer.read_field(1, "pbit")? == 1)
    }

    /// Write the p-bit preceding a type2 element
    pub fn write_pbit(buffer: &mut BitBuffer, val: u8) {
        buffer.write_bit(val);
    }

    /// Read an m-bit found before a type3 or type4 element, and trailing the message
    pub fn read_mbit(buffer: &mut BitBuffer) -> Result<bool, PduParseErr> {
        Ok(buffer.read_field(1, "mbit")? == 1)
    }

    /// Write the m-bit before a type3 or type4 element, and trailing the message
    pub fn write_mbit(buffer: &mut BitBuffer, val: u8) {
        buffer.write_bit(val);
    }
}

pub mod typed {
    use crate::{
        bitbuffer::BitBuffer,
        pdu_parse_error::PduParseErr,
        typed_pdu_fields::{Type3FieldGeneric, Type4FieldGeneric, delimiters},
    };

    pub fn parse_type2_generic(
        obit: bool,
        buffer: &mut BitBuffer,
        num_bits: usize,
        field_name: &'static str,
    ) -> Result<Option<u64>, PduParseErr> {
        if !obit {
            return Ok(None);
        }
        match delimiters::read_pbit(buffer) {
            Ok(true) => {
                // Field present
                tracing::trace!("parse_type2_generic field_present {:20}: {}", field_name, buffer.dump_bin());
                match buffer.read_field(num_bits, field_name) {
                    Ok(v) => Ok(Some(v)),
                    Err(e) => Err(e),
                }
            }
            Ok(false) => {
                // Field not present
                tracing::trace!("parse_type2_generic no_field      {:20}: {}", field_name, buffer.dump_bin());
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Parse a Type-2 element into a struct that implements `from_bitbuf`.
    pub fn parse_type2_struct<T, F>(obit: bool, buffer: &mut BitBuffer, parser: F) -> Result<Option<T>, PduParseErr>
    where
        F: FnOnce(&mut BitBuffer) -> Result<T, PduParseErr>,
    {
        if !obit {
            return Ok(None);
        }

        match delimiters::read_pbit(buffer) {
            Ok(true) => {
                // Field present
                tracing::trace!("parse_type2_struct field_present: {}", buffer.dump_bin());
                let value = parser(buffer)?;
                Ok(Some(value))
            }
            Ok(false) => {
                // Field not present
                tracing::trace!("parse_type2_struct no_field      : {}", buffer.dump_bin());
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Write one Type-2 element.
    /// If `value` is `Some(v)`, writes P-bit=1 then `len` bits of `v`. If `None`, writes P-bit=0.
    pub fn write_type2_generic(obit: bool, buffer: &mut BitBuffer, value: Option<u64>, len: usize) {
        // No optional elements
        if !obit {
            assert!(value.is_none(), "Type2 element cannot be present when obit is false");
            return;
        }

        match value {
            Some(v) => {
                tracing::trace!("write_type2_generic field_present {}", buffer.dump_bin());
                delimiters::write_pbit(buffer, 1);
                buffer.write_bits(v, len);
            }
            None => {
                tracing::trace!("write_type2_generic no_field {}", buffer.dump_bin());
                delimiters::write_pbit(buffer, 0);
            }
        }
    }

    /// Write a Type-2 element from a struct that implements `to_bitbuf`.
    pub fn write_type2_struct<T, F>(obit: bool, buffer: &mut BitBuffer, value: &Option<T>, writer: F) -> Result<(), PduParseErr>
    where
        F: Fn(&T, &mut BitBuffer) -> Result<(), PduParseErr>,
    {
        // No optional elements
        if !obit {
            assert!(value.is_none(), "Type2 element cannot be present when obit is false");
            return Ok(());
        }
        match value {
            Some(v) => {
                tracing::trace!("write_type2_struct field_present {}", buffer.dump_bin());
                delimiters::write_pbit(buffer, 1);
                writer(v, buffer)?;
                Ok(())
            }
            None => {
                tracing::trace!("write_type2_struct no_field {}", buffer.dump_bin());
                delimiters::write_pbit(buffer, 0);
                Ok(())
            }
        }
    }

    /// Read the m-bit for a type3 or type4 element without advancing the buffer pos
    /// If set, reads the type3/4 field identifier and compares to expected id.
    /// Return true if present, false if not present, or PduParseErr on error
    fn peek_type34_mbit_and_id(buffer: &BitBuffer, expected_id: u64) -> Result<bool, PduParseErr> {
        let mbit = buffer.peek_bits(1);
        match mbit {
            Some(0) => {
                // Field not present
                Ok(false)
            }
            Some(1) => {
                // Some field is present, read and compare id
                let id_bits = buffer.peek_bits_posoffset(1, 4);
                match id_bits {
                    Some(id) if id == expected_id => {
                        // The expected is here; the field exists
                        Ok(true)
                    }
                    Some(_) => {
                        // Some different field is here
                        Ok(false)
                    }
                    None => {
                        // Read failed
                        Err(PduParseErr::BufferEnded {
                            field: Some("peek_type34_mbit_and_id id_bits"),
                        })
                    }
                }
            }
            None => Err(PduParseErr::BufferEnded {
                field: Some("peek_type34_mbit_and_id mbit"),
            }),
            _ => panic!(), // Never happens
        }
    }

    /// Parse type3 field into a placeholder struct, pending implementation.
    /// Checks whether a given type3 field identifier is present. If not, returns None without advancing
    /// the bitbuffer position. If present, reads the element and returns it as a u64, advancing the buffer position.
    /// to the end of the element.
    pub fn parse_type3_generic<E>(obit: bool, buffer: &mut BitBuffer, expected_id: E) -> Result<Option<Type3FieldGeneric>, PduParseErr>
    where
        E: Into<u64>,
    {
        // If the obit is set to false, the element cannot be present
        if !obit {
            return Ok(None);
        }

        // Obit is present, check if mbit present, and check if the elementid is the expected one
        let id = expected_id.into();
        let field_present = peek_type34_mbit_and_id(buffer, id)?;
        if !field_present {
            return Ok(None);
        }

        // Target field is present. Advance buffer position and read field contents
        buffer.seek_rel(5);
        let len_bits = match buffer.read_bits(11) {
            Some(x) => x as usize,
            None => {
                return Err(PduParseErr::BufferEnded {
                    field: Some("parse_type3_generic len_bits"),
                });
            }
        };

        // Read up to 128 bits of payload. BitBuffer::read_bits is u64-only, so for
        // lengths over 64 we split into two reads (high half first, then low half).
        let read_bits = if len_bits > 128 { 128 } else { len_bits };
        let data: u128 = if read_bits <= 64 {
            let v = match buffer.read_bits(read_bits) {
                Some(x) => x,
                None => {
                    return Err(PduParseErr::BufferEnded {
                        field: Some("parse_type3_generic data"),
                    });
                }
            };
            v as u128
        } else {
            let hi_bits = read_bits - 64;
            let hi = match buffer.read_bits(hi_bits) {
                Some(x) => x,
                None => {
                    return Err(PduParseErr::BufferEnded {
                        field: Some("parse_type3_generic data (high)"),
                    });
                }
            };
            let lo = match buffer.read_bits(64) {
                Some(x) => x,
                None => {
                    return Err(PduParseErr::BufferEnded {
                        field: Some("parse_type3_generic data (low)"),
                    });
                }
            };
            ((hi as u128) << 64) | (lo as u128)
        };

        // Seek forward past any bits beyond what we stored (>128 bits).
        if len_bits > 128 {
            tracing::warn!("Type3 element {} length {} exceeds 128 bits, data truncated", id, len_bits);
            buffer.seek_rel(len_bits as isize - 128);
        }

        Ok(Some(Type3FieldGeneric {
            field_id: id,
            len: len_bits,
            data,
        }))
    }

    /// Parse a Type-3 element into a struct that implements `from_bitbuf`.
    /// Validates the m-bit and element ID, then calls the parser function directly on the buffer if present.
    pub fn parse_type3_struct<E, T, F>(obit: bool, buffer: &mut BitBuffer, expected_id: E, parser: F) -> Result<Option<T>, PduParseErr>
    where
        E: Into<u64>,
        F: FnOnce(&mut BitBuffer) -> Result<T, PduParseErr>,
    {
        // If the obit is set to false, the element cannot be present
        if !obit {
            return Ok(None);
        }

        // Obit is present, peek if mbit present, and peek if the elementid is the expected one
        let id = expected_id.into();
        let field_present = peek_type34_mbit_and_id(buffer, id)?;
        if !field_present {
            tracing::trace!("parse_type3_struct no_field {}: {}", id, buffer.dump_bin());
            return Ok(None);
        }
        // Target field is present. Advance buffer past m-bit (1) + id (4) + length (11)
        buffer.seek_rel(5); // m-bit + id

        tracing::trace!("parse_type3_struct got header for {:2}: {}", id, buffer.dump_bin());

        let len_bits = match buffer.read_bits(11) {
            Some(x) => x as usize,
            None => {
                return Err(PduParseErr::BufferEnded {
                    field: Some("parse_type3_struct len_bits"),
                });
            }
        };

        tracing::trace!("parse_type3_struct got len {:4}:      {}", len_bits, buffer.dump_bin());

        // Store current position to check parsed length for discrepancies. Then, read length
        let start_pos = buffer.get_pos();

        // Now buffer is positioned at the data. Parse the struct directly. The parser is responsible for reading exactly len_bits
        let result = parser(buffer)?;

        tracing::trace!("parse_type3_struct done parsing:      {}", buffer.dump_bin());

        // If read out length does not match expectation, something went very wrong
        if start_pos + len_bits != buffer.get_pos() {
            tracing::warn!(
                "Type3 element {} parsed length mismatch: expected {}, parsed {}",
                id,
                len_bits,
                buffer.get_pos() - start_pos
            );
            return Err(PduParseErr::InconsistentLength {
                expected: len_bits,
                found: (buffer.get_pos() - start_pos) as usize,
            });
        };

        // Parsed and expected length matches, return result
        Ok(Some(result))
    }

    /// Write the type4 header start (1-bit mbit + 4-bit field type)
    pub fn write_type34_header_generic(buffer: &mut BitBuffer, field_type: u64) {
        delimiters::write_mbit(buffer, 1);
        buffer.write_bits(field_type, 4);
    }

    /// Write an optional Type-3 element using a `to_bitbuf` function.
    pub fn write_type3_struct<E, T, F>(
        obit: bool,
        buffer: &mut BitBuffer,
        value: &Option<T>,
        field_id: E,
        writer: F,
    ) -> Result<(), PduParseErr>
    where
        E: Into<u64>,
        F: Fn(&T, &mut BitBuffer) -> Result<(), PduParseErr>,
    {
        // Sanity check
        let id = field_id.into();
        if !obit && value.is_some() {
            return Err(PduParseErr::InvalidValue {
                field: "write_type3_struct",
                value: id,
            });
        }

        if let Some(elem) = value {
            tracing::trace!("write_type3_struct writing field {:2} {}", id, buffer.dump_bin());

            // Write mbit and 4-bit field ID, then length field, then write the element itself
            write_type34_header_generic(buffer, id);
            let pos_len_field = buffer.get_raw_pos();
            buffer.write_bits(0, 11); // Write instead of seek to autoexpand

            tracing::trace!("write_type3_struct header           {}", buffer.dump_bin());

            writer(elem, buffer)?;

            tracing::trace!("write_type3_struct payload          {}", buffer.dump_bin());

            // Calculate actual length and backfill
            let pos_end = buffer.get_raw_pos();
            let len_bits = (pos_end - pos_len_field - 11) as u64;
            buffer.set_raw_pos(pos_len_field);
            buffer.write_bits(len_bits, 11);

            tracing::trace!("write_type3_struct len {:2}:          {}", len_bits, buffer.dump_bin());
            buffer.set_raw_pos(pos_end);
        } else {
            // Don't write anything (no mbit)
            tracing::trace!("write_type3_struct no_field          {}", buffer.dump_bin());
        }
        Ok(())
    }

    /// Write an optional Type-3 element using a `to_bitbuf` function.
    pub fn write_type3_generic<E>(
        obit: bool,
        buffer: &mut BitBuffer,
        value: &Option<Type3FieldGeneric>,
        field_id: E,
    ) -> Result<(), PduParseErr>
    where
        E: Into<u64>,
    {
        // Sanity check
        let id = field_id.into();
        if !obit && value.is_some() {
            return Err(PduParseErr::InvalidValue {
                field: "write_type3_generic",
                value: id,
            });
        }

        if let Some(elem) = value {
            tracing::trace!("write_type3_generic field_present {}", buffer.dump_bin());
            // Write mbit and 4-bit field ID, then write length, then the element itself
            write_type34_header_generic(buffer, id);
            buffer.write_bits(elem.len as u64, 11);
            // BitBuffer::write_bits accepts u64. For payloads up to 64 bits we cast
            // directly; for longer payloads we split into high-half + low-half writes.
            if elem.len <= 64 {
                buffer.write_bits(elem.data as u64, elem.len);
            } else {
                let hi_bits = elem.len - 64;
                let hi = (elem.data >> 64) as u64;
                let lo = elem.data as u64;
                buffer.write_bits(hi, hi_bits);
                buffer.write_bits(lo, 64);
            }
        } else {
            // Don't write anything (no mbit)
            tracing::trace!("write_type3_generic no_field {}", buffer.dump_bin());
        }
        Ok(())
    }

    fn parse_type4_header(buffer: &mut BitBuffer, expected_id: u64) -> Result<Option<(usize, usize)>, PduParseErr> {
        // Check whether the element is present
        let id = expected_id.into();
        let field_present = peek_type34_mbit_and_id(buffer, id)?;
        if !field_present {
            return Ok(None);
        }

        // Target field is present. Advance buffer position and read field contents
        buffer.seek_rel(5);
        let len_bits = match buffer.read_bits(11) {
            Some(x) => x as usize,
            None => {
                return Err(PduParseErr::BufferEnded {
                    field: Some("parse_type4_header len_bits"),
                });
            }
        };
        // tracing::debug!("MmType4FieldUl: len_bits: {}", len_bits);
        let num_elems = match buffer.read_bits(6) {
            Some(x) => x as usize,
            None => {
                return Err(PduParseErr::BufferEnded {
                    field: Some("parse_type4_header num_elems"),
                });
            }
        };

        tracing::trace!(
            "parse_type4_header got header for {:2}, len {}, count {}: {}",
            id,
            len_bits,
            num_elems,
            buffer.dump_bin()
        );

        if len_bits < 6 {
            return Err(PduParseErr::InconsistentLength {
                expected: 6,
                found: len_bits,
            });
        }
        if num_elems == 0 {
            return Err(PduParseErr::InvalidValue {
                field: "parse_type4_header num_elems",
                value: 0,
            });
        }

        Ok(Some((num_elems, len_bits - 6)))
    }

    /// Parse a Type-4 element into a Vec of structs that implement `from_bitbuf`.
    pub fn parse_type4_struct<E, T, F>(obit: bool, buffer: &mut BitBuffer, expected_id: E, parser: F) -> Result<Option<Vec<T>>, PduParseErr>
    where
        E: Into<u64>,
        F: Fn(&mut BitBuffer) -> Result<T, PduParseErr>,
    {
        // If the obit is set to false, the element cannot be present
        if !obit {
            return Ok(None);
        }

        // Obit is present, check if mbit present, and check if the elementid is the expected one
        let id = expected_id.into();
        match parse_type4_header(buffer, id)? {
            None => {
                // Field not present
                Ok(None)
            }
            Some((num_elems, len_bits)) => {
                // Field is present, and we've gout our total lenght and number of elements
                let mut elems = Vec::with_capacity(num_elems);
                let start_pos = buffer.get_pos();

                // Parse all elements into array structs
                for _ in 0..num_elems {
                    let elem = parser(buffer)?;
                    elems.push(elem);
                }

                // If read out length does not match expectation, something went very wrong
                if start_pos + len_bits != buffer.get_pos() {
                    tracing::warn!(
                        "Type4 element {} parsed length mismatch: expected {}, parsed {}",
                        id,
                        len_bits,
                        buffer.get_pos() - start_pos
                    );
                    return Err(PduParseErr::InconsistentLength {
                        expected: len_bits,
                        found: (buffer.get_pos() - start_pos) as usize,
                    });
                };

                // Parsed and expected length matches, return result
                Ok(Some(elems))
            }
        }
    }

    /// Parse a Type-4 element into a placeholder struct type, pending proper implementation.
    /// Imperfect as we cannot know individual element sizes, besides issues with overflowing the 64-bit read
    pub fn parse_type4_generic<E>(obit: bool, buffer: &mut BitBuffer, expected_id: E) -> Result<Option<Type4FieldGeneric>, PduParseErr>
    where
        E: Into<u64>,
    {
        // If the obit is set to false, the element cannot be present
        if !obit {
            return Ok(None);
        }

        // Obit is present, check if mbit present, and check if the elementid is the expected one
        let id = expected_id.into();
        match parse_type4_header(buffer, id)? {
            None => {
                // Field not present
                Ok(None)
            }
            Some((num_elems, len_bits)) => {
                // Field is present, and we've got our total lenght and number of elements
                let read_bits = if len_bits > 64 { 64 } else { len_bits };
                let val = buffer.read_field(read_bits, "parse_type4_header")?;

                // Build placeholder return struct
                let ret = Type4FieldGeneric {
                    field_id: id,
                    len: len_bits,
                    elems: num_elems,
                    data: val,
                };

                // Seek forward to end of element, if larger than 64 bits
                if len_bits > 64 {
                    tracing::warn!("Type4 element {} length {} exceeds 64 bits, data truncated", id, len_bits);
                    buffer.seek_rel(len_bits as isize - 64);
                }

                // Parsed and expected length matches, return result
                Ok(Some(ret))
            }
        }
    }

    /// Write a Type-4 element from a Vec of structs using a `to_bitbuf` function.
    pub fn write_type4_struct<E, T, F>(
        obit: bool,
        buffer: &mut BitBuffer,
        value: &Option<Vec<T>>,
        field_id: E,
        writer: F,
    ) -> Result<(), PduParseErr>
    where
        E: Into<u64>,
        F: Fn(&T, &mut BitBuffer) -> Result<(), PduParseErr>,
    {
        // Sanity check
        let id = field_id.into();
        if !obit && value.is_some() {
            return Err(PduParseErr::InvalidValue {
                field: "write_type4_struct",
                value: id,
            });
        }

        if let Some(elems) = value {
            if elems.is_empty() {
                return Err(PduParseErr::InvalidValue {
                    field: "write_type4_struct elems",
                    value: 0,
                });
            }
            if elems.len() > 63 {
                return Err(PduParseErr::InvalidValue {
                    field: "write_type4_struct elems",
                    value: elems.len() as u64,
                });
            }

            // EN 300 392-2 Annex E encodes the Type-4 length as the 6-bit
            // element count plus all repeated sub-elements. Build the payload
            // first so validation failures do not leave a partial IE in the
            // destination buffer.
            let mut payload = BitBuffer::new_autoexpand((elems.len() * 64).max(64));
            for elem in elems {
                writer(elem, &mut payload)?;
            }
            let payload_len = payload.get_len();
            let len_bits = payload_len + 6;
            if len_bits > 2047 {
                return Err(PduParseErr::InvalidValue {
                    field: "write_type4_struct len",
                    value: len_bits as u64,
                });
            }

            write_type34_header_generic(buffer, id);
            buffer.write_bits(len_bits as u64, 11);
            buffer.write_bits(elems.len() as u64, 6);
            payload.seek(0);
            buffer.copy_bits(&mut payload, payload_len);
        }
        // If None, don't write anything (no m-bit)
        Ok(())
    }

    /// Write a parsed generic Type-4 element.
    ///
    /// EN 300 392-2 Annex E encodes Type-4 optional information elements as
    /// M-bit + element id + total bit length + element count + payload. The
    /// generic parser stores only the first 64 payload bits, so generic
    /// re-serialization is safe only while the whole payload fits in that
    /// stored value.
    pub fn write_type4_todo<E>(
        obit: bool,
        buffer: &mut BitBuffer,
        value: &Option<Type4FieldGeneric>,
        field_id: E,
    ) -> Result<(), PduParseErr>
    where
        E: Into<u64>,
    {
        // Sanity check
        let id = field_id.into();
        if !obit && value.is_some() {
            return Err(PduParseErr::InvalidValue {
                field: "write_type4_todo",
                value: id,
            });
        }

        if let Some(_elem) = value {
            let elem = _elem;
            if elem.field_id != id {
                return Err(PduParseErr::InvalidValue {
                    field: "write_type4_todo field_id",
                    value: elem.field_id,
                });
            }
            if elem.len > 64 {
                return Err(PduParseErr::NotImplemented {
                    field: Some("write_type4_todo truncated_payload"),
                });
            }
            if elem.len + 6 > 2047 {
                return Err(PduParseErr::InvalidValue {
                    field: "write_type4_todo len",
                    value: elem.len as u64,
                });
            }
            if elem.elems > 63 {
                return Err(PduParseErr::InvalidValue {
                    field: "write_type4_todo elems",
                    value: elem.elems as u64,
                });
            }
            if elem.len < 64 && elem.data >= (1u64 << elem.len) {
                return Err(PduParseErr::InvalidValue {
                    field: "write_type4_todo data",
                    value: elem.data,
                });
            }

            write_type34_header_generic(buffer, id);
            buffer.write_bits((elem.len + 6) as u64, 11);
            buffer.write_bits(elem.elems as u64, 6);
            buffer.write_bits(elem.data, elem.len);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Type4FieldGeneric, typed};
    use crate::{BitBuffer, pdu_parse_error::PduParseErr};

    #[test]
    fn generic_type4_round_trips_when_payload_is_fully_retained() {
        let elem = Type4FieldGeneric {
            field_id: 3,
            len: 12,
            elems: 2,
            data: 0x0abc,
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        typed::write_type4_todo(true, &mut buf, &Some(elem), 3u8).expect("generic Type4 write should succeed");
        buf.seek(0);
        let parsed = typed::parse_type4_generic(true, &mut buf, 3u8)
            .expect("generic Type4 should parse")
            .expect("generic Type4 should be present");

        assert_eq!(
            parsed,
            Type4FieldGeneric {
                field_id: 3,
                len: 12,
                elems: 2,
                data: 0x0abc,
            }
        );
        assert_eq!(buf.get_len_remaining(), 0);
    }

    #[test]
    fn generic_type4_returns_typed_error_for_truncated_payload() {
        let elem = Type4FieldGeneric {
            field_id: 3,
            len: 65,
            elems: 1,
            data: 0,
        };
        let mut buf = BitBuffer::new_autoexpand(32);

        assert_eq!(
            typed::write_type4_todo(true, &mut buf, &Some(elem), 3u8),
            Err(PduParseErr::NotImplemented {
                field: Some("write_type4_todo truncated_payload"),
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn generic_type4_rejects_length_shorter_than_count_field() {
        let mut buf = BitBuffer::new_autoexpand(32);
        typed::write_type34_header_generic(&mut buf, 3);
        buf.write_bits(5, 11);
        buf.write_bits(1, 6);
        buf.seek(0);

        assert_eq!(
            typed::parse_type4_generic(true, &mut buf, 3u8),
            Err(PduParseErr::InconsistentLength { expected: 6, found: 5 })
        );
    }

    #[test]
    fn generic_type4_rejects_zero_repeated_elements() {
        let mut buf = BitBuffer::new_autoexpand(32);
        typed::write_type34_header_generic(&mut buf, 3);
        buf.write_bits(6, 11);
        buf.write_bits(0, 6);
        buf.seek(0);

        assert_eq!(
            typed::parse_type4_generic(true, &mut buf, 3u8),
            Err(PduParseErr::InvalidValue {
                field: "parse_type4_header num_elems",
                value: 0,
            })
        );
    }

    #[test]
    fn structured_type4_rejects_empty_collection_without_mutating_buffer() {
        let mut buf = BitBuffer::new_autoexpand(32);
        let elems: Option<Vec<u8>> = Some(vec![]);

        assert_eq!(
            typed::write_type4_struct(true, &mut buf, &elems, 3u8, |elem, out| {
                out.write_bits(*elem as u64, 3);
                Ok(())
            }),
            Err(PduParseErr::InvalidValue {
                field: "write_type4_struct elems",
                value: 0,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn structured_type4_rejects_too_many_elements_without_mutating_buffer() {
        let mut buf = BitBuffer::new_autoexpand(32);
        let elems = Some(vec![0u8; 64]);

        assert_eq!(
            typed::write_type4_struct(true, &mut buf, &elems, 3u8, |elem, out| {
                out.write_bits(*elem as u64, 3);
                Ok(())
            }),
            Err(PduParseErr::InvalidValue {
                field: "write_type4_struct elems",
                value: 64,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }

    #[test]
    fn structured_type4_rejects_overwide_length_without_mutating_buffer() {
        let mut buf = BitBuffer::new_autoexpand(32);
        let elems = Some(vec![0u8]);

        assert_eq!(
            typed::write_type4_struct(true, &mut buf, &elems, 3u8, |_elem, out| {
                out.write_zeroes(2042);
                Ok(())
            }),
            Err(PduParseErr::InvalidValue {
                field: "write_type4_struct len",
                value: 2048,
            })
        );
        assert_eq!(buf.get_len(), 0);
    }
}
