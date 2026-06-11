use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use toml::Value;

pub const TX_CALIBRATION_DEFAULT_FILE: &str = "calibration.toml";

/// SoapySDR configuration
#[derive(Debug, Clone)]
pub struct CfgSoapySdr {
    /// Uplink frequency in Hz
    pub ul_freq: f64,
    /// Downlink frequency in Hz
    pub dl_freq: f64,
    /// PPM frequency error correction
    pub ppm_err: f64,
    /// Argument string to select a specific SDR device.
    /// If None, devices will be enumerated until the first supported device is found.
    pub device: Option<String>,
    /// RX antenna. Device specific default will be used if None.
    pub rx_ant: Option<String>,
    /// TX antenna. Device specific default will be used if None.
    pub tx_ant: Option<String>,
    /// RX gain values.
    /// Device specific defaults will be used for gains that are not set.
    pub rx_gains: HashMap<String, f64>,
    /// TX gain values.
    /// Device specific defaults will be used for gains that are not set.
    pub tx_gains: HashMap<String, f64>,
    /// RX and TX sample rate. Device specific default will be used if None.
    pub fs: Option<f64>,
    /// RX channel number
    pub rx_ch: Option<usize>,
    /// TX channel number
    pub tx_ch: Option<usize>,
    /// Apply TX DC/IQ calibration from calibration file at startup.
    pub tx_calibration_enabled: bool,
    /// Path to TX calibration TOML. Relative paths are resolved by the service
    /// working directory; Nexus-BS systemd units run from the install folder.
    pub tx_calibration_file: String,
    /// Apply persisted TX DC offset correction.
    pub tx_calibration_apply_dc: bool,
    /// Apply persisted TX IQ balance correction. This is intentionally opt-in:
    /// CW tone image improvement alone is not enough evidence for live TETRA
    /// burst EVM improvement.
    pub tx_calibration_apply_iq: bool,
}

impl CfgSoapySdr {
    /// Get corrected UL frequency with PPM error applied
    pub fn ul_freq_corrected(&self) -> (f64, f64) {
        let ppm = self.ppm_err;
        let err = (self.ul_freq / 1_000_000.0) * ppm;
        (self.ul_freq + err, err)
    }

    /// Get corrected DL frequency with PPM error applied
    pub fn dl_freq_corrected(&self) -> (f64, f64) {
        let ppm = self.ppm_err;
        let err = (self.dl_freq / 1_000_000.0) * ppm;
        (self.dl_freq + err, err)
    }
}

#[derive(Deserialize)]
pub struct SoapySdrDto {
    pub rx_freq: f64,
    pub tx_freq: f64,
    pub ppm_err: Option<f64>,

    pub device: Option<String>,

    pub rx_antenna: Option<String>,
    pub tx_antenna: Option<String>,

    pub sample_rate: Option<f64>,
    pub rx_channel: Option<usize>,
    pub tx_channel: Option<usize>,

