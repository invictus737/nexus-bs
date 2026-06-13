// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use tetra_core::unimplemented_log;
use tetra_saps::SapMsg;

use crate::MessageQueue;

/// Clause 12 Supplementary Services CMCE sub-entity
pub struct SsMsSubentity {}

impl SsMsSubentity {
    pub fn new() -> Self {
        SsMsSubentity {}
    }

    pub fn route_re_deliver(&mut self, _queue: &mut MessageQueue, mut _message: SapMsg) {
        tracing::trace!("route_re_deliver");
        // Supplementary Services not implemented yet — log instead of panicking.
        unimplemented_log!("SsMsSubentity::route_re_deliver: Supplementary Services not implemented");
    }
}
