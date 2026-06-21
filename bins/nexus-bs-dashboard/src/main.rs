// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use clap::Parser;
use crossbeam_channel::{Receiver, bounded};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tetra_config::bluestation::{SharedConfig, StackConfig, parsing};
use tetra_entities::net_control::codec::ControlCodecJson;
use tetra_entities::net_control::commands::ControlCommand;
use tetra_entities::net_dashboard::DashboardServer;
use tetra_entities::net_telemetry::codec::TelemetryCodecJson;
use tetra_entities::net_telemetry::{TELEMETRY_PROTOCOL_VERSION, select_telemetry_subprotocol};
use tungstenite::Message;
use tungstenite::handshake::server::{ErrorResponse, Request, Response};

const DASHBOARD_CONTROL_QUEUE_CAPACITY: usize = 2048;

#[derive(Parser, Debug)]
#[command(
    name = "nexus-bs-dashboard",
    author,
    version,
    about = "Nexus-BS external dashboard/API service",
    long_about = "Serves the Nexus-BS dashboard API, WebSocket and static assets from a separate process. Runtime telemetry is received from the core over the telemetry link; commands are submitted to nexus-bs-control-service."
)]
struct Args {
    #[arg(
        long,
        help = "Dashboard bind address; default NEXUS_BS_DASHBOARD_BIND or [dashboard].bind or 0.0.0.0"
    )]
    bind: Option<String>,
    #[arg(long, help = "Dashboard port; default NEXUS_BS_DASHBOARD_PORT or [dashboard].port or 8080")]
    port: Option<u16>,
    #[arg(
        long,
        help = "Persistent config path; default NEXUS_BS_PERSISTENT_CONFIG or /etc/nexus-bs/config.toml"
    )]
    config: Option<String>,
    #[arg(
        long,
        help = "Dashboard static asset directory; default NEXUS_BS_DASHBOARD_STATIC_DIR or [dashboard].static_dir or ./dashboard"
    )]
    static_dir: Option<String>,
    #[arg(
        long,
        help = "Telemetry listen address for core connection; default NEXUS_BS_DASHBOARD_TELEMETRY_LISTEN or 127.0.0.1:9001"
    )]
    telemetry_listen: Option<String>,
    #[arg(
        long,
        help = "Control-service HTTP command endpoint; default NEXUS_BS_DASHBOARD_CONTROL_URL or http://127.0.0.1:9003/command"
    )]
    control_url: Option<String>,
}

#[derive(Clone, Debug)]
struct DashboardRuntimeConfig {
    bind: String,
    port: u16,
    config_path: String,
    static_dir: Option<String>,
    source_dir: Option<String>,
    auth: Option<(String, String)>,
    telemetry_listen: String,
    control_url: String,
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let config_path = args
        .config
        .or_else(|| non_empty_env("NEXUS_BS_PERSISTENT_CONFIG"))
        .unwrap_or_else(|| "/etc/nexus-bs/config.toml".to_string());
    let stack_config = load_optional_stack_config(&config_path);
    let dash_cfg = stack_config.as_ref().and_then(|cfg| cfg.dashboard.clone());

    let runtime = DashboardRuntimeConfig {
        bind: args
            .bind
            .or_else(|| non_empty_env("NEXUS_BS_DASHBOARD_BIND"))
            .or_else(|| dash_cfg.as_ref().map(|cfg| cfg.bind.clone()))
            .unwrap_or_else(|| "0.0.0.0".to_string()),
        port: args
            .port
            .or_else(|| non_empty_env("NEXUS_BS_DASHBOARD_PORT").and_then(|value| value.parse::<u16>().ok()))
            .or_else(|| dash_cfg.as_ref().map(|cfg| cfg.port))
            .unwrap_or(8080),
        config_path,
        static_dir: args
            .static_dir
            .or_else(|| non_empty_env("NEXUS_BS_DASHBOARD_STATIC_DIR"))
            .or_else(|| dash_cfg.as_ref().and_then(|cfg| cfg.static_dir.clone()))
            .or_else(|| Some("dashboard".to_string())),
        source_dir: dash_cfg.as_ref().and_then(|cfg| cfg.source_dir.clone()),
        auth: dash_cfg.as_ref().and_then(|cfg| match (&cfg.username, &cfg.password) {
            (Some(user), Some(pass)) => Some((user.clone(), pass.clone())),
            _ => None,
        }),
        telemetry_listen: args
            .telemetry_listen
            .or_else(|| non_empty_env("NEXUS_BS_DASHBOARD_TELEMETRY_LISTEN"))
            .unwrap_or_else(|| "127.0.0.1:9001".to_string()),
        control_url: args
            .control_url
            .or_else(|| non_empty_env("NEXUS_BS_DASHBOARD_CONTROL_URL"))
            .unwrap_or_else(|| "http://127.0.0.1:9003/command".to_string()),
    };

