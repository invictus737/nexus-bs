// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};

use super::{HealthDomain, registry};

const HEALTH_ACTION_CHANNEL_CAPACITY: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthActionKind {
    RestartService,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthActionRequest {
    pub domain: HealthDomain,
    pub kind: HealthActionKind,
    pub reason: String,
}

#[derive(Clone)]
pub struct HealthActionSink {
    tx: Sender<HealthActionRequest>,
}

pub struct HealthActionSource {
    rx: Receiver<HealthActionRequest>,
}

impl HealthActionSink {
    pub fn try_send(&self, request: HealthActionRequest) -> bool {
        let queue_len = self.tx.len();
        match self.tx.try_send(request) {
            Ok(()) => {
                registry().set_health_action_backlog(self.tx.len());
                true
            }
            Err(TrySendError::Full(_)) => {
                registry().set_health_action_backlog(queue_len);
                registry().incr_health_action_drop();
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                registry().incr_health_action_drop();
                false
            }
        }
    }
}

impl HealthActionSource {
    pub fn recv(&self) -> Option<HealthActionRequest> {
        let request = self.rx.recv().ok()?;
        registry().set_health_action_backlog(self.rx.len());
        Some(request)
    }
}

pub fn health_action_channel() -> (HealthActionSink, HealthActionSource) {
    let (tx, rx) = bounded(HEALTH_ACTION_CHANNEL_CAPACITY);
    (HealthActionSink { tx }, HealthActionSource { rx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_action_channel_is_bounded_and_non_blocking() {
        let (sink, source) = health_action_channel();
        let request = HealthActionRequest {
            domain: HealthDomain::Service,
            kind: HealthActionKind::RestartService,
            reason: "test".to_string(),
        };

        let mut accepted = 0usize;
        for _ in 0..(HEALTH_ACTION_CHANNEL_CAPACITY + 4) {
            if sink.try_send(request.clone()) {
                accepted += 1;
            }
        }

        assert_eq!(accepted, HEALTH_ACTION_CHANNEL_CAPACITY);
        for _ in 0..accepted {
            assert_eq!(source.recv(), Some(request.clone()));
        }
    }
}
