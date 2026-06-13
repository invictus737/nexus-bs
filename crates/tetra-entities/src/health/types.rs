// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthSeverity {
    Ok,
    Degraded,
    Critical,
}

impl HealthSeverity {
    pub(crate) fn max(self, other: Self) -> Self {
        use HealthSeverity::{Critical, Degraded, Ok};
        match (self, other) {
            (Critical, _) | (_, Critical) => Critical,
            (Degraded, _) | (_, Degraded) => Degraded,
            (Ok, Ok) => Ok,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Encode, Decode, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthDomain {
    Service,
    Telemetry,
    Brew,
    Voice,
    Sds,
    P2p,
    Congestion,
    Rf,
}

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub struct HealthMetric {
    pub name: String,
    pub value: i64,
    pub unit: String,
}

impl HealthMetric {
    pub(crate) fn new(name: &str, value: i64, unit: &str) -> Self {
        Self {
            name: name.to_string(),
            value,
            unit: unit.to_string(),
        }
    }
}

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub struct HealthDomainSnapshot {
    pub domain: HealthDomain,
    pub severity: HealthSeverity,
    pub message: String,
    pub metrics: Vec<HealthMetric>,
}

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub struct HealthActionRecord {
    pub domain: HealthDomain,
    pub action: String,
    pub reason: String,
    pub unix_ms: u64,
}

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub struct HealthSnapshot {
    pub unix_ms: u64,
    pub overall: HealthSeverity,
    pub domains: Vec<HealthDomainSnapshot>,
    pub recent_actions: Vec<HealthActionRecord>,
}

#[derive(Debug, Clone, Copy)]
pub struct HealthThresholds {
    pub service_critical_tick_age_ms: u64,
}

impl Default for HealthThresholds {
    fn default() -> Self {
        Self {
            service_critical_tick_age_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CmceHealthStats {
    pub active_group_calls: usize,
    pub pending_group_releases: usize,
    pub pending_group_floor_activations: usize,
    pub pending_network_group_readies: usize,
    pub group_floor_waiters: usize,
    pub active_individual_calls: usize,
    pub pending_individual_calls: usize,
    pub pending_individual_releases: usize,
    pub pending_network_individual_connects: usize,
    pub pending_individual_disconnects: usize,
    pub individual_floor_waiters: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SdsHealthStats {
    pub live_queue_len: usize,
    pub pending_actions: usize,
    pub tl_report_contexts: usize,
    pub tl_report_context_evictions: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct UmacHealthStats {
    pub dl_queue_total: usize,
    pub dl_queue_max_per_ts: usize,
    pub next_slot_queue_len: usize,
    pub pending_ra_ack_total: usize,
    pub pending_ra_ack_max_per_ts: usize,
    pub pending_tma_reports: usize,
    pub pending_private_ul_media_total: usize,
    pub pending_stch: bool,
}
