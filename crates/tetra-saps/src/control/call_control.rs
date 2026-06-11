use tetra_core::{Direction, SsiType, TetraAddress};

use crate::control::enums::circuit_mode_type::CircuitModeType;

/// Specifies where downlink media originates for an open circuit.
/// Used by UMAC to decide whether to loopback UL audio or pull from network bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitDlMediaSource {
    /// Downlink media comes from local UL loopback (classic BS group/simplex behavior).
    LocalLoopback,
    /// Downlink media is generated locally by CMCE after recording UL frames.
    LocalParrot,
    /// Downlink media is supplied by SwMI over the network bridge (Brew/TetraPack).
    SwMI,
}

#[derive(Debug, Clone)]
pub struct Circuit {
    /// Direction
    pub direction: Direction,

    /// Timeslot in which this circuit exists
    pub ts: u8,

    /// Optional peer timeslot for P2P cross-routing (UL on ts -> DL on peer_ts).
    /// For local P2P calls, including simplex calls where only one MS holds the
    /// floor at a time, calling MS and called MS may use different assigned
    /// timeslots and audio is crossed between them. None for group calls and
    /// same-timeslot local loopback.
    pub peer_ts: Option<u8>,

    /// Usage number, between 4 and 63
    pub usage: u8,

    /// Traffic channel type
    pub circuit_mode: CircuitModeType,

    /// 2 opt, 00 = TETRA encoded speech, 1|2 = reserved, 3 = proprietary
    pub speech_service: Option<u8>,
    /// Whether end-to-end encryption is enabled on this circuit
    pub etee_encrypted: bool,

    /// Downlink media source policy for this circuit.
    pub dl_media_source: CircuitDlMediaSource,

    /// Primary local ISSI/GSSI whose energy-economy sleep cycle is suspended
    /// while this assigned-channel/call context is active.
    ///
    /// The primary address also defines the bearer scope: group calls keep a
    /// GSSI primary even when the current speaker ISSI is tracked as secondary;
    /// private/P2P calls use an ISSI primary so UMAC can enforce the individual
    /// participant set.
    pub active_addr: Option<TetraAddress>,

    /// Additional local ISSI/GSSI values whose energy-economy sleep cycle is
    /// suspended by the same assigned-channel/call context. Secondary ISSIs
    /// are metadata for EG/listening and do not by themselves make a group
    /// bearer private/P2P-scoped.
    pub active_secondary_addrs: Vec<TetraAddress>,
}

impl Circuit {
    pub fn active_addresses(&self) -> impl Iterator<Item = TetraAddress> + '_ {
        self.active_addr.into_iter().chain(self.active_secondary_addrs.iter().copied())
    }

    pub fn is_active_for_addr(&self, addr: TetraAddress) -> bool {
        self.active_addresses().any(|active_addr| active_addr == addr)
    }

    /// True for individual/private bearers where the primary address is an
    /// ISSI. EN 300 392-2 clause 14.5.1 individual calls need strict
    /// participant filtering, while clause 14.5.2 group calls remain GSSI
    /// scoped even when the current speaker ISSI is stored as secondary.
    pub fn is_primary_issi_scoped(&self) -> bool {
        self.active_addr.is_some_and(|active_addr| active_addr.ssi_type == SsiType::Issi)
    }
}

/// Metadata for a circuit-switched individual call (P2P/PBX) bridged over Brew/TetraPack.
/// Mirrors the TetraPack CIRCUIT_CALL_SETUP / CIRCUIT_CALL_CONNECT PDU fields.
#[derive(Debug, Clone)]
pub struct NetworkCircuitCall {
    /// Calling party ISSI
    pub source_issi: u32,
    /// Called party ISSI/GSSI when available; 0 for external/PBX calls.
    pub destination: u32,
    /// External number for PBX/phone calls (ASCII digits, may be empty).
    pub number: String,
    /// Call priority (ETSI 14.8.27 Table 14.73)
    pub priority: u8,
    /// Speech service (ETSI Table 14.79)
    pub service: u8,
    /// Circuit mode type (ETSI Table 14.52)
    pub mode: u8,
    /// Duplex flag (0 = simplex, 1 = duplex; ETSI 14.8.17)
    pub duplex: u8,
    /// Hook method selection (ETSI Table 14.62)
    pub method: u8,
    /// Communication type (ETSI Table 14.54)
    pub communication: u8,
    /// Transmission grant (ETSI Table 14.80)
    pub grant: u8,
    /// Transmission request permission (ETSI Table 14.81)
    pub permission: u8,
    /// Call timeout (ETSI Table 14.50)
    pub timeout: u8,
    /// Call ownership (ETSI Table 14.38)
    pub ownership: u8,
    /// Call queued flag (ETSI Table 14.48)
    pub queued: u8,
}

