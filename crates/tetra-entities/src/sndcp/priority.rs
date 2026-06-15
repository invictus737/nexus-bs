// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original TETRA SNDCP packet-data priority policy primitives.

pub const SNDCP_UNDEFINED_DATA_PRIORITY_FALLBACK: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpDataScheduling {
    NonScheduled,
    InitialScheduled,
    Scheduled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SndcpPriorityPolicy {
    pub pdu_priority_max: u8,
    pub sn_sap_pdu_priority: Option<u8>,
    pub sn_sap_data_priority: Option<u8>,
    pub nsapi_data_priority: Option<u8>,
    pub ms_default_data_priority: Option<u8>,
    pub scheduling: SndcpDataScheduling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SndcpResolvedPriority {
    pub pdu_priority: u8,
    pub data_priority: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SndcpPriorityError {
    PduPriorityMaxOutOfRange(u8),
    SnSapPduPriorityOutOfRange(u8),
    SnSapDataPriorityOutOfRange(u8),
    NsapiDataPriorityOutOfRange(u8),
    MsDefaultDataPriorityOutOfRange(u8),
}

impl SndcpPriorityPolicy {
    pub fn packet_data(pdu_priority_max: u8) -> Self {
        Self {
            pdu_priority_max,
            sn_sap_pdu_priority: None,
            sn_sap_data_priority: None,
            nsapi_data_priority: None,
            ms_default_data_priority: None,
            scheduling: SndcpDataScheduling::NonScheduled,
        }
    }

    pub fn with_sn_sap_pdu_priority(mut self, priority: Option<u8>) -> Self {
        self.sn_sap_pdu_priority = priority;
        self
    }

    pub fn with_sn_sap_data_priority(mut self, priority: Option<u8>) -> Self {
        self.sn_sap_data_priority = priority;
        self
    }

    pub fn with_nsapi_data_priority(mut self, priority: Option<u8>) -> Self {
        self.nsapi_data_priority = priority;
        self
    }

    pub fn with_ms_default_data_priority(mut self, priority: Option<u8>) -> Self {
        self.ms_default_data_priority = priority;
        self
    }

    pub fn with_scheduling(mut self, scheduling: SndcpDataScheduling) -> Self {
        self.scheduling = scheduling;
        self
    }

    pub fn resolve_unitdata(self) -> Result<SndcpResolvedPriority, SndcpPriorityError> {
        validate_priority(self.pdu_priority_max).map_err(|_| SndcpPriorityError::PduPriorityMaxOutOfRange(self.pdu_priority_max))?;
        validate_optional_priority(self.sn_sap_pdu_priority, SndcpPriorityError::SnSapPduPriorityOutOfRange)?;
        validate_optional_priority(self.sn_sap_data_priority, SndcpPriorityError::SnSapDataPriorityOutOfRange)?;
        validate_optional_priority(self.nsapi_data_priority, SndcpPriorityError::NsapiDataPriorityOutOfRange)?;
        validate_optional_priority(self.ms_default_data_priority, SndcpPriorityError::MsDefaultDataPriorityOutOfRange)?;

        Ok(SndcpResolvedPriority {
            pdu_priority: self
                .sn_sap_pdu_priority
                .map(|priority| priority.min(self.pdu_priority_max))
                .unwrap_or(self.pdu_priority_max),
            data_priority: self.resolve_unitdata_data_priority(),
        })
    }

    fn resolve_unitdata_data_priority(self) -> Option<u8> {
        match self.scheduling {
            SndcpDataScheduling::Scheduled => None,
            SndcpDataScheduling::NonScheduled | SndcpDataScheduling::InitialScheduled => Some(
                self.sn_sap_data_priority
                    .or(self.nsapi_data_priority)
                    .or(self.ms_default_data_priority)
                    .unwrap_or(SNDCP_UNDEFINED_DATA_PRIORITY_FALLBACK),
            ),
        }
    }
}

fn validate_optional_priority(value: Option<u8>, error: fn(u8) -> SndcpPriorityError) -> Result<(), SndcpPriorityError> {
    if let Some(value) = value {
        validate_priority(value).map_err(|_| error(value))?;
    }
    Ok(())
}

fn validate_priority(value: u8) -> Result<(), ()> {
    if value <= 7 { Ok(()) } else { Err(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sn_sap_pdu_priority_is_capped_by_pdp_context_max() {
        let resolved = SndcpPriorityPolicy::packet_data(3)
            .with_sn_sap_pdu_priority(Some(7))
            .resolve_unitdata()
            .expect("valid priorities should resolve");

        assert_eq!(resolved.pdu_priority, 3);
        assert_eq!(resolved.data_priority, Some(SNDCP_UNDEFINED_DATA_PRIORITY_FALLBACK));
    }

    #[test]
    fn missing_sn_sap_pdu_priority_uses_pdp_context_max() {
        let resolved = SndcpPriorityPolicy::packet_data(5)
            .resolve_unitdata()
            .expect("valid priorities should resolve");

        assert_eq!(resolved.pdu_priority, 5);
    }

    #[test]
    fn sn_sap_data_priority_wins_for_non_scheduled_unitdata() {
        let resolved = SndcpPriorityPolicy::packet_data(4)
            .with_sn_sap_data_priority(Some(6))
            .with_nsapi_data_priority(Some(3))
            .with_ms_default_data_priority(Some(1))
            .resolve_unitdata()
            .expect("valid priorities should resolve");

        assert_eq!(resolved.data_priority, Some(6));
    }

    #[test]
    fn nsapi_data_priority_wins_over_ms_default_when_sn_sap_omits_it() {
        let resolved = SndcpPriorityPolicy::packet_data(4)
            .with_nsapi_data_priority(Some(3))
            .with_ms_default_data_priority(Some(1))
            .resolve_unitdata()
            .expect("valid priorities should resolve");

        assert_eq!(resolved.data_priority, Some(3));
    }

    #[test]
    fn ms_default_priority_is_used_when_sn_sap_and_nsapi_are_undefined() {
        let resolved = SndcpPriorityPolicy::packet_data(4)
            .with_ms_default_data_priority(Some(1))
            .resolve_unitdata()
            .expect("valid priorities should resolve");

        assert_eq!(resolved.data_priority, Some(1));
    }

    #[test]
    fn non_scheduled_unitdata_never_resolves_to_undefined_priority() {
        let resolved = SndcpPriorityPolicy::packet_data(4)
            .resolve_unitdata()
            .expect("valid priorities should resolve");

        assert_eq!(resolved.data_priority, Some(SNDCP_UNDEFINED_DATA_PRIORITY_FALLBACK));
    }

    #[test]
    fn scheduled_unitdata_priority_is_undefined_even_with_sn_sap_priority() {
        let resolved = SndcpPriorityPolicy::packet_data(4)
            .with_sn_sap_data_priority(Some(7))
            .with_scheduling(SndcpDataScheduling::Scheduled)
            .resolve_unitdata()
            .expect("valid priorities should resolve");

        assert_eq!(resolved.data_priority, None);
    }

    #[test]
    fn initial_scheduled_unitdata_may_use_non_scheduled_priority_resolution() {
        let resolved = SndcpPriorityPolicy::packet_data(4)
            .with_ms_default_data_priority(Some(2))
            .with_scheduling(SndcpDataScheduling::InitialScheduled)
            .resolve_unitdata()
            .expect("valid priorities should resolve");

        assert_eq!(resolved.data_priority, Some(2));
    }

    #[test]
    fn out_of_range_priorities_are_rejected() {
        assert_eq!(
            SndcpPriorityPolicy::packet_data(8).resolve_unitdata(),
            Err(SndcpPriorityError::PduPriorityMaxOutOfRange(8))
        );
        assert_eq!(
            SndcpPriorityPolicy::packet_data(4)
                .with_sn_sap_pdu_priority(Some(8))
                .resolve_unitdata(),
            Err(SndcpPriorityError::SnSapPduPriorityOutOfRange(8))
        );
        assert_eq!(
            SndcpPriorityPolicy::packet_data(4)
                .with_sn_sap_data_priority(Some(8))
                .resolve_unitdata(),
            Err(SndcpPriorityError::SnSapDataPriorityOutOfRange(8))
        );
        assert_eq!(
            SndcpPriorityPolicy::packet_data(4)
                .with_nsapi_data_priority(Some(8))
                .resolve_unitdata(),
            Err(SndcpPriorityError::NsapiDataPriorityOutOfRange(8))
        );
        assert_eq!(
            SndcpPriorityPolicy::packet_data(4)
                .with_ms_default_data_priority(Some(8))
                .resolve_unitdata(),
            Err(SndcpPriorityError::MsDefaultDataPriorityOutOfRange(8))
        );
    }
}
