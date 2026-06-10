use clap::Parser;
use std::fs;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const HTTP_HEADER_MAX: usize = 64 * 1024;
const STATIC_FILE_MAX: u64 = 2 * 1024 * 1024;
const CONNECTION_MAX: usize = 128;
const AUTH_STATUS_RESPONSE_MAX: usize = 16 * 1024;
const CORE_AUTH_STATUS_TIMEOUT: Duration = Duration::from_millis(500);
const SECURITY_HEADERS: &str = concat!(
    "X-Content-Type-Options: nosniff\r\n",
    "X-Frame-Options: DENY\r\n",
    "Referrer-Policy: no-referrer\r\n",
    "Content-Security-Policy: frame-ancestors 'none'; object-src 'none'; base-uri 'none'\r\n",
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StaticAuthAccess {
    Allow,
    LoginRequired,
}

#[derive(Parser, Debug)]
#[command(
    name = "nexus-bs-dashboard",
    author,
    version,
    about = "Nexus-BS external dashboard front-end",
    long_about = "Serves Nexus-BS dashboard assets from a separate process and proxies API/WebSocket traffic to the loopback-only Nexus-BS core dashboard API"
)]
struct Args {
    #[arg(long, help = "Public dashboard bind address; default NEXUS_BS_DASHBOARD_BIND or 0.0.0.0")]
    bind: Option<String>,
    #[arg(long, help = "Public dashboard port; default NEXUS_BS_DASHBOARD_PORT or 8080")]
    port: Option<u16>,
    #[arg(long, help = "Core dashboard API address; default NEXUS_BS_DASHBOARD_CORE or 127.0.0.1:18080")]
    core: Option<String>,
    #[arg(
        long,
        help = "Dashboard static asset directory; default NEXUS_BS_DASHBOARD_STATIC_DIR or ./dashboard"
    )]
    static_dir: Option<String>,
}

#[derive(Clone, Debug)]
struct DashboardFrontendConfig {
    bind: String,
    port: u16,
    core: String,
    static_dir: PathBuf,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let config = DashboardFrontendConfig {
        bind: args
            .bind
            .or_else(|| non_empty_env("NEXUS_BS_DASHBOARD_BIND"))
            .unwrap_or_else(|| "0.0.0.0".to_string()),
        port: args
            .port
            .or_else(|| non_empty_env("NEXUS_BS_DASHBOARD_PORT").and_then(|value| value.parse::<u16>().ok()))
            .unwrap_or(8080),
        core: args
            .core
            .or_else(|| non_empty_env("NEXUS_BS_DASHBOARD_CORE"))
            .unwrap_or_else(|| "127.0.0.1:18080".to_string()),
        static_dir: PathBuf::from(
            args.static_dir
                .or_else(|| non_empty_env("NEXUS_BS_DASHBOARD_STATIC_DIR"))
                .unwrap_or_else(|| "dashboard".to_string()),
        ),
    };

