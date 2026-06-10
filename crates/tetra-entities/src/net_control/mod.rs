//! External networked command component
//!
//! Runs outside the real-time core in its own thread. Receives [`Command`]s
//! from a remote server via a pluggable network transport, forwards them
//! toward the stack through a [`CommandSink`], and sends [`CommandResponse`]s
//! back to the server.

pub mod channel;
pub mod codec;
pub mod commands;
pub mod worker;

use std::time::Duration;

pub use self::channel::{CommandDispatcher, ControlEndpoint, make_control_link};
pub use self::commands::{ControlCommand, ControlResponse};
pub use self::worker::ControlWorker;

/// Sent as subprotocol in WebSocket handshake
pub const CONTROL_PROTOCOL_VERSION: &str = tetra_core::CONTROL_PROTOCOL_VERSION;
pub const CONTROL_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
pub const CONTROL_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);

pub fn select_control_subprotocol(header_value: &str) -> Option<&'static str> {
    for protocol in header_value.split(',').map(str::trim) {
        if protocol == CONTROL_PROTOCOL_VERSION {
            return Some(CONTROL_PROTOCOL_VERSION);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_protocol_tracks_nexus_bs_product_version() {
        assert_eq!(CONTROL_PROTOCOL_VERSION, "nexus-bs-control-v0.1.64");
    }

    #[test]
    fn select_control_subprotocol_prefers_current_protocol() {
        assert_eq!(
            select_control_subprotocol("nexus-bs-control-v0.1.64"),
            Some(CONTROL_PROTOCOL_VERSION)
        );
    }

    #[test]
    fn select_control_subprotocol_rejects_legacy_protocol() {
        assert_eq!(select_control_subprotocol("legacy-control-v1"), None);
    }
}
