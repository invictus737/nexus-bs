// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::collections::{HashMap, HashSet, VecDeque};

use tetra_config::bluestation::{EnergySavingAssignment, SharedConfig};
use tetra_core::freqs::FreqInfo;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{
    BitBuffer, Direction, EndpointId, PhyBlockNum, Sap, SsiType, TdmaTime, TetraAddress, TimeslotOwner, Todo, TxReporter, TxState,
    unimplemented_log,
};
use tetra_pdus::cmce::{
    enums::{cmce_pdu_type_dl::CmcePduTypeDl, transmission_grant::TransmissionGrant},
    pdus::{d_info::DInfo, d_tx_granted::DTxGranted},
};
use tetra_pdus::llc::enums::llc_pdu_type::LlcPduType;
use tetra_pdus::llc::pdus::bl_ack::BlAck;
use tetra_pdus::llc::pdus::bl_adata::BlAdata;
use tetra_pdus::llc::pdus::bl_udata::BlUdata;
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;
use tetra_pdus::mle::fields::bs_service_details::BsServiceDetails;
use tetra_pdus::mle::pdus::d_mle_sync::DMleSync;
use tetra_pdus::mle::pdus::d_mle_sysinfo::DMleSysinfo;
use tetra_pdus::umac::enums::mac_pdu_type::MacPduType;
use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
use tetra_pdus::umac::enums::sysinfo_opt_field_flag::SysinfoOptFieldFlag;
use tetra_pdus::umac::fields::channel_allocation::ChanAllocElement;
use tetra_pdus::umac::fields::sysinfo_default_def_for_access_code_a::SysinfoDefaultDefForAccessCodeA;
use tetra_pdus::umac::fields::sysinfo_ext_services::SysinfoExtendedServices;
use tetra_pdus::umac::pdus::mac_access::MacAccess;
use tetra_pdus::umac::pdus::mac_data::MacData;
use tetra_pdus::umac::pdus::mac_end_hu::MacEndHu;
use tetra_pdus::umac::pdus::mac_end_ul::MacEndUl;
use tetra_pdus::umac::pdus::mac_frag_ul::MacFragUl;
use tetra_pdus::umac::pdus::mac_resource::MacResource;
use tetra_pdus::umac::pdus::mac_sync::MacSync;
use tetra_pdus::umac::pdus::mac_sysinfo::MacSysinfo;
use tetra_pdus::umac::pdus::mac_u_blck::MacUBlck;
use tetra_pdus::umac::pdus::mac_u_signal::MacUSignal;
use tetra_saps::control::call_control::{CallControl, Circuit, CircuitDlMediaSource};
use tetra_saps::lcmc::enums::alloc_type::ChanAllocType;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::tlmc::{TlmcConfigureReq, TlmcEnergyEconomyStartpoint};
use tetra_saps::tma::{TmaReport, TmaReportInd, TmaUnitdataInd, TmaUnitdataReq};
use tetra_saps::tmv::TmvConfigureReq;
use tetra_saps::tmv::enums::logical_chans::LogicalChannel;
use tetra_saps::{SapMsg, SapMsgInner};

use crate::lmac::components::scrambler;
use crate::umac::subcomp::bs_sched::{BsChannelScheduler, PrecomputedUmacPdus, TCH_S_CAP};
use crate::umac::subcomp::fillbits;
use crate::{MessagePrio, MessageQueue, TetraEntityTrait};

use super::subcomp::bs_defrag::BsDefrag;

pub struct UmacBs {
    self_component: TetraEntity,
    config: SharedConfig,
    dltime: TdmaTime,
    system_wide_services: bool,

    /// This MAC's endpoint ID, for addressing by the higher layers
    /// When using only a single base radio, we can set this to a fixed value
    endpoint_id: u32,

    /// Subcomponents
    defrag: BsDefrag,
    /// Pending STCH MAC-DATA spanning block1+block2 (length_ind=0b111110), keyed by timeslot.
    pending_stch: Option<PendingStch>,
    // event_label_store: EventLabelStore,
    /// Contains UL/DL scheduling logic
    /// Access to this field is used only by testing code
    pub channel_scheduler: BsChannelScheduler,
    pending_tma_reports: Vec<PendingTmaReport>,
    // ulrx_scheduler: UlScheduler,
    /// Timestamp of last received UL voice frame per timeslot (0-indexed: ts1..ts4).
    /// Used to detect UL inactivity when a radio disappears mid-transmission.
    last_ul_voice: [Option<TdmaTime>; 4],
    /// Private-call UL media from LMAC, held until same-burst STCH floor-control
    /// signalling has drained through CMCE. This preserves valid TCH/S speech
    /// without letting stale audio race ahead of a U-TX CEASED/FloorReleased event.
    pending_private_ul_media: [VecDeque<PendingPrivateUlMedia>; 4],
    /// Current ISSI allowed to send U-plane signalling on each assigned UL
    /// timeslot. MAC-U-SIGNAL has no address field, so STCH signalling such
    /// as U-TX DEMAND / U-TX CEASED must inherit the speaker identity from
    /// the active circuit/floor state.
    current_ul_speaker: [Option<TetraAddress>; 4],
    /// Small per-floor diagnostic counter for accepted UL media. This is used
    /// only in timeout logs so RF tests can distinguish "no uplink decoded" from
    /// "uplink decoded but later not routed".
    ul_media_events_since_floor: [u16; 4],
    /// Local endpoint context for SNDCP assigned-PDCH uplink. The endpoint is
    /// not encoded in MAC-U-BLCK/MAC-END, so UMAC must carry the endpoint from
    /// the downlink channel allocation that created the PDCH.
    packet_data_link_contexts: HashMap<TetraAddress, PacketDataLinkContext>,
    active_energy_saving_suspensions: HashMap<EnergySavingSuspensionKey, Vec<u32>>,
}

#[derive(Debug, Clone, Copy)]
struct PacketDataLinkContext {
    endpoint_id: EndpointId,
}

struct PendingStch {
    addr: TetraAddress,
    scrambling_code: u32,
    encrypted: bool,
    fill_bits: bool,
    sdu_part: BitBuffer,
}

struct PendingTmaReport {
    req_handle: Todo,
    tx_reporter: TxReporter,
    created_at: TdmaTime,
    context: PendingTmaReportContext,
}

#[derive(Debug, Clone)]
struct PendingTmaReportContext {
    main_address: TetraAddress,
    endpoint_id: u32,
    pdu_bits: usize,
    pdu_prio: Todo,
    stealing_permission: bool,
    stealing_repeats_flag: Option<bool>,
    chan_alloc: Option<PendingTmaChanAllocContext>,
    priority: TmaAdmissionPriority,
    cmce_pdu_type: Option<CmcePduTypeDl>,
}

#[derive(Debug, Clone, Copy)]
struct PendingTmaChanAllocContext {
    usage: Option<u8>,
    timeslots: [bool; 4],
    alloc_type: ChanAllocType,
    ul_dl_assigned: UlDlAssignment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TmaAdmissionPriority {
    Ordinary,
    CallControl,
    ChannelAllocation,
    ListenerFloorGrant,
    PositiveFloorGrant,
    FloorWithdraw,
}

impl PendingTmaReportContext {
    fn from_tma_unitdata_req(prim: &TmaUnitdataReq, priority: TmaAdmissionPriority) -> Self {
        let chan_alloc = prim.chan_alloc.as_ref().map(|chan_alloc| PendingTmaChanAllocContext {
            usage: chan_alloc.usage,
            timeslots: chan_alloc.timeslots,
            alloc_type: chan_alloc.alloc_type,
            ul_dl_assigned: chan_alloc.ul_dl_assigned,
        });

        Self {
            main_address: prim.main_address,
            endpoint_id: prim.endpoint_id,
            pdu_bits: prim.pdu.get_len(),
            pdu_prio: prim.pdu_prio,
            stealing_permission: prim.stealing_permission,
            stealing_repeats_flag: prim.stealing_repeats_flag,
            chan_alloc,
            priority,
            cmce_pdu_type: UmacBs::cmce_dl_pdu_type_from_tma_sdu(&prim.pdu),
        }
    }

