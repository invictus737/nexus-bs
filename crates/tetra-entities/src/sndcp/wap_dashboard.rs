// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original pure dashboard-state adapter for TETRA SNDCP WAP/IP status pages.

use super::wap_status::{WAP_STATUS_DETAIL_MAX_LINES, WapStatusSnapshot};
use crate::health::{HealthDomain, HealthSeverity, HealthSnapshot};
use crate::net_dashboard::state::{CallEntry, DashboardStateInner, LastHeardEntry, MsEntry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WapDashboardSnapshotOptions {
    pub title: String,
    pub stack_version: String,
    pub uptime_secs: u64,
    pub service_state_override: Option<String>,
    pub max_radio_lines: usize,
    pub max_call_lines: usize,
}

impl Default for WapDashboardSnapshotOptions {
    fn default() -> Self {
        Self {
            title: "Nexus-BS".to_string(),
            stack_version: tetra_core::STACK_VERSION.to_string(),
            uptime_secs: 0,
            service_state_override: None,
            max_radio_lines: WAP_STATUS_DETAIL_MAX_LINES,
            max_call_lines: WAP_STATUS_DETAIL_MAX_LINES,
        }
    }
}

pub fn wap_status_snapshot_from_dashboard(state: &DashboardStateInner, options: &WapDashboardSnapshotOptions) -> WapStatusSnapshot {
    WapStatusSnapshot {
        title: options.title.clone(),
        stack_version: options.stack_version.clone(),
        service_state: dashboard_service_state(state, options),
        registered_ms: state.ms_map.len(),
        active_calls: state.calls.len(),
        queued_sds: queued_sds_from_health(state.last_health.as_ref()),
        uptime_secs: options.uptime_secs,
        last_activity: state.last_heard.front().map(last_activity_text),
        health_summary: health_summary_from_dashboard(state.last_health.as_ref()),
        health_lines: health_lines_from_dashboard(state.last_health.as_ref()),
        radio_lines: radio_lines_from_dashboard(state, options.max_radio_lines),
        call_lines: call_lines_from_dashboard(state, options.max_call_lines),
    }
}

pub fn dashboard_service_state(state: &DashboardStateInner, options: &WapDashboardSnapshotOptions) -> String {
    if let Some(override_state) = options
        .service_state_override
        .as_deref()
        .map(str::trim)
        .filter(|state| !state.is_empty())
    {
        return override_state.to_string();
    }

    match state.last_health.as_ref().map(|health| health.overall) {
        Some(HealthSeverity::Critical) => "CRITICAL".to_string(),
        Some(HealthSeverity::Degraded) => "DEGRADED".to_string(),
        _ if state.fallback_config_active => "FALLBACK".to_string(),
        _ if state.brew_online => format!("ON AIR/BREW{}", state.brew_version),
        _ => "ON AIR".to_string(),
    }
}

pub fn queued_sds_from_health(health: Option<&HealthSnapshot>) -> usize {
    let Some(health) = health else {
        return 0;
    };

    let Some(sds) = health.domains.iter().find(|domain| domain.domain == HealthDomain::Sds) else {
        return 0;
    };

    metric_value(&sds.metrics, "live_queue_len").saturating_add(metric_value(&sds.metrics, "pending_actions"))
}

pub fn last_activity_text(entry: &LastHeardEntry) -> String {
    let kind = match entry.activity.as_str() {
        "call_group" => "GRP",
        "call_p2p_simplex" => "P2P-S",
        "call_p2p_duplex" => "P2P-D",
        "sds" => "SDS",
        _ => "ACT",
    };

    if entry.dest == 0 {
        format!("{kind} {}", entry.issi)
    } else {
        format!("{kind} {}>{}", entry.issi, entry.dest)
    }
}

pub fn radio_lines_from_dashboard(state: &DashboardStateInner, max_lines: usize) -> Vec<String> {
    let mut entries: Vec<&MsEntry> = state.ms_map.values().collect();
    entries.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then_with(|| a.issi.cmp(&b.issi)));
    entries.into_iter().take(max_lines).map(radio_line_text).collect()
}

pub fn call_lines_from_dashboard(state: &DashboardStateInner, max_lines: usize) -> Vec<String> {
    let mut entries: Vec<&CallEntry> = state.calls.values().collect();
    entries.sort_by(|a, b| b.started_at.cmp(&a.started_at).then_with(|| a.call_id.cmp(&b.call_id)));
    entries.into_iter().take(max_lines).map(call_line_text).collect()
}

