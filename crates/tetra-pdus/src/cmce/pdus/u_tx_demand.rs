// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use crate::cmce::enums::{cmce_pdu_type_ul::CmcePduTypeUl, type3_elem_id::CmceType3ElemId};
use tetra_core::typed_pdu_fields::*;
use tetra_core::{BitBuffer, expect_pdu_type, pdu_parse_error::PduParseErr};

const MAX_U2: u64 = 0x03;
const MAX_U14: u64 = 0x3fff;

/// Representation of the U-TX DEMAND PDU (Clause 14.7.2.12).
/// This PDU shall be the message to the SwMI that a transmission is requested.
/// Response expected: -
/// Response to: D-TX GRANTED

// note 1: This information element is not used in this version of the present document and its value shall be set to "0".
#[derive(Debug)]
pub struct UTxDemand {
    /// Type1, 14 bits, Call identifier
    pub call_identifier: u16,
    /// Type1, 2 bits, TX demand priority
    pub tx_demand_priority: u8,
    /// Type1, 1 bits, Encryption control
    pub encryption_control: bool,
    /// Type1, 1 bits, See note,
    pub reserved: bool,
    /// Type3, Facility
    pub facility: Option<Type3FieldGeneric>,
    /// Type3, DM-MS address
    pub dm_ms_address: Option<Type3FieldGeneric>,
    /// Type3, Proprietary
    pub proprietary: Option<Type3FieldGeneric>,
}

impl UTxDemand {
    fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 clause 14.7.2.12 / table 14.32 fixes the Call
        // identifier to 14 bits and TX demand priority to 2 bits. The table
        // note requires the reserved bit to be set to "0".
        if self.call_identifier as u64 > MAX_U14 {
            return Err(PduParseErr::InvalidValue {
                field: "call_identifier",
                value: self.call_identifier as u64,
            });
        }
        if self.tx_demand_priority as u64 > MAX_U2 {
            return Err(PduParseErr::InvalidValue {
                field: "tx_demand_priority",
                value: self.tx_demand_priority as u64,
            });
        }
        if self.reserved {
            return Err(PduParseErr::InvalidValue {
                field: "reserved",
                value: 1,
            });
        }

        Ok(())
    }

    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(5, "pdu_type")?;
        expect_pdu_type!(pdu_type, CmcePduTypeUl::UTxDemand)?;

        // Type1
        let call_identifier = buffer.read_field(14, "call_identifier")? as u16;
        // Type1
        let tx_demand_priority = buffer.read_field(2, "tx_demand_priority")? as u8;
        // Type1
        let encryption_control = buffer.read_field(1, "encryption_control")? != 0;
        // Type1
        let reserved = buffer.read_field(1, "reserved")? != 0;

        // obit designates presence of any further type2, type3 or type4 fields
        let mut obit = delimiters::read_obit(buffer)?;

        // Type3
        let facility = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::Facility)?;

        // Type3
        let dm_ms_address = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::DmMsAddr)?;

        // Type3
        let proprietary = typed::parse_type3_generic(obit, buffer, CmceType3ElemId::Proprietary)?;

        // Read trailing mbit (if not previously encountered)
        obit = if obit { buffer.read_field(1, "trailing_obit")? == 1 } else { obit };
        if obit {
            return Err(PduParseErr::InvalidTrailingMbitValue);
        }

        Ok(UTxDemand {
            call_identifier,
            tx_demand_priority,
            encryption_control,
            reserved,
            facility,
            dm_ms_address,
            proprietary,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        // PDU Type
        buffer.write_bits(CmcePduTypeUl::UTxDemand.into_raw(), 5);
        // Type1
        buffer.write_bits(self.call_identifier as u64, 14);
        // Type1
        buffer.write_bits(self.tx_demand_priority as u64, 2);
        // Type1
        buffer.write_bits(self.encryption_control as u64, 1);
        // Type1
        buffer.write_bits(self.reserved as u64, 1);

        // Check if any optional field present and place o-bit
        let obit = self.facility.is_some() || self.dm_ms_address.is_some() || self.proprietary.is_some();
        delimiters::write_obit(buffer, obit as u8);
        if !obit {
            return Ok(());
        }

        // Type3
        typed::write_type3_generic(obit, buffer, &self.facility, CmceType3ElemId::Facility)?;

        // Type3
        typed::write_type3_generic(obit, buffer, &self.dm_ms_address, CmceType3ElemId::DmMsAddr)?;

        // Type3
        typed::write_type3_generic(obit, buffer, &self.proprietary, CmceType3ElemId::Proprietary)?;

        // Write terminating m-bit
        delimiters::write_mbit(buffer, 0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_u_tx_demand() -> UTxDemand {
        UTxDemand {
            call_identifier: 0x1234,
            tx_demand_priority: 0x02,
            encryption_control: false,
            reserved: false,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        }
    }

    fn serialize_err(pdu: &UTxDemand) -> PduParseErr {
        let mut buf = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut buf).unwrap_err()
    }

    #[test]
    fn u_tx_demand_rejects_overwide_call_identifier() {
        let pdu = UTxDemand {
            call_identifier: 0x4000,
            ..base_u_tx_demand()
        };

        assert!(matches!(
            serialize_err(&pdu),
            PduParseErr::InvalidValue {
                field: "call_identifier",
                value: 0x4000
            }
        ));
    }

    #[test]
    fn u_tx_demand_rejects_overwide_tx_demand_priority() {
        let pdu = UTxDemand {
            tx_demand_priority: 0x04,
            ..base_u_tx_demand()
        };

        assert!(matches!(
            serialize_err(&pdu),
            PduParseErr::InvalidValue {
                field: "tx_demand_priority",
                value: 0x04
            }
        ));
    }

    #[test]
    fn u_tx_demand_rejects_reserved_bit_set() {
        let pdu = UTxDemand {
            reserved: true,
            ..base_u_tx_demand()
        };

        assert!(matches!(
            serialize_err(&pdu),
            PduParseErr::InvalidValue {
                field: "reserved",
                value: 1
            }
        ));
    }

    #[test]
    fn u_tx_demand_allows_encryption_control_bit_for_peer_handling() {
        let pdu = UTxDemand {
            encryption_control: true,
            ..base_u_tx_demand()
        };
        let mut buf = BitBuffer::new_autoexpand(64);

        pdu.to_bitbuf(&mut buf)
            .expect("encryption control is an on-air bit, not local cipher support");
    }
}

impl fmt::Display for UTxDemand {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "UTxDemand {{ call_identifier: {:?} tx_demand_priority: {:?} encryption_control: {:?} reserved: {:?} facility: {:?} dm_ms_address: {:?} proprietary: {:?} }}",
            self.call_identifier,
            self.tx_demand_priority,
            self.encryption_control,
            self.reserved,
            self.facility,
            self.dm_ms_address,
            self.proprietary,
        )
    }
}
