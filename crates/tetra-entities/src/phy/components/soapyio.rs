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
const TX_CAL_CAPTURE_SAMPLES: usize = 4096;
const TX_CAL_MIN_SNR_DB: f64 = 25.0;

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
    tx_gain: Vec<(String, f64)>,
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
        let tx_gain_configured = sdr_settings.tx_gain.clone();

        // Get PPM corrected freqs
        let (dl_corrected, _) = soapy_cfg.dl_freq_corrected();
        let (ul_corrected, _) = soapy_cfg.ul_freq_corrected();

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

            apply_configured_tx_calibration(&dev, tx_ch, soapy_cfg);
        }

        let mut rx_args = soapysdr::Args::new();
        for (key, value) in sdr_settings.rx_args {
            rx_args.set(key, value);
        }

        let mut tx_args = soapysdr::Args::new();
        for (key, value) in sdr_settings.tx_args {
            tx_args.set(key, value);
        }

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
            tx_gain: tx_gain_configured,
        })
    }

    pub fn run_tx_calibration(&mut self, calibration_path: &str) -> Result<TxCalibrationFile, String> {
        if !self.tx_enabled() || !self.rx_enabled() {
            return Err("TX calibration requires both RX and TX streams".to_string());
        }

        tracing::warn!(
            "SoapySDR: starting destructive TX DC/IQ calibration using RF loopback capture; path={}",
            calibration_path
        );

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

        self.deactivate_streams_for_calibration()?;
        self.dev
            .set_frequency(soapysdr::Direction::Rx, self.rx_ch, tx_freq, soapysdr::Args::new())
            .map_err(|e| format!("retune RX to TX frequency {:.0} Hz: {}", tx_freq, e))?;

        let result = self.run_tx_calibration_inner(calibration_path, tx_freq);

        let restore_result = self.restore_after_tx_calibration(original_rx_freq, rx_active_before, tx_active_before);
        if let Err(restore_err) = restore_result {
            tracing::error!("SoapySDR: TX calibration restore failed: {}", restore_err);
            if result.is_ok() {
                return Err(restore_err);
            }
        }

        result
    }

    fn run_tx_calibration_inner(&mut self, calibration_path: &str, tx_freq: f64) -> Result<TxCalibrationFile, String> {
        self.activate_streams_for_calibration()?;

        let tone = calibration_tone(self.tx_fs, TX_CAL_TONE_HZ, TX_CAL_TONE_AMPLITUDE, TX_CAL_CAPTURE_SAMPLES);
        let limits = TxCalibrationLimits::default();

        let baseline_dc = self.capture_calibration_baseline_dc()?;
        let neutral = TxCalibrationCoefficients::default();
        let reference_meas = self.capture_calibration_measurement(Some(baseline_dc), &tone, neutral)?;
        if reference_meas.snr_db < TX_CAL_MIN_SNR_DB {
            return Err(format!(
                "RF loopback signal too weak for calibration: SNR {:.1} dB < {:.1} dB",
                reference_meas.snr_db, TX_CAL_MIN_SNR_DB
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
                    let meas = self.capture_calibration_measurement(Some(baseline_dc), &tone, candidate)?;
                    if meas.carrier_leakage_dbc < axis_meas.carrier_leakage_dbc {
                        axis_best = candidate;
                        axis_meas = meas;
                    }
                }
                best = axis_best;
                best_meas = axis_meas;
            }
        }

        for step in [0.12, 0.06, 0.03, 0.015] {
            for axis in [CalibrationAxis::IqI, CalibrationAxis::IqQ] {
                let mut axis_best = best;
                let mut axis_meas = best_meas;
                let mut axis_score = iq_calibration_score(&axis_meas);
                for direction in [-1.0, 1.0] {
                    let candidate = with_axis_delta(best, axis, step * direction, &limits);
                    let meas = self.capture_calibration_measurement(Some(baseline_dc), &tone, candidate)?;
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

        // Re-measure final coefficients after all changes have settled.
        let final_meas = self.capture_calibration_measurement(Some(baseline_dc), &tone, best)?;
        let carrier_improvement = reference_meas.carrier_leakage_dbc - final_meas.carrier_leakage_dbc;
        let image_improvement = final_meas.image_rejection_db - reference_meas.image_rejection_db;
        let evm_improvement = reference_meas.evm_proxy_pct - final_meas.evm_proxy_pct;
        let accepted = carrier_improvement >= limits.min_carrier_improvement_db || image_improvement >= limits.min_image_improvement_db;
        let applied = if accepted { best } else { TxCalibrationCoefficients::default() };

        self.apply_tx_calibration_coefficients(applied, true, true)?;

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
                tx_frequency_hz: tx_freq,
                rx_frequency_hz: self.rx_center_frequency().unwrap_or(0.0),
                sample_rate_hz: self.tx_fs,
                tx_channel: self.tx_ch,
                rx_channel: self.rx_ch,
                tx_antenna: self.tx_ant.clone().unwrap_or_else(|| "auto".to_string()),
                rx_antenna: self.rx_ant.clone().unwrap_or_else(|| "auto".to_string()),
                tx_gains_fingerprint: gains_fingerprint(&self.tx_gain),
            },
            limits,
            reference: reference_meas.into_point("neutral_no_calibration", neutral),
            calibrated: final_meas.into_point("calibrated_candidate", best),
            applied,
            report: TxCalibrationReport {
                carrier_leakage_improvement_db: carrier_improvement,
                image_rejection_improvement_db: image_improvement,
                evm_proxy_improvement_pct: evm_improvement,
                accepted,
                summary: if accepted {
                    format!(
                        "accepted: carrier leak {:+.1} dB, image rejection {:+.1} dB, EVM proxy {:+.2} pp",
                        carrier_improvement, image_improvement, evm_improvement
                    )
                } else {
                    format!(
                        "rejected: improvement below thresholds; carrier {:+.1} dB, image {:+.1} dB",
                        carrier_improvement, image_improvement
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

    fn activate_streams_for_calibration(&mut self) -> Result<(), String> {
        if let Some(rx) = &mut self.rx {
            if !rx.active() {
                rx.activate(None)
                    .map_err(|e| format!("activate RX stream for calibration: {}", e))?;
            }
        }
        if let Some(tx) = &mut self.tx {
            if !tx.active() {
                tx.activate(None)
                    .map_err(|e| format!("activate TX stream for calibration: {}", e))?;
            }
        }
        Ok(())
    }

    fn restore_after_tx_calibration(&mut self, original_rx_freq: f64, rx_active: bool, tx_active: bool) -> Result<(), String> {
        self.deactivate_streams_for_calibration()?;
        self.dev
            .set_frequency(soapysdr::Direction::Rx, self.rx_ch, original_rx_freq, soapysdr::Args::new())
            .map_err(|e| format!("restore RX frequency {:.0} Hz: {}", original_rx_freq, e))?;
        if rx_active {
            if let Some(rx) = &mut self.rx {
                rx.activate(None).map_err(|e| format!("reactivate RX stream: {}", e))?;
            }
        }
        if tx_active {
            if let Some(tx) = &mut self.tx {
                tx.activate(None).map_err(|e| format!("reactivate TX stream: {}", e))?;
            }
        }
        self.initial_time = None;
        self.rx_next_count = 0;
        self.prev_time_ns = -1;
        Ok(())
    }

    fn capture_calibration_measurement(
        &mut self,
        baseline_dc: Option<ComplexF64>,
        tone: &[StreamType],
        coeffs: TxCalibrationCoefficients,
    ) -> Result<CalibrationMeasurement, String> {
        self.apply_tx_calibration_coefficients(coeffs, true, true)?;
        std::thread::sleep(std::time::Duration::from_millis(25));

        let tx = self.tx.as_mut().ok_or_else(|| "TX stream unavailable".to_string())?;
        tx.write_all(&[tone], None, false, 250_000)
            .map_err(|e| format!("write calibration tone: {}", e))?;

        let mut capture = vec![ComplexSample::ZERO; TX_CAL_CAPTURE_SAMPLES];
        let rx = self.rx.as_mut().ok_or_else(|| "RX stream unavailable".to_string())?;
        let len = rx
            .read(&mut [&mut capture[..]], 250_000)
            .map_err(|e| format!("read calibration capture: {}", e))?;
        if len < TX_CAL_CAPTURE_SAMPLES / 2 {
            return Err(format!("short calibration capture: {} samples", len));
        }
        capture.truncate(len);
        Ok(measure_calibration_capture(&capture, self.rx_fs, TX_CAL_TONE_HZ, baseline_dc))
    }

    fn capture_calibration_baseline_dc(&mut self) -> Result<ComplexF64, String> {
        self.apply_tx_calibration_coefficients(TxCalibrationCoefficients::default(), true, true)?;
        std::thread::sleep(std::time::Duration::from_millis(25));
        let mut capture = vec![ComplexSample::ZERO; TX_CAL_CAPTURE_SAMPLES];
        let rx = self.rx.as_mut().ok_or_else(|| "RX stream unavailable".to_string())?;
        let len = rx
            .read(&mut [&mut capture[..]], 250_000)
            .map_err(|e| format!("read calibration baseline capture: {}", e))?;
        if len < TX_CAL_CAPTURE_SAMPLES / 2 {
            return Err(format!("short calibration baseline capture: {} samples", len));
        }
        capture.truncate(len);
        Ok(mean_complex(&capture))
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
struct CalibrationMeasurement {
    dc: ComplexF64,
    carrier_leakage_dbc: f64,
    image_rejection_db: f64,
    evm_proxy_pct: f64,
    signal_dbfs: f64,
    noise_floor_dbfs: f64,
    snr_db: f64,
}

impl CalibrationMeasurement {
    fn into_point(self, label: &str, tx: TxCalibrationCoefficients) -> TxCalibrationPoint {
        TxCalibrationPoint {
            label: label.to_string(),
            tx,
            carrier_leakage_dbc: self.carrier_leakage_dbc,
            image_rejection_db: self.image_rejection_db,
            evm_proxy_pct: self.evm_proxy_pct,
            signal_dbfs: self.signal_dbfs,
            noise_floor_dbfs: self.noise_floor_dbfs,
            snr_db: self.snr_db,
        }
    }
}

fn apply_configured_tx_calibration(dev: &soapysdr::Device, tx_ch: usize, soapy_cfg: &CfgSoapySdr) {
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
    sample_rate: f64,
    tone_hz: f64,
    baseline_dc: Option<ComplexF64>,
) -> CalibrationMeasurement {
    let dc_raw = mean_complex(samples);
    let dc = baseline_dc.map(|baseline| dc_raw.sub(baseline)).unwrap_or(dc_raw);
    let signal = dft_at(samples, sample_rate, tone_hz);
    let image = dft_at(samples, sample_rate, -tone_hz);
    let dc_amp = dc.abs().max(1.0e-12);
    let signal_amp = signal.abs().max(1.0e-12);
    let image_amp = image.abs().max(1.0e-12);
    let rms_power = samples.iter().map(|s| s.norm_sqr() as f64).sum::<f64>() / samples.len().max(1) as f64;
    let model_power = signal_amp * signal_amp + image_amp * image_amp + dc_amp * dc_amp;
    let noise_rms = (rms_power - model_power).max(1.0e-12).sqrt();

    CalibrationMeasurement {
        dc,
        carrier_leakage_dbc: 20.0 * (dc_amp / signal_amp).max(1.0e-12).log10(),
        image_rejection_db: 20.0 * (signal_amp / image_amp).max(1.0e-12).log10(),
        evm_proxy_pct: ((dc_amp / signal_amp).powi(2) + (image_amp / signal_amp).powi(2)).sqrt() * 100.0,
        signal_dbfs: 20.0 * signal_amp.max(1.0e-12).log10(),
        noise_floor_dbfs: 20.0 * noise_rms.max(1.0e-12).log10(),
        snr_db: 20.0 * (signal_amp / noise_rms).max(1.0e-12).log10(),
    }
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

fn dft_at(samples: &[StreamType], sample_rate: f64, freq_hz: f64) -> ComplexF64 {
    if samples.is_empty() || !sample_rate.is_finite() || sample_rate <= 0.0 {
        return ComplexF64::ZERO;
    }
    let mut re = 0.0;
    let mut im = 0.0;
    for (n, sample) in samples.iter().enumerate() {
        let phase = -2.0 * std::f64::consts::PI * freq_hz * n as f64 / sample_rate;
        let (sin, cos) = phase.sin_cos();
        re += sample.re as f64 * cos - sample.im as f64 * sin;
        im += sample.re as f64 * sin + sample.im as f64 * cos;
    }
    let scale = samples.len() as f64;
    ComplexF64 {
        re: re / scale,
        im: im / scale,
    }
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
