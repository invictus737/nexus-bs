//! Common things used by both modulator and demodulator.

use super::dsp_types::*;

/// TS 100 392-2 clauses 5.5 and 5.6 define the phase-modulation pulse as
/// a square-root raised cosine with alpha = 0.35 and a linear-phase filter.
pub const CHANNEL_FILTER_ROLL_OFF: RealSample = 0.35;

/// Preserve the old TX pulse energy so the filter-quality improvement does not
/// also change the SDR/PA drive level.
pub const CHANNEL_FILTER_REFERENCE_ENERGY: RealSample = 0.249_927_21;

/// Square-root raised cosine channel filter taps for 4 samples/symbol.
///
/// The FIR implementation stores one half of an even-length symmetric impulse:
/// the full impulse is `taps.rev() + taps`, so the group delay is 47.5 samples.
/// Taps are generated from the TS 100 392-2 clause 5.5 SRRC equation on the
/// half-sample grid `(n + 0.5) / 4`, truncated to 96 taps and scaled to
/// `CHANNEL_FILTER_REFERENCE_ENERGY`.
pub const CHANNEL_FILTER_TAPS: [RealSample; 48] = [
    0.26493451,
    0.19999303,
    0.10062770,
    0.00998109,
    -0.04013558,
    -0.04405054,
    -0.01982437,
    0.00642361,
    0.01744118,
    0.01213265,
    0.00071211,
    -0.00609447,
    -0.00488425,
    0.00028615,
    0.00345359,
    0.00220781,
    -0.00124009,
    -0.00315397,
    -0.00195379,
    0.00079812,
    0.00240048,
    0.00165401,
    -0.00032550,
    -0.00152291,
    -0.00100962,
    0.00039474,
    0.00117382,
    0.00064545,
    -0.00051604,
    -0.00110651,
    -0.00060786,
    0.00040085,
    0.00092239,
    0.00053523,
    -0.00027552,
    -0.00068617,
    -0.00035874,
    0.00029222,
    0.00058376,
    0.00025525,
    -0.00031976,
    -0.00055673,
    -0.00024826,
    0.00026700,
    0.00047997,
    0.00021861,
    -0.00021584,
    -0.00038373,
];
