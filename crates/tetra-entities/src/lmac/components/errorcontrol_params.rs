use tetra_saps::tmv::enums::logical_chans::LogicalChannel;

/// Each LogicalChannel is associated with a set of error control parameters.
#[derive(Debug)]
pub struct ErrorControlParams {
    // pub name:           &'static str,
    pub type345_bits: usize,
    pub type2_bits: usize,
    pub type1_bits: usize,
    pub interleave_a: usize,
    pub have_crc16: bool,
}

/// Parameters for the BSCH (Broadcast Synchronization Channel)
pub const BSCH_PARAMS: ErrorControlParams = ErrorControlParams {
    type345_bits: 120,
    type2_bits: 80,
    type1_bits: 60,
    interleave_a: 11,
    have_crc16: true,
};

/// Parameters for the SCH/HD (half slot) signalling channel, also for STCH and BNCH
pub const SCH_HD_PARAMS: ErrorControlParams = ErrorControlParams {
    type345_bits: 216,
    type2_bits: 144,
    type1_bits: 124,
    interleave_a: 101,
    have_crc16: true,
};

/// Parameters for the BBK (Broadcast Block) channel, used for AACH
pub const AACH_PARAMS: ErrorControlParams = ErrorControlParams {
    type345_bits: 30,
    type2_bits: 30,
    type1_bits: 14,
    interleave_a: 0, // No interleaving
    have_crc16: false,
};

/// Parameters for the SCH/F channel
pub const SCH_F_PARAMS: ErrorControlParams = ErrorControlParams {
    type345_bits: 432,
    type2_bits: 288,
    type1_bits: 268,
    interleave_a: 103,
    have_crc16: true,
};

/// Parameters for the SCH/HU (half slot uplink, Control Uplink Burst) channel
pub const SCH_HU_PARAMS: ErrorControlParams = ErrorControlParams {
    type345_bits: 168,
    type2_bits: 112,
    type1_bits: 92,
    interleave_a: 13,
    have_crc16: true,
};

/// Parameters for TCH/S (full-rate 7.2 kbit/s speech).
/// 274 bits + 4 tail + 10 padding = 288 type-2 bits. No CRC.
pub const TCH_S_PARAMS: ErrorControlParams = ErrorControlParams {
    type345_bits: 432,
    type2_bits: 288,
    type1_bits: 274,
    interleave_a: 103,
    have_crc16: false,
};

/// Gets implemented error-control parameters for a given logical channel.
pub fn get_params(lchan: LogicalChannel) -> Option<&'static ErrorControlParams> {
    match lchan {
        LogicalChannel::Bsch => Some(&BSCH_PARAMS),
        LogicalChannel::SchHd | LogicalChannel::Stch | LogicalChannel::Bnch => Some(&SCH_HD_PARAMS),
        LogicalChannel::Aach => Some(&AACH_PARAMS),
        LogicalChannel::SchF => Some(&SCH_F_PARAMS),
        LogicalChannel::SchHu => Some(&SCH_HU_PARAMS),
        LogicalChannel::TchS => Some(&TCH_S_PARAMS),

        // EN 300 392-2 defines these as distinct logical channels, but this
        // stack does not implement their channel coding paths yet. Return
        // None so LMAC can fail closed instead of panicking or reusing the
        // wrong TCH/S speech coding.
        LogicalChannel::Tch24 | LogicalChannel::Tch48 | LogicalChannel::Tch72 => None,

        // Linearization channels are not ordinary control-plane coding paths.
        LogicalChannel::Blch | LogicalChannel::Clch => None,
    }
}
