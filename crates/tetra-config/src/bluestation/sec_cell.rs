use serde::Deserialize;
use std::collections::HashMap;

use tetra_core::ranges::{SortedDisjointSsiRanges, SsiRange};
use toml::Value;

/// Default SwMI/broadcast source SSI for periodic SDS broadcasts.
pub const DEFAULT_BROADCAST_SOURCE_SSI: u32 = 0x00FF_FFFF;
const MAX_24_BIT_SSI: u32 = 0x00FF_FFFF;
/// Nexus-BS operator default for energy economy mode.
///
/// EN 300 392-2 clauses 16.7.1, 16.10.9 and 16.10.10 define the SwMI/MM
/// signalling for energy economy mode allocation, while clause 23.7.6/table
/// 23.9 defines the resulting receive cycle. `auto` is a local Nexus-BS policy:
/// accept the MS-requested StayAlive/EG1..EG7 value and do not impose a
/// BS-initiated energy economy allocation when the MS did not ask for one.
pub const ENERGY_SAVING_MODE_AUTO: u8 = u8::MAX;
pub const DEFAULT_ENERGY_SAVING_MODE: u8 = ENERGY_SAVING_MODE_AUTO;
/// Default SDS-TL protocol identifier for text-message SDS broadcasts.
///
/// EN 300 392-2 table 29.21 assigns 0x82 to text messaging. Vendor/home-screen
/// compatibility PIDs such as 0xDC remain supported when explicitly configured.
pub const DEFAULT_SDS_TEXT_PROTOCOL_ID: u8 = 0x82;

/// Local support predicate for SDS-TL transport PIDs used by SDS-TRANSFER.
///
/// EN 300 392-2 clause 29.4.1 and table 29.21 reserve 0x00..=0x7F for
/// applications outside the SDS-TL data transport service. 0xFF is the
/// extension escape; this stack does not implement extended protocol IDs yet.
pub fn is_supported_sds_tl_transport_protocol_id(protocol_id: u8) -> bool {
    (0x80..=0xFE).contains(&protocol_id)
}

/// Local support predicate for text-like periodic/live SDS broadcasts.
///
/// EN 300 392-2 table 29.21 puts standard SDS-TL applications and
/// user-defined applications in the same high-bit PID space. The periodic/live
/// paths here build a text-style SDS-TL TRANSFER envelope, not opaque WAP/WCMP,
/// location, concatenation, or other standard application payloads. Keep those
/// standard non-text PIDs fail-closed on these text-style config/dashboard
/// paths, while preserving explicit vendor/user-defined broadcast PIDs such as
/// 0xDC.
pub fn is_supported_periodic_sds_protocol_id(protocol_id: u8) -> bool {
    protocol_id == DEFAULT_SDS_TEXT_PROTOCOL_ID || (0xC0..=0xFE).contains(&protocol_id)
}

/// Text coding scheme for Home Mode Display SDS payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum HomeModeDisplaySdsTextCodingScheme {
    /// ISO-8859-1 8-bit Latin alphabet
    LATIN,
    /// UCS-2 / UTF-16BE for Unicode text
    UTF16,
}

/// Compiled Home Mode Display configuration (present when `[cell_info.home_mode_display]` exists).
#[derive(Debug, Clone)]
pub struct CfgHomeModeDisplay {
    /// ISSI used as the source address of the broadcast SDS.
    pub source_issi: u32,
    /// Broadcast interval in TDMA multiframes (1 multiframe = 18 frames = 72 timeslots).
    pub interval_multiframes: u32,
    /// SDS Type4 protocol identifier byte. Default: 0x82 (ETSI SDS-TL text messaging).
    pub protocol_id: u8,
    /// Text coding scheme prepended to user data. LATIN = ISO-8859-1, UTF16 = UCS-2/UTF-16BE.
    pub text_coding_scheme: HomeModeDisplaySdsTextCodingScheme,
    /// Text to broadcast (UTF-8 source, encoded per text_coding_scheme on TX).
    pub text: String,
}

