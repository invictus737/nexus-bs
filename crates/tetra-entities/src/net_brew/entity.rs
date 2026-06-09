//! Brew protocol entity bridging a remote network backend to UMAC/MLE with hangtime-based circuit reuse
//!
//! Transport-agnostic: the concrete transport (WebSocket, QUIC, TCP, …) is
//! injected at construction time via [`BrewEntity::new`].

use std::collections::{HashMap, HashSet};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};
use tetra_saps::control::enums::sds_user_data::SdsUserData;
use tetra_saps::control::sds::CmceSdsData;
use uuid::Uuid;

use crate::net_brew::components::jitter_buffer::{JitterFrame, VoiceJitterBuffer};
use crate::net_telemetry::{TelemetryEvent, channel::TelemetrySink};
use crate::network::transports::NetworkTransport;
use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::{CfgBrew, SharedConfig};
use tetra_core::{Sap, TdmaTime, tetra_entities::TetraEntity};
use tetra_core::{TxReporter, TxState};
use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
use tetra_saps::{
    SapMsg, SapMsgInner,
    control::call_control::{CallControl, NetworkCircuitCall},
    tmd::TmdCircuitDataReq,
};

use super::worker::{BrewCommand, BrewEvent, BrewWorker};

/// Hangtime before releasing group call circuit to allow reuse without re-signaling.
const GROUP_CALL_HANGTIME_DEFAULT_SECS: u64 = 5;
const TETRA_ACELP_TCH_S_BITS: usize = 274;
const BREW_STE_FRAME_BYTES: usize = 36;
const BREW_STE_FRAME_BITS: u16 = (BREW_STE_FRAME_BYTES * 8) as u16;
const BREW_STE_HEADER_NORMAL_SPEECH: u8 = 0x80;
const BREW_STE_UNUSED_TAIL_MASK: u8 = 0x3f;
const BREW_EVENT_CHANNEL_CAPACITY: usize = 2048;
const BREW_COMMAND_CHANNEL_CAPACITY: usize = 8192;
const BREW_CRITICAL_COMMAND_TIMEOUT: Duration = Duration::from_millis(1);

fn send_brew_command(sender: &Sender<BrewCommand>, command: BrewCommand) -> bool {
    let label = command.label();
    let critical = command.is_critical();
    let sent = if critical {
        match sender.send_timeout(command, BREW_CRITICAL_COMMAND_TIMEOUT) {
            Ok(()) => true,
            Err(crossbeam_channel::SendTimeoutError::Timeout(_)) | Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => false,
        }
    } else {
        sender.try_send(command).is_ok()
    };

    if !sent {
        if critical {
            tracing::warn!("BrewEntity: critical command queue overloaded; dropped {label}");
        } else {
            tracing::debug!("BrewEntity: non-critical command queue overloaded; dropped {label}");
        }
    }

    sent
}

// ─── Active call tracking ─────────────────────────────────────────

/// Tracks the state of a single active Brew group call (currently transmitting)
#[derive(Debug)]
struct ActiveCall {
    /// Brew session UUID
    uuid: Uuid,
    /// TETRA call identifier (14-bit) - None until NetworkCallReady received
    call_id: Option<u16>,
    /// Allocated timeslot (2-4) - None until NetworkCallReady received
    ts: Option<u8>,
    /// Usage number for the channel allocation - None until NetworkCallReady received
    usage: Option<u8>,
    /// Calling party ISSI (from Brew)
    source_issi: u32,
    /// Destination GSSI (from Brew)
    dest_gssi: u32,
    /// Number of voice frames received
    frame_count: u64,
}

/// Group call in hangtime with circuit still allocated.
#[derive(Debug)]
struct HangingCall {
    /// Brew session UUID
    uuid: Uuid,
    /// TETRA call identifier (14-bit)
    call_id: u16,
    /// Allocated timeslot (2-4)
    ts: u8,
    /// Usage number for the channel allocation
    usage: u8,
    /// Last calling party ISSI (needed for D-SETUP re-send during late entry)
    source_issi: u32,
    /// Destination GSSI
    dest_gssi: u32,
    /// Total voice frames received during the call
    frame_count: u64,
    /// When the call entered hangtime (wall clock)
    since: Instant,
}

/// Kind of UL call being forwarded to TetraPack
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UlForwardKind {
    /// PTT group call (floor-controlled)
    Group,
    /// Full-duplex individual circuit call
    Circuit,
}

/// Tracks a local UL call being forwarded to TetraPack
#[derive(Debug)]
struct UlForwardedCall {
    /// Brew session UUID for this forwarded call
    uuid: Uuid,
    /// TETRA call identifier
    call_id: u16,
    /// Source ISSI of the calling radio
    source_issi: u32,
    /// Destination GSSI (group calls) or called ISSI (circuit calls)
    dest_gssi: u32,
    /// Number of voice frames forwarded
    frame_count: u64,
    /// Call kind: group PTT or individual circuit
    kind: UlForwardKind,
}

#[derive(Debug)]
struct PendingSdsReport {
    reporter: TxReporter,
}

// ─── BrewEntity ───────────────────────────────────────────────────

pub struct BrewEntity {
    config: SharedConfig,

    /// Also contained in the SharedConfig, but kept for fast, convenient access
    brew_config: CfgBrew,

    dltime: TdmaTime,

    /// Receive events from the worker thread
    event_receiver: Receiver<BrewEvent>,
    /// Send commands to the worker thread
    command_sender: Sender<BrewCommand>,

    /// Active DL calls from Brew keyed by session UUID (currently transmitting)
    active_calls: HashMap<Uuid, ActiveCall>,
    /// Per-call jitter/playout buffer for downlink voice from Brew.
    dl_jitter: HashMap<Uuid, VoiceJitterBuffer>,
    /// Jitter buffers that are draining after GROUP_IDLE — kept alive until empty so the
    /// last frames of a transmission are played out instead of being silently discarded.
    /// The Instant records when draining started, so a buffer whose timeslot stops being
    /// scheduled (and therefore never finishes draining naturally) can be reaped instead
    /// of leaking across repeated PTT cycles.
    draining_jitter: HashMap<Uuid, (u8, VoiceJitterBuffer, Instant)>,

    /// DL calls in hangtime keyed by dest_gssi — circuit stays open, waiting for
    /// new speaker or timeout. Only one hanging call per GSSI.
    hanging_calls: HashMap<u32, HangingCall>,

    /// UL calls being forwarded to TetraPack, keyed by timeslot
    ul_forwarded: HashMap<u8, UlForwardedCall>,

    /// Registered subscriber groups (ISSI -> set of GSSIs)
    subscriber_groups: HashMap<u32, HashSet<u32>>,

    /// Whether the worker is connected
    connected: bool,
    /// Optional telemetry sink for emitting brew status events
    telemetry_sink: Option<TelemetrySink>,

    /// Rate limiting for RSSI export: tracks last sent time per ISSI.
    /// Only used when feature_rssi_export is enabled in config.
    rssi_last_sent: HashMap<u32, Instant>,

    /// Brew-origin SDS reports waiting for the air-interface TxReporter to
    /// reach a terminal state before reporting success or failure upstream.
    pending_sds_reports: HashMap<Uuid, PendingSdsReport>,

    /// Worker thread handle for graceful shutdown
    worker_handle: Option<thread::JoinHandle<()>>,
}

impl BrewEntity {
    fn brew_routable_groups(&self, groups: impl IntoIterator<Item = u32>) -> Vec<u32> {
        groups
            .into_iter()
            .filter(|gssi| super::is_brew_gssi_routable(&self.config, *gssi))
            .collect()
    }

    /// Create a new BrewEntity with the given transport.
    ///
    /// The transport is moved into a worker thread. Any [`NetworkTransport`]
    /// implementation can be used (WebSocket, QUIC, TCP, …).
    pub fn new<T: NetworkTransport + 'static>(config: SharedConfig, transport: T) -> Self {
        // Create channels
        let (event_sender, event_receiver) = bounded::<BrewEvent>(BREW_EVENT_CHANNEL_CAPACITY);
        let (command_sender, command_receiver) = bounded::<BrewCommand>(BREW_COMMAND_CHANNEL_CAPACITY);

        // Spawn worker thread with the provided transport
        let brew_config = config.config().as_ref().brew.clone().unwrap(); // Never fails
        let worker_config = config.clone();
        let handle = thread::Builder::new()
            .name("brew-worker".to_string())
            .spawn(move || {
                let mut worker = BrewWorker::new(worker_config, event_sender, command_receiver, transport);
                worker.run();
            })
            .expect("failed to spawn BrewWorker thread");

        {
            let mut state = config.state_write();
            state.network_connected = false;
        }

