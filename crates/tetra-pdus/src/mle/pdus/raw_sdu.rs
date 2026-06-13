// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

pub(super) fn read_remaining_u64(buffer: &mut BitBuffer, field: &'static str) -> Result<Option<u64>, PduParseErr> {
    let len = buffer.get_len_remaining();
    if len == 0 {
        return Ok(None);
    }
    if len > 64 {
        return Err(PduParseErr::NotImplemented { field: Some(field) });
    }
    buffer.read_field(len, field).map(Some)
}

pub(super) fn reject_write_if_present(value: Option<u64>, field: &'static str) -> Result<(), PduParseErr> {
    if value.is_some() {
        return Err(PduParseErr::NotImplemented { field: Some(field) });
    }
    Ok(())
}
