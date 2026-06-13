// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalChannel {
    Tp,
    Cp,
    Unallocated,
}

/// The endpoint identifiers between the MLE and LLC, and between the LLC and MAC, refer to the MAC resource that is
/// currently used for that service. These identifiers may be local. There shall be a unique correspondence between the
/// endpoint identifier and the physical allocation (timeslot or timeslots) used in the MAC. (This correspondence is known
/// only within the MAC.) More than one advanced link may use one MAC resource.
/// In the current implementation, the endpoint_id is just the timeslot number used by the MAC.
pub type EndpointId = u32;

pub type LinkId = u32;

/// Handle assigned by MLE to primitives for MM/CMCE/SNDCP
pub type MleHandle = u32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Layer2Service {
    /// Temporary sentinel while legacy call sites are audited.
    /// MLE must reject this value instead of guessing an ETSI LLC service.
    Todo,
    /// Use acknowledged BL-DATA (or BL-ADATA) service
    Acknowledged,
    /// EN 300 392-2 clause 18.6.6 acknowledged response maps to TL-DATA response.
    AcknowledgedResponse,
    /// Use unacknowledged BL-UDATA service
    Unacknowledged,
}
