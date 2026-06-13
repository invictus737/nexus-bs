// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::time::Duration;

const SERVICE_UNIT_ENV: &str = "NEXUS_BS_SERVICE_UNIT";
const DEFAULT_SERVICE_UNIT: &str = "nexus-bs.service";
const NO_EXIT_REQUESTED: i32 = i32::MIN;
const RESTART_EXIT_CODE: i32 = 75;
const NOTIFY_SOCKET_ENV: &str = "NOTIFY_SOCKET";
const WATCHDOG_USEC_ENV: &str = "WATCHDOG_USEC";

struct LifecycleControl {
    running: Arc<AtomicBool>,
    exit_code: AtomicI32,
}

static LIFECYCLE_CONTROL: OnceLock<LifecycleControl> = OnceLock::new();
static STACK_TICK_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Service unit configured from the TOML config file (e.g. service_name = "nexus-bs").
/// Takes precedence over cgroup auto-detection but is overridden by NEXUS_BS_SERVICE_UNIT.
static CONFIGURED_SERVICE_UNIT: OnceLock<String> = OnceLock::new();

/// Set the service unit from config — should be called once at startup.
/// Subsequent calls are ignored (OnceLock).
pub fn set_configured_service_unit(unit: &str) {
    if let Some(normalized) = normalize_service_unit(unit) {
        let _ = CONFIGURED_SERVICE_UNIT.set(normalized);
    } else {
        tracing::warn!("Service control: ignoring invalid configured service_name={:?}", unit);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ServiceAction {
    Restart,
    Stop,
    PowerOffHost,
}

impl ServiceAction {
    fn systemctl_verb(self) -> &'static str {
        match self {
            ServiceAction::Restart => "restart",
            ServiceAction::Stop => "stop",
            ServiceAction::PowerOffHost => "poweroff",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ServiceAction::Restart => "restart",
            ServiceAction::Stop => "shutdown",
            ServiceAction::PowerOffHost => "host poweroff",
        }
    }
}

pub fn install_lifecycle_control(running: Arc<AtomicBool>) {
    let _ = LIFECYCLE_CONTROL.set(LifecycleControl {
        running,
        exit_code: AtomicI32::new(NO_EXIT_REQUESTED),
    });
}

pub fn requested_exit_code() -> Option<i32> {
    let lifecycle = LIFECYCLE_CONTROL.get()?;
    let code = lifecycle.exit_code.load(Ordering::SeqCst);
    (code != NO_EXIT_REQUESTED).then_some(code)
}

pub fn mark_stack_tick() {
    STACK_TICK_COUNTER.fetch_add(1, Ordering::Relaxed);
}

fn stack_tick_count() -> u64 {
    STACK_TICK_COUNTER.load(Ordering::Relaxed)
}

pub fn notify_ready(status: &str) {
    let payload = format!("READY=1\nSTATUS={}", sanitize_notify_status(status));
    notify_systemd(&payload);
}

pub fn notify_stopping(status: &str) {
    let payload = format!("STOPPING=1\nSTATUS={}", sanitize_notify_status(status));
    notify_systemd(&payload);
}

pub fn spawn_systemd_watchdog(running: Arc<AtomicBool>) -> Option<std::thread::JoinHandle<()>> {
    let interval = watchdog_interval_from_env()?;
    tracing::info!("Service watchdog: enabled with heartbeat interval {:?}", interval);
    std::thread::Builder::new()
        .name("systemd-watchdog".into())
        .spawn(move || {
            let mut last_tick = stack_tick_count();
            loop {
                std::thread::sleep(interval);
                if !running.load(Ordering::Relaxed) {
                    notify_stopping("Nexus-BS stopping");
                    break;
                }

                let current_tick = stack_tick_count();
                if current_tick == last_tick {
                    let calibration_phase = crate::rf_calibration::current_phase();
                    if calibration_phase.allows_service_watchdog_stall() {
                        notify_systemd(&format!(
                            "WATCHDOG=1\nSTATUS=Nexus-BS RF calibration {}",
                            calibration_phase.as_str()
                        ));
                        continue;
                    }
                    tracing::error!(
                        "Service watchdog: stack tick counter stalled at {}, withholding WATCHDOG=1",
                        current_tick
                    );
                    continue;
                }
                last_tick = current_tick;
                notify_systemd("WATCHDOG=1\nSTATUS=Nexus-BS RF loop alive");
            }
        })
        .ok()
}

fn watchdog_interval_from_env() -> Option<Duration> {
    std::env::var(WATCHDOG_USEC_ENV)
        .ok()
        .and_then(|value| watchdog_interval_from_usec(&value))
}

fn watchdog_interval_from_usec(value: &str) -> Option<Duration> {
    let usec = value.trim().parse::<u64>().ok()?;
    if usec == 0 {
        return None;
    }
    let half = (usec / 2).max(1_000_000);
    Some(Duration::from_micros(half))
}

fn sanitize_notify_status(status: &str) -> String {
    status.replace(['\n', '\r'], " ")
}

fn notify_systemd(payload: &str) {
    let Ok(socket) = std::env::var(NOTIFY_SOCKET_ENV) else {
        return;
    };
    if socket.trim().is_empty() {
        return;
    }
    if let Err(e) = send_notify_datagram(&socket, payload.as_bytes()) {
        tracing::debug!("Service watchdog: systemd notify failed: {}", e);
    }
}

#[cfg(unix)]
fn send_notify_datagram(socket: &str, payload: &[u8]) -> Result<(), String> {
    use std::os::unix::net::UnixDatagram;

    let sock = UnixDatagram::unbound().map_err(|e| e.to_string())?;

    #[cfg(target_os = "linux")]
    if let Some(name) = socket.strip_prefix('@') {
        use std::os::linux::net::SocketAddrExt;
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).map_err(|e| e.to_string())?;
        return sock.send_to_addr(payload, &addr).map(|_| ()).map_err(|e| e.to_string());
    }

    if socket.starts_with('@') {
        return Err("abstract NOTIFY_SOCKET is only supported on Linux".to_string());
    }

    sock.send_to(payload, socket).map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn send_notify_datagram(_socket: &str, _payload: &[u8]) -> Result<(), String> {
    Err("systemd notify is only supported on Unix".to_string())
}

pub fn schedule_service_action(action: ServiceAction, delay: Duration) {
    let unit = resolve_service_unit();
    let service_user = service_user(&unit).unwrap_or_else(|| "unknown".to_string());
    tracing::warn!(
        "Service control: scheduling {} for {} (unit User={}) in {:?}",
        action.label(),
        unit,
        service_user,
        delay
    );

    std::thread::Builder::new()
        .name("service-control".into())
        .spawn(move || {
            std::thread::sleep(delay);
            if matches!(action, ServiceAction::PowerOffHost) {
                match run_host_poweroff() {
                    Ok(()) => {
                        tracing::warn!("Service control: host poweroff requested");
                        if let Some(lifecycle) = LIFECYCLE_CONTROL.get() {
                            lifecycle.running.store(false, Ordering::SeqCst);
                        }
                    }
                    Err(e) => tracing::error!("Service control: host poweroff failed: {}", e),
                }
                return;
            }
            if let Some(lifecycle) = LIFECYCLE_CONTROL.get() {
                let exit_code = match action {
                    ServiceAction::Restart => RESTART_EXIT_CODE,
                    ServiceAction::Stop => 0,
                    ServiceAction::PowerOffHost => unreachable!("poweroff handled before lifecycle stop"),
                };
                lifecycle.exit_code.store(exit_code, Ordering::SeqCst);
                lifecycle.running.store(false, Ordering::SeqCst);
                tracing::info!(
                    "Service control: {} requested internally for {} with exit code {}",
                    action.label(),
                    unit,
                    exit_code
                );
            } else {
                match run_service_action(action, &unit) {
                    Ok(()) => tracing::info!("Service control: {} requested for {}", action.label(), unit),
                    Err(e) => tracing::error!("Service control: {} failed for {}: {}", action.label(), unit, e),
                }
            }
        })
        .ok();
}

pub fn resolve_service_unit() -> String {
    if let Ok(value) = std::env::var(SERVICE_UNIT_ENV) {
        if let Some(unit) = normalize_service_unit(&value) {
            return unit;
        }
        tracing::warn!("Service control: ignoring invalid {}={:?}", SERVICE_UNIT_ENV, value);
    }
    if let Some(configured) = CONFIGURED_SERVICE_UNIT.get() {
        return configured.clone();
    }

    std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|text| service_unit_from_cgroup_text(&text))
        .unwrap_or_else(|| DEFAULT_SERVICE_UNIT.to_string())
}

