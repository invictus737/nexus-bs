// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0

const state = {
  radios: new Map(),
  calls: new Map(),
  callCleanupTimers: new Map(),
  heard: [],
  logs: [],
  site: null,
  txVisual: null,
  txQuality: null,
  sdrHealth: null,
  sysHealth: null,
  slotActivity: new Map(),
  brewOnline: false,
  brewVersion: 0,
  connected: false,
  wsConnected: false,
  wsState: "connecting",
  lastHttpOkMs: 0,
  lastHttpFailMs: 0,
  httpFailureCount: 0,
  lastWsOpenMs: 0,
  lastWsMessageMs: 0,
  lastWsCloseMs: 0,
  system: null,
  snapshotInflight: false,
  callsInflight: false,
  siteInflight: false,
  siteReloadingUntilMs: 0,
  callsPayloadKey: "",
  configProfiles: [],
  configProfilesLoaded: false,
  configEditorName: "config.toml",
  configEditorLoadedName: "",
  configEditorContent: "",
  configEditorDirty: false,
  configEditorStatus: "idle",
  configBusy: false,
  rfCarrierBusy: false,
  rfCarrierPendingInhibited: null,
  rfCarrierPendingUntilMs: 0,
  rfProfileApplyBusy: false,
  calibration: null,
  calibrationBusy: false,
  wifi: null,
  wifiBusy: false,
  wifiSelectedSsid: "",
  wifiScanVisible: false,
  wifiActionStatus: "idle",
  updateCatalog: null,
  updateBusy: false,
  updateStatus: "checking",
  updateLog: "",
  selectedUpdateUrl: "",
  serviceBusy: false,
  serviceStatus: "idle",
  easyStart: null,
  easyStartStep: 0,
  easyStartBusy: false,
  easyStartDraft: null,
  easyStartPreview: null,
  easyStartDismissed: false,
  factoryResetBusy: false,
  logAutoScroll: true,
  logBusy: false,
  activePage: "system",
};

const radioId = {
  cache: new Map(),
  queue: [],
  queued: new Set(),
  inflight: false,
  lastRequestMs: 0,
  failureUntil: new Map(),
  saveTimer: null,
};

const RADIOID_CACHE_KEY = "nexus-bs.radioid.cache.v2";
const RADIOID_CACHE_TTL_MS = 7 * 24 * 60 * 60 * 1000;
const RADIOID_NEGATIVE_TTL_MS = 24 * 60 * 60 * 1000;
const RADIOID_RETRY_MS = 30 * 1000;
const RADIOID_MIN_INTERVAL_MS = 2500;
const RADIOID_MAX_CACHE = 500;
const RADIOID_MAX_QUEUE = 64;
const RADIOID_FETCH_TIMEOUT_MS = 6000;
const BUILTIN_RADIO_IDENTITIES = new Map([
  ["99999", { issi: 99999, callsign: "Parrot", name: "", country: "", missing: false, builtin: true, fetchedAt: Number.MAX_SAFE_INTEGER }],
]);
const GROUP_CALL_HANGTIME_UI_MS = 12000;
const CALLS_REFRESH_MS = 1000;
const CALLS_FETCH_TIMEOUT_MS = 2500;
const SNAPSHOT_REFRESH_MS = 10000;
const SITE_REFRESH_MS = 10000;
const SITE_FETCH_TIMEOUT_MS = 3500;
const COMMAND_FETCH_TIMEOUT_MS = 5000;
const CORE_ONLINE_GRACE_MS = 5000;
const CORE_RECONNECT_GRACE_MS = 12000;
const SLOT_ACTIVITY_MS = 2000;

const pages = {
  system: "System",
  traffic: "Traffic",
  settings: "Settings",
  logs: "Logs",
  about: "About",
};

function $(id) {
  return document.getElementById(id);
}

function setText(id, value) {
  const node = $(id);
  if (node) node.textContent = value ?? "--";
}

function setHtml(id, value) {
  const node = $(id);
  const html = value ?? "";
  if (node && node.innerHTML !== html) node.innerHTML = html;
}

function setClass(id, className, enabled) {
  const node = $(id);
  if (node) node.classList.toggle(className, !!enabled);
}

function setStatusTone(id, tone) {
  const node = $(id);
  if (!node) return;
  node.classList.toggle("status-ok", tone === "ok");
  node.classList.toggle("status-warn", tone === "warn");
  node.classList.toggle("status-bad", tone === "bad");
}

function setIndustrialTone(id, tone) {
  const node = $(id);
  if (!node) return;
  for (const cls of ["is-ok", "is-warn", "is-bad", "is-idle", "is-on"]) node.classList.remove(cls);
  node.classList.add(`is-${tone || "idle"}`);
}

function compactHostLabel(host) {
  return String(host || "")
    .trim()
    .replace(/^wss?:\/\//i, "")
    .replace(/^https?:\/\//i, "")
    .replace(/\/.*$/, "")
    .replace(/:\d+$/, "");
}

function inferNetworkCore() {
  if (!state.site && Date.now() < state.siteReloadingUntilMs) {
    return { label: "Reloading", hint: "waiting for active config", tone: "warn" };
  }
  const brew = state.site?.config?.brew || {};
  const host = compactHostLabel(brew.host);
  const haystack = host.toLowerCase();
  let label = "";

  if (haystack.includes("tetrapack")) {
    label = "TETRAPACK Core";
  } else if (haystack.includes("tetralink") || haystack.includes("tetra-link")) {
    label = "TETRALink Core";
  } else if (haystack.includes("tetraflow") || haystack.includes("flowstation") || haystack.includes("tetra-flow")) {
    label = "TETRAFlow Core";
  } else if (haystack.includes("tmo.services") || haystack.includes("tmoservices") || haystack.includes("tmo-services")) {
    label = "TMO.Services Core";
  } else if (haystack.includes("brandmeister")) {
    label = "BrandMeister Core";
  } else if (brew.configured || state.brewOnline) {
    label = "Brew Core";
  } else {
    label = "Local Core";
  }

  const hint = host
    ? `${host}${brew.port ? `:${brew.port}` : ""}`
    : state.brewOnline
      ? `BREW v${state.brewVersion || 1}`
      : "local routing";
  const tone = state.brewOnline || !brew.configured ? "ok" : "warn";
  return { label, hint, tone };
}

function esc(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

async function fetchDashboardJson(url, options = {}, timeoutMs = CALLS_FETCH_TIMEOUT_MS) {
  const res = await fetchWithTimeout(url, options, timeoutMs);
  if (!res.ok) throw new Error(`${url} ${res.status}`);
  return await res.json();
}

async function fetchWithTimeout(url, options = {}, timeoutMs = COMMAND_FETCH_TIMEOUT_MS) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(url, { ...options, signal: controller.signal });
  } finally {
    clearTimeout(timeout);
  }
}

function markHttpOk() {
  state.lastHttpOkMs = Date.now();
  state.httpFailureCount = 0;
}

function markHttpFail() {
  state.lastHttpFailMs = Date.now();
  state.httpFailureCount += 1;
}

function markWsMessage() {
  state.lastWsMessageMs = Date.now();
  state.wsState = "open";
}

function coreHealth() {
  const now = Date.now();
  const lastOk = Math.max(state.lastHttpOkMs || 0, state.lastWsOpenMs || 0, state.lastWsMessageMs || 0);
  if (state.wsConnected || now - lastOk <= CORE_ONLINE_GRACE_MS) {
    return { label: "ONLINE", className: "ok" };
  }
  if (lastOk && now - lastOk <= CORE_RECONNECT_GRACE_MS) {
    return { label: "RECONNECTING", className: "warn" };
  }
  return { label: "OFFLINE", className: "bad" };
}

function radioIdEndpoint() {
  return document.querySelector('meta[name="nexus-bs-radioid-endpoint"]')?.content?.trim() || "";
}

function validLookupIssi(issi) {
  const id = Number(issi);
  return Number.isInteger(id) && id > 0 && id < 0x00ffffff;
}

function normalizeIssi(issi) {
  return String(Number(issi));
}

function validRadioIdCacheEntry(entry) {
  if (!entry) return false;
  if (entry.builtin) return true;
  const ttl = entry.missing ? RADIOID_NEGATIVE_TTL_MS : RADIOID_CACHE_TTL_MS;
  return Date.now() - Number(entry.fetchedAt || 0) <= ttl;
}

function builtinRadioIdentity(issi) {
  if (!validLookupIssi(issi)) return null;
  return BUILTIN_RADIO_IDENTITIES.get(normalizeIssi(issi)) || null;
}

function loadRadioIdCache() {
  try {
    const raw = localStorage.getItem(RADIOID_CACHE_KEY);
    if (!raw) return;
    const parsed = JSON.parse(raw);
    const entries = Array.isArray(parsed?.entries) ? parsed.entries : [];
    const now = Date.now();
    for (const entry of entries) {
      if (!validLookupIssi(entry?.issi)) continue;
      const ttl = entry.missing ? RADIOID_NEGATIVE_TTL_MS : RADIOID_CACHE_TTL_MS;
      if (now - Number(entry.fetchedAt || 0) > ttl) continue;
      radioId.cache.set(normalizeIssi(entry.issi), {
        issi: Number(entry.issi),
        callsign: String(entry.callsign || "").trim(),
        name: String(entry.name || "").trim(),
        country: String(entry.country || "").trim(),
        missing: !!entry.missing,
        fetchedAt: Number(entry.fetchedAt || now),
      });
    }
  } catch {
    radioId.cache.clear();
  }
}

function pruneRadioIdCache() {
  const now = Date.now();
  for (const [issi, entry] of radioId.cache) {
    const ttl = entry.missing ? RADIOID_NEGATIVE_TTL_MS : RADIOID_CACHE_TTL_MS;
    if (now - Number(entry.fetchedAt || 0) > ttl) radioId.cache.delete(issi);
  }
  if (radioId.cache.size <= RADIOID_MAX_CACHE) return;
  const keep = Array.from(radioId.cache.entries())
    .sort((a, b) => Number(b[1].fetchedAt || 0) - Number(a[1].fetchedAt || 0))
    .slice(0, RADIOID_MAX_CACHE);
  radioId.cache = new Map(keep);
}

function scheduleRadioIdCacheSave() {
  clearTimeout(radioId.saveTimer);
  radioId.saveTimer = setTimeout(() => {
    try {
      pruneRadioIdCache();
      localStorage.setItem(
        RADIOID_CACHE_KEY,
        JSON.stringify({ version: 1, entries: Array.from(radioId.cache.values()) })
      );
    } catch {
      // Browser storage can be unavailable or full; lookup remains best-effort.
    }
  }, 750);
}

function radioIdDisplay(entry) {
  if (!entry || entry.missing || !entry.callsign) return "";
  return entry.name ? `${entry.callsign} - ${entry.name}` : entry.callsign;
}

function radioIdentityHtml(issi) {
  if (!validLookupIssi(issi)) return `<span class="identity-issi">${esc(issi || "--")}</span>`;
  const builtin = builtinRadioIdentity(issi);
  if (builtin) {
    const key = normalizeIssi(issi);
    return `<span class="identity resolved builtin"><span class="identity-primary">${esc(radioIdDisplay(builtin) || builtin.callsign)}</span><span class="identity-issi">ISSI ${esc(key)}</span></span>`;
  }
  const endpoint = radioIdEndpoint();
  if (!endpoint) return `<span class="identity-issi">${esc(issi)}</span>`;
  const key = normalizeIssi(issi);
  const entry = radioId.cache.get(key);
  if (entry && validRadioIdCacheEntry(entry)) {
    const label = radioIdDisplay(entry);
    if (label) {
      return `<span class="identity resolved"><span class="identity-primary">${esc(label)}</span><span class="identity-issi">${esc(key)}</span></span>`;
    }
    return `<span class="identity unresolved"><span class="identity-primary">${esc(key)}</span><span class="identity-issi">not found</span></span>`;
  }
  if (entry) {
    radioId.cache.delete(key);
    scheduleRadioIdCacheSave();
  }
  queueRadioIdLookup(issi);
  const failureUntil = Number(radioId.failureUntil.get(key) || 0);
  const status = failureUntil > Date.now() ? "lookup retrying" : radioId.queued.has(key) ? "queued" : "pending";
  return `<span class="identity pending"><span class="identity-primary">${esc(key)}</span><span class="identity-issi">${esc(status)}</span></span>`;
}

function activeCallIdentityHtml(issi, options = {}) {
  const key = normalizeIssi(issi);
  const speakerClass = options.speaker ? " speaker-now" : "";
  const mutedClass = options.muted ? " muted" : "";
  const issiClass = options.speaker ? "call-identity-issi speaker-issi" : "call-identity-issi";
  const render = (stateClass, callsign, name, issiLabel = key) => `
    <span class="call-identity ${esc(stateClass)}${speakerClass}${mutedClass}">
      <span class="call-identity-callsign">${esc(callsign || "--")}</span>
      <span class="call-identity-name">${esc(name || "RadioID pending")}</span>
      <span class="${issiClass}">ISSI ${esc(issiLabel || "--")}</span>
    </span>
  `;

  if (!validLookupIssi(issi)) return render("unresolved", "--", "invalid identity", issi || "--");

  const builtin = builtinRadioIdentity(issi);
  if (builtin) return render("resolved builtin", builtin.callsign || radioIdDisplay(builtin), builtin.name || "Built-in service", key);

  const endpoint = radioIdEndpoint();
  if (!endpoint) return render("unresolved", key, "RadioID disabled", key);

  const entry = radioId.cache.get(key);
  if (entry && validRadioIdCacheEntry(entry)) {
    if (entry.callsign && !entry.missing) return render("resolved", entry.callsign, entry.name || entry.country || "Registered radio", key);
    return render("unresolved", key, "not found", key);
  }
  if (entry) {
    radioId.cache.delete(key);
    scheduleRadioIdCacheSave();
  }
  queueRadioIdLookup(issi);
  const failureUntil = Number(radioId.failureUntil.get(key) || 0);
  const status = failureUntil > Date.now() ? "lookup retrying" : radioId.queued.has(key) ? "queued" : "pending";
  return render("pending", key, status, key);
}

function destinationHtml(entry) {
  const dest = entry?.dest;
  if (!dest) return "--";
  if (entry.activity === "call_group") return `<span class="identity target-group"><span class="identity-primary">GSSI ${esc(dest)}</span></span>`;
  return radioIdentityHtml(dest);
}

function queueRadioIdLookup(issi, options = {}) {
  if (!validLookupIssi(issi)) return;
  if (builtinRadioIdentity(issi)) return;
  const endpoint = radioIdEndpoint();
  if (!endpoint) return;
  const key = normalizeIssi(issi);
  const cached = radioId.cache.get(key);
  if ((validRadioIdCacheEntry(cached) && !(options.priority && cached?.missing)) || radioId.queued.has(key) || radioId.failureUntil.get(key) > Date.now()) return;
  if (cached) radioId.cache.delete(key);
  if (radioId.queue.length >= RADIOID_MAX_QUEUE) {
    if (!options.priority) return;
    const dropped = radioId.queue.pop();
    if (dropped !== undefined) radioId.queued.delete(normalizeIssi(dropped));
  }
  if (options.priority) {
    radioId.queue.unshift(Number(issi));
  } else {
    radioId.queue.push(Number(issi));
  }
  radioId.queued.add(key);
  pumpRadioIdQueue();
}

function identityIsResolved(issi) {
  if (!validLookupIssi(issi)) return true;
  if (builtinRadioIdentity(issi)) return true;
  const entry = radioId.cache.get(normalizeIssi(issi));
  return !!entry && validRadioIdCacheEntry(entry) && !entry.missing && !!entry.callsign;
}

function queueRadioIdRefresh(issi) {
  if (!validLookupIssi(issi) || identityIsResolved(issi)) return;
  queueRadioIdLookup(issi, { priority: true });
}

function parseRadioIdResult(payload, issi) {
  if (payload?.ok && Number(payload.issi) === Number(issi)) {
    const callsign = String(payload.callsign || "").trim().toUpperCase();
    if (!callsign || payload.missing) return { issi: Number(issi), missing: true, fetchedAt: Date.now() };
    const name = String(payload.name || "").trim();
    const country = String(payload.country || "").trim();
    return { issi: Number(issi), callsign, name, country, missing: false, fetchedAt: Date.now() };
  }
  const rows = Array.isArray(payload?.results) ? payload.results : Array.isArray(payload) ? payload : [];
  const row = rows.find((item) => Number(item?.id) === Number(issi)) || rows[0];
  if (!row) return { issi: Number(issi), missing: true, fetchedAt: Date.now() };
  const callsign = String(row.callsign || row.call || "").trim().toUpperCase();
  const name = String(
    row.name ||
      [row.fname, row.surname].filter(Boolean).join(" ") ||
      [row.first_name, row.last_name].filter(Boolean).join(" ") ||
      ""
  ).trim();
  if (!callsign) return { issi: Number(issi), missing: true, fetchedAt: Date.now() };
  const country = String(row.country || row.country_name || "").trim();
  return { issi: Number(issi), callsign, name, country, missing: false, fetchedAt: Date.now() };
}

function radioIdUrl(issi) {
  const url = new URL(radioIdEndpoint(), window.location.href);
  url.searchParams.set("id", normalizeIssi(issi));
  url.searchParams.set("id_sel", "=");
  url.searchParams.set("page", "1");
  url.searchParams.set("per_page", "1");
  return url;
}

function renderSoon() {
  if (renderSoon.queued) return;
  renderSoon.queued = true;
  requestAnimationFrame(() => {
    renderSoon.queued = false;
    renderAll();
  });
}

function callsPayloadKey(msg) {
  const calls = (msg.calls || [])
    .map((call) =>
      [
        call.call_id,
        call.call_type,
        call.gssi,
        call.caller_issi,
        call.called_issi,
        call.active_speaker,
        Math.floor(Number(call.started_secs_ago || 0)),
        call.simplex,
        call.ts,
        call.secondary_ts,
      ].join(":")
    )
    .sort()
    .join("|");
  const heard = (msg.last_heard || [])
    .slice(0, 4)
    .map((entry) => [entry.ts, entry.issi, entry.activity, entry.dest].join(":"))
    .join("|");
  return `${calls}#${heard}#${msg.brew_online ? 1 : 0}:${msg.brew_version || 0}`;
}

function callIdentityIssis(call) {
  const ids = [];
  const speaker = liveSpeakerIssi(call);
  if (speaker) ids.push(speaker);
  for (const value of [call?.active_speaker, call?._lastSpeaker, call?.caller_issi]) {
    const issi = normalizedSpeakerIssi(call, value);
    if (issi) ids.push(issi);
  }
  if (call?.called_issi && call.call_type !== "group") ids.push(call.called_issi);
  return [...new Set(ids.map(Number).filter(validLookupIssi))];
}

function callStartedMsFromPayload(call) {
  const age = Number(call?.started_secs_ago);
  if (!Number.isFinite(age)) return null;
  return Date.now() - Math.max(0, Math.floor(age)) * 1000;
}

function refreshCallIdentities(call) {
  const ids = callIdentityIssis(call);
  if (!ids.length) return;
  for (const delay of [0, 3000, 8000]) {
    setTimeout(() => {
      for (const issi of ids) queueRadioIdRefresh(issi);
      renderSoon();
    }, delay);
  }
}

function upsertCall(call, options = {}) {
  if (call?.call_id === undefined || call?.call_id === null) return;
  const existing = state.calls.get(call.call_id) || {};
  const merged = { ...existing, ...call };
  const previousSpeaker = existing.active_speaker || null;
  let nextSpeaker = Object.prototype.hasOwnProperty.call(call, "active_speaker") ? call.active_speaker || null : existing.active_speaker || null;
  nextSpeaker = normalizedSpeakerIssi(merged, nextSpeaker);
  const incomingStartedMs = callStartedMsFromPayload(call);
  const speakerChanged = Number(previousSpeaker || 0) !== Number(nextSpeaker || 0);
  const callerChanged =
    Object.prototype.hasOwnProperty.call(call, "caller_issi") &&
    Number(existing.caller_issi || 0) !== Number(call.caller_issi || 0);
  const targetChanged =
    Object.prototype.hasOwnProperty.call(call, "gssi") &&
    Number(existing.gssi || 0) !== Number(call.gssi || 0);
  const reusesEndedCall = !!existing._ended || !!existing._hangUntilMs;
  if (incomingStartedMs !== null && (!merged._startedMs || reusesEndedCall || speakerChanged || callerChanged || targetChanged)) {
    merged._startedMs = incomingStartedMs;
  } else if (reusesEndedCall || speakerChanged || callerChanged || targetChanged) {
    merged._startedMs = Date.now();
  } else if (!merged._startedMs) {
    merged._startedMs = Date.now();
  }
  merged.active_speaker = nextSpeaker;
  if (nextSpeaker) {
    merged._lastSpeaker = nextSpeaker;
    if (!existing._speakerStartedMs || Number(previousSpeaker) !== Number(nextSpeaker)) {
      merged._speakerStartedMs = Date.now();
    }
  } else if (!merged._lastSpeaker && existing._lastSpeaker) {
    merged._lastSpeaker = existing._lastSpeaker;
    merged._speakerStartedMs = null;
  }
  merged._hangUntilMs = null;
  merged._ended = false;
  state.calls.set(merged.call_id, merged);
  clearCallCleanup(merged.call_id);
  if (options.identityRefresh !== false) {
    refreshCallIdentities(merged);
  }
}

function clearCallCleanup(callId) {
  const timer = state.callCleanupTimers.get(callId);
  if (timer) clearTimeout(timer);
  state.callCleanupTimers.delete(callId);
}

function removeCall(callId) {
  clearCallCleanup(callId);
  state.calls.delete(callId);
}

function endCall(msg) {
  const existing = state.calls.get(msg.call_id);
  const isGroup = msg.call_type === "group" || existing?.call_type === "group";
  if (!isGroup) {
    removeCall(msg.call_id);
    return;
  }
  if (!existing) return;
  existing._lastSpeaker = existing.active_speaker || existing._lastSpeaker || existing.caller_issi || null;
  existing.active_speaker = null;
  existing._speakerStartedMs = null;
  existing._ended = true;
  existing._hangUntilMs = Date.now() + GROUP_CALL_HANGTIME_UI_MS;
  clearCallCleanup(msg.call_id);
  state.callCleanupTimers.set(
    msg.call_id,
    setTimeout(() => {
      const call = state.calls.get(msg.call_id);
      if (call?._hangUntilMs && call._hangUntilMs <= Date.now()) removeCall(msg.call_id);
      renderSoon();
    }, GROUP_CALL_HANGTIME_UI_MS)
  );
}

async function pumpRadioIdQueue() {
  if (radioId.inflight) return;
  const issi = radioId.queue.shift();
  if (!issi) return;
  const key = normalizeIssi(issi);
  radioId.queued.delete(key);
  if (radioId.cache.has(key) || radioId.failureUntil.get(key) > Date.now()) {
    return pumpRadioIdQueue();
  }

  radioId.inflight = true;
  const delay = Math.max(0, RADIOID_MIN_INTERVAL_MS - (Date.now() - radioId.lastRequestMs));
  if (delay) await new Promise((resolve) => setTimeout(resolve, delay));

  try {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), RADIOID_FETCH_TIMEOUT_MS);
    radioId.lastRequestMs = Date.now();
    try {
      const res = await fetch(radioIdUrl(issi), {
        credentials: radioIdUrl(issi).origin === window.location.origin ? "same-origin" : "omit",
        cache: "default",
        signal: controller.signal,
      });
      if (!res.ok) throw new Error(`RadioID ${res.status}`);
      const payload = await res.json();
      if (payload?.pending) {
        if (!radioId.queued.has(key) && radioId.queue.length < RADIOID_MAX_QUEUE) {
          radioId.queue.push(Number(issi));
          radioId.queued.add(key);
        }
      } else if (payload?.unavailable) {
        radioId.failureUntil.set(key, Date.now() + RADIOID_RETRY_MS);
      } else {
        radioId.cache.set(key, parseRadioIdResult(payload, issi));
        scheduleRadioIdCacheSave();
        renderSoon();
      }
    } finally {
      clearTimeout(timeout);
    }
  } catch {
    radioId.failureUntil.set(key, Date.now() + RADIOID_RETRY_MS);
  } finally {
    radioId.inflight = false;
    if (radioId.queue.length) setTimeout(pumpRadioIdQueue, RADIOID_MIN_INTERVAL_MS);
  }
}

