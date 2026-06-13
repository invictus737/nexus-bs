// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::collections::HashMap;

use tetra_core::{EndpointId, LinkId, MleHandle, TdmaTime, TetraAddress};

#[derive(Debug, Clone)]
pub struct MleConnState {
    pub addr: TetraAddress,
    pub link_id: LinkId,
    pub endpoint_id: EndpointId,
    pub ts_created: TdmaTime,
    pub ts_last_used: TdmaTime,
}

/// Local MLE handle allocator for received TL-DATA/TL-UNITDATA indications.
///
/// EN 300 392-2 clause 22.3.1.1 requires MLE/LLC handles to identify
/// subsequent related primitives. Keep handle zero unused so missing handle
/// propagation cannot accidentally look valid at the LLC boundary.
pub struct MleRouter {
    states: HashMap<MleHandle, MleConnState>,
    next_handle: MleHandle,
}

impl MleRouter {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
            next_handle: 1,
        }
    }

    pub fn create_handle(&mut self, addr: TetraAddress, link_id: LinkId, endpoint_id: EndpointId, ts: TdmaTime) -> MleHandle {
        loop {
            let handle = self.next_handle;
            self.next_handle = self.next_handle.wrapping_add(1);
            if self.next_handle == 0 {
                self.next_handle = 1;
            }

            if handle != 0 && !self.states.contains_key(&handle) {
                self.states.insert(
                    handle,
                    MleConnState {
                        addr,
                        link_id,
                        endpoint_id,
                        ts_created: ts,
                        ts_last_used: ts,
                    },
                );
                return handle;
            }
        }
    }

    pub fn use_handle(&mut self, handle: MleHandle, ts: TdmaTime) -> Option<&MleConnState> {
        let conn = self.states.get_mut(&handle)?;
        conn.ts_last_used = ts;
        Some(conn)
    }

    pub fn delete_handle(&mut self, handle: MleHandle) -> Option<MleConnState> {
        self.states.remove(&handle)
    }
}

impl Default for MleRouter {
    fn default() -> Self {
        Self::new()
    }
}
