use super::*;

// TETRA TDMA timing: one slot is 170/12 milliseconds.
const TIMESLOT_DURATION_MS: f64 = 170.0 / 12.0;
const MAX_GROUP_FLOOR_WAITERS: usize = 4096;

#[inline]
fn seconds_to_timeslots(seconds: i32) -> i32 {
    debug_assert!(seconds >= 0);
    (f64::from(seconds) * 1_000.0 / TIMESLOT_DURATION_MS) as i32
}

#[inline]
fn setup_timeout_to_timeslots(timeout: CallTimeoutSetupPhase) -> Option<i32> {
    match timeout {
        CallTimeoutSetupPhase::Predefined => Some(seconds_to_timeslots(10)),
        CallTimeoutSetupPhase::T1s => Some(seconds_to_timeslots(1)),
        CallTimeoutSetupPhase::T2s => Some(seconds_to_timeslots(2)),
        CallTimeoutSetupPhase::T5s => Some(seconds_to_timeslots(5)),
        CallTimeoutSetupPhase::T10s => Some(seconds_to_timeslots(10)),
        CallTimeoutSetupPhase::T20s => Some(seconds_to_timeslots(20)),
        CallTimeoutSetupPhase::T30s => Some(seconds_to_timeslots(30)),
        CallTimeoutSetupPhase::T60s => Some(seconds_to_timeslots(60)),
    }
}

#[inline]
pub(super) fn call_timeout_to_timeslots(timeout: CallTimeout) -> Option<i32> {
    match timeout {
        CallTimeout::Infinite | CallTimeout::Reserved => None,
        CallTimeout::T30s => Some(seconds_to_timeslots(30)),
        CallTimeout::T45s => Some(seconds_to_timeslots(45)),
        CallTimeout::T60s => Some(seconds_to_timeslots(60)),
        CallTimeout::T2m => Some(seconds_to_timeslots(120)),
        CallTimeout::T3m => Some(seconds_to_timeslots(180)),
        CallTimeout::T4m => Some(seconds_to_timeslots(240)),
        CallTimeout::T5m => Some(seconds_to_timeslots(300)),
        CallTimeout::T6m => Some(seconds_to_timeslots(360)),
        CallTimeout::T8m => Some(seconds_to_timeslots(480)),
        CallTimeout::T10m => Some(seconds_to_timeslots(600)),
        CallTimeout::T12m => Some(seconds_to_timeslots(720)),
        CallTimeout::T15m => Some(seconds_to_timeslots(900)),
        CallTimeout::T20m => Some(seconds_to_timeslots(1200)),
        CallTimeout::T30m => Some(seconds_to_timeslots(1800)),
    }
}

/// Origin of a group call
#[derive(Clone)]
pub(super) enum CallOrigin {
    /// Local MS-initiated call
    Local { caller_addr: TetraAddress },
    /// Network-initiated call from TetraPack/Brew
    Network { brew_uuid: uuid::Uuid },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum GroupCallState {
    /// An active speaker is currently transmitting.
    Transmitting,
    /// No active speaker; call is still alive during hangtime.
    NoActiveSpeaker { since: TdmaTime },
}

/// Tracks an active group call (local or network-initiated)
#[derive(Clone)]
pub(super) struct ActiveCall {
    pub(super) origin: CallOrigin,
    pub(super) dest_gssi: u32,
    pub(super) source_issi: u32,
    pub(super) created_at: TdmaTime,
    pub(super) call_timeout: CallTimeout,
    pub(super) priority: u8,
    pub(super) ts: u8,
    pub(super) usage: u8,
    pub(super) tx_active: bool,
    pub(super) hangtime_start: Option<TdmaTime>,
    queued_floor_demands: VecDeque<TetraAddress>,
    queued_floor_demand_ssis: HashSet<u32>,
    pub(super) brew_uuid: Option<uuid::Uuid>,
}

impl ActiveCall {
    pub(super) fn new_local(
        caller_addr: TetraAddress,
        dest_gssi: u32,
        source_issi: u32,
        ts: u8,
        usage: u8,
        created_at: TdmaTime,
        call_timeout: CallTimeout,
        priority: u8,
    ) -> Self {
        Self {
            origin: CallOrigin::Local { caller_addr },
            dest_gssi,
            source_issi,
            created_at,
            call_timeout,
            priority,
            ts,
            usage,
            tx_active: true,
            hangtime_start: None,
            queued_floor_demands: VecDeque::new(),
            queued_floor_demand_ssis: HashSet::new(),
            brew_uuid: None,
        }
    }

