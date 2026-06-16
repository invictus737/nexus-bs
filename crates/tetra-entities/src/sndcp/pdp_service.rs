// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original TETRA SNDCP PDP context activation service primitive.

use super::bearer_policy::SndcpPacketDataBearerProfile;
use super::context::{SndcpContextError, SndcpContextKey, SndcpContextTable, SndcpPdpContext};
use super::pdp::{
    SndcpActivateAddressDemand, SndcpActivatePdpContextAccept, SndcpActivatePdpContextDemand, SndcpActivatePdpContextReject,
    SndcpActivationRejectCause, SndcpDeactivation, SndcpMaximumTransmissionUnit, SndcpTypeIdentifierInAccept,
};
use tetra_saps::sn::{SnAddress, SnPacketDataMsType, validate_pdu_priority};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SndcpIpv4Pool {
    pub prefix: [u8; 3],
    pub first_host: u8,
    pub last_host: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SndcpPdpPolicy {
    pub service_enabled: bool,
    pub dynamic_ipv4_pool: Option<SndcpIpv4Pool>,
    pub allow_static_ipv4: bool,
    pub allow_secondary_contexts: bool,
    pub accept_type_a_parallel: bool,
    pub accept_type_b_alternating: bool,
    pub accept_type_c_ip_single_mode: bool,
    pub accept_type_d_restricted_ip_single_mode: bool,
    pub pdu_priority_max: u8,
    pub ready_timer: u8,
    pub standby_timer: u8,
    pub response_wait_timer: u8,
    pub maximum_transmission_unit: SndcpMaximumTransmissionUnit,
    pub max_contexts_per_issi: usize,
    pub default_bearer_profile: SndcpPacketDataBearerProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SndcpPdpActivationResult {
    Accepted {
        accept: SndcpActivatePdpContextAccept,
        context: SndcpPdpContext,
        reused_existing: bool,
    },
    Rejected(SndcpActivatePdpContextReject),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SndcpPdpDeactivationResult {
    pub accept: SndcpDeactivation,
    pub removed_contexts: usize,
}

#[derive(Debug, Clone)]
pub struct SndcpPdpService {
    policy: SndcpPdpPolicy,
    contexts: SndcpContextTable,
}

impl Default for SndcpIpv4Pool {
    fn default() -> Self {
        Self {
            prefix: [10, 0, 0],
            first_host: 2,
            last_host: 254,
        }
    }
}

impl Default for SndcpPdpPolicy {
    fn default() -> Self {
        Self {
            service_enabled: false,
            dynamic_ipv4_pool: None,
            allow_static_ipv4: false,
            allow_secondary_contexts: false,
            accept_type_a_parallel: false,
            accept_type_b_alternating: false,
            accept_type_c_ip_single_mode: false,
            accept_type_d_restricted_ip_single_mode: false,
            pdu_priority_max: 4,
            ready_timer: 8,
            standby_timer: 4,
            response_wait_timer: 7,
            maximum_transmission_unit: SndcpMaximumTransmissionUnit::Octets576,
            max_contexts_per_issi: 4,
            default_bearer_profile: SndcpPacketDataBearerProfile::default(),
        }
    }
}

impl SndcpPdpPolicy {
    pub fn experimental_wap_ipv4() -> Self {
        Self {
            service_enabled: true,
            dynamic_ipv4_pool: Some(SndcpIpv4Pool::default()),
            allow_static_ipv4: true,
            allow_secondary_contexts: false,
            accept_type_a_parallel: true,
            accept_type_b_alternating: true,
            accept_type_c_ip_single_mode: true,
            accept_type_d_restricted_ip_single_mode: false,
            maximum_transmission_unit: SndcpMaximumTransmissionUnit::Octets1500,
            ..Self::default()
        }
    }
}

impl SndcpPdpService {
    pub fn new(policy: SndcpPdpPolicy) -> Self {
        Self {
            policy,
            contexts: SndcpContextTable::default(),
        }
    }

    pub fn policy(&self) -> &SndcpPdpPolicy {
        &self.policy
    }

    pub fn contexts(&self) -> &SndcpContextTable {
        &self.contexts
    }

    pub fn into_contexts(self) -> SndcpContextTable {
        self.contexts
    }

    pub fn handle_activate_demand(&mut self, issi: u32, demand: SndcpActivatePdpContextDemand) -> SndcpPdpActivationResult {
        if !self.policy.service_enabled {
            return self.reject(demand.nsapi, SndcpActivationRejectCause::SndcpServiceTemporarilyNotAvailable);
        }

        if !self.packet_data_ms_type_supported(demand.packet_data_ms_type) {
            return self.reject(demand.nsapi, SndcpActivationRejectCause::PacketDataMsTypeNotSupported);
        }

        if let Some(existing_context) = self.contexts.get_issi_nsapi(issi, demand.nsapi).ok().flatten().cloned() {
            if let Some(accept) = self.accept_for_existing_context(&demand, &existing_context) {
                return SndcpPdpActivationResult::Accepted {
                    accept,
                    context: existing_context,
                    reused_existing: true,
                };
            }
            return self.reject(demand.nsapi, SndcpActivationRejectCause::Undefined);
        }

        if self.contexts.contexts_for_issi(issi).count() >= self.policy.max_contexts_per_issi {
            return self.reject(demand.nsapi, SndcpActivationRejectCause::MaximumNumberOfPdpContextsPerItsiExceeded);
        }

        if validate_pdu_priority(self.policy.pdu_priority_max).is_err() {
            return self.reject(demand.nsapi, SndcpActivationRejectCause::Undefined);
        }

        let (context, accept) = match self.build_context_and_accept(issi, &demand) {
            Ok(result) => result,
            Err(cause) => return self.reject(demand.nsapi, cause),
        };

        match self.contexts.activate(context.clone()) {
            Ok(()) => SndcpPdpActivationResult::Accepted {
                accept,
                context,
                reused_existing: false,
            },
            Err(SndcpContextError::PrimaryContextMissing(_)) => {
                self.reject(demand.nsapi, SndcpActivationRejectCause::PrimaryPdpContextDoesNotExist)
            }
            Err(SndcpContextError::DuplicateContext(_)) => self.reject(demand.nsapi, SndcpActivationRejectCause::Undefined),
            Err(SndcpContextError::ReservedNsapi(nsapi)) => self.reject(nsapi, SndcpActivationRejectCause::Other(0)),
            Err(SndcpContextError::MissingContext(_)) => self.reject(demand.nsapi, SndcpActivationRejectCause::Other(0)),
        }
    }

    pub fn handle_deactivate_demand(&mut self, issi: u32, deactivation: SndcpDeactivation) -> SndcpPdpDeactivationResult {
        let removed_contexts = match deactivation {
            SndcpDeactivation::AllNsapis => self.contexts.deactivate_issi(issi),
            SndcpDeactivation::Nsapi(nsapi) => {
                let Ok(key) = SndcpContextKey::new(issi, nsapi) else {
                    return SndcpPdpDeactivationResult {
                        accept: deactivation,
                        removed_contexts: 0,
                    };
                };
                match self.contexts.deactivate_with_linked_secondaries(key) {
                    Ok(removed) => removed,
                    Err(SndcpContextError::MissingContext(_)) => 0,
                    Err(_) => 0,
                }
            }
        };

        SndcpPdpDeactivationResult {
            accept: deactivation,
            removed_contexts,
        }
    }

    fn build_context_and_accept(
        &self,
        issi: u32,
        demand: &SndcpActivatePdpContextDemand,
    ) -> Result<(SndcpPdpContext, SndcpActivatePdpContextAccept), SndcpActivationRejectCause> {
        let (context, type_identifier, assigned_address) = match demand.address {
            SndcpActivateAddressDemand::Ipv4Static(address) => {
                if !self.policy.allow_static_ipv4 {
                    return Err(SndcpActivationRejectCause::StaticAddressNotAllowed);
                }
                if !self.static_ipv4_address_allowed(address) {
                    return Err(SndcpActivationRejectCause::StaticAddressNotAllowed);
                }
                if self.contexts.any_ipv4_context(address).is_some() {
                    return Err(SndcpActivationRejectCause::StaticAddressInUse);
                }
                (
                    SndcpPdpContext::primary_ipv4(issi, demand.nsapi, SnAddress::Ipv4(address), demand.packet_data_ms_type)
                        .map_err(context_error_to_reject_cause)?,
                    SndcpTypeIdentifierInAccept::Ipv4StaticAddress,
                    Some(SnAddress::Ipv4(address)),
                )
            }
            SndcpActivateAddressDemand::Ipv4Dynamic => {
                let Some(address) = self.allocate_dynamic_ipv4() else {
                    return Err(SndcpActivationRejectCause::DynamicAddressPoolEmpty);
                };
                (
                    SndcpPdpContext::primary_ipv4(issi, demand.nsapi, SnAddress::Ipv4(address), demand.packet_data_ms_type)
                        .map_err(context_error_to_reject_cause)?,
                    SndcpTypeIdentifierInAccept::Ipv4DynamicAddress,
                    Some(SnAddress::Ipv4(address)),
                )
            }
            SndcpActivateAddressDemand::SecondaryPdpContext { primary_nsapi } => {
                if !self.policy.allow_secondary_contexts {
                    return Err(SndcpActivationRejectCause::SecondaryPdpContextsNotSupported);
                }
                let Some(primary) = self
                    .contexts
                    .get_issi_nsapi(issi, primary_nsapi)
                    .map_err(context_error_to_reject_cause)?
                else {
                    return Err(SndcpActivationRejectCause::PrimaryPdpContextDoesNotExist);
                };
                (
                    SndcpPdpContext::secondary(
                        issi,
                        demand.nsapi,
                        primary_nsapi,
                        primary.pdp_type,
                        primary.address,
                        demand.packet_data_ms_type,
                    )
                    .map_err(context_error_to_reject_cause)?,
                    SndcpTypeIdentifierInAccept::NoAddress,
                    None,
                )
            }
            SndcpActivateAddressDemand::Ipv6 => return Err(SndcpActivationRejectCause::Ipv6NotSupported),
            SndcpActivateAddressDemand::MobileIpv4ForeignAgentCareOfAddress => {
                return Err(SndcpActivationRejectCause::Other(17));
            }
            SndcpActivateAddressDemand::MobileIpv4CoLocatedCareOfAddress => return Err(SndcpActivationRejectCause::Other(18)),
        };

        let context = context
            .with_qos(
                Some(self.policy.pdu_priority_max),
                None,
                Some(maximum_transmission_unit_octets(self.policy.maximum_transmission_unit)),
            )
            .with_bearer_profile(self.policy.default_bearer_profile);
        let accept = SndcpActivatePdpContextAccept {
            nsapi: demand.nsapi,
            pdu_priority_max: self.policy.pdu_priority_max,
            ready_timer: self.policy.ready_timer,
            standby_timer: self.policy.standby_timer,
            response_wait_timer: self.policy.response_wait_timer,
            type_identifier,
            assigned_address,
            pcomp_negotiation: 0,
            maximum_transmission_unit: self.policy.maximum_transmission_unit,
        };

        Ok((context, accept))
    }

    fn accept_for_existing_context(
        &self,
        demand: &SndcpActivatePdpContextDemand,
        context: &SndcpPdpContext,
    ) -> Option<SndcpActivatePdpContextAccept> {
        if context.packet_data_ms_type != demand.packet_data_ms_type {
            return None;
        }

        let (type_identifier, assigned_address) = match (&demand.address, context.address) {
            (SndcpActivateAddressDemand::Ipv4Dynamic, SnAddress::Ipv4(address)) => {
                (SndcpTypeIdentifierInAccept::Ipv4DynamicAddress, Some(SnAddress::Ipv4(address)))
            }
            (SndcpActivateAddressDemand::Ipv4Static(requested), SnAddress::Ipv4(existing)) if *requested == existing => {
                (SndcpTypeIdentifierInAccept::Ipv4StaticAddress, Some(SnAddress::Ipv4(existing)))
            }
            (SndcpActivateAddressDemand::SecondaryPdpContext { primary_nsapi }, _) if context.primary_nsapi == Some(*primary_nsapi) => {
                (SndcpTypeIdentifierInAccept::NoAddress, None)
            }
            _ => return None,
        };

        Some(SndcpActivatePdpContextAccept {
            nsapi: demand.nsapi,
            pdu_priority_max: self.policy.pdu_priority_max,
            ready_timer: self.policy.ready_timer,
            standby_timer: self.policy.standby_timer,
            response_wait_timer: self.policy.response_wait_timer,
            type_identifier,
            assigned_address,
            pcomp_negotiation: 0,
            maximum_transmission_unit: self.policy.maximum_transmission_unit,
        })
    }

    fn packet_data_ms_type_supported(&self, packet_data_ms_type: SnPacketDataMsType) -> bool {
        match packet_data_ms_type {
            SnPacketDataMsType::TypeAParallel => self.policy.accept_type_a_parallel,
            SnPacketDataMsType::TypeBAlternating => self.policy.accept_type_b_alternating,
            SnPacketDataMsType::TypeCIpSingleMode => self.policy.accept_type_c_ip_single_mode,
            SnPacketDataMsType::TypeDRestrictedIpSingleMode => self.policy.accept_type_d_restricted_ip_single_mode,
        }
    }

    fn allocate_dynamic_ipv4(&self) -> Option<[u8; 4]> {
        let pool = self.policy.dynamic_ipv4_pool?;
        if pool.first_host > pool.last_host {
            return None;
        }

        (pool.first_host..=pool.last_host)
            .map(|host| [pool.prefix[0], pool.prefix[1], pool.prefix[2], host])
            .find(|address| self.contexts.any_ipv4_context(*address).is_none())
    }

    fn static_ipv4_address_allowed(&self, address: [u8; 4]) -> bool {
        self.policy
            .dynamic_ipv4_pool
            .map(|pool| ipv4_in_pool(address, pool))
            .unwrap_or(true)
    }

    fn reject(&self, nsapi: u8, cause: SndcpActivationRejectCause) -> SndcpPdpActivationResult {
        SndcpPdpActivationResult::Rejected(SndcpActivatePdpContextReject { nsapi, cause })
    }
}

fn ipv4_in_pool(address: [u8; 4], pool: SndcpIpv4Pool) -> bool {
    address[..3] == pool.prefix[..] && (pool.first_host..=pool.last_host).contains(&address[3])
}

impl Default for SndcpPdpService {
    fn default() -> Self {
        Self::new(SndcpPdpPolicy::default())
    }
}

pub fn maximum_transmission_unit_octets(maximum_transmission_unit: SndcpMaximumTransmissionUnit) -> u16 {
    match maximum_transmission_unit {
        SndcpMaximumTransmissionUnit::Octets296 => 296,
        SndcpMaximumTransmissionUnit::Octets576 => 576,
        SndcpMaximumTransmissionUnit::Octets1006 => 1006,
        SndcpMaximumTransmissionUnit::Octets1500 => 1500,
        SndcpMaximumTransmissionUnit::Octets2002 => 2002,
    }
}

fn context_error_to_reject_cause(error: SndcpContextError) -> SndcpActivationRejectCause {
    match error {
        SndcpContextError::PrimaryContextMissing(_) => SndcpActivationRejectCause::PrimaryPdpContextDoesNotExist,
        SndcpContextError::DuplicateContext(_) => SndcpActivationRejectCause::Undefined,
        SndcpContextError::ReservedNsapi(_) | SndcpContextError::MissingContext(_) => SndcpActivationRejectCause::Other(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dynamic_demand(nsapi: u8) -> SndcpActivatePdpContextDemand {
        SndcpActivatePdpContextDemand {
            sndcp_version: 1,
            nsapi,
            address: SndcpActivateAddressDemand::Ipv4Dynamic,
            packet_data_ms_type: SnPacketDataMsType::TypeAParallel,
            pcomp_negotiation: 0,
        }
    }

    fn dynamic_demand_with_ms_type(nsapi: u8, packet_data_ms_type: SnPacketDataMsType) -> SndcpActivatePdpContextDemand {
        SndcpActivatePdpContextDemand {
            packet_data_ms_type,
            ..dynamic_demand(nsapi)
        }
    }

    fn static_demand(nsapi: u8, address: [u8; 4]) -> SndcpActivatePdpContextDemand {
        SndcpActivatePdpContextDemand {
            address: SndcpActivateAddressDemand::Ipv4Static(address),
            ..dynamic_demand(nsapi)
        }
    }

    fn service() -> SndcpPdpService {
        SndcpPdpService::new(SndcpPdpPolicy::experimental_wap_ipv4())
    }

    #[test]
    fn default_policy_rejects_activation_until_explicitly_enabled() {
        let mut service = SndcpPdpService::default();

        assert_eq!(
            service.handle_activate_demand(2260618, dynamic_demand(2)),
            SndcpPdpActivationResult::Rejected(SndcpActivatePdpContextReject {
                nsapi: 2,
                cause: SndcpActivationRejectCause::SndcpServiceTemporarilyNotAvailable
            })
        );
        assert!(service.contexts().is_empty());
    }

    #[test]
    fn dynamic_ipv4_activation_allocates_context_and_accepts_with_assigned_address() {
        let mut service = service();

        let result = service.handle_activate_demand(2260618, dynamic_demand(2));

        let SndcpPdpActivationResult::Accepted {
            accept,
            context,
            reused_existing,
        } = result
        else {
            panic!("dynamic IPv4 demand should be accepted");
        };
        assert!(!reused_existing);
        assert_eq!(accept.nsapi, 2);
        assert_eq!(accept.type_identifier, SndcpTypeIdentifierInAccept::Ipv4DynamicAddress);
        assert_eq!(accept.assigned_address, Some(SnAddress::Ipv4([10, 0, 0, 2])));
        assert_eq!(accept.maximum_transmission_unit, SndcpMaximumTransmissionUnit::Octets1500);
        assert_eq!(context.key.issi, 2260618);
        assert_eq!(context.address, SnAddress::Ipv4([10, 0, 0, 2]));
        assert_eq!(context.max_npdu_len, Some(1500));
        assert!(service.contexts().get_issi_nsapi(2260618, 2).unwrap().is_some());
    }

    #[test]
    fn wap_ipv4_policy_accepts_type_b_and_type_c_ms_profiles() {
        for packet_data_ms_type in [SnPacketDataMsType::TypeBAlternating, SnPacketDataMsType::TypeCIpSingleMode] {
            let mut service = service();

            let result = service.handle_activate_demand(2260618, dynamic_demand_with_ms_type(2, packet_data_ms_type));

            let SndcpPdpActivationResult::Accepted {
                accept,
                context,
                reused_existing,
            } = result
            else {
                panic!("dynamic IPv4 demand should accept {packet_data_ms_type:?}");
            };
            assert!(!reused_existing);
            assert_eq!(accept.nsapi, 2);
            assert_eq!(accept.assigned_address, Some(SnAddress::Ipv4([10, 0, 0, 2])));
            assert_eq!(context.packet_data_ms_type, packet_data_ms_type);
        }
    }

    #[test]
    fn wap_ipv4_policy_keeps_restricted_type_d_fail_closed() {
        let mut service = service();

        assert_eq!(
            service.handle_activate_demand(
                2260618,
                dynamic_demand_with_ms_type(2, SnPacketDataMsType::TypeDRestrictedIpSingleMode)
            ),
            SndcpPdpActivationResult::Rejected(SndcpActivatePdpContextReject {
                nsapi: 2,
                cause: SndcpActivationRejectCause::PacketDataMsTypeNotSupported
            })
        );
        assert!(service.contexts().is_empty());
    }

    #[test]
    fn dynamic_ipv4_pool_reuses_released_address_after_deactivation() {
        let mut service = service();
        assert!(matches!(
            service.handle_activate_demand(2260618, dynamic_demand(2)),
            SndcpPdpActivationResult::Accepted { .. }
        ));
        assert_eq!(
            service
                .handle_deactivate_demand(2260618, SndcpDeactivation::Nsapi(2))
                .removed_contexts,
            1
        );

        let result = service.handle_activate_demand(2260082, dynamic_demand(2));
        let SndcpPdpActivationResult::Accepted { accept, .. } = result else {
            panic!("released dynamic IPv4 address should be reusable");
        };
        assert_eq!(accept.assigned_address, Some(SnAddress::Ipv4([10, 0, 0, 2])));
    }

    #[test]
    fn duplicate_dynamic_activation_reuses_existing_context() {
        let mut service = service();
        assert!(matches!(
            service.handle_activate_demand(2260618, dynamic_demand_with_ms_type(1, SnPacketDataMsType::TypeBAlternating)),
            SndcpPdpActivationResult::Accepted {
                reused_existing: false,
                ..
            }
        ));

        let result = service.handle_activate_demand(2260618, dynamic_demand_with_ms_type(1, SnPacketDataMsType::TypeBAlternating));

        let SndcpPdpActivationResult::Accepted {
            accept,
            context,
            reused_existing,
        } = result
        else {
            panic!("compatible duplicate dynamic activation should be accepted as a reactivation");
        };
        assert!(reused_existing);
        assert_eq!(accept.nsapi, 1);
        assert_eq!(accept.type_identifier, SndcpTypeIdentifierInAccept::Ipv4DynamicAddress);
        assert_eq!(accept.assigned_address, Some(SnAddress::Ipv4([10, 0, 0, 2])));
        assert_eq!(context.address, SnAddress::Ipv4([10, 0, 0, 2]));
        assert_eq!(service.contexts().contexts_for_issi(2260618).count(), 1);
    }

    #[test]
    fn incompatible_duplicate_nsapi_activation_is_rejected_without_claiming_max_contexts() {
        let mut service = SndcpPdpService::new(SndcpPdpPolicy {
            max_contexts_per_issi: 1,
            ..SndcpPdpPolicy::experimental_wap_ipv4()
        });
        assert!(matches!(
            service.handle_activate_demand(2260618, dynamic_demand(2)),
            SndcpPdpActivationResult::Accepted { .. }
        ));

        assert_eq!(
            service.handle_activate_demand(2260618, static_demand(2, [10, 0, 0, 18])),
            SndcpPdpActivationResult::Rejected(SndcpActivatePdpContextReject {
                nsapi: 2,
                cause: SndcpActivationRejectCause::Undefined
            })
        );
        assert_eq!(
            service
                .contexts()
                .get_issi_nsapi(2260618, 2)
                .unwrap()
                .map(|context| context.address),
            Some(SnAddress::Ipv4([10, 0, 0, 2]))
        );
    }

    #[test]
    fn static_ipv4_activation_rejects_address_already_in_use() {
        let mut service = service();
        assert!(matches!(
            service.handle_activate_demand(2260618, static_demand(2, [10, 0, 0, 18])),
            SndcpPdpActivationResult::Accepted { .. }
        ));

        let result = service.handle_activate_demand(2260082, static_demand(2, [10, 0, 0, 18]));

        assert_eq!(
            result,
            SndcpPdpActivationResult::Rejected(SndcpActivatePdpContextReject {
                nsapi: 2,
                cause: SndcpActivationRejectCause::StaticAddressInUse
            })
        );
    }

    #[test]
    fn static_ipv4_activation_rejects_addresses_outside_terminal_pool() {
        let mut service = service();

        assert_eq!(
            service.handle_activate_demand(2260618, static_demand(2, [10, 0, 0, 1])),
            SndcpPdpActivationResult::Rejected(SndcpActivatePdpContextReject {
                nsapi: 2,
                cause: SndcpActivationRejectCause::StaticAddressNotAllowed
            })
        );
        assert_eq!(
            service.handle_activate_demand(2260618, static_demand(2, [10, 0, 1, 18])),
            SndcpPdpActivationResult::Rejected(SndcpActivatePdpContextReject {
                nsapi: 2,
                cause: SndcpActivationRejectCause::StaticAddressNotAllowed
            })
        );
        assert!(service.contexts().is_empty());
    }

    #[test]
    fn secondary_context_requires_existing_primary_and_inherits_address() {
        let mut service = SndcpPdpService::new(SndcpPdpPolicy {
            allow_secondary_contexts: true,
            ..SndcpPdpPolicy::experimental_wap_ipv4()
        });
        let secondary = SndcpActivatePdpContextDemand {
            nsapi: 3,
            address: SndcpActivateAddressDemand::SecondaryPdpContext { primary_nsapi: 2 },
            ..dynamic_demand(3)
        };

        assert_eq!(
            service.handle_activate_demand(2260618, secondary.clone()),
            SndcpPdpActivationResult::Rejected(SndcpActivatePdpContextReject {
                nsapi: 3,
                cause: SndcpActivationRejectCause::PrimaryPdpContextDoesNotExist
            })
        );

        assert!(matches!(
            service.handle_activate_demand(2260618, static_demand(2, [10, 0, 0, 18])),
            SndcpPdpActivationResult::Accepted { .. }
        ));
        let result = service.handle_activate_demand(2260618, secondary);

        let SndcpPdpActivationResult::Accepted { accept, context, .. } = result else {
            panic!("secondary PDP context should activate after primary exists");
        };
        assert_eq!(accept.type_identifier, SndcpTypeIdentifierInAccept::NoAddress);
        assert_eq!(accept.assigned_address, None);
        assert_eq!(context.primary_nsapi, Some(2));
        assert_eq!(context.address, SnAddress::Ipv4([10, 0, 0, 18]));
    }

    #[test]
    fn primary_deactivation_cascades_to_linked_secondary_contexts() {
        let mut service = SndcpPdpService::new(SndcpPdpPolicy {
            allow_secondary_contexts: true,
            ..SndcpPdpPolicy::experimental_wap_ipv4()
        });
        let secondary = SndcpActivatePdpContextDemand {
            nsapi: 3,
            address: SndcpActivateAddressDemand::SecondaryPdpContext { primary_nsapi: 2 },
            ..dynamic_demand(3)
        };

        assert!(matches!(
            service.handle_activate_demand(2260618, static_demand(2, [10, 0, 0, 18])),
            SndcpPdpActivationResult::Accepted { .. }
        ));
        assert!(matches!(
            service.handle_activate_demand(2260618, secondary),
            SndcpPdpActivationResult::Accepted { .. }
        ));

        let result = service.handle_deactivate_demand(2260618, SndcpDeactivation::Nsapi(2));

        assert_eq!(result.removed_contexts, 2);
        assert!(service.contexts().get_issi_nsapi(2260618, 2).unwrap().is_none());
        assert!(service.contexts().get_issi_nsapi(2260618, 3).unwrap().is_none());
    }

    #[test]
    fn unsupported_packet_data_ms_type_and_address_family_are_rejected() {
        let mut service = SndcpPdpService::new(SndcpPdpPolicy {
            accept_type_c_ip_single_mode: false,
            ..SndcpPdpPolicy::experimental_wap_ipv4()
        });
        let type_c = SndcpActivatePdpContextDemand {
            packet_data_ms_type: SnPacketDataMsType::TypeCIpSingleMode,
            ..dynamic_demand(2)
        };
        assert_eq!(
            service.handle_activate_demand(2260618, type_c),
            SndcpPdpActivationResult::Rejected(SndcpActivatePdpContextReject {
                nsapi: 2,
                cause: SndcpActivationRejectCause::PacketDataMsTypeNotSupported
            })
        );

        let ipv6 = SndcpActivatePdpContextDemand {
            address: SndcpActivateAddressDemand::Ipv6,
            ..dynamic_demand(2)
        };
        assert_eq!(
            service.handle_activate_demand(2260618, ipv6),
            SndcpPdpActivationResult::Rejected(SndcpActivatePdpContextReject {
                nsapi: 2,
                cause: SndcpActivationRejectCause::Ipv6NotSupported
            })
        );
    }

    #[test]
    fn mobile_ipv4_reject_causes_distinguish_fa_and_co_located_requests() {
        let mut service = service();
        let fa = SndcpActivatePdpContextDemand {
            address: SndcpActivateAddressDemand::MobileIpv4ForeignAgentCareOfAddress,
            ..dynamic_demand(2)
        };
        let co_located = SndcpActivatePdpContextDemand {
            nsapi: 3,
            address: SndcpActivateAddressDemand::MobileIpv4CoLocatedCareOfAddress,
            ..dynamic_demand(3)
        };

        assert_eq!(
            service.handle_activate_demand(2260618, fa),
            SndcpPdpActivationResult::Rejected(SndcpActivatePdpContextReject {
                nsapi: 2,
                cause: SndcpActivationRejectCause::Other(17)
            })
        );
        assert_eq!(
            service.handle_activate_demand(2260618, co_located),
            SndcpPdpActivationResult::Rejected(SndcpActivatePdpContextReject {
                nsapi: 3,
                cause: SndcpActivationRejectCause::Other(18)
            })
        );
    }

    #[test]
    fn max_contexts_per_issi_is_enforced_without_affecting_other_subscribers() {
        let mut service = SndcpPdpService::new(SndcpPdpPolicy {
            max_contexts_per_issi: 1,
            ..SndcpPdpPolicy::experimental_wap_ipv4()
        });
        assert!(matches!(
            service.handle_activate_demand(2260618, dynamic_demand(2)),
            SndcpPdpActivationResult::Accepted { .. }
        ));

        assert_eq!(
            service.handle_activate_demand(2260618, dynamic_demand(3)),
            SndcpPdpActivationResult::Rejected(SndcpActivatePdpContextReject {
                nsapi: 3,
                cause: SndcpActivationRejectCause::MaximumNumberOfPdpContextsPerItsiExceeded
            })
        );
        assert!(matches!(
            service.handle_activate_demand(2260082, dynamic_demand(2)),
            SndcpPdpActivationResult::Accepted { .. }
        ));
    }

    #[test]
    fn out_of_range_policy_pdu_priority_max_is_rejected_before_context_activation() {
        let mut service = SndcpPdpService::new(SndcpPdpPolicy {
            pdu_priority_max: 8,
            ..SndcpPdpPolicy::experimental_wap_ipv4()
        });

        assert_eq!(
            service.handle_activate_demand(2260618, dynamic_demand(2)),
            SndcpPdpActivationResult::Rejected(SndcpActivatePdpContextReject {
                nsapi: 2,
                cause: SndcpActivationRejectCause::Undefined
            })
        );
        assert!(service.contexts().is_empty());
    }

    #[test]
    fn deactivation_removes_one_or_all_contexts_for_one_subscriber() {
        let mut service = service();
        assert!(matches!(
            service.handle_activate_demand(2260618, dynamic_demand(2)),
            SndcpPdpActivationResult::Accepted { .. }
        ));
        assert!(matches!(
            service.handle_activate_demand(2260618, dynamic_demand(3)),
            SndcpPdpActivationResult::Accepted { .. }
        ));
        assert!(matches!(
            service.handle_activate_demand(2260082, dynamic_demand(2)),
            SndcpPdpActivationResult::Accepted { .. }
        ));

        let single = service.handle_deactivate_demand(2260618, SndcpDeactivation::Nsapi(2));
        assert_eq!(single.removed_contexts, 1);
        assert!(service.contexts().get_issi_nsapi(2260618, 2).unwrap().is_none());
        assert!(service.contexts().get_issi_nsapi(2260618, 3).unwrap().is_some());

        let all = service.handle_deactivate_demand(2260618, SndcpDeactivation::AllNsapis);
        assert_eq!(all.removed_contexts, 1);
        assert!(service.contexts().get_issi_nsapi(2260082, 2).unwrap().is_some());
    }
}
