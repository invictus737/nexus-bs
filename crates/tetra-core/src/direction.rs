// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum Direction {
    None,
    /// Uplink
    Ul,
    /// Downlink
    Dl,
    Both,
}

impl Direction {
    #[inline]
    pub fn includes_ul(&self) -> bool {
        matches!(self, Direction::Ul | Direction::Both)
    }

    #[inline]
    pub fn includes_dl(&self) -> bool {
        matches!(self, Direction::Dl | Direction::Both)
    }
}
