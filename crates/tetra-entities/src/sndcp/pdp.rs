// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original TETRA SNDCP PDP context PDU primitives.

use tetra_core::BitBuffer;
use tetra_saps::sn::{SnAddress, SnPacketDataMsType, validate_nsapi};

const SN_PDU_TYPE_ACTIVATE_PDP_CONTEXT: u8 = 0;
const SN_PDU_TYPE_DEACTIVATE_PDP_CONTEXT_ACCEPT: u8 = 1;
const SN_PDU_TYPE_DEACTIVATE_PDP_CONTEXT_DEMAND: u8 = 2;
const SN_PDU_TYPE_ACTIVATE_PDP_CONTEXT_REJECT: u8 = 3;

const SNDCP_VERSION_1: u8 = 1;
const PCOMP_NEGOTIATION_NONE: u8 = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SndcpPdpError {
    TooShort(&'static str),
    TrailingBits { bits: usize },
    UnsupportedOptionalElements,
    UnexpectedPduType { expected: u8, actual: u8 },
    ReservedSndcpVersion(u8),
    ReservedNsapi(u8),
    ReservedAddressTypeIdentifierInDemand(u8),
    ReservedTypeIdentifierInAccept(u8),
    ReservedPacketDataMsType(u8),
    ReservedReadyTimer(u8),
    ReservedStandbyTimer(u8),
    ReservedResponseWaitTimer(u8),
    ReservedPduPriorityMax(u8),
    ReservedMaximumTransmissionUnit(u8),
    ReservedDeactivationType(u8),
    UnsupportedPcompNegotiation(u8),
    MissingStaticIpv4Address,
    MissingPrimaryNsapi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpAddressTypeIdentifierInDemand {
    Ipv4StaticAddress,
    Ipv4DynamicAddressNegotiation,
    Ipv6,
    MobileIpv4ForeignAgentCareOfAddressRequested,
    MobileIpv4CoLocatedCareOfAddressRequested,
    PrimaryNsapiSecondaryPdpContextRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpTypeIdentifierInAccept {
    NoAddress,
    Ipv4StaticAddress,
    Ipv4DynamicAddress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpMaximumTransmissionUnit {
    Octets296,
    Octets576,
    Octets1006,
    Octets1500,
    Octets2002,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SndcpActivateAddressDemand {
    Ipv4Static([u8; 4]),
    Ipv4Dynamic,
    Ipv6,
    MobileIpv4ForeignAgentCareOfAddress,
    MobileIpv4CoLocatedCareOfAddress,
    SecondaryPdpContext { primary_nsapi: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SndcpActivatePdpContextDemand {
    pub sndcp_version: u8,
    pub nsapi: u8,
    pub address: SndcpActivateAddressDemand,
    pub packet_data_ms_type: SnPacketDataMsType,
    pub pcomp_negotiation: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SndcpActivatePdpContextAccept {
    pub nsapi: u8,
    pub pdu_priority_max: u8,
    pub ready_timer: u8,
    pub standby_timer: u8,
    pub response_wait_timer: u8,
    pub type_identifier: SndcpTypeIdentifierInAccept,
    pub assigned_address: Option<SnAddress>,
    pub pcomp_negotiation: u8,
    pub maximum_transmission_unit: SndcpMaximumTransmissionUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpActivationRejectCause {
    Undefined,
    MsNotProvisionedForPacketData,
    Ipv4NotSupported,
    Ipv6NotSupported,
    Ipv4DynamicAddressNegotiationNotSupported,
    DynamicAddressPoolEmpty,
    StaticAddressNotCorrect,
    StaticAddressInUse,
    StaticAddressNotAllowed,
    PacketDataMsTypeNotSupported,
    SndcpVersionNotSupported,
    MaximumNumberOfPdpContextsPerItsiExceeded,
    SecondaryPdpContextsNotSupported,
    PrimaryPdpContextDoesNotExist,
    SndcpServiceTemporarilyNotAvailable,
    Other(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SndcpActivatePdpContextReject {
    pub nsapi: u8,
    pub cause: SndcpActivationRejectCause,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SndcpDeactivation {
    AllNsapis,
    Nsapi(u8),
}

pub fn encode_activate_pdp_context_demand(demand: &SndcpActivatePdpContextDemand) -> Result<BitBuffer, SndcpPdpError> {
    validate_sndcp_version(demand.sndcp_version)?;
    validate_nsapi_for_pdp(demand.nsapi)?;
    validate_pcomp_negotiation(demand.pcomp_negotiation)?;

    let atid = address_demand_code(&demand.address);
    let conditional_len = match demand.address {
        SndcpActivateAddressDemand::Ipv4Static(_) => 32,
        SndcpActivateAddressDemand::SecondaryPdpContext { primary_nsapi } => {
            validate_nsapi_for_pdp(primary_nsapi)?;
            4
        }
        _ => 0,
    };
    let mut pdu = BitBuffer::new(4 + 4 + 4 + 3 + conditional_len + 4 + 8 + 1);
    pdu.write_bits(SN_PDU_TYPE_ACTIVATE_PDP_CONTEXT as u64, 4);
    pdu.write_bits(demand.sndcp_version as u64, 4);
    pdu.write_bits(demand.nsapi as u64, 4);
    pdu.write_bits(atid as u64, 3);
    match demand.address {
        SndcpActivateAddressDemand::Ipv4Static(address) => write_ipv4(&mut pdu, address),
        SndcpActivateAddressDemand::SecondaryPdpContext { primary_nsapi } => {
            pdu.write_bits(primary_nsapi as u64, 4);
        }
        _ => {}
    }
    pdu.write_bits(packet_data_ms_type_code(demand.packet_data_ms_type) as u64, 4);
    pdu.write_bits(demand.pcomp_negotiation as u64, 8);
    write_no_optional_elements(&mut pdu);
    pdu.seek(0);
    Ok(pdu)
}

pub fn decode_activate_pdp_context_demand(pdu: &BitBuffer) -> Result<SndcpActivatePdpContextDemand, SndcpPdpError> {
    let mut pdu = reader(pdu);
    expect_pdu_type(&mut pdu, SN_PDU_TYPE_ACTIVATE_PDP_CONTEXT)?;
    let sndcp_version = read_u8(&mut pdu, 4, "sndcp_version")?;
    validate_sndcp_version(sndcp_version)?;
    let nsapi = read_u8(&mut pdu, 4, "nsapi")?;
    validate_nsapi_for_pdp(nsapi)?;
    let atid = read_u8(&mut pdu, 3, "address_type_identifier_in_demand")?;
    let address_type = address_type_identifier_in_demand(atid)?;
    let address = match address_type {
        SndcpAddressTypeIdentifierInDemand::Ipv4StaticAddress => SndcpActivateAddressDemand::Ipv4Static(read_ipv4(&mut pdu)?),
        SndcpAddressTypeIdentifierInDemand::Ipv4DynamicAddressNegotiation => SndcpActivateAddressDemand::Ipv4Dynamic,
        SndcpAddressTypeIdentifierInDemand::Ipv6 => SndcpActivateAddressDemand::Ipv6,
        SndcpAddressTypeIdentifierInDemand::MobileIpv4ForeignAgentCareOfAddressRequested => {
            SndcpActivateAddressDemand::MobileIpv4ForeignAgentCareOfAddress
        }
        SndcpAddressTypeIdentifierInDemand::MobileIpv4CoLocatedCareOfAddressRequested => {
            SndcpActivateAddressDemand::MobileIpv4CoLocatedCareOfAddress
        }
        SndcpAddressTypeIdentifierInDemand::PrimaryNsapiSecondaryPdpContextRequested => {
            let primary_nsapi = read_u8(&mut pdu, 4, "primary_nsapi")?;
            validate_nsapi_for_pdp(primary_nsapi)?;
            SndcpActivateAddressDemand::SecondaryPdpContext { primary_nsapi }
        }
    };
    let packet_data_ms_type = packet_data_ms_type(read_u8(&mut pdu, 4, "packet_data_ms_type")?)?;
    let pcomp_negotiation = read_u8(&mut pdu, 8, "pcomp_negotiation")?;
    validate_pcomp_negotiation(pcomp_negotiation)?;
    read_no_optional_tail(&mut pdu)?;

    Ok(SndcpActivatePdpContextDemand {
        sndcp_version,
        nsapi,
        address,
        packet_data_ms_type,
        pcomp_negotiation,
    })
}

pub fn encode_activate_pdp_context_accept(accept: &SndcpActivatePdpContextAccept) -> Result<BitBuffer, SndcpPdpError> {
    validate_nsapi_for_pdp(accept.nsapi)?;
    validate_pdu_priority_max(accept.pdu_priority_max)?;
    validate_ready_timer(accept.ready_timer)?;
    validate_standby_timer(accept.standby_timer)?;
    validate_response_wait_timer(accept.response_wait_timer)?;
    validate_pcomp_negotiation(accept.pcomp_negotiation)?;
    let mtu_code = maximum_transmission_unit_code(accept.maximum_transmission_unit);
    let tia = type_identifier_in_accept_code(accept.type_identifier);
    let conditional_len = match accept.type_identifier {
        SndcpTypeIdentifierInAccept::NoAddress => 0,
        SndcpTypeIdentifierInAccept::Ipv4StaticAddress | SndcpTypeIdentifierInAccept::Ipv4DynamicAddress => 32,
    };
    let mut pdu = BitBuffer::new(4 + 4 + 3 + 4 + 4 + 4 + 3 + conditional_len + 8 + 3 + 1);
    pdu.write_bits(SN_PDU_TYPE_ACTIVATE_PDP_CONTEXT as u64, 4);
    pdu.write_bits(accept.nsapi as u64, 4);
    pdu.write_bits(accept.pdu_priority_max as u64, 3);
    pdu.write_bits(accept.ready_timer as u64, 4);
    pdu.write_bits(accept.standby_timer as u64, 4);
    pdu.write_bits(accept.response_wait_timer as u64, 4);
    pdu.write_bits(tia as u64, 3);
    if conditional_len == 32 {
        let Some(SnAddress::Ipv4(address)) = accept.assigned_address else {
            return Err(SndcpPdpError::MissingStaticIpv4Address);
        };
        write_ipv4(&mut pdu, address);
    }
    pdu.write_bits(accept.pcomp_negotiation as u64, 8);
    pdu.write_bits(mtu_code as u64, 3);
    write_no_optional_elements(&mut pdu);
    pdu.seek(0);
    Ok(pdu)
}

pub fn decode_activate_pdp_context_accept(pdu: &BitBuffer) -> Result<SndcpActivatePdpContextAccept, SndcpPdpError> {
    let mut pdu = reader(pdu);
    expect_pdu_type(&mut pdu, SN_PDU_TYPE_ACTIVATE_PDP_CONTEXT)?;
    let nsapi = read_u8(&mut pdu, 4, "nsapi")?;
    validate_nsapi_for_pdp(nsapi)?;
    let pdu_priority_max = read_u8(&mut pdu, 3, "pdu_priority_max")?;
    validate_pdu_priority_max(pdu_priority_max)?;
    let ready_timer = read_u8(&mut pdu, 4, "ready_timer")?;
    validate_ready_timer(ready_timer)?;
    let standby_timer = read_u8(&mut pdu, 4, "standby_timer")?;
    validate_standby_timer(standby_timer)?;
    let response_wait_timer = read_u8(&mut pdu, 4, "response_wait_timer")?;
    validate_response_wait_timer(response_wait_timer)?;
    let type_identifier = type_identifier_in_accept(read_u8(&mut pdu, 3, "type_identifier_in_accept")?)?;
    let assigned_address = match type_identifier {
        SndcpTypeIdentifierInAccept::NoAddress => None,
        SndcpTypeIdentifierInAccept::Ipv4StaticAddress | SndcpTypeIdentifierInAccept::Ipv4DynamicAddress => {
            Some(SnAddress::Ipv4(read_ipv4(&mut pdu)?))
        }
    };
    let pcomp_negotiation = read_u8(&mut pdu, 8, "pcomp_negotiation")?;
    validate_pcomp_negotiation(pcomp_negotiation)?;
    let maximum_transmission_unit = maximum_transmission_unit(read_u8(&mut pdu, 3, "maximum_transmission_unit")?)?;
    read_no_optional_tail(&mut pdu)?;

    Ok(SndcpActivatePdpContextAccept {
        nsapi,
        pdu_priority_max,
        ready_timer,
        standby_timer,
        response_wait_timer,
        type_identifier,
        assigned_address,
        pcomp_negotiation,
        maximum_transmission_unit,
    })
}

pub fn encode_activate_pdp_context_reject(reject: &SndcpActivatePdpContextReject) -> Result<BitBuffer, SndcpPdpError> {
    validate_nsapi_for_pdp(reject.nsapi)?;

    let mut pdu = BitBuffer::new(4 + 4 + 8 + 1);
    pdu.write_bits(SN_PDU_TYPE_ACTIVATE_PDP_CONTEXT_REJECT as u64, 4);
    pdu.write_bits(reject.nsapi as u64, 4);
    pdu.write_bits(activation_reject_cause_code(reject.cause) as u64, 8);
    write_no_optional_elements(&mut pdu);
    pdu.seek(0);
    Ok(pdu)
}

pub fn decode_activate_pdp_context_reject(pdu: &BitBuffer) -> Result<SndcpActivatePdpContextReject, SndcpPdpError> {
    let mut pdu = reader(pdu);
    expect_pdu_type(&mut pdu, SN_PDU_TYPE_ACTIVATE_PDP_CONTEXT_REJECT)?;
    let nsapi = read_u8(&mut pdu, 4, "nsapi")?;
    validate_nsapi_for_pdp(nsapi)?;
    let cause = activation_reject_cause(read_u8(&mut pdu, 8, "activation_reject_cause")?);
    read_no_optional_tail(&mut pdu)?;
    Ok(SndcpActivatePdpContextReject { nsapi, cause })
}

pub fn encode_deactivate_pdp_context_demand(deactivation: &SndcpDeactivation) -> Result<BitBuffer, SndcpPdpError> {
    encode_deactivate_pdp_context(SN_PDU_TYPE_DEACTIVATE_PDP_CONTEXT_DEMAND, deactivation)
}

pub fn decode_deactivate_pdp_context_demand(pdu: &BitBuffer) -> Result<SndcpDeactivation, SndcpPdpError> {
    decode_deactivate_pdp_context(pdu, SN_PDU_TYPE_DEACTIVATE_PDP_CONTEXT_DEMAND)
}

pub fn encode_deactivate_pdp_context_accept(deactivation: &SndcpDeactivation) -> Result<BitBuffer, SndcpPdpError> {
    encode_deactivate_pdp_context(SN_PDU_TYPE_DEACTIVATE_PDP_CONTEXT_ACCEPT, deactivation)
}

pub fn decode_deactivate_pdp_context_accept(pdu: &BitBuffer) -> Result<SndcpDeactivation, SndcpPdpError> {
    decode_deactivate_pdp_context(pdu, SN_PDU_TYPE_DEACTIVATE_PDP_CONTEXT_ACCEPT)
}

fn encode_deactivate_pdp_context(pdu_type: u8, deactivation: &SndcpDeactivation) -> Result<BitBuffer, SndcpPdpError> {
    let conditional_len = match deactivation {
        SndcpDeactivation::AllNsapis => 0,
        SndcpDeactivation::Nsapi(nsapi) => {
            validate_nsapi_for_pdp(*nsapi)?;
            4
        }
    };
    let mut pdu = BitBuffer::new(4 + 8 + conditional_len + 1);
    pdu.write_bits(pdu_type as u64, 4);
    pdu.write_bits(deactivation_type_code(deactivation) as u64, 8);
    if let SndcpDeactivation::Nsapi(nsapi) = deactivation {
        pdu.write_bits(*nsapi as u64, 4);
    }
    write_no_optional_elements(&mut pdu);
    pdu.seek(0);
    Ok(pdu)
}

fn decode_deactivate_pdp_context(pdu: &BitBuffer, expected_pdu_type: u8) -> Result<SndcpDeactivation, SndcpPdpError> {
    let mut pdu = reader(pdu);
    expect_pdu_type(&mut pdu, expected_pdu_type)?;
    let deactivation_type = read_u8(&mut pdu, 8, "deactivation_type")?;
    let deactivation = match deactivation_type {
        0 => SndcpDeactivation::AllNsapis,
        1 => {
            let nsapi = read_u8(&mut pdu, 4, "nsapi")?;
            validate_nsapi_for_pdp(nsapi)?;
            SndcpDeactivation::Nsapi(nsapi)
        }
        other => return Err(SndcpPdpError::ReservedDeactivationType(other)),
    };
    read_no_optional_tail(&mut pdu)?;
    Ok(deactivation)
}

fn reader(pdu: &BitBuffer) -> BitBuffer {
    let mut pdu = BitBuffer::from_bitbuffer(pdu);
    pdu.seek(0);
    pdu
}

fn expect_pdu_type(pdu: &mut BitBuffer, expected: u8) -> Result<(), SndcpPdpError> {
    let actual = read_u8(pdu, 4, "sn_pdu_type")?;
    if actual == expected {
        Ok(())
    } else {
        Err(SndcpPdpError::UnexpectedPduType { expected, actual })
    }
}

fn read_u8(pdu: &mut BitBuffer, bits: usize, field: &'static str) -> Result<u8, SndcpPdpError> {
    pdu.read_bits(bits).map(|value| value as u8).ok_or(SndcpPdpError::TooShort(field))
}

fn read_ipv4(pdu: &mut BitBuffer) -> Result<[u8; 4], SndcpPdpError> {
    Ok([
        read_u8(pdu, 8, "ipv4_octet_1")?,
        read_u8(pdu, 8, "ipv4_octet_2")?,
        read_u8(pdu, 8, "ipv4_octet_3")?,
        read_u8(pdu, 8, "ipv4_octet_4")?,
    ])
}

fn write_ipv4(pdu: &mut BitBuffer, address: [u8; 4]) {
    for octet in address {
        pdu.write_bits(octet as u64, 8);
    }
}

fn write_no_optional_elements(pdu: &mut BitBuffer) {
    pdu.write_bits(0, 1);
}

fn read_no_optional_tail(pdu: &mut BitBuffer) -> Result<(), SndcpPdpError> {
    match read_u8(pdu, 1, "o_bit")? {
        0 => {
            let bits = pdu.get_len_remaining();
            if bits == 0 {
                Ok(())
            } else {
                Err(SndcpPdpError::TrailingBits { bits })
            }
        }
        _ => Err(SndcpPdpError::UnsupportedOptionalElements),
    }
}

fn validate_sndcp_version(version: u8) -> Result<(), SndcpPdpError> {
    if version == SNDCP_VERSION_1 {
        Ok(())
    } else {
        Err(SndcpPdpError::ReservedSndcpVersion(version))
    }
}

fn validate_nsapi_for_pdp(nsapi: u8) -> Result<(), SndcpPdpError> {
    validate_nsapi(nsapi).map(|_| ()).map_err(|_| SndcpPdpError::ReservedNsapi(nsapi))
}

fn validate_pcomp_negotiation(pcomp_negotiation: u8) -> Result<(), SndcpPdpError> {
    if pcomp_negotiation == PCOMP_NEGOTIATION_NONE {
        Ok(())
    } else {
        Err(SndcpPdpError::UnsupportedPcompNegotiation(pcomp_negotiation))
    }
}

fn validate_pdu_priority_max(pdu_priority_max: u8) -> Result<(), SndcpPdpError> {
    if pdu_priority_max <= 7 {
        Ok(())
    } else {
        Err(SndcpPdpError::ReservedPduPriorityMax(pdu_priority_max))
    }
}

fn validate_ready_timer(ready_timer: u8) -> Result<(), SndcpPdpError> {
    if (1..=14).contains(&ready_timer) {
        Ok(())
    } else {
        Err(SndcpPdpError::ReservedReadyTimer(ready_timer))
    }
}

fn validate_standby_timer(standby_timer: u8) -> Result<(), SndcpPdpError> {
    if (1..=15).contains(&standby_timer) {
        Ok(())
    } else {
        Err(SndcpPdpError::ReservedStandbyTimer(standby_timer))
    }
}

fn validate_response_wait_timer(response_wait_timer: u8) -> Result<(), SndcpPdpError> {
    if response_wait_timer <= 14 {
        Ok(())
    } else {
        Err(SndcpPdpError::ReservedResponseWaitTimer(response_wait_timer))
    }
}

fn address_type_identifier_in_demand(code: u8) -> Result<SndcpAddressTypeIdentifierInDemand, SndcpPdpError> {
    match code {
        0 => Ok(SndcpAddressTypeIdentifierInDemand::Ipv4StaticAddress),
        1 => Ok(SndcpAddressTypeIdentifierInDemand::Ipv4DynamicAddressNegotiation),
        2 => Ok(SndcpAddressTypeIdentifierInDemand::Ipv6),
        3 => Ok(SndcpAddressTypeIdentifierInDemand::MobileIpv4ForeignAgentCareOfAddressRequested),
        4 => Ok(SndcpAddressTypeIdentifierInDemand::MobileIpv4CoLocatedCareOfAddressRequested),
        5 => Ok(SndcpAddressTypeIdentifierInDemand::PrimaryNsapiSecondaryPdpContextRequested),
        other => Err(SndcpPdpError::ReservedAddressTypeIdentifierInDemand(other)),
    }
}

fn address_demand_code(address: &SndcpActivateAddressDemand) -> u8 {
    match address {
        SndcpActivateAddressDemand::Ipv4Static(_) => 0,
        SndcpActivateAddressDemand::Ipv4Dynamic => 1,
        SndcpActivateAddressDemand::Ipv6 => 2,
        SndcpActivateAddressDemand::MobileIpv4ForeignAgentCareOfAddress => 3,
        SndcpActivateAddressDemand::MobileIpv4CoLocatedCareOfAddress => 4,
        SndcpActivateAddressDemand::SecondaryPdpContext { .. } => 5,
    }
}

fn packet_data_ms_type(code: u8) -> Result<SnPacketDataMsType, SndcpPdpError> {
    match code {
        0 => Ok(SnPacketDataMsType::TypeAParallel),
        1 => Ok(SnPacketDataMsType::TypeBAlternating),
        2 => Ok(SnPacketDataMsType::TypeCIpSingleMode),
        3 => Ok(SnPacketDataMsType::TypeDRestrictedIpSingleMode),
        other => Err(SndcpPdpError::ReservedPacketDataMsType(other)),
    }
}

fn packet_data_ms_type_code(packet_data_ms_type: SnPacketDataMsType) -> u8 {
    match packet_data_ms_type {
        SnPacketDataMsType::TypeAParallel => 0,
        SnPacketDataMsType::TypeBAlternating => 1,
        SnPacketDataMsType::TypeCIpSingleMode => 2,
        SnPacketDataMsType::TypeDRestrictedIpSingleMode => 3,
    }
}

fn type_identifier_in_accept(code: u8) -> Result<SndcpTypeIdentifierInAccept, SndcpPdpError> {
    match code {
        0 => Ok(SndcpTypeIdentifierInAccept::NoAddress),
        1 => Ok(SndcpTypeIdentifierInAccept::Ipv4StaticAddress),
        2 => Ok(SndcpTypeIdentifierInAccept::Ipv4DynamicAddress),
        other => Err(SndcpPdpError::ReservedTypeIdentifierInAccept(other)),
    }
}

fn type_identifier_in_accept_code(type_identifier: SndcpTypeIdentifierInAccept) -> u8 {
    match type_identifier {
        SndcpTypeIdentifierInAccept::NoAddress => 0,
        SndcpTypeIdentifierInAccept::Ipv4StaticAddress => 1,
        SndcpTypeIdentifierInAccept::Ipv4DynamicAddress => 2,
    }
}

fn maximum_transmission_unit(code: u8) -> Result<SndcpMaximumTransmissionUnit, SndcpPdpError> {
    match code {
        1 => Ok(SndcpMaximumTransmissionUnit::Octets296),
        2 => Ok(SndcpMaximumTransmissionUnit::Octets576),
        3 => Ok(SndcpMaximumTransmissionUnit::Octets1006),
        4 => Ok(SndcpMaximumTransmissionUnit::Octets1500),
        5 => Ok(SndcpMaximumTransmissionUnit::Octets2002),
        other => Err(SndcpPdpError::ReservedMaximumTransmissionUnit(other)),
    }
}

fn maximum_transmission_unit_code(maximum_transmission_unit: SndcpMaximumTransmissionUnit) -> u8 {
    match maximum_transmission_unit {
        SndcpMaximumTransmissionUnit::Octets296 => 1,
        SndcpMaximumTransmissionUnit::Octets576 => 2,
        SndcpMaximumTransmissionUnit::Octets1006 => 3,
        SndcpMaximumTransmissionUnit::Octets1500 => 4,
        SndcpMaximumTransmissionUnit::Octets2002 => 5,
    }
}

fn deactivation_type_code(deactivation: &SndcpDeactivation) -> u8 {
    match deactivation {
        SndcpDeactivation::AllNsapis => 0,
        SndcpDeactivation::Nsapi(_) => 1,
    }
}

fn activation_reject_cause(code: u8) -> SndcpActivationRejectCause {
    match code {
        0 => SndcpActivationRejectCause::Undefined,
        1 => SndcpActivationRejectCause::MsNotProvisionedForPacketData,
        2 => SndcpActivationRejectCause::Ipv4NotSupported,
        3 => SndcpActivationRejectCause::Ipv6NotSupported,
        4 => SndcpActivationRejectCause::Ipv4DynamicAddressNegotiationNotSupported,
        7 => SndcpActivationRejectCause::DynamicAddressPoolEmpty,
        8 => SndcpActivationRejectCause::StaticAddressNotCorrect,
        9 => SndcpActivationRejectCause::StaticAddressInUse,
        10 => SndcpActivationRejectCause::StaticAddressNotAllowed,
        15 => SndcpActivationRejectCause::PacketDataMsTypeNotSupported,
        16 => SndcpActivationRejectCause::SndcpVersionNotSupported,
        19 => SndcpActivationRejectCause::MaximumNumberOfPdpContextsPerItsiExceeded,
        27 => SndcpActivationRejectCause::SecondaryPdpContextsNotSupported,
        28 => SndcpActivationRejectCause::PrimaryPdpContextDoesNotExist,
        34 => SndcpActivationRejectCause::SndcpServiceTemporarilyNotAvailable,
        other => SndcpActivationRejectCause::Other(other),
    }
}

fn activation_reject_cause_code(cause: SndcpActivationRejectCause) -> u8 {
    match cause {
        SndcpActivationRejectCause::Undefined => 0,
        SndcpActivationRejectCause::MsNotProvisionedForPacketData => 1,
        SndcpActivationRejectCause::Ipv4NotSupported => 2,
        SndcpActivationRejectCause::Ipv6NotSupported => 3,
        SndcpActivationRejectCause::Ipv4DynamicAddressNegotiationNotSupported => 4,
        SndcpActivationRejectCause::DynamicAddressPoolEmpty => 7,
        SndcpActivationRejectCause::StaticAddressNotCorrect => 8,
        SndcpActivationRejectCause::StaticAddressInUse => 9,
        SndcpActivationRejectCause::StaticAddressNotAllowed => 10,
        SndcpActivationRejectCause::PacketDataMsTypeNotSupported => 15,
        SndcpActivationRejectCause::SndcpVersionNotSupported => 16,
        SndcpActivationRejectCause::MaximumNumberOfPdpContextsPerItsiExceeded => 19,
        SndcpActivationRejectCause::SecondaryPdpContextsNotSupported => 27,
        SndcpActivationRejectCause::PrimaryPdpContextDoesNotExist => 28,
        SndcpActivationRejectCause::SndcpServiceTemporarilyNotAvailable => 34,
        SndcpActivationRejectCause::Other(code) => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dynamic_ipv4_demand() -> SndcpActivatePdpContextDemand {
        SndcpActivatePdpContextDemand {
            sndcp_version: SNDCP_VERSION_1,
            nsapi: 2,
            address: SndcpActivateAddressDemand::Ipv4Dynamic,
            packet_data_ms_type: SnPacketDataMsType::TypeAParallel,
            pcomp_negotiation: PCOMP_NEGOTIATION_NONE,
        }
    }

    fn dynamic_ipv4_accept() -> SndcpActivatePdpContextAccept {
        SndcpActivatePdpContextAccept {
            nsapi: 2,
            pdu_priority_max: 4,
            ready_timer: 8,
            standby_timer: 4,
            response_wait_timer: 7,
            type_identifier: SndcpTypeIdentifierInAccept::Ipv4DynamicAddress,
            assigned_address: Some(SnAddress::Ipv4([10, 0, 0, 226])),
            pcomp_negotiation: PCOMP_NEGOTIATION_NONE,
            maximum_transmission_unit: SndcpMaximumTransmissionUnit::Octets576,
        }
    }

    #[test]
    fn activate_demand_dynamic_ipv4_round_trips_without_optional_elements() {
        let demand = dynamic_ipv4_demand();

        let encoded = encode_activate_pdp_context_demand(&demand).expect("demand should encode");
        let decoded = decode_activate_pdp_context_demand(&encoded).expect("demand should decode");

        assert_eq!(encoded.get_len(), 28);
        assert_eq!(decoded, demand);
    }

    #[test]
    fn activate_demand_static_ipv4_and_secondary_contexts_encode_conditional_fields() {
        let static_demand = SndcpActivatePdpContextDemand {
            address: SndcpActivateAddressDemand::Ipv4Static([10, 0, 0, 18]),
            ..dynamic_ipv4_demand()
        };
        assert_eq!(
            decode_activate_pdp_context_demand(&encode_activate_pdp_context_demand(&static_demand).unwrap()).unwrap(),
            static_demand
        );

        let secondary = SndcpActivatePdpContextDemand {
            nsapi: 3,
            address: SndcpActivateAddressDemand::SecondaryPdpContext { primary_nsapi: 2 },
            ..dynamic_ipv4_demand()
        };
        assert_eq!(
            decode_activate_pdp_context_demand(&encode_activate_pdp_context_demand(&secondary).unwrap()).unwrap(),
            secondary
        );
    }

    #[test]
    fn activate_accept_dynamic_ipv4_round_trips_without_optional_elements() {
        let accept = dynamic_ipv4_accept();

        let encoded = encode_activate_pdp_context_accept(&accept).expect("accept should encode");
        let decoded = decode_activate_pdp_context_accept(&encoded).expect("accept should decode");

        assert_eq!(encoded.get_len(), 70);
        assert_eq!(decoded, accept);
    }

    #[test]
    fn activate_reject_round_trips_clause_28_4_5_3_cause() {
        let reject = SndcpActivatePdpContextReject {
            nsapi: 2,
            cause: SndcpActivationRejectCause::SndcpServiceTemporarilyNotAvailable,
        };

        let encoded = encode_activate_pdp_context_reject(&reject).expect("reject should encode");
        let decoded = decode_activate_pdp_context_reject(&encoded).expect("reject should decode");

        assert_eq!(encoded.get_len(), 17);
        assert_eq!(decoded, reject);
    }

    #[test]
    fn deactivate_demand_and_accept_round_trip_all_or_single_nsapi() {
        let all = SndcpDeactivation::AllNsapis;
        assert_eq!(
            decode_deactivate_pdp_context_demand(&encode_deactivate_pdp_context_demand(&all).unwrap()).unwrap(),
            all
        );
        assert_eq!(
            decode_deactivate_pdp_context_accept(&encode_deactivate_pdp_context_accept(&all).unwrap()).unwrap(),
            all
        );

        let single = SndcpDeactivation::Nsapi(2);
        assert_eq!(
            decode_deactivate_pdp_context_demand(&encode_deactivate_pdp_context_demand(&single).unwrap()).unwrap(),
            single
        );
        assert_eq!(
            decode_deactivate_pdp_context_accept(&encode_deactivate_pdp_context_accept(&single).unwrap()).unwrap(),
            single
        );
    }

    #[test]
    fn activation_decoders_reject_reserved_values_and_optionals() {
        let mut reserved_nsapi = encode_activate_pdp_context_demand(&dynamic_ipv4_demand()).unwrap();
        reserved_nsapi.seek(8);
        reserved_nsapi.write_bits(15, 4);
        reserved_nsapi.seek(0);
        assert_eq!(
            decode_activate_pdp_context_demand(&reserved_nsapi),
            Err(SndcpPdpError::ReservedNsapi(15))
        );

        let mut optional = encode_activate_pdp_context_demand(&dynamic_ipv4_demand()).unwrap();
        optional.seek(optional.get_len() - 1);
        optional.write_bits(1, 1);
        optional.seek(0);
        assert_eq!(
            decode_activate_pdp_context_demand(&optional),
            Err(SndcpPdpError::UnsupportedOptionalElements)
        );

        let unsupported_pcomp = SndcpActivatePdpContextDemand {
            pcomp_negotiation: 1,
            ..dynamic_ipv4_demand()
        };
        assert_pdp_encode_error(
            encode_activate_pdp_context_demand(&unsupported_pcomp),
            SndcpPdpError::UnsupportedPcompNegotiation(1),
        );
    }

    #[test]
    fn accept_decoder_rejects_reserved_timers_and_missing_ipv4_address() {
        let mut reserved_ready_timer = encode_activate_pdp_context_accept(&dynamic_ipv4_accept()).unwrap();
        reserved_ready_timer.seek(11);
        reserved_ready_timer.write_bits(0, 4);
        reserved_ready_timer.seek(0);
        assert_eq!(
            decode_activate_pdp_context_accept(&reserved_ready_timer),
            Err(SndcpPdpError::ReservedReadyTimer(0))
        );

        let missing_address = SndcpActivatePdpContextAccept {
            assigned_address: None,
            ..dynamic_ipv4_accept()
        };
        assert_pdp_encode_error(
            encode_activate_pdp_context_accept(&missing_address),
            SndcpPdpError::MissingStaticIpv4Address,
        );
    }

    #[test]
    fn activate_accept_rejects_out_of_range_pdu_priority_max() {
        let invalid_priority = SndcpActivatePdpContextAccept {
            pdu_priority_max: 8,
            ..dynamic_ipv4_accept()
        };

        assert_pdp_encode_error(
            encode_activate_pdp_context_accept(&invalid_priority),
            SndcpPdpError::ReservedPduPriorityMax(8),
        );
    }

    #[test]
    fn deactivate_rejects_reserved_deactivation_type() {
        let mut pdu = BitBuffer::new(4 + 8 + 4 + 1);
        pdu.write_bits(SN_PDU_TYPE_DEACTIVATE_PDP_CONTEXT_DEMAND as u64, 4);
        pdu.write_bits(2, 8);
        pdu.write_bits(2, 4);
        pdu.write_bits(0, 1);
        pdu.seek(0);

        assert_eq!(
            decode_deactivate_pdp_context_demand(&pdu),
            Err(SndcpPdpError::ReservedDeactivationType(2))
        );
    }

    fn assert_pdp_encode_error(result: Result<BitBuffer, SndcpPdpError>, expected: SndcpPdpError) {
        match result {
            Ok(_) => panic!("expected PDP encode error {expected:?}"),
            Err(err) => assert_eq!(err, expected),
        }
    }
}
