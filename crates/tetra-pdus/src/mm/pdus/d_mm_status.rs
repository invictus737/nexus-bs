// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use core::fmt;

use tetra_core::expect_pdu_type;
use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mm::enums::mm_pdu_type_dl::MmPduTypeDl;
use crate::mm::enums::status_downlink::StatusDownlink;
use crate::mm::fields::energy_saving_information::EnergySavingInformation;

/// Representation of the D-MM STATUS PDU (Clause 16.9.2.5.1).
/// The infrastructure sends this message to the MS to request or indicate/reject a change of an operation mode.
/// Response expected: -/U-MM STATUS
/// Response to: -/U-MM STATUS

// note 1: This information element shall indicate the requested service or a response to a request and the sub-type of the D-MM STATUS PDU.
// note 2: This information element or set of information elements shall be as defined by the status downlink information element, refer to clauses 16.9.2.5.1 to 16.9.2.5.7.
// note 3: This Status downlink element indicates which sub-PDU this D-MM STATUS PDU contains. If the receiving party does not support the indicated function but recognizes the PDU structure, it should set the value to Not-supported sub-PDU type element.
#[derive(Debug)]
pub struct DMmStatus {
    /// Type1, 6 bits, See notes 1 and 3,
    pub status_downlink: StatusDownlink,
    /// Energy saving information, present for ChangeOfEnergySavingModeRequest/Response
    pub energy_saving_information: Option<EnergySavingInformation>,
}

impl DMmStatus {
    /// Parse from BitBuffer
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let pdu_type = buffer.read_field(4, "pdu_type")?;
        expect_pdu_type!(pdu_type, MmPduTypeDl::DMmStatus)?;

        // Type1
        let val = buffer.read_field(6, "status_downlink")?;
        let status_downlink = StatusDownlink::try_from(val).map_err(|_| PduParseErr::InvalidValue {
            field: "status_downlink",
            value: val,
        })?;

        let energy_saving_information = match status_downlink {
            StatusDownlink::ChangeOfEnergySavingModeRequest | StatusDownlink::ChangeOfEnergySavingModeResponse => {
                Some(EnergySavingInformation::from_bitbuf(buffer)?)
            }
            _ => {
                return Err(PduParseErr::NotImplemented {
                    field: Some("status_downlink_dependent_information"),
                });
            }
        };

        Ok(DMmStatus {
            status_downlink,
            energy_saving_information,
        })
    }

    /// Serialize this PDU into the given BitBuffer.
    pub fn to_bitbuf(&self, buffer: &mut BitBuffer) -> Result<(), PduParseErr> {
        // PDU Type
        buffer.write_bits(MmPduTypeDl::DMmStatus.into_raw(), 4);
        // Type1
        buffer.write_bits(self.status_downlink.into_raw(), 6);

        match self.status_downlink {
            StatusDownlink::ChangeOfEnergySavingModeRequest | StatusDownlink::ChangeOfEnergySavingModeResponse => {
                if let Some(ref esi) = self.energy_saving_information {
                    esi.to_bitbuf(buffer)?;
                } else {
                    return Err(PduParseErr::FieldNotPresent {
                        field: Some("energy_saving_information"),
                    });
                }
            }
            _ => {
                return Err(PduParseErr::NotImplemented {
                    field: Some("status_downlink_dependent_information"),
                });
            }
        }

        Ok(())
    }
}

impl fmt::Display for DMmStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "DMmStatus {{ status_downlink: {} energy_saving_information: {:?} }}",
            self.status_downlink, self.energy_saving_information,
        )
    }
}

#[cfg(test)]
mod tests {
    use tetra_core::debug;

    use super::*;

    #[test]
    fn test_d_mm_status_unsupported_sub_pdu_returns_not_implemented() {
        debug::setup_logging_verbose();

        // EN 300 392-2 clause 16.9.2.5.1 table 16.3: Status downlink selects
        // the sub-PDU. This stack currently implements only energy-saving
        // D-MM STATUS sub-PDUs, so recognized but unsupported selectors must
        // fail as typed parser errors rather than panic.
        let mut buf = BitBuffer::from_bitstr("1100000011");
        assert!(matches!(
            DMmStatus::from_bitbuf(&mut buf),
            Err(PduParseErr::NotImplemented {
                field: Some("status_downlink_dependent_information")
            })
        ));
    }

    #[test]
    fn test_d_mm_status_unsupported_sub_pdu_serializer_returns_not_implemented() {
        let pdu = DMmStatus {
            status_downlink: StatusDownlink::DualWatchModeResponse,
            energy_saving_information: None,
        };
        let mut buf = BitBuffer::new_autoexpand(16);

        assert!(matches!(
            pdu.to_bitbuf(&mut buf),
            Err(PduParseErr::NotImplemented {
                field: Some("status_downlink_dependent_information")
            })
        ));
    }
}