/// Serde DTO for `[cell_info.home_mode_display]` config block.
#[derive(Default, Deserialize)]
pub struct HomeModeDisplayDto {
    pub source_issi: Option<u32>,
    #[serde(alias = "interval_frames")]
    pub interval_multiframes: Option<u32>,
    pub protocol_id: Option<u8>,
    pub text_coding_scheme: Option<HomeModeDisplaySdsTextCodingScheme>,
    pub text: Option<String>,
}

/// Service details for a neighbor cell — mirrors BsServiceDetails but for config parsing.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CfgBsServiceDetails {
    #[serde(default)]
    pub registration: bool,
    #[serde(default)]
    pub deregistration: bool,
    #[serde(default)]
    pub priority_cell: bool,
    #[serde(default)]
    pub no_minimum_mode: bool,
    #[serde(default)]
    pub migration: bool,
    #[serde(default)]
    pub system_wide_services: bool,
    #[serde(default)]
    pub voice_service: bool,
    #[serde(default)]
    pub circuit_mode_data_service: bool,
    #[serde(default)]
    pub sndcp_service: bool,
    #[serde(default)]
    pub aie_service: bool,
    #[serde(default)]
    pub advanced_link: bool,
}

/// Configuration for a single CA neighbor cell, included in D-NWRK-BROADCAST.
/// Per ETSI EN 300 392-2 clause 18.5.17 / Table 18.64.
#[derive(Debug, Clone, Deserialize)]
pub struct CfgNeighborCellCa {
    /// 5 bits — cell identifier within the CA cluster (0-31)
    pub cell_identifier_ca: u8,
    /// 2 bits — cell reselection types supported (0-3)
    pub cell_reselection_types_supported: u8,
    /// 1 bit — true if this neighbor is time-synchronized with us
    pub neighbor_cell_synchronized: bool,
    /// 2 bits — current load indicator (0=low, 3=high)
    pub cell_load_ca: u8,
    /// 12 bits — main carrier number of the neighbor cell (0-4095)
    pub main_carrier_number: u16,

