// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

//! GitHub release update-check for the dashboard.
//!
//! Compares the locally built version (`tetra_core::STACK_VERSION`, e.g. the current
//! Nexus-BS release tag plus a git suffix) against the latest GitHub release tag and
//! reports whether a newer version exists. The dashboard keeps OTA update disabled for
//! now; this helper remains isolated for the future `/api/update/check` path.
//!
//! The check is best-effort: any network/parse failure yields `UpdateCheck::unknown()`
//! rather than an error, so a flaky connection never breaks the dashboard.

use serde::Serialize;
use std::time::Duration;

const GITHUB_API_LATEST: &str = "https://api.github.com/repos/invictus737/nexus-bs/releases/latest";
const GITHUB_API_RELEASES: &str = "https://api.github.com/repos/invictus737/nexus-bs/releases?per_page=50";
// GitHub requires a User-Agent on all API requests.
const USER_AGENT: &str = tetra_core::PRODUCT_USER_AGENT;

/// A parsed semantic version (major.minor.patch). Pre-release/build metadata is ignored
/// for comparison purposes — we only care about the release triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SemVer {
    major: u32,
    minor: u32,
    patch: u32,
}

impl SemVer {
    /// Parse a version from a string like "v0.1.57", "0.1.57", or "v0.1.57-gabc123".
    /// Leading 'v'/'V' is optional; anything after the patch (a '-' or '+' suffix) is
    /// ignored. Returns None if the major.minor.patch core can't be parsed.
    fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
        // Cut at the first '-' or '+' (pre-release / build / git suffix).
        let core = s.split(['-', '+']).next().unwrap_or(s);
        let mut it = core.split('.');
        let major = it.next()?.trim().parse().ok()?;
        let minor = it.next().unwrap_or("0").trim().parse().unwrap_or(0);
        let patch = it.next().unwrap_or("0").trim().parse().unwrap_or(0);
        Some(SemVer { major, minor, patch })
    }
}

