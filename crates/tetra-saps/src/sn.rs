// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original TETRA SN-SAP primitive model for SNDCP/IP/WAP work.

use tetra_core::{BitBuffer, MleHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnPrimitiveError {
    ReservedNsapi(u8),
    PduPriorityOutOfRange(u8),
    DataPriorityOutOfRange(u8),
    EmptyNPdu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnPdpType {
    Ipv4,
    Ipv6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnAddress {
    DynamicIpv4,
    Ipv4([u8; 4]),
    Ipv6([u8; 16]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnPacketDataMsType {
    TypeAParallel,
    TypeBAlternating,
    TypeCIpSingleMode,
    TypeDRestrictedIpSingleMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnDataImportance {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnScheduleSurplusFlag {
    NotSurplusToSchedule,
    SurplusToSchedule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnActivationResult {
    Accepted,
    Rejected(SnRejectCause),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnRejectCause {
    UnsupportedPdpType,
    UnsupportedAddressType,
    UnsupportedNsapi,
    UnsupportedPacketDataMsType,
    SndcpServiceTemporarilyNotAvailable,
    Other(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnDeliveryStatus {
    /// Valid for acknowledged `SN-DATA`; `SN-UNITDATA` success is completed
    /// silently and does not emit `SN-DELIVERY` per TS 100 392-2 table 28.2.
    Success,
    Failure,
    DeletedOrCancelledBySndcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnNsapiDeactivation {
    AllNsapis,
    Nsapi(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnNsapiAllocReq {
    pub nsapi: u8,
    pub pdp_type: SnPdpType,
    pub requested_address: SnAddress,
    pub packet_data_ms_type: SnPacketDataMsType,
    pub pdu_priority: Option<u8>,
    pub data_priority: Option<u8>,
    pub max_npdu_len: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnNsapiAllocCnf {
    pub nsapi: u8,
    pub result: SnActivationResult,
    pub assigned_address: Option<SnAddress>,
    pub negotiated_max_npdu_len: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnNsapiDeallocReq {
    pub deactivation: SnNsapiDeactivation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnNsapiDeallocInd {
    pub deactivation: SnNsapiDeactivation,
}

#[derive(Debug, Clone)]
pub struct SnUnitdataReq {
    pub nsapi: u8,
    pub handle: MleHandle,
    pub n_pdu: BitBuffer,
    pub pdu_priority: Option<u8>,
    pub data_priority: Option<u8>,
    pub data_importance: SnDataImportance,
    pub schedule_surplus_flag: SnScheduleSurplusFlag,
}

#[derive(Debug, Clone)]
pub struct SnUnitdataInd {
    pub nsapi: u8,
    pub n_pdu: BitBuffer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnDeliveryInd {
    pub handle: MleHandle,
    pub status: SnDeliveryStatus,
}

pub fn validate_nsapi(nsapi: u8) -> Result<u8, SnPrimitiveError> {
    // EN 300 392-2 clause 28.3.3.2 reserves NSAPI 0 and 15. Values 1..14
    // identify dynamically allocated PDP contexts.
    if (1..=14).contains(&nsapi) {
        Ok(nsapi)
    } else {
        Err(SnPrimitiveError::ReservedNsapi(nsapi))
    }
}

pub fn validate_pdu_priority(pdu_priority: u8) -> Result<u8, SnPrimitiveError> {
    // EN 300 392-2 clause 28.4.1/table 28.22 and clause 28.4.5.30 define
    // eight SNDCP PDU priority levels, 0..7.
    if pdu_priority <= 7 {
        Ok(pdu_priority)
    } else {
        Err(SnPrimitiveError::PduPriorityOutOfRange(pdu_priority))
    }
}

pub fn validate_data_priority(data_priority: u8) -> Result<u8, SnPrimitiveError> {
    // EN 300 392-2 clause 28.2.3.2 defines data priority 0..7.
    if data_priority <= 7 {
        Ok(data_priority)
    } else {
        Err(SnPrimitiveError::DataPriorityOutOfRange(data_priority))
    }
}

pub fn validate_nsapi_deactivation(deactivation: SnNsapiDeactivation) -> Result<SnNsapiDeactivation, SnPrimitiveError> {
    if let SnNsapiDeactivation::Nsapi(nsapi) = deactivation {
        validate_nsapi(nsapi)?;
    }
    Ok(deactivation)
}

pub fn sn_unitdata_req(
    nsapi: u8,
    handle: MleHandle,
    n_pdu: BitBuffer,
    pdu_priority: Option<u8>,
    data_priority: Option<u8>,
) -> Result<SnUnitdataReq, SnPrimitiveError> {
    validate_nsapi(nsapi)?;
    if let Some(pdu_priority) = pdu_priority {
        validate_pdu_priority(pdu_priority)?;
    }
    if let Some(data_priority) = data_priority {
        validate_data_priority(data_priority)?;
    }
    if n_pdu.get_len() == 0 {
        return Err(SnPrimitiveError::EmptyNPdu);
    }

    Ok(SnUnitdataReq {
        nsapi,
        handle,
        n_pdu,
        pdu_priority,
        data_priority,
        data_importance: SnDataImportance::Low,
        schedule_surplus_flag: SnScheduleSurplusFlag::NotSurplusToSchedule,
    })
}

pub fn sn_unitdata_ind(nsapi: u8, n_pdu: BitBuffer) -> Result<SnUnitdataInd, SnPrimitiveError> {
    validate_nsapi(nsapi)?;
    if n_pdu.get_len() == 0 {
        return Err(SnPrimitiveError::EmptyNPdu);
    }
    Ok(SnUnitdataInd { nsapi, n_pdu })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_nsapi_accepts_dynamic_range_only() {
        assert_eq!(validate_nsapi(1), Ok(1));
        assert_eq!(validate_nsapi(14), Ok(14));
        assert_eq!(validate_nsapi(0), Err(SnPrimitiveError::ReservedNsapi(0)));
        assert_eq!(validate_nsapi(15), Err(SnPrimitiveError::ReservedNsapi(15)));
    }

    #[test]
    fn validate_priorities_accept_only_three_bit_sndcp_range() {
        assert_eq!(validate_pdu_priority(0), Ok(0));
        assert_eq!(validate_pdu_priority(7), Ok(7));
        assert_eq!(validate_pdu_priority(8), Err(SnPrimitiveError::PduPriorityOutOfRange(8)));

        assert_eq!(validate_data_priority(0), Ok(0));
        assert_eq!(validate_data_priority(7), Ok(7));
        assert_eq!(validate_data_priority(8), Err(SnPrimitiveError::DataPriorityOutOfRange(8)));
    }

    #[test]
    fn sn_unitdata_request_tracks_table_28_8_core_fields() {
        let n_pdu = BitBuffer::from_bytes(&[0x45, 0x00, 0x00, 0x14]);

        let req = sn_unitdata_req(3, 73, n_pdu.clone(), Some(2), Some(1)).expect("SN-UNITDATA request should accept valid NSAPI and N-PDU");

        assert_eq!(req.nsapi, 3);
        assert_eq!(req.handle, 73);
        assert_eq!(req.pdu_priority, Some(2));
        assert_eq!(req.data_priority, Some(1));
        assert_eq!(req.data_importance, SnDataImportance::Low);
        assert_eq!(req.schedule_surplus_flag, SnScheduleSurplusFlag::NotSurplusToSchedule);
        assert_eq!(req.n_pdu.to_bitstr(), n_pdu.to_bitstr());
    }

    #[test]
    fn sn_unitdata_rejects_reserved_nsapi_and_empty_npdu() {
        let n_pdu = BitBuffer::from_bytes(&[0x45]);

        assert!(matches!(
            sn_unitdata_req(0, 1, n_pdu.clone(), None, None),
            Err(SnPrimitiveError::ReservedNsapi(0))
        ));
        assert!(matches!(sn_unitdata_ind(15, n_pdu), Err(SnPrimitiveError::ReservedNsapi(15))));
        assert_eq!(
            sn_unitdata_req(1, 1, BitBuffer::new(0), None, None).map(|_| ()),
            Err(SnPrimitiveError::EmptyNPdu)
        );
    }

    #[test]
    fn sn_unitdata_request_rejects_out_of_range_priorities() {
        let n_pdu = BitBuffer::from_bytes(&[0x45]);

        assert_eq!(
            sn_unitdata_req(1, 1, n_pdu.clone(), Some(8), None).map(|_| ()),
            Err(SnPrimitiveError::PduPriorityOutOfRange(8))
        );
        assert_eq!(
            sn_unitdata_req(1, 1, n_pdu, None, Some(8)).map(|_| ()),
            Err(SnPrimitiveError::DataPriorityOutOfRange(8))
        );
    }

    #[test]
    fn nsapi_alloc_confirm_can_carry_dynamic_ipv4_assignment() {
        let cnf = SnNsapiAllocCnf {
            nsapi: 2,
            result: SnActivationResult::Accepted,
            assigned_address: Some(SnAddress::Ipv4([10, 0, 0, 226])),
            negotiated_max_npdu_len: Some(576),
        };

        assert_eq!(cnf.nsapi, 2);
        assert_eq!(cnf.result, SnActivationResult::Accepted);
        assert_eq!(cnf.assigned_address, Some(SnAddress::Ipv4([10, 0, 0, 226])));
        assert_eq!(cnf.negotiated_max_npdu_len, Some(576));
    }

    #[test]
    fn nsapi_deallocation_models_table_28_5_conditional_nsapi() {
        let all = SnNsapiDeallocReq {
            deactivation: SnNsapiDeactivation::AllNsapis,
        };
        let one = SnNsapiDeallocInd {
            deactivation: SnNsapiDeactivation::Nsapi(2),
        };

        assert_eq!(validate_nsapi_deactivation(all.deactivation), Ok(SnNsapiDeactivation::AllNsapis));
        assert_eq!(validate_nsapi_deactivation(one.deactivation), Ok(SnNsapiDeactivation::Nsapi(2)));
        assert_eq!(
            validate_nsapi_deactivation(SnNsapiDeactivation::Nsapi(15)),
            Err(SnPrimitiveError::ReservedNsapi(15))
        );
    }
}
