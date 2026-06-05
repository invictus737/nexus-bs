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
