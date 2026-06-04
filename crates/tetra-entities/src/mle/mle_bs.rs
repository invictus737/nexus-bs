use std::collections::HashMap;

use crate::mle::components::broadcast::MleBroadcast;
use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Layer2Service, MleHandle, Sap, TdmaTime, TetraAddress, Todo, unimplemented_log};
use tetra_saps::lcmc::{LcmcMleReportInd, LcmcMleUnitdataInd};
use tetra_saps::lmm::{LmmMleReportInd, LmmMleUnitdataInd};
use tetra_saps::ltpd::{LtpdMleReportInd, LtpdMleUnitdataInd};
use tetra_saps::tla::{
    TLA_REPORT_FAILED_TRANSFER, TLA_REPORT_FIRST_COMPLETE_TRANSMISSION, TLA_REPORT_NO_SPECIFIC_REPORT, TLA_REPORT_SUCCESSFUL_TRANSFER,
    TlDataConfBl, TlDataRespBl, TlaTlDataReqBl, TlaTlReportInd, TlaTlUnitdataReqBl,
};
use tetra_saps::{SapMsg, SapMsgInner};

use tetra_pdus::mle::enums::mle_pdu_type_dl::MlePduTypeDl;
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;

pub struct MleBs {
    config: SharedConfig,
    broadcast: MleBroadcast,
    next_tla_handle: Todo,
    pending_data_transfers: HashMap<Todo, PendingMleTransfer>,
}

#[derive(Debug, Clone, Copy)]
enum MleSapUser {
    Mm,
    Cmce,
    Sndcp,
    Broadcast,
}

#[derive(Debug, Clone, Copy)]
struct PendingMleTransfer {
    user: MleSapUser,
    upper_handle: MleHandle,
}

/// Multiframes at which D-NWRK-BROADCAST is sent within each hyperframe.
/// Two broadcasts per hyperframe (~30.6s interval) for faster time/date display on terminals.
/// The legacy default was 1 per hyperframe (~61.2s), which is slow on cold attach.
/// We don't use the first multiframe to avoid congestion with other hyperframe-triggered events.
const MLE_BROADCAST_MULTIFRAMES: [u8; 2] = [20, 50];
/// Frame at which D-NWRK-BROADCAST is sent within the broadcast multiframe.
const MLE_BROADCAST_FRAME: u8 = 1;

impl MleBs {
    pub fn new(config: SharedConfig) -> Self {
        let broadcast = MleBroadcast::new(config.clone());
        Self {
            config,
            broadcast,
            next_tla_handle: 1,
            pending_data_transfers: HashMap::new(),
        }
    }

    fn allocate_tla_handle(&mut self) -> Todo {
        loop {
            let handle = self.next_tla_handle;
            self.next_tla_handle += 1;
            if self.next_tla_handle <= 0 {
                self.next_tla_handle = 1;
            }

            if handle > 0 && !self.pending_data_transfers.contains_key(&handle) {
                return handle;
            }
        }
    }

    fn track_tla_data_request(&mut self, user: MleSapUser, upper_handle: MleHandle) -> Todo {
        let handle = self.allocate_tla_handle();
        self.pending_data_transfers
            .insert(handle, PendingMleTransfer { user, upper_handle });
        handle
    }

    fn todo_to_mle_handle(handle: Todo) -> MleHandle {
        if handle >= 0 { handle as MleHandle } else { 0 }
    }

    fn mle_handle_to_todo(handle: MleHandle) -> Todo {
        if handle <= i32::MAX as MleHandle {
            handle as Todo
        } else {
            i32::MAX
        }
    }

    fn subscriber_class(&self) -> Todo {
        self.config.config().cell.subscriber_class as Todo
    }

