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
  callsPayloadKey: "",
  configProfiles: [],
  configProfilesLoaded: false,
  configEditorName: "config.toml",
  configEditorLoadedName: "",
  configEditorContent: "",
  configEditorDirty: false,
  configEditorStatus: "idle",
  configBusy: false,
  serviceBusy: false,
  serviceStatus: "idle",
  logAutoScroll: true,
  logBusy: false,
  activePage: "system",
  pageScroll: new Map(),
  scrollRestoreFrame: 0,
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
const GROUP_CALL_HANGTIME_UI_MS = 12000;
const CALLS_REFRESH_MS = 1000;
const CALLS_FETCH_TIMEOUT_MS = 2500;
const SNAPSHOT_REFRESH_MS = 10000;
const SITE_REFRESH_MS = 10000;
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
  if (node) node.innerHTML = value ?? "";
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

function currentScrollY() {
  return window.scrollY || document.documentElement.scrollTop || document.body.scrollTop || 0;
}

function maxScrollY() {
  return Math.max(0, document.documentElement.scrollHeight - window.innerHeight);
}

function clampScrollY(value) {
  return Math.min(Math.max(0, Number(value) || 0), maxScrollY());
}

function rememberPageScroll(page = state.activePage) {
  if (!pages[page]) return;
  state.pageScroll.set(page, currentScrollY());
}

function restorePageScroll(page = state.activePage, fallback = 0) {
  if (!pages[page]) return;
  const target = state.pageScroll.has(page) ? state.pageScroll.get(page) : fallback;
  if (state.scrollRestoreFrame) cancelAnimationFrame(state.scrollRestoreFrame);
  state.scrollRestoreFrame = requestAnimationFrame(() => {
    state.scrollRestoreFrame = 0;
    window.scrollTo({ top: clampScrollY(target), left: 0, behavior: "auto" });
  });
}

function preserveActivePageScroll(renderFn) {
  const page = state.activePage;
  const y = currentScrollY();
  renderFn();
  if (state.activePage === page) restorePageScroll(page, y);
}

