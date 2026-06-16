// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original LLC Advanced Link PDU support.

use core::fmt;

use tetra_core::pdu_parse_error::*;
use tetra_core::{BitBuffer, expect_value, let_field};

/// EN 300 392-2 clause 21.2.3.5 AL-SETUP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlSetup {
    pub acknowledged_service: bool,
    /// Raw over-air advanced link number field: 0..3 means AL number 1..4.
    pub advanced_link_number: u8,
    pub max_tl_sdu_len_code: u8,
    pub connection_width: bool,
    pub advanced_link_symmetry: bool,
    pub uplink_timeslots: Option<u8>,
    pub downlink_timeslots: Option<u8>,
    pub throughput_code: u8,
    pub window_size_code: u8,
    pub max_tl_sdu_retransmissions: u8,
    pub max_segment_retransmissions: u8,
    pub setup_report: u8,
    pub ns: Option<u8>,
    pub augmented: Option<AlSetupAugmented>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlSetupAugmented {
    pub extended_advanced_link: bool,
    pub original_window_size_code: Option<u8>,
    pub extended_window_size_code: Option<u8>,
    pub reserved: u8,
}

impl AlSetup {
    pub const SETUP_REPORT_SUCCESS: u8 = 0;
    pub const SETUP_REPORT_SERVICE_DEFINITION: u8 = 1;
    pub const SETUP_REPORT_SERVICE_CHANGE: u8 = 2;
    pub const SETUP_REPORT_RESET: u8 = 3;
    pub const SETUP_REPORT_SUCCESS_QOS_INCOMPLETE: u8 = 4;

    pub fn from_bitbuf(buf: &mut BitBuffer) -> Result<Self, PduParseErr> {
        let_field!(buf, llc_pdu_type, 4);
        expect_value!(llc_pdu_type, 8)?;
        let_field!(buf, advanced_link_service, 1);
        let_field!(buf, advanced_link_number, 2);
        let_field!(buf, max_tl_sdu_len_code, 3);
        let_field!(buf, connection_width, 1);
        let_field!(buf, advanced_link_symmetry, 1);

        let uplink_timeslots = if connection_width != 0 {
            let_field!(buf, uplink_timeslots, 2);
            Some(uplink_timeslots as u8)
        } else {
            None
        };
        let downlink_timeslots = if connection_width != 0 && advanced_link_symmetry != 0 {
            let_field!(buf, downlink_timeslots, 2);
            Some(downlink_timeslots as u8)
        } else {
            None
        };

        let_field!(buf, throughput_code, 3);
        let_field!(buf, window_size_code, 2);
        let_field!(buf, max_tl_sdu_retransmissions, 3);
        let_field!(buf, max_segment_retransmissions, 4);
        let_field!(buf, setup_report, 3);

        let ns = if advanced_link_service == 0 {
            let_field!(buf, ns, 8);
            Some(ns as u8)
        } else {
            None
        };

        let augmented = if window_size_code == 0 {
            let_field!(buf, advanced_link_type, 1);
            let extended_advanced_link = advanced_link_type != 0;
            let (original_window_size_code, extended_window_size_code) = if extended_advanced_link {
                let_field!(buf, extended_window_size_code, 4);
                (None, Some(extended_window_size_code as u8))
            } else {
                let_field!(buf, original_window_size_code, 2);
                (Some(original_window_size_code as u8), None)
            };
            let_field!(buf, reserved, 3);
            Some(AlSetupAugmented {
                extended_advanced_link,
                original_window_size_code,
                extended_window_size_code,
                reserved: reserved as u8,
            })
        } else {
            None
        };

        Ok(Self {
            acknowledged_service: advanced_link_service != 0,
            advanced_link_number: advanced_link_number as u8,
            max_tl_sdu_len_code: max_tl_sdu_len_code as u8,
            connection_width: connection_width != 0,
            advanced_link_symmetry: advanced_link_symmetry != 0,
            uplink_timeslots,
            downlink_timeslots,
            throughput_code: throughput_code as u8,
            window_size_code: window_size_code as u8,
            max_tl_sdu_retransmissions: max_tl_sdu_retransmissions as u8,
            max_segment_retransmissions: max_segment_retransmissions as u8,
            setup_report: setup_report as u8,
            ns,
            augmented,
        })
    }

