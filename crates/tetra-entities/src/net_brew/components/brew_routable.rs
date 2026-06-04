use tetra_config::bluestation::SharedConfig;

/// Returns true if the Brew component is active
#[inline]
pub fn is_active(config: &SharedConfig) -> bool {
    config.config().brew.is_some()
}

/// Returns true if the SDS over Brew feature is enabled
#[inline]
pub fn feature_sds_enabled(config: &SharedConfig) -> bool {
    config.config().brew.as_ref().map_or(false, |brew| brew.feature_sds_enabled)
}

/// Returns true if the configured Brew server is TetraPack (core.tetrapack.online)
fn is_tetrapack(config: &SharedConfig) -> bool {
    if let Some(brew_config) = &config.config().brew {
        brew_config.host == "core.tetrapack.online"
    } else {
        false
    }
}

/// Determine if a given GSSI should be routed over Brew, or is restricted to local handling
pub fn is_brew_gssi_routable(config: &SharedConfig, ssi: u32) -> bool {
    let Some(brew_config) = &config.config().brew else {
        // Brew not configured, so no routing to Brew
        return false;
    };
    if config.config().cell.local_ssi_ranges.contains(ssi) {
        // Range overridden as local
        return false;
    }

    // Check if whitelist is present and if so, check
    if let Some(whitelist) = &brew_config.whitelisted_ssis {
        if whitelist.contains(&ssi) {
            // Range explicitly whitelisted for routing to Brew
            return true;
        } else {
            // Not in whitelist - block routing to Brew
            return false;
        }
    }

    // No whitelist present, default to allow
    true
}

/// Determine if a given ISSI should be sent to the Brew server.
/// On TetraPack, subscriber ISSIs must be 7 digits (1_000_000..=9_999_999).
/// Special service ISSIs (e.g. 600 echo, short numbers) are always forwarded to Brew —
/// TetraPack Core handles them internally; blocking them here causes "Service Denied".
pub fn is_brew_issi_routable(config: &SharedConfig, issi: u32) -> bool {
    if config.config().brew.is_none() {
        return false;
    }
    if config.config().cell.local_ssi_ranges.contains(issi) {
        // Local routing policy: configured local SSI ranges stay inside this
        // cell. This is not an ETSI air-interface rule; it prevents local
        // private-call ISSIs from being registered or dialled over Brew.
        return false;
    }
    if is_tetrapack(config) {
        // 7-digit subscriber ISSIs are always routable.
        // Short ISSIs (< 1_000_000) are service numbers handled by TetraPack Core —
        // let them through so the core can respond (echo test 600, etc.)
        issi >= 1_000_000 && issi <= 9_999_999 || issi < 1_000_000
    } else {
        true
    }
}

#[cfg(test)]
mod tests {
    use tetra_config::bluestation::{SharedConfig, from_toml_str};

    use super::*;

    fn example_config() -> SharedConfig {
        let toml = format!(
            "{}\n\n[brew]\nhost = \"core.tetrapack.online\"\nport = 443\ntls = true\nusername = 226008200\npassword = \"test\"\n",
            include_str!("../../../../../example_config/config.toml")
        );
        let cfg = from_toml_str(&toml).expect("example config with Brew should parse");
        SharedConfig::from_parts(cfg, None)
    }

    #[test]
    fn local_ssi_ranges_block_issi_brew_routing() {
        let config = example_config();

        assert!(
            !is_brew_issi_routable(&config, 2260082),
            "local private-call lab ISSIs must remain inside this cell instead of being routed over Brew"
        );
        assert!(
            !is_brew_issi_routable(&config, 2260616),
            "local private-call lab ISSIs must remain inside this cell instead of being routed over Brew"
        );
    }

    #[test]
    fn non_local_tetrapack_issi_remains_brew_routable() {
        let config = example_config();

        assert!(
            is_brew_issi_routable(&config, 3108031),
            "non-local 7-digit subscriber ISSIs keep the existing TetraPack routing behaviour"
        );
    }
}