function esc(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

async function fetchDashboardJson(url, options = {}, timeoutMs = CALLS_FETCH_TIMEOUT_MS) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const res = await fetch(url, { ...options, signal: controller.signal });
    if (!res.ok) throw new Error(`${url} ${res.status}`);
    return await res.json();
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
  const ttl = entry.missing ? RADIOID_NEGATIVE_TTL_MS : RADIOID_CACHE_TTL_MS;
  return Date.now() - Number(entry.fetchedAt || 0) <= ttl;
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

function destinationHtml(entry) {
  const dest = entry?.dest;
  if (!dest) return "--";
  if (entry.activity === "call_group") return `GSSI ${esc(dest)}`;
  return radioIdentityHtml(dest);
}

function queueRadioIdLookup(issi, options = {}) {
  if (!validLookupIssi(issi)) return;
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

function fmtSignedHz(value) {
  const n = Number(value);
  if (!Number.isFinite(n)) return "--";
  const sign = n > 0 ? "+" : "";
  return `${sign}${fmtHz(n)}`;
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
  } catch (error) {
    setConfigStatus(`activate failed: ${String(error.message || error).slice(0, 120)}`);
  } finally {
    state.configBusy = false;
    renderConfigProfiles();
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
  for (const id of ["serviceRestartBtn", "serviceShutdownBtn", "serviceStopGoBtn"]) {
    const button = $(id);
    if (button) button.disabled = !!state.serviceBusy;
  }
  setText("serviceActionStatus", state.serviceStatus || "idle");
}

async function requestServiceAction(action) {
  const endpoints = {
    restart: "/api/service/restart",
    shutdown: "/api/service/shutdown",
    stopgo: "/api/service/stop-go",
  };
  const labels = {
    restart: "restart",
    shutdown: "shutdown",
    stopgo: "stop & go",
  };
  const endpoint = endpoints[action];
  if (!endpoint || state.serviceBusy) return;
  if (action !== "restart" && !window.confirm(`${labels[action]} BS?`)) return;
  state.serviceBusy = true;
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
  } catch (error) {
    setServiceStatus(`${labels[action]} failed: ${String(error.message || error).slice(0, 90)}`);
  } finally {
    window.setTimeout(() => {
      state.serviceBusy = false;
      renderServiceControls();
    }, 2500);
  }
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
  if (!issi) return '<span class="speaker-now muted"><span class="speaker-issi">--</span><span class="speaker-detail">no active speaker</span></span>';
  const entry = radioId.cache.get(normalizeIssi(issi));
  const detail = entry && validRadioIdCacheEntry(entry) && radioIdDisplay(entry) ? radioIdDisplay(entry) : "RadioID pending";
  return `<span class="speaker-now"><span class="speaker-issi">ISSI ${esc(issi)}</span><span class="speaker-detail">${esc(detail)}</span></span>`;
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
  return `<span class="call-ts">${call?.ts ? `TS${esc(call.ts)}` : "TS--"}</span>`;
}

function callCardHtml(call) {
  const mode = callMode(call);
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
      <div class="call-card-grid">
        <div class="call-card-speaker">
          <span>${esc(speakerLabel)}</span>
          <strong>${instantSpeakerHtml(call)}</strong>
        </div>
        <div>
          <span>Caller</span>
          <strong>${call?.caller_issi ? radioIdentityHtml(call.caller_issi) : "--"}</strong>
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
  setText("rfState", rfOnline ? "READY" : "WAITING");
  setText("diagramRfState", rfOnline ? "READY" : "WAITING");
  setText("diagramAntennaState", rfOnline ? "radiating carrier" : "RF path muted");
  setText("diagramPathToggleState", rfOnline ? "ON AIR" : "STANDBY");
  setIndustrialTone("mapRfMeter", rfOnline ? "ok" : "warn");
  setIndustrialTone("nodeRfDevice", rfOnline ? "ok" : "warn");
  setIndustrialTone("diagramPathToggle", rfOnline ? "ok" : "warn");
  setClass("diagramPathToggle", "is-on", rfOnline);
  setClass("rfState", "ok", rfOnline);
  setClass("rfState", "warn", !rfOnline);
  setStatusTone("rfOpsStatusStrip", rfOnline ? "ok" : "warn");

  const lastOkAge = state.lastHttpOkMs ? Math.floor((Date.now() - state.lastHttpOkMs) / 1000) : null;
  setText("systemTelemetryState", lastOkAge === null ? state.wsState.toUpperCase() : `${lastOkAge}s`);
  setText("railTelemetryState", lastOkAge === null ? state.wsState.toUpperCase() : `${lastOkAge}s`);
  setClass("systemTelemetryState", "ok", lastOkAge !== null && lastOkAge <= 5);
  setClass("systemTelemetryState", "warn", lastOkAge === null || lastOkAge > 5);
  setClass("railTelemetryState", "ok", lastOkAge !== null && lastOkAge <= 5);
  setClass("railTelemetryState", "warn", lastOkAge === null || lastOkAge > 5);
  setStatusTone("telemetryStatusStrip", lastOkAge !== null && lastOkAge <= 5 ? "ok" : "warn");
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
      <td>${groupLabel(radio.groups)}</td>
      <td>${rssiLabel(radio.rssi_dbfs)}</td>
      <td><span class="pill ${radio.energy_saving_mode ? "amber" : "green"}">${esc(eeLabel(radio))}</span></td>
      <td>${esc(radioLastSeen(radio))}</td>
    </tr>
  `);
  const html = rowsOrEmpty(rows, 5, "No registered radios");
  setHtml("radiosTable", html);
}

function renderCalls() {
  const currentCalls = activeCalls();
  const trackedCalls = sortedCalls();
  const overview = currentCalls.map((call) => callCardHtml(call));
  setHtml("overviewCalls", overview.length ? overview.join("") : '<div class="empty-call-board">No active calls</div>');

  const rows = trackedCalls.map((call) => {
    const mode = callMode(call);
    return `
      <tr>
        <td>${callCountryHtml(call)}</td>
        <td>${esc(call.call_id)}</td>
        <td><span class="pill ${mode.className}">${esc(mode.label)}</span></td>
        <td>${callTargetHtml(call)}</td>
        <td>${call.caller_issi ? radioIdentityHtml(call.caller_issi) : "--"}</td>
        <td>${call.call_type === "group" ? "--" : call.called_issi ? radioIdentityHtml(call.called_issi) : "--"}</td>
        <td>${callSpeakerHtml(call)}</td>
        <td data-call-seconds="${esc(call.call_id)}">${callAgeSeconds(call)}s</td>
        <td>${esc(call.ts || "--")}</td>
      </tr>
    `;
  });
  setHtml("callsTable", rowsOrEmpty(rows, 9, "No active calls"));
  renderSlots();
}

function heardRows(limit) {
  return state.heard.slice(0, limit).map((entry) => `
    <tr>
      <td>${esc(entry.ts || "--")}</td>
      <td>${radioIdentityHtml(entry.issi)}</td>
      <td>${esc(entry.activity || "--")}</td>
      <td>${destinationHtml(entry)}</td>
    </tr>
  `);
}

function renderHeard() {
  setText("overviewHeardLabel", `${Math.min(state.heard.length, 8)} shown, ${radioId.cache.size} cached`);
  setHtml("overviewHeard", rowsOrEmpty(heardRows(8), 4, "No recent activity"));
  const rows = heardRows(80);
  setHtml("heardTable", rowsOrEmpty(rows, 4, "No recent activity"));
}

function renderLogs() {
  setText("logCount", `${state.logs.length} lines`);
  const list = $("logList");
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
  if (scrollButton) scrollButton.textContent = state.logAutoScroll ? "Pause" : "Play";
  for (const id of ["logClearBtn", "logExportBtn", "logAutoScrollBtn"]) {
    const button = $(id);
    if (button) button.disabled = !!state.logBusy;
  }
  if (state.activePage === "logs" && state.logAutoScroll && list) {
    list.scrollTop = list.scrollHeight;
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
  return activeCalls().find((call) => Number(call.ts) === Number(ts));
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

  const txHz = soapy.tx_hz ?? cell.derived_dl_hz;
  const rxHz = soapy.rx_hz ?? cell.derived_ul_hz;
  const shift = Number.isFinite(Number(txHz)) && Number.isFinite(Number(rxHz)) ? Number(txHz) - Number(rxHz) : null;

  setText("rfTxFreq", fmtHz(txHz));
  setText("rfRxFreq", fmtHz(rxHz));
  setText("rfDuplexShift", shift === null ? "--" : fmtSignedHz(shift));
  setText("rfBandCarrier", cell.freq_band !== undefined ? `band ${cell.freq_band}, carrier ${cell.main_carrier}` : "--");
  setText("rfMccMnc", cfg.network ? `${cfg.network.mcc} / ${cfg.network.mnc}` : "--");
  setText("rfLocationArea", cell.location_area ?? "--");
  setText("rfColourCode", cell.colour_code ?? "--");
  setText("rfFrame18", compactBool(cell.frame_18_ext));
  setText("rfFreqMatch", soapy.frequency_match ? "matched" : cfg.available ? "check" : "--");
  setText("diagramFrequency", txHz ? fmtHz(txHz) : "--");
  setText("diagramNetwork", cfg.network ? `${cfg.network.mcc}-${cfg.network.mnc} / CC ${cell.colour_code ?? "--"}` : "--");
  setText("diagramProgram", cell.main_carrier !== undefined ? `carrier ${cell.main_carrier}` : "local carrier");

  setText("rfDevice", soapy.device || state.system?.sdr_name || "auto");
  setText("diagramRfDevice", soapy.device || state.system?.sdr_name || "auto");
  setText("rfSampleRate", soapy.sample_rate ? fmtHz(soapy.sample_rate, "kHz") : txVisual.sample_rate ? fmtHz(txVisual.sample_rate, "kHz") : "--");
  setText("rfPpm", soapy.ppm_err !== undefined ? `${Number(soapy.ppm_err).toFixed(2)} ppm` : "--");
  setText("rfChannels", `RX ${soapy.rx_ch ?? "auto"} / TX ${soapy.tx_ch ?? "auto"}`);
  setText("rfAntennas", `RX ${soapy.rx_ant || "auto"} / TX ${soapy.tx_ant || "auto"}`);
  setText("rfTxGains", gainsLabel(sdrHealth.tx_gains || soapy.tx_gains));
  setText("rfRxGains", gainsLabel(sdrHealth.rx_gains || soapy.rx_gains));
  setText("rfTemp", sdrHealth.temperature_c !== null && sdrHealth.temperature_c !== undefined ? `${Number(sdrHealth.temperature_c).toFixed(1)} C` : state.system?.cpu_temp_c ? `${Number(state.system.cpu_temp_c).toFixed(1)} C host` : "--");

  setText("rfRms", fmtDb(txVisual.rms_dbfs, "dBFS"));
  setText("rfPeak", fmtDb(txVisual.peak_dbfs, "dBFS"));
  setText("rfEvm", fmtPct(txQuality.evm_pct));
  setText("rfPapr", fmtDb(txQuality.papr_db));
  setText("rfObw", fmtHz(txQuality.occupied_bandwidth_hz, "kHz"));
  setText("rfCarrierLeak", fmtDb(txQuality.carrier_leakage_db));
  setText("rfPower", sysHealth.total_power_w !== null && sysHealth.total_power_w !== undefined ? `${Number(sysHealth.total_power_w).toFixed(1)} W` : "--");
  setText("rfSnapshotAge", cfg.available ? "live config" : Object.keys(cached).length ? "cached" : "--");

  setText("settingsCarrier", cell.main_carrier !== undefined ? `${fmtHz(txHz)} / carrier ${cell.main_carrier}` : "--");
  setText("settingsNetworkCode", cfg.network ? `${cfg.network.mcc} / ${cfg.network.mnc}` : "--");
  setText("settingsColorCode", cell.colour_code ?? "--");
  setText("settingsTimeslots", "TS1 control, TS2-TS4 traffic");
  setText("settingsEnergyEconomy", cell.energy_saving_label || "auto");
  setText("settingsRadioIdEndpoint", radioIdEndpoint() || "disabled");
  setText("settingsConfigMode", cfg.available ? "live core config" : "unavailable");
  setText("adminAccessMode", "external dashboard proxy");
  setText("adminUpdateChannel", "disabled");
  setText("adminAuditSink", "volatile runtime log");
  setText("adminCoreProcess", "nexus-bs core service");
  setText("adminDashboardProcess", "nexus-bs-dashboard service");
  renderServiceControls();
  setText("slotSummary", "TS1 control, TS2-TS4 traffic");
  setText("diagramSlotState", "TS1 control, TS2-TS4 traffic");
}

function renderSystem() {
  const sys = state.system || {};
  setText("buildLabel", sys.product_version_tag || "--");
  setText("subtitle", sys.stack_version || "Nexus-BS local TETRA eBTS");
  setText("hostName", sys.hostname || "--");
  setText("productName", sys.product_name && sys.product_version_tag ? `${sys.product_name} ${sys.product_version_tag}` : "--");
  setText("cpuName", sys.cpu_model ? `${sys.cpu_model}${sys.cpu_cores ? ` (${sys.cpu_cores} cores)` : ""}` : "--");
  setText("memoryUse", sys.ram_total_mb ? `${sys.ram_used_mb || 0} / ${sys.ram_total_mb} MB` : "--");
  setText("cpuTemp", sys.cpu_temp_c !== null && sys.cpu_temp_c !== undefined ? `${Number(sys.cpu_temp_c).toFixed(1)} C` : "--");
  renderSystemUptime();
  setText("configPath", sys.config_path || "--");
  setText("runtimeConfigPath", sys.runtime_config_path || sys.config_path || "--");
  setText("sdrName", sys.sdr_name || "--");
  setText("soapyInfo", sys.soapy_info || "--");
  setText("aboutVersion", sys.product_name && sys.product_version_tag ? `Nexus-BS Project ${sys.product_version_tag} by Chris YO3TCO` : "Nexus-BS Project by Chris YO3TCO");
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
  preserveActivePageScroll(() => {
    renderStatus();
    renderCallSeconds();
    renderSystemUptime();
    renderSlots();
  });
}

function renderAll() {
  preserveActivePageScroll(() => {
    renderStatus();
    renderMetrics();
    renderRadios();
    renderCalls();
    renderHeard();
    if (state.activePage === "logs") renderLogs();
    renderSystem();
    renderConfigProfiles();
  });
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

function pushHeard(entry) {
  if (!entry) return;
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

async function loadSite() {
  if (state.siteInflight) return;
  state.siteInflight = true;
  try {
    const res = await fetch("/api/site", {
      credentials: "same-origin",
      cache: "no-store",
    });
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
    preserveActivePageScroll(() => {
      renderStatus();
      renderMetrics();
      renderCalls();
      renderHeard();
    });
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
  loadConfigProfiles();
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
  rememberPageScroll();
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
  restorePageScroll(page, 0);
}

function initNav() {
  window.addEventListener("scroll", () => rememberPageScroll(), { passive: true });
  for (const node of document.querySelectorAll(".nav-item")) {
    node.addEventListener("click", () => switchPage(node.dataset.page));
  }
  $("refreshBtn").addEventListener("click", refreshDashboardData);
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
  $("logAutoScrollBtn")?.addEventListener("click", () => {
    state.logAutoScroll = !state.logAutoScroll;
    renderLogs();
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
setInterval(loadSystem, 15000);
setInterval(loadSite, SITE_REFRESH_MS);
setInterval(loadSnapshot, SNAPSHOT_REFRESH_MS);
setInterval(loadCallsSnapshot, CALLS_REFRESH_MS);
setInterval(renderLiveTick, 1000);
