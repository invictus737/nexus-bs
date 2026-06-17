// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{
    BitBuffer, EndpointId, Layer2Service, LinkId, Sap, SsiType, TdmaTime, TetraAddress, Todo, TxReporter, TxState, unimplemented_log,
};
use tetra_saps::lcmc::enums::alloc_type::ChanAllocType;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::tla::{
    TLA_REPORT_FAILED_TRANSFER, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION, TLA_REPORT_NO_SPECIFIC_REPORT, TLA_REPORT_SUCCESSFUL_TRANSFER,
    TlDataConfBl, TlaTlDataIndBl, TlaTlDataReqBl, TlaTlReportInd, TlaTlUnitdataIndBl, TlaTlUnitdataReqBl,
};
use tetra_saps::tma::{TmaCancelReq, TmaReport, TmaReportInd, TmaUnitdataInd, TmaUnitdataReq};
use tetra_saps::{SapMsg, SapMsgInner};

use crate::llc::components::fcs;
use tetra_pdus::llc::consts::consts::{
    N251_BL_MAX_TLSDU_LEN_BITS, N252_BL_MAX_TLSDU_RETRANSMITS_ACKED, N253_BL_MAX_TLSDU_REPETITIONS_UNACKED,
};
use tetra_pdus::llc::consts::timers::{T251_SENDER_RETRY_TIMER, T252_ACK_WAITING_TIMER};
use tetra_pdus::llc::enums::llc_pdu_type::LlcPduType;
use tetra_pdus::llc::pdus::al_ack::AlAck;
use tetra_pdus::llc::pdus::al_data::AlData;
use tetra_pdus::llc::pdus::al_setup::AlSetup;
use tetra_pdus::llc::pdus::bl_ack::BlAck;
use tetra_pdus::llc::pdus::bl_adata::BlAdata;
use tetra_pdus::llc::pdus::bl_data::BlData;
use tetra_pdus::llc::pdus::bl_udata::BlUdata;
use tetra_pdus::umac::fields::channel_allocation::ChanAllocElement;
use tetra_pdus::umac::pdus::mac_resource::MacResource;

use crate::umac::subcomp::bs_sched::SCH_F_CAP;

const TDMA_TIMESLOTS_PER_FRAME: u32 = 4;
const T251_SENDER_RETRY_SIGNALLING_FRAMES: u32 = T251_SENDER_RETRY_TIMER / TDMA_TIMESLOTS_PER_FRAME;
const T252_AL_ACK_WAITING_SIGNALLING_FRAMES: u32 = T252_ACK_WAITING_TIMER / TDMA_TIMESLOTS_PER_FRAME;
const INBOUND_DUPLICATE_SUPPRESSION_SIGNALLING_FRAMES: u32 =
    (N252_BL_MAX_TLSDU_RETRANSMITS_ACKED as u32 + 1) * T251_SENDER_RETRY_SIGNALLING_FRAMES;
const CHANNEL_ALLOCATION_LATE_ACK_GRACE_SIGNALLING_FRAMES: u32 = 18;
const AL_LATE_ACK_GRACE_SIGNALLING_FRAMES: u32 = 72;
const N253_MAX_REQUESTED_TLSDU_REPEATS: u8 = 5;
const TMA_HIGHEST_PDU_PRIORITY: Todo = 7;
const COMMON_CONTROL_TIMESLOT: u8 = 1;
const FIRST_PACKET_DATA_TIMESLOT: u8 = 2;
const AL_SETUP_PHASE_MOD_PDCH_MAX_TIMESLOTS: u8 = 1;
const AL_ORIGINAL_DATA_HEADER_BITS: usize = 17;
const AL_ORIGINAL_SEGMENTS_PER_ACK_REQUEST: usize = 16;
pub const LLC_MAX_OUTBOUND_ACKED_MESSAGES: usize = 8192;
pub const LLC_MAX_OUTBOUND_UDATA_MESSAGES: usize = 8192;

