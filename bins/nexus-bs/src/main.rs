use clap::Parser;
use crossbeam_channel;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use tetra_core::tetra_entities::TetraEntity;
use tetra_entities::net_control::channel::build_all_control_links;
use tetra_entities::net_control::{
    CONTROL_HEARTBEAT_INTERVAL, CONTROL_HEARTBEAT_TIMEOUT, CONTROL_PROTOCOL_VERSION, CommandDispatcher, ControlWorker,
};

use tetra_config::bluestation::{PhyBackend, SharedConfig, StackConfig, StackState, parsing};
use tetra_core::{PRODUCT_NAME, PRODUCT_USER_AGENT, PRODUCT_VERSION_TAG, TdmaTime, debug};
use tetra_entities::MessageRouter;
use tetra_entities::net_brew::entity::BrewEntity;
use tetra_entities::net_brew::new_websocket_transport;
use tetra_entities::net_dashboard::DashboardServer;
use tetra_entities::net_telemetry::worker::TelemetryWorker;
use tetra_entities::net_telemetry::{
    TELEMETRY_HEARTBEAT_INTERVAL, TELEMETRY_HEARTBEAT_TIMEOUT, TELEMETRY_PROTOCOL_VERSION, TelemetrySource, telemetry_channel,
};
use tetra_entities::network::transports::websocket::{WebSocketTransport, WebSocketTransportConfig};
use tetra_entities::{
    cmce::cmce_bs::CmceBs,
    llc::llc_bs_ms::Llc,
    lmac::lmac_bs::LmacBs,
    mle::mle_bs::MleBs,
    mm::mm_bs::MmBs,
    phy::{components::soapy_dev::RxTxDevSoapySdr, phy_bs::PhyBs},
    sndcp::sndcp_bs::Sndcp,
    umac::umac_bs::UmacBs,
};

const DASHBOARD_LOG_CHANNEL_CAPACITY: usize = 2048;

/// Result of loading config — either primary or fallback.
enum ConfigLoadResult {
    Primary(StackConfig),
    Fallback {
        config: StackConfig,
        fallback_path: String,
        primary_error: String,
    },
}

/// Try to load the primary config. If it fails, try the fallback
/// (`<config>.fallback` alongside the primary file).
/// Returns Ok(ConfigLoadResult) or exits if both fail.
fn load_config_with_fallback(cfg_path: &str) -> ConfigLoadResult {
    match parsing::from_file(cfg_path) {
        Ok(c) => ConfigLoadResult::Primary(c),
        Err(primary_err) => {
            let primary_err_str = primary_err.to_string();
            eprintln!("WARNING: Failed to load primary config '{}': {}", cfg_path, primary_err_str);

            // Fallback path: same directory, same name + ".fallback"
            let fallback_path = format!("{}.fallback", cfg_path);

            eprintln!("WARNING: Trying fallback config '{}'...", fallback_path);
            match parsing::from_file(&fallback_path) {
                Ok(c) => {
                    eprintln!(
                        "WARNING: Started on FALLBACK config '{}'. Primary config is invalid!",
                        fallback_path
                    );
                    ConfigLoadResult::Fallback {
                        config: c,
                        fallback_path,
                        primary_error: primary_err_str,
                    }
                }
                Err(fallback_err) => {
                    eprintln!("ERROR: Fallback config '{}' also failed: {}", fallback_path, fallback_err);
                    eprintln!("ERROR: No valid config available. Cannot start.");
                    eprintln!("HINT:  Fix '{}' or create a valid fallback at '{}'", cfg_path, fallback_path);
                    std::process::exit(1);
                }
            }
        }
    }
}

fn dashboard_editable_config_path(runtime_config_path: &str) -> String {
    std::env::var("NEXUS_BS_PERSISTENT_CONFIG")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| runtime_config_path.to_string())
}

