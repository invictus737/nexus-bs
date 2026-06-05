use crate::net_control::ControlEndpoint;
use crate::net_telemetry::channel::TelemetrySink;
use crate::{MessageQueue, TetraEntityTrait, net_brew};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use tetra_config::bluestation::{EnergySavingAssignment, SharedConfig};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::typed_pdu_fields::{Type3FieldGeneric, delimiters, typed};
use tetra_core::{BitBuffer, Layer2Service, MleHandle, Sap, SsiType, TdmaTime, TetraAddress, unimplemented_log};
use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
use tetra_saps::lmm::LmmMleUnitdataReq;
use tetra_saps::tla::{TLA_REPORT_FAILED_TRANSFER, TLA_REPORT_SUCCESSFUL_TRANSFER};
use tetra_saps::{SapMsg, SapMsgInner};

use crate::mm::components::client_state::{ClientMgrErr, GroupAttachmentInfo, MmClientMgr, MmClientState};
use crate::mm::components::not_supported::make_ul_mm_pdu_function_not_supported;
use tetra_pdus::mm::enums::energy_saving_mode::EnergySavingMode;
use tetra_pdus::mm::enums::location_update_accept_type::LocationUpdateAcceptType;
use tetra_pdus::mm::enums::location_update_type::LocationUpdateType;
use tetra_pdus::mm::enums::mm_pdu_type_ul::MmPduTypeUl;
use tetra_pdus::mm::enums::reject_cause::RejectCause;
use tetra_pdus::mm::enums::status_downlink::StatusDownlink;
use tetra_pdus::mm::enums::status_uplink::StatusUplink;
use tetra_pdus::mm::enums::type34_elem_id_dl::MmType34ElemIdDl;
use tetra_pdus::mm::enums::type34_elem_id_ul::MmType34ElemIdUl;
use tetra_pdus::mm::fields::energy_saving_information::EnergySavingInformation;
use tetra_pdus::mm::fields::group_identity_attachment::GroupIdentityAttachment;
use tetra_pdus::mm::fields::group_identity_downlink::GroupIdentityDownlink;
use tetra_pdus::mm::fields::group_identity_location_accept::GroupIdentityLocationAccept;
use tetra_pdus::mm::fields::group_identity_uplink::GroupIdentityUplink;
use tetra_pdus::mm::pdus::d_attach_detach_group_identity::DAttachDetachGroupIdentity;
use tetra_pdus::mm::pdus::d_attach_detach_group_identity_acknowledgement::DAttachDetachGroupIdentityAcknowledgement;
use tetra_pdus::mm::pdus::d_location_update_accept::DLocationUpdateAccept;
use tetra_pdus::mm::pdus::d_location_update_command::DLocationUpdateCommand;
use tetra_pdus::mm::pdus::d_location_update_reject::DLocationUpdateReject;
use tetra_pdus::mm::pdus::d_mm_status::DMmStatus;
use tetra_pdus::mm::pdus::u_attach_detach_group_identity::UAttachDetachGroupIdentity;
use tetra_pdus::mm::pdus::u_attach_detach_group_identity_acknowledgement::UAttachDetachGroupIdentityAcknowledgement;
use tetra_pdus::mm::pdus::u_itsi_detach::UItsiDetach;
use tetra_pdus::mm::pdus::u_location_update_demand::ULocationUpdateDemand;
use tetra_pdus::mm::pdus::u_mm_status::UMmStatus;
use tetra_pdus::mm::pdus::u_tei_provide::UTeiProvide;

pub struct MmBs {
    config: SharedConfig,
    telemetry: Option<TelemetrySink>,
    control: Option<ControlEndpoint>,
    client_mgr: MmClientMgr,
    dltime: TdmaTime,
    pending_energy_saving: HashMap<u32, PendingEnergySavingAssignment>,
    pending_swmi_group_transactions: HashMap<u32, PendingSwmiGroupTransaction>,
    pending_solicited_group_reports: HashMap<u32, TdmaTime>,
    restart_recovery: HashMap<u32, RestartRecoveryProbe>,
    pending_critical_downlinks: HashMap<MleHandle, PendingCriticalMmDownlink>,
    next_critical_downlink_handle: MleHandle,
}

#[derive(Debug, Clone, Copy)]
struct RestartRecoveryProbe {
    attempts: u8,
    next_due: TdmaTime,
}

struct PendingEnergySavingAssignment {
    esi: EnergySavingInformation,
    start_time: Option<TdmaTime>,
    expires_at: TdmaTime,
    previous_active: Option<EnergySavingAssignment>,
}

struct PendingSwmiGroupTransaction {
    handle: u32,
    expires_at: TdmaTime,
    group_identity_downlink: Vec<GroupIdentityDownlink>,
    detach_all_then_attach: bool,
    accepts_unrouted_ack_handle: bool,
    rollback_unconfirmed_attachments_on_failure: bool,
    reprobe_group_report_on_failure: bool,
    remaining_restart_group_refresh: Vec<(u32, GroupAttachmentInfo)>,
}

struct CachedRestartGroupRefresh {
    groups: Vec<(u32, GroupAttachmentInfo)>,
    remaining: Vec<(u32, GroupAttachmentInfo)>,
}

type RestartRecoveryCache = BTreeMap<u32, BTreeMap<u32, GroupAttachmentInfo>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CriticalMmDownlinkKind {
    LocationUpdateAccept,
}

#[derive(Debug, Clone, Copy)]
struct PendingCriticalMmDownlink {
    issi: u32,
    retry_handle: u32,
    kind: CriticalMmDownlinkKind,
}

struct GroupIdentityProcessResult {
    group_identity_downlink: Vec<GroupIdentityDownlink>,
    all_accepted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GroupIdentityAddress {
    gssi: Option<u32>,
    address_extension: Option<u32>,
    vgssi: Option<u32>,
}

impl GroupIdentityAddress {
    fn from_downlink(gid: &GroupIdentityDownlink) -> Self {
        Self {
            gssi: gid.gssi,
            address_extension: gid.address_extension,
            vgssi: gid.vgssi,
        }
    }

    fn from_uplink(gid: &GroupIdentityUplink) -> Self {
        Self {
            gssi: gid.gssi,
            address_extension: gid.address_extension,
            vgssi: gid.vgssi,
        }
    }

    fn plain_gssi(self) -> Option<u32> {
        match (self.gssi, self.address_extension, self.vgssi) {
            (Some(gssi), None, None) => Some(gssi),
            _ => None,
        }
    }
}

impl MmBs {
    const MAX_AIR_INTERFACE_SSI: u32 = 0x00FF_FFFF;
    const MAX_GROUPS_PER_ATTACH: usize = 12;
    // EN 300 392-2 clause 23.7.6 / timer T.210: after signalling activity,
    // the MS remains awake for 18 TDMA frames before returning to energy economy.
    const T210_AWAKE_FRAMES: i32 = 18;
    const ENERGY_ECONOMY_UNSCHEDULED_SCH_F_FRAME: u8 = 18;
    // EN 300 392-2 clauses 16.7.3 and 16.11.1.2: T352 energy mode response
    // time is 30 s. One TETRA multiframe is 18 TDMA frames (72 slots), so this
    // deterministic TDMA approximation is just over 30 s.
    const T352_ENERGY_RESPONSE_TIMESLOTS: i32 = 30 * 18 * 4;
    // EN 300 392-2 clause 16.11.1.3: T353 is 10 s for attach/detach group
    // identity response. One TETRA second is 18 frames * 4 timeslots.
    const T353_GROUP_RESPONSE_TIMESLOTS: i32 = 10 * 18 * 4;
    // Local restart recovery retry cadence. This is an operator-side SwMI
    // policy around the ETSI clause 16.4.4 infrastructure-initiated
    // registration procedure, not an ETSI timer value.
    //
    // The initial guard and inter-ISSI spacing are local RF robustness policy:
    // after process restart the SDR/PHY path may still be settling, and
    // blasting several acknowledged MM commands in the same first TDMA tick can
    // lose the very D-LOCATION UPDATE COMMANDs that are meant to clear
    // "Unit Not Attached" states.
    const RESTART_RECOVERY_INITIAL_DELAY_TIMESLOTS: i32 = 18 * 4;
    const RESTART_RECOVERY_COMMAND_SPACING_TIMESLOTS: i32 = 18 * 4;
    const RESTART_RECOVERY_RETRY_TIMESLOTS: i32 = 2 * 18 * 4;
    const RESTART_RECOVERY_MAX_ATTEMPTS: u8 = 150;
    // Local acceptance window for the group-report phase requested by
    // D-LOCATION UPDATE COMMAND(group identity report=1), per EN 300 392-2
    // clause 16.4.4. This is not an ETSI timer value; it matches the local
    // registration-command grace used before declaring the MS stale.
    const SOLICITED_GROUP_REPORT_WINDOW_TIMESLOTS: i32 = 60 * 18 * 4;

    pub fn new(config: SharedConfig, telemetry: Option<TelemetrySink>, control: Option<ControlEndpoint>) -> Self {
        let client_mgr = MmClientMgr::new(telemetry.clone());
        let restart_recovery_start = TdmaTime::default().add_timeslots(Self::RESTART_RECOVERY_INITIAL_DELAY_TIMESLOTS);
        let restart_recovery = Self::load_restart_recovery_candidates(&config)
            .into_iter()
            .enumerate()
            .map(|(index, issi)| {
                (
                    issi,
                    RestartRecoveryProbe {
                        attempts: 0,
                        next_due: restart_recovery_start.add_timeslots(index as i32 * Self::RESTART_RECOVERY_COMMAND_SPACING_TIMESLOTS),
                    },
                )
            })
            .collect();
        Self {
            config,
            telemetry,
            control,
            client_mgr,
            dltime: TdmaTime::default(),
            pending_energy_saving: HashMap::new(),
            pending_swmi_group_transactions: HashMap::new(),
            pending_solicited_group_reports: HashMap::new(),
            restart_recovery,
            pending_critical_downlinks: HashMap::new(),
            next_critical_downlink_handle: 0x8000_0000,
        }
    }

    fn expected_ms_mni(&self) -> u64 {
        let net = &self.config.config().net;
        ((net.mcc as u64) << 14) | net.mnc as u64
    }

    fn subscriber_recovery_path(config: &SharedConfig) -> Option<String> {
        config.state_read().subscriber_recovery_path.clone()
    }

    fn restart_recovery_eligible(config: &SharedConfig, issi: u32) -> bool {
        if issi > Self::MAX_AIR_INTERFACE_SSI {
            return false;
        }

        let cfg = config.config();
        let local_ranges = &cfg.cell.local_ssi_ranges;
        if !local_ranges.as_slice().is_empty() && !local_ranges.contains(issi) {
            return false;
        }

        cfg.security.is_issi_allowed(issi)
    }

    fn parse_restart_recovery_group_spec(spec: &str) -> Result<(u32, GroupAttachmentInfo), String> {
        let clean = spec.trim().trim_matches(',').trim_matches('[').trim_matches(']');
        if clean.is_empty() {
            return Err("empty group token".to_string());
        }

        let mut fields = clean.split(':');
        let gssi = fields
            .next()
            .ok_or_else(|| "missing GSSI".to_string())?
            .parse::<u32>()
            .map_err(|err| format!("invalid GSSI: {err}"))?;
        let group_identity_attachment_lifetime = match fields.next() {
            Some(value) if !value.is_empty() => value
                .parse::<u8>()
                .map_err(|err| format!("invalid group attachment lifetime: {err}"))?,
            _ => 0,
        };
        let class_of_usage = match fields.next() {
            Some(value) if !value.is_empty() => value.parse::<u8>().map_err(|err| format!("invalid group class of usage: {err}"))?,
            _ => 0,
        };
        if fields.next().is_some() {
            return Err("too many ':' fields in group token".to_string());
        }

        Ok((
            gssi,
            GroupAttachmentInfo {
                group_identity_attachment_lifetime,
                class_of_usage,
            },
        ))
    }

    fn read_restart_recovery_cache(path: &str) -> RestartRecoveryCache {
        let mut cache = RestartRecoveryCache::new();
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return cache,
            Err(err) => {
                tracing::warn!("MM: failed reading restart recovery cache '{}': {}", path, err);
                return cache;
            }
        };

