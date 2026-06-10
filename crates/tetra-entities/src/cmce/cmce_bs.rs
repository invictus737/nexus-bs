use crate::net_control::{ControlCommand, ControlEndpoint, ControlResponse};
use crate::net_telemetry::TelemetrySink;
use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::{LIVE_SDS_QUEUE_MAX_LEN, SharedConfig, is_supported_periodic_sds_protocol_id};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{Sap, TdmaTime, unimplemented_log};
use tetra_saps::{SapMsg, SapMsgInner};

use tetra_pdus::cmce::enums::cmce_pdu_type_ul::CmcePduTypeUl;

use super::subentities::cc_bs::CcBsSubentity;
use super::subentities::sds_bs::{SdsBsSubentity, SdsPendingAction};
use super::subentities::ss_bs::SsBsSubentity;

pub struct CmceBs {
    config: SharedConfig,
    telemetry: Option<TelemetrySink>,
    control: Option<ControlEndpoint>,
    dashboard_control: Option<ControlEndpoint>,
    cc: CcBsSubentity,
    sds: SdsBsSubentity,
    ss: SsBsSubentity,
}

impl CmceBs {
    pub fn new(config: SharedConfig, telemetry: Option<TelemetrySink>, control: Option<ControlEndpoint>) -> Self {
        let mut cc = CcBsSubentity::new(config.clone());
        if let Some(ref sink) = telemetry {
            cc.set_telemetry(sink.clone());
        }
        let mut sds = SdsBsSubentity::new(config.clone());
        if let Some(ref sink) = telemetry {
            sds.set_telemetry(sink.clone());
        }
        Self {
            config: config.clone(),
            telemetry,
            control,
            dashboard_control: None,
            sds,
            cc,
            ss: SsBsSubentity::new(),
        }
    }

    pub fn set_dashboard_control(&mut self, endpoint: ControlEndpoint) {
        self.dashboard_control = Some(endpoint);
    }

    /// Wire the control-command sender used by the built-in WX/METAR service to deliver
    /// replies (it re-injects SendSds commands from its background fetch thread).
    pub fn set_wx_cmd_sender(&mut self, tx: crossbeam_channel::Sender<ControlCommand>) {
        self.sds.set_wx_cmd_sender(tx);
    }

    #[doc(hidden)]
    pub fn debug_force_next_call_identifier(&mut self, next_call_identifier: u16) {
        self.cc.debug_force_next_call_identifier(next_call_identifier);
    }

    #[doc(hidden)]
    pub fn debug_active_call_ids(&self) -> Vec<u16> {
        self.cc.debug_active_call_ids()
    }

    #[doc(hidden)]
    pub fn debug_subscriber_groups_for(&self, issi: u32) -> Vec<u32> {
        self.cc.subscriber_groups_for(issi)
    }