function fmtAge(seconds) {
  const s = Math.max(0, Math.floor(seconds || 0));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

function fmtDurationSecs(seconds) {
  const s = Math.max(0, Math.floor(seconds || 0));
  const days = Math.floor(s / 86400);
  const hours = Math.floor((s % 86400) / 3600);
  const minutes = Math.floor((s % 3600) / 60);
  const secs = s % 60;
  if (days) return `${days}d ${hours}h ${minutes}m`;
  if (hours) return `${hours}h ${minutes}m ${secs}s`;
  if (minutes) return `${minutes}m ${secs}s`;
  return `${secs}s`;
}

function fmtHz(value, unit = "MHz") {
  const n = Number(value);
  if (!Number.isFinite(n)) return "--";
  if (unit === "kHz") return `${(n / 1000).toFixed(1)} kHz`;
  if (unit === "Hz") return `${Math.round(n)} Hz`;
  return `${(n / 1000000).toFixed(6)} MHz`;
}

function fmtDb(value, suffix = "dB") {
  const n = Number(value);
  if (!Number.isFinite(n)) return "--";
  return `${n.toFixed(1)} ${suffix}`;
}

function fmtPct(value) {
  const n = Number(value);
  if (!Number.isFinite(n)) return "--";
  return `${n.toFixed(1)}%`;
}

function fmtDuplexShift(value, spacingId) {
  const n = Number(value);
  if (!Number.isFinite(n)) return "--";
  const id = Number(spacingId);
  const suffix = Number.isFinite(id) ? ` (${id})` : "";
  return `${fmtHz(n)}${suffix}`;
}

function gainsLabel(gains) {
  if (!gains) return "--";
  if (Array.isArray(gains)) {
    if (!gains.length) return "--";
    return gains.map((item) => `${item[0]} ${Number(item[1]).toFixed(1)}`).join(", ");
  }
  const rows = Object.entries(gains).sort((a, b) => a[0].localeCompare(b[0]));
  if (!rows.length) return "--";
  return rows.map(([name, value]) => `${name} ${Number(value).toFixed(1)}`).join(", ");
}

function compactBool(value) {
  return value ? "enabled" : "disabled";
}

function validConfigName(name) {
  return /^[A-Za-z0-9._+-]+\.toml$/.test(String(name || "")) && !String(name || "").endsWith(".bak") && !String(name || "").includes("..");
}

function selectedConfigName() {
  return $("configProfileSelect")?.value || state.configProfiles.find((profile) => profile.active)?.name || "config.toml";
}

function activeConfigName() {
  return state.configProfiles.find((profile) => profile.active)?.name || "config.toml";
}

function normalizeConfigProfiles(payload) {
  const rows = Array.isArray(payload) ? payload : Array.isArray(payload?.profiles) ? payload.profiles : [];
  return rows
    .map((profile) => ({
      name: String(profile.name || "").trim(),
      active: !!profile.active,
      runtime: !!profile.runtime,
    }))
    .filter((profile) => validConfigName(profile.name))
    .sort((a, b) => a.name.localeCompare(b.name));
}

function nextConfigName(seed = "config.toml") {
  const existing = new Set(state.configProfiles.map((profile) => profile.name));
  const stem = String(seed || "config.toml").replace(/\.toml$/i, "").replace(/\+\d+$/u, "") || "config";
  for (let idx = 1; idx < 1000; idx += 1) {
    const name = `${stem}+${idx}.toml`;
    if (!existing.has(name)) return name;
  }
  return `config+${Date.now()}.toml`;
}

function configUrl(name) {
  return name === "config.toml" ? "/api/config" : `/api/configs/${encodeURIComponent(name)}`;
}

function setConfigStatus(message) {
  state.configEditorStatus = message || "idle";
  setText("configEditorStatus", state.configEditorStatus);
}

function syncConfigEditorDom() {
  const name = $("configFileName");
  const editor = $("configEditor");
  if (name && name.value !== state.configEditorName) name.value = state.configEditorName;
  if (editor && !state.configEditorDirty && editor.value !== state.configEditorContent) editor.value = state.configEditorContent;
}

function renderConfigProfiles() {
  const select = $("configProfileSelect");
  if (!select) return;
  const selected = select.value || activeConfigName();
  const rows = state.configProfiles.length ? state.configProfiles : [{ name: "config.toml", active: true, runtime: true }];
  const options = rows
    .map((profile) => {
      const suffix = profile.active ? " active" : profile.runtime ? " runtime" : "";
      return `<option value="${esc(profile.name)}">${esc(profile.name + suffix)}</option>`;
    })
    .join("");
  if (select.innerHTML !== options) select.innerHTML = options;
  select.value = rows.some((profile) => profile.name === selected) ? selected : activeConfigName();
  setText("configProfileStatus", state.configProfilesLoaded ? `${rows.length} profile${rows.length === 1 ? "" : "s"}` : "loading");
  setText("configApplyStatus", activeConfigName() === "config.toml" ? "current config selected" : "activation persists after restart");
  for (const id of [
    "configLoadSelectedBtn",
    "configActivateBtn",
    "configDuplicateBtn",
    "configDeleteBtn",
    "configRefreshBtn",
    "configLoadCurrentBtn",
    "configSaveBtn",
  ]) {
    const button = $(id);
    if (button) button.disabled = !!state.configBusy;
  }
  syncConfigEditorDom();
}

async function loadConfigProfiles() {
  try {
    const res = await fetch("/api/configs", { credentials: "same-origin", cache: "no-store" });
    if (!res.ok) throw new Error(`profiles ${res.status}`);
    state.configProfiles = normalizeConfigProfiles(await res.json());
    state.configProfilesLoaded = true;
    renderConfigProfiles();
  } catch {
    state.configProfilesLoaded = false;
    setText("configProfileStatus", "unavailable");
  }
}

async function loadConfigText(name = "config.toml") {
  const profileName = validConfigName(name) ? name : "config.toml";
  state.configBusy = true;
  setConfigStatus("loading");
  try {
    const res = await fetch(configUrl(profileName), { credentials: "same-origin", cache: "no-store" });
    if (!res.ok) throw new Error(await res.text());
    const content = await res.text();
    state.configEditorName = profileName;
    state.configEditorLoadedName = profileName;
    state.configEditorContent = content;
    state.configEditorDirty = false;
    setConfigStatus(`loaded ${profileName}`);
    syncConfigEditorDom();
  } catch (error) {
    setConfigStatus(`load failed: ${String(error.message || error).slice(0, 120)}`);
  } finally {
    state.configBusy = false;
    renderConfigProfiles();
  }
}

async function saveConfigText() {
  const name = String($("configFileName")?.value || state.configEditorName || "config.toml").trim();
  const editor = $("configEditor");
  const content = editor ? editor.value : state.configEditorContent;
  if (!validConfigName(name)) {
    setConfigStatus("invalid file name");
    return;
  }
  state.configBusy = true;
  setConfigStatus("saving");
  try {
    const res = await fetch(configUrl(name), {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
      headers: { "Content-Type": "text/plain; charset=utf-8" },
      body: content,
    });
    if (!res.ok) throw new Error(await res.text());
    state.configEditorName = name;
    state.configEditorLoadedName = name;
    state.configEditorContent = content;
    state.configEditorDirty = false;
    setConfigStatus(`saved ${name}`);
    await loadConfigProfiles();
  } catch (error) {
    setConfigStatus(`save failed: ${String(error.message || error).slice(0, 120)}`);
  } finally {
    state.configBusy = false;
    renderConfigProfiles();
  }
}

async function activateSelectedConfig() {
  const name = selectedConfigName();
  if (!validConfigName(name)) {
    setConfigStatus("invalid selected profile");
    return;
  }
  state.configBusy = true;
  setConfigStatus("activating");
  try {
    invalidateSiteConfig("config activation");
    const res = await fetch("/api/configs/activate", {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
      headers: { "Content-Type": "text/plain; charset=utf-8" },
      body: name,
    });
    if (!res.ok) throw new Error(await res.text());
    setConfigStatus(`activated ${name}`);
    await loadConfigProfiles();
    await loadSystem();
    scheduleSiteReloads();
  } catch (error) {
    setConfigStatus(`activate failed: ${String(error.message || error).slice(0, 120)}`);
  } finally {
    state.configBusy = false;
    renderConfigProfiles();
  }
}

function invalidateSiteConfig(reason = "config reload") {
  state.site = null;
  state.siteInflight = false;
  state.siteReloadingUntilMs = Date.now() + 30000;
  state.callsPayloadKey = "";
  setText("networkCoreState", "Reloading");
  setText("networkCoreHint", reason);
  setStatusTone("networkStatusStrip", "warn");
  renderStatus();
}

function scheduleSiteReloads() {
  for (const delay of [250, 750, 1500, 3000, 6000, 10000, 15000, 22000, 30000]) {
    window.setTimeout(() => {
      loadSystem();
      loadSite({ force: true });
      loadSnapshot();
    }, delay);
  }
}

async function duplicateSelectedConfig() {
  const source = selectedConfigName();
  let content = $("configEditor")?.value || state.configEditorContent;
  if (!content || source !== state.configEditorLoadedName) {
    try {
      const res = await fetch(configUrl(source), { credentials: "same-origin", cache: "no-store" });
      if (!res.ok) throw new Error(await res.text());
      content = await res.text();
    } catch (error) {
      setConfigStatus(`duplicate failed: ${String(error.message || error).slice(0, 120)}`);
      return;
    }
  }
  const target = nextConfigName(source);
  state.configEditorName = target;
  state.configEditorLoadedName = target;
  state.configEditorContent = content;
  state.configEditorDirty = false;
  syncConfigEditorDom();
  await saveConfigText();
}

async function deleteSelectedConfig() {
  const name = selectedConfigName();
  if (!validConfigName(name)) {
    setConfigStatus("invalid selected profile");
    return;
  }
  if (name === activeConfigName() || name === "config.toml") {
    setConfigStatus("cannot delete active config");
    return;
  }
  if (!window.confirm(`Delete ${name}?`)) return;
  state.configBusy = true;
  setConfigStatus("deleting");
  try {
    const res = await fetch(`/api/configs/${encodeURIComponent(name)}`, {
      method: "DELETE",
      credentials: "same-origin",
      cache: "no-store",
    });
    if (!res.ok) throw new Error(await res.text());
    setConfigStatus(`deleted ${name}`);
    state.configEditorName = "config.toml";
    state.configEditorLoadedName = "";
    state.configEditorDirty = false;
    await loadConfigProfiles();
  } catch (error) {
    setConfigStatus(`delete failed: ${String(error.message || error).slice(0, 120)}`);
  } finally {
    state.configBusy = false;
    renderConfigProfiles();
  }
}

function setServiceStatus(message) {
  state.serviceStatus = message || "idle";
  setText("serviceActionStatus", state.serviceStatus);
}

function renderServiceControls() {
  for (const id of ["serviceRestartBtn", "serviceShutdownBtn", "serviceStopGoBtn", "factoryResetBtn"]) {
    const button = $(id);
    if (button) button.disabled = !!state.serviceBusy || !!state.factoryResetBusy;
  }
  setText("serviceActionStatus", state.serviceStatus || "idle");
}

function defaultEasyStartDraft() {
  const defaults = state.easyStart?.defaults || {};
  return {
    mcc: defaults.mcc ?? 901,
    mnc: defaults.mnc ?? 9999,
    timezone: defaults.timezone || "Europe/Bucharest",
    tx_freq: defaults.tx_freq ?? 438025000,
    duplex_spacing: defaults.duplex_spacing ?? 1,
    custom_spacing_enabled: !!defaults.custom_spacing_enabled,
    custom_spacing_hz: defaults.custom_spacing_hz ?? 7600000,
    brew_enabled: defaults.brew_enabled !== false,
    brew_host: defaults.brew_host || "core.tetrapack.online",
    brew_port: defaults.brew_port ?? 443,
    brew_tls: defaults.brew_tls !== false,
    brew_username: defaults.brew_username ?? 123456789,
    brew_password: defaults.brew_password || "",
  };
}

function easyStartDraftFromInputs() {
  const draft = state.easyStartDraft || defaultEasyStartDraft();
  const valueOr = (id, fallback) => {
    const node = $(id);
    return node ? node.value : fallback;
  };
  const checkedOr = (id, fallback) => {
    const node = $(id);
    return node ? !!node.checked : fallback;
  };
  return {
    ...draft,
    mcc: Number(valueOr("easyMcc", draft.mcc)),
    mnc: Number(valueOr("easyMnc", draft.mnc)),
    timezone: String(valueOr("easyTimezone", draft.timezone)).trim(),
    tx_freq: Number(valueOr("easyTxFreq", draft.tx_freq)),
    duplex_spacing: Number(valueOr("easyDuplexSpacing", draft.duplex_spacing)),
    custom_spacing_enabled: checkedOr("easyCustomSpacingEnabled", !!draft.custom_spacing_enabled),
    custom_spacing_hz: Number(valueOr("easyCustomSpacingHz", draft.custom_spacing_hz)),
    brew_enabled: checkedOr("easyBrewEnabled", !!draft.brew_enabled),
    brew_host: String(valueOr("easyBrewHost", draft.brew_host)).trim(),
    brew_port: Number(valueOr("easyBrewPort", draft.brew_port)),
    brew_tls: checkedOr("easyBrewTls", !!draft.brew_tls),
    brew_username: Number(valueOr("easyBrewUsername", draft.brew_username)),
    brew_password: String(valueOr("easyBrewPassword", draft.brew_password)),
  };
}

function easyStartSpacingHz(draft) {
  if (draft.custom_spacing_enabled) return Number(draft.custom_spacing_hz) || 7000000;
  const code = Number(draft.duplex_spacing);
  if (code === 1) return 7000000;
  if (code === 2) return 10000000;
  if (code === 3) return 45000000;
  return 7000000;
}

function easyStartCellEstimate(draft) {
  const tx = Number(draft.tx_freq);
  if (!Number.isFinite(tx) || tx <= 0) return {};
  const freqBand = Math.floor(tx / 100000000);
  const bandBase = freqBand * 100000000;
  const offsets = [0, 6250, -6250, 12500];
  let best = null;
  for (const offset of offsets) {
    const carrier = Math.round((tx - bandBase - offset) / 25000);
    if (carrier < 0) continue;
    const reconstructed = bandBase + offset + carrier * 25000;
    const error = Math.abs(reconstructed - tx);
    const candidate = { error, freq_band: freqBand, main_carrier: carrier, freq_offset: offset };
    if (!best || candidate.error < best.error) best = candidate;
  }
  const shift = easyStartSpacingHz(draft);
  return {
    ...(best || {}),
    tx_freq: tx,
    rx_freq: tx > shift ? tx - shift : null,
    duplex_shift_hz: shift,
  };
}

function saveEasyStartInputs() {
  if ($("easyStartModal")?.classList.contains("hidden")) return;
  state.easyStartDraft = easyStartDraftFromInputs();
}

function setEasyStartMessage(message, kind = "") {
  const node = $("easyStartMessage");
  if (!node) return;
  node.textContent = message || "";
  node.classList.toggle("error", kind === "error");
  node.classList.toggle("ok", kind === "ok");
}

function easyStartSteps() {
  return ["Network", "Carrier", "Brew", "Review"];
}

function easyField(id, label, value, attrs = "") {
  return `<label class="field">
    <span>${esc(label)}</span>
    <input id="${esc(id)}" value="${esc(value)}" ${attrs}>
  </label>`;
}

function renderEasyStartProgress() {
  const steps = easyStartSteps();
  setHtml("easyStartProgress", steps.map((step, index) => `<div class="wizard-step-pill${index === state.easyStartStep ? " active" : ""}">${index + 1}. ${esc(step)}</div>`).join(""));
}

function renderEasyStartWizard() {
  const draft = state.easyStartDraft || defaultEasyStartDraft();
  const step = state.easyStartStep;
  renderEasyStartProgress();
  let body = "";
  if (step === 0) {
    body = `<p class="wizard-copy">Set the network identity and local time. For a private/test TETRA network, 901 / 9999 is the usual starter pair.</p>
      <div class="wizard-grid">
        ${easyField("easyMcc", "MCC", draft.mcc, 'type="number" min="1" max="999" inputmode="numeric"')}
        ${easyField("easyMnc", "MNC", draft.mnc, 'type="number" min="0" max="9999" inputmode="numeric"')}
        ${easyField("easyTimezone", "Timezone", draft.timezone, 'type="text" autocomplete="off" spellcheck="false" class="full"')}
      </div>`;
  } else if (step === 1) {
    body = `<p class="wizard-copy">Enter the downlink TX frequency in Hz. Nexus-BS calculates RX frequency, carrier number, frequency band, and offset for you.</p>
      <div class="wizard-grid">
        ${easyField("easyTxFreq", "TX frequency Hz", draft.tx_freq, 'type="number" min="300000000" max="999999999" inputmode="numeric"')}
        ${easyField("easyDuplexSpacing", "Duplex spacing code", draft.duplex_spacing, 'type="number" min="1" max="15" inputmode="numeric"')}
        <label class="check-field full"><input id="easyCustomSpacingEnabled" type="checkbox" ${draft.custom_spacing_enabled ? "checked" : ""}><span>Use custom duplex spacing</span></label>
        ${easyField("easyCustomSpacingHz", "Custom spacing Hz", draft.custom_spacing_hz, 'type="number" min="1000000" max="20000000" inputmode="numeric"')}
      </div>`;
  } else if (step === 2) {
    body = `<p class="wizard-copy">Brew connects this cell to a TETRAPACK/TETRALink/TETRAFlow style core. Turn it off for a standalone local cell.</p>
      <div class="wizard-grid">
        <label class="check-field full"><input id="easyBrewEnabled" type="checkbox" ${draft.brew_enabled ? "checked" : ""}><span>Enable Brew core connection</span></label>
        ${easyField("easyBrewHost", "Core host", draft.brew_host, 'type="text" autocomplete="off" spellcheck="false"')}
        ${easyField("easyBrewPort", "Core port", draft.brew_port, 'type="number" min="1" max="65535" inputmode="numeric"')}
        <label class="check-field"><input id="easyBrewTls" type="checkbox" ${draft.brew_tls ? "checked" : ""}><span>Use SSL/TLS</span></label>
        ${easyField("easyBrewUsername", "Username / SSI", draft.brew_username, 'type="number" min="1" inputmode="numeric"')}
        ${easyField("easyBrewPassword", "Password", draft.brew_password, 'type="password" autocomplete="new-password"')}
      </div>`;
  } else {
    const estimate = easyStartCellEstimate(draft);
    const summary = { ...estimate, ...(state.easyStartPreview?.summary || {}) };
    body = `<p class="wizard-copy">Verify checks the generated config with the same parser Nexus-BS uses at startup. Commit writes config.toml and restarts the BS.</p>
      <div class="wizard-review">
        <div class="wizard-review-row"><span>Network</span><strong>${esc(summary.network || `${draft.mcc} / ${draft.mnc}`)}</strong></div>
        <div class="wizard-review-row"><span>TX</span><strong>${esc(summary.tx_freq || draft.tx_freq)} Hz</strong></div>
        <div class="wizard-review-row"><span>RX</span><strong>${esc(summary.rx_freq || "--")} Hz</strong></div>
        <div class="wizard-review-row"><span>Duplex spacing</span><strong>code ${esc(draft.duplex_spacing)} / ${esc(summary.duplex_shift_hz || easyStartSpacingHz(draft))} Hz</strong></div>
        <div class="wizard-review-row"><span>Carrier</span><strong>${esc(summary.main_carrier ?? "--")}</strong></div>
        <div class="wizard-review-row"><span>Offset</span><strong>${esc(summary.freq_offset ?? "--")}</strong></div>
        <div class="wizard-review-row"><span>Freq band</span><strong>${esc(summary.freq_band ?? "--")}</strong></div>
        <div class="wizard-review-row"><span>Timezone</span><strong>${esc(summary.timezone || draft.timezone)}</strong></div>
        <div class="wizard-review-row"><span>Brew</span><strong>${esc(summary.brew || (draft.brew_enabled ? `${draft.brew_host}:${draft.brew_port}` : "disabled"))}</strong></div>
      </div>`;
  }
  setHtml("easyStartBody", body);
  $("easyStartBackBtn").style.display = step > 0 ? "" : "none";
  $("easyStartNextBtn").style.display = step < 3 ? "" : "none";
  $("easyStartVerifyBtn").style.display = step === 3 ? "" : "none";
  $("easyStartCommitBtn").style.display = step === 3 ? "" : "none";
  for (const id of ["easyStartBackBtn", "easyStartNextBtn", "easyStartVerifyBtn", "easyStartCommitBtn", "easyStartSkipBtn"]) {
    const button = $(id);
    if (button) button.disabled = !!state.easyStartBusy;
  }
}

async function loadEasyStartStatus() {
  if (state.easyStartDismissed) return;
  try {
    const res = await fetch("/api/easy-start/status", { credentials: "same-origin", cache: "no-store" });
    if (!res.ok) return;
    const body = await res.json();
    state.easyStart = body;
    const modal = $("easyStartModal");
    const alreadyOpen = modal ? !modal.classList.contains("hidden") : false;
    if (body.required && !alreadyOpen) {
      openEasyStartWizard();
    }
  } catch {
    // Non-critical; dashboard remains usable.
  }
}

function openEasyStartWizard() {
  state.easyStartDraft = state.easyStartDraft || defaultEasyStartDraft();
  state.easyStartStep = 0;
  state.easyStartPreview = null;
  $("easyStartModal")?.classList.remove("hidden");
  setEasyStartMessage("Fill the basic fields. Advanced TETRA values are calculated automatically.");
  renderEasyStartWizard();
}

function easyStartRequestedByUrl() {
  return window.location.pathname === "/easy-start" || window.location.hash === "#easy-start";
}

function openEasyStartWizardFromUrl() {
  if (!easyStartRequestedByUrl()) return;
  openEasyStartWizard();
  if (window.location.pathname === "/easy-start") {
    window.history.replaceState({}, "", `${window.location.origin}/${window.location.hash || ""}`);
  }
}

async function skipEasyStartWizard() {
  if (state.easyStartBusy) return;
  state.easyStartBusy = true;
  renderEasyStartWizard();
  try {
    await fetch("/api/easy-start/skip", { method: "POST", credentials: "same-origin", cache: "no-store" });
  } finally {
    state.easyStartDismissed = true;
    $("easyStartModal")?.classList.add("hidden");
    state.easyStartBusy = false;
  }
}

async function verifyEasyStartConfig() {
  if (state.easyStartBusy) return false;
  saveEasyStartInputs();
  state.easyStartBusy = true;
  setEasyStartMessage("Verifying generated config...");
  renderEasyStartWizard();
  try {
    const res = await fetch("/api/easy-start/preview", {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(state.easyStartDraft),
    });
    const body = await res.json().catch(() => ({}));
    if (!res.ok || body.ok === false) throw new Error(body.error || body.message || `HTTP ${res.status}`);
    state.easyStartPreview = body;
    setEasyStartMessage(body.message || "Config verified.", "ok");
    return true;
  } catch (error) {
    state.easyStartPreview = null;
    setEasyStartMessage(String(error.message || error).slice(0, 180), "error");
    return false;
  } finally {
    state.easyStartBusy = false;
    renderEasyStartWizard();
  }
}

async function commitEasyStartConfig() {
  if (state.easyStartBusy) return;
  const verified = state.easyStartPreview || (await verifyEasyStartConfig());
  if (!verified) return;
  if (!window.confirm("Save this config and restart Nexus-BS now?")) return;
  state.easyStartBusy = true;
  setEasyStartMessage("Saving config and starting Nexus-BS...");
  renderEasyStartWizard();
  try {
    const res = await fetch("/api/easy-start/commit", {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(state.easyStartDraft),
    });
    const body = await res.json().catch(() => ({}));
    if (!res.ok || body.ok === false) throw new Error(body.error || body.message || `HTTP ${res.status}`);
    setEasyStartMessage(body.message || "Config saved. Restart queued.", "ok");
    state.easyStartDismissed = true;
    invalidateSiteConfig("Easy Start committed");
    scheduleSiteReloads();
    setTimeout(() => $("easyStartModal")?.classList.add("hidden"), 1200);
  } catch (error) {
    setEasyStartMessage(`Commit failed: ${String(error.message || error).slice(0, 160)}`, "error");
  } finally {
    state.easyStartBusy = false;
    renderEasyStartWizard();
  }
}

function nextEasyStartStep() {
  saveEasyStartInputs();
  state.easyStartStep = Math.min(3, state.easyStartStep + 1);
  if (state.easyStartStep === 3) state.easyStartPreview = null;
  setEasyStartMessage(state.easyStartStep === 3 ? "Calculated values are shown now. Verify confirms them with the Nexus-BS parser." : "Fill the fields on this step.");
  renderEasyStartWizard();
  if (state.easyStartStep === 3) {
    window.setTimeout(() => verifyEasyStartConfig(), 0);
  }
}

function previousEasyStartStep() {
  saveEasyStartInputs();
  state.easyStartStep = Math.max(0, state.easyStartStep - 1);
  setEasyStartMessage("Review or adjust the fields.");
  renderEasyStartWizard();
}

function openFactoryResetDialog() {
  if (state.factoryResetBusy) return;
  const input = $("factoryResetConfirmText");
  if (input) input.value = "";
  setText("factoryResetStatus", "No reset requested.");
  $("factoryResetModal")?.classList.remove("hidden");
}

async function requestFactoryReset() {
  if (state.factoryResetBusy) return;
  const confirmation = String($("factoryResetConfirmText")?.value || "").trim();
  if (confirmation !== "RESET NEXUS-BS") {
    setText("factoryResetStatus", "Type RESET NEXUS-BS exactly.");
    return;
  }
  state.factoryResetBusy = true;
  renderServiceControls();
  setText("factoryResetStatus", "Reset running. Host shutdown will follow.");
  try {
    const res = await fetch("/api/factory-reset", {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ confirmation }),
    });
    const body = await res.json().catch(() => ({}));
    if (!res.ok || body.ok === false) throw new Error(body.error || body.message || `HTTP ${res.status}`);
    setText("factoryResetStatus", body.message || "Factory reset accepted. Host shutdown accepted.");
    setServiceStatus("factory reset queued");
  } catch (error) {
    setText("factoryResetStatus", `reset failed: ${String(error.message || error).slice(0, 120)}`);
    state.factoryResetBusy = false;
    renderServiceControls();
  }
}

