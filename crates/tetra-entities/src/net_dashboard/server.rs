// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Instant, SystemTime};

use tungstenite::{
    Message, accept_hdr,
    handshake::server::{Request, Response},
};

use tetra_config::bluestation::{DEFAULT_SDS_TEXT_PROTOCOL_ID, LIVE_SDS_QUEUE_MAX_LEN, is_supported_periodic_sds_protocol_id};

use crate::net_control::commands::{
    ControlCommand, WAP_MVP_COLOR_PAGE_TEXT, WAP_MVP_PAGE_TEXT, WAP_SDS_TYPE4_MAX_BYTE_ALIGNED_PAYLOAD_BYTES, wap_sds_type4_payload,
};
use crate::net_dashboard::html::DASHBOARD_HTML;
use crate::net_dashboard::state::{CallEntry, DashboardState, DashboardStateInner, MsEntry};
use crate::net_telemetry::TelemetryEvent;

type CmdSender = crossbeam_channel::Sender<ControlCommand>;

// Each WS connection registers a Sender here.
// broadcast() sends to all of them; dead connections are pruned automatically.
type WsBroadcastTx = crossbeam_channel::Sender<String>;
type WsClients = Arc<Mutex<Vec<WsBroadcastTx>>>;

#[derive(Clone)]
struct CachedDashboardConfig {
    config_path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
    config: tetra_config::bluestation::StackConfig,
}

static DASHBOARD_CONFIG_CACHE: OnceLock<Mutex<HashMap<String, CachedDashboardConfig>>> = OnceLock::new();

const MAX_AIR_INTERFACE_SSI: u64 = 0x00FF_FFFF;
const DASHBOARD_WAP_SOURCE_SSI: u32 = 0x00FF_FFFF;
const DASHBOARD_OTA_UPDATE_ENABLED: bool = false;
const DASHBOARD_WS_BROADCAST_QUEUE_MAX: usize = 4096;
const DASHBOARD_HTTP_CONNECTION_MAX: usize = 64;
const DASHBOARD_HTTP_HEADER_MAX: usize = 8 * 1024;
const DASHBOARD_HTTP_HEADER_LINE_MAX: usize = 2 * 1024;
const DASHBOARD_HTTP_BODY_MAX: usize = 512 * 1024;
const DASHBOARD_SMALL_BODY_MAX: usize = 4096;
const DASHBOARD_STATIC_FILE_MAX: u64 = 2 * 1024 * 1024;
const EASY_START_TEMPLATE_TOML: &str = include_str!("../../../../example_config/config_template.toml");
const EASY_START_DONE_MARKER: &str = ".easy-start-done";
const EASY_START_SKIP_MARKER: &str = ".easy-start-skipped";
const FACTORY_RESET_CONFIRMATION: &str = "RESET NEXUS-BS";
const DASHBOARD_SECURITY_HEADERS: &str = concat!(
    "X-Content-Type-Options: nosniff\r\n",
    "X-Frame-Options: DENY\r\n",
    "Referrer-Policy: no-referrer\r\n",
    "Content-Security-Policy: frame-ancestors 'none'; object-src 'none'; base-uri 'none'\r\n",
);

#[derive(Debug, PartialEq, Eq)]
struct LiveSdsPost {
    text: String,
    protocol_id: u8,
    source_issi: u32,
    repeat_count: u32,
}

#[derive(Debug, PartialEq, Eq)]
struct WsSdsCommand {
    dest_issi: u32,
    message: String,
}

#[derive(Debug, PartialEq, Eq)]
struct WsWapCommand {
    dest_issi: u32,
    color: bool,
}

fn optional_u64_field(v: &serde_json::Value, field: &str) -> Result<Option<u64>, String> {
    match v.get(field) {
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{field} must be an unsigned integer")),
        None => Ok(None),
    }
}

fn parse_live_sds_post(v: &serde_json::Value) -> Result<LiveSdsPost, String> {
    let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
    if text.is_empty() || text.len() > 251 {
        return Err("text required, max 251 chars".to_string());
    }

    // EN 300 392-2 table 29.21 defines the SDS-TL protocol identifier as an
    // 8-bit field; do not wrap dashboard input into an unintended PID.
    let protocol_id = match optional_u64_field(v, "protocol_id")? {
        Some(raw) => u8::try_from(raw).map_err(|_| "protocol_id must be 0..255".to_string())?,
        None => DEFAULT_SDS_TEXT_PROTOCOL_ID,
    };
    if !is_supported_periodic_sds_protocol_id(protocol_id) {
        return Err(
            "protocol_id must be 0x82 text messaging or user-defined SDS-TL PID 0xC0..0xFE for text-style SDS; standard non-text SDS-TL application PIDs, including WAP/WCMP, require their own payload encoders and are rejected here"
                .to_string(),
        );
    }

    // EN 300 392-2 clause 29.3.3.8.2 uses the 24-bit all-ones SSI (0xFFFFFF)
    // as the broadcast source address; reject wider dashboard input instead of
    // truncating it to a different on-air SSI.
    let source_issi = match optional_u64_field(v, "source_issi")? {
        Some(raw) if raw <= MAX_AIR_INTERFACE_SSI => raw as u32,
        Some(_) => return Err("source_issi must be 0..16777215".to_string()),
        None => MAX_AIR_INTERFACE_SSI as u32,
    };

    let repeat_count = match optional_u64_field(v, "repeat_count")? {
        Some(raw) => u32::try_from(raw).map_err(|_| "repeat_count must be 0..4294967295".to_string())?,
        None => 0,
    };

    Ok(LiveSdsPost {
        text,
        protocol_id,
        source_issi,
        repeat_count,
    })
}

fn parse_ws_sds_command(v: &serde_json::Value) -> Result<WsSdsCommand, String> {
    let raw_dest = optional_u64_field(v, "dest_issi")?.ok_or_else(|| "dest_issi required".to_string())?;
    if raw_dest == 0 {
        return Err("dest_issi required".to_string());
    }
    if raw_dest > MAX_AIR_INTERFACE_SSI {
        return Err("dest_issi must be 1..16777215".to_string());
    }

    let message = v.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
    if message.is_empty() {
        return Err("message required".to_string());
    }

    Ok(WsSdsCommand {
        dest_issi: raw_dest as u32,
        message,
    })
}

fn parse_ws_wap_command(v: &serde_json::Value) -> Result<WsWapCommand, String> {
    let raw_dest = optional_u64_field(v, "dest_issi")?.ok_or_else(|| "dest_issi required".to_string())?;
    if raw_dest == 0 {
        return Err("dest_issi required".to_string());
    }
    if raw_dest > MAX_AIR_INTERFACE_SSI {
        return Err("dest_issi must be 1..16777215".to_string());
    }

    Ok(WsWapCommand {
        dest_issi: raw_dest as u32,
        color: v.get("color").and_then(|c| c.as_bool()).unwrap_or(false),
    })
}

// ---------------------------------------------------------------------------
// OTA update state — shared between the HTTP handler and the update thread.
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
enum UpdatePhase {
    Idle,
    Running,
    Done { success: bool },
}

struct UpdateState {
    phase: UpdatePhase,
    log: String,
}

impl UpdateState {
    fn new() -> Self {
        UpdateState {
            phase: UpdatePhase::Idle,
            log: String::new(),
        }
    }
    fn append(&mut self, line: &str) {
        self.log.push_str(line);
        self.log.push('\n');
    }
    fn start(&mut self) {
        self.phase = UpdatePhase::Running;
        self.log.clear();
    }
    fn finish(&mut self, success: bool) {
        self.phase = UpdatePhase::Done { success };
    }
}

type SharedUpdateState = Arc<Mutex<UpdateState>>;

#[derive(Debug, Clone)]
struct DebUpdateRequest {
    version: String,
    asset_name: String,
    url: String,
}

/// In-memory session store for cookie-based authentication.
///
/// We deliberately don't use Basic Auth from the browser any more: on iOS Safari and
/// older mobile browsers the native Basic Auth dialog frequently asks for credentials
/// 2-3 times in a row, prompts on every WebSocket reconnect, or "forgets" credentials
/// after switching tabs. A cookie-backed session avoids all of that and lets us
/// design a proper login screen.
///
/// Tokens are random 32-byte hex strings. They expire after 7 days of inactivity.
/// The store is per-process (no on-disk persistence) — restarting Nexus-BS logs
/// every session out. That's fine: the dashboard is typically a single-operator tool.
pub struct SessionStore {
    sessions: HashMap<String, std::time::Instant>,
    /// Sessions older than this are pruned on access.
    ttl: std::time::Duration,
}

impl SessionStore {
    fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            ttl: std::time::Duration::from_secs(7 * 24 * 60 * 60),
        }
    }

    /// Create a new session, return its token. Caller sets it as a cookie.
    fn create(&mut self) -> String {
        self.prune();
        let token = generate_session_token();
        self.sessions.insert(token.clone(), std::time::Instant::now());
        token
    }

    /// Return true if the token is known and not expired. Refreshes last-seen on hit.
    fn validate(&mut self, token: &str) -> bool {
        self.prune();
        if let Some(seen) = self.sessions.get_mut(token) {
            *seen = std::time::Instant::now();
            return true;
        }
        false
    }

    fn invalidate(&mut self, token: &str) {
        self.sessions.remove(token);
    }

    fn prune(&mut self) {
        let now = std::time::Instant::now();
        let ttl = self.ttl;
        self.sessions.retain(|_, seen| now.duration_since(*seen) < ttl);
    }
}

type SharedSessionStore = Arc<Mutex<SessionStore>>;

/// 32 bytes of entropy → 64-char hex string. Uses the OS RNG via `getrandom`-style
/// `/dev/urandom` read. Falls back to a time+pid mix if /dev/urandom is unavailable —
/// not cryptographically perfect, but adequate for a session token on a LAN-only
/// dashboard. Production-grade deployments behind a reverse proxy already get HTTPS
/// hardening from the proxy layer.
fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut bytes);
    } else {
        // Fallback: deterministic-ish entropy from time + pid + addr-of-self.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id() as u128;
        let mix = nanos.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(pid << 64);
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = ((mix >> (i * 4)) & 0xff) as u8;
        }
    }
    let mut s = String::with_capacity(64);
    for b in &bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// Extract `fs_session=<token>` from a Cookie header in the raw request.
fn parse_session_cookie(headers: &str) -> Option<String> {
    for line in headers.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("cookie:") {
            // Use original (non-lowered) line for the value to preserve case in token.
            let value_line = &line["cookie:".len()..];
            for kv in value_line.split(';') {
                let kv = kv.trim();
                if let Some(token) = kv.strip_prefix("fs_session=") {
                    return Some(token.to_string());
                }
            }
            let _ = rest;
        }
    }
    None
}

/// Resolve the Nexus-BS git source directory for OTA updates.
///
/// Resolution order (first match wins):
///   1. `override_dir` from config ([dashboard].source_dir) — explicit user choice.
///   2. Walk up from `current_exe()` looking for a `.git` directory. This handles
///      the development case where the binary lives at `<src>/target/release/...`.
///   3. Well-known Nexus-BS install path: `/opt/nexus-bs`.
///      Useful when the binary was deployed separately from the source tree
///      and the sources were cloned in `/opt/nexus-bs/`.
///   4. `current_dir()` if it contains a `.git` directory.
///
/// Returns `Ok(path)` on success, or `Err(message)` listing all paths tried.
/// The returned path is guaranteed to contain a `.git` entry (file or directory —
/// `.git` can be a file in git worktrees).
fn resolve_source_dir(override_dir: Option<&str>) -> Result<std::path::PathBuf, String> {
    fn is_git_repo(p: &std::path::Path) -> bool {
        // `.git` is a directory in normal clones, but a file in git worktrees,
        // so check for existence of either form.
        p.join(".git").exists()
    }

    fn is_acceptable_path(p: &std::path::Path) -> bool {
        // Reject filesystem root, single-character paths, /usr, /bin, etc.
        // These are never valid source directories and would just produce confusing errors.
        let s = p.to_string_lossy();
        s != "/" && s.len() > 6 && !matches!(s.as_ref(), "/usr" | "/bin" | "/sbin" | "/etc" | "/var" | "/tmp")
    }

    let mut tried: Vec<String> = Vec::new();

    // 1. Explicit override from config.
    if let Some(dir) = override_dir {
        let path = std::path::PathBuf::from(dir);
        if is_git_repo(&path) && is_acceptable_path(&path) {
            return Ok(path);
        }
        tried.push(format!("{} (from config: not a git repo)", path.display()));
    }

    // 2. Walk up from the running binary path, up to 6 levels.
    if let Ok(exe) = std::env::current_exe() {
        let mut cur = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..6 {
            let Some(p) = cur else { break };
            if !is_acceptable_path(&p) {
                tried.push(format!("{} (rejected: system path or too shallow)", p.display()));
                break;
            }
            if is_git_repo(&p) {
                return Ok(p);
            }
            tried.push(format!("{} (walked up from binary)", p.display()));
            cur = p.parent().map(|pp| pp.to_path_buf());
        }
    }

    // 3. Well-known install paths.
    for candidate in &["/opt/nexus-bs"] {
        let p = std::path::PathBuf::from(candidate);
        if is_git_repo(&p) {
            return Ok(p);
        }
        if p.exists() {
            tried.push(format!("{} (well-known path: exists but not a git repo)", candidate));
        }
    }

    // 4. Current working directory.
    if let Ok(cwd) = std::env::current_dir() {
        if is_git_repo(&cwd) && is_acceptable_path(&cwd) {
            return Ok(cwd);
        }
        tried.push(format!("{} (current working dir: not a git repo)", cwd.display()));
    }

    Err(format!(
        "OTA update needs the Nexus-BS git source tree to be present on this machine, \
         but none was found. You have two options:\n\
         \n\
         1) Clone the sources next to your binary:\n\
            git clone <nexus-bs-repository-url> /opt/nexus-bs\n\
            Then either move the binary into that tree, or set source_dir in config:\n\
            [dashboard]\n\
            source_dir = \"/opt/nexus-bs\"\n\
         \n\
         2) If your platform can't compile (e.g. Pi Zero), update manually by downloading \
         the latest release binary from GitHub.\n\
         \n\
         Paths tried: {}",
        if tried.is_empty() { "(none)".to_string() } else { tried.join("; ") }
    ))
}

/// Run git pull + cargo build --release in a background thread.
/// Steps:
///   1. Resolve source dir (config override -> walk-up -> well-known paths -> CWD)
///   2. Validate it is a git repository
///   3. Backup config.toml -> config.toml.bak
///   4. git fetch + compare commits
///   5. git merge --ff-only origin/main
///   6. cargo build --release
///   7. systemctl restart <service>  (after short delay)
fn run_update(update: SharedUpdateState, config_path: String, source_dir_override: Option<String>) {
    macro_rules! log {
        ($update:expr, $($arg:tt)*) => {{
            let line = format!($($arg)*);
            tracing::info!("UPDATE: {}", line);
            $update.lock().unwrap().append(&line);
        }};
    }

    log!(update, "=== Nexus-BS OTA Update ===");

    // Step 1: resolve source directory. Bail out cleanly if we can't find a git repo.
    let src_dir = match resolve_source_dir(source_dir_override.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            log!(update, "ERROR: {}", e);
            update.lock().unwrap().finish(false);
            return;
        }
    };

    log!(update, "Source dir: {}", src_dir.display());

    /// Run a command, stream stdout+stderr into the log, return Ok(stdout) or Err.
    fn run_cmd_output(update: &SharedUpdateState, program: &str, args: &[&str], dir: &std::path::Path) -> Option<String> {
        let line = format!("$ {} {}", program, args.join(" "));
        tracing::info!("UPDATE: {}", line);
        update.lock().unwrap().append(&line);

        match std::process::Command::new(program).args(args).current_dir(dir).output() {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                for l in stdout.lines() {
                    update.lock().unwrap().append(l);
                }
                for l in stderr.lines() {
                    update.lock().unwrap().append(l);
                }
                if out.status.success() {
                    Some(stdout)
                } else {
                    update.lock().unwrap().append(&format!("ERROR: exited with {}", out.status));
                    update.lock().unwrap().finish(false);
                    None
                }
            }
            Err(e) => {
                update.lock().unwrap().append(&format!("ERROR: failed to run '{}': {}", program, e));
                update.lock().unwrap().finish(false);
                None
            }
        }
    }

    let src_str = src_dir.to_str().unwrap_or(".");

    // Step 2: explicit sanity check that this is a working git repo.
    // The .git existence check in resolve_source_dir() is necessary but not sufficient
    // (e.g. a corrupted repo). This catches edge cases with a clear error.
    //
    // Common edge case: Nexus-BS runs as root (e.g. via systemd) but the git clone
    // lives in a user's home directory (e.g. /home/pi/nexus-bs, owned by pi:pi).
    // Recent git versions refuse to operate on repos owned by a different user with
    // "dubious ownership" — fatal: detected dubious ownership in repository at '...'.
    // We try once first, and if we see that error, register the path as a safe.directory
    // via `git config --global --add safe.directory <path>` and retry.
    log!(update, "--- Verifying git repository ---");
    if run_cmd_output(&update, "git", &["-C", src_str, "rev-parse", "--is-inside-work-tree"], &src_dir).is_none() {
        // Check if the failure was specifically dubious ownership. The error went to the log
        // already; we look at the log content to decide whether to attempt the auto-fix.
        let saw_dubious_ownership = {
            let u = update.lock().unwrap();
            u.log.contains("dubious ownership")
        };
        if !saw_dubious_ownership {
            return;
        }
        log!(update, "");
        log!(update, "--- Detected dubious ownership — registering as safe.directory ---");
        if run_cmd_output(
            &update,
            "git",
            &["config", "--global", "--add", "safe.directory", src_str],
            &src_dir,
        )
        .is_none()
        {
            log!(update, "ERROR: could not register safe.directory automatically.");
            log!(update, "Manual fix: run this on the server as the user that runs Nexus-BS:");
            log!(update, "    git config --global --add safe.directory {}", src_str);
            return;
        }
        // Retry the verification.
        if run_cmd_output(&update, "git", &["-C", src_str, "rev-parse", "--is-inside-work-tree"], &src_dir).is_none() {
            log!(update, "ERROR: git verification still failing after safe.directory fix.");
            return;
        }
        log!(update, "✓ safe.directory registered, continuing.");
    }

    // Step 3: fetch remote without merging — just update refs
    log!(update, "--- Checking remote for updates ---");
    if run_cmd_output(&update, "git", &["-C", src_str, "fetch", "origin", "main"], &src_dir).is_none() {
        return;
    }

    // Step 4: compare local HEAD with remote origin/main
    let local_commit = run_cmd_output(&update, "git", &["-C", src_str, "rev-parse", "HEAD"], &src_dir)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if local_commit.is_empty() {
        return;
    }

    let remote_commit = run_cmd_output(&update, "git", &["-C", src_str, "rev-parse", "origin/main"], &src_dir)
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if remote_commit.is_empty() {
        return;
    }

    log!(update, "Local  commit: {}", &local_commit[..local_commit.len().min(12)]);
    log!(update, "Remote commit: {}", &remote_commit[..remote_commit.len().min(12)]);

    if local_commit == remote_commit {
        log!(update, "Already up to date — nothing to do.");
        update.lock().unwrap().finish(true);
        return;
    }

    // Step 5: show what changed
    let _ = run_cmd_output(
        &update,
        "git",
        &["-C", src_str, "log", "--oneline", &format!("HEAD..origin/main")],
        &src_dir,
    );

    // Step 6: backup config before touching anything
    let backup_path = format!("{}.bak", config_path);
    match std::fs::copy(&config_path, &backup_path) {
        Ok(_) => log!(update, "Config backed up → {}", backup_path),
        Err(e) => log!(update, "WARNING: config backup failed: {} (continuing)", e),
    }

    // Step 7: fast-forward merge (only changed files are touched on disk)
    log!(update, "--- git merge (fast-forward only) ---");
    if run_cmd_output(&update, "git", &["-C", src_str, "merge", "--ff-only", "origin/main"], &src_dir).is_none() {
        return;
    }

    // Step 8: incremental build (cargo only recompiles changed crates)
    log!(update, "--- cargo build --release (incremental) ---");
    if run_cmd_output(&update, "cargo", &["build", "--release"], &src_dir).is_none() {
        return;
    }

    // Step 9: done — schedule restart
    log!(update, "--- Build successful. Restarting service in 2s... ---");
    update.lock().unwrap().finish(true);

    crate::service_control::schedule_service_action(crate::service_control::ServiceAction::Restart, std::time::Duration::from_secs(2));
}

fn update_log(update: &SharedUpdateState, line: impl AsRef<str>) {
    let line = line.as_ref();
    tracing::info!("UPDATE: {}", line);
    update.lock().unwrap().append(line);
}

fn run_update_command(update: &SharedUpdateState, program: &str, args: &[&str]) -> bool {
    update_log(update, format!("$ {} {}", program, args.join(" ")));
    match Command::new(program).args(args).output() {
        Ok(out) => {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                update.lock().unwrap().append(line);
            }
            for line in String::from_utf8_lossy(&out.stderr).lines() {
                update.lock().unwrap().append(line);
            }
            if out.status.success() {
                true
            } else {
                update_log(update, format!("ERROR: exited with {}", out.status));
                false
            }
        }
        Err(e) => {
            update_log(update, format!("ERROR: failed to run '{}': {}", program, e));
            false
        }
    }
}

fn update_process_runs_as_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim() == "0")
        .unwrap_or(false)
}

fn run_update_command_privileged(update: &SharedUpdateState, program: &str, args: &[&str]) -> bool {
    if update_process_runs_as_root() {
        return run_update_command(update, program, args);
    }

    let mut sudo_args = Vec::with_capacity(args.len() + 2);
    sudo_args.push("-n");
    sudo_args.push(program);
    sudo_args.extend_from_slice(args);
    run_update_command(update, "sudo", &sudo_args)
}

fn run_update_command_capture(update: &SharedUpdateState, program: &str, args: &[&str]) -> Option<String> {
    update_log(update, format!("$ {} {}", program, args.join(" ")));
    match Command::new(program).args(args).output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            for line in stdout.lines() {
                update.lock().unwrap().append(line);
            }
            for line in stderr.lines() {
                update.lock().unwrap().append(line);
            }
            if out.status.success() {
                Some(stdout)
            } else {
                update_log(update, format!("ERROR: exited with {}", out.status));
                None
            }
        }
        Err(e) => {
            update_log(update, format!("ERROR: failed to run '{}': {}", program, e));
            None
        }
    }
}

fn run_deb_update(update: SharedUpdateState, deb: DebUpdateRequest) {
    update_log(&update, "=== Nexus-BS Debian package update ===");
    update_log(&update, format!("Selected version: {}", deb.version));
    update_log(&update, format!("Asset: {}", deb.asset_name));

    let work_dir = PathBuf::from("/tmp/nexus-bs-update");
    if let Err(e) = fs::create_dir_all(&work_dir) {
        update_log(&update, format!("ERROR: failed to create {}: {}", work_dir.display(), e));
        update.lock().unwrap().finish(false);
        return;
    }
    let deb_path = work_dir.join(&deb.asset_name);
    let Some(deb_path_str) = deb_path.to_str() else {
        update_log(&update, "ERROR: update path is not valid UTF-8");
        update.lock().unwrap().finish(false);
        return;
    };
    let _ = fs::remove_file(&deb_path);

    if !run_update_command(
        &update,
        "curl",
        &["-fL", "--connect-timeout", "10", "--max-time", "180", "-o", deb_path_str, &deb.url],
    ) {
        update.lock().unwrap().finish(false);
        return;
    }

    let package = run_update_command_capture(&update, "dpkg-deb", &["-f", deb_path_str, "Package"]).unwrap_or_default();
    let version = run_update_command_capture(&update, "dpkg-deb", &["-f", deb_path_str, "Version"]).unwrap_or_default();
    let arch = run_update_command_capture(&update, "dpkg-deb", &["-f", deb_path_str, "Architecture"]).unwrap_or_default();
    if package.trim() != "nexus-bs" {
        update_log(&update, format!("ERROR: unexpected package '{}'", package.trim()));
        update.lock().unwrap().finish(false);
        return;
    }
    let runtime_arch = debian_arch();
    if arch.trim() != runtime_arch {
        update_log(
            &update,
            format!(
                "ERROR: package architecture '{}' does not match host '{}'",
                arch.trim(),
                runtime_arch
            ),
        );
        update.lock().unwrap().finish(false);
        return;
    }
    update_log(&update, format!("Installing nexus-bs {} ({})", version.trim(), arch.trim()));

    if !run_update_stop_bs_services(&update) {
        update.lock().unwrap().finish(false);
        return;
    }
    if !run_update_command_privileged(&update, "apt-get", &["install", "-y", deb_path_str]) {
        update.lock().unwrap().finish(false);
        return;
    }
    if !run_update_post_install_start(&update) {
        update.lock().unwrap().finish(false);
        return;
    }
    if !schedule_dashboard_restart_after_deb_update(&update) {
        update.lock().unwrap().finish(false);
        return;
    }
    update.lock().unwrap().finish(true);
}

fn run_update_stop_bs_services(update: &SharedUpdateState) -> bool {
    update_log(update, "Stopping Nexus-BS core/control services before package install");
    run_update_command_privileged(update, "systemctl", &["stop", "nexus-bs.service", "nexus-bs-control.service"])
}

fn run_update_post_install_start(update: &SharedUpdateState) -> bool {
    update_log(update, "Reloading systemd units after package install");
    let _ = run_update_command_privileged(update, "systemctl", &["daemon-reload"]);

    update_log(update, "Ensuring Nexus-BS core/control services are fully stopped after package install");
    if !run_update_command_privileged(update, "systemctl", &["stop", "nexus-bs.service", "nexus-bs-control.service"]) {
        return false;
    }

    update_log(update, "Clearing stale Nexus-BS systemd failure state");
    let _ = run_update_command_privileged(
        update,
        "systemctl",
        &["reset-failed", "nexus-bs.service", "nexus-bs-control.service", "nexus-bs-dashboard.service"],
    );

    update_log(update, "Waiting 5s before starting Nexus-BS services");
    std::thread::sleep(std::time::Duration::from_secs(5));

    update_log(update, "Starting Nexus-BS control service after package install");
    if !run_update_command_privileged(update, "systemctl", &["start", "nexus-bs-control.service"]) {
        return false;
    }

    update_log(update, "Starting Nexus-BS core service after package install");
    if !run_update_command_privileged(update, "systemctl", &["start", "nexus-bs.service"]) {
        return false;
    }

    update_log(
        update,
        "Nexus-BS core/control services are online after package install",
    );
    true
}

fn schedule_dashboard_restart_after_deb_update(update: &SharedUpdateState) -> bool {
    let command = "sleep 3; if [ \"$(id -u)\" = 0 ]; then systemctl --no-block restart nexus-bs-dashboard.service; else sudo -n systemctl --no-block restart nexus-bs-dashboard.service; fi";

    update_log(
        update,
        "Scheduling dashboard service restart in 3s to load the installed version",
    );
    update_log(update, format!("$ sh -c '{}'", command));
    match Command::new("sh").arg("-c").arg(command).spawn() {
        Ok(_) => {
            update_log(update, "Dashboard restart queued; browser may reconnect shortly.");
            true
        }
        Err(e) => {
            update_log(update, format!("ERROR: failed to queue dashboard restart: {}", e));
            false
        }
    }
}

pub struct DashboardServer {
    pub state: DashboardState,
    clients: WsClients,
    config_path: String,
    /// Shared stack config — used to read live_sds_queue from StackState.
    shared_config: Option<tetra_config::bluestation::SharedConfig>,
    cmd_tx: Option<CmdSender>,
    rf_cmd_tx: Option<CmdSender>,
    phy_cmd_tx: Option<CmdSender>,
    update_state: SharedUpdateState,
    /// Optional override for the OTA update source directory.
    /// If None, the update routine auto-detects.
    source_dir_override: Option<String>,
    /// Optional external dashboard asset directory.
    /// `/api/*`, `/ws`, `/login` stay inside this Rust process; only browser
    /// assets are read from this directory when configured.
    static_dir: Option<String>,
    /// Authentication credentials. None = no auth (open access). When set, requests
    /// must carry a valid `fs_session` cookie obtained from `POST /api/login`.
    auth: Option<(String, String)>,
    /// In-memory session store backing the cookie auth.
    sessions: SharedSessionStore,
    /// Last time a ts_voice WS message was broadcast per TS (indexed 0..3 for TS1..TS4)
    ts_last_broadcast: std::sync::Mutex<[std::time::Instant; 4]>,
    active_http_connections: Arc<AtomicUsize>,
    bs_started_at: Instant,
}

impl DashboardServer {
    pub fn new(config_path: String) -> Self {
        let now = std::time::Instant::now();
        Self {
            state: Arc::new(RwLock::new(DashboardStateInner::new(config_path.clone()))),
            clients: Arc::new(Mutex::new(Vec::new())),
            config_path,
            shared_config: None,
            cmd_tx: None,
            rf_cmd_tx: None,
            phy_cmd_tx: None,
            update_state: Arc::new(Mutex::new(UpdateState::new())),
            source_dir_override: None,
            static_dir: None,
            auth: None,
            sessions: Arc::new(Mutex::new(SessionStore::new())),
            ts_last_broadcast: std::sync::Mutex::new([now; 4]),
            active_http_connections: Arc::new(AtomicUsize::new(0)),
            bs_started_at: now,
        }
    }

    pub fn set_cmd_sender(&mut self, tx: CmdSender) {
        self.cmd_tx = Some(tx);
    }