    /// Optional: carrier number extension (10 bits, 0-1023)
    pub main_carrier_number_extension: Option<u16>,
    /// Optional: MCC of the neighbor (10 bits, 0-1023)
    pub mcc: Option<u16>,
    /// Optional: MNC of the neighbor (14 bits, 0-16383)
    pub mnc: Option<u16>,
    /// Optional: location area of the neighbor (14 bits, 0-16383)
    pub location_area: Option<u16>,
    /// Optional: max MS TX power allowed in neighbor cell (3 bits, 0-7)
    pub maximum_ms_transmit_power: Option<u8>,
    /// Optional: minimum RX level for access (4 bits, 0-15)
    pub minimum_rx_access_level: Option<u8>,
    /// Optional: subscriber class mask (16 bits)
    pub subscriber_class: Option<u16>,
    /// Optional: BS service details for the neighbor cell
    pub bs_service_details: Option<CfgBsServiceDetails>,
    /// Optional: timeshare/security parameters (5 bits, 0-31)
    pub timeshare_cell_information_or_security_parameters: Option<u8>,
    /// Optional: TDMA frame offset relative to this cell (6 bits, 0-63)
    pub tdma_frame_offset: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct CfgCellInfo {
    // 2 bits, from 18.4.2.1 D-MLE-SYNC
    pub neighbor_cell_broadcast: u8,
    // 2 bits, from 18.4.2.1 D-MLE-SYNC
    pub late_entry_supported: bool,

    /// 12 bits, from MAC SYSINFO
    pub main_carrier: u16,
    /// 4 bits, from MAC SYSINFO
    pub freq_band: u8,
    /// Offset in Hz from 25kHz aligned carrier. Options: 0, 6250, -6250, 12500 Hz
    /// Represented as 0-3 in SYSINFO
    pub freq_offset_hz: i16,
    /// Index in duplex setting table. Sent in SYSINFO. Maps to a specific duplex spacing in Hz.
    /// Custom spacing can be provided optionally by setting
    pub duplex_spacing_id: u8,
    /// Custom duplex spacing in Hz, for users that use a modified, non-standard duplex spacing table.
    pub custom_duplex_spacing: Option<u32>,
    /// 1 bits, from MAC SYSINFO
    pub reverse_operation: bool,

    // 14 bits, from 18.4.2.2 D-MLE-SYSINFO
    pub location_area: u16,
    // 16 bits, from 18.4.2.2 D-MLE-SYSINFO
    pub subscriber_class: u16,

    // 1-bit service flags
    pub registration: bool,
    pub deregistration: bool,
    pub priority_cell: bool,
    pub no_minimum_mode: bool,
    pub migration: bool,
    pub system_wide_services: bool,
    pub voice_service: bool,
    pub circuit_mode_data_service: bool,
    pub sndcp_service: bool,
    pub aie_service: bool,
    pub advanced_link: bool,

    // From SYNC
    pub system_code: u8,
    pub colour_code: u8,
    pub sharing_mode: u8,
    pub ts_reserved_frames: u8,
    pub u_plane_dtx: bool,
    pub frame_18_ext: bool,

    pub ms_txpwr_max_cell: u8,

    pub local_ssi_ranges: SortedDisjointSsiRanges,
    /// Explicit local ISSIs to probe after a Nexus-BS process restart.
    ///
    /// These are operator bootstrap hints, not an ETSI air-interface IE. MM
    /// uses them to send ETSI EN 300 392-2 clause 16.4.4
    /// D-LOCATION-UPDATE-COMMAND PDUs so still-camped radios re-register and
    /// report group affiliations.
    pub restart_recovery_issis: Vec<u32>,
    /// Optional GSSI provisioning policy for MM group attachment. None keeps
    /// compatibility with dynamic scan-list systems and accepts any plain GSSI;
    /// Some(ranges) rejects attach requests outside the configured ranges as
    /// unknown group identities.
    pub allowed_gssi_ranges: Option<SortedDisjointSsiRanges>,

    /// IANA timezone name (e.g. "Europe/Amsterdam"). When set, enables D-NWRK-BROADCAST
    /// time broadcasting so MSs can synchronize their clocks.
    pub timezone: Option<String>,

    /// Periodic automatic broadcast of Home Mode Display SDS using a configurable Type4 PID.
    /// Enabled when `Some`, i.e. `[cell_info.home_mode_display]` exists in config.
    /// Broadcasts the configured text to all MSs once per interval as a D-SDS-DATA to GSSI 0xFFFFFF.
    pub home_mode_display: Option<CfgHomeModeDisplay>,

    /// Optional supplemental periodic SDS broadcast with a custom PID.
    /// Useful for sending ETSI text PID 0x82 alongside an explicitly configured vendor PID.
    /// Configured via `[cell_info.sds_broadcast]`. Uses the same structure as home_mode_display.
    pub sds_broadcast: Option<CfgHomeModeDisplay>,

    /// Neighbor cells to include in D-NWRK-BROADCAST for cell reselection.
    /// Up to 7 entries. MSs use this list to find alternative cells when signal degrades.
    pub neighbor_cells_ca: Vec<CfgNeighborCellCa>,

    /// Group call hangtime in seconds: how long an idle group call circuit stays open
    /// after the last speaker releases the floor, before the call is torn down.
    /// During hangtime, any MS can retake the floor without a new D-SETUP/D-CONNECT cycle.
    /// Default: 5 seconds. Range: 0–300.
    pub hangtime_secs: u32,

    /// Maximum active call duration in seconds (ETSI T310 equivalent, EN 300 392-2 §14.9.1).
    /// After this time the BS sends D-RELEASE regardless of call activity.
    /// Shorter values free up timeslots faster when MS leaves coverage without disconnecting.
    /// Default: 120 seconds (2 minutes). Range: 30–300.
    pub call_timeout_secs: u32,

    /// UL inactivity timeout in seconds: if no voice frames are received from the transmitting
    /// MS for this duration, the BS forces TX-CEASED and enters hangtime.
    /// Must be above T.213 (1s) to tolerate DTX and brief RF fading.
    /// Default: 3 seconds. Range: 1–30.
    pub ul_inactivity_secs: u32,

    /// Enable CMCE transmission interruption using D-TX INTERRUPT.
    /// Default: false. When false, the SwMI queues or rejects requests instead
    /// of pre-empting an MS that currently has transmit permission.
    pub transmission_interruption_enabled: bool,

    /// Force local private P2P calls to use on/off-hook signalling even when
    /// the calling MS requests direct through-connect.
    ///
    /// This is a local compatibility override for terminals that render
    /// direct-setup private simplex calls as not answered. Default: false.
    pub force_private_p2p_hook_signalling: bool,

    /// Enable the Nexus-BS legacy GSSI compatibility profile.
    ///
    /// This is a local compatibility mode for older TETRA terminals that fail
    /// repeated same-speaker hangtime retakes on an already assigned group
    /// traffic channel. When enabled, no-queue local group overs are released
    /// after D-TX CEASED instead of being kept for fast hangtime retake.
    /// Default: false.
    pub legacy_gssi_group_call: bool,

    /// Local SwMI periodic-registration watchdog interval in seconds.
    /// This is not ETSI T351: EN 300 392-2 §16.11.1.1 defines T351 as the
    /// 10 s MS-side registration response timer.
    /// MS must re-register within this interval or be deregistered by the BS.
    /// 0 = disabled — MS registrations never expire.
    /// Default: 0 (disabled). Valid range when non-zero: 60–86400.
    pub periodic_registration_secs: u32,

    /// SwMI energy economy allocation.
    /// 255 = local auto policy: accept the MS-requested mode and do not impose.
    /// 0 = StayAlive, 1..=7 = EG1..EG7 per EN 300 392-2 table 16.39.
    /// When explicitly non-zero/non-auto, the BS allocates the configured EG
    /// and lower layers schedule per the negotiated monitoring window.
    pub energy_saving_mode: u8,

    /// Remote control via SDS U-STATUS to ISSI 9999. None = disabled.
    pub sds_command_control: Option<CfgSdsCommandControl>,
}

#[derive(Default, Deserialize)]
pub struct CellInfoDto {
    pub main_carrier: u16,
    pub freq_band: u8,
    pub freq_offset: i16,
    pub duplex_spacing: u8,
    pub reverse_operation: bool,
    pub custom_duplex_spacing: Option<u32>,