    pub tx_calibration_enabled: Option<bool>,
    pub tx_calibration_file: Option<String>,
    pub tx_calibration_apply_dc: Option<bool>,
    pub tx_calibration_apply_iq: Option<bool>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TxCalibrationFile {
    pub schema_version: u32,
    pub status: String,
    pub created_unix_secs: u64,
    pub updated_unix_secs: u64,
    pub device: TxCalibrationDevice,
    pub limits: TxCalibrationLimits,
    pub reference: TxCalibrationPoint,
    pub calibrated: TxCalibrationPoint,
    pub applied: TxCalibrationCoefficients,
    pub report: TxCalibrationReport,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TxCalibrationDevice {
    pub name: String,
    pub tx_frequency_hz: f64,
    pub rx_frequency_hz: f64,
    pub tx_center_frequency_hz: f64,
    pub rx_center_frequency_hz: f64,
    pub calibration_frequency_hz: f64,
    pub duplex_shift_hz: f64,
    pub sample_rate_hz: f64,
    pub tx_channel: usize,
    pub rx_channel: usize,
    pub tx_antenna: String,
    pub rx_antenna: String,
    pub loopback_source: String,
    pub pa_setting: String,
    pub tx_gains_fingerprint: String,
    pub rx_gains_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TxCalibrationLimits {
    pub tx_dc_abs_max: f64,
    pub tx_iq_abs_max: f64,
    pub min_carrier_improvement_db: f64,
    pub min_image_improvement_db: f64,
}

impl Default for TxCalibrationLimits {
    fn default() -> Self {
        Self {
            tx_dc_abs_max: 0.08,
            tx_iq_abs_max: 0.25,
            min_carrier_improvement_db: 3.0,
            min_image_improvement_db: 3.0,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TxCalibrationPoint {
    pub label: String,
    pub tx: TxCalibrationCoefficients,
    pub carrier_leakage_dbc: f64,
    pub carrier_leakage_dbfs: f64,
    pub image_rejection_db: f64,
    pub evm_proxy_pct: f64,
    pub tetra_known_rms_evm_pct: Option<f64>,
    pub tetra_known_peak_evm_pct: Option<f64>,
    pub tetra_known_differential_rms_deg: Option<f64>,
    pub tetra_known_symbols_used: Option<usize>,
    pub tetra_known_timing_sample: Option<f64>,
    pub tetra_known_frequency_rotation_rad_per_symbol: Option<f64>,
    pub signal_dbfs: f64,
    pub noise_floor_dbfs: f64,
    pub floor_before_dbfs: f64,
    pub floor_after_dbfs: f64,
    pub loopback_floor_dbfs: f64,
    pub rx_baseline_dbfs: f64,
    pub floor_drift_db: f64,
    pub floor_drift_abs_db: f64,
    pub max_component_abs: f64,
    pub clipped_fraction: f64,
    pub snr_db: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TxCalibrationCoefficients {
    pub dc_i: f64,
    pub dc_q: f64,
    pub iq_i: f64,
    pub iq_q: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TxCalibrationReport {
    pub carrier_leakage_improvement_db: f64,
    pub image_rejection_improvement_db: f64,
    pub evm_proxy_improvement_pct: f64,
    pub tetra_known_rms_evm_improvement_pct: Option<f64>,
    pub tetra_known_peak_evm_improvement_pct: Option<f64>,
    pub tetra_known_evm_quality_ok: bool,
    pub tx_dc_actuator_step: Option<f64>,
    pub tx_dc_actuator_carrier_span_db: Option<f64>,
    pub tx_dc_actuator_min_carrier_dbc: Option<f64>,
    pub tx_dc_actuator_max_carrier_dbc: Option<f64>,
    pub tx_dc_actuator_estimate_step: Option<f64>,
    pub tx_dc_actuator_estimated_dc_i: Option<f64>,
    pub tx_dc_actuator_estimated_dc_q: Option<f64>,
    pub tx_dc_actuator_estimate_valid: bool,
    pub tx_dc_actuator_readback_ok: bool,
    pub tx_dc_actuator_effective: bool,
    pub rf_limiting_factor: String,
    pub accepted: bool,
    pub accepted_dc: bool,
    pub accepted_iq: bool,
    pub accepted_mode: String,
    pub dc_confirmed: bool,
    pub image_quality_ok: bool,
    pub final_quality_ok: bool,
    pub confirmation_passes: u32,
    pub confirmation_carrier_spread_db: f64,
    pub confirmation_signal_spread_db: f64,
    pub timing_warning: bool,
    pub timing_status_events: u32,
    pub timing_confirmation_status_events: u32,
    pub timing_underflows: u32,
    pub timing_overflows: u32,
    pub timing_time_errors: u32,
    pub timing_other_errors: u32,
    pub timing_last_status: String,
    pub summary: String,
}

pub fn read_tx_calibration_file(path: impl AsRef<Path>) -> Result<TxCalibrationFile, String> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    let mut file: TxCalibrationFile = toml::from_str(&text).map_err(|e| format!("parse {}: {}", path.display(), e))?;
    if file.schema_version == 0 {
        file.schema_version = 1;
    }
    validate_tx_calibration_file(&file)?;
    Ok(file)
}

pub fn write_tx_calibration_file_atomic(path: impl AsRef<Path>, file: &TxCalibrationFile) -> Result<(), String> {
    let path = path.as_ref();
    let text = toml::to_string_pretty(file).map_err(|e| format!("serialize calibration: {}", e))?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {}", parent.display(), e))?;
        }
    }
    let tmp_path = path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, text).map_err(|e| format!("write {}: {}", tmp_path.display(), e))?;
    std::fs::rename(&tmp_path, path).map_err(|e| format!("rename {} -> {}: {}", tmp_path.display(), path.display(), e))?;
    Ok(())
}

pub fn tx_calibration_run_report_path(path: impl AsRef<Path>) -> PathBuf {
    path.as_ref().with_extension("run.toml")
}

pub fn tx_calibration_rejected_report_path(path: impl AsRef<Path>) -> PathBuf {
    path.as_ref().with_extension("rejected.toml")
}

pub fn write_tx_calibration_result_file_atomic(path: impl AsRef<Path>, file: &TxCalibrationFile) -> Result<PathBuf, String> {
    let path = path.as_ref();
    let run_path = tx_calibration_run_report_path(path);
    write_tx_calibration_file_atomic(&run_path, file)?;

    if file.report.accepted {
        write_tx_calibration_file_atomic(path, file)?;
        Ok(path.to_path_buf())
    } else {
        let rejected_path = tx_calibration_rejected_report_path(path);
        write_tx_calibration_file_atomic(&rejected_path, file)?;
        Ok(run_path)
    }
}

pub fn validate_tx_calibration_file(file: &TxCalibrationFile) -> Result<(), String> {
    if file.schema_version != 1 {
        return Err(format!("unsupported calibration schema_version {}", file.schema_version));
    }
    validate_coefficients("applied", file.applied, &file.limits)?;
    validate_coefficients("reference.tx", file.reference.tx, &file.limits)?;
    validate_coefficients("calibrated.tx", file.calibrated.tx, &file.limits)?;
    for (name, value) in [
        ("reference.carrier_leakage_dbc", file.reference.carrier_leakage_dbc),
        ("reference.carrier_leakage_dbfs", file.reference.carrier_leakage_dbfs),
        ("reference.image_rejection_db", file.reference.image_rejection_db),
        ("reference.evm_proxy_pct", file.reference.evm_proxy_pct),
        ("reference.signal_dbfs", file.reference.signal_dbfs),
        ("reference.noise_floor_dbfs", file.reference.noise_floor_dbfs),
        ("reference.floor_before_dbfs", file.reference.floor_before_dbfs),
        ("reference.floor_after_dbfs", file.reference.floor_after_dbfs),
        ("reference.loopback_floor_dbfs", file.reference.loopback_floor_dbfs),
        ("reference.rx_baseline_dbfs", file.reference.rx_baseline_dbfs),
        ("reference.floor_drift_db", file.reference.floor_drift_db),
        ("reference.floor_drift_abs_db", file.reference.floor_drift_abs_db),
        ("reference.max_component_abs", file.reference.max_component_abs),
        ("reference.clipped_fraction", file.reference.clipped_fraction),
        ("reference.snr_db", file.reference.snr_db),
        ("calibrated.carrier_leakage_dbc", file.calibrated.carrier_leakage_dbc),
        ("calibrated.carrier_leakage_dbfs", file.calibrated.carrier_leakage_dbfs),
        ("calibrated.image_rejection_db", file.calibrated.image_rejection_db),
        ("calibrated.evm_proxy_pct", file.calibrated.evm_proxy_pct),
        ("calibrated.signal_dbfs", file.calibrated.signal_dbfs),
        ("calibrated.noise_floor_dbfs", file.calibrated.noise_floor_dbfs),
        ("calibrated.floor_before_dbfs", file.calibrated.floor_before_dbfs),
        ("calibrated.floor_after_dbfs", file.calibrated.floor_after_dbfs),
        ("calibrated.loopback_floor_dbfs", file.calibrated.loopback_floor_dbfs),
        ("calibrated.rx_baseline_dbfs", file.calibrated.rx_baseline_dbfs),
        ("calibrated.floor_drift_db", file.calibrated.floor_drift_db),
        ("calibrated.floor_drift_abs_db", file.calibrated.floor_drift_abs_db),
        ("calibrated.max_component_abs", file.calibrated.max_component_abs),
        ("calibrated.clipped_fraction", file.calibrated.clipped_fraction),
        ("calibrated.snr_db", file.calibrated.snr_db),
        ("report.carrier_leakage_improvement_db", file.report.carrier_leakage_improvement_db),
        ("report.image_rejection_improvement_db", file.report.image_rejection_improvement_db),
        ("report.evm_proxy_improvement_pct", file.report.evm_proxy_improvement_pct),
        ("report.confirmation_carrier_spread_db", file.report.confirmation_carrier_spread_db),
        ("report.confirmation_signal_spread_db", file.report.confirmation_signal_spread_db),
    ] {
        if !value.is_finite() {
            return Err(format!("{name} must be finite"));
        }
    }
    for (name, value) in [
        ("reference.tetra_known_rms_evm_pct", file.reference.tetra_known_rms_evm_pct),
        ("reference.tetra_known_peak_evm_pct", file.reference.tetra_known_peak_evm_pct),
        (
            "reference.tetra_known_differential_rms_deg",
            file.reference.tetra_known_differential_rms_deg,
        ),
        ("reference.tetra_known_timing_sample", file.reference.tetra_known_timing_sample),
        (
            "reference.tetra_known_frequency_rotation_rad_per_symbol",
            file.reference.tetra_known_frequency_rotation_rad_per_symbol,
        ),
        ("calibrated.tetra_known_rms_evm_pct", file.calibrated.tetra_known_rms_evm_pct),
        ("calibrated.tetra_known_peak_evm_pct", file.calibrated.tetra_known_peak_evm_pct),
        (
            "calibrated.tetra_known_differential_rms_deg",
            file.calibrated.tetra_known_differential_rms_deg,
        ),
        ("calibrated.tetra_known_timing_sample", file.calibrated.tetra_known_timing_sample),
        (
            "calibrated.tetra_known_frequency_rotation_rad_per_symbol",
            file.calibrated.tetra_known_frequency_rotation_rad_per_symbol,
        ),
        (
            "report.tetra_known_rms_evm_improvement_pct",
            file.report.tetra_known_rms_evm_improvement_pct,
        ),
        (
            "report.tetra_known_peak_evm_improvement_pct",
            file.report.tetra_known_peak_evm_improvement_pct,
        ),
        ("report.tx_dc_actuator_step", file.report.tx_dc_actuator_step),
        ("report.tx_dc_actuator_carrier_span_db", file.report.tx_dc_actuator_carrier_span_db),
        ("report.tx_dc_actuator_min_carrier_dbc", file.report.tx_dc_actuator_min_carrier_dbc),
        ("report.tx_dc_actuator_max_carrier_dbc", file.report.tx_dc_actuator_max_carrier_dbc),
        ("report.tx_dc_actuator_estimate_step", file.report.tx_dc_actuator_estimate_step),
        ("report.tx_dc_actuator_estimated_dc_i", file.report.tx_dc_actuator_estimated_dc_i),
        ("report.tx_dc_actuator_estimated_dc_q", file.report.tx_dc_actuator_estimated_dc_q),
    ] {
        if let Some(value) = value {
            if !value.is_finite() {
                return Err(format!("{name} must be finite"));
            }
        }
    }
    Ok(())
}

fn validate_coefficients(name: &str, coeffs: TxCalibrationCoefficients, limits: &TxCalibrationLimits) -> Result<(), String> {
    let dc_limit = limits.tx_dc_abs_max.max(0.0);
    let iq_limit = limits.tx_iq_abs_max.max(0.0);
    for (field, value, limit) in [
        ("dc_i", coeffs.dc_i, dc_limit),
        ("dc_q", coeffs.dc_q, dc_limit),
        ("iq_i", coeffs.iq_i, iq_limit),
        ("iq_q", coeffs.iq_q, iq_limit),
    ] {
        if !value.is_finite() {
            return Err(format!("{name}.{field} must be finite"));
        }
        if value.abs() > limit {
            return Err(format!("{name}.{field}={} exceeds limit {}", value, limit));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_calibration() -> TxCalibrationFile {
        TxCalibrationFile {
            schema_version: 1,
            status: "calibrated".to_string(),
            device: TxCalibrationDevice {
                name: "SXceiver".to_string(),
                tx_frequency_hz: 438_362_500.0,
                rx_frequency_hz: 431_362_500.0,
                tx_center_frequency_hz: 438_362_500.0,
                rx_center_frequency_hz: 431_342_500.0,
                calibration_frequency_hz: 438_362_500.0,
                duplex_shift_hz: 7_000_000.0,
                tx_antenna: "TX".to_string(),
                rx_antenna: "RX".to_string(),
                loopback_source: "rx_internal_lb".to_string(),
                pa_setting: "AUTO".to_string(),
                tx_gains_fingerprint: "DAC=9.00,MIXER=30.00".to_string(),
                rx_gains_fingerprint: "LNA=42.00,PGA=16.00".to_string(),
                ..Default::default()
            },
            limits: TxCalibrationLimits::default(),
            reference: TxCalibrationPoint {
                label: "neutral".to_string(),
                tx: TxCalibrationCoefficients::default(),
                carrier_leakage_dbc: -30.0,
                carrier_leakage_dbfs: -50.0,
                image_rejection_db: 25.0,
                evm_proxy_pct: 4.5,
                tetra_known_rms_evm_pct: Some(8.2),
                tetra_known_peak_evm_pct: Some(22.0),
                tetra_known_differential_rms_deg: Some(3.4),
                tetra_known_symbols_used: Some(192),
                tetra_known_timing_sample: Some(47.5),
                tetra_known_frequency_rotation_rad_per_symbol: Some(0.002),
                signal_dbfs: -20.0,
                noise_floor_dbfs: -75.0,
                floor_before_dbfs: -81.0,
                floor_after_dbfs: -80.8,
                loopback_floor_dbfs: -80.0,
                rx_baseline_dbfs: -65.0,
                floor_drift_db: 0.2,
                floor_drift_abs_db: 0.2,
                max_component_abs: 0.20,
                clipped_fraction: 0.0,
                snr_db: 55.0,
            },
            calibrated: TxCalibrationPoint {
                label: "calibrated".to_string(),
                tx: TxCalibrationCoefficients {
                    dc_i: 0.01,
                    dc_q: -0.02,
                    iq_i: 0.03,
                    iq_q: -0.04,
                },
                carrier_leakage_dbc: -48.0,
                carrier_leakage_dbfs: -68.0,
                image_rejection_db: 41.0,
                evm_proxy_pct: 1.2,
                tetra_known_rms_evm_pct: Some(4.8),
                tetra_known_peak_evm_pct: Some(15.5),
                tetra_known_differential_rms_deg: Some(1.6),
                tetra_known_symbols_used: Some(192),
                tetra_known_timing_sample: Some(48.0),
                tetra_known_frequency_rotation_rad_per_symbol: Some(0.001),
                signal_dbfs: -20.0,
                noise_floor_dbfs: -76.0,
                floor_before_dbfs: -80.9,
                floor_after_dbfs: -81.0,
                loopback_floor_dbfs: -81.0,
                rx_baseline_dbfs: -65.0,
                floor_drift_db: -0.1,
                floor_drift_abs_db: 0.1,
                max_component_abs: 0.18,
                clipped_fraction: 0.0,
                snr_db: 56.0,
            },
            applied: TxCalibrationCoefficients {
                dc_i: 0.01,
                dc_q: -0.02,
                iq_i: 0.03,
                iq_q: -0.04,
            },
            report: TxCalibrationReport {
                carrier_leakage_improvement_db: 18.0,
                image_rejection_improvement_db: 16.0,
                evm_proxy_improvement_pct: 3.3,
                tetra_known_rms_evm_improvement_pct: Some(3.4),
                tetra_known_peak_evm_improvement_pct: Some(6.5),
                tetra_known_evm_quality_ok: true,
                tx_dc_actuator_step: Some(0.04),
                tx_dc_actuator_carrier_span_db: Some(20.0),
                tx_dc_actuator_min_carrier_dbc: Some(-48.0),
                tx_dc_actuator_max_carrier_dbc: Some(-28.0),
                tx_dc_actuator_estimate_step: Some(0.001),
                tx_dc_actuator_estimated_dc_i: Some(0.001),
                tx_dc_actuator_estimated_dc_q: Some(-0.002),
                tx_dc_actuator_estimate_valid: true,
                tx_dc_actuator_readback_ok: true,
                tx_dc_actuator_effective: true,
                rf_limiting_factor: "within_known_evm_gate".to_string(),
                accepted: true,
                accepted_dc: true,
                accepted_iq: true,
                accepted_mode: "dc_iq".to_string(),
                dc_confirmed: true,
                image_quality_ok: true,
                final_quality_ok: true,
                confirmation_passes: 4,
                confirmation_carrier_spread_db: 0.3,
                confirmation_signal_spread_db: 0.1,
                timing_warning: false,
                timing_status_events: 0,
                timing_confirmation_status_events: 0,
                timing_underflows: 0,
                timing_overflows: 0,
                timing_time_errors: 0,
                timing_other_errors: 0,
                timing_last_status: String::new(),
                summary: "accepted".to_string(),
            },
            ..Default::default()
        }
    }

    #[test]
    fn tx_calibration_file_roundtrips_and_validates() {
        let file = valid_calibration();
        validate_tx_calibration_file(&file).expect("valid calibration");

        let path = std::env::temp_dir().join(format!("nexus-bs-calibration-test-{}.toml", std::process::id()));
        write_tx_calibration_file_atomic(&path, &file).expect("write calibration");
        let parsed = read_tx_calibration_file(&path).expect("read calibration");
        let _ = std::fs::remove_file(&path);

        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.reference.label, "neutral");
        assert_eq!(parsed.applied.dc_i, 0.01);
        assert_eq!(parsed.device.duplex_shift_hz, 7_000_000.0);
        assert_eq!(parsed.device.loopback_source, "rx_internal_lb");
        assert_eq!(parsed.device.rx_gains_fingerprint, "LNA=42.00,PGA=16.00");
        assert_eq!(parsed.reference.tetra_known_symbols_used, Some(192));
        assert_eq!(parsed.report.tetra_known_rms_evm_improvement_pct, Some(3.4));
        assert_eq!(parsed.report.tx_dc_actuator_carrier_span_db, Some(20.0));
        assert_eq!(parsed.report.tx_dc_actuator_estimate_step, Some(0.001));
        assert_eq!(parsed.report.tx_dc_actuator_estimated_dc_i, Some(0.001));
        assert!(parsed.report.tx_dc_actuator_estimate_valid);
        assert!(parsed.report.tx_dc_actuator_readback_ok);
        assert_eq!(parsed.report.rf_limiting_factor, "within_known_evm_gate");
        assert!(parsed.report.accepted);
    }

    #[test]
    fn tx_calibration_rejects_coefficients_outside_limits() {
        let mut file = valid_calibration();
        file.applied.dc_i = 0.50;
        let err = validate_tx_calibration_file(&file).expect_err("must reject unsafe DC coefficient");
        assert!(err.contains("applied.dc_i"));
    }

    #[test]
    fn tx_calibration_result_preserves_primary_file_when_run_is_rejected() {
        let accepted = valid_calibration();
        let mut rejected = valid_calibration();
        rejected.status = "rejected".to_string();
        rejected.applied = TxCalibrationCoefficients::default();
        rejected.report.accepted = false;
        rejected.report.accepted_dc = false;
        rejected.report.accepted_iq = false;
        rejected.report.accepted_mode = "rejected".to_string();
        rejected.report.summary = "rejected test run".to_string();

        let path = std::env::temp_dir().join(format!("nexus-bs-calibration-result-test-{}.toml", std::process::id()));
        let run_path = tx_calibration_run_report_path(&path);
        let rejected_path = tx_calibration_rejected_report_path(&path);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&run_path);
        let _ = std::fs::remove_file(&rejected_path);

        write_tx_calibration_file_atomic(&path, &accepted).expect("write accepted primary");
        let written_path = write_tx_calibration_result_file_atomic(&path, &rejected).expect("write rejected run");

        assert_eq!(written_path, run_path);
        let primary = read_tx_calibration_file(&path).expect("primary calibration remains readable");
        let run = read_tx_calibration_file(&run_path).expect("run report is readable");
        let rejected_copy = read_tx_calibration_file(&rejected_path).expect("rejected report is readable");

        assert!(primary.report.accepted);
        assert_eq!(primary.applied.dc_i, accepted.applied.dc_i);
        assert!(!run.report.accepted);
        assert!(!rejected_copy.report.accepted);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&run_path);
        let _ = std::fs::remove_file(&rejected_path);
    }
}
