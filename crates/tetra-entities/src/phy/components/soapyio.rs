use soapysdr;
use tetra_config::bluestation::{
    SharedConfig, StackMode,
    sec_phy_soapy::{
        CfgSoapySdr, TxCalibrationCoefficients, TxCalibrationDevice, TxCalibrationFile, TxCalibrationLimits, TxCalibrationPoint,
        TxCalibrationReport, read_tx_calibration_file, write_tx_calibration_file_atomic,
    },
};

use tetra_pdus::phy::traits::rxtx_dev::RxTxDevError;

use super::dsp_types::*;
use super::soapy_settings;
use super::soapy_settings::{SdrSettings, SupportedDevice};
use super::soapy_time::{ticks_to_time_ns, time_ns_to_ticks};

type StreamType = ComplexSample;
const SOAPY_FREQ_OFFSET: f64 = 20000.0;
const TX_CAL_TONE_HZ: f64 = 24_000.0;
const TX_CAL_TONE_AMPLITUDE: f32 = 0.20;
const TX_CAL_FALLBACK_BLOCK_SAMPLES: usize = 4096;
const TX_CAL_PREFILL_BLOCKS: usize = 8;
const TX_CAL_SETTLE_BLOCKS: usize = 3;
const TX_CAL_CAPTURE_BLOCKS: usize = 3;
const TX_CAL_MIN_SNR_DB: f64 = 25.0;
const TX_CAL_ACCEPT_MIN_SNR_DB: f64 = 35.0;
const TX_CAL_ACCEPT_CARRIER_DBC: f64 = -30.0;
const TX_CAL_GOOD_CARRIER_DBC: f64 = -35.0;
const TX_CAL_ACCEPT_IMAGE_REJECTION_DB: f64 = 35.0;
const TX_CAL_GOOD_IMAGE_REJECTION_DB: f64 = 40.0;
const TX_CAL_ACCEPT_MAX_EVM_PROXY_PCT: f64 = 10.0;
const TX_CAL_ACCEPT_MAX_EVM_WORSEN_PCT: f64 = 1.0;
const TX_CAL_ACCEPT_MIN_IMPROVEMENT_DB: f64 = 3.0;
const TX_CAL_ACCEPT_MAX_FLOOR_DRIFT_DB: f64 = 3.0;
const TX_CAL_MAX_COMPONENT_ABS: f64 = 0.85;
const TX_CAL_CLIP_LEVEL: f32 = 0.98;
const TX_CAL_MAX_CLIPPED_FRACTION: f64 = 0.001;
const TX_CAL_FREQ_MATCH_TOLERANCE_HZ: f64 = 50.0;
const TX_CAL_SAMPLE_RATE_MATCH_TOLERANCE_HZ: f64 = 1.0;
const SOAPY_SX_LOOPBACK_ANTENNA: &str = "LB";
const SOAPY_SX_PA_SETTING: &str = "PA";

pub struct RxResult {
    /// Number of samples read
    pub len: usize,
    /// Sample counter for the first sample read
    pub count: SampleCount,
}

pub struct SoapyIo {
    rx_ch: usize,
    tx_ch: usize,
    rx_fs: f64,
    tx_fs: f64,
    /// Timestamp for the first sample read from SDR.
    /// This is subtracted from all following timestamps,
    /// so that sample counter startsB210 from 0 even if timestamp does not.
    initial_time: Option<i64>,
    rx_next_count: SampleCount,
    prev_time_ns: i64,

    /// If false, timestamp of latest RX read is used to estimate
    /// current hardware time. This is used in case get_hardware_time
    /// is unacceptably slow or not supported.
    use_get_hardware_time: bool,

    dev: soapysdr::Device,
    /// Receive stream. None if receiving is disabled.
    rx: Option<soapysdr::RxStream<StreamType>>,
    /// Transmit stream. None if transmitting is disabled.
    tx: Option<soapysdr::TxStream<StreamType>>,
    temperature_sensor_reads_supported: bool,
    sdr_name: String,
    rx_ant: Option<String>,
    tx_ant: Option<String>,
    rx_gain: Vec<(String, f64)>,
    tx_gain: Vec<(String, f64)>,
    rx_args: Vec<(String, String)>,
    tx_args: Vec<(String, String)>,
    live_rx_carrier_hz: Option<f64>,
    live_tx_carrier_hz: Option<f64>,
}

/// Soapy/Lime timestamps can occasionally jitter by a single sample.
/// Treat tiny deltas as contiguous to avoid triggering large block realignments downstream.
const RX_TIMESTAMP_JITTER_TOLERANCE_SAMPLES: SampleCount = 1;

/// It is annoying to repeat error handling so do that in a macro.
/// ? could be used but then it could not print which SoapySDR call failed.
macro_rules! soapycheck {
    ($text:literal, $soapysdr_call:expr) => {
        match $soapysdr_call {
            Ok(ret) => ret,
            Err(err) => {
                tracing::error!("SoapySDR: Failed to {}: {}", $text, err);
                return Err(err);
            }
        }
    };
}

#[derive(Clone, Debug)]
struct TxCalibrationSession {
    live_rx_carrier_hz: f64,
    live_tx_carrier_hz: f64,
    live_rx_center_hz: f64,
    live_tx_center_hz: f64,
    calibration_center_hz: f64,
    rx_stream_existed_before: bool,
    tx_stream_existed_before: bool,
    rx_active_before: bool,
    tx_active_before: bool,
    original_rx_antenna: Option<String>,
    original_pa_setting: Option<String>,
    loopback_source: String,
}

#[derive(Clone, Debug)]
struct TxCalibrationRuntimeConfig {
    name: String,
    live_rx_carrier_hz: f64,
    live_tx_carrier_hz: f64,
    live_rx_center_hz: f64,
    live_tx_center_hz: f64,
    sample_rate_hz: f64,
    rx_ch: usize,
    tx_ch: usize,
    rx_ant: String,
    tx_ant: String,
    pa_setting: String,
    rx_gains_fingerprint: String,
    tx_gains_fingerprint: String,
}