    let (cmd_tx, cmd_rx) = bounded::<ControlCommand>(DASHBOARD_CONTROL_QUEUE_CAPACITY);
    start_control_bridge(runtime.control_url.clone(), cmd_rx);

    let mut dashboard = DashboardServer::new(runtime.config_path.clone());
    dashboard.set_source_dir(runtime.source_dir.clone());
    dashboard.set_static_dir(runtime.static_dir.clone());
    dashboard.set_auth(runtime.auth.clone());
    if let Some(config) = stack_config {
        dashboard.set_shared_config(SharedConfig::from_parts(config, None));
    }
    dashboard.set_cmd_sender(cmd_tx.clone());
    dashboard.set_rf_cmd_sender(cmd_tx.clone());
    dashboard.set_phy_cmd_sender(cmd_tx);
    dashboard.start(&runtime.bind, runtime.port);

    let dashboard = Arc::new(dashboard);
    start_telemetry_listener(runtime.telemetry_listen.clone(), Arc::clone(&dashboard));
    start_journal_log_bridge(Arc::clone(&dashboard));

    tracing::info!(
        "Nexus-BS external dashboard listening on http://{}:{} (config={}, static_dir={}, telemetry={}, control={})",
        runtime.bind,
        runtime.port,
        runtime.config_path,
        runtime.static_dir.as_deref().unwrap_or("<embedded>"),
        runtime.telemetry_listen,
        runtime.control_url
    );

    loop {
        std::thread::park();
    }
}

