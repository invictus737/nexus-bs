// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::collections::{HashMap, HashSet};

use crate::net_telemetry::{TelemetryEvent, channel::TelemetrySink};
use tetra_pdus::mm::enums::energy_saving_mode::EnergySavingMode;
use tetra_pdus::mm::fields::class_of_ms::ClassOfMs;

#[derive(Debug)]
pub enum ClientMgrErr {
    ClientNotFound { issi: u32 },
    GroupNotFound { gssi: u32 },
    IssiInGroupRange { issi: u32 },
    GssiInClientRange { gssi: u32 },
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum MmClientState {
    Unknown,
    Attached,
    Detached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupAttachmentInfo {
    pub group_identity_attachment_lifetime: u8,
    pub class_of_usage: u8,
}

impl Default for GroupAttachmentInfo {
    fn default() -> Self {
        Self {
            group_identity_attachment_lifetime: 0,
            class_of_usage: 0,
        }
    }
}

pub struct MmClientProperties {
    pub issi: u32,
    pub state: MmClientState,
    pub groups: HashSet<u32>,
    pub group_attachments: HashMap<u32, GroupAttachmentInfo>,
    pub energy_saving_mode: EnergySavingMode,
    /// TDMA frame number (0-17) at which this MS wakes up to monitor MCCH.
    /// Set to None for StayAlive MSs. Used by CMCE to schedule DSetup retransmissions.
    pub monitoring_frame: Option<u8>,
    /// Multiframe offset within the Eg cycle at which this MS wakes up.
    /// Set to None for StayAlive MSs.
    pub monitoring_multiframe: Option<u8>,
    /// Last measured RSSI from this MS in dBFS (dB relative to ADC full scale).
    /// Updated on every UL burst received from this ISSI.
    /// None until first burst received after registration.
    pub last_rssi: Option<f32>,
    /// Timestamp (system time) when this MS last registered or re-registered.
    /// Used by the local SwMI periodic-registration watchdog. This is distinct
    /// from ETSI T351, the MS-side registration response timer.
    pub last_registration_time: std::time::Instant,
    pub class_of_ms: Option<ClassOfMs>,
    /// Layer-2 handle from the last successful location update.
    /// Required for sending downlink MM PDUs (D-LOCATION-UPDATE-COMMAND etc.)
    /// to this MS. Set to 0 until the first location update is received.
    pub last_handle: u32,
    /// Terminal Equipment Identity (60-bit hardware ID, like IMEI).
    /// Set when the MS sends U-TEI-PROVIDE. None if not yet received.
    pub tei: Option<u64>,
    /// True after BS sends D-LOCATION-UPDATE-COMMAND at local watchdog expiry.
    /// If the terminal re-registers, this is cleared. If the grace period
    /// expires while this is still true, the local watchdog clears this flag
    /// fail-soft and keeps the subscriber route intact for service continuity.
    pub pending_command_sent: bool,
    /// When Some, terminal has until this instant to respond to D-LOCATION-UPDATE-COMMAND.
    pub grace_expires_at: Option<std::time::Instant>,
    // pub last_seen: TdmaTime,
}

impl MmClientProperties {
    pub fn new(ssi: u32) -> Self {
        MmClientProperties {
            issi: ssi,
            state: MmClientState::Unknown,
            groups: HashSet::new(),
            group_attachments: HashMap::new(),
            energy_saving_mode: EnergySavingMode::StayAlive,
            monitoring_frame: None,
            monitoring_multiframe: None,
            last_rssi: None,
            pending_command_sent: false,
            grace_expires_at: None,
            last_registration_time: std::time::Instant::now(),
            class_of_ms: None,
            last_handle: 0,
            tei: None,
            // last_seen: TdmaTime::default(),
        }
    }
}

/// Lightweight range guard local to the client manager.
/// Configured GSSI provisioning is enforced in `MmBs::try_attach_detach_groups`,
/// where SharedConfig is available.
fn is_individual(issi: u32) -> bool {
    issi <= 0x00FF_FFFF
}
/// GSSIs are 24-bit SSI values on the air interface.
fn in_group_range(gssi: u32) -> bool {
    gssi <= 0x00FF_FFFF
}
/// The client manager has no provisioning context; explicit allowed-GSSI policy
/// is applied by the BS MM entity before this method mutates affiliation state.
fn is_group(gssi: u32) -> bool {
    gssi <= 0x00FF_FFFF
}
/// Per-ISSI/per-GSSI authorization is a BS policy decision and is applied by
/// the caller when configured. Keep this fallback permissive for dynamic
/// radio-provided scan lists.
fn may_attach(_issi: u32, _gssi: u32) -> bool {
    return true;
}

pub struct MmClientMgr {
    clients: HashMap<u32, MmClientProperties>,
    telemetry_sink: Option<TelemetrySink>,
}

impl MmClientMgr {
    pub fn telemetry_sink(&self) -> Option<&TelemetrySink> {
        self.telemetry_sink.as_ref()
    }

    pub fn new(telemetry_sink: Option<TelemetrySink>) -> Self {
        MmClientMgr {
            clients: HashMap::new(),
            telemetry_sink,
        }
    }

    pub fn get_client_by_issi(&mut self, issi: u32) -> Option<&MmClientProperties> {
        self.clients.get(&issi)
    }

    pub fn client_is_known(&self, issi: u32) -> bool {
        self.clients.contains_key(&issi)
    }

    pub fn set_client_state(&mut self, issi: u32, state: MmClientState) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.state = state;
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    pub fn set_client_energy_saving_mode(&mut self, issi: u32, mode: EnergySavingMode) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.energy_saving_mode = mode;
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    pub fn set_client_monitoring_window(&mut self, issi: u32, frame: Option<u8>, multiframe: Option<u8>) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.monitoring_frame = frame;
            client.monitoring_multiframe = multiframe;
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    /// Update RSSI for a known MS. Silently ignored if MS is not registered.
    pub fn update_client_rssi(&mut self, issi: u32, rssi_dbfs: f32) {
        if let Some(client) = self.clients.get_mut(&issi) {
            let should_log = match client.last_rssi {
                None => true,                                  // First measurement
                Some(prev) => (rssi_dbfs - prev).abs() >= 3.0, // Log on >=3dB change
            };
            client.last_rssi = Some(rssi_dbfs);
            if should_log {
                tracing::info!("RSSI: ISSI {} = {:.1} dBFS", issi, rssi_dbfs);
            }
        }
    }

    /// Reset the periodic registration timer for a MS (called on each U-LOCATION-UPDATING-DEMAND).
    pub fn reset_registration_timer(&mut self, issi: u32) {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.last_registration_time = std::time::Instant::now();
            client.pending_command_sent = false;
            client.grace_expires_at = None;
        }
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_backdate_registration_for_test(&mut self, issi: u32, elapsed: std::time::Duration) -> bool {
        let Some(client) = self.clients.get_mut(&issi) else {
            return false;
        };
        client.last_registration_time = std::time::Instant::now()
            .checked_sub(elapsed)
            .unwrap_or_else(std::time::Instant::now);
        true
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_expire_registration_grace_for_test(&mut self, issi: u32) -> bool {
        let Some(client) = self.clients.get_mut(&issi) else {
            return false;
        };
        client.pending_command_sent = true;
        client.grace_expires_at = Some(
            std::time::Instant::now()
                .checked_sub(std::time::Duration::from_millis(1))
                .unwrap_or_else(std::time::Instant::now),
        );
        true
    }

    /// Returns true if a D-LOCATION-UPDATE-COMMAND was sent and terminal hasn't responded yet.
    pub fn is_pending_command(&self, issi: u32) -> bool {
        self.clients.get(&issi).map(|c| c.pending_command_sent).unwrap_or(false)
    }

    /// Mark that we sent D-LOCATION-UPDATE-COMMAND at local watchdog expiry.
    /// Terminal has grace_secs to respond before the watchdog abandons this
    /// probe and retries at the next periodic interval.
    pub fn set_pending_command(&mut self, issi: u32, grace_secs: u32) {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.pending_command_sent = true;
            // Set last_registration_time so elapsed() > interval after grace_secs.
            // Achieved by back-dating: last_registration_time = now - (interval - grace_secs)
            // But we don't know interval here, so we use a simpler approach:
            // collect_expired_registrations checks pending_command_sent + grace separately.
            client.grace_expires_at = Some(std::time::Instant::now() + std::time::Duration::from_secs(grace_secs as u64));
        }
    }

    /// Returns list of ISSIs whose periodic registration has expired.
    /// interval_secs=0 means disabled — always returns empty list.
    pub fn collect_expired_registrations(&self, interval_secs: u32) -> Vec<u32> {
        if interval_secs == 0 {
            return Vec::new();
        }
        let threshold = std::time::Duration::from_secs(interval_secs as u64);
        let now = std::time::Instant::now();
        self.clients
            .iter()
            .filter(|(_, c)| {
                if c.pending_command_sent {
                    // Already sent COMMAND — report once when grace expires.
                    c.grace_expires_at.map(|d| now >= d).unwrap_or(true)
                } else {
                    // Normal local periodic-registration watchdog check.
                    c.last_registration_time.elapsed() > threshold
                }
            })
            .map(|(&issi, _)| issi)
            .collect()
    }

    pub fn set_client_class_of_ms(&mut self, issi: u32, class: Option<ClassOfMs>) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.class_of_ms = class;
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    /// Store the TEI (Terminal Equipment Identity) received from U-TEI-PROVIDE.
    /// If the ISSI is not registered yet, the TEI is silently ignored (can't fail critically).
    pub fn set_client_tei(&mut self, issi: u32, tei: u64) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.tei = Some(tei);
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    /// Registers a fresh state for a client, based on ssi
    /// If client is already registered, previous state is discarded.
    pub fn try_register_client(&mut self, issi: u32, attached: bool) -> Result<bool, ClientMgrErr> {
        if !is_individual(issi) {
            return Err(ClientMgrErr::IssiInGroupRange { issi });
        };

        // discard previous state if any
        self.clients.remove(&issi);

        // Create and insert new client state
        let mut elem = MmClientProperties::new(issi);
        elem.state = if attached {
            MmClientState::Attached
        } else {
            MmClientState::Unknown
        };
        self.clients.insert(issi, elem);

        // Send telemetry event
        if let Some(sink) = &self.telemetry_sink {
            sink.send(TelemetryEvent::MsRegistration { issi });
        }

        Ok(true)
    }

    /// Removes a client from the registry, returning its properties if found
    /// Returns (issi, last_handle) for all known clients that have a valid handle (handle != 0).
    /// Used by mm_bs to send D-LOCATION-UPDATE-COMMAND after Brew reconnection.
    pub fn all_clients_with_handle(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.clients
            .values()
            .filter(|c| c.last_handle != 0)
            .map(|c| (c.issi, c.last_handle))
    }

    /// Deterministic snapshot of every known client for operator-wide cleanup.
    pub fn all_clients_snapshot(&self) -> Vec<(u32, u32, Vec<u32>)> {
        let mut clients: Vec<(u32, u32, Vec<u32>)> = self
            .clients
            .values()
            .map(|client| {
                let mut groups: Vec<u32> = client.groups.iter().copied().collect();
                groups.sort_unstable();
                (client.issi, client.last_handle, groups)
            })
            .collect();
        clients.sort_by_key(|(issi, _, _)| *issi);
        clients
    }

    /// Update the last known L2 handle for a registered client.
    pub fn set_client_handle(&mut self, issi: u32, handle: u32) {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.last_handle = handle;
        }
    }

    pub fn remove_client(&mut self, ssi: u32) -> Option<MmClientProperties> {
        if let Some(client) = self.clients.remove(&ssi) {
            // Send telemetry event
            if let Some(sink) = &self.telemetry_sink {
                sink.send(TelemetryEvent::MsDeregistration { issi: ssi });
            }
            Some(client)
        } else {
            None
        }
    }

    fn client_detach_all_groups_inner(&mut self, issi: u32, emit_telemetry: bool) -> Result<bool, ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            if emit_telemetry {
                if let Some(sink) = &self.telemetry_sink {
                    sink.send(TelemetryEvent::MsGroupDetach {
                        issi: client.issi,
                        gssis: client.groups.iter().cloned().collect(),
                    });
                }
            }
            client.groups.clear();
            client.group_attachments.clear();
            Ok(true)
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    /// Detaches all groups from a client.
    pub fn client_detach_all_groups(&mut self, issi: u32) -> Result<bool, ClientMgrErr> {
        self.client_detach_all_groups_inner(issi, true)
    }

    /// Detaches all groups without emitting an intermediate telemetry event.
    /// Used when MM applies ETSI mode=1 as one logical replace operation and
    /// will publish the final group list after accepted attachments are known.
    pub fn client_detach_all_groups_silent(&mut self, issi: u32) -> Result<bool, ClientMgrErr> {
        self.client_detach_all_groups_inner(issi, false)
    }

    /// Attaches or detaches a client from a group
    pub fn client_group_attach(&mut self, issi: u32, gssi: u32, do_attach: bool) -> Result<bool, ClientMgrErr> {
        self.client_group_attach_with_info(issi, gssi, do_attach, GroupAttachmentInfo::default())
    }

    /// Attaches or detaches a client from a group while preserving the ETSI
    /// group-identity attachment sub-elements needed for later group reports.
    pub fn client_group_attach_with_info(
        &mut self,
        issi: u32,
        gssi: u32,
        do_attach: bool,
        attachment_info: GroupAttachmentInfo,
    ) -> Result<bool, ClientMgrErr> {
        // Checks
        if !in_group_range(gssi) {
            return Err(ClientMgrErr::GssiInClientRange { gssi });
        };
        if !is_group(gssi) {
            return Err(ClientMgrErr::GroupNotFound { gssi });
        };
        if !may_attach(issi, gssi) {
            return Err(ClientMgrErr::GroupNotFound { gssi });
        };

        if let Some(client) = self.clients.get_mut(&issi) {
            if do_attach {
                // Send telemetry event
                if let Some(sink) = &self.telemetry_sink {
                    sink.send(TelemetryEvent::MsGroupAttach {
                        issi: client.issi,
                        gssis: vec![gssi].into_iter().collect(),
                    });
                }

                let changed = client.groups.insert(gssi);
                client.group_attachments.insert(gssi, attachment_info);
                Ok(changed)
            } else {
                let changed = client.groups.remove(&gssi);
                client.group_attachments.remove(&gssi);
                Ok(changed)
            }
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    pub fn client_group_attachment_info(&self, issi: u32, gssi: u32) -> Option<GroupAttachmentInfo> {
        self.clients
            .get(&issi)
            .and_then(|client| client.group_attachments.get(&gssi).copied())
    }
}