pub fn health_summary_from_dashboard(health: Option<&HealthSnapshot>) -> Option<String> {
    let health = health?;
    let overall = health_severity_label(health.overall);

    if health.overall == HealthSeverity::Ok {
        return Some(overall.to_string());
    }

    let worst_domain_severity = if health.domains.iter().any(|domain| domain.severity == HealthSeverity::Critical) {
        HealthSeverity::Critical
    } else {
        HealthSeverity::Degraded
    };
    let mut worst_domains: Vec<&crate::health::HealthDomainSnapshot> = health
        .domains
        .iter()
        .filter(|domain| domain.severity == worst_domain_severity)
        .collect();
    worst_domains.sort_by_key(|domain| health_domain_label(domain.domain));

    let Some(first) = worst_domains.first() else {
        return Some(overall.to_string());
    };
    let affected_domains = health.domains.iter().filter(|domain| domain.severity != HealthSeverity::Ok).count();
    let suffix = affected_domains
        .checked_sub(1)
        .filter(|extra| *extra > 0)
        .map(|extra| format!("+{extra}"))
        .unwrap_or_default();
    Some(format!("{overall}:{}{suffix}", health_domain_label(first.domain)))
}

pub fn health_lines_from_dashboard(health: Option<&HealthSnapshot>) -> Vec<String> {
    let Some(health) = health else {
        return Vec::new();
    };

    let mut lines: Vec<String> = health
        .domains
        .iter()
        .map(|domain| format!("{} {}", health_domain_label(domain.domain), health_severity_short(domain.severity)))
        .collect();
    lines.sort();
    lines
}

pub fn radio_line_text(entry: &MsEntry) -> String {
    let rssi = entry
        .rssi_dbfs
        .map(|rssi| format!("{rssi:.0}dB"))
        .unwrap_or_else(|| "--dB".to_string());
    format!(
        "MS {} {} G{} {}",
        entry.issi,
        rssi,
        group_summary(&entry.groups),
        energy_saving_label(entry.energy_saving_mode)
    )
}

pub fn call_line_text(entry: &CallEntry) -> String {
    let ts = match entry.secondary_ts {
        Some(secondary_ts) => format!("TS{}/{}", entry.ts, secondary_ts),
        None => format!("TS{}", entry.ts),
    };

    if entry.is_group {
        format!("GRP {} sp{} {}", entry.gssi, entry.speaker_issi.unwrap_or(entry.caller_issi), ts)
    } else {
        let mode = if entry.simplex { "P2P-S" } else { "P2P-D" };
        format!("{mode} {}>{} {ts}", entry.caller_issi, entry.called_issi)
    }
}

fn group_summary(groups: &[u32]) -> String {
    let mut groups = groups.to_vec();
    groups.sort_unstable();
    match groups.as_slice() {
        [] => "0".to_string(),
        [group] => group.to_string(),
        [first, rest @ ..] => format!("{first}+{}", rest.len()),
    }
}

fn energy_saving_label(mode: u8) -> String {
    if mode == 0 { "SA".to_string() } else { format!("EG{mode}") }
}

fn health_severity_label(severity: HealthSeverity) -> &'static str {
    match severity {
        HealthSeverity::Ok => "OK",
        HealthSeverity::Degraded => "DEGRADED",
        HealthSeverity::Critical => "CRITICAL",
    }
}

fn health_severity_short(severity: HealthSeverity) -> &'static str {
    match severity {
        HealthSeverity::Ok => "OK",
        HealthSeverity::Degraded => "WARN",
        HealthSeverity::Critical => "BAD",
    }
}

fn health_domain_label(domain: HealthDomain) -> &'static str {
    match domain {
        HealthDomain::Service => "CORE",
        HealthDomain::Telemetry => "TEL",
        HealthDomain::Brew => "BREW",
        HealthDomain::Voice => "VOICE",
        HealthDomain::Sds => "SDS",
        HealthDomain::P2p => "P2P",
        HealthDomain::Congestion => "LOAD",
        HealthDomain::Rf => "RF",
    }
}

