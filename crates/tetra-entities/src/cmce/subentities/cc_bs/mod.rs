use std::collections::{HashMap, HashSet, VecDeque};

use tetra_config::bluestation::SharedConfig;
use tetra_core::typed_pdu_fields::Type3FieldGeneric;
use tetra_core::{
    BitBuffer, Direction, Layer2Service, Sap, SsiType, TdmaTime, TetraAddress, TimeslotOwner, TxReporter, TxState,
    tetra_entities::TetraEntity, unimplemented_log,
};
use tetra_pdus::cmce::enums::disconnect_cause::DisconnectCause;
use tetra_pdus::cmce::{
    enums::{
        call_timeout::CallTimeout, call_timeout_setup_phase::CallTimeoutSetupPhase, cmce_pdu_type_dl::CmcePduTypeDl,
        cmce_pdu_type_ul::CmcePduTypeUl, transmission_grant::TransmissionGrant, type3_elem_id::CmceType3ElemId,
    },
    fields::basic_service_information::BasicServiceInformation,
    pdus::{
        cmce_function_not_supported::CmceFunctionNotSupported, d_alert::DAlert, d_call_proceeding::DCallProceeding, d_connect::DConnect,
        d_connect_acknowledge::DConnectAcknowledge, d_disconnect::DDisconnect, d_info::DInfo, d_release::DRelease, d_setup::DSetup,
        d_tx_ceased::DTxCeased, d_tx_granted::DTxGranted, d_tx_interrupt::DTxInterrupt, u_alert::UAlert, u_connect::UConnect,
        u_disconnect::UDisconnect, u_info::UInfo, u_release::URelease, u_setup::USetup, u_tx_ceased::UTxCeased, u_tx_demand::UTxDemand,
    },
    structs::cmce_circuit::CmceCircuit,
};
use tetra_saps::{
    SapMsg, SapMsgInner,
    control::{
        brew::{BrewSubscriberAction, MmSubscriberUpdate},
        call_control::{CallControl, Circuit, CircuitDlMediaSource, NetworkCircuitCall},
        enums::{circuit_mode_type::CircuitModeType, communication_type::CommunicationType},
    },
    lcmc::{
        LcmcMleUnitdataReq,
        enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment},
        fields::chan_alloc_req::CmceChanAllocReq,
    },
};

use crate::net_brew;
use crate::{
    MessageQueue,
    cmce::components::circuit_mgr::{CircuitMgr, CircuitMgrCmd},
};

mod call;
mod dtmf;
mod echo;
mod fsm;
mod ingress;
mod network;
mod parrot;
mod shared;
mod timers;
use echo::EchoSession;
use parrot::ParrotSession;

use call::{ActiveCall, CallOrigin, GroupCallState, IndividualCall, IndividualCallState, TxDemandQueueResult};
use fsm::{GroupTransitionError, IndividualTransitionError};

struct CachedSetup {
    pdu: DSetup,
    dest_addr: TetraAddress,
    resend: bool,
    last_resend_reporter: Option<TxReporter>,
    /// True for P2P individual calls where DSetup must be resent on MCCH (no chan_alloc).
    /// False for group calls where DSetup is resent on the traffic channel with chan_alloc.
    is_individual: bool,
}

struct PendingGroupRelease {
    call_id: u16,
    dest_gssi: u32,
    ts: u8,
    cause: DisconnectCause,
    reporter: TxReporter,
    started_at: TdmaTime,
    preclosed_circuit: Option<CmceCircuit>,
    is_local: bool,
    network_brew_uuid: Option<uuid::Uuid>,
}

struct PendingIndividualRelease {
    call_id: u16,
    call: IndividualCall,
    cause: DisconnectCause,
    reporters: Vec<TxReporter>,
    started_at: TdmaTime,
    preclosed_circuits: Vec<CmceCircuit>,
    notify_brew: bool,
}

struct PendingIndividualDisconnectDelivery {
    awaiting_release_from: u32,
    release_to_issi: u32,
    cause: DisconnectCause,
    reporter: TxReporter,
    started_at: TdmaTime,
}

struct PendingIndividualDisconnectTailDrain {
    sender: TetraAddress,
    peer_issi: u32,
    cause: DisconnectCause,
    started_at: TdmaTime,
}

struct PendingIndividualDisconnectReleaseAck {
    release_to_issi: u32,
    cause: DisconnectCause,
    reporters: Vec<TxReporter>,
    peer_release_reporters: Vec<TxReporter>,
    started_at: TdmaTime,
    peer_clear_complete: bool,
}