    pub(super) fn new_network(
        brew_uuid: uuid::Uuid,
        dest_gssi: u32,
        source_issi: u32,
        ts: u8,
        usage: u8,
        created_at: TdmaTime,
        call_timeout: CallTimeout,
        priority: u8,
    ) -> Self {
        Self {
            origin: CallOrigin::Network { brew_uuid },
            dest_gssi,
            source_issi,
            created_at,
            call_timeout,
            priority,
            ts,
            usage,
            tx_active: true,
            hangtime_start: None,
            queued_floor_demands: VecDeque::new(),
            queued_floor_demand_ssis: HashSet::new(),
            brew_uuid: Some(brew_uuid),
        }
    }

    #[inline]
    pub(super) fn state(&self) -> GroupCallState {
        if self.tx_active {
            GroupCallState::Transmitting
        } else {
            GroupCallState::NoActiveSpeaker {
                since: self.hangtime_start.unwrap_or_default(),
            }
        }
    }

    #[inline]
    pub(super) fn is_tx_active(&self) -> bool {
        matches!(self.state(), GroupCallState::Transmitting)
    }

    #[inline]
    pub(super) fn is_current_speaker(&self, issi: u32) -> bool {
        self.tx_active && self.source_issi == issi
    }

    #[inline]
    pub(super) fn call_timeout_expired(&self, now: TdmaTime) -> bool {
        match call_timeout_to_timeslots(self.call_timeout) {
            Some(timeout) => self.created_at.age(now) > timeout,
            None => false,
        }
    }

    pub(super) fn enter_hangtime(&mut self, now: TdmaTime) {
        self.tx_active = false;
        self.hangtime_start = Some(now);
    }

    /// Reset the call timeout clock. Called when a new network speaker takes the floor so that
    /// the 120s (T2m) window is measured from the latest transmission, not from call creation.
    /// Without this, a conversation with multiple back-to-back speakers always expires at
    /// `created_at + timeout` regardless of how recently the last speaker started talking.
    pub(super) fn reset_timeout(&mut self, now: TdmaTime) {
        self.created_at = now;
    }

    pub(super) fn grant_floor(&mut self, source_issi: u32, _speaker_addr: Option<TetraAddress>) {
        self.source_issi = source_issi;
        self.tx_active = true;
        self.hangtime_start = None;
    }

    pub(super) fn queue_tx_demand(&mut self, requester: TetraAddress) -> TxDemandQueueResult {
        if self.is_current_speaker(requester.ssi) {
            return TxDemandQueueResult::FromCurrentSpeaker;
        }

        if self.queued_floor_demand_ssis.contains(&requester.ssi) {
            return TxDemandQueueResult::AlreadyQueuedBySameUser;
        }

        if self.queued_floor_demands.len() >= MAX_GROUP_FLOOR_WAITERS {
            return TxDemandQueueResult::QueueBusy;
        }

        self.queued_floor_demands.push_back(requester);
        self.queued_floor_demand_ssis.insert(requester.ssi);
        TxDemandQueueResult::Queued
    }

    pub(super) fn take_queued_tx_demand(&mut self) -> Option<TetraAddress> {
        let requester = self.queued_floor_demands.pop_front()?;
        self.queued_floor_demand_ssis.remove(&requester.ssi);
        Some(requester)
    }

    pub(super) fn queued_tx_demands(&self) -> impl Iterator<Item = TetraAddress> + '_ {
        self.queued_floor_demands.iter().copied()
    }

