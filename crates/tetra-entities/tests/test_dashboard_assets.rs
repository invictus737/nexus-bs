// SPDX-FileCopyrightText: Historical upstream contributors
// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: Apache-2.0 AND PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Modified by Nexus-BS Project; see CHANGES-NEXUS.md for change notices.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("tetra-entities should live under crates/tetra-entities")
        .to_path_buf()
}

#[test]
fn external_dashboard_asset_manifest_is_coherent() {
    let root = workspace_root();
    let index_path = root.join("dashboard/index.html");
    let app_path = root.join("dashboard/assets/app.js");
    let css_path = root.join("dashboard/assets/styles.css");
    let logo_path = root.join("dashboard/assets/nexus-bs-logo.svg");
    let deploy_script_path = root.join("scripts/nexus-bs-test-deploy.sh");
    let deb_postinst_path = root.join("packaging/deb/templates/postinst.in");
    let deb_postrm_path = root.join("packaging/deb/templates/postrm.in");
    let deb_reinstall_path = root.join("scripts/tetrahs-reinstall-nexus-bs.sh");
    let source_install_path = root.join("scripts/install-from-source.sh");
    let dashboard_server_path = root.join("crates/tetra-entities/src/net_dashboard/server.rs");
    let core_unit_path = root.join("contrib/systemd/nexus-bs.service");
    let control_unit_path = root.join("contrib/systemd/nexus-bs-control.service");
    let dashboard_unit_path = root.join("contrib/systemd/nexus-bs-dashboard.service");

    let index = std::fs::read_to_string(&index_path).expect("dashboard index.html should exist");
    let app = std::fs::read_to_string(&app_path).expect("dashboard app.js should exist");
    let css = std::fs::read_to_string(&css_path).expect("dashboard styles.css should exist");
    let deploy_script = std::fs::read_to_string(&deploy_script_path).expect("deploy script should exist");
    let deb_postinst = std::fs::read_to_string(&deb_postinst_path).expect("deb postinst template should exist");
    let deb_postrm = std::fs::read_to_string(&deb_postrm_path).expect("deb postrm template should exist");
    let deb_reinstall = std::fs::read_to_string(&deb_reinstall_path).expect("clean reinstall script should exist");
    let source_install = std::fs::read_to_string(&source_install_path).expect("source installer should exist");
    let dashboard_server = std::fs::read_to_string(&dashboard_server_path).expect("dashboard server source should exist");
    let core_unit = std::fs::read_to_string(&core_unit_path).expect("core systemd unit should exist");
    let control_unit = std::fs::read_to_string(&control_unit_path).expect("control systemd unit should exist");
    let dashboard_unit = std::fs::read_to_string(&dashboard_unit_path).expect("dashboard systemd unit should exist");

    assert!(
        index.contains(r#"<link rel="stylesheet" href="/assets/styles.css">"#),
        "index.html must reference the deploy-copied stylesheet"
    );
    assert!(
        index.contains(r#"<script src="/assets/app.js" defer></script>"#),
        "index.html must reference the deploy-copied application script"
    );
    assert!(logo_path.is_file(), "dashboard vector logo asset must exist on disk");
    assert!(
        index.contains(r#"<img class="brand-logo" src="/assets/nexus-bs-logo.svg" alt="Nexus-BS">"#)
            && !index.contains(r#"class="brand-mark""#)
            && !index.contains(r#"class="brand-name""#)
            && !index.contains(r#"id="buildLabel""#)
            && !app.contains("buildLabel"),
        "brand area must use the vector Nexus-BS lockup without a version label"
    );
    assert!(
        deploy_script.contains("dashboard/assets/nexus-bs-logo.svg"),
        "deploy script must copy the dashboard vector logo asset to the remote static directory"
    );
    assert!(
        index.contains(r#"id="overviewHeard""#),
        "overview page must include the integrated Last Heard table"
    );
    assert!(
        index.contains(r#"class="panel subscriber-registry-panel""#)
            && index.contains(r#"id="radiosTable""#)
            && !index.contains(r#"class="traffic-detail-stack""#)
            && !index.contains(r#"id="callsTable""#)
            && !index.contains(r#"id="heardTable""#)
            && !index.contains("Call Control")
            && !index.contains("Activity Log"),
        "Subscriber Registry must live under System while redundant Traffic Call Control and Activity Log panels stay removed"
    );
    assert!(
        index.contains(r#"class="panel active-calls-panel""#)
            && index.contains(r#"id="overviewCalls""#)
            && !index.contains("Current Floor")
            && !index.contains("Live Radios")
            && !index.contains(r#"id="currentFloorPanel""#)
            && !index.contains(r#"id="overviewRadios""#),
        "Traffic page must make Active Calls the primary board and remove Current Floor/Live Radios overview panels"
    );
    assert!(
        !index.contains(r#"data-page="radios""#)
            && !index.contains(r#"data-page="calls""#)
            && !index.contains(r#"data-page="lastheard""#)
            && !index.contains(r#"id="page-radios""#)
            && !index.contains(r#"id="page-calls""#)
            && !index.contains(r#"id="page-lastheard""#),
        "duplicated traffic workflow tabs must stay consolidated under the Traffic page"
    );
    assert!(
        index.contains(r#"id="rfTxFreq""#)
            && index.contains(r#"id="rfRxFreq""#)
            && index.contains(r#"id="deviceMapPanel""#)
            && index.contains(r#"id="diagramFrequency""#)
            && index.contains(r#"id="slotGrid""#),
        "System/RF Ops and Traffic pages must expose RF, slot, and active-call render targets"
    );
    let timeslots_pos = index.find("<h2>Timeslots</h2>").expect("Timeslots panel should exist");
    let registry_pos = index
        .find(r#"<h2 id="radiosHeading">Subscriber Registry</h2>"#)
        .expect("Subscriber Registry panel should exist");
    let host_pos = index.find("<h2>Host</h2>").expect("Host panel should exist");
    let carrier_pos = index.find("<h2>Carrier Plan</h2>").expect("Carrier Plan panel should exist");
    assert!(
        timeslots_pos < registry_pos && registry_pos < host_pos && timeslots_pos < carrier_pos,
        "System page must show Subscriber Registry directly after Timeslots and before Host/Carrier Plan details"
    );
    assert!(
        index.contains(r#"id="settingsSection""#) && index.contains(r#"id="aboutSection""#),
        "dashboard must keep Settings/Admin and About/Credits sections"
    );
    assert!(
        index.contains(r#"<dt>Project</dt><dd>Nexus-BS Project</dd>"#)
            && index.contains(r#"<dt>Version</dt><dd id="aboutVersion">--</dd>"#),
        "external dashboard About must keep the project name separate from the dynamic version"
    );
    assert!(
        app.contains(r#"setText("aboutVersion", sys.product_version_tag || sys.stack_version || "--")"#)
            && !app.contains("Nexus-BS Project ${sys.product_version_tag} by Chris YO3TCO")
            && !index.contains("Nexus-BS Project by Chris YO3TCO"),
        "external dashboard About version must be populated dynamically without mixing project name and author"
    );
    for link in [
        "https://github.com/invictus737/nexus-bs",
        "https://github.com/misadeks/tetra-bluestation",
        "https://github.com/MidnightBlueLabs/tetra-bluestation",
        "https://github.com/razvanzeces/flowstation",
        "https://sxceiver.com/",
    ] {
        assert!(index.contains(link), "external dashboard About must contain link {link}");
    }
    assert!(
        index.contains("Dennis DB2OE for dashboard theme inspiration"),
        "external dashboard About must credit Dennis DB2OE for dashboard theme inspiration"
    );
    assert!(
        index.contains("Mihajlo YU4MSH") && index.contains("FDX P2P contribution"),
        "external dashboard About must credit Mihajlo YU4MSH separately for FDX P2P"
    );
    assert!(
        index.contains("Tatu Peltola for his") && index.contains(r#"href="https://sxceiver.com/""#) && index.contains(">SXCEIVER</a>"),
        "external dashboard About must credit Tatu Peltola for his hyperlinked SXCEIVER"
    );
    assert!(
        !index.contains("(https://sxceiver.com/)")
            && !index.contains("native Rust Viterbi")
            && !index.contains("rust-soapysdr")
            && !index.contains("Tatu Peltola</a> for SXCEIVER work"),
        "external dashboard About must not show old Tatu wording or a raw SXCEIVER URL"
    );
    assert!(
        index.contains(r#"id="configManager""#) && index.contains(r#"id="configProfileSelect""#) && index.contains(r#"id="configEditor""#),
        "Settings must expose config profile selection and current TOML editing controls"
    );
    assert!(
        index.contains(r#"id="serviceRestartBtn""#)
            && index.contains(r#"id="serviceShutdownBtn""#)
            && index.contains(r#"id="serviceStopGoBtn""#)
            && index.contains(r#"id="factoryResetBtn""#)
            && index.contains(r#"id="factoryResetModal""#)
            && index.contains(r#"id="configDeleteBtn""#),
        "Settings must expose service lifecycle controls and config profile deletion"
    );
    assert!(
        index.contains(r#"id="easyStartModal""#)
            && index.contains(r#"id="easyStartCommitBtn""#)
            && app.contains("function renderEasyStartWizard")
            && app.contains("function verifyEasyStartConfig")
            && app.contains("function requestFactoryReset")
            && app.contains(r#"fetch("/api/easy-start/status""#)
            && app.contains(r#"fetch("/api/easy-start/preview""#)
            && app.contains(r#"fetch("/api/easy-start/commit""#)
            && app.contains(r#"fetch("/api/factory-reset""#),
        "Dashboard must expose beginner Easy Start and confirmed Factory Reset flows"
    );
    assert!(
        index.contains(r#"id="wifiPanel""#)
            && index.contains(r#"id="wifiScanBtn""#)
            && index.contains(r#"id="wifiClearBtn""#)
            && index.contains(r#"id="wifiConnectBtn""#)
            && index.contains(r#"id="wifiShowPassword""#)
            && app.contains("function renderWifiCurrentCard")
            && app.contains("function clearWifiScanList")
            && app.contains("wifiBandLabel")
            && app.contains("wifiNetworkDetail")
            && app.contains(r#"fetch("/api/wifi""#)
            && app.contains(r#"fetchWithTimeout("/api/wifi/scan""#)
            && app.contains(r#"fetchWithTimeout("/api/wifi/connect""#),
        "Settings must expose Wi-Fi scan/select/connect controls backed by dashboard API endpoints"
    );
    assert!(
        index.contains(r#"id="logAutoScrollBtn""#) && index.contains(r#"id="logExportBtn""#) && index.contains(r#"id="logClearBtn""#),
        "Logs page must expose pause/play autoscroll, export, and clear controls"
    );
    assert!(
        !index.contains(r#"id="updateBtn""#),
        "disabled OTA update controls must not be shown in the operator dashboard"
    );
    assert!(
        index.contains(r#"name="nexus-bs-radioid-endpoint""#),
        "dashboard must expose the RadioID endpoint as a configurable document setting"
    );
    assert!(
        index.contains(r#"content="/api/radioid""#),
        "RadioID lookup must use the same-origin dashboard proxy to avoid browser CORS failures"
    );
    assert!(
        app.contains(r#"fetch("/api/system""#),
        "external dashboard must expose the system API"
    );
    assert!(
        app.contains(r#"fetchWithTimeout("/api/site""#) && app.contains("SITE_REFRESH_MS"),
        "external dashboard must expose RF/cell/timeslot state through the site API"
    );
    assert!(
        index.contains(r#"id="networkStatusStrip""#)
            && index.contains(r#"id="networkCoreState""#)
            && index.contains(r#"id="networkCoreHint""#)
            && app.contains("function inferNetworkCore")
            && app.contains("SITE_FETCH_TIMEOUT_MS")
            && app.contains("loadSite({ force: true })")
            && app.contains("state.siteInflight = false")
            && app.contains("TETRAPACK Core")
            && app.contains("TETRALink Core")
            && app.contains("TETRAFlow Core")
            && app.contains("TMO.Services Core"),
        "System status strip must identify the configured Brew network core without exposing credentials"
    );
    assert!(
        app.contains(r#"fetch("/api/snapshot""#) && app.contains("SNAPSHOT_REFRESH_MS"),
        "external dashboard must periodically reconcile active calls from the snapshot API"
    );
    assert!(
        app.contains(r#"fetchDashboardJson("/api/calls""#)
            && app.contains("CALLS_REFRESH_MS = 1000")
            && app.contains("CALLS_FETCH_TIMEOUT_MS"),
        "active calls and speaker state must reconcile once per second through the lightweight calls API with a bounded fetch"
    );
    assert!(
        app.contains("CALLS_FETCH_TIMEOUT_MS")
            && app.contains("function fetchDashboardJson")
            && app.contains("callStartedMsFromPayload")
            && app.contains("reusesEndedCall")
            && app.contains("speakerChanged")
            && app.contains("callerChanged"),
        "active-call polling must not remain blocked and reused TG91 call IDs must reset speaker/start state from snapshots"
    );
    assert!(
        app.contains("setInterval(renderLiveTick, 1000)") && !app.contains("setInterval(renderAll, 1000)"),
        "the browser must not redraw the whole dashboard every second just to tick call duration and uptime"
    );
    assert!(
        !app.contains("renderCurrentFloor")
            && app.contains("activePage: \"system\"")
            && app.contains("pageScroll: new Map()")
            && app.contains("function preserveActivePageScroll")
            && app.contains("SCROLL_INPUT_GRACE_MS")
            && app.contains("history.scrollRestoration = \"manual\"")
            && app.contains("function scheduleScrollRestore")
            && app.contains("function markScrollInput")
            && app.contains("function recentScrollInput")
            && app.contains("Live telemetry updates must not fight manual operator scrolling")
            && app.contains("replace dynamic DOM blocks; preserve the viewport")
            && app.contains("restorePageScroll(page, 0)")
            && app.contains("cancelAnimationFrame(state.scrollRestoreFrame)")
            && app.contains("cancelAnimationFrame(state.scrollRestoreSecondFrame)")
            && app.contains(r#"state.activePage === "logs" && state.logAutoScroll"#),
        "dashboard tab changes and live renders must preserve per-page scroll without fighting fresh operator scroll input"
    );
    assert!(
        app.contains(r#"/ws`"#),
        "browser dashboard must keep using the external dashboard WebSocket endpoint"
    );
    assert!(
        app.contains("RADIOID_MIN_INTERVAL_MS") && app.contains("RADIOID_MAX_QUEUE"),
        "RadioID lookup must be rate-limited and bounded"
    );
    assert!(
        app.contains("BUILTIN_RADIO_IDENTITIES")
            && app.contains(r#""99999""#)
            && app.contains(r#"callsign: "Parrot""#)
            && app.contains("function builtinRadioIdentity"),
        "dashboard must resolve local Parrot ISSI 99999 without external RadioID lookup"
    );
    assert!(
        app.contains("localStorage") && app.contains("RADIOID_CACHE_TTL_MS") && app.contains("nexus-bs.radioid.cache.v2"),
        "RadioID lookup must use a persistent browser cache with a version that can invalidate stale payload shape"
    );
    assert!(
        app.contains("function callMode") && app.contains(r#"label: "group""#),
        "group calls must not be rendered as duplex calls"
    );
    assert!(
        app.contains("function coreHealth") && app.contains("CORE_RECONNECT_GRACE_MS"),
        "Core online/offline status must be debounced across WebSocket reconnects and HTTP health"
    );
    assert!(
        index.contains(r#"id="railConsoleState""#)
            && index.contains(r#"id="railTelemetryState""#)
            && app.contains("railConsoleState")
            && app.contains("railTelemetryState"),
        "rail status must be live data, not static LOCAL/LIVE labels"
    );
    assert!(
        index.contains(r#"id="activeConfigPath""#)
            && index.contains("Active Config")
            && !index.contains("Runtime Config")
            && !index.contains(r#"id="runtimeConfigPath""#)
            && !app.contains("runtime_config_path")
            && index.contains("Config Store")
            && app.contains("active_config_name")
            && app.contains("active_config_path"),
        "Host panel must show the selected active profile without separate runtime config state"
    );
    assert!(
        app.contains("function setIndustrialTone")
            && app.contains("diagramRfState")
            && app.contains("diagramBrewState")
            && app.contains("diagramPhyState")
            && app.contains("requestRfCarrierToggle")
            && app.contains("effectiveCarrierInhibited")
            && app.contains("rfCarrierPendingInhibited")
            && app.contains("Scan list")
            && index.contains("Groups / Scan List")
            && app.contains(r#"fetchWithTimeout("/api/rf/carrier""#)
            && app.contains("COMMAND_FETCH_TIMEOUT_MS")
            && index.contains(r#"id="nodePhy""#)
            && index.contains(r#"id="diagramPathToggle""#)
            && index.contains(r#"<button type="button" class="path-toggle"#)
            && css.contains("brand-logo")
            && css.contains("subscriber-groups")
            && css.contains("group-chips")
            && css.contains("device-map")
            && css.contains("minmax(190px, 220px)")
            && css.contains("text-wrap: balance"),
        "dashboard must keep the industrial device-map view wired to live status, RF carrier control, and readable component node titles"
    );
    assert!(
        app.contains(r#"fetch("/api/configs""#)
            && app.contains(r#"fetch("/api/configs/activate""#)
            && app.contains("method: \"DELETE\"")
            && app.contains("function duplicateSelectedConfig"),
        "dashboard config manager must list, activate, and duplicate flat TOML config profiles"
    );
    assert!(
        app.contains(r#"restart: "/api/service/restart""#)
            && app.contains(r#"shutdown: "/api/service/shutdown""#)
            && app.contains(r#"stopgo: "/api/service/stop-go""#)
            && app.contains("function requestServiceAction"),
        "dashboard must call the core-owned service lifecycle API"
    );
    assert!(
        deploy_script.contains(r#"REMOTE_SERVICE="${REMOTE_SERVICE:-nexus-bs.service}""#)
            && deploy_script.contains(r#"REMOTE_CONTROL_SERVICE="${REMOTE_CONTROL_SERVICE:-nexus-bs-control.service}""#)
            && deploy_script.contains(r#"REMOTE_DASHBOARD_SERVICE="${REMOTE_DASHBOARD_SERVICE:-nexus-bs-dashboard.service}""#)
            && !deploy_script.contains("nexus-bs@.service")
            && !deploy_script.contains("nexus-bs-control@.service")
            && !deploy_script.contains("nexus-bs-dashboard@.service"),
        "deploy script must use simple service names, not systemd template units"
    );
    assert!(
        core_unit.contains("Environment=NEXUS_BS_SERVICE_UNIT=nexus-bs.service")
            && !core_unit.contains("NEXUS_BS_CORE_DASHBOARD")
            && !dashboard_unit.contains("NEXUS_BS_DASHBOARD_CORE")
            && dashboard_unit.contains("Environment=NEXUS_BS_DASHBOARD_TELEMETRY_LISTEN=127.0.0.1:9001")
            && dashboard_unit.contains("Environment=NEXUS_BS_DASHBOARD_CONTROL_URL=http://127.0.0.1:9003/command")
            && control_unit.contains("--command-listen 127.0.0.1:9003")
            && !core_unit.contains("@%i")
            && !control_unit.contains("@%i")
            && !dashboard_unit.contains("@%i"),
        "split systemd units must run dashboard API externally and avoid @user service names"
    );
    assert!(
        core_unit.contains("CPUAffinity=1 2") && control_unit.contains("CPUAffinity=3") && dashboard_unit.contains("CPUAffinity=3"),
        "systemd split deployment must pin RF core to CPU 1+2 and dashboard/control to CPU 3"
    );
    assert!(
        core_unit.contains("CPUSchedulingPolicy=rr")
            && core_unit.contains("CPUSchedulingPriority=80")
            && core_unit.contains("LimitRTPRIO=80"),
        "RF core must run with systemd RT scheduling equivalent to chrt -r 80"
    );
    assert!(
        dashboard_server.contains(r#"run_update_command_privileged(&update, "apt-get", &["install", "-y", deb_path_str])"#)
            && dashboard_server.contains("run_update_post_install_restart")
            && dashboard_server.contains("same action as Settings/Admin Restart BS")
            && deb_postinst.contains("install_dashboard_sudoers")
            && deb_postinst.contains("/usr/bin/apt-get install -y /tmp/nexus-bs-update/nexus-bs_*.deb")
            && deb_postinst.contains("/usr/bin/systemctl restart nexus-bs-dashboard.service")
            && deb_postinst.contains("/usr/bin/systemctl --no-block poweroff")
            && deb_postrm.contains("rm -f /etc/sudoers.d/nexus-bs-dashboard")
            && dashboard_unit.contains("NoNewPrivileges=no")
            && !dashboard_unit.contains("NoNewPrivileges=yes"),
        "dashboard package update, downgrade, and factory reset need a narrow sudoers path and must not be blocked by NoNewPrivileges"
    );
    assert!(
        deb_postinst.contains("detect_run_user")
            && deb_postinst.contains("install_service_user_dropins")
            && deb_postinst.contains("User=%s")
            && deb_postinst.contains("Group=%s")
            && deb_postinst.contains("chown -R \"$run_user:$run_group\" /etc/nexus-bs")
            && deb_postinst.contains("find /etc/nexus-bs -type f -name '*.toml' -exec chmod 0600")
            && deb_postinst.contains("find /etc/nexus-bs -type f -name '*.toml.active' -exec chmod 0600")
            && !deb_postinst.contains("chown root:root /etc/nexus-bs")
            && !deb_postinst.contains("install -d -o root -g root -m 0755 /etc/nexus-bs")
            && deb_reinstall.contains("RUN_USER")
            && deb_reinstall.contains("chown -R \"$RUN_USER:$RUN_GROUP\" /etc/nexus-bs")
            && !deb_reinstall.contains("chown root:root /etc/nexus-bs")
            && source_install.contains("User=${RUN_USER}")
            && source_install.contains("Group=${RUN_GROUP}"),
        "Debian install and recovery flows must keep /etc/nexus-bs owned by the runtime user so dashboard config writes, calibration, profile duplicate, and restart flows work"
    );
    assert!(
        app.contains(r#"fetch("/api/logs/clear""#)
            && app.contains("function exportLogs")
            && app.contains("Log${logTimestampForFile()}.log")
            && app.contains("logAutoScroll"),
        "dashboard logs must support clear, export, and pause/play autoscroll"
    );
    assert!(
        app.contains("function callAgeSeconds") && app.contains("data-call-seconds") && app.contains("Call time"),
        "call duration display must be a live seconds counter"
    );
    assert!(
        app.contains("const MCC_TO_ISO")
            && app.contains("function flagForIso")
            && app.contains("function callCountryHtml")
            && app.contains("function callCountryCandidates")
            && app.contains("recentGroupSpeakerForCall(call)")
            && app.contains("function normalizedSpeakerIssi")
            && app.contains(r#"Number(call.gssi || 0) === issi"#)
            && app.contains("function countryByRadioId")
            && app.contains("const radioIdCountry = countryByRadioId(value)")
            && app.contains("payload.country")
            && app.contains("function activeCallIdentityHtml")
            && app.contains("function callCardPartyHtml")
            && app.contains("function instantSpeakerHtml")
            && app.contains("ISSI ${esc(issiLabel || \"--\")}")
            && app.contains("group-call-grid")
            && !app.contains("<span>Caller</span>")
            && app.contains("case \"speaker_changed\"")
            && app.contains("202: \"GR\"")
            && app.contains("226: \"RO\"")
            && app.contains("750: \"FK\"")
            && css.contains(".active-call-board")
            && css.contains(".call-country")
            && css.contains(".call-ts")
            && css.contains(".call-identity-callsign")
            && css.contains(".call-card-grid > div > span")
            && css.contains(".speaker-issi"),
        "active calls must render aligned country flag/code, TS, unified group speaker identity, and high-legibility radio identity text"
    );
    assert!(
        app.contains("GROUP_CALL_HANGTIME_UI_MS") && app.contains("function endCall"),
        "group calls must remain visible briefly through hangtime so speaker-change events can update the row"
    );
    assert!(
        app.contains("function callInHangtime") && app.contains("last speaker"),
        "hangtime rows must not be counted or rendered as a current active speaker"
    );
    assert!(
        app.contains("const currentCalls = activeCalls()")
            && app.contains("const overview = currentCalls.map")
            && app.contains("caller_issi: msg.speaker_issi"),
        "the primary Active Calls board must not render stale hangtime calls, and speaker changes must refresh the operational caller ISSI"
    );
    assert!(
        app.contains("preserveHangtime") && app.contains("retainedHangtimeCalls"),
        "snapshot reconciliation must not erase locally retained group-call hangtime rows"
    );
    assert!(
        app.contains("function refreshCallIdentities") && app.contains("queueRadioIdRefresh"),
        "active call identities must be retried after QSO start/speaker changes"
    );
    assert!(
        index.contains(r#"id="bsUptime""#) && app.contains("bs_uptime_secs"),
        "system page must show Nexus-BS process uptime, not only host uptime"
    );
    assert!(
        index.contains(r#"id="hostUptime""#) && app.contains("host_uptime_secs"),
        "system page must label host uptime separately"
    );
    assert!(
        !css.contains("letter-spacing: -"),
        "dashboard CSS should not use negative letter spacing"
    );
    assert!(
        css.contains("overflow-anchor: none")
            && css.contains(".page:not(.active)")
            && css.contains("--dash-page-pad")
            && !css.contains("Legacy structural baseline"),
        "dashboard CSS must use one explicit page visibility contract and disable scroll anchoring on live panels"
    );
    assert!(
        css.contains("@media (pointer: coarse)") && css.contains("@media (min-resolution: 2dppx)"),
        "dashboard CSS must include touch-target and high-DPI density handling"
    );
    for (line_no, line) in css.lines().enumerate() {
        assert!(
            !(line.contains("font-size") && line.contains("vw")),
            "dashboard CSS must not scale font size from viewport width at line {}: {}",
            line_no + 1,
            line
        );
    }
}