    fn do_control_command(
        sds: &mut SdsBsSubentity,
        cc: &mut CcBsSubentity,
        queue: &mut MessageQueue,
        cmd: ControlCommand,
        responder: Option<&ControlEndpoint>,
    ) {
        match cmd {
            ControlCommand::SendSds { handle, .. } => {
                let success = sds.rx_sds_from_control(queue, cmd);
                if let Some(cep) = responder {
                    cep.respond(ControlResponse::SendSdsResponse { handle, success });
                }
            }
            ControlCommand::SendRawSds { handle, .. } => {
                let success = sds.rx_raw_sds_from_control(queue, cmd);
                if let Some(cep) = responder {
                    cep.respond(ControlResponse::SendRawSdsResponse { handle, success });
                }
            }
            ControlCommand::SendStatus { handle, .. } => {
                let success = sds.rx_status_command_from_control(queue, cmd);
                if let Some(cep) = responder {
                    cep.respond(ControlResponse::SendStatusResponse { handle, success });
                }
            }
            ControlCommand::KickMs { issi } => {
                let groups: Vec<u32> = cc.subscriber_groups_for(issi);
                if !groups.is_empty() {
                    use tetra_core::Sap;
                    use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
                    queue.push_back(tetra_saps::SapMsg {
                        sap: Sap::Control,
                        src: TetraEntity::Cmce,
                        dest: TetraEntity::Mm,
                        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
                            issi,
                            groups: groups.clone(),
                            action: BrewSubscriberAction::Deaffiliate,
                        }),
                    });
                }
                tracing::info!("CMCE: KickMs issi={} requested", issi);
                let success = cc.kick_ms(queue, issi);
                if let Some(cep) = responder {
                    cep.respond(ControlResponse::KickMsResponse { issi, success });
                }
            }
            ControlCommand::RestartService => {
                tracing::info!("CMCE: RestartService requested");
                crate::service_control::schedule_service_action(
                    crate::service_control::ServiceAction::Restart,
                    std::time::Duration::from_millis(500),
                );
            }
            ControlCommand::ShutdownService => {
                tracing::info!("CMCE: ShutdownService requested");
                crate::service_control::schedule_service_action(
                    crate::service_control::ServiceAction::Stop,
                    std::time::Duration::from_millis(500),
                );
            }
            ControlCommand::StopGoService { start_delay_secs } => {
                tracing::info!(
                    "CMCE: StopGoService requested; core exits now, systemd restart delay should be {}s",
                    start_delay_secs
                );
                crate::service_control::schedule_service_action(crate::service_control::ServiceAction::Restart, std::time::Duration::ZERO);
            }
            ControlCommand::AddLiveSds {
                text,
                protocol_id,
                source_issi,
                repeat_count,
            } => {
                if !is_supported_periodic_sds_protocol_id(protocol_id) {
                    tracing::warn!(
                        "CMCE: AddLiveSds rejected unsupported periodic/live SDS protocol_id={} text={:?}",
                        protocol_id,
                        text
                    );
                    return;
                }
                if source_issi > 0x00FF_FFFF {
                    tracing::warn!(
                        "CMCE: AddLiveSds rejected invalid 24-bit source_issi={} text={:?}",
                        source_issi,
                        text
                    );
                    return;
                }
                let mut state = sds.shared_config().state_write();
                if state.live_sds_queue.len() >= LIVE_SDS_QUEUE_MAX_LEN {
                    tracing::warn!(
                        "CMCE: AddLiveSds rejected full live SDS queue len={} max={} text={:?}",
                        state.live_sds_queue.len(),
                        LIVE_SDS_QUEUE_MAX_LEN,
                        text
                    );
                    return;
                }
                let id = state.next_live_sds_id;
                state.next_live_sds_id = state.next_live_sds_id.wrapping_add(1).max(1);
                state.live_sds_queue.push_back(tetra_config::bluestation::LiveSdsMessage {
                    id,
                    text: text.clone(),
                    protocol_id,
                    source_issi,
                    repeat_count,
                    sent_count: 0,
                });
                tracing::info!("CMCE: AddLiveSds id={} repeat={} text={:?}", id, repeat_count, text);
            }
            ControlCommand::DeleteLiveSds { id } => {
                let mut state = sds.shared_config().state_write();
                let before = state.live_sds_queue.len();
                state.live_sds_queue.retain(|m| m.id != id);
                let removed = before - state.live_sds_queue.len();
                tracing::info!("CMCE: DeleteLiveSds id={} removed={}", id, removed);
            }
            ControlCommand::ClearLiveSds => {
                let mut state = sds.shared_config().state_write();
                let n = state.live_sds_queue.len();
                state.live_sds_queue.clear();
                tracing::info!("CMCE: ClearLiveSds removed={}", n);
            }
            _ => {
                tracing::warn!("CMCE: ignoring unsupported control command {:?}", cmd);
            }
        }
    }

    pub fn rx_lcmc_mle_unitdata_ind(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_lcmc_mle_unitdata_ind");
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let Some(bits) = prim.sdu.peek_bits(5) else {
            tracing::warn!("insufficient bits: {}", prim.sdu.dump_bin());
            return;
        };
        let Ok(pdu_type) = CmcePduTypeUl::try_from(bits) else {
            tracing::warn!("invalid pdu type: {} in {}", bits, prim.sdu.dump_bin());
            return;
        };
        match pdu_type {
            CmcePduTypeUl::UAlert
            | CmcePduTypeUl::UConnect
            | CmcePduTypeUl::UDisconnect
            | CmcePduTypeUl::UInfo
            | CmcePduTypeUl::URelease
            | CmcePduTypeUl::USetup
            | CmcePduTypeUl::UTxCeased
            | CmcePduTypeUl::UTxDemand
            | CmcePduTypeUl::UCallRestore => {
                self.cc.route_xx_deliver(queue, message);
            }
            CmcePduTypeUl::UStatus => {
                self.sds.route_status_deliver(queue, message);
            }
            CmcePduTypeUl::USdsData => {
                self.sds.route_rf_deliver(queue, message);
            }
            CmcePduTypeUl::UFacility => {
                // ETSI EN 300 392-2 §14.7.2.5:
                // BS does not support supplementary services (SS). Respond with
                // D-CMCE-FUNCTION-NOT-SUPPORTED, function_not_supported_pointer=0
                // (the PDU type itself is not supported, not a specific field).
                tracing::debug!(
                    "CMCE: received UFacility from ISSI {} — responding D-CMCE-FUNCTION-NOT-SUPPORTED",
                    prim.received_tetra_address.ssi
                );
                queue.push_back(CcBsSubentity::build_cmce_function_not_supported_direct(
                    CmcePduTypeUl::UFacility,
                    prim.received_tetra_address,
                    prim.handle,
                    prim.link_id,
                    prim.endpoint_id,
                ));
            }
            CmcePduTypeUl::CmceFunctionNotSupported => {
                unimplemented_log!("{:?}", pdu_type);
            }
        };
    }
}

