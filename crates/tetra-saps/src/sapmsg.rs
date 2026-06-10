use core::fmt::Display;

use tetra_core::Sap;
use tetra_core::tetra_entities::TetraEntity;

use crate::control::brew::MmSubscriberUpdate;
use crate::control::call_control::CallControl;
use crate::control::sds::{CmceSdsData, CmceSdsStatus};
use crate::tmd::TmdCircuitDataInd;
use crate::tmd::TmdCircuitDataReq;
use crate::tnmm::TnmmTestDemand;
use crate::tnmm::TnmmTestResponse;

use super::lcmc::*;
use super::lmm::*;
use super::ltpd::*;
use super::tla::*;
use super::tlmb::*;
use super::tlmc::*;
use super::tma::*;
use super::tmv::*;
use super::tp::*;

/// Exhaustive list of SapMsgType structs for use in the SapMsg struct
/// See Clause 19.2.1 for an overview of all lower-layer SAPs
#[derive(Debug, Clone)]
pub enum SapMsgInner {
    // TODO FIXME and all that stuff
    // PhyControlUpdateNetinfo(PhyControlUpdateNetinfo),

    // LmacControlUpdateNetinfo(LmacControlUpdateNetinfo),
    /// TP-SAP (Contents not defined in standard)
    TpUnitdataInd(TpUnitdataInd),
    TpUnitdataReq(TpUnitdataReqSlot),

    // TMV-SAP
    TmvUnitdataReq(TmvUnitdataReqSlot),
    TmvUnitdataInd(TmvUnitdataInd),
    TmvConfigureReq(TmvConfigureReq),
    TmvConfigureConf(TmvConfigureConf),

    // TMA-SAP
    TmaUnitdataInd(TmaUnitdataInd),
    TmaUnitdataReq(TmaUnitdataReq),
    TmaCancelReq(TmaCancelReq),
    TmaReportInd(TmaReportInd),

    // TMB-SAP / TLB-SAP (merged to TLMB-SAP)
    TlmbSyncInd(TlmbSyncInd),
    TlmbSysinfoInd(TlmbSysinfoInd),

    // TMC-SAP
    TlmcConfigureReq(TlmcConfigureReq),

    // TMD-SAP (Uplane traffic and signalling)
    TmdCircuitDataReq(TmdCircuitDataReq),
    TmdCircuitDataInd(TmdCircuitDataInd),

    // TLB-SAP
    // TlmbSyncInd(TlmbSyncInd),
    // TlmbSysinfoInd(TlmbSysinfoInd),

    // TLA-SAP
    TlaTlDataConfBl(TlDataConfBl),
    TlaTlDataIndBl(TlaTlDataIndBl),
    TlaTlDataReqBl(TlaTlDataReqBl),
    TlaTlDataRespBl(TlDataRespBl),
    TlaTlReportInd(TlaTlReportInd),
    TlaTlUnitdataIndBl(TlaTlUnitdataIndBl),
    TlaTlUnitdataReqBl(TlaTlUnitdataReqBl),

    // LMM-SAP (MLE-MM)
    LmmMleReportInd(LmmMleReportInd),
    LmmMleUnitdataInd(LmmMleUnitdataInd),
    LmmMleUnitdataReq(LmmMleUnitdataReq),

    // LCMC-SAP (MLE-CMCE)
    LcmcMleConfigureReq(LcmcMleConfigureReq),
    LcmcMleReportInd(LcmcMleReportInd),
    LcmcMleUnitdataInd(LcmcMleUnitdataInd),
    LcmcMleUnitdataReq(LcmcMleUnitdataReq),

    // CMCE -> UMAC control
    CmceCallControl(CallControl),

    // MM -> Brew/CMCE subscriber update
    MmSubscriberUpdate(MmSubscriberUpdate),

    /// Sent by UMAC to MM when a UL burst is received from a known MS.
    /// MM stores the RSSI value per MS for logging and future handover decisions.
    MsRssiUpdate {
        issi: u32,
        rssi_dbfs: f32,
    },

    /// Sent by BrewEntity to MM when the Brew backhaul reconnects.
    /// MM responds by sending D-LOCATION-UPDATE-COMMAND to all locally registered MS,
    /// forcing them to re-affiliate. Without this, MS units registered before a
    /// Brew disconnect do not re-register and PTT calls are denied until power-cycle.
    BrewReconnected,

    /// Operator RF carrier inhibit request. MM owns the registered MS context,
    /// so it performs the final on-air notification and registry cleanup before
    /// allowing PHY to hard-gate TX.
    RfCarrierInhibit {
        inhibited: bool,
    },

    // CMCE SDS <-> Brew SDS routing
    CmceSdsData(CmceSdsData),
    CmceSdsStatus(CmceSdsStatus),