    pub fn set_rf_cmd_sender(&mut self, tx: CmdSender) {
        self.rf_cmd_tx = Some(tx);
    }

    pub fn set_phy_cmd_sender(&mut self, tx: CmdSender) {
        self.phy_cmd_tx = Some(tx);
    }

    /// Provide the SharedConfig so the dashboard can read live SDS queue state.
    pub fn set_shared_config(&mut self, cfg: tetra_config::bluestation::SharedConfig) {
        self.shared_config = Some(cfg);
    }

    /// Configure an explicit source directory for OTA updates.
    pub fn set_source_dir(&mut self, source_dir: Option<String>) {
        self.source_dir_override = source_dir;
    }

    /// Configure an explicit external asset directory for the dashboard SPA.
    pub fn set_static_dir(&mut self, static_dir: Option<String>) {
        self.static_dir = static_dir;
    }

    /// Configure dashboard form-login credentials.
    pub fn set_auth(&mut self, auth: Option<(String, String)>) {
        self.auth = auth;
    }

    /// Mark that the stack started on the fallback config, with the reason why.
    /// The dashboard will display a persistent warning banner.
    pub fn set_fallback_config(&self, reason: String) {
        let mut s = self.state.write().unwrap();
        s.fallback_config_active = true;
        s.fallback_config_reason = reason;
    }

    pub fn start(&mut self, bind: &str, port: u16) {
        let addr = format!("{}:{}", bind, port);
        let state = Arc::clone(&self.state);
        let clients = Arc::clone(&self.clients);
        let config_path = self.config_path.clone();
        let cmd_tx: Arc<Mutex<Option<CmdSender>>> = Arc::new(Mutex::new(self.cmd_tx.take()));
        let rf_cmd_tx: Arc<Mutex<Option<CmdSender>>> = Arc::new(Mutex::new(self.rf_cmd_tx.take()));
        let phy_cmd_tx: Arc<Mutex<Option<CmdSender>>> = Arc::new(Mutex::new(self.phy_cmd_tx.take()));
        let update_state = Arc::clone(&self.update_state);
        let source_dir_override = self.source_dir_override.clone();
        let static_dir = self.static_dir.clone();
        let auth = self.auth.clone();
        let shared_config = self.shared_config.clone();
        let sessions = Arc::clone(&self.sessions);
        let active_http_connections = Arc::clone(&self.active_http_connections);
        let bs_started_at = self.bs_started_at;

        std::thread::Builder::new()
            .name("dashboard-server".into())
            .spawn(move || {
                let listener = match TcpListener::bind(&addr) {
                    Ok(l) => {
                        tracing::info!("Dashboard listening on http://{}", addr);
                        l
                    }
                    Err(e) => {
                        tracing::error!("Dashboard failed to bind {}: {}", addr, e);
                        return;
                    }
                };
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let current = active_http_connections.fetch_add(1, Ordering::AcqRel);
                    if current >= DASHBOARD_HTTP_CONNECTION_MAX {
                        active_http_connections.fetch_sub(1, Ordering::AcqRel);
                        tracing::warn!(
                            "Dashboard: rejecting connection over active limit {}",
                            DASHBOARD_HTTP_CONNECTION_MAX
                        );
                        http_response(stream, 503, "dashboard connection limit reached");
                        continue;
                    }

                    let state = Arc::clone(&state);
                    let clients = Arc::clone(&clients);
                    let config_path = config_path.clone();
                    let cmd_tx = Arc::clone(&cmd_tx);
                    let rf_cmd_tx = Arc::clone(&rf_cmd_tx);
                    let phy_cmd_tx = Arc::clone(&phy_cmd_tx);
                    let update_state = Arc::clone(&update_state);
                    let source_dir_override = source_dir_override.clone();
                    let static_dir = static_dir.clone();
                    let auth = auth.clone();
                    let shared_config = shared_config.clone();
                    let sessions = Arc::clone(&sessions);
                    let bs_started_at = bs_started_at;
                    let active_for_guard = Arc::clone(&active_http_connections);
                    let active_for_spawn_error = Arc::clone(&active_http_connections);
                    if let Err(e) = std::thread::Builder::new().name("dashboard-conn".into()).spawn(move || {
                        let _guard = DashboardConnectionGuard::new(active_for_guard);
                        handle_connection(
                            stream,
                            state,
                            clients,
                            config_path,
                            cmd_tx,
                            rf_cmd_tx,
                            phy_cmd_tx,
                            update_state,
                            source_dir_override,
                            static_dir,
                            auth,
                            shared_config,
                            sessions,
                            bs_started_at,
                        )
                    }) {
                        active_for_spawn_error.fetch_sub(1, Ordering::AcqRel);
                        tracing::warn!("Dashboard: failed to spawn connection handler: {}", e);
                    }
                }
            })
            .expect("failed to spawn dashboard thread");
    }

    pub fn handle_telemetry(&self, event: TelemetryEvent) {
        let msg = event_to_ws_msg(&event);
        {
            let mut s = self.state.write().unwrap();
            match &event {
                TelemetryEvent::MsRegistration { issi } => {
                    let entry = s.ms_map.entry(*issi).or_insert_with(|| empty_ms_entry(*issi));
                    entry.registered_at = Instant::now();
                    entry.last_seen = Instant::now();
                    s.push_log("INFO", format!("MS {} registered", issi));
                }
                TelemetryEvent::MsDeregistration { issi } => {
                    s.ms_map.remove(issi);
                    s.push_log("INFO", format!("MS {} deregistered", issi));
                }
                TelemetryEvent::MsGroupAttach { issi, gssis } => {
                    let e = s.ms_map.entry(*issi).or_insert_with(|| empty_ms_entry(*issi));
                    for g in gssis {
                        if !e.groups.contains(g) {
                            e.groups.push(*g);
                        }
                    }
                }
                TelemetryEvent::MsGroupsSnapshot { issi, gssis } => {
                    let e = s.ms_map.entry(*issi).or_insert_with(|| empty_ms_entry(*issi));
                    e.groups = gssis.clone();
                }
                TelemetryEvent::MsGroupDetach { issi, gssis } => {
                    if let Some(e) = s.ms_map.get_mut(issi) {
                        e.groups.retain(|g| !gssis.contains(g));
                    }
                }
                TelemetryEvent::MsRssi { issi, rssi_dbfs } => {
                    let e = s.ms_map.entry(*issi).or_insert_with(|| empty_ms_entry(*issi));
                    e.rssi_dbfs = Some(*rssi_dbfs);
                    e.last_seen = Instant::now();
                }
                TelemetryEvent::MsEnergySaving {
                    issi,
                    mode,
                    frame,
                    multiframe,
                } => {
                    let e = s.ms_map.entry(*issi).or_insert_with(|| empty_ms_entry(*issi));
                    e.energy_saving_mode = *mode;
                    e.energy_saving_frame = *frame;
                    e.energy_saving_multiframe = *multiframe;
                    e.last_seen = Instant::now();
                }
                TelemetryEvent::GroupCallStarted {
                    call_id,
                    gssi,
                    caller_issi,
                    ts,
                } => {
                    if let Some(e) = s.ms_map.get_mut(caller_issi) {
                        e.last_seen = Instant::now();
                        if !e.groups.contains(gssi) {
                            e.groups.push(*gssi);
                        }
                    }
                    s.calls.insert(
                        *call_id,
                        CallEntry {
                            call_id: *call_id,
                            is_group: true,
                            gssi: *gssi,
                            caller_issi: *caller_issi,
                            called_issi: 0,
                            speaker_issi: Some(*caller_issi),
                            started_at: Instant::now(),
                            simplex: false,
                            ts: *ts,
                            secondary_ts: None,
                        },
                    );
                    s.push_last_heard(*caller_issi, "call_group", *gssi);
                    s.push_log("INFO", format!("Group call {} started: {} -> GSSI {}", call_id, caller_issi, gssi));
                }
                TelemetryEvent::GroupCallEnded { call_id, gssi: _ } => {
                    s.calls.remove(call_id);
                    s.push_log("INFO", format!("Group call {} ended", call_id));
                }
                TelemetryEvent::GroupCallSpeakerChanged {
                    call_id,
                    gssi,
                    speaker_issi,
                } => {
                    if let Some(e) = s.ms_map.get_mut(speaker_issi) {
                        e.last_seen = Instant::now();
                        if !e.groups.contains(gssi) {
                            e.groups.push(*gssi);
                        }
                    }
                    if let Some(c) = s.calls.get_mut(call_id) {
                        c.speaker_issi = Some(*speaker_issi);
                        c.caller_issi = *speaker_issi;
                        c.started_at = Instant::now();
                    }
                    s.push_last_heard(*speaker_issi, "call_group", *gssi);
                }
                TelemetryEvent::IndividualCallStarted {
                    call_id,
                    calling_issi,
                    called_issi,
                    simplex,
                    ts,
                    secondary_ts,
                } => {
                    if let Some(caller) = s.ms_map.get_mut(calling_issi) {
                        caller.last_seen = Instant::now();
                    }
                    if let Some(called) = s.ms_map.get_mut(called_issi) {
                        called.last_seen = Instant::now();
                    }
                    s.calls.insert(
                        *call_id,
                        CallEntry {
                            call_id: *call_id,
                            is_group: false,
                            gssi: 0,
                            caller_issi: *calling_issi,
                            called_issi: *called_issi,
                            speaker_issi: None,
                            started_at: Instant::now(),
                            simplex: *simplex,
                            ts: *ts,
                            secondary_ts: *secondary_ts,
                        },
                    );
                    s.push_last_heard(*calling_issi, individual_call_last_heard_activity(*simplex), *called_issi);
                    s.push_log("INFO", format!("P2P call {} started: {} -> {}", call_id, calling_issi, called_issi));
                }
                TelemetryEvent::IndividualCallEnded { call_id } => {
                    s.calls.remove(call_id);
                    s.push_log("INFO", format!("P2P call {} ended", call_id));
                }
                TelemetryEvent::BrewConnected { connected, server_version } => {
                    s.brew_online = *connected;
                    if *connected {
                        s.brew_version = *server_version;
                    }
                }
                TelemetryEvent::SdsActivity { source_issi, dest_issi } => {
                    if let Some(source) = s.ms_map.get_mut(source_issi) {
                        source.last_seen = Instant::now();
                    }
                    s.push_last_heard(*source_issi, "sds", *dest_issi);
                }
                TelemetryEvent::TsVoiceActivity { .. } => {
                    // Handled below with rate limiting — no state update needed
                }
                TelemetryEvent::TxVisual {
                    sample_rate,
                    center_freq_hz,
                    rms_dbfs,
                    peak_dbfs,
                    spectrum_db_tenths,
                    constellation_iq,
                } => {
                    // Cache the visual snapshot so newly-connected dashboard clients
                    // see something on the RF page before the next ~200 ms emit cycle.
                    s.last_tx_visual = Some(crate::net_dashboard::state::TxVisualSnapshot {
                        sample_rate: *sample_rate,
                        center_freq_hz: *center_freq_hz,
                        rms_dbfs: *rms_dbfs,
                        peak_dbfs: *peak_dbfs,
                        spectrum_db_tenths: spectrum_db_tenths.clone(),
                        constellation_iq: constellation_iq.clone(),
                    });
                }
                TelemetryEvent::TxQuality {
                    papr_db,
                    evm_pct,
                    dc_offset_i,
                    dc_offset_q,
                    iq_amplitude_imbalance_db,
                    iq_phase_imbalance_deg,
                    carrier_leakage_db,
                    occupied_bandwidth_hz,
                } => {
                    // Cache the quality numbers so late-joining clients get them
                    // straight away rather than waiting up to a second.
                    s.last_tx_quality = Some(crate::net_dashboard::state::TxQualitySnapshot {
                        papr_db: *papr_db,
                        evm_pct: *evm_pct,
                        dc_offset_i: *dc_offset_i,
                        dc_offset_q: *dc_offset_q,
                        iq_amplitude_imbalance_db: *iq_amplitude_imbalance_db,
                        iq_phase_imbalance_deg: *iq_phase_imbalance_deg,
                        carrier_leakage_db: *carrier_leakage_db,
                        occupied_bandwidth_hz: *occupied_bandwidth_hz,
                    });
                }
                TelemetryEvent::SdrHealth {
                    temperature_c,
                    tx_gains,
                    rx_gains,
                } => {
                    s.last_sdr_health = Some(crate::net_dashboard::state::SdrHealthSnapshot {
                        temperature_c: *temperature_c,
                        tx_gains: tx_gains.clone(),
                        rx_gains: rx_gains.clone(),
                    });
                }
                TelemetryEvent::SysHealth { total_power_w, sensors } => {
                    s.last_sys_health = Some(crate::net_dashboard::state::SysHealthSnapshot {
                        total_power_w: *total_power_w,
                        sensors: sensors.clone(),
                    });
                }
                TelemetryEvent::HealthSnapshot(snapshot) => {
                    s.last_health = Some(snapshot.clone());
                }
            }
        }
        if let Some(json) = msg {
            self.broadcast(&json);
        }
        // TsVoiceActivity: rate-limit broadcasts to max 4/sec per TS (250ms cooldown)
        if let TelemetryEvent::TsVoiceActivity { ts } = &event {
            let idx = (ts.saturating_sub(1) as usize).min(3);
            let now = std::time::Instant::now();
            if let Ok(mut arr) = self.ts_last_broadcast.try_lock() {
                if now.duration_since(arr[idx]) >= std::time::Duration::from_millis(250) {
                    arr[idx] = now;
                    drop(arr);
                    if let Some(json) = event_to_ws_msg(&event) {
                        self.broadcast(&json);
                    }
                }
            }
        }
    }

    pub fn reset_runtime_snapshot(&self, reason: &str) {
        {
            let mut s = self.state.write().unwrap();
            s.ms_map.clear();
            s.calls.clear();
            s.last_heard.clear();
            s.brew_online = false;
            s.brew_version = 0;
            s.fallback_config_active = false;
            s.fallback_config_reason.clear();
            s.last_tx_visual = None;
            s.last_tx_quality = None;
            s.last_sdr_health = None;
            s.last_sys_health = None;
            s.last_health = None;
            s.push_log("INFO", format!("Dashboard runtime snapshot reset: {}", reason));
        }
        self.broadcast(&dashboard_snapshot_json(&self.state));
    }

    pub fn push_log(&self, level: &str, msg: String) {
        let entry = {
            let mut s = self.state.write().unwrap();
            s.push_log(level, msg);
            s.log_ring.back().cloned()
        };
        if let Some(entry) = entry {
            if let Ok(json) = serde_json::to_string(&serde_json::json!({
                "type": "log", "ts": entry.ts, "level": entry.level, "msg": entry.msg
            })) {
                self.broadcast(&json);
            }
        }
    }

    fn broadcast(&self, msg: &str) {
        let mut clients = self.clients.lock().unwrap();
        clients.retain(|tx| match tx.try_send(msg.to_owned()) {
            Ok(()) => true,
            Err(crossbeam_channel::TrySendError::Full(_)) => {
                tracing::warn!(
                    "dashboard WS client broadcast queue exceeded {} messages; dropping slow client",
                    DASHBOARD_WS_BROADCAST_QUEUE_MAX
                );
                false
            }
            Err(crossbeam_channel::TrySendError::Disconnected(_)) => false,
        });
    }
}

struct DashboardConnectionGuard {
    active: Arc<AtomicUsize>,
}

impl DashboardConnectionGuard {
    fn new(active: Arc<AtomicUsize>) -> Self {
        Self { active }
    }
}

impl Drop for DashboardConnectionGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

fn empty_ms_entry(issi: u32) -> MsEntry {
    MsEntry {
        issi,
        groups: Vec::new(),
        rssi_dbfs: None,
        registered_at: Instant::now(),
        last_seen: Instant::now(),
        energy_saving_mode: 0,
        energy_saving_frame: None,
        energy_saving_multiframe: None,
    }
}

fn event_to_ws_msg(event: &TelemetryEvent) -> Option<String> {
    let v = match event {
        TelemetryEvent::MsRegistration { issi } => serde_json::json!({"type":"ms_registered","issi":issi}),
        TelemetryEvent::MsDeregistration { issi } => serde_json::json!({"type":"ms_deregistered","issi":issi}),
        TelemetryEvent::MsGroupAttach { issi, gssis } => serde_json::json!({"type":"ms_groups","issi":issi,"groups":gssis}),
        TelemetryEvent::MsGroupDetach { issi, gssis } => serde_json::json!({"type":"ms_groups_detach","issi":issi,"groups":gssis}),
        TelemetryEvent::MsGroupsSnapshot { issi, gssis } => serde_json::json!({"type":"ms_groups_all","issi":issi,"groups":gssis}),
        TelemetryEvent::MsRssi { issi, rssi_dbfs } => serde_json::json!({"type":"ms_rssi","issi":issi,"rssi_dbfs":rssi_dbfs}),
        TelemetryEvent::MsEnergySaving {
            issi,
            mode,
            frame,
            multiframe,
        } => serde_json::json!({"type":"ms_energy_saving","issi":issi,"mode":mode,"frame":frame,"multiframe":multiframe}),
        TelemetryEvent::GroupCallStarted {
            call_id,
            gssi,
            caller_issi,
            ts,
        } => {
            serde_json::json!({"type":"call_started","call_id":call_id,"call_type":"group","gssi":gssi,"caller_issi":caller_issi,"active_speaker":caller_issi,"ts":ts,"last_heard":{"issi":caller_issi,"activity":"call_group","dest":gssi}})
        }
        TelemetryEvent::GroupCallEnded { call_id, gssi } => {
            serde_json::json!({"type":"call_ended","call_id":call_id,"call_type":"group","gssi":gssi})
        }
        TelemetryEvent::GroupCallSpeakerChanged {
            call_id,
            gssi,
            speaker_issi,
        } => {
            serde_json::json!({"type":"speaker_changed","call_id":call_id,"gssi":gssi,"speaker_issi":speaker_issi,"last_heard":{"issi":speaker_issi,"activity":"call_group","dest":gssi}})
        }
        TelemetryEvent::IndividualCallStarted {
            call_id,
            calling_issi,
            called_issi,
            simplex,
            ts,
            secondary_ts,
        } => {
            let activity = individual_call_last_heard_activity(*simplex);
            serde_json::json!({"type":"call_started","call_id":call_id,"call_type":"individual","caller_issi":calling_issi,"called_issi":called_issi,"simplex":simplex,"ts":ts,"secondary_ts":secondary_ts,"last_heard":{"issi":calling_issi,"activity":activity,"dest":called_issi}})
        }
        TelemetryEvent::IndividualCallEnded { call_id } => serde_json::json!({"type":"call_ended","call_id":call_id}),
        TelemetryEvent::BrewConnected { connected, server_version } => {
            serde_json::json!({"type":"brew_status","connected":connected,"brew_version":server_version})
        }
        TelemetryEvent::SdsActivity { source_issi, dest_issi } => {
            serde_json::json!({"type":"last_heard","issi":source_issi,"activity":"sds","dest":dest_issi})
        }
        TelemetryEvent::TsVoiceActivity { ts } => serde_json::json!({"type":"ts_voice","ts":ts}),
        TelemetryEvent::TxVisual {
            sample_rate,
            center_freq_hz,
            rms_dbfs,
            peak_dbfs,
            spectrum_db_tenths,
            constellation_iq,
        } => serde_json::json!({
            "type": "tx_visual",
            "sample_rate": sample_rate,
            "center_freq_hz": center_freq_hz,
            "rms_dbfs": rms_dbfs,
            "peak_dbfs": peak_dbfs,
            "spectrum_db_tenths": spectrum_db_tenths,
            "constellation_iq": constellation_iq,
        }),
        TelemetryEvent::TxQuality {
            papr_db,
            evm_pct,
            dc_offset_i,
            dc_offset_q,
            iq_amplitude_imbalance_db,
            iq_phase_imbalance_deg,
            carrier_leakage_db,
            occupied_bandwidth_hz,
        } => serde_json::json!({
            "type": "tx_quality",
            "papr_db": papr_db,
            "evm_pct": evm_pct,
            "dc_offset_i": dc_offset_i,
            "dc_offset_q": dc_offset_q,
            "iq_amplitude_imbalance_db": iq_amplitude_imbalance_db,
            "iq_phase_imbalance_deg": iq_phase_imbalance_deg,
            "carrier_leakage_db": carrier_leakage_db,
            "occupied_bandwidth_hz": occupied_bandwidth_hz,
        }),
        TelemetryEvent::SdrHealth {
            temperature_c,
            tx_gains,
            rx_gains,
        } => serde_json::json!({
            "type": "sdr_health",
            "temperature_c": temperature_c,
            "tx_gains": tx_gains,
            "rx_gains": rx_gains,
        }),
        TelemetryEvent::SysHealth { total_power_w, sensors } => serde_json::json!({
            "type": "sys_health",
            "total_power_w": total_power_w,
            "sensors": sensors,
        }),
        TelemetryEvent::HealthSnapshot(snapshot) => serde_json::json!({
            "type": "health",
            "snapshot": snapshot,
        }),
    };
    serde_json::to_string(&v).ok()
}

fn individual_call_last_heard_activity(simplex: bool) -> &'static str {
    if simplex { "call_p2p_simplex" } else { "call_p2p_duplex" }
}

// ---------------------------------------------------------------------------
// HTTP Basic Auth helpers
// ---------------------------------------------------------------------------

/// Parse the `Authorization: Basic <base64>` header from raw HTTP headers string.
/// Returns `Some((username, password))` on success, `None` if absent or malformed.
///
/// Kept for potential future use (e.g. an opt-in scripting endpoint). The dashboard
/// now uses cookie-based sessions, so this is currently unreferenced.
#[allow(dead_code)]
fn parse_basic_auth(headers: &str) -> Option<(String, String)> {
    for line in headers.lines() {
        let lower = line.to_lowercase();
        if lower.starts_with("authorization:") {
            let value = line[14..].trim();
            if let Some(encoded) = value.strip_prefix("Basic ").or_else(|| value.strip_prefix("basic ")) {
                use base64::Engine;
                let decoded = base64::engine::general_purpose::STANDARD.decode(encoded.trim()).ok()?;
                let s = String::from_utf8(decoded).ok()?;
                let mut parts = s.splitn(2, ':');
                let user = parts.next()?.to_string();
                let pass = parts.next().unwrap_or("").to_string();
                return Some((user, pass));
            }
        }
    }
    None
}

/// Constant-time byte slice comparison to mitigate timing attacks.
/// Returns true iff a == b in length and content.
fn timing_safe_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Send an HTTP 401 Unauthorized response that triggers the browser's native
/// Basic Auth dialog. Unused since the switch to cookie sessions.
#[allow(dead_code)]
fn http_response_401(mut stream: TcpStream) {
    let body = "Unauthorized";
    let resp = format!(
        "HTTP/1.1 401 Unauthorized\r\n\
         WWW-Authenticate: Basic realm=\"Nexus-BS Dashboard\", charset=\"UTF-8\"\r\n\
         Content-Type: text/plain\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
}

/// Send a ControlCommand through the dashboard -> CMCE channel, best-effort.
fn send_control_cmd(cmd_tx: &Arc<Mutex<Option<CmdSender>>>, cmd: ControlCommand) -> bool {
    if let Ok(guard) = cmd_tx.lock() {
        if let Some(ref tx) = *guard {
            return tx.try_send(cmd).is_ok();
        }
    }
    false
}

fn serve_service_command(stream: TcpStream, cmd_tx: &Arc<Mutex<Option<CmdSender>>>, label: &str, cmd: ControlCommand) {
    let accepted = send_control_cmd(cmd_tx, cmd);
    if accepted {
        tracing::warn!("Dashboard: service {} requested", label);
        let body = serde_json::to_string(&serde_json::json!({
            "ok": true,
            "action": label,
            "message": format!("{} queued", label),
        }))
        .unwrap_or_else(|_| r#"{"ok":true}"#.to_string());
        http_json_response(stream, 200, &body);
    } else {
        let body = serde_json::to_string(&serde_json::json!({
            "ok": false,
            "action": label,
            "error": "control channel unavailable",
        }))
        .unwrap_or_else(|_| r#"{"ok":false,"error":"control channel unavailable"}"#.to_string());
        http_json_response(stream, 503, &body);
    }
}

#[derive(Debug, Clone)]
struct EasyStartRequest {
    mcc: u16,
    mnc: u16,
    timezone: String,
    tx_freq: u64,
    duplex_spacing: u8,
    custom_spacing_enabled: bool,
    custom_spacing_hz: Option<u64>,
    brew_enabled: bool,
    brew_host: String,
    brew_port: u16,
    brew_tls: bool,
    brew_username: u32,
    brew_password: String,
}

#[derive(Debug, Clone, Copy)]
struct EasyStartCellPlan {
    freq_band: u8,
    main_carrier: i64,
    freq_offset: i64,
    rx_freq: u64,
    reverse_operation: bool,
    spacing_hz: u64,
}

fn config_dir_for(config_path: &str) -> PathBuf {
    Path::new(config_path).parent().unwrap_or_else(|| Path::new(".")).to_path_buf()
}

fn easy_start_marker_path(config_path: &str, marker: &str) -> PathBuf {
    config_dir_for(config_path).join(marker)
}

fn easy_start_completed(config_path: &str) -> bool {
    easy_start_marker_path(config_path, EASY_START_DONE_MARKER).is_file()
        || easy_start_marker_path(config_path, EASY_START_SKIP_MARKER).is_file()
}

fn mark_easy_start(config_path: &str, marker: &str) -> Result<(), String> {
    write_dashboard_config_file(&easy_start_marker_path(config_path, marker), "done\n")
        .map_err(|e| format!("failed to save Easy Start state: {}", e))
}

fn parse_easy_start_request(value: &serde_json::Value) -> Result<EasyStartRequest, String> {
    fn str_field(value: &serde_json::Value, key: &str) -> String {
        value.get(key).and_then(serde_json::Value::as_str).unwrap_or("").trim().to_string()
    }
    fn bool_field(value: &serde_json::Value, key: &str, default: bool) -> bool {
        value.get(key).and_then(serde_json::Value::as_bool).unwrap_or(default)
    }
    fn u64_field(value: &serde_json::Value, key: &str) -> Result<u64, String> {
        value
            .get(key)
            .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok())))
            .ok_or_else(|| format!("{} is required", key))
    }

    let mcc = u64_field(value, "mcc")?;
    let mnc = u64_field(value, "mnc")?;
    let tx_freq = u64_field(value, "tx_freq")?;
    let duplex_spacing = u64_field(value, "duplex_spacing")?;
    let custom_spacing_enabled = bool_field(value, "custom_spacing_enabled", false);
    let custom_spacing_hz = value
        .get("custom_spacing_hz")
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok())))
        .filter(|v| *v > 0);
    let timezone = str_field(value, "timezone");
    let brew_enabled = bool_field(value, "brew_enabled", false);
    let brew_host = str_field(value, "brew_host");
    let brew_port = value
        .get("brew_port")
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok())))
        .unwrap_or(443);
    let brew_tls = bool_field(value, "brew_tls", true);
    let brew_username = value
        .get("brew_username")
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok())))
        .unwrap_or(0);
    let brew_password = str_field(value, "brew_password");

    if !(1..=999).contains(&mcc) {
        return Err("MCC must be a number from 1 to 999".to_string());
    }
    if !(0..=9999).contains(&mnc) {
        return Err("MNC must be a number from 0 to 9999".to_string());
    }
    if timezone.parse::<chrono_tz::Tz>().is_err() {
        return Err("Timezone must look like Europe/Bucharest".to_string());
    }
    if !(300_000_000..=999_999_999).contains(&tx_freq) {
        return Err("TX frequency must be in Hz, for example 438025000".to_string());
    }
    if !(1..=15).contains(&duplex_spacing) {
        return Err("Duplex spacing must be a small TETRA spacing code, for example 1".to_string());
    }
    if custom_spacing_enabled {
        let spacing = custom_spacing_hz.ok_or_else(|| "Custom spacing needs a value in Hz".to_string())?;
        if !(1_000_000..=20_000_000).contains(&spacing) {
            return Err("Custom spacing must be between 1000000 and 20000000 Hz".to_string());
        }
    }
    if brew_enabled {
        if brew_host.is_empty() || brew_host.len() > 253 || brew_host.contains('/') || brew_host.contains(' ') {
            return Err("Brew host must be a hostname like core.tetrapack.online".to_string());
        }
        if brew_port == 0 || brew_port > u16::MAX as u64 {
            return Err("Brew port must be 1-65535".to_string());
        }
        if brew_username == 0 || brew_username > u32::MAX as u64 {
            return Err("Brew username must be your numeric SSI".to_string());
        }
        if brew_password.is_empty() {
            return Err("Brew password is required when Brew is enabled".to_string());
        }
    }

    Ok(EasyStartRequest {
        mcc: mcc as u16,
        mnc: mnc as u16,
        timezone,
        tx_freq,
        duplex_spacing: duplex_spacing as u8,
        custom_spacing_enabled,
        custom_spacing_hz,
        brew_enabled,
        brew_host,
        brew_port: brew_port as u16,
        brew_tls,
        brew_username: brew_username as u32,
        brew_password,
    })
}

fn spacing_hz_for(code: u8, custom_enabled: bool, custom: Option<u64>) -> u64 {
    if custom_enabled {
        return custom.unwrap_or(7_000_000);
    }
    match code {
        1 => 7_000_000,
        2 => 10_000_000,
        3 => 45_000_000,
        _ => 7_000_000,
    }
}

