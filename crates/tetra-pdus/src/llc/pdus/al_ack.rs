// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original LLC Advanced Link PDU support.

use core::fmt;

use tetra_core::pdu_parse_error::*;
use tetra_core::{BitBuffer, expect_value, let_field};

/// EN 300 392-2 clause 21.2.3.1 original AL-ACK/AL-RNR acknowledgement block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlAckBlock {
    pub nr: u8,
    pub acknowledgement_length: u8,
    pub sr: Option<u8>,
    pub acknowledgement_bitmap: u64,
}

/// EN 300 392-2 clause 21.2.3.1 original AL-ACK/AL-RNR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlAck {
    pub receiver_ready: bool,
    pub nr: u8,
    pub acknowledgement_length: u8,
    pub sr: Option<u8>,
    pub acknowledgement_bitmap: u64,
    pub acknowledgement_blocks: Vec<AlAckBlock>,
}

impl AlAckBlock {
    pub const ACK_LENGTH_COMPLETE_TL_SDU: u8 = 0;
    pub const ACK_LENGTH_REPEAT_ENTIRE_TL_SDU: u8 = 0b111111;
    pub const ACK_LENGTH_MAX_SELECTIVE_SEGMENTS: u8 = 0b111110;
    pub const SR_REST_OF_SDU_CORRECTLY_RECEIVED: u8 = 0b11111010;

    pub fn complete(nr: u8) -> Self {
        Self {
            nr,
            acknowledgement_length: Self::ACK_LENGTH_COMPLETE_TL_SDU,
            sr: None,
            acknowledgement_bitmap: 0,
        }
    }

    pub fn repeat_entire(nr: u8) -> Self {
        Self {
            nr,
            acknowledgement_length: Self::ACK_LENGTH_REPEAT_ENTIRE_TL_SDU,
            sr: None,
            acknowledgement_bitmap: 0,
        }
    }

    pub fn selective(nr: u8, sr: u8, acknowledgement_bitmap: u64, acknowledgement_length: u8) -> Self {
        Self {
            nr,
            acknowledgement_length: acknowledgement_length.clamp(1, Self::ACK_LENGTH_MAX_SELECTIVE_SEGMENTS),
            sr: Some(sr),
            acknowledgement_bitmap,
        }
    }

    fn from_bitbuf_block(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let_field!(buf, nr, 3);
        let_field!(buf, acknowledgement_length, 6);
        let mut sr = None;
        let mut acknowledgement_bitmap = 0u64;
        if (1..=Self::ACK_LENGTH_MAX_SELECTIVE_SEGMENTS).contains(&(acknowledgement_length as u8)) {
            let Some(sr_bits) = buf.read_bits(8) else {
                return Err(PduParseErr::BufferEnded { field: Some("sr") });
            };
            sr = Some(sr_bits as u8);
            for bit_idx in 0..acknowledgement_length.saturating_sub(1) {
                let Some(bit) = buf.read_bits(1) else {
                    return Err(PduParseErr::BufferEnded {
                        field: Some("acknowledgement_bitmap"),
                    });
                };
                if bit != 0 {
                    acknowledgement_bitmap |= 1u64 << bit_idx;
                }
            }
        }

        Ok(Self {
            nr: nr as u8,
            acknowledgement_length: acknowledgement_length as u8,
            sr,
            acknowledgement_bitmap,
        })
    }

    fn to_bitbuf_block(&self, buf: &mut BitBuffer) {
        buf.write_bits(self.nr as u64, 3);
        buf.write_bits(self.acknowledgement_length as u64, 6);
        if (1..=Self::ACK_LENGTH_MAX_SELECTIVE_SEGMENTS).contains(&self.acknowledgement_length) {
            buf.write_bits(self.sr.unwrap_or(0) as u64, 8);
            for bit_idx in 0..self.acknowledgement_length.saturating_sub(1) {
                buf.write_bits((self.acknowledgement_bitmap >> bit_idx) & 1, 1);
            }
        }
    }

    pub fn acknowledges_complete_tl_sdu(&self) -> bool {
        self.acknowledgement_length == Self::ACK_LENGTH_COMPLETE_TL_SDU
    }

    pub fn requests_repeat_entire_tl_sdu(&self) -> bool {
        self.acknowledgement_length == Self::ACK_LENGTH_REPEAT_ENTIRE_TL_SDU
    }

    pub fn is_selective_segment_ack(&self) -> bool {
        (1..=Self::ACK_LENGTH_MAX_SELECTIVE_SEGMENTS).contains(&self.acknowledgement_length) && self.sr.is_some()
    }

    pub fn is_rest_of_sdu_correctly_received(&self) -> bool {
        self.acknowledgement_length == 1 && self.sr == Some(Self::SR_REST_OF_SDU_CORRECTLY_RECEIVED)
    }