const _: () = assert!(T251_SENDER_RETRY_TIMER % TDMA_TIMESLOTS_PER_FRAME == 0);
const _: () = assert!(T252_ACK_WAITING_TIMER % TDMA_TIMESLOTS_PER_FRAME == 0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct BasicLinkKey {
    addr: TetraAddress,
    endpoint_id: EndpointId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct AdvancedLinkKey {
    addr: TetraAddress,
    endpoint_id: EndpointId,
    link_id: LinkId,
}

#[derive(Debug, Clone, Copy)]
struct ReceiveSeqState {
    last_ns: u8,
    received_at: TdmaTime,
    ack_timeslot: u8,
}

#[derive(Debug, Clone, Copy)]
enum TmaReportOwner {
    BlData(usize),
    BlUdata(usize),
    AlData(usize, usize),
}

struct RebuildableAckTransfer {
    has_fcs: bool,
    tl_sdu: BitBuffer,
    embedded_nr: Option<u8>,
}

#[derive(Debug, Clone)]
struct AdvancedLinkSession {
    key: AdvancedLinkKey,
    al_number: u8,
    max_tl_sdu_retransmissions: u8,
    max_segment_retransmissions: u8,
    tx_next_ns: u8,
    last_delivered_ns: Option<u8>,
}

#[derive(Debug, Clone)]
struct AdvancedLinkReceiveAssembly {
    key: AdvancedLinkKey,
    ns: u8,
    segments: BTreeMap<u8, BitBuffer>,
    final_ss: Option<u8>,
}

struct ExpectedAlSegment {
    ss: u8,
    req_handle: Todo,
    t_submitted_to_umac: Option<TdmaTime>,
    t_umac_done: Option<TdmaTime>,
    current_mac_reporter: Option<TxReporter>,
    acknowledged: bool,
    retransmission_requested: bool,
    retransmit_count: u8,
    retransmission_buf: SapMsg,
}

struct ExpectedAlAck {
    key: AdvancedLinkKey,
    ns: u8,
    req_handle: Todo,
    pdu_prio: Todo,
    fcs_flag: bool,
    air_interface_encryption: Todo,
    t_first: TdmaTime,
    t_submitted_to_umac: Option<TdmaTime>,
    t_umac_done: Option<TdmaTime>,
    tx_reporter: TxReporter,
    current_mac_reporter: Option<TxReporter>,
    ack_timeslot: u8,
    ack_stealing_repeats_flag: Option<bool>,
    segments: Vec<ExpectedAlSegment>,
    retransmit_count: u8,
    first_complete_report_sent: bool,
    t_retransmissions_exhausted: Option<TdmaTime>,
    selective_segment_retry_pending: bool,
}

/// Struct that maintains state expected acknowledgement data for a transmitted message.
/// Aka, we still expect an ack for this.
pub struct ExpectedInAck {
    /// Timeslot on which the original message was sent
    pub ts: u8,
    /// Address to which the message was sent
    pub addr: TetraAddress,

    /// Expected ack sequence number for the original message
    pub ns: u8,

    /// TMA request handle used by MAC reports for this transfer.
    pub req_handle: Todo,
    pub link_id: LinkId,
    pub endpoint_id: EndpointId,
    pub pdu_prio: Todo,
    pub fcs_flag: bool,
    pub air_interface_encryption: Todo,
    /// EN 300 392-2 Annex A.1: for T.251, when the PDU was sent with the
    /// stealing repeats flag while the MS is transmitting traffic, count all
    /// downlink frames instead of only the monitored signalling frames.
    pub stealing_repeats_flag: Option<bool>,

    pub bl_type: Layer2Service,

    /// Time this message was received from the MLE
    pub t_first: TdmaTime,
    /// Time this message was actually passed down to the Umac. If a previous message on the basic link is already
    /// submitted, the message has to wait until that previous message was sent and acknowledged, or lost.
    pub t_submitted_to_umac: Option<TdmaTime>,
    /// Time the RxReporter signalled the message was fully transmitted. Also set if the Umac discarded the message
    /// This helps attempting to retransmit the message after a brief delay.
    pub t_umac_done: Option<TdmaTime>,
    /// Time when N.252 retransmissions were exhausted but the transfer is
    /// still retained for a bounded late BL-ACK on channel-allocation flows.
    pub t_retransmissions_exhausted: Option<TdmaTime>,
    /// Service-level TxReporter exposed to the LLC user. It moves to
    /// Transmitted after the first complete MAC transmission, then to
    /// Acknowledged or Lost according to the peer BL-ACK result.
    pub tx_reporter: TxReporter,
    /// Per-TMA-attempt TxReporter used only between UMAC and LLC. A retry gets
    /// a fresh reporter so stale MAC completion for an older attempt cannot
    /// mutate the service-level reporter.
    pub current_mac_reporter: Option<TxReporter>,

    // Optional retransmission buffer, to allow for automatic retransmission of the PDU if no acknowledgement is received
    pub retransmission_buf: SapMsg,
    /// Number of retransmissions performed so far
    pub retransmit_count: u8,
    pub first_complete_report_sent: bool,
}

/// Struct that maintains state for an ACK we still need to send back.
#[derive(Debug, Clone)]
pub struct ScheduledOutAck {
    pub addr: TetraAddress,
    pub t_start: TdmaTime,
    /// Received sequence number
    pub nr: u8,
    /// LLC-generated handle from the corresponding TL-DATA.ind.
    pub ind_req_handle: Todo,
    /// TLA/MLE endpoint that delivered the BL-DATA needing acknowledgement.
    pub endpoint_id: EndpointId,
    /// Air-interface encryption context copied from the received TM-SDU.
    pub air_interface_encryption: Todo,
    /// Timeslot on which the original message was received
    pub ts: u8,
}

pub struct QueuedUdata {
    pub addr: TetraAddress,
    pub t_first: TdmaTime,
    pub req_handle: Todo,
    pub endpoint_id: EndpointId,
    pub pdu_prio: Todo,
    pub sapmsg: SapMsg,
    pub service_tx_reporter: Option<TxReporter>,
    pub current_mac_reporter: Option<TxReporter>,
    pub n253: u8,
    pub target_complete_transmissions: u8,
    pub complete_transmissions: u8,
    pub failed_transmissions: u8,
    pub submitted: bool,
    pub defer_mac_ready_once: bool,
}

pub struct Llc {
    config: SharedConfig,
    dltime: TdmaTime,

    /// When we receive a message, and it needs to be acknowledged, we store it here for later
    /// integration into a response message, or we will make a separate BL-ACK for it.
    scheduled_out_acks: VecDeque<ScheduledOutAck>,

    /// Outbound messages that are either already submitted to UMAC and waiting
    /// for ACK, or queued behind a previous message on the same basic-link
    /// endpoint.
    outbound_messages: VecDeque<ExpectedInAck>,
    outbound_udata_messages: VecDeque<QueuedUdata>,
    outbound_al_messages: VecDeque<ExpectedAlAck>,

    /// Per-link send sequence variable per basic-link endpoint. Alternates between 0 and 1.
    link_send_seq: HashMap<BasicLinkKey, u8>,

    /// Last valid inbound acknowledged DATA sequence per basic-link endpoint.
    inbound_receive_seq: HashMap<BasicLinkKey, ReceiveSeqState>,
    advanced_links: HashMap<AdvancedLinkKey, AdvancedLinkSession>,
    pending_advanced_link_setups: HashMap<AdvancedLinkKey, AlSetup>,
    advanced_link_rx: HashMap<AdvancedLinkKey, AdvancedLinkReceiveAssembly>,

    /// LLC-generated handles for acknowledged TL-DATA.ind primitives. Keep
    /// them negative so they do not collide with local MLE request handles.
    next_tl_data_ind_req_handle: Todo,
}

impl Llc {
    pub fn new(config: SharedConfig) -> Self {
        Self {
            dltime: TdmaTime::default(),
            config,
            scheduled_out_acks: VecDeque::new(),
            outbound_messages: VecDeque::new(),
            outbound_udata_messages: VecDeque::new(),
            outbound_al_messages: VecDeque::new(),
            link_send_seq: HashMap::new(),
            inbound_receive_seq: HashMap::new(),
            advanced_links: HashMap::new(),
            pending_advanced_link_setups: HashMap::new(),
            advanced_link_rx: HashMap::new(),
            next_tl_data_ind_req_handle: -1,
        }
    }

    fn wap_ip_diag_enabled(&self) -> bool {
        let cfg = self.config.config();
        cfg.cell.sndcp_service && cfg.cell.wap_ip.as_ref().is_some_and(|wap| wap.enabled)
    }

    fn append_tl_sdu_and_optional_fcs(pdu_buf: &mut BitBuffer, tl_sdu: &mut BitBuffer, has_fcs: bool) {
        let tl_sdu_start = pdu_buf.get_len_written();
        let sdu_len = tl_sdu.get_len_remaining();
        pdu_buf.copy_bits(tl_sdu, sdu_len);
        if has_fcs {
            // EN 300 392-2 clauses 21.1.2.3 and 21.2.2 place the 32-bit LLC
            // FCS immediately after the TL-SDU when a basic-link FCS PDU
            // variant is selected.
            let fcs_value = fcs::compute_fcs(pdu_buf, tl_sdu_start, pdu_buf.get_len());
            pdu_buf.write_bits(fcs_value as u64, 32);
        }
        pdu_buf.seek(0);
    }

    fn n251_max_tl_sdu_bits(fcs_flag: bool) -> usize {
        // EN 300 392-2 Annex A N.251: maximum TL-SDU length is 2595 bits
        // when FCS is used; without FCS the TL-SDU part may be four octets
        // larger because those bits are not occupied by the FCS.
        let fcs_slack_bits = if fcs_flag { 0 } else { 32 };
        N251_BL_MAX_TLSDU_LEN_BITS as usize + fcs_slack_bits
    }

    fn tl_sdu_exceeds_n251(tl_sdu: &BitBuffer, fcs_flag: bool) -> bool {
        tl_sdu.get_len_remaining() > Self::n251_max_tl_sdu_bits(fcs_flag)
    }

    fn strip_validated_fcs(pdu: &mut BitBuffer) {
        let payload_end = pdu.get_raw_end() - 32;
        pdu.set_raw_end(payload_end);
    }

    fn cmce_to_mac_chanalloc_for_capacity(main_carrier: u16, chan_alloc: &CmceChanAllocReq) -> ChanAllocElement {
        let clch_permission = (chan_alloc.alloc_type == ChanAllocType::Replace || chan_alloc.alloc_type == ChanAllocType::Additional)
            && (chan_alloc.ul_dl_assigned == UlDlAssignment::Ul || chan_alloc.ul_dl_assigned == UlDlAssignment::Both);
        ChanAllocElement {
            alloc_type: chan_alloc.alloc_type,
            ts_assigned: chan_alloc.timeslots,
            ul_dl_assigned: chan_alloc.ul_dl_assigned,
            clch_permission,
            cell_change_flag: false,
            carrier_num: main_carrier,
            ext: None,
            mon_pattern: 0,
            frame18_mon_pattern: Some(0),
        }
    }

    fn sch_f_mac_resource_tm_sdu_capacity_bits(main_carrier: u16, addr: TetraAddress, chan_alloc: Option<&CmceChanAllocReq>) -> usize {
        let mac_chan_alloc = chan_alloc.map(|chan_alloc| Self::cmce_to_mac_chanalloc_for_capacity(main_carrier, chan_alloc));
        let usage_marker = chan_alloc.and_then(|chan_alloc| chan_alloc.usage);
        let mac_resource = MacResource {
            fill_bits: false,
            pos_of_grant: 0,
            encryption_mode: 0,
            random_access_flag: false,
            length_ind: 0,
            addr: Some(addr),
            event_label: None,
            usage_marker,
            power_control_element: None,
            slot_granting_element: None,
            chan_alloc_element: mac_chan_alloc,
        };

        SCH_F_CAP.saturating_sub(mac_resource.compute_header_len())
    }

    fn bl_adata_len_bits(tl_sdu: &BitBuffer, has_fcs: bool) -> usize {
        let mut header = BitBuffer::new_autoexpand(8);
        BlAdata { has_fcs, nr: 0, ns: 0 }.to_bitbuf(&mut header);
        header.get_len() + tl_sdu.get_len_remaining() + usize::from(has_fcs) * 32
    }

    fn bl_adata_exceeds_sch_f_capacity(
        main_carrier: u16,
        addr: TetraAddress,
        chan_alloc: Option<&CmceChanAllocReq>,
        tl_sdu: &BitBuffer,
        has_fcs: bool,
    ) -> bool {
        Self::bl_adata_len_bits(tl_sdu, has_fcs) > Self::sch_f_mac_resource_tm_sdu_capacity_bits(main_carrier, addr, chan_alloc)
    }

    /// Schedule an ACK to be sent at a later time
    pub fn schedule_outgoing_ack(
        &mut self,
        dltime: TdmaTime,
        addr: TetraAddress,
        endpoint_id: EndpointId,
        ns: u8,
        ind_req_handle: Todo,
        air_interface_encryption: Todo,
    ) {
        Self::upsert_scheduled_out_ack(
            &mut self.scheduled_out_acks,
            dltime,
            addr,
            endpoint_id,
            ns,
            ind_req_handle,
            air_interface_encryption,
        );
    }

    fn upsert_scheduled_out_ack(
        acks: &mut VecDeque<ScheduledOutAck>,
        dltime: TdmaTime,
        addr: TetraAddress,
        endpoint_id: EndpointId,
        ns: u8,
        ind_req_handle: Todo,
        air_interface_encryption: Todo,
    ) {
        let ts = dltime.t;
        if let Some(existing) = acks
            .iter_mut()
            .find(|ack| ack.addr.ssi == addr.ssi && ack.addr.ssi_type == addr.ssi_type && ack.endpoint_id == endpoint_id)
        {
            existing.t_start = dltime;
            existing.nr = ns;
            existing.addr = addr;
            existing.endpoint_id = endpoint_id;
            existing.ind_req_handle = ind_req_handle;
            existing.air_interface_encryption = air_interface_encryption;
            existing.ts = ts;
            return;
        }

        acks.push_back(ScheduledOutAck {
            t_start: dltime,
            nr: ns,
            ind_req_handle,
            air_interface_encryption,
            addr,
            endpoint_id,
            ts,
        });
    }

    /// Returns details for outstanding to-be-sent ACK, if any. Returned u8 is the sequence number.
    /// ETSI 22.3.2.3 case d: when a waiting ACK and outgoing TL-DATA exist for the same
    /// basic link, the LLC shall emit a combined BL-ADATA PDU. The endpoint
    /// identifies the MAC resource for the basic link; distinct physical
    /// allocations must use distinct endpoint identifiers.
    fn take_scheduled_out_ack_for_addr(&mut self, addr: TetraAddress, endpoint_id: EndpointId, _ts: u8) -> Option<ScheduledOutAck> {
        for i in 0..self.scheduled_out_acks.len() {
            if self.scheduled_out_acks[i].addr == addr && self.scheduled_out_acks[i].endpoint_id == endpoint_id {
                return self.scheduled_out_acks.remove(i);
            }
        }
        None
    }

    fn cancel_scheduled_out_ack_for_new_bl_data(&mut self, addr: TetraAddress, endpoint_id: EndpointId) -> bool {
        for i in 0..self.scheduled_out_acks.len() {
            if self.scheduled_out_acks[i].addr == addr && self.scheduled_out_acks[i].endpoint_id == endpoint_id {
                self.scheduled_out_acks.remove(i);
                return true;
            }
        }
        false
    }

    fn scheduled_out_ack_handle_for_key(&self, key: BasicLinkKey) -> Option<Todo> {
        self.scheduled_out_acks
            .iter()
            .find(|ack| ack.addr == key.addr && ack.endpoint_id == key.endpoint_id)
            .map(|ack| ack.ind_req_handle)
    }

    fn take_scheduled_out_ack_for_response(
        &mut self,
        addr: TetraAddress,
        endpoint_id: EndpointId,
        req_handle: Todo,
    ) -> Option<ScheduledOutAck> {
        // EN 300 392-2 clauses 22.3.1.1 and 22.3.2.3(b/c) bind a
        // TL-DATA.response to the LLC-generated handle from the corresponding
        // TL-DATA.ind. A zero/unknown handle must not consume whichever ACK is
        // still pending for the same endpoint.
        for i in 0..self.scheduled_out_acks.len() {
            if self.scheduled_out_acks[i].addr == addr
                && self.scheduled_out_acks[i].endpoint_id == endpoint_id
                && self.scheduled_out_acks[i].ind_req_handle == req_handle
            {
                return self.scheduled_out_acks.remove(i);
            }
        }
        None
    }

    fn next_tl_data_ind_req_handle(&mut self) -> Todo {
        let handle = self.next_tl_data_ind_req_handle;
        self.next_tl_data_ind_req_handle = if handle == i32::MIN { -1 } else { handle - 1 };
        handle
    }

    fn basic_link_key(addr: TetraAddress, endpoint_id: EndpointId) -> BasicLinkKey {
        BasicLinkKey { addr, endpoint_id }
    }

    fn advanced_link_key(addr: TetraAddress, endpoint_id: EndpointId, link_id: LinkId) -> AdvancedLinkKey {
        AdvancedLinkKey {
            addr,
            endpoint_id,
            link_id,
        }
    }

    fn advanced_link_key_from_setup(addr: TetraAddress, endpoint_id: EndpointId, setup: &AlSetup) -> AdvancedLinkKey {
        Self::advanced_link_key(addr, endpoint_id, setup.link_id())
    }

    fn negotiate_inbound_al_setup_response(setup: &AlSetup) -> AlSetup {
        let response = setup.response_with_lower_phase_mod_timeslots(AL_SETUP_PHASE_MOD_PDCH_MAX_TIMESLOTS);
        if response.setup_report == AlSetup::SETUP_REPORT_SERVICE_CHANGE {
            return response;
        }
        setup.response_success()
    }

    fn clear_other_original_advanced_links(&mut self, key: AdvancedLinkKey) {
        self.advanced_links.retain(|existing, _| {
            !(existing.addr == key.addr && existing.endpoint_id == key.endpoint_id && existing.link_id != key.link_id)
        });
        self.pending_advanced_link_setups.retain(|existing, _| {
            !(existing.addr == key.addr && existing.endpoint_id == key.endpoint_id && existing.link_id != key.link_id)
        });
        self.advanced_link_rx.retain(|existing, _| {
            !(existing.addr == key.addr && existing.endpoint_id == key.endpoint_id && existing.link_id != key.link_id)
        });
        self.outbound_al_messages.retain(|outbound| {
            !(outbound.key.addr == key.addr && outbound.key.endpoint_id == key.endpoint_id && outbound.key.link_id != key.link_id)
        });
    }

    fn establish_advanced_link(&mut self, key: AdvancedLinkKey, setup: AlSetup) {
        self.clear_other_original_advanced_links(key);
        self.pending_advanced_link_setups.remove(&key);
        self.advanced_links.insert(
            key,
            AdvancedLinkSession {
                key,
                al_number: setup.advanced_link_number,
                max_tl_sdu_retransmissions: setup.max_tl_sdu_retransmissions,
                max_segment_retransmissions: setup.max_segment_retransmissions,
                tx_next_ns: 0,
                last_delivered_ns: None,
            },
        );
        self.advanced_link_rx.remove(&key);
    }

    fn resolve_original_advanced_link_key(&self, addr: TetraAddress, endpoint_id: EndpointId) -> Option<AdvancedLinkKey> {
        let mut matches = self
            .advanced_links
            .keys()
            .copied()
            .filter(|key| key.addr == addr && key.endpoint_id == endpoint_id);
        let first = matches.next()?;
        if matches.next().is_some() {
            tracing::warn!(
                "LLC: ambiguous original AL PDU for SSI {} endpoint {}; more than one advanced link is active",
                addr.ssi,
                endpoint_id
            );
            return None;
        }
        Some(first)
    }

    fn al_tl_sdu_with_fcs(tl_sdu: &mut BitBuffer) -> BitBuffer {
        let mut tl_sdu_with_fcs = BitBuffer::new_autoexpand(tl_sdu.get_len_remaining() + 32);
        let sdu_len = tl_sdu.get_len_remaining();
        tl_sdu_with_fcs.copy_bits(tl_sdu, sdu_len);
        let fcs_value = fcs::compute_fcs(&tl_sdu_with_fcs, 0, tl_sdu_with_fcs.get_len());
        tl_sdu_with_fcs.write_bits(fcs_value as u64, 32);
        tl_sdu_with_fcs.seek(0);
        tl_sdu_with_fcs
    }

    fn al_original_segment_payload_bits(main_carrier: u16, addr: TetraAddress, chan_alloc: Option<&CmceChanAllocReq>) -> usize {
        // EN 300 392-2 clauses 21.1.4.2 and 22.3.3.2.1: the LLC segment is
        // the selective-retransmission unit, while MAC should not fragment
        // normal pi/4-DQPSK advanced-link data. Use the full SCH/F TM-SDU
        // capacity left after the original AL-DATA/AL-FINAL header.
        Self::sch_f_mac_resource_tm_sdu_capacity_bits(main_carrier, addr, chan_alloc)
            .saturating_sub(AL_ORIGINAL_DATA_HEADER_BITS)
            .max(1)
    }

    fn split_al_tl_sdu_segments(tl_sdu_with_fcs: &mut BitBuffer, segment_payload_bits: usize) -> Option<Vec<BitBuffer>> {
        let total_bits = tl_sdu_with_fcs.get_len_remaining();
        if total_bits == 0 {
            return None;
        }
        let segment_count = total_bits.div_ceil(segment_payload_bits);
        if segment_count > (u8::MAX as usize + 1) {
            return None;
        }

        let mut segments = Vec::with_capacity(segment_count);
        for idx in 0..segment_count {
            let remaining = tl_sdu_with_fcs.get_len_remaining();
            let chunk_bits = remaining.min(segment_payload_bits);
            let mut segment = BitBuffer::new_autoexpand(chunk_bits);
            segment.copy_bits(tl_sdu_with_fcs, chunk_bits);
            segment.seek(0);
            debug_assert!(idx <= u8::MAX as usize);
            segments.push(segment);
        }
        Some(segments)
    }

    fn push_al_setup_response_to_umac(
        queue: &mut MessageQueue,
        addr: TetraAddress,
        endpoint_id: EndpointId,
        response: AlSetup,
        req_handle: Todo,
        air_interface_encryption: Todo,
    ) {
        let mut pdu_buf = BitBuffer::new_autoexpand(32);
        response.to_bitbuf(&mut pdu_buf);
        pdu_buf.seek(0);
        tracing::debug!("-> {} {}", response, pdu_buf.dump_bin());

        queue.push_back(SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
                req_handle,
                pdu: pdu_buf,
                main_address: addr,
                endpoint_id,
                pdu_prio: 5,
                stealing_permission: false,
                subscriber_class: 0,
                air_interface_encryption: Some(air_interface_encryption),
                stealing_repeats_flag: None,
                data_category: None,
                chan_alloc: None,
                tx_reporter: None,
            }),
        });
    }

    fn push_al_ack_to_umac(queue: &mut MessageQueue, key: AdvancedLinkKey, ack: AlAck, req_handle: Todo, air_interface_encryption: Todo) {
        let mut pdu_buf = BitBuffer::new_autoexpand(16);
        ack.to_bitbuf(&mut pdu_buf);
        pdu_buf.seek(0);
        tracing::debug!("-> {} {}", ack, pdu_buf.dump_bin());

        queue.push_back(SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
                req_handle,
                pdu: pdu_buf,
                main_address: key.addr,
                endpoint_id: key.endpoint_id,
                // EN 300 392-2 clause 22.3.3.2.2: AL-DATA-AR and
                // AL-FINAL-AR responses use priority level 5 and permit
                // stealing. UMAC may still put packet data on reserved PDCH.
                pdu_prio: 5,
                stealing_permission: true,
                subscriber_class: 0,
                air_interface_encryption: Some(air_interface_encryption),
                stealing_repeats_flag: None,
                data_category: None,
                chan_alloc: None,
                tx_reporter: None,
            }),
        });
    }

    fn concatenate_al_segments(assembly: &AdvancedLinkReceiveAssembly) -> Option<BitBuffer> {
        let final_ss = assembly.final_ss?;
        for ss in 0..=final_ss {
            if !assembly.segments.contains_key(&ss) {
                return None;
            }
        }

        let total_bits = (0..=final_ss)
            .filter_map(|ss| assembly.segments.get(&ss))
            .map(BitBuffer::get_len)
            .sum();
        let mut out = BitBuffer::new_autoexpand(total_bits);
        for ss in 0..=final_ss {
            let mut segment = BitBuffer::from_bitbuffer(assembly.segments.get(&ss)?);
            segment.seek(0);
            let segment_len = segment.get_len_remaining();
            out.copy_bits(&mut segment, segment_len);
        }
        out.seek(0);
        Some(out)
    }

    fn inbound_duplicate_state_expired(state: ReceiveSeqState, now: TdmaTime) -> bool {
        Self::downlink_signalling_frames_elapsed(state.received_at, now, state.ack_timeslot)
            > INBOUND_DUPLICATE_SUPPRESSION_SIGNALLING_FRAMES
    }

    fn prune_expired_inbound_receive_seq(&mut self, now: TdmaTime) {
        self.inbound_receive_seq.retain(|key, state| {
            let expired = Self::inbound_duplicate_state_expired(*state, now);
            if expired {
                tracing::debug!(
                    "LLC: expiring inbound duplicate guard for SSI {} endpoint {} N(S) {}",
                    key.addr.ssi,
                    key.endpoint_id,
                    state.last_ns
                );
            }
            !expired
        });
    }

    fn expected_ack_key(ack: &ExpectedInAck) -> BasicLinkKey {
        Self::basic_link_key(ack.addr, ack.endpoint_id)
    }

    fn channel_allocation_late_ack_grace(ack: &ExpectedInAck) -> Option<u32> {
        let SapMsgInner::TmaUnitdataReq(prim) = &ack.retransmission_buf.msg else {
            return None;
        };
        prim.chan_alloc
            .as_ref()
            .map(|_| CHANNEL_ALLOCATION_LATE_ACK_GRACE_SIGNALLING_FRAMES)
    }

    fn late_ack_grace_expired(ack: &ExpectedInAck, now: TdmaTime) -> bool {
        let Some(started_at) = ack.t_retransmissions_exhausted else {
            return false;
        };
        let Some(grace_frames) = Self::channel_allocation_late_ack_grace(ack) else {
            return true;
        };
        Self::t251_downlink_frames_elapsed(started_at, now, ack.ts, ack.stealing_repeats_flag) >= grace_frames
    }

    fn al_late_ack_grace_expired(al: &ExpectedAlAck, now: TdmaTime) -> bool {
        let Some(started_at) = al.t_retransmissions_exhausted else {
            return false;
        };
        Self::t251_downlink_frames_elapsed(started_at, now, al.ack_timeslot, al.ack_stealing_repeats_flag)
            >= AL_LATE_ACK_GRACE_SIGNALLING_FRAMES
    }

    fn expected_ack_timeslot_for_outbound_bl(prim: &TlaTlDataReqBl) -> u8 {
        if !prim.stealing_permission && prim.chan_alloc.is_some() {
            // EN 300 392-2 clauses 23.5.2.2 and 23.5.4.3 allow a channel
            // allocation to carry a basic slot grant on the current channel.
            // For late-assignment P2P call-control sent on the MCCH, UMAC
            // grants the peer's BL-ACK before the channel change so LLC must
            // age T.251 against the MCCH, not the newly allocated traffic slot.
            return COMMON_CONTROL_TIMESLOT;
        }

        prim.chan_alloc
            .as_ref()
            .and_then(|ca| ca.timeslots.iter().enumerate().find(|&(_, &set)| set).map(|(i, _)| (i + 1) as u8))
            .unwrap_or(COMMON_CONTROL_TIMESLOT)
    }

    fn first_assigned_timeslot(chan_alloc: Option<&CmceChanAllocReq>) -> Option<u8> {
        chan_alloc
            .and_then(|ca| ca.timeslots.iter().enumerate().find(|&(_, &set)| set).map(|(i, _)| (i + 1) as u8))
            .filter(|ts| (1..=4).contains(ts))
    }

    fn expected_ack_timeslot_for_outbound_al(prim: &TlaTlDataReqBl) -> u8 {
        if let Some(ts) = Self::first_assigned_timeslot(prim.chan_alloc.as_ref()) {
            return ts;
        }

        // Original AL used by WAP/SNDCP keeps the logical packet-data endpoint
        // separate from the active PDCH. Live packet-data responses can use
        // endpoint 0 with a non-basic link, so non-zero link IDs without an
        // explicit channel allocation must still age T.252 against PDCH, not
        // the MCCH.
        let endpoint_ts = u8::try_from(prim.endpoint_id).ok();
        if let Some(ts @ 2..=4) = endpoint_ts {
            return ts;
        }
        if prim.link_id != 0 {
            return FIRST_PACKET_DATA_TIMESLOT;
        }

        COMMON_CONTROL_TIMESLOT
    }

    fn queued_udata_key(udata: &QueuedUdata) -> BasicLinkKey {
        Self::basic_link_key(udata.addr, udata.endpoint_id)
    }

    fn expected_al_key(al: &ExpectedAlAck) -> AdvancedLinkKey {
        al.key
    }

    fn queued_udata_tx_state(udata: &QueuedUdata) -> Option<TxState> {
        udata.current_mac_reporter.as_ref().map(TxReporter::get_state)
    }

    fn mark_reporter_transmitted_if_pending(reporter: &TxReporter) {
        reporter.try_mark_transmitted();
    }

    fn mark_reporter_discarded_if_pending(reporter: &TxReporter) {
        reporter.try_mark_discarded();
    }

    fn mark_udata_current_mac_transmitted(udata: &mut QueuedUdata) {
        if let Some(reporter) = udata.current_mac_reporter.take() {
            Self::mark_reporter_transmitted_if_pending(&reporter);
        }
    }

    fn mark_udata_current_mac_discarded(udata: &mut QueuedUdata) {
        if let Some(reporter) = udata.current_mac_reporter.take() {
            Self::mark_reporter_discarded_if_pending(&reporter);
        }
    }

    fn mark_udata_service_transmitted(udata: &QueuedUdata) {
        if let Some(reporter) = &udata.service_tx_reporter {
            Self::mark_reporter_transmitted_if_pending(reporter);
        }
    }

    fn mark_udata_service_discarded(udata: &QueuedUdata) {
        if let Some(reporter) = &udata.service_tx_reporter {
            Self::mark_reporter_discarded_if_pending(reporter);
        }
    }

    fn ack_current_mac_state(ack: &ExpectedInAck) -> Option<TxState> {
        ack.current_mac_reporter.as_ref().map(TxReporter::get_state)
    }

    fn al_current_mac_state(al: &ExpectedAlAck) -> Option<TxState> {
        if al.segments.is_empty() {
            return None;
        }

        let mut any_unacknowledged = false;
        let mut all_transmitted = true;
        for segment in &al.segments {
            if segment.acknowledged {
                continue;
            }
            any_unacknowledged = true;
            let Some(state) = segment.current_mac_reporter.as_ref().map(TxReporter::get_state) else {
                return None;
            };
            match state {
                TxState::Pending => return None,
                TxState::Transmitted | TxState::Acknowledged => {}
                TxState::Discarded | TxState::Lost => return Some(TxState::Discarded),
            }
            all_transmitted &= matches!(state, TxState::Transmitted | TxState::Acknowledged);
        }

        if !any_unacknowledged {
            return Some(TxState::Transmitted);
        }

        all_transmitted.then_some(TxState::Transmitted)
    }

    fn mark_ack_current_mac_transmitted(ack: &mut ExpectedInAck) {
        if let Some(reporter) = &ack.current_mac_reporter {
            reporter.try_mark_transmitted();
        }
    }

    fn mark_al_current_mac_transmitted(al: &mut ExpectedAlAck) {
        for segment in &al.segments {
            if segment.acknowledged {
                continue;
            }
            if let Some(reporter) = &segment.current_mac_reporter {
                reporter.try_mark_transmitted();
            }
        }
    }

    fn mark_ack_current_mac_discarded(ack: &mut ExpectedInAck) {
        if let Some(reporter) = &ack.current_mac_reporter {
            reporter.try_mark_discarded();
        }
    }

    fn mark_al_current_mac_discarded(al: &mut ExpectedAlAck) {
        for segment in &al.segments {
            if segment.acknowledged {
                continue;
            }
            if let Some(reporter) = &segment.current_mac_reporter {
                reporter.try_mark_discarded();
            }
        }
    }

    fn mark_ack_service_first_complete(ack: &mut ExpectedInAck) {
        ack.tx_reporter.try_mark_transmitted();
    }

    fn mark_al_service_first_complete(al: &mut ExpectedAlAck) {
        al.tx_reporter.try_mark_transmitted();
    }

    fn mark_ack_service_failed(ack: &ExpectedInAck) {
        // EN 300 392-2 clause 22.3.2.3(g/h/i/k): MAC failure, retry
        // exhaustion, T.251 expiry, or wrong-ACK exhaustion produces one
        // failed-transfer result for the service TL-SDU. In production these
        // reports can be observed through asynchronous reporter clones, so
        // late failure must not panic after a prior completion.
        match ack.tx_reporter.get_state() {
            TxState::Pending => {
                ack.tx_reporter.try_mark_discarded();
            }
            TxState::Transmitted => {
                ack.tx_reporter.try_mark_lost();
            }
            TxState::Discarded | TxState::Lost | TxState::Acknowledged => {}
        }
    }

    fn mark_al_service_failed(al: &ExpectedAlAck) {
        match al.tx_reporter.get_state() {
            TxState::Pending => {
                al.tx_reporter.try_mark_discarded();
            }
            TxState::Transmitted => {
                al.tx_reporter.try_mark_lost();
            }
            TxState::Discarded | TxState::Lost | TxState::Acknowledged => {}
        }
    }

    fn reset_al_transmission_attempt(al: &mut ExpectedAlAck) {
        al.t_submitted_to_umac = None;
        al.t_umac_done = None;
        al.current_mac_reporter = None;
        al.t_retransmissions_exhausted = None;
        al.selective_segment_retry_pending = false;
        for segment in &mut al.segments {
            segment.t_submitted_to_umac = None;
            segment.t_umac_done = None;
            segment.current_mac_reporter = None;
            segment.acknowledged = false;
            segment.retransmission_requested = false;
            segment.retransmit_count = 0;
        }
    }

    fn al_segment_pdu_with_ack_request(pdu: &BitBuffer) -> Option<BitBuffer> {
        let mut reader = BitBuffer::from_bitbuffer(pdu);
        let header = AlData::from_bitbuf(&mut reader).ok()?;
        if header.acknowledgement_requested {
            return Some(BitBuffer::from_bitbuffer(pdu));
        }

        let payload_bits = reader.get_len_remaining();
        let mut updated = BitBuffer::new_autoexpand(17 + payload_bits);
        AlData {
            acknowledgement_requested: true,
            ..header
        }
        .to_bitbuf(&mut updated);
        updated.copy_bits(&mut reader, payload_bits);
        updated.seek(0);
        Some(updated)
    }

    fn schedule_requested_al_segment_retries(al: &mut ExpectedAlAck, max_segment_retransmissions: u8) -> Result<usize, u8> {
        let mut retry_segments = 0usize;
        for segment in &mut al.segments {
            if segment.acknowledged || !segment.retransmission_requested {
                continue;
            }
            if segment.retransmit_count >= max_segment_retransmissions {
                return Err(segment.ss);
            }

            segment.t_submitted_to_umac = None;
            segment.t_umac_done = None;
            segment.current_mac_reporter = None;
            retry_segments += 1;
        }

        if retry_segments > 0 {
            al.t_submitted_to_umac = None;
            al.t_umac_done = None;
            al.current_mac_reporter = None;
            al.t_retransmissions_exhausted = None;
        }
        Ok(retry_segments)
    }

    fn find_outbound_al_ack_match(&self, prim: &TmaUnitdataInd, ack: &AlAck) -> (Option<usize>, usize) {
        let mut exact_idx = None;
        let mut exact_matches = 0usize;

        for (idx, outbound) in self.outbound_al_messages.iter().enumerate() {
            if outbound.key.addr != prim.main_address || outbound.key.endpoint_id != prim.endpoint_id {
                continue;
            }
            if outbound.ns == ack.nr {
                exact_matches += 1;
                exact_idx = Some(idx);
            }
        }

        if exact_matches == 1 {
            return (exact_idx, exact_matches);
        }
        (None, exact_matches)
    }

    fn lowest_priority_unsubmitted_udata_below(messages: &VecDeque<QueuedUdata>, incoming_pdu_prio: Todo) -> Option<usize> {
        let mut selected: Option<(usize, Todo)> = None;
        for (idx, udata) in messages.iter().enumerate() {
            if udata.submitted || udata.pdu_prio >= incoming_pdu_prio {
                continue;
            }

            match selected {
                Some((_, selected_prio)) if selected_prio <= udata.pdu_prio => {}
                _ => selected = Some((idx, udata.pdu_prio)),
            }
        }
        selected.map(|(idx, _)| idx)
    }

    fn ensure_udata_backlog_capacity(
        queue: &mut MessageQueue,
        messages: &mut VecDeque<QueuedUdata>,
        incoming_pdu_prio: Todo,
        limit: usize,
    ) -> bool {
        while messages.len() >= limit {
            let Some(victim_idx) = Self::lowest_priority_unsubmitted_udata_below(messages, incoming_pdu_prio) else {
                return false;
            };
            let Some(mut victim) = messages.remove(victim_idx) else {
                return false;
            };

            // EN 300 392-2 clauses 20.4.1.1.3 and 22.3.2.4.1 define the
            // service-user failure report and BL-UDATA store/repeat model.
            // This cap is local resource control: only an unsubmitted
            // lower-priority TL-UNITDATA is failed, and it is reported
            // explicitly rather than being silently dropped.
            Self::mark_udata_current_mac_discarded(&mut victim);
            Self::mark_udata_service_discarded(&victim);
            tracing::warn!(
                "LLC: evicting queued BL-UDATA req_handle={} prio={} for incoming prio={} after backlog limit {}",
                victim.req_handle,
                victim.pdu_prio,
                incoming_pdu_prio,
                limit
            );
            Self::push_tla_report(queue, victim.req_handle, TLA_REPORT_FAILED_TRANSFER, Some(victim.endpoint_id));
        }

        true
    }

    fn reject_tl_unitdata_backlog_full(queue: &mut MessageQueue, prim: &mut TlaTlUnitdataReqBl) {
        if let Some(reporter) = prim.tx_reporter.take() {
            Self::mark_reporter_discarded_if_pending(&reporter);
        }
        Self::push_tla_report(queue, prim.req_handle, TLA_REPORT_FAILED_TRANSFER, Some(prim.endpoint_id));
    }

    fn reject_tldata_backlog_full(queue: &mut MessageQueue, prim: &mut TlaTlDataReqBl) {
        if let Some(reporter) = prim.tx_reporter.take() {
            Self::mark_reporter_discarded_if_pending(&reporter);
        }
        Self::push_tla_report(queue, prim.req_handle, TLA_REPORT_FAILED_TRANSFER, Some(prim.endpoint_id));
    }

    fn has_pending_outbound_for_key(&self, key: BasicLinkKey) -> bool {
        self.outbound_messages.iter().any(|ack| Self::expected_ack_key(ack) == key)
    }

    /// Returns the next send sequence number V(S) for this link, then toggles it.
    /// Each link independently starts at 0 and alternates 0,1,0,1,...
    fn get_next_send_seq(&mut self, addr: TetraAddress, endpoint_id: EndpointId) -> u8 {
        let key = Self::basic_link_key(addr, endpoint_id);
        let vs = self.link_send_seq.entry(key).or_insert(0);
        let ns = *vs;
        *vs ^= 1;
        ns
    }

    /// Returns and removes the expected ACK entry for the given basic-link context, if any.
    fn take_expected_ack_for_key(&mut self, key: BasicLinkKey) -> Option<ExpectedInAck> {
        for i in 0..self.outbound_messages.len() {
            let msg = &self.outbound_messages[i];
            if Self::expected_ack_key(msg) == key && msg.t_submitted_to_umac.is_some() {
                return self.outbound_messages.remove(i);
            }
        }
        None
    }

    fn single_expected_ack_index_by_req_handle(&self, req_handle: Todo) -> Result<Option<usize>, usize> {
        let mut found = None;
        let mut matches = 0;
        for (idx, ack) in self.outbound_messages.iter().enumerate() {
            if ack.req_handle == req_handle && ack.t_submitted_to_umac.is_some() {
                matches += 1;
                found = Some(idx);
            }
        }
        if matches > 1 { Err(matches) } else { Ok(found) }
    }

    fn single_queued_udata_index_by_req_handle(&self, req_handle: Todo) -> Result<Option<usize>, usize> {
        let mut found = None;
        let mut matches = 0;
        for (idx, udata) in self.outbound_udata_messages.iter().enumerate() {
            if udata.req_handle == req_handle {
                matches += 1;
                found = Some(idx);
            }
        }
        if matches > 1 { Err(matches) } else { Ok(found) }
    }

    fn single_tma_report_owner_by_req_handle(&self, req_handle: Todo) -> Result<Option<TmaReportOwner>, usize> {
        let mut found = None;
        let mut matches = 0;

        for (idx, ack) in self.outbound_messages.iter().enumerate() {
            if ack.req_handle == req_handle && ack.t_submitted_to_umac.is_some() {
                matches += 1;
                found = Some(TmaReportOwner::BlData(idx));
            }
        }

        for (idx, udata) in self.outbound_udata_messages.iter().enumerate() {
            if udata.req_handle == req_handle && udata.submitted {
                matches += 1;
                found = Some(TmaReportOwner::BlUdata(idx));
            }
        }

        for (idx, al) in self.outbound_al_messages.iter().enumerate() {
            for (segment_idx, segment) in al.segments.iter().enumerate() {
                if segment.req_handle == req_handle && segment.t_submitted_to_umac.is_some() {
                    matches += 1;
                    found = Some(TmaReportOwner::AlData(idx, segment_idx));
                }
            }
        }

        if matches > 1 { Err(matches) } else { Ok(found) }
    }

    fn push_tla_report(queue: &mut MessageQueue, req_handle: Todo, report: Todo, endpoint_id: Option<EndpointId>) {
        queue.push_back(SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::TlaTlReportInd(TlaTlReportInd {
                req_handle: Some(req_handle),
                report,
                chan_change_resp_req: None,
                chan_change_handle: None,
                chan_info: None,
                endpoint_id: endpoint_id.map(|endpoint_id| endpoint_id as Todo),
            }),
        });
    }

    fn push_tma_cancel(queue: &mut MessageQueue, req_handle: Todo) {
        queue.push_back(SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmaCancelReq(TmaCancelReq { req_handle }),
        });
    }

    fn push_tl_data_conf(
        queue: &mut MessageQueue,
        ack: &ExpectedInAck,
        tl_sdu: Option<BitBuffer>,
        scrambling_code: u32,
        new_endpoint_id: Option<EndpointId>,
        css_endpoint_id: Option<EndpointId>,
        fcs_flag: bool,
        air_interface_encryption: Todo,
        chan_change_resp_req: bool,
        chan_change_handle: Option<Todo>,
        chan_info: Option<Todo>,
    ) {
        queue.push_back(SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::TlaTlDataConfBl(TlDataConfBl {
                main_address: ack.addr,
                link_id: ack.link_id,
                endpoint_id: ack.endpoint_id,
                new_endpoint_id: new_endpoint_id.map(|endpoint_id| endpoint_id as Todo),
                css_endpoint_id: css_endpoint_id.map(|endpoint_id| endpoint_id as Todo),
                tl_sdu,
                scrambling_code: scrambling_code as Todo,
                fcs_flag,
                air_interface_encryption,
                chan_change_resp_req,
                chan_change_handle,
                chan_info,
                req_handle: ack.req_handle,
                report: TLA_REPORT_SUCCESSFUL_TRANSFER,
            }),
        });
    }

    fn reconcile_umac_done_from_reporter(queue: &mut MessageQueue, ack: &mut ExpectedInAck, dltime: TdmaTime, context: &str) -> bool {
        let mac_state = Self::ack_current_mac_state(ack);
        if ack.t_umac_done.is_some() || !matches!(mac_state, Some(TxState::Transmitted | TxState::Discarded)) {
            return false;
        }

        ack.t_umac_done = Some(dltime);
        tracing::trace!(
            "{}: SSI {} endpoint {} UMAC done at {}",
            context,
            ack.addr.ssi,
            ack.endpoint_id,
            dltime
        );

        if mac_state == Some(TxState::Transmitted) && !ack.first_complete_report_sent {
            // EN 300 392-2 clause 22.3.2.3(f): first complete BL-DATA
            // transmission starts the T.251 wait for a peer ACK and produces
            // TL-REPORT first-complete. The production UMAC path may surface
            // that event through TxReporter immediately before the peer ACK is
            // processed, so ACK handling must reconcile it synchronously.
            Self::mark_ack_service_first_complete(ack);
            Self::push_tla_report(queue, ack.req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION, Some(ack.endpoint_id));
            ack.first_complete_report_sent = true;
            return true;
        }

        false
    }

    /// Process incoming ACK per ETSI 22.3.2.3(j).
    /// Matches by address, endpoint, and N(R) so concurrent basic links for
    /// one SSI do not acknowledge each other.
    fn process_incoming_ack(
        &mut self,
        queue: &mut MessageQueue,
        addr: TetraAddress,
        endpoint_id: EndpointId,
        nr: u8,
    ) -> Option<ExpectedInAck> {
        let key = Self::basic_link_key(addr, endpoint_id);
        // Get the expected ACK entry
        let Some(mut expected_ack) = self.take_expected_ack_for_key(key) else {
            tracing::debug!(
                "received BL-ACK for SSI {} endpoint {} N(R) {} with no outstanding downlink; no-op unless it carries a TL-SDU",
                addr.ssi,
                endpoint_id,
                nr
            );
            return None;
        };

        Self::reconcile_umac_done_from_reporter(queue, &mut expected_ack, self.dltime, "process_incoming_ack");

        // Check it was indeed already transmitted by the Umac. A matching
        // BL-ACK can arrive before the local MAC completion reporter has been
        // reconciled; that ACK is itself evidence that the peer received this
        // N(S), so complete the local transfer instead of retransmitting.
        if expected_ack.t_umac_done.is_none() || !expected_ack.first_complete_report_sent {
            if expected_ack.ns == nr {
                tracing::info!(
                    "received matching ACK for SSI {} endpoint {} N(R) {} before local UMAC completion; accepting as complete transfer",
                    addr.ssi,
                    endpoint_id,
                    nr
                );
                Self::mark_ack_current_mac_transmitted(&mut expected_ack);
                expected_ack.t_umac_done = Some(self.dltime);
                if !expected_ack.first_complete_report_sent {
                    Self::mark_ack_service_first_complete(&mut expected_ack);
                    Self::push_tla_report(
                        queue,
                        expected_ack.req_handle,
                        TLA_REPORT_FIRST_COMPLETE_TRANSMISSION,
                        Some(expected_ack.endpoint_id),
                    );
                    expected_ack.first_complete_report_sent = true;
                }
                expected_ack.tx_reporter.mark_acknowledged();
                return Some(expected_ack);
            }

            // This may be an old retransmission of an ack for the before-last basic link message
            // Let's push the ack back into the head of the queue (not tail)..
            tracing::warn!(
                "received ACK for SSI {} endpoint {} N(R) {} before a complete UMAC transmission. Ignoring",
                addr.ssi,
                endpoint_id,
                nr
            );
            self.outbound_messages.push_front(expected_ack);
            return None;
        }

        // Check N(R)
        if expected_ack.ns == nr {
            // Successful ACK: N(R) matches N(S)
            tracing::debug!(
                "received ACK for SSI {} endpoint {} N(R) {}",
                addr.ssi,
                endpoint_id,
                expected_ack.ns
            );
            expected_ack.tx_reporter.mark_acknowledged();
            return Some(expected_ack);
        } else {
            // N(R) mismatch — per ETSI EN 300 392-2 22.3.2.3(j), not a successful ACK.
            // Retransmit immediately rather than waiting for the next T.251 expiry.
            tracing::warn!(
                "received unexpected ACK for SSI {} endpoint {}: N(R)={}, expected N(S)={}",
                addr.ssi,
                endpoint_id,
                nr,
                expected_ack.ns
            );

            if expected_ack.retransmit_count < N252_BL_MAX_TLSDU_RETRANSMITS_ACKED {
                expected_ack.retransmit_count += 1;
                // EN 300 392-2 clause 22.3.2.3(l) handles BL-ADATA as ACK
                // first and DATA second. A wrong N(R) keeps this TL-SDU in the
                // sending buffer for retry, but it must remain MAC-ready until
                // tick_end so the DATA half's waiting ACK can be folded into
                // the retry as BL-ADATA when it fits.
                expected_ack.t_submitted_to_umac = None;
                expected_ack.t_umac_done = None;
                expected_ack.t_retransmissions_exhausted = None;
                expected_ack.current_mac_reporter = None;
                self.outbound_messages.push_front(expected_ack);
            } else {
                if expected_ack.tx_reporter.get_state() != TxState::Transmitted {
                    tracing::warn!(
                        "received unexpected ACK for SSI {} N(S) {} with service reporter in {:?}; failing transfer",
                        expected_ack.addr.ssi,
                        expected_ack.ns,
                        expected_ack.tx_reporter.get_state()
                    );
                }
                Self::mark_ack_service_failed(&expected_ack);
                Self::push_tla_report(
                    queue,
                    expected_ack.req_handle,
                    TLA_REPORT_FAILED_TRANSFER,
                    Some(expected_ack.endpoint_id),
                );
            }
            return None;
        }

        // The expected_ack is confirmed as matched and goes out of scope here
    }

    fn rx_tma_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tma_prim");
        match message.msg {
            SapMsgInner::TmaUnitdataInd(_) => {
                self.rx_tma_unitdata_ind(queue, message);
            }
            SapMsgInner::TmaReportInd(_) => {
                self.rx_tma_report_ind(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }

    fn rx_tla_tlunitdata_req_bl(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tla_tlunitdata_req_bl");
        let SapMsgInner::TlaTlUnitdataReqBl(mut prim) = message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        if prim.link_id != 0 {
            tracing::warn!(
                "LLC: rejecting TL-UNITDATA.req for advanced link {} on SSI {} endpoint {}; AL-UDATA is not implemented",
                prim.link_id,
                prim.main_address.ssi,
                prim.endpoint_id
            );
            Self::reject_tl_unitdata_backlog_full(queue, &mut prim);
            return;
        }

        if Self::tl_sdu_exceeds_n251(&prim.tl_sdu, prim.fcs_flag) {
            tracing::warn!(
                "LLC: rejecting oversized TL-UNITDATA.req TL-SDU bits={} fcs={} max_bits={}",
                prim.tl_sdu.get_len_remaining(),
                prim.fcs_flag,
                Self::n251_max_tl_sdu_bits(prim.fcs_flag)
            );
            if let Some(reporter) = prim.tx_reporter.take() {
                reporter.mark_discarded();
            }
            Self::push_tla_report(queue, prim.req_handle, TLA_REPORT_FAILED_TRANSFER, Some(prim.endpoint_id));
            return;
        }

        if !Self::ensure_udata_backlog_capacity(
            queue,
            &mut self.outbound_udata_messages,
            prim.pdu_prio,
            LLC_MAX_OUTBOUND_UDATA_MESSAGES,
        ) {
            // EN 300 392-2 clauses 20.4.1.1.3 and 22.3.2.4.1 do not require
            // an unbounded implementation queue. When local admission fails,
            // fail the TL-UNITDATA request explicitly before any MAC request
            // is created.
            tracing::warn!(
                "LLC: rejecting TL-UNITDATA.req req_handle={} prio={} after BL-UDATA backlog reached {}",
                prim.req_handle,
                prim.pdu_prio,
                LLC_MAX_OUTBOUND_UDATA_MESSAGES
            );
            Self::reject_tl_unitdata_backlog_full(queue, &mut prim);
            return;
        }

        let mut pdu_buf = BitBuffer::new_autoexpand(32);
        let pdu = BlUdata { has_fcs: prim.fcs_flag };
        pdu.to_bitbuf(&mut pdu_buf);
        Self::append_tl_sdu_and_optional_fcs(&mut pdu_buf, &mut prim.tl_sdu, pdu.has_fcs);
        tracing::debug!("-> {:?} sdu {}", pdu, pdu_buf.dump_bin());

        let service_tx_reporter = prim.tx_reporter.take();
        let sapmsg = SapMsg {
            sap: Sap::TmaSap,
            src: self.entity(),
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
                req_handle: prim.req_handle,
                pdu: pdu_buf,
                main_address: prim.main_address,
                endpoint_id: prim.endpoint_id,
                pdu_prio: prim.pdu_prio,
                stealing_permission: prim.stealing_permission,
                subscriber_class: prim.subscriber_class,
                air_interface_encryption: prim.air_interface_encryption,
                // EN 300 392-2 clauses 18.3.5.3.1 and 22.3.2.4.1: the
                // layer-3 stealing repeats parameter reaches LLC through
                // TL-UNITDATA and must be preserved when BL-UDATA is delivered
                // to the MAC formatter as TMA-UNITDATA.
                stealing_repeats_flag: prim.stealing_repeats_flag,
                data_category: prim.data_class_info,
                chan_alloc: prim.chan_alloc,
                tx_reporter: None,
            }),
        };

        let n253 = match prim.n_tlsdu_repeats {
            Some(requested) => {
                let clamped = requested.min(N253_MAX_REQUESTED_TLSDU_REPEATS);
                if requested > N253_MAX_REQUESTED_TLSDU_REPEATS {
                    tracing::warn!(
                        "LLC: TL-UNITDATA requested N.253={} above Annex A.2 maximum {}; clamping",
                        requested,
                        N253_MAX_REQUESTED_TLSDU_REPEATS
                    );
                }
                clamped
            }
            None => N253_BL_MAX_TLSDU_REPETITIONS_UNACKED.min(N253_MAX_REQUESTED_TLSDU_REPEATS as u32) as u8,
        };

        // Put into transmit queue. EN 300 392-2 clause 22.3.2.4.1 stores the
        // TL-SDU for N.253 + 1 complete BL-UDATA transmissions.
        self.outbound_udata_messages.push_back(QueuedUdata {
            addr: prim.main_address,
            t_first: self.dltime,
            req_handle: prim.req_handle,
            endpoint_id: prim.endpoint_id,
            pdu_prio: prim.pdu_prio,
            sapmsg,
            service_tx_reporter,
            current_mac_reporter: None,
            n253,
            target_complete_transmissions: n253.saturating_add(1),
            complete_transmissions: 0,
            failed_transmissions: 0,
            submitted: false,
            defer_mac_ready_once: false,
        });
    }

    /// Schedules a message that was not acked in time for a retransmission
    fn submit_for_acknowledged_transmission(queue: &mut MessageQueue, ack: &mut ExpectedInAck, dltime: TdmaTime) {
        // Clone the sapmsg. Make sure we set (or for retransmission: reset) timers properly
        let mut sapmsg = ack.retransmission_buf.clone();
        let mac_reporter = TxReporter::new_unacked();
        if let SapMsgInner::TmaUnitdataReq(prim) = &mut sapmsg.msg {
            prim.tx_reporter = Some(mac_reporter.clone());
        } else {
            tracing::warn!(
                "LLC: cannot attach MAC reporter for SSI {} endpoint {}; retransmission buffer is not TMA-UNITDATA",
                ack.addr.ssi,
                ack.endpoint_id
            );
        }
        ack.t_submitted_to_umac = Some(dltime);
        ack.t_umac_done = None;
        ack.current_mac_reporter = Some(mac_reporter);

        // Send the message
        queue.push_back(sapmsg);
    }

    fn submit_for_al_transmission(queue: &mut MessageQueue, al: &mut ExpectedAlAck, dltime: TdmaTime, max_segment_retransmissions: u8) {
        let mut submitted_any = false;
        for segment in &mut al.segments {
            if segment.acknowledged || segment.t_submitted_to_umac.is_some() {
                continue;
            }
            if segment.retransmission_requested {
                if segment.retransmit_count >= max_segment_retransmissions {
                    tracing::warn!(
                        "LLC: AL TL-SDU SSI {} endpoint {} link {} N(S) {} segment S(S) {} reached N.274 before retransmission",
                        al.key.addr.ssi,
                        al.key.endpoint_id,
                        al.key.link_id,
                        al.ns,
                        segment.ss
                    );
                    continue;
                }
                segment.retransmit_count += 1;
            }
            let mut sapmsg = segment.retransmission_buf.clone();
            let mac_reporter = TxReporter::new_unacked();
            if let SapMsgInner::TmaUnitdataReq(prim) = &mut sapmsg.msg {
                if segment.retransmission_requested {
                    if let Some(updated_pdu) = Self::al_segment_pdu_with_ack_request(&prim.pdu) {
                        prim.pdu = updated_pdu;
                    } else {
                        tracing::warn!(
                            "LLC: cannot set AL retransmission ACK request for SSI {} endpoint {} link {} segment {}; preserving original PDU",
                            al.key.addr.ssi,
                            al.key.endpoint_id,
                            al.key.link_id,
                            segment.ss
                        );
                    }
                }
                prim.tx_reporter = Some(mac_reporter.clone());
            } else {
                tracing::warn!(
                    "LLC: cannot attach MAC reporter for AL SSI {} endpoint {} link {} segment {}; retransmission buffer is not TMA-UNITDATA",
                    al.key.addr.ssi,
                    al.key.endpoint_id,
                    al.key.link_id,
                    segment.ss
                );
            }
            segment.t_submitted_to_umac = Some(dltime);
            segment.t_umac_done = None;
            segment.current_mac_reporter = Some(mac_reporter);
            queue.push_back(sapmsg);
            submitted_any = true;
        }
        if submitted_any {
            al.t_submitted_to_umac = Some(dltime);
        }
        al.t_umac_done = None;
        al.current_mac_reporter = None;
    }

    fn rx_tla_tldata_req_al(&mut self, queue: &mut MessageQueue, mut prim: TlaTlDataReqBl) {
        tracing::trace!("rx_tla_tldata_req_al");

        if prim.main_address.ssi_type == SsiType::Gssi {
            tracing::error!("LLC: AL-DATA requested for GSSI-addressed message; dropping");
            Self::reject_tldata_backlog_full(queue, &mut prim);
            return;
        }

        if self.outbound_al_messages.len() >= LLC_MAX_OUTBOUND_ACKED_MESSAGES {
            tracing::warn!(
                "LLC: rejecting AL TL-DATA.req req_handle={} prio={} after AL backlog reached {}",
                prim.req_handle,
                prim.pdu_prio,
                LLC_MAX_OUTBOUND_ACKED_MESSAGES
            );
            Self::reject_tldata_backlog_full(queue, &mut prim);
            return;
        }

        let key = Self::advanced_link_key(prim.main_address, prim.endpoint_id, prim.link_id);
        let Some(session) = self.advanced_links.get_mut(&key) else {
            tracing::warn!(
                "LLC: rejecting AL TL-DATA.req for SSI {} endpoint {} link {}; no established original acknowledged AL",
                prim.main_address.ssi,
                prim.endpoint_id,
                prim.link_id
            );
            Self::reject_tldata_backlog_full(queue, &mut prim);
            return;
        };

        let ns = session.tx_next_ns;
        session.tx_next_ns = (session.tx_next_ns + 1) & 0b111;

        let segment_payload_bits =
            Self::al_original_segment_payload_bits(self.config.config().cell.main_carrier, prim.main_address, prim.chan_alloc.as_ref());
        let mut tl_sdu_with_fcs = Self::al_tl_sdu_with_fcs(&mut prim.tl_sdu);
        let Some(payload_segments) = Self::split_al_tl_sdu_segments(&mut tl_sdu_with_fcs, segment_payload_bits) else {
            tracing::warn!(
                "LLC: rejecting AL TL-DATA.req for SSI {} endpoint {} link {}; TL-SDU needs more than 256 AL segments",
                prim.main_address.ssi,
                prim.endpoint_id,
                prim.link_id
            );
            Self::reject_tldata_backlog_full(queue, &mut prim);
            return;
        };
        let segment_count = payload_segments.len();
        let last_segment_idx = segment_count.saturating_sub(1);
        let mut segments = Vec::with_capacity(segment_count);

        for (idx, mut payload_segment) in payload_segments.into_iter().enumerate() {
            let final_segment = idx == last_segment_idx;
            let acknowledgement_requested = final_segment || (idx + 1) % AL_ORIGINAL_SEGMENTS_PER_ACK_REQUEST == 0;
            let ss = idx as u8;
            let pdu = AlData {
                final_segment,
                acknowledgement_requested,
                ns,
                ss,
            };
            let mut pdu_buf = BitBuffer::new_autoexpand(17 + payload_segment.get_len_remaining());
            pdu.to_bitbuf(&mut pdu_buf);
            let payload_bits = payload_segment.get_len_remaining();
            pdu_buf.copy_bits(&mut payload_segment, payload_bits);
            pdu_buf.seek(0);
            tracing::debug!(
                ts=%self.dltime,
                "-> queued {} SSI {} endpoint {} link {} segment {}/{} bits={}",
                pdu,
                key.addr.ssi,
                key.endpoint_id,
                key.link_id,
                idx + 1,
                segment_count,
                pdu_buf.get_len()
            );

            let segment_req_handle = if segment_count == 1 {
                prim.req_handle
            } else {
                self.next_tl_data_ind_req_handle()
            };
            segments.push(ExpectedAlSegment {
                ss,
                req_handle: segment_req_handle,
                t_submitted_to_umac: None,
                t_umac_done: None,
                current_mac_reporter: None,
                acknowledged: false,
                retransmission_requested: false,
                retransmit_count: 0,
                retransmission_buf: SapMsg {
                    sap: Sap::TmaSap,
                    src: self.entity(),
                    dest: TetraEntity::Umac,
                    msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
                        req_handle: segment_req_handle,
                        pdu: pdu_buf,
                        main_address: prim.main_address,
                        endpoint_id: prim.endpoint_id,
                        pdu_prio: prim.pdu_prio,
                        stealing_permission: prim.stealing_permission,
                        subscriber_class: prim.subscriber_class,
                        air_interface_encryption: prim.air_interface_encryption,
                        stealing_repeats_flag: prim.stealing_repeats_flag,
                        data_category: prim.data_class_info,
                        chan_alloc: prim.chan_alloc.clone(),
                        tx_reporter: None,
                    }),
                },
            });
        }

        let tx_reporter = prim.tx_reporter.take().unwrap_or_else(TxReporter::new);
        let ack_timeslot = Self::expected_ack_timeslot_for_outbound_al(&prim);
        let ack_stealing_repeats_flag = prim.stealing_repeats_flag;

        self.outbound_al_messages.push_back(ExpectedAlAck {
            key,
            ns,
            req_handle: prim.req_handle,
            pdu_prio: prim.pdu_prio,
            fcs_flag: true,
            air_interface_encryption: prim.air_interface_encryption.unwrap_or(0),
            t_first: self.dltime,
            t_submitted_to_umac: None,
            t_umac_done: None,
            tx_reporter,
            current_mac_reporter: None,
            ack_timeslot,
            ack_stealing_repeats_flag,
            segments,
            retransmit_count: 0,
            first_complete_report_sent: false,
            t_retransmissions_exhausted: None,
            selective_segment_retry_pending: false,
        });

        Self::push_tla_report(queue, prim.req_handle, TLA_REPORT_NO_SPECIFIC_REPORT, Some(prim.endpoint_id));
    }

    /// See Clause 22.3.2.3 for Acknowledged data transmission in basic link
    fn rx_tla_tldata_req_bl(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tla_tldata_req_bl");
        let SapMsgInner::TlaTlDataReqBl(mut prim) = message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        if prim.link_id != 0 {
            self.rx_tla_tldata_req_al(queue, prim);
            return;
        }

        if prim.main_address.ssi_type == SsiType::Gssi {
            tracing::error!("LLC: BL-DATA requested for GSSI-addressed message — not supported, dropping");
            return;
        }

        if Self::tl_sdu_exceeds_n251(&prim.tl_sdu, prim.fcs_flag) {
            tracing::warn!(
                "LLC: rejecting oversized TL-DATA.req TL-SDU bits={} fcs={} max_bits={}",
                prim.tl_sdu.get_len_remaining(),
                prim.fcs_flag,
                Self::n251_max_tl_sdu_bits(prim.fcs_flag)
            );
            if let Some(reporter) = prim.tx_reporter.take() {
                reporter.mark_discarded();
            }
            Self::push_tla_report(queue, prim.req_handle, TLA_REPORT_FAILED_TRANSFER, Some(prim.endpoint_id));
            return;
        }

        if self.outbound_messages.len() >= LLC_MAX_OUTBOUND_ACKED_MESSAGES {
            // EN 300 392-2 clause 22.3.2.3 gives the N(S)/ACK/retransmission
            // procedure once a BL-DATA transfer is admitted. Rejecting here is
            // deliberately before N(S) allocation, so the basic-link send
            // sequence is not consumed by a locally failed request.
            tracing::warn!(
                "LLC: rejecting TL-DATA.req req_handle={} prio={} after BL-DATA backlog reached {}",
                prim.req_handle,
                prim.pdu_prio,
                LLC_MAX_OUTBOUND_ACKED_MESSAGES
            );
            Self::reject_tldata_backlog_full(queue, &mut prim);
            return;
        }

        // Must be done before chan_alloc is moved into TmaUnitdataReq below.
        let derived_ts = Self::expected_ack_timeslot_for_outbound_bl(&prim);

        // Get per-link send sequence number N(S) = V(S), then toggle V(S)
        // EN 300 392-2 clauses 22.3.1.1 and 23.1.2.5.2 make the
        // endpoint identifier part of the local basic-link context: ACKs use
        // the same endpoint, and that endpoint identifies the MAC resource.
        let ns = self.get_next_send_seq(prim.main_address, prim.endpoint_id);

        // Construct PDU, write header
        let mut pdu_buf = BitBuffer::new_autoexpand(32);

        // Queue as BL-DATA first. If a peer acknowledgement is still waiting,
        // EN 300 392-2 clause 22.3.2.3(d) is applied at MAC-ready time after
        // LLC has selected the highest-priority TL-SDU for this basic link.
        // This keeps a lower-priority queued TL-SDU from consuming the ACK
        // before a later higher-priority TL-SDU arrives in the same tick.
        let pdu = BlData {
            has_fcs: prim.fcs_flag,
            ns,
        };
        pdu.to_bitbuf(&mut pdu_buf);
        Self::append_tl_sdu_and_optional_fcs(&mut pdu_buf, &mut prim.tl_sdu, pdu.has_fcs);
        tracing::debug!(ts=%self.dltime, "-> queued {:?} sdu {}", pdu, pdu_buf.dump_bin());

        // Either take tx_reporter passed down or create a new one
        let tx_reporter = prim.tx_reporter.take().unwrap_or_else(|| TxReporter::new());

        let sapmsg = SapMsg {
            sap: Sap::TmaSap,
            src: self.entity(),
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
                req_handle: prim.req_handle,
                pdu: pdu_buf,
                main_address: prim.main_address,
                endpoint_id: prim.endpoint_id,
                pdu_prio: prim.pdu_prio,
                stealing_permission: prim.stealing_permission,
                subscriber_class: prim.subscriber_class,
                air_interface_encryption: prim.air_interface_encryption,
                stealing_repeats_flag: prim.stealing_repeats_flag,
                data_category: prim.data_class_info,
                chan_alloc: prim.chan_alloc,
                tx_reporter: None,
            }),
        };

        // Register that we expect an ACK for this message on the derived timeslot
        tracing::trace!("setting expected ack for ts{}", derived_ts);
        self.outbound_messages.push_back(ExpectedInAck {
            ns,
            req_handle: prim.req_handle,
            link_id: prim.link_id,
            endpoint_id: prim.endpoint_id,
            pdu_prio: prim.pdu_prio,
            fcs_flag: prim.fcs_flag,
            air_interface_encryption: prim.air_interface_encryption.unwrap_or(0),
            stealing_repeats_flag: prim.stealing_repeats_flag,
            addr: prim.main_address,
            ts: derived_ts,
            bl_type: Layer2Service::Acknowledged,
            tx_reporter,
            current_mac_reporter: None,
            t_first: self.dltime,
            t_submitted_to_umac: None,
            t_umac_done: None,
            t_retransmissions_exhausted: None,
            retransmission_buf: sapmsg, // Clone the message to keep a copy for potential retransmission
            retransmit_count: 0,
            first_complete_report_sent: false,
        });

        Self::push_tla_report(queue, prim.req_handle, TLA_REPORT_NO_SPECIFIC_REPORT, Some(prim.endpoint_id));

        // The message will now be picked up for transmission at end-of-tick, if the ssi does not yet have
        // a pending message waiting for an ack.
    }

    fn push_standalone_bl_ack(queue: &mut MessageQueue, ack: &ScheduledOutAck) {
        tracing::debug!("auto-ack for ssi: {}, n: {}, ts: {}", ack.addr.ssi, ack.nr, ack.ts);

        // Send BL-ACK via FACCH (stealing) on the traffic timeslot if the original
        // message arrived on a traffic channel (TS2-4), otherwise via MCCH (TS1).
        let steal = matches!(ack.ts, 2..=4);
        let mut pdu_buf = BitBuffer::new_autoexpand(5);
        let pdu = BlAck {
            has_fcs: false,
            nr: ack.nr,
        };
        pdu.to_bitbuf(&mut pdu_buf);
        pdu_buf.seek(0);
        tracing::debug!("-> {:?} {}", pdu, pdu_buf.dump_bin());

        let chan_alloc = match steal {
            true => {
                let mut timeslots = [false; 4];
                timeslots[(ack.ts - 1) as usize] = true;
                Some(CmceChanAllocReq {
                    usage: None,
                    timeslots,
                    alloc_type: ChanAllocType::Replace,
                    ul_dl_assigned: UlDlAssignment::Both,
                    carrier: None,
                })
            }
            false => None,
        };
        queue.push_back(SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
                // EN 300 392-2 clause 22.3.1.1 gives the TL-DATA.ind a
                // retained handle; use it as the MAC request handle for this
                // corresponding BL-ACK instead of a placeholder.
                req_handle: ack.ind_req_handle,
                pdu: pdu_buf,
                main_address: ack.addr,
                endpoint_id: ack.endpoint_id,
                // EN 300 392-2 clause 22.3.2.3: BL-ACK should set PDU
                // priority level 5, even though MAC does not normally use it.
                pdu_prio: 5,
                stealing_permission: steal,
                subscriber_class: 0,
                air_interface_encryption: Some(ack.air_interface_encryption),
                stealing_repeats_flag: None,
                data_category: None,
                chan_alloc,
                tx_reporter: None, // By definition, no higher layer entity is interested
            }),
        });
    }

    /// EN 300 392-2 clause 22.3.2.3(b/c): TL-DATA response before the
    /// corresponding acknowledgement is sent is carried in the BL-ACK PDU. If
    /// the acknowledgement already left the queue, the response is sent as a
    /// normal acknowledged BL-DATA transfer.
    fn rx_tla_tldata_resp_bl(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tla_tldata_resp_bl");
        let SapMsgInner::TlaTlDataRespBl(mut prim) = message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        if Self::tl_sdu_exceeds_n251(&prim.tl_sdu, prim.fcs_flag) {
            tracing::warn!(
                "LLC: rejecting oversized TL-DATA.response TL-SDU bits={} fcs={} max_bits={}",
                prim.tl_sdu.get_len_remaining(),
                prim.fcs_flag,
                Self::n251_max_tl_sdu_bits(prim.fcs_flag)
            );
            Self::push_tla_report(queue, prim.req_handle, TLA_REPORT_FAILED_TRANSFER, Some(prim.endpoint_id));
            return;
        }

        let Some(ack) = self.take_scheduled_out_ack_for_response(prim.main_address, prim.endpoint_id, prim.req_handle) else {
            let req = TlaTlDataReqBl {
                main_address: prim.main_address,
                link_id: prim.link_id,
                endpoint_id: prim.endpoint_id,
                tl_sdu: prim.tl_sdu,
                pdu_prio: prim.pdu_prio,
                stealing_permission: prim.stealing_permission,
                subscriber_class: prim.subscriber_class,
                fcs_flag: prim.fcs_flag,
                air_interface_encryption: Some(prim.air_interface_encryption),
                stealing_repeats_flag: prim.stealing_repeats_flag,
                data_class_info: prim.data_class_info,
                req_handle: prim.req_handle,
                graceful_degradation: None,
                chan_alloc: None,
                tx_reporter: None,
            };
            self.rx_tla_tldata_req_bl(
                queue,
                SapMsg {
                    sap: Sap::TlaSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Llc,
                    msg: SapMsgInner::TlaTlDataReqBl(req),
                },
            );
            return;
        };

        let mut pdu_buf = BitBuffer::new_autoexpand(32);
        let has_payload = prim.tl_sdu.get_len_remaining() > 0;
        let pdu = BlAck {
            has_fcs: prim.fcs_flag && has_payload,
            nr: ack.nr,
        };
        pdu.to_bitbuf(&mut pdu_buf);
        Self::append_tl_sdu_and_optional_fcs(&mut pdu_buf, &mut prim.tl_sdu, pdu.has_fcs);
        tracing::debug!(ts=%self.dltime, "-> {:?} response_sdu {}", pdu, pdu_buf.dump_bin());

        let steal = matches!(ack.ts, 2..=4);
        let chan_alloc = if steal {
            let mut timeslots = [false; 4];
            timeslots[(ack.ts - 1) as usize] = true;
            Some(CmceChanAllocReq {
                usage: None,
                timeslots,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
                carrier: None,
            })
        } else {
            None
        };

        queue.push_back(SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
                req_handle: prim.req_handle,
                pdu: pdu_buf,
                main_address: ack.addr,
                endpoint_id: prim.endpoint_id,
                // EN 300 392-2 clause 22.3.2.3 applies to BL-ACK with or
                // without service-user response data.
                pdu_prio: 5,
                stealing_permission: steal,
                subscriber_class: prim.subscriber_class,
                air_interface_encryption: Some(prim.air_interface_encryption),
                stealing_repeats_flag: prim.stealing_repeats_flag,
                data_category: prim.data_class_info,
                chan_alloc,
                tx_reporter: None,
            }),
        });
    }

    fn rx_tla_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tla_prim");
        match &message.msg {
            SapMsgInner::TlaTlDataReqBl(_) => {
                self.rx_tla_tldata_req_bl(queue, message);
            }
            SapMsgInner::TlaTlDataRespBl(_) => {
                self.rx_tla_tldata_resp_bl(queue, message);
            }
            SapMsgInner::TlaTlUnitdataReqBl(_) => {
                self.rx_tla_tlunitdata_req_bl(queue, message);
            }
            _ => {
                tracing::warn!("unhandled match variant, ignoring");
            }
        }
    }

    fn rx_tma_report_ind(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tma_report_ind");
        let SapMsgInner::TmaReportInd(prim) = message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        // ETSI EN 300 392-2 clause 20.4.1.1.3: TMA-REPORT.ind reports the
        // progress/failure of the MAC request procedure. The primitive carries
        // only req_handle, so the handle must identify exactly one outstanding
        // BL-DATA or BL-UDATA MAC request before LLC applies service-specific
        // side effects.
        let idx = match self.single_tma_report_owner_by_req_handle(prim.req_handle) {
            Ok(Some(TmaReportOwner::BlData(idx))) => idx,
            Ok(Some(TmaReportOwner::BlUdata(idx))) => {
                self.handle_udata_tma_report_at_index(queue, &prim, idx);
                return;
            }
            Ok(Some(TmaReportOwner::AlData(idx, segment_idx))) => {
                self.handle_al_tma_report_at_index(queue, &prim, idx, segment_idx);
                return;
            }
            Ok(None) => {
                if self.handle_udata_tma_report(queue, &prim) {
                    return;
                }
                tracing::debug!(
                    "LLC: TMA-REPORT for untracked req_handle={} report={:?}",
                    prim.req_handle,
                    prim.report
                );
                return;
            }
            Err(matches) => {
                tracing::warn!(
                    "LLC: TMA-REPORT req_handle={} matches {} submitted BL-DATA/BL-UDATA requests; ignoring ambiguous report",
                    prim.req_handle,
                    matches
                );
                return;
            }
        };

        if matches!(prim.report, TmaReport::RandomAccessFailure | TmaReport::FailedTransfer) {
            let Some(mut ack) = self.outbound_messages.remove(idx) else {
                return;
            };
            Self::mark_ack_current_mac_discarded(&mut ack);
            Self::mark_ack_service_failed(&ack);
            // EN 300 392-2 clauses 20.4.1.1.3 and 22.3.2.3(g): a MAC
            // failure report for service-user data is terminal for this
            // TL-SDU. Retries are only specified for fragmentation failure
            // and for T.251 expiry while waiting for BL-ACK.
            Self::push_tla_report(queue, ack.req_handle, TLA_REPORT_FAILED_TRANSFER, Some(ack.endpoint_id));
            return;
        }

        if matches!(prim.report, TmaReport::FragmentationFailure) {
            let Some(mut ack) = self.outbound_messages.remove(idx) else {
                return;
            };

            Self::mark_ack_current_mac_discarded(&mut ack);
            ack.t_umac_done = Some(self.dltime);
            ack.t_retransmissions_exhausted = None;

            if ack.retransmit_count < N252_BL_MAX_TLSDU_RETRANSMITS_ACKED {
                // EN 300 392-2 22.3.2.3(h): a BL-DATA/BL-ADATA fragmentation
                // failure retries by signalling DATA_IN_BUFFER immediately;
                // it does not wait for the separate T.251 expiry path.
                ack.retransmit_count += 1;
                tracing::info!(
                    "LLC: fragmentation failure for SSI {} endpoint {} N(S) {}, immediate retry attempt {}",
                    ack.addr.ssi,
                    ack.endpoint_id,
                    ack.ns,
                    ack.retransmit_count
                );
                let resubmit_time = self.dltime.forward_to_timeslot(ack.t_first.t);
                Self::fold_waiting_ack_into_mac_ready_bl_data(
                    self.config.config().cell.main_carrier,
                    queue,
                    &mut self.scheduled_out_acks,
                    &mut ack,
                );
                Self::submit_for_acknowledged_transmission(queue, &mut ack, resubmit_time);
                self.outbound_messages.insert(idx, ack);
            } else {
                Self::mark_ack_service_failed(&ack);
                tracing::warn!(
                    "LLC: fragmentation failure exhausted N.252 for SSI {} endpoint {} N(S) {}",
                    ack.addr.ssi,
                    ack.endpoint_id,
                    ack.ns
                );
                Self::push_tla_report(queue, ack.req_handle, TLA_REPORT_FAILED_TRANSFER, Some(ack.endpoint_id));
            }
            return;
        }

        let ack = &mut self.outbound_messages[idx];
        match prim.report {
            TmaReport::ConfirmHandle => {
                tracing::trace!("LLC: TMA-REPORT confirm handle for req_handle={}", prim.req_handle);
            }
            TmaReport::SuccessRandomAccess | TmaReport::SuccessReservedOrStealing => {
                Self::mark_ack_current_mac_transmitted(ack);
                if ack.t_umac_done.is_none() {
                    ack.t_umac_done = Some(self.dltime);
                }
                if !ack.first_complete_report_sent {
                    Self::mark_ack_service_first_complete(ack);
                    Self::push_tla_report(queue, ack.req_handle, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION, Some(ack.endpoint_id));
                    ack.first_complete_report_sent = true;
                }
            }
            TmaReport::FailedTransfer => unreachable!("handled before borrowing pending ACK entry"),
            TmaReport::FragmentationFailure => unreachable!("handled before borrowing pending ACK entry"),
            TmaReport::RandomAccessFailure => unreachable!("handled before borrowing pending ACK entry"),
        }
    }

    fn handle_udata_tma_report(&mut self, queue: &mut MessageQueue, prim: &TmaReportInd) -> bool {
        let idx = match self.single_queued_udata_index_by_req_handle(prim.req_handle) {
            Ok(Some(idx)) => idx,
            Ok(None) => return false,
            Err(matches) => {
                tracing::warn!(
                    "LLC: TMA-REPORT req_handle={} matches {} queued BL-UDATA requests; ignoring ambiguous report",
                    prim.req_handle,
                    matches
                );
                return true;
            }
        };

        self.handle_udata_tma_report_at_index(queue, prim, idx)
    }

    fn handle_al_tma_report_at_index(&mut self, queue: &mut MessageQueue, prim: &TmaReportInd, idx: usize, segment_idx: usize) -> bool {
        if matches!(prim.report, TmaReport::ConfirmHandle) {
            tracing::trace!("LLC: AL-DATA TMA-REPORT confirm handle for req_handle={}", prim.req_handle);
            return true;
        }

        match prim.report {
            TmaReport::SuccessRandomAccess | TmaReport::SuccessReservedOrStealing => {
                let al = &mut self.outbound_al_messages[idx];
                let Some(segment) = al.segments.get_mut(segment_idx) else {
                    tracing::warn!(
                        "LLC: AL TMA-REPORT req_handle={} matched missing segment index {}",
                        prim.req_handle,
                        segment_idx
                    );
                    return true;
                };
                if let Some(reporter) = &segment.current_mac_reporter {
                    reporter.try_mark_transmitted();
                }
                segment.t_umac_done = Some(self.dltime);
                if al.t_umac_done.is_none() && Self::al_current_mac_state(al) == Some(TxState::Transmitted) {
                    al.t_umac_done = Some(self.dltime);
                    Self::mark_al_service_first_complete(al);
                    Self::push_tla_report(
                        queue,
                        al.req_handle,
                        TLA_REPORT_FIRST_COMPLETE_TRANSMISSION,
                        Some(al.key.endpoint_id),
                    );
                    al.first_complete_report_sent = true;
                }
            }
            TmaReport::FailedTransfer | TmaReport::FragmentationFailure | TmaReport::RandomAccessFailure => {
                let Some(mut al) = self.outbound_al_messages.remove(idx) else {
                    return true;
                };
                Self::mark_al_current_mac_discarded(&mut al);
                Self::mark_al_service_failed(&al);
                Self::push_tla_report(queue, al.req_handle, TLA_REPORT_FAILED_TRANSFER, Some(al.key.endpoint_id));
            }
            TmaReport::ConfirmHandle => unreachable!("handled above"),
        }

        true
    }

    fn handle_udata_tma_report_at_index(&mut self, queue: &mut MessageQueue, prim: &TmaReportInd, idx: usize) -> bool {
        if matches!(prim.report, TmaReport::ConfirmHandle) {
            tracing::trace!("LLC: BL-UDATA TMA-REPORT confirm handle for req_handle={}", prim.req_handle);
            return true;
        }

        if !self.outbound_udata_messages[idx].submitted {
            tracing::warn!(
                "LLC: ignoring BL-UDATA TMA-REPORT {:?} for req_handle={} while no transmission is outstanding",
                prim.report,
                prim.req_handle
            );
            return true;
        }

        match prim.report {
            TmaReport::SuccessRandomAccess => {
                let Some(mut udata) = self.outbound_udata_messages.remove(idx) else {
                    return true;
                };
                Self::mark_udata_current_mac_transmitted(&mut udata);
                Self::mark_udata_service_transmitted(&udata);
                tracing::debug!(
                    "LLC: BL-UDATA req_handle={} completed by random access for SSI {}",
                    udata.req_handle,
                    udata.addr.ssi
                );
                Self::push_tla_report(queue, udata.req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER, Some(udata.endpoint_id));
            }
            TmaReport::SuccessReservedOrStealing => {
                let is_complete = {
                    let udata = &mut self.outbound_udata_messages[idx];
                    // EN 300 392-2 clauses 20.4.1.1.3 and 22.3.2.4.1:
                    // TMA-REPORT is per MAC request, while the stored TL-SDU
                    // completes only after N.253 + 1 reserved/stealing
                    // complete transmissions.
                    Self::mark_udata_current_mac_transmitted(udata);
                    udata.complete_transmissions = udata.complete_transmissions.saturating_add(1);
                    udata.submitted = false;
                    udata.complete_transmissions >= udata.target_complete_transmissions
                };
                if is_complete {
                    let Some(udata) = self.outbound_udata_messages.remove(idx) else {
                        return true;
                    };
                    Self::mark_udata_service_transmitted(&udata);
                    Self::push_tla_report(queue, udata.req_handle, TLA_REPORT_SUCCESSFUL_TRANSFER, Some(udata.endpoint_id));
                }
            }
            TmaReport::FailedTransfer | TmaReport::FragmentationFailure => {
                let is_failed = {
                    let udata = &mut self.outbound_udata_messages[idx];
                    Self::mark_udata_current_mac_discarded(udata);
                    udata.failed_transmissions = udata.failed_transmissions.saturating_add(1);
                    udata.submitted = false;
                    let max_failed_transmissions = if udata.n253 == 0 { 2 } else { udata.n253.saturating_add(1) };
                    udata.failed_transmissions >= max_failed_transmissions
                        && udata.complete_transmissions < udata.target_complete_transmissions
                };
                if is_failed {
                    let Some(udata) = self.outbound_udata_messages.remove(idx) else {
                        return true;
                    };
                    Self::mark_udata_service_discarded(&udata);
                    Self::push_tla_report(queue, udata.req_handle, TLA_REPORT_FAILED_TRANSFER, Some(udata.endpoint_id));
                }
            }
            TmaReport::RandomAccessFailure => {
                let Some(mut udata) = self.outbound_udata_messages.remove(idx) else {
                    return true;
                };
                Self::mark_udata_current_mac_discarded(&mut udata);
                Self::mark_udata_service_discarded(&udata);
                Self::push_tla_report(queue, udata.req_handle, TLA_REPORT_FAILED_TRANSFER, Some(udata.endpoint_id));
            }
            TmaReport::ConfirmHandle => unreachable!("handled above"),
        }

        true
    }

    /// Clause 20.4.1.1.4 TMA-UNITDATA primitive
    /// TMA-UNITDATA indication: this primitive shall be used by the MAC to deliver a received TM-SDU. This primitive
    /// may also be used with no TM-SDU if the MAC needs to inform the higher layers of a channel allocation received
    /// without an associated TM-SDU.
    fn rx_tma_unitdata_ind(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_tma_unitdata_ind");

        // Determine which type of TL-SDU we have
        let pdu_type = if let SapMsgInner::TmaUnitdataInd(prim) = &mut message.msg {
            let Some(pdu) = prim.pdu.as_ref() else {
                tracing::warn!("LLC: rx_tma_unitdata_ind received message with no pdu, ignoring");
                return;
            };
            let Some(bits) = pdu.peek_bits(4) else {
                tracing::warn!("insufficient bits: {}", pdu.dump_bin());
                return;
            };
            let Ok(pdu_type) = LlcPduType::try_from(bits) else {
                tracing::warn!("invalid pdu type: {} in {}", bits, pdu.dump_bin());
                return;
            };

            pdu_type
        } else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        // Call handler function
        match pdu_type {
            // All Basic Link types can be handled by the same function
            LlcPduType::BlAdata
            | LlcPduType::BlAdataFcs
            | LlcPduType::BlData
            | LlcPduType::BlDataFcs
            | LlcPduType::BlUdata
            | LlcPduType::BlUdataFcs
            | LlcPduType::BlAck
            | LlcPduType::BlAckFcs => {
                self.rx_tma_unitdata_ind_bl(queue, message);
            }

            LlcPduType::AlSetup | LlcPduType::AlDataAlFinal | LlcPduType::AlAckAlRnr => {
                self.rx_tma_unitdata_ind_al(queue, message);
            }

            LlcPduType::AlAlUdataAlUfinal | LlcPduType::AlReconnect | LlcPduType::AlDisc => {
                unimplemented_log!("LlcPduType unsupported Advanced Link PDU: {}", pdu_type);
            }

            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }

    fn rx_tma_unitdata_ind_al(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_tma_unitdata_ind_al");
        let SapMsgInner::TmaUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let Some(pdu) = prim.pdu.take() else {
            tracing::warn!("LLC: rx_tma_unitdata_ind_al received message with no pdu, ignoring");
            return;
        };
        let Some(bits) = pdu.peek_bits(4) else {
            tracing::warn!("insufficient AL bits: {}", pdu.dump_bin());
            return;
        };
        let Ok(pdu_type) = LlcPduType::try_from(bits) else {
            tracing::warn!("invalid AL pdu type: {} in {}", bits, pdu.dump_bin());
            return;
        };

        match pdu_type {
            LlcPduType::AlSetup => self.rx_al_setup(queue, prim, pdu),
            LlcPduType::AlDataAlFinal => self.rx_al_data_final(queue, prim, pdu),
            LlcPduType::AlAckAlRnr => self.rx_al_ack(queue, prim, pdu),
            _ => {
                tracing::error!("BUG: unexpected AL handler dispatch {}", pdu_type);
            }
        }
    }

    fn rx_al_setup(&mut self, queue: &mut MessageQueue, prim: &TmaUnitdataInd, mut pdu: BitBuffer) {
        let setup = match AlSetup::from_bitbuf(&mut pdu) {
            Ok(setup) => setup,
            Err(err) => {
                tracing::warn!("LLC: failed parsing AL-SETUP {:?} {}", err, pdu.dump_bin());
                return;
            }
        };
        tracing::debug!(ts=%self.dltime, "<- {}", setup);

        if !setup.is_original_acknowledged_non_augmented() {
            tracing::warn!(
                "LLC: unsupported AL-SETUP from SSI {} endpoint {}: acknowledged={} augmented={}",
                prim.main_address.ssi,
                prim.endpoint_id,
                setup.acknowledged_service,
                setup.augmented.is_some()
            );
            return;
        }

        let key = Self::advanced_link_key_from_setup(prim.main_address, prim.endpoint_id, &setup);
        if setup.setup_report == AlSetup::SETUP_REPORT_SUCCESS {
            self.establish_advanced_link(key, setup);
            tracing::info!(
                "LLC: AL-SETUP success accepted for SSI {} endpoint {} link {}",
                key.addr.ssi,
                key.endpoint_id,
                key.link_id
            );
            return;
        }

        let response = Self::negotiate_inbound_al_setup_response(&setup);
        if response.setup_report == AlSetup::SETUP_REPORT_SERVICE_CHANGE {
            self.clear_other_original_advanced_links(key);
            self.pending_advanced_link_setups.insert(key, response);
            self.advanced_links.remove(&key);
            self.advanced_link_rx.remove(&key);
            tracing::info!(
                "LLC: AL-SETUP for SSI {} endpoint {} link {} negotiated lower QoS {:?} -> {:?}",
                key.addr.ssi,
                key.endpoint_id,
                key.link_id,
                setup,
                response
            );
        } else {
            self.establish_advanced_link(key, setup);
        }

        let req_handle = self.next_tl_data_ind_req_handle();
        Self::push_al_setup_response_to_umac(
            queue,
            prim.main_address,
            prim.endpoint_id,
            response,
            req_handle,
            prim.air_interface_encryption,
        );
    }

    fn rx_al_data_final(&mut self, queue: &mut MessageQueue, prim: &TmaUnitdataInd, mut pdu: BitBuffer) {
        let header = match AlData::from_bitbuf(&mut pdu) {
            Ok(header) => header,
            Err(err) => {
                tracing::warn!("LLC: failed parsing AL-DATA/FINAL {:?} {}", err, pdu.dump_bin());
                return;
            }
        };
        tracing::debug!(ts=%self.dltime, "<- {}", header);

        let Some(key) = self.resolve_original_advanced_link_key(prim.main_address, prim.endpoint_id) else {
            tracing::warn!(
                "LLC: dropping original {} for SSI {} endpoint {}; no unambiguous established AL",
                header,
                prim.main_address.ssi,
                prim.endpoint_id
            );
            return;
        };

        let duplicate_delivered = self
            .advanced_links
            .get(&key)
            .and_then(|session| session.last_delivered_ns)
            .is_some_and(|last_ns| last_ns == header.ns);
        if duplicate_delivered {
            tracing::debug!(
                "LLC: re-ACKing duplicate delivered AL TL-SDU N(S)={} for SSI {} endpoint {} link {}",
                header.ns,
                key.addr.ssi,
                key.endpoint_id,
                key.link_id
            );
            Self::push_al_ack_to_umac(
                queue,
                key,
                AlAck::complete(header.ns),
                self.next_tl_data_ind_req_handle(),
                prim.air_interface_encryption,
            );
            return;
        }

        let segment = BitBuffer::from_bitbuffer_pos(&pdu);
        let replace_assembly = self.advanced_link_rx.get(&key).is_some_and(|assembly| assembly.ns != header.ns);
        if replace_assembly {
            tracing::warn!(
                "LLC: replacing incomplete AL receive assembly for SSI {} endpoint {} link {} old N(S) with new N(S)={}",
                key.addr.ssi,
                key.endpoint_id,
                key.link_id,
                header.ns
            );
            self.advanced_link_rx.remove(&key);
        }

        let assembly = self.advanced_link_rx.entry(key).or_insert_with(|| AdvancedLinkReceiveAssembly {
            key,
            ns: header.ns,
            segments: BTreeMap::new(),
            final_ss: None,
        });
        assembly.segments.entry(header.ss).or_insert(segment);
        if header.final_segment {
            assembly.final_ss = Some(header.ss);
        }

        let Some(mut complete) = self.advanced_link_rx.get(&key).and_then(Self::concatenate_al_segments) else {
            if header.acknowledgement_requested {
                Self::push_al_ack_to_umac(
                    queue,
                    key,
                    AlAck::repeat_entire(header.ns),
                    self.next_tl_data_ind_req_handle(),
                    prim.air_interface_encryption,
                );
            }
            return;
        };

        if complete.get_len_remaining() < 32 || !fcs::check_fcs(&complete) {
            tracing::warn!(
                "LLC: AL TL-SDU FCS failed for SSI {} endpoint {} link {} N(S)={}",
                key.addr.ssi,
                key.endpoint_id,
                key.link_id,
                header.ns
            );
            self.advanced_link_rx.remove(&key);
            Self::push_al_ack_to_umac(
                queue,
                key,
                AlAck::repeat_entire(header.ns),
                self.next_tl_data_ind_req_handle(),
                prim.air_interface_encryption,
            );
            return;
        }

        let payload_end = complete.get_raw_end() - 32;
        complete.set_raw_end(payload_end);
        complete.seek(0);
        self.advanced_link_rx.remove(&key);
        if let Some(session) = self.advanced_links.get_mut(&key) {
            session.last_delivered_ns = Some(header.ns);
        }

        let req_handle = self.next_tl_data_ind_req_handle();
        queue.push_back(SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::TlaTlDataIndBl(TlaTlDataIndBl {
                main_address: prim.main_address,
                link_id: key.link_id,
                endpoint_id: prim.endpoint_id,
                new_endpoint_id: prim.new_endpoint_id,
                css_endpoint_id: prim.css_endpoint_id,
                tl_sdu: if complete.get_len_remaining() > 0 { Some(complete) } else { None },
                scrambling_code: prim.scrambling_code,
                fcs_flag: true,
                air_interface_encryption: prim.air_interface_encryption,
                chan_change_resp_req: prim.chan_change_response_req,
                chan_change_handle: prim.chan_change_handle,
                chan_info: prim.chan_info,
                req_handle,
            }),
        });

        Self::push_al_ack_to_umac(queue, key, AlAck::complete(header.ns), req_handle, prim.air_interface_encryption);
    }

    fn rx_al_ack(&mut self, queue: &mut MessageQueue, prim: &TmaUnitdataInd, mut pdu: BitBuffer) {
        let ack = match AlAck::from_bitbuf(&mut pdu) {
            Ok(ack) => ack,
            Err(err) => {
                tracing::warn!("LLC: failed parsing AL-ACK/RNR {:?} {}", err, pdu.dump_bin());
                return;
            }
        };
        tracing::debug!(ts=%self.dltime, "<- {}", ack);

        let (match_idx, exact_matches) = self.find_outbound_al_ack_match(prim, &ack);
        let Some(idx) = match_idx else {
            tracing::warn!(
                "LLC: AL-ACK/RNR for SSI {} endpoint {} N(R)={} matched {} outstanding AL transfers; ignoring",
                prim.main_address.ssi,
                prim.endpoint_id,
                ack.nr,
                exact_matches
            );
            return;
        };

        if ack.acknowledges_complete_tl_sdu() {
            let Some(mut outbound) = self.outbound_al_messages.remove(idx) else {
                return;
            };
            if !outbound.first_complete_report_sent {
                Self::mark_al_current_mac_transmitted(&mut outbound);
                outbound.t_umac_done = Some(self.dltime);
                Self::mark_al_service_first_complete(&mut outbound);
                Self::push_tla_report(
                    queue,
                    outbound.req_handle,
                    TLA_REPORT_FIRST_COMPLETE_TRANSMISSION,
                    Some(outbound.key.endpoint_id),
                );
            }
            outbound.tx_reporter.mark_acknowledged();
            Self::push_tla_report(
                queue,
                outbound.req_handle,
                TLA_REPORT_SUCCESSFUL_TRANSFER,
                Some(outbound.key.endpoint_id),
            );
            return;
        }

        if ack.requests_repeat_entire_tl_sdu() {
            let max_retransmissions = self
                .advanced_links
                .get(&self.outbound_al_messages[idx].key)
                .map(|session| session.max_tl_sdu_retransmissions)
                .unwrap_or(0);
            let outbound = &mut self.outbound_al_messages[idx];
            if outbound.retransmit_count < max_retransmissions {
                outbound.retransmit_count += 1;
                Self::reset_al_transmission_attempt(outbound);
                tracing::info!(
                    "LLC: AL peer requested whole TL-SDU repeat for SSI {} endpoint {} link {} N(S)={} attempt {}",
                    outbound.key.addr.ssi,
                    outbound.key.endpoint_id,
                    outbound.key.link_id,
                    outbound.ns,
                    outbound.retransmit_count
                );
            } else {
                let Some(mut outbound) = self.outbound_al_messages.remove(idx) else {
                    return;
                };
                Self::mark_al_current_mac_discarded(&mut outbound);
                Self::mark_al_service_failed(&outbound);
                Self::push_tla_report(
                    queue,
                    outbound.req_handle,
                    TLA_REPORT_FAILED_TRANSFER,
                    Some(outbound.key.endpoint_id),
                );
            }
            return;
        }

        if ack.is_selective_segment_ack() {
            let max_segment_retransmissions = self
                .advanced_links
                .get(&self.outbound_al_messages[idx].key)
                .map(|session| session.max_segment_retransmissions)
                .unwrap_or(0);
            let max_tl_sdu_retransmissions = self
                .advanced_links
                .get(&self.outbound_al_messages[idx].key)
                .map(|session| session.max_tl_sdu_retransmissions)
                .unwrap_or(0);
            let mut acknowledged_segments = 0usize;
            let mut retransmit_segments = 0usize;
            let mut exhausted_segment = None;
            {
                let outbound = &mut self.outbound_al_messages[idx];
                for segment in &mut outbound.segments {
                    match ack.segment_acknowledged_in_first_block(segment.ss) {
                        Some(true) => {
                            if !segment.acknowledged {
                                acknowledged_segments += 1;
                            }
                            segment.acknowledged = true;
                            segment.retransmission_requested = false;
                            if let Some(reporter) = &segment.current_mac_reporter {
                                reporter.try_mark_transmitted();
                            }
                        }
                        Some(false) => {
                            if segment.retransmit_count < max_segment_retransmissions {
                                segment.acknowledged = false;
                                segment.retransmission_requested = true;
                                segment.t_submitted_to_umac = None;
                                segment.t_umac_done = None;
                                segment.current_mac_reporter = None;
                                retransmit_segments += 1;
                            } else {
                                exhausted_segment = Some(segment.ss);
                                break;
                            }
                        }
                        None => {
                            segment.retransmission_requested = false;
                        }
                    }
                }

                if retransmit_segments > 0 {
                    outbound.t_submitted_to_umac = None;
                    outbound.t_umac_done = None;
                    outbound.current_mac_reporter = None;
                    outbound.t_retransmissions_exhausted = None;
                    outbound.selective_segment_retry_pending = true;
                } else {
                    outbound.selective_segment_retry_pending = false;
                }
            }

            if let Some(ss) = exhausted_segment {
                let outbound = &mut self.outbound_al_messages[idx];
                if outbound.retransmit_count < max_tl_sdu_retransmissions {
                    outbound.retransmit_count += 1;
                    Self::reset_al_transmission_attempt(outbound);
                    tracing::warn!(
                        "LLC: AL selective ACK exhausted N.274 for SSI {} endpoint {} link {} N(S)={} S(S)={}; retransmitting complete TL-SDU attempt {} using N.273",
                        outbound.key.addr.ssi,
                        outbound.key.endpoint_id,
                        outbound.key.link_id,
                        outbound.ns,
                        ss,
                        outbound.retransmit_count
                    );
                    return;
                }

                let Some(mut outbound) = self.outbound_al_messages.remove(idx) else {
                    return;
                };
                tracing::warn!(
                    "LLC: AL selective ACK exhausted N.274 and N.273 for SSI {} endpoint {} link {} N(S)={} S(S)={}",
                    outbound.key.addr.ssi,
                    outbound.key.endpoint_id,
                    outbound.key.link_id,
                    outbound.ns,
                    ss
                );
                Self::mark_al_current_mac_discarded(&mut outbound);
                Self::mark_al_service_failed(&outbound);
                Self::push_tla_report(
                    queue,
                    outbound.req_handle,
                    TLA_REPORT_FAILED_TRANSFER,
                    Some(outbound.key.endpoint_id),
                );
                return;
            }

            tracing::info!(
                "LLC: AL selective {} for SSI {} endpoint {} N(R)={} sr={:?} ack_len={} acknowledged_segments={} retransmit_segments={}",
                if ack.receiver_ready { "ACK" } else { "RNR" },
                prim.main_address.ssi,
                prim.endpoint_id,
                ack.nr,
                ack.sr,
                ack.acknowledgement_length,
                acknowledged_segments,
                retransmit_segments
            );
            return;
        }

        tracing::debug!(
            "LLC: AL-RNR/unknown AL-ACK for SSI {} endpoint {} N(R)={} ack_len={} is not actionable; waiting for retry/complete ACK",
            prim.main_address.ssi,
            prim.endpoint_id,
            ack.nr,
            ack.acknowledgement_length
        );
    }

    fn rx_tma_unitdata_ind_bl(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_tma_unitdata_ind_bl");

        // Get header bits (again) and prepare MLE message
        let SapMsgInner::TmaUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let Some(mut pdu) = prim.pdu.take() else {
            tracing::warn!("LLC: rx_tma_unitdata_ind_bl received message with no pdu, ignoring");
            return;
        };
        let Some(bits) = pdu.peek_bits(4) else {
            tracing::warn!("insufficient bits: {}", pdu.dump_bin());
            return;
        };
        let Ok(pdu_type) = LlcPduType::try_from(bits) else {
            tracing::warn!("invalid pdu type: {} in {}", bits, pdu.dump_bin());
            return;
        };

        let (has_fcs, ns, nr) = match pdu_type {
            LlcPduType::BlAdata | LlcPduType::BlAdataFcs => match BlAdata::from_bitbuf(&mut pdu) {
                Ok(pdu) => {
                    tracing::debug!(ts=%self.dltime, "<- {:?}", pdu);
                    (pdu.has_fcs, Some(pdu.ns), Some(pdu.nr))
                }
                Err(e) => {
                    tracing::warn!("Failed parsing BlAdata: {:?} {}", e, pdu.dump_bin());
                    return;
                }
            },

            LlcPduType::BlData | LlcPduType::BlDataFcs => match BlData::from_bitbuf(&mut pdu) {
                Ok(pdu) => {
                    tracing::debug!(ts=%self.dltime, "<- {:?}", pdu);
                    (pdu.has_fcs, Some(pdu.ns), None)
                }
                Err(e) => {
                    tracing::warn!("Failed parsing BlData: {:?} {}", e, pdu.dump_bin());
                    return;
                }
            },
            LlcPduType::BlAck | LlcPduType::BlAckFcs => match BlAck::from_bitbuf(&mut pdu) {
                Ok(pdu) => {
                    tracing::debug!(ts=%self.dltime, "<- {:?}", pdu);
                    (pdu.has_fcs, None, Some(pdu.nr))
                }
                Err(e) => {
                    tracing::warn!("Failed parsing BlAck: {:?} {}", e, pdu.dump_bin());
                    return;
                }
            },
            LlcPduType::BlUdata | LlcPduType::BlUdataFcs => match BlUdata::from_bitbuf(&mut pdu) {
                Ok(pdu) => {
                    tracing::debug!(ts=%self.dltime, "<- {:?}", pdu);
                    (pdu.has_fcs, None, None)
                }
                Err(e) => {
                    tracing::warn!("Failed parsing BlUdata: {:?} {}", e, pdu.dump_bin());
                    return;
                }
            },
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        };

        let fcs_ok = !has_fcs || fcs::check_fcs(&pdu);
        if ns.is_some() && !fcs_ok {
            // EN 300 392-2 clause 22.3.2.3(k) and note 2: reception of a new
            // BL-DATA before the previous one is acknowledged stops the older
            // acknowledgement action independently of N(S). A bad FCS still
            // suppresses delivery and ACKing of the corrupt TL-SDU below.
            self.cancel_scheduled_out_ack_for_new_bl_data(prim.main_address, prim.endpoint_id);
        }
        if has_fcs && !fcs_ok {
            tracing::warn!("FCS check failed");
            if matches!(pdu_type, LlcPduType::BlDataFcs | LlcPduType::BlUdataFcs) {
                return;
            }
        }
        if has_fcs && fcs_ok {
            Self::strip_validated_fcs(&mut pdu);
        }
        if self.wap_ip_diag_enabled() {
            tracing::info!(
                "WAP/IP diag: LLC inbound BL addr={:?} endpoint={} pdu_type={} has_fcs={} fcs_ok={} ns={:?} nr={:?} tl_sdu_bits={} chan_change={} chan_info={:?}",
                prim.main_address,
                prim.endpoint_id,
                pdu_type,
                has_fcs,
                fcs_ok,
                ns,
                nr,
                pdu.get_len_remaining(),
                prim.chan_change_response_req,
                prim.chan_info
            );
        }

        // If N(S) is present, a valid TL-SDU needs an ACK. For BL-ADATA with a
        // bad optional FCS, process the independent N(R) ACK but do not ACK or
        // deliver the corrupt contained TL-SDU (EN 300 392-2 §22.3.2.3(j/l)).
        let msg_dltime = self.dltime.add_timeslots(-2); // Msg on uplink was sent two timeslots ago.
        let mut duplicate_inbound_ns = false;
        let tl_data_ind_req_handle = if let Some(ns) = ns
            && fcs_ok
        {
            self.prune_expired_inbound_receive_seq(self.dltime);
            let key = Self::basic_link_key(prim.main_address, prim.endpoint_id);
            duplicate_inbound_ns = self
                .inbound_receive_seq
                .get(&key)
                .is_some_and(|receive_seq| receive_seq.last_ns == ns);
            let handle = if duplicate_inbound_ns {
                // EN 300 392-2 clause 22.3.2.3 and Annex A.1: a peer may
                // retransmit a BL-DATA/BL-ADATA when our BL-ACK was lost.
                // Re-ACK the duplicate but do not create a second TL-DATA.ind.
                // If the original ACK is still pending, keep its indication
                // handle so a matching TL-DATA.response can still consume it;
                // otherwise use a fresh internal handle only for the BL-ACK
                // TMA request, avoiding a stale response/ACK association.
                self.scheduled_out_ack_handle_for_key(key)
                    .unwrap_or_else(|| self.next_tl_data_ind_req_handle())
            } else {
                let handle = self.next_tl_data_ind_req_handle();
                self.inbound_receive_seq.insert(
                    key,
                    ReceiveSeqState {
                        last_ns: ns,
                        received_at: msg_dltime,
                        ack_timeslot: msg_dltime.t,
                    },
                );
                handle
            };
            Some(handle)
        } else {
            None
        };
        if let Some(ns) = ns
            && fcs_ok
        {
            // Send ACK
            self.schedule_outgoing_ack(
                msg_dltime,
                prim.main_address,
                prim.endpoint_id,
                ns,
                tl_data_ind_req_handle.expect("N(S) data must have a TL-DATA.ind handle"),
                prim.air_interface_encryption,
            );
        }

        // if nr is present, we have received an ACK on a previous message
        let matching_ack = if let Some(nr) = nr {
            self.process_incoming_ack(queue, prim.main_address, prim.endpoint_id, nr)
        } else {
            None
        };

        let is_bl_ack = pdu_type == LlcPduType::BlAck || pdu_type == LlcPduType::BlAckFcs;
        if let Some(ack) = matching_ack {
            if is_bl_ack {
                // EN 300 392-2 clause 22.3.2.3(j): a matching BL-ACK may
                // acknowledge a previous TL-SDU and carry a peer TL-DATA
                // response; the response is bit-oriented and may be shorter
                // than one octet.
                let tl_sdu = if fcs_ok && pdu.get_len_remaining() > 0 {
                    pdu.set_raw_start(pdu.get_raw_pos());
                    Some(pdu)
                } else {
                    None
                };
                Self::push_tl_data_conf(
                    queue,
                    &ack,
                    tl_sdu,
                    prim.scrambling_code,
                    prim.new_endpoint_id,
                    prim.css_endpoint_id,
                    has_fcs,
                    prim.air_interface_encryption,
                    prim.chan_change_response_req,
                    prim.chan_change_handle,
                    prim.chan_info,
                );
                return;
            }

            if pdu_type == LlcPduType::BlAdata || pdu_type == LlcPduType::BlAdataFcs {
                Self::push_tl_data_conf(
                    queue,
                    &ack,
                    None,
                    prim.scrambling_code,
                    prim.new_endpoint_id,
                    prim.css_endpoint_id,
                    ack.fcs_flag,
                    ack.air_interface_encryption,
                    prim.chan_change_response_req,
                    prim.chan_change_handle,
                    prim.chan_info,
                );
                if !fcs_ok {
                    return;
                }
            }
        }

        if is_bl_ack {
            // EN 300 392-2 clause 22.3.2.3(j): a BL-ACK with unexpected or
            // wrong N(R) does not confirm the outstanding transfer, but any
            // contained TL-SDU is still delivered as TL-DATA.ind when FCS is
            // valid. If no TL-SDU is waiting, contained data is also delivered.
            if fcs_ok && pdu.get_len_remaining() > 0 {
                pdu.set_raw_start(pdu.get_raw_pos());
                queue.push_back(SapMsg {
                    sap: Sap::TlaSap,
                    src: TetraEntity::Llc,
                    dest: TetraEntity::Mle,
                    msg: SapMsgInner::TlaTlDataIndBl(TlaTlDataIndBl {
                        main_address: prim.main_address,
                        link_id: 0,
                        endpoint_id: prim.endpoint_id,
                        new_endpoint_id: prim.new_endpoint_id,
                        css_endpoint_id: prim.css_endpoint_id,
                        tl_sdu: Some(pdu),
                        scrambling_code: prim.scrambling_code,
                        fcs_flag: has_fcs,
                        air_interface_encryption: prim.air_interface_encryption,
                        chan_change_resp_req: prim.chan_change_response_req,
                        chan_change_handle: prim.chan_change_handle,
                        chan_info: prim.chan_info,
                        req_handle: self.next_tl_data_ind_req_handle(),
                    }),
                });
            }
            return;
        }

        if !fcs_ok {
            return;
        }

        if duplicate_inbound_ns {
            tracing::debug!(
                "LLC: suppressing duplicate inbound BL-DATA/BL-ADATA N(S)={} for SSI {} endpoint {}; ACK remains scheduled",
                ns.expect("duplicate flag is only set for N(S) PDUs"),
                prim.main_address.ssi,
                prim.endpoint_id
            );
            return;
        }

        // If unacknowledged data transfer service, we send a TL-UNITDATA indication
        // to MLE. If acknowledged data transfer service, we send a TL-DATA indication
        pdu.set_raw_start(pdu.get_raw_pos());
        let s = if pdu_type == LlcPduType::BlUdata || pdu_type == LlcPduType::BlUdataFcs {
            // Unacknowledged data transfer service
            let m = TlaTlUnitdataIndBl {
                main_address: prim.main_address,
                link_id: 0,
                endpoint_id: prim.endpoint_id,
                new_endpoint_id: prim.new_endpoint_id,
                css_endpoint_id: prim.css_endpoint_id,
                tl_sdu: if pdu.get_len_remaining() > 0 { Some(pdu) } else { None },
                scrambling_code: prim.scrambling_code,
                fcs_flag: has_fcs,
                air_interface_encryption: prim.air_interface_encryption,
                chan_change_resp_req: prim.chan_change_response_req,
                chan_change_handle: prim.chan_change_handle,
                chan_info: prim.chan_info,
                report: None,
            };
            SapMsg {
                sap: Sap::TlaSap,
                src: TetraEntity::Llc,
                dest: TetraEntity::Mle,
                msg: SapMsgInner::TlaTlUnitdataIndBl(m),
            }
        } else {
            // Acknowledged data transfer service
            let m = TlaTlDataIndBl {
                main_address: prim.main_address,
                link_id: 0,
                endpoint_id: prim.endpoint_id,
                new_endpoint_id: prim.new_endpoint_id,
                css_endpoint_id: prim.css_endpoint_id,
                tl_sdu: if pdu.get_len_remaining() > 0 { Some(pdu) } else { None },
                scrambling_code: prim.scrambling_code,
                fcs_flag: has_fcs,
                air_interface_encryption: prim.air_interface_encryption,
                chan_change_resp_req: prim.chan_change_response_req,
                chan_change_handle: prim.chan_change_handle,
                chan_info: prim.chan_info,
                req_handle: tl_data_ind_req_handle.expect("BL-DATA/BL-ADATA indication must have retained handle"),
            };
            SapMsg {
                sap: Sap::TlaSap,
                src: TetraEntity::Llc,
                dest: TetraEntity::Mle,
                msg: SapMsgInner::TlaTlDataIndBl(m),
            }
        };

        queue.push_back(s);
    }

    fn highest_priority_unsubmitted_al_index(&self, link_blocked: &HashSet<AdvancedLinkKey>) -> Option<usize> {
        let mut selected: Option<(usize, Todo)> = None;
        for (idx, al) in self.outbound_al_messages.iter().enumerate() {
            if al.t_submitted_to_umac.is_some() || link_blocked.contains(&Self::expected_al_key(al)) {
                continue;
            }
            match selected {
                Some((_, selected_prio)) if selected_prio >= al.pdu_prio => {}
                _ => selected = Some((idx, al.pdu_prio)),
            }
        }
        selected.map(|(idx, _)| idx)
    }

    fn submit_free_al_messages_to_umac(&mut self, queue: &mut MessageQueue) -> bool {
        let mut had_activity = false;
        let mut link_blocked: HashSet<AdvancedLinkKey> = self
            .outbound_al_messages
            .iter()
            .filter(|al| al.t_submitted_to_umac.is_some())
            .map(Self::expected_al_key)
            .collect();

        while let Some(idx) = self.highest_priority_unsubmitted_al_index(&link_blocked) {
            let key = Self::expected_al_key(&self.outbound_al_messages[idx]);
            let max_segment_retransmissions = self
                .advanced_links
                .get(&key)
                .map(|session| session.max_segment_retransmissions)
                .unwrap_or(0);
            let al = &mut self.outbound_al_messages[idx];
            tracing::debug!(
                "submitting AL TL-SDU for SSI {} endpoint {} link {} N(S) {} segments {} pdu_prio {} to umac",
                key.addr.ssi,
                key.endpoint_id,
                key.link_id,
                al.ns,
                al.segments.len(),
                al.pdu_prio
            );
            Self::submit_for_al_transmission(
                queue,
                al,
                self.dltime.forward_to_timeslot(al.t_first.t),
                max_segment_retransmissions,
            );
            link_blocked.insert(key);
            had_activity = true;
        }

        had_activity
    }

    fn submit_al_retransmissions_to_umac(&mut self, queue: &mut MessageQueue) -> bool {
        let mut had_activity = false;
        let dltime = self.dltime;
        let mut removals: Vec<usize> = Vec::new();

        for idx in 0..self.outbound_al_messages.len() {
            let al = &mut self.outbound_al_messages[idx];
            if al.t_retransmissions_exhausted.is_some() {
                if Self::al_late_ack_grace_expired(al, dltime) {
                    removals.push(idx);
                }
                continue;
            }

            let mac_state = Self::al_current_mac_state(al);
            if al.t_umac_done.is_none() && matches!(mac_state, Some(TxState::Transmitted | TxState::Discarded)) {
                al.t_umac_done = Some(dltime);
                if mac_state == Some(TxState::Transmitted) && !al.first_complete_report_sent {
                    Self::mark_al_service_first_complete(al);
                    Self::push_tla_report(
                        queue,
                        al.req_handle,
                        TLA_REPORT_FIRST_COMPLETE_TRANSMISSION,
                        Some(al.key.endpoint_id),
                    );
                    al.first_complete_report_sent = true;
                    had_activity = true;
                }
            }

            let Some(t_umac_done) = al.t_umac_done else {
                continue;
            };
            let age = Self::t251_downlink_frames_elapsed(t_umac_done, dltime, al.ack_timeslot, al.ack_stealing_repeats_flag);
            if age < T252_AL_ACK_WAITING_SIGNALLING_FRAMES {
                continue;
            }

            let segment_retry_exhausted = if al.selective_segment_retry_pending {
                let max_segment_retransmissions = self
                    .advanced_links
                    .get(&al.key)
                    .map(|session| session.max_segment_retransmissions)
                    .unwrap_or(0);
                match Self::schedule_requested_al_segment_retries(al, max_segment_retransmissions) {
                    Ok(retry_segments) if retry_segments > 0 => {
                        tracing::info!(
                            "LLC: AL TL-SDU SSI {} endpoint {} link {} N(S) {} retrying {} selectively requested segment(s) after T.252 using N.274",
                            al.key.addr.ssi,
                            al.key.endpoint_id,
                            al.key.link_id,
                            al.ns,
                            retry_segments
                        );
                        had_activity = true;
                        continue;
                    }
                    Ok(_) => None,
                    Err(ss) => Some(ss),
                }
            } else {
                None
            };

            let max_retransmissions = self
                .advanced_links
                .get(&al.key)
                .map(|session| session.max_tl_sdu_retransmissions)
                .unwrap_or(0);
            if let Some(ss) = segment_retry_exhausted {
                tracing::warn!(
                    "LLC: AL TL-SDU SSI {} endpoint {} link {} N(S) {} segment S(S) {} exhausted N.274; falling back to N.273 TL-SDU retransmission policy",
                    al.key.addr.ssi,
                    al.key.endpoint_id,
                    al.key.link_id,
                    al.ns,
                    ss
                );
            }
            if al.retransmit_count < max_retransmissions {
                al.retransmit_count += 1;
                al.t_retransmissions_exhausted = None;
                Self::reset_al_transmission_attempt(al);
                tracing::info!(
                    "LLC: retransmitting AL TL-SDU SSI {} endpoint {} link {} N(S) {} attempt {}",
                    al.key.addr.ssi,
                    al.key.endpoint_id,
                    al.key.link_id,
                    al.ns,
                    al.retransmit_count
                );
                had_activity = true;
            } else {
                if !al.first_complete_report_sent {
                    removals.push(idx);
                    continue;
                }
                if al.t_retransmissions_exhausted.is_none() {
                    al.t_retransmissions_exhausted = Some(dltime);
                    tracing::warn!(
                        "LLC: AL TL-SDU SSI {} endpoint {} link {} N(S) {} exhausted retransmissions after MAC success; waiting for late peer AL-ACK grace on TS{}",
                        al.key.addr.ssi,
                        al.key.endpoint_id,
                        al.key.link_id,
                        al.ns,
                        al.ack_timeslot
                    );
                    had_activity = true;
                } else if Self::al_late_ack_grace_expired(al, dltime) {
                    removals.push(idx);
                }
            }
        }

        for idx in removals.into_iter().rev() {
            let Some(mut al) = self.outbound_al_messages.remove(idx) else {
                continue;
            };
            tracing::warn!(
                "LLC: AL TL-SDU SSI {} endpoint {} link {} N(S) {} exhausted retransmissions",
                al.key.addr.ssi,
                al.key.endpoint_id,
                al.key.link_id,
                al.ns
            );
            Self::mark_al_current_mac_discarded(&mut al);
            Self::mark_al_service_failed(&al);
            Self::push_tla_report(queue, al.req_handle, TLA_REPORT_FAILED_TRANSFER, Some(al.key.endpoint_id));
            had_activity = true;
        }

        had_activity
    }

    fn submit_retransmissions_to_umac(&mut self, queue: &mut MessageQueue) -> bool {
        let mut had_activity = false;
        let dltime = self.dltime;
        let mut removals: Option<Vec<BasicLinkKey>> = None;

        // if !self.outbound_messages.is_empty() {
        //     tracing::error!("{}", Self::format_expected_ack_list(&self.outbound_messages));
        // }

        for ack in self.outbound_messages.iter_mut() {
            had_activity |= Self::reconcile_umac_done_from_reporter(queue, ack, dltime, "schedule_retransmissions");

            if ack.t_retransmissions_exhausted.is_some() {
                if Self::late_ack_grace_expired(ack, dltime) {
                    removals.get_or_insert(Vec::new()).push(Self::expected_ack_key(ack));
                }
                continue;
            }

            // If we don't have a t_umac_done, there is no need for a retransmission in any case
            let Some(t_umac_done) = ack.t_umac_done else {
                continue;
            };

            // Retransmit scenario 1: it was transmitted but no ack received within the expected window (ETSI T.251 / N.252)
            // Retransmission scenario 2: it has been dropped by Umac due to congestion. Retransmit after same window
            let age = Self::t251_downlink_frames_elapsed(t_umac_done, dltime, ack.ts, ack.stealing_repeats_flag);
            if age >= T251_SENDER_RETRY_SIGNALLING_FRAMES {
                // Time for either retransmitting or giving up
                if ack.retransmit_count < N252_BL_MAX_TLSDU_RETRANSMITS_ACKED {
                    // Retransmit
                    ack.retransmit_count += 1;
                    ack.t_retransmissions_exhausted = None;
                    tracing::info!(
                        "retransmitting SSI {} N(S) {} attempt {}",
                        ack.addr.ssi,
                        ack.ns,
                        ack.retransmit_count
                    );

                    Self::fold_waiting_ack_into_mac_ready_bl_data(
                        self.config.config().cell.main_carrier,
                        queue,
                        &mut self.scheduled_out_acks,
                        ack,
                    );
                    Self::submit_for_acknowledged_transmission(queue, ack, self.dltime.forward_to_timeslot(ack.t_first.t));
                    had_activity = true;
                } else if let Some(grace_frames) = Self::channel_allocation_late_ack_grace(ack) {
                    ack.t_retransmissions_exhausted = Some(dltime);
                    tracing::warn!(
                        "schedule_retransmissions: SSI {} N(S) {} exhausted retransmissions; retaining channel-allocation transfer for {} signalling-frame late-ACK grace",
                        ack.addr.ssi,
                        ack.ns,
                        grace_frames
                    );
                } else {
                    // Exhausted retransmissions, flag for discard
                    removals.get_or_insert(Vec::new()).push(Self::expected_ack_key(ack));
                }
            }
        }

        // Remove any expired entries
        if let Some(removals) = removals {
            for key in removals {
                // addr was just collected from expected_acks above, so the entry exists.
                // Use if-let rather than unwrap so a future refactor of the collection
                // logic can't panic the LLC worker here.
                let Some(ack) = self.take_expected_ack_for_key(key) else {
                    tracing::debug!(
                        "schedule_retransmissions: expected ACK for address {} endpoint {} already gone, skipping",
                        key.addr,
                        key.endpoint_id
                    );
                    continue;
                };
                tracing::warn!(
                    "schedule_retransmissions: SSI {} N(S) {} exhausted retransmissions",
                    ack.addr.ssi,
                    ack.ns
                );
                if ack.tx_reporter.get_state() != TxState::Transmitted {
                    tracing::warn!(
                        "schedule_retransmissions: SSI {} N(S) {} exhausted before service reporter reached Transmitted; state {:?}",
                        ack.addr.ssi,
                        ack.ns,
                        ack.tx_reporter.get_state()
                    );
                }
                Self::mark_ack_service_failed(&ack);
                Self::push_tla_report(queue, ack.req_handle, TLA_REPORT_FAILED_TRANSFER, Some(ack.endpoint_id));
            }
            // The ack expires here
        }

        had_activity
    }

    /// EN 300 392-2 Annex A.1 defines T.251 in downlink signalling frames,
    /// not raw TDMA timeslots. For the current single-slot common control path,
    /// count only arrivals of the downlink timeslot where the LLC PDU is expected.
    fn downlink_signalling_frames_elapsed(start: TdmaTime, now: TdmaTime, target_timeslot: u8) -> u32 {
        if !(1..=4).contains(&target_timeslot) {
            return 0;
        }

        let first_counted = start.add_timeslots(1).forward_to_timeslot(target_timeslot);
        let diff = now.diff(first_counted);
        if diff < 0 {
            return 0;
        }

        (diff as u32 / 4) + 1
    }

    fn all_downlink_frames_elapsed(start: TdmaTime, now: TdmaTime) -> u32 {
        let first_counted = start.add_timeslots(1).forward_to_timeslot(1);
        let diff = now.diff(first_counted);
        if diff < 0 {
            return 0;
        }

        (diff as u32 / TDMA_TIMESLOTS_PER_FRAME) + 1
    }

    fn t251_downlink_frames_elapsed(start: TdmaTime, now: TdmaTime, target_timeslot: u8, stealing_repeats_flag: Option<bool>) -> u32 {
        if stealing_repeats_flag == Some(true) {
            Self::all_downlink_frames_elapsed(start, now)
        } else {
            Self::downlink_signalling_frames_elapsed(start, now, target_timeslot)
        }
    }

    fn acknowledged_payload_for_rebuild(pdu: &BitBuffer) -> Option<RebuildableAckTransfer> {
        let mut pdu = pdu.clone();
        let pdu_type = pdu.peek_bits(4).and_then(|bits| LlcPduType::try_from(bits).ok())?;
        let (has_fcs, embedded_nr) = match pdu_type {
            LlcPduType::BlData | LlcPduType::BlDataFcs => {
                let header = BlData::from_bitbuf(&mut pdu).ok()?;
                (header.has_fcs, None)
            }
            LlcPduType::BlAdata | LlcPduType::BlAdataFcs => {
                let header = BlAdata::from_bitbuf(&mut pdu).ok()?;
                (header.has_fcs, Some(header.nr))
            }
            _ => return None,
        };
        if has_fcs {
            if pdu.get_len_remaining() < 32 {
                return None;
            }
            let payload_end = pdu.get_raw_end() - 32;
            pdu.set_raw_end(payload_end);
        }
        pdu.set_raw_start(pdu.get_raw_pos());
        Some(RebuildableAckTransfer {
            has_fcs,
            tl_sdu: pdu,
            embedded_nr,
        })
    }

    fn rebuild_retransmission_as_bl_data(expected_ack: &mut ExpectedInAck, mut tl_sdu: BitBuffer, has_fcs: bool) {
        let SapMsgInner::TmaUnitdataReq(prim) = &mut expected_ack.retransmission_buf.msg else {
            return;
        };
        let mut pdu_buf = BitBuffer::new_autoexpand(32);
        let pdu = BlData {
            has_fcs,
            ns: expected_ack.ns,
        };
        pdu.to_bitbuf(&mut pdu_buf);
        Self::append_tl_sdu_and_optional_fcs(&mut pdu_buf, &mut tl_sdu, has_fcs);
        prim.pdu = pdu_buf;
    }

    fn rebuild_retransmission_as_bl_adata(expected_ack: &mut ExpectedInAck, mut tl_sdu: BitBuffer, has_fcs: bool, nr: u8) {
        let SapMsgInner::TmaUnitdataReq(prim) = &mut expected_ack.retransmission_buf.msg else {
            return;
        };
        let mut pdu_buf = BitBuffer::new_autoexpand(32);
        let pdu = BlAdata {
            has_fcs,
            nr,
            ns: expected_ack.ns,
        };
        pdu.to_bitbuf(&mut pdu_buf);
        Self::append_tl_sdu_and_optional_fcs(&mut pdu_buf, &mut tl_sdu, has_fcs);
        prim.pdu = pdu_buf;
    }

    fn rewrite_retransmission_ns(expected_ack: &mut ExpectedInAck, ns: u8) {
        if expected_ack.ns == ns {
            return;
        }

        let rebuildable = match &expected_ack.retransmission_buf.msg {
            SapMsgInner::TmaUnitdataReq(prim) => Self::acknowledged_payload_for_rebuild(&prim.pdu),
            _ => None,
        };
        expected_ack.ns = ns;
        if let Some(rebuildable) = rebuildable {
            if let Some(nr) = rebuildable.embedded_nr {
                // EN 300 392-2 clause 22.3.2.3(a)(v): if LLC cancels a
                // lower-priority BL-ADATA before transmission, it memorizes
                // the ACK N(R). Rewriting the DATA N(S) for priority order
                // must not silently drop that embedded acknowledgement.
                Self::rebuild_retransmission_as_bl_adata(expected_ack, rebuildable.tl_sdu, rebuildable.has_fcs, nr);
            } else {
                Self::rebuild_retransmission_as_bl_data(expected_ack, rebuildable.tl_sdu, rebuildable.has_fcs);
            }
        }
    }

    fn fold_waiting_ack_into_mac_ready_bl_data(
        main_carrier: u16,
        queue: &mut MessageQueue,
        scheduled_out_acks: &mut VecDeque<ScheduledOutAck>,
        expected_ack: &mut ExpectedInAck,
    ) {
        let waiting_ack_index = scheduled_out_acks
            .iter()
            .position(|ack| ack.addr == expected_ack.addr && ack.endpoint_id == expected_ack.endpoint_id);
        let SapMsgInner::TmaUnitdataReq(prim) = &mut expected_ack.retransmission_buf.msg else {
            return;
        };
        let Some(rebuildable) = Self::acknowledged_payload_for_rebuild(&prim.pdu) else {
            tracing::warn!(
                "LLC: cannot rebuild queued acknowledged transfer for SSI {} endpoint {}; queued PDU is not BL-DATA/BL-ADATA",
                expected_ack.addr.ssi,
                expected_ack.endpoint_id
            );
            return;
        };
        let RebuildableAckTransfer {
            has_fcs,
            mut tl_sdu,
            embedded_nr,
        } = rebuildable;

        let Some(waiting_ack_index) = waiting_ack_index else {
            if embedded_nr.is_some() && expected_ack.retransmit_count > 0 && expected_ack.first_complete_report_sent {
                // EN 300 392-2 clause 22.3.2.3 note 2 stops acknowledgement
                // actions for a previous received BL-DATA when a newer BL-DATA
                // arrives before acknowledgement. If a stored BL-ADATA is
                // retransmitted after a complete transmission and with no
                // matching waiting ACK, retransmit only the data part so the
                // old N(R) is not repeated. First submission and pre-delivery
                // MAC failures keep the ACK that was deliberately folded in.
                Self::rebuild_retransmission_as_bl_data(expected_ack, tl_sdu, has_fcs);
            }
            return;
        };

        let waiting_ack = scheduled_out_acks
            .remove(waiting_ack_index)
            .expect("scheduled ACK index came from the same queue");
        if Self::bl_adata_exceeds_sch_f_capacity(main_carrier, expected_ack.addr, prim.chan_alloc.as_ref(), &tl_sdu, has_fcs) {
            // EN 300 392-2 clause 22.3.2.3(d): when BL-ADATA would not fit in
            // this MAC block, issue the acknowledgement as standalone BL-ACK
            // and keep the TL-SDU as BL-DATA. Rebuild even if the stored retry
            // was already BL-ADATA so a stale N(R) is stripped.
            Self::push_standalone_bl_ack(queue, &waiting_ack);
            Self::rebuild_retransmission_as_bl_data(expected_ack, tl_sdu, has_fcs);
            return;
        }

        let SapMsgInner::TmaUnitdataReq(prim) = &mut expected_ack.retransmission_buf.msg else {
            return;
        };
        let mut pdu_buf = BitBuffer::new_autoexpand(32);
        let pdu = BlAdata {
            has_fcs,
            nr: waiting_ack.nr,
            ns: expected_ack.ns,
        };
        pdu.to_bitbuf(&mut pdu_buf);
        Self::append_tl_sdu_and_optional_fcs(&mut pdu_buf, &mut tl_sdu, has_fcs);
        tracing::debug!(
            "LLC: folded waiting ACK N(R)={} into queued BL-DATA N(S)={} as BL-ADATA for SSI {} endpoint {}",
            waiting_ack.nr,
            expected_ack.ns,
            expected_ack.addr.ssi,
            expected_ack.endpoint_id
        );
        prim.pdu = pdu_buf;
    }

    fn unsubmitted_ack_indices_for_key(&self, key: BasicLinkKey) -> Vec<usize> {
        self.outbound_messages
            .iter()
            .enumerate()
            .filter_map(|(idx, ack)| (Self::expected_ack_key(ack) == key && ack.t_submitted_to_umac.is_none()).then_some(idx))
            .collect()
    }

    fn normalize_unsubmitted_ns_for_key(&mut self, key: BasicLinkKey) {
        let indices = self.unsubmitted_ack_indices_for_key(key);
        if indices.len() < 2 {
            return;
        }

        let ns_sequence: Vec<u8> = indices.iter().map(|idx| self.outbound_messages[*idx].ns).collect();
        let mut priority_order = indices;
        priority_order.sort_by(|a, b| {
            let a_ack = &self.outbound_messages[*a];
            let b_ack = &self.outbound_messages[*b];
            b_ack.pdu_prio.cmp(&a_ack.pdu_prio).then_with(|| a.cmp(b))
        });

        for (idx, ns) in priority_order.into_iter().zip(ns_sequence) {
            let ack = &mut self.outbound_messages[idx];
            Self::rewrite_retransmission_ns(ack, ns);
        }
    }

    fn highest_priority_unsubmitted_ack_index(&self, link_blocked: &HashSet<BasicLinkKey>) -> Option<usize> {
        let mut selected: Option<(usize, Todo)> = None;
        for (idx, ack) in self.outbound_messages.iter().enumerate() {
            if ack.t_submitted_to_umac.is_some() || link_blocked.contains(&Self::expected_ack_key(ack)) {
                continue;
            }
            match selected {
                Some((_, selected_prio)) if selected_prio >= ack.pdu_prio => {}
                _ => selected = Some((idx, ack.pdu_prio)),
            }
        }
        selected.map(|(idx, _)| idx)
    }

    fn highest_unsubmitted_pdu_prio_for_key(&self, key: BasicLinkKey) -> Option<Todo> {
        self.outbound_messages
            .iter()
            .filter(|ack| ack.t_submitted_to_umac.is_none() && Self::expected_ack_key(ack) == key)
            .map(|ack| ack.pdu_prio)
            .max()
    }

    fn cancel_lower_priority_pending_mac_transfers_for_emergency(&mut self, queue: &mut MessageQueue) -> bool {
        let cancel_ack_indices: Vec<usize> = self
            .outbound_messages
            .iter()
            .enumerate()
            .filter_map(|(idx, ack)| {
                if ack.t_submitted_to_umac.is_none()
                    || ack.t_umac_done.is_some()
                    || Self::ack_current_mac_state(ack) != Some(TxState::Pending)
                    || ack.pdu_prio >= TMA_HIGHEST_PDU_PRIORITY
                {
                    return None;
                }

                let key = Self::expected_ack_key(ack);
                let highest_waiting_prio = self.highest_unsubmitted_pdu_prio_for_key(key)?;
                (highest_waiting_prio >= TMA_HIGHEST_PDU_PRIORITY && highest_waiting_prio > ack.pdu_prio).then_some(idx)
            })
            .collect();
        let cancel_udata_indices: Vec<usize> = self
            .outbound_udata_messages
            .iter()
            .enumerate()
            .filter_map(|(idx, udata)| {
                if !udata.submitted
                    || udata.defer_mac_ready_once
                    || udata.pdu_prio >= TMA_HIGHEST_PDU_PRIORITY
                    || Self::queued_udata_tx_state(udata).is_some_and(|state| state != TxState::Pending)
                {
                    return None;
                }

                let key = Self::queued_udata_key(udata);
                let highest_waiting_prio = self.highest_unsubmitted_pdu_prio_for_key(key)?;
                (highest_waiting_prio >= TMA_HIGHEST_PDU_PRIORITY && highest_waiting_prio > udata.pdu_prio).then_some(idx)
            })
            .collect();

        let mut had_activity = false;
        for idx in cancel_ack_indices {
            let ack = &mut self.outbound_messages[idx];
            // EN 300 392-2 clause 20.4.1.1.1 defines TMA-CANCEL for a
            // TMA-UNITDATA.req already submitted by LLC. Use it only while
            // the retained reporter is still Pending; once MAC reports a
            // complete transmission, clause 22.3.2.3's BL-ACK/T.251 path owns
            // completion. Table 20.54 defines priority level 7 as the highest
            // TMA priority, so only that level may displace lower-priority
            // untransmitted BL-DATA on the same basic link.
            tracing::info!(
                "LLC: cancelling pending TMA req_handle={} prio={} for highest-priority BL-DATA on SSI {} endpoint {}",
                ack.req_handle,
                ack.pdu_prio,
                ack.addr.ssi,
                ack.endpoint_id
            );
            Self::push_tma_cancel(queue, ack.req_handle);
            ack.t_submitted_to_umac = None;
            ack.t_umac_done = None;
            Self::mark_ack_current_mac_discarded(ack);
            ack.current_mac_reporter = None;
            had_activity = true;
        }
        for idx in cancel_udata_indices {
            let udata = &mut self.outbound_udata_messages[idx];
            // EN 300 392-2 clause 22.3.2.3(a)(v) permits a highest-priority
            // acknowledged TL-DATA to cancel lower-priority TL-DATA or
            // TL-UNITDATA already submitted to MAC but not yet transmitted.
            // Clause 20.4.1.1.1 supplies TMA-CANCEL for that submitted
            // TMA-UNITDATA.req. Keep the BL-UDATA buffered for the remaining
            // N.253 + 1 transmission attempts, but skip the current MAC-ready
            // turn so the cancelled request is not immediately re-issued.
            tracing::info!(
                "LLC: cancelling pending BL-UDATA TMA req_handle={} prio={} for highest-priority BL-DATA on SSI {} endpoint {}",
                udata.req_handle,
                udata.pdu_prio,
                udata.addr.ssi,
                udata.endpoint_id
            );
            Self::push_tma_cancel(queue, udata.req_handle);
            udata.submitted = false;
            // EN 300 392-2 clauses 20.4.1.1.1 and 22.3.2.3(a)(v): cancel
            // the submitted MAC request, not the stored unacknowledged TL-SDU.
            // The service reporter remains pending; the per-attempt MAC
            // reporter is one-shot and will not be reused on the resubmission.
            Self::mark_udata_current_mac_discarded(udata);
            udata.defer_mac_ready_once = true;
            had_activity = true;
        }

        had_activity
    }

    fn submit_free_messages_to_umac(&mut self, queue: &mut MessageQueue) -> bool {
        let mut had_activity = self.cancel_lower_priority_pending_mac_transfers_for_emergency(queue);
        let mut link_blocked: HashSet<BasicLinkKey> = self
            .outbound_messages
            .iter()
            .filter(|ack| ack.t_submitted_to_umac.is_some())
            .map(Self::expected_ack_key)
            .collect();
        let main_carrier = self.config.config().cell.main_carrier;

        while let Some(idx) = self.highest_priority_unsubmitted_ack_index(&link_blocked) {
            let key = Self::expected_ack_key(&self.outbound_messages[idx]);
            self.normalize_unsubmitted_ns_for_key(key);
            let ack = &mut self.outbound_messages[idx];
            Self::fold_waiting_ack_into_mac_ready_bl_data(main_carrier, queue, &mut self.scheduled_out_acks, ack);
            tracing::debug!(
                "submitting message for SSI {} endpoint {} N(S) {} pdu_prio {} to umac: {:?}",
                ack.addr.ssi,
                ack.endpoint_id,
                ack.ns,
                ack.pdu_prio,
                ack.retransmission_buf.msg
            );
            Self::submit_for_acknowledged_transmission(queue, ack, self.dltime.forward_to_timeslot(ack.t_first.t));
            link_blocked.insert(key);
            had_activity = true;
        }

        had_activity
    }

    /// Sends standalone BL-ACKs left after MAC-ready TL-DATA piggybacking has had a chance to consume them.
    fn submit_ack_replies_to_umac(&mut self, queue: &mut MessageQueue) -> bool {
        let mut had_activity = false;
        while let Some(ack) = self.scheduled_out_acks.pop_front() {
            // EN 300 392-2 clause 22.3.2.3(d): at MAC-READY, if a waiting
            // acknowledgement cannot be combined with an available TL-DATA
            // response/request as BL-ACK-with-data or BL-ADATA, issue a
            // standalone BL-ACK without service user data.
            Self::push_standalone_bl_ack(queue, &ack);
            had_activity = true;
        }
        had_activity
    }

    /// Pops all elements from the scheduled_out_acks queue, prepares BL-ACK messages, and send them down
    fn submit_udata_msgs_to_umac(&mut self, queue: &mut MessageQueue) -> bool {
        let mut had_activity = false;

        while let Some(idx) = Self::highest_priority_unsubmitted_udata_index(&self.outbound_udata_messages) {
            let msg = &mut self.outbound_udata_messages[idx];
            let mac_reporter = TxReporter::new_unacked();
            let mut sapmsg = msg.sapmsg.clone();
            let SapMsgInner::TmaUnitdataReq(prim) = &mut sapmsg.msg else {
                panic!("queued BL-UDATA must retain a TMA-UNITDATA request");
            };
            prim.tx_reporter = Some(mac_reporter.clone());
            tracing::debug!(
                "submitting BL-UDATA req_handle={} SSI {} pdu_prio {} complete {}/{} failed {} from {:?} to umac: {:?}",
                msg.req_handle,
                msg.addr.ssi,
                msg.pdu_prio,
                msg.complete_transmissions,
                msg.target_complete_transmissions,
                msg.failed_transmissions,
                msg.t_first,
                sapmsg.msg
            );
            queue.push_back(sapmsg);
            msg.current_mac_reporter = Some(mac_reporter);
            msg.submitted = true;
            had_activity = true;
        }

        for msg in &mut self.outbound_udata_messages {
            msg.defer_mac_ready_once = false;
        }

        had_activity
    }

    fn highest_priority_unsubmitted_udata_index(messages: &VecDeque<QueuedUdata>) -> Option<usize> {
        let mut selected: Option<(usize, Todo)> = None;
        for (idx, msg) in messages.iter().enumerate() {
            if msg.submitted || msg.defer_mac_ready_once {
                continue;
            }
            match selected {
                Some((_, selected_prio)) if selected_prio >= msg.pdu_prio => {}
                _ => selected = Some((idx, msg.pdu_prio)),
            }
        }
        selected.map(|(idx, _)| idx)
    }

    fn format_expected_ack_list(ack_list: &VecDeque<ExpectedInAck>) -> String {
        let mut ret = String::new();
        ret.push_str("Expected in acks:\n");
        for ack in ack_list {
            ret.push_str(&format!(
                "  ssi: {}, n: {}, retransmissions: {}, t_first: {:?}, t_umac_done: {:?}, state: {:?}\n",
                ack.addr.ssi,
                ack.ns,
                ack.retransmit_count,
                ack.t_first,
                ack.t_umac_done,
                ack.tx_reporter.get_state()
            ));
        }
        ret
    }

    fn format_scheduled_ack_list(ack_list: &Vec<ScheduledOutAck>) -> String {
        let mut ret = String::new();
        ret.push_str("Scheduled out acks:\n");
        for ack in ack_list {
            ret.push_str(&format!("  t_start: {}, ssi: {}, n: {}\n", ack.t_start.t, ack.addr.ssi, ack.nr));
        }
        ret
    }
}

