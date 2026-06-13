// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdsUserData {
    /// Type field 0, 16 bits, short_data_type_identifier == 0
    Type1(u16),
    /// Type field 1, 32 bits, short_data_type_identifier == 1
    Type2(u32),
    /// Type field 2, 64 bits, short_data_type_identifier == 2
    Type3(u64),
    /// Type field 3, variable length, short_data_type_identifier == 3
    Type4(u16, Vec<u8>),
}

impl SdsUserData {
    pub const TYPE4_MAX_BITS: u16 = 2047;

    pub fn type_identifier(&self) -> u8 {
        match self {
            SdsUserData::Type1(_) => 0,
            SdsUserData::Type2(_) => 1,
            SdsUserData::Type3(_) => 2,
            SdsUserData::Type4(_, _) => 3,
        }
    }

    pub fn length_bits(&self) -> u16 {
        match self {
            SdsUserData::Type1(_) => 16,
            SdsUserData::Type2(_) => 32,
            SdsUserData::Type3(_) => 64,
            SdsUserData::Type4(len_bits, _) => *len_bits,
        }
    }

    pub fn type4_declared_byte_count(len_bits: u16) -> Option<usize> {
        if len_bits > Self::TYPE4_MAX_BITS {
            return None;
        }
        Some(usize::from(len_bits).div_ceil(8))
    }

    pub fn canonical_type4_bytes(len_bits: u16, data: &[u8]) -> Option<Vec<u8>> {
        let expected_bytes = Self::type4_declared_byte_count(len_bits)?;
        if data.len() < expected_bytes {
            return None;
        }

        let mut out = data[..expected_bytes].to_vec();
        let remaining_bits = usize::from(len_bits) % 8;
        if remaining_bits > 0
            && let Some(last) = out.last_mut()
        {
            *last &= 0xffu8 << (8 - remaining_bits);
        }
        Some(out)
    }

    pub fn to_arr(&self) -> Vec<u8> {
        match self {
            SdsUserData::Type1(value) => value.to_be_bytes().to_vec(),
            SdsUserData::Type2(value) => value.to_be_bytes().to_vec(),
            SdsUserData::Type3(value) => value.to_be_bytes().to_vec(),
            SdsUserData::Type4(len_bits, data) => Self::canonical_type4_bytes(*len_bits, data).unwrap_or_else(|| data.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type4_canonical_bytes_truncates_and_masks_to_declared_bits() {
        let data = vec![0x82, 0b1010_1100, 0b1100_1111, 0x55];

        assert_eq!(
            SdsUserData::canonical_type4_bytes(20, &data),
            Some(vec![0x82, 0b1010_1100, 0b1100_0000])
        );
        assert_eq!(SdsUserData::Type4(20, data).to_arr(), vec![0x82, 0b1010_1100, 0b1100_0000]);
    }

    #[test]
    fn type4_canonical_bytes_rejects_inconsistent_length() {
        assert_eq!(SdsUserData::canonical_type4_bytes(2048, &[0; 256]), None);
        assert_eq!(SdsUserData::canonical_type4_bytes(17, &[0; 2]), None);
    }
}