    pub location_area: u16,

    pub neighbor_cell_broadcast: Option<u8>,
    pub late_entry_supported: Option<bool>,
    pub subscriber_class: Option<u16>,
    pub registration: Option<bool>,
    pub deregistration: Option<bool>,
    pub priority_cell: Option<bool>,
    pub no_minimum_mode: Option<bool>,
    pub migration: Option<bool>,
    pub system_wide_services: Option<bool>,
    pub voice_service: Option<bool>,
    pub circuit_mode_data_service: Option<bool>,
    pub sndcp_service: Option<bool>,
    pub aie_service: Option<bool>,
    pub advanced_link: Option<bool>,

    pub system_code: Option<u8>,
    pub colour_code: Option<u8>,
    pub sharing_mode: Option<u8>,
    pub ts_reserved_frames: Option<u8>,
    pub u_plane_dtx: Option<bool>,
    pub frame_18_ext: Option<bool>,

    pub ms_txpwr_max_cell: Option<u8>,

    pub local_ssi_ranges: Option<Vec<(u32, u32)>>,
    pub restart_recovery_issis: Option<Vec<u32>>,
    pub allowed_gssi_ranges: Option<Vec<(u32, u32)>>,

    pub timezone: Option<String>,

    /// Home Mode Display periodic SDS broadcast. Enabled by presence of this sub-section.
    pub home_mode_display: Option<HomeModeDisplayDto>,

