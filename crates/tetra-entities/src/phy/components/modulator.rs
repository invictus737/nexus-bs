use tetra_core::TdmaTime;

use tetra_pdus::phy::traits::rxtx_dev::TxSlotBits;

use crate::phy::components::dsp_types::*;
use crate::phy::components::modem_common::*;

/// Samples per symbol
const SPS: SampleCount = 4;

/// Samples per slot
const SAMPLES_SLOT: SampleCount = SPS * 255;

const FULL_CHANNEL_FILTER_TAPS: usize = CHANNEL_FILTER_TAPS.len() * 2;
const POLYPHASE_SYMBOL_HISTORY: usize = FULL_CHANNEL_FILTER_TAPS / SPS as usize;

/// Output sample rate
pub const SAMPLE_RATE: f64 = 18000.0 * SPS as f64;

#[derive(PartialEq)]
pub enum Mode {
    /// Downlink modulation.
    Dl,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phy::components::fir;

    fn phase_index(symbol: ComplexSample) -> i8 {
        ((symbol.im.atan2(symbol.re) / sample_consts::FRAC_PI_4).round() as i8).rem_euclid(8)
    }

    #[test]
    fn dqpsk_mapper_phase_transitions_match_tetra_table_5_1() {
        let mut mapper = DqpskMapper::new();

        let symbols = [
            mapper.symbol(true, true),
            mapper.symbol(true, false),
            mapper.symbol(false, false),
            mapper.symbol(false, true),
        ];
        let phases: Vec<i8> = symbols.into_iter().map(phase_index).collect();

        assert_eq!(phases, vec![5, 4, 5, 0]);
    }

    #[test]
    fn polyphase_pulse_shaper_matches_zero_stuffed_symmetric_fir() {
        let mut legacy = fir::FirComplexSym::new(CHANNEL_FILTER_TAPS.len());
        let mut polyphase = PolyphasePulseShaper::new();
        let mut mapper = DqpskMapper::new();

        for sample_idx in 0..520 {
            let phase = sample_idx % SPS as usize;
            let input = if phase == 0 {
                let symbol_idx = sample_idx / SPS as usize;
                mapper.symbol((symbol_idx & 1) != 0, (symbol_idx & 2) != 0)
            } else {
                ComplexSample::ZERO
            };

            let legacy_sample = legacy.sample(&CHANNEL_FILTER_TAPS, input);
            let polyphase_sample = polyphase.sample(phase, input);
            assert!(
                (legacy_sample - polyphase_sample).norm() < 1.0e-6,
                "sample {} legacy={} polyphase={}",
                sample_idx,
                legacy_sample,
                polyphase_sample
            );
        }
    }
}

pub struct Modulator {
    mode: Mode,
    /// Sample counter value at the beginning of hyperframe number 0
    reference_time: SampleCount,
    /// Polyphase pulse shaping filter.
    pulse: PolyphasePulseShaper,
    dqpsk: DqpskMapper,
}

pub enum Error {
    /// Modulator needs data for another slot
    /// before it can continue producing TX signal.
    NeedMoreData,
}

impl Modulator {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            reference_time: 0,
            pulse: PolyphasePulseShaper::new(),
            dqpsk: DqpskMapper::new(),
        }
    }

    /// Produce one output sample.
    pub fn sample(&mut self, sample_counter: SampleCount, tx_slot: &TxSlotBits) -> Result<ComplexSample, Error> {
        // Compensate for delay of pulse shaping filter in sample count
        let sample_counter = sample_counter + CHANNEL_FILTER_TAPS.len() as SampleCount;

        // Sample counter at beginning of current slot.
        // TODO: adjust self.reference_time when hyperframe number wraps to 0.
        // Now it breaks after 46 days.
        // This could also be further optimized by computing and storing it
        // only when a new slot becomes available.
        let slot_begin = self.reference_time + TdmaTime::to_int(tx_slot.time) as SampleCount * SAMPLES_SLOT;

        let mut sample = ComplexSample::ZERO;
        match self.mode {
            Mode::Dl => {
                let sample_in_slot = sample_counter - slot_begin;
                if sample_in_slot < 0 {
                    // Slot is in the future.
                    // Transmit silence until we reach the slot.
                } else if sample_in_slot >= SAMPLES_SLOT {
                    // Slot is in the past, so it has already been transmitted.
                    // Return and wait for data for the next slot to be available.
                    return Err(Error::NeedMoreData);
                } else if let Some(bits) = tx_slot.slot {
                    if sample_in_slot % SPS == 0 {
                        let symbol_i = (sample_in_slot / SPS) as usize;
                        sample = self.dqpsk.symbol(bits[symbol_i * 2] != 0, bits[symbol_i * 2 + 1] != 0);
                    }
                }
            }
        }

        Ok(self.pulse.sample(sample_counter.rem_euclid(SPS) as usize, sample))
    }
}