        Self {
            config,
            brew_config,
            dltime: TdmaTime::default(),
            event_receiver,
            command_sender,
            active_calls: HashMap::new(),
            dl_jitter: HashMap::new(),
            draining_jitter: HashMap::new(),
            hanging_calls: HashMap::new(),
            ul_forwarded: HashMap::new(),
            subscriber_groups: HashMap::new(),
            connected: false,
            telemetry_sink: None,
            rssi_last_sent: HashMap::new(),
            pending_sds_reports: HashMap::new(),
            worker_handle: Some(handle),
        }
    }

    /// Set telemetry sink for emitting brew status events.
    pub fn set_telemetry_sink(&mut self, sink: TelemetrySink) {
        self.telemetry_sink = Some(sink);
    }

    /// Process all pending events from the worker thread
    fn process_events(&mut self, queue: &mut MessageQueue) {
        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                BrewEvent::Connected { server_version } => {
                    tracing::debug!("BrewEntity: connected to TetraPack server (Brew v{})", server_version);
                    self.connected = true;
                    self.resync_subscribers();
                    self.set_network_connected(true, server_version);
                }
                BrewEvent::VersionDetected { version } => {
                    tracing::info!("BrewEntity: server Brew version detected from message length: v{}", version);
                    self.emit_brew_version(version);
                    // Notify MM that Brew reconnected so it can send D-LOCATION-UPDATE-COMMAND
                    // to all locally registered MS. Without this, MS units that were registered
                    // before the disconnect believe they are still affiliated and do not
                    // re-register — PTT calls are denied until the radio is power-cycled.
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Mm,
                        msg: SapMsgInner::BrewReconnected,
                    });
                }
                BrewEvent::Disconnected(reason) => {
                    tracing::warn!("BrewEntity: Brew backhaul disconnected: {} — releasing all active calls", reason);
                    self.set_network_connected(false, 0);
                    // ETSI EN 300 392-2 §14.9.4: BS must release all circuits immediately
                    // when backhaul connection is lost. MS will receive D-RELEASE.
                    self.release_all_calls(queue);
                }
                BrewEvent::GroupCallStart {
                    uuid,
                    source_issi,
                    dest_gssi,
                    priority,
                    service,
                } => {
                    tracing::info!("BrewEntity: GROUP_TX service={} (0=TETRA ACELP, expect 0)", service);
                    self.handle_group_call_start(queue, uuid, source_issi, dest_gssi, priority);
                }
                BrewEvent::GroupCallEnd { uuid, cause } => {
                    self.handle_group_call_end(queue, uuid, cause);
                }
                BrewEvent::VoiceFrame { uuid, length_bits, data } => {
                    self.handle_voice_frame(uuid, length_bits, data);
                }
                BrewEvent::SdsTransfer {
                    uuid,
                    source,
                    destination,
                    data,
                    length_bits,
                } => {
                    self.handle_sds_transfer(queue, uuid, source, destination, data, length_bits);
                }
                BrewEvent::SdsReport { uuid, status } => {
                    tracing::debug!("BrewEntity: SDS report uuid={} status={}", uuid, status);
                }
                BrewEvent::SubscriberEvent { msg_type, issi, groups } => {
                    tracing::debug!("BrewEntity: subscriber event type={} issi={} groups={:?}", msg_type, issi, groups);
                    // External subscriber (e.g. SvxLink gateway) affiliated/deaffiliated on Brew server.
                    // Notify CMCE so it updates group_listeners — without this, has_listener()
                    // returns false for GSSIs where only external subscribers are present,
                    // causing BS to reject U-SETUP with "no listeners".
                    match msg_type {
                        crate::net_brew::protocol::BREW_SUBSCRIBER_AFFILIATE => {
                            if !groups.is_empty() {
                                tracing::info!("BrewEntity: external subscriber issi={} → AFFILIATE groups={:?}", issi, groups);
                                queue.push_back(SapMsg {
                                    sap: tetra_core::Sap::Control,
                                    src: TetraEntity::Brew,
                                    dest: TetraEntity::Cmce,
                                    msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
                                        issi,
                                        groups: groups.clone(),
                                        action: BrewSubscriberAction::Affiliate,
                                    }),
                                });
                            }
                        }
                        crate::net_brew::protocol::BREW_SUBSCRIBER_DEAFFILIATE => {
                            if !groups.is_empty() {
                                tracing::info!("BrewEntity: external subscriber issi={} → DEAFFILIATE groups={:?}", issi, groups);
                                queue.push_back(SapMsg {
                                    sap: tetra_core::Sap::Control,
                                    src: TetraEntity::Brew,
                                    dest: TetraEntity::Cmce,
                                    msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
                                        issi,
                                        groups: groups.clone(),
                                        action: BrewSubscriberAction::Deaffiliate,
                                    }),
                                });
                            }
                        }
                        crate::net_brew::protocol::BREW_SUBSCRIBER_DEREGISTER => {
                            tracing::info!("BrewEntity: external subscriber issi={} → DEREGISTER", issi);
                            queue.push_back(SapMsg {
                                sap: tetra_core::Sap::Control,
                                src: TetraEntity::Brew,
                                dest: TetraEntity::Cmce,
                                msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
                                    issi,
                                    groups: Vec::new(),
                                    action: BrewSubscriberAction::Deregister,
                                }),
                            });
                        }
                        _ => {}
                    }
                }
                BrewEvent::ServerError { error_type, data } => {
                    tracing::error!("BrewEntity: server error type={} data={} bytes", error_type, data.len());
                }

                // ── Circuit / individual call events ──────────────────────
                BrewEvent::CircuitSetupRequest { uuid, call } => {
                    // TetraPack initiates a call to a local MS (BS is the called side).
                    // Map Brew wire struct → SAP NetworkCircuitCall and forward to CMCE.
                    let network_call = Self::map_brew_to_network_circuit_call(&call);
                    tracing::info!(
                        "BrewEntity: CIRCUIT SETUP REQUEST uuid={} src={} dst={} number='{}' duplex={}",
                        uuid,
                        call.source,
                        call.destination,
                        call.number,
                        call.duplex
                    );
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest {
                            brew_uuid: uuid,
                            call: network_call,
                        }),
                    });
                }
                BrewEvent::CircuitSetupAccept { uuid } => {
                    tracing::info!("BrewEntity: CIRCUIT SETUP ACCEPT uuid={}", uuid);
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { brew_uuid: uuid }),
                    });
                }
                BrewEvent::CircuitSetupReject { uuid, cause } => {
                    tracing::info!("BrewEntity: CIRCUIT SETUP REJECT uuid={} cause={}", uuid, cause);
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject { brew_uuid: uuid, cause }),
                    });
                }
                BrewEvent::CircuitCallAlert { uuid } => {
                    tracing::info!("BrewEntity: CIRCUIT CALL ALERT uuid={}", uuid);
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitAlert { brew_uuid: uuid }),
                    });
                }
                BrewEvent::CircuitConnectRequest { uuid, call } => {
                    let network_call = Self::map_brew_to_network_circuit_call(&call);
                    tracing::info!(
                        "BrewEntity: CIRCUIT CONNECT REQUEST uuid={} src={} dst={} duplex={}",
                        uuid,
                        call.source,
                        call.destination,
                        call.duplex
                    );
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest {
                            brew_uuid: uuid,
                            call: network_call,
                        }),
                    });
                }
                BrewEvent::CircuitConnectConfirm { uuid, grant, permission } => {
                    tracing::info!(
                        "BrewEntity: CIRCUIT CONNECT CONFIRM uuid={} grant={} permission={}",
                        uuid,
                        grant,
                        permission
                    );
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
                            brew_uuid: uuid,
                            grant,
                            permission,
                        }),
                    });
                }
                BrewEvent::CircuitCallRelease { uuid, cause } => {
                    tracing::info!("BrewEntity: CIRCUIT CALL RELEASE uuid={} cause={}", uuid, cause);
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause }),
                    });
                }
                BrewEvent::CircuitDtmf { uuid, length_bits, data } => {
                    tracing::debug!("BrewEntity: CIRCUIT DTMF uuid={} bits={}", uuid, length_bits);
                    // DTMF from network → CMCE (CMCE can forward to local MS via U-INFO if needed)
                    // For now we log it; full downstream DTMF is a future extension.
                    let _ = (uuid, length_bits, data);
                }
            }
        }
    }

    /// Handle RSSI update from MM. Forwards to Brew server if feature_rssi_export is enabled,
    /// applying rate limiting (one update per MS every 5 seconds) to avoid flooding the server.
    fn handle_rssi_update(&mut self, issi: u32, rssi_dbfs: f32) {
        let brew_cfg = self.config.config();
        let Some(ref brew) = brew_cfg.brew else {
            return;
        };
        if !brew.feature_rssi_export {
            return;
        }
        if !self.connected {
            return;
        }

        const RSSI_EXPORT_INTERVAL: Duration = Duration::from_secs(5);

        let now = Instant::now();
        let should_send = match self.rssi_last_sent.get(&issi) {
            None => true,
            Some(last) => now.duration_since(*last) >= RSSI_EXPORT_INTERVAL,
        };

        if should_send {
            self.rssi_last_sent.insert(issi, now);
            send_brew_command(&self.command_sender, BrewCommand::SendRssiUpdate { issi, rssi_dbfs });
            tracing::debug!("Brew: queued RSSI export issi={} rssi={:.1}dBFS", issi, rssi_dbfs);
        }
    }

    fn handle_subscriber_update(&mut self, update: MmSubscriberUpdate) {
        let issi = update.issi;
        let groups = update.groups;
        let issi_routable = super::is_brew_issi_routable(&self.config, issi);

        match update.action {
            BrewSubscriberAction::Register => {
                self.subscriber_groups.entry(issi).or_insert_with(HashSet::new);
                if issi_routable && self.connected {
                    tracing::info!("BrewEntity: subscriber register issi={} → REGISTER", issi);
                    send_brew_command(&self.command_sender, BrewCommand::RegisterSubscriber { issi });
                } else if !issi_routable {
                    tracing::debug!("BrewEntity: subscriber register issi={} (filtered, not sent to Brew)", issi);
                } else {
                    // routable but disconnected — affiliations replayed on reconnect via resync
                    tracing::debug!("BrewEntity: subscriber register issi={} cached until Brew reconnect", issi);
                }
            }
            BrewSubscriberAction::Deregister => {
                let existing_groups: Vec<u32> = self
                    .subscriber_groups
                    .remove(&issi)
                    .map(|g| g.into_iter().collect())
                    .unwrap_or_default();
                let existing_groups = self.brew_routable_groups(existing_groups);
                let had_group_subscription = !existing_groups.is_empty();
                if (issi_routable || had_group_subscription) && self.connected {
                    tracing::info!("BrewEntity: subscriber deregister issi={} → DEAFFILIATE + DEREGISTER", issi);
                    if had_group_subscription {
                        send_brew_command(
                            &self.command_sender,
                            BrewCommand::DeaffiliateGroups {
                                issi,
                                groups: existing_groups,
                            },
                        );
                    }
                    send_brew_command(&self.command_sender, BrewCommand::DeregisterSubscriber { issi });
                } else if !issi_routable {
                    tracing::debug!("BrewEntity: subscriber deregister issi={} (filtered, not sent to Brew)", issi);
                } else {
                    tracing::debug!("BrewEntity: subscriber deregister issi={} cached while Brew disconnected", issi);
                }
            }
            BrewSubscriberAction::Affiliate => {
                let routable_groups = self.brew_routable_groups(groups);
                let entry = self.subscriber_groups.entry(issi).or_insert_with(HashSet::new);
                let mut new_groups = Vec::new();
                for gssi in routable_groups {
                    if entry.insert(gssi) {
                        new_groups.push(gssi);
                    }
                }
                if !new_groups.is_empty() && self.connected {
                    if issi_routable {
                        tracing::info!("BrewEntity: affiliate issi={} → AFFILIATE groups={:?}", issi, new_groups);
                    } else {
                        // Interconnect policy, not an ETSI air-interface rule:
                        // local_ssi_ranges keeps private ISSI calls local, but a
                        // local MS may still subscribe the cell to Brew-routable
                        // talkgroups such as GSSI 91.
                        tracing::info!(
                            "BrewEntity: local ISSI {} group-subscribe → REGISTER + AFFILIATE groups={:?}",
                            issi,
                            new_groups
                        );
                        send_brew_command(&self.command_sender, BrewCommand::RegisterSubscriber { issi });
                    }
                    send_brew_command(&self.command_sender, BrewCommand::AffiliateGroups { issi, groups: new_groups });
                } else if !new_groups.is_empty() {
                    tracing::debug!(
                        "BrewEntity: affiliate issi={} groups={:?} cached until Brew reconnect",
                        issi,
                        new_groups
                    );
                }
            }
            BrewSubscriberAction::Deaffiliate => {
                let mut removed_groups = Vec::new();
                if let Some(entry) = self.subscriber_groups.get_mut(&issi) {
                    for gssi in groups {
                        if entry.remove(&gssi) {
                            removed_groups.push(gssi);
                        }
                    }
                }
                let removed_groups = self.brew_routable_groups(removed_groups);
                if !removed_groups.is_empty() && self.connected {
                    tracing::info!("BrewEntity: deaffiliate issi={} → DEAFFILIATE groups={:?}", issi, removed_groups);
                    send_brew_command(
                        &self.command_sender,
                        BrewCommand::DeaffiliateGroups {
                            issi,
                            groups: removed_groups,
                        },
                    );
                } else if !removed_groups.is_empty() {
                    tracing::debug!(
                        "BrewEntity: deaffiliate issi={} groups={:?} cached while Brew disconnected",
                        issi,
                        removed_groups
                    );
                }
            }
            BrewSubscriberAction::ReleaseIndividualCalls => {
                tracing::debug!(
                    "BrewEntity: ignoring internal individual-call cleanup for issi={} (MM/CMCE local state only)",
                    issi
                );
            }
        }
    }

    fn resync_subscribers(&self) {
        for (issi, groups) in &self.subscriber_groups {
            let gssi_list = self.brew_routable_groups(groups.iter().copied());
            if !super::is_brew_issi_routable(&self.config, *issi) && gssi_list.is_empty() {
                tracing::debug!("BrewEntity: resync skipping issi={} (filtered)", issi);
                continue;
            }
            send_brew_command(&self.command_sender, BrewCommand::RegisterSubscriber { issi: *issi });
            if gssi_list.is_empty() {
                tracing::info!("BrewEntity: resync issi={} — registered, no group affiliations", issi);
            } else {
                tracing::info!(
                    "BrewEntity: resync issi={} — registered, affiliating {} groups: {:?}",
                    issi,
                    gssi_list.len(),
                    gssi_list
                );
                send_brew_command(
                    &self.command_sender,
                    BrewCommand::AffiliateGroups {
                        issi: *issi,
                        groups: gssi_list,
                    },
                );
            }
        }
    }

    fn set_network_connected(&mut self, connected: bool, server_version: u8) {
        self.connected = connected;
        let changed = {
            let mut state = self.config.state_write();
            if state.network_connected != connected {
                state.network_connected = connected;
                tracing::info!("BrewEntity: backhaul {}", if connected { "CONNECTED" } else { "DISCONNECTED" });
                true
            } else {
                false
            }
        };
        if changed {
            if let Some(ref sink) = self.telemetry_sink {
                let _ = sink.send(TelemetryEvent::BrewConnected { connected, server_version });
            }
        }
    }

    /// Emit a brew version upgrade event directly without changing connection state.
    fn emit_brew_version(&self, version: u8) {
        if let Some(ref sink) = self.telemetry_sink {
            let _ = sink.send(TelemetryEvent::BrewConnected {
                connected: true,
                server_version: version,
            });
        }
    }

    /// Handle new group call from Brew, reusing hanging call circuits if available.
    fn handle_group_call_start(&mut self, queue: &mut MessageQueue, uuid: Uuid, source_issi: u32, dest_gssi: u32, priority: u8) {
        // Check if this call is already active (speaker change or repeated GROUP_TX)
        if let Some(call) = self.active_calls.get_mut(&uuid) {
            // Only notify CMCE if the speaker actually changed
            if call.source_issi != source_issi {
                tracing::info!(
                    "BrewEntity: GROUP_TX speaker change on uuid={} new_speaker={} (was {})",
                    uuid,
                    source_issi,
                    call.source_issi
                );
                call.source_issi = source_issi;

                // Flush stale audio from previous speaker immediately.
                // ETSI EN 300 392-2 §14.8.43: when transmission grant changes,
                // the previous speaker's audio must not be forwarded to the new speaker.
                if let Some(jitter) = self.dl_jitter.get_mut(&uuid) {
                    let dropped = jitter.flush();
                    if dropped > 0 {
                        tracing::debug!(
                            "BrewEntity: flushed {} stale frames from jitter on speaker change uuid={}",
                            dropped,
                            uuid
                        );
                    }
                }

                // Forward speaker change to CMCE
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Brew,
                    dest: TetraEntity::Cmce,
                    msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
                        brew_uuid: uuid,
                        source_issi,
                        dest_gssi,
                        priority,
                    }),
                });
            } else {
                // Repeated GROUP_TX with same speaker - this is normal, just log at trace level
                tracing::trace!("BrewEntity: repeated GROUP_TX on uuid={} speaker={}", uuid, source_issi);
            }
            return;
        }

        // Check if there's a hanging call we can reuse
        if let Some(hanging) = self.hanging_calls.remove(&dest_gssi) {
            tracing::info!(
                "BrewEntity: reusing hanging circuit for gssi={} uuid={} (hangtime {:.1}s)",
                dest_gssi,
                uuid,
                hanging.since.elapsed().as_secs_f32()
            );

            // Track the call. Carry over the resources the hanging circuit already
            // had (call_id/ts/usage) as a fallback: when CMCE reuses the circuit it
            // may not emit a fresh NetworkCallReady, and if these stayed None the call
            // could never be re-marked as hanging on its next end — leaking the circuit
            // and breaking reuse on subsequent PTTs. NetworkCallReady, if it does come,
            // will simply overwrite these with identical values.
            let call = ActiveCall {
                uuid,
                call_id: Some(hanging.call_id),
                ts: Some(hanging.ts),
                usage: Some(hanging.usage),
                source_issi,
                dest_gssi,
                frame_count: hanging.frame_count,
            };
            self.active_calls.insert(uuid, call);
            self.dl_jitter
                .entry(uuid)
                .or_insert_with(|| VoiceJitterBuffer::with_initial_latency(self.brew_config.jitter_initial_latency_frames as usize));

            // Forward to CMCE (will reuse circuit automatically)
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Brew,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
                    brew_uuid: uuid,
                    source_issi,
                    dest_gssi,
                    priority,
                }),
            });
            return;
        }

        // New call - track it and request CMCE to allocate and set up
        tracing::info!(
            "BrewEntity: requesting new network call uuid={} src={} gssi={}",
            uuid,
            source_issi,
            dest_gssi
        );

        // Track the call - resources will be set by NetworkCallReady
        let call = ActiveCall {
            uuid,
            call_id: None, // Set by NetworkCallReady
            ts: None,      // Set by NetworkCallReady
            usage: None,   // Set by NetworkCallReady
            source_issi,
            dest_gssi,
            frame_count: 0,
        };
        self.active_calls.insert(uuid, call);
        self.dl_jitter
            .entry(uuid)
            .or_insert_with(|| VoiceJitterBuffer::with_initial_latency(self.brew_config.jitter_initial_latency_frames as usize));

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
                brew_uuid: uuid,
                source_issi,
                dest_gssi,
                priority,
            }),
        });
    }

    /// Handle GROUP_IDLE by forwarding to CMCE and tracking for hangtime reuse
    fn handle_group_call_end(&mut self, queue: &mut MessageQueue, uuid: Uuid, _cause: u8) {
        let Some(call) = self.active_calls.remove(&uuid) else {
            tracing::debug!("BrewEntity: GROUP_IDLE for unknown uuid={}", uuid);
            return;
        };

        // Move jitter buffer to draining instead of dropping it — remaining frames
        // will continue to be played out until the buffer empties naturally.
        if let Some(jitter) = self.dl_jitter.remove(&uuid) {
            if let Some(ts) = call.ts {
                if !jitter.is_empty() {
                    tracing::debug!(
                        "BrewEntity: GROUP_IDLE uuid={} moving {} buffered frames to drain",
                        uuid,
                        jitter.len()
                    );
                    self.draining_jitter.insert(uuid, (ts, jitter, Instant::now()));
                }
            }
        }

        tracing::info!(
            "BrewEntity: group call ended uuid={} call_id={:?} gssi={} frames={}",
            uuid,
            call.call_id,
            call.dest_gssi,
            call.frame_count
        );

        // Request CMCE to end the call
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid: uuid }),
        });

        // Track as hanging for potential reuse (only if resources were allocated)
        if let (Some(call_id), Some(ts), Some(usage)) = (call.call_id, call.ts, call.usage) {
            // If a hanging call already exists for this GSSI (e.g. rapid PTT cycling on
            // the same group), the previous hanging entry's UUID would be silently
            // overwritten and leak — its jitter/active state never cleaned and TetraPack
            // left with a dangling session reference. Drop the stale one explicitly first.
            if let Some(stale) = self.hanging_calls.remove(&call.dest_gssi) {
                if stale.uuid != uuid {
                    tracing::debug!(
                        "BrewEntity: replacing stale hanging call gssi={} old_uuid={} new_uuid={}",
                        call.dest_gssi,
                        stale.uuid,
                        uuid
                    );
                    self.dl_jitter.remove(&stale.uuid);
                    self.draining_jitter.remove(&stale.uuid);
                }
            }
            self.hanging_calls.insert(
                call.dest_gssi,
                HangingCall {
                    uuid,
                    call_id,
                    ts,
                    usage,
                    source_issi: call.source_issi,
                    dest_gssi: call.dest_gssi,
                    frame_count: call.frame_count,
                    since: Instant::now(),
                },
            );
        }
    }

    /// Clean up expired hanging call tracking hints (CMCE already released circuits)
    fn expire_hanging_calls(&mut self, _queue: &mut MessageQueue) {
        let hangtime = Duration::from_secs(self.config.config().cell.hangtime_secs as u64);
        let expired: Vec<u32> = self
            .hanging_calls
            .iter()
            .filter(|(_, h)| h.since.elapsed() >= hangtime)
            .map(|(gssi, _)| *gssi)
            .collect();

        for gssi in expired {
            if let Some(hanging) = self.hanging_calls.remove(&gssi) {
                tracing::debug!("BrewEntity: hanging call expired gssi={} uuid={} (no reuse)", gssi, hanging.uuid);
                // No action needed - CMCE already released the circuit
            }
        }
    }

    /// Handle a voice frame from Brew — inject into the downlink
    fn handle_voice_frame(&mut self, uuid: Uuid, _length_bits: u16, data: Vec<u8>) {
        let Some(call) = self.active_calls.get_mut(&uuid) else {
            // Voice frame for unknown call — might arrive before GROUP_TX or after GROUP_IDLE
            tracing::trace!("BrewEntity: voice frame for unknown uuid={} ({} bytes)", uuid, data.len());
            return;
        };

        call.frame_count += 1;

        // Check if resources have been allocated yet
        let Some(ts) = call.ts else {
            // Audio arrived before NetworkCallReady - drop it
            if call.frame_count == 1 {
                tracing::debug!(
                    "BrewEntity: voice frame arrived before resources allocated, uuid={}, dropping",
                    uuid
                );
            }
            return;
        };

        // Log first voice frame per call
        if call.frame_count == 1 {
            tracing::info!(
                "BrewEntity: voice frame #{} uuid={} len={} bytes ts={}",
                call.frame_count,
                uuid,
                data.len(),
                ts
            );
        }

        // STE format: byte 0 = header (control bits), bytes 1-35 = 274 ACELP bits for TCH/S.
        // Strip the STE header and pass only the ACELP payload.
        if data.len() < 36 {
            tracing::warn!("BrewEntity: voice frame too short ({} bytes, expected 36 STE bytes)", data.len());
            return;
        }
        let acelp_data = data[1..].to_vec(); // 35 bytes = 280 bits, of which 274 are ACELP

        self.dl_jitter
            .entry(uuid)
            .or_insert_with(|| VoiceJitterBuffer::with_initial_latency(self.brew_config.jitter_initial_latency_frames as usize))
            .push(acelp_data);
    }

    fn drain_jitter_playout(&mut self, queue: &mut MessageQueue) {
        if self.dltime.f == 18 {
            return;
        }

        let mut to_send: Vec<(u8, Uuid, usize, JitterFrame)> = Vec::new();

        for (uuid, call) in &self.active_calls {
            let Some(ts) = call.ts else {
                continue;
            };
            if ts != self.dltime.t {
                continue;
            }
            let Some(jitter) = self.dl_jitter.get_mut(uuid) else {
                continue;
            };
            jitter.maybe_warn_unhealthy(*uuid);
            if let Some(frame) = jitter.pop_ready() {
                to_send.push((ts, *uuid, jitter.target_frames(), frame));
            }
        }

        // Also drain buffers from calls that ended (GROUP_IDLE) but still have frames buffered.
        // A buffer is removed when it empties naturally, OR when it has been draining for
        // far longer than any reasonable playout (its timeslot stopped being scheduled) —
        // the latter guards against slow leaks under repeated PTT cycling.
        const DRAIN_REAP_AFTER: Duration = Duration::from_secs(5);
        let finished: Vec<Uuid> = self
            .draining_jitter
            .iter_mut()
            .filter_map(|(uuid, (ts, jitter, started))| {
                if started.elapsed() >= DRAIN_REAP_AFTER {
                    // Stale: never finished draining within a sane window — reap it.
                    return Some(*uuid);
                }
                if *ts != self.dltime.t {
                    return None;
                }
                match jitter.pop_drain() {
                    Some(frame) => {
                        to_send.push((*ts, *uuid, 0, frame));
                        None
                    }
                    None => Some(*uuid),
                }
            })
            .collect();
        for uuid in finished {
            tracing::debug!("BrewEntity: drain complete (or reaped) for uuid={}", uuid);
            self.draining_jitter.remove(&uuid);
        }

        for (ts, uuid, target_frames, frame) in to_send {
            tracing::trace!(
                "BrewEntity: playout uuid={} ts={} rx_seq={} age_ms={} target_frames={}",
                uuid,
                ts,
                frame.rx_seq,
                frame.rx_at.elapsed().as_millis(),
                target_frames
            );
            queue.push_back(SapMsg {
                sap: Sap::TmdSap,
                src: TetraEntity::Brew,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::TmdCircuitDataReq(TmdCircuitDataReq {
                    ts,
                    data: frame.acelp_data,
                    raw_tch_s_block: None,
                }),
            });
        }
    }

    /// Release all active calls (on disconnect)
    fn release_all_calls(&mut self, queue: &mut MessageQueue) {
        // Request CMCE to end all active network calls
        let calls: Vec<(Uuid, ActiveCall)> = self.active_calls.drain().collect();
        for (uuid, _) in calls {
            self.dl_jitter.remove(&uuid);
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Brew,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid: uuid }),
            });
        }

        // Clear hanging call tracking
        self.hanging_calls.clear();
        self.dl_jitter.clear();
        self.draining_jitter.clear();

        // Also clear uplink forwarding state. Without this, an MS that was transmitting
        // (PTT held) at the moment the backhaul dropped would leave a stale ul_forwarded
        // entry referencing a UUID the server has forgotten; after reconnect its UL voice
        // frames would be sent against that dead UUID. A fresh PTT will re-create the
        // entry cleanly via handle_local_call_start.
        if !self.ul_forwarded.is_empty() {
            tracing::debug!(
                "BrewEntity: clearing {} stale ul_forwarded entries on release",
                self.ul_forwarded.len()
            );
            self.ul_forwarded.clear();
        }
    }

    /// Handle NetworkCallReady response from CMCE
    fn rx_network_call_ready(&mut self, queue: &mut MessageQueue, brew_uuid: Uuid, call_id: u16, ts: u8, usage: u8) {
        tracing::info!(
            "BrewEntity: network call ready uuid={} call_id={} ts={} usage={}",
            brew_uuid,
            call_id,
            ts,
            usage
        );

        // Update active call with CMCE-allocated resources
        if let Some(call) = self.active_calls.get_mut(&brew_uuid) {
            call.call_id = Some(call_id);
            call.ts = Some(ts);
            call.usage = Some(usage);
        } else {
            // Race: CMCE finished allocating a circuit (call_id/ts/usage) for a call that
            // Brew has already torn down — e.g. a GROUP_IDLE or disconnect arrived between
            // our NetworkCallStart and this NetworkCallReady. If we just drop this on the
            // floor, CMCE keeps the circuit allocated forever (orphaned ts/usage that no
            // GROUP_IDLE will ever release). Tell CMCE to release it so the timeslot is
            // freed for the next call.
            tracing::warn!(
                "BrewEntity: NetworkCallReady for unknown uuid={} (call already gone) — releasing orphaned circuit call_id={} ts={}",
                brew_uuid,
                call_id,
                ts
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Brew,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
            });
        }
    }

    /// Drop an active circuit call state. Returns true if there was an active circuit.
    /// Flushes the jitter buffer immediately to prevent audio from being sent to a
    /// closed circuit (EN 300 392-2 §14.9: resources must be released immediately on disconnect).
    fn drop_network_circuit(&mut self, brew_uuid: Uuid) -> bool {
        // Flush and remove jitter buffer immediately — prevents DL voice frames
        // from being sent to UMAC after the circuit is already closed.
        let had_jitter = self.dl_jitter.remove(&brew_uuid).is_some();
        self.draining_jitter.remove(&brew_uuid);
        let ts_to_remove: Vec<u8> = self
            .ul_forwarded
            .iter()
            .filter_map(|(&ts, fwd)| {
                if fwd.uuid == brew_uuid && fwd.kind == UlForwardKind::Circuit {
                    Some(ts)
                } else {
                    None
                }
            })
            .collect();
        let had_ts = !ts_to_remove.is_empty();
        for ts in ts_to_remove {
            self.ul_forwarded.remove(&ts);
        }
        // Remove from active_calls — circuit calls are registered there by NetworkCircuitMediaReady
        // so that DL audio from TetraPack reaches the MS. Clean up here to avoid stale entries.
        self.active_calls.remove(&brew_uuid);
        if had_jitter || had_ts {
            tracing::info!("BrewEntity: dropped circuit uuid={}", brew_uuid);
        } else {
            tracing::debug!("BrewEntity: drop_network_circuit for unknown uuid={}", brew_uuid);
        }
        had_jitter || had_ts
    }

    fn drop_network_call(&mut self, brew_uuid: Uuid) {
        if let Some(call) = self.active_calls.remove(&brew_uuid) {
            tracing::info!(
                "BrewEntity: dropping network call uuid={} gssi={} (CMCE request)",
                brew_uuid,
                call.dest_gssi
            );
            self.dl_jitter.remove(&brew_uuid);
            self.hanging_calls.remove(&call.dest_gssi);
            return;
        }

        let hanging_gssi = self
            .hanging_calls
            .iter()
            .find_map(|(gssi, hanging)| if hanging.uuid == brew_uuid { Some(*gssi) } else { None });
        if let Some(gssi) = hanging_gssi {
            tracing::info!("BrewEntity: dropping hanging call uuid={} gssi={} (CMCE request)", brew_uuid, gssi);
            self.hanging_calls.remove(&gssi);
        } else {
            tracing::debug!("BrewEntity: drop requested for unknown uuid={}", brew_uuid);
        }
    }
}