    // LTPD-SAP (MLE-LTPD)
    LtpdMleReportInd(LtpdMleReportInd),
    LtpdMleUnitdataReq(LtpdMleUnitdataReq),
    LtpdMleUnitdataInd(LtpdMleUnitdataInd),

    // TNMM-SAP (MM-User)
    TnmmTestDemand(TnmmTestDemand),
    TnmmTestResponse(TnmmTestResponse),
}

impl Display for SapMsgInner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            // TP-SAP
            // SapMsgInner::TpUnitdataInd(_) => write!(f, "TpUnitdataInd"),

            // TMV-SAP
            SapMsgInner::TmvUnitdataReq(_) => write!(f, "TmvUnitdataReq"),
            SapMsgInner::TmvUnitdataInd(_) => write!(f, "TmvUnitdataInd"),
            SapMsgInner::TmvConfigureReq(_) => write!(f, "TmvConfigureReq"),
            SapMsgInner::TmvConfigureConf(_) => write!(f, "TmvConfigureConf"),

            // TMA-SAP
            SapMsgInner::TmaUnitdataInd(_) => write!(f, "TmaUnitdataInd"),
            SapMsgInner::TmaUnitdataReq(_) => write!(f, "TmaUnitdataReq"),
            SapMsgInner::TmaCancelReq(_) => write!(f, "TmaCancelReq"),

            // TMB-SAP
            SapMsgInner::TlmbSyncInd(_) => write!(f, "TmbSyncInd"),
            SapMsgInner::TlmbSysinfoInd(_) => write!(f, "TmbSysinfoInd"),

            // Control/Brew
            SapMsgInner::MmSubscriberUpdate(_) => write!(f, "MmSubscriberUpdate"),
            SapMsgInner::MsRssiUpdate { issi, rssi_dbfs } => write!(f, "MsRssiUpdate(issi={}, rssi={:.1}dBFS)", issi, rssi_dbfs),
            SapMsgInner::RfCarrierInhibit { inhibited } => write!(f, "RfCarrierInhibit(inhibited={})", inhibited),
            SapMsgInner::CmceSdsData(_) => write!(f, "CmceSdsData"),
            SapMsgInner::CmceSdsStatus(_) => write!(f, "CmceSdsStatus"),

            // TLB-SAP
            // SapMsgInner::TlbTlSyncInd(_) => write!(f, "TlbTlSyncInd"),
            // SapMsgInner::TlbTlSysinfoInd(_) => write!(f, "TlbTlSysinfoInd"),
            SapMsgInner::TlaTlDataConfBl(_) => write!(f, "TlaTlDataConfBl"),
            SapMsgInner::TlaTlDataIndBl(_) => write!(f, "TlaTlDataIndBl"),
            SapMsgInner::TlaTlDataReqBl(_) => write!(f, "TlaTlDataReqBl"),
            SapMsgInner::TlaTlDataRespBl(_) => write!(f, "TlaTlDataRespBl"),
            SapMsgInner::TlaTlReportInd(_) => write!(f, "TlaTlReportInd"),
            SapMsgInner::TlaTlUnitdataIndBl(_) => write!(f, "TlaTlUnitdataIndBl"),
            SapMsgInner::TlaTlUnitdataReqBl(_) => write!(f, "TlaTlUnitdataReqBl"),
            SapMsgInner::LmmMleReportInd(_) => write!(f, "LmmMleReportInd"),
            SapMsgInner::LcmcMleConfigureReq(_) => write!(f, "LcmcMleConfigureReq"),
            SapMsgInner::LcmcMleReportInd(_) => write!(f, "LcmcMleReportInd"),
            SapMsgInner::LtpdMleReportInd(_) => write!(f, "LtpdMleReportInd"),
            SapMsgInner::LtpdMleUnitdataReq(_) => write!(f, "LtpdMleUnitdataReq"),
            SapMsgInner::LtpdMleUnitdataInd(_) => write!(f, "LtpdMleUnitdataInd"),
            _ => panic!("Unknown SapMsgInner type"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SapMsg {
    pub sap: Sap,
    pub src: TetraEntity,
    pub dest: TetraEntity,
    pub msg: SapMsgInner,
}

impl SapMsg {
    pub fn new(sap: Sap, src: TetraEntity, dest: TetraEntity, msg: SapMsgInner) -> Self {
        Self { sap, src, dest, msg }
    }

    pub fn get_source(&self) -> &TetraEntity {
        &self.src
    }
    pub fn get_dest(&self) -> &TetraEntity {
        &self.dest
    }
    pub fn get_sap(&self) -> &Sap {
        &self.sap
    }
    // pub fn get_prim(&self) -> &SapPrim {
    //     &self.prim
    // }
    // pub fn get_subprim(&self) -> &SapSubPrim {
    //     &self.subprim
    // }
}