fn easy_start_cell_plan(req: &EasyStartRequest) -> Result<EasyStartCellPlan, String> {
    let freq_band = (req.tx_freq / 100_000_000) as u8;
    let band_base = u64::from(freq_band) * 100_000_000;
    if band_base == 0 || req.tx_freq <= band_base {
        return Err("TX frequency is outside a supported TETRA band".to_string());
    }
    let offset_candidates = [0_i64, 6250, -6250, 12500];
    let mut best: Option<(u64, usize, i64, i64)> = None;
    for (rank, offset) in offset_candidates.iter().enumerate() {
        let relative = req.tx_freq as i64 - band_base as i64 - *offset;
        let carrier = ((relative as f64) / 25_000.0).round() as i64;
        if carrier < 0 {
            continue;
        }
        let reconstructed = band_base as i64 + *offset + carrier * 25_000;
        let error = (reconstructed - req.tx_freq as i64).unsigned_abs();
        let candidate = (error, rank, carrier, *offset);
        if best.map(|current| candidate < current).unwrap_or(true) {
            best = Some(candidate);
        }
    }
    let Some((error, _, main_carrier, freq_offset)) = best else {
        return Err("Could not calculate TETRA carrier from TX frequency".to_string());
    };
    if error > 6250 {
        return Err("TX frequency is not close to a TETRA 25 kHz raster".to_string());
    }
    let spacing_hz = spacing_hz_for(req.duplex_spacing, req.custom_spacing_enabled, req.custom_spacing_hz);
    if req.tx_freq <= spacing_hz {
        return Err("Duplex spacing is larger than TX frequency".to_string());
    }
    Ok(EasyStartCellPlan {
        freq_band,
        main_carrier,
        freq_offset,
        rx_freq: req.tx_freq - spacing_hz,
        reverse_operation: false,
        spacing_hz,
    })
}

fn replace_toml_key_in_section(toml: &mut String, section: Option<&str>, key: &str, value: &str) {
    let mut output = String::with_capacity(toml.len() + value.len());
    let mut current_section: Option<String> = None;
    let mut replaced = false;
    for line in toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            current_section = Some(trimmed.trim_matches(['[', ']']).to_string());
        }
        let section_matches = match section {
            Some(expected) => current_section.as_deref() == Some(expected),
            None => current_section.is_none(),
        };
        if !replaced && section_matches && trimmed.starts_with(key) {
            let after_key = trimmed.get(key.len()..).unwrap_or("");
            if after_key.trim_start().starts_with('=') {
                let indent = line.chars().take_while(|c| c.is_whitespace()).collect::<String>();
                output.push_str(&format!("{indent}{key} = {value}\n"));
                replaced = true;
                continue;
            }
        }
        output.push_str(line);
        output.push('\n');
    }
    *toml = output;
}