// ─── TetraEntityTrait implementation ──────────────────────────────

impl TetraEntityTrait for BrewEntity {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Brew
    }

    fn set_config(&mut self, config: SharedConfig) {
        self.config = config;
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        self.dltime = ts;
        // Process all pending events from the worker thread
        self.process_events(queue);
        self.drain_pending_sds_reports();
        // Feed one buffered frame at each traffic playout opportunity.
        self.drain_jitter_playout(queue);
        // Expire hanging calls that have exceeded hangtime
        self.expire_hanging_calls(queue);
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        match message.msg {
            // UL voice from UMAC — forward to TetraPack if this timeslot is being forwarded
            SapMsgInner::TmdCircuitDataInd(prim) => {
                if prim.raw_tch_s_block.is_some() {
                    tracing::trace!(
                        "BrewEntity: ignoring raw TCH/S half-slot on ts={} because Brew expects ACELP frames",
                        prim.ts
                    );
                } else {
                    self.handle_ul_voice(prim.ts, prim.data);
                }
            }
            // Floor-control and call lifecycle notifications from CMCE
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id,
                source_issi,
                dest_gssi,
                ts,
            }) => {
                self.handle_local_call_start(call_id, source_issi, dest_gssi, ts);
            }
            SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id, ts }) => {
                self.handle_local_call_tx_stopped(call_id, ts);
            }
            SapMsgInner::CmceCallControl(CallControl::CallEnded { call_id, ts }) => {
                self.handle_local_call_end(call_id, ts);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }) => {
                self.drop_network_call(brew_uuid);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid,
                call_id,
                ts,
                usage,
            }) => {
                self.rx_network_call_ready(queue, brew_uuid, call_id, ts, usage);
            }
            // UlInactivityTimeout is UMAC→CMCE only; Brew handles FloorReleased instead
            SapMsgInner::CmceCallControl(CallControl::UlInactivityTimeout { .. }) => {}

            // ── Circuit / individual call outbound signals (CMCE → Brew → TetraPack) ──
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, call }) => {
                if !self.connected {
                    tracing::debug!("BrewEntity: not connected, dropping NetworkCircuitSetupRequest uuid={}", brew_uuid);
                    return;
                }
                let wire_call = Self::map_network_to_brew_circuit_call(&call);
                send_brew_command(
                    &self.command_sender,
                    BrewCommand::SendSetupRequest {
                        uuid: brew_uuid,
                        call: wire_call,
                    },
                );
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { brew_uuid }) => {
                if self.connected {
                    send_brew_command(&self.command_sender, BrewCommand::SendSetupAccept { uuid: brew_uuid });
                }
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject { brew_uuid, cause }) => {
                if self.connected {
                    send_brew_command(&self.command_sender, BrewCommand::SendSetupReject { uuid: brew_uuid, cause });
                }
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitAlert { brew_uuid }) => {
                if self.connected {
                    send_brew_command(&self.command_sender, BrewCommand::SendCallAlert { uuid: brew_uuid });
                }
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest { brew_uuid, call }) => {
                if !self.connected {
                    tracing::debug!(
                        "BrewEntity: not connected, dropping NetworkCircuitConnectRequest uuid={}",
                        brew_uuid
                    );
                    return;
                }
                let wire_call = Self::map_network_to_brew_circuit_call(&call);
                send_brew_command(
                    &self.command_sender,
                    BrewCommand::SendConnectRequest {
                        uuid: brew_uuid,
                        call: wire_call,
                    },
                );
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
                brew_uuid,
                grant,
                permission,
            }) => {
                if self.connected {
                    send_brew_command(
                        &self.command_sender,
                        BrewCommand::SendConnectConfirm {
                            uuid: brew_uuid,
                            grant,
                            permission,
                        },
                    );
                }
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady { brew_uuid, call_id, ts }) => {
                tracing::info!("BrewEntity: circuit media ready uuid={} call_id={} ts={}", brew_uuid, call_id, ts);
                // Register UL forwarding: voice on `ts` gets sent to TetraPack.
                self.ul_forwarded.insert(
                    ts,
                    UlForwardedCall {
                        uuid: brew_uuid,
                        call_id,
                        source_issi: 0,
                        dest_gssi: 0,
                        kind: UlForwardKind::Circuit,
                        frame_count: 0,
                    },
                );
                // Register in active_calls with ts already known so that DL voice frames received
                // from TetraPack (handle_voice_frame + drain_jitter_playout) are delivered to the MS.
                // Without this entry handle_voice_frame silently drops all incoming DL audio because
                // it looks up the uuid in active_calls and finds nothing.
                self.active_calls.entry(brew_uuid).or_insert_with(|| ActiveCall {
                    uuid: brew_uuid,
                    call_id: Some(call_id),
                    ts: Some(ts),
                    usage: None,
                    source_issi: 0,
                    dest_gssi: 0,
                    frame_count: 0,
                });
                self.dl_jitter
                    .entry(brew_uuid)
                    .or_insert_with(|| VoiceJitterBuffer::with_initial_latency(self.brew_config.jitter_initial_latency_frames as usize));
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitDtmf {
                brew_uuid,
                length_bits,
                data,
            }) => {
                if self.connected {
                    send_brew_command(
                        &self.command_sender,
                        BrewCommand::SendDtmf {
                            uuid: brew_uuid,
                            length_bits,
                            data,
                        },
                    );
                }
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { brew_uuid, cause }) => {
                let was_active = self.drop_network_circuit(brew_uuid);
                if was_active && self.connected {
                    send_brew_command(&self.command_sender, BrewCommand::SendCallRelease { uuid: brew_uuid, cause });
                }
            }
            SapMsgInner::MmSubscriberUpdate(update) => {
                self.handle_subscriber_update(update);
            }
            SapMsgInner::MsRssiUpdate { issi, rssi_dbfs } => {
                self.handle_rssi_update(issi, rssi_dbfs);
            }
            SapMsgInner::CmceSdsData(sds) => {
                self.handle_sds_send(sds);
            }
            _ => {
                tracing::debug!("BrewEntity: unexpected rx_prim from {:?} on {:?}", message.src, message.sap);
            }
        }
    }
}

