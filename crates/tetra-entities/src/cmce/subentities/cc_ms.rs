use tetra_core::{BitBuffer, Layer2Service, Sap, tetra_entities::TetraEntity, unimplemented_log};
use tetra_pdus::cmce::enums::cmce_pdu_type_dl::CmcePduTypeDl;
use tetra_pdus::cmce::enums::disconnect_cause::DisconnectCause;
use tetra_pdus::cmce::enums::transmission_grant::TransmissionGrant;
use tetra_pdus::cmce::fields::basic_service_information::BasicServiceInformation;
use tetra_pdus::cmce::pdus::d_connect::DConnect;
use tetra_pdus::cmce::pdus::d_connect_acknowledge::DConnectAcknowledge;
use tetra_pdus::cmce::pdus::d_disconnect::DDisconnect;
use tetra_pdus::cmce::pdus::d_release::DRelease;
use tetra_pdus::cmce::pdus::d_setup::DSetup;
use tetra_pdus::cmce::pdus::d_tx_granted::DTxGranted;
use tetra_pdus::cmce::pdus::u_connect::UConnect;
use tetra_pdus::cmce::pdus::u_disconnect::UDisconnect;
use tetra_pdus::cmce::pdus::u_release::URelease;
use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
use tetra_saps::control::enums::communication_type::CommunicationType;
use tetra_saps::lcmc::{LcmcMleConfigureReq, LcmcMleUnitdataInd, LcmcMleUnitdataReq};
use tetra_saps::{SapMsg, SapMsgInner};

use crate::MessageQueue;

/// Clause 11 Call Control CMCE sub-entity
pub struct CcMsSubentity {
    active_call: Option<MsCallContext>,
}

#[derive(Debug, Clone, Copy)]
struct MsCallContext {
    call_identifier: u16,
    circuit_mode_type: CircuitModeType,
    simplex_duplex: bool,
}

impl CcMsSubentity {
    pub fn new() -> Self {
        CcMsSubentity { active_call: None }
    }

    fn remember_call(&mut self, call_identifier: u16, circuit_mode_type: CircuitModeType, simplex_duplex: bool) {
        self.active_call = Some(MsCallContext {
            call_identifier,
            circuit_mode_type,
            simplex_duplex,
        });
    }

    fn u_plane_config_for_grant(transmission_grant: TransmissionGrant) -> (bool, bool) {
        match transmission_grant {
            TransmissionGrant::Granted => (true, true),
            TransmissionGrant::GrantedToOtherUser => (true, false),
            TransmissionGrant::NotGranted | TransmissionGrant::RequestQueued => (false, false),
        }
    }

