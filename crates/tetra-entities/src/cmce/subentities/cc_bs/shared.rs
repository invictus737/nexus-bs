use super::*;

const TETRA_TIMESLOTS_PER_SECOND: i32 = 18 * 4;

// Local bounded cleanup guard; ETSI EN 300 392-2 clauses 14.5.2.3.2/14.5.2.3.3
// define group-call release with D-RELEASE, while clauses 23.5.2.2.7/23.7.6
// require downlink signalling to account for energy-economy receive windows.
// EG7 sleeps for 359 frames, so one full receive cycle is 360 TDMA frames.
const GROUP_RELEASE_PENDING_TIMEOUT_TIMESLOTS: i32 = 360 * 4;
// EN 300 392-2 clauses 14.5.1.3.2/14.5.1.3.3 require D-RELEASE/D-DISCONNECT
// before the MS clears an established individual call. Keep assigned-channel
// FACCH/STCH alive long enough for all repeated BL-UDATA fragments to finish;
// this is a local safety guard, not a replacement for the CMCE clear procedure.
const INDIVIDUAL_RELEASE_PENDING_TIMEOUT_TIMESLOTS: i32 = 2 * TETRA_TIMESLOTS_PER_SECOND;
const INDIVIDUAL_DISCONNECT_DELIVERY_TIMEOUT_TIMESLOTS: i32 = 2 * TETRA_TIMESLOTS_PER_SECOND;
// EN 300 392-2 clause 23.8.5 BS operation mandates this ordering for N=4/8
// circuit-mode data. CMCE does not yet expose bearer-specific interleaving
// depth here, so simplex speech uses the same short N=4-equivalent guard as a
// conservative bearer-tail compatibility drain before peer-facing clear state.
const INDIVIDUAL_SIMPLEX_TAIL_DRAIN_TIMESLOTS: i32 = (4 - 1) * 4;
// Group speech uses the same TCH/S interleaver tail ordering. Delay the
// no-speaker D-TX CEASED/FloorReleased transition long enough for UMAC to
// flush a deferred NTS2 Block2 speech half-slot before entering hangtime.
const GROUP_TX_CEASED_TAIL_DRAIN_TIMESLOTS: i32 = (4 - 1) * 4;
// Brew-origin group calls must not feed speech into UMAC until the RF control
// edge (D-SETUP or D-TX GRANTED) is known to have left the downlink scheduler.
// This bounded guard is a BS/interconnect robustness policy; the air-interface
// baseline remains EN 300 392-2 clauses 14.5.2.1 and 14.5.2.2.1.
const NETWORK_GROUP_READY_PENDING_TIMEOUT_TIMESLOTS: i32 = 2 * TETRA_TIMESLOTS_PER_SECOND;
// EN 300 392-2 table 20.54 defines priority 7 as the highest TMA priority.
// Keep this restricted to time-critical floor-control signalling.
const CMCE_FLOOR_CONTROL_PDU_PRIO: i32 = 7;
const MAX_LOCAL_LISTENER_INDIVIDUAL_FLOOR_GRANTS: usize = 100;
// EN 300 392-2 clause 14.5.1.2.2 f references EN 300 392-9 notification
// value 26 as "Notice of imminent call disconnection".
const NOTIFICATION_IMMINENT_CALL_DISCONNECTION: u64 = 26;

// EN 300 392-2 table 14.33 uses pointer 0 for an unsupported whole PDU.
// Non-zero pointers below are bit offsets into the received-PDU extract, which
// excludes the 5-bit CMCE PDU type as required by table 14.33 note 6.
const CMCE_FNS_PTR_U_CONNECT_BASIC_SERVICE_INFORMATION: u8 = 17;
const CMCE_FNS_PTR_U_CONNECT_TYPE3: u8 = 18;
const CMCE_FNS_PTR_U_ALERT_RESERVED: u8 = 14;
const CMCE_FNS_PTR_U_ALERT_BASIC_SERVICE_INFORMATION: u8 = 17;
const CMCE_FNS_PTR_U_ALERT_TYPE3: u8 = 18;
const CMCE_FNS_PTR_U_DISCONNECT_TYPE3: u8 = 20;
const CMCE_FNS_PTR_U_INFO_MODIFY: u8 = 16;
const CMCE_FNS_PTR_U_INFO_TYPE3: u8 = 17;
const CMCE_FNS_PTR_U_RELEASE_TYPE3: u8 = 20;
const CMCE_FNS_PTR_U_TX_DEMAND_ENCRYPTION_CONTROL: u8 = 16;
const CMCE_FNS_PTR_U_TX_DEMAND_RESERVED: u8 = 17;
const CMCE_FNS_PTR_U_TX_DEMAND_TYPE3: u8 = 19;

impl CcBsSubentity {
    pub fn new(config: SharedConfig) -> Self {
        CcBsSubentity {
            config,
            dltime: TdmaTime::default(),
            cached_setups: HashMap::new(),
            circuits: CircuitMgr::new(),
            active_calls: HashMap::new(),
            pending_group_releases: HashMap::new(),
            individual_calls: HashMap::new(),
            pending_individual_disconnect_tail_drains: HashMap::new(),
            pending_individual_disconnect_deliveries: HashMap::new(),
            pending_individual_disconnect_release_acks: HashMap::new(),
            pending_individual_tx_ceased_tail_drains: HashMap::new(),
            pending_group_tx_ceased_tail_drains: HashMap::new(),
            pending_group_floor_activations: HashMap::new(),
            pending_network_group_readies: HashMap::new(),
            pending_individual_connect_acks: HashMap::new(),
            pending_network_individual_connects: HashMap::new(),
            pending_individual_releases: HashMap::new(),
            subscriber_groups: HashMap::new(),
            group_listeners: HashMap::new(),
            external_subscriber_groups: HashMap::new(),
            external_group_listeners: HashMap::new(),
            telemetry: None,
            echo_session: None,
            parrot_session: None,
        }
    }

    pub fn set_telemetry(&mut self, sink: crate::net_telemetry::channel::TelemetrySink) {
        self.telemetry = Some(sink);
    }

    pub fn health_stats(&self) -> crate::health::CmceHealthStats {
        let active_individual_calls = self.individual_calls.values().filter(|call| call.is_active()).count();
        let pending_individual_calls = self.individual_calls.len().saturating_sub(active_individual_calls);
        let pending_individual_disconnects = self.pending_individual_disconnect_tail_drains.len()
            + self.pending_individual_disconnect_deliveries.len()
            + self.pending_individual_disconnect_release_acks.len()
            + self.pending_individual_tx_ceased_tail_drains.len();
        let group_floor_waiters = self.active_calls.values().map(|call| call.queued_tx_demands().count()).sum();
        let individual_floor_waiters = self
            .individual_calls
            .values()
            .filter(|call| call.queued_tx_demand.is_some())
            .count();

        crate::health::CmceHealthStats {
            active_group_calls: self.active_calls.len(),
            pending_group_releases: self.pending_group_releases.len(),
            pending_group_floor_activations: self.pending_group_floor_activations.len(),
            pending_network_group_readies: self.pending_network_group_readies.len(),
            group_floor_waiters,
            active_individual_calls,
            pending_individual_calls,
            pending_individual_releases: self.pending_individual_releases.len(),
            pending_network_individual_connects: self.pending_network_individual_connects.len(),
            pending_individual_disconnects,
            individual_floor_waiters,
        }
    }

    /// Called when an UL voice frame arrives on TmdSap.
    /// If an echo session owns this timeslot, loopback the frame as DL.
    pub fn handle_echo_ul_frame(&mut self, queue: &mut MessageQueue, ts: u8, data: Vec<u8>) {
        let Some(session) = self.echo_session.as_mut() else { return };
        if session.ts != ts {
            return;
        }
        if let Some(echo_data) = session.push_ul_frame(data) {
            queue.push_back(crate::cmce::subentities::cc_bs::echo::EchoSession::make_dl_msg(ts, echo_data));
        }
    }

    pub fn handle_parrot_ul_frame(
        &mut self,
        _queue: &mut MessageQueue,
        ts: u8,
        data: Vec<u8>,
        raw_tch_s_block: Option<tetra_core::PhyBlockNum>,
    ) -> bool {
        let Some(session) = self.parrot_session.as_mut() else {
            return false;
        };
        if !session.owns_ts(ts) {
            return false;
        }
        if session.record_ul_frame(ts, data, raw_tch_s_block) {
            tracing::trace!(
                "CMCE: parrot service recorded frame call_id={} ts={} frames={}",
                session.call_id,
                ts,
                session.recorded_len()
            );
        } else {
            tracing::trace!(
                "CMCE: parrot service consumed late/non-recording UL frame call_id={} ts={}",
                session.call_id,
                ts
            );
        }
        true
    }

    pub(super) fn drive_parrot_session(&mut self, queue: &mut MessageQueue) {
        let Some(session) = self.parrot_session.as_mut() else {
            return;
        };
        if let Some(msg) = session.next_playback_msg(self.dltime) {
            queue.push_back(msg);
        }
        if session.take_playback_finished() {
            let call_id = session.call_id;
            let caller_issi = session.caller_issi();
            tracing::info!("CMCE: parrot playback complete, releasing call_id={}", call_id);
            self.release_individual_call_to_issi(queue, call_id, DisconnectCause::SwmiRequestedDisconnection, caller_issi);
        }
    }

    /// Release echo session if it owns `call_id`.
    pub fn release_echo_session_if_matches(&mut self, call_id: u16) {
        if let Some(ref s) = self.echo_session {
            if s.call_id == call_id {
                tracing::info!("CMCE: echo service session released (call_id={})", call_id);
                self.echo_session = None;
            }
        }
        if let Some(ref s) = self.parrot_session {
            if s.call_id == call_id {
                tracing::info!("CMCE: parrot service session released (call_id={})", call_id);
                self.parrot_session = None;
            }
        }
    }

    pub(super) fn emit(&self, event: crate::net_telemetry::TelemetryEvent) {
        if let Some(sink) = &self.telemetry {
            sink.send(event);
        }
    }

    pub fn set_config(&mut self, config: SharedConfig) {
        self.config = config;
    }