// ─── UL call forwarding to TetraPack ──────────────────────────────

impl BrewEntity {
    /// Map Brew wire BrewCircularCall to SAP NetworkCircuitCall (CMCE-facing).
    fn map_brew_to_network_circuit_call(call: &super::protocol::BrewCircularCall) -> NetworkCircuitCall {
        NetworkCircuitCall {
            source_issi: call.source,
            destination: call.destination,
            number: call.number.clone(),
            priority: call.priority,
            service: call.service,
            mode: call.mode,
            duplex: call.duplex,
            method: call.method,
            communication: call.communication,
            grant: call.grant,
            permission: call.permission,
            timeout: call.timeout,
            ownership: call.ownership,
            queued: call.queued,
        }
    }

    /// Map SAP NetworkCircuitCall to Brew wire BrewCircularCall (network-facing).
    fn map_network_to_brew_circuit_call(call: &NetworkCircuitCall) -> super::protocol::BrewCircularCall {
        super::protocol::BrewCircularCall {
            source: call.source_issi,
            destination: call.destination,
            number: call.number.clone(),
            priority: call.priority,
            service: call.service,
            mode: call.mode,
            duplex: call.duplex,
            method: call.method,
            communication: call.communication,
            grant: call.grant,
            permission: call.permission,
            timeout: call.timeout,
            ownership: call.ownership,
            queued: call.queued,
            mnemonic: None,
        }
    }

