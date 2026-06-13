// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use serde::Deserialize;
use toml::Value;

use crate::bluestation::sec_cell::{
    CfgNeighborCellCa, SdsCommandControlDto, sds_command_control_dto_to_cfg, validate_neighbor_sndcp_service_is_not_advertised,
};
use crate::bluestation::{CellInfoDto, CfgControlDto, NetInfoDto, apply_control_patch, cell_dto_to_cfg, net_dto_to_cfg};

use super::config::{StackConfig, StackMode};
use super::sec_brew::{CfgBrewDto, apply_brew_patch};
use super::sec_dashboard::{CfgDashboardDto, apply_dashboard_patch};
use super::sec_health::{CfgHealthDto, apply_health_patch};
use super::sec_security::{CfgSecurityDto, apply_security_patch};
use super::sec_telemetry::{CfgTelemetryDto, apply_telemetry_patch};
use super::sec_wx::{CfgWxServiceDto, apply_wx_service_patch};
use super::{PhyIoDto, phy_dto_to_cfg};

/// Build `StackConfig` from a TOML configuration file
pub fn from_toml_str(toml_str: &str) -> Result<StackConfig, Box<dyn std::error::Error>> {
    // Parse once as raw Value so we can extract neighbor_cells_ca before
    // deserializing into typed DTOs. This avoids a conflict between serde's
    // #[flatten] HashMap (used for unrecognised-field detection) and an array-of-
    // tables field: the flatten map would capture neighbor_cells_ca as an opaque
    // Value, causing the "unrecognised field" check to fire.
    let mut raw: toml::Table = toml::from_str(toml_str)?;

    // Extract neighbor_cells_ca from cell_info before typed deserialisation.
    let neighbor_cells_ca: Vec<CfgNeighborCellCa> = raw
        .get_mut("cell_info")
        .and_then(|ci| {
            if let Value::Table(t) = ci {
                t.remove("neighbor_cells_ca")
            } else {
                None
            }
        })
        .map(|v| {
            // v is a Value::Array of Value::Table — deserialise via serde
            v.try_into::<Vec<toml::Table>>()
                .map_err(|e| format!("cell_info.neighbor_cells_ca: {}", e))
                .and_then(|tables| {
                    tables
                        .into_iter()
                        .enumerate()
                        .map(|(i, t)| {
                            Value::Table(t)
                                .try_into::<CfgNeighborCellCa>()
                                .map_err(|e| format!("cell_info.neighbor_cells_ca[{}]: {}", i, e))
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
        })
        .transpose()?
        .unwrap_or_default();

    if neighbor_cells_ca.len() > 7 {
        return Err("cell_info.neighbor_cells_ca: at most 7 entries allowed".into());
    }
    validate_neighbor_sndcp_service_is_not_advertised(&neighbor_cells_ca)?;

    // Extract sds_command_control from cell_info before typed deserialisation
    // (same reason as neighbor_cells_ca: serde #[flatten] would capture it as opaque Value)
    let sds_command_control_raw = raw.get_mut("cell_info").and_then(|ci| {
        if let Value::Table(t) = ci {
            t.remove("sds_command_control")
        } else {
            None
        }
    });

    // Now deserialise the (mutated) Value into the typed root — neighbor_cells_ca
    // has been removed so it will not appear in the flatten HashMap.
    let root: TomlConfigRoot = Value::Table(raw).try_into()?;

    // Various sanity checks
    let expected_config_version = "0.6";
    if !root.config_version.eq(expected_config_version) {
        return Err(format!(
            "Unrecognized config_version: {}, expect {}",
            root.config_version, expected_config_version
        )
        .into());
    }
    if !root.extra.is_empty() {
        return Err(format!("Unrecognized top-level fields: {:?}", sorted_keys(&root.extra)).into());
    }

    if !root.phy_io.extra.is_empty() {
        return Err(format!("Unrecognized fields: phy_io::{:?}", sorted_keys(&root.phy_io.extra)).into());
    }
    if let Some(ref soapy) = root.phy_io.soapysdr {
        let extra_keys = sorted_keys(&soapy.extra);
        let extra_keys_filtered = extra_keys
            .iter()
            .filter(|key| !(key.starts_with("rx_gain_") || key.starts_with("tx_gain_")))
            .collect::<Vec<&&str>>();
        if !extra_keys_filtered.is_empty() {
            return Err(format!("Unrecognized fields: phy_io.soapysdr::{:?}", extra_keys_filtered).into());
        }
    }
    if !root.net_info.extra.is_empty() {
        return Err(format!("Unrecognized fields in net_info: {:?}", sorted_keys(&root.net_info.extra)).into());
    }
    if !root.cell_info.extra.is_empty() {
        return Err(format!("Unrecognized fields in cell_info: {:?}", sorted_keys(&root.cell_info.extra)).into());
    }

    // Optional brew section
    if let Some(ref brew) = root.brew {
        if !brew.extra.is_empty() {
            return Err(format!("Unrecognized fields in brew config: {:?}", sorted_keys(&brew.extra)).into());
        }
    }

    // Optional telemetry section
    if let Some(ref telemetry) = root.telemetry {
        if !telemetry.extra.is_empty() {
            return Err(format!("Unrecognized fields in telemetry config: {:?}", sorted_keys(&telemetry.extra)).into());
        }
    }

    // Optional health section
    if let Some(ref health) = root.health {
        if !health.extra.is_empty() {
            return Err(format!("Unrecognized fields in health config: {:?}", sorted_keys(&health.extra)).into());
        }
    }

    // Build cell config, then inject the separately-parsed neighbor cells and sds_command_control
    let mut cell_cfg = cell_dto_to_cfg(root.cell_info)?;
    cell_cfg.neighbor_cells_ca = neighbor_cells_ca;
    if let Some(v) = sds_command_control_raw {
        let dto = v
            .try_into::<SdsCommandControlDto>()
            .map_err(|e| format!("cell_info.sds_command_control: {}", e))?;
        cell_cfg.sds_command_control = Some(sds_command_control_dto_to_cfg("cell_info.sds_command_control", dto)?);
    }

    // Build config from required and optional values
    let mut cfg = StackConfig {
        stack_mode: root.stack_mode,
        debug_log: root.debug_log,
        service_name: root.service_name,
        phy_io: phy_dto_to_cfg(root.phy_io)?,
        net: net_dto_to_cfg(root.net_info),
        cell: cell_cfg,
        brew: None,
        dashboard: None,
        telemetry: None,
        control: None,
        health: apply_health_patch(root.health.unwrap_or_default())?,
        security: apply_security_patch(root.security.unwrap_or_default()),
        wx_service: apply_wx_service_patch(root.wx_service.unwrap_or_default()),
    };

    if let Some(brew) = root.brew {
        cfg.brew = Some(apply_brew_patch(brew));
    }

    if let Some(dashboard) = root.dashboard {
        cfg.dashboard = Some(apply_dashboard_patch(dashboard)?);
    }

    if let Some(telemetry) = root.telemetry {
        cfg.telemetry = Some(apply_telemetry_patch(telemetry)?);
    }

    if let Some(command) = root.command {
        cfg.control = Some(apply_control_patch(command)?);
    }

    Ok(cfg)
}

/// Build `SharedConfig` from any reader.
pub fn from_reader<R: Read>(reader: R) -> Result<StackConfig, Box<dyn std::error::Error>> {
    let mut contents = String::new();
    let mut reader = BufReader::new(reader);
    reader.read_to_string(&mut contents)?;
    from_toml_str(&contents)
}

/// Build `SharedConfig` from a file path.
pub fn from_file<P: AsRef<Path>>(path: P) -> Result<StackConfig, Box<dyn std::error::Error>> {
    let f = File::open(path)?;
    let r = BufReader::new(f);
    let cfg = from_reader(r)?;
    Ok(cfg)
}

fn sorted_keys(map: &HashMap<String, Value>) -> Vec<&str> {
    let mut v: Vec<&str> = map.keys().map(|s| s.as_str()).collect();
    v.sort_unstable();
    v
}

/// ----------------------- DTOs for input shape -----------------------

#[derive(Deserialize)]
struct TomlConfigRoot {
    config_version: String,
    stack_mode: StackMode,
    debug_log: Option<String>,
    #[serde(default)]
    service_name: Option<String>,

    phy_io: PhyIoDto,
    net_info: NetInfoDto,
    cell_info: CellInfoDto,

    brew: Option<CfgBrewDto>,
    dashboard: Option<CfgDashboardDto>,
    telemetry: Option<CfgTelemetryDto>,
    command: Option<CfgControlDto>,
    health: Option<CfgHealthDto>,
    security: Option<CfgSecurityDto>,
    #[serde(rename = "wx_service")]
    wx_service: Option<CfgWxServiceDto>,

    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bluestation::sec_cell::ENERGY_SAVING_MODE_AUTO;

    fn minimal_toml(extra_cell: &str) -> String {
        format!(
            r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1584
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1
{}
"#,
            extra_cell
        )
    }

    fn minimal_soapy_toml(extra_soapy: &str) -> String {
        format!(
            r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "SoapySdr"

[phy_io.soapysdr]
rx_freq = 430000000.0
tx_freq = 435000000.0
{}

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1584
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1
"#,
            extra_soapy
        )
    }

    #[test]
    fn test_no_neighbor_cells() {
        let toml = minimal_toml("");
        let cfg = from_toml_str(&toml).expect("parse failed");
        assert_eq!(cfg.cell.neighbor_cells_ca.len(), 0);
    }

    #[test]
    fn test_two_neighbor_cells() {
        let toml = minimal_toml(
            r#"
neighbor_cell_broadcast = 2

[[cell_info.neighbor_cells_ca]]
cell_identifier_ca = 1
cell_reselection_types_supported = 0
neighbor_cell_synchronized = false
cell_load_ca = 0
main_carrier_number = 1585
mcc = 901
mnc = 9999
location_area = 1

[[cell_info.neighbor_cells_ca]]
cell_identifier_ca = 2
cell_reselection_types_supported = 0
neighbor_cell_synchronized = false
cell_load_ca = 1
main_carrier_number = 1586
"#,
        );
        let cfg = from_toml_str(&toml).expect("parse failed");
        assert_eq!(cfg.cell.neighbor_cells_ca.len(), 2);
        assert_eq!(cfg.cell.neighbor_cells_ca[0].cell_identifier_ca, 1);
        assert_eq!(cfg.cell.neighbor_cells_ca[0].main_carrier_number, 1585);
        assert_eq!(cfg.cell.neighbor_cells_ca[1].cell_identifier_ca, 2);
        assert_eq!(cfg.cell.neighbor_cells_ca[1].cell_load_ca, 1);
        assert_eq!(cfg.cell.neighbor_cell_broadcast, 2);
    }

    #[test]
    fn test_too_many_neighbor_cells_rejected() {
        // 8 entries — should fail validation
        let entries: String = (1u8..=8)
            .map(|i| format!(
                "\n[[cell_info.neighbor_cells_ca]]\ncell_identifier_ca = {}\ncell_reselection_types_supported = 0\nneighbor_cell_synchronized = false\ncell_load_ca = 0\nmain_carrier_number = {}\n",
                i, 1584 + i as u16
            ))
            .collect();
        let toml = minimal_toml(&entries);
        assert!(from_toml_str(&toml).is_err(), "should reject >7 neighbours");
    }

    #[test]
    fn test_unrecognized_cell_info_field_still_rejected() {
        let toml = minimal_toml("bogus_field = 42");
        assert!(from_toml_str(&toml).is_err(), "should reject unknown field");
    }

    #[test]
    fn test_soapy_gain_fields_accept_numeric_values() {
        let cfg = from_toml_str(&minimal_soapy_toml(
            r#"
rx_gain_lna = 32
tx_gain_vga = 12.5
"#,
        ))
        .expect("numeric gain fields should parse");

        let soapy = cfg.phy_io.soapysdr.expect("SoapySdr config expected");
        assert_eq!(soapy.rx_gains.get("lna"), Some(&32.0));
        assert_eq!(soapy.tx_gains.get("vga"), Some(&12.5));
    }

    #[test]
    fn test_soapy_gain_fields_reject_non_numeric_values_without_panic() {
        for (field, expected_error) in [
            ("rx_gain_lna = \"high\"", "phy_io.soapysdr.rx_gain_lna must be a number"),
            ("tx_gain_vga = true", "phy_io.soapysdr.tx_gain_vga must be a number"),
        ] {
            let toml = minimal_soapy_toml(field);
            let result = std::panic::catch_unwind(|| from_toml_str(&toml));
            assert!(result.is_ok(), "invalid gain field must return Err, not panic: {field}");
            let err = result.unwrap().expect_err("non-numeric gain field should be rejected");
            assert!(
                err.to_string().contains(expected_error),
                "expected error to contain {expected_error:?}, got {err}"
            );
        }
    }

    #[test]
    fn test_soapy_tx_calibration_defaults_to_disabled() {
        let cfg = from_toml_str(&minimal_soapy_toml("")).expect("parse failed");
        let soapy = cfg.phy_io.soapysdr.expect("SoapySdr config expected");
        assert!(!soapy.tx_calibration_enabled);
        assert_eq!(soapy.tx_calibration_file, "calibration.toml");
        assert!(soapy.tx_calibration_apply_dc);
        assert!(!soapy.tx_calibration_apply_iq);
    }

    #[test]
    fn test_soapy_tx_calibration_accepts_explicit_settings() {
        let cfg = from_toml_str(&minimal_soapy_toml(
            r#"
tx_calibration_enabled = true
tx_calibration_file = "calibration.toml"
tx_calibration_apply_dc = true
tx_calibration_apply_iq = false
"#,
        ))
        .expect("calibration fields should parse");
        let soapy = cfg.phy_io.soapysdr.expect("SoapySdr config expected");
        assert!(soapy.tx_calibration_enabled);
        assert_eq!(soapy.tx_calibration_file, "calibration.toml");
        assert!(soapy.tx_calibration_apply_dc);
        assert!(!soapy.tx_calibration_apply_iq);
    }

    #[test]
    fn test_soapy_tx_calibration_rejects_wrong_types() {
        for input in [
            "tx_calibration_enabled = \"yes\"",
            "tx_calibration_file = true",
            "tx_calibration_apply_dc = 1",
            "tx_calibration_apply_iq = \"false\"",
        ] {
            let toml = minimal_soapy_toml(input);
            assert!(from_toml_str(&toml).is_err(), "should reject {input}");
        }
    }

    #[test]
    fn test_energy_saving_mode_defaults_to_auto() {
        let toml = minimal_toml("");
        let cfg = from_toml_str(&toml).expect("parse failed");
        assert_eq!(cfg.cell.energy_saving_mode, ENERGY_SAVING_MODE_AUTO);
    }

    #[test]
    fn test_energy_saving_mode_accepts_explicit_eg_values() {
        for (input, expected) in [
            ("energy_saving_mode = 0", 0),
            ("energy_saving_mode = 7", 7),
            ("energy_saving_mode = false", 0),
            ("energy_saving_mode = \"stay_alive\"", 0),
            ("energy_saving_mode = \"auto\"", ENERGY_SAVING_MODE_AUTO),
            ("energy_saving_mode = \"eg1\"", 1),
            ("energy_saving_mode = \"eg7\"", 7),
            ("energy_economy_group = \"auto\"", ENERGY_SAVING_MODE_AUTO),
            ("energy_economy_group = \"eg3\"", 3),
        ] {
            let toml = minimal_toml(input);
            let cfg = from_toml_str(&toml).expect("parse failed");
            assert_eq!(cfg.cell.energy_saving_mode, expected, "input: {input}");
        }
    }

    #[test]
    fn test_energy_saving_mode_rejects_implicit_or_invalid_values() {
        for input in [
            "energy_saving_mode = true",
            "energy_saving_mode = 8",
            "energy_saving_mode = \"eg8\"",
            "energy_saving_mode = []",
        ] {
            let toml = minimal_toml(input);
            assert!(from_toml_str(&toml).is_err(), "should reject {input}");
        }
    }

    #[test]
    fn test_transmission_interruption_defaults_off() {
        let toml = minimal_toml("");
        let cfg = from_toml_str(&toml).expect("parse failed");
        assert!(!cfg.cell.transmission_interruption_enabled);
    }

    #[test]
    fn test_legacy_gssi_group_call_defaults_off() {
        let toml = minimal_toml("");
        let cfg = from_toml_str(&toml).expect("parse failed");
        assert!(!cfg.cell.legacy_gssi_group_call);
    }

    #[test]
    fn test_restart_recovery_issis_parse_deduped_inside_local_ranges() {
        let cfg = from_toml_str(&minimal_toml(
            r#"
local_ssi_ranges = [[2260000, 2269999]]
restart_recovery_issis = [2260616, 2260082, 2260616]
"#,
        ))
        .expect("restart recovery ISSIs inside local range should parse");

        assert_eq!(cfg.cell.restart_recovery_issis, vec![2260082, 2260616]);
    }

    #[test]
    fn test_restart_recovery_issis_reject_nonlocal_seed() {
        let err = from_toml_str(&minimal_toml(
            r#"
local_ssi_ranges = [[2260000, 2269999]]
restart_recovery_issis = [1234]
"#,
        ))
        .expect_err("restart recovery seed outside local ranges must be rejected");

        assert!(
            err.to_string().contains("outside cell_info.local_ssi_ranges"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_transmission_interruption_accepts_explicit_flag_and_alias() {
        for input in [
            "transmission_interruption_enabled = true",
            "group_call_preemption = true",
            "call_preemptive = true",
        ] {
            let toml = minimal_toml(input);
            let cfg = from_toml_str(&toml).expect("parse failed");
            assert!(cfg.cell.transmission_interruption_enabled, "input: {input}");
        }
    }

    #[test]
    fn test_transmission_interruption_accepts_explicit_false_and_aliases() {
        for input in [
            "transmission_interruption_enabled = false",
            "group_call_preemption = false",
            "call_preemptive = false",
        ] {
            let toml = minimal_toml(input);
            let cfg = from_toml_str(&toml).expect("parse failed");
            assert!(!cfg.cell.transmission_interruption_enabled, "input: {input}");
        }
    }

    #[test]
    fn test_private_p2p_hook_override_accepts_flag_and_aliases() {
        for input in [
            "force_private_p2p_hook_signalling = true",
            "private_p2p_force_hook = true",
            "private_call_force_hook = true",
        ] {
            let toml = minimal_toml(input);
            let cfg = from_toml_str(&toml).expect("parse failed");
            assert!(cfg.cell.force_private_p2p_hook_signalling, "input: {input}");
        }
    }

    #[test]
    fn test_legacy_gssi_group_call_accepts_explicit_flag_and_aliases() {
        for input in [
            "legacy_gssi_group_call = true",
            "legacy_group_call = true",
            "legacy_group_same_speaker_retake = true",
        ] {
            let toml = minimal_toml(input);
            let cfg = from_toml_str(&toml).expect("parse failed");
            assert!(cfg.cell.legacy_gssi_group_call, "input: {input}");
        }
    }

    #[test]
    fn test_transmission_interruption_rejects_conflicting_aliases() {
        for input in [
            r#"
transmission_interruption_enabled = false
call_preemptive = true
"#,
            r#"
group_call_preemption = true
call_preemptive = false
"#,
        ] {
            let toml = minimal_toml(input);
            assert!(
                from_toml_str(&toml).is_err(),
                "conflicting transmission interruption aliases must be rejected: {input}"
            );
        }
    }

    #[test]
    fn test_sndcp_service_is_rejected_until_packet_data_bearer_is_implemented() {
        let toml = minimal_toml("sndcp_service = true");

        // EN 300 392-2 clauses 17.2 and 18.5.21 map SNDCP packet data through
        // MLE/TLPD service details. This stack does not implement SNDCP/WAP
        // packet data yet, so the BS must not advertise that local service.
        let err = from_toml_str(&toml).expect_err("SNDCP service advertising must stay fail-closed");
        assert!(
            err.to_string().contains("SNDCP/WAP packet-data bearer is not implemented"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_neighbor_sndcp_service_is_rejected_until_packet_data_bearer_is_implemented() {
        let toml = minimal_toml(
            r#"
neighbor_cell_broadcast = 2

[[cell_info.neighbor_cells_ca]]
cell_identifier_ca = 1
cell_reselection_types_supported = 0
neighbor_cell_synchronized = false
cell_load_ca = 0
main_carrier_number = 1585

[cell_info.neighbor_cells_ca.bs_service_details]
voice_service = true
sndcp_service = true
"#,
        );

        // EN 300 392-2 clause 18.5.17 allows neighbour-cell BS service
        // details in D-NWRK-BROADCAST, while table 18.26 defines
        // SNDCP service=1 as available packet data on that cell. The stack's
        // WAP MVP is SDS-based, so packet-data service advertising stays off.
        let err = from_toml_str(&toml).expect_err("neighbour SNDCP service advertising must stay fail-closed");
        assert!(
            err.to_string()
                .contains("neighbor_cells_ca[0].bs_service_details.sndcp_service=true is not supported"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_allowed_gssi_ranges_default_open_and_explicit_restrictive_ranges() {
        let cfg = from_toml_str(&minimal_toml("")).expect("parse failed");
        assert!(cfg.cell.allowed_gssi_ranges.is_none());

        let cfg = from_toml_str(&minimal_toml(
            r#"
allowed_gssi_ranges = [
    [3000, 3001],
    [4000, 4000],
]
"#,
        ))
        .expect("parse failed");
        let ranges = cfg.cell.allowed_gssi_ranges.expect("expected configured GSSI ranges");
        assert!(ranges.contains(3000));
        assert!(ranges.contains(3001));
        assert!(!ranges.contains(3002));
        assert!(ranges.contains(4000));
    }

    #[test]
    fn test_allowed_gssi_ranges_accepts_24_bit_boundary() {
        let cfg = from_toml_str(&minimal_toml(
            r#"
allowed_gssi_ranges = [
    [16777215, 16777215],
]
"#,
        ))
        .expect("parse failed");

        let ranges = cfg.cell.allowed_gssi_ranges.expect("expected configured GSSI ranges");
        assert!(ranges.contains(0x00FF_FFFF));
    }

    #[test]
    fn test_allowed_gssi_ranges_rejects_invalid_ranges_without_panic() {
        for input in [
            r#"
allowed_gssi_ranges = [
    [16777216, 16777216],
]
"#,
            r#"
allowed_gssi_ranges = [
    [4000, 3000],
]
"#,
            r#"
allowed_gssi_ranges = [
    [3000, 4000],
    [3500, 4500],
]
"#,
        ] {
            let toml = minimal_toml(input);
            let result = std::panic::catch_unwind(|| from_toml_str(&toml));
            assert!(result.is_ok(), "invalid allowed_gssi_ranges must return Err, not panic: {input}");
            assert!(result.unwrap().is_err(), "should reject invalid allowed_gssi_ranges: {input}");
        }
    }

    #[test]
    fn test_example_config_keeps_transmission_interruption_disabled() {
        let toml = include_str!("../../../../example_config/config.toml");
        let cfg = from_toml_str(toml).expect("example config should parse");

        // EN 300 392-2 clause 14.5.2.2.1 f) makes pre-emptive transmission
        // interruption conditional on SwMI support; the shipped example keeps
        // that support disabled unless the operator explicitly enables it.
        assert!(toml.contains("call_preemptive = false"));
        assert!(toml.contains("force_private_p2p_hook_signalling = false"));
        assert!(toml.contains("legacy_gssi_group_call = true"));
        assert!(toml.contains("energy_saving_mode = \"auto\""));
        assert!(toml.contains("# static_dir = "));
        assert!(!toml.contains("IqMaster"));
        assert!(!toml.contains("[identity]"));
        assert!(toml.contains("backend = \"SoapySdr\""));
        assert!(toml.contains("BlueStation reference RF profile"));
        let soapy = cfg.phy_io.soapysdr.as_ref().expect("example should configure SoapySDR");
        assert_eq!(soapy.dl_freq, 438_025_000.0);
        assert_eq!(soapy.ul_freq, 433_025_000.0);
        assert_eq!(soapy.fs, Some(600_000.0));
        assert_eq!(cfg.net.mcc, 901);
        assert_eq!(cfg.net.mnc, 9999);
        assert_eq!(cfg.cell.freq_band, 4);
        assert_eq!(cfg.cell.main_carrier, 1521);
        assert_eq!(cfg.cell.duplex_spacing_id, 4);
        assert_eq!(cfg.cell.freq_offset_hz, 0);
        assert!(!cfg.cell.reverse_operation);
        assert_eq!(cfg.cell.location_area, 3);
        assert_eq!(cfg.cell.system_code, 3);
        assert_eq!(cfg.cell.neighbor_cell_broadcast, 1);
        assert!(cfg.cell.local_ssi_ranges.contains(42));
        assert!(!cfg.cell.local_ssi_ranges.contains(226333));
        assert!(!cfg.cell.transmission_interruption_enabled);
        assert!(!cfg.cell.force_private_p2p_hook_signalling);
        assert!(cfg.cell.legacy_gssi_group_call);
        assert_eq!(cfg.cell.energy_saving_mode, ENERGY_SAVING_MODE_AUTO);
        assert_eq!(cfg.cell.periodic_registration_secs, 0);
        assert!(!cfg.cell.sndcp_service);
        assert!(cfg.dashboard.is_some());
        let control = cfg.control.as_ref().expect("example should configure local control endpoint");
        assert_eq!(control.host, "127.0.0.1");
        assert_eq!(control.port, 9002);
        assert!(!control.use_tls);
        assert_eq!(cfg.service_name.as_deref(), Some("nexus-bs"));
        assert!(cfg.health.enabled);
        assert_eq!(cfg.health.snapshot_interval_secs, 1);
        assert!(!cfg.health.restart_on_core_stall);
    }

    #[test]
    fn test_health_defaults_to_observe_only_when_section_is_absent() {
        let toml = minimal_toml("");
        let cfg = from_toml_str(&toml).expect("parse failed");

        assert!(cfg.health.enabled);
        assert_eq!(cfg.health.snapshot_interval_secs, 1);
        assert_eq!(cfg.health.core_stall_critical_ms, 10_000);
        assert!(!cfg.health.restart_on_core_stall);
    }

    #[test]
    fn test_health_section_parses_explicit_values() {
        let toml = format!(
            "{}\n{}",
            minimal_toml(""),
            r#"
[health]
enabled = false
snapshot_interval_secs = 2
core_stall_critical_ms = 15000
restart_on_core_stall = false
restart_after_critical_secs = 45
restart_cooldown_secs = 900
"#
        );
        let cfg = from_toml_str(&toml).expect("parse failed");

        assert!(!cfg.health.enabled);
        assert_eq!(cfg.health.snapshot_interval_secs, 2);
        assert_eq!(cfg.health.core_stall_critical_ms, 15_000);
        assert_eq!(cfg.health.restart_after_critical_secs, 45);
        assert_eq!(cfg.health.restart_cooldown_secs, 900);
    }

    #[test]
    fn test_health_section_rejects_unknown_fields() {
        let toml = format!(
            "{}\n{}",
            minimal_toml(""),
            r#"
[health]
enabled = true
surprise = 1
"#
        );
        let err = from_toml_str(&toml).expect_err("unknown health field should fail");

        assert!(err.to_string().contains("health"));
        assert!(err.to_string().contains("surprise"));
    }

    #[test]
    fn test_home_mode_display_default_source_issi_is_all_ones() {
        let toml = minimal_toml(
            r#"
    [cell_info.home_mode_display]
    text = "Nexus-BS"
    "#,
        );
        let cfg = from_toml_str(&toml).expect("parse failed");
        assert_eq!(
            cfg.cell.home_mode_display.as_ref().map(|h| h.source_issi),
            Some(crate::bluestation::DEFAULT_BROADCAST_SOURCE_SSI)
        );
        assert_eq!(
            cfg.cell.home_mode_display.as_ref().map(|h| h.protocol_id),
            Some(crate::bluestation::DEFAULT_SDS_TEXT_PROTOCOL_ID)
        );
    }

    #[test]
    fn test_sds_broadcast_default_source_issi_is_all_ones() {
        let toml = minimal_toml(
            r#"
    [cell_info.sds_broadcast]
    text = "Nexus-BS"
    "#,
        );
        let cfg = from_toml_str(&toml).expect("parse failed");
        assert_eq!(
            cfg.cell.sds_broadcast.as_ref().map(|h| h.source_issi),
            Some(crate::bluestation::DEFAULT_BROADCAST_SOURCE_SSI)
        );
        assert_eq!(
            cfg.cell.sds_broadcast.as_ref().map(|h| h.protocol_id),
            Some(crate::bluestation::DEFAULT_SDS_TEXT_PROTOCOL_ID)
        );
    }

    #[test]
    fn test_periodic_sds_explicit_source_issi_is_preserved() {
        let toml = minimal_toml(
            r#"
    [cell_info.home_mode_display]
    source_issi = 1234
    text = "Nexus-BS"

    [cell_info.sds_broadcast]
    source_issi = 5678
    text = "Status"
    "#,
        );
        let cfg = from_toml_str(&toml).expect("parse failed");
        assert_eq!(cfg.cell.home_mode_display.as_ref().map(|h| h.source_issi), Some(1234));
        assert_eq!(cfg.cell.sds_broadcast.as_ref().map(|h| h.source_issi), Some(5678));
    }

    #[test]
    fn test_periodic_sds_explicit_vendor_protocol_id_is_preserved() {
        let toml = minimal_toml(
            r#"
    [cell_info.home_mode_display]
    protocol_id = 220
    text = "Nexus-BS"
    "#,
        );
        let cfg = from_toml_str(&toml).expect("parse failed");
        assert_eq!(cfg.cell.home_mode_display.as_ref().map(|h| h.protocol_id), Some(220));
    }

    #[test]
    fn test_periodic_sds_rejects_non_sds_tl_transport_protocol_id() {
        for (section, protocol_id) in [("home_mode_display", 0x02), ("sds_broadcast", 0xFF)] {
            let toml = minimal_toml(&format!(
                r#"
    [cell_info.{section}]
    protocol_id = {protocol_id}
    text = "Nexus-BS"
    "#
            ));
            let err = from_toml_str(&toml).expect_err("non-SDS-TL transport PID must be rejected");
            let err = err.to_string();
            assert!(
                err.contains("protocol_id must be 0x82 text messaging or user-defined SDS-TL PID 0xC0..0xFE"),
                "unexpected error for {section} PID {protocol_id}: {err}"
            );
        }
    }

    #[test]
    fn test_periodic_sds_rejects_wap_protocol_id_0x84() {
        for section in ["home_mode_display", "sds_broadcast"] {
            let toml = minimal_toml(&format!(
                r#"
    [cell_info.{section}]
    protocol_id = 0x84
    text = "Nexus-BS"
    "#
            ));
            let err = from_toml_str(&toml).expect_err("WAP PID must be rejected by text-style SDS config");
            let err = err.to_string();
            assert!(
                err.contains(
                    "standard non-text SDS-TL application PIDs, including WAP/WCMP, require their own payload encoders and are rejected here"
                ),
                "unexpected error for {section}: {err}"
            );
        }
    }

    #[test]
    fn test_sds_command_control_rejects_reserved_and_sds_tl_status_codes() {
        for status_code in [1, 32001, 32767] {
            let toml = minimal_toml(&format!(
                r#"
[cell_info.sds_command_control]
authorized_issis = [1000001]

[[cell_info.sds_command_control.commands]]
status_code = {status_code}
action = "restart"
"#
            ));

            // EN 300 392-2 clause 14.8.34 table 14.72 leaves only
            // 32768..=65535 for TETRA network/user-specific meanings. Local
            // command-control status codes must not occupy reserved or SDS-TL
            // short-report space.
            let err = from_toml_str(&toml).expect_err("non-network/user-specific command status must be rejected");
            assert!(
                err.to_string().contains("network/user-specific range 32768..=65535"),
                "unexpected error for status {status_code}: {err}"
            );
        }
    }

    #[test]
    fn test_sds_command_control_accepts_network_user_specific_status_code() {
        let toml = minimal_toml(
            r#"
[cell_info.sds_command_control]
authorized_issis = [1000001]

[[cell_info.sds_command_control.commands]]
status_code = 36865
action = "restart"
"#,
        );

        let cfg = from_toml_str(&toml).expect("network/user-specific command status should parse");
        let ctrl = cfg.cell.sds_command_control.expect("expected command control config");
        assert_eq!(ctrl.commands[0].status_code, 0x9001);
    }
}
