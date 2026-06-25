// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::types::{
    CmceHealthStats, HealthActionRecord, HealthDomain, HealthDomainSnapshot, HealthMetric, HealthSeverity, HealthSnapshot,
    HealthThresholds, SdsHealthStats, UmacHealthStats,
};

const SERVICE_DEGRADED_TICK_AGE_MS: u64 = 2_000;
const SERVICE_QUEUE_DEGRADED: u64 = 512;
const SERVICE_QUEUE_CRITICAL: u64 = 2_048;
const SERVICE_LOOP_DEGRADED_US: u64 = 100_000;
const SERVICE_LOOP_CRITICAL_US: u64 = 500_000;

const BREW_PENDING_DEGRADED: u64 = 1;
const BREW_PENDING_CRITICAL: u64 = 64;
const BREW_QUEUE_DEGRADED: u64 = 512;
const BREW_QUEUE_CRITICAL: u64 = 4_096;
const TELEMETRY_QUEUE_DEGRADED: u64 = 4_096;
const TELEMETRY_QUEUE_CRITICAL: u64 = 7_680;
const CMCE_PENDING_DEGRADED: u64 = 16;
const CMCE_PENDING_CRITICAL: u64 = 128;
const CMCE_FLOOR_WAITERS_DEGRADED: u64 = 32;
const CMCE_FLOOR_WAITERS_CRITICAL: u64 = 512;
const SDS_LIVE_QUEUE_DEGRADED: u64 = 64;
const SDS_LIVE_QUEUE_CRITICAL: u64 = 256;
const SDS_TL_CONTEXT_DEGRADED: u64 = 240;
const UMAC_TMA_DEGRADED: u64 = 2_048;
const UMAC_TMA_CRITICAL: u64 = 4_096;
const UMAC_QUEUE_DEGRADED: u64 = 512;
const UMAC_QUEUE_CRITICAL: u64 = 2_048;
const RF_RXTEX_DEGRADED_US: u64 = 100_000;
const RF_RXTEX_CRITICAL_US: u64 = 500_000;
const RF_ANOMALY_RECENT_MS: u64 = 10_000;
const RF_TX_DSP_EVM_DEGRADED_MILLI_PCT: u64 = 7_000;
const RF_TX_DSP_EVM_CRITICAL_MILLI_PCT: u64 = 10_000;
const MAX_RECENT_ACTIONS: usize = 16;

static HEALTH_REGISTRY: OnceLock<HealthRegistry> = OnceLock::new();

pub fn registry() -> &'static HealthRegistry {
    HEALTH_REGISTRY.get_or_init(HealthRegistry::new)
}

#[derive(Debug)]
pub struct HealthRegistry {
    router_tick_count: AtomicU64,
    router_last_tick_unix_ms: AtomicU64,
    router_last_loop_us: AtomicU64,
    router_last_queue_len: AtomicU64,
    router_max_queue_len: AtomicU64,

    brew_connected: AtomicBool,
    brew_server_version: AtomicU8,
    brew_command_queue_len: AtomicU64,
    brew_pending_critical_commands: AtomicU64,
    brew_noncritical_drops: AtomicU64,

    telemetry_sent: AtomicU64,
    telemetry_dropped_full: AtomicU64,
    telemetry_dropped_disconnected: AtomicU64,
    telemetry_queue_len: AtomicU64,
    telemetry_max_queue_len: AtomicU64,

    cmce_active_group_calls: AtomicU64,
    cmce_pending_group_releases: AtomicU64,
    cmce_pending_group_floor_activations: AtomicU64,
    cmce_pending_network_group_readies: AtomicU64,
    cmce_group_floor_waiters: AtomicU64,
    cmce_active_individual_calls: AtomicU64,
    cmce_pending_individual_calls: AtomicU64,
    cmce_pending_individual_releases: AtomicU64,
    cmce_pending_network_individual_connects: AtomicU64,
    cmce_pending_individual_disconnects: AtomicU64,
    cmce_individual_floor_waiters: AtomicU64,

    sds_live_queue_len: AtomicU64,
    sds_pending_actions: AtomicU64,
    sds_tl_report_contexts: AtomicU64,
    sds_tl_report_context_evictions: AtomicU64,

    umac_dl_queue_total: AtomicU64,
    umac_dl_queue_max_per_ts: AtomicU64,
    umac_next_slot_queue_len: AtomicU64,
    umac_pending_ra_ack_total: AtomicU64,
    umac_pending_ra_ack_max_per_ts: AtomicU64,
    umac_pending_tma_reports: AtomicU64,
    umac_pending_private_ul_media_total: AtomicU64,
    umac_pending_stch: AtomicBool,

    phy_last_rxtx_us: AtomicU64,
    phy_max_rxtx_us: AtomicU64,
    phy_tx_late_events: AtomicU64,
    phy_tx_late_blocks: AtomicU64,
    phy_rx_lost_events: AtomicU64,
    phy_rx_lost_samples: AtomicU64,
    phy_rx_processing_blocks: AtomicU64,
    phy_last_anomaly_unix_ms: AtomicU64,
    phy_tx_dsp_evm_milli_pct: AtomicU64,
    phy_tx_dsp_papr_deci_db: AtomicU64,
    phy_tx_carrier_leakage_deci_db_offset: AtomicU64,
    phy_tx_occupied_bandwidth_hz: AtomicU64,
    phy_tx_quality_gate: AtomicU8,