    fn push_mle_report(
        queue: &mut MessageQueue,
        pending: PendingMleTransfer,
        transfer_result: Todo,
        channel_change_response_required: bool,
        channel_change_handle: Todo,
    ) {
        match pending.user {
            MleSapUser::Mm => queue.push_back(SapMsg {
                sap: Sap::LmmSap,
                src: TetraEntity::Mle,
                dest: TetraEntity::Mm,
                msg: SapMsgInner::LmmMleReportInd(LmmMleReportInd {
                    handle: pending.upper_handle,
                    transfer_result,
                }),
            }),
            MleSapUser::Cmce => queue.push_back(SapMsg {
                sap: Sap::LcmcSap,
                src: TetraEntity::Mle,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::LcmcMleReportInd(LcmcMleReportInd {
                    handle: pending.upper_handle as Todo,
                    transfer_result,
                    channel_change_response_required,
                    channel_change_handle,
                }),
            }),
            MleSapUser::Sndcp => queue.push_back(SapMsg {
                sap: Sap::TlpdSap,
                src: TetraEntity::Mle,
                dest: TetraEntity::Sndcp,
                msg: SapMsgInner::LtpdMleReportInd(LtpdMleReportInd {
                    handle: pending.upper_handle as Todo,
                    transfer_result,
                }),
            }),
            MleSapUser::Broadcast => {}
        }
    }

    fn rx_tla_mle_pdu(&mut self, _queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tla_mle_pdu");

        // Extract tm_sdu from whatever primitive we have
        let tm_sdu = {
            match message.msg {
                SapMsgInner::TlaTlDataIndBl(prim) => prim.tl_sdu,
                _ => {
                    tracing::error!("BUG: unexpected message or state -- routing error");
                    return;
                }
            }
        };
        let Some(sdu) = tm_sdu else {
            tracing::debug!("rx_tla_mle_pdu: no tm_sdu");
            return;
        };

        // Determine which type of TL-SDU we have and call handler function
        let Some(bits) = sdu.peek_bits(3) else {
            tracing::warn!("insufficient bits: {}", sdu.dump_bin());
            return;
        };
        let Ok(pdu_type) = MlePduTypeDl::try_from(bits) else {
            tracing::warn!("invalid pdu type: {} in {}", bits, sdu.dump_bin());
            return;
        };

        match pdu_type {
            MlePduTypeDl::DNewCell => {
                unimplemented_log!("DNewCell")
            }
            MlePduTypeDl::DPrepareFail => {
                unimplemented_log!("DPrepareFail")
            }
            MlePduTypeDl::DNwrkBroadcast => {
                unimplemented_log!("DNwrkBroadcast")
            }
            MlePduTypeDl::DNwrkBroadcastExt => {
                unimplemented_log!("DNwrkBroadcastExt")
            }
            MlePduTypeDl::DRestoreAck => {
                unimplemented_log!("DRestoreAck")
            }
            MlePduTypeDl::DRestoreFail => {
                unimplemented_log!("DRestoreFail")
            }
            MlePduTypeDl::DChannelResponse => {
                unimplemented_log!("DChannelResponse")
            }
            MlePduTypeDl::ExtPdu => {
                unimplemented_log!("ExtPdu")
            }
        }
    }