    /// Handle notification that a local UL group call has started.
    /// If the group is subscribed (in config.groups), start forwarding to TetraPack.
    fn handle_local_call_start(&mut self, call_id: u16, source_issi: u32, dest_gssi: u32, ts: u8) {
        if !self.connected {
            tracing::trace!("BrewEntity: not connected, ignoring local call start");
            return;
        }

        if let Some(fwd) = self.ul_forwarded.get_mut(&ts) {
            if fwd.kind == UlForwardKind::Circuit {
                if fwd.call_id != call_id {
                    tracing::warn!(
                        "BrewEntity: circuit floor grant call_id mismatch on ts={}: expected {} got {}",
                        ts,
                        fwd.call_id,
                        call_id
                    );
                }
                fwd.call_id = call_id;
                fwd.source_issi = source_issi;
                fwd.dest_gssi = dest_gssi;
                tracing::debug!(
                    "BrewEntity: circuit floor grant on ts={} uuid={} src={} dst={}, preserving circuit media session",
                    ts,
                    fwd.uuid,
                    source_issi,
                    dest_gssi
                );
                return;
            }
        }

        if !super::is_brew_gssi_routable(&self.config, dest_gssi) {
            tracing::debug!("BrewEntity: suppressing GROUP_TX for gssi={} (not Brew-routable)", dest_gssi);
            return;
        }

        // If we're already forwarding on this timeslot, treat as a talker change/update
        if let Some(fwd) = self.ul_forwarded.get_mut(&ts) {
            if fwd.call_id != call_id || fwd.dest_gssi != dest_gssi {
                tracing::warn!(
                    "BrewEntity: updating forwarded call on ts={} (was call_id={} gssi={}) -> (call_id={} gssi={})",
                    ts,
                    fwd.call_id,
                    fwd.dest_gssi,
                    call_id,
                    dest_gssi
                );
            }

            fwd.call_id = call_id;
            fwd.source_issi = source_issi;
            fwd.dest_gssi = dest_gssi;
            fwd.frame_count = 0;

            // Send GROUP_TX update for the new talker
            send_brew_command(
                &self.command_sender,
                BrewCommand::SendGroupTx {
                    uuid: fwd.uuid,
                    source_issi,
                    dest_gssi,
                    priority: 0,
                    service: 0, // TETRA encoded speech
                },
            );
            return;
        }

        // Generate a UUID for this Brew session
        let uuid = Uuid::new_v4();
        tracing::info!(
            "BrewEntity: forwarding local call to TetraPack: call_id={} src={} gssi={} ts={} uuid={}",
            call_id,
            source_issi,
            dest_gssi,
            ts,
            uuid
        );

        // Send GROUP_TX to TetraPack
        send_brew_command(
            &self.command_sender,
            BrewCommand::SendGroupTx {
                uuid,
                source_issi,
                dest_gssi,
                priority: 0,
                service: 0, // TETRA encoded speech
            },
        );

        // Track this forwarded call
        self.ul_forwarded.insert(
            ts,
            UlForwardedCall {
                uuid,
                call_id,
                source_issi,
                dest_gssi,
                frame_count: 0,
                kind: UlForwardKind::Group,
            },
        );
    }

