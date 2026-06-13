// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::collections::HashMap;

use tetra_core::TetraAddress;
use tetra_pdus::umac::fields::EventLabel;

// EN 300 392-2 clause 23.4.1.2.3 reserves all-zero and all-ones event labels
// for special BS downlink meanings; normal MAC use is 1..=1022.
const FIRST_NORMAL_EVENT_LABEL: EventLabel = 1;
const LAST_NORMAL_EVENT_LABEL: EventLabel = 0x03fe;

pub struct EventLabelMapping {
    // pub valid_until: TdmaTime,
    pub addr: TetraAddress,
    pub label: EventLabel,
}

pub struct EventLabelStore {
    labels: HashMap<EventLabel, EventLabelMapping>,
    next_label: EventLabel,
}

impl EventLabelStore {
    pub fn new() -> Self {
        Self {
            labels: HashMap::new(),
            next_label: 1,
        }
    }

    /// Get the next free event label. Event labels are allocated linearly, and we assume the next one to be
    /// free. Upon rollover, we assume old labels to have been dropped by now. If not, we'll crash when inserting
    /// a label into the labels hashmap.
    pub fn get_free_label(&mut self) -> EventLabel {
        let ret = self.next_label;
        self.next_label += 1;
        if self.next_label > LAST_NORMAL_EVENT_LABEL {
            self.next_label = FIRST_NORMAL_EVENT_LABEL;
        }
        ret
    }

    /// Create an event label for a TetraAddress. There should not yet exist a label for this address, or we
    /// crash. Returns the generated event label.
    fn create_label_for_addr(&mut self, addr: TetraAddress) -> EventLabel {
        assert!(
            self.get_label_by_addr(addr).is_none(),
            "an event label for this TetraAddress already exists"
        );

        let label = self.get_free_label();
        let entry = EventLabelMapping { addr, label };
        self.labels.insert(label, entry);

        label
    }

    /// Retrieve an address by its label. The returned address may be encrypted if
    /// the unencrypted variant was not known at the time of label creation
    pub fn get_addr_by_label(&self, label: EventLabel) -> Option<TetraAddress> {
        self.labels.get(&label).map(|event_label| event_label.addr)
    }

    /// Find if a label is associated with a full typed TETRA address.
    pub fn get_label_by_addr(&self, addr: TetraAddress) -> Option<EventLabel> {
        self.labels
            .values()
            .find(|event_label| event_label.addr == addr)
            .map(|event_label| event_label.label)
    }

    /// Find if a label is associated with some SSI.
    pub fn get_label_by_ssi(&self, ssi: u32) -> Option<EventLabel> {
        self.labels
            .values()
            .find(|event_label| event_label.addr.ssi == ssi)
            .map(|event_label| event_label.label)
    }

    // pub fn remove_label(&mut self, label: EventLabel) -> Option<EventLabel> {
    //     self.labels.remove(&label)
    // }

    // pub fn contains_label(&self, label: EventLabel) -> bool {
    //     self.labels.contains_key(&label)
    // }

    // pub fn len(&self) -> usize {
    //     self.labels.len()
    // }

    // pub fn is_empty(&self) -> bool {
    //     self.labels.is_empty()
    // }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tetra_core::SsiType;

    #[test]
    fn get_free_label_wraps_across_normal_event_labels_without_reserved_values() {
        let mut store = EventLabelStore::new();

        let mut labels = Vec::new();
        for _ in 0..=((LAST_NORMAL_EVENT_LABEL as usize) * 2) {
            let label = store.get_free_label();
            assert!(
                (FIRST_NORMAL_EVENT_LABEL..=LAST_NORMAL_EVENT_LABEL).contains(&label),
                "event label {label} must stay in the normal-use range"
            );
            assert_ne!(label, 0);
            assert_ne!(label, 0x03ff);
            labels.push(label);
        }

        assert_eq!(labels[0], FIRST_NORMAL_EVENT_LABEL);
        assert_eq!(labels[(LAST_NORMAL_EVENT_LABEL - 1) as usize], LAST_NORMAL_EVENT_LABEL);
        assert_eq!(labels[LAST_NORMAL_EVENT_LABEL as usize], FIRST_NORMAL_EVENT_LABEL);
    }

    #[test]
    fn event_labels_are_keyed_by_typed_tetra_address() {
        let mut store = EventLabelStore::new();
        let issi = TetraAddress::new(0x3021, SsiType::Issi);
        let gssi = TetraAddress::new(0x3021, SsiType::Gssi);

        let issi_label = store.create_label_for_addr(issi);
        let gssi_label = store.create_label_for_addr(gssi);

        assert_ne!(issi_label, gssi_label);
        assert_eq!(store.get_label_by_addr(issi), Some(issi_label));
        assert_eq!(store.get_label_by_addr(gssi), Some(gssi_label));
        assert_eq!(store.get_addr_by_label(issi_label), Some(issi));
        assert_eq!(store.get_addr_by_label(gssi_label), Some(gssi));
    }
}