    fn route_prefixed_tl_sdu(
        &mut self,
        queue: &mut MessageQueue,
        mut sdu: BitBuffer,
        main_address: TetraAddress,
        endpoint_id: u32,
        link_id: u32,
        handle: MleHandle,
    ) {
        if sdu.get_pos() != 0 {
            tracing::warn!("MLE: received TL-SDU not at start position (pos={}), seeking to 0", sdu.get_pos());
            sdu.seek(0);
        }
        let Some(bits) = sdu.read_bits(3) else {
            tracing::warn!("insufficient bits: {}", sdu.dump_bin());
            return;
        };
        let Ok(pdu_type) = MleProtocolDiscriminator::try_from(bits) else {
            tracing::warn!("invalid pdu type: {} in {}", bits, sdu.dump_bin());
            return;
        };

        match pdu_type {
            MleProtocolDiscriminator::Mm => {
                let m = LmmMleUnitdataInd {
                    sdu,
                    handle,
                    received_address: main_address,
                };
                queue.push_back(SapMsg {
                    sap: Sap::LmmSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Mm,
                    msg: SapMsgInner::LmmMleUnitdataInd(m),
                });
            }
            MleProtocolDiscriminator::Cmce => {
                let m = LcmcMleUnitdataInd {
                    sdu,
                    handle,
                    received_tetra_address: main_address,
                    endpoint_id,
                    link_id,
                    // Normal TL-DATA/TL-UNITDATA indication has no MLE
                    // channel-change request attached.
                    chan_change_resp_req: false,
                    chan_change_handle: None,
                };
                queue.push_back(SapMsg {
                    sap: Sap::LcmcSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Cmce,
                    msg: SapMsgInner::LcmcMleUnitdataInd(m),
                });
            }
            MleProtocolDiscriminator::Sndcp => {
                let m = LtpdMleUnitdataInd {
                    sdu,
                    endpoint_id,
                    link_id,
                    received_tetra_address: main_address,
                    // Normal TL-DATA/TL-UNITDATA indication has no MLE
                    // channel-change request attached.
                    chan_change_resp_req: false,
                    chan_change_handle: None,
                };
                queue.push_back(SapMsg {
                    sap: Sap::TlpdSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Sndcp,
                    msg: SapMsgInner::LtpdMleUnitdataInd(m),
                });
            }
            MleProtocolDiscriminator::Mle => {
                unimplemented_log!("MleProtocolDiscriminator::Mle");
            }
            MleProtocolDiscriminator::TetraManagementEntity => {
                unimplemented_log!("MleProtocolDiscriminator::TetraManagementEntity");
            }
        }
    }