fn remove_toml_section(toml: &str, section: &str) -> String {
    let header = format!("[{section}]");
    let mut output = String::new();
    let mut skipping = false;
    for line in toml.lines() {
        let trimmed = line.trim();
        if trimmed == header {
            skipping = true;
            continue;
        }
        if skipping && trimmed.starts_with('[') && trimmed.ends_with(']') {
            skipping = false;
        }
        if !skipping {
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

fn render_easy_start_config(req: &EasyStartRequest) -> Result<(String, EasyStartCellPlan), String> {
    let plan = easy_start_cell_plan(req)?;
    let mut toml = EASY_START_TEMPLATE_TOML.to_string();
    replace_toml_key_in_section(&mut toml, Some("phy_io.soapysdr"), "tx_freq", &req.tx_freq.to_string());
    replace_toml_key_in_section(&mut toml, Some("phy_io.soapysdr"), "rx_freq", &plan.rx_freq.to_string());
    replace_toml_key_in_section(&mut toml, Some("net_info"), "mcc", &req.mcc.to_string());
    replace_toml_key_in_section(&mut toml, Some("net_info"), "mnc", &req.mnc.to_string());
    replace_toml_key_in_section(&mut toml, Some("cell_info"), "freq_band", &plan.freq_band.to_string());
    replace_toml_key_in_section(&mut toml, Some("cell_info"), "main_carrier", &plan.main_carrier.to_string());
    replace_toml_key_in_section(&mut toml, Some("cell_info"), "duplex_spacing", &req.duplex_spacing.to_string());
    replace_toml_key_in_section(&mut toml, Some("cell_info"), "freq_offset", &plan.freq_offset.to_string());
    replace_toml_key_in_section(
        &mut toml,
        Some("cell_info"),
        "reverse_operation",
        if plan.reverse_operation { "true" } else { "false" },
    );
    replace_toml_key_in_section(&mut toml, Some("cell_info"), "timezone", &format!("{:?}", req.timezone));
    if req.custom_spacing_enabled {
        let custom_line = format!("custom_duplex_spacing = {}", plan.spacing_hz);
        toml = toml.replace(
            "# custom_duplex_spacing = 7600000   # Don't uncomment unless you have programmed a custom duplex spacing entry in your radios",
            &custom_line,
        );
    }
    if req.brew_enabled {
        replace_toml_key_in_section(&mut toml, Some("brew"), "host", &format!("{:?}", req.brew_host));
        replace_toml_key_in_section(&mut toml, Some("brew"), "port", &req.brew_port.to_string());
        replace_toml_key_in_section(&mut toml, Some("brew"), "tls", if req.brew_tls { "true" } else { "false" });
        replace_toml_key_in_section(&mut toml, Some("brew"), "username", &req.brew_username.to_string());
        replace_toml_key_in_section(&mut toml, Some("brew"), "password", &format!("{:?}", req.brew_password));
    } else {
        toml = remove_toml_section(&toml, "brew");
    }
    tetra_config::bluestation::parsing::from_toml_str(&toml).map_err(|e| format!("Generated config did not validate: {}", e))?;
    Ok((toml, plan))
}

fn serve_easy_start_status(stream: TcpStream, config_path: &str) {
    let required = !easy_start_completed(config_path);
    let body = serde_json::json!({
        "ok": true,
        "required": required,
        "completed": !required,
        "defaults": {
            "mcc": 901,
            "mnc": 9999,
            "timezone": "Europe/Bucharest",
            "tx_freq": 438025000_u64,
            "duplex_spacing": 1,
            "custom_spacing_enabled": false,
            "custom_spacing_hz": 7600000_u64,
            "brew_enabled": true,
            "brew_host": "core.tetrapack.online",
            "brew_port": 443,
            "brew_tls": true,
            "brew_username": 123456789_u32,
            "brew_password": ""
        }
    });
    http_json_response(
        stream,
        200,
        &serde_json::to_string(&body).unwrap_or_else(|_| r#"{"ok":true}"#.to_string()),
    );
}

fn easy_start_preview_json(req: &EasyStartRequest, plan: EasyStartCellPlan, include_toml: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "ok": true,
        "summary": {
            "network": format!("{} / {}", req.mcc, req.mnc),
            "timezone": req.timezone,
            "tx_freq": req.tx_freq,
            "rx_freq": plan.rx_freq,
            "freq_band": plan.freq_band,
            "main_carrier": plan.main_carrier,
            "freq_offset": plan.freq_offset,
            "duplex_spacing": req.duplex_spacing,
            "duplex_shift_hz": plan.spacing_hz,
            "custom_spacing_hz": if req.custom_spacing_enabled { Some(plan.spacing_hz) } else { None },
            "brew": if req.brew_enabled {
                format!("{}:{} ({})", req.brew_host, req.brew_port, if req.brew_tls { "TLS" } else { "plain" })
            } else {
                "disabled".to_string()
            },
        },
        "message": "Config verified. Review the summary, then Commit & Start Nexus-BS."
    });
    if let Some(toml) = include_toml {
        if let Some(obj) = body.as_object_mut() {
            obj.insert("toml".to_string(), serde_json::Value::String(toml.to_string()));
        }
    }
    body
}

fn serve_easy_start_preview(stream: TcpStream, value: &serde_json::Value) {
    let response = match parse_easy_start_request(value)
        .and_then(|req| render_easy_start_config(&req).map(|(toml, plan)| easy_start_preview_json(&req, plan, Some(&toml))))
    {
        Ok(body) => (200, body),
        Err(err) => (400, serde_json::json!({"ok": false, "error": err})),
    };
    http_json_response(
        stream,
        response.0,
        &serde_json::to_string(&response.1).unwrap_or_else(|_| r#"{"ok":false}"#.to_string()),
    );
}

fn serve_easy_start_commit(stream: TcpStream, config_path: &str, cmd_tx: &Arc<Mutex<Option<CmdSender>>>, value: &serde_json::Value) {
    let response = match parse_easy_start_request(value).and_then(|req| {
        let (toml, plan) = render_easy_start_config(&req)?;
        let backup_path = format!("{}.bak", config_path);
        if let Err(e) = std::fs::copy(config_path, &backup_path) {
            tracing::warn!("Dashboard: Easy Start config backup failed: {}", e);
        }
        write_dashboard_config_file(Path::new(config_path), &toml).map_err(|e| format!("failed to write config.toml: {}", e))?;
        mark_easy_start(config_path, EASY_START_DONE_MARKER)?;
        let restart_accepted = send_control_cmd(cmd_tx, ControlCommand::RestartService);
        if !restart_accepted {
            crate::service_control::schedule_service_action(
                crate::service_control::ServiceAction::Restart,
                std::time::Duration::from_secs(2),
            );
        }
        tracing::warn!("Dashboard: Easy Start committed config and queued service restart");
        let mut body = easy_start_preview_json(&req, plan, None);
        if let Some(obj) = body.as_object_mut() {
            obj.insert("restart_queued".to_string(), serde_json::Value::Bool(true));
            obj.insert(
                "message".to_string(),
                serde_json::Value::String(if restart_accepted {
                    "Config saved. Nexus-BS restart queued.".to_string()
                } else {
                    "Config saved. Restart scheduled through service manager.".to_string()
                }),
            );
        }
        Ok(body)
    }) {
        Ok(body) => (200, body),
        Err(err) => (400, serde_json::json!({"ok": false, "error": err})),
    };
    http_json_response(
        stream,
        response.0,
        &serde_json::to_string(&response.1).unwrap_or_else(|_| r#"{"ok":false}"#.to_string()),
    );
}

fn serve_easy_start_skip(stream: TcpStream, config_path: &str) {
    let response = match mark_easy_start(config_path, EASY_START_SKIP_MARKER) {
        Ok(_) => (200, serde_json::json!({"ok": true, "message": "Easy Start skipped"})),
        Err(err) => (500, serde_json::json!({"ok": false, "error": err})),
    };
    http_json_response(
        stream,
        response.0,
        &serde_json::to_string(&response.1).unwrap_or_else(|_| r#"{"ok":false}"#.to_string()),
    );
}

#[derive(Debug, Default)]
struct FactoryResetCleanup {
    wifi_removed: Vec<String>,
    config_removed: Vec<String>,
    ssh_removed: Vec<String>,
}

fn clean_factory_reset_configs(config_path: &str) -> Result<Vec<String>, Vec<String>> {
    let config_dir = config_dir_for(config_path);
    let mut removed = Vec::new();
    let mut errors = Vec::new();
    let entries = match fs::read_dir(&config_dir) {
        Ok(entries) => entries,
        Err(e) => return Err(vec![format!("config dir {} not readable: {}", config_dir.display(), e)]),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                errors.push(format!("config dir entry read failed: {}", e));
                continue;
            }
        };
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let remove = name.ends_with(".toml")
            || name.ends_with(".toml.bak")
            || name.ends_with(".toml.active")
            || name == EASY_START_DONE_MARKER
            || name == EASY_START_SKIP_MARKER;
        if !remove {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(_) => removed.push(name),
            Err(e) => errors.push(format!("failed to remove config {}: {}", path.display(), e)),
        }
    }
    if errors.is_empty() { Ok(removed) } else { Err(errors) }
}

fn clean_factory_reset_wifi() -> Result<Vec<String>, Vec<String>> {
    let mut removed = Vec::new();
    let mut errors = Vec::new();
    let profiles = match crate::wifi::list_saved() {
        Ok(profiles) => profiles,
        Err(e) => return Err(vec![format!("could not list saved Wi-Fi profiles: {}", e)]),
    };
    for profile in profiles {
        match crate::wifi::forget(&profile.uuid) {
            Ok(_) => removed.push(profile.name),
            Err(e) => errors.push(format!("failed to forget Wi-Fi profile {}: {}", profile.name, e)),
        }
    }
    if errors.is_empty() { Ok(removed) } else { Err(errors) }
}

#[cfg(unix)]
fn current_user_home_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.trim().is_empty() && home != "/" {
        return Ok(PathBuf::from(home));
    }
    let id_output = Command::new("id")
        .arg("-un")
        .output()
        .map_err(|e| format!("could not determine service user: {}", e))?;
    if !id_output.status.success() {
        return Err("could not determine service user with id -un".to_string());
    }
    let user = String::from_utf8_lossy(&id_output.stdout).trim().to_string();
    if user.is_empty() || user == "root" {
        return Err("refusing to erase SSH keys without a non-root service user home".to_string());
    }
    let passwd_output = Command::new("getent")
        .args(["passwd", &user])
        .output()
        .map_err(|e| format!("could not query passwd database for {}: {}", user, e))?;
    if !passwd_output.status.success() {
        return Err(format!("could not query passwd database for {}", user));
    }
    let passwd = String::from_utf8_lossy(&passwd_output.stdout);
    let home = passwd
        .split(':')
        .nth(5)
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "/")
        .ok_or_else(|| format!("could not determine home directory for {}", user))?;
    Ok(PathBuf::from(home))
}

#[cfg(unix)]
fn clean_factory_reset_ssh_keys() -> Result<Vec<String>, Vec<String>> {
    let mut removed = Vec::new();
    let home = match current_user_home_dir() {
        Ok(home) => home,
        Err(e) => return Err(vec![e]),
    };
    let ssh_dir = home.join(".ssh");
    if !ssh_dir.exists() {
        return Ok(removed);
    }
    let entries = match fs::read_dir(&ssh_dir) {
        Ok(entries) => entries,
        Err(e) => return Err(vec![format!("SSH dir {} not readable: {}", ssh_dir.display(), e)]),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => return Err(vec![format!("SSH dir entry read failed: {}", e)]),
        };
        let path = entry.path();
        removed.push(path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    }
    fs::remove_dir_all(&ssh_dir).map_err(|e| vec![format!("failed to remove SSH dir {}: {}", ssh_dir.display(), e)])?;
    Ok(removed)
}

#[cfg(not(unix))]
fn clean_factory_reset_ssh_keys() -> Result<Vec<String>, Vec<String>> {
    Ok(Vec::new())
}

fn clean_factory_reset(config_path: &str) -> Result<FactoryResetCleanup, Vec<String>> {
    let mut errors = Vec::new();
    let wifi_removed = match clean_factory_reset_wifi() {
        Ok(removed) => removed,
        Err(mut step_errors) => {
            errors.append(&mut step_errors);
            Vec::new()
        }
    };
    let config_removed = match clean_factory_reset_configs(config_path) {
        Ok(removed) => removed,
        Err(mut step_errors) => {
            errors.append(&mut step_errors);
            Vec::new()
        }
    };
    let ssh_removed = match clean_factory_reset_ssh_keys() {
        Ok(removed) => removed,
        Err(mut step_errors) => {
            errors.append(&mut step_errors);
            Vec::new()
        }
    };
    if errors.is_empty() {
        Ok(FactoryResetCleanup {
            wifi_removed,
            config_removed,
            ssh_removed,
        })
    } else {
        Err(errors)
    }
}

fn serve_factory_reset(stream: TcpStream, config_path: &str, cmd_tx: &Arc<Mutex<Option<CmdSender>>>, value: &serde_json::Value) {
    let confirmation = value.get("confirmation").and_then(serde_json::Value::as_str).unwrap_or("").trim();
    if confirmation != FACTORY_RESET_CONFIRMATION {
        let body = serde_json::json!({
            "ok": false,
            "error": format!("Type {} to confirm factory reset", FACTORY_RESET_CONFIRMATION),
        });
        return http_json_response(
            stream,
            400,
            &serde_json::to_string(&body).unwrap_or_else(|_| r#"{"ok":false}"#.to_string()),
        );
    }

    let cleanup = match clean_factory_reset(config_path) {
        Ok(cleanup) => cleanup,
        Err(errors) => {
            tracing::error!("Dashboard: factory reset refused after cleanup errors: {:?}", errors);
            let body = serde_json::json!({
                "ok": false,
                "error": "Factory reset did not complete cleanly. Host shutdown was not queued.",
                "errors": errors,
            });
            return http_json_response(
                stream,
                500,
                &serde_json::to_string(&body).unwrap_or_else(|_| r#"{"ok":false}"#.to_string()),
            );
        }
    };
    let poweroff_accepted = send_control_cmd(cmd_tx, ControlCommand::PowerOffHost);
    crate::service_control::schedule_service_action(
        crate::service_control::ServiceAction::PowerOffHost,
        std::time::Duration::from_secs(3),
    );
    tracing::warn!(
        "Dashboard: factory reset requested; removed {} wifi profiles, {} config files, {} ssh key files",
        cleanup.wifi_removed.len(),
        cleanup.config_removed.len(),
        cleanup.ssh_removed.len()
    );
    let body = serde_json::json!({
        "ok": true,
        "message": "Factory reset accepted. Wi-Fi, configs, and SSH keys were cleared. Host shutdown queued.",
        "wifi_removed": cleanup.wifi_removed,
        "config_removed": cleanup.config_removed,
        "ssh_removed": cleanup.ssh_removed,
        "poweroff_queued": true,
        "control_poweroff_queued": poweroff_accepted,
        "local_poweroff_fallback_queued": true,
    });
    http_json_response(
        stream,
        200,
        &serde_json::to_string(&body).unwrap_or_else(|_| r#"{"ok":true}"#.to_string()),
    );
}

fn nmcli_available() -> bool {
    Command::new("nmcli")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn nmcli_output(args: &[&str]) -> Result<String, String> {
    let output = Command::new("nmcli")
        .args(args)
        .output()
        .map_err(|e| format!("nmcli unavailable: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("nmcli exited with status {}", output.status)
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn split_nmcli_terse_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for ch in line.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == ':' {
            fields.push(current);
            current = String::new();
        } else {
            current.push(ch);
        }
    }
    if escaped {
        current.push('\\');
    }
    fields.push(current);
    fields
}

fn wifi_devices_json() -> Result<Vec<serde_json::Value>, String> {
    let text = nmcli_output(&["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device", "status"])?;
    let devices = text
        .lines()
        .filter_map(|line| {
            let fields = split_nmcli_terse_line(line);
            if fields.get(1).map(String::as_str) != Some("wifi") {
                return None;
            }
            Some(serde_json::json!({
                "name": fields.first().cloned().unwrap_or_default(),
                "type": fields.get(1).cloned().unwrap_or_default(),
                "state": fields.get(2).cloned().unwrap_or_default(),
                "connection": fields.get(3).cloned().unwrap_or_default(),
            }))
        })
        .collect();
    Ok(devices)
}

fn wifi_rescan_devices(devices: &[serde_json::Value]) {
    let mut requested = false;
    for device in devices
        .iter()
        .filter_map(|device| device.get("name").and_then(serde_json::Value::as_str))
        .filter(|name| !name.trim().is_empty())
    {
        requested = true;
        if let Err(err) = nmcli_output(&["device", "wifi", "rescan", "ifname", device]) {
            tracing::warn!("Dashboard Wi-Fi: rescan on {} failed: {}", device, err);
        }
    }
    if !requested {
        let _ = nmcli_output(&["device", "wifi", "rescan"]);
    }
    // On Raspberry Pi / NetworkManager, `nmcli device wifi rescan` can return
    // before the scan cache is repopulated. User-observed scans on wlan0 take
    // roughly 3.4s, so wait past that before reading the cached list.
    std::thread::sleep(std::time::Duration::from_millis(4200));
}

fn parse_wifi_networks(text: &str) -> Vec<serde_json::Value> {
    let mut best_by_ssid: HashMap<String, serde_json::Value> = HashMap::new();
    for line in text.lines() {
        let fields = split_nmcli_terse_line(line);
        let ssid = fields.get(1).cloned().unwrap_or_default();
        if ssid.trim().is_empty() {
            continue;
        }
        let signal = fields.get(2).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
        let security = fields.get(3).cloned().unwrap_or_default();
        let active = fields.first().map(|s| s == "yes").unwrap_or(false);
        let row = serde_json::json!({
            "active": active,
            "ssid": ssid,
            "signal": signal,
            "security": security,
            "secure": !security.trim().is_empty(),
            "device": fields.get(4).cloned().unwrap_or_default(),
            "channel": fields.get(5).cloned().unwrap_or_default(),
            "rate": fields.get(6).cloned().unwrap_or_default(),
        });
        let replace = best_by_ssid
            .get(row["ssid"].as_str().unwrap_or_default())
            .and_then(|old| old.get("signal").and_then(serde_json::Value::as_u64))
            .map(|old_signal| active || u64::from(signal) > old_signal)
            .unwrap_or(true);
        if replace {
            best_by_ssid.insert(row["ssid"].as_str().unwrap_or_default().to_string(), row);
        }
    }
    let mut networks: Vec<_> = best_by_ssid.into_values().collect();
    networks.sort_by(|a, b| {
        b.get("active")
            .and_then(serde_json::Value::as_bool)
            .cmp(&a.get("active").and_then(serde_json::Value::as_bool))
            .then_with(|| {
                b.get("signal")
                    .and_then(serde_json::Value::as_u64)
                    .cmp(&a.get("signal").and_then(serde_json::Value::as_u64))
            })
            .then_with(|| {
                a.get("ssid")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .cmp(b.get("ssid").and_then(serde_json::Value::as_str).unwrap_or(""))
            })
    });
    networks
}

fn wifi_networks_text(rescan_arg: &str) -> Result<String, String> {
    nmcli_output(&[
        "-t",
        "-f",
        "ACTIVE,SSID,SIGNAL,SECURITY,DEVICE,CHAN,RATE",
        "device",
        "wifi",
        "list",
        "--rescan",
        rescan_arg,
    ])
}

fn wifi_networks_json(rescan: bool, devices: &[serde_json::Value]) -> Result<Vec<serde_json::Value>, String> {
    if rescan {
        wifi_rescan_devices(devices);
    }
    let mut networks = parse_wifi_networks(&wifi_networks_text("no")?);
    if rescan && networks.len() <= 1 {
        if let Ok(text) = wifi_networks_text("yes") {
            let fallback = parse_wifi_networks(&text);
            if fallback.len() > networks.len() {
                networks = fallback;
            }
        }
    }
    Ok(networks)
}

fn wifi_status_json(rescan: bool) -> serde_json::Value {
    if !nmcli_available() {
        return serde_json::json!({
            "ok": false,
            "available": false,
            "status": "unavailable",
            "message": "NetworkManager nmcli is not installed or not executable",
            "devices": [],
            "networks": [],
        });
    }
    let devices = match wifi_devices_json() {
        Ok(devices) => devices,
        Err(err) => {
            return serde_json::json!({
                "ok": false,
                "available": false,
                "status": "unavailable",
                "message": err,
                "devices": [],
                "networks": [],
            });
        }
    };
    let device = devices.first().cloned().unwrap_or_else(|| serde_json::json!({}));
    let networks = wifi_networks_json(rescan, &devices).unwrap_or_default();
    let active_scanned_ssid = networks
        .iter()
        .find(|network| network.get("active").and_then(serde_json::Value::as_bool).unwrap_or(false))
        .and_then(|network| network.get("ssid").and_then(serde_json::Value::as_str))
        .unwrap_or("");
    let connection_name = device
        .get("connection")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty() && *value != "--")
        .unwrap_or("")
        .to_string();
    let current_ssid = if active_scanned_ssid.is_empty() {
        connection_name.clone()
    } else {
        active_scanned_ssid.to_string()
    };
    serde_json::json!({
        "ok": true,
        "available": !devices.is_empty(),
        "status": if devices.is_empty() { "no wifi device" } else { "ready" },
        "message": if devices.is_empty() { "No Wi-Fi device found" } else { "" },
        "device": device,
        "device_name": device.get("name").and_then(serde_json::Value::as_str).unwrap_or(""),
        "devices": devices,
        "connection_name": connection_name,
        "current_ssid": current_ssid,
        "networks": networks,
    })
}

fn serve_wifi_status(stream: TcpStream, rescan: bool) {
    let body = serde_json::to_string(&wifi_status_json(rescan)).unwrap_or_else(|_| r#"{"ok":false}"#.to_string());
    http_json_response(stream, 200, &body);
}

fn parse_wifi_connect_request(value: &serde_json::Value) -> Result<(String, String, bool), String> {
    let ssid = value
        .get("ssid")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if ssid.is_empty() || ssid.len() > 32 {
        return Err("ssid required, max 32 bytes/chars".to_string());
    }
    let password = value.get("password").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
    if password.len() > 128 {
        return Err("password too long".to_string());
    }
    let hidden = value.get("hidden").and_then(serde_json::Value::as_bool).unwrap_or(false);
    Ok((ssid, password, hidden))
}

fn saved_wifi_uuid_for_ssid(ssid: &str) -> Option<String> {
    let suffix = format!("-{}", ssid);
    crate::wifi::list_saved().ok()?.into_iter().find_map(|profile| {
        if profile.name == ssid || profile.name.ends_with(&suffix) || (profile.active && profile.name.contains(ssid)) {
            Some(profile.uuid)
        } else {
            None
        }
    })
}

fn scanned_wifi_security_for_ssid(status: &serde_json::Value, ssid: &str) -> Option<String> {
    status
        .get("networks")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .find(|network| network.get("ssid").and_then(serde_json::Value::as_str) == Some(ssid))
        .and_then(|network| network.get("security").and_then(serde_json::Value::as_str))
        .map(str::to_string)
}

fn update_saved_wifi_password(uuid: &str, ssid: &str, password: &str) -> Result<(), String> {
    nmcli_output(&[
        "connection",
        "modify",
        "uuid",
        uuid,
        "802-11-wireless.ssid",
        ssid,
        "802-11-wireless-security.key-mgmt",
        "wpa-psk",
        "802-11-wireless-security.psk",
        password,
    ])
    .map(|_| ())
}

fn serve_wifi_connect(stream: TcpStream, value: &serde_json::Value) {
    let (ssid, password, hidden) = match parse_wifi_connect_request(value) {
        Ok(req) => req,
        Err(err) => {
            http_json_response(
                stream,
                400,
                &serde_json::to_string(&serde_json::json!({"ok": false, "error": err})).unwrap_or_else(|_| r#"{"ok":false}"#.to_string()),
            );
            return;
        }
    };
    let status = wifi_status_json(false);
    if !status.get("available").and_then(serde_json::Value::as_bool).unwrap_or(false) {
        let body = serde_json::to_string(&serde_json::json!({
            "ok": false,
            "error": status.get("message").and_then(serde_json::Value::as_str).unwrap_or("Wi-Fi unavailable"),
        }))
        .unwrap_or_else(|_| r#"{"ok":false}"#.to_string());
        http_json_response(stream, 503, &body);
        return;
    }

    let result = if password.is_empty() {
        if let Some(uuid) = saved_wifi_uuid_for_ssid(&ssid) {
            crate::wifi::connect_saved(&uuid).map_err(|e| e.to_string())
        } else {
            let security = scanned_wifi_security_for_ssid(&status, &ssid).unwrap_or_default();
            if !security.trim().is_empty() {
                Err(format!("password required for secured Wi-Fi network '{}'", ssid))
            } else {
                crate::wifi::connect_new(&ssid, "", hidden).map_err(|e| e.to_string())
            }
        }
    } else if let Some(uuid) = saved_wifi_uuid_for_ssid(&ssid) {
        update_saved_wifi_password(&uuid, &ssid, &password).and_then(|_| crate::wifi::connect_saved(&uuid).map_err(|e| e.to_string()))
    } else {
        crate::wifi::connect_new(&ssid, &password, hidden).map_err(|e| e.to_string())
    };

    match result {
        Ok(_) => {
            tracing::info!("Dashboard: Wi-Fi connect requested ssid='{}'", ssid);
            let mut body = wifi_status_json(false);
            if let Some(obj) = body.as_object_mut() {
                obj.insert("message".to_string(), serde_json::Value::String(format!("connected {}", ssid)));
            }
            http_json_response(
                stream,
                200,
                &serde_json::to_string(&body).unwrap_or_else(|_| r#"{"ok":true}"#.to_string()),
            );
        }
        Err(err) => {
            tracing::warn!("Dashboard: Wi-Fi connect failed ssid='{}': {}", ssid, err);
            let body = serde_json::to_string(&serde_json::json!({
                "ok": false,
                "error": err,
            }))
            .unwrap_or_else(|_| r#"{"ok":false}"#.to_string());
            http_json_response(stream, 500, &body);
        }
    }
}

fn parse_rf_carrier_inhibited(value: &serde_json::Value) -> Result<bool, String> {
    value
        .get("inhibited")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| "expected JSON object with boolean field 'inhibited'".to_string())
}

fn serve_rf_carrier_post(stream: TcpStream, rf_cmd_tx: &Arc<Mutex<Option<CmdSender>>>, value: &serde_json::Value) {
    let inhibited = match parse_rf_carrier_inhibited(value) {
        Ok(inhibited) => inhibited,
        Err(err) => {
            http_response(stream, 400, &err);
            return;
        }
    };

    let accepted = send_control_cmd(rf_cmd_tx, ControlCommand::SetRfCarrierInhibit { inhibited });
    if !accepted {
        let body = serde_json::to_string(&serde_json::json!({
            "ok": false,
            "inhibited": inhibited,
            "state": "carrier_command_unavailable",
            "error": "RF carrier control channel unavailable",
        }))
        .unwrap_or_else(|_| r#"{"ok":false,"error":"RF carrier control channel unavailable"}"#.to_string());
        http_json_response(stream, 503, &body);
        return;
    }

    tracing::warn!("Dashboard: RF carrier {} requested", if inhibited { "inhibit" } else { "enable" });
    let mut restart_queued = false;
    if !inhibited {
        restart_queued = send_control_cmd(rf_cmd_tx, ControlCommand::RestartService);
        if restart_queued {
            tracing::warn!("Dashboard: core service restart queued after RF carrier enable for SDR reinit");
        } else {
            tracing::warn!("Dashboard: core service restart was not queued after RF carrier enable");
        }
        crate::service_control::schedule_service_action(crate::service_control::ServiceAction::Restart, std::time::Duration::from_secs(4));
    }
    let body = serde_json::to_string(&serde_json::json!({
        "ok": true,
        "inhibited": inhibited,
        "restart_queued": restart_queued,
        "state": if inhibited { "carrier_inhibit_queued" } else if restart_queued { "carrier_enable_restart_queued" } else { "carrier_enable_queued" },
        "message": if inhibited {
            "RF carrier inhibit queued"
        } else if restart_queued {
            "RF carrier enable queued; core service restart follows for SDR reinit"
        } else {
            "RF carrier enable queued; core restart was not accepted"
        },
    }))
    .unwrap_or_else(|_| r#"{"ok":true}"#.to_string());
    http_json_response(stream, 200, &body);
}

fn serve_rf_calibration_status(stream: TcpStream, config_path: &str) {
    let path = crate::rf_calibration::calibration_path_for_config(config_path);
    let body = serde_json::to_string(&crate::rf_calibration::status_json(&path)).unwrap_or_else(|_| r#"{"ok":false}"#.to_string());
    http_json_response(stream, 200, &body);
}

fn serve_rf_calibration_run(
    stream: TcpStream,
    state: &DashboardState,
    config_path: &str,
    rf_cmd_tx: &Arc<Mutex<Option<CmdSender>>>,
    phy_cmd_tx: &Arc<Mutex<Option<CmdSender>>>,
) {
    let active_calls = match state.read() {
        Ok(snapshot) => snapshot.snapshot_calls(),
        Err(_) => {
            http_json_response(stream, 503, r#"{"ok":false,"error":"dashboard state unavailable"}"#);
            return;
        }
    };
    if !active_calls.is_empty() {
        let body = serde_json::to_string(&serde_json::json!({
            "ok": false,
            "error": "RF calibration refused while calls are active",
            "calls": active_calls,
        }))
        .unwrap_or_else(|_| r#"{"ok":false,"error":"RF calibration refused while calls are active"}"#.to_string());
        http_json_response(stream, 423, &body);
        return;
    }

    let calibration_path = crate::rf_calibration::calibration_path_for_config(config_path);
    let calibration_path_string = calibration_path.display().to_string();
    if let Err(err) = crate::rf_calibration::try_start(&calibration_path_string) {
        let body = serde_json::to_string(&serde_json::json!({
            "ok": false,
            "error": err,
            "status": crate::rf_calibration::current_phase().as_str(),
        }))
        .unwrap_or_else(|_| r#"{"ok":false}"#.to_string());
        http_json_response(stream, 409, &body);
        return;
    }

    let rf_accepted = send_control_cmd(rf_cmd_tx, ControlCommand::SetRfCarrierInhibit { inhibited: true });
    if !rf_accepted {
        crate::rf_calibration::mark_failed("RF carrier control channel unavailable");
        http_json_response(stream, 503, r#"{"ok":false,"error":"RF carrier control channel unavailable"}"#);
        return;
    }

    let phy_sender = phy_cmd_tx.lock().ok().and_then(|guard| guard.clone());
    let Some(phy_sender) = phy_sender else {
        crate::rf_calibration::mark_failed("PHY control channel unavailable");
        let _ = send_control_cmd(rf_cmd_tx, ControlCommand::SetRfCarrierInhibit { inhibited: false });
        http_json_response(stream, 503, r#"{"ok":false,"error":"PHY control channel unavailable"}"#);
        return;
    };
    let rf_sender = rf_cmd_tx.lock().ok().and_then(|guard| guard.clone());

    let config_path_string = config_path.to_string();
    if let Err(err) = std::thread::Builder::new().name("rf-calibration".into()).spawn(move || {
        run_rf_calibration_orchestrator(config_path_string, calibration_path_string, rf_sender, phy_sender);
    }) {
        crate::rf_calibration::mark_failed(format!("failed to start RF calibration orchestrator: {}", err));
        let _ = send_control_cmd(rf_cmd_tx, ControlCommand::SetRfCarrierInhibit { inhibited: false });
        http_json_response(stream, 503, r#"{"ok":false,"error":"failed to start RF calibration orchestrator"}"#);
        return;
    }

    let body = serde_json::to_string(&serde_json::json!({
        "ok": true,
        "status": "inhibiting",
        "path": calibration_path.display().to_string(),
        "message": "RF calibration started",
    }))
    .unwrap_or_else(|_| r#"{"ok":true}"#.to_string());
    http_json_response(stream, 202, &body);
}

fn run_rf_calibration_orchestrator(config_path: String, calibration_path: String, rf_sender: Option<CmdSender>, phy_sender: CmdSender) {
    crate::rf_calibration::append_log("RF carrier inhibit queued; waiting notification guard before PHY calibration");
    std::thread::sleep(std::time::Duration::from_secs(3));
    if phy_sender
        .try_send(ControlCommand::RunTxCalibration {
            calibration_path: calibration_path.clone(),
        })
        .is_err()
    {
        crate::rf_calibration::mark_failed("failed to queue PHY calibration command");
        restore_rf_carrier_after_calibration(&rf_sender, "failed PHY queue");
        return;
    }

    crate::rf_calibration::append_log("PHY calibration command queued; waiting for run report from core");
    let run_report_path = tetra_config::bluestation::tx_calibration_run_report_path(&calibration_path);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    loop {
        if run_report_path.exists() {
            crate::rf_calibration::mark_calibrated("PHY calibration report available");
            break;
        }
        if matches!(
            crate::rf_calibration::current_phase(),
            crate::rf_calibration::CalibrationPhase::Failed
        ) {
            request_core_restart_after_rf_calibration(&rf_sender, "failed calibration");
            return;
        }
        if std::time::Instant::now() >= deadline {
            crate::rf_calibration::mark_failed("timeout waiting for PHY calibration report");
            request_core_restart_after_rf_calibration(&rf_sender, "calibration timeout");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    let report_path = if run_report_path.exists() {
        &run_report_path
    } else {
        Path::new(&calibration_path)
    };
    let report = match tetra_config::bluestation::read_tx_calibration_file(report_path) {
        Ok(report) => report,
        Err(err) => {
            crate::rf_calibration::mark_failed(format!("calibration run report was not readable after PHY finished: {}", err));
            request_core_restart_after_rf_calibration(&rf_sender, "unreadable calibration file");
            return;
        }
    };
    if !report.report.accepted {
        crate::rf_calibration::mark_failed(format!("calibration rejected; config not changed: {}", report.report.summary));
        request_core_restart_after_rf_calibration(&rf_sender, "rejected calibration");
        return;
    }

    match enable_tx_calibration_in_config(&config_path) {
        Ok(()) => {
            crate::rf_calibration::mark_restarting("calibration accepted; config.toml updated; restarting service");
            request_core_restart_after_rf_calibration(&rf_sender, "accepted calibration");
        }
        Err(err) => {
            crate::rf_calibration::mark_failed(format!("calibration accepted but config update failed: {}", err));
            request_core_restart_after_rf_calibration(&rf_sender, "config update failure");
        }
    }
}

fn request_core_restart_after_rf_calibration(sender: &Option<CmdSender>, reason: &str) {
    let restart_message = format!(
        "core service restart requested after {}; full SDR reinit required after destructive calibration",
        reason
    );
    if matches!(
        crate::rf_calibration::current_phase(),
        crate::rf_calibration::CalibrationPhase::Failed
    ) {
        crate::rf_calibration::append_log(restart_message);
    } else {
        crate::rf_calibration::mark_restarting(restart_message);
    }
    let Some(sender) = sender else {
        crate::rf_calibration::append_log("ERROR: core restart unavailable after calibration; restart nexus-bs.service manually");
        return;
    };
    match sender.try_send(ControlCommand::RestartService) {
        Ok(()) => crate::rf_calibration::append_log("core restart command queued"),
        Err(_) => crate::rf_calibration::append_log("ERROR: core restart command queue failed after calibration"),
    }
    crate::rf_calibration::append_log("core restart fallback scheduled through systemd");
    crate::service_control::schedule_service_action(crate::service_control::ServiceAction::Restart, std::time::Duration::from_secs(4));
}

fn restore_rf_carrier_after_calibration(rf_sender: &Option<CmdSender>, reason: &str) {
    let Some(sender) = rf_sender else {
        crate::rf_calibration::append_log(format!("ERROR: RF carrier enable unavailable after {}", reason));
        return;
    };
    match sender.try_send(ControlCommand::SetRfCarrierInhibit { inhibited: false }) {
        Ok(()) => crate::rf_calibration::append_log(format!("RF carrier enable queued after {}", reason)),
        Err(_) => crate::rf_calibration::append_log(format!("ERROR: RF carrier enable queue failed after {}", reason)),
    }
}

fn enable_tx_calibration_in_config(config_path: &str) -> Result<(), String> {
    let path = Path::new(config_path);
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    let updated = upsert_soapy_calibration_keys(&text)?;
    let backup_path = format!("{}.bak", config_path);
    let _ = std::fs::copy(path, &backup_path);
    let tmp_path = path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, updated).map_err(|e| format!("write {}: {}", tmp_path.display(), e))?;
    std::fs::rename(&tmp_path, path).map_err(|e| format!("rename {} -> {}: {}", tmp_path.display(), path.display(), e))?;
    Ok(())
}

fn upsert_soapy_calibration_keys(input: &str) -> Result<String, String> {
    let mut lines: Vec<String> = input.lines().map(|line| line.to_string()).collect();
    let had_trailing_newline = input.ends_with('\n');
    let section_start = lines
        .iter()
        .position(|line| line.trim() == "[phy_io.soapysdr]")
        .ok_or_else(|| "missing [phy_io.soapysdr] section".to_string())?;
    let section_end = lines
        .iter()
        .enumerate()
        .skip(section_start + 1)
        .find(|(_, line)| {
            let trimmed = line.trim();
            trimmed.starts_with('[') && trimmed.ends_with(']')
        })
        .map(|(idx, _)| idx)
        .unwrap_or(lines.len());

    let desired = [
        ("tx_calibration_enabled", "true"),
        ("tx_calibration_file", "\"calibration.toml\""),
        ("tx_calibration_apply_dc", "true"),
        ("tx_calibration_apply_iq", "false"),
    ];
    let mut present = [false; 4];
    for idx in (section_start + 1)..section_end {
        let matched = {
            let trimmed = lines[idx].trim_start();
            desired.iter().enumerate().find_map(|(key_idx, (key, value))| {
                (trimmed.starts_with(key) && trimmed[key.len()..].trim_start().starts_with('=')).then_some((key_idx, *key, *value))
            })
        };
        if let Some((key_idx, key, value)) = matched {
            lines[idx] = format!("{key} = {value}");
            present[key_idx] = true;
        }
    }
    let mut insert_at = section_end;
    for (key_idx, (key, value)) in desired.iter().enumerate() {
        if !present[key_idx] {
            lines.insert(insert_at, format!("{key} = {value}"));
            insert_at += 1;
        }
    }
    let mut out = lines.join("\n");
    if had_trailing_newline {
        out.push('\n');
    }
    Ok(out)
}

fn serve_clear_dashboard_logs(stream: TcpStream, state: &DashboardState) {
    if let Ok(mut s) = state.write() {
        s.clear_logs();
    }
    http_json_response(stream, 200, r#"{"ok":true}"#);
}

fn live_sds_queue_full(cfg: &Option<tetra_config::bluestation::SharedConfig>) -> bool {
    cfg.as_ref()
        .is_some_and(|c| c.state_read().live_sds_queue.len() >= LIVE_SDS_QUEUE_MAX_LEN)
}

/// Serialize the current live SDS queue to JSON and serve it.
fn serve_live_sds_list(stream: TcpStream, cfg: &Option<tetra_config::bluestation::SharedConfig>) {
    let items: Vec<serde_json::Value> = cfg
        .as_ref()
        .map(|c| {
            let state = c.state_read();
            state
                .live_sds_queue
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m.id,
                        "text": m.text,
                        "protocol_id": m.protocol_id,
                        "source_issi": m.source_issi,
                        "repeat_count": m.repeat_count,
                        "sent_count": m.sent_count,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let body = serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string());
    http_json_response(stream, 200, &body);
}

fn handle_connection(
    mut stream: TcpStream,
    state: DashboardState,
    clients: WsClients,
    config_path: String,
    cmd_tx: Arc<Mutex<Option<CmdSender>>>,
    rf_cmd_tx: Arc<Mutex<Option<CmdSender>>>,
    phy_cmd_tx: Arc<Mutex<Option<CmdSender>>>,
    update_state: SharedUpdateState,
    source_dir_override: Option<String>,
    static_dir: Option<String>,
    auth: Option<(String, String)>,
    shared_config: Option<tetra_config::bluestation::SharedConfig>,
    sessions: SharedSessionStore,
    bs_started_at: Instant,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));

    // ── Read the first 4KB of headers into a buffer, peek first for routing ──
    // We need to both route on the request line AND read the Authorization header,
    // so we collect all headers before dispatching.
    let mut header_buf = Vec::with_capacity(2048);
    {
        // peek for the request line (already works for routing)
        let mut peek_buf = [0u8; DASHBOARD_HTTP_HEADER_MAX];
        let n = match stream.peek(&mut peek_buf) {
            Ok(n) => n,
            Err(_) => return,
        };
        if n == 0 {
            return;
        }
        if !peek_buf[..n].windows(4).any(|window| window == b"\r\n\r\n") {
            http_response(
                stream,
                431,
                &format!("request headers too large or incomplete (max {DASHBOARD_HTTP_HEADER_MAX} bytes)"),
            );
            return;
        }
        header_buf.extend_from_slice(&peek_buf[..n]);
    }
    let header_str = String::from_utf8_lossy(&header_buf);
    let req_line = header_str.lines().next().unwrap_or("").to_string();

    if request_matches(&req_line, "GET", "/api/auth/status") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_dashboard_auth_status(buf.into_inner(), &auth, &sessions, &header_str);
        return;
    }

    // ── Cookie-session auth ──────────────────────────────────────────────────
    // We replaced the browser-native Basic Auth dialog with a form-based login at
    // /login that issues an fs_session cookie. The native dialog has well-known
    // mobile usability issues (iOS Safari prompts 2-3 times, forgets credentials
    // between WebSocket reconnects, etc.). With cookies we control the UX fully.
    //
    // Public routes (no auth required): GET /login and POST /api/login.
    // Dashboard assets are served after the same cookie-session gate as `/`.
    // Every other route is checked here against the session store.
    if let Some((ref expected_user, ref expected_pass)) = auth {
        // Login page and login API must remain reachable without a session.
        let is_login_page = request_matches(&req_line, "GET", "/login");
        let is_login_api = request_matches(&req_line, "POST", "/api/login");

        // Validate session cookie when present. Note: validate() refreshes last-seen,
        // so active users effectively never time out.
        let session_ok = parse_session_cookie(&header_str)
            .and_then(|token| {
                let mut store = sessions.lock().ok()?;
                Some(store.validate(&token))
            })
            .unwrap_or(false);

        if is_login_page {
            let mut buf = BufReader::new(stream);
            loop {
                let mut line = String::new();
                let _ = buf.read_line(&mut line);
                if line == "\r\n" || line.is_empty() || line == "\n" {
                    break;
                }
            }
            // If already logged in, send them straight to the dashboard.
            if session_ok {
                http_redirect(buf.into_inner(), "/");
            } else {
                serve_login_page(buf.into_inner());
            }
            return;
        }

        if is_login_api {
            // Body has form-encoded or JSON-encoded credentials.
            let mut buf = BufReader::new(stream);
            let body = match read_http_body_from_buf(&mut buf, DASHBOARD_SMALL_BODY_MAX) {
                Ok(body) => body,
                Err(e) => {
                    respond_http_body_error(buf.into_inner(), e);
                    return;
                }
            };
            let body_str = String::from_utf8_lossy(&body);

            let (user, pass) = parse_login_body(&body_str);
            let ok = timing_safe_eq(user.as_bytes(), expected_user.as_bytes()) && timing_safe_eq(pass.as_bytes(), expected_pass.as_bytes());

            if ok {
                let token = if let Ok(mut store) = sessions.lock() {
                    store.create()
                } else {
                    String::new()
                };
                tracing::info!("Dashboard: login OK (user: {})", user);
                serve_login_success(buf.into_inner(), &token);
            } else {
                tracing::warn!("Dashboard: login FAILED (user attempt: {})", user);
                // Small artificial delay to limit brute-force throughput.
                std::thread::sleep(std::time::Duration::from_millis(500));
                http_response(buf.into_inner(), 401, "Invalid credentials");
            }
            return;
        }

        // Logout: invalidate the cookie, then redirect to /login.
        if request_matches(&req_line, "POST", "/api/logout") || request_matches(&req_line, "GET", "/logout") {
            if let Some(token) = parse_session_cookie(&header_str) {
                if let Ok(mut store) = sessions.lock() {
                    store.invalidate(&token);
                }
            }
            let mut buf = BufReader::new(stream);
            loop {
                let mut line = String::new();
                let _ = buf.read_line(&mut line);
                if line == "\r\n" || line.is_empty() || line == "\n" {
                    break;
                }
            }
            serve_logout(buf.into_inner());
            return;
        }

        // All other routes require a valid session.
        if !session_ok {
            let mut buf = BufReader::new(stream);
            loop {
                let mut line = String::new();
                let _ = buf.read_line(&mut line);
                if line == "\r\n" || line.is_empty() || line == "\n" {
                    break;
                }
            }
            // For GET / (the dashboard SPA): redirect to /login so the browser navigates.
            // For API requests: 401 so JS code can detect and refresh.
            if request_matches(&req_line, "GET", "/") {
                http_redirect(buf.into_inner(), "/login");
            } else {
                http_response(buf.into_inner(), 401, "Unauthorized — please log in");
            }
            return;
        }
    }

    if request_matches(&req_line, "GET", "/ws") {
        handle_ws(stream, state, clients, cmd_tx, update_state, auth);
    } else if request_matches(&req_line, "GET", "/api/system") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_system_info(buf.into_inner(), &config_path, bs_started_at);
    } else if request_matches(&req_line, "GET", "/api/snapshot") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_dashboard_snapshot(buf.into_inner(), &state);
    } else if request_matches(&req_line, "GET", "/api/calls") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_dashboard_calls(buf.into_inner(), &state);
    } else if request_matches(&req_line, "GET", "/api/site") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_dashboard_site(buf.into_inner(), &state, &shared_config, &config_path);
    } else if request_matches(&req_line, "GET", "/api/radioid") {
        let issi = request_query_param(&req_line, "id").and_then(|id| id.parse::<u32>().ok());
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        crate::net_dashboard::radioid::serve(buf.into_inner(), issi);
    } else if request_matches(&req_line, "POST", "/api/service/restart") {
        drain_http_headers(&mut stream);
        serve_service_command(stream, &cmd_tx, "restart", ControlCommand::RestartService);
    } else if request_matches(&req_line, "POST", "/api/service/shutdown") {
        drain_http_headers(&mut stream);
        serve_service_command(stream, &cmd_tx, "host shutdown", ControlCommand::PowerOffHost);
    } else if request_matches(&req_line, "POST", "/api/service/stop-go") {
        drain_http_headers(&mut stream);
        serve_service_command(stream, &cmd_tx, "stop-go", ControlCommand::StopGoService { start_delay_secs: 5 });
    } else if request_matches(&req_line, "GET", "/api/easy-start/status") {
        drain_http_headers(&mut stream);
        serve_easy_start_status(stream, &config_path);
    } else if request_matches(&req_line, "POST", "/api/easy-start/preview") {
        let mut buf = BufReader::new(stream);
        let body = match read_http_body_from_buf(&mut buf, DASHBOARD_SMALL_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                respond_http_body_error(buf.into_inner(), e);
                return;
            }
        };
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(value) => serve_easy_start_preview(buf.into_inner(), &value),
            Err(e) => http_json_response(
                buf.into_inner(),
                400,
                &serde_json::to_string(&serde_json::json!({"ok": false, "error": format!("invalid JSON: {}", e)}))
                    .unwrap_or_else(|_| r#"{"ok":false}"#.to_string()),
            ),
        }
    } else if request_matches(&req_line, "POST", "/api/easy-start/commit") {
        let mut buf = BufReader::new(stream);
        let body = match read_http_body_from_buf(&mut buf, DASHBOARD_SMALL_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                respond_http_body_error(buf.into_inner(), e);
                return;
            }
        };
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(value) => serve_easy_start_commit(buf.into_inner(), &config_path, &cmd_tx, &value),
            Err(e) => http_json_response(
                buf.into_inner(),
                400,
                &serde_json::to_string(&serde_json::json!({"ok": false, "error": format!("invalid JSON: {}", e)}))
                    .unwrap_or_else(|_| r#"{"ok":false}"#.to_string()),
            ),
        }
    } else if request_matches(&req_line, "POST", "/api/easy-start/skip") {
        drain_http_headers(&mut stream);
        serve_easy_start_skip(stream, &config_path);
    } else if request_matches(&req_line, "POST", "/api/factory-reset") {
        let mut buf = BufReader::new(stream);
        let body = match read_http_body_from_buf(&mut buf, DASHBOARD_SMALL_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                respond_http_body_error(buf.into_inner(), e);
                return;
            }
        };
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(value) => serve_factory_reset(buf.into_inner(), &config_path, &cmd_tx, &value),
            Err(e) => http_json_response(
                buf.into_inner(),
                400,
                &serde_json::to_string(&serde_json::json!({"ok": false, "error": format!("invalid JSON: {}", e)}))
                    .unwrap_or_else(|_| r#"{"ok":false}"#.to_string()),
            ),
        }
    } else if request_matches(&req_line, "POST", "/api/rf/carrier") {
        let mut buf = BufReader::new(stream);
        let body = match read_http_body_from_buf(&mut buf, DASHBOARD_SMALL_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                respond_http_body_error(buf.into_inner(), e);
                return;
            }
        };
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(value) => serve_rf_carrier_post(buf.into_inner(), &rf_cmd_tx, &value),
            Err(e) => http_response(buf.into_inner(), 400, &format!("invalid JSON: {}", e)),
        }
    } else if request_matches(&req_line, "POST", "/api/rf/calibration/run") {
        drain_http_headers(&mut stream);
        serve_rf_calibration_run(stream, &state, &config_path, &rf_cmd_tx, &phy_cmd_tx);
    } else if request_matches(&req_line, "GET", "/api/rf/calibration/status") {
        drain_http_headers(&mut stream);
        serve_rf_calibration_status(stream, &config_path);
    } else if request_matches(&req_line, "GET", "/api/wifi") {
        drain_http_headers(&mut stream);
        serve_wifi_status(stream, false);
    } else if request_matches(&req_line, "POST", "/api/wifi/scan") {
        drain_http_headers(&mut stream);
        serve_wifi_status(stream, true);
    } else if request_matches(&req_line, "POST", "/api/wifi/connect") {
        let mut buf = BufReader::new(stream);
        let body = match read_http_body_from_buf(&mut buf, DASHBOARD_SMALL_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                respond_http_body_error(buf.into_inner(), e);
                return;
            }
        };
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(value) => serve_wifi_connect(buf.into_inner(), &value),
            Err(e) => http_json_response(
                buf.into_inner(),
                400,
                &serde_json::to_string(&serde_json::json!({"ok": false, "error": format!("invalid JSON: {}", e)}))
                    .unwrap_or_else(|_| r#"{"ok":false}"#.to_string()),
            ),
        }
    } else if request_matches(&req_line, "POST", "/api/logs/clear") {
        drain_http_headers(&mut stream);
        serve_clear_dashboard_logs(stream, &state);
    } else if request_matches(&req_line, "POST", "/api/configs/activate") {
        let mut buf = BufReader::new(stream);
        let body = match read_http_body_from_buf(&mut buf, DASHBOARD_SMALL_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                respond_http_body_error(buf.into_inner(), e);
                return;
            }
        };
        let profile = String::from_utf8_lossy(&body).trim().to_string();
        match activate_config_profile(&config_path, &profile) {
            Ok(_) => {
                tracing::info!("Dashboard: activated config profile '{}'", profile);
                http_response(buf.into_inner(), 200, "OK")
            }
            Err(e) => http_response(buf.into_inner(), 500, &e),
        }
    } else if request_matches(&req_line, "GET", "/api/configs") || request_path_has_prefix(&req_line, "GET", "/api/configs/") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        // GET /api/configs/<name> — read a specific profile's content
        // GET /api/configs       — list all profiles
        let profile_name = request_profile_name(&req_line, "/api/configs/");
        if let Some(name) = profile_name {
            serve_config_profile_get(buf.into_inner(), &config_path, &name);
        } else {
            serve_config_list(buf.into_inner(), &config_path);
        }
    } else if request_path_has_prefix(&req_line, "POST", "/api/configs/") {
        // POST /api/configs/<name> — save content to a specific profile (not activate)
        let profile_name = request_profile_name(&req_line, "/api/configs/");
        let mut buf = BufReader::new(stream);
        let body = match read_http_body_from_buf(&mut buf, DASHBOARD_HTTP_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                respond_http_body_error(buf.into_inner(), e);
                return;
            }
        };
        match profile_name {
            None => http_response(buf.into_inner(), 400, "missing profile name"),
            Some(name) => match save_config_profile(&config_path, &name, &String::from_utf8_lossy(&body)) {
                Ok(_) => {
                    tracing::info!("Dashboard: saved profile '{}'", name);
                    http_response(buf.into_inner(), 200, "OK")
                }
                Err(e) => http_response(buf.into_inner(), 500, &e),
            },
        }
    } else if request_path_has_prefix(&req_line, "DELETE", "/api/configs/") {
        let profile_name = request_profile_name(&req_line, "/api/configs/");
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        match profile_name {
            None => http_response(buf.into_inner(), 400, "missing profile name"),
            Some(name) => match delete_config_profile(&config_path, &name) {
                Ok(_) => {
                    tracing::info!("Dashboard: deleted profile '{}'", name);
                    http_response(buf.into_inner(), 200, "OK")
                }
                Err(e) => http_response(buf.into_inner(), 500, &e),
            },
        }
    } else if request_matches(&req_line, "GET", "/api/update/check") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_update_check(buf.into_inner());
    } else if request_matches(&req_line, "GET", "/api/update/status") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_update_status(buf.into_inner(), &update_state);
    } else if request_matches(&req_line, "POST", "/api/update") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        if !DASHBOARD_OTA_UPDATE_ENABLED {
            http_response(buf.into_inner(), 403, "OTA update disabled");
            return;
        }
        {
            let mut u = update_state.lock().unwrap();
            if u.phase == UpdatePhase::Running {
                http_response(buf.into_inner(), 409, "Update already in progress");
                return;
            }
            u.start();
        }
        tracing::info!("Dashboard: OTA update triggered");
        let update_clone = Arc::clone(&update_state);
        let cfg_clone = config_path.clone();
        let src_override = source_dir_override.clone();
        std::thread::Builder::new()
            .name("ota-update".into())
            .spawn(move || run_update(update_clone, cfg_clone, src_override))
            .ok();
        http_response(buf.into_inner(), 200, "OK");
    } else if request_matches(&req_line, "POST", "/api/update/deb") {
        let mut buf = BufReader::new(stream);
        let body = match read_http_body_from_buf(&mut buf, DASHBOARD_SMALL_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                http_response(buf.into_inner(), 400, &format!("Invalid update request: {e:?}"));
                return;
            }
        };
        let body = match std::str::from_utf8(&body) {
            Ok(body) => body,
            Err(_) => {
                http_response(buf.into_inner(), 400, "Invalid update request: body is not UTF-8");
                return;
            }
        };
        let deb = match parse_deb_update_request(body) {
            Ok(deb) => deb,
            Err(e) => {
                http_response(buf.into_inner(), 400, &e);
                return;
            }
        };
        {
            let mut u = update_state.lock().unwrap();
            if u.phase == UpdatePhase::Running {
                http_response(buf.into_inner(), 409, "Update already in progress");
                return;
            }
            u.start();
        }
        tracing::info!("Dashboard: Debian package update triggered for {}", deb.version);
        let update_clone = Arc::clone(&update_state);
        std::thread::Builder::new()
            .name("deb-update".into())
            .spawn(move || run_deb_update(update_clone, deb))
            .ok();
        http_json_response(buf.into_inner(), 200, r#"{"ok":true,"message":"package update started"}"#);
    } else if request_matches(&req_line, "GET", "/api/config/backup") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        let backup_path = format!("{}.bak", config_path);
        serve_config_get(buf.into_inner(), &backup_path);
    } else if request_matches(&req_line, "POST", "/api/config/restore") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        let backup_path = format!("{}.bak", config_path);
        match std::fs::copy(&backup_path, &config_path) {
            Ok(_) => {
                tracing::info!("Dashboard: config restored from backup");
                http_response(buf.into_inner(), 200, "OK")
            }
            Err(e) => http_response(buf.into_inner(), 500, &e.to_string()),
        }
    } else if request_matches(&req_line, "POST", "/api/config") {
        let mut buf = BufReader::new(stream);
        let body = match read_http_body_from_buf(&mut buf, DASHBOARD_HTTP_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                respond_http_body_error(buf.into_inner(), e);
                return;
            }
        };
        let body_str = String::from_utf8_lossy(&body);
        // Write backup of current config before overwriting
        let backup_path = format!("{}.bak", config_path);
        if let Err(e) = std::fs::copy(&config_path, &backup_path) {
            tracing::warn!("Dashboard: failed to write config backup: {}", e);
        }
        match write_dashboard_config_file(std::path::Path::new(&config_path), body_str.as_ref()) {
            Ok(_) => http_response(buf.into_inner(), 200, "OK"),
            Err(e) => http_response(buf.into_inner(), 500, &e.to_string()),
        }
    } else if request_matches(&req_line, "GET", "/api/whitelist") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_whitelist_get(buf.into_inner(), &shared_config);
    } else if request_matches(&req_line, "POST", "/api/whitelist") {
        let mut buf = BufReader::new(stream);
        let body = match read_http_body_from_buf(&mut buf, DASHBOARD_HTTP_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                respond_http_body_error(buf.into_inner(), e);
                return;
            }
        };
        let body_str = String::from_utf8_lossy(&body);
        serve_whitelist_post(buf.into_inner(), &shared_config, &config_path, body_str.as_ref(), &cmd_tx);
    } else if request_matches(&req_line, "GET", "/api/wx") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_wx_get(buf.into_inner(), &shared_config);
    } else if request_matches(&req_line, "POST", "/api/wx") {
        let mut buf = BufReader::new(stream);
        let body = match read_http_body_from_buf(&mut buf, DASHBOARD_HTTP_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                respond_http_body_error(buf.into_inner(), e);
                return;
            }
        };
        let body_str = String::from_utf8_lossy(&body);
        serve_wx_post(buf.into_inner(), &shared_config, &config_path, body_str.as_ref());
    } else if request_matches(&req_line, "GET", "/api/config") {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_config_get(buf.into_inner(), &config_path);
    } else if request_matches(&req_line, "GET", "/api/live-sds") {
        // Return current live SDS queue as JSON.
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_live_sds_list(buf.into_inner(), &shared_config);
    } else if request_path_has_prefix(&req_line, "DELETE", "/api/live-sds/") {
        // DELETE /api/live-sds/<id>
        let id: u32 = req_line
            .split('/')
            .nth(3)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        if id == 0 {
            http_response(buf.into_inner(), 400, "invalid id");
        } else {
            send_control_cmd(&cmd_tx, ControlCommand::DeleteLiveSds { id });
            http_response(buf.into_inner(), 200, "OK");
        }
    } else if request_matches(&req_line, "DELETE", "/api/live-sds") {
        // DELETE /api/live-sds  — clear all
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        send_control_cmd(&cmd_tx, ControlCommand::ClearLiveSds);
        http_response(buf.into_inner(), 200, "OK");
    } else if request_matches(&req_line, "POST", "/api/live-sds") {
        // POST /api/live-sds  body: JSON { "text": "...", "protocol_id": 130, "source_issi": 16777215, "repeat_count": 0 }
        let mut buf = BufReader::new(stream);
        let body = match read_http_body_from_buf(&mut buf, DASHBOARD_SMALL_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                respond_http_body_error(buf.into_inner(), e);
                return;
            }
        };
        match serde_json::from_slice::<serde_json::Value>(&body) {
            Ok(v) => match parse_live_sds_post(&v) {
                Ok(post) => {
                    if live_sds_queue_full(&shared_config) {
                        http_response(
                            buf.into_inner(),
                            429,
                            &format!("live SDS queue full (max {LIVE_SDS_QUEUE_MAX_LEN})"),
                        );
                        return;
                    }
                    tracing::info!("Dashboard: AddLiveSds text={:?} repeat={}", post.text, post.repeat_count);
                    let accepted = send_control_cmd(
                        &cmd_tx,
                        ControlCommand::AddLiveSds {
                            text: post.text,
                            protocol_id: post.protocol_id,
                            source_issi: post.source_issi,
                            repeat_count: post.repeat_count,
                        },
                    );
                    if accepted {
                        http_response(buf.into_inner(), 200, "OK");
                    } else {
                        http_response(buf.into_inner(), 503, "control channel unavailable");
                    }
                }
                Err(e) => http_response(buf.into_inner(), 400, &e),
            },
            Err(e) => http_response(buf.into_inner(), 400, &format!("invalid JSON: {}", e)),
        }
    // ── WiFi management endpoints ──────────────────────────────────────
    // All paths under /api/wifi/* are GET (read) or POST (mutate). We keep
    // the handlers small and delegate to the `wifi` module — see that for
    // docs on what each operation does. Responses are JSON.
    } else if request_matches(&req_line, "GET", "/api/wifi/status") {
        drain_http_headers(&mut stream);
        let body = match crate::wifi::status() {
            Ok(s) => serde_json::to_string(&serde_json::json!({"ok": true, "status": s})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if request_matches(&req_line, "GET", "/api/wifi/scan") {
        drain_http_headers(&mut stream);
        let body = match crate::wifi::scan() {
            Ok(networks) => serde_json::to_string(&serde_json::json!({"ok": true, "networks": networks})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if request_matches(&req_line, "GET", "/api/wifi/saved") {
        drain_http_headers(&mut stream);
        let body = match crate::wifi::list_saved() {
            Ok(profiles) => serde_json::to_string(&serde_json::json!({"ok": true, "profiles": profiles})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if request_matches(&req_line, "POST", "/api/wifi/connect") {
        // Body shape: {"ssid": "...", "psk": "...", "hidden": false} for a new
        // network, or {"uuid": "..."} to bring up a saved profile.
        let body = match read_http_body(&mut stream, DASHBOARD_SMALL_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                respond_http_body_error(stream, e);
                return;
            }
        };
        let req: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                http_response(stream, 400, &format!("invalid JSON: {}", e));
                return;
            }
        };
        let result = if let Some(uuid) = req.get("uuid").and_then(|v| v.as_str()) {
            tracing::info!("Dashboard: connecting saved WiFi profile uuid={}", uuid);
            crate::wifi::connect_saved(uuid)
        } else if let Some(ssid) = req.get("ssid").and_then(|v| v.as_str()) {
            let psk = req.get("psk").and_then(|v| v.as_str()).unwrap_or("");
            let hidden = req.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false);
            tracing::info!("Dashboard: connecting new WiFi ssid={} hidden={}", ssid, hidden);
            crate::wifi::connect_new(ssid, psk, hidden)
        } else {
            http_response(stream, 400, "missing uuid or ssid");
            return;
        };
        let body = match result {
            Ok(_) => serde_json::to_string(&serde_json::json!({"ok": true})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if request_matches(&req_line, "POST", "/api/wifi/disconnect") {
        drain_http_headers(&mut stream);
        // Find the wireless device name and disconnect it. The body is empty.
        let iface = match crate::wifi::status() {
            Ok(s) if s.device_present => "wlan0".to_string(), // nmcli accepts any wifi dev name; wlan0 covers RPi
            _ => {
                http_response(stream, 400, "no wifi device");
                return;
            }
        };
        tracing::info!("Dashboard: disconnecting WiFi iface={}", iface);
        let body = match crate::wifi::disconnect(&iface) {
            Ok(_) => serde_json::to_string(&serde_json::json!({"ok": true})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if request_matches(&req_line, "POST", "/api/wifi/forget") {
        // Body: {"uuid": "..."}
        let body = match read_http_body(&mut stream, DASHBOARD_SMALL_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                respond_http_body_error(stream, e);
                return;
            }
        };
        let req: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                http_response(stream, 400, &format!("invalid JSON: {}", e));
                return;
            }
        };
        let uuid = match req.get("uuid").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => {
                http_response(stream, 400, "missing uuid");
                return;
            }
        };
        tracing::info!("Dashboard: forgetting WiFi profile uuid={}", uuid);
        let body = match crate::wifi::forget(uuid) {
            Ok(_) => serde_json::to_string(&serde_json::json!({"ok": true})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if request_matches(&req_line, "POST", "/api/wifi/radio") {
        // Body: {"enabled": true|false}
        let body = match read_http_body(&mut stream, DASHBOARD_SMALL_BODY_MAX) {
            Ok(body) => body,
            Err(e) => {
                respond_http_body_error(stream, e);
                return;
            }
        };
        let req: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                http_response(stream, 400, &format!("invalid JSON: {}", e));
                return;
            }
        };
        let enabled = req.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
        tracing::info!("Dashboard: setting WiFi radio enabled={}", enabled);
        let body = match crate::wifi::set_radio(enabled) {
            Ok(_) => serde_json::to_string(&serde_json::json!({"ok": true})).unwrap_or_default(),
            Err(e) => serde_json::to_string(&serde_json::json!({"ok": false, "error": e})).unwrap_or_default(),
        };
        http_json_response(stream, 200, &body);
    } else if request_matches(&req_line, "GET", "/api/wifi/available") {
        // Cheap probe used by the dashboard to decide whether to even show
        // the WiFi tab. Returns {"available": true|false}.
        drain_http_headers(&mut stream);
        let body = serde_json::to_string(&serde_json::json!({
            "available": crate::wifi::available()
        }))
        .unwrap_or_default();
        http_json_response(stream, 200, &body);
    } else if request_is_api(&req_line) {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        http_json_response(buf.into_inner(), 404, r#"{"ok":false,"error":"API endpoint not found"}"#);
    } else {
        let mut buf = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let _ = buf.read_line(&mut line);
            if line == "\r\n" || line.is_empty() || line == "\n" {
                break;
            }
        }
        serve_dashboard_asset(buf.into_inner(), static_dir.as_deref(), &req_line);
    }
}

fn handle_ws(
    stream: TcpStream,
    state: DashboardState,
    clients: WsClients,
    cmd_tx: Arc<Mutex<Option<CmdSender>>>,
    update_state: SharedUpdateState,
    _auth: Option<(String, String)>,
) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(50)));

    // Note: cookie-based auth is checked by handle_connection BEFORE we get here,
    // so we don't need to re-validate during the WS upgrade. The cookie travels on
    // the Upgrade request and was already verified against the session store.
    let callback = move |_req: &Request, res: Response| -> Result<Response, _> { Ok(res) };

    let mut ws = match accept_hdr(stream, callback) {
        Ok(w) => w,
        Err(e) => {
            tracing::debug!("WS handshake failed: {}", e);
            return;
        }
    };

    // Register this connection for broadcasts
    let (broadcast_tx, broadcast_rx) = crossbeam_channel::bounded::<String>(DASHBOARD_WS_BROADCAST_QUEUE_MAX);
    {
        let mut c = clients.lock().unwrap();
        c.push(broadcast_tx);
    }

    // Send initial snapshot.
    let _ = ws.send(Message::Text(dashboard_snapshot_json(&state)));

    let _ = ws.get_ref().set_read_timeout(Some(std::time::Duration::from_millis(20)));

    loop {
        // Drain outbound broadcast messages first
        while let Ok(msg) = broadcast_rx.try_recv() {
            if ws.send(Message::Text(msg)).is_err() {
                return;
            }
        }

        // Then check for inbound messages from browser
        match ws.read() {
            Ok(Message::Text(text)) => {
                handle_ws_command(&text, &state, &cmd_tx, &update_state);
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(data)) => {
                let _ = ws.send(Message::Pong(data));
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
}

fn dashboard_snapshot_json(state: &DashboardState) -> String {
    let s = state.read().unwrap();
    let ms = s.snapshot_ms();
    let calls = s.snapshot_calls();
    let logs: Vec<_> = s.log_ring.iter().cloned().collect();
    let last_heard: Vec<_> = s.last_heard.iter().cloned().collect();
    let brew_online = s.brew_online;
    let brew_version = s.brew_version;
    let fallback_active = s.fallback_config_active;
    let fallback_reason = s.fallback_config_reason.clone();
    let last_tx_visual = s.last_tx_visual.clone();
    let last_tx_quality = s.last_tx_quality.clone();
    let last_sdr_health = s.last_sdr_health.clone();
    let last_sys_health = s.last_sys_health.clone();
    let last_health = s.last_health.clone();
    drop(s);

    serde_json::to_string(&serde_json::json!({
        "type": "snapshot",
        "ms": ms,
        "calls": calls,
        "log": logs,
        "brew_online": brew_online,
        "brew_version": brew_version,
        "last_heard": last_heard,
        "fallback_config_active": fallback_active,
        "fallback_config_reason": fallback_reason,
        "last_tx_visual": last_tx_visual,
        "last_tx_quality": last_tx_quality,
        "last_sdr_health": last_sdr_health,
        "last_sys_health": last_sys_health,
        "last_health": last_health,
    }))
    .unwrap_or_else(|_| r#"{"type":"snapshot","ms":[],"calls":[],"log":[],"last_heard":[]}"#.to_string())
}

fn serve_dashboard_snapshot(stream: TcpStream, state: &DashboardState) {
    let body = dashboard_snapshot_json(state);
    http_json_response(stream, 200, &body);
}

fn dashboard_calls_json(state: &DashboardState) -> String {
    let s = state.read().unwrap();
    let calls = s.snapshot_calls();
    let last_heard: Vec<_> = s.last_heard.iter().cloned().collect();
    let brew_online = s.brew_online;
    let brew_version = s.brew_version;
    drop(s);

    serde_json::to_string(&serde_json::json!({
        "type": "calls",
        "calls": calls,
        "last_heard": last_heard,
        "brew_online": brew_online,
        "brew_version": brew_version,
    }))
    .unwrap_or_else(|_| r#"{"type":"calls","calls":[],"last_heard":[]}"#.to_string())
}

fn serve_dashboard_calls(stream: TcpStream, state: &DashboardState) {
    let body = dashboard_calls_json(state);
    http_json_response(stream, 200, &body);
}

fn energy_saving_config_label(mode: u8) -> String {
    if mode == tetra_config::bluestation::ENERGY_SAVING_MODE_AUTO {
        "auto".to_string()
    } else if mode == 0 {
        "StayAlive".to_string()
    } else if (1..=7).contains(&mode) {
        format!("EG{}", mode)
    } else {
        format!("unknown({})", mode)
    }
}

fn gain_map_json(gains: &std::collections::HashMap<String, f64>) -> serde_json::Value {
    let mut rows: Vec<_> = gains.iter().collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    serde_json::Value::Object(
        rows.into_iter()
            .map(|(name, value)| (name.clone(), serde_json::json!(value)))
            .collect(),
    )
}

fn cached_active_dashboard_config(config_path: &str) -> Result<tetra_config::bluestation::StackConfig, String> {
    let active_path = active_profile_path(config_path);
    let metadata = fs::metadata(&active_path).map_err(|e| format!("active config metadata failed for {}: {}", active_path.display(), e))?;
    let modified = metadata.modified().ok();
    let len = metadata.len();
    let cache_key = config_path.to_string();
    let cache = DASHBOARD_CONFIG_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().map_err(|_| "dashboard config cache lock poisoned".to_string())?;
    if let Some(cached) = guard.get(&cache_key) {
        if cached.config_path == active_path && cached.modified == modified && cached.len == len {
            return Ok(cached.config.clone());
        }
    }
    let config = tetra_config::bluestation::parsing::from_file(&active_path)
        .map_err(|e| format!("active config parse failed for {}: {}", active_path.display(), e))?;
    guard.insert(
        cache_key,
        CachedDashboardConfig {
            config_path: active_path,
            modified,
            len,
            config: config.clone(),
        },
    );
    Ok(config)
}

fn dashboard_site_json(
    state: &DashboardState,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    config_path: &str,
) -> String {
    let (calls, last_tx_visual, last_tx_quality, last_sdr_health, last_sys_health, last_health, radio_count, brew_online, brew_version) = {
        let s = state.read().unwrap();
        (
            s.snapshot_calls(),
            s.last_tx_visual.clone(),
            s.last_tx_quality.clone(),
            s.last_sdr_health.clone(),
            s.last_sys_health.clone(),
            s.last_health.clone(),
            s.ms_map.len(),
            s.brew_online,
            s.brew_version,
        )
    };

    let config = match cached_active_dashboard_config(config_path) {
        Ok(cfg) => {
            let freq_info = tetra_core::freqs::FreqInfo::from_components(
                cfg.cell.freq_band,
                cfg.cell.main_carrier,
                cfg.cell.freq_offset_hz,
                cfg.cell.reverse_operation,
                cfg.cell.duplex_spacing_id,
                cfg.cell.custom_duplex_spacing,
            )
            .ok();
            let (derived_dl_hz, derived_ul_hz, duplex_spacing_hz) = freq_info
                .as_ref()
                .map(|freq| {
                    let (dl, ul) = freq.get_freqs();
                    (Some(dl), Some(ul), Some(freq.duplex_spacing_val))
                })
                .unwrap_or((None, None, None));

            let soapy = cfg.phy_io.soapysdr.as_ref();
            let soapy_tx_hz = soapy.map(|s| s.dl_freq);
            let soapy_rx_hz = soapy.map(|s| s.ul_freq);
            let (soapy_tx_corrected_hz, soapy_tx_ppm_error_hz) = soapy
                .map(|s| {
                    let (corrected, error) = s.dl_freq_corrected();
                    (Some(corrected), Some(error))
                })
                .unwrap_or((None, None));
            let (soapy_rx_corrected_hz, soapy_rx_ppm_error_hz) = soapy
                .map(|s| {
                    let (corrected, error) = s.ul_freq_corrected();
                    (Some(corrected), Some(error))
                })
                .unwrap_or((None, None));
            let frequency_match = match (derived_dl_hz, derived_ul_hz, soapy_tx_hz, soapy_rx_hz) {
                (Some(dl), Some(ul), Some(tx), Some(rx)) => (tx.round() as u32 == dl) && (rx.round() as u32 == ul),
                _ => false,
            };

            let (timeslot_owners, live_sds_queue_len, whitelist_override, carrier_inhibited) = if let Some(shared) = shared_config {
                let stack_state = shared.state_read();
                let timeslot_owners: Vec<_> = (2..=4)
                    .map(|ts| {
                        serde_json::json!({
                            "ts": ts,
                            "owner": stack_state.timeslot_alloc.owner(ts).map(|owner| format!("{:?}", owner)),
                            "free": stack_state.timeslot_alloc.is_free(ts),
                        })
                    })
                    .collect();
                let live_sds_queue_len = stack_state.live_sds_queue.len();
                let whitelist_override = stack_state.issi_whitelist_override.clone();
                let carrier_inhibited = stack_state.carrier_inhibited;
                (timeslot_owners, live_sds_queue_len, whitelist_override, carrier_inhibited)
            } else {
                (Vec::new(), 0, None, false)
            };

            let whitelist_count = whitelist_override
                .as_ref()
                .map(|list| list.len())
                .unwrap_or_else(|| cfg.security.issi_whitelist.len());
            let whitelist_source = if whitelist_override.is_some() { "runtime" } else { "config" };

            serde_json::json!({
            "available": true,
            "stack_mode": format!("{:?}", cfg.stack_mode),
            "phy_backend": format!("{:?}", cfg.phy_io.backend),
            "network": {
                "mcc": cfg.net.mcc,
                "mnc": cfg.net.mnc,
            },
            "cell": {
                "main_carrier": cfg.cell.main_carrier,
                "freq_band": cfg.cell.freq_band,
                "freq_offset_hz": cfg.cell.freq_offset_hz,
                "duplex_spacing_id": cfg.cell.duplex_spacing_id,
                "custom_duplex_spacing_hz": cfg.cell.custom_duplex_spacing,
                "duplex_spacing_hz": duplex_spacing_hz,
                "reverse_operation": cfg.cell.reverse_operation,
                "location_area": cfg.cell.location_area,
                "system_code": cfg.cell.system_code,
                "colour_code": cfg.cell.colour_code,
                "frame_18_ext": cfg.cell.frame_18_ext,
                "ts_reserved_frames": cfg.cell.ts_reserved_frames,
                "u_plane_dtx": cfg.cell.u_plane_dtx,
                "hangtime_secs": cfg.cell.hangtime_secs,
                "call_timeout_secs": cfg.cell.call_timeout_secs,
                "ul_inactivity_secs": cfg.cell.ul_inactivity_secs,
                "legacy_gssi_group_call": cfg.cell.legacy_gssi_group_call,
                "energy_saving_mode": cfg.cell.energy_saving_mode,
                "energy_saving_label": energy_saving_config_label(cfg.cell.energy_saving_mode),
                "timezone": cfg.cell.timezone,
                "derived_dl_hz": derived_dl_hz,
                "derived_ul_hz": derived_ul_hz,
            },
            "soapy": soapy.map(|s| serde_json::json!({
                "tx_hz": s.dl_freq,
                "rx_hz": s.ul_freq,
                "tx_corrected_hz": soapy_tx_corrected_hz,
                "rx_corrected_hz": soapy_rx_corrected_hz,
                "tx_ppm_error_hz": soapy_tx_ppm_error_hz,
                "rx_ppm_error_hz": soapy_rx_ppm_error_hz,
                "ppm_err": s.ppm_err,
                "device": s.device,
                "rx_ant": s.rx_ant,
                "tx_ant": s.tx_ant,
                "rx_ch": s.rx_ch,
                "tx_ch": s.tx_ch,
                "sample_rate": s.fs,
                "rx_gains": gain_map_json(&s.rx_gains),
                "tx_gains": gain_map_json(&s.tx_gains),
                "frequency_match": frequency_match,
            })),
            "brew": cfg.brew.as_ref().map(|brew| serde_json::json!({
                "configured": true,
                "host": brew.host,
                "port": brew.port,
                "tls": brew.tls,
            })),
            "timeslot_owners": timeslot_owners,
            "rf_control": {
                "carrier_inhibited": carrier_inhibited,
                "carrier_state": if carrier_inhibited { "carrier_inhibited" } else { "carrier_active" },
            },
            "services": {
                "voice": cfg.cell.voice_service,
                "sds_live_queue_len": live_sds_queue_len,
                "whitelist_enabled": whitelist_count > 0,
                "whitelist_count": whitelist_count,
                "whitelist_source": whitelist_source,
                "home_mode_display": cfg.cell.home_mode_display.is_some(),
                "sds_broadcast": cfg.cell.sds_broadcast.is_some(),
                "wx_service": cfg.wx_service.enabled,
            },
            "health": {
                "enabled": cfg.health.enabled,
                "snapshot_interval_secs": cfg.health.snapshot_interval_secs,
                "restart_on_core_stall": cfg.health.restart_on_core_stall,
                "restart_after_critical_secs": cfg.health.restart_after_critical_secs,
                "restart_cooldown_secs": cfg.health.restart_cooldown_secs,
            },
            })
        }
        Err(reason) => serde_json::json!({
            "available": false,
            "reason": reason,
        }),
    };

    let mut timeslots = Vec::new();
    for ts in 1..=4u8 {
        let call = calls.iter().find(|call| call.ts == ts || call.secondary_ts == Some(ts));
        let secondary = call.is_some_and(|call| call.secondary_ts == Some(ts));
        timeslots.push(serde_json::json!({
            "ts": ts,
            "role": if ts == 1 { "control" } else { "traffic" },
            "state": if ts == 1 { "control" } else if call.is_some() { "active" } else { "idle" },
            "secondary": secondary,
            "call": call,
        }));
    }

    serde_json::to_string(&serde_json::json!({
        "type": "site",
        "config": config,
        "counts": {
            "radios": radio_count,
            "calls": calls.len(),
        },
        "backhaul": {
            "brew_online": brew_online,
            "brew_version": brew_version,
        },
        "timeslots": timeslots,
        "rf_cached": {
            "tx_visual": last_tx_visual,
            "tx_quality": last_tx_quality,
            "sdr_health": last_sdr_health,
            "sys_health": last_sys_health,
            "health": last_health,
        },
    }))
    .unwrap_or_else(|_| r#"{"type":"site","config":{"available":false}}"#.to_string())
}

fn serve_dashboard_site(
    stream: TcpStream,
    state: &DashboardState,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    config_path: &str,
) {
    let body = dashboard_site_json(state, shared_config, config_path);
    http_json_response(stream, 200, &body);
}

fn dashboard_session_valid(headers: &str, sessions: &SharedSessionStore) -> bool {
    parse_session_cookie(headers)
        .and_then(|token| {
            let mut store = sessions.lock().ok()?;
            Some(store.validate(&token))
        })
        .unwrap_or(false)
}

fn dashboard_auth_status_json(auth: &Option<(String, String)>, sessions: &SharedSessionStore, headers: &str) -> String {
    let auth_required = auth.is_some();
    let session_valid = if auth_required {
        dashboard_session_valid(headers, sessions)
    } else {
        true
    };

    serde_json::to_string(&serde_json::json!({
        "auth_required": auth_required,
        "session_valid": session_valid,
        "session_cookie": "fs_session",
    }))
    .unwrap_or_else(|_| r#"{"auth_required":true,"session_valid":false}"#.to_string())
}

fn serve_dashboard_auth_status(stream: TcpStream, auth: &Option<(String, String)>, sessions: &SharedSessionStore, headers: &str) {
    let body = dashboard_auth_status_json(auth, sessions, headers);
    http_json_response(stream, 200, &body);
}

fn handle_ws_command(text: &str, state: &DashboardState, cmd_tx: &Arc<Mutex<Option<CmdSender>>>, update_state: &SharedUpdateState) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };

    let send_cmd = |cmd: ControlCommand| -> bool {
        if let Ok(guard) = cmd_tx.lock() {
            if let Some(ref tx) = *guard {
                return tx.try_send(cmd).is_ok();
            }
        }
        false
    };

    match v.get("type").and_then(|t| t.as_str()) {
        Some("kick") => {
            let issi = v.get("issi").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
            if issi == 0 {
                return;
            }
            tracing::info!("Dashboard: kick ISSI {}", issi);
            if !send_cmd(ControlCommand::KickMs { issi }) {
                tracing::warn!("Dashboard: no control dispatcher for kick");
            }
            let mut s = state.write().unwrap();
            s.push_log("INFO", format!("Kick requested for ISSI {}", issi));
        }
        Some("restart") => {
            tracing::info!("Dashboard: restart service requested");
            if !send_cmd(ControlCommand::RestartService) {
                tracing::warn!("Dashboard: control dispatcher unavailable for restart request");
                let mut s = state.write().unwrap();
                s.push_log("WARN", "Restart request was not queued; control channel unavailable".to_string());
            }
        }
        Some("shutdown") => {
            tracing::warn!("Dashboard: host poweroff requested");
            if !send_cmd(ControlCommand::PowerOffHost) {
                tracing::warn!("Dashboard: control dispatcher unavailable for host poweroff request");
                let mut s = state.write().unwrap();
                s.push_log(
                    "WARN",
                    "Host poweroff request was not queued; control channel unavailable".to_string(),
                );
            }
        }
        Some("update") => {
            if !DASHBOARD_OTA_UPDATE_ENABLED {
                tracing::warn!("Dashboard: OTA update requested while disabled");
                return;
            }
            let mut u = update_state.lock().unwrap();
            if u.phase == UpdatePhase::Running {
                tracing::warn!("Dashboard: update already in progress, ignoring");
                return;
            }
            u.start();
            drop(u);
            tracing::info!("Dashboard: OTA update triggered via WS");
            // config_path not available here; caller must use POST /api/update instead
            // This WS variant is for UI convenience — it signals the browser to poll /api/update/status
            // The actual update must be triggered via POST /api/update from JS first.
            // Here we just ack that status polling should begin.
            let mut s = state.write().unwrap();
            s.push_log("INFO", "OTA update started — check /api/update/status for progress".to_string());
        }
        Some("sds") => {
            let Ok(sds) = parse_ws_sds_command(&v) else {
                return;
            };
            let dest = sds.dest_issi;
            let msg_text = sds.message;
            tracing::info!("Dashboard: SDS to {} = {}", dest, msg_text);

            // Send bare UTF-8 text. CMCE is the single SDS-TL Type4 encoder.
            let payload = msg_text.as_bytes().to_vec();
            let len_bits = (payload.len() * 8) as u16;

            if send_cmd(ControlCommand::SendSds {
                handle: 0,
                source_ssi: 9999,
                dest_ssi: dest,
                dest_is_group: false,
                len_bits,
                payload,
            }) {
                let mut s = state.write().unwrap();
                s.push_log("INFO", format!("SDS queued to {}: {}", dest, msg_text));
            } else {
                tracing::warn!("Dashboard: control dispatcher unavailable for SDS to {}", dest);
                let mut s = state.write().unwrap();
                s.push_log("WARN", format!("SDS to {} was not queued; control channel unavailable", dest));
            }
        }
        Some("wap") => {
            let Ok(wap) = parse_ws_wap_command(&v) else {
                return;
            };
            let page_text = if wap.color { WAP_MVP_COLOR_PAGE_TEXT } else { WAP_MVP_PAGE_TEXT };
            let payload = wap_sds_type4_payload(page_text);
            if payload.len() > WAP_SDS_TYPE4_MAX_BYTE_ALIGNED_PAYLOAD_BYTES {
                tracing::warn!("Dashboard: SDS WAP page is too large for SDS Type4");
                return;
            }
            let len_bits = (payload.len() * 8) as u16;

            // EN 300 392-2 table 29.21 identifies direct WAP-over-SDS by PID
            // 0x04. This dashboard command drives that raw Type4 path only; it
            // does not advertise or imply an SNDCP/IP packet-data bearer.
            if send_cmd(ControlCommand::SendRawSds {
                handle: 0,
                source_ssi: DASHBOARD_WAP_SOURCE_SSI,
                dest_ssi: wap.dest_issi,
                dest_is_group: false,
                sdti: 3,
                len_bits,
                payload,
            }) {
                tracing::info!("Dashboard: SDS WAP page sent to {}", wap.dest_issi);
                let mut s = state.write().unwrap();
                s.push_log("INFO", format!("SDS WAP page sent to ISSI {}", wap.dest_issi));
            } else {
                tracing::warn!("Dashboard: no control dispatcher for SDS WAP page");
            }
        }
        _ => {}
    }
}

fn serve_update_status(stream: TcpStream, update_state: &SharedUpdateState) {
    let (phase_str, success, log) = {
        let u = update_state.lock().unwrap();
        let phase_str = match &u.phase {
            UpdatePhase::Idle => "idle",
            UpdatePhase::Running => "running",
            UpdatePhase::Done { success: true } => "done_ok",
            UpdatePhase::Done { success: false } => "done_err",
        };
        let success = matches!(u.phase, UpdatePhase::Done { success: true });
        (phase_str, success, u.log.clone())
    };
    let body = format!(
        "{{\"status\":\"{}\",\"success\":{},\"log\":{}}}",
        phase_str,
        success,
        serde_json::to_string(&log).unwrap_or_else(|_| "\"\"".into())
    );
    http_json_response(stream, 200, &body);
}

/// GET /api/update/check — query GitHub for the latest release and report whether a newer
/// version than the running build exists. Best-effort; on any failure returns
/// check_failed=true so the dashboard simply hides the badge.
fn serve_update_check(stream: TcpStream) {
    let result = crate::net_dashboard::update_check::list_deb_releases(tetra_core::STACK_VERSION, &debian_arch());
    let body = result.to_json();
    http_json_response(stream, 200, &body);
}

fn debian_arch() -> String {
    Command::new("dpkg")
        .arg("--print-architecture")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|arch| !arch.is_empty())
        .unwrap_or_else(|| match std::env::consts::ARCH {
            "aarch64" => "arm64".to_string(),
            "x86_64" => "amd64".to_string(),
            "arm" => "armhf".to_string(),
            other => other.to_string(),
        })
}

fn parse_deb_update_request(body: &str) -> Result<DebUpdateRequest, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| format!("invalid JSON: {e}"))?;
    let version = v.get("version").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let asset_name = v.get("asset_name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let url = v
        .get("url")
        .or_else(|| v.get("deb_url"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    validate_deb_update_request(&version, &asset_name, &url)?;
    Ok(DebUpdateRequest { version, asset_name, url })
}

fn validate_deb_update_request(version: &str, asset_name: &str, url: &str) -> Result<(), String> {
    let arch = debian_arch();
    if version.is_empty() || asset_name.is_empty() || url.is_empty() {
        return Err("version, asset_name and url are required".to_string());
    }
    if version.contains("_dev") || asset_name.contains("_dev") || url.contains("_dev") {
        return Err("development releases are not installable from dashboard".to_string());
    }
    if !asset_name.ends_with(".deb") || !asset_name.starts_with("nexus-bs_") || !asset_name.contains(&format!("_{arch}.deb")) {
        return Err(format!("asset must be a nexus-bs {arch} .deb package"));
    }
    if asset_name.contains('/') || asset_name.contains('\\') || asset_name.contains("..") {
        return Err("invalid asset name".to_string());
    }
    let allowed_prefix = "https://github.com/invictus737/nexus-bs/releases/download/";
    if !url.starts_with(allowed_prefix) {
        return Err("release URL is not a trusted Nexus-BS GitHub release asset".to_string());
    }
    if !url.ends_with(asset_name) {
        return Err("release URL does not match selected asset".to_string());
    }
    Ok(())
}

/// GET /api/whitelist — return the effective whitelist as JSON:
/// `{"issi_whitelist":[...], "source":"override"|"config", "enabled":bool}`.
/// `enabled` is false when the list is empty (open network).
fn serve_whitelist_get(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
    let (list, source): (Vec<u32>, &str) = match shared_config {
        Some(cfg) => {
            let override_list = cfg.state_read().issi_whitelist_override.clone();
            match override_list {
                Some(l) => (l, "override"),
                None => (cfg.config().security.issi_whitelist.clone(), "config"),
            }
        }
        None => (Vec::new(), "config"),
    };
    let items: Vec<String> = list.iter().map(|n| n.to_string()).collect();
    let body = format!(
        "{{\"issi_whitelist\":[{}],\"source\":\"{}\",\"enabled\":{}}}",
        items.join(","),
        source,
        !list.is_empty()
    );
    http_json_response(stream, 200, &body);
}

/// POST /api/whitelist — set the whitelist. Body: JSON array `[1,2,3]` or
/// `{"issi_whitelist":[1,2,3]}`. Applies immediately via the StackState override AND
/// rewrites the TOML so it survives a restart. An empty list = open network.
fn serve_whitelist_post(
    stream: TcpStream,
    shared_config: &Option<tetra_config::bluestation::SharedConfig>,
    config_path: &str,
    body: &str,
    cmd_tx: &Arc<Mutex<Option<CmdSender>>>,
) {
    use crate::net_dashboard::whitelist;

    let list = match whitelist::parse_whitelist_body(body) {
        Ok(l) => l,
        Err(e) => {
            http_response(stream, 400, &format!("Invalid whitelist: {e}"));
            return;
        }
    };

    let Some(cfg) = shared_config else {
        http_response(stream, 503, "Config not available");
        return;
    };

    // 1) Apply at runtime immediately so the next registration sees it.
    {
        let mut state = cfg.state_write();
        state.issi_whitelist_override = Some(list.clone());
    }

    // 2) Enforce immediately on terminals that are ALREADY registered. The whitelist is
    //    only checked at registration time, so without this an enabling edit would leave
    //    disallowed radios connected (looks like access control never turned on) and a
    //    removal would only take effect when the terminal next re-registers — i.e. on a
    //    reboot. Kick every currently-registered ISSI the new list no longer allows; it
    //    re-registers and is then rejected by MM. Empty list = open network = kick nobody.
    if !list.is_empty() {
        let to_kick: Vec<u32> = {
            let state = cfg.state_read();
            state
                .subscribers
                .all_registered_issis()
                .filter(|issi| !list.contains(issi))
                .collect()
        };
        for issi in to_kick {
            tracing::info!("Dashboard: whitelist change — kicking non-whitelisted ISSI {}", issi);
            send_control_cmd(cmd_tx, ControlCommand::KickMs { issi });
        }
    }

    // 3) Persist to TOML so it survives a restart.
    if let Err(e) = whitelist::write_whitelist_to_toml(config_path, &list) {
        tracing::warn!("Dashboard: whitelist applied at runtime but failed to persist to TOML: {}", e);
        // Runtime change still took effect; report partial success so the operator knows
        // to check file permissions.
        http_response(stream, 200, "Applied at runtime; failed to write config file (check permissions)");
        return;
    }

    tracing::info!("Dashboard: ISSI whitelist updated ({} entries)", list.len());
    http_response(stream, 200, "OK");
}

// ---------------------------------------------------------------------------
// WX/METAR service config (dashboard-editable). See net_dashboard::wx_service.
// ---------------------------------------------------------------------------

/// GET /api/wx — return the effective WX service settings as JSON.
fn serve_wx_get(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>) {
    let wx = match shared_config {
        Some(cfg) => cfg.effective_wx_service(),
        None => tetra_config::bluestation::CfgWxService::default(),
    };
    let body = format!(
        "{{\"enabled\":{},\"service_issi\":{},\"periodic_enabled\":{},\"periodic_issi\":{},\"periodic_is_group\":{},\"periodic_icao\":\"{}\",\"periodic_interval_secs\":{}}}",
        wx.enabled,
        wx.service_issi,
        wx.periodic_enabled,
        wx.periodic_issi,
        wx.periodic_is_group,
        wx.periodic_icao.replace('\\', "\\\\").replace('"', "\\\""),
        wx.periodic_interval_secs
    );
    http_json_response(stream, 200, &body);
}

/// POST /api/wx — update WX service settings. Body: JSON object with the same fields as
/// GET. Applies immediately via the StackState override AND rewrites the TOML.
fn serve_wx_post(stream: TcpStream, shared_config: &Option<tetra_config::bluestation::SharedConfig>, config_path: &str, body: &str) {
    use tetra_config::bluestation::WxRuntimeOverride;

    let json: serde_json::Value = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(e) => {
            http_response(stream, 400, &format!("Invalid JSON: {e}"));
            return;
        }
    };

    let Some(cfg) = shared_config else {
        http_response(stream, 503, "Config not available");
        return;
    };

    // Start from the current effective values so a partial POST only changes what it sends.
    let cur = cfg.effective_wx_service();
    let as_u32 = |v: &serde_json::Value, k: &str, d: u32| v.get(k).and_then(|x| x.as_u64()).map(|n| n as u32).unwrap_or(d);
    let as_u64 = |v: &serde_json::Value, k: &str, d: u64| v.get(k).and_then(|x| x.as_u64()).unwrap_or(d);
    let as_bool = |v: &serde_json::Value, k: &str, d: bool| v.get(k).and_then(|x| x.as_bool()).unwrap_or(d);
    let icao = json
        .get("periodic_icao")
        .and_then(|x| x.as_str())
        .map(|s| {
            s.trim()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(4)
                .collect::<String>()
                .to_uppercase()
        })
        .unwrap_or(cur.periodic_icao.clone());

    let ov = WxRuntimeOverride {
        enabled: as_bool(&json, "enabled", cur.enabled),
        service_issi: as_u32(&json, "service_issi", cur.service_issi),
        periodic_enabled: as_bool(&json, "periodic_enabled", cur.periodic_enabled),
        periodic_issi: as_u32(&json, "periodic_issi", cur.periodic_issi),
        periodic_is_group: as_bool(&json, "periodic_is_group", cur.periodic_is_group),
        periodic_icao: icao,
        periodic_interval_secs: as_u64(&json, "periodic_interval_secs", cur.periodic_interval_secs),
    };

    // 1) Apply at runtime.
    {
        let mut state = cfg.state_write();
        state.wx_override = Some(ov.clone());
    }

    // 2) Persist to TOML.
    if let Err(e) = crate::net_dashboard::wx_service::write_wx_to_toml(config_path, &ov) {
        tracing::warn!("Dashboard: WX applied at runtime but failed to persist to TOML: {}", e);
        http_response(stream, 200, "Applied at runtime; failed to write config file (check permissions)");
        return;
    }

    tracing::info!(
        "Dashboard: WX service updated (enabled={} svc_issi={} periodic={} -> {} icao={})",
        ov.enabled,
        ov.service_issi,
        ov.periodic_enabled,
        ov.periodic_issi,
        ov.periodic_icao
    );
    http_response(stream, 200, "OK");
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CpuDescriptor {
    model: String,
    cores: usize,
}

fn cpuinfo_values<'a>(cpuinfo: &'a str, key: &str) -> Vec<&'a str> {
    cpuinfo
        .lines()
        .filter_map(|line| {
            let (line_key, value) = line.split_once(':')?;
            if line_key.trim().eq_ignore_ascii_case(key) {
                Some(value.trim())
            } else {
                None
            }
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn first_cpuinfo_value<'a>(cpuinfo: &'a str, key: &str) -> Option<&'a str> {
    cpuinfo_values(cpuinfo, key).into_iter().next()
}

fn parse_hex_or_decimal(value: &str) -> Option<u32> {
    let trimmed = value.trim();
    let without_prefix = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X")).unwrap_or(trimmed);
    if trimmed.starts_with("0x") || trimmed.starts_with("0X") || without_prefix.chars().any(|c| c.is_ascii_alphabetic()) {
        u32::from_str_radix(without_prefix, 16).ok()
    } else {
        without_prefix.parse::<u32>().ok()
    }
}

fn arm_implementer_name(implementer: u32) -> Option<&'static str> {
    match implementer {
        0x41 => Some("ARM"),
        0x42 => Some("Broadcom"),
        0x43 => Some("Cavium"),
        0x46 => Some("Fujitsu"),
        0x48 => Some("HiSilicon"),
        0x4d => Some("Motorola/Freescale"),
        0x4e => Some("NVIDIA"),
        0x51 => Some("Qualcomm"),
        0x53 => Some("Samsung"),
        0x56 => Some("Marvell"),
        0x61 => Some("Apple"),
        0x69 => Some("Intel"),
        _ => None,
    }
}

fn arm_core_name(implementer: u32, part: u32) -> Option<&'static str> {
    match (implementer, part) {
        (0x41, 0xd01) => Some("Cortex-A32"),
        (0x41, 0xd02) => Some("Cortex-A34"),
        (0x41, 0xd03) => Some("Cortex-A53"),
        (0x41, 0xd04) => Some("Cortex-A35"),
        (0x41, 0xd05) => Some("Cortex-A55"),
        (0x41, 0xd07) => Some("Cortex-A57"),
        (0x41, 0xd08) => Some("Cortex-A72"),
        (0x41, 0xd09) => Some("Cortex-A73"),
        (0x41, 0xd0a) => Some("Cortex-A75"),
        (0x41, 0xd0b) => Some("Cortex-A76"),
        (0x41, 0xd0c) => Some("Neoverse-N1"),
        (0x41, 0xd0d) => Some("Cortex-A77"),
        (0x41, 0xd0e) => Some("Cortex-A76AE"),
        (0x41, 0xd40) => Some("Neoverse-V1"),
        (0x41, 0xd41) => Some("Cortex-A78"),
        (0x41, 0xd42) => Some("Cortex-A78AE"),
        (0x41, 0xd44) => Some("Cortex-X1"),
        (0x41, 0xd46) => Some("Cortex-A510"),
        (0x41, 0xd47) => Some("Cortex-A710"),
        (0x41, 0xd48) => Some("Cortex-X2"),
        (0x41, 0xd49) => Some("Neoverse-N2"),
        (0x41, 0xd4a) => Some("Neoverse-E1"),
        (0x42, 0x00f) => Some("Brahma-B15"),
        (0x42, 0x100) => Some("Brahma-B53"),
        (0x51, 0x06f) => Some("Krait"),
        (0x51, 0x201) | (0x51, 0x205) | (0x51, 0x211) | (0x51, 0x801) => Some("Kryo"),
        (0x51, 0x800) => Some("Falkor-V1"),
        (0x4e, 0x000) => Some("Denver"),
        (0x4e, 0x003) => Some("Denver 2"),
        (0x4e, 0x004) => Some("Carmel"),
        _ => None,
    }
}

fn vendor_from_board_context(compatible: Option<&str>, board_model: Option<&str>, hardware: Option<&str>) -> Option<&'static str> {
    let text = [compatible.unwrap_or(""), board_model.unwrap_or(""), hardware.unwrap_or("")]
        .join("\n")
        .to_lowercase();
    if text.contains("brcm,") || text.contains("bcm27") || text.contains("bcm28") || text.contains("raspberry pi") {
        Some("Broadcom")
    } else if text.contains("rockchip,") {
        Some("Rockchip")
    } else if text.contains("allwinner,") || text.contains("sunxi") {
        Some("Allwinner")
    } else if text.contains("amlogic,") {
        Some("Amlogic")
    } else if text.contains("qcom,") || text.contains("qualcomm") {
        Some("Qualcomm")
    } else if text.contains("mediatek,") {
        Some("MediaTek")
    } else if text.contains("nvidia,") {
        Some("NVIDIA")
    } else if text.contains("samsung,") {
        Some("Samsung")
    } else if text.contains("ti,") {
        Some("Texas Instruments")
    } else if text.contains("fsl,") || text.contains("nxp,") {
        Some("NXP")
    } else {
        None
    }
}

fn format_cpu_freq_khz(khz: u64) -> Option<String> {
    if khz == 0 {
        return None;
    }
    if khz >= 1_000_000 {
        let ghz = khz as f64 / 1_000_000.0;
        if (ghz - ghz.round()).abs() < 0.05 {
            Some(format!("{:.0}GHz", ghz))
        } else {
            Some(format!("{:.1}GHz", ghz))
        }
    } else {
        Some(format!("{}MHz", (khz + 500) / 1000))
    }
}

fn cpu_bits_label(cpuinfo: &str, arch: Option<&str>) -> Option<&'static str> {
    let arch_lc = arch.unwrap_or("").to_lowercase();
    if arch_lc.contains("64") || matches!(arch_lc.as_str(), "aarch64" | "arm64" | "riscv64" | "ppc64le" | "ppc64") {
        return Some("64-bit");
    }
    if arch_lc.contains("86") || arch_lc.starts_with("armv7") || arch_lc.starts_with("armv6") || arch_lc == "arm" {
        return Some("32-bit");
    }
    if first_cpuinfo_value(cpuinfo, "CPU architecture").is_some_and(|value| value == "8") {
        return Some("64-bit");
    }
    None
}

fn count_cpu_cores(cpuinfo: &str) -> usize {
    let processor_count = cpuinfo.lines().filter(|line| line.trim_start().starts_with("processor")).count();
    if processor_count > 0 {
        processor_count
    } else {
        std::thread::available_parallelism().map(usize::from).unwrap_or(0)
    }
}

fn build_cpu_descriptor(
    cpuinfo: &str,
    board_model: Option<&str>,
    compatible: Option<&str>,
    max_freq_khz: Option<u64>,
    arch: Option<&str>,
) -> CpuDescriptor {
    let cores = count_cpu_cores(cpuinfo);
    let model_name = first_cpuinfo_value(cpuinfo, "model name").map(str::to_string);
    let hardware = first_cpuinfo_value(cpuinfo, "Hardware");
    let cpu_model_field = first_cpuinfo_value(cpuinfo, "Model");
    let implementer = first_cpuinfo_value(cpuinfo, "CPU implementer").and_then(parse_hex_or_decimal);
    let part = first_cpuinfo_value(cpuinfo, "CPU part").and_then(parse_hex_or_decimal);

    let mut model = if let Some(model_name) = model_name.filter(|value| !value.eq_ignore_ascii_case("unknown")) {
        model_name
    } else if let (Some(implementer), Some(part)) = (implementer, part) {
        let core = arm_core_name(implementer, part).unwrap_or("ARM-compatible CPU");
        let vendor = vendor_from_board_context(compatible, board_model, hardware).or_else(|| arm_implementer_name(implementer));
        match vendor {
            Some(vendor) if !core.starts_with(vendor) => format!("{vendor} {core}"),
            _ => core.to_string(),
        }
    } else if let Some(hardware) = hardware {
        hardware.to_string()
    } else if let Some(model) = board_model.or(cpu_model_field) {
        model.to_string()
    } else {
        "unknown".to_string()
    };

    let model_lc = model.to_lowercase();
    if let Some(freq) = max_freq_khz.and_then(format_cpu_freq_khz)
        && !model_lc.contains("ghz")
        && !model_lc.contains("mhz")
    {
        model.push(' ');
        model.push_str(&freq);
    }

    if let Some(bits) = cpu_bits_label(cpuinfo, arch)
        && !model.contains("64-bit")
        && !model.contains("32-bit")
    {
        model.push(' ');
        model.push_str(bits);
    }

    CpuDescriptor { model, cores }
}

fn read_trimmed_string(path: &str) -> Option<String> {
    std::fs::read(path).ok().and_then(|bytes| {
        let text = String::from_utf8_lossy(&bytes).replace('\0', "\n");
        let trimmed = text.trim();
        if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
    })
}

fn detect_cpu_descriptor() -> CpuDescriptor {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let board_model = read_trimmed_string("/proc/device-tree/model");
    let compatible = read_trimmed_string("/proc/device-tree/compatible");
    let max_freq_khz = read_trimmed_string("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq").and_then(|s| s.parse::<u64>().ok());
    let arch = std::process::Command::new("uname")
        .arg("-m")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    build_cpu_descriptor(
        &cpuinfo,
        board_model.as_deref(),
        compatible.as_deref(),
        max_freq_khz,
        arch.as_deref(),
    )
}

fn serve_system_info(stream: TcpStream, config_path: &str, bs_started_at: Instant) {
    let product = dashboard_product_identity();
    let hostname = std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let uptime_secs: u64 = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(|n| n.parse::<f64>().ok()))
        .flatten()
        .map(|f| f as u64)
        .unwrap_or(0);
    let bs_uptime_secs = bs_started_at.elapsed().as_secs();

    let os_info = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("PRETTY_NAME="))
                .map(|l| l.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "Linux".to_string());

    let config_dir = std::path::Path::new(config_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());
    let active_config_name = read_active_config_marker(config_path).unwrap_or_else(|| active_config_filename(config_path));
    let active_config_path = std::path::Path::new(&config_dir)
        .join(&active_config_name)
        .to_string_lossy()
        .to_string();

    let cpu = detect_cpu_descriptor();

    // CPU load — /proc/stat first line: user nice system idle iowait irq softirq
    // Take a 100ms sample for a meaningful reading
    fn read_cpu_stat() -> Option<(u64, u64)> {
        let s = std::fs::read_to_string("/proc/stat").ok()?;
        let line = s.lines().next()?;
        let nums: Vec<u64> = line.split_whitespace().skip(1).filter_map(|n| n.parse().ok()).collect();
        if nums.len() < 4 {
            return None;
        }
        let idle = nums[3] + nums.get(4).copied().unwrap_or(0); // idle + iowait
        let total: u64 = nums.iter().sum();
        Some((total, idle))
    }
    let cpu_pct = if let (Some((t1, i1)), Some((t2, i2))) = (read_cpu_stat(), {
        std::thread::sleep(std::time::Duration::from_millis(100));
        read_cpu_stat()
    }) {
        let dt = t2.saturating_sub(t1);
        let di = i2.saturating_sub(i1);
        if dt > 0 { ((dt - di) * 100 / dt) as u8 } else { 0 }
    } else {
        0
    };

    // RAM — /proc/meminfo MemTotal and MemAvailable
    let (ram_total_mb, ram_used_mb) = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .map(|s| {
            let mut total = 0u64;
            let mut available = 0u64;
            for line in s.lines() {
                if line.starts_with("MemTotal:") {
                    total = line.split_whitespace().nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
                } else if line.starts_with("MemAvailable:") {
                    available = line.split_whitespace().nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
                }
            }
            (total / 1024, (total.saturating_sub(available)) / 1024)
        })
        .unwrap_or((0, 0));

    // CPU temperature — try common Linux thermal zone paths
    let cpu_temp_c: Option<f32> = [
        "/sys/class/thermal/thermal_zone0/temp",
        "/sys/class/thermal/thermal_zone1/temp",
        "/sys/devices/virtual/thermal/thermal_zone0/temp",
    ]
    .iter()
    .find_map(|path| {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .map(|t| t as f32 / 1000.0)
            .filter(|&t| t > 0.0 && t < 150.0) // sanity check
    });

    // RF / SoapySDR info — first check the binary exists at all, then run --find
    // (which is much cheaper than --probe and works even with no device attached;
    // --probe can hang for seconds on some drivers like HackRF/Pluto, and exits
    // with non-zero status when no device is found, which would have been
    // misreported as "not available").
    //
    // Try a couple of well-known install paths in addition to PATH because when
    // the stack runs under systemd it gets a minimal PATH that doesn't always
    // include /usr/local/bin where SoapySDR sometimes lands.
    let soapy_info = (|| -> String {
        let candidates = ["SoapySDRUtil", "/usr/bin/SoapySDRUtil", "/usr/local/bin/SoapySDRUtil"];
        for bin in &candidates {
            // First: does the binary respond to --info at all? That's the canonical
            // "is it installed?" check (`--info` is in every SoapySDR release and
            // returns the module summary regardless of attached hardware). We used
            // `--version` previously but on some SoapySDR builds it isn't a
            // recognised option, so the binary printed its help text with exit 0
            // and we misread that as "installed" → subsequently `--find` failed
            // and the user saw a confusing "SoapySDRUtil --find failed" with the
            // full help dump pasted in front of it. Thanks @shawnchain for the
            // PR comment.
            let probe = std::process::Command::new(bin).arg("--info").output();
            if let Ok(out) = probe {
                if out.status.success() {
                    // Now run --find to enumerate devices. Empty result means
                    // "no SDR connected" which is a valid, useful state to show.
                    let find = std::process::Command::new(bin)
                        .arg("--find")
                        .output()
                        .ok()
                        .filter(|o| o.status.success())
                        .map(|o| String::from_utf8_lossy(&o.stdout).to_string());
                    // Keep only the first few lines of --info (banner + API/ABI
                    // version + module path). Beyond that it dumps a long module
                    // listing that isn't useful in the dashboard card.
                    let info = String::from_utf8_lossy(&out.stdout);
                    let info_summary: String = info
                        .lines()
                        .filter(|l| {
                            let ll = l.to_lowercase();
                            ll.contains("lib version")
                                || ll.contains("api version")
                                || ll.contains("abi version")
                                || ll.contains("install root")
                        })
                        .take(4)
                        .collect::<Vec<&str>>()
                        .join("\n");
                    return match find {
                        Some(text) if text.lines().any(|l| l.to_lowercase().contains("found device")) => {
                            // Keep only the useful per-device lines (driver/serial/label) to
                            // avoid dumping pages of advertising.
                            let lines: Vec<&str> = text
                                .lines()
                                .filter(|l| {
                                    let ll = l.to_lowercase();
                                    ll.contains("found device")
                                        || ll.contains("driver")
                                        || ll.contains("serial")
                                        || ll.contains("label")
                                        || ll.contains("name")
                                        || ll.contains("manufacturer")
                                })
                                .take(20)
                                .collect();
                            format!("{}\n{}", info_summary, lines.join("\n"))
                        }
                        Some(_) => format!("{}\nNo SDR device detected.", info_summary),
                        None => format!("{}\nSoapySDRUtil --find failed.", info_summary),
                    };
                }
            }
        }
        // Falling through the loop without returning means no candidate path
        // successfully ran `--info` — the binary is genuinely missing.
        "SoapySDRUtil not installed (apt install soapysdr-tools).".to_string()
    })();

    // Auto-detected SDR name — set by `phy::components::soapy_settings::get_settings()`
    // at stack startup. None if no SoapySDR-backed phy is in use (file backend etc).
    let sdr_name = crate::phy::components::soapy_settings::detected_sdr_name()
        .filter(|name| !name.trim().is_empty() && !name.eq_ignore_ascii_case("unknown"))
        .or_else(|| infer_sdr_name_from_soapy_info(&soapy_info))
        .unwrap_or_else(|| "unknown".to_string());

    let body = serde_json::to_string(&serde_json::json!({
        "hostname": hostname,
        "bs_uptime_secs": bs_uptime_secs,
        "host_uptime_secs": uptime_secs,
        "uptime_secs": uptime_secs,
        "os": os_info,
        "config_path": config_path,
        "active_config_name": active_config_name,
        "active_config_path": active_config_path,
        "config_dir": config_dir,
        "product_name": product.name,
        "product_version": product.version,
        "product_version_tag": product.version_tag,
        "product_user_agent": product.user_agent,
        "stack_version": tetra_core::STACK_VERSION,
        "cpu_model": cpu.model,
        "cpu_cores": cpu.cores,
        "cpu_pct": cpu_pct,
        "ram_total_mb": ram_total_mb,
        "ram_used_mb": ram_used_mb,
        "cpu_temp_c": cpu_temp_c,
        "soapy_info": soapy_info,
        "sdr_name": sdr_name,
    }))
    .unwrap_or_else(|_| "{}".to_string());

    http_json_response(stream, 200, &body);
}

#[derive(Debug, Clone, Copy)]
struct DashboardProductIdentity {
    name: &'static str,
    version: &'static str,
    version_tag: &'static str,
    user_agent: &'static str,
}

fn dashboard_product_identity() -> DashboardProductIdentity {
    DashboardProductIdentity {
        name: tetra_core::PRODUCT_NAME,
        version: tetra_core::PRODUCT_VERSION,
        version_tag: tetra_core::PRODUCT_VERSION_TAG,
        user_agent: tetra_core::PRODUCT_USER_AGENT,
    }
}

fn infer_sdr_name_from_soapy_info(soapy_info: &str) -> Option<String> {
    let lower = soapy_info.to_lowercase();
    if lower.contains("driver = sx") || lower.contains("label = sx") || lower.contains("soapysx") {
        Some("SXceiver".to_string())
    } else if lower.contains("driver = plutosdr") || lower.contains("adalm-pluto") {
        Some("ADALM-Pluto".to_string())
    } else if lower.contains("driver = hackrf") {
        Some("HackRF".to_string())
    } else if lower.contains("driver = lime") {
        Some("LimeSDR".to_string())
    } else if lower.contains("driver = rtlsdr") {
        Some("RTL-SDR".to_string())
    } else {
        None
    }
}

fn serve_config_list(stream: TcpStream, config_path: &str) {
    let runtime_active_name = active_config_filename(config_path);
    let selected_name = read_active_config_marker(config_path).unwrap_or_else(|| runtime_active_name.clone());
    let config_dir = std::path::Path::new(config_path).parent().unwrap_or(std::path::Path::new("."));

    let mut profiles: Vec<serde_json::Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(config_dir) {
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                // Include .toml files, exclude backups (.bak)
                if name.ends_with(".toml") && !name.ends_with(".bak") {
                    Some(name)
                } else {
                    None
                }
            })
            .collect();
        names.sort();
        for name in names {
            profiles.push(serde_json::json!({
                "name": name,
                "active": name == selected_name,
                "runtime": name == runtime_active_name,
            }));
        }
    }

    let body = serde_json::to_string(&profiles).unwrap_or_else(|_| "[]".to_string());
    http_json_response(stream, 200, &body);
}