fn run_service_action(action: ServiceAction, unit: &str) -> Result<(), String> {
    let verb = action.systemctl_verb();
    match run_command("systemctl", &[verb, unit]) {
        Ok(()) => Ok(()),
        Err(systemctl_err) => match run_command("sudo", &["-n", "systemctl", verb, unit]) {
            Ok(()) => Ok(()),
            Err(sudo_err) => Err(format!("systemctl: {}; sudo -n: {}", systemctl_err, sudo_err)),
        },
    }
}

fn run_host_poweroff() -> Result<(), String> {
    match run_command("systemctl", &["--no-block", "poweroff"]) {
        Ok(()) => Ok(()),
        Err(systemctl_err) => match run_command("sudo", &["-n", "systemctl", "--no-block", "poweroff"]) {
            Ok(()) => Ok(()),
            Err(sudo_err) => Err(format!("systemctl: {}; sudo -n: {}", systemctl_err, sudo_err)),
        },
    }
}

fn run_command(program: &str, args: &[&str]) -> Result<(), String> {
    match Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(output_error(output)),
        Err(e) => Err(e.to_string()),
    }
}

fn output_error(output: Output) -> String {
    let status = output.status.to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stderr.is_empty() {
        format!("{}: {}", status, stderr)
    } else if !stdout.is_empty() {
        format!("{}: {}", status, stdout)
    } else {
        status
    }
}

