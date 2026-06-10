//! Lightweight health telemetry for Nexus-BS.
//!
//! P0 is observe-only: hot paths update atomics and a sidecar sampler emits a
//! snapshot through the already bounded telemetry channel. Remediation actions
//! are represented as data so P1 can add entity-owned recovery without letting
//! the monitor mutate RF/CMCE/UMAC state directly.

mod actions;
mod registry;
mod supervisor;
mod types;

pub use actions::{HealthActionKind, HealthActionRequest, HealthActionSink, HealthActionSource, health_action_channel};
pub use registry::{HealthRegistry, registry};
pub use supervisor::{HealthMonitorConfig, spawn_health_action_worker, spawn_health_monitor};
pub use types::{
    CmceHealthStats, HealthActionRecord, HealthDomain, HealthDomainSnapshot, HealthMetric, HealthSeverity, HealthSnapshot,
    HealthThresholds, SdsHealthStats, UmacHealthStats,
};