struct PolyphasePulseShaper {
    newest_symbol: usize,
    symbols: [ComplexSample; POLYPHASE_SYMBOL_HISTORY],
}

impl PolyphasePulseShaper {
    fn new() -> Self {
        Self {
            newest_symbol: 0,
            symbols: [ComplexSample::ZERO; POLYPHASE_SYMBOL_HISTORY],
        }
    }

    fn sample(&mut self, phase: usize, input: ComplexSample) -> ComplexSample {
        debug_assert!(phase < SPS as usize);

        if phase == 0 {
            self.newest_symbol = if self.newest_symbol == 0 {
                POLYPHASE_SYMBOL_HISTORY - 1
            } else {
                self.newest_symbol - 1
            };
            self.symbols[self.newest_symbol] = input;
        }

        let mut out = ComplexSample::ZERO;
        let mut delay = phase;
        let mut symbol_delay = 0;
        while delay < FULL_CHANNEL_FILTER_TAPS {
            out += self.symbol(symbol_delay) * full_channel_filter_tap(delay);
            delay += SPS as usize;
            symbol_delay += 1;
        }
        out
    }

    fn symbol(&self, delay_symbols: usize) -> ComplexSample {
        self.symbols[(self.newest_symbol + delay_symbols) % POLYPHASE_SYMBOL_HISTORY]
    }
}

fn full_channel_filter_tap(delay_samples: usize) -> RealSample {
    debug_assert!(delay_samples < FULL_CHANNEL_FILTER_TAPS);
    let half_len = CHANNEL_FILTER_TAPS.len();
    if delay_samples < half_len {
        CHANNEL_FILTER_TAPS[half_len - 1 - delay_samples]
    } else {
        CHANNEL_FILTER_TAPS[delay_samples - half_len]
    }
}

struct DqpskMapper {
    pub phase: i8,
}

impl DqpskMapper {
    pub fn new() -> Self {
        Self { phase: 0 }
    }

    #[allow(dead_code)]
    pub fn reset_phase(&mut self) {
        self.phase = 0;
    }

    pub fn symbol(&mut self, bit0: bool, bit1: bool) -> ComplexSample {
        self.phase = (self.phase
            + match (bit0, bit1) {
                (true, true) => -3,
                (true, false) => -1,
                (false, false) => 1,
                (false, true) => 3,
            })
            & 7;
        // Look-up table to map phase (in multiples of pi/4)
        // to constellation points. Generated in Python with:
        // import numpy as np
        // print(",\n".join("ComplexSample{ re: %9.6f, im: %9.6f }" % (v.real, v.imag) for v in np.exp(1j*np.linspace(0, np.pi*2, 8, endpoint=False))))
        const CONSTELLATION: [ComplexSample; 8] = [
            ComplexSample {
                re: 1.000000,
                im: 0.000000,
            },
            ComplexSample {
                re: 0.707107,
                im: 0.707107,
            },
            ComplexSample {
                re: 0.000000,
                im: 1.000000,
            },
            ComplexSample {
                re: -0.707107,
                im: 0.707107,
            },
            ComplexSample {
                re: -1.000000,
                im: 0.000000,
            },
            ComplexSample {
                re: -0.707107,
                im: -0.707107,
            },
            ComplexSample {
                re: -0.000000,
                im: -1.000000,
            },
            ComplexSample {
                re: 0.707107,
                im: -0.707107,
            },
        ];
        CONSTELLATION[self.phase as usize]
    }
}
