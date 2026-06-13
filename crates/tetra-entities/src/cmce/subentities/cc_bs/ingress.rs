// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use super::*;

impl CcBsSubentity {
    pub fn route_xx_deliver(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("route_xx_deliver");

        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("route_xx_deliver: expected LcmcMleUnitdataInd, got unexpected SAP message type");
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
            CmcePduTypeUl::USetup => self.rx_u_setup(queue, message),
            CmcePduTypeUl::UTxCeased => self.rx_u_tx_ceased(queue, message),
            CmcePduTypeUl::UTxDemand => self.rx_u_tx_demand(queue, message),
            CmcePduTypeUl::URelease => self.rx_u_release(queue, message),
            CmcePduTypeUl::UDisconnect => self.rx_u_disconnect(queue, message),
            CmcePduTypeUl::UAlert => self.rx_u_alert(queue, message),
            CmcePduTypeUl::UConnect => self.rx_u_connect(queue, message),
            CmcePduTypeUl::UInfo => self.rx_u_info(queue, message),
            CmcePduTypeUl::UStatus => {
                unimplemented_log!("{}", pdu_type);
            }
            CmcePduTypeUl::UCallRestore => {
                tracing::debug!(
                    "CMCE: received unsupported U-CALL RESTORE from ISSI {}; responding CMCE FUNCTION NOT SUPPORTED",
                    prim.received_tetra_address.ssi
                );
                queue.push_back(Self::build_cmce_function_not_supported_direct(
                    pdu_type,
                    prim.received_tetra_address,
                    prim.handle,
                    prim.link_id,
                    prim.endpoint_id,
                ));
            }
            _ => {
                tracing::warn!("route_xx_deliver: unhandled PDU type {:?}, ignoring", pdu_type);
            }
        }
    }

    pub fn rx_call_control(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        let SapMsgInner::CmceCallControl(call_control) = message.msg else {
            tracing::error!("rx_call_control: expected CmceCallControl, got unexpected SAP message type");
            return;
        };

        match call_control {
            CallControl::NetworkCallStart {
                brew_uuid,
                source_issi,
                dest_gssi,
                priority,
            } => {
                self.rx_network_call_start(queue, brew_uuid, source_issi, dest_gssi, priority);
            }
            CallControl::NetworkCallEnd { brew_uuid } => {
                self.rx_network_call_end(queue, brew_uuid);
            }
            CallControl::NetworkCircuitSetupRequest { brew_uuid, call } => {
                self.rx_network_circuit_setup_request(queue, brew_uuid, call);
            }
            CallControl::NetworkCircuitSetupAccept { brew_uuid } => {
                self.rx_network_circuit_setup_accept(brew_uuid);
            }
            CallControl::NetworkCircuitSetupReject { brew_uuid, cause } => {
                self.rx_network_circuit_setup_reject(queue, brew_uuid, cause);
            }
            CallControl::NetworkCircuitAlert { brew_uuid } => {
                self.rx_network_circuit_alert(queue, brew_uuid);
            }
            CallControl::NetworkCircuitConnectRequest { brew_uuid, call } => {
                self.rx_network_circuit_connect_request(queue, brew_uuid, call);
            }
            CallControl::NetworkCircuitConnectConfirm {
                brew_uuid,
                grant,
                permission,
            } => {
                self.rx_network_circuit_connect_confirm(queue, brew_uuid, grant, permission);
            }
            CallControl::NetworkCircuitSimplexGranted {
                brew_uuid,
                grant,
                permission,
            } => {
                self.rx_network_circuit_simplex_granted(queue, brew_uuid, grant, permission);
            }
            CallControl::NetworkCircuitSimplexIdle {
                brew_uuid,
                grant,
                permission,
            } => {
                self.rx_network_circuit_simplex_idle(queue, brew_uuid, grant, permission);
            }
            CallControl::NetworkCircuitMediaReady { brew_uuid, .. } => {
                tracing::trace!("CMCE: ignoring unexpected NetworkCircuitMediaReady uuid={}", brew_uuid);
            }
            CallControl::NetworkCircuitRelease { brew_uuid, cause } => {
                self.rx_network_circuit_release(queue, brew_uuid, cause);
            }
            CallControl::UlInactivityTimeout { ts } => {
                self.handle_ul_inactivity_timeout(queue, ts);
            }
            _ => {
                tracing::warn!("Unexpected CallControl message: {:?}", call_control);
            }
        }
    }

    pub(super) fn rx_u_setup(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_u_setup: {:?}", message);
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("rx_u_setup: expected LcmcMleUnitdataInd, got unexpected SAP message type");
            return;
        };
        let calling_party = prim.received_tetra_address;

        let pdu = match USetup::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-SETUP {:?}", pdu);
                tracing::info!(
                    "CMCE: <- U-SETUP from ISSI {} called_party={:?} comm_type={:?} simplex={} hook={} priority={}",
                    calling_party.ssi,
                    pdu.called_party_ssi,
                    pdu.basic_service_information.communication_type,
                    !pdu.simplex_duplex_selection,
                    pdu.hook_method_selection,
                    pdu.call_priority
                );
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-SETUP: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        self.fsm_on_u_setup(queue, &message, &pdu, calling_party);
    }

    pub(super) fn rx_u_tx_ceased(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("rx_u_tx_ceased: expected LcmcMleUnitdataInd, got unexpected SAP message type");
            return;
        };

        let sender = prim.received_tetra_address;
        let pdu = match UTxCeased::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-TX CEASED {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-TX CEASED: {:?}", e);
                return;
            }
        };

        self.fsm_on_u_tx_ceased(queue, sender, pdu);
    }

    pub(super) fn rx_u_tx_demand(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("rx_u_tx_demand: expected LcmcMleUnitdataInd, got unexpected SAP message type");
            return;
        };

        let requesting_party = prim.received_tetra_address;
        let ul_handle = prim.handle;
        let ul_link_id = prim.link_id;
        let ul_endpoint_id = prim.endpoint_id;
        let received_pdu = prim.sdu.clone();
        let pdu = match UTxDemand::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-TX DEMAND {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-TX DEMAND: {:?}", e);
                return;
            }
        };

        if let Some((pointer, reason)) = Self::unsupported_u_tx_demand_function(&pdu) {
            tracing::info!(
                "CMCE: rejecting unsupported U-TX DEMAND call_id={} from ISSI {}: {}; responding CMCE FUNCTION NOT SUPPORTED",
                pdu.call_identifier,
                requesting_party.ssi,
                reason
            );
            queue.push_back(Self::build_cmce_function_not_supported_element_direct(
                CmcePduTypeUl::UTxDemand,
                pdu.call_identifier,
                pointer,
                &received_pdu,
                requesting_party,
                ul_handle,
                ul_link_id,
                ul_endpoint_id,
            ));
            return;
        }

        self.fsm_on_u_tx_demand(queue, requesting_party, ul_handle, ul_link_id, ul_endpoint_id, pdu);
    }

    pub(super) fn rx_u_release(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("rx_u_release: expected LcmcMleUnitdataInd, got unexpected SAP message type");
            return;
        };

        let sender = prim.received_tetra_address;
        let ul_handle = prim.handle;
        let ul_link_id = prim.link_id;
        let ul_endpoint_id = prim.endpoint_id;
        let received_pdu = prim.sdu.clone();
        let pdu = match URelease::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-RELEASE {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-RELEASE: {:?}", e);
                return;
            }
        };

        if let Some((pointer, reason)) = Self::unsupported_u_release_function(&pdu) {
            tracing::info!(
                "CMCE: rejecting unsupported U-RELEASE call_id={} from ISSI {}: {}; responding CMCE FUNCTION NOT SUPPORTED",
                pdu.call_identifier,
                sender.ssi,
                reason
            );
            queue.push_back(Self::build_cmce_function_not_supported_element_direct(
                CmcePduTypeUl::URelease,
                pdu.call_identifier,
                pointer,
                &received_pdu,
                sender,
                ul_handle,
                ul_link_id,
                ul_endpoint_id,
            ));
            return;
        }

        self.fsm_on_u_release(queue, sender, ul_handle, ul_link_id, ul_endpoint_id, pdu);
    }

    pub(super) fn rx_u_disconnect(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("rx_u_disconnect: expected LcmcMleUnitdataInd, got unexpected SAP message type");
            return;
        };

        let sender = prim.received_tetra_address;
        let ul_handle = prim.handle;
        let ul_link_id = prim.link_id;
        let ul_endpoint_id = prim.endpoint_id;

        let received_pdu = prim.sdu.clone();
        let pdu = match UDisconnect::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-DISCONNECT {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-DISCONNECT: {:?}", e);
                return;
            }
        };

        if let Some((pointer, reason)) = Self::unsupported_u_disconnect_function(&pdu) {
            tracing::info!(
                "CMCE: rejecting unsupported U-DISCONNECT call_id={} from ISSI {}: {}; responding CMCE FUNCTION NOT SUPPORTED",
                pdu.call_identifier,
                sender.ssi,
                reason
            );
            queue.push_back(Self::build_cmce_function_not_supported_element_direct(
                CmcePduTypeUl::UDisconnect,
                pdu.call_identifier,
                pointer,
                &received_pdu,
                sender,
                ul_handle,
                ul_link_id,
                ul_endpoint_id,
            ));
            return;
        }

        self.fsm_on_u_disconnect(queue, sender, ul_handle, ul_link_id, ul_endpoint_id, pdu);
    }

    pub(super) fn rx_u_alert(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("rx_u_alert: expected LcmcMleUnitdataInd, got unexpected SAP message type");
            return;
        };

        let received_pdu = prim.sdu.clone();
        let pdu = match UAlert::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-ALERT {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-ALERT: {:?}", e);
                return;
            }
        };

        let requested_basic_service = self
            .cached_setups
            .get(&pdu.call_identifier)
            .map(|cached| &cached.pdu.basic_service_information);
        if let Some((pointer, reason)) = Self::unsupported_u_alert_function(&pdu, requested_basic_service) {
            tracing::info!(
                "CMCE: rejecting unsupported U-ALERT call_id={} from ISSI {}: {}; responding CMCE FUNCTION NOT SUPPORTED",
                pdu.call_identifier,
                prim.received_tetra_address.ssi,
                reason
            );
            queue.push_back(Self::build_cmce_function_not_supported_element_direct(
                CmcePduTypeUl::UAlert,
                pdu.call_identifier,
                pointer,
                &received_pdu,
                prim.received_tetra_address,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
            ));
            return;
        }

        self.fsm_on_u_alert(queue, prim.received_tetra_address, prim.handle, prim.link_id, prim.endpoint_id, pdu);
    }

    /// Handle U-CONNECT for an individual call.
    pub(super) fn rx_u_connect(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("rx_u_connect: expected LcmcMleUnitdataInd, got unexpected SAP message type");
            return;
        };

        let received_pdu = prim.sdu.clone();
        let pdu = match UConnect::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-CONNECT {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-CONNECT: {:?}", e);
                return;
            }
        };

        self.fsm_on_u_connect(
            queue,
            prim.received_tetra_address,
            prim.handle,
            prim.link_id,
            prim.endpoint_id,
            pdu,
            received_pdu,
        );
    }

    pub(super) fn rx_u_info(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("rx_u_info: expected LcmcMleUnitdataInd, got unexpected SAP message type");
            return;
        };

        let received_pdu = prim.sdu.clone();
        let pdu = match UInfo::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- U-INFO {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-INFO: {:?}", e);
                return;
            }
        };

        if let Some((pointer, reason)) = Self::unsupported_u_info_function(&pdu) {
            tracing::info!(
                "CMCE: rejecting unsupported U-INFO call_id={} from ISSI {}: {}; responding CMCE FUNCTION NOT SUPPORTED",
                pdu.call_identifier,
                prim.received_tetra_address.ssi,
                reason
            );
            queue.push_back(Self::build_cmce_function_not_supported_element_direct(
                CmcePduTypeUl::UInfo,
                pdu.call_identifier,
                pointer,
                &received_pdu,
                prim.received_tetra_address,
                prim.handle,
                prim.link_id,
                prim.endpoint_id,
            ));
            return;
        }

        self.fsm_on_u_info(queue, pdu);
    }
}