impl TetraEntityTrait for Llc {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Llc
    }

    fn set_config(&mut self, config: SharedConfig) {
        self.config = config;
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);
        // tracing::debug!(ts=%message.dltime, "rx_prim: {:?}", message);

        match message.sap {
            Sap::TmaSap => {
                self.rx_tma_prim(queue, message);
            }

            // TMB-SAP and TMC-SAP are skipped and passed straight between MAC and MLE
            Sap::TlaSap => {
                self.rx_tla_prim(queue, message);
            }
            _ => {
                tracing::warn!("unhandled match variant, ignoring");
            }
        }
    }

    fn tick_start(&mut self, _queue: &mut MessageQueue, ts: TdmaTime) {
        self.dltime = ts;
    }

    fn tick_end(&mut self, queue: &mut MessageQueue, _ts: TdmaTime) -> bool {
        let mut had_activity = false;

        // AL runs before BL so established packet-data advanced links do not
        // fall back onto the basic-link service while XHTML is active.
        had_activity |= self.submit_al_retransmissions_to_umac(queue);
        had_activity |= self.submit_free_al_messages_to_umac(queue);

        // Step 1 / 4: Check if we have any transmitted messages that were not acked within the expected window
        // Schedule a retransmission if appropriate.
        had_activity |= self.submit_retransmissions_to_umac(queue);

        // Step 2 / 4: Check if there are any messages that were not yet sent down, that we can now send down the stack
        // Messages may be kept since the target SSI has not yet acked them . If the link is now free, we can send the message down and register that we expect an ACK for it.
        had_activity |= self.submit_free_messages_to_umac(queue);

        // Step 3 / 4: Check if any unsent ACKs are still here
        // Take oldest element from scheduled_out_acks, and remove it from the list
        had_activity |= self.submit_ack_replies_to_umac(queue);

        // Step 4 / 4: Send any U-DATA messages
        had_activity |= self.submit_udata_msgs_to_umac(queue);

        had_activity
    }
}

