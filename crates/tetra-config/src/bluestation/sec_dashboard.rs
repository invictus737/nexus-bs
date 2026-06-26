// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use serde::Deserialize;
use std::collections::HashMap;
use toml::Value;

/// Dashboard HTTP server configuration
#[derive(Debug, Clone)]
pub struct CfgDashboard {
    /// Port to listen on (default: 8080)
    pub port: u16,
    /// Bind address (default: 0.0.0.0)
    pub bind: String,
    /// Optional explicit path to the Nexus-BS git source directory used for OTA updates.
    /// When unset, the dashboard auto-detects by:
    ///   1. Walking up from the running binary path until a `.git` directory is found
    ///   2. Trying the well-known Nexus-BS install path (/opt/nexus-bs)
    ///   3. Falling back to the current working directory if it is a git repo
    /// Set this explicitly when the binary is installed outside the repo
    /// with the git clone elsewhere, or when auto-detection picks the wrong directory.
    pub source_dir: Option<String>,
    /// Optional external dashboard asset directory.
    ///
    /// When set, the core serves index.html and static assets from this directory
    /// while keeping `/api/*`, `/ws`, `/login` and session handling inside the
    /// lightweight Rust gateway. When unset, the embedded dashboard remains the
    /// compatibility fallback. Missing directories are accepted at parse time so
    /// appliance images can boot while assets are deployed later; the dashboard
    /// server falls back to the embedded UI until the directory is usable.
    pub static_dir: Option<String>,
    /// Enable dashboard form login.
    ///
    /// Default is false so existing local dashboards remain open unless the
    /// operator explicitly opts in. When enabled, all dashboard requests require
    /// a cookie session obtained from the form login at `/login`.
    pub auth_enabled: bool,
    /// Dashboard login credentials.
    ///
    /// These are only required and used when `auth_enabled = true`.
    ///
    /// SECURITY NOTE: without TLS the login POST still crosses the LAN in clear
    /// text. For internet-facing deployments, put a reverse proxy with HTTPS in
    /// front of the dashboard.
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Default for CfgDashboard {
    fn default() -> Self {
        Self {
            port: 8080,
            bind: "0.0.0.0".to_string(),
            source_dir: None,
            static_dir: None,
            auth_enabled: false,
            username: None,
            password: None,
        }
    }
}

#[derive(Deserialize)]
pub struct CfgDashboardDto {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default)]
    pub source_dir: Option<String>,
    #[serde(default)]
    pub static_dir: Option<String>,
    #[serde(default)]
    pub auth_enabled: bool,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

fn default_port() -> u16 {
    8080
}
fn default_bind() -> String {
    "0.0.0.0".to_string()
}
pub fn apply_dashboard_patch(src: CfgDashboardDto) -> Result<CfgDashboard, String> {
    if src.port == 0 {
        return Err("dashboard: port cannot be 0".to_string());
    }
    if src.bind.trim().is_empty() {
        return Err("dashboard: bind cannot be empty".to_string());
    }
    // Validate source_dir if provided: must be an existing directory.
    if let Some(ref sd) = src.source_dir {
        validate_existing_dir("source_dir", sd)?;
    }
    if let Some(ref sd) = src.static_dir {
        validate_dashboard_static_dir(sd)?;
    }
    if src.auth_enabled {
        match (&src.username, &src.password) {
            (Some(username), Some(password)) => {
                if username.trim().is_empty() {
                    return Err("dashboard: username cannot be empty".to_string());
                }
                if password.trim().is_empty() {
                    return Err("dashboard: password cannot be empty".to_string());
                }
            }
            (None, None) => {
                return Err("dashboard: username and password are required when auth_enabled = true".to_string());
            }
            _ => return Err("dashboard: set both 'username' and 'password' when auth_enabled = true".to_string()),
        }
    } else {
        if src.username.as_ref().is_some_and(|value| value.trim().is_empty()) {
            return Err("dashboard: username cannot be empty".to_string());
        }
        if src.password.as_ref().is_some_and(|value| value.trim().is_empty()) {
            return Err("dashboard: password cannot be empty".to_string());
        }
    }
    Ok(CfgDashboard {
        port: src.port,
        bind: src.bind,
        source_dir: src.source_dir,
        static_dir: src.static_dir,
        auth_enabled: src.auth_enabled,
        username: src.username,
        password: src.password,
    })
}