    health_action_queue_len: AtomicU64,
    health_action_drops: AtomicU64,
    recent_actions: Mutex<VecDeque<HealthActionRecord>>,
}

impl HealthRegistry {
    pub fn new() -> Self {
        Self {
            router_tick_count: AtomicU64::new(0),
            router_last_tick_unix_ms: AtomicU64::new(0),
            router_last_loop_us: AtomicU64::new(0),
            router_last_queue_len: AtomicU64::new(0),
            router_max_queue_len: AtomicU64::new(0),
            brew_connected: AtomicBool::new(false),
            brew_server_version: AtomicU8::new(0),
            brew_command_queue_len: AtomicU64::new(0),
            brew_pending_critical_commands: AtomicU64::new(0),
            brew_noncritical_drops: AtomicU64::new(0),
            telemetry_sent: AtomicU64::new(0),
            telemetry_dropped_full: AtomicU64::new(0),
            telemetry_dropped_disconnected: AtomicU64::new(0),
            telemetry_queue_len: AtomicU64::new(0),
            telemetry_max_queue_len: AtomicU64::new(0),
            cmce_active_group_calls: AtomicU64::new(0),
            cmce_pending_group_releases: AtomicU64::new(0),
            cmce_pending_group_floor_activations: AtomicU64::new(0),
            cmce_pending_network_group_readies: AtomicU64::new(0),
            cmce_group_floor_waiters: AtomicU64::new(0),
            cmce_active_individual_calls: AtomicU64::new(0),
            cmce_pending_individual_calls: AtomicU64::new(0),
            cmce_pending_individual_releases: AtomicU64::new(0),
            cmce_pending_network_individual_connects: AtomicU64::new(0),
            cmce_pending_individual_disconnects: AtomicU64::new(0),
            cmce_individual_floor_waiters: AtomicU64::new(0),
            sds_live_queue_len: AtomicU64::new(0),
            sds_pending_actions: AtomicU64::new(0),
            sds_tl_report_contexts: AtomicU64::new(0),
            sds_tl_report_context_evictions: AtomicU64::new(0),
            umac_dl_queue_total: AtomicU64::new(0),
            umac_dl_queue_max_per_ts: AtomicU64::new(0),
            umac_next_slot_queue_len: AtomicU64::new(0),
            umac_pending_ra_ack_total: AtomicU64::new(0),
            umac_pending_ra_ack_max_per_ts: AtomicU64::new(0),
            umac_pending_tma_reports: AtomicU64::new(0),
            umac_pending_private_ul_media_total: AtomicU64::new(0),
            umac_pending_stch: AtomicBool::new(false),
            phy_last_rxtx_us: AtomicU64::new(0),
            phy_max_rxtx_us: AtomicU64::new(0),
            phy_tx_late_events: AtomicU64::new(0),
            phy_tx_late_blocks: AtomicU64::new(0),
            phy_rx_lost_events: AtomicU64::new(0),
            phy_rx_lost_samples: AtomicU64::new(0),
            phy_rx_processing_blocks: AtomicU64::new(0),
            phy_last_anomaly_unix_ms: AtomicU64::new(0),
            phy_tx_dsp_evm_milli_pct: AtomicU64::new(0),
            phy_tx_dsp_papr_deci_db: AtomicU64::new(0),
            phy_tx_carrier_leakage_deci_db_offset: AtomicU64::new(0),
            phy_tx_occupied_bandwidth_hz: AtomicU64::new(0),
            phy_tx_quality_gate: AtomicU8::new(0),
            health_action_queue_len: AtomicU64::new(0),
            health_action_drops: AtomicU64::new(0),
            recent_actions: Mutex::new(VecDeque::new()),
        }
    }

    pub fn mark_router_tick(&self, queue_len: usize, loop_duration: Duration) {
        let queue_len = queue_len as u64;
        self.router_tick_count.fetch_add(1, Ordering::Relaxed);
        self.router_last_tick_unix_ms.store(unix_ms(), Ordering::Relaxed);
        self.router_last_loop_us
            .store(loop_duration.as_micros().min(u128::from(u64::MAX)) as u64, Ordering::Relaxed);
        self.router_last_queue_len.store(queue_len, Ordering::Relaxed);
        self.router_max_queue_len.fetch_max(queue_len, Ordering::Relaxed);
    }

    pub fn set_brew_status(&self, connected: bool, server_version: u8) {
        self.brew_connected.store(connected, Ordering::Relaxed);
        self.brew_server_version.store(server_version, Ordering::Relaxed);
    }

    pub fn set_brew_command_backlog(&self, command_queue_len: usize, pending_critical_commands: usize) {
        self.brew_command_queue_len.store(command_queue_len as u64, Ordering::Relaxed);
        self.brew_pending_critical_commands
            .store(pending_critical_commands as u64, Ordering::Relaxed);
    }

    pub fn incr_brew_noncritical_drop(&self) {
        self.brew_noncritical_drops.fetch_add(1, Ordering::Relaxed);
    }

    pub fn mark_telemetry_sent(&self, queue_len: usize) {
        let queue_len = queue_len as u64;
        self.telemetry_sent.fetch_add(1, Ordering::Relaxed);
        self.telemetry_queue_len.store(queue_len, Ordering::Relaxed);
        self.telemetry_max_queue_len.fetch_max(queue_len, Ordering::Relaxed);
    }

