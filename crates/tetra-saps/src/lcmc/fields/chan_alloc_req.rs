// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use tetra_core::Todo;

use crate::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};

#[derive(Debug, Clone)]
pub struct CmceChanAllocReq {
    /// Set for new allocation, None for QuitAndGo
    pub usage: Option<u8>,
    /// Carrier frequency; by default, uses self
    pub carrier: Option<Todo>,
    /// Bitmap of slots to use.
    pub timeslots: [bool; 4],
    /// Alloc type.
    /// Additional: new allocation.
    /// Replace: update existing allocation, or create if it does not exist.
    /// QuitAndGo: remove existing allocation.
    pub alloc_type: ChanAllocType,
    pub ul_dl_assigned: UlDlAssignment,
}
