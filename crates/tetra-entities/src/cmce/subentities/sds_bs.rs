use std::collections::{HashMap, VecDeque};

use tetra_config::bluestation::SharedConfig;
use tetra_core::Layer2Service;
use tetra_core::{BitBuffer, Sap, SsiType, TdmaTime, TetraAddress, TxReporter, tetra_entities::TetraEntity, unimplemented_log};
use tetra_pdus::cmce::enums::cmce_pdu_type_ul::CmcePduTypeUl;
use tetra_pdus::cmce::enums::pre_coded_status::PreCodedStatus;
use tetra_pdus::cmce::enums::short_report_type::ShortReportType;
use tetra_saps::control::enums::sds_user_data::SdsUserData;
use tetra_saps::control::sds::{CmceSdsData, CmceSdsStatus};
use tetra_saps::lcmc::LcmcMleUnitdataReq;
use tetra_saps::{SapMsg, SapMsgInner};

use tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier;
use tetra_pdus::cmce::pdus::cmce_function_not_supported::CmceFunctionNotSupported;
use tetra_pdus::cmce::pdus::d_sds_data::DSdsData;
use tetra_pdus::cmce::pdus::d_status::DStatus;
use tetra_pdus::cmce::pdus::u_sds_data::USdsData;
use tetra_pdus::cmce::pdus::u_status::UStatus;

use super::home_mode_display::HomeModeDisplaySender;
use crate::MessageQueue;
use crate::net_brew;
use crate::net_control::ControlCommand;
use crate::net_telemetry::{TelemetryEvent, TelemetrySink};

/// Clause 13 Short Data Service CMCE sub-entity
/// Actions that sds_bs cannot execute itself (need access to CcBsSubentity or system),
/// queued during U-STATUS processing and drained by CmceBs::tick_start.
#[derive(Debug, Clone)]
pub enum SdsPendingAction {
    KickAll,
}

const MAX_AIR_INTERFACE_SSI: u32 = 0x00FF_FFFF;
const SDS_TL_TEXT_PROTOCOL_ID: u8 = 0x82;
const SDS_TL_TRANSFER_NO_REPORT_REQ: u8 = 0x00;
const SDS_TL_TRANSFER_WITH_REPORT_REQ: u8 = 0x04;
const SDS_TL_TRANSFER_REPORT_REQ_MASK: u8 = 0x0C;
const SDS_TL_TEXT_CODING_LATIN_1: u8 = 0x01;
const SDS_TL_TEXT_CODING_UCS2: u8 = 0x1A;
const SDS_TYPE4_MAX_BITS: u16 = 0x07FF;
const SDS_TL_REPORT_RECEIVED_MASK: u8 = 0x01;
const SDS_TL_REPORT_CONSUMED_MASK: u8 = 0x02;
// SDS-SHORT REPORT carries an 8-bit message reference. Keep one complete
// reference space worth of remembered SDS-TL PIDs and evict older contexts.
const SDS_TL_REPORT_CONTEXT_LIMIT: usize = 256;

#[derive(Debug, Clone, Copy)]
struct ControlSdsRoute {
    air_source_ssi: u32,
    dest_ssi_type: SsiType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SdsTlReportContextKey {
    reporting_issi: u32,
    peer_issi: u32,
    message_reference: u8,
}

#[derive(Debug, Clone, Copy)]
struct SdsTlReportContext {
    protocol_id: u8,
    requested_report_mask: u8,
    converted_report_mask: u8,
}

pub struct SdsBsSubentity {
    config: SharedConfig,
    telemetry: Option<TelemetrySink>,
    home_mode_display_sender: HomeModeDisplaySender,
    sds_broadcast_sender: HomeModeDisplaySender,
    live_sds_sender: HomeModeDisplaySender,
    pub pending_actions: Vec<SdsPendingAction>,
    /// Control-command sender used to re-inject WX/METAR replies into the stack from the
    /// background fetch thread. Cloned from the CMCE command dispatcher at startup. When
    /// None (no control links), the WX responder still works for nothing — replies need
    /// this channel — so it is wired in main.rs alongside the dashboard sender.
    wx_cmd_tx: Option<crossbeam_channel::Sender<ControlCommand>>,
    /// Monotonic timestamp of the last periodic WX auto-send, to rate-limit the broadcast.
    last_periodic_wx: Option<std::time::Instant>,
    sds_tl_report_context: HashMap<SdsTlReportContextKey, SdsTlReportContext>,
    sds_tl_report_context_order: VecDeque<SdsTlReportContextKey>,
}

impl SdsBsSubentity {
    pub fn new(config: SharedConfig) -> Self {
        SdsBsSubentity {
            config,
            telemetry: None,
            home_mode_display_sender: HomeModeDisplaySender::new(),
            sds_broadcast_sender: HomeModeDisplaySender::new(),
            live_sds_sender: HomeModeDisplaySender::new(),
            pending_actions: Vec::new(),
            wx_cmd_tx: None,
            last_periodic_wx: None,
            sds_tl_report_context: HashMap::new(),
            sds_tl_report_context_order: VecDeque::new(),
        }
    }

    pub fn set_telemetry(&mut self, sink: TelemetrySink) {
        self.telemetry = Some(sink);
    }

    /// Provide the control-command sender used to deliver WX/METAR replies.
    pub fn set_wx_cmd_sender(&mut self, tx: crossbeam_channel::Sender<ControlCommand>) {
        self.wx_cmd_tx = Some(tx);
    }

    pub fn shared_config(&self) -> &SharedConfig {
        &self.config
    }

    fn emit(&self, event: TelemetryEvent) {
        if let Some(sink) = &self.telemetry {
            sink.send(event);
        }
    }

    fn valid_air_interface_ssi(ssi: u32) -> bool {
        ssi <= MAX_AIR_INTERFACE_SSI
    }

    fn rf_calling_party_issi(address: TetraAddress) -> Option<u32> {
        if address.ssi_type != SsiType::Issi || !Self::valid_air_interface_ssi(address.ssi) {
            // EN 300 392-2 clauses 13.2, 14.7.2.7 and 14.7.2.8 carry
            // mobile-originated SDS/status from an individual MS. The RF source
            // indication is the only local calling-party identity for these
            // uplinks, so reject non-ISSI or out-of-range sources before local,
            // WX or Brew forwarding side effects.
            tracing::warn!("SDS: rejecting RF uplink from invalid calling party {}", address);
            return None;
        }
        Some(address.ssi)
    }

    fn registered_rf_calling_party_issi(&self, address: TetraAddress) -> Option<u32> {
        let source_issi = Self::rf_calling_party_issi(address)?;
        if !self.config.state_read().subscribers.is_registered(source_issi) {
            // EN 300 392-2 clauses 13.3.2.1, 13.3.2.3, 14.7.2.7 and
            // 14.7.2.8 treat mobile-originated SDS/status as MS service
            // requests. This SwMI only routes those uplinks after MM has put
            // the calling ISSI into the local registration state (clause 16.4).
            tracing::warn!("SDS: rejecting RF uplink from unregistered calling party ISSI {}", source_issi);
            return None;
        }
        Some(source_issi)
    }

    fn is_sds_tl_transport_protocol_id(protocol_id: u8) -> bool {
        // EN 300 392-2 clause 29.4.1: protocol identifiers 0x80..=0xFE use
        // the SDS-TL transport PDUs described in clause 29. 0x00..=0x7F do
        // not, so their second octet must not be treated as SDS-TL flags.
        (0x80..=0xFE).contains(&protocol_id)
    }

    fn system_broadcast_source_issi(source_issi: u32, dest_ssi: u32) -> u32 {
        if dest_ssi == MAX_AIR_INTERFACE_SSI {
            // EN 300 392-2 clause 29.3.3.8.2: SwMI-generated SDS-TL
            // system broadcast should use the all-ones source address.
            MAX_AIR_INTERFACE_SSI
        } else {
            source_issi
        }
    }

