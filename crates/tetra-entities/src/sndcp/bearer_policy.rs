// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original TETRA SNDCP lower-bearer selection policy primitives.

use tetra_core::Layer2Service;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpPacketDataClass {
    Background,
    Telemetry,
    RealTime,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SndcpPacketDataBearerProfile {
    pub qos_negotiated: bool,
    pub data_class: SndcpPacketDataClass,
    pub unacknowledged_basic_link_repetitions: u8,
    pub fcs_required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SndcpResolvedLowerBearer {
    pub layer2service: Layer2Service,
    pub unacked_bl_repetitions: Option<u8>,
    pub fcs_flag: bool,
}

impl Default for SndcpPacketDataBearerProfile {
    fn default() -> Self {
        Self {
            qos_negotiated: false,
            data_class: SndcpPacketDataClass::Background,
            unacknowledged_basic_link_repetitions: 0,
            fcs_required: false,
        }
    }
}

impl SndcpPacketDataBearerProfile {
    pub fn background_default() -> Self {
        Self::default()
    }

    pub fn negotiated_realtime_unacknowledged(repetitions: u8, fcs_required: bool) -> Self {
        Self {
            qos_negotiated: true,
            data_class: SndcpPacketDataClass::RealTime,
            unacknowledged_basic_link_repetitions: repetitions,
            fcs_required,
        }
    }

    pub fn resolve_swmi_unitdata_downlink(self) -> SndcpResolvedLowerBearer {
        match (self.qos_negotiated, self.data_class) {
            (true, SndcpPacketDataClass::RealTime) => SndcpResolvedLowerBearer {
                layer2service: Layer2Service::Unacknowledged,
                unacked_bl_repetitions: Some(self.unacknowledged_basic_link_repetitions),
                fcs_flag: self.fcs_required,
            },
            _ => SndcpResolvedLowerBearer {
                layer2service: Layer2Service::Unacknowledged,
                unacked_bl_repetitions: Some(0),
                fcs_flag: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_background_without_qos_uses_unacknowledged_unitdata_bearer() {
        let resolved = SndcpPacketDataBearerProfile::background_default().resolve_swmi_unitdata_downlink();

        assert_eq!(resolved.layer2service, Layer2Service::Unacknowledged);
        assert_eq!(resolved.unacked_bl_repetitions, Some(0));
        assert!(!resolved.fcs_flag);
    }

    #[test]
    fn telemetry_without_negotiated_realtime_qos_stays_on_unacknowledged_unitdata_bearer() {
        let resolved = SndcpPacketDataBearerProfile {
            qos_negotiated: false,
            data_class: SndcpPacketDataClass::Telemetry,
            unacknowledged_basic_link_repetitions: 3,
            fcs_required: true,
        }
        .resolve_swmi_unitdata_downlink();

        assert_eq!(resolved.layer2service, Layer2Service::Unacknowledged);
        assert_eq!(resolved.unacked_bl_repetitions, Some(0));
        assert!(!resolved.fcs_flag);
    }

    #[test]
    fn realtime_with_negotiated_qos_uses_unacknowledged_basic_link_parameters() {
        let resolved = SndcpPacketDataBearerProfile::negotiated_realtime_unacknowledged(2, true).resolve_swmi_unitdata_downlink();

        assert_eq!(resolved.layer2service, Layer2Service::Unacknowledged);
        assert_eq!(resolved.unacked_bl_repetitions, Some(2));
        assert!(resolved.fcs_flag);
    }
}