        for (line_no, line) in contents.lines().enumerate() {
            let body = line.split('#').next().unwrap_or("").trim();
            if body.is_empty() {
                continue;
            }
            let mut tokens = body.split_whitespace();
            let Some(issi_token) = tokens.next() else {
                continue;
            };
            match issi_token.trim_end_matches(',').parse::<u32>() {
                Ok(issi) => {
                    let groups = cache.entry(issi).or_default();
                    for token in tokens {
                        let token = token.trim();
                        let token = token.strip_prefix("groups=").unwrap_or(token);
                        for spec in token.split(',') {
                            if spec.trim().is_empty() {
                                continue;
                            }
                            match Self::parse_restart_recovery_group_spec(spec) {
                                Ok((gssi, info)) if gssi <= Self::MAX_AIR_INTERFACE_SSI => {
                                    groups.insert(gssi, info);
                                }
                                Ok((gssi, _)) => {
                                    tracing::warn!(
                                        "MM: ignored invalid cached GSSI {} in restart recovery cache '{}' line {}",
                                        gssi,
                                        path,
                                        line_no + 1
                                    );
                                }
                                Err(err) => {
                                    tracing::warn!(
                                        "MM: ignored invalid cached group token '{}' in restart recovery cache '{}' line {}: {}",
                                        spec,
                                        path,
                                        line_no + 1,
                                        err
                                    );
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        "MM: ignored invalid ISSI '{}' in restart recovery cache '{}' line {}: {}",
                        issi_token,
                        path,
                        line_no + 1,
                        err
                    );
                }
            }
        }

        cache
    }

    fn write_restart_recovery_cache(path: &str, cache: &RestartRecoveryCache) -> std::io::Result<()> {
        if let Some(parent) = Path::new(path).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }

        let mut body = String::from(
            "# Nexus-BS local subscriber restart recovery cache\n\
             # Auto-managed by MM. Format: ISSI [GSSI:lifetime:class_of_usage ...]\n",
        );
        for (issi, groups) in cache {
            body.push_str(&issi.to_string());
            for (gssi, info) in groups {
                body.push_str(&format!(
                    " {gssi}:{}:{}",
                    info.group_identity_attachment_lifetime, info.class_of_usage
                ));
            }
            body.push('\n');
        }

        let tmp = format!("{path}.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, path)
    }

    fn load_restart_recovery_candidates(config: &SharedConfig) -> BTreeSet<u32> {
        if !config.config().cell.registration {
            return BTreeSet::new();
        }

        let mut issis = BTreeSet::new();
        for issi in &config.config().cell.restart_recovery_issis {
            if Self::restart_recovery_eligible(config, *issi) {
                issis.insert(*issi);
            }
        }

        if let Some(path) = Self::subscriber_recovery_path(config) {
            for issi in Self::read_restart_recovery_cache(&path).keys().copied() {
                if Self::restart_recovery_eligible(config, issi) {
                    issis.insert(issi);
                } else {
                    tracing::warn!(
                        "MM: restart recovery cache ISSI {} ignored because it is outside local policy/whitelist",
                        issi
                    );
                }
            }
        }

        if !issis.is_empty() {
            tracing::info!("MM: restart recovery armed for {} local ISSI(s): {:?}", issis.len(), issis);
        }

        issis
    }

    fn cached_restart_recovery_groups_for_issi(&self, issi: u32) -> Vec<(u32, GroupAttachmentInfo)> {
        let Some(path) = Self::subscriber_recovery_path(&self.config) else {
            return Vec::new();
        };
        let cache = Self::read_restart_recovery_cache(&path);
        let Some(groups) = cache.get(&issi) else {
            return Vec::new();
        };
        groups
            .iter()
            .filter_map(|(&gssi, &info)| {
                if self.restart_recovery_group_allowed(gssi) {
                    Some((gssi, info))
                } else {
                    tracing::warn!(
                        "MM: cached GSSI {} for ISSI {} ignored because it is outside local group policy",
                        gssi,
                        issi
                    );
                    None
                }
            })
            .collect()
    }

    fn restart_recovery_group_allowed(&self, gssi: u32) -> bool {
        if gssi > Self::MAX_AIR_INTERFACE_SSI {
            return false;
        }
        !self
            .config
            .config()
            .cell
            .allowed_gssi_ranges
            .as_ref()
            .is_some_and(|ranges| !ranges.contains(gssi))
    }

    fn current_restart_recovery_groups_for_client_with_remaining(
        &mut self,
        issi: u32,
        remaining_restart_group_refresh: &[(u32, GroupAttachmentInfo)],
    ) -> BTreeMap<u32, GroupAttachmentInfo> {
        let groups: Vec<u32> = self
            .client_mgr
            .get_client_by_issi(issi)
            .map(|client| client.groups.iter().copied().collect())
            .unwrap_or_default();
        let mut cached_groups = BTreeMap::new();
        for gssi in groups {
            if self.restart_recovery_group_allowed(gssi) {
                let info = self.client_mgr.client_group_attachment_info(issi, gssi).unwrap_or_default();
                cached_groups.insert(gssi, info);
            }
        }
        if let Some(pending) = self.pending_swmi_group_transactions.get(&issi) {
            for &(gssi, info) in &pending.remaining_restart_group_refresh {
                if self.restart_recovery_group_allowed(gssi) {
                    cached_groups.insert(gssi, info);
                }
            }
        }
        for &(gssi, info) in remaining_restart_group_refresh {
            if self.restart_recovery_group_allowed(gssi) {
                cached_groups.insert(gssi, info);
            }
        }
        cached_groups
    }

    fn current_restart_recovery_groups_for_client(&mut self, issi: u32) -> BTreeMap<u32, GroupAttachmentInfo> {
        self.current_restart_recovery_groups_for_client_with_remaining(issi, &[])
    }

    fn remember_restart_recovery_issi(&mut self, issi: u32) {
        self.remember_restart_recovery_issi_with_remaining(issi, &[]);
    }

    fn remember_restart_recovery_issi_with_remaining(&mut self, issi: u32, remaining_restart_group_refresh: &[(u32, GroupAttachmentInfo)]) {
        self.restart_recovery.remove(&issi);
        if !Self::restart_recovery_eligible(&self.config, issi) {
            return;
        }
        let Some(path) = Self::subscriber_recovery_path(&self.config) else {
            return;
        };

        let groups = self.current_restart_recovery_groups_for_client_with_remaining(issi, remaining_restart_group_refresh);
        let mut cache = Self::read_restart_recovery_cache(&path);
        let changed = cache.get(&issi) != Some(&groups);
        cache.insert(issi, groups);
        if changed {
            if let Err(err) = Self::write_restart_recovery_cache(&path, &cache) {
                tracing::warn!("MM: failed persisting ISSI {} to restart recovery cache '{}': {}", issi, path, err);
            }
        }
    }

    fn forget_restart_recovery_issi(&mut self, issi: u32) {
        self.restart_recovery.remove(&issi);
        let Some(path) = Self::subscriber_recovery_path(&self.config) else {
            return;
        };

        let mut cache = Self::read_restart_recovery_cache(&path);
        if cache.remove(&issi).is_some() {
            if let Err(err) = Self::write_restart_recovery_cache(&path, &cache) {
                tracing::warn!("MM: failed removing ISSI {} from restart recovery cache '{}': {}", issi, path, err);
            }
        }
    }

    fn allocate_mm_downlink_handle(&mut self) -> MleHandle {
        loop {
            let handle = self.next_critical_downlink_handle;
            self.next_critical_downlink_handle = self.next_critical_downlink_handle.wrapping_add(1);
            if self.next_critical_downlink_handle < 0x8000_0000 {
                self.next_critical_downlink_handle = 0x8000_0000;
            }
            let swmi_group_handle_in_use = self
                .pending_swmi_group_transactions
                .values()
                .any(|pending| pending.handle == handle);
            if handle != 0 && !self.pending_critical_downlinks.contains_key(&handle) && !swmi_group_handle_in_use {
                return handle;
            }
        }
    }

    fn track_location_update_accept_downlink(&mut self, issi: u32, retry_handle: u32) -> MleHandle {
        self.pending_critical_downlinks
            .retain(|_, pending| !(pending.issi == issi && pending.kind == CriticalMmDownlinkKind::LocationUpdateAccept));
        let handle = self.allocate_mm_downlink_handle();
        self.pending_critical_downlinks.insert(
            handle,
            PendingCriticalMmDownlink {
                issi,
                retry_handle,
                kind: CriticalMmDownlinkKind::LocationUpdateAccept,
            },
        );
        handle
    }

    fn clear_critical_downlinks_for_issi(&mut self, issi: u32) {
        self.pending_critical_downlinks.retain(|_, pending| pending.issi != issi);
    }

    fn mark_registration_unconfirmed_and_reprobe(&mut self, queue: &mut MessageQueue, issi: u32, retry_handle: u32, reason: &str) {
        tracing::warn!(
            "MM: registration for ISSI {} is not confirmed after {}; sending D-LOCATION-UPDATE-COMMAND",
            issi,
            reason
        );
        self.set_client_stay_alive(issi);
        self.send_d_location_update_command(queue, issi, retry_handle);
        self.abandon_pending_swmi_group_transaction(issi, reason);
        self.client_mgr.set_pending_command(issi, 60);

        let groups: Vec<u32> = self
            .client_mgr
            .get_client_by_issi(issi)
            .map(|c| c.groups.iter().copied().collect())
            .unwrap_or_default();
        tracing::info!(
            "MM: preserving provisional subscriber state for ISSI {} groups={:?} while registration reprobe is pending",
            issi,
            groups
        );
    }

    fn rx_lmm_mle_report_ind(&mut self, queue: &mut MessageQueue, handle: MleHandle, transfer_result: i32) {
        let Some(pending) = self.pending_critical_downlinks.remove(&handle) else {
            tracing::trace!(
                "MM: MLE-REPORT indication handle={} transfer_result={} for untracked MM downlink",
                handle,
                transfer_result
            );
            return;
        };

        match transfer_result {
            TLA_REPORT_SUCCESSFUL_TRANSFER => {
                tracing::trace!(
                    "MM: critical {:?} downlink to ISSI {} confirmed by MLE handle={}",
                    pending.kind,
                    pending.issi,
                    handle
                );
            }
            TLA_REPORT_FAILED_TRANSFER => match pending.kind {
                CriticalMmDownlinkKind::LocationUpdateAccept => {
                    // EN 300 392-2 clause 16.4.4 allows the SwMI to initiate a
                    // fresh registration at any time with D-LOCATION UPDATE
                    // COMMAND. If the acknowledged D-LOCATION UPDATE ACCEPT is
                    // not delivered, the BS-side registration state is only
                    // provisional; reprobe instead of leaving the MS/BS views
                    // split.
                    self.mark_registration_unconfirmed_and_reprobe(
                        queue,
                        pending.issi,
                        pending.retry_handle,
                        "D-LOCATION UPDATE ACCEPT failed transfer",
                    );
                }
            },
            other => {
                tracing::debug!(
                    "MM: ignoring non-terminal MLE-REPORT {} for critical {:?} downlink to ISSI {} handle={}",
                    other,
                    pending.kind,
                    pending.issi,
                    handle
                );
                self.pending_critical_downlinks.insert(handle, pending);
            }
        }
    }

    fn tick_restart_recovery(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        if self.restart_recovery.is_empty() || !self.config.config().cell.registration {
            return;
        }

        let mut done = Vec::new();
        let mut commands = Vec::new();
        let mut command_scheduled_this_tick = false;
        for (&issi, probe) in self.restart_recovery.iter_mut() {
            if self.client_mgr.client_is_known(issi) {
                done.push(issi);
                continue;
            }
            if !Self::restart_recovery_eligible(&self.config, issi) {
                done.push(issi);
                continue;
            }
            if probe.attempts >= Self::RESTART_RECOVERY_MAX_ATTEMPTS {
                tracing::warn!(
                    "MM: restart recovery for ISSI {} stopped after {} D-LOCATION-UPDATE-COMMAND attempt(s)",
                    issi,
                    probe.attempts
                );
                done.push(issi);
                continue;
            }
            if ts.diff(probe.next_due) < 0 {
                continue;
            }
            if command_scheduled_this_tick {
                probe.next_due = ts.add_timeslots(Self::RESTART_RECOVERY_COMMAND_SPACING_TIMESLOTS);
                continue;
            }

            tracing::info!(
                "MM: restart recovery attempt {}/{} for ISSI {} — sending D-LOCATION-UPDATE-COMMAND",
                probe.attempts + 1,
                Self::RESTART_RECOVERY_MAX_ATTEMPTS,
                issi
            );
            commands.push(issi);
            command_scheduled_this_tick = true;
            probe.attempts = probe.attempts.saturating_add(1);
            probe.next_due = ts.add_timeslots(Self::RESTART_RECOVERY_RETRY_TIMESLOTS);
        }

        for issi in done {
            self.restart_recovery.remove(&issi);
        }
        for issi in commands {
            self.send_d_location_update_command(queue, issi, 0);
        }
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_backdate_registration_for_test(&mut self, issi: u32, elapsed: std::time::Duration) -> bool {
        self.client_mgr.debug_backdate_registration_for_test(issi, elapsed)
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_expire_registration_grace_for_test(&mut self, issi: u32) -> bool {
        self.client_mgr.debug_expire_registration_grace_for_test(issi)
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_client_energy_for_test(&mut self, issi: u32) -> Option<(EnergySavingMode, Option<u8>, Option<u8>)> {
        self.client_mgr
            .get_client_by_issi(issi)
            .map(|client| (client.energy_saving_mode, client.monitoring_frame, client.monitoring_multiframe))
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_client_tei_for_test(&mut self, issi: u32) -> Option<Option<u64>> {
        self.client_mgr.get_client_by_issi(issi).map(|client| client.tei)
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_solicited_group_report_pending_for_test(&self, issi: u32) -> bool {
        self.solicited_group_report_pending(issi)
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_begin_swmi_group_transaction_for_test(
        &mut self,
        issi: u32,
        handle: u32,
        group_identity_downlink: Vec<GroupIdentityDownlink>,
        detach_all_then_attach: bool,
    ) -> bool {
        if !self.client_mgr.client_is_known(issi) {
            return false;
        }
        self.pending_swmi_group_transactions.insert(
            issi,
            PendingSwmiGroupTransaction {
                handle,
                expires_at: self.dltime.add_timeslots(Self::T353_GROUP_RESPONSE_TIMESLOTS),
                group_identity_downlink,
                detach_all_then_attach,
                accepts_unrouted_ack_handle: false,
                rollback_unconfirmed_attachments_on_failure: false,
                reprobe_group_report_on_failure: false,
                remaining_restart_group_refresh: Vec::new(),
            },
        );
        true
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_swmi_group_transaction_pending_for_test(&self, issi: u32) -> bool {
        self.pending_swmi_group_transactions.contains_key(&issi)
    }

    fn stay_alive_energy_saving_information() -> EnergySavingInformation {
        EnergySavingInformation {
            energy_saving_mode: EnergySavingMode::StayAlive,
            frame_number: None,
            multiframe_number: None,
        }
    }

    fn set_client_stay_alive(&mut self, issi: u32) {
        self.pending_energy_saving.remove(&issi);
        let _ = self.client_mgr.set_client_energy_saving_mode(issi, EnergySavingMode::StayAlive);
        let _ = self.client_mgr.set_client_monitoring_window(issi, None, None);
        self.config.state_write().energy_saving.remove(&issi);
        self.emit_energy_saving_telemetry(issi, EnergySavingMode::StayAlive, None, None);
    }

    fn emit_energy_saving_telemetry(&self, issi: u32, mode: EnergySavingMode, frame: Option<u8>, multiframe: Option<u8>) {
        if let Some(sink) = &self.telemetry {
            sink.send(crate::net_telemetry::TelemetryEvent::MsEnergySaving {
                issi,
                mode: mode as u8,
                frame,
                multiframe,
            });
        }
    }

    fn configured_energy_saving_mode(&self) -> EnergySavingMode {
        Self::energy_saving_mode_from_u8(self.config.config().cell.energy_saving_mode)
    }

    fn select_energy_saving_mode(&self, requested: Option<EnergySavingMode>) -> EnergySavingMode {
        let configured = self.configured_energy_saving_mode();
        if requested == Some(EnergySavingMode::StayAlive) {
            return EnergySavingMode::StayAlive;
        }
        configured
    }

    fn energy_saving_mode_from_u8(mode: u8) -> EnergySavingMode {
        match mode {
            1 => EnergySavingMode::Eg1,
            2 => EnergySavingMode::Eg2,
            3 => EnergySavingMode::Eg3,
            4 => EnergySavingMode::Eg4,
            5 => EnergySavingMode::Eg5,
            6 => EnergySavingMode::Eg6,
            7 => EnergySavingMode::Eg7,
            _ => EnergySavingMode::StayAlive,
        }
    }

    fn energy_saving_cycle_uses_frame(mode: EnergySavingMode, start_time: TdmaTime, frame: u8) -> bool {
        let Some(sleep_frames) = EnergySavingAssignment::sleep_frames(mode as u8) else {
            return false;
        };
        let cycle_frames = sleep_frames as i32 + 1;
        let start_index = (start_time.m as i32 - 1) * 18 + (start_time.f as i32 - 1);
        let multiframe_cycle_frames = 18 * 60;
        let mut offset = 0;
        while offset < multiframe_cycle_frames {
            let current_index = (start_index + offset).rem_euclid(multiframe_cycle_frames);
            if (current_index % 18) + 1 == frame as i32 {
                return true;
            }
            offset += cycle_frames;
        }
        false
    }

    fn allocate_energy_saving_information(&self, issi: u32, mode: EnergySavingMode) -> (EnergySavingInformation, Option<TdmaTime>) {
        let mode_u8 = mode as u8;
        let Some(sleep_frames) = EnergySavingAssignment::sleep_frames(mode_u8) else {
            return (Self::stay_alive_energy_saving_information(), None);
        };

        let cycle_frames = sleep_frames + 1;
        let guard_frames = 2 * 18;
        let spread_frames = (issi % cycle_frames as u32) as i32;
        let mut start_time = self.dltime.add_timeslots((guard_frames + spread_frames) * 4);
        let mut attempts = 0;
        while Self::energy_saving_cycle_uses_frame(mode, start_time, Self::ENERGY_ECONOMY_UNSCHEDULED_SCH_F_FRAME) {
            // EN 300 392-2 clauses 16.7.1 and 16.10.10 make the ESI frame/MF the
            // absolute energy economy start point. Clause 23.7.6 derives future
            // receive frames from it, while clause 23.5.2.2.7 requires the BS to
            // send where the MS listens. Nexus-BS does not yet advertise full
            // frame-18 receive support for EG sleep cycles, so choose a valid
            // start point whose repeating EG cycle does not include frame 18.
            start_time = start_time.add_timeslots(4);
            attempts += 1;
            if attempts >= 18 {
                tracing::warn!(
                    "MM: could not allocate frame-18-safe energy economy start for ISSI {} mode {:?}; using StayAlive",
                    issi,
                    mode
                );
                return (Self::stay_alive_energy_saving_information(), None);
            }
        }

        (
            EnergySavingInformation {
                energy_saving_mode: mode,
                frame_number: Some(start_time.f),
                multiframe_number: Some(start_time.m),
            },
            Some(start_time),
        )
    }

    fn apply_energy_saving_information(&mut self, issi: u32, esi: &EnergySavingInformation, start_time: Option<TdmaTime>) {
        // EN 300 392-2 clause 16.7.1 allows energy economy to be allocated in
        // D-LOCATION UPDATE ACCEPT or negotiated by D/U-CHANGE OF ENERGY
        // SAVING MODE. A fresh accepted allocation supersedes any older
        // BS-initiated pending negotiation for the same MS.
        self.pending_energy_saving.remove(&issi);

        if esi.energy_saving_mode == EnergySavingMode::StayAlive {
            self.set_client_stay_alive(issi);
            return;
        }

        let _ = self.client_mgr.set_client_energy_saving_mode(issi, esi.energy_saving_mode);
        let _ = self
            .client_mgr
            .set_client_monitoring_window(issi, esi.frame_number, esi.multiframe_number);

        let awake_until = start_time.map(|start_time| {
            let t210_until = self.dltime.add_timeslots(Self::T210_AWAKE_FRAMES * 4);
            if start_time.diff(t210_until) >= 0 { start_time } else { t210_until }
        });
        let existing_suspension_count = self
            .config
            .state_read()
            .energy_saving
            .get(&issi)
            .map(|assignment| assignment.suspension_count)
            .unwrap_or(0);

        self.config.state_write().energy_saving.insert(
            issi,
            EnergySavingAssignment {
                mode: esi.energy_saving_mode as u8,
                frame: esi.frame_number,
                multiframe: esi.multiframe_number,
                awake_until,
                suspension_count: existing_suspension_count,
            },
        );
        self.emit_energy_saving_telemetry(issi, esi.energy_saving_mode, esi.frame_number, esi.multiframe_number);
    }

    fn has_active_energy_saving_assignment(&self, issi: u32, mode: EnergySavingMode) -> bool {
        self.config
            .state_read()
            .energy_saving
            .get(&issi)
            .copied()
            .is_some_and(|assignment| assignment.mode == mode as u8 && assignment.is_energy_economy())
    }

    fn active_energy_saving_assignment(&self, issi: u32) -> Option<EnergySavingAssignment> {
        self.config
            .state_read()
            .energy_saving
            .get(&issi)
            .copied()
            .filter(|assignment| assignment.is_energy_economy())
    }

    fn restore_energy_saving_assignment(&mut self, issi: u32, assignment: EnergySavingAssignment) {
        let mode = Self::energy_saving_mode_from_u8(assignment.mode);
        let _ = self.client_mgr.set_client_energy_saving_mode(issi, mode);
        let _ = self
            .client_mgr
            .set_client_monitoring_window(issi, assignment.frame, assignment.multiframe);
        self.config.state_write().energy_saving.insert(issi, assignment);
        self.emit_energy_saving_telemetry(issi, mode, assignment.frame, assignment.multiframe);
    }

    fn fail_pending_energy_saving_assignment(&mut self, issi: u32, pending: PendingEnergySavingAssignment, reason: &str) {
        if let Some(previous_active) = pending.previous_active {
            tracing::warn!(
                "MM: {} for BS-initiated energy saving assignment to ISSI {}; preserving previous active mode {:?}",
                reason,
                issi,
                Self::energy_saving_mode_from_u8(previous_active.mode)
            );
            self.restore_energy_saving_assignment(issi, previous_active);
        } else {
            tracing::warn!(
                "MM: {} for BS-initiated energy saving assignment to ISSI {}; keeping StayAlive",
                reason,
                issi
            );
            self.set_client_stay_alive(issi);
        }
    }

    fn forget_energy_saving(&mut self, issi: u32) {
        self.pending_energy_saving.remove(&issi);
        self.config.state_write().energy_saving.remove(&issi);
    }

    fn expire_pending_energy_saving(&mut self, now: TdmaTime) {
        let expired: Vec<u32> = self
            .pending_energy_saving
            .iter()
            .filter_map(|(&issi, pending)| if now.diff(pending.expires_at) >= 0 { Some(issi) } else { None })
            .collect();

        for issi in expired {
            if let Some(pending) = self.pending_energy_saving.remove(&issi) {
                self.fail_pending_energy_saving_assignment(issi, pending, "T352 expired");
            }
        }
    }

    fn expire_pending_swmi_group_transactions(&mut self, queue: &mut MessageQueue, now: TdmaTime) {
        let expired: Vec<u32> = self
            .pending_swmi_group_transactions
            .iter()
            .filter_map(|(&issi, pending)| if now.diff(pending.expires_at) >= 0 { Some(issi) } else { None })
            .collect();

        for issi in expired {
            if let Some(pending) = self.pending_swmi_group_transactions.remove(&issi) {
                tracing::warn!(
                    "MM: T353 expired for SwMI-initiated group attach/detach transaction to ISSI {}; discarding pending transaction",
                    issi
                );
                if pending.rollback_unconfirmed_attachments_on_failure {
                    let mut deaff_groups = Vec::new();
                    for gid in &pending.group_identity_downlink {
                        if gid.group_identity_attachment.is_some() {
                            if let Some(gssi) = GroupIdentityAddress::from_downlink(gid).plain_gssi() {
                                self.rollback_swmi_group_attachment(issi, gssi, &mut deaff_groups, "T353 expired");
                            }
                        }
                    }
                    self.finish_swmi_group_failure_recovery(queue, issi, &pending, deaff_groups, "T353 expired");
                }
            }
        }
    }

    fn arm_solicited_group_report(&mut self, issi: u32) {
        let expires_at = self.dltime.add_timeslots(Self::SOLICITED_GROUP_REPORT_WINDOW_TIMESLOTS);
        self.pending_solicited_group_reports.insert(issi, expires_at);
    }

    fn solicited_group_report_pending(&self, issi: u32) -> bool {
        self.pending_solicited_group_reports.contains_key(&issi)
    }

    fn clear_solicited_group_report(&mut self, issi: u32) {
        self.pending_solicited_group_reports.remove(&issi);
    }

    fn expire_pending_solicited_group_reports(&mut self, queue: &mut MessageQueue, now: TdmaTime) {
        let expired: Vec<u32> = self
            .pending_solicited_group_reports
            .iter()
            .filter_map(|(&issi, &expires_at)| if now.diff(expires_at) >= 0 { Some(issi) } else { None })
            .collect();

        for issi in expired {
            if self.pending_solicited_group_reports.remove(&issi).is_some() {
                let retry_group_report = self
                    .client_mgr
                    .get_client_by_issi(issi)
                    .map(|client| (client.last_handle, client.groups.is_empty()))
                    .filter(|(_, groups_empty)| *groups_empty);

                if let Some((last_handle, _)) = retry_group_report {
                    tracing::info!(
                        "MM: solicited group report window expired for ISSI {} with no attached groups; re-requesting group report",
                        issi
                    );
                    // EN 300 392-2 clause 16.4.4 lets the SwMI request a
                    // group identity report with D-LOCATION UPDATE COMMAND.
                    // Reuse the same standardized procedure if a restarted BS
                    // recovered terminal registration but no group affiliation.
                    self.send_d_location_update_command(queue, issi, last_handle);
                } else {
                    tracing::debug!(
                        "MM: solicited group report window expired for ISSI {} after D-LOCATION-UPDATE-COMMAND",
                        issi
                    );
                }
            }
        }
    }

    fn abandon_pending_swmi_group_transaction(&mut self, issi: u32, reason: &str) {
        if self.pending_swmi_group_transactions.remove(&issi).is_some() {
            tracing::debug!(
                "MM: abandoning pending SwMI-initiated group attach/detach transaction for ISSI {}: {}",
                issi,
                reason
            );
        }
    }

    fn pending_swmi_group_transaction_is_restart_refresh(&self, issi: u32) -> bool {
        self.pending_swmi_group_transactions
            .get(&issi)
            .is_some_and(|pending| pending.rollback_unconfirmed_attachments_on_failure && pending.reprobe_group_report_on_failure)
    }

    fn handle_pending_swmi_group_transaction_for_location_update(
        &mut self,
        issi: u32,
        location_update_carries_group_state: bool,
        may_preserve_restart_group_refresh: bool,
    ) {
        if location_update_carries_group_state {
            self.abandon_pending_swmi_group_transaction(
                issi,
                "accepted U-LOCATION UPDATE DEMAND with explicit group state overrides pending group attach/detach procedure",
            );
        } else if self.pending_swmi_group_transactions.contains_key(&issi) {
            if may_preserve_restart_group_refresh {
                // EN 300 392-2 clause 16.8.6 collision handling permits the
                // SwMI to keep the already-started group attach/detach
                // procedure authoritative when the colliding accepted
                // location update does not carry group state. Limit this to
                // restart-recovery refreshes; true registration overrides and
                // rejected LUs abandon pending SwMI group state elsewhere.
                tracing::debug!(
                    "MM: preserving pending restart SwMI group refresh for ISSI {} across group-less U-LOCATION UPDATE DEMAND",
                    issi
                );
            } else {
                self.abandon_pending_swmi_group_transaction(
                    issi,
                    "accepted group-less U-LOCATION UPDATE DEMAND starts or refreshes registration outside restart group refresh context",
                );
            }
        }
    }

    fn push_unique_group(groups: &mut Vec<u32>, gssi: u32) {
        if !groups.contains(&gssi) {
            groups.push(gssi);
        }
    }

    fn ack_rejects_group_identity(pdu: &UAttachDetachGroupIdentityAcknowledgement, identity: GroupIdentityAddress) -> bool {
        if !pdu.group_identity_acknowledgement_type {
            return false;
        }
        pdu.group_identity_uplink
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|entry| entry.group_identity_detachment_uplink.is_some() && GroupIdentityAddress::from_uplink(entry) == identity)
    }

    fn reject_ack_has_rejected_pending_attachment(
        pending: &PendingSwmiGroupTransaction,
        pdu: &UAttachDetachGroupIdentityAcknowledgement,
    ) -> bool {
        pending.group_identity_downlink.iter().any(|gid| {
            gid.group_identity_attachment.is_some() && Self::ack_rejects_group_identity(pdu, GroupIdentityAddress::from_downlink(gid))
        })
    }

    fn detach_all_groups_for_swmi_transaction(&mut self, issi: u32, deaff_groups: &mut Vec<u32>) -> bool {
        let prior_groups: Vec<u32> = self
            .client_mgr
            .get_client_by_issi(issi)
            .map(|client| client.groups.iter().copied().collect())
            .unwrap_or_default();
        match self.client_mgr.client_detach_all_groups(issi) {
            Ok(_) => {
                if !prior_groups.is_empty() {
                    let mut state = self.config.state_write();
                    for gssi in prior_groups {
                        state.subscribers.deaffiliate(issi, gssi);
                        Self::push_unique_group(deaff_groups, gssi);
                    }
                }
                true
            }
            Err(e) => {
                tracing::warn!("MM: failed detach-all for SwMI-initiated group transaction ISSI {}: {:?}", issi, e);
                false
            }
        }
    }

    fn rollback_swmi_group_attachment(&mut self, issi: u32, gssi: u32, deaff_groups: &mut Vec<u32>, reason: &str) {
        let shared_affiliated_before = self.config.state_read().subscribers.contains_group_member(gssi, issi);
        match self.client_mgr.client_group_attach(issi, gssi, false) {
            Ok(changed) => {
                if changed || shared_affiliated_before {
                    if shared_affiliated_before {
                        self.config.state_write().subscribers.deaffiliate(issi, gssi);
                    }
                    Self::push_unique_group(deaff_groups, gssi);
                    tracing::warn!(
                        "MM: rolled back unconfirmed SwMI group attachment ISSI {} GSSI {} after {}",
                        issi,
                        gssi,
                        reason
                    );
                }
            }
            Err(e) => tracing::warn!(
                "MM: failed rolling back unconfirmed SwMI group attachment ISSI {} GSSI {} after {}: {:?}",
                issi,
                gssi,
                reason,
                e
            ),
        }
    }

    fn finish_swmi_group_failure_recovery(
        &mut self,
        queue: &mut MessageQueue,
        issi: u32,
        pending: &PendingSwmiGroupTransaction,
        deaff_groups: Vec<u32>,
        reason: &str,
    ) {
        if !deaff_groups.is_empty() {
            self.emit_subscriber_update(queue, issi, deaff_groups, BrewSubscriberAction::Deaffiliate);
        }
        self.remember_restart_recovery_issi_with_remaining(issi, &pending.remaining_restart_group_refresh);
        if pending.reprobe_group_report_on_failure {
            tracing::info!(
                "MM: requesting fresh group report from ISSI {} after failed restart group refresh ({})",
                issi,
                reason
            );
            self.send_d_location_update_command(queue, issi, 0);
        }
    }

    fn apply_swmi_group_ack(
        &mut self,
        queue: &mut MessageQueue,
        issi: u32,
        pending: PendingSwmiGroupTransaction,
        pdu: &UAttachDetachGroupIdentityAcknowledgement,
    ) {
        let mut aff_groups = Vec::new();
        let mut deaff_groups = Vec::new();

        if pending.detach_all_then_attach && !self.detach_all_groups_for_swmi_transaction(issi, &mut deaff_groups) {
            return;
        }

        for gid in &pending.group_identity_downlink {
            let identity = GroupIdentityAddress::from_downlink(gid);
            let Some(gssi) = identity.plain_gssi() else {
                tracing::warn!(
                    "MM: SwMI group ACK transaction for ISSI {} contains unsupported non-GSSI group identity {:?}; local affiliation unchanged",
                    issi,
                    gid
                );
                continue;
            };

            if gid.group_identity_detachment_uplink.is_some() {
                // EN 300 392-2 Annex G: an MS cannot reject a SwMI requested
                // detachment. Apply detachment regardless of ACK type/list.
                match self.client_mgr.client_group_attach(issi, gssi, false) {
                    Ok(changed) => {
                        if changed {
                            self.config.state_write().subscribers.deaffiliate(issi, gssi);
                            Self::push_unique_group(&mut deaff_groups, gssi);
                        }
                    }
                    Err(e) => tracing::warn!("MM: failed SwMI-requested group detach ISSI {} GSSI {}: {:?}", issi, gssi, e),
                }
                continue;
            }

            if let Some(attachment) = &gid.group_identity_attachment {
                if Self::ack_rejects_group_identity(pdu, identity) {
                    tracing::debug!(
                        "MM: MS {} explicitly rejected SwMI-requested group attachment to GSSI {}",
                        issi,
                        gssi
                    );
                    if pending.rollback_unconfirmed_attachments_on_failure {
                        self.rollback_swmi_group_attachment(
                            issi,
                            gssi,
                            &mut deaff_groups,
                            "U-ATTACH/DETACH GROUP IDENTITY ACK rejected attachment",
                        );
                    }
                    continue;
                }
                let attachment_info = GroupAttachmentInfo {
                    group_identity_attachment_lifetime: attachment.group_identity_attachment_lifetime,
                    class_of_usage: attachment.class_of_usage,
                };
                match self.client_mgr.client_group_attach_with_info(issi, gssi, true, attachment_info) {
                    Ok(changed) => {
                        if changed {
                            self.config.state_write().subscribers.affiliate(issi, gssi);
                            Self::push_unique_group(&mut aff_groups, gssi);
                        }
                    }
                    Err(e) => tracing::warn!("MM: failed SwMI-requested group attach ISSI {} GSSI {}: {:?}", issi, gssi, e),
                }
            }
        }

        let had_deaff_groups = !deaff_groups.is_empty();
        if had_deaff_groups {
            self.emit_subscriber_update(queue, issi, deaff_groups, BrewSubscriberAction::Deaffiliate);
        }
        if !aff_groups.is_empty() {
            self.emit_subscriber_update(queue, issi, aff_groups, BrewSubscriberAction::Affiliate);
        }
        if pending.rollback_unconfirmed_attachments_on_failure && had_deaff_groups {
            self.finish_swmi_group_failure_recovery(
                queue,
                issi,
                &pending,
                Vec::new(),
                "U-ATTACH/DETACH GROUP IDENTITY ACK rejected attachment",
            );
        } else if pending.rollback_unconfirmed_attachments_on_failure && !pending.remaining_restart_group_refresh.is_empty() {
            let split_at = pending.remaining_restart_group_refresh.len().min(Self::MAX_GROUPS_PER_ATTACH);
            let (next_batch, remaining) = pending.remaining_restart_group_refresh.split_at(split_at);
            let next_refresh = self.restore_cached_restart_recovery_group_batch(queue, issi, next_batch);
            if !next_refresh.is_empty() {
                self.send_swmi_group_attach_refresh(queue, issi, &next_refresh, remaining.to_vec());
            }
            self.remember_restart_recovery_issi(issi);
        } else {
            self.remember_restart_recovery_issi(issi);
        }
    }

    fn restore_shared_subscriber_state_for_reported_groups(&mut self, queue: &mut MessageQueue, issi: u32, groups: &[u32]) {
        let (needs_register, missing_groups) = {
            let state = self.config.state_read();
            let needs_register = !state.subscribers.is_registered(issi);
            let missing_groups = groups
                .iter()
                .copied()
                .filter(|gssi| !state.subscribers.contains_group_member(*gssi, issi))
                .collect::<Vec<u32>>();
            (needs_register, missing_groups)
        };

        if !needs_register && missing_groups.is_empty() {
            return;
        }

        {
            let mut state = self.config.state_write();
            if needs_register {
                state.subscribers.register(issi);
            }
            for gssi in &missing_groups {
                state.subscribers.affiliate(issi, *gssi);
            }
        }

        // EN 300 392-2 clauses 16.8.0 and 16.8.4 make reported downlink
        // group identities valid attached identities. Keep the local routing
        // registry coherent with any groups MM advertises back to the MS.
        if needs_register {
            self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Register);
        }
        if !missing_groups.is_empty() {
            self.emit_subscriber_update(queue, issi, missing_groups, BrewSubscriberAction::Affiliate);
        }
    }

    fn restore_shared_subscriber_registration_for_known_client(&mut self, queue: &mut MessageQueue, issi: u32) -> bool {
        if self.config.state_read().subscribers.is_registered(issi) {
            return false;
        }
        if !self.client_mgr.client_is_known(issi) {
            return false;
        }

        self.config.state_write().subscribers.register(issi);

        // EN 300 392-2 clauses 16.4.3 and 16.8.2 define standalone group
        // attach/detach as an MM procedure for an attached MS. Clause 16.9.2.8
        // lets this local watchdog ask for re-registration with
        // D-LOCATION-UPDATE-COMMAND, but while the known client is still in the
        // grace window the shared routing registry must be restored before any
        // accepted group affiliation is advertised.
        self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Register);
        true
    }

    fn emit_current_group_snapshot(&mut self, issi: u32) {
        let Some(sink) = self.client_mgr.telemetry_sink().cloned() else {
            return;
        };
        let all_groups: Vec<u32> = self
            .client_mgr
            .get_client_by_issi(issi)
            .map(|c| c.groups.iter().copied().collect())
            .unwrap_or_default();
        sink.send(crate::net_telemetry::TelemetryEvent::MsGroupsSnapshot { issi, gssis: all_groups });
    }

    fn retainable_mode_one_attach_groups(&self, issi: u32, giu_for_mode: &[GroupIdentityUplink]) -> BTreeSet<u32> {
        giu_for_mode
            .iter()
            .filter_map(|giu| {
                if giu.group_identity_detachment_uplink.is_some() || giu.vgssi.is_some() || giu.address_extension.is_some() {
                    return None;
                }
                let gssi = giu.gssi?;
                if gssi > 0x00FF_FFFF {
                    return None;
                }
                if self
                    .config
                    .config()
                    .cell
                    .allowed_gssi_ranges
                    .as_ref()
                    .is_some_and(|ranges| !ranges.contains(gssi))
                {
                    tracing::warn!("Rejecting group attach from ISSI {} to unprovisioned GSSI {}", issi, gssi);
                    return None;
                }
                Some(gssi)
            })
            .collect()
    }

    fn prepare_detach_all_then_attach(&mut self, queue: &mut MessageQueue, issi: u32, giu_for_mode: &[GroupIdentityUplink]) -> bool {
        let prior_groups: Vec<u32> = self
            .client_mgr
            .get_client_by_issi(issi)
            .map(|client| client.groups.iter().copied().collect())
            .unwrap_or_default();
        let retained_groups = self.retainable_mode_one_attach_groups(issi, giu_for_mode);

        if let Err(e) = self.client_mgr.client_detach_all_groups_silent(issi) {
            tracing::warn!("Failed detaching all groups for MS {}: {:?}", issi, e);
            return false;
        }

        let deaff_groups: Vec<u32> = prior_groups.into_iter().filter(|gssi| !retained_groups.contains(gssi)).collect();
        if !deaff_groups.is_empty() {
            {
                let mut state = self.config.state_write();
                for &gssi in &deaff_groups {
                    state.subscribers.deaffiliate(issi, gssi);
                }
            }
            self.emit_subscriber_update(queue, issi, deaff_groups, BrewSubscriberAction::Deaffiliate);
        }

        true
    }

    fn restore_cached_restart_recovery_groups(&mut self, queue: &mut MessageQueue, issi: u32) -> CachedRestartGroupRefresh {
        let cached_groups: Vec<(u32, GroupAttachmentInfo)> = self.cached_restart_recovery_groups_for_issi(issi).into_iter().collect();
        if cached_groups.is_empty() {
            return CachedRestartGroupRefresh {
                groups: Vec::new(),
                remaining: Vec::new(),
            };
        }

        if cached_groups.len() > Self::MAX_GROUPS_PER_ATTACH {
            tracing::warn!(
                "MM: cached restart recovery for ISSI {} has {} groups; sending SwMI refresh in {}-group batches",
                issi,
                cached_groups.len(),
                Self::MAX_GROUPS_PER_ATTACH
            );
        }

        let split_at = cached_groups.len().min(Self::MAX_GROUPS_PER_ATTACH);
        let (batch, remaining) = cached_groups.split_at(split_at);
        let groups = self.restore_cached_restart_recovery_group_batch(queue, issi, batch);

        CachedRestartGroupRefresh {
            groups,
            remaining: remaining.to_vec(),
        }
    }

    fn restore_cached_restart_recovery_group_batch(
        &mut self,
        queue: &mut MessageQueue,
        issi: u32,
        cached_groups: &[(u32, GroupAttachmentInfo)],
    ) -> Vec<(u32, GroupAttachmentInfo)> {
        let mut restored_groups = Vec::new();
        for &(gssi, attachment_info) in cached_groups {
            let shared_affiliated_before = self.config.state_read().subscribers.contains_group_member(gssi, issi);
            match self.client_mgr.client_group_attach_with_info(issi, gssi, true, attachment_info) {
                Ok(changed) => {
                    if changed || !shared_affiliated_before {
                        self.restore_shared_subscriber_registration_for_known_client(queue, issi);
                    }
                    if !shared_affiliated_before && self.config.state_write().subscribers.affiliate(issi, gssi) {
                        restored_groups.push((gssi, attachment_info));
                    }
                }
                Err(err) => {
                    tracing::warn!("MM: failed restoring cached restart group ISSI {} GSSI {}: {:?}", issi, gssi, err);
                }
            }
        }

        if !restored_groups.is_empty() {
            let restored_gssis: Vec<u32> = restored_groups.iter().map(|(gssi, _)| *gssi).collect();
            tracing::info!(
                "MM: restored cached restart group affiliation for ISSI {} groups={:?}",
                issi,
                restored_gssis
            );
            self.emit_subscriber_update(queue, issi, restored_gssis, BrewSubscriberAction::Affiliate);
            self.emit_current_group_snapshot(issi);
        }

        restored_groups
    }

    fn parse_u_mm_status_energy_saving_mode(pdu: &UMmStatus, issi: u32) -> Option<EnergySavingMode> {
        let Some(dep_info) = pdu.status_uplink_dependent_information else {
            tracing::warn!(
                "MM: malformed {:?} from ISSI {}: missing energy saving mode",
                pdu.status_uplink,
                issi
            );
            return None;
        };
        let dep_len = pdu.status_uplink_dependent_information_len.unwrap_or(0);
        if dep_len < 3 {
            tracing::warn!(
                "MM: malformed {:?} from ISSI {}: energy saving mode is {} bits, expected at least 3",
                pdu.status_uplink,
                issi,
                dep_len
            );
            return None;
        }

        let mode_val = dep_info >> (dep_len - 3);
        let mode = match EnergySavingMode::try_from(mode_val) {
            Ok(mode) => mode,
            Err(_) => {
                tracing::warn!(
                    "MM: malformed {:?} from ISSI {}: invalid energy saving mode {}",
                    pdu.status_uplink,
                    issi,
                    mode_val
                );
                return None;
            }
        };

        let trailing_len = dep_len - 3;
        if trailing_len == 0 {
            return Some(mode);
        }

        let trailing_mask = if trailing_len == 64 { u64::MAX } else { (1u64 << trailing_len) - 1 };
        let trailing = dep_info & trailing_mask;
        let mut trailing_buf = BitBuffer::new_autoexpand(trailing_len);
        trailing_buf.write_bits(trailing, trailing_len);
        trailing_buf.seek(0);

        // EN 300 392-2 tables 16.20/16.21 define the dependent data as the
        // mandatory 3-bit energy saving mode followed only by optional uplink
        // Type 3 Proprietary. A lone zero m-bit is accepted as "no Type 3 IE".
        let proprietary = match typed::parse_type3_generic(true, &mut trailing_buf, MmType34ElemIdUl::Proprietary) {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(
                    "MM: malformed {:?} from ISSI {}: invalid trailing Type 3 proprietary data after energy saving mode: {:?}",
                    pdu.status_uplink,
                    issi,
                    err
                );
                return None;
            }
        };
        match delimiters::read_mbit(&mut trailing_buf) {
            Ok(false) if trailing_buf.get_len_remaining() == 0 => Some(mode),
            Ok(false) => {
                tracing::warn!(
                    "MM: malformed {:?} from ISSI {}: {} extra bit(s) after Type 3 proprietary terminator",
                    pdu.status_uplink,
                    issi,
                    trailing_buf.get_len_remaining()
                );
                None
            }
            Ok(true) => {
                tracing::warn!(
                    "MM: malformed {:?} from ISSI {}: unsupported trailing Type 3 IE after energy saving mode (only proprietary is supported); parsed proprietary={}",
                    pdu.status_uplink,
                    issi,
                    proprietary.is_some()
                );
                None
            }
            Err(err) => {
                tracing::warn!(
                    "MM: malformed {:?} from ISSI {}: missing Type 3 terminator after energy saving mode: {:?}",
                    pdu.status_uplink,
                    issi,
                    err
                );
                None
            }
        }
    }

    /// Force CMCE to release any individual P2P calls involving the given ISSI,
    /// without touching MM/Brew registration or accepted GSSI affiliations.
    /// Used on soft re-attach to prevent stale call state from causing PTT
    /// denial on the next private call.
    fn emit_individual_call_release_for_issi(&self, queue: &mut MessageQueue, issi: u32) {
        let release = MmSubscriberUpdate {
            issi,
            groups: Vec::new(),
            action: BrewSubscriberAction::ReleaseIndividualCalls,
        };
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Mm,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::MmSubscriberUpdate(release),
        });

        tracing::info!(
            "MM: requested CMCE individual-call cleanup for ISSI {} while preserving group affiliation (soft re-attach)",
            issi
        );
    }

    fn emit_subscriber_update(&self, queue: &mut MessageQueue, issi: u32, groups: Vec<u32>, action: BrewSubscriberAction) {
        // If brew is active, forward subscriber updates to the Brew entity.
        // Register/Deregister must always be sent for brew-routable ISSIs,
        // even when there are no group affiliations yet. The Brew worker
        // decides whether to send REGISTER or REREGISTER based on its own state.
        // Affiliate/Deaffiliate only sent when there are brew-routable groups.
        if net_brew::is_active(&self.config) {
            let brew_groups = groups
                .iter()
                .filter(|gssi| net_brew::is_brew_gssi_routable(&self.config, **gssi))
                .copied()
                .collect::<Vec<u32>>();
            let should_send = match action {
                BrewSubscriberAction::Register | BrewSubscriberAction::Deregister => net_brew::is_brew_issi_routable(&self.config, issi),
                BrewSubscriberAction::Affiliate | BrewSubscriberAction::Deaffiliate => !brew_groups.is_empty(),
                BrewSubscriberAction::ReleaseIndividualCalls => false,
            };
            if should_send {
                let brew_update = MmSubscriberUpdate {
                    issi,
                    groups: brew_groups,
                    action,
                };
                let msg = SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Mm,
                    dest: TetraEntity::Brew,
                    msg: SapMsgInner::MmSubscriberUpdate(brew_update),
                };
                queue.push_back(msg);
            }
        }

        // Always emit an update to the Cmce entity
        let mm_update = MmSubscriberUpdate { issi, groups, action };
        let msg = SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Mm,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::MmSubscriberUpdate(mm_update),
        };
        queue.push_back(msg);
    }

    fn rx_u_itsi_detach(&mut self, _queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_u_itsi_detach");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let pdu = match UItsiDetach::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing UItsiDetach: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        // Check if we can satisfy this request, print unsupported stuff
        if !Self::feature_check_u_itsi_detach(&pdu) {
            tracing::error!("Unsupported critical features in UItsiDetach");
            return;
        }

        let ssi = prim.received_address.ssi;
        self.abandon_pending_swmi_group_transaction(
            ssi,
            "U-ITSI DETACH terminates the registered MM context before any pending SwMI group ACK",
        );
        self.clear_solicited_group_report(ssi);
        self.clear_critical_downlinks_for_issi(ssi);
        self.forget_restart_recovery_issi(ssi);
        let detached_client = self.client_mgr.remove_client(ssi);
        if let Some(client) = detached_client {
            self.forget_energy_saving(ssi);
            self.config.state_write().subscribers.deregister(ssi);
            if !client.groups.is_empty() {
                let groups: Vec<u32> = client.groups.iter().copied().collect();
                self.emit_subscriber_update(_queue, ssi, groups, BrewSubscriberAction::Deaffiliate);
            }
            self.emit_subscriber_update(_queue, ssi, Vec::new(), BrewSubscriberAction::Deregister);
        } else {
            tracing::warn!("Received UItsiDetach for unknown client with SSI: {}", ssi);
            // return;
        };
    }

    fn rx_u_location_update_demand(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_location_update_demand");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let pdu = match ULocationUpdateDemand::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing ULocationUpdateDemand: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        let received_issi = prim.received_address.ssi;
        if let Some(pdu_ssi) = pdu.ssi
            && pdu_ssi != received_issi as u64
        {
            tracing::warn!(
                "MM: U-LOCATION UPDATE DEMAND SSI {} does not match received L2 address {}; rejecting",
                pdu_ssi,
                received_issi
            );
            self.abandon_pending_swmi_group_transaction(
                received_issi,
                "rejected U-LOCATION UPDATE DEMAND with mismatched SSI cannot complete pending SwMI group procedure",
            );
            Self::send_d_location_update_reject_cause(
                queue,
                received_issi,
                prim.handle,
                pdu.location_update_type,
                pdu.address_extension,
                RejectCause::MessageConsistencyError,
            );
            return;
        }

        if let Some(pdu_mni) = pdu.address_extension {
            let expected_mni = self.expected_ms_mni();
            if pdu_mni != expected_mni {
                tracing::warn!(
                    "MM: U-LOCATION UPDATE DEMAND MNI {} does not match configured network MNI {}; rejecting",
                    pdu_mni,
                    expected_mni
                );
                self.abandon_pending_swmi_group_transaction(
                    received_issi,
                    "rejected U-LOCATION UPDATE DEMAND with mismatched MNI cannot complete pending SwMI group procedure",
                );
                Self::send_d_location_update_reject_cause(
                    queue,
                    received_issi,
                    prim.handle,
                    pdu.location_update_type,
                    pdu.address_extension,
                    RejectCause::MessageConsistencyError,
                );
                return;
            }
        }

        // Migration not supported: ETSI 16.4.1.1 case b) requires identity exchange via
        // D-LOCATION-UPDATE-PROCEEDING which we don't implement. Reject with cause
        // "Migration not supported" (12, Table 16.81) so the MS can act on it.
        if pdu.location_update_type == LocationUpdateType::MigratingLocationUpdating
            || pdu.location_update_type == LocationUpdateType::ServiceRestorationMigratingLocationUpdating
        {
            // Terminal wants to migrate to another network (e.g. SmartConnect).
            // We don't implement D-LOCATION-UPDATE-PROCEEDING identity exchange (ETSI §16.4.1.1 case b),
            // so we can't accept migration formally. But we MUST release the terminal from Brew
            // so the destination network can register it without identity conflict.
            // Send REJECT so terminal knows to try the other network, but first deregister from Brew.
            let issi = received_issi;
            tracing::info!("MM: ISSI {} migrating to another network — releasing from Brew", issi);
            self.abandon_pending_swmi_group_transaction(
                issi,
                "migration U-LOCATION UPDATE DEMAND starts an unsupported registration path and rejects pending SwMI group procedure",
            );
            self.clear_solicited_group_report(issi);
            self.clear_critical_downlinks_for_issi(issi);
            self.forget_restart_recovery_issi(issi);
            let detached = self.client_mgr.remove_client(issi);
            if let Some(client) = detached {
                self.set_client_stay_alive(issi);
                self.config.state_write().subscribers.deregister(issi);
                if !client.groups.is_empty() {
                    let groups: Vec<u32> = client.groups.iter().copied().collect();
                    self.emit_subscriber_update(queue, issi, groups, BrewSubscriberAction::Deaffiliate);
                }
                self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
            }
            Self::send_d_location_update_reject_cause(
                queue,
                issi,
                prim.handle,
                pdu.location_update_type,
                pdu.address_extension,
                RejectCause::MigrationNotSupported,
            );
            return;
        }

        if pdu.location_update_type == LocationUpdateType::DisabledMsUpdating {
            // EN 300 392-2 clause 16.9.3.4 defines U-LOCATION UPDATE DEMAND as
            // answered by D-LOCATION UPDATE ACCEPT/REJECT. This SwMI does not
            // support disabled MS updating, so preserve the requested LU type in
            // the reject instead of silently dropping the procedure.
            self.abandon_pending_swmi_group_transaction(
                received_issi,
                "disabled-MS U-LOCATION UPDATE DEMAND is rejected and cannot complete pending SwMI group procedure",
            );
            Self::send_d_location_update_reject_cause(
                queue,
                received_issi,
                prim.handle,
                pdu.location_update_type,
                pdu.address_extension,
                RejectCause::ServiceNotSubscribed,
            );
            return;
        }

        // Check if we can satisfy this request, print unsupported stuff
        if let Some(reject_cause) = Self::reject_cause_for_unsupported_u_location_update_demand(&pdu) {
            tracing::error!(
                "Unsupported critical features in ULocationUpdateDemand; rejecting with {}",
                reject_cause
            );
            self.abandon_pending_swmi_group_transaction(
                received_issi,
                "unsupported U-LOCATION UPDATE DEMAND is rejected and cannot complete pending SwMI group procedure",
            );
            Self::send_d_location_update_reject_cause(
                queue,
                received_issi,
                prim.handle,
                pdu.location_update_type,
                pdu.address_extension,
                reject_cause,
            );
            return;
        }

        let configured_esm = self.configured_energy_saving_mode();
        let energy_saving_assignment = if pdu.energy_saving_mode.is_some() || configured_esm == EnergySavingMode::StayAlive {
            let allocated_esm = self.select_energy_saving_mode(pdu.energy_saving_mode);
            let (esi, start_time) = self.allocate_energy_saving_information(prim.received_address.ssi, allocated_esm);
            if pdu.energy_saving_mode != Some(allocated_esm) {
                tracing::info!(
                    "MS {} requested energy saving mode {:?}; allocating {:?}",
                    prim.received_address.ssi,
                    pdu.energy_saving_mode,
                    allocated_esm
                );
            }
            Some((esi, start_time))
        } else {
            None
        };

        // Try to register the client
        let issi = received_issi;
        let handle = prim.handle;
        let location_update_carries_group_state =
            pdu.group_identity_location_demand.is_some() || Self::group_report_response_is_complete(&pdu);
        let was_solicited_group_report_pending = self.solicited_group_report_pending(issi);
        let was_restart_recovery_candidate = self.restart_recovery.contains_key(&issi);

        // ISSI whitelist check — reject if whitelist is non-empty and ISSI not in it.
        // The dashboard can override the config whitelist at runtime (state override takes
        // precedence so edits apply without a restart); fall back to the config value when
        // no override is set. An empty list (in either place) means "open network".
        let issi_allowed = {
            let state = self.config.state_read();
            match &state.issi_whitelist_override {
                Some(list) => list.is_empty() || list.contains(&issi),
                None => self.config.config().security.is_issi_allowed(issi),
            }
        };
        if !issi_allowed {
            tracing::warn!("MM: ISSI {} not in whitelist, rejecting registration", issi);
            self.abandon_pending_swmi_group_transaction(
                issi,
                "whitelist-rejected U-LOCATION UPDATE DEMAND cannot complete pending SwMI group procedure",
            );
            Self::send_d_location_update_reject_cause(
                queue,
                issi,
                handle,
                pdu.location_update_type,
                pdu.address_extension,
                RejectCause::ServiceNotSubscribed,
            );
            return;
        }

        let was_pending = self.client_mgr.is_pending_command(issi);
        let is_new = !self.client_mgr.client_is_known(issi);
        let mut soft_reattach_cmce_reset = false;
        let mut hard_reregistration_cleanup = false;
        if !is_new {
            // MS is re-registering while already known. Three cases:
            //
            // A) RoamingLocationUpdating — MS re-registered from scratch (RF loss / reboot /
            //    power-cycle, no prior U-ITSI-DETACH). Clean up stale state so CMCE releases
            //    any ghost calls and group_listeners stays accurate.
            //
            // B) PeriodicLocationUpdating — healthy MS renewing the local watchdog. No cleanup.
            //
            // C) DemandLocationUpdating — MS responding to our D-LOCATION-UPDATE-COMMAND.
            //    This is the second message in the normal registration flow; the first message
            //    already registered+affiliated the MS. Do NOT clean up here.
            let needs_cleanup = if pdu.location_update_type == LocationUpdateType::RoamingLocationUpdating
                || pdu.location_update_type == LocationUpdateType::ServiceRestorationRoamingLocationUpdating
            {
                // Some terminals (e.g. Sepura) send RoamingLocationUpdating after every PTT
                // release, not just on power-cycle or RF loss. If we treat this as a full reboot
                // and do deregister→register, CMCE has a brief window where it doesn't know the
                // terminal — a PTT press in that window gets "no listeners" and the terminal
                // interprets it as a network error and fully disconnects.
                //
                // Heuristic: treat RoamingLocationUpdating as a soft re-attach (no cleanup) if
                // the terminal registered less than 120 seconds ago.
                let recently_registered = self
                    .client_mgr
                    .get_client_by_issi(issi)
                    .map(|c| c.last_registration_time.elapsed().as_secs() < 120)
                    .unwrap_or(false);
                if recently_registered {
                    tracing::debug!(
                        "MM: ISSI {} RoamingLocationUpdating within 120s of last register — treating as soft re-attach (Sepura post-PTT)",
                        issi
                    );
                    // Even on soft re-attach, force CMCE to release any individual P2P calls
                    // involving this ISSI. Terminals (e.g. Motorola MTP3550) that drop RF for
                    // 2s and re-attach lose call state but BS keeps the call alive — next PTT
                    // is rejected ("PTT denied") because the terminal doesn't recognize the call_id
                    // in our D-TX-GRANTED. Releasing the individual call here forces a clean U-SETUP
                    // on the next PTT.
                    self.emit_individual_call_release_for_issi(queue, issi);
                    soft_reattach_cmce_reset = true;
                    false
                } else {
                    true
                }
            } else {
                false
            };

            // needs_cleanup: Roaming = MS rebooted, need CMCE reset
            // was_pending: local watchdog expired, we already sent Deregister to Brew — just re-register
            if needs_cleanup {
                hard_reregistration_cleanup = true;
                let old_groups: Vec<u32> = self
                    .client_mgr
                    .get_client_by_issi(issi)
                    .map(|c| c.groups.iter().copied().collect())
                    .unwrap_or_default();
                if !old_groups.is_empty() {
                    self.emit_subscriber_update(queue, issi, old_groups, BrewSubscriberAction::Deaffiliate);
                }
                if let Err(e) = self.client_mgr.client_detach_all_groups(issi) {
                    tracing::warn!("Failed clearing stale groups for re-registering MS {}: {:?}", issi, e);
                }
                // EN 300 392-2 clauses 16.4.1.1/16.7.1: a hard
                // RoamingLocationUpdating re-registration establishes fresh
                // accepted MM state. Reset shared routing/EG state alongside
                // client_mgr so stale GSSI listeners or active EG assignment
                // cannot survive into the new registration.
                self.set_client_stay_alive(issi);
                self.config.state_write().subscribers.register(issi);
                self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
                self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Register);
            } else if was_pending {
                // Local-watchdog re-registration: Brew already got Deregister — just re-register
                // CMCE gets a fresh affiliate when groups are processed below.
                // EN 300 392-2 clauses 16.9.2.8 and 16.9.3.4 make the
                // DemandLocationUpdating response a location-registration
                // update; if we accept it, the shared subscriber registry
                // must exist before group identity processing can affiliate it.
                tracing::info!("MM: ISSI {} re-registered after local periodic-registration command", issi);
                self.config.state_write().subscribers.register(issi);
            }
            // Always reset the registration timer on any re-registration
            self.client_mgr.reset_registration_timer(issi);
        }

        let may_preserve_restart_group_refresh =
            !is_new && !hard_reregistration_cleanup && self.pending_swmi_group_transaction_is_restart_refresh(issi);
        self.handle_pending_swmi_group_transaction_for_location_update(
            issi,
            location_update_carries_group_state,
            may_preserve_restart_group_refresh,
        );

        // Determine if we need to emit Register toward Brew.
        // We do this when:
        //   A) Terminal is genuinely new (never seen before).
        //   B) Terminal is known but re-attaching via ItsiAttach — migrated from another network.
        //   C) Terminal is known but had pending_command_sent=true — local watchdog expired, we sent COMMAND
        //      and deregistered from Brew. Now terminal is back, re-register.
        let is_itsi_attach = pdu.location_update_type == LocationUpdateType::ItsiAttach;
        let needs_brew_register = is_new || (!is_new && is_itsi_attach) || (!is_new && was_pending);

        if is_new {
            match self.client_mgr.try_register_client(issi, true) {
                Ok(_) => {
                    self.config.state_write().subscribers.register(issi);
                }
                Err(e) => {
                    tracing::warn!("Failed registering roaming MS {}: {:?}", issi, e);
                    return;
                }
            }
        } else if let Err(e) = self.client_mgr.set_client_state(issi, MmClientState::Attached) {
            tracing::warn!("Failed updating roaming MS {}: {:?}", issi, e);
            return;
        }
        if needs_brew_register {
            if !is_new {
                tracing::info!(
                    "MM: ISSI {} re-attaching via ItsiAttach (returned from another network) — re-registering in Brew",
                    issi
                );
            }
            self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Register);
        }