fn validate_existing_dir(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("dashboard: {field} cannot be empty (omit the field instead)"));
    }
    let path = std::path::Path::new(value);
    if !path.exists() {
        return Err(format!("dashboard: {field} '{}' does not exist", value));
    }
    if !path.is_dir() {
        return Err(format!("dashboard: {field} '{}' is not a directory", value));
    }
    Ok(())
}

fn validate_dashboard_static_dir(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("dashboard: static_dir cannot be empty (omit the field instead)".to_string());
    }
    let path = std::path::Path::new(value);
    if path.exists() && !path.is_dir() {
        return Err(format!("dashboard: static_dir '{}' is not a directory", value));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dashboard_dto_with_static_dir(static_dir: Option<String>) -> CfgDashboardDto {
        CfgDashboardDto {
            port: default_port(),
            bind: default_bind(),
            source_dir: None,
            static_dir,
            auth_enabled: false,
            username: None,
            password: None,
            extra: HashMap::new(),
        }
    }

    #[test]
    fn dashboard_static_dir_accepts_existing_directory() {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nexus-bs-dashboard-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create dashboard static_dir test dir");

        let cfg =
            apply_dashboard_patch(dashboard_dto_with_static_dir(Some(dir.to_string_lossy().to_string()))).expect("static_dir should parse");
        assert_eq!(cfg.static_dir.as_deref(), Some(dir.to_string_lossy().as_ref()));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dashboard_static_dir_accepts_missing_directory_for_runtime_fallback() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("nexus-bs-dashboard-config-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let cfg = apply_dashboard_patch(dashboard_dto_with_static_dir(Some(dir.to_string_lossy().to_string())))
            .expect("missing static_dir should parse so runtime can fall back to embedded dashboard");
        assert_eq!(cfg.static_dir.as_deref(), Some(dir.to_string_lossy().as_ref()));
    }

    #[test]
    fn dashboard_static_dir_rejects_existing_file() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "nexus-bs-dashboard-config-file-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::write(&path, b"not a directory").expect("create dashboard static_dir test file");

        let err = apply_dashboard_patch(dashboard_dto_with_static_dir(Some(path.to_string_lossy().to_string())))
            .expect_err("existing static_dir file must fail");
        assert!(err.contains("static_dir"));
        assert!(err.contains("not a directory"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn dashboard_auth_defaults_to_disabled_without_credentials() {
        let cfg = apply_dashboard_patch(dashboard_dto_with_static_dir(None)).expect("dashboard defaults should parse");

        assert!(!cfg.auth_enabled);
        assert!(cfg.username.is_none());
        assert!(cfg.password.is_none());
    }

    #[test]
    fn dashboard_auth_accepts_explicit_username_and_password() {
        let mut dto = dashboard_dto_with_static_dir(None);
        dto.auth_enabled = true;
        dto.username = Some("operator".to_string());
        dto.password = Some("change-this".to_string());

        let cfg = apply_dashboard_patch(dto).expect("paired credentials should parse");

        assert!(cfg.auth_enabled);
        assert_eq!(cfg.username.as_deref(), Some("operator"));
        assert_eq!(cfg.password.as_deref(), Some("change-this"));
    }

    #[test]
    fn dashboard_auth_enabled_requires_explicit_credentials() {
        let mut dto = dashboard_dto_with_static_dir(None);
        dto.auth_enabled = true;

        let err = apply_dashboard_patch(dto).expect_err("auth_enabled must not invent credentials");

        assert!(err.contains("required"));
    }

    #[test]
    fn dashboard_auth_rejects_blank_credentials() {
        let mut blank_user = dashboard_dto_with_static_dir(None);
        blank_user.auth_enabled = true;
        blank_user.username = Some("  ".to_string());
        blank_user.password = Some("change-this".to_string());
        let err = apply_dashboard_patch(blank_user).expect_err("blank username must fail");
        assert!(err.contains("username"));

        let mut blank_password = dashboard_dto_with_static_dir(None);
        blank_password.auth_enabled = true;
        blank_password.username = Some("operator".to_string());
        blank_password.password = Some("  ".to_string());
        let err = apply_dashboard_patch(blank_password).expect_err("blank password must fail");
        assert!(err.contains("password"));
    }

    #[test]
    fn dashboard_rejects_empty_bind_address() {
        let mut dto = dashboard_dto_with_static_dir(None);
        dto.bind = "  ".to_string();

        let err = apply_dashboard_patch(dto).expect_err("empty bind must fail");

        assert!(err.contains("bind"));
    }
}
