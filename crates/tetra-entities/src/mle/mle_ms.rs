use crate::mle::components::mle_router::MleRouter;
use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Layer2Service, MleHandle, Sap, TdmaTime, Todo, unimplemented_log};
use tetra_saps::lcmc::LcmcMleUnitdataInd;
use tetra_saps::lmm::LmmMleUnitdataInd;
use tetra_saps::ltpd::LtpdMleUnitdataInd;
use tetra_saps::tla::{TlDataRespBl, TlaTlDataReqBl, TlaTlUnitdataReqBl};
use tetra_saps::{SapMsg, SapMsgInner};

use tetra_pdus::mle::enums::mle_pdu_type_dl::MlePduTypeDl;
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;
use tetra_pdus::mle::pdus::d_mle_sync::DMleSync;
use tetra_pdus::mle::pdus::d_mle_sysinfo::DMleSysinfo;

pub struct MleMs {
    config: SharedConfig,
    router: MleRouter,
    dltime: TdmaTime,
}

impl MleMs {
    pub fn new(config: SharedConfig) -> Self {
        Self {
            config,
            router: MleRouter::new(),
            dltime: TdmaTime::default(),
        }
    }

    fn mle_handle_to_todo(handle: MleHandle) -> Todo {
        if handle <= i32::MAX as MleHandle {
            handle as Todo
        } else {
            i32::MAX
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

    fn rx_tla_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tla_prim");
        match message.msg {
            SapMsgInner::TlaTlDataIndBl(_) => {
                self.rx_tla_data_ind_bl(queue, message);
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

    fn rx_tla_data_ind_bl(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        // Take ownership of bitbuf and read protocol discriminator
        let SapMsgInner::TlaTlDataIndBl(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let Some(mut sdu) = prim.tl_sdu.take() else {
            tracing::warn!("MLE: received message with no tl_sdu, ignoring");
            return;
        };
        if sdu.get_pos() != 0 {
            tracing::warn!("MLE: sdu not at start position (pos={}), seeking to 0", sdu.get_pos());
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

        // Dispatch to appropriate component (or to self if for MLE)
        match pdu_type {
            MleProtocolDiscriminator::Mm => {
                let handle = self
                    .router
                    .create_handle(prim.main_address, prim.link_id, prim.endpoint_id, self.dltime);
                let m = LmmMleUnitdataInd {
                    sdu,
                    handle,
                    received_address: prim.main_address,
                };
                let msg = SapMsg {
                    sap: Sap::LmmSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Mm,
                    msg: SapMsgInner::LmmMleUnitdataInd(m),
                };
                queue.push_back(msg);
            }
            MleProtocolDiscriminator::Cmce => {
                let handle = self
                    .router
                    .create_handle(prim.main_address, prim.link_id, prim.endpoint_id, self.dltime);
                let m = LcmcMleUnitdataInd {
                    sdu,
                    handle,
                    received_tetra_address: prim.main_address,
                    endpoint_id: prim.endpoint_id,
                    link_id: prim.link_id,
                    chan_change_resp_req: false,
                    chan_change_handle: None,
                };
                let msg = SapMsg {
                    sap: Sap::LcmcSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Cmce,
                    msg: SapMsgInner::LcmcMleUnitdataInd(m),
                };
                queue.push_back(msg);
            }
            MleProtocolDiscriminator::Sndcp => {
                let m = LtpdMleUnitdataInd {
                    sdu,
                    endpoint_id: prim.endpoint_id,
                    link_id: prim.link_id,
                    received_tetra_address: prim.main_address,
                    chan_change_resp_req: false,
                    chan_change_handle: None,
                };
                let msg = SapMsg {
                    sap: Sap::TlpdSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Sndcp,
                    msg: SapMsgInner::LtpdMleUnitdataInd(m),
                };
                queue.push_back(msg);
            }
            MleProtocolDiscriminator::Mle => {
                self.rx_tla_mle_pdu(queue, message);
            }
            MleProtocolDiscriminator::TetraManagementEntity => {
                unimplemented_log!("MleProtocolDiscriminator::TetraManagementEntity");
            }
        }
    }

    fn rx_tla_unitdata_ind_bl(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        // TL-UNITDATA is the unacknowledged LLC service; after the LLC service
        // boundary the MLE discriminator still selects the same upper SAP.

        // Take ownership of bitbuf and read protocol discriminator
        let SapMsgInner::TlaTlUnitdataIndBl(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let Some(mut sdu) = prim.tl_sdu.take() else {
            tracing::warn!("MLE: received message with no tl_sdu, ignoring");
            return;
        };
        if sdu.get_pos() != 0 {
            tracing::warn!("MLE: sdu not at start position (pos={}), seeking to 0", sdu.get_pos());
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

        // Dispatch to appropriate component (or to self if for MLE)
        match pdu_type {
            MleProtocolDiscriminator::Mm => {
                let handle = self
                    .router
                    .create_handle(prim.main_address, prim.link_id, prim.endpoint_id, self.dltime);
                let m = LmmMleUnitdataInd {
                    sdu,
                    handle,
                    received_address: prim.main_address,
                };
                let msg = SapMsg {
                    sap: Sap::LmmSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Mm,
                    msg: SapMsgInner::LmmMleUnitdataInd(m),
                };
                queue.push_back(msg);
            }
            MleProtocolDiscriminator::Cmce => {
                let handle = self
                    .router
                    .create_handle(prim.main_address, prim.link_id, prim.endpoint_id, self.dltime);
                let m = LcmcMleUnitdataInd {
                    sdu,
                    handle,
                    endpoint_id: prim.endpoint_id,
                    link_id: prim.link_id,
                    received_tetra_address: prim.main_address,
                    chan_change_resp_req: false,
                    chan_change_handle: None,
                };
                let msg = SapMsg {
                    sap: Sap::LcmcSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Cmce,
                    msg: SapMsgInner::LcmcMleUnitdataInd(m),
                };
                queue.push_back(msg);
            }
            MleProtocolDiscriminator::Sndcp => {
                let m = LtpdMleUnitdataInd {
                    sdu,
                    endpoint_id: prim.endpoint_id,
                    link_id: prim.link_id,
                    received_tetra_address: prim.main_address,
                    chan_change_resp_req: false,
                    chan_change_handle: None,
                };
                let msg = SapMsg {
                    sap: Sap::TlpdSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Sndcp,
                    msg: SapMsgInner::LtpdMleUnitdataInd(m),
                };
                queue.push_back(msg);
            }
            MleProtocolDiscriminator::Mle => {
                self.rx_tla_mle_pdu(queue, message);
            }
            MleProtocolDiscriminator::TetraManagementEntity => {
                unimplemented_log!("MleProtocolDiscriminator::TetraManagementEntity");
            }
        }
    }

    fn rx_tlmb_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tlmb_prim");
        match message.msg {
            SapMsgInner::TlmbSysinfoInd(_) => {
                self.rx_tlmb_tl_sysinfo_ind(queue, message);
            }
            SapMsgInner::TlmbSyncInd(_) => {
                self.rx_tlmb_tl_sync_ind(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }

    pub fn rx_tlmb_tl_sysinfo_ind(&self, _queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_tlmb_tl_sysinfo_ind");

        let SapMsgInner::TlmbSysinfoInd(inner) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        // Parse the TL-SDU
        let _pdu = match DMleSysinfo::from_bitbuf(&mut inner.tl_sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing DMleSysinfo: {:?} {}", e, inner.tl_sdu.dump_bin());
                return;
            }
        };

        unimplemented_log!("rx_tlmb_tl_sysinfo_ind");
        // let need_global_state_update = {
        //     let cfg = self.config.read();

        //     pdu.location_area != cfg.la_info.location_area
        //     || pdu.subscriber_class != cfg.la_info.subscriber_class
        //     || pdu.bs_service_details.registration != cfg.la_info.registration
        //     || pdu.bs_service_details.deregistration != cfg.la_info.deregistration
        //     || pdu.bs_service_details.priority_cell != cfg.la_info.priority_cell
        //     || pdu.bs_service_details.no_minimum_mode != cfg.la_info.no_minimum_mode
        //     || pdu.bs_service_details.migration != cfg.la_info.migration
        //     || pdu.bs_service_details.system_wide_services != cfg.la_info.system_wide_services
        //     || pdu.bs_service_details.voice_service != cfg.la_info.voice_service
        //     || pdu.bs_service_details.circuit_mode_data_service != cfg.la_info.circuit_mode_data_service
        //     || pdu.bs_service_details.sndcp_service != cfg.la_info.sndcp_service
        //     || pdu.bs_service_details.aie_service != cfg.la_info.aie_service
        //     || pdu.bs_service_details.advanced_link != cfg.la_info.advanced_link
        // };

        // if need_global_state_update {
        //     let mut cfg = self.config.write();
        //     cfg.la_info.location_area = pdu.location_area;
        //     cfg.la_info.subscriber_class = pdu.subscriber_class;
        //     cfg.la_info.registration = pdu.bs_service_details.registration;
        //     cfg.la_info.deregistration = pdu.bs_service_details.deregistration;
        //     cfg.la_info.priority_cell = pdu.bs_service_details.priority_cell;
        //     cfg.la_info.no_minimum_mode = pdu.bs_service_details.no_minimum_mode;
        //     cfg.la_info.migration = pdu.bs_service_details.migration;
        //     cfg.la_info.system_wide_services = pdu.bs_service_details.system_wide_services;
        //     cfg.la_info.voice_service = pdu.bs_service_details.voice_service;
        //     cfg.la_info.circuit_mode_data_service = pdu.bs_service_details.circuit_mode_data_service;
        //     cfg.la_info.sndcp_service = pdu.bs_service_details.sndcp_service;
        //     cfg.la_info.aie_service = pdu.bs_service_details.aie_service;
        //     cfg.la_info.advanced_link = pdu.bs_service_details.advanced_link;
        //     tracing::info!("Updated TetraGlobalState: {:?}", pdu);
        // } else {
        //     tracing::trace!("rx_tlmb_tl_sysinfo_ind: TetraGlobalState update not required");
        // }
    }

    pub fn rx_tlmb_tl_sync_ind(&self, _queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_tlmb_tl_sync_ind");

        let SapMsgInner::TlmbSyncInd(inner) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        // Parse the TL-SDU
        let _pdu = match DMleSync::from_bitbuf(&mut inner.tl_sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing DMleSync: {:?} {}", e, inner.tl_sdu.dump_bin());
                return;
            }
        };

        unimplemented_log!("rx_tlmb_tl_sync_ind");

        // MLE-MS broadcast state update is intentionally not implemented in
        // this clause-scoped patch; D-MLE-SYNC parsing stays fail-safe.
    }

    fn rx_tlmc_prim(&mut self, _queue: &mut MessageQueue, _message: SapMsg) {
        tracing::trace!("rx_tlmc_prim");
        // TLMC SAP not implemented yet. Log instead of panicking so an unexpected
        // primitive doesn't kill the whole MLE worker.
        unimplemented_log!("rx_tlmc_prim called but TLMC SAP is not implemented");
        // match &message.msg {
        //     _ => {
        //         panic!();
        //     }
        // }
    }

    fn rx_lmm_mle_unitdata_req(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_lmm_mle_unitdata_req");
        let SapMsgInner::LmmMleUnitdataReq(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        if prim.layer2service == Layer2Service::Todo {
            // EN 300 392-2 clause 18.3.5.3.1 permits acknowledged request,
            // acknowledged response, and unacknowledged service. Do not infer
            // an LLC service from the legacy placeholder.
            tracing::error!("MLE-MS: rejecting MM MLE-UNITDATA with unspecified Layer2Service::Todo");
            return;
        }

        let mle_prot_discriminator = MleProtocolDiscriminator::Mm;
        let sdu_len = prim.sdu.get_len();
        let mut pdu = BitBuffer::new(3 + sdu_len);
        pdu.write_bits(mle_prot_discriminator.into_raw(), 3);
        pdu.copy_bits(&mut prim.sdu, sdu_len);
        pdu.seek(0);

        // let (addr, link, endpoint) = self.router.use_handle(prim.handle, self.dltime);
        // assert_eq!(addr.ssi, prim.address.ssi);
        let req_handle = Self::mle_handle_to_todo(prim.handle);
        let msg = match prim.layer2service {
            Layer2Service::Unacknowledged => SapMsgInner::TlaTlUnitdataReqBl(TlaTlUnitdataReqBl {
                main_address: prim.address,
                link_id: 0,
                endpoint_id: 0,
                tl_sdu: pdu,
                pdu_prio: 0,
                stealing_permission: prim.stealing_permission,
                subscriber_class: 0,
                fcs_flag: false,
                air_interface_encryption: None,
                stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                packet_data_flag: false,
                n_tlsdu_repeats: None,
                data_class_info: None,
                // EN 300 392-2 clause 18.3.5.3.1: unacknowledged
                // MLE-UNITDATA from MM maps to TL-UNITDATA after the MLE
                // protocol discriminator is prefixed.
                req_handle,
                chan_alloc: None,
                tx_reporter: prim.tx_reporter.take(),
            }),
            Layer2Service::AcknowledgedResponse => SapMsgInner::TlaTlDataRespBl(TlDataRespBl {
                main_address: prim.address,
                link_id: 0,
                endpoint_id: 0,
                tl_sdu: pdu,
                scrambling_code: 0,
                pdu_prio: 0,
                stealing_permission: prim.stealing_permission,
                subscriber_class: 0,
                fcs_flag: false,
                air_interface_encryption: 0,
                stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                data_class_info: None,
                req_handle,
            }),
            Layer2Service::Acknowledged => SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
                main_address: prim.address,
                link_id: 0,
                endpoint_id: 0,
                tl_sdu: pdu,
                pdu_prio: 0,
                stealing_permission: prim.stealing_permission,
                subscriber_class: 0,
                fcs_flag: false,
                air_interface_encryption: None,
                stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                data_class_info: None,
                req_handle,
                graceful_degradation: None,
                chan_alloc: None,
                tx_reporter: prim.tx_reporter.take(),
            }),
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
            tracing::error!("MLE-MS: rejecting SNDCP MLE-UNITDATA with unspecified Layer2Service::Todo");
            return;
        }

        let sdu_len = prim.sdu.get_len();
        let mut pdu = BitBuffer::new(3 + sdu_len);
        pdu.write_bits(MleProtocolDiscriminator::Sndcp.into_raw(), 3);
        pdu.copy_bits(&mut prim.sdu, sdu_len);
        pdu.seek(0);

        let req_handle = prim.handle;
        let msg = match prim.layer2service {
            Layer2Service::Unacknowledged => SapMsgInner::TlaTlUnitdataReqBl(TlaTlUnitdataReqBl {
                main_address: prim.address,
                link_id: prim.link_id,
                endpoint_id: prim.endpoint_id,
                tl_sdu: pdu,
                pdu_prio: prim.pdu_prio,
                stealing_permission: prim.stealing_permission,
                subscriber_class: 0,
                fcs_flag: prim.fcs_flag,
                air_interface_encryption: None,
                stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                packet_data_flag: true,
                n_tlsdu_repeats: Self::ltpd_todo_to_optional_u8(prim.unacked_bl_repetitions),
                data_class_info: Self::ltpd_todo_to_optional_todo(prim.data_class_info),
                req_handle,
                chan_alloc: None,
                tx_reporter: None,
            }),
            Layer2Service::AcknowledgedResponse => SapMsgInner::TlaTlDataRespBl(TlDataRespBl {
                main_address: prim.address,
                link_id: prim.link_id,
                endpoint_id: prim.endpoint_id,
                tl_sdu: pdu,
                scrambling_code: 0,
                pdu_prio: prim.pdu_prio,
                stealing_permission: prim.stealing_permission,
                subscriber_class: 0,
                fcs_flag: prim.fcs_flag,
                air_interface_encryption: 0,
                stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                data_class_info: Self::ltpd_todo_to_optional_todo(prim.data_class_info),
                req_handle,
            }),
            Layer2Service::Acknowledged => SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
                main_address: prim.address,
                link_id: prim.link_id,
                endpoint_id: prim.endpoint_id,
                tl_sdu: pdu,
                pdu_prio: prim.pdu_prio,
                stealing_permission: prim.stealing_permission,
                subscriber_class: 0,
                fcs_flag: prim.fcs_flag,
                air_interface_encryption: None,
                stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                data_class_info: Self::ltpd_todo_to_optional_todo(prim.data_class_info),
                req_handle,
                graceful_degradation: None,
                chan_alloc: None,
                tx_reporter: None,
            }),
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
            // EN 300 392-2 clause 18.3.5.3.1 has no unspecified/default
            // service. CMCE must choose the LLC service explicitly.
            tracing::error!("MLE-MS: rejecting CMCE MLE-UNITDATA with unspecified Layer2Service::Todo");
            return;
        }

        let mle_prot_discriminator = MleProtocolDiscriminator::Cmce;
        let sdu_len = prim.sdu.get_len();
        let mut pdu = BitBuffer::new(3 + sdu_len);
        pdu.write_bits(mle_prot_discriminator.into_raw(), 3);
        pdu.copy_bits(&mut prim.sdu, sdu_len);
        pdu.seek(0);

        // let (_addr, link, endpoint) = self.router.use_handle(prim.handle, self.dltime);
        // assert_eq!(link, prim.link_id);
        // assert_eq!(endpoint, prim.endpoint_id);
        // Take Channel Allocation Request if any
        let chan_alloc = prim.chan_alloc.take();

        let req_handle = Self::mle_handle_to_todo(prim.handle);
        let msg = match prim.layer2service {
            Layer2Service::Unacknowledged => SapMsgInner::TlaTlUnitdataReqBl(TlaTlUnitdataReqBl {
                main_address: prim.main_address,
                link_id: prim.link_id,
                endpoint_id: prim.endpoint_id,
                tl_sdu: pdu,
                pdu_prio: prim.pdu_prio,
                stealing_permission: prim.stealing_permission,
                subscriber_class: 0,
                fcs_flag: false,
                air_interface_encryption: None,
                stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                packet_data_flag: false,
                n_tlsdu_repeats: prim.unacked_bl_repetitions,
                data_class_info: None,
                req_handle,
                chan_alloc,
                tx_reporter: prim.tx_reporter.take(),
            }),
            Layer2Service::AcknowledgedResponse => SapMsgInner::TlaTlDataRespBl(TlDataRespBl {
                main_address: prim.main_address,
                link_id: prim.link_id,
                endpoint_id: prim.endpoint_id,
                tl_sdu: pdu,
                scrambling_code: 0,
                pdu_prio: prim.pdu_prio,
                stealing_permission: prim.stealing_permission,
                subscriber_class: 0,
                fcs_flag: false,
                air_interface_encryption: 0,
                stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                data_class_info: None,
                req_handle,
            }),
            Layer2Service::Acknowledged => SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
                main_address: prim.main_address,
                link_id: prim.link_id,
                endpoint_id: prim.endpoint_id,
                tl_sdu: pdu,
                pdu_prio: prim.pdu_prio,
                stealing_permission: prim.stealing_permission,
                subscriber_class: 0,
                fcs_flag: false,
                air_interface_encryption: None,
                stealing_repeats_flag: Some(prim.stealing_repeats_flag),
                data_class_info: None,
                req_handle,
                graceful_degradation: None,
                chan_alloc,
                tx_reporter: prim.tx_reporter.take(),
            }),
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
            SapMsgInner::LcmcMleConfigureReq(prim) => {
                // EN 300 392-2 clause 14.5.1.3.3 requires CMCE to issue a
                // lower-layer CONFIGURE request when release/disconnect clears
                // a circuit-mode call. Full MS U-plane channel switching is
                // still a lower MAC implementation boundary; consume the
                // primitive explicitly so release cleanup is not dropped as an
                // unknown LCMC message.
                tracing::debug!(
                    "MLE-MS: CMCE configure endpoint={} switch_u_plane={} tx_grant={} chan_change_accepted={:?}",
                    prim.endpoint_id,
                    prim.switch_u_plane,
                    prim.tx_grant,
                    prim.chan_change_accepted
                );
            }
            SapMsgInner::LcmcMleUnitdataReq(_) => {
                self.rx_lcmc_mle_unitdata_req(queue, message);
            }
            _ => {
                tracing::warn!("unhandled match variant, ignoring");
            }
        }
    }
}

impl TetraEntityTrait for MleMs {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Mle
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);
        match message.sap {
            Sap::TlaSap => {
                self.rx_tla_prim(queue, message);
            }
            Sap::TlmbSap => {
                self.rx_tlmb_prim(queue, message);
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

    fn tick_start(&mut self, _queue: &mut MessageQueue, ts: TdmaTime) {
        self.dltime = ts;
    }
}