    let addr = format!("{}:{}", config.bind, config.port);
    let listener = match TcpListener::bind(&addr) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!("Dashboard front-end failed to bind {}: {}", addr, error);
            std::process::exit(1);
        }
    };

    tracing::info!(
        "Nexus-BS dashboard front-end listening on http://{} (core={}, static_dir={})",
        addr,
        config.core,
        config.static_dir.display()
    );

    let active = Arc::new(AtomicUsize::new(0));
    let config = Arc::new(config);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let current = active.fetch_add(1, Ordering::AcqRel);
        if current >= CONNECTION_MAX {
            active.fetch_sub(1, Ordering::AcqRel);
            text_response(stream, 503, "dashboard front-end connection limit reached");
            continue;
        }

        let active_for_guard = Arc::clone(&active);
        let active_for_error = Arc::clone(&active);
        let config = Arc::clone(&config);
        if let Err(error) = std::thread::Builder::new().name("dashboard-front-conn".into()).spawn(move || {
            let _guard = ConnectionGuard(active_for_guard);
            handle_connection(stream, &config);
        }) {
            tracing::warn!("Dashboard front-end failed to spawn connection handler: {}", error);
            active_for_error.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

struct ConnectionGuard(Arc<AtomicUsize>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn handle_connection(mut client: TcpStream, config: &DashboardFrontendConfig) {
    let _ = client.set_read_timeout(Some(Duration::from_secs(3)));
    let header = match read_http_header(&mut client) {
        Ok(header) => header,
        Err(error) => {
            text_response(client, 400, &error);
            return;
        }
    };
    let header_text = String::from_utf8_lossy(&header);
    let req_line = header_text.lines().next().unwrap_or("");
    let method = request_method(req_line).unwrap_or("");
    let path = request_path(req_line).unwrap_or_else(|| "/".to_string());

    if is_backend_route(&path) {
        proxy_to_core(client, header, &config.core);
        return;
    }

    if method != "GET" && method != "HEAD" {
        text_response(client, 405, "method not allowed");
        return;
    }

    match authorize_static_request(&header_text, &config.core) {
        Ok(StaticAuthAccess::Allow) => {}
        Ok(StaticAuthAccess::LoginRequired) => {
            if should_redirect_unauthenticated_static(&path) {
                redirect_response(client, "/login");
            } else {
                text_response(client, 401, "Unauthorized - please log in");
            }
            return;
        }
        Err(error) => {
            tracing::warn!("Dashboard front-end auth status check failed: {}", error);
            text_response(client, 502, "Nexus-BS core dashboard auth status unavailable");
            return;
        }
    }

    serve_static(client, &config.static_dir, &path, method == "HEAD");
}

fn read_http_header(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut header = Vec::with_capacity(2048);
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).map_err(|_| "failed to read request".to_string())?;
        if n == 0 {
            return Err("empty request".to_string());
        }
        header.push(byte[0]);
        if header.len() > HTTP_HEADER_MAX {
            return Err(format!("request headers too large (max {HTTP_HEADER_MAX} bytes)"));
        }
        if header.ends_with(b"\r\n\r\n") {
            return Ok(header);
        }
    }
}

fn request_method(req_line: &str) -> Option<&str> {
    req_line.split_whitespace().next()
}

fn request_path(req_line: &str) -> Option<String> {
    let target = req_line.split_whitespace().nth(1)?;
    let path = target.split('?').next().unwrap_or(target);
    Some(path.to_string())
}

fn is_backend_route(path: &str) -> bool {
    path == "/ws" || path == "/login" || path == "/logout" || path.starts_with("/api/")
}

fn authorize_static_request(headers: &str, core: &str) -> Result<StaticAuthAccess, String> {
    let mut backend = TcpStream::connect(core).map_err(|error| format!("connect core {core}: {error}"))?;
    let _ = backend.set_read_timeout(Some(CORE_AUTH_STATUS_TIMEOUT));
    let _ = backend.set_write_timeout(Some(CORE_AUTH_STATUS_TIMEOUT));

    let request = format!(
        "GET /api/auth/status HTTP/1.1\r\nHost: nexus-bs-core\r\nConnection: close\r\n{}\r\n",
        cookie_headers_for_core(headers)
    );
    backend
        .write_all(request.as_bytes())
        .map_err(|error| format!("write auth status request: {error}"))?;

    let mut response = Vec::new();
    backend
        .take(AUTH_STATUS_RESPONSE_MAX as u64)
        .read_to_end(&mut response)
        .map_err(|error| format!("read auth status response: {error}"))?;
    parse_auth_status_response(&response)
}

fn cookie_headers_for_core(headers: &str) -> String {
    let mut out = String::new();
    for line in headers.lines() {
        if line.to_ascii_lowercase().starts_with("cookie:") {
            out.push_str(line.trim_end_matches('\r'));
            out.push_str("\r\n");
        }
    }
    out
}

fn parse_auth_status_response(response: &[u8]) -> Result<StaticAuthAccess, String> {
    let text = String::from_utf8_lossy(response);
    let status = text.lines().next().unwrap_or("");
    if !status.starts_with("HTTP/1.") || !status.contains(" 200 ") {
        return Err(format!("unexpected auth status response: {status}"));
    }
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .or_else(|| text.split_once("\n\n").map(|(_, body)| body))
        .ok_or_else(|| "auth status response missing body".to_string())?;
    let auth_required = json_bool_field(body, "auth_required").ok_or_else(|| "auth_required missing".to_string())?;
    let session_valid = json_bool_field(body, "session_valid").ok_or_else(|| "session_valid missing".to_string())?;

    if !auth_required || session_valid {
        Ok(StaticAuthAccess::Allow)
    } else {
        Ok(StaticAuthAccess::LoginRequired)
    }
}