    fn build_cmce_function_not_supported_direct(
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
            .expect("serialize CMCE FUNCTION NOT SUPPORTED for SDS unsupported feature");
        sdu.seek(0);

        SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle,
                endpoint_id,
                link_id,
                // EN 300 392-2 clauses 14.7.3.1/14.7.3.2: this is a
                // direct CMCE FUNCTION NOT SUPPORTED response to the MS that
                // sent an individually addressed unsupported SDS/status PDU.
                layer2service: Layer2Service::Acknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                chan_alloc: None,
                main_address: target,
                tx_reporter: None,
            }),
        }
    }

    fn control_sds_air_route(&self, command_name: &str, source_ssi: u32, dest_ssi: u32, dest_is_group: bool) -> Option<ControlSdsRoute> {
        if !Self::valid_air_interface_ssi(source_ssi) || !Self::valid_air_interface_ssi(dest_ssi) {
            tracing::warn!(
                "SDS: rejecting {} with invalid 24-bit SSI source={} dest={}",
                command_name,
                source_ssi,
                dest_ssi
            );
            return None;
        }

        let broadcast_dest = dest_ssi == MAX_AIR_INTERFACE_SSI;
        let dest_ssi_type = if broadcast_dest || dest_is_group {
            SsiType::Gssi
        } else {
            SsiType::Issi
        };

        {
            let state = self.config.state_read();
            match dest_ssi_type {
                SsiType::Issi if !state.subscribers.is_registered(dest_ssi) => {
                    // EN 300 392-2 clauses 13.2, 13.3.2.1 and 13.3.2.3
                    // distinguish individual SDS delivery from group
                    // delivery. A control-origin individual SDS is only
                    // locally deliverable after MM registration has made the
                    // destination ISSI known to this SwMI.
                    tracing::warn!("SDS: rejecting {} to unregistered ISSI destination {}", command_name, dest_ssi);
                    return None;
                }
                SsiType::Gssi if !broadcast_dest && !state.subscribers.has_group_members(dest_ssi) => {
                    // EN 300 392-2 clause 13.2 includes group SDS, and
                    // clause 23.4.1.2.1 note 3 reserves only the all-ones
                    // GSSI as broadcast. Non-broadcast local GSSI delivery
                    // needs at least one affiliated local member.
                    tracing::warn!("SDS: rejecting {} to GSSI {} without local group members", command_name, dest_ssi);
                    return None;
                }
                _ => {}
            }
        }

        let air_source_ssi = if broadcast_dest {
            // EN 300 392-2 29.3.3.8.2 says system broadcast messages sent to
            // 0xFFFFFF should indicate the broadcast source address too.
            MAX_AIR_INTERFACE_SSI
        } else {
            source_ssi
        };

        Some(ControlSdsRoute {
            air_source_ssi,
            dest_ssi_type,
        })
    }

    fn clear_sds_tl_delivery_report_request(data: &mut [u8]) {
        if data.len() >= 2 && Self::is_sds_tl_transport_protocol_id(data[0]) && data[1] & 0xF0 == SDS_TL_TRANSFER_NO_REPORT_REQ {
            data[1] &= !SDS_TL_TRANSFER_REPORT_REQ_MASK;
        }
    }

    fn sds_tl_without_delivery_report(user_defined_data: SdsUserData) -> SdsUserData {
        match user_defined_data {
            SdsUserData::Type4(len_bits, mut data) => {
                if let Some(canonical) = SdsUserData::canonical_type4_bytes(len_bits, &data) {
                    data = canonical;
                }
                Self::clear_sds_tl_delivery_report_request(&mut data);
                SdsUserData::Type4(len_bits, data)
            }
            other => other,
        }
    }

    fn sds_tl_report_request_mask(report_request: u8) -> u8 {
        match report_request & SDS_TL_TRANSFER_REPORT_REQ_MASK {
            0x04 => SDS_TL_REPORT_RECEIVED_MASK,
            0x08 => SDS_TL_REPORT_CONSUMED_MASK,
            0x0C => SDS_TL_REPORT_RECEIVED_MASK | SDS_TL_REPORT_CONSUMED_MASK,
            _ => 0,
        }
    }

    fn sds_tl_short_report_mask(short_report_type: ShortReportType) -> u8 {
        match short_report_type {
            ShortReportType::MessageReceived => SDS_TL_REPORT_RECEIVED_MASK,
            ShortReportType::MessageConsumed => SDS_TL_REPORT_CONSUMED_MASK,
            ShortReportType::DestMemFull | ShortReportType::ProtOrEncodingNotSupported => {
                SDS_TL_REPORT_RECEIVED_MASK | SDS_TL_REPORT_CONSUMED_MASK
            }
        }
    }

    fn sds_tl_transfer_report_context(user_defined_data: &SdsUserData) -> Option<(u8, u8, u8)> {
        let SdsUserData::Type4(len_bits, data) = user_defined_data else {
            return None;
        };
        if *len_bits < 24 {
            return None;
        }
        let data = SdsUserData::canonical_type4_bytes(*len_bits, data)?;
        if data.len() < 3 {
            return None;
        }
        if !Self::is_sds_tl_transport_protocol_id(data[0]) {
            return None;
        }

        let message_type = data[1] & 0xF0;
        let report_request = data[1] & SDS_TL_TRANSFER_REPORT_REQ_MASK;
        let requested_report_mask = Self::sds_tl_report_request_mask(report_request);
        if message_type == SDS_TL_TRANSFER_NO_REPORT_REQ && requested_report_mask != 0 {
            Some((data[0], data[2], requested_report_mask))
        } else {
            None
        }
    }

    fn remember_sds_tl_report_context(&mut self, source_issi: u32, dest_ssi: u32, dest_ssi_type: SsiType, user_defined_data: &SdsUserData) {
        if dest_ssi_type != SsiType::Issi {
            return;
        }

        let Some((protocol_id, message_reference, requested_report_mask)) = Self::sds_tl_transfer_report_context(user_defined_data) else {
            return;
        };
        let key = SdsTlReportContextKey {
            reporting_issi: dest_ssi,
            peer_issi: source_issi,
            message_reference,
        };

        if self
            .sds_tl_report_context
            .insert(
                key,
                SdsTlReportContext {
                    protocol_id,
                    requested_report_mask,
                    converted_report_mask: 0,
                },
            )
            .is_some()
        {
            self.sds_tl_report_context_order.retain(|existing| *existing != key);
        }
        self.sds_tl_report_context_order.push_back(key);

        while self.sds_tl_report_context.len() > SDS_TL_REPORT_CONTEXT_LIMIT {
            let Some(oldest) = self.sds_tl_report_context_order.pop_front() else {
                break;
            };
            if oldest != key {
                self.sds_tl_report_context.remove(&oldest);
            }
        }
    }

    fn take_sds_tl_report_protocol_id(
        &mut self,
        reporting_issi: u32,
        peer_issi: u32,
        message_reference: u8,
        short_report_type: ShortReportType,
    ) -> Option<u8> {
        let key = SdsTlReportContextKey {
            reporting_issi,
            peer_issi,
            message_reference,
        };
        let report_mask = Self::sds_tl_short_report_mask(short_report_type);
        let (protocol_id, remove_context) = {
            let context = self.sds_tl_report_context.get_mut(&key)?;
            context.converted_report_mask |= report_mask;
            let requested_reports_complete =
                (context.converted_report_mask & context.requested_report_mask) == context.requested_report_mask;
            let report_was_not_requested = report_mask & context.requested_report_mask == 0;
            let terminal_negative_report = matches!(
                short_report_type,
                ShortReportType::DestMemFull | ShortReportType::ProtOrEncodingNotSupported
            );
            (
                context.protocol_id,
                requested_reports_complete || report_was_not_requested || terminal_negative_report,
            )
        };
        if remove_context {
            self.sds_tl_report_context_order.retain(|existing| *existing != key);
            self.sds_tl_report_context.remove(&key);
        }
        Some(protocol_id)
    }

    /// Called every tick from CmceBs::tick_start. Fires Home Mode Display broadcast when due.
    pub fn tick_start(&mut self, queue: &mut MessageQueue, dltime: TdmaTime) {
        if let Some(hmd_tx) = self.home_mode_display_sender.tick_start(&self.config, dltime) {
            self.send_d_sds_data(
                queue,
                Self::system_broadcast_source_issi(hmd_tx.source_issi, hmd_tx.dest_gssi),
                hmd_tx.dest_gssi,
                SsiType::Gssi,
                hmd_tx.payload,
            );
        }
        if let Some(tx) = self.sds_broadcast_sender.tick_start_broadcast(&self.config, dltime) {
            self.send_d_sds_data(
                queue,
                Self::system_broadcast_source_issi(tx.source_issi, tx.dest_gssi),
                tx.dest_gssi,
                SsiType::Gssi,
                tx.payload,
            );
        }
        if let Some(tx) = self.live_sds_sender.tick_live_sds(&self.config, dltime) {
            self.send_d_sds_data(
                queue,
                Self::system_broadcast_source_issi(tx.source_issi, tx.dest_gssi),
                tx.dest_gssi,
                SsiType::Gssi,
                tx.payload,
            );
        }
    }

    /// Handle incoming U-SDS-DATA from a local MS (via RF uplink)
    pub fn route_rf_deliver(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("SDS route_rf_deliver");

        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let calling_party = prim.received_tetra_address;
        let Some(source_ssi) = self.registered_rf_calling_party_issi(calling_party) else {
            return;
        };

        let pdu = match USdsData::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-SDS-DATA: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        if !Self::feature_check_u_sds_data(&pdu) {
            // EN 300 392-2 clauses 14.7.2.8 and 14.7.3.2/table 14.33:
            // respond to an individually addressed CMCE U-SDS-DATA that uses
            // an unsupported destination addressing form. Pointer 0 marks the
            // unsupported U-SDS-DATA service variant rather than guessing a
            // bit pointer for the address element.
            tracing::warn!("Unsupported features in U-SDS-DATA, returning CMCE FUNCTION NOT SUPPORTED");
            queue.push_back(Self::build_cmce_function_not_supported_direct(
                CmcePduTypeUl::USdsData,
                calling_party,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
            ));
            return;
        }

        // Extract destination SSI (guaranteed present after feature check)
        let Some(dest_ssi_raw) = pdu.called_party_ssi else {
            tracing::warn!("SDS: U-SDS-DATA missing called_party_ssi after feature check, dropping");
            return;
        };
        let dest_ssi = dest_ssi_raw as u32;

        tracing::info!(
            "SDS: U-SDS-DATA from ISSI {} to ISSI {}, type={}",
            source_ssi,
            dest_ssi,
            pdu.user_defined_data.type_identifier()
        );

        // Built-in WX/METAR service: if this SDS is addressed to the configured service
        // ISSI and the responder is enabled, treat the text as a weather command, fetch
        // asynchronously, and reply to the sender. Consumed locally (not routed onward).
        let wx = self.config.effective_wx_service();
        if wx.enabled && dest_ssi == wx.service_issi {
            // An SDS-TL SHORT REPORT / STATUS (PID 0x82/0x89, message-type byte 0x10) is a
            // delivery confirmation for a reply we already sent — never a fresh request.
            // Feeding it back into the responder produced an infinite SDS storm: each reply
            // requests a delivery report, the terminal returns one, and its message-reference
            // byte decoded as a single-character "command" that triggered yet another reply.
            // tetraflow-sds-bot guards against this in handle_downlink_sds / parse_text_payload
            // by rejecting data[1] == 0x10; mirror that here and absorb the report.
            if Self::is_sds_tl_report(&pdu.user_defined_data) {
                tracing::debug!("SDS: absorbing SDS-TL delivery report to WX service from ISSI {}", source_ssi);
                return;
            }
            // Delivery confirmation, identical to tetraflow-sds-bot's queue_u_status: before
            // answering, send an SDS-TL SHORT REPORT back to the requester so the terminal
            // marks its outgoing message as delivered. The report echoes the request's
            // message-reference byte and carries [0x82, 0x10, 0x00, MR], from the service
            // ISSI to the requester.
            if let Some(mr) = Self::sds_tl_message_reference(&pdu.user_defined_data) {
                let report = SdsUserData::Type4(32, vec![0x82u8, 0x10u8, 0x00u8, mr]);
                self.send_d_sds_data(queue, wx.service_issi, source_ssi, SsiType::Issi, report);
            }
            self.handle_wx_request(source_ssi, &pdu.user_defined_data);
            self.emit(TelemetryEvent::SdsActivity {
                source_issi: source_ssi,
                dest_issi: dest_ssi,
            });
            return;
        }

        // Route: local delivery (ISSI or GSSI), Brew forward, or drop
        let is_broadcast_group = dest_ssi == MAX_AIR_INTERFACE_SSI;
        let (is_local_issi, is_local_group) = {
            let state = self.config.state_read();
            (
                !is_broadcast_group && state.subscribers.is_registered(dest_ssi),
                is_broadcast_group || state.subscribers.has_group_members(dest_ssi),
            )
        };

        if is_local_issi && is_local_group {
            // EN 300 392-2 clause 13.2 distinguishes individual and group SDS
            // services. With only a numeric SSI and no explicit internal
            // destination kind, choosing ISSI first could deliver a group SDS as
            // an individual acknowledged SDS.
            tracing::warn!(
                "SDS: ambiguous U-SDS-DATA destination {} is both local ISSI and local GSSI; dropping",
                dest_ssi
            );
            return;
        }

        if is_local_issi {
            tracing::info!("SDS: local delivery: {} -> {}", source_ssi, dest_ssi);
            self.send_d_sds_data(queue, source_ssi, dest_ssi, SsiType::Issi, pdu.user_defined_data);
            self.emit(TelemetryEvent::SdsActivity {
                source_issi: source_ssi,
                dest_issi: dest_ssi,
            });
        } else if is_local_group {
            tracing::info!("SDS: group delivery: {} -> GSSI {}", source_ssi, dest_ssi);
            self.send_d_sds_data(queue, source_ssi, dest_ssi, SsiType::Gssi, pdu.user_defined_data);
            self.emit(TelemetryEvent::SdsActivity {
                source_issi: source_ssi,
                dest_issi: dest_ssi,
            });
        } else if net_brew::feature_sds_enabled(&self.config) {
            tracing::info!("SDS: forwarding to Brew: {} -> {}", source_ssi, dest_ssi);
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceSdsData(CmceSdsData {
                    source_issi: source_ssi,
                    dest_issi: dest_ssi,
                    dest_ssi_type: None,
                    user_defined_data: pdu.user_defined_data,
                    tx_reporter: None,
                }),
            });
        } else {
            tracing::warn!("SDS: dest SSI {} not local and not Brew-routable, dropping", dest_ssi);
        }
    }

    /// Handle incoming SDS data from Brew entity (network-originated SDS)
    pub fn rx_sds_from_brew(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        let SapMsgInner::CmceSdsData(sds) = message.msg else {
            tracing::error!("SDS: rx_sds_from_brew expected CmceSdsData, got unexpected message type");
            return;
        };

        tracing::info!(
            "SDS: received from Brew: {} -> {}, type={}, {} bits",
            sds.source_issi,
            sds.dest_issi,
            sds.user_defined_data.type_identifier(),
            sds.user_defined_data.length_bits()
        );

        if !Self::valid_air_interface_ssi(sds.source_issi) || !Self::valid_air_interface_ssi(sds.dest_issi) {
            tracing::warn!(
                "SDS: rejecting Brew SDS with invalid 24-bit SSI source={} dest={}",
                sds.source_issi,
                sds.dest_issi
            );
            Self::discard_sds_reporter(sds.tx_reporter);
            return;
        }

        let broadcast_dest = sds.dest_issi == MAX_AIR_INTERFACE_SSI;
        let (air_source_ssi, dest_ssi_type, user_defined_data) = if broadcast_dest {
            (
                MAX_AIR_INTERFACE_SSI,
                SsiType::Gssi,
                Self::sds_tl_without_delivery_report(sds.user_defined_data),
            )
        } else {
            let state = self.config.state_read();
            let is_local_issi = state.subscribers.is_registered(sds.dest_issi);
            let is_local_group = state.subscribers.has_group_members(sds.dest_issi);
            match sds.dest_ssi_type {
                Some(SsiType::Issi) if is_local_issi => (sds.source_issi, SsiType::Issi, sds.user_defined_data),
                Some(SsiType::Issi) => {
                    tracing::warn!(
                        "SDS: explicit ISSI Brew SDS dest {} is not locally registered, dropping",
                        sds.dest_issi
                    );
                    Self::discard_sds_reporter(sds.tx_reporter);
                    return;
                }
                Some(SsiType::Gssi) if is_local_group => (sds.source_issi, SsiType::Gssi, sds.user_defined_data),
                Some(SsiType::Gssi) => {
                    tracing::warn!("SDS: explicit GSSI Brew SDS dest {} has no local members, dropping", sds.dest_issi);
                    Self::discard_sds_reporter(sds.tx_reporter);
                    return;
                }
                Some(other) => {
                    tracing::warn!("SDS: unsupported explicit Brew SDS destination type {:?}, dropping", other);
                    Self::discard_sds_reporter(sds.tx_reporter);
                    return;
                }
                None if is_local_issi && is_local_group => {
                    tracing::warn!(
                        "SDS: Brew SDS dest SSI {} is both local ISSI and local GSSI with members, dropping ambiguous destination",
                        sds.dest_issi
                    );
                    Self::discard_sds_reporter(sds.tx_reporter);
                    return;
                }
                None if is_local_issi => (sds.source_issi, SsiType::Issi, sds.user_defined_data),
                None if is_local_group => {
                    // EN 300 392-2 clause 13.2 includes mobile-terminated user-defined
                    // group short messages. Legacy Brew SDS carries only the 24-bit SSI,
                    // so local routing resolves a non-ISSI destination with group members as GSSI.
                    (sds.source_issi, SsiType::Gssi, sds.user_defined_data)
                }
                None => {
                    tracing::warn!(
                        "SDS: dest SSI {} from Brew is neither a local ISSI nor a local GSSI with members, dropping",
                        sds.dest_issi
                    );
                    Self::discard_sds_reporter(sds.tx_reporter);
                    return;
                }
            }
        };

        if dest_ssi_type == SsiType::Gssi {
            tracing::debug!("SDS: routing Brew SDS to local GSSI {}", sds.dest_issi);
        }

        // Send D-SDS-DATA downlink to the local MS/group. Schedule on next ts1 to ensure it gets sent on the MCCH.
        self.send_d_sds_data_with_reporter(
            queue,
            air_source_ssi,
            sds.dest_issi,
            dest_ssi_type,
            user_defined_data,
            sds.tx_reporter,
        );
    }

    /// Handle incoming pre-coded status from Brew/control (network-originated status).
    pub fn rx_status_from_control(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        let SapMsgInner::CmceSdsStatus(status) = message.msg else {
            tracing::error!("SDS: rx_status_from_control expected CmceSdsStatus, got unexpected message type");
            return;
        };

        let _ = self.route_status_from_control_fields(queue, status);
    }

    pub fn rx_status_command_from_control(&mut self, queue: &mut MessageQueue, message: ControlCommand) -> bool {
        let ControlCommand::SendStatus {
            handle,
            source_ssi,
            dest_ssi,
            dest_is_group,
            status_number,
        } = message
        else {
            tracing::error!("SDS: rx_status_command_from_control expected SendStatus command, got unexpected command type");
            return false;
        };

        tracing::info!(
            "SDS-STATUS: received status from Control {}: {} -> {}, type={}, status={}",
            handle,
            source_ssi,
            dest_ssi,
            dest_is_group.then(|| "GSSI").unwrap_or("ISSI"),
            status_number
        );

        let dest_ssi_type = if dest_is_group || dest_ssi == MAX_AIR_INTERFACE_SSI {
            SsiType::Gssi
        } else {
            SsiType::Issi
        };

        // EN 300 392-2 clauses 13.2 and 14.7.1.11 define pre-defined status
        // as D-STATUS. Keep it separate from Control SendRawSds sdti=0, which
        // is SDS Type1 user-defined data and serializes as D-SDS-DATA.
        self.route_status_from_control_fields(
            queue,
            CmceSdsStatus {
                source_issi: source_ssi,
                dest_issi: dest_ssi,
                dest_ssi_type,
                status_number,
                tx_reporter: None,
            },
        )
    }

    fn discard_status_reporter(tx_reporter: Option<TxReporter>) {
        if let Some(reporter) = tx_reporter {
            reporter.mark_discarded();
        }
    }

    fn discard_sds_reporter(tx_reporter: Option<TxReporter>) {
        if let Some(reporter) = tx_reporter {
            reporter.mark_discarded();
        }
    }

    fn route_status_from_control_fields(&self, queue: &mut MessageQueue, status: CmceSdsStatus) -> bool {
        let CmceSdsStatus {
            source_issi,
            dest_issi,
            dest_ssi_type,
            status_number,
            tx_reporter,
        } = status;
        tracing::info!(
            "SDS-STATUS: received from Control/Brew: {} -> {}:{}, status={}",
            source_issi,
            dest_ssi_type,
            dest_issi,
            status_number
        );

        if !Self::valid_air_interface_ssi(source_issi) || !Self::valid_air_interface_ssi(dest_issi) {
            tracing::warn!(
                "SDS-STATUS: rejecting network-origin status with invalid 24-bit SSI source={} dest={}",
                source_issi,
                dest_issi
            );
            Self::discard_status_reporter(tx_reporter);
            return false;
        }

        let pre_coded_status = PreCodedStatus::from(status_number);

        match dest_ssi_type {
            SsiType::Issi => {
                if dest_issi == MAX_AIR_INTERFACE_SSI || !self.config.state_read().subscribers.is_registered(dest_issi) {
                    tracing::warn!(
                        "SDS-STATUS: network-origin ISSI destination {} is not locally registered, dropping",
                        dest_issi
                    );
                    Self::discard_status_reporter(tx_reporter);
                    return false;
                }
                self.send_d_status(queue, source_issi, dest_issi, SsiType::Issi, pre_coded_status, tx_reporter);
                true
            }
            SsiType::Gssi => {
                if dest_issi != MAX_AIR_INTERFACE_SSI && !self.config.state_read().subscribers.has_group_members(dest_issi) {
                    tracing::warn!(
                        "SDS-STATUS: network-origin GSSI destination {} has no local group members, dropping",
                        dest_issi
                    );
                    Self::discard_status_reporter(tx_reporter);
                    return false;
                }
                let air_source_issi = if dest_issi == MAX_AIR_INTERFACE_SSI {
                    // EN 300 392-2 defines the all-ones SSI as the predefined
                    // broadcast group address. Mirror the SDS-TL broadcast
                    // source convention so over-air broadcast D-STATUS does
                    // not expose an arbitrary controller ISSI.
                    MAX_AIR_INTERFACE_SSI
                } else {
                    source_issi
                };
                self.send_d_status(queue, air_source_issi, dest_issi, SsiType::Gssi, pre_coded_status, tx_reporter);
                true
            }
            other => {
                tracing::warn!(
                    "SDS-STATUS: network-origin destination {} uses unsupported address kind {}, dropping",
                    dest_issi,
                    other
                );
                Self::discard_status_reporter(tx_reporter);
                false
            }
        }
    }

    /// Handle incoming SDS data from Control entity (network-originated SDS)
    pub fn rx_sds_from_control(&mut self, queue: &mut MessageQueue, message: ControlCommand) -> bool {
        let ControlCommand::SendSds {
            handle,
            source_ssi,
            dest_ssi,
            dest_is_group,
            len_bits,
            payload,
        } = message
        else {
            tracing::error!("SDS: rx_sds_from_control expected SendSds command, got unexpected command type");
            return false;
        };

        tracing::info!(
            "SDS: received from Control {}: {} -> {}, type={}, {} bits",
            handle,
            source_ssi,
            dest_ssi,
            dest_is_group.then(|| "GSSI").unwrap_or("ISSI"),
            len_bits
        );

        let Some(route) = self.control_sds_air_route("Control SendSds", source_ssi, dest_ssi, dest_is_group) else {
            return false;
        };

        let broadcast_dest = dest_ssi == MAX_AIR_INTERFACE_SSI;

        if len_bits as usize != payload.len() * 8 {
            tracing::warn!(
                "SDS: rejecting Control SendSds with non-byte-aligned or mismatched len_bits={} for {} payload byte(s); use SendRawSds for raw Type1..Type4 SDS",
                len_bits,
                payload.len()
            );
            return false;
        }

        // EN 300 392-2 clause 29.4.2.4 carries SDS-TL text as a Type4
        // SDS-TRANSFER. Table 29.29 defines one text-coding octet before the
        // text body. Control SendSds payloads are bare text, never pre-wrapped.
        static SDS_MR: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);
        let mr = {
            let v = SDS_MR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if v == 0 {
                SDS_MR.store(1, std::sync::atomic::Ordering::Relaxed);
                1
            } else {
                v
            }
        };
        let text_user_data = Self::encode_control_text_payload(&payload);
        let transfer_flags = if dest_is_group || broadcast_dest {
            // EN 300 392-2 29.3.3.8.2: broadcast SDS shall not request a
            // delivery report. Group SDS is also sent unacknowledged here.
            SDS_TL_TRANSFER_NO_REPORT_REQ
        } else {
            SDS_TL_TRANSFER_WITH_REPORT_REQ
        };
        let wrapped_payload: Vec<u8> = {
            let mut v = vec![SDS_TL_TEXT_PROTOCOL_ID, transfer_flags, mr];
            v.extend_from_slice(&text_user_data);
            v
        };
        if wrapped_payload.len() > 255 {
            tracing::warn!(
                "SDS: rejecting Control SendSds with {} wrapped bytes; Type4 SDS length is limited to 255 byte-aligned octets",
                wrapped_payload.len()
            );
            return false;
        }
        let wrapped_len_bits = (wrapped_payload.len() * 8) as u16;

        self.send_d_sds_data(
            queue,
            route.air_source_ssi,
            dest_ssi,
            route.dest_ssi_type,
            SdsUserData::Type4(wrapped_len_bits, wrapped_payload),
        );

        true
    }

    /// Handle incoming raw SDS data from Control entity (network-originated SDS).
    pub fn rx_raw_sds_from_control(&mut self, queue: &mut MessageQueue, message: ControlCommand) -> bool {
        let ControlCommand::SendRawSds {
            handle,
            source_ssi,
            dest_ssi,
            dest_is_group,
            sdti,
            len_bits,
            payload,
        } = message
        else {
            tracing::error!("SDS: rx_raw_sds_from_control expected SendRawSds command, got unexpected command type");
            return false;
        };

        tracing::info!(
            "SDS: received raw from Control {}: {} -> {}, type={}, sdti={}, {} bits",
            handle,
            source_ssi,
            dest_ssi,
            dest_is_group.then(|| "GSSI").unwrap_or("ISSI"),
            sdti,
            len_bits
        );

        let Some(route) = self.control_sds_air_route("Control SendRawSds", source_ssi, dest_ssi, dest_is_group) else {
            return false;
        };

        let Some(user_defined_data) = Self::raw_control_sds_user_data(sdti, len_bits, payload) else {
            return false;
        };
        let user_defined_data = if route.dest_ssi_type == SsiType::Gssi {
            // EN 300 392-2 clause 29.3.3.8.2: SDS-TL broadcast/group
            // transfer has no associated acknowledgement, so raw control
            // broadcasts must not request delivery reports either.
            Self::sds_tl_without_delivery_report(user_defined_data)
        } else {
            user_defined_data
        };

        self.send_d_sds_data(queue, route.air_source_ssi, dest_ssi, route.dest_ssi_type, user_defined_data);

        true
    }

    fn raw_control_sds_user_data(sdti: u8, len_bits: u16, payload: Vec<u8>) -> Option<SdsUserData> {
        match sdti {
            0 => {
                Self::check_raw_sds_payload("Type1", len_bits, 16, payload.len(), 2)?;
                Some(SdsUserData::Type1(u16::from_be_bytes([payload[0], payload[1]])))
            }
            1 => {
                Self::check_raw_sds_payload("Type2", len_bits, 32, payload.len(), 4)?;
                Some(SdsUserData::Type2(u32::from_be_bytes([
                    payload[0], payload[1], payload[2], payload[3],
                ])))
            }
            2 => {
                Self::check_raw_sds_payload("Type3", len_bits, 64, payload.len(), 8)?;
                Some(SdsUserData::Type3(u64::from_be_bytes([
                    payload[0], payload[1], payload[2], payload[3], payload[4], payload[5], payload[6], payload[7],
                ])))
            }
            3 => {
                // EN 300 392-2 clauses 13.3.3 and 14.8.52: Type4 user data
                // begins with an 8-bit protocol identifier, so generated raw
                // Type4 SDS shorter than one octet has no valid PID.
                if len_bits < 8 {
                    tracing::warn!(
                        "SDS: rejecting Control SendRawSds Type4 with len_bits={} below mandatory protocol identifier length",
                        len_bits
                    );
                    return None;
                }
                if len_bits > SDS_TYPE4_MAX_BITS {
                    tracing::warn!(
                        "SDS: rejecting Control SendRawSds Type4 with len_bits={} above 11-bit length indicator maximum {}",
                        len_bits,
                        SDS_TYPE4_MAX_BITS
                    );
                    return None;
                }
                let Some(expected_bytes) = SdsUserData::type4_declared_byte_count(len_bits) else {
                    tracing::warn!("SDS: rejecting Control SendRawSds Type4 with invalid len_bits={}", len_bits);
                    return None;
                };
                if payload.len() < expected_bytes {
                    tracing::warn!(
                        "SDS: rejecting Control SendRawSds Type4 with len_bits={} requiring {} payload byte(s), got {}",
                        len_bits,
                        expected_bytes,
                        payload.len()
                    );
                    return None;
                }

                let Some(data) = SdsUserData::canonical_type4_bytes(len_bits, &payload) else {
                    tracing::warn!("SDS: rejecting Control SendRawSds Type4 with inconsistent declared length");
                    return None;
                };
                Some(SdsUserData::Type4(len_bits, data))
            }
            other => {
                tracing::warn!("SDS: rejecting Control SendRawSds with invalid SDTI {}; expected 0..3", other);
                None
            }
        }
    }

    fn check_raw_sds_payload(kind: &str, len_bits: u16, expected_bits: u16, payload_len: usize, expected_bytes: usize) -> Option<()> {
        if len_bits != expected_bits {
            tracing::warn!(
                "SDS: rejecting Control SendRawSds {} with len_bits={}, expected {}",
                kind,
                len_bits,
                expected_bits
            );
            return None;
        }
        if payload_len != expected_bytes {
            tracing::warn!(
                "SDS: rejecting Control SendRawSds {} requiring exactly {} payload byte(s), got {}",
                kind,
                expected_bytes,
                payload_len
            );
            return None;
        }
        Some(())
    }

    fn encode_control_text_payload(payload: &[u8]) -> Vec<u8> {
        if let Ok(text) = std::str::from_utf8(payload) {
            if text.chars().all(|ch| ch as u32 <= 0xFF) {
                let mut user_data = Vec::with_capacity(text.len() + 1);
                user_data.push(SDS_TL_TEXT_CODING_LATIN_1);
                user_data.extend(text.chars().map(|ch| ch as u8));
                return user_data;
            }

            let mut user_data = Vec::with_capacity(text.len() * 2 + 1);
            user_data.push(SDS_TL_TEXT_CODING_UCS2);
            user_data.extend(text.encode_utf16().flat_map(|unit| unit.to_be_bytes()));
            return user_data;
        }

        let mut user_data = Vec::with_capacity(payload.len() + 1);
        user_data.push(SDS_TL_TEXT_CODING_LATIN_1);
        user_data.extend_from_slice(payload);
        user_data
    }

    /// Handle incoming U-STATUS from a local MS (via RF uplink)
    pub fn route_status_deliver(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("SDS route_status_deliver");

        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let calling_party = prim.received_tetra_address;
        let Some(source_ssi) = self.registered_rf_calling_party_issi(calling_party) else {
            return;
        };

        let pdu = match UStatus::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-STATUS: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        if !Self::feature_check_u_status(&pdu) {
            // EN 300 392-2 clauses 14.7.2.7 and 14.7.3.2/table 14.33:
            // a registered ISSI-origin U-STATUS with unsupported SNA/TSI or
            // external/DM addressing is rejected explicitly instead of being
            // silently routed by a partial SSI interpretation.
            tracing::warn!("Unsupported features in U-STATUS, returning CMCE FUNCTION NOT SUPPORTED");
            queue.push_back(Self::build_cmce_function_not_supported_direct(
                CmcePduTypeUl::UStatus,
                calling_party,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
            ));
            return;
        }

        // Extract destination SSI (guaranteed present after feature check)
        let Some(dest_ssi_raw) = pdu.called_party_ssi else {
            tracing::warn!("SDS: U-STATUS missing called_party_ssi after feature check, dropping");
            return;
        };
        let dest_ssi = dest_ssi_raw as u32;

        tracing::info!(
            "SDS: U-STATUS from ISSI {} to ISSI {}, status={}",
            source_ssi,
            dest_ssi,
            pdu.pre_coded_status
        );

        // Local compatibility extension: U-STATUS to ISSI 9999 may trigger
        // configured command control. If no configured command matches, keep
        // routing the U-STATUS normally as an SSI/GSSI addressed message.
        if dest_ssi == 9999 && self.handle_sds_command_status(source_ssi, &pdu.pre_coded_status) {
            return;
        }

        // Route: local ISSI delivery, local GSSI delivery, Brew forward, or drop.
        let is_broadcast_group = dest_ssi == MAX_AIR_INTERFACE_SSI;
        let (is_local_issi, is_local_group) = {
            let state = self.config.state_read();
            (
                !is_broadcast_group && state.subscribers.is_registered(dest_ssi),
                is_broadcast_group || state.subscribers.has_group_members(dest_ssi),
            )
        };

        if is_local_issi && is_local_group {
            // EN 300 392-2 clauses 13.2 and 14.5.5 keep predefined individual
            // and group status services distinct. Do not infer ISSI delivery
            // when the same numeric destination is also a local GSSI.
            tracing::warn!(
                "SDS-STATUS: ambiguous U-STATUS destination {} is both local ISSI and local GSSI; dropping",
                dest_ssi
            );
            return;
        }

        if is_local_issi {
            tracing::info!("SDS-STATUS: local delivery: {} -> {}", source_ssi, dest_ssi);
            self.send_d_status(queue, source_ssi, dest_ssi, SsiType::Issi, pdu.pre_coded_status, None);
        } else if is_local_group {
            tracing::info!("SDS-STATUS: group delivery: {} -> GSSI {}", source_ssi, dest_ssi);
            self.send_d_status(queue, source_ssi, dest_ssi, SsiType::Gssi, pdu.pre_coded_status, None);
        } else if net_brew::feature_sds_enabled(&self.config) {
            // Brew forwarding only: when the pre-coded status carries an SDS-TL short report
            // (ETSI 29.4.2.3), convert it to a full SDS-TL REPORT PDU (Type4) so the
            // remote end recognizes it as a delivery confirmation. ETSI 29.3.3.4.4
            // explicitly allows SwMI to "modify a short report to a standard report."
            // Non-SDS-TL pre-coded statuses are forwarded as-is (Type1).
            // Local delivery (D-STATUS) is not affected, it stays as pre-coded status above.
            let user_defined_data = if let PreCodedStatus::SdsTl(report) = &pdu.pre_coded_status {
                // EN 300 392-2 clause 29.3.3.4.4 permits SwMI conversion
                // from SDS-SHORT REPORT to SDS-REPORT. Map table 29.23 short
                // reasons onto table 29.16 delivery-status values. The short
                // form collapses protocol and encoding errors; use the
                // protocol-not-supported full-report value as the conservative
                // negative status when no finer cause is available.
                let delivery_status = match report.short_report_type() {
                    ShortReportType::MessageReceived => 0x00,
                    ShortReportType::MessageConsumed => 0x02,
                    ShortReportType::DestMemFull => 0x52,
                    ShortReportType::ProtOrEncodingNotSupported => 0x50,
                };
                let message_reference = report.message_reference();
                let short_report_type = report.short_report_type();
                if let Some(protocol_id) = self.take_sds_tl_report_protocol_id(source_ssi, dest_ssi, message_reference, short_report_type) {
                    let sds_tl_report = vec![protocol_id, 0x10, delivery_status, message_reference];
                    tracing::info!(
                        "SDS-STATUS: converting SDS-TL short report to Type4 for Brew: PID=0x{:02x} MR={} status=0x{:02x}",
                        protocol_id,
                        message_reference,
                        delivery_status
                    );
                    SdsUserData::Type4(32, sds_tl_report)
                } else {
                    tracing::warn!(
                        "SDS-STATUS: forwarding SDS-TL short report as pre-coded status because no PID context exists for source={} dest={} MR={}",
                        source_ssi,
                        dest_ssi,
                        message_reference
                    );
                    SdsUserData::Type1(pdu.pre_coded_status.into_raw())
                }
            } else {
                SdsUserData::Type1(pdu.pre_coded_status.into_raw())
            };

            tracing::info!("SDS-STATUS: forwarding to Brew: {} -> {}", source_ssi, dest_ssi);
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceSdsData(CmceSdsData {
                    source_issi: source_ssi,
                    dest_issi: dest_ssi,
                    dest_ssi_type: None,
                    user_defined_data,
                    tx_reporter: None,
                }),
            });
        } else {
            tracing::warn!(
                "SDS-STATUS: dest ISSI {} not locally registered and not Brew-routable, dropping",
                dest_ssi
            );
        }
    }

    /// Build and send a D-STATUS PDU to a local MS
    fn send_d_status(
        &self,
        queue: &mut MessageQueue,
        source_issi: u32,
        dest_ssi: u32,
        dest_ssi_type: SsiType,
        pre_coded_status: PreCodedStatus,
        tx_reporter: Option<TxReporter>,
    ) {
        if !Self::valid_air_interface_ssi(source_issi) || !Self::valid_air_interface_ssi(dest_ssi) {
            tracing::warn!(
                "SDS-STATUS: dropping D-STATUS with invalid 24-bit SSI source={} dest={}",
                source_issi,
                dest_ssi
            );
            Self::discard_status_reporter(tx_reporter);
            return;
        }

        let pdu = DStatus {
            calling_party_type_identifier: PartyTypeIdentifier::Ssi,
            calling_party_address_ssi: Some(source_issi as u64),
            calling_party_extension: None,
            pre_coded_status,
            external_subscriber_number: None,
            dm_ms_address: None,
        };

        tracing::debug!("-> D-STATUS {:?}", pdu);

        let mut sdu = BitBuffer::new_autoexpand(64);
        if let Err(e) = pdu.to_bitbuf(&mut sdu) {
            tracing::error!("Failed to serialize D-STATUS: {:?}", e);
            Self::discard_status_reporter(tx_reporter);
            return;
        }
        sdu.seek(0);

        let dest_addr = TetraAddress::new(dest_ssi, dest_ssi_type);
        let layer2service = match dest_ssi_type {
            SsiType::Issi => Layer2Service::Acknowledged,
            // EN 300 392-2 table 14.14 D-STATUS is a one-way pre-coded status PDU.
            // Local GSSI delivery uses unacknowledged L2 service because there is no
            // single group peer to complete an acknowledged basic-link exchange.
            _ => Layer2Service::Unacknowledged,
        };
        let msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: 0,
                endpoint_id: 0,
                link_id: 0,
                layer2service,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                chan_alloc: None,
                main_address: dest_addr,
                tx_reporter,
            }),
        };
        queue.push_back(msg);
    }

    // ── Built-in WX/METAR service ──────────────────────────────────────────
    //
    // Extract the text from an incoming SDS, parse the weather command, fetch the METAR on
    // a background thread (network I/O must not block the stack loop), then re-inject the
    // reply as a ControlCommand::SendSds — the same path the dashboard uses, so it lands
    // back in rx_sds_from_control on the stack thread.

    /// True when the SDS user data is an SDS-TL SHORT REPORT / STATUS PDU — i.e. a
    /// delivery confirmation rather than a text request. Recognised as PID 0x82/0x89 with
    /// message-type byte 0x10. Mirrors the `data[1] == 0x10` check in tetraflow-sds-bot's
    /// `parse_text_payload` / `handle_downlink_sds`, the proven discriminator that keeps
    /// reports out of the responder.
    fn is_sds_tl_report(data: &SdsUserData) -> bool {
        let bytes = data.to_arr();
        bytes.len() >= 4 && matches!(bytes.first(), Some(0x82) | Some(0x89)) && bytes[1] == 0x10
    }

    /// Message-reference byte (data[2]) of an SDS-TL text request — PID 0x82/0x89 that is
    /// not itself a report. Echoed back in the delivery confirmation, mirroring the
    /// `message_reference` the bot pulls in `parse_text_payload`. `None` when there is no
    /// usable SDS-TL header.
    fn sds_tl_message_reference(data: &SdsUserData) -> Option<u8> {
        let bytes = data.to_arr();
        if bytes.len() >= 4 && matches!(bytes.first(), Some(0x82) | Some(0x89)) && bytes[1] != 0x10 {
            Some(bytes[2])
        } else {
            None
        }
    }

    /// Pull the human-readable text out of an SDS user-data field. Handles the SDS-TL
    /// "simple text" wrapper (PID 0x82/0x80/0x8A, msg-type byte, message-ref, encoding,
    /// then text) as well as raw text payloads. Returns an ASCII string (best-effort).
    fn extract_sds_text(data: &SdsUserData) -> String {
        let bytes = data.to_arr();
        if bytes.is_empty() {
            return String::new();
        }
        // SDS-TL text messaging PIDs: 0x82 (text), 0x80/0x8A (text w/ variants). When the
        // first byte looks like one of these and there is a 4-byte header, skip it.
        let payload: &[u8] = match bytes.first() {
            Some(0x82) | Some(0x80) | Some(0x8A) if bytes.len() > 4 => &bytes[4..],
            // Some terminals send a bare text-coding-scheme byte (0x01..=0x03) then text.
            Some(0x01..=0x03) if bytes.len() > 1 => &bytes[1..],
            _ => &bytes[..],
        };
        payload
            .iter()
            .filter(|&&b| b == b'\t' || (0x20..=0x7E).contains(&b))
            .map(|&b| b as char)
            .collect::<String>()
            .trim()
            .to_string()
    }

    /// Handle a weather request SDS addressed to the service ISSI. Spawns a worker that
    /// fetches the METAR and sends the reply back to `requester_issi`.
    fn handle_wx_request(&self, requester_issi: u32, data: &SdsUserData) {
        use crate::net_dashboard::wx_service::{self, WxRequest};

        let text = Self::extract_sds_text(data);
        tracing::info!("WX: request from ISSI {}: {:?}", requester_issi, text);

        let Some(tx) = self.wx_cmd_tx.clone() else {
            tracing::warn!("WX: no control sender wired, cannot reply to {}", requester_issi);
            return;
        };
        let service_issi = self.config.effective_wx_service().service_issi;

        // Only two commands exist: METAR (aviationweather) and WX (wttr.in). Anything else is
        // not a command and gets no reply. Both do blocking network I/O, so each runs on a
        // worker thread and re-injects its reply via the control channel.
        let Some(request) = wx_service::parse_wx_request(&text) else {
            tracing::debug!(
                "WX: ignoring non-command SDS from ISSI {} (only METAR/WX): {:?}",
                requester_issi,
                text
            );
            return;
        };

        std::thread::Builder::new()
            .name("wx-fetch".into())
            .spawn(move || {
                let reply = match request {
                    WxRequest::Metar(icao) => match wx_service::fetch_metar_decoded(&icao) {
                        Ok(decoded) if !decoded.is_empty() => decoded,
                        Ok(_) => format!("{icao}: no data"),
                        Err(e) => {
                            tracing::warn!("WX: METAR fetch {} failed: {}", icao, e);
                            format!("{icao}: unavailable")
                        }
                    },
                    WxRequest::Wx(loc) => match wx_service::fetch_wx(&loc) {
                        Ok(decoded) if !decoded.is_empty() => decoded,
                        Ok(_) => format!("{loc}: no data"),
                        Err(e) => {
                            tracing::warn!("WX: wttr fetch {} failed: {}", loc, e);
                            format!("{loc}: unavailable")
                        }
                    },
                };
                Self::queue_wx_reply(&tx, service_issi, requester_issi, &reply);
            })
            .ok();
    }

    /// Build a SendSds control command carrying `text` and push it onto the control queue.
    /// `payload` here is the bare text; rx_sds_from_control wraps it in the SDS-TL header.
    fn queue_wx_reply(tx: &crossbeam_channel::Sender<ControlCommand>, source_issi: u32, dest_issi: u32, text: &str) {
        // TETRA SDS-TL simple text is length-limited; trim to a safe size.
        let mut payload: Vec<u8> = text.bytes().take(220).collect();
        if payload.is_empty() {
            payload = b"(no data)".to_vec();
        }
        let len_bits = (payload.len() * 8) as u16;
        let cmd = ControlCommand::SendSds {
            handle: 0,
            source_ssi: source_issi,
            dest_ssi: dest_issi,
            dest_is_group: false,
            len_bits,
            payload,
        };
        if tx.send(cmd).is_err() {
            tracing::warn!("WX: failed to enqueue reply to ISSI {}", dest_issi);
        }
    }

    /// Called every tick. When periodic WX is enabled and the interval has elapsed, fetch
    /// the configured station's METAR and send it to the configured destination.
    pub fn tick_periodic_wx(&mut self) {
        let wx = self.config.effective_wx_service();
        if !wx.periodic_enabled || wx.periodic_issi == 0 || wx.periodic_icao.trim().is_empty() {
            return;
        }
        let interval = std::time::Duration::from_secs(wx.effective_interval_secs());
        let due = match self.last_periodic_wx {
            None => true,
            Some(t) => t.elapsed() >= interval,
        };
        if !due {
            return;
        }
        self.last_periodic_wx = Some(std::time::Instant::now());

        let Some(tx) = self.wx_cmd_tx.clone() else {
            return;
        };
        let icao = wx.periodic_icao.clone();
        let dest = wx.periodic_issi;
        let is_group = wx.periodic_is_group;
        let source_issi = wx.service_issi;

        std::thread::Builder::new()
            .name("wx-periodic".into())
            .spawn(move || {
                use crate::net_dashboard::wx_service;
                let reply = match wx_service::fetch_metar_decoded(&icao) {
                    Ok(d) if !d.is_empty() => d,
                    _ => return, // skip this cycle on failure; try again next interval
                };
                let payload: Vec<u8> = reply.bytes().take(220).collect();
                let len_bits = (payload.len() * 8) as u16;
                let cmd = ControlCommand::SendSds {
                    handle: 0,
                    source_ssi: source_issi,
                    dest_ssi: dest,
                    dest_is_group: is_group,
                    len_bits,
                    payload,
                };
                let _ = tx.send(cmd);
            })
            .ok();
    }

    /// Build and send a D-SDS-DATA PDU to a local MS
    fn send_d_sds_data(
        &mut self,
        queue: &mut MessageQueue,
        source_issi: u32,
        dest_ssi: u32,
        dest_ssi_type: SsiType,
        user_defined_data: SdsUserData,
    ) {
        self.send_d_sds_data_with_reporter(queue, source_issi, dest_ssi, dest_ssi_type, user_defined_data, None);
    }

    fn send_d_sds_data_with_reporter(
        &mut self,
        queue: &mut MessageQueue,
        source_issi: u32,
        dest_ssi: u32,
        dest_ssi_type: SsiType,
        user_defined_data: SdsUserData,
        tx_reporter: Option<TxReporter>,
    ) {
        if !Self::valid_air_interface_ssi(source_issi) || !Self::valid_air_interface_ssi(dest_ssi) {
            tracing::warn!(
                "SDS: dropping D-SDS-DATA with invalid 24-bit SSI source={} dest={}",
                source_issi,
                dest_ssi
            );
            Self::discard_sds_reporter(tx_reporter);
            return;
        }

        // EN 300 392-2 clause 29.4.1 makes the Type4 SDS-TL protocol
        // identifier the first information element, while clause 29.4.3.11
        // says SDS-SHORT REPORT carries only the message reference. Remember
        // the PID for individual transfers that request a report so later
        // U-STATUS short reports can be expanded without assuming text PID
        // 0x82 for vendor/user-defined SDS-TL traffic.
        self.remember_sds_tl_report_context(source_issi, dest_ssi, dest_ssi_type, &user_defined_data);

        let pdu = DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Ssi,
            calling_party_address_ssi: Some(source_issi as u64),
            calling_party_extension: None,
            user_defined_data,
            external_subscriber_number: None,
            dm_ms_address: None,
        };

        tracing::debug!("-> D-SDS-DATA {:?}", pdu);

        let mut sdu = BitBuffer::new_autoexpand(128);
        if let Err(e) = pdu.to_bitbuf(&mut sdu) {
            tracing::error!("Failed to serialize D-SDS-DATA: {:?}", e);
            Self::discard_sds_reporter(tx_reporter);
            return;
        }
        sdu.seek(0);

        let dest_addr = TetraAddress::new(dest_ssi, dest_ssi_type);
        let layer2service = match dest_ssi_type {
            SsiType::Issi => Layer2Service::Acknowledged,
            // Group and anything else: unacknowledged. All current callers pass Issi or
            // Gssi; default the rest to Unacknowledged rather than unreachable!()-panic.
            _ => Layer2Service::Unacknowledged,
        };
        let msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: 0,
                endpoint_id: 0,
                link_id: 0,
                layer2service,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                chan_alloc: None,
                main_address: dest_addr,
                tx_reporter,
            }),
        };
        queue.push_back(msg);
    }

    fn feature_check_u_sds_data(pdu: &USdsData) -> bool {
        let mut supported = true;
        if pdu.called_party_ssi.is_none() {
            if pdu.called_party_short_number_address.is_some() {
                unimplemented_log!("SDS: short number addressing not supported");
            } else {
                tracing::warn!("SDS: no destination address in U-SDS-DATA");
            }
            supported = false;
        }
        if pdu.called_party_extension.is_some() {
            unimplemented_log!("SDS: TSI extension addressing not supported");
            supported = false;
        }
        if pdu.external_subscriber_number.is_some() {
            unimplemented_log!("SDS: external_subscriber_number not supported");
            supported = false;
        }
        if pdu.dm_ms_address.is_some() {
            unimplemented_log!("SDS: dm_ms_address not supported");
            supported = false;
        }
        supported
    }

    fn feature_check_u_status(pdu: &UStatus) -> bool {
        let mut supported = true;
        if pdu.called_party_ssi.is_none() {
            if pdu.called_party_short_number_address.is_some() {
                unimplemented_log!("SDS-STATUS: short number addressing not supported");
            } else {
                tracing::warn!("SDS-STATUS: no destination address in U-STATUS");
            }
            supported = false;
        }
        if pdu.called_party_extension.is_some() {
            unimplemented_log!("SDS-STATUS: TSI extension addressing not supported");
            supported = false;
        }
        if pdu.external_subscriber_number.is_some() {
            unimplemented_log!("SDS-STATUS: external_subscriber_number not supported");
            supported = false;
        }
        if pdu.dm_ms_address.is_some() {
            unimplemented_log!("SDS-STATUS: dm_ms_address not supported");
            supported = false;
        }
        supported
    }

    /// Execute a configured local action triggered by SDS U-STATUS to ISSI 9999.
    /// Returns true only when this compatibility extension consumes the U-STATUS.
    fn handle_sds_command_status(&mut self, source_ssi: u32, status: &PreCodedStatus) -> bool {
        let PreCodedStatus::NetworkUserSpecific(status_code) = *status else {
            tracing::debug!(
                "SDS-CMD: U-STATUS to 9999 from {} carries non network/user-specific status {}, not a local command",
                source_ssi,
                status
            );
            return false;
        };

        let cfg = self.config.config();
        let Some(ref ctrl) = cfg.cell.sds_command_control else {
            tracing::debug!(
                "SDS-CMD: U-STATUS to 9999 from {} (status={}) but sds_command_control not configured, ignoring",
                source_ssi,
                status_code
            );
            return false;
        };

        if !ctrl.authorized_issis.contains(&source_ssi) {
            tracing::warn!(
                "SDS-CMD: U-STATUS to 9999 from ISSI {} (status={}) — ISSI not in authorized_issis, ignoring",
                source_ssi,
                status_code
            );
            return false;
        }

        let Some(entry) = ctrl.commands.iter().find(|e| e.status_code == status_code) else {
            tracing::debug!(
                "SDS-CMD: U-STATUS to 9999 from ISSI {} status={} — no matching command, ignoring",
                source_ssi,
                status_code
            );
            return false;
        };

        tracing::info!(
            "SDS-CMD: ISSI {} triggered action='{}' via status={}",
            source_ssi,
            entry.action,
            status_code
        );

        match entry.action.as_str() {
            "restart" => {
                crate::service_control::schedule_service_action(
                    crate::service_control::ServiceAction::Restart,
                    std::time::Duration::from_millis(500),
                );
                true
            }
            "shutdown" => {
                crate::service_control::schedule_service_action(
                    crate::service_control::ServiceAction::Stop,
                    std::time::Duration::from_millis(500),
                );
                true
            }
            "kick_all" => {
                self.pending_actions.push(SdsPendingAction::KickAll);
                true
            }
            other => {
                tracing::warn!("SDS-CMD: unknown action '{}' for status={}, ignoring", other, status_code);
                false
            }
        }
    }
}