impl SoapyIo {
    pub fn new(cfg: &SharedConfig) -> Result<Self, soapysdr::Error> {
        let binding = cfg.config();
        let soapy_cfg = binding
            .phy_io
            .soapysdr
            .as_ref()
            .expect("SoapySdr config must be set for SoapySdr PhyIo");

        let mode = cfg.config().stack_mode;

        let (dev, sdr_settings) = open_device(&soapy_cfg, mode)?;

        let rx_ch = sdr_settings.rx_ch;
        let tx_ch = sdr_settings.tx_ch;
        let temperature_sensor_reads_supported = temperature_sensor_reads_supported(&sdr_settings.name);
        let sdr_name = sdr_settings.name.clone();
        let rx_ant_configured = sdr_settings.rx_ant.clone();
        let tx_ant_configured = sdr_settings.tx_ant.clone();
        let rx_gain_configured = sdr_settings.rx_gain.clone();
        let tx_gain_configured = sdr_settings.tx_gain.clone();
        let rx_args_configured = sdr_settings.rx_args.clone();
        let tx_args_configured = sdr_settings.tx_args.clone();

        // Get PPM corrected freqs
        let (dl_corrected, _) = soapy_cfg.dl_freq_corrected();
        let (ul_corrected, _) = soapy_cfg.ul_freq_corrected();

        let (live_rx_carrier_hz, live_tx_carrier_hz) = match mode {
            StackMode::Bs => (Some(ul_corrected), Some(dl_corrected)),
            StackMode::Ms => (Some(dl_corrected), Some(ul_corrected)),
            StackMode::Mon => {
                unimplemented!("Monitor mode not implemented yet");
            }
        };

        let (rx_freq, tx_freq) = match mode {
            StackMode::Bs => (
                Some(ul_corrected - SOAPY_FREQ_OFFSET), // Offset RX center frequency from carrier frequency
                Some(dl_corrected),
            ),
            StackMode::Ms => (
                Some(dl_corrected - SOAPY_FREQ_OFFSET), // Offset RX center frequency from carrier frequency
                Some(ul_corrected),
            ),
            StackMode::Mon => {
                unimplemented!("Monitor mode not implemented yet");
            }
        };

        let rx_enabled = rx_freq.is_some();
        let tx_enabled = tx_freq.is_some();

        let mut rx_fs: f64 = 0.0;
        if rx_enabled {
            soapycheck!(
                "set RX sample rate",
                dev.set_sample_rate(soapysdr::Direction::Rx, rx_ch, sdr_settings.fs)
            );
            // Read the actual sample rate obtained and store it
            // to avoid having to read it again every time it is needed.
            rx_fs = soapycheck!("get RX sample rate", dev.sample_rate(soapysdr::Direction::Rx, rx_ch));
        }
        let mut tx_fs: f64 = 0.0;
        if tx_enabled {
            soapycheck!(
                "set TX sample rate",
                dev.set_sample_rate(soapysdr::Direction::Tx, tx_ch, sdr_settings.fs)
            );
            tx_fs = soapycheck!("get TX sample rate", dev.sample_rate(soapysdr::Direction::Tx, tx_ch));
        }

        if rx_enabled {
            // If rx_enabled is true, we already know rx_freq is not None,
            // so unwrap is fine here.
            soapycheck!(
                "set RX center frequency",
                dev.set_frequency(soapysdr::Direction::Rx, rx_ch, rx_freq.unwrap(), soapysdr::Args::new())
            );

            if let Some(ref ant) = sdr_settings.rx_ant {
                soapycheck!("set RX antenna", dev.set_antenna(soapysdr::Direction::Rx, rx_ch, ant.as_str()));
            }

            for (name, gain) in &sdr_settings.rx_gain {
                soapycheck!(
                    "set RX gain",
                    dev.set_gain_element(soapysdr::Direction::Rx, rx_ch, name.as_str(), *gain)
                );
            }
        }

        if tx_enabled {
            soapycheck!(
                "set TX center frequency",
                dev.set_frequency(soapysdr::Direction::Tx, tx_ch, tx_freq.unwrap(), soapysdr::Args::new())
            );

            if let Some(ref ant) = sdr_settings.tx_ant {
                soapycheck!("set TX antenna", dev.set_antenna(soapysdr::Direction::Tx, tx_ch, ant.as_str()));
            }

            for (name, gain) in &sdr_settings.tx_gain {
                soapycheck!(
                    "set TX gain",
                    dev.set_gain_element(soapysdr::Direction::Tx, tx_ch, name.as_str(), *gain)
                );
            }

            let runtime_calibration_config = TxCalibrationRuntimeConfig {
                name: sdr_name.clone(),
                live_rx_carrier_hz: live_rx_carrier_hz.unwrap_or(rx_freq.unwrap_or_default()),
                live_tx_carrier_hz: live_tx_carrier_hz.unwrap_or(tx_freq.unwrap_or_default()),
                live_rx_center_hz: dev
                    .frequency(soapysdr::Direction::Rx, rx_ch)
                    .unwrap_or_else(|_| rx_freq.unwrap_or_default()),
                live_tx_center_hz: dev
                    .frequency(soapysdr::Direction::Tx, tx_ch)
                    .unwrap_or_else(|_| tx_freq.unwrap_or_default()),
                sample_rate_hz: tx_fs,
                rx_ch,
                tx_ch,
                rx_ant: dev
                    .antenna(soapysdr::Direction::Rx, rx_ch)
                    .ok()
                    .or_else(|| rx_ant_configured.clone())
                    .unwrap_or_else(|| "auto".to_string()),
                tx_ant: dev
                    .antenna(soapysdr::Direction::Tx, tx_ch)
                    .ok()
                    .or_else(|| tx_ant_configured.clone())
                    .unwrap_or_else(|| "auto".to_string()),
                pa_setting: dev.read_setting(SOAPY_SX_PA_SETTING).unwrap_or_else(|_| "unknown".to_string()),
                rx_gains_fingerprint: device_gains_fingerprint(&dev, soapysdr::Direction::Rx, rx_ch, &rx_gain_configured),
                tx_gains_fingerprint: device_gains_fingerprint(&dev, soapysdr::Direction::Tx, tx_ch, &tx_gain_configured),
            };
            apply_configured_tx_calibration(&dev, tx_ch, soapy_cfg, &runtime_calibration_config);
        }

        let rx_args = args_from_pairs(&rx_args_configured);
        let tx_args = args_from_pairs(&tx_args_configured);

        let mut rx = if rx_enabled {
            Some(soapycheck!("setup RX stream", dev.rx_stream_args(&[rx_ch], rx_args)))
        } else {
            None
        };
        let mut tx = if tx_enabled {
            Some(soapycheck!("setup TX stream", dev.tx_stream_args(&[tx_ch], tx_args)))
        } else {
            None
        };
        if let Some(rx) = &mut rx {
            soapycheck!("activate RX stream", rx.activate(None));
        }
        if let Some(tx) = &mut tx {
            soapycheck!("activate TX stream", tx.activate(None));
        }
        Ok(Self {
            rx_ch,
            tx_ch,
            rx_fs,
            tx_fs,
            initial_time: None,
            rx_next_count: 0,
            prev_time_ns: -1,
            use_get_hardware_time: sdr_settings.use_get_hardware_time,
            dev,
            rx,
            tx,
            temperature_sensor_reads_supported,
            sdr_name,
            rx_ant: rx_ant_configured,
            tx_ant: tx_ant_configured,
            rx_gain: rx_gain_configured,
            tx_gain: tx_gain_configured,
            rx_args: rx_args_configured,
            tx_args: tx_args_configured,
            live_rx_carrier_hz,
            live_tx_carrier_hz,
        })
    }

    pub fn run_tx_calibration(&mut self, calibration_path: &str) -> Result<TxCalibrationFile, String> {
        if !self.tx_enabled() || !self.rx_enabled() {
            return Err("TX calibration requires both RX and TX streams".to_string());
        }

        let original_rx_freq = self
            .dev
            .frequency(soapysdr::Direction::Rx, self.rx_ch)
            .map_err(|e| format!("read RX frequency before calibration: {}", e))?;
        let tx_freq = self
            .dev
            .frequency(soapysdr::Direction::Tx, self.tx_ch)
            .map_err(|e| format!("read TX frequency before calibration: {}", e))?;
        let tx_active_before = self.tx.as_ref().is_some_and(|tx| tx.active());
        let rx_active_before = self.rx.as_ref().is_some_and(|rx| rx.active());

        let mut session = TxCalibrationSession {
            live_rx_carrier_hz: self.live_rx_carrier_hz.unwrap_or(original_rx_freq),
            live_tx_carrier_hz: self.live_tx_carrier_hz.unwrap_or(tx_freq),
            live_rx_center_hz: original_rx_freq,
            live_tx_center_hz: tx_freq,
            calibration_center_hz: tx_freq,
            rx_stream_existed_before: self.rx.is_some(),
            tx_stream_existed_before: self.tx.is_some(),
            rx_active_before,
            tx_active_before,
            original_rx_antenna: None,
            original_pa_setting: None,
            loopback_source: "external_rf_coupling".to_string(),
        };

        tracing::warn!(
            "SoapySDR: starting destructive TX DC/IQ calibration path={} live_rx_carrier={:.0}Hz live_tx_carrier={:.0}Hz duplex_shift={:+.0}Hz rx_center={:.0}Hz tx_center={:.0}Hz",
            calibration_path,
            session.live_rx_carrier_hz,
            session.live_tx_carrier_hz,
            session.live_tx_carrier_hz - session.live_rx_carrier_hz,
            session.live_rx_center_hz,
            session.live_tx_center_hz
        );

        let result = match self.prepare_tx_calibration_session(&mut session) {
            Ok(()) => self.run_tx_calibration_inner(calibration_path, &session),
            Err(err) => Err(err),
        };

        let restore_result = self.restore_after_tx_calibration(&session);
        if let Err(restore_err) = restore_result {
            tracing::error!("SoapySDR: TX calibration restore failed: {}", restore_err);
            if result.is_ok() {
                return Err(restore_err);
            }
        }

        result
    }

    fn prepare_tx_calibration_session(&mut self, session: &mut TxCalibrationSession) -> Result<(), String> {
        self.drop_streams_for_calibration()?;

        session.original_rx_antenna = self.dev.antenna(soapysdr::Direction::Rx, self.rx_ch).ok();
        let rx_antennas = self
            .dev
            .antennas(soapysdr::Direction::Rx, self.rx_ch)
            .map_err(|e| format!("list RX antennas before calibration: {}", e))?;
        let has_internal_loopback = rx_antennas.iter().any(|ant| ant == SOAPY_SX_LOOPBACK_ANTENNA);

        if has_internal_loopback {
            session.original_pa_setting = self.dev.read_setting(SOAPY_SX_PA_SETTING).ok();
            self.dev
                .set_antenna(soapysdr::Direction::Rx, self.rx_ch, SOAPY_SX_LOOPBACK_ANTENNA)
                .map_err(|e| format!("select RX internal loopback antenna {}: {}", SOAPY_SX_LOOPBACK_ANTENNA, e))?;
            session.loopback_source = "rx_internal_lb".to_string();
            tracing::warn!(
                "SoapySDR: TX calibration leaves {}={} while RX uses internal LB",
                SOAPY_SX_PA_SETTING,
                session.original_pa_setting.as_deref().unwrap_or("unknown")
            );
        } else if self.sdr_name == "SXceiver" {
            return Err(
                "SXceiver internal RF loopback antenna LB is not available; external RF coupling is too weak for reliable calibration"
                    .to_string(),
            );
        }

        self.dev
            .set_frequency(
                soapysdr::Direction::Tx,
                self.tx_ch,
                session.calibration_center_hz,
                soapysdr::Args::new(),
            )
            .map_err(|e| format!("set TX calibration frequency {:.0} Hz: {}", session.calibration_center_hz, e))?;
        self.dev
            .set_frequency(
                soapysdr::Direction::Rx,
                self.rx_ch,
                session.calibration_center_hz,
                soapysdr::Args::new(),
            )
            .map_err(|e| {
                format!(
                    "retune RX to TX calibration frequency {:.0} Hz: {}",
                    session.calibration_center_hz, e
                )
            })?;

        tracing::warn!(
            "SoapySDR: TX calibration using {} at {:.0}Hz; live duplex RX/TX remains {:.0}/{:.0}Hz in calibration.toml",
            session.loopback_source,
            session.calibration_center_hz,
            session.live_rx_carrier_hz,
            session.live_tx_carrier_hz
        );
        Ok(())
    }