        // Always update the last known L2 handle so we can send downlink PDUs later
        // (e.g. D-LOCATION-UPDATE-COMMAND after Brew reconnection).
        self.client_mgr.set_client_handle(issi, handle);

        let mut post_attach_energy_saving_request = None;
        let mut esi_for_accept = None;
        if let Some((ref esi, start_time)) = energy_saving_assignment {
            self.apply_energy_saving_information(issi, esi, start_time);
            esi_for_accept = Some(esi.clone());
        } else if configured_esm != EnergySavingMode::StayAlive {
            if self.has_active_energy_saving_assignment(issi, configured_esm) {
                // EN 300 392-2 clause 16.7.1 permits the BS to change or
                // allocate an MS energy economy mode with D-MM STATUS. If the
                // same valid mode is already active, avoid a redundant
                // allocation request and a fresh T352 window that could later
                // clear a working assignment.
                tracing::debug!(
                    "MM: ISSI {} already has active {:?} assignment; not sending duplicate D-CHANGE OF ENERGY SAVING MODE REQUEST",
                    issi,
                    configured_esm
                );
            } else {
                let previous_active = self.active_energy_saving_assignment(issi);
                let (esi, start_time) = self.allocate_energy_saving_information(issi, configured_esm);
                self.pending_energy_saving.insert(
                    issi,
                    PendingEnergySavingAssignment {
                        esi: esi.clone(),
                        start_time,
                        expires_at: self.dltime.add_timeslots(Self::T352_ENERGY_RESPONSE_TIMESLOTS),
                        previous_active,
                    },
                );
                post_attach_energy_saving_request = Some(esi);
            }
        } else {
            self.set_client_stay_alive(issi);
        }

