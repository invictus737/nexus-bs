// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::thread;
use std::time::{Duration, Instant};

use crate::net_telemetry::{TelemetryEvent, TelemetrySink};

use super::actions::{HealthActionKind, HealthActionRequest, HealthActionSink, HealthActionSource};
use super::registry;
use super::types::{HealthDomain, HealthThresholds};

#[derive(Clone)]
pub struct HealthMonitorConfig {
    pub snapshot_interval: Duration,
    pub thresholds: HealthThresholds,
    pub restart_on_core_stall: bool,
    pub restart_after_critical: Duration,
    pub restart_cooldown: Duration,
    pub action_sink: Option<HealthActionSink>,
}

impl HealthMonitorConfig {
    pub fn observe_only(snapshot_interval: Duration) -> Self {
        Self {
            snapshot_interval,
            thresholds: HealthThresholds::default(),
            restart_on_core_stall: false,
            restart_after_critical: Duration::from_secs(30),
            restart_cooldown: Duration::from_secs(600),
            action_sink: None,
        }
    }
}

/// Spawn the health sampler.
///
/// The sampler never calls RF/CMCE/UMAC methods directly. It reads atomics and
/// emits through bounded queues, so dashboard/telemetry/remediation cannot
/// block TETRA core functions.
pub fn spawn_health_monitor(sink: TelemetrySink, config: HealthMonitorConfig) {
    let snapshot_interval = config.snapshot_interval.max(Duration::from_secs(1));
    let thresholds = config.thresholds;
    let restart_on_core_stall = config.restart_on_core_stall;
    let restart_after_critical = config.restart_after_critical.max(Duration::from_secs(1));
    let restart_cooldown = config.restart_cooldown.max(Duration::from_secs(1));
    let action_sink = config.action_sink;
    thread::Builder::new()
        .name("health-monitor".into())
        .spawn(move || {
            let mut service_critical_since: Option<Instant> = None;
            let mut last_restart_request: Option<Instant> = None;
            loop {
                thread::sleep(snapshot_interval);
                let snapshot = registry().snapshot_with_thresholds(&thresholds);
                sink.send(TelemetryEvent::HealthSnapshot(snapshot));

                if !restart_on_core_stall {
                    continue;
                }

                let now = Instant::now();
                let tick_age_ms = registry().service_tick_age_ms();
                if tick_age_ms < thresholds.service_critical_tick_age_ms.max(1_000) {
                    service_critical_since = None;
                    continue;
                }

                let critical_since = *service_critical_since.get_or_insert(now);
                if now.duration_since(critical_since) < restart_after_critical {
                    continue;
                }
                if last_restart_request.is_some_and(|last| now.duration_since(last) < restart_cooldown) {
                    continue;
                }

                let Some(action_sink) = &action_sink else {
                    continue;
                };
                let reason = format!("core loop stale for {} ms", tick_age_ms);
                if action_sink.try_send(HealthActionRequest {
                    domain: HealthDomain::Service,
                    kind: HealthActionKind::RestartService,
                    reason,
                }) {
                    last_restart_request = Some(now);
                }
            }
        })
        .expect("failed to spawn health-monitor thread");
}

pub fn spawn_health_action_worker(source: HealthActionSource) {
    thread::Builder::new()
        .name("health-actions".into())
        .spawn(move || {
            while let Some(request) = source.recv() {
                match request.kind {
                    HealthActionKind::RestartService => {
                        registry().record_action(request.domain, "restart_service", &request.reason);
                        crate::service_control::schedule_service_action(crate::service_control::ServiceAction::Restart, Duration::ZERO);
                    }
                }
            }
        })
        .expect("failed to spawn health-actions thread");
}