    /// Handle notification that a local UL call has ended.
    fn handle_local_call_tx_stopped(&mut self, call_id: u16, ts: u8) {
        if let Some(fwd) = self.ul_forwarded.get(&ts) {
            if fwd.kind == UlForwardKind::Circuit {
                if fwd.call_id != call_id {
                    tracing::warn!(
                        "BrewEntity: circuit floor release call_id mismatch on ts={}: expected {} got {}",
                        ts,
                        fwd.call_id,
                        call_id
                    );
                }
                tracing::debug!(
                    "BrewEntity: circuit floor release on ts={} uuid={} frames={}, preserving circuit media session",
                    ts,
                    fwd.uuid,
                    fwd.frame_count
                );
                return;
            }
        }

        if let Some(fwd) = self.ul_forwarded.remove(&ts) {
            if fwd.call_id != call_id {
                tracing::warn!(
                    "BrewEntity: call_id mismatch on ts={}: expected {} got {}",
                    ts,
                    fwd.call_id,
                    call_id
                );
            }
            tracing::info!(
                "BrewEntity: local call transmission stopped, sending GROUP_IDLE to TetraPack: uuid={} frames={}",
                fwd.uuid,
                fwd.frame_count
            );
            send_brew_command(
                &self.command_sender,
                BrewCommand::SendGroupIdle {
                    uuid: fwd.uuid,
                    cause: 0, // Normal release
                },
            );
        }
    }

    fn handle_local_call_end(&mut self, call_id: u16, ts: u8) {
        if let Some(fwd) = self.ul_forwarded.get(&ts) {
            if fwd.kind == UlForwardKind::Circuit {
                if fwd.call_id != call_id {
                    tracing::warn!(
                        "BrewEntity: circuit call end call_id mismatch on ts={}: expected {} got {}",
                        ts,
                        fwd.call_id,
                        call_id
                    );
                }
                tracing::debug!(
                    "BrewEntity: circuit call end on ts={} uuid={} ignored; NetworkCircuitRelease owns teardown",
                    ts,
                    fwd.uuid
                );
                return;
            }
        }

        // Check if ul_forwarded entry still exists (might have been removed by handle_local_call_tx_stopped)
        if let Some(fwd) = self.ul_forwarded.remove(&ts) {
            if fwd.call_id != call_id {
                tracing::warn!(
                    "BrewEntity: call_id mismatch on ts={}: expected {} got {}",
                    ts,
                    fwd.call_id,
                    call_id
                );
            }
            tracing::debug!(
                "BrewEntity: local call ended (already sent GROUP_IDLE during tx_stopped): uuid={} frames={}",
                fwd.uuid,
                fwd.frame_count
            );
        } else {
            tracing::debug!("BrewEntity: local call ended on ts={} (already cleaned up during tx_stopped)", ts);
        }
    }

    /// Handle UL voice data from UMAC. If the timeslot is being forwarded to TetraPack,
    /// convert to STE format and send.
    fn handle_ul_voice(&mut self, ts: u8, acelp_bits: Vec<u8>) {
        let Some(fwd) = self.ul_forwarded.get_mut(&ts) else {
            return; // Not forwarded to TetraPack
        };

        fwd.frame_count += 1;

        let Some(ste_data) = Self::build_brew_ste_voice_frame(&acelp_bits) else {
            tracing::warn!("BrewEntity: UL voice too short: {} bits", acelp_bits.len());
            return;
        };

        send_brew_command(
            &self.command_sender,
            BrewCommand::SendVoiceFrame {
                uuid: fwd.uuid,
                length_bits: BREW_STE_FRAME_BITS,
                data: ste_data,
            },
        );
    }

    fn build_brew_ste_voice_frame(acelp_bits: &[u8]) -> Option<Vec<u8>> {
        if acelp_bits.len() == BREW_STE_FRAME_BYTES {
            let mut ste = acelp_bits.to_vec();
            ste[0] = (ste[0] | BREW_STE_HEADER_NORMAL_SPEECH) & 0xfc;
            ste[BREW_STE_FRAME_BYTES - 1] |= BREW_STE_UNUSED_TAIL_MASK;
            return Some(ste);
        }

        let packed_payload = if acelp_bits.len() == BREW_STE_FRAME_BYTES - 1 {
            acelp_bits.to_vec()
        } else if acelp_bits.len() >= TETRA_ACELP_TCH_S_BITS {
            let mut packed = Vec::with_capacity(BREW_STE_FRAME_BYTES - 1);
            for chunk_idx in 0..(BREW_STE_FRAME_BYTES - 1) {
                let mut byte = 0u8;
                for bit in 0..8 {
                    let bit_idx = chunk_idx * 8 + bit;
                    if bit_idx < TETRA_ACELP_TCH_S_BITS {
                        byte |= (acelp_bits[bit_idx] & 1) << (7 - bit);
                    }
                }
                packed.push(byte);
            }
            packed
        } else {
            return None;
        };

        let mut ste = Vec::with_capacity(BREW_STE_FRAME_BYTES);
        ste.push(BREW_STE_HEADER_NORMAL_SPEECH);
        ste.extend_from_slice(&packed_payload);
        ste[BREW_STE_FRAME_BYTES - 1] |= BREW_STE_UNUSED_TAIL_MASK;
        Some(ste)
    }
}

// ─── SDS handling ─────────────────────────────────────────────────

const SDS_TYPE4_MAX_BITS: u16 = 2047;
const SDS_TYPE4_MIN_BITS: u16 = 8;
const AIR_INTERFACE_MAX_SSI: u32 = 0x00FF_FFFF;
const SDS_TL_TRANSFER_NO_REPORT_REQ: u8 = 0x00;
const SDS_TL_TRANSFER_REPORT_REQ_MASK: u8 = 0x0C;
const BREW_SDS_REPORT_SUCCESS: u8 = 0;
const BREW_SDS_REPORT_FAILED: u8 = 1;

impl BrewEntity {
    fn valid_air_interface_ssi(ssi: u32) -> bool {
        ssi <= AIR_INTERFACE_MAX_SSI
    }

    fn sds_type4_payload_can_serialize(length_bits: u16, data: &[u8]) -> bool {
        length_bits >= SDS_TYPE4_MIN_BITS
            && length_bits <= SDS_TYPE4_MAX_BITS
            && SdsUserData::canonical_type4_bytes(length_bits, data).is_some()
    }

    fn is_sds_tl_transport_protocol_id(protocol_id: u8) -> bool {
        // EN 300 392-2 clause 29.4.1: protocol identifiers 0x80..=0xFE use
        // the SDS-TL transport PDUs. Lower PIDs have protocol-specific payloads
        // outside SDS-TL transport, so their second octet is not a report flag.
        (0x80..=0xFE).contains(&protocol_id)
    }

    fn clear_sds_tl_delivery_report_request(data: &mut [u8]) {
        if data.len() >= 2 && Self::is_sds_tl_transport_protocol_id(data[0]) && data[1] & 0xF0 == SDS_TL_TRANSFER_NO_REPORT_REQ {
            data[1] &= !SDS_TL_TRANSFER_REPORT_REQ_MASK;
        }
    }

    fn sds_report_status_for_reporter(reporter: &TxReporter) -> Option<u8> {
        if !reporter.is_in_final_state() {
            return None;
        }

        match reporter.get_state() {
            TxState::Transmitted | TxState::Acknowledged => Some(BREW_SDS_REPORT_SUCCESS),
            TxState::Discarded | TxState::Lost => Some(BREW_SDS_REPORT_FAILED),
            TxState::Pending => None,
        }
    }

    fn drain_pending_sds_reports(&mut self) {
        let ready: Vec<(Uuid, u8)> = self
            .pending_sds_reports
            .iter()
            .filter_map(|(&uuid, pending)| Self::sds_report_status_for_reporter(&pending.reporter).map(|status| (uuid, status)))
            .collect();

        for (uuid, status) in ready {
            self.pending_sds_reports.remove(&uuid);
            send_brew_command(&self.command_sender, BrewCommand::SendSdsReport { uuid, status });
            tracing::info!(
                "BrewEntity: SDS_REPORT uuid={} status={} -> Brew after air-interface result",
                uuid,
                status
            );
        }
    }

