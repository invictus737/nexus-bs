// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use tetra_core::typed_pdu_fields::{Type3FieldGeneric, Type4FieldGeneric, delimiters};
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mm::enums::type34_elem_id_dl::MmType34ElemIdDl;
use crate::mm::fields::group_identity_downlink::GroupIdentityDownlink;
use crate::mm::fields::group_identity_uplink::GroupIdentityUplink;

const MAX_STORED_TYPE3_BITS: usize = 128;
const MAX_STORED_TYPE4_BITS: usize = 64;
const MAX_TYPE34_LENGTH_BITS: usize = 2047;
const MAX_TYPE4_ELEMS: usize = 63;

fn invalid_type3_data_value(data: u128) -> u64 {
    data.min(u64::MAX as u128) as u64
}

fn skip_bits(buffer: &mut BitBuffer, len_bits: usize, field: &'static str) -> Result<(), PduParseErr> {
    if buffer.get_len_remaining() < len_bits {
        return Err(PduParseErr::BufferEnded { field: Some(field) });
    }
    buffer.seek_rel(len_bits as isize);
    Ok(())
}

fn read_type3_generic_payload(buffer: &mut BitBuffer, field_id: u64, len_bits: usize) -> Result<Type3FieldGeneric, PduParseErr> {
    let read_bits = len_bits.min(MAX_STORED_TYPE3_BITS);
    let data = if read_bits <= 64 {
        buffer
            .read_bits(read_bits)
            .ok_or(PduParseErr::BufferEnded { field: Some("type3 data") })? as u128
    } else {
        let hi_bits = read_bits - 64;
        let hi = buffer.read_bits(hi_bits).ok_or(PduParseErr::BufferEnded {
            field: Some("type3 data high"),
        })?;
        let lo = buffer.read_bits(64).ok_or(PduParseErr::BufferEnded {
            field: Some("type3 data low"),
        })?;
        ((hi as u128) << 64) | lo as u128
    };
    if len_bits > MAX_STORED_TYPE3_BITS {
        skip_bits(buffer, len_bits - MAX_STORED_TYPE3_BITS, "type3 truncated data")?;
    }
    Ok(Type3FieldGeneric {
        field_id,
        len: len_bits,
        data,
    })
}

fn read_type4_group_identity_downlink_payload(buffer: &mut BitBuffer, len_bits: usize) -> Result<Vec<GroupIdentityDownlink>, PduParseErr> {
    if len_bits < 6 {
        return Err(PduParseErr::InconsistentLength {
            expected: 6,
            found: len_bits,
        });
    }
    let num_elems = buffer.read_field(6, "type4 num_elems")? as usize;
    if num_elems == 0 {
        return Err(PduParseErr::InvalidValue {
            field: "parse_type4_header num_elems",
            value: 0,
        });
    }

    let payload_len = len_bits - 6;
    let start_pos = buffer.get_pos();
    let mut elems = Vec::with_capacity(num_elems);
    for _ in 0..num_elems {
        elems.push(GroupIdentityDownlink::from_bitbuf(buffer)?);
    }
    let parsed_len = buffer.get_pos() - start_pos;
    if parsed_len != payload_len {
        return Err(PduParseErr::InconsistentLength {
            expected: payload_len,
            found: parsed_len,
        });
    }
    Ok(elems)
}

fn read_type4_generic_payload(buffer: &mut BitBuffer, field_id: u64, len_bits: usize) -> Result<Type4FieldGeneric, PduParseErr> {
    if len_bits < 6 {
        return Err(PduParseErr::InconsistentLength {
            expected: 6,
            found: len_bits,
        });
    }
    let elems = buffer.read_field(6, "type4 num_elems")? as usize;
    if elems == 0 {
        return Err(PduParseErr::InvalidValue {
            field: "parse_type4_header num_elems",
            value: 0,
        });
    }
    let payload_len = len_bits - 6;
    let read_bits = payload_len.min(MAX_STORED_TYPE4_BITS);
    let data = buffer.read_field(read_bits, "type4 data")?;
    if payload_len > MAX_STORED_TYPE4_BITS {
        skip_bits(buffer, payload_len - MAX_STORED_TYPE4_BITS, "type4 truncated data")?;
    }
    Ok(Type4FieldGeneric {
        field_id,
        len: payload_len,
        elems,
        data,
    })
}