    /// Supplemental SDS broadcast with custom PID. Enabled by presence of this sub-section.
    pub sds_broadcast: Option<HomeModeDisplayDto>,

    /// Neighbor cells for D-NWRK-BROADCAST. Up to 7 entries.
    /// Parsed separately in parsing.rs from toml::Value to avoid serde flatten conflict.
    #[serde(skip)]
    pub neighbor_cells_ca: Vec<CfgNeighborCellCa>,

    /// Group call hangtime in seconds. Default: 5.
    pub hangtime_secs: Option<u32>,

    /// Active call timeout (T310) in seconds. Default: 120.
    pub call_timeout_secs: Option<u32>,

    /// UL inactivity timeout in seconds. Default: 3.
    pub ul_inactivity_secs: Option<u32>,

    /// Enable CMCE group-call transmission interruption using D-TX INTERRUPT.
    /// Default: false.
    #[serde(alias = "group_call_preemption")]
    #[serde(alias = "call_preemptive")]
    pub transmission_interruption_enabled: Option<bool>,

    /// Local compatibility override for private P2P direct setup.
    #[serde(alias = "private_p2p_force_hook")]
    #[serde(alias = "private_call_force_hook")]
    pub force_private_p2p_hook_signalling: Option<bool>,

    /// Local compatibility mode for older terminals that do not reliably retake
    /// same-speaker GSSI floor during hangtime.
    #[serde(alias = "legacy_group_call")]
    #[serde(alias = "legacy_group_same_speaker_retake")]
    pub legacy_gssi_group_call: Option<bool>,

    /// Periodic registration interval in seconds. 0 = disabled. Default: 0.
    pub periodic_registration_secs: Option<u32>,

    /// Energy economy allocation: "auto", "stay_alive"/0 or "eg1".."eg7"/1..7.
    #[serde(alias = "energy_economy_group")]
    pub energy_saving_mode: Option<Value>,