        let group_report_complete = Self::group_report_response_is_complete(&pdu);

        // Process optional GroupIdentityLocationDemand field
        let _has_groups = pdu.group_identity_location_demand.is_some();
        let mut cached_restart_group_refresh = CachedRestartGroupRefresh {
            groups: Vec::new(),
            remaining: Vec::new(),
        };
        let gila = if let Some(gild) = pdu.group_identity_location_demand {
            let giu_for_mode = gild
                .group_identity_uplink
                .as_ref()
                .map(|giu| Self::group_identity_uplink_for_mode(gild.group_identity_attach_detach_mode == 1, giu));
            let over_ack_cap = giu_for_mode
                .as_ref()
                .is_some_and(|giu| self.group_identity_uplink_exceeds_ack_capacity(issi, giu.len()));

            let group_result = if over_ack_cap {
                Some(GroupIdentityProcessResult {
                    group_identity_downlink: giu_for_mode
                        .as_ref()
                        .map(|giu| Self::rejected_group_identity_downlinks(giu))
                        .unwrap_or_default(),
                    all_accepted: false,
                })
            } else {
                // ETSI Table 16.49 (clause 16.10.17): mode=1 means "detach all currently
                // attached group identities and attach group identities defined in the
                // group identity uplink element." Apply that mutation only after the
                // request has passed local ACK-capacity validation, otherwise Annex G
                // cannot be represented coherently and the procedure remains unchanged.
                if gild.group_identity_attach_detach_mode == 1 {
                    // EN 300 392-2 clause 16.10.17 mode=1 is one logical
                    // replace operation: detach all current groups, then attach
                    // the listed groups. Keep retained GSSIs affiliated in the
                    // shared routing/dashboard state so a restart refresh for
                    // the same scan group cannot create a transient No Group.
                    if !self.prepare_detach_all_then_attach(queue, issi, giu_for_mode.as_deref().unwrap_or(&[])) {
                        Some(GroupIdentityProcessResult {
                            group_identity_downlink: giu_for_mode
                                .as_ref()
                                .map(|giu| Self::rejected_group_identity_downlinks(giu))
                                .unwrap_or_default(),
                            all_accepted: false,
                        })
                    } else {
                        if giu_for_mode.is_none() {
                            self.emit_current_group_snapshot(issi);
                        }
                        giu_for_mode.as_ref().map(|giu| self.try_attach_detach_groups(queue, issi, giu))
                    }
                } else {
                    giu_for_mode.as_ref().map(|giu| self.try_attach_detach_groups(queue, issi, giu))
                }
            };

            let group_identity_accept_reject = group_result
                .as_ref()
                .map(|result| if result.all_accepted { 0 } else { 1 })
                .unwrap_or(0);
            let group_identity_downlink = group_result.and_then(|result| {
                if result.group_identity_downlink.is_empty() {
                    None
                } else {
                    Some(result.group_identity_downlink)
                }
            });
            let gila = GroupIdentityLocationAccept {
                group_identity_accept_reject,
                group_identity_downlink,
            };

            Some(gila)
        } else if group_report_complete {
            // EN 300 392-2 clauses 16.4.3 and 16.10.27a: when the SwMI
            // requested a group report and the MS has no attached groups, it
            // answers with "group report complete". Treat that as a real empty
            // group report: clear stale affiliations and do not re-affiliate
            // from the local coverage-return cache.
            let prior_groups: Vec<u32> = self
                .client_mgr
                .get_client_by_issi(issi)
                .map(|client| client.groups.iter().copied().collect())
                .unwrap_or_default();
            if let Err(e) = self.client_mgr.client_detach_all_groups(issi) {
                tracing::warn!("Failed clearing groups for group-report-complete MS {}: {:?}", issi, e);
            } else if !prior_groups.is_empty() {
                {
                    let mut state = self.config.state_write();
                    for &gssi in &prior_groups {
                        state.subscribers.deaffiliate(issi, gssi);
                    }
                }
                self.emit_subscriber_update(queue, issi, prior_groups, BrewSubscriberAction::Deaffiliate);
            }
            None
        } else {
            None
        };
        if group_report_complete {
            // EN 300 392-2 clause 16.4.4 allows the MS to answer a SwMI
            // D-LOCATION-UPDATE-COMMAND group-report request by reporting group
            // identities in U-LOCATION UPDATE DEMAND. The local recovery window
            // is complete only when the standardized group-report-complete IE
            // is present; otherwise a final U-ATTACH/DETACH GROUP IDENTITY PDU
            // may follow with remaining or repeated group identities.
            self.clear_solicited_group_report(issi);
        }

