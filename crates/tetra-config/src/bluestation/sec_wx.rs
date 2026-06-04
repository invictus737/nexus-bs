use serde::Deserialize;

/// Built-in WX/METAR SDS service configuration.
///
/// Two independent capabilities:
///   1. On-demand: a radio sends an SDS like "METAR LROP" (or just "LROP") to
///      `service_issi`; the BS fetches the METAR, decodes it to a human-readable line, and
///      replies to the sender. Enabled by `enabled`.
///   2. Periodic: the BS auto-sends the decoded METAR for `periodic_icao` to
///      `periodic_issi` every `periodic_interval_secs`. Enabled by `periodic_enabled`.
///
/// All of this runs inside Nexus-BS — no external bot needed. The dashboard can toggle
/// `enabled`/`periodic_enabled` and change the target ISSIs/ICAO at runtime.
#[derive(Debug, Clone)]
pub struct CfgWxService {
    /// Master on/off for the on-demand METAR responder.
    pub enabled: bool,
    /// ISSI that radios address to request weather (e.g. 9998). When a local SDS arrives
    /// for this ISSI and the service is enabled, the text is treated as an ICAO request.
    pub service_issi: u32,
    /// On/off for periodic auto-broadcast of a fixed station's METAR.
    pub periodic_enabled: bool,
    /// Destination of the periodic METAR (an ISSI or a GSSI for a group broadcast).
    pub periodic_issi: u32,
    /// Whether `periodic_issi` is a group (GSSI) rather than an individual (ISSI).
    pub periodic_is_group: bool,
    /// ICAO code whose METAR is auto-sent periodically (e.g. "LROP").
    pub periodic_icao: String,
    /// Period between automatic sends, in seconds. Clamped to a sane minimum at use.
    pub periodic_interval_secs: u64,
}

impl Default for CfgWxService {
    fn default() -> Self {
        CfgWxService {
            enabled: false,
            service_issi: 9998,
            periodic_enabled: false,
            periodic_issi: 0,
            periodic_is_group: false,
            periodic_icao: String::new(),
            periodic_interval_secs: 1800,
        }
    }
}

impl CfgWxService {
    /// Minimum allowed periodic interval to avoid hammering the upstream API.
    pub const MIN_PERIODIC_SECS: u64 = 300;

    /// Effective periodic interval, clamped to the minimum.
    pub fn effective_interval_secs(&self) -> u64 {
        self.periodic_interval_secs.max(Self::MIN_PERIODIC_SECS)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CfgWxServiceDto {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_service_issi")]
    pub service_issi: u32,
    #[serde(default)]
    pub periodic_enabled: bool,
    #[serde(default)]
    pub periodic_issi: u32,
    #[serde(default)]
    pub periodic_is_group: bool,
    #[serde(default)]
    pub periodic_icao: String,
    #[serde(default = "default_interval")]
    pub periodic_interval_secs: u64,
}

fn default_service_issi() -> u32 {
    9998
}
fn default_interval() -> u64 {
    1800
}

pub fn apply_wx_service_patch(dto: CfgWxServiceDto) -> CfgWxService {
    CfgWxService {
        enabled: dto.enabled,
        service_issi: dto.service_issi,
        periodic_enabled: dto.periodic_enabled,
        periodic_issi: dto.periodic_issi,
        periodic_is_group: dto.periodic_is_group,
        periodic_icao: dto.periodic_icao,
        periodic_interval_secs: dto.periodic_interval_secs,
    }
}