function setWifiStatus(message) {
  state.wifiActionStatus = message || "idle";
  setText("wifiActionStatus", state.wifiActionStatus);
}

function wifiSecurityLabel(network) {
  const security = String(network?.security || "").trim();
  return security || (network?.secure ? "secured" : "open");
}

function wifiBandLabel(network) {
  const raw = String(network?.frequency || "").toLowerCase();
  const mhz = Number((raw.match(/\d+/) || [])[0]);
  if (Number.isFinite(mhz) && mhz > 0) {
    if (mhz >= 4900) return "5 GHz";
    if (mhz >= 2400) return "2.4 GHz";
  }
  const channel = Number(network?.channel);
  if (Number.isFinite(channel) && channel > 0) return channel > 14 ? "5 GHz" : "2.4 GHz";
  return "--";
}

function wifiNetworkDetail(network) {
  const details = [];
  const security = wifiSecurityLabel(network);
  if (security) details.push(security);
  const band = wifiBandLabel(network);
  if (band !== "--") details.push(band);
  if (network?.channel) details.push(`ch ${network.channel}`);
  if (network?.rate) details.push(network.rate);
  return details.join(" / ") || "--";
}

function renderWifiCurrentCard(currentNetwork, currentSsid) {
  const ssid = currentSsid || currentNetwork?.ssid || "--";
  const signal = Number(currentNetwork?.signal);
  const signalLabel = Number.isFinite(signal) ? `${signal}%` : "--";
  const band = wifiBandLabel(currentNetwork);
  const detailParts = [];
  const security = wifiSecurityLabel(currentNetwork);
  if (security && security !== "--") detailParts.push(security);
  if (band !== "--") detailParts.push(band);
  if (currentNetwork?.channel) detailParts.push(`ch ${currentNetwork.channel}`);
  const detail = detailParts.join(" / ") || "connected";
  return `<div class="wifi-current-card">
    <div>
      <span>Connected</span>
      <strong>${esc(ssid)}</strong>
      <small>${esc(detail)}</small>
    </div>
    <em>${esc(signalLabel)}</em>
  </div>`;
}