fn dashboard_static_dir(config_static_dir: Option<String>) -> Option<String> {
    config_static_dir.or_else(|| {
        std::env::var("NEXUS_BS_DASHBOARD_STATIC_DIR")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

fn dashboard_core_bind(config_bind: String) -> String {
    std::env::var("NEXUS_BS_CORE_DASHBOARD_BIND")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or(config_bind)
}

fn dashboard_core_port(config_port: u16) -> u16 {
    std::env::var("NEXUS_BS_CORE_DASHBOARD_PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(config_port)
}

fn start_telemetry_worker(cfg: SharedConfig, telemetry_source: TelemetrySource) -> thread::JoinHandle<()> {
    let config = cfg.config();
    let tcfg = config.telemetry.as_ref().unwrap();

    let custom_root_certs = tcfg.ca_cert.as_ref().map(|path| {
        let der_bytes = std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("Failed to read CA certificate from '{}': {}", path, e);
            std::process::exit(1);
        });
        vec![rustls::pki_types::CertificateDer::from(der_bytes)]
    });

    let ws_config = WebSocketTransportConfig {
        host: tcfg.host.clone(),
        port: tcfg.port,
        use_tls: tcfg.use_tls,
        digest_auth_credentials: None,
        basic_auth_credentials: tcfg.credentials.clone(),
        endpoint_path: "/".to_string(),
        subprotocol: Some(TELEMETRY_PROTOCOL_VERSION.to_string()),
        user_agent: PRODUCT_USER_AGENT.to_string(),
        heartbeat_interval: TELEMETRY_HEARTBEAT_INTERVAL,
        heartbeat_timeout: TELEMETRY_HEARTBEAT_TIMEOUT,
        custom_root_certs,
    };

    thread::spawn(move || {
        let transport = WebSocketTransport::new(ws_config);
        let mut worker = TelemetryWorker::new(telemetry_source, transport);
        worker.run();
    })
}

fn start_control_worker(cfg: SharedConfig, command_dispatchers: HashMap<TetraEntity, CommandDispatcher>) -> thread::JoinHandle<()> {
    let config = cfg.config();
    let ccfg = config.control.as_ref().unwrap();

    let custom_root_certs = ccfg.ca_cert.as_ref().map(|path| {
        let der_bytes = std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("Failed to read CA certificate from '{}': {}", path, e);
            std::process::exit(1);
        });
        vec![rustls::pki_types::CertificateDer::from(der_bytes)]
    });

    let ws_config = WebSocketTransportConfig {
        host: ccfg.host.clone(),
        port: ccfg.port,
        use_tls: ccfg.use_tls,
        digest_auth_credentials: None,
        basic_auth_credentials: ccfg.credentials.clone(),
        endpoint_path: "/".to_string(),
        subprotocol: Some(CONTROL_PROTOCOL_VERSION.to_string()),
        user_agent: PRODUCT_USER_AGENT.to_string(),
        heartbeat_interval: CONTROL_HEARTBEAT_INTERVAL,
        heartbeat_timeout: CONTROL_HEARTBEAT_TIMEOUT,
        custom_root_certs,
    };

    thread::spawn(move || {
        let transport = WebSocketTransport::new(ws_config);
        let mut worker = ControlWorker::new(command_dispatchers, transport);
        worker.run();
    })
}

