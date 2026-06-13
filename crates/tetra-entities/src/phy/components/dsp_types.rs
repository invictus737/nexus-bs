// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

//! Data types used for signal processing

use num_complex;

pub type RealSample = f32;
pub use std::f32::consts as sample_consts;

pub type ComplexSample = num_complex::Complex<RealSample>;

pub type SampleCount = i64;