function renderWifi() {
  const wifi = state.wifi || {};
  const networks = Array.isArray(wifi.networks) ? wifi.networks : [];
  const device = wifi.device || {};
  const currentSsid = wifi.current_ssid || device.connection || "";
  const selected = state.wifiSelectedSsid || currentSsid || "";
  const currentNetwork = networks.find((network) => network.active || (currentSsid && network.ssid === currentSsid));

  setText("wifiStatus", wifi.available === false ? "unavailable" : state.wifiBusy ? "working" : wifi.status || "ready");
  setText("wifiDevice", device.name || wifi.device_name || "--");
  setText("wifiState", device.state || wifi.message || "--");
  setText("wifiCurrent", currentSsid || "--");
  setText("wifiActionStatus", state.wifiActionStatus || "idle");

  const ssidInput = $("wifiSsid");
  if (ssidInput && document.activeElement !== ssidInput && selected) ssidInput.value = selected;

  if (!state.wifiScanVisible) {
    setHtml("wifiNetworkList", renderWifiCurrentCard(currentNetwork, currentSsid));
  } else {
    const rows = networks.map((network) => {
    const ssid = String(network.ssid || "");
    const isCurrent = !!network.active || (currentSsid && ssid === currentSsid);
    const isSelected = selected && ssid === selected;
    const signal = Number(network.signal);
    const signalLabel = Number.isFinite(signal) ? `${signal}%` : "--";
    return `<button class="wifi-network${isCurrent ? " current" : ""}${isSelected ? " selected" : ""}" type="button" data-ssid="${esc(ssid)}">
      <span>
        <strong>${esc(ssid || "(hidden)")}</strong>
        <small>${esc(wifiNetworkDetail(network))}${isCurrent ? " / connected" : ""}</small>
      </span>
      <em>${esc(signalLabel)}</em>
    </button>`;
    });
    setHtml("wifiNetworkList", rows.length ? rows.join("") : '<div class="empty wifi-empty">No Wi-Fi networks scanned</div>');
    for (const node of document.querySelectorAll(".wifi-network[data-ssid]")) {
      node.addEventListener("click", () => {
        state.wifiSelectedSsid = node.dataset.ssid || "";
        const input = $("wifiSsid");
        if (input) input.value = state.wifiSelectedSsid;
        renderWifi();
      });
    }
  }

  for (const id of ["wifiScanBtn", "wifiClearBtn", "wifiConnectBtn"]) {
    const button = $(id);
    if (button) button.disabled = !!state.wifiBusy || wifi.available === false;
  }
}

async function loadWifiStatus() {
  try {
    const res = await fetch("/api/wifi", { credentials: "same-origin", cache: "no-store" });
    if (!res.ok) throw new Error(await res.text());
    const fresh = await res.json();
    if (state.wifiScanVisible && Array.isArray(state.wifi?.networks) && state.wifi.networks.length) {
      const freshNetworks = Array.isArray(fresh.networks) ? fresh.networks : [];
      const keepScannedNetworks = state.wifi.networks.length > freshNetworks.length;
      state.wifi = {
        ...fresh,
        networks: keepScannedNetworks ? state.wifi.networks : freshNetworks,
      };
    } else {
      state.wifi = fresh;
    }
    if (!state.wifiSelectedSsid && state.wifi?.current_ssid) state.wifiSelectedSsid = state.wifi.current_ssid;
  } catch (error) {
    state.wifi = {
      ok: false,
      available: false,
      status: "unavailable",
      message: String(error.message || error).slice(0, 120),
      networks: [],
    };
  } finally {
    renderWifi();
  }
}

async function scanWifiNetworks() {
  if (state.wifiBusy) return;
  state.wifiBusy = true;
  setWifiStatus("scanning");
  renderWifi();
  try {
    const res = await fetchWithTimeout("/api/wifi/scan", {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
    }, 20000);
    const body = await res.json().catch(() => ({}));
    if (!res.ok || body.ok === false) throw new Error(body.error || body.message || `HTTP ${res.status}`);
    state.wifi = body;
    state.wifiScanVisible = true;
    setWifiStatus(`found ${(body.networks || []).length} networks`);
  } catch (error) {
    setWifiStatus(`scan failed: ${String(error.message || error).slice(0, 100)}`);
  } finally {
    state.wifiBusy = false;
    renderWifi();
  }
}

function clearWifiScanList() {
  state.wifiScanVisible = false;
  setWifiStatus("ready");
  renderWifi();
}

async function connectWifiNetwork() {
  if (state.wifiBusy) return;
  const ssid = String($("wifiSsid")?.value || "").trim();
  const password = String($("wifiPassword")?.value || "");
  const hidden = !!$("wifiHidden")?.checked;
  if (!ssid) {
    setWifiStatus("SSID required");
    return;
  }
  state.wifiBusy = true;
  state.wifiSelectedSsid = ssid;
  setWifiStatus("connecting");
  renderWifi();
  try {
    const res = await fetchWithTimeout("/api/wifi/connect", {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ ssid, password, hidden }),
    }, 30000);
    const body = await res.json().catch(() => ({}));
    if (!res.ok || body.ok === false) throw new Error(body.error || body.message || `HTTP ${res.status}`);
    state.wifi = body;
    const passwordInput = $("wifiPassword");
    if (passwordInput) passwordInput.value = "";
    setWifiStatus(body.message || `connected ${ssid}`);
  } catch (error) {
    setWifiStatus(`connect failed: ${String(error.message || error).slice(0, 100)}`);
  } finally {
    state.wifiBusy = false;
    renderWifi();
  }
}

function syncWifiPasswordVisibility() {
  const input = $("wifiPassword");
  const show = !!$("wifiShowPassword")?.checked;
  if (input) input.type = show ? "text" : "password";
}

async function requestServiceAction(action) {
  const endpoints = {
    restart: "/api/service/restart",
    shutdown: "/api/service/shutdown",
    stopgo: "/api/service/stop-go",
  };
  const labels = {
    restart: "restart",
    shutdown: "shutdown BS + OS",
    stopgo: "stop & go",
  };
  const confirmations = {
    shutdown: "Shutdown BS and power off the Linux host?\nRF and dashboard will stop until physical power is restored.",
    stopgo: "Stop & Go BS?",
  };
  const endpoint = endpoints[action];
  if (!endpoint || state.serviceBusy) return;
  if (action !== "restart" && !window.confirm(confirmations[action] || `${labels[action]} BS?`)) return;
  state.serviceBusy = true;
  if (action === "restart" || action === "stopgo") invalidateSiteConfig(`${labels[action]} queued`);
  renderServiceControls();
  setServiceStatus(`${labels[action]} queued`);
  try {
    const res = await fetch(endpoint, {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
    });
    const body = await res.json().catch(() => ({}));
    if (!res.ok || body.ok === false) throw new Error(body.error || body.message || `HTTP ${res.status}`);
    setServiceStatus(body.message || `${labels[action]} accepted`);
    if (action === "restart" || action === "stopgo") scheduleSiteReloads();
  } catch (error) {
    setServiceStatus(`${labels[action]} failed: ${String(error.message || error).slice(0, 90)}`);
  } finally {
    window.setTimeout(() => {
      state.serviceBusy = false;
      renderServiceControls();
    }, 2500);
  }
}

function updateRelationLabel(release) {
  const rel = release?.relation || "available";
  if (rel === "newer") return "update";
  if (rel === "older") return "downgrade";
  if (rel === "current") return "installed";
  return rel;
}

function selectedUpdateRelease() {
  const url = $("updateReleaseSelect")?.value || state.selectedUpdateUrl || "";
  return (state.updateCatalog?.releases || []).find((release) => release.deb_url === url) || null;
}

async function loadUpdateCatalog() {
  state.updateStatus = "checking";
  renderUpdatePanel();
  try {
    const catalog = await fetchDashboardJson(
      "/api/update/check",
      { credentials: "same-origin", cache: "no-store" },
      14000
    );
    state.updateCatalog = catalog;
    const releases = catalog.releases || [];
    const preferred = releases.find((release) => release.update_available) || releases.find((release) => release.relation === "current") || releases[0];
    state.selectedUpdateUrl = preferred?.deb_url || "";
    state.updateStatus = catalog.check_failed ? "check_failed" : catalog.update_available ? "update_available" : "latest";
    if (catalog.check_failed) state.updateLog = catalog.error || "Update check failed.";
  } catch (error) {
    state.updateCatalog = null;
    state.updateStatus = "check_failed";
    state.updateLog = `Update check failed: ${String(error.message || error)}`;
  }
  renderUpdatePanel();
}

async function loadUpdateStatus() {
  try {
    const status = await fetchDashboardJson(
      "/api/update/status",
      { credentials: "same-origin", cache: "no-store" },
      5000
    );
    state.updateLog = status.log || state.updateLog || "";
    if (status.status === "running") state.updateStatus = "running";
    if (status.status === "done_ok") state.updateStatus = "done_ok";
    if (status.status === "done_err") state.updateStatus = "done_err";
    state.updateBusy = status.status === "running";
    renderUpdatePanel();
  } catch {
    // Status polling is best-effort; release catalog remains useful.
  }
}

