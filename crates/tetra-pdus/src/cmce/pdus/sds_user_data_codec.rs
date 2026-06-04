use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};
use tetra_saps::control::enums::sds_user_data::SdsUserData;

const SDS_TYPE4_MAX_BITS: u16 = 2047;
const SDS_TYPE4_MIN_BITS: u16 = 8;

pub(crate) fn read_sds_type4_user_data(buffer: &mut BitBuffer, len_bits: u16) -> Result<SdsUserData, PduParseErr> {
    validate_type4_length(len_bits)?;

    let num_bytes = SdsUserData::type4_declared_byte_count(len_bits).ok_or(PduParseErr::InvalidValue {
        field: "length_indicator",
        value: len_bits as u64,
    })?;
    let mut data = vec![0u8; num_bytes];
    buffer
        .read_bits_into_slice(len_bits as usize, &mut data)
        .ok_or(PduParseErr::BufferEnded {
            field: Some("user_defined_data_4"),
        })?;
    let data = SdsUserData::canonical_type4_bytes(len_bits, &data).ok_or(PduParseErr::InconsistentLength {
        expected: num_bytes,
        found: data.len(),
    })?;
    Ok(SdsUserData::Type4(len_bits, data))
}

pub(crate) fn write_sds_user_data(buffer: &mut BitBuffer, user_defined_data: &SdsUserData) -> Result<(), PduParseErr> {
    buffer.write_bits(user_defined_data.type_identifier() as u64, 2);

    match user_defined_data {
        SdsUserData::Type1(value) => buffer.write_bits(*value as u64, 16),
        SdsUserData::Type2(value) => buffer.write_bits(*value as u64, 32),
        SdsUserData::Type3(value) => buffer.write_bits(*value, 64),
        SdsUserData::Type4(len_bits, data) => {
            validate_type4_payload(*len_bits, data)?;

            buffer.write_bits(*len_bits as u64, 11);
            let full_bytes = (*len_bits as usize) / 8;
            let remaining_bits = (*len_bits as usize) % 8;
            for byte in data.iter().take(full_bytes) {
                buffer.write_bits(*byte as u64, 8);
            }
            if remaining_bits > 0 {
                buffer.write_bits((data[full_bytes] >> (8 - remaining_bits)) as u64, remaining_bits);
            }
        }
    }

    Ok(())
}

fn validate_type4_length(len_bits: u16) -> Result<(), PduParseErr> {
    if len_bits < SDS_TYPE4_MIN_BITS {
        return Err(PduParseErr::InvalidValue {
            field: "length_indicator",
            value: len_bits as u64,
        });
    }

    if len_bits > SDS_TYPE4_MAX_BITS {
        return Err(PduParseErr::InvalidValue {
            field: "length_indicator",
            value: len_bits as u64,
        });
    }

    Ok(())
}

fn validate_type4_payload(len_bits: u16, data: &[u8]) -> Result<(), PduParseErr> {
    // EN 300 392-2 clause 14.8.52: Type 4 SDS carries an 8-bit protocol
    // identifier followed by 0..2039 protocol-dependent bits. Keep inbound
    // parsing and outbound serialization on the same 8..=2047-bit envelope.
    validate_type4_length(len_bits)?;

    let expected_bytes = (len_bits as usize + 7) / 8;
    if data.len() < expected_bytes {
        return Err(PduParseErr::InconsistentLength {
            expected: expected_bytes,
            found: data.len(),
        });
    }

    Ok(())
}