fn ensure_absent<T>(field: &'static str, value: &Option<T>) -> Result<(), PduParseErr> {
    if value.is_some() {
        return Err(PduParseErr::Inconsistency {
            field,
            reason: "duplicate type3/type4 element",
        });
    }
    Ok(())
}

pub(super) type DAttachDetachGroupIdentityOptions = (
    Option<Type3FieldGeneric>,
    Option<Type3FieldGeneric>,
    Option<Vec<GroupIdentityDownlink>>,
    Option<Type4FieldGeneric>,
);

pub(super) fn parse_d_attach_detach_group_identity_options(
    obit: bool,
    buffer: &mut BitBuffer,
) -> Result<DAttachDetachGroupIdentityOptions, PduParseErr> {
    let mut proprietary = None;
    let mut group_report_response = None;
    let mut group_identity_downlink = None;
    let mut group_identity_security_related_information = None;

    if !obit {
        return Ok((
            proprietary,
            group_report_response,
            group_identity_downlink,
            group_identity_security_related_information,
        ));
    }

    while delimiters::read_mbit(buffer)? {
        let field_id = buffer.read_field(4, "type34 field_id")?;
        let len_bits = buffer.read_field(11, "type34 len")? as usize;

        match MmType34ElemIdDl::try_from(field_id) {
            Ok(MmType34ElemIdDl::GroupReportResponse) => {
                ensure_absent("group_report_response", &group_report_response)?;
                group_report_response = Some(read_type3_generic_payload(buffer, field_id, len_bits)?);
            }
            Ok(MmType34ElemIdDl::GroupIdentityDownlink) => {
                ensure_absent("group_identity_downlink", &group_identity_downlink)?;
                group_identity_downlink = Some(read_type4_group_identity_downlink_payload(buffer, len_bits)?);
            }
            Ok(MmType34ElemIdDl::GroupIdentitySecurityRelatedInformation) => {
                ensure_absent(
                    "group_identity_security_related_information",
                    &group_identity_security_related_information,
                )?;
                group_identity_security_related_information = Some(read_type4_generic_payload(buffer, field_id, len_bits)?);
            }
            Ok(MmType34ElemIdDl::Proprietary) => {
                ensure_absent("proprietary", &proprietary)?;
                proprietary = Some(read_type3_generic_payload(buffer, field_id, len_bits)?);
            }
            _ => skip_bits(buffer, len_bits, "unknown type3/type4 element")?,
        }
    }

    Ok((
        proprietary,
        group_report_response,
        group_identity_downlink,
        group_identity_security_related_information,
    ))
}

pub(super) type DAttachDetachGroupIdentityAckOptions = (
    Option<Type3FieldGeneric>,
    Option<Vec<GroupIdentityDownlink>>,
    Option<Type4FieldGeneric>,
);

pub(super) fn parse_d_attach_detach_group_identity_ack_options(
    obit: bool,
    buffer: &mut BitBuffer,
) -> Result<DAttachDetachGroupIdentityAckOptions, PduParseErr> {
    let mut proprietary = None;
    let mut group_identity_downlink = None;
    let mut group_identity_security_related_information = None;

    if !obit {
        return Ok((proprietary, group_identity_downlink, group_identity_security_related_information));
    }

    while delimiters::read_mbit(buffer)? {
        let field_id = buffer.read_field(4, "type34 field_id")?;
        let len_bits = buffer.read_field(11, "type34 len")? as usize;

        match MmType34ElemIdDl::try_from(field_id) {
            Ok(MmType34ElemIdDl::GroupIdentityDownlink) => {
                ensure_absent("group_identity_downlink", &group_identity_downlink)?;
                group_identity_downlink = Some(read_type4_group_identity_downlink_payload(buffer, len_bits)?);
            }
            Ok(MmType34ElemIdDl::GroupIdentitySecurityRelatedInformation) => {
                ensure_absent(
                    "group_identity_security_related_information",
                    &group_identity_security_related_information,
                )?;
                group_identity_security_related_information = Some(read_type4_generic_payload(buffer, field_id, len_bits)?);
            }
            Ok(MmType34ElemIdDl::Proprietary) => {
                ensure_absent("proprietary", &proprietary)?;
                proprietary = Some(read_type3_generic_payload(buffer, field_id, len_bits)?);
            }
            _ => skip_bits(buffer, len_bits, "unknown type3/type4 element")?,
        }
    }

    Ok((proprietary, group_identity_downlink, group_identity_security_related_information))
}

