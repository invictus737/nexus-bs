use super::dsp_types::*;
use super::fir;
use super::modem_common::CHANNEL_FILTER_TAPS;

pub const TETRA_MODEM_SPS: usize = 4;
pub const TETRA_RMS_EVM_LIMIT: RealSample = 0.10;
pub const TETRA_PEAK_EVM_LIMIT: RealSample = 0.30;

const MIN_EVM_SYMBOLS: usize = 48;
const MAX_EVM_SYMBOLS: usize = 192;
const SEARCH_SYMBOL_OFFSETS: usize = 24;
const SEARCH_SAMPLES: usize = 96;
const TIMING_SUBSTEPS: usize = 4;
const THETA_SEARCH_STEPS: usize = 41;
const THETA_SEARCH_RANGE_RAD_PER_SYMBOL: RealSample = 0.05;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModulationAccuracy {
    /// RMS vector error as a fraction of fitted ideal symbol amplitude.
    pub rms_evm: RealSample,
    /// Peak vector error as a fraction of fitted ideal symbol amplitude.
    pub peak_evm: RealSample,
    /// RMS error of the differential phase transitions in degrees.
    pub differential_rms_deg: RealSample,
    /// Matched-filter sample position selected for the first compared symbol.
    pub timing_sample: RealSample,
    /// First reference symbol used in the comparison.
    pub reference_symbol_offset: usize,
    /// Residual carrier rotation fitted per symbol.
    pub frequency_rotation_rad_per_symbol: RealSample,
    pub dc_offset: ComplexSample,
    pub gain: ComplexSample,
    pub symbols_used: usize,
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    accuracy: ModulationAccuracy,
    score: RealSample,
}

pub fn pi4_dqpsk_symbols_from_bits(bits: &[u8]) -> Result<Vec<ComplexSample>, String> {
    if bits.len() % 2 != 0 {
        return Err("pi/4-DQPSK symbol generation requires an even bit count".to_string());
    }

    let mut phase: i32 = 0;
    let mut symbols = Vec::with_capacity(bits.len() / 2);
    for dibit in bits.chunks_exact(2) {
        phase = (phase
            + match (dibit[0] != 0, dibit[1] != 0) {
                (true, true) => -3,
                (true, false) => -1,
                (false, false) => 1,
                (false, true) => 3,
            })
        .rem_euclid(8);
        symbols.push(ideal_phase_point(phase));
    }
    Ok(symbols)
}

pub fn evaluate_modulation_accuracy(
    samples: &[ComplexSample],
    reference_symbols: &[ComplexSample],
    samples_per_symbol: usize,
) -> Option<ModulationAccuracy> {
    if samples_per_symbol == 0 || reference_symbols.len() < MIN_EVM_SYMBOLS || samples.len() < MIN_EVM_SYMBOLS * samples_per_symbol {
        return None;
    }

    let filtered = matched_filter(samples);
    let max_ref_offset = reference_symbols.len().saturating_sub(MIN_EVM_SYMBOLS).min(SEARCH_SYMBOL_OFFSETS);
    let mut best: Option<Candidate> = None;

    for reference_symbol_offset in 0..=max_ref_offset {
        let reference = &reference_symbols[reference_symbol_offset..];
        for timing_step in 0..=(SEARCH_SAMPLES * TIMING_SUBSTEPS) {
            let timing_sample = timing_step as RealSample / TIMING_SUBSTEPS as RealSample;
            let count = symbol_count_for_timing(filtered.len(), reference.len(), samples_per_symbol, timing_sample).min(MAX_EVM_SYMBOLS);
            if count < MIN_EVM_SYMBOLS {
                continue;
            }
            let Some(measured) = sample_symbol_points(&filtered, timing_sample, samples_per_symbol, count) else {
                continue;
            };
            for theta_step in 0..THETA_SEARCH_STEPS {
                let theta = theta_for_step(theta_step);
                let Some(candidate) =
                    fit_modulation_candidate(&measured, &reference[..count], theta, timing_sample, reference_symbol_offset)
                else {
                    continue;
                };
                match best {
                    Some(current) if candidate.score >= current.score => {}
                    _ => best = Some(candidate),
                }
            }
        }
    }

    best.map(|candidate| candidate.accuracy)
}