#[derive(Clone, Copy)]
struct IndividualTailDrainLeg {
    addr: TetraAddress,
    ts: u8,
    usage: u8,
}

struct PendingIndividualTxCeasedTailDrain {
    call_id: u16,
    sender: IndividualTailDrainLeg,
    peer: IndividualTailDrainLeg,
    notify_brew: bool,
    started_at: TdmaTime,
}

struct PendingGroupTxCeasedTailDrain {
    call_id: u16,
    sender: TetraAddress,
    dest_gssi: u32,
    ts: u8,
    usage: u8,
    notify_brew: bool,
    started_at: TdmaTime,
}

struct PendingIndividualConnectAck {
    reporter: TxReporter,
    stage: PendingIndividualConnectAckStage,
    started_at: TdmaTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingIndividualConnectAckStage {
    CalledConnectAck { attempts: u8, requires_l2_ack: bool },
    CallerDConnect { attempts: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingNetworkIndividualConnectKind {
    LocalCallerDConnect,
    LocalCalledDConnectAck,
}

struct PendingNetworkIndividualConnect {
    reporter: TxReporter,
    brew_uuid: uuid::Uuid,
    call_id: u16,
    ts: u8,
    local_addr: TetraAddress,
    peer_addr: TetraAddress,
    simplex_duplex: bool,
    grant: TransmissionGrant,
    permission: u8,
    started_at: TdmaTime,
    kind: PendingNetworkIndividualConnectKind,
}

/// Clause 14 Call Control CMCE sub-entity (ETSI EN 300 392-2)
/// Supports group calls (simplex PTT), individual calls (full-duplex P2P),
/// and circuit-switched calls bridged over Brew/TetraPack.
pub struct CcBsSubentity {
    config: SharedConfig,
    dltime: TdmaTime,
    /// Cached D-SETUP PDUs for late-entry re-sends: call_id -> cached setup
    cached_setups: HashMap<u16, CachedSetup>,
    circuits: CircuitMgr,
    /// Active group calls: call_id -> call info
    active_calls: HashMap<u16, ActiveCall>,
    /// Group releases waiting for FACCH/STCH D-RELEASE transmission or a bounded guard timeout.
    pending_group_releases: HashMap<u16, PendingGroupRelease>,
    /// Active or pending individual calls (P2P / duplex)
    individual_calls: HashMap<u16, IndividualCall>,
    /// Simplex private-call release signalling waiting for traffic interleaver tail bits.
    pending_individual_disconnect_tail_drains: HashMap<u16, PendingIndividualDisconnectTailDrain>,
    /// Active D-DISCONNECT deliveries that must reach MAC before the peer-response timer starts.
    pending_individual_disconnect_deliveries: HashMap<u16, PendingIndividualDisconnectDelivery>,
    /// Prompt D-RELEASE acknowledgements already sent to the MS that requested individual disconnection.
    pending_individual_disconnect_release_acks: HashMap<u16, PendingIndividualDisconnectReleaseAck>,
    /// Simplex private-call TX-CEASED/floor handoff signalling waiting for traffic tail bits.
    pending_individual_tx_ceased_tail_drains: HashMap<u16, PendingIndividualTxCeasedTailDrain>,
    /// Group-call TX-CEASED/floor idle signalling waiting for traffic tail bits.
    pending_group_tx_ceased_tail_drains: HashMap<u16, PendingGroupTxCeasedTailDrain>,
    /// Direct private-call setup delivery guards before caller authorization.
    pending_individual_connect_acks: HashMap<u16, PendingIndividualConnectAck>,
    /// Brew-bridged individual connect waiting for the local RF leg to acknowledge D-CONNECT/D-CONNECT ACK.
    pending_network_individual_connects: HashMap<u16, PendingNetworkIndividualConnect>,
    /// Individual releases waiting for assigned-channel D-RELEASE transmission or a bounded guard timeout.
    pending_individual_releases: HashMap<u16, PendingIndividualRelease>,
    /// Registered subscriber groups (ISSI -> set of GSSIs)
    subscriber_groups: HashMap<u32, HashSet<u32>>,
    /// Listener counts per GSSI
    group_listeners: HashMap<u32, usize>,
    /// Telemetry sink for call events (optional)
    telemetry: Option<crate::net_telemetry::channel::TelemetrySink>,
    /// Active echo service session (ISSI 999), if any
    echo_session: Option<EchoSession>,
    /// Active local Parrot/Papagal simplex service session (ISSI 99999), if any.
    parrot_session: Option<ParrotSession>,
}
