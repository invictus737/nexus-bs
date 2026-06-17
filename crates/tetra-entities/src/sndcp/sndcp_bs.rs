// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::collections::HashMap;
use std::time::Instant;

use super::ip::{IPV4_PROTOCOL_UDP, bitbuffer_npdu_octets, parse_ipv4_packet, parse_udp_datagram};
use super::ltpd_pipeline::{SndcpWapLtpdPipeline, SndcpWapLtpdPipelineError, issi_from_ltpd_ind};
use super::pdch::{SndcpLtpdConfigureReason, SndcpPdchState, SndcpStatusForMle};
use super::pdp::{
    SndcpActivatePdpContextDemand, SndcpDeactivation, SndcpPdpError, decode_activate_pdp_context_demand,
    decode_deactivate_pdp_context_accept, decode_deactivate_pdp_context_demand,
};
use super::pdp_service::{SndcpIpv4Pool, SndcpPdpPolicy};
use super::transfer::{
    SN_PDU_TYPE_DATA, SN_PDU_TYPE_DATA_TRANSMIT_REQUEST, SN_PDU_TYPE_DATA_TRANSMIT_RESPONSE, SN_PDU_TYPE_END_OF_DATA,
    SN_PDU_TYPE_NOT_SUPPORTED, SN_PDU_TYPE_RECONNECT, SndcpDataTransmitRequest, SndcpDataTransmitResponse, SndcpDataTransmitResponseResult,
    SndcpTransferControl, SndcpTransferError, SndcpTransferRejectCause, decode_data_transmit_response, decode_transfer_control_pdu,
    encode_data_transmit_response,
};
use super::unitdata::{SN_PDU_TYPE_UNITDATA, SndcpUnitdataError, decode_sn_unitdata_body};
use super::wap_ip::{WapIpEndpoint, WapIpServicePolicy, WapUdpRequestKind, parse_wap_udp_request};
use super::wap_session::{SndcpWapSession, SndcpWapSessionError};
use super::wap_status::WapStatusSnapshot;
use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::{CfgWapIp, SharedConfig};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, EndpointId, Layer2Service, LinkId, Sap, SsiType, TimeslotOwner, Todo};
use tetra_saps::control::brew::BrewSubscriberAction;
use tetra_saps::ltpd::{LtpdMleConfigureInd, LtpdMleReportInd};
use tetra_saps::tla::{
    TLA_REPORT_FAILED_TRANSFER, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION, TLA_REPORT_NO_SPECIFIC_REPORT, TLA_REPORT_SUCCESSFUL_TRANSFER,
};
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
    pending_pdch_handoffs: HashMap<Todo, PendingPacketDataHandoff>,
    pending_wap_responses: HashMap<Todo, PendingWapTransactionKey>,
    pending_wap_response_keys: HashMap<PendingWapTransactionKey, Todo>,
    started_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingPacketDataHandoff {
    issi: u32,
    endpoint_id: EndpointId,
    link_id: LinkId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PendingWapTransactionKey {
    issi: u32,
    endpoint_id: EndpointId,
    nsapi: u8,
    client_addr: [u8; 4],
    server_addr: [u8; 4],
    client_port: u16,
    server_port: u16,
    transaction_id: u16,
    kind: PendingWapTransactionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PendingWapTransactionKind {
    Connect,
    Resume,
    Status,
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
            pending_pdch_handoffs: HashMap::new(),
            pending_wap_responses: HashMap::new(),
            pending_wap_response_keys: HashMap::new(),
            started_at: Instant::now(),
        }
    }

    pub fn with_runtime_handoff_policy(config: SharedConfig, runtime_handoff: SndcpRuntimeHandoffPolicy) -> Self {
        Self {
            config,
            runtime_handoff,
            wap_pipeline: None,
            pending_pdch_handoffs: HashMap::new(),
            pending_wap_responses: HashMap::new(),
            pending_wap_response_keys: HashMap::new(),
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
                    "SNDCP: decoded SN-DATA/UNITDATA nsapi={} pcomp={} dcomp={} n_pdu_bits={} kind={:?}; no SN-SAP/IP/WAP handoff is implemented, dropping fail-closed",
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
                tracing::warn!("SNDCP: unsupported/reserved NSAPI {}, dropping SN-DATA/UNITDATA", nsapi);
            }
            SndcpDecode::UnsupportedCompression { pcomp, dcomp } => {
                tracing::warn!(
                    "SNDCP: unsupported SN-DATA/UNITDATA compression pcomp={} dcomp={}, dropping",
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
        if matches!(decode, SndcpDecode::TransferControl(SndcpTransferControl::DataTransmitRequest(_)))
            && let Ok(issi) = issi_from_ltpd_ind(&prim)
        {
            self.prepare_packet_data_retry_after_bearer_break(issi, prim.endpoint_id, prim.link_id);
        }
        let pending_wap_key = self.pending_wap_transaction_key_for_decode(&prim, decode);
        if let Some(key) = pending_wap_key
            && let Some(handle) = self.pending_wap_response_keys.get(&key)
        {
            tracing::warn!(
                "SNDCP/WAP-IP: suppressing duplicate WTP request while response handle={} is still pending key={:?}",
                handle,
                key
            );
            return;
        }
        let response = match self.wap_pipeline.as_mut() {
            Some(pipeline) => pipeline.handle_ltpd_mle_unitdata_ind_allocating_optional(&prim, &snapshot),
            None => {
                tracing::warn!("SNDCP/WAP-IP runtime is enabled but no WAP/IP pipeline is configured; dropping");
                return;
            }
        };

        let Some(mut response) = (match response {
            Ok(response) => response,
            Err(err) => {
                self.log_wap_pipeline_drop(&err);
                return;
            }
        }) else {
            tracing::debug!("SNDCP/WAP-IP: handled inbound SNDCP PDU with no response required");
            return;
        };

        if self.is_accepted_packet_data_handoff_response(decode, &response)
            && let Some(data_transmit) = data_transmit_request_for_packet_data_handoff(decode)
            && let Ok(issi) = issi_from_ltpd_ind(&prim)
        {
            let active_circuit_mode_service = snapshot.active_calls > 0;
            let packet_data_capacity_available = self.packet_data_handoff_capacity_available_for(issi);
            let parallel_voice_data_permitted = active_circuit_mode_service && packet_data_capacity_available;
            if !packet_data_capacity_available {
                tracing::warn!(
                    "SNDCP/WAP-IP: rejecting packet-data handoff issi={} nsapi={} endpoint={} link={} because no voice-safe PDCH slot is available",
                    issi,
                    data_transmit.nsapi,
                    prim.endpoint_id,
                    prim.link_id
                );
                if let Some(pipeline) = self.wap_pipeline.as_mut() {
                    pipeline
                        .pdch_mut()
                        .mark_common_control_on_link(issi, prim.endpoint_id, prim.link_id);
                }
                if let Err(err) = Self::reject_packet_data_handoff_response(
                    &mut response,
                    data_transmit.nsapi,
                    SndcpTransferRejectCause::SndcpServiceTemporarilyNotAvailable,
                ) {
                    self.log_wap_pipeline_drop(&err);
                    return;
                }
            } else {
                {
                    let Some(pipeline) = self.wap_pipeline.as_mut() else {
                        tracing::warn!("SNDCP/WAP-IP runtime is enabled but no WAP/IP pipeline is configured; dropping PDCH handoff");
                        return;
                    };
                    let retry_after_radio_resource_loss = pipeline
                        .pdch()
                        .session(issi)
                        .is_some_and(|session| session.state == SndcpPdchState::RadioResourceLost);
                    if matches!(decode, SndcpDecode::TransferControl(SndcpTransferControl::Reconnect(_))) || retry_after_radio_resource_loss
                    {
                        pipeline
                            .pdch_mut()
                            .mark_common_control_on_link(issi, prim.endpoint_id, prim.link_id);
                    }
                    if let Err(err) = pipeline.attach_mvp_pdch_allocation_for_data_transmit_response(
                        &mut response,
                        &prim,
                        issi,
                        &data_transmit,
                        active_circuit_mode_service,
                        parallel_voice_data_permitted,
                    ) {
                        self.log_wap_pipeline_drop(&err);
                        return;
                    }
                }
                self.track_pending_pdch_handoff(response.handle, issi, prim.endpoint_id, prim.link_id);
                if self.runtime_handoff.assume_pdch_ready_after_data_transmit() {
                    tracing::warn!(
                        "SNDCP/WAP-IP: unsafe compatibility mode marks PDCH ready before lower MLE-REPORT handle={} issi={} endpoint={} link={}",
                        response.handle,
                        issi,
                        prim.endpoint_id,
                        prim.link_id
                    );
                    if let Some(pipeline) = self.wap_pipeline.as_mut() {
                        pipeline.mark_pdch_ready(issi, prim.endpoint_id, prim.link_id);
                    }
                }
            }
        }

        if matches!(decode, SndcpDecode::TransferControl(SndcpTransferControl::EndOfData(_)))
            && let Ok(issi) = issi_from_ltpd_ind(&prim)
            && let Some(pipeline) = self.wap_pipeline.as_mut()
        {
            match pipeline.attach_common_control_allocation_for_end_of_data_response(&mut response, &prim, issi) {
                Ok(true) => {}
                Ok(false) => {
                    tracing::debug!(
                        "SNDCP/WAP-IP: SN-END OF DATA response for issi={} had no active PDCH allocation to release",
                        issi
                    );
                }
                Err(err) => {
                    self.log_wap_pipeline_drop(&err);
                    return;
                }
            }
        }

        tracing::info!(
            "SNDCP/WAP-IP: emitting {:?} response bits={} endpoint={} link={}",
            response.layer2service,
            response.sdu.get_len(),
            response.endpoint_id,
            response.link_id
        );
        if response.layer2service == Layer2Service::Acknowledged
            && let Some(key) = pending_wap_key
        {
            self.track_pending_wap_response(response.handle, key);
        }
        queue.push_back(SapMsg {
            sap: Sap::TlpdSap,
            src: TetraEntity::Sndcp,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LtpdMleUnitdataReq(response),
        });
    }

    fn is_accepted_packet_data_handoff_response(&self, decode: &SndcpDecode, response: &tetra_saps::ltpd::LtpdMleUnitdataReq) -> bool {
        if data_transmit_request_for_packet_data_handoff(decode).is_none() {
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

    fn track_pending_pdch_handoff(&mut self, handle: Todo, issi: u32, endpoint_id: EndpointId, link_id: LinkId) {
        if handle <= 0 {
            tracing::warn!(
                "SNDCP/WAP-IP: cannot track PDCH handoff with invalid MLE handle={} issi={} endpoint={} link={}",
                handle,
                issi,
                endpoint_id,
                link_id
            );
            return;
        }
        self.pending_pdch_handoffs.insert(
            handle,
            PendingPacketDataHandoff {
                issi,
                endpoint_id,
                link_id,
            },
        );
    }

    fn mark_pending_pdch_ready(&mut self, handle: Todo, pending: PendingPacketDataHandoff, reason: Todo) {
        let Some(pipeline) = self.wap_pipeline.as_mut() else {
            tracing::warn!(
                "SNDCP/WAP-IP: MLE-REPORT handle={} reason={} matched PDCH handoff, but WAP pipeline is disabled",
                handle,
                reason
            );
            return;
        };
        tracing::info!(
            "SNDCP/WAP-IP: PDCH ready after lower MLE-REPORT handle={} report={} issi={} endpoint={} link={}",
            handle,
            reason,
            pending.issi,
            pending.endpoint_id,
            pending.link_id
        );
        pipeline.mark_pdch_ready(pending.issi, pending.endpoint_id, pending.link_id);
    }

    fn pending_wap_transaction_key_for_decode(
        &self,
        prim: &tetra_saps::ltpd::LtpdMleUnitdataInd,
        decode: &SndcpDecode,
    ) -> Option<PendingWapTransactionKey> {
        let SndcpDecode::Unitdata(unitdata) = decode else {
            return None;
        };
        let policy = self.wap_pipeline.as_ref()?.session().wap_policy();
        let request_npdu = bitbuffer_npdu_octets(&unitdata.n_pdu).ok()?;
        let request_ip = parse_ipv4_packet(&request_npdu).ok()?;
        if request_ip.protocol != IPV4_PROTOCOL_UDP {
            return None;
        }
        let request_udp = parse_udp_datagram(request_ip.payload).ok()?;
        let request_kind = parse_wap_udp_request(request_udp.payload, policy).ok()?;
        let (transaction_id, kind) = match request_kind {
            WapUdpRequestKind::WtpWspConnect { transaction_id, .. } => (transaction_id, PendingWapTransactionKind::Connect),
            WapUdpRequestKind::WtpWspResume { transaction_id, .. } => (transaction_id, PendingWapTransactionKind::Resume),
            WapUdpRequestKind::WtpWspStatus { transaction_id, .. } => (transaction_id, PendingWapTransactionKind::Status),
            WapUdpRequestKind::Empty | WapUdpRequestKind::Status | WapUdpRequestKind::WtpControlNoResponse { .. } => return None,
        };
        Some(PendingWapTransactionKey {
            issi: prim.received_tetra_address.ssi,
            endpoint_id: prim.endpoint_id,
            nsapi: unitdata.nsapi,
            client_addr: request_ip.source,
            server_addr: request_ip.destination,
            client_port: request_udp.source_port,
            server_port: request_udp.destination_port,
            transaction_id,
            kind,
        })
    }

    fn track_pending_wap_response(&mut self, handle: Todo, key: PendingWapTransactionKey) {
        if handle <= 0 {
            tracing::warn!(
                "SNDCP/WAP-IP: cannot track pending WTP response with invalid handle={} key={:?}",
                handle,
                key
            );
            return;
        }
        self.pending_wap_responses.insert(handle, key);
        self.pending_wap_response_keys.insert(key, handle);
        tracing::debug!("SNDCP/WAP-IP: tracking pending WTP response handle={} key={:?}", handle, key);
    }

    fn finish_pending_wap_response(&mut self, handle: Todo, transfer_result: Todo) {
        let Some(key) = self.pending_wap_responses.remove(&handle) else {
            return;
        };
        self.pending_wap_response_keys.remove(&key);
        tracing::debug!(
            "SNDCP/WAP-IP: cleared pending WTP response handle={} transfer_result={} key={:?}",
            handle,
            transfer_result,
            key
        );
    }

    fn clear_pending_wap_responses_matching(&mut self, mut should_clear: impl FnMut(&PendingWapTransactionKey) -> bool) {
        let handles: Vec<_> = self
            .pending_wap_responses
            .iter()
            .filter_map(|(handle, key)| should_clear(key).then_some(*handle))
            .collect();
        for handle in handles {
            self.finish_pending_wap_response(handle, TLA_REPORT_FAILED_TRANSFER);
        }
    }

    fn rx_ltpd_mle_report_ind(&mut self, prim: LtpdMleReportInd) {
        match prim.transfer_result {
            TLA_REPORT_NO_SPECIFIC_REPORT => {
                tracing::debug!(
                    "SNDCP: received progress MLE-REPORT.ind handle={} transfer_result={}",
                    prim.handle,
                    prim.transfer_result
                );
            }
            TLA_REPORT_FIRST_COMPLETE_TRANSMISSION => {
                if let Some(pending) = self.pending_pdch_handoffs.get(&prim.handle).copied() {
                    self.mark_pending_pdch_ready(prim.handle, pending, prim.transfer_result);
                } else {
                    tracing::debug!(
                        "SNDCP: received first-complete MLE-REPORT.ind handle={} with no pending PDCH handoff",
                        prim.handle
                    );
                }
            }
            TLA_REPORT_SUCCESSFUL_TRANSFER => {
                self.finish_pending_wap_response(prim.handle, prim.transfer_result);
                if let Some(pending) = self.pending_pdch_handoffs.remove(&prim.handle) {
                    self.mark_pending_pdch_ready(prim.handle, pending, prim.transfer_result);
                } else {
                    tracing::debug!(
                        "SNDCP: received successful MLE-REPORT.ind handle={} with no pending PDCH handoff",
                        prim.handle
                    );
                }
            }
            TLA_REPORT_FAILED_TRANSFER => {
                self.finish_pending_wap_response(prim.handle, prim.transfer_result);
                if let Some(pending) = self.pending_pdch_handoffs.remove(&prim.handle) {
                    tracing::warn!(
                        "SNDCP/WAP-IP: PDCH handoff failed before lower completion handle={} issi={} endpoint={} link={}",
                        prim.handle,
                        pending.issi,
                        pending.endpoint_id,
                        pending.link_id
                    );
                } else {
                    tracing::debug!(
                        "SNDCP: received failed MLE-REPORT.ind handle={} with no pending PDCH handoff",
                        prim.handle
                    );
                }
            }
            other => {
                tracing::warn!("SNDCP: unsupported MLE-REPORT.ind handle={} transfer_result={}", prim.handle, other);
            }
        }
    }

    fn rx_ltpd_mle_configure_ind(&mut self, queue: &mut MessageQueue, prim: LtpdMleConfigureInd) {
        let Some(pipeline) = self.wap_pipeline.as_ref() else {
            tracing::debug!(
                "SNDCP/WAP-IP: dropping MLE-CONFIGURE.ind endpoint={} reason={} with WAP/IP pipeline disabled",
                prim.endpoint_id,
                prim.reason_for_config_indication
            );
            return;
        };
        let affected_issis = match prim.received_tetra_address {
            Some(addr) if addr.ssi_type == SsiType::Issi => {
                if pipeline.pdch().session(addr.ssi).is_some() {
                    vec![addr.ssi]
                } else {
                    Vec::new()
                }
            }
            Some(addr) => {
                tracing::debug!(
                    "SNDCP/WAP-IP: dropping MLE-CONFIGURE.ind endpoint={} reason={} for non-ISSI address {}",
                    prim.endpoint_id,
                    prim.reason_for_config_indication,
                    addr
                );
                return;
            }
            None => pipeline.pdch().session_issis_for_endpoint(prim.endpoint_id),
        };
        if affected_issis.is_empty() {
            tracing::debug!(
                "SNDCP/WAP-IP: MLE-CONFIGURE.ind endpoint={} reason={} had no matching PDCH session",
                prim.endpoint_id,
                prim.reason_for_config_indication
            );
            return;
        }

        if matches!(
            SndcpLtpdConfigureReason::from_todo(prim.reason_for_config_indication),
            SndcpLtpdConfigureReason::LossOfRadioResources
        ) {
            if let Some(addr) = prim.received_tetra_address.filter(|addr| addr.ssi_type == SsiType::Issi) {
                self.pending_pdch_handoffs.retain(|_, pending| pending.issi != addr.ssi);
                self.clear_pending_wap_responses_matching(|key| key.issi == addr.ssi);
            } else {
                self.pending_pdch_handoffs
                    .retain(|_, pending| pending.endpoint_id != prim.endpoint_id);
                self.clear_pending_wap_responses_matching(|key| key.endpoint_id == prim.endpoint_id);
            }
        }

        for issi in affected_issis {
            let status = self
                .wap_pipeline
                .as_ref()
                .map(|pipeline| SndcpStatusForMle::from(pipeline.session().state_for_issi(issi)))
                .unwrap_or(SndcpStatusForMle::Idle);
            let response = {
                let Some(pipeline) = self.wap_pipeline.as_mut() else {
                    return;
                };
                pipeline.pdch_mut().handle_ltpd_configure_ind_fail_closed(issi, &prim, status)
            };

            match response {
                Ok(Some(response)) => {
                    tracing::info!(
                        "SNDCP/WAP-IP: MLE-CONFIGURE.ind endpoint={} reason={} applied to issi={}; sending fail-closed MLE-CONFIGURE.req",
                        prim.endpoint_id,
                        prim.reason_for_config_indication,
                        issi
                    );
                    queue.push_back(SapMsg {
                        sap: Sap::TlpdSap,
                        src: TetraEntity::Sndcp,
                        dest: TetraEntity::Mle,
                        msg: SapMsgInner::LtpdMleConfigureReq(response),
                    });
                }
                Ok(None) => {
                    tracing::info!(
                        "SNDCP/WAP-IP: MLE-CONFIGURE.ind endpoint={} reason={} applied to issi={}",
                        prim.endpoint_id,
                        prim.reason_for_config_indication,
                        issi
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        "SNDCP/WAP-IP: failed to apply MLE-CONFIGURE.ind endpoint={} reason={} to issi={}: {:?}",
                        prim.endpoint_id,
                        prim.reason_for_config_indication,
                        issi,
                        err
                    );
                }
            }
        }
    }

    fn log_wap_pipeline_drop(&self, err: &SndcpWapLtpdPipelineError) {
        tracing::warn!("SNDCP/WAP-IP: dropping inbound SNDCP PDU: {:?}", err);
    }

    fn prepare_packet_data_retry_after_bearer_break(&mut self, issi: u32, endpoint_id: EndpointId, link_id: LinkId) {
        let ts2_owner = {
            let state = self.config.state_read();
            state.timeslot_alloc.owner(2)
        };
        let Some(pipeline) = self.wap_pipeline.as_mut() else {
            return;
        };
        let pdch_state = pipeline.pdch().session(issi).map(|session| session.state);
        let local_pdch_active = ts2_owner == Some(TimeslotOwner::PacketData) && pdch_state == Some(SndcpPdchState::PdchReady);
        if local_pdch_active {
            return;
        }

        if let Err(err) = pipeline.session_mut().mark_ready_bearer_temporarily_broken(issi) {
            tracing::warn!(
                "SNDCP/WAP-IP: failed to prepare packet-data retry after bearer break issi={} endpoint={} link={}: {:?}",
                issi,
                endpoint_id,
                link_id,
                err
            );
            return;
        }

        if ts2_owner != Some(TimeslotOwner::PacketData) || pdch_state == Some(SndcpPdchState::RadioResourceLost) {
            pipeline.pdch_mut().mark_common_control_on_link(issi, endpoint_id, link_id);
        }
    }

    fn rx_control(&mut self, message: SapMsg) {
        match message.msg {
            SapMsgInner::MmSubscriberUpdate(update) => {
                if update.action != BrewSubscriberAction::Deregister {
                    tracing::debug!(
                        "SNDCP/WAP-IP: ignoring subscriber lifecycle action {:?} for issi={}",
                        update.action,
                        update.issi
                    );
                    return;
                }

                let Some(pipeline) = self.wap_pipeline.as_mut() else {
                    tracing::debug!(
                        "SNDCP/WAP-IP: received deregister for issi={} with WAP/IP pipeline disabled",
                        update.issi
                    );
                    return;
                };

                if let Err(err) = pipeline.deregister_issi(update.issi) {
                    self.log_wap_pipeline_drop(&err);
                }
                self.pending_pdch_handoffs.retain(|_, pending| pending.issi != update.issi);
                self.clear_pending_wap_responses_matching(|key| key.issi == update.issi);
            }
            other => {
                tracing::warn!("SNDCP: dropping unexpected Control primitive {:?}", other);
            }
        }
    }

    fn packet_data_handoff_capacity_available_for(&self, issi: u32) -> bool {
        let state = self.config.state_read();
        let ts2_owner = state.timeslot_alloc.owner(2);
        if ts2_owner.is_none() {
            return true;
        }
        let packet_data_slot_exists = ts2_owner == Some(TimeslotOwner::PacketData);
        drop(state);

        packet_data_slot_exists
            && self
                .wap_pipeline
                .as_ref()
                .and_then(|pipeline| pipeline.pdch().session(issi))
                .is_some_and(|session| session.state == SndcpPdchState::PdchReady)
    }

    fn reject_packet_data_handoff_response(
        response: &mut tetra_saps::ltpd::LtpdMleUnitdataReq,
        nsapi: u8,
        cause: SndcpTransferRejectCause,
    ) -> Result<(), SndcpWapLtpdPipelineError> {
        response.sdu = encode_data_transmit_response(&SndcpDataTransmitResponse {
            nsapi,
            result: SndcpDataTransmitResponseResult::Rejected(cause),
        })
        .map_err(SndcpWapSessionError::from)?;
        response.packet_data_flag = false;
        response.chan_alloc = None;
        Ok(())
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
            last_activity: crate::net_dashboard::state::latest_last_heard_entry()
                .map(|entry| super::wap_dashboard::last_activity_text(&entry)),
            health_summary: Some(health_summary(&health)),
            health_lines: health_lines_from_health(&health),
            radio_lines: registered_issis.into_iter().take(3).map(|issi| format!("MS {issi}")).collect(),
            call_lines: call_lines_from_health(&health),
        }
    }
}

fn data_transmit_request_for_packet_data_handoff(decode: &SndcpDecode) -> Option<SndcpDataTransmitRequest> {
    match decode {
        SndcpDecode::TransferControl(SndcpTransferControl::DataTransmitRequest(request)) => Some(request.clone()),
        SndcpDecode::TransferControl(SndcpTransferControl::Reconnect(reconnect)) => reconnect.data_transmit_request(),
        _ => None,
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
        SN_PDU_TYPE_UNITDATA | SN_PDU_TYPE_DATA => {}
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
        match message.sap {
            Sap::TlpdSap => match message.msg {
                SapMsgInner::LtpdMleUnitdataInd(prim) => self.rx_ltpd_mle_unitdata_ind(_queue, prim),
                SapMsgInner::LtpdMleReportInd(prim) => self.rx_ltpd_mle_report_ind(prim),
                SapMsgInner::LtpdMleConfigureInd(prim) => self.rx_ltpd_mle_configure_ind(_queue, prim),
                other => {
                    tracing::warn!("SNDCP: dropping unexpected LTPD primitive {:?}", other);
                }
            },
            Sap::Control => self.rx_control(message),
            other => tracing::warn!("SNDCP: dropping unexpected {:?} primitive", other),
        }
    }
}