#[derive(Debug, Clone)]
pub enum CallControl {
    /// Signals to set up a circuit
    Open(Circuit),
    /// Updates the downlink media source policy for an already-open circuit.
    SetDlMediaSource { ts: u8, dl_media_source: CircuitDlMediaSource },
    /// Signals to release a circuit
    Close(Direction, u8),
    /// Floor granted: a speaker has been given transmission permission.
    FloorGranted {
        call_id: u16,
        source_issi: u32,
        dest_gssi: u32,
        ts: u8,
    },
    /// Floor released: speaker stopped transmitting (entering hangtime).
    FloorReleased { call_id: u16, ts: u8 },
    /// Call ended: the call is being torn down.
    CallEnded { call_id: u16, ts: u8 },
    /// Request CMCE to start a network-initiated group call
    NetworkCallStart {
        brew_uuid: uuid::Uuid,
        source_issi: u32,
        dest_gssi: u32,
        priority: u8,
    },
    /// Notify Brew that network call is ready with allocated resources
    NetworkCallReady {
        brew_uuid: uuid::Uuid,
        call_id: u16,
        ts: u8,
        usage: u8,
    },
    /// Request ending a network call
    NetworkCallEnd { brew_uuid: uuid::Uuid },
    /// UL inactivity detected on a traffic timeslot.
    UlInactivityTimeout { ts: u8 },

    // ---- Full-duplex individual / circuit-switched call signalling (ETSI EN 300 392-2 §14) ----
    /// CMCE -> Brew: local MS initiated a call to a non-local ISSI or PBX number.
    NetworkCircuitSetupRequest { brew_uuid: uuid::Uuid, call: NetworkCircuitCall },
    /// Brew -> CMCE: TetraPack accepted the circuit setup.
    NetworkCircuitSetupAccept { brew_uuid: uuid::Uuid },
    /// Brew -> CMCE: TetraPack rejected the circuit setup.
    NetworkCircuitSetupReject { brew_uuid: uuid::Uuid, cause: u8 },
    /// CMCE -> Brew / Brew -> CMCE: alerting phase.
    NetworkCircuitAlert { brew_uuid: uuid::Uuid },
    /// CMCE -> Brew: called MS sent U-CONNECT.
    NetworkCircuitConnectRequest { brew_uuid: uuid::Uuid, call: NetworkCircuitCall },
    /// Brew -> CMCE: TetraPack confirmed the circuit connect.
    NetworkCircuitConnectConfirm { brew_uuid: uuid::Uuid, grant: u8, permission: u8 },
    /// Brew -> CMCE / CMCE -> Brew: simplex floor granted on an active individual circuit.
    NetworkCircuitSimplexGranted { brew_uuid: uuid::Uuid, grant: u8, permission: u8 },
    /// Brew -> CMCE / CMCE -> Brew: simplex floor idle on an active individual circuit.
    NetworkCircuitSimplexIdle { brew_uuid: uuid::Uuid, grant: u8, permission: u8 },
    /// CMCE -> Brew: traffic channel is open, bridge can start media.
    NetworkCircuitMediaReady { brew_uuid: uuid::Uuid, call_id: u16, ts: u8 },
    /// CMCE -> Brew: DTMF/U-INFO payload forwarded from local MS.
    NetworkCircuitDtmf {
        brew_uuid: uuid::Uuid,
        length_bits: u16,
        data: Vec<u8>,
    },
    /// Either side: release the individual circuit call.
    NetworkCircuitRelease { brew_uuid: uuid::Uuid, cause: u8 },
}