    fn run_tx_calibration_inner(&mut self, calibration_path: &str, session: &TxCalibrationSession) -> Result<TxCalibrationFile, String> {
        let (mut rx_stream, mut tx_stream) = self.setup_tx_calibration_streams()?;

        let result = (|| {
            let block_len = self.calibration_block_len();
            let tone_hz = quantized_tone_hz(TX_CAL_TONE_HZ, self.tx_fs, block_len);
            let tone = calibration_tone(self.tx_fs, tone_hz, TX_CAL_TONE_AMPLITUDE, block_len);
            let limits = TxCalibrationLimits::default();

            let rx_baseline = self.capture_rx_only_calibration_baseline(&mut rx_stream, &mut tx_stream, block_len)?;
            let neutral = TxCalibrationCoefficients::default();
            let reference_meas =
                self.capture_calibration_measurement(&mut rx_stream, &mut tx_stream, rx_baseline, &tone, tone_hz, neutral)?;
            if reference_meas.snr_db < TX_CAL_MIN_SNR_DB {
                return Err(format!(
                    "RF loopback signal too weak for calibration: SNR {:.1} dB < {:.1} dB source={} tone {:.1} Hz signal {:.1} dBFS measured_floor {:.1} dBFS rx_baseline {:.1} dBFS calibration_freq {:.0} Hz live_rx {:.0} Hz live_tx {:.0} Hz",
                    reference_meas.snr_db,
                    TX_CAL_MIN_SNR_DB,
                    session.loopback_source,
                    tone_hz,
                    reference_meas.signal_dbfs,
                    reference_meas.loopback_floor_dbfs,
                    reference_meas.rx_baseline_dbfs,
                    session.calibration_center_hz,
                    session.live_rx_carrier_hz,
                    session.live_tx_carrier_hz
                ));
            }

            let mut best = neutral;
            let mut best_meas = reference_meas;

            for step in [0.04, 0.02, 0.01, 0.005] {
                for axis in [CalibrationAxis::DcI, CalibrationAxis::DcQ] {
                    let mut axis_best = best;
                    let mut axis_meas = best_meas;
                    for direction in [-1.0, 1.0] {
                        let candidate = with_axis_delta(best, axis, step * direction, &limits);
                        let meas =
                            self.capture_calibration_measurement(&mut rx_stream, &mut tx_stream, rx_baseline, &tone, tone_hz, candidate)?;
                        if meas.carrier_leakage_dbc < axis_meas.carrier_leakage_dbc {
                            axis_best = candidate;
                            axis_meas = meas;
                        }
                    }
                    best = axis_best;
                    best_meas = axis_meas;
                }
            }

            let dc_best = best;
            let dc_best_meas = best_meas;

            for step in [0.12, 0.06, 0.03, 0.015] {
                for axis in [CalibrationAxis::IqI, CalibrationAxis::IqQ] {
                    let mut axis_best = best;
                    let mut axis_meas = best_meas;
                    let mut axis_score = iq_calibration_score(&axis_meas);
                    for direction in [-1.0, 1.0] {
                        let candidate = with_axis_delta(best, axis, step * direction, &limits);
                        let meas =
                            self.capture_calibration_measurement(&mut rx_stream, &mut tx_stream, rx_baseline, &tone, tone_hz, candidate)?;
                        let score = iq_calibration_score(&meas);
                        if score < axis_score {
                            axis_best = candidate;
                            axis_meas = meas;
                            axis_score = score;
                        }
                    }
                    best = axis_best;
                    best_meas = axis_meas;
                }
            }

            let iq_best = best;
            let iq_best_meas = best_meas;
            let dc_accepted = dc_calibration_accepted(&reference_meas, &dc_best_meas);
            let iq_accepted = iq_calibration_accepted(&reference_meas, &iq_best_meas);
            let mut applied = TxCalibrationCoefficients::default();
            if dc_accepted {
                applied.dc_i = dc_best.dc_i;
                applied.dc_q = dc_best.dc_q;
            }
            if iq_accepted {
                applied = iq_best;
            }

            self.apply_tx_calibration_coefficients(applied, true, true)?;
            let final_meas = self.capture_calibration_measurement(&mut rx_stream, &mut tx_stream, rx_baseline, &tone, tone_hz, applied)?;
            let carrier_improvement = reference_meas.carrier_leakage_dbc - final_meas.carrier_leakage_dbc;
            let image_improvement = final_meas.image_rejection_db - reference_meas.image_rejection_db;
            let evm_improvement = reference_meas.evm_proxy_pct - final_meas.evm_proxy_pct;
            let accepted = applied != TxCalibrationCoefficients::default()
                && (dc_accepted || iq_accepted)
                && calibration_capture_quality_ok(&final_meas);

            let now = unix_secs_now();
            let file = TxCalibrationFile {
                schema_version: 1,
                status: if accepted {
                    "calibrated".to_string()
                } else {
                    "rejected".to_string()
                },
                created_unix_secs: now,
                updated_unix_secs: now,
                device: TxCalibrationDevice {
                    name: self.sdr_name.clone(),
                    tx_frequency_hz: session.live_tx_carrier_hz,
                    rx_frequency_hz: session.live_rx_carrier_hz,
                    tx_center_frequency_hz: session.live_tx_center_hz,
                    rx_center_frequency_hz: session.live_rx_center_hz,
                    calibration_frequency_hz: session.calibration_center_hz,
                    duplex_shift_hz: session.live_tx_carrier_hz - session.live_rx_carrier_hz,
                    sample_rate_hz: self.tx_fs,
                    tx_channel: self.tx_ch,
                    rx_channel: self.rx_ch,
                    tx_antenna: self.tx_ant.clone().unwrap_or_else(|| "auto".to_string()),
                    rx_antenna: self
                        .rx_ant
                        .clone()
                        .unwrap_or_else(|| session.original_rx_antenna.clone().unwrap_or_else(|| "auto".to_string())),
                    loopback_source: session.loopback_source.clone(),
                    pa_setting: session.original_pa_setting.clone().unwrap_or_else(|| "unknown".to_string()),
                    tx_gains_fingerprint: device_gains_fingerprint(&self.dev, soapysdr::Direction::Tx, self.tx_ch, &self.tx_gain),
                    rx_gains_fingerprint: device_gains_fingerprint(&self.dev, soapysdr::Direction::Rx, self.rx_ch, &self.rx_gain),
                },
                limits,
                reference: reference_meas.into_point("neutral_no_calibration", neutral),
                calibrated: final_meas.into_point("applied_candidate", applied),
                applied,
                report: TxCalibrationReport {
                    carrier_leakage_improvement_db: carrier_improvement,
                    image_rejection_improvement_db: image_improvement,
                    evm_proxy_improvement_pct: evm_improvement,
                    accepted,
                    accepted_dc: dc_accepted,
                    accepted_iq: iq_accepted,
                    summary: if accepted {
                        format!(
                            "accepted {}: carrier leak {:+.1} dB ({:.1}->{:.1} dBc), image {:+.1} dB ({:.1}->{:.1} dB), EVM {:+.2} pp, SNR {:.1}->{:.1} dB",
                            if iq_accepted {
                                "DC+IQ"
                            } else if dc_accepted {
                                "DC-only"
                            } else {
                                "neutral"
                            },
                            carrier_improvement,
                            reference_meas.carrier_leakage_dbc,
                            final_meas.carrier_leakage_dbc,
                            image_improvement,
                            reference_meas.image_rejection_db,
                            final_meas.image_rejection_db,
                            evm_improvement,
                            reference_meas.snr_db,
                            final_meas.snr_db
                        )
                    } else {
                        format!(
                            "rejected: gates failed; carrier {:+.1} dB ({:.1}->{:.1} dBc), image {:+.1} dB ({:.1}->{:.1} dB), EVM {:+.2} pp, SNR {:.1}->{:.1} dB, floor drift {:.1} dB",
                            carrier_improvement,
                            reference_meas.carrier_leakage_dbc,
                            final_meas.carrier_leakage_dbc,
                            image_improvement,
                            reference_meas.image_rejection_db,
                            final_meas.image_rejection_db,
                            evm_improvement,
                            reference_meas.snr_db,
                            final_meas.snr_db,
                            final_meas.floor_drift_db
                        )
                    },
                },
            };

            write_tx_calibration_file_atomic(calibration_path, &file)?;
            tracing::warn!(
                "SoapySDR: TX calibration {} saved to {}: {}",
                if accepted { "accepted" } else { "rejected" },
                calibration_path,
                file.report.summary
            );
            Ok(file)
        })();

        let zero = vec![ComplexSample::ZERO; self.calibration_block_len()];
        tx_stream.write_all(&[&zero], None, false, 250_000).ok();
        tx_stream.deactivate(None).ok();
        rx_stream.deactivate(None).ok();
        result
    }

    fn deactivate_streams_for_calibration(&mut self) -> Result<(), String> {
        if let Some(tx) = &mut self.tx {
            if tx.active() {
                tx.deactivate(None).map_err(|e| format!("deactivate TX stream: {}", e))?;
            }
        }
        if let Some(rx) = &mut self.rx {
            if rx.active() {
                rx.deactivate(None).map_err(|e| format!("deactivate RX stream: {}", e))?;
            }
        }
        Ok(())
    }

    fn drop_streams_for_calibration(&mut self) -> Result<(), String> {
        self.deactivate_streams_for_calibration()?;
        self.tx = None;
        self.rx = None;
        Ok(())
    }

    fn setup_tx_calibration_streams(&self) -> Result<(soapysdr::RxStream<StreamType>, soapysdr::TxStream<StreamType>), String> {
        let rx_args = args_from_pairs(&self.rx_args);
        let tx_args = args_from_pairs(&self.tx_args);
        let mut rx = self
            .dev
            .rx_stream_args::<StreamType, _>(&[self.rx_ch], rx_args)
            .map_err(|e| format!("setup RX calibration stream: {}", e))?;
        let mut tx = self
            .dev
            .tx_stream_args::<StreamType, _>(&[self.tx_ch], tx_args)
            .map_err(|e| format!("setup TX calibration stream: {}", e))?;
        rx.activate(None).map_err(|e| format!("activate RX calibration stream: {}", e))?;
        tx.activate(None).map_err(|e| format!("activate TX calibration stream: {}", e))?;
        Ok((rx, tx))
    }