/// Result of an update check, serialised to JSON for the dashboard.
#[derive(Debug, Clone)]
pub struct UpdateCheck {
    /// Locally built version string (as-is, e.g. "v0.1.57-gabc123").
    pub current: String,
    /// Latest release tag from GitHub, if the check succeeded.
    pub latest: Option<String>,
    /// True when `latest` parses to a strictly higher SemVer than `current`.
    pub update_available: bool,
    /// URL of the latest release page, if available (for a "view release" link).
    pub release_url: Option<String>,
    /// True when the check itself failed (network/parse). The badge should stay hidden.
    pub check_failed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DebRelease {
    pub tag: String,
    pub version: String,
    pub name: String,
    pub published_at: Option<String>,
    pub html_url: Option<String>,
    pub deb_asset_name: String,
    pub deb_url: String,
    pub deb_size: Option<u64>,
    pub update_available: bool,
    pub relation: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReleaseCatalog {
    pub current: String,
    pub arch: String,
    pub latest: Option<String>,
    pub update_available: bool,
    pub releases: Vec<DebRelease>,
    pub check_failed: bool,
    pub error: Option<String>,
}

impl UpdateCheck {
    fn unknown(current: &str) -> Self {
        UpdateCheck {
            current: current.to_string(),
            latest: None,
            update_available: false,
            release_url: None,
            check_failed: true,
        }
    }

    /// Render as a JSON object for `GET /api/update/check`.
    pub fn to_json(&self) -> String {
        let latest = self
            .latest
            .as_deref()
            .map(|s| format!("\"{}\"", json_escape(s)))
            .unwrap_or_else(|| "null".to_string());
        let url = self
            .release_url
            .as_deref()
            .map(|s| format!("\"{}\"", json_escape(s)))
            .unwrap_or_else(|| "null".to_string());
        format!(
            "{{\"current\":\"{}\",\"latest\":{},\"update_available\":{},\"release_url\":{},\"check_failed\":{}}}",
            json_escape(&self.current),
            latest,
            self.update_available,
            url,
            self.check_failed
        )
    }
}

impl ReleaseCatalog {
    fn failed(current: &str, arch: &str, error: impl Into<String>) -> Self {
        Self {
            current: current.to_string(),
            arch: arch.to_string(),
            latest: None,
            update_available: false,
            releases: Vec::new(),
            check_failed: true,
            error: Some(error.into()),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                "{{\"current\":\"{}\",\"arch\":\"{}\",\"latest\":null,\"update_available\":false,\"releases\":[],\"check_failed\":true}}",
                json_escape(&self.current),
                json_escape(&self.arch)
            )
        })
    }
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Query GitHub for the latest release and compare against `current_version`
/// (typically `tetra_core::STACK_VERSION`). Blocking; call from a worker thread.
pub fn check_for_update(current_version: &str) -> UpdateCheck {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .build()
    {
        Ok(c) => c,
        Err(_) => return UpdateCheck::unknown(current_version),
    };

    let resp = match client
        .get(GITHUB_API_LATEST)
        .header("Accept", "application/vnd.github+json")
        .send()
        .and_then(|r| r.error_for_status())
    {
        Ok(r) => r,
        Err(_) => return UpdateCheck::unknown(current_version),
    };

    let json: serde_json::Value = match resp.json() {
        Ok(j) => j,
        Err(_) => return UpdateCheck::unknown(current_version),
    };

    let tag = json.get("tag_name").and_then(|v| v.as_str());
    let html_url = json.get("html_url").and_then(|v| v.as_str()).map(|s| s.to_string());

    let Some(tag) = tag else {
        return UpdateCheck::unknown(current_version);
    };

    let update_available = match (SemVer::parse(current_version), SemVer::parse(tag)) {
        (Some(cur), Some(latest)) => latest > cur,
        // If we can't parse one side, don't claim an update is available.
        _ => false,
    };

    UpdateCheck {
        current: current_version.to_string(),
        latest: Some(tag.to_string()),
        update_available,
        release_url: html_url,
        check_failed: false,
    }
}

pub fn list_deb_releases(current_version: &str, arch: &str) -> ReleaseCatalog {
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent(USER_AGENT)
        .build()
    {
        Ok(c) => c,
        Err(e) => return ReleaseCatalog::failed(current_version, arch, format!("http client: {e}")),
    };

    let resp = match client
        .get(GITHUB_API_RELEASES)
        .header("Accept", "application/vnd.github+json")
        .send()
        .and_then(|r| r.error_for_status())
    {
        Ok(r) => r,
        Err(e) => return ReleaseCatalog::failed(current_version, arch, format!("github releases: {e}")),
    };

    let json: serde_json::Value = match resp.json() {
        Ok(j) => j,
        Err(e) => return ReleaseCatalog::failed(current_version, arch, format!("github json: {e}")),
    };
    let Some(items) = json.as_array() else {
        return ReleaseCatalog::failed(current_version, arch, "github releases response was not an array");
    };

    let current_semver = SemVer::parse(current_version);
    let mut releases = Vec::new();

    for release in items {
        if release.get("draft").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        let Some(tag) = release.get("tag_name").and_then(|v| v.as_str()) else {
            continue;
        };
        if tag.contains("_dev") {
            continue;
        }
        let Some(version) = SemVer::parse(tag) else {
            continue;
        };
        let Some(assets) = release.get("assets").and_then(|v| v.as_array()) else {
            continue;
        };
        let Some(asset) = assets.iter().find(|asset| {
            let name = asset.get("name").and_then(|v| v.as_str()).unwrap_or("");
            name.ends_with(".deb") && name.contains(arch) && !name.contains("_dev")
        }) else {
            continue;
        };
        let Some(deb_url) = asset.get("browser_download_url").and_then(|v| v.as_str()) else {
            continue;
        };
        let relation = match current_semver {
            Some(current) if version > current => "newer",
            Some(current) if version < current => "older",
            Some(_) => "current",
            None => "available",
        };
        releases.push(DebRelease {
            tag: tag.to_string(),
            version: tag.trim_start_matches(['v', 'V']).to_string(),
            name: release
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(tag)
                .to_string(),
            published_at: release.get("published_at").and_then(|v| v.as_str()).map(str::to_string),
            html_url: release.get("html_url").and_then(|v| v.as_str()).map(str::to_string),
            deb_asset_name: asset.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            deb_url: deb_url.to_string(),
            deb_size: asset.get("size").and_then(|v| v.as_u64()),
            update_available: relation == "newer",
            relation: relation.to_string(),
        });
    }

    releases.sort_by(|a, b| {
        SemVer::parse(&b.tag)
            .cmp(&SemVer::parse(&a.tag))
            .then_with(|| b.published_at.cmp(&a.published_at))
    });
    let latest = releases.first().map(|release| release.tag.clone());
    let update_available = releases.iter().any(|release| release.update_available);

    ReleaseCatalog {
        current: current_version.to_string(),
        arch: arch.to_string(),
        latest,
        update_available,
        releases,
        check_failed: false,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next_patch_tag() -> String {
        let cur = SemVer::parse(tetra_core::PRODUCT_VERSION_TAG).unwrap();
        format!("v{}.{}.{}", cur.major, cur.minor, cur.patch + 1)
    }

    fn following_patch_tag() -> String {
        let cur = SemVer::parse(tetra_core::PRODUCT_VERSION_TAG).unwrap();
        format!("v{}.{}.{}", cur.major, cur.minor, cur.patch + 2)
    }

    fn next_major_tag() -> String {
        let cur = SemVer::parse(tetra_core::PRODUCT_VERSION_TAG).unwrap();
        format!("v{}.0.0", cur.major + 1)
    }

    #[test]
    fn parse_plain() {
        let version = SemVer::parse(tetra_core::PRODUCT_VERSION).unwrap();
        assert_eq!(
            SemVer::parse(tetra_core::PRODUCT_VERSION),
            Some(SemVer {
                major: version.major,
                minor: version.minor,
                patch: version.patch
            })
        );
    }

    #[test]
    fn parse_v_prefix() {
        assert_eq!(
            SemVer::parse("v1.4.0"),
            Some(SemVer {
                major: 1,
                minor: 4,
                patch: 0
            })
        );
    }

    #[test]
    fn parse_git_suffix() {
        let version_with_git_suffix = format!("{}-gabc123", tetra_core::PRODUCT_VERSION_TAG);
        let version = SemVer::parse(tetra_core::PRODUCT_VERSION_TAG).unwrap();
        assert_eq!(
            SemVer::parse(&version_with_git_suffix),
            Some(SemVer {
                major: version.major,
                minor: version.minor,
                patch: version.patch
            })
        );
    }

    #[test]
    fn parse_partial() {
        assert_eq!(
            SemVer::parse("v2.1"),
            Some(SemVer {
                major: 2,
                minor: 1,
                patch: 0
            })
        );
        assert_eq!(
            SemVer::parse("3"),
            Some(SemVer {
                major: 3,
                minor: 0,
                patch: 0
            })
        );
    }

    #[test]
    fn compare_versions() {
        let a = SemVer::parse(tetra_core::PRODUCT_VERSION_TAG).unwrap();
        let b = SemVer::parse(&next_patch_tag()).unwrap();
        let c = SemVer::parse(&following_patch_tag()).unwrap();
        let d = SemVer::parse(&next_major_tag()).unwrap();
        assert!(b > a);
        assert!(c > b);
        assert!(d > c);
        assert!(a == SemVer::parse(&format!("{}-gdeadbeef", tetra_core::PRODUCT_VERSION)).unwrap());
    }

    #[test]
    fn newer_release_detected() {
        // Simulate the comparison check_for_update does.
        let cur = SemVer::parse(&format!("{}-gabc", tetra_core::PRODUCT_VERSION_TAG)).unwrap();
        let latest = SemVer::parse(&next_patch_tag()).unwrap();
        assert!(latest > cur);
    }

    #[test]
    fn same_version_no_update() {
        let cur = SemVer::parse(&format!("{}-gabc", tetra_core::PRODUCT_VERSION_TAG)).unwrap();
        let latest = SemVer::parse(tetra_core::PRODUCT_VERSION_TAG).unwrap();
        assert!(!(latest > cur));
    }

    #[test]
    fn unparseable_tag_no_update() {
        assert_eq!(SemVer::parse("nightly"), None);
    }

    #[test]
    fn json_output() {
        let latest = next_patch_tag();
        let uc = UpdateCheck {
            current: format!("{}-gabc", tetra_core::PRODUCT_VERSION_TAG),
            latest: Some(latest.clone()),
            update_available: true,
            release_url: Some(format!("https://github.com/Nexus-BS/nexus-bs/releases/tag/{latest}")),
            check_failed: false,
        };
        let j = uc.to_json();
        assert!(j.contains("\"update_available\":true"));
        assert!(j.contains(&format!("\"latest\":\"{latest}\"")));
        assert!(j.contains("\"check_failed\":false"));
    }
}