    fn summary(&self) -> String {
        let chan_alloc = self.chan_alloc.map_or_else(
            || "none".to_string(),
            |chan_alloc| {
                format!(
                    "usage={:?} timeslots={:?} alloc_type={} ul_dl={}",
                    chan_alloc.usage, chan_alloc.timeslots, chan_alloc.alloc_type, chan_alloc.ul_dl_assigned
                )
            },
        );
        format!(
            "addr={} endpoint_id={} pdu_bits={} pdu_prio={} stealing={} repeats={:?} chan_alloc=[{}] priority={:?} cmce_pdu={:?}",
            self.main_address,
            self.endpoint_id,
            self.pdu_bits,
            self.pdu_prio,
            self.stealing_permission,
            self.stealing_repeats_flag,
            chan_alloc,
            self.priority,
            self.cmce_pdu_type
        )
    }
}

struct PendingPrivateUlMedia {
    ul_ts: u8,
    dl_target_ts: u8,
    received_at: TdmaTime,
    speaker_addr: Option<TetraAddress>,
    peer_ts: Option<u8>,
    deferred_during_hangtime: bool,
    media: PendingPrivateUlMediaKind,
}

enum PendingPrivateUlMediaKind {
    RawTchSHalfSlot { block_num: PhyBlockNum, type5_bits: Vec<u8> },
    AcElp { packed_bits: Vec<u8> },
}

impl PendingPrivateUlMedia {
    fn label(&self) -> &'static str {
        match &self.media {
            PendingPrivateUlMediaKind::RawTchSHalfSlot { .. } => "raw TCH/S",
            PendingPrivateUlMediaKind::AcElp { .. } => "ACELP",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct EnergySavingSuspensionKey {
    ts: u8,
    addr: TetraAddress,
}

const PREDEFINED_BROADCAST_GSSI: u32 = 0xFF_FFFF;

impl UmacBs {
    const MAX_PENDING_TMA_REPORTS: usize = 4096;
    const MAX_PENDING_PRIVATE_UL_MEDIA_PER_TS: usize = 4;
    const PENDING_PRIVATE_UL_MEDIA_TTL_TIMESLOTS: i32 = 18;
    // Local guard for a retained TMA-UNITDATA reporter that never reaches a
    // terminal TxReporter state. EN 300 392-2 clause 20.4.1.1.3 defines the
    // report primitive; this timeout only prevents implementation-state leaks.
    const TMA_REPORT_PENDING_TIMEOUT_TIMESLOTS: i32 = 30 * 18 * 4;

    pub fn new(config: SharedConfig) -> Self {
        let c = config.config();
        let scrambling_code = scrambler::tetra_scramb_get_init(c.net.mcc, c.net.mnc, c.cell.colour_code);
        let system_wide_services = Self::get_system_wide_services_state(&config);
        let precomps = Self::generate_precomps(&config);
        Self::log_on_air_service_capabilities(&config, &precomps);
        Self {
            self_component: TetraEntity::Umac,
            config,
            dltime: TdmaTime::default(),
            system_wide_services,
            endpoint_id: 1,
            defrag: BsDefrag::new(),
            pending_stch: None,
            // event_label_store: EventLabelStore::new(),
            channel_scheduler: BsChannelScheduler::new(scrambling_code, precomps),
            pending_tma_reports: Vec::new(),
            last_ul_voice: [None; 4],
            pending_private_ul_media: std::array::from_fn(|_| VecDeque::new()),
            current_ul_speaker: [None; 4],
            ul_media_events_since_floor: [0; 4],
            packet_data_link_contexts: HashMap::new(),
            active_energy_saving_suspensions: HashMap::new(),
        }
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_max_pending_tma_reports_for_test(&self) -> usize {
        Self::MAX_PENDING_TMA_REPORTS
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_pending_tma_report_count_for_test(&self) -> usize {
        self.pending_tma_reports.len()
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_tma_report_pending_timeout_timeslots_for_test(&self) -> i32 {
        Self::TMA_REPORT_PENDING_TIMEOUT_TIMESLOTS
    }

    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn debug_force_pending_tma_report_age_for_test(&mut self, age_timeslots: i32) {
        for report in &mut self.pending_tma_reports {
            report.created_at = self.dltime.add_timeslots(-age_timeslots);
        }
    }

    fn mark_ms_signalling_activity(&self, addr: TetraAddress, activity_time: TdmaTime) {
        if addr.ssi_type != SsiType::Issi {
            return;
        }
        if let Some(assignment) = self.config.state_write().energy_saving.get_mut(&addr.ssi) {
            // Compatibility guard: EN 300 392-2 clause 23.7.6/T.210 evidence is
            // covered in the scheduler after actual downlink signalling
            // transmission. Uplink-triggered wake extension keeps known radios
            // reachable while the corresponding downlink ACK/grant is queued,
            // but it must not be treated as formal T.210 conformance proof.
            assignment.mark_awake_from_signalling_activity(activity_time);
        }
    }

    fn active_addr_targets(&self, addr: TetraAddress) -> Vec<u32> {
        match addr.ssi_type {
            SsiType::Issi => vec![addr.ssi],
            SsiType::Gssi if addr.ssi == PREDEFINED_BROADCAST_GSSI => self.config.state_read().subscribers.all_registered_issis().collect(),
            SsiType::Gssi => self.config.state_read().subscribers.group_member_issis(addr.ssi).collect(),
            _ => Vec::new(),
        }
    }

    fn suspend_energy_saving_for_active_addr(&mut self, ts: u8, addr: TetraAddress, covered_issis: &mut HashSet<u32>) {
        let key = EnergySavingSuspensionKey { ts, addr };
        if let Some(existing_targets) = self.active_energy_saving_suspensions.get(&key) {
            covered_issis.extend(existing_targets.iter().copied());
            return;
        }

        let mut targets = self.active_addr_targets(addr);
        targets.retain(|issi| covered_issis.insert(*issi));
        if targets.is_empty() {
            return;
        }
        let tracked_targets = targets.clone();
        let mut state = self.config.state_write();
        for issi in targets {
            if let Some(assignment) = state.energy_saving.get_mut(&issi) {
                assignment.suspend_for_assigned_channel();
            }
        }
        drop(state);

        self.active_energy_saving_suspensions.insert(key, tracked_targets);
    }

    fn suspend_energy_saving_for_circuit(&mut self, ts: u8, circuit: &Circuit) {
        let mut covered_issis = HashSet::new();
        for addr in circuit.active_addresses() {
            self.suspend_energy_saving_for_active_addr(ts, addr, &mut covered_issis);
        }
    }

    fn resume_energy_saving_for_suspension_key(&mut self, key: EnergySavingSuspensionKey) {
        let Some(targets) = self.active_energy_saving_suspensions.remove(&key) else {
            return;
        };

        let mut state = self.config.state_write();
        for issi in targets {
            if let Some(assignment) = state.energy_saving.get_mut(&issi) {
                assignment.resume_from_assigned_channel(self.dltime);
            }
        }
    }

    fn resume_energy_saving_for_suspension_key_if_unowned(&mut self, key: EnergySavingSuspensionKey) {
        if self.channel_scheduler.circuit_is_active_for_addr(Direction::Dl, key.ts, key.addr)
            || self.channel_scheduler.circuit_is_active_for_addr(Direction::Ul, key.ts, key.addr)
        {
            return;
        }
        self.resume_energy_saving_for_suspension_key(key);
    }

    fn active_suspension_count_for_issi(&self, issi: u32) -> u16 {
        self.active_energy_saving_suspensions
            .values()
            .filter(|targets| targets.contains(&issi))
            .count()
            .try_into()
            .unwrap_or(u16::MAX)
    }

    fn active_suspension_key_covers_issi(&self, key: EnergySavingSuspensionKey, issi: u32) -> bool {
        match key.addr.ssi_type {
            SsiType::Issi => key.addr.ssi == issi,
            SsiType::Gssi if key.addr.ssi == PREDEFINED_BROADCAST_GSSI => self
                .config
                .state_read()
                .subscribers
                .all_registered_issis()
                .any(|registered_issi| registered_issi == issi),
            SsiType::Gssi => self.config.state_read().subscribers.contains_group_member(key.addr.ssi, issi),
            _ => false,
        }
    }

    fn sync_active_suspensions_for_issi(&mut self, issi: u32) -> u16 {
        let keys: Vec<EnergySavingSuspensionKey> = self.active_energy_saving_suspensions.keys().copied().collect();
        let mut active_count: u16 = 0;

        for key in keys {
            if !self.active_suspension_key_covers_issi(key, issi) {
                continue;
            }
            let Some(targets) = self.active_energy_saving_suspensions.get_mut(&key) else {
                continue;
            };
            if !targets.contains(&issi) {
                targets.push(issi);
            }
            active_count = active_count.saturating_add(1);
        }

        active_count
    }

    fn set_current_ul_speaker(&mut self, ts: u8, addr: TetraAddress) {
        if !(1..=4).contains(&ts) {
            tracing::warn!("UMAC: ignoring current UL speaker for invalid ts {}", ts);
            return;
        }
        if addr.ssi_type != SsiType::Issi {
            tracing::trace!("UMAC: current UL speaker for ts {} not set from non-ISSI address {}", ts, addr);
            return;
        }
        self.current_ul_speaker[ts as usize - 1] = Some(addr);
    }

    fn initial_ul_speaker_for_open_circuit(circuit: &Circuit) -> Option<TetraAddress> {
        let private_shared_simplex = circuit.peer_ts.is_none()
            && matches!(circuit.active_addr, Some(addr) if addr.ssi_type == SsiType::Issi)
            && circuit.active_secondary_addrs.iter().any(|addr| addr.ssi_type == SsiType::Issi);
        if private_shared_simplex {
            // For private simplex on one shared assigned channel, CMCE
            // FloorGranted is the authoritative U-plane switch. Opening the
            // bearer only makes the channel/listener context available; it
            // must not authorize a speaker before D-CONNECT ACK L2 ACK.
            return None;
        }

        match circuit.active_addr {
            Some(addr) if addr.ssi_type == SsiType::Issi => Some(addr),
            Some(addr) if addr.ssi_type == SsiType::Gssi => circuit
                .active_secondary_addrs
                .iter()
                .copied()
                .find(|addr| addr.ssi_type == SsiType::Issi),
            _ => None,
        }
    }

    fn clear_current_ul_speaker(&mut self, ts: u8) {
        if (1..=4).contains(&ts) {
            self.current_ul_speaker[ts as usize - 1] = None;
        }
    }

    fn current_ul_signal_addr(&self, ts: u8) -> Option<TetraAddress> {
        if !(1..=4).contains(&ts) {
            return None;
        }
        self.current_ul_speaker[ts as usize - 1]
    }

    fn scheduled_or_packet_data_uplink_context(&self, msg_dltime: TdmaTime, block_num: PhyBlockNum) -> Option<(TetraAddress, EndpointId)> {
        if let Some(addr) = self
            .channel_scheduler
            .ul_get_slot_owner(msg_dltime, block_num)
            .map(TetraAddress::issi)
        {
            return Some((addr, 0));
        }

        let addr = self.channel_scheduler.packet_data_uplink_owner(msg_dltime, block_num)?;
        let endpoint_id = self
            .packet_data_link_contexts
            .get(&addr)
            .map(|context| context.endpoint_id)
            .unwrap_or(0);
        Some((addr, endpoint_id))
    }

    fn llc_sdu_is_original_advanced_link(sdu: &BitBuffer) -> bool {
        matches!(
            sdu.peek_bits(4).and_then(|bits| LlcPduType::try_from(bits).ok()),
            Some(LlcPduType::AlSetup | LlcPduType::AlDataAlFinal | LlcPduType::AlAckAlRnr | LlcPduType::AlReconnect | LlcPduType::AlDisc)
        )
    }

    fn packet_data_context_endpoint_id(&self, addr: TetraAddress) -> EndpointId {
        self.packet_data_link_contexts
            .get(&addr)
            .map(|context| context.endpoint_id)
            .unwrap_or(0)
    }

    fn packet_data_advanced_link_endpoint_id(&self, addr: TetraAddress, sdu: Option<&BitBuffer>) -> EndpointId {
        if addr.ssi_type != SsiType::Issi || !sdu.is_some_and(Self::llc_sdu_is_original_advanced_link) {
            return 0;
        }
        self.packet_data_context_endpoint_id(addr)
    }

    fn packet_data_mac_data_endpoint_id(
        &self,
        addr: TetraAddress,
        msg_dltime: TdmaTime,
        block_num: PhyBlockNum,
        sdu: Option<&BitBuffer>,
    ) -> EndpointId {
        match self.channel_scheduler.packet_data_uplink_owner(msg_dltime, block_num) {
            Some(owner) if owner == addr => self.packet_data_context_endpoint_id(addr),
            _ => self.packet_data_advanced_link_endpoint_id(addr, sdu),
        }
    }

    fn remember_packet_data_link_context_from_tma_req(&mut self, prim: &TmaUnitdataReq) {
        let Some(chan_alloc) = prim.chan_alloc.as_ref() else {
            return;
        };
        if prim.main_address.ssi_type != SsiType::Issi || !BsChannelScheduler::sdu_is_sndcp_packet_data(&prim.pdu) {
            return;
        }

        if matches!(chan_alloc.alloc_type, ChanAllocType::QuitAndGo) || !chan_alloc.timeslots.iter().any(|assigned| *assigned) {
            self.packet_data_link_contexts.remove(&prim.main_address);
            return;
        }

        if matches!(chan_alloc.alloc_type, ChanAllocType::Replace | ChanAllocType::Additional) {
            self.packet_data_link_contexts.insert(
                prim.main_address,
                PacketDataLinkContext {
                    endpoint_id: prim.endpoint_id,
                },
            );
        }
    }

    fn reset_ul_media_diagnostic(&mut self, ts: u8) {
        if (1..=4).contains(&ts) {
            self.ul_media_events_since_floor[ts as usize - 1] = 0;
        }
    }

    fn note_accepted_ul_media(&mut self, ts: u8) {
        if (1..=4).contains(&ts) {
            let idx = ts as usize - 1;
            self.ul_media_events_since_floor[idx] = self.ul_media_events_since_floor[idx].saturating_add(1);
            if self.ul_media_events_since_floor[idx] == 1 {
                tracing::info!(
                    "UMAC RF diag: first accepted UL media after floor ts={} speaker={:?} dltime={}",
                    ts,
                    self.current_ul_signal_addr(ts),
                    self.dltime
                );
            }
        }
    }

    fn llc_sdu_is_ack_response(sdu: &BitBuffer) -> bool {
        let probe = BitBuffer::from_bitbuffer(sdu);
        matches!(
            probe.peek_bits(4).and_then(|bits| LlcPduType::try_from(bits).ok()),
            Some(LlcPduType::BlAck | LlcPduType::BlAckFcs | LlcPduType::BlAdata | LlcPduType::BlAdataFcs)
        )
    }

    fn pre_floor_private_ack_routing(&self, ts: u8, sdu: &BitBuffer) -> Option<(Vec<TetraAddress>, BitBuffer)> {
        if !self.private_simplex_waiting_for_floor_grant(ts) || !Self::llc_sdu_is_ack_response(sdu) {
            return None;
        }

        let participants = self.channel_scheduler.ul_circuit_issi_participants(ts);
        if participants.is_empty() {
            return None;
        }

        // EN 300 392-2 Annex D.4 and clauses 21.4.5/22.3.2.3: before the
        // private-simplex FloorGranted, STCH MAC-U-SIGNAL has no ISSI field.
        // A BL-ACK or BL-ADATA may be the caller's L2 ACK for D-CONNECT even
        // while the temporary bearer primary is the called ISSI. Pass only an
        // ACK-only copy to LLC under each candidate address; do not duplicate
        // any ambiguous BL-ADATA/BL-ACK payload before CMCE identifies the
        // speaker.
        Self::pre_floor_private_ack_only_sdu(sdu).map(|ack_sdu| (participants, ack_sdu))
    }

    fn pre_floor_private_ack_only_sdu(sdu: &BitBuffer) -> Option<BitBuffer> {
        let mut probe = BitBuffer::from_bitbuffer(sdu);
        match probe.peek_bits(4).and_then(|bits| LlcPduType::try_from(bits).ok())? {
            LlcPduType::BlAck | LlcPduType::BlAckFcs => {
                let ack = BlAck::from_bitbuf(&mut probe).ok()?;
                let mut ack_sdu = BitBuffer::new_autoexpand(8);
                BlAck {
                    has_fcs: false,
                    nr: ack.nr,
                }
                .to_bitbuf(&mut ack_sdu);
                ack_sdu.seek(0);
                Some(ack_sdu)
            }
            LlcPduType::BlAdata | LlcPduType::BlAdataFcs => {
                let ack = BlAdata::from_bitbuf(&mut probe).ok()?;
                let mut ack_sdu = BitBuffer::new_autoexpand(8);
                BlAck {
                    has_fcs: false,
                    nr: ack.nr,
                }
                .to_bitbuf(&mut ack_sdu);
                ack_sdu.seek(0);
                Some(ack_sdu)
            }
            _ => None,
        }
    }

    fn private_simplex_waiting_for_floor_grant(&self, ts: u8) -> bool {
        self.channel_scheduler.ul_circuit_is_private_participant_scoped(ts)
            && self.channel_scheduler.ul_circuit_peer_ts(ts).is_none()
            && self.current_ul_signal_addr(ts).is_none()
    }

    fn ul_media_speaker_tag(&self, ts: u8) -> Option<TetraAddress> {
        if !(1..=4).contains(&ts) {
            return None;
        }

        // In private simplex on one shared assigned channel, raw TCH/S carries
        // no ISSI. Around U-TX DEMAND/D-TX GRANTED the first speech half-slot
        // can arrive before CMCE has updated current_ul_speaker, so tagging it
        // with the old floor holder would falsely purge the first requester
        // media. Cross-routed P2P and group calls keep the speaker tag because
        // their source side is unambiguous enough for stale-media filtering.
        if self.channel_scheduler.ul_circuit_is_private_participant_scoped(ts) && self.channel_scheduler.ul_circuit_peer_ts(ts).is_none() {
            None
        } else {
            self.current_ul_signal_addr(ts)
        }
    }

    fn can_defer_ul_media_during_hangtime(&self, ts: u8) -> bool {
        use tetra_saps::control::call_control::CircuitDlMediaSource;

        if !(1..=4).contains(&ts) || !self.channel_scheduler.circuit_is_active(Direction::Ul, ts) {
            return false;
        }
        if matches!(
            self.channel_scheduler.ul_circuit_dl_media_source(ts),
            CircuitDlMediaSource::SwMI | CircuitDlMediaSource::LocalParrot
        ) {
            return false;
        }

        let dl_target_ts = self.channel_scheduler.ul_circuit_peer_ts(ts).unwrap_or(ts);
        self.channel_scheduler.circuit_is_active(Direction::Dl, dl_target_ts)
    }

    fn discard_pending_private_ul_media_involving(&mut self, ts: u8, reason: &str) {
        if !(1..=4).contains(&ts) {
            return;
        }
        for pending in &mut self.pending_private_ul_media {
            pending.retain(|media| {
                let should_discard = media.ul_ts == ts || media.dl_target_ts == ts;
                if should_discard {
                    tracing::info!(
                        "UMAC: dropped deferred private {} ul_ts={} dl_ts={} received_at={} because {}",
                        media.label(),
                        media.ul_ts,
                        media.dl_target_ts,
                        media.received_at,
                        reason
                    );
                }
                !should_discard
            });
        }
    }

    fn discard_pending_private_ul_media_except_source(
        &mut self,
        affected_ts: u8,
        source_ul_ts: u8,
        source_addr: TetraAddress,
        reason: &str,
    ) {
        if !(1..=4).contains(&affected_ts) {
            return;
        }
        for pending in &mut self.pending_private_ul_media {
            pending.retain(|media| {
                let should_consider = media.ul_ts == affected_ts || media.dl_target_ts == affected_ts;
                if !should_consider {
                    return true;
                }

                let preserve_for_new_source =
                    media.ul_ts == source_ul_ts && media.speaker_addr.is_none_or(|speaker_addr| speaker_addr == source_addr);
                if preserve_for_new_source {
                    return true;
                }

                tracing::info!(
                    "UMAC: dropped deferred private {} ul_ts={} dl_ts={} received_at={} because {}",
                    media.label(),
                    media.ul_ts,
                    media.dl_target_ts,
                    media.received_at,
                    reason
                );
                false
            });
        }
    }

    fn discard_pending_group_ul_media_except_hangtime_source(
        &mut self,
        affected_ts: u8,
        source_ul_ts: u8,
        source_addr: TetraAddress,
        reason: &str,
    ) {
        if !(1..=4).contains(&affected_ts) {
            return;
        }
        for pending in &mut self.pending_private_ul_media {
            pending.retain(|media| {
                let should_consider = media.ul_ts == affected_ts || media.dl_target_ts == affected_ts;
                if !should_consider {
                    return true;
                }

                let preserve_hangtime_media_for_new_source = media.deferred_during_hangtime
                    && media.ul_ts == source_ul_ts
                    && media.speaker_addr.is_none_or(|speaker_addr| speaker_addr == source_addr);
                if preserve_hangtime_media_for_new_source {
                    return true;
                }

                tracing::info!(
                    "UMAC: dropped deferred group {} ul_ts={} dl_ts={} received_at={} because {}",
                    media.label(),
                    media.ul_ts,
                    media.dl_target_ts,
                    media.received_at,
                    reason
                );
                false
            });
        }
    }

    fn defer_private_ul_media(&mut self, ul_ts: u8, dl_target_ts: u8, media: PendingPrivateUlMediaKind) {
        if !(1..=4).contains(&ul_ts) {
            return;
        }
        let idx = ul_ts as usize - 1;
        let speaker_addr = self.ul_media_speaker_tag(ul_ts);
        let peer_ts = self.channel_scheduler.ul_circuit_peer_ts(ul_ts);
        let deferred_during_hangtime = self.channel_scheduler.is_hangtime(ul_ts) || self.channel_scheduler.is_hangtime(dl_target_ts);
        let queue = &mut self.pending_private_ul_media[idx];
        if queue.len() >= Self::MAX_PENDING_PRIVATE_UL_MEDIA_PER_TS
            && let Some(old) = queue.pop_front()
        {
            tracing::warn!(
                "UMAC: dropping oldest deferred private {} ul_ts={} dl_ts={} received_at={} because pending media queue reached {} item(s)",
                old.label(),
                old.ul_ts,
                old.dl_target_ts,
                old.received_at,
                Self::MAX_PENDING_PRIVATE_UL_MEDIA_PER_TS
            );
        }
        queue.push_back(PendingPrivateUlMedia {
            ul_ts,
            dl_target_ts,
            received_at: self.dltime,
            speaker_addr,
            peer_ts,
            deferred_during_hangtime,
            media,
        });
    }

    fn flush_pending_private_ul_media(&mut self) {
        for idx in 0..self.pending_private_ul_media.len() {
            let pending = std::mem::take(&mut self.pending_private_ul_media[idx]);
            for media in pending {
                if let Some(media) = self.flush_pending_private_ul_media_item(media) {
                    self.pending_private_ul_media[idx].push_back(media);
                }
            }
        }
    }

    fn flush_pending_private_ul_media_item(&mut self, media: PendingPrivateUlMedia) -> Option<PendingPrivateUlMedia> {
        use tetra_saps::control::call_control::CircuitDlMediaSource;

        if media.received_at.age(self.dltime) > Self::PENDING_PRIVATE_UL_MEDIA_TTL_TIMESLOTS {
            tracing::debug!(
                "UMAC: dropping deferred private {} ul_ts={} dl_ts={} received_at={} because age exceeded {} timeslots",
                media.label(),
                media.ul_ts,
                media.dl_target_ts,
                media.received_at,
                Self::PENDING_PRIVATE_UL_MEDIA_TTL_TIMESLOTS
            );
            return None;
        }

        if !self.channel_scheduler.circuit_is_active(Direction::Ul, media.ul_ts) {
            tracing::debug!(
                "UMAC: dropping deferred private {} ul_ts={} because UL circuit is inactive",
                media.label(),
                media.ul_ts
            );
            return None;
        }
        if self.channel_scheduler.is_hangtime(media.ul_ts) {
            tracing::debug!(
                "UMAC: keeping deferred private {} ul_ts={} because UL floor is still in hangtime",
                media.label(),
                media.ul_ts
            );
            return Some(media);
        }
        if self.private_simplex_waiting_for_floor_grant(media.ul_ts) {
            tracing::debug!(
                "UMAC: keeping deferred private {} ul_ts={} because private simplex has no FloorGranted speaker yet",
                media.label(),
                media.ul_ts
            );
            return Some(media);
        }
        if media.speaker_addr.is_some() && self.current_ul_signal_addr(media.ul_ts) != media.speaker_addr {
            tracing::debug!(
                "UMAC: dropping deferred private {} ul_ts={} because the current speaker changed from {:?} to {:?}",
                media.label(),
                media.ul_ts,
                media.speaker_addr,
                self.current_ul_signal_addr(media.ul_ts)
            );
            return None;
        }

        let current_peer_ts = self.channel_scheduler.ul_circuit_peer_ts(media.ul_ts);
        if current_peer_ts != media.peer_ts {
            tracing::debug!(
                "UMAC: dropping deferred private {} ul_ts={} because peer_ts changed from {:?} to {:?}",
                media.label(),
                media.ul_ts,
                media.peer_ts,
                current_peer_ts
            );
            return None;
        }

        let dl_target_ts = match current_peer_ts {
            Some(peer_ts) => peer_ts,
            None => {
                if matches!(
                    self.channel_scheduler.ul_circuit_dl_media_source(media.ul_ts),
                    CircuitDlMediaSource::SwMI | CircuitDlMediaSource::LocalParrot
                ) {
                    tracing::debug!(
                        "UMAC: dropping deferred private {} ul_ts={} because {:?} supplies DL media",
                        media.label(),
                        media.ul_ts,
                        self.channel_scheduler.ul_circuit_dl_media_source(media.ul_ts)
                    );
                    return None;
                }
                media.ul_ts
            }
        };
        if dl_target_ts != media.dl_target_ts {
            tracing::debug!(
                "UMAC: dropping deferred private {} ul_ts={} because DL target changed from {} to {}",
                media.label(),
                media.ul_ts,
                media.dl_target_ts,
                dl_target_ts
            );
            return None;
        }
        if self.channel_scheduler.is_hangtime(dl_target_ts) {
            tracing::debug!(
                "UMAC: keeping deferred private {} ul_ts={} dl_ts={} because DL target is still in hangtime",
                media.label(),
                media.ul_ts,
                dl_target_ts
            );
            return Some(media);
        }
        if !self.channel_scheduler.circuit_is_active(Direction::Dl, dl_target_ts) {
            tracing::debug!(
                "UMAC: dropping deferred private {} ul_ts={} dl_ts={} because DL circuit is inactive",
                media.label(),
                media.ul_ts,
                dl_target_ts
            );
            return None;
        }

        let source_ul_ts = media.ul_ts;
        let speaker_addr = media.speaker_addr;
        let received_at = media.received_at;
        match media.media {
            PendingPrivateUlMediaKind::RawTchSHalfSlot { block_num, type5_bits } => {
                tracing::debug!(
                    "UMAC voice route: UL ts={} deferred raw TCH/S {:?} bits={} -> DL ts={} peer_ts={:?} received_at={} media_source={:?}",
                    source_ul_ts,
                    block_num,
                    type5_bits.len(),
                    dl_target_ts,
                    current_peer_ts,
                    received_at,
                    self.channel_scheduler.ul_circuit_dl_media_source(source_ul_ts)
                );
                self.channel_scheduler.dl_schedule_raw_tch_s_half_slot_from_ul(
                    dl_target_ts,
                    source_ul_ts,
                    speaker_addr,
                    block_num,
                    type5_bits,
                );
            }
            PendingPrivateUlMediaKind::AcElp { packed_bits } => {
                tracing::debug!(
                    "UMAC voice route: UL ts={} deferred ACELP packed_bytes={} -> DL ts={} peer_ts={:?} received_at={} media_source={:?}",
                    source_ul_ts,
                    packed_bits.len(),
                    dl_target_ts,
                    current_peer_ts,
                    received_at,
                    self.channel_scheduler.ul_circuit_dl_media_source(source_ul_ts)
                );
                self.channel_scheduler
                    .dl_schedule_tmd_from_ul(dl_target_ts, source_ul_ts, speaker_addr, packed_bits);
            }
        }
        self.last_ul_voice[source_ul_ts as usize - 1] = Some(received_at);
        if let Some(peer_ts) = current_peer_ts
            && (1..=4).contains(&peer_ts)
            && self.channel_scheduler.circuit_is_active(Direction::Ul, peer_ts)
        {
            self.last_ul_voice[peer_ts as usize - 1] = Some(received_at);
        }
        None
    }

    fn floor_media_timeslots(&self, ts: u8) -> [Option<u8>; 2] {
        if !(1..=4).contains(&ts) {
            return [None, None];
        }

        let peer_ts = self
            .channel_scheduler
            .ul_circuit_peer_ts(ts)
            .filter(|peer_ts| (1..=4).contains(peer_ts) && *peer_ts != ts);
        [Some(ts), peer_ts]
    }

    fn tlmc_energy_start_time(&self, startpoint: TlmcEnergyEconomyStartpoint) -> TdmaTime {
        let mut start_time = TdmaTime {
            t: self.dltime.t,
            f: startpoint.frame,
            m: startpoint.multiframe,
            h: self.dltime.h,
        };
        if start_time.diff(self.dltime) < 0 {
            start_time = start_time.add_timeslots(60 * 18 * 4);
        }
        start_time
    }

    fn apply_tlmc_energy_economy_config(&mut self, prim: &TlmcConfigureReq) {
        let Some(group) = prim.energy_economy_group else {
            return;
        };
        let Some(issi) = prim.energy_economy_issi else {
            tracing::warn!("UMAC BS TLMC energy economy config ignored without target ISSI");
            return;
        };

        if group == 0 {
            self.config.state_write().energy_saving.remove(&issi);
            return;
        }

        let Some(startpoint) = prim.energy_economy_startpoint else {
            tracing::warn!("UMAC BS TLMC energy economy config ignored without startpoint for ISSI {}", issi);
            return;
        };
        if EnergySavingAssignment::sleep_frames(group).is_none() {
            tracing::warn!("UMAC BS TLMC energy economy config ignored invalid EG{} for ISSI {}", group, issi);
            return;
        }
        if !matches!(startpoint.frame, 1..=17) || !matches!(startpoint.multiframe, 1..=60) {
            tracing::warn!(
                "UMAC BS TLMC energy economy config ignored invalid startpoint frame={} multiframe={} for ISSI {}",
                startpoint.frame,
                startpoint.multiframe,
                issi
            );
            return;
        }
        // EN 300 392-2 clauses 16.10.10 and 23.7.6 make this startpoint the
        // recurring EG receive cycle. Clause 23.5.2.2.7 then requires the BS
        // to send where the MS listens. Nexus-BS does not yet advertise full
        // frame-18 receive support for EG sleep cycles, so reject cycles that
        // would require frame 18.
        if EnergySavingAssignment::receive_cycle_uses_frame(group, startpoint.frame, startpoint.multiframe, 18) {
            tracing::warn!(
                "UMAC BS TLMC energy economy config ignored because EG{} startpoint frame={} multiframe={} recurs on frame 18 for ISSI {}",
                group,
                startpoint.frame,
                startpoint.multiframe,
                issi
            );
            return;
        }

        let start_time = self.tlmc_energy_start_time(startpoint);
        let t210_until = self.dltime.add_timeslots(18 * 4);
        let awake_until = if start_time.diff(t210_until) >= 0 { start_time } else { t210_until };
        let existing_suspension_count = self
            .config
            .state_read()
            .energy_saving
            .get(&issi)
            .map(|assignment| assignment.suspension_count)
            .unwrap_or(0);
        let active_suspension_count = self.active_suspension_count_for_issi(issi);
        let current_active_suspension_count = self.sync_active_suspensions_for_issi(issi);

        // EN 300 392-2 clauses 20.3.5.4.1c, 20.4.3 and 23.7.6 route the
        // negotiated energy economy group/startpoint to MAC through
        // TL/TMC-CONFIGURE. The local ISSI binds that assignment to one MS.
        // Clause 23.7.6 also suspends the sleep cycle while the MS has an
        // assigned channel/call active, so reconfiguration must preserve any
        // active local suspension until the assigned channel is released.
        self.config.state_write().energy_saving.insert(
            issi,
            EnergySavingAssignment {
                mode: group,
                frame: Some(startpoint.frame),
                multiframe: Some(startpoint.multiframe),
                awake_until: Some(awake_until),
                suspension_count: existing_suspension_count
                    .max(active_suspension_count)
                    .max(current_active_suspension_count),
            },
        );
    }

    fn rx_tlmc_configure_req(&mut self, message: SapMsg) {
        let SapMsgInner::TlmcConfigureReq(prim) = message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        self.apply_tlmc_energy_economy_config(&prim);
    }

    fn rx_tlmc_prim(&mut self, _queue: &mut MessageQueue, message: SapMsg) {
        match message.msg {
            SapMsgInner::TlmcConfigureReq(_) => self.rx_tlmc_configure_req(message),
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
            }
        }
    }

    /// Precomputes SYNC, SYSINFO messages (and subfield variants) for faster TX msg building
    /// Precomputed PDUs are passed to scheduler
    /// Needs to be re-invoked if any network parameter changes
    pub fn generate_precomps(config: &SharedConfig) -> PrecomputedUmacPdus {
        let c = config.config();

        // EN 300 392-2 clause 21.4.4.1 table 21.67 carries security
        // information by reference to EN 300 392-7 A.8.77. Nexus-BS does not
        // implement air-interface encryption yet, so keep the broadcast
        // fail-closed even if a direct StackConfig requests AIE.
        let wap_ip_sndcp_profile_enabled = Self::local_wap_ip_sndcp_profile_enabled(config);
        let mut section1_services = 0;
        if wap_ip_sndcp_profile_enabled {
            // EN 300 392-2 clause 21.4.4.1 table 21.68: this stack resolves
            // SNDCP data priority through LTPD/MLE, but does not yet advertise
            // extended AL, QoS scheduled access, D8PSK, or extra sections.
            section1_services |= 0b100_0000;
        }

        let ext_services = SysinfoExtendedServices {
            auth_required: false,
            class1_supported: false,
            class2_supported: false,
            class3_supported: false,
            sck_n: None,
            dck_retrieval_during_cell_select: None,
            dck_retrieval_during_cell_reselect: None,
            linked_gck_crypto_periods: None,
            short_gck_vn: None,
            sdstl_addressing_method: 2,
            gck_supported: false,
            section: 0,
            section_data: section1_services,
        };

        let def_access = SysinfoDefaultDefForAccessCodeA {
            imm: 8,
            wt: 5,
            nu: 5,
            fl_factor: false,
            ts_ptr: 0,
            min_pdu_prio: 0,
        };

        let sysinfo1 = MacSysinfo {
            main_carrier: c.cell.main_carrier,
            freq_band: c.cell.freq_band,
            freq_offset_index: FreqInfo::freq_offset_hz_to_id(c.cell.freq_offset_hz)
                .unwrap_or_else(|| panic!(
                    "Invalid [cell] freq_offset_hz = {} Hz. TETRA only allows 0, +6250, -6250, or +12500 Hz (ETSI freq offset IDs). Fix the config.",
                    c.cell.freq_offset_hz
                )),
            duplex_spacing: c.cell.duplex_spacing_id,
            reverse_operation: c.cell.reverse_operation,
            num_of_csch: 0, // Common secondary control channels
            ms_txpwr_max_cell: c.cell.ms_txpwr_max_cell,
            rxlev_access_min: 3, // -110 dBm (permissive, suitable for single-cell)
            access_parameter: 7, // -39 dBm (MS open-loop power control setpoint)
            radio_dl_timeout: 3, // 432 timeslots (~6s radio link timeout)
            cck_id: None,
            hyperframe_number: Some(0), // Updated dynamically in scheduler
            option_field: SysinfoOptFieldFlag::DefaultDefForAccCodeA,
            ts_common_frames: None,
            default_access_code: Some(def_access),
            ext_services: None,
        };

        let sysinfo2 = MacSysinfo {
            main_carrier: sysinfo1.main_carrier,
            freq_band: sysinfo1.freq_band,
            freq_offset_index: sysinfo1.freq_offset_index,
            duplex_spacing: sysinfo1.duplex_spacing,
            reverse_operation: sysinfo1.reverse_operation,
            num_of_csch: sysinfo1.num_of_csch,
            ms_txpwr_max_cell: sysinfo1.ms_txpwr_max_cell,
            rxlev_access_min: sysinfo1.rxlev_access_min,
            access_parameter: sysinfo1.access_parameter,
            radio_dl_timeout: sysinfo1.radio_dl_timeout,
            cck_id: None,
            hyperframe_number: Some(0), // Updated dynamically in scheduler
            option_field: SysinfoOptFieldFlag::ExtServicesBroadcast,
            ts_common_frames: None,
            default_access_code: None,
            ext_services: Some(ext_services),
        };

        let system_wide_services = Self::get_system_wide_services_state(config);
        let mle_sysinfo_pdu = DMleSysinfo {
            location_area: c.cell.location_area,
            subscriber_class: c.cell.subscriber_class,
            bs_service_details: BsServiceDetails {
                registration: c.cell.registration,
                deregistration: c.cell.deregistration,
                priority_cell: c.cell.priority_cell,
                no_minimum_mode: c.cell.no_minimum_mode,
                migration: c.cell.migration,
                system_wide_services,
                voice_service: c.cell.voice_service,
                circuit_mode_data_service: c.cell.circuit_mode_data_service,
                // EN 300 392-2 clauses 18.5.2.1/table 18.26 advertise
                // packet-data/SNDCP availability through local BS service
                // details. The parser only permits this bit for the explicit
                // local WAP/IP SNDCP MVP profile.
                sndcp_service: c.cell.sndcp_service && c.cell.wap_ip.as_ref().is_some_and(|wap| wap.enabled),
                // Same fail-closed rule for air-interface encryption: do not
                // advertise AIE until EN 300 392-7 security procedures are
                // implemented and tested. Advanced link is advertised only for
                // the local SNDCP/WAP profile whose LLC AL-SETUP/AL-FINAL path
                // is implemented and test-backed.
                aie_service: false,
                advanced_link: c.cell.advanced_link && c.cell.sndcp_service && c.cell.wap_ip.as_ref().is_some_and(|wap| wap.enabled),
            },
        };

        let mac_sync_pdu = MacSync {
            system_code: c.cell.system_code,
            colour_code: c.cell.colour_code,
            time: TdmaTime::default(), // replaced dynamically in scheduler
            sharing_mode: c.cell.sharing_mode,
            ts_reserved_frames: c.cell.ts_reserved_frames,
            u_plane_dtx: c.cell.u_plane_dtx,
            // EN 300 392-2 clause 21.4.6.5: frame 18 extension tells MSs
            // they may receive downlink information on all slots of frame 18.
            // This BS scheduler now allows scheduled SCH/F only on legal
            // non-fixed frame-18 opportunities and still protects mandatory
            // BSCH/BNCH/CLCH slots, so advertising all-slot reception would
            // overstate the implemented MAC behaviour even if config asks.
            frame_18_ext: false,
        };

        let mle_sync_pdu = DMleSync {
            mcc: c.net.mcc,
            mnc: c.net.mnc,
            // Per ETSI EN 300 392-2 Table 18.17:
            // 0 = no broadcast, 1 = broadcast+enquiry, 2 = broadcast only, 3 = reserved
            // Hardcoded to 2 (broadcast only) to match the legacy upstream behavior —
            // required for Motorola terminals (MXP600, MTM800E, MTM5400) to accept
            // and display network time/date received via D-NWRK-BROADCAST.
            // Driving this from config (c.cell.neighbor_cell_broadcast) caused a
            // regression where missing config field → unwrap_or(0) → terminals ignore broadcast.
            neighbor_cell_broadcast: 2,
            cell_load_ca: 0, // TODO implement dynamic setting. 0 = info unavailable
            late_entry_supported: c.cell.late_entry_supported,
        };

        PrecomputedUmacPdus {
            mac_sysinfo1: sysinfo1,
            mac_sysinfo2: sysinfo2,
            mle_sysinfo: mle_sysinfo_pdu,
            mac_sync: mac_sync_pdu,
            mle_sync: mle_sync_pdu,
        }
    }

    fn local_wap_ip_sndcp_profile_enabled(config: &SharedConfig) -> bool {
        let cfg = config.config();
        cfg.cell.sndcp_service && cfg.cell.wap_ip.as_ref().is_some_and(|wap| wap.enabled)
    }

    /// Retrieve currently set value of system-wide services.
    ///
    /// EN 300 392-2 table 18.26 exposes normal mode/system-wide services in
    /// D-MLE-SYSINFO. When the local WAP/IP SNDCP profile is enabled, Nexus-BS
    /// owns the packet-data service locally and must not make the CA cell
    /// oscillate between normal and fallback mode because an external Brew
    /// backhaul reconnects.
    fn get_system_wide_services_state(config: &SharedConfig) -> bool {
        let cfg = config.config();
        if Self::local_wap_ip_sndcp_profile_enabled(config) {
            cfg.cell.system_wide_services
        } else if cfg.brew.is_some() {
            config.state_read().network_connected
        } else {
            cfg.cell.system_wide_services
        }
    }

    fn log_on_air_service_capabilities(config: &SharedConfig, precomps: &PrecomputedUmacPdus) {
        let cfg = config.config();
        let brew_connected = config.state_read().network_connected;
        let details = &precomps.mle_sysinfo.bs_service_details;
        let ext = precomps.mac_sysinfo2.ext_services.as_ref();
        let (section, section_data) = ext.map(|services| (services.section, services.section_data)).unwrap_or((0, 0));
        let section1_data_priority = section == 0 && (section_data & 0b100_0000) != 0;
        let section1_extended_advanced_link = section == 0 && (section_data & 0b010_0000) != 0;
        let section1_qos_negotiation = section == 0 && (section_data & 0b001_0000) != 0;
        let section1_d8psk = section == 0 && (section_data & 0b000_1000) != 0;

        tracing::info!(
            "UmacBs: on-air CA services system_wide={} sndcp={} voice={} circuit_data={} advanced_link={} aie={} wap_ip_profile={} brew_configured={} brew_connected={} ext_section={} ext_section_data=0b{:07b} data_priority={} ext_advanced_link={} qos_negotiation={} d8psk={}",
            details.system_wide_services,
            details.sndcp_service,
            details.voice_service,
            details.circuit_mode_data_service,
            details.advanced_link,
            details.aie_service,
            Self::local_wap_ip_sndcp_profile_enabled(config),
            cfg.brew.is_some(),
            brew_connected,
            section,
            section_data,
            section1_data_priority,
            section1_extended_advanced_link,
            section1_qos_negotiation,
            section1_d8psk
        );
    }

    fn refresh_system_wide_services(&mut self) {
        let is_effective = Self::get_system_wide_services_state(&self.config);
        if is_effective != self.system_wide_services {
            self.system_wide_services = is_effective;
            self.channel_scheduler.set_system_wide_services_state(is_effective);

            // Should already be signalled at SwMI interface level
            tracing::debug!("UmacBs: system_wide_services {}", if is_effective { "ENABLED" } else { "DISABLED" });
        }
    }

    fn wap_ip_diag_enabled(&self) -> bool {
        Self::local_wap_ip_sndcp_profile_enabled(&self.config)
    }

    fn cmce_to_mac_chanalloc(chan_alloc: &CmceChanAllocReq, carrier_num: u16) -> ChanAllocElement {
        // We grant clch permission for Replace and Additional allocations on the uplink
        let clch_permission = (chan_alloc.alloc_type == ChanAllocType::Replace || chan_alloc.alloc_type == ChanAllocType::Additional)
            && (chan_alloc.ul_dl_assigned == UlDlAssignment::Ul || chan_alloc.ul_dl_assigned == UlDlAssignment::Both);
        ChanAllocElement {
            alloc_type: chan_alloc.alloc_type,
            ts_assigned: chan_alloc.timeslots,
            ul_dl_assigned: chan_alloc.ul_dl_assigned,
            clch_permission,
            cell_change_flag: false,
            carrier_num,
            ext: None,
            mon_pattern: 0,
            frame18_mon_pattern: Some(0),
        }
    }

    /// Convenience function to send a TMA-REPORT.ind
    fn send_tma_report_ind(queue: &mut MessageQueue, handle: Todo, report: TmaReport) {
        let tma_report_ind = TmaReportInd {
            req_handle: handle,
            report,
        };
        let msg = SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Umac,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TmaReportInd(tma_report_ind),
        };
        queue.push_back(msg);
    }

    fn cmce_dl_payload_from_tma_sdu(sdu: &BitBuffer) -> Option<BitBuffer> {
        let direct = BitBuffer::from_bitbuffer(sdu);
        if matches!(
            direct.peek_bits(5).and_then(|bits| CmcePduTypeDl::try_from(bits).ok()),
            Some(CmcePduTypeDl::DTxGranted | CmcePduTypeDl::DTxCeased | CmcePduTypeDl::DTxInterrupt)
        ) {
            // EN 300 392-2 clauses 14.5.2.2.1 and 23.5 allow assigned-channel
            // floor control directly on STCH. D-TX INTERRUPT can start with the
            // same first four bits as LLC BL-UDATA-FCS, so classify direct
            // floor-control before trying the LLC wrapper.
            return Some(direct);
        }

        let mut wrapped = BitBuffer::from_bitbuffer(sdu);
        if BlUdata::from_bitbuf(&mut wrapped).is_ok() {
            let discriminator = wrapped
                .read_field(3, "mle_protocol_discriminator")
                .ok()
                .and_then(|bits| MleProtocolDiscriminator::try_from(bits).ok());
            if discriminator == Some(MleProtocolDiscriminator::Cmce) {
                return Some(wrapped);
            }
            return None;
        }

        if matches!(
            direct.peek_bits(4).and_then(|bits| LlcPduType::try_from(bits).ok()),
            Some(LlcPduType::BlAck | LlcPduType::BlAckFcs | LlcPduType::BlAdata | LlcPduType::BlAdataFcs)
        ) {
            return None;
        }
        if direct.get_len_remaining() < 10 {
            return None;
        }
        direct
            .peek_bits(5)
            .and_then(|bits| CmcePduTypeDl::try_from(bits).ok())
            .map(|_| direct)
    }

    fn cmce_dl_pdu_type_from_tma_sdu(sdu: &BitBuffer) -> Option<CmcePduTypeDl> {
        let mut pdu_type_probe = Self::cmce_dl_payload_from_tma_sdu(sdu)?;
        pdu_type_probe
            .read_field(5, "cmce_pdu_type_dl")
            .ok()
            .and_then(|bits| CmcePduTypeDl::try_from(bits).ok())
    }

    fn cmce_setup_call_control_priority(pdu_type: CmcePduTypeDl) -> bool {
        matches!(
            pdu_type,
            CmcePduTypeDl::DAlert
                | CmcePduTypeDl::DCallProceeding
                | CmcePduTypeDl::DConnect
                | CmcePduTypeDl::DConnectAcknowledge
                | CmcePduTypeDl::DDisconnect
                | CmcePduTypeDl::DRelease
                | CmcePduTypeDl::DSetup
                | CmcePduTypeDl::DCallRestore
                | CmcePduTypeDl::CmceFunctionNotSupported
        )
    }

    fn classify_tma_admission_priority(prim: &TmaUnitdataReq) -> TmaAdmissionPriority {
        let has_uplink_allocation = prim
            .chan_alloc
            .as_ref()
            .is_some_and(|chan_alloc| matches!(chan_alloc.ul_dl_assigned, UlDlAssignment::Ul | UlDlAssignment::Both));
        let has_channel_allocation = prim.chan_alloc.is_some();
        let cmce_pdu_type = Self::cmce_dl_pdu_type_from_tma_sdu(&prim.pdu);

        if !prim.stealing_permission {
            return if has_channel_allocation {
                TmaAdmissionPriority::ChannelAllocation
            } else if cmce_pdu_type.is_some_and(Self::cmce_setup_call_control_priority) {
                // EN 300 392-2 clause 14.5.1 call setup/release messages are
                // C-plane call-control progress. They still obey MAC/EG
                // scheduling below, but must not be admitted as ordinary SDS/data
                // when a pending setup is competing with local queue pressure.
                TmaAdmissionPriority::CallControl
            } else {
                TmaAdmissionPriority::Ordinary
            };
        }

        let Some(pdu_type) = cmce_pdu_type else {
            return if has_channel_allocation {
                TmaAdmissionPriority::ChannelAllocation
            } else {
                TmaAdmissionPriority::Ordinary
            };
        };

        match Some(pdu_type) {
            Some(CmcePduTypeDl::DTxInterrupt) | Some(CmcePduTypeDl::DTxCeased) => {
                // EN 300 392-2 clause 14.5.2.2.1 floor withdrawal/interrupt
                // is time-critical assigned-channel signalling.
                TmaAdmissionPriority::FloorWithdraw
            }
            Some(CmcePduTypeDl::DTxGranted) => {
                let Some(mut grant_probe) = Self::cmce_dl_payload_from_tma_sdu(&prim.pdu) else {
                    return TmaAdmissionPriority::Ordinary;
                };
                let Ok(grant) = DTxGranted::from_bitbuf(&mut grant_probe) else {
                    return if has_channel_allocation {
                        TmaAdmissionPriority::ChannelAllocation
                    } else {
                        TmaAdmissionPriority::Ordinary
                    };
                };
                if grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8 && has_uplink_allocation {
                    // This D-TX GRANTED is the response that lets an MS enter
                    // the assigned-channel U-plane; it must not be admitted
                    // behind thousands of lower-value busy responses.
                    TmaAdmissionPriority::PositiveFloorGrant
                } else if grant.transmission_grant == TransmissionGrant::GrantedToOtherUser.into_raw() as u8 && has_channel_allocation {
                    // EN 300 392-2 clause 14.5.2.2.1 b): the group-addressed
                    // listener notification must remain near the positive
                    // grant under storm pressure.
                    TmaAdmissionPriority::ListenerFloorGrant
                } else if has_channel_allocation {
                    TmaAdmissionPriority::ChannelAllocation
                } else {
                    TmaAdmissionPriority::Ordinary
                }
            }
            _ if has_channel_allocation => TmaAdmissionPriority::ChannelAllocation,
            _ => TmaAdmissionPriority::Ordinary,
        }
    }

    fn d_tx_granted_from_tma_sdu(sdu: &BitBuffer) -> Option<DTxGranted> {
        let mut payload = Self::cmce_dl_payload_from_tma_sdu(sdu)?;
        DTxGranted::from_bitbuf(&mut payload).ok()
    }

    fn tma_sdu_is_d_info_reset_t310(sdu: &BitBuffer) -> bool {
        let Some(mut payload) = Self::cmce_dl_payload_from_tma_sdu(sdu) else {
            return false;
        };
        DInfo::from_bitbuf(&mut payload).is_ok_and(|d_info| {
            d_info.reset_call_time_out_timer_t310_
                && d_info.call_time_out.is_none()
                && d_info.call_time_out_set_up_phase_t301_t302_.is_none()
                && d_info.new_call_identifier.is_none()
                && d_info.call_ownership.is_none()
                && d_info.modify.is_none()
                && d_info.call_status.is_none()
        })
    }

    fn tma_sdu_has_acknowledged_basic_link_tx(sdu: &BitBuffer) -> bool {
        matches!(
            BitBuffer::from_bitbuffer(sdu)
                .peek_bits(4)
                .and_then(|bits| LlcPduType::try_from(bits).ok()),
            Some(LlcPduType::BlData | LlcPduType::BlDataFcs | LlcPduType::BlAdata | LlcPduType::BlAdataFcs)
        )
    }

    fn tma_sdu_is_standalone_ack_only_bl_ack(sdu: &BitBuffer) -> bool {
        let mut probe = BitBuffer::from_bitbuffer(sdu);
        if !matches!(
            probe.peek_bits(4).and_then(|bits| LlcPduType::try_from(bits).ok()),
            Some(LlcPduType::BlAck | LlcPduType::BlAckFcs)
        ) {
            return false;
        }

        BlAck::from_bitbuf(&mut probe).is_ok_and(|_| probe.get_len_remaining() == 0)
    }

    fn tma_needs_current_channel_ack_grant(prim: &TmaUnitdataReq) -> bool {
        if prim.stealing_permission || !Self::tma_sdu_has_acknowledged_basic_link_tx(&prim.pdu) {
            return false;
        }

        prim.chan_alloc
            .as_ref()
            .is_some_and(|chan_alloc| matches!(chan_alloc.ul_dl_assigned, UlDlAssignment::Ul | UlDlAssignment::Both))
    }

    fn is_redundant_private_floor_grant_chan_alloc(&self, prim: &TmaUnitdataReq, ts: u8, grant: &DTxGranted) -> bool {
        if prim.main_address.ssi_type != SsiType::Issi {
            return false;
        }
        if grant.transmission_grant != TransmissionGrant::Granted.into_raw() as u8
            && grant.transmission_grant != TransmissionGrant::GrantedToOtherUser.into_raw() as u8
        {
            return false;
        }
        if !self.channel_scheduler.ul_circuit_is_private_participant_scoped(ts) {
            return false;
        }
        self.channel_scheduler
            .circuit_is_active_for_addr(Direction::Dl, ts, prim.main_address)
            || self
                .channel_scheduler
                .circuit_is_active_for_addr(Direction::Ul, ts, prim.main_address)
    }

    fn evict_lower_priority_tma_report(&mut self, queue: &mut MessageQueue, incoming_priority: TmaAdmissionPriority) -> bool {
        let Some((pos, _)) = self
            .pending_tma_reports
            .iter()
            .enumerate()
            .filter(|(_, pending)| pending.context.priority < incoming_priority && pending.tx_reporter.get_state() == TxState::Pending)
            .min_by_key(|(_, pending)| pending.context.priority)
        else {
            return false;
        };

        let pending = self.pending_tma_reports.remove(pos);
        let removed = self.channel_scheduler.dl_cancel_by_reporter(&pending.tx_reporter);
        if removed == 0 {
            pending.tx_reporter.try_mark_discarded();
        }
        tracing::warn!(
            "UMAC: evicting queued TMA req_handle={} priority {:?} context=\"{}\" for incoming priority {:?} under pending-report cap",
            pending.req_handle,
            pending.context.priority,
            pending.context.summary(),
            incoming_priority
        );
        Self::send_tma_report_ind(queue, pending.req_handle, TmaReport::FragmentationFailure);
        true
    }

    fn track_tma_request(
        &mut self,
        queue: &mut MessageQueue,
        handle: Todo,
        tx_reporter: Option<TxReporter>,
        context: PendingTmaReportContext,
        retain_report: bool,
    ) -> Option<Option<TxReporter>> {
        self.emit_completed_tma_reports(queue);
        if !retain_report {
            tracing::debug!(
                "UMAC: TMA-UNITDATA req_handle={} has no TxReporter and no retained TMA report context=\"{}\"",
                handle,
                context.summary()
            );
            return Some(None);
        }

        let tx_reporter = tx_reporter.unwrap_or_else(TxReporter::new_unacked);
        if self.pending_tma_reports.len() >= Self::MAX_PENDING_TMA_REPORTS && !self.evict_lower_priority_tma_report(queue, context.priority)
        {
            tracing::warn!(
                "UMAC: dropping TMA-UNITDATA req_handle={} priority {:?} context=\"{}\" because {} pending TMA reports are already retained",
                handle,
                context.priority,
                context.summary(),
                self.pending_tma_reports.len()
            );
            tx_reporter.try_mark_discarded();
            Self::send_tma_report_ind(queue, handle, TmaReport::FragmentationFailure);
            return None;
        }
        self.pending_tma_reports.push(PendingTmaReport {
            req_handle: handle,
            tx_reporter: tx_reporter.clone(),
            created_at: self.dltime,
            context,
        });
        Some(Some(tx_reporter))
    }

    fn rx_tma_cancel_req(&mut self, message: SapMsg) {
        let SapMsgInner::TmaCancelReq(prim) = message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let Some(pos) = self
            .pending_tma_reports
            .iter()
            .position(|pending| pending.req_handle == prim.req_handle)
        else {
            tracing::debug!("UMAC: TMA-CANCEL for unknown req_handle={}", prim.req_handle);
            return;
        };

        let pending = self.pending_tma_reports.remove(pos);
        if pending.tx_reporter.is_transmitted() {
            // EN 300 392-2 clause 20.4.1.1.1 cancels a submitted
            // TMA-UNITDATA request. Once the retained reporter shows complete
            // transmission, cancellation is too late and the normal
            // TMA-REPORT.ind success path must remain intact.
            tracing::debug!(
                "UMAC: TMA-CANCEL req_handle={} arrived after transmission; keeping pending report",
                prim.req_handle
            );
            self.pending_tma_reports.push(pending);
            return;
        }

        let removed = self.channel_scheduler.dl_cancel_by_reporter(&pending.tx_reporter);
        if removed == 0 {
            tracing::debug!(
                "UMAC: TMA-CANCEL req_handle={} found no queued scheduler elements; keeping pending report",
                prim.req_handle
            );
            self.pending_tma_reports.push(pending);
            return;
        }

        tracing::debug!(
            "UMAC: TMA-CANCEL req_handle={} removed {} queued scheduler element(s)",
            prim.req_handle,
            removed
        );
    }

    fn emit_completed_tma_reports(&mut self, queue: &mut MessageQueue) {
        let mut pending = Vec::new();
        for report in self.pending_tma_reports.drain(..) {
            if report.tx_reporter.is_transmitted() {
                // EN 300 392-2 clauses 20.4.1.1.3 and 23.1.2.1.1: once a
                // complete TM-SDU or final fragment has been sent, MAC reports
                // complete transmission to LLC using the retained request handle.
                Self::send_tma_report_ind(queue, report.req_handle, TmaReport::SuccessReservedOrStealing);
            } else if report.tx_reporter.get_state() == TxState::Discarded {
                // The local reporter only says the MAC did not completely send
                // the TM-SDU. Report the standard TMA failure that LLC clause
                // 22.3.2.3 uses for retry/failure handling instead of inventing
                // a TMA "failed transfer" result.
                Self::send_tma_report_ind(queue, report.req_handle, TmaReport::FragmentationFailure);
            } else if self
                .dltime
                .diff(report.created_at.add_timeslots(Self::TMA_REPORT_PENDING_TIMEOUT_TIMESLOTS))
                >= 0
            {
                let age_timeslots = self.dltime.diff(report.created_at);
                let removed = self.channel_scheduler.dl_cancel_by_reporter(&report.tx_reporter);
                tracing::warn!(
                    "UMAC: TMA report req_handle={} timed out after local pending-report guard age_timeslots={} cancelled_queued={} context=\"{}\"",
                    report.req_handle,
                    age_timeslots,
                    removed,
                    report.context.summary()
                );
                if removed == 0 {
                    report.tx_reporter.try_mark_discarded();
                }
                Self::send_tma_report_ind(queue, report.req_handle, TmaReport::FragmentationFailure);
            } else {
                pending.push(report);
            }
        }
        self.pending_tma_reports = pending;
    }

    fn rx_tmv_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tmv_prim");
        match message.msg {
            SapMsgInner::TmvUnitdataInd(_) => {
                self.rx_tmv_unitdata_ind(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }

    pub fn rx_tmv_unitdata_ind(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        tracing::trace!("rx_tmv_unitdata_ind: {:?}", prim.logical_channel);

        match prim.logical_channel {
            LogicalChannel::SchF => {
                // Full slot signalling — must be a full block. A mismatched block_num
                // would indicate a PHY/LMAC routing problem; drop and log instead of
                // asserting so a single odd block can't take down the cell.
                if prim.block_num != PhyBlockNum::Both {
                    tracing::warn!(
                        "rx_tmv_unitdata_ind: {:?} with unexpected block_num {:?}, dropping",
                        prim.logical_channel,
                        prim.block_num
                    );
                    return;
                }
                self.rx_tmv_sch(queue, message);
            }
            LogicalChannel::Stch | LogicalChannel::SchHu => {
                // Half slot signalling
                if !matches!(prim.block_num, PhyBlockNum::Block1 | PhyBlockNum::Block2) {
                    tracing::warn!(
                        "rx_tmv_unitdata_ind: {:?} with unexpected block_num {:?}, dropping",
                        prim.logical_channel,
                        prim.block_num
                    );
                    return;
                }
                self.rx_tmv_sch(queue, message);
            }
            // Any other logical channel reaching here is a routing error. Log and drop
            // rather than unreachable!()-panicking on wire-derived data.
            other => {
                tracing::warn!("rx_tmv_unitdata_ind: unhandled logical channel {:?}, dropping", other);
            }
        }
    }

    /// Receive signalling (SCH, or STCH / BNCH)
    pub fn rx_tmv_sch(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_tmv_sch");

        // Iterate until no more messages left in mac block
        loop {
            // let (lchan, block_num) = match &message.msg {
            //     SapMsgInner::TmvUnitdataInd(prim) => (prim.logical_channel, prim.block_num),
            //     _ => { tracing::warn!("unhandled match variant, ignoring"); }
            // };

            // Handle STCH MAC-DATA spanning block1+block2 (length_ind=0b111110)
            // if lchan == LogicalChannel::Stch {
            //     if block_num == PhyBlockNum::Block2 {
            //         if let Some(pending) = self.pending_stch.take() {
            //             self.rx_stch_second_half(queue, &mut message, pending);
            //             break;
            //         }
            //     } else if self.pending_stch.is_some() {
            //         tracing::warn!(
            //             "rx_tmv_sch: pending STCH second-half but got {:?} on ts {}",
            //             block_num,
            //             message.dltime.t
            //         );
            //         self.pending_stch = None;
            //     }
            // }

            // Extract info from inner block
            let SapMsgInner::TmvUnitdataInd(prim) = &message.msg else {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            };
            let Some(bits) = prim.pdu.peek_bits(3) else {
                tracing::warn!("insufficient bits: {}", prim.pdu.dump_bin());
                return;
            };
            if self.wap_ip_diag_enabled() {
                tracing::info!(
                    "WAP/IP diag: UMAC pre-parse lchan={:?} block={:?} rssi_dbfs={:.1} bits_remaining={} first3=0b{:03b}",
                    prim.logical_channel,
                    prim.block_num,
                    prim.rssi_dbfs,
                    prim.pdu.get_len_remaining(),
                    bits
                );
            }
            let orig_start = prim.pdu.get_raw_start();
            let lchan = prim.logical_channel;

            // Clause 21.4.1; handling differs between SCH_HU and others
            match lchan {
                LogicalChannel::SchF | LogicalChannel::Stch => {
                    // First two bits are MAC PDU type
                    let Ok(pdu_type) = MacPduType::try_from(bits >> 1) else {
                        tracing::warn!("invalid pdu type: {}", bits >> 1);
                        return;
                    };

                    match pdu_type {
                        MacPduType::MacResourceMacData => {
                            self.rx_mac_data(queue, &mut message);
                        }
                        MacPduType::MacFragMacEnd => {
                            // Also need third bit; designates mac-frag versus mac-end
                            if bits & 1 == 0 {
                                self.rx_mac_frag_ul(queue, &mut message);
                            } else {
                                self.rx_mac_end_ul(queue, &mut message);
                            }
                        }
                        MacPduType::SuppMacUSignal => {
                            // STCH determines which subtype is relevant
                            if lchan == LogicalChannel::Stch {
                                self.rx_ul_mac_u_signal(queue, &mut message);
                            } else {
                                // Supplementary MAC PDU type
                                if bits & 1 == 0 {
                                    self.rx_ul_mac_u_blck(queue, &mut message);
                                } else {
                                    tracing::warn!("unexpected supplementary PDU type")
                                }
                            }
                        }
                        _ => {
                            tracing::warn!("unknown pdu type: {}", pdu_type);
                        }
                    }
                }
                LogicalChannel::SchHu => {
                    // Need only 1 bit for a single subtype distinction
                    let pdu_type = (bits >> 2) & 1;
                    match pdu_type {
                        0 => self.rx_mac_access(queue, &mut message),
                        1 => self.rx_mac_end_hu(queue, &mut message),
                        _ => {
                            tracing::warn!("unhandled match variant, ignoring");
                        }
                    }
                }

                _ => {
                    tracing::warn!("unknown logical channel: {:?}", lchan);
                }
            }

            // Check if end of message reached by re-borrowing inner
            // If start was not updated, we also consider it end of message
            // If 16 or more bits remain (len of null pdu), we continue parsing
            if let SapMsgInner::TmvUnitdataInd(prim) = &message.msg {
                if prim.pdu.get_raw_start() != orig_start && prim.pdu.get_len() >= 16 {
                    tracing::trace!("orig {} now {}", orig_start, prim.pdu.get_raw_start());
                    tracing::trace!(
                        "rx_tmv_unitdata_ind_sch: Remaining {} bits: {:?}",
                        prim.pdu.get_len_remaining(),
                        prim.pdu.dump_bin_full(true)
                    );
                } else {
                    tracing::trace!("rx_tmv_unitdata_ind_sch: End of message reached");
                    break;
                }
            }
        }
    }

    fn rx_mac_data(&mut self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_mac_data");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        assert!(prim.pdu.get_pos() == 0); // We should be at the start of the MAC PDU

        let pdu = match MacData::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacData: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        // Get addr, either from pdu addr field or by resolving the event label
        if pdu.event_label.is_some() {
            unimplemented_log!("event labels not implemented");
            return;
        }
        // A well-formed MAC PDU must carry either addr or event_label. We already
        // returned for event_label above; if addr is also missing the PDU is malformed
        // — drop it instead of panicking on .unwrap().
        let Some(addr) = pdu.addr else {
            tracing::warn!("UMAC: rx_mac_data: PDU has neither addr nor event_label; dropping");
            return;
        };

        let (mut pdu_len_bits, is_frag_start, second_half_stolen, is_null_pdu) = {
            if let Some(len_ind) = pdu.length_ind {
                // We have a length ind, either clear length or a fragmentation start
                match len_ind {
                    0b000000 => {
                        // Null PDU
                        (if pdu.event_label.is_some() { 23 } else { 37 }, false, false, true)
                    }
                    0b000010..0b111000 => (len_ind as usize * 8, false, false, false),
                    0b111110 => {
                        // Second half stolen. Should be in STCH
                        (prim.pdu.get_len(), false, true, false)
                    }
                    0b111111 => {
                        // Start of fragmentation
                        (prim.pdu.get_len(), true, false, false)
                    }
                    _ => {
                        tracing::warn!("UMAC: rx_mac_data: unexpected length_ind {:#08b}, dropping PDU", len_ind);
                        return;
                    }
                }
            } else {
                // We have a capacity request — per spec, MacData with cap_req must
                // carry frag_flag. If a malformed PDU arrives without it, fall back to
                // frag_flag=false rather than panic.
                let frag_flag = pdu.frag_flag.unwrap_or_else(|| {
                    tracing::warn!("rx_mac_data: cap_req PDU missing frag_flag; assuming false");
                    false
                });
                tracing::trace!("rx_mac_data: cap_req {}", if frag_flag { "with frag_start" } else { "" });
                (prim.pdu.get_len(), frag_flag, false, false)
            }
        };

        if second_half_stolen {
            tracing::debug!("rx_mac_data: STCH 2nd half stolen");
            let msg_dltime = self.dltime.add_timeslots(-2);
            self.signal_lmac_second_half_stolen(queue, msg_dltime);
        }

        // Truncate len if past end (okay with standard)
        if pdu_len_bits > prim.pdu.get_len() {
            tracing::warn!("truncating MAC-DATA len from {} to {}", pdu_len_bits, prim.pdu.get_len());
            pdu_len_bits = prim.pdu.get_len() as usize;
        }
        if self.wap_ip_diag_enabled() {
            tracing::info!(
                "WAP/IP diag: UMAC MAC-DATA lchan={:?} block={:?} addr={:?} length_ind={:?} frag_flag={:?} reservation={:?} raw_bits={} pdu_len_bits={} null={}",
                prim.logical_channel,
                prim.block_num,
                addr,
                pdu.length_ind,
                pdu.frag_flag,
                pdu.reservation_req,
                prim.pdu.get_len(),
                pdu_len_bits,
                is_null_pdu
            );
        }

        // Strip fill bits. Maintain original end to allow for later parsing of a second mac block
        tracing::trace!("rx_mac_data: {}", prim.pdu.dump_bin_full(true));
        let num_fill_bits = {
            if pdu.fill_bits {
                fillbits::removal::get_num_fill_bits(&prim.pdu, pdu_len_bits, is_null_pdu)
            } else {
                0
            }
        };
        pdu_len_bits -= num_fill_bits;
        let orig_end = prim.pdu.get_raw_end();
        prim.pdu.set_raw_end(prim.pdu.get_raw_start() + pdu_len_bits);
        tracing::trace!(
            "rx_mac_data: pdu: {} sdu: {} fb: {}: {}",
            pdu_len_bits,
            prim.pdu.get_len_remaining(),
            num_fill_bits,
            prim.pdu.dump_bin_full(true)
        );

        if is_null_pdu {
            // TODO not sure if there is scenarios in which we want to pass a null pdu to the LLC
            // tracing::warn!("rx_mac_data: Null PDU not passed to LLC");
            return;
        }

        // Decrypt if needed
        if pdu.encrypted {
            unimplemented_log!("rx_mac_data: Encryption mode > 0");
            return;
        }

        // Handle reservation if present
        let msg_dltime = self.dltime.add_timeslots(-2); // Msg on uplink was sent two timeslots ago.
        self.mark_ms_signalling_activity(addr, msg_dltime);
        if let Some(res_req) = &pdu.reservation_req {
            self.channel_scheduler.dl_enqueue_reservation_grant(msg_dltime.t, addr, *res_req);
        };

        tracing::debug!("rx_mac_data: {}", prim.pdu.dump_bin_full(true));
        if is_frag_start {
            // Fragmentation start, add to defragmenter
            self.defrag.insert_first(&mut prim.pdu, msg_dltime, addr, None);
        } else {
            self.defrag
                .discard_incomplete_for_addr(msg_dltime, addr, "new MAC-DATA before MAC-END");

            // Pass directly to LLC
            let sdu = {
                if prim.pdu.get_len_remaining() == 0 {
                    None // No more data in this block
                } else {
                    // TODO FIXME should not copy here but take ownership
                    // Copy inner part, without MAC header or fill bits
                    Some(BitBuffer::from_bitbuffer_pos(&prim.pdu))
                }
            };

            if sdu.is_some() {
                let endpoint_id = self.packet_data_mac_data_endpoint_id(addr, msg_dltime, prim.block_num, sdu.as_ref());
                // We have an SDU for the LLC, deliver it.
                if self.wap_ip_diag_enabled() {
                    if let Some(sdu) = &sdu {
                        tracing::info!(
                            "WAP/IP diag: UMAC MAC-DATA delivering TM-SDU addr={:?} endpoint={} sdu_bits={} llc_prefix={:?} fill_bits={} stripped_fill_bits={}",
                            addr,
                            endpoint_id,
                            sdu.get_len(),
                            sdu.peek_bits(4),
                            pdu.fill_bits,
                            num_fill_bits
                        );
                    }
                }
                let m = SapMsg {
                    sap: Sap::TmaSap,
                    src: TetraEntity::Umac,
                    dest: TetraEntity::Llc,

                    msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
                        pdu: sdu,
                        main_address: addr,
                        scrambling_code: prim.scrambling_code,
                        endpoint_id,
                        new_endpoint_id: None, // TODO FIXME
                        css_endpoint_id: None, // TODO FIXME
                        air_interface_encryption: pdu.encrypted as Todo,
                        chan_change_response_req: false,
                        chan_change_handle: None,
                        chan_info: None,
                    }),
                };
                queue.push_back(m);
            } else {
                // Either this is a null pdu or we are at the end of the block
                // For now, we don't deliver this. However, important data may need to be signalled upwards
                tracing::warn!("rx_mac_data: empty PDU not passed to LLC");
            }
        }

        // Since this is not a null pdu, more MAC PDUs may follow
        // This allows parent function to continue parsing
        prim.pdu.set_raw_end(orig_end);
        prim.pdu.set_raw_pos(prim.pdu.get_raw_start() + pdu_len_bits + num_fill_bits);
        prim.pdu.set_raw_start(prim.pdu.get_raw_pos());
    }

    fn rx_mac_access(&mut self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_mac_access");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        assert!(prim.pdu.get_pos() == 0); // We should be at the start of the MAC PDU

        let pdu = match MacAccess::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacAccess: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        // Resolve event label (if supplied)
        let addr = if let Some(_label) = pdu.event_label {
            tracing::warn!("event labels not implemented");
            return;
        } else if let Some(addr) = pdu.addr {
            addr
        } else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        // Compute len and extract flags
        let mut pdu_len_bits;
        if let Some(length_ind) = pdu.length_ind {
            if length_ind == 0 {
                // Null PDU
                if pdu.event_label.is_some() {
                    // Short event label present
                    pdu_len_bits = 22; // 22 bits for event label
                } else {
                    // SSI
                    pdu_len_bits = 36;
                }
            } else {
                // Full length ind
                pdu_len_bits = length_ind as usize * 8;
            }
        } else {
            // No length ind, we have capacity request. Fill slot.
            pdu_len_bits = prim.pdu.get_len();
        }
        if pdu_len_bits > prim.pdu.get_len() {
            tracing::warn!("truncating MAC-ACCESS len from {} to {}", pdu_len_bits, prim.pdu.get_len());
            pdu_len_bits = prim.pdu.get_len();
        }
        if self.wap_ip_diag_enabled() {
            tracing::info!(
                "WAP/IP diag: UMAC MAC-ACCESS lchan={:?} block={:?} addr={:?} length_ind={:?} frag_flag={:?} reservation={:?} raw_bits={} pdu_len_bits={} null={}",
                prim.logical_channel,
                prim.block_num,
                addr,
                pdu.length_ind,
                pdu.frag_flag,
                pdu.reservation_req,
                prim.pdu.get_len(),
                pdu_len_bits,
                pdu.is_null_pdu()
            );
        }

        // Strip fill bits. Maintain original end to allow for later parsing of a second mac block
        // tracing::trace!("rx_mac_access: {}", prim.pdu.dump_bin_full(true));
        let num_fill_bits = if pdu.fill_bits {
            fillbits::removal::get_num_fill_bits(&prim.pdu, pdu_len_bits, pdu.is_null_pdu())
        } else {
            0
        };
        pdu_len_bits -= num_fill_bits;
        let orig_end = prim.pdu.get_raw_end();
        prim.pdu.set_raw_end(prim.pdu.get_raw_start() + pdu_len_bits);
        tracing::trace!(
            "rx_mac_access: pdu: {} sdu: {} fb: {}: {}",
            pdu_len_bits,
            prim.pdu.get_len_remaining(),
            num_fill_bits,
            prim.pdu.dump_bin_full(true)
        );

        if pdu.is_null_pdu() {
            // tracing::warn!("rx_mac_access: Null PDU not passed to LLC");
            return;
        }

        // Schedule acknowledgement of this message
        // let ul_time = message.dltime.add_timeslots(-2);
        let msg_dltime = self.dltime.add_timeslots(-2); // Msg on uplink was sent two timeslots ago.
        self.mark_ms_signalling_activity(addr, msg_dltime);
        self.channel_scheduler.dl_enqueue_random_access_ack(msg_dltime.t, addr);

        // Notify MM of RSSI for this MS so it can be stored per-subscriber.
        // Only sent when RSSI is a finite value (i.e. demodulator calculated it).
        if prim.rssi_dbfs.is_finite() {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Umac,
                dest: TetraEntity::Mm,
                msg: SapMsgInner::MsRssiUpdate {
                    issi: addr.ssi,
                    rssi_dbfs: prim.rssi_dbfs,
                },
            });
        }

        // Decrypt if needed
        if pdu.encrypted {
            unimplemented_log!("rx_mac_access: Encryption mode > 0");
            return;
        }

        // Handle reservation if present
        if let Some(res_req) = &pdu.reservation_req {
            self.channel_scheduler.dl_enqueue_reservation_grant(msg_dltime.t, addr, *res_req);
        };

        // tracing::debug!("rx_mac_access: {}", prim.pdu.dump_bin_full(true));
        if pdu.is_frag_start() {
            // Fragmentation start, add to defragmenter
            self.defrag.insert_first(&mut prim.pdu, msg_dltime, addr, None);
        } else {
            self.defrag
                .discard_incomplete_for_addr(msg_dltime, addr, "new MAC-ACCESS before MAC-END");

            // Pass directly to LLC
            if prim.pdu.get_len_remaining() == 0 {
                // Either this is a null pdu or we are at the end of the block
                // For now, we don't deliver this. However, important data may need to be signalled upwards
                tracing::warn!("rx_mac_access: empty PDU not passed to LLC");
                return;
            };

            // Pass directly to LLC
            let sdu = {
                if prim.pdu.get_len_remaining() == 0 {
                    None // No more data in this block
                } else {
                    // TODO FIXME check if there is a reasonable way to avoid copying here by taking ownership
                    Some(BitBuffer::from_bitbuffer_pos(&prim.pdu))
                }
            };

            if sdu.is_some() {
                let endpoint_id = self.packet_data_advanced_link_endpoint_id(addr, sdu.as_ref());
                // We have an SDU for the LLC, deliver it.
                if self.wap_ip_diag_enabled() {
                    if let Some(sdu) = &sdu {
                        tracing::info!(
                            "WAP/IP diag: UMAC MAC-ACCESS delivering TM-SDU addr={:?} endpoint={} sdu_bits={} llc_prefix={:?} fill_bits={} stripped_fill_bits={}",
                            addr,
                            endpoint_id,
                            sdu.get_len(),
                            sdu.peek_bits(4),
                            pdu.fill_bits,
                            num_fill_bits
                        );
                    }
                }
                let m = SapMsg {
                    sap: Sap::TmaSap,
                    src: TetraEntity::Umac,
                    dest: TetraEntity::Llc,
                    msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
                        pdu: sdu,
                        main_address: addr,
                        scrambling_code: prim.scrambling_code,
                        endpoint_id,
                        new_endpoint_id: None, // TODO FIXME
                        css_endpoint_id: None, // TODO FIXME
                        air_interface_encryption: pdu.encrypted as Todo,
                        chan_change_response_req: false,
                        chan_change_handle: None,
                        chan_info: None,
                    }),
                };
                queue.push_back(m);
            } else {
                // Either this is a null pdu or we are at the end of the block
                // For now, we don't deliver this. However, important data may need to be signalled upwards
                tracing::warn!("rx_mac_data: empty PDU not passed to LLC");
            }
        }

