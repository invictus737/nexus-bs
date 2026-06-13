// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

//! Core utilities for Nexus-BS
//!
//! This crate provides fundamental types and utilities used across the TETRA stack

/// Public product name shown by tools, dashboard and network clients.
pub const PRODUCT_NAME: &str = "Nexus-BS";
/// Workspace package version shared by the Nexus-BS crates.
pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Release-style version tag without the git suffix.
pub const PRODUCT_VERSION_TAG: &str = const_format::formatcp!("v{}", PRODUCT_VERSION);
/// HTTP/WebSocket User-Agent for external services and Nexus-BS clients.
pub const PRODUCT_USER_AGENT: &str = const_format::formatcp!("{}/{}", PRODUCT_NAME, PRODUCT_VERSION_TAG);
/// WebSocket subprotocol used by Nexus-BS control clients and servers.
pub const CONTROL_PROTOCOL_VERSION: &str = const_format::formatcp!("nexus-bs-control-{}", PRODUCT_VERSION_TAG);
/// WebSocket subprotocol used by Nexus-BS telemetry clients and servers.
pub const TELEMETRY_PROTOCOL_VERSION: &str = const_format::formatcp!("nexus-bs-telemetry-{}", PRODUCT_VERSION_TAG);

/// Short git commit hash, set at compile time (e.g. "g2aad62c")
pub const GIT_HASH: &str = git_version::git_version!(
    args = ["--always", "--dirty=-modified", "--match=", "--abbrev=8"],
    fallback = "unknown"
);
/// Full stack version string, e.g. "v0.1.66-g2aad62c"
pub const STACK_VERSION: &str = const_format::formatcp!("{}-{}", PRODUCT_VERSION_TAG, GIT_HASH);

pub mod address;
pub mod bitbuffer;
pub mod debug;
pub mod direction;
pub mod freqs;
pub mod pdu_parse_error;
pub mod phy_types;
pub mod ranges;
pub mod sap_fields;
pub mod tdma_time;
pub mod tetra_common;
pub mod tetra_entities;
pub mod timeslot_alloc;
pub mod tx_receipt;
pub mod typed_pdu_fields;

// Re-export commonly used items
pub use address::*;
pub use bitbuffer::BitBuffer;
pub use direction::Direction;
pub use pdu_parse_error::PduParseErr;
pub use phy_types::*;
pub use sap_fields::*;
pub use tdma_time::TdmaTime;
pub use tetra_common::*;
pub use timeslot_alloc::*;
pub use tx_receipt::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_identity_tracks_workspace_version() {
        assert_eq!(PRODUCT_NAME, "Nexus-BS");
        assert_eq!(PRODUCT_VERSION, "0.1.66");
        assert_eq!(PRODUCT_VERSION_TAG, "v0.1.66");
        assert_eq!(PRODUCT_USER_AGENT, "Nexus-BS/v0.1.66");
        assert_eq!(CONTROL_PROTOCOL_VERSION, "nexus-bs-control-v0.1.66");
        assert_eq!(TELEMETRY_PROTOCOL_VERSION, "nexus-bs-telemetry-v0.1.66");
    }
}