    fn calibration_block_len(&self) -> usize {
        stream_period_samples(&self.tx_args)
            .or_else(|| stream_period_samples(&self.rx_args))
            .map(|period| period.saturating_mul(4))
            .unwrap_or(TX_CAL_FALLBACK_BLOCK_SAMPLES)
            .max(64)
    }

    fn setup_live_streams_after_tx_calibration(&mut self, session: &TxCalibrationSession) -> Result<(), String> {
        let rx = if session.rx_stream_existed_before {
            let rx_args = args_from_pairs(&self.rx_args);
            Some(
                self.dev
                    .rx_stream_args::<StreamType, _>(&[self.rx_ch], rx_args)
                    .map_err(|e| format!("setup RX live stream after calibration: {}", e))?,
            )
        } else {
            None
        };
        let tx = if session.tx_stream_existed_before {
            let tx_args = args_from_pairs(&self.tx_args);
            Some(
                self.dev
                    .tx_stream_args::<StreamType, _>(&[self.tx_ch], tx_args)
                    .map_err(|e| format!("setup TX live stream after calibration: {}", e))?,
            )
        } else {
            None
        };

        self.rx = rx;
        self.tx = tx;

        if session.rx_active_before {
            if let Some(rx) = &mut self.rx {
                rx.activate(None).map_err(|e| format!("reactivate RX live stream: {}", e))?;
            }
        }
        if session.tx_active_before {
            if let Some(tx) = &mut self.tx {
                tx.activate(None).map_err(|e| format!("reactivate TX live stream: {}", e))?;
            }
        }
        Ok(())
    }

    fn restore_after_tx_calibration(&mut self, session: &TxCalibrationSession) -> Result<(), String> {
        self.drop_streams_for_calibration()?;
        if let Some(original_pa) = &session.original_pa_setting {
            if let Err(err) = self.dev.write_setting(SOAPY_SX_PA_SETTING, original_pa.as_str()) {
                tracing::warn!(
                    "SoapySDR: failed to restore {}={} after calibration: {}",
                    SOAPY_SX_PA_SETTING,
                    original_pa,
                    err
                );
            }
        }
        if let Some(original_rx_antenna) = &session.original_rx_antenna {
            if let Err(err) = self
                .dev
                .set_antenna(soapysdr::Direction::Rx, self.rx_ch, original_rx_antenna.as_str())
            {
                tracing::warn!(
                    "SoapySDR: failed to restore RX antenna {} after calibration: {}",
                    original_rx_antenna,
                    err
                );
            }
        }
        self.dev
            .set_frequency(
                soapysdr::Direction::Tx,
                self.tx_ch,
                session.live_tx_center_hz,
                soapysdr::Args::new(),
            )
            .map_err(|e| format!("restore TX frequency {:.0} Hz: {}", session.live_tx_center_hz, e))?;
        self.dev
            .set_frequency(
                soapysdr::Direction::Rx,
                self.rx_ch,
                session.live_rx_center_hz,
                soapysdr::Args::new(),
            )
            .map_err(|e| format!("restore RX frequency {:.0} Hz: {}", session.live_rx_center_hz, e))?;
        self.setup_live_streams_after_tx_calibration(session)?;
        self.initial_time = None;
        self.rx_next_count = 0;
        self.prev_time_ns = -1;
        Ok(())
    }

    fn capture_calibration_measurement(
        &mut self,
        rx: &mut soapysdr::RxStream<StreamType>,
        tx: &mut soapysdr::TxStream<StreamType>,
        rx_baseline: CalibrationRxBaseline,
        tone: &[StreamType],
        tone_hz: f64,
        coeffs: TxCalibrationCoefficients,
    ) -> Result<CalibrationMeasurement, String> {
        self.apply_tx_calibration_coefficients(coeffs, true, true)?;
        std::thread::sleep(std::time::Duration::from_millis(25));

        let zero = vec![ComplexSample::ZERO; tone.len()];
        let floor_before = capture_calibration_samples(rx, tx, &zero)?;
        let capture = capture_calibration_samples(rx, tx, tone)?;
        let floor_after = capture_calibration_samples(rx, tx, &zero)?;
        Ok(measure_calibration_capture(
            &capture,
            &floor_before,
            &floor_after,
            self.rx_fs,
            tone_hz,
            rx_baseline,
        ))
    }

    fn capture_rx_only_calibration_baseline(
        &mut self,
        rx: &mut soapysdr::RxStream<StreamType>,
        tx: &mut soapysdr::TxStream<StreamType>,
        block_len: usize,
    ) -> Result<CalibrationRxBaseline, String> {
        self.apply_tx_calibration_coefficients(TxCalibrationCoefficients::default(), true, true)?;
        std::thread::sleep(std::time::Duration::from_millis(25));
        capture_rx_only_calibration_samples(rx, tx, block_len).map(|capture| CalibrationRxBaseline::from_samples(&capture))
    }

    fn apply_tx_calibration_coefficients(&self, coeffs: TxCalibrationCoefficients, apply_dc: bool, apply_iq: bool) -> Result<(), String> {
        if apply_dc {
            if self.dev.has_dc_offset_mode(soapysdr::Direction::Tx, self.tx_ch).unwrap_or(false) {
                let _ = self.dev.set_dc_offset_mode(soapysdr::Direction::Tx, self.tx_ch, false);
            }
            if !self
                .dev
                .has_dc_offset(soapysdr::Direction::Tx, self.tx_ch)
                .map_err(|e| format!("query TX DC offset support: {}", e))?
            {
                return Err("TX DC offset correction is not supported by this SoapySDR driver".to_string());
            }
            self.dev
                .set_dc_offset(soapysdr::Direction::Tx, self.tx_ch, coeffs.dc_i, coeffs.dc_q)
                .map_err(|e| format!("set TX DC offset: {}", e))?;
        }

        if apply_iq {
            if !self
                .dev
                .has_iq_balance(soapysdr::Direction::Tx, self.tx_ch)
                .map_err(|e| format!("query TX IQ balance support: {}", e))?
            {
                return Err("TX IQ balance correction is not supported by this SoapySDR driver".to_string());
            }
            self.dev
                .set_iq_balance(soapysdr::Direction::Tx, self.tx_ch, coeffs.iq_i, coeffs.iq_q)
                .map_err(|e| format!("set TX IQ balance: {}", e))?;
        }

        Ok(())
    }

    pub fn receive(&mut self, buffer: &mut [StreamType]) -> Result<RxResult, RxTxDevError> {
        if let Some(rx) = &mut self.rx {
            // RX is enabled
            match rx.read(&mut [buffer], 1000000) {
                Ok(len) => {
                    // Get timestamp, set initial time if not yet set
                    let time = rx.time_ns();
                    // rust-soapysdr does not let us if a timestamp was available
                    // so we have to guess by checking whether it has changed from its previous value.
                    let timestamp_available = time != self.prev_time_ns;
                    self.prev_time_ns = time;

                    if self.initial_time.is_none() && timestamp_available {
                        self.initial_time = Some(time - ticks_to_time_ns(self.rx_next_count, self.rx_fs));
                        tracing::trace!("Set initial_time to {} ns", self.initial_time.unwrap());
                    };

                    // Re-compute total count from timestamp (gracefully handles lost samples).
                    let mut count = if timestamp_available {
                        time_ns_to_ticks(time - self.initial_time.unwrap(), self.rx_fs)
                    } else {
                        // If timestamp was not available,
                        // assume the read continues right after the previous read.
                        // Some drivers, particularly SoapyRemote,
                        // may provide a timestamp only in some of the reads.
                        self.rx_next_count
                    };

                    // Smooth tiny timestamp jitter (e.g. +/-1 sample) to keep counters monotonic
                    // This is known to happen for LimeSDR Mini v2 after some time
                    let delta_from_expected = count - self.rx_next_count;
                    if delta_from_expected.abs() <= RX_TIMESTAMP_JITTER_TOLERANCE_SAMPLES {
                        if delta_from_expected != 0 {
                            // Re-anchor phase so persistent +/-1 sample offset is corrected
                            let initial_time = self.initial_time.unwrap() + ticks_to_time_ns(delta_from_expected, self.rx_fs); // unwrap never fails
                            self.initial_time = Some(initial_time);
                            tracing::debug!(
                                "RX timestamp jitter {} sample(s); re-anchoring initial_time by {} ns",
                                delta_from_expected,
                                ticks_to_time_ns(delta_from_expected, self.rx_fs)
                            );
                        }
                        count = self.rx_next_count;
                    }

                    // Store expected sample count for the next sample to be read.
                    // This is used in case timestamp is missing.
                    self.rx_next_count = count + len as SampleCount;

                    Ok(RxResult { len, count })
                }
                Err(_) => Err(RxTxDevError::RxReadError),
            }
        } else {
            // RX is disabled
            Err(RxTxDevError::RxReadError)
        }
    }

