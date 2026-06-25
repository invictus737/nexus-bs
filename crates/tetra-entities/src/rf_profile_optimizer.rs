// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0

use serde::{Deserialize, Serialize};
use tetra_config::bluestation::TxCalibrationFile;

const CLEAN_EVM_PCT: f32 = 7.0;
const CRITICAL_EVM_PCT: f32 = 10.0;
const PA_CLEAN_EVM_PCT: f32 = 5.0;
const PA_MAX_EVM_PCT: f32 = 7.0;
const CARRIER_LEAKAGE_WARN_DB: f32 = -35.0;
const FREQUENCY_ERROR_WARN_HZ: f32 = 250.0;
const RECENT_TIMING_ANOMALY_MS: u64 = 15_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfDeploymentProfile {
    Hotspot,
    LowPowerBasestation,
    PowerAmplifiedBasestation,
}

impl RfDeploymentProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            RfDeploymentProfile::Hotspot => "hotspot",
            RfDeploymentProfile::LowPowerBasestation => "low_power_basestation",
            RfDeploymentProfile::PowerAmplifiedBasestation => "power_amplified_basestation",
        }
    }

    pub fn from_tx_gain_profile(profile: &str) -> Self {
        match profile {
            "low_drive_calibration" => RfDeploymentProfile::Hotspot,
            "pa_drive_linear" | "max_test_only" => RfDeploymentProfile::PowerAmplifiedBasestation,
            _ => RfDeploymentProfile::LowPowerBasestation,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "hotspot" => Some(RfDeploymentProfile::Hotspot),
            "low_power_basestation" | "low_power" => Some(RfDeploymentProfile::LowPowerBasestation),
            "power_amplified_basestation" | "pa" | "pa_basestation" => Some(RfDeploymentProfile::PowerAmplifiedBasestation),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfOptimizationSeverity {
    InsufficientMeasurement,
    Unknown,
    Ok,
    Tune,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfProfileValidationStatus {
    Unmeasured,
    PendingRetest,
    Validated,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfOptimizationActionKind {
    CollectRfLoopbackMeasurement,
    KeepCurrentProfile,
    UseLowDriveCalibration,
    UseNominalClean,
    UsePaDriveLinear,
    RefusePaDrive,
    LowerTxDrive,
    LowerRxGain,
    ImproveAntennaIsolation,
    AddRxTxFiltering,
    RunTxDcIqCalibration,
    CheckReferenceClock,
    FixSdrTiming,
    ApplyProfileAndRetest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RfOptimizationAction {
    pub kind: RfOptimizationActionKind,
    pub message: String,
}

impl RfOptimizationAction {
    fn new(kind: RfOptimizationActionKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RfQualityMeasurement {
    pub tx_gain_profile: String,
    pub measured_sources: Vec<String>,
    pub calibration_report_accepted: Option<bool>,
    pub dsp_evm_pct: Option<f32>,
    pub rf_known_rms_evm_pct: Option<f32>,
    pub rf_known_peak_evm_pct: Option<f32>,
    pub rf_evm_proxy_pct: Option<f32>,
    pub papr_db: Option<f32>,
    pub carrier_leakage_db: Option<f32>,
    pub image_rejection_db: Option<f32>,
    pub snr_db: Option<f32>,
    pub frequency_error_hz: Option<f32>,
    pub rf_timing_severity: Option<String>,
    pub tx_late_events: u64,
    pub rx_lost_events: u64,
    pub last_timing_anomaly_age_ms: Option<u64>,
    pub rx_overload_hint: bool,
    pub near_field_coupling_hint: bool,
}

impl RfQualityMeasurement {
    pub fn from_live_tx_quality(
        tx_gain_profile: impl Into<String>,
        dsp_evm_pct: f32,
        papr_db: f32,
        carrier_leakage_db: f32,
        frequency_error_hz: f32,
        rf_timing_severity: impl Into<String>,
        tx_late_events: u64,
        rx_lost_events: u64,
        last_timing_anomaly_age_ms: Option<u64>,
    ) -> Self {
        let rf_timing_severity = rf_timing_severity.into();
        Self {
            tx_gain_profile: tx_gain_profile.into(),
            measured_sources: vec!["live_tx_quality_dsp".to_string()],
            calibration_report_accepted: None,
            dsp_evm_pct: Some(dsp_evm_pct),
            rf_known_rms_evm_pct: None,
            rf_known_peak_evm_pct: None,
            rf_evm_proxy_pct: None,
            papr_db: Some(papr_db),
            carrier_leakage_db: Some(carrier_leakage_db),
            image_rejection_db: None,
            snr_db: None,
            frequency_error_hz: Some(frequency_error_hz),
            rf_timing_severity: if rf_timing_severity.is_empty() {
                None
            } else {
                Some(rf_timing_severity)
            },
            tx_late_events,
            rx_lost_events,
            last_timing_anomaly_age_ms,
            rx_overload_hint: false,
            near_field_coupling_hint: false,
        }
    }

    pub fn with_tx_calibration_report(mut self, calibration: &TxCalibrationFile) -> Self {
        self.measured_sources.push("tx_calibration_rf_loopback".to_string());
        if calibration.calibrated.tetra_known_rms_evm_pct.is_some() {
            self.measured_sources.push("tx_calibration_known_tetra_evm".to_string());
        }
        self.calibration_report_accepted = Some(calibration.report.accepted);
        self.rf_known_rms_evm_pct = calibration.calibrated.tetra_known_rms_evm_pct.map(|v| v as f32);
        self.rf_known_peak_evm_pct = calibration.calibrated.tetra_known_peak_evm_pct.map(|v| v as f32);
        self.rf_evm_proxy_pct = Some(calibration.calibrated.evm_proxy_pct as f32);
        self.carrier_leakage_db = Some(calibration.calibrated.carrier_leakage_dbc as f32);
        self.image_rejection_db = Some(calibration.calibrated.image_rejection_db as f32);
        self.snr_db = Some(calibration.calibrated.snr_db as f32);
        if calibration.report.timing_warning {
            self.rf_timing_severity = Some("degraded".to_string());
        }
        self
    }

    pub fn has_concrete_rf_evm(&self) -> bool {
        self.rf_known_rms_evm_pct.is_some_and(|evm| evm.is_finite() && evm > 0.0)
    }

    fn effective_evm_pct(&self) -> Option<f32> {
        self.rf_known_rms_evm_pct
    }

    fn has_timing_fault(&self) -> bool {
        let severity_fault = matches!(self.rf_timing_severity.as_deref(), Some("degraded") | Some("critical"));
        if !severity_fault {
            return false;
        }
        self.last_timing_anomaly_age_ms
            .map(|age_ms| age_ms <= RECENT_TIMING_ANOMALY_MS)
            .unwrap_or(true)
    }

    fn has_dc_iq_fault(&self) -> bool {
        self.carrier_leakage_db.is_some_and(|db| db > CARRIER_LEAKAGE_WARN_DB)
    }

    fn has_frequency_fault(&self) -> bool {
        self.frequency_error_hz.is_some_and(|hz| hz.abs() > FREQUENCY_ERROR_WARN_HZ)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RfProfileRecommendation {
    pub target_profile: RfDeploymentProfile,
    pub current_tx_gain_profile: String,
    pub recommended_tx_gain_profile: String,
    pub severity: RfOptimizationSeverity,
    pub measurement_valid: bool,
    pub profile_validation_status: RfProfileValidationStatus,
    pub validated_profile: Option<RfDeploymentProfile>,
    pub calibration_report_accepted: Option<bool>,
    pub measurement_sources: Vec<String>,
    pub evm_target_pct: f32,
    pub safe_auto_apply: bool,
    pub tx_drive_backoff_db: Option<f32>,
    pub rx_gain_backoff_db: Option<f32>,
    pub requires_rf_isolation: bool,
    pub requires_filtering_or_duplexer: bool,
    pub actions: Vec<RfOptimizationAction>,
}

impl RfProfileRecommendation {
    pub fn summary(&self) -> &str {
        self.actions
            .first()
            .map(|action| action.message.as_str())
            .unwrap_or("waiting for RF quality measurement")
    }

    fn push(&mut self, kind: RfOptimizationActionKind, message: impl Into<String>) {
        self.actions.push(RfOptimizationAction::new(kind, message));
    }

    fn raise(&mut self, severity: RfOptimizationSeverity) {
        self.severity = match (self.severity, severity) {
            (RfOptimizationSeverity::InsufficientMeasurement, _) => severity,
            (_, RfOptimizationSeverity::InsufficientMeasurement) => RfOptimizationSeverity::InsufficientMeasurement,
            (RfOptimizationSeverity::Critical, _) | (_, RfOptimizationSeverity::Critical) => RfOptimizationSeverity::Critical,
            (RfOptimizationSeverity::Tune, _) | (_, RfOptimizationSeverity::Tune) => RfOptimizationSeverity::Tune,
            (RfOptimizationSeverity::Ok, RfOptimizationSeverity::Unknown) => RfOptimizationSeverity::Ok,
            (_, next) => next,
        };
    }
}

pub fn recommend_rf_profile_adjustment(target_profile: RfDeploymentProfile, measurement: &RfQualityMeasurement) -> RfProfileRecommendation {
    let evm_target_pct = match target_profile {
        RfDeploymentProfile::PowerAmplifiedBasestation => PA_CLEAN_EVM_PCT,
        RfDeploymentProfile::Hotspot => CLEAN_EVM_PCT,
        RfDeploymentProfile::LowPowerBasestation => CLEAN_EVM_PCT,
    };
    let mut recommendation = RfProfileRecommendation {
        target_profile,
        current_tx_gain_profile: measurement.tx_gain_profile.clone(),
        recommended_tx_gain_profile: measurement.tx_gain_profile.clone(),
        severity: RfOptimizationSeverity::Ok,
        measurement_valid: measurement.has_concrete_rf_evm(),
        profile_validation_status: RfProfileValidationStatus::Unmeasured,
        validated_profile: None,
        calibration_report_accepted: measurement.calibration_report_accepted,
        measurement_sources: measurement.measured_sources.clone(),
        evm_target_pct,
        safe_auto_apply: false,
        tx_drive_backoff_db: None,
        rx_gain_backoff_db: None,
        requires_rf_isolation: false,
        requires_filtering_or_duplexer: false,
        actions: Vec::new(),
    };

    if !measurement.has_concrete_rf_evm() {
        recommendation.severity = RfOptimizationSeverity::InsufficientMeasurement;
        recommendation.measurement_valid = false;
        recommendation.safe_auto_apply = false;
        recommendation.push(
            RfOptimizationActionKind::CollectRfLoopbackMeasurement,
            "run RF loopback/known-symbol EVM measurement before profile optimization",
        );
        return recommendation;
    }

    let Some(evm_pct) = measurement.effective_evm_pct() else {
        recommendation.severity = RfOptimizationSeverity::InsufficientMeasurement;
        recommendation.measurement_valid = false;
        recommendation.push(
            RfOptimizationActionKind::CollectRfLoopbackMeasurement,
            "waiting for measured RF EVM before changing RF profile",
        );
        return recommendation;
    };

    let measured_profile = RfDeploymentProfile::from_tx_gain_profile(&measurement.tx_gain_profile);
    if measured_profile != target_profile {
        recommendation.profile_validation_status = RfProfileValidationStatus::PendingRetest;
        recommendation.recommended_tx_gain_profile = default_tx_gain_profile_for_target(target_profile).to_string();
        match target_profile {
            RfDeploymentProfile::PowerAmplifiedBasestation => {
                recommendation.raise(RfOptimizationSeverity::Tune);
                recommendation.safe_auto_apply = false;
                recommendation.push(
                    RfOptimizationActionKind::RefusePaDrive,
                    "PA profile is not validated on this run; measure pa_drive_linear under controlled PA load before enabling it",
                );
            }
            RfDeploymentProfile::Hotspot | RfDeploymentProfile::LowPowerBasestation => {
                recommendation.safe_auto_apply = recommendation.recommended_tx_gain_profile != measurement.tx_gain_profile
                    && !measurement.has_timing_fault()
                    && !measurement.has_frequency_fault();
                recommendation.push(
                    RfOptimizationActionKind::ApplyProfileAndRetest,
                    format!(
                        "apply {} and run RF loopback/known-symbol EVM retest before marking {} validated",
                        recommendation.recommended_tx_gain_profile,
                        target_profile.as_str()
                    ),
                );
                recommendation.push(
                    RfOptimizationActionKind::CollectRfLoopbackMeasurement,
                    "current EVM is real, but it was measured on a different TX profile",
                );
            }
        }
        return recommendation;
    }
    recommendation.profile_validation_status = if evm_pct <= recommendation.evm_target_pct && !measurement.has_timing_fault() {
        recommendation.validated_profile = Some(target_profile);
        RfProfileValidationStatus::Validated
    } else {
        RfProfileValidationStatus::Failed
    };

    if measurement.has_timing_fault() {
        recommendation.raise(RfOptimizationSeverity::Critical);
        recommendation.push(
            RfOptimizationActionKind::FixSdrTiming,
            "fix SDR timing first: late TX/RX lost events invalidate gain optimization",
        );
    }
    if measurement.has_frequency_fault() {
        recommendation.raise(RfOptimizationSeverity::Tune);
        recommendation.push(
            RfOptimizationActionKind::CheckReferenceClock,
            "check PPM/reference clock before trusting EVM trend",
        );
    }
    if measurement.has_dc_iq_fault() {
        recommendation.raise(RfOptimizationSeverity::Tune);
        recommendation.push(
            RfOptimizationActionKind::RunTxDcIqCalibration,
            "run TX DC/IQ calibration: carrier leakage is too high",
        );
    }

    match target_profile {
        RfDeploymentProfile::Hotspot => tune_hotspot(evm_pct, measurement, &mut recommendation),
        RfDeploymentProfile::LowPowerBasestation => tune_low_power(evm_pct, measurement, &mut recommendation),
        RfDeploymentProfile::PowerAmplifiedBasestation => tune_pa(evm_pct, measurement, &mut recommendation),
    }

    if recommendation.actions.is_empty() {
        recommendation.push(
            RfOptimizationActionKind::KeepCurrentProfile,
            "RF profile is within the current EVM target",
        );
    }
    recommendation
}

fn default_tx_gain_profile_for_target(target_profile: RfDeploymentProfile) -> &'static str {
    match target_profile {
        RfDeploymentProfile::Hotspot => "low_drive_calibration",
        RfDeploymentProfile::LowPowerBasestation => "nominal_clean",
        RfDeploymentProfile::PowerAmplifiedBasestation => "pa_drive_linear",
    }
}

fn tune_hotspot(evm_pct: f32, measurement: &RfQualityMeasurement, recommendation: &mut RfProfileRecommendation) {
    recommendation.recommended_tx_gain_profile = "low_drive_calibration".to_string();
    if evm_pct >= CRITICAL_EVM_PCT || measurement.rx_overload_hint || measurement.near_field_coupling_hint {
        recommendation.raise(RfOptimizationSeverity::Critical);
        recommendation.tx_drive_backoff_db = Some(6.0);
        recommendation.rx_gain_backoff_db = Some(6.0);
        recommendation.requires_rf_isolation = true;
        recommendation.requires_filtering_or_duplexer = true;
        recommendation.push(
            RfOptimizationActionKind::UseLowDriveCalibration,
            "switch hotspot to low_drive_calibration before further testing",
        );
        recommendation.push(
            RfOptimizationActionKind::LowerTxDrive,
            "reduce TX drive by about 6 dB and retest EVM",
        );
        recommendation.push(
            RfOptimizationActionKind::LowerRxGain,
            "reduce RX gain by about 6 dB to avoid local TX desensing",
        );
        recommendation.push(
            RfOptimizationActionKind::ImproveAntennaIsolation,
            "increase RX/TX antenna isolation; close antennas can dominate 12% EVM",
        );
        recommendation.push(
            RfOptimizationActionKind::AddRxTxFiltering,
            "add RX/TX filtering or a duplexer before raising drive",
        );
    } else if evm_pct > CLEAN_EVM_PCT {
        recommendation.raise(RfOptimizationSeverity::Tune);
        recommendation.tx_drive_backoff_db = Some(3.0);
        recommendation.rx_gain_backoff_db = Some(3.0);
        recommendation.safe_auto_apply = true;
        recommendation.push(
            RfOptimizationActionKind::LowerTxDrive,
            "lower hotspot TX drive by about 3 dB and retest",
        );
        recommendation.push(
            RfOptimizationActionKind::LowerRxGain,
            "lower hotspot RX gain by about 3 dB if uplink RSSI allows it",
        );
    } else {
        recommendation.safe_auto_apply = measurement.tx_gain_profile != "low_drive_calibration";
        recommendation.push(
            RfOptimizationActionKind::UseLowDriveCalibration,
            "hotspot is clean enough; keep low_drive_calibration for margin",
        );
    }
}

fn tune_low_power(evm_pct: f32, _measurement: &RfQualityMeasurement, recommendation: &mut RfProfileRecommendation) {
    recommendation.recommended_tx_gain_profile = "nominal_clean".to_string();
    if evm_pct >= CRITICAL_EVM_PCT {
        recommendation.raise(RfOptimizationSeverity::Critical);
        recommendation.recommended_tx_gain_profile = "low_drive_calibration".to_string();
        recommendation.tx_drive_backoff_db = Some(6.0);
        recommendation.push(
            RfOptimizationActionKind::UseLowDriveCalibration,
            "fall back to low_drive_calibration: EVM is above the critical gate",
        );
        recommendation.push(
            RfOptimizationActionKind::LowerTxDrive,
            "reduce TX drive by about 6 dB before repeating the profile test",
        );
    } else if evm_pct > CLEAN_EVM_PCT {
        recommendation.raise(RfOptimizationSeverity::Tune);
        recommendation.tx_drive_backoff_db = Some(3.0);
        recommendation.safe_auto_apply = true;
        recommendation.push(
            RfOptimizationActionKind::LowerTxDrive,
            "trim TX drive by about 3 dB and keep nominal_clean only if EVM falls",
        );
    } else {
        recommendation.safe_auto_apply = recommendation.current_tx_gain_profile != "nominal_clean";
        recommendation.push(
            RfOptimizationActionKind::UseNominalClean,
            "low-power base station is clean; nominal_clean is the preferred profile",
        );
    }
}

fn tune_pa(evm_pct: f32, measurement: &RfQualityMeasurement, recommendation: &mut RfProfileRecommendation) {
    recommendation.safe_auto_apply = false;
    if evm_pct > PA_MAX_EVM_PCT || measurement.has_timing_fault() || measurement.has_dc_iq_fault() {
        recommendation.raise(RfOptimizationSeverity::Critical);
        recommendation.recommended_tx_gain_profile = "nominal_clean".to_string();
        recommendation.tx_drive_backoff_db = Some(if evm_pct >= CRITICAL_EVM_PCT { 6.0 } else { 3.0 });
        recommendation.requires_rf_isolation = evm_pct >= CRITICAL_EVM_PCT;
        recommendation.push(
            RfOptimizationActionKind::RefusePaDrive,
            "PA drive refused until EVM, DC/IQ and SDR timing are clean",
        );
        recommendation.push(
            RfOptimizationActionKind::UseNominalClean,
            "return to nominal_clean before repeating the PA linearity test",
        );
        recommendation.push(RfOptimizationActionKind::LowerTxDrive, "back off TX drive before the next PA sweep");
    } else if evm_pct > PA_CLEAN_EVM_PCT {
        recommendation.raise(RfOptimizationSeverity::Tune);
        recommendation.recommended_tx_gain_profile = "nominal_clean".to_string();
        recommendation.tx_drive_backoff_db = Some(3.0);
        recommendation.push(
            RfOptimizationActionKind::RefusePaDrive,
            "hold PA drive: EVM must be at or below 5% before linear PA profiling",
        );
    } else {
        recommendation.recommended_tx_gain_profile = "pa_drive_linear".to_string();
        recommendation.push(
            RfOptimizationActionKind::UsePaDriveLinear,
            "pre-PA signal is clean enough for a controlled pa_drive_linear test",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RfDeploymentProfile, RfOptimizationActionKind, RfOptimizationSeverity, RfProfileValidationStatus, RfQualityMeasurement,
        recommend_rf_profile_adjustment,
    };

    fn measurement(profile: &str, evm_pct: f32) -> RfQualityMeasurement {
        RfQualityMeasurement {
            tx_gain_profile: profile.to_string(),
            measured_sources: vec!["test_rf_loopback".to_string()],
            calibration_report_accepted: Some(true),
            dsp_evm_pct: Some(evm_pct),
            rf_known_rms_evm_pct: Some(evm_pct),
            rf_known_peak_evm_pct: Some(evm_pct * 2.0),
            rf_evm_proxy_pct: Some(evm_pct),
            papr_db: Some(3.8),
            carrier_leakage_db: Some(-45.0),
            image_rejection_db: Some(45.0),
            snr_db: Some(42.0),
            frequency_error_hz: Some(20.0),
            rf_timing_severity: Some("ok".to_string()),
            tx_late_events: 0,
            rx_lost_events: 0,
            last_timing_anomaly_age_ms: None,
            rx_overload_hint: false,
            near_field_coupling_hint: false,
        }
    }

    fn has_action(recommendation: &super::RfProfileRecommendation, kind: RfOptimizationActionKind) -> bool {
        recommendation.actions.iter().any(|action| action.kind == kind)
    }

    #[test]
    fn hotspot_with_twelve_percent_evm_recommends_backoff_and_isolation() {
        let mut sample = measurement("low_drive_calibration", 12.0);
        sample.rx_overload_hint = true;
        sample.near_field_coupling_hint = true;

        let advice = recommend_rf_profile_adjustment(RfDeploymentProfile::Hotspot, &sample);

        assert_eq!(advice.severity, RfOptimizationSeverity::Critical);
        assert_eq!(advice.profile_validation_status, RfProfileValidationStatus::Failed);
        assert_eq!(advice.recommended_tx_gain_profile, "low_drive_calibration");
        assert_eq!(advice.tx_drive_backoff_db, Some(6.0));
        assert_eq!(advice.rx_gain_backoff_db, Some(6.0));
        assert!(advice.requires_rf_isolation);
        assert!(advice.requires_filtering_or_duplexer);
        assert!(has_action(&advice, RfOptimizationActionKind::ImproveAntennaIsolation));
        assert!(has_action(&advice, RfOptimizationActionKind::AddRxTxFiltering));
    }

    #[test]
    fn low_power_clean_signal_prefers_nominal_clean() {
        let sample = measurement("nominal_clean", 4.8);

        let advice = recommend_rf_profile_adjustment(RfDeploymentProfile::LowPowerBasestation, &sample);

        assert_eq!(advice.severity, RfOptimizationSeverity::Ok);
        assert_eq!(advice.profile_validation_status, RfProfileValidationStatus::Validated);
        assert_eq!(advice.validated_profile, Some(RfDeploymentProfile::LowPowerBasestation));
        assert_eq!(advice.recommended_tx_gain_profile, "nominal_clean");
        assert!(!advice.safe_auto_apply);
        assert!(has_action(&advice, RfOptimizationActionKind::UseNominalClean));
    }

    #[test]
    fn different_target_profile_requires_apply_and_retest() {
        let sample = measurement("low_drive_calibration", 4.8);

        let advice = recommend_rf_profile_adjustment(RfDeploymentProfile::LowPowerBasestation, &sample);

        assert_eq!(advice.profile_validation_status, RfProfileValidationStatus::PendingRetest);
        assert_eq!(advice.validated_profile, None);
        assert_eq!(advice.recommended_tx_gain_profile, "nominal_clean");
        assert!(advice.safe_auto_apply);
        assert!(has_action(&advice, RfOptimizationActionKind::ApplyProfileAndRetest));
        assert!(has_action(&advice, RfOptimizationActionKind::CollectRfLoopbackMeasurement));
    }

    #[test]
    fn pa_profile_refuses_drive_when_evm_or_timing_is_bad() {
        let mut sample = measurement("pa_drive_linear", 8.5);
        sample.rf_timing_severity = Some("critical".to_string());
        sample.tx_late_events = 1;
        sample.last_timing_anomaly_age_ms = Some(100);

        let advice = recommend_rf_profile_adjustment(RfDeploymentProfile::PowerAmplifiedBasestation, &sample);

        assert_eq!(advice.severity, RfOptimizationSeverity::Critical);
        assert_eq!(advice.recommended_tx_gain_profile, "nominal_clean");
        assert!(!advice.safe_auto_apply);
        assert!(has_action(&advice, RfOptimizationActionKind::FixSdrTiming));
        assert!(has_action(&advice, RfOptimizationActionKind::RefusePaDrive));
    }

    #[test]
    fn pa_clean_signal_allows_linear_drive_but_not_max_test_only() {
        let sample = measurement("pa_drive_linear", 4.2);

        let advice = recommend_rf_profile_adjustment(RfDeploymentProfile::PowerAmplifiedBasestation, &sample);

        assert_eq!(advice.severity, RfOptimizationSeverity::Ok);
        assert_eq!(advice.profile_validation_status, RfProfileValidationStatus::Validated);
        assert_eq!(advice.recommended_tx_gain_profile, "pa_drive_linear");
        assert!(!advice.safe_auto_apply);
        assert!(has_action(&advice, RfOptimizationActionKind::UsePaDriveLinear));
    }

    #[test]
    fn optimizer_refuses_to_recommend_without_rf_evm_measurement() {
        let sample = RfQualityMeasurement::from_live_tx_quality("nominal_clean", 12.0, 3.8, -45.0, 20.0, "ok", 0, 0, None);

        let advice = recommend_rf_profile_adjustment(RfDeploymentProfile::Hotspot, &sample);

        assert_eq!(advice.severity, RfOptimizationSeverity::InsufficientMeasurement);
        assert!(!advice.measurement_valid);
        assert!(!advice.safe_auto_apply);
        assert!(has_action(&advice, RfOptimizationActionKind::CollectRfLoopbackMeasurement));
    }

    #[test]
    fn rejected_calibration_report_still_counts_as_concrete_rf_evm_measurement() {
        let mut sample = measurement("nominal_clean", 3.9);
        sample.calibration_report_accepted = Some(false);

        let advice = recommend_rf_profile_adjustment(RfDeploymentProfile::LowPowerBasestation, &sample);

        assert_eq!(advice.severity, RfOptimizationSeverity::Ok);
        assert!(advice.measurement_valid);
        assert_eq!(advice.recommended_tx_gain_profile, "nominal_clean");
        assert!(!has_action(&advice, RfOptimizationActionKind::CollectRfLoopbackMeasurement));
    }

    #[test]
    fn stale_startup_timing_counter_does_not_block_rf_profile_advice() {
        let mut sample = measurement("nominal_clean", 3.9);
        sample.rf_timing_severity = Some("critical".to_string());
        sample.tx_late_events = 1;
        sample.last_timing_anomaly_age_ms = Some(super::RECENT_TIMING_ANOMALY_MS + 1);

        let advice = recommend_rf_profile_adjustment(RfDeploymentProfile::LowPowerBasestation, &sample);

        assert_eq!(advice.severity, RfOptimizationSeverity::Ok);
        assert!(!has_action(&advice, RfOptimizationActionKind::FixSdrTiming));
    }
}
