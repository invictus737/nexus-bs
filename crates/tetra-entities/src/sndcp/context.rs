// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original TETRA SNDCP PDP context table primitives.

use std::collections::BTreeMap;

use super::bearer_policy::SndcpPacketDataBearerProfile;
use tetra_saps::sn::{SnAddress, SnPacketDataMsType, SnPdpType, validate_nsapi};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SndcpContextKey {
    pub issi: u32,
    pub nsapi: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SndcpPdpContext {
    pub key: SndcpContextKey,
    pub pdp_type: SnPdpType,
    pub address: SnAddress,
    pub packet_data_ms_type: SnPacketDataMsType,
    pub primary_nsapi: Option<u8>,
    pub pdu_priority: Option<u8>,
    pub data_priority: Option<u8>,
    pub max_npdu_len: Option<u16>,
    pub bearer_profile: SndcpPacketDataBearerProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SndcpContextError {
    ReservedNsapi(u8),
    DuplicateContext(SndcpContextKey),
    MissingContext(SndcpContextKey),
    PrimaryContextMissing(SndcpContextKey),
}

#[derive(Debug, Clone, Default)]
pub struct SndcpContextTable {
    contexts: BTreeMap<SndcpContextKey, SndcpPdpContext>,
}

impl SndcpContextKey {
    pub fn new(issi: u32, nsapi: u8) -> Result<Self, SndcpContextError> {
        validate_nsapi(nsapi).map_err(|_| SndcpContextError::ReservedNsapi(nsapi))?;
        Ok(Self { issi, nsapi })
    }
}

impl SndcpPdpContext {
    pub fn primary_ipv4(
        issi: u32,
        nsapi: u8,
        address: SnAddress,
        packet_data_ms_type: SnPacketDataMsType,
    ) -> Result<Self, SndcpContextError> {
        Ok(Self {
            key: SndcpContextKey::new(issi, nsapi)?,
            pdp_type: SnPdpType::Ipv4,
            address,
            packet_data_ms_type,
            primary_nsapi: None,
            pdu_priority: None,
            data_priority: None,
            max_npdu_len: None,
            bearer_profile: SndcpPacketDataBearerProfile::default(),
        })
    }

    pub fn secondary(
        issi: u32,
        nsapi: u8,
        primary_nsapi: u8,
        pdp_type: SnPdpType,
        address: SnAddress,
        packet_data_ms_type: SnPacketDataMsType,
    ) -> Result<Self, SndcpContextError> {
        Ok(Self {
            key: SndcpContextKey::new(issi, nsapi)?,
            pdp_type,
            address,
            packet_data_ms_type,
            primary_nsapi: Some(SndcpContextKey::new(issi, primary_nsapi)?.nsapi),
            pdu_priority: None,
            data_priority: None,
            max_npdu_len: None,
            bearer_profile: SndcpPacketDataBearerProfile::default(),
        })
    }

    pub fn with_qos(mut self, pdu_priority: Option<u8>, data_priority: Option<u8>, max_npdu_len: Option<u16>) -> Self {
        self.pdu_priority = pdu_priority;
        self.data_priority = data_priority;
        self.max_npdu_len = max_npdu_len;
        self
    }

    pub fn with_bearer_profile(mut self, bearer_profile: SndcpPacketDataBearerProfile) -> Self {
        self.bearer_profile = bearer_profile;
        self
    }
}

impl SndcpContextTable {
    pub fn len(&self) -> usize {
        self.contexts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.contexts.is_empty()
    }

    pub fn activate(&mut self, context: SndcpPdpContext) -> Result<(), SndcpContextError> {
        if self.contexts.contains_key(&context.key) {
            return Err(SndcpContextError::DuplicateContext(context.key));
        }

        if let Some(primary_nsapi) = context.primary_nsapi {
            let primary_key = SndcpContextKey::new(context.key.issi, primary_nsapi)?;
            if !self.contexts.contains_key(&primary_key) {
                return Err(SndcpContextError::PrimaryContextMissing(primary_key));
            }
        }

        self.contexts.insert(context.key, context);
        Ok(())
    }

    pub fn replace_existing(&mut self, context: SndcpPdpContext) -> Result<SndcpPdpContext, SndcpContextError> {
        if !self.contexts.contains_key(&context.key) {
            return Err(SndcpContextError::MissingContext(context.key));
        }

        if let Some(primary_nsapi) = context.primary_nsapi {
            let primary_key = SndcpContextKey::new(context.key.issi, primary_nsapi)?;
            if !self.contexts.contains_key(&primary_key) {
                return Err(SndcpContextError::PrimaryContextMissing(primary_key));
            }
        }

        Ok(self
            .contexts
            .insert(context.key, context)
            .expect("context existence checked before replace"))
    }

    pub fn get(&self, key: SndcpContextKey) -> Option<&SndcpPdpContext> {
        self.contexts.get(&key)
    }

    pub fn get_issi_nsapi(&self, issi: u32, nsapi: u8) -> Result<Option<&SndcpPdpContext>, SndcpContextError> {
        Ok(self.get(SndcpContextKey::new(issi, nsapi)?))
    }

    pub fn deactivate(&mut self, key: SndcpContextKey) -> Result<SndcpPdpContext, SndcpContextError> {
        self.contexts.remove(&key).ok_or(SndcpContextError::MissingContext(key))
    }

    pub fn deactivate_with_linked_secondaries(&mut self, key: SndcpContextKey) -> Result<usize, SndcpContextError> {
        let removed = self.deactivate(key)?;
        if removed.primary_nsapi.is_some() {
            return Ok(1);
        }

        let before = self.contexts.len();
        self.contexts
            .retain(|candidate, context| candidate.issi != key.issi || context.primary_nsapi != Some(key.nsapi));
        Ok(1 + before - self.contexts.len())
    }

    pub fn deactivate_issi(&mut self, issi: u32) -> usize {
        let before = self.contexts.len();
        self.contexts.retain(|key, _| key.issi != issi);
        before - self.contexts.len()
    }

    pub fn ipv4_context(&self, issi: u32, address: [u8; 4]) -> Option<&SndcpPdpContext> {
        self.contexts
            .values()
            .find(|context| context.key.issi == issi && context.pdp_type == SnPdpType::Ipv4 && context.address == SnAddress::Ipv4(address))
    }

    pub fn any_ipv4_context(&self, address: [u8; 4]) -> Option<&SndcpPdpContext> {
        self.contexts
            .values()
            .find(|context| context.pdp_type == SnPdpType::Ipv4 && context.address == SnAddress::Ipv4(address))
    }

    pub fn contexts_for_issi(&self, issi: u32) -> impl Iterator<Item = &SndcpPdpContext> {
        self.contexts.values().filter(move |context| context.key.issi == issi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn primary(issi: u32, nsapi: u8, address: [u8; 4]) -> SndcpPdpContext {
        SndcpPdpContext::primary_ipv4(issi, nsapi, SnAddress::Ipv4(address), SnPacketDataMsType::TypeAParallel)
            .expect("primary context should be valid")
    }

    #[test]
    fn context_key_rejects_reserved_nsapis() {
        assert_eq!(SndcpContextKey::new(2260618, 0), Err(SndcpContextError::ReservedNsapi(0)));
        assert_eq!(SndcpContextKey::new(2260618, 15), Err(SndcpContextError::ReservedNsapi(15)));
        assert_eq!(SndcpContextKey::new(2260618, 1).unwrap().nsapi, 1);
        assert_eq!(SndcpContextKey::new(2260618, 14).unwrap().nsapi, 14);
    }

    #[test]
    fn primary_contexts_are_keyed_by_issi_and_nsapi() {
        let mut table = SndcpContextTable::default();

        table
            .activate(primary(2260618, 2, [10, 0, 0, 18]))
            .expect("first context should activate");
        table
            .activate(primary(2260082, 2, [10, 0, 0, 82]))
            .expect("same NSAPI for another ISSI should activate independently");

        assert_eq!(table.len(), 2);
        assert_eq!(
            table.get_issi_nsapi(2260618, 2).unwrap().map(|context| context.address),
            Some(SnAddress::Ipv4([10, 0, 0, 18]))
        );
        assert_eq!(
            table.get_issi_nsapi(2260082, 2).unwrap().map(|context| context.address),
            Some(SnAddress::Ipv4([10, 0, 0, 82]))
        );
    }

    #[test]
    fn duplicate_activation_does_not_overwrite_existing_context() {
        let mut table = SndcpContextTable::default();
        let key = SndcpContextKey::new(2260618, 2).unwrap();

        table
            .activate(primary(2260618, 2, [10, 0, 0, 18]))
            .expect("first context should activate");
        assert_eq!(
            table.activate(primary(2260618, 2, [10, 0, 0, 19])),
            Err(SndcpContextError::DuplicateContext(key))
        );

        assert_eq!(table.get(key).map(|context| context.address), Some(SnAddress::Ipv4([10, 0, 0, 18])));
    }

    #[test]
    fn replace_existing_updates_context_without_creating_a_duplicate() {
        let mut table = SndcpContextTable::default();
        let key = SndcpContextKey::new(2260618, 2).unwrap();

        table
            .activate(primary(2260618, 2, [10, 0, 0, 18]))
            .expect("first context should activate");
        let previous = table
            .replace_existing(primary(2260618, 2, [10, 0, 0, 19]))
            .expect("existing context should be replaceable");

        assert_eq!(previous.address, SnAddress::Ipv4([10, 0, 0, 18]));
        assert_eq!(table.len(), 1);
        assert_eq!(table.get(key).map(|context| context.address), Some(SnAddress::Ipv4([10, 0, 0, 19])));
    }

    #[test]
    fn secondary_context_requires_existing_primary_for_same_issi() {
        let mut table = SndcpContextTable::default();
        let missing_primary = SndcpContextKey::new(2260618, 2).unwrap();
        let secondary = SndcpPdpContext::secondary(
            2260618,
            3,
            2,
            SnPdpType::Ipv4,
            SnAddress::Ipv4([10, 0, 0, 18]),
            SnPacketDataMsType::TypeAParallel,
        )
        .expect("secondary context fields should be valid");

        assert_eq!(
            table.activate(secondary.clone()),
            Err(SndcpContextError::PrimaryContextMissing(missing_primary))
        );

        table
            .activate(primary(2260618, 2, [10, 0, 0, 18]))
            .expect("primary context should activate");
        table
            .activate(secondary)
            .expect("secondary context should activate after primary exists");
        assert_eq!(table.len(), 2);
    }

    #[test]
    fn deactivation_can_remove_one_context_or_all_contexts_for_an_issi() {
        let mut table = SndcpContextTable::default();
        let first_key = SndcpContextKey::new(2260618, 2).unwrap();
        table.activate(primary(2260618, 2, [10, 0, 0, 18])).unwrap();
        table.activate(primary(2260618, 3, [10, 0, 0, 19])).unwrap();
        table.activate(primary(2260082, 2, [10, 0, 0, 82])).unwrap();

        assert_eq!(table.deactivate(first_key).unwrap().address, SnAddress::Ipv4([10, 0, 0, 18]));
        assert_eq!(table.deactivate(first_key), Err(SndcpContextError::MissingContext(first_key)));

        assert_eq!(table.deactivate_issi(2260618), 1);
        assert_eq!(table.len(), 1);
        assert!(table.get_issi_nsapi(2260082, 2).unwrap().is_some());
    }

    #[test]
    fn deactivating_primary_context_removes_linked_secondaries_for_same_issi() {
        let mut table = SndcpContextTable::default();
        let primary_key = SndcpContextKey::new(2260618, 2).unwrap();
        let secondary_key = SndcpContextKey::new(2260618, 3).unwrap();
        let other_issi_key = SndcpContextKey::new(2260082, 3).unwrap();

        table.activate(primary(2260618, 2, [10, 0, 0, 18])).unwrap();
        table.activate(primary(2260082, 2, [10, 0, 0, 82])).unwrap();
        table
            .activate(
                SndcpPdpContext::secondary(
                    2260618,
                    3,
                    2,
                    SnPdpType::Ipv4,
                    SnAddress::Ipv4([10, 0, 0, 18]),
                    SnPacketDataMsType::TypeAParallel,
                )
                .unwrap(),
            )
            .unwrap();
        table
            .activate(
                SndcpPdpContext::secondary(
                    2260082,
                    3,
                    2,
                    SnPdpType::Ipv4,
                    SnAddress::Ipv4([10, 0, 0, 82]),
                    SnPacketDataMsType::TypeAParallel,
                )
                .unwrap(),
            )
            .unwrap();

        assert_eq!(table.deactivate_with_linked_secondaries(primary_key), Ok(2));
        assert!(table.get(primary_key).is_none());
        assert!(table.get(secondary_key).is_none());
        assert!(table.get(other_issi_key).is_some());
    }

    #[test]
    fn deactivating_secondary_context_does_not_remove_primary() {
        let mut table = SndcpContextTable::default();
        let primary_key = SndcpContextKey::new(2260618, 2).unwrap();
        let secondary_key = SndcpContextKey::new(2260618, 3).unwrap();

        table.activate(primary(2260618, 2, [10, 0, 0, 18])).unwrap();
        table
            .activate(
                SndcpPdpContext::secondary(
                    2260618,
                    3,
                    2,
                    SnPdpType::Ipv4,
                    SnAddress::Ipv4([10, 0, 0, 18]),
                    SnPacketDataMsType::TypeAParallel,
                )
                .unwrap(),
            )
            .unwrap();

        assert_eq!(table.deactivate_with_linked_secondaries(secondary_key), Ok(1));
        assert!(table.get(primary_key).is_some());
        assert!(table.get(secondary_key).is_none());
    }

    #[test]
    fn ipv4_lookup_matches_only_the_requested_subscriber() {
        let mut table = SndcpContextTable::default();
        table.activate(primary(2260618, 2, [10, 0, 0, 18])).unwrap();
        table.activate(primary(2260082, 2, [10, 0, 0, 18])).unwrap();

        assert_eq!(
            table.ipv4_context(2260618, [10, 0, 0, 18]).map(|context| context.key.issi),
            Some(2260618)
        );
        assert_eq!(table.ipv4_context(2260616, [10, 0, 0, 18]), None);
    }

    #[test]
    fn any_ipv4_context_detects_address_use_across_subscribers() {
        let mut table = SndcpContextTable::default();
        table.activate(primary(2260618, 2, [10, 0, 0, 18])).unwrap();

        assert_eq!(
            table.any_ipv4_context([10, 0, 0, 18]).map(|context| context.key.issi),
            Some(2260618)
        );
        assert_eq!(table.any_ipv4_context([10, 0, 0, 19]), None);
    }
}
