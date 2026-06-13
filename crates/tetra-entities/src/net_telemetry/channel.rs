// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError, bounded};
use std::time::Duration;

use crate::net_telemetry::events::TelemetryEvent;

pub const TELEMETRY_CHANNEL_CAPACITY: usize = 8192;

// ---------------------------------------------------------------------------
// TelemetrySink  (cloneable, push‑only handle given to entities)
//
// crossbeam Sender is Arc‑backed; cloning is a single atomic increment.
// send() is lock‑free — it claims a slot via atomic FAA and memcpys the
// TelemetryEvent into it.  Small events require zero heap allocation.
// Larger events should use a Box to keep the TelemetryEvent size small
// and avoid heap allocation on send.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct TelemetrySink {
    tx: Sender<TelemetryEvent>,
}

impl TelemetrySink {
    /// Push a telemetry event. Fire-and-forget: silently drops if the receiver is gone
    /// or the bounded queue is full, so telemetry cannot block RF/core paths.
    #[inline]
    pub fn send(&self, event: TelemetryEvent) {
        let queue_len = self.tx.len();
        match self.tx.try_send(event) {
            Ok(()) => crate::health::registry().mark_telemetry_sent(self.tx.len()),
            Err(TrySendError::Full(_)) => crate::health::registry().mark_telemetry_dropped_full(queue_len),
            Err(TrySendError::Disconnected(_)) => crate::health::registry().mark_telemetry_dropped_disconnected(),
        }
    }
}

// ---------------------------------------------------------------------------
// TelemetrySource  (receive side, owned by the Telemetry component)
// ---------------------------------------------------------------------------

pub struct TelemetrySource {
    rx: Receiver<TelemetryEvent>,
}

/// Result of a receive-with-timeout operation.
pub enum RecvEvent {
    /// A telemetry event was received.
    Event(TelemetryEvent),
    /// Timed out waiting — channel is still open.
    Timeout,
    /// All sinks were dropped — channel is closed.
    Closed,
}

impl TelemetrySource {
    /// Blocking receive.  Returns `None` when all sinks have been dropped.
    pub fn recv(&self) -> Option<TelemetryEvent> {
        self.rx.recv().ok()
    }

    /// Blocking receive with timeout, distinguishing timeout from channel close.
    pub fn recv_timeout(&self, timeout: Duration) -> RecvEvent {
        match self.rx.recv_timeout(timeout) {
            Ok(event) => RecvEvent::Event(event),
            Err(RecvTimeoutError::Timeout) => RecvEvent::Timeout,
            Err(RecvTimeoutError::Disconnected) => RecvEvent::Closed,
        }
    }

    /// Non-blocking try_recv.
    pub fn try_recv(&self) -> Option<TelemetryEvent> {
        self.rx.try_recv().ok()
    }
}

// ---------------------------------------------------------------------------
// Channel constructor
// ---------------------------------------------------------------------------

/// Create a linked (sink, source) pair.
pub fn telemetry_channel() -> (TelemetrySink, TelemetrySource) {
    let (tx, rx) = bounded(TELEMETRY_CHANNEL_CAPACITY);
    (TelemetrySink { tx }, TelemetrySource { rx })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_two_events() {
        let (sink, source) = telemetry_channel();

        sink.send(TelemetryEvent::MsRegistration { issi: 12345 });

        // Clone the sink (simulating a second entity) and send an Attach event
        let sink2 = sink.clone();
        sink2.send(TelemetryEvent::MsGroupAttach {
            issi: 12345,
            gssis: vec![1, 2, 3],
        });

        // Receive and verify
        let a = source.try_recv().expect("should receive Registration");
        assert!(matches!(a, TelemetryEvent::MsRegistration { issi: 12345 }));

        let b = source.try_recv().expect("should receive Attach");
        if let TelemetryEvent::MsGroupAttach { issi, gssis } = &b {
            assert_eq!(*issi, 12345);
            assert_eq!(*gssis, vec![1, 2, 3]);
        } else {
            panic!("expected Attach variant");
        }

        // No more items
        assert!(source.try_recv().is_none());
    }

    #[test]
    fn telemetry_channel_is_bounded_and_non_blocking_on_overflow() {
        let (sink, source) = telemetry_channel();

        for idx in 0..(TELEMETRY_CHANNEL_CAPACITY + 16) {
            sink.send(TelemetryEvent::MsRegistration { issi: idx as u32 });
        }

        let mut received = 0usize;
        while source.try_recv().is_some() {
            received += 1;
        }
        assert_eq!(received, TELEMETRY_CHANNEL_CAPACITY);
    }

    #[test]
    fn telemetry_health_snapshots_are_bounded_and_non_blocking_on_overflow() {
        let (sink, source) = telemetry_channel();
        let snapshot = crate::health::registry().snapshot();

        for _ in 0..(TELEMETRY_CHANNEL_CAPACITY + 16) {
            sink.send(TelemetryEvent::HealthSnapshot(snapshot.clone()));
        }

        let mut received = 0usize;
        while let Some(event) = source.try_recv() {
            assert!(matches!(event, TelemetryEvent::HealthSnapshot(_)));
            received += 1;
        }
        assert_eq!(received, TELEMETRY_CHANNEL_CAPACITY);
    }
}
