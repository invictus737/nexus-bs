// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

//! Re-export PHY types from tetra-core for backward compatibility
//!
//! These types are defined in tetra-core because they're used across multiple
//! layers (PHY, LMAC, UMAC) and in SAP primitives.

pub use tetra_core::{BurstType, PhyBlockNum, PhyBlockType, TrainingSequence};