        // Coverage-return re-affiliation (fixes "PTT no longer works after leaving and
        // returning to coverage", workaround = DMO→TMO).
        //
        // Sequence that breaks PTT:
        //   1. MS affiliates to a GSSI → CMCE group_listeners[gssi] += 1. PTT works.
        //   2. MS leaves coverage; local BS watchdog expires and emits Deregister to CMCE, which
        //      does dec_group_listener() → the GSSI now has 0 listeners.
        //   3. MS returns. Because we hand out attachment_lifetime=0 (persistent), the MS
        //      believes it is still affiliated and sends a plain location update WITHOUT a
        //      group identity report.
        //   4. MM re-registers the MS but never re-affiliates the groups → CMCE still has
        //      0 listeners for the GSSI → the next PTT is rejected with "no listeners"
        //      ("please wait" on the radio). DMO→TMO forces an ItsiAttach with a full group
        //      report, which is why that clears it.
        //
        // Fix: when a *known* MS re-registers without supplying a group report, but we
        // still hold groups for it in client_mgr, re-emit Affiliate for those groups so
        // CMCE's group_listeners (and Brew) are resynced with what the MS believes.
        if !soft_reattach_cmce_reset && !is_new && !_has_groups && !group_report_complete {
            let stored_groups: Vec<u32> = self
                .client_mgr
                .get_client_by_issi(issi)
                .map(|c| c.groups.iter().copied().collect())
                .unwrap_or_default();
            if !stored_groups.is_empty() {
                tracing::info!(
                    "MM: ISSI {} re-registered without group report but has {} stored group(s) {:?} — re-affiliating to resync CMCE/Brew (coverage-return fix)",
                    issi,
                    stored_groups.len(),
                    stored_groups
                );
                {
                    let mut state = self.config.state_write();
                    for &gssi in &stored_groups {
                        state.subscribers.affiliate(issi, gssi);
                    }
                }
                self.emit_subscriber_update(queue, issi, stored_groups, BrewSubscriberAction::Affiliate);
                self.emit_current_group_snapshot(issi);
            }
        }

        if is_new && !_has_groups && !group_report_complete && (was_solicited_group_report_pending || was_restart_recovery_candidate) {
            // EN 300 392-2 clause 16.8.0 keeps previously accepted group
            // identities valid while their lifetime remains valid. When a
            // restarted BS has just recovered the registration but the MS did
            // not include a fresh group report yet, restore only the locally
            // cached, previously accepted persistent groups for routing. A
            // later explicit empty or replacement group report remains
            // authoritative and clears/replaces these cached groups.
            cached_restart_group_refresh = self.restore_cached_restart_recovery_groups(queue, issi);
        }

        // Store and log class_of_ms
        if let Some(ref class) = pdu.class_of_ms {
            tracing::info!("MS {} class_of_ms: {}", issi, class);
        }
        // Per ETSI EN 300 392-2 clauses 16.10.46 and 16.10.8: if the MS
        // signals clch_needed=true or common_scch=true, include the 6-bit
        // SCCH information + distribution-on-18th-frame IE. This stack uses
        // MS SCCH allocation 0 and frame-18 distribution 00 = time slot 1,
        // matching the MCCH/control slot used here.
        let scch_info = pdu
            .class_of_ms
            .as_ref()
            .and_then(|c| if c.clch_needed || c.common_scch { Some(0x00u64) } else { None });

        let _ = self.client_mgr.set_client_class_of_ms(issi, pdu.class_of_ms);

        // Reset periodic registration timer on every successful registration.
        self.client_mgr.reset_registration_timer(issi);

        // EN 300 392-2 clause 16.10.35a: Location update accept type is a DL
        // IE with different names for raw values 1 and 5. Preserve the raw
        // registration type for the supported accepted requests.
        let Some(accept_type) = Self::location_update_accept_type_for(pdu.location_update_type) else {
            tracing::error!(
                "BUG: unsupported location update type reached ACCEPT path: {}",
                pdu.location_update_type
            );
            return;
        };

        // Build D-LOCATION UPDATE ACCEPT pdu
        let pdu_response = DLocationUpdateAccept {
            location_update_accept_type: accept_type,
            ssi: Some(issi as u64),
            address_extension: None,
            subscriber_class: None,
            energy_saving_information: esi_for_accept,
            scch_information_and_distribution_on_18th_frame: scch_info,
            new_registered_area: None,
            security_downlink: None,
            group_identity_location_accept: gila,
            default_group_attachment_lifetime: None,
            authentication_downlink: None,
            group_identity_security_related_information: None,
            cell_type_control: None,
            proprietary: None,
        };

        // Convert pdu to bits
        let pdu_len = 4 + 3 + 24 + 1 + 1 + 1; // Minimal lenght; may expand beyond this.
        let mut sdu = BitBuffer::new_autoexpand(pdu_len);
        pdu_response.to_bitbuf(&mut sdu).unwrap(); // we want to know when this happens
        sdu.seek(0);
        tracing::debug!("-> {} sdu {}", pdu_response, sdu.dump_bin());

        let response_handle = self.track_location_update_accept_downlink(issi, handle);

        // Build and submit response prim
        let msg = SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle: response_handle,
                address: TetraAddress::issi(issi),
                layer2service: Layer2Service::Acknowledged,
                stealing_permission: false,
                stealing_repeats_flag: false,
                encryption_flag: false,
                is_null_pdu: false,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);

        if !cached_restart_group_refresh.groups.is_empty() {
            // EN 300 392-2 clause 16.8.1 gives the SwMI a separate
            // infrastructure-initiated group attach path. Use it after the
            // location update accept so a restarted terminal that came back
            // group-less receives an over-air GSSI refresh instead of only a
            // local CMCE/dashboard affiliation replay.
            self.send_swmi_group_attach_refresh(
                queue,
                issi,
                &cached_restart_group_refresh.groups,
                cached_restart_group_refresh.remaining,
            );
        }

        // Send D-LOCATION-UPDATE-COMMAND to prompt a full re-registration (TEI +
        // group identity report) for a genuinely new radio that did not already
        // include a group report.
        //
        // This remains deliberately narrow:
        //  - A new radio doing RoamingLocationUpdating without groups gets exactly one
        //    COMMAND so it re-registers with its group list.
        //  - A restart-recovery candidate doing an unsolicited ITSI attach without
        //    groups also gets exactly one COMMAND. If a persistent GSSI was cached,
        //    local routing was restored above, but the MS report remains the final
        //    authority and can still clear/replace that cache.
        //  - A radio we ALREADY know never gets a COMMAND here. This is critical for
        //    receive-only devices like the Motorola TPG2200 pager, which never report any
        //    talkgroups: keying COMMAND at them on every update made them answer with yet
        //    another group-less RoamingLocationUpdating, producing an endless COMMAND loop
        //    and a permanent "Unit Not Attached" that even a kick couldn't clear (regression
        //    fixed here).
        //  - Motorola handsets (MTM800/MXP600) that answer a COMMAND with another
        //    RoamingLocationUpdating are now known on that second update, so they get no
        //    further COMMAND and can't loop.
        let has_groups = _has_groups || group_report_complete;
        let request_restart_group_report = is_new
            && !has_groups
            && cached_restart_group_refresh.groups.is_empty()
            && !was_solicited_group_report_pending
            && (pdu.location_update_type != LocationUpdateType::ItsiAttach || was_restart_recovery_candidate);

        self.remember_restart_recovery_issi(issi);

        if request_restart_group_report {
            tracing::info!("Sending D-LOCATION UPDATE COMMAND to returning MS {} to request group report", issi);
            self.send_d_location_update_command(queue, issi, handle);
        }