    /// Handle incoming SDS transfer from Brew (network → local MS)
    fn handle_sds_transfer(
        &mut self,
        queue: &mut MessageQueue,
        uuid: Uuid,
        source: u32,
        destination: u32,
        data: Vec<u8>,
        length_bits: u16,
    ) {
        tracing::info!(
            "BrewEntity: SDS transfer uuid={} src={} dst={} {} bytes",
            uuid,
            source,
            destination,
            data.len()
        );

        if !Self::valid_air_interface_ssi(source) || !Self::valid_air_interface_ssi(destination) {
            tracing::warn!(
                "BrewEntity: SDS uuid={} has invalid 24-bit SSI source={} destination={}, dropping without success report",
                uuid,
                source,
                destination
            );
            return;
        }

        // Forward and acknowledge only if the destination can be delivered on this air
        // interface. EN 300 392-2 clause 13.2 includes mobile-terminated
        // individual and group SDS; Brew carries only the 24-bit SSI, so CMCE
        // resolves a non-ISSI destination with local group members as GSSI.
        let broadcast_dest = destination == AIR_INTERFACE_MAX_SSI;
        let dest_ssi_type = if broadcast_dest {
            Some(tetra_core::SsiType::Gssi)
        } else {
            let state = self.config.state_read();
            let is_local_issi = state.subscribers.is_registered(destination);
            let is_local_group = state.subscribers.has_group_members(destination);
            if is_local_issi && is_local_group {
                tracing::warn!(
                    "BrewEntity: SDS dest SSI {} is both local ISSI and local GSSI, dropping ambiguous destination uuid={}",
                    destination,
                    uuid
                );
                None
            } else if is_local_issi {
                Some(tetra_core::SsiType::Issi)
            } else if is_local_group {
                Some(tetra_core::SsiType::Gssi)
            } else {
                None
            }
        };
        let Some(dest_ssi_type) = dest_ssi_type else {
            tracing::warn!(
                "BrewEntity: SDS dest SSI {} is not a local ISSI or GSSI, dropping (no report sent) uuid={}",
                destination,
                uuid
            );
            return;
        };

        if !Self::sds_type4_payload_can_serialize(length_bits, &data) {
            tracing::warn!(
                "BrewEntity: SDS uuid={} has invalid Type4 length_bits={} for {} byte(s), dropping without success report",
                uuid,
                length_bits,
                data.len()
            );
            return;
        }
        let Some(mut data) = SdsUserData::canonical_type4_bytes(length_bits, &data) else {
            tracing::warn!(
                "BrewEntity: SDS uuid={} has inconsistent Type4 length_bits={} for {} byte(s), dropping without success report",
                uuid,
                length_bits,
                data.len()
            );
            return;
        };

        let air_source = if broadcast_dest {
            // EN 300 392-2 29.3.3.8.2 says system broadcast messages to
            // 0xFFFFFF should also indicate the all-ones broadcast source and
            // request no delivery report.
            Self::clear_sds_tl_delivery_report_request(&mut data);
            AIR_INTERFACE_MAX_SSI
        } else {
            source
        };

        // Brew protocol always delivers SDS as variable-length (Type 4). This means the
        // downlink D-SDS-DATA will use SDTI=3, even if the original uplink was a 16-bit
        // pre-coded status (SDTI=0 / Type 1). This is a Brew protocol constraint.
        let user_defined_data = SdsUserData::Type4(length_bits, data);
        let tx_reporter = match dest_ssi_type {
            tetra_core::SsiType::Issi => TxReporter::new(),
            _ => TxReporter::new_unacked(),
        };

        // Forward to CMCE SDS subentity for downlink delivery
        // Set dltime to next ts1 to ensure it gets sent on MCCH
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceSdsData(CmceSdsData {
                source_issi: air_source,
                dest_issi: destination,
                dest_ssi_type: Some(dest_ssi_type),
                user_defined_data,
                tx_reporter: Some(tx_reporter.clone()),
            }),
        });

        // EN 300 392-2 clause 13.2 defines the SDS service; for ISSI delivery the
        // L2 path uses an acknowledged basic link and for GSSI it uses unacknowledged
        // group transmission. Brew status 0 is therefore sent only once the local
        // air-interface reporter reaches its terminal state. For GSSI that means
        // accepted/transmitted on air, not per-recipient delivery.
        self.pending_sds_reports.insert(uuid, PendingSdsReport { reporter: tx_reporter });
    }

    /// Handle outgoing SDS from CMCE → Brew (local MS → network)
    fn handle_sds_send(&self, sds: CmceSdsData) {
        if !self.connected {
            tracing::warn!(
                "BrewEntity: not connected, dropping outgoing SDS {} -> {}",
                sds.source_issi,
                sds.dest_issi
            );
            return;
        }

        let uuid = Uuid::new_v4();
        tracing::info!(
            "BrewEntity: sending SDS uuid={} src={} dst={} type={} {} bits",
            uuid,
            sds.source_issi,
            sds.dest_issi,
            sds.user_defined_data.type_identifier(),
            sds.user_defined_data.length_bits()
        );

        send_brew_command(
            &self.command_sender,
            BrewCommand::SendSds {
                uuid,
                source: sds.source_issi,
                destination: sds.dest_issi,
                data: sds.user_defined_data.to_arr(),
                length_bits: sds.user_defined_data.length_bits(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use crossbeam_channel::{Receiver, bounded};
    use tetra_config::bluestation::{SharedConfig, from_toml_str};
    use tetra_core::{TdmaTime, TxReporter};
    use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};

    use super::{
        BREW_COMMAND_CHANNEL_CAPACITY, BREW_EVENT_CHANNEL_CAPACITY, BREW_STE_FRAME_BITS, BREW_STE_FRAME_BYTES,
        BREW_STE_HEADER_NORMAL_SPEECH, BREW_STE_UNUSED_TAIL_MASK, BrewEntity,
    };
    use crate::net_brew::worker::{BrewCommand, BrewEvent};

    fn brew_test_config() -> SharedConfig {
        let toml = format!(
            "{}\n\n[brew]\nhost = \"core.tetrapack.online\"\nport = 443\ntls = true\nusername = 226008230\npassword = \"test\"\n",
            include_str!("../../../../example_config/config.toml")
        );
        let cfg = from_toml_str(&toml).expect("brew test config parses");
        SharedConfig::from_parts(cfg, None)
    }

    fn brew_entity_without_worker() -> (BrewEntity, Receiver<BrewCommand>) {
        let config = brew_test_config();
        let (_event_sender, event_receiver) = bounded::<BrewEvent>(BREW_EVENT_CHANNEL_CAPACITY);
        let (command_sender, command_receiver) = bounded::<BrewCommand>(BREW_COMMAND_CHANNEL_CAPACITY);
        let brew_config = config.config().brew.clone().expect("brew config");

        (
            BrewEntity {
                config,
                brew_config,
                dltime: TdmaTime::default(),
                event_receiver,
                command_sender,
                active_calls: HashMap::new(),
                dl_jitter: HashMap::new(),
                draining_jitter: HashMap::new(),
                hanging_calls: HashMap::new(),
                ul_forwarded: HashMap::new(),
                subscriber_groups: HashMap::new(),
                connected: true,
                telemetry_sink: None,
                rssi_last_sent: HashMap::new(),
                pending_sds_reports: HashMap::new(),
                worker_handle: None,
            },
            command_receiver,
        )
    }

    #[test]
    fn brew_command_channel_is_bounded_and_non_blocking_on_overflow() {
        let (entity, rx) = brew_entity_without_worker();

        for idx in 0..(BREW_COMMAND_CHANNEL_CAPACITY + 8) {
            let _ = entity.command_sender.try_send(BrewCommand::RegisterSubscriber { issi: idx as u32 });
        }

        let mut received = 0usize;
        while rx.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(received, BREW_COMMAND_CHANNEL_CAPACITY);
    }

    fn subscriber_update(issi: u32, groups: Vec<u32>, action: BrewSubscriberAction) -> MmSubscriberUpdate {
        MmSubscriberUpdate { issi, groups, action }
    }

    fn assert_register(cmd: BrewCommand, expected_issi: u32) {
        match cmd {
            BrewCommand::RegisterSubscriber { issi } => assert_eq!(issi, expected_issi),
            other => panic!("expected RegisterSubscriber, got {other:?}"),
        }
    }

    fn assert_affiliate(cmd: BrewCommand, expected_issi: u32, expected_groups: &[u32]) {
        match cmd {
            BrewCommand::AffiliateGroups { issi, groups } => {
                assert_eq!(issi, expected_issi);
                assert_eq!(groups, expected_groups);
            }
            other => panic!("expected AffiliateGroups, got {other:?}"),
        }
    }

    fn assert_deaffiliate(cmd: BrewCommand, expected_issi: u32, expected_groups: &[u32]) {
        match cmd {
            BrewCommand::DeaffiliateGroups { issi, groups } => {
                assert_eq!(issi, expected_issi);
                assert_eq!(groups, expected_groups);
            }
            other => panic!("expected DeaffiliateGroups, got {other:?}"),
        }
    }

    #[test]
    fn sds_type4_rejects_length_indicator_above_etsi_limit() {
        // EN 300 392-2 clauses 14.7.2.8 note 5 and 14.8.52 limit
        // the Type4 length indicator to the 11-bit 0..=2 047-bit envelope.
        assert!(!BrewEntity::sds_type4_payload_can_serialize(2048, &[0; 256]));
    }

    #[test]
    fn sds_type4_rejects_length_indicator_below_protocol_identifier_width() {
        // EN 300 392-2 clause 14.8.52: Type4 SDS starts with an 8-bit
        // protocol identifier. Brew-origin SDS must not enter CMCE with a
        // shorter length that serialization will later reject.
        assert!(!BrewEntity::sds_type4_payload_can_serialize(0, &[]));
        assert!(!BrewEntity::sds_type4_payload_can_serialize(7, &[0x80]));
    }

    #[test]
    fn sds_type4_requires_enough_packed_octets_for_tail_bits() {
        assert!(BrewEntity::sds_type4_payload_can_serialize(16, &[0; 2]));
        assert!(!BrewEntity::sds_type4_payload_can_serialize(17, &[0; 2]));
        assert!(BrewEntity::sds_type4_payload_can_serialize(17, &[0; 3]));
    }

    #[test]
    fn sds_type4_accepts_maximum_etsi_length_with_required_octets() {
        assert!(BrewEntity::sds_type4_payload_can_serialize(2047, &[0; 256]));
        assert!(!BrewEntity::sds_type4_payload_can_serialize(2047, &[0; 255]));
    }

    #[test]
    fn sds_tl_broadcast_transfer_clears_delivery_report_request() {
        let mut data = vec![0x82, 0x04, 0x44, 0x01, b'A'];
        BrewEntity::clear_sds_tl_delivery_report_request(&mut data);

        assert_eq!(data, vec![0x82, 0x00, 0x44, 0x01, b'A']);
    }

    #[test]
    fn sds_tl_broadcast_transfer_clears_vendor_pid_delivery_report_request() {
        let mut data = vec![0xDC, 0x04, 0x44, 0x01, b'A'];
        BrewEntity::clear_sds_tl_delivery_report_request(&mut data);

        assert_eq!(data, vec![0xDC, 0x00, 0x44, 0x01, b'A']);
    }

    #[test]
    fn sds_tl_broadcast_non_sds_tl_pid_payload_is_not_rewritten() {
        let mut data = vec![0x02, 0x04, 0x44, 0x01, b'A'];
        BrewEntity::clear_sds_tl_delivery_report_request(&mut data);

        assert_eq!(data, vec![0x02, 0x04, 0x44, 0x01, b'A']);
    }

    #[test]
    fn sds_tl_broadcast_report_pdu_is_not_rewritten_as_transfer() {
        let mut data = vec![0xDC, 0x10, 0x00, 0x44];
        BrewEntity::clear_sds_tl_delivery_report_request(&mut data);

        assert_eq!(data, vec![0xDC, 0x10, 0x00, 0x44]);
    }

    #[test]
    fn sds_report_status_waits_for_acknowledged_air_result() {
        let reporter = TxReporter::new();
        assert_eq!(BrewEntity::sds_report_status_for_reporter(&reporter), None);

        reporter.mark_transmitted();
        assert_eq!(
            BrewEntity::sds_report_status_for_reporter(&reporter),
            None,
            "ISSI SDS must wait for LLC acknowledgement, not just MAC transmission"
        );

        reporter.mark_acknowledged();
        assert_eq!(BrewEntity::sds_report_status_for_reporter(&reporter), Some(0));
    }

    #[test]
    fn sds_report_status_accepts_unacknowledged_group_transmission() {
        let reporter = TxReporter::new_unacked();
        assert_eq!(BrewEntity::sds_report_status_for_reporter(&reporter), None);

        reporter.mark_transmitted();
        assert_eq!(
            BrewEntity::sds_report_status_for_reporter(&reporter),
            Some(0),
            "GSSI SDS has no per-recipient basic-link ACK; success means transmitted on air"
        );
    }

    #[test]
    fn sds_report_status_maps_air_failure_to_nonzero_status() {
        let discarded = TxReporter::new();
        discarded.mark_discarded();
        assert_eq!(BrewEntity::sds_report_status_for_reporter(&discarded), Some(1));

        let lost = TxReporter::new();
        lost.mark_transmitted();
        lost.mark_lost();
        assert_eq!(BrewEntity::sds_report_status_for_reporter(&lost), Some(1));
    }

    #[test]
    fn runtime_issi_registers_and_subscribes_brew_routable_group() {
        let (mut entity, rx) = brew_entity_without_worker();

        entity.handle_subscriber_update(subscriber_update(2260616, Vec::new(), BrewSubscriberAction::Register));
        assert_register(
            rx.try_recv()
                .expect("runtime subscriber register is forwarded for Brew interconnect"),
            2260616,
        );

        entity.handle_subscriber_update(subscriber_update(2260616, vec![91], BrewSubscriberAction::Affiliate));

        assert_affiliate(rx.try_recv().expect("runtime subscriber affiliates GSSI"), 2260616, &[91]);
        assert!(rx.try_recv().is_err(), "only REGISTER then AFFILIATE expected");
    }

    #[test]
    fn runtime_issi_group_subscription_deaffiliates_and_deregisters() {
        let (mut entity, rx) = brew_entity_without_worker();

        entity.handle_subscriber_update(subscriber_update(2260616, Vec::new(), BrewSubscriberAction::Register));
        assert_register(rx.try_recv().expect("initial register"), 2260616);

        entity.handle_subscriber_update(subscriber_update(2260616, vec![91], BrewSubscriberAction::Affiliate));
        assert_affiliate(rx.try_recv().expect("initial affiliate"), 2260616, &[91]);

        entity.handle_subscriber_update(subscriber_update(2260616, vec![91], BrewSubscriberAction::Deaffiliate));
        assert_deaffiliate(rx.try_recv().expect("runtime group deaffiliate"), 2260616, &[91]);

        entity.handle_subscriber_update(subscriber_update(2260616, vec![91], BrewSubscriberAction::Affiliate));
        assert_affiliate(rx.try_recv().expect("second affiliate"), 2260616, &[91]);

        entity.handle_subscriber_update(subscriber_update(2260616, Vec::new(), BrewSubscriberAction::Deregister));
        assert_deaffiliate(rx.try_recv().expect("deregister deaffiliates active groups"), 2260616, &[91]);
        match rx.try_recv().expect("deregister unregisters local group subscriber") {
            BrewCommand::DeregisterSubscriber { issi } => assert_eq!(issi, 2260616),
            other => panic!("expected DeregisterSubscriber, got {other:?}"),
        }
    }

    #[test]
    fn resync_replays_runtime_issi_group_subscription() {
        let (mut entity, rx) = brew_entity_without_worker();
        entity.subscriber_groups.insert(2260616, HashSet::from([91]));

        entity.resync_subscribers();

        assert_register(rx.try_recv().expect("resync register"), 2260616);
        assert_affiliate(rx.try_recv().expect("resync affiliate"), 2260616, &[91]);
        assert!(rx.try_recv().is_err(), "only resync REGISTER + AFFILIATE expected");
    }

    #[test]
    fn local_private_issi_group_tx_uses_gssi_routing_policy() {
        let (mut entity, rx) = brew_entity_without_worker();

        entity.handle_local_call_start(7, 2260616, 91, 2);

        match rx.try_recv().expect("GSSI 91 GROUP_TX must be forwarded") {
            BrewCommand::SendGroupTx {
                source_issi,
                dest_gssi,
                priority,
                service,
                ..
            } => {
                assert_eq!(source_issi, 2260616);
                assert_eq!(dest_gssi, 91);
                assert_eq!(priority, 0);
                assert_eq!(service, 0);
            }
            other => panic!("expected SendGroupTx, got {other:?}"),
        }
    }

    #[test]
    fn local_group_ul_voice_is_sent_as_brew_v1_ste_frame() {
        let (mut entity, rx) = brew_entity_without_worker();

        entity.handle_local_call_start(7, 2260082, 22699, 2);
        let group_uuid = match rx.try_recv().expect("GSSI 22699 GROUP_TX must be forwarded") {
            BrewCommand::SendGroupTx {
                uuid,
                source_issi,
                dest_gssi,
                priority,
                service,
            } => {
                assert_eq!(source_issi, 2260082);
                assert_eq!(dest_gssi, 22699);
                assert_eq!(priority, 0);
                assert_eq!(service, 0);
                uuid
            }
            other => panic!("expected SendGroupTx, got {other:?}"),
        };

        let acelp_bits: Vec<u8> = (0..274).map(|idx| (idx % 2) as u8).collect();
        entity.handle_ul_voice(2, acelp_bits);

        match rx.try_recv().expect("UL ACELP must be forwarded as Brew voice") {
            BrewCommand::SendVoiceFrame { uuid, length_bits, data } => {
                assert_eq!(uuid, group_uuid);
                assert_eq!(length_bits, BREW_STE_FRAME_BITS);
                assert_eq!(data.len(), BREW_STE_FRAME_BYTES);
                assert_eq!(data[0], BREW_STE_HEADER_NORMAL_SPEECH);
                assert_eq!(
                    data[BREW_STE_FRAME_BYTES - 1] & BREW_STE_UNUSED_TAIL_MASK,
                    BREW_STE_UNUSED_TAIL_MASK
                );
            }
            other => panic!("expected SendVoiceFrame, got {other:?}"),
        }
    }

    #[test]
    fn local_group_range_still_suppresses_group_tx_to_brew() {
        let (mut entity, rx) = brew_entity_without_worker();

        entity.handle_local_call_start(7, 2260616, 90, 2);

        assert!(rx.try_recv().is_err(), "GSSI 90 is local by config and must not be sent to Brew");
    }
}

impl Drop for BrewEntity {
    fn drop(&mut self) {
        tracing::debug!("BrewEntity: shutting down, sending graceful disconnect");
        send_brew_command(&self.command_sender, BrewCommand::Disconnect);

        // Give the worker thread time to send DEAFFILIATE + DEREGISTER and close
        if let Some(handle) = self.worker_handle.take() {
            let timeout = std::time::Duration::from_secs(3);
            let start = std::time::Instant::now();
            loop {
                if handle.is_finished() {
                    let _ = handle.join();
                    tracing::debug!("BrewEntity: worker thread joined cleanly");
                    break;
                }
                if start.elapsed() >= timeout {
                    tracing::warn!("BrewEntity: worker thread did not finish in time, abandoning");
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}