        // Since this is not a null pdu, more MAC PDUs may follow
        // This allows parent function to continue parsing
        prim.pdu.set_raw_end(orig_end);
        prim.pdu.set_raw_pos(prim.pdu.get_raw_start() + pdu_len_bits + num_fill_bits);
        prim.pdu.set_raw_start(prim.pdu.get_raw_pos());
    }

    fn rx_mac_frag_ul(&mut self, _queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_mac_frag_ul");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        assert!(prim.pdu.get_pos() == 0); // We should be at the start of the MAC PDU

        // Parse header and optional ChanAlloc
        let pdu = match MacFragUl::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacFragUl: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        // Strip fill bits. This message is known to fill the slot.
        let mut pdu_len_bits = prim.pdu.get_len();
        let num_fill_bits = {
            if pdu.fill_bits {
                fillbits::removal::get_num_fill_bits(&prim.pdu, pdu_len_bits, false)
            } else {
                0
            }
        };
        pdu_len_bits -= num_fill_bits;
        prim.pdu.set_raw_end(prim.pdu.get_raw_start() + pdu_len_bits);
        tracing::debug!("rx_mac_frag_ul: pdu_len_bits: {} fill_bits: {}", pdu_len_bits, num_fill_bits);

        // Get slot owner from schedule
        let msg_dltime = self.dltime.add_timeslots(-2); // Msg on uplink was sent two timeslots ago.
        let Some((slot_owner_addr, _slot_owner_endpoint_id)) = self.scheduled_or_packet_data_uplink_context(msg_dltime, prim.block_num)
        else {
            tracing::warn!("rx_mac_frag_ul: Received MAC-FRAG-UL for unassigned block {:?}", prim.block_num);
            self.channel_scheduler.dump_ul_schedule_full(true);
            return;
        };
        self.mark_ms_signalling_activity(slot_owner_addr, msg_dltime);

        if let Some(_aie_info) = self.defrag.get_aie_info(slot_owner_addr, msg_dltime) {
            unimplemented_log!("rx_mac_frag_ul: Encryption not supported");
            return;
        }

        // Insert into defragmenter
        self.defrag.insert_next(&mut prim.pdu, slot_owner_addr, msg_dltime);
    }

