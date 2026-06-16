// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::time::Instant;

use super::ltpd_pipeline::{SndcpWapLtpdPipeline, SndcpWapLtpdPipelineError, issi_from_ltpd_ind};
use super::pdp::{
    SndcpActivatePdpContextDemand, SndcpDeactivation, SndcpPdpError, decode_activate_pdp_context_demand,
    decode_deactivate_pdp_context_accept, decode_deactivate_pdp_context_demand,
};
use super::pdp_service::{SndcpIpv4Pool, SndcpPdpPolicy};
use super::transfer::{
    SN_PDU_TYPE_DATA_TRANSMIT_REQUEST, SN_PDU_TYPE_DATA_TRANSMIT_RESPONSE, SN_PDU_TYPE_END_OF_DATA, SN_PDU_TYPE_NOT_SUPPORTED,
    SN_PDU_TYPE_RECONNECT, SndcpDataTransmitResponseResult, SndcpTransferControl, SndcpTransferError, decode_data_transmit_response,
    decode_transfer_control_pdu,
};
use super::unitdata::{SN_PDU_TYPE_UNITDATA, SndcpUnitdataError, decode_sn_unitdata_body};
use super::wap_ip::{WapIpEndpoint, WapIpServicePolicy};
use super::wap_session::SndcpWapSession;
use super::wap_status::WapStatusSnapshot;
use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::{CfgWapIp, SharedConfig};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Layer2Service, Sap};
use tetra_saps::{SapMsg, SapMsgInner};

pub use super::unitdata::{NetworkPduKind, SnUnitdata, SndcpEncodeError, encode_sn_unitdata};

const SN_PDU_TYPE_ACTIVATE_PDP_CONTEXT: u8 = 0;
const SN_PDU_TYPE_DEACTIVATE_PDP_CONTEXT_ACCEPT: u8 = 1;
const SN_PDU_TYPE_DEACTIVATE_PDP_CONTEXT_DEMAND: u8 = 2;

