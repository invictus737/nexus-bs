// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

// ---------------------------------------------------------------------------
// TelemetryEvent — concrete enum sent through the channel
//
// Small, hot-path variants are inline (no heap allocation).
// Rare / large variants use heap-allocated payload so the enum stays small.
// ---------------------------------------------------------------------------

use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use crate::health::HealthSnapshot;

/// TelemetryEvent enum sent by a TetraEntity through the TelemetrySink
/// then, serializable by any codec for transmission over the network,
/// using any Transport.
#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub enum TelemetryEvent {
    /// MS registered on BS
    MsRegistration { issi: u32 },
    /// MS deregistered. Also counts as deregistration for all groups.
    MsDeregistration { issi: u32 },
    /// MS affiliated to groups
    MsGroupAttach { issi: u32, gssis: Vec<u32> },
    /// Full snapshot of all currently attached groups — emitted after any attach/detach
    MsGroupsSnapshot { issi: u32, gssis: Vec<u32> },
    /// MS detached from groups
    MsGroupDetach { issi: u32, gssis: Vec<u32> },
    /// RSSI measurement for a known MS (dBFS)
    MsRssi { issi: u32, rssi_dbfs: f32 },
    /// Group call started
    GroupCallStarted { call_id: u16, gssi: u32, caller_issi: u32, ts: u8 },
    /// Group call ended
    GroupCallEnded { call_id: u16, gssi: u32 },
    /// Speaker changed on active group call
    GroupCallSpeakerChanged { call_id: u16, gssi: u32, speaker_issi: u32 },
    /// Individual (P2P) call started
    IndividualCallStarted {
        call_id: u16,
        calling_issi: u32,
        called_issi: u32,
        simplex: bool,
        ts: u8,
        secondary_ts: Option<u8>,
    },
    /// Individual call ended
    IndividualCallEnded { call_id: u16 },
    /// Energy saving mode updated for MS (0=StayAlive, 1=Eg1..7=Eg7)
    MsEnergySaving {
        issi: u32,
        mode: u8,
        frame: Option<u8>,
        multiframe: Option<u8>,
    },
    /// Brew (TetraPack) backhaul connection status changed
    BrewConnected { connected: bool, server_version: u8 },
    /// SDS message activity (local delivery or group)
    SdsActivity { source_issi: u32, dest_issi: u32 },
    /// Voice frame activity on a traffic timeslot (UL or DL)
    TsVoiceActivity { ts: u8 },
    /// Fast visual feed for the RF dashboard: spectrum + constellation + RMS/peak.
    /// Emitted ~5 times per second so spectrum/constellation/waterfall feel fluid.
    /// Cheap to compute (FFT + magnitude). Constellation symbol recovery is the
    /// only non-trivial bit, but it's still well under 1 ms on a Pi 5.
    ///
    /// Works on any radio (LimeSDR, SXceiver, µCell, USRP, Pluto) because the
    /// analysis runs on the complex baseband samples Nexus-BS generates
    /// internally, BEFORE they reach the SDR — no receive-side feedback required.
    TxVisual {
        sample_rate: f32,
        center_freq_hz: f64,
        /// RMS amplitude in dBFS (0 = full scale). Shown smoothed in the UI.
        rms_dbfs: f32,
        /// Peak amplitude in dBFS. Shown smoothed in the UI.
        peak_dbfs: f32,
        /// 512-bin spectrum, magnitude in tenths of a dB (i16 to keep the WS message compact).
        spectrum_db_tenths: Vec<i16>,
        /// Recovered symbol-rate IQ samples, interleaved I,Q,I,Q,... scaled to fit i16.
        constellation_iq: Vec<i16>,
    },
    /// Slow, expensive signal-quality metrics. Emitted once per second so the
    /// numbers on the RF dashboard sit still instead of flickering. The values
    /// are aggregates over multiple symbol blocks — averaging is done in the DSP,
    /// not in the browser, so a single message is already a stable reading.
    TxQuality {
        /// Peak-to-Average Power Ratio in dB. Typical π/4-DQPSK target ≈ 3.5-4 dB.
        /// Higher values indicate clipping or modulation problems.
        papr_db: f32,
        /// Pre-SDR DSP Error Vector Magnitude estimate as a percentage. This is
        /// useful for visual health, but it is not transmitted RF EVM and must
        /// not be used as TETRA modulation-accuracy evidence.
        evm_pct: f32,
        /// Mean of the I component across captured samples (DC offset on I).
        /// Should be ~0; non-zero indicates DC bias from the SDR front-end.
        dc_offset_i: f32,
        /// Mean of the Q component (DC offset on Q).
        dc_offset_q: f32,
        /// Amplitude imbalance between I and Q in dB. 0 dB = balanced.
        iq_amplitude_imbalance_db: f32,
        /// Phase imbalance between I and Q in degrees (deviation from ideal 90°).
        iq_phase_imbalance_deg: f32,
        /// Carrier (LO) leakage in dB relative to total signal power. More negative
        /// is better. Direct-conversion SDRs (SXceiver, µCell) typically show this.
        carrier_leakage_db: f32,
        /// Occupied bandwidth in Hz — width containing 99% of total power.
        occupied_bandwidth_hz: f32,
    },
    /// SDR hardware health snapshot. Emitted every ~5 seconds. Some fields may be
    /// absent (None) depending on what the radio exposes via Soapy.
    SdrHealth {
        /// Sensor reading in °C if the device exposes it (LimeSDR ✓, USRP ✓, Pluto ✓, SXceiver ✗)
        temperature_c: Option<f32>,
        /// Actually-set TX gain values per gain stage, queried back from the radio.
        /// Vec of (name, dB) — e.g. [("PAD", 40.0), ("IAMP", 6.0)] for LimeSDR.
        tx_gains: Vec<(String, f32)>,
        /// Same for RX gain stages.
        rx_gains: Vec<(String, f32)>,
    },
    /// Host system health: temperatures, voltages, currents, power consumption.
    /// Aggregated from whatever sysfs exposes — works on RPi 5 (full PMIC),
    /// x86 with RAPL, RPi 4 (CPU temp only), laptops (battery), and degrades
    /// gracefully on anything else. Emitted every ~2 seconds.
    SysHealth {
        /// Estimated total system power draw in watts, if we can compute it.
        /// On RPi 5 this is the sum of all PMIC rails (~5-12W typical).
        /// On x86 it's RAPL package power (CPU only, not whole system).
        /// On laptop/battery devices it's the battery discharge rate.
        /// None when no power-capable source is detected.
        total_power_w: Option<f32>,
        /// Individual sensor readings, in display order.
        sensors: Vec<SysSensor>,
    },
    /// Operational health snapshot. Emitted by the observe-only health
    /// monitor at a fixed low rate; dashboard/telemetry consumers must treat it
    /// as status data, not a control path.
    HealthSnapshot(HealthSnapshot),
}

/// A single host-system sensor reading. Kept flat for easy JSON serialisation
/// and rendering in tables.
#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub struct SysSensor {
    /// Human label, e.g. "CPU package", "VDD_CORE", "Battery", "NVMe".
    pub name: String,
    /// What kind of measurement this is — drives the unit and the display column.
    pub kind: SysSensorKind,
    /// Numeric value in the unit implied by `kind`.
    pub value: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SysSensorKind {
    /// Degrees Celsius
    Temperature,
    /// Volts
    Voltage,
    /// Amperes
    Current,
    /// Watts (rail power = V × I, or RAPL energy/time)
    Power,
}