pub fn reconstructed_rrc_impulse_response() -> Vec<RealSample> {
    let mut taps = Vec::with_capacity(CHANNEL_FILTER_TAPS.len() * 2);
    taps.extend(CHANNEL_FILTER_TAPS.iter().rev().copied());
    taps.extend(CHANNEL_FILTER_TAPS.iter().copied());
    taps
}

fn matched_filter(samples: &[ComplexSample]) -> Vec<ComplexSample> {
    let mut filter = fir::FirComplexSym::new(CHANNEL_FILTER_TAPS.len());
    samples.iter().map(|sample| filter.sample(&CHANNEL_FILTER_TAPS, *sample)).collect()
}

fn symbol_count_for_timing(sample_len: usize, reference_len: usize, samples_per_symbol: usize, timing_sample: RealSample) -> usize {
    if sample_len < 2 || timing_sample >= (sample_len - 1) as RealSample {
        return 0;
    }
    let available = (((sample_len - 1) as RealSample - timing_sample) / samples_per_symbol as RealSample).floor() as usize + 1;
    available.min(reference_len)
}

fn sample_symbol_points(
    samples: &[ComplexSample],
    timing_sample: RealSample,
    samples_per_symbol: usize,
    count: usize,
) -> Option<Vec<ComplexSample>> {
    let mut out = Vec::with_capacity(count);
    for idx in 0..count {
        let position = timing_sample + idx as RealSample * samples_per_symbol as RealSample;
        out.push(linear_sample(samples, position)?);
    }
    Some(out)
}

fn fit_modulation_candidate(
    measured: &[ComplexSample],
    reference: &[ComplexSample],
    theta: RealSample,
    timing_sample: RealSample,
    reference_symbol_offset: usize,
) -> Option<Candidate> {
    debug_assert_eq!(measured.len(), reference.len());
    let n = measured.len();
    if n < MIN_EVM_SYMBOLS {
        return None;
    }

    let mut sum_x = ComplexSample::ZERO;
    let mut sum_y = ComplexSample::ZERO;
    let mut sum_conj_x_y = ComplexSample::ZERO;
    for (idx, (y, s)) in measured.iter().zip(reference.iter()).enumerate() {
        let x = *s * rot(theta * idx as RealSample);
        sum_x += x;
        sum_y += *y;
        sum_conj_x_y += x.conj() * *y;
    }

    let n_f = n as RealSample;
    let determinant = n_f * n_f - sum_x.norm_sqr();
    if determinant <= 1.0e-6 {
        return None;
    }

    let dc_offset = (sum_y * n_f - sum_x * sum_conj_x_y) / determinant;
    let gain = (sum_conj_x_y * n_f - sum_x.conj() * sum_y) / determinant;
    let gain_norm = gain.norm();
    if gain_norm <= 1.0e-5 {
        return None;
    }

    let mut err_power = 0.0;
    let mut peak_evm = 0.0_f32;
    let mut previous_corrected: Option<ComplexSample> = None;
    let mut previous_reference: Option<ComplexSample> = None;
    let mut diff_phase_err_power = 0.0;
    let mut diff_phase_err_count = 0usize;

    for (idx, (y, s)) in measured.iter().zip(reference.iter()).enumerate() {
        let theta_rot = rot(theta * idx as RealSample);
        let predicted = dc_offset + gain * *s * theta_rot;
        let err = *y - predicted;
        let evm = err.norm() / gain_norm;
        err_power += evm * evm;
        peak_evm = peak_evm.max(evm);

        let corrected = (*y - dc_offset) / gain * rot(-theta * idx as RealSample);
        if let (Some(prev_corr), Some(prev_ref)) = (previous_corrected, previous_reference) {
            let measured_delta = (corrected * prev_corr.conj()).arg();
            let reference_delta = (*s * prev_ref.conj()).arg();
            let phase_err = wrap_pi(measured_delta - reference_delta);
            diff_phase_err_power += phase_err * phase_err;
            diff_phase_err_count += 1;
        }
        previous_corrected = Some(corrected);
        previous_reference = Some(*s);
    }

    let rms_evm = (err_power / n_f).sqrt();
    let differential_rms_deg = if diff_phase_err_count > 0 {
        (diff_phase_err_power / diff_phase_err_count as RealSample).sqrt().to_degrees()
    } else {
        0.0
    };

    Some(Candidate {
        score: rms_evm + peak_evm * 0.01,
        accuracy: ModulationAccuracy {
            rms_evm,
            peak_evm,
            differential_rms_deg,
            timing_sample,
            reference_symbol_offset,
            frequency_rotation_rad_per_symbol: theta,
            dc_offset,
            gain,
            symbols_used: n,
        },
    })
}

