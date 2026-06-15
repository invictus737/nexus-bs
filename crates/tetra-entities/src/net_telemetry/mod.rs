// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

//! External networked telemetry component
//!
//! Runs outside the real-time core in its own thread. Receives [`TelemetryEvent`]s
//! from the core via a [`TelemetrySource`] and forwards them over a pluggable
//! network transport.

pub mod channel;
pub mod codec;
pub mod events;
pub mod worker;

use std::time::Duration;

pub use self::channel::{TelemetrySink, TelemetrySource, telemetry_channel};
pub use self::events::TelemetryEvent;
pub use self::worker::TelemetryWorker;

/// Sent as subprotocol in WebSocket handshake
pub const TELEMETRY_PROTOCOL_VERSION: &str = tetra_core::TELEMETRY_PROTOCOL_VERSION;
pub const TELEMETRY_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
pub const TELEMETRY_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);

pub fn select_telemetry_subprotocol(header_value: &str) -> Option<&'static str> {
    for protocol in header_value.split(',').map(str::trim) {
        if protocol == TELEMETRY_PROTOCOL_VERSION {
            return Some(TELEMETRY_PROTOCOL_VERSION);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_protocol_tracks_nexus_bs_product_version() {
        assert_eq!(TELEMETRY_PROTOCOL_VERSION, "nexus-bs-telemetry-v0.1.66_dev");
    }

    #[test]
    fn select_telemetry_subprotocol_prefers_current_protocol() {
        assert_eq!(
            select_telemetry_subprotocol("nexus-bs-telemetry-v0.1.66_dev"),
            Some(TELEMETRY_PROTOCOL_VERSION)
        );
    }

    #[test]
    fn select_telemetry_subprotocol_rejects_legacy_protocol() {
        assert_eq!(select_telemetry_subprotocol("legacy-telemetry-v1"), None);
    }
}