    pub(super) fn take_queued_tx_demand_through(&mut self, target_issi: Option<u32>) -> Option<TetraAddress> {
        while let Some(requester) = self.queued_floor_demands.pop_front() {
            self.queued_floor_demand_ssis.remove(&requester.ssi);
            if Some(requester.ssi) == target_issi {
                return Some(requester);
            }
        }
        None
    }

    pub(super) fn clear_queued_tx_demand_from(&mut self, issi: u32) -> bool {
        let removed = self.queued_floor_demand_ssis.remove(&issi);
        if !removed {
            return false;
        }
        self.queued_floor_demands.retain(|requester| requester.ssi != issi);
        true
    }

    pub(super) fn clear_all_queued_tx_demands(&mut self) {
        self.queued_floor_demands.clear();
        self.queued_floor_demand_ssis.clear();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TxDemandQueueResult {
    Queued,
    AlreadyQueuedBySameUser,
    QueueBusy,
    FromCurrentSpeaker,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum IndividualCallState {
    /// Generic setup state for locally initiated individual calls.
    CallSetupPending,
    /// Setup state for incoming call leg while awaiting local user/app response.
    IncomingSetupPending,
    /// Incoming call has alerted the destination side.
    IncomingAlerting,
    /// Incoming call setup is waiting for backhaul/network confirmation.
    IncomingSetupWaitNetworkAck,
    /// Call is established.
    Active,
    /// D-DISCONNECT was sent to one party; EN 300 392-2 14.7.1.6 expects U-RELEASE.
    DisconnectPending {
        awaiting_release_from: u32,
        release_to_issi: u32,
        started_at: TdmaTime,
        cause: DisconnectCause,
    },
}

#[derive(Clone)]
pub(super) struct IndividualCall {
    pub(super) calling_addr: TetraAddress,
    pub(super) called_addr: TetraAddress,
    pub(super) calling_handle: u32,
    pub(super) calling_link_id: u32,
    pub(super) calling_endpoint_id: u32,
    pub(super) called_handle: Option<u32>,
    pub(super) called_link_id: Option<u32>,
    pub(super) called_endpoint_id: Option<u32>,
    pub(super) calling_ts: u8,
    pub(super) called_ts: u8,
    pub(super) calling_usage: u8,
    pub(super) called_usage: u8,
    /// true = full duplex (ETSI 14.8.17), false = simplex
    pub(super) simplex_duplex: bool,
    /// Original U-SETUP request-to-transmit/send-data bit. EN 300 392-2
    /// table 14.74 defines the raw value as 0 = request to transmit/send data,
    /// 1 = request that the other MS may transmit/send data. Clause 14.5.1.2.1
    /// gives that raw field setup-method-specific meaning.
    pub(super) request_to_transmit_send_data: bool,
    pub(super) state: IndividualCallState,
    /// Start instant for setup timeout (T301/T302 equivalent on BS side).
    pub(super) setup_timer_started: Option<TdmaTime>,
    /// Setup timeout value while the call is not active.
    pub(super) setup_timeout: Option<CallTimeoutSetupPhase>,
    /// Start instant for active call timeout (T310 equivalent).
    pub(super) active_timer_started: Option<TdmaTime>,
    /// Active call timeout value.
    pub(super) call_timeout: CallTimeout,
    /// True when the called party lives behind Brew/TetraPack.
    pub(super) called_over_brew: bool,
    /// True when the calling party lives behind Brew/TetraPack.
    pub(super) calling_over_brew: bool,
    /// Brew UUID when this call is bridged to TetraPack.
    pub(super) brew_uuid: Option<uuid::Uuid>,
    /// Cached network call metadata for Brew bridged legs.
    pub(super) network_call: Option<NetworkCircuitCall>,
    /// True once CONNECT_REQUEST has been sent for Brew-originated setup.
    pub(super) connect_request_sent: bool,
    /// SSI of the party currently holding the floor (simplex P2P only).
    /// None until the call is active. Used by UL inactivity timeout to force TX-CEASED.
    pub(super) floor_holder: Option<u32>,
    /// Most recent simplex P2P floor holder, retained after U-TX CEASED so
    /// disconnect cleanup can distinguish the last speaker from a passive peer.
    pub(super) last_floor_holder: Option<u32>,
    /// Pending simplex P2P request-to-transmit while the peer still has the floor.
    pub(super) queued_tx_demand: Option<TetraAddress>,
}

impl IndividualCall {
    #[inline]
    pub(super) fn is_alerted(&self) -> bool {
        matches!(
            self.state,
            IndividualCallState::IncomingAlerting | IndividualCallState::IncomingSetupWaitNetworkAck | IndividualCallState::Active
        )
    }

    pub(super) fn mark_alerted(&mut self, now: TdmaTime, setup_timeout: CallTimeoutSetupPhase) {
        if matches!(
            self.state,
            IndividualCallState::CallSetupPending | IndividualCallState::IncomingSetupPending
        ) {
            self.state = IndividualCallState::IncomingAlerting;
        }
        self.setup_timer_started = Some(now);
        self.setup_timeout = Some(setup_timeout);
    }

    #[inline]
    pub(super) fn is_active(&self) -> bool {
        self.state == IndividualCallState::Active
    }

    #[inline]
    pub(super) fn has_established_circuit(&self) -> bool {
        matches!(
            self.state,
            IndividualCallState::Active | IndividualCallState::DisconnectPending { .. }
        )
    }

    pub(super) fn activate(&mut self, now: TdmaTime) {
        self.state = IndividualCallState::Active;
        self.setup_timer_started = None;
        self.setup_timeout = None;
        self.active_timer_started = Some(now);
        self.connect_request_sent = false;
    }

    pub(super) fn peer_issi_for(&self, sender_issi: u32) -> Option<u32> {
        if sender_issi == self.calling_addr.ssi {
            Some(self.called_addr.ssi)
        } else if sender_issi == self.called_addr.ssi {
            Some(self.calling_addr.ssi)
        } else {
            None
        }
    }

    pub(super) fn begin_disconnect_pending(
        &mut self,
        awaiting_release_from: u32,
        release_to_issi: u32,
        started_at: TdmaTime,
        cause: DisconnectCause,
    ) {
        self.state = IndividualCallState::DisconnectPending {
            awaiting_release_from,
            release_to_issi,
            started_at,
            cause,
        };
        self.clear_floor_holder();
        self.queued_tx_demand = None;
    }

    pub(super) fn set_floor_holder(&mut self, holder_issi: u32) {
        self.floor_holder = Some(holder_issi);
        self.last_floor_holder = Some(holder_issi);
    }

    pub(super) fn clear_floor_holder(&mut self) {
        if let Some(holder_issi) = self.floor_holder {
            self.last_floor_holder = Some(holder_issi);
        }
        self.floor_holder = None;
    }

    pub(super) fn peer_is_current_or_last_floor_holder(&self, peer_issi: u32) -> bool {
        self.floor_holder == Some(peer_issi) || (self.floor_holder.is_none() && self.last_floor_holder == Some(peer_issi))
    }

    pub(super) fn queue_tx_demand(&mut self, requester: TetraAddress) -> TxDemandQueueResult {
        if self.floor_holder == Some(requester.ssi) {
            return TxDemandQueueResult::FromCurrentSpeaker;
        }

        match self.queued_tx_demand {
            Some(existing) if existing.ssi == requester.ssi => TxDemandQueueResult::AlreadyQueuedBySameUser,
            Some(_) => TxDemandQueueResult::QueueBusy,
            None => {
                self.queued_tx_demand = Some(requester);
                TxDemandQueueResult::Queued
            }
        }
    }

    pub(super) fn take_queued_tx_demand(&mut self) -> Option<TetraAddress> {
        self.queued_tx_demand.take()
    }

    #[inline]
    pub(super) fn pending_disconnect_release_if_awaited_by(&self, sender_issi: u32) -> Option<(DisconnectCause, u32)> {
        match self.state {
            IndividualCallState::DisconnectPending {
                awaiting_release_from,
                release_to_issi,
                cause,
                ..
            } if awaiting_release_from == sender_issi => Some((cause, release_to_issi)),
            _ => None,
        }
    }

    #[inline]
    pub(super) fn pending_disconnect_timeout_expired(&self, now: TdmaTime, limit_timeslots: i32) -> Option<DisconnectCause> {
        match self.state {
            IndividualCallState::DisconnectPending { started_at, cause, .. } if started_at.age(now) > limit_timeslots => Some(cause),
            _ => None,
        }
    }

    #[inline]
    pub(super) fn setup_timeout_expired(&self, now: TdmaTime) -> bool {
        if self.is_active() {
            return false;
        }
        let Some(started) = self.setup_timer_started else {
            return false;
        };
        let Some(timeout) = self.setup_timeout else {
            return false;
        };
        let Some(limit) = setup_timeout_to_timeslots(timeout) else {
            return false;
        };
        started.age(now) > limit
    }

    #[inline]
    pub(super) fn active_timeout_expired(&self, now: TdmaTime) -> bool {
        if !self.is_active() {
            return false;
        }
        // Full-duplex individual calls (normal voice calls) have no timeout —
        // participants may talk for as long as they want.
        // Only simplex (half-duplex PTT) calls are subject to call_timeout,
        // to release the slot if an MS disappears without disconnecting.
        if self.simplex_duplex {
            return false;
        }
        let Some(started) = self.active_timer_started else {
            return false;
        };
        let Some(limit) = call_timeout_to_timeslots(self.call_timeout) else {
            return false;
        };
        started.age(now) > limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_group_call() -> ActiveCall {
        ActiveCall::new_local(TetraAddress::issi(100), 91, 100, 2, 4, TdmaTime::default(), CallTimeout::T30s, 0)
    }

    #[test]
    fn group_floor_waiter_fifo_is_deduplicated_and_bounded() {
        let mut call = test_group_call();

        assert_eq!(call.queue_tx_demand(TetraAddress::issi(101)), TxDemandQueueResult::Queued);
        assert_eq!(
            call.queue_tx_demand(TetraAddress::issi(101)),
            TxDemandQueueResult::AlreadyQueuedBySameUser
        );

        for offset in 1..MAX_GROUP_FLOOR_WAITERS {
            assert_eq!(
                call.queue_tx_demand(TetraAddress::issi(101 + offset as u32)),
                TxDemandQueueResult::Queued,
                "waiter offset {offset} should fit in the bounded FIFO"
            );
        }

        assert_eq!(
            call.queue_tx_demand(TetraAddress::issi(900_000)),
            TxDemandQueueResult::QueueBusy,
            "overflow beyond the bounded group floor FIFO must be rejected"
        );
        assert_eq!(call.take_queued_tx_demand().map(|addr| addr.ssi), Some(101));
        assert_eq!(call.take_queued_tx_demand().map(|addr| addr.ssi), Some(102));
        assert_eq!(
            call.queue_tx_demand(TetraAddress::issi(101)),
            TxDemandQueueResult::Queued,
            "a requester removed from the FIFO may request the floor again later"
        );
        assert!(call.clear_queued_tx_demand_from(101));
        assert_eq!(call.queue_tx_demand(TetraAddress::issi(101)), TxDemandQueueResult::Queued);
    }

    #[test]
    fn group_floor_waiter_take_through_drops_stale_prefix_and_preserves_tail() {
        let mut call = test_group_call();

        assert_eq!(call.queue_tx_demand(TetraAddress::issi(101)), TxDemandQueueResult::Queued);
        assert_eq!(call.queue_tx_demand(TetraAddress::issi(102)), TxDemandQueueResult::Queued);
        assert_eq!(call.queue_tx_demand(TetraAddress::issi(103)), TxDemandQueueResult::Queued);

        assert_eq!(
            call.take_queued_tx_demand_through(Some(102)).map(|requester| requester.ssi),
            Some(102)
        );
        assert_eq!(call.take_queued_tx_demand().map(|requester| requester.ssi), Some(103));
        assert_eq!(call.queue_tx_demand(TetraAddress::issi(101)), TxDemandQueueResult::Queued);
        assert_eq!(call.queue_tx_demand(TetraAddress::issi(102)), TxDemandQueueResult::Queued);
    }
}