fn linear_sample(samples: &[ComplexSample], position: RealSample) -> Option<ComplexSample> {
    if !position.is_finite() || position < 0.0 {
        return None;
    }
    let idx = position.floor() as usize;
    let next = idx.checked_add(1)?;
    if next >= samples.len() {
        return None;
    }
    let frac = position - idx as RealSample;
    Some(samples[idx] * (1.0 - frac) + samples[next] * frac)
}

fn theta_for_step(step: usize) -> RealSample {
    if THETA_SEARCH_STEPS <= 1 {
        return 0.0;
    }
    let norm = step as RealSample / (THETA_SEARCH_STEPS - 1) as RealSample;
    (norm * 2.0 - 1.0) * THETA_SEARCH_RANGE_RAD_PER_SYMBOL
}

fn ideal_phase_point(phase: i32) -> ComplexSample {
    let angle = phase as RealSample * sample_consts::FRAC_PI_4;
    ComplexSample {
        re: angle.cos(),
        im: angle.sin(),
    }
}

fn rot(angle: RealSample) -> ComplexSample {
    let (sin, cos) = angle.sin_cos();
    ComplexSample { re: cos, im: sin }
}

fn wrap_pi(angle: RealSample) -> RealSample {
    (angle + sample_consts::PI).rem_euclid(sample_consts::TAU) - sample_consts::PI
}

#[cfg(test)]
mod tests {
    use super::super::modem_common::{CHANNEL_FILTER_REFERENCE_ENERGY, CHANNEL_FILTER_ROLL_OFF};
    use super::*;

    fn phase_index(symbol: ComplexSample) -> i32 {
        ((symbol.arg() / sample_consts::FRAC_PI_4).round() as i32).rem_euclid(8)
    }

