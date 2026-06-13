// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::collections::HashMap;

use serde::Deserialize;
use toml::Value;

/// Observe-only operational health configuration.
#[derive(Debug, Clone)]
pub struct CfgHealth {
    /// Emit health snapshots through the bounded telemetry/dashboard path.
    pub enabled: bool,
    /// Snapshot cadence in seconds.
    pub snapshot_interval_secs: u64,
    /// Core loop age that should be reported as critical.
    pub core_stall_critical_ms: u64,
    /// Self-healing restart gate for persistent RF/core-loop stalls. Default is false.
    pub restart_on_core_stall: bool,
    /// Required duration of a critical core-loop stall before requesting restart.
    pub restart_after_critical_secs: u64,
    /// Minimum interval between self-healing restart requests.
    pub restart_cooldown_secs: u64,
}

impl Default for CfgHealth {
    fn default() -> Self {
        Self {
            enabled: true,
            snapshot_interval_secs: 1,
            core_stall_critical_ms: 10_000,
            restart_on_core_stall: false,
            restart_after_critical_secs: 30,
            restart_cooldown_secs: 600,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CfgHealthDto {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_snapshot_interval_secs")]
    pub snapshot_interval_secs: u64,
    #[serde(default = "default_core_stall_critical_ms")]
    pub core_stall_critical_ms: u64,
    #[serde(default)]
    pub restart_on_core_stall: bool,
    #[serde(default = "default_restart_after_critical_secs")]
    pub restart_after_critical_secs: u64,
    #[serde(default = "default_restart_cooldown_secs")]
    pub restart_cooldown_secs: u64,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Default for CfgHealthDto {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            snapshot_interval_secs: default_snapshot_interval_secs(),
            core_stall_critical_ms: default_core_stall_critical_ms(),
            restart_on_core_stall: false,
            restart_after_critical_secs: default_restart_after_critical_secs(),
            restart_cooldown_secs: default_restart_cooldown_secs(),
            extra: HashMap::new(),
        }
    }
}

fn default_enabled() -> bool {
    true
}

fn default_snapshot_interval_secs() -> u64 {
    1
}

fn default_core_stall_critical_ms() -> u64 {
    10_000
}

fn default_restart_after_critical_secs() -> u64 {
    30
}

fn default_restart_cooldown_secs() -> u64 {
    600
}

pub fn apply_health_patch(src: CfgHealthDto) -> Result<CfgHealth, String> {
    if src.snapshot_interval_secs == 0 {
        return Err("health: snapshot_interval_secs must be >= 1".to_string());
    }
    if src.core_stall_critical_ms < 1_000 {
        return Err("health: core_stall_critical_ms must be >= 1000".to_string());
    }
    if src.restart_after_critical_secs == 0 {
        return Err("health: restart_after_critical_secs must be >= 1".to_string());
    }
    if src.restart_cooldown_secs == 0 {
        return Err("health: restart_cooldown_secs must be >= 1".to_string());
    }

    Ok(CfgHealth {
        enabled: src.enabled,
        snapshot_interval_secs: src.snapshot_interval_secs,
        core_stall_critical_ms: src.core_stall_critical_ms,
        restart_on_core_stall: src.restart_on_core_stall,
        restart_after_critical_secs: src.restart_after_critical_secs,
        restart_cooldown_secs: src.restart_cooldown_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_defaults_are_observe_only() {
        let cfg = apply_health_patch(CfgHealthDto::default()).expect("default health config");

        assert!(cfg.enabled);
        assert_eq!(cfg.snapshot_interval_secs, 1);
        assert!(!cfg.restart_on_core_stall);
        assert_eq!(cfg.restart_cooldown_secs, 600);
    }

    #[test]
    fn health_rejects_zero_snapshot_interval() {
        let dto = CfgHealthDto {
            snapshot_interval_secs: 0,
            ..CfgHealthDto::default()
        };
        let err = apply_health_patch(dto).expect_err("zero interval should fail");

        assert!(err.contains("snapshot_interval_secs"));
    }
}
