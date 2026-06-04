use core::fmt;

use tetra_core::{BitBuffer, PduParseErr};
use tetra_saps::control::enums::{circuit_mode_type::CircuitModeType, communication_type::CommunicationType};

/// Clause 14.8.2 Basic service information
#[derive(Debug, Clone)]
pub struct BasicServiceInformation {
    // 3
    pub circuit_mode_type: CircuitModeType,
    // 1, 0 = unencrypted, 1 = E2EE encrypted
    pub encryption_flag: bool,
    // 2, 0 = point-to-point, 1 = point-to-multipoint, 2 = point-to-multipoint acknowledged, 3 = broadcast
    pub communication_type: CommunicationType,
    /// 2 opt, 0 = 1 slot, 1 = 2 slots, 2 = 3 slots, 3 = 4 slots
    pub slots_per_frame: Option<u8>,
    /// 2 opt, 00 = TETRA encoded speech, 1|2 = reserved, 3 = proprietary
    pub speech_service: Option<u8>,
}

impl BasicServiceInformation {
    pub fn validate(&self) -> Result<(), PduParseErr> {
        // EN 300 392-2 clause 14.8.2 encodes the final Basic service
        // information subfield on two bits: Speech service for TCH/S, or
        // Slots per frame for the other circuit-mode bearer types.
        match (self.circuit_mode_type, self.slots_per_frame, self.speech_service) {
            (CircuitModeType::TchS, None, Some(speech_service)) if speech_service <= 0x03 => Ok(()),
            (CircuitModeType::TchS, None, Some(speech_service)) => Err(PduParseErr::InvalidValue {
                field: "speech_service",
                value: speech_service as u64,
            }),
            (CircuitModeType::TchS, Some(_), _) | (CircuitModeType::TchS, _, None) => Err(PduParseErr::InvalidValue {
                field: "circuit_mode_type",
                value: self.circuit_mode_type as u64,
            }),
            (_, Some(slots_per_frame), None) if slots_per_frame <= 0x03 => Ok(()),
            (_, Some(slots_per_frame), None) => Err(PduParseErr::InvalidValue {
                field: "slots_per_frame",
                value: slots_per_frame as u64,
            }),
            (_, _, _) => Err(PduParseErr::InvalidValue {
                field: "circuit_mode_type",
                value: self.circuit_mode_type as u64,
            }),
        }
    }

    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let val = buffer.read_field(3, "circuit_mode_type")?;
        let circuit_mode_type = CircuitModeType::try_from(val).unwrap(); // Never fails

        let encryption_flag = buffer.read_field(1, "encryption_flag")? != 0;
        let val = buffer.read_field(2, "communication_type")?;
        let communication_type = CommunicationType::try_from(val).unwrap(); // Never fails

        let (speech_service, slots_per_frame) = match circuit_mode_type {
            CircuitModeType::TchS => {
                let speech_service = buffer.read_field(2, "speech_service")? as u8;
                (Some(speech_service), None)
            }
            _ => {
                let slots_per_frame = buffer.read_field(2, "slots_per_frame")? as u8;
                (None, Some(slots_per_frame))
            }
        };

        let s = BasicServiceInformation {
            circuit_mode_type,
            encryption_flag,
            communication_type,
            slots_per_frame,
            speech_service,
        };

        Ok(s)
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        self.validate()?;

        buf.write_bits(self.circuit_mode_type as u64, 3);
        buf.write_bits(self.encryption_flag as u64, 1);
        buf.write_bits(self.communication_type as u64, 2);

        if let Some(v) = self.slots_per_frame {
            buf.write_bits(v as u64, 2);
        }
        if let Some(v) = self.speech_service {
            buf.write_bits(v as u64, 2);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speech_bsi(speech_service: u8) -> BasicServiceInformation {
        BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2p,
            slots_per_frame: None,
            speech_service: Some(speech_service),
        }
    }

    fn data_bsi(slots_per_frame: u8) -> BasicServiceInformation {
        BasicServiceInformation {
            circuit_mode_type: CircuitModeType::Tch72,
            encryption_flag: false,
            communication_type: CommunicationType::P2p,
            slots_per_frame: Some(slots_per_frame),
            speech_service: None,
        }
    }

    fn serialize_err(bsi: &BasicServiceInformation) -> PduParseErr {
        let mut buf = BitBuffer::new_autoexpand(8);
        bsi.to_bitbuf(&mut buf).unwrap_err()
    }

    #[test]
    fn basic_service_information_rejects_overwide_speech_service() {
        assert!(matches!(
            serialize_err(&speech_bsi(4)),
            PduParseErr::InvalidValue {
                field: "speech_service",
                value: 4
            }
        ));
    }

    #[test]
    fn basic_service_information_rejects_overwide_slots_per_frame() {
        assert!(matches!(
            serialize_err(&data_bsi(4)),
            PduParseErr::InvalidValue {
                field: "slots_per_frame",
                value: 4
            }
        ));
    }
}

impl fmt::Display for BasicServiceInformation {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "BasicServiceInformation {{ circuit_mode_type: {:?} encryption_flag: {:?} communication_type: {:?} slots_per_frame: {:?} speech_service: {:?} }}",
            self.circuit_mode_type, self.encryption_flag, self.communication_type, self.slots_per_frame, self.speech_service,
        )
    }
}