    fn deterministic_bits(symbols: usize) -> Vec<u8> {
        let mut state = 0x5eed_u32;
        let mut bits = Vec::with_capacity(symbols * 2);
        for _ in 0..symbols * 2 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            bits.push(((state >> 31) & 1) as u8);
        }
        bits
    }

    fn pulse_shape(symbols: &[ComplexSample], samples_per_symbol: usize) -> Vec<ComplexSample> {
        let mut filter = fir::FirComplexSym::new(CHANNEL_FILTER_TAPS.len());
        let total_samples = symbols.len() * samples_per_symbol + CHANNEL_FILTER_TAPS.len() * 4;
        let mut out = Vec::with_capacity(total_samples);
        for sample_idx in 0..total_samples {
            let input = if sample_idx % samples_per_symbol == 0 {
                symbols.get(sample_idx / samples_per_symbol).copied().unwrap_or(ComplexSample::ZERO)
            } else {
                ComplexSample::ZERO
            };
            out.push(filter.sample(&CHANNEL_FILTER_TAPS, input));
        }
        out
    }

    fn ideal_srrc_impulse(t_symbols: f64, alpha: f64) -> f64 {
        let singular = 1.0 / (4.0 * alpha);
        if t_symbols.abs() < 1.0e-12 {
            return 1.0 + alpha * (4.0 / std::f64::consts::PI - 1.0);
        }
        if (t_symbols.abs() - singular).abs() < 1.0e-10 {
            return alpha / 2.0_f64.sqrt()
                * ((1.0 + 2.0 / std::f64::consts::PI) * (std::f64::consts::PI / (4.0 * alpha)).sin()
                    + (1.0 - 2.0 / std::f64::consts::PI) * (std::f64::consts::PI / (4.0 * alpha)).cos());
        }

        let pi_t = std::f64::consts::PI * t_symbols;
        ((pi_t * (1.0 - alpha)).sin() + 4.0 * alpha * t_symbols * (pi_t * (1.0 + alpha)).cos())
            / (pi_t * (1.0 - (4.0 * alpha * t_symbols).powi(2)))
    }

    fn filter_energy(taps: &[RealSample]) -> RealSample {
        taps.iter().map(|tap| tap * tap).sum()
    }

    fn spectrum_energy_fraction(taps: &[RealSample], sample_rate_hz: f64, low_hz: f64, high_hz: f64) -> f64 {
        let bins = 4096usize;
        let mut selected = 0.0;
        let mut total = 0.0;

        for bin in 0..bins {
            let frequency = -sample_rate_hz * 0.5 + sample_rate_hz * (bin as f64 + 0.5) / bins as f64;
            let mut re = 0.0;
            let mut im = 0.0;
            for (sample_idx, tap) in taps.iter().enumerate() {
                let angle = -2.0 * std::f64::consts::PI * frequency * sample_idx as f64 / sample_rate_hz;
                re += *tap as f64 * angle.cos();
                im += *tap as f64 * angle.sin();
            }
            let power = re * re + im * im;
            total += power;
            let abs_frequency = frequency.abs();
            if abs_frequency >= low_hz && abs_frequency < high_hz {
                selected += power;
            }
        }

        selected / total
    }

    #[test]
    fn pi4_dqpsk_phase_transitions_match_clause_5_4_mapping() {
        let bits = [1, 1, 1, 0, 0, 0, 0, 1];
        let symbols = pi4_dqpsk_symbols_from_bits(&bits).expect("valid dibits");
        let phases: Vec<i32> = symbols.into_iter().map(phase_index).collect();

        assert_eq!(phases, vec![5, 4, 5, 0]);
    }

    #[test]
    fn channel_filter_taps_match_tetra_alpha_035_srrc_reference() {
        let alpha = CHANNEL_FILTER_ROLL_OFF as f64;
        let unscaled: Vec<f64> = (0..CHANNEL_FILTER_TAPS.len())
            .map(|tap_idx| ideal_srrc_impulse((tap_idx as f64 + 0.5) / TETRA_MODEM_SPS as f64, alpha))
            .collect();
        let unscaled_energy = 2.0 * unscaled.iter().map(|tap| tap * tap).sum::<f64>();
        let scale = (CHANNEL_FILTER_REFERENCE_ENERGY as f64 / unscaled_energy).sqrt();

        for (tap_idx, (actual, expected_unscaled)) in CHANNEL_FILTER_TAPS.iter().zip(unscaled.iter()).enumerate() {
            let expected = expected_unscaled * scale;
            assert!(
                (*actual as f64 - expected).abs() < 2.0e-7,
                "tap {} actual {} expected {}",
                tap_idx,
                actual,
                expected
            );
        }
    }

    #[test]
    fn channel_filter_preserves_tx_drive_energy() {
        let taps = reconstructed_rrc_impulse_response();
        let energy = filter_energy(&taps);

        assert!(
            (energy - CHANNEL_FILTER_REFERENCE_ENERGY).abs() < 1.0e-6,
            "filter energy {} expected {}",
            energy,
            CHANNEL_FILTER_REFERENCE_ENERGY
        );
    }

    #[test]
    fn rrc_cascade_has_low_symbol_spaced_isi() {
        let taps = reconstructed_rrc_impulse_response();
        assert_eq!(taps.len(), CHANNEL_FILTER_TAPS.len() * 2);
        let group_delay = (taps.len() as RealSample - 1.0) / 2.0;
        assert!((group_delay - (CHANNEL_FILTER_TAPS.len() as RealSample - 0.5)).abs() < 1.0e-6);

        let mut cascade = vec![0.0; taps.len() * 2 - 1];
        for (i, a) in taps.iter().enumerate() {
            for (j, b) in taps.iter().enumerate() {
                cascade[i + j] += a * b;
            }
        }
        let center = taps.len() - 1;
        let peak = cascade[center].abs().max(1.0e-12);
        let mut worst_isi = 0.0_f32;
        let span_symbols = (CHANNEL_FILTER_TAPS.len() / TETRA_MODEM_SPS) as i32;
        for offset_symbols in -span_symbols..=span_symbols {
            if offset_symbols == 0 {
                continue;
            }
            let idx = center as i32 + offset_symbols * TETRA_MODEM_SPS as i32;
            if idx >= 0 && (idx as usize) < cascade.len() {
                worst_isi = worst_isi.max(cascade[idx as usize].abs() / peak);
            }
        }

        assert!(
            worst_isi < 0.001,
            "SRRC cascade deterministic symbol-spaced ISI is {:.3}%",
            worst_isi * 100.0
        );
    }

    #[test]
    fn rrc_filter_limits_adjacent_channel_energy_proxy() {
        let taps = reconstructed_rrc_impulse_response();
        let adjacent_fraction = spectrum_energy_fraction(&taps, 72_000.0, 12_500.0, 25_000.0);

        assert!(
            adjacent_fraction < 5.0e-6,
            "SRRC adjacent-band energy proxy is {:.3} dB relative to total",
            10.0 * adjacent_fraction.max(1.0e-300).log10()
        );
    }

    #[test]
    fn modulation_accuracy_passes_clean_tx_filter_chain() {
        let bits = deterministic_bits(255);
        let symbols = pi4_dqpsk_symbols_from_bits(&bits).expect("valid dibits");
        let samples = pulse_shape(&symbols, TETRA_MODEM_SPS);
        let report = evaluate_modulation_accuracy(&samples, &symbols, TETRA_MODEM_SPS).expect("EVM report");

        assert!(report.rms_evm < 0.02, "RMS EVM {:.3}%", report.rms_evm * 100.0);
        assert!(report.peak_evm < 0.08, "peak EVM {:.3}%", report.peak_evm * 100.0);
        assert!(report.rms_evm < TETRA_RMS_EVM_LIMIT);
        assert!(report.peak_evm < TETRA_PEAK_EVM_LIMIT);
    }

    #[test]
    fn modulation_accuracy_estimates_out_dc_gain_phase_and_frequency_rotation() {
        let bits = deterministic_bits(255);
        let symbols = pi4_dqpsk_symbols_from_bits(&bits).expect("valid dibits");
        let clean = pulse_shape(&symbols, TETRA_MODEM_SPS);
        let theta = 0.010;
        let gain = ComplexSample { re: 1.1, im: 0.2 };
        let dc = ComplexSample { re: 0.08, im: -0.05 };
        let impaired: Vec<ComplexSample> = clean
            .iter()
            .enumerate()
            .map(|(idx, sample)| {
                let symbol_time = idx as RealSample / TETRA_MODEM_SPS as RealSample;
                dc + gain * *sample * rot(theta * symbol_time)
            })
            .collect();

        let report = evaluate_modulation_accuracy(&impaired, &symbols, TETRA_MODEM_SPS).expect("EVM report");

        assert!(report.rms_evm < 0.035, "RMS EVM {:.3}%", report.rms_evm * 100.0);
        assert!(
            (report.frequency_rotation_rad_per_symbol - theta).abs() <= 0.005,
            "estimated theta {} expected {}",
            report.frequency_rotation_rad_per_symbol,
            theta
        );
    }
}