fn request_profile_name(req_line: &str, prefix: &str) -> Option<String> {
    request_path(req_line)
        .and_then(|path| path.strip_prefix(prefix).map(str::to_string))
        .and_then(|name| percent_decode_path(&name))
}

fn active_config_filename(config_path: &str) -> String {
    std::path::Path::new(config_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "config.toml".to_string())
}

fn active_config_marker_path(config_path: &str) -> PathBuf {
    PathBuf::from(format!("{config_path}.active"))
}

fn validate_config_profile_name(profile_name: &str) -> Result<String, String> {
    let name = profile_name.trim();
    if name.is_empty() {
        return Err("profile name cannot be empty".to_string());
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err("invalid profile name".to_string());
    }
    if !name.ends_with(".toml") || name.ends_with(".bak") {
        return Err("profile must be a non-backup .toml file".to_string());
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
    {
        return Err("profile name may contain only letters, numbers, '.', '_', '-' and '+'".to_string());
    }
    Ok(name.to_string())
}

fn read_active_config_marker(config_path: &str) -> Option<String> {
    let marker_path = active_config_marker_path(config_path);
    let marker = std::fs::read_to_string(marker_path).ok()?;
    let name = validate_config_profile_name(marker.trim()).ok()?;
    let config_dir = std::path::Path::new(config_path).parent().unwrap_or(std::path::Path::new("."));
    if config_dir.join(&name).is_file() { Some(name) } else { None }
}

fn active_profile_path(config_path: &str) -> PathBuf {
    let active_name = read_active_config_marker(config_path).unwrap_or_else(|| active_config_filename(config_path));
    let config_dir = std::path::Path::new(config_path).parent().unwrap_or(std::path::Path::new("."));
    config_dir.join(active_name)
}

fn active_profile_brew_json(config_path: &str) -> Option<serde_json::Value> {
    let active_path = active_profile_path(config_path);
    match tetra_config::bluestation::parsing::from_file(&active_path) {
        Ok(cfg) => cfg.brew.as_ref().map(|brew| {
            serde_json::json!({
                "configured": true,
                "host": brew.host,
                "port": brew.port,
                "tls": brew.tls,
            })
        }),
        Err(e) => {
            tracing::warn!(
                "Dashboard: failed to read active config '{}' for Brew status: {}",
                active_path.display(),
                e
            );
            None
        }
    }
}

fn write_active_config_marker(config_path: &str, profile_name: &str) -> Result<(), String> {
    let name = validate_config_profile_name(profile_name)?;
    write_dashboard_config_file(&active_config_marker_path(config_path), &format!("{name}\n"))
        .map_err(|e| format!("failed to write active config marker: {}", e))
}

fn write_dashboard_config_file(path: &Path, content: &str) -> std::io::Result<()> {
    fs::write(path, content.as_bytes())?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// Read a specific config profile and serve its content as plain text.
fn serve_config_profile_get(stream: TcpStream, config_path: &str, profile_name: &str) {
    let profile_name = match validate_config_profile_name(profile_name) {
        Ok(name) => name,
        Err(e) => return http_response(stream, 400, &e),
    };
    let config_dir = std::path::Path::new(config_path).parent().unwrap_or(std::path::Path::new("."));
    let profile_path = config_dir.join(profile_name);
    serve_config_get(stream, &profile_path.to_string_lossy());
}

/// Save content to a specific config profile (not the active config).
/// The active config is identified by config_path; writing to it is rejected
/// (use POST /api/config for that).
fn save_config_profile(config_path: &str, profile_name: &str, content: &str) -> Result<(), String> {
    let profile_name = validate_config_profile_name(profile_name)?;
    let config_dir = std::path::Path::new(config_path).parent().unwrap_or(std::path::Path::new("."));
    let profile_path = config_dir.join(&profile_name);

    // Refuse to overwrite the active config through this endpoint
    let active_name = active_config_filename(config_path);
    if profile_name == active_name {
        return Err("cannot overwrite active config via profile editor — use the Config editor tab".to_string());
    }

    write_dashboard_config_file(&profile_path, content).map_err(|e| format!("failed to write profile: {}", e))
}

fn delete_config_profile(config_path: &str, profile_name: &str) -> Result<(), String> {
    let profile_name = validate_config_profile_name(profile_name)?;
    let runtime_name = active_config_filename(config_path);
    let selected_name = read_active_config_marker(config_path).unwrap_or_else(|| runtime_name.clone());

    if profile_name == runtime_name {
        return Err("cannot delete the runtime config".to_string());
    }
    if profile_name == selected_name {
        return Err("cannot delete the active selected profile".to_string());
    }

    let config_dir = std::path::Path::new(config_path).parent().unwrap_or(std::path::Path::new("."));
    let profile_path = config_dir.join(&profile_name);
    if !profile_path.is_file() {
        return Err(format!("profile '{}' not found", profile_name));
    }
    std::fs::remove_file(&profile_path).map_err(|e| format!("failed to delete profile: {}", e))
}

/// Copy selected profile over the active config_path, preserving a backup.
fn activate_config_profile(config_path: &str, profile_name: &str) -> Result<(), String> {
    // Security: profile_name must be a plain filename with no path separators.
    let profile_name = validate_config_profile_name(profile_name)?;

    let config_dir = std::path::Path::new(config_path).parent().unwrap_or(std::path::Path::new("."));
    let profile_path = config_dir.join(&profile_name);

    if !profile_path.exists() {
        return Err(format!("profile '{}' not found", profile_name));
    }

    if profile_name == active_config_filename(config_path) {
        return write_active_config_marker(config_path, &profile_name);
    }

    // Backup current config before switching
    let backup_path = format!("{}.bak", config_path);
    if let Err(e) = std::fs::copy(config_path, &backup_path) {
        tracing::warn!("Dashboard: failed to backup config before profile switch: {}", e);
    }

    std::fs::copy(&profile_path, config_path).map_err(|e| format!("failed to copy profile: {}", e))?;
    write_active_config_marker(config_path, &profile_name)
}

fn serve_html(mut stream: TcpStream) {
    let body = render_product_template(DASHBOARD_HTML);
    let body = body.as_bytes();
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n{}Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        DASHBOARD_SECURITY_HEADERS,
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

enum DashboardAsset {
    File { path: PathBuf, mime: &'static str },
    EmbeddedFallback,
    NotFound,
    BadRequest(&'static str),
}

fn serve_dashboard_asset(stream: TcpStream, static_dir: Option<&str>, req_line: &str) {
    match resolve_dashboard_asset(static_dir, req_line) {
        DashboardAsset::File { path, mime } => serve_static_file(stream, &path, mime),
        DashboardAsset::EmbeddedFallback => serve_html(stream),
        DashboardAsset::NotFound => http_response(stream, 404, "Not found"),
        DashboardAsset::BadRequest(reason) => http_response(stream, 400, reason),
    }
}

fn resolve_dashboard_asset(static_dir: Option<&str>, req_line: &str) -> DashboardAsset {
    let Some(static_dir) = static_dir else {
        return DashboardAsset::EmbeddedFallback;
    };
    let base = Path::new(static_dir);
    if !base.is_dir() {
        tracing::warn!("Dashboard: static_dir '{}' is not usable, serving embedded fallback", static_dir);
        return DashboardAsset::EmbeddedFallback;
    }

    let Some(path) = request_path(req_line) else {
        return DashboardAsset::EmbeddedFallback;
    };
    let Some(decoded) = percent_decode_path(&path) else {
        return DashboardAsset::BadRequest("invalid URL path encoding");
    };
    if decoded.contains('\\') {
        return DashboardAsset::BadRequest("invalid path");
    }

    let safe_relative = match safe_relative_path(&decoded) {
        Some(path) => path,
        None => return DashboardAsset::BadRequest("invalid path"),
    };

    let candidate = if safe_relative.as_os_str().is_empty() {
        base.join("index.html")
    } else {
        base.join(&safe_relative)
    };
    let candidate = if candidate.is_dir() {
        candidate.join("index.html")
    } else {
        candidate
    };

    if candidate.is_file() && is_within_static_dir(base, &candidate) {
        let mime = mime_for_path(&candidate);
        return DashboardAsset::File { path: candidate, mime };
    }

    if decoded_has_extension(&decoded) {
        DashboardAsset::NotFound
    } else {
        let index = base.join("index.html");
        if index.is_file() && is_within_static_dir(base, &index) {
            DashboardAsset::File {
                path: index,
                mime: "text/html; charset=utf-8",
            }
        } else {
            DashboardAsset::EmbeddedFallback
        }
    }
}

fn request_path(req_line: &str) -> Option<String> {
    let target = req_line.split_whitespace().nth(1)?;
    let path = target.split('?').next().unwrap_or(target);
    Some(path.to_string())
}

fn request_query_param(req_line: &str, key: &str) -> Option<String> {
    let target = req_line.split_whitespace().nth(1)?;
    let query = target.split_once('?')?.1;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            return percent_decode_path(value);
        }
    }
    None
}

fn request_method(req_line: &str) -> Option<&str> {
    req_line.split_whitespace().next()
}

fn request_matches(req_line: &str, method: &str, path: &str) -> bool {
    request_method(req_line) == Some(method) && request_path(req_line).as_deref() == Some(path)
}

fn request_path_has_prefix(req_line: &str, method: &str, prefix: &str) -> bool {
    request_method(req_line) == Some(method) && request_path(req_line).is_some_and(|path| path.starts_with(prefix))
}

fn request_is_api(req_line: &str) -> bool {
    request_path(req_line).is_some_and(|path| path == "/api" || path.starts_with("/api/"))
}

fn percent_decode_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] == b'%' {
            if idx + 2 >= bytes.len() {
                return None;
            }
            let hi = hex_value(bytes[idx + 1])?;
            let lo = hex_value(bytes[idx + 2])?;
            out.push((hi << 4) | lo);
            idx += 3;
        } else {
            out.push(bytes[idx]);
            idx += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn safe_relative_path(path: &str) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return Some(relative);
    }
    for component in Path::new(trimmed).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(relative)
}

fn is_within_static_dir(base: &Path, path: &Path) -> bool {
    match (base.canonicalize(), path.canonicalize()) {
        (Ok(base), Ok(path)) => path.starts_with(base),
        _ => false,
    }
}

fn decoded_has_extension(path: &str) -> bool {
    Path::new(path.trim_start_matches('/')).extension().is_some()
}

fn mime_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn serve_static_file(mut stream: TcpStream, path: &Path, mime: &str) {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > DASHBOARD_STATIC_FILE_MAX => {
            http_response(
                stream,
                413,
                &format!("static asset too large (max {DASHBOARD_STATIC_FILE_MAX} bytes)"),
            );
        }
        Ok(meta) => match std::fs::File::open(path) {
            Ok(mut file) => {
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {}\r\n{}Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    mime,
                    DASHBOARD_SECURITY_HEADERS,
                    meta.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = std::io::copy(&mut file, &mut stream);
            }
            Err(e) => http_response(stream, 500, &e.to_string()),
        },
        Err(e) => http_response(stream, 500, &e.to_string()),
    }
}