    pub fn set_tx_stream_active(&mut self, active: bool) -> Result<bool, RxTxDevError> {
        let Some(tx) = &mut self.tx else {
            return Ok(false);
        };
        if tx.active() == active {
            return Ok(false);
        }

        let result = if active { tx.activate(None) } else { tx.deactivate(None) };
        match result {
            Ok(()) => Ok(true),
            Err(err) => {
                tracing::error!(
                    "SoapySDR: Failed to {} TX stream: {}",
                    if active { "activate" } else { "deactivate" },
                    err
                );
                Err(RxTxDevError::TxStreamError)
            }
        }
    }

    pub fn transmit(&mut self, buffer: &[StreamType], count: Option<SampleCount>) -> Result<(), RxTxDevError> {
        if let Some(tx) = &mut self.tx {
            if let Some(initial_time) = self.initial_time {
                tx.write_all(
                    &[buffer],
                    count.map(|count| initial_time + ticks_to_time_ns(count, self.tx_fs)),
                    false,
                    1000000,
                )
                .map_err(|_| RxTxDevError::TxStreamError)
            } else {
                // initial_time is not available, so TX is not possible yet
                Err(RxTxDevError::TxStreamError)
            }
        } else {
            // TX is disabled
            Err(RxTxDevError::TxStreamError)
        }
    }

    pub fn current_time(&self) -> Result<i64, RxTxDevError> {
        self.dev.get_hardware_time(None).map_err(|_| RxTxDevError::RxReadError)
    }

    /// Current hardware time as RX sample count
    pub fn rx_current_count(&self) -> Result<SampleCount, RxTxDevError> {
        if !self.rx_enabled() {
            return Ok(0);
        }
        if self.use_get_hardware_time {
            Ok(time_ns_to_ticks(self.current_time()? - self.initial_time.unwrap_or(0), self.rx_fs))
        } else {
            Ok(self.rx_next_count - 1)
        }
    }

    /// Current hardware time as TX sample count
    pub fn tx_current_count(&self) -> Result<SampleCount, RxTxDevError> {
        if !self.tx_enabled() {
            return Ok(0);
        }
        if self.use_get_hardware_time {
            Ok(time_ns_to_ticks(self.current_time()? - self.initial_time.unwrap_or(0), self.tx_fs))
        } else {
            // Assumes equal RX and TX sample rates
            // and does not work if RX is disabled.
            // This is not a problem right now but could be fixed if needed.
            Ok(self.rx_next_count - 1)
        }
    }

    pub fn tx_possible(&self) -> bool {
        // initial_time is obtained from the first RX read (that includes a timestamp),
        // so prevent TX before it is available.
        self.tx_enabled() && self.initial_time.is_some()
    }

    pub fn rx_sample_rate(&self) -> f64 {
        self.rx_fs
    }

    pub fn tx_sample_rate(&self) -> f64 {
        self.tx_fs
    }

    pub fn rx_center_frequency(&self) -> Result<f64, soapysdr::Error> {
        self.dev.frequency(soapysdr::Direction::Rx, self.rx_ch)
    }

    pub fn tx_center_frequency(&self) -> Result<f64, soapysdr::Error> {
        self.dev.frequency(soapysdr::Direction::Tx, self.tx_ch)
    }

    pub fn rx_enabled(&self) -> bool {
        self.rx.is_some()
    }

    pub fn tx_enabled(&self) -> bool {
        self.tx.is_some()
    }

    /// Read SDR temperature in °C if the device exposes a temp-like sensor.
    /// LimeSDR returns "temp" via list_sensors; USRP usually "fp_temp" or similar;
    /// SXceiver / µCell don't currently expose any sensor and this returns None.
    /// We probe sensor names rather than hard-coding per-driver, so any future radio
    /// that follows the Soapy convention works without code changes.
    pub fn read_temperature_c(&self) -> Option<f32> {
        if !self.temperature_sensor_reads_supported {
            return None;
        }
        let sensors = self.dev.list_sensors().ok()?;
        for name in sensors {
            let s = name.to_string();
            let lower = s.to_lowercase();
            if lower.contains("temp") {
                if let Ok(val) = self.dev.read_sensor(&s) {
                    if let Ok(parsed) = val.to_string().parse::<f32>() {
                        return Some(parsed);
                    }
                }
            }
        }
        None
    }