#[derive(Debug, Clone)]
pub enum SndcpDecode {
    ActivatePdpContextDemand(SndcpActivatePdpContextDemand),
    DeactivatePdpContextDemand(SndcpDeactivation),
    DeactivatePdpContextAccept(SndcpDeactivation),
    Unitdata(SnUnitdata),
    TransferControl(SndcpTransferControl),
    UnsupportedPduType(u8),
    UnsupportedNsapi(u8),
    UnsupportedCompression { pcomp: u8, dcomp: u8 },
    MalformedPdpContext(SndcpPdpError),
    MalformedTransferControl(SndcpTransferError),
    Malformed(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SndcpRuntimeHandoffPolicy {
    wap_ip: SndcpRuntimeHandoffMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpRuntimeHandoffMode {
    Disabled,
    WapIpStatus { assume_pdch_ready_after_data_transmit: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpRuntimeHandoffDecision {
    HandleWapIpStatus,
    DropServiceUnavailable,
    DropRuntimeHandoffDisabled { pdu: SndcpRuntimePduClass },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpRuntimePduClass {
    PdpActivationDemand,
    PdpDeactivationDemand,
    PdpDeactivationAccept,
    Unitdata,
    TransferControl,
    UnsupportedOrReserved,
    Malformed,
}

pub struct Sndcp {
    // config: Option<SharedConfig>,
    config: SharedConfig,
    runtime_handoff: SndcpRuntimeHandoffPolicy,
    wap_pipeline: Option<SndcpWapLtpdPipeline>,
    started_at: Instant,
}

impl Default for SndcpRuntimeHandoffPolicy {
    fn default() -> Self {
        Self::disabled()
    }
}

impl SndcpRuntimeHandoffPolicy {
    pub fn disabled() -> Self {
        Self {
            wap_ip: SndcpRuntimeHandoffMode::Disabled,
        }
    }

    pub fn wap_ip_status(assume_pdch_ready_after_data_transmit: bool) -> Self {
        Self {
            wap_ip: SndcpRuntimeHandoffMode::WapIpStatus {
                assume_pdch_ready_after_data_transmit,
            },
        }
    }

    pub fn assume_pdch_ready_after_data_transmit(&self) -> bool {
        match self.wap_ip {
            SndcpRuntimeHandoffMode::WapIpStatus {
                assume_pdch_ready_after_data_transmit,
            } => assume_pdch_ready_after_data_transmit,
            SndcpRuntimeHandoffMode::Disabled => false,
        }
    }

    pub fn decide_ltpd_unitdata_ind(&self, service_available: bool, decode: &SndcpDecode) -> SndcpRuntimeHandoffDecision {
        if !service_available {
            return SndcpRuntimeHandoffDecision::DropServiceUnavailable;
        }

        match self.wap_ip {
            SndcpRuntimeHandoffMode::Disabled => SndcpRuntimeHandoffDecision::DropRuntimeHandoffDisabled {
                pdu: SndcpRuntimePduClass::from_decode(decode),
            },
            SndcpRuntimeHandoffMode::WapIpStatus { .. } => SndcpRuntimeHandoffDecision::HandleWapIpStatus,
        }
    }
}

impl SndcpRuntimePduClass {
    pub fn from_decode(decode: &SndcpDecode) -> Self {
        match decode {
            SndcpDecode::ActivatePdpContextDemand(_) => SndcpRuntimePduClass::PdpActivationDemand,
            SndcpDecode::DeactivatePdpContextDemand(_) => SndcpRuntimePduClass::PdpDeactivationDemand,
            SndcpDecode::DeactivatePdpContextAccept(_) => SndcpRuntimePduClass::PdpDeactivationAccept,
            SndcpDecode::Unitdata(_) => SndcpRuntimePduClass::Unitdata,
            SndcpDecode::TransferControl(_) => SndcpRuntimePduClass::TransferControl,
            SndcpDecode::UnsupportedPduType(_) | SndcpDecode::UnsupportedNsapi(_) | SndcpDecode::UnsupportedCompression { .. } => {
                SndcpRuntimePduClass::UnsupportedOrReserved
            }
            SndcpDecode::MalformedPdpContext(_) | SndcpDecode::MalformedTransferControl(_) | SndcpDecode::Malformed(_) => {
                SndcpRuntimePduClass::Malformed
            }
        }
    }
}

impl Sndcp {
    pub fn new(config: SharedConfig) -> Self {
        let wap = config.config().cell.wap_ip.clone();
        let runtime_handoff = wap
            .as_ref()
            .filter(|wap| wap.enabled)
            .map(|wap| SndcpRuntimeHandoffPolicy::wap_ip_status(wap.assume_pdch_ready_after_data_transmit))
            .unwrap_or_default();
        let wap_pipeline = wap.as_ref().filter(|wap| wap.enabled).map(wap_ltpd_pipeline_from_cfg);
        Self {
            config,
            runtime_handoff,
            wap_pipeline,
            started_at: Instant::now(),
        }
    }

    pub fn with_runtime_handoff_policy(config: SharedConfig, runtime_handoff: SndcpRuntimeHandoffPolicy) -> Self {
        Self {
            config,
            runtime_handoff,
            wap_pipeline: None,
            started_at: Instant::now(),
        }
    }

    fn rx_ltpd_mle_unitdata_ind(&mut self, queue: &mut MessageQueue, prim: tetra_saps::ltpd::LtpdMleUnitdataInd) {
        if !self.config.config().cell.sndcp_service {
            tracing::warn!("SNDCP/WAP packet-data bearer is disabled; dropping LTPD MLE-UNITDATA.ind");
            return;
        }

        let prim = ltpd_ind_with_effective_sndcp_sdu(prim);
        let decode = decode_ltpd_sdu(&prim.sdu);
        tracing::info!(
            "WAP/IP diag: SNDCP inbound addr={:?} endpoint={} link={} sdu_bits={} decode={:?} runtime_policy={:?}",
            prim.received_tetra_address,
            prim.endpoint_id,
            prim.link_id,
            prim.sdu.get_len_remaining(),
            decode,
            self.runtime_handoff
        );
        match self.runtime_handoff.decide_ltpd_unitdata_ind(true, &decode) {
            SndcpRuntimeHandoffDecision::HandleWapIpStatus => {
                self.handle_wap_ip_ltpd_unitdata_ind(queue, prim, &decode);
                return;
            }
            SndcpRuntimeHandoffDecision::DropRuntimeHandoffDisabled { pdu } => {
                tracing::debug!(
                    "SNDCP: runtime WAP/IP handoff policy is disabled for {:?}; decode/log/drop remains fail-closed",
                    pdu
                );
            }
            SndcpRuntimeHandoffDecision::DropServiceUnavailable => {
                tracing::warn!("SNDCP/WAP packet-data bearer is unavailable; dropping LTPD MLE-UNITDATA.ind");
                return;
            }
        }

        match decode {
            SndcpDecode::ActivatePdpContextDemand(demand) => {
                tracing::warn!(
                    "SNDCP: decoded SN-ACTIVATE PDP CONTEXT DEMAND nsapi={} address={:?} packet_data_ms_type={:?}; PDP activation handler is not implemented, dropping fail-closed",
                    demand.nsapi,
                    demand.address,
                    demand.packet_data_ms_type
                );
            }
            SndcpDecode::DeactivatePdpContextDemand(deactivation) => {
                tracing::warn!(
                    "SNDCP: decoded SN-DEACTIVATE PDP CONTEXT DEMAND {:?}; PDP context table handoff is not implemented, dropping fail-closed",
                    deactivation
                );
            }
            SndcpDecode::DeactivatePdpContextAccept(deactivation) => {
                tracing::debug!(
                    "SNDCP: received SN-DEACTIVATE PDP CONTEXT ACCEPT {:?} with no pending PDP deactivation",
                    deactivation
                );
            }
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
            SndcpDecode::TransferControl(control) => {
                tracing::warn!(
                    "SNDCP: decoded transfer-control SN-PDU {:?}; READY/STANDBY bearer control is not wired, dropping fail-closed",
                    control
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
            SndcpDecode::MalformedPdpContext(err) => {
                tracing::warn!("SNDCP: malformed or unsupported PDP context SN-PDU {:?}, dropping", err);
            }
            SndcpDecode::MalformedTransferControl(err) => {
                tracing::warn!("SNDCP: malformed or unsupported transfer-control SN-PDU {:?}, dropping", err);
            }
            SndcpDecode::Malformed(field) => {
                tracing::warn!("SNDCP: malformed LTPD SN-PDU at {}, dropping", field);
            }
        }
    }

    fn handle_wap_ip_ltpd_unitdata_ind(
        &mut self,
        queue: &mut MessageQueue,
        prim: tetra_saps::ltpd::LtpdMleUnitdataInd,
        decode: &SndcpDecode,
    ) {
        let snapshot = self.wap_status_snapshot();
        let response = match self.wap_pipeline.as_mut() {
            Some(pipeline) => pipeline.handle_ltpd_mle_unitdata_ind_allocating(&prim, &snapshot),
            None => {
                tracing::warn!("SNDCP/WAP-IP runtime is enabled but no WAP/IP pipeline is configured; dropping");
                return;
            }
        };

        let response = match response {
            Ok(response) => response,
            Err(err) => {
                self.log_wap_pipeline_drop(&err);
                return;
            }
        };

        if self.should_mark_pdch_ready_after_response(decode, &response)
            && let Ok(issi) = issi_from_ltpd_ind(&prim)
            && let Some(pipeline) = self.wap_pipeline.as_mut()
        {
            // MVP-local common-control shortcut. Full PDCH/assigned-channel
            // allocation is deliberately left out to avoid perturbing
            // validated CMCE/voice resource handling.
            pipeline.mark_pdch_ready(issi, prim.endpoint_id, prim.link_id);
        }

        tracing::debug!(
            "SNDCP/WAP-IP: emitting {:?} response bits={} endpoint={} link={}",
            response.layer2service,
            response.sdu.get_len(),
            response.endpoint_id,
            response.link_id
        );
        queue.push_back(SapMsg {
            sap: Sap::TlpdSap,
            src: TetraEntity::Sndcp,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LtpdMleUnitdataReq(response),
        });
    }

    fn should_mark_pdch_ready_after_response(&self, decode: &SndcpDecode, response: &tetra_saps::ltpd::LtpdMleUnitdataReq) -> bool {
        if !self.runtime_handoff.assume_pdch_ready_after_data_transmit() {
            return false;
        }
        if !matches!(decode, SndcpDecode::TransferControl(SndcpTransferControl::DataTransmitRequest(_))) {
            return false;
        }
        if response.layer2service != Layer2Service::Acknowledged || response.packet_data_flag {
            return false;
        }

        match decode_data_transmit_response(&response.sdu) {
            Ok(response) => response.result == SndcpDataTransmitResponseResult::Accepted,
            Err(_) => false,
        }
    }

    fn log_wap_pipeline_drop(&self, err: &SndcpWapLtpdPipelineError) {
        tracing::warn!("SNDCP/WAP-IP: dropping inbound SNDCP PDU: {:?}", err);
    }

    fn wap_status_snapshot(&self) -> WapStatusSnapshot {
        let health = crate::health::registry().snapshot();
        let mut registered_issis: Vec<u32> = self.config.state_read().subscribers.all_registered_issis().collect();
        registered_issis.sort_unstable();
        let registered_ms = registered_issis.len();
        let active_calls = metric_from_health(&health, crate::health::HealthDomain::Voice, "active_group_calls").saturating_add(
            metric_from_health(&health, crate::health::HealthDomain::P2p, "active_individual_calls"),
        );
        let queued_sds = metric_from_health(&health, crate::health::HealthDomain::Sds, "live_queue_len")
            .saturating_add(metric_from_health(&health, crate::health::HealthDomain::Sds, "pending_actions"));

        WapStatusSnapshot {
            title: "Nexus-BS".to_string(),
            stack_version: tetra_core::STACK_VERSION.to_string(),
            service_state: service_state_from_health(health.overall).to_string(),
            registered_ms,
            active_calls,
            queued_sds,
            uptime_secs: self.started_at.elapsed().as_secs(),
            last_activity: None,
            health_summary: Some(health_summary(&health)),
            health_lines: health_lines_from_health(&health),
            radio_lines: registered_issis.into_iter().take(3).map(|issi| format!("MS {issi}")).collect(),
            call_lines: call_lines_from_health(&health),
        }
    }
}

fn wap_ltpd_pipeline_from_cfg(wap: &CfgWapIp) -> SndcpWapLtpdPipeline {
    let mut pdp_policy = SndcpPdpPolicy::experimental_wap_ipv4();
    pdp_policy.dynamic_ipv4_pool = Some(SndcpIpv4Pool {
        prefix: wap.dynamic_pool_prefix,
        first_host: wap.dynamic_pool_first_host,
        last_host: wap.dynamic_pool_last_host,
    });
    pdp_policy.allow_static_ipv4 = wap.allow_static_ipv4;

    let endpoint = WapIpEndpoint {
        address: wap.address,
        port: wap.port,
        response_ttl: wap.response_ttl,
    };
    let wap_policy = WapIpServicePolicy {
        status_enabled: true,
        accept_empty_probe: wap.accept_empty_probe,
        accept_root_path: wap.accept_root_path,
        accept_status_path: wap.accept_status_path,
        accept_status_wml_path: wap.accept_status_wml_path,
        max_request_payload_bytes: wap.max_request_payload_bytes,
        allowed_issis: None,
    };

    SndcpWapLtpdPipeline::new(SndcpWapSession::new(pdp_policy, endpoint, wap_policy))
}

fn metric_from_health(health: &crate::health::HealthSnapshot, domain: crate::health::HealthDomain, metric: &str) -> usize {
    health
        .domains
        .iter()
        .find(|snapshot| snapshot.domain == domain)
        .and_then(|snapshot| snapshot.metrics.iter().find(|m| m.name == metric))
        .and_then(|metric| usize::try_from(metric.value).ok())
        .unwrap_or(0)
}

fn service_state_from_health(severity: crate::health::HealthSeverity) -> &'static str {
    match severity {
        crate::health::HealthSeverity::Ok => "ON AIR",
        crate::health::HealthSeverity::Degraded => "DEGRADED",
        crate::health::HealthSeverity::Critical => "CRITICAL",
    }
}

fn health_summary(health: &crate::health::HealthSnapshot) -> String {
    match health.overall {
        crate::health::HealthSeverity::Ok => "OK".to_string(),
        severity => health
            .domains
            .iter()
            .find(|domain| domain.severity == severity)
            .map(|domain| format!("{}:{:?}", service_state_from_health(severity), domain.domain))
            .unwrap_or_else(|| service_state_from_health(severity).to_string()),
    }
}

fn health_lines_from_health(health: &crate::health::HealthSnapshot) -> Vec<String> {
    let mut lines: Vec<String> = health
        .domains
        .iter()
        .map(|domain| format!("{} {}", health_domain_label(domain.domain), health_severity_short(domain.severity)))
        .collect();
    lines.sort();
    lines
}

fn health_severity_short(severity: crate::health::HealthSeverity) -> &'static str {
    match severity {
        crate::health::HealthSeverity::Ok => "OK",
        crate::health::HealthSeverity::Degraded => "WARN",
        crate::health::HealthSeverity::Critical => "BAD",
    }
}

fn health_domain_label(domain: crate::health::HealthDomain) -> &'static str {
    match domain {
        crate::health::HealthDomain::Service => "CORE",
        crate::health::HealthDomain::Telemetry => "TEL",
        crate::health::HealthDomain::Brew => "BREW",
        crate::health::HealthDomain::Voice => "VOICE",
        crate::health::HealthDomain::Sds => "SDS",
        crate::health::HealthDomain::P2p => "P2P",
        crate::health::HealthDomain::Congestion => "LOAD",
        crate::health::HealthDomain::Rf => "RF",
    }
}

fn call_lines_from_health(health: &crate::health::HealthSnapshot) -> Vec<String> {
    let group = metric_from_health(health, crate::health::HealthDomain::Voice, "active_group_calls");
    let p2p = metric_from_health(health, crate::health::HealthDomain::P2p, "active_individual_calls");
    let mut lines = Vec::new();
    if group > 0 {
        lines.push(format!("GRP active {group}"));
    }
    if p2p > 0 {
        lines.push(format!("P2P active {p2p}"));
    }
    lines
}

pub fn decode_ltpd_sdu(sdu: &BitBuffer) -> SndcpDecode {
    let mut sdu = BitBuffer::from_bitbuffer_pos(sdu);

    let Some(sn_pdu_type) = sdu.read_bits(4) else {
        return SndcpDecode::Malformed("sn_pdu_type");
    };
    let sn_pdu_type = sn_pdu_type as u8;

    match sn_pdu_type {
        SN_PDU_TYPE_ACTIVATE_PDP_CONTEXT => {
            return decode_activate_pdp_context_demand(&sdu)
                .map(SndcpDecode::ActivatePdpContextDemand)
                .unwrap_or_else(SndcpDecode::MalformedPdpContext);
        }
        SN_PDU_TYPE_DEACTIVATE_PDP_CONTEXT_DEMAND => {
            return decode_deactivate_pdp_context_demand(&sdu)
                .map(SndcpDecode::DeactivatePdpContextDemand)
                .unwrap_or_else(SndcpDecode::MalformedPdpContext);
        }
        SN_PDU_TYPE_DEACTIVATE_PDP_CONTEXT_ACCEPT => {
            return decode_deactivate_pdp_context_accept(&sdu)
                .map(SndcpDecode::DeactivatePdpContextAccept)
                .unwrap_or_else(SndcpDecode::MalformedPdpContext);
        }
        SN_PDU_TYPE_UNITDATA => {}
        SN_PDU_TYPE_DATA_TRANSMIT_REQUEST
        | SN_PDU_TYPE_DATA_TRANSMIT_RESPONSE
        | SN_PDU_TYPE_END_OF_DATA
        | SN_PDU_TYPE_RECONNECT
        | SN_PDU_TYPE_NOT_SUPPORTED => {
            return decode_transfer_control_pdu(&sdu)
                .map(SndcpDecode::TransferControl)
                .unwrap_or_else(SndcpDecode::MalformedTransferControl);
        }
        _ => return SndcpDecode::UnsupportedPduType(sn_pdu_type),
    }

    decode_sn_unitdata_body(&sdu)
        .map(SndcpDecode::Unitdata)
        .unwrap_or_else(unitdata_error_to_decode)
}

fn unitdata_error_to_decode(error: SndcpUnitdataError) -> SndcpDecode {
    match error {
        SndcpUnitdataError::UnsupportedPduType(sn_pdu_type) => SndcpDecode::UnsupportedPduType(sn_pdu_type),
        SndcpUnitdataError::UnsupportedNsapi(nsapi) => SndcpDecode::UnsupportedNsapi(nsapi),
        SndcpUnitdataError::UnsupportedCompression { pcomp, dcomp } => SndcpDecode::UnsupportedCompression { pcomp, dcomp },
        SndcpUnitdataError::EmptyNPdu => SndcpDecode::Malformed("n_pdu"),
        SndcpUnitdataError::Malformed(field) => SndcpDecode::Malformed(field),
        SndcpUnitdataError::Sn(_) => SndcpDecode::Malformed("sn_sap"),
    }
}

fn ltpd_ind_with_effective_sndcp_sdu(mut prim: tetra_saps::ltpd::LtpdMleUnitdataInd) -> tetra_saps::ltpd::LtpdMleUnitdataInd {
    // MLE strips the 3-bit protocol discriminator by advancing the BitBuffer
    // cursor before handing the TL-SDU to LTPD-SAP. Keep SNDCP local decoding
    // cursor-relative so the same runtime accepts direct unit tests and real
    // MLE-routed packet-data indications without touching MM/CMCE paths.
    if prim.sdu.get_pos() != 0 {
        prim.sdu = BitBuffer::from_bitbuffer_pos(&prim.sdu);
    }
    prim
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
            SapMsgInner::LtpdMleUnitdataInd(prim) => self.rx_ltpd_mle_unitdata_ind(_queue, prim),
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
