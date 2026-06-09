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
    /// Optional dashboard login credentials.
    /// When both username and password are set, all dashboard requests require a
    /// cookie session obtained from the form login at `/login`.
    /// When omitted, the dashboard is accessible without a password (default, home-network use).
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
    // Validate source_dir if provided: must be an existing directory.
    if let Some(ref sd) = src.source_dir {
        validate_existing_dir("source_dir", sd)?;
    }
    if let Some(ref sd) = src.static_dir {
        validate_dashboard_static_dir(sd)?;
    }
    // Auth: either both username+password are set, or neither.
    match (&src.username, &src.password) {
        (Some(u), Some(p)) => {
            if u.trim().is_empty() {
                return Err("dashboard: username cannot be empty".to_string());
            }
            if p.is_empty() {
                return Err("dashboard: password cannot be empty".to_string());
            }
        }
        (None, None) => {}
        _ => return Err("dashboard: set both 'username' and 'password', or neither".to_string()),
    }
    Ok(CfgDashboard {
        port: src.port,
        bind: src.bind,
        source_dir: src.source_dir,
        static_dir: src.static_dir,
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
}