pub(super) fn validate_type3_generic_field(
    field: &'static str,
    value: &Option<Type3FieldGeneric>,
    expected_id: u64,
    required_len: Option<usize>,
) -> Result<(), PduParseErr> {
    let Some(elem) = value else {
        return Ok(());
    };

    // Locally constructed tests sometimes leave field_id at zero because the
    // enclosing writer supplies the element id. Any explicit non-zero id must
    // match the PDU table element id.
    if elem.field_id != 0 && elem.field_id != expected_id {
        return Err(PduParseErr::InvalidValue {
            field,
            value: elem.field_id,
        });
    }
    if elem.len == 0 {
        return Err(PduParseErr::InconsistentLength { expected: 1, found: 0 });
    }
    if let Some(required_len) = required_len {
        if elem.len != required_len {
            return Err(PduParseErr::InconsistentLength {
                expected: required_len,
                found: elem.len,
            });
        }
    }
    if elem.len > MAX_STORED_TYPE3_BITS {
        return Err(PduParseErr::NotImplemented { field: Some(field) });
    }
    if elem.len < MAX_STORED_TYPE3_BITS && elem.data >= (1u128 << elem.len) {
        return Err(PduParseErr::InvalidValue {
            field,
            value: invalid_type3_data_value(elem.data),
        });
    }

    Ok(())
}

pub(super) fn validate_group_report_response(value: &Option<Type3FieldGeneric>, expected_id: u64) -> Result<(), PduParseErr> {
    // EN 300 392-2 clause 16.10.27a/table 16.59 defines this Type-3 IE as a
    // one-bit value. The reserved value is left to MM procedure handling so
    // malformed incoming reports can still be rejected with an MM response.
    validate_type3_generic_field("group_report_response", value, expected_id, Some(1))
}

pub(super) fn validate_group_identity_uplink_collection(
    field: &'static str,
    value: &Option<Vec<GroupIdentityUplink>>,
) -> Result<(), PduParseErr> {
    let Some(elems) = value else {
        return Ok(());
    };
    if elems.is_empty() {
        return Err(PduParseErr::InvalidValue { field, value: 0 });
    }
    for elem in elems {
        let mut scratch = BitBuffer::new_autoexpand(96);
        elem.to_bitbuf(&mut scratch)?;
    }
    Ok(())
}

pub(super) fn validate_group_identity_downlink_collection(
    field: &'static str,
    value: &Option<Vec<GroupIdentityDownlink>>,
) -> Result<(), PduParseErr> {
    let Some(elems) = value else {
        return Ok(());
    };
    if elems.is_empty() {
        return Err(PduParseErr::InvalidValue { field, value: 0 });
    }
    for elem in elems {
        let mut scratch = BitBuffer::new_autoexpand(128);
        elem.to_bitbuf(&mut scratch)?;
    }
    Ok(())
}

pub(super) fn validate_type4_generic_field(
    field: &'static str,
    value: &Option<Type4FieldGeneric>,
    expected_id: u64,
) -> Result<(), PduParseErr> {
    let Some(elem) = value else {
        return Ok(());
    };
    if elem.field_id != expected_id {
        return Err(PduParseErr::InvalidValue {
            field,
            value: elem.field_id,
        });
    }
    if elem.len > MAX_STORED_TYPE4_BITS {
        return Err(PduParseErr::NotImplemented { field: Some(field) });
    }
    if elem.len + 6 > MAX_TYPE34_LENGTH_BITS {
        return Err(PduParseErr::InvalidValue {
            field,
            value: elem.len as u64,
        });
    }
    if elem.elems > MAX_TYPE4_ELEMS {
        return Err(PduParseErr::InvalidValue {
            field,
            value: elem.elems as u64,
        });
    }
    if elem.len < MAX_STORED_TYPE4_BITS && elem.data >= (1u64 << elem.len) {
        return Err(PduParseErr::InvalidValue { field, value: elem.data });
    }
    Ok(())
}