    /// Read back the currently-active TX gain per stage, in dB.
    /// Returns the same gain-element names the radio uses (e.g. "PAD","IAMP" on LimeSDR).
    pub fn read_tx_gains(&self) -> Vec<(String, f32)> {
        if !self.tx_enabled() {
            return Vec::new();
        }
        self.dev
            .list_gains(soapysdr::Direction::Tx, self.tx_ch)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|name| {
                let s = name.to_string();
                self.dev
                    .gain_element(soapysdr::Direction::Tx, self.tx_ch, s.clone())
                    .ok()
                    .map(|g| (s, g as f32))
            })
            .collect()
    }

    /// Read back the currently-active RX gain per stage, in dB.
    pub fn read_rx_gains(&self) -> Vec<(String, f32)> {
        if !self.rx_enabled() {
            return Vec::new();
        }
        self.dev
            .list_gains(soapysdr::Direction::Rx, self.rx_ch)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|name| {
                let s = name.to_string();
                self.dev
                    .gain_element(soapysdr::Direction::Rx, self.rx_ch, s.clone())
                    .ok()
                    .map(|g| (s, g as f32))
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
struct ComplexF64 {
    re: f64,
    im: f64,
}

impl ComplexF64 {
    const ZERO: Self = Self { re: 0.0, im: 0.0 };

    fn abs(self) -> f64 {
        (self.re * self.re + self.im * self.im).sqrt()
    }

    fn sub(self, other: Self) -> Self {
        Self {
            re: self.re - other.re,
            im: self.im - other.im,
        }
    }
}

#[derive(Clone, Copy)]
enum CalibrationAxis {
    DcI,
    DcQ,
    IqI,
    IqQ,
}

#[derive(Clone, Copy)]
struct CalibrationRxBaseline {
    dc: ComplexF64,
    rms_amp: f64,
}

impl CalibrationRxBaseline {
    fn from_samples(samples: &[StreamType]) -> Self {
        let dc = mean_complex(samples);
        Self {
            dc,
            rms_amp: centered_rms(samples, dc),
        }
    }
}

#[derive(Clone, Copy)]
struct CalibrationMeasurement {
    dc: ComplexF64,
    carrier_leakage_dbc: f64,
    carrier_leakage_dbfs: f64,
    image_rejection_db: f64,
    evm_proxy_pct: f64,
    signal_dbfs: f64,
    noise_floor_dbfs: f64,
    loopback_floor_dbfs: f64,
    rx_baseline_dbfs: f64,
    floor_drift_db: f64,
    max_component_abs: f64,
    clipped_fraction: f64,
    snr_db: f64,
}

impl CalibrationMeasurement {
    fn into_point(self, label: &str, tx: TxCalibrationCoefficients) -> TxCalibrationPoint {
        TxCalibrationPoint {
            label: label.to_string(),
            tx,
            carrier_leakage_dbc: self.carrier_leakage_dbc,
            carrier_leakage_dbfs: self.carrier_leakage_dbfs,
            image_rejection_db: self.image_rejection_db,
            evm_proxy_pct: self.evm_proxy_pct,
            signal_dbfs: self.signal_dbfs,
            noise_floor_dbfs: self.noise_floor_dbfs,
            loopback_floor_dbfs: self.loopback_floor_dbfs,
            rx_baseline_dbfs: self.rx_baseline_dbfs,
            floor_drift_db: self.floor_drift_db,
            max_component_abs: self.max_component_abs,
            clipped_fraction: self.clipped_fraction,
            snr_db: self.snr_db,
        }
    }
}

fn apply_configured_tx_calibration(
    dev: &soapysdr::Device,
    tx_ch: usize,
    soapy_cfg: &CfgSoapySdr,
    runtime_config: &TxCalibrationRuntimeConfig,
) {
    if !soapy_cfg.tx_calibration_enabled {
        return;
    }
    let path = soapy_cfg.tx_calibration_file.as_str();
    let calibration = match read_tx_calibration_file(path) {
        Ok(calibration) => calibration,
        Err(err) => {
            tracing::error!("SoapySDR: TX calibration enabled but {} is not usable: {}", path, err);
            return;
        }
    };
    if calibration.status != "calibrated" || !calibration.report.accepted {
        tracing::error!(
            "SoapySDR: TX calibration enabled but {} is not an accepted calibrated report (status={} accepted={}); not applying DC/IQ",
            path,
            calibration.status,
            calibration.report.accepted
        );
        return;
    }
    if let Err(err) = validate_tx_calibration_matches_runtime_config(&calibration, runtime_config) {
        tracing::error!(
            "SoapySDR: TX calibration enabled but {} does not match current resolved RX/TX config: {}; not applying stale DC/IQ",
            path,
            err
        );
        return;
    }

    let coeffs = calibration.applied;
    if soapy_cfg.tx_calibration_apply_dc {
        match dev.has_dc_offset(soapysdr::Direction::Tx, tx_ch) {
            Ok(true) => {
                if dev.has_dc_offset_mode(soapysdr::Direction::Tx, tx_ch).unwrap_or(false) {
                    let _ = dev.set_dc_offset_mode(soapysdr::Direction::Tx, tx_ch, false);
                }
                if let Err(err) = dev.set_dc_offset(soapysdr::Direction::Tx, tx_ch, coeffs.dc_i, coeffs.dc_q) {
                    tracing::error!("SoapySDR: failed to apply TX DC calibration from {}: {}", path, err);
                }
            }
            Ok(false) => tracing::error!("SoapySDR: TX DC calibration requested but driver does not support it"),
            Err(err) => tracing::error!("SoapySDR: failed to query TX DC calibration support: {}", err),
        }
    }

    if soapy_cfg.tx_calibration_apply_iq {
        match dev.has_iq_balance(soapysdr::Direction::Tx, tx_ch) {
            Ok(true) => {
                if let Err(err) = dev.set_iq_balance(soapysdr::Direction::Tx, tx_ch, coeffs.iq_i, coeffs.iq_q) {
                    tracing::error!("SoapySDR: failed to apply TX IQ calibration from {}: {}", path, err);
                }
            }
            Ok(false) => tracing::error!("SoapySDR: TX IQ calibration requested but driver does not support it"),
            Err(err) => tracing::error!("SoapySDR: failed to query TX IQ calibration support: {}", err),
        }
    }

    tracing::warn!(
        "SoapySDR: TX calibration applied from {} dc=({:+.6},{:+.6}) iq=({:+.6},{:+.6}) report={}",
        path,
        coeffs.dc_i,
        coeffs.dc_q,
        coeffs.iq_i,
        coeffs.iq_q,
        calibration.report.summary
    );
}

fn validate_tx_calibration_matches_runtime_config(
    calibration: &TxCalibrationFile,
    runtime: &TxCalibrationRuntimeConfig,
) -> Result<(), String> {
    let device = &calibration.device;
    let expected_duplex_shift_hz = runtime.live_tx_carrier_hz - runtime.live_rx_carrier_hz;
    let mut mismatches = Vec::new();

    if device.name != runtime.name {
        mismatches.push(format!("device.name stored={} current={}", device.name, runtime.name));
    }
    compare_hz(
        &mut mismatches,
        "tx_frequency_hz",
        device.tx_frequency_hz,
        runtime.live_tx_carrier_hz,
        TX_CAL_FREQ_MATCH_TOLERANCE_HZ,
    );
    compare_hz(
        &mut mismatches,
        "rx_frequency_hz",
        device.rx_frequency_hz,
        runtime.live_rx_carrier_hz,
        TX_CAL_FREQ_MATCH_TOLERANCE_HZ,
    );
    compare_hz(
        &mut mismatches,
        "tx_center_frequency_hz",
        device.tx_center_frequency_hz,
        runtime.live_tx_center_hz,
        TX_CAL_FREQ_MATCH_TOLERANCE_HZ,
    );
    compare_hz(
        &mut mismatches,
        "calibration_frequency_hz",
        device.calibration_frequency_hz,
        runtime.live_tx_center_hz,
        TX_CAL_FREQ_MATCH_TOLERANCE_HZ,
    );
    compare_hz(
        &mut mismatches,
        "rx_center_frequency_hz",
        device.rx_center_frequency_hz,
        runtime.live_rx_center_hz,
        TX_CAL_FREQ_MATCH_TOLERANCE_HZ,
    );
    compare_hz(
        &mut mismatches,
        "duplex_shift_hz",
        device.duplex_shift_hz,
        expected_duplex_shift_hz,
        TX_CAL_FREQ_MATCH_TOLERANCE_HZ,
    );
    compare_hz(
        &mut mismatches,
        "sample_rate_hz",
        device.sample_rate_hz,
        runtime.sample_rate_hz,
        TX_CAL_SAMPLE_RATE_MATCH_TOLERANCE_HZ,
    );
    if device.tx_channel != runtime.tx_ch {
        mismatches.push(format!("tx_channel stored={} current={}", device.tx_channel, runtime.tx_ch));
    }
    if device.rx_channel != runtime.rx_ch {
        mismatches.push(format!("rx_channel stored={} current={}", device.rx_channel, runtime.rx_ch));
    }
    if !antenna_matches(&device.tx_antenna, &runtime.tx_ant) {
        mismatches.push(format!("tx_antenna stored={} current={}", device.tx_antenna, runtime.tx_ant));
    }
    if !antenna_matches(&device.rx_antenna, &runtime.rx_ant) {
        mismatches.push(format!("rx_antenna stored={} current={}", device.rx_antenna, runtime.rx_ant));
    }
    if device.pa_setting != runtime.pa_setting {
        mismatches.push(format!("pa_setting stored={} current={}", device.pa_setting, runtime.pa_setting));
    }
    if device.tx_gains_fingerprint != runtime.tx_gains_fingerprint {
        mismatches.push(format!(
            "tx_gains stored={} current={}",
            device.tx_gains_fingerprint, runtime.tx_gains_fingerprint
        ));
    }
    if device.rx_gains_fingerprint != runtime.rx_gains_fingerprint {
        mismatches.push(format!(
            "rx_gains stored={} current={}",
            device.rx_gains_fingerprint, runtime.rx_gains_fingerprint
        ));
    }

    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(mismatches.join("; "))
    }
}

fn compare_hz(mismatches: &mut Vec<String>, name: &str, stored: f64, current: f64, tolerance_hz: f64) {
    if !stored.is_finite() || !current.is_finite() || (stored - current).abs() > tolerance_hz {
        mismatches.push(format!("{name} stored={stored:.3} current={current:.3}"));
    }
}

fn antenna_matches(stored: &str, current: &str) -> bool {
    stored == current || stored == "auto" || current == "auto"
}

fn calibration_tone(sample_rate: f64, tone_hz: f64, amplitude: f32, len: usize) -> Vec<StreamType> {
    (0..len)
        .map(|n| {
            let phase = 2.0 * std::f64::consts::PI * tone_hz * n as f64 / sample_rate;
            ComplexSample {
                re: amplitude * phase.cos() as f32,
                im: amplitude * phase.sin() as f32,
            }
        })
        .collect()
}

fn measure_calibration_capture(
    samples: &[StreamType],
    floor_before: &[StreamType],
    floor_after: &[StreamType],
    sample_rate: f64,
    tone_hz: f64,
    rx_baseline: CalibrationRxBaseline,
) -> CalibrationMeasurement {
    let dc_raw = mean_complex(samples);
    let dc = dc_raw.sub(rx_baseline.dc);
    let signal = dft_at_centered(samples, rx_baseline.dc, sample_rate, tone_hz);
    let image = dft_at_centered(samples, rx_baseline.dc, sample_rate, -tone_hz);
    let dc_amp = dc.abs().max(1.0e-12);
    let signal_amp = signal.abs().max(1.0e-12);
    let image_amp = image.abs().max(1.0e-12);
    let rms_power = samples
        .iter()
        .map(|s| {
            let centered = ComplexF64 {
                re: s.re as f64 - rx_baseline.dc.re,
                im: s.im as f64 - rx_baseline.dc.im,
            };
            centered.re * centered.re + centered.im * centered.im
        })
        .sum::<f64>()
        / samples.len().max(1) as f64;
    let model_power = signal_amp * signal_amp + image_amp * image_amp + dc_amp * dc_amp;
    let noise_rms = (rms_power - model_power).max(1.0e-12).sqrt();
    let floor_before_amp = floor_bin_magnitude(floor_before, sample_rate, tone_hz);
    let floor_after_amp = floor_bin_magnitude(floor_after, sample_rate, tone_hz);
    let loopback_floor_amp = floor_before_amp.max(floor_after_amp).max(1.0e-12);
    let floor_drift_db = ratio_db(floor_after_amp, floor_before_amp);

    CalibrationMeasurement {
        dc,
        carrier_leakage_dbc: 20.0 * (dc_amp / signal_amp).max(1.0e-12).log10(),
        carrier_leakage_dbfs: dbfs_amp(dc_amp),
        image_rejection_db: 20.0 * (signal_amp / image_amp).max(1.0e-12).log10(),
        evm_proxy_pct: ((dc_amp / signal_amp).powi(2) + (image_amp / signal_amp).powi(2)).sqrt() * 100.0,
        signal_dbfs: dbfs_amp(signal_amp),
        noise_floor_dbfs: dbfs_amp(noise_rms),
        loopback_floor_dbfs: dbfs_amp(loopback_floor_amp),
        rx_baseline_dbfs: dbfs_amp(rx_baseline.rms_amp),
        floor_drift_db,
        max_component_abs: max_component_abs(samples),
        clipped_fraction: clipped_fraction(samples),
        snr_db: ratio_db(signal_amp, loopback_floor_amp),
    }
}

fn floor_bin_magnitude(samples: &[StreamType], sample_rate: f64, tone_hz: f64) -> f64 {
    let dc = mean_complex(samples);
    dft_at_centered(samples, dc, sample_rate, tone_hz)
        .abs()
        .max(dft_at_centered(samples, dc, sample_rate, -tone_hz).abs())
}

fn centered_rms(samples: &[StreamType], dc: ComplexF64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let power = samples
        .iter()
        .map(|s| {
            let re = s.re as f64 - dc.re;
            let im = s.im as f64 - dc.im;
            re * re + im * im
        })
        .sum::<f64>()
        / samples.len() as f64;
    power.sqrt()
}

fn max_component_abs(samples: &[StreamType]) -> f64 {
    samples
        .iter()
        .map(|sample| sample.re.abs().max(sample.im.abs()) as f64)
        .fold(0.0, f64::max)
}

fn clipped_fraction(samples: &[StreamType]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let clipped = samples
        .iter()
        .filter(|sample| sample.re.abs() >= TX_CAL_CLIP_LEVEL || sample.im.abs() >= TX_CAL_CLIP_LEVEL)
        .count();
    clipped as f64 / samples.len() as f64
}

fn dbfs_amp(value: f64) -> f64 {
    20.0 * value.max(1.0e-12).log10()
}

fn ratio_db(numerator: f64, denominator: f64) -> f64 {
    dbfs_amp(numerator) - dbfs_amp(denominator)
}

fn mean_complex(samples: &[StreamType]) -> ComplexF64 {
    if samples.is_empty() {
        return ComplexF64::ZERO;
    }
    let mut re = 0.0;
    let mut im = 0.0;
    for sample in samples {
        re += sample.re as f64;
        im += sample.im as f64;
    }
    let n = samples.len() as f64;
    ComplexF64 { re: re / n, im: im / n }
}

fn capture_calibration_samples(
    rx: &mut soapysdr::RxStream<StreamType>,
    tx: &mut soapysdr::TxStream<StreamType>,
    tx_block: &[StreamType],
) -> Result<Vec<StreamType>, String> {
    let mut rx_block = vec![ComplexSample::ZERO; tx_block.len()];
    let mut capture = Vec::with_capacity(tx_block.len() * TX_CAL_CAPTURE_BLOCKS);

    for _ in 0..TX_CAL_PREFILL_BLOCKS {
        tx.write_all(&[tx_block], None, false, 250_000)
            .map_err(|e| format!("write calibration prefill block: {}", e))?;
        read_calibration_block(rx, &mut rx_block, 250_000)?;
    }

    for block_idx in 0..(TX_CAL_SETTLE_BLOCKS + TX_CAL_CAPTURE_BLOCKS) {
        tx.write_all(&[tx_block], None, false, 250_000)
            .map_err(|e| format!("write calibration block: {}", e))?;
        read_calibration_block(rx, &mut rx_block, 250_000)?;
        if block_idx >= TX_CAL_SETTLE_BLOCKS {
            capture.extend_from_slice(&rx_block);
        }
    }
    if capture.len() < tx_block.len() {
        return Err(format!("short calibration capture: {} samples", capture.len()));
    }
    Ok(capture)
}

fn capture_rx_only_calibration_samples(
    rx: &mut soapysdr::RxStream<StreamType>,
    tx: &mut soapysdr::TxStream<StreamType>,
    block_len: usize,
) -> Result<Vec<StreamType>, String> {
    tx.deactivate(None)
        .map_err(|e| format!("deactivate TX stream for RX-only calibration baseline: {}", e))?;
    let result = (|| {
        let mut rx_block = vec![ComplexSample::ZERO; block_len];
        let mut capture = Vec::with_capacity(block_len * TX_CAL_CAPTURE_BLOCKS);

        for _ in 0..TX_CAL_PREFILL_BLOCKS {
            read_calibration_block(rx, &mut rx_block, 250_000)?;
        }

        for block_idx in 0..(TX_CAL_SETTLE_BLOCKS + TX_CAL_CAPTURE_BLOCKS) {
            read_calibration_block(rx, &mut rx_block, 250_000)?;
            if block_idx >= TX_CAL_SETTLE_BLOCKS {
                capture.extend_from_slice(&rx_block);
            }
        }
        if capture.len() < block_len {
            return Err(format!("short RX-only calibration baseline: {} samples", capture.len()));
        }
        Ok(capture)
    })();

    let activate_result = tx.activate(None);
    match (result, activate_result) {
        (Ok(capture), Ok(())) => Ok(capture),
        (Err(err), Ok(())) => Err(err),
        (Ok(_), Err(err)) => Err(format!("reactivate TX stream after RX-only calibration baseline: {}", err)),
        (Err(err), Err(activate_err)) => Err(format!(
            "{}; additionally failed to reactivate TX stream after RX-only calibration baseline: {}",
            err, activate_err
        )),
    }
}

fn read_calibration_block(rx: &mut soapysdr::RxStream<StreamType>, out: &mut [StreamType], timeout_us: i64) -> Result<(), String> {
    let mut offset = 0;
    while offset < out.len() {
        let len = rx
            .read(&mut [&mut out[offset..]], timeout_us)
            .map_err(|e| format!("read calibration block: {}", e))?;
        if len == 0 {
            return Err("read calibration block: no samples".to_string());
        }
        offset += len;
    }
    Ok(())
}

fn args_from_pairs(pairs: &[(String, String)]) -> soapysdr::Args {
    let mut args = soapysdr::Args::new();
    for (key, value) in pairs {
        args.set(key.as_str(), value.as_str());
    }
    args
}

fn stream_period_samples(args: &[(String, String)]) -> Option<usize> {
    args.iter()
        .find(|(key, _)| key == "period")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .filter(|period| *period > 0)
}

fn quantized_tone_hz(target_hz: f64, sample_rate: f64, block_len: usize) -> f64 {
    if !target_hz.is_finite() || !sample_rate.is_finite() || sample_rate <= 0.0 || block_len < 4 {
        return target_hz;
    }
    let max_bin = (block_len / 2).saturating_sub(1).max(1) as f64;
    let bin = (target_hz * block_len as f64 / sample_rate).round().clamp(1.0, max_bin);
    bin * sample_rate / block_len as f64
}

fn dft_at(samples: &[StreamType], sample_rate: f64, freq_hz: f64) -> ComplexF64 {
    dft_at_centered(samples, ComplexF64::ZERO, sample_rate, freq_hz)
}

fn dft_at_centered(samples: &[StreamType], dc: ComplexF64, sample_rate: f64, freq_hz: f64) -> ComplexF64 {
    if samples.is_empty() || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return ComplexF64::ZERO;
    }
    let mut re = 0.0;
    let mut im = 0.0;
    for (n, sample) in samples.iter().enumerate() {
        let phase = -2.0 * std::f64::consts::PI * freq_hz * n as f64 / sample_rate;
        let (sin, cos) = phase.sin_cos();
        let sample_re = sample.re as f64 - dc.re;
        let sample_im = sample.im as f64 - dc.im;
        re += sample_re * cos - sample_im * sin;
        im += sample_re * sin + sample_im * cos;
    }
    let scale = samples.len() as f64;
    ComplexF64 {
        re: re / scale,
        im: im / scale,
    }
}