    pub fn segment_acknowledged(&self, ss: u8) -> Option<bool> {
        let sr = self.sr?;
        if !self.is_selective_segment_ack() {
            return None;
        }
        if self.is_rest_of_sdu_correctly_received() {
            return Some(true);
        }
        if ss < sr {
            return Some(true);
        }
        if ss == sr {
            return Some(false);
        }
        let offset = ss.saturating_sub(sr) as usize;
        if offset >= self.acknowledgement_length as usize {
            return None;
        }
        Some(((self.acknowledgement_bitmap >> (offset - 1)) & 1) != 0)
    }
}

impl AlAck {
    pub const ACK_LENGTH_COMPLETE_TL_SDU: u8 = AlAckBlock::ACK_LENGTH_COMPLETE_TL_SDU;
    pub const ACK_LENGTH_REPEAT_ENTIRE_TL_SDU: u8 = AlAckBlock::ACK_LENGTH_REPEAT_ENTIRE_TL_SDU;
    pub const ACK_LENGTH_MAX_SELECTIVE_SEGMENTS: u8 = AlAckBlock::ACK_LENGTH_MAX_SELECTIVE_SEGMENTS;
    pub const SR_REST_OF_SDU_CORRECTLY_RECEIVED: u8 = AlAckBlock::SR_REST_OF_SDU_CORRECTLY_RECEIVED;

    pub fn complete(nr: u8) -> Self {
        Self::with_blocks(true, vec![AlAckBlock::complete(nr)])
    }

    pub fn repeat_entire(nr: u8) -> Self {
        Self::with_blocks(true, vec![AlAckBlock::repeat_entire(nr)])
    }

    pub fn selective(receiver_ready: bool, nr: u8, sr: u8, acknowledgement_bitmap: u64, acknowledgement_length: u8) -> Self {
        Self::with_blocks(
            receiver_ready,
            vec![AlAckBlock::selective(nr, sr, acknowledgement_bitmap, acknowledgement_length)],
        )
    }

    pub fn with_blocks(receiver_ready: bool, acknowledgement_blocks: Vec<AlAckBlock>) -> Self {
        let first = acknowledgement_blocks.first().copied().unwrap_or_else(|| AlAckBlock::complete(0));
        Self {
            receiver_ready,
            nr: first.nr,
            acknowledgement_length: first.acknowledgement_length,
            sr: first.sr,
            acknowledgement_bitmap: first.acknowledgement_bitmap,
            acknowledgement_blocks,
        }
    }

    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let_field!(buf, llc_pdu_type, 4);
        expect_value!(llc_pdu_type, 11)?;
        let_field!(buf, receiver_ready, 1);

        let mut acknowledgement_blocks = vec![AlAckBlock::from_bitbuf_block(buf)?];
        while buf.get_len_remaining() >= 9 {
            acknowledgement_blocks.push(AlAckBlock::from_bitbuf_block(buf)?);
        }
        if buf.get_len_remaining() > 0 {
            let remaining_bits = buf.get_len_remaining();
            let trailing = buf.read_bits(remaining_bits).ok_or(PduParseErr::BufferEnded {
                field: Some("al_ack_trailing_padding"),
            })?;
            if trailing != 0 {
                return Err(PduParseErr::InvalidValue {
                    field: "al_ack_trailing_padding",
                    value: trailing,
                });
            }
        }

        Ok(Self::with_blocks(receiver_ready != 0, acknowledgement_blocks))
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        buf.write_bits(11, 4);
        buf.write_bits(self.receiver_ready as u64, 1);
        if self.acknowledgement_blocks.is_empty() {
            AlAckBlock {
                nr: self.nr,
                acknowledgement_length: self.acknowledgement_length,
                sr: self.sr,
                acknowledgement_bitmap: self.acknowledgement_bitmap,
            }
            .to_bitbuf_block(buf);
            return;
        }
        for block in &self.acknowledgement_blocks {
            block.to_bitbuf_block(buf);
        }
    }

    pub fn acknowledges_complete_tl_sdu(&self) -> bool {
        self.acknowledgement_blocks
            .first()
            .is_some_and(AlAckBlock::acknowledges_complete_tl_sdu)
    }

    pub fn requests_repeat_entire_tl_sdu(&self) -> bool {
        self.acknowledgement_blocks
            .first()
            .is_some_and(AlAckBlock::requests_repeat_entire_tl_sdu)
    }

    pub fn is_selective_segment_ack(&self) -> bool {
        self.acknowledgement_blocks
            .first()
            .is_some_and(AlAckBlock::is_selective_segment_ack)
    }

    pub fn segment_acknowledged_in_first_block(&self, ss: u8) -> Option<bool> {
        self.acknowledgement_blocks.first()?.segment_acknowledged(ss)
    }
}

impl fmt::Display for AlAck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = if self.receiver_ready { "al_ack" } else { "al_rnr" };
        write!(f, "{} {{ blocks: {:?} }}", name, self.acknowledgement_blocks)
    }
}
