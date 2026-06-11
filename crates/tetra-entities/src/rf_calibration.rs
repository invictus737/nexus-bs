use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::json;
use tetra_config::bluestation::read_tx_calibration_file;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationPhase {
    Idle,
    Inhibiting,
    Calibrating,
    Calibrated,
    Restarting,
    Failed,
}

impl CalibrationPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            CalibrationPhase::Idle => "idle",
            CalibrationPhase::Inhibiting => "inhibiting",
            CalibrationPhase::Calibrating => "calibrating",
            CalibrationPhase::Calibrated => "calibrated",
            CalibrationPhase::Restarting => "restarting",
            CalibrationPhase::Failed => "failed",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(
            self,
            CalibrationPhase::Inhibiting | CalibrationPhase::Calibrating | CalibrationPhase::Restarting
        )
    }

    pub fn allows_service_watchdog_stall(self) -> bool {
        matches!(
            self,
            CalibrationPhase::Calibrating | CalibrationPhase::Calibrated | CalibrationPhase::Restarting
        )
    }
}

#[derive(Debug, Clone)]
pub struct CalibrationRuntimeState {
    phase: CalibrationPhase,
    calibration_path: String,
    log: String,
    error: String,
    updated_unix_secs: u64,
}

impl Default for CalibrationRuntimeState {
    fn default() -> Self {
        Self {
            phase: CalibrationPhase::Idle,
            calibration_path: String::new(),
            log: String::new(),
            error: String::new(),
            updated_unix_secs: 0,
        }
    }
}

static STATE: OnceLock<Mutex<CalibrationRuntimeState>> = OnceLock::new();

fn state() -> &'static Mutex<CalibrationRuntimeState> {
    STATE.get_or_init(|| Mutex::new(CalibrationRuntimeState::default()))
}

pub fn calibration_path_for_config(config_path: &str) -> PathBuf {
    Path::new(config_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(tetra_config::bluestation::TX_CALIBRATION_DEFAULT_FILE)
}

pub fn try_start(calibration_path: &str) -> Result<(), String> {
    let mut s = state().lock().map_err(|_| "calibration state lock poisoned".to_string())?;
    if s.phase.is_active() {
        return Err(format!("calibration already running: {}", s.phase.as_str()));
    }
    s.phase = CalibrationPhase::Inhibiting;
    s.calibration_path = calibration_path.to_string();
    s.error.clear();
    s.log.clear();
    s.updated_unix_secs = unix_secs_now();
    s.append("dashboard accepted calibration request");
    Ok(())
}

pub fn append_log(message: impl AsRef<str>) {
    if let Ok(mut s) = state().lock() {
        s.append(message.as_ref());
    }
}

pub fn mark_calibrating(calibration_path: &str) {
    if let Ok(mut s) = state().lock() {
        s.phase = CalibrationPhase::Calibrating;
        s.calibration_path = calibration_path.to_string();
        s.updated_unix_secs = unix_secs_now();
        s.append("PHY started destructive TX DC/IQ calibration");
    }
}

pub fn mark_calibrated(message: impl AsRef<str>) {
    if let Ok(mut s) = state().lock() {
        s.phase = CalibrationPhase::Calibrated;
        s.error.clear();
        s.updated_unix_secs = unix_secs_now();
        s.append(message.as_ref());
    }
}

pub fn mark_restarting(message: impl AsRef<str>) {
    if let Ok(mut s) = state().lock() {
        s.phase = CalibrationPhase::Restarting;
        s.updated_unix_secs = unix_secs_now();
        s.append(message.as_ref());
    }
}

pub fn mark_failed(error: impl AsRef<str>) {
    if let Ok(mut s) = state().lock() {
        s.phase = CalibrationPhase::Failed;
        s.error = error.as_ref().to_string();
        s.updated_unix_secs = unix_secs_now();
        let line = format!("ERROR: {}", s.error);
        s.append(&line);
    }
}

pub fn current_phase() -> CalibrationPhase {
    state().lock().map(|s| s.phase).unwrap_or(CalibrationPhase::Failed)
}

pub fn status_json(default_calibration_path: &Path) -> serde_json::Value {
    let snapshot = state().lock().map(|s| s.clone()).unwrap_or_default();
    let active_path = if snapshot.calibration_path.is_empty() {
        default_calibration_path.to_path_buf()
    } else {
        PathBuf::from(&snapshot.calibration_path)
    };
    let file = read_tx_calibration_file(&active_path).ok();
    json!({
        "ok": true,
        "status": snapshot.phase.as_str(),
        "active": snapshot.phase.is_active(),
        "path": active_path.display().to_string(),
        "error": if snapshot.error.is_empty() { serde_json::Value::Null } else { json!(snapshot.error) },
        "updated_unix_secs": snapshot.updated_unix_secs,
        "log": snapshot.log,
        "report": file,
    })
}

impl CalibrationRuntimeState {
    fn append(&mut self, message: &str) {
        let ts = unix_secs_now();
        self.updated_unix_secs = ts;
        self.log.push_str(&format!("[{}] {}\n", ts, message));
    }
}

fn unix_secs_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::CalibrationPhase;

    #[test]
    fn calibration_phase_allows_watchdog_stall_only_while_phy_is_blocked_or_restarting() {
        assert!(!CalibrationPhase::Idle.allows_service_watchdog_stall());
        assert!(!CalibrationPhase::Inhibiting.allows_service_watchdog_stall());
        assert!(CalibrationPhase::Calibrating.allows_service_watchdog_stall());
        assert!(CalibrationPhase::Calibrated.allows_service_watchdog_stall());
        assert!(CalibrationPhase::Restarting.allows_service_watchdog_stall());
        assert!(!CalibrationPhase::Failed.allows_service_watchdog_stall());
    }
}