    fn rx_mac_end_ul(&mut self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_mac_end_ul");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        assert!(prim.pdu.get_pos() == 0); // We should be at the start of the MAC PDU

        // Parse header and optional ChanAlloc
        let pdu = match MacEndUl::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacEndUl: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        // Will have either length_ind or reservation_req, never none or both
        let mut pdu_len_bits = if let Some(length_ind) = pdu.length_ind {
            length_ind as usize * 8
        } else {
            // No length ind, we have capacity request. Fill slot.
            prim.pdu.get_len()
        };
        if pdu_len_bits > prim.pdu.get_len() {
            tracing::warn!("truncating MAC-END-UL len from {} to {}", pdu_len_bits, prim.pdu.get_len());
            pdu_len_bits = prim.pdu.get_len();
        }

        // Strip fill bits if any
        let num_fill_bits = {
            if pdu.fill_bits {
                fillbits::removal::get_num_fill_bits(&prim.pdu, pdu_len_bits, false)
            } else {
                0
            }
        };
        pdu_len_bits -= num_fill_bits;
        let orig_end = prim.pdu.get_raw_end();
        prim.pdu.set_raw_end(prim.pdu.get_raw_start() + pdu_len_bits);
        tracing::trace!(
            "rx_mac_end_ul: pdu: {} sdu: {} fb: {}: {}",
            pdu_len_bits,
            prim.pdu.get_len_remaining(),
            num_fill_bits,
            prim.pdu.dump_bin_full(true)
        );

        // Get slot owner from schedule, decrypt if needed
        let msg_dltime = self.dltime.add_timeslots(-2); // Msg on uplink was sent two timeslots ago.
        let Some((slot_owner_addr, slot_owner_endpoint_id)) = self.scheduled_or_packet_data_uplink_context(msg_dltime, prim.block_num)
        else {
            // Common with scan-list terminals that transmit on UL without waiting for a grant
            tracing::debug!("rx_mac_end_ul: Received MAC-END-UL for unassigned block {:?}", prim.block_num);
            return;
        };
        self.mark_ms_signalling_activity(slot_owner_addr, msg_dltime);
        if let Some(_aie_info) = self.defrag.get_aie_info(slot_owner_addr, msg_dltime) {
            // EN 300 392-2 air-interface encryption is not implemented in
            // this stack. Drop encrypted continuations instead of panicking or
            // forwarding undeciphered bits as a clear C-plane SDU.
            unimplemented_log!("rx_mac_end_ul: Encryption not supported");
            return;
        }

        // Insert last fragment and retrieve finalized block
        let defragbuf = self.defrag.insert_last(&mut prim.pdu, slot_owner_addr, msg_dltime);
        let Some(defragbuf) = defragbuf else {
            tracing::warn!("rx_mac_end_ul: could not obtain defragged buf");
            return;
        };
        self.mark_ms_signalling_activity(defragbuf.addr, msg_dltime);

        // Handle reservation if present
        if let Some(res_req) = &pdu.reservation_req {
            self.channel_scheduler
                .dl_enqueue_reservation_grant(msg_dltime.t, defragbuf.addr, *res_req);
        };

        // Pass completed block to LLC
        tracing::debug!("rx_mac_end_ul: sdu: {:?}", defragbuf.buffer.dump_bin());

        let m = SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Umac,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
                pdu: Some(defragbuf.buffer),
                main_address: defragbuf.addr,
                scrambling_code: prim.scrambling_code,
                endpoint_id: slot_owner_endpoint_id,
                new_endpoint_id: None,       // TODO FIXME
                css_endpoint_id: None,       // TODO FIXME
                air_interface_encryption: 0, // TODO FIXME implement
                chan_change_response_req: false,
                chan_change_handle: None,
                chan_info: None,
            }),
        };
        queue.push_back(m);

        // Since this is not a null pdu, more MAC PDUs may follow
        // This allows parent function to continue parsing
        prim.pdu.set_raw_end(orig_end);
        prim.pdu.set_raw_pos(prim.pdu.get_raw_start() + pdu_len_bits + num_fill_bits);
        prim.pdu.set_raw_start(prim.pdu.get_raw_pos());
    }

    fn rx_mac_end_hu(&mut self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_mac_end_hu");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        assert!(prim.pdu.get_pos() == 0); // We should be at the start of the MAC PDU

        // Parse header and optional ChanAlloc
        let pdu = match MacEndHu::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacEndHu: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        // Will have either length_ind or reservation_req, never none or both
        let mut pdu_len_bits = if let Some(length_ind) = pdu.length_ind {
            if length_ind == 0 {
                // Table 21.44: length indication 0 is reserved, discard PDU
                tracing::debug!("rx_mac_end_hu: discarding PDU with reserved length indication 0");
                return;
            }
            let len = length_ind as usize * 8;
            if len > prim.pdu.get_len() { prim.pdu.get_len() } else { len }
        } else {
            // No length ind, we have capacity request. Fill slot.
            prim.pdu.get_len()
        };
        if pdu_len_bits > prim.pdu.get_len() {
            tracing::warn!("truncating MAC-END-HU len from {} to {}", pdu_len_bits, prim.pdu.get_len());
            pdu_len_bits = prim.pdu.get_len();
        }

        // Strip fill bits if any
        let num_fill_bits = {
            if pdu.fill_bits {
                fillbits::removal::get_num_fill_bits(&prim.pdu, pdu_len_bits, false)
            } else {
                0
            }
        };
        pdu_len_bits -= num_fill_bits;
        let orig_end = prim.pdu.get_raw_end();
        prim.pdu.set_raw_end(prim.pdu.get_raw_start() + pdu_len_bits);

        // set to trace
        tracing::trace!(
            "rx_mac_end_hu: pdu: {} sdu: {} fb: {}: {}",
            pdu_len_bits,
            prim.pdu.get_len_remaining(),
            num_fill_bits,
            prim.pdu.dump_bin_full(true)
        );

        // Get slot owner from schedule, decrypt if needed
        let msg_dltime = self.dltime.add_timeslots(-2); // Msg on uplink was sent two timeslots ago.
        let Some(slot_owner) = self.channel_scheduler.ul_get_slot_owner(msg_dltime, prim.block_num) else {
            tracing::warn!("rx_mac_end_hu: Received MAC-END-HU for unassigned block {:?}", prim.block_num);
            self.channel_scheduler.dump_ul_schedule_full(true);
            return;
        };
        let slot_owner_addr = TetraAddress::issi(slot_owner);
        self.mark_ms_signalling_activity(slot_owner_addr, msg_dltime);
        if let Some(_aie_info) = self.defrag.get_aie_info(slot_owner_addr, msg_dltime) {
            // EN 300 392-2 air-interface encryption is not implemented in
            // this stack. Drop encrypted continuations instead of panicking or
            // forwarding undeciphered bits as a clear C-plane SDU.
            unimplemented_log!("rx_mac_end_hu: Encryption not supported");
            return;
        }

        // Insert last fragment and retrieve finalized block
        let defragbuf = self.defrag.insert_last(&mut prim.pdu, slot_owner_addr, msg_dltime);
        let Some(defragbuf) = defragbuf else {
            tracing::warn!("rx_mac_end_hu: could not obtain defragged buf");
            return;
        };
        self.mark_ms_signalling_activity(defragbuf.addr, msg_dltime);

        // Handle reservation if present
        if let Some(res_req) = &pdu.reservation_req {
            self.channel_scheduler
                .dl_enqueue_reservation_grant(msg_dltime.t, defragbuf.addr, *res_req);
        };

        // Pass completed block to LLC
        tracing::debug!("rx_mac_end_hu: sdu: {:?}", defragbuf.buffer.dump_bin());

        let m = SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Umac,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
                pdu: Some(defragbuf.buffer),
                main_address: defragbuf.addr,
                scrambling_code: prim.scrambling_code,
                endpoint_id: 0,              // TODO FIXME
                new_endpoint_id: None,       // TODO FIXME
                css_endpoint_id: None,       // TODO FIXME
                air_interface_encryption: 0, // TODO FIXME implement
                chan_change_response_req: false,
                chan_change_handle: None,
                chan_info: None,
            }),
        };
        queue.push_back(m);

        // Since this is not a null pdu, more MAC PDUs may follow
        // This allows parent function to continue parsing
        // tracing::trace!("rx_mac_end_hu: orig_end {} raw_start {} num_fill_bits {} curr_pos {}", orig_end, prim.pdu.get_raw_start(), num_fill_bits, prim.pdu.get_raw_pos());
        prim.pdu.set_raw_end(orig_end);
        prim.pdu.set_raw_pos(prim.pdu.get_raw_start() + pdu_len_bits + num_fill_bits);
        prim.pdu.set_raw_start(prim.pdu.get_raw_pos());
    }

    /// UL MAC-U-SIGNAL on STCH: extract TM-SDU and forward to LLC → MLE → CMCE.
    /// This carries signaling like U-TX CEASED / U-TX DEMAND on the traffic channel.
    fn rx_ul_mac_u_signal(&mut self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_ul_mac_u_signal");

        // Extract sdu and parse pdu
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let pdu = match MacUSignal::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacUSignal: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        if pdu.second_half_stolen {
            // ETSI EN 300 392-2 clauses 21.4.5 and 23.8.4.2.2: the
            // first-half MAC-U-SIGNAL still carries its 121-bit TM-SDU, while
            // the stolen flag tells the receiver to decode the second half as
            // STCH instead of TCH. Preserve the current TM-SDU and notify LMAC
            // before it processes block 2, otherwise signalling bits may be
            // passed upward as speech and heard as static.
            let msg_dltime = self.dltime.add_timeslots(-2);
            self.signal_lmac_second_half_stolen(queue, msg_dltime);
        }

        // The remaining bits after the MAC-U-SIGNAL header are the TM-SDU (LLC PDU)
        if prim.pdu.get_len_remaining() == 0 {
            tracing::trace!("rx_ul_mac_u_signal: empty TM-SDU");
            return;
        }

        let sdu = BitBuffer::from_bitbuffer_pos(&prim.pdu);
        tracing::debug!("rx_ul_mac_u_signal: forwarding {} bit TM-SDU to LLC", sdu.get_len());

        let msg_dltime = self.dltime.add_timeslots(-2);
        let (main_addresses, routed_sdu) = if let Some(addr) = self.current_ul_signal_addr(msg_dltime.t) {
            (vec![addr], sdu)
        } else if let Some((addrs, ack_sdu)) = self.pre_floor_private_ack_routing(msg_dltime.t, &sdu) {
            (addrs, ack_sdu)
        } else {
            (Vec::new(), sdu)
        };
        if main_addresses.is_empty() {
            tracing::warn!(
                "rx_ul_mac_u_signal: dropping STCH TM-SDU because no current ISSI speaker is known for UL ts {}",
                msg_dltime.t
            );
            return;
        }
        if main_addresses.len() > 1 {
            tracing::debug!(
                "rx_ul_mac_u_signal: routing pre-floor private ACK response on UL ts {} to participant candidates {:?}",
                msg_dltime.t,
                main_addresses
            );
        }

        // EN 300 392-2 clauses 21.4.5 and 14.5.1.2.1/14.5.2.2.1: MAC-U-SIGNAL
        // carries U-plane signalling on STCH without an address field. Preserve
        // the current assigned-channel speaker identity when it is known. Before
        // private-simplex FloorGranted, ACK responses have no sender address,
        // so route an ACK-only copy to the participant candidates and let LLC
        // match the pending acknowledged transfer by SSI/N(S).
        for main_address in main_addresses {
            queue.push_back(SapMsg {
                sap: Sap::TmaSap,
                src: TetraEntity::Umac,
                dest: TetraEntity::Llc,
                msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
                    pdu: Some(routed_sdu.clone()),
                    main_address,
                    scrambling_code: prim.scrambling_code,
                    endpoint_id: 0,
                    new_endpoint_id: None,
                    css_endpoint_id: None,
                    air_interface_encryption: 0,
                    chan_change_response_req: false,
                    chan_change_handle: None,
                    chan_info: None,
                }),
            });
        }
    }

    /// TMA-SAP MAC-U-BLCK
    fn rx_ul_mac_u_blck(&mut self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_ul_mac_u_blck");

        // Extract sdu and parse pdu
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let pdu = match MacUBlck::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacUBlck: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        if pdu.event_label == 0 || pdu.event_label == 0x03ff {
            // EN 300 392-2 clause 23.4.1.2.3: event label all-zero and
            // all-ones have special downlink meanings and are not valid for
            // normal MAC-U-BLCK use. The BS shall ignore MAC-U-BLCK with
            // either value.
            tracing::warn!(
                "MAC-U-BLCK event_label={} is reserved for non-normal use; ignoring PDU",
                pdu.event_label
            );
            return;
        }

        let msg_dltime = self.dltime.add_timeslots(-2); // Msg on uplink was sent two timeslots ago.
        let slot_owner_context = self.scheduled_or_packet_data_uplink_context(msg_dltime, prim.block_num);
        if let Some((slot_owner_addr, _slot_owner_endpoint_id)) = slot_owner_context {
            self.defrag
                .discard_incomplete_for_addr(msg_dltime, slot_owner_addr, "new MAC-U-BLCK before MAC-END");
            self.mark_ms_signalling_activity(slot_owner_addr, msg_dltime);
        }

        if let Some(res_req) = pdu.reservation_requirement() {
            self.enqueue_mac_u_blck_reservation(msg_dltime, prim.block_num, res_req, pdu.event_label);
        } else {
            tracing::debug!(
                "MAC-U-BLCK event_label={} explicitly indicated no reservation requirement",
                pdu.event_label
            );
        }

        if pdu.encrypted {
            unimplemented_log!("rx_ul_mac_u_blck: Encryption mode > 0");
            return;
        }

        let Some((slot_owner_addr, slot_owner_endpoint_id)) = slot_owner_context else {
            tracing::warn!(
                "MAC-U-BLCK event_label={} reservation_req={} has no scheduled or assigned-PDCH uplink owner; dropping TM-SDU",
                pdu.event_label,
                pdu.reservation_req
            );
            return;
        };

        let mut pdu_len_bits = prim.pdu.get_len();
        let num_fill_bits = if pdu.fill_bits {
            fillbits::removal::get_num_fill_bits(&prim.pdu, pdu_len_bits, false)
        } else {
            0
        };
        pdu_len_bits -= num_fill_bits;
        let orig_end = prim.pdu.get_raw_end();
        prim.pdu.set_raw_end(prim.pdu.get_raw_start() + pdu_len_bits);
        if prim.pdu.get_len_remaining() == 0 {
            prim.pdu.set_raw_end(orig_end);
            tracing::debug!(
                "MAC-U-BLCK event_label={} reservation_req={} carried no TM-SDU payload",
                pdu.event_label,
                pdu.reservation_req
            );
            return;
        }

        let sdu = BitBuffer::from_bitbuffer_pos(&prim.pdu);
        if self.wap_ip_diag_enabled() {
            tracing::info!(
                "WAP/IP diag: UMAC MAC-U-BLCK delivering TM-SDU addr={:?} event_label={} reservation_req={} sdu_bits={} llc_prefix={:?} fill_bits={} stripped_fill_bits={}",
                slot_owner_addr,
                pdu.event_label,
                pdu.reservation_req,
                sdu.get_len(),
                sdu.peek_bits(4),
                pdu.fill_bits,
                num_fill_bits
            );
        }
        queue.push_back(SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Umac,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
                pdu: Some(sdu),
                main_address: slot_owner_addr,
                scrambling_code: prim.scrambling_code,
                endpoint_id: slot_owner_endpoint_id,
                new_endpoint_id: None,
                css_endpoint_id: None,
                air_interface_encryption: pdu.encrypted as Todo,
                chan_change_response_req: false,
                chan_change_handle: None,
                chan_info: None,
            }),
        });
        prim.pdu.set_raw_end(orig_end);
    }

    fn enqueue_mac_u_blck_reservation(
        &mut self,
        msg_dltime: TdmaTime,
        block_num: PhyBlockNum,
        res_req: ReservationRequirement,
        event_label: u16,
    ) {
        let Some((addr, _endpoint_id)) = self.scheduled_or_packet_data_uplink_context(msg_dltime, block_num) else {
            tracing::warn!(
                "MAC-U-BLCK event_label={} requested {:?}, but no reserved-access or assigned-PDCH uplink owner is known; dropping reservation",
                event_label,
                res_req
            );
            return;
        };

        self.mark_ms_signalling_activity(addr, msg_dltime);
        self.channel_scheduler.dl_enqueue_reservation_grant(msg_dltime.t, addr, res_req);
    }

    fn rx_ul_tma_unitdata_req(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_ul_tma_unitdata_req");

        // Extract sdu
        let SapMsgInner::TmaUnitdataReq(mut prim) = message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let admission_priority = Self::classify_tma_admission_priority(&prim);
        let report_context = PendingTmaReportContext::from_tma_unitdata_req(&prim, admission_priority);
        let supplied_tx_reporter = prim.tx_reporter.take();
        let retain_tma_report = supplied_tx_reporter.is_some() || !Self::tma_sdu_is_standalone_ack_only_bl_ack(&prim.pdu);
        let Some(tx_reporter) = self.track_tma_request(queue, prim.req_handle, supplied_tx_reporter, report_context, retain_tma_report)
        else {
            return;
        };
        let needs_current_channel_ack_grant = Self::tma_needs_current_channel_ack_grant(&prim);
        let current_channel_ack_grant_addr = prim.main_address;
        let mut sdu = prim.pdu.clone();

        // ── FACCH/Stealing path ──────────────────────────────────────────
        // stealing_permission → STCH on traffic channel for time-critical signaling
        // (D-TX CEASED, D-TX GRANTED) per EN 300 392-2, clause 23.5.
        // CRITICAL: DL STCH uses MAC-RESOURCE (124-bit half-slot), NOT MAC-U-SIGNAL (UL-only).
        if prim.stealing_permission {
            // Determine the target traffic timeslot for FACCH stealing.
            // If chan_alloc specifies a timeslot, use it; otherwise fall back to first active DL circuit.
            let traffic_ts = prim
                .chan_alloc
                .as_ref()
                .and_then(|ca| ca.timeslots.iter().enumerate().find(|&(_, &set)| set).map(|(i, _)| (i + 1) as u8))
                .or_else(|| (2..=4u8).find(|&t| self.channel_scheduler.circuit_is_active(Direction::Dl, t)));

            if let Some(ts) = traffic_ts {
                // Guard: don't steal on a circuit that was just released (race between
                // D-RELEASE enqueue and circuit close). Drop silently — the MS will
                // receive the release via MCCH fallback or retransmit as needed.
                if !self.channel_scheduler.circuit_is_active(Direction::Dl, ts) {
                    tracing::debug!(
                        "rx_ul_tma_unitdata_req: FACCH stealing on ts {} skipped (circuit already closed)",
                        ts
                    );
                    // Fall through to MCCH path so the PDU still gets delivered.
                } else {
                    // Build MAC-RESOURCE PDU for the STCH half-slot (124 type1 bits).
                    const STCH_CAP: usize = 124;

                    let requested_usage_marker = prim.chan_alloc.as_ref().and_then(|ca| ca.usage);
                    let d_tx_granted = Self::d_tx_granted_from_tma_sdu(&sdu);
                    let omit_redundant_private_floor_chan_alloc = d_tx_granted
                        .as_ref()
                        .is_some_and(|grant| self.is_redundant_private_floor_grant_chan_alloc(&prim, ts, grant));
                    let omit_group_timer_d_info_chan_alloc = prim.main_address.ssi_type == SsiType::Gssi
                        && prim
                            .chan_alloc
                            .as_ref()
                            .is_some_and(|chan_alloc| chan_alloc.ul_dl_assigned == UlDlAssignment::Dl)
                        && Self::tma_sdu_is_d_info_reset_t310(&sdu);
                    let usage_marker = if omit_group_timer_d_info_chan_alloc {
                        // EN 300 392-2 clause 14.5.2.2.2 c): D-INFO reset T310
                        // is timer signalling, not assigned-channel authorization.
                        // Keep the CMCE channel allocation only as local routing
                        // metadata; on air this timer-only GSSI STCH must not
                        // carry either a channel allocation or a traffic usage
                        // marker immediately after D-TX GRANTED.
                        None
                    } else {
                        requested_usage_marker
                    };
                    let consume_ra_ack_without_chan_alloc = omit_redundant_private_floor_chan_alloc
                        && d_tx_granted
                            .as_ref()
                            .is_some_and(|grant| grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8);
                    let mac_chan_alloc = prim
                        .chan_alloc
                        .as_ref()
                        .filter(|_| !omit_redundant_private_floor_chan_alloc && !omit_group_timer_d_info_chan_alloc)
                        .map(|chan_alloc| Self::cmce_to_mac_chanalloc(chan_alloc, self.config.config().cell.main_carrier));
                    let mut mac_pdu = MacResource {
                        fill_bits: false,
                        pos_of_grant: 0,
                        encryption_mode: 0,
                        random_access_flag: false,
                        length_ind: 0,
                        addr: Some(prim.main_address),
                        event_label: None,
                        usage_marker,
                        power_control_element: None,
                        slot_granting_element: None,
                        chan_alloc_element: mac_chan_alloc,
                    };
                    let sdu_len = sdu.get_len();
                    let mut header_len = mac_pdu.compute_header_len();
                    let mut fill_bits = fillbits::addition::compute_required(header_len + sdu_len, STCH_CAP);
                    let mut total_len = header_len + sdu_len + fill_bits;

                    if total_len > STCH_CAP
                        && prim.main_address.ssi_type == SsiType::Gssi
                        && prim
                            .chan_alloc
                            .as_ref()
                            .is_some_and(|chan_alloc| chan_alloc.ul_dl_assigned == UlDlAssignment::Dl)
                    {
                        // EN 300 392-2 clause 14.5.2.2.1 b) recommends that
                        // group-addressed "granted to another user" signalling
                        // can identify the current speaker. On an already
                        // assigned traffic channel, the channel allocation in
                        // this TMA primitive is routing metadata; if including
                        // it would push the speaker-qualified GSSI PDU out of
                        // STCH capacity, keep the FACCH delivery and omit the
                        // redundant MAC channel allocation element.
                        mac_pdu.chan_alloc_element = None;
                        header_len = mac_pdu.compute_header_len();
                        fill_bits = fillbits::addition::compute_required(header_len + sdu_len, STCH_CAP);
                        total_len = header_len + sdu_len + fill_bits;
                    }

                    if total_len <= STCH_CAP {
                        let carries_channel_allocation = mac_pdu.chan_alloc_element.is_some() || consume_ra_ack_without_chan_alloc;
                        let is_group_requester_positive_floor_grant = d_tx_granted
                            .as_ref()
                            .is_some_and(|grant| grant.transmission_grant == TransmissionGrant::Granted.into_raw() as u8)
                            && prim.main_address.ssi_type == SsiType::Issi
                            && mac_pdu
                                .chan_alloc_element
                                .as_ref()
                                .is_some_and(|chan_alloc| matches!(chan_alloc.ul_dl_assigned, UlDlAssignment::Ul | UlDlAssignment::Both))
                            && !self.channel_scheduler.ul_circuit_is_private_participant_scoped(ts)
                            && self
                                .channel_scheduler
                                .ul_circuit_primary_addr(ts)
                                .is_some_and(|addr| addr.ssi_type == SsiType::Gssi);
                        mac_pdu.random_access_flag = if is_group_requester_positive_floor_grant {
                            self.channel_scheduler
                                .take_pending_or_ready_ra_ack_for_stch(ts, prim.main_address, carries_channel_allocation)
                        } else {
                            self.channel_scheduler
                                .take_pending_ra_ack_for_stch(ts, prim.main_address, carries_channel_allocation)
                        };
                        mac_pdu.length_ind = (total_len / 8) as u8;
                        mac_pdu.fill_bits = fill_bits > 0;

                        if let Some(grant) = d_tx_granted.as_ref() {
                            tracing::info!(
                                "UMAC RF diag: STCH D-TX GRANTED call_id={} ts={} addr={} grant={} ra_ack={} usage={:?} chan_alloc={:?} group_requester_positive={} omit_private_chan_alloc={} omit_group_timer_chan_alloc={} bits={{hdr:{},sdu:{},fill:{},total:{}}}",
                                grant.call_identifier,
                                ts,
                                prim.main_address,
                                grant.transmission_grant,
                                mac_pdu.random_access_flag,
                                usage_marker,
                                mac_pdu.chan_alloc_element.as_ref().map(|ca| (
                                    ca.ts_assigned,
                                    ca.ul_dl_assigned,
                                    ca.mon_pattern,
                                    ca.frame18_mon_pattern
                                )),
                                is_group_requester_positive_floor_grant,
                                omit_redundant_private_floor_chan_alloc,
                                omit_group_timer_d_info_chan_alloc,
                                header_len,
                                sdu_len,
                                fill_bits,
                                total_len
                            );
                        } else if omit_group_timer_d_info_chan_alloc {
                            tracing::info!(
                                "UMAC RF diag: STCH D-INFO reset T310 ts={} addr={} usage={:?} omitted_dl_only_chan_alloc=true bits={{hdr:{},sdu:{},fill:{},total:{}}}",
                                ts,
                                prim.main_address,
                                usage_marker,
                                header_len,
                                sdu_len,
                                fill_bits,
                                total_len
                            );
                        }

                        let mut stch_block = BitBuffer::new(STCH_CAP);
                        mac_pdu.to_bitbuf(&mut stch_block);

                        sdu.seek(0);
                        stch_block.copy_bits(&mut sdu, sdu_len);
                        fillbits::addition::write(&mut stch_block, Some(fill_bits));

                        tracing::debug!(
                            "rx_ul_tma_unitdata_req: FACCH stealing on ts {} (MAC-RESOURCE hdr {} + SDU {} + fill {} bits -> {} STCH bits)",
                            ts,
                            header_len,
                            sdu_len,
                            fill_bits,
                            stch_block.get_len()
                        );

                        self.channel_scheduler
                            .dl_enqueue_stealing(ts, stch_block, prim.main_address, tx_reporter);

                        return;
                    }

                    tracing::warn!(
                        "rx_ul_tma_unitdata_req: FACCH stealing on ts {} does not fit STCH (MAC-RESOURCE hdr {} + SDU {} bits > {}), falling back to MCCH/SCH-F",
                        ts,
                        header_len,
                        sdu_len,
                        STCH_CAP
                    );
                } // end circuit_is_active guard
            } else {
                tracing::warn!("rx_ul_tma_unitdata_req: stealing requested but no active DL circuit, falling back to MCCH");
                // Fall through to normal MCCH path below
            }
        }

        // ── Normal signaling path (MCCH / SCH/F) ────────────────────────
        self.remember_packet_data_link_context_from_tma_req(&prim);
        let (usage_marker, mac_chan_alloc) = if let Some(chan_alloc) = prim.chan_alloc {
            (
                chan_alloc.usage,
                Some(Self::cmce_to_mac_chanalloc(&chan_alloc, self.config.config().cell.main_carrier)),
            )
        } else {
            (None, None)
        };

        // Build MAC-RESOURCE optimistically (as if it would always fit in one slot).
        // ETSI EN 300 392-2 clause 21.4.3.1 defines random_access_flag as an
        // acknowledgement of random access, not as a generic ISSI-address marker.
        let mut pdu = MacResource {
            fill_bits: false, // Updated later
            pos_of_grant: 0,
            encryption_mode: 0,
            random_access_flag: false,
            length_ind: 0, // Updated later
            addr: Some(prim.main_address),
            event_label: None,
            usage_marker,
            power_control_element: None,
            slot_granting_element: None,
            chan_alloc_element: mac_chan_alloc,
        };
        pdu.update_len_and_fill_ind(sdu.get_len());

        // // Per ETSI EN 300 392-2 Clause 23.3.1.1.2: idle MSes monitor the MCCH (slot 1)
        // // for signaling. Without common SCCHs, all MSes listen on slot 1.
        // // All signaling on the normal path (non-FACCH) must go to the MCCH.
        // if message.dltime.t != 1 {
        //     tracing::warn!("rx_ul_tma_unitdata_req: signaling scheduled for non-MCCH {}", message.dltime.t);
        // }
        // self.channel_scheduler.dl_enqueue_tma(message.dltime.t, pdu, sdu, prim.tx_reporter);

        if needs_current_channel_ack_grant {
            // EN 300 392-2 clauses 23.5.2.2 and 23.5.4.3: late-assignment
            // individual call control may carry channel allocation while still
            // granting the BL-ACK subslot on the current MCCH. This avoids
            // asking the MS to acknowledge on a traffic slot that the BS has
            // already switched to U-plane reception.
            self.channel_scheduler.dl_enqueue_tma_with_current_channel_ack_grant(
                pdu,
                sdu,
                tx_reporter,
                current_channel_ack_grant_addr,
                ReservationRequirement::Req1Subslot,
            );
        } else {
            self.channel_scheduler.dl_enqueue_tma(pdu, sdu, tx_reporter);
        }

        // let enqueue_ts = 1;
        // self.channel_scheduler.dl_enqueue_tma(enqueue_ts, pdu, sdu, prim.tx_reporter);
    }

    fn rx_tma_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tma_prim");
        match message.msg {
            SapMsgInner::TmaUnitdataReq(_) => {
                self.rx_ul_tma_unitdata_req(queue, message);
            }
            SapMsgInner::TmaCancelReq(_) => {
                self.rx_tma_cancel_req(message);
            }
            _ => {
                tracing::warn!("unhandled match variant, ignoring");
            }
        }
    }

    fn rx_tlmb_prim(&mut self, _queue: &mut MessageQueue, _message: SapMsg) {
        tracing::trace!("rx_tlmb_prim");
        tracing::error!("BUG: unexpected message or state -- routing error");
        return;
    }

    fn rx_tmd_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        use tetra_saps::control::call_control::CircuitDlMediaSource;

        tracing::trace!("rx_tmd_prim");

        let src = message.src;
        match message.msg {
            // DL voice from Brew/upper layer → schedule for DL transmission
            SapMsgInner::TmdCircuitDataReq(prim) => {
                let ts = prim.ts;
                // Refresh UL inactivity timer when DL voice is being fed (network call scenario).
                // This prevents false timeout when Brew is the speaker and no UL radio is transmitting.
                if (1..=4).contains(&ts) && self.channel_scheduler.circuit_is_active(Direction::Ul, ts) {
                    self.last_ul_voice[ts as usize - 1] = Some(self.dltime);
                }
                if let Some(peer_ul_ts) = self.channel_scheduler.dl_circuit_peer_ts(ts)
                    && (1..=4).contains(&peer_ul_ts)
                    && self.channel_scheduler.circuit_is_active(Direction::Ul, peer_ul_ts)
                {
                    self.last_ul_voice[peer_ul_ts as usize - 1] = Some(self.dltime);
                }
                if self.channel_scheduler.circuit_is_active(Direction::Dl, ts) {
                    if let Some(block_num) = prim.raw_tch_s_block {
                        if block_num == PhyBlockNum::Block2 && prim.data.len() == 216 {
                            if self.channel_scheduler.ul_circuit_dl_media_source(ts) == CircuitDlMediaSource::LocalParrot {
                                self.channel_scheduler
                                    .dl_schedule_raw_tch_s_half_slot_from_ul(ts, ts, None, block_num, prim.data);
                            } else {
                                self.channel_scheduler.dl_schedule_raw_tch_s_half_slot(ts, block_num, prim.data);
                            }
                        } else {
                            tracing::warn!(
                                "rx_tmd_prim: dropping DL raw TCH/S {:?} length {} on ts={} src={:?}",
                                block_num,
                                prim.data.len(),
                                ts,
                                src
                            );
                        }
                    } else if let Some(packed_bits) = pack_ul_acelp_bits(&prim.data) {
                        if self.channel_scheduler.ul_circuit_dl_media_source(ts) == CircuitDlMediaSource::LocalParrot {
                            self.channel_scheduler.dl_schedule_tmd_from_ul(ts, ts, None, packed_bits);
                        } else {
                            self.channel_scheduler.dl_schedule_tmd(ts, packed_bits);
                        }
                    } else {
                        tracing::warn!(
                            "rx_tmd_prim: dropping unsupported DL voice length {} on ts={} src={:?}",
                            prim.data.len(),
                            ts,
                            src
                        );
                    }
                } else {
                    tracing::warn!(
                        "rx_tmd_prim: dropping DL voice on inactive circuit ts={} src={:?} dltime={}",
                        ts,
                        src,
                        self.dltime
                    );
                }
            }
            // UL voice from LMAC → forward to Brew + cross-route (duplex) or loopback (simplex) to DL
            SapMsgInner::TmdCircuitDataInd(prim) => {
                let ts = prim.ts;
                let data = prim.data;
                let raw_tch_s_block = prim.raw_tch_s_block;

                if !(1..=4).contains(&ts) {
                    tracing::warn!("rx_tmd_prim: dropping UL voice on invalid ts={}", ts);
                    return;
                }
                if !self.channel_scheduler.circuit_is_active(Direction::Ul, ts) {
                    tracing::trace!("rx_tmd_prim: no active UL circuit on ts={}, dropping UL voice", ts);
                    return;
                }
                enum AcceptedUlMedia {
                    RawTchSHalfSlot { block_num: PhyBlockNum, type5_bits: Vec<u8> },
                    AcElp { original_bits: Vec<u8>, packed_bits: Vec<u8> },
                }

                let accepted_media = if let Some(block_num) = raw_tch_s_block {
                    if block_num == PhyBlockNum::Block2 && data.len() == 216 {
                        AcceptedUlMedia::RawTchSHalfSlot {
                            block_num,
                            type5_bits: data,
                        }
                    } else {
                        tracing::warn!(
                            "rx_tmd_prim: unsupported raw TCH/S block {:?} length {} on ts={}, skipping",
                            block_num,
                            data.len(),
                            ts
                        );
                        return;
                    }
                } else if let Some(packed_bits) = pack_ul_acelp_bits(&data) {
                    AcceptedUlMedia::AcElp {
                        original_bits: data,
                        packed_bits,
                    }
                } else {
                    tracing::warn!("rx_tmd_prim: unsupported UL voice length {} on ts={}, skipping", data.len(), ts);
                    return;
                };
                self.note_accepted_ul_media(ts);

                let defer_ul_media_during_hangtime = self.can_defer_ul_media_during_hangtime(ts);
                if self.channel_scheduler.is_hangtime(ts) && !defer_ul_media_during_hangtime {
                    tracing::debug!(
                        "rx_tmd_prim: dropping UL voice on ts={} during hangtime to keep U-plane stopped",
                        ts
                    );
                    return;
                }

                if self.private_simplex_waiting_for_floor_grant(ts) {
                    // EN 300 392-2 Annex D.4 delays caller authorization until
                    // the called D-CONNECT ACK is L2-acknowledged. The private
                    // simplex bearer may already be open, but U-plane speech
                    // must wait for CMCE FloorGranted to identify the speaker.
                    match accepted_media {
                        AcceptedUlMedia::RawTchSHalfSlot { block_num, type5_bits } => {
                            self.defer_private_ul_media(ts, ts, PendingPrivateUlMediaKind::RawTchSHalfSlot { block_num, type5_bits });
                        }
                        AcceptedUlMedia::AcElp { packed_bits, .. } => {
                            self.defer_private_ul_media(ts, ts, PendingPrivateUlMediaKind::AcElp { packed_bits });
                        }
                    }
                    return;
                }

                let mut delivered_media = false;

                // Forward valid full-slot ACELP UL voice to Brew (User plane) if loaded.
                // Do this after validating the TMD payload so unsupported local
                // media cannot mask inactivity or leak as clean speech to Brew.
                if self.channel_scheduler.ul_circuit_dl_media_source(ts) == CircuitDlMediaSource::LocalParrot {
                    match accepted_media {
                        AcceptedUlMedia::RawTchSHalfSlot { block_num, type5_bits } => {
                            queue.push_back(SapMsg {
                                sap: Sap::TmdSap,
                                src: TetraEntity::Umac,
                                dest: TetraEntity::Cmce,
                                msg: SapMsgInner::TmdCircuitDataInd(tetra_saps::tmd::TmdCircuitDataInd {
                                    ts,
                                    data: type5_bits,
                                    raw_tch_s_block: Some(block_num),
                                }),
                            });
                        }
                        AcceptedUlMedia::AcElp { original_bits, .. } => {
                            queue.push_back(SapMsg {
                                sap: Sap::TmdSap,
                                src: TetraEntity::Umac,
                                dest: TetraEntity::Cmce,
                                msg: SapMsgInner::TmdCircuitDataInd(tetra_saps::tmd::TmdCircuitDataInd {
                                    ts,
                                    data: original_bits,
                                    raw_tch_s_block: None,
                                }),
                            });
                        }
                    }
                    self.last_ul_voice[ts as usize - 1] = Some(self.dltime);
                    return;
                }

                if let AcceptedUlMedia::AcElp { original_bits, .. } = &accepted_media
                    && self.config.config().brew.is_some()
                {
                    queue.push_back(SapMsg {
                        sap: Sap::TmdSap,
                        src: TetraEntity::Umac,
                        dest: TetraEntity::Brew,
                        msg: SapMsgInner::TmdCircuitDataInd(tetra_saps::tmd::TmdCircuitDataInd {
                            ts,
                            data: original_bits.clone(),
                            raw_tch_s_block: None,
                        }),
                    });
                    delivered_media = true;
                }

                // Determine DL target timeslot:
                //   - Full-duplex P2P (local): UL on `ts` cross-routed to peer MS's DL on `peer_ts`.
                //   - Group / simplex (LocalLoopback, no peer_ts): same-ts loopback so all members hear.
                //   - Circuit call via Brew/TetraPack (SwMI, no peer_ts): suppress local loopback.
                //     DL audio comes from Brew via TmdCircuitDataReq; looping back UL here causes the
                //     calling MS to hear their own voice instead of the remote party.
                // Refresh the peer's UL inactivity timer so the remote MS isn't timed out while
                // only the other party is talking.
                let dl_target_ts = match self.channel_scheduler.ul_circuit_peer_ts(ts) {
                    Some(peer_ts) => {
                        tracing::debug!("rx_tmd_prim: duplex P2P cross-route UL ts={} -> DL ts={}", ts, peer_ts);
                        peer_ts
                    }
                    None => {
                        if matches!(
                            self.channel_scheduler.ul_circuit_dl_media_source(ts),
                            CircuitDlMediaSource::SwMI | CircuitDlMediaSource::LocalParrot
                        ) {
                            // Circuit call via Brew: DL comes from TetraPack, not local loopback.
                            // Suppress UL->DL reflection so the caller doesn't hear their own voice.
                            tracing::debug!(
                                "rx_tmd_prim: circuit call ts={}, suppressing local UL loopback ({:?})",
                                ts,
                                self.channel_scheduler.ul_circuit_dl_media_source(ts)
                            );
                            if delivered_media {
                                self.last_ul_voice[ts as usize - 1] = Some(self.dltime);
                            }
                            return;
                        }
                        ts
                    }
                };

                if self.channel_scheduler.is_hangtime(dl_target_ts) && !defer_ul_media_during_hangtime {
                    tracing::debug!(
                        "rx_tmd_prim: dropping UL voice from ts={} because DL target ts={} is in hangtime",
                        ts,
                        dl_target_ts
                    );
                    return;
                }

                if self.channel_scheduler.circuit_is_active(Direction::Dl, dl_target_ts) {
                    let defer_for_hangtime = defer_ul_media_during_hangtime
                        && (self.channel_scheduler.is_hangtime(ts) || self.channel_scheduler.is_hangtime(dl_target_ts));
                    match accepted_media {
                        AcceptedUlMedia::RawTchSHalfSlot { block_num, type5_bits } => {
                            tracing::debug!(
                                "UMAC voice route: UL ts={} deferring raw TCH/S {:?} bits={} -> DL ts={} peer_ts={:?} media_source={:?}",
                                ts,
                                block_num,
                                type5_bits.len(),
                                dl_target_ts,
                                self.channel_scheduler.ul_circuit_peer_ts(ts),
                                self.channel_scheduler.ul_circuit_dl_media_source(ts)
                            );
                            // EN 300 392-2 clause 23.5 permits STCH/FACCH in
                            // one half-slot while TCH/S remains in the other,
                            // and clause 23.8.5 requires preserving a valid
                            // non-stolen TCH/S half-slot. Defer raw Block2
                            // until same-burst STCH has drained through CMCE,
                            // so U-TX CEASED/FloorReleased cannot race stale
                            // speech into the downlink scheduler.
                            self.defer_private_ul_media(
                                ts,
                                dl_target_ts,
                                PendingPrivateUlMediaKind::RawTchSHalfSlot { block_num, type5_bits },
                            );
                        }
                        AcceptedUlMedia::AcElp {
                            original_bits,
                            packed_bits,
                        } => {
                            tracing::debug!(
                                "UMAC voice route: UL ts={} bits={} -> DL ts={} packed_bytes={} peer_ts={:?} media_source={:?}",
                                ts,
                                original_bits.len(),
                                dl_target_ts,
                                packed_bits.len(),
                                self.channel_scheduler.ul_circuit_peer_ts(ts),
                                self.channel_scheduler.ul_circuit_dl_media_source(ts)
                            );
                            if defer_for_hangtime {
                                // EN 300 392-2 clauses 14.5.1.4.2 and 23.8.5:
                                // once the SwMI grants the private floor, the first
                                // valid TCH/S frame from that speaker must not be
                                // discarded only because lower-layer grant state was
                                // still draining through FACCH/STCH.
                                self.defer_private_ul_media(ts, dl_target_ts, PendingPrivateUlMediaKind::AcElp { packed_bits });
                            } else {
                                self.channel_scheduler.dl_schedule_tmd_from_ul(
                                    dl_target_ts,
                                    ts,
                                    self.ul_media_speaker_tag(ts),
                                    packed_bits,
                                );
                                delivered_media = true;
                            }
                        }
                    }
                } else {
                    tracing::debug!(
                        "rx_tmd_prim: no active DL circuit on ts={} (UL src ts={}), skipping",
                        dl_target_ts,
                        ts
                    );
                }
                if delivered_media {
                    self.last_ul_voice[ts as usize - 1] = Some(self.dltime);
                    if let Some(peer_ts) = self.channel_scheduler.ul_circuit_peer_ts(ts)
                        && (1..=4).contains(&peer_ts)
                        && self.channel_scheduler.circuit_is_active(Direction::Ul, peer_ts)
                    {
                        self.last_ul_voice[peer_ts as usize - 1] = Some(self.dltime);
                    }
                }
            }
            _ => {
                tracing::warn!("rx_tmd_prim: unexpected message type");
            }
        }
    }

    fn signal_lmac_second_half_stolen(&mut self, queue: &mut MessageQueue, ul_time: TdmaTime) {
        // Signal LMAC that Block2 is also stolen (STCH, not TCH).
        // Must be Immediate priority so LMAC sees it before processing Block2.
        // EN 300 392-2 clause 21.4.5 scopes the stolen second half to this
        // received traffic burst; pass the UL time so LMAC does not apply the
        // indication to a later private-simplex TCH/S burst on another slot.
        let m = SapMsg {
            sap: Sap::TmvSap,
            src: self.self_component,
            dest: TetraEntity::Lmac,
            msg: SapMsgInner::TmvConfigureReq(TmvConfigureReq {
                blk2_stolen: Some(true),
                time: Some(ul_time),
                ..Default::default()
            }),
        };
        queue.push_prio(m, MessagePrio::Immediate);
    }

    // fn rx_stch_second_half(&mut self, queue: &mut MessageQueue, message: &mut SapMsg, pending: PendingStch) {
    //     let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
    //         panic!()
    //     };

    //     // Sanity checks
    //     assert!(prim.logical_channel == LogicalChannel::Stch, "rx_stch_second_half: expected STCH logical channel, got {:?}", prim.logical_channel);
    //     assert!(prim.block_num == PhyBlockNum::Block2, "rx_stch_second_half: expected Block2, got {:?}", prim.block_num);
    //     assert!(self.pending_stch.is_some(), "rx_stch_second_half: no pending STCH, cannot process second half");

    //     let mut first = pending.sdu_part;
    //     first.seek(0);
    //     let first_len = first.get_len_remaining();
    //     prim.pdu.seek(0);
    //     let second_len = prim.pdu.get_len_remaining();

    //     self.rx_mac_access(queue, message);

    //     let mut combined = BitBuffer::new(first_len + second_len);
    //     combined.copy_bits(&mut first, first_len);
    //     combined.copy_bits(&mut prim.pdu, second_len);
    //     combined.seek(0);

    //     if pending.fill_bits {
    //         let total_len = combined.get_len();
    //         let num_fill_bits = fillbits::removal::get_num_fill_bits(&combined, total_len, false);
    //         if num_fill_bits > 0 {
    //             combined.set_raw_end(total_len - num_fill_bits);
    //         }
    //         combined.seek(0);
    //     }

    //     let m = SapMsg {
    //         sap: Sap::TmaSap,
    //         src: TetraEntity::Umac,
    //         dest: TetraEntity::Llc,
    //         dltime: message.dltime,
    //         msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
    //             pdu: Some(combined),
    //             main_address: pending.addr,
    //             scrambling_code: pending.scrambling_code,
    //             endpoint_id: 0,
    //             new_endpoint_id: None,
    //             css_endpoint_id: None,
    //             air_interface_encryption: pending.encrypted as Todo,
    //             chan_change_response_req: false,
    //             chan_change_handle: None,
    //             chan_info: None,
    //         }),
    //     };
    //     queue.push_back(m);
    // }

    fn rx_control_circuit_open(&mut self, _queue: &mut MessageQueue, prim: CallControl) {
        let CallControl::Open(circuit) = prim else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let ts = circuit.ts;
        let dir = circuit.direction;
        self.discard_pending_private_ul_media_involving(ts, "new circuit open/replacement");

        // Direction::Both needs to be split into separate DL and UL operations
        // because the UMAC circuit manager tracks them independently.
        let dirs: Vec<Direction> = match dir {
            Direction::Both => vec![Direction::Dl, Direction::Ul],
            d @ (Direction::Dl | Direction::Ul) => vec![d],
            Direction::None => {
                tracing::warn!("rx_control_circuit_open: Direction::None, ignoring");
                return;
            }
        };

        let mut replaced_suspensions = HashSet::new();
        let requested_active_addrs: Vec<_> = circuit.active_addresses().collect();
        for d in dirs {
            // See if pre-existing circuit somehow needs to be closed
            if self.channel_scheduler.circuit_is_active(d, ts) {
                tracing::warn!("rx_control_circuit_open: Circuit already exists for {:?} {}, closing first", d, ts);
                if let Some(old_circuit) = self.channel_scheduler.close_circuit(d, ts) {
                    for addr in old_circuit.active_addresses() {
                        replaced_suspensions.insert(EnergySavingSuspensionKey { ts, addr });
                    }
                }
            }

            let c = Circuit {
                direction: d,
                ts: circuit.ts,
                peer_ts: circuit.peer_ts,
                usage: circuit.usage,
                circuit_mode: circuit.circuit_mode,
                speech_service: circuit.speech_service,
                etee_encrypted: circuit.etee_encrypted,
                dl_media_source: circuit.dl_media_source,
                active_addr: circuit.active_addr,
                active_secondary_addrs: circuit.active_secondary_addrs.clone(),
            };
            self.channel_scheduler.create_circuit(d, c);

            // Start UL inactivity timer when opening a UL circuit
            if d == Direction::Ul && (1..=4).contains(&ts) {
                self.last_ul_voice[ts as usize - 1] = Some(self.dltime);
                self.clear_current_ul_speaker(ts);
                self.reset_ul_media_diagnostic(ts);
                if let Some(speaker_addr) = Self::initial_ul_speaker_for_open_circuit(&circuit) {
                    self.set_current_ul_speaker(ts, speaker_addr);
                }
            }

            tracing::info!(
                "rx_control_circuit_open: opened {:?} ts={} usage={} mode={:?} speech={:?} peer_ts={:?} media_source={:?} active_addrs={:?}",
                d,
                ts,
                circuit.usage,
                circuit.circuit_mode,
                circuit.speech_service,
                circuit.peer_ts,
                circuit.dl_media_source,
                requested_active_addrs
            );
        }
        for key in replaced_suspensions {
            self.resume_energy_saving_for_suspension_key_if_unowned(key);
        }
        self.suspend_energy_saving_for_circuit(ts, &circuit);
    }

    fn rx_control_circuit_close(&mut self, _queue: &mut MessageQueue, prim: CallControl) {
        let CallControl::Close(dir, ts) = prim else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        self.discard_pending_private_ul_media_involving(ts, "circuit close");

        // Direction::Both needs to be split into separate DL and UL close operations
        let dirs: Vec<Direction> = match dir {
            Direction::Both => vec![Direction::Dl, Direction::Ul],
            d @ (Direction::Dl | Direction::Ul) => vec![d],
            Direction::None => {
                tracing::warn!("rx_control_circuit_close: Direction::None, ignoring");
                return;
            }
        };

        let mut closed_suspensions = HashSet::new();
        for d in dirs {
            match self.channel_scheduler.close_circuit(d, ts) {
                Some(circuit) => {
                    for addr in circuit.active_addresses() {
                        closed_suspensions.insert(EnergySavingSuspensionKey { ts, addr });
                    }
                    if d == Direction::Dl {
                        self.channel_scheduler.dl_discard_pending_stealing(ts, "DL circuit close");
                    }
                    // Clear UL inactivity timer when closing a UL circuit
                    if d == Direction::Ul && (1..=4).contains(&ts) {
                        self.last_ul_voice[ts as usize - 1] = None;
                        self.clear_current_ul_speaker(ts);
                        self.reset_ul_media_diagnostic(ts);
                    }
                    tracing::info!("  rx_control_circuit_close: Closed {:?} circuit for ts {}", d, ts);
                }
                None => {
                    tracing::warn!("  rx_control_circuit_close: No {:?} circuit to close for ts {}", d, ts);
                }
            }
        }
        for key in closed_suspensions {
            self.resume_energy_saving_for_suspension_key_if_unowned(key);
        }
    }

    /// Check for UL inactivity on traffic timeslots. If no voice frames have arrived
    /// for UL_INACTIVITY_TIMESLOTS on a timeslot with an active UL circuit (and not in
    /// hangtime), send UlInactivityTimeout to CMCE.
    fn check_ul_inactivity(&mut self, queue: &mut MessageQueue) {
        // Read from config: ul_inactivity_secs * timeslots_per_second (72 = 18 frames * 4 slots)
        // Must be above T.213 (1s) to tolerate DTX and brief RF fading.
        let ul_inactivity_timeslots: i32 = self.config.config().cell.ul_inactivity_secs as i32 * 18 * 4;

        for ts in 1..=4u8 {
            let idx = ts as usize - 1;

            // Only check timeslots with an active UL circuit
            if !self.channel_scheduler.circuit_is_active(Direction::Ul, ts) {
                continue;
            }

            // Skip if in hangtime (no voice expected)
            if self.channel_scheduler.is_hangtime(ts) {
                continue;
            }

            // Check if we've exceeded the inactivity threshold
            let timed_out = match self.last_ul_voice[idx] {
                Some(t) => t.age(self.dltime) > ul_inactivity_timeslots,
                None => false, // Initialized at circuit open; shouldn't be None here
            };

            if timed_out {
                tracing::warn!(
                    "UL inactivity timeout on ts={}, accepted_ul_media_since_floor={}, sending notification to CMCE",
                    ts,
                    self.ul_media_events_since_floor[idx]
                );
                self.last_ul_voice[idx] = None;

                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Umac,
                    dest: TetraEntity::Cmce,
                    msg: SapMsgInner::CmceCallControl(CallControl::UlInactivityTimeout { ts }),
                });
            }
        }
    }

    fn rx_control(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_control");
        let SapMsgInner::CmceCallControl(prim) = message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        match prim {
            CallControl::Open(_) => {
                self.rx_control_circuit_open(queue, prim);
            }
            CallControl::SetDlMediaSource { ts, dl_media_source } => {
                if self.channel_scheduler.set_circuit_dl_media_source(ts, dl_media_source) {
                    tracing::info!("UMAC: set DL media source ts={} media_source={:?}", ts, dl_media_source);
                } else {
                    tracing::warn!(
                        "UMAC: ignoring DL media source update for inactive circuit ts={} media_source={:?}",
                        ts,
                        dl_media_source
                    );
                }
            }
            CallControl::Close(_, _) => {
                self.rx_control_circuit_close(queue, prim);
            }
            // Floor-control signals drive traffic↔signalling transitions during hangtime.
            CallControl::FloorReleased { ts, .. } => {
                for floor_ts in self.floor_media_timeslots(ts).into_iter().flatten() {
                    self.discard_pending_private_ul_media_involving(floor_ts, "floor released; U-plane enters hangtime");
                    self.channel_scheduler
                        .clear_dl_media_queue(floor_ts, "floor released; U-plane enters hangtime");
                    self.channel_scheduler.set_hangtime(floor_ts, true);
                    // Stop checking UL inactivity during hangtime. For crossed
                    // P2P media, the downlink target timeslot can still hold
                    // queued old-speaker TCH/S, so clear both affected slots.
                    self.last_ul_voice[floor_ts as usize - 1] = None;
                    self.clear_current_ul_speaker(floor_ts);
                    self.reset_ul_media_diagnostic(floor_ts);
                }
            }
            CallControl::FloorGranted {
                call_id,
                source_issi,
                dest_gssi,
                ts,
            } => {
                // Restart UL inactivity timer when new speaker gets floor
                if (1..=4).contains(&ts) {
                    let source_addr = TetraAddress::issi(source_issi);
                    let private_participant_scoped = self.channel_scheduler.ul_circuit_is_private_participant_scoped(ts);
                    let source_is_local_participant = self.channel_scheduler.circuit_is_active_for_addr(Direction::Ul, ts, source_addr);
                    let swmi_downlink_floor = private_participant_scoped
                        && !source_is_local_participant
                        && self.channel_scheduler.ul_circuit_dl_media_source(ts) == CircuitDlMediaSource::SwMI;
                    if private_participant_scoped && !source_is_local_participant && !swmi_downlink_floor {
                        tracing::warn!(
                            "UMAC: ignoring FloorGranted for non-participant ISSI {} on private UL ts {}",
                            source_issi,
                            ts
                        );
                        return;
                    }
                    if !swmi_downlink_floor {
                        self.set_current_ul_speaker(ts, source_addr);
                    }
                    for floor_ts in self.floor_media_timeslots(ts).into_iter().flatten() {
                        if private_participant_scoped {
                            if swmi_downlink_floor {
                                self.discard_pending_private_ul_media_involving(
                                    floor_ts,
                                    "new external SwMI private floor grant; discard local stale media",
                                );
                                // Brew may deliver the first remote speech frames before
                                // the matching circuit SIMPLEX_GRANTED reaches UMAC. On a
                                // SwMI-fed private bearer the DL queue contains network
                                // media, not locally looped-back stale speaker media.
                            } else {
                                self.discard_pending_private_ul_media_except_source(
                                    floor_ts,
                                    ts,
                                    source_addr,
                                    "new private floor grant; discard previous speaker media",
                                );
                                self.channel_scheduler.clear_dl_media_queue_except_source(
                                    floor_ts,
                                    ts,
                                    source_addr,
                                    "new private floor grant; discard previous speaker media",
                                );
                            }
                        } else {
                            self.discard_pending_group_ul_media_except_hangtime_source(
                                floor_ts,
                                ts,
                                source_addr,
                                "new group floor grant; discard previous speaker media",
                            );
                            self.channel_scheduler
                                .clear_dl_media_queue(floor_ts, "new floor grant; discard previous speaker media");
                        }
                        self.channel_scheduler.set_hangtime(floor_ts, false);
                    }
                    self.flush_pending_private_ul_media();
                    if !private_participant_scoped {
                        let group_addr = TetraAddress::new(dest_gssi, SsiType::Gssi);
                        let removed = self
                            .channel_scheduler
                            .dl_drop_queued_gssi_repeats(group_addr, "floor granted to a new/current speaker");
                        if removed > 0 {
                            tracing::debug!(
                                "UMAC: dropped {} stale GSSI repeat item(s) for {} after floor grant call_id={} source_issi={}",
                                removed,
                                group_addr,
                                call_id,
                                source_issi
                            );
                        }
                    }
                    self.last_ul_voice[ts as usize - 1] = if swmi_downlink_floor { None } else { Some(self.dltime) };
                    self.reset_ul_media_diagnostic(ts);
                    tracing::info!(
                        "UMAC floor granted: call_id={} source_issi={} dest_gssi={} ul_ts={} peer_ts={:?} media_source={:?} private_participant_scoped={}",
                        call_id,
                        source_issi,
                        dest_gssi,
                        ts,
                        self.channel_scheduler.ul_circuit_peer_ts(ts),
                        self.channel_scheduler.ul_circuit_dl_media_source(ts),
                        private_participant_scoped
                    );
                }
            }
            CallControl::CallEnded { ts, .. } => {
                for floor_ts in self.floor_media_timeslots(ts).into_iter().flatten() {
                    self.discard_pending_private_ul_media_involving(floor_ts, "call ended");
                    self.channel_scheduler.clear_dl_media_queue(floor_ts, "call ended");
                    self.channel_scheduler.set_hangtime(floor_ts, false);
                    self.last_ul_voice[floor_ts as usize - 1] = None;
                    self.clear_current_ul_speaker(floor_ts);
                    self.reset_ul_media_diagnostic(floor_ts);
                }
            }

            // UlInactivityTimeout is UMAC→CMCE only, UMAC won't receive it back
            CallControl::UlInactivityTimeout { .. } => {}

            // NetworkCall* and NetworkCircuit* are for CMCE ↔ Brew, not UMAC
            CallControl::NetworkCallStart { .. }
            | CallControl::NetworkCallReady { .. }
            | CallControl::NetworkCallEnd { .. }
            | CallControl::NetworkCircuitSetupRequest { .. }
            | CallControl::NetworkCircuitSetupAccept { .. }
            | CallControl::NetworkCircuitSetupReject { .. }
            | CallControl::NetworkCircuitAlert { .. }
            | CallControl::NetworkCircuitConnectRequest { .. }
            | CallControl::NetworkCircuitConnectConfirm { .. }
            | CallControl::NetworkCircuitSimplexGranted { .. }
            | CallControl::NetworkCircuitSimplexIdle { .. }
            | CallControl::NetworkCircuitMediaReady { .. }
            | CallControl::NetworkCircuitDtmf { .. }
            | CallControl::NetworkCircuitRelease { .. } => {
                tracing::trace!("rx_control: ignoring CMCE-Brew notification (not for UMAC)");
            }
        }
    }
}