fn metric_value(metrics: &[crate::health::HealthMetric], name: &str) -> usize {
    metrics
        .iter()
        .find(|metric| metric.name == name)
        .map(|metric| usize::try_from(metric.value.max(0)).unwrap_or(usize::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::{HealthDomainSnapshot, HealthMetric};
    use crate::net_dashboard::state::{CallEntry, LastHeardEntry, MsEntry};
    use std::time::{Duration, Instant};

    fn options() -> WapDashboardSnapshotOptions {
        WapDashboardSnapshotOptions {
            title: "Nexus-BS WAP".to_string(),
            stack_version: "v0.1.69_dev-test".to_string(),
            uptime_secs: 3661,
            service_state_override: None,
            max_radio_lines: WAP_STATUS_DETAIL_MAX_LINES,
            max_call_lines: WAP_STATUS_DETAIL_MAX_LINES,
        }
    }

    fn health(overall: HealthSeverity, live_queue_len: i64, pending_actions: i64) -> HealthSnapshot {
        HealthSnapshot {
            unix_ms: 0,
            overall,
            domains: vec![HealthDomainSnapshot {
                domain: HealthDomain::Sds,
                severity: HealthSeverity::Ok,
                message: "sds".to_string(),
                metrics: vec![
                    HealthMetric {
                        name: "live_queue_len".to_string(),
                        value: live_queue_len,
                        unit: "messages".to_string(),
                    },
                    HealthMetric {
                        name: "pending_actions".to_string(),
                        value: pending_actions,
                        unit: "actions".to_string(),
                    },
                ],
            }],
            recent_actions: Vec::new(),
        }
    }

    #[test]
    fn dashboard_state_maps_to_compact_wap_status_snapshot() {
        let now = Instant::now();
        let mut state = DashboardStateInner::new("test.toml".to_string());
        state.ms_map.insert(
            2_260_618,
            MsEntry {
                issi: 2_260_618,
                groups: vec![91],
                rssi_dbfs: Some(-47.0),
                registered_at: now,
                last_seen: now,
                energy_saving_mode: 0,
                energy_saving_frame: None,
                energy_saving_multiframe: None,
            },
        );
        state.calls.insert(
            7,
            CallEntry {
                call_id: 7,
                is_group: true,
                gssi: 91,
                caller_issi: 2_260_618,
                called_issi: 0,
                speaker_issi: Some(2_260_618),
                started_at: now,
                simplex: true,
                ts: 2,
                secondary_ts: None,
            },
        );
        state.last_heard.push_front(LastHeardEntry {
            ts: "12:00:00".to_string(),
            issi: 2_260_618,
            activity: "sds".to_string(),
            dest: 2_260_082,
        });
        state.last_health = Some(health(HealthSeverity::Degraded, 2, 3));

        let snapshot = wap_status_snapshot_from_dashboard(&state, &options());

        assert_eq!(snapshot.title, "Nexus-BS WAP");
        assert_eq!(snapshot.stack_version, "v0.1.69_dev-test");
        assert_eq!(snapshot.service_state, "DEGRADED");
        assert_eq!(snapshot.registered_ms, 1);
        assert_eq!(snapshot.active_calls, 1);
        assert_eq!(snapshot.queued_sds, 5);
        assert_eq!(snapshot.uptime_secs, 3661);
        assert_eq!(snapshot.last_activity.as_deref(), Some("SDS 2260618>2260082"));
        assert_eq!(snapshot.health_summary.as_deref(), Some("DEGRADED"));
        assert_eq!(snapshot.radio_lines, vec!["MS 2260618 -47dB G91 SA"]);
        assert_eq!(snapshot.call_lines, vec!["GRP 91 sp2260618 TS2"]);
    }

    #[test]
    fn dashboard_service_state_uses_override_then_health_then_local_flags() {
        let mut state = DashboardStateInner::new("test.toml".to_string());
        state.brew_online = true;
        state.brew_version = 1;

        assert_eq!(dashboard_service_state(&state, &options()), "ON AIR/BREW1");

        state.fallback_config_active = true;
        assert_eq!(dashboard_service_state(&state, &options()), "FALLBACK");

        state.last_health = Some(health(HealthSeverity::Critical, 0, 0));
        assert_eq!(dashboard_service_state(&state, &options()), "CRITICAL");

        let mut options = options();
        options.service_state_override = Some(" FIELD TEST ".to_string());
        assert_eq!(dashboard_service_state(&state, &options), "FIELD TEST");
    }

    #[test]
    fn queued_sds_uses_only_nonnegative_sds_health_metrics() {
        assert_eq!(queued_sds_from_health(None), 0);
        assert_eq!(queued_sds_from_health(Some(&health(HealthSeverity::Ok, -2, 7))), 7);
        assert_eq!(queued_sds_from_health(Some(&health(HealthSeverity::Ok, 4, 6))), 10);
    }

    #[test]
    fn last_activity_text_is_short_for_wap_terminals() {
        assert_eq!(
            last_activity_text(&LastHeardEntry {
                ts: "12:00:00".to_string(),
                issi: 2_260_618,
                activity: "call_group".to_string(),
                dest: 91,
            }),
            "GRP 2260618>91"
        );
        assert_eq!(
            last_activity_text(&LastHeardEntry {
                ts: "12:00:01".to_string(),
                issi: 2_260_618,
                activity: "call_p2p_duplex".to_string(),
                dest: 2_260_082,
            }),
            "P2P-D 2260618>2260082"
        );
    }

    #[test]
    fn radio_and_call_lines_are_sorted_and_bounded_for_wap_cards() {
        let now = Instant::now();
        let mut state = DashboardStateInner::new("test.toml".to_string());
        for (issi, rssi, energy_saving_mode, groups, last_seen_age_secs) in [
            (2_260_618, -47.0, 0, vec![91], 8),
            (2_260_082, -52.0, 3, vec![91], 4),
            (2_260_616, -41.0, 1, vec![93, 91, 92], 0),
        ] {
            state.ms_map.insert(
                issi,
                MsEntry {
                    issi,
                    groups,
                    rssi_dbfs: Some(rssi),
                    registered_at: now,
                    last_seen: now - Duration::from_secs(last_seen_age_secs),
                    energy_saving_mode,
                    energy_saving_frame: None,
                    energy_saving_multiframe: None,
                },
            );
        }
        state.calls.insert(
            9,
            CallEntry {
                call_id: 9,
                is_group: false,
                gssi: 0,
                caller_issi: 2_260_618,
                called_issi: 2_260_082,
                speaker_issi: None,
                started_at: now - Duration::from_secs(5),
                simplex: false,
                ts: 2,
                secondary_ts: Some(3),
            },
        );
        state.calls.insert(
            1,
            CallEntry {
                call_id: 1,
                is_group: true,
                gssi: 91,
                caller_issi: 2_260_616,
                called_issi: 0,
                speaker_issi: Some(2_260_616),
                started_at: now,
                simplex: true,
                ts: 4,
                secondary_ts: None,
            },
        );

        assert_eq!(
            radio_lines_from_dashboard(&state, 2),
            vec!["MS 2260616 -41dB G91+2 EG1", "MS 2260082 -52dB G91 EG3"]
        );
        assert_eq!(
            call_lines_from_dashboard(&state, 2),
            vec!["GRP 91 sp2260616 TS4", "P2P-D 2260618>2260082 TS2/3"]
        );

        state.ms_map.insert(
            2_260_000,
            MsEntry {
                issi: 2_260_000,
                groups: Vec::new(),
                rssi_dbfs: None,
                registered_at: now,
                last_seen: now,
                energy_saving_mode: 0,
                energy_saving_frame: None,
                energy_saving_multiframe: None,
            },
        );
        assert_eq!(radio_lines_from_dashboard(&state, 0), Vec::<String>::new());
        assert_eq!(radio_line_text(state.ms_map.get(&2_260_000).unwrap()), "MS 2260000 --dB G0 SA");
    }

    #[test]
    fn health_summary_counts_degraded_and_critical_domains() {
        let snapshot = HealthSnapshot {
            unix_ms: 0,
            overall: HealthSeverity::Critical,
            domains: vec![
                HealthDomainSnapshot {
                    domain: HealthDomain::Rf,
                    severity: HealthSeverity::Critical,
                    message: "rf".to_string(),
                    metrics: Vec::new(),
                },
                HealthDomainSnapshot {
                    domain: HealthDomain::Brew,
                    severity: HealthSeverity::Degraded,
                    message: "brew".to_string(),
                    metrics: Vec::new(),
                },
            ],
            recent_actions: Vec::new(),
        };

        assert_eq!(health_summary_from_dashboard(Some(&snapshot)).as_deref(), Some("CRITICAL:RF+1"));
        assert_eq!(health_summary_from_dashboard(None), None);
    }
}