    fn send_connect_configure(
        queue: &mut MessageQueue,
        prim: &LcmcMleUnitdataInd,
        circuit_mode_type: CircuitModeType,
        simplex_duplex: bool,
        encryption_flag: bool,
        transmission_grant: TransmissionGrant,
    ) {
        let (switch_u_plane, tx_grant) = Self::u_plane_config_for_grant(transmission_grant);

        queue.push_back(SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleConfigureReq(LcmcMleConfigureReq {
                endpoint_id: prim.endpoint_id,
                chan_change_accepted: prim.chan_change_resp_req.then_some(true),
                chan_change_handle: prim.chan_change_handle.unwrap_or_default(),
                call_release: None,
                encryption_flag,
                circuit_mode_type,
                add_temp_gssi: None,
                del_temp_gssi: None,
                simplex_duplex,
                tx_grant,
                switch_u_plane,
            }),
        });
    }

    fn send_release_configure(&mut self, queue: &mut MessageQueue, prim: &LcmcMleUnitdataInd, call_identifier: u16) {
        let context = self.active_call.take().filter(|context| context.call_identifier == call_identifier);
        let circuit_mode_type = context.map(|context| context.circuit_mode_type).unwrap_or(CircuitModeType::TchS);
        let simplex_duplex = context.map(|context| context.simplex_duplex).unwrap_or(false);

        queue.push_back(SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleConfigureReq(LcmcMleConfigureReq {
                endpoint_id: prim.endpoint_id,
                chan_change_accepted: prim.chan_change_resp_req.then_some(true),
                chan_change_handle: prim.chan_change_handle.unwrap_or_default(),
                call_release: Some(call_identifier as i32),
                encryption_flag: false,
                circuit_mode_type,
                add_temp_gssi: None,
                del_temp_gssi: None,
                simplex_duplex,
                tx_grant: false,
                switch_u_plane: false,
            }),
        });
    }

    fn send_u_release(queue: &mut MessageQueue, prim: &LcmcMleUnitdataInd, pdu: &DDisconnect) {
        let u_release = URelease {
            call_identifier: pdu.call_identifier,
            disconnect_cause: pdu.disconnect_cause,
            facility: None,
            proprietary: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(32);
        if let Err(err) = u_release.to_bitbuf(&mut sdu) {
            tracing::warn!(
                "CMCE-MS: failed to serialize U-RELEASE for call_id={}: {:?}",
                pdu.call_identifier,
                err
            );
            return;
        }
        sdu.seek(0);

        queue.push_back(SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: prim.handle,
                endpoint_id: prim.endpoint_id,
                link_id: prim.link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: matches!(prim.endpoint_id, 2..=4),
                stealing_repeats_flag: false,
                unacked_bl_repetitions: None,
                main_address: prim.received_tetra_address,
                chan_alloc: None,
                tx_reporter: None,
            }),
        });
    }

    fn send_u_connect(queue: &mut MessageQueue, prim: &LcmcMleUnitdataInd, pdu: &DSetup) {
        let u_connect = UConnect {
            call_identifier: pdu.call_identifier,
            hook_method_selection: pdu.hook_method_selection,
            simplex_duplex_selection: pdu.simplex_duplex_selection,
            basic_service_information: None,
            facility: None,
            proprietary: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(32);
        if let Err(err) = u_connect.to_bitbuf(&mut sdu) {
            tracing::warn!(
                "CMCE-MS: failed to serialize U-CONNECT for call_id={}: {:?}",
                pdu.call_identifier,
                err
            );
            return;
        }
        sdu.seek(0);

        queue.push_back(SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: prim.handle,
                endpoint_id: prim.endpoint_id,
                link_id: prim.link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: matches!(prim.endpoint_id, 2..=4),
                stealing_repeats_flag: false,
                unacked_bl_repetitions: None,
                main_address: prim.received_tetra_address,
                chan_alloc: None,
                tx_reporter: None,
            }),
        });
    }

    fn send_u_disconnect(queue: &mut MessageQueue, prim: &LcmcMleUnitdataInd, call_identifier: u16, disconnect_cause: DisconnectCause) {
        let u_disconnect = UDisconnect {
            call_identifier,
            disconnect_cause,
            facility: None,
            proprietary: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(32);
        if let Err(err) = u_disconnect.to_bitbuf(&mut sdu) {
            tracing::warn!(
                "CMCE-MS: failed to serialize U-DISCONNECT for call_id={}: {:?}",
                call_identifier,
                err
            );
            return;
        }
        sdu.seek(0);

        queue.push_back(SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: prim.handle,
                endpoint_id: prim.endpoint_id,
                link_id: prim.link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: matches!(prim.endpoint_id, 2..=4),
                stealing_repeats_flag: false,
                unacked_bl_repetitions: None,
                main_address: prim.received_tetra_address,
                chan_alloc: None,
                tx_reporter: None,
            }),
        });
    }

    fn supports_direct_private_setup(bsi: &BasicServiceInformation) -> bool {
        bsi.circuit_mode_type == CircuitModeType::TchS && bsi.communication_type == CommunicationType::P2p && !bsi.encryption_flag
    }

    fn direct_private_setup_reject_cause(bsi: &BasicServiceInformation) -> DisconnectCause {
        if bsi.encryption_flag {
            DisconnectCause::CalledPartyDoesNotSupportEncryption
        } else {
            DisconnectCause::CallRejectedByTheCalledParty
        }
    }

    fn rx_d_disconnect(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("rx_d_disconnect: expected LcmcMleUnitdataInd, got unexpected SAP message type");
            return;
        };

        let pdu = match DDisconnect::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- D-DISCONNECT {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing D-DISCONNECT: {:?}", e);
                return;
            }
        };

        // EN 300 392-2 clause 14.5.1.3.3: an MS receiving D-DISCONNECT shall
        // acknowledge the network-initiated disconnect with U-RELEASE, clear
        // call state, and tell lower layers to switch U-plane off.
        Self::send_u_release(queue, prim, &pdu);
        self.send_release_configure(queue, prim, pdu.call_identifier);
    }

    fn rx_d_release(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("rx_d_release: expected LcmcMleUnitdataInd, got unexpected SAP message type");
            return;
        };

        let pdu = match DRelease::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- D-RELEASE {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing D-RELEASE: {:?}", e);
                return;
            }
        };

        // EN 300 392-2 clause 14.5.1.3.3: D-RELEASE clears the call without
        // any uplink CMCE response, then tells lower layers to switch U-plane
        // off and leave the assigned channel if present.
        tracing::debug!(
            "CMCE-MS: D-RELEASE call_id={} cause={:?}; no U-RELEASE response required",
            pdu.call_identifier,
            pdu.disconnect_cause
        );
        self.send_release_configure(queue, prim, pdu.call_identifier);
    }

    fn rx_d_setup(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("rx_d_setup: expected LcmcMleUnitdataInd, got unexpected SAP message type");
            return;
        };

        let pdu = match DSetup::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- D-SETUP {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing D-SETUP: {:?}", e);
                return;
            }
        };

        if pdu.hook_method_selection {
            tracing::debug!(
                "CMCE-MS: D-SETUP call_id={} uses on/off-hook signalling; waiting for TNCC/user response",
                pdu.call_identifier
            );
            return;
        }

        if !Self::supports_direct_private_setup(&pdu.basic_service_information) {
            let disconnect_cause = Self::direct_private_setup_reject_cause(&pdu.basic_service_information);
            tracing::warn!(
                "CMCE-MS: D-SETUP call_id={} is outside supported direct private speech profile: {:?}; rejecting with {:?}",
                pdu.call_identifier,
                pdu.basic_service_information,
                disconnect_cause
            );
            // EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.5: if the
            // called MS cannot accept or offer the requested basic service it
            // rejects with U-DISCONNECT. Unsupported encryption uses the
            // explicit encryption disconnect causes; this headless MS shim
            // currently supports only unencrypted direct P2P TCH/S.
            Self::send_u_disconnect(queue, prim, pdu.call_identifier, disconnect_cause);
            return;
        }

        // EN 300 392-2 clause 14.5.1.1.1: for direct set-up signalling, once
        // the called application accepts, CC sends U-CONNECT and waits for
        // D-CONNECT ACKNOWLEDGE. This headless MS shim only auto-accepts the
        // narrow unencrypted P2P TCH/S profile above; full TNCC interaction,
        // timers, and complete user-application state handling remain follow-up
        // work. Lower-layer through-connect CONFIGURE is issued on
        // D-CONNECT/D-CONNECT ACKNOWLEDGE.
        self.remember_call(
            pdu.call_identifier,
            pdu.basic_service_information.circuit_mode_type,
            pdu.simplex_duplex_selection,
        );
        Self::send_u_connect(queue, prim, &pdu);
    }

    fn rx_d_connect(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("rx_d_connect: expected LcmcMleUnitdataInd, got unexpected SAP message type");
            return;
        };

        let pdu = match DConnect::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- D-CONNECT {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing D-CONNECT: {:?}", e);
                return;
            }
        };

        // EN 300 392-2 clauses 14.5.1.2.1, 14.7.1.4 and 14.5.1.4.1:
        // D-CONNECT orders the calling MS to through-connect and has no CMCE
        // response PDU; CC shall still issue lower-layer CONFIGURE according
        // to the transmission grant.
        tracing::debug!(
            "CMCE-MS: D-CONNECT call_id={} grant={:?}; no uplink CMCE response required",
            pdu.call_identifier,
            pdu.transmission_grant
        );
        let context = self.active_call;
        let circuit_mode_type = pdu
            .basic_service_information
            .as_ref()
            .map(|bsi| bsi.circuit_mode_type)
            .or_else(|| context.map(|context| context.circuit_mode_type))
            .unwrap_or(CircuitModeType::TchS);
        let encryption_flag = pdu
            .basic_service_information
            .as_ref()
            .map(|bsi| bsi.encryption_flag)
            .unwrap_or(false);
        self.remember_call(pdu.call_identifier, circuit_mode_type, pdu.simplex_duplex_selection);
        Self::send_connect_configure(
            queue,
            prim,
            circuit_mode_type,
            pdu.simplex_duplex_selection,
            encryption_flag,
            pdu.transmission_grant,
        );
    }

    fn rx_d_connect_acknowledge(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("rx_d_connect_acknowledge: expected LcmcMleUnitdataInd, got unexpected SAP message type");
            return;
        };

        let pdu = match DConnectAcknowledge::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- D-CONNECT-ACKNOWLEDGE {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing D-CONNECT-ACKNOWLEDGE: {:?}", e);
                return;
            }
        };

        // EN 300 392-2 clauses 14.5.1.1.1, 14.7.1.5 and 14.5.1.4.1: after
        // U-CONNECT, D-CONNECT ACKNOWLEDGE orders the called MS to
        // through-connect and expects no further CMCE response; CC shall still
        // issue lower-layer CONFIGURE according to the transmission grant.
        tracing::debug!(
            "CMCE-MS: D-CONNECT-ACKNOWLEDGE call_id={} grant={}; no uplink CMCE response required",
            pdu.call_identifier,
            pdu.transmission_grant
        );
        let Some(context) = self.active_call else {
            tracing::warn!(
                "CMCE-MS: D-CONNECT-ACKNOWLEDGE call_id={} without setup context; rejecting invalid call identifier",
                pdu.call_identifier
            );
            Self::send_u_disconnect(queue, prim, pdu.call_identifier, DisconnectCause::InvalidCallIdentifier);
            return;
        };
        if context.call_identifier != pdu.call_identifier {
            tracing::warn!(
                "CMCE-MS: D-CONNECT-ACKNOWLEDGE call_id={} does not match active call_id={}; rejecting invalid call identifier",
                pdu.call_identifier,
                context.call_identifier
            );
            Self::send_u_disconnect(queue, prim, pdu.call_identifier, DisconnectCause::InvalidCallIdentifier);
            return;
        }
        let circuit_mode_type = context.circuit_mode_type;
        let simplex_duplex = context.simplex_duplex;
        Self::send_connect_configure(queue, prim, circuit_mode_type, simplex_duplex, false, pdu.transmission_grant);
    }

    fn rx_d_tx_granted(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("rx_d_tx_granted: expected LcmcMleUnitdataInd, got unexpected SAP message type");
            return;
        };

        let pdu = match DTxGranted::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- D-TX GRANTED {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing D-TX GRANTED: {:?}", e);
                return;
            }
        };

        let Some(context) = self.active_call else {
            tracing::warn!(
                "CMCE-MS: D-TX GRANTED call_id={} without active call; rejecting invalid call identifier",
                pdu.call_identifier
            );
            Self::send_u_disconnect(queue, prim, pdu.call_identifier, DisconnectCause::InvalidCallIdentifier);
            return;
        };
        if context.call_identifier != pdu.call_identifier {
            tracing::warn!(
                "CMCE-MS: D-TX GRANTED call_id={} does not match active call_id={}; rejecting invalid call identifier",
                pdu.call_identifier,
                context.call_identifier
            );
            Self::send_u_disconnect(queue, prim, pdu.call_identifier, DisconnectCause::InvalidCallIdentifier);
            return;
        }

        let transmission_grant = TransmissionGrant::try_from(pdu.transmission_grant as u64)
            .expect("D-TX GRANTED transmission grant is a two-bit validated field");
        match transmission_grant {
            TransmissionGrant::Granted | TransmissionGrant::GrantedToOtherUser => {
                // EN 300 392-2 clause 14.5.1.4.2 and table 14.80:
                // granted/granted-to-other-user switch U-plane on and carry
                // Tx grant true/false respectively. Queued/not-granted leave
                // U-plane state unchanged.
                Self::send_connect_configure(
                    queue,
                    prim,
                    context.circuit_mode_type,
                    context.simplex_duplex,
                    pdu.encryption_control,
                    transmission_grant,
                );
            }
            TransmissionGrant::NotGranted | TransmissionGrant::RequestQueued => {
                tracing::debug!(
                    "CMCE-MS: D-TX GRANTED call_id={} grant={:?}; leaving U-plane state unchanged",
                    pdu.call_identifier,
                    transmission_grant
                );
            }
        }
    }

    pub fn route_rd_deliver(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("route_rd_deliver");

        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let Some(bits) = prim.sdu.peek_bits(5) else {
            tracing::warn!("insufficient bits: {}", prim.sdu.dump_bin());
            return;
        };

        let Ok(pdu_type) = CmcePduTypeDl::try_from(bits) else {
            tracing::warn!("invalid pdu type: {} in {}", bits, prim.sdu.dump_bin());
            return;
        };

        // TODO FIXME: Besides these PDUs, we can also receive several signals (BUSY ind, CLOSE ind, etc)
        match pdu_type {
            CmcePduTypeDl::DAlert => {
                unimplemented_log!("{}", pdu_type);
            }
            CmcePduTypeDl::DCallProceeding => {
                unimplemented_log!("{}", pdu_type);
            }
            CmcePduTypeDl::DCallRestore => {
                unimplemented_log!("{}", pdu_type);
            }
            CmcePduTypeDl::DConnect => self.rx_d_connect(queue, message),
            CmcePduTypeDl::DConnectAcknowledge => self.rx_d_connect_acknowledge(queue, message),
            CmcePduTypeDl::DDisconnect => self.rx_d_disconnect(queue, message),
            CmcePduTypeDl::DInfo => {
                unimplemented_log!("{}", pdu_type);
            }
            CmcePduTypeDl::DRelease => self.rx_d_release(queue, message),
            CmcePduTypeDl::DSetup => self.rx_d_setup(queue, message),
            CmcePduTypeDl::DTxCeased => {
                unimplemented_log!("{}", pdu_type);
            }
            CmcePduTypeDl::DTxContinue => {
                unimplemented_log!("{}", pdu_type);
            }
            CmcePduTypeDl::DTxGranted => self.rx_d_tx_granted(queue, message),
            CmcePduTypeDl::DTxInterrupt => {
                unimplemented_log!("{}", pdu_type);
            }
            CmcePduTypeDl::DTxWait => {
                unimplemented_log!("{}", pdu_type);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }
}