fn serve_config_get(mut stream: TcpStream, config_path: &str) {
    match std::fs::read_to_string(config_path) {
        Ok(content) => {
            let body = content.as_bytes();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n{}Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                DASHBOARD_SECURITY_HEADERS,
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body);
        }
        Err(e) => http_response(stream, 500, &e.to_string()),
    }
}

fn http_response(mut stream: TcpStream, code: u16, body: &str) {
    let status = if code == 200 { "OK" } else { "Error" };
    let resp = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/plain\r\n{}Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        code,
        status,
        DASHBOARD_SECURITY_HEADERS,
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
}

/// Like `http_response` but serves JSON. Used by the WiFi management endpoints
/// which all return structured `{"ok": ..., ...}` payloads.
fn http_json_response(mut stream: TcpStream, code: u16, body: &str) {
    let status = if code == 200 { "OK" } else { "Error" };
    let resp = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\n{}Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        code,
        status,
        DASHBOARD_SECURITY_HEADERS,
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
}

/// Consume and discard HTTP request headers up to the blank line. Use this
/// for GET-style endpoints that don't read a body — we still need to clear
/// the headers off the stream before responding, otherwise some clients
/// reuse the connection and get confused.
fn drain_http_headers(stream: &mut TcpStream) {
    // We read byte-by-byte to find the \r\n\r\n delimiter. This is slower
    // than BufReader-line reads but doesn't consume bytes past the headers,
    // which matters for POST handlers that need to keep reading the body.
    let mut prev3 = [0u8; 3];
    let mut byte = [0u8; 1];
    let mut read = 0usize;
    loop {
        if stream.read(&mut byte).unwrap_or(0) == 0 {
            break;
        }
        read = read.saturating_add(1);
        if read > DASHBOARD_HTTP_HEADER_MAX {
            break;
        }
        // Detect "\r\n\r\n" by sliding a 4-byte window.
        if prev3 == [b'\r', b'\n', b'\r'] && byte[0] == b'\n' {
            break;
        }
        prev3 = [prev3[1], prev3[2], byte[0]];
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HttpBodyReadError {
    TooLarge { max: usize },
    HeadersTooLarge { max: usize },
    ReadFailed,
}

fn respond_http_body_error(stream: TcpStream, err: HttpBodyReadError) {
    match err {
        HttpBodyReadError::TooLarge { max } => http_response(stream, 413, &format!("request body too large (max {max} bytes)")),
        HttpBodyReadError::HeadersTooLarge { max } => http_response(stream, 431, &format!("request headers too large (max {max} bytes)")),
        HttpBodyReadError::ReadFailed => http_response(stream, 400, "failed to read request body"),
    }
}

/// Read an HTTP request body from a stream, bounded by `max_len`.
fn read_http_body(stream: &mut TcpStream, max_len: usize) -> Result<Vec<u8>, HttpBodyReadError> {
    let mut buf = BufReader::new(stream);
    read_http_body_from_buf(&mut buf, max_len)
}

/// Read headers and body from a buffered HTTP request, bounded by `max_len`.
fn read_http_body_from_buf<R: BufRead>(buf: &mut R, max_len: usize) -> Result<Vec<u8>, HttpBodyReadError> {
    let mut content_length = 0usize;
    let mut header_bytes = 0usize;
    loop {
        let Some(line) = read_http_header_line_bounded(buf)? else {
            break;
        };
        header_bytes = header_bytes.saturating_add(line.len());
        if header_bytes > DASHBOARD_HTTP_HEADER_MAX {
            return Err(HttpBodyReadError::HeadersTooLarge {
                max: DASHBOARD_HTTP_HEADER_MAX,
            });
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        let lower = line.to_lowercase();
        if lower.starts_with("content-length:") {
            content_length = lower
                .trim_start_matches("content-length:")
                .trim()
                .trim_end_matches("\r\n")
                .trim_end_matches('\n')
                .parse()
                .unwrap_or(0);
        }
    }

    if content_length > max_len {
        return Err(HttpBodyReadError::TooLarge { max: max_len });
    }
    if content_length == 0 {
        return Ok(Vec::new());
    }

    let mut body = vec![0u8; content_length];
    buf.read_exact(&mut body).map_err(|_| HttpBodyReadError::ReadFailed)?;
    Ok(body)
}

fn read_http_header_line_bounded<R: BufRead>(buf: &mut R) -> Result<Option<String>, HttpBodyReadError> {
    let mut bytes = Vec::new();
    loop {
        let available = buf.fill_buf().map_err(|_| HttpBodyReadError::ReadFailed)?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            break;
        }

        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|idx| idx + 1)
            .unwrap_or(available.len());
        if bytes.len().saturating_add(take) > DASHBOARD_HTTP_HEADER_LINE_MAX {
            return Err(HttpBodyReadError::HeadersTooLarge {
                max: DASHBOARD_HTTP_HEADER_LINE_MAX,
            });
        }

        bytes.extend_from_slice(&available[..take]);
        buf.consume(take);
        if bytes.ends_with(b"\n") {
            break;
        }
    }

    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

// ── Login UI / session helpers ──────────────────────────────────────────────

/// Parse a login POST body. Accepts both `application/x-www-form-urlencoded`
/// (user=...&password=...) and a minimal JSON shape `{"user":"...","password":"..."}`.
/// This makes the endpoint trivially usable from both an HTML form and fetch().
fn parse_login_body(body: &str) -> (String, String) {
    let trimmed = body.trim();
    // JSON shape: look for "user":"..." and "password":"..." anywhere in the string.
    // We deliberately don't bring in a JSON parser for these two fields.
    if trimmed.starts_with('{') {
        let user = json_field(trimmed, "user").unwrap_or_default();
        let pass = json_field(trimmed, "password").unwrap_or_default();
        return (user, pass);
    }
    // Form-encoded.
    let mut user = String::new();
    let mut pass = String::new();
    for pair in trimmed.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = it.next().unwrap_or("");
        let decoded = url_decode(v);
        match k {
            "user" | "username" => user = decoded,
            "password" | "pass" => pass = decoded,
            _ => {}
        }
    }
    (user, pass)
}

fn json_field(s: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let idx = s.find(&needle)?;
    let after = &s[idx + needle.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

fn http_redirect(mut stream: TcpStream, location: &str) {
    let resp = format!(
        "HTTP/1.1 302 Found\r\nLocation: {}\r\n{}Cache-Control: no-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        location, DASHBOARD_SECURITY_HEADERS
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn serve_login_success(mut stream: TcpStream, token: &str) {
    // Two cookies:
    //   fs_session: HttpOnly — the actual session token, inaccessible to JS.
    //   fs_auth: readable — a marker telling the dashboard JS "auth is on",
    //                       so it can decide to show the Logout button.
    // The marker carries no security value; the HttpOnly session is what's checked.
    let body = "{\"ok\":true}";
    let resp = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/json\r\n\
         {}\
         Cache-Control: no-store\r\n\
         Content-Length: {}\r\n\
         Set-Cookie: fs_session={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800\r\n\
         Set-Cookie: fs_auth=1; Path=/; SameSite=Lax; Max-Age=604800\r\n\
         Connection: close\r\n\r\n{}",
        DASHBOARD_SECURITY_HEADERS,
        body.len(),
        token,
        body
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn serve_logout(mut stream: TcpStream) {
    // Expire both cookies immediately; client navigates to /login next.
    let resp = "HTTP/1.1 302 Found\r\n\
                Location: /login\r\n\
                X-Content-Type-Options: nosniff\r\n\
                X-Frame-Options: DENY\r\n\
                Referrer-Policy: no-referrer\r\n\
                Content-Security-Policy: frame-ancestors 'none'; object-src 'none'; base-uri 'none'\r\n\
                Cache-Control: no-store\r\n\
                Set-Cookie: fs_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0\r\n\
                Set-Cookie: fs_auth=; Path=/; SameSite=Lax; Max-Age=0\r\n\
                Content-Length: 0\r\n\
                Connection: close\r\n\r\n";
    let _ = stream.write_all(resp.as_bytes());
}

fn serve_login_page(mut stream: TcpStream) {
    let body = render_product_template(crate::net_dashboard::html::LOGIN_HTML);
    let header = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         {}\
         Cache-Control: no-store\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        DASHBOARD_SECURITY_HEADERS,
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn render_product_template(template: &str) -> String {
    template
        .replace("{{STACK_VERSION}}", tetra_core::STACK_VERSION)
        .replace("{{PRODUCT_VERSION}}", tetra_core::PRODUCT_VERSION)
        .replace("{{PRODUCT_VERSION_TAG}}", tetra_core::PRODUCT_VERSION_TAG)
        .replace("{{PRODUCT_USER_AGENT}}", tetra_core::PRODUCT_USER_AGENT)
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::fs;
    use std::io::Cursor;
    use std::path::PathBuf;

    use crate::net_control::commands::WAP_WDP_PROTOCOL_ID;

    use super::*;

    fn temp_dashboard_static_dir() -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nexus-bs-dashboard-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("assets")).expect("create test dashboard dir");
        fs::write(dir.join("index.html"), "<html>Nexus-BS dashboard</html>").expect("write index");
        fs::write(dir.join("assets/app.js"), "console.log('nexus-bs');").expect("write js");
        dir
    }

    fn temp_config_dir(label: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nexus-bs-config-profile-test-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create temp config dir");
        dir
    }

    #[test]
    fn product_template_renders_current_nexus_bs_identity() {
        let rendered = render_product_template(
            "<title>Nexus-BS {{PRODUCT_VERSION_TAG}}</title>{{PRODUCT_VERSION_TAG}} {{PRODUCT_USER_AGENT}} {{STACK_VERSION}}",
        );

        assert!(!rendered.contains("{{"));
        assert!(rendered.contains(&format!("Nexus-BS {}", tetra_core::PRODUCT_VERSION_TAG)));
        let stale_dotted_tag = ["v", ".", tetra_core::PRODUCT_VERSION].concat();
        assert!(!rendered.contains(&stale_dotted_tag));
        assert!(rendered.contains(tetra_core::PRODUCT_USER_AGENT));
        assert!(rendered.contains(tetra_core::STACK_VERSION));
    }

    #[test]
    fn dashboard_static_dir_serves_index_and_assets() {
        let dir = temp_dashboard_static_dir();
        let dir_str = dir.to_string_lossy().to_string();

        match resolve_dashboard_asset(Some(&dir_str), "GET / HTTP/1.1") {
            DashboardAsset::File { path, mime } => {
                assert_eq!(path, dir.join("index.html"));
                assert_eq!(mime, "text/html; charset=utf-8");
            }
            _ => panic!("expected dashboard index from static_dir"),
        }

        match resolve_dashboard_asset(Some(&dir_str), "GET /assets/app.js HTTP/1.1") {
            DashboardAsset::File { path, mime } => {
                assert_eq!(path, dir.join("assets/app.js"));
                assert_eq!(mime, "text/javascript; charset=utf-8");
            }
            _ => panic!("expected JS asset from static_dir"),
        }

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dashboard_static_dir_falls_back_to_index_for_spa_routes() {
        let dir = temp_dashboard_static_dir();
        let dir_str = dir.to_string_lossy().to_string();

        match resolve_dashboard_asset(Some(&dir_str), "GET /radios/2260618 HTTP/1.1") {
            DashboardAsset::File { path, mime } => {
                assert_eq!(path, dir.join("index.html"));
                assert_eq!(mime, "text/html; charset=utf-8");
            }
            _ => panic!("expected SPA route fallback to index.html"),
        }

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dashboard_static_dir_rejects_path_traversal() {
        let dir = temp_dashboard_static_dir();
        let dir_str = dir.to_string_lossy().to_string();

        for req in [
            "GET /assets/../config.toml HTTP/1.1",
            "GET /assets/%2e%2e/config.toml HTTP/1.1",
            "GET /assets/%2E%2E/config.toml HTTP/1.1",
        ] {
            match resolve_dashboard_asset(Some(&dir_str), req) {
                DashboardAsset::BadRequest(_) => {}
                _ => panic!("expected traversal to be rejected for {req}"),
            }
        }

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dashboard_http_body_reader_rejects_oversized_content_length() {
        let request = b"POST /api/config HTTP/1.1\r\nContent-Length: 10\r\n\r\nabcdefghij";
        let mut reader = BufReader::new(Cursor::new(request.as_slice()));

        let err = read_http_body_from_buf(&mut reader, 4).expect_err("oversized body must fail");

        assert_eq!(err, HttpBodyReadError::TooLarge { max: 4 });
    }

    #[test]
    fn dashboard_http_body_reader_rejects_oversized_header_line() {
        let long_header = "X-Test: ".to_string() + &"a".repeat(DASHBOARD_HTTP_HEADER_LINE_MAX + 1);
        let request = format!("POST /api/config HTTP/1.1\r\n{long_header}\r\nContent-Length: 0\r\n\r\n");
        let mut reader = BufReader::new(Cursor::new(request.as_bytes()));

        let err = read_http_body_from_buf(&mut reader, DASHBOARD_SMALL_BODY_MAX).expect_err("oversized header must fail");

        assert_eq!(
            err,
            HttpBodyReadError::HeadersTooLarge {
                max: DASHBOARD_HTTP_HEADER_LINE_MAX
            }
        );
    }

    #[test]
    fn dashboard_unknown_api_paths_are_reserved_from_spa_fallback() {
        assert!(request_is_api("GET /api/unknown HTTP/1.1"));
        assert!(request_is_api("POST /api/configure HTTP/1.1"));
        assert!(!request_matches("GET /api/system2 HTTP/1.1", "GET", "/api/system"));
        assert!(request_matches("GET /api/system?fresh=1 HTTP/1.1", "GET", "/api/system"));
        assert!(request_matches("GET /api/snapshot HTTP/1.1", "GET", "/api/snapshot"));
        assert!(request_matches("GET /api/calls HTTP/1.1", "GET", "/api/calls"));
        assert!(request_matches("GET /api/site HTTP/1.1", "GET", "/api/site"));
        assert!(request_matches(
            "GET /api/easy-start/status HTTP/1.1",
            "GET",
            "/api/easy-start/status"
        ));
        assert!(request_matches(
            "POST /api/easy-start/preview HTTP/1.1",
            "POST",
            "/api/easy-start/preview"
        ));
        assert!(request_matches(
            "POST /api/easy-start/commit HTTP/1.1",
            "POST",
            "/api/easy-start/commit"
        ));
        assert!(request_matches(
            "POST /api/easy-start/skip HTTP/1.1",
            "POST",
            "/api/easy-start/skip"
        ));
        assert!(request_matches("POST /api/factory-reset HTTP/1.1", "POST", "/api/factory-reset"));
        assert!(request_matches(
            "POST /api/service/restart HTTP/1.1",
            "POST",
            "/api/service/restart"
        ));
        assert!(request_matches(
            "POST /api/service/stop-go HTTP/1.1",
            "POST",
            "/api/service/stop-go"
        ));
        assert!(request_matches("POST /api/rf/carrier HTTP/1.1", "POST", "/api/rf/carrier"));
        assert!(request_matches("GET /api/wifi HTTP/1.1", "GET", "/api/wifi"));
        assert!(request_matches("POST /api/wifi/scan HTTP/1.1", "POST", "/api/wifi/scan"));
        assert!(request_matches("POST /api/wifi/connect HTTP/1.1", "POST", "/api/wifi/connect"));
        assert!(request_matches("POST /api/logs/clear HTTP/1.1", "POST", "/api/logs/clear"));
        assert!(request_path_has_prefix(
            "GET /api/configs/profile.toml HTTP/1.1",
            "GET",
            "/api/configs/"
        ));
        assert!(request_path_has_prefix(
            "DELETE /api/configs/config%2B1.toml HTTP/1.1",
            "DELETE",
            "/api/configs/"
        ));
        assert!(!request_is_api("GET /assets/app.js HTTP/1.1"));
        assert!(!request_is_api("GET /radios/2260618 HTTP/1.1"));
    }

    #[test]
    fn easy_start_calculates_cell_info_for_user_example() {
        let req = EasyStartRequest {
            mcc: 901,
            mnc: 9999,
            timezone: "Europe/Bucharest".to_string(),
            tx_freq: 438_025_000,
            duplex_spacing: 1,
            custom_spacing_enabled: false,
            custom_spacing_hz: None,
            brew_enabled: true,
            brew_host: "core.tetrapack.online".to_string(),
            brew_port: 443,
            brew_tls: true,
            brew_username: 123_456_789,
            brew_password: "".to_string(),
        };
        let (toml, plan) = render_easy_start_config(&req).expect("easy start config should validate");
        assert_eq!(plan.freq_band, 4);
        assert_eq!(plan.main_carrier, 1521);
        assert_eq!(plan.freq_offset, 0);
        assert_eq!(plan.rx_freq, 431_025_000);
        assert!(toml.contains("tx_freq = 438025000"));
        assert!(toml.contains("rx_freq = 431025000"));
        assert!(toml.contains("main_carrier = 1521"));
        assert!(toml.contains("freq_offset = 0"));
    }

    #[test]
    fn dashboard_site_config_cache_reloads_when_active_config_changes() {
        let dir = temp_config_dir("site-config-cache");
        let config_path = dir.join("config.toml");
        let mut req = EasyStartRequest {
            mcc: 901,
            mnc: 9999,
            timezone: "Europe/Bucharest".to_string(),
            tx_freq: 438_362_500,
            duplex_spacing: 1,
            custom_spacing_enabled: false,
            custom_spacing_hz: None,
            brew_enabled: false,
            brew_host: "core.tetrapack.online".to_string(),
            brew_port: 443,
            brew_tls: true,
            brew_username: 123_456_789,
            brew_password: "".to_string(),
        };
        let (first, _) = render_easy_start_config(&req).expect("first config");
        fs::write(&config_path, first).expect("write first config");
        let dashboard = DashboardServer::new(config_path.to_string_lossy().to_string());

        let first_site = dashboard_site_json(&dashboard.state, &None, config_path.to_string_lossy().as_ref());
        assert!(first_site.contains("\"main_carrier\":1534"));
        assert!(first_site.contains("\"tx_hz\":438362500.0"));

        req.tx_freq = 438_625_000;
        let (second, _) = render_easy_start_config(&req).expect("second config");
        std::thread::sleep(std::time::Duration::from_millis(2));
        fs::write(&config_path, second).expect("write second config");

        let second_site = dashboard_site_json(&dashboard.state, &None, config_path.to_string_lossy().as_ref());
        assert!(second_site.contains("\"main_carrier\":1545"));
        assert!(second_site.contains("\"tx_hz\":438625000.0"));
        assert!(!second_site.contains("\"main_carrier\":1534"));
    }

    #[test]
    fn dashboard_rf_carrier_parser_requires_boolean_inhibit_field() {
        assert_eq!(
            parse_rf_carrier_inhibited(&json!({ "inhibited": true })).expect("true is valid"),
            true
        );
        assert_eq!(
            parse_rf_carrier_inhibited(&json!({ "inhibited": false })).expect("false is valid"),
            false
        );
        assert!(parse_rf_carrier_inhibited(&json!({})).is_err());
        assert!(parse_rf_carrier_inhibited(&json!({ "inhibited": "true" })).is_err());
    }

    #[test]
    fn nmcli_terse_parser_handles_escaped_colons() {
        assert_eq!(
            split_nmcli_terse_line(r"yes:Shop\:Office:87:WPA2:wlan0"),
            vec!["yes", "Shop:Office", "87", "WPA2", "wlan0"]
        );
        assert_eq!(
            split_nmcli_terse_line(r"no:Back\\slash:40::wlan1"),
            vec!["no", "Back\\slash", "40", "", "wlan1"]
        );
    }

    #[test]
    fn dashboard_config_profile_name_is_decoded_and_sanitized() {
        assert_eq!(
            request_profile_name("GET /api/configs/config%2B1.toml HTTP/1.1", "/api/configs/").as_deref(),
            Some("config+1.toml")
        );
        assert!(validate_config_profile_name("config+1.toml").is_ok());
        assert!(validate_config_profile_name("../config.toml").is_err());
        assert!(validate_config_profile_name("config.toml.bak").is_err());
        assert!(validate_config_profile_name("bad name.toml").is_err());
    }

    #[test]
    fn dashboard_config_profile_save_creates_private_duplicate_file() {
        let dir = temp_config_dir("save-profile");
        let config_path = dir.join("config.toml");
        let duplicate_path = dir.join("config+1.toml");
        fs::write(&config_path, "name = \"base\"\n").expect("write base config");

        save_config_profile(config_path.to_string_lossy().as_ref(), "config+1.toml", "name = \"duplicate\"\n")
            .expect("save duplicate profile");

        assert_eq!(
            fs::read_to_string(&duplicate_path).expect("read duplicate profile"),
            "name = \"duplicate\"\n"
        );
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&duplicate_path)
                .expect("duplicate profile metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dashboard_config_profile_activation_copies_and_persists_selection_marker() {
        let dir = temp_config_dir("activate");
        let config_path = dir.join("config.toml");
        let next_path = dir.join("config+1.toml");
        fs::write(&config_path, "name = \"base\"\n").expect("write base config");
        fs::write(&next_path, "name = \"next\"\n").expect("write next config");

        activate_config_profile(config_path.to_string_lossy().as_ref(), "config+1.toml").expect("activate profile");

        assert_eq!(fs::read_to_string(&config_path).expect("read active config"), "name = \"next\"\n");
        assert_eq!(
            read_active_config_marker(config_path.to_string_lossy().as_ref()).as_deref(),
            Some("config+1.toml")
        );
        assert_eq!(
            fs::read_to_string(active_config_marker_path(config_path.to_string_lossy().as_ref()))
                .expect("read active marker")
                .trim(),
            "config+1.toml"
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dashboard_config_profile_delete_refuses_active_and_removes_inactive_profiles() {
        let dir = temp_config_dir("delete");
        let config_path = dir.join("config.toml");
        let active_path = dir.join("config+1.toml");
        let stale_path = dir.join("config+2.toml");
        fs::write(&config_path, "name = \"runtime\"\n").expect("write runtime config");
        fs::write(&active_path, "name = \"active\"\n").expect("write active profile");
        fs::write(&stale_path, "name = \"stale\"\n").expect("write stale profile");
        write_active_config_marker(config_path.to_string_lossy().as_ref(), "config+1.toml").expect("write marker");

        assert!(delete_config_profile(config_path.to_string_lossy().as_ref(), "config.toml").is_err());
        assert!(delete_config_profile(config_path.to_string_lossy().as_ref(), "config+1.toml").is_err());
        delete_config_profile(config_path.to_string_lossy().as_ref(), "config+2.toml").expect("delete inactive profile");

        assert!(!stale_path.exists());
        assert!(active_path.exists());
        assert!(config_path.exists());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dashboard_http_body_reader_reads_bounded_body() {
        let request = b"POST /api/config HTTP/1.1\r\nContent-Length: 5\r\n\r\nabcde";
        let mut reader = BufReader::new(Cursor::new(request.as_slice()));

        let body = read_http_body_from_buf(&mut reader, 5).expect("body should fit limit");

        assert_eq!(body, b"abcde");
    }

    #[test]
    fn dashboard_connection_guard_decrements_active_count() {
        let active = Arc::new(AtomicUsize::new(1));
        {
            let _guard = DashboardConnectionGuard::new(Arc::clone(&active));
            assert_eq!(active.load(Ordering::Acquire), 1);
        }
        assert_eq!(active.load(Ordering::Acquire), 0);
    }

    fn auth_status_value(auth: Option<(String, String)>, sessions: &SharedSessionStore, headers: &str) -> serde_json::Value {
        serde_json::from_str(&dashboard_auth_status_json(&auth, sessions, headers)).expect("auth status json should parse")
    }

    #[test]
    fn dashboard_auth_status_reports_open_dashboard_as_valid() {
        let sessions = Arc::new(Mutex::new(SessionStore::new()));

        let status = auth_status_value(None, &sessions, "");

        assert_eq!(status["auth_required"], json!(false));
        assert_eq!(status["session_valid"], json!(true));
        assert_eq!(status["session_cookie"], json!("fs_session"));
    }

    #[test]
    fn dashboard_auth_status_requires_session_when_credentials_are_set() {
        let sessions = Arc::new(Mutex::new(SessionStore::new()));
        let auth = Some(("admin".to_string(), "change-this".to_string()));

        let status = auth_status_value(auth, &sessions, "GET / HTTP/1.1\r\n\r\n");

        assert_eq!(status["auth_required"], json!(true));
        assert_eq!(status["session_valid"], json!(false));
    }

    #[test]
    fn dashboard_auth_status_accepts_valid_session_cookie() {
        let sessions = Arc::new(Mutex::new(SessionStore::new()));
        let token = sessions.lock().unwrap().create();
        let auth = Some(("admin".to_string(), "change-this".to_string()));
        let headers = format!("GET / HTTP/1.1\r\nCookie: theme=dark; fs_session={token}; fs_auth=1\r\n\r\n");

        let status = auth_status_value(auth, &sessions, &headers);

        assert_eq!(status["auth_required"], json!(true));
        assert_eq!(status["session_valid"], json!(true));
    }

    #[test]
    fn dashboard_session_cookie_parser_handles_cookie_lists() {
        let headers = "GET / HTTP/1.1\r\nCookie: theme=dark; fs_session=ABC123; fs_auth=1\r\n\r\n";

        assert_eq!(parse_session_cookie(headers).as_deref(), Some("ABC123"));
    }

    #[test]
    fn dashboard_session_store_create_validate_and_invalidate() {
        let mut sessions = SessionStore::new();
        let token = sessions.create();

        assert!(sessions.validate(&token));
        sessions.invalidate(&token);
        assert!(!sessions.validate(&token));
    }

    #[test]
    fn dashboard_security_headers_are_present() {
        assert!(DASHBOARD_SECURITY_HEADERS.contains("X-Content-Type-Options: nosniff"));
        assert!(DASHBOARD_SECURITY_HEADERS.contains("X-Frame-Options: DENY"));
        assert!(DASHBOARD_SECURITY_HEADERS.contains("frame-ancestors 'none'"));
    }

    #[test]
    fn shipped_dashboard_pages_render_current_version_identity() {
        let dashboard = render_product_template(crate::net_dashboard::html::DASHBOARD_HTML);
        let login = render_product_template(crate::net_dashboard::html::LOGIN_HTML);
        let stale_project_line = format!("Nexus-BS Project - version {} by Chris YO3TCO", tetra_core::PRODUCT_VERSION_TAG);
        let stale_dotted_tag = ["v", ".", tetra_core::PRODUCT_VERSION].concat();

        for rendered in [&dashboard, &login] {
            assert!(!rendered.contains("{{"));
            assert!(rendered.contains(tetra_core::PRODUCT_VERSION_TAG));
            assert!(!rendered.contains(&stale_dotted_tag));
            assert!(!rendered.contains("0.1.54"));
            assert!(!rendered.contains("0.2.7"));
            assert!(!rendered.contains("Razvan"));
            assert!(!rendered.contains("YO6RVZ"));
        }

        assert!(dashboard.contains("Nexus-BS Project"));
        assert!(dashboard.contains(tetra_core::PRODUCT_USER_AGENT));
        assert!(login.contains("Nexus-BS Project"));
        assert!(!dashboard.contains(&stale_project_line));
        assert!(!login.contains(&stale_project_line));
    }

    #[test]
    fn dashboard_locks_update_disabled_and_about_credits() {
        let dashboard = render_product_template(crate::net_dashboard::html::DASHBOARD_HTML);

        assert!(!DASHBOARD_OTA_UPDATE_ENABLED);
        assert!(dashboard.contains("const OTA_UPDATE_ENABLED=false;"));
        assert!(dashboard.contains(r#"id="update-btn""#));
        assert!(dashboard.contains(r#"class="btn disabled""#));
        assert!(dashboard.contains(r#"disabled title="OTA update disabled for now""#));

        for credit in [
            "BlueStation Project",
            "Tatu Peltola",
            "his",
            "Mihajlo YU4MSH",
            "FDX P2P",
            "FlowStation Project",
            "SXCEIVER",
            "Dennis DB2OE",
            "All Contributors",
        ] {
            assert!(dashboard.contains(credit), "missing About credit {credit}");
        }
        for link in [
            "https://github.com/invictus737/nexus-bs",
            "https://github.com/misadeks/tetra-bluestation",
            "https://github.com/MidnightBlueLabs/tetra-bluestation",
            "https://github.com/razvanzeces/flowstation",
            "https://sxceiver.com/",
        ] {
            assert!(dashboard.contains(link), "missing About link {link}");
        }
        assert!(!dashboard.contains("(https://sxceiver.com/)"));
        assert!(dashboard.contains("current Nexus-BS project governance"));
    }

    #[test]
    fn dashboard_exposes_wap_action_for_registered_radios() {
        let dashboard = render_product_template(crate::net_dashboard::html::DASHBOARD_HTML);

        assert!(dashboard.contains(r#"onclick="sendWap(${m.issi})""#));
        assert!(dashboard.contains("function sendWap(issi){wsSend({type:'wap',dest_issi:issi});}"));
        assert!(dashboard.contains("wap:'SDS WAP'"));
    }

    #[test]
    fn dashboard_product_identity_tracks_workspace_version() {
        let product = dashboard_product_identity();

        assert_eq!(product.name, "Nexus-BS");
        assert_eq!(product.version, tetra_core::PRODUCT_VERSION);
        assert_eq!(product.version_tag, tetra_core::PRODUCT_VERSION_TAG);
        assert_eq!(product.user_agent, tetra_core::PRODUCT_USER_AGENT);
    }

    #[test]
    fn dashboard_preserves_group_if_group_event_precedes_registration() {
        let dashboard = DashboardServer::new("test.toml".to_string());
        let issi = 2260618;
        let gssi = 226333;

        // Dashboard telemetry is observability, not an air-interface rule. The
        // field bug after restart was that MM/CMCE had already rebuilt the GSSI
        // affiliation, but the UI could still show "No Group" if the group
        // event raced the registration event.
        dashboard.handle_telemetry(TelemetryEvent::MsGroupAttach { issi, gssis: vec![gssi] });
        dashboard.handle_telemetry(TelemetryEvent::MsRegistration { issi });

        let snapshot = dashboard.state.read().unwrap().snapshot_ms();
        let ms = snapshot.iter().find(|ms| ms.issi == issi).expect("radio should be visible");
        assert_eq!(ms.groups, vec![gssi]);
    }

    #[test]
    fn dashboard_preserves_group_snapshot_if_snapshot_precedes_registration() {
        let dashboard = DashboardServer::new("test.toml".to_string());
        let issi = 2260616;
        let gssi = 226333;

        dashboard.handle_telemetry(TelemetryEvent::MsGroupsSnapshot { issi, gssis: vec![gssi] });
        dashboard.handle_telemetry(TelemetryEvent::MsRegistration { issi });

        let snapshot = dashboard.state.read().unwrap().snapshot_ms();
        let ms = snapshot.iter().find(|ms| ms.issi == issi).expect("radio should be visible");
        assert_eq!(ms.groups, vec![gssi]);
    }

    #[test]
    fn dashboard_reconstructs_radio_from_activity_without_registration_event() {
        let dashboard = DashboardServer::new("test.toml".to_string());
        let issi = 2260618;
        let gssi = 91;

        dashboard.handle_telemetry(TelemetryEvent::MsRssi { issi, rssi_dbfs: -47.5 });
        dashboard.handle_telemetry(TelemetryEvent::GroupCallStarted {
            call_id: 7,
            gssi,
            caller_issi: issi,
            ts: 2,
        });

        let snapshot = dashboard.state.read().unwrap().snapshot_ms();
        let ms = snapshot
            .iter()
            .find(|ms| ms.issi == issi)
            .expect("radio should be visible from activity");
        assert_eq!(ms.rssi_dbfs, Some(-47.5));
        assert_eq!(ms.groups, vec![gssi]);
    }

    #[test]
    fn dashboard_call_activity_does_not_create_external_subscriber_rows() {
        let dashboard = DashboardServer::new("test.toml".to_string());

        dashboard.handle_telemetry(TelemetryEvent::GroupCallStarted {
            call_id: 7,
            gssi: 91,
            caller_issi: 2320567,
            ts: 2,
        });
        dashboard.handle_telemetry(TelemetryEvent::GroupCallSpeakerChanged {
            call_id: 7,
            gssi: 91,
            speaker_issi: 3110797,
        });
        dashboard.handle_telemetry(TelemetryEvent::IndividualCallStarted {
            call_id: 8,
            calling_issi: 2260618,
            called_issi: 99999,
            simplex: true,
            ts: 2,
            secondary_ts: None,
        });

        let state = dashboard.state.read().unwrap();
        assert!(
            state.ms_map.is_empty(),
            "Subscriber Registry must show only RF/MM known local MS entries, not external Brew callers or services"
        );
        assert_eq!(state.last_heard.len(), 3);
        assert_eq!(state.calls.len(), 2);
    }

    #[test]
    fn dashboard_preserves_energy_saving_if_energy_event_precedes_registration() {
        let dashboard = DashboardServer::new("test.toml".to_string());
        let issi = 2260618;

        dashboard.handle_telemetry(TelemetryEvent::MsEnergySaving {
            issi,
            mode: 7,
            frame: Some(11),
            multiframe: Some(42),
        });
        dashboard.handle_telemetry(TelemetryEvent::MsRegistration { issi });

        let snapshot = dashboard.state.read().unwrap().snapshot_ms();
        let ms = snapshot.iter().find(|ms| ms.issi == issi).expect("radio should be visible");
        assert_eq!(ms.energy_saving_mode, 7);
        assert_eq!(ms.energy_saving_frame, Some(11));
        assert_eq!(ms.energy_saving_multiframe, Some(42));
    }

    #[test]
    fn dashboard_caches_and_serializes_health_snapshot() {
        let dashboard = DashboardServer::new("test.toml".to_string());
        let snapshot = crate::health::registry().snapshot();

        dashboard.handle_telemetry(TelemetryEvent::HealthSnapshot(snapshot.clone()));

        let cached = dashboard
            .state
            .read()
            .unwrap()
            .last_health
            .clone()
            .expect("dashboard should cache last health snapshot");
        assert_eq!(cached.unix_ms, snapshot.unix_ms);

        let snapshot_json = dashboard_snapshot_json(&dashboard.state);
        assert!(snapshot_json.contains("\"last_health\""));
        assert!(snapshot_json.contains("\"overall\""));

        let site_json = dashboard_site_json(&dashboard.state, &None, "test.toml");
        assert!(site_json.contains("\"health\""));
        assert!(site_json.contains("\"rf_cached\""));
    }

    #[test]
    fn dashboard_runtime_snapshot_reset_clears_stale_core_records() {
        let dashboard = DashboardServer::new("test.toml".to_string());
        dashboard.handle_telemetry(TelemetryEvent::MsRegistration { issi: 2260618 });
        dashboard.handle_telemetry(TelemetryEvent::MsGroupsSnapshot {
            issi: 2260618,
            gssis: vec![91],
        });
        dashboard.handle_telemetry(TelemetryEvent::GroupCallStarted {
            call_id: 7,
            gssi: 91,
            caller_issi: 2260618,
            ts: 2,
        });
        dashboard.handle_telemetry(TelemetryEvent::BrewConnected {
            connected: true,
            server_version: 1,
        });
        dashboard.handle_telemetry(TelemetryEvent::HealthSnapshot(crate::health::registry().snapshot()));

        dashboard.reset_runtime_snapshot("test restart");

        let state = dashboard.state.read().unwrap();
        assert!(state.ms_map.is_empty());
        assert!(state.calls.is_empty());
        assert!(state.last_heard.is_empty());
        assert!(!state.brew_online);
        assert_eq!(state.brew_version, 0);
        assert!(state.last_health.is_none());
        drop(state);

        let snapshot_json = dashboard_snapshot_json(&dashboard.state);
        assert!(snapshot_json.contains("\"ms\":[]"));
        assert!(snapshot_json.contains("\"calls\":[]"));
        assert!(snapshot_json.contains("\"last_heard\":[]"));
        assert!(snapshot_json.contains("Dashboard runtime snapshot reset"));
    }

    #[test]
    fn dashboard_browser_creates_ms_entry_for_group_events_before_registration() {
        let dashboard = render_product_template(crate::net_dashboard::html::DASHBOARD_HTML);

        assert!(dashboard.contains("function ensureMsEntry(issi)"));
        assert!(dashboard.contains("if((msg.groups||[]).length){const e=ensureMsEntry(msg.issi);"));
        assert!(dashboard.contains("if(state.ms[msg.issi]||(msg.groups||[]).length)ensureMsEntry(msg.issi).groups=msg.groups||[];"));
        assert!(dashboard.contains("ensureMsEntry(msg.issi)._last_seen_ts=Date.now();"));
    }

    #[test]
    fn dashboard_browser_creates_ms_entry_for_energy_saving_before_registration() {
        let dashboard = render_product_template(crate::net_dashboard::html::DASHBOARD_HTML);

        assert!(dashboard.contains("case 'ms_energy_saving':"));
        assert!(dashboard.contains("const e=ensureMsEntry(msg.issi);e.energy_saving_mode=msg.mode;"));
        assert!(dashboard.contains("e.energy_saving_frame=msg.frame??null;"));
        assert!(dashboard.contains("e.energy_saving_multiframe=msg.multiframe??null;"));
        assert!(dashboard.contains("e._last_seen_ts=Date.now();"));
    }

    #[test]
    fn dashboard_broadcast_drops_slow_ws_client_at_bounded_queue_cap() {
        let dashboard = DashboardServer::new("test.toml".to_string());
        let (tx, _rx) = crossbeam_channel::bounded::<String>(DASHBOARD_WS_BROADCAST_QUEUE_MAX);
        dashboard.clients.lock().unwrap().push(tx);

        for idx in 0..DASHBOARD_WS_BROADCAST_QUEUE_MAX {
            dashboard.broadcast(&format!("msg-{idx}"));
        }
        assert_eq!(dashboard.clients.lock().unwrap().len(), 1);

        dashboard.broadcast("overflow");
        assert!(
            dashboard.clients.lock().unwrap().is_empty(),
            "slow websocket client should be dropped instead of growing an unbounded broadcast queue"
        );
    }

    #[test]
    fn dashboard_browser_coalesces_station_event_rendering() {
        let dashboard = render_product_template(crate::net_dashboard::html::DASHBOARD_HTML);

        assert!(dashboard.contains("let stationRenderQueued=false;"));
        assert!(dashboard.contains("function scheduleRenderStations()"));
        assert!(dashboard.contains("window.requestAnimationFrame||((fn)=>setTimeout(fn,16))"));
        assert!(dashboard.contains("schedule(()=>{stationRenderQueued=false;renderStations();});"));
        assert!(dashboard.contains("ensureMsEntry(msg.issi)._last_seen_ts=Date.now();\n      scheduleRenderStations();break;"));
        assert!(dashboard.contains("e._last_seen_ts=Date.now();}\n      scheduleRenderStations();break;"));
    }

    #[test]
    fn cpu_descriptor_detects_raspberry_pi_zero_2_w_cortex_a53() {
        let cpuinfo = r#"processor	: 0
BogoMIPS	: 38.40
Features	: fp asimd evtstrm crc32 cpuid
CPU implementer	: 0x41
CPU architecture: 8
CPU variant	: 0x0
CPU part	: 0xd03
CPU revision	: 4

processor	: 1
BogoMIPS	: 38.40
Features	: fp asimd evtstrm crc32 cpuid
CPU implementer	: 0x41
CPU architecture: 8
CPU variant	: 0x0
CPU part	: 0xd03
CPU revision	: 4

processor	: 2
BogoMIPS	: 38.40
Features	: fp asimd evtstrm crc32 cpuid
CPU implementer	: 0x41
CPU architecture: 8
CPU variant	: 0x0
CPU part	: 0xd03
CPU revision	: 4

processor	: 3
BogoMIPS	: 38.40
Features	: fp asimd evtstrm crc32 cpuid
CPU implementer	: 0x41
CPU architecture: 8
CPU variant	: 0x0
CPU part	: 0xd03
CPU revision	: 4

Revision	: 902120
Serial		: 000000004d3bc489
Model		: Raspberry Pi Zero 2 W Rev 1.0
"#;

        let cpu = build_cpu_descriptor(
            cpuinfo,
            Some("Raspberry Pi Zero 2 W Rev 1.0"),
            Some("raspberrypi,model-zero-2-w\nbrcm,bcm2837"),
            Some(1_000_000),
            Some("aarch64"),
        );

        assert_eq!(
            cpu,
            CpuDescriptor {
                model: "Broadcom Cortex-A53 1GHz 64-bit".to_string(),
                cores: 4,
            }
        );
    }

    #[test]
    fn cpu_descriptor_preserves_x86_model_name_and_core_count() {
        let cpuinfo = r#"processor	: 0
vendor_id	: GenuineIntel
model name	: Intel(R) Core(TM) i7-8650U CPU @ 1.90GHz

processor	: 1
vendor_id	: GenuineIntel
model name	: Intel(R) Core(TM) i7-8650U CPU @ 1.90GHz
"#;

        let cpu = build_cpu_descriptor(cpuinfo, None, None, None, Some("x86_64"));

        assert_eq!(
            cpu,
            CpuDescriptor {
                model: "Intel(R) Core(TM) i7-8650U CPU @ 1.90GHz 64-bit".to_string(),
                cores: 2,
            }
        );
    }

    #[test]
    fn live_sds_post_defaults_to_etsi_text_pid_and_all_ones_source() {
        let post = parse_live_sds_post(&json!({ "text": "hello" })).expect("valid live SDS post");

        assert_eq!(
            post,
            LiveSdsPost {
                text: "hello".to_string(),
                protocol_id: DEFAULT_SDS_TEXT_PROTOCOL_ID,
                source_issi: 0x00FF_FFFF,
                repeat_count: 0,
            }
        );
    }

    #[test]
    fn calibration_config_upsert_enables_dc_but_keeps_iq_opt_in() {
        let input = r#"[phy_io.soapysdr]
rx_freq = 431362500.0
tx_freq = 438362500.0
tx_calibration_apply_iq = true

[cell]
main_carrier = 1
"#;

        let updated = upsert_soapy_calibration_keys(input).expect("upsert calibration keys");

        assert!(updated.contains("tx_calibration_enabled = true"));
        assert!(updated.contains("tx_calibration_file = \"calibration.toml\""));
        assert!(updated.contains("tx_calibration_apply_dc = true"));
        assert!(updated.contains("tx_calibration_apply_iq = false"));
        assert!(!updated.contains("tx_calibration_apply_iq = true"));
    }

    #[test]
    fn system_info_infers_sxceiver_from_soapy_find_output() {
        let soapy_info = "\
Lib Version: v0.8.1
Found device 0
  driver = sx
  label = sx";

        assert_eq!(infer_sdr_name_from_soapy_info(soapy_info).as_deref(), Some("SXceiver"));
    }

    #[test]
    fn live_sds_post_rejects_protocol_id_that_would_wrap() {
        let err = parse_live_sds_post(&json!({
            "text": "hello",
            "protocol_id": 386
        }))
        .expect_err("protocol_id above 255 must not wrap to 130");

        assert_eq!(err, "protocol_id must be 0..255");
    }

    #[test]
    fn live_sds_post_accepts_vendor_sds_tl_transport_pid() {
        let post = parse_live_sds_post(&json!({
            "text": "hello",
            "protocol_id": 0xDC
        }))
        .expect("vendor/user-defined SDS-TL PID should be accepted");

        assert_eq!(post.protocol_id, 0xDC);
    }

    #[test]
    fn live_sds_post_rejects_non_sds_tl_transport_pid() {
        for protocol_id in [0x02, 0xFF] {
            let err = parse_live_sds_post(&json!({
                "text": "hello",
                "protocol_id": protocol_id
            }))
            .expect_err("non-SDS-TL transport PID must be rejected");

            assert_eq!(
                err,
                "protocol_id must be 0x82 text messaging or user-defined SDS-TL PID 0xC0..0xFE for text-style SDS; standard non-text SDS-TL application PIDs, including WAP/WCMP, require their own payload encoders and are rejected here"
            );
        }
    }

    #[test]
    fn live_sds_post_rejects_wap_protocol_id_0x84() {
        let err = parse_live_sds_post(&json!({
            "text": "hello",
            "protocol_id": 0x84
        }))
        .expect_err("dashboard live SDS text path must reject WAP PID");

        assert_eq!(
            err,
            "protocol_id must be 0x82 text messaging or user-defined SDS-TL PID 0xC0..0xFE for text-style SDS; standard non-text SDS-TL application PIDs, including WAP/WCMP, require their own payload encoders and are rejected here"
        );
    }

    #[test]
    fn live_sds_post_rejects_source_issi_above_24_bits() {
        let err = parse_live_sds_post(&json!({
            "text": "hello",
            "source_issi": 0x0100_0000u64
        }))
        .expect_err("source_issi above 24 bits must not wrap to 0");

        assert_eq!(err, "source_issi must be 0..16777215");
    }

    #[test]
    fn ws_sds_command_accepts_24_bit_dest_issi() {
        let cmd = parse_ws_sds_command(&json!({
            "dest_issi": 0x00FF_FFFFu64,
            "message": "hello"
        }))
        .expect("24-bit destination should be accepted");

        assert_eq!(
            cmd,
            WsSdsCommand {
                dest_issi: 0x00FF_FFFF,
                message: "hello".to_string(),
            }
        );
    }

    #[test]
    fn ws_sds_command_rejects_dest_issi_above_24_bits() {
        let err = parse_ws_sds_command(&json!({
            "dest_issi": 0x0100_0000u64,
            "message": "hello"
        }))
        .expect_err("dest_issi above 24 bits must not reach CMCE");

        assert_eq!(err, "dest_issi must be 1..16777215");
    }

    #[test]
    fn ws_sds_command_rejects_dest_issi_that_would_truncate_to_valid_issi() {
        let err = parse_ws_sds_command(&json!({
            "dest_issi": 0x1_0000_0001u64,
            "message": "hello"
        }))
        .expect_err("dest_issi above u32 must not truncate to ISSI 1");

        assert_eq!(err, "dest_issi must be 1..16777215");
    }

    #[test]
    fn ws_wap_command_accepts_24_bit_dest_issi() {
        let cmd = parse_ws_wap_command(&json!({
            "dest_issi": 0x00FF_FFFFu64,
            "color": true
        }))
        .expect("24-bit destination should be accepted");

        assert_eq!(
            cmd,
            WsWapCommand {
                dest_issi: 0x00FF_FFFF,
                color: true,
            }
        );
    }

    #[test]
    fn ws_wap_command_rejects_dest_issi_above_24_bits() {
        let err = parse_ws_wap_command(&json!({
            "dest_issi": 0x0100_0000u64
        }))
        .expect_err("dest_issi above 24 bits must not reach CMCE");

        assert_eq!(err, "dest_issi must be 1..16777215");
    }

    #[test]
    fn dashboard_wap_ws_dispatches_raw_type4_wap_sds() {
        let state = Arc::new(RwLock::new(DashboardStateInner::new("test.toml".to_string())));
        let (tx, rx) = crossbeam_channel::bounded(crate::net_control::channel::CONTROL_COMMAND_CHANNEL_CAPACITY);
        let cmd_tx = Arc::new(Mutex::new(Some(tx)));
        let update_state = Arc::new(Mutex::new(UpdateState::new()));

        handle_ws_command(r#"{"type":"wap","dest_issi":2260618}"#, &state, &cmd_tx, &update_state);

        let command = rx.try_recv().expect("dashboard should dispatch a WAP command");
        let ControlCommand::SendRawSds {
            source_ssi,
            dest_ssi,
            dest_is_group,
            sdti,
            len_bits,
            payload,
            ..
        } = command
        else {
            panic!("expected dashboard WAP to use raw SDS Type4");
        };

        assert_eq!(source_ssi, DASHBOARD_WAP_SOURCE_SSI);
        assert_eq!(dest_ssi, 2260618);
        assert!(!dest_is_group);
        assert_eq!(sdti, 3);
        assert_eq!(payload[0], WAP_WDP_PROTOCOL_ID);
        assert_eq!(&payload[1..], WAP_MVP_PAGE_TEXT.as_bytes());
        assert_eq!(len_bits, (payload.len() * 8) as u16);
        assert!(payload.len() <= WAP_SDS_TYPE4_MAX_BYTE_ALIGNED_PAYLOAD_BYTES);
    }
}