/// Start base station stack
fn build_bs_stack(cfg: &mut SharedConfig) -> (MessageRouter, Option<TelemetrySource>, HashMap<TetraEntity, CommandDispatcher>) {
    let mut router = MessageRouter::new(cfg.clone());

    // Build telemetry sink/source — always create if either telemetry or dashboard is enabled
    let needs_telemetry = cfg.config().telemetry.is_some() || cfg.config().dashboard.is_some();
    let (tsink, tsource) = if needs_telemetry {
        let (a, b) = telemetry_channel();
        (Some(a), Some(b))
    } else {
        (None, None)
    };

    // Add suitable Phy component based on PhyIo type
    match cfg.config().phy_io.backend {
        PhyBackend::SoapySdr => {
            let rxdev = RxTxDevSoapySdr::with_telemetry(cfg, tsink.clone());
            let phy = PhyBs::new(cfg.clone(), rxdev);
            router.register_entity(Box::new(phy));
        }
        _ => {
            panic!("Unsupported PhyIo type: {:?}", cfg.config().phy_io.backend);
        }
    }

    // Background sys-health worker — reads /sys for temperatures, voltages,
    // currents, power. Universal across host hardware: RPi 5 (full PMIC),
    // RPi 4 (CPU temp), x86 desktop (RAPL + motherboard sensors), laptops
    // (battery). Falls back gracefully if nothing is available.
    if let Some(ref sink) = tsink {
        tetra_entities::sys_telemetry::spawn_sys_health(sink.clone());
        let health = cfg.config().health.clone();
        if health.enabled {
            let (action_sink, action_source) = tetra_entities::health::health_action_channel();
            tetra_entities::health::spawn_health_action_worker(action_source);
            tetra_entities::health::spawn_health_monitor(
                sink.clone(),
                tetra_entities::health::HealthMonitorConfig {
                    snapshot_interval: std::time::Duration::from_secs(health.snapshot_interval_secs),
                    thresholds: tetra_entities::health::HealthThresholds {
                        service_critical_tick_age_ms: health.core_stall_critical_ms,
                    },
                    restart_on_core_stall: health.restart_on_core_stall,
                    restart_after_critical: std::time::Duration::from_secs(health.restart_after_critical_secs),
                    restart_cooldown: std::time::Duration::from_secs(health.restart_cooldown_secs),
                    action_sink: Some(action_sink),
                },
            );
        }
    }

    // Always build control links — dashboard needs them even without external control server
    let (mut c_d, mut c_e) = build_all_control_links();

    // Add remaining components
    let lmac = LmacBs::new(cfg.clone());
    let umac = UmacBs::new(cfg.clone());
    let llc = Llc::new(cfg.clone());
    let mle = MleBs::new(cfg.clone());
    let mm = MmBs::new(cfg.clone(), tsink.clone(), c_e.remove(&TetraEntity::Mm));
    let sndcp = Sndcp::new(cfg.clone());
    let mut cmce = CmceBs::new(cfg.clone(), tsink.clone(), c_e.remove(&TetraEntity::Cmce));
    // Wire the built-in WX/METAR service's reply channel: its background fetch threads
    // re-inject SendSds commands through the CMCE command dispatcher, same as the dashboard.
    if let Some(d) = c_d.get(&TetraEntity::Cmce) {
        cmce.set_wx_cmd_sender(d.clone_sender());
    }
    router.register_entity(Box::new(lmac));
    router.register_entity(Box::new(umac));
    router.register_entity(Box::new(llc));
    router.register_entity(Box::new(mle));
    router.register_entity(Box::new(mm));
    router.register_entity(Box::new(sndcp));
    router.register_entity(Box::new(cmce));

    // Drop all command links that were not given to a TetraEntity
    for (entity, dispatcher) in c_e.into_iter() {
        drop(dispatcher);
        c_d.remove(&entity);
    }

    // Register Brew entity if enabled
    if let Some(ref brew_cfg) = cfg.config().brew {
        let transport = new_websocket_transport(brew_cfg);
        let mut brew_entity = BrewEntity::new(cfg.clone(), transport);
        if let Some(ref sink) = tsink {
            brew_entity.set_telemetry_sink(sink.clone());
        }
        router.register_entity(Box::new(brew_entity));
        eprintln!(" -> Brew/TetraPack integration enabled");
    }

    // Init network time
    router.set_dl_time(TdmaTime::default());

    (router, tsource, c_d)
}

#[derive(Parser, Debug)]
#[command(
    name = "nexus-bs",
    author,
    version,
    about = "Nexus-BS TETRA base station stack",
    long_about = "Runs the Nexus-BS TETRA base station stack using the provided TOML configuration files"
)]

struct Args {
    /// Config file (required)
    #[arg(help = "TOML config with network/cell parameters")]
    config: String,
}