    pub fn mark_telemetry_dropped_full(&self, queue_len: usize) {
        let queue_len = queue_len as u64;
        self.telemetry_dropped_full.fetch_add(1, Ordering::Relaxed);
        self.telemetry_queue_len.store(queue_len, Ordering::Relaxed);
        self.telemetry_max_queue_len.fetch_max(queue_len, Ordering::Relaxed);
    }

    pub fn mark_telemetry_dropped_disconnected(&self) {
        self.telemetry_dropped_disconnected.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_cmce_stats(&self, stats: CmceHealthStats) {
        self.cmce_active_group_calls
            .store(stats.active_group_calls as u64, Ordering::Relaxed);
        self.cmce_pending_group_releases
            .store(stats.pending_group_releases as u64, Ordering::Relaxed);
        self.cmce_pending_group_floor_activations
            .store(stats.pending_group_floor_activations as u64, Ordering::Relaxed);
        self.cmce_pending_network_group_readies
            .store(stats.pending_network_group_readies as u64, Ordering::Relaxed);
        self.cmce_group_floor_waiters
            .store(stats.group_floor_waiters as u64, Ordering::Relaxed);
        self.cmce_active_individual_calls
            .store(stats.active_individual_calls as u64, Ordering::Relaxed);
        self.cmce_pending_individual_calls
            .store(stats.pending_individual_calls as u64, Ordering::Relaxed);
        self.cmce_pending_individual_releases
            .store(stats.pending_individual_releases as u64, Ordering::Relaxed);
        self.cmce_pending_network_individual_connects
            .store(stats.pending_network_individual_connects as u64, Ordering::Relaxed);
        self.cmce_pending_individual_disconnects
            .store(stats.pending_individual_disconnects as u64, Ordering::Relaxed);
        self.cmce_individual_floor_waiters
            .store(stats.individual_floor_waiters as u64, Ordering::Relaxed);
    }

    pub fn set_sds_stats(&self, stats: SdsHealthStats) {
        self.sds_live_queue_len.store(stats.live_queue_len as u64, Ordering::Relaxed);
        self.sds_pending_actions.store(stats.pending_actions as u64, Ordering::Relaxed);
        self.sds_tl_report_contexts
            .store(stats.tl_report_contexts as u64, Ordering::Relaxed);
        self.sds_tl_report_context_evictions
            .store(stats.tl_report_context_evictions, Ordering::Relaxed);
    }

    pub fn incr_sds_tl_report_context_eviction(&self) {
        self.sds_tl_report_context_evictions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn sds_tl_report_context_evictions(&self) -> u64 {
        self.sds_tl_report_context_evictions.load(Ordering::Relaxed)
    }

    pub fn set_umac_stats(&self, stats: UmacHealthStats) {
        self.umac_dl_queue_total.store(stats.dl_queue_total as u64, Ordering::Relaxed);
        self.umac_dl_queue_max_per_ts
            .store(stats.dl_queue_max_per_ts as u64, Ordering::Relaxed);
        self.umac_next_slot_queue_len
            .store(stats.next_slot_queue_len as u64, Ordering::Relaxed);
        self.umac_pending_ra_ack_total
            .store(stats.pending_ra_ack_total as u64, Ordering::Relaxed);
        self.umac_pending_ra_ack_max_per_ts
            .store(stats.pending_ra_ack_max_per_ts as u64, Ordering::Relaxed);
        self.umac_pending_tma_reports
            .store(stats.pending_tma_reports as u64, Ordering::Relaxed);
        self.umac_pending_private_ul_media_total
            .store(stats.pending_private_ul_media_total as u64, Ordering::Relaxed);
        self.umac_pending_stch.store(stats.pending_stch, Ordering::Relaxed);
    }

    pub fn mark_phy_rxtx_duration(&self, duration: Duration) {
        let duration_us = duration.as_micros().min(u128::from(u64::MAX)) as u64;
        self.phy_last_rxtx_us.store(duration_us, Ordering::Relaxed);
        self.phy_max_rxtx_us.fetch_max(duration_us, Ordering::Relaxed);
    }

    pub fn incr_phy_tx_late(&self, skipped_blocks: u64) {
        self.phy_tx_late_events.fetch_add(1, Ordering::Relaxed);
        self.phy_tx_late_blocks.fetch_add(skipped_blocks, Ordering::Relaxed);
        self.phy_last_anomaly_unix_ms.store(unix_ms(), Ordering::Relaxed);
    }

    pub fn incr_phy_rx_lost(&self, samples_lost: u64, processing_blocks: u64) {
        self.phy_rx_lost_events.fetch_add(1, Ordering::Relaxed);
        self.phy_rx_lost_samples.fetch_add(samples_lost, Ordering::Relaxed);
        self.phy_rx_processing_blocks.fetch_add(processing_blocks, Ordering::Relaxed);
        self.phy_last_anomaly_unix_ms.store(unix_ms(), Ordering::Relaxed);
    }

    pub fn mark_phy_tx_quality(&self, evm_pct: f32, papr_db: f32, carrier_leakage_db: f32, occupied_bandwidth_hz: f32) {
        if evm_pct.is_finite() && evm_pct > 0.0 {
            let evm_milli_pct = (evm_pct * 1000.0).round().max(0.0) as u64;
            self.phy_tx_dsp_evm_milli_pct.store(evm_milli_pct, Ordering::Relaxed);
            let gate = if evm_milli_pct >= RF_TX_DSP_EVM_CRITICAL_MILLI_PCT {
                3
            } else if evm_milli_pct >= RF_TX_DSP_EVM_DEGRADED_MILLI_PCT {
                2
            } else {
                1
            };
            self.phy_tx_quality_gate.store(gate, Ordering::Relaxed);
        }
        if papr_db.is_finite() {
            self.phy_tx_dsp_papr_deci_db
                .store((papr_db * 10.0).round().max(0.0) as u64, Ordering::Relaxed);
        }
        if carrier_leakage_db.is_finite() {
            // Keep a signed dB-like value in an unsigned atomic with +200 dB offset.
            self.phy_tx_carrier_leakage_deci_db_offset
                .store(((carrier_leakage_db + 200.0) * 10.0).round().max(0.0) as u64, Ordering::Relaxed);
        }
        if occupied_bandwidth_hz.is_finite() {
            self.phy_tx_occupied_bandwidth_hz
                .store(occupied_bandwidth_hz.round().max(0.0) as u64, Ordering::Relaxed);
        }
    }

    pub fn set_health_action_backlog(&self, queue_len: usize) {
        self.health_action_queue_len.store(queue_len as u64, Ordering::Relaxed);
    }

    pub fn incr_health_action_drop(&self) {
        self.health_action_drops.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_action(&self, domain: HealthDomain, action: &str, reason: &str) {
        let record = HealthActionRecord {
            domain,
            action: action.to_string(),
            reason: reason.to_string(),
            unix_ms: unix_ms(),
        };
        if let Ok(mut actions) = self.recent_actions.lock() {
            actions.push_front(record);
            while actions.len() > MAX_RECENT_ACTIONS {
                actions.pop_back();
            }
        }
    }

    pub fn service_tick_age_ms(&self) -> u64 {
        self.service_tick_age_ms_at(unix_ms())
    }

    pub fn snapshot(&self) -> HealthSnapshot {
        self.snapshot_with_thresholds(&HealthThresholds::default())
    }

    pub fn snapshot_with_thresholds(&self, thresholds: &HealthThresholds) -> HealthSnapshot {
        let now = unix_ms();
        let service = self.service_snapshot(now, thresholds);
        let telemetry = self.telemetry_snapshot();
        let brew = self.brew_snapshot();
        let voice = self.voice_snapshot();
        let sds = self.sds_snapshot();
        let p2p = self.p2p_snapshot();
        let congestion = self.congestion_snapshot();
        let rf = self.rf_snapshot(now);
        let overall = [
            service.severity,
            telemetry.severity,
            brew.severity,
            voice.severity,
            sds.severity,
            p2p.severity,
            congestion.severity,
            rf.severity,
        ]
        .into_iter()
        .fold(HealthSeverity::Ok, HealthSeverity::max);
        let recent_actions = self
            .recent_actions
            .lock()
            .map(|actions| actions.iter().cloned().collect())
            .unwrap_or_default();

        HealthSnapshot {
            unix_ms: now,
            overall,
            domains: vec![service, telemetry, brew, voice, sds, p2p, congestion, rf],
            recent_actions,
        }
    }

    fn service_tick_age_ms_at(&self, now: u64) -> u64 {
        let last_tick_ms = self.router_last_tick_unix_ms.load(Ordering::Relaxed);
        if last_tick_ms == 0 {
            u64::MAX
        } else {
            now.saturating_sub(last_tick_ms)
        }
    }

    fn service_snapshot(&self, now: u64, thresholds: &HealthThresholds) -> HealthDomainSnapshot {
        let tick_count = self.router_tick_count.load(Ordering::Relaxed);
        let tick_age_ms = self.service_tick_age_ms_at(now);
        let loop_us = self.router_last_loop_us.load(Ordering::Relaxed);
        let queue_len = self.router_last_queue_len.load(Ordering::Relaxed);
        let max_queue_len = self.router_max_queue_len.load(Ordering::Relaxed);
        let critical_tick_age_ms = thresholds.service_critical_tick_age_ms.max(1_000);
        let degraded_tick_age_ms = SERVICE_DEGRADED_TICK_AGE_MS.min(critical_tick_age_ms.saturating_div(2).max(1_000));

        let mut severity = HealthSeverity::Ok;
        if tick_age_ms >= critical_tick_age_ms || queue_len >= SERVICE_QUEUE_CRITICAL || loop_us >= SERVICE_LOOP_CRITICAL_US {
            severity = HealthSeverity::Critical;
        } else if tick_age_ms >= degraded_tick_age_ms || queue_len >= SERVICE_QUEUE_DEGRADED || loop_us >= SERVICE_LOOP_DEGRADED_US {
            severity = HealthSeverity::Degraded;
        }

        let message = match severity {
            HealthSeverity::Ok => "RF stack loop healthy".to_string(),
            HealthSeverity::Degraded => "RF stack loop degraded".to_string(),
            HealthSeverity::Critical => "RF stack loop stalled or overloaded".to_string(),
        };

        HealthDomainSnapshot {
            domain: HealthDomain::Service,
            severity,
            message,
            metrics: vec![
                HealthMetric::new("tick_count", tick_count as i64, "count"),
                HealthMetric::new("tick_age_ms", tick_age_ms.min(i64::MAX as u64) as i64, "ms"),
                HealthMetric::new("last_loop_us", loop_us.min(i64::MAX as u64) as i64, "us"),
                HealthMetric::new("queue_len", queue_len.min(i64::MAX as u64) as i64, "messages"),
                HealthMetric::new("max_queue_len", max_queue_len.min(i64::MAX as u64) as i64, "messages"),
            ],
        }
    }

    fn brew_snapshot(&self) -> HealthDomainSnapshot {
        let connected = self.brew_connected.load(Ordering::Relaxed);
        let server_version = self.brew_server_version.load(Ordering::Relaxed);
        let queue_len = self.brew_command_queue_len.load(Ordering::Relaxed);
        let pending_critical = self.brew_pending_critical_commands.load(Ordering::Relaxed);
        let noncritical_drops = self.brew_noncritical_drops.load(Ordering::Relaxed);

        let mut severity = if connected { HealthSeverity::Ok } else { HealthSeverity::Degraded };
        if pending_critical >= BREW_PENDING_CRITICAL || queue_len >= BREW_QUEUE_CRITICAL {
            severity = HealthSeverity::Critical;
        } else if pending_critical >= BREW_PENDING_DEGRADED || queue_len >= BREW_QUEUE_DEGRADED {
            severity = severity.max(HealthSeverity::Degraded);
        }

        let message = match (connected, severity) {
            (true, HealthSeverity::Ok) => "Brew backhaul healthy".to_string(),
            (true, HealthSeverity::Degraded) => "Brew command path degraded".to_string(),
            (true, HealthSeverity::Critical) => "Brew command path congested".to_string(),
            (false, _) => "Brew backhaul disconnected".to_string(),
        };

        HealthDomainSnapshot {
            domain: HealthDomain::Brew,
            severity,
            message,
            metrics: vec![
                HealthMetric::new("connected", i64::from(connected), "bool"),
                HealthMetric::new("server_version", i64::from(server_version), "version"),
                HealthMetric::new("command_queue_len", queue_len.min(i64::MAX as u64) as i64, "commands"),
                HealthMetric::new(
                    "pending_critical_commands",
                    pending_critical.min(i64::MAX as u64) as i64,
                    "commands",
                ),
                HealthMetric::new("noncritical_drops", noncritical_drops.min(i64::MAX as u64) as i64, "count"),
            ],
        }
    }

    fn telemetry_snapshot(&self) -> HealthDomainSnapshot {
        let sent = self.telemetry_sent.load(Ordering::Relaxed);
        let dropped_full = self.telemetry_dropped_full.load(Ordering::Relaxed);
        let dropped_disconnected = self.telemetry_dropped_disconnected.load(Ordering::Relaxed);
        let queue_len = self.telemetry_queue_len.load(Ordering::Relaxed);
        let max_queue_len = self.telemetry_max_queue_len.load(Ordering::Relaxed);
        let action_queue_len = self.health_action_queue_len.load(Ordering::Relaxed);
        let action_drops = self.health_action_drops.load(Ordering::Relaxed);

        let severity = if queue_len >= TELEMETRY_QUEUE_CRITICAL {
            HealthSeverity::Critical
        } else if queue_len >= TELEMETRY_QUEUE_DEGRADED || dropped_full > 0 || dropped_disconnected > 0 || action_drops > 0 {
            HealthSeverity::Degraded
        } else {
            HealthSeverity::Ok
        };

        let message = match severity {
            HealthSeverity::Ok => "Telemetry path healthy".to_string(),
            HealthSeverity::Degraded => "Telemetry path dropping or backpressured".to_string(),
            HealthSeverity::Critical => "Telemetry path congested".to_string(),
        };

        HealthDomainSnapshot {
            domain: HealthDomain::Telemetry,
            severity,
            message,
            metrics: vec![
                HealthMetric::new("sent", sent.min(i64::MAX as u64) as i64, "events"),
                HealthMetric::new("dropped_full", dropped_full.min(i64::MAX as u64) as i64, "events"),
                HealthMetric::new("dropped_disconnected", dropped_disconnected.min(i64::MAX as u64) as i64, "events"),
                HealthMetric::new("queue_len", queue_len.min(i64::MAX as u64) as i64, "events"),
                HealthMetric::new("max_queue_len", max_queue_len.min(i64::MAX as u64) as i64, "events"),
                HealthMetric::new("action_queue_len", action_queue_len.min(i64::MAX as u64) as i64, "actions"),
                HealthMetric::new("action_drops", action_drops.min(i64::MAX as u64) as i64, "actions"),
            ],
        }
    }

    fn voice_snapshot(&self) -> HealthDomainSnapshot {
        let active_group = self.cmce_active_group_calls.load(Ordering::Relaxed);
        let pending_releases = self.cmce_pending_group_releases.load(Ordering::Relaxed);
        let pending_floor_activations = self.cmce_pending_group_floor_activations.load(Ordering::Relaxed);
        let pending_network_ready = self.cmce_pending_network_group_readies.load(Ordering::Relaxed);
        let floor_waiters = self.cmce_group_floor_waiters.load(Ordering::Relaxed);
        let pending_total = pending_releases + pending_floor_activations + pending_network_ready;

        let severity = if pending_total >= CMCE_PENDING_CRITICAL || floor_waiters >= CMCE_FLOOR_WAITERS_CRITICAL {
            HealthSeverity::Critical
        } else if pending_total >= CMCE_PENDING_DEGRADED || floor_waiters >= CMCE_FLOOR_WAITERS_DEGRADED {
            HealthSeverity::Degraded
        } else {
            HealthSeverity::Ok
        };

        let message = match severity {
            HealthSeverity::Ok => "Group-call control healthy".to_string(),
            HealthSeverity::Degraded => "Group-call control backlog detected".to_string(),
            HealthSeverity::Critical => "Group-call control congested".to_string(),
        };

        HealthDomainSnapshot {
            domain: HealthDomain::Voice,
            severity,
            message,
            metrics: vec![
                HealthMetric::new("active_group_calls", active_group.min(i64::MAX as u64) as i64, "calls"),
                HealthMetric::new("pending_group_releases", pending_releases.min(i64::MAX as u64) as i64, "calls"),
                HealthMetric::new(
                    "pending_group_floor_activations",
                    pending_floor_activations.min(i64::MAX as u64) as i64,
                    "calls",
                ),
                HealthMetric::new(
                    "pending_network_group_readies",
                    pending_network_ready.min(i64::MAX as u64) as i64,
                    "calls",
                ),
                HealthMetric::new("group_floor_waiters", floor_waiters.min(i64::MAX as u64) as i64, "requests"),
            ],
        }
    }

    fn p2p_snapshot(&self) -> HealthDomainSnapshot {
        let active = self.cmce_active_individual_calls.load(Ordering::Relaxed);
        let pending_calls = self.cmce_pending_individual_calls.load(Ordering::Relaxed);
        let pending_releases = self.cmce_pending_individual_releases.load(Ordering::Relaxed);
        let pending_network_connects = self.cmce_pending_network_individual_connects.load(Ordering::Relaxed);
        let pending_disconnects = self.cmce_pending_individual_disconnects.load(Ordering::Relaxed);
        let floor_waiters = self.cmce_individual_floor_waiters.load(Ordering::Relaxed);
        let pending_total = pending_calls + pending_releases + pending_network_connects + pending_disconnects;

        let severity = if pending_total >= CMCE_PENDING_CRITICAL || floor_waiters >= CMCE_FLOOR_WAITERS_CRITICAL {
            HealthSeverity::Critical
        } else if pending_total >= CMCE_PENDING_DEGRADED || floor_waiters >= CMCE_FLOOR_WAITERS_DEGRADED {
            HealthSeverity::Degraded
        } else {
            HealthSeverity::Ok
        };

        let message = match severity {
            HealthSeverity::Ok => "Private-call control healthy".to_string(),
            HealthSeverity::Degraded => "Private-call control backlog detected".to_string(),
            HealthSeverity::Critical => "Private-call control congested".to_string(),
        };

        HealthDomainSnapshot {
            domain: HealthDomain::P2p,
            severity,
            message,
            metrics: vec![
                HealthMetric::new("active_individual_calls", active.min(i64::MAX as u64) as i64, "calls"),
                HealthMetric::new("pending_individual_calls", pending_calls.min(i64::MAX as u64) as i64, "calls"),
                HealthMetric::new("pending_individual_releases", pending_releases.min(i64::MAX as u64) as i64, "calls"),
                HealthMetric::new(
                    "pending_network_individual_connects",
                    pending_network_connects.min(i64::MAX as u64) as i64,
                    "calls",
                ),
                HealthMetric::new(
                    "pending_individual_disconnects",
                    pending_disconnects.min(i64::MAX as u64) as i64,
                    "calls",
                ),
                HealthMetric::new("individual_floor_waiters", floor_waiters.min(i64::MAX as u64) as i64, "requests"),
            ],
        }
    }

    fn sds_snapshot(&self) -> HealthDomainSnapshot {
        let live_queue = self.sds_live_queue_len.load(Ordering::Relaxed);
        let pending_actions = self.sds_pending_actions.load(Ordering::Relaxed);
        let tl_contexts = self.sds_tl_report_contexts.load(Ordering::Relaxed);
        let tl_evictions = self.sds_tl_report_context_evictions.load(Ordering::Relaxed);

        let severity = if live_queue >= SDS_LIVE_QUEUE_CRITICAL || pending_actions >= CMCE_PENDING_CRITICAL {
            HealthSeverity::Critical
        } else if live_queue >= SDS_LIVE_QUEUE_DEGRADED
            || pending_actions >= CMCE_PENDING_DEGRADED
            || tl_contexts >= SDS_TL_CONTEXT_DEGRADED
        {
            HealthSeverity::Degraded
        } else {
            HealthSeverity::Ok
        };

        let message = match severity {
            HealthSeverity::Ok => "SDS path healthy".to_string(),
            HealthSeverity::Degraded => "SDS path backlog detected".to_string(),
            HealthSeverity::Critical => "SDS path congested".to_string(),
        };

        HealthDomainSnapshot {
            domain: HealthDomain::Sds,
            severity,
            message,
            metrics: vec![
                HealthMetric::new("live_queue_len", live_queue.min(i64::MAX as u64) as i64, "messages"),
                HealthMetric::new("pending_actions", pending_actions.min(i64::MAX as u64) as i64, "actions"),
                HealthMetric::new("tl_report_contexts", tl_contexts.min(i64::MAX as u64) as i64, "contexts"),
                HealthMetric::new("tl_report_context_evictions", tl_evictions.min(i64::MAX as u64) as i64, "contexts"),
            ],
        }
    }

    fn congestion_snapshot(&self) -> HealthDomainSnapshot {
        let dl_total = self.umac_dl_queue_total.load(Ordering::Relaxed);
        let dl_max = self.umac_dl_queue_max_per_ts.load(Ordering::Relaxed);
        let next_slot = self.umac_next_slot_queue_len.load(Ordering::Relaxed);
        let ra_total = self.umac_pending_ra_ack_total.load(Ordering::Relaxed);
        let ra_max = self.umac_pending_ra_ack_max_per_ts.load(Ordering::Relaxed);
        let tma = self.umac_pending_tma_reports.load(Ordering::Relaxed);
        let private_ul = self.umac_pending_private_ul_media_total.load(Ordering::Relaxed);
        let pending_stch = self.umac_pending_stch.load(Ordering::Relaxed);
        let worst_queue = dl_total.max(next_slot).max(ra_total);

        let severity = if tma >= UMAC_TMA_CRITICAL || worst_queue >= UMAC_QUEUE_CRITICAL {
            HealthSeverity::Critical
        } else if tma >= UMAC_TMA_DEGRADED || worst_queue >= UMAC_QUEUE_DEGRADED {
            HealthSeverity::Degraded
        } else {
            HealthSeverity::Ok
        };

        let message = match severity {
            HealthSeverity::Ok => "UMAC scheduler healthy".to_string(),
            HealthSeverity::Degraded => "UMAC scheduler backlog detected".to_string(),
            HealthSeverity::Critical => "UMAC scheduler congested".to_string(),
        };

        HealthDomainSnapshot {
            domain: HealthDomain::Congestion,
            severity,
            message,
            metrics: vec![
                HealthMetric::new("dl_queue_total", dl_total.min(i64::MAX as u64) as i64, "items"),
                HealthMetric::new("dl_queue_max_per_ts", dl_max.min(i64::MAX as u64) as i64, "items"),
                HealthMetric::new("next_slot_queue_len", next_slot.min(i64::MAX as u64) as i64, "items"),
                HealthMetric::new("pending_ra_ack_total", ra_total.min(i64::MAX as u64) as i64, "acks"),
                HealthMetric::new("pending_ra_ack_max_per_ts", ra_max.min(i64::MAX as u64) as i64, "acks"),
                HealthMetric::new("pending_tma_reports", tma.min(i64::MAX as u64) as i64, "reports"),
                HealthMetric::new("pending_private_ul_media", private_ul.min(i64::MAX as u64) as i64, "frames"),
                HealthMetric::new("pending_stch", i64::from(pending_stch), "bool"),
            ],
        }
    }

    fn rf_snapshot(&self, now: u64) -> HealthDomainSnapshot {
        let last_rxtx_us = self.phy_last_rxtx_us.load(Ordering::Relaxed);
        let max_rxtx_us = self.phy_max_rxtx_us.load(Ordering::Relaxed);
        let tx_late_events = self.phy_tx_late_events.load(Ordering::Relaxed);
        let tx_late_blocks = self.phy_tx_late_blocks.load(Ordering::Relaxed);
        let rx_lost_events = self.phy_rx_lost_events.load(Ordering::Relaxed);
        let rx_lost_samples = self.phy_rx_lost_samples.load(Ordering::Relaxed);
        let rx_processing_blocks = self.phy_rx_processing_blocks.load(Ordering::Relaxed);
        let tx_dsp_evm_milli_pct = self.phy_tx_dsp_evm_milli_pct.load(Ordering::Relaxed);
        let tx_dsp_papr_deci_db = self.phy_tx_dsp_papr_deci_db.load(Ordering::Relaxed);
        let tx_carrier_leakage_deci_db = self
            .phy_tx_carrier_leakage_deci_db_offset
            .load(Ordering::Relaxed)
            .saturating_sub(2_000) as i64;
        let tx_occupied_bandwidth_hz = self.phy_tx_occupied_bandwidth_hz.load(Ordering::Relaxed);
        let tx_quality_gate = self.phy_tx_quality_gate.load(Ordering::Relaxed);
        let last_anomaly = self.phy_last_anomaly_unix_ms.load(Ordering::Relaxed);
        let anomaly_age_ms = if last_anomaly == 0 {
            u64::MAX
        } else {
            now.saturating_sub(last_anomaly)
        };

        let severity = if last_rxtx_us >= RF_RXTEX_CRITICAL_US || tx_quality_gate >= 3 {
            HealthSeverity::Critical
        } else if last_rxtx_us >= RF_RXTEX_DEGRADED_US || anomaly_age_ms <= RF_ANOMALY_RECENT_MS || tx_quality_gate >= 2 {
            HealthSeverity::Degraded
        } else {
            HealthSeverity::Ok
        };

        let message = match severity {
            HealthSeverity::Ok => "RF timing healthy".to_string(),
            HealthSeverity::Degraded if tx_quality_gate >= 2 => "RF DSP EVM gate degraded".to_string(),
            HealthSeverity::Degraded => "RF timing anomaly detected".to_string(),
            HealthSeverity::Critical if tx_quality_gate >= 3 => "RF DSP EVM gate critical".to_string(),
            HealthSeverity::Critical => "RF timing stalled".to_string(),
        };

        HealthDomainSnapshot {
            domain: HealthDomain::Rf,
            severity,
            message,
            metrics: vec![
                HealthMetric::new("last_rxtx_us", last_rxtx_us.min(i64::MAX as u64) as i64, "us"),
                HealthMetric::new("max_rxtx_us", max_rxtx_us.min(i64::MAX as u64) as i64, "us"),
                HealthMetric::new("tx_late_events", tx_late_events.min(i64::MAX as u64) as i64, "events"),
                HealthMetric::new("tx_late_blocks", tx_late_blocks.min(i64::MAX as u64) as i64, "blocks"),
                HealthMetric::new("rx_lost_events", rx_lost_events.min(i64::MAX as u64) as i64, "events"),
                HealthMetric::new("rx_lost_samples", rx_lost_samples.min(i64::MAX as u64) as i64, "samples"),
                HealthMetric::new("rx_processing_blocks", rx_processing_blocks.min(i64::MAX as u64) as i64, "blocks"),
                HealthMetric::new(
                    "tx_dsp_evm_milli_pct",
                    tx_dsp_evm_milli_pct.min(i64::MAX as u64) as i64,
                    "milli_pct",
                ),
                HealthMetric::new("tx_dsp_papr_deci_db", tx_dsp_papr_deci_db.min(i64::MAX as u64) as i64, "deci_db"),
                HealthMetric::new("tx_carrier_leakage_deci_db", tx_carrier_leakage_deci_db, "deci_db"),
                HealthMetric::new(
                    "tx_occupied_bandwidth_hz",
                    tx_occupied_bandwidth_hz.min(i64::MAX as u64) as i64,
                    "hz",
                ),
                HealthMetric::new("last_anomaly_age_ms", anomaly_age_ms.min(i64::MAX as u64) as i64, "ms"),
            ],
        }
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_snapshot_reports_ok_after_recent_tick() {
        let registry = HealthRegistry::new();
        registry.mark_router_tick(2, Duration::from_millis(1));

        let snapshot = registry.snapshot();
        let service = snapshot
            .domains
            .iter()
            .find(|domain| domain.domain == HealthDomain::Service)
            .expect("service domain");

        assert_eq!(service.severity, HealthSeverity::Ok);
        assert!(service.metrics.iter().any(|metric| metric.name == "queue_len" && metric.value == 2));
    }

    #[test]
    fn service_snapshot_escalates_stale_core_ticks_by_age() {
        let registry = HealthRegistry::new();
        registry.router_tick_count.store(1, Ordering::Relaxed);
        registry.router_last_tick_unix_ms.store(1_000, Ordering::Relaxed);
        let thresholds = HealthThresholds {
            service_critical_tick_age_ms: 10_000,
        };

        let degraded = registry.service_snapshot(1_000 + SERVICE_DEGRADED_TICK_AGE_MS, &thresholds);
        assert_eq!(degraded.severity, HealthSeverity::Degraded);

        let critical = registry.service_snapshot(1_000 + thresholds.service_critical_tick_age_ms, &thresholds);
        assert_eq!(critical.severity, HealthSeverity::Critical);
    }

    #[test]
    fn brew_pending_critical_commands_escalate_health() {
        let registry = HealthRegistry::new();
        registry.set_brew_status(true, 1);
        registry.set_brew_command_backlog(12, BREW_PENDING_CRITICAL as usize);

        let snapshot = registry.snapshot();
        let brew = snapshot
            .domains
            .iter()
            .find(|domain| domain.domain == HealthDomain::Brew)
            .expect("brew domain");

        assert_eq!(brew.severity, HealthSeverity::Critical);
    }

    #[test]
    fn snapshot_includes_all_health_domains() {
        let registry = HealthRegistry::new();
        registry.mark_router_tick(0, Duration::from_millis(1));
        registry.set_brew_status(true, 1);

        let snapshot = registry.snapshot();
        for expected in [
            HealthDomain::Service,
            HealthDomain::Telemetry,
            HealthDomain::Brew,
            HealthDomain::Voice,
            HealthDomain::Sds,
            HealthDomain::P2p,
            HealthDomain::Congestion,
            HealthDomain::Rf,
        ] {
            assert!(
                snapshot.domains.iter().any(|domain| domain.domain == expected),
                "missing health domain {:?}",
                expected
            );
        }
    }

    #[test]
    fn rf_snapshot_escalates_on_tx_dsp_evm_gate() {
        let registry = HealthRegistry::new();
        registry.mark_phy_tx_quality(6.0, 3.5, -45.0, 18_000.0);
        let ok = registry.rf_snapshot(unix_ms());
        assert_eq!(ok.severity, HealthSeverity::Ok);
        assert!(
            ok.metrics
                .iter()
                .any(|metric| metric.name == "tx_dsp_evm_milli_pct" && metric.value == 6_000)
        );

        registry.mark_phy_tx_quality(8.0, 3.5, -45.0, 18_000.0);
        let degraded = registry.rf_snapshot(unix_ms());
        assert_eq!(degraded.severity, HealthSeverity::Degraded);

        registry.mark_phy_tx_quality(12.0, 3.5, -45.0, 18_000.0);
        let critical = registry.rf_snapshot(unix_ms());
        assert_eq!(critical.severity, HealthSeverity::Critical);
    }

    #[test]
    fn recent_actions_are_bounded() {
        let registry = HealthRegistry::new();
        for idx in 0..(MAX_RECENT_ACTIONS + 8) {
            registry.record_action(HealthDomain::Service, "restart_service", &format!("reason-{idx}"));
        }

        let snapshot = registry.snapshot();
        assert_eq!(snapshot.recent_actions.len(), MAX_RECENT_ACTIONS);
        assert_eq!(
            snapshot.recent_actions.first().map(|action| action.reason.as_str()),
            Some("reason-23")
        );
        assert_eq!(
            snapshot.recent_actions.last().map(|action| action.reason.as_str()),
            Some("reason-8")
        );
    }
}