    pub(in crate::cmce::subentities::cc_bs) fn cmce_downlink_pdu_prio(sdu: &BitBuffer) -> i32 {
        let mut probe = BitBuffer::from_bitbuffer(sdu);
        let pdu_type = probe
            .read_field(5, "cmce_pdu_type_dl")
            .ok()
            .and_then(|bits| CmcePduTypeDl::try_from(bits).ok());

        match pdu_type {
            Some(CmcePduTypeDl::DTxCeased | CmcePduTypeDl::DTxInterrupt) => CMCE_FLOOR_CONTROL_PDU_PRIO,
            Some(CmcePduTypeDl::DTxGranted) => {
                let mut grant_probe = BitBuffer::from_bitbuffer(sdu);
                if DTxGranted::from_bitbuf(&mut grant_probe).is_ok_and(|grant| {
                    matches!(
                        TransmissionGrant::try_from(grant.transmission_grant as u64),
                        Ok(TransmissionGrant::Granted | TransmissionGrant::GrantedToOtherUser)
                    )
                }) {
                    CMCE_FLOOR_CONTROL_PDU_PRIO
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    pub(in crate::cmce) fn debug_force_next_call_identifier(&mut self, next_call_identifier: u16) {
        self.circuits.next_call_identifier = next_call_identifier;
    }

    pub(in crate::cmce) fn debug_active_call_ids(&self) -> Vec<u16> {
        let mut ids: Vec<_> = self.occupied_call_ids().into_iter().collect();
        ids.sort_unstable();
        ids
    }

    pub(super) fn occupied_call_ids(&self) -> HashSet<u16> {
        let mut ids = HashSet::with_capacity(
            self.cached_setups.len()
                + self.active_calls.len()
                + self.pending_group_releases.len()
                + self.individual_calls.len()
                + self.pending_individual_disconnect_tail_drains.len()
                + self.pending_individual_disconnect_deliveries.len()
                + self.pending_individual_disconnect_release_acks.len()
                + self.pending_individual_tx_ceased_tail_drains.len()
                + self.pending_group_tx_ceased_tail_drains.len()
                + self.pending_group_floor_activations.len()
                + self.pending_network_group_readies.len()
                + self.pending_individual_connect_acks.len()
                + self.pending_individual_releases.len()
                + 4,
        );

        ids.extend(self.cached_setups.keys().copied());
        ids.extend(self.active_calls.keys().copied());
        ids.extend(self.pending_group_releases.keys().copied());
        ids.extend(self.individual_calls.keys().copied());
        ids.extend(self.pending_individual_disconnect_tail_drains.keys().copied());
        ids.extend(self.pending_individual_disconnect_deliveries.keys().copied());
        ids.extend(self.pending_individual_disconnect_release_acks.keys().copied());
        ids.extend(self.pending_individual_tx_ceased_tail_drains.keys().copied());
        ids.extend(self.pending_group_tx_ceased_tail_drains.keys().copied());
        ids.extend(self.pending_group_floor_activations.keys().copied());
        ids.extend(self.pending_network_group_readies.keys().copied());
        ids.extend(self.pending_individual_connect_acks.keys().copied());
        ids.extend(self.pending_individual_releases.keys().copied());
        ids.extend(self.circuits.active_call_ids());
        if let Some(session) = &self.echo_session {
            ids.insert(session.call_id);
        }
        if let Some(session) = &self.parrot_session {
            ids.insert(session.call_id);
        }

        ids.remove(&0);
        ids
    }

    pub(super) fn build_d_setup_prim(pdu: &DSetup, usage: u8, ts: u8, ul_dl: UlDlAssignment) -> (BitBuffer, CmceChanAllocReq) {
        tracing::debug!("-> {:?}", pdu);

        let mut sdu = BitBuffer::new_autoexpand(80);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DSetup");
        sdu.seek(0);

        let mut timeslots = [false; 4];
        timeslots[ts as usize - 1] = true;
        let chan_alloc = CmceChanAllocReq {
            usage: Some(usage),
            alloc_type: ChanAllocType::Replace,
            carrier: None,
            timeslots,
            ul_dl_assigned: ul_dl,
        };
        (sdu, chan_alloc)
    }

    pub(super) fn refresh_group_cached_d_setup_speaker(&mut self, call_id: u16, speaker_issi: u32) {
        let Some(cached) = self.cached_setups.get_mut(&call_id) else {
            tracing::warn!(
                "CMCE: cannot refresh group D-SETUP speaker for call_id={} (missing cached setup)",
                call_id
            );
            return;
        };
        if cached.is_individual {
            tracing::warn!("CMCE: refusing to refresh group D-SETUP speaker for individual call_id={}", call_id);
            return;
        }

        // EN 300 392-2 clauses 14.5.2.1.1/14.5.2.1.2 make D-SETUP carry
        // the call's current calling/transmitting party for group-call setup
        // and late entry. Clause 14.5.2.2.1 then moves the active floor with
        // D-TX GRANTED; keep the cached back-up D-SETUP coherent with that
        // floor so late-entry resends do not advertise a stale speaker.
        cached.pdu.calling_party_address_ssi = Some(speaker_issi);
        cached.pdu.transmission_grant = TransmissionGrant::GrantedToOtherUser;
        cached.pdu.transmission_request_permission = false;
        cached.last_resend_reporter = None;
    }

    /// Build a generic SAP message addressed to MLE via LCMC.
    /// `layer2service` controls acknowledged vs unacknowledged LLC.
    pub(super) fn build_sapmsg(
        sdu: BitBuffer,
        chan_alloc: Option<CmceChanAllocReq>,
        address: TetraAddress,
        layer2service: Layer2Service,
        reporter: Option<TxReporter>,
    ) -> SapMsg {
        let pdu_prio = Self::cmce_downlink_pdu_prio(&sdu);
        SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: 0,
                endpoint_id: 0,
                link_id: 0,
                layer2service,
                pdu_prio,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                unacked_bl_repetitions: None,
                chan_alloc,
                main_address: address,
                tx_reporter: reporter,
            }),
        }
    }

    /// Build a SAP message with explicit LLC link context (handle/link_id/endpoint_id).
    /// Used for individually-addressed responses that must be routed back through
    /// the established LLC link of a specific MS.
    pub(super) fn build_sapmsg_direct(sdu: BitBuffer, address: TetraAddress, handle: u32, link_id: u32, endpoint_id: u32) -> SapMsg {
        let pdu_prio = Self::cmce_downlink_pdu_prio(&sdu);
        SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle,
                endpoint_id,
                link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                unacked_bl_repetitions: None,
                chan_alloc: None,
                main_address: address,
                tx_reporter: None,
            }),
        }
    }

    /// EN 300 392-2 clause 14.7.3.2: CMCE FUNCTION NOT SUPPORTED may be sent
    /// as a response to an individually addressed CMCE PDU. Pointer 0 means the
    /// whole PDU type is unsupported, not a specific information element.
    pub(crate) fn build_cmce_function_not_supported_direct(
        unsupported_pdu_type: CmcePduTypeUl,
        target: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
    ) -> SapMsg {
        let pdu = CmceFunctionNotSupported {
            not_supported_pdu_type: unsupported_pdu_type.into_raw() as u8,
            call_identifier_present: false,
            call_identifier: None,
            function_not_supported_pointer: 0,
            length_of_received_pdu_extract: None,
            received_pdu_extract: None,
        };
        let mut sdu = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut sdu)
            .expect("serialize CMCE FUNCTION NOT SUPPORTED for unsupported PDU type");
        sdu.seek(0);
        Self::build_sapmsg_direct(sdu, target, handle, link_id, endpoint_id)
    }

    /// EN 300 392-2 §14.7.3.1/§14.7.3.2 and table 14.33: for an
    /// individually addressed CMCE PDU with an unsupported element/value, send
    /// CMCE FUNCTION NOT SUPPORTED with a non-zero pointer and an extract of
    /// the received PDU contents excluding the PDU type.
    pub(crate) fn build_cmce_function_not_supported_element_direct(
        unsupported_pdu_type: CmcePduTypeUl,
        call_identifier: u16,
        function_not_supported_pointer: u8,
        received_pdu: &BitBuffer,
        target: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
    ) -> SapMsg {
        debug_assert!(
            function_not_supported_pointer != 0,
            "pointer 0 is reserved for whole-PDU unsupported"
        );

        let mut received = received_pdu.clone();
        received.seek(5);
        let extract_len = received.get_len_remaining();
        let mut received_pdu_extract = BitBuffer::new_autoexpand(extract_len);
        received_pdu_extract.copy_bits(&mut received, extract_len);
        received_pdu_extract.seek(0);

        let pdu = CmceFunctionNotSupported {
            not_supported_pdu_type: unsupported_pdu_type.into_raw() as u8,
            call_identifier_present: true,
            call_identifier: Some(call_identifier as u64),
            function_not_supported_pointer,
            length_of_received_pdu_extract: Some(extract_len as u64),
            received_pdu_extract: Some(received_pdu_extract),
        };
        let mut sdu = BitBuffer::new_autoexpand(64 + extract_len);
        pdu.to_bitbuf(&mut sdu)
            .expect("serialize CMCE FUNCTION NOT SUPPORTED for unsupported element");
        sdu.seek(0);
        Self::build_sapmsg_direct(sdu, target, handle, link_id, endpoint_id)
    }

    /// Build a SAP message using FACCH stealing on a traffic channel timeslot.
    /// ETSI EN 300 392-2 §21: FACCH stealing allows signalling PDUs to be
    /// transmitted in place of voice on an active TCH.
    pub(super) fn build_sapmsg_stealing(sdu: BitBuffer, address: TetraAddress, ts: u8, usage: Option<u8>) -> SapMsg {
        Self::build_sapmsg_stealing_ul_dl(sdu, address, ts, usage, UlDlAssignment::Both)
    }

    /// Like `build_sapmsg_stealing` but with an explicit UL/DL assignment.
    /// Used for simplex PTT floor changes: the new speaker gets `Ul`, the listener gets `Dl`.
    pub(super) fn build_sapmsg_stealing_ul_dl(
        sdu: BitBuffer,
        address: TetraAddress,
        ts: u8,
        usage: Option<u8>,
        ul_dl_assigned: UlDlAssignment,
    ) -> SapMsg {
        Self::build_sapmsg_stealing_ul_dl_reported_with_repetitions(sdu, address, ts, usage, ul_dl_assigned, None, None)
    }

    pub(super) fn build_sapmsg_stealing_ul_dl_with_repetitions(
        sdu: BitBuffer,
        address: TetraAddress,
        ts: u8,
        usage: Option<u8>,
        ul_dl_assigned: UlDlAssignment,
        unacked_bl_repetitions: Option<u8>,
    ) -> SapMsg {
        Self::build_sapmsg_stealing_ul_dl_reported_with_repetitions(sdu, address, ts, usage, ul_dl_assigned, None, unacked_bl_repetitions)
    }

    pub(super) fn build_sapmsg_stealing_ul_dl_reported(
        sdu: BitBuffer,
        address: TetraAddress,
        ts: u8,
        usage: Option<u8>,
        ul_dl_assigned: UlDlAssignment,
        reporter: Option<TxReporter>,
    ) -> SapMsg {
        Self::build_sapmsg_stealing_ul_dl_reported_with_repetitions(sdu, address, ts, usage, ul_dl_assigned, reporter, None)
    }

    pub(super) fn build_sapmsg_stealing_ul_dl_reported_with_repetitions(
        sdu: BitBuffer,
        address: TetraAddress,
        ts: u8,
        usage: Option<u8>,
        ul_dl_assigned: UlDlAssignment,
        reporter: Option<TxReporter>,
        unacked_bl_repetitions: Option<u8>,
    ) -> SapMsg {
        let mut timeslots = [false; 4];
        timeslots[(ts - 1) as usize] = true;
        let chan_alloc = CmceChanAllocReq {
            usage,
            carrier: None,
            timeslots,
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned,
        };
        let pdu_prio = Self::cmce_downlink_pdu_prio(&sdu);

        SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: 0,
                endpoint_id: 0,
                link_id: 0,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio,
                layer2_qos: 0,
                stealing_permission: true,
                stealing_repeats_flag: false,
                unacked_bl_repetitions,
                chan_alloc: Some(chan_alloc),
                main_address: address,
                tx_reporter: reporter,
            }),
        }
    }

    pub(super) fn build_d_release(call_identifier: u16, disconnect_cause: DisconnectCause) -> BitBuffer {
        Self::build_d_release_with_notification(call_identifier, disconnect_cause, None)
    }

    pub(super) fn build_d_release_with_notification(
        call_identifier: u16,
        disconnect_cause: DisconnectCause,
        notification_indicator: Option<u64>,
    ) -> BitBuffer {
        let pdu = DRelease {
            call_identifier,
            disconnect_cause,
            notification_indicator,
            facility: None,
            proprietary: None,
        };
        tracing::info!("-> {:?}", pdu);

        let mut sdu = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DRelease");
        sdu.seek(0);
        sdu
    }

    pub(super) fn build_d_release_from_d_setup(d_setup_pdu: &DSetup, disconnect_cause: DisconnectCause) -> BitBuffer {
        Self::build_d_release(d_setup_pdu.call_identifier, disconnect_cause)
    }

    fn build_d_info_imminent_call_disconnection(call_id: u16) -> BitBuffer {
        let pdu = DInfo {
            call_identifier: call_id,
            reset_call_time_out_timer_t310_: false,
            poll_request: false,
            new_call_identifier: None,
            call_time_out: None,
            call_time_out_set_up_phase_t301_t302_: None,
            call_ownership: None,
            modify: None,
            call_status: None,
            temporary_address: None,
            notification_indicator: Some(NOTIFICATION_IMMINENT_CALL_DISCONNECTION),
            poll_response_percentage: None,
            poll_response_number: None,
            dtmf: None,
            facility: None,
            poll_response_addresses: None,
            proprietary: None,
        };
        tracing::info!("-> {:?} (imminent private-call disconnection)", pdu);

        let mut sdu = BitBuffer::new_autoexpand(48);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DInfo");
        sdu.seek(0);
        sdu
    }

    fn push_established_individual_release_imminent_notice(
        queue: &mut MessageQueue,
        call_id: u16,
        address: TetraAddress,
        ts: u8,
        usage: u8,
    ) {
        // EN 300 392-2 14.5.1.2.2 f: before actual call disconnection the
        // SwMI may send D-INFO with "Notice of imminent call disconnection".
        // Keep this on the assigned FACCH/STCH and ahead of D-RELEASE.
        queue.push_back(Self::build_sapmsg_stealing_ul_dl_with_repetitions(
            Self::build_d_info_imminent_call_disconnection(call_id),
            address,
            ts,
            Some(usage),
            UlDlAssignment::Dl,
            Some(0),
        ));
    }

    /// EN 300 392-2 clauses 14.5.1.3.2 and 14.5.2.3.2 require a D-RELEASE
    /// when the SwMI cannot support a requested call. If the response is sent
    /// before a SwMI call identity is allocated, clause 14.5.1.1.2 points to
    /// the dummy call reference; clause 3.1 defines that value as zero.
    pub(super) fn reject_u_setup_before_call_id(
        queue: &mut MessageQueue,
        calling_party: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        disconnect_cause: DisconnectCause,
    ) {
        let sdu = Self::build_d_release(0, disconnect_cause);
        let msg = Self::build_sapmsg_direct(sdu, calling_party, handle, link_id, endpoint_id);
        queue.push_back(msg);
    }

    pub(super) fn has_listener(&self, gssi: u32) -> bool {
        if self.config.state_read().subscribers.has_group_members(gssi) {
            return true;
        }
        self.group_listeners.get(&gssi).copied().unwrap_or(0) > 0 || self.external_group_listeners.get(&gssi).copied().unwrap_or(0) > 0
    }

    pub(super) fn has_local_listener(&self, gssi: u32) -> bool {
        if self.config.state_read().subscribers.has_group_members(gssi) {
            return true;
        }
        self.group_listeners.get(&gssi).copied().unwrap_or(0) > 0
    }

    pub(super) fn subscriber_affiliated_to_group(&self, issi: u32, gssi: u32) -> bool {
        if self.config.state_read().subscribers.contains_group_member(gssi, issi) {
            return true;
        }
        self.subscriber_groups
            .get(&issi)
            .map(|groups| groups.contains(&gssi))
            .unwrap_or(false)
    }

    pub(super) fn first_affiliated_group_floor_requester(&self, call_id: u16, call: &ActiveCall, context: &str) -> Option<TetraAddress> {
        call.queued_tx_demands().find(|requester| {
            let affiliated = self.subscriber_affiliated_to_group(requester.ssi, call.dest_gssi);
            if !affiliated {
                tracing::info!(
                    "CMCE: dropping queued group floor requester ISSI {} for call_id={} gssi={} after affiliation loss during {}",
                    requester.ssi,
                    call_id,
                    call.dest_gssi,
                    context
                );
            }
            affiliated
        })
    }

    fn sync_shared_subscribers_from_mm_update(&self, issi: u32, groups: &[u32], action: BrewSubscriberAction, source: TetraEntity) {
        if source != TetraEntity::Mm {
            tracing::debug!(
                "CMCE: keeping {:?} subscriber update issi={} action={:?} groups={:?} out of shared RF subscriber registry",
                source,
                issi,
                action,
                groups
            );
            return;
        }

        let mut state = self.config.state_write();
        match action {
            BrewSubscriberAction::Register => {
                if !state.subscribers.is_registered(issi) {
                    state.subscribers.register(issi);
                }
            }
            BrewSubscriberAction::Deregister => {
                state.subscribers.deregister(issi);
            }
            BrewSubscriberAction::Affiliate => {
                if !state.subscribers.is_registered(issi) {
                    tracing::warn!(
                        "CMCE: not syncing affiliate for unknown ISSI {} into shared subscriber registry",
                        issi
                    );
                    return;
                }
                for &gssi in groups {
                    state.subscribers.affiliate(issi, gssi);
                }
            }
            BrewSubscriberAction::Deaffiliate => {
                for &gssi in groups {
                    state.subscribers.deaffiliate(issi, gssi);
                }
            }
            BrewSubscriberAction::ReleaseIndividualCalls => {}
        }
    }

    pub(super) fn clear_group_floor_state_for_departure(&mut self, queue: &mut MessageQueue, issi: u32, gssi: u32) {
        let call_ids: Vec<u16> = self
            .active_calls
            .iter()
            .filter(|(_, call)| call.dest_gssi == gssi)
            .map(|(call_id, _)| *call_id)
            .collect();

        for call_id in call_ids {
            let current_speaker_departed = self
                .active_calls
                .get(&call_id)
                .is_some_and(|call| call.tx_active && call.source_issi == issi);

            if current_speaker_departed && self.has_listener(gssi) {
                if let Err(err) = self.fsm_group_on_current_speaker_departed(queue, call_id, issi) {
                    tracing::warn!(
                        "CMCE: failed to release departed group speaker call_id={} issi={} gssi={} err={:?}",
                        call_id,
                        issi,
                        gssi,
                        err
                    );
                }
                continue;
            }

            if let Some(call) = self.active_calls.get_mut(&call_id)
                && call.clear_queued_tx_demand_from(issi)
            {
                tracing::info!(
                    "CMCE: cleared queued group floor request call_id={} issi={} gssi={} after subscriber departure",
                    call_id,
                    issi,
                    gssi
                );
            }
        }
    }

    pub(super) fn inc_group_listener(&mut self, gssi: u32) {
        let entry = self.group_listeners.entry(gssi).or_insert(0);
        *entry += 1;
    }

    pub(super) fn dec_group_listener(&mut self, gssi: u32) {
        if let Some(entry) = self.group_listeners.get_mut(&gssi) {
            if *entry <= 1 {
                self.group_listeners.remove(&gssi);
            } else {
                *entry -= 1;
            }
        }
    }

    pub(super) fn inc_external_group_listener(&mut self, gssi: u32) {
        let entry = self.external_group_listeners.entry(gssi).or_insert(0);
        *entry += 1;
    }

    pub(super) fn dec_external_group_listener(&mut self, gssi: u32) {
        if let Some(entry) = self.external_group_listeners.get_mut(&gssi) {
            if *entry <= 1 {
                self.external_group_listeners.remove(&gssi);
            } else {
                *entry -= 1;
            }
        }
    }

    pub(super) fn find_individual_call_by_issi(&self, issi: u32) -> Option<(u16, IndividualCallState)> {
        self.individual_calls
            .iter()
            .find(|(_, call)| call.calling_addr.ssi == issi || call.called_addr.ssi == issi)
            .map(|(call_id, call)| (*call_id, call.state))
            .or_else(|| {
                self.pending_individual_releases
                    .iter()
                    .find(|(_, pending)| pending.call.calling_addr.ssi == issi || pending.call.called_addr.ssi == issi)
                    .map(|(call_id, pending)| (*call_id, pending.call.state))
            })
    }

    pub(super) fn find_brew_individual_call(&self, brew_uuid: uuid::Uuid) -> Option<(u16, IndividualCall)> {
        self.individual_calls
            .iter()
            .find(|(_, call)| call.brew_uuid == Some(brew_uuid))
            .map(|(call_id, call)| (*call_id, call.clone()))
    }

    pub(super) fn drop_group_calls_if_unlistened(&mut self, queue: &mut MessageQueue, gssi: u32) {
        if self.has_listener(gssi) {
            return;
        }

        let to_drop: Vec<u16> = self
            .active_calls
            .iter()
            .filter(|(_, call)| call.dest_gssi == gssi)
            .map(|(call_id, _)| *call_id)
            .collect();

        for call_id in to_drop {
            tracing::info!("CMCE: dropping call_id={} gssi={} (no listeners)", call_id, gssi);
            // ETSI EN 300 392-2 clauses 14.5.2.2.7 and 14.5.2.3.2: external-to-group
            // calls use group-call signalling, and SwMI sends D-RELEASE before
            // subsequently releasing the call. Brew is notified from pending release
            // completion so the external call end is emitted once after D-RELEASE delivery.
            self.release_group_call(queue, call_id, DisconnectCause::SwmiRequestedDisconnection);
        }
    }

    fn handle_external_subscriber_update(&mut self, queue: &mut MessageQueue, update: MmSubscriberUpdate, source: TetraEntity) {
        let issi = update.issi;
        let groups = update.groups;

        match update.action {
            BrewSubscriberAction::Register => {
                let known = self.external_subscriber_groups.contains_key(&issi);
                self.external_subscriber_groups.entry(issi).or_insert_with(HashSet::new);
                tracing::info!(
                    "CMCE: external subscriber register source={:?} issi={} known={}",
                    source,
                    issi,
                    known
                );
            }
            BrewSubscriberAction::Deregister => {
                if let Some(existing) = self.external_subscriber_groups.remove(&issi) {
                    let existing_groups: Vec<u32> = existing.into_iter().collect();
                    for gssi in &existing_groups {
                        self.dec_external_group_listener(*gssi);
                    }
                    for gssi in &existing_groups {
                        self.clear_group_floor_state_for_departure(queue, issi, *gssi);
                    }
                    for gssi in &existing_groups {
                        self.drop_group_calls_if_unlistened(queue, *gssi);
                    }
                }
                tracing::info!("CMCE: external subscriber deregister source={:?} issi={}", source, issi);
            }
            BrewSubscriberAction::Affiliate => {
                let mut new_groups = Vec::new();
                {
                    let entry = self.external_subscriber_groups.entry(issi).or_insert_with(HashSet::new);
                    for gssi in groups {
                        if entry.insert(gssi) {
                            new_groups.push(gssi);
                        }
                    }
                }
                for gssi in &new_groups {
                    self.inc_external_group_listener(*gssi);
                }

                if new_groups.is_empty() {
                    tracing::debug!("CMCE: external affiliate ignored source={:?} issi={} (no new groups)", source, issi);
                } else {
                    tracing::info!(
                        "CMCE: external subscriber affiliate source={:?} issi={} groups={:?}",
                        source,
                        issi,
                        new_groups
                    );
                }
            }
            BrewSubscriberAction::Deaffiliate => {
                let mut removed_groups = Vec::new();
                if let Some(entry) = self.external_subscriber_groups.get_mut(&issi) {
                    for gssi in groups {
                        if entry.remove(&gssi) {
                            removed_groups.push(gssi);
                        }
                    }
                    if entry.is_empty() {
                        self.external_subscriber_groups.remove(&issi);
                    }
                }

                for gssi in &removed_groups {
                    self.dec_external_group_listener(*gssi);
                }

                if removed_groups.is_empty() {
                    tracing::debug!(
                        "CMCE: external deaffiliate ignored source={:?} issi={} (no matching groups)",
                        source,
                        issi
                    );
                } else {
                    tracing::info!(
                        "CMCE: external subscriber deaffiliate source={:?} issi={} groups={:?}",
                        source,
                        issi,
                        removed_groups
                    );
                    for gssi in &removed_groups {
                        self.clear_group_floor_state_for_departure(queue, issi, *gssi);
                    }
                    for gssi in &removed_groups {
                        self.drop_group_calls_if_unlistened(queue, *gssi);
                    }
                }
            }
            BrewSubscriberAction::ReleaseIndividualCalls => {
                tracing::debug!("CMCE: ignoring external ReleaseIndividualCalls source={:?} issi={}", source, issi);
            }
        }
    }

    pub fn handle_subscriber_update(&mut self, queue: &mut MessageQueue, update: MmSubscriberUpdate, source: TetraEntity) {
        let issi = update.issi;
        self.sync_shared_subscribers_from_mm_update(issi, &update.groups, update.action, source);
        if source != TetraEntity::Mm {
            self.handle_external_subscriber_update(queue, update, source);
            return;
        }

        let groups = update.groups;

        match update.action {
            BrewSubscriberAction::Register => {
                let known = self.subscriber_groups.contains_key(&issi);
                self.subscriber_groups.entry(issi).or_insert_with(HashSet::new);
                tracing::info!("CMCE: subscriber register issi={} known={}", issi, known);
            }
            BrewSubscriberAction::Deregister => {
                if let Some(existing) = self.subscriber_groups.remove(&issi) {
                    let existing_groups: Vec<u32> = existing.into_iter().collect();
                    for gssi in existing_groups {
                        self.dec_group_listener(gssi);
                        self.clear_group_floor_state_for_departure(queue, issi, gssi);
                        self.drop_group_calls_if_unlistened(queue, gssi);
                    }
                }

                // Release any active P2P individual calls involving this ISSI.
                // Without this, the TDMA timeslot stays occupied until call_timeout (120s).
                let calls_to_release: Vec<u16> = self
                    .individual_calls
                    .iter()
                    .filter(|(_, call)| call.calling_addr.ssi == issi || call.called_addr.ssi == issi)
                    .map(|(&id, _)| id)
                    .collect();
                for call_id in calls_to_release {
                    tracing::info!("CMCE: releasing individual call_id={} because ISSI {} deregistered", call_id, issi);
                    self.release_individual_call(queue, call_id, DisconnectCause::UserRequestedDisconnection);
                }

                tracing::info!("CMCE: subscriber deregister issi={}", issi);
            }
            BrewSubscriberAction::Affiliate => {
                let mut new_groups = Vec::new();
                {
                    let entry = self.subscriber_groups.entry(issi).or_insert_with(HashSet::new);
                    for gssi in groups {
                        if entry.insert(gssi) {
                            new_groups.push(gssi);
                        }
                    }
                }
                for gssi in &new_groups {
                    self.inc_group_listener(*gssi);
                }

                if new_groups.is_empty() {
                    tracing::debug!("CMCE: affiliate ignored (no new groups) issi={}", issi);
                } else {
                    tracing::info!("CMCE: subscriber affiliate issi={} groups={:?}", issi, new_groups);
                }
            }
            BrewSubscriberAction::Deaffiliate => {
                let mut removed_groups = Vec::new();
                let mut known_issi = false;
                if let Some(entry) = self.subscriber_groups.get_mut(&issi) {
                    known_issi = true;
                    for gssi in groups {
                        if entry.remove(&gssi) {
                            removed_groups.push(gssi);
                        }
                    }
                } else {
                    removed_groups = groups;
                }
                if known_issi {
                    for gssi in &removed_groups {
                        self.dec_group_listener(*gssi);
                    }
                }

                if removed_groups.is_empty() {
                    tracing::debug!("CMCE: deaffiliate ignored (no matching groups) issi={}", issi);
                } else {
                    tracing::info!("CMCE: subscriber deaffiliate issi={} groups={:?}", issi, removed_groups);
                    for gssi in &removed_groups {
                        self.clear_group_floor_state_for_departure(queue, issi, *gssi);
                    }
                    for gssi in &removed_groups {
                        self.drop_group_calls_if_unlistened(queue, *gssi);
                    }
                }
            }
            BrewSubscriberAction::ReleaseIndividualCalls => {
                let calls_to_release: Vec<u16> = self
                    .individual_calls
                    .iter()
                    .filter(|(_, call)| call.calling_addr.ssi == issi || call.called_addr.ssi == issi)
                    .map(|(&id, _)| id)
                    .collect();
                for call_id in calls_to_release {
                    tracing::info!(
                        "CMCE: releasing individual call_id={} for ISSI {} soft re-attach while preserving group affiliation",
                        call_id,
                        issi
                    );
                    self.release_individual_call(queue, call_id, DisconnectCause::UserRequestedDisconnection);
                }

                if self.subscriber_groups.contains_key(&issi) {
                    tracing::info!(
                        "CMCE: individual-call cleanup issi={} preserved groups={:?}",
                        issi,
                        self.subscriber_groups_for(issi)
                    );
                } else {
                    tracing::debug!("CMCE: individual-call cleanup issi={} with no subscriber-group state", issi);
                }
            }
        }
    }

    /// Send D-CALL-PROCEEDING (ETSI 14.7.1 step 1 of call setup).
    pub(super) fn send_d_call_proceeding(
        &mut self,
        queue: &mut MessageQueue,
        message: &SapMsg,
        pdu_request: &USetup,
        call_id: u16,
        setup_timeout: CallTimeoutSetupPhase,
        hook_method_selection: bool,
    ) {
        tracing::trace!("send_d_call_proceeding");

        let SapMsgInner::LcmcMleUnitdataInd(prim) = &message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let pdu_response = DCallProceeding {
            call_identifier: call_id,
            call_time_out_set_up_phase: setup_timeout,
            hook_method_selection,
            simplex_duplex_selection: pdu_request.simplex_duplex_selection,
            basic_service_information: None,
            call_status: None,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(25);
        pdu_response.to_bitbuf(&mut sdu).expect("Failed to serialize DCallProceeding");
        sdu.seek(0);
        tracing::debug!("send_d_call_proceeding: -> {:?} sdu {}", pdu_response, sdu.dump_bin());

        let msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: prim.handle,
                endpoint_id: prim.endpoint_id,
                link_id: prim.link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                unacked_bl_repetitions: None,
                chan_alloc: None,
                main_address: prim.received_tetra_address,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    /// Send D-ALERT to the calling MS for an individual call.
    /// ETSI EN 300 392-2 §14.7.3: BS sends D-ALERT after called MS responds with U-ALERT.
    pub(super) fn send_d_alert_individual(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        simplex_duplex: bool,
        calling_addr: TetraAddress,
        calling_handle: u32,
        calling_link_id: u32,
        calling_endpoint_id: u32,
        setup_timeout: CallTimeoutSetupPhase,
    ) {
        let d_alert = DAlert {
            call_identifier: call_id,
            call_time_out_set_up_phase: setup_timeout.into_raw() as u8,
            reserved: true, // per spec note: set to 1 for backwards compatibility
            simplex_duplex_selection: simplex_duplex,
            call_queued: false,
            basic_service_information: None,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };

        tracing::info!("-> {:?}", d_alert);
        let mut sdu = BitBuffer::new_autoexpand(32);
        d_alert.to_bitbuf(&mut sdu).expect("Failed to serialize DAlert");
        sdu.seek(0);

        let msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: calling_handle,
                endpoint_id: calling_endpoint_id,
                link_id: calling_link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                unacked_bl_repetitions: None,
                chan_alloc: None,
                main_address: calling_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    /// Decode External Subscriber Number IE (BCD-packed digits, ETSI 14.8.21).
    /// Type3FieldGeneric.data is u128 (up to 128 bits packed, max 32 BCD digits).
    /// ETSI specifies a max of 24 digits but we support up to 32 to cover edge cases.
    pub(super) fn decode_external_subscriber_number(field: &Type3FieldGeneric) -> String {
        if field.len == 0 {
            return String::new();
        }

        let nibble_count = (field.len / 4).min(32) as usize; // max 128 bits = 32 nibbles
        let total_bits = nibble_count * 4;
        let mut digits = String::with_capacity(nibble_count);
        for i in 0..nibble_count {
            // data stores the dialled number BCD-packed with the most-significant
            // nibble at the top of the used bits.
            let shift = total_bits - 4 - (i * 4);
            let nibble = ((field.data >> shift) & 0x0f) as u8;
            match nibble {
                0..=9 => digits.push(char::from(b'0' + nibble)),
                0x0a => digits.push('*'),
                0x0b => digits.push('#'),
                0x0c..=0x0f => {}
                _ => {}
            }
        }
        digits
    }

    /// Encode External Subscriber Number IE (ETSI 14.8.21).
    /// Supports up to 32 BCD digits (128 bits). ETSI allows up to 24 in spec; we go a bit further.
    pub(super) fn encode_external_subscriber_number(number: &str) -> Option<Type3FieldGeneric> {
        let trimmed = number.trim();
        if trimmed.is_empty() {
            return None;
        }

        const MAX_DIGITS: usize = 32;
        let mut nibbles: Vec<u8> = Vec::with_capacity(MAX_DIGITS);
        let mut encoded_preview = String::with_capacity(MAX_DIGITS);

        for ch in trimmed.chars() {
            let nibble = match ch {
                '0'..='9' => ch as u8 - b'0',
                '*' => 0x0a,
                '#' => 0x0b,
                _ => {
                    tracing::debug!("CMCE: ignoring unsupported external number char '{}' in '{}'", ch, number);
                    continue;
                }
            };

            if nibbles.len() == MAX_DIGITS {
                tracing::warn!(
                    "CMCE: external subscriber number '{}' exceeds {} BCD digits — truncating to '{}'",
                    number,
                    MAX_DIGITS,
                    encoded_preview
                );
                break;
            }

            nibbles.push(nibble);
            encoded_preview.push(ch);
        }

        if nibbles.is_empty() {
            tracing::debug!("CMCE: external number '{}' has no encodable digits", number);
            return None;
        }

        let len_bits = nibbles.len() * 4;
        let mut data: u128 = 0;
        // Pack nibbles MSB-first within the used bits, matching decode.
        for (idx, nibble) in nibbles.into_iter().enumerate() {
            let shift = len_bits - 4 - (idx * 4);
            data |= (nibble as u128) << shift;
        }

        Some(Type3FieldGeneric {
            field_id: CmceType3ElemId::ExtSubscriberNum.into_raw(),
            len: len_bits,
            data,
        })
    }

    pub(super) fn build_network_circuit_call_from_u_setup(pdu: &USetup, source_issi: u32) -> NetworkCircuitCall {
        // Prefer called_party_ssi as the number when it's a short service number (< 1_000_000)
        // and external_subscriber_number is present — terminals sometimes encode service codes
        // like 600 as SSI=600 + external_number='000' (BCD artifact). We must send '600' to
        // TetraPack, not '000'.
        let number = if let Some(ssi) = pdu.called_party_ssi {
            let ssi_u32 = ssi as u32;
            if ssi_u32 > 0 && ssi_u32 < 1_000_000 && pdu.external_subscriber_number.is_some() {
                // Use the SSI value as the dialled number string
                ssi_u32.to_string()
            } else {
                pdu.external_subscriber_number
                    .as_ref()
                    .map(Self::decode_external_subscriber_number)
                    .unwrap_or_default()
            }
        } else {
            pdu.external_subscriber_number
                .as_ref()
                .map(Self::decode_external_subscriber_number)
                .unwrap_or_default()
        };

        NetworkCircuitCall {
            source_issi,
            destination: pdu.called_party_ssi.unwrap_or(0) as u32,
            number,
            priority: pdu.call_priority,
            service: pdu.basic_service_information.speech_service.unwrap_or(0),
            mode: pdu.basic_service_information.circuit_mode_type.into_raw() as u8,
            duplex: pdu.simplex_duplex_selection as u8,
            method: pdu.hook_method_selection as u8,
            communication: pdu.basic_service_information.communication_type.into_raw() as u8,
            grant: 0,
            permission: pdu.request_to_transmit_send_data as u8,
            timeout: CallTimeout::T5m.into_raw() as u8,
            ownership: 1,
            queued: 0,
        }
    }

    #[inline]
    pub(super) fn has_external_called_party(pdu: &USetup, network_call: &NetworkCircuitCall) -> bool {
        !network_call.number.is_empty() || pdu.external_subscriber_number.is_some() || pdu.called_party_short_number_address.is_some()
    }

    /// Send D-DISCONNECT to the other party of an individual call.
    pub(super) fn send_d_disconnect_individual(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        call_snapshot: &IndividualCall,
        sender: TetraAddress,
        disconnect_cause: DisconnectCause,
    ) -> Option<TxReporter> {
        let target_addr = if sender.ssi == call_snapshot.calling_addr.ssi {
            Some(call_snapshot.called_addr)
        } else if sender.ssi == call_snapshot.called_addr.ssi {
            Some(call_snapshot.calling_addr)
        } else {
            tracing::warn!(
                "U-DISCONNECT/U-RELEASE (individual) call_id={} from unexpected ISSI {} (calling {}, called {})",
                call_id,
                sender.ssi,
                call_snapshot.calling_addr.ssi,
                call_snapshot.called_addr.ssi
            );
            None
        };

        let Some(target_addr) = target_addr else {
            return None;
        };

        let target_ts = if target_addr.ssi == call_snapshot.calling_addr.ssi {
            call_snapshot.calling_ts
        } else {
            call_snapshot.called_ts
        };
        let mut delivery_reporter = None;

        let d_disconnect = DDisconnect {
            call_identifier: call_id,
            disconnect_cause,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };
        tracing::info!("-> {:?} (to ISSI {})", d_disconnect, target_addr.ssi);

        let mut sdu = BitBuffer::new_autoexpand(32);
        d_disconnect.to_bitbuf(&mut sdu).expect("Failed to serialize DDisconnect");
        sdu.seek(0);

        let msg = if call_snapshot.has_established_circuit() {
            let usage = if target_addr.ssi == call_snapshot.calling_addr.ssi {
                Some(call_snapshot.calling_usage)
            } else {
                Some(call_snapshot.called_usage)
            };
            let reporter = TxReporter::new_unacked();
            delivery_reporter = Some(reporter.clone());
            // EN 300 392-2 clause 14.5.1.3.3 defines D-DISCONNECT as a
            // downlink request that expects an uplink U-RELEASE response. Keep
            // the assigned channel response-capable; the final D-RELEASE path
            // remains DL-only because it expects no MS response.
            Self::build_sapmsg_stealing_ul_dl_reported(sdu, target_addr, target_ts, usage, UlDlAssignment::Both, Some(reporter))
        } else if target_addr.ssi == call_snapshot.calling_addr.ssi {
            Self::build_sapmsg_direct(
                sdu,
                target_addr,
                call_snapshot.calling_handle,
                call_snapshot.calling_link_id,
                call_snapshot.calling_endpoint_id,
            )
        } else if let (Some(handle), Some(link_id), Some(endpoint_id)) = (
            call_snapshot.called_handle,
            call_snapshot.called_link_id,
            call_snapshot.called_endpoint_id,
        ) {
            Self::build_sapmsg_direct(sdu, target_addr, handle, link_id, endpoint_id)
        } else {
            Self::build_sapmsg(sdu, None, target_addr, Layer2Service::Unacknowledged, None)
        };
        queue.push_back(msg);
        delivery_reporter
    }

    pub(super) fn begin_group_tx_ceased_tail_drain(
        &mut self,
        call_id: u16,
        sender: TetraAddress,
        dest_gssi: u32,
        ts: u8,
        usage: u8,
        notify_brew: bool,
    ) {
        if self.pending_group_tx_ceased_tail_drains.contains_key(&call_id) {
            tracing::debug!("CMCE: group TX-CEASED tail drain already pending for call_id={}", call_id);
            return;
        }
        self.cancel_group_floor_activation(call_id, "group speaker sent U-TX CEASED");

        tracing::debug!(
            "CMCE: delaying group TX-CEASED/floor idle call_id={} sender ISSI {} GSSI {} for {} timeslots of TCH tail drain",
            call_id,
            sender.ssi,
            dest_gssi,
            GROUP_TX_CEASED_TAIL_DRAIN_TIMESLOTS
        );
        self.pending_group_tx_ceased_tail_drains.insert(
            call_id,
            PendingGroupTxCeasedTailDrain {
                call_id,
                sender,
                dest_gssi,
                ts,
                usage,
                notify_brew,
                started_at: self.dltime,
            },
        );
    }

    pub(super) fn cancel_group_tx_ceased_tail_drain(&mut self, call_id: u16, reason: &str) -> bool {
        let Some(pending) = self.pending_group_tx_ceased_tail_drains.remove(&call_id) else {
            return false;
        };
        tracing::debug!(
            "CMCE: cancelling pending group TX-CEASED tail drain call_id={} sender ISSI {} because {}",
            pending.call_id,
            pending.sender.ssi,
            reason
        );
        true
    }

    pub(super) fn cancel_matching_group_tx_ceased_tail_drain(&mut self, call_id: u16, sender_issi: u32, reason: &str) -> bool {
        let should_cancel = self
            .pending_group_tx_ceased_tail_drains
            .get(&call_id)
            .is_some_and(|pending| pending.sender.ssi == sender_issi);
        if !should_cancel {
            return false;
        }
        self.cancel_group_tx_ceased_tail_drain(call_id, reason)
    }

    pub(super) fn has_pending_group_floor_activation(&self, call_id: u16) -> bool {
        self.pending_group_floor_activations.contains_key(&call_id)
    }

    pub(super) fn cancel_group_floor_activation(&mut self, call_id: u16, reason: &str) -> bool {
        if let Some(pending) = self.pending_group_floor_activations.remove(&call_id) {
            tracing::debug!(
                "CMCE: cancelling pending group floor activation call_id={} source ISSI {} because {}",
                pending.call_id,
                pending.source_issi,
                reason
            );
            true
        } else {
            false
        }
    }

    pub(super) fn queue_group_floor_activation(
        &mut self,
        call_id: u16,
        source_issi: u32,
        dest_gssi: u32,
        ts: u8,
        reporter: TxReporter,
        notify_brew: bool,
    ) {
        // EN 300 392-2 clause 14.5.2.2.1 makes D-TX GRANTED the SwMI's
        // air-interface floor authorization. Keep UMAC/Brew U-plane activation
        // behind the positive requester grant reaching MAC, so a rapid PTT
        // retake cannot start local speech acceptance before the MS has been
        // told to transmit.
        if let Some(pending) = self.pending_group_floor_activations.get_mut(&call_id) {
            if pending.source_issi == source_issi && pending.dest_gssi == dest_gssi && pending.ts == ts {
                pending.reporters.push(reporter);
                pending.notify_brew |= notify_brew;
                return;
            }
            tracing::debug!(
                "CMCE: replacing pending group floor activation call_id={} old ISSI {} -> new ISSI {}",
                call_id,
                pending.source_issi,
                source_issi
            );
        }

        self.pending_group_floor_activations.insert(
            call_id,
            PendingGroupFloorActivation {
                call_id,
                source_issi,
                dest_gssi,
                ts,
                reporters: vec![reporter],
                notify_brew,
                started_at: self.dltime,
            },
        );
    }

    pub(super) fn drain_pending_group_floor_activations(&mut self, queue: &mut MessageQueue) {
        let ready: Vec<(u16, bool)> = self
            .pending_group_floor_activations
            .iter()
            .filter_map(|(&call_id, pending)| {
                if pending.reporters.iter().any(TxReporter::is_transmitted) {
                    Some((call_id, true))
                } else if !pending.reporters.is_empty() && pending.reporters.iter().all(TxReporter::is_discarded) {
                    Some((call_id, false))
                } else {
                    None
                }
            })
            .collect();

        for (call_id, was_transmitted) in ready {
            let Some(pending) = self.pending_group_floor_activations.remove(&call_id) else {
                continue;
            };

            if was_transmitted {
                self.complete_group_floor_activation(queue, pending);
                continue;
            }

            tracing::warn!(
                "CMCE: suppressing group FloorGranted call_id={} source ISSI {} because positive D-TX GRANTED was discarded before transmission",
                pending.call_id,
                pending.source_issi
            );
            if let Some(call) = self.active_calls.get_mut(&pending.call_id)
                && call.is_current_speaker(pending.source_issi)
                && call.dest_gssi == pending.dest_gssi
                && call.ts == pending.ts
            {
                call.enter_hangtime(self.dltime);
            }
        }
    }

    pub(super) fn queue_network_group_ready(
        &mut self,
        call_id: u16,
        brew_uuid: uuid::Uuid,
        source_issi: u32,
        dest_gssi: u32,
        ts: u8,
        usage: u8,
        reporters: Vec<TxReporter>,
        notify_speaker_changed: bool,
    ) {
        if reporters.is_empty() {
            tracing::warn!(
                "CMCE: network group ready call_id={} uuid={} has no RF reporter; completing without gating",
                call_id,
                brew_uuid
            );
            self.pending_network_group_readies.insert(
                call_id,
                PendingNetworkGroupReady {
                    brew_uuid,
                    call_id,
                    source_issi,
                    dest_gssi,
                    ts,
                    usage,
                    reporters,
                    notify_speaker_changed,
                    started_at: self.dltime,
                },
            );
            return;
        }

        if let Some(pending) = self.pending_network_group_readies.insert(
            call_id,
            PendingNetworkGroupReady {
                brew_uuid,
                call_id,
                source_issi,
                dest_gssi,
                ts,
                usage,
                reporters,
                notify_speaker_changed,
                started_at: self.dltime,
            },
        ) {
            tracing::debug!(
                "CMCE: replaced pending network group ready call_id={} old_uuid={} new_uuid={}",
                call_id,
                pending.brew_uuid,
                brew_uuid
            );
        }
    }

    pub(super) fn cancel_network_group_ready(&mut self, call_id: u16, reason: &str) -> bool {
        if let Some(pending) = self.pending_network_group_readies.remove(&call_id) {
            tracing::debug!(
                "CMCE: cancelling pending network group ready call_id={} uuid={} because {}",
                call_id,
                pending.brew_uuid,
                reason
            );
            true
        } else {
            false
        }
    }

    pub(super) fn drain_pending_network_group_readies(&mut self, queue: &mut MessageQueue) {
        let ready: Vec<(u16, bool)> = self
            .pending_network_group_readies
            .iter()
            .filter_map(|(&call_id, pending)| {
                if pending.reporters.is_empty() || pending.reporters.iter().any(TxReporter::is_transmitted) {
                    Some((call_id, true))
                } else if pending.reporters.iter().all(TxReporter::is_discarded)
                    || pending.started_at.age(self.dltime) >= NETWORK_GROUP_READY_PENDING_TIMEOUT_TIMESLOTS
                {
                    Some((call_id, false))
                } else {
                    None
                }
            })
            .collect();

        for (call_id, was_transmitted) in ready {
            let Some(pending) = self.pending_network_group_readies.remove(&call_id) else {
                continue;
            };

            if was_transmitted {
                self.complete_network_group_ready(queue, pending);
                continue;
            }

            tracing::warn!(
                "CMCE: releasing network group call_id={} uuid={} because RF setup/floor signalling was not transmitted before ready guard",
                pending.call_id,
                pending.brew_uuid
            );
            self.release_group_call(queue, pending.call_id, DisconnectCause::AcknowledgedServiceNotComplete);
        }
    }

    fn complete_network_group_ready(&mut self, queue: &mut MessageQueue, pending: PendingNetworkGroupReady) {
        let Some(call) = self.active_calls.get(&pending.call_id) else {
            tracing::debug!(
                "CMCE: dropping pending network group ready call_id={} uuid={}; call no longer active",
                pending.call_id,
                pending.brew_uuid
            );
            return;
        };

        if !call.is_current_speaker(pending.source_issi)
            || call.dest_gssi != pending.dest_gssi
            || call.ts != pending.ts
            || call.usage != pending.usage
            || call.brew_uuid != Some(pending.brew_uuid)
        {
            tracing::debug!(
                "CMCE: dropping stale network group ready call_id={} uuid={} source={} gssi={} ts={}; active source={} gssi={} ts={} uuid={:?}",
                pending.call_id,
                pending.brew_uuid,
                pending.source_issi,
                pending.dest_gssi,
                pending.ts,
                call.source_issi,
                call.dest_gssi,
                call.ts,
                call.brew_uuid
            );
            return;
        }

        tracing::info!(
            "CMCE: network group ready call_id={} uuid={} source ISSI {} GSSI {} ts={} after RF control signalling",
            pending.call_id,
            pending.brew_uuid,
            pending.source_issi,
            pending.dest_gssi,
            pending.ts
        );

        Self::signal_umac_dl_media_source(queue, pending.ts, CircuitDlMediaSource::SwMI);

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: pending.call_id,
                source_issi: pending.source_issi,
                dest_gssi: pending.dest_gssi,
                ts: pending.ts,
            }),
        });

        if pending.notify_speaker_changed {
            self.emit(crate::net_telemetry::TelemetryEvent::GroupCallSpeakerChanged {
                call_id: pending.call_id,
                gssi: pending.dest_gssi,
                speaker_issi: pending.source_issi,
            });
        }

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Brew,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid: pending.brew_uuid,
                call_id: pending.call_id,
                ts: pending.ts,
                usage: pending.usage,
            }),
        });
    }

    fn complete_group_floor_activation(&mut self, queue: &mut MessageQueue, pending: PendingGroupFloorActivation) {
        let Some(call) = self.active_calls.get(&pending.call_id) else {
            tracing::debug!(
                "CMCE: dropping pending group floor activation call_id={} source ISSI {}; call no longer active",
                pending.call_id,
                pending.source_issi
            );
            return;
        };
        if !call.is_current_speaker(pending.source_issi) || call.dest_gssi != pending.dest_gssi || call.ts != pending.ts {
            tracing::debug!(
                "CMCE: dropping stale group floor activation call_id={} source ISSI {} dest GSSI {} ts {}; active call is source ISSI {} dest GSSI {} ts {}",
                pending.call_id,
                pending.source_issi,
                pending.dest_gssi,
                pending.ts,
                call.source_issi,
                call.dest_gssi,
                call.ts
            );
            return;
        }

        tracing::info!(
            "CMCE: group floor activation call_id={} source ISSI {} dest GSSI {} ts={} after D-TX GRANTED transmission",
            pending.call_id,
            pending.source_issi,
            pending.dest_gssi,
            pending.ts
        );

        Self::signal_umac_dl_media_source(queue, pending.ts, CircuitDlMediaSource::LocalLoopback);

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id: pending.call_id,
                source_issi: pending.source_issi,
                dest_gssi: pending.dest_gssi,
                ts: pending.ts,
            }),
        });

        if pending.notify_brew {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                    call_id: pending.call_id,
                    source_issi: pending.source_issi,
                    dest_gssi: pending.dest_gssi,
                    ts: pending.ts,
                }),
            });
        }
    }

    pub(super) fn drain_pending_group_tx_ceased_tail_drains(&mut self, queue: &mut MessageQueue) {
        let ready: Vec<u16> = self
            .pending_group_tx_ceased_tail_drains
            .iter()
            .filter_map(|(&call_id, pending)| {
                if pending.started_at.age(self.dltime) >= GROUP_TX_CEASED_TAIL_DRAIN_TIMESLOTS {
                    Some(call_id)
                } else {
                    None
                }
            })
            .collect();

        for call_id in ready {
            let Some(pending) = self.pending_group_tx_ceased_tail_drains.remove(&call_id) else {
                continue;
            };
            self.complete_group_tx_ceased_tail_drain(queue, pending);
        }
    }

    fn complete_group_tx_ceased_tail_drain(&mut self, queue: &mut MessageQueue, pending: PendingGroupTxCeasedTailDrain) {
        let (queued_requester, legacy_release_after_cease) = {
            let Some(call) = self.active_calls.get(&pending.call_id) else {
                return;
            };
            if !call.is_current_speaker(pending.sender.ssi) || call.ts != pending.ts {
                tracing::debug!(
                    "CMCE: group TX-CEASED tail drain call_id={} skipped; floor changed from ISSI {}",
                    pending.call_id,
                    pending.sender.ssi
                );
                return;
            }
            let legacy_release_after_cease =
                self.config.config().cell.legacy_gssi_group_call && matches!(&call.origin, CallOrigin::Local { .. });
            let queued_requester = if legacy_release_after_cease {
                let mut first_affiliated = None;
                let mut first_affiliated_other_speaker = None;
                for requester in call.queued_tx_demands() {
                    let affiliated = self.subscriber_affiliated_to_group(requester.ssi, call.dest_gssi);
                    if !affiliated {
                        tracing::info!(
                            "CMCE: dropping queued group floor requester ISSI {} for call_id={} gssi={} after affiliation loss during legacy group TX-CEASED tail drain",
                            requester.ssi,
                            pending.call_id,
                            call.dest_gssi
                        );
                        continue;
                    }
                    first_affiliated.get_or_insert(requester);
                    if requester.ssi != pending.sender.ssi {
                        first_affiliated_other_speaker = Some(requester);
                        break;
                    }
                }
                first_affiliated_other_speaker.or(first_affiliated)
            } else {
                self.first_affiliated_group_floor_requester(pending.call_id, call, "group TX-CEASED tail drain")
            };
            (queued_requester, legacy_release_after_cease)
        };

        let queued_requester = {
            let Some(call) = self.active_calls.get_mut(&pending.call_id) else {
                return;
            };
            if !call.is_current_speaker(pending.sender.ssi) || call.ts != pending.ts {
                return;
            }

            let queued_requester = call.take_queued_tx_demand_through(queued_requester.map(|requester| requester.ssi));
            let legacy_same_speaker_retake =
                legacy_release_after_cease && queued_requester.is_some_and(|requester| requester.ssi == pending.sender.ssi);
            if let Some(requester) = queued_requester.filter(|_| !legacy_same_speaker_retake) {
                call.grant_floor(requester.ssi, Some(requester));
                Some(requester)
            } else {
                call.enter_hangtime(self.dltime);
                queued_requester
            }
        };

        let legacy_same_speaker_retake =
            legacy_release_after_cease && queued_requester.is_some_and(|requester| requester.ssi == pending.sender.ssi);

        if let Some(requester) = queued_requester.filter(|_| !legacy_same_speaker_retake) {
            tracing::info!(
                "U-TX CEASED (group) call_id={} tail-drained from ISSI {} -> granting queued floor to ISSI {}",
                pending.call_id,
                pending.sender.ssi,
                requester.ssi
            );

            let reporter = self.fsm_send_d_tx_granted_individual_reported(
                queue,
                pending.call_id,
                requester,
                pending.ts,
                pending.usage,
                TransmissionGrant::Granted,
                Some(requester.ssi),
            );
            self.send_group_listener_d_tx_granted_facch(
                queue,
                pending.call_id,
                requester.ssi,
                pending.dest_gssi,
                pending.ts,
                pending.usage,
            );
            self.reset_group_t310_after_floor_grant(pending.call_id);
            self.refresh_group_cached_d_setup_speaker(pending.call_id, requester.ssi);

            self.emit(crate::net_telemetry::TelemetryEvent::GroupCallSpeakerChanged {
                call_id: pending.call_id,
                gssi: pending.dest_gssi,
                speaker_issi: requester.ssi,
            });

            self.queue_group_floor_activation(
                pending.call_id,
                requester.ssi,
                pending.dest_gssi,
                pending.ts,
                reporter,
                pending.notify_brew,
            );
            return;
        }

        if legacy_same_speaker_retake {
            tracing::info!(
                "CMCE: legacy GSSI group call mode suppresses same-speaker fast retake call_id={} ISSI {}; clearing old over before fresh setup",
                pending.call_id,
                pending.sender.ssi
            );
        }

        tracing::info!(
            "-> D-TX CEASED (group, tail-drained FACCH) call_id={} GSSI {} sender ISSI {}",
            pending.call_id,
            pending.dest_gssi,
            pending.sender.ssi
        );
        self.send_d_tx_ceased_facch(queue, pending.call_id, pending.dest_gssi, pending.ts, pending.usage);

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased {
                call_id: pending.call_id,
                ts: pending.ts,
            }),
        });

        if pending.notify_brew {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased {
                    call_id: pending.call_id,
                    ts: pending.ts,
                }),
            });
        }

        if legacy_release_after_cease {
            // Compatibility mode for older Motorola MR5/MR19 class terminals:
            // D-TX-CEASED is the normal clause 14.5.2.2.1(e)
            // end-of-transmission edge, and D-RELEASE is the clause 14.5.2.3
            // group release PDU. The release-after-over decision is a local
            // compatibility policy, not a general ETSI requirement. Release the
            // maintained group call after the normal D-TX-CEASED edge so the
            // next PTT uses the proven fresh
            // U-SETUP/D-CONNECT/D-SETUP sequence instead of a same-channel
            // hangtime retake that some old terminals acknowledge but do not
            // transmit on.
            tracing::info!(
                "CMCE: legacy GSSI group call mode releasing local group call_id={} after no-handoff over",
                pending.call_id
            );
            self.release_group_call(queue, pending.call_id, DisconnectCause::SwmiRequestedDisconnection);
        }
    }

    pub(super) fn begin_individual_tx_ceased_tail_drain(
        &mut self,
        call_id: u16,
        sender: IndividualTailDrainLeg,
        peer: IndividualTailDrainLeg,
        notify_brew: bool,
    ) {
        if self.pending_individual_tx_ceased_tail_drains.contains_key(&call_id) {
            tracing::debug!("CMCE: simplex private TX-CEASED tail drain already pending for call_id={}", call_id);
            return;
        }

        tracing::debug!(
            "CMCE: delaying simplex private TX-CEASED/floor handoff call_id={} sender ISSI {} for {} timeslots of TCH tail drain",
            call_id,
            sender.addr.ssi,
            INDIVIDUAL_SIMPLEX_TAIL_DRAIN_TIMESLOTS
        );
        self.pending_individual_tx_ceased_tail_drains.insert(
            call_id,
            PendingIndividualTxCeasedTailDrain {
                call_id,
                sender,
                peer,
                notify_brew,
                started_at: self.dltime,
            },
        );
    }

    pub(super) fn begin_individual_disconnect_tail_drain(
        &mut self,
        call_id: u16,
        sender: TetraAddress,
        peer_issi: u32,
        disconnect_cause: DisconnectCause,
    ) {
        if self.pending_individual_disconnect_tail_drains.contains_key(&call_id) {
            tracing::debug!(
                "CMCE: simplex private disconnect tail drain already pending for call_id={}",
                call_id
            );
            return;
        }

        let started_at = match self.pending_individual_tx_ceased_tail_drains.remove(&call_id) {
            Some(pending) => {
                tracing::debug!(
                    "CMCE: simplex private disconnect call_id={} supersedes pending TX-CEASED tail drain from ISSI {}; suppressing stale D-TX CEASED",
                    call_id,
                    pending.sender.addr.ssi
                );
                pending.started_at
            }
            None => self.dltime,
        };

        tracing::debug!(
            "CMCE: delaying simplex private peer D-RELEASE call_id={} sender ISSI {} peer ISSI {} for TCH tail drain",
            call_id,
            sender.ssi,
            peer_issi
        );
        self.pending_individual_disconnect_tail_drains.insert(
            call_id,
            PendingIndividualDisconnectTailDrain {
                sender,
                peer_issi,
                cause: disconnect_cause,
                started_at,
            },
        );
    }

    pub(super) fn has_pending_individual_disconnect_tail_drain(&self, call_id: u16) -> bool {
        self.pending_individual_disconnect_tail_drains.contains_key(&call_id)
    }

    pub(super) fn cancel_matching_individual_tx_ceased_tail_drain(&mut self, call_id: u16, sender_issi: u32) -> bool {
        let should_cancel = self
            .pending_individual_tx_ceased_tail_drains
            .get(&call_id)
            .is_some_and(|pending| pending.sender.addr.ssi == sender_issi);
        if !should_cancel {
            return false;
        }

        if let Some(pending) = self.pending_individual_tx_ceased_tail_drains.remove(&call_id) {
            tracing::debug!(
                "CMCE: same-speaker U-TX DEMAND call_id={} from ISSI {} supersedes pending TX-CEASED tail drain; suppressing stale D-TX CEASED/FloorReleased",
                call_id,
                pending.sender.addr.ssi
            );
            true
        } else {
            false
        }
    }

    pub(super) fn drain_pending_individual_tx_ceased_tail_drains(&mut self, queue: &mut MessageQueue) {
        let ready: Vec<u16> = self
            .pending_individual_tx_ceased_tail_drains
            .iter()
            .filter_map(|(&call_id, pending)| {
                if pending.started_at.age(self.dltime) >= INDIVIDUAL_SIMPLEX_TAIL_DRAIN_TIMESLOTS {
                    Some(call_id)
                } else {
                    None
                }
            })
            .collect();

        for call_id in ready {
            let Some(pending) = self.pending_individual_tx_ceased_tail_drains.remove(&call_id) else {
                continue;
            };
            self.complete_individual_tx_ceased_tail_drain(queue, pending);
        }
    }

    fn complete_individual_tx_ceased_tail_drain(&mut self, queue: &mut MessageQueue, pending: PendingIndividualTxCeasedTailDrain) {
        let Some(call_snapshot) = self.individual_calls.get(&pending.call_id).cloned() else {
            return;
        };
        let queued_requester = {
            let Some(call) = self.individual_calls.get_mut(&pending.call_id) else {
                return;
            };
            if !call.is_active() || call.simplex_duplex {
                return;
            }
            if call.floor_holder != Some(pending.sender.addr.ssi) {
                tracing::debug!(
                    "CMCE: simplex private TX-CEASED tail drain call_id={} skipped; floor holder changed from ISSI {} to {:?}",
                    pending.call_id,
                    pending.sender.addr.ssi,
                    call.floor_holder
                );
                return;
            }
            call.clear_floor_holder();
            call.take_queued_tx_demand()
        };

        if let Some(requester) = queued_requester {
            let (requester_leg, listener_leg) = if requester.ssi == pending.peer.addr.ssi {
                (pending.peer, pending.sender)
            } else {
                (pending.sender, pending.peer)
            };

            tracing::info!(
                "U-TX CEASED (individual) call_id={} tail-drained from ISSI {} -> granting queued floor to ISSI {}",
                pending.call_id,
                pending.sender.addr.ssi,
                requester_leg.addr.ssi
            );

            if let Some(call) = self.individual_calls.get_mut(&pending.call_id) {
                call.set_floor_holder(requester_leg.addr.ssi);
            }

            Self::push_individual_d_tx_granted_if_local_rf(
                queue,
                &call_snapshot,
                pending.call_id,
                requester_leg.addr,
                requester_leg.ts,
                requester_leg.usage,
                UlDlAssignment::Both,
                TransmissionGrant::Granted,
                requester_leg.addr.ssi,
            );
            Self::push_individual_d_tx_granted_if_local_rf(
                queue,
                &call_snapshot,
                pending.call_id,
                listener_leg.addr,
                listener_leg.ts,
                listener_leg.usage,
                UlDlAssignment::Both,
                TransmissionGrant::GrantedToOtherUser,
                requester_leg.addr.ssi,
            );

            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                    call_id: pending.call_id,
                    source_issi: requester_leg.addr.ssi,
                    dest_gssi: listener_leg.addr.ssi,
                    ts: requester_leg.ts,
                }),
            });

            if pending.notify_brew {
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Brew,
                    msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                        call_id: pending.call_id,
                        source_issi: requester_leg.addr.ssi,
                        dest_gssi: listener_leg.addr.ssi,
                        ts: requester_leg.ts,
                    }),
                });
            }
            return;
        }

        tracing::info!(
            "-> D-TX CEASED (individual simplex, tail-drained FACCH) call_id={} to sender ISSI {} and peer ISSI {}",
            pending.call_id,
            pending.sender.addr.ssi,
            pending.peer.addr.ssi
        );
        Self::push_individual_d_tx_ceased_if_local_rf(
            queue,
            &call_snapshot,
            pending.call_id,
            pending.sender.addr,
            pending.sender.ts,
            pending.sender.usage,
        );
        Self::push_individual_d_tx_ceased_if_local_rf(
            queue,
            &call_snapshot,
            pending.call_id,
            pending.peer.addr,
            pending.peer.ts,
            pending.peer.usage,
        );

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased {
                call_id: pending.call_id,
                ts: pending.sender.ts,
            }),
        });

        if pending.notify_brew {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased {
                    call_id: pending.call_id,
                    ts: pending.sender.ts,
                }),
            });
        }
    }

    pub(super) fn drain_pending_individual_disconnect_tail_drains(&mut self, queue: &mut MessageQueue) {
        let ready: Vec<u16> = self
            .pending_individual_disconnect_tail_drains
            .iter()
            .filter_map(|(&call_id, pending)| {
                if pending.started_at.age(self.dltime) >= INDIVIDUAL_SIMPLEX_TAIL_DRAIN_TIMESLOTS {
                    Some(call_id)
                } else {
                    None
                }
            })
            .collect();

        for call_id in ready {
            let Some(pending) = self.pending_individual_disconnect_tail_drains.remove(&call_id) else {
                continue;
            };
            let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
                continue;
            };
            if !call_snapshot.is_active() {
                tracing::debug!(
                    "CMCE: simplex private disconnect tail drain call_id={} skipped; call is no longer active",
                    call_id
                );
                continue;
            }
            if call_snapshot.peer_issi_for(pending.sender.ssi) != Some(pending.peer_issi) {
                tracing::warn!(
                    "CMCE: simplex private disconnect tail drain call_id={} participant mismatch for sender ISSI {}",
                    call_id,
                    pending.sender.ssi
                );
                continue;
            }

            tracing::info!(
                "CMCE: simplex private peer clear by D-RELEASE call_id={} peer ISSI {} cause={:?}",
                call_id,
                pending.peer_issi,
                pending.cause
            );
            // EN 300 392-2 clause 14.5.1.3.1 permits the SwMI to inform the
            // other MS by D-DISCONNECT or D-RELEASE. Live Motorola MXP600 RF
            // testing showed peer D-DISCONNECT can trigger a terminal reboot,
            // so local simplex uses the D-RELEASE alternative after the same
            // bearer-tail drain. This expects no U-RELEASE response; the BS
            // closes only after the release reporters complete or guard out.
            self.send_individual_disconnect_peer_release(queue, call_id, &call_snapshot, pending.cause, pending.peer_issi);
        }
    }

    pub(super) fn begin_individual_disconnect_delivery(
        &mut self,
        call_id: u16,
        awaiting_release_from: u32,
        release_to_issi: u32,
        reporter: TxReporter,
        disconnect_cause: DisconnectCause,
    ) {
        // EN 300 392-2 clause 14.5.1.3.3 makes U-RELEASE the MS response to
        // D-DISCONNECT. Start the peer-response wait only after the active
        // channel D-DISCONNECT has been reported to MAC, with a local guard so
        // a lost reporter cannot pin the traffic circuit forever.
        self.pending_individual_disconnect_deliveries.insert(
            call_id,
            PendingIndividualDisconnectDelivery {
                awaiting_release_from,
                release_to_issi,
                cause: disconnect_cause,
                reporter,
                started_at: self.dltime,
            },
        );
    }

    pub(super) fn take_individual_disconnect_delivery_release_if_awaited_by(
        &mut self,
        call_id: u16,
        sender_issi: u32,
    ) -> Option<(DisconnectCause, u32)> {
        if self
            .pending_individual_disconnect_deliveries
            .get(&call_id)
            .is_some_and(|pending| pending.awaiting_release_from == sender_issi)
        {
            return self
                .pending_individual_disconnect_deliveries
                .remove(&call_id)
                .map(|pending| (pending.cause, pending.release_to_issi));
        }
        None
    }

    pub(super) fn has_pending_individual_disconnect_delivery(&self, call_id: u16) -> bool {
        self.pending_individual_disconnect_deliveries.contains_key(&call_id)
    }

    pub(super) fn has_pending_individual_disconnect_release_ack(&self, call_id: u16) -> bool {
        self.pending_individual_disconnect_release_acks.contains_key(&call_id)
    }

    pub(super) fn has_pending_individual_release(&self, call_id: u16) -> bool {
        self.pending_individual_releases.contains_key(&call_id)
    }

    pub(super) fn drain_pending_individual_disconnect_deliveries(&mut self, queue: &mut MessageQueue) {
        let ready: Vec<(u16, bool)> = self
            .pending_individual_disconnect_deliveries
            .iter()
            .filter_map(|(&call_id, pending)| {
                if pending.reporter.is_transmitted() {
                    Some((call_id, true))
                } else if pending.reporter.is_discarded()
                    || pending.started_at.age(self.dltime) >= INDIVIDUAL_DISCONNECT_DELIVERY_TIMEOUT_TIMESLOTS
                {
                    Some((call_id, false))
                } else {
                    None
                }
            })
            .collect();

        for (call_id, disconnect_was_transmitted) in ready {
            let Some(pending) = self.pending_individual_disconnect_deliveries.remove(&call_id) else {
                continue;
            };
            if disconnect_was_transmitted {
                if let Some(call) = self.individual_calls.get_mut(&call_id)
                    && call.is_active()
                {
                    call.begin_disconnect_pending(pending.awaiting_release_from, pending.release_to_issi, self.dltime, pending.cause);
                }
            } else if pending.reporter.is_discarded() {
                tracing::warn!(
                    "CMCE: D-DISCONNECT discarded before delivery for call_id={}; sending D-RELEASE instead of waiting for peer U-RELEASE",
                    call_id
                );
                self.release_individual_disconnect_fallback(queue, call_id, pending.cause, Some(pending.awaiting_release_from));
            } else {
                tracing::warn!(
                    "CMCE: D-DISCONNECT delivery reporter still pending for call_id={} after local guard; sending D-RELEASE instead of waiting for peer U-RELEASE",
                    call_id
                );
                self.release_individual_disconnect_fallback(queue, call_id, pending.cause, Some(pending.awaiting_release_from));
            }
        }
    }

    pub(super) fn send_individual_disconnect_release_ack(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        call: &IndividualCall,
        release_to_issi: u32,
        disconnect_cause: DisconnectCause,
    ) {
        if self.pending_individual_disconnect_release_acks.contains_key(&call_id) {
            return;
        }

        // EN 300 392-2 clause 14.5.1.3.1 says the MS that sent
        // U-DISCONNECT waits for D-RELEASE. Send that acknowledgement promptly
        // to the requesting leg. The peer leg is cleared after the bearer
        // tail drain by D-RELEASE, a clause 14.5.1.3.1 peer-clear option that
        // expects no MS response.
        let reporters = self.send_established_individual_release_pdus(queue, call_id, call, disconnect_cause, Some(release_to_issi));
        self.pending_individual_disconnect_release_acks.insert(
            call_id,
            PendingIndividualDisconnectReleaseAck {
                release_to_issi,
                cause: disconnect_cause,
                reporters,
                peer_release_reporters: Vec::new(),
                started_at: self.dltime,
                peer_clear_complete: false,
            },
        );
        self.complete_individual_disconnect_if_ready(queue, call_id);
    }

    pub(super) fn send_individual_disconnect_peer_release(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        call: &IndividualCall,
        disconnect_cause: DisconnectCause,
        peer_issi: u32,
    ) {
        // EN 300 392-2 clause 14.5.1.3.1 permits peer clear with D-RELEASE.
        // Clause 14.5.1.2.2 f) makes the "imminent call disconnection"
        // notification optional; live Motorola private-simplex tests render
        // that optional notification as "No answer" on otherwise established
        // calls. Keep the mandatory peer D-RELEASE, but do not add D-INFO or
        // notification 26 on this compatibility-sensitive path.
        let reporters = self.send_established_individual_release_pdus(queue, call_id, call, disconnect_cause, Some(peer_issi));
        if let Some(pending) = self.pending_individual_disconnect_release_acks.get_mut(&call_id) {
            pending.peer_release_reporters.extend(reporters);
            self.complete_individual_disconnect_if_ready(queue, call_id);
        } else {
            self.begin_individual_release(queue, call_id, disconnect_cause, Vec::new(), true, Some(peer_issi));
        }
    }

    pub(super) fn complete_individual_disconnect_peer_release(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        disconnect_cause: DisconnectCause,
        release_to_issi: u32,
    ) {
        if let Some(pending) = self.pending_individual_disconnect_release_acks.get_mut(&call_id) {
            pending.peer_clear_complete = true;
            self.complete_individual_disconnect_if_ready(queue, call_id);
        } else {
            self.release_individual_call_to_issi(queue, call_id, disconnect_cause, release_to_issi);
        }
    }

    pub(super) fn complete_individual_disconnect_peer_clear_without_downlink(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        disconnect_cause: DisconnectCause,
    ) {
        if let Some(pending) = self.pending_individual_disconnect_release_acks.get_mut(&call_id) {
            pending.peer_clear_complete = true;
            self.complete_individual_disconnect_if_ready(queue, call_id);
            return;
        }

        let Some(call) = self.individual_calls.remove(&call_id) else {
            return;
        };
        self.complete_individual_release_cleanup(queue, call_id, call, disconnect_cause, Vec::new(), true);
    }

    pub(super) fn release_individual_disconnect_fallback(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        disconnect_cause: DisconnectCause,
        release_to_issi: Option<u32>,
    ) {
        if let Some(pending_ack) = self.pending_individual_disconnect_release_acks.get(&call_id) {
            let release_to_peer = release_to_issi.or_else(|| {
                self.individual_calls
                    .get(&call_id)
                    .and_then(|call| call.peer_issi_for(pending_ack.release_to_issi))
            });
            if let Some(peer_issi) = release_to_peer {
                self.begin_individual_release(queue, call_id, disconnect_cause, Vec::new(), true, Some(peer_issi));
                return;
            }
        }

        self.release_individual_call(queue, call_id, disconnect_cause);
    }

    pub(super) fn drain_pending_individual_disconnect_release_acks(&mut self, queue: &mut MessageQueue) {
        let ready: Vec<u16> = self
            .pending_individual_disconnect_release_acks
            .iter()
            .filter_map(|(&call_id, pending)| {
                if self.individual_disconnect_peer_clear_done(pending) && self.individual_disconnect_release_ack_done(pending) {
                    Some(call_id)
                } else {
                    None
                }
            })
            .collect();

        for call_id in ready {
            self.complete_individual_disconnect_if_ready(queue, call_id);
        }
    }

    fn individual_disconnect_release_ack_done(&self, pending: &PendingIndividualDisconnectReleaseAck) -> bool {
        pending.reporters.iter().all(TxReporter::is_transmitted) && pending.peer_release_reporters.iter().all(TxReporter::is_transmitted)
            || pending.started_at.age(self.dltime) >= INDIVIDUAL_RELEASE_PENDING_TIMEOUT_TIMESLOTS
    }

    fn individual_disconnect_peer_clear_done(&self, pending: &PendingIndividualDisconnectReleaseAck) -> bool {
        pending.peer_clear_complete
            || (!pending.peer_release_reporters.is_empty()
                && (pending.peer_release_reporters.iter().all(TxReporter::is_transmitted)
                    || pending.started_at.age(self.dltime) >= INDIVIDUAL_RELEASE_PENDING_TIMEOUT_TIMESLOTS))
    }

    fn complete_individual_disconnect_if_ready(&mut self, queue: &mut MessageQueue, call_id: u16) {
        let ready = self
            .pending_individual_disconnect_release_acks
            .get(&call_id)
            .is_some_and(|pending| {
                self.individual_disconnect_peer_clear_done(pending) && self.individual_disconnect_release_ack_done(pending)
            });
        if !ready {
            return;
        }

        let Some(pending) = self.pending_individual_disconnect_release_acks.remove(&call_id) else {
            return;
        };
        if !pending.reporters.iter().all(TxReporter::is_transmitted) {
            tracing::warn!(
                "CMCE: completing individual disconnect call_id={} after D-RELEASE acknowledgement guard for ISSI {} cause={:?}",
                call_id,
                pending.release_to_issi,
                pending.cause
            );
        }
        let Some(call) = self.individual_calls.remove(&call_id) else {
            return;
        };
        self.complete_individual_release_cleanup(queue, call_id, call, pending.cause, Vec::new(), true);
    }

    /// Notify UMAC to open a traffic circuit (ETSI §21 circuit management).
    /// `peer_ts` is Some for local P2P calls on separate assigned timeslots,
    /// including simplex calls where floor control still requires crossed media.
    pub(super) fn signal_umac_circuit_open(
        queue: &mut MessageQueue,
        call: &CmceCircuit,
        peer_ts: Option<u8>,
        dl_media_source: CircuitDlMediaSource,
        active_addr: Option<TetraAddress>,
    ) {
        Self::signal_umac_circuit_open_with_secondary(queue, call, peer_ts, dl_media_source, active_addr, Vec::new());
    }

    pub(super) fn signal_umac_circuit_open_with_secondary(
        queue: &mut MessageQueue,
        call: &CmceCircuit,
        peer_ts: Option<u8>,
        dl_media_source: CircuitDlMediaSource,
        active_addr: Option<TetraAddress>,
        active_secondary_addrs: Vec<TetraAddress>,
    ) {
        let circuit = Circuit {
            direction: call.direction,
            ts: call.ts,
            peer_ts,
            usage: call.usage,
            circuit_mode: call.circuit_mode,
            speech_service: call.speech_service,
            etee_encrypted: call.etee_encrypted,
            dl_media_source,
            active_addr,
            active_secondary_addrs,
        };
        let active_addrs: Vec<_> = circuit.active_addresses().collect();
        tracing::info!(
            "CMCE opening UMAC circuit: direction={:?} ts={} usage={} mode={:?} speech={:?} peer_ts={:?} media_source={:?} active_addrs={:?}",
            circuit.direction,
            circuit.ts,
            circuit.usage,
            circuit.circuit_mode,
            circuit.speech_service,
            circuit.peer_ts,
            circuit.dl_media_source,
            active_addrs
        );
        let cmd = SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::Open(circuit)),
        };
        queue.push_back(cmd);
    }

    pub(super) fn signal_umac_dl_media_source(queue: &mut MessageQueue, ts: u8, dl_media_source: CircuitDlMediaSource) {
        let cmd = SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::SetDlMediaSource { ts, dl_media_source }),
        };
        queue.push_back(cmd);
    }

    pub(super) fn signal_umac_circuit_close(queue: &mut MessageQueue, circuit: CmceCircuit) {
        let cmd = SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::Close(circuit.direction, circuit.ts)),
        };
        queue.push_back(cmd);
    }

    /// Validate U-SETUP PDU for supported features.
    /// ETSI EN 300 392-2 §14.7.1: BS must check service compatibility before accepting.
    pub(super) fn feature_check_u_setup(pdu: &USetup) -> bool {
        let mut supported = true;

        if pdu.area_selection != 0 {
            unimplemented_log!("Area selection not supported: {}", pdu.area_selection);
            supported = false;
        };
        // Both simplex and duplex are supported for P2P calls.
        // Group/broadcast remain simplex only.
        if pdu.basic_service_information.communication_type != CommunicationType::P2p && pdu.simplex_duplex_selection {
            unimplemented_log!(
                "Duplex only supported for P2P calls (comm_type={})",
                pdu.basic_service_information.communication_type
            );
            supported = false;
        };
        if pdu.clir_control != 0 {
            unimplemented_log!("clir_control not supported: {}", pdu.clir_control);
            supported = false;
        };
        if pdu.called_party_ssi.is_none() && pdu.called_party_short_number_address.is_none() && pdu.external_subscriber_number.is_none() {
            unimplemented_log!("U-SETUP called party not set (no SSI, short number or external number)");
        };
        if pdu.called_party_extension.is_some()
            && pdu.called_party_type_identifier != tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier::Tsi
        {
            unimplemented_log!(
                "U-SETUP called_party_extension present with unexpected called_party_type_identifier={}",
                pdu.called_party_type_identifier
            );
        };
        if let Some(v) = &pdu.facility {
            unimplemented_log!("facility not supported: {:?}", v);
            supported = false;
        };
        if let Some(v) = &pdu.dm_ms_address {
            unimplemented_log!("dm_ms_address not supported: {:?}", v);
            supported = false;
        };
        if let Some(v) = &pdu.proprietary {
            unimplemented_log!("proprietary not supported: {:?}", v);
            supported = false;
        };

        supported
    }

    fn basic_service_information_matches(a: &BasicServiceInformation, b: &BasicServiceInformation) -> bool {
        a.circuit_mode_type == b.circuit_mode_type
            && a.encryption_flag == b.encryption_flag
            && a.communication_type == b.communication_type
            && a.slots_per_frame == b.slots_per_frame
            && a.speech_service == b.speech_service
    }

    pub(super) fn unsupported_u_connect_function(
        pdu: &UConnect,
        requested_basic_service: Option<&BasicServiceInformation>,
    ) -> Option<(u8, &'static str)> {
        if let Some(offered) = &pdu.basic_service_information {
            let accepted = requested_basic_service
                .map(|requested| Self::basic_service_information_matches(offered, requested))
                .unwrap_or(false);
            if !accepted {
                return Some((
                    CMCE_FNS_PTR_U_CONNECT_BASIC_SERVICE_INFORMATION,
                    "basic_service_information offer not supported",
                ));
            }
        }
        if pdu.facility.is_some() {
            return Some((CMCE_FNS_PTR_U_CONNECT_TYPE3, "facility not supported"));
        }
        if pdu.proprietary.is_some() {
            return Some((CMCE_FNS_PTR_U_CONNECT_TYPE3, "proprietary not supported"));
        }
        None
    }

    pub(super) fn unsupported_u_alert_function(
        pdu: &UAlert,
        requested_basic_service: Option<&BasicServiceInformation>,
    ) -> Option<(u8, &'static str)> {
        if !pdu.reserved {
            // EN 300 392-2 table 14.21 note: this information element is not
            // used in this edition and its value shall be set to 1.
            return Some((CMCE_FNS_PTR_U_ALERT_RESERVED, "reserved bit clear"));
        }
        if let Some(offered) = &pdu.basic_service_information {
            let accepted = requested_basic_service
                .map(|requested| Self::basic_service_information_matches(offered, requested))
                .unwrap_or(false);
            if !accepted {
                return Some((
                    CMCE_FNS_PTR_U_ALERT_BASIC_SERVICE_INFORMATION,
                    "basic_service_information offer not supported",
                ));
            }
        }
        if pdu.facility.is_some() {
            return Some((CMCE_FNS_PTR_U_ALERT_TYPE3, "facility not supported"));
        }
        if pdu.proprietary.is_some() {
            return Some((CMCE_FNS_PTR_U_ALERT_TYPE3, "proprietary not supported"));
        }
        None
    }

    pub(super) fn unsupported_u_disconnect_function(pdu: &UDisconnect) -> Option<(u8, &'static str)> {
        if pdu.facility.is_some() {
            return Some((CMCE_FNS_PTR_U_DISCONNECT_TYPE3, "facility not supported"));
        }
        if pdu.proprietary.is_some() {
            return Some((CMCE_FNS_PTR_U_DISCONNECT_TYPE3, "proprietary not supported"));
        }
        None
    }

    pub(super) fn unsupported_u_info_function(pdu: &UInfo) -> Option<(u8, &'static str)> {
        if pdu.modify.is_some() {
            return Some((CMCE_FNS_PTR_U_INFO_MODIFY, "modify not supported"));
        }
        if pdu.facility.is_some() {
            return Some((CMCE_FNS_PTR_U_INFO_TYPE3, "facility not supported"));
        }
        if pdu.proprietary.is_some() {
            return Some((CMCE_FNS_PTR_U_INFO_TYPE3, "proprietary not supported"));
        }
        None
    }

    pub(super) fn unsupported_u_release_function(pdu: &URelease) -> Option<(u8, &'static str)> {
        if pdu.facility.is_some() {
            return Some((CMCE_FNS_PTR_U_RELEASE_TYPE3, "facility not supported"));
        }
        if pdu.proprietary.is_some() {
            return Some((CMCE_FNS_PTR_U_RELEASE_TYPE3, "proprietary not supported"));
        }
        None
    }

    pub(super) fn unsupported_u_tx_demand_function(pdu: &UTxDemand) -> Option<(u8, &'static str)> {
        if pdu.encryption_control {
            return Some((
                CMCE_FNS_PTR_U_TX_DEMAND_ENCRYPTION_CONTROL,
                "encrypted transmission demand not supported",
            ));
        }
        if pdu.reserved {
            // EN 300 392-2 table 14.32 note: this reserved information
            // element is not used in this version and shall be set to 0.
            return Some((CMCE_FNS_PTR_U_TX_DEMAND_RESERVED, "reserved bit set"));
        }
        if pdu.facility.is_some() {
            return Some((CMCE_FNS_PTR_U_TX_DEMAND_TYPE3, "facility not supported"));
        }
        if pdu.dm_ms_address.is_some() {
            return Some((CMCE_FNS_PTR_U_TX_DEMAND_TYPE3, "dm_ms_address not supported"));
        }
        if pdu.proprietary.is_some() {
            return Some((CMCE_FNS_PTR_U_TX_DEMAND_TYPE3, "proprietary not supported"));
        }
        None
    }

    /// Map call_timeout_secs from config to the nearest ETSI CallTimeout enum value.
    /// ETSI EN 300 392-2 Table 14.50: BS sets D-SETUP call_time_out to indicate max call duration.
    pub(super) fn config_call_timeout(&self) -> CallTimeout {
        let secs = self.config.config().cell.call_timeout_secs;
        match secs {
            0 => CallTimeout::Infinite, // 0 = no limit
            1..=37 => CallTimeout::T30s,
            38..=52 => CallTimeout::T45s,
            53..=90 => CallTimeout::T60s,
            91..=150 => CallTimeout::T2m,
            151..=210 => CallTimeout::T3m,
            211..=270 => CallTimeout::T4m,
            271..=390 => CallTimeout::T5m,
            391..=540 => CallTimeout::T6m,
            541..=720 => CallTimeout::T8m,
            721..=900 => CallTimeout::T10m,
            901..=1080 => CallTimeout::T12m,
            1081..=1350 => CallTimeout::T15m,
            1351..=1800 => CallTimeout::T20m,
            _ => CallTimeout::T30m,
        }
    }

    /// Reset the local group-call T310 timer after a floor grant.
    pub(super) fn reset_group_t310_after_floor_grant(&mut self, call_id: u16) {
        let call_timeout = self
            .active_calls
            .get_mut(&call_id)
            .map(|call| {
                call.reset_timeout(self.dltime);
                call.call_timeout
            })
            .unwrap_or(CallTimeout::Infinite);

        if call_timeout == CallTimeout::Infinite {
            return;
        }

        // EN 300 392-2 clauses 14.5.2.2.2(c), 14.7.1.8 and 14.8.37:
        // D-INFO with Reset call time-out timer = 1 can restart T310 at the
        // MS, but it is timer signalling, not the transmit authorization from
        // clause 14.5.2.2.1(b). Real Motorola MR5/MR19 field tests showed
        // that injecting timer-only group D-INFO immediately after the
        // requester grant can leave the new over with no valid uplink media.
        // Keep the SwMI's local call timeout fresh and leave the first
        // post-grant assigned-channel FACCH/STCH frames dedicated to floor
        // control and traffic.
        tracing::debug!(
            "CMCE: reset local group T310 after floor grant call_id={} without emitting timer-only D-INFO on FACCH",
            call_id
        );
    }

    /// Send D-TX GRANTED via FACCH stealing on the group traffic channel.
    pub(super) fn send_d_tx_granted_facch(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        source_issi: u32,
        dest_gssi: u32,
        ts: u8,
        usage: u8,
    ) {
        self.send_d_tx_granted_facch_inner(queue, call_id, source_issi, dest_gssi, ts, usage, None);
    }

    pub(super) fn send_d_tx_granted_facch_reported(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        source_issi: u32,
        dest_gssi: u32,
        ts: u8,
        usage: u8,
    ) -> TxReporter {
        let reporter = TxReporter::new_unacked();
        self.send_d_tx_granted_facch_inner(queue, call_id, source_issi, dest_gssi, ts, usage, Some(reporter.clone()));
        reporter
    }

    fn send_d_tx_granted_facch_inner(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        source_issi: u32,
        dest_gssi: u32,
        ts: u8,
        usage: u8,
        reporter: Option<TxReporter>,
    ) {
        // EN 300 392-2 clause 14.5.2.2.1 b) requires group listeners to be
        // informed when another user receives transmit permission. The same
        // clause notes that the group-addressed D-TX GRANTED should identify
        // the transmitting party when needed to prevent the newly granted MS
        // from switching back to U-plane receive after its individual grant.
        // Keep that speaker identity in the GSSI PDU; UMAC may omit the
        // redundant MAC channel-allocation element on the already assigned
        // traffic channel so this still fits FACCH/STCH.
        let pdu = DTxGranted {
            call_identifier: call_id,
            transmission_grant: TransmissionGrant::GrantedToOtherUser.into_raw() as u8,
            transmission_request_permission: false,
            encryption_control: false,
            reserved: false,
            notification_indicator: None,
            transmitting_party_type_identifier: Some(1), // SSI
            transmitting_party_address_ssi: Some(source_issi as u64),
            transmitting_party_extension: None,
            external_subscriber_number: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };

        tracing::debug!("-> D-TX GRANTED (FACCH) {:?}", pdu);
        let mut sdu = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DTxGranted");
        sdu.seek(0);

        let dest_addr = TetraAddress::new(dest_gssi, SsiType::Gssi);
        // DL-only on group FACCH: only the current speaker holds UL.
        let msg = Self::build_sapmsg_stealing_ul_dl_reported_with_repetitions(
            sdu,
            dest_addr,
            ts,
            Some(usage),
            UlDlAssignment::Dl,
            reporter,
            Some(0),
        );
        queue.push_back(msg);
    }

    /// Notify group listeners that another MS has the floor without sending a
    /// GSSI "granted to other user" copy back to the newly granted local MS.
    pub(super) fn send_group_listener_d_tx_granted_facch(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        source_issi: u32,
        dest_gssi: u32,
        ts: u8,
        usage: u8,
    ) {
        let (source_is_local_member, listener_issis): (bool, Vec<u32>) = {
            let state = self.config.state_read();
            (
                state.subscribers.contains_group_member(dest_gssi, source_issi),
                state
                    .subscribers
                    .group_member_issis(dest_gssi)
                    .filter(|issi| *issi != source_issi)
                    .collect(),
            )
        };

        if source_is_local_member && listener_issis.len() <= MAX_LOCAL_LISTENER_INDIVIDUAL_FLOOR_GRANTS {
            if listener_issis.is_empty() {
                tracing::debug!(
                    "CMCE: no local group listener floor notification needed for GSSI {} source ISSI {}",
                    dest_gssi,
                    source_issi
                );
                return;
            }

            // EN 300 392-2 clause 14.5.2.2.1 requires the SwMI to announce the
            // floor state. For local group speakers, avoid a GSSI
            // GrantedToOtherUser PDU that the speaker itself also receives and
            // may interpret as receive-only. Individual listener copies carry
            // the same mandatory transmission-grant IE while excluding the
            // granted speaker from this listener-only notification.
            for listener_issi in listener_issis {
                self.fsm_send_d_tx_granted_individual(
                    queue,
                    call_id,
                    TetraAddress::new(listener_issi, SsiType::Issi),
                    ts,
                    usage,
                    TransmissionGrant::GrantedToOtherUser,
                    Some(source_issi),
                );
            }
            return;
        }

        if source_is_local_member {
            tracing::debug!(
                "CMCE: using GSSI listener floor notification for GSSI {} with {} local listener(s), above individual fanout cap {}",
                dest_gssi,
                listener_issis.len(),
                MAX_LOCAL_LISTENER_INDIVIDUAL_FLOOR_GRANTS
            );
        }
        self.send_d_tx_granted_facch(queue, call_id, source_issi, dest_gssi, ts, usage);
    }

    /// Send D-TX INTERRUPT via FACCH stealing on the group traffic channel.
    pub(super) fn send_d_tx_interrupt_facch(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        target_addr: TetraAddress,
        source_issi: u32,
        ts: u8,
        usage: u8,
        transmission_grant: TransmissionGrant,
    ) {
        let pdu = DTxInterrupt {
            call_identifier: call_id,
            transmission_grant: transmission_grant.into_raw() as u8,
            transmission_request_permission: false,
            encryption_control: false,
            reserved: false,
            notification_indicator: None,
            transmitting_party_type_identifier: Some(1), // SSI
            transmitting_party_address_ssi: Some(source_issi as u64),
            transmitting_party_extension: None,
            external_subscriber_number: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };

        tracing::debug!("-> D-TX INTERRUPT (FACCH) {:?}", pdu);
        let mut sdu = BitBuffer::new_autoexpand(30);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DTxInterrupt");
        sdu.seek(0);

        // DL-only on group FACCH: the interrupted MS/listeners must not retain UL permission.
        // Keep the active traffic usage marker so the STCH MAC-RESOURCE is
        // still tied to the assigned channel carrying the call.
        let msg = Self::build_sapmsg_stealing_ul_dl_with_repetitions(sdu, target_addr, ts, Some(usage), UlDlAssignment::Dl, Some(0));
        queue.push_back(msg);
    }

    /// Send D-TX CEASED via FACCH stealing on the group traffic channel.
    pub(super) fn send_d_tx_ceased_facch(&mut self, queue: &mut MessageQueue, call_id: u16, dest_gssi: u32, ts: u8, usage: u8) {
        let pdu = DTxCeased {
            call_identifier: call_id,
            transmission_request_permission: false, // ETSI 14.8.43: 0 = allowed to request transmission
            notification_indicator: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };

        tracing::debug!("-> D-TX CEASED (FACCH) {:?}", pdu);
        let mut sdu = BitBuffer::new_autoexpand(30);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DTxCeased");
        sdu.seek(0);

        let dest_addr = TetraAddress::new(dest_gssi, SsiType::Gssi);
        // DL-only on group FACCH: signalling to all members, no UL expected here.
        let msg = Self::build_sapmsg_stealing_ul_dl_with_repetitions(sdu, dest_addr, ts, Some(usage), UlDlAssignment::Dl, Some(0));
        queue.push_back(msg);
    }

    fn build_group_release_sdu(&self, call_id: u16, disconnect_cause: DisconnectCause) -> BitBuffer {
        if let Some(cached) = self.cached_setups.get(&call_id) {
            Self::build_d_release_from_d_setup(&cached.pdu, disconnect_cause)
        } else {
            tracing::warn!("CMCE: no cached D-SETUP for group release call_id={}, using call id only", call_id);
            Self::build_d_release(call_id, disconnect_cause)
        }
    }

    fn send_group_release_pdus(
        &self,
        queue: &mut MessageQueue,
        call_id: u16,
        dest_addr: TetraAddress,
        ts: u8,
        usage: u8,
        disconnect_cause: DisconnectCause,
        reporter: TxReporter,
    ) {
        let facch_sdu = self.build_group_release_sdu(call_id, disconnect_cause);
        let facch = Self::build_sapmsg_stealing_ul_dl_reported(facch_sdu, dest_addr, ts, Some(usage), UlDlAssignment::Dl, Some(reporter));
        queue.push_back(facch);

        let mcch_sdu = self.build_group_release_sdu(call_id, disconnect_cause);
        let mcch = Self::build_sapmsg(mcch_sdu, None, dest_addr, Layer2Service::Unacknowledged, None);
        queue.push_back(mcch);
    }

    /// Begin a group release. ETSI EN 300 392-2 §14.5.2.3 releases a group call with
    /// D-RELEASE; keep the assigned circuit open until the FACCH/STCH delivery is reported.
    pub(super) fn release_group_call(&mut self, queue: &mut MessageQueue, call_id: u16, disconnect_cause: DisconnectCause) {
        self.begin_group_release(queue, call_id, disconnect_cause, None);
    }

    pub(super) fn begin_group_release(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        disconnect_cause: DisconnectCause,
        preclosed_circuit: Option<CmceCircuit>,
    ) {
        self.cancel_group_tx_ceased_tail_drain(call_id, "group release started");
        self.cancel_group_floor_activation(call_id, "group release started");
        self.cancel_network_group_ready(call_id, "group release started");

        if self.pending_group_releases.contains_key(&call_id) {
            tracing::debug!("CMCE: group release already pending for call_id={}", call_id);
            return;
        }

        let Some(call) = self.active_calls.get(&call_id).cloned() else {
            tracing::warn!("CMCE: group release for unknown call_id={}", call_id);
            return;
        };

        let dest_addr = self
            .cached_setups
            .get(&call_id)
            .map(|cached| cached.dest_addr)
            .unwrap_or_else(|| TetraAddress::new(call.dest_gssi, SsiType::Gssi));
        let is_local = matches!(call.origin, CallOrigin::Local { .. });
        let network_brew_uuid = if let CallOrigin::Network { brew_uuid } = call.origin {
            Some(brew_uuid)
        } else {
            None
        };
        let reporter = TxReporter::new_unacked();

        self.send_group_release_pdus(queue, call_id, dest_addr, call.ts, call.usage, disconnect_cause, reporter.clone());

        self.pending_group_releases.insert(
            call_id,
            PendingGroupRelease {
                call_id,
                dest_gssi: call.dest_gssi,
                ts: call.ts,
                cause: disconnect_cause,
                reporter,
                started_at: self.dltime,
                preclosed_circuit,
                is_local,
                network_brew_uuid,
            },
        );
    }

    pub(super) fn drain_pending_group_releases(&mut self, queue: &mut MessageQueue) {
        let ready: Vec<u16> = self
            .pending_group_releases
            .iter()
            .filter_map(|(&call_id, pending)| {
                if pending.reporter.is_transmitted() || pending.started_at.age(self.dltime) >= GROUP_RELEASE_PENDING_TIMEOUT_TIMESLOTS {
                    Some(call_id)
                } else {
                    None
                }
            })
            .collect();

        for call_id in ready {
            self.complete_pending_group_release(queue, call_id);
        }
    }

    fn complete_pending_group_release(&mut self, queue: &mut MessageQueue, call_id: u16) {
        let Some(pending) = self.pending_group_releases.remove(&call_id) else {
            return;
        };
        let timed_out = !pending.reporter.is_transmitted();
        if timed_out {
            tracing::warn!(
                "CMCE: completing group release call_id={} after local pending timeout cause={:?}",
                pending.call_id,
                pending.cause
            );
        }

        if let Some(circuit) = pending.preclosed_circuit {
            Self::signal_umac_circuit_close(queue, circuit);
        } else if let Ok(circuit) = self.circuits.close_circuit(Direction::Both, pending.ts) {
            Self::signal_umac_circuit_close(queue, circuit);
        }

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::CallEnded {
                call_id: pending.call_id,
                ts: pending.ts,
            }),
        });

        self.release_timeslot(pending.ts);

        if net_brew::is_brew_gssi_routable(&self.config, pending.dest_gssi) {
            if pending.is_local {
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Brew,
                    msg: SapMsgInner::CmceCallControl(CallControl::CallEnded {
                        call_id: pending.call_id,
                        ts: pending.ts,
                    }),
                });
            } else if let Some(brew_uuid) = pending.network_brew_uuid {
                tracing::info!(
                    "complete_pending_group_release: notifying Brew of ended network call_id={} brew_uuid={} cause={:?}",
                    pending.call_id,
                    brew_uuid,
                    pending.cause
                );
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Brew,
                    msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
                });
            }
        }

        self.cached_setups.remove(&pending.call_id);
        self.active_calls.remove(&pending.call_id);
        self.emit(crate::net_telemetry::TelemetryEvent::GroupCallEnded {
            call_id: pending.call_id,
            gssi: pending.dest_gssi,
        });
    }

    fn build_individual_release_sdu(&self, call_id: u16, disconnect_cause: DisconnectCause) -> BitBuffer {
        self.build_individual_release_sdu_with_notification(call_id, disconnect_cause, None)
    }

    fn build_individual_release_sdu_with_notification(
        &self,
        call_id: u16,
        disconnect_cause: DisconnectCause,
        notification_indicator: Option<u64>,
    ) -> BitBuffer {
        if let Some(cached) = self.cached_setups.get(&call_id) {
            if notification_indicator.is_some() {
                Self::build_d_release_with_notification(cached.pdu.call_identifier, disconnect_cause, notification_indicator)
            } else {
                Self::build_d_release_from_d_setup(&cached.pdu, disconnect_cause)
            }
        } else {
            Self::build_d_release_with_notification(call_id, disconnect_cause, notification_indicator)
        }
    }

    pub(super) fn send_pending_individual_release_direct_ack(
        &self,
        queue: &mut MessageQueue,
        call_id: u16,
        target: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        uplink_label: &str,
    ) -> bool {
        let Some(pending) = self.pending_individual_releases.get(&call_id) else {
            return false;
        };

        tracing::debug!(
            "CMCE: {} absorbed for private call_id={} from ISSI {} while D-RELEASE is pending; repeating direct D-RELEASE cause={:?}",
            uplink_label,
            call_id,
            target.ssi,
            pending.cause
        );
        let sdu = self.build_individual_release_sdu(call_id, pending.cause);
        queue.push_back(Self::build_sapmsg_direct(sdu, target, handle, link_id, endpoint_id));
        true
    }

    fn send_direct_individual_release_pdu(
        &self,
        queue: &mut MessageQueue,
        call_id: u16,
        call: &IndividualCall,
        disconnect_cause: DisconnectCause,
        target_issi: u32,
    ) {
        let sdu = self.build_individual_release_sdu(call_id, disconnect_cause);
        if target_issi == call.calling_addr.ssi {
            queue.push_back(Self::build_sapmsg_direct(
                sdu,
                call.calling_addr,
                call.calling_handle,
                call.calling_link_id,
                call.calling_endpoint_id,
            ));
        } else if target_issi == call.called_addr.ssi {
            if let (Some(handle), Some(link_id), Some(endpoint_id)) = (call.called_handle, call.called_link_id, call.called_endpoint_id) {
                queue.push_back(Self::build_sapmsg_direct(sdu, call.called_addr, handle, link_id, endpoint_id));
            } else {
                queue.push_back(Self::build_sapmsg(sdu, None, call.called_addr, Layer2Service::Unacknowledged, None));
            }
        }
    }

    fn send_connecting_individual_release_pdus(
        &self,
        queue: &mut MessageQueue,
        call_id: u16,
        call: &IndividualCall,
        disconnect_cause: DisconnectCause,
        release_to_issi: Option<u32>,
    ) -> Vec<TxReporter> {
        let mut reporters = Vec::new();

        let send_calling_leg = !call.calling_over_brew && release_to_issi.map_or(true, |issi| issi == call.calling_addr.ssi);
        let send_called_leg = !call.called_over_brew && release_to_issi.map_or(true, |issi| issi == call.called_addr.ssi);

        if send_calling_leg {
            // The caller leg is not active until caller D-CONNECT is L2-ACKed.
            // Release it on the original signalling context so a caller that
            // rejected the call id does not need to have moved to traffic.
            self.send_direct_individual_release_pdu(queue, call_id, call, disconnect_cause, call.calling_addr.ssi);
        }

        if send_called_leg {
            if matches!(call.state, IndividualCallState::CallerConnectAckPending) {
                let reporter = TxReporter::new_unacked();
                let facch = Self::build_sapmsg_stealing_ul_dl_reported(
                    self.build_individual_release_sdu(call_id, disconnect_cause),
                    call.called_addr,
                    call.called_ts,
                    Some(call.called_usage),
                    UlDlAssignment::Dl,
                    Some(reporter.clone()),
                );
                queue.push_back(facch);
                reporters.push(reporter);
            } else {
                // Called D-CONNECT ACK has not been L2-ACKed yet. Keep the
                // rejection/no-answer/invalid cleanup on the called MS's
                // current signalling context.
                self.send_direct_individual_release_pdu(queue, call_id, call, disconnect_cause, call.called_addr.ssi);
            }
        }

        reporters
    }

    fn send_established_individual_release_pdus(
        &self,
        queue: &mut MessageQueue,
        call_id: u16,
        call: &IndividualCall,
        disconnect_cause: DisconnectCause,
        release_to_issi: Option<u32>,
    ) -> Vec<TxReporter> {
        self.send_established_individual_release_pdus_with_notice(queue, call_id, call, disconnect_cause, release_to_issi, None)
    }

    fn send_established_individual_release_pdus_with_notice(
        &self,
        queue: &mut MessageQueue,
        call_id: u16,
        call: &IndividualCall,
        disconnect_cause: DisconnectCause,
        release_to_issi: Option<u32>,
        imminent_notice_to_issi: Option<u32>,
    ) -> Vec<TxReporter> {
        let mut reporters = Vec::new();

        let send_calling_leg = !call.calling_over_brew && release_to_issi.map_or(true, |issi| issi == call.calling_addr.ssi);
        let send_called_leg = !call.called_over_brew && release_to_issi.map_or(true, |issi| issi == call.called_addr.ssi);

        if send_calling_leg {
            let reporter = TxReporter::new_unacked();
            // Established individual calls already have an assigned channel;
            // keep release on FACCH/STCH with a reporter so the same MS does
            // not receive duplicate D-RELEASE copies on MCCH.
            let notification = if imminent_notice_to_issi == Some(call.calling_addr.ssi) {
                Self::push_established_individual_release_imminent_notice(
                    queue,
                    call_id,
                    call.calling_addr,
                    call.calling_ts,
                    call.calling_usage,
                );
                Some(NOTIFICATION_IMMINENT_CALL_DISCONNECTION)
            } else {
                None
            };
            let facch = Self::build_sapmsg_stealing_ul_dl_reported(
                self.build_individual_release_sdu_with_notification(call_id, disconnect_cause, notification),
                call.calling_addr,
                call.calling_ts,
                Some(call.calling_usage),
                UlDlAssignment::Dl,
                Some(reporter.clone()),
            );
            queue.push_back(facch);
            reporters.push(reporter);
        }

        if send_called_leg {
            let reporter = TxReporter::new_unacked();
            // EN 300 392-2 clause 14.5.1.3.1 permits clearing the peer with
            // D-RELEASE; in an established call it belongs on the assigned
            // channel rather than as an additional MCCH fallback duplicate.
            let notification = if imminent_notice_to_issi == Some(call.called_addr.ssi) {
                Self::push_established_individual_release_imminent_notice(
                    queue,
                    call_id,
                    call.called_addr,
                    call.called_ts,
                    call.called_usage,
                );
                Some(NOTIFICATION_IMMINENT_CALL_DISCONNECTION)
            } else {
                None
            };
            let facch = Self::build_sapmsg_stealing_ul_dl_reported(
                self.build_individual_release_sdu_with_notification(call_id, disconnect_cause, notification),
                call.called_addr,
                call.called_ts,
                Some(call.called_usage),
                UlDlAssignment::Dl,
                Some(reporter.clone()),
            );
            queue.push_back(facch);
            reporters.push(reporter);
        }

        reporters
    }

    fn complete_individual_release_cleanup(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        call: IndividualCall,
        disconnect_cause: DisconnectCause,
        preclosed_circuits: Vec<CmceCircuit>,
        notify_brew: bool,
    ) {
        let mut ts_list = vec![call.calling_ts];
        if call.called_ts != call.calling_ts {
            ts_list.push(call.called_ts);
        }
        for ts in ts_list {
            if let Some(circuit) = preclosed_circuits.iter().find(|circuit| circuit.ts == ts).cloned() {
                Self::signal_umac_circuit_close(queue, circuit);
            } else if let Ok(circuit) = self.circuits.close_circuit(Direction::Both, ts) {
                Self::signal_umac_circuit_close(queue, circuit);
            }

            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::CmceCallControl(CallControl::CallEnded { call_id, ts }),
            });

            self.release_timeslot(ts);
        }
        self.cached_setups.remove(&call_id);
        self.individual_calls.remove(&call_id);
        self.pending_individual_disconnect_tail_drains.remove(&call_id);
        self.pending_individual_disconnect_deliveries.remove(&call_id);
        self.pending_individual_disconnect_release_acks.remove(&call_id);
        self.pending_individual_tx_ceased_tail_drains.remove(&call_id);
        self.pending_individual_connect_acks.remove(&call_id);
        self.pending_network_individual_connects.remove(&call_id);

        if notify_brew
            && (call.called_over_brew || call.calling_over_brew)
            && disconnect_cause != DisconnectCause::SwmiRequestedDisconnection
        {
            if let Some(brew_uuid) = call.brew_uuid {
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Brew,
                    msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease {
                        brew_uuid,
                        cause: disconnect_cause.into_raw() as u8,
                    }),
                });
            }
        }
        self.emit(crate::net_telemetry::TelemetryEvent::IndividualCallEnded { call_id });
        self.release_echo_session_if_matches(call_id);
    }

    pub(super) fn drain_pending_individual_releases(&mut self, queue: &mut MessageQueue) {
        let ready: Vec<u16> = self
            .pending_individual_releases
            .iter()
            .filter_map(|(&call_id, pending)| {
                let all_reporters_transmitted = pending.reporters.iter().all(TxReporter::is_transmitted);
                if all_reporters_transmitted || pending.started_at.age(self.dltime) >= INDIVIDUAL_RELEASE_PENDING_TIMEOUT_TIMESLOTS {
                    Some(call_id)
                } else {
                    None
                }
            })
            .collect();

        for call_id in ready {
            self.complete_pending_individual_release(queue, call_id);
        }
    }

    fn complete_pending_individual_release(&mut self, queue: &mut MessageQueue, call_id: u16) {
        let Some(pending) = self.pending_individual_releases.remove(&call_id) else {
            return;
        };
        if !pending.reporters.iter().all(TxReporter::is_transmitted) {
            tracing::warn!(
                "CMCE: completing individual release call_id={} after local pending timeout cause={:?}",
                pending.call_id,
                pending.cause
            );
        }
        self.complete_individual_release_cleanup(
            queue,
            pending.call_id,
            pending.call,
            pending.cause,
            pending.preclosed_circuits,
            pending.notify_brew,
        );
    }

    /// Release an individual call: send D-RELEASE to both parties, close circuits, clean up state.
    /// Handles active assigned-channel delivery with a reporter drain and setup-phase MCCH delivery.
    pub(super) fn release_individual_call(&mut self, queue: &mut MessageQueue, call_id: u16, disconnect_cause: DisconnectCause) {
        self.begin_individual_release(queue, call_id, disconnect_cause, Vec::new(), true, None);
    }

    pub(super) fn release_individual_call_without_brew_echo(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        disconnect_cause: DisconnectCause,
    ) {
        self.begin_individual_release(queue, call_id, disconnect_cause, Vec::new(), false, None);
    }

    pub(super) fn release_individual_call_to_issi(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        disconnect_cause: DisconnectCause,
        release_to_issi: u32,
    ) {
        self.begin_individual_release(queue, call_id, disconnect_cause, Vec::new(), true, Some(release_to_issi));
    }

    pub(super) fn begin_individual_release(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        disconnect_cause: DisconnectCause,
        preclosed_circuits: Vec<CmceCircuit>,
        notify_brew: bool,
        release_to_issi: Option<u32>,
    ) {
        if self.pending_individual_releases.contains_key(&call_id) {
            tracing::debug!("CMCE: individual release already pending for call_id={}", call_id);
            return;
        }
        self.pending_individual_disconnect_tail_drains.remove(&call_id);
        self.pending_individual_disconnect_deliveries.remove(&call_id);
        self.pending_individual_disconnect_release_acks.remove(&call_id);
        self.pending_individual_tx_ceased_tail_drains.remove(&call_id);
        self.pending_individual_connect_acks.remove(&call_id);
        self.pending_network_individual_connects.remove(&call_id);

        let Some(call_snapshot) = self.individual_calls.remove(&call_id) else {
            tracing::warn!("No individual call for call_id={}", call_id);
            return;
        };

        let send_calling_leg =
            !call_snapshot.calling_over_brew && release_to_issi.map_or(true, |issi| issi == call_snapshot.calling_addr.ssi);
        let send_called_leg = !call_snapshot.called_over_brew && release_to_issi.map_or(true, |issi| issi == call_snapshot.called_addr.ssi);

        const SETUP_RELEASE_REPEATS: usize = 3;

        if call_snapshot.has_assigned_circuit() {
            // EN 300 392-2 14.5.1.3.2 says the SwMI sends D-RELEASE to both
            // MSs and subsequently releases the call. Assigned-mode MSs receive
            // CC signalling on the assigned channel, so keep the circuit open
            // until FACCH/STCH D-RELEASE transmission is reported or a local
            // guard timeout expires.
            let reporters = if call_snapshot.is_connect_ack_pending() {
                self.send_connecting_individual_release_pdus(queue, call_id, &call_snapshot, disconnect_cause, release_to_issi)
            } else {
                self.send_established_individual_release_pdus(queue, call_id, &call_snapshot, disconnect_cause, release_to_issi)
            };
            self.pending_individual_releases.insert(
                call_id,
                PendingIndividualRelease {
                    call_id,
                    call: call_snapshot,
                    cause: disconnect_cause,
                    reporters,
                    started_at: self.dltime,
                    preclosed_circuits,
                    notify_brew,
                },
            );
            if self
                .pending_individual_releases
                .get(&call_id)
                .is_some_and(|pending| pending.reporters.is_empty())
            {
                self.complete_pending_individual_release(queue, call_id);
            }
            return;
        } else {
            // During setup: deliver on MCCH (force link_id=0, both parties monitor MCCH).
            for _ in 0..SETUP_RELEASE_REPEATS {
                let sdu_calling = self.build_individual_release_sdu(call_id, disconnect_cause);
                let sdu_called = self.build_individual_release_sdu(call_id, disconnect_cause);
                if send_calling_leg {
                    let prim_calling =
                        Self::build_sapmsg(sdu_calling, None, call_snapshot.calling_addr, Layer2Service::Unacknowledged, None);
                    queue.push_back(prim_calling);
                }

                if send_called_leg {
                    let prim_called = Self::build_sapmsg(sdu_called, None, call_snapshot.called_addr, Layer2Service::Unacknowledged, None);
                    queue.push_back(prim_called);
                }
            }

            self.complete_individual_release_cleanup(queue, call_id, call_snapshot, disconnect_cause, preclosed_circuits, notify_brew);
        }
    }

    pub(super) fn release_timeslot(&mut self, ts: u8) {
        let mut state = self.config.state_write();
        if let Err(err) = state.timeslot_alloc.release(TimeslotOwner::Cmce, ts) {
            tracing::warn!("CcBsSubentity: failed to release timeslot ts={} err={:?}", ts, err);
        }
    }

    // ── Dashboard / API helpers ────────────────────────────────────────────────

    /// Returns all currently registered ISSI values.
    pub fn subscriber_issis(&self) -> Vec<u32> {
        self.subscriber_groups.keys().copied().collect()
    }

    /// Returns the list of GSSIs the given ISSI is affiliated to.
    pub fn subscriber_groups_for(&self, issi: u32) -> Vec<u32> {
        self.subscriber_groups
            .get(&issi)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Force-deregister an MS: release its active calls and clean up state.
    /// Returns true if the MS was known.
    pub fn kick_ms(&mut self, queue: &mut MessageQueue, issi: u32) -> bool {
        if !self.subscriber_groups.contains_key(&issi) {
            tracing::warn!("CMCE: kick_ms issi={} not found in subscriber_groups", issi);
            return false;
        }
        // Release all active individual calls involving this MS
        let individual_ids: Vec<u16> = self
            .individual_calls
            .iter()
            .filter(|(_, c)| c.calling_addr.ssi == issi || c.called_addr.ssi == issi)
            .map(|(&id, _)| id)
            .collect();
        for id in individual_ids {
            self.release_individual_call(queue, id, DisconnectCause::UserRequestedDisconnection);
        }
        // Clean up CMCE state
        if let Some(groups) = self.subscriber_groups.remove(&issi) {
            for g in &groups {
                self.dec_group_listener(*g);
            }
        }
        // Tell MM to deregister the MS — this also notifies Brew
        use tetra_core::Sap;
        use tetra_saps::SapMsgInner;
        use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
        queue.push_back(tetra_saps::SapMsg {
            sap: Sap::Control,
            src: tetra_core::tetra_entities::TetraEntity::Cmce,
            dest: tetra_core::tetra_entities::TetraEntity::Mm,
            msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
                issi,
                groups: Vec::new(),
                action: BrewSubscriberAction::Deregister,
            }),
        });
        tracing::info!("CMCE: kick_ms issi={} — deregistered", issi);
        true
    }

    /// Snapshot of active group calls for the dashboard.
    pub fn active_calls_snapshot(&self) -> Vec<(u16, u32, u32, bool)> {
        self.active_calls
            .iter()
            .map(|(&id, c)| {
                let caller = match &c.origin {
                    crate::cmce::subentities::cc_bs::call::CallOrigin::Local { caller_addr } => caller_addr.ssi,
                    _ => 0,
                };
                (id, c.dest_gssi, caller, c.tx_active)
            })
            .collect()
    }

    /// Snapshot of active individual calls for the dashboard.
    pub fn individual_calls_snapshot(&self) -> Vec<(u16, u32, u32, bool)> {
        self.individual_calls
            .iter()
            .map(|(&id, c)| (id, c.calling_addr.ssi, c.called_addr.ssi, !c.simplex_duplex))
            .collect()
    }

    /// Find the active call_id occupying the given timeslot, group or individual.
    /// Returns None if the timeslot is idle. Used by the recording manager.
    pub fn call_id_for_ts(&self, ts: u8) -> Option<u16> {
        if let Some((&id, _)) = self.active_calls.iter().find(|(_, c)| c.ts == ts) {
            return Some(id);
        }
        if let Some((&id, _)) = self
            .individual_calls
            .iter()
            .find(|(_, c)| c.has_assigned_circuit() && (c.calling_ts == ts || c.called_ts == ts))
        {
            return Some(id);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::CcBsSubentity;

    #[test]
    fn external_subscriber_number_supports_16_digits() {
        let number = "1234567890123456";
        let field = CcBsSubentity::encode_external_subscriber_number(number).expect("field should be generated");
        assert_eq!(field.len, 64);
        assert_eq!(CcBsSubentity::decode_external_subscriber_number(&field), number);
    }

    #[test]
    fn external_subscriber_number_supports_24_digits() {
        // ETSI EN 300 392-2 §14.8.21 max is 24 digits.
        let number = "123456789012345678901234";
        let field = CcBsSubentity::encode_external_subscriber_number(number).expect("field should be generated");
        assert_eq!(field.len, 96);
        assert_eq!(CcBsSubentity::decode_external_subscriber_number(&field), number);
    }

    #[test]
    fn external_subscriber_number_supports_32_digits() {
        // We support up to 32 (128 bits) — above the ETSI max for safety margin.
        let number = "12345678901234567890123456789012";
        let field = CcBsSubentity::encode_external_subscriber_number(number).expect("field should be generated");
        assert_eq!(field.len, 128);
        assert_eq!(CcBsSubentity::decode_external_subscriber_number(&field), number);
    }

    #[test]
    fn external_subscriber_number_truncates_above_32_digits() {
        let number = "123456789012345678901234567890123"; // 33 digits
        let field = CcBsSubentity::encode_external_subscriber_number(number).expect("field should be generated");
        assert_eq!(field.len, 128);
        assert_eq!(
            CcBsSubentity::decode_external_subscriber_number(&field),
            "12345678901234567890123456789012"
        );
    }
}