fn start_journal_log_bridge(dashboard: Arc<DashboardServer>) {
    let units = non_empty_env("NEXUS_BS_DASHBOARD_JOURNAL_UNITS")
        .unwrap_or_else(|| "nexus-bs.service,nexus-bs-control.service,nexus-bs-dashboard.service".to_string());
    std::thread::Builder::new()
        .name("dashboard-journal-log".into())
        .spawn(move || {
            let unit_args: Vec<String> = units
                .split(',')
                .map(str::trim)
                .filter(|unit| !unit.is_empty())
                .flat_map(|unit| ["-u".to_string(), unit.to_string()])
                .collect();
            if unit_args.is_empty() {
                return;
            }
            loop {
                let mut command = Command::new("journalctl");
                command
                    .arg("-f")
                    .arg("-n")
                    .arg("120")
                    .arg("--no-pager")
                    .args(&unit_args)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null());
                let mut child = match command.spawn() {
                    Ok(child) => child,
                    Err(error) => {
                        dashboard.push_log("WARN", format!("journal log bridge unavailable: {error}"));
                        std::thread::sleep(Duration::from_secs(15));
                        continue;
                    }
                };
                let Some(stdout) = child.stdout.take() else {
                    let _ = child.kill();
                    std::thread::sleep(Duration::from_secs(15));
                    continue;
                };
                let reader = std::io::BufReader::new(stdout);
                for line in std::io::BufRead::lines(reader) {
                    match line {
                        Ok(line) if !line.trim().is_empty() => {
                            let level = journal_level_hint(&line);
                            dashboard.push_log(level, line);
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
                std::thread::sleep(Duration::from_secs(3));
            }
        })
        .expect("failed to spawn dashboard journal log bridge");
}

fn journal_level_hint(line: &str) -> &'static str {
    if line.contains(" ERROR ") || line.contains(" error:") || line.contains(" failed") || line.contains("Failed") {
        "ERROR"
    } else if line.contains(" WARN ") || line.contains("WARNING") || line.contains(" warning") {
        "WARN"
    } else {
        "INFO"
    }
}

fn load_optional_stack_config(path: &str) -> Option<StackConfig> {
    match parsing::from_file(path) {
        Ok(config) => Some(config),
        Err(error) => {
            tracing::warn!("Dashboard: config '{}' could not be loaded for dashboard settings: {}", path, error);
            None
        }
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn start_control_bridge(control_url: String, rx: Receiver<ControlCommand>) {
    std::thread::Builder::new()
        .name("dashboard-control-bridge".into())
        .spawn(move || {
            let codec = ControlCodecJson;
            while let Ok(command) = rx.recv() {
                let body = codec.encode_command(&command);
                if let Err(error) = post_control_command(&control_url, &body) {
                    tracing::warn!("Dashboard control command dropped: {}", error);
                }
            }
        })
        .expect("failed to spawn dashboard control bridge");
}

fn post_control_command(url: &str, body: &[u8]) -> Result<(), String> {
    let (host_port, path) = parse_http_url(url)?;
    let mut stream = TcpStream::connect(&host_port).map_err(|error| format!("connect {host_port}: {error}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|error| format!("write control request: {error}"))?;

    let mut response = Vec::new();
    stream
        .take(4096)
        .read_to_end(&mut response)
        .map_err(|error| format!("read control response: {error}"))?;
    let status = String::from_utf8_lossy(&response).lines().next().unwrap_or("").to_string();
    if status.contains(" 200 ") || status.contains(" 202 ") {
        Ok(())
    } else {
        Err(format!("control service rejected command: {status}"))
    }
}

fn parse_http_url(url: &str) -> Result<(String, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| "only http:// control URLs are supported on localhost".to_string())?;
    let (host_port, path) = rest.split_once('/').unwrap_or((rest, "command"));
    if host_port.trim().is_empty() {
        return Err("control URL host is empty".to_string());
    }
    let path = format!("/{}", path.trim_start_matches('/'));
    Ok((host_port.to_string(), path))
}

fn start_telemetry_listener(listen: String, dashboard: Arc<DashboardServer>) {
    std::thread::Builder::new()
        .name("dashboard-telemetry-listener".into())
        .spawn(move || {
            let listener = match TcpListener::bind(&listen) {
                Ok(listener) => listener,
                Err(error) => {
                    tracing::error!("Dashboard telemetry listener failed to bind {}: {}", listen, error);
                    return;
                }
            };
            tracing::info!("Dashboard telemetry listener on {}", listen);
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let dashboard = Arc::clone(&dashboard);
                        let peer = stream
                            .peer_addr()
                            .map(|addr| addr.to_string())
                            .unwrap_or_else(|_| "unknown".to_string());
                        std::thread::Builder::new()
                            .name("dashboard-telemetry-conn".into())
                            .spawn(move || handle_telemetry_connection(stream, &peer, dashboard))
                            .ok();
                    }
                    Err(error) => tracing::warn!("Dashboard telemetry accept failed: {}", error),
                }
            }
        })
        .expect("failed to spawn dashboard telemetry listener");
}

fn handle_telemetry_connection(stream: TcpStream, peer: &str, dashboard: Arc<DashboardServer>) {
    let callback = |req: &Request, mut response: Response| -> Result<Response, ErrorResponse> {
        let proto = req
            .headers()
            .get("Sec-WebSocket-Protocol")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if let Some(selected_protocol) = select_telemetry_subprotocol(proto) {
            response
                .headers_mut()
                .insert("Sec-WebSocket-Protocol", selected_protocol.parse().unwrap());
            Ok(response)
        } else {
            Err(ErrorResponse::new(Some(format!(
                "unsupported subprotocol; expected {}",
                TELEMETRY_PROTOCOL_VERSION
            ))))
        }
    };

    let mut ws = match tungstenite::accept_hdr(stream, callback) {
        Ok(ws) => ws,
        Err(error) => {
            tracing::warn!("[{}] telemetry websocket rejected: {}", peer, error);
            return;
        }
    };
    let codec = TelemetryCodecJson;
    tracing::info!("[{}] telemetry connected", peer);
    dashboard.reset_runtime_snapshot(&format!("telemetry connected from {peer}"));
    loop {
        match ws.read() {
            Ok(Message::Binary(data)) => match codec.decode(&data) {
                Ok(event) => dashboard.handle_telemetry(event),
                Err(error) => tracing::warn!("[{}] telemetry decode failed: {}", peer, error),
            },
            Ok(Message::Text(text)) => tracing::warn!("[{}] unexpected telemetry text frame ({} bytes)", peer, text.len()),
            Ok(Message::Ping(_)) => {
                let _ = ws.flush();
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::ConnectionClosed) => break,
            Err(error) => {
                tracing::warn!("[{}] telemetry websocket error: {}", peer, error);
                break;
            }
        }
    }
    tracing::info!("[{}] telemetry disconnected", peer);
    dashboard.reset_runtime_snapshot(&format!("telemetry disconnected from {peer}"));
}

#[allow(dead_code)]
fn ensure_static_dir_path(path: Option<String>) -> Option<String> {
    path.map(PathBuf::from)
        .map(|p| p.to_string_lossy().to_string())
        .filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_http_control_url_defaults_path_when_absent() {
        assert_eq!(
            parse_http_url("http://127.0.0.1:9003").unwrap(),
            ("127.0.0.1:9003".to_string(), "/command".to_string())
        );
    }

    #[test]
    fn parse_http_control_url_keeps_explicit_path() {
        assert_eq!(
            parse_http_url("http://127.0.0.1:9003/api/control").unwrap(),
            ("127.0.0.1:9003".to_string(), "/api/control".to_string())
        );
    }

    #[test]
    fn parse_http_control_url_rejects_tls_for_local_bridge() {
        assert!(parse_http_url("https://127.0.0.1:9003/command").is_err());
    }
}