fn calibration_capture_quality_ok(meas: &CalibrationMeasurement) -> bool {
    meas.snr_db >= TX_CAL_ACCEPT_MIN_SNR_DB
        && meas.floor_drift_db.abs() <= TX_CAL_ACCEPT_MAX_FLOOR_DRIFT_DB
        && meas.max_component_abs <= TX_CAL_MAX_COMPONENT_ABS
        && meas.clipped_fraction <= TX_CAL_MAX_CLIPPED_FRACTION
}

fn dc_calibration_accepted(reference: &CalibrationMeasurement, candidate: &CalibrationMeasurement) -> bool {
    let improvement = reference.carrier_leakage_dbc - candidate.carrier_leakage_dbc;
    calibration_capture_quality_ok(candidate)
        && candidate.carrier_leakage_dbc <= TX_CAL_ACCEPT_CARRIER_DBC
        && (improvement >= TX_CAL_ACCEPT_MIN_IMPROVEMENT_DB || candidate.carrier_leakage_dbc <= TX_CAL_GOOD_CARRIER_DBC)
}

fn iq_calibration_accepted(reference: &CalibrationMeasurement, candidate: &CalibrationMeasurement) -> bool {
    let image_improvement = candidate.image_rejection_db - reference.image_rejection_db;
    calibration_capture_quality_ok(candidate)
        && candidate.carrier_leakage_dbc <= TX_CAL_ACCEPT_CARRIER_DBC
        && candidate.image_rejection_db >= TX_CAL_ACCEPT_IMAGE_REJECTION_DB
        && (image_improvement >= TX_CAL_ACCEPT_MIN_IMPROVEMENT_DB || candidate.image_rejection_db >= TX_CAL_GOOD_IMAGE_REJECTION_DB)
        && candidate.evm_proxy_pct <= TX_CAL_ACCEPT_MAX_EVM_PROXY_PCT
        && candidate.evm_proxy_pct <= reference.evm_proxy_pct + TX_CAL_ACCEPT_MAX_EVM_WORSEN_PCT
}

fn with_axis_delta(
    mut coeffs: TxCalibrationCoefficients,
    axis: CalibrationAxis,
    delta: f64,
    limits: &TxCalibrationLimits,
) -> TxCalibrationCoefficients {
    match axis {
        CalibrationAxis::DcI => coeffs.dc_i = (coeffs.dc_i + delta).clamp(-limits.tx_dc_abs_max, limits.tx_dc_abs_max),
        CalibrationAxis::DcQ => coeffs.dc_q = (coeffs.dc_q + delta).clamp(-limits.tx_dc_abs_max, limits.tx_dc_abs_max),
        CalibrationAxis::IqI => coeffs.iq_i = (coeffs.iq_i + delta).clamp(-limits.tx_iq_abs_max, limits.tx_iq_abs_max),
        CalibrationAxis::IqQ => coeffs.iq_q = (coeffs.iq_q + delta).clamp(-limits.tx_iq_abs_max, limits.tx_iq_abs_max),
    }
    coeffs
}

fn iq_calibration_score(meas: &CalibrationMeasurement) -> f64 {
    -meas.image_rejection_db + meas.evm_proxy_pct * 0.10 + meas.carrier_leakage_dbc.max(-80.0) * 0.01
}

fn gains_fingerprint(gains: &[(String, f64)]) -> String {
    let mut gains = gains.to_vec();
    gains.sort_by(|a, b| a.0.cmp(&b.0));
    gains
        .into_iter()
        .map(|(name, value)| format!("{}={:.2}", name, value))
        .collect::<Vec<_>>()
        .join(",")
}