fn json_bool_field(body: &str, key: &str) -> Option<bool> {
    let needle = format!("\"{key}\"");
    let idx = body.find(&needle)?;
    let after_key = &body[idx + needle.len()..];
    let colon = after_key.find(':')?;
    let value = after_key[colon + 1..].trim_start();
    if value.starts_with("true") {
        Some(true)
    } else if value.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn should_redirect_unauthenticated_static(path: &str) -> bool {
    path == "/" || Path::new(path.trim_start_matches('/')).extension().is_none()
}

fn proxy_to_core(mut client: TcpStream, header: Vec<u8>, core: &str) {
    let mut backend = match TcpStream::connect(core) {
        Ok(stream) => stream,
        Err(error) => {
            tracing::warn!("Dashboard front-end failed to connect core {}: {}", core, error);
            text_response(client, 502, "Nexus-BS core dashboard API unavailable");
            return;
        }
    };

    if backend.write_all(&header).is_err() {
        text_response(client, 502, "failed to forward request to core");
        return;
    }

    let Ok(mut client_reader) = client.try_clone() else {
        text_response(client, 500, "failed to clone client stream");
        return;
    };
    let Ok(mut backend_writer) = backend.try_clone() else {
        text_response(client, 500, "failed to clone backend stream");
        return;
    };

    let uplink = std::thread::Builder::new().name("dashboard-front-uplink".into()).spawn(move || {
        let _ = std::io::copy(&mut client_reader, &mut backend_writer);
        let _ = backend_writer.shutdown(Shutdown::Write);
    });

    let _ = std::io::copy(&mut backend, &mut client);
    let _ = client.shutdown(Shutdown::Write);
    if let Ok(join) = uplink {
        let _ = join.join();
    }
}

fn serve_static(mut stream: TcpStream, static_dir: &Path, path: &str, head_only: bool) {
    let asset = match resolve_static_path(static_dir, path) {
        Ok(path) => path,
        Err(status) => {
            text_response(stream, status, if status == 400 { "bad request" } else { "not found" });
            return;
        }
    };

    let Ok(meta) = fs::metadata(&asset) else {
        text_response(stream, 404, "not found");
        return;
    };
    if !meta.is_file() {
        text_response(stream, 404, "not found");
        return;
    }
    if meta.len() > STATIC_FILE_MAX {
        text_response(stream, 413, "static asset too large");
        return;
    }

    let body = if head_only {
        Vec::new()
    } else {
        match fs::read(&asset) {
            Ok(body) => body,
            Err(error) => {
                text_response(stream, 500, &error.to_string());
                return;
            }
        }
    };
    let content_len = if head_only { meta.len() as usize } else { body.len() };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\n{}Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        content_type(&asset),
        SECURITY_HEADERS,
        content_len
    );
    let _ = stream.write_all(header.as_bytes());
    if !head_only {
        let _ = stream.write_all(&body);
    }
}