#[cfg(test)]
mod tests {
    use super::{
        INBOUND_DUPLICATE_SUPPRESSION_SIGNALLING_FRAMES, LLC_MAX_OUTBOUND_ACKED_MESSAGES, LLC_MAX_OUTBOUND_UDATA_MESSAGES, Llc,
        QueuedUdata, ReceiveSeqState, ScheduledOutAck, T251_SENDER_RETRY_SIGNALLING_FRAMES, TDMA_TIMESLOTS_PER_FRAME,
    };
    use std::collections::VecDeque;
    use tetra_core::tetra_entities::TetraEntity;
    use tetra_core::{BitBuffer, EndpointId, Sap, SsiType, TdmaTime, TetraAddress, Todo, TxReporter, TxState};
    use tetra_pdus::llc::consts::timers::T251_SENDER_RETRY_TIMER;
    use tetra_saps::lcmc::enums::alloc_type::ChanAllocType;
    use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
    use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
    use tetra_saps::tla::TLA_REPORT_FAILED_TRANSFER;
    use tetra_saps::tla::TlaTlDataReqBl;
    use tetra_saps::tma::TmaUnitdataReq;
    use tetra_saps::{SapMsg, SapMsgInner};

    use crate::MessageQueue;

    fn queued_udata_for_test(req_handle: Todo, pdu_prio: Todo, submitted: bool, reporter: Option<TxReporter>) -> QueuedUdata {
        let addr = TetraAddress::new(1001, SsiType::Issi);
        let endpoint_id: EndpointId = 0;
        QueuedUdata {
            addr,
            t_first: TdmaTime::default(),
            req_handle,
            endpoint_id,
            pdu_prio,
            sapmsg: SapMsg {
                sap: Sap::TmaSap,
                src: TetraEntity::Llc,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
                    req_handle,
                    pdu: BitBuffer::from_bytes(&[0x55]),
                    main_address: addr,
                    endpoint_id,
                    pdu_prio,
                    stealing_permission: false,
                    subscriber_class: 0,
                    air_interface_encryption: None,
                    stealing_repeats_flag: None,
                    data_category: None,
                    chan_alloc: None,
                    tx_reporter: None,
                }),
            },
            service_tx_reporter: reporter,
            current_mac_reporter: None,
            n253: 0,
            target_complete_transmissions: 1,
            complete_transmissions: 0,
            failed_transmissions: 0,
            submitted,
            defer_mac_ready_once: false,
        }
    }

    fn pop_failed_report_req_handle(queue: &mut MessageQueue) -> Option<Todo> {
        let msg = queue.pop_front()?;
        let SapMsgInner::TlaTlReportInd(report) = msg.msg else {
            panic!("expected TL-REPORT.ind");
        };
        assert_eq!(msg.sap, Sap::TlaSap);
        assert_eq!(msg.src, TetraEntity::Llc);
        assert_eq!(msg.dest, TetraEntity::Mle);
        assert_eq!(report.report, TLA_REPORT_FAILED_TRANSFER);
        report.req_handle
    }

    #[test]
    fn llc_timer_signalling_frame_constants_match_annex_timer_constants() {
        assert_eq!(
            T251_SENDER_RETRY_SIGNALLING_FRAMES * TDMA_TIMESLOTS_PER_FRAME,
            T251_SENDER_RETRY_TIMER
        );
    }

    #[test]
    fn acked_channel_allocation_sent_on_mcch_expects_peer_ack_on_current_channel() {
        let mut assigned = [false; 4];
        assigned[1] = true;
        let mut prim = TlaTlDataReqBl {
            main_address: TetraAddress::issi(1001),
            link_id: 0,
            endpoint_id: 0,
            tl_sdu: BitBuffer::from_bitstr("101010"),
            pdu_prio: 6,
            stealing_permission: false,
            subscriber_class: 0,
            fcs_flag: false,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_class_info: None,
            req_handle: 1,
            graceful_degradation: None,
            chan_alloc: Some(CmceChanAllocReq {
                usage: Some(4),
                timeslots: assigned,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
                carrier: None,
            }),
            tx_reporter: None,
        };

        assert_eq!(
            Llc::expected_ack_timeslot_for_outbound_bl(&prim),
            1,
            "EN 300 392-2 23.5.2.2/23.5.4.3: MCCH late-assignment call-control grants the BL-ACK on the current channel"
        );

        prim.stealing_permission = true;
        assert_eq!(
            Llc::expected_ack_timeslot_for_outbound_bl(&prim),
            2,
            "FACCH/STCH acknowledged signalling still ages T.251 against the assigned traffic timeslot"
        );
    }

    #[test]
    fn original_al_packet_data_endpoint_zero_nonzero_link_expects_pdch_ack_timeslot() {
        let prim = TlaTlDataReqBl {
            main_address: TetraAddress::issi(1001),
            link_id: 1,
            endpoint_id: 0,
            tl_sdu: BitBuffer::from_bitstr("101010"),
            pdu_prio: 5,
            stealing_permission: true,
            subscriber_class: 0,
            fcs_flag: false,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_class_info: None,
            req_handle: 1,
            graceful_degradation: None,
            chan_alloc: None,
            tx_reporter: None,
        };

        assert_eq!(
            Llc::expected_ack_timeslot_for_outbound_al(&prim),
            2,
            "WAP/SNDCP original AL can use endpoint 0 with link 1; without an explicit allocation, peer AL-ACK timing must follow PDCH TS2, not MCCH TS1"
        );
    }

    #[test]
    fn outbound_backlog_limits_are_sized_for_thousands_of_terminals() {
        assert!(LLC_MAX_OUTBOUND_ACKED_MESSAGES >= 4096);
        assert!(LLC_MAX_OUTBOUND_UDATA_MESSAGES >= 4096);
    }

    #[test]
    fn udata_backlog_limit_evicts_lower_priority_unsubmitted_with_failed_report() {
        let low_reporter = TxReporter::new_unacked();
        let mut messages = VecDeque::from([
            queued_udata_for_test(10, 1, false, Some(low_reporter.clone())),
            queued_udata_for_test(11, 6, false, None),
        ]);
        let mut queue = MessageQueue::new();

        assert!(
            Llc::ensure_udata_backlog_capacity(&mut queue, &mut messages, 7, 2),
            "incoming highest-priority BL-UDATA may displace a lower-priority unsubmitted backlog entry"
        );

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].req_handle, 11);
        assert_eq!(low_reporter.get_state(), TxState::Discarded);
        assert_eq!(pop_failed_report_req_handle(&mut queue), Some(10));
        assert!(
            queue.pop_front().is_none(),
            "one evicted TL-UNITDATA must produce exactly one failed-transfer report"
        );
    }

    #[test]
    fn udata_backlog_limit_preserves_submitted_and_equal_priority_entries() {
        let mut messages = VecDeque::from([queued_udata_for_test(20, 1, true, None), queued_udata_for_test(21, 6, false, None)]);
        let mut queue = MessageQueue::new();

        assert!(
            !Llc::ensure_udata_backlog_capacity(&mut queue, &mut messages, 6, 2),
            "LLC must not evict submitted MAC work or equal-priority FIFO backlog entries"
        );

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].req_handle, 20);
        assert_eq!(messages[1].req_handle, 21);
        assert!(queue.pop_front().is_none());
    }

    #[test]
    fn t251_age_counts_downlink_signalling_frames_not_timeslots() {
        let start = TdmaTime { t: 1, f: 1, m: 1, h: 0 };

        assert_eq!(Llc::downlink_signalling_frames_elapsed(start, start.add_timeslots(3), 1), 0);
        assert_eq!(Llc::downlink_signalling_frames_elapsed(start, start.add_timeslots(4), 1), 1);
        assert_eq!(Llc::downlink_signalling_frames_elapsed(start, start.add_timeslots(15), 1), 3);
        assert_eq!(Llc::downlink_signalling_frames_elapsed(start, start.add_timeslots(16), 1), 4);
        assert_eq!(
            Llc::downlink_signalling_frames_elapsed(start, start.add_timeslots(16), 1),
            T251_SENDER_RETRY_SIGNALLING_FRAMES
        );
    }

    #[test]
    fn t251_age_uses_the_target_downlink_timeslot() {
        let start = TdmaTime { t: 2, f: 1, m: 1, h: 0 };

        assert_eq!(Llc::downlink_signalling_frames_elapsed(start, start.add_timeslots(1), 2), 0);
        assert_eq!(Llc::downlink_signalling_frames_elapsed(start, start.add_timeslots(4), 2), 1);
        assert_eq!(Llc::downlink_signalling_frames_elapsed(start, start.add_timeslots(8), 2), 2);
    }

    #[test]
    fn t251_age_uses_all_downlink_frames_when_stealing_repeats_is_set() {
        let start = TdmaTime { t: 3, f: 1, m: 1, h: 0 };
        let now = start.add_timeslots(14);

        assert_eq!(Llc::downlink_signalling_frames_elapsed(start, now, 3), 3);
        assert_eq!(
            Llc::t251_downlink_frames_elapsed(start, now, 3, Some(true)),
            T251_SENDER_RETRY_SIGNALLING_FRAMES,
            "EN 300 392-2 Annex A.1 counts all downlink frames for T.251 when stealing repeats is set"
        );
        assert_eq!(Llc::t251_downlink_frames_elapsed(start, now, 3, Some(false)), 3);
        assert_eq!(Llc::t251_downlink_frames_elapsed(start, now, 3, None), 3);
    }

    #[test]
    fn inbound_duplicate_guard_expires_after_full_retransmission_horizon() {
        let received_at = TdmaTime { t: 2, f: 1, m: 1, h: 0 };
        let state = ReceiveSeqState {
            last_ns: 1,
            received_at,
            ack_timeslot: 2,
        };

        let boundary = received_at.add_timeslots((INBOUND_DUPLICATE_SUPPRESSION_SIGNALLING_FRAMES * TDMA_TIMESLOTS_PER_FRAME) as i32);
        assert!(
            !Llc::inbound_duplicate_state_expired(state, boundary),
            "same N(S) at the full N.252/T.251 retry horizon is still treated as a possible retransmission"
        );
        assert!(
            Llc::inbound_duplicate_state_expired(state, boundary.add_timeslots(TDMA_TIMESLOTS_PER_FRAME as i32)),
            "same N(S) after the full retry horizon may be delivered as a new transfer"
        );
    }

    #[test]
    fn scheduled_ack_for_same_basic_link_replaces_prior_pending_ack() {
        let mut acks: VecDeque<ScheduledOutAck> = VecDeque::new();
        let addr = TetraAddress::new(1001, SsiType::Issi);
        let first = TdmaTime { t: 2, f: 1, m: 1, h: 0 };
        let second = first.add_timeslots(4);

        Llc::upsert_scheduled_out_ack(&mut acks, first, addr, 0, 0, -1, 0);
        Llc::upsert_scheduled_out_ack(&mut acks, second, addr, 0, 1, -2, 0);

        assert_eq!(acks.len(), 1);
        assert_eq!(acks[0].addr.ssi, addr.ssi);
        assert_eq!(acks[0].addr.ssi_type, SsiType::Issi);
        assert_eq!(acks[0].endpoint_id, 0);
        assert_eq!(acks[0].ts, 2);
        assert_eq!(acks[0].nr, 1);
        assert_eq!(acks[0].ind_req_handle, -2);
        assert_eq!(acks[0].t_start, second);
    }

    #[test]
    fn scheduled_ack_for_same_endpoint_replaces_across_receive_timeslots() {
        let mut acks: VecDeque<ScheduledOutAck> = VecDeque::new();
        let addr = TetraAddress::new(1001, SsiType::Issi);
        let ts2 = TdmaTime { t: 2, f: 1, m: 1, h: 0 };
        let ts3 = TdmaTime { t: 3, f: 1, m: 1, h: 0 };

        Llc::upsert_scheduled_out_ack(&mut acks, ts2, addr, 0, 0, -1, 0);
        Llc::upsert_scheduled_out_ack(&mut acks, ts3, addr, 0, 1, -2, 0);

        assert_eq!(acks.len(), 1);
        assert_eq!(acks[0].ts, 3);
        assert_eq!(acks[0].nr, 1);
        assert_eq!(acks[0].ind_req_handle, -2);
    }

    #[test]
    fn scheduled_ack_keeps_different_endpoints_separate() {
        let mut acks: VecDeque<ScheduledOutAck> = VecDeque::new();
        let addr = TetraAddress::new(1001, SsiType::Issi);
        let ts = TdmaTime { t: 2, f: 1, m: 1, h: 0 };

        Llc::upsert_scheduled_out_ack(&mut acks, ts, addr, 1, 0, -1, 0);
        Llc::upsert_scheduled_out_ack(&mut acks, ts, addr, 2, 1, -2, 0);

        assert_eq!(acks.len(), 2);
        assert_eq!(acks[0].endpoint_id, 1);
        assert_eq!(acks[0].nr, 0);
        assert_eq!(acks[0].ind_req_handle, -1);
        assert_eq!(acks[1].endpoint_id, 2);
        assert_eq!(acks[1].nr, 1);
        assert_eq!(acks[1].ind_req_handle, -2);
    }
}
