// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrewSubscriberAction {
    Register,
    Deregister,
    Affiliate,
    Deaffiliate,
    /// Internal MM -> CMCE request: clear stale individual-call state for an
    /// ISSI while preserving registration and group affiliations.
    ReleaseIndividualCalls,
}

#[derive(Debug, Clone)]
pub struct MmSubscriberUpdate {
    pub issi: u32,
    pub groups: Vec<u32>,
    pub action: BrewSubscriberAction,
}
