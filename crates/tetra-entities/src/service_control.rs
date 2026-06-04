use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

const SERVICE_UNIT_ENV: &str = "NEXUS_BS_SERVICE_UNIT";
const DEFAULT_SERVICE_UNIT: &str = "nexus-bs.service";
const NO_EXIT_REQUESTED: i32 = i32::MIN;
const RESTART_EXIT_CODE: i32 = 75;

struct LifecycleControl {
    running: Arc<AtomicBool>,
    exit_code: AtomicI32,
}

static LIFECYCLE_CONTROL: OnceLock<LifecycleControl> = OnceLock::new();

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
}

impl ServiceAction {
    fn systemctl_verb(self) -> &'static str {
        match self {
            ServiceAction::Restart => "restart",
            ServiceAction::Stop => "stop",
        }
    }

    fn label(self) -> &'static str {
        match self {
            ServiceAction::Restart => "restart",
            ServiceAction::Stop => "shutdown",
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
            if let Some(lifecycle) = LIFECYCLE_CONTROL.get() {
                let exit_code = match action {
                    ServiceAction::Restart => RESTART_EXIT_CODE,
                    ServiceAction::Stop => 0,
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
    use super::{normalize_service_unit, service_unit_from_cgroup_text};

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
}
