use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Sap};
use tetra_saps::{SapMsg, SapMsgInner};

const SN_PDU_TYPE_UNITDATA: u8 = 4;
const SNDCP_NO_COMPRESSION: u8 = 0;
const IPV4_VERSION: u8 = 4;
const IPV6_VERSION: u8 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPduKind {
    Ipv4,
    Ipv6,
    Other(u8),
    TooShort,
}

#[derive(Debug, Clone)]
pub struct SnUnitdata {
    pub nsapi: u8,
    pub pcomp: u8,
    pub dcomp: u8,
    pub n_pdu: BitBuffer,
    pub network_pdu_kind: NetworkPduKind,
}

#[derive(Debug, Clone)]
pub enum SndcpDecode {
    Unitdata(SnUnitdata),
    UnsupportedPduType(u8),
    UnsupportedNsapi(u8),
    UnsupportedCompression { pcomp: u8, dcomp: u8 },
    Malformed(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SndcpEncodeError {
    UnsupportedNsapi(u8),
    UnsupportedCompression { pcomp: u8, dcomp: u8 },
    EmptyNPdu,
}

pub struct Sndcp {
    // config: Option<SharedConfig>,
    config: SharedConfig,
}

impl Sndcp {
    pub fn new(config: SharedConfig) -> Self {
        Self { config }
    }

    fn rx_ltpd_mle_unitdata_ind(&mut self, prim: tetra_saps::ltpd::LtpdMleUnitdataInd) {
        if !self.config.config().cell.sndcp_service {
            tracing::warn!("SNDCP/WAP packet-data bearer is disabled; dropping LTPD MLE-UNITDATA.ind");
            return;
        }

        match decode_ltpd_sdu(&prim.sdu) {
            SndcpDecode::Unitdata(unitdata) => {
                tracing::warn!(
                    "SNDCP: decoded SN-UNITDATA nsapi={} pcomp={} dcomp={} n_pdu_bits={} kind={:?}; no SN-SAP/IP/WAP handoff is implemented, dropping fail-closed",
                    unitdata.nsapi,
                    unitdata.pcomp,
                    unitdata.dcomp,
                    unitdata.n_pdu.get_len(),
                    unitdata.network_pdu_kind
                );
            }
            SndcpDecode::UnsupportedPduType(sn_pdu_type) => {
                tracing::warn!("SNDCP: unsupported SN PDU type {}, dropping", sn_pdu_type);
            }
            SndcpDecode::UnsupportedNsapi(nsapi) => {
                tracing::warn!("SNDCP: unsupported/reserved NSAPI {}, dropping SN-UNITDATA", nsapi);
            }
            SndcpDecode::UnsupportedCompression { pcomp, dcomp } => {
                tracing::warn!(
                    "SNDCP: unsupported SN-UNITDATA compression pcomp={} dcomp={}, dropping",
                    pcomp,
                    dcomp
                );
            }
            SndcpDecode::Malformed(field) => {
                tracing::warn!("SNDCP: malformed LTPD SN-PDU at {}, dropping", field);
            }
        }
    }
}

pub fn decode_ltpd_sdu(sdu: &BitBuffer) -> SndcpDecode {
    let mut sdu = BitBuffer::from_bitbuffer(sdu);
    if sdu.get_pos() != 0 {
        sdu.seek(0);
    }

    let Some(sn_pdu_type) = sdu.read_bits(4) else {
        return SndcpDecode::Malformed("sn_pdu_type");
    };
    let sn_pdu_type = sn_pdu_type as u8;

    if sn_pdu_type != SN_PDU_TYPE_UNITDATA {
        return SndcpDecode::UnsupportedPduType(sn_pdu_type);
    }

    let Some(nsapi) = sdu.read_bits(4) else {
        return SndcpDecode::Malformed("nsapi");
    };
    let nsapi = nsapi as u8;
    if !(1..=14).contains(&nsapi) {
        return SndcpDecode::UnsupportedNsapi(nsapi);
    }

    let Some(pcomp) = sdu.read_bits(4) else {
        return SndcpDecode::Malformed("pcomp");
    };
    let Some(dcomp) = sdu.read_bits(4) else {
        return SndcpDecode::Malformed("dcomp");
    };
    let pcomp = pcomp as u8;
    let dcomp = dcomp as u8;

    if pcomp != SNDCP_NO_COMPRESSION || dcomp != SNDCP_NO_COMPRESSION {
        return SndcpDecode::UnsupportedCompression { pcomp, dcomp };
    }

    if sdu.get_len_remaining() == 0 {
        return SndcpDecode::Malformed("n_pdu");
    }

    let n_pdu = BitBuffer::from_bitbuffer_pos(&sdu);
    let network_pdu_kind = classify_network_pdu(&n_pdu);

    SndcpDecode::Unitdata(SnUnitdata {
        nsapi,
        pcomp,
        dcomp,
        n_pdu,
        network_pdu_kind,
    })
}

pub fn encode_sn_unitdata(nsapi: u8, pcomp: u8, dcomp: u8, n_pdu: &BitBuffer) -> Result<BitBuffer, SndcpEncodeError> {
    // EN 300 392-2 clause 28.4.4.14/table 28.43: SN-UNITDATA is
    // SN PDU type(4), NSAPI(4), PCOMP(4), DCOMP(4), then a variable N-PDU
    // whose length is defined by the lower layer PDU length.
    if !(1..=14).contains(&nsapi) {
        return Err(SndcpEncodeError::UnsupportedNsapi(nsapi));
    }
    if pcomp != SNDCP_NO_COMPRESSION || dcomp != SNDCP_NO_COMPRESSION {
        return Err(SndcpEncodeError::UnsupportedCompression { pcomp, dcomp });
    }
    if n_pdu.get_len() == 0 {
        return Err(SndcpEncodeError::EmptyNPdu);
    }

    let mut pdu = BitBuffer::new(16 + n_pdu.get_len());
    pdu.write_bits(SN_PDU_TYPE_UNITDATA as u64, 4);
    pdu.write_bits(nsapi as u64, 4);
    pdu.write_bits(pcomp as u64, 4);
    pdu.write_bits(dcomp as u64, 4);

    let mut n_pdu = BitBuffer::from_bitbuffer(n_pdu);
    n_pdu.seek(0);
    while let Some(bit) = n_pdu.read_bits(1) {
        pdu.write_bits(bit, 1);
    }
    pdu.seek(0);
    Ok(pdu)
}

fn classify_network_pdu(n_pdu: &BitBuffer) -> NetworkPduKind {
    let Some(version) = n_pdu.peek_bits(4) else {
        return NetworkPduKind::TooShort;
    };

    match version as u8 {
        IPV4_VERSION => NetworkPduKind::Ipv4,
        IPV6_VERSION => NetworkPduKind::Ipv6,
        other => NetworkPduKind::Other(other),
    }
}

impl TetraEntityTrait for Sndcp {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Sndcp
    }

    fn rx_prim(&mut self, _queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);
        // tracing::debug!(ts=%message.dltime, "rx_prim: {:?}", message);

        // EN 300 392-2 clause 17.3.5 defines the MLE-SNDCP service at
        // LTPD-SAP. Clause 18.5.21 routes protocol discriminator 100b to
        // SNDCP before this point; table 18.26 service advertising remains
        // fail-closed unless this entity can serve the packet-data bearer.
        if message.sap != Sap::TlpdSap {
            tracing::warn!("SNDCP: dropping unexpected {:?} primitive", message.sap);
            return;
        }

        match message.msg {
            SapMsgInner::LtpdMleUnitdataInd(prim) => self.rx_ltpd_mle_unitdata_ind(prim),
            SapMsgInner::LtpdMleReportInd(prim) => {
                tracing::debug!(
                    "SNDCP: received MLE-REPORT.ind handle={} transfer_result={} with no pending local SN request",
                    prim.handle,
                    prim.transfer_result
                );
            }
            other => {
                tracing::warn!("SNDCP: dropping unexpected LTPD primitive {:?}", other);
            }
        }
    }
}