fn device_gains_fingerprint(
    dev: &soapysdr::Device,
    direction: soapysdr::Direction,
    channel: usize,
    configured_gains: &[(String, f64)],
) -> String {
    let gains = configured_gains
        .iter()
        .map(|(name, fallback)| {
            let value = dev.gain_element(direction, channel, name.as_str()).unwrap_or(*fallback);
            (name.clone(), value)
        })
        .collect::<Vec<_>>();
    gains_fingerprint(&gains)
}

fn unix_secs_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn temperature_sensor_reads_supported(settings_name: &str) -> bool {
    !matches!(settings_name, "SXceiver" | "µCell")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime_calibration_config() -> TxCalibrationRuntimeConfig {
        TxCalibrationRuntimeConfig {
            name: "SXceiver".to_string(),
            live_rx_carrier_hz: 431_362_500.0,
            live_tx_carrier_hz: 438_362_500.0,
            live_rx_center_hz: 431_342_500.0,
            live_tx_center_hz: 438_362_512.0,
            sample_rate_hz: 600_000.0,
            rx_ch: 0,
            tx_ch: 0,
            rx_ant: "RX".to_string(),
            tx_ant: "TX".to_string(),
            pa_setting: "AUTO".to_string(),
            rx_gains_fingerprint: "LNA=42.00,PGA=16.00".to_string(),
            tx_gains_fingerprint: "DAC=9.00,MIXER=30.00".to_string(),
        }
    }

    fn calibration_file_for_runtime(runtime: &TxCalibrationRuntimeConfig) -> TxCalibrationFile {
        TxCalibrationFile {
            schema_version: 1,
            status: "calibrated".to_string(),
            device: TxCalibrationDevice {
                name: runtime.name.clone(),
                tx_frequency_hz: runtime.live_tx_carrier_hz,
                rx_frequency_hz: runtime.live_rx_carrier_hz,
                tx_center_frequency_hz: runtime.live_tx_center_hz,
                rx_center_frequency_hz: runtime.live_rx_center_hz,
                calibration_frequency_hz: runtime.live_tx_center_hz,
                duplex_shift_hz: runtime.live_tx_carrier_hz - runtime.live_rx_carrier_hz,
                sample_rate_hz: runtime.sample_rate_hz,
                tx_channel: runtime.tx_ch,
                rx_channel: runtime.rx_ch,
                tx_antenna: runtime.tx_ant.clone(),
                rx_antenna: runtime.rx_ant.clone(),
                loopback_source: "rx_internal_lb".to_string(),
                pa_setting: runtime.pa_setting.clone(),
                tx_gains_fingerprint: runtime.tx_gains_fingerprint.clone(),
                rx_gains_fingerprint: runtime.rx_gains_fingerprint.clone(),
            },
            report: TxCalibrationReport {
                accepted: true,
                accepted_dc: true,
                accepted_iq: false,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn tx_calibration_accepts_matching_resolved_runtime_config() {
        let runtime = runtime_calibration_config();
        let calibration = calibration_file_for_runtime(&runtime);

        validate_tx_calibration_matches_runtime_config(&calibration, &runtime).expect("matching resolved runtime config must be accepted");
    }

    #[test]
    fn tx_calibration_rejects_stale_runtime_config() {
        let runtime = runtime_calibration_config();
        let mut calibration = calibration_file_for_runtime(&runtime);
        calibration.device.rx_frequency_hz -= 25_000.0;
        calibration.device.tx_gains_fingerprint = "DAC=6.00,MIXER=30.00".to_string();

        let err = validate_tx_calibration_matches_runtime_config(&calibration, &runtime)
            .expect_err("stale RX/TX/gain calibration must be rejected");

        assert!(err.contains("rx_frequency_hz"));
        assert!(err.contains("tx_gains"));
    }

    #[test]
    fn tx_calibration_rejects_stale_pa_runtime_config() {
        let runtime = runtime_calibration_config();
        let mut calibration = calibration_file_for_runtime(&runtime);
        calibration.device.pa_setting = "OFF".to_string();

        let err =
            validate_tx_calibration_matches_runtime_config(&calibration, &runtime).expect_err("stale PA calibration must be rejected");

        assert!(err.contains("pa_setting"));
    }

    #[test]
    fn sxceiver_like_devices_skip_runtime_temperature_reads() {
        assert!(!temperature_sensor_reads_supported("SXceiver"));
        assert!(!temperature_sensor_reads_supported("µCell"));
        assert!(temperature_sensor_reads_supported("LimeSDR Mini v2"));
        assert!(temperature_sensor_reads_supported("USRP B210"));
    }
}

// Messy logic related to opening a device follows...

/// Struct to temporarily hold stuff related to opening and detecting a device
struct OpenedDevice {
    dev_args: soapysdr::Args,
    dev: soapysdr::Device,
    driver_key: String,
    hardware_key: String,
    detected_device: SupportedDevice,
    soapyremote_used: bool,
}

fn open_given_device(dev_args: soapysdr::Args) -> Result<OpenedDevice, soapysdr::Error> {
    let soapyremote_used = match dev_args.get("driver") {
        Some("remote") => true,
        _ => false,
    };
    tracing::info!("Trying to open a device with arguments: {}", dev_args);

    let dev_args_copy: soapysdr::Args = dev_args.iter().collect();
    let dev = match soapysdr::Device::new(dev_args_copy) {
        Ok(dev) => dev,
        Err(err) => {
            tracing::info!("Skipping a SoapySDR device because opening failed: {}", err);
            return Err(err);
        }
    };
    let driver_key = dev.driver_key().unwrap_or_default();
    let hardware_key = dev.hardware_key().unwrap_or_default();

    // Check whether the device is supported
    if let Some(detected_device) = SupportedDevice::detect(&driver_key, &hardware_key) {
        tracing::info!(
            "Found supported device with driver_key '{}' hardware_key '{}'",
            driver_key,
            hardware_key
        );
        Ok(OpenedDevice {
            dev_args,
            dev,
            driver_key,
            hardware_key,
            detected_device,
            soapyremote_used,
        })
    } else {
        tracing::info!(
            "Skipping unsupported device with driver_key '{}' hardware_key '{}'",
            driver_key,
            hardware_key
        );
        Err(soapysdr::Error {
            code: soapysdr::ErrorCode::NotSupported,
            message: "Unsupported device".to_string(),
        })
    }
}

/// Enumerate devices and find the first supported device
fn find_supported_device(filter_args: soapysdr::Args) -> Result<OpenedDevice, soapysdr::Error> {
    for dev_args in soapycheck!("Enumerate SoapySDR devices", soapysdr::enumerate(filter_args)) {
        //tracing::info!("Trying to open a device with arguments: {}", args_formatted);
        match open_given_device(dev_args) {
            Ok(opened_device) => return Ok(opened_device),
            Err(_) => {}
        }
    }
    return Err(soapysdr::Error {
        code: soapysdr::ErrorCode::NotSupported,
        message: "No supported devices found".to_string(),
    });
}

/// Open a given device if argument string is given,
/// automatically find the first supported device if not.
fn open_device(soapy_cfg: &CfgSoapySdr, mode: StackMode) -> Result<(soapysdr::Device, SdrSettings), soapysdr::Error> {
    let mut opened_device = if let Some(arg_string) = &soapy_cfg.device {
        open_given_device(arg_string.as_str().into())
    } else {
        find_supported_device(soapysdr::Args::new())
    }?;

    let mut sdr_settings = match SdrSettings::get_settings(&soapy_cfg, opened_device.detected_device, mode) {
        Ok(sdr_settings) => sdr_settings,
        Err(soapy_settings::Error::InvalidConfiguration) => {
            return Err(soapysdr::Error {
                code: soapysdr::ErrorCode::Other,
                message: "Invalid SDR device configuration".to_string(),
            });
        }
    };

    if opened_device.soapyremote_used {
        // Getting hardware time may be too slow over SoapyRemote
        tracing::info!("SoapyRemote detected, forcing use_get_hardware_time=false");
        sdr_settings.use_get_hardware_time = false;
    }

    tracing::info!("Using settings: {:?}", sdr_settings);

    // If additional driver arguments are needed, reopen the device with them
    if sdr_settings.dev_args.len() > 0 {
        // Append additional arguments from settings
        for (key, value) in &sdr_settings.dev_args {
            opened_device.dev_args.set(key.as_str(), value.as_str());
        }

        tracing::info!("Reopening device with additional arguments: {}", opened_device.dev_args);

        // Make sure device gets closed first. Not sure if needed.
        std::mem::drop(opened_device.dev);
        opened_device.dev = soapycheck!(
            "open SoapySDR device with additional arguments",
            soapysdr::Device::new(opened_device.dev_args)
        );
        // Make sure it is still the same device.
        // Unlikely to change, but who knows if a device got connected just in between,
        // or if the device broke from first opening attempt and something else got opened
        // because device arguments were not precise enough to guarantee a specific device.
        let new_driver_key = opened_device.dev.driver_key().unwrap_or_default();
        let new_hardware_key = opened_device.dev.hardware_key().unwrap_or_default();
        if new_driver_key != opened_device.driver_key || new_hardware_key != opened_device.hardware_key {
            tracing::info!(
                "Expected the same driver_key='{}' hardware_key='{}' after reopen, got driver_key='{}' hardware_key='{}'",
                opened_device.driver_key,
                opened_device.hardware_key,
                new_driver_key,
                new_hardware_key
            );
            return Err(soapysdr::Error {
                code: soapysdr::ErrorCode::Other,
                message: "Reopened a different device".to_string(),
            });
        }
    }

    Ok((opened_device.dev, sdr_settings))
}
