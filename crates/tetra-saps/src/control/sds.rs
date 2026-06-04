use crate::control::enums::sds_user_data::SdsUserData;
use tetra_core::{SsiType, TxReporter};

/// SDS data routing between CMCE SDS subentity and Brew entity
#[derive(Debug, Clone)]
pub struct CmceSdsData {
    /// Source ISSI (calling party)
    pub source_issi: u32,
    /// Destination ISSI (called party)
    pub dest_issi: u32,
    /// Optional explicit destination kind. None keeps legacy numeric routing.
    pub dest_ssi_type: Option<SsiType>,
    /// User-defined data (type1, type2, type3, or type4)
    pub user_defined_data: SdsUserData,
    /// Optional air-interface delivery reporter kept by the originator.
    pub tx_reporter: Option<TxReporter>,
}

/// SDS pre-coded status routing at the user SAP.
#[derive(Debug, Clone)]
pub struct CmceSdsStatus {
    /// Source ISSI (calling party)
    pub source_issi: u32,
    /// Destination ISSI (called party)
    pub dest_issi: u32,
    /// Destination address kind on air.
    pub dest_ssi_type: SsiType,
    /// Raw 16-bit TNSDS status number.
    pub status_number: u16,
    /// Optional air-interface delivery reporter kept by the originator.
    pub tx_reporter: Option<TxReporter>,
}