    fn rx_tla_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tla_prim");
        match message.msg {
            SapMsgInner::TlaTlDataIndBl(_) => {
                self.rx_tla_data_ind_bl(queue, message);
            }
            SapMsgInner::TlaTlDataConfBl(prim) => {
                self.rx_tla_data_conf_bl(queue, prim);
            }
            SapMsgInner::TlaTlReportInd(prim) => {
                self.rx_tla_report_ind(queue, prim);
            }
            SapMsgInner::TlaTlUnitdataIndBl(_) => {
                self.rx_tla_unitdata_ind_bl(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }

    fn rx_tla_report_ind(&mut self, queue: &mut MessageQueue, prim: TlaTlReportInd) {
        let Some(req_handle) = prim.req_handle else {
            tracing::debug!("MLE: TL-REPORT without data req_handle report={}", prim.report);
            return;
        };

        match prim.report {
            TLA_REPORT_NO_SPECIFIC_REPORT | TLA_REPORT_FIRST_COMPLETE_TRANSMISSION => {
                tracing::trace!("MLE: TL-DATA progress report req_handle={} report={}", req_handle, prim.report);
            }
            TLA_REPORT_FAILED_TRANSFER | TLA_REPORT_SUCCESSFUL_TRANSFER => {
                let Some(pending) = self.pending_data_transfers.remove(&req_handle) else {
                    tracing::warn!("MLE: terminal TL-REPORT for unknown req_handle={}", req_handle);
                    return;
                };
                Self::push_mle_report(
                    queue,
                    pending,
                    prim.report,
                    prim.chan_change_resp_req.unwrap_or(false),
                    prim.chan_change_handle.unwrap_or_default(),
                );
            }
            _ => {
                tracing::warn!("MLE: unhandled TL-REPORT req_handle={} report={}", req_handle, prim.report);
            }
        }
    }

    fn rx_tla_data_conf_bl(&mut self, queue: &mut MessageQueue, mut prim: TlDataConfBl) {
        let pending = self.pending_data_transfers.remove(&prim.req_handle);
        if let Some(pending) = pending {
            Self::push_mle_report(
                queue,
                pending,
                prim.report,
                prim.chan_change_resp_req,
                prim.chan_change_handle.unwrap_or_default(),
            );
        } else {
            tracing::warn!("MLE: TL-DATA.conf for unknown req_handle={}", prim.req_handle);
        }

        if let Some(sdu) = prim.tl_sdu.take() {
            // EN 300 392-2 clause 18.3.5.3.1 requires a confirm carrying an
            // SDU to be analysed after the successful MLE-REPORT indication.
            let handle = pending
                .map(|pending| pending.upper_handle)
                .unwrap_or_else(|| Self::todo_to_mle_handle(prim.req_handle));
            self.route_prefixed_tl_sdu(queue, sdu, prim.main_address, prim.endpoint_id, prim.link_id, handle);
        }
    }

    fn rx_tla_data_ind_bl(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        // Take ownership of bitbuf and read protocol discriminator
        let SapMsgInner::TlaTlDataIndBl(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let Some(sdu) = prim.tl_sdu.take() else {
            tracing::warn!("MLE: rx_tla_data_ind_bl received message with no tl_sdu, ignoring");
            return;
        };
        let handle = Self::todo_to_mle_handle(prim.req_handle);
        self.route_prefixed_tl_sdu(queue, sdu, prim.main_address, prim.endpoint_id, prim.link_id, handle);
    }

    fn rx_tla_unitdata_ind_bl(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        // EN 300 392-2 clause 20.3.5.1.9: TL-UNITDATA indication delivers
        // received unacknowledged TL-SDUs to the layer 2 service user.
        let SapMsgInner::TlaTlUnitdataIndBl(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let Some(sdu) = prim.tl_sdu.take() else {
            tracing::warn!("MLE: rx_tla_unitdata_ind_bl received message with no tl_sdu, ignoring");
            return;
        };
        self.route_prefixed_tl_sdu(queue, sdu, prim.main_address, prim.endpoint_id, prim.link_id, 0);
    }

    fn rx_tlmc_prim(&mut self, _queue: &mut MessageQueue, _message: SapMsg) {
        tracing::trace!("rx_tlmc_prim");
        // TLMC SAP not implemented yet. Log instead of panicking so an unexpected
        // primitive doesn't kill the whole MLE worker.
        unimplemented_log!("rx_tlmc_prim called but TLMC SAP is not implemented");
    }

    fn rx_lmm_mle_unitdata_req(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_lmm_mle_unitdata_req");
        let SapMsgInner::LmmMleUnitdataReq(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        if prim.layer2service == Layer2Service::Todo {
            // EN 300 392-2 clause 18.3.5.3.1 defines only acknowledged
            // request, acknowledged response, and unacknowledged service.
            // A legacy placeholder must not be guessed as acknowledged.
            tracing::error!("MLE-BS: rejecting MM MLE-UNITDATA with unspecified Layer2Service::Todo");
            return;
        }

        let mle_prot_discriminator = MleProtocolDiscriminator::Mm;
        let sdu_len = prim.sdu.get_len();
        let mut pdu = BitBuffer::new(3 + sdu_len);
        pdu.write_bits(mle_prot_discriminator.into_raw(), 3);
        pdu.copy_bits(&mut prim.sdu, sdu_len);
        pdu.seek(0);

        // let (addr, link, endpoint) = self.router.use_handle(prim.handle, message.dltime);
        // assert_eq!(addr.ssi, prim.address.ssi);
        let req_handle = Self::mle_handle_to_todo(prim.handle);
        let subscriber_class = self.subscriber_class();
        let msg = match prim.layer2service {
            Layer2Service::Unacknowledged => SapMsgInner::TlaTlUnitdataReqBl(TlaTlUnitdataReqBl {
                main_address: prim.address,
                link_id: 0,
                endpoint_id: 0,
                tl_sdu: pdu,
                pdu_prio: 0,
                stealing_permission: prim.stealing_permission,
                subscriber_class,
                fcs_flag: false,
                air_interface_encryption: None,
                stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                packet_data_flag: false,
                n_tlsdu_repeats: None,
                data_class_info: None,
                // EN 300 392-2 clause 18.3.5.3.1: an MLE-UNITDATA request
                // whose layer 2 service is unacknowledged is transferred to
                // LLC as TL-UNITDATA request after adding the MLE protocol
                // discriminator for the originating SAP.
                req_handle,
                chan_alloc: None,
                tx_reporter: prim.tx_reporter.take(),
            }),
            Layer2Service::AcknowledgedResponse => {
                SapMsgInner::TlaTlDataRespBl(TlDataRespBl {
                    main_address: prim.address,
                    link_id: 0,
                    endpoint_id: 0,
                    tl_sdu: pdu,
                    scrambling_code: 0,
                    pdu_prio: 0,
                    stealing_permission: prim.stealing_permission,
                    subscriber_class,
                    fcs_flag: false,
                    air_interface_encryption: 0,
                    // EN 300 392-2 clause 18.3.5.3.1: stealing permission
                    // and stealing repeats are set by MM/CMCE and simply
                    // passed through MLE to layer 2.
                    stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                    data_class_info: None,
                    req_handle,
                })
            }
            Layer2Service::Acknowledged => {
                let tla_handle = self.track_tla_data_request(MleSapUser::Mm, prim.handle);
                SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
                    main_address: prim.address,
                    link_id: 0,
                    endpoint_id: 0,
                    tl_sdu: pdu,
                    pdu_prio: 0,
                    stealing_permission: prim.stealing_permission,
                    subscriber_class,
                    fcs_flag: false,
                    air_interface_encryption: None,
                    // EN 300 392-2 clause 18.3.5.3.1: layer-3 stealing
                    // parameters are not MLE policy; they are passed to LLC.
                    stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                    data_class_info: None,
                    req_handle: tla_handle,
                    graceful_degradation: None,
                    chan_alloc: None,
                    tx_reporter: prim.tx_reporter.take(),
                })
            }
            Layer2Service::Todo => unreachable!("Layer2Service::Todo rejected before service selection"),
        };
        let sapmsg = SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Mle,
            dest: TetraEntity::Llc,
            msg,
        };
        queue.push_back(sapmsg);
    }

    fn rx_lmm_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_lmm_prim");
        match &message.msg {
            SapMsgInner::LmmMleUnitdataReq(_prim) => {
                self.rx_lmm_mle_unitdata_req(queue, message);
            }
            _ => {
                tracing::warn!("unhandled match variant, ignoring");
            }
        }
    }

    fn ltpd_todo_to_optional_u8(value: Todo) -> Option<u8> {
        u8::try_from(value).ok()
    }

    fn ltpd_todo_to_optional_todo(value: Todo) -> Option<Todo> {
        (value >= 0).then_some(value)
    }

    fn rx_ltpd_mle_unitdata_req(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_ltpd_mle_unitdata_req");
        let SapMsgInner::LtpdMleUnitdataReq(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        if prim.layer2service == Layer2Service::Todo {
            // EN 300 392-2 clause 18.3.5.3.1 only permits acknowledged
            // request/response and unacknowledged service selection. SNDCP
            // must not rely on a legacy placeholder here.
            tracing::error!("MLE-BS: rejecting SNDCP MLE-UNITDATA with unspecified Layer2Service::Todo");
            return;
        }

        let sdu_len = prim.sdu.get_len();
        let mut pdu = BitBuffer::new(3 + sdu_len);
        pdu.write_bits(MleProtocolDiscriminator::Sndcp.into_raw(), 3);
        pdu.copy_bits(&mut prim.sdu, sdu_len);
        pdu.seek(0);

        let subscriber_class = self.subscriber_class();
        let msg = match prim.layer2service {
            Layer2Service::Unacknowledged => {
                let tla_handle = self.track_tla_data_request(MleSapUser::Sndcp, Self::todo_to_mle_handle(prim.handle));
                SapMsgInner::TlaTlUnitdataReqBl(TlaTlUnitdataReqBl {
                    main_address: prim.address,
                    link_id: prim.link_id,
                    endpoint_id: prim.endpoint_id,
                    tl_sdu: pdu,
                    pdu_prio: prim.pdu_prio,
                    stealing_permission: prim.stealing_permission,
                    subscriber_class,
                    fcs_flag: prim.fcs_flag,
                    air_interface_encryption: None,
                    stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                    packet_data_flag: true,
                    n_tlsdu_repeats: Self::ltpd_todo_to_optional_u8(prim.unacked_bl_repetitions),
                    data_class_info: Self::ltpd_todo_to_optional_todo(prim.data_class_info),
                    req_handle: tla_handle,
                    chan_alloc: None,
                    tx_reporter: None,
                })
            }
            Layer2Service::AcknowledgedResponse => SapMsgInner::TlaTlDataRespBl(TlDataRespBl {
                main_address: prim.address,
                link_id: prim.link_id,
                endpoint_id: prim.endpoint_id,
                tl_sdu: pdu,
                scrambling_code: 0,
                pdu_prio: prim.pdu_prio,
                stealing_permission: prim.stealing_permission,
                subscriber_class,
                fcs_flag: prim.fcs_flag,
                air_interface_encryption: 0,
                stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                data_class_info: Self::ltpd_todo_to_optional_todo(prim.data_class_info),
                req_handle: prim.handle,
            }),
            Layer2Service::Acknowledged => {
                let tla_handle = self.track_tla_data_request(MleSapUser::Sndcp, Self::todo_to_mle_handle(prim.handle));
                SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
                    main_address: prim.address,
                    link_id: prim.link_id,
                    endpoint_id: prim.endpoint_id,
                    tl_sdu: pdu,
                    pdu_prio: prim.pdu_prio,
                    stealing_permission: prim.stealing_permission,
                    subscriber_class,
                    fcs_flag: prim.fcs_flag,
                    air_interface_encryption: None,
                    stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                    data_class_info: Self::ltpd_todo_to_optional_todo(prim.data_class_info),
                    req_handle: tla_handle,
                    graceful_degradation: None,
                    chan_alloc: None,
                    tx_reporter: None,
                })
            }
            Layer2Service::Todo => unreachable!("Layer2Service::Todo rejected before service selection"),
        };

        queue.push_back(SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Mle,
            dest: TetraEntity::Llc,
            msg,
        });
    }

    fn rx_tlpd_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tlpd_prim");
        match &message.msg {
            SapMsgInner::LtpdMleUnitdataReq(_) => {
                self.rx_ltpd_mle_unitdata_req(queue, message);
            }
            _ => {
                tracing::warn!("unhandled match variant, ignoring");
            }
        }
    }

    fn rx_lcmc_mle_unitdata_req(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_lcmc_mle_unitdata_req");
        let SapMsgInner::LcmcMleUnitdataReq(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        if prim.layer2service == Layer2Service::Todo {
            // EN 300 392-2 clause 18.3.5.3.1 defines the permitted LLC
            // service selections. Do not silently promote an unspecified
            // CMCE request to acknowledged transfer.
            tracing::error!("MLE-BS: rejecting CMCE MLE-UNITDATA with unspecified Layer2Service::Todo");
            return;
        }

        let mle_prot_discriminator = MleProtocolDiscriminator::Cmce;
        let sdu_len = prim.sdu.get_len();
        let mut pdu = BitBuffer::new(3 + sdu_len);
        pdu.write_bits(mle_prot_discriminator.into_raw(), 3);
        pdu.copy_bits(&mut prim.sdu, sdu_len);
        pdu.seek(0);

        // let (_addr, link, endpoint) = self.router.use_handle(prim.handle, message.dltime);
        // assert_eq!(link, prim.link_id);
        // assert_eq!(endpoint, prim.endpoint_id);
        let subscriber_class = self.subscriber_class();
        let msg = match prim.layer2service {
            Layer2Service::Unacknowledged => {
                // Unacknowledged service, send a TlUnitdataReqBl.
                let tla_handle = self.track_tla_data_request(MleSapUser::Cmce, prim.handle);
                SapMsgInner::TlaTlUnitdataReqBl(TlaTlUnitdataReqBl {
                    main_address: prim.main_address,
                    link_id: prim.link_id,
                    endpoint_id: prim.endpoint_id,
                    tl_sdu: pdu,
                    pdu_prio: prim.pdu_prio,
                    stealing_permission: prim.stealing_permission,
                    subscriber_class,
                    fcs_flag: false,
                    air_interface_encryption: None,
                    stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                    packet_data_flag: false,
                    n_tlsdu_repeats: None,
                    data_class_info: None,
                    // EN 300 392-2 clauses 20.4.1.1.3 and 22.3.2.4.1 use
                    // request handles to relate subsequent LLC/MAC reports
                    // to the original TL-UNITDATA request. CMCE frequently
                    // supplies upper-layer handle 0 for D-TX/D-RELEASE FACCH
                    // messages, so BS MLE must allocate a unique lower-layer
                    // handle before passing the primitive to LLC.
                    req_handle: tla_handle,

                    chan_alloc: prim.chan_alloc.take(),
                    tx_reporter: prim.tx_reporter.take(),
                })
            }
            Layer2Service::AcknowledgedResponse => SapMsgInner::TlaTlDataRespBl(TlDataRespBl {
                main_address: prim.main_address,
                link_id: prim.link_id,
                endpoint_id: prim.endpoint_id,
                tl_sdu: pdu,
                scrambling_code: 0,
                pdu_prio: prim.pdu_prio,
                stealing_permission: prim.stealing_permission,
                subscriber_class,
                fcs_flag: false,
                air_interface_encryption: 0,
                stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                data_class_info: None,
                req_handle: Self::mle_handle_to_todo(prim.handle),
            }),
            Layer2Service::Acknowledged => {
                // Acknowledged request service, send a TlDataReqBl.
                let tla_handle = self.track_tla_data_request(MleSapUser::Cmce, prim.handle);
                SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
                    main_address: prim.main_address,
                    link_id: prim.link_id,
                    endpoint_id: prim.endpoint_id,
                    tl_sdu: pdu,
                    pdu_prio: prim.pdu_prio,
                    stealing_permission: prim.stealing_permission,
                    subscriber_class,
                    fcs_flag: false,
                    air_interface_encryption: None,
                    stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                    data_class_info: None,
                    req_handle: tla_handle,
                    graceful_degradation: None,
                    chan_alloc: prim.chan_alloc.take(),
                    tx_reporter: prim.tx_reporter.take(),
                })
            }
            Layer2Service::Todo => unreachable!("Layer2Service::Todo rejected before service selection"),
        };
        let sapmsg = SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Mle,
            dest: TetraEntity::Llc,
            msg,
        };

        queue.push_back(sapmsg);
    }

    fn rx_lcmc_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_lcmc_prim");
        match &message.msg {
            SapMsgInner::LcmcMleUnitdataReq(_) => {
                self.rx_lcmc_mle_unitdata_req(queue, message);
            }
            _ => {
                tracing::warn!("unhandled match variant, ignoring");
            }
        }
    }
}