impl TetraEntityTrait for UmacBs {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Umac
    }

    fn set_config(&mut self, config: SharedConfig) {
        self.config = config;
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        // tracing::debug!("rx_prim: {:?}", message);
        // tracing::debug!(ts=%message.dltime, "rx_prim: {:?}", message);

        match message.sap {
            Sap::TmvSap => {
                self.rx_tmv_prim(queue, message);
            }
            Sap::TmaSap => {
                self.rx_tma_prim(queue, message);
            }
            Sap::TmdSap => {
                self.rx_tmd_prim(queue, message);
            }
            Sap::TlmbSap => {
                self.rx_tlmb_prim(queue, message);
            }
            Sap::TlmcSap => {
                self.rx_tlmc_prim(queue, message);
            }
            Sap::Control => {
                self.rx_control(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        self.dltime = ts;
        self.refresh_system_wide_services();
        self.defrag.age_buffers(ts);

        if self.channel_scheduler.cur_dltime != ts && self.channel_scheduler.cur_dltime == (TdmaTime { t: 0, f: 0, m: 0, h: 0 }) {
            // Upon start of the system, we need to set the dl time for the channel scheduler
            self.channel_scheduler.set_dl_time(ts);
            self.config.state_write().timeslot_alloc.release_all(TimeslotOwner::PacketData);
        } else {
            // When running, we adopt the new time and check for desync
            self.channel_scheduler.tick_start(ts);
        }

        // Check for UL inactivity (stuck transmitter detection)
        self.check_ul_inactivity(queue);

        self.flush_pending_private_ul_media();

        // Collect/construct traffic that should be sent down to the LMAC
        // This is basically the _previous_ timeslot
        let elem = {
            let mut state = self.config.state_write();
            let tetra_config::bluestation::StackState {
                subscribers,
                energy_saving,
                timeslot_alloc,
                ..
            } = &mut *state;
            self.channel_scheduler
                .finalize_ts_for_tick_with_timeslot_allocator(subscribers, energy_saving, timeslot_alloc)
        };
        let s = SapMsg {
            sap: Sap::TmvSap,
            src: self.self_component,
            dest: TetraEntity::Lmac,
            msg: SapMsgInner::TmvUnitdataReq(elem),
        };
        tracing::trace!("UmacBs tick: Pushing finalized timeslot to LMAC: {:?}", s);
        queue.push_back(s);
        self.emit_completed_tma_reports(queue);
        let mut stats = self.channel_scheduler.health_stats();
        stats.pending_tma_reports = self.pending_tma_reports.len();
        stats.pending_private_ul_media_total = self.pending_private_ul_media.iter().map(VecDeque::len).sum();
        stats.pending_stch = self.pending_stch.is_some();
        crate::health::registry().set_umac_stats(stats);
    }
}

/// Pack UL ACELP voice bits (274 bits, one-bit-per-byte) into packed byte array for DL transmission.
/// Handles both already-packed (35 bytes) and unpacked (274 bytes) formats.
fn pack_ul_acelp_bits(bits: &[u8]) -> Option<Vec<u8>> {
    const PACKED_TCH_S_BYTES: usize = (TCH_S_CAP + 7) / 8;

    // Already packed format — pass through
    if bits.len() == PACKED_TCH_S_BYTES {
        return Some(bits.to_vec());
    }
    // Insufficient data
    if bits.len() < TCH_S_CAP {
        return None;
    }

    // Pack 274 one-bit-per-byte into 35 bytes (last byte has 2 padding bits)
    let mut out = Vec::with_capacity(PACKED_TCH_S_BYTES);
    for chunk_idx in 0..PACKED_TCH_S_BYTES {
        let mut byte = 0u8;
        for bit in 0..8 {
            let bit_idx = chunk_idx * 8 + bit;
            if bit_idx < TCH_S_CAP {
                byte |= (bits[bit_idx] & 1) << (7 - bit);
            }
        }
        out.push(byte);
    }
    Some(out)
}
