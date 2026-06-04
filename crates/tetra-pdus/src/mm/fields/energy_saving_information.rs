use core::fmt;

use tetra_core::{BitBuffer, pdu_parse_error::PduParseErr};

use crate::mm::enums::energy_saving_mode::EnergySavingMode;

/// 16.10.10 Energy saving information

#[derive(Debug, Clone)]
pub struct EnergySavingInformation {
    // 3
    pub energy_saving_mode: EnergySavingMode,
    // 5, when energy saving mode is "Stay alive" this field has no meaning and is set to 0
    pub frame_number: Option<u8>,
    // 6, when energy saving mode is "Stay alive" this field has no meaning and is set to 0
    pub multiframe_number: Option<u8>,
}

impl EnergySavingInformation {
    pub fn from_bitbuf(buffer: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let val = buffer.read_field(3, "energy_saving_mode")? as u8;
        let energy_saving_mode = EnergySavingMode::try_from(val as u64).unwrap(); // Never fails

        let fn_val = buffer.read_field(5, "frame_number")? as u8;
        let mn_val = buffer.read_field(6, "multiframe_number")? as u8;

        // For StayAlive the spec says frame/multiframe fields "have no meaning";
        // parse them to advance the buffer, then discard.
        let (f, m) = if energy_saving_mode == EnergySavingMode::StayAlive {
            if fn_val != 0 {
                return Err(PduParseErr::InvalidValue {
                    field: "frame_number",
                    value: fn_val as u64,
                });
            }
            if mn_val != 0 {
                return Err(PduParseErr::InvalidValue {
                    field: "multiframe_number",
                    value: mn_val as u64,
                });
            }
            (None, None)
        } else {
            Self::validate_start_point(fn_val, mn_val)?;
            (Some(fn_val), Some(mn_val))
        };

        let s = EnergySavingInformation {
            energy_saving_mode,
            frame_number: f,
            multiframe_number: m,
        };

        Ok(s)
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) -> Result<(), PduParseErr> {
        buf.write_bits(self.energy_saving_mode as u64, 3);

        // Sanity check
        if self.energy_saving_mode == EnergySavingMode::StayAlive {
            if let Some(f) = self.frame_number {
                return Err(PduParseErr::InvalidValue {
                    field: "frame_number",
                    value: f as u64,
                });
            }
            if let Some(f) = self.multiframe_number {
                return Err(PduParseErr::InvalidValue {
                    field: "multiframe_number",
                    value: f as u64,
                });
            }
            buf.write_bits(0, 5 + 6);
        } else {
            if let Some(f) = self.frame_number {
                if !(1..=18).contains(&f) {
                    return Err(PduParseErr::InvalidValue {
                        field: "frame_number",
                        value: f as u64,
                    });
                }
                buf.write_bits(f as u64, 5);
            } else {
                return Err(PduParseErr::FieldNotPresent {
                    field: Some("frame_number"),
                });
            }
            if let Some(f) = self.multiframe_number {
                if !(1..=60).contains(&f) {
                    return Err(PduParseErr::InvalidValue {
                        field: "multiframe_number",
                        value: f as u64,
                    });
                }
                buf.write_bits(f as u64, 6);
            } else {
                return Err(PduParseErr::FieldNotPresent {
                    field: Some("multiframe_number"),
                });
            }
        }

        Ok(())
    }

    fn validate_start_point(frame_number: u8, multiframe_number: u8) -> Result<(), PduParseErr> {
        if !(1..=18).contains(&frame_number) {
            return Err(PduParseErr::InvalidValue {
                field: "frame_number",
                value: frame_number as u64,
            });
        }
        if !(1..=60).contains(&multiframe_number) {
            return Err(PduParseErr::InvalidValue {
                field: "multiframe_number",
                value: multiframe_number as u64,
            });
        }
        Ok(())
    }
}

impl fmt::Display for EnergySavingInformation {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "EnergySavingInformation {{ energy_saving_mode: {:?} frame_number: {:?} multiframe_number: {:?} }}",
            self.energy_saving_mode, self.frame_number, self.multiframe_number,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_energy_saving_information_accepts_valid_eg_start_point() {
        let mut buf = BitBuffer::from_bitstr("00100001000001");
        let esi = EnergySavingInformation::from_bitbuf(&mut buf).expect("valid EG1 start point should parse");

        assert_eq!(esi.energy_saving_mode, EnergySavingMode::Eg1);
        assert_eq!(esi.frame_number, Some(1));
        assert_eq!(esi.multiframe_number, Some(1));
        assert_eq!(buf.get_len_remaining(), 0);

        let mut out = BitBuffer::new_autoexpand(14);
        esi.to_bitbuf(&mut out).unwrap();
        assert_eq!(out.to_bitstr(), "00100001000001");
    }

    #[test]
    fn test_energy_saving_information_rejects_invalid_eg_start_point() {
        for bitstr in ["00100000000001", "00110011000001", "00100001000000", "00100001111101"] {
            let mut buf = BitBuffer::from_bitstr(bitstr);
            assert!(EnergySavingInformation::from_bitbuf(&mut buf).is_err());
        }
    }

    #[test]
    fn test_energy_saving_information_rejects_stay_alive_nonzero_start_fields() {
        for bitstr in ["00000001000000", "00000000000001"] {
            let mut buf = BitBuffer::from_bitstr(bitstr);
            assert!(EnergySavingInformation::from_bitbuf(&mut buf).is_err());
        }
    }

    #[test]
    fn test_energy_saving_information_serializer_rejects_invalid_eg_start_point() {
        for esi in [
            EnergySavingInformation {
                energy_saving_mode: EnergySavingMode::Eg1,
                frame_number: Some(0),
                multiframe_number: Some(1),
            },
            EnergySavingInformation {
                energy_saving_mode: EnergySavingMode::Eg1,
                frame_number: Some(19),
                multiframe_number: Some(1),
            },
            EnergySavingInformation {
                energy_saving_mode: EnergySavingMode::Eg1,
                frame_number: Some(1),
                multiframe_number: Some(0),
            },
            EnergySavingInformation {
                energy_saving_mode: EnergySavingMode::Eg1,
                frame_number: Some(1),
                multiframe_number: Some(61),
            },
        ] {
            let mut out = BitBuffer::new_autoexpand(14);
            assert!(esi.to_bitbuf(&mut out).is_err());
        }
    }
}