    /// Remote control via SDS U-STATUS. Optional section.
    pub sds_command_control: Option<SdsCommandControlDto>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

pub fn cell_dto_to_cfg(ci: CellInfoDto) -> Result<CfgCellInfo, String> {
    let home_mode_display = ci
        .home_mode_display
        .map(|h| home_mode_display_dto_to_cfg("cell_info.home_mode_display", h))
        .transpose()?;
    let sds_broadcast = ci
        .sds_broadcast
        .map(|h| home_mode_display_dto_to_cfg("cell_info.sds_broadcast", h))
        .transpose()?;
    if ci.sndcp_service.unwrap_or(false) {
        // EN 300 392-2 clauses 17.2 and 18.5.21 advertise SNDCP packet-data
        // service through local BS service details. This stack currently only
        // routes MLE SNDCP SDUs to a stub SNDCP entity, so accepting true here
        // would advertise a WAP/IP bearer that cannot be served.
        return Err("cell_info.sndcp_service=true is not supported: SNDCP/WAP packet-data bearer is not implemented".into());
    }

    let local_ssi_ranges = ci
        .local_ssi_ranges
        .map(SortedDisjointSsiRanges::from_vec_tuple)
        .unwrap_or(default_tetrapack_local_ranges());
    let restart_recovery_issis = parse_restart_recovery_issis(ci.restart_recovery_issis, &local_ssi_ranges)?;

    Ok(CfgCellInfo {
        main_carrier: ci.main_carrier,
        freq_band: ci.freq_band,
        freq_offset_hz: ci.freq_offset,
        duplex_spacing_id: ci.duplex_spacing,
        reverse_operation: ci.reverse_operation,
        custom_duplex_spacing: ci.custom_duplex_spacing,
        location_area: ci.location_area,
        neighbor_cell_broadcast: ci.neighbor_cell_broadcast.unwrap_or(0),
        late_entry_supported: ci.late_entry_supported.unwrap_or(false),
        subscriber_class: ci.subscriber_class.unwrap_or(65535), // All subscriber classes allowed
        registration: ci.registration.unwrap_or(true),
        deregistration: ci.deregistration.unwrap_or(true),
        priority_cell: ci.priority_cell.unwrap_or(false),
        no_minimum_mode: ci.no_minimum_mode.unwrap_or(false),
        migration: ci.migration.unwrap_or(false),
        system_wide_services: ci.system_wide_services.unwrap_or(false),
        voice_service: ci.voice_service.unwrap_or(true),
        circuit_mode_data_service: ci.circuit_mode_data_service.unwrap_or(false),
        sndcp_service: false,
        aie_service: ci.aie_service.unwrap_or(false),
        advanced_link: ci.advanced_link.unwrap_or(false),
        system_code: ci.system_code.unwrap_or(3), // 3 = ETSI EN 300 392-2 V3.1.1
        colour_code: ci.colour_code.unwrap_or(0),
        sharing_mode: ci.sharing_mode.unwrap_or(0),
        ts_reserved_frames: ci.ts_reserved_frames.unwrap_or(0),
        u_plane_dtx: ci.u_plane_dtx.unwrap_or(false),
        frame_18_ext: ci.frame_18_ext.unwrap_or(false),
        ms_txpwr_max_cell: ci.ms_txpwr_max_cell.unwrap_or(4), // 30 dBm (1W), Table 18.57
        local_ssi_ranges,
        restart_recovery_issis,
        allowed_gssi_ranges: parse_allowed_gssi_ranges(ci.allowed_gssi_ranges)?,
        timezone: ci.timezone,
        home_mode_display,
        sds_broadcast,
        neighbor_cells_ca: ci.neighbor_cells_ca,
        hangtime_secs: ci.hangtime_secs.unwrap_or(5).clamp(0, 300),
        call_timeout_secs: {
            let v = ci.call_timeout_secs.unwrap_or(120);
            if v == 0 { 0 } else { v.clamp(30, 86400) }
        },
        ul_inactivity_secs: ci.ul_inactivity_secs.unwrap_or(3).clamp(1, 30),
        transmission_interruption_enabled: ci.transmission_interruption_enabled.unwrap_or(false),
        force_private_p2p_hook_signalling: ci.force_private_p2p_hook_signalling.unwrap_or(false),
        legacy_gssi_group_call: ci.legacy_gssi_group_call.unwrap_or(false),
        periodic_registration_secs: {
            let v = ci.periodic_registration_secs.unwrap_or(0);
            if v == 0 { 0 } else { v.clamp(60, 86400) }
        },
        energy_saving_mode: parse_energy_saving_mode(ci.energy_saving_mode)?,
        sds_command_control: ci
            .sds_command_control
            .map(|dto| sds_command_control_dto_to_cfg("cell_info.sds_command_control", dto))
            .transpose()?,
    })
}

fn parse_restart_recovery_issis(issis: Option<Vec<u32>>, local_ssi_ranges: &SortedDisjointSsiRanges) -> Result<Vec<u32>, String> {
    let mut issis = issis.unwrap_or_default();
    issis.sort_unstable();
    issis.dedup();

    for issi in &issis {
        if *issi > MAX_24_BIT_SSI {
            return Err(format!(
                "cell_info.restart_recovery_issis contains {issi}, exceeding 24-bit ISSI maximum {MAX_24_BIT_SSI}"
            ));
        }
        if !local_ssi_ranges.as_slice().is_empty() && !local_ssi_ranges.contains(*issi) {
            return Err(format!(
                "cell_info.restart_recovery_issis contains {issi}, which is outside cell_info.local_ssi_ranges"
            ));
        }
    }

    Ok(issis)
}

pub fn validate_neighbor_sndcp_service_is_not_advertised(neighbor_cells: &[CfgNeighborCellCa]) -> Result<(), String> {
    for (index, cell) in neighbor_cells.iter().enumerate() {
        if cell.bs_service_details.as_ref().is_some_and(|details| details.sndcp_service) {
            // EN 300 392-2 clauses 17.2, 18.5.17 and table 18.26 define the
            // SNDCP service bit as packet-data service availability. The WAP
            // MVP in this stack is SDS-based; until a real SNDCP/IP bearer is
            // implemented, neighbour-cell broadcasts must not advertise SNDCP.
            return Err(format!(
                "cell_info.neighbor_cells_ca[{index}].bs_service_details.sndcp_service=true is not supported: SNDCP/WAP packet-data bearer is not implemented"
            ));
        }
    }

    Ok(())
}

fn home_mode_display_dto_to_cfg(section: &str, h: HomeModeDisplayDto) -> Result<CfgHomeModeDisplay, String> {
    let protocol_id = h.protocol_id.unwrap_or(DEFAULT_SDS_TEXT_PROTOCOL_ID);
    if !is_supported_periodic_sds_protocol_id(protocol_id) {
        return Err(format!(
            "{section}.protocol_id must be 0x82 text messaging or user-defined SDS-TL PID 0xC0..0xFE for text-style SDS; standard non-text SDS-TL application PIDs, including WAP/WCMP, require their own payload encoders and are rejected here"
        ));
    }

    Ok(CfgHomeModeDisplay {
        source_issi: h.source_issi.unwrap_or(DEFAULT_BROADCAST_SOURCE_SSI),
        interval_multiframes: h.interval_multiframes.unwrap_or(96),
        protocol_id,
        text_coding_scheme: h.text_coding_scheme.unwrap_or(HomeModeDisplaySdsTextCodingScheme::LATIN),
        text: h.text.unwrap_or_default(),
    })
}

fn parse_allowed_gssi_ranges(ranges: Option<Vec<(u32, u32)>>) -> Result<Option<SortedDisjointSsiRanges>, String> {
    let Some(mut ranges) = ranges else {
        return Ok(None);
    };

    // EN 300 392-2 tables 16.57 and 16.58 carry GSSI in 24 bits. Rejecting
    // invalid operator provisioning here keeps malformed group identities out
    // of MM attach/affiliation processing and avoids panics in the sorted-range
    // helper.
    ranges.sort_by_key(|(start, _)| *start);
    let mut previous_end = None;
    for (start, end) in &ranges {
        if start > end {
            return Err(format!("cell_info.allowed_gssi_ranges has reversed range [{start}, {end}]"));
        }
        if *end > MAX_24_BIT_SSI {
            return Err(format!(
                "cell_info.allowed_gssi_ranges end {end} exceeds 24-bit GSSI maximum {MAX_24_BIT_SSI}"
            ));
        }
        if let Some(prev_end) = previous_end
            && *start <= prev_end
        {
            return Err(format!(
                "cell_info.allowed_gssi_ranges has overlapping ranges ending at {prev_end} and starting at {start}"
            ));
        }
        previous_end = Some(*end);
    }

    Ok(Some(SortedDisjointSsiRanges::from_vec_tuple(ranges)))
}

fn parse_energy_saving_mode(value: Option<Value>) -> Result<u8, String> {
    let Some(value) = value else {
        return Ok(DEFAULT_ENERGY_SAVING_MODE);
    };

    match value {
        Value::Integer(v) if (0..=7).contains(&v) => Ok(v as u8),
        // EN 300 392-2 clause 16.10.9 defines Stay Alive as mode 0. Accept
        // explicit local TOML false as a safe off spelling, but keep true
        // invalid so EG is never enabled without selecting EG1..EG7.
        Value::Boolean(false) => Ok(0),
        Value::String(s) => {
            let normalized = s.trim().to_ascii_lowercase().replace(['-', ' '], "_");
            match normalized.as_str() {
                "auto" | "terminal" | "terminal_request" | "ms" | "ms_request" => Ok(ENERGY_SAVING_MODE_AUTO),
                "0" | "off" | "none" | "stayalive" | "stay_alive" => Ok(0),
                "1" | "eg1" => Ok(1),
                "2" | "eg2" => Ok(2),
                "3" | "eg3" => Ok(3),
                "4" | "eg4" => Ok(4),
                "5" | "eg5" => Ok(5),
                "6" | "eg6" => Ok(6),
                "7" | "eg7" => Ok(7),
                _ => Err("cell_info.energy_saving_mode must be auto, stay_alive/off/0, or eg1..eg7/1..7".to_string()),
            }
        }
        _ => Err("cell_info.energy_saving_mode must be auto, stay_alive/off/0, or eg1..eg7/1..7".to_string()),
    }
}

/// Default local SSI ranges are defined as 0-90 (inclusive), which fits the TetraPack configuration.
/// This helps prevent excessive flows of unroutable traffic to TetraPack, and can be overridden
/// by users if needed.
fn default_tetrapack_local_ranges() -> SortedDisjointSsiRanges {
    SortedDisjointSsiRanges::from_vec_ssirange(vec![SsiRange::new(0, 90)])
}

// ── SDS command control ────────────────────────────────────────────────────

/// A single SDS status code → action mapping for remote control via U-STATUS.
#[derive(Debug, Clone)]
pub struct CfgSdsCommandEntry {
    /// Pre-coded status value that triggers this action.
    pub status_code: u16,
    /// Action to execute: "restart", "shutdown", or "kick_all".
    pub action: String,
}

/// Remote control via SDS U-STATUS PDUs sent to ISSI 9999.
/// Only ISSIs listed in `authorized_issis` can trigger commands.
#[derive(Debug, Clone)]
pub struct CfgSdsCommandControl {
    pub authorized_issis: Vec<u32>,
    pub commands: Vec<CfgSdsCommandEntry>,
}

pub fn sds_command_control_dto_to_cfg(section: &str, dto: SdsCommandControlDto) -> Result<CfgSdsCommandControl, String> {
    if !dto.extra.is_empty() {
        return Err(format!(
            "Unrecognized fields in {section}: {:?}",
            dto.extra.keys().collect::<Vec<_>>()
        ));
    }

    let mut commands = Vec::with_capacity(dto.commands.len());
    for (index, entry) in dto.commands.into_iter().enumerate() {
        validate_sds_command_status_code(section, index, entry.status_code)?;
        commands.push(CfgSdsCommandEntry {
            status_code: entry.status_code,
            action: entry.action,
        });
    }

    Ok(CfgSdsCommandControl {
        authorized_issis: dto.authorized_issis,
        commands,
    })
}

fn validate_sds_command_status_code(section: &str, index: usize, status_code: u16) -> Result<(), String> {
    if status_code < 0x8000 {
        // EN 300 392-2 clause 14.8.34 table 14.72 reserves 0x0001..=0x7BFF
        // and maps 0x7C00..=0x7FFF to SDS-TL short reporting. Local remote
        // commands are a Nexus-BS extension, so they must stay inside the
        // TETRA network/user-specific range 0x8000..=0xFFFF.
        return Err(format!(
            "{section}.commands[{index}].status_code must be in ETSI network/user-specific range 32768..=65535"
        ));
    }

    Ok(())
}

#[derive(Default, Deserialize)]
pub struct SdsCommandEntryDto {
    pub status_code: u16,
    pub action: String,
}

#[derive(Default, Deserialize)]
pub struct SdsCommandControlDto {
    #[serde(default)]
    pub authorized_issis: Vec<u32>,
    #[serde(default)]
    pub commands: Vec<SdsCommandEntryDto>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}