    pub fn to_bitbuf(&self, buf: &mut BitBuffer) {
        buf.write_bits(8, 4);
        buf.write_bits(self.acknowledged_service as u64, 1);
        buf.write_bits(self.advanced_link_number as u64, 2);
        buf.write_bits(self.max_tl_sdu_len_code as u64, 3);
        buf.write_bits(self.connection_width as u64, 1);
        buf.write_bits(self.advanced_link_symmetry as u64, 1);
        if self.connection_width {
            buf.write_bits(self.uplink_timeslots.unwrap_or(0) as u64, 2);
        }
        if self.connection_width && self.advanced_link_symmetry {
            buf.write_bits(self.downlink_timeslots.unwrap_or(0) as u64, 2);
        }
        buf.write_bits(self.throughput_code as u64, 3);
        buf.write_bits(self.window_size_code as u64, 2);
        buf.write_bits(self.max_tl_sdu_retransmissions as u64, 3);
        buf.write_bits(self.max_segment_retransmissions as u64, 4);
        buf.write_bits(self.setup_report as u64, 3);
        if !self.acknowledged_service {
            buf.write_bits(self.ns.unwrap_or(0) as u64, 8);
        }
        if let Some(augmented) = self.augmented {
            buf.write_bits(augmented.extended_advanced_link as u64, 1);
            if augmented.extended_advanced_link {
                buf.write_bits(augmented.extended_window_size_code.unwrap_or(1) as u64, 4);
            } else {
                buf.write_bits(augmented.original_window_size_code.unwrap_or(1) as u64, 2);
            }
            buf.write_bits(augmented.reserved as u64, 3);
        }
    }

    pub fn response_success(&self) -> Self {
        let mut response = *self;
        response.setup_report = Self::SETUP_REPORT_SUCCESS;
        response
    }

    pub fn response_with_lower_phase_mod_timeslots(&self, max_timeslots: u8) -> Self {
        let mut response = *self;
        let max_code = max_timeslots.clamp(1, 4) - 1;

        if response.connection_width {
            response.uplink_timeslots = response.uplink_timeslots.map(|timeslots| timeslots.min(max_code));
            if response.advanced_link_symmetry {
                let requested_downlink = response.downlink_timeslots.unwrap_or(response.uplink_timeslots.unwrap_or(max_code));
                response.downlink_timeslots = Some(requested_downlink.min(max_code));
            }
        }

        response.setup_report = if response != *self {
            Self::SETUP_REPORT_SERVICE_CHANGE
        } else {
            Self::SETUP_REPORT_SUCCESS
        };
        response
    }

    pub fn response_with_service_change(&self) -> Self {
        let mut response = *self;
        response.setup_report = Self::SETUP_REPORT_SERVICE_CHANGE;
        response
    }

    pub fn is_original_acknowledged_non_augmented(&self) -> bool {
        self.acknowledged_service && self.augmented.is_none()
    }

    pub fn link_id(&self) -> u32 {
        u32::from(self.advanced_link_number) + 1
    }
}

impl fmt::Display for AlSetup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "al_setup {{ ack: {}, al: {}, max_len: {}, width: {}, sym: {}, throughput: {}, window: {}, n273: {}, n274: {}, report: {}, augmented: {} }}",
            self.acknowledged_service,
            self.advanced_link_number + 1,
            self.max_tl_sdu_len_code,
            self.connection_width,
            self.advanced_link_symmetry,
            self.throughput_code,
            self.window_size_code,
            self.max_tl_sdu_retransmissions,
            self.max_segment_retransmissions,
            self.setup_report,
            self.augmented.is_some()
        )
    }
}