fn main() {
    eprintln!("  _   _                     ____ ____  ");
    eprintln!(" | \\ | | _____  ___   _ ___| __ ) ___| ");
    eprintln!(" |  \\| |/ _ \\ \\/ / | | / __|  _ \\___ \\ ");
    eprintln!(" | |\\  |  __/>  <| |_| \\__ \\ |_) |__) |");
    eprintln!(" |_| \\_|\\___/_/\\_\\\\__,_|___/____/____/ \n");
    eprintln!("  Nexus-BS Project by Chris YO3TCO");
    eprintln!("  Credits: BlueStation, FlowStation, SXCEIVER");
    eprintln!("  Version: {} {}", PRODUCT_NAME, PRODUCT_VERSION_TAG);
    eprintln!("  Build: {}", tetra_core::STACK_VERSION);

    // Parse command-line arguments
    let args = Args::parse();

    // Load config — tries primary, falls back to <config>.fallback if primary is invalid.
    let (stack_cfg, fallback_info) = match load_config_with_fallback(&args.config) {
        ConfigLoadResult::Primary(c) => (c, None),
        ConfigLoadResult::Fallback {
            config,
            fallback_path,
            primary_error,
        } => (config, Some((fallback_path, primary_error))),
    };

    // Build immutable, cheaply clonable SharedConfig and build the base station stack
    let mut stack_state = StackState::default();
    stack_state.subscriber_recovery_path = Some(format!("{}.subscribers", args.config));
    let mut cfg = SharedConfig::from_parts(stack_cfg, Some(stack_state));

    // If dashboard is enabled, set up log capture channel BEFORE logging initialises
    let dashboard_log_rx = if cfg.config().dashboard.is_some() {
        let (tx, rx) = crossbeam_channel::bounded::<(String, String)>(DASHBOARD_LOG_CHANNEL_CAPACITY);
        debug::set_dashboard_log_sender(tx);
        Some(rx)
    } else {
        None
    };

    let _log_guards = debug::setup_logging_default(cfg.config().debug_log.clone());

    // Apply explicit systemd service name from config, if provided.
    // Used by SDS command control (restart/shutdown) and dashboard OTA.
    // Auto-detection from /proc/self/cgroup is still the fallback.
    if let Some(ref service_name) = cfg.config().service_name {
        tetra_entities::service_control::set_configured_service_unit(service_name);
        tracing::info!("Service control: using configured service_name={}", service_name);
    }

    // Log fallback immediately after logging is set up, even without dashboard.
    if let Some((ref fb_path, ref fb_reason)) = fallback_info {
        tracing::warn!(
            "FALLBACK CONFIG ACTIVE: primary config '{}' failed ({}). Running on '{}'.",
            args.config,
            fb_reason,
            fb_path
        );
    }

    let (mut router, tsource, cdispatchers) = build_bs_stack(&mut cfg);

    // Start Telemetry and Control threads, if enabled
    // If dashboard is also enabled, tee the telemetry events to both.
    if let Some(telemetry_source) = tsource {
        let has_telemetry_server = cfg.config().telemetry.is_some();
        let has_dashboard = cfg.config().dashboard.is_some();

        if has_dashboard {
            let mut dash_cfg = cfg.config().dashboard.clone().unwrap();
            dash_cfg.bind = dashboard_core_bind(dash_cfg.bind);
            dash_cfg.port = dashboard_core_port(dash_cfg.port);
            let editable_config_path = dashboard_editable_config_path(&args.config);
            if editable_config_path != args.config {
                tracing::info!(
                    "Dashboard: editing persistent config '{}' while core runs runtime config '{}'",
                    editable_config_path,
                    args.config
                );
            }
            let mut dashboard = DashboardServer::new(editable_config_path);
            dashboard.set_runtime_config_path(args.config.clone());

            // Propagate optional source_dir override for OTA updates.
            dashboard.set_source_dir(dash_cfg.source_dir.clone());
            dashboard.set_static_dir(dashboard_static_dir(dash_cfg.static_dir.clone()));

            // Propagate optional HTTP Basic Auth credentials.
            if let (Some(user), Some(pass)) = (dash_cfg.username.clone(), dash_cfg.password.clone()) {
                tracing::info!("Dashboard: HTTP Basic Auth enabled (user: {})", user);
                dashboard.set_auth(Some((user, pass)));
            }

            // Propagate SharedConfig so the dashboard can read live SDS queue state.
            dashboard.set_shared_config(cfg.clone());

            // Create a control link so dashboard can send commands to CMCE
            let dash_cmd_tx = {
                use tetra_core::tetra_entities::TetraEntity;
                cdispatchers.get(&TetraEntity::Cmce).map(|d| d.clone_sender())
            };

            if let Some(tx) = dash_cmd_tx {
                dashboard.set_cmd_sender(tx);
            }

            // start() must be called before Arc::new() because it takes &mut self
            dashboard.start(&dash_cfg.bind, dash_cfg.port);
            eprintln!(" -> Dashboard enabled on http://{}:{}", dash_cfg.bind, dash_cfg.port);

            // If we started on fallback config, tell the dashboard to show the warning banner.
            if let Some((ref fb_path, ref fb_reason)) = fallback_info {
                let reason = format!(
                    "Primary config '{}' failed to load: {}. Running on fallback '{}'.",
                    args.config, fb_reason, fb_path
                );
                tracing::warn!("{}", reason);
                dashboard.set_fallback_config(reason);
            }

            let dashboard = std::sync::Arc::new(dashboard);
            let dash_clone = std::sync::Arc::clone(&dashboard);

            // Forward log entries to dashboard
            if let Some(log_rx) = dashboard_log_rx {
                let dash_log = std::sync::Arc::clone(&dashboard);
                thread::Builder::new()
                    .name("dashboard-log".into())
                    .spawn(move || {
                        while let Ok((level, msg)) = log_rx.recv() {
                            // Filter out debug/trace noise from dashboard log tab
                            if level == "DEBUG" || level == "TRACE" {
                                continue;
                            }
                            // Filter out TDMA tick noise — thousands per second
                            if msg.contains("tick dl") || msg.contains("tick ul") || msg.starts_with("--- tick") {
                                continue;
                            }
                            dash_log.push_log(&level, msg);
                        }
                    })
                    .expect("failed to spawn dashboard-log thread");
            }

            if has_telemetry_server {
                let cfg2 = cfg.clone();
                let (tee_sink, tee_source) = telemetry_channel();
                thread::Builder::new()
                    .name("telemetry-tee".into())
                    .spawn(move || {
                        loop {
                            match telemetry_source.recv() {
                                Some(event) => {
                                    dash_clone.handle_telemetry(event.clone());
                                    let _ = tee_sink.send(event);
                                }
                                None => break,
                            }
                        }
                    })
                    .expect("failed to spawn telemetry-tee thread");
                start_telemetry_worker(cfg2, tee_source);
            } else {
                thread::Builder::new()
                    .name("telemetry-dash".into())
                    .spawn(move || {
                        loop {
                            match telemetry_source.recv() {
                                Some(event) => dash_clone.handle_telemetry(event),
                                None => break,
                            }
                        }
                    })
                    .expect("failed to spawn telemetry-dash thread");
            }
        } else if has_telemetry_server {
            start_telemetry_worker(cfg.clone(), telemetry_source);
        }
    };

    if cfg.config().control.is_some() {
        start_control_worker(cfg.clone(), cdispatchers);
    };

    // Set up Ctrl+C handler for graceful shutdown.
    // Also installs lifecycle control so RestartService / ShutdownService commands
    // can request shutdown with the correct exit code (75 for restart, signaling
    // systemd to restart us instead of treating it as a normal exit).
    let is_running = Arc::new(AtomicBool::new(true));
    tetra_entities::service_control::install_lifecycle_control(is_running.clone());
    let _watchdog_handle = tetra_entities::service_control::spawn_systemd_watchdog(is_running.clone());
    let is_running_clone = is_running.clone();
    ctrlc::set_handler(move || {
        is_running_clone.store(false, Ordering::SeqCst);
    })
    .expect("failed to set Ctrl+C handler");

    // Start the stack
    tetra_entities::service_control::notify_ready("Nexus-BS stack initialized");
    router.run_stack(None, Some(is_running));
    tetra_entities::service_control::notify_stopping("Nexus-BS stack stopped");

    // router drops here → entities are dropped, networked entities disconnect.
    // If RestartService/ShutdownService was triggered, exit with the requested code
    // so systemd can restart us (exit 75) or stop cleanly (exit 0).
    if let Some(code) = tetra_entities::service_control::requested_exit_code() {
        std::process::exit(code);
    }
}
