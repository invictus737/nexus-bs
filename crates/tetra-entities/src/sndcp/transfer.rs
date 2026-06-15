// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original TETRA SNDCP transfer-control PDU primitives.

use tetra_core::BitBuffer;
use tetra_saps::sn::validate_nsapi;

pub const SN_PDU_TYPE_DATA: u8 = 5;
pub const SN_PDU_TYPE_DATA_TRANSMIT_REQUEST: u8 = 6;
pub const SN_PDU_TYPE_DATA_TRANSMIT_RESPONSE: u8 = 7;
pub const SN_PDU_TYPE_END_OF_DATA: u8 = 8;
pub const SN_PDU_TYPE_RECONNECT: u8 = 9;
pub const SN_PDU_TYPE_PAGE: u8 = 10;
pub const SN_PDU_TYPE_NOT_SUPPORTED: u8 = 11;
pub const SN_PDU_TYPE_DATA_PRIORITY: u8 = 12;
pub const SN_PDU_TYPE_MODIFY: u8 = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpTransferRejectCause {
    Undefined,
    UnknownNsapi,
    SystemResourcesNotAvailable,
    RequestedMinimumPeakThroughputNotAvailable,
    RequestedScheduleNotAvailable,
    SndcpServiceTemporarilyNotAvailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SndcpDataTransmitRequest {
    pub nsapi: u8,
    pub logical_link_status: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SndcpDataTransmitResponse {
    pub nsapi: u8,
    pub result: SndcpDataTransmitResponseResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpDataTransmitResponseResult {
    Accepted,
    Rejected(SndcpTransferRejectCause),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SndcpEndOfData {
    pub immediate_service_change: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SndcpReconnect {
    pub nsapi: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SndcpNotSupported {
    pub not_supported_pdu_type: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SndcpTransferControl {
    DataTransmitRequest(SndcpDataTransmitRequest),
    DataTransmitResponse(SndcpDataTransmitResponse),
    EndOfData(SndcpEndOfData),
    Reconnect(SndcpReconnect),
    NotSupported(SndcpNotSupported),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SndcpTransferError {
    TooShort(&'static str),
    TrailingBits { bits: usize },
    UnsupportedOptionalElements,
    UnsupportedResourceRequest,
    UnexpectedPduType { expected: u8, actual: u8 },
    UnsupportedPduType(u8),
    ReservedNsapi(u8),
    ReservedTransmitRejectCause(u8),
    ReservedNotSupportedPduType(u8),
    MissingRejectCause,
    UnexpectedRejectCause,
}

pub fn encode_data_transmit_request(request: &SndcpDataTransmitRequest) -> Result<BitBuffer, SndcpTransferError> {
    validate_nsapi_for_transfer(request.nsapi)?;

    let mut pdu = BitBuffer::new(4 + 4 + 1 + 1 + 1);
    pdu.write_bits(SN_PDU_TYPE_DATA_TRANSMIT_REQUEST as u64, 4);
    pdu.write_bits(request.nsapi as u64, 4);
    pdu.write_bits(request.logical_link_status as u64, 1);
    pdu.write_bits(0, 1);
    write_no_optional_elements(&mut pdu);
    pdu.seek(0);
    Ok(pdu)
}

pub fn decode_data_transmit_request(pdu: &BitBuffer) -> Result<SndcpDataTransmitRequest, SndcpTransferError> {
    let mut pdu = reader(pdu);
    expect_pdu_type(&mut pdu, SN_PDU_TYPE_DATA_TRANSMIT_REQUEST)?;
    let nsapi = read_u8(&mut pdu, 4, "nsapi")?;
    validate_nsapi_for_transfer(nsapi)?;
    let logical_link_status = read_bool(&mut pdu, "logical_link_status")?;
    let enhanced_pi4dqpsk_service = read_bool(&mut pdu, "enhanced_pi4dqpsk_service")?;
    if enhanced_pi4dqpsk_service {
        return Err(SndcpTransferError::UnsupportedResourceRequest);
    }
    read_no_optional_tail(&mut pdu)?;
    Ok(SndcpDataTransmitRequest {
        nsapi,
        logical_link_status,
    })
}

pub fn encode_data_transmit_response(response: &SndcpDataTransmitResponse) -> Result<BitBuffer, SndcpTransferError> {
    validate_nsapi_for_transfer(response.nsapi)?;
    let reject_bits = match response.result {
        SndcpDataTransmitResponseResult::Accepted => 0,
        SndcpDataTransmitResponseResult::Rejected(_) => 8,
    };

    let mut pdu = BitBuffer::new(4 + 4 + 1 + reject_bits + 1);
    pdu.write_bits(SN_PDU_TYPE_DATA_TRANSMIT_RESPONSE as u64, 4);
    pdu.write_bits(response.nsapi as u64, 4);
    match response.result {
        SndcpDataTransmitResponseResult::Accepted => pdu.write_bits(1, 1),
        SndcpDataTransmitResponseResult::Rejected(cause) => {
            pdu.write_bits(0, 1);
            pdu.write_bits(transmit_reject_cause_code(cause) as u64, 8);
        }
    }
    write_no_optional_elements(&mut pdu);
    pdu.seek(0);
    Ok(pdu)
}

pub fn decode_data_transmit_response(pdu: &BitBuffer) -> Result<SndcpDataTransmitResponse, SndcpTransferError> {
    let mut pdu = reader(pdu);
    expect_pdu_type(&mut pdu, SN_PDU_TYPE_DATA_TRANSMIT_RESPONSE)?;
    let nsapi = read_u8(&mut pdu, 4, "nsapi")?;
    validate_nsapi_for_transfer(nsapi)?;
    let accept = read_bool(&mut pdu, "accept_reject")?;
    let result = if accept {
        SndcpDataTransmitResponseResult::Accepted
    } else {
        let cause = read_u8(&mut pdu, 8, "transmit_response_reject_cause")?;
        SndcpDataTransmitResponseResult::Rejected(transmit_reject_cause(cause)?)
    };
    read_no_optional_tail(&mut pdu)?;
    Ok(SndcpDataTransmitResponse { nsapi, result })
}

pub fn encode_end_of_data(end_of_data: &SndcpEndOfData) -> Result<BitBuffer, SndcpTransferError> {
    let mut pdu = BitBuffer::new(4 + 1 + 1);
    pdu.write_bits(SN_PDU_TYPE_END_OF_DATA as u64, 4);
    pdu.write_bits(end_of_data.immediate_service_change as u64, 1);
    write_no_optional_elements(&mut pdu);
    pdu.seek(0);
    Ok(pdu)
}

pub fn decode_end_of_data(pdu: &BitBuffer) -> Result<SndcpEndOfData, SndcpTransferError> {
    let mut pdu = reader(pdu);
    expect_pdu_type(&mut pdu, SN_PDU_TYPE_END_OF_DATA)?;
    let immediate_service_change = read_bool(&mut pdu, "immediate_service_change")?;
    read_no_optional_tail(&mut pdu)?;
    Ok(SndcpEndOfData { immediate_service_change })
}

pub fn encode_reconnect(reconnect: &SndcpReconnect) -> Result<BitBuffer, SndcpTransferError> {
    if let Some(nsapi) = reconnect.nsapi {
        validate_nsapi_for_transfer(nsapi)?;
    }

    let nsapi_bits = if reconnect.nsapi.is_some() { 4 } else { 0 };
    let mut pdu = BitBuffer::new(4 + 1 + nsapi_bits + 1 + 1);
    pdu.write_bits(SN_PDU_TYPE_RECONNECT as u64, 4);
    pdu.write_bits(reconnect.nsapi.is_some() as u64, 1);
    if let Some(nsapi) = reconnect.nsapi {
        pdu.write_bits(nsapi as u64, 4);
    }
    pdu.write_bits(0, 1);
    write_no_optional_elements(&mut pdu);
    pdu.seek(0);
    Ok(pdu)
}

pub fn decode_reconnect(pdu: &BitBuffer) -> Result<SndcpReconnect, SndcpTransferError> {
    let mut pdu = reader(pdu);
    expect_pdu_type(&mut pdu, SN_PDU_TYPE_RECONNECT)?;
    let data_to_send = read_bool(&mut pdu, "data_to_send")?;
    let nsapi = if data_to_send {
        let nsapi = read_u8(&mut pdu, 4, "nsapi")?;
        validate_nsapi_for_transfer(nsapi)?;
        Some(nsapi)
    } else {
        None
    };
    let enhanced_pi4dqpsk_service = read_bool(&mut pdu, "enhanced_pi4dqpsk_service")?;
    if enhanced_pi4dqpsk_service {
        return Err(SndcpTransferError::UnsupportedResourceRequest);
    }
    read_no_optional_tail(&mut pdu)?;
    Ok(SndcpReconnect { nsapi })
}

pub fn encode_not_supported(not_supported: &SndcpNotSupported) -> Result<BitBuffer, SndcpTransferError> {
    validate_not_supported_pdu_type(not_supported.not_supported_pdu_type)?;
    let mut pdu = BitBuffer::new(8);
    pdu.write_bits(SN_PDU_TYPE_NOT_SUPPORTED as u64, 4);
    pdu.write_bits(not_supported.not_supported_pdu_type as u64, 4);
    pdu.seek(0);
    Ok(pdu)
}

pub fn decode_not_supported(pdu: &BitBuffer) -> Result<SndcpNotSupported, SndcpTransferError> {
    let mut pdu = reader(pdu);
    expect_pdu_type(&mut pdu, SN_PDU_TYPE_NOT_SUPPORTED)?;
    let not_supported_pdu_type = read_u8(&mut pdu, 4, "not_supported_sn_pdu_type")?;
    validate_not_supported_pdu_type(not_supported_pdu_type)?;
    reject_trailing_bits(&pdu)?;
    Ok(SndcpNotSupported { not_supported_pdu_type })
}

pub fn decode_transfer_control_pdu(pdu: &BitBuffer) -> Result<SndcpTransferControl, SndcpTransferError> {
    match sn_pdu_type(pdu)? {
        SN_PDU_TYPE_DATA_TRANSMIT_REQUEST => decode_data_transmit_request(pdu).map(SndcpTransferControl::DataTransmitRequest),
        SN_PDU_TYPE_DATA_TRANSMIT_RESPONSE => decode_data_transmit_response(pdu).map(SndcpTransferControl::DataTransmitResponse),
        SN_PDU_TYPE_END_OF_DATA => decode_end_of_data(pdu).map(SndcpTransferControl::EndOfData),
        SN_PDU_TYPE_RECONNECT => decode_reconnect(pdu).map(SndcpTransferControl::Reconnect),
        SN_PDU_TYPE_NOT_SUPPORTED => decode_not_supported(pdu).map(SndcpTransferControl::NotSupported),
        other => Err(SndcpTransferError::UnsupportedPduType(other)),
    }
}

pub fn sn_pdu_type(pdu: &BitBuffer) -> Result<u8, SndcpTransferError> {
    let mut pdu = reader(pdu);
    read_u8(&mut pdu, 4, "sn_pdu_type")
}

fn reader(pdu: &BitBuffer) -> BitBuffer {
    let mut pdu = BitBuffer::from_bitbuffer(pdu);
    pdu.seek(0);
    pdu
}

fn expect_pdu_type(pdu: &mut BitBuffer, expected: u8) -> Result<(), SndcpTransferError> {
    let actual = read_u8(pdu, 4, "sn_pdu_type")?;
    if actual == expected {
        Ok(())
    } else {
        Err(SndcpTransferError::UnexpectedPduType { expected, actual })
    }
}

fn read_bool(pdu: &mut BitBuffer, field: &'static str) -> Result<bool, SndcpTransferError> {
    read_u8(pdu, 1, field).map(|value| value != 0)
}

fn read_u8(pdu: &mut BitBuffer, bits: usize, field: &'static str) -> Result<u8, SndcpTransferError> {
    pdu.read_bits(bits)
        .map(|value| value as u8)
        .ok_or(SndcpTransferError::TooShort(field))
}

fn write_no_optional_elements(pdu: &mut BitBuffer) {
    pdu.write_bits(0, 1);
}

fn read_no_optional_tail(pdu: &mut BitBuffer) -> Result<(), SndcpTransferError> {
    match read_u8(pdu, 1, "o_bit")? {
        0 => reject_trailing_bits(pdu),
        _ => Err(SndcpTransferError::UnsupportedOptionalElements),
    }
}

fn reject_trailing_bits(pdu: &BitBuffer) -> Result<(), SndcpTransferError> {
    let bits = pdu.get_len_remaining();
    if bits == 0 {
        Ok(())
    } else {
        Err(SndcpTransferError::TrailingBits { bits })
    }
}

fn validate_nsapi_for_transfer(nsapi: u8) -> Result<(), SndcpTransferError> {
    validate_nsapi(nsapi)
        .map(|_| ())
        .map_err(|_| SndcpTransferError::ReservedNsapi(nsapi))
}

fn transmit_reject_cause(code: u8) -> Result<SndcpTransferRejectCause, SndcpTransferError> {
    match code {
        0 => Ok(SndcpTransferRejectCause::Undefined),
        1 => Ok(SndcpTransferRejectCause::UnknownNsapi),
        2 => Ok(SndcpTransferRejectCause::SystemResourcesNotAvailable),
        23 => Ok(SndcpTransferRejectCause::RequestedMinimumPeakThroughputNotAvailable),
        25 => Ok(SndcpTransferRejectCause::RequestedScheduleNotAvailable),
        34 => Ok(SndcpTransferRejectCause::SndcpServiceTemporarilyNotAvailable),
        other => Err(SndcpTransferError::ReservedTransmitRejectCause(other)),
    }
}

fn transmit_reject_cause_code(cause: SndcpTransferRejectCause) -> u8 {
    match cause {
        SndcpTransferRejectCause::Undefined => 0,
        SndcpTransferRejectCause::UnknownNsapi => 1,
        SndcpTransferRejectCause::SystemResourcesNotAvailable => 2,
        SndcpTransferRejectCause::RequestedMinimumPeakThroughputNotAvailable => 23,
        SndcpTransferRejectCause::RequestedScheduleNotAvailable => 25,
        SndcpTransferRejectCause::SndcpServiceTemporarilyNotAvailable => 34,
    }
}

fn validate_not_supported_pdu_type(pdu_type: u8) -> Result<(), SndcpTransferError> {
    match pdu_type {
        0..=10 | 12 | 13 => Ok(()),
        other => Err(SndcpTransferError::ReservedNotSupportedPduType(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_transmit_request_round_trips_without_optional_elements() {
        let request = SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
        };

        let encoded = encode_data_transmit_request(&request).expect("request should encode");
        let decoded = decode_data_transmit_request(&encoded).expect("request should decode");

        assert_eq!(encoded.get_len(), 11);
        assert_eq!(decoded, request);
    }

    #[test]
    fn data_transmit_response_accept_and_reject_round_trip() {
        let accepted = SndcpDataTransmitResponse {
            nsapi: 2,
            result: SndcpDataTransmitResponseResult::Accepted,
        };
        let rejected = SndcpDataTransmitResponse {
            nsapi: 3,
            result: SndcpDataTransmitResponseResult::Rejected(SndcpTransferRejectCause::SndcpServiceTemporarilyNotAvailable),
        };

        assert_eq!(
            decode_data_transmit_response(&encode_data_transmit_response(&accepted).unwrap()).unwrap(),
            accepted
        );
        assert_eq!(
            decode_data_transmit_response(&encode_data_transmit_response(&rejected).unwrap()).unwrap(),
            rejected
        );
    }

    #[test]
    fn end_of_data_round_trips_immediate_service_change_flag() {
        for immediate_service_change in [false, true] {
            let end_of_data = SndcpEndOfData { immediate_service_change };

            let encoded = encode_end_of_data(&end_of_data).expect("SN-END OF DATA should encode");
            let decoded = decode_end_of_data(&encoded).expect("SN-END OF DATA should decode");

            assert_eq!(encoded.get_len(), 6);
            assert_eq!(decoded, end_of_data);
        }
    }

    #[test]
    fn reconnect_round_trips_with_and_without_data_to_send() {
        let no_data = SndcpReconnect { nsapi: None };
        let data = SndcpReconnect { nsapi: Some(2) };

        assert_eq!(decode_reconnect(&encode_reconnect(&no_data).unwrap()).unwrap(), no_data);
        assert_eq!(decode_reconnect(&encode_reconnect(&data).unwrap()).unwrap(), data);
        assert_eq!(encode_reconnect(&no_data).unwrap().get_len(), 7);
        assert_eq!(encode_reconnect(&data).unwrap().get_len(), 11);
    }

    #[test]
    fn not_supported_round_trips_valid_pdu_types_and_rejects_reserved_type_11() {
        let not_supported = SndcpNotSupported {
            not_supported_pdu_type: SN_PDU_TYPE_UNITDATA_FOR_TEST,
        };

        assert_eq!(
            decode_not_supported(&encode_not_supported(&not_supported).unwrap()).unwrap(),
            not_supported
        );
        assert_eq!(
            encode_not_supported(&SndcpNotSupported {
                not_supported_pdu_type: SN_PDU_TYPE_NOT_SUPPORTED
            })
            .expect_err("SN-NOT SUPPORTED cannot name reserved not-supported type 11"),
            SndcpTransferError::ReservedNotSupportedPduType(SN_PDU_TYPE_NOT_SUPPORTED)
        );
    }

    #[test]
    fn transfer_control_dispatches_supported_control_types() {
        let request = encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
        })
        .unwrap();
        let end = encode_end_of_data(&SndcpEndOfData {
            immediate_service_change: false,
        })
        .unwrap();

        assert!(matches!(
            decode_transfer_control_pdu(&request),
            Ok(SndcpTransferControl::DataTransmitRequest(_))
        ));
        assert!(matches!(decode_transfer_control_pdu(&end), Ok(SndcpTransferControl::EndOfData(_))));
    }

    #[test]
    fn decoders_reject_reserved_nsapi_resource_request_optionals_and_trailing_bits() {
        let mut reserved_nsapi = encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
        })
        .unwrap();
        reserved_nsapi.seek(4);
        reserved_nsapi.write_bits(15, 4);
        reserved_nsapi.seek(0);
        assert_eq!(
            decode_data_transmit_request(&reserved_nsapi),
            Err(SndcpTransferError::ReservedNsapi(15))
        );

        let mut resource = encode_data_transmit_request(&SndcpDataTransmitRequest {
            nsapi: 2,
            logical_link_status: false,
        })
        .unwrap();
        resource.seek(9);
        resource.write_bits(1, 1);
        resource.seek(0);
        assert_eq!(
            decode_data_transmit_request(&resource),
            Err(SndcpTransferError::UnsupportedResourceRequest)
        );

        let mut optional = encode_end_of_data(&SndcpEndOfData {
            immediate_service_change: false,
        })
        .unwrap();
        optional.seek(optional.get_len() - 1);
        optional.write_bits(1, 1);
        optional.seek(0);
        assert_eq!(decode_end_of_data(&optional), Err(SndcpTransferError::UnsupportedOptionalElements));

        let mut trailing = BitBuffer::new(9);
        trailing.write_bits(SN_PDU_TYPE_NOT_SUPPORTED as u64, 4);
        trailing.write_bits(SN_PDU_TYPE_END_OF_DATA as u64, 4);
        trailing.write_bits(0, 1);
        trailing.seek(0);
        assert_eq!(decode_not_supported(&trailing), Err(SndcpTransferError::TrailingBits { bits: 1 }));
    }

    #[test]
    fn response_reject_cause_is_conditional_and_reserved_values_fail_closed() {
        let mut rejected_without_cause = BitBuffer::new(10);
        rejected_without_cause.write_bits(SN_PDU_TYPE_DATA_TRANSMIT_RESPONSE as u64, 4);
        rejected_without_cause.write_bits(2, 4);
        rejected_without_cause.write_bits(0, 1);
        rejected_without_cause.write_bits(0, 1);
        rejected_without_cause.seek(0);
        assert_eq!(
            decode_data_transmit_response(&rejected_without_cause),
            Err(SndcpTransferError::TooShort("transmit_response_reject_cause"))
        );

        let mut reserved_cause = encode_data_transmit_response(&SndcpDataTransmitResponse {
            nsapi: 2,
            result: SndcpDataTransmitResponseResult::Rejected(SndcpTransferRejectCause::UnknownNsapi),
        })
        .unwrap();
        reserved_cause.seek(9);
        reserved_cause.write_bits(3, 8);
        reserved_cause.seek(0);
        assert_eq!(
            decode_data_transmit_response(&reserved_cause),
            Err(SndcpTransferError::ReservedTransmitRejectCause(3))
        );
    }

    const SN_PDU_TYPE_UNITDATA_FOR_TEST: u8 = 4;
}