async function applySelectedUpdate() {
  const release = selectedUpdateRelease();
  if (!release || state.updateBusy) return;
  const action = release.relation === "older" ? "Downgrade" : release.relation === "current" ? "Reinstall" : "Update";
  if (
    !window.confirm(
      `${action} Nexus-BS to ${release.tag}?\n\nThe package will be downloaded from GitHub. Core/control services stop before install, wait 5s, then start again.`
    )
  ) {
    return;
  }
  state.updateBusy = true;
  state.updateStatus = "running";
  state.updateLog = `Starting ${action.toLowerCase()} to ${release.tag}...`;
  renderUpdatePanel();
  try {
    const res = await fetchWithTimeout(
      "/api/update/deb",
      {
        method: "POST",
        credentials: "same-origin",
        cache: "no-store",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          version: release.tag,
          asset_name: release.deb_asset_name,
          url: release.deb_url,
        }),
      },
      8000
    );
    const body = await res.json().catch(async () => ({ error: await res.text().catch(() => "") }));
    if (!res.ok || body.ok === false) throw new Error(body.error || body.message || `HTTP ${res.status}`);
    state.updateLog = body.message || "Package update started.";
    pollUpdateStatus();
  } catch (error) {
    state.updateBusy = false;
    state.updateStatus = "done_err";
    state.updateLog = `Update request failed: ${String(error.message || error)}`;
    renderUpdatePanel();
  }
}

function pollUpdateStatus() {
  loadUpdateStatus();
  if (state.updateBusy) window.setTimeout(pollUpdateStatus, 1500);
}

function setSiteCarrierInhibited(inhibited) {
  if (!state.site) state.site = {};
  if (!state.site.config) state.site.config = {};
  if (!state.site.config.rf_control) state.site.config.rf_control = {};
  state.site.config.rf_control.carrier_inhibited = !!inhibited;
  state.site.config.rf_control.carrier_state = inhibited ? "carrier_inhibited" : "carrier_active";
}

function effectiveCarrierInhibited() {
  if (state.rfCarrierPendingInhibited !== null && Date.now() < state.rfCarrierPendingUntilMs) {
    return !!state.rfCarrierPendingInhibited;
  }
  state.rfCarrierPendingInhibited = null;
  state.rfCarrierPendingUntilMs = 0;
  return !!state.site?.config?.rf_control?.carrier_inhibited;
}

async function requestRfCarrierToggle() {
  if (state.rfCarrierBusy || !state.site?.config?.available) return;
  const current = effectiveCarrierInhibited();
  const next = !current;
  if (next && !window.confirm("Inhibit RF carrier?\nThe BS stays running, but on-air service will stop until RF Carrier is enabled again.")) {
    return;
  }

  state.rfCarrierBusy = true;
  renderStatus();
  try {
    const res = await fetchWithTimeout("/api/rf/carrier", {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ inhibited: next }),
    }, COMMAND_FETCH_TIMEOUT_MS);
    const body = await res.json().catch(() => ({}));
    if (!res.ok || body.ok === false) throw new Error(body.error || body.message || `HTTP ${res.status}`);
    const inhibited = !!body.inhibited;
    state.rfCarrierPendingInhibited = inhibited;
    state.rfCarrierPendingUntilMs = Date.now() + 15000;
    setSiteCarrierInhibited(inhibited);
    renderAll();
    await loadSite();
  } catch (error) {
    state.logs.push({
      ts: new Date().toLocaleTimeString(),
      level: "WARN",
      msg: `RF carrier command failed: ${String(error.message || error).slice(0, 110)}`,
    });
  } finally {
    state.rfCarrierBusy = false;
    renderAll();
  }
}

function setCalibrationStatus(message) {
  setText("calibrationActionStatus", message || "idle");
}

async function requestTxCalibration() {
  if (state.calibrationBusy) return;
  if (!window.confirm("Run destructive TX DC/IQ calibration?\nTETRA traffic will be stopped. Accepted runs update calibration.toml and restart with DC correction; rejected runs keep the existing calibration and save only the run report. IQ is measured but remains opt-in until RF burst EVM validation passes.")) {
    return;
  }
  state.calibrationBusy = true;
  renderCalibration();
  setCalibrationStatus("starting calibration");
  try {
    const res = await fetchWithTimeout("/api/rf/calibration/run", {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
    }, COMMAND_FETCH_TIMEOUT_MS);
    const body = await res.json().catch(() => ({}));
    if (!res.ok || body.ok === false) throw new Error(body.error || body.message || `HTTP ${res.status}`);
    state.calibration = body;
    setCalibrationStatus(body.message || "calibration running");
    await loadCalibrationStatus();
  } catch (error) {
    setCalibrationStatus(`calibration failed: ${String(error.message || error).slice(0, 110)}`);
  } finally {
    window.setTimeout(() => {
      state.calibrationBusy = false;
      renderCalibration();
    }, 2500);
  }
}

async function loadCalibrationStatus() {
  try {
    const res = await fetch("/api/rf/calibration/status", {
      credentials: "same-origin",
      cache: "no-store",
    });
    if (!res.ok) return;
    state.calibration = await res.json();
    state.calibrationBusy = !!state.calibration?.active;
    renderCalibration();
  } catch {
    // Keep the last report visible.
  }
}

async function applySafeRfProfile(target = null) {
  if (state.rfProfileApplyBusy) return;
  const advice = state.site?.rf_profile_advice || {};
  const pendingRetest = advice.profile_validation_status === "pending_retest";
  const targetSuffix = target ? ` (${target.replaceAll("_", " ")})` : "";
  const title = pendingRetest || target ? `Apply profile for RF/EVM retest${targetSuffix}?` : "Apply measured RF profile?";
  const fallback = pendingRetest
    ? "The backend will switch profile only as a retest candidate; run RF calibration again after restart."
    : "The backend will refuse if RF EVM evidence is missing or unsafe.";
  if (!window.confirm(`${title}\n${advice.summary || fallback}`)) {
    return;
  }
  state.rfProfileApplyBusy = true;
  setCalibrationStatus(pendingRetest ? "applying profile for RF/EVM retest" : "applying measured RF profile");
  try {
    const res = await fetchWithTimeout("/api/rf/profile-autotest/apply-safe", {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(target ? { target } : {}),
    }, COMMAND_FETCH_TIMEOUT_MS);
    const body = await res.json().catch(() => ({}));
    if (!res.ok || body.ok === false) throw new Error(body.error || body.message || `HTTP ${res.status}`);
    setCalibrationStatus(body.message || "RF profile applied; restart queued");
    await loadSite({ force: true });
  } catch (error) {
    setCalibrationStatus(`profile apply refused: ${String(error.message || error).slice(0, 110)}`);
  } finally {
    window.setTimeout(() => {
      state.rfProfileApplyBusy = false;
      renderCalibration();
    }, 2500);
  }
}

function renderCalibration() {
  const status = state.calibration || {};
  const report = status.report || {};
  const activeReport = status.active_report || {};
  const reference = report.reference || {};
  const calibrated = report.calibrated || {};
  const applied = report.applied || {};
  const summary = report.report || {};
  const activeSummary = activeReport.report || {};
  const accepted = !!summary.accepted;
  const activeAccepted = !!activeSummary.accepted;
  const active = !!status.active || !!state.calibrationBusy;
  const phase = status.status || "idle";
  const failed = phase === "failed";
  const rfAdvice = state.site?.rf_profile_advice || {};

  setText("calibrationStatus", active ? phase.toUpperCase() : accepted && !failed ? "APPLIED" : phase.toUpperCase());
  setText("calibrationPath", status.report_path || status.path || "calibration.toml");
  setText(
    "calibrationApplied",
    report.status
      ? `${report.status} dc(${fmtSignedFixed(applied.dc_i, 4)}, ${fmtSignedFixed(applied.dc_q, 4)}) iq(${fmtSignedFixed(applied.iq_i, 4)}, ${fmtSignedFixed(applied.iq_q, 4)})`
      : "--"
  );
  setText(
    "calibrationCarrier",
    metricBeforeAfter(reference.carrier_leakage_dbc, calibrated.carrier_leakage_dbc, "dBc", summary.carrier_leakage_improvement_db)
  );
  setText(
    "calibrationImage",
    metricBeforeAfter(reference.image_rejection_db, calibrated.image_rejection_db, "dB", summary.image_rejection_improvement_db)
  );
  setText(
    "calibrationKnownRmsEvm",
    metricBeforeAfter(reference.tetra_known_rms_evm_pct, calibrated.tetra_known_rms_evm_pct, "%", summary.tetra_known_rms_evm_improvement_pct)
  );
  setText(
    "calibrationKnownPeakEvm",
    metricBeforeAfter(reference.tetra_known_peak_evm_pct, calibrated.tetra_known_peak_evm_pct, "%", summary.tetra_known_peak_evm_improvement_pct)
  );
  setText(
    "calibrationEvm",
    metricBeforeAfter(reference.evm_proxy_pct, calibrated.evm_proxy_pct, "%", summary.evm_proxy_improvement_pct)
  );
  setText(
    "calibrationProfileAdvice",
    rfAdvice.measurement_valid
      ? `${String(rfAdvice.profile_validation_status || "unmeasured").toUpperCase()} / ${String(rfAdvice.severity || "ok").toUpperCase()} / ${rfAdvice.summary || "--"}`
      : rfAdvice.summary || "--"
  );
  setText(
    "calibrationActionStatus",
    active
      ? "running; accepted DC restarts service; IQ remains opt-in"
      : failed
        ? status.error || summary.summary || "calibration failed"
        : summary.summary || (activeAccepted ? "active calibration preserved" : status.error || "traffic outage required")
  );
  setText("calibrationLog", status.log || (summary.summary ? `${summary.summary}\n` : ""));
  const button = $("calibrationRunBtn");
  if (button) button.disabled = active || state.calibrationBusy || !state.site?.config?.available;
  const profileButton = $("rfProfileApplyBtn");
  if (profileButton) {
    profileButton.textContent = rfAdvice.profile_validation_status === "pending_retest" ? "Apply & Retest Profile" : "Apply Safe Profile";
    profileButton.disabled = active || state.rfProfileApplyBusy || !rfAdvice.measurement_valid || !rfAdvice.safe_auto_apply;
  }
  for (const [id, target] of [
    ["rfProfileHotspotBtn", "hotspot"],
    ["rfProfileLowPowerBtn", "low_power_basestation"],
    ["rfProfilePaBtn", "power_amplified_basestation"],
  ]) {
    const targetButton = $(id);
    if (!targetButton) continue;
    const currentTarget = rfAdvice.target_profile === target;
    targetButton.disabled = active || state.rfProfileApplyBusy || !rfAdvice.measurement_valid || currentTarget && rfAdvice.profile_validation_status === "validated";
  }
}

function metricBeforeAfter(before, after, unit, improvement) {
  const b = Number(before);
  const a = Number(after);
  if (!Number.isFinite(b) || !Number.isFinite(a)) return "--";
  const imp = Number(improvement);
  const suffix = Number.isFinite(imp) ? ` / Δ ${fmtSignedFixed(imp, 1)} ${unit === "%" ? "pp" : "dB"}` : "";
  return `${b.toFixed(1)} ${unit} -> ${a.toFixed(1)} ${unit}${suffix}`;
}

function fmtSignedFixed(value, digits = 1) {
  const n = Number(value);
  if (!Number.isFinite(n)) return "--";
  return `${n >= 0 ? "+" : ""}${n.toFixed(digits)}`;
}

