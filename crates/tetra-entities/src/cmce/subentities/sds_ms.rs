// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use tetra_core::{Sap, SsiType, tetra_entities::TetraEntity};
use tetra_pdus::cmce::{
    enums::{cmce_pdu_type_dl::CmcePduTypeDl, party_type_identifier::PartyTypeIdentifier},
    pdus::{d_sds_data::DSdsData, d_status::DStatus},
};
use tetra_saps::control::{
    enums::sds_user_data::SdsUserData,
    sds::{CmceSdsData, CmceSdsStatus},
};
use tetra_saps::{SapMsg, SapMsgInner};

use crate::MessageQueue;

/// Clause 13 Short Data Service CMCE sub-entity
pub struct SdsMsSubentity {}

const MAX_AIR_INTERFACE_SSI: u32 = 0x00FF_FFFF;

impl SdsMsSubentity {
    /// Create a new instance of the SdsSubentity
    pub fn new() -> Self {
        SdsMsSubentity {}
    }

    fn valid_air_interface_ssi(ssi: u32) -> bool {
        ssi <= MAX_AIR_INTERFACE_SSI
    }

    fn calling_party_ssi(
        cpti: PartyTypeIdentifier,
        calling_party_address_ssi: Option<u64>,
        calling_party_extension: Option<u64>,
    ) -> Option<u32> {
        match cpti {
            PartyTypeIdentifier::Ssi => {
                let Some(ssi) = calling_party_address_ssi else {
                    tracing::warn!("SDS-MS: D-SDS/D-STATUS missing calling_party_address_ssi");
                    return None;
                };
                if ssi <= MAX_AIR_INTERFACE_SSI as u64 {
                    Some(ssi as u32)
                } else {
                    tracing::warn!("SDS-MS: D-SDS/D-STATUS invalid 24-bit source SSI {}", ssi);
                    None
                }
            }
            PartyTypeIdentifier::Tsi => {
                // EN 300 392-2 tables 14.13/14.14 carry Calling Party
                // Extension when CPTI is TSI, and clauses 13.3.2.1/13.3.2.3
                // preserve that extension in TNSDS STATUS/UNITDATA
                // indications. CmceSdsData currently carries only source_issi,
                // so do not collapse TSI to SSI and deliver a misleading source
                // identity.
                tracing::warn!(
                    "SDS-MS: unsupported TSI calling party extension {:?}; dropping to avoid SSI rewrite",
                    calling_party_extension
                );
                None
            }
            PartyTypeIdentifier::Sna | PartyTypeIdentifier::Reserved => {
                tracing::warn!("SDS-MS: unsupported calling_party_type_identifier {:?}", cpti);
                None
            }
        }
    }

    fn deliver_to_user(queue: &mut MessageQueue, source_issi: u32, dest_issi: u32, dest_ssi_type: SsiType, user_defined_data: SdsUserData) {
        if !Self::valid_air_interface_ssi(source_issi) || !Self::valid_air_interface_ssi(dest_issi) {
            tracing::warn!(
                "SDS-MS: dropping downlink SDS with invalid 24-bit SSI source={} dest={}",
                source_issi,
                dest_issi
            );
            return;
        }

        queue.push_back(SapMsg {
            sap: Sap::TnsdsSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::User,
            msg: SapMsgInner::CmceSdsData(CmceSdsData {
                source_issi,
                dest_issi,
                dest_ssi_type: Some(dest_ssi_type),
                user_defined_data,
                tx_reporter: None,
            }),
        });
    }

    fn deliver_status_to_user(queue: &mut MessageQueue, source_issi: u32, dest_issi: u32, dest_ssi_type: SsiType, status_number: u16) {
        if !Self::valid_air_interface_ssi(source_issi) || !Self::valid_air_interface_ssi(dest_issi) {
            tracing::warn!(
                "SDS-MS: dropping downlink status with invalid 24-bit SSI source={} dest={}",
                source_issi,
                dest_issi
            );
            return;
        }

        queue.push_back(SapMsg {
            sap: Sap::TnsdsSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::User,
            msg: SapMsgInner::CmceSdsStatus(CmceSdsStatus {
                source_issi,
                dest_issi,
                dest_ssi_type,
                status_number,
                tx_reporter: None,
            }),
        });
    }

    pub fn rx_sds_data(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_sds_data");

        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let dest_issi = prim.received_tetra_address.ssi;
        let dest_ssi_type = prim.received_tetra_address.ssi_type;
        let pdu = match DSdsData::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("Received DSdsData: {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing DSdsData: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        let Some(source_issi) = Self::calling_party_ssi(
            pdu.calling_party_type_identifier,
            pdu.calling_party_address_ssi,
            pdu.calling_party_extension,
        ) else {
            return;
        };

        // EN 300 392-2 clauses 13.2 and 14.7.1.10: D-SDS-DATA carries
        // individual or group user-defined SDS downlink to the MS. Preserve
        // the received address type at the local TNSDS boundary.
        Self::deliver_to_user(queue, source_issi, dest_issi, dest_ssi_type, pdu.user_defined_data);
    }

    pub fn rx_status(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_status");

        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let dest_issi = prim.received_tetra_address.ssi;
        let dest_ssi_type = prim.received_tetra_address.ssi_type;
        let pdu = match DStatus::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("Received DStatus: {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing DStatus: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        let Some(source_issi) = Self::calling_party_ssi(
            pdu.calling_party_type_identifier,
            pdu.calling_party_address_ssi,
            pdu.calling_party_extension,
        ) else {
            return;
        };

        // EN 300 392-2 clauses 13.3.2.1 and 14.7.1.11: D-STATUS carries a
        // pre-coded status, so expose it as TNSDS-STATUS rather than
        // user-defined TNSDS-UNITDATA.
        Self::deliver_status_to_user(queue, source_issi, dest_issi, dest_ssi_type, pdu.pre_coded_status.into_raw());
    }

    /// Poor man's rx_prim, as this is a subcomponent and not governed by the MessageRouter
    /// If need be, we can deviate from the standard's subentity ranking and make this a full-fledged component
    /// See Figure 14.2: Block view of CMCE-MS
    pub fn route_rf_deliver(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("route_rf_deliver");

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
            CmcePduTypeDl::DSdsData => {
                self.rx_sds_data(queue, message);
            }
            CmcePduTypeDl::DStatus => {
                self.rx_status(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }
}
