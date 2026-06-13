// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};

// Entities as used in the standard
#[derive(PartialEq, Eq, Hash, Clone, Debug, Copy, Encode, Decode, Serialize, Deserialize)]
pub enum TetraEntity {
    /// Physical layer
    Phy,
    /// Lower MAC layer
    Lmac,
    /// Upper MAC layer
    Umac,
    /// Logical link control
    Llc,
    /// Mobile Link Entity
    Mle,
    /// Mobility Management
    Mm,
    /// Circuit Mode Control Entity
    Cmce,
    /// SubNetwork Dependent Convergence Protocol
    Sndcp,

    /// Any U-plane entity. SAP determines routing
    User,

    /// Brew protocol bridge (TetraPack/BrandMeister integration)
    Brew,
}