function liveSystemSeconds(field, fallbackField) {
  const sys = state.system || {};
  const raw = sys[field] ?? (fallbackField ? sys[fallbackField] : undefined);
  const base = Number(raw || 0);
  if (!base) return 0;
  const loadedAt = Number(sys._loadedAtMs || Date.now());
  return base + Math.max(0, Math.floor((Date.now() - loadedAt) / 1000));
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
  const uniqueGroups = [...new Set(groups.map((g) => Number(g)).filter((g) => Number.isFinite(g)))];
  const summary =
    uniqueGroups.length > 1
      ? `<span class="group-summary">Scan list <strong>${uniqueGroups.length}</strong></span>`
      : '<span class="group-summary">Group</span>';
  const chips = uniqueGroups.map((g) => `<span class="pill blue">${esc(g)}</span>`).join("");
  return `<span class="group-list${uniqueGroups.length > 1 ? " is-scan-list" : ""}">${summary}<span class="group-chips">${chips}</span></span>`;
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

function callInHangtime(call) {
  return !!call?._hangUntilMs && call._hangUntilMs > Date.now();
}

function activeCalls() {
  return sortedCalls().filter((call) => !callInHangtime(call));
}

function normalizedSpeakerIssi(call, value) {
  const issi = Number(value || 0);
  if (!validLookupIssi(issi)) return null;
  if (call?.call_type === "group" && Number(call.gssi || 0) === issi) return null;
  return issi;
}

function recentGroupSpeakerForCall(call) {
  if (call?.call_type !== "group" || !call.gssi) return null;
  const heard = state.heard.find(
    (entry) =>
      entry?.activity === "call_group" &&
      Number(entry.dest || 0) === Number(call.gssi || 0) &&
      normalizedSpeakerIssi(call, entry.issi)
  );
  return heard ? normalizedSpeakerIssi(call, heard.issi) : null;
}

function callAgeSeconds(call) {
  const startedMs = Number(call?._startedMs || 0);
  if (startedMs > 0) return Math.max(0, Math.floor((Date.now() - startedMs) / 1000));
  return Math.max(0, Math.floor(call?.started_secs_ago || 0));
}

function callMode(call) {
  if (callInHangtime(call)) {
    return { label: "hangtime", className: "amber" };
  }
  if (call?.call_type === "group") {
    return { label: "group", className: "blue" };
  }
  if (call?.simplex) {
    return { label: "simplex", className: "amber" };
  }
  return { label: "duplex", className: "green" };
}

function callTargetHtml(call) {
  if (call?.call_type === "group") return `TG ${esc(call.gssi || "--")}`;
  if (call?.called_issi) return radioIdentityHtml(call.called_issi);
  return "--";
}

function callCallingPartyHtml(call) {
  return call?.caller_issi ? radioIdentityHtml(call.caller_issi) : "--";
}

function callCardPartyHtml(issi) {
  return issi ? activeCallIdentityHtml(issi) : "--";
}

function callCalledPartyHtml(call) {
  if (call?.call_type === "group") return callTargetHtml(call);
  return call?.called_issi ? radioIdentityHtml(call.called_issi) : "--";
}

function callSpeakerHtml(call) {
  const speaker = liveSpeakerIssi(call);
  if (speaker && !callInHangtime(call)) return radioIdentityHtml(speaker);
  if (speaker) {
    return `<span class="identity muted"><span class="identity-primary">last speaker</span>${radioIdentityHtml(speaker)}</span>`;
  }
  return "--";
}

function liveSpeakerIssi(call) {
  return (
    normalizedSpeakerIssi(call, call?.active_speaker) ||
    recentGroupSpeakerForCall(call) ||
    (callInHangtime(call) ? normalizedSpeakerIssi(call, call?._lastSpeaker) : null) ||
    normalizedSpeakerIssi(call, call?.caller_issi) ||
    null
  );
}

function instantSpeakerHtml(call) {
  const issi = liveSpeakerIssi(call);
  if (!issi) return '<span class="call-identity speaker-now muted"><span class="call-identity-callsign">--</span><span class="call-identity-name">no active speaker</span><span class="call-identity-issi speaker-issi">ISSI --</span></span>';
  return activeCallIdentityHtml(issi, { speaker: true, muted: callInHangtime(call) });
}

const WORLD_FLAG = { flag: "🌐", code: "WW", label: "Worldwide" };
const COUNTRY_NAME_OVERRIDES = {
  XK: "Kosovo",
  WW: "Worldwide",
};
const MCC_TO_ISO = {
  202: "GR", 204: "NL", 206: "BE", 208: "FR", 212: "MC", 213: "AD", 214: "ES", 216: "HU", 218: "BA",
  219: "HR", 220: "RS", 221: "XK", 222: "IT", 225: "VA", 226: "RO", 228: "CH", 230: "CZ", 231: "SK",
  232: "AT", 234: "GB", 235: "GB", 238: "DK", 240: "SE", 242: "NO", 244: "FI", 246: "LT", 247: "LV",
  248: "EE", 250: "RU", 255: "UA", 257: "BY", 259: "MD", 260: "PL", 262: "DE", 266: "GI", 268: "PT",
  270: "LU", 272: "IE", 274: "IS", 276: "AL", 278: "MT", 280: "CY", 282: "GE", 283: "AM", 284: "BG",
  286: "TR", 288: "FO", 289: "GE", 290: "GL", 292: "SM", 293: "SI", 294: "MK", 295: "LI", 297: "ME",
  302: "CA", 308: "PM", 310: "US", 311: "US", 312: "US", 313: "US", 314: "US", 315: "US", 316: "US",
  330: "PR", 332: "VI", 334: "MX", 338: "JM", 340: "GP", 342: "BB", 344: "AG", 346: "KY", 348: "VG",
  350: "BM", 352: "GD", 354: "MS", 356: "KN", 358: "LC", 360: "VC", 362: "CW", 363: "AW", 364: "BS",
  365: "AI", 366: "DM", 368: "CU", 370: "DO", 372: "HT", 374: "TT", 376: "TC",
  400: "AZ", 401: "KZ", 402: "BT", 404: "IN", 405: "IN", 406: "IN", 410: "PK", 412: "AF", 413: "LK",
  414: "MM", 415: "LB", 416: "JO", 417: "SY", 418: "IQ", 419: "KW", 420: "SA", 421: "YE", 422: "OM",
  424: "AE", 425: "IL", 426: "BH", 427: "QA", 428: "MN", 429: "NP", 430: "AE", 431: "AE", 432: "IR",
  434: "UZ", 436: "TJ", 437: "KG", 438: "TM", 440: "JP", 441: "JP", 450: "KR", 452: "VN", 454: "HK",
  455: "MO", 456: "KH", 457: "LA", 460: "CN", 461: "CN", 466: "TW", 467: "KP", 470: "BD", 472: "MV",
  502: "MY", 505: "AU", 510: "ID", 514: "TL", 515: "PH", 520: "TH", 525: "SG", 528: "BN", 530: "NZ",
  536: "NR", 537: "PG", 539: "TO", 540: "SB", 541: "VU", 542: "FJ", 543: "WF", 544: "AS", 545: "KI",
  546: "NC", 547: "PF", 548: "CK", 549: "WS", 550: "FM", 551: "MH", 552: "PW", 553: "TV", 554: "TK",
  555: "NU",
  602: "EG", 603: "DZ", 604: "MA", 605: "TN", 606: "LY", 607: "GM", 608: "SN", 609: "MR", 610: "ML",
  611: "GN", 612: "CI", 613: "BF", 614: "NE", 615: "TG", 616: "BJ", 617: "MU", 618: "LR", 619: "SL",
  620: "GH", 621: "NG", 622: "TD", 623: "CF", 624: "CM", 625: "CV", 626: "ST", 627: "GQ", 628: "GA",
  629: "CG", 630: "CD", 631: "AO", 632: "GW", 633: "SC", 634: "SD", 635: "RW", 636: "ET", 637: "SO",
  638: "DJ", 639: "KE", 640: "TZ", 641: "UG", 642: "BI", 643: "MZ", 645: "ZM", 646: "MG", 647: "RE",
  648: "ZW", 649: "NA", 650: "MW", 651: "LS", 652: "BW", 653: "SZ", 654: "KM", 655: "ZA", 657: "ER",
  658: "SH", 659: "SS",
  702: "BZ", 704: "GT", 706: "SV", 708: "HN", 710: "NI", 712: "CR", 714: "PA", 716: "PE", 722: "AR",
  724: "BR", 730: "CL", 732: "CO", 734: "VE", 736: "BO", 738: "GY", 740: "EC", 742: "GF", 744: "PY",
  746: "SR", 748: "UY", 750: "FK",
  901: "WW", 991: "WW",
};

let regionNameFormatter = null;
let englishRegionNameFormatter = null;

function countryName(code) {
  if (COUNTRY_NAME_OVERRIDES[code]) return COUNTRY_NAME_OVERRIDES[code];
  try {
    regionNameFormatter ||= new Intl.DisplayNames([navigator.language || "en"], { type: "region" });
    return regionNameFormatter.of(code) || code;
  } catch {
    return code;
  }
}

function englishCountryName(code) {
  if (COUNTRY_NAME_OVERRIDES[code]) return COUNTRY_NAME_OVERRIDES[code];
  try {
    englishRegionNameFormatter ||= new Intl.DisplayNames(["en"], { type: "region" });
    return englishRegionNameFormatter.of(code) || code;
  } catch {
    return code;
  }
}

function flagForIso(code) {
  if (code === "WW") return WORLD_FLAG.flag;
  if (!/^[A-Z]{2}$/.test(code)) return "";
  return String.fromCodePoint(...[...code].map((char) => 0x1f1e6 + char.charCodeAt(0) - 65));
}

function countryFromIso(code) {
  if (!code) return null;
  if (code === "WW") return WORLD_FLAG;
  return { flag: flagForIso(code), code, label: countryName(code) };
}

function normalizedCountryName(value) {
  return String(value || "")
    .trim()
    .toLowerCase()
    .replace(/&/g, "and")
    .replace(/[^a-z0-9]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function countryByName(name) {
  const normalized = normalizedCountryName(name);
  if (!normalized) return null;
  for (const code of new Set(Object.values(MCC_TO_ISO))) {
    if (normalizedCountryName(englishCountryName(code)) === normalized || normalizedCountryName(countryName(code)) === normalized) {
      return countryFromIso(code);
    }
  }
  return null;
}

function countryByRadioId(issi) {
  if (!validLookupIssi(issi)) return null;
  const entry = radioId.cache.get(normalizeIssi(issi));
  if (!entry || !validRadioIdCacheEntry(entry) || entry.missing) return null;
  return countryByName(entry.country);
}

function countryByNumericPrefix(value) {
  const raw = String(value || "").replace(/\D/g, "");
  if (raw === "91") return WORLD_FLAG;
  if (raw.length < 3) return null;
  return countryFromIso(MCC_TO_ISO[Number(raw.slice(0, 3))]) || null;
}

function callCountryCandidates(call) {
  const speaker = liveSpeakerIssi(call);
  if (call?.call_type === "group") {
    return [speaker, recentGroupSpeakerForCall(call), call?.caller_issi, call?.active_speaker, call?.called_issi, call?.gssi];
  }
  return [speaker, call?.caller_issi, call?.called_issi, call?.gssi];
}

function callCountry(call) {
  for (const value of callCountryCandidates(call)) {
    const radioIdCountry = countryByRadioId(value);
    if (radioIdCountry) return radioIdCountry;
    const country = countryByNumericPrefix(value);
    if (country) return country;
  }
  const mcc = state.site?.config?.network?.mcc;
  return countryFromIso(MCC_TO_ISO[Number(mcc)]) || { flag: "", code: "--", label: "Unknown country" };
}

function callCountryHtml(call) {
  const country = callCountry(call);
  return `<span class="call-country" title="${esc(country.label)}"><span class="flag" aria-hidden="true">${esc(country.flag)}</span><span class="code">${esc(country.code)}</span></span>`;
}

function callSlotHtml(call) {
  const primary = call?.ts ? `TS${esc(call.ts)}` : "TS--";
  const secondary = call?.secondary_ts ? `+TS${esc(call.secondary_ts)}` : "";
  return `<span class="call-ts">${primary}${secondary}</span>`;
}

function callCardHtml(call) {
  const mode = callMode(call);
  if (call?.call_type !== "group") {
    return `
      <article class="active-call-card mode-${esc(mode.className)}">
        <div class="call-card-top">
          ${callCountryHtml(call)}
          <span class="pill ${mode.className}">${esc(mode.label)}</span>
          ${callSlotHtml(call)}
        </div>
        <div class="call-card-main">
          <span class="call-label">Called party</span>
          <strong>${callCardPartyHtml(call.called_issi)}</strong>
        </div>
        <div class="call-card-grid private-call-grid">
          <div>
            <span>Calling party</span>
            <strong>${callCardPartyHtml(call.caller_issi)}</strong>
          </div>
          <div>
            <span>Bearer</span>
            <strong>${call?.secondary_ts ? "2 TS" : "1 TS"}</strong>
          </div>
          <div class="call-card-time">
            <span>Call time</span>
            <strong data-call-seconds="${esc(call.call_id)}">${callAgeSeconds(call)}s</strong>
          </div>
        </div>
      </article>
    `;
  }

  const speakerLabel = callInHangtime(call) ? "Last speaker" : "Speaker";
  return `
    <article class="active-call-card mode-${esc(mode.className)}">
      <div class="call-card-top">
        ${callCountryHtml(call)}
        <span class="pill ${mode.className}">${esc(mode.label)}</span>
        ${callSlotHtml(call)}
      </div>
      <div class="call-card-main">
        <span class="call-label">Target</span>
        <strong>${callTargetHtml(call)}</strong>
      </div>
      <div class="call-card-grid group-call-grid">
        <div class="call-card-speaker">
          <span>${esc(speakerLabel)}</span>
          <strong>${instantSpeakerHtml(call)}</strong>
        </div>
        <div class="call-card-time">
          <span>Call time</span>
          <strong data-call-seconds="${esc(call.call_id)}">${callAgeSeconds(call)}s</strong>
        </div>
      </div>
    </article>
  `;
}

function renderStatus() {
  const core = coreHealth();
  setText("coreState", core.label);
  setText("diagramCoreState", core.label);
  setText("railConsoleState", core.label);
  setIndustrialTone("mapCoreMeter", core.className);
  setIndustrialTone("nodeScheduler", core.className);
  setIndustrialTone("nodeOutput", core.className);
  const coreNode = $("coreState");
  if (coreNode) {
    coreNode.classList.toggle("ok", core.className === "ok");
    coreNode.classList.toggle("warn", core.className === "warn");
    coreNode.classList.toggle("bad", core.className === "bad");
  }
  const railConsoleNode = $("railConsoleState");
  if (railConsoleNode) {
    railConsoleNode.classList.toggle("ok", core.className === "ok");
    railConsoleNode.classList.toggle("warn", core.className === "warn");
    railConsoleNode.classList.toggle("bad", core.className === "bad");
  }
  setStatusTone("systemStatusStrip", core.className);

  setText("brewState", state.brewOnline ? `BREW v${state.brewVersion || 1}` : "LOCAL");
  setText("diagramBrewState", state.brewOnline ? `BREW v${state.brewVersion || 1}` : "LOCAL");
  setIndustrialTone("nodeBrew", state.brewOnline ? "ok" : "warn");
  setClass("brewState", "ok", state.brewOnline);
  setClass("brewState", "warn", !state.brewOnline);
  setStatusTone("brewStatusStrip", state.brewOnline ? "ok" : "warn");

  const rfOnline = !!(state.site?.config?.available || state.txVisual || state.sdrHealth);
  const carrierInhibited = effectiveCarrierInhibited();
  const rfTone = carrierInhibited ? "bad" : rfOnline ? "ok" : "warn";
  setText("rfState", carrierInhibited ? "INHIBITED" : rfOnline ? "READY" : "WAITING");
  setText("diagramRfState", carrierInhibited ? "INHIBITED" : rfOnline ? "READY" : "WAITING");
  setText("diagramAntennaState", carrierInhibited ? "carrier inhibited" : rfOnline ? "radiating carrier" : "RF path muted");
  setText(
    "diagramPathToggleState",
    state.rfCarrierBusy ? "COMMAND PENDING" : carrierInhibited ? "CARRIER INHIBITED" : rfOnline ? "CARRIER ACTIVE" : "CONTROL UNAVAILABLE"
  );
  setIndustrialTone("mapRfMeter", rfTone);
  setIndustrialTone("nodeRfDevice", rfTone);
  setIndustrialTone("nodePhy", rfTone);
  setIndustrialTone("diagramPathToggle", state.rfCarrierBusy ? "warn" : rfTone);
  setClass("diagramPathToggle", "is-on", rfOnline && !carrierInhibited);
  setClass("rfState", "ok", rfOnline && !carrierInhibited);
  setClass("rfState", "warn", !rfOnline && !carrierInhibited);
  setClass("rfState", "bad", carrierInhibited);
  const carrierButton = $("diagramPathToggle");
  if (carrierButton) {
    carrierButton.disabled = state.rfCarrierBusy || !state.site?.config?.available;
    carrierButton.setAttribute("aria-pressed", carrierInhibited ? "true" : "false");
  }
  setStatusTone("rfOpsStatusStrip", rfTone);

  const lastOkAge = state.lastHttpOkMs ? Math.floor((Date.now() - state.lastHttpOkMs) / 1000) : null;
  const networkCore = inferNetworkCore();
  setText("networkCoreState", networkCore.label);
  setText("networkCoreHint", networkCore.hint);
  setText("railTelemetryState", lastOkAge === null ? state.wsState.toUpperCase() : `${lastOkAge}s`);
  setClass("networkCoreState", "ok", networkCore.tone === "ok");
  setClass("networkCoreState", "warn", networkCore.tone === "warn");
  setClass("railTelemetryState", "ok", lastOkAge !== null && lastOkAge <= 5);
  setClass("railTelemetryState", "warn", lastOkAge === null || lastOkAge > 5);
  setStatusTone("networkStatusStrip", networkCore.tone);
}

function renderMetrics() {
  const activeCount = activeCalls().length;
  setText("metricRadios", state.radios.size);
  setText("metricCalls", activeCount);
  setText("metricHeard", state.heard.length);
  setText("metricCpu", state.system?.cpu_pct !== undefined ? `${state.system.cpu_pct}%` : "--");
  setText("callCountLabel", `${activeCount} active`);
}

function renderRadios() {
  const rows = sortedRadios().map((radio) => `
    <tr>
      <td>${radioIdentityHtml(radio.issi)}</td>
      <td class="subscriber-groups">${groupLabel(radio.groups)}</td>
      <td>${rssiLabel(radio.rssi_dbfs)}</td>
      <td><span class="pill ${radio.energy_saving_mode ? "amber" : "green"}">${esc(eeLabel(radio))}</span></td>
      <td>${esc(radioLastSeen(radio))}</td>
    </tr>
  `);
  const html = rowsOrEmpty(rows, 5, "No registered radios");
  setHtml("radiosTable", html);
}

function renderCalls() {
  const currentCalls = activeCalls().sort((a, b) => {
    const ats = Number(a?.ts || 99);
    const bts = Number(b?.ts || 99);
    if (ats !== bts) return ats - bts;
    return Number(a?.call_id || 0) - Number(b?.call_id || 0);
  });
  const overview = currentCalls.map((call) => callCardHtml(call));
  const board = $("overviewCalls");
  if (board) board.dataset.callCount = String(Math.min(Math.max(currentCalls.length, 0), 3));
  setHtml("overviewCalls", overview.length ? overview.join("") : '<div class="empty-call-board">No active calls</div>');
  renderSlots();
}

function activityMeta(activity) {
  switch (activity) {
    case "call_group":
      return { label: "Group voice", className: "blue" };
    case "call_individual":
    case "call_p2p_simplex":
      return { label: "P2P simplex", className: "green" };
    case "call_p2p_duplex":
      return { label: "P2P duplex", className: "amber" };
    case "sds":
      return { label: "SDS", className: "amber" };
    default:
      return { label: activity || "--", className: "" };
  }
}

function activityHtml(activity) {
  const meta = activityMeta(activity);
  return `<span class="pill ${esc(meta.className)}">${esc(meta.label)}</span>`;
}

function heardRows(limit) {
  return state.heard.slice(0, limit).map((entry) => `
    <tr>
      <td>${esc(entry.ts || "--")}</td>
      <td>${radioIdentityHtml(entry.issi)}</td>
      <td>${activityHtml(entry.activity)}</td>
      <td>${destinationHtml(entry)}</td>
    </tr>
  `);
}

function renderHeard() {
  const shown = Math.min(state.heard.length, 30);
  setText("overviewHeardLabel", `${shown} shown, ${radioId.cache.size} RadioID cached`);
  setHtml("overviewHeard", rowsOrEmpty(heardRows(30), 4, "No recent voice or SDS activity"));
}

function renderLogs(options = {}) {
  setText("logCount", `${state.logs.length} lines`);
  const list = $("logList");
  const previousScrollTop = list ? list.scrollTop : 0;
  const rows = state.logs.slice(-500).map((entry) => {
    const level = String(entry.level || "INFO").toLowerCase();
    const cls = level.includes("error") ? "error" : level.includes("warn") ? "warn" : "";
    return `<div class="log-row ${cls}">
      <span>${esc(entry.ts || "--")}</span>
      <span class="level">${esc(entry.level || "INFO")}</span>
      <span class="msg">${esc(entry.msg || "")}</span>
    </div>`;
  });
  setHtml("logList", rows.length ? rows.join("") : '<div class="log-row"><span>--</span><span class="level">INFO</span><span class="msg">No log entries</span></div>');
  const scrollButton = $("logAutoScrollBtn");
  if (scrollButton) {
    scrollButton.textContent = state.logAutoScroll ? "Pause" : "Play";
    scrollButton.setAttribute("aria-pressed", state.logAutoScroll ? "true" : "false");
  }
  for (const id of ["logClearBtn", "logExportBtn", "logAutoScrollBtn"]) {
    const button = $(id);
    if (button) button.disabled = !!state.logBusy;
  }
  if (state.activePage === "logs" && list && (state.logAutoScroll || options.forceBottom)) {
    list.scrollTop = list.scrollHeight;
  } else if (state.activePage === "logs" && list) {
    list.scrollTop = Math.min(previousScrollTop, Math.max(0, list.scrollHeight - list.clientHeight));
  }
}

function logTimestampForFile(date = new Date()) {
  const pad = (value) => String(value).padStart(2, "0");
  return `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}-${pad(date.getHours())}${pad(date.getMinutes())}${pad(date.getSeconds())}`;
}

function logsAsText() {
  return state.logs
    .map((entry) => `${entry.ts || "--"} ${entry.level || "INFO"} ${entry.msg || ""}`)
    .join("\n") + (state.logs.length ? "\n" : "");
}

function exportLogs() {
  const blob = new Blob([logsAsText()], { type: "text/plain;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = `Log${logTimestampForFile()}.log`;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

async function clearLogs() {
  if (state.logBusy) return;
  state.logBusy = true;
  renderLogs();
  try {
    const res = await fetch("/api/logs/clear", {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
    });
    if (!res.ok) throw new Error(await res.text());
    state.logs = [];
  } catch (error) {
    state.logs.push({
      ts: new Date().toLocaleTimeString(),
      level: "WARN",
      msg: `clear logs failed: ${String(error.message || error).slice(0, 100)}`,
    });
  } finally {
    state.logBusy = false;
    renderLogs();
  }
}

function slotCall(ts) {
  return activeCalls().find((call) => Number(call.ts) === Number(ts) || Number(call.secondary_ts) === Number(ts));
}

function renderSlots() {
  const siteSlots = Array.isArray(state.site?.timeslots) ? state.site.timeslots : [];
  const rows = [1, 2, 3, 4].map((ts) => {
    const call = slotCall(ts);
    const siteSlot = siteSlots.find((slot) => Number(slot.ts) === ts) || {};
    const recentVoice = state.slotActivity.get(ts) && Date.now() - state.slotActivity.get(ts) <= SLOT_ACTIVITY_MS;
    const stateLabel = ts === 1 ? "control" : call ? "traffic" : recentVoice ? "voice" : siteSlot.state || "idle";
    const target = call ? callTargetHtml(call) : ts === 1 ? "MCCH / signalling" : siteSlot.owner ? `${esc(siteSlot.owner)} owned` : "available";
    const speaker = call?.active_speaker ? radioIdentityHtml(call.active_speaker) : "--";
    return `
      <div class="slot-card ${stateLabel}">
        <div class="slot-top"><strong>TS${ts}</strong><span>${esc(stateLabel)}</span></div>
        <div class="slot-body">
          <div>${target}</div>
          <small>${call ? `call ${esc(call.call_id)} / ${speaker}` : recentVoice ? "recent voice activity" : "idle"}</small>
        </div>
      </div>
    `;
  });
  setHtml("slotGrid", rows.join(""));
}

function renderSite() {
  const site = state.site || {};
  const cfg = site.config || {};
  const cell = cfg.cell || {};
  const soapy = cfg.soapy || {};
  const cached = site.rf_cached || {};
  const txVisual = state.txVisual || cached.tx_visual || {};
  const txQuality = state.txQuality || cached.tx_quality || {};
  const sdrHealth = state.sdrHealth || cached.sdr_health || {};
  const sysHealth = state.sysHealth || cached.sys_health || {};
  const rfAdvice = state.site?.rf_profile_advice || {};
  const carrierInhibited = !!cfg.rf_control?.carrier_inhibited;
  const services = cfg.services || {};

  const txHz = soapy.tx_hz ?? cell.derived_dl_hz;
  const rxHz = soapy.rx_hz ?? cell.derived_ul_hz;
  const shift = Number.isFinite(Number(txHz)) && Number.isFinite(Number(rxHz)) ? Number(txHz) - Number(rxHz) : null;

  setText("rfTxFreq", fmtHz(txHz));
  setText("rfRxFreq", fmtHz(rxHz));
  setText("rfDuplexShift", shift === null ? "--" : fmtDuplexShift(shift, cell.duplex_spacing_id));
  setText("rfBandCarrier", cell.freq_band !== undefined ? `band ${cell.freq_band}, carrier ${cell.main_carrier}` : "--");
  setText("rfMccMnc", cfg.network ? `${cfg.network.mcc} / ${cfg.network.mnc}` : "--");
  setText("rfLocationArea", cell.location_area ?? "--");
  setText("rfColourCode", cell.colour_code ?? "--");
  setText("rfFrame18", compactBool(cell.frame_18_ext));
  setText("rfFreqMatch", soapy.frequency_match ? "matched" : cfg.available ? "check" : "--");
  setText("diagramFrequency", txHz ? fmtHz(txHz) : "--");
  setText(
    "diagramNetwork",
    cfg.network ? `${cfg.network.mcc}-${cfg.network.mnc} / LA ${cell.location_area ?? "--"} / CC ${cell.colour_code ?? "--"}` : "--"
  );
  setText("diagramProgram", cell.main_carrier !== undefined ? `carrier ${cell.main_carrier}` : "local carrier");

  const sdrName = state.system?.sdr_name && state.system.sdr_name !== "unknown" ? state.system.sdr_name : "";
  setText("rfDevice", sdrName || soapy.device || "auto");
  setText("diagramRfDevice", sdrName || soapy.device || "auto");
  setText(
    "diagramPhyState",
    carrierInhibited
      ? "TX inhibited, RX monitor"
      : txQuality.evm_pct !== undefined
        ? `pi/4-DQPSK, DSP EVM est. ${fmtPct(txQuality.evm_pct)}`
        : "pi/4-DQPSK, waiting TX"
  );
  setText(
    "diagramServicesState",
    [
      services.voice ? "CMCE voice" : "voice off",
      "MM attach",
      `SDS q=${services.sds_live_queue_len ?? 0}`,
      services.wx_service ? "WX SDS" : null,
    ]
      .filter(Boolean)
      .join(", ")
  );
  setText("rfSampleRate", soapy.sample_rate ? fmtHz(soapy.sample_rate, "kHz") : txVisual.sample_rate ? fmtHz(txVisual.sample_rate, "kHz") : "--");
  setText("rfPpm", soapy.ppm_err !== undefined ? `${Number(soapy.ppm_err).toFixed(2)} ppm` : "--");
  setText("rfReferenceClock", txQuality.reference_clock || soapy.reference_clock || "internal");
  setText("rfTxCorrected", fmtHz(soapy.tx_corrected_hz ?? txHz));
  setText("rfRxCorrected", fmtHz(soapy.rx_corrected_hz ?? rxHz));
  setText(
    "rfPpmErrorHz",
    soapy.tx_ppm_error_hz !== undefined || soapy.rx_ppm_error_hz !== undefined
      ? `TX ${fmtHz(soapy.tx_ppm_error_hz || 0)} / RX ${fmtHz(soapy.rx_ppm_error_hz || 0)}`
      : txQuality.frequency_error_hz !== undefined
        ? fmtHz(txQuality.frequency_error_hz)
        : "--"
  );
  setText("rfChannels", `RX ${soapy.rx_ch ?? "auto"} / TX ${soapy.tx_ch ?? "auto"}`);
  setText("rfAntennas", `RX ${soapy.rx_ant || "auto"} / TX ${soapy.tx_ant || "auto"}`);
  setText("rfTxGainProfile", txQuality.tx_gain_profile || soapy.tx_gain_profile || "nominal_clean");
  setText("rfTxGains", gainsLabel(sdrHealth.tx_gains || soapy.tx_gains));
  setText("rfRxGains", gainsLabel(sdrHealth.rx_gains || soapy.rx_gains));
  setText("rfTemp", sdrHealth.temperature_c !== null && sdrHealth.temperature_c !== undefined ? `${Number(sdrHealth.temperature_c).toFixed(1)} C` : state.system?.cpu_temp_c ? `${Number(state.system.cpu_temp_c).toFixed(1)} C host` : "--");

  setText("rfRms", fmtDb(txVisual.rms_dbfs, "dBFS"));
  setText("rfPeak", fmtDb(txVisual.peak_dbfs, "dBFS"));
  setText("rfEvm", txQuality.evm_pct !== undefined ? `${fmtPct(txQuality.evm_pct)} DSP` : "--");
  setText(
    "rfEvmGate",
    txQuality.evm_gate
      ? `${String(txQuality.evm_gate).toUpperCase()} / ${fmtPct(txQuality.evm_limit_pct)} DSP limit`
      : "--"
  );
  setText("rfPapr", fmtDb(txQuality.papr_db));
  setText("rfObw", fmtHz(txQuality.occupied_bandwidth_hz, "kHz"));
  setText("rfCarrierLeak", fmtDb(txQuality.carrier_leakage_db));
  setText(
    "rfTiming",
    txQuality.rf_timing_severity
      ? `${String(txQuality.rf_timing_severity).toUpperCase()} / late ${txQuality.rf_tx_late_events ?? 0}, rx lost ${txQuality.rf_rx_lost_events ?? 0}`
      : "--"
  );
  setText("rfAdvice", rfAdvice.summary || "--");
  setText("rfPower", sysHealth.total_power_w !== null && sysHealth.total_power_w !== undefined ? `${Number(sysHealth.total_power_w).toFixed(1)} W` : "--");
  setText("rfSnapshotAge", cfg.available ? "live config" : Object.keys(cached).length ? "cached" : "--");

  setText("settingsCarrier", cell.main_carrier !== undefined ? `${fmtHz(txHz)} / carrier ${cell.main_carrier}` : "--");
  setText("settingsNetworkCode", cfg.network ? `${cfg.network.mcc} / ${cfg.network.mnc}` : "--");
  setText("settingsColorCode", cell.colour_code ?? "--");
  setText("settingsTimeslots", "TS1 control, TS2-TS4 traffic");
  setText("settingsEnergyEconomy", cell.energy_saving_label || "auto");
  setText("settingsRadioIdEndpoint", radioIdEndpoint() || "disabled");
  setText("settingsConfigMode", cfg.available ? "live core config" : "unavailable");
  setText("adminAccessMode", "external API/WS");
  setText("adminUpdateChannel", state.updateCatalog?.check_failed ? "check failed" : "GitHub releases");
  setText("adminAuditSink", "volatile runtime log");
  setText("adminCoreProcess", "nexus-bs core service");
  setText("adminDashboardProcess", "nexus-bs-dashboard service");
  renderServiceControls();
  renderUpdatePanel();
  renderCalibration();
  setText("slotSummary", "TS1 control, TS2-TS4 traffic");
  setText("diagramSlotState", "TS1 control, TS2-TS4 traffic");
}

function renderUpdatePanel() {
  const catalog = state.updateCatalog || {};
  const releases = catalog.releases || [];
  const top = $("topUpdateBtn");
  const topState = $("topUpdateState");
  const statusText =
    state.updateStatus === "running"
      ? "Updating"
      : state.updateStatus === "update_available"
        ? "Update available"
        : state.updateStatus === "check_failed"
          ? "Check failed"
          : state.updateStatus === "done_err"
            ? "Update failed"
            : state.updateStatus === "done_ok"
              ? "Installed"
              : catalog.latest
                ? catalog.latest
                : "checking";
  if (topState) topState.textContent = statusText;
  if (top) {
    top.classList.toggle("has-update", state.updateStatus === "update_available");
    top.classList.toggle("is-unknown", state.updateStatus === "check_failed" || state.updateStatus === "done_err");
  }
  setText("updatePanelStatus", statusText);
  setText("updateCurrentVersion", catalog.current || state.system?.stack_version || "--");
  setText("updateLatestVersion", catalog.latest || "--");

  const select = $("updateReleaseSelect");
  if (select) {
    const selected = state.selectedUpdateUrl || select.value;
    select.innerHTML = releases.length
      ? releases
          .map((release) => {
            const relation = updateRelationLabel(release);
            const size = release.deb_size ? `, ${Math.round(release.deb_size / 1024 / 1024)} MB` : "";
            return `<option value="${esc(release.deb_url)}">${esc(release.tag)} - ${relation} (${esc(release.deb_asset_name)}${size})</option>`;
          })
          .join("")
      : `<option value="">No .deb releases found</option>`;
    select.value = releases.some((release) => release.deb_url === selected) ? selected : releases[0]?.deb_url || "";
    state.selectedUpdateUrl = select.value;
  }
  const selectedRelease = selectedUpdateRelease();
  const apply = $("updateApplyBtn");
  if (apply) {
    apply.disabled = state.updateBusy || !selectedRelease;
    apply.textContent = selectedRelease?.relation === "older" ? "Downgrade" : selectedRelease?.relation === "current" ? "Reinstall" : "Apply Update";
  }
  const check = $("updateRefreshBtn");
  if (check) check.disabled = state.updateBusy;
  setText("updateLog", state.updateLog || (catalog.check_failed ? catalog.error : "No update activity."));
}

function renderSystem() {
  const sys = state.system || {};
  setText("subtitle", sys.stack_version || "Nexus-BS local TETRA eBTS");
  setText("hostName", sys.hostname || "--");
  setText("productName", sys.product_name && sys.product_version_tag ? `${sys.product_name} ${sys.product_version_tag}` : "--");
  setText("cpuName", sys.cpu_model ? `${sys.cpu_model}${sys.cpu_cores ? ` (${sys.cpu_cores} cores)` : ""}` : "--");
  setText("memoryUse", sys.ram_total_mb ? `${sys.ram_used_mb || 0} / ${sys.ram_total_mb} MB` : "--");
  setText("cpuTemp", sys.cpu_temp_c !== null && sys.cpu_temp_c !== undefined ? `${Number(sys.cpu_temp_c).toFixed(1)} C` : "--");
  renderSystemUptime();
  setText("activeConfigPath", sys.active_config_name || sys.active_config_path || sys.config_path || "--");
  setText("configPath", sys.config_dir || sys.config_path || "--");
  setText("sdrName", sys.sdr_name || "--");
  setText("soapyInfo", sys.soapy_info || "--");
  setText("aboutVersion", sys.product_version_tag || sys.stack_version || "--");
  renderSite();
}

function renderSystemUptime() {
  setText("bsUptime", state.system?.bs_uptime_secs !== undefined ? fmtDurationSecs(liveSystemSeconds("bs_uptime_secs")) : "--");
  setText(
    "hostUptime",
    state.system?.host_uptime_secs !== undefined || state.system?.uptime_secs !== undefined
      ? fmtDurationSecs(liveSystemSeconds("host_uptime_secs", "uptime_secs"))
      : "--"
  );
}

function renderCallSeconds() {
  for (const node of document.querySelectorAll("[data-call-seconds]")) {
    const call = state.calls.get(Number(node.dataset.callSeconds));
    node.textContent = call ? `${callAgeSeconds(call)}s` : "--";
  }
}

function renderLiveTick() {
  renderStatus();
  renderCallSeconds();
  renderSystemUptime();
  renderSlots();
}

function renderAll() {
  renderStatus();
  renderMetrics();
  renderRadios();
  renderCalls();
  renderHeard();
  if (state.activePage === "logs") renderLogs();
  renderSystem();
  renderConfigProfiles();
  renderWifi();
}

function reconcileCalls(incomingCalls, options = {}) {
  const now = Date.now();
  const incomingCallIds = new Set(incomingCalls.map((call) => call.call_id));
  const retainedHangtimeCalls = new Map();
  if (options.preserveHangtime) {
    for (const [callId, call] of state.calls) {
      if (!incomingCallIds.has(callId) && call?._hangUntilMs && call._hangUntilMs > now) {
        retainedHangtimeCalls.set(callId, call);
      }
    }
  }

  state.calls.clear();
  for (const [callId, timer] of state.callCleanupTimers) {
    if (!retainedHangtimeCalls.has(callId)) {
      clearTimeout(timer);
      state.callCleanupTimers.delete(callId);
    }
  }
  for (const [callId, call] of retainedHangtimeCalls) {
    state.calls.set(callId, call);
  }
  for (const call of incomingCalls) {
    upsertCall(call, { identityRefresh: !options.quiet });
  }
}

function applySnapshot(msg, options = {}) {
  state.radios.clear();
  reconcileCalls(msg.calls || [], options);
  state.heard = msg.last_heard || [];
  state.logs = msg.log || [];
  state.txVisual = msg.last_tx_visual || state.txVisual;
  state.txQuality = msg.last_tx_quality || state.txQuality;
  state.sdrHealth = msg.last_sdr_health || state.sdrHealth;
  state.sysHealth = msg.last_sys_health || state.sysHealth;
  for (const radio of msg.ms || []) {
    state.radios.set(radio.issi, { ...radio, _lastSeenMs: Date.now() - (radio.last_seen_secs_ago || 0) * 1000 });
  }
  state.brewOnline = !!msg.brew_online;
  state.brewVersion = msg.brew_version || 0;
}

function applyCallsSnapshot(msg, options = {}) {
  reconcileCalls(msg.calls || [], options);
  state.heard = msg.last_heard || state.heard;
  state.brewOnline = !!msg.brew_online;
  state.brewVersion = msg.brew_version || 0;
}

function ensureRadio(issi) {
  if (!state.radios.has(issi)) {
    state.radios.set(issi, { issi, groups: [], rssi_dbfs: null, energy_saving_mode: 0, last_seen_secs_ago: 0 });
  }
  return state.radios.get(issi);
}

function touchRegisteredRadio(issi, update) {
  const radio = state.radios.get(issi);
  if (!radio) return null;
  if (update) Object.assign(radio, update);
  radio._lastSeenMs = Date.now();
  return radio;
}

function pushHeard(entry) {
  if (!entry) return;
  if (entry.issi) touchRegisteredRadio(entry.issi);
  state.heard.unshift(entry);
  state.heard = state.heard.slice(0, 80);
}

function applyMessage(msg) {
  markWsMessage();
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
      if (msg.caller_issi) {
        const radio = touchRegisteredRadio(msg.caller_issi);
        if (radio && msg.call_type === "group" && msg.gssi && !(radio.groups || []).includes(msg.gssi)) {
          radio.groups = [...(radio.groups || []), msg.gssi];
        }
      }
      upsertCall({
        ...msg,
        active_speaker:
          Object.prototype.hasOwnProperty.call(msg, "active_speaker")
            ? msg.active_speaker
            : msg.call_type === "group"
              ? msg.caller_issi
              : null,
        _startedMs: Date.now(),
      });
      pushHeard(msg.last_heard);
      break;
    case "speaker_changed":
      if (msg.speaker_issi) {
        const radio = touchRegisteredRadio(msg.speaker_issi);
        if (radio && msg.gssi && !(radio.groups || []).includes(msg.gssi)) {
          radio.groups = [...(radio.groups || []), msg.gssi];
        }
      }
      if (state.calls.has(msg.call_id)) {
        const call = state.calls.get(msg.call_id);
        upsertCall({
          ...call,
          gssi: msg.gssi || call.gssi || msg.last_heard?.dest || 0,
          active_speaker: msg.speaker_issi,
          caller_issi: msg.speaker_issi,
        });
      } else {
        upsertCall({
          call_id: msg.call_id,
          call_type: "group",
          gssi: msg.gssi || msg.last_heard?.dest || 0,
          caller_issi: msg.speaker_issi,
          active_speaker: msg.speaker_issi,
          ts: msg.ts || "--",
          _startedMs: Date.now(),
        });
      }
      pushHeard(msg.last_heard);
      break;
    case "call_ended":
      endCall(msg);
      break;
    case "last_heard":
      pushHeard(msg.entry || msg);
      break;
    case "log":
      state.logs.push({ ts: msg.ts, level: msg.level, msg: msg.msg });
      state.logs = state.logs.slice(-500);
      if (state.activePage === "logs") renderLogs();
      return;
    case "ts_voice":
      state.slotActivity.set(Number(msg.ts), Date.now());
      break;
    case "tx_visual":
      state.txVisual = msg;
      break;
    case "tx_quality":
      state.txQuality = msg;
      break;
    case "sdr_health":
      state.sdrHealth = msg;
      break;
    case "sys_health":
      state.sysHealth = msg;
      break;
    default:
      break;
  }
  renderAll();
}

async function loadSystem() {
  try {
    const res = await fetch("/api/system", { credentials: "same-origin" });
    if (!res.ok) {
      markHttpFail();
      return;
    }
    markHttpOk();
    state.system = { ...(await res.json()), _loadedAtMs: Date.now() };
    renderAll();
  } catch {
    markHttpFail();
    // Keep last known values.
  }
}

async function loadSite(options = {}) {
  if (state.siteInflight && !options.force) return;
  if (options.force) state.siteInflight = false;
  state.siteInflight = true;
  try {
    const res = await fetchWithTimeout("/api/site", {
      credentials: "same-origin",
      cache: "no-store",
    }, SITE_FETCH_TIMEOUT_MS);
    if (!res.ok) {
      markHttpFail();
      return;
    }
    markHttpOk();
    state.site = await res.json();
    if (state.site?.rf_cached) {
      state.txVisual = state.site.rf_cached.tx_visual || state.txVisual;
      state.txQuality = state.site.rf_cached.tx_quality || state.txQuality;
      state.sdrHealth = state.site.rf_cached.sdr_health || state.sdrHealth;
      state.sysHealth = state.site.rf_cached.sys_health || state.sysHealth;
    }
    renderAll();
  } catch {
    markHttpFail();
  } finally {
    state.siteInflight = false;
  }
}

async function loadSnapshot() {
  if (state.snapshotInflight) return;
  state.snapshotInflight = true;
  try {
    const res = await fetch("/api/snapshot", {
      credentials: "same-origin",
      cache: "no-store",
    });
    if (!res.ok) {
      markHttpFail();
      return;
    }
    markHttpOk();
    applySnapshot(await res.json(), { preserveHangtime: true, quiet: true });
    renderAll();
  } catch {
    markHttpFail();
    // WebSocket events remain the primary live path; snapshot is only reconciliation.
  } finally {
    state.snapshotInflight = false;
  }
}

async function loadCallsSnapshot() {
  if (state.callsInflight) return;
  state.callsInflight = true;
  try {
    const msg = await fetchDashboardJson("/api/calls", {
      credentials: "same-origin",
      cache: "no-store",
    });
    markHttpOk();
    const key = callsPayloadKey(msg);
    if (key === state.callsPayloadKey) {
      renderLiveTick();
      return;
    }
    state.callsPayloadKey = key;
    applyCallsSnapshot(msg, { preserveHangtime: true, quiet: false });
    renderStatus();
    renderMetrics();
    renderCalls();
    renderHeard();
  } catch {
    markHttpFail();
    // WebSocket events remain primary; this path is a cheap one-second guard.
  } finally {
    state.callsInflight = false;
  }
}

function refreshDashboardData() {
  loadSystem();
  loadSite();
  loadSnapshot();
  loadCallsSnapshot();
  loadCalibrationStatus();
  loadConfigProfiles();
  loadWifiStatus();
  loadUpdateCatalog();
  loadEasyStartStatus();
}

function connectWs() {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const ws = new WebSocket(`${proto}//${location.host}/ws`);

  ws.addEventListener("open", () => {
    state.connected = true;
    state.wsConnected = true;
    state.wsState = "open";
    state.lastWsOpenMs = Date.now();
    state.lastWsMessageMs = Date.now();
    renderAll();
    loadSite({ force: true });
    loadSystem();
  });

  ws.addEventListener("message", (event) => {
    try {
      markWsMessage();
      applyMessage(JSON.parse(event.data));
    } catch {
      // Ignore malformed dashboard frames.
    }
  });

  ws.addEventListener("close", () => {
    state.connected = false;
    state.wsConnected = false;
    state.wsState = "reconnecting";
    state.lastWsCloseMs = Date.now();
    renderAll();
    setTimeout(connectWs, 1500);
  });

  ws.addEventListener("error", () => {
    state.connected = false;
    state.wsConnected = false;
    state.wsState = "reconnecting";
    state.lastWsCloseMs = Date.now();
    renderAll();
  });
}

function switchPage(page) {
  if (!pages[page]) return;
  state.activePage = page;
  for (const node of document.querySelectorAll(".page")) node.classList.remove("active");
  for (const node of document.querySelectorAll(".nav-item")) {
    const active = node.dataset.page === page;
    node.classList.toggle("active", active);
    if (active) {
      node.setAttribute("aria-current", "page");
    } else {
      node.removeAttribute("aria-current");
    }
  }
  $(`page-${page}`)?.classList.add("active");
  setText("pageTitle", pages[page] || "Overview");
  if (page === "logs") renderLogs();
  if (page === "settings") {
    loadUpdateStatus();
    if (!state.updateCatalog) loadUpdateCatalog();
  }
}

function initNav() {
  for (const node of document.querySelectorAll(".nav-item")) {
    node.addEventListener("click", () => switchPage(node.dataset.page));
  }
  $("topUpdateBtn")?.addEventListener("click", () => {
    switchPage("settings");
    loadUpdateCatalog();
  });
  $("configRefreshBtn")?.addEventListener("click", loadConfigProfiles);
  $("configLoadCurrentBtn")?.addEventListener("click", () => loadConfigText("config.toml"));
  $("configLoadSelectedBtn")?.addEventListener("click", () => loadConfigText(selectedConfigName()));
  $("configActivateBtn")?.addEventListener("click", activateSelectedConfig);
  $("configDuplicateBtn")?.addEventListener("click", duplicateSelectedConfig);
  $("configDeleteBtn")?.addEventListener("click", deleteSelectedConfig);
  $("configSaveBtn")?.addEventListener("click", saveConfigText);
  $("serviceRestartBtn")?.addEventListener("click", () => requestServiceAction("restart"));
  $("serviceShutdownBtn")?.addEventListener("click", () => requestServiceAction("shutdown"));
  $("serviceStopGoBtn")?.addEventListener("click", () => requestServiceAction("stopgo"));
  $("factoryResetBtn")?.addEventListener("click", openFactoryResetDialog);
  $("factoryResetCancelBtn")?.addEventListener("click", () => $("factoryResetModal")?.classList.add("hidden"));
  $("factoryResetConfirmBtn")?.addEventListener("click", requestFactoryReset);
  $("easyStartSkipBtn")?.addEventListener("click", skipEasyStartWizard);
  $("easyStartBackBtn")?.addEventListener("click", previousEasyStartStep);
  $("easyStartNextBtn")?.addEventListener("click", nextEasyStartStep);
  $("easyStartVerifyBtn")?.addEventListener("click", verifyEasyStartConfig);
  $("easyStartCommitBtn")?.addEventListener("click", commitEasyStartConfig);
  $("updateRefreshBtn")?.addEventListener("click", loadUpdateCatalog);
  $("updateApplyBtn")?.addEventListener("click", applySelectedUpdate);
  $("updateReleaseSelect")?.addEventListener("change", (event) => {
    state.selectedUpdateUrl = event.target.value || "";
    renderUpdatePanel();
  });
  $("calibrationRunBtn")?.addEventListener("click", requestTxCalibration);
  $("rfProfileApplyBtn")?.addEventListener("click", () => applySafeRfProfile());
  $("rfProfileHotspotBtn")?.addEventListener("click", () => applySafeRfProfile("hotspot"));
  $("rfProfileLowPowerBtn")?.addEventListener("click", () => applySafeRfProfile("low_power_basestation"));
  $("rfProfilePaBtn")?.addEventListener("click", () => applySafeRfProfile("power_amplified_basestation"));
  $("wifiScanBtn")?.addEventListener("click", scanWifiNetworks);
  $("wifiClearBtn")?.addEventListener("click", clearWifiScanList);
  $("wifiConnectBtn")?.addEventListener("click", connectWifiNetwork);
  $("wifiSsid")?.addEventListener("input", (event) => {
    state.wifiSelectedSsid = event.target.value || "";
  });
  $("wifiShowPassword")?.addEventListener("change", syncWifiPasswordVisibility);
  $("diagramPathToggle")?.addEventListener("click", requestRfCarrierToggle);
  $("logAutoScrollBtn")?.addEventListener("click", () => {
    state.logAutoScroll = !state.logAutoScroll;
    renderLogs({ forceBottom: state.logAutoScroll });
  });
  $("logExportBtn")?.addEventListener("click", exportLogs);
  $("logClearBtn")?.addEventListener("click", clearLogs);
  $("configProfileSelect")?.addEventListener("change", () => setConfigStatus(`selected ${selectedConfigName()}`));
  $("configFileName")?.addEventListener("input", (event) => {
    state.configEditorName = event.target.value;
    state.configEditorDirty = true;
    setConfigStatus("editing");
  });
  $("configEditor")?.addEventListener("input", (event) => {
    state.configEditorContent = event.target.value;
    state.configEditorDirty = true;
    setConfigStatus("editing");
  });
}

loadRadioIdCache();
initNav();
refreshDashboardData();
loadConfigText("config.toml");
connectWs();
openEasyStartWizardFromUrl();
setInterval(loadSystem, 15000);
setInterval(loadSite, SITE_REFRESH_MS);
setInterval(loadSnapshot, SNAPSHOT_REFRESH_MS);
setInterval(loadCallsSnapshot, CALLS_REFRESH_MS);
setInterval(() => {
  if (state.calibrationBusy || state.calibration?.active) loadCalibrationStatus();
}, 1000);
setInterval(loadCalibrationStatus, 30000);
setInterval(loadWifiStatus, 60000);
setInterval(renderLiveTick, 1000);