impl TetraEntityTrait for CmceBs {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Cmce
    }

    fn set_config(&mut self, config: SharedConfig) {
        self.config = config;
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        self.sds.tick_start(queue, ts);
        self.sds.tick_periodic_wx();
        let call_events = self.cc.tick_start_with_events(queue, ts);
        if let Some(sink) = &self.telemetry {
            for event in call_events {
                sink.send(event);
            }
        }
        if let Some(cep) = &self.control {
            while let Some(cmd) = cep.try_recv() {
                CmceBs::do_control_command(&mut self.sds, &mut self.cc, queue, cmd, Some(cep));
            }
        }
        if let Some(cep) = &self.dashboard_control {
            while let Some(cmd) = cep.try_recv() {
                CmceBs::do_control_command(&mut self.sds, &mut self.cc, queue, cmd, None);
            }
        }
        // Drain SDS-triggered actions that require access to CcBsSubentity
        let pending = std::mem::take(&mut self.sds.pending_actions);
        for action in pending {
            match action {
                SdsPendingAction::KickAll => {
                    let issis: Vec<u32> = self.cc.subscriber_issis();
                    tracing::info!("SDS-CMD: kick_all — deregistering {} subscribers", issis.len());
                    for issi in issis {
                        self.cc.kick_ms(queue, issi);
                    }
                }
            }
        }
        crate::health::registry().set_cmce_stats(self.cc.health_stats());
        crate::health::registry().set_sds_stats(self.sds.health_stats());
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);
        match message.sap {
            Sap::LcmcSap => match message.msg {
                SapMsgInner::LcmcMleUnitdataInd(_) => {
                    self.rx_lcmc_mle_unitdata_ind(queue, message);
                }
                SapMsgInner::LcmcMleReportInd(prim) => {
                    tracing::trace!(
                        "CMCE: MLE-REPORT indication handle={} transfer_result={}",
                        prim.handle,
                        prim.transfer_result
                    );
                }
                _ => {
                    tracing::warn!("CMCE: unexpected message on LcmcSap: {:?}, ignoring", message.msg);
                }
            },
            Sap::Control => match message.msg {
                SapMsgInner::CmceCallControl(_) => {
                    self.cc.rx_call_control(queue, message);
                }
                SapMsgInner::MmSubscriberUpdate(update) => {
                    self.cc.handle_subscriber_update(queue, update, message.src);
                }
                SapMsgInner::CmceSdsData(_) => {
                    self.sds.rx_sds_from_brew(queue, message);
                }
                SapMsgInner::CmceSdsStatus(_) => {
                    self.sds.rx_status_from_control(queue, message);
                }
                _ => {
                    tracing::warn!("CMCE: unexpected control message: {:?}, ignoring", message.msg);
                }
            },
            Sap::TmdSap => {
                // UL voice frame — feed to echo session if active, and forward to Brew for FDX calls
                if let SapMsgInner::TmdCircuitDataInd(ref prim) = message.msg {
                    let consumed_by_parrot = self
                        .cc
                        .handle_parrot_ul_frame(queue, prim.ts, prim.data.clone(), prim.raw_tch_s_block);
                    self.cc.handle_echo_ul_frame(queue, prim.ts, prim.data.clone());
                    // Emit TS activity for dashboard visualizer
                    if let Some(ref sink) = self.telemetry {
                        let _ = sink.send(crate::net_telemetry::TelemetryEvent::TsVoiceActivity { ts: prim.ts });
                    }
                    if consumed_by_parrot {
                        return;
                    }
                    // Forward UL audio to Brew so TetraPack receives the terminal's voice
                    queue.push_back(message);
                }
            }
            _ => {
                tracing::warn!("CMCE: unexpected SAP {:?}, ignoring", message.sap);
            }
        }
    }
}