        if let Some(esi) = post_attach_energy_saving_request {
            tracing::info!(
                "MM: allocating energy saving mode {:?} to ISSI {} after registration",
                esi.energy_saving_mode,
                issi
            );
            Self::send_d_mm_status_energy_saving_request(queue, issi, prim.handle, esi);
        }
    }

    fn rx_u_mm_status(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_u_mm_status");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let pdu = match UMmStatus::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing UMmStatus: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        let issi = prim.received_address.ssi;
        let handle = prim.handle;
        if matches!(
            pdu.status_uplink,
            StatusUplink::ChangeOfEnergySavingModeRequest | StatusUplink::ChangeOfEnergySavingModeResponse
        ) && !self.client_mgr.client_is_known(issi)
        {
            tracing::warn!(
                "MM: ignoring {:?} from unknown ISSI {}; energy economy requires registered MM state",
                pdu.status_uplink,
                issi
            );
            return;
        }

        let mut handled = false;
        match pdu.status_uplink {
            StatusUplink::ChangeOfEnergySavingModeRequest => {
                let Some(requested_esm) = Self::parse_u_mm_status_energy_saving_mode(&pdu, issi) else {
                    return;
                };

                let allocated_esm = self.select_energy_saving_mode(Some(requested_esm));
                if requested_esm != allocated_esm {
                    tracing::info!(
                        "MS {} requested energy saving mode change to {:?}; allocating {:?}",
                        issi,
                        requested_esm,
                        allocated_esm
                    );
                } else {
                    tracing::info!("MS {} energy saving mode change request: {:?}", issi, requested_esm);
                }

                let (esi, start_time) = self.allocate_energy_saving_information(issi, allocated_esm);
                self.apply_energy_saving_information(issi, &esi, start_time);
                Self::send_d_mm_status_energy_saving(queue, issi, handle, esi);
                handled = true;
            }
            StatusUplink::ChangeOfEnergySavingModeResponse => {
                // MS confirming a BS-initiated change
                let Some(esm) = Self::parse_u_mm_status_energy_saving_mode(&pdu, issi) else {
                    return;
                };

                let pending = self.pending_energy_saving.remove(&issi);
                match pending {
                    Some(_pending) if esm == EnergySavingMode::StayAlive => {
                        // EN 300 392-2 clause 16.7.1 permits the MS response
                        // to a BS-initiated energy economy change to reject the
                        // requested EG by returning StayAlive. Honour that as a
                        // valid rejection instead of treating it as a generic
                        // mismatch that could restore an older EG assignment.
                        tracing::info!("MS {} rejected BS-initiated energy saving change with StayAlive", issi);
                        self.set_client_stay_alive(issi);
                    }
                    Some(pending) if pending.esi.energy_saving_mode == esm => {
                        tracing::info!("MS {} accepted energy saving mode {:?}", issi, esm);
                        if esm == EnergySavingMode::StayAlive {
                            self.set_client_stay_alive(issi);
                        } else {
                            self.apply_energy_saving_information(issi, &pending.esi, pending.start_time);
                        }
                    }
                    Some(pending) => {
                        tracing::warn!(
                            "MS {} responded with energy saving mode {:?} that does not match the BS-initiated pending assignment",
                            issi,
                            esm
                        );
                        self.fail_pending_energy_saving_assignment(issi, pending, "mismatched U-CHANGE response");
                    }
                    None => {
                        tracing::warn!(
                            "MS {} responded with energy saving mode {:?} without a matching BS-initiated pending assignment; ignoring stale response",
                            issi,
                            esm
                        );
                    }
                }
                handled = true;
            }
            StatusUplink::DualWatchModeRequest
            | StatusUplink::TerminatingDualWatchModeRequest
            | StatusUplink::ChangeOfDualWatchModeResponse
            | StatusUplink::StartOfDirectModeOperation
            | StatusUplink::MsFrequencyBandsInformation
            | StatusUplink::RequestToStartDmGatewayOperation
            | StatusUplink::RequestToContinuedmGatewayOperation
            | StatusUplink::RequestToStopDmGatewayOperation
            | StatusUplink::RequestToAddDmMsAddresses
            | StatusUplink::RequestToRemoveDmMsAddresses
            | StatusUplink::RequestToReplaceDmMsAddresses
            | StatusUplink::AcceptanceToRemovalOfDmMsAddresses
            | StatusUplink::AcceptanceToChangeRegistrationLabel
            | StatusUplink::AcceptanceToStopDmGatewayOperation => {
                unimplemented_log!("{:?}", pdu.status_uplink)
            }
            _ => {
                // Status types we don't handle (e.g. NetworkOrUserSpecific*, reserved
                // values). This is a valid-but-unsupported PDU, not a code bug, so log it
                // as unimplemented rather than asserting — assert_warn made it look like
                // an internal fault in the operator's logs. handled stays false, so we
                // still reply with "function not supported" below.
                unimplemented_log!("Unhandled UMmStatus type {:?}", pdu.status_uplink);
            }
        }

        if !handled {
            // A fairly untested, best-effort way of sending a PDU not supported error back
            // Note that an MS is not required to really do anything with this message.
            let (sapmsg, debug_str) = make_ul_mm_pdu_function_not_supported(
                handle,
                MmPduTypeUl::UMmStatus,
                Some((6, pdu.status_uplink.into())),
                prim.received_address,
            );
            tracing::debug!("-> {}", debug_str);
            queue.push_back(sapmsg);
        }
    }

    fn rx_u_attach_detach_group_identity(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_u_attach_detach_group_identity");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let issi = prim.received_address.ssi;

        let pdu = match UAttachDetachGroupIdentity::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing UAttachDetachGroupIdentity: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        self.abandon_pending_swmi_group_transaction(
            issi,
            "U-ATTACH/DETACH GROUP IDENTITY overrides pending SwMI group attach/detach procedure",
        );

        if pdu.group_identity_report {
            // EN 300 392-2 clause 16.8.4: for an MS-initiated group report
            // request accepted by the SwMI, the response is D-ATTACH/DETACH
            // GROUP IDENTITY. Keep unknown MSs on the explicit unsupported
            // path below so a report request cannot synthesize registration.
            if pdu.group_identity_attach_detach_mode || pdu.group_identity_uplink.is_some() || pdu.group_report_response.is_some() {
                // Clause 16.8.4 requires a report request to use amendment
                // mode and to omit group identity uplink elements. A report
                // response belongs to the answering side of a report procedure,
                // so accepting a mixed PDU here would wrongly advertise valid
                // groups for an inconsistent request.
                tracing::warn!(
                    "Rejecting malformed group report request from ISSI {}: mode={} uplink_present={} report_response_present={}",
                    issi,
                    pdu.group_identity_attach_detach_mode,
                    pdu.group_identity_uplink.is_some(),
                    pdu.group_report_response.is_some()
                );
                let (sapmsg, debug_str) = make_ul_mm_pdu_function_not_supported(
                    prim.handle,
                    MmPduTypeUl::UAttachDetachGroupIdentity,
                    None,
                    prim.received_address,
                );
                tracing::debug!("-> {}", debug_str);
                queue.push_back(sapmsg);
                return;
            }
            if let Some(client) = self.client_mgr.get_client_by_issi(issi) {
                let mut groups: Vec<u32> = client.groups.iter().copied().collect();
                groups.sort_unstable();
                self.restore_shared_subscriber_state_for_reported_groups(queue, issi, &groups);
                self.send_d_attach_detach_group_report_response(queue, issi, prim.handle, &groups);
            } else {
                let (sapmsg, debug_str) = make_ul_mm_pdu_function_not_supported(
                    prim.handle,
                    MmPduTypeUl::UAttachDetachGroupIdentity,
                    None,
                    prim.received_address,
                );
                tracing::debug!("-> {}", debug_str);
                queue.push_back(sapmsg);
            }
            return;
        }

        if !self.client_mgr.client_is_known(issi) {
            // EN 300 392-2 clause 16.4.3 treats group attachment as an MM
            // procedure for an attached MS; initial registration belongs to the
            // location update path in clauses 16.9.3.4 and 16.9.2.7/9. Do not
            // synthesize or imply subscriber registration from standalone group
            // attach because that bypasses LU accept/reject semantics.
            tracing::warn!("Rejecting standalone group attach from unknown MS {}", issi);
            let rejected_groups = pdu
                .group_identity_uplink
                .as_ref()
                .map(|giu| {
                    let giu_for_mode = Self::group_identity_uplink_for_mode(pdu.group_identity_attach_detach_mode, giu);
                    Self::rejected_group_identity_downlinks(&giu_for_mode)
                })
                .unwrap_or_default();
            self.send_d_attach_detach_ack_reject_with_downlink(queue, issi, prim.handle, rejected_groups);
            return;
        }

        if pdu.group_identity_uplink.is_none() {
            if let Some(response) = &pdu.group_report_response {
                if response.len == 1 && response.data == 0 {
                    // EN 300 392-2 clauses 16.4.3 and 16.10.27a define group
                    // report complete as the MS reporting no attached groups.
                    // Treat it as an empty report, not as a no-op ACK of stale
                    // local affiliations.
                    let prior_groups: Vec<u32> = self
                        .client_mgr
                        .get_client_by_issi(issi)
                        .map(|client| client.groups.iter().copied().collect())
                        .unwrap_or_default();
                    match self.client_mgr.client_detach_all_groups(issi) {
                        Ok(_) => {
                            if !prior_groups.is_empty() {
                                {
                                    let mut state = self.config.state_write();
                                    for &gssi in &prior_groups {
                                        state.subscribers.deaffiliate(issi, gssi);
                                    }
                                }
                                self.emit_subscriber_update(queue, issi, prior_groups, BrewSubscriberAction::Deaffiliate);
                            }
                            self.send_d_attach_detach_ack(queue, issi, prim.handle, &[]);
                            self.clear_solicited_group_report(issi);
                            self.remember_restart_recovery_issi(issi);
                        }
                        Err(e) => {
                            tracing::warn!("Failed clearing groups for standalone group-report-complete MS {}: {:?}", issi, e);
                            self.send_d_attach_detach_ack_reject(queue, issi, prim.handle);
                        }
                    }
                } else {
                    tracing::warn!(
                        "UAttachDetachGroupIdentity from {} has unsupported group_report_response len={} data={}",
                        issi,
                        response.len,
                        response.data
                    );
                    self.send_d_attach_detach_ack_reject(queue, issi, prim.handle);
                }
                return;
            }
        }

        if pdu.group_identity_attach_detach_mode && pdu.group_identity_uplink.is_none() {
            // EN 300 392-2 clause 16.8.2 and Annex G requirement 9 permit a
            // mode=1 attach/detach request with no groups to detach all active
            // group identities. ACK with no downlink group list once local state
            // and subscriber affiliation state have both been cleared.
            let prior_groups: Vec<u32> = self
                .client_mgr
                .get_client_by_issi(issi)
                .map(|client| client.groups.iter().copied().collect())
                .unwrap_or_default();
            match self.client_mgr.client_detach_all_groups(issi) {
                Ok(_) => {
                    if !prior_groups.is_empty() {
                        {
                            let mut state = self.config.state_write();
                            for &gssi in &prior_groups {
                                state.subscribers.deaffiliate(issi, gssi);
                            }
                        }
                        self.emit_subscriber_update(queue, issi, prior_groups, BrewSubscriberAction::Deaffiliate);
                    }
                    self.send_d_attach_detach_ack(queue, issi, prim.handle, &[]);
                    self.remember_restart_recovery_issi(issi);
                }
                Err(e) => {
                    tracing::warn!("Failed detaching all groups for MS {}: {:?}", issi, e);
                    self.send_d_attach_detach_ack_reject(queue, issi, prim.handle);
                }
            }
            return;
        }

        if !pdu.group_identity_attach_detach_mode && pdu.group_identity_uplink.is_none() {
            // Annex G requirement 9 reserves "no group identities" as detach-all
            // only when mode=1, except for solicited group-report responses
            // handled above. A bare mode=0 PDU carries no requested identities,
            // so acknowledge the no-op without echoing the current local group
            // set as if those groups had appeared in this transaction.
            tracing::info!(
                "UAttachDetachGroupIdentity from {} has mode=0 and no uplink groups; ACKing no-op without current group echo",
                issi
            );
            self.send_d_attach_detach_ack(queue, issi, prim.handle, &[]);
            return;
        }

        if let Some(response) = &pdu.group_report_response {
            // ETSI EN 300 392-2 clause 16.8.3/16.8.4 and 16.10.27a allow the
            // final group-report PDU to carry both the reported groups and the
            // "group report complete" IE in the solicited group-report window.
            // Some terminals split restart recovery as U-LOCATION UPDATE DEMAND
            // with groups followed by U-ATTACH/DETACH GROUP IDENTITY complete.
            let report_complete = response.len == 1 && response.data == 0;
            let complete_report_with_groups =
                report_complete && pdu.group_identity_uplink.is_some() && self.solicited_group_report_pending(issi);

            if complete_report_with_groups {
                let Some(giu) = pdu.group_identity_uplink.as_ref() else {
                    unreachable!("complete_report_with_groups requires group_identity_uplink");
                };
                tracing::info!(
                    "MM: accepting U-ATTACH/DETACH GROUP IDENTITY group-report completion from ISSI {} with {} group identity item(s), solicited={}, mode_detach_all_then_attach={}",
                    issi,
                    giu.len(),
                    self.solicited_group_report_pending(issi),
                    pdu.group_identity_attach_detach_mode
                );
                if self.process_attach_detach_group_identity_uplink(queue, issi, prim.handle, pdu.group_identity_attach_detach_mode, giu) {
                    self.clear_solicited_group_report(issi);
                }
                return;
            }

            if self.solicited_group_report_pending(issi) {
                tracing::warn!(
                    "Rejecting solicited U-ATTACH/DETACH GROUP IDENTITY from {} with reserved/unsupported group_report_response len={} data={}",
                    issi,
                    response.len,
                    response.data
                );
                let rejected_groups = pdu
                    .group_identity_uplink
                    .as_ref()
                    .map(|giu| Self::rejected_group_identity_downlinks(giu))
                    .unwrap_or_default();
                self.send_d_attach_detach_ack_reject_with_downlink(queue, issi, prim.handle, rejected_groups);
                return;
            }

            // EN 300 392-2 clause 16.8.2: an MS-initiated attach/detach group
            // identity request shall set "not report request" and shall not
            // include group report response. Treat a mixed report response plus
            // explicit requested identities as a consistency failure before any
            // detach-all or affiliation mutation.
            tracing::warn!(
                "Rejecting mixed U-ATTACH/DETACH GROUP IDENTITY from {}: group_report_response len={} data={} present with group identities",
                issi,
                response.len,
                response.data
            );
            let rejected_groups = pdu
                .group_identity_uplink
                .as_ref()
                .map(|giu| Self::rejected_group_identity_downlinks(giu))
                .unwrap_or_default();
            self.send_d_attach_detach_ack_reject_with_downlink(queue, issi, prim.handle, rejected_groups);
            return;
        }

        // Check if we can satisfy this request, print unsupported stuff
        if !Self::feature_check_u_attach_detach_group_identity(&pdu) {
            // group_identity_uplink missing — terminal is sending a group report response
            // without requesting any group changes. Send ACK with current groups so
            // terminal knows it's affiliated and can use PTT.
            tracing::info!(
                "UAttachDetachGroupIdentity from {} has no uplink groups — sending ACK with current groups",
                issi
            );
            let current_groups: Vec<u32> = self
                .client_mgr
                .get_client_by_issi(issi)
                .map(|c| c.groups.iter().copied().collect())
                .unwrap_or_default();
            self.send_d_attach_detach_ack(queue, issi, prim.handle, &current_groups);
            return;
        }

        // ETSI EN 300 392-2 Annex G requires reject ACKs to represent rejected
        // identities coherently. This local BS keeps group ACKs within one
        // unsegmented MM TM-SDU; when a request exceeds that capacity, reject it
        // before any mode=1 detach-all or affiliation mutation.
        // feature_check_u_attach_detach_group_identity above guarantees this is Some,
        // but use let-else instead of .unwrap() so a future refactor that loosens that
        // check doesn't crash the MM worker on a malformed PDU.
        let Some(giu) = pdu.group_identity_uplink.as_ref() else {
            tracing::warn!("rx_u_attach_detach_group_identity: group_identity_uplink missing after feature_check; ignoring");
            return;
        };
        self.process_attach_detach_group_identity_uplink(queue, issi, prim.handle, pdu.group_identity_attach_detach_mode, giu);
    }

    fn rx_u_attach_detach_group_identity_acknowledgement(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_u_attach_detach_group_identity_acknowledgement");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let issi = prim.received_address.ssi;
        let pdu = match UAttachDetachGroupIdentityAcknowledgement::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!(
                    "Failed parsing UAttachDetachGroupIdentityAcknowledgement: {:?} {}",
                    e,
                    prim.sdu.dump_bin()
                );
                return;
            }
        };

        if pdu.proprietary.is_some() {
            unimplemented_log!("UAttachDetachGroupIdentityAcknowledgement proprietary IE ignored");
        }

        let Some(pending) = self.pending_swmi_group_transactions.get(&issi) else {
            tracing::debug!(
                "MM: ignoring unmatched U-ATTACH/DETACH GROUP IDENTITY ACKNOWLEDGEMENT from ISSI {} handle={}",
                issi,
                prim.handle
            );
            return;
        };

        if pending.handle != prim.handle && !pending.accepts_unrouted_ack_handle {
            tracing::debug!(
                "MM: U-ATTACH/DETACH GROUP IDENTITY ACK handle mismatch for ISSI {}: pending={} received={}; ignoring",
                issi,
                pending.handle,
                prim.handle
            );
            return;
        }

        // EN 300 392-2 Annex G requirement 7 and 16.10.14: reject means at
        // least one attachment was rejected, and the rejected identities shall
        // be present in the ACK. Annex G requirement 8b marks a rejected
        // attachment as a detachment entry; attachment entries may merely
        // identify accepted groups with changed CoU/lifetime.
        if pdu.group_identity_acknowledgement_type && !pdu.group_identity_uplink.as_ref().is_some_and(|groups| !groups.is_empty()) {
            tracing::warn!(
                "MM: malformed U-ATTACH/DETACH GROUP IDENTITY ACK from ISSI {} rejected at handle={}: reject bit set without rejected group identities",
                issi,
                prim.handle
            );
            return;
        }
        if pdu.group_identity_acknowledgement_type
            && pending
                .group_identity_downlink
                .iter()
                .any(|gid| gid.group_identity_attachment.is_some())
            && !Self::reject_ack_has_rejected_pending_attachment(pending, &pdu)
        {
            tracing::warn!(
                "MM: malformed U-ATTACH/DETACH GROUP IDENTITY ACK from ISSI {} rejected at handle={}: reject bit set without any rejected attachment entries",
                issi,
                prim.handle
            );
            return;
        }

        // EN 300 392-2 clauses 16.8.4 and 16.10.16: this uplink ACK is the
        // response to a SwMI D-ATTACH/DETACH GROUP IDENTITY and expects no
        // downlink response. Omitted ACK identities are accepted; explicit
        // rejected attachments are listed in GroupIdentityUplink.
        let pending = self
            .pending_swmi_group_transactions
            .remove(&issi)
            .expect("pending SwMI group transaction was validated above");
        self.apply_swmi_group_ack(queue, issi, pending, &pdu);
    }

    fn rx_lmm_mle_unitdata_ind(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        // unimplemented_log!("rx_lmm_mle_unitdata_ind for MM component");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let Some(bits) = prim.sdu.peek_bits(4) else {
            tracing::warn!("insufficient bits: {}", prim.sdu.dump_bin());
            return;
        };

        let Ok(pdu_type) = MmPduTypeUl::try_from(bits) else {
            tracing::warn!("invalid pdu type: {} in {}", bits, prim.sdu.dump_bin());
            return;
        };

        if prim.received_address.ssi_type != SsiType::Issi || prim.received_address.ssi > Self::MAX_AIR_INTERFACE_SSI {
            // EN 300 392-2 clauses 16.4.3, 16.8.2, 16.8.4 and 16.9.3.4 are
            // MS/ITSI mobility procedures. Clause 16.8.8 only permits an
            // unsupported-function response for an individually addressed MM
            // PDU, so non-ISSI RF sources are dropped before any registration,
            // group or energy-economy state mutation.
            tracing::warn!(
                "MM: dropping {:?} from non-individual RF source {}",
                pdu_type,
                prim.received_address
            );
            return;
        }

        match pdu_type {
            MmPduTypeUl::UAuthentication => unimplemented_log!("UAuthentication"),
            MmPduTypeUl::UItsiDetach => self.rx_u_itsi_detach(queue, message),
            MmPduTypeUl::ULocationUpdateDemand => self.rx_u_location_update_demand(queue, message),
            MmPduTypeUl::UMmStatus => self.rx_u_mm_status(queue, message),
            MmPduTypeUl::UCkChangeResult => unimplemented_log!("UCkChangeResult"),
            MmPduTypeUl::UOtar => unimplemented_log!("UOtar"),
            MmPduTypeUl::UInformationProvide => {
                // EN 300 392-2 clause 16.8.8 permits an explicit
                // MM PDU/FUNCTION NOT SUPPORTED response for an individually
                // addressed MM PDU that this SwMI recognizes but does not
                // support.
                let (sapmsg, debug_str) = make_ul_mm_pdu_function_not_supported(prim.handle, pdu_type, None, prim.received_address);
                tracing::debug!("-> {}", debug_str);
                queue.push_back(sapmsg);
            }
            MmPduTypeUl::UAttachDetachGroupIdentity => self.rx_u_attach_detach_group_identity(queue, message),
            MmPduTypeUl::UAttachDetachGroupIdentityAcknowledgement => {
                self.rx_u_attach_detach_group_identity_acknowledgement(queue, message)
            }
            MmPduTypeUl::UTeiProvide => self.rx_u_tei_provide(queue, message),
            MmPduTypeUl::UDisableStatus => {
                // Keep security-specific MM PDUs out of scope here; this is a
                // non-security unsupported whole-PDU response.
                let (sapmsg, debug_str) = make_ul_mm_pdu_function_not_supported(prim.handle, pdu_type, None, prim.received_address);
                tracing::debug!("-> {}", debug_str);
                queue.push_back(sapmsg);
            }
            MmPduTypeUl::MmPduFunctionNotSupported => unimplemented_log!("MmPduFunctionNotSupported"),
        };
    }

    fn try_attach_detach_groups(
        &mut self,
        queue: &mut MessageQueue,
        issi: u32,
        giu_vec: &[GroupIdentityUplink],
    ) -> GroupIdentityProcessResult {
        let mut group_identity_downlink = Vec::new();
        let mut all_accepted = true;
        let mut aff_groups = Vec::new();
        let mut deaff_groups = Vec::new();

        for giu in giu_vec.iter() {
            // Currently only address_type=0 (plain GSSI) is implemented. Anything else
            // (vgssi, address extension, missing gssi) is unsupported — reject.
            let Some(gssi) = giu.gssi else {
                unimplemented_log!("GroupIdentityUplink without gssi field");
                all_accepted = false;
                if let Some(rejected) = Self::rejected_group_identity_downlink(giu) {
                    group_identity_downlink.push(rejected);
                }
                continue;
            };
            if giu.vgssi.is_some() || giu.address_extension.is_some() {
                unimplemented_log!("Only support GroupIdentityUplink with address_type 0");
                all_accepted = false;
                if let Some(rejected) = Self::rejected_group_identity_downlink(giu) {
                    group_identity_downlink.push(rejected);
                }
                continue;
            }

            let is_detach = giu.group_identity_detachment_uplink.is_some();
            if !is_detach
                && self
                    .config
                    .config()
                    .cell
                    .allowed_gssi_ranges
                    .as_ref()
                    .is_some_and(|ranges| !ranges.contains(gssi))
            {
                // EN 300 392-2 clause 16.10.20/table 16.52 defines reject
                // reason 0 as unknown group identity. Keep the default policy
                // open for dynamic scan lists, but honour an explicit local
                // provisioning range when configured.
                tracing::warn!("Rejecting group attach from ISSI {} to unprovisioned GSSI {}", issi, gssi);
                all_accepted = false;
                if let Some(rejected) = Self::rejected_group_identity_downlink(giu) {
                    group_identity_downlink.push(rejected);
                }
                continue;
            }

            if is_detach {
                let shared_affiliated_before = self.config.state_read().subscribers.contains_group_member(gssi, issi);
                match self.client_mgr.client_group_attach(issi, gssi, false) {
                    Ok(changed) => {
                        if changed {
                            self.restore_shared_subscriber_registration_for_known_client(queue, issi);
                            if shared_affiliated_before && self.config.state_write().subscribers.deaffiliate(issi, gssi) {
                                deaff_groups.push(gssi);
                            }
                        }
                        // EN 300 392-2 Annex G requirement 6d/8d: accepted
                        // detachment is implicit when accept/reject=0. Do not
                        // echo the detached group, because explicit detachment
                        // in the ACK gives no useful information and may be
                        // interpreted ambiguously by the requester.
                    }
                    Err(ClientMgrErr::ClientNotFound { .. }) => {
                        tracing::debug!("Group detach for ISSI {} gssi={} skipped: client no longer registered", issi, gssi);
                        all_accepted = false;
                        if let Some(rejected) = Self::rejected_group_identity_downlink(giu) {
                            group_identity_downlink.push(rejected);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed detaching MS {} from group {}: {:?}", issi, gssi, e);
                        all_accepted = false;
                        if let Some(rejected) = Self::rejected_group_identity_downlink(giu) {
                            group_identity_downlink.push(rejected);
                        }
                    }
                }
            } else {
                let shared_affiliated_before = self.config.state_read().subscribers.contains_group_member(gssi, issi);
                let attachment_info = GroupAttachmentInfo {
                    group_identity_attachment_lifetime: 0,
                    class_of_usage: giu.class_of_usage.unwrap_or(0),
                };
                match self.client_mgr.client_group_attach_with_info(issi, gssi, true, attachment_info) {
                    Ok(changed) => {
                        if changed || !shared_affiliated_before {
                            self.restore_shared_subscriber_registration_for_known_client(queue, issi);
                        }
                        if !shared_affiliated_before && self.config.state_write().subscribers.affiliate(issi, gssi) {
                            aff_groups.push(gssi);
                        }
                        // We have added the client to this group. Add an entry to the downlink response.
                        //
                        // group_identity_attachment_lifetime values (ETSI EN 300 392-2 §16.10.19):
                        //   0 = Attachment not needed → MS keeps the group attached indefinitely
                        //                                until an explicit detach. This is what we want
                        //                                for scan lists / persistent group attachments.
                        //   1 = Attachment required for the next ITSI attach → MS re-affiliates on next
                        //                                ITSI attach (rare event: reboot, cell reselect).
                        //   2 = Attachment not allowed for next ITSI attach → SwMI denies.
                        //   3 = Attachment required for next location update → MS re-affiliates at every
                        //                                LU (every few minutes), generating churn.
                        //
                        // We previously used 1 with a "good default" comment, but that interacted badly
                        // with Motorola MTP-series radios in scan-list mode: those radios send the scan
                        // list incrementally (2 GSSIs at a time, with one anchor + one new GSSI), and
                        // expect the BS-side affiliation to persist between batches. With lifetime=1 the
                        // MS internally drops the affiliation a few minutes later ("5-minute timer" per
                        // dk5ras), then PTT fails with "Unit not attached" until the user changes GSSI.
                        // Lifetime=0 makes the attachment persistent on the MS side — matching the BS
                        // side which already keeps affiliations across attach cycles — and resolves
                        // FH-BUG-022.
                        let gid = GroupIdentityDownlink {
                            group_identity_attachment: Some(GroupIdentityAttachment {
                                group_identity_attachment_lifetime: attachment_info.group_identity_attachment_lifetime,
                                class_of_usage: attachment_info.class_of_usage,
                            }),
                            group_identity_detachment_uplink: None,
                            gssi: Some(gssi),
                            address_extension: None,
                            vgssi: None,
                        };
                        group_identity_downlink.push(gid);
                    }
                    Err(ClientMgrErr::ClientNotFound { .. }) => {
                        // Terminal was removed after local watchdog grace expiry while PDU was in flight — ignore.
                        tracing::debug!("Group attach for ISSI {} gssi={} skipped: client no longer registered", issi, gssi);
                        all_accepted = false;
                        if let Some(rejected) = Self::rejected_group_identity_downlink(giu) {
                            group_identity_downlink.push(rejected);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed attaching MS {} to group {}: {:?}", issi, gssi, e);
                        all_accepted = false;
                        if let Some(rejected) = Self::rejected_group_identity_downlink(giu) {
                            group_identity_downlink.push(rejected);
                        }
                    }
                }
            }
        }

        if !aff_groups.is_empty() {
            self.emit_subscriber_update(queue, issi, aff_groups, BrewSubscriberAction::Affiliate);
        }
        if !deaff_groups.is_empty() {
            self.emit_subscriber_update(queue, issi, deaff_groups, BrewSubscriberAction::Deaffiliate);
        }

        // Emit a single snapshot of all current groups so the dashboard always
        // has the final ETSI mode=1 replace result, not an intermediate empty
        // list from the local detach-all step.
        self.emit_current_group_snapshot(issi);
        self.remember_restart_recovery_issi(issi);

        GroupIdentityProcessResult {
            group_identity_downlink,
            all_accepted,
        }
    }

    fn send_d_attach_detach_ack_for_group_result(
        &self,
        queue: &mut MessageQueue,
        issi: u32,
        handle: u32,
        group_result: GroupIdentityProcessResult,
    ) {
        let group_identity_accept_reject = if group_result.all_accepted { 0 } else { 1 };
        let group_identity_downlink = if group_result.group_identity_downlink.is_empty() {
            None
        } else {
            Some(group_result.group_identity_downlink)
        };

        let pdu_response = DAttachDetachGroupIdentityAcknowledgement {
            group_identity_accept_reject,
            // EN 300 392-2 clause 16.9.2.2 table 16.2 defines this as a
            // mandatory one-bit Reserved field. Keep the reserved bit clear.
            reserved: false,
            proprietary: None,
            group_identity_downlink,
            group_identity_security_related_information: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(32);
        pdu_response.to_bitbuf(&mut sdu).unwrap();
        sdu.seek(0);
        tracing::debug!("-> {:?} sdu {}", pdu_response, sdu.dump_bin());

        queue.push_back(SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle,
                address: TetraAddress::issi(issi),
                layer2service: Layer2Service::Acknowledged,
                stealing_permission: false,
                stealing_repeats_flag: false,
                encryption_flag: false,
                is_null_pdu: false,
                tx_reporter: None,
            }),
        });
    }

    fn send_swmi_group_attach_refresh(
        &mut self,
        queue: &mut MessageQueue,
        issi: u32,
        groups: &[(u32, GroupAttachmentInfo)],
        remaining_restart_group_refresh: Vec<(u32, GroupAttachmentInfo)>,
    ) {
        if groups.is_empty() {
            return;
        }

        let groups_to_send = if groups.len() > Self::MAX_GROUPS_PER_ATTACH {
            tracing::warn!(
                "MM: cached restart group refresh for ISSI {} has {} groups; limiting one D-ATTACH/DETACH GROUP IDENTITY PDU to {} groups",
                issi,
                groups.len(),
                Self::MAX_GROUPS_PER_ATTACH
            );
            &groups[..Self::MAX_GROUPS_PER_ATTACH]
        } else {
            groups
        };

        let group_identity_downlink = Self::group_identity_downlink_for_attachment_infos(groups_to_send);
        if group_identity_downlink.is_empty() {
            return;
        }
        let handle = self.allocate_mm_downlink_handle();

        let pdu = DAttachDetachGroupIdentity {
            group_identity_report: false,
            group_identity_acknowledgement_request: true,
            // EN 300 392-2 clause 16.8.1/table 16.49: use amendment for a
            // restart refresh so cached groups are attached without detaching
            // any additional scan-list groups the MS may still hold. A later
            // explicit group report or empty report remains authoritative and
            // abandons this pending SwMI transaction before mutating state.
            group_identity_attach_detach_mode: false,
            proprietary: None,
            group_report_response: None,
            group_identity_downlink: Some(group_identity_downlink.clone()),
            group_identity_security_related_information: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(64);
        match pdu.to_bitbuf(&mut sdu) {
            Ok(()) => {
                sdu.seek(0);
                tracing::info!(
                    "MM: sending SwMI group attach refresh to ISSI {} for cached restart group(s) {:?}",
                    issi,
                    groups_to_send.iter().map(|(gssi, _)| *gssi).collect::<Vec<u32>>()
                );
                self.pending_swmi_group_transactions.insert(
                    issi,
                    PendingSwmiGroupTransaction {
                        handle,
                        expires_at: self.dltime.add_timeslots(Self::T353_GROUP_RESPONSE_TIMESLOTS),
                        group_identity_downlink,
                        detach_all_then_attach: false,
                        // LLC/MLE uplink indications generated for a standalone
                        // MS ACK may not preserve the downlink request handle.
                        // EN 300 392-2 clause 16.8.1 keys the over-air ACK by
                        // the same ISSI/procedure, not by this local handle.
                        accepts_unrouted_ack_handle: true,
                        rollback_unconfirmed_attachments_on_failure: true,
                        reprobe_group_report_on_failure: true,
                        remaining_restart_group_refresh,
                    },
                );
                queue.push_back(SapMsg {
                    sap: Sap::LmmSap,
                    src: TetraEntity::Mm,
                    dest: TetraEntity::Mle,
                    msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                        sdu,
                        handle,
                        address: TetraAddress::issi(issi),
                        layer2service: Layer2Service::Acknowledged,
                        stealing_permission: false,
                        stealing_repeats_flag: false,
                        encryption_flag: false,
                        is_null_pdu: false,
                        tx_reporter: None,
                    }),
                });
            }
            Err(err) => {
                tracing::warn!(
                    "MM: failed serializing SwMI group attach refresh for ISSI {} groups={:?}: {:?}",
                    issi,
                    groups_to_send.iter().map(|(gssi, _)| *gssi).collect::<Vec<u32>>(),
                    err
                );
            }
        }
    }

    fn process_attach_detach_group_identity_uplink(
        &mut self,
        queue: &mut MessageQueue,
        issi: u32,
        handle: u32,
        detach_all_then_attach: bool,
        giu: &[GroupIdentityUplink],
    ) -> bool {
        let giu_for_mode = Self::group_identity_uplink_for_mode(detach_all_then_attach, giu);
        let group_result = if self.group_identity_uplink_exceeds_ack_capacity(issi, giu_for_mode.len()) {
            GroupIdentityProcessResult {
                group_identity_downlink: Self::rejected_group_identity_downlinks(&giu_for_mode),
                all_accepted: false,
            }
        } else {
            if detach_all_then_attach && !self.prepare_detach_all_then_attach(queue, issi, &giu_for_mode) {
                return false;
            }

            self.try_attach_detach_groups(queue, issi, &giu_for_mode)
        };

        self.send_d_attach_detach_ack_for_group_result(queue, issi, handle, group_result);
        true
    }

    fn rejected_group_identity_downlink(giu: &GroupIdentityUplink) -> Option<GroupIdentityDownlink> {
        if giu.gssi.is_none() && giu.vgssi.is_none() {
            return None;
        }

        // EN 300 392-2 16.10.12 and Annex G requirements 7/8 require an
        // explicit per-identity rejection. Attachment rejection is encoded with
        // GIADTI=1 and a group identity detachment downlink reason; rejected
        // detachment is encoded with GIADTI=0 as an attachment entry.
        let (group_identity_attachment, group_identity_detachment_uplink) = if giu.group_identity_detachment_uplink.is_some() {
            (
                Some(GroupIdentityAttachment {
                    group_identity_attachment_lifetime: 0,
                    class_of_usage: giu.class_of_usage.unwrap_or(0),
                }),
                None,
            )
        } else {
            // EN 300 392-2 16.10.20 table 16.52: 0 = unknown group identity.
            (None, Some(0))
        };

        Some(GroupIdentityDownlink {
            group_identity_attachment,
            group_identity_detachment_uplink,
            gssi: giu.gssi,
            address_extension: giu.address_extension,
            vgssi: giu.vgssi,
        })
    }

    fn rejected_group_identity_downlinks(giu: &[GroupIdentityUplink]) -> Vec<GroupIdentityDownlink> {
        giu.iter().filter_map(Self::rejected_group_identity_downlink).collect()
    }

    fn group_identity_uplink_exceeds_ack_capacity(&self, issi: u32, requested_count: usize) -> bool {
        if requested_count > Self::MAX_GROUPS_PER_ATTACH {
            tracing::warn!(
                "ISSI {} requested attach/detach for {} groups; local accept capacity is {}. Explicitly rejecting without partial affiliation so Annex G accept/reject state stays transactional.",
                issi,
                requested_count,
                Self::MAX_GROUPS_PER_ATTACH
            );
            return true;
        }
        false
    }

    fn group_identity_uplink_for_mode(detach_all_then_attach: bool, giu: &[GroupIdentityUplink]) -> Vec<GroupIdentityUplink> {
        if !detach_all_then_attach {
            return giu.to_vec();
        }

        let filtered: Vec<GroupIdentityUplink> = giu
            .iter()
            .filter(|entry| entry.group_identity_detachment_uplink.is_none())
            .cloned()
            .collect();
        let ignored = giu.len().saturating_sub(filtered.len());
        if ignored > 0 {
            tracing::debug!(
                "MM: ignoring {} explicit group detachment entries because group_identity_attach_detach_mode=1 already detaches all groups",
                ignored
            );
        }
        filtered
    }

    fn send_d_attach_detach_ack_reject(&self, queue: &mut MessageQueue, issi: u32, handle: u32) {
        self.send_d_attach_detach_ack_reject_with_downlink(queue, issi, handle, Vec::new());
    }

    /// Send D-ATTACH-DETACH-GROUP-IDENTITY-ACKNOWLEDGEMENT with aggregate
    /// reject. EN 300 392-2 clause 16.10.12 marks at least one rejected
    /// attachment/detachment; Annex G requirement 7 requires rejected identities
    /// to be listed when the request contained explicit groups.
    fn send_d_attach_detach_ack_reject_with_downlink(
        &self,
        queue: &mut MessageQueue,
        issi: u32,
        handle: u32,
        group_identity_downlink: Vec<GroupIdentityDownlink>,
    ) {
        let pdu = DAttachDetachGroupIdentityAcknowledgement {
            group_identity_accept_reject: 1, // 1 = reject per ETSI §14.8.7
            reserved: false,
            proprietary: None,
            group_identity_downlink: if group_identity_downlink.is_empty() {
                None
            } else {
                Some(group_identity_downlink)
            },
            group_identity_security_related_information: None,
        };
        let mut sdu = BitBuffer::new_autoexpand(16);
        pdu.to_bitbuf(&mut sdu).unwrap();
        sdu.seek(0);
        tracing::debug!("-> DAttachDetachGroupIdentityAcknowledgement (reject) to ISSI {}", issi);
        let msg = SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle,
                address: TetraAddress::issi(issi),
                layer2service: Layer2Service::Acknowledged,
                stealing_permission: false,
                stealing_repeats_flag: false,
                encryption_flag: false,
                is_null_pdu: false,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    fn group_attachment_info_for_report(&self, issi: u32, gssi: u32) -> GroupAttachmentInfo {
        self.client_mgr.client_group_attachment_info(issi, gssi).unwrap_or_default()
    }

    fn group_identity_downlink_for_attachment_infos(groups: &[(u32, GroupAttachmentInfo)]) -> Vec<GroupIdentityDownlink> {
        groups
            .iter()
            .map(|&(gssi, attachment_info)| GroupIdentityDownlink {
                group_identity_attachment: Some(GroupIdentityAttachment {
                    // EN 300 392-2 clauses 16.8.4 and 16.10.19: preserve the
                    // lifetime and class of usage recorded when the group was
                    // attached or recovered.
                    group_identity_attachment_lifetime: attachment_info.group_identity_attachment_lifetime,
                    class_of_usage: attachment_info.class_of_usage,
                }),
                group_identity_detachment_uplink: None,
                gssi: Some(gssi),
                address_extension: None,
                vgssi: None,
            })
            .collect()
    }

    fn group_identity_downlink_for_reported_groups(&self, issi: u32, groups: &[u32]) -> Vec<GroupIdentityDownlink> {
        let groups_with_info: Vec<(u32, GroupAttachmentInfo)> = groups
            .iter()
            .map(|&gssi| (gssi, self.group_attachment_info_for_report(issi, gssi)))
            .collect();
        Self::group_identity_downlink_for_attachment_infos(&groups_with_info)
    }

    fn send_d_attach_detach_ack(&self, queue: &mut MessageQueue, issi: u32, handle: u32, groups: &[u32]) {
        let gid = self.group_identity_downlink_for_reported_groups(issi, groups);
        let ack = DAttachDetachGroupIdentityAcknowledgement {
            group_identity_accept_reject: 0,
            reserved: false,
            proprietary: None,
            group_identity_downlink: if gid.is_empty() { None } else { Some(gid) },
            group_identity_security_related_information: None,
        };
        let mut sdu = BitBuffer::new_autoexpand(32);
        if ack.to_bitbuf(&mut sdu).is_ok() {
            sdu.seek(0);
            tracing::debug!("-> DAttachDetachGroupIdentityAcknowledgement (ack-only) sdu {}", sdu.dump_bin());
            queue.push_back(SapMsg {
                sap: Sap::LmmSap,
                src: TetraEntity::Mm,
                dest: TetraEntity::Mle,
                msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                    sdu,
                    handle,
                    address: TetraAddress::issi(issi),
                    layer2service: Layer2Service::Acknowledged,
                    stealing_permission: false,
                    stealing_repeats_flag: false,
                    encryption_flag: false,
                    is_null_pdu: false,
                    tx_reporter: None,
                }),
            });
        }
    }

    fn send_d_attach_detach_group_report_response(&self, queue: &mut MessageQueue, issi: u32, handle: u32, groups: &[u32]) {
        if groups.is_empty() {
            self.send_d_attach_detach_group_report_response_segment(
                queue,
                issi,
                handle,
                &[],
                true,
                true,
                Layer2Service::AcknowledgedResponse,
            );
            return;
        }

        let chunk_count = groups.chunks(Self::MAX_GROUPS_PER_ATTACH).count();
        for (idx, chunk) in groups.chunks(Self::MAX_GROUPS_PER_ATTACH).enumerate() {
            // EN 300 392-2 clause 16.8.4: if reported groups do not fit into
            // one D-ATTACH/DETACH GROUP IDENTITY PDU, omit group-report-complete
            // until the last PDU. The first PDU uses mode=1 (detach all and
            // attach listed groups); subsequent PDUs use amendment mode.
            let first_segment = idx == 0;
            let last_segment = idx + 1 == chunk_count;
            let layer2service = if first_segment {
                Layer2Service::AcknowledgedResponse
            } else {
                Layer2Service::Acknowledged
            };
            self.send_d_attach_detach_group_report_response_segment(queue, issi, handle, chunk, first_segment, last_segment, layer2service);
        }
    }

    fn send_d_attach_detach_group_report_response_segment(
        &self,
        queue: &mut MessageQueue,
        issi: u32,
        handle: u32,
        groups: &[u32],
        detach_all_then_attach: bool,
        group_report_complete: bool,
        layer2service: Layer2Service,
    ) {
        let group_identity_downlink = self.group_identity_downlink_for_reported_groups(issi, groups);
        let pdu = DAttachDetachGroupIdentity {
            group_identity_report: false,
            // EN 300 392-2 clause 16.8.4 permits either value here. Use no
            // ACK request for these report PDUs to avoid creating another
            // pending SwMI transaction while still answering the MS request.
            group_identity_acknowledgement_request: false,
            group_identity_attach_detach_mode: detach_all_then_attach,
            proprietary: None,
            group_report_response: group_report_complete.then_some(Type3FieldGeneric {
                field_id: MmType34ElemIdDl::GroupReportResponse.into_raw(),
                len: 1,
                data: 0,
            }),
            group_identity_downlink: if group_identity_downlink.is_empty() {
                None
            } else {
                Some(group_identity_downlink)
            },
            group_identity_security_related_information: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(32);
        match pdu.to_bitbuf(&mut sdu) {
            Ok(()) => {
                sdu.seek(0);
                tracing::debug!("-> DAttachDetachGroupIdentity (group report response) sdu {}", sdu.dump_bin());
                queue.push_back(SapMsg {
                    sap: Sap::LmmSap,
                    src: TetraEntity::Mm,
                    dest: TetraEntity::Mle,
                    msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                        sdu,
                        handle,
                        address: TetraAddress::issi(issi),
                        layer2service,
                        stealing_permission: false,
                        stealing_repeats_flag: false,
                        encryption_flag: false,
                        is_null_pdu: false,
                        tx_reporter: None,
                    }),
                });
            }
            Err(e) => {
                tracing::warn!(
                    "Failed serializing DAttachDetachGroupIdentity group report response for ISSI {} groups={:?}: {:?}",
                    issi,
                    groups,
                    e
                );
            }
        }
    }

    fn send_d_location_update_command(&mut self, queue: &mut MessageQueue, issi: u32, handle: u32) {
        Self::enqueue_d_location_update_command(queue, issi, handle);
        self.arm_solicited_group_report(issi);
    }

    fn enqueue_d_location_update_command(queue: &mut MessageQueue, issi: u32, handle: u32) {
        let pdu = DLocationUpdateCommand {
            group_identity_report: true,
            cipher_control: false,
            ciphering_parameters: None,
            address_extension: None,
            cell_type_control: None,
            proprietary: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(16);
        pdu.to_bitbuf(&mut sdu).unwrap();
        sdu.seek(0);
        tracing::debug!("-> DLocationUpdateCommand sdu {}", sdu.dump_bin());

        let msg = SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle,
                address: TetraAddress::issi(issi),
                layer2service: Layer2Service::Acknowledged,
                stealing_permission: false,
                stealing_repeats_flag: false,
                encryption_flag: false,
                is_null_pdu: false,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    /// Sends a D-LOCATION UPDATE REJECT PDU (ETSI clause 16.9.2.9)
    fn send_d_location_update_reject_cause(
        queue: &mut MessageQueue,
        issi: u32,
        handle: u32,
        location_update_type: LocationUpdateType,
        address_extension: Option<u64>,
        reject_cause: RejectCause,
    ) {
        let pdu = DLocationUpdateReject {
            location_update_type,
            reject_cause: reject_cause as u8,
            cipher_control: false,
            ciphering_parameters: None,
            address_extension,
            cell_type_control: None,
            proprietary: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(16);
        pdu.to_bitbuf(&mut sdu).unwrap();
        sdu.seek(0);
        tracing::debug!("-> {} sdu {}", pdu, sdu.dump_bin());

        let msg = SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle,
                address: TetraAddress::issi(issi),
                layer2service: Layer2Service::Acknowledged,
                stealing_permission: false,
                stealing_repeats_flag: false,
                encryption_flag: false,
                is_null_pdu: false,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    fn location_update_accept_type_for(location_update_type: LocationUpdateType) -> Option<LocationUpdateAcceptType> {
        match location_update_type {
            LocationUpdateType::RoamingLocationUpdating => Some(LocationUpdateAcceptType::RoamingLocationUpdating),
            LocationUpdateType::PeriodicLocationUpdating => Some(LocationUpdateAcceptType::PeriodicLocationUpdating),
            LocationUpdateType::ItsiAttach => Some(LocationUpdateAcceptType::ItsiAttach),
            LocationUpdateType::ServiceRestorationRoamingLocationUpdating => {
                Some(LocationUpdateAcceptType::ServiceRestorationRoamingLocationUpdating)
            }
            LocationUpdateType::DemandLocationUpdating => Some(LocationUpdateAcceptType::DemandLocationUpdating),
            LocationUpdateType::MigratingLocationUpdating
            | LocationUpdateType::ServiceRestorationMigratingLocationUpdating
            | LocationUpdateType::DisabledMsUpdating => None,
        }
    }

    fn send_d_mm_status_energy_saving_with_status(
        queue: &mut MessageQueue,
        issi: u32,
        handle: u32,
        status_downlink: StatusDownlink,
        esi: EnergySavingInformation,
    ) {
        let pdu = DMmStatus {
            status_downlink,
            energy_saving_information: Some(esi),
        };

        let mut sdu = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut sdu).unwrap();
        sdu.seek(0);
        tracing::debug!("-> {} sdu {}", pdu, sdu.dump_bin());

        let msg = SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle,
                address: TetraAddress::issi(issi),
                layer2service: Layer2Service::Acknowledged,
                stealing_permission: false,
                stealing_repeats_flag: false,
                encryption_flag: false,
                is_null_pdu: false,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    /// Sends a D-MM-STATUS with ChangeOfEnergySavingModeResponse.
    fn send_d_mm_status_energy_saving(queue: &mut MessageQueue, issi: u32, handle: u32, esi: EnergySavingInformation) {
        Self::send_d_mm_status_energy_saving_with_status(queue, issi, handle, StatusDownlink::ChangeOfEnergySavingModeResponse, esi);
    }

    /// Sends a D-MM-STATUS with ChangeOfEnergySavingModeRequest.
    fn send_d_mm_status_energy_saving_request(queue: &mut MessageQueue, issi: u32, handle: u32, esi: EnergySavingInformation) {
        Self::send_d_mm_status_energy_saving_with_status(queue, issi, handle, StatusDownlink::ChangeOfEnergySavingModeRequest, esi);
    }

    fn feature_check_u_itsi_detach(pdu: &UItsiDetach) -> bool {
        let supported = true;
        if pdu.address_extension.is_some() {
            unimplemented_log!("Unsupported address_extension present");
        };
        if pdu.proprietary.is_some() {
            unimplemented_log!("Unsupported proprietary present");
        };
        supported
    }

    fn rx_u_tei_provide(&mut self, _queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_u_tei_provide");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let pdu = match UTeiProvide::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing UTeiProvide: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        let issi = prim.received_address.ssi;
        tracing::info!("MM: TEI received from ISSI {} → TEI={} ({:060b})", issi, pdu.tei_hex(), pdu.tei,);

        // Store TEI in client state for future use (e.g. whitelist checking)
        if let Err(e) = self.client_mgr.set_client_tei(issi, pdu.tei) {
            tracing::warn!("MM: failed to store TEI for ISSI {}: {:?}", issi, e);
        }
    }

    fn reject_cause_for_unsupported_u_location_update_demand(pdu: &ULocationUpdateDemand) -> Option<RejectCause> {
        if pdu.location_update_type == LocationUpdateType::MigratingLocationUpdating
            || pdu.location_update_type == LocationUpdateType::DisabledMsUpdating
        {
            unimplemented_log!("Unsupported {}", pdu.location_update_type);
            return Some(RejectCause::MessageConsistencyError);
        }
        if pdu.request_to_append_la == true {
            unimplemented_log!("Unsupported request_to_append_la == true");
            return Some(RejectCause::LaNotAllowed);
        }
        if pdu.cipher_control == true {
            unimplemented_log!("Unsupported cipher_control == true");
            return Some(RejectCause::NoCipherKsg);
        }
        if pdu.ciphering_parameters.is_some() {
            unimplemented_log!("Unsupported ciphering_parameters present");
            return Some(RejectCause::NoCipherKsg);
        }
        if pdu.la_information.is_some() {
            unimplemented_log!("Unsupported la_information present");
            return Some(RejectCause::LaNotAllowed);
        }
        if pdu.ssi.is_some() {
            tracing::debug!("DemandLocationUpdating: ssi present (expected from radio, ignored)");
        }
        if pdu.address_extension.is_some() {
            tracing::debug!("DemandLocationUpdating: address_extension present (expected from radio, ignored)");
        }
        if let Some(response) = &pdu.group_report_response {
            if response.len != 1 || response.data != 0 {
                tracing::warn!(
                    "DemandLocationUpdating: unsupported group_report_response len={} data={}",
                    response.len,
                    response.data
                );
                return Some(RejectCause::MessageConsistencyError);
            }
            if pdu.group_identity_location_demand.is_some() && pdu.location_update_type != LocationUpdateType::DemandLocationUpdating {
                tracing::warn!(
                    "{}: group_report_response present together with group_identity_location_demand outside a BS-commanded DemandLocationUpdating response",
                    pdu.location_update_type
                );
                return Some(RejectCause::MessageConsistencyError);
            }
        }
        if pdu.authentication_uplink.is_some() {
            unimplemented_log!("Unsupported authentication_uplink present");
            return Some(RejectCause::MessageConsistencyError);
        }
        if pdu.extended_capabilities.is_some() {
            // EN 300 392-2 clause 16.4.4 says the MS includes extended
            // capabilities in U-LOCATION UPDATE DEMAND when it supports one or
            // more of the listed items. Nexus-BS does not currently consume
            // those feature bits, but the IE itself is not a registration
            // consistency error.
            tracing::debug!("DemandLocationUpdating: extended_capabilities present (accepted, not acted on)");
        }
        if pdu.proprietary.is_some() {
            unimplemented_log!("Unsupported proprietary present");
            return Some(RejectCause::MessageConsistencyError);
        }

        None
    }

    fn group_report_response_is_complete(pdu: &ULocationUpdateDemand) -> bool {
        pdu.group_report_response
            .as_ref()
            .is_some_and(|response| response.len == 1 && response.data == 0)
    }

    /// Check for unsupported features in U-ATTACH/DETACH GROUP IDENTITY
    /// Returns false if a critical feature is missing
    fn feature_check_u_attach_detach_group_identity(pdu: &UAttachDetachGroupIdentity) -> bool {
        let mut supported = true;
        if pdu.group_identity_report == true {
            unimplemented_log!("Unsupported group_identity_report == true");
        }
        if pdu.group_identity_uplink.is_none() {
            unimplemented_log!("Missing group_identity_uplink");
            supported = false;
        }
        if pdu.group_report_response.is_some() {
            tracing::debug!("UAttachDetachGroupIdentity: group_report_response present (expected from radio, ignored)");
        }
        if pdu.proprietary.is_some() {
            unimplemented_log!("Unsupported proprietary present");
        }

        supported
    }
}

impl TetraEntityTrait for MmBs {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Mm
    }

    fn set_config(&mut self, config: SharedConfig) {
        self.config = config;
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        self.dltime = ts;

        if let Some(cep) = &self.control {
            while let Some(cmd) = cep.try_recv() {
                match cmd {
                    _ => {
                        tracing::warn!("MM: ignoring unsupported control command {:?}", cmd);
                    }
                }
            }
        }

        self.expire_pending_energy_saving(ts);
        self.expire_pending_swmi_group_transactions(queue, ts);
        self.expire_pending_solicited_group_reports(queue, ts);
        self.tick_restart_recovery(queue, ts);

        // Local periodic-registration watchdog. Do not call this T351:
        // EN 300 392-2 §16.11.1.1 defines T351 as the 10 s MS-side
        // registration response timer.
        // Uses wall-clock time — no TDMA precision needed.
        let interval_secs = self.config.config().cell.periodic_registration_secs;
        let expired = self.client_mgr.collect_expired_registrations(interval_secs);
        for issi in expired {
            tracing::info!(
                "MM: ISSI {} periodic registration expired ({}s) — sending D-LOCATION-UPDATE-COMMAND",
                issi,
                interval_secs
            );
            // Send D-LOCATION-UPDATE-COMMAND to prompt re-registration.
            //
            // Analysis of real traffic (MTM800/MXP600/MTM5400) shows these terminals
            // may not perform periodic registration at the interval configured for
            // this local SwMI watchdog.
            // They rely entirely on BS initiative to re-register.
            //
            // - REJECT(ExpiryOfTimer): terminals enter waiting state, never re-attach. BAD.
            // - Silent removal: terminals never notice, never re-register. BAD.
            // - D-LOCATION-UPDATE-COMMAND: terminals respond with U-LOCATION-UPDATING-DEMAND
            //   (DemandLocationUpdating), BS re-registers them immediately. GOOD.
            //
            // The Roaming loop bug from before is NOT triggered here because:
            // 1. This command is sent once per expiry, not on every registration.
            // 2. The fix in rx_u_location_updating_demand already skips sending
            //    COMMAND after RoamingLocationUpdating.
            let already_sent = self.client_mgr.is_pending_command(issi);
            if already_sent {
                // Second expiry — terminal didn't respond to COMMAND within grace period.
                // Send D-LOCATION-UPDATE-REJECT(ExpiryOfTimer) so the terminal knows it must
                // re-attach. Without this, terminals like Sepura stay "connected" locally
                // while the BS has already removed them, causing a silent desync.
                let last_handle = self.client_mgr.get_client_by_issi(issi).map(|c| c.last_handle).unwrap_or(0);
                tracing::info!(
                    "MM: ISSI {} did not respond to D-LOCATION-UPDATE-COMMAND — sending REJECT and removing",
                    issi
                );
                Self::send_d_location_update_reject_cause(
                    queue,
                    issi,
                    last_handle,
                    LocationUpdateType::PeriodicLocationUpdating,
                    None,
                    RejectCause::ExpiryOfTimer,
                );
                let detached = self.client_mgr.remove_client(issi);
                if let Some(client) = detached {
                    self.forget_energy_saving(issi);
                    self.clear_solicited_group_report(issi);
                    self.clear_critical_downlinks_for_issi(issi);
                    self.abandon_pending_swmi_group_transaction(
                        issi,
                        "periodic registration reject/removal terminates pending SwMI group procedure",
                    );
                    self.config.state_write().subscribers.deregister(issi);
                    if !client.groups.is_empty() {
                        let groups: Vec<u32> = client.groups.iter().copied().collect();
                        self.emit_subscriber_update(queue, issi, groups, BrewSubscriberAction::Deaffiliate);
                    }
                    self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
                }
                continue;
            }
            // First expiry — send COMMAND and wait grace period (60s) for response.
            // Do NOT remove_client here: keeping the client in registry preserves ESM
            // and group state so the terminal re-registers cleanly without losing EE mode.
            // Only notify Brew so it stops routing calls to this terminal until it re-registers.
            let last_handle = self.client_mgr.get_client_by_issi(issi).map(|c| c.last_handle).unwrap_or(0);
            self.send_d_location_update_command(queue, issi, last_handle);
            // EN 300 392-2 clause 16.8.6: registration overrides group
            // attach/detach/report procedures. Once the BS asks the MS to
            // re-register, a later U-ATTACH/DETACH GROUP IDENTITY ACK must not
            // complete an older SwMI group transaction in Annex G.
            self.abandon_pending_swmi_group_transaction(issi, "periodic D-LOCATION UPDATE COMMAND starts a fresh registration procedure");
            self.client_mgr.set_pending_command(issi, 60);
            let groups: Vec<u32> = self
                .client_mgr
                .get_client_by_issi(issi)
                .map(|c| c.groups.iter().copied().collect())
                .unwrap_or_default();
            if !groups.is_empty() {
                self.emit_subscriber_update(queue, issi, groups, BrewSubscriberAction::Deaffiliate);
            }
            self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
            // Mark as detached in state but keep in client_mgr (preserves ESM + groups)
            self.config.state_write().subscribers.deregister(issi);
        }
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);
        // tracing::debug!(ts=%message.dltime, "rx_prim: {:?}", message);

        match message.sap {
            Sap::LmmSap => match message.msg {
                SapMsgInner::LmmMleUnitdataInd(_) => {
                    self.rx_lmm_mle_unitdata_ind(queue, message);
                }
                SapMsgInner::LmmMleReportInd(prim) => {
                    self.rx_lmm_mle_report_ind(queue, prim.handle, prim.transfer_result);
                }
                _ => {
                    tracing::error!("BUG: unexpected message or state -- routing error");
                    return;
                }
            },
            Sap::Control => {
                match message.msg {
                    SapMsgInner::BrewReconnected => {
                        self.rx_brew_reconnected(queue);
                    }
                    SapMsgInner::MsRssiUpdate { issi, rssi_dbfs } => {
                        self.client_mgr.update_client_rssi(issi, rssi_dbfs);
                        // Emit RSSI telemetry for dashboard
                        if let Some(sink) = &self.telemetry {
                            sink.send(crate::net_telemetry::TelemetryEvent::MsRssi { issi, rssi_dbfs });
                        }
                        // Forward to Brew entity for optional export to Brew server.
                        // BrewEntity applies its own rate limiting and checks feature_rssi_export.
                        queue.push_back(SapMsg {
                            sap: Sap::Control,
                            src: TetraEntity::Mm,
                            dest: TetraEntity::Brew,
                            msg: SapMsgInner::MsRssiUpdate { issi, rssi_dbfs },
                        });
                    }
                    SapMsgInner::MmSubscriberUpdate(update) => {
                        // CMCE can ask MM to deregister an MS (e.g. kick from dashboard)
                        if update.action == BrewSubscriberAction::Deregister {
                            let issi = update.issi;
                            tracing::info!(
                                "MM: kicking ISSI {} — sending D-LOCATION-UPDATE-COMMAND to force re-registration",
                                issi
                            );
                            // D-LOCATION-UPDATE-COMMAND forces the terminal to immediately
                            // send a new U-LOCATION-UPDATING-DEMAND, effectively re-registering.
                            // This is cleaner than a reject: the terminal stays on the network
                            // but goes through a full re-registration cycle.
                            self.send_d_location_update_command(queue, issi, 0);
                            // EN 300 392-2 clause 16.8.6 gives registration
                            // precedence over group attach/detach/report
                            // procedures, so dashboard kick/re-registration
                            // invalidates any pending SwMI group ACK.
                            self.abandon_pending_swmi_group_transaction(issi, "control kick starts a fresh registration procedure");
                            let groups: Vec<u32> = self
                                .client_mgr
                                .get_client_by_issi(issi)
                                .map(|c| c.groups.iter().copied().collect())
                                .unwrap_or_default();
                            if !groups.is_empty() {
                                self.emit_subscriber_update(queue, issi, groups, BrewSubscriberAction::Deaffiliate);
                            }
                            self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
                            self.client_mgr.remove_client(issi);
                            self.forget_energy_saving(issi);
                            self.clear_solicited_group_report(issi);
                            self.clear_critical_downlinks_for_issi(issi);
                            self.config.state_write().subscribers.deregister(issi);
                        }
                    }
                    _ => {
                        tracing::warn!("mm_bs: unexpected Control message from {:?}", message.src);
                    }
                }
            }
            _ => {
                tracing::warn!("MM: unexpected SAP {:?}, ignoring", message.sap);
            }
        }
    }
}

impl MmBs {
    /// Called when Brew backhaul reconnects. Sends D-LOCATION-UPDATE-COMMAND to all
    /// locally registered MS to force them to re-affiliate. This fixes the PTT-denied
    /// symptom where MS units registered before a Brew disconnect never re-register.
    fn rx_brew_reconnected(&mut self, queue: &mut MessageQueue) {
        let clients: Vec<(u32, u32)> = self.client_mgr.all_clients_with_handle().collect();
        if clients.is_empty() {
            tracing::info!("mm_bs: BrewReconnected — no registered MS to re-register");
            return;
        }
        tracing::info!(
            "mm_bs: BrewReconnected — sending D-LOCATION-UPDATE-COMMAND to {} MS unit(s)",
            clients.len()
        );
        for (issi, handle) in clients {
            tracing::debug!("mm_bs: re-registering ISSI {} (handle={})", issi, handle);
            self.send_d_location_update_command(queue, issi, handle);
            // EN 300 392-2 clause 16.4.3 permits SwMI-initiated
            // registration by D-LOCATION UPDATE COMMAND, and clause 16.8.6
            // makes that registration procedure override pending group
            // attach/detach/report procedures. Keep the existing client state
            // while marking the command pending so the later
            // DemandLocationUpdating response replays Register/Affiliate
            // coherently after Brew reconnect.
            self.abandon_pending_swmi_group_transaction(
                issi,
                "Brew reconnect D-LOCATION UPDATE COMMAND starts a fresh registration procedure",
            );
            self.client_mgr.set_pending_command(issi, 60);
        }
    }
}
