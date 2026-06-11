use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
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
    /// Apply persisted TX IQ balance correction.
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
    pub sample_rate_hz: f64,
    pub tx_channel: usize,
    pub rx_channel: usize,
    pub tx_antenna: String,
    pub rx_antenna: String,
    pub tx_gains_fingerprint: String,
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
            min_carrier_improvement_db: 1.0,
            min_image_improvement_db: 1.0,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TxCalibrationPoint {
    pub label: String,
    pub tx: TxCalibrationCoefficients,
    pub carrier_leakage_dbc: f64,
    pub image_rejection_db: f64,
    pub evm_proxy_pct: f64,
    pub signal_dbfs: f64,
    pub noise_floor_dbfs: f64,
    pub snr_db: f64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
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
    pub accepted: bool,
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

pub fn validate_tx_calibration_file(file: &TxCalibrationFile) -> Result<(), String> {
    if file.schema_version != 1 {
        return Err(format!("unsupported calibration schema_version {}", file.schema_version));
    }
    validate_coefficients("applied", file.applied, &file.limits)?;
    validate_coefficients("reference.tx", file.reference.tx, &file.limits)?;
    validate_coefficients("calibrated.tx", file.calibrated.tx, &file.limits)?;
    for (name, value) in [
        ("reference.carrier_leakage_dbc", file.reference.carrier_leakage_dbc),
        ("reference.image_rejection_db", file.reference.image_rejection_db),
        ("reference.evm_proxy_pct", file.reference.evm_proxy_pct),
        ("calibrated.carrier_leakage_dbc", file.calibrated.carrier_leakage_dbc),
        ("calibrated.image_rejection_db", file.calibrated.image_rejection_db),
        ("calibrated.evm_proxy_pct", file.calibrated.evm_proxy_pct),
    ] {
        if !value.is_finite() {
            return Err(format!("{name} must be finite"));
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
            limits: TxCalibrationLimits::default(),
            reference: TxCalibrationPoint {
                label: "neutral".to_string(),
                tx: TxCalibrationCoefficients::default(),
                carrier_leakage_dbc: -30.0,
                image_rejection_db: 25.0,
                evm_proxy_pct: 4.5,
                signal_dbfs: -20.0,
                noise_floor_dbfs: -75.0,
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
                image_rejection_db: 41.0,
                evm_proxy_pct: 1.2,
                signal_dbfs: -20.0,
                noise_floor_dbfs: -76.0,
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
                accepted: true,
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
        assert!(parsed.report.accepted);
    }

    #[test]
    fn tx_calibration_rejects_coefficients_outside_limits() {
        let mut file = valid_calibration();
        file.applied.dc_i = 0.50;
        let err = validate_tx_calibration_file(&file).expect_err("must reject unsafe DC coefficient");
        assert!(err.contains("applied.dc_i"));
    }
}
