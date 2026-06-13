// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use crate::MessageQueue;
use tetra_core::unimplemented_log;
use tetra_saps::SapMsg;

/// Clause 12 Supplementary Services CMCE sub-entity
pub struct SsBsSubentity {}

impl SsBsSubentity {
    pub fn new() -> Self {
        SsBsSubentity {}
    }

    pub fn route_re_deliver(&mut self, _queue: &mut MessageQueue, mut _message: SapMsg) {
        tracing::trace!("route_re_deliver");
        // Supplementary Services (call hold, transfer, etc.) not implemented yet.
        // Log and ignore instead of panicking when an MS sends an SS PDU.
        unimplemented_log!("SsBsSubentity::route_re_deliver: Supplementary Services not implemented");
    }
}
