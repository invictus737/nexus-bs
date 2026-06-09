const state = {
  radios: new Map(),
  calls: new Map(),
  heard: [],
  logs: [],
  brewOnline: false,
  brewVersion: 0,
  connected: false,
  system: null,
};

const pages = {
  overview: "Overview",
  radios: "Radios",
  calls: "Calls",
  lastheard: "Last Heard",
  logs: "Logs",
  system: "System",
};

function $(id) {
  return document.getElementById(id);
}

function esc(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function fmtAge(seconds) {
  const s = Math.max(0, Math.floor(seconds || 0));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

function radioLastSeen(radio) {
  if (radio._lastSeenMs) {
    return fmtAge((Date.now() - radio._lastSeenMs) / 1000);
  }
  return fmtAge(radio.last_seen_secs_ago || 0);
}

function eeLabel(radio) {
  const mode = radio.energy_saving_mode || 0;
  if (!mode) return "StayAlive";
  return `EG${mode}`;
}

function groupLabel(groups) {
  if (!groups || !groups.length) return '<span class="empty">No Group</span>';
  return groups.map((g) => `<span class="pill blue">${esc(g)}</span>`).join(" ");
}

function rssiLabel(value) {
  if (value === null || value === undefined) return '<span class="empty">--</span>';
  const cls = value > -40 ? "green" : value > -70 ? "amber" : "red";
  return `<span class="pill ${cls}">${Number(value).toFixed(1)}</span>`;
}

function rowsOrEmpty(rows, cols, label) {
  if (rows.length) return rows.join("");
  return `<tr><td colspan="${cols}" class="empty">${esc(label)}</td></tr>`;
}

function sortedRadios() {
  return Array.from(state.radios.values()).sort((a, b) => a.issi - b.issi);
}

function sortedCalls() {
  return Array.from(state.calls.values()).sort((a, b) => a.call_id - b.call_id);
}

function renderStatus() {
  $("coreState").textContent = state.connected ? "ONLINE" : "OFFLINE";
  $("coreState").classList.toggle("ok", state.connected);
  $("brewState").textContent = state.brewOnline ? `BREW v${state.brewVersion || 1}` : "OFFLINE";
  $("brewState").classList.toggle("ok", state.brewOnline);
}

function renderMetrics() {
  $("metricRadios").textContent = state.radios.size;
  $("metricCalls").textContent = state.calls.size;
  $("metricHeard").textContent = state.heard.length;
  $("metricCpu").textContent = state.system?.cpu_pct !== undefined ? `${state.system.cpu_pct}%` : "--";
  $("radioCountLabel").textContent = `${state.radios.size} online`;
  $("callCountLabel").textContent = `${state.calls.size} active`;
}

function renderRadios() {
  const rows = sortedRadios().map((radio) => `
    <tr>
      <td>${esc(radio.issi)}</td>
      <td>${groupLabel(radio.groups)}</td>
      <td>${rssiLabel(radio.rssi_dbfs)}</td>
      <td><span class="pill ${radio.energy_saving_mode ? "amber" : "green"}">${esc(eeLabel(radio))}</span></td>
      <td>${esc(radioLastSeen(radio))}</td>
    </tr>
  `);
  const html = rowsOrEmpty(rows, 5, "No registered radios");
  $("overviewRadios").innerHTML = html;
  $("radiosTable").innerHTML = html;
}

function renderCalls() {
  const overview = sortedCalls().map((call) => `
    <tr>
      <td>${esc(call.call_id)}</td>
      <td>${esc(call.call_type || "call")}</td>
      <td>${esc(call.gssi || call.called_issi || "--")}</td>
      <td>${esc(call.active_speaker || "--")}</td>
      <td>${esc(call.ts || "--")}</td>
    </tr>
  `);
  $("overviewCalls").innerHTML = rowsOrEmpty(overview, 5, "No active calls");

  const rows = sortedCalls().map((call) => `
    <tr>
      <td>${esc(call.call_id)}</td>
      <td><span class="pill ${call.simplex ? "amber" : "green"}">${esc(call.simplex ? "simplex" : "duplex")}</span></td>
      <td>${esc(call.gssi || "--")}</td>
      <td>${esc(call.caller_issi || "--")}</td>
      <td>${esc(call.called_issi || "--")}</td>
      <td>${esc(call.active_speaker || "--")}</td>
      <td>${esc(fmtAge(call.started_secs_ago || ((Date.now() - (call._startedMs || Date.now())) / 1000)))}</td>
      <td>${esc(call.ts || "--")}</td>
    </tr>
  `);
  $("callsTable").innerHTML = rowsOrEmpty(rows, 8, "No active calls");
}

function renderHeard() {
  const rows = state.heard.slice(0, 80).map((entry) => `
    <tr>
      <td>${esc(entry.ts || "--")}</td>
      <td>${esc(entry.issi || "--")}</td>
      <td>${esc(entry.activity || "--")}</td>
      <td>${esc(entry.dest || "--")}</td>
    </tr>
  `);
  $("heardTable").innerHTML = rowsOrEmpty(rows, 4, "No recent activity");
}

function renderLogs() {
  $("logCount").textContent = `${state.logs.length} lines`;
  const rows = state.logs.slice(-500).reverse().map((entry) => {
    const level = String(entry.level || "INFO").toLowerCase();
    const cls = level.includes("error") ? "error" : level.includes("warn") ? "warn" : "";
    return `<div class="log-row ${cls}">
      <span>${esc(entry.ts || "--")}</span>
      <span class="level">${esc(entry.level || "INFO")}</span>
      <span class="msg">${esc(entry.msg || "")}</span>
    </div>`;
  });
  $("logList").innerHTML = rows.length ? rows.join("") : '<div class="log-row"><span>--</span><span class="level">INFO</span><span class="msg">No log entries</span></div>';
}

function renderSystem() {
  const sys = state.system || {};
  $("buildLabel").textContent = sys.product_version_tag || "--";
  $("subtitle").textContent = sys.stack_version || "Nexus-BS local TETRA base station";
  $("hostName").textContent = sys.hostname || "--";
  $("productName").textContent = sys.product_name && sys.product_version_tag ? `${sys.product_name} ${sys.product_version_tag}` : "--";
  $("cpuName").textContent = sys.cpu_model ? `${sys.cpu_model}${sys.cpu_cores ? ` (${sys.cpu_cores} cores)` : ""}` : "--";
  $("memoryUse").textContent = sys.ram_total_mb ? `${sys.ram_used_mb || 0} / ${sys.ram_total_mb} MB` : "--";
  $("cpuTemp").textContent = sys.cpu_temp_c !== null && sys.cpu_temp_c !== undefined ? `${Number(sys.cpu_temp_c).toFixed(1)} C` : "--";
  $("configPath").textContent = sys.config_path || "--";
  $("runtimeConfigPath").textContent = sys.runtime_config_path || sys.config_path || "--";
  $("sdrName").textContent = sys.sdr_name || "--";
  $("soapyInfo").textContent = sys.soapy_info || "--";
}

function renderAll() {
  renderStatus();
  renderMetrics();
  renderRadios();
  renderCalls();
  renderHeard();
  renderLogs();
  renderSystem();
}

function applySnapshot(msg) {
  state.radios.clear();
  state.calls.clear();
  state.heard = msg.last_heard || [];
  state.logs = msg.log || [];
  for (const radio of msg.ms || []) {
    state.radios.set(radio.issi, { ...radio, _lastSeenMs: Date.now() - (radio.last_seen_secs_ago || 0) * 1000 });
  }
  for (const call of msg.calls || []) {
    state.calls.set(call.call_id, { ...call, _startedMs: Date.now() - (call.started_secs_ago || 0) * 1000 });
  }
  state.brewOnline = !!msg.brew_online;
  state.brewVersion = msg.brew_version || 0;
}

function ensureRadio(issi) {
  if (!state.radios.has(issi)) {
    state.radios.set(issi, { issi, groups: [], rssi_dbfs: null, energy_saving_mode: 0, last_seen_secs_ago: 0 });
  }
  return state.radios.get(issi);
}

function pushHeard(entry) {
  if (!entry) return;
  state.heard.unshift(entry);
  state.heard = state.heard.slice(0, 80);
}

function applyMessage(msg) {
  switch (msg.type) {
    case "snapshot":
      applySnapshot(msg);
      break;
    case "brew_status":
      state.brewOnline = !!msg.connected;
      state.brewVersion = msg.brew_version || state.brewVersion;
      break;
    case "ms_registered":
      ensureRadio(msg.issi)._lastSeenMs = Date.now();
      break;
    case "ms_deregistered":
      state.radios.delete(msg.issi);
      break;
    case "ms_rssi":
      Object.assign(ensureRadio(msg.issi), { rssi_dbfs: msg.rssi_dbfs, _lastSeenMs: Date.now() });
      break;
    case "ms_groups":
    case "ms_groups_all":
      Object.assign(ensureRadio(msg.issi), { groups: msg.groups || [], _lastSeenMs: Date.now() });
      break;
    case "ms_groups_detach": {
      const radio = ensureRadio(msg.issi);
      const remove = new Set(msg.groups || []);
      radio.groups = (radio.groups || []).filter((group) => !remove.has(group));
      radio._lastSeenMs = Date.now();
      break;
    }
    case "ms_energy_saving":
      Object.assign(ensureRadio(msg.issi), {
        energy_saving_mode: msg.mode || 0,
        energy_saving_frame: msg.frame ?? null,
        energy_saving_multiframe: msg.multiframe ?? null,
        _lastSeenMs: Date.now(),
      });
      break;
    case "call_started":
      state.calls.set(msg.call_id, { ...msg, _startedMs: Date.now() });
      pushHeard(msg.last_heard);
      break;
    case "speaker_changed":
      if (state.calls.has(msg.call_id)) state.calls.get(msg.call_id).active_speaker = msg.speaker_issi;
      pushHeard(msg.last_heard);
      break;
    case "call_ended":
      state.calls.delete(msg.call_id);
      break;
    case "last_heard":
      pushHeard(msg.entry || msg);
      break;
    case "log":
      state.logs.push({ ts: msg.ts, level: msg.level, msg: msg.msg });
      state.logs = state.logs.slice(-500);
      break;
    default:
      break;
  }
  renderAll();
}

async function loadSystem() {
  try {
    const res = await fetch("/api/system", { credentials: "same-origin" });
    if (!res.ok) return;
    state.system = await res.json();
    renderAll();
  } catch {
    // Keep last known values.
  }
}

function connectWs() {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const ws = new WebSocket(`${proto}//${location.host}/ws`);

  ws.addEventListener("open", () => {
    state.connected = true;
    renderAll();
  });

  ws.addEventListener("message", (event) => {
    try {
      applyMessage(JSON.parse(event.data));
    } catch {
      // Ignore malformed dashboard frames.
    }
  });

  ws.addEventListener("close", () => {
    state.connected = false;
    renderAll();
    setTimeout(connectWs, 1500);
  });

  ws.addEventListener("error", () => {
    state.connected = false;
    renderAll();
  });
}

function switchPage(page) {
  for (const node of document.querySelectorAll(".page")) node.classList.remove("active");
  for (const node of document.querySelectorAll(".nav-item")) node.classList.toggle("active", node.dataset.page === page);
  $(`page-${page}`)?.classList.add("active");
  $("pageTitle").textContent = pages[page] || "Overview";
}

function initNav() {
  for (const node of document.querySelectorAll(".nav-item")) {
    node.addEventListener("click", () => switchPage(node.dataset.page));
  }
  $("refreshBtn").addEventListener("click", loadSystem);
}

initNav();
loadSystem();
connectWs();
setInterval(loadSystem, 15000);
setInterval(renderAll, 1000);