fn service_user(unit: &str) -> Option<String> {
    let output = Command::new("systemctl")
        .args(["show", unit, "--property=User", "--value"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let user = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if user.is_empty() { Some("root".to_string()) } else { Some(user) }
}

fn service_unit_from_cgroup_text(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.split('/')
            .find(|component| component.ends_with(".service"))
            .and_then(normalize_service_unit)
    })
}

fn normalize_service_unit(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains('/') || trimmed.contains('\0') {
        return None;
    }

    let unit = if trimmed.ends_with(".service") {
        trimmed.to_string()
    } else {
        format!("{}.service", trimmed)
    };

    if unit
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'@' | b':' | b'\\'))
    {
        Some(unit)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{normalize_service_unit, sanitize_notify_status, service_unit_from_cgroup_text, watchdog_interval_from_usec};

    #[test]
    fn finds_service_unit_in_cgroup_v2() {
        let text = "0::/system.slice/nexus-bs.service\n";
        assert_eq!(service_unit_from_cgroup_text(text).as_deref(), Some("nexus-bs.service"));
    }

    #[test]
    fn normalizes_unit_without_suffix() {
        assert_eq!(normalize_service_unit("nexus-bs").as_deref(), Some("nexus-bs.service"));
    }

    #[test]
    fn rejects_path_like_unit_names() {
        assert!(normalize_service_unit("../tetra").is_none());
    }

    #[test]
    fn watchdog_interval_uses_half_of_systemd_timeout() {
        assert_eq!(watchdog_interval_from_usec("30000000"), Some(Duration::from_secs(15)));
    }

    #[test]
    fn watchdog_interval_has_one_second_floor() {
        assert_eq!(watchdog_interval_from_usec("1000"), Some(Duration::from_secs(1)));
        assert_eq!(watchdog_interval_from_usec("0"), None);
    }

    #[test]
    fn notify_status_is_single_line() {
        assert_eq!(sanitize_notify_status("ready\nwith\rcarriage"), "ready with carriage");
    }
}