impl TetraEntityTrait for MleBs {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Mle
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        // Broadcast D-NWRK-BROADCAST twice per hyperframe (~30.6s interval) if timezone is configured.
        // Two evenly-spaced slots [20, 50] avoid congestion with other hyperframe-triggered events
        // and give terminals a faster time/date update after cold attach.
        if MLE_BROADCAST_MULTIFRAMES.contains(&ts.m) && ts.f == MLE_BROADCAST_FRAME && ts.t == 1 {
            tracing::debug!("MLE: hyperframe broadcast slot (hf={} m={} f={} t={})", ts.h, ts.m, ts.f, ts.t);
            let req_handle = self.allocate_tla_handle();
            if self.broadcast.send_broadcast(queue, req_handle) {
                self.pending_data_transfers.insert(
                    req_handle,
                    PendingMleTransfer {
                        user: MleSapUser::Broadcast,
                        upper_handle: 0,
                    },
                );
            }
        }
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);
        // tracing::debug!(ts=%message.dltime, "rx_prim: {:?}", message);

        match message.sap {
            Sap::TlaSap => {
                self.rx_tla_prim(queue, message);
            }
            Sap::TlmbSap => {
                tracing::warn!("MLE: BS received unexpected broadcast message on TlmbSap, ignoring");
            }
            Sap::TlmcSap => {
                self.rx_tlmc_prim(queue, message);
            }
            Sap::LmmSap => {
                self.rx_lmm_prim(queue, message);
            }
            Sap::TlpdSap => {
                self.rx_tlpd_prim(queue, message);
            }
            Sap::LcmcSap => {
                self.rx_lcmc_prim(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }
}