fn resolve_static_path(static_dir: &Path, request_path: &str) -> Result<PathBuf, u16> {
    if request_path == "/" || request_path.is_empty() {
        return Ok(static_dir.join("index.html"));
    }

    let trimmed = request_path.trim_start_matches('/');
    if trimmed.is_empty() {
        return Ok(static_dir.join("index.html"));
    }

    let mut relative = PathBuf::new();
    for component in Path::new(trimmed).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            _ => return Err(400),
        }
    }

    let candidate = static_dir.join(&relative);
    if candidate.is_file() {
        return Ok(candidate);
    }
    Ok(static_dir.join("index.html"))
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn text_response(mut stream: TcpStream, code: u16, body: &str) {
    let status = match code {
        200 => "OK",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/plain; charset=utf-8\r\n{}Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        code,
        status,
        SECURITY_HEADERS,
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
}

fn redirect_response(mut stream: TcpStream, location: &str) {
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: {}\r\n{}Cache-Control: no-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        location, SECURITY_HEADERS
    );
    let _ = stream.write_all(response.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_routes_are_proxied_to_core() {
        assert!(is_backend_route("/api/calls"));
        assert!(is_backend_route("/api/system"));
        assert!(is_backend_route("/api/auth/status"));
        assert!(is_backend_route("/ws"));
        assert!(is_backend_route("/login"));
        assert!(!is_backend_route("/"));
        assert!(!is_backend_route("/assets/app.js"));
    }

    #[test]
    fn static_path_rejects_traversal() {
        let root = Path::new("/tmp/dashboard");
        assert_eq!(resolve_static_path(root, "/../config.toml").unwrap_err(), 400);
        assert_eq!(resolve_static_path(root, "/assets/../../config.toml").unwrap_err(), 400);
    }

    #[test]
    fn auth_status_parser_allows_open_dashboard() {
        let response = br#"HTTP/1.1 200 OK
Content-Type: application/json

{"auth_required":false,"session_valid":true}"#;

        assert_eq!(parse_auth_status_response(response).unwrap(), StaticAuthAccess::Allow);
    }

    #[test]
    fn auth_status_parser_requires_login_without_session() {
        let response = br#"HTTP/1.1 200 OK
Content-Type: application/json

{"auth_required":true,"session_valid":false}"#;

        assert_eq!(parse_auth_status_response(response).unwrap(), StaticAuthAccess::LoginRequired);
    }

    #[test]
    fn auth_status_parser_allows_valid_session() {
        let response = br#"HTTP/1.1 200 OK
Content-Type: application/json

{"auth_required":true,"session_valid":true}"#;

        assert_eq!(parse_auth_status_response(response).unwrap(), StaticAuthAccess::Allow);
    }

    #[test]
    fn auth_status_parser_rejects_malformed_response() {
        let response = b"HTTP/1.1 200 OK\r\n\r\n{\"auth_required\":true}";

        assert!(parse_auth_status_response(response).is_err());
    }

    #[test]
    fn unauthenticated_static_redirect_policy_separates_pages_from_assets() {
        assert!(should_redirect_unauthenticated_static("/"));
        assert!(should_redirect_unauthenticated_static("/traffic"));
        assert!(!should_redirect_unauthenticated_static("/assets/app.js"));
        assert!(!should_redirect_unauthenticated_static("/favicon.ico"));
    }

    #[test]
    fn cookie_headers_forward_only_cookie_to_core_auth_probe() {
        let headers = "GET / HTTP/1.1\r\nHost: example\r\nCookie: fs_session=abc; fs_auth=1\r\nX-Test: no\r\n\r\n";

        let forwarded = cookie_headers_for_core(headers);

        assert_eq!(forwarded, "Cookie: fs_session=abc; fs_auth=1\r\n");
    }

    #[test]
    fn frontend_security_headers_lock_down_static_responses() {
        assert!(SECURITY_HEADERS.contains("X-Content-Type-Options: nosniff"));
        assert!(SECURITY_HEADERS.contains("X-Frame-Options: DENY"));
        assert!(SECURITY_HEADERS.contains("frame-ancestors 'none'"));
    }

    fn spawn_fake_core_auth_status(body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake core");
        let addr = listener.local_addr().expect("fake core addr").to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fake core request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set fake core read timeout");
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while stream.read(&mut byte).expect("read fake core request") != 0 {
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("write fake core response");
            String::from_utf8_lossy(&request).to_string()
        });
        (addr, handle)
    }

    #[test]
    fn split_frontend_auth_probe_allows_when_core_accepts_session() {
        let (core, handle) = spawn_fake_core_auth_status(r#"{"auth_required":true,"session_valid":true}"#);
        let headers = "GET / HTTP/1.1\r\nHost: dashboard\r\nCookie: fs_session=abc; fs_auth=1\r\nX-Ignore: no\r\n\r\n";

        let access = authorize_static_request(headers, &core).expect("auth probe should parse");
        let request = handle.join().expect("fake core should return request");

        assert_eq!(access, StaticAuthAccess::Allow);
        assert!(request.starts_with("GET /api/auth/status HTTP/1.1"));
        assert!(request.contains("Cookie: fs_session=abc; fs_auth=1\r\n"));
        assert!(!request.contains("X-Ignore"));
    }

    #[test]
    fn split_frontend_auth_probe_blocks_when_core_requires_login() {
        let (core, handle) = spawn_fake_core_auth_status(r#"{"auth_required":true,"session_valid":false}"#);

        let access = authorize_static_request("GET / HTTP/1.1\r\nHost: dashboard\r\n\r\n", &core).expect("auth probe should parse");
        let _ = handle.join().expect("fake core should complete");

        assert_eq!(access, StaticAuthAccess::LoginRequired);
    }
}
