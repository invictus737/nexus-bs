# Nexus-BS Project Timeline

## 2026-06-10 12:36 EEST - Stop false Brew reconnect re-registration churn

Scope:

- Brew/MM availability patch and config-default hardening.
- Field symptom: operator observed repeated TETRA station reconnects and Brew
  reconnect indications during normal service.
- Runtime evidence:
  - systemd services did not restart (`NRestarts=0`);
  - live logs showed `BrewReconnected` being emitted around normal Brew
    `GROUP_TX` traffic;
  - live logs also showed BS-initiated periodic registration refreshes every
    `3600s` for local ISSIs.

Root cause:

- `BrewWorker` emitted `VersionDetected { v1 }` for every v1 `GROUP_TX` that
  carried a mnemonic.
- `BrewEntity` treated every `VersionDetected` as a real Brew reconnect and
  forwarded `BrewReconnected` to MM.
- MM's `BrewReconnected` handler sends `D-LOCATION-UPDATE-COMMAND` to locally
  registered MS units, so normal Brew traffic could force unnecessary
  re-registration.
- Separately, `periodic_registration_secs = 3600` enabled an hourly local MM
  watchdog that intentionally sends `D-LOCATION-UPDATE-COMMAND`; default is now
  `0` so that watchdog is opt-in for 24x7 stability.

ETSI clause discipline:

- The patch does not change normal attach/group-affiliation acceptance,
  group-call floor control, LLC, UMAC scheduling, SDS, WAP, RF or parrot
  behavior.
- It narrows when existing SwMI-initiated registration refresh is requested:
  Brew protocol-version discovery is telemetry, not an EN 300 392-2
  registration trigger.
- The retained real-reconnect MM refresh remains the existing
  `D-LOCATION-UPDATE-COMMAND` path scoped to EN 300 392-2 registration/location
  update behavior; no formal conformance claim is made.

Verification:

- `cargo fmt -p tetra-config -p tetra-entities --check` passed.
- `cargo test -p tetra-config --lib bluestation::parsing --locked` passed.
- `cargo test -p tetra-entities --lib net_brew --locked` passed.
- `cargo test -p tetra-entities --test test_mm_bs brew_reconnect --locked`
  passed.
- `cargo check -p tetra-config -p tetra-entities --locked` passed.
- `git diff --check` passed.

## 2026-06-10 00:55 EEST - Brew GSSI zero-media watchdog for 226333 stalls

Scope:

- Protocol-adjacent Brew/CMCE interconnect robustness patch. No private call,
  SDS, WAP, parrot, MM attach, LLC or UMAC slot-allocation behavior changed.
- Field issue: GSSI 226333 transmission appeared dead after Brew delivered
  `GROUP_TX` and CMCE/UMAC granted RF floor on TS2, but no Brew voice frame
  arrived. The active network-origin group call then held the RF floor until
  normal call timeout.
- Added a Brew-side first-media watchdog:
  - starts only after CMCE reports `NetworkCallReady`;
  - counts only valid post-ready STE/TCH-S voice frames;
  - sends existing `NetworkCallEnd` to CMCE after 3 seconds with zero valid
    media, letting CMCE emit the normal network-speaker floor release path;
  - resets the zero-media epoch on network speaker changes and hangtime circuit
    reuse so previous speaker audio cannot mask a new silent speaker.

ETSI clause discipline:

- EN 300 392-2 group call setup/floor/release behavior remains in CMCE:
  group setup/floor grant per clause 14.5.2.1 / 14.5.2.2.1, remote speaker
  cease via `D-TX CEASED`, and actual group teardown via `D-RELEASE`.
- This watchdog is a pragmatic Brew/IP interconnect robustness guard, not an
  ETSI air-interface timer and not a formal certification claim.

Verification:

- `cargo test -p tetra-entities net_brew::entity::tests::network_group_zero_media_guard_releases_rf_floor --locked` passed.
- `cargo test -p tetra-entities net_brew::entity::tests::network_group_zero_media_guard_ignores_call_after_valid_voice_frame --locked` passed.
- `cargo test -p tetra-entities net_brew::entity::tests::network_group_hangtime_reuse_resets_zero_media_guard --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_network_group_speaker_change_updates_dashboard_after_rf_grant --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_network_group_call_end_from_active_network_speaker_enters_hangtime_without_release --locked` passed.
- `git diff --check` passed.

Deploy evidence:

- Deployed with `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
- Remote target: `chris@192.168.1.179`, flat runtime directory
  `/home/chris/nexus-bs`.
- Core SHA256:
  `1292f74bf3085a69ec1b41c15554065d474a4ae2504684f2ea261218400bd48f`.
- Control-service SHA256:
  `90173861c34833d5feecf1d5a3ec2fb93fe9acfafe01f17fec81a374946b24f1`.
- Dashboard SHA256:
  `6d60d6c3c56685c872a73d0d9149b7789163c0e975d6d62093338c14fbad1d3a`.
- `nexus-bs@chris.service`, `nexus-bs-control@chris.service` and
  `nexus-bs-dashboard@chris.service` active/running since
  `2026-06-10 00:57:15 EEST`.
- Post-deploy `/api/snapshot` returned overall health `ok`, Brew v1 online,
  RF timing healthy and zero pending Brew critical commands.
- Post-deploy log showed a Brew-origin 226333 call from ISSI 2261313 reached
  `NetworkCallReady` and received `voice frame #1` 15 ms later; zero-media
  watchdog did not trigger for that valid media case.

## 2026-06-09 09:38 EEST - Dashboard Overview Last Heard with cached RadioID labels

Scope:

- Dashboard-only patch. No TETRA PDU, CMCE, UMAC, MM, LLC, SDS, WAP or RF
  behavior changed.
- Integrated the `Last Heard` feed into the Overview page.
- Added browser-side RadioID resolution for ISSI display as `callsign - name`
  when available, with ISSI retained as the secondary identifier.
- Lookup is intentionally off-core:
  - runs in the browser, not in the BS TETRA stack loop;
  - uses localStorage cache with positive and negative TTLs;
  - uses one lookup in flight, bounded queue length, request spacing and retry
    backoff;
  - keeps the endpoint configurable through the dashboard document metadata.
- This preserves the current dashboard-decoupling direction: UI enrichment must
  not add blocking CPU/network work to core RF/CMCE/UMAC operation.

Verification:

- `node --check dashboard/assets/app.js` passed.
- `cargo test -p tetra-entities external_dashboard_asset_manifest --locked`
  passed.
- No protocol compliance claim is made by this dashboard-only change.

Deploy evidence:

- Deployed with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Remote target: `chris@192.168.1.179`, flat runtime directory
  `/home/chris/nexus-bs`.
- Deployed commit marker: `4ab64b2e`.
- Core SHA256:
  `c0b84b0687d7603365c97d8397f23edc32be7b909aaf45926cf932849c231893`.
- Control-service SHA256:
  `e27f15d836d4927b678c3068965fdfee90a02f28abb4092bbd5398187c4a2e02`.
- `nexus-bs@chris.service` active/running since `2026-06-09 10:08:01 EEST`.
- `nexus-bs-control@chris.service` active/running since
  `2026-06-09 10:08:00 EEST`.
- Remote asset verification passed:
  `/home/chris/nexus-bs/dashboard/index.html` contains `overviewHeard`;
  `/home/chris/nexus-bs/dashboard/assets/app.js` contains the bounded RadioID
  queue/cache code.
- Remote `/api/system` returned `product_user_agent = Nexus-BS/v0.1.61`,
  CPU `Broadcom Cortex-A53 1GHz 64-bit`, runtime config
  `/run/nexus-bs-chris/config.toml`, persistent config
  `/home/chris/nexus-bs/config.toml`.

Follow-up fix:

- Field issue: browser showed `RadioID pending`.
- Root cause confirmed by direct HTTP check: RadioID returns valid JSON, but the
  public response does not provide browser-readable CORS access for the
  dashboard page.
- Added a same-origin dashboard endpoint `GET /api/radioid?id=ISSI`.
- `crates/tetra-entities/src/net_dashboard/radioid.rs`
  - Normalizes RadioID payload to `{ok, issi, callsign, name, missing}`.
  - Uses positive and negative TTL cache.
  - Allows only one outbound RadioID fetch at a time.
  - Applies global request spacing and per-ISSI failure backoff.
  - Runs under dashboard HTTP workers, not in RF/CMCE/UMAC stack execution.
- `dashboard/index.html` now points `nexus-bs-radioid-endpoint` to
  `/api/radioid`.
- Browser cache/queue remains in place, now talking to the same-origin endpoint.

Follow-up verification:

- `node --check dashboard/assets/app.js` passed.
- `cargo fmt -p tetra-entities` completed.
- `cargo test -p tetra-entities net_dashboard::radioid --locked` passed.
- `cargo test -p tetra-entities external_dashboard_asset_manifest --locked`
  passed.
- `git diff --check` passed.

Follow-up deploy evidence:

- Redeployed with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Core SHA256:
  `f3d18a9afcdbbad39a96815ec84b6df8bb69202fb1b29afdcb2b225922a07a25`.
- Control-service SHA256:
  `d54b4fbdc34e87a498c199b55c056b0e5945e7c6da3800be068b879eb538ed79`.
- `nexus-bs@chris.service` active/running since `2026-06-09 10:16:21 EEST`.
- `nexus-bs-control@chris.service` active/running since
  `2026-06-09 10:16:21 EEST`.
- Remote `GET /api/radioid?id=2260618` returned
  `{"callsign":"YO3TCO","issi":2260618,"missing":false,"name":"Cristian","ok":true}`.
- Remote `GET /api/radioid?id=2260616` returned
  `{"callsign":"YO3TCO","issi":2260616,"missing":false,"name":"Cristian","ok":true}`.
- Remote asset verification confirmed
  `<meta name="nexus-bs-radioid-endpoint" content="/api/radioid">`.

Second follow-up fix:

- Field issue: the `Calls` tab showed TG91 / WW group calls as `duplex`, and
  call age could appear stale instead of updating dynamically.
- Root cause:
  - UI rendered mode as `simplex ? simplex : duplex`, but group calls carry
    `simplex=false` in the dashboard state because they are neither private
    simplex nor private duplex.
  - UI used the snapshot `started_secs_ago` value directly when it was nonzero,
    so age froze after snapshot instead of advancing from `_startedMs`.
- `dashboard/assets/app.js`
  - Added `callMode()` so `call_type="group"` renders as `group`, not `duplex`.
  - Added `callAgeSeconds()` so the `Calls` tab always shows a live seconds
    counter.
  - Added `callTargetHtml()` so group targets render as `TG N`, while private
    targets still use ISSI/RadioID resolution.
  - If a dashboard client receives `speaker_changed` without a prior
    `call_started` event, it now recreates a minimal group-call row from the
    event instead of silently ignoring it.
- `dashboard/index.html`
  - Renamed the Calls tab duration column from `Age` to `Seconds`.
- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Added `gssi` to `speaker_changed` websocket frames for cleaner client-side
    recovery.

Second follow-up verification:

- `node --check dashboard/assets/app.js` passed.
- `cargo fmt -p tetra-entities` completed.
- `cargo test -p tetra-entities external_dashboard_asset_manifest --locked`
  passed.
- `git diff --check` passed.

Second follow-up deploy evidence:

- Redeployed with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Core SHA256:
  `12e5db4c5a176d60a6c954306b41ac2be1587d55155b2b9f61217d65f1c7bab1`.
- Control-service SHA256:
  `cb2c4dce278c31dd64f128f6a69107c100acfd6dab2de3c08b1e3c2bee853619`.
- `nexus-bs@chris.service` active/running since `2026-06-09 10:32:36 EEST`.
- `nexus-bs-control@chris.service` active/running since
  `2026-06-09 10:32:36 EEST`.
- Remote asset verification passed:
  `/home/chris/nexus-bs/dashboard/assets/app.js` contains `function callMode`;
  `/home/chris/nexus-bs/dashboard/index.html` contains `<th>Seconds</th>`.

Third follow-up fix:

- Field issue: dashboard dynamic updates still missed the current speaker,
  missed speaker changes around repeated TG91/WW PTTs, and could leave callsign
  unresolved if RadioID was not ready on first attempt.
- Root cause:
  - `call_started` group WS frames did not carry `active_speaker`, so the first
    speaker depended on client inference.
  - Brew/TG91 calls often emit `GROUP_IDLE` while CMCE/UMAC still keep hangtime;
    the dashboard deleted the call row immediately, so the next
    `speaker_changed` could arrive without a stable row.
  - RadioID retry was passive and tied mostly to render-time lookup.
- `crates/tetra-entities/src/net_dashboard/server.rs`
  - `GroupCallStarted` WS frames now include `active_speaker=caller_issi`.
  - `GroupCallEnded` WS frames now include `call_type="group"` and `gssi`.
- `dashboard/assets/app.js`
  - Keeps group call rows visible for a bounded hangtime UI window after
    `call_ended`, so reused TG91 calls can update the same row on
    `speaker_changed`.
  - Cancels the hangtime cleanup when a speaker-change/reuse arrives.
  - Adds `upsertCall()` so snapshots, starts and speaker changes merge state
    instead of replacing/losing context.
  - Adds priority RadioID refresh after call start and speaker change, with
    retries at start, 3 s and 8 s. Browser retry remains bounded; server-side
    `/api/radioid` still rate-limits and caches.
  - Reduces browser failure retry delay to 30 s; server backoff remains in
    place to protect RadioID and core operation.
- No TETRA air-interface behavior changed.

Third follow-up verification:

- `node --check dashboard/assets/app.js` passed.
- `cargo fmt -p tetra-entities` completed.
- `cargo test -p tetra-entities external_dashboard_asset_manifest --locked`
  passed.
- `git diff --check` passed.

Third follow-up deploy evidence:

- Redeployed with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Core SHA256:
  `5e9955c50064d8b8c8890ede0329d6226478882ea30752d9c11be3aef2009070`.
- Control-service SHA256:
  `275268118907945fa72d59e64eed8c8000628665e52f8427060b55cb0e5b56e2`.
- `nexus-bs@chris.service` active/running since `2026-06-09 10:43:54 EEST`.
- `nexus-bs-control@chris.service` active/running since
  `2026-06-09 10:43:53 EEST`.
- Remote asset verification passed:
  `/home/chris/nexus-bs/dashboard/assets/app.js` contains
  `GROUP_CALL_HANGTIME_UI_MS`, `refreshCallIdentities`, and `active_speaker`.

## 2026-06-09 09:23 EEST - Exported FlowStation delta history report

Scope:

- Exported the Nexus-BS vs FlowStation function-delta report to
  `NEXUS_BS_FLOWSTATION_DELTA_REPORT.md`.
- The report records the comparison date and exact compared releases:
  Nexus-BS `v0.1.61-1-g4ab64b2` / `4ab64b2`, FlowStation `v0.2.7` /
  `c2f0ee6`, and secondary FlowStation reference `v0.3.0` / `fcac34e`.
- No TETRA protocol behavior changed.
- This is engineering history only, not a formal TETRA/ETSI certification
  statement.

## 2026-06-09 08:05 EEST - P0 runtime config persistence and first dashboard decoupling step

Scope:

- Infrastructure/dashboard patch only. No TETRA PDU, CMCE, UMAC, MM, LLC,
  SDS or WAP protocol behavior changed.
- ETSI law memory was reloaded before work. Since this patch does not alter
  air-interface behavior, no new clause-scoped protocol claim is made.

Patch:

- `bins/nexus-bs/src/main.rs`
  - Dashboard config APIs now use `NEXUS_BS_PERSISTENT_CONFIG` when present.
  - The core may still run the volatile `/run/nexus-bs-USER/config.toml` copy,
    preserving volatile subscriber recovery/cache behavior.
  - Dashboard system API records the runtime config path separately for
    diagnostics.
- `contrib/systemd/nexus-bs@.service`
  - Sets `NEXUS_BS_PERSISTENT_CONFIG=/home/%i/nexus-bs/config.toml`.
  - Sets `NEXUS_BS_DASHBOARD_STATIC_DIR=/home/%i/nexus-bs/dashboard` for the
    optional external dashboard asset module.
- `crates/tetra-config/src/bluestation/sec_dashboard.rs`
  - Added `[dashboard].static_dir` as an explicit external asset path, separate
    from OTA `source_dir`.
  - Parser validates explicit `static_dir` as an existing directory.
- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Added `DashboardServer::set_static_dir`.
  - Keeps `/api/*`, `/ws`, `/login` and session handling in the Rust gateway.
  - Serves `/`, `/assets/*` and SPA routes from `static_dir` when configured.
  - Falls back to the embedded dashboard when no external assets are configured
    or the asset directory is not usable.
  - Rejects path traversal, including percent-encoded `..`.
- `README.md` and `example_config/config.toml`
  - Documented `static_dir` and persistent-dashboard-config behavior.

Verification:

- `cargo fmt -p tetra-config -p tetra-entities -p nexus-bs` passed.
- `cargo test -p tetra-config dashboard_static_dir --locked` passed.
- `cargo test -p tetra-config test_example_config_keeps_transmission_interruption_disabled --locked` passed.
- `cargo test -p tetra-entities dashboard_static_dir --locked` passed.
- `cargo check -p nexus-bs --locked` passed.
- `git diff --check` passed.

Next non-repeating execution gates:

1. Deploy only after the next runtime build gate, then verify dashboard config
   edit survives service restart because it writes `/home/USER/nexus-bs/config.toml`.
2. Continue P0 runtime hardening with dashboard HTTP body/concurrency caps and
   bounded telemetry/log queues.
3. Keep protected private simplex/P2P and v0.1.60 legacy GSSI protocol behavior
   untouched unless fresh RF logs specifically implicate those paths.
4. RF validation still required for GSSI `226333`, P2P close behavior, restart
   attach/group recovery, mixed EE auto and WAP/SDS.

### Runtime P0 continuation - dashboard memory/thread bounds

Additional patch in the same workstream:

- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Added `DASHBOARD_HTTP_CONNECTION_MAX = 64` active dashboard connection
    limit.
  - Added RAII connection guard so the active count is decremented when handler
    threads exit.
  - Added bounded HTTP body reader.
  - Oversized dashboard POST bodies now return `413` instead of allocating from
    untrusted `Content-Length` or silently truncating.
  - Applied body caps to login, config/profile edit, whitelist, WX, live-SDS and
    WiFi mutation endpoints.

Additional verification:

- `cargo test -p tetra-entities dashboard_http_body_reader --locked` passed.
- `cargo test -p tetra-entities dashboard_connection_guard --locked` passed.
- `cargo check -p nexus-bs --locked` passed after the runtime bounds patch.

### Runtime P0 continuation - bounded queues

Additional patch in the same workstream:

- Removed remaining unbounded crossbeam channels from `crates/` and `bins/`.
- Bounded telemetry events at 8192 entries and changed telemetry sink sends to
  non-blocking `try_send`.
- Bounded control command/response links at 1024 entries and changed control
  dispatch/response sends to non-blocking `try_send`.
- Bounded dashboard log forwarding at 2048 entries; the tracing dashboard layer
  already used `try_send`.
- Bounded Brew worker/event queues and changed Brew command/event sends to
  non-blocking `try_send`.
- Bounded generic network entity worker queues and removed the panic-on-send
  path from `NetEntity::rx_prim`.
- Bounded PHY async file writer queues.
- Bounded the `nexus-bs-control` per-client command queue.

Additional verification:

- `cargo test -p tetra-entities telemetry_channel_is_bounded --locked` passed.
- `cargo test -p tetra-entities control_link_is_bounded --locked` passed.
- `cargo test -p tetra-entities brew_command_channel_is_bounded --locked` passed.
- `cargo test -p tetra-entities phy_io_file --locked` passed.
- `cargo test -p tetra-entities wx --locked` passed after WX reply enqueue
  switched to non-blocking `try_send`.
- `cargo check -p nexus-bs --locked` passed.
- `cargo check -p nexus-bs-control --locked` passed.

### Runtime P0 continuation - systemd readiness/watchdog

Additional patch in the same workstream:

- `crates/tetra-entities/src/service_control.rs`
  - Added direct `sd_notify` support using `NOTIFY_SOCKET`, without adding a
    new dependency.
  - Added `READY=1`, `STOPPING=1` and watchdog notification helpers.
  - Added `WATCHDOG_USEC` interval parsing with a one-second minimum.
  - Watchdog notifications are tied to the stack tick counter.
- `crates/tetra-entities/src/messagerouter.rs`
  - Marks stack progress after each completed router tick.
- `bins/nexus-bs/src/main.rs`
  - Starts the systemd watchdog helper and emits readiness/stopping status.
- `contrib/systemd/nexus-bs@.service` and legacy sample
  - Switched to `Type=notify`.
  - Added `NotifyAccess=main`, `WatchdogSec=30s`,
    `RestartForceExitStatus=75`, `OOMPolicy=stop`, `MemoryMax=512M`,
    `TasksMax=128`, `LimitNOFILE=4096` and `NoNewPrivileges=yes`.

Additional verification:

- `cargo test -p tetra-entities service_control --locked` passed.
- `cargo check -p nexus-bs --locked` passed after the readiness/watchdog patch.

## 2026-06-09 02:52 EEST - Architecture swarm started for robustness and dashboard goals

Swarm status:

- Closed previous telecom analysis agents before starting the new workstream.
- Started orchestration/PM agent `019eaaab-bf55-7c12-9b1e-b918f56ed61b` to produce milestone sequencing, owners, acceptance gates, RF gates and timeline update format for the two active goals.
- Started protocol architect agent `019eaaab-f110-7341-b742-95c6bb659d4e` for clause-scoped TETRA backlog across GSSI group call, private simplex/duplex, SDS/WAP, MM attach/affiliation/restart recovery, EE auto/EG and scan/group stability.
- Started runtime reliability architect agent `019eaaac-1d82-70d0-a99a-99f1ca82cf68` for 24x7 systemd/runtime/logging/resource/recovery hardening.
- Started dashboard architect agent `019eaaac-489f-7523-88b1-19ef2cae67ee` for extracting the operator dashboard from the Rust binary and replacing it with a Nexus-BS-owned operational dashboard.
- Started QA/soak architect agent `019eaaac-81ea-7d50-a4e5-a84607e06652` for automated/RF/soak/restart/affiliation/private/SDS/WAP/dashboard acceptance matrix.

Execution rules for the swarm:

- All protocol work remains governed by `/Users/ctermure/.codex/memories/tetra-etsi-compliance-law.md`: clause-scoped ETSI EN 300 392-2 analysis, focused tests, no formal certification claim without official conformance evidence.
- FlowStation remains historical/upstream context only. Nexus-BS product identity, dashboard, runtime packaging and operational roadmap are Nexus-BS-owned and must not imply collaboration or shared product governance.
- Dashboard redesign must be modern, dense and operational, inspired by the discipline of professional TETRA management consoles, not a marketing page or decorative demo UI.

Integration update:

- All five swarm agents completed and were closed after read-only analysis.
- Added `MISSION_READINESS.md` as the central operational backlog and QA gate document for mission-readiness evidence.
- Updated README and dashboard About wording so BlueStation/FlowStation/SXCEIVER remain historical credits only; Nexus-BS is described as independently maintained and not a FlowStation collaboration or shared product.
- Added a dashboard test assertion for the independent Nexus-BS wording to prevent accidental regression.

## 2026-06-09 02:18 EEST - Nexus-BS v0.1.60 release bump and mission-critical robustness goal

Release scope:

- Bumped current Nexus-BS identity from `v0.1.59` to `v0.1.60` across workspace metadata, lockfile package versions, README, example config, systemd samples, control/telemetry protocol tests and dashboard product-identity tests.
- Commit/deploy target message: `fixed compatibility GSSI calls for legacy tetra terminals`.
- This release is the current checkpoint for the legacy local GSSI group-call compatibility profile:
  - parser default remains `legacy_gssi_group_call = false`;
  - field/example profile explicitly enables `legacy_gssi_group_call = true`;
  - local GSSI no-handoff overs send normal `D-TX CEASED`/floor release and then release the maintained group call so old Motorola/MR-era terminals do not reuse a silent same-channel hangtime retake;
  - stale same-speaker tail retakes are skipped if a different speaker is queued, preserving normal group conversation turn-taking.

Mission-critical project goal:

- Treat 24x7/365 operation without downtime as the engineering target for Nexus-BS service hardening: robust local group call, private simplex/duplex, SDS, restart recovery, affiliation retention, group/scan stability, bounded queues, volatile/circular logging, systemd recovery and Pi appliance operation.
- This is an operational reliability objective, not a formal uptime guarantee and not a formal ETSI/TETRA certification claim.
- Every future TETRA protocol change still follows `/Users/ctermure/.codex/memories/tetra-etsi-compliance-law.md`: clause-scoped EN 300 392-2 analysis, focused tests, and no whole-stack certification claims without official conformance evidence.

Secondary dashboard goal:

- Move the operator dashboard out of the Rust binary as a separate Nexus-BS UI asset/application so it can be changed without recompiling the TETRA core.
- Build a modern, mission-focused Nexus-BS dashboard: dense operational panels for RF health, service state, subscribers, affiliations, calls, SDS/WAP, Brew, logs and alarms; no marketing/landing-page layout and no obvious "vibe-coded" decorative UI.
- The design reference is the operational discipline of professional TETRA management consoles such as Motorola DIMETRA-style and Rohill tetraNode-style dashboards, but the implementation, identity and wording must be Nexus-BS-owned and must not imply collaboration, sharing, or endorsement by FlowStation or any other upstream project.
- Keep only useful controls. Remove or gray unsupported functionality until it is implemented and verified.

## 2026-06-09 02:05 EEST - Legacy GSSI group-call compatibility mode for old Motorola retake

Field trigger:

- User narrowed the relevant RF test to old-terminal repeated PTT in local GSSI group call: first PTT works, second same-terminal PTT opens the assigned channel for about 15 s with no voice.
- Fresh journal after `f78cf4e` showed the D-INFO/T310 hypothesis was wrong for the remaining failure: no immediate group `D-INFO reset T310` was emitted, but the same-speaker hangtime retake still received positive `D-TX GRANTED` and then timed out with `accepted_ul_media_since_floor=0`.
- The failed path was a maintained-call/hangtime retake, not initial group setup. The initial `U-SETUP -> D-CONNECT/D-SETUP` path accepted UL media.

Clause-scoped reasoning:

- EN 300 392-2 clause 14.5.2.2.1(b): `D-TX GRANTED` remains the SwMI floor/transmit authorization during a maintained group call.
- EN 300 392-2 clause 14.5.2.2.1(e): `U-TX CEASED` / `D-TX CEASED` end the current group transmission.
- EN 300 392-2 clause 14.5.2.3: group call release uses `D-RELEASE`.
- EN 300 392-2 clause 23.8.1 covers BS traffic-channel trunking policy. The new behavior is an explicit local compatibility profile, not a general ETSI requirement: after a no-handoff local over, release the maintained GSSI call instead of keeping it for fast same-channel hangtime retake.
- This is not a formal conformance claim and is not the default parser behavior.

Patch:

- `crates/tetra-config/src/bluestation/sec_cell.rs`
  - Added `cell_info.legacy_gssi_group_call` with aliases `legacy_group_call` and `legacy_group_same_speaker_retake`; default is `false` when omitted.
- `example_config/config.toml`
  - Explicitly enables `legacy_gssi_group_call = true` for the Nexus-BS field profile with older terminals.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - In legacy mode, a local GSSI no-handoff over sends normal tail-drained `D-TX CEASED`/internal `FloorReleased`, then starts `D-RELEASE`.
  - If the same speaker queued a retake during the TX-ceased tail, the positive fast grant is suppressed and the old over is cleared instead.
  - Different-speaker queued handoff remains unchanged.
  - If a stale same-speaker retake is ahead of a different speaker in the tail queue, the stale retake is skipped and the different speaker receives the normal positive handoff grant; the call is not released prematurely.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - In legacy mode, a stale same-speaker `U-TX DEMAND` during no-active-speaker hangtime releases the maintained call instead of opening a silent positive-grant over.
- Private simplex/P2P, duplex, SDS, parrot, UMAC grant encoding, RA ACK and LMAC voice decode were not changed.

Verification:

- `cargo test -p tetra-config --lib legacy_gssi --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs legacy_gssi --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_legacy_gssi_group_skips_stale_same_speaker_retake_when_later_speaker_is_queued --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_226333_same_speaker_retake_during_tx_ceased_tail_defers_positive_grant --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_tx_ceased_tail_drain_then_grants_requester_queued_during_tail --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_example_config_simple_private_call_works_with_preemption_default_off --locked` passed.
- `cargo check -p tetra-config --locked` passed.
- `cargo check -p tetra-entities --tests --locked` passed.
- `cargo fmt --package tetra-config --package tetra-entities -- --check` passed.
- `git diff --check` passed.

Next RF gate:

1. Deploy to `chris@192.168.1.179` using `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
2. Ensure runtime `config.toml` contains `legacy_gssi_group_call = true`.
3. Clean volatile journal and restart service.
4. Test only old-terminal repeated PTT on GSSI `226333`: same ISSI, PTT 1, release, PTT 2.
5. Expected log shape after first `U-TX CEASED`: `D-TX CEASED`, `FloorReleased`, then `D-RELEASE`; the second PTT should appear as a fresh `U-SETUP` rather than positive same-call hangtime `D-TX GRANTED` followed by `accepted_ul_media_since_floor=0`.

## 2026-06-09 01:03 EEST - Suppress immediate group D-INFO reset T310 after floor grant

Field trigger:

- RF test on deployed `v0.1.59-95e9a819` failed on local GSSI `226333`.
- Full service journal from current restart showed call_id `4`: initial group setup from ISSI `2260616` accepted UL media, but the post-`D-TX CEASED` retake from the same ISSI received a positive requester `D-TX GRANTED` and then timed out with `accepted_ul_media_since_floor=0`.
- The failing retake emitted the immediate timer-only group `D-INFO reset T310` after the positive requester grant and listener grants. The successful initial setup did not use this timer D-INFO path.

Clause-scoped reasoning:

- EN 300 392-2 clause 14.5.2.2.1(b): positive `D-TX GRANTED` is the floor/transmit authorization.
- EN 300 392-2 clause 14.5.2.2.2(c): `D-INFO reset T310` is timer signalling, not transmit authorization.
- EN 300 392-2 clauses 14.7.1.8 and 14.8.37 define the D-INFO reset field, but do not require SwMI to inject it immediately after each floor grant.
- Therefore the BS can keep its local call timeout fresh without placing a timer-only group D-INFO into the first post-grant assigned-channel FACCH/STCH frames.
- This is compatibility hardening for Motorola MR5/MR19 retake behaviour; it is not a formal conformance claim.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Replaced `send_group_d_info_reset_t310_facch(...)` with `reset_group_t310_after_floor_grant(call_id)`.
  - The function now resets local SwMI T310 state only and logs that no timer-only D-INFO is emitted on FACCH.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - All group floor grant/handoff/regrant paths now call the local reset helper instead of queueing a group D-INFO PDU.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated group handoff/retake tests to assert that floor grants do not emit `D-INFO reset T310` on FACCH.
- Private simplex/P2P, duplex, SDS, parrot, LMAC voice decode, RA ACK, AACH, and `D-TX GRANTED` ordering were not changed.

Verification:

- `cargo check -p tetra-entities --tests --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs group --locked` passed before cleanup: 75 tests.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` passed: 12 tests.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_tx_ceased_hands_floor_to_queued_requester --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_hangtime_tx_demand_defers_late_entry_d_setup_refresh --locked` passed.
- `cargo test -p tetra-entities --test test_umac_bs group_d_info_reset_t310 --locked` passed.
- `cargo fmt --package tetra-entities -- --check` passed.
- `git diff --check` passed.

Next RF gate:

1. Deploy this build directly to testing with the normal local-build deploy script.
2. Clean remote journal, restart BS, and test GSSI `226333`.
3. Expected log difference: after `UMAC RF diag: STCH D-TX GRANTED ... group_requester_positive=true`, no `UMAC RF diag: STCH D-INFO reset T310` should appear.
4. Required field behaviour: repeated/rapid PTT from Motorola MR5/MR19 must enter valid UL media; if it still fails with `accepted_ul_media_since_floor=0`, continue at LMAC/PHY post-grant observation.

## 2026-06-09 00:24 EEST - FlowStation upstream GSSI comparison and post-grant RF diagnostics

Context:

- User requested a dedicated agent to compare local Nexus-BS GSSI group-call handling with upstream FlowStation and to re-check the ETSI law before more fixes.
- Reloaded `/Users/ctermure/.codex/memories/tetra-etsi-compliance-law.md` and `/Users/ctermure/.codex/memories/flowstation-tetra-eg-swmi-resume-2026-06-02.md`.
- Spawned explorer agent `019ea914-bfa1-7d71-98f1-52269ee7dc7f` (`Mill the 6th`) for read-only FlowStation comparison.
- Cloned upstream FlowStation read-only to `/private/tmp/flowstation-upstream-compare`, upstream commit `fcac34e`.
- Local base was clean at `c6e6966` (`Make group timer D-INFO markerless`).

Upstream comparison:

- Upstream FlowStation activates internal UMAC/Brew `FloorGranted` immediately after queueing positive `D-TX GRANTED`; Nexus-BS gates group floor activation on the positive requester grant `TxReporter`.
- Upstream requester STCH drops MAC channel allocation and marks any ISSI-addressed STCH as random-access response; Nexus-BS preserves real/request-ready RA ACK and carries the uplink-capable requester allocation.
- Upstream hangtime AACH remains `AssignedControl`/`AssignedOnly` while STCH retake is queued; Nexus-BS advertises `Traffic(...)` while a positive group requester grant is pending.
- Upstream has no group `D-INFO reset T310`; Nexus-BS adds timer signalling but `c6e6966` strips channel allocation and usage marker over the air.
- Upstream LMAC can classify NTS2 Block2 as STCH when the local marker says non-traffic; Nexus-BS treats non-stolen NTS2 Block2 as TCH/S and preserves raw Block2.
- Conclusion: upstream is not a safer behavior source for the current Motorola GSSI bug. The remaining fault is below CMCE grant ownership unless RF logs prove the immediate timer `D-INFO` confuses the terminal.

Clause scope:

- EN 300 392-2 clause 14.5.2.2.1(b): positive `D-TX GRANTED` is the transmit/floor authorization.
- EN 300 392-2 clause 14.5.2.2.1(e): `U-TX CEASED` / `D-TX CEASED` end the current transmission.
- EN 300 392-2 clause 14.5.2.2.2(c): `D-INFO reset T310` is timer signalling, not transmit authorization.
- EN 300 392-2 clause 21.4.3.1: MAC random-access acknowledgement flag must reflect real RA acknowledgement.
- EN 300 392-2 clauses 23.5, 23.8.4.1.4, and 23.8.5: assigned-channel timing, STCH/TCH half-slot interpretation, and TCH/S half-slot preservation.
- This is clause-scoped engineering evidence only, not formal TETRA certification.

Diagnostic-only patch:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added `UMAC RF diag: selected STCH D-TX GRANTED ...` when the scheduler actually selects a positive `D-TX GRANTED` STCH for transmission.
  - Fields include tx time, address, call id, grant value, RA ACK, usage marker, channel allocation, active UL primary address, STCH priority, and whether speech was present.
- `crates/tetra-entities/src/lmac/lmac_bs.rs`
  - Added a bounded 18-frame / 24-event post-grant UL diagnostic window when LMAC transmits an ISSI-addressed STCH with uplink-capable channel allocation.
  - Logs first post-grant UL candidates with pchan, burst type, train sequence, block number, `block2_stolen`, logical-channel decision, RSSI, and final result such as `forward_raw_block2`, `forward_acelp`, `control_crc_fail`, `drop_crc`, `drop_partial`, `drop_len`, or `decode_none`.
  - No behavior changes: only additional INFO logs during a short post-grant diagnostic window.

Verification:

- `cargo check -p tetra-entities --locked` passed.
- `cargo test -p tetra-entities --test test_umac_bs test_group_floor --locked` passed: 6 tests.
- `cargo test -p tetra-entities --test test_cmce_bs group_226333 --locked` passed: 4 tests.
- `cargo test -p tetra-entities --test test_cmce_bs simplex_p2p --locked` passed: 13 tests.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` passed: 12 tests.
- `cargo fmt --package tetra-entities -- --check` passed.
- `git diff --check` passed.

Next RF gate:

1. Commit and deploy this diagnostic-only build locally to the BS with the normal local-build deploy path.
2. Clean remote volatile journal, restart BS, then test GSSI `226333`: alternate PTT/floor between `2260618`, `2260616`, and Motorola `2260082`; include rapid/repeated PTT from `2260082`.
3. For a failing Motorola turn, inspect this exact chain:
   - `UMAC RF diag: STCH D-TX GRANTED ... group_requester_positive=true ... ra_ack=true ... chan_alloc=... Both`
   - `UMAC RF diag: selected STCH D-TX GRANTED ...`
   - `LMAC RF diag: armed post-grant UL window ...`
   - `LMAC RF diag: post-grant UL candidate ...`
   - `LMAC RF diag: post-grant UL result ...`
   - either `UMAC RF diag: first accepted UL media after floor ...` or `UL inactivity timeout ... accepted_ul_media_since_floor=0`
4. If no LMAC candidates appear after a coherent grant, suspect terminal not entering UL/RF timing/grant interpretation.
5. If LMAC candidates appear but result is `control_crc_fail`, `drop_crc`, `drop_len`, or `decode_none`, continue in PHY/LMAC demod/decode path.
6. If candidates forward to UMAC but `accepted_ul_media_since_floor` still stays zero, continue in UMAC TMD acceptance/routing.

## 2026-06-09 00:04 EEST - Timer-only group D-INFO now omits traffic usage marker

Field evidence:

- RF logs on build `v0.1.59-29bc9172` showed the issue is broader than `2260082` MTP3550 MR19.9.
- A newly powered older Motorola MTP850 MR5.14 using ISSI `2260616` reproduced the same group-call failure on GSSI `226333`.
- Pattern for both Motorola terminals:
  - initial group call setup can pass voice and logs `first accepted UL media`;
  - after `D-TX CEASED`, the terminal sends `U-TX DEMAND`;
  - SwMI sends individual `D-TX GRANTED(Granted)` with `ra_ack=true`, `chan_alloc=Both`, and AACH `Traffic(...)`;
  - immediately after that, group-addressed `D-INFO reset T310` is sent as timer signalling;
  - no valid TCH/S reaches UMAC, ending in `accepted_ul_media_since_floor=0`.

Component in simple technical terms:

- `D-TX GRANTED` tells one terminal it may speak.
- `D-INFO reset T310` only refreshes the group call timer; it is not another floor grant.
- The previous patch removed the DL-only channel allocation from this timer D-INFO, but the MAC resource still carried the traffic usage marker. Older Motorola terminals appear to treat that immediate GSSI timer STCH as a conflicting assigned-channel hint.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1 b) makes the individually addressed `D-TX GRANTED(Granted)` the transmit authorization and U-plane-on trigger.
- EN 300 392-2 clause 14.5.2.2.2 c) allows `D-INFO` to reset T310, but that message is timer signalling, not transmit authorization.
- EN 300 392-2 clause 23.5 requires assigned-channel signalling to stay coherent with the call-control state.
- EN 300 392-2 clause 21.4.3.1 keeps RA ACK semantics on the requester grant.
- This is clause-scoped engineering hardening, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - For group-addressed `D-INFO reset T310` on assigned-channel STCH, keeps the CMCE channel allocation only as local routing metadata.
  - Omits both over-air MAC channel allocation and over-air MAC `usage_marker` for that timer-only D-INFO.
  - Leaves requester `D-TX GRANTED(Granted)` unchanged: RA ACK, `Both` channel allocation, usage marker, and AACH traffic indication stay intact.
  - Leaves private/P2P, SDS, duplex, and parrot paths unchanged.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Extended the D-INFO regression test to require both `chan_alloc_element=None` and `usage_marker=None`.

Verification:

- `cargo fmt --package tetra-entities -- --check` passed.
- `cargo test -p tetra-entities --test test_umac_bs test_group_d_info_reset_t310_stch_omits_dl_only_channel_allocation_and_usage_marker --locked` passed.
- `cargo test -p tetra-entities --test test_umac_bs test_group_floor --locked` passed: 6 tests.
- `cargo test -p tetra-entities --test test_cmce_bs group_226333 --locked` passed: 4 tests.
- `cargo test -p tetra-entities --test test_cmce_bs simplex_p2p --locked` passed: 13 tests.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

## 2026-06-09 11:58 EEST - Dashboard auth hardening and split static protection

Scope:

- Dashboard/API/security-only patch. No TETRA air-interface protocol, CMCE,
  UMAC/MAC, LLC, MM, SDS, Brew media or RF audio behavior was changed.
- Added public core `GET /api/auth/status` for split dashboard auth checks. It
  reports only `auth_required`, `session_valid` and the cookie name; it never
  exposes configured credentials.
- Hardened the separate `nexus-bs-dashboard` front-end so static dashboard
  routes (`/`, SPA routes and `/assets/*`) consult the loopback core auth status
  before serving files when `[dashboard] username/password` are configured.
- Added HTTP security headers to split dashboard static/error responses and
  brought core dashboard JSON/HTML helper responses under the same no-store,
  no-frame, no-sniff and no-referrer policy.
- Tightened dashboard config validation: `bind` cannot be empty, credentials
  must be set as a pair, username cannot be blank, and password cannot be blank.
- Updated README/example config wording for optional dashboard auth in split
  deployment. This is automated hardening/verification, not formal security or
  ETSI/TETRA certification.

Verification:

- `cargo fmt -p tetra-config -p tetra-entities -p nexus-bs-dashboard --check`
  passed.
- `cargo test -p tetra-config dashboard --locked` passed: 7 dashboard config
  tests.
- `cargo test -p tetra-entities dashboard_auth_status --locked` passed: 3
  core auth-status tests.
- `cargo test -p tetra-entities dashboard_session --locked` passed: 2 session
  cookie/store tests.
- `cargo test -p tetra-entities dashboard_security_headers --locked` passed.
- `cargo test -p nexus-bs-dashboard --locked` passed: 11 split front-end auth,
  network auth-probe and path tests.
- `cargo check -p tetra-config -p tetra-entities -p nexus-bs-dashboard --locked`
  passed.
- `node --check dashboard/assets/app.js` passed.
- `git diff --check` passed.

Deploy verification:

- Built locally only and deployed to `chris@192.168.1.179` with
  `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
- Remote SHA-256 after deploy:
  - core `4496d51cade28597ba46f381e09fa81fc2bd88faa4f2013b52973741b11220e1`
  - control `2b86bcc924bd47dd6b106739b7d7a3bb0f9e3c51c77b29955f70bb9a59e1ff0c`
  - dashboard `6d60d6c3c56685c872a73d0d9149b7789163c0e975d6d62093338c14fbad1d3a`
- Remote services active/running after final deploy:
  - `nexus-bs@chris.service` PID `114223`
  - `nexus-bs-control@chris.service` PID `114208`
  - `nexus-bs-dashboard@chris.service` PID `114240`
- Public dashboard proxy `GET http://127.0.0.1:8080/api/auth/status` returned
  `{"auth_required":false,"session_cookie":"fs_session","session_valid":true}`
  for the current testing config, where dashboard auth is not enabled.
- Public dashboard `/` returned security headers:
  `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`,
  `Referrer-Policy: no-referrer`,
  `Content-Security-Policy: frame-ancestors 'none'; object-src 'none'; base-uri 'none'`,
  and `Cache-Control: no-store`.
- Public `/api/system` returned Nexus-BS `v0.1.61`, user-agent
  `Nexus-BS/v0.1.61`, stack `v0.1.61-4ab64b2e-modified`, runtime config
  `/run/nexus-bs-chris/config.toml`, persistent config
  `/home/chris/nexus-bs/config.toml`, CPU `Broadcom Cortex-A53 1GHz 64-bit`,
  and SDR `SXceiver`.

## 2026-06-09 12:20 EEST - External dashboard RF Ops/Traffic revamp

Scope:

- Dashboard/API-only observability patch. No TETRA air-interface protocol,
  CMCE, UMAC/MAC scheduling, LLC/MM/SDS, RF audio or call-control behavior was
  changed.
- Ran a small read-only architect/QA swarm for eBTS field-dashboard content and
  live-refresh correctness. Integrated findings locally; agents did not modify
  RF/protocol code.
- Added read-only `GET /api/site` in the core dashboard API. It reads existing
  `SharedConfig` and cached dashboard telemetry only:
  - MCC/MNC, LA, colour code, carrier, band, duplex spacing, derived TX/RX
    frequencies and configured SoapySDR TX/RX/sample-rate/gains.
  - Energy economy config label (`auto`, `StayAlive`, `EG1..EG7`).
  - Current TS1-TS4 operational summary based on existing dashboard call state.
  - Cached RF health/quality snapshots already carried by telemetry.
- Revamped external dashboard assets in `dashboard/`:
  - System/RF Ops page now shows host, carrier plan, RF device, signal quality,
    and timeslot cards.
  - Traffic page now has Current Floor, active calls, live radios, and integrated
    Last Heard with cached RadioID resolution.
  - Settings/Admin page now shows live cell/RF config values plus explicitly
    disabled update/admin status.
  - About page keeps Nexus-BS identity, version-by-runtime, Chris YO3TCO project
    line, and BlueStation/FlowStation/SXCEIVER/all-contributors credits.
- Fixed dashboard dynamic-refresh correctness:
  - `/api/calls` reconciliation no longer clears `state.radios`, so the radio
    table and registered count do not flicker between full snapshots.
  - Core online/offline status is debounced across WebSocket reconnects and
    recent successful REST polls; short WS reconnects no longer flash OFFLINE.
  - Group-call hangtime is labelled as hangtime and excluded from active-call
    counts. Last speaker is shown separately from current active speaker.
  - `speaker_changed` no longer mutates the original group-call caller ISSI.
  - Current Floor fields now render target, speaker, call ID, TS, call seconds,
    and speaker seconds.
  - `ts_voice`, `tx_visual`, `tx_quality`, `sdr_health`, and `sys_health` WS
    events are consumed by the external dashboard state.
  - RadioID lookup UI now distinguishes queued/pending/retrying/not-found while
    keeping bounded queueing, localStorage cache and rate limiting.

Verification:

- `node --check dashboard/assets/app.js` passed.
- `cargo fmt -p tetra-entities` passed/applied formatting.
- `cargo test -p tetra-entities external_dashboard_asset_manifest_is_coherent --locked`
  passed.
- `cargo test -p tetra-entities dashboard_unknown_api_paths_are_reserved_from_spa_fallback --locked`
  passed.
- `cargo check -p tetra-entities --locked` passed.
- `cargo check -p nexus-bs-dashboard --locked` passed.
- `git diff --check` passed.

Next:

- Deploy with `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
- Verify on `chris@192.168.1.179`:
  - `curl http://127.0.0.1:8080/api/site`
  - `curl http://127.0.0.1:8080/api/calls`
  - systemd split remains: core on loopback API, external dashboard on `:8080`,
    separate CPU affinity/cgroup from the RF/TETRA core.

Next RF gate:

1. Deploy this patch.
2. Test GSSI `226333` as a group call only: open group call, alternate floor with PTT from `2260618`, `2260082`, and old Motorola `2260616`, including rapid repeated PTT from the Motorola terminals.
3. Expected log after requester grant: `STCH D-INFO reset T310 ... usage=None omitted_dl_only_chan_alloc=true`.
4. Expected RF behaviour: after `D-TX GRANTED(Granted)` to the requester, valid TCH/S should appear as `first accepted UL media after floor`.
5. If `accepted_ul_media_since_floor=0` persists with `usage=None`, continue with scheduler-selected-STCH and LMAC/PHY post-grant diagnostics; do not change CMCE/P2P/SDS/parrot.

## 2026-06-08 23:49 EEST - Group RF diagnostic proof logs for 2260082 MTP3550

Field context:

- Latest deployed functional patch was `8a15551` (`Omit group D-INFO reset channel allocation`), build `v0.1.59-8a155515`.
- Active service after that deploy had no new GSSI/floor RF test in the journal yet; the last observed `2260082` failure was from the previous run before `8a15551`.
- Previous failure signature stayed precise: CMCE granted `2260082`, UMAC activated floor, then `accepted_ul_media_since_floor=0`.

Component in simple technical terms:

- CMCE decides who may talk in the group.
- UMAC/MAC puts that decision on RF as STCH `D-TX GRANTED`, channel allocation, random-access ACK, and AACH traffic/assigned-control hints.
- LMAC/PHY proves whether the terminal actually sent valid TCH/S voice after the grant.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1 b) defines `D-TX GRANTED(Granted)` as the transmit authorization and U-plane-on edge for the granted MS.
- EN 300 392-2 clause 21.4.3.1 defines the random access acknowledgement flag in `MAC-RESOURCE`.
- EN 300 392-2 clause 23.5 requires assigned-channel signalling, traffic use, and AACH slot usage to remain coherent.
- This patch adds proof logging only; it does not claim formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Logs every STCH `D-TX GRANTED` diagnostic line with call id, address, transmission grant, RA ACK, usage marker, channel allocation, and whether it is the group requester positive grant.
  - Logs timer-only group `D-INFO reset T310` when its redundant DL-only channel allocation is omitted.
  - Logs the first accepted UL media after each floor grant, proving that LMAC/PHY delivered valid voice to UMAC.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Logs the actual AACH DL/UL usage when a group positive requester grant is pending for a traffic slot.

Verification:

- `cargo fmt --package tetra-entities -- --check` passed.
- `cargo test -p tetra-entities --test test_umac_bs test_group_floor --locked` passed: 6 tests.
- `cargo test -p tetra-entities --test test_cmce_bs group_226333 --locked` passed: 4 tests.
- `cargo test -p tetra-entities --test test_cmce_bs simplex_p2p --locked` passed: 13 tests.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

Next RF gate:

1. Deploy this diagnostic build.
2. Test local GSSI `226333` as a group call: open the group, alternate floor with PTT from `2260618`, `2260616`, and `2260082`, then rapid 2x PTT from `2260082`.
3. Read logs from service start. Required proof:
   - `STCH D-TX GRANTED ... addr=ISSI 2260082 ... ra_ack=true ... chan_alloc=... Both/Ul`.
   - `AACH group-positive-grant pending ... dl_usage=Traffic(...) ul_usage=Traffic(...)`.
   - If voice works, `first accepted UL media after floor ts=2 speaker=Some(ISSI 2260082)`.
   - If inactivity still reports `accepted_ul_media_since_floor=0`, the remaining fault is not CMCE floor ownership; continue at terminal grant interpretation or LMAC/PHY burst acceptance.

## 2026-06-08 23:44 EEST - Group D-INFO reset T310 no longer repeats DL-only channel allocation

Field evidence:

- RF test after commit `68bfc82` still failed on `2260082` Motorola MTP3550 MR19.9.
- Logs showed no `PTT denied`: `2260082` sent `U-TX DEMAND`, received individual `D-TX GRANTED(Granted)`, and UMAC activated the floor after reporter transmission.
- Failure remained `accepted_ul_media_since_floor=0`, followed by one regrant and then forced TX ceased.
- `2260618` worked in the same call sequence, so the remaining issue is terminal-specific interpretation of immediate post-grant MAC signalling, not generic CMCE floor ownership.

Component in simple technical terms:

- `D-INFO reset T310` only resets the group call timeout timer. It is not permission to talk.
- The BS was still carrying a DL-only MAC channel allocation with this timer-only D-INFO on assigned-channel STCH.
- A strict older Motorola can interpret that DL-only allocation immediately after its UL/Both requester grant as a receive-only reconfiguration.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1 b) makes individually addressed `D-TX GRANTED(Granted)` the transmit authorization and U-plane-on trigger for the granted MS.
- EN 300 392-2 clause 14.5.2.2.2 c) allows D-INFO to reset T310, but it does not grant transmit permission.
- EN 300 392-2 clause 23.5 requires assigned-channel MAC signalling to stay coherent with the CC procedure.
- This is clause-scoped engineering hardening, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Detects group-addressed D-INFO reset T310 on FACCH/STCH.
  - Keeps CMCE `chan_alloc` as local routing metadata to choose the assigned traffic timeslot.
  - Omits the MAC channel-allocation element from the over-air MAC-RESOURCE for this timer-only D-INFO.
  - Leaves `D-TX GRANTED`, `D-TX CEASED`, private/P2P, SDS, and parrot paths unchanged.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_group_d_info_reset_t310_stch_omits_dl_only_channel_allocation`.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs test_group_d_info_reset_t310_stch_omits_dl_only_channel_allocation --locked` passed.
- `cargo test -p tetra-entities --test test_umac_bs test_group_requester_d_tx_granted_stch_consumes_ready_random_access_ack --locked` passed.
- `cargo test -p tetra-entities --test test_umac_bs test_group_floor --locked` passed: 6 tests.
- `cargo test -p tetra-entities --test test_cmce_bs group_226333 --locked` passed: 4 tests.
- `cargo test -p tetra-entities --test test_cmce_bs simplex_p2p --locked` passed: 13 tests.
- `cargo check -p tetra-entities --locked` passed.
- `cargo fmt --package tetra-entities -- --check` passed.
- `git diff --check` passed.

Next RF gate:

- Deploy this patch and retest GSSI `226333` with `2260082` retaking floor after `2260618`.
- Expected improvement: after `D-TX GRANTED(Granted)` to `2260082`, no immediate DL-only timer D-INFO channel allocation should override the granted MS's transmit state.
- If `accepted_ul_media_since_floor=0` still repeats, next layer is RF/PHY/LMAC burst decode or terminal not transmitting despite coherent CC/MAC grant ordering.

## 2026-06-08 23:32 EEST - Group requester grant AACH/RA ACK hardening for Motorola MTP3550 MR19.9

Component in simple technical terms:

- CMCE decides which MS may talk in a GSSI group call; UMAC/MAC must then put the exact grant and access hints on the RF slot.
- AACH is the small per-slot MAC announcement that tells terminals whether a slot is traffic or assigned-control.
- Random access ACK is the MAC flag that confirms the terminal's `U-TX DEMAND` access was heard.
- Field evidence narrowed the remaining group-call issue to `2260082`, Motorola MTP3550 MR19.9: CMCE/UMAC granted the floor, but `accepted_ul_media_since_floor=0` showed no valid voice reached UMAC after the grant, while `2260616` and `2260618` passed repeated PTT cycles.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1 b) requires the SwMI to send an individually addressed `D-TX GRANTED` with `transmission granted` before the requester may transmit, and to inform other group members separately.
- The same clause says `D-TX GRANTED/Granted` switches U-plane on for the granted MS; the RF slot carrying that grant must not simultaneously look like assigned-control hangtime to a strict older terminal.
- EN 300 392-2 clause 21.4.3.1 defines the random-access acknowledgement flag in MAC-RESOURCE.
- EN 300 392-2 clause 23.5 requires assigned-channel signalling and traffic timing to stay coherent on FACCH/STCH.
- This is clause-scoped engineering hardening and focused regression evidence, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added detection for pending group requester positive `D-TX GRANTED` STCH: individual ISSI address, GSSI-primary bearer, uplink-capable channel allocation, and `TransmissionGrant::Granted`.
  - When that exact STCH is pending, AACH now advertises traffic usage instead of hangtime `AssignedControl/AssignedOnly`.
  - `D-TX CEASED` stays on the hangtime assigned-control path so floor-withdraw semantics are unchanged.
  - Added `take_pending_or_ready_ra_ack_for_stch` so a group requester grant can consume a matching ready `RandomAccessAck` still in the normal scheduler queue.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Uses the new ready-ACK helper only for group requester positive `D-TX GRANTED`; private/P2P, SDS, parrot, listener GSSI grants, and D-TX CEASED remain on existing paths.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added a real-ordering regression: queued ready RA ACK plus positive group requester `D-TX GRANTED` before UMAC `FloorGranted` must produce STCH `random_access_flag=true`.

Verification:

- `cargo test -p tetra-entities --lib test_hangtime_group_positive_floor_grant_aach_advertises_traffic_for_older_ms --locked` passed.
- `cargo test -p tetra-entities --lib test_hangtime_group_d_tx_ceased_aach_stays_assigned_control --locked` passed.
- `cargo test -p tetra-entities --test test_umac_bs test_group_requester_d_tx_granted_stch_consumes_ready_random_access_ack --locked` passed.
- `cargo test -p tetra-entities --test test_umac_bs test_group_floor --locked` passed: 6 tests.
- `cargo test -p tetra-entities --test test_umac_bs test_group_same_speaker_floor_retake_reopens_ul_traffic_for_lmac_tch_s_decode --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs group_226333 --locked` passed: 4 tests.
- `cargo test -p tetra-entities --test test_umac_bs test_private_floor_grant_stch_carries_preserved_random_access_ack --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs simplex_p2p --locked` passed: 13 tests.
- `cargo check -p tetra-entities --locked` passed.
- `cargo fmt --package tetra-entities -- --check` passed.
- `git diff --check` passed.

Next RF gate:

- Deploy direct only after this checkpoint is committed or explicitly selected for RF test.
- Clear volatile service journal, restart `nexus-bs@chris.service`, and test GSSI `226333` with `2260082` MTP3550 MR19.9 retaking PTT after `2260616` and `2260618`.
- Expected field behaviour: the positive grant slot for `2260082` should advertise traffic in AACH, carry the requester RA ACK if present, then produce valid accepted uplink media instead of `accepted_ul_media_since_floor=0`.
- If the counter still remains zero, the next layer is PHY/LMAC RF decode or terminal not transmitting after a standards-coherent grant, not CMCE floor ownership.

## 2026-06-08 20:08 EEST - LMAC preserves NTS2 Block2 TCH/S without stolen marker

Component in simple technical terms:

- LMAC BS is the lower MAC receiver. It looks at the PHY burst training sequence and decides whether the received uplink bits are signalling (`STCH`) or voice traffic (`TCH/S`).
- For NTS2 traffic bursts, the first half-slot is signalling; the second half-slot is voice unless the first-half MAC header explicitly says the second half is also stolen for signalling.
- This patch targets the live `2260082` group-call symptom where CMCE/UMAC grant the floor, but `accepted_ul_media_since_floor=0` shows no valid voice reached UMAC after the grant.

ETSI clause scope:

- EN 300 392-2 clause 23.8.4.1.4 says that on uplink slots assigned for traffic, NTS2 means the first half slot is STCH, and the BS shall inspect MAC headers to know whether the second half is also stolen.
- The same clause says if the second half slot is not stolen, the BS shall interpret the second half slot as TCH.
- EN 300 392-2 clause 23.8.5 says the BS should preserve uplink traffic half-slot handling and replace stolen halves appropriately on downlink.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/lmac/lmac_bs.rs`
  - Removed the stale local `!burst_is_traffic` guard that made NTS2 Block2 become STCH just because the local UL physical-channel marker still said CP/non-traffic.
  - NTS2 Block2 is now STCH only when `blk2_stolen_at` matches the exact uplink time; otherwise it is raw `TCH/S` Block2.
- `crates/tetra-entities/tests/test_lmac_bs.rs`
  - Added a regression where Block2 bits are deliberately valid as STCH, but without an explicit second-half-stolen marker they must still be preserved as raw TCH/S and must not be consumed as control.

Verification:

- `cargo test -p tetra-entities --test test_lmac_bs bs_lmac_preserves_seq2_block2_as_tch_s_even_if_bits_decode_as_stch_without_stolen_marker --locked` passed.
- `cargo test -p tetra-entities --test test_lmac_bs bs_lmac_recovers_seq2_block2_tch_s_after_hangtime_cp_marker_lag --locked` passed.
- `cargo test -p tetra-entities --test test_lmac_bs bs_lmac_ignores_stale_blk2_stolen_marker_for_later_tch_s_block2 --locked` passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` passed: 12 tests.
- `cargo test -p tetra-entities --test test_umac_bs group_floor --locked` passed: 6 tests.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` passed: 11 tests.
- `cargo test -p tetra-entities --test test_umac_bs test_group_floor_handoff_reopens_ul_traffic_for_lmac_tch_s_decode --locked` passed.
- `cargo check -p tetra-entities --tests --locked` passed.
- `cargo fmt --package tetra-entities -- --check` passed.
- `git diff --check` passed.

Next RF gate:

- Deploy direct to `/home/chris/nexus-bs/nexus-bs`.
- Clear volatile service journal.
- Test local group `226333`, especially `2260082` retaking PTT after another speaker.
- Expected improvement: after positive `D-TX GRANTED` to `2260082`, valid NTS2 Block2 voice should reach UMAC instead of being consumed as STCH during marker lag.
- If `accepted_ul_media_since_floor=0` still appears, the remaining evidence points lower: PHY duplicate candidate selection, RF decode/CRC, or the terminal not transmitting after grant.

## 2026-06-08 19:47 EEST - Rejected group hangtime D-TX CONTINUE candidate

Component in simple technical terms:

- CMCE group call floor control is the BS logic that decides which terminal may talk on a GSSI and tells all other terminals to listen.
- Hangtime/no-active-speaker is the normal short pause after a terminal releases PTT and sends `U-TX CEASED`.
- Withdrawn transmission / WAIT is a different state where the SwMI explicitly pauses a transmission with `D-TX WAIT`.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1 b) says the SwMI grants a group speaker with an individually addressed `D-TX GRANTED` and informs the rest of the group with group-addressed `D-TX GRANTED`.
- EN 300 392-2 clause 14.5.2.2.1 d) case 3 says that when transmission was withdrawn and a terminal requested permission during that withdrawn period, the SwMI should first send group-addressed `D-TX CONTINUE` with Continue set to `not continue`, then send `D-TX GRANTED`.
- This withdrawn-transmission clause does not describe normal hangtime after `U-TX CEASED`; normal group retake stays on `U-TX DEMAND` -> individual `D-TX GRANTED` to the new speaker plus group listener indication.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Audit result:

- Candidate patch rejected before commit/deploy: sending `D-TX CONTINUE/not-continue` in ordinary hangtime would mix the withdrawn-transmission procedure into a normal floor retake.
- The candidate also only proved order in the CMCE queue. UMAC priority scheduling could still transmit the positive `D-TX GRANTED` before the `D-TX CONTINUE`, so it would not be a reliable RF-order fix without a lower-layer ordering change.
- No protocol code from this candidate should be committed unless a future patch introduces a real `D-TX WAIT`/withdrawn state and enforces RF transmit order.

Verification:

- Candidate local tests had passed, but the ETSI/state audit overruled the patch before deploy.
- The important lesson is that a passing CMCE queue test is not enough for group-call RF correctness when STCH/FACCH priority may reorder traffic.

Next execution:

- Keep normal hangtime floor retake ETSI-clean.
- Investigate the real field failure below CMCE: FACCH/STCH grant delivery timing, UMAC priority/order, LMAC/PHY decode, RF level, and terminal-specific grant reception for `2260082`.
- Key diagnostic remains `accepted_ul_media_since_floor=0` after a sent grant: BS decided floor correctly, but no valid uplink TCH/S reached UMAC for that floor epoch.

## 2026-06-08 01:15 EEST - Nexus-BS v0.1.58 Parrot/P2P field checkpoint

Release note:

- Bumped Nexus-BS release identity from `v0.1.57` to `v0.1.58`.
- This release checkpoint preserves the current RF field result: Parrot/Papagal `99999` is treated as 100% OK for the current test build, and private P2P is treated as 99% working with the remaining Motorola-visible `No answer` final-clearance issue to be fixed.
- This is not a formal ETSI/TETRA certification claim. It is a field checkpoint plus clause-scoped engineering evidence for the touched behavior.

Component in simple technical terms:

- Product identity is the version shown by the binary banner, dashboard, User-Agent, control protocol, telemetry protocol, README, example config, and systemd samples.
- It does not change the TETRA call-control or media procedure by itself.

Patch:

- `Cargo.toml` and `Cargo.lock` now identify workspace crates and binaries as `0.1.58`.
- `tetra_core::PRODUCT_VERSION_TAG`, User-Agent, control protocol and telemetry protocol track `v0.1.58`.
- Dashboard identity tests, README, example config, and systemd sample descriptions now reference `Nexus-BS v0.1.58`.

Verification:

- `cargo check -p tetra-core -p tetra-entities -p tetra-saps --tests` passed and refreshed `Cargo.lock`.
- `cargo test -p tetra-core --locked product_identity_tracks_workspace_version` passed.
- `cargo test -p tetra-entities --locked product` passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked parrot` passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked parrot` passed.
- `cargo check -p tetra-core -p tetra-entities -p tetra-saps --tests --locked` passed.
- `git diff --check` passed.

Deploy:

- Built locally with `scripts/nexus-bs-test-deploy.sh` using `RUN_TESTS=0` after the focused locked verification above.
- Deployed directly to `/home/chris/nexus-bs/nexus-bs`; no remote binary backup was created.
- Live SHA-256 on `chris@192.168.1.179`: `fb299c08a2ce2787a379594d43d64cb720422e1dd9b42e7c19b7efcf9b5139f4`.
- Service `nexus-bs@chris.service` active/running with PID `77133`, active since `2026-06-08 11:12:03 EEST`.
- Startup banner reports `Version: Nexus-BS v0.1.58`, build `v0.1.58-9773aee2-modified`.

## 2026-06-08 01:05 EEST - Papagal 99999 isolated Hytera compatibility gate

Component in simple technical terms:

- CMCE Parrot/Papagal is the local virtual private-call service on ISSI `99999`.
- It answers a simplex private call, records the caller's uplink TCH/S voice frames, plays the exact frames back after PTT release, then clears only the real caller leg.
- UMAC is the media scheduler that converts those recorded TCH/S frames back to downlink traffic on the assigned timeslot.

RF problem reported:

- Hytera `2260616 -> 99999` Parrot playback sounded bad and displayed `call modified`.
- Normal private simplex P2P and group call audio were reported very good, so this stage must stay isolated to Parrot and must not alter normal P2P or GSSI media paths.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.1.1 says the SwMI may answer a U-SETUP with D-CALL PROCEEDING, D-ALERT, or D-CONNECT during mobile-originated call setup.
- EN 300 392-2 table 14.62 defines the hook method selection IE: `0` direct through-connect, `1` on/off-hook signalling.
- EN 300 392-2 clause 14.5.1.1.1 says if D-CALL PROCEEDING, D-ALERT, or D-CONNECT indicates a service different from the one requested, the user application may treat the call as changed.
- Therefore the Parrot virtual service now preserves the caller's requested hook method in D-CALL PROCEEDING and D-CONNECT instead of forcing direct through-connect.
- EN 300 392-2 note near clause 23.5.4.3.1 warns that a simplex MS should not attempt simultaneous transmit and receive on one circuit-mode call. Parrot playback remains after U-TX CEASED, not while the caller is still transmitting, and the virtual-speaker grant is DL-only because ISSI `99999` is not an RF peer needing uplink.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Parrot D-CALL PROCEEDING and D-CONNECT preserve `U-SETUP.hook_method_selection`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Parrot playback announces the virtual speaker with D-TX GRANTED/GrantedToOtherUser using `UlDlAssignment::Dl`, so Hytera-class callers are receive-only while the virtual peer plays back audio.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/parrot.rs`
  - Playback now skips frame 18 because the current UMAC scheduler does not send circuit traffic there.
  - Playback waits a short guard after the D-TX GRANTED/FACCH handoff before sending the first recorded TCH/S frame.
  - Release waits a short drain guard after the final queued playback frame so CMCE does not close the local circuit before UMAC/LMAC can transmit the tail.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - LocalParrot downlink playback uses the `_from_ul` TCH/S scheduling path for both ACELP and raw Block2, matching the working UMAC timing/source path used by local P2P/group media.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Per-frame FACCH/STCH and raw TCH/S preservation logs were demoted from INFO to DEBUG to reduce voice-test log pressure.
- `crates/tetra-entities/src/phy/phy_bs.rs`
  - Per-burst `rx_tpsap_prim got NormalTrainSeq...` logging was demoted from INFO to DEBUG to avoid CPU/log pressure during voice tests.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs --locked parrot` passed: 4 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked parrot` passed: 3 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked test_tmd_dl_req_raw_block2_playback_preserves_tch_s_halfslot -- --exact` passed.
- `cargo check -p tetra-entities -p tetra-saps --tests --locked` passed.
- `git diff --check` passed.
- Live deploy SHA on `chris@192.168.1.179` is `c63beaecb3dc70a3c409b5e8ce69094f8839421ca91266a48b1cdd32d61d371f`, active since `2026-06-08 01:05:03 EEST`.
- The RF gate is armed against this live build; the volatile journal was rotated/vacuumed immediately before asking for the Hytera `2260616 -> 99999` test.

Next RF gate:

- Test only Hytera `2260616 -> 99999`.
- Expected log shape: `U-SETUP ... called_party=99999 ... hook=true`, Parrot D-CALL PROCEEDING/D-CONNECT with `hook_method_selection: true`, `D-TX GRANTED/GrantedToOtherUser` with DL-only allocation, `U-TX CEASED (parrot) ... starting paced playback`, then `parrot playback complete, releasing`.
- Required RF result: no `call modified`, playback intelligible, no regression in P2P/group.

## 2026-06-07 22:00 EEST - Private simplex peer clear restored to tail-drained D-DISCONNECT

Component in simple technical terms:

- CMCE individual-call release is the BS state machine that handles red-key hangup in a private call.
- The disconnecting MS receives `D-RELEASE`; that is the acknowledgement required after its `U-DISCONNECT`.
- The other MS can be cleared by `D-DISCONNECT`, which explicitly asks it to release the call and answer with `U-RELEASE`.
- The bearer tail drain keeps final call-control signalling behind the last interleaved speech/tail frames so voice is not cut by the release PDU.

RF failure analysed:

- Deployed build `v0.1.57-7a895d8b` fixed the caller `D-CONNECT` STCH packing regression.
- RF test `2260082 -> 2260618` reached active private simplex successfully:
  - called `D-CONNECT ACKNOWLEDGE notification_indicator=Some(19)`;
  - caller `D-CONNECT notification_indicator=None`;
  - assigned-channel retry fit exactly as `MAC-RESOURCE hdr 76 + SDU 38 + fill 6 bits -> 124 STCH bits`;
  - `FloorGranted` was emitted for `2260082`, then later for `2260618`;
  - speech was present in both PTT directions.
- Final close still produced terminal-visible `No Answer` on `2260618`.
- Log cause: after `2260082` sent `U-DISCONNECT(UserRequestedDisconnection)`, BS sent prompt `D-RELEASE` to `2260082`, then peer clear by `D-RELEASE(UserRequestedDisconnection)` to `2260618`. Motorola rendered that peer-side final `D-RELEASE` as `No Answer` even though the call was established.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.3.1 requires the MS that sent `U-DISCONNECT` to wait for `D-RELEASE`.
- The same clause says the SwMI should inform the other MS of call clearance either by `D-DISCONNECT` or `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3 says an MS receiving `D-DISCONNECT` responds with `U-RELEASE`; an MS receiving `D-RELEASE` sends no response.
- EN 300 392-2 clause 23.8.2.2 says after `U-TX CEASED` or `U-DISCONNECT`, BS should issue tail bits before sending `D-TX CEASED`, `D-RELEASE`, or `D-DISCONNECT` to receiving MSs.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Local simplex `U-DISCONNECT` still sends prompt `D-RELEASE(UserRequestedDisconnection)` to the initiator.
  - Peer clear now waits for existing private-simplex tail drain, then uses `D-DISCONNECT(UserRequestedDisconnection)` rather than peer `D-RELEASE`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Tail-drain completion now starts the existing reporter-tracked `D-DISCONNECT -> U-RELEASE` state machine for the peer.
  - Existing timeout behaviour is preserved: if peer `U-RELEASE` never arrives, BS closes locally after the bounded guard without sending another peer clear PDU.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated full private setup/release workflow, MXP600 field regressions, stale PTT/disconnect windows, pending release, call-id wrap, and called-party-disconnect coverage to assert tail-drained peer `D-DISCONNECT`, peer `U-RELEASE`, and no premature circuit close.

Verification:

- `cargo fmt --package tetra-entities` completed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked test_simple_private_call_full_direct_setup_and_release_workflow -- --exact` passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked mxp600` passed: 2 tests.
- `cargo test -p tetra-entities --test test_cmce_bs --locked p2p` passed: 82 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked private` passed: 20 tests.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

Next RF gate:

- Commit and deploy directly to `/home/chris/nexus-bs/nexus-bs`.
- Clear volatile journal.
- Repeat exactly one local private simplex test: `2260082 -> 2260618`, one PTT each direction, close from `2260082`.
- Expected log shape: initiator `D-RELEASE(UserRequestedDisconnection)`, tail-drained peer `D-DISCONNECT(UserRequestedDisconnection)`, optional peer `U-RELEASE`, no fallback peer `D-RELEASE`, no BS restart.
- Required RF result: no false `No Answer` on `2260618` and no Motorola soft reboot.

## 2026-06-07 21:41 EEST - Private simplex caller D-CONNECT kept compact for STCH recovery

Component in simple technical terms:

- CMCE is the private-call control logic. It sends the called-side `D-CONNECT ACKNOWLEDGE`, then the caller-side `D-CONNECT`, and only opens the initial simplex voice floor after the caller leg is delivered.
- UMAC/MAC is the radio scheduler. During private-call recovery it may need to send call-control signalling as FACCH/STCH inside the already assigned traffic channel.
- `notification_indicator` is optional call-control UI/status information. It must not make an assigned-channel recovery PDU too large to fit the RF signalling slot.

RF failure analysed:

- The previous connected-notification patch put notification value `19` (`Called user connected`) on both the called `D-CONNECT ACKNOWLEDGE` and caller `D-CONNECT`.
- In the failed `2260082 -> 2260618` private simplex RF test, the caller `D-CONNECT` missed its first current-channel L2 ACK and needed assigned-channel recovery.
- UMAC reported the concrete packing failure: `MAC-RESOURCE hdr 76 + SDU 49 bits > 124`, then fell back to MCCH/SCH-F instead of FACCH/STCH.
- The caller never acknowledged that `D-CONNECT`; CMCE released setup with `AcknowledgedServiceNotComplete`.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 require `D-CONNECT ACKNOWLEDGE` and `D-CONNECT` to complete individual-call setup and carry the transmit permission state.
- EN 300 392-2 clause 14.5.1.2.1 keeps U-plane transmit permission under SwMI control; Nexus-BS still opens private-simplex media only after caller `D-CONNECT` delivery.
- EN 300 392-2 Annex D.4 describes the direct-call same-cell acknowledgement race and supports assigned-channel recovery after traffic channel assignment.
- The connected notification is optional UI/status information; it is not allowed to break the mandatory caller setup delivery path. This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Caller `D-CONNECT` now keeps `notification_indicator = None` so first delivery and assigned-channel retry remain compact.
  - Called `D-CONNECT ACKNOWLEDGE` still carries notification value `19`; this preserves the Motorola UI hint on the called leg without increasing the caller recovery PDU.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added assertions that both the first caller `D-CONNECT` and the assigned-channel retry keep `notification_indicator = None`.
  - Updated the full private setup/release workflow to preserve called ACK notification `19` while caller `D-CONNECT` remains compact.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added an RF-size regression proving acknowledged caller `D-CONNECT` with `notification_indicator = None` fits FACCH/STCH with `MAC-RESOURCE` channel allocation.
  - The same regression proves adding optional notification `19` to caller `D-CONNECT` reproduces the over-capacity shape (`header + SDU > 124`).

Verification:

- `cargo fmt --package tetra-entities` completed.
- `cargo test -p tetra-entities --test test_umac_bs --locked test_private_caller_d_connect_assigned_channel_recovery_fits_stch_when_compact -- --exact` passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked test_p2p_caller_d_connect_missing_l2_ack_retries_on_assigned_channel_before_floor -- --exact` passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked test_simple_private_call_full_direct_setup_and_release_workflow -- --exact` passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked p2p` passed: 82 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked private` passed: 20 tests.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

Next RF gate:

- Commit and deploy directly to `/home/chris/nexus-bs/nexus-bs`.
- Clear volatile journal.
- Repeat one controlled local private simplex test: `2260082 -> 2260618`, first PTT short voice, called side response, normal caller close.
- Expected log shape: called `D-CONNECT ACKNOWLEDGE notification_indicator=Some(19)`, caller `D-CONNECT notification_indicator=None`, no `does not fit STCH`, assigned-channel retry ACKs if needed, then `FloorGranted`.

## 2026-06-07 09:08 EEST - Private simplex caller D-CONNECT assigned-channel recovery

Component in simple technical terms:

- CMCE is the call-control state machine. It decides the private-call setup sequence and when UMAC may open the voice floor.
- `D-CONNECT` is the downlink setup PDU sent to the calling MS after the called side has accepted and received its `D-CONNECT ACKNOWLEDGE`.
- FACCH/STCH is the signalling path inside the assigned traffic channel. It is needed when a terminal has already moved away from MCCH after channel allocation.

RF failure analysed:

- `2260616 -> 2260618` with on/off-hook signalling succeeded: caller `D-CONNECT` was L2-ACKed and CMCE emitted `FloorGranted`.
- `2260082 -> 2260618` with direct setup failed: called `D-CONNECT ACKNOWLEDGE` completed, but caller `D-CONNECT` to `2260082` was retransmitted and never L2-ACKed. UMAC correctly deferred/dropped ACELP before `FloorGranted`, producing the user-visible symptom: destination channel opened, but no voice.
- The prior UMAC BL-ADATA ACK-attribution patch was not sufficient for this Motorola caller path; the remaining gap was CMCE retrying caller `D-CONNECT` only through the current-channel delivery path.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1 says SwMI controls which MS may transmit and an MS may begin U-plane transmission only after permission.
- EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 require `D-CONNECT ACKNOWLEDGE`/`D-CONNECT` to indicate which party may transmit.
- EN 300 392-2 Annex D.4 describes the same-cell direct setup race: after channel allocation, the BS may need to repeat/page on the assigned traffic channel until it receives layer-2 acknowledgement.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Added `PrivateCallerDConnectDelivery`.
  - First caller `D-CONNECT` remains current-channel with acknowledged L2 service and channel allocation.
  - Retry attempts after missing caller L2 ACK now use assigned-channel recovery (`stealing_permission=true`) so the same acknowledged `D-CONNECT` can be delivered via FACCH/STCH to a terminal that already moved to the traffic channel.
  - U-plane floor remains blocked until the caller `D-CONNECT` reporter is L2-acknowledged.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added regression coverage proving caller `D-CONNECT` retry moves to assigned-channel recovery and still does not emit `FloorGranted` until the caller BL-ACK.

RF gate after deploy:

- Deployed commit `aa77b7c0` as build `v0.1.57-aa77b7c0` to `nexus-bs@chris.service`.
- Test: `2260082 -> 2260618` private simplex, direct setup (`hook=false`).
- First caller `D-CONNECT` on current-channel signalling missed L2 ACK and exhausted LLC retransmission/late-ACK grace.
- CMCE retried caller `D-CONNECT` via assigned-channel recovery FACCH/STCH at `09:04:55.403`.
- Caller `D-CONNECT` was L2-acknowledged at `09:04:55.504`, then CMCE activated call `call_id=4` and emitted `FloorGranted` for ISSI `2260082`.
- Subsequent floor turns from both `2260082` and `2260618` produced `U-TX DEMAND`, `FloorGranted`, and `speech_present=true`.
- Caller disconnect at `09:05:14.655` produced assigned-channel `D-RELEASE` to initiator and peer clear by `D-RELEASE` to `2260618`; UMAC closed DL/UL circuit for `ts=2`.
- No `PTT denied`, `Network trouble`, `AcknowledgedServiceNotComplete`, BS crash, or systemd restart was observed in the RF log.

## 2026-06-07 08:49 EEST - Private simplex pre-floor BL-ADATA ACK attribution fix

Component in simple technical terms:

- UMAC is the radio scheduler/adapter that receives STCH signalling on an assigned traffic channel and forwards its LLC payload upward.
- LLC `BL-ACK` confirms a previous acknowledged downlink transfer, such as caller `D-CONNECT`.
- LLC `BL-ADATA` can carry both an ACK number and new data. Before private-simplex `FloorGranted`, STCH has no ISSI field, so UMAC cannot know which private-call participant sent it.

RF failure analysed:

- In the failed `2260082 -> 2260618` private simplex call, CMCE sent called-side `D-CONNECT ACKNOWLEDGE`, then caller `D-CONNECT` to `2260082`.
- The called Motorola opened the private channel, but the caller `D-CONNECT` L2 ACK never matched at LLC, so CMCE never emitted `FloorGranted` and UMAC deferred/dropped media instead of forwarding voice.
- Current code already duplicated pure pre-floor `BL-ACK` to both private participants, but `BL-ADATA` remained attributed only to the temporary primary ISSI. If the caller's ACK arrived as `BL-ADATA`, LLC could see it under the wrong ISSI and the setup would stall.

ETSI clause scope:

- EN 300 392-2 Annex D.4 describes individual-call direct setup where the called side is moved to the assigned channel and the BS authorizes caller transmission after setup signalling progress.
- EN 300 392-2 clause 21.4.5 carries MAC-U-SIGNAL on STCH without an explicit ISSI address.
- EN 300 392-2 clause 22.3.2.3 treats `BL-ADATA` as carrying ACK state as part of basic-link acknowledged transfer.
- EN 300 392-2 clause 14.5.1.2.1 keeps U-plane transmit permission under SwMI control; this patch does not open speech before CMCE `FloorGranted`.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Replaced pre-floor private ACK routing with an ACK-only routing path.
  - Before private-simplex `FloorGranted`, both `BL-ACK` and `BL-ADATA` ACK responses are converted to ACK-only `BL-ACK` copies and delivered to both private-call ISSI candidates.
  - Any ambiguous pre-floor `BL-ADATA` or `BL-ACK` payload is not duplicated under both ISSIs; only the ACK header is used to let LLC match pending `D-CONNECT`.
  - Once `FloorGranted` exists, normal STCH routing by the known speaker remains unchanged.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added a regression test proving pre-floor `BL-ADATA` routes ACK-only copies to both participants and strips payload.
  - Kept the pure `BL-ACK` candidate-routing guard.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs stch_bl_ --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p_caller_d_connect --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs private --locked` -> 19 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 81 passed.
- `cargo check -p tetra-entities --locked` passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 82 passed.
- `git diff --check` passed.

Next RF gate:

- Build locally and deploy direct to `/home/chris/nexus-bs`.
- Clean journal and run one structured private simplex RF test, preferably `2260082 -> 2260618`, then read logs for `caller D-CONNECT L2-acknowledged` followed by `FloorGranted`.

## 2026-06-07 08:18 EEST - Private simplex Motorola caller D-CONNECT ACK attribution fix

Component in simple technical terms:

- `MAC-U-SIGNAL` on STCH is the signalling path carried on the assigned traffic channel.
- `BL-ACK` is the LLC acknowledgement that confirms an acknowledged downlink transfer such as caller `D-CONNECT`.
- In private simplex on one shared assigned timeslot, STCH signalling carries no ISSI address before `FloorGranted`; UMAC must infer which participant sent the ACK from bearer context.

RF evidence:

- After log clean, two P2P calls were captured.
- `call_id=15`, `2260616 -> 2260618`, succeeded: called `D-CONNECT ACKNOWLEDGE`, caller `D-CONNECT`, caller L2 ACK, setup-time `FloorGranted`, repeated floor changes, and final `D-RELEASE` all completed.
- `call_id=16`, `2260082 -> 2260618`, failed before activation: called leg completed, caller `D-CONNECT` to `2260082` retransmitted/exhausted, media frames accumulated while U-plane was correctly blocked, then CMCE released setup with `AcknowledgedServiceNotComplete`.
- A late `U-CONNECT for unknown call_id=16` arrived after release, indicating the failure was setup/ACK timing or attribution, not a post-activation audio-floor bug.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1/14.5.1.1.2 define setup `D-CONNECT ACKNOWLEDGE` / `D-CONNECT` grant semantics.
- EN 300 392-2 clause 14.5.1.2.1 keeps U-plane transmit permission under SwMI control and requires permission before circuit-mode U-plane transmission.
- EN 300 392-2 Annex D.4 allows repeated/unacknowledged setup signalling where the layer-2 ACK path is unreliable; Nexus-BS still keeps caller `D-CONNECT` acknowledged and gates `FloorGranted` on its ACK.
- EN 300 392-2 clause 21.4.5 STCH/MAC-U-SIGNAL carries assigned-channel signalling without an explicit ISSI field, so pre-floor private BL-ACK must be attributed by bearer participant context.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - For pre-floor private-simplex STCH `MAC-U-SIGNAL`, pure standalone `BL-ACK` is routed to all ISSI participants on the shared private bearer.
  - LLC remains the authority that accepts only the ACK matching a pending acknowledged transfer by SSI/N(S).
  - Non-ACK STCH before floor remains blocked; `BL-ADATA` is not duplicated because it may carry payload.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added `ul_circuit_issi_participants()` to expose private bearer participants to UMAC.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Corrected caller `D-CONNECT` retry/failure logs to say missing L2 ACK rather than missing local transmission.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Updated the pre-floor private BL-ACK test to assert both participant candidate ISSIs are delivered upward.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs stch_bl_ack_before_private_floor --locked` passed.
- `cargo test -p tetra-entities --test test_umac_bs private --locked` -> 18 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 81 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 81 passed.
- `cargo check -p tetra-entities --locked` passed.

## 2026-06-07 07:58 EEST - Nexus-BS v0.1.57 RF-good private simplex checkpoint

Component in simple technical terms:

- Release identity is the version/name layer used by binaries, dashboard, control, telemetry, User-Agent, docs, example config, and systemd descriptions.
- This checkpoint does not change CMCE/UMAC/LLC TETRA protocol behavior.
- The protected RF-good protocol base is commit `63c3b2f` (`Fix private simplex called connect ack delivery`).

RF result:

- Field report: private simplex P2P is now perfect on the current `63c3b2f` behavior.
- Do not regress the private simplex setup path without first re-reading the ETSI private-call clauses and this checkpoint.

ETSI clause scope:

- The protected behavior is the private simplex setup delivery path already documented below against EN 300 392-2 clauses 14.5.1.1.1, 14.5.1.1.2, 14.5.1.2.1, and Annex D.4.
- This release bump is metadata only and is not formal ETSI/TETRA certification.

Patch:

- Bumped Nexus-BS release identity from `v0.1.56` to `v0.1.57`.
- Updated workspace version, Cargo lockfile package versions, product identity tests, dashboard expectations, control/telemetry subprotocol tests, README, example config, and systemd descriptions.

Verification:

- `cargo test -p tetra-core --locked` -> 47 passed.
- `cargo test -p tetra-entities net_control --locked` -> 22 passed.
- `cargo test -p tetra-entities net_telemetry --locked` -> 8 passed, 5 ignored transport tests.
- `cargo test -p tetra-entities net_dashboard --locked` -> 53 passed.
- `cargo check -p tetra-core -p tetra-entities --locked` passed.
- `git diff --check` passed.

## 2026-06-07 01:58 EEST - Private simplex called-leg delivery switched to Annex D.4 unacknowledged repeat path

Component in simple technical terms:

- CMCE private-call setup opens the call-control path between caller ISSI and called ISSI.
- `D-CONNECT ACKNOWLEDGE` is the called-side acceptance message sent after the called terminal sends `U-CONNECT`.
- Layer 2 acknowledged service waits for a BL-ACK from the terminal; unacknowledged service repeats the message without waiting for that BL-ACK.
- `FloorGranted` remains the internal UMAC media switch. It is still emitted only after caller `D-CONNECT` delivery, not when the called-side ACK message is merely queued.

RF failure analysed:

- Live `v0.1.56-8bf759df` test at `2026-06-07 01:49:43 EEST` showed `2260616 -> 2260618`, call_id `4`.
- Nexus-BS sent called-side `D-CONNECT ACKNOWLEDGE` to `2260618` with `TransmissionGrant::Granted`.
- The called-side BL-ACK never arrived after five acknowledged attempts, including assigned-channel recovery attempts.
- CMCE never sent caller `D-CONNECT`; setup was released at `01:49:52.922` with `AcknowledgedServiceNotComplete`.
- Therefore the failed RF test was not a first-floor/audio bug. It was a called-side setup delivery gate stuck on missing BL-ACK.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 require `D-CONNECT ACKNOWLEDGE` / `D-CONNECT` to identify the party permitted to transmit.
- EN 300 392-2 clause 14.5.1.2.1 says the SwMI controls transmit permission; during setup, the response to the transmit request is handled by 14.5.1.1.1/14.5.1.1.2, while later in-call changes use `U-TX DEMAND` / `D-TX GRANTED`.
- EN 300 392-2 Annex D.4 says if the BS does not receive the called-side layer 2 acknowledgement, it cannot know whether the downlink failed or only the uplink response failed. For repeat signalling, the BS should either use unacknowledged service, grant a subslot on MCCH before channel change, or delay the layer 2 acknowledgement until frame 18.
- Nexus-BS now uses the Annex D.4 unacknowledged repeat option for simplex called-side `D-CONNECT ACKNOWLEDGE`; duplex remains acknowledged.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Simplex called-side `D-CONNECT ACKNOWLEDGE` now uses `Layer2Service::Unacknowledged`.
  - It requests 3 BL-UDATA repetitions, giving 4 complete transmissions under the LLC `N.253 + 1` model.
  - CMCE proceeds to caller `D-CONNECT` after local transmission of the unacknowledged repeated called-leg message.
  - Duplex called-side `D-CONNECT ACKNOWLEDGE` remains `Layer2Service::Acknowledged`.
  - Caller `D-CONNECT` remains acknowledged and still gates initial `FloorGranted`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated P2P tests to assert the Annex D.4 unacknowledged repeat path for simplex called-leg `D-CONNECT ACKNOWLEDGE`.
  - Kept local-discard retry coverage: if MAC cannot transmit the message at all, CMCE retries and eventually releases setup.
  - Kept duplex tests on acknowledged called-leg delivery.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 81 passed.
- `cargo check -p tetra-entities --locked` passed.
- `cargo test -p tetra-entities --test test_umac_bs private --locked` -> 18 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 168 passed.
- `cargo fmt --package tetra-entities` passed.
- `git diff --check` passed.

Next RF gate:

- Commit and deploy direct.
- Clean journal.
- Ask for one structured private simplex test `2260616 -> 2260618`.
- Expected log: called-side `D-CONNECT ACKNOWLEDGE` sent as unacknowledged repeated delivery, then caller `D-CONNECT`, then setup-floor `FloorGranted` only after caller `D-CONNECT` ACK.

## 2026-06-07 01:46 EEST - Nexus-BS v0.1.56 release bump and private-simplex setup-floor correction

Component in simple technical terms:

- CMCE private-call setup is the BS logic that opens a private simplex call between two ISSIs.
- `D-CONNECT ACKNOWLEDGE` is sent to the called radio; `D-CONNECT` is sent to the calling radio.
- Their `transmission_grant` fields are not just cosmetic status: in private simplex they identify the setup-phase party allowed to transmit.
- UMAC `FloorGranted` is the internal BS media scheduler command that opens the uplink speech path for the selected ISSI.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 require the setup connect PDUs to indicate which party may transmit.
- EN 300 392-2 clause 14.5.1.2.1 b) treats this setup response as the initial transmit-permission path during setup; later in-call floor changes remain under `U-TX DEMAND` / `D-TX GRANTED`.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- Bumped Nexus-BS crate, dashboard, control, telemetry, user-agent, docs, example config, and systemd text from `v0.1.55` to `v0.1.56`.
- Restored setup grant polarity for private simplex:
  - called-first: called `D-CONNECT ACKNOWLEDGE = Granted`, caller `D-CONNECT = GrantedToOtherUser`;
  - caller-first: called `D-CONNECT ACKNOWLEDGE = GrantedToOtherUser`, caller `D-CONNECT = Granted`.
- After the caller `D-CONNECT` is L2-ACKed, CMCE now seeds exactly one setup-time UMAC `FloorGranted` for the ISSI selected by the ETSI setup grant.
- U-plane media is still blocked until both private-call setup legs have completed, so the BS does not open speech before caller-side setup delivery.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 81 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 168 passed.
- `cargo test -p tetra-entities --test test_umac_bs private --locked` -> 18 passed.
- `cargo check -p tetra-entities --locked` passed.
- `cargo test -p tetra-core --locked` -> 47 passed.
- `cargo test -p tetra-entities net_control --locked` -> 22 passed.
- `cargo test -p tetra-entities net_telemetry --locked` -> 8 passed, 5 ignored.
- `cargo test -p tetra-entities net_dashboard --locked` -> 53 passed.
- `cargo check -p tetra-core -p tetra-entities --locked` passed.
- `git diff --check` passed.

Next RF gate:

- Commit the v0.1.56 release state.
- Deploy direct to `/home/chris/nexus-bs` using local build only.
- Clean volatile journal and request one structured private simplex test before reading logs.

## 2026-06-07 01:42 EEST - Private simplex NotGranted regression reverted, setup floor still gated

Component in simple technical terms:

- CMCE private-call setup sends `D-CONNECT ACKNOWLEDGE` to the called radio and `D-CONNECT` to the caller.
- The `transmission_grant` field is terminal-facing call-control state. Motorola MXP600 uses it to decide whether the private call identifier is valid/usable.
- UMAC `FloorGranted` is internal BS scheduler state. It decides when Nexus-BS starts accepting/looping speech for one ISSI.

RF regression found:

- The previous patch made both simplex connect PDUs `TransmissionGrant::NotGranted`.
- Live RF immediately proved this is not acceptable for Motorola MXP600:
  - `2260616 -> 2260618`, call_id `4`.
  - Called `D-CONNECT ACKNOWLEDGE` was L2-ACKed.
  - Caller `D-CONNECT` was L2-ACKed.
  - `2260618` then sent `U-DISCONNECT cause=InvalidCallIdentifier`.
- A second attempt, call_id `5`, also completed silent setup but produced no useful `U-TX DEMAND`; the assigned channel later hit UL inactivity.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 say `D-CONNECT ACKNOWLEDGE` / `D-CONNECT` shall indicate which party is permitted to transmit.
- EN 300 392-2 table 14.80 defines `Granted`, `NotGranted`, `RequestQueued`, and `GrantedToOtherUser`.
- EN 300 392-2 clause 14.5.1.2.1 keeps later in-call transmit permission under `U-TX DEMAND` / `D-TX GRANTED`.
- This patch restores the terminal-facing setup grant polarity but does not restore automatic BS UMAC floor seeding.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Restored simplex setup grant polarity:
    - called-first: called `D-CONNECT ACKNOWLEDGE = Granted`, caller `D-CONNECT = GrantedToOtherUser`;
    - caller-first: called `D-CONNECT ACKNOWLEDGE = GrantedToOtherUser`, caller `D-CONNECT = Granted`.
  - Kept the internal `UMAC FloorGranted` withheld after caller `D-CONNECT` L2 ACK.
  - The first BS media floor still requires explicit `U-TX DEMAND`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated P2P tests to assert restored setup grants and zero setup-time UMAC floor.
  - Kept explicit first-PTT tests proving `U-TX DEMAND` produces `D-TX GRANTED` to both parties and one UMAC floor.

Verification:

- `cargo fmt --package tetra-entities` passed.
- `git diff --check` passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 81 passed.
- `cargo test -p tetra-entities --test test_cmce_bs private_simplex --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 168 passed.
- `cargo test -p tetra-entities --test test_umac_bs private --locked` -> 18 passed.
- `cargo check -p tetra-entities --locked` passed.

Next RF gate:

- Deploy directly to testing and clean journal.
- Test `2260616 -> 2260618` private simplex again.
- Expected improvement versus failed `NotGranted` build: no `U-DISCONNECT InvalidCallIdentifier` from `2260618`.
- Expected improvement versus setup-floor build: no log line `setup grant seeds initial floor` and no setup-time UMAC `FloorGranted` before real `U-TX DEMAND`.

## 2026-06-07 01:33 EEST - Private simplex silent setup restored after RF static/no-voice report

Component in simple technical terms:

- CMCE individual call control is the BS private-call state machine: it sets up the call, tells both radios the call is active, handles PTT requests, and clears the call.
- `D-CONNECT ACKNOWLEDGE` and `D-CONNECT` are call-control messages to the called and calling radios. Their `transmission_grant` field can switch a real radio's U-plane/audio path on.
- UMAC `FloorGranted` is the internal scheduler command that actually names the ISSI allowed to send uplink speech on the assigned traffic channel.

Issue covered:

- RF test confirmed a remaining regression: the first PTT used to initiate a private simplex call opened the called-side audio channel with no voice/static.
- Logs showed why: Nexus-BS sent setup `TransmissionGrant::Granted` / `GrantedToOtherUser` and emitted setup-time UMAC `FloorGranted` before any explicit `U-TX DEMAND`.
- That made setup behave like an audio floor, while the desired real-terminal behavior is silent call opening/acceptance and voice only after a participant presses PTT inside the established call.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 allow `D-CONNECT ACKNOWLEDGE` / `D-CONNECT` to complete setup with transmission not granted, provided the MS is allowed to request transmission permission.
- EN 300 392-2 clause 14.5.1.2.1 says the SwMI fully controls which MS may transmit; during call progress the MS requests permission with `U-TX DEMAND` and the SwMI answers with `D-TX GRANTED`.
- EN 300 392-2 clause 14.5.1.4.1 says `Granted` / `GrantedToOtherUser` switch U-plane on; `NotGranted` keeps it off.
- EN 300 392-2 table 14.81 value `0` means the MS is allowed to request transmission permission.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Private simplex `D-CONNECT ACKNOWLEDGE` and caller `D-CONNECT` now use `TransmissionGrant::NotGranted` with request permission allowed.
  - Caller `D-CONNECT` L2 ACK still activates the private call, but no setup-time UMAC `FloorGranted` is emitted.
  - First U-plane/audio floor now comes only from explicit participant `U-TX DEMAND`, which sends `D-TX GRANTED` to both private-call parties and then UMAC `FloorGranted`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated direct local P2P tests to assert silent setup: zero UMAC floor grants through `D-CONNECT` completion.
  - Added/kept explicit first-PTT guards proving `U-TX DEMAND` after connect opens exactly one floor and notifies both radios.
  - Kept Brew private-call tests separate because that path is network-origin/local media behavior and was not the RF failure under test.

Verification:

- `cargo fmt --package tetra-entities` passed.
- `git diff --check` passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 81 passed.
- `cargo test -p tetra-entities --test test_cmce_bs private_simplex --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 168 passed.
- `cargo test -p tetra-entities --test test_umac_bs private --locked` -> 18 passed.
- `cargo check -p tetra-entities --locked` passed.

Next RF gate:

- Deploy latest build directly to testing.
- Clean volatile journal.
- Test `2260616 -> 2260618` local private simplex.
- Expected log shape:
  - `D-CONNECT ACKNOWLEDGE` to `2260618` with `TransmissionGrant::NotGranted`.
  - `D-CONNECT` to `2260616` with `TransmissionGrant::NotGranted`.
  - no setup-time `private-simplex ... setup grant seeds initial floor` log and no UMAC `FloorGranted` until `U-TX DEMAND`.
  - first real PTT after call establishment shows `U-TX DEMAND`, two `D-TX GRANTED` PDUs, and one UMAC `FloorGranted`.
- Required RF result: first initiating PTT opens the private call silently on the other terminal, with no static/no-voice audio channel; voice starts only on the accepted/next PTT, release still shows proper call ended/disconnected state, and no BS/terminal crash.

## 2026-06-07 01:16 EEST - RF P2P simplex log gate after setup-grant deploy

Component in simple technical terms:

- CMCE individual call control is the BS logic that accepts a private call setup, tells each terminal its role, handles PTT turns, and clears the call.
- UMAC floor control is the radio scheduler part that gives one terminal permission to send speech on the assigned traffic channel.
- `speech_present=true` in scheduler logs means the traffic channel is carrying detected speech frames, not only signalling.

RF test observed:

- Test route: local P2P simplex `2260616 -> 2260618`, call_id `7`.
- Setup order matched the intended clause-scoped path:
  - `U-SETUP` from `2260616` to `2260618`.
  - Called leg `D-CONNECT ACKNOWLEDGE` to `2260618` with `TransmissionGrant::Granted`.
  - Caller leg `D-CONNECT` to `2260616` with `TransmissionGrant::GrantedToOtherUser`.
  - Call activation waited until caller `D-CONNECT` L2 ACK, then seeded initial UMAC floor for called ISSI `2260618`.
- First seeded floor timed out on local UL inactivity after about 3 seconds with no uplink speech, before the first explicit `U-TX DEMAND`.
- Later PTT turns from both `2260618` and `2260616` produced `U-TX DEMAND`, UMAC `FloorGranted`, `speech_present=true`, and tail-drained `D-TX CEASED`.
- Call close from `2260616` produced `U-DISCONNECT` and `D-RELEASE` with `UserRequestedDisconnection` to both legs.
- Service remained running: no Nexus-BS restart, no `No Answer`, no `Network trouble`, no `InvalidCallIdentifier`, no `PTT denied` in this P2P log window.

Interpretation:

- CMCE setup/release is improved and log-coherent for this RF gate.
- Remaining ambiguity is subjective terminal behavior during the first seeded called-party floor: the log shows no uplink speech until the later explicit `U-TX DEMAND`; this may be normal if the called MS did not press PTT inside the initial floor window, but it must be confirmed from the radios before closing the P2P item.
- No new protocol patch should be made from this log alone. Next action is to ask for the terminal symptom, then test reverse direction if audio/release was OK.

## 2026-06-07 01:05 EEST - Private simplex setup grant restored for local P2P, pending RF deploy

Component in simple technical terms:

- CMCE CC_BS individual call is the BS private-call control state machine: it owns `U-SETUP`, `D-SETUP`, `U-CONNECT`, `D-CONNECT ACKNOWLEDGE`, `D-CONNECT`, PTT floor, and release.
- UMAC FloorGranted is the internal radio scheduler command that names the ISSI currently allowed to send speech on the assigned traffic channel.
- LLC BL-ACK is the layer-2 proof that a terminal received an acknowledged downlink call-control PDU.

Issue covered:

- Live RF for `2260616 -> 2260618` failed after setup: logs showed called `D-CONNECT ACKNOWLEDGE` was BL-ACKed, caller `D-CONNECT` was BL-ACKed, then the call became active with no `U-TX DEMAND` and timed out on UL inactivity.
- The previous "no setup floor until U-TX DEMAND" rule was too strict for the first setup-phase transmit permission. It made local P2P differ from the already-working Brew private path.
- Agent review confirmed the current no-floor UMAC state can drop assigned-channel `MAC-U-SIGNAL` before CMCE if no current speaker is known.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2: `D-CONNECT ACKNOWLEDGE` and `D-CONNECT` shall indicate which party is permitted to transmit.
- EN 300 392-2 clause 14.5.1.2.1 b): during call setup, the response to the initial request to transmit is handled by clauses 14.5.1.1.1 and 14.5.1.1.2; later call-in-progress requests use `U-TX DEMAND -> D-TX GRANTED`.
- EN 300 392-2 clause 14.5.1.4.1: `TransmissionGrant::Granted` switches U-plane on for transmit and `GrantedToOtherUser` switches U-plane on for receive.
- EN 300 392-2 table 14.74: `request_to_transmit_send_data = 1` means the other MS may transmit/send data for on/off-hook setup.
- EN 300 392-2 table 14.81: `transmission_request_permission = 0` still allows later transmit requests.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Added private setup grant derivation from the cached U-SETUP method/request bit.
  - For simplex caller-first: called `D-CONNECT ACKNOWLEDGE = GrantedToOtherUser`, caller `D-CONNECT = Granted`.
  - For simplex called-first: called `D-CONNECT ACKNOWLEDGE = Granted`, caller `D-CONNECT = GrantedToOtherUser`.
  - After caller `D-CONNECT` L2 ACK, CMCE marks the call active and emits one UMAC `FloorGranted` for the setup-granted speaker.
  - No unsolicited `D-TX GRANTED` is sent; `D-TX GRANTED` remains only for later `U-TX DEMAND`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated local P2P direct setup tests to assert setup-derived initial floor after the caller `D-CONNECT` L2 ACK.
  - Preserved guards that no floor is emitted before called `D-CONNECT ACKNOWLEDGE` BL-ACK and caller `D-CONNECT` L2 ACK.
  - Added/updated called-first hook coverage so `D-CONNECT ACKNOWLEDGE` grants the called MS and `D-CONNECT` puts the caller in receive mode.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 81 passed.
- `cargo test -p tetra-entities --test test_cmce_bs private_simplex --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 168 passed.
- `cargo test -p tetra-entities --test test_umac_bs private --locked` -> 18 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 81 passed.
- `cargo test -p tetra-entities --test test_llc_bs matching_bl_ack --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 85 passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

Next RF gate:

- Deploy directly to testing.
- Clean journal.
- Test one local P2P simplex call `2260616 -> 2260618`.
- Expected log shape: called `D-CONNECT ACKNOWLEDGE` grant is `Granted` only for called-first setup or `GrantedToOtherUser` for caller-first setup; caller `D-CONNECT` carries the opposite grant; after caller L2 ACK, log must show `setup grant seeds initial floor speaker ISSI ...` and UMAC `FloorGranted`.
- If RF still fails with no first audio, inspect UMAC assigned-channel `MAC-U-SIGNAL` drop path next, not CMCE grant fields.

## 2026-06-06 20:35 EEST - Private P2P connect ACK state hardening, no RF deploy

Component in simple technical terms:

- CMCE CC_BS individual call is the BS private-call state machine for caller ISSI, called ISSI, setup, PTT floor, and release.
- LLC ACK is the terminal's lower-layer confirmation that a critical call-control PDU was received, not just locally queued/transmitted by the BS.
- `D-CONNECT ACKNOWLEDGE` completes the called leg after `U-CONNECT`; `D-CONNECT` completes the caller leg; simplex audio remains off until `U-TX DEMAND` receives `D-TX GRANTED`.

Issue covered:

- Live RF showed `Invalid number` / `Number busy` after the previous no-early-audio patch, and then a BS service crash was reported before a new deploy could be made.
- Local and agent review found the direct local P2P path activated the call when caller `D-CONNECT` was locally transmitted, before the caller L2 ACK proved the caller recognized the call identifier.
- That could make a caller `U-DISCONNECT(InvalidCallIdentifier)` during caller connect setup look like an established-call teardown, prematurely clear the called leg, and leave late `U-CONNECT` / next setup as busy.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2: called `U-CONNECT` is completed by `D-CONNECT ACKNOWLEDGE`, and caller setup is completed by `D-CONNECT`.
- EN 300 392-2 clause 14.5.1.2.1: simplex private transmit permission remains SwMI-controlled; the MS must use `U-TX DEMAND` and receive `D-TX GRANTED` before U-plane speech.
- EN 300 392-2 clause 14.5.1.4.1: `TransmissionGrant::NotGranted` keeps U-plane off at setup, even when request permission is allowed.
- EN 300 392-2 clauses 14.5.1.3.1 through 14.5.1.3.3: setup abort/reject/no-answer/busy/invalid cases are cleared with `D-RELEASE`; established peer `D-DISCONNECT` remains only for states where the MS is known to recognize the call id and a `U-RELEASE` response is expected.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/call.rs`
  - Added `CalledConnectAckPending` and `CallerConnectAckPending` states.
  - Added assigned-circuit/connect-pending helpers so setup-with-bearer is not confused with fully active call state.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Caller `D-CONNECT` now completes only after L2 ACK, not local transmission.
  - `U-CONNECT` duplicates in connect-ACK pending state are absorbed.
  - Call activation still emits no implicit simplex floor; first audio remains `U-TX DEMAND -> D-TX GRANTED`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - `U-DISCONNECT` / `U-RELEASE` logs now include sender ISSI and individual-call state.
  - Late `U-DISCONNECT` for a pending release repeats direct `D-RELEASE` instead of becoming unknown-call noise.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added connect-abort release path: caller leg is released on direct/current signalling; called leg uses assigned-channel `D-RELEASE` only after called connect ACK was L2-ACKed.
  - Pending-release late `U-ALERT` / `U-CONNECT` / `U-DISCONNECT` can be absorbed with direct `D-RELEASE`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Extended caller `D-CONNECT` no-L2-ACK test so premature `U-TX DEMAND` produces no `D-TX GRANTED` and no UMAC floor.
  - Added regression for caller `U-DISCONNECT(InvalidCallIdentifier)` during `CallerConnectAckPending`: no `D-DISCONNECT`, no floor, correct `D-RELEASE`, and fresh setup is not left busy.
  - Added setup-phase cause preservation coverage for `CallRejectedByTheCalledParty`, `CalledPartyBusy`, `ExpiryOfTimer`, and `InvalidCallIdentifier`.

Verification:

- `cargo fmt --package tetra-entities` passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_caller_d_connect_transmitted_without_l2_ack_does_not_seed_initial_floor --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_caller_invalid_call_identifier_during_caller_connect_ack_pending_releases_without_active_teardown --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_setup_phase_reject_busy_and_no_answer_causes_are_preserved --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 81 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 167 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 81 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 34 passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

Deploy / RF status:

- Not deployed. User is not near TetraHS/Pi, so continue locally only.
- SSH to `chris@192.168.1.179` was unavailable from the current network; no RF log read or remote restart was performed.

Host cleanup:

- `/private/tmp` was full because old Nexus-BS/FlowStation Cargo/Zig target directories consumed about 28G.
- Removed only enumerated temporary target/cache directories under `/private/tmp`; the repo was not touched.
- Free space improved from 1.8 GiB to 29 GiB; `/private/tmp` now reports about 52M used.

## 2026-06-06 19:15 EEST - Private simplex setup keeps U-plane off until U-TX DEMAND

Component in simple technical terms:

- CMCE CC_BS is the BS call-control finite-state machine for private calls.
- `D-CONNECT ACKNOWLEDGE` tells the called terminal that the private call setup completed.
- `D-CONNECT` tells the calling terminal that the private call setup completed.
- `UMAC FloorGranted` is the internal scheduler command that allows one ISSI to transmit audio on the assigned bearer.
- This patch separates "call active on signalling" from "audio floor active".

Issue covered:

- RF test after the previous deploy still showed `No Answer` at final clear.
- More importantly, live logs for `2260616 -> 2260618` showed the BS sent called-leg `D-CONNECT ACKNOWLEDGE` with `TransmissionGrant::Granted`, then emitted `initial private-simplex FloorGranted` for `2260618` before any `U-TX DEMAND`.
- The called Motorola opened receive/transmit U-plane and produced static/no voice before the user accepted/talked with PTT.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2: if transmission is not granted in `D-CONNECT ACKNOWLEDGE` / `D-CONNECT` but request permission is allowed, the MS follows the transmission control procedure.
- EN 300 392-2 clause 14.5.1.2.1: SwMI fully controls which MS may transmit; an MS must request and receive permission before U-plane transmission.
- EN 300 392-2 clause 14.5.1.4.1: `TransmissionGrant::Granted` switches U-plane on for transmit; `GrantedToOtherUser` switches U-plane on for receive; other grant values keep U-plane off.
- EN 300 392-2 table 14.74: U-SETUP `request to transmit/send data` value 1 requests that the other MS may transmit/send data, but this is now treated as setup preference, not as an implicit UMAC floor grant.
- EN 300 392-2 table 14.81: `transmission_request_permission = 0` means the MS may request transmission permission.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Private simplex `D-CONNECT ACKNOWLEDGE` and `D-CONNECT` now use `TransmissionGrant::NotGranted` with request permission allowed.
  - Caller `D-CONNECT` local transmission activates the private call but no longer emits an implicit `FloorGranted`.
  - First audio floor now comes only from a participant `U-TX DEMAND`, which receives `D-TX GRANTED` and then `UMAC FloorGranted`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated private simplex connect tests to assert zero setup-time floor grants.
  - Added/updated hook called-first regression coverage so the called MS gets floor only after its explicit `U-TX DEMAND`.
  - Updated P2P fixtures so tests that need an active speaker now simulate the first PTT explicitly.

Verification:

- `cargo fmt --package tetra-entities` completed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 79 passed.
- `cargo test -p tetra-entities --test test_cmce_bs mxp600 --locked` -> 2 passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

Deploy status:

- Deployed directly to `/home/chris/nexus-bs/nexus-bs`.
- Running service: `nexus-bs@chris.service`, `MainPID=51128`, active since `2026-06-06 19:14:28 EEST`.
- Deployed binary SHA: `6a9add61ea5b57b8531ad722f6f2ec25e3e141ac6b81cfefc0da88a6921580b0`.
- Journal was rotated/vacuumed after deploy for a clean RF gate; immediate post-clean read returned no entries.

Next RF gate:

1. Clean logs and restart Nexus-BS after deploy.
2. User performs private simplex `2260616 -> 2260618`.
3. Expected setup path:
   - `D-CONNECT ACKNOWLEDGE` to `2260618` with `TransmissionGrant::NotGranted`.
   - `D-CONNECT` to `2260616` with `TransmissionGrant::NotGranted`.
   - no `initial private-simplex FloorGranted` before `U-TX DEMAND`.
   - first `U-TX DEMAND` from the terminal that presses PTT receives `D-TX GRANTED`, and only then UMAC emits `FloorGranted`.
4. Required RF result for success: no static/no-voice channel opened before accepted PTT, no terminal-visible `PTT denied` in the established call, no false `No Answer` at normal close, and no Motorola soft reboot.

## 2026-06-06 12:31 EEST - Network time broadcast freshness patch staged locally

Component in simple technical terms:

- MLE builds `D-NWRK-BROADCAST`, the broadcast PDU that can carry TETRA network time.
- LLC wraps that MLE PDU into BL-UDATA and can repeat the same TL-SDU when `N.253` is left as default.
- For a clock PDU, repeating an old encoded timestamp can make a terminal display stale time even if the original timestamp was correct.

Issue covered:

- Field observation: a Motorola terminal clock appeared about two minutes behind while Nexus-BS was broadcasting time.
- Decoding the live log value `0x67164C18D7FF` from `12:17:12 EEST` showed the PDU itself encoded `2026-06-06T09:17:12Z` plus `UTC+3`, so no fixed offset was found in the encoder.
- The likely freshness risk was LLC repeating the same encoded `D-NWRK-BROADCAST` TL-SDU via the default unacknowledged repeat policy.

ETSI clause scope:

- EN 300 392-2 clause 18.4.1.4.1 / table 18.2: `D-NWRK-BROADCAST` may carry optional TETRA network time.
- EN 300 392-2 clause 18.5.24 / table 18.100: TETRA network time contains UTC time, local offset, year, and reserved bits.
- EN 300 392-2 clause 22.3.2.4.1: BL-UDATA transmissions are repeated `N.253 + 1` times. For clock freshness, Nexus-BS now explicitly uses `N.253 = 0`.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch staged locally:

- `crates/tetra-entities/src/mle/components/broadcast.rs`: `D-NWRK-BROADCAST` now sends `n_tlsdu_repeats = Some(0)` so each encoded timestamp has one LLC transmission.
- `crates/tetra-entities/tests/test_mle_bs.rs`: added regression test for fresh single-transmission network-time broadcast.
- `example_config/config.toml`: corrected the timezone comment from once per hyperframe to twice per hyperframe, approximately every 30.6 seconds.

Verification:

- `cargo test -p tetra-entities --test test_mle_bs network_time --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib network_time --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_mle_bs --locked` -> 28 passed.
- `cargo check -p tetra-entities --locked` -> passed.
- `git diff --check -- crates/tetra-entities/src/mle/components/broadcast.rs crates/tetra-entities/tests/test_mle_bs.rs example_config/config.toml` -> passed.

Deploy status:

- Not deployed yet. The running RF test gate still uses the already deployed P2P close fix from `12:17:28 EEST`.
- Deploy this time-broadcast freshness patch only after the current P2P close RF test/log read is complete, or after explicitly opening a new RF gate.

## 2026-06-06 12:22 EEST - P2P established-call close normalized to D-DISCONNECT peer handshake

Component in simple technical terms:

- CMCE private-call control decides what each terminal receives when a simplex private call is closed.
- The terminal that presses red sends `U-DISCONNECT` and expects `D-RELEASE`.
- The other terminal is still a peer in an already established call, so Nexus-BS clears it with `D-DISCONNECT` and waits for its `U-RELEASE` before closing the bearer.

Issue covered:

- Live private simplex close could leave the Motorola peer showing `Not Answered` even after a call with working voice.
- The old compatibility branch could clear the peer with `D-RELEASE`, which is ETSI-permitted but can look like a setup/no-answer result on real terminals.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.3.1: after `U-DISCONNECT`, the disconnecting MS waits for `D-RELEASE`; SwMI should inform the other MS of clearance by `D-DISCONNECT` or `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3: `D-DISCONNECT` requires the MS to respond with `U-RELEASE`; `D-RELEASE` requires no MS response.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- Active local simplex private `U-DISCONNECT` now always sends prompt assigned-channel `D-RELEASE` to the initiating MS and tail-drained assigned-channel `D-DISCONNECT` to the peer.
- Removed the dead peer `D-RELEASE` tail-drain branch so future patches cannot accidentally reintroduce the `Not Answered`-prone close route.
- Fallback remains separate: if `D-DISCONNECT` delivery is discarded or locally times out, CMCE may use peer `D-RELEASE` as a bounded recovery path rather than keeping a bearer pinned forever.

Verification so far:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 74 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 161 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 84 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 80 passed.
- `cargo check -p tetra-entities --locked` -> passed.
- `git diff --check` -> passed.

Deploy:

- Ran `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh` after local verification.
- Local ARM64 release build passed.
- Deployed binary SHA-256: `dbfae8a6f26b5c5bda55538ca2cf53463e35ded4f2c49cc0a214d4000580040b`.
- Service: `nexus-bs@chris.service`.
- Restart timestamp: `Sat 2026-06-06 12:17:28 EEST`.
- PID after restart: `47700`.
- Journal rotated/vacuumed for the RF gate at `2026-06-06T12:17:48+03:00`; `journalctl -u nexus-bs@chris.service -n 8` returned `-- No entries --`.

Next RF gate:

- Controlled RF scenario: `2260616 -> 2260618`, allow normal voice, let `2260618` speak last, then close from `2260616` with red key. Expected result: `2260618` should show normal end call, not `Not Answered` or reboot.
- After user confirms test completion, read journal since `2026-06-06 12:17:48 EEST`.

## 2026-06-06 12:10 EEST - Current P2P handoff and release-review priority

Current status:

- P2P has improved through current-channel setup, stale TMA cancellation, EG7 D-SETUP prioritization, and called `D-CONNECT ACKNOWLEDGE` recovery work.
- Latest user priority: private-call close/release still leaves the terminal/UI showing `Not Answered`; treat close semantics as the active blocker, not initial setup.
- Active review scope is CMCE private-call release under EN 300 392-2 clause 14.5.1.3.
- Practical scale target is 100 simultaneous terminals. Do not spend time on broad thousand-terminal refactors before the core RF call paths are stable.
- This remains clause-scoped engineering evidence only. Do not claim formal ETSI/TETRA certification.

Next RF gate:

- Before any next RF test, the assistant must ask the user explicitly and name the exact controlled P2P scenario and log window to use.
- Next investigation should inspect close/release logs, confirm which `U-` and `D-` release/clearing PDUs are observed, and verify the terminal-facing `Not Answered` result against clause 14.5.1.3.
- Protocol-source follow-up remains allowed under the standing law: identify the ETSI clause first, make a focused patch, test locally, deploy locally built binaries only, then gate RF with an explicit user prompt.

## 2026-06-06 10:39 EEST - EG energy-economy frame-18 starvation audit verified

Component in simple technical terms:

- MM negotiates Energy Economy mode with the terminal and chooses the frame/multiframe start point carried in MM signalling.
- UMAC consumes that start point and decides when a sleeping terminal is expected to listen for downlink signalling.
- LMAC/scheduler support is currently not a full all-slot frame-18 receive model, so MM/UMAC must not create EG receive cycles that require frame 18.

Audit result:

- No new protocol patch was required in this step: the current worktree already contains the frame-18 protection.
- `crates/tetra-entities/src/mm/mm_bs.rs` chooses an EG start point whose full recurring receive cycle does not include frame 18, and falls back to `StayAlive` if a safe start point cannot be allocated.
- `crates/tetra-entities/src/umac/umac_bs.rs` rejects TLMC Energy Economy start points whose recurring cycle would require unsupported frame-18 receive behaviour.
- `crates/tetra-entities/tests/test_mm_bs.rs` and `crates/tetra-entities/tests/test_umac_bs.rs` contain focused regression tests for this path.

ETSI clause scope:

- EN 300 392-2 clauses 16.7.1 and 16.10.10: Energy Economy mode/start point are negotiated and carried by MM.
- EN 300 392-2 clause 23.7.6 and table 23.9: the energy-economy start point plus EG sleep duration defines the recurring receive cycle.
- EN 300 392-2 clause 23.5.2.2.7: BS downlink scheduling should account for energy economy reception opportunities.
- Timer T.210: after signalling/activity, the MS remains awake before returning to the sleep cycle.
- This is clause-scoped engineering evidence, not formal ETSI/TETRA certification.

Verification:

- `cargo test -p tetra-entities --test test_mm_bs frame_18 --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_umac_bs frame_18 --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_mm_bs energy_saving --locked` -> 27 passed.

Runtime observation:

- Live service `nexus-bs@chris.service` remains active from restart `2026-06-06 10:36:50 EEST`.
- Fresh post-deploy log scan showed only energy-saving negotiation warnings, including T352 expiry for BS-initiated EG assignment; no new P2P RF test has appeared yet after the latest deploy.

## 2026-06-06 10:37 EEST - P2P current-channel setup deployed, clean RF log ready

Component in simple technical terms:

- CMCE is the private-call controller: it sends the called `D-CONNECT ACKNOWLEDGE`, waits for the called lower-layer ACK, then sends caller `D-CONNECT` and opens the first simplex floor.
- LLC is the reliability layer: its acknowledged basic link and `TxReporter` prove whether the called radio acknowledged the setup PDU.
- UMAC/LMAC are the radio scheduler/executor: they decide whether signalling is current-channel common signalling or assigned-channel STCH/FACCH and then put it on RF.

Deploy evidence:

- Local tests/build/deploy were run with `RUN_TESTS=1 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Deployed commit label: `332fa519`.
- Remote binary SHA256: `f629ed1db0736b994a7f8b12f580c1250b24ee91d5c1547b2a05b27cf3191436`.
- Remote path: `/home/chris/nexus-bs/nexus-bs`.
- Service: `nexus-bs@chris.service`.
- Restart timestamp: `2026-06-06 10:36:50 EEST`.
- Main PID after restart: `46624`.
- Post-deploy journal was rotated/vacuumed; `journalctl -u nexus-bs@chris.service -n 8` returned `-- No entries --`, so the next RF test can be read cleanly.

Verification gate before deploy:

- `cargo test -p tetra-entities --test test_cmce_bs repeated_group_u_setup --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 161 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 74 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 33 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.
- `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` -> pass.

ETSI clause scope:

- Same clause-scoped behavior as the `10:29 EEST` patch below: EN 300 392-2 clauses 14.5.1.1.1, 14.5.1.1.2, 14.5.3.1, 23.5.4.3.1, and 23.8.2.2 note 3.
- This is a deployed engineering compliance-hardening step, not formal ETSI/TETRA certification.

Next RF gate:

- Retest `2260082 -> 2260618` and inspect fresh logs for: called `D-CONNECT ACKNOWLEDGE`, called L2 ACK received, caller `D-CONNECT`, initial `FloorGranted`, and voice on first/repeated PTT.
- If the called ACK still fails, do not guess another CMCE sequence; inspect UMAC/LLC ACK subslot, reporter correlation, and EG7 receive-window coverage first.

## 2026-06-06 10:29 EEST - P2P called-leg D-CONNECT ACK current-channel setup patch

Component in simple technical terms:

- CMCE/CC individual call is the private-call controller. It sends `D-CONNECT ACKNOWLEDGE` to the called MS after `U-CONNECT`, waits for that lower-layer ACK, then sends `D-CONNECT` to the caller and only then seeds the initial simplex floor.
- LLC acknowledged basic link is the reliability layer. Its `TxReporter` is the local proof that the called MS acknowledged the setup PDU.
- UMAC/MAC is the radio scheduler. `stealing_permission=false` keeps this setup PDU on the normal current-channel signalling path; `stealing_permission=true` would put it on assigned-channel STCH/FACCH stealing.

Issue covered:

- Live RF P2P `2260082 -> 2260618` failed twice after restart `2026-06-06 09:54:18 EEST`.
- Both attempts sent called-leg `D-CONNECT ACKNOWLEDGE` to `2260618`, then LLC exhausted retransmissions without receiving the called L2 ACK.
- The current code had encoded the first called-leg `D-CONNECT ACKNOWLEDGE` as `stealing_permission=true`, making the authoritative setup PDU assigned-channel STCH/FACCH even before the BS had proof that the called MS had moved correctly.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.1.1 requires `D-CONNECT ACKNOWLEDGE` after the called MS `U-CONNECT` and requires it to indicate which party may transmit.
- EN 300 392-2 clause 14.5.3.1 allows late traffic-channel assignment in `D-CONNECT` and `D-CONNECT ACKNOWLEDGE`; for late assignment the caller remains on the control channel until instructed to move.
- EN 300 392-2 clause 23.5.4.3.1 says that for acknowledged service with channel allocation that the MS may reject, the BS should grant the L2 ACK subslot on the current channel, because a grant only on the allocated channel is unusable if the MS does not move.
- EN 300 392-2 clause 23.8.2.2 note 3 covers the ACK-missing ambiguity: if the BS does not receive the expected ACK for a message giving receive authorization, it cannot know whether the MS is in signalling or traffic mode, so recovery must be ambiguity-safe.
- Annex D.4 remains informative ordering evidence only: wait for called-leg ACK before caller authorization. It is not a mandate that the first called `D-CONNECT ACKNOWLEDGE` be STCH-only.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Kept the called-first gate: caller `D-CONNECT` and initial `FloorGranted` still wait for called-leg L2 ACK.
  - Changed the first called `D-CONNECT ACKNOWLEDGE` to `stealing_permission=false`.
  - Kept `Layer2Service::Acknowledged`, `chan_alloc=Some(Both)`, target called ISSI, and `TxReporter`.
  - Updated comments to cite clauses 14.5.1.1.1, 14.5.1.1.2, 14.5.3.1, 23.5.4.3.1, and 23.8.2.2.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated P2P setup assertions so the first called `D-CONNECT ACKNOWLEDGE` must be acknowledged current-channel signalling with channel allocation, not STCH-only stealing.
  - Verified test-first: this test failed before the production patch at `test_cmce_bs.rs:9086`.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Renamed/reworded the STCH priority unit test as explicit channel-allocation STCH recovery, not first-path P2P setup evidence.
  - Kept STCH recovery priority intact for ambiguity-safe recovery paths.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_u_connect_waits_for_called_l2_ack_before_caller_d_connect --locked`
  - Before patch: failed on the new `!stealing_permission` assertion.
  - After patch: passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 74 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_stch_bl_ack_before_private_floor_granted_uses_called_primary --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs private --locked` -> 18 passed.
- `cargo test -p tetra-entities --lib test_explicit_channel_allocation_stch_recovery_preempts_ack_only_facch --locked` -> 1 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `rustfmt --edition 2024 --check crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs crates/tetra-entities/src/umac/subcomp/bs_sched.rs crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `git diff --check` -> pass.

Limits and next execution:

- This is a clause-scoped engineering patch, not formal ETSI/TETRA certification.
- It has not yet been deployed or RF-retested.
- Next live gate: deploy locally built binary only, then retest `2260082 -> 2260618` and confirm log sequence:
  - `D-CONNECT ACKNOWLEDGE` to `2260618`;
  - called L2 ACK received;
  - caller `D-CONNECT`;
  - initial `FloorGranted`;
  - first and repeated PTT carry voice.
- If called ACK still fails, inspect UMAC/LLC reporter-order, endpoint correlation, and EG7 sleep coverage before any further CMCE sequencing patch.

## 2026-06-06 10:23 EEST - P2P private simplex ETSI re-audit, no speculative patch

Component in simple technical terms:

- CMCE/CC individual call is the private-call controller. It decides when the called MS receives `D-CONNECT ACKNOWLEDGE`, when the caller receives `D-CONNECT`, who gets the first simplex transmit permission, and how release is signalled.
- LLC acknowledged basic link is the layer-2 reliability mechanism. For this bug, its `TxReporter` is the local proof that the called MS acknowledged the downlink `D-CONNECT ACKNOWLEDGE`.
- UMAC/MAC owns the radio path: MCCH/SCH-F common signalling, assigned-channel FACCH/STCH stealing, EG listen windows, and TCH/S speech routing.
- `FloorGranted` is Nexus-BS internal CMCE -> UMAC control. It must not be emitted before call-control setup has enough ETSI/lower-layer evidence.

Live RF evidence:

- Latest restart log from `nexus-bs@chris.service` was read from `2026-06-06 09:54:18 EEST`.
- Test `2260082 -> 2260618` failed twice at `09:59:39` and `09:59:47`.
- Both attempts reached `U-SETUP` and opened the private same-timeslot bearer on `ts=2`.
- CMCE sent `D-CONNECT ACKNOWLEDGE` to called ISSI `2260618` with `TransmissionGrant::GrantedToOtherUser`.
- LLC retransmitted the acknowledged transfer to `2260618` until exhaustion.
- CMCE never logged `called D-CONNECT ACK L2 ACK received`; it released setup with `AcknowledgedServiceNotComplete`.
- Therefore the current blocking failure is before caller `D-CONNECT` and before first voice/floor; this specific failure is a called-leg ACK delivery/correlation problem, not yet a media/floor bug.

ETSI clause scope reloaded:

- EN 300 392-2 clause 14.5.1.1.1: for direct incoming individual call setup, called MS sends `U-CONNECT`, then SwMI returns `D-CONNECT ACKNOWLEDGE`; the PDU indicates which party may transmit.
- EN 300 392-2 clause 14.5.1.1.2: caller receives `D-CONNECT`; it carries call identifier, service/floor information, and any channel allocation.
- EN 300 392-2 clause 14.5.1.2.1: in simplex, SwMI fully controls transmit permission; an MS may not begin U-plane transmit without permission.
- EN 300 392-2 clause 14.5.1.4.1: `D-CONNECT` and `D-CONNECT ACKNOWLEDGE` with `transmission granted` or `transmission granted to another user` switch U-plane on for transmit or receive respectively.
- EN 300 392-2 clause 14.5.3.1: late assignment individual call may assign traffic channel only after `U-CONNECT`; caller remains on the control channel until told to move.
- EN 300 392-2 clause 23.5.4.3.1: for acknowledged service, when a channel allocation may be rejected, the BS should grant the L2 ACK subslot on the current channel; granting only on the allocated channel can lose the ACK if the MS does not move.
- EN 300 392-2 clause 23.8.2.2 note 3: if the BS does not receive expected L2 ACK for a message giving/withdrawing receive authorization, it cannot know whether the MS is in signalling mode or traffic mode; retransmission must be interpretable in that ambiguity.
- EN 300 392-2 clause 23.8.4.1.1: uplink C-plane STCH uses `MAC-DATA`/`MAC-END`; `MAC-U-SIGNAL` is U-plane signalling and must not be used as the only C-plane ACK model.
- EN 300 392-2 Annex D is informative. D.4 is a useful example for waiting on called-leg ACK before caller authorization, but it is not a normative mandate for an STCH-only first `D-CONNECT ACKNOWLEDGE` implementation.

Swarm conclusion:

- All agents were closed after read-only audit.
- The previous "conservative route" was incomplete: the ordering `called D-CONNECT ACKNOWLEDGE -> called L2 ACK -> caller D-CONNECT` remains defensible, but the current transport assumption `stealing_permission=true` / assigned-channel STCH for the first called ACK is not confirmed by ETSI as mandatory and is contradicted by the RF failure.
- Existing tests that assert `stealing_permission=true` for the first called `D-CONNECT ACKNOWLEDGE` are now suspect because they encode an implementation assumption, not clause-scoped proof.
- No protocol patch was made in this entry.

Concrete test-first fix plan:

1. Add a failing CMCE/UMAC/LLC integration test for called-leg `D-CONNECT ACKNOWLEDGE` delivery where the first authoritative PDU is sent as late-assignment common signalling with channel allocation and an acknowledged-service reporter, not as STCH-only traffic stealing. Expected gate: no caller `D-CONNECT` and no `FloorGranted` until the called reporter is acknowledged.
2. Add a full-path ACK-correlation test for same-timeslot private simplex: called ISSI primary before floor, BL-ACK from called on assigned channel, reporter becomes acknowledged, then caller `D-CONNECT` and initial `FloorGranted` are emitted exactly once.
3. Add an EG7 called-leg test: if a terminal is really in EG7, opening an assigned private bearer must suspend/cover sleep correctly so the setup ACK path is not starved by the 2-second CMCE guard. If `T352 expired ... keeping StayAlive`, the RF precondition is not EG7.
4. Add reporter-order regression: ACK arriving before MAC complete must not falsely confirm, but a valid ACK after transmitted state must confirm the pending called-leg reporter.
5. Only after a failing test identifies the layer, patch narrowly:
   - likely CMCE change: first called `D-CONNECT ACKNOWLEDGE` uses current-channel/common signalling path with channel allocation; no initial STCH-only assumption;
   - possible UMAC/LLC change: fix ACK endpoint/address correlation if the new full-path test proves misattribution;
   - possible EG change: keep both private participants awake/scheduled during setup if the EG7 test proves starvation.
6. After setup succeeds locally, re-run live RF matrix: `2260082 -> 2260618`, `2260618 -> 2260082`, `2260616 -> 2260618`, with first PTT voice, repeated same-speaker PTT, opposite-speaker PTT, and release by both sides.

Guardrails:

- Do not implement D.5 fast-path as default. It can lose first traffic if the called MS misses `D-CONNECT ACKNOWLEDGE`.
- Do not claim formal certification. This remains clause-scoped engineering evidence until official conformance evidence exists.
- Do not deploy another P2P protocol build until at least the called-leg ACK test and relevant targeted tests pass locally.

## 2026-06-06 03:25 EEST - Flat-folder WAP control wrapper deployed and live-smoked

Component in simple technical terms:

- `nexus-bs-control` is the user-facing CLI wrapper that writes a complete operator command line into the volatile systemd FIFO.
- `nexus-bs-control-service` is the long-running local websocket bridge that parses that line and forwards a `SendRawSds` command to the running BS.
- CMCE/SDS is the TETRA short-data layer that accepts the raw SDS Type4 payload and emits the WAP MVP as `D-SDS-DATA`.

Issue covered:

- The final flat install folder `/home/chris/nexus-bs` had `nexus-bs`, `nexus-bs-control-service`, and `config.toml`, but no `nexus-bs-control` wrapper.
- The example config already documented `nexus-bs-control sendwap ...`, so a normal operator could not run the documented command from the final folder.

Patch/deploy:

- `scripts/nexus-bs-control`
  - Added the canonical wrapper.
  - It derives the service user from `NEXUS_BS_USER`, then `$USER`, then `id -un`.
  - It writes to `/run/nexus-bs-USER/control.commands` by default.
  - It supports overrides: `NEXUS_BS_RUN_DIR` and `NEXUS_BS_CONTROL_FIFO`.
  - With arguments, it writes one newline-terminated command; without arguments, it forwards stdin.
- `scripts/nexus-bs-control-command.sh`
  - Kept as a compatibility alias that execs the canonical wrapper.
- `contrib/systemd/nexus-bs-control@.service`
  - Comment now points operators to the flat `/home/USER/nexus-bs/nexus-bs-control` wrapper.
- `README.md` and `example_config/config.toml`
  - Document the flat folder layout and WAP MVP command from `~/nexus-bs`.
- Deployed only the wrapper to `/home/chris/nexus-bs/nexus-bs-control`; no Rust build was run on the Pi and no binary backup was created.
- Remote wrapper SHA-256: `a4cbc12775c5324bedb69594ea2933f7e53185a9e4fa0284b8eedc213528987b`.

ETSI clause scope:

- EN 300 392-2 clause 13.2 covers SDS services.
- EN 300 392-2 clauses 13.3.3 and 14.8.52 define SDS Type4 user-defined data and the Type4 length/PID boundary.
- EN 300 392-2 clause 29.4.1 and table 29.21 define WAP/WDP direct over SDS Type4 with PID `0x04`.
- This patch is operator/runtime plumbing for an already clause-scoped SDS/WAP path; it is not a new TETRA PDU implementation and is not formal ETSI/TETRA certification.

Verification:

- Local shell checks:
  - `sh -n scripts/nexus-bs-control` -> pass.
  - `sh -n scripts/nexus-bs-control-command.sh` -> pass.
  - Temp FIFO argument write produced `sendwap 16777215 2260618 false`.
  - Temp FIFO stdin write produced `sendwapcolor 16777215 2260618 false`.
- Local focused tests:
  - `cargo test -p nexus-bs-control sendwap --locked` -> 7 passed.
  - `cargo test -p tetra-entities --test test_sds_bs wap --locked` -> 12 passed.
  - `cargo test -p tetra-entities --lib net_dashboard::server::tests::dashboard_wap_ws_dispatches_raw_type4_wap_sds --locked` -> 1 passed.
  - `git diff --check` -> pass.
- Remote runtime:
  - Before deploy, `/home/chris/nexus-bs` lacked `nexus-bs-control`.
  - After deploy, `/home/chris/nexus-bs/nexus-bs-control` is executable and hash-matches local wrapper.
  - `nexus-bs-control@chris.service` and `nexus-bs@chris.service` were active.
  - Remote command executed: `/home/chris/nexus-bs/nexus-bs-control sendwap 16777215 2260618 false`.
  - `nexus-bs-control@chris.service` logged `command dispatched to 1 client(s)`.
  - `nexus-bs-control@chris.service` logged `>> SendRawSds { handle: 2, source_ssi: 16777215, dest_ssi: 2260618, dest_is_group: false, sdti: 3, len_bits: 1984, payload[0]=4, ... }`.
  - `nexus-bs@chris.service` logged `SDS: received raw from Control 2: 16777215 -> 2260618, type=ISSI, sdti=3, 1984 bits`.
  - `nexus-bs-control@chris.service` logged `<< SendRawSdsResponse { handle: 2, success: true }`.

Limit:

- This proves the documented flat-folder operator command reaches the running BS and CMCE/SDS accepts the WAP Type4 payload.
- It does not prove the terminal WAP browser rendered the page; that still requires operator observation on `2260618` or another target terminal.

Next non-repeating execution:

1. Ask operator to open the WAP browser on `2260618` and confirm whether the WML page is displayed/flashing.
2. Continue live RF private simplex and group-call PTT validation on the current deployed BS.
3. If terminal WAP rendering fails, inspect terminal expectations for direct WAP PID `0x04` versus WAP SDS-TL PID `0x84`, staying inside EN 300 392-2 table 29.21 and not advertising SNDCP/IP service.

## 2026-06-06 03:15 EEST - WAP/SDS control FIFO bridge fixed and live WAP smoke accepted by CMCE

Component in simple technical terms:

- `nexus-bs-control@USER.service` is the local command bridge. A normal operator writes a line like `sendwap ...` into `/run/nexus-bs-USER/control.commands`.
- `nexus-bs-control-service` parses that line and forwards a structured `SendRawSds` command over the local websocket to the running BS.
- CMCE/SDS is the TETRA short-data component. It turns the raw WAP Type4 bytes into `D-SDS-DATA` for the target ISSI/GSSI.
- The WAP MVP is not full SNDCP/IP WAP; it is WAP/WDP PID `0x04` over SDS Type4, carrying the compact Nexus-BS WML greeting page.

Issue covered:

- A live WAP send attempt through `/run/nexus-bs-chris/control.commands` initially produced no `SendRawSds` dispatch in the control-service journal.
- The service template used `tail -n 0 -F` on a FIFO. For simple operator writes this is a fragile bridge and can make WAP/SDS appear submitted while no command reaches the control service.
- A bad remote test write without a real newline also produced a malformed concatenated command (`falsensendwap`), which confirmed the command path must receive complete newline-terminated operator lines.

Patch:

- `contrib/systemd/nexus-bs-control@.service`
  - Replaced the FIFO reader with `while true; do /bin/cat /run/nexus-bs-%i/control.commands; done | ...`.
  - This keeps the service stdin open across short-lived FIFO writers and supports repeated simple operator commands without restarting the control websocket service.

ETSI clause scope:

- EN 300 392-2 clause 13.2 covers SDS services.
- EN 300 392-2 clauses 13.3.3 and 14.8.52 define SDS Type4 user-defined data and its length/PID structure.
- EN 300 392-2 clause 29.4.1 and table 29.21 define SDS-TL/WAP protocol identifiers; the WAP MVP uses direct WAP/WDP PID `0x04`.
- EN 300 392-2 clause 29.3.3.8.2 is relevant for SDS-TL broadcast behaviour; this live smoke was an individual ISSI WAP SDS, not a broadcast.
- The systemd FIFO patch is runtime plumbing, not a TETRA PDU change.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Verification:

- Deployed the updated `nexus-bs-control@.service` to `/etc/systemd/system/nexus-bs-control@.service`, ran `systemctl daemon-reload`, and restarted `nexus-bs-control@chris.service`.
- `nexus-bs@chris.service` stayed active and reconnected to the control websocket at `2026-06-06 03:12:41 EEST`.
- Live WAP command sent after reconnect:
  - `sendwap 16777215 2260618 false`
  - control-service logged `command dispatched to 1 client(s)`.
  - control-service logged `>> SendRawSds { handle: 1, source_ssi: 16777215, dest_ssi: 2260618, dest_is_group: false, sdti: 3, len_bits: 1984, payload[0]=4, ... }`.
  - BS logged `SDS: received raw from Control 1: 16777215 -> 2260618, type=ISSI, sdti=3, 1984 bits`.
  - control-service logged `<< SendRawSdsResponse { handle: 1, success: true }`.
- Local focused tests:
  - `cargo test -p nexus-bs-control sendwap --locked` -> 7 passed.
  - `cargo test -p tetra-entities --test test_sds_bs wap --locked` -> 12 passed.
  - `cargo test -p tetra-entities --lib net_dashboard::server::tests::dashboard_wap_ws_dispatches_raw_type4_wap_sds --locked` -> 1 passed.
  - `cargo test -p tetra-entities --lib net_dashboard::server::tests::live_sds_post_rejects_wap_protocol_id_0x84 --locked` -> 1 passed.
  - `git diff --check` -> pass.

Limit:

- This proves local operator command transport and CMCE/SDS acceptance of the WAP MVP payload.
- It does not prove the terminal WAP browser displayed the page; that still requires the operator to open/observe the page on the target terminal.

Next non-repeating execution:

1. Ask operator to check WAP browser on `2260618` for the Nexus-BS greeting page after the `03:14:04` send, or trigger another send while the browser is open.
2. Continue live RF validation for private simplex/group PTT after the `03:07:21` Annex D.4 deploy.
3. Continue LLC acknowledged-path audit only where current tests do not already cover the claimed behaviour.

## 2026-06-06 03:02 EEST - Annex D.4 Motorola-like private setup directive rechecked

Component in simple technical terms:

- CMCE/CC individual call is the private-call controller: it prepares the called MS, waits for lower-layer delivery proof, then authorizes the caller.
- `D-CONNECT ACKNOWLEDGE` is sent to the called MS with channel allocation.
- The L2 ACK through `TxReporter` is the local proof that the called MS received that command.
- `D-CONNECT` to the caller and the initial simplex `FloorGranted` remain blocked until that called-leg ACK arrives.

User directive checked:

- Keep the conservative Annex D.4 order for Motorola-like terminals: called `D-CONNECT ACKNOWLEDGE`, wait for L2 ACK, then caller `D-CONNECT`.
- No protocol code was changed in this entry because the current CMCE path already implements this order in `pending_individual_connect_acks`.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 define the private individual-call setup/through-connect signalling.
- EN 300 392-2 clause 14.5.1.2.1 keeps private-simplex transmit permission under SwMI control.
- EN 300 392-2 Annex D.4 describes the conservative direct individual-call setup sequence: called `D-CONNECT ACKNOWLEDGE`, L2 ACK, then caller `D-CONNECT`.
- EN 300 392-2 Annex D.5 is the faster alternative and warns that first traffic can be missed if the called MS misses `D-CONNECT ACKNOWLEDGE`.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 72 passed.
- Parallel QA agent found no direct local P2P path where caller `D-CONNECT` or initial UMAC `FloorGranted` is emitted before called `D-CONNECT ACKNOWLEDGE` L2 ACK.
- QA explicitly classified Brew-routed P2P and ISSI 999 echo as non-Annex-D.4 exceptions because they are external/synthetic services, not direct setup toward a local called MS.
- Integrated local verification before deploy:
  - `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 33 passed.
  - `cargo test -p tetra-entities --test test_cmce_bs group --locked` -> 69 passed.
  - `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 72 passed.
  - `cargo test -p tetra-entities --test test_umac_bs --locked` -> 71 passed.
  - `cargo test -p tetra-entities --test test_lmac_bs tch_s --locked` -> 10 passed.
  - `cargo test -p tetra-entities --test test_sds_bs status --locked` -> 50 passed.
  - `cargo check -p tetra-entities --locked` -> pass.
  - `git diff --check` -> pass.

Deploy/runtime:

- Built locally only with the AArch64 SoapySDR sysroot; no compile was run on `chris@192.168.1.179`.
- Command shape: `cargo zigbuild --release -p nexus-bs -p nexus-bs-control --target aarch64-unknown-linux-gnu --locked`.
- Deployed direct to flat `/home/chris/nexus-bs`, no binary backup, preserving existing `/home/chris/nexus-bs/config.toml`.
- Local/remote SHA-256:
  - `nexus-bs`: `481a3ae45f30a3b150aa9ee46e0f27b23cdfaa1304469a425229b04faa4aa259`.
  - `nexus-bs-control-service`: `0c28160ac5928090ab5ee9fdf2eaae6f54e6776484043434e6dc423bf6dc81a9`.
- Restarted `nexus-bs-control@chris.service` and `nexus-bs@chris.service`; both active from `2026-06-06 03:07:21 EEST`.
- Startup evidence:
  - Dashboard listening on `http://0.0.0.0:8080`.
  - Control receiver listening on `127.0.0.1:9002` and websocket connected.
  - Restart recovery armed for `{2260082, 2260616, 2260618}`.
  - `2260616`, `2260618`, and `2260082` registered and affiliated to `[226333]`.
  - Live config currently allocates EG7 to the three lab ISSIs.

Next non-repeating execution:

1. Live RF retest after `03:07:21`:
   - private simplex `2260616 -> 2260618`, first PTT must carry voice after Motorola-like Annex D.4 setup;
   - private simplex `2260082 -> 2260618`, repeated same-speaker PTT must carry voice;
   - group `226333`, alternating PTT from all affiliated ISSIs must not produce `PTT denied`, `Service unavailable`, or static-only audio.
2. Inspect only bounded journal slices after the test window and patch the first proven failing layer.

## 2026-06-06 00:23:36 EEST - Private simplex same-speaker tail-drain stale release suppression

User report context:

- Live RF `2260082 -> 2260618`, both Motorola: first PTT from `2260082` had voice, later PTTs from the same ISSI lost voice.
- After the UMAC hangtime/raw-media patch, agent CMCE audit found a second same-speaker race: a pending `U-TX CEASED` tail drain could later emit stale `D-TX CEASED` / `FloorReleased` after the same ISSI had already rekeyed and received a fresh `D-TX GRANTED`.

Component explanation:

- CMCE is the private-call floor-control layer. It translates `U-TX CEASED` and `U-TX DEMAND` into peer-facing `D-TX CEASED`/`D-TX GRANTED` and UMAC `FloorReleased`/`FloorGranted` controls.
- The private-simplex TX-CEASED tail drain is a short local guard that lets FACCH/TCH tail timing drain before declaring the floor idle.
- The bug was that a same-speaker rekey during that drain did not cancel the old drain, so the old drain could later switch U-plane off after a valid new grant.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1 b): `U-TX DEMAND` is answered by `D-TX GRANTED` when the SwMI grants the private-simplex floor.
- EN 300 392-2 clause 14.5.1.2.1 e): `U-TX CEASED` may be followed by `D-TX CEASED` at end of transmission, but that stale indication must not override a later valid grant.
- EN 300 392-2 clause 14.5.1.4.2: `D-TX GRANTED` and `D-TX CEASED` switch U-plane on/off; preserving ordering is required so a later grant is not undone by an older cease.
- EN 300 392-2 clauses 23.5 and 23.8.5 cover the assigned-channel FACCH/STCH/TCH timing that the local tail-drain guard protects.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added `cancel_matching_individual_tx_ceased_tail_drain(call_id, sender_issi)`.
  - It cancels only a pending TX-CEASED tail drain whose sender ISSI matches the newly granted requester.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Calls the cancellation helper on the positive private-simplex `U-TX DEMAND` grant path before setting the current floor holder.
  - Different-speaker queued handoff semantics remain unchanged.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_simplex_p2p_same_speaker_rekey_during_tx_ceased_tail_suppresses_stale_floor_release` using `2260082 -> 2260618`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_simplex_p2p_same_speaker_rekey_during_tx_ceased_tail_suppresses_stale_floor_release --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 70 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 7 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 10 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Rebuild ARM64 locally and deploy direct to `/home/chris/nexus-bs`; the previous `00:20:45` deploy had only the UMAC patch, not this CMCE tail-drain fix.
2. Restart `nexus-bs@chris.service` and `nexus-bs-control@chris.service`.
3. Live retest `2260082 -> 2260618` with both patterns:
   - Release, wait visible hangtime/idle, then rekey same `2260082`.
   - Release and rekey quickly from same `2260082` before any long wait.
4. Journal should show fresh `UMAC floor granted` and `UMAC voice route` for each rekey, with no stale `FloorReleased` / `D-TX CEASED` after the fresh grant.

## 2026-06-06 00:18:56 EEST - Private simplex Motorola same-speaker hangtime media guard

User report:

- Live RF retest `2260082 -> 2260618`, both Motorola.
- First PTT from `2260082` had voice.
- Later PTTs from the same `2260082` showed private-call/floor behaviour but no voice.

Component explanation:

- CMCE is the call-control layer: it accepts `U-TX DEMAND`, sends `D-TX GRANTED`, and enters hangtime after `U-TX CEASED`.
- UMAC is the MAC scheduler layer: it decides whether the assigned channel is in hangtime or traffic mode and routes valid TCH/S voice to the peer downlink.
- LMAC already recovers the raw TCH/S burst after a late `Cp` marker. The remaining bug was UMAC dropping raw TCH/S Block2 immediately if it arrived while the private call was still marked hangtime, before CMCE's `FloorGranted` control message had cleared hangtime.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1 b): a later private-simplex `U-TX DEMAND` after `U-TX CEASED` is a fresh transmit request and, when accepted, is answered with `D-TX GRANTED`.
- EN 300 392-2 clause 14.5.1.4.2: `D-TX GRANTED` switches U-plane transmit/receive state for the granted MS.
- EN 300 392-2 clauses 23.5 and 23.8.5: FACCH/STCH stealing must not destroy a valid non-stolen TCH/S half-slot on an assigned traffic channel.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Moved the hangtime media gate after media classification.
  - Added a narrow guard that defers only raw TCH/S Block2 for an already active private local circuit during hangtime.
  - Group-call and ordinary ACELP media are still dropped during hangtime.
  - If the expected `FloorGranted` does not arrive before flush, the deferred raw block is dropped by the existing hangtime checks.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_private_simplex_same_speaker_raw_block2_reentry_survives_hangtime` using field ISSIs `2260082` and `2260618`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_private_simplex_same_speaker_raw_block2_reentry_survives_hangtime --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 7 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_group_ul_raw_block2_is_dropped_during_hangtime --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 10 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 69 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Cross-build locally and deploy direct to `/home/chris/nexus-bs`; do not compile on Pi and do not create backup binaries.
2. Restart `nexus-bs@chris.service`.
3. Live retest `2260082 -> 2260618`: first PTT voice, release, second/third PTT from `2260082` must have voice.
4. Inspect journal for `UMAC voice route` after each re-entry and absence of `UL inactivity timeout` for the granted speaker.

## 2026-06-06 00:10:26 EEST - Private simplex same-speaker re-entry audio recovery

User report:

- Live RF test `2260082 -> 2260618`, both Motorola.
- First PTT from `2260082` had voice.
- Later PTTs from the same `2260082` showed normal private-call/floor UI but no voice before UL inactivity timeout.

Component explanation:

- CMCE is call control: it decides who has private simplex transmit permission and sends `D-TX GRANTED` / `D-TX CEASED`.
- UMAC owns the assigned channel state: hangtime, active circuit, current floor speaker, and downlink routing of voice.
- LMAC classifies raw radio bursts as control (`Cp`/SCH/STCH) or traffic (`Tp`/TCH/S). After hangtime, its per-timeslot uplink marker can lag the new floor grant by two timeslots.
- The bug was in LMAC classification tolerance: after hangtime, a valid TCH/S burst arriving while the cached marker still said `Cp` was first tried as control and then discarded instead of being retried as TCH/S.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: after `U-TX CEASED`, a later `U-TX DEMAND` from the same MS is a valid fresh request; if granted, SwMI sends `D-TX GRANTED` to the requester and informs the other MS.
- EN 300 392-2 clause 14.5.1.4.2: `D-TX GRANTED` switches U-plane transmission/reception on according to the grant state.
- EN 300 392-2 clauses 23.5.2.2.1, 23.8.4.1.4, 23.8.4.2.2, and 23.8.5: STCH/FACCH and TCH/S half-slot timing must be interpreted/preserved; a valid non-stolen TCH/S half-slot must not be lost only because local marker state is late.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/lmac/lmac_bs.rs`
  - Renamed and widened the NUB fallback from `Unallocated` only to `Unallocated | Cp`.
  - The fallback still runs only after control decoding fails CRC and only for TCH/S-compatible NUB shapes: `NormalTrainSeq1/Both` and `NormalTrainSeq2/Block2`.
  - UMAC remains the final guard: media is dropped unless a matching non-hangtime active circuit/floor exists.
- `crates/tetra-entities/tests/test_lmac_bs.rs`
  - Added `bs_lmac_recovers_fullslot_tch_s_after_hangtime_cp_marker_lag`.
  - Added `bs_lmac_recovers_seq2_block2_tch_s_after_hangtime_cp_marker_lag`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 10 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 6 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 69 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.
- Cross-built locally with `cargo zigbuild --release -p nexus-bs -p nexus-bs-control --target aarch64-unknown-linux-gnu --locked`.
- Deployed direct to `/home/chris/nexus-bs` on `chris@192.168.1.179`, no backup binaries.
- Remote hashes:
  - `nexus-bs`: `ff90d99a02fed8e57acafee7b248704a1b430246bd42b33206bf24e2b57ed0ea`.
  - `nexus-bs-control-service`: `e398d0a3bf5e686ffbbbb25fa2513f31ced9bfd780ac0f99359b7621fc8c623f`.
- Services active after restart at `2026-06-06 00:10:26 EEST`.
- Restart recovery observed:
  - `2260616` registered and affiliated `[226333]`.
  - `2260618` registered and affiliated `[226333]`.
  - `2260082` registered and affiliated `[226333]`.

Next non-repeating execution:

1. Live retest private simplex `2260082 -> 2260618`: first PTT voice, release, second and third PTT from same `2260082` must have voice before any UL inactivity timeout.
2. Inspect `journalctl -u nexus-bs@chris.service --since '2026-06-06 00:10:26'` for `LMAC: retrying undecoded NUB as candidate TCH/S on non-traffic UL marker Cp`, `rx_blk_traffic`, `UMAC voice route`, and absence of `UL inactivity timeout`.
3. Repeat with `2260616 -> 2260618` to cover Hytera-to-Motorola first-PTT and re-entry behavior.

## 2026-06-05 22:33:00 EEST - Final Pi service layout rework to systemd volatile runtime

User report:

- Rework deployment to be simpler with systemd service management.
- Logs must be volatile/circular because the Pi filesystem will be read-only/clean-boot.
- The setup should work for normal users such as `chris`, `pi`, or `dennis`.
- Remove/disable unnecessary Pi OS services such as Bluetooth, polkit, ModemManager, and swap where present.

Component explanation:

- Runtime orchestration is the operating-system layer that starts/stops Nexus-BS and keeps state in the right place. It is not TETRA protocol behavior.
- `nexus-bs@USER.service` runs the TETRA base-station stack as that user. The editable `config.toml` lives in the same `~/nexus-bs` folder as the binaries, then the service copies it into `/run/nexus-bs-USER` before start so runtime cache files stay volatile.
- `nexus-bs-control@USER.service` runs the local command bridge. Operator commands are written to a FIFO in `/run/nexus-bs-USER/control.commands`.
- `journald` is the system log manager. With `Storage=volatile` and `RuntimeMax*` limits, logs stay in RAM and rotate by size.

ETSI clause scope:

- No ETSI EN 300 392-2 protocol field or PDU behavior was changed in this service-layout patch.
- This supports long-running validation of group call, private call, SDS/WAP, and attach behavior but is not formal ETSI/TETRA certification.

Patch:

- Added `contrib/systemd/nexus-bs@.service`.
- Added `contrib/systemd/nexus-bs-control@.service`.
- Added `contrib/systemd/journald-nexus-bs-volatile.conf`.
- Added `scripts/nexus-bs-control-command.sh`.
- Marked `contrib/systemd/nexus-bs.service` as the legacy sample and pointed new deployments to the template units.
- `tetra-core::debug` now honours `RUST_LOG` before falling back to the development default filter.
- `MessageRouter::tick_start` moved the per-TDMA tick line from `info` to `trace`.

Deployment verification on `chris@192.168.1.179`:

- Final home layout is flat: `/home/chris/nexus-bs/nexus-bs`, `/home/chris/nexus-bs/nexus-bs-control-service`, `/home/chris/nexus-bs/config.toml`.
- `nexus-bs@chris.service` and `nexus-bs-control@chris.service` are enabled and active.
- Runtime files are volatile under `/run/nexus-bs-chris`: copied `config.toml`, `config.toml.subscribers`, and FIFO `control.commands`.
- `RuntimeDirectoryPreserve=yes` keeps the shared FIFO/cache directory across BS/control restarts.
- `journald` is configured volatile with `RuntimeMaxUse=64M`, `RuntimeMaxFileSize=8M`, `RuntimeMaxFiles=8`, `MaxRetentionSec=1day`.
- `/var/log/journal` is absent; `/run/log/journal` is active.
- Bluetooth, Avahi, polkit, and `dev-zram0.swap` are inactive/masked; `/proc/swaps` is empty.
- Recent journal since the final restart has no `--- tick dl` or `rx_tpsap_prim got` flood.
- No formal ETSI/TETRA certification is claimed.

Next non-repeating execution:

1. Run live RF validation for group call turn-taking, private simplex/duplex close, SDS, and WAP delivery using the deployed systemd service.
2. If any PTT denied/static/no-answer issue appears, inspect `journalctl -u nexus-bs@chris.service --since <test-start>` and patch the relevant CMCE/UMAC/MM clause path with focused tests.

## 2026-06-05 22:39:00 EEST - Live config HMD PID 220 display text

User report:

- Configure PID 220 so the terminal display shows `Nexus-BS`.

Component explanation:

- Home Mode Display is a periodic SDS Type4 broadcast generated by CMCE/SDS.
- `protocol_id = 220` is SDS-TL PID `0xDC`, which is in the user-defined/vendor range; terminals that implement this display path may show the configured text.
- This is config-only behavior, not a TETRA protocol code patch.

ETSI clause scope:

- EN 300 392-2 SDS-TL PID handling allows user-defined/vendor PIDs in this range; this config uses that path for a short text payload.
- No formal ETSI/TETRA certification is claimed.

Deployment verification on `chris@192.168.1.179`:

- Added `[cell_info.home_mode_display]` to `/home/chris/nexus-bs/config.toml`.
- Runtime copy `/run/nexus-bs-chris/config.toml` contains `protocol_id = 220`, `text_coding_scheme = "LATIN"`, `interval_multiframes = 96`, `text = "Nexus-BS"`.
- Restarted `nexus-bs@chris.service`; service is active.
- Journal confirms: `SDS: Home Mode Display broadcast ... protocol_id=220 ... text_coding_scheme=0x01`.

## 2026-06-05 17:11:23 EEST - CMCE shared registry fallback for restart-affiliated groups

User report:

- Group affiliations must survive BS restart/resync and remain usable for many terminals.
- Avoid the "No Group"/"no listeners" failure mode when the central MM registry knows the GSSI but CMCE's local mirror is not yet rebuilt.

Component explanation:

- MM is Mobility Management. It owns the shared subscriber registry: registered ISSIs and their GSSI affiliations.
- CMCE is Call Control. It keeps a local mirror of subscribers for fast call/floor decisions, but after restart recovery that mirror can lag the shared registry.
- A GSSI listener check asks "is anyone affiliated to this group?". A floor-affiliation check asks "is this ISSI allowed to request PTT on this GSSI?".

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1: group call setup is addressed to a GSSI with locally affiliated listeners.
- EN 300 392-2 clause 14.5.2.2.1: SwMI floor control grants/queues/rejects PTT requests from affiliated group members.
- EN 300 392-2 clause 16.4.4: SwMI may initiate registration recovery.
- EN 300 392-2 clause 16.8.1: group attach/detach state must remain coherent.
- This is restart/resync robustness hardening, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - `has_listener(gssi)` now checks the shared `SubscriberRegistry` first and falls back to CMCE's local listener count.
  - `subscriber_affiliated_to_group(issi, gssi)` now checks the shared `SubscriberRegistry` first and falls back to CMCE's local `subscriber_groups`.
  - `handle_subscriber_update` syncs MM updates into the shared registry as a defensive reconciliation path.
  - Duplicate `Register` does not clear existing shared affiliations; this preserves current tolerant CMCE semantics.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_group_call_uses_shared_registry_when_cmce_listener_mirror_is_empty`.
  - Seeds only the shared registry, leaves CMCE's local mirror empty, starts a group call, and confirms a second shared-registry member gets `RequestQueued` rather than release/no-listener rejection.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_call_uses_shared_registry_when_cmce_listener_mirror_is_empty --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 135 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add large restart-recovery tests for thousands of cached ISSIs all affiliated to one GSSI.
2. Add UMAC scheduler queue depth caps/coalescing for non-critical downlink backlog while preserving call-control/FACCH priority.
3. Audit global `MessageQueue` boundedness/backpressure.

## 2026-06-05 17:06:39 EEST - Large-group UMAC/CMCE robustness for thousands of affiliates

User report:

- Make group operation robust for thousands of terminals on one GSSI, not just two or three radios.
- Continue clause-scoped ETSI EN 300 392-2 hardening and do not claim formal certification.

Component explanation:

- CMCE is call control. For a group call it keeps one current PTT floor owner and decides how the next requesting ISSI receives `D-TX GRANTED`, `RequestQueued`, or `NotGranted`.
- UMAC is the MAC scheduler. It turns CMCE decisions into MAC resources, FACCH/STCH signalling, random-access ACKs, and assigned-channel state on the radio timeslots.
- RA ACK is the MAC acknowledgement that a terminal's random access was heard. When hangtime cleanup has to preserve an ACK for the next STCH, that preserved state must stay bounded under mass access.
- Energy Economy/EG7 lets terminals sleep between receive windows. While a GSSI assigned channel is active, each affiliated EG member must be suspended exactly once and resumed with a T.210 awake guard after close.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1: normal group calls are listener-signalled by the GSSI while floor responses can address the requesting ISSI.
- EN 300 392-2 clause 14.5.2.2.1: SwMI-controlled group floor procedure uses `D-TX GRANTED` states for granted, queued, or not-granted PTT requests.
- EN 300 392-2 clause 21.4.3.1: MAC random-access acknowledgement is carried by the random-access flag.
- EN 300 392-2 clause 23.5.1.3.3: random access acknowledgement and reserved access grant must remain coherent.
- EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6/T.210: BS downlink scheduling and assigned-channel operation must account for Energy Economy receive windows.
- This is clause-scoped engineering hardening and test evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added `MAX_PENDING_RA_ACKS_PER_TIMESLOT = 8192`.
  - Deduplicated deferred random-access ACKs by full `TetraAddress`.
  - Bounded deferred RA ACK retention during hangtime cleanup so repeated access from thousands of affiliates cannot grow scheduler memory without limit.
  - Reworked `dl_drop_all_except_stolen` from repeated middle `Vec::remove` to a linear drain/rebuild pass, preserving STCH/FACCH stealing items while discarding/reporting other queued signalling as before.
  - Switched dropped grant lookup to `HashSet<TetraAddress>` so ACK/grant coherence checks stay linear under mass access.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_large_eg7_group_call_open_suspends_all_members_once_and_resumes_after_close` with 4096 EG7 affiliates on one GSSI plus an unrelated ISSI.
  - Asserts each affiliate gets one assigned-channel EG suspension on group call open and resumes with T.210 awake guard on close.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Strengthened `test_large_group_floor_handoff_uses_one_gssi_listener_grant` from two-speaker ping-pong to 32 distinct speakers within a 2048-member GSSI.
  - Asserts each handoff emits one individual requester grant, one GSSI listener grant, one UMAC `FloorGranted`, and no release/close.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 58 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_large_eg7_group_call_open_suspends_all_members_once_and_resumes_after_close --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_floor_handoff_uses_one_gssi_listener_grant --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 58 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 134 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add large restart-recovery tests for thousands of cached ISSIs affiliated to the same GSSI, covering MM recovery cache and CMCE listener restoration.
2. Add a many-queued-GSSI-resources test that asserts queue length remains resource-count bounded across mixed StayAlive/EG7 batches.
3. Continue SDS/status and LLC field-level hardening after group-call scale tests remain green.

## 2026-06-05 16:26:47 EEST - CMCE bounded group floor queue stress coverage

User report:

- Continue toward robust TETRA group call behavior for thousands of terminals, not just a few radios.
- Keep changes clause-scoped to ETSI EN 300 392-2 and do not claim formal certification.

Component explanation:

- CMCE is call control. For group calls it decides which affiliated ISSI currently owns the PTT floor.
- A queued floor requester is an MS that pressed PTT while another MS is still speaking. This stack intentionally keeps one waiter for direct handoff; additional contenders receive an explicit busy/not-granted response instead of being stored in an unbounded queue.
- UMAC should only be notified when the actual floor owner changes. Busy contenders must not receive `FloorGranted` and must not replace the queued requester.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: SwMI group floor control uses `D-TX GRANTED` with `Granted`, `RequestQueued`, or `NotGranted` state for request-to-transmit handling.
- EN 300 392-2 clause 14.5.2.1: group call remains GSSI-scoped for listener signalling while individual floor responses go back to the requesting ISSI.
- This is an engineering stress/regression test for the local one-waiter policy, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_large_group_floor_queue_is_bounded_and_busy_requesters_are_not_granted`.
  - Fixture registers 2048 affiliated ISSIs on one GSSI.
  - First non-speaker PTT receives `RequestQueued`.
  - The remaining 2046 affiliated contenders receive individual `NotGranted` responses with no UMAC `FloorGranted`, no release, and no call close.
  - When the current speaker sends `U-TX CEASED`, only the first queued requester receives the `Granted` handoff; busy contenders do not replace it.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_floor_queue_is_bounded_and_busy_requesters_are_not_granted --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 134 passed.
- `git diff --check` -> pass.
- `cargo check -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Continue UMAC large-GSSI hardening with a per-slot readiness cache only if stress/profiling shows repeated queued GSSI elements causing `Q * N` scans.
2. Continue SDS/status and LLC remaining audit gaps with clause-scoped field-level tests.

## 2026-06-05 16:22:09 EEST - Large GSSI group-call and UMAC scheduling scalability hardening

User report:

- Group call must be robust for thousands of terminals on one GSSI, not only two or three radios.
- Continue clause-scoped ETSI EN 300 392-2 hardening without claiming formal certification.

Component explanation:

- `SubscriberRegistry` is the shared ISSI/GSSI affiliation table. It answers "which ISSIs are attached to this GSSI?" for MM, CMCE, UMAC, SDS, dashboard, and restart recovery.
- MM is Mobility Management. It restores registration/group affiliation after restart/roaming and must not scan a full group list just to check one ISSI.
- UMAC is the MAC scheduler. It turns group signalling into real downlink MAC-RESOURCE/FACCH blocks and must repeat GSSI downlinks by actual EG receive batch, not by one PDU per terminal.
- CMCE is call control. It arbitrates PTT floor changes and must keep group floor signalling GSSI-scoped for listeners even when thousands of terminals are affiliated.
- RA ACK is the MAC acknowledgement of random access; a grant tells an MS where it may continue. Under mass access these must integrate into MAC-RESOURCEs without quadratic queue churn.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.2.1 and 14.5.2.2.1: group call setup/floor control and group-addressed `D-TX GRANTED` listener notification.
- EN 300 392-2 clause 21.4.3.1: MAC random-access acknowledgement.
- EN 300 392-2 clause 23.5.2.2.2: slot-grant response handling.
- EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6/T.210: BS downlink scheduling must account for energy-economy receive windows.
- This is clause-scoped engineering hardening and test evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-config/src/bluestation/state.rs`
  - Added reverse GSSI membership index `group_members_by_gssi`.
  - Added `contains_group_member(gssi, issi)` and `group_member_issis(gssi)` so callers can avoid allocating/scanning a full member list for one membership check.
  - Preserved tolerant duplicate affiliation semantics while preventing unknown ISSI phantom registration.
- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Replaced runtime `group_members(...).contains(...)` checks with indexed `contains_group_member(...)` in restart recovery and attach/detach paths.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Changed GSSI delivery `covered` and `active_batch` tracking from `Vec` to `HashSet`, removing quadratic coverage checks for large groups.
  - Added no-allocation readiness iteration for GSSI listeners where only "any target listens now?" is needed.
  - Reworked ready grant/RA-ACK extraction to partition the queue in one pass instead of repeated middle `Vec::remove`.
  - Reworked grant/ACK integration to index MAC-RESOURCEs by address, so mass ACK/grant bursts collapse to one resource per ISSI without repeated linear searches.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Uses the no-allocation GSSI member iterator when suspending EG for active group circuits.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added a 2048-member GSSI floor-handoff regression with 16 back-and-forth PTT cycles: requester receives `RequestQueued`, handoff emits one ISSI `Granted` plus one GSSI `GrantedToOtherUser`, and no release/close occurs.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs` tests
  - Added 2048-member StayAlive GSSI delivery test: one group resource, no per-member repeat.
  - Added 2048-member mixed StayAlive/EG7 GSSI delivery test: repeat by receive batch, not by member; sleeping EG7 members do not get T.210 from another batch.
  - Added 2048-ISSI mass RA ACK/grant integration test: one MAC-RESOURCE per ISSI with both ACK and grant integrated.

Verification:

- `cargo fmt --package tetra-config`
- `cargo fmt --package tetra-entities`
- `cargo test -p tetra-config --lib bluestation::state --locked` -> 18 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 53 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 133 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 56 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 133 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add a bounded test for CMCE one-deep floor queue under hundreds/thousands of simultaneous requesters: first waiter queued, later contenders explicitly `NotGranted`, no unbounded state growth.
2. Continue UMAC optimization with a per-slot GSSI readiness cache if profiling or stress tests show repeated `Q * N` scans with many queued GSSI elements.
3. Deploy only after a release build if live RF validation is requested; no formal certification claim without official conformance evidence.

## 2026-06-05 14:59:46 EEST - UMAC group speaker secondary tracking without P2P regression

User report:

- Patch must take P2P/private calls into account while continuing group-call floor hardening.
- Recent live group symptoms were static/no voice on first or returning PTT, so the group fix must not break the now-working private simplex/duplex path.

Component explanation:

- CMCE is call control. It creates group/private calls and tells UMAC when a circuit is open and which ISSI has the PTT floor.
- UMAC is the MAC scheduler. It maps the current floor holder to uplink/downlink TCH/S traffic on the assigned timeslot.
- P2P/private means ISSI-to-ISSI. The circuit primary active address is an ISSI, and `active_secondary_addrs` contains the peer ISSI for the same private bearer.
- Group means GSSI-scoped. The circuit primary active address is the GSSI. The current speaker ISSI may be tracked as secondary, but that must not make UMAC treat the group bearer as a private participant list.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: private/individual call floor control and participant-scoped transmit permission.
- EN 300 392-2 clauses 14.5.2.1 and 14.5.2.2.1: group call setup and SwMI floor grant with `D-TX GRANTED`.
- EN 300 392-2 clause 21.4.5: STCH `MAC-U-SIGNAL` has no SSI field, so UMAC must inherit the active speaker identity from CMCE floor state.
- EN 300 392-2 clause 23.5.2.2.7: BS assigns and marks applicable uplink/downlink traffic usage.
- This is clause-scoped engineering hardening only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-saps/src/control/call_control.rs`
  - Added `Circuit::is_primary_issi_scoped()` so callers can distinguish private/P2P circuits from group circuits even when a group circuit carries a secondary speaker ISSI.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added `ul_circuit_is_private_participant_scoped(ts)`.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - `FloorGranted` now applies the strict ISSI participant guard only when the UL circuit primary active address is ISSI. This preserves P2P non-participant rejection while allowing GSSI group handoff to any CMCE-authorized group speaker.
  - Energy-economy assigned-channel suspension now de-duplicates ISSI targets per circuit, so a group speaker already covered through the primary GSSI is not suspended a second time as a secondary speaker ISSI.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Local group setup now opens UMAC with primary GSSI plus initial speaker ISSI as secondary.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/network.rs`
  - Network-origin group setup now uses the same primary GSSI plus speaker ISSI secondary shape.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added a regression where group `active_addr=GSSI` and `active_secondary_addrs=[first_speaker ISSI]`, then `FloorGranted(second_speaker)` must be accepted and STCH attributed to the second speaker.
  - Added a regression proving a group secondary speaker ISSI does not double-count EG suspension when that ISSI is already an affiliated member of the primary GSSI.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Strengthened local group, numeric-collision group, network-origin group, and private simplex P2P circuit-shape assertions.

Verification:

- `cargo fmt --package tetra-saps --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_group_floor_grant_accepts_new_speaker_when_initial_speaker_is_secondary --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_group_secondary_speaker_does_not_double_suspend_energy_saving --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_stch_mac_u_signal_ignores_floor_granted_for_non_participant_private_speaker --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_setup_sends_proceeding_connect_and_group_setup_with_allocations --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group --locked` -> 52 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 65 passed.
- `cargo test -p tetra-entities --test test_umac_bs group --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_umac_bs private --locked` -> 10 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 132 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 53 passed.
- `cargo check -p tetra-saps -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deployment note:

- Committed and deployed direct to the Pi test instance after local release build packaging; no binary backup was created.
- Startup evidence after deploy showed the Nexus-BS process running and `2260616`, `2260082`, and `2260618` registered plus affiliated to GSSI `226333`.
- A post-restart error scan found no `PTT denied`, `Service unavailable`, `Unit Not Attached`, or `RequestedServiceNotAvailable` lines in the fresh test log.

Next non-repeating execution:

1. Operator live test: real GSSI `226333` alternating PTT across multiple radios, then private simplex between lab ISSIs.
2. If static persists, inspect whether the failing burst lacks `UMAC voice route` after `UMAC floor granted`; do not change P2P guard semantics unless a new clause-scoped reason is identified.

## 2026-06-05 14:08:45 EEST - UMAC rejects invalid traffic timeslots before clean Pi redeploy

User report:

- Clear the BS log and restart the test BS with the latest deployed updates.
- Keep ETSI clause-scoped hardening and do not claim formal certification.

Component explanation:

- UMAC is the MAC scheduler. It turns MM/CMCE/LLC requests into TETRA downlink and uplink slot usage.
- The circuit manager is UMAC's table of active assigned traffic channels.
- In this single-carrier Nexus-BS scheduler, TS1 is the MCCH/SCH-F common-control carrier. Assigned voice traffic circuits are modelled on TS2..TS4. TS1 may still carry reserved uplink access through ACCESS-ASSIGN, but it must not be converted into an assigned voice traffic channel by a bad request.

ETSI clause scope:

- EN 300 392-2 clause 21.4.6.5: SCH/F/common-control channel context.
- EN 300 392-2 clause 23.5.2.2.7: BS slot granting and energy-economy-aware scheduling.
- This patch is fail-closed local robustness for invalid UMAC requests. It is clause-scoped engineering evidence only, not formal TETRA certification.

Patch:

- `crates/tetra-entities/src/umac/subcomp/circuit_mgr.rs`
  - Added bounds checks for UMAC circuit timeslot access so TS0/TS5+ return/log instead of indexing out of bounds.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - `BsChannelScheduler::create_circuit` now rejects invalid timeslots and rejects TS1 traffic circuits before AACH generation.
  - Added `test_ts1_traffic_circuit_request_is_rejected_without_panic` to prove TS1 remains common control and invalid TS0 does not panic.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib ts1_traffic_circuit --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 51 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deployment note:

- A temporary dirty build `v0.1.55-6e783b4e-modified` was deployed direct to the Pi and restarted once to confirm the patch boots.
- Fresh restart evidence after log clear showed `2260616`, `2260082`, and `2260618` affiliated to GSSI `226333`; no `PTT denied`, `Service unavailable`, `Unit Not Attached`, or `T353` rollback appeared in the filtered startup evidence.
- Next action: commit this patch and redeploy once more so the Pi runs a clean commit build ID, then clear the log and restart again for the operator test.

## 2026-06-05 13:48:26 EEST - MM coverage-return group snapshot hardening for restart `No Group` visibility

User report:

- After BS restart, radios can appear attached but with `No Group`.
- Current live cache and latest restart log still show `2260082`, `2260616`, and `2260618` affiliated to GSSI `226333`, so the remaining hardening target is a status/dashboard-visible group gap rather than a reproduced missing CMCE listener in this restart.

Component explanation:

- MM is Mobility Management. It owns ISSI registration, remembered group affiliations, restart recovery, and the `D-LOCATION-UPDATE-COMMAND` / `U-LOCATION UPDATE DEMAND` flow.
- CMCE is call control. It consumes MM register/affiliate events so group PTT has listeners.
- Dashboard telemetry is observability. It must receive the final group list, not depend on an incremental event that may be absent when MM reuses an already cached client group.

ETSI clause scope:

- EN 300 392-2 clauses 16.9.2.8 and 16.9.3.4: a BS-commanded location update can be answered by `U-LOCATION UPDATE DEMAND` and accepted with the same update type.
- Clause 16.8.0: previously accepted persistent group identities remain valid until a real detach/replacement.
- Clause 16.4.4: SwMI may command location update/group reporting after restart.
- This patch is clause-scoped engineering hardening and dashboard consistency evidence only; it is not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - When a known MS returns after a local `D-LOCATION-UPDATE-COMMAND` without a fresh group report, MM already replays cached groups to CMCE/Brew. It now also emits a full current-group telemetry snapshot.
  - This prevents a dashboard/status `No Group` state when `client_mgr` still has the group and CMCE listener state was restored, but no new `MsGroupAttach` telemetry was generated.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added a telemetry-enabled MM regression for group-less coverage return after periodic command.
  - The test proves the final dashboard replay sees `groups=[3002]` instead of an empty group list.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs test_group_less_coverage_return_publishes_dashboard_group_snapshot --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 25 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_restart_recovery_cached_226333_group_restores_cmce_listeners_after_unrouted_ack --locked` -> 1 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit and deploy direct to the Pi test instance; build locally only and do not create binary backups.
2. Restart test BS and read fresh log plus runtime cache.
3. Verify dashboard/WebSocket state and radio-side group state after restart for `2260082`, `2260616`, `2260618` on `226333`.
4. If a terminal screen still says `No Group` while dashboard shows `groups=[226333]`, capture exact ISSI/time and inspect whether that terminal received/ACKed the group accept/refresh on air.

## 2026-06-05 13:37:22 EEST - Deployed SDS local TSI hardening to Pi test instance

Deployment:

- Deployed direct to `chris@192.168.1.179` test instance with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Built locally only; no build was performed on the Pi and no binary backup was created.
- Running build: `Nexus-BS v0.1.55`, build `v0.1.55-29de6e15`.
- Deployed commit: `29de6e15`.
- Deployed binary SHA256: `365b08eee4e073cf23f8741e77009a888c0587ba69fe4f5c3176de1744e48838`.
- Running processes:
  - `nexus-bs-control-service --listen 127.0.0.1:9002`
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Post-deploy restart evidence:

- Runtime restart cache still contains:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`
- Fresh log from the new build marker contains 416 lines in the startup window.
- `MM: restart recovery armed for 3 local ISSI(s): {2260082, 2260616, 2260618}`.
- `2260082`: `D-LOCATION UPDATE ACCEPT` includes `GroupIdentityLocationAccept` for `226333`; CMCE logs `subscriber affiliate issi=2260082 groups=[226333]`.
- `2260618`: `D-LOCATION UPDATE ACCEPT` includes `GroupIdentityLocationAccept` for `226333`; CMCE logs `subscriber affiliate issi=2260618 groups=[226333]`.
- `2260616`: `D-LOCATION UPDATE ACCEPT` includes `GroupIdentityLocationAccept` for `226333` with EG7 information; CMCE logs `subscriber affiliate issi=2260616 groups=[226333]`.
- No `No Group`, `Unit Not Attached`, `T353`, failed transfer, `PTT denied`, service-unavailable, `FUNCTION NOT SUPPORTED`, or `TSI extension` strings appear in the new startup window.

Remaining live validation:

- No new `U-SDS-DATA` occurred after this deploy in the captured startup window, so the local-MNI TSI SDS fix is validated by local tests and ready for live WAP/browser trigger.
- Next live action: open the terminal WAP/browser home page again and confirm the previous `TSI extension addressing not supported` log does not recur for local MNI.

## 2026-06-05 13:34:45 EEST - SDS local TSI routing hardening and live restart `No Group` re-audit

User report:

- After BS restart, terminals can appear attached with `No Group`.
- Continue broader clause-scoped ETSI EN 300 392-2 hardening without claiming formal certification.

Component explanation:

- SDS is the TETRA short data/status service inside CMCE.
- SSI is the 24-bit local subscriber/group identity. TSI is the full TETRA identity: SSI plus network extension/MNI.
- MNI is MCC+MNC. In this lab the local MNI is `901/9999`, encoded as `(901 << 14) | 9999 = 14771983`.
- A local TSI with our MNI can be routed as a local SSI/GSSI. A foreign TSI must not be collapsed onto a local numeric SSI/GSSI.

ETSI clause scope:

- EN 300 392-2 clause 13.2: SDS includes individual and group user-defined/predefined messages.
- Clauses 13.3.2.1 and 13.3.2.3: TNSDS status/unitdata primitives carry called party SSI and optional called party extension; if absent at the service boundary the current network MNI is assumed.
- Clause 14.7.2.7/table 14.27: `U-STATUS` CPTI=2 carries called party SSI plus called party extension.
- Clause 14.7.2.8/table 14.28: `U-SDS-DATA` CPTI=2 carries called party SSI plus called party extension.
- Clause 14.7.3.2/table 14.33: unsupported SDS/status address forms are rejected with `CMCE FUNCTION NOT SUPPORTED`.
- Clause 18.3.5.3.1: ISSI delivery uses acknowledged L2; GSSI delivery uses unacknowledged unitdata.
- This is clause-scoped engineering evidence only, not formal TETRA certification.

Live restart audit from `/home/chris/nexus-bs-v0.1.55-test/nexus-bs.log` build `v0.1.55-2b334a02`:

- Runtime restart cache still contains:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`
- Latest restart log from `13:21:03 EEST` shows `MM: restart recovery armed for 3 local ISSI(s): {2260082, 2260616, 2260618}`.
- `2260082`, `2260618`, and `2260616` each sent location update with GSSI `226333`; MM returned `D-LOCATION UPDATE ACCEPT` with `GroupIdentityLocationAccept` for `226333`; CMCE logged `subscriber affiliate ... groups=[226333]` for all three.
- No `No Group`, `Unit Not Attached`, `T353`, group refresh reject, failed transfer, `PTT denied`, or service-unavailable string was present in the latest restart log.
- Live config still has `energy_saving_mode = "eg7"` and `call_preemptive = false`.
- Conclusion: the current live evidence does not reproduce a BS-side persistent restart `No Group` state. If a terminal display still shows `No Group`, capture exact ISSI and timestamp so the terminal UI/over-air refresh path can be correlated.

Live SDS issue found:

- The same live log shows three `U-SDS-DATA` attempts rejected as `unimplemented: SDS: TSI extension addressing not supported`, followed by `CMCE FUNCTION NOT SUPPORTED`.
- That rejection is wrong for local TSI when the extension is the configured local MNI.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/sds_bs.rs`
  - Added local MNI decoding from configured MCC/MNC.
  - Replaced blanket TSI-extension rejection with a common called-party address check for `U-SDS-DATA` and `U-STATUS`.
  - Accepts TSI only when called-party extension matches the local MNI.
  - Keeps SNA, external subscriber number, DM-MS address, reserved CPTI, malformed fields, out-of-range SSI, and foreign TSI fail-closed with `CMCE FUNCTION NOT SUPPORTED`.
  - Preserves existing ambiguous ISSI/GSSI numeric collision drop behavior.
- `crates/tetra-entities/tests/test_sds_bs.rs`
  - Added local-MNI TSI tests for ISSI `D-SDS-DATA` and GSSI `D-SDS-DATA`.
  - Added local-MNI TSI tests for ISSI `D-STATUS` and GSSI `D-STATUS`.
  - Converted old TSI tests to prove foreign-MNI TSI is not rewritten to a local registered ISSI.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_sds_bs tsi --locked` -> 6 passed.
- `cargo test -p tetra-entities --test test_sds_bs --locked` -> 116 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 25 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_restart_recovery_cached_226333_group_restores_cmce_listeners_after_unrouted_ack --locked` -> 1 passed.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit the SDS local-TSI patch.
2. Deploy direct to the Pi test instance, building locally only and without binary backups.
3. Re-test the SDS/WAP/browser action that produced local TSI addressing; expected result is no `TSI extension addressing not supported` for local MNI.
4. If any station still visually shows `No Group`, capture exact ISSI and wall-clock time immediately.

## 2026-06-05 13:23:30 EEST - Live deploy validation for restart `No Group` hardening

Deployment:

- Deployed direct to `chris@192.168.1.179` test instance with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- No build was performed on the Pi and no binary backup was created.
- Running build: `Nexus-BS v0.1.55`, build `v0.1.55-2b334a02`.
- Deployed binary SHA256: `822514d2ac772e127c66e0f91c519c9de91e9dd1d1f9752c8bf5f9b64a16f43c`.
- Running processes:
  - `nexus-bs-control-service --listen 127.0.0.1:9002`
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Live restart evidence:

- Runtime cache `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` contains:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`
- Fresh log from the new build marker showed:
  - `MM: restart recovery armed for 3 local ISSI(s): {2260082, 2260616, 2260618}`.
  - `2260082` sent `RoamingLocationUpdating` with `GroupIdentityLocationDemand` for GSSI `226333`; MM returned `D-LOCATION UPDATE ACCEPT` with `GroupIdentityLocationAccept`; CMCE logged `subscriber register` and `subscriber affiliate groups=[226333]`.
  - `2260618` registered and CMCE logged `subscriber affiliate groups=[226333]`.
  - `2260616` registered and CMCE logged `subscriber affiliate groups=[226333]`.
- Dashboard WebSocket snapshot after restart:
  - `2260082`: `groups=[226333]`, `energy_saving_mode=0` after T352 fallback.
  - `2260616`: `groups=[226333]`, `energy_saving_mode=7`, EG frame/multiframe present.
  - `2260618`: `groups=[226333]`, `energy_saving_mode=0` after T352 fallback.

Conclusion:

- The deployed build has all three lab terminals affiliated to `226333` after restart at MM/CMCE/dashboard state level.
- The specific group-less LU interleaving fixed in code did not occur in this fresh live restart because the observed terminals reported GSSI `226333` directly; unit tests cover the missing interleaving.
- Next live action is operator PTT validation on group and private call. If a terminal still displays `No Group`, capture the exact ISSI and timestamp immediately.

## 2026-06-05 13:18:48 EEST - MM restart group refresh survives group-less LU without masking explicit clear

User report:

- After BS restart, terminals reattached but could appear with `No Group`.
- Current hard live config uses EG7, and lab ISSIs are expected to restore GSSI `226333`.

Component explanation:

- MM is Mobility Management: it registers ISSIs, owns group affiliation state, restart recovery cache, SwMI `D-ATTACH/DETACH GROUP IDENTITY`, ACK/T353, and EG negotiation.
- CMCE is call control: it consumes MM register/affiliate events so group PTT has listeners.
- Dashboard renders telemetry cache. The render path cannot show an empty group if the browser state has `groups=[226333]`; an empty row means telemetry/group state was empty or later cleared.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI may command location update and group reporting during restart recovery.
- Clauses 16.8.0 and 16.8.1: accepted/persistent group identities and SwMI-initiated group attach refresh.
- Clause 16.8.6: group attach/detach/report collision handling with other MM procedures.
- Clauses 16.8.5 and 16.11.1.3: acknowledged SwMI group refresh remains bounded by T353.
- Clause 16.10.27a: explicit group-report-complete is authoritative.
- EG7 remains covered by clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7, and 23.7.6.
- This is clause-scoped engineering evidence only, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - A group-less accepted `U-LOCATION UPDATE DEMAND` now preserves a pending restart-recovery SwMI group refresh instead of deleting it before the terminal ACKs.
  - Preservation is deliberately narrow: only restart refresh transactions with rollback/reprobe semantics, only for already-known clients, and not during hard re-registration cleanup.
  - Explicit group state in `U-LOCATION UPDATE DEMAND` still abandons the older pending SwMI refresh, so an empty complete group report can clear stale cache.
  - Rejected LU paths now abandon pending SwMI group transactions before returning: mismatched SSI/MNI, migration reject, disabled MS reject, unsupported feature reject, and whitelist reject.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added restart recovery interleaving tests for group-less LU before SwMI ACK and before T353.
  - Added explicit complete report, hard roaming re-registration, and rejected LU tests proving stale ACKs cannot re-affiliate cleared groups.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 25 passed.
- `cargo test -p tetra-entities --test test_mm_bs swmi_group_ack --locked` -> 12 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_restart_recovery_cached_226333_group_restores_cmce_listeners_after_unrouted_ack --locked` -> 1 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 132 passed.

Next non-repeating execution:

1. Commit and deploy direct to the Pi test instance; do not build on the Pi and do not make binary backups.
2. After restart, capture the fresh BS log from the new process only and the dashboard WebSocket snapshot.
3. Verify live cache still contains `2260082/2260616/2260618` with `226333:0:4`, the log shows SwMI group refresh/ACK or explicit group report, and dashboard state shows `groups=[226333]`.
4. Run the first group PTT/private PTT check after all three ISSIs are attached; if a station still displays `No Group`, capture exact ISSI and timestamp immediately.

## 2026-06-05 13:07:20 EEST - Live restart `No Group` audit under EG7, no protocol patch required

User report:

- After BS restart, stations appeared attached but with `No Group`.
- The live config was intentionally harder: `energy_saving_mode = "eg7"`.

Component explanation:

- MM is Mobility Management: it registers ISSIs, receives/accepts reported GSSIs, persists restart recovery cache, and assigns EG modes.
- CMCE is Call Management and Control Entity: it consumes MM `Register` and `Affiliate` events so group PTT knows which subscribers listen on a GSSI.
- Dashboard is observability: it renders MM telemetry. It is not the protocol source of truth, so live WebSocket state was checked against MM/CMCE logs.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: the SwMI may command location update and request group identity reporting after restart.
- Clauses 16.8.0, 16.8.1, 16.8.4, 16.8.5, and 16.10.27a: group identities are valid when reported/accepted or refreshed by SwMI attach/detach, with T353 bounding acknowledged SwMI group refresh.
- Clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7, and 23.7.6 remain the EG7 scheduling scope.
- This is live engineering evidence for the affected clauses, not formal TETRA certification.

Live evidence:

- Running Pi build: `Nexus-BS v0.1.55`, build `v0.1.55-113f2a91`.
- Runtime cache `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` contained:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`
- Fresh log from the latest restart showed:
  - `MM: restart recovery armed for 3 local ISSI(s): {2260082, 2260616, 2260618}`.
  - `2260618` sent `U-LOCATION UPDATE DEMAND` with GSSI `226333`; MM replied with `D-LOCATION UPDATE ACCEPT` containing `GroupIdentityLocationAccept` for `226333`; CMCE logged `subscriber affiliate issi=2260618 groups=[226333]`.
  - `2260082` sent `DemandLocationUpdating` with GSSI `226333`; MM replied with `D-LOCATION UPDATE ACCEPT` containing `226333`; CMCE logged `subscriber affiliate issi=2260082 groups=[226333]`.
  - `2260616` sent `ItsiAttach` with requested EG7 and GSSI `226333`; MM replied with `D-LOCATION UPDATE ACCEPT` containing EG7 start `F1/MF20` and `GroupIdentityLocationAccept` for `226333`; CMCE logged `subscriber affiliate issi=2260616 groups=[226333]`; LLC/MLE later reported successful transfer for the tracked accept handle.
- Live dashboard WebSocket snapshot showed all three local stations with `groups:[226333]`:
  - `2260616`: EG7, `groups=[226333]`
  - `2260082`: StayAlive after T352 expiry, `groups=[226333]`
  - `2260618`: StayAlive after T352 expiry, `groups=[226333]`

Verification:

- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 22 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_restart_recovery_cached_226333_group_restores_cmce_listeners_after_unrouted_ack --locked` -> 1 passed.
- `cargo check -p tetra-entities --locked` -> pass.

Conclusion:

- Current deployed build did not reproduce dashboard/MM/CMCE `No Group`; the active BS state has all three lab ISSIs affiliated to `226333`.
- No protocol patch was made in this checkpoint because changing MM semantics without a reproduced failing path would risk breaking the clause-scoped restart recovery behavior that is currently passing tests.

Next non-repeating execution:

1. If a terminal screen still shows `No Group` while dashboard WebSocket shows `groups=[226333]`, capture the exact ISSI and timestamp immediately after restart and inspect whether the terminal received/ACKed the `D-LOCATION UPDATE ACCEPT` carrying `GroupIdentityLocationAccept`.
2. Add a focused regression only if a real failing path is found. Candidate edges already identified:
   - EG7 response arriving before cached SwMI group-refresh ACK.
   - Group-less `U-LOCATION UPDATE DEMAND` arriving while a cached SwMI refresh is pending.
   - Cached selected GSSI beyond the first 12-GSSI scan-list refresh batch.
3. Continue broader stack hardening with private/group call, SDS, WAP, and 24/7 robustness tests; do not claim formal certification.

## 2026-06-05 12:03:36 EEST - MM segmented cached restart scan-list refresh

Context:

- Previous restart recovery patch refreshed one cached group over air, but a large cached scan-list could restore more local GSSIs than were carried in one `D-ATTACH/DETACH GROUP IDENTITY`.
- That could split BS and MS state: CMCE would think the terminal is affiliated to unsent groups while the terminal never received those group attachments after restart.

Component explanation:

- MM is Mobility Management: it owns restart recovery, group affiliation, SwMI group attach/detach, and the restart recovery cache.
- A scan-list here means multiple cached GSSIs for one ISSI. It must be refreshed in bounded over-air batches without declaring unsent groups active locally.
- CMCE is call control and depends on MM group affiliation events before it allows group PTT.

ETSI clause scope:

- EN 300 392-2 clause 16.8.0: attached group identities are valid when attached by SwMI and accepted by the MS, or when previous valid attachments remain in force.
- Clause 16.8.1: infrastructure-initiated `D-ATTACH/DETACH GROUP IDENTITY` may add groups using amendment mode and ACK request.
- Clause 16.8.5 / 16.11.1.3: each attach/detach transaction is bounded by T353.
- Clause 16.8.6: avoid colliding group-report and attach/detach procedures.
- Clauses 16.10.13, 16.10.14, 16.10.17, and 16.10.19: ACK request/type, amendment mode, lifetime, and class-of-usage fields.
- This is clause-scoped engineering hardening, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Cached restart restore now restores only the batch that will be sent in the current SwMI `D-ATTACH/DETACH GROUP IDENTITY`.
  - Remaining cached groups are held in the pending SwMI transaction and preserved in the recovery cache while the batch is waiting for ACK.
  - When ACK arrives for a batch, MM restores and sends the next batch. If a batch is rejected or T353 expires, MM keeps the failure rollback/reprobe behavior and does not continue with remaining groups.
  - `GroupIdentityDownlink` for restart refresh now uses the cached `GroupAttachmentInfo` directly, preserving lifetime/class per group.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added a 13-GSSI cached scan-list restart test: first refresh carries 12 groups, ACK triggers the final group refresh, unsent groups are not locally restored early, and the cache retains the full scan-list across the pending transaction.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 20 passed.
- `cargo test -p tetra-entities --test test_mm_bs swmi_group_ack --locked` -> 12 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 123 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group_ --locked` -> 50 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this segmented scan-list restart refresh patch.
2. Next MM restart hardening target: multi-ISSI restart recovery integration into CMCE group PTT, proving `2260082`, `2260616`, and `2260618` all re-affiliate to `226333` before first group call.
3. Remote deploy remains dependent on SSH reachability to `chris@192.168.1.179`.

## 2026-06-05 11:52:03 EEST - MM cached restart group refresh over air, EG7 ordering, and failure rollback

User report:

- After BS restart, terminals attach but display `No Group`.
- Current hard test shape includes cached group `226333`, terminals such as `2260616`/`2260618`, and BS energy saving configured as EG7.

Component explanation:

- MM is Mobility Management: it owns ISSI registration, cached restart recovery, group affiliation, SwMI group attach/detach, and energy economy negotiation.
- CMCE is call control: it depends on MM `Register`/`Affiliate` events to decide whether group/private PTT is valid.
- EG7 is an energy-saving mode: the terminal may sleep for long cycles, so group refresh must be queued before BS-initiated EG7 sleep assignment is activated.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI may initiate registration and request group identity reporting.
- Clause 16.8.0: previously accepted group identities remain valid while their lifetime remains valid.
- Clause 16.8.1: SwMI-initiated `D-ATTACH/DETACH GROUP IDENTITY` must use `group identity report = not report request`, no group-report-response IE, and may use amendment mode.
- Clause 16.8.5 and 16.11.1.3: T353 bounds attach/detach response waiting; expiry is treated as failed refresh in this implementation.
- Clauses 16.8.6, 16.10.13, 16.10.14, 16.10.17, 16.10.19, and Annex G: ACK request/type, amendment mode, attachment lifetime/class, rejected attachment handling, and procedure collision handling.
- Clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7, and 23.7.6 remain the EG scheduling scope. This is clause-scoped engineering hardening, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Cached restart group restore now returns the restored GSSIs and sends a separate acknowledged SwMI `D-ATTACH/DETACH GROUP IDENTITY` refresh after `D-LOCATION UPDATE ACCEPT`.
  - The refresh uses amendment mode and a non-zero local downlink handle; handle `0` is accepted only as an unrouted uplink ACK fallback for this restart-refresh transaction.
  - Cached refresh no longer immediately collides with a fresh `D-LOCATION UPDATE COMMAND(group_identity_report=1)` in the same unsolicited ITSI attach cycle.
  - If the MS rejects the refreshed group or T353 expires, MM rolls back the provisional cached affiliation, persists the bare ISSI cache, and requests a fresh group report.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs` and group FSM/timers
  - Real group floor grants send group-addressed FACCH/STCH `D-INFO` with reset T310 while preserving `UlDlAssignment::Both`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 19 passed.
- `cargo test -p tetra-entities --test test_mm_bs swmi_group_ack --locked` -> 12 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 122 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group_ --locked` -> 50 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 129 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this MM/CMCE patch.
2. Deploy direct when `chris@192.168.1.179` is reachable: `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. After restart, verify terminals `2260082`, `2260616`, and `2260618` show group `226333`, not `No Group`.
4. In fresh logs, confirm cached restart restore is followed by `D-ATTACH/DETACH GROUP IDENTITY`, ACK handling, and no duplicate affiliate event.
5. Retest group PTT under EG7: first PTT after restart should not be denied because CMCE should already have the restored MM affiliate.

## 2026-06-05 11:18:02 EEST - Restart `No Group` remains deploy/log blocked; LMAC fail-closed verified locally

User report:

- After BS restart, stations attach but show `No Group`.
- The test setup is intentionally harsher now because the BS config is expected to run `energy_saving_mode = "eg7"`.

Component explanation:

- MM is Mobility Management: it owns terminal registration, group affiliation, restart recovery, and energy economy negotiation.
- CMCE is call control: it consumes MM `Register`/`Affiliate` events so group calls know which ISSIs are valid listeners.
- Dashboard is observability: it must display MM/CMCE state without losing group events that race registration events.
- LMAC is the lower MAC/channel-coding edge: it turns logical MAC blocks into PHY channel-coded bits and must not encode unsupported logical channels as the wrong channel type.

Current local finding:

- The restart `No Group` fix is already present in local history:
  - `f02371a fix: recover restart candidate groups before eg`
  - Current HEAD is `a5d8b9e docs: record restart no group validation`.
- The relevant local MM/dashboard behavior is already covered:
  - restart candidates are captured before registration removes them from the recovery map;
  - group-less restart candidate self-attach restores cached GSSI locally when present;
  - `D-LOCATION UPDATE COMMAND(group_identity_report=1)` is queued before configured BS-initiated EG7 request;
  - explicit empty complete group reports remain authoritative and clear cached groups;
  - dashboard preserves group attach/snapshot events that arrive before registration events.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI may initiate registration and request group identity report after restart.
- Clauses 16.8.0, 16.8.2, 16.8.3, 16.8.4 and 16.10.27a: reported groups and explicit complete empty reports are authoritative for affiliation state.
- Clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7 and 23.7.6/T.210: EG7 scheduling must not make the terminal sleep before the BS requests the group report.
- LMAC fail-closed work is scoped to implemented channel coding only: unsupported TCH/2.4, TCH/4.8, TCH/7.2 and linearization channels are dropped with warnings instead of being encoded through TCH/S or ordinary C-plane paths.
- This is clause-scoped engineering evidence, not formal TETRA certification.

Verification run:

- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 16 passed.
- `cargo test -p tetra-entities --lib dashboard_ --locked` -> 8 passed.
- `cargo test -p tetra-entities --lib test_unsupported_logical_channels_fail_closed --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 8 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 51 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deploy/log status:

- Commit created after verification:
  - `e7afd48 fix: fail closed unsupported lmac channels`
- `ssh chris@192.168.1.179 ...` timed out on port 22 while trying to read `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` and the full `/home/chris/nexus-bs-v0.1.55-test/nexus-bs.log` from the latest restart.
- Live restart validation and direct deploy are still blocked by SSH reachability, not by local code/test failures.

Next non-repeating execution:

1. When SSH returns, deploy directly with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`; do not compile on the Pi and do not create binary backups.
2. Confirm remote build id is current HEAD and read the cache:
   - expected: `2260082 226333:0:4`, `2260616 226333:0:4`, `2260618 226333:0:4` or equivalent class values.
3. Read the fresh full log from the latest restart and compare terminal/dashboard `No Group` with MM/CMCE `subscriber affiliate` evidence before making another MM patch.

## 2026-06-05 09:49:05 EEST - Group restart grant and raw Block2 same-burst floor-release hardening

Live problem targeted:

- Latest restart log showed group PTT/service-unavailable symptoms after a previous same-GSSI group call was still draining `D-RELEASE`.
- Concrete log pattern: new `U-SETUP` for GSSI `226333` from `2260616` arrived while the stale call release was pending; old CMCE logic rejected it with `RequestedServiceNotAvailable`.
- User also reported intermittent static/no voice on the other station during group/PTT transitions. UMAC/LMAC audit found a narrow race where `NormalTrainSeq2` Block1 STCH can carry `U-TX CEASED` while Block2 is still accepted as raw TCH/S before CMCE floor release reaches UMAC.
- EG7 note: the current latest restart log shows `2260616` requested `Eg7`, but Nexus-BS accepted/configured `Eg3` on air. True EG7 field testing requires the BS energy-saving config to assign EG7; terminal-side preference alone is not what the current SwMI advertises.

Components, simple technical meaning:

- CMCE: call control and PTT/floor logic. It decides `D-CALL PROCEEDING`, `D-CONNECT`, `D-SETUP`, `D-TX GRANTED`, `D-TX CEASED`, and `D-RELEASE`.
- UMAC: MAC scheduler/media router. It receives voice from LMAC, applies active circuit/floor state, and schedules DL TCH/S or FACCH/STCH signalling.
- LMAC: lower MAC burst classifier. It decides whether a burst half is `STCH` signalling or `TCH/S` speech before passing it upward.
- MM/EG: mobility/energy-economy negotiation. It assigns StayAlive/EG1..EG7 and the frame/multiframe where sleeping terminals should listen.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1: normal group call setup after a later `U-SETUP`.
- EN 300 392-2 clause 14.5.2.3: old group call release with `D-RELEASE` must drain independently.
- EN 300 392-2 clause 23.5: FACCH/STCH stealing and traffic/signalling half-slot distinction.
- EN 300 392-2 clauses 23.8.4.1.4 and 23.8.5: valid non-stolen TCH/S half-slot timing/position must be preserved; stale or floor-released media must not be fabricated as clean speech.
- EN 300 392-2 clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7, 23.7.6, and T.210 remain the active scope for EG3/EG7 scheduler safety. This is engineering hardening, not formal certification evidence.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Same-GSSI `U-SETUP` now ignores stale pending-release calls when deciding whether an active call already exists.
  - If only a pending-release call exists for the same GSSI, CMCE starts a fresh group call instead of sending service-unavailable `D-RELEASE`.
  - The stale pending release remains tied to its original assigned channel and closes only when its reporter/guard completes.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Raw `NormalTrainSeq2` Block2 TCH/S is now deferred in UMAC until same-burst STCH/CMCE floor-control has drained.
  - `FloorReleased`, `FloorGranted`, `CallEnded`, circuit close, and replacement open discard deferred stale raw media before it can enter the DL scheduler.
  - Valid raw Block2 is still preserved and emitted after the deferral window when the UL/DL circuit is active, not in hangtime, and speaker/peer routing still matches.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added/regressed same-GSSI pending-release restart coverage with fresh call id, fresh traffic allocation, no service-unavailable `D-RELEASE`, and old release closing only its old slot.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs raw_block2 --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group_release_pending --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 51 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 129 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 8 passed.
- `cargo test -p tetra-entities --test test_mm_bs energy --locked` -> 29 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Run final focused tests once more after commit/deploy build.
2. Deploy direct with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Retest group GSSI `226333`: first PTT immediately after prior call release/hangtime must grant on first try, not second try, and no `RequestedServiceNotAvailable` should appear for same-GSSI replacement setup.
4. Retest static/no-voice: alternating group PTT should not transmit raw Block2 after `U-TX CEASED`/`D-TX CEASED`.
5. If true EG7 field test is desired, set the BS config to assign EG7 and verify attach accept advertises `EnergySavingInformation { energy_saving_mode: Eg7, ... }`; otherwise current on-air behavior remains EG3 despite terminal request.

## 2026-06-05 01:23:50 EEST - LMAC first private-simplex TCH/S recovery before traffic marker

Live problem targeted:

- User reported current private simplex P2P is broken.
- Fresh live grep on running build `v0.1.55-bcc5e08b` did not show any decoded private-call CMCE sequence (`U-SETUP`, `D-CONNECT`, `U-TX DEMAND`, `D-TX GRANTED`) after the report.
- The same log did show uplink `NormalTrainSeq` fullslot bursts without higher-layer P2P decode, which is consistent with first traffic bursts arriving while the BS-side lower MAC has not yet marked that UL timeslot as `Tp`, or with a terminal still transmitting on a traffic slot the current BS state has not opened.

Components, simple technical meaning:

- LMAC: lower MAC classifier. It receives demodulated bursts from PHY and decides whether they are control signalling (`SCH/F`, `SCH/HU`, `STCH`) or speech traffic (`TCH/S`).
- UMAC: upper MAC circuit/router. It accepts speech only when CMCE has opened a matching circuit, so an LMAC fallback cannot create a call by itself.
- CMCE private simplex: call-control state machine that opens the private call and owns PTT floor permission. This patch does not change CMCE grants or release rules.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: private simplex calls use SwMI-controlled transmit permission; initial transmit permission from setup must still allow valid speech once the assigned channel is opened.
- EN 300 392-2 clause 23.5.2.2.1: MAC resource/channel allocation transitions the MS to the assigned physical channel.
- EN 300 392-2 clauses 23.8.3, 23.8.3.2 and 23.8.5: TCH/S speech frames must not be converted from bad CRC or partial conditions into clean audio; a non-stolen second half-slot must preserve its half-slot timing/position.
- This is clause-scoped engineering hardening and RF-retest preparation, not formal ETSI/TETRA certification evidence.

Patch implemented:

- `crates/tetra-entities/src/lmac/lmac_bs.rs`
  - `rx_blk_control` now returns whether a valid control block was actually forwarded.
  - If a `NUB` on an as-yet `Unallocated` UL slot fails as control, LMAC retries only the TCH/S-compatible cases:
    - `NormalTrainSeq1 + Both` full-slot TCH/S.
    - `NormalTrainSeq2 + Block2` non-stolen raw TCH/S half-slot.
  - Full-slot fallback still requires the TCH/S speech CRC to pass; bad CRC remains dropped so static is not forwarded as clean speech.
  - Raw Block2 fallback is still handed to UMAC, where it is dropped unless a matching active circuit exists.
- `crates/tetra-entities/tests/test_lmac_bs.rs`
  - Added first-burst fallback coverage for full-slot TCH/S before the UL traffic marker is present.
  - Added first-burst fallback coverage for raw `NormalTrainSeq2` Block2 TCH/S before the UL traffic marker is present.
  - Added bad-CRC unknown-channel fallback regression so corrupt speech is not emitted as audio.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added private simplex initial-floor media regression proving UMAC routes the first TCH/S burst after `CallControl::Open` without requiring an extra `FloorGranted`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 8 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 63 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 51 passed.
- `cargo check -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Run `git diff --check`, commit, and deploy direct to `/home/chris/nexus-bs-v0.1.55-test`.
2. Retest private simplex `2260616 <-> 2260618`.
3. Expected live evidence for the targeted race: if the terminal sends immediate speech, log should now show `LMAC: retrying undecoded NUB as candidate TCH/S`, then `rx_blk_traffic: decoded valid TCH/S frame` or raw Block2 forwarding, followed by `UMAC voice route`.
4. If no `U-SETUP`/`U-TX DEMAND` appears and only raw `NormalTrainSeq` continues, diagnose stale terminal traffic-channel state/recovery separately instead of changing CMCE floor logic.

## 2026-06-04 23:34:48 EEST - LLC inbound duplicate guard bounded by T.251/N.252

Live problem targeted:

- User reported group-call tests with visible terminal-side `PTT denied`.
- Live BS log review did not show CMCE sending a floor denial in the sampled window; repeated group `U-TX DEMAND` events were answered with `D-TX GRANTED`.
- The same log did show a lower-layer signalling fault:
  - `23:28:32.289`: `LLC: suppressing duplicate inbound BL-DATA/BL-ADATA N(S)=1 for SSI 2260618 endpoint 0; ACK remains scheduled`.
  - This occurred many seconds after prior `2260618` signalling, so the old unbounded receive duplicate memory could suppress a new CMCE PDU before it reached call control.

Components, simple technical meaning:

- LLC: logical link control between MAC and MLE/CMCE/SDS. It acknowledges BL-DATA/BL-ADATA and prevents duplicate service-user delivery when a peer retransmits because its ACK was lost.
- `N(S)`: one-bit basic-link sequence number for acknowledged data.
- `inbound_receive_seq`: per-terminal/per-endpoint memory of the last valid inbound `N(S)`.
- Patch scope: keep duplicate suppression for short retransmission windows, but stop treating the same `N(S)` as a duplicate forever.

ETSI clause scope:

- EN 300 392-2 clause 22.3.2.3: acknowledged BL-DATA/BL-ADATA uses `N(S)`/`N(R)` and must ACK valid inbound data; BL-ADATA handles ACK first and DATA second.
- EN 300 392-2 clause 22.3.2.3 note 3: numbering alone does not guarantee safe duplicate suppression.
- EN 300 392-2 Annex A.1: T.251 is counted in downlink signalling frames; default T.251 is 4 signalling frames.
- EN 300 392-2 Annex A.2: N.252 defines the maximum retransmissions; local configured value is 3, giving a conservative duplicate guard of `(3 + 1) * 4 = 16` downlink signalling frames.
- This is clause-scoped robustness hardening, not formal ETSI certification evidence.

Patch implemented:

- `crates/tetra-entities/src/llc/llc_bs_ms.rs`
  - `ReceiveSeqState` now stores `last_ns`, `received_at`, and `ack_timeslot`.
  - Added a duplicate-suppression horizon based on `(N.252 + 1) * T.251`.
  - Prunes expired receive-sequence entries before duplicate comparison.
  - Does not refresh the duplicate window when suppressing a duplicate, so repeated stale frames cannot extend suppression indefinitely.
  - Keeps duplicate BL-DATA/BL-ADATA ACK scheduling intact.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_llc_bs inbound --locked` -> 6 passed.
- `cargo test -p tetra-entities llc::llc_bs_ms::tests::inbound_duplicate_guard_expires_after_full_retransmission_horizon --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 80 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deployment result:

- Committed as `8452f9b fix: bound LLC inbound duplicate suppression`.
- Deployed direct to testing with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Running BS build: `v0.1.55-8452f9b2`.
- Deployed binary SHA-256: `f385e880db8df5cd5e79541d004c616cf9583f1b8d4027a424eadf6fac01cc08`.
- Post-start log showed `2260082`, `2260618`, and `2260616` registered and affiliated to `226333`.
- Post-start log also showed the new bounded behaviour:
  - `LLC: expiring inbound duplicate guard for SSI 2260082 endpoint 0 N(S) 1`.
- No immediate `PTT denied`, `RequestedServiceNotAvailable`, `Service unavailable`, or `Unit Not Attached` lines appeared in the post-deploy filter.

Next non-repeating execution:

1. Retest group alternating PTT with `2260616`, `2260618`, and `2260082` on GSSI `226333`.
2. Expected live evidence: no stale `LLC: suppressing duplicate inbound ...` for new group-call control after the T.251/N.252 horizon; CMCE should receive the control PDU and either grant or explicitly log any real floor denial reason.

## 2026-06-04 23:21:39 EEST - Dashboard CPU model detection across boards

Problem targeted:

- Dashboard System tab showed `unknown (4 cores)` on the live test board.
- Live board evidence:
  - `/proc/cpuinfo`: `CPU implementer=0x41`, `CPU part=0xd03`, `CPU architecture=8`, 4 processors.
  - `/proc/device-tree/model`: `Raspberry Pi Zero 2 W Rev 1.0`.
  - `/proc/device-tree/compatible`: `raspberrypi,model-zero-2-w`, `brcm,bcm2837`.
  - `cpuinfo_max_freq`: `1000000`.
  - `uname -m`: `aarch64`.

Components, simple technical meaning:

- Dashboard `/api/system`: HTTP endpoint that feeds the System tab.
- CPU descriptor parser: converts Linux kernel CPU identity fields into a readable hardware string such as `Broadcom Cortex-A53 1GHz 64-bit`.
- This is observability/dashboard work, not TETRA air-interface signalling and not formal conformance evidence.

Patch implemented:

- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Replaced single-line `/proc/cpuinfo` lookup with a board-aware parser.
  - Uses `/proc/cpuinfo`, device-tree `model`/`compatible`, `cpufreq` max frequency, and `uname -m`.
  - Maps ARM implementer/part IDs to core names including Cortex-A53/A55/A72/A76/A78/A510/A710, Neoverse, Broadcom Brahma, Qualcomm Kryo/Krait/Falkor, and NVIDIA Denver/Carmel.
  - Keeps x86 `model name` intact while adding architecture width when available.
  - Added tests for the live Raspberry Pi Zero 2 W case and x86 model-name preservation.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities net_dashboard::server::tests::cpu_descriptor --locked` -> 2 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deployment result:

- Committed as `5d4b888 fix: detect dashboard CPU model across boards`.
- Deployed direct to testing with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Running BS build: `v0.1.55-5d4b888c`.
- Deployed binary SHA-256: `a251ba3a82d5b5e02054254dd1e27dfe1820efe6c93ae8c4f39a1de39e2bc244`.
- Live `/api/system` verification:
  - `cpu_model`: `Broadcom Cortex-A53 1GHz 64-bit`.
  - `cpu_cores`: `4`.
  - UI should render `Broadcom Cortex-A53 1GHz 64-bit (4 cores)`.
- Post-start radio state still showed `2260082`, `2260618`, and `2260616` registered and affiliated to `226333`.

## 2026-06-04 23:09:56 EEST - Private simplex MXP600 last-speaker release guard

Live problem targeted:

- User retested `2260616 -> 2260618`, let `2260618` speak last, then hung up with red key on `2260616`.
- Deployed test BS was already `Build: v0.1.55-5f03000c`.
- Live log evidence:
  - `22:54:54` private simplex `call_id=4` opened from `2260616` to `2260618`; initial floor holder `2260616`.
  - `22:54:59` `2260618` obtained the private simplex floor.
  - `22:55:04` `2260618` sent `U-TX CEASED`; CMCE tail-drained and sent `D-TX CEASED`.
  - `22:55:05` `2260616` sent `U-DISCONNECT`; because `floor_holder` had already been cleared to `None`, the previous code fell back to peer `D-DISCONNECT`.
  - `22:55:05.339` `2260618` sent `U-RELEASE`, then `22:55:23` re-attached, matching the reported MXP600 soft reboot.

Components, simple technical meaning:

- CMCE/CC-BS: private-call control. It tracks who is in the call, who has PTT floor, and which release PDU is sent.
- `floor_holder`: current simplex private speaker, if someone is actively transmitting.
- `last_floor_holder`: new retained memory of the last simplex private speaker after `U-TX CEASED`.
- UMAC: assigned-channel bearer/router. It stays open until CMCE release reporters prove the release messages were sent.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: private simplex transmit permission is controlled by SwMI with `D-TX GRANTED`/`D-TX CEASED`.
- EN 300 392-2 clause 14.5.1.3.1: either user may initiate disconnection; the MS sending `U-DISCONNECT` waits for `D-RELEASE`; the SwMI may inform the other MS by either `D-DISCONNECT` or `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3: `D-DISCONNECT` expects `U-RELEASE`; `D-RELEASE` expects no response.
- EN 300 392-2 clause 23.8.5 still motivates the bounded speech-bearer tail drain before peer clear signalling. This is clause-scoped engineering hardening plus a Motorola compatibility guard, not formal certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/call.rs`
  - Added `last_floor_holder` to `IndividualCall`.
  - Added `set_floor_holder`, `clear_floor_holder`, and `peer_is_current_or_last_floor_holder` helpers so floor transitions retain the last simplex speaker.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Private simplex `U-DISCONNECT` now sends peer `D-RELEASE` if the peer is the current or most recent floor holder, even if it already sent `U-TX CEASED`.
  - Passive-peer cases may still use the existing `D-DISCONNECT -> U-RELEASE` path after tail drain.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/network.rs`
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Replaced direct floor mutations with the new helpers so queued handoff, normal floor grant, inactivity recovery, and setup activation all keep `last_floor_holder` coherent.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added field regression test for exact live sequence: `2260616 -> 2260618`, peer gets floor, peer sends `U-TX CEASED`, caller hangs up, peer receives tail-drained `D-RELEASE` and no `D-DISCONNECT`.

Verification:

- `cargo check -p tetra-entities --locked` -> pass.
- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_caller_disconnect_releases_mxp600_peer_after_peer_ceased_last_floor --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 63 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 125 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 9 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deployment:

- Commit: `89404b9 fix: release last private simplex speaker`.
- Deployed direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot path; no build on `chris@192.168.1.179`.
- Remote build banner: `Build: v0.1.55-89404b98`.
- Remote binary SHA-256: `fdd966e670a3bd1895880537566e7ae930fb0688a377b59e2ea8de18b4746fcf`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`
- Post-start log showed `2260618`, `2260616`, and `2260082` registered and affiliated to GSSI `226333`.

Next non-repeating execution:

1. Retest the exact field case: `2260616 -> 2260618`, let `2260618` speak last and release PTT, then hang up from `2260616`.
2. Expected live evidence: prompt `D-RELEASE` to `2260616`, no `D-DISCONNECT` to `2260618`, tail-drained `D-RELEASE` to `2260618`, no `U-RELEASE` required from `2260618`, no MXP600 reboot.
3. If reboot still occurs, capture whether any peer-directed `D-DISCONNECT` remains in the release window; do not patch further without fresh log evidence.


## 2026-06-04 22:32:27 EEST - Private simplex peer-floor hangup tail-drain

Problem targeted:

- User report: private simplex `2260616 -> 2260618` works for voice, but when `2260616` hangs up with the red key, the peer Motorola MXP600 `2260618` soft reboots.
- The current deployed build after the previous patch is `Build: v0.1.55-a3bc4078`; the sampled post-deploy log did not contain a fresh private-call attempt, only startup/register/affiliate activity.
- Audit gap found in the current code: peer-facing `D-DISCONNECT` was tail-drained only when the disconnecting MS was the current simplex floor holder. If the peer MXP600 was the current/last floor holder and the caller hung up, Nexus-BS could still send `D-DISCONNECT` to that peer immediately.

Components, simple technical meaning:

- CMCE/CC-BS: call-control state machine. It decides private-call setup, PTT floor ownership, `D-RELEASE`, `D-DISCONNECT`, and when the traffic circuit may close.
- UMAC: assigned traffic-channel owner/router. It keeps the simplex bearer open while CMCE drains/release-confirms the call.
- Tail-drain guard: a short bounded wait before peer-facing clear signalling so the speech bearer is not cleared in the same instant as recent TCH/S traffic.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: simplex individual calls use controlled `U-TX DEMAND`, `D-TX GRANTED`, `U-TX CEASED`, and `D-TX CEASED`; no unsolicited peer grant is introduced.
- EN 300 392-2 clauses 14.5.1.3.1 and 14.5.1.3.3: the MS that sends `U-DISCONNECT` receives `D-RELEASE`; the peer leg is cleared with `D-DISCONNECT` and answers with `U-RELEASE`.
- EN 300 392-2 clause 14.7.1.6: `D-DISCONNECT` expects `U-RELEASE`.
- EN 300 392-2 clause 23.8.5 gives an N-1 traffic-slot tail-bit rule for N=4/8 circuit-mode data. Applying the same short N=4-equivalent guard to TCH/S speech remains a bounded Motorola/bearer compatibility guard, not a formal certification claim.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Private simplex `U-DISCONNECT` now tail-drains peer-facing `D-DISCONNECT` whenever any simplex floor holder is active, not only when the disconnecting MS is the floor holder.
  - Prompt `D-RELEASE` to the MS that pressed red is still immediate.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added reusable helpers for field-ISSI private-call setup.
  - Added `LAB_ISSI_MXP600 = 2260618`.
  - Added regression test for `2260616 -> 2260618`: 2260618 obtains the private simplex floor, 2260616 hangs up, `D-RELEASE` goes promptly to 2260616, and `D-DISCONNECT` to 2260618 appears only after tail-drain.
  - Updated the mirrored called-party disconnect test so the floor-holding peer is also tail-drained before `D-DISCONNECT`.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_caller_disconnect_tail_drains_when_mxp600_peer_holds_floor --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 62 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 124 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 9 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deployment:

- Commit: `c01572f fix: tail-drain private peer-floor disconnect`.
- Deployed direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Remote build banner: `Build: v0.1.55-c01572fb`.
- Remote binary SHA-256: `92ca23ac132c508a12776c3759bbbf1603899c782600b4326419b922f0e67f31`.
- Post-start log: `2260082`, `2260618`, and `2260616` registered and affiliated to group `226333`.

Next non-repeating execution:

1. Retest exact Motorola case: `2260616 -> 2260618`, make 2260618 talk last if possible, then hang up on `2260616`.
2. Expected live log: prompt `D-RELEASE` to 2260616, no immediate `D-DISCONNECT` to 2260618, delayed `D-DISCONNECT` after tail-drain, peer `U-RELEASE`, no fallback timeout, no MXP600 reboot.
3. If a reboot still happens, inspect whether `2260618` was the `D-DISCONNECT` recipient or whether it sent/failed to send `U-RELEASE` before changing sequencing again.

## 2026-06-04 21:54:29 EEST - Patched private simplex hangup No Answer release acknowledgement

User symptom:

- Motorola showed `No answer` at the end of a private simplex call.
- The live test BS was already running `Build: v0.1.55-a26e3a23`, which had `D-DISCONNECT` response capability and required peer `U-RELEASE`, but the current post-restart log sample did not contain a fresh private-call release attempt.

Component in simple technical terms:

- CMCE/CC-BS is the call-control state machine for private and group calls.
- `U-DISCONNECT` is the terminal request to end a private call.
- `D-RELEASE` is the BS response expected by the terminal that requested the end of the call.
- `D-DISCONNECT` is the BS request to clear the other terminal; that peer answers with `U-RELEASE`.
- UMAC stays responsible for keeping the assigned traffic channel open until the required release messages are reported transmitted.

ETSI clause scope checked:

- EN 300 392-2 clause 14.5.1.3.1 says an MS that sends `U-DISCONNECT` waits for `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3 says an MS receiving `D-DISCONNECT` responds with `U-RELEASE`, while `D-RELEASE` expects no response.
- EN 300 392-2 clause 14.7.1.6 defines `D-DISCONNECT` with response expected `U-RELEASE`.
- EN 300 392-2 clause 14.7.1.9 defines `D-RELEASE` as the infrastructure release message with no response expected.
- This is clause-scoped engineering alignment only, not formal ETSI/TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - A valid active private-call `U-DISCONNECT` now sends prompt `D-RELEASE` to the requesting MS before/alongside peer clearing.
  - Peer clearing still uses `D-DISCONNECT` and waits for peer `U-RELEASE`; peer `U-DISCONNECT` is still not treated as the acknowledgement.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added pending prompt-release ACK tracking so the traffic circuit closes only after the initiator `D-RELEASE` is transmitted and peer clearing completes.
  - Fallback paths for lost/discarded `D-DISCONNECT` now avoid duplicating `D-RELEASE` to the initiator; they release only the remaining peer leg when the prompt ACK was already sent.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Timer drain now includes pending private disconnect release ACKs.
  - Peer `U-RELEASE` timeout uses the new peer-leg fallback release path.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated private-call release tests to assert prompt initiator `D-RELEASE`, peer `D-DISCONNECT` with `UlDlAssignment::Both`, no duplicate initiator release after peer `U-RELEASE`, and no UMAC close before release reporters complete.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 61 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 123 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 9 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Build/deploy:

- Commit: `5acff30 fix: ack private disconnect initiator promptly`.
- Deployed with the one-shot local script:
  - `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Remote deployed binary SHA-256:
  - `a74b39670e1af2bd0f09e7a2fbfd518c2c1375b69fc06632d11fe8db01bf5607`
- Startup banner reports `Build: v0.1.55-5acff30d`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`
- Post-restart register/affiliate observed for `2260082`, `2260618`, and `2260616` on GSSI `226333`.

Next live validation:

- Retest private simplex between `2260082` and `2260616`.
- Expected live evidence: after one terminal hangs up, logs show `U-DISCONNECT`, prompt `D-RELEASE` to that ISSI, `D-DISCONNECT` to the peer with assigned channel response capability, peer `U-RELEASE`, and no `Pending individual D-DISCONNECT timed out`.

## 2026-06-04 21:01:37 EEST - Fixed private simplex first-PTT floor inversion for hook setup

User symptom:

- Private simplex call first PTT did not pass voice while PTT was held; a second PTT worked.
- Live private-call log showed the concrete bad sequence:
  - `2260616 -> 2260618` `U-SETUP` with `hook_method_selection=true`, `simplex_duplex_selection=false`, and `request_to_transmit_send_data=true`.
  - Nexus-BS then sent `D-CONNECT` to the caller with `transmission_grant=GrantedToOtherUser`.
  - Nexus-BS sent `D-CONNECT-ACKNOWLEDGE` to the called MS with `transmission_grant=Granted`.
  - CMCE recorded `initial floor_holder = ISSI 2260618`.
  - Later `2260616` sent `U-TX DEMAND`, Nexus-BS granted floor to `2260616`, and voice route became active.

Component in simple technical terms:

- CMCE/CC-BS is the call-control brain. In a private simplex call it decides which terminal gets the first transmit floor in `D-CONNECT` / `D-CONNECT-ACKNOWLEDGE`.
- UMAC is the traffic-channel router. It uses the CMCE `CallControl::Open.active_addr` speaker to decide whose uplink TCH/S speech is valid and should be looped to the listeners on the shared simplex bearer.
- The bug was in CMCE's interpretation of one setup bit, not in encryption, WAP, or RF.

ETSI clause scope checked:

- EN 300 392-2 clause 14.5.1.2.1 says the SwMI fully controls private-call transmit permission.
- For on/off-hook signalling, normal operation gives the called MS permission to transmit, but if the calling MS sets the `request to transmit` bit in `U-SETUP`, the calling MS is asking for transmit permission.
- For direct setup signalling, normal operation gives the calling MS permission to transmit; the same bit allows the called user application to request permission first, but it is not an automatic grant to the called MS.
- EN 300 392-2 table 14.80 defines `transmission_grant`: granted, not granted, queued, or granted to another user.
- This is clause-scoped engineering alignment only, not formal ETSI/TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Initial private simplex floor selection now interprets `request_to_transmit_send_data` by setup method:
    - hook/on-off signalling with bit set -> caller receives initial floor;
    - hook/on-off signalling with bit clear -> called MS receives normal initial floor;
    - direct setup -> caller remains initial floor; the bit only permits called-first request flow, not automatic called grant.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/call.rs`
  - Updated the field comment so future patches do not reintroduce the inverted interpretation.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added/updated focused tests for hook setup with and without request-to-transmit:
    - `hook=true, request=true` keeps the calling MS as initial UMAC speaker and sends caller `D-CONNECT Granted`;
    - `hook=true, request=false` keeps the called MS as normal initial speaker and sends caller `D-CONNECT GrantedToOtherUser`.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_hook_setup_request_to_transmit_keeps_calling_ms_initial_floor --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 121 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Build/deploy:

- Commit: `9469dd2 fix: align private simplex initial floor`.
- Deployed with the one-shot local script:
  - `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Remote deployed binary SHA-256:
  - `7eafdfa7df28472caee5f776411b56fdeffb39b0d0b7dd0e173605bf7f2f95cb`
- Startup banner reports `Build: v0.1.55-9469dd20`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`
- Post-restart register/affiliate observed for `2260618`, `2260082`, and `2260616` on GSSI `226333`.

Next live validation:

- Deploy this patch to the test BS.
- Retest private simplex while holding PTT on the calling terminal.
- Expected evidence for the first call attempt:
  - `D-CONNECT` to caller has `transmission_grant=Granted`;
  - `D-CONNECT-ACKNOWLEDGE` to called MS has `transmission_grant=GrantedToOtherUser`;
  - `Simplex P2P initial floor_holder` matches the caller;
  - `UMAC voice route` appears during the first held PTT, without waiting for a second `U-TX DEMAND`.

## 2026-06-04 20:08:31 EEST - Extended assigned-channel usage marker to group floor cease/interrupt

Live evidence after `33ef3ca`:

- The deployed build no longer showed `PTT denied`, `NotGranted`, or `RequestedServiceNotAvailable` in the sampled group PTT window.
- Group floor grants were accepted at CMCE/UMAC level:
  - `2260082` and `2260618` each received `FSM -> D-TX GRANTED (individual, Granted)` on `call_id=4`.
  - `UMAC floor granted` followed the active speaker ISSI.
- Remaining defect moved lower: repeated `UL inactivity timeout on ts=2` showed the BS granted floor but did not receive/accept valid uplink traffic consistently after some grants.
- The live log also showed `D-TX CEASED` FACCH/STCH still carried `chan_alloc.usage=None`, while `D-TX GRANTED` had already been fixed to carry `usage=Some(4)`.

Component in simple technical terms:

- `D-TX CEASED` tells group listeners that the current speaker stopped and the floor is released.
- `D-TX INTERRUPT` withdraws a current speaker during supported pre-emption.
- Both are CMCE floor-control messages carried by UMAC as MAC-RESOURCE/STCH on the same assigned traffic channel.
- If their STCH wrapper lacks the active usage marker, a terminal may treat the signalling as not belonging to the traffic circuit it is monitoring.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1 defines SwMI group floor control, including granting, queueing, denial, interruption, and floor release while MSs remain in `CALL-ACTIVE`.
- EN 300 392-2 clause 23.8.1 says TCH/STCH on the assigned channel uses the corresponding traffic usage marker.
- EN 300 392-2 clause 23.8.2.3.1 says transmit traffic needs both CC authorization and an applicable uplink traffic usage marker.
- EN 300 392-2 clause 23.8.4.2 permits downlink C-plane signalling on STCH using MAC-RESOURCE.
- This remains clause-scoped engineering alignment, not formal conformance certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - `send_d_tx_ceased_facch` now accepts `usage` and sends DL-only FACCH/STCH with `chan_alloc.usage=Some(usage)`.
  - `send_d_tx_interrupt_facch` now accepts `usage` and sends DL-only FACCH/STCH with `chan_alloc.usage=Some(usage)`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - Passes active call usage into floor cease and pre-emption interrupt paths.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - UL-inactivity-forced floor release now preserves the call usage marker in `D-TX CEASED`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - `D-TX CEASED` and `D-TX INTERRUPT` tests now assert the actual circuit timeslot, usage marker, and DL-only direction.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 118 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 46 passed.
- `git diff --check` -> pass.

Build/deploy:

- Commit: `dcb542d fix: preserve group floor signalling usage marker`.
- Deployed with the one-shot local script:
  - `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Remote deployed binary SHA-256:
  - `3a075b027925d89c986032ca82ab06514e0be38cb9ef652fae2e1b49578901b1`
- Restarted test BS with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Live evidence after deploy:

- Startup banner reports `Build: v0.1.55-dcb542dd`.
- `2260616` and `2260082` registered and affiliated to `226333`.
- Operator clarified that the active radios are now both on GSSI `226333`, so the next defect isolation must stay on group floor/traffic handling instead of treating the current issue as a group mismatch.

Next live validation:

- Test BS is already running `v0.1.55-dcb542dd`.
- Retest alternating group PTT on GSSI `226333`, preferably first between `2260616` and `2260082`.
- Required evidence: no terminal `PTT denied`, no BS `NotGranted` unless another MS is truly active, `D-TX CEASED` FACCH/STCH shows `usage=Some(4)`, no repeated `UL inactivity timeout` immediately after floor grants, and audio is intelligible.
- If CMCE grants floor but audio is static or one-way, move immediately to UMAC/LMAC traffic evidence: valid TCH/S uplink frames after `UMAC floor granted`, direction by direction, before changing call-control again.
- Parallel audit agents are active for CMCE group/private control, UMAC/MAC traffic path, MM restart affiliation/EG, QA tests, and project-log continuity.

## 2026-06-04 20:01:36 EEST - Patched group return-PTT alias to pure floor control

User symptom:

- Live group-call validation still reported terminal-side `PTT denied` on return PTT.
- Previous logs showed CMCE/UMAC sending `D-TX GRANTED` and `FloorGranted`, so the remaining risk was not a BS-side `NotGranted` decision, but inconsistent active-call signalling around the grant.

Components in simple technical terms:

- CMCE/CC is call control. It decides whether a group call is being set up or whether an already active group call is only changing the speaker/floor.
- `D-TX GRANTED` is the CMCE floor response that tells a terminal whether it may transmit now, is queued, or is not granted.
- MLE/UMAC wraps that CMCE message into MAC-RESOURCE/STCH on the assigned traffic channel.
- The traffic usage marker is the MAC label for the active assigned channel. Without it, a terminal can receive signalling but still not treat it as valid permission for that traffic circuit.

ETSI clause scope checked:

- EN 300 392-2 clause 14.5.2.1 covers group-call setup with `D-CALL PROCEEDING`, `D-CONNECT`, and `D-SETUP`.
- EN 300 392-2 clause 14.5.2.2.1 covers active group-call floor control with `U-TX DEMAND` / `D-TX GRANTED`; queued/not-granted/granted floor responses keep the MS in `CALL-ACTIVE`.
- EN 300 392-2 clause 23.8.1 says the BS allocates a traffic usage marker for the assigned channel and that TCH/STCH traffic uses the corresponding usage marker.
- EN 300 392-2 clause 23.8.2.3.1 says an MS shall not transmit traffic unless authorized by CC and unless it has an applicable uplink traffic usage marker.
- EN 300 392-2 clause 23.8.4.2 allows downlink C-plane signalling on STCH using MAC-RESOURCE.
- This is clause-scoped engineering alignment only. It is not formal ETSI/TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - A compatible repeated `U-SETUP` to an already maintained same-GSSI call is now treated as a floor-request alias.
  - It no longer emits setup-phase `D-CALL PROCEEDING` or `D-CONNECT` before the floor response.
  - It still rejects releasing, unaffiliated, or incompatible same-GSSI attempts with the existing release path.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - Individual `D-TX GRANTED` FACCH/STCH wrappers now preserve the active traffic `usage` marker.
  - `Granted` uses `UlDlAssignment::Both`; `RequestQueued` and `NotGranted` use `UlDlAssignment::Dl`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Group-addressed `D-TX GRANTED GrantedToOtherUser` FACCH/STCH now also preserves the active traffic `usage` marker and remains DL-only.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Repeated same-GSSI active-call, current-speaker, and hangtime tests now assert no setup-phase `D-CALL PROCEEDING`/`D-CONNECT`.
  - Tests now assert compact `D-TX GRANTED` plus channel allocation with the actual circuit timeslot, usage marker, and UL/DL direction.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs repeated_group_u_setup --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 118 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 46 passed.
- `git diff --check` -> pass.

Build/deploy:

- Commit: `33ef3ca fix: treat repeated group setup as floor control`.
- Deployed with the one-shot local script:
  - `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Remote deployed binary SHA-256:
  - `06b913c5f3254330596034b5a821cb874b3bc20a694932f7376a75df5e831a09`
- Restarted test BS with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Live evidence after deploy:

- Startup banner reports `Build: v0.1.55-33ef3ca8`.
- `2260082`, `2260616`, and `2260618` registered and affiliated to `226333`.
- No complete post-deploy group PTT attempt was present in the checked log sample yet, so terminal-side RF validation remains required.

Next live validation:

- Test BS is already running `v0.1.55-33ef3ca8`.
- Validate alternating group PTT on GSSI `226333` with `2260082`, `2260616`, and `2260618`.
- Required field evidence: no terminal `PTT denied`, no BS `NotGranted` unless another MS is truly still transmitting, `UMAC floor granted` follows the active speaker, and audio is intelligible rather than static.

## 2026-06-04 19:47:00 EEST - Patched first-PTT retry caused by missing floor reassert

User symptom:

- Group PTT intervention no longer shows the earlier `Service unavailable`, but first PTT sometimes only takes effect on the second attempt.

Live evidence:

- Logs after `4693056` showed repeated same-GSSI `U-SETUP` from the MS that Nexus-BS already considered the current speaker, for example:
  - `CMCE: mapping repeated U-SETUP ... call_id=5 state=Transmitting`
  - `DConnect { transmission_grant: Granted ... }`
- In that current-speaker path, the existing floor FSM returned `FromCurrentSpeaker` and did not emit a fresh `D-TX GRANTED` or UMAC `FloorGranted`.

Component in simple technical terms:

- CMCE tells the terminal whether it may transmit using `D-TX GRANTED`.
- UMAC needs `FloorGranted` to keep the traffic-channel uplink speaker mapped to the correct ISSI.
- `D-CONNECT Granted` is call setup/connection signalling; for repeated PTT inside an active group call it was not enough for this field behavior.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1.2 covers `D-CONNECT` as group-call setup/through-connect.
- EN 300 392-2 clause 14.5.2.2.1 b) defines `D-TX GRANTED` as the SwMI response that grants, queues, or denies group transmit permission, while MSs remain in `CALL-ACTIVE`.
- The reassert path is a compatibility handling of a repeated same-GSSI `U-SETUP` accepted as existing-call re-entry/floor intent. It is explicitly responsive to the received PDU, not an unsolicited random grant. This is clause-scoped engineering alignment only, not formal ETSI/TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - Added `fsm_group_reassert_current_speaker_floor`.
  - Validates the call exists, is transmitting, and the requester is the current speaker and affiliated to the GSSI.
  - Sends individual `D-TX GRANTED Granted`, group FACCH `D-TX GRANTED GrantedToOtherUser`, and UMAC `FloorGranted`.
  - Resets the local call timeout clock when the floor is reasserted.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Repeated same-GSSI `U-SETUP` from current speaker now uses the reassert path instead of the duplicate `U-TX DEMAND` path that ignored `FromCurrentSpeaker`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_repeated_group_u_setup_from_current_speaker_reasserts_existing_floor`.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs repeated_group_u_setup --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 118 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Build/deploy:

- Commit: `deefa8d fix: reassert group floor on repeated setup`
- Deployed with the new one-shot local script:
  - `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Remote deployed binary SHA-256:
  - `ddbb38afa84973c83b9f727ede434fa571423c7ef94255f9780f23f0513b81b6`
- Restarted test BS with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Live evidence after deploy:

- Startup banner reports `Build: v0.1.55-deefa8d4`.
- `2260082`, `2260616`, and `2260618` registered and affiliated to `226333`.
- Repeated same-GSSI `U-SETUP` from current speaker now emits:
  - individual `D-TX GRANTED (Granted)`;
  - group FACCH `D-TX GRANTED (GrantedToOtherUser)`;
  - UMAC `FloorGranted` on the same `call_id`.
- Concrete post-deploy examples:
  - `19:50:48` `2260618` repeated `U-SETUP` on `call_id=4` -> `D-TX GRANTED Granted` + `UMAC floor granted`.
  - `19:50:55` `2260616` repeated `U-SETUP` on `call_id=4` -> `D-TX GRANTED Granted` + `UMAC floor granted`.
- No `rejecting colliding`, `RequestedServiceNotAvailable`, `Service unavailable`, `PTT denied`, or `Unit Not Attached` appeared in the filtered post-deploy group-call log sample.

Next non-repeating action:

- Operator validates first-try group PTT audio from `2260616` and `2260618`.
- If first-try floor now works but audio remains static/silent, move to UMAC/TCH-S uplink media evidence; do not reopen the CMCE setup-collision hypothesis.

## 2026-06-04 19:37:30 EEST - Patched repeated group PTT U-SETUP service-unavailable path

User symptom:

- Repeated PTT in group call reports `Service unavailable`.
- Live logs showed `CMCE: rejecting colliding U-SETUP ... active gssi=226333` followed by `DRelease { call_identifier: 0, disconnect_cause: RequestedServiceNotAvailable }`.

Components in simple technical terms:

- CMCE is call control. It owns `U-SETUP`, `D-CONNECT`, `D-SETUP`, `U-TX DEMAND`, `D-TX GRANTED`, and group-call release decisions.
- UMAC is the lower MAC scheduler. It does not decide call policy; it receives CMCE `FloorGranted`/`FloorReleased` commands and maps the current speaker onto the traffic channel.
- Hangtime is Nexus-BS local call retention after a speaker releases PTT. The call is still maintained so the next PTT can reuse the same `call_id` and traffic circuit.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1.2 defines group call setup, `D-CALL PROCEEDING`, `D-CONNECT`, and the SwMI call identifier used for subsequent PDUs.
- EN 300 392-2 clause 14.5.2.1.3 covers same-group setup collision handling and does not require creating a parallel group call.
- EN 300 392-2 clause 14.5.2.2.1 says the SwMI controls group transmit permission with `U-TX DEMAND` / `D-TX GRANTED`; queued/not-granted floor responses keep the MS in `CALL-ACTIVE`.
- EN 300 392-2 clause 14.5.2.3.2 uses `D-RELEASE` when the SwMI cannot support a call/request. A compatible repeated setup for the same active GSSI is now not treated as that failure case.
- This is clause-scoped engineering alignment, not formal ETSI/TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Same-GSSI `U-SETUP` while a group call is active or in hangtime no longer receives dummy-call `D-RELEASE RequestedServiceNotAvailable`.
  - If the call is already pending release, the old rejection remains; a releasing traffic circuit must not be reused or duplicated.
  - The repeated requester must be affiliated to the GSSI and request a compatible service before rejoining the existing call.
  - Nexus-BS responds with existing-call `D-CALL PROCEEDING` and `D-CONNECT`, including the active traffic allocation and existing `call_id`.
  - The repeated setup is then routed through the existing group floor FSM:
    - while another MS is transmitting, the requester receives `D-TX GRANTED RequestQueued`;
    - during hangtime, the requester receives floor grant on the existing call and UMAC gets one `FloorGranted`;
    - no second group circuit is allocated.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Replaced the old regression that expected service-unavailable rejection.
  - Added active-speaker coverage: no `D-RELEASE`, no second `D-SETUP`, no second UMAC open, queued floor response on the active call id.
  - Added hangtime coverage: no `D-RELEASE`, no second circuit, requester gets floor on the active call id.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs repeated_group_u_setup --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 117 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 46 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 9 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Build/deploy:

- Commit: `4693056 fix: reuse active group call for repeated setup`
- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Local and remote deployed binary SHA-256:
  - `0c9f655795931d9dfd924e40b6f16b63ff8c58baa5b2b35abd052e681bbc3eaa`
- Deployed direct over `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`.
- No binary backup was created.
- Restarted test BS with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Live evidence after deploy:

- Startup banner reports `Build: v0.1.55-46930568`.
- MM armed restart recovery for cached/configured ISSIs `{2260082, 2260616, 2260618}`.
- `2260618` registered/affiliated to `226333` at about 4 s after restart.
- `2260616` registered/affiliated to `226333` at about 5 s after restart.
- `2260082` registered/affiliated to `226333` at about 6 s after restart, then repeated its attach/affiliation once.
- Initial post-restart log filter found no `rejecting colliding`, `RequestedServiceNotAvailable`, `Service unavailable`, `PTT denied`, or `Unit Not Attached`.
- No live PTT attempt had occurred in the sampled post-restart log yet, so the field validation step remains required.

Next non-repeating actions:

- Restart test BS and validate group PTT on `226333`, especially repeated PTT from `2260616` and `2260082` during active speaker and hangtime.
- If static audio persists after this service-unavailable fix, the next non-repeating investigation is UMAC/TCH-S voice-direction evidence, not another CMCE setup-collision hypothesis.

## 2026-06-04 19:28:03 EEST - Deployed MM attach-confirmation hardening to test BS

User symptom:

- After BS restart, terminals can show `Unit Not Attached` even while Nexus-BS is rebuilding its local MM/CMCE state.

Components in simple technical terms:

- MM (Mobility Management) owns terminal registration, location update, group affiliation state, and restart recovery.
- MLE/LLC carry MM downlink PDUs over the air and report whether acknowledged transfers succeeded or failed.
- CMCE consumes MM register/affiliate updates so group/private call control knows which ISSIs are usable listeners.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4 permits SwMI-initiated registration at any time using `D-LOCATION UPDATE COMMAND`, including a group identity report request.
- EN 300 392-2 clause 16.4.4 also permits/defines the `U-LOCATION UPDATE DEMAND` response to that command.
- EN 300 392-2 clause 16.4.4 says an MS that supports listed extended capabilities includes the extended capabilities IE in `U-LOCATION UPDATE DEMAND`; Nexus-BS now accepts that IE as non-fatal but does not yet act on the capability bits.
- LLC/MLE transfer reports are local SAP evidence, not over-air certification. No formal ETSI/TETRA certification is claimed.

Patch implemented:

- Commit: `c38b13b fix: harden MM restart attach confirmation`
- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Restart recovery now sends the first probe after 1 s, spaces cached ISSIs by 1 s, retries every 2 s, and keeps the same long total window with 150 attempts.
  - `U-LOCATION UPDATE DEMAND.extended_capabilities` is accepted/logged instead of rejecting registration.
  - `D-LOCATION UPDATE ACCEPT` now uses a tracked local MLE handle.
  - If MLE/LLC reports `FAILED_TRANSFER` for that accept, MM treats the registration as unconfirmed, fails open to `StayAlive`, withdraws shared CMCE registration/affiliation, and sends a fresh `D-LOCATION UPDATE COMMAND(group identity report=1)`.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added coverage for standards-permitted `extended_capabilities`.
  - Added coverage for failed `D-LOCATION UPDATE ACCEPT` transfer causing MM to re-probe instead of leaving BS/MS state split.

Verification:

- `cargo test -p tetra-entities --test test_mm_bs failed_location_update_accept --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 9 passed.
- `cargo test -p tetra-entities --test test_mm_bs extended_capabilities --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 112 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Build/deploy:

- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Local and remote test binary SHA-256:
  - `80f72b4d226c5da7e83e42d525188848a80adb02d8bf094a2f1758cfe690e01c`
- Deployed direct over `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`.
- No binary backup was created.
- Restarted test BS with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Live evidence after deploy:

- Startup banner reports `Build: v0.1.55-c38b13ba`.
- MM armed restart recovery for cached/configured ISSIs `{2260082, 2260616, 2260618}`.
- `2260616` registered/affiliated to `226333` at about 4 s after restart.
- `2260082` registered/affiliated to `226333` at about 5 s after restart.
- `2260618` needed repeated 2 s recovery probes and then registered/affiliated to `226333` at about 22 s after restart.
- No `D-LOCATION UPDATE ACCEPT failed transfer` reprobe occurred in this sampled live run; the new failed-accept path remains unit-tested.
- No literal `Unit Not Attached` appeared in `nexus-bs.log` or `control.log`.

Residual observations:

- `2260618` is still slower to answer restart recovery than the other terminals; this now retries more frequently and remains long-lived, but operator screen verdict is still required.
- `2260616` later hit BS-initiated EG3 T352 timeout and stayed `StayAlive`, which is safe for PTT validation.
- Next non-repeating action is live group/private PTT validation, not another MM hypothesis unless a terminal remains unattached after the recovery window.

## 2026-06-04 13:17:51 EEST - Deployed long-lived MM restart recovery to test BS

Deployed commit:

- `4588590 fix: extend MM restart recovery`

Build/deploy:

- Built locally only with the Nexus-BS AArch64 SoapySDR sysroot command.
- Local/remote deployed binary SHA-256:
  - `dd061b3bf5169c5ff0ff45a5505cc0d3dca1a7b30f21584f39a74a8ea1722bda`
- Deployed direct over `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`.
- No binary backup was created.
- Restarted test BS with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`.
- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`

Live evidence:

- Startup banner reports `Build: v0.1.55-4588590b`.
- MM armed restart recovery for cached/configured ISSIs `{2260082, 2260616, 2260618}`.
- `2260082` self re-registered and CMCE received:
  - `subscriber register issi=2260082`
  - `subscriber affiliate issi=2260082 groups=[226333]`
- `2260616` self re-registered and CMCE received:
  - `subscriber register issi=2260616`
  - `subscriber affiliate issi=2260616 groups=[226333]`
- `2260618` missed early recovery commands, then the new 5 s long-lived retry sequence ran:
  - attempts `1/60`, `2/60`, `3/60`, `4/60`
  - then `2260618` re-registered and CMCE received `subscriber affiliate issi=2260618 groups=[226333]`.
- After waiting past the prior 60 s group-report window, no `solicited group report window expired` or `re-requesting group report` appeared after the group-bearing `U-LOCATION UPDATE DEMAND`.
- No literal `Unit Not Attached` appeared in `nexus-bs.log` or `control.log`.

Residual observations:

- BS-initiated EG3 assignment still timed out for `2260082` and `2260616`; fail-safe behavior kept `StayAlive`.
- That EG fallback is not a blocker for group/private PTT validation, but EG3 default should be revisited after the voice path is stable because terminals are not confirming the BS-initiated EG assignment in this live run.

Next non-repeating actions:

- Test group PTT on `226333` in sequence:
  - `2260616` PTT/speak/release.
  - `2260082` PTT/speak/release.
  - `2260618` PTT/speak/release if available.
- Capture logs for `U-TX DEMAND`, `D-TX GRANTED`, `FloorGranted`, `UMAC floor granted`, `rx_blk_traffic`, `TCH/S`, `UL inactivity`, `PTT denied`, and `NotGranted`.
- Then repeat private simplex `2260082 <-> 2260616`.
- Do not re-open the pure UMAC bit-copy hypothesis unless new evidence contradicts the existing component tests.

## 2026-06-04 13:09:59 EEST - MM restart recovery made long-lived and group-report pending cleared

User symptom:

- After BS restart, some terminals still show `Unit Not Attached` during the recovery window.

Component in simple terms:

- MM restart recovery is the BS-side procedure that asks locally known/cached terminals to perform location update again after the Nexus-BS process restarts.
- LLC carries those MM commands with ACK/retry. If recovery commands are retried too aggressively during SDR startup, they can exhaust while the radio/air path is still settling.
- The solicited group-report window is a local MM bookkeeping window opened after `D-LOCATION-UPDATE-COMMAND(group identity report=1)`.

Live evidence from `chris@192.168.1.179` before this patch:

- Running binary was current deployed build `Nexus-BS v0.1.55`, `Build: v0.1.55-acbba6d5`.
- Recovery cache `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` contained `2260082`, `2260616`, and `2260618`.
- Startup log:
  - `2260082` self re-registered and affiliated to `226333` almost immediately.
  - `2260616` needed repeated restart-recovery `D-LOCATION-UPDATE-COMMAND` attempts before registration/affiliation recovered.
  - `2260618` recovered later, but the local solicited group-report window still expired even though its later `U-LOCATION UPDATE DEMAND` carried group `226333`.
- Conclusion: current cache/recovery path works eventually, but early retries are too compressed and one local pending flag remains misleading after a late group-bearing location update.

ETSI scope:

- EN 300 392-2 clause 16.4.4 / figure 16.6 permits SwMI-initiated registration using `D-LOCATION UPDATE COMMAND`, including group identity report request.
- EN 300 392-2 clauses 16.9.3.4, 16.10.17, 16.10.23, and 16.10.35a define the accepted location-update and group identity response handling used by the existing MM path.
- This patch does not change standardized PDU fields. It changes local Nexus-BS retry cadence and local pending-window cleanup only.
- No formal certification claim is made.

Patch:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Recovery command spacing changed from 18 timeslots to `18 * 4` timeslots (about 1 s).
  - Recovery retry cadence changed from `18 * 4` timeslots (about 1 s) to `5 * 18 * 4` timeslots (about 5 s).
  - Max recovery attempts changed from 5 to 60, making recovery long-lived for several minutes instead of giving up in a few seconds.
  - A group-bearing `U-LOCATION UPDATE DEMAND` now clears the local solicited group-report window even when the optional group-report-response IE is absent.
  - If a terminal recovers registration but still has no attached groups when the solicited group-report window expires, MM re-requests the group report with `D-LOCATION UPDATE COMMAND` instead of leaving CMCE with no group listener.
  - Added a debug-only MM test accessor for the solicited group-report pending flag.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Updated restart recovery pacing assertions for 1 s inter-ISSI spacing.
  - Added assertion that reported group identities complete the solicited restart group-report window.
  - Added `test_restart_recovery_re_requests_group_report_when_recovered_without_groups`.
  - Added `test_restart_recovery_retries_are_long_lived_and_paced`.

Verification:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 8 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 110 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating actions:

- Commit this MM patch.
- Build locally for AArch64 with the Nexus-BS SoapySDR sysroot command.
- Deploy direct over `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; do not compile on the Pi and do not create binary backups.
- Restart test BS and verify:
  - build hash changed from `acbba6d5`;
  - recovery attempts are spaced about 1 s per ISSI and about 5 s per retry;
  - `2260082`, `2260616`, and `2260618` all re-register/re-affiliate to `226333`;
  - no stale `solicited group report window expired` appears after a group-bearing location update already restored the terminal.

## 2026-06-04 13:00:32 EEST - MM restart recovery pacing for post-restart Unit Not Attached window

User symptom:

- After BS restart, terminals can show `Unit Not Attached`.

Component in simple terms:

- MM is Mobility Management. It owns terminal registration/location update and group affiliation rebuild after restart.
- Restart recovery is the MM procedure that asks still-camped radios to re-register after the Nexus-BS process has lost in-memory subscriber state.

Live evidence from `chris@192.168.1.179`:

- Active process: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`.
- Recovery cache exists at `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` and contains `2260082`, `2260616`, `2260618`.
- At `12:49:26` Nexus-BS armed restart recovery for all three ISSIs and immediately sent `D-LOCATION-UPDATE-COMMAND` to all three in the same startup window.
- The live log also showed startup PHY timing loss/late TX warnings, then LLC retransmission exhaustion for some initial recovery commands.
- All three terminals later recovered:
  - `2260082`: `D-LOCATION UPDATE ACCEPT`, CMCE Register, CMCE Affiliate `[226333]`, solicited group report accepted.
  - `2260618`: `DemandLocationUpdating`, `D-LOCATION UPDATE ACCEPT`, EG3 allocation, CMCE Register/Affiliate `[226333]`.
  - `2260616`: slower recovery after repeated `D-LOCATION-UPDATE-COMMAND`, then Register/Affiliate `[226333]`.
- No literal `Unit Not Attached` appears in `nexus-bs.log` or `control.log`; the label is likely the radio/Brew symptom during the temporary not-yet-recovered MM state.

ETSI scope:

- EN 300 392-2 clause 16.4.4 / figure 16.6 permits infrastructure-initiated registration using `D-LOCATION UPDATE COMMAND`, optionally with group report request.
- This patch does not change the standardized PDU or registration semantics. It changes only local Nexus-BS retry timing so acknowledged MM commands are not blasted during SDR startup.
- No formal certification claim is made.

Patch:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Added a 2-second TDMA startup guard before restart recovery probes.
  - Added 250 ms inter-ISSI spacing for cached/configured recovery ISSIs.
  - Added one-command-per-tick deferral so multiple due probes cannot burst together.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Updated restart recovery tests to assert no command during startup guard.
  - Asserted cached/configured ISSIs are paced instead of sent in the same tick.
  - Preserved existing tests proving that actual registration/group/EG state is rebuilt only from terminal responses.

Verification:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 6 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 108 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating actions:

- Commit, build locally for AArch64, deploy direct over `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`.
- Restart test BS and verify post-restart logs show spaced recovery attempts, not three immediate commands at `t=0`.
- After this MM patch is live, return to CMCE group queued `U-TX CEASED` withdrawal and live group/private audio validation. Do not reopen the solved pure UMAC bit-copy hypothesis.

## 2026-06-04 10:22:33 EEST - PM orchestration checkpoint

Goal in force: clause-scoped ETSI EN 300 392-2 hardening for a robust Nexus-BS TETRA stack. The target remains practical engineering evidence for group call, private call simplex/duplex, SDS/status, MM attach/affiliation persistence, scan/group retention, WAP MVP, and long-running BS stability. This is not a formal certification claim; formal certification requires official conformance evidence.

Mandatory law reloaded before this checkpoint:

- `/Users/ctermure/.codex/memories/tetra-etsi-compliance-law.md`
- `/Users/ctermure/.codex/memories/flowstation-tetra-eg-swmi-resume-2026-06-02.md`
- `/Users/ctermure/.codex/memories/flowstation-aarch64-soapysdr-build.md`

Repo status:

- Branch: `nexus-bs-v0.1.55`
- Latest commit: `b18ed13 fix: allocate unique MLE handles for CMCE unitdata`
- Current dirty files:
  - `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - `crates/tetra-entities/tests/test_cmce_bs.rs`
  - `crates/tetra-entities/tests/test_umac_bs.rs`

Live critical defects still open:

- Private simplex call between `2260082` and `2260616` grants PTT but one direction produces static.
- Group call has the same static symptom when the other subscriber transmits.
- This points to the shared voice path, not only to private-call CMCE: CMCE floor grant -> UMAC circuit/timeslot routing -> LMAC TCH/S voice encode/decode.
- BS restart/long-run robustness remains a hard requirement: terminals must re-register/re-affiliate, groups must be retained coherently, and scan behavior must not be broken.

Component map in simple technical terms:

- CMCE: call control. It decides setup, floor/PTT permission, release, private/group call state.
- UMAC/MAC: radio scheduler. It maps signalling and voice to timeslots, STCH/FACCH/TCH, grants, and random access responses.
- LMAC: low MAC/physical framing. It decodes and encodes bursts, including TCH/S voice bits.
- MLE/LLC: reliable/unreliable signalling delivery, handles, ACK/report timers.
- MM: registration, attach, group affiliation, energy saving/EG behavior.
- SDS/status: short data and status messages over CMCE/SDS subentities.
- WAP: packet/page delivery track for the Nexus-BS terminal browser MVP.

Agent roster:

- Project Manager: `019e911b-1c4b-7f02-a72f-2bdf280d6c35` (`Aquinas the 3rd`)
  - Owns execution order, timeline updates, anti-loop discipline, and ETSI law reminders.
- Review Agent: `019e911b-1cad-75a2-8032-1bd9fe865e83` (`Heisenberg the 3rd`)
  - Reviews current diffs for regressions, missing tests, and unsupported compliance claims.
- Architecture Agent - voice path: `019e911b-1d27-7b91-a06c-c8393037b7e7` (`Arendt the 3rd`)
  - Traces CMCE floor -> UMAC routing -> LMAC TCH/S for the static-audio defect.
- QA Agent: `019e9134-b662-7ac0-a27f-dd8446c1c03b` (`Maxwell the 3rd`)
  - Maintains BASIC 24x7 test matrix: group, private simplex/duplex, SDS, attach, affiliation, scan, restart.
- MAC/UMAC Scheduling Architect: `019e9134-cc83-7793-a243-e7e1428e2587` (`Ohm the 3rd`)
  - Focuses on TCH/S bit preservation, timeslot routing, FACCH/STCH stealing, and grants.
- MM/SDS Robustness Architect: `019e9134-e2c2-7cb1-a626-0f86672f87f6` (`Herschel the 3rd`)
  - Focuses on MM persistence, EG risk, group affiliation, SDS/status delivery.

Current WIP:

- CMCE pending group release timer hardening is implemented locally:
  - `check_call_timeout_expiry` skips calls already in `pending_group_releases`.
  - `check_hangtime_expiry` skips calls already in `pending_group_releases`.
  - Test extension asserts no repeated `D-RELEASE` and no early `NetworkCallEnd` while reporter completion is pending.
  - ETSI anchor: EN 300 392-2 clause 14.5.2.3 group call release. The patch prevents an internal retry storm; it does not change the standardized group release primitive.
- UMAC voice tests are partially prepared:
  - Helpers exist for private-call circuit opening, UL voice injection, ACELP test bits, and DL TCH/S bit collection.
  - The actual group/private voice-routing tests still need to be written before claiming this path is protected.

Verification already reported for the current CMCE timer WIP:

- `cargo fmt -p tetra-entities`
- `cargo test -p tetra-entities --test test_cmce_bs network_group_hangtime_release --locked`
- `cargo test -p tetra-entities --test test_cmce_bs --locked`

Do not repeat those as proof for UMAC voice; they only cover CMCE release behavior.

Immediate next execution order:

1. Finish UMAC TCH/S voice-routing tests in `crates/tetra-entities/tests/test_umac_bs.rs`.
   - Group/local path: UL TCH/S bits on the active speaker timeslot must be scheduled back as DL TCH/S without bit corruption.
   - Private simplex cross-route: UL TCH/S from one party must be transmitted on the peer timeslot, not the wrong slot, without bit corruption.
   - Duplex path must be added after simplex routing is stable.
2. If UMAC tests fail, fix UMAC/LMAC routing or ACELP bit packing before any deploy.
3. Run focused local tests only, one Cargo command at a time:
   - `cargo fmt -p tetra-entities`
   - `cargo test -p tetra-entities --test test_umac_bs --locked`
   - `cargo test -p tetra-entities --test test_cmce_bs --locked`
   - `git diff --check`
4. Commit the narrow verified patch.
5. Build locally only with the Nexus-BS AArch64 command from build memory.
6. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs` on `chris@192.168.1.179`; do not compile on the Pi and do not create binary backups.
7. Restart the test BS and collect bounded logs around `2260082`, `2260616`, `2260618`, `U-TX DEMAND`, `D-TX GRANTED`, `TCH`, `UL inactivity`, `TMA-REPORT`, and `D-RELEASE`.

ETSI anchors for the next patches:

- Private simplex/floor: EN 300 392-2 clause 14.5.1.2.1.
- Group floor and calls: EN 300 392-2 clauses 14.5.2.1.3 and 14.5.2.2.1.
- Group release: EN 300 392-2 clause 14.5.2.3.
- MAC traffic/channel allocation/FACCH/STCH/TCH: EN 300 392-2 clauses 23.5, 23.5.4.1, 23.5.4.2.
- LLC/MLE report handles and ACK timing: EN 300 392-2 clauses 20.4.1.1.3 and 22.3.2.4.1.
- Energy economy/EG: EN 300 392-2 clauses 16.7.1, 16.10.9, 16.10.10, 16.10.35a, 22.3.2.3, 23.5.2.2.7, 23.7.6, and timer T.210.
- SDS/status: EN 300 392-2 clause 13.2 and U/D-STATUS tables 14.27 and 14.14.

Anti-loop rules:

- Every protocol change starts by naming the exact clause scope and the exact runtime symptom or failing test.
- Every hypothesis must end in one of three states: proven by test/log, disproven by test/log, or parked with a concrete missing artifact.
- Do not reopen solved MLE handle ambiguity unless `req_handle=0 ambiguous` reappears after `b18ed13`.
- Do not treat CMCE release tests as proof of voice path correctness.
- Do not deploy a protocol patch that has only dashboard/config tests.
- Do not claim `100% certified` in logs, commits, or user output. Use `clause-scoped ETSI-aligned` until official conformance evidence exists.
- Encryption TODOs are tracked but not the current focus.
- `call_preemptive` / transmission interruption must stay default off unless explicitly enabled in config.
- Energy saving default changes must be checked against the full EG scheduling path; `StayAlive` remains the safest baseline unless real EG is configured and verified.

Open project tracks after the voice blocker:

- Private duplex call setup and traffic tests.
- SDS/status field-level and routing tests.
- MM restart recovery: terminal reattach/re-affiliation and group retention evidence.
- Scan behavior validation with configured groups.
- WAP MVP: terminal browser home page must deliver the Nexus-BS greeting page; dynamic/flashing/color behavior is a UI/application layer feature and must not mask packet delivery failures.
- 24x7 soak harness: bounded log rotation, health metrics, terminal registry checks, call/SDS periodic probes.

## 2026-06-04 10:27:26 EEST - UMAC voice contract tests added

Changed files:

- `crates/tetra-entities/tests/test_umac_bs.rs`
- `timeline.md`

What was added:

- `test_group_ul_voice_loopback_preserves_tch_s_bits`
- `test_private_simplex_ul_voice_loopback_preserves_tch_s_bits`
- `test_private_duplex_ul_voice_cross_route_preserves_tch_s_bits`

Result:

- UMAC preserves the 274 ACELP TCH/S bits in the pure component model.
- Group/local loopback, private simplex same-channel loopback, and private duplex `peer_ts` cross-route all pass.
- The helper now checks both `blk1` and `blk2`, so FACCH/stealing placement does not hide a TCH/S frame in the second half-slot.

Verification:

- `cargo fmt -p tetra-entities`
- `cargo test -p tetra-entities --test test_umac_bs voice --locked` -> 3 passed
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 42 passed
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 115 passed
- `git diff --check`

Updated conclusion for static-audio defect:

- Do not keep looping on the simple UMAC bit-copy hypothesis; it is now covered by tests.
- Next investigation must target the live path:
  - actual CMCE circuit allocation used by radios `2260082` and `2260616`;
  - whether private simplex live call is opened as same-channel loopback or mistakenly as SwMI/media-suppressed path;
  - FACCH/STCH stealing around `D-TX GRANTED` and `D-TX CEASED`;
  - LMAC TCH/S encode/decode with real burst framing;
  - RF/PHY direction-specific decode quality.

## 2026-06-04 10:29:20 EEST - Agent review incorporated

Agent input:

- QA Agent produced a BASIC 24x7 matrix for group call, private simplex, private duplex, SDS/status/WAP, attach/affiliation persistence, scan/group retention, and BS restart recovery.
- Review Agent found one real coverage gap: the CMCE timer patch changed both hangtime and call-timeout paths, but only hangtime duplicate suppression was tested.
- MM/SDS Architect flagged restart recovery as ISSI-cache driven; active GSSI affiliation after restart must be rebuilt from terminal group reports, not assumed from disk as truth.

Action taken:

- Extended `test_network_group_call_timeout_reports_network_end_after_expiry_release_delivery`.
- After timeout-driven `D-RELEASE` enters `pending_group_releases`, two additional timer ticks now assert:
  - no duplicate `D-RELEASE`;
  - no early `NetworkCallEnd`;
  - no early UMAC traffic-circuit close.

Verification after agent review:

- `cargo fmt -p tetra-entities`
- `cargo test -p tetra-entities --test test_cmce_bs test_network_group_call_timeout_reports_network_end_after_expiry_release_delivery --locked` -> 1 passed
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 115 passed
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 42 passed
- `git diff --check`

Next non-repeating execution:

- Commit this narrow batch.
- Build/deploy to the Pi test directory.
- Live-log focus must not repeat pure UMAC bit-copy tests; inspect real call circuit source:
  - whether private simplex is opened with `CircuitDlMediaSource::LocalLoopback`;
  - whether live CMCE sends the expected `CallControl::Open`/`FloorGranted` sequence for both `2260082` and `2260616`;
  - whether LMAC reports valid TCH/S uplink frames after each `D-TX GRANTED`;
  - whether static appears only when FACCH stealing occurs near speech start.

## 2026-06-04 10:34:05 EEST - Built and deployed test BS

Commit deployed:

- `21f2b4c test: track CMCE release timers and UMAC voice routing`

Local build:

- Built locally on macOS only.
- Command used: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Output binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs` (`11M`).

Remote deploy:

- Host: `chris@192.168.1.179`
- Target path: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- Deployed direct over the testing binary; no binary backup was created.
- Started from `/home/chris/nexus-bs-v0.1.55-test` with `bin/nexus-bs config.live.toml`.
- Active process after wrapper cleanup: `15161 bin/nexus-bs config.live.toml`
- Dashboard listens on `0.0.0.0:8080`.

Post-deploy observations:

- Terminals re-registered/re-affiliated shortly after restart:
  - `2260616` registered and affiliated to `[226333]`, RSSI about `-26 dBFS`.
  - `2260082` registered and affiliated to `[226333]`, RSSI about `-26 dBFS`.
  - `2260618` registered and affiliated to `[226333]`, RSSI about `-41 dBFS`, requested `Eg1`, BS allocated `Eg3`.
- Dashboard root returned HTML.
- After `10:31:00` there were no matches for:
  - `req_handle=0 ambiguous`
  - `Hangtime expired`
  - `group release already pending`
  - `D-RELEASE`
  - `UL inactivity`
  - `U-TX DEMAND`
  - `D-TX GRANTED`
  - `TCH`

Interpretation:

- Restart recovery did bring the observed terminals back after the new deploy.
- No post-deploy PTT test has run yet, so the static-audio defect is not proven fixed.
- Next live action: trigger private simplex and group PTT again, then inspect only post-deploy logs for CMCE `CallControl::Open`/`FloorGranted`, UMAC traffic mode, LMAC TCH/S uplink indications, and any FACCH stealing around speech start.

## 2026-06-04 10:42:58 EEST - PM agent orchestration reloaded

User directive:

- Add a Project Manager agent that orchestrates and delegates the work.
- Split agents into review, architecture, and QA responsibilities.
- Keep execution status in `timeline.md` so the next resume knows exactly what was done and what must happen next.
- Avoid loops and repeated work.
- Reload the ETSI law/status/project log before changing protocol behavior.

Law/status reload:

- Reloaded `/Users/ctermure/.codex/memories/tetra-etsi-compliance-law.md`.
- Reloaded `/Users/ctermure/.codex/memories/flowstation-tetra-eg-swmi-resume-2026-06-02.md`.
- Active goal remains clause-scoped ETSI EN 300 392-2 hardening. Do not claim formal certification without official conformance evidence.

Current repo state:

- Workdir: `/Users/ctermure/Work/basestion`.
- Branch: `nexus-bs-v0.1.55`.
- Latest commit observed: `0031716 docs: record Nexus-BS test deployment checkpoint`.
- Dirty file observed: `crates/tetra-entities/tests/test_mm_bs.rs`.
- Dirty test adds restart-recovery coverage for terminal `2260618`, group `226333`, DemandLocationUpdating, group affiliation rebuild, and EG3 assignment after restart recovery.

Project Manager setup:

- New spawn was attempted, but the agent thread limit was already reached.
- Existing agent `019e911b-1c4b-7f02-a72f-2bdf280d6c35` (`Aquinas the 3rd`) is now the Project Manager.
- PM owns execution order, anti-loop discipline, timeline handoff quality, and ETSI-law reminders.
- PM must not edit protocol code directly unless explicitly re-tasked; it reports integration guidance back to the main worker.

Agent role split:

- PM / orchestration: `019e911b-1c4b-7f02-a72f-2bdf280d6c35` (`Aquinas the 3rd`).
- Review agent: `019e911b-1cad-75a2-8032-1bd9fe865e83` (`Heisenberg the 3rd`).
- Architecture agent, voice path: `019e911b-1d27-7b91-a06c-c8393037b7e7` (`Arendt the 3rd`).
- QA agent: `019e9134-b662-7ac0-a27f-dd8446c1c03b` (`Maxwell the 3rd`).
- MAC/UMAC scheduling architect: `019e9134-cc83-7793-a243-e7e1428e2587` (`Ohm the 3rd`).
- MM/SDS robustness architect: `019e9134-e2c2-7cb1-a626-0f86672f87f6` (`Herschel the 3rd`).

Simple component meanings for this phase:

- PM: keeps the work ordered and makes sure each next step has evidence.
- Review: catches regressions, missing tests, and unsupported compliance claims.
- Architecture voice path: traces why live speech becomes static between CMCE, UMAC, LMAC, and PHY.
- QA: defines the proof matrix for BASIC functionality and 24x7 stability.
- MAC/UMAC: controls grants, timeslots, TCH/S traffic routing, FACCH/STCH stealing, and EG scheduling.
- MM/SDS: controls registration, affiliation, restart recovery, EG behavior, SDS/status, and packet/WAP dependencies.

Delegated tasks now in flight:

- PM: read law memories, `timeline.md`, and git state; return a non-repeating execution plan and log text.
- Review: inspect current diff and timeline for regressions/missing tests, especially the MM restart-recovery test and unsupported certification wording.
- Voice architecture: trace the static-audio defect in live private/group calls without repeating the already-passed pure UMAC bit-copy tests.
- QA: produce the next BASIC validation matrix for group call, private simplex/duplex, SDS/status, WAP, restart recovery, scan/group retention, and soak.
- MAC/UMAC architecture: identify next checks for traffic grants, TCH/S routing, FACCH/STCH stealing, EG/T.210 interactions, and PTT denial.
- MM/SDS architecture: review restart recovery, group rebuild, EG3 behavior, SDS/status routing, and WAP packet dependency.

Anti-loop state:

- Do not repeat pure UMAC TCH/S bit-preservation tests as the main static-audio hypothesis; those component tests already passed.
- Do not treat CMCE group release timer tests as proof of live voice correctness.
- Do not assume restart recovery is complete only from a cache entry. It must be rebuilt from confirmed terminal responses and group reports.
- Do not change encryption now; it is tracked but not current focus.
- Keep `call_preemptive` default off.
- Keep every protocol patch clause-scoped to EN 300 392-2 and backed by focused tests/logs.

Next execution order:

1. Finish and verify the dirty MM restart-recovery test in `crates/tetra-entities/tests/test_mm_bs.rs`.
2. Run focused local verification:
   - `cargo fmt -p tetra-entities`
   - `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked`
   - `cargo test -p tetra-entities --test test_mm_bs --locked`
   - `git diff --check`
3. Integrate agent feedback into this timeline before further protocol changes.
4. For the static live-audio defect, inspect post-deploy logs only around actual PTT attempts for `2260082`, `2260616`, and group `226333`.
5. If logs show wrong CMCE circuit source or floor state, patch CMCE with clause scope 14.5.1/14.5.2.
6. If logs show correct CMCE but bad TCH/S framing or slot routing, patch UMAC/LMAC with clause scope 23.5/23.5.4.
7. If logs show RF/PHY direction-specific decode failure, isolate PHY quality and do not mask it with CMCE/MAC guesses.
8. Only after local tests pass, build locally and deploy direct to testing on `chris@192.168.1.179`; never compile on the Pi and do not create binary backups.

Evidence required before calling BASIC paths robust:

- Private simplex: both `2260082 -> 2260616` and `2260616 -> 2260082` pass with intelligible speech and matching floor logs.
- Group call: at least two radios can alternately PTT on group `226333` with intelligible speech and no stale floor state.
- Private duplex: setup, media direction, release, and fallback behavior tested separately from simplex.
- SDS/status: ISSI and GSSI routing tested with expected L2 service.
- WAP: terminal browser reaches the Nexus-BS home page and receives the configured greeting page.
- Restart recovery: terminals re-register/re-affiliate after BS restart without fabricated stale state.
- Scan/group retention: terminals keep usable group state after restart and during normal idle/scan cycles.
- 24x7 stability: log rotation, subscriber registry health, periodic call/SDS/WAP probes, and bounded resource growth are measured.

## 2026-06-04 10:44:34 EEST - MM restart recovery WIP verified

PM feedback integrated:

- The PM confirmed that `crates/tetra-entities/tests/test_mm_bs.rs` is the only dirty protocol test file.
- The dirty WIP is the correct next item because it covers restart recovery from a cache seed without fabricating stale registration/group/EG state.
- The static-audio blocker still needs fresh live PTT logs; pure UMAC bit-copy tests must not be repeated as the main hypothesis.
- MAC/UMAC `generate_default_blks` frame-18 filler is a parked candidate, not the active patch, until the current MM WIP is resolved or explicitly assigned.

MM component meaning in this patch:

- MM is Mobility Management. It handles terminal registration, location update, group affiliation, detach, and energy economy negotiation.
- This test checks that after a BS restart, a cached terminal is only recovered after the terminal responds with a real `U-LOCATION UPDATE DEMAND`.
- It also checks that the BS rebuilds group affiliation and EG3 state from the terminal response instead of trusting stale cache state as active truth.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI may command a terminal to perform location updating.
- EN 300 392-2 clause 16.9.3.4: U-LOCATION UPDATE DEMAND carries the terminal response.
- EN 300 392-2 clause 16.10.23: GroupIdentityLocationAccept carries the group-location accept result.
- EN 300 392-2 clause 16.7.1: energy economy negotiation/assignment is rebuilt from the active exchange.

Verification run locally:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 105 passed.
- `git diff --check` -> pass.

Current conclusion:

- Restart-recovery MM test coverage is now verified locally.
- This does not prove the live static-audio issue is fixed.
- Next non-repeating action remains live post-deploy PTT log collection for private simplex and group call, then patch the first failing layer proven by logs.

## 2026-06-04 10:46:53 EEST - MM/SDS architect review integrated

Agent feedback integrated from MM/SDS robustness architect:

- The restart-recovery test is valid for the clause-scoped objective.
- It needed stronger assertions for the L2 service of `D-LOCATION-UPDATE-COMMAND`.
- It needed explicit `GroupIdentityLocationAccept` assertions for the rebuilt GSSI affiliation.
- Its ETSI comment needed the precise GroupIdentityLocationAccept and EG-specific clauses.

Patch refinement:

- `test_restart_recovery_demand_location_update_restores_affiliation_and_eg3` now asserts that the recovery command uses `Layer2Service::Acknowledged`.
- The test now asserts that `D-LOCATION UPDATE ACCEPT` includes `GroupIdentityLocationAccept`, lists GSSI `226333`, carries attachment lifetime/class-of-usage, and does not encode detachment for that group.
- The ETSI comment now names clauses 16.4.4, 16.9.3.4, 16.10.23, 16.10.35a, 16.7.1, 16.10.9, 16.10.10, and 23.7.6/T.210.
- Added helper `location_update_command_details` for tests that need address, handle, L2 service, and decoded PDU without changing existing helper call sites.

Verification after refinement:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_demand_location_update_restores_affiliation_and_eg3 --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 105 passed.
- `git diff --check` -> pass.

Remaining MM/SDS gaps after this patch:

- Restart recovery with group report complete/empty list is now the next targeted test to verify, proving stale groups are not restored.
- SDS/status ISSI/GSSI routing still needs focused tests and, if needed, patches.
- WAP over SDS Type 4 home page is separate from full SNDCP packet WAP; full WAP/SNDCP remains open unless explicitly implemented.

## 2026-06-04 10:49:30 EEST - QA BASIC/24x7 matrix integrated

QA feedback integrated:

- The immediate MM verification path is complete in this turn.
- UMAC pure voice bit-copy, CMCE timer, and deployed test evidence from earlier timeline entries must not be reused as proof of live audio correctness.
- Static audio and PTT denial require fresh live Pi evidence from real PTT attempts.
- SDS/WAP test names from QA must be checked for existence before running; do not burn time on non-existent filters.

Minimum BASIC pass evidence:

- Private simplex:
  - Test `2260082 -> 2260616`, then `2260616 -> 2260082`.
  - Required evidence: setup/connect, `CallControl::Open`, `D-TX GRANTED`, correct `FloorGranted source_issi`, intelligible audio both ways, no unjustified `PTT denied`, no static.
- Group call:
  - Test alternating PTT on GSSI `226333` with at least two terminals.
  - Required evidence: floor granted to active speaker, group listeners receive the speech path, no stale floor, no static.
- Restart recovery:
  - Local MM test now covers cache seed -> command -> confirmed DemandLocationUpdating -> register/affiliate/EG3.
  - Live test still required after BS restart with observed terminals.
- Group retention and scan:
  - Required evidence: terminal group state is rebuilt from terminal response, remains usable after idle/scan cycles, and PTT works on selected group.
- SDS/status:
  - Required evidence: ISSI route uses acknowledged delivery; GSSI route uses unacknowledged delivery; local status/SDS is delivered to intended target.
- WAP:
  - Required evidence: terminal browser reaches the Nexus-BS greeting page.
  - Current MVP is SDS Type 4/WAP-style page delivery; full WAP over SNDCP packet data remains an open implementation track unless explicitly completed.
- Private duplex:
  - Required evidence: duplex setup/media/release if terminal support exists; otherwise documented simplex fallback without wrong bearer setup.
- Long-run BS robustness:
  - Required evidence: 24h soak with dashboard health, subscriber registry stability, call/SDS/WAP probes, bounded logs/memory, no panic, no D-RELEASE storm, no `req_handle=0 ambiguous`, no unjustified PTT denial.

Recommended soak probes after the live blocker is fixed:

- Every 15 minutes: dashboard health, registry ISSI/GSSI snapshot, error log scan.
- Every 60 minutes: group PTT alternation and private simplex in both directions.
- Every 2 hours: SDS/status plus WAP home-page delivery.
- Once during soak: controlled BS restart followed by re-register/re-affiliate verification.

Current waiting agent feedback:

- Review, voice architecture, and MAC/UMAC architecture agents were still running at this checkpoint.
- Do not block on them for the verified MM test.
- Integrate their findings before the next protocol patch if they return concrete risks or a better live-log plan.

## 2026-06-04 10:52:29 EEST - Review feedback applied to MM restart recovery

Review feedback integrated:

- Tightened clause language: `GroupIdentityLocationAccept` is anchored to EN 300 392-2 clause 16.10.23.
- Kept 16.10.17 out of the restart-recovery group-location accept explanation except where mode=1 attach/detach semantics are explicitly under test elsewhere.
- Confirmed no formal certification claim was introduced; wording remains clause-scoped engineering evidence.

Additional test added:

- `test_restart_recovery_group_report_complete_keeps_groups_empty`
- Scenario: cache-seeded ISSI `2260618` gets `D-LOCATION-UPDATE-COMMAND`, then answers `U-LOCATION UPDATE DEMAND` with group-report-complete/empty list.
- Expected result: BS registers the terminal, does not advertise stale GSSI entries, does not affiliate GSSI `226333`, does not create EG state, keeps recovery cache, and stops further recovery probes.

Verification after review integration:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_group_report_complete_keeps_groups_empty --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 106 passed.
- `git diff --check` -> pass.

MM restart-recovery coverage now includes:

- Cache seed -> recovery command with `Layer2Service::Acknowledged`.
- DemandLocationUpdating response with GSSI report -> register, affiliate, EG3 assignment, no frame-18 EG start.
- DemandLocationUpdating response with complete empty group report -> register only, no stale group affiliation, no EG state.

Next non-repeating execution:

- Do not add more MM restart-recovery tests unless a new specific gap appears.
- Resume live static-audio investigation with post-deploy logs for private simplex and group PTT.
- First live log question: does CMCE open the correct circuit/media source and floor state for both `2260082` and `2260616` before UMAC/LMAC traffic starts?
- Second live log question: if CMCE is correct, does UMAC/LMAC preserve valid TCH/S traffic around `D-TX GRANTED`, FACCH/STCH stealing, and `D-TX CEASED`?

## 2026-06-04 10:57:25 EEST - MM committed and SDS/status/WAP evidence refreshed

Committed local evidence batch:

- Commit: `7f85bc0 test: harden MM restart recovery evidence`
- Scope: MM restart-recovery tests and project timeline only.
- Verification immediately before commit:
  - `cargo fmt -p tetra-entities` -> pass.
  - `cargo test -p tetra-entities --test test_mm_bs --locked` -> 106 passed.
  - `git diff --check` -> pass.

Live BS/log inspection:

- Remote test BS process still running on `chris@192.168.1.179`: `bin/nexus-bs config.live.toml`, pid `15161`.
- Active log: `/home/chris/nexus-bs-v0.1.55-test/nexus-bs.log`.
- No fresh post-checkpoint private/group PTT evidence was found in the current log tail.
- Older log entries contain `U-TX CEASED`, repeated `Hangtime expired`, and soft reattach markers before the current non-repeat checkpoint; do not use those as proof of the current static-audio defect without a fresh PTT attempt.

SDS/status/WAP component meaning:

- SDS is short data service: small user/application data over CMCE.
- STATUS is pre-coded SDS signalling: compact 16-bit status codes.
- WAP MVP currently rides on SDS Type 4/SDS-TL style delivery, not full SNDCP packet WAP.

SDS/status/WAP evidence refreshed:

- ETSI anchors checked locally:
  - EN 300 392-2 clause 13.2: individual/group SDS services.
  - EN 300 392-2 D-STATUS table 14.14 and U-STATUS table 14.27.
  - EN 300 392-2 clause 18.3.5.3.1: layer 2 service selection.
- `cargo test -p tetra-entities --test test_sds_bs --locked` -> 112 passed.
- Passing coverage includes:
  - ISSI D-STATUS uses `Layer2Service::Acknowledged`.
  - GSSI D-STATUS uses `Layer2Service::Unacknowledged`.
  - local group SDS/status routes as GSSI.
  - all-ones broadcast SDS/status uses GSSI and unacknowledged delivery.
  - WAP MVP text variants preserve the requested Nexus-BS message and Type 4 payload budget.

Current conclusion:

- SDS/status/WAP component evidence is current locally.
- Live WAP terminal-browser validation still remains separate from component tests.
- Voice static/PTT denial remains the active live blocker and needs a fresh PTT trace.

## 2026-06-04 11:06:54 EEST - PM/review/architecture/QA orchestration checkpoint

User directive:

- Keep a Project Manager agent responsible for orchestration and delegation.
- Split work across review, architecture, and QA agents.
- Keep `timeline.md` current enough that the next resume knows exactly what has been done and what comes next.
- Reload the ETSI law/status/project log before protocol work.
- Avoid loops, repeated hypotheses, and unsupported certification claims.

Law/status reload completed before this checkpoint:

- `/Users/ctermure/.codex/memories/tetra-etsi-compliance-law.md`
- `/Users/ctermure/.codex/memories/flowstation-tetra-eg-swmi-resume-2026-06-02.md`
- `/Users/ctermure/.codex/memories/flowstation-aarch64-soapysdr-build.md`
- Active goal remains clause-scoped ETSI EN 300 392-2 hardening. Do not claim formal certification without official conformance evidence.

Current repo state:

- Workdir: `/Users/ctermure/Work/basestion`
- Branch: `nexus-bs-v0.1.55`
- HEAD: `3776497 chore: trace live voice circuit routing`
- Worktree: clean
- Latest relevant commits:
  - `3776497 chore: trace live voice circuit routing`
  - `6a13dd1 docs: refresh SDS and live validation evidence`
  - `7f85bc0 test: harden MM restart recovery evidence`

Agent orchestration state:

- A new PM spawn was attempted, but the sub-agent thread limit was already reached.
- Existing agent `019e911b-1c4b-7f02-a72f-2bdf280d6c35` (`Aquinas the 3rd`) remains assigned as Project Manager.
- Review agent: `019e911b-1cad-75a2-8032-1bd9fe865e83` (`Heisenberg the 3rd`).
- Voice-path architecture agent: `019e911b-1d27-7b91-a06c-c8393037b7e7` (`Arendt the 3rd`).
- QA agent: `019e9134-b662-7ac0-a27f-dd8446c1c03b` (`Maxwell the 3rd`).
- MAC/UMAC scheduling architect: `019e9134-cc83-7793-a243-e7e1428e2587` (`Ohm the 3rd`).
- MM/SDS robustness architect: `019e9134-e2c2-7cb1-a626-0f86672f87f6` (`Herschel the 3rd`).

Agent feedback integrated:

- PM: keep execution ordered by evidence. Do not reopen solved UMAC bit-copy, MLE handle ambiguity, or MM restart-recovery coverage unless fresh logs/tests prove a new gap.
- Review: the MM restart-recovery evidence is now committed and no formal certification wording was introduced.
- QA: BASIC validation must still prove private simplex both directions, group call on `226333`, SDS/status, WAP terminal delivery, restart re-affiliation, scan/group retention, and long-run stability.
- Voice architecture: live symptom still looks like media starvation or lower-layer traffic handling after a valid floor grant. The next live proof must compare good and bad PTT directions using `CallControl::Open`, `FloorGranted`, UMAC circuit metadata, LMAC TCH/S decode, and PHY train/block logs.
- MAC/UMAC architecture: pure UMAC TCH/S bit preservation is already covered. The strongest non-repeating code suspect is BS LMAC handling of `NormalTrainSeq2` second half (`Block2`) when the first half is STCH and the second half is non-stolen TCH/S.
- MM/SDS architecture: MM restart recovery is closed for current scope; WAP MVP is SDS Type 4/WAP PID delivery, not full SNDCP/IP WAP.

Simple component meanings for next work:

- CMCE is call control: setup, PTT/floor, call release, and who is allowed to talk.
- UMAC/MAC is the scheduler: grants resources, maps speech/signalling to slots, and routes uplink voice to the right downlink slot.
- LMAC is burst framing: turns physical traffic blocks into TCH/S voice frames and drops bad CRC frames.
- PHY is the radio burst layer: train sequence, half-slot/block identity, RF decode quality, and timing.
- MM is terminal mobility: registration, restart recovery, group affiliation, and energy economy.
- SDS/status is short data/status messaging.
- WAP MVP is the terminal browser page path over SDS Type 4/WAP PID; full SNDCP/IP WAP remains a separate open track.

Immediate non-repeating execution order:

1. Do not add more MM restart-recovery tests unless a new gap appears; current MM batch is committed in `7f85bc0`.
2. Use the current observability commit `3776497` to capture fresh live PTT logs after a real test:
   - private simplex `2260082 -> 2260616`;
   - private simplex `2260616 -> 2260082`;
   - group PTT on GSSI `226333`.
3. In those logs, answer first:
   - does CMCE open the expected local `CircuitDlMediaSource::LocalLoopback` circuit for local private/group calls?
   - does `FloorGranted source_issi` match the radio pressing PTT?
   - does UMAC route UL TCH/S to the expected DL slot/peer slot?
   - does LMAC report valid TCH/S frames, CRC failures, or ignored partial blocks?
4. If logs prove CMCE opens the wrong circuit, patch CMCE under EN 300 392-2 clause 14.5.1/14.5.2 with a focused test.
5. If CMCE is correct but LMAC ignores or drops real TCH/S, patch LMAC under EN 300 392-2 clause 23.5.4 with tests around `NormalTrainSeq2`/STCH/TCH half-slot behavior.
6. If LMAC sees CRC failures or PHY train mismatch only for one terminal/direction, isolate RF/PHY quality before masking it with call-control changes.
7. After a focused local patch passes tests, build locally only and deploy direct to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; do not compile on `chris@192.168.1.179` and do not create binary backups.

ETSI anchors for the next likely patch:

- EN 300 392-2 clause 14.5.1: private/individual call control and floor handling.
- EN 300 392-2 clause 14.5.2: group call control and floor handling.
- EN 300 392-2 clause 23.5 / 23.5.4: MAC traffic channel, FACCH/STCH/TCH behavior, and traffic block handling.
- EN 300 392-2 clause 13.2 and tables 14.14/14.27: SDS/status if the next patch touches data/status.
- EN 300 392-2 clauses 16.4.4, 16.7.1, 16.10.9, 16.10.10, 16.10.23, 16.10.35a, 23.7.6/T.210: MM/EG only if the next patch touches restart or energy economy.

Anti-loop rules from this checkpoint:

- Do not repeat UMAC pure bit-copy tests as the main static-audio investigation; they already pass.
- Do not treat SDS/WAP component tests as proof that the terminal browser live page works; live WAP validation remains required.
- Do not call the stack `100% certified`; only clause-scoped engineering evidence exists.
- Keep `call_preemptive` / transmission interruption default off.
- Encryption remains out of focus unless explicitly requested.

## 2026-06-04 11:14:01 EEST - UMAC idle traffic no longer emits all-zero speech

Patch scope:

- File changed: `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
- Component: UMAC/MAC downlink scheduler.
- Simple meaning: this scheduler decides what the BS transmits on an assigned traffic slot. If it has real uplink speech, it sends TCH/S speech. If it has call-control signalling, it uses FACCH/STCH stealing. If it has neither, it must keep the assigned channel alive without inventing invalid speech.

ETSI clause scope:

- EN 300 392-2 clause 23.8.5: when the BS does not receive data from the sending MS, it should still transmit on the downlink channel; examples include C-plane Null PDUs or substitution traffic.
- EN 300 392-2 clause 23.5 / 23.5.4: traffic channel and STCH/FACCH slot handling.
- This is clause-scoped engineering hardening, not a formal certification claim.

Behavior changed:

- Before: an active DL traffic circuit with no queued uplink voice produced a 274-bit all-zero TCH/S block as "silence".
- After: an active DL traffic circuit with no queued uplink voice transmits C-plane Null PDUs on STCH half-slots.
- FACCH/STCH with real queued voice still keeps first half = STCH and second half = TCH/S.
- FACCH/STCH without queued voice uses first half = STCH signalling and second half = STCH Null PDU.
- This avoids sending an all-zero ACELP frame as clean speech when that frame is not proven to be a valid TETRA silence/substitution frame.

Tests added/updated:

- Added `test_active_traffic_slot_without_voice_uses_stch_null_not_zero_tch`.
- Added `test_facch_without_voice_replaces_second_half_with_stch_null`.
- Added `test_facch_with_voice_keeps_second_half_tch_s`.
- Updated EG/FACCH tests that previously expected TCH/S zero filler while a FACCH item was deferred or pruned.

Verification:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib active_traffic_slot_without_voice --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib facch_ --locked` -> 11 passed.
- `cargo test -p tetra-entities --lib --locked` -> 204 passed, 5 ignored.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 42 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 2 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Current conclusion:

- This patch removes one plausible static-audio source: synthetic all-zero TCH/S during active call gaps or missing uplink voice.
- It does not prove live private/group audio is fixed; live PTT validation is still required.
- Next live test must still capture `2260082 -> 2260616`, `2260616 -> 2260082`, and group `226333`, with logs for `CMCE opening UMAC circuit`, `FloorGranted`, `UMAC voice route`, `rx_blk_traffic`, CRC failures, and STCH/FACCH events.

## 2026-06-04 11:17:37 EEST - Deployed null-idle traffic patch to test BS

Commit deployed:

- `2201923 fix: transmit null traffic idle instead of zero speech`

Local build:

- Built locally on macOS only; no compilation was done on `chris@192.168.1.179`.
- Command shape used: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Output binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs`
- Local SHA256: `bcd5bc2cff5e1f253f80b66c0009db7da5930de9117b348ea63b9bbf49006da5`

Remote deploy:

- Host: `chris@192.168.1.179`
- Target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- First SCP failed while the old BS process still held the destination open.
- Stopped test BS/control-service via existing pidfile method.
- Deployed direct over the testing binary; no binary backup was created.
- Remote SHA256 matches local: `bcd5bc2cff5e1f253f80b66c0009db7da5930de9117b348ea63b9bbf49006da5`
- Restarted with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`
- New remote processes:
  - control wrapper pid `15798`
  - control-service pid `15801`
  - nexus-bs pid `15803`

Post-restart live checks:

- Dashboard root on `127.0.0.1:8080` returned Nexus-BS v0.1.55 HTML.
- `2260082` re-registered/re-affiliated to group `226333`; RSSI around `-22 dBFS`; EG3 allocated after registration.
- `2260616` re-registered/re-affiliated to group `226333`; RSSI around `-33 dBFS`; EG3 allocated after registration.
- `2260618` re-registered/re-affiliated to group `226333`; RSSI around `-38 dBFS`; requested EG1 and BS allocated EG3.
- No post-deploy PTT attempt was present yet in the checked log filters for `U-TX DEMAND`, `D-TX GRANTED`, `UMAC voice route`, `rx_blk_traffic`, `UL inactivity`, `PTT denied`, or `NotGranted`.

Next operator validation:

- Test private simplex `2260082 -> 2260616`, then `2260616 -> 2260082`.
- Test group PTT on `226333` with at least two radios alternating.
- If static remains, collect only post-`2201923` logs and decide by evidence:
  - wrong CMCE circuit/floor -> patch CMCE under EN 300 392-2 clause 14.5.1/14.5.2;
  - no valid `rx_blk_traffic` after grant -> isolate LMAC/PHY;
  - valid `UMAC voice route` but bad receive audio -> inspect downlink FACCH/STCH/TCH and RF path.

## 2026-06-04 11:23:47 EEST - PM orchestration refreshed and delegated

User directive:

- Add a Project Manager agent to orchestrate the work.
- Split work into review, architecture, and QA responsibilities.
- Reload ETSI law/status/project log before further protocol work.
- Keep execution state and next actions in `timeline.md` so the next resume does not loop.

Law/status reload completed before this checkpoint:

- `/Users/ctermure/.codex/memories/tetra-etsi-compliance-law.md`
- `/Users/ctermure/.codex/memories/flowstation-tetra-eg-swmi-resume-2026-06-02.md`
- `/Users/ctermure/.codex/memories/flowstation-aarch64-soapysdr-build.md`

Current repo state:

- Workdir: `/Users/ctermure/Work/basestion`
- Branch: `nexus-bs-v0.1.55`
- HEAD: `84a15d9 docs: record null-idle test deployment`
- Worktree: clean
- Active goal: clause-scoped ETSI EN 300 392-2 hardening only. This is not a formal certification claim.

PM agent status:

- A new PM agent spawn was attempted again, but the agent thread limit is reached.
- Existing agent `019e911b-1c4b-7f02-a72f-2bdf280d6c35` (`Aquinas the 3rd`) remains assigned as Project Manager.
- PM role: orchestration only. PM owns execution order, anti-loop discipline, timeline handoff quality, and evidence gates.

Delegated agent roles:

- PM / orchestration: `019e911b-1c4b-7f02-a72f-2bdf280d6c35` (`Aquinas the 3rd`).
- Review: `019e911b-1cad-75a2-8032-1bd9fe865e83` (`Heisenberg the 3rd`).
- Voice architecture: `019e911b-1d27-7b91-a06c-c8393037b7e7` (`Arendt the 3rd`).
- QA: `019e9134-b662-7ac0-a27f-dd8446c1c03b` (`Maxwell the 3rd`).
- MAC/UMAC architecture: `019e9134-cc83-7793-a243-e7e1428e2587` (`Ohm the 3rd`).
- MM/SDS robustness: `019e9134-e2c2-7cb1-a626-0f86672f87f6` (`Herschel the 3rd`).

Simple component meanings for operators and next handoff:

- PM: keeps the work ordered and blocks circular work without evidence.
- Review: looks for regressions, missing tests, and unsupported compliance wording.
- CMCE: call control. It handles call setup, floor/PTT grant, who may speak, and call release.
- UMAC/MAC: radio scheduler. It maps signalling and speech to slots and routes uplink voice to downlink users.
- LMAC: burst framing. It decodes/encodes traffic bursts, including TCH/S speech and CRC/BFI decisions.
- PHY: radio burst layer. It detects train sequence, block identity, timing, and RF decode quality.
- MM: mobility management. It handles registration, restart recovery, group affiliation, and energy economy.
- SDS/status: short data and status messaging.
- WAP MVP: terminal browser page delivery over SDS Type 4/WAP PID in the current stack. Full SNDCP/IP WAP is still a separate open implementation track.

Agent feedback integrated in this checkpoint:

- PM confirmed current state: `2201923` null-idle traffic patch is deployed and `84a15d9` records the deployment; no post-deploy live PTT result is yet recorded.
- QA produced a BASIC/24x7 validation matrix. Required live evidence remains private simplex both directions, private duplex if supported, group PTT on `226333`, SDS/status, WAP terminal page, restart re-affiliation, scan/group retention, and soak stability.
- MM/SDS confirmed restart-recovery and SDS/status component tests are strong for their current scope, but live WAP terminal-browser delivery and EG-window delivery evidence remain separate from unit tests.
- MM/SDS also confirmed current WAP is SDS Type 4/WAP MVP, not full SNDCP/IP WAP service advertising.
- Review, Voice Architecture, and MAC/UMAC agents were re-tasked and must be integrated before the next protocol patch if they return concrete risks.

Local technical observation for the next voice investigation:

- `crates/tetra-entities/src/lmac/lmac_bs.rs` classifies `NUB + NormalTrainSeq2 + Block2` as `LogicalChannel::TchS` when the burst is traffic and block 2 is not stolen.
- The same file's `rx_blk_traffic` currently forwards only `LogicalChannel::TchS` with `PhyBlockNum::Both`; it drops `Block2` as partial/unsupported.
- This is a credible non-repeating suspect for the live one-way/static-audio path, but it must not be patched by pretending a 216-bit half-block is a clean 274-bit ACELP frame.
- Clause scope if patched: EN 300 392-2 clause 23.5/23.5.4 for traffic channel STCH/TCH handling and clause 23.8.3 for bad/partial speech-frame handling. Any compatibility behavior must be labelled as such.

Immediate execution order:

1. Do not repeat pure UMAC TCH/S bit-copy tests as the main static-audio hypothesis; they already pass.
2. Collect fresh post-`2201923` live PTT evidence:
   - private simplex `2260082 -> 2260616`;
   - private simplex `2260616 -> 2260082`;
   - group PTT on GSSI `226333`, alternating radios.
3. For each attempt, map:
   - `U-TX DEMAND`;
   - `D-TX GRANTED`;
   - `FloorGranted source_issi`;
   - `CMCE opening UMAC circuit`;
   - `CircuitDlMediaSource`;
   - `UMAC voice route`;
   - `rx_blk_traffic`;
   - CRC/BFI;
   - FACCH/STCH/TCH placement;
   - operator audio result.
4. If CMCE circuit/floor is wrong, patch CMCE under EN 300 392-2 clause 14.5.1/14.5.2 with a focused test.
5. If CMCE is correct but LMAC drops valid traffic blocks, patch LMAC under EN 300 392-2 clause 23.5/23.5.4 and 23.8.3 with a focused unit test.
6. If LMAC sees CRC/PHY quality failures only in one direction, isolate PHY/RF before masking the issue in CMCE or UMAC.
7. After a focused local patch passes tests, build locally only and deploy direct to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; do not compile on `chris@192.168.1.179` and do not create binary backups.

Evidence required before BASIC paths are called robust:

- Private simplex: `2260082 -> 2260616` and `2260616 -> 2260082` both have intelligible audio, correct floor owner, and no unjustified `PTT denied`.
- Group call: at least two radios alternate PTT on `226333` with intelligible audio and no stale floor.
- Private duplex: setup, media routing, and release are validated separately from simplex, or unsupported terminal behavior is documented without wrong bearer setup.
- SDS/status: ISSI route uses acknowledged delivery; GSSI route uses unacknowledged delivery; live delivery works.
- WAP: terminal browser reaches the Nexus-BS greeting page. Unit tests for SDS payload are not enough.
- Restart recovery: terminals re-register/re-affiliate after BS restart based on real terminal responses, not fabricated stale cache state.
- Scan/group retention: selected group remains usable after idle/scan cycles and after restart.
- 24x7 stability: process/dashboard/registry stay healthy, logs are bounded, periodic private/group/SDS/WAP probes pass, and there is no panic, log storm, stale floor, or repeated unjustified PTT denial.

Anti-loop rules:

- No broad refactor before the live voice blocker is classified by evidence.
- No certification wording. Use clause-scoped ETSI-aligned evidence only.
- `call_preemptive` / transmission interruption stays default off.
- Encryption remains out of focus.
- Do not advertise full SNDCP/IP WAP until that bearer is implemented and tested.

## 2026-06-04 11:26:10 EEST - Review/MAC agent feedback integrated

Agent feedback received:

- QA, PM, MM/SDS, MAC/UMAC, and Review agents returned read-only feedback.
- Voice Architecture was still running at the wait timeout; do not block on it unless it returns a concrete contradiction.

Review findings:

- High risk: `LMAC` can still drop the likely live half-slot speech path. `NormalTrainSeq2 + Block2` may be classified as `TchS`, but `rx_blk_traffic` only forwards `PhyBlockNum::Both`.
- High risk: there is still no post-`2201923` live PTT evidence, so null-idle traffic hardening is not proof that live static/audio is fixed.
- Medium risk: UMAC null-idle tests stop at scheduler output; add/keep LMAC boundary coverage for `STCH+STCH` and `STCH+TCH/S` through `NormalTrainSeq2`.
- Medium risk: CMCE and UMAC private simplex component coverage is good, but not end-to-end through LMAC/PHY for field ISSIs `2260082` and `2260616`.
- Low risk: SDS/WAP wording is currently safe because WAP is scoped as SDS Type 4 MVP, not full SNDCP/IP WAP.

MAC/UMAC architecture findings:

- Priority suspect is `LMAC` `NormalTrainSeq2` block semantics:
  - `determine_logical_channel_ul` may classify `Block2` as `TchS`;
  - `rx_blk_traffic` then drops non-`Both` traffic as partial/unsupported.
- This can bypass the existing UMAC voice tests because those tests inject `TmdCircuitDataInd` after LMAC.
- Do not forward a 216-bit half-block as clean 274-bit ACELP speech unless a correct ETSI/BFI-bearing path exists.
- Existing bad-CRC behavior is correct for the current SAP: since TMD cannot carry BFI/half-slot condition, corrupt/partial speech must fail closed instead of becoming clean speech/static.

Decision:

- Next local patch, if no fresher live logs contradict it, should be a focused LMAC evidence patch:
  - add a test proving `NormalTrainSeq2 + Block2` traffic is not silently treated as valid clean speech when only a half TCH/S block is available;
  - improve LMAC logging/guarding so live logs distinguish:
    - valid full TCH/S decoded and forwarded;
    - CRC/BFI drop;
    - partial `Block2` TCH/S drop due missing full speech frame support.
- This is an observability/safety patch unless a full ETSI-supported BFI/TMD SAP path is added.

ETSI clause scope for that patch:

- EN 300 392-2 clause 23.5 / 23.5.4: traffic-channel STCH/TCH/FACCH placement and block handling.
- EN 300 392-2 clause 23.8.3 / 23.8.3.2: bad/undecodable speech frame handling.
- Any current inability to carry BFI is local implementation limitation and must be labelled as such.

Focused local verification for the LMAC patch:

- `cargo fmt -p tetra-entities`
- `cargo test -p tetra-entities --test test_lmac_bs --locked`
- `cargo test -p tetra-entities --lib facch_ --locked`
- `cargo test -p tetra-entities --test test_umac_bs voice --locked`
- `git diff --check`

Live validation still required after any patch:

- Test private simplex `2260082 -> 2260616`.
- Test private simplex `2260616 -> 2260082`.
- Test group PTT on `226333`.
- Log filter:
  - `2260082|2260616|2260618|226333|U-TX DEMAND|D-TX GRANTED|NotGranted|RequestQueued|GrantedToOtherUser|CallControl::Open|FloorGranted|FloorReleased|peer_ts|media_source|UMAC voice route|FACCH|STCH|TCH|NormalTrainSeq2|Block2|blk2_stolen|rx_blk_traffic|CRC fail|partial/unsupported|UL inactivity|T.210|energy`

## 2026-06-04 11:33:05 EEST - LMAC partial TCH/S guard and evidence tests

Patch scope:

- Files changed:
  - `crates/tetra-entities/src/lmac/lmac_bs.rs`
  - `crates/tetra-entities/tests/test_lmac_bs.rs`
  - `timeline.md`
- Component: LMAC, the lower MAC burst-framing layer.
- Simple meaning: LMAC decides whether a received radio burst is control signalling, full TCH/S speech, or a partial/stolen traffic block. It must not present incomplete or bad speech as clean voice.

ETSI clause scope:

- EN 300 392-2 clause 23.5 / 23.5.4: traffic channel, STCH/FACCH/TCH placement, and burst/block handling.
- EN 300 392-2 clause 23.8.3 / 23.8.3.2: bad/undecodable speech frame handling.
- Current implementation limitation: the local TMD SAP does not carry BFI/half-slot condition, so LMAC fails closed for partial/bad speech rather than forwarding static as valid ACELP.

Behavior clarified:

- Before: `rx_blk_traffic` had one generic trace-level drop for every non-`Both` or non-`TchS` traffic block.
- After: LMAC explicitly distinguishes:
  - unsupported traffic channel -> trace and drop;
  - partial `TchS` block such as `NormalTrainSeq2 + Block2` -> debug log and drop because the TMD SAP cannot preserve BFI/half-slot condition;
  - full-slot `TchS` with good CRC -> decode and forward to UMAC;
  - full-slot `TchS` with bad speech CRC -> drop, as before.

Tests added/strengthened:

- Added `bs_lmac_forwards_valid_fullslot_tch_s_to_umac`.
  - Proves a valid 432-bit full-slot TCH/S frame decodes and reaches UMAC as `TmdCircuitDataInd`.
  - Also verifies the test vector round-trips through `encode_tp`/`decode_tp`.
- Added `bs_lmac_drops_normal_seq2_block2_tch_s_without_forwarding_clean_speech`.
  - Proves a `NormalTrainSeq2 + Block2` half TCH/S block is not forwarded as clean speech.
  - This protects against turning 216-bit partial speech into static/audio corruption.
- Strengthened `bs_lmac_drops_bad_crc_tch_s_instead_of_forwarding_static_speech`.
  - The harness now marks all four UL timeslots as traffic before injecting the corrupt TCH/S frame, so the test really exercises the traffic CRC path.

Verification:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 4 passed.
- `cargo test -p tetra-entities --lib facch_ --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_umac_bs voice --locked` -> 3 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Live log check during this checkpoint:

- Remote test BS on `chris@192.168.1.179` is still running:
  - wrapper/control pid `15798`;
  - control-service pid `15801`;
  - nexus-bs pid `15803`.
- Log still does not contain a complete post-`2201923` live PTT trace with `U-TX DEMAND`, `D-TX GRANTED`, `UMAC voice route`, and `rx_blk_traffic`.
- Several `rx_tpsap_prim got NormalTrainSeq2 in fullslot` entries exist after deploy. Without this LMAC patch, the log did not say whether those resulted in STCH control, partial TCH/S drop, or valid speech.

Conclusion:

- This patch does not claim live private/group audio is fixed.
- It makes the LMAC boundary safe and observable:
  - full valid TCH/S is proven to pass;
  - partial/bad TCH/S is proven not to become clean speech/static.
- Next step after commit/deploy is still a live private/group PTT test using the log filter from the previous checkpoint.

## 2026-06-04 11:36:35 EEST - Deployed LMAC guard build to test BS

Commit deployed:

- `bfc1960 test: guard LMAC partial speech handling`

Local build:

- Built locally on macOS only; no compilation was done on `chris@192.168.1.179`.
- Command shape used: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Output binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs`
- Local SHA256: `b830daa3e18bcb478092e453a6be6618165b7110e109701c4a188cbf0865ae7c`

Remote deploy:

- Host: `chris@192.168.1.179`
- Target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- Stopped the prior test BS/control-service using the existing pidfiles.
- Deployed direct over the testing binary; no binary backup was created.
- Remote SHA256 matches local: `b830daa3e18bcb478092e453a6be6618165b7110e109701c4a188cbf0865ae7c`
- Restarted with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`
- New remote processes:
  - control wrapper pid `16125`
  - control-service pid `16128`
  - nexus-bs pid `16130`

Post-restart checks:

- Dashboard root on `127.0.0.1:8080` returned HTML.
- `2260616` re-registered/re-affiliated to group `226333`; soft re-attach after restart was handled and EG3 assignment was attempted.
- `2260082` reappeared with RSSI/ACK activity and soft re-attach handling; EG assignment later timed out and fell back to `StayAlive`.
- `2260618` re-registered/re-affiliated to group `226333`; requested EG1 and BS allocated EG3.
- `T352 expired` appeared for BS-initiated EG assignment to `2260082` and `2260616`; current fail-safe behavior keeps `StayAlive`.
- One post-deploy `NormalTrainSeq2 in fullslot` was seen at PHY after restart, but no complete PTT trace was present yet.

Current live status:

- The deployed binary contains the LMAC partial TCH/S debug guard.
- No post-`bfc1960` private/group PTT attempt has been recorded yet.
- The next operator test must be:
  - private simplex `2260082 -> 2260616`;
  - private simplex `2260616 -> 2260082`;
  - group PTT on `226333`, alternating radios.
- Required log focus:
  - `U-TX DEMAND`, `D-TX GRANTED`, `FloorGranted`, `CMCE opening UMAC circuit`, `UMAC voice route`, `rx_blk_traffic`, `dropping partial TCH/S`, `CRC fail`, `NormalTrainSeq2`, `Block2`, `PTT denied`, `NotGranted`, `UL inactivity`.

## 2026-06-04 11:39:20 EEST - Voice architecture feedback integrated

Voice Architecture returned after the LMAC guard deploy.

Key feedback:

- Strongest non-repeating live suspect remains real-air LMAC TCH/S handling around `NormalTrainSeq2`, not UMAC bit-copy.
- Existing UMAC voice tests inject after LMAC, so they cannot prove that PHY/LMAC traffic bursts reach UMAC.
- The deployed `bfc1960` patch covers the safe fail-closed/logging side:
  - valid full-slot TCH/S passes;
  - partial `Block2` TCH/S is not forwarded as clean speech/static.
- It does not implement a BFI/half-slot-condition capable traffic SAP or explicit half-slot TCH/S decode. That remains a future LMAC design task if live logs prove terminals send speech primarily as `NormalTrainSeq2 + Block2`.

Additional patch candidate from Voice Architecture:

- CMCE private setup should not route a configured-local ISSI over Brew only because that ISSI is currently absent from `subscriber_groups`.
- For configured local SSI ranges, an unregistered callee should be rejected locally/recovered locally, not misclassified as external/Brew.
- ETSI scope if patched:
  - EN 300 392-2 clause 14.5.1.1.2 for first setup response/dummy call reference.
  - EN 300 392-2 clause 14.5.1.3.2 for unsupported/rejected individual-call release.
  - Local SSI range is a deployment policy guard, not an ETSI rule.

Next code task selected:

- Add a focused CMCE guard/test for local-range but unregistered private-call destination.
- Keep PBX/phone `called_ssi == 0` and non-local Brew-routable ISSIs on the existing Brew path.

## 2026-06-04 11:40:32 EEST - CMCE local unregistered private-call guard

Patch scope:

- Files changed:
  - `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - `crates/tetra-entities/tests/test_cmce_bs.rs`
  - `timeline.md`
- Component: CMCE private-call setup.
- Simple meaning: CMCE decides whether a private call is local, external/Brew, or rejected before setup.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.1.2: first SwMI response to individual-call setup.
- EN 300 392-2 clause 14.5.1.3.2: rejecting unsupported/unreachable individual call with D-RELEASE before a SwMI call identity exists.
- Configured `local_ssi_ranges` remains deployment policy, not an ETSI address rule.

Behavior changed:

- Before: if a called private ISSI was not in `subscriber_groups`, CMCE entered the Brew fallback path first. Later Brew routing checks could reject it, but logs/semantics misclassified a configured-local offline ISSI as an external routing candidate.
- After: if `called_addr.ssi` is inside `config.cell.local_ssi_ranges` and is not registered/affiliated locally, CMCE rejects locally with dummy-call-id `D-RELEASE` cause `CalledPartyNotReachable`.
- PBX/phone calls with `called_ssi == 0` still use the Brew path.
- Non-local unregistered ISSIs still use the existing Brew path if routable/configured.

Test added:

- `test_p2p_setup_to_configured_local_unregistered_issi_rejects_without_brew_fallback`
  - Configures local SSI range `2260000..2269999`.
  - Registers only the caller.
  - Calls local but unregistered `2260616`.
  - Asserts one dummy-call-id `D-RELEASE` with `CalledPartyNotReachable`.
  - Asserts no `NetworkCircuitSetupRequest` is sent to Brew and no UMAC traffic circuit opens.

Verification:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_setup_to_configured_local_unregistered_issi_rejects_without_brew_fallback --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 116 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Conclusion:

- This is a private-call routing hardening patch. It does not claim live audio is fixed.
- It prevents one misleading local-vs-external call setup path for lab ISSIs such as `2260082`, `2260616`, and `2260618` when they are configured local but not currently registered.

## 2026-06-04 11:43:35 EEST - Deployed CMCE local setup guard to test BS

Commit deployed:

- `1b390c8 fix: reject local unregistered private setup locally`

Local build:

- Built locally on macOS only; no compilation was done on `chris@192.168.1.179`.
- Command shape used: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Output binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs`
- Local SHA256: `57875e881f324b462a03f893aa705fdbcf2ae02bbf74a7a7729e5cb0a024253d`

Remote deploy:

- Host: `chris@192.168.1.179`
- Target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- Stopped the prior test BS/control-service using existing pidfiles.
- Deployed direct over the testing binary; no binary backup was created.
- Remote SHA256 matches local: `57875e881f324b462a03f893aa705fdbcf2ae02bbf74a7a7729e5cb0a024253d`
- Restarted with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`
- New remote processes:
  - control wrapper pid `16325`
  - control-service pid `16328`
  - nexus-bs pid `16330`

Post-restart checks:

- Dashboard root returned `Nexus-BS v0.1.55 Dashboard`.
- `2260082` reappeared, re-affiliated to group `226333`, and showed RSSI/ACK activity around `-22 dBFS`.
- `2260616` re-registered/re-affiliated to group `226333`, RSSI around `-26 dBFS`, and EG3 assignment was attempted.
- `2260618` had post-restart retry/LLC activity in the sampled log tail; confirm full registration/affiliation again before using it for WAP/live call evidence.
- `T352 expired` appeared for BS-initiated EG assignment to `2260082`; fail-safe behavior kept `StayAlive`.

Current live status:

- The test BS now includes:
  - null-idle traffic patch `2201923`;
  - LMAC partial TCH/S guard `bfc1960`;
  - CMCE local-unregistered private setup guard `1b390c8`.
- No post-`1b390c8` private/group PTT attempt has been recorded yet.
- Next operator validation is unchanged:
  - private simplex `2260082 -> 2260616`;
  - private simplex `2260616 -> 2260082`;
  - group PTT on `226333`, alternating radios.
- Required live log filter:
  - `2260082|2260616|2260618|226333|U-SETUP|D-SETUP|D-CONNECT|U-CONNECT|U-TX DEMAND|D-TX GRANTED|FloorGranted|CMCE opening UMAC circuit|media_source|peer_ts|UMAC voice route|rx_blk_traffic|dropping partial TCH/S|CRC fail|NormalTrainSeq2|Block2|PTT denied|NotGranted|UL inactivity|CalledPartyNotReachable`

## 2026-06-04 11:53:57 EEST - Group floor handoff FACCH compacting patch

User live report:

- Group call still failed for alternating speakers; requirement is multiple MSs on one GSSI taking turns normally.
- Treat this as BASIC TETRA SwMI behavior, not a certification claim.

Live log evidence from `chris@192.168.1.179:/home/chris/nexus-bs-v0.1.55-test/nexus-bs.log`:

- Group call `call_id=4`, GSSI `226333`, traffic slot `ts=2`.
- `2260616` had floor first and UMAC routed voice repeatedly:
  - `UMAC floor granted: call_id=4 source_issi=2260616 dest_gssi=226333 ul_ts=2 media_source=LocalLoopback`
  - repeated `UMAC voice route: UL ts=2 bits=274 -> DL ts=2`.
- After `U-TX CEASED`, `2260082` requested floor:
  - `U-TX DEMAND: ISSI 2260082 requests floor on call_id=4`
  - CMCE emitted `D-TX GRANTED`.
- Defect observed:
  - `D-TX GRANTED` with optional transmitting-party address serialized as a 61-bit CMCE SDU.
  - With the MAC-RESOURCE header it exceeded STCH capacity: `MAC-RESOURCE hdr 70 + SDU 61 bits > 124`.
  - UMAC therefore fell back to MCCH/SCH-F while the terminals were on the assigned traffic channel.
  - The preserved `RandomAccessAck` for `2260082` on `ts=2` was discarded by `dl_drop_all_except_stolen`.
  - No `UMAC voice route` followed for `2260082`; `UL inactivity timeout on ts=2` fired.

Component explanation:

- CMCE group floor control decides who may transmit next in the group.
- `D-TX GRANTED` is the CMCE message that gives one MS the microphone and tells the group that another user is transmitting.
- FACCH/STCH is the assigned traffic-channel signalling path. If the floor grant falls back to common-channel SCH/F while radios are listening on the assigned channel, the new speaker may never switch to transmit voice.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: during a group call, SwMI sends individual `D-TX GRANTED` to the granted MS and group-addressed `D-TX GRANTED` to the other MSs.
- EN 300 392-2 table 14.18: transmitting-party type/address IEs in `D-TX GRANTED` are optional/conditional.
- EN 300 392-2 clause 23.5 and 23.5 traffic-mode STCH/FACCH text: signalling may be stolen from the traffic channel for call-control messages during an over.
- Engineering decision: omit optional transmitting-party IEs in assigned-channel `D-TX GRANTED` so the mandatory floor state fits STCH/FACCH. This is clause-scoped ETSI-aligned behavior, not a full-stack certification claim.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - `fsm_send_d_tx_granted_individual` now emits compact `D-TX GRANTED` without optional transmitting-party IEs.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - `send_d_tx_granted_facch` now emits compact group-addressed `D-TX GRANTED`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `assert_compact_d_tx_granted_facch`.
  - Updated group floor queue, default-off preemption, enabled preemption grant, unaffiliated rejection, and queued handoff tests to require compact 25-bit `D-TX GRANTED`.

Verification:

- `cargo fmt -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs group_tx_demand --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_tx_ceased_hands_floor_to_queued_requester --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group_preemptive --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 116 passed.
- `cargo test -p tetra-entities --test test_umac_bs facch --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_private_floor_grant_stch_carries_preserved_random_access_ack --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 42 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Current conclusion:

- The logged group-call failure is explained by an oversized `D-TX GRANTED` floor handoff that could not be transmitted on assigned-channel STCH/FACCH.
- The patch removes that cause and keeps preemption default-off.
- Live group audio is not yet proven fixed until this build is deployed and `2260616`/`2260082` alternate PTT on GSSI `226333` without `UL inactivity timeout`, `PTT denied`, or SCH/F fallback for `D-TX GRANTED`.

Next non-repeating execution:

1. Commit this narrow CMCE group FACCH compacting patch.
2. Build locally only with the Nexus-BS AArch64 command from build memory.
3. Deploy direct over `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; no binary backup.
4. Restart BS test.
5. Live test:
   - group PTT `2260616 -> 226333`, release;
   - group PTT `2260082 -> 226333`, release;
   - repeat at least three alternating turns.
6. Required pass evidence:
   - compact `D-TX GRANTED` does not log `does not fit STCH`;
   - no `dl_drop_all_except_stolen` discards the requester ACK needed for floor grant;
   - `UMAC floor granted` changes source ISSI on each turn;
  - `UMAC voice route` appears for each speaker;
  - no `UL inactivity timeout` during an active speaker over;
  - operator audio verdict confirms voice, not static.

## 2026-06-04 11:58:30 EEST - Deployed compact group floor grant build to test BS

Commit deployed:

- `419ce67 fix: compact group floor grants for FACCH`

Local verification before deploy:

- `cargo fmt -p tetra-entities`
- `cargo test -p tetra-entities --test test_cmce_bs group_tx_demand --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_tx_ceased_hands_floor_to_queued_requester --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group_preemptive --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 116 passed.
- `cargo test -p tetra-entities --test test_umac_bs facch --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_private_floor_grant_stch_carries_preserved_random_access_ack --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 42 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Local build:

- Built locally on macOS only; no compilation was done on `chris@192.168.1.179`.
- Command shape: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Local binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs`
- Local SHA256: `427626e5f9bffc708884aa77534fad5d63673ff9049ed489d0f6a383c1f16c12`

Remote deploy:

- Host: `chris@192.168.1.179`
- Target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- Stopped the prior test BS/control-service via existing pidfiles before copying.
- Deployed direct over the testing binary; no binary backup was created.
- Remote SHA256 matches local: `427626e5f9bffc708884aa77534fad5d63673ff9049ed489d0f6a383c1f16c12`
- Restarted with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`
- New remote processes:
  - control wrapper pid `16704`
  - control-service pid `16707`
  - nexus-bs pid `16709`
- Dashboard root returned HTML.

Post-restart terminal state:

- `2260082` registered and affiliated to `[226333]` twice during restart recovery; RSSI about `-22 dBFS`.
- `2260616` registered and affiliated to `[226333]`; it also deaffiliated/re-affiliated once during group update; EG assignment later timed out and stayed/fell back to `StayAlive`.
- `2260618` registered and affiliated to `[226333]`; RSSI about `-46 dBFS`; EG3 was allocated.

Current live status:

- No post-`419ce67` PTT test has been recorded yet.
- The BS is ready for physical group test on GSSI `226333`.
- Required test sequence:
  - `2260616` PTT on group `226333`, speak, release.
  - `2260082` PTT on group `226333`, speak, release.
  - Repeat at least three alternating turns.
- Required log filter after the test:
  - `2260082|2260616|226333|U-TX DEMAND|D-TX GRANTED|does not fit STCH|FACCH stealing|FloorGranted|UMAC floor granted|UMAC voice route|dl_drop_all_except_stolen|RandomAccessAck|UL inactivity|PTT denied|NotGranted`

Pass criteria for this patch:

- No `D-TX GRANTED ... does not fit STCH` on the group floor handoff path.
- No requester `RandomAccessAck` is discarded at the handoff.
- `UMAC floor granted` alternates `source_issi` between `2260616` and `2260082`.
- `UMAC voice route` appears after each grant.
- No `UL inactivity timeout` during an active over.
- Operator audio verdict: both directions are voice, not static.

## 2026-06-04 12:15:00 EEST - Preserved raw TCH/S Block2 for group call alternation

User-reported failure:

- Group call still produced bad audio/static when multiple local subscribers tried to speak in turn.
- Existing CMCE compact `D-TX GRANTED` patch fixed an STCH/FACCH size cause, but did not prove received voice after floor handoff.

Component meaning:

- CMCE: call-control and floor/PTT authority. It decides who is allowed to speak.
- UMAC/MAC scheduler: maps voice/signalling onto assigned traffic slots and routes uplink voice back to downlink listeners.
- LMAC: burst framing. It interprets PHY bursts as STCH signalling or TCH/S speech and encodes the downlink burst sent to PHY.

ETSI clause scope:

- EN 300 392-2 clause 23.8.4.1.4: on uplink `NormalTrainSeq2`, first half is STCH; if the second half is not stolen, the BS shall interpret the second half as TCH.
- EN 300 392-2 clause 23.8.5: BS should pass U-plane TCH onward while preserving timing, ordering and half-slot pairing; if replacing a stolen half-slot, it may use C-plane Null PDU or substitution traffic.
- EN 300 392-2 clause 23.5: STCH/FACCH permits signalling capacity to be stolen from a traffic channel during an over.
- Engineering decision: do not decode a raw `Block2` half-slot as a clean 274-bit ACELP frame, because the first half was STCH and the current SAP has no BFI field. Instead, tag the 216-bit type-5 `TCH/S Block2` as raw, route it locally through UMAC, and re-emit it in the same second-half position on downlink.

Patch:

- `crates/tetra-saps/src/tmd/mod.rs`
  - `TmdCircuitDataReq` and `TmdCircuitDataInd` now carry `raw_tch_s_block: Option<PhyBlockNum>`.
- `crates/tetra-entities/src/lmac/lmac_bs.rs`
  - Uplink `NormalTrainSeq2/Block2` `TchS` is forwarded as raw 216-bit type-5 TCH/S with `raw_tch_s_block=Some(Block2)`.
  - Downlink `TchS` `blk2` with 216 bits is treated as already type-5 encoded and sent unchanged to PHY.
  - Full-slot `TchS` still decodes normally and bad CRC full-slot speech is still dropped.
- `crates/tetra-entities/src/umac/subcomp/circuit_mgr.rs`
  - Circuit TX queue now distinguishes normal ACELP from raw `TCH/S Block2`.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Raw `TCH/S Block2` is emitted as `STCH + TCH/S`, using an existing FACCH/STCH first half if present or a C-plane Null first half otherwise.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Raw `TCH/S Block2` is routed locally to the group/simplex/peer downlink and is not forwarded to Brew as ACELP.
- `crates/tetra-entities/src/net_brew/entity.rs`
  - Brew ignores raw half-slot payloads if one reaches it defensively.

Focused tests:

- `test_lmac_bs::bs_lmac_forwards_normal_seq2_block2_tch_s_as_raw_halfslot`
- `test_lmac_bs::bs_lmac_preserves_preencoded_raw_tch_s_block2_on_downlink`
- `test_umac_bs::test_group_ul_raw_block2_loopback_preserves_tch_s_halfslot`

Verification:

- `rustfmt --edition 2024` on touched files -> pass.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 5 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 43 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 116 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Current conclusion:

- The local component path now preserves valid `NormalTrainSeq2/Block2` TCH/S across LMAC -> UMAC -> LMAC instead of dropping it before `UMAC voice route`.
- This is clause-scoped ETSI hardening for traffic-mode half-slot TCH preservation. It is not formal certification.
- Live RF/audio is still required before claiming the group conversation issue is fixed in the test BS.

Next non-repeating execution:

1. Commit the raw TCH/S Block2 patch.
2. Build locally only with the Nexus-BS AArch64 command from build memory.
3. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; no binary backup.
4. Restart the test BS.
5. Live group test on GSSI `226333`:
   - `2260616` PTT, speak, release.
   - `2260082` PTT, speak, release.
   - `2260618` PTT if available, speak, release.
   - Repeat at least three alternating turns.
6. Required pass evidence:
   - `U-TX DEMAND` and compact `D-TX GRANTED` for each speaker.
   - `UMAC floor granted` source changes to the active speaker.
   - `rx_blk_traffic: forwarding raw TCH/S Block2` or `rx_blk_traffic: decoded valid TCH/S frame` appears after each grant.
   - `UMAC voice route` appears after each grant.
   - No `PTT denied`, `NotGranted`, `does not fit STCH`, or `UL inactivity timeout` during active overs.
   - Operator audio verdict confirms each subscriber can be heard by the others, not static.

## 2026-06-04 12:16:30 EEST - Deployed raw TCH/S Block2 build to test BS

Commit deployed:

- `d96db1c fix: preserve raw group traffic half slots`

Local verification before deploy:

- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 5 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 43 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 116 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Local build:

- Built locally on macOS only; no compilation was done on `chris@192.168.1.179`.
- Command shape: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Local binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs`
- Local SHA256: `43063625f0bd962c17b67175a0dc2e8ce32a524b65efef1f6935eb556f32c5b7`

Remote deploy:

- Host: `chris@192.168.1.179`
- Target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- Stopped the prior test BS/control-service from pidfile PIDs before copying.
- Deployed direct over the testing binary; no binary backup was created.
- Remote SHA256 matches local: `43063625f0bd962c17b67175a0dc2e8ce32a524b65efef1f6935eb556f32c5b7`
- Restarted with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`
- New remote processes:
  - control wrapper pid `17009`
  - control-service pid `17012`
  - nexus-bs pid `17014`
- Dashboard root returned Nexus-BS HTML.

Post-restart terminal state:

- `2260082` registered and affiliated to `[226333]`; RSSI about `-22 dBFS`.
- `2260616` registered and affiliated to `[226333]`; RSSI about `-23 dBFS`.
- Both `2260082` and `2260616` had BS-initiated EG3 assignment started, but T352 expired and the BS kept/fell back to `StayAlive`, so group PTT testing is not blocked by EG sleep.
- `2260618` was not present in the final concise post-deploy affiliate summary yet.
- MM still logs the pre-existing mixed `U-ATTACH/DETACH GROUP IDENTITY` reject for `group_report_response len=1 data=0` plus a GSSI list. Do not patch this under the group-audio fix unless the next log proves it is the active blocker.

Current live status:

- No post-`d96db1c` physical group PTT attempt has been captured yet.
- The BS is running the raw half-slot preservation build and is ready for live group test on GSSI `226333`.

Required next physical test:

1. `2260616` PTT on group `226333`, speak, release.
2. `2260082` PTT on group `226333`, speak, release.
3. Repeat at least three alternating turns.
4. If `2260618` appears/reattaches, add it as a third speaker.

Post-test log filter:

- `2260082|2260616|2260618|226333|U-TX DEMAND|D-TX GRANTED|FloorGranted|UMAC floor granted|rx_blk_traffic: forwarding raw TCH/S Block2|rx_blk_traffic: decoded valid TCH/S frame|UMAC voice route|FACCH stealing|preserving raw TCH/S|UL inactivity|PTT denied|NotGranted|does not fit STCH`

Pass criteria:

- Every speaker receives `D-TX GRANTED`.
- `UMAC floor granted` follows the active speaker.
- Either decoded full-slot TCH/S or raw `Block2` TCH/S reaches UMAC after each grant.
- `UMAC voice route` appears after each grant.
- No `PTT denied`, `NotGranted`, `does not fit STCH`, or active-over `UL inactivity timeout`.
- Operator audio verdict: each speaker is intelligible to the other group members.

## 2026-06-04 12:37:58 EEST - MM restart recovery accepts solicited complete group reports

Problem observed:

- After BS restart, terminals could show `Unit Not Attached`.
- Live logs showed `U-ATTACH/DETACH GROUP IDENTITY` from `2260082`/`2260616` carrying:
  - `group_identity_attach_detach_mode=true`
  - `group_report_response len=1 data=0`
  - `group_identity_uplink=[226333]`
- MM rejected that as a malformed mixed standalone request, which prevented coherent group re-affiliation after restart.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: `D-LOCATION UPDATE COMMAND` may request a group report; the MS may report/re-attach groups either in `U-LOCATION UPDATE DEMAND` or following `U-ATTACH/DETACH GROUP IDENTITY`.
- EN 300 392-2 clause 16.8.3: for SwMI-initiated group report, `U-ATTACH/DETACH GROUP IDENTITY` uses `not report request`, detach-all-then-attach for the first report PDU, and includes `group report complete` when all reported groups fit.
- EN 300 392-2 clause 16.8.2 remains enforced for unsolicited MS-initiated attach/detach: `group report response` must not be present.
- EN 300 392-2 clause 16.10.27a: `group_report_response` length 1 value 0 means complete; value 1 is reserved.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Added a per-ISSI local pending window for group reports solicited by `D-LOCATION UPDATE COMMAND(group_identity_report=true)`.
  - `U-LOCATION UPDATE DEMAND` now accepts `GroupIdentityLocationDemand` plus `group_report_response(1,0)` only for `DemandLocationUpdating`.
  - `U-ATTACH/DETACH GROUP IDENTITY` still rejects unsolicited mixed report-response + group-list PDUs, but accepts the same shape when a solicited group report is pending.
  - A BS-commanded DemandLocationUpdating response with no groups no longer immediately triggers a duplicate `D-LOCATION UPDATE COMMAND` while a follow-up group report is pending.
  - Registration is still not synthesized from standalone group attach; unknown ISSIs must pass the location-update path.

Focused tests:

- `test_restart_recovery_demand_location_update_accepts_complete_group_report_with_groups`
- `test_restart_recovery_accepts_solicited_attach_detach_group_report_completion`
- Existing unsolicited mixed reject tests still pass:
  - `test_mixed_group_report_response_and_attach_list_rejects_without_affiliation`
  - `test_mixed_group_report_response_and_mode_one_preserves_existing_groups`

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/src/mm/mm_bs.rs crates/tetra-entities/tests/test_mm_bs.rs` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 108 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this MM restart recovery patch.
2. Build Nexus-BS AArch64 locally only with the SoapySDR sysroot command from build memory.
3. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; no binary backup.
4. Restart the test BS and verify logs no longer show `Rejecting mixed U-ATTACH/DETACH GROUP IDENTITY` for the solicited restart-recovery report.
5. Confirm `2260082`, `2260616`, and any visible `2260618` register and affiliate to `226333` after restart.
6. Resume group-call audio hardening next: UMAC must gate raw/decoded TCH/S media by current floor owner/floor epoch to prevent stale-speaker static.

## 2026-06-04 12:41:48 EEST - Deployed MM solicited group-report fix to test BS

Commit deployed:

- `8981c33 fix: accept solicited restart group reports`

Local verification before deploy:

- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 108 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Local build:

- Built locally on macOS only; no compilation was done on `chris@192.168.1.179`.
- Command shape: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Local binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs`
- Local SHA256: `c103827eb5bce81ad5e340766178b130fdb9b54dbf90532b0df89a10fec8cf72`

Remote deploy:

- Host: `chris@192.168.1.179`
- Target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- Stopped the previous test BS/control-service from pidfile PIDs before copying.
- Deployed direct over the testing binary; no binary backup was created.
- Remote SHA256 matches local: `c103827eb5bce81ad5e340766178b130fdb9b54dbf90532b0df89a10fec8cf72`
- Restarted with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`
- New remote processes:
  - control wrapper pid `17434`
  - control-service pid `17437`
  - nexus-bs pid `17439`
- Dashboard root responds on configured port `8080`.

Live restart evidence:

- `2260082`:
  - `U-LOCATION UPDATE DEMAND(DemandLocationUpdating)` accepted.
  - Group `226333` accepted in `D-LOCATION UPDATE ACCEPT`.
  - Later solicited `U-ATTACH/DETACH GROUP IDENTITY` with `group_report_response len=1 data=0` and `gssi=226333` was accepted, ACKed, and re-affiliated.
- `2260616`:
  - Same solicited mixed group-report completion was accepted and ACKed.
  - CMCE received final `Affiliate` for `226333`.
- `2260618`:
  - Roaming update accepted with EG3 allocation and group `226333`.
  - CMCE received `Register` then `Affiliate`.
- `grep "Rejecting mixed U-ATTACH/DETACH GROUP IDENTITY"` returned no post-deploy entries.

Remaining observations:

- Startup still had short LLC retransmission bursts for `2260082`, `2260616`, and `2260618` before the final ACK/affiliate state settled.
- One startup PHY warning appeared: `Too late to produce TX block ...`; do not chase RF until a live RF symptom repeats after attach stability.
- Next protocol hardening target remains group/private call audio static: UMAC media should be gated by current CMCE floor owner/floor epoch, and stale queued raw TCH/S should be purged on floor transitions.

## 2026-06-04 12:47:05 EEST - UMAC purges stale group-call media on floor transitions

Problem targeted:

- Group call audio could become static when speakers alternate.
- Read-only UMAC/CMCE reviews identified that queued TCH/S media was per-timeslot only and was not purged on CMCE floor transitions.
- TMD media indications still do not carry source ISSI/floor epoch, so this patch does not claim full speaker-source validation.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: group request/grant/cease controls who may transmit.
- EN 300 392-2 clause 14.5.2.4: CMCE and MAC/UMAC must synchronize U-plane switching with traffic permission state.
- EN 300 392-2 clauses 23.8.4.1.4 and 23.8.5 remain the raw TCH/S half-slot preservation scope.

Patch implemented:

- `crates/tetra-entities/src/umac/subcomp/circuit_mgr.rs`
  - Added `clear_tx_data(ts)` to drop queued DL media blocks for a traffic slot.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added `clear_dl_media_queue(ts, reason)` wrapper with logging.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Drops UL TMD media when no UL circuit is active.
  - Drops UL TMD media during hangtime before refreshing `last_ul_voice` or routing to DL/Brew.
  - Drops media if the DL target timeslot is in hangtime.
  - Clears queued DL media on `FloorReleased`, `FloorGranted`, and `CallEnded`.

Focused tests:

- `test_group_ul_raw_block2_is_dropped_during_hangtime`
- `test_group_floor_release_purges_queued_raw_block2_media`
- `test_group_floor_grant_purges_stale_raw_block2_but_allows_new_media`

Verification:

- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 46 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 116 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 5 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit UMAC floor-transition media purge patch.
2. Build/deploy to test BS only after commit.
3. Run live alternating group PTT on GSSI `226333`:
   - `2260616` PTT/speak/release.
   - `2260082` PTT/speak/release.
   - `2260618` if available.
   - Repeat at least three turns.
4. Watch for `U-TX DEMAND`, `D-TX GRANTED`, `FloorGranted`, `FloorReleased`, raw/decoded TCH/S route, no `PTT denied`, and no stale/static audio.
5. If static persists, next required design change is extending TMD/CircuitTxBlock metadata with source/floor epoch; the current SAP cannot prove late media belongs to the current floor holder.

## 2026-06-04 12:50:07 EEST - Deployed UMAC stale-media purge build to test BS

Commit deployed:

- `32ee733 fix: purge stale UMAC media on floor changes`

Local verification before deploy:

- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 46 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 116 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 5 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Local build:

- Built locally on macOS only; no compilation was done on `chris@192.168.1.179`.
- Command shape: Nexus-BS AArch64 `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` with the SoapySDR AArch64 sysroot env from build memory.
- Local binary: `target/aarch64-unknown-linux-gnu/release/nexus-bs`
- Local SHA256: `d228a02bfbceee2e8ce4bb2975c39932f574152f0c241031b9e269a7bb7a98b1`

Remote deploy:

- Host: `chris@192.168.1.179`
- Target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- Stopped the prior test BS/control-service from pidfile PIDs before copying.
- Deployed direct over the testing binary; no binary backup was created.
- Remote SHA256 matches local: `d228a02bfbceee2e8ce4bb2975c39932f574152f0c241031b9e269a7bb7a98b1`
- Restarted with `/home/chris/nexus-bs-v0.1.55-test/start-test.sh`
- New remote processes:
  - control wrapper pid `17696`
  - control-service pid `17699`
  - nexus-bs pid `17701`

Post-restart terminal state:

- `2260082`: Register/Affiliate to `226333`, RSSI about `-23 dBFS`.
- `2260618`: Register/Affiliate to `226333`, RSSI about `-46 dBFS`.
- `2260616`: Register/Affiliate to `226333`, RSSI about `-25 dBFS`.
- Solicited group-report completion was accepted for all three:
  - `2260082`
  - `2260618`
  - `2260616`
- `grep "Rejecting mixed U-ATTACH/DETACH GROUP IDENTITY"` returned no post-deploy entries.

Required next physical test:

1. On group `226333`, have `2260616` PTT/speak/release.
2. Then `2260082` PTT/speak/release.
3. Then `2260618` PTT/speak/release if available.
4. Repeat at least three alternating turns.
5. Post-test log filter:
   - `2260082|2260616|2260618|226333|U-TX DEMAND|U-TX CEASED|D-TX GRANTED|D-TX CEASED|FloorGranted|FloorReleased|UMAC floor granted|UMAC voice route|dropped .*queued DL media|rx_blk_traffic: forwarding raw TCH/S Block2|rx_blk_traffic: decoded valid TCH/S frame|PTT denied|NotGranted|UL inactivity`
6. Pass requires operator audio verdict: each speaker intelligible to the other group members, not static.

## 2026-06-04 20:42:01 EEST - CMCE group D-SETUP speaker refresh before redeploy

Live symptom:

- User again reported static/no intelligible audio when the other station entered with PTT on group call.
- Current group context remains GSSI `226333`; recent logs include radios `2260082`, `2260616`, and stale `2260618`.

Log evidence:

- Live test BS still showed group traffic where voice was sometimes routed correctly:
  - `UMAC voice route: UL ts=2 bits=274 -> DL ts=2`
  - `UMAC voice route: UL ts=2 raw TCH/S Block2 bits=216 -> DL ts=2`
- The same live log also showed stale late-entry group setup:
  - `DSetup ... calling_party_address_ssi: Some(2260618)` while the active test context had moved to `2260082`/`2260616` on GSSI `226333`.
- This points at CMCE late-entry/back-up D-SETUP speaker coherence during floor handoff, not encryption.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.2.1.1 and 14.5.2.1.2: group `D-SETUP` carries the group-call setup/late-entry context.
- EN 300 392-2 clause 14.5.2.2.1: `D-TX GRANTED` moves transmit permission/floor.
- EN 300 392-2 clauses 23.5 and 23.8: assigned-channel TCH/S media must remain traffic, including FACCH/STCH stealing cases.
- This is clause-scoped hardening only; no formal certification claim.

Patch status:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added cached group D-SETUP speaker refresh helper.
  - Added immediate group D-SETUP refresh with channel allocation after floor changes.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - Repeated U-TX DEMAND from current speaker now reasserts the existing floor.
  - Floor handoff paths now refresh late-entry D-SETUP with the new speaker.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `226333` alternating PTT regression.
  - Added hangtime retake/queued handoff assertions that D-SETUP refresh uses the new speaker.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added UMAC/LMAC boundary regression proving that after `FloorReleased` then `FloorGranted`, UMAC schedules `ul_phy_chan=Tp` and LMAC decodes a valid full-slot TCH/S frame to `TmdCircuitDataInd`.

Verification so far:

- `cargo fmt` -> pass.
- `git diff --check` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 120 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 5 passed.
- `cargo check -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Commit the CMCE D-SETUP speaker refresh and UMAC/LMAC boundary test.
2. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Ask the operator to retest alternating group PTT on `226333` with `2260082` and `2260616`.
4. If static persists after this deploy, the next patch target is not cached D-SETUP; inspect whether repeated FACCH/STCH refresh steals too much audio and whether late `NormalTrainSeq2` arrivals occur while no active group floor is present.

## 2026-06-04 20:45:30 EEST - Deployed CMCE D-SETUP speaker refresh build to test BS

Commit deployed:

- `d518c03 fix: refresh group setup speaker on floor handoff`

Local verification before deploy:

- `cargo fmt` -> pass.
- `git diff --check` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 120 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 5 passed.
- `cargo check -p tetra-entities --locked` -> pass.

Deploy:

- Command: `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Build was local on macOS only; no Rust/TETRA compile was done on `chris@192.168.1.179`.
- Remote target: `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`
- No binary backup was created.
- Build line: `Build: v0.1.55-d518c03c`
- Remote SHA256: `cf022002aabcafcf85ee4299cfb3686020707bd9928d61e5f338f5bc5ac19143`
- Remote processes after restart:
  - control wrapper pid `21076`
  - control-service pid `21079`
  - nexus-bs pid `21081`

Post-restart terminal state:

- `2260082`: CMCE register then affiliate to `226333`.
- `2260618`: CMCE register then affiliate to `226333`.
- `2260616`: CMCE register then affiliate to `226333` twice during startup settling.

Required next physical test:

1. Retest alternating group PTT on `226333` between `2260082` and `2260616`.
2. Verify each entry gets first-try transmit permission and intelligible audio, not static.
3. If static repeats, capture the 30 seconds around the event and inspect:
   - stale `DSetup ... calling_party_address_ssi`
   - `FACCH stealing ... speech_present=false` density during talk spurts
   - `rx_tpsap_prim got NormalTrainSeq2` without `UMAC voice route`
   - any `PTT denied`, `NotGranted`, `Service unavailable`, or `UL inactivity`

## 2026-06-04 20:58:20 EEST - Group UL inactivity grants queued requester

Problem targeted:

- Post-deploy logs showed `UL inactivity timeout on ts=2` during group-call testing.
- Before this patch, the group-call timeout path forced `D-TX CEASED`/hangtime even when another MS already had a queued U-TX DEMAND. That can make the waiting MS need a second PTT attempt.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: group floor request/grant/cease controls who may transmit.
- The local UL inactivity guard is treated as the BS-side cease event. If a valid requester was already queued, SwMI grants that requester immediately instead of requiring another demand.
- Clause-scoped hardening only; no formal certification claim.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Late-entry timer D-SETUP resend now re-derives group `calling_party_address_ssi` from `active_calls[call_id].source_issi` before serialization.
  - Group UL inactivity timeout now:
    - filters queued requester by current group affiliation,
    - grants queued requester with individual `D-TX GRANTED`,
    - sends group FACCH `D-TX GRANTED`,
    - refreshes group D-SETUP with the new speaker,
    - emits UMAC/Brew `FloorGranted`.
  - If no valid requester is queued, old behavior remains: enter hangtime, send `D-TX CEASED`, emit `FloorReleased`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - Made the existing compact group individual-grant helper visible inside the CC-BS module for timer reuse.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_group_ul_inactivity_hands_floor_to_queued_requester` for GSSI `226333`.

Verification:

- `cargo fmt` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_ul_inactivity_hands_floor_to_queued_requester --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 121 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 5 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this timeout handoff patch.
2. Deploy direct to test BS with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Retest alternating group PTT on `226333`; expected behavior is first queued PTT becomes speaker after timeout/cease without a second press.

## 2026-06-04 21:21:02 EEST - Private-call shutdown hardening for Motorola restart symptom

Problem targeted:

- Field report: private call voice now works, but when the opposite party closes the private call, a Motorola terminal restarts and re-attaches.
- Live log evidence showed `U-DISCONNECT` for private `call_id=4`, then `D-DISCONNECT` to the peer, followed by a very short local fallback to `D-RELEASE` and circuit close while FACCH/STCH repeats were still in flight.
- CMCE auditor also found a spec-order risk: after peer receives `D-DISCONNECT`, EN 300 392-2 expects peer `U-RELEASE` and local call clearing, so sending final `D-RELEASE` again to that same peer can double-clear the call context.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.3.1: an MS initiating disconnection sends `U-DISCONNECT` and waits for `D-RELEASE`.
- EN 300 392-2 clauses 14.5.1.3.2/14.5.1.3.3 and 14.7.1.6/14.7.1.9: BS uses `D-DISCONNECT` to request peer release, peer responds with `U-RELEASE`, and `D-RELEASE` informs that the connection has been released.
- EN 300 392-2 clause 14.5.1.2.1: simplex private floor control must not race call clearing with new `D-TX GRANTED` / `D-TX CEASED` signalling.
- This is clause-scoped hardening only; no formal certification claim.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/call.rs`
  - `DisconnectPending` now tracks both `awaiting_release_from` and `release_to_issi`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Active-call `D-DISCONNECT` now uses assigned-channel `UlDlAssignment::Dl`.
  - Local delivery guards for private `D-DISCONNECT` and `D-RELEASE` increased from 16 timeslots to 2 seconds, so FACCH/STCH repeats are not cut short.
  - Added targeted final release: after peer `U-RELEASE`, final `D-RELEASE` is sent only to the original `U-DISCONNECT` initiator, not to the peer that already cleared after `D-DISCONNECT`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Peer `U-RELEASE` response guard increased from 16 timeslots to 5 seconds before fallback `D-RELEASE`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Suppresses private `U-TX DEMAND` / `U-TX CEASED` while `D-DISCONNECT` delivery is pending.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated caller/called private disconnect assertions for one-leg final `D-RELEASE`.
  - Added `test_p2p_pending_disconnect_delivery_suppresses_floor_pdus`.
  - Hardened fallback timeout tests to prove no early close before the longer delivery guards.

Verification:

- `rustfmt --edition 2024` on touched CMCE/test files -> pass.
- `git diff --check` on touched files -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_pending_disconnect_delivery_suppresses_floor_pdus --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_u_disconnect_delivery_guard_falls_back_to_release_without_peer_wait --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_pending_disconnect_closes_after_bounded_timeout --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_active_p2p_discarded_release_reporters_do_not_close_before_guard_timeout --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 122 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.

Next non-repeating execution:

1. Commit this private-call shutdown patch.
2. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Retest private simplex call between `2260082` and `2260616`/`2260618`:
   - voice in both directions,
   - close from caller,
   - close from called party,
   - no Motorola restart/re-attach after remote hangup.
4. If restart persists, capture the 20 seconds around hangup and inspect `D-DISCONNECT`, peer `U-RELEASE`, final `D-RELEASE`, and circuit close order.

## 2026-06-04 21:29:51 EEST - Private simplex hangup `No answer` follow-up

Problem targeted:

- Field report after build `v0.1.55-78d4644a`: Motorola showed `No answer` at the end of a private simplex call.
- Live log around `21:24:00` showed `U-DISCONNECT` from `2260616`, BS `D-DISCONNECT` to peer `2260618`, but no peer `U-RELEASE`.
- At `21:24:06` the local pending-disconnect guard fired and sent fallback `D-RELEASE` to both legs. This explains a timeout-like terminal UI instead of a clean release handshake.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.3.1: the MS initiating disconnection sends `U-DISCONNECT` and waits for `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3: an MS receiving `D-DISCONNECT` shall respond with `U-RELEASE`; an MS receiving `D-RELEASE` sends no response.
- EN 300 392-2 clause 14.7.1.6: `D-DISCONNECT` response expected is `U-RELEASE`.
- EN 300 392-2 clause 14.7.1.9: `D-RELEASE` response expected is none.
- This is clause-scoped hardening only; no formal certification claim.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Active private-call `D-DISCONNECT` now uses assigned-channel `UlDlAssignment::Both`, because that PDU explicitly expects the MS uplink `U-RELEASE` response.
  - Final private-call `D-RELEASE` remains downlink-only (`UlDlAssignment::Dl`) through the existing release path, because no MS response is expected.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated the three private-call disconnect tests to assert response-capable `D-DISCONNECT` channel allocation in both caller-hangs-up and called-party-hangs-up directions.
  - Existing helper coverage still asserts final `D-RELEASE` FACCH/STCH remains `Dl`.

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 122 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this private simplex hangup patch.
2. Deploy direct with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Retest private simplex between `2260082` and `2260616`/`2260618`:
   - peer should answer `D-DISCONNECT` with `U-RELEASE`,
   - initiator should receive final one-leg `D-RELEASE`,
   - no `Pending individual D-DISCONNECT timed out`,
   - no Motorola `No answer` at normal hangup.
4. If the peer still does not send `U-RELEASE`, inspect on-air MAC STCH allocation bits and raw decoded `MacResource` for the `D-DISCONNECT` slot.

## 2026-06-04 21:34:15 EEST - Private disconnect collision semantics tightened

Problem targeted:

- Agent CMCE audit found that BS accepted peer `U-DISCONNECT` as if it were the `U-RELEASE` acknowledgement to a pending private-call `D-DISCONNECT`.
- That is too loose for ETSI call clearing: it can hide a missing `U-RELEASE` and complete the wrong state transition.

ETSI clause scope:

- EN 300 392-2 clause 14.7.1.6: `D-DISCONNECT` response expected is `U-RELEASE`.
- EN 300 392-2 clause 14.7.2.4: `U-DISCONNECT` is an MS request to disconnect a call and expects `D-DISCONNECT`/`D-RELEASE`; it is not the acknowledgement to `D-DISCONNECT`.
- EN 300 392-2 clause 14.7.2.9: `U-RELEASE` is the acknowledgement to `D-DISCONNECT`.
- EN 300 392-2 clause 14.5.1.3.5: in colliding disconnection, the MS shall respond to incoming `D-DISCONNECT` as in clause 14.5.1.3.3, i.e. with `U-RELEASE`.
- This is clause-scoped hardening only; no formal certification claim.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Removed the two branches that allowed `U-DISCONNECT` from the awaited peer to complete pending `D-DISCONNECT` delivery or pending `DisconnectPending` state.
  - Pending private disconnect now ignores peer `U-DISCONNECT` and continues waiting for real `U-RELEASE` or bounded fallback timeout.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_p2p_peer_u_disconnect_does_not_ack_pending_d_disconnect`.
  - The test proves peer `U-DISCONNECT` does not trigger final `D-RELEASE`; peer `U-RELEASE` remains the completing PDU.

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_peer_u_disconnect_does_not_ack_pending_d_disconnect --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 123 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit and deploy direct to test BS.
2. Retest private simplex hangup. Expected clean log is `U-DISCONNECT` -> `D-DISCONNECT` -> peer `U-RELEASE` -> final one-leg `D-RELEASE`, with no fallback timeout and no terminal `No answer`.
3. If the peer sends `U-DISCONNECT` instead of `U-RELEASE`, treat that as a remaining terminal/protocol collision signal to inspect on-air delivery, not as a successful acknowledgement.

## 2026-06-04 22:19:43 EEST - Private simplex bearer-tail drain before peer clear

Problem targeted:

- Field test after build `v0.1.55-5acff30d`: private simplex call `2260616 -> 2260618` worked, but ending the call caused peer `2260618` Motorola MXP600 to soft reboot.
- Live log showed private-call teardown tightly adjacent to recent speech/floor signalling:
  - `U-TX CEASED` from a private-call participant was followed immediately by `D-TX CEASED` to both legs.
  - Later `U-DISCONNECT` was followed immediately by prompt initiator `D-RELEASE` plus peer `D-DISCONNECT`.
- The previous prompt `D-RELEASE` fix is still required to avoid terminal `No answer`; the remaining risk is peer-facing clear/cease signalling being sent before the traffic bearer has drained.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: simplex individual floor control uses `U-TX DEMAND`, `D-TX GRANTED`, `U-TX CEASED`, and `D-TX CEASED`; unsolicited peer grants remain forbidden.
- EN 300 392-2 clauses 14.5.1.3.1/14.5.1.3.3: a private-call disconnect initiator waits for `D-RELEASE`; the peer is cleared by `D-DISCONNECT -> U-RELEASE`.
- EN 300 392-2 clause 23.8.5: for N=4/8 circuit-mode data, after `U-TX CEASED` or `U-DISCONNECT` from the transmitting MS, BS should issue N-1 traffic slots containing tail bits before `D-TX CEASED`, `D-RELEASE`, or `D-DISCONNECT` to receiving MS(s).
- The implemented guard applies the same short N=4-equivalent drain to current simplex speech as a conservative Motorola/bearer-tail compatibility guard because CMCE does not yet expose bearer interleaving depth. This is clause-scoped hardening, not formal certification evidence.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/mod.rs`
  - Added pending tail-drain state for simplex private `U-TX CEASED` and `U-DISCONNECT`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added `INDIVIDUAL_SIMPLEX_TAIL_DRAIN_TIMESLOTS = 12` (`N=4`, `N-1=3` traffic-frame recurrences).
  - Added drain handlers that delay peer-facing `D-TX CEASED` and peer `D-DISCONNECT`.
  - If `U-DISCONNECT` arrives while a same-speaker `U-TX CEASED` drain is pending, the pending `D-TX CEASED` is cancelled and peer clear uses the original drain start time.
  - Prompt `D-RELEASE` to the disconnecting MS remains immediate.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - `U-TX CEASED` with no queued requester now starts tail drain before sending `D-TX CEASED`.
  - If a requester was already queued before `U-TX CEASED`, the ETSI handoff path with `D-TX GRANTED` remains immediate.
  - Floor requests and duplicate disconnects are suppressed while private disconnect clear is pending.
  - `U-DISCONNECT` from the current simplex floor holder sends initiator `D-RELEASE` promptly, then delays peer `D-DISCONNECT` until the tail drain expires.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - CMCE tick now drains private simplex tail queues before normal release/delivery timeout handling.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated private simplex workflow tests to assert prompt initiator `D-RELEASE`, no immediate peer `D-DISCONNECT`, and tail-drained peer clear.
  - Updated idle-floor tests to wait for delayed `D-TX CEASED`.
  - Kept queued floor-handoff tests immediate via `D-TX GRANTED`.

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/src/cmce/subentities/cc_bs/mod.rs crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 61 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 123 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 9 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit and deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
2. Retest exact Motorola case: private simplex `2260616 -> 2260618`, talk both ways, then close with red button.
3. Expected live log:
   - no immediate peer `D-DISCONNECT` in the same tick as floor-holder `U-DISCONNECT`,
   - prompt `D-RELEASE` to the disconnecting ISSI,
   - peer `D-DISCONNECT` after the short tail drain,
   - peer `U-RELEASE`,
   - no fallback timeout,
   - no MXP600 soft reboot and no `No answer`.
4. If MXP600 still reboots, inspect whether it is the `D-RELEASE` recipient or the peer `D-DISCONNECT` recipient in that exact trace before changing sequencing again.

## 2026-06-04 22:22:30 EEST - Private simplex tail-drain test build deployed

Deployment:

- Committed patch: `a3bc407 fix: tail-drain private simplex clear`.
- Deployed with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Remote binary SHA-256: `33d97d5e722fb357423c5c5baa355ad3de060f4926ec95d39dbe6aa59f37eea1`.
- Remote build banner: `Build: v0.1.55-a3bc4078`.
- Test service path: `/home/chris/nexus-bs-v0.1.55-test`.

Post-start live state:

- `nexus-bs` running with `/home/chris/nexus-bs-v0.1.55-test/config.live.toml`.
- `nexus-bs-control-service` running on `127.0.0.1:9002`.
- Post-start log showed:
  - `2260616` registered and affiliated to `226333`.
  - `2260082` registered and affiliated to `226333`.
  - `2260618` registered and affiliated to `226333`.

Next non-repeating execution:

1. User retests private simplex `2260616 -> 2260618`.
2. Watch logs for `U-TX CEASED`, tail-drain debug/info, prompt `D-RELEASE`, delayed peer `D-DISCONNECT`, peer `U-RELEASE`, and absence of fallback timeout.
3. If MXP600 still soft reboots, capture exact recipient of the last downlink PDU before reattach.

## 2026-06-04 22:49:59 EEST - Private simplex peer-floor clear uses D-RELEASE

Problem targeted:

- Field retest: private simplex `2260616 -> 2260618` voice worked, but when `2260616` ended the call with the red key, peer `2260618` Motorola MXP600 soft rebooted.
- The risky case is peer-floor shutdown: the peer may still be the current simplex floor holder while the disconnecting MS receives prompt `D-RELEASE`.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.3.1: after `U-DISCONNECT`, the disconnecting MS waits for `D-RELEASE`; the SwMI should inform the other MS of call clearance either by `D-DISCONNECT` or by `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3: `D-DISCONNECT` requires `U-RELEASE`, while `D-RELEASE` requires no response.
- EN 300 392-2 clause 23.8.5: after `U-TX CEASED` or `U-DISCONNECT` from a transmitting MS, BS should drain `N-1` traffic slots before sending `D-TX CEASED`, `D-RELEASE`, or `D-DISCONNECT` to receiving MSs.
- This is clause-scoped hardening plus a bounded Motorola compatibility guard; it is not formal certification evidence.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/mod.rs`
  - Added `IndividualDisconnectPeerClear::{Disconnect, Release}` and peer-clear reporter state for pending private disconnects.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Tail-drained private disconnect can now complete peer clear with `D-RELEASE` instead of `D-DISCONNECT`.
  - Peer `D-RELEASE` is reporter-tracked; the traffic circuit closes only after both the prompt initiator `D-RELEASE` and peer `D-RELEASE` transmit, or after the bounded local delivery guard.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - If a simplex private peer is the current floor holder and the other MS disconnects, BS now tail-drains then sends peer `D-RELEASE`.
  - Duplicate `U-DISCONNECT`, `U-TX DEMAND`, and `U-TX CEASED` are suppressed while the peer-release clear is pending.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated MXP600 field regression to require zero peer `D-DISCONNECT` and a tail-drained peer `D-RELEASE`.
  - Updated symmetric called-party disconnect test for floor-holder peer release.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_caller_disconnect_tail_drains_when_mxp600_peer_holds_floor --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_called_party_u_disconnect_waits_for_caller_release_before_circuit_close --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 62 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 124 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 47 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 9 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit and deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
2. Retest private simplex `2260616 -> 2260618`; make `2260618` talk last, then close from `2260616`.
3. Expected live behavior: prompt `D-RELEASE` to `2260616`, no `D-DISCONNECT` to `2260618`, tail-drained `D-RELEASE` to `2260618`, no MXP600 soft reboot.
4. Continue SDS/LLC hardening next: SDS status-preserving Brew forward and bounded LLC duplicate suppression.

## 2026-06-04 22:52:49 EEST - Private peer-floor D-RELEASE build deployed

Deployment:

- Committed patch: `5f03000 fix: release private peer floor holder`.
- Deployed with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Remote binary SHA-256: `cc58d4a85adb9cc096b16d885b41d2b6ed9fef8f7947007dcea3ed31cb0f2b3f`.
- Remote build banner: `Build: v0.1.55-5f03000c`.
- Test service path: `/home/chris/nexus-bs-v0.1.55-test`.

Post-start live state:

- `nexus-bs` running with `/home/chris/nexus-bs-v0.1.55-test/config.live.toml`.
- `nexus-bs-control-service` running on `127.0.0.1:9002`.
- Post-start log showed:
  - `2260618` registered and affiliated to `226333`.
  - `2260616` registered and affiliated to `226333`.
  - `2260082` registered and affiliated to `226333`.

Next non-repeating execution:

1. User retests private simplex `2260616 -> 2260618`.
2. Expected log around hangup: `U-DISCONNECT` from `2260616`, prompt `D-RELEASE` to `2260616`, no `D-DISCONNECT` to `2260618`, tail-drained peer `D-RELEASE` to `2260618`, circuit close only after D-RELEASE reporter completion or bounded local guard.
3. If MXP600 still reboots, inspect the last 20 seconds of `2260618` downlink and registration log before making another protocol change.

## 2026-06-04 23:53:42 EEST - P2P/group floor-control BL-UDATA repeat guard

Problem targeted:

- Live private simplex `2260618 -> 2260616` at `23:37:36` reached `U-CONNECT` and opened the traffic bearer.
- First speaker `2260618` sent voice normally on ts=2.
- At `23:37:49.500`, `2260618` sent `U-TX CEASED`; CMCE tail-drained and sent `D-TX CEASED` to both MSs.
- At `23:37:49.952`, `2260616` sent `U-TX DEMAND` and CMCE granted the floor, but stale `D-TX CEASED` BL-UDATA repetitions were still interleaved after the new `D-TX GRANTED`.
- Result: no TCH/S voice followed from `2260616`, UMAC timed out at `23:37:53.012`, and later `U-TX CEASED` from `2260616` was ignored because CMCE had already cleared the floor.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1 b/e: simplex individual-call floor control uses `U-TX DEMAND`, `D-TX GRANTED`, `U-TX CEASED`, and `D-TX CEASED`; a queued handover may grant the next MS without a separate `D-TX CEASED`.
- EN 300 392-2 clause 14.5.2.2.1: group-call floor control uses the same request/grant/cease pattern for one speaker at a time.
- EN 300 392-2 clause 22.3.2.4.1 and Annex A.2: for unacknowledged BL-UDATA, `N.253 + 1` complete transmissions are sent; an explicit `N.253=0` means one complete transmission.
- This patch is clause-scoped hardening of time-sensitive floor-control delivery; it is not formal ETSI certification evidence.

Patch implemented:

- `crates/tetra-saps/src/lcmc/mod.rs`
  - Added optional `LcmcMleUnitdataReq.unacked_bl_repetitions`.
- `crates/tetra-entities/src/mle/mle_bs.rs` and `crates/tetra-entities/src/mle/mle_ms.rs`
  - Pass CMCE's explicit unacknowledged BL repetition request through to LLC as `n_tlsdu_repeats`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Private simplex `D-TX GRANTED` and `D-TX CEASED` now request `N.253=0`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs` and `shared.rs`
  - Group FACCH `D-TX GRANTED`, `D-TX INTERRUPT`, and `D-TX CEASED` now request `N.253=0`.
- Other CMCE/SDS/MM-originating LCMC messages keep `None`, so setup/release/status retain the existing LLC default repetition behavior.

Verification:

- `cargo fmt --all` -> pass.
- `cargo test -p tetra-entities --test test_mle_bs lcmc_unacknowledged --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs simplex_p2p --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group --locked` -> 49 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 125 passed.
- `cargo test -p tetra-entities --test test_mle_bs --locked` -> 27 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 80 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this floor-control repeat guard.
2. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Retest private simplex both directions on `2260616`/`2260618`: expected live log after reverse PTT is `D-TX GRANTED` followed by TCH/S voice, with no stale post-grant `D-TX CEASED` repeats to the newly granted MS.
4. Retest group `226333` alternating PTT between two terminals: expected no first-return `PTT denied`, no stale `D-TX CEASED` after grant, and no static-only talk spurt.

## 2026-06-05 00:19:24 EEST - Floor-control repeat guard deployed to RF test BS

Deployment:

- Committed patch: `51f8eb8 fix: single-shot floor-control BL-UDATA`.
- Deployed direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Remote binary SHA-256: `1f669e251b134a0af947fe796eb1a6313e8f8ea40318bbf74ff0ffe7f2e13d55`.
- Remote build banner: `Build: v0.1.55-51f8eb8f`.
- Remote processes after restart: `nexus-bs-control-service` on `127.0.0.1:9002`, `nexus-bs` with `/home/chris/nexus-bs-v0.1.55-test/config.live.toml`.

Post-start live state:

- `2260618` registered and affiliated to `226333`.
- `2260082` registered and affiliated to `226333`.
- `2260616` registered and affiliated to `226333`.
- A bounded 140 s live tail after deploy saw no new `U-SETUP`, `U-TX DEMAND`, `PTT denied`, `Service unavailable`, or `Unit Not Attached` event, so no RF private-simplex retest has been observed in the post-deploy log yet.

Next non-repeating execution:

1. Run private simplex RF retest `2260616 <-> 2260618`, both PTT directions.
2. Inspect the post-deploy log for the first reverse-PTT sequence: expected `U-TX DEMAND` then `D-TX GRANTED` then TCH/S voice, with no stale `D-TX CEASED` emitted after the new grant.
3. If P2P still fails, patch the next proven layer only: likely UMAC bearer/speaker gating or CMCE floor-holder state, not another LLC repetition change without fresh log evidence.
4. Then retest group `226333` alternating PTT for the same stale floor-control pattern.

## 2026-06-05 00:32:46 EEST - P2P simplex crossed-timeslot floor media cleanup

Problem targeted:

- User reported current P2P simplex is broken after the floor-control repeat guard deployment.
- The live post-deploy log still showed no `USetup`/`UTxDemand` P2P sequence in the current process log; a bounded 90 s tail saw only broadcasts, Brew deregisters, and one isolated TCH burst.
- Code inspection found a missing UMAC case for local P2P simplex when the two MSs are on separate assigned timeslots: floor release/grant cleanup only cleared the source UL timeslot, while downlink speech for that source is queued on the crossed peer timeslot.
- That can leave old-speaker raw TCH/S queued on the peer DL timeslot across `D-TX CEASED`/`D-TX GRANTED`, matching the static/no-voice symptom class.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1 b/e: private simplex request-to-transmit/floor release must switch the single authorized speaker cleanly.
- EN 300 392-2 clause 23.5: assigned traffic channels carry FACCH/STCH signalling during floor control.
- EN 300 392-2 clause 23.8.5: TCH/S media timing/half-slot handling must not be carried across an obsolete floor epoch.
- This is clause-scoped engineering hardening and test evidence, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added `floor_media_timeslots(ts)` to include the peer timeslot for local P2P cross-route circuits.
  - `FloorReleased` now clears DL media, enters hangtime, clears UL inactivity state, and clears current STCH speaker state on both the source and crossed peer timeslots.
  - `FloorGranted` now clears stale media and exits hangtime on both affected timeslots before accepting the new floor holder.
  - `CallEnded` now clears both affected timeslots for crossed P2P circuits.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_private_simplex_cross_route_floor_release_purges_peer_dl_media`.
  - Added `test_private_simplex_cross_route_floor_grant_keeps_new_peer_audio`.
- `crates/tetra-saps/src/control/call_control.rs` and `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Updated internal comments: `peer_ts` is for local P2P cross-routing, including simplex calls on separate assigned timeslots, not only duplex.

Verification:

- `cargo fmt --all` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_cmce_bs simplex_p2p --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 49 passed.
- `cargo check -p tetra-saps -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this UMAC crossed-timeslot cleanup.
2. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Retest private simplex `2260616 <-> 2260618`: expected reverse PTT has `U-TX DEMAND`, `D-TX GRANTED`, then TCH/S routed to the peer TS with no old raw TCH/S from the previous floor.
4. If P2P setup still does not appear in the log, instrument/inspect the MAC/LLC decode path before changing CMCE floor semantics again.

## 2026-06-05 00:34:40 EEST - Crossed P2P media cleanup deployed to RF test BS

Deployment:

- Committed patch: `82297b5 fix: clear crossed P2P floor media`.
- Deployed direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Remote binary SHA-256: `8317e81208d92ef5e4ec7839e2ee1037bcd3a0f5117b772884e7ea8614bebb67`.
- Remote build banner: `Build: v0.1.55-82297b54`.
- Remote processes after restart: `nexus-bs-control-service` on `127.0.0.1:9002`, `nexus-bs` with `/home/chris/nexus-bs-v0.1.55-test/config.live.toml`.

Post-start live state:

- `2260618` registered and affiliated to `226333`.
- `2260616` registered and affiliated to `226333`.
- `2260082` registered and affiliated to `226333`.
- A bounded post-deploy tail for P2P/floor-control/audio-route events expired without observing a new private simplex attempt.

Next non-repeating execution:

1. RF retest private simplex both directions between `2260616` and `2260618`.
2. Expected on reverse PTT: `U-TX DEMAND`, `D-TX GRANTED`, `UMAC floor granted`, then `UMAC voice route` from granted UL TS to peer DL TS.
3. If terminal still shows PTT denied or no P2P setup appears, collect a fresh bounded log around the attempt and inspect MAC/LLC decode before another CMCE/LLC semantic patch.

## 2026-06-05 00:42:56 EEST - UMAC invalid TCH/S no longer refreshes floor voice timer

Problem targeted:

- Current post-deploy RF log still has no new private simplex setup/floor-control sequence after `82297b5`.
- Code inspection showed UMAC refreshed `last_ul_voice` immediately after any `TmdCircuitDataInd` on an active UL circuit, before validating that the media was a supported TCH/S payload and before scheduling it to downlink or forwarding it to Brew.
- Unsupported UL voice could therefore mask the BS-side inactivity timeout for a simplex private floor holder and keep floor state alive while no valid speech was delivered.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: simplex private floor ownership must be released/handoff-driven when the current speaker stops transmitting valid speech.
- EN 300 392-2 clauses 23.8.3 and 23.8.5: bad/unsupported TCH/S media must not be treated as clean speech on the downlink path.
- This is clause-scoped engineering hardening and test evidence, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Validates UL media before treating it as voice activity.
  - For full-slot ACELP, accepts only payloads that `pack_ul_acelp_bits` can pack and forwards to Brew only after validation.
  - For raw TCH/S half-slot media, accepts only `Block2` with 216 bits.
  - Refreshes `last_ul_voice` only after valid media is actually delivered to Brew or scheduled to a downlink circuit.
  - Refreshes the peer timeslot timer for crossed P2P only after successful media delivery.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_unsupported_ul_voice_does_not_refresh_inactivity_timer`.

Verification:

- `cargo fmt --all` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs unsupported_ul_voice --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 50 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this UMAC invalid-media timer guard.
2. Decide deploy after commit: deploy if we want the RF BS to include this guard before the next private/group PTT retest.
3. Continue next hardening candidate if no RF retest evidence arrives: CMCE Brew-routed private simplex initial `floor_holder`, or UMAC media admission requiring explicit floor epochs for local-loopback traffic.

## 2026-06-05 01:05:00 EEST - CMCE Brew-routed private simplex initial floor hardening

Live diagnostic:

- Remote test BS is running `Build: v0.1.55-2cef71d3`.
- The current log after restart shows `2260082`, `2260616`, and `2260618` re-registering/affiliating to `226333`, plus one `UFacility` from `2260618`; no fresh `U-SETUP`, `U-TX DEMAND`, `D-TX GRANTED`, `UMAC voice route`, or P2P media sequence was present in the bounded log search.
- Live config has `call_preemptive = false`; no private/group pre-emption was enabled.

Problem targeted:

- Code audit found a real CMCE gap on Brew-routed private simplex paths: the call became active and UMAC opened a SwMI-backed bearer, but CMCE did not seed `floor_holder` from the `D-CONNECT` / `D-CONNECT-ACKNOWLEDGE` transmission grant.
- With `floor_holder=None`, a granted local MS could later send `U-TX CEASED` and have it ignored, preventing clean floor release/tail-drain behavior. This can affect P2P simplex when a local destination is temporarily not recognized as local and the setup falls through to Brew routing.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1: simplex private-call transmission permission is controlled by the SwMI, and `U-TX CEASED` must be handled for the MS that owns the floor.
- EN 300 392-2 tables 14.80 and 14.81: `D-CONNECT` / `D-CONNECT-ACKNOWLEDGE` transmission grant and request permission must drive the initial transmit permission state.
- This is clause-scoped engineering hardening and test evidence, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/network.rs`
  - Added `network_circuit_grant()` to decode Brew/TetraPack grant values through the CMCE `TransmissionGrant` enum.
  - Added `apply_brew_simplex_initial_floor()` to seed `floor_holder` for Brew-routed private simplex calls from the actual connect grant.
  - Local-origin Brew private connect now preserves `call_info.grant` and `call_info.permission` in `D-CONNECT` and the Brew connect confirm instead of hardcoding granted/no-permission.
  - Network-origin Brew private connect confirm now seeds local floor state after opening the SwMI bearer.
  - `FloorGranted` is emitted to UMAC only when the local MS is the granted initial speaker.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_local_origin_brew_private_simplex_connect_sets_initial_floor`.
  - Added `test_network_origin_brew_private_simplex_connect_confirm_sets_initial_floor`.

Verification:

- `cargo fmt --all` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs brew_private_simplex --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs simplex_p2p --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 127 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit the CMCE Brew private simplex floor patch.
2. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Confirm remote build banner and SHA.
4. Retest private simplex `2260616 <-> 2260618`. If no `U-SETUP` appears, diagnose MAC/LLC decode before changing floor semantics again.

## 2026-06-05 01:09:30 EEST - MM recovery preserves subscriber state during failed accept reprobe

Live diagnostic:

- Remote test BS was running `Build: v0.1.55-dd16dae5`.
- The bounded log search after the build marker found no private simplex call-control sequence: no `U-SETUP`, `D-SETUP`, `U-CONNECT`, `D-CONNECT`, `U-TX`, `D-TX`, `FloorGranted`, or `UMAC voice route`.
- The same log showed `2260616` reappearing on RF with good RSSI and MAC access, then being deaffiliated/deregistered after a failed `D-LOCATION UPDATE ACCEPT` transfer report, then soft re-attaching shortly after.
- This means the observed "broken P2P simplex" path was blocked before CMCE private-call setup: MM/LLC recovery temporarily removed a live terminal from CMCE subscriber routing.

Problem targeted:

- `mark_registration_unconfirmed_and_reprobe()` sent a new `D-LOCATION-UPDATE-COMMAND` after failed delivery of an acknowledged `D-LOCATION UPDATE ACCEPT`, but also emitted `Deaffiliate` and `Deregister` and removed the shared subscriber.
- That made CMCE forget the ISSI and its GSSI during the recovery window even while the MS was still transmitting MAC access and later ACKs. A private or group PTT during that window could be rejected as not attached/not affiliated.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: the SwMI may initiate a location update with `D-LOCATION-UPDATE-COMMAND`; the command is the recovery procedure, not proof that the existing MS context must be torn down immediately.
- EN 300 392-2 clauses 16.9.2.8 and 16.9.3.4: `DemandLocationUpdating` is the MS response path for BS-initiated location update; subscriber routing should remain coherent until a detach, reject, timeout, or completed replacement update says otherwise.
- This is clause-scoped engineering hardening and test evidence, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - `mark_registration_unconfirmed_and_reprobe()` still fails open to `StayAlive`, sends `D-LOCATION-UPDATE-COMMAND`, abandons stale pending group transactions, and marks the command pending.
  - It no longer emits immediate `Deaffiliate`/`Deregister` or removes the shared subscriber during the reprobe window.
  - It logs that provisional subscriber state is preserved while the registration reprobe is pending.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Updated `test_restart_recovery_failed_location_update_accept_reprobes_registration` to assert that a failed accept transfer reprobes without dropping CMCE/Brew subscriber routing.
  - The test now verifies that ISSI registration and GSSI affiliation survive the reprobe, while energy saving is cleared to StayAlive.

Verification:

- `cargo fmt --all` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery_failed_location_update_accept_reprobes_registration --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs simplex_p2p --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 112 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit and deploy this MM recovery hardening directly to the test BS.
2. Confirm the remote build banner and watch for `2260616` recovery without CMCE deregister/deaffiliate churn.
3. Retest private simplex only after a fresh `U-SETUP -> D-CONNECT -> U-TX/D-TX -> UMAC voice route` sequence appears in the log.

## 2026-06-05 09:23:15 EEST - CMCE private simplex initial floor aligned with ETSI raw request bit

Live diagnostic:

- The recent private simplex log for `2260616 -> 2260618` showed `U-SETUP` with `hook_method_selection=true` and `request_to_transmit_send_data=true`, then `D-SETUP` with `transmission_grant=NotGranted`.
- The same call then opened the shared P2P traffic bearer and routed voice, but CMCE only set `floor_holder`; it did not emit an internal `CallControl::FloorGranted` to UMAC for the initial setup grant.
- This left CMCE and UMAC relying on `Open.active_addr` instead of the same floor event model used by group and Brew private paths.

Problem targeted:

- The local `U-SETUP` raw request-to-transmit/send-data bit was being interpreted from the Rust field name rather than ETSI table 14.74.
- EN 300 392-2 table 14.74 defines raw value `0` as "request to transmit/send data" and raw value `1` as "request that other MS may transmit/send data".
- For on/off-hook private simplex, clause 14.5.1.2.1 uses that field to decide the setup-phase transmit permission. A raw `1` in the lab trace means the called MS may transmit first, not the caller.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.1.1: `D-CONNECT ACKNOWLEDGE` tells the called MS which party is permitted to transmit and triggers lower-layer configuration at through-connection.
- EN 300 392-2 clause 14.5.1.1.2: `D-CONNECT` tells the calling MS which party is permitted to transmit and triggers lower-layer configuration at through-connection.
- EN 300 392-2 clause 14.5.1.2.1 and table 14.74: the SwMI controls private simplex transmit permission; on/off-hook setup interprets raw request-to-transmit/send-data values as setup permission direction.
- EN 300 392-2 clause 14.5.1.2.1 also says the SwMI shall not send unsolicited `D-TX GRANTED`; this patch does not send an over-air `D-TX GRANTED` at setup. It emits only internal CMCE-to-UMAC floor synchronization after `U-CONNECT`.
- This is clause-scoped engineering hardening and test evidence, not formal TETRA certification.

Patch implemented:

- `crates/tetra-pdus/src/cmce/pdus/u_setup.rs`
  - Corrected the `USetup::request_to_transmit_send_data` field comment to document the raw table 14.74 values.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/call.rs`
  - Corrected `IndividualCall::request_to_transmit_send_data` documentation to avoid treating the bool as a semantic "caller requested" flag.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Added `private_simplex_called_ms_transmits_first()` to keep setup and connect semantics in one place.
  - Local `U-CONNECT` now uses that helper for initial floor-holder selection.
  - After setting initial simplex `floor_holder`, CMCE now emits internal `CallControl::FloorGranted` to UMAC so hangtime/media queues/current speaker are synchronized with the grant carried by `D-CONNECT` / `D-CONNECT-ACKNOWLEDGE`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Local P2P `D-SETUP` now carries a setup-phase grant only for on/off-hook simplex:
    - raw bit `0`: called MS sees `GrantedToOtherUser`;
    - raw bit `1`: called MS sees `Granted`;
    - direct setup and duplex stay `NotGranted` until `D-CONNECT` / `D-CONNECT-ACKNOWLEDGE`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added assertions that local P2P `U-CONNECT` emits the initial UMAC `FloorGranted`.
  - Corrected on/off-hook raw bit `0` and raw bit `1` tests against table 14.74.
  - Added `test_simplex_p2p_current_floor_holder_u_tx_demand_is_granted_not_denied`.

Verification:

- `cargo fmt --package tetra-entities --package tetra-pdus` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 64 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 8 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 128 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Remaining risk / next non-repeating execution:

1. Commit and deploy directly to `/home/chris/nexus-bs-v0.1.55-test` with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
2. Retest private simplex `2260616 -> 2260618` with the same sequence that produced the bad first floor.
3. Watch live logs for `U-SETUP request_to_transmit_send_data=true`, `D-SETUP transmission_grant=Granted`, `D-CONNECT transmission_grant=GrantedToOtherUser`, `D-CONNECT-ACKNOWLEDGE transmission_grant=Granted`, and `UMAC floor granted source_issi=2260618`.
4. If static persists after the floor fix, do not change CMCE again first; inspect the remaining LMAC/UMAC raw `NormalTrainSeq2 Block2` path. That path preserves ETSI TCH/S half-slot timing but the current SAP cannot carry a bad-half-slot condition, so a future patch should add an explicit quality/condition field instead of treating unknown raw half-slots as clean speech.

## 2026-06-05 10:11:00 EEST - MM restart group report completion keeps GSSI after BS restart

Field symptom:

- After a BS restart, terminals showed attached to the network but displayed `No Group`.
- The post-restart log showed terminals sending `U-LOCATION UPDATE DEMAND` with GSSI `226333`, then a follow-up `U-ATTACH/DETACH GROUP IDENTITY` carrying the same GSSI plus `group_report_response = complete`.
- The BS accepted the first group list but cleared the local solicited group-report window too early, then rejected the follow-up complete PDU as a mixed MS-initiated attach/detach request.

Component explanation:

- MM is Mobile Management. It owns terminal registration, energy saving mode negotiation, and group affiliation state.
- Group report recovery is the restart path where the BS asks a still-camped terminal to restate its active groups so CMCE and the dashboard know which GSSI listeners exist.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: after `D-LOCATION UPDATE COMMAND` with group report request, the MS may report group identities in `U-LOCATION UPDATE DEMAND` or by `U-ATTACH/DETACH GROUP IDENTITY`.
- Clause 16.4.4 further says if all reported groups fit in one PDU, the PDU contains `group report complete`; otherwise a final follow-up PDU may carry completion.
- Clause 16.8.3 defines the SwMI-initiated group report response using `U-ATTACH/DETACH GROUP IDENTITY`.
- Clause 16.10.27a defines `group_report_response` value `0` as group report complete.
- This is clause-scoped hardening and test evidence, not a formal certification claim.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - `U-LOCATION UPDATE DEMAND` with group identities but without `group_report_response = complete` no longer clears the solicited group-report window.
  - `U-ATTACH/DETACH GROUP IDENTITY` with group identities plus `group_report_response = complete` is accepted only while that solicited group-report window is pending.
  - The existing reject path remains for the same mixed PDU outside the SwMI-requested report window.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added restart-recovery coverage for the real split sequence: `D-LOCATION UPDATE COMMAND`, then `U-LOCATION UPDATE DEMAND` with GSSI but no complete, then `U-ATTACH/DETACH GROUP IDENTITY` with GSSI and complete.
  - Updated the EG3 restart recovery test so group identities without complete keep the report window open.
  - Kept the negative non-solicited mixed-PDU rejection test.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs group_report --locked` -> 24 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 113 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit and deploy directly to `/home/chris/nexus-bs-v0.1.55-test` with EG7 config preserved.
2. Restart the test BS and verify post-restart logs no longer contain `Rejecting mixed U-ATTACH/DETACH GROUP IDENTITY` for `2260082`, `2260616`, or `2260618`.
3. Confirm terminals show GSSI `226333` rather than `No Group` after restart and before any PTT test.

## 2026-06-05 10:11:04 EEST - Post-deploy restart log confirms group affiliation recovery

Field validation:

- Remote test BS is running `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`.
- Remote build banner: `Build: v0.1.55-79272974`.
- Remote config keeps the hard EG test case enabled: `energy_saving_mode = "eg7"` and `call_preemptive = false`.
- Local inspected log copy: `/private/tmp/nexus-bs-current.log`.

Component explanation:

- MM restart recovery asks already-camped terminals to re-state their registration and group identities after BS restart.
- CMCE consumes MM `Register` and `Affiliate` updates so group call control and dashboard state know which ISSIs are valid listeners for a GSSI.

Observed result after the latest restart:

- `2260616` sent `U-LOCATION UPDATE DEMAND` with GSSI `226333`; BS sent `D-LOCATION UPDATE ACCEPT` with `GroupIdentityLocationAccept`; CMCE registered and affiliated `2260616` to `[226333]`.
- `2260082` answered the BS recovery command with `DemandLocationUpdating` and GSSI `226333`; BS accepted and affiliated it to `[226333]`.
- `2260618` re-registered with GSSI `226333`; BS accepted and affiliated it to `[226333]`.
- The prior blocker sequence was present for `2260082`: follow-up `U-ATTACH/DETACH GROUP IDENTITY` with `group_report_response len=1 data=0` and GSSI `226333`.
- The new code accepted that final group-report-complete PDU with `solicited=true`, sent `D-ATTACH/DETACH GROUP IDENTITY ACK` with `group_identity_accept_reject=0`, and left CMCE affiliated to `[226333]`.

Negative checks:

- No `Rejecting mixed U-ATTACH/DETACH GROUP IDENTITY` appeared in the current post-restart log.
- No `PTT denied`, `RequestedServiceNotAvailable`, `Service unavailable`, `Unit Not Attached`, `No answer`, or `ERROR` appeared in the current post-restart log slice.

Remaining risk / next non-repeating execution:

1. User should confirm the terminal UI now shows group `226333`, not `No Group`, immediately after BS restart.
2. If any terminal still displays `No Group`, capture a fresh full log from the new restart before patching; check whether dashboard display state diverges from MM/CMCE affiliate state.
3. If the group display is fixed, continue field validation with EG7 active: group PTT turn-taking, private simplex/duplex, SDS/WAP smoke, and longer soak.

## 2026-06-05 10:45:39 EEST - Dashboard and MM restart cache hardened for `No Group` after restart

Field symptom:

- User reported that after BS restart the stations appeared attached but with `No Group`.
- Fresh remote log `/home/chris/nexus-bs-v0.1.55-test/nexus-bs.log`, copied to `/private/tmp/nexus-bs-current.log`, started at `10:07:57` with build `v0.1.55-79272974` and `energy_saving_mode = "eg7"`.
- The active remote restart cache `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` was still old format: only `2260082`, `2260616`, `2260618`, no cached GSSI.

Log finding:

- MM/CMCE did not lose the group in this restart slice.
- `2260616`, `2260082`, and `2260618` each sent location update with GSSI `226333`; MM sent `D-LOCATION UPDATE ACCEPT` with `GroupIdentityLocationAccept` for `226333`; CMCE logged `subscriber affiliate ... groups=[226333]`.
- Therefore the observed `No Group` was either dashboard/browser event-order display loss, or a restart-cache risk for cases where an MS answers the restart recovery command without a fresh group report.

Component explanation:

- MM is Mobile Management: it owns terminal registration, group affiliation state, and the local restart-recovery cache used after BS restart.
- CMCE is call control: it consumes MM register/affiliate updates so group/private calls know which terminals are valid participants.
- Dashboard telemetry is observability only: it must accurately show MM/CMCE state, but it is not the ETSI air-interface procedure.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: BS-commanded registration can request a group identity report after restart.
- Clauses 16.8.0, 16.8.2, 16.8.3, 16.8.4 and 16.10.27a: group identities reported/accepted during MM attach/group-report procedures remain the authority for affiliation state.
- Clauses 16.10.19 and 16.10.20: accepted group attachment information and reject reasons must be coherent; this patch preserves accepted `GroupAttachmentInfo` in the restart cache and does not fabricate an over-air GSSI accept when the MS did not report one.
- This is clause-scoped hardening and test evidence, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Restart recovery cache now supports `ISSI GSSI:lifetime:class_of_usage`.
  - Successful group affiliation persists the current GSSI/class/lifetime in the local cache.
  - Legacy `ISSI`-only cache entries remain valid.
  - If a restarted BS has a cached accepted GSSI and a solicited `DemandLocationUpdating` response arrives without a fresh group report, MM restores only local routing/CMCE affiliation from the cache; it does not add a fake `GroupIdentityLocationAccept` to that over-air response.
  - Explicit empty complete reports clear cached groups; explicit group reports replace cached groups.
- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Dashboard state now creates/preserves an MS entry when `MsGroupAttach` or `MsGroupsSnapshot` arrives before `MsRegistration`, instead of later showing `No Group`.
- `crates/tetra-entities/src/net_dashboard/html.rs`
  - Browser-side WS handling now uses `ensureMsEntry()` for `ms_groups` and non-empty `ms_groups_all`, so a live browser does not drop group events that race ahead of `ms_registered`.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added restart-cache GSSI persistence/restoration/empty-clear/replacement coverage.
- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Added server-state and shipped-HTML regression tests for group-before-registration ordering.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 14 passed.
- `cargo test -p tetra-entities --lib dashboard_ --locked` -> 8 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Deploy this local patch directly to `/home/chris/nexus-bs-v0.1.55-test` without compiling on the Pi.
2. Restart the test BS with EG7 still active and confirm the cache rewrites to `2260616 226333:0:4`, `2260082 226333:0:4`, `2260618 226333:0:4` or equivalent class values after the terminals report.
3. Re-open the dashboard after restart and verify all three stations show `226333`, not `No Group`.
4. If a terminal itself, not the dashboard, still displays `No Group`, inspect whether it received/ACKed `D-LOCATION UPDATE ACCEPT` and whether it later sends an explicit empty group-report-complete.

## 2026-06-05 10:54:12 EEST - Restart candidate self-attach without groups no longer clears cached GSSI

Additional audit finding:

- A read-only MM audit found a remaining race: a terminal can self-attach before BS sends the startup `D-LOCATION UPDATE COMMAND`.
- If that early self-attach is `U-LOCATION UPDATE DEMAND` / `ITSI attach` with no group identities, old logic did not treat it as a solicited restart recovery response and could call `remember_restart_recovery_issi()` with an empty local group set.
- With EG7, that missed group-report command can persist longer because the terminal sleeps more aggressively after the initial attach.

Component explanation:

- MM restart recovery has two valid field orders:
  - BS first: BS sends `D-LOCATION UPDATE COMMAND(group identity report=1)`, then MS answers.
  - MS first: still-camped MS sends its own attach/update before the BS command is due.
- The second order must not erase the restart cache just because the MS did not include a fresh GSSI in that first PDU.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4 permits SwMI-commanded registration and group identity report.
- Clauses 16.8.0, 16.8.2, 16.8.3, 16.8.4 and 16.10.27a keep explicit group reports/complete reports authoritative.
- Clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7 and 23.7.6/T.210 constrain the EG7 interaction: group-report recovery must be scheduled before a BS-initiated EG request can make the MS harder to reach.
- This remains clause-scoped hardening and test evidence, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Captures `was_restart_recovery_candidate` before registration removes the candidate from the recovery map.
  - For a new, group-less restart candidate, restores cached GSSI locally even if no `D-LOCATION UPDATE COMMAND` was already pending and even if the LU type is `ITSI attach`.
  - Still does not fabricate `GroupIdentityLocationAccept`; the explicit MS group report or empty complete report remains authoritative.
  - Queues `D-LOCATION UPDATE COMMAND(group_identity_report=1)` for a restart candidate that self-attaches without groups.
  - Queues that group-report command before a configured BS-initiated `D-MM STATUS` energy-saving request, so EG7 does not hide the terminal before group recovery is requested.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added coverage for unsolicited group-less `ITSI attach` restoring cached GSSI and preserving cache.
  - Added EG7 coverage proving group-report command order precedes `D-MM STATUS`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 16 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this second MM hardening patch.
2. Redeploy to test BS and confirm build id changes from `7d72c06b`.
3. Verify cache stays `ISSI 226333:0:4` for `2260082`, `2260616`, and `2260618` after restart.

## 2026-06-05 10:58:30 EEST - Final deploy blocked by SSH timeout

Execution status:

- Code commit `f02371a` (`fix: recover restart candidate groups before eg`) was created after local verification.
- Local `scripts/nexus-bs-test-deploy.sh` cross-build completed successfully for `nexus-bs v0.1.55`.
- The remote deploy phase failed before copying the new binary because `ssh chris@192.168.1.179` timed out on port 22.
- Two short-timeout SSH retries also timed out.
- Last confirmed remote running build remains `v0.1.55-7d72c06b`; that build already includes the dashboard/cache GSSI persistence patch and had rewritten the cache to:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`

Next non-repeating execution:

1. When `chris@192.168.1.179` is reachable again, rerun `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
2. Confirm remote build id changes from `v0.1.55-7d72c06b` to the current HEAD.
3. Re-read `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` and verify all three ISSIs still persist `226333:0:4`.

## 2026-06-05 11:04:26 EEST - Restart `No Group` status check after user report

User report:

- After BS restart, terminals attach but appear with `No Group`.
- This is the MM/GMM restart-recovery path: MM owns terminal registration and GSSI affiliation state; CMCE consumes MM register/affiliate events for group calls; dashboard only displays that state.

Local findings:

- Repo was clean before inspection.
- Current HEAD is `70ad46f` with the prior MM restart recovery fixes already committed.
- Local code already contains the follow-up fix from `f02371a`:
  - restart candidates are captured before registration removes them from the recovery map;
  - a new group-less restart candidate restores cached GSSI locally when available;
  - an unsolicited group-less `ITSI attach` still gets `D-LOCATION UPDATE COMMAND(group_identity_report=1)`;
  - that group-report command is queued before the configured BS-initiated EG7 `D-MM STATUS` request;
  - no fake over-air `GroupIdentityLocationAccept` is generated when the MS did not report a group.
- Dashboard code already preserves `ms_groups` / group snapshot events if they arrive before `ms_registered`, preventing UI-only `No Group`.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI may command registration and request group identity report.
- Clauses 16.8.0, 16.8.2, 16.8.3, 16.8.4 and 16.10.27a: explicit group reports and complete empty reports remain authoritative.
- Clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7 and 23.7.6/T.210: EG7 scheduling must not hide the MS before group recovery is requested.
- This is engineering evidence for the touched clauses only, not formal TETRA certification.

Verification rerun:

- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 16 passed.
- `cargo test -p tetra-entities --lib dashboard_ --locked` -> 8 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deploy/log status:

- `ssh -o BatchMode=yes -o ConnectTimeout=5 -o ServerAliveInterval=2 -o ServerAliveCountMax=1 chris@192.168.1.179 date` timed out twice.
- Could not read live logs or deploy current HEAD because port 22 is unreachable.
- Do not patch further from the field symptom alone while local clause-scoped tests already cover it; next execution is deploy current HEAD and inspect the fresh full log from the next restart.

Next non-repeating execution:

1. When SSH returns, deploy directly with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
2. Confirm remote build id is current HEAD, not `v0.1.55-7d72c06b`.
3. Read `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers`; expected steady state after terminal reports is `2260082`, `2260616`, and `2260618` with `226333:0:4` or equivalent class values.
4. Read the fresh full log from the latest restart and compare MM `subscriber affiliate` state with dashboard display if any terminal still shows `No Group`.

## 2026-06-05 12:23:29 EEST - MM restart `No Group` ACK hardening and CMCE proof

User report:

- After BS restart, terminals attach but appear with `No Group`.
- Field concern is especially relevant with `2260616`, `2260082`, `2260618`, group `226333`, and EG7 energy saving.

Components touched:

- MM restart recovery: restores cached GSSI affiliation after restart and runs the SwMI-initiated `D-ATTACH/DETACH GROUP IDENTITY` refresh.
- CMCE group call control: consumes MM `Register`/`Affiliate` updates; if these are missing, group call/PTT behaves like the terminal has no group.
- Restart recovery cache: local persistent memory of `ISSI -> GSSI:lifetime:class_of_usage` used after BS restart.

Patch summary:

- Restart-refresh group ACKs now accept the same ISSI even if the local MLE primitive handle is non-zero and does not match the downlink handle.
- Normal non-restart SwMI group transactions remain strict on matching MLE handle.
- Segmented cached scan-list recovery now preserves groups that were not yet sent over air if an earlier batch fails or T353 expires.
- Added MM tests for non-zero ACK, EG7 restart refresh, segmented success, and T353 segmented failure preservation.
- Added MM+CMCE integration for `226333`: three lab ISSIs recover cached group, ACK with non-matching handles, survive T353, start a group call, and queue return PTT without release/deny.

ETSI clause scope:

- EN 300 392-2 clause 16.8.1: SwMI-initiated attach/detach group identity procedure and ACK request.
- Clauses 16.10.14/16.10.17/16.10.19: ACK type and group identity attachment information.
- Clause 16.11.1.3: T353 expiry handling.
- Clause 14.5.2.2.1: group call floor request/queued transmission behavior after CMCE receives restored affiliation.
- The MLE handle is local stack plumbing, not an over-air ETSI ACK discriminator; the clause-scoped key is the same ISSI and active group identity procedure.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery_group_refresh_accepts_unrouted_nonzero_ack_without_t353_purge --locked` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery_group_less_demand_segments_cached_scan_list_refresh --locked` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery_segmented_group_refresh_t353_preserves_unsent_cached_groups --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs restart_recovery_cached_226333_group_restores_cmce_listeners_after_unrouted_ack --locked` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 22 passed.
- `cargo test -p tetra-entities --test test_mm_bs swmi_group_ack --locked` -> 12 passed.
- `cargo test -p tetra-entities --test test_cmce_bs 226333 --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group_ --locked` -> 51 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this patch.
2. Deploy directly to testing with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
3. Confirm remote build id changes from the old live `v0.1.55-7d72c06b` to the new commit.
4. After restart, read the fresh full log and confirm:
   - `MM: sending SwMI group attach refresh` for cached `226333` when a terminal answers without group IE;
   - `CMCE: subscriber affiliate issi=... groups=[226333]`;
   - no `T353 expired` rollback for accepted same-ISSI ACKs;
   - no `No Group` steady state for `2260082`, `2260616`, `2260618`.

## 2026-06-05 12:26:33 EEST - Deployed restart ACK hardening to test BS

Deployment:

- Commit deployed: `40398d91` (`fix: accept restart group ack handles`).
- Command used: `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Build happened locally and binary was copied directly to testing; no Rust compile on Pi and no binary backup step.
- Live header now shows `Build: v0.1.55-40398d91`.

Remote state after restart:

- Running processes:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`
  - `nexus-bs-control-service --listen 127.0.0.1:9002`
- Recovery cache:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`

Fresh log review from last restart:

- Full log copied to `/private/tmp/nexus-bs-after-40398d91.log` and read in chunks.
- `MM: restart recovery armed for 3 local ISSI(s): {2260082, 2260616, 2260618}` appears immediately after startup.
- `2260082` registers with `226333` and CMCE affiliates `groups=[226333]`.
- `2260618` registers with `226333`; one solicited attach/detach completion briefly deaffiliates then immediately reaffiliates `226333`, ending affiliated.
- `2260616` initially misses a few recovery command deliveries, then sends `DemandLocationUpdating` with EG7 and `226333`; CMCE registers and affiliates `groups=[226333]`.
- No `T353 expired`, `No Group`, `PTT denied`, `Unit Not Attached`, `RequestedServiceNotAvailable`, or `service unavailable` found in the fresh restart log.
- Remaining warnings are RF/startup or expected live-radio noise:
  - startup TX late / lost samples;
  - `SX1255 temperature read failed` while streams are active;
  - initial LLC retransmission exhaustion before the radios answer;
  - occasional malformed/short MAC access bursts.

Status:

- The deployed build fixes the MM restart ACK-handle purge path in code and proves, from this restart, that the three lab terminals are affiliated to `226333` at CMCE level.
- This is clause-scoped engineering validation for the touched ETSI procedures, not formal certification.

Next non-repeating execution:

1. User should visually confirm terminals no longer show `No Group` after this restart.
2. If any terminal still displays `No Group`, capture the fresh log interval after that visual state and compare dashboard state vs MM/CMCE affiliation lines.
3. Run a live group PTT test on `226333`; if failure occurs, inspect from the current build log around `U-SETUP`, `U-TX DEMAND`, `D-TX GRANTED`, and UMAC floor events.

## 2026-06-05 12:44:11 EEST - Hardened restart group retention against transient No Group

Component scope:

- MM: Mobility Management owns terminal registration and group affiliation after attach/restart.
- CMCE: call control consumes MM subscriber updates; a short false deaffiliate can make group PTT look unavailable.
- Dashboard telemetry: observability only; it must show the final MM group list, not a local intermediate state.

Patch:

- `client_detach_all_groups_silent()` now lets MM apply ETSI mode=1 as one logical replace operation without first emitting an intermediate empty group telemetry event.
- MM mode=1 handling now keeps retained GSSIs affiliated in shared subscriber/CMCE state when the replacement list contains the same accepted group.
- MM still deaffiliates groups that are actually absent from the replacement list, so explicit empty reports/detach-all remain authoritative.
- Cached restart group restoration now emits a full current group snapshot after replaying cached affiliations, so dashboard clients do not depend only on incremental attach ordering.
- Added tests for standalone and location-update mode=1 refreshes retaining `226333` without CMCE `Deaffiliate -> Affiliate` churn.
- Updated the restart-recovery follow-up test to assert no transient CMCE `No Group` window when the final complete report retains the same GSSI.

ETSI clause scope:

- EN 300 392-2 clause 16.10.17/table 16.49: group identity attach/detach mode=1 means detach all current groups and attach the listed identities as one requested operation.
- Clauses 16.8.0/16.8.4: accepted group identities remain valid attached identities and are represented in the downlink acknowledgement/accept.
- This is a clause-scoped engineering hardening for restart/group-affiliation behavior, not formal certification.

Verification:

- `cargo test -p tetra-entities --test test_mm_bs mode_one_retains_same_group --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 22 passed.
- `cargo test -p tetra-entities --test test_mm_bs swmi_group_ack --locked` -> 12 passed.
- `cargo test -p tetra-entities --test test_mm_bs group_identity --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 127 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --lib dashboard --locked` -> 49 passed.
- `cargo test -p tetra-entities --test test_cmce_bs 226333 --locked` -> 2 passed.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this MM restart/no-transient-group patch.
2. Deploy directly to testing with local build only.
3. After restart, read the full fresh log from process start and confirm:
   - `2260082`, `2260616`, `2260618` end with `groups=[226333]`;
   - no `CMCE: subscriber deaffiliate issi=... groups=[226333]` followed immediately by re-affiliate for retained `226333`;
   - no steady dashboard/WebSocket snapshot with `groups:[]` for the three lab ISSIs;
   - no `PTT denied`, `No Group`, `Unit Not Attached`, or `T353 expired` in the restart interval.

## 2026-06-05 12:54:07 EEST - Split soft re-attach private-call cleanup from group affiliation

Live trigger:

- Deployed `4317457e` and read the full fresh log from Pi start.
- The three lab terminals ended correctly affiliated to `226333` in the WebSocket snapshot.
- Remaining risk found in the fresh log: a soft `RoamingLocationUpdating` from `2260082` emitted an internal CMCE `Deregister -> Register -> Affiliate` sequence in the same millisecond. That can create a tiny dashboard/CMCE `No Group` window even though the final group state is correct.

Component scope:

- MM: registration and group affiliation owner. On soft re-attach, it may need to clear stale private-call state but must not withdraw accepted groups.
- CMCE: call/PTT controller. It now has a separate internal action to release stale individual calls without touching `subscriber_groups` or GSSI listener counts.
- Brew/backhaul: ignores this internal cleanup action; it is not a subscriber deregistration or group-affiliation procedure.

Patch:

- Added internal `ReleaseIndividualCalls` subscriber action.
- MM soft re-attach now emits `ReleaseIndividualCalls` to CMCE instead of simulating `Deregister -> Register -> Affiliate`.
- CMCE handles `ReleaseIndividualCalls` by releasing active individual calls involving the ISSI, while preserving group memberships and listener counts.
- Brew ignores the internal action defensively.
- Updated the MM soft re-attach test so the expected behavior is one private-call cleanup action and no group churn.
- Added a CMCE test proving a group-affiliated MS still receives a queued return PTT grant after private-call cleanup.

ETSI clause scope:

- EN 300 392-2 clauses 16.9.3.4 and 16.10.35a: the soft location update is accepted as the same location-registration update type.
- Clauses 16.8.0/16.8.4: accepted group identities remain valid attached group identities until an explicit detach/replacement procedure changes them.
- The private-call cleanup is a local robustness guard for stale CMCE individual-call state; it is not an over-air ETSI group detach operation and must not be represented as one.
- This is clause-scoped engineering validation only, not formal certification.

Verification:

- `cargo fmt -p tetra-saps -p tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs soft_roaming_reattach --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs private_call_cleanup_preserves_group_floor_membership --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 127 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 131 passed.
- `cargo check -p tetra-saps -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this soft re-attach/no-group-churn patch.
2. Deploy directly to testing with local build only.
3. After restart, read the fresh full Pi log and confirm:
   - build id matches the new commit;
   - `2260082`, `2260616`, `2260618` end with `groups=[226333]`;
   - soft re-attach logs `ReleaseIndividualCalls`/private cleanup and no CMCE `Deregister -> Register -> Affiliate` group churn for retained `226333`;
   - no `No Group`, `Unit Not Attached`, `PTT denied`, `RequestedServiceNotAvailable`, or `T353 expired` during restart recovery.

## 2026-06-05 12:56:41 EEST - Deployed soft re-attach group preservation to test BS

Deployment:

- Commit deployed: `113f2a91` (`fix: preserve groups during soft reattach cleanup`).
- Command used: `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Build happened locally and binary was copied directly to testing; no Rust compile on Pi and no binary backup step.
- Live header now shows `Build: v0.1.55-113f2a91`.

Remote restart evidence:

- Fresh full log copied to `/private/tmp/nexus-bs-after-113f2a91.log` and read from process start.
- Recovery cache remains coherent:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`
- Dashboard WebSocket snapshot after restart:
  - `2260082` has `groups:[226333]`;
  - `2260616` has `groups:[226333]`;
  - `2260618` has `groups:[226333]`.
- `2260082` soft re-attach now logs:
  - `MM: requested CMCE individual-call cleanup for ISSI 2260082 while preserving group affiliation (soft re-attach)`;
  - `CMCE: individual-call cleanup issi=2260082 preserved groups=[226333]`.
- The old local `CMCE Deregister -> Register -> Affiliate` churn for `2260082` no longer appears in this restart interval.

Negative log review:

- No `PTT denied`.
- No `No Group`.
- No `Unit Not Attached`.
- No `RequestedServiceNotAvailable`.
- No `Service unavailable`.
- No `T353 expired`.
- No `CMCE: subscriber deaffiliate` or `CMCE: subscriber deregister` for `2260082`, `2260616`, or `2260618` with retained `226333`.

Observed non-blocking warnings:

- Startup TX late / lost samples.
- `SX1255 temperature read failed` while streams are active.
- LLC retransmission exhaustion while radios are not yet answering during restart probing.
- One unexpected ACK from `2260616` after successful registration.
- Short malformed MAC access bursts from live RF.

Status:

- The restart `No Group` transient caused by soft re-attach private-call cleanup has been removed in this live deployment.
- Current validation is engineering evidence scoped to the touched EN 300 392-2 procedures and local CMCE state handling; it is not formal TETRA certification.

Next non-repeating execution:

1. User should test group PTT on `226333` immediately after this restart and report any `PTT denied` or radio-side `No Group`.
2. If a new failure appears, inspect the live interval around `U-SETUP`, `U-TX DEMAND`, `D-TX GRANTED`, `ReleaseIndividualCalls`, and UMAC floor events.
3. Continue broader hardening on remaining basic stack surfaces: long-run registration/affiliation retention, group floor handoff under EG7, private simplex/duplex, SDS, and WAP.

## 2026-06-05 13:54:03 EEST - Live restart No Group report audit on build 7cf2e4a2

User report:

- After BS restart, radios appeared attached but with `No Group`.

Live evidence read from the current test Pi process:

- Running build: `Nexus-BS v0.1.55`, build `v0.1.55-7cf2e4a2`.
- Runtime restart cache `/home/chris/nexus-bs-v0.1.55-test/config.live.toml.subscribers` still contains:
  - `2260082 226333:0:4`
  - `2260616 226333:0:4`
  - `2260618 226333:0:4`
- Fresh log from the current restart shows `MM: restart recovery armed for 3 local ISSI(s): {2260082, 2260616, 2260618}`.
- `2260082` sent `U-LOCATION UPDATE DEMAND` with `GroupIdentityLocationDemand` for `226333`; BS replied with `D-LOCATION UPDATE ACCEPT` carrying `GroupIdentityLocationAccept` for `226333`, then CMCE registered and affiliated `groups=[226333]`.
- `2260618` followed the same location update and group accept path for `226333`.
- `2260616`, configured/requested for EG7, missed early BS-initiated restart probes but later sent `DemandLocationUpdating` with `energy_saving_mode=Eg7` and `GroupIdentityLocationDemand` for `226333`; BS replied with `D-LOCATION UPDATE ACCEPT` carrying EG7 information and `GroupIdentityLocationAccept` for `226333`.
- Dashboard WebSocket snapshot after the restart reports:
  - `2260082 groups=[226333] energy_saving_mode=0`
  - `2260618 groups=[226333] energy_saving_mode=0`
  - `2260616 groups=[226333] energy_saving_mode=7 frame=12 multiframe=19`

Negative log review for the current restart interval:

- No `No Group`.
- No `Unit Not Attached`.
- No `PTT denied`.
- No `RequestedServiceNotAvailable`.
- No `Service unavailable`.
- No `T353 expired`.

Observed but non-blocking:

- EG7 station `2260616` may not hear early restart recovery `D-LOCATION-UPDATE-COMMAND` transmissions until its listen window or next uplink activity; it later completed the standardized location update with the group included and acknowledged.
- `2260082` and `2260618` timed out BS-initiated EG7 assignment and stayed in `StayAlive`; this is expected when those radios do not accept the optional BS-initiated energy saving change.

Technical conclusion:

- The current BS state, CMCE listener state, restart cache, and dashboard snapshot all have `226333` restored. The specific stale dashboard `No Group` path is covered by commit `7cf2e4a2`.
- If a radio screen still shows `No Group` while the BS snapshot has `groups=[226333]`, the next investigation is terminal-side retained display/scan-list state or a short visible interval before the radio receives/ACKs the group-bearing `D-LOCATION UPDATE ACCEPT`, not a lost BS restart cache.

ETSI clause scope:

- EN 300 392-2 clauses 16.9.2.8, 16.9.3.4, and 16.10.35a: location update accept type and accepted location update response path.
- EN 300 392-2 clauses 16.8.0, 16.8.4, and 16.10.17: accepted group identities and attach/detach group identity semantics.
- EN 300 392-2 clauses 16.7.1, 16.10.9, 16.10.10, 16.10.35a, 23.5.2.2.7, and 23.7.6: EG7 negotiation/assignment and scheduling awareness.
- Dashboard telemetry remains observability evidence only; it is not formal ETSI conformance evidence.

Next non-repeating execution:

1. Have the user check the actual radio display after the completed location update window, especially `2260616` in EG7.
2. If a radio still shows `No Group`, capture the exact ISSI and wall-clock time, then inspect the log around that station's `U-LOCATION UPDATE DEMAND`, `D-LOCATION UPDATE ACCEPT`, LLC ACK, and any subsequent group report/attach-detach PDU.
3. Continue live group PTT validation on `226333`; if PTT fails, inspect the interval around `U-SETUP`, `U-TX DEMAND`, `D-TX GRANTED`, floor ownership, and UMAC voice grant timing.

## 2026-06-05 14:30:21 EEST - Private simplex called-ISSI End Call release

User symptom:

- Private simplex end-of-call still had two bad peer-side outcomes: the called ISSI could show `Not Answered`, or Motorola MXP600 `2260618` could soft-reset after the remote caller pressed red.
- The high-risk live shape is `2260616 -> 2260618`: caller clears with `U-DISCONNECT`, while `2260618` is the called ISSI and may have recently held or released the simplex floor.

Component, simple technical meaning:

- CMCE/CC-BS is the private-call control state machine inside the BS. It decides which call-control PDU is sent when a terminal opens, speaks in, or ends a private call.
- `U-DISCONNECT` is the uplink terminal request to end the call.
- `D-RELEASE` is the downlink release indication that does not require a terminal response; this is the clean peer-side "end call" path.
- `D-DISCONNECT` is a downlink disconnect request that requires the peer to answer `U-RELEASE`; keeping this path for sensitive caller-hangup-to-called-peer cases was the likely source of the bad UI/reset behavior.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.3.1: either calling or called user may initiate individual-call disconnection; the MS that sent `U-DISCONNECT` waits for `D-RELEASE`; the SwMI should inform the other MS of call clearance either by `D-DISCONNECT` or by `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3: an MS receiving `D-DISCONNECT` shall respond with `U-RELEASE`; an MS receiving `D-RELEASE` sends no response.
- This patch uses the `D-RELEASE` alternative explicitly allowed by clause 14.5.1.3.1 for the called ISSI in local private simplex caller-hangup cases. It is clause-scoped hardening, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Private simplex caller-hangup now detects `sender == calling_addr` and `peer == called_addr`.
  - In that case the called peer is cleared after the existing tail drain with `D-RELEASE`, not `D-DISCONNECT`.
  - The disconnecting caller still gets prompt `D-RELEASE(UserRequestedDisconnection)`.
  - The called peer gets `D-RELEASE(SwmiRequestedDisconnection)`, so the peer sees SwMI call release / end-call semantics instead of a user-request/no-answer-style handshake.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/mod.rs`
  - Pending private disconnect tail-drain state now stores a separate peer cause.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Tail-drain completion now uses that separate peer cause when sending peer `D-RELEASE`.
  - A private disconnect now also consumes any pending simplex `U-TX CEASED` tail-drain for the same call. This preserves the bearer-tail wait but suppresses stale `D-TX CEASED` / floor-release signalling after call release has started.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated direct simple private call workflow and unsolicited `U-RELEASE` regression to assert no peer `D-DISCONNECT` for caller-hangup-to-called-ISSI.
  - Added/kept D-DISCONNECT coverage by making the called party disconnect after it has held the floor, so the caller peer path still exercises `D-DISCONNECT -> U-RELEASE` where applicable.
  - Updated MXP600 regressions to require peer `D-RELEASE(SwmiRequestedDisconnection)`.
  - Added overlap regression for `2260616 -> 2260618`: MXP600 peer sends `U-TX CEASED`, caller presses red before tail-drain expiry, and BS sends only peer `D-RELEASE` with no delayed `D-TX CEASED`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_caller_disconnect_cancels_pending_peer_tx_ceased_tail --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 65 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 132 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit and deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with the normal local-build deploy script.
2. Retest exact field case: `2260616 -> 2260618`, let `2260618` speak last if desired, then press red on `2260616`.
3. Expected live log: prompt `D-RELEASE(UserRequestedDisconnection)` to `2260616`, no peer `D-DISCONNECT` to `2260618`, tail-drained `D-RELEASE(SwmiRequestedDisconnection)` to `2260618`, no peer `U-RELEASE` required, no MXP600 soft reboot, no `Not Answered`.

## 2026-06-05 15:12:44 EEST - Group first-speaker floor retake without immediate back-up D-SETUP

User symptom:

- In GSSI group call, the station that opens/retakes PTT could produce static/no voice, while later interventions by other stations could carry voice.
- The live log slice for `call_id=11`, GSSI `226333`, showed a floor grant to ISSI `2260082`, then a burst of FACCH/STCH signalling with `speech_present=false`, followed by UL inactivity timeout. Earlier and later periods showed normal `UMAC voice route`, so the failure path was tied to the first frames after group floor retake.

Component, simple technical meaning:

- CMCE/CC-BS group floor control decides who is allowed to speak in a group call and sends `D-TX GRANTED` / `D-TX CEASED`.
- UMAC media routing carries actual TCH/S speech bits on the assigned traffic slot after CMCE has granted floor.
- Back-up `D-SETUP` is the group-call late-entry mechanism: radios that missed the original setup can still join an ongoing group call. It is not the immediate floor-grant mechanism.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: SwMI controls group transmit permission; the speaking MS gets individually addressed `D-TX GRANTED`, while the group gets `D-TX GRANTED` with "granted to another user".
- EN 300 392-2 clause 14.5.2.1 and Annex D: `D-SETUP` establishes/backs up group-call entry; Annex D describes optional back-up `D-SETUP` for called group members.
- EN 300 392-2 clause 23.8.2.3.1 requires both CC transmit authorization and an uplink-applicable traffic usage marker before an MS transmits traffic.
- This is clause-scoped hardening of the group-call floor path, not formal TETRA certification.

Patch implemented:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - Group floor grant paths now update the cached late-entry `D-SETUP` speaker but do not immediately enqueue a fresh group `D-SETUP` in the same burst as `D-TX GRANTED`.
  - Immediate floor retake therefore sends the individual grant to the new speaker and the group grant to listeners without a same-burst setup refresh that can disturb the first speech frames.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - UL-inactivity handoff to a queued group requester follows the same rule: grant floor immediately, refresh cached late-entry setup, defer the actual back-up `D-SETUP` to the normal late-entry scheduler.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Group `D-INFO` T310 reset remains FACCH/STCH but now carries DL-only channel allocation. It is timer/listener signalling, not transmit authorization for all GSSI members.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated group handoff, hangtime retake, repeated same-GSSI setup, and UL-inactivity handoff regressions to assert no immediate back-up `D-SETUP` during the first floor-grant burst.
  - Added/kept checks that the deferred late-entry `D-SETUP` still advertises the new speaker when the retry window runs.
  - Updated group `D-INFO` reset assertions to require DL-only allocation.

P2P/private-call safety:

- No P2P/private-call code path was changed.
- P2P regression suite still passes, including simplex floor handoff, caller/called release, pending disconnect, and MXP600-safe called-peer release tests.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_hangtime_tx_demand_defers_late_entry_d_setup_refresh --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_tx_ceased_hands_floor_to_queued_requester --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_ul_inactivity_hands_floor_to_queued_requester --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs group --locked` -> 52 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 65 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 51 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 132 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this patch, then deploy direct to the test Pi with local build only.
2. Retest GSSI `226333`: initial group setup speech, hangtime retake by the original speaker, and handoff/return PTT by the other station.
3. Expected live log around retake: `D-TX GRANTED` individual + group, `D-INFO` reset T310 DL-only, no immediate `DSetup` in the same floor-grant burst, then early `UMAC voice route` instead of `UL inactivity timeout`.

## 2026-06-05 15:25:08 EEST - P2P/private-call scope guard and group initial-speaker seed

User request:

- Patch must take P2P calls into account while continuing group-call hardening.

Component, simple technical meaning:

- `Circuit.active_addr` is the primary address for an assigned traffic bearer.
- For a private/P2P call this primary address is an ISSI, so only the two private participants may drive floor/audio state.
- For a group call this primary address is a GSSI. The current speaker ISSI can be stored as secondary metadata so EG/listening state is correct, but that must not turn the group bearer into a private-call participant list.
- UMAC scheduler is the component that enforces this distinction before accepting a `FloorGranted` update.
- UMAC also owns STCH `MAC-U-SIGNAL` attribution. STCH signalling has no SSI field, so UMAC must already know the current ISSI speaker when it forwards early traffic-channel signalling.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1: individual/private calls are participant-scoped ISSI-to-ISSI services.
- EN 300 392-2 clause 14.5.2: group calls are P2MP/GSSI-scoped services where different affiliated ISSIs may take transmit permission over the same group bearer.
- EN 300 392-2 clause 23.7.6 remains relevant because assigned-channel activity must keep the correct EG terminals awake.
- This is clause-scoped engineering hardening, not formal TETRA certification.

Patch implemented:

- `crates/tetra-saps/src/control/call_control.rs`
  - Documented that `active_addr` is the primary bearer scope.
  - Documented that secondary ISSIs are EG/listening metadata and do not by themselves make a group bearer private/P2P-scoped.
  - Documented `Circuit::is_primary_issi_scoped()` as the private/P2P discriminator.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Documented `ul_circuit_is_private_participant_scoped()` as primary-address based, not "any secondary ISSI" based.
  - Added a scheduler regression proving GSSI-primary + speaker-ISSI-secondary remains group-scoped.
  - Added a scheduler regression proving ISSI-primary + peer-ISSI-secondary remains strict private/P2P-scoped and excludes a third ISSI.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - On circuit open, UMAC now seeds the current UL speaker from ISSI-primary circuits for P2P exactly as before.
  - For GSSI-primary group circuits, UMAC seeds the current UL speaker from the first secondary ISSI if present. This covers early STCH before a later `FloorGranted`, without turning the bearer into a private/P2P participant list.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added a regression that a GSSI-primary group `Open` with speaker ISSI secondary forwards immediate STCH `MAC-U-SIGNAL` as that ISSI.
  - Updated the group handoff audio-path test to use the real current group circuit shape: GSSI primary plus first-speaker ISSI secondary.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Tightened group setup helper assertions to require exactly one secondary speaker ISSI.
  - Tightened hook private-simplex setup assertions to require exactly one secondary peer ISSI.

P2P/private-call safety:

- P2P audio and release runtime paths were not changed; the new speaker seed keeps the existing ISSI-primary P2P behavior and adds only the GSSI-primary group secondary fallback.
- The new tests protect the current behavior so future group-call fixes cannot weaken private-call participant filtering.

Verification:

- `cargo fmt --package tetra-saps --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib test_ul_private_scope --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_stch_mac_u_signal_uses_secondary_speaker_from_group_open_circuit --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_group_floor_handoff_reopens_ul_traffic_for_lmac_tch_s_decode --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_stch_mac_u_signal_uses_current_ul_speaker_from_private_open_circuit --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_group_floor_grant_accepts_new_speaker_when_initial_speaker_is_secondary --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_stch_mac_u_signal_ignores_floor_granted_for_non_participant_private_speaker --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_simplex_p2p_current_floor_holder_u_tx_demand_is_granted_not_denied --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_caller_disconnect_tail_drains_when_mxp600_peer_holds_floor --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_setup_sends_proceeding_connect_and_group_setup_with_allocations --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_hook_setup_other_ms_request_sets_called_ms_initial_floor --locked` -> pass.
- `cargo check -p tetra-saps -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Operational note:

- Local `target` filled the disk during parallel Cargo tests. Ran `cargo clean` locally and reran verification sequentially with `CARGO_INCREMENTAL=0`.

Next non-repeating execution:

1. Commit this guard/seed patch.
2. Continue group-call live validation on GSSI `226333`; if static/no-voice persists, inspect the next live log for actual `UMAC voice route` vs FACCH/STCH-only frames rather than changing P2P release paths.
3. Keep P2P regressions in the verification set for every group-call patch.

## 2026-06-05 15:32:30 EEST - P2P/group speaker scope build deployed

Commit deployed:

- `8a53b919` (`fix: preserve p2p scope with group speaker metadata`)

Build/deploy:

- Command:
  - `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Local build only; no compile on `chris@192.168.1.179`.
- Remote binary copied directly to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; no binary backup created.
- Remote deployed binary SHA-256:
  - `b88bc154e727f1fe8c3f21b00e93c1e956773b213cd506ee11e9f231da3ca774`

Live evidence after deploy:

- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`
- Startup build line:
  - `Build: v0.1.55-8a53b919`
- Startup registration/affiliation:
  - `2260618` registered and affiliated to `226333`.
  - `2260616` registered and affiliated to `226333`.
  - `2260082` registered and affiliated to `226333`.
- The post-start filtered log sample showed no `RequestedServiceNotAvailable`, `Service unavailable`, `PTT denied`, or `Unit Not Attached`.

Next non-repeating execution:

1. RF retest GSSI `226333`: first PTT after group setup, then alternating PTT between stations.
2. RF retest private simplex P2P `2260616` <-> `2260618`, including reverse PTT and red-key close.
3. If group static/no-voice persists, inspect post-deploy log for `UMAC voice route`, `rx_blk_traffic`, early STCH/FACCH stealing, and `UL inactivity timeout` around the exact PTT window.

## 2026-06-05 15:58:03 EEST - GSSI floor notification hardening for 2260082/MTP3550

Observed live issue:

- User RF test on GSSI `226333` showed repeated PTT static/no-voice only from ISSI `2260082` (Motorola MTP3550).
- The terminal did not display `PTT denied`; UI looked normal for TX/RX.
- Log analysis confirmed this was not a CMCE denial:
  - `U-TX DEMAND` from `2260082` was accepted.
  - CMCE emitted individual `D-TX GRANTED` to `2260082`.
  - UMAC emitted `FloorGranted` for `source_issi=2260082`.
  - After grant, no `NormalTrainSeq*` / `UMAC voice route` appeared before `UL inactivity timeout`.

ETSI clause-scoped reasoning, not formal certification:

- EN 300 392-2 clause 14.5.2.2.1 b): group floor response must send individual `D-TX GRANTED` to the granted MS and group-addressed `D-TX GRANTED` to listeners indicating "granted to another user".
- The same clause notes the group-addressed grant should identify the transmitting party when needed to prevent the newly granted MS from switching back to U-plane receive.
- Clause 21.4.3.1: `random_access_flag` acknowledges successful random access. This must remain ISSI-scoped; do not acknowledge one ISSI's access on a GSSI resource for a large group.
- Clause 23.8 / assigned-channel FACCH/STCH: listener floor-control signalling should stay on the assigned traffic channel where group members are listening.

Patch summary:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Group-addressed `D-TX GRANTED/GrantedToOtherUser` now carries `transmitting_party_type_identifier=SSI` and `transmitting_party_address_ssi=<current speaker ISSI>`.
  - This keeps one scalable GSSI notification for all listeners while preventing the just-granted speaker from interpreting the GSSI PDU as "someone else got the floor".
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - If a speaker-qualified GSSI listener grant would exceed 124-bit STCH when serializing a redundant MAC channel allocation, UMAC keeps it on STCH and omits only that MAC channel-allocation element.
  - The primitive still carries channel allocation internally for timeslot routing; the on-air GSSI listener PDU keeps the usage marker and remains FACCH/STCH.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Preserved random-access ACKs remain exact `TetraAddress` matches.
  - ACK-only STCH may mirror `random_access_flag` for the same ISSI but does not consume it before the following channel-allocation STCH.
  - Another ISSI cannot mirror or consume that ACK, which is required for groups with many members.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_group_listener_floor_grant_with_speaker_id_stays_on_stch`.
  - Added group requester RA ACK regression for `2260082`-like floor grant.
  - Updated private RA ACK regression to prove P2P remains protected.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated D-TX GRANTED helper: ISSI/P2P stays compact; GSSI listener grants must carry speaker SSI.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib test_pending_random_access_ack_for_stch_waits_for_channel_allocation --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_group_listener_floor_grant_with_speaker_id_stays_on_stch --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_private_floor_grant_stch_carries_preserved_random_access_ack --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_group_floor_grant_stch_repeats_preserved_random_access_ack_for_requester --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_ul_inactivity_hands_floor_to_queued_requester --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_repeated_group_u_setup_same_gssi_during_hangtime_grants_existing_call_floor --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_repeated_group_u_setup_from_current_speaker_reasserts_existing_floor --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_simplex_p2p_u_tx_ceased_hands_floor_to_queued_requester --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 56 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 132 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this patch.
2. Deploy direct to `/home/chris/nexus-bs-v0.1.55-test` with local build only.
3. RF retest GSSI `226333`, especially repeated PTT from `2260082`.
4. Expected live evidence after fix: for `2260082` PTT, log should show individual `D-TX GRANTED`, GSSI `GrantedToOtherUser` with transmitting party `2260082`, then `NormalTrainSeq*` and `UMAC voice route` before any inactivity timeout.

## 2026-06-05 16:00:44 EEST - Deployed group floor notification hardening

Commit deployed:

- `3531b3c6` (`fix: harden group floor notification`)

Build/deploy:

- Command:
  - `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`
- Local build only; no compile on `chris@192.168.1.179`.
- Remote binary copied directly to `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs`; no binary backup created.
- Remote deployed binary SHA-256:
  - `52918b7ca64258dac9c89c7b7f6e77f6a5a2025f24e110949ed2503a635cc036`

Live startup evidence:

- Running process:
  - `/home/chris/nexus-bs-v0.1.55-test/bin/nexus-bs /home/chris/nexus-bs-v0.1.55-test/config.live.toml`
- Startup build line:
  - `Build: v0.1.55-3531b3c6`
- Startup registration/affiliation:
  - `2260616` registered and affiliated to `226333`.
  - `2260082` registered and affiliated to `226333`.
  - `2260618` registered and affiliated to `226333`.
- Startup filtered log sample showed no `RequestedServiceNotAvailable`, `Service unavailable`, `PTT denied`, or `Unit Not Attached`.

Next non-repeating execution:

1. RF retest GSSI `226333` with repeated PTT from `2260082` (MTP3550).
2. Confirm live log contains:
   - individual `D-TX GRANTED` to `2260082`;
   - GSSI `D-TX GRANTED/GrantedToOtherUser` carrying transmitting party `2260082`;
   - `NormalTrainSeq*` and `UMAC voice route` before inactivity timeout.
3. If static persists with those logs present, next investigation should be PHY/RSSI/vocoder path for 2260082, not CMCE floor denial.

## 2026-06-05 16:42:53 EEST - UMAC large-GSSI scheduler hardening

User goal:

- Make group operation robust for thousands of affiliated terminals, not only two or three radios.
- Continue clause-scoped ETSI EN 300 392-2 hardening without claiming formal certification.

Component in simple technical terms:

- UMAC scheduler is the layer that decides which downlink MAC PDU is sent on each radio slot.
- CMCE decides who may talk; UMAC makes the floor-control/status/SDS/grant messages actually fit onto MCCH/SCH-F or assigned-channel FACCH/STCH.
- For a large GSSI, the scheduler must keep one group-addressed message and repeat it only by real Energy Economy receive batches, not create one message per affiliated ISSI.

ETSI clause scope:

- EN 300 392-2 clauses 21.4.3.1 and 23.5.2.2.2: random-access ACK and slot-grant scheduling remain addressed to the exact requesting MS.
- EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6/T.210: downlink signalling to EG terminals is sent when the target MS or group batch is listening; T.210 is marked only for the batch that actually received the downlink.
- EN 300 392-2 clause 14.5.2.2.1: group floor listener notification remains one GSSI-addressed FACCH/STCH transfer for the group, not per-member signalling.
- Invalid local timeslot guard is internal robustness only; it does not alter valid over-air PDU semantics.
- This is engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added slot-scoped `GroupReadinessCache` so repeated readiness checks for the same GSSI in one scheduling opportunity reuse the current target list and awake/asleep result.
  - Avoided allocating a temporary uncovered-listener `Vec` just to answer "is any member listening?" for existing GSSI delivery/stealing state.
  - Reused the same FACCH/STCH readiness cache when building the selected group stealing state.
  - Added defensive downlink timeslot validation on enqueue/drop/ACK helper APIs so invalid `ts=0` or `ts=5` logs and returns instead of panicking.
  - Invalid reported STCH enqueue now marks its `TxReporter` discarded rather than leaving a permanent pending request.

Tests added:

- `test_large_mixed_eg7_gssi_facch_stealing_repeats_by_receive_batch_not_member`
- `test_large_gssi_readiness_cache_is_slot_scoped_across_queued_resources`
- `test_invalid_downlink_timeslot_enqueue_and_drop_apis_do_not_panic_or_mutate`

Verification:

- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 56 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 56 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_floor --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 134 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this UMAC large-GSSI scheduler hardening patch.
2. Continue with QA findings that remain open:
   - MM restart-recovery cache I/O amplification for thousands of ISSIs.
   - UMAC/TMA pending report bounds when downlink completion stalls.
   - DL media queue backpressure under sustained group-call overfeed.
3. Keep private simplex/duplex and group floor tests in the regression set before any deploy.

## 2026-06-05 16:49:28 EEST - MM restart recovery cache scaling

User goal:

- After BS restart, thousands of terminals must remain recoverable without "Unit Not Attached" drift or slow full-file cache churn.
- Keep group affiliation and scan-list recovery robust for lab GSSI `226333` and larger deployments.

Component in simple technical terms:

- MM is Mobility Management: it handles registration/attach, group affiliation state, energy economy negotiation, and BS-initiated `D-LOCATION UPDATE COMMAND` recovery.
- The restart recovery cache is a local Nexus-BS persistence file. It remembers local ISSIs and cached GSSI affiliation hints so the BS can reprobe camped terminals after process restart.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI may initiate registration with `D-LOCATION UPDATE COMMAND`.
- EN 300 392-2 clause 16.8.1/table 16.49: group identity attach/detach state is refreshed through MM group identity procedures.
- File caching is local implementation robustness only; it is not an over-air ETSI PDU change.
- This is engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - MM now keeps the restart recovery cache in memory for the current configured path.
  - Startup reads the cache once and arms restart recovery from the in-memory view.
  - `remember_restart_recovery_issi*` and `forget_restart_recovery_issi` no longer read the whole file per ISSI update.
  - Multiple same-window updates are coalesced and flushed from memory instead of forcing full-file write churn for every ISSI.
  - Path changes flush the old dirty cache and load the new path cleanly.
  - Added debug-only test helpers for cache dirty state, cache size, and forced flush.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added `test_restart_recovery_cache_coalesces_multiple_updates_until_flush`.

Verification:

- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_cache_coalesces_multiple_updates_until_flush --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 134 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_restart_recovery_cached_226333_group_restores_cmce_listeners_after_unrouted_ack --locked` -> 1 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Commit this MM restart recovery cache scaling patch.
2. Continue with remaining QA findings:
   - UMAC/TMA pending report bounds when downlink completion stalls.
   - DL media queue backpressure under sustained group-call overfeed.
3. Before deploy, rerun UMAC scheduler, MM, CMCE group/private, and diff checks.

## 2026-06-05 16:54:34 EEST - UMAC TMA pending report bounds

User goal:

- Keep Nexus-BS robust for long-running 24/7 operation with large groups and sustained signalling load.
- Avoid internal queues that can grow forever when RF/downlink completion stalls.

Component in simple technical terms:

- TMA is the MAC service primitive used by LLC/upper layers to submit a downlink TM-SDU and later receive a `TMA-REPORT`.
- UMAC tracks each submitted request with a `TxReporter`; when the scheduler transmits or discards the request, UMAC reports success or fragmentation failure back to LLC.

ETSI clause scope:

- EN 300 392-2 clause 20.4.1.1.1: `TMA-CANCEL` cancels a submitted `TMA-UNITDATA` request.
- EN 300 392-2 clause 20.4.1.1.3: `TMA-REPORT` reports MAC transfer completion state to LLC.
- EN 300 392-2 clause 22.3.2.3 uses MAC/TMA failure reporting for LLC retry/failure handling.
- The new cap/timeout is local resource-control hardening; it preserves the existing `FragmentationFailure` report for incomplete local MAC transfer and does not claim formal conformance.

Patch summary:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added `MAX_PENDING_TMA_REPORTS` cap for retained TMA report state.
  - Added local pending-report timeout guard for reporters that never reach transmitted/discarded.
  - Overflowed reported requests are immediately marked discarded and reported as `TmaReport::FragmentationFailure` instead of growing `pending_tma_reports`.
  - Added debug-only helpers for pending TMA report cap/count.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_tma_report_tracking_is_bounded_under_stalled_downlink_completion`.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs test_tma_report_tracking_is_bounded_under_stalled_downlink_completion --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 57 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Commit this UMAC TMA pending report bounds patch.
2. Continue with DL media queue backpressure under sustained group-call overfeed.
3. Before deploy, rerun UMAC scheduler, MM, CMCE group/private, and diff checks.

## 2026-06-05 16:57:19 EEST - DL media queue backpressure

User goal:

- Keep group/private call audio paths stable under sustained overfeed and long-running operation.
- Prevent stale queued speech frames from accumulating when producer rate exceeds the radio drain rate.

Component in simple technical terms:

- `CircuitMgr` owns active UL/DL traffic circuits and the per-timeslot downlink media queues.
- These queues hold ACELP or raw TCH/S blocks that are waiting to be transmitted on an assigned traffic channel.
- If the queue grows without bound, old speech can add latency, stale audio, or memory pressure; for live PTT voice, keeping the latest bounded window is safer.

ETSI clause scope:

- EN 300 392-2 clause 23.5 traffic-channel scheduling: this is local queue/backpressure behavior before selecting the next TCH/S block for a valid assigned channel.
- Floor release/grant still purges stale media at UMAC/CMCE boundaries; this patch only bounds ordinary per-timeslot media buildup.
- This is engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/circuit_mgr.rs`
  - Added `MAX_TX_DATA_BLOCKS_PER_TIMESLOT`.
  - `put_block` and `put_raw_tch_s_half_slot` now push through a bounded helper.
  - When full, the oldest queued DL media block is dropped and the newest block is retained.
  - Added unit tests for bounded ACELP and raw TCH/S overfeed.

Verification:

- `cargo test -p tetra-entities --lib umac::subcomp::circuit_mgr --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 57 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Commit this DL media queue backpressure patch.
2. Run a final combined verification set across scheduler, MM, UMAC, CMCE, and diff checks.
3. If deploy is requested, build locally and deploy direct to testing only; do not compile on the Pi and do not create binary backups.

## 2026-06-05 17:19:27 EEST - UMAC downlink signalling queue caps

User goal:

- Make group operation robust for thousands of terminals, not only two or three radios.
- Keep long-running Nexus-BS operation bounded when many terminals reattach, request floor, or receive group signalling at once.

Component in simple technical terms:

- UMAC is the MAC scheduler for the BS. It decides which downlink signalling PDU goes into each radio timeslot.
- The downlink signalling queues hold MAC-RESOURCE, random-access ACKs, slot grants, channel allocations, and FACCH/STCH signalling before over-air transmission.
- For large groups, these queues must have bounded memory growth while still preserving the control messages that keep PTT, attach, and call setup correct.

ETSI clause scope:

- EN 300 392-2 clause 21.4.3.1: random-access acknowledgement/grant timing is critical and must not be discarded as ordinary backlog.
- EN 300 392-2 clause 23.5.2.2.2: slot grants must keep correct timing semantics.
- EN 300 392-2 clause 23.5.2.2.7 and clause 23.7.6: downlink scheduling must account for energy-economy receive windows.
- EN 300 392-2 clause 14.5.2.1 and 14.5.2.2.1: group-call/floor signalling remains protected through the existing FACCH/STCH and grant paths.
- The queue caps are local implementation robustness; they are clause-scoped engineering hardening, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added bounded push helpers for per-timeslot downlink queues and the next-slot merge queue.
  - Preserved protected control signalling under backpressure: direct grants, pending grants, random-access ACKs, FACCH/STCH stealing, channel allocations, and MAC-RESOURCEs carrying integrated grant/RA ACK.
  - Ordinary queued MAC-RESOURCE/FragBuf backlog is discardable only through the existing reporter path, so upper layers can observe a local MAC transfer failure instead of waiting forever.
  - The next-slot merge path also enforces the cap after deferred signalling is merged back into the active timeslot queue.

Tests added:

- `test_downlink_scheduler_discards_reported_ordinary_resource_when_queue_cap_is_reached`
- `test_downlink_scheduler_backpressure_preserves_grants_over_ordinary_resources`

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 60 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 58 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 135 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this UMAC downlink signalling queue cap patch.
2. Continue hardening the remaining large-group risks:
   - global/message ingress queue backpressure for very large bursts,
   - restart recovery and group affiliation tests at multi-thousand scale,
   - active EG suspension memory shape for many simultaneous GSSI circuits.
3. Keep the regression set anchored on private simplex/duplex, group-call turn taking, SDS/status, MM attach/group affiliation, and UMAC EG7 scheduling before deploy.

## 2026-06-05 17:24:32 EEST - UMAC StayAlive large-GSSI state fast path

User goal:

- Make group operation robust for thousands of terminals, not just a two-radio lab case.
- Avoid per-member repeat state when a group has no Energy Economy subscribers.

Component in simple technical terms:

- `GroupDeliveryState` and `GroupStealingState` are UMAC bookkeeping objects.
- They remember which GSSI members have already had an EG receive opportunity for a group-addressed MAC-RESOURCE or FACCH/STCH block.
- If no group member has an Energy Economy assignment, all members are treated as continuously listening, so this repeat tracker is unnecessary.

ETSI clause scope:

- EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6 require the BS to account for Energy Economy receive windows.
- This patch keeps the EG repeat path unchanged when any affiliated target has an Energy Economy assignment.
- For pure `StayAlive` groups, one GSSI-addressed downlink remains sufficient; skipping the local repeat tracker is an implementation memory/CPU optimization, not an over-air PDU change.
- This is engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - `group_state_for_resource` now receives the current Energy Economy assignment map.
  - Large GSSI MAC-RESOURCE delivery skips `GroupDeliveryState` allocation when no target ISSI has an Energy Economy assignment.
  - GSSI FACCH/STCH stealing similarly skips `GroupStealingState` allocation for pure `StayAlive` groups.
  - Existing EG mixed/EG7 tests continue to exercise the full repeat-by-receive-batch path.

Tests added:

- `test_large_stayalive_gssi_resource_skips_group_delivery_state_snapshot`
- `test_large_stayalive_gssi_facch_transmits_once_without_group_stealing_state`

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 62 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 58 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 135 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this UMAC StayAlive large-GSSI fast path.
2. Continue with large-group restart/affiliation persistence tests at multi-thousand scale.
3. Review global `MessageQueue`/LLC queue behavior separately; do not add arbitrary drops without clause-scoped handling/reporting.

## 2026-06-05 17:26:21 EEST - UMAC deferred downlink queue cap test

User goal:

- Keep Nexus-BS robust under large group/restart signalling bursts, including traffic deferred into the next TDMA frame.

Component in simple technical terms:

- `dltx_next_slot_queue` is the scheduler holding area for downlink signalling that cannot fit in the current frame and must be tried again on the next frame.
- It is separate from the live per-timeslot queue, so it needs its own regression evidence.

ETSI clause scope:

- EN 300 392-2 clause 20.4.1.1.3: MAC reports local transfer completion/failure through TMA reporting.
- EN 300 392-2 clauses 21.4.3.1 and 23.5.2.2.2 remain protected by the production cap logic; this test covers ordinary deferred signalling only.
- This is local robustness evidence, not formal certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added a unit test proving `dltx_next_slot_queue` is capped and reports the discarded ordinary deferred request through `TxReporter`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 63 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this test-only deferred queue cap evidence patch.
2. Continue with CMCE large initial group setup fanout or MM restart recovery large-GSSI persistence tests.

## 2026-06-05 17:29:37 EEST - CMCE large group setup fanout evidence

User goal:

- Group call setup must scale to thousands of terminals without sending one call setup per ISSI.

Component in simple technical terms:

- CMCE is TETRA call control: it owns group/private call setup, release, and floor-control decisions.
- For a group call, CMCE should address setup signalling to the GSSI and open one UMAC traffic circuit for that group, not create per-terminal setup fanout.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1 defines normal group call setup using group identity scoped signalling.
- The test verifies the existing implementation remains group-scoped for a 2048-member GSSI; it is engineering regression evidence only, not formal certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_large_group_setup_uses_one_gssi_d_setup_and_one_umac_open`.
  - The test registers and affiliates 2048 ISSIs to one GSSI, starts a group call, and asserts:
    - exactly one `D-SETUP`,
    - the `D-SETUP` main address is the GSSI,
    - exactly one UMAC `Open`,
    - the traffic circuit primary address is the GSSI,
    - the initial speaker is only a secondary ISSI,
    - no release is emitted during setup.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_setup_uses_one_gssi_d_setup_and_one_umac_open --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 136 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this CMCE large group setup evidence patch.
2. Continue with MM restart recovery large-GSSI persistence and EG7 restart-derived group tests.

## 2026-06-05 17:32:38 EEST - CMCE/MM large restart-recovered GSSI evidence

User goal:

- After BS restart, thousands of terminals must remain attached to their groups and group PTT must not degrade to `PTT denied`, `No Group`, or unsolicited release.

Component in simple technical terms:

- MM restart recovery restores cached ISSI/GSSI affiliation when a terminal reappears after BS restart.
- CMCE consumes MM subscriber updates so call control knows which ISSIs are valid group listeners.
- This test proves the restored state is usable for real group floor control, not only visible in dashboard state.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI may initiate or recover registration using location update procedures.
- EN 300 392-2 clause 16.8.1: group identity attach/detach is confirmed through group identity procedures.
- EN 300 392-2 clause 14.5.2.1 and 14.5.2.2.1: group call setup and floor request handling remain GSSI scoped after recovery.
- The restart cache and large-scale test harness are local engineering evidence only, not formal certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_restart_recovery_large_cached_gssi_restores_cmce_listeners_and_turn_taking`.
  - Seeds the restart recovery cache with 2048 ISSIs affiliated to one GSSI.
  - Drives Demand Location Updating without group list for every ISSI.
  - Sends SwMI group refresh ACKs with non-matching handles to cover the field-observed unrouted ACK path.
  - Asserts all 2048 affiliates remain in the shared subscriber registry.
  - Starts a group call and verifies a restored listener receives `RequestQueued` for return PTT, with no release or UMAC close.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_restart_recovery_large_cached_gssi_restores_cmce_listeners_and_turn_taking --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 137 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this large restart-recovered GSSI evidence patch.
2. Continue with EG7 restart-derived large-group suspension/resume tests.
3. Keep the deploy gate as local build only; do not compile on the Pi and do not create binary backups.

## 2026-06-05 17:35:43 EEST - MM large restart-recovered EG7 activation evidence

User goal:

- EG7 must work for large restored groups after BS restart, not only for one terminal.
- Terminals restored from cache must not remain in a half-attached/no-group state while Energy Economy negotiation is pending.

Component in simple technical terms:

- MM owns registration, group restore, and Energy Economy negotiation.
- In EG7, the BS requests a long sleep-cycle mode, but the assignment only becomes active after the MS sends `U-MM STATUS` response confirming it.
- This test proves that 2048 restart-restored group members can all confirm EG7 and remain affiliated to the restored GSSI.

ETSI clause scope:

- EN 300 392-2 clauses 16.4.4 and 16.8.1 cover registration/group identity recovery procedures.
- EN 300 392-2 clauses 16.7.1, 16.10.9, 16.10.10, 23.5.2.2.7, and 23.7.6 cover Energy Economy negotiation and scheduling constraints.
- The restart cache is local implementation state; this remains engineering evidence only, not formal certification.

Patch summary:

- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added `test_restart_recovery_large_cached_group_eg7_activates_assignments_for_all_members`.
  - Seeds 2048 cached ISSI -> GSSI affiliations.
  - Drives ITSI attach without explicit group identities for every ISSI.
  - Confirms SwMI group refresh handling and then sends matching EG7 responses for every restored member.
  - Asserts all restored members remain group-affiliated and each has an active EG7 assignment with no assigned-channel suspension leakage.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_large_cached_group_eg7_activates_assignments_for_all_members --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 135 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this MM EG7 restart-recovery evidence patch.
2. Run a final focused combined regression across UMAC scheduler, UMAC integration, CMCE, MM, and diff checks.
3. Next implementation target after that: inspect global `MessageQueue` and LLC queue backpressure policies without dropping protocol-critical messages blindly.

## 2026-06-05 17:46:15 EEST - LLC outbound backlog caps for large groups

User goal:

- Group and SDS/status operation must remain robust with thousands of affiliated terminals, not only two or three radios.
- Local queues must not grow without bound under group traffic, EG scheduling delay, or MAC congestion.

Component in simple technical terms:

- LLC is the link layer between TETRA service users and MAC. It stores BL-DATA while waiting for ACK/retransmission and stores BL-UDATA for `N.253 + 1` repeated transmissions.
- For large groups, LLC must keep control/data queues finite, report local admission failure explicitly, and preserve already-submitted MAC work.

ETSI clause scope:

- EN 300 392-2 clause 20.4.1.1.3: MAC/LLC completion or failure is reported upward through TMA/TLA report semantics.
- EN 300 392-2 clause 22.3.2.3: acknowledged BL-DATA owns N(S), ACK, T.251, and retransmission once admitted.
- EN 300 392-2 clause 22.3.2.4.1: BL-UDATA is stored for `N.253 + 1` complete transmissions.
- The queue caps are local Nexus-BS resource-control hardening. They do not claim formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/llc/llc_bs_ms.rs`
  - Added finite LLC outbound caps sized for thousands of terminals:
    - `LLC_MAX_OUTBOUND_ACKED_MESSAGES = 8192`
    - `LLC_MAX_OUTBOUND_UDATA_MESSAGES = 8192`
  - BL-DATA now rejects new requests before N(S) allocation when the acknowledged backlog is full, preserving the basic-link sequence state and returning `TLA_REPORT_FAILED_TRANSFER`.
  - BL-UDATA now enforces capacity before creating new MAC work. If an incoming higher-priority UDATA arrives at capacity, LLC may evict only a lower-priority unsubmitted UDATA entry and reports that evicted service as failed.
  - Submitted MAC work, equal-priority FIFO entries, and existing priority-7 work are not evicted by the local cap.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib llc::llc_bs_ms::tests::udata_backlog_limit --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 80 passed.
- `cargo test -p tetra-entities --lib llc::llc_bs_ms::tests::outbound_backlog_limits_are_sized_for_thousands_of_terminals --locked` -> 1 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this LLC backpressure patch.
2. Integrate any returned agent audit findings into the next implementation target.
3. Continue with global `MessageRouter`/UMAC queue pressure and CMCE group floor robustness for thousands of listeners.

Agent audit integration:

- LLC/timers agent confirmed the global outbound caps are the correct first backpressure step. Remaining LLC work is per-link admission caps, per-tick standalone BL-ACK budget, O(n) scan reduction for TMA report routing, and T.251 timer scheduling that does not scan thousands of non-due entries every tick.
- QA agent prioritized large-group round-robin PTT, restart-recovered usable CMCE, SDS/status/WAP one-GSSI delivery, mixed EG3/EG7 resource batching, and LLC pressure tests at 4096+ scale.
- MM/EG agent flagged restart-recovery pacing fairness, mass T353 rollback/reprobe, mass T352 non-response fallback, and the combined case of restart-restored EG7 members entering an assigned-channel group call.
- UMAC/MAC EG agent flagged stale GSSI repeat snapshots after floor changes, late-affiliating EG listeners during active group calls, mixed StayAlive+EG groups retaining too much per-member repeat state, and 5000-member grant/RA storms exceeding existing 4096 scheduler cap assumptions.
- SDS/status/WAP agent flagged unbounded ingress/control queues before LLC caps, live SDS queue/repeat pressure, dashboard "sent" logging before confirmed acceptance, and missing queue/failure observability.
- CMCE group/private call auditor did not return before this commit gate; keep CMCE floor/private release robustness as an active next audit/patch target rather than treating it as completed.

## 2026-06-05 17:53:08 EEST - UMAC GSSI repeat state tracks only real EG listeners

User goal:

- Group call and group signalling must scale to thousands of terminals, including mixed StayAlive and EG members.
- A single EG terminal in a large group must not force Nexus-BS to retain per-member repeat state for every always-awake terminal.

Component in simple technical terms:

- UMAC/MAC scheduling decides when a GSSI-addressed MAC-RESOURCE or FACCH/STCH block is actually transmitted.
- Energy Economy members may sleep, so GSSI signalling is repeated until sleeping EG batches have had a listening window. StayAlive members are already listening and should not remain in the repeat snapshot.

ETSI clause scope:

- EN 300 392-2 clause 23.5.2.2.7 requires downlink scheduling to account for MS reception opportunities.
- EN 300 392-2 clause 23.7.6 defines Energy Economy sleep-cycle behaviour and T.210 activity handling.
- EN 300 392-2 clause 20.4.1.1.3 remains the reporter/completion context for retained MAC requests.
- This is local resource-control and scheduling hardening, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - GSSI `GroupDeliveryState` and `GroupStealingState` now retain only targets with a valid `EnergySavingAssignment::is_energy_economy()`.
  - StayAlive or fail-open members still make the first GSSI transmission ready, but they no longer inflate the retained repeat snapshot.
  - Invalid/fail-open EG entries, including unsupported frame-18 receive recurrence, no longer trigger GSSI repeat state.
  - Pruning of retained GSSI repeat state now rechecks current valid EG targets, so stale assignment changes can complete or shrink pending repeats.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched::tests::test_mixed_stayalive_eg_gssi_resource_tracks_only_energy_economy_targets --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched::tests::test_fail_open_energy_assignment_does_not_create_gssi_repeat_snapshot --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched::tests::test_large_mixed_eg7_gssi --locked` -> 2 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 65 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 58 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Commit this UMAC mixed EG/StayAlive repeat-state patch.
2. Continue with UMAC stale GSSI repeat invalidation on floor changes and late-affiliating EG listeners during active group calls.
3. Then address ingress/global `MessageQueue` and live SDS/WAP admission/observability caps.

## 2026-06-05 17:57:49 EEST - UMAC drops stale GSSI repeat snapshots on floor grant

User goal:

- Group floor changes must remain correct for large EG groups; late receive batches must not hear stale old-speaker signalling after a new PTT/floor grant.

Component in simple technical terms:

- CMCE decides who owns the group floor and sends the new `D-TX GRANTED`.
- UMAC may still have old GSSI repeat-state queued for EG listeners that were sleeping during an earlier batch.
- On a new group `FloorGranted`, UMAC now drops only already-created GSSI repeat snapshots for that group. Fresh unsent signalling for the new floor remains queued.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: SwMI floor control uses `D-TX GRANTED` to move transmission permission.
- EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6: EG-aware downlink repeats must match the relevant receive opportunities.
- This patch is local stale-state invalidation around those clauses, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added `dl_drop_queued_gssi_repeats`.
  - It removes only queued `group_state: Some` GSSI Resource/FragBuf/Stealing repeat items matching the group address.
  - It does not remove fresh `group_state: None` signalling for the same GSSI and does not remove repeat state for other groups.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - `CallControl::FloorGranted` now calls the stale-repeat dropper only for group-scoped bearers.
  - Private/P2P `FloorGranted` remains strict ISSI-participant scoped and does not invoke GSSI cleanup.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched::tests::test_floor_change_drops_only_requeued_gssi_repeat_state --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 66 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 58 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Commit this stale GSSI repeat invalidation patch.
2. Continue with late-affiliating EG listeners during active assigned-channel group calls.
3. Then address global ingress/control `MessageQueue` and live SDS/WAP admission/observability caps.

## 2026-06-05 18:02:08 EEST - UMAC late EG activation joins active group suspension

User goal:

- Terminals that join/activate Energy Economy while a group call is already active must not fall asleep and miss assigned-channel group traffic.

Component in simple technical terms:

- UMAC tracks active assigned-channel suspensions so EG radios stay awake during calls.
- Previously the suspension target list was a snapshot taken when the circuit opened. A later EG activation for an ISSI newly affiliated to the same GSSI could miss that active suspension.

ETSI clause scope:

- EN 300 392-2 clause 23.7.6: Energy Economy sleep cycle is suspended while the MS has an assigned channel/call active.
- EN 300 392-2 clauses 20.3.5.4.1c and 20.4.3: TLMC configuration carries energy-economy parameters from upper layers to MAC.
- This is local suspension-state robustness, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added `sync_active_suspensions_for_issi`.
  - When TLMC configures EG for an ISSI, UMAC now checks all active suspension keys and adds the ISSI if the current subscriber/group state is covered by an active GSSI/broadcast/ISSI suspension.
  - The new assignment starts with the correct `suspension_count`, and later close/resume decrements it normally.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_late_group_eg_activation_joins_active_assigned_channel_suspension`.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs test_late_group_eg_activation_joins_active_assigned_channel_suspension --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 59 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Commit this late EG active-suspension patch.
2. Continue with global ingress/control `MessageQueue` and live SDS/WAP admission/observability caps.
3. Continue CMCE large round-robin group PTT tests once the call-control auditor returns or after direct local inspection.

## 2026-06-05 18:14:35 EEST - CMCE call-id wrap skips live group/private calls

User goal:

- Group and private calls must remain robust at thousands of terminals and across long 24x7 runtime, not only with two or three radios.
- A fresh setup after call-id wrap must not overwrite an active or pending group/private call, because that can route PTT, release, or late-entry signalling to the wrong call.

Component in simple technical terms:

- `CircuitMgr` is the local traffic-channel manager. It chooses timeslot, usage number, and the SwMI call identifier for a new call.
- `CcBsSubentity` is the CMCE BS call-control state machine. It stores active group calls, private calls, cached `D-SETUP` PDUs, and pending `D-RELEASE`/`D-DISCONNECT` cleanup.
- The patch makes `CircuitMgr` ask CMCE which call identifiers are still occupied before allocating a new first-leg call identifier. Duplex private calls still intentionally reuse the same call-id for their second bearer leg because that is the same call.

ETSI clause scope:

- EN 300 392-2 clause 14.2.3: the CMCE call identifier is the call-handling reference allocated by the SwMI and then used by subsequent CMCE messages for that call.
- EN 300 392-2 table 14.36: call identifier is a 14-bit information element; value 0 is dummy and values 1..16383 identify calls.
- EN 300 392-2 clauses 14.5.1.1.2 and 14.5.1.2.1: individual/private call setup and initial floor state rely on the allocated call identifier.
- EN 300 392-2 clauses 14.5.2.1 and 14.5.2.2.1: group setup and group floor control use the group call identifier as the maintained call reference.
- EN 300 392-2 clause 14.5.2.3: group release keeps the call identifier relevant until release cleanup completes.
- This is clause-scoped hardening and test evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/cmce/components/circuit_mgr.rs`
  - Added `get_next_call_id_avoiding` over the full 14-bit non-zero call-id range.
  - Added `CircuitErr::CallIdentifierExhausted` for the pathological case where every real call-id is occupied.
  - Added allocator variant `allocate_circuit_with_allocator_duplex_avoiding`.
  - Releases a just-reserved timeslot if call-id selection fails or circuit opening rejects, so a failed setup does not leak local timeslot state.
  - Added `active_call_ids` as a defensive backstop for circuit state that has not yet been reflected into higher CMCE maps.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added `occupied_call_ids`, covering cached setups, active group calls, pending group releases, live individual calls, all pending private release/tail-drain maps, circuit-manager active ids, and echo session id.
  - Added hidden debug accessors for integration tests.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Local group setup, local private/P2P setup, Brew-originated local private setup, and echo setup now allocate first-leg call identifiers while avoiding occupied CMCE ids.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/network.rs`
  - Network-origin private and group setup now use the same occupied-id avoiding allocator.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added a group collision regression: keep group call A active, force allocator wrap to A's call-id, start group call B, then verify A still queues return PTT with the original call-id and no release/close side effects.
  - Added a private collision regression: keep a private setup call-id live, force allocator wrap to that id, start a group call, and verify the group call receives a different id.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib call_identifier --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_call_id_wrap_skips_live_group_call_and_preserves_ptt --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_setup_call_id_wrap_skips_live_private_call --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 139 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this CMCE call-id continuity patch.
2. Extend large-GSSI stress beyond current 2048-member tests toward 4096/5000 members for round-robin PTT, repeated `U-SETUP`, queued handoff, restart recovery, and EG7 listeners.
3. Continue SDS/WAP admission and truthful observability so live SDS/WAP cannot silently claim delivery when queues are saturated.
4. Continue MM affiliation persistence and restart recovery scale tests so terminals do not return as `No Group`/`Unit Not Attached` after BS restart.

## 2026-06-05 18:17:27 EEST - CMCE large GSSI tests raised to 4096 members

User goal:

- Group call handling must be robust for thousands of terminals, not just two or three lab radios.

Component in simple technical terms:

- CMCE group-call setup and floor control should remain group-scoped: one GSSI `D-SETUP`, one GSSI listener grant on handoff, and one bounded queued floor owner.
- MM restart-recovery must restore enough affiliation state that CMCE can still accept and queue return PTT after a BS restart.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1: normal group call setup is addressed to the group identity.
- EN 300 392-2 clause 14.5.2.2.1: SwMI floor control grants, queues, or denies transmission permission without creating per-member group setup fanout.
- EN 300 392-2 clause 16.8.1: group attach/detach acknowledgement is the confirmation point used by the restart-recovery tests.
- This is scale regression evidence for the existing clause-scoped behavior, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `LARGE_GSSI_MEMBER_COUNT = 4096`.
  - Raised these CMCE regressions from 2048 to 4096 affiliated ISSIs:
    - large group setup emits one GSSI `D-SETUP` and one UMAC open.
    - large group PTT handoff emits one requester grant plus one GSSI listener grant.
    - large group floor queue remains bounded and later contenders do not replace the first queued requester.
    - restart-recovered cached GSSI restores CMCE listeners and return PTT works after attach/ACK refresh.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs large --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 139 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this 4096-member CMCE test-scale patch.
2. Continue SDS/WAP admission and truthful delivery/queue observability.
3. Continue MM affiliation persistence tests at EG7 and restart scale.
4. Add UMAC/MAC large EG batch pressure tests so 4096-member GSSI scheduling remains bounded below CMCE.

## 2026-06-05 18:24:48 EEST - Live SDS admission bounded for dashboard/WAP robustness

User goal:

- SDS/WAP features must stay robust during long-running BS operation and must not silently grow unbounded control state.
- The WAP MVP delivery path must keep working while live text-style SDS broadcast remains fail-closed for WAP PIDs that require their own raw payload encoder.

Component in simple technical terms:

- Live SDS is the dashboard/control queue for operator-injected broadcast messages, transmitted later by the Home Mode Display/SDS-TL sender.
- WAP MVP uses raw SDS Type4/WAP payload helpers, not the text-style live SDS queue.
- This patch caps only the live SDS broadcast queue. It does not limit normal SDS, raw SDS WAP delivery, or status delivery.

ETSI clause scope:

- EN 300 392-2 clause 13.2: SDS includes individual and group short data/status services.
- EN 300 392-2 clause 29.3.3.8.2: SDS-TL system broadcast may use the all-ones broadcast address.
- EN 300 392-2 clause 29.4.1 and table 29.21: SDS-TL transport PIDs are distinct from WAP/WCMP application PIDs; WAP raw Type4 remains on the raw SDS path.
- This is bounded local admission/observability hardening around SDS/WAP, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-config/src/bluestation/state.rs`
  - Added `LIVE_SDS_QUEUE_MAX_LEN = 256` for runtime live SDS broadcast entries.
- `crates/tetra-entities/src/cmce/cmce_bs.rs`
  - `AddLiveSds` now rejects new live SDS entries when the queue is full before allocating an ID or mutating state.
- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Dashboard live SDS POST now checks the shared queue and returns HTTP 429 when full.
  - Dashboard live SDS POST now returns HTTP 503 if the CMCE control channel is unavailable, rather than reporting OK after a failed send.
- `crates/tetra-entities/tests/test_sds_bs.rs`
  - Added `test_live_sds_control_queue_is_bounded`, proving overflow is rejected without evicting accepted broadcasts, consuming an ID, or emitting an RF message.

Verification:

- `cargo fmt --package tetra-config --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_sds_bs live_sds --locked` -> 5 passed.
- `cargo test -p tetra-entities --test test_sds_bs wap --locked` -> 12 passed.
- `cargo test -p tetra-entities net_dashboard::server::tests::live_sds --locked` -> 6 relevant dashboard tests passed.
- `cargo test -p tetra-entities --test test_sds_bs --locked` -> 117 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this bounded live SDS admission patch.
2. Continue MM restart/affiliation persistence with EG7-scale tests.
3. Continue UMAC/MAC EG batch pressure below CMCE for 4096-member GSSI scheduling.
4. Add dashboard/API observability for accepted-vs-transmitted WAP/SDS where a synchronous response path exists.

## 2026-06-05 18:28:08 EEST - MM restart recovery EG7 scale raised to 4096 members

User goal:

- After BS restart, terminals must not come back as `Unit Not Attached` or `No Group`; cached affiliations and EG mode must converge robustly for thousands of terminals.

Component in simple technical terms:

- MM owns registration, group affiliation, restart recovery, and energy-economy negotiation.
- The restart recovery cache seeds known ISSIs/GSSIs after process restart; MM then refreshes the group on air and waits for explicit terminal ACK/EG response.
- EG7 is the longest configured energy economy mode in this test, so it is the harshest case for restart recovery plus sleeping terminals.

ETSI clause scope:

- EN 300 392-2 clauses 16.4 and 16.8: registration and group attach/detach procedures restore the MS and its group identities.
- EN 300 392-2 clauses 16.7.1, 16.10.9 and 16.10.10: energy-economy mode is negotiated and activated after the matching MS response.
- EN 300 392-2 clause 23.7.6 and table 23.9: EG7 has the longest sleep-cycle behavior, so it must not be activated speculatively before explicit response.
- This is scale regression evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added `LARGE_RESTART_RECOVERY_MEMBER_COUNT = 4096`.
  - Raised `test_restart_recovery_large_cached_group_eg7_activates_assignments_for_all_members` from 2048 to 4096 members.
  - The test now confirms 4096 cached members can ITSI attach, receive cached group refresh, ACK it, explicitly respond to EG7, remain affiliated to the GSSI, and receive an EG7 assignment.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_large_cached_group_eg7_activates_assignments_for_all_members --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 135 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this MM 4096-member restart recovery/EG7 test-scale patch.
2. Continue UMAC/MAC EG batch pressure below CMCE for 4096-member GSSI scheduling.
3. Continue dashboard/API observability for accepted-vs-transmitted WAP/SDS where a synchronous response path exists.

## 2026-06-05 18:39:33 EEST - UMAC STCH floor grants prioritized under 4096-entry group pressure

User goal:

- Group calls must remain robust for thousands of terminals, not just two or three radios.
- A queued requester that becomes the next speaker must receive the positive floor grant promptly even if the group has generated thousands of lower-value busy/queued responses.

Component in simple technical terms:

- UMAC/MAC scheduler is the layer that chooses which downlink control block is placed into the next radio timeslot.
- STCH/FACCH stealing is the assigned-channel control path used during voice traffic for urgent call-control messages such as `D-TX GRANTED`, `D-TX CEASED`, and `D-TX INTERRUPT`.
- This patch does not change the CMCE floor-control decision; it changes only which already-built STCH control block is transmitted first when the assigned-channel queue is under heavy group pressure.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: SwMI group-call floor control uses `D-TX GRANTED`, `D-TX CEASED`, and related responses to grant, queue, reject, or withdraw transmission permission.
- EN 300 392-2 clause 23.5: STCH/FACCH is the assigned-channel signalling path during traffic.
- EN 300 392-2 clause 23.5.2.2.7 remains relevant because assigned uplink opportunities must be reserved/advertised coherently with downlink control.
- This is clause-scoped scheduler hardening and regression evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added STCH scheduling priority derived from the actual MAC-RESOURCE/CMCE bitstream already queued for transmission.
  - Keeps `D-TX INTERRUPT` and `D-TX CEASED` ahead of lower-value floor responses so withdrawal/preemption ordering is not inverted.
  - Prioritizes positive `D-TX GRANTED` with an uplink channel allocation (`UL`/`Both`) ahead of DL-only `RequestQueued`/`NotGranted` backlog.
  - Preserves FIFO ordering within the same priority class.
  - Added `test_large_group_positive_floor_grant_stch_preempts_busy_response_backlog`, which queues 4096 DL-only `RequestQueued` STCH responses before a positive requester grant and proves the positive UL+DL grant is transmitted first.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib test_large_group_positive_floor_grant_stch_preempts_busy_response_backlog --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 67 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 59 passed.
- `cargo test -p tetra-entities --test test_cmce_bs large --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 139 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this UMAC STCH large-group floor-grant priority patch.
2. Add a scheduler/integration regression that proves `D-TX INTERRUPT` remains before preemptive `D-TX GRANTED` on the air path, not just in CMCE message order.
3. Continue pending group/private release call-id wrap and pending-release flood tests.
4. Continue SDS/WAP accepted-vs-transmitted observability and long-run bounded queues.

## 2026-06-05 18:45:46 EEST - UMAC STCH backpressure bounded for low-value group floor responses

User goal:

- The BS must stay robust with thousands of terminals in one group and must not grow unbounded STCH control queues during PTT storms.
- Preemptive floor-control ordering must stay correct: withdraw/interruption before the new grant.

Component in simple technical terms:

- Backpressure is the scheduler's safety valve when too many downlink control messages are waiting for one traffic timeslot.
- `D-TX GRANTED/RequestQueued/NotGranted` DL-only responses are useful feedback, but under a storm they are lower value than the one positive grant that lets the next speaker enter U-plane.
- `D-TX INTERRUPT` and `D-TX CEASED` are floor-withdrawal messages; those remain protected because they stop or move the current speaker.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: floor-control responses grant, queue, reject, or withdraw permission to transmit.
- EN 300 392-2 clause 14.5.2.2.1 f): transmission interruption withdraws the current permission before the new speaker is advertised.
- EN 300 392-2 clause 23.5: these messages are carried on assigned-channel STCH/FACCH during traffic.
- This is queue robustness and ordering regression evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Backpressure no longer protects every `DlSchedElem::Stealing` item blindly.
  - Protects floor-withdrawal STCH (`D-TX INTERRUPT`, `D-TX CEASED`) and positive `D-TX GRANTED` with `UL`/`Both` channel allocation.
  - Allows lower-value DL-only `D-TX GRANTED` outcomes (`RequestQueued`, `NotGranted`, listener-only `GrantedToOtherUser`) to be shed when the STCH queue is full.
  - Added `test_preemptive_floor_interrupt_stch_stays_ahead_of_positive_grant`, proving the air-path scheduler sends interrupt before a positive grant even when the grant was queued first.
  - Tightened the 4096-entry large-group test to assert the queue stays bounded at `MAX_DLSCHED_ELEMS_PER_TIMESLOT` while preserving/transmitting the positive grant.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib test_large_group_positive_floor_grant_stch_preempts_busy_response_backlog --locked` -> 1 passed after the bounded-backpressure assertion.
- `cargo test -p tetra-entities --lib test_preemptive_floor_interrupt_stch_stays_ahead_of_positive_grant --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 68 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 59 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this bounded STCH backpressure/preemption regression patch.
2. Continue pending group/private release call-id wrap tests.
3. Continue large pending-release PTT flood tests.
4. Continue restart-recovered EG7 affiliation through UMAC scheduling.

## 2026-06-05 18:50:03 EEST - CMCE pending-release call-id wrap regressions added

User goal:

- Group and private calls must remain stable across repeated setup/release cycles and must not confuse old release signalling with a fresh call when the 14-bit call identifier wraps.

Component in simple technical terms:

- CMCE owns call setup, call release, and the short call identifier used in all call-control PDUs.
- Pending release means the BS has started clearing the call, but it is still waiting for FACCH/MCCH delivery reports or local guard timers before the old call identity can be reused safely.
- These tests force the allocator to try reusing the old identifier while release is pending and prove it skips to a fresh identifier instead.

ETSI clause scope:

- EN 300 392-2 clause 14.2.3 and table 14.36: the SwMI call identifier is the call reference and is only 14 real bits.
- EN 300 392-2 clause 14.5.2.3: group-call release uses `D-RELEASE` and may remain locally pending while delivery drains.
- EN 300 392-2 clauses 14.5.1.3.2 and 14.5.1.3.3: individual/private-call clearing must keep the call reference coherent until the release path completes.
- This is clause-scoped regression evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_group_pending_release_call_id_wrap_skips_old_release_id`.
  - Added `test_p2p_pending_release_call_id_wrap_skips_old_release_id`.
  - Both tests force `next_call_identifier` to the old pending-release call id, start a fresh call, and assert the new setup uses a different call id while the old pending id remains occupied.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_pending_release_call_id_wrap_skips_old_release_id --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_pending_release_call_id_wrap_skips_old_release_id --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 141 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this CMCE pending-release call-id wrap test patch.
2. Continue large pending-release PTT flood tests.
3. Continue restart-recovered EG7 affiliation through UMAC scheduling.
4. Continue SDS/WAP accepted-vs-transmitted observability and long-run bounded queues.

## 2026-06-05 18:56:04 EEST - CMCE large pending group PTT flood regression added

User goal:

- Group calls must remain robust for thousands of terminals in one GSSI, not only for two or three radios.
- During group-call release, late or repeated PTT attempts from many affiliated terminals must not restart the old call, steal the floor, evict the pending call id, or generate contradictory downlink signalling.

Component in simple technical terms:

- CMCE is the call-control state machine: it owns group setup, floor/turn-taking, and release.
- Pending release means the BS already sent `D-RELEASE`, but keeps the old call locally alive until the release can drain over FACCH/MCCH or a bounded guard expires.
- This regression floods the pending-release state with 4095 additional `U-TX DEMAND` messages from a 4096-member GSSI and proves the BS ignores them without reopening floor control.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.3: group-call release is cleared with `D-RELEASE`, so the old group call remains in release handling until the SwMI completes local cleanup.
- EN 300 392-2 clause 14.5.2.2.1: group floor-control PDUs grant, queue, reject, cease, or withdraw transmission permission; while release is pending, old-call floor traffic must not resume normal turn-taking.
- This is clause-scoped regression evidence and queue/state robustness hardening, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_group_pending_release_large_ptt_flood_is_ignored_without_signalling`.
  - Registers and affiliates 4096 members to one GSSI.
  - Starts a group call, enters pending release, then submits `U-TX DEMAND` from every other member.
  - Asserts the flood emits no `D-TX GRANTED`, no `D-TX CEASED`, no extra `D-RELEASE`, no UMAC floor grant/release, and no premature UMAC call close.
  - Asserts the pending call id remains occupied, preventing reuse confusion while release is still draining.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_pending_release_large_ptt_flood_is_ignored_without_signalling --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 142 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this CMCE large pending-release flood regression.
2. Add the analogous P2P pending-release flood regression so private simplex teardown also stays bounded under repeated stale floor attempts.
3. Continue restart-recovered EG7 affiliation through real UMAC scheduling.
4. Continue SDS/WAP accepted-vs-transmitted observability and long-run bounded queues.

## 2026-06-05 19:00:03 EEST - CMCE large repeated GSSI U-SETUP floor alias regression added

User goal:

- Group PTT must stay robust with thousands of affiliated terminals, including terminals that signal a same-GSSI PTT as a repeated `U-SETUP` rather than a plain `U-TX DEMAND`.
- A PTT storm from a maintained group call must not fan out new setup transactions, new traffic circuits, or release/error signalling.

Component in simple technical terms:

- CMCE maps a repeated `U-SETUP` for the already-active GSSI to floor control.
- The first non-speaker requester may be queued for the next turn; later contenders are explicitly told `NotGranted` and are not retained as extra waiters.
- This keeps the group floor queue bounded at one waiter even when 4096 group members contend.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.1: group setup applies when establishing the call.
- EN 300 392-2 clause 14.5.2.2.1: once the group call is maintained, transmit permission is controlled through floor-control responses such as queued/granted/not-granted.
- This is clause-scoped regression evidence and queue/state hardening, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_large_group_repeated_u_setup_floor_alias_is_bounded_without_setup_fanout`.
  - Registers and affiliates 4096 members to one GSSI, starts a real group call, queues the first repeated same-GSSI `U-SETUP`, then floods repeated `U-SETUP` from every other member.
  - Asserts no `D-CALL PROCEEDING`, no `D-CONNECT`, no `D-SETUP`, no `D-RELEASE`, no UMAC open/close, and no premature UMAC floor grant during the storm.
  - Asserts later contenders receive `NotGranted` without replacing the first queued requester.
  - Asserts `U-TX CEASED` from the current speaker hands the floor only to the first queued requester plus one GSSI listener notification.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_repeated_u_setup_floor_alias_is_bounded_without_setup_fanout --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 143 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this CMCE large repeated same-GSSI `U-SETUP` floor-alias regression.
2. Add the analogous P2P pending-release flood regression so private simplex teardown also stays bounded under repeated stale floor attempts.
3. Continue SDS/WAP accepted-vs-transmitted observability and long-run bounded queues.
4. Continue global ingress queue pressure tests for attach + SDS + PTT bursts.

## 2026-06-05 19:02:48 EEST - CMCE pending group release ignores non-owner stale release/disconnect

User goal:

- Group call release must be stable under repeated/stale uplink traffic from many group members, without extra `D-RELEASE` responses that can confuse terminals.
- This specifically hardens the case where the group owner already started release and another affiliated MS sends late `U-DISCONNECT` or `U-RELEASE` for the same call id.

Component in simple technical terms:

- CMCE keeps a group call in `pending_group_releases` while the FACCH/MCCH `D-RELEASE` drains.
- During that period the old call id is still visible locally, but the call is already clearing.
- Late non-owner release/disconnect PDUs for that call are stale; the BS now ignores them instead of generating service-unavailable `D-RELEASE` noise.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.3: group-call clearing is performed by SwMI `D-RELEASE`; the procedure does not require an MS response.
- EN 300 392-2 clause 14.5.2.2.1 remains relevant because stale uplink control traffic must not restart or contradict floor/call state while release is pending.
- This is clause-scoped release-state hardening, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Added pending group-release guards in the group branches of `fsm_on_u_release` and `fsm_on_u_disconnect`.
  - If the `call_id` is already pending group release, CMCE logs and returns before owner/non-owner rejection logic can emit another `D-RELEASE`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_group_pending_release_ignores_non_owner_disconnect_release_without_extra_signalling`.
  - Starts a group call, enters pending release through owner `U-DISCONNECT`, then submits non-owner `U-DISCONNECT` and `U-RELEASE`.
  - Asserts no extra `D-RELEASE` and no premature UMAC close.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_pending_release_ignores_non_owner_disconnect_release_without_extra_signalling --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 144 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this CMCE pending group-release stale non-owner guard.
2. Extend the large pending-release flood test to prove release still completes after reporter transmission.
3. Add the analogous P2P pending-release flood regression so private simplex teardown remains bounded.
4. Continue SDS/WAP accepted-vs-transmitted observability and global ingress pressure tests.

## 2026-06-05 19:05:05 EEST - CMCE large pending group flood now proves release completion

User goal:

- Large GSSI PTT storms must not leave stale calls or call identifiers stuck forever.
- A 4096-member late PTT flood during group release must be harmless and the original release must still finish when `D-RELEASE` transmission is reported.

Component in simple technical terms:

- `TxReporter` is the local delivery state for a downlink PDU.
- CMCE keeps the group bearer and call id alive while the `D-RELEASE` reporter is pending.
- The strengthened test now proves that after the flood, marking the release reporter transmitted closes the UMAC call and frees the old call id.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.3: group calls are cleared by SwMI `D-RELEASE`.
- EN 300 392-2 clause 14.2.3/table 14.36: the call identifier remains the call reference and must not be reused or leaked while release is pending.
- This is clause-scoped release-completion evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Extended `test_group_pending_release_large_ptt_flood_is_ignored_without_signalling`.
  - Extracts the FACCH `D-RELEASE` reporter from the initial release.
  - After the 4095-message late `U-TX DEMAND` flood, marks the reporter transmitted.
  - Asserts UMAC close/call-ended signalling occurs and the old pending call id is no longer active.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_pending_release_large_ptt_flood_is_ignored_without_signalling --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 144 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this CMCE large pending-release flood completion regression.
2. Add the analogous P2P pending-release flood regression so private simplex teardown remains bounded.
3. Continue SDS/WAP accepted-vs-transmitted observability and global ingress pressure tests.
4. Re-run targeted UMAC large EG7/group tests before the next field deploy.

## 2026-06-05 19:13:38 EEST - CMCE P2P pending release duplicate flood regression added

User goal:

- Private simplex/P2P call teardown must remain robust after stale PTT/disconnect bursts and must not regress the MXP600-safe release path.
- A pending private-call release must not emit contradictory `D-TX`, `D-DISCONNECT`, or duplicate `D-RELEASE` signalling under a 4096-message stale flood.

Component in simple technical terms:

- P2P/private simplex release has two related pieces: prompt `D-RELEASE` to the MS that requested disconnect, and peer clearing with `D-DISCONNECT` followed by expected peer `U-RELEASE`.
- The new regression floods duplicate initiator `U-DISCONNECT` and peer `U-TX DEMAND` while that release path is pending.
- After the flood, the expected peer `U-RELEASE` must still close the call and free the call id once the initiator's `D-RELEASE` reporter is transmitted.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.3.2 and 14.5.1.3.3: individual-call clearing uses `D-RELEASE`/`D-DISCONNECT` sequencing.
- EN 300 392-2 clauses 14.7.1.6 and 14.7.2.9: peer `U-RELEASE` is the response path to `D-DISCONNECT`.
- This is clause-scoped private-call teardown regression evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_p2p_pending_release_large_duplicate_disconnect_ptt_flood_is_ignored_and_closes`.
  - Starts an active private simplex call, moves floor to the called party, begins disconnect release, and marks peer `D-DISCONNECT` transmitted.
  - Injects 4096 stale duplicate initiator `U-DISCONNECT` / peer `U-TX DEMAND` messages.
  - Asserts no `D-DISCONNECT`, no duplicate `D-RELEASE`, no `D-TX GRANTED`, no UMAC floor grant, and no premature close.
  - Marks the initiator `D-RELEASE` reporter transmitted, sends the expected peer `U-RELEASE`, then asserts UMAC close and call-id cleanup.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_pending_release_large_duplicate_disconnect_ptt_flood_is_ignored_and_closes --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 145 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this P2P pending-release duplicate flood regression.
2. Continue SDS/WAP accepted-vs-transmitted observability and global ingress pressure tests.
3. Re-run targeted UMAC large EG7/group tests before next field deploy.
4. Continue network-origin call-id wrap regressions for group/private call starts.

## 2026-06-05 19:18:13 EEST - UMAC grant/RA ACK coalescing bounds 4096-requester bursts

User goal:

- Group/PTT robustness must scale to thousands of terminals without internal scheduler queues growing beyond local caps.
- Random-access acknowledgement plus slot grant must remain coherent for each requester while staying bounded under 4096 simultaneous requesters.

Component in simple technical terms:

- UMAC scheduler owns the downlink MAC-RESOURCE queue that carries slot grants and random-access ACK flags.
- Before this patch, `Grant` and `RandomAccessAck` were both protected from backpressure, so 4096 requesters could create 8192 protected queue elements before later integration collapsed them.
- The scheduler now coalesces ready grant/ACK pairs for the same address into one minimal MAC-RESOURCE at enqueue time, preserving the final over-air fields while keeping transient queue length bounded.

ETSI clause scope:

- EN 300 392-2 clause 21.4.3.1: `random_access_flag` acknowledges successful random access for the addressed MS.
- EN 300 392-2 clause 23.5.2.2.2: slot grant signalling must remain coherent with the addressed MS and uplink reservation.
- EN 300 392-2 clause 23.5 covers the MAC downlink control path carrying these MAC-RESOURCEs.
- The queue cap is local robustness hardening; this is not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added enqueue-time coalescing for ready `Grant`/`RandomAccessAck` elements targeting the same `TetraAddress`.
  - Merges either order: grant then ACK, ACK then grant, or either into an existing MAC-RESOURCE.
  - Preserves usage marker and grant fields while setting `random_access_flag`.
  - Updated `test_mass_random_access_grant_ack_integration_uses_one_resource_per_issi` from 2048 to 4096 requesters and asserts the transient queue stays at 4096, not 8192.
  - Updated `test_dl_grant_and_ack_integration` to expect early coalescing into the existing MAC-RESOURCE.

Verification:

- `cargo fmt --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --lib test_mass_random_access_grant_ack_integration_uses_one_resource_per_issi --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 68 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 59 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit this UMAC bounded grant/RA ACK coalescing patch.
2. Continue cross-layer UMAC test for large group PTT storm prioritizing requester grant with preserved RA ACK.
3. Continue SDS/WAP accepted-vs-transmitted observability and global ingress pressure tests.
4. Continue network-origin call-id wrap regressions for group/private starts.

## 2026-06-05 19:34:20 EEST - UMAC TMA admission preserves critical group floor grant under 4096-message storm

User goal:

- Group calls must remain robust with thousands of terminals, not only two or three radios.
- A valid requester must receive the first positive floor grant and assigned-channel allocation even when thousands of lower-value busy/not-granted responses are already queued.
- The BS must remain bounded; the fix must not solve the problem by growing unbounded queues.

Component in simple technical terms:

- UMAC receives `TMA-UNITDATA.req` from LLC/CMCE and turns it into MAC downlink signalling.
- `pending_tma_reports` is the local UMAC list that remembers which TMA requests still need a final MAC report back to LLC.
- The bug was before the radio scheduler: 4096 ordinary busy/not-granted TMA reports filled this local report list, so the 4097th TMA request, the positive `D-TX GRANTED` for the next speaker, was discarded before it could reach STCH/FACCH.
- UMAC now assigns admission priority to TMA requests. Critical assigned-channel floor-control PDUs (`D-TX GRANTED` with UL/Both allocation, `D-TX CEASED`, `D-TX INTERRUPT`) can evict one lower-priority pending ordinary report under the local cap. Ordinary overload remains fail-closed with `TMA-REPORT.ind FragmentationFailure`.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: group-call transmission request/floor signalling, including positive grants and queued/not-granted responses.
- EN 300 392-2 clause 20.4.1.1.3: MAC reports TMA request progress/failure using `TMA-REPORT.ind`.
- EN 300 392-2 clause 21.4.3.1: `random_access_flag` acknowledges the requester's random access.
- EN 300 392-2 clauses 23.5 and 23.5.2.2.1: STCH/FACCH assigned-channel signalling carries the floor-control MAC-RESOURCE.
- This is clause-scoped engineering hardening and regression evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added `TmaAdmissionPriority` and CMCE PDU inspection for TMA admission.
  - Retained the 4096 pending-report cap.
  - On critical incoming TMA under cap pressure, evicts one lower-priority pending ordinary TMA report, cancels its queued scheduler element by `TxReporter`, and emits `TMA-REPORT.ind FragmentationFailure` for that lower-priority request.
  - Keeps equal/higher-priority cap pressure fail-closed.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_large_group_ptt_storm_prioritizes_requester_grant_with_preserved_ra_ack`.
  - Builds an active GSSI call, preserves the requester's RA ACK through hangtime cleanup, queues 4096 lower-value `D-TX GRANTED(NotGranted)` STCH requests, then submits the positive requester `D-TX GRANTED(Granted)` with `UlDlAssignment::Both`.
  - Asserts the requester grant is transmitted before busy responses, carries channel allocation `Both`, and repeats `random_access_flag=true`.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs test_large_group_ptt_storm_prioritizes_requester_grant_with_preserved_ra_ack --locked` -> 1 passed.
- `rustfmt --edition 2024 crates/tetra-entities/src/umac/umac_bs.rs crates/tetra-entities/tests/test_umac_bs.rs` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 60 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 68 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 145 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Run `git diff --check`, then commit this UMAC TMA admission/floor-grant storm patch.
2. Add CMCE->UMAC integrated 4096-member handoff test: real CMCE PTT contenders, `U-TX CEASED`, decoded STCH/FACCH grant, and voice route/source speaker.
3. Add restart recovery EG7 -> CMCE -> UMAC assigned-channel test with 4096 cached group members.
4. Continue SDS/WAP accepted-vs-transmitted observability and global ingress pressure tests.

## 2026-06-05 - PM checkpoint: Nexus-BS v0.1.55 clause-scoped hardening at scale

Workstream state:

- Nexus-BS v0.1.55 hardening remains clause-scoped engineering work against ETSI EN 300 392-2. Do not claim whole-stack formal TETRA certification without official conformance-suite/lab evidence.
- Basic service targets stay unchanged: robust group call, private call simplex and duplex, SDS/status, WAP home-page delivery, MM attach/group affiliation persistence, scan-safe group state, and long-running BS operation without forgetting terminals.
- Scale requirement is explicit: tests and designs must cover thousands of terminals or bounded large-storm equivalents, not only two or three radios.

Component map in simple terms:

- PM/log: keeps the execution trail and next actions clear so workers do not loop.
- MM: owns terminal registration, attach state, group affiliation, and energy saving mode such as EG3/EG7.
- CMCE: owns call control: group call, private call, floor/PTT state, call setup, and release.
- SDS/status: owns short data and status messages over the TETRA signalling path.
- WAP: owns the small terminal browser service and Nexus-BS welcome page delivery.
- LLC/MLE: wraps CMCE/SDS data before UMAC and manages acknowledged/unacknowledged logical link delivery.
- UMAC/MAC: turns upper-layer requests into downlink MAC resources, grants, random-access ACKs, and assigned-channel signalling.

Current active technical focus:

- UMAC/MAC large-group floor-grant storm work is active. The current patch line protects positive group floor grants and random-access ACKs under 4096-message pressure, then extends the same priority recognition through the real LLC `BL-UDATA` + MLE `Cmce` wrapping path, not only direct synthetic CMCE test payloads.
- The field symptoms still requiring proof are repeated group PTT static audio on selected terminals, first-PTT/private-call edge cases, and restart recovery where terminals must return attached with coherent group affiliation.

Next concrete execution:

1. Finish the dirty UMAC/MAC wrapped-payload priority patch without touching unrelated files.
2. Add a CMCE->MLE->LLC->UMAC integrated large-group handoff test that decodes actual STCH/FACCH output and proves the requester receives the positive grant before lower-priority busy/not-granted traffic.
3. Add restart/EG7 recovery regression for thousands of cached affiliates: after BS restart, MM must restore attach-visible state and group affiliation coherently before group PTT.
4. Add SDS/WAP accepted-vs-transmitted observability tests so dashboard/logs distinguish queued, transmitted, failed, and terminal-delivered states where the stack can know them.
5. Keep private-call release fixes in the validation queue: called party must receive correct end-call/release semantics, not `No answer`, and Motorola MXP600 must not be driven into soft reboot by Nexus-BS release signalling.

## 2026-06-05 - Wrapped floor-grant priority closed through UMAC/MAC and CMCE cross-layer tests

Component in simple technical terms:

- CMCE decides who owns the call floor/PTT.
- MLE marks the payload as CMCE.
- LLC wraps that payload as `BL-UDATA`.
- UMAC receives `TMA-UNITDATA.req`, admits it into bounded queues, and builds MAC signalling.
- MAC scheduler chooses which STCH/FACCH block is sent first on the assigned traffic channel.

Problem fixed:

- The previous UMAC/MAC priority recognition handled direct synthetic CMCE payloads, but real stack traffic reaches UMAC wrapped as LLC `BL-UDATA` plus a 3-bit MLE `Cmce` discriminator.
- Under a large GSSI storm, that meant the wrapped positive `D-TX GRANTED(Granted)` with uplink allocation could look ordinary and sit behind thousands of lower-value `RequestQueued`/`NotGranted` floor responses.
- The fix now decodes the wrapped shape before priority classification, while keeping direct CMCE support for existing tests and narrow fixtures.
- In the scheduler, the CMCE parser now reads the TM-SDU after `MAC-RESOURCE` using the current bit position, not the beginning of the STCH block.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: group-call floor request and floor grant/queued/not-granted signalling.
- EN 300 392-2 clause 20.4.1.1.3: TMA request/report handling.
- EN 300 392-2 clause 21.4.3.1: random access acknowledgement carried into the grant path.
- EN 300 392-2 clause 22.3.2.4.1: LLC `BL-UDATA` unacknowledged transfer.
- EN 300 392-2 clauses 23.5 and 23.5.2.2.1: STCH/FACCH assigned-channel signalling and MAC-RESOURCE delivery.
- This is clause-scoped regression evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added `BL-UDATA`/MLE(CMCE) decoding before TMA admission priority classification.
  - Positive wrapped `D-TX GRANTED` with UL/Both allocation now receives the same bounded-queue priority as direct CMCE.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added wrapped CMCE detection for STCH/FACCH stealing priority.
  - Fixed parser position by extracting payload after `MAC-RESOURCE` with `BitBuffer::from_bitbuffer_pos`.
  - Added wrapped scheduler regression where a positive grant preempts a full backlog of wrapped busy responses.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added wrapped UMAC 4096-message storm regression with preserved requester random-access ACK.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added CMCE->MLE->LLC->UMAC->LMAC cross-layer 4096-member regression that decodes `MAC-RESOURCE -> BL-UDATA -> MLE(CMCE) -> D-TX GRANTED`.
  - Confirms the requester positive grant reaches STCH before lower-value storm `NotGranted` responses.

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/tests/test_cmce_bs.rs crates/tetra-entities/tests/test_umac_bs.rs crates/tetra-entities/src/umac/umac_bs.rs crates/tetra-entities/src/umac/subcomp/bs_sched.rs` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 61 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 69 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 146 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit the wrapped floor-grant priority patch.
2. Continue restart/EG7 recovery regression for thousands of cached affiliates: after BS restart, MM and CMCE must restore attach-visible state and group affiliation coherently before group PTT.
3. Add SDS/WAP accepted-vs-transmitted observability tests so dashboard/logs distinguish queued, transmitted, failed, and terminal-delivered states where the stack can know them.
4. Keep private-call release validation queued: called party should receive correct end-call/release semantics, not `No answer`, and MXP600 must not be driven into soft reboot by release signalling.

## 2026-06-05 - Listener floor notification priority and LLC pdu_prio storm hardening

Component in simple technical terms:

- CMCE creates floor-control messages such as `D-TX GRANTED`, `D-TX CEASED`, and `D-TX INTERRUPT`.
- MLE adds the CMCE discriminator.
- LLC stores and orders `BL-UDATA` before UMAC sees it.
- UMAC admits the resulting `TMA-UNITDATA.req`.
- MAC scheduler decides the STCH/FACCH order on the assigned channel.

Problem fixed:

- Group listener notifications `D-TX GRANTED(GrantedToOtherUser)` were protected at UMAC admission only as generic channel allocation, and scheduler priority treated non-`Granted` `D-TX GRANTED` as ordinary.
- In a large GSSI PTT storm, that could let the requester receive the floor while the group listener notification sat behind many low-value `RequestQueued`/`NotGranted` responses.
- Cross-layer testing then exposed a higher bottleneck: even with UMAC/MAC protection, CMCE emitted floor-control with `pdu_prio = 0`, so LLC could keep `GrantedToOtherUser` behind a large BL-UDATA backlog.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: group-call floor grant, queued/not-granted, and listener notification behaviour.
- EN 300 392-2 clause 20.4.1.1.3 and table 20.54: TMA priority/report handling; priority 7 is the highest TMA priority.
- EN 300 392-2 clause 22.3.2.4.1: LLC `BL-UDATA` store/submit path.
- EN 300 392-2 clauses 23.5 and 23.5.2.2.1: STCH/FACCH assigned-channel signalling.
- This is clause-scoped regression evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added `cmce_downlink_pdu_prio`.
  - Raises only time-critical floor-control downlink PDUs to priority 7:
    `D-TX GRANTED(Granted)`, `D-TX GRANTED(GrantedToOtherUser)`, `D-TX CEASED`, and `D-TX INTERRUPT`.
  - Leaves `D-CALL PROCEEDING`, `D-ALERT`, SDS/status, setup/release, and other non-floor CMCE at normal priority.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added `ListenerFloorGrant` TMA admission priority, between generic channel allocation and positive requester grant.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added `ListenerFloorGrant` scheduler priority, below positive requester grant and above ordinary busy responses.
  - Added 4096-message wrapped STCH test proving requester grant first, GSSI listener notification second, and busy responses later.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added UMAC 4096-message wrapped admission test proving GSSI `GrantedToOtherUser` is not dropped before scheduler.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Strengthened the CMCE->MLE->LLC->UMAC->LMAC cross-layer 4096-member test to require wrapped GSSI `GrantedToOtherUser` at STCH before lower-value storm `NotGranted` responses.

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/src/cmce/subentities/cc_bs/mod.rs crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs crates/tetra-entities/src/umac/umac_bs.rs crates/tetra-entities/src/umac/subcomp/bs_sched.rs crates/tetra-entities/tests/test_umac_bs.rs crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 70 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 62 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 146 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit listener floor notification priority and CMCE floor-control `pdu_prio` hardening.
2. Implement bounded FIFO group floor fairness: replace the single waiter with a de-duplicated bounded queue, cleanup on deaffiliate/release, and large repeated-PTT tests.
3. Continue restart/EG7 recovery regression for thousands of cached affiliates before group PTT.
4. Continue SDS/WAP accepted-vs-transmitted observability and private-call release validation.

## 2026-06-05 - Bounded FIFO group floor fairness for thousands of terminals

Component in simple technical terms:

- CMCE group floor control decides which ISSI may transmit on a GSSI group call.
- A `D-TX GRANTED(RequestQueued)` tells a waiting terminal that its PTT request is accepted into the SwMI queue, but the current speaker still owns the floor.
- A `D-TX GRANTED(Granted)` with uplink allocation gives the next terminal permission to talk.
- A GSSI `D-TX GRANTED(GrantedToOtherUser)` tells group listeners that another terminal now owns the floor.

Problem fixed:

- Group calls previously retained only one queued floor requester. That was enough for 2-3 radios, but not robust for thousands of affiliated terminals contending on the same GSSI.
- The active group call now keeps a bounded FIFO of queued floor requesters, capped at 4096 waiters.
- The FIFO is de-duplicated by ISSI with a `HashSet`, so repeated PTT/U-SETUP aliases from the same terminal do not scan or grow the whole queue.
- On `U-TX CEASED` or UL inactivity, CMCE grants the floor to the first still-affiliated queued requester and drops stale deaffiliated requesters ahead of it.
- A queued requester may now withdraw its own pending request with `U-TX CEASED` before floor grant; no CC protocol response is emitted and the withdrawn ISSI is not granted later.
- The 4097th queued contender receives explicit `NotGranted` without disturbing the 4096 accepted FIFO waiters.
- Private simplex/duplex calls keep their existing one-waiter P2P logic; this patch is group-call scoped.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: group-call floor request response can be grant, queued, or not granted. The bounded FIFO is Nexus-BS SwMI policy over that allowed signalling.
- EN 300 392-2 clause 14.5.2.2.1 a): a user application may withdraw a queued request-to-transmit by sending `U-TX CEASED`; no CC protocol response is expected from the SwMI.
- EN 300 392-2 clause 14.5.2.1: repeated same-GSSI `U-SETUP` while a group call is already maintained is handled as floor-control alias, not as a duplicate setup fanout.
- EN 300 392-2 clause 20.4.1.1.3 and clause 23.5: floor-control signalling must survive the CMCE/MLE/LLC/UMAC assigned-channel path.
- This is clause-scoped regression evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/call.rs`
  - Replaced the group `queued_tx_demand: Option<TetraAddress>` with `VecDeque<TetraAddress>` plus `HashSet<u32>`.
  - Added bounded FIFO queue, O(1) duplicate detection, FIFO pop-through for stale requester cleanup, and clear-on-departure/release helpers.
  - Added unit coverage for deduplication, 4096-waiter cap, FIFO pop, pop-through stale-prefix cleanup, re-queue after removal, and explicit clear.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added first-affiliated queued requester selection so deaffiliated stale waiters are skipped before handoff.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - `U-TX CEASED` now hands the floor to the first still-affiliated FIFO waiter, not a single overwritten waiter.
  - `U-TX CEASED` from a queued non-speaker now withdraws that requester from the FIFO without emitting contradictory floor signalling.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - UL inactivity fallback uses the same affiliated FIFO handoff path.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated repeated same-GSSI `U-SETUP` alias tests to expect FIFO `RequestQueued` behaviour without D-SETUP fanout.
  - Added/strengthened 4096-member group tests proving first and second FIFO handoffs.
  - Added CMCE regressions for queued-request withdrawal, deaffiliated head-waiter skip to the next FIFO requester, and overflow `NotGranted` after 4096 waiters.
  - Updated cross-layer storm validation so lower-value storm responses may be `RequestQueued` or `NotGranted`, while requester and listener grants still win priority.

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/src/cmce/subentities/cc_bs/call.rs crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `cargo test -p tetra-entities --lib group_floor_waiter --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_repeated_u_setup_floor_alias_is_bounded_without_setup_fanout --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_cross_layer_large_group_floor_grant_survives_wrapped_ptt_storm_to_lmac --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_queued_requester_u_tx_ceased_withdraws_before_handoff --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_tx_ceased_skips_deaffiliated_front_waiter_and_grants_next_fifo_waiter --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_floor_fifo_overflow_returns_not_granted_after_4096_waiters --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 149 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 62 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Commit bounded FIFO group floor fairness.
2. Continue restart/EG7 recovery regression for thousands of cached affiliates: after BS restart, MM and CMCE must restore attach-visible state and GSSI affiliation before group PTT.
3. Continue live-lab validation for Motorola MTP3550 static-audio-on-repeated-PTT, using logs from last restart and without changing ETSI baseline.
4. Continue SDS/WAP accepted-vs-transmitted observability and private-call release validation.

## 2026-06-05 - Large-group preemption FIFO stale-entry hardening

Component in simple technical terms:

- CMCE is the call-control layer that decides group PTT/floor ownership.
- The FIFO floor queue is the SwMI-side waiting list of ISSIs that pressed PTT while another MS owns the group floor.
- Pre-emption is optional/default-off Nexus-BS behavior where an explicitly configured high-priority `U-TX DEMAND` can interrupt the current speaker.

Problem fixed:

- A group floor grant changed the active speaker but did not defensively remove that ISSI from the waiting FIFO.
- In normal FIFO handoff this was usually hidden because handoff already popped the selected waiter first.
- Under optional pre-emption, or any future direct grant path, a requester could already be queued and then become the new speaker while still present in the FIFO.
- When that speaker later sent `U-TX CEASED`, CMCE could grant the floor back to the same ISSI instead of the next queued group member.
- This is especially risky in thousands-terminal GSSI groups, where stale FIFO entries are hard to see manually and can look like first-PTT loss, false turn-taking, or static/no-voice symptoms.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: SwMI controls group transmission permission and may respond to `U-TX DEMAND` with `Granted`, `RequestQueued`, or `NotGranted`.
- EN 300 392-2 clause 14.5.2.2.1 f) and table 14.85: pre-emptive/emergency TX demand may withdraw current transmit permission when SwMI interruption support is explicitly enabled.
- This patch preserves the same over-the-air PDUs and only hardens Nexus-BS internal queue ownership; it is clause-scoped engineering evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/call.rs`
  - `ActiveCall::grant_floor` now removes the granted speaker from the group floor FIFO before setting it as active speaker.
  - Added a unit test proving a granted speaker is removed from the FIFO, current-speaker reassertion does not duplicate the waiter, and the ISSI can request again after hangtime.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added a 4096-member GSSI regression where all in-cap contenders queue, an explicitly enabled pre-emptive requester takes the floor, and its later `U-TX CEASED` grants the next FIFO requester rather than itself.
  - The test also verifies one GSSI listener notification, no false `D-TX CEASED`, no floor release, and one UMAC floor grant.

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/src/cmce/subentities/cc_bs/call.rs crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `cargo test -p tetra-entities --lib group_floor_grant_removes_new_speaker_from_waiter_fifo --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_preemptive_grant_removes_requester_from_fifo_before_next_handoff --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 150 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 62 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add full FIFO drain regression for a 4096-member GSSI: every queued waiter gets exactly one ordered floor handoff, no `NotGranted`, no duplicated self-grant, no per-member listener fanout.
2. Add overflow retry regression: 4097th waiter receives `NotGranted`, then after one FIFO slot frees the same ISSI can retry and enters the tail without corrupting order.
3. Extend restart/EG7 large-group recovery to observable dashboard/telemetry state so "attached but No Group" is covered beyond internal registry state.
4. Add stress-aftercare regression: large group storm then simple private simplex and duplex still setup, grant floor, and release cleanly.

## 2026-06-05 - Bounded FIFO overflow retry order

Component in simple technical terms:

- CMCE owns the group PTT waiting list.
- A full FIFO means Nexus-BS has accepted the maximum local number of waiting speakers for one group call.
- `NotGranted` for an over-cap requester is a temporary local admission response, not a permanent ban.

Problem fixed:

- The previous overflow test proved the 4097th waiter received `D-TX GRANTED(NotGranted)` and that the first queued waiter still got the next floor.
- It did not prove what happens when the denied terminal retries after capacity is available again.
- The test now verifies that after one queued waiter is granted, the previously denied ISSI can retry and receives `RequestQueued`.
- It also verifies the retry enters the FIFO tail and does not jump ahead of already accepted waiters.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1 allows the SwMI response to a group `U-TX DEMAND` to be `Granted`, `RequestQueued`, or `NotGranted`.
- The 4096 waiter cap and tail retry policy are Nexus-BS local SwMI admission policy over the standard floor-control states.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Extended `test_large_group_floor_fifo_overflow_returns_not_granted_after_4096_waiters`.
  - After FIFO overflow denial, the current speaker ceases, the first accepted waiter gets `Granted`, and the overflow ISSI retries.
  - The retry receives `RequestQueued` with DL-only allocation, then the next original FIFO waiter receives `Granted` before the retry.

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_floor_fifo_overflow_returns_not_granted_after_4096_waiters --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 150 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Add full FIFO drain regression for a 4096-member GSSI.
2. Harden UMAC protected queue caps so protected-only floor-control storms cannot grow past scheduler limits.
3. Extend restart/EG7 large-group recovery to observable dashboard/telemetry state.
4. Add stress-aftercare regression: large group storm then simple private simplex and duplex still work.

## 2026-06-05 - UMAC protected floor-control queue cap

Component in simple technical terms:

- UMAC/MAC is the radio scheduler. It takes CMCE floor-control messages and decides which STCH/FACCH block is sent over the air first.
- `D-TX CEASED` and `D-TX INTERRUPT` are floor-withdraw messages: they tell terminals that transmit permission is no longer held by the old speaker.
- Random-access ACKs are separate MAC acknowledgements for uplink access and keep their own local cap.

Problem fixed:

- The scheduler already capped ordinary downlink queues, but if every queued item was classified as protected it logged a warning and allowed the queue to remain above the cap.
- In a pathological large-group floor-control storm, protected-only STCH/FACCH entries could therefore grow past `MAX_DLSCHED_ELEMS_PER_TIMESLOT`.
- UMAC now assigns each queued element a local backpressure priority and still enforces a hard cap when all items are protected.
- Ordinary items are still discarded first; if no ordinary item exists, the oldest lowest-priority protected item is discarded and reported.
- Repeated same-call/same-address floor-withdraw STCH entries are coalesced so only the newest duplicate remains queued.
- `RandomAccessAck` keeps its existing 8192-entry RA-specific cap instead of being forced into the 4096 STCH/FACCH cap, preserving the large random-access churn behavior already tested.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: floor-control permission/withdrawal semantics for group calls.
- EN 300 392-2 clause 21.4.3.1: MAC random access acknowledgement behavior.
- EN 300 392-2 clauses 23.5 and 23.5.2.2.1: assigned-channel STCH/FACCH signalling and slot grants.
- This patch is local scheduler backpressure policy over standard PDUs; it is clause-scoped engineering evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added `DlBackpressurePriority` for local queue shedding order.
  - Added `FloorWithdrawKey` and duplicate coalescing for `D-TX CEASED`/`D-TX INTERRUPT` STCH/FACCH entries with the same address, call id, and PDU type.
  - Changed protected-only cap enforcement from warning-only to bounded discard of the oldest lowest-priority protected element.
  - Added a configurable internal push helper so `RandomAccessAck` can retain the existing 8192 RA cap while normal scheduler queues remain capped at 4096.
  - Added tests for duplicate floor-withdraw coalescing, protected-only floor-withdraw backlog bounding, and preservation of the existing RA ACK 8192 cap.

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/src/umac/subcomp/bs_sched.rs` -> pass.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched::tests::test_floor_withdraw_duplicate_coalesces_and_keeps_latest_reporter --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched::tests::test_protected_floor_withdraw_backlog_stays_bounded_and_retains_newest --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched::tests::test_pending_random_access_ack_queue_is_bounded_for_large_group_churn --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib umac::subcomp::bs_sched --locked` -> 72 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 62 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add full FIFO drain regression for a 4096-member GSSI.
2. Add restart recovery large-cache pacing test for 4096 cached ISSIs.
3. Extend restart/EG7 large-group recovery to observable dashboard/telemetry state.
4. Add stress-aftercare regression: large group storm then simple private simplex and duplex still work.

## 2026-06-05 - Full 4096-waiter group FIFO drain evidence

Component in simple technical terms:

- CMCE group floor FIFO is the ordered waiting list for group PTT requests while another terminal is speaking.
- A full FIFO drain means each queued ISSI receives the floor exactly in accepted order as the previous speaker sends `U-TX CEASED`.
- The listener notification remains one GSSI-addressed `D-TX GRANTED(GrantedToOtherUser)`, not one message per terminal.

Problem fixed:

- Previous large-group tests proved 4096 waiters could be queued and that the first/second handoff stayed FIFO.
- They did not prove that all accepted waiters could drain in order until the queue was empty.
- The 4096-member FIFO test now drains every accepted waiter to completion and verifies each handoff.
- After the last queued waiter ceases, CMCE enters hangtime with one `D-TX CEASED` and one UMAC floor release instead of emitting another stale grant.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: SwMI group floor-control responses and queued/granted turn taking.
- EN 300 392-2 clause 23.5: assigned-channel signalling carries the floor-control messages.
- The 4096 FIFO length is Nexus-BS local SwMI policy and test evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Extended `test_large_group_floor_queue_is_bounded_fifo_for_thousands_of_waiters`.
  - After first and second handoff checks, the test now drains all remaining queued ISSIs from `first_issi + 3` through the final member.
  - Each drain step requires exactly one individual `Granted` response, exactly one GSSI `GrantedToOtherUser` listener response, no `NotGranted`, no release/close, and exactly one UMAC floor grant.
  - Final no-waiter cease requires `D-TX CEASED` and UMAC floor release, proving stale FIFO state is empty.

Verification:

- `rustfmt --edition 2024 crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_floor_queue_is_bounded_fifo_for_thousands_of_waiters --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 150 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add restart recovery large-cache pacing test for 4096 cached ISSIs.
2. Extend restart/EG7 large-group recovery to observable dashboard/telemetry state.
3. Add stress-aftercare regression: large group storm then simple private simplex and duplex still work.

## 2026-06-05 21:04 EEST - MM restart recovery large-cache pacing and fairness

Component in simple technical terms:

- MM is the BS registration and affiliation controller: it remembers which ISSI is attached, which GSSI groups each terminal belongs to, and when a terminal is using energy economy.
- Restart recovery is the BS-side procedure used after process restart to ask still-camped terminals to refresh registration and group state with `D-LOCATION UPDATE COMMAND`.
- For thousands of terminals, this must be paced like a work queue: one terminal per configured interval, no first-frame burst, no retry loop that keeps hitting the first radios while later radios remain unprobed.

Problem fixed:

- `tick_restart_recovery()` previously stored probes in a `HashMap` and scanned every cached ISSI every TDMA tick.
- The old one-command-per-tick guard prevented bursts, but retry due-times could collide with later first probes. If radios did not answer, an early ISSI retry could be selected before every cached ISSI had received one first command.
- MM now keeps restart probes in two structures:
  - `HashMap<ISSI, RestartRecoveryProbe>` for direct removal when a terminal successfully registers or is forgotten.
  - `BTreeSet<(due_tick, ISSI)>` for deterministic ordered scheduling.
- A local monotonic recovery clock is used instead of raw `TdmaTime` ordering, so scheduling remains stable across long-running BS time progression.
- Retry timing is sweep-based: every cached ISSI receives its first probe before any retry from the first sweep is eligible.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: SwMI-commanded registration via `D-LOCATION UPDATE COMMAND`, including group identity report request.
- EN 300 392-2 clauses 16.8.0, 16.9.3.4, 16.10.23, and 16.10.35a: group identity state and coherent `DemandLocationUpdating` accept behavior after a BS-commanded update.
- The startup guard, inter-ISSI spacing, and sweep fairness are Nexus-BS local RF robustness policy over standard MM PDUs. This is clause-scoped engineering evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Added deterministic due-index scheduling for restart recovery probes.
  - Added monotonic local restart recovery clock.
  - Removed probes from both direct and due-index stores when registration succeeds or the ISSI is forgotten.
  - Changed retry scheduling so retries occur in complete sweeps, preserving first-pass fairness for thousands of cached terminals.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added `test_restart_recovery_large_cache_paces_one_command_per_interval_and_restores_groups`.
  - Added `test_restart_recovery_large_cache_first_sweep_reaches_every_issi_before_retry`.

Verification:

- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_large_cache_paces_one_command_per_interval_and_restores_groups --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_large_cache_first_sweep_reaches_every_issi_before_retry --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 29 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `rustfmt --edition 2024 --check crates/tetra-entities/src/mm/mm_bs.rs crates/tetra-entities/tests/test_mm_bs.rs` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add CMCE duplicate queued `U-TX DEMAND` idempotency regression for 4096-member GSSI: repeated PTT from the same waiting ISSI must produce one queued grant and then hand off to the next unique waiter.
2. Add UMAC mixed EG7/StayAlive group floor-control stress test: requester positive grant and listener GSSI grant must survive queue pressure without unbounded growth.
3. Extend restart/EG7 large-group recovery to dashboard/telemetry observable state.
4. Add stress-aftercare regression: large group storm, then simple private simplex and duplex still work.

## 2026-06-05 21:08 EEST - CMCE large-group duplicate PTT idempotency evidence

Component in simple technical terms:

- CMCE is the call-control and floor-control layer. For group calls it decides which terminal currently has PTT/floor, which terminals are waiting, and what `D-TX GRANTED` response is sent.
- A duplicate `U-TX DEMAND` is the same waiting terminal pressing/retrying PTT while another terminal still has the floor.
- In a large GSSI, duplicate PTT pressure must not add duplicate FIFO entries; otherwise the same terminal can be granted again and delay the next unique speaker.

Problem covered:

- The implementation already deduplicates queued floor waiters by ISSI in `ActiveCall`.
- The missing evidence was a component-level 4096-member GSSI test proving that repeated same-ISSI `U-TX DEMAND` stays idempotent across real CMCE messages and UMAC floor events.
- New test queues eight duplicate PTT demands from one requester, then one PTT demand from the next requester.
- When the current speaker ceases, the duplicate requester receives exactly one granted handoff.
- When that requester ceases, the next unique requester receives the next handoff; the duplicate requester is not still in FIFO.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: group call floor-control responses, including queued request-to-transmit and granted transmission.
- EN 300 392-2 clause 23.5: assigned-channel FACCH/STCH transport of floor-control signalling.
- The 4096-member stress shape and duplicate FIFO idempotency are Nexus-BS robustness evidence over standard CMCE PDUs, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_large_group_duplicate_queued_u_tx_demand_is_idempotent_before_handoff`.
  - The test registers 4096 group members, starts a group call, applies duplicate same-ISSI PTT pressure, and verifies FIFO handoff order remains unique.
  - It also checks no `D-RELEASE`, no UMAC close/end event, and exactly one UMAC floor grant per real handoff.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs test_large_group_duplicate_queued_u_tx_demand_is_idempotent_before_handoff --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 151 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `rustfmt --edition 2024 --check crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add UMAC mixed EG7/StayAlive group floor-control stress test: requester positive grant and listener GSSI grant must survive queue pressure without unbounded growth.
2. Extend restart/EG7 large-group recovery to dashboard/telemetry observable state.
3. Add stress-aftercare regression: large group storm, then simple private simplex and duplex still work.

## 2026-06-05 21:12 EEST - UMAC mixed EG7/StayAlive large-group floor-control stress

Component in simple technical terms:

- UMAC/MAC is the radio scheduler. It turns CMCE/MM decisions into MAC-RESOURCE, STCH/FACCH, random-access ACKs, and channel allocations on the real downlink timeslot.
- Energy Economy EG7 means some terminals sleep most of the time and only listen in scheduled windows unless an assigned-channel group call suspends that sleep cycle.
- Group floor-control needs two critical downlinks during handoff: the requester ISSI must receive a positive `D-TX GRANTED` with uplink allocation, and the GSSI listeners must receive `GrantedToOtherUser`.

Problem covered:

- Existing tests covered large PTT storm requester grants, listener grants, and EG7 group suspension separately.
- The missing combined evidence was a 4096-member GSSI with mixed StayAlive/EG7 members under thousands of lower-value busy floor responses.
- The new test opens a group call, verifies the EG7 requester is suspended awake while a StayAlive member has no EG scheduler entry, preserves the requester random-access ACK, then queues 4096 lower-value busy responses plus both critical handoff grants.
- The requester positive grant and GSSI listener grant both transmit before lower-value busy responses.
- The requester grant carries the preserved random-access ACK; the GSSI listener grant does not incorrectly ACK one requester's random access for the whole group.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: group floor-control `D-TX GRANTED` semantics for requester and listeners.
- EN 300 392-2 clause 21.4.3.1: random-access acknowledgement must apply to the requesting MS.
- EN 300 392-2 clause 23.5: assigned-channel STCH/FACCH delivery of floor-control signalling.
- EN 300 392-2 clause 23.7.6: Energy Economy receive/suspend behaviour for assigned-channel activity.
- This is scheduler-level robustness evidence over standard PDUs, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_large_group_ptt_storm_mixed_eg7_stayalive_keeps_requester_and_listener_floor_grants`.
  - The test registers 4096 GSSI members, half with EG7 assignments and half StayAlive/no EG state.
  - It verifies both critical floor-control grants survive a 4096-item busy-response backlog and retain correct random-access ACK semantics.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs test_large_group_ptt_storm_mixed_eg7_stayalive_keeps_requester_and_listener_floor_grants --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 63 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `rustfmt --edition 2024 --check crates/tetra-entities/tests/test_umac_bs.rs` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Extend restart/EG7 large-group recovery to dashboard/telemetry observable state.
2. Add stress-aftercare regression: large group storm, then simple private simplex and duplex still work.
3. Add long-run no-terminal-loss model test: repeated attach/affiliate/EG7/PTT cycles must leave subscriber/group registries coherent.

## 2026-06-05 21:21 EEST - Dashboard/MM restart observability for cached GSSI + EG7

Component in simple technical terms:

- MM is mobility management: registration, attach, group affiliation, restart recovery, and Energy Economy negotiation.
- Dashboard/telemetry is operator visibility. It is not the TETRA air interface, but it must faithfully show the MM state so a recovered terminal does not appear as "No Group" or missing EG mode.

Problem covered:

- `MsGroupAttach` and `MsGroupsSnapshot` already created a dashboard MS entry if group telemetry arrived before registration.
- `MsEnergySaving` only updated an existing entry. During restart recovery at scale, an EG7 response can race registration/group events, so the dashboard could drop the EG state even though MM negotiated it correctly.
- The patch makes `MsEnergySaving` create the dashboard entry and update `last_seen`, matching the order-tolerant behaviour of group telemetry.
- A new MM integration test seeds restart recovery with cached `ISSI -> GSSI`, lets the MS return without a group list, accepts the SwMI group refresh ACK, then activates EG7 and feeds all telemetry through the dashboard.
- The resulting dashboard state must show both the recovered GSSI and the EG7 receive opportunity.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4: restart/registration recovery context.
- EN 300 392-2 clauses 16.8.0, 16.9.3.4, 16.10.23, and 16.10.35a: group identity state, group attach/detach signalling, and location update accept semantics.
- EN 300 392-2 clauses 16.7.1, 16.10.9, 16.10.10, 23.7.6, and timer T.210: Energy Economy negotiation and receive opportunity state.
- Dashboard state is operational evidence over the clause-scoped MM/EG behaviour, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/net_dashboard/server.rs`
  - `TelemetryEvent::MsEnergySaving` now uses `entry(...).or_insert_with(empty_ms_entry)` and updates `last_seen`.
  - Added `dashboard_preserves_energy_saving_if_energy_event_precedes_registration`.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added `dashboard_ms_after` helper for complete dashboard MS state assertions.
  - Added `test_restart_recovery_eg7_cached_group_is_visible_in_dashboard_after_refresh`.
  - Added a telemetry MM helper that sets the subscriber recovery cache path before constructing `MmBs`, so restart cache loading is exercised correctly.

Verification:

- `cargo test -p tetra-entities dashboard_preserves_energy_saving_if_energy_event_precedes_registration --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_eg7_cached_group_is_visible_in_dashboard_after_refresh --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 30 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `rustfmt --edition 2024 --check crates/tetra-entities/src/net_dashboard/server.rs crates/tetra-entities/tests/test_mm_bs.rs` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add stress-aftercare regression: large group storm, then simple private simplex and duplex still work.
2. Add long-run no-terminal-loss model test: repeated attach/affiliate/EG7/PTT cycles must leave subscriber/group registries coherent.
3. Continue CMCE/UMAC audit for any fixed-size or O(n)-while-locked behaviour that can break GSSI calls with thousands of terminals.

## 2026-06-05 21:31 EEST - MM group-state durability and browser EG ordering

Component in simple technical terms:

- MM owns the durable subscriber/group recovery state used after a BS restart.
- SwMI group ACK is the confirmation path for BS-initiated group attach/detach changes.
- Dashboard browser state is the client-side view; it must handle telemetry in any order, because thousands of returning terminals can interleave registration, group, and EG events.

Problem covered:

- The server now preserved `MsEnergySaving` before registration, but the browser reducer still ignored `ms_energy_saving` unless the MS row already existed.
- SwMI-requested detach changed MM/CMCE state but did not publish a final dashboard group snapshot, so the UI could retain stale groups.
- Restart recovery cache writes were still coalesced for normal registration updates. That is correct for scale, but group affiliation changes need stronger durability: if a BS restarts immediately after a group attach/detach, the cache file must already reflect the final GSSI set.

ETSI clause scope:

- EN 300 392-2 clauses 16.4.3 and 16.4.4: SwMI/MM registration and group-report recovery procedures.
- EN 300 392-2 clauses 16.8.0, 16.8.1, 16.8.2, 16.9.3.4, 16.10.17, and Annex G: group identity attach/detach semantics, including SwMI-requested detach and mode=1 replace/detach-all.
- EN 300 392-2 clauses 16.7.1, 16.10.9, 16.10.10, 23.7.6, and T.210: Energy Economy state represented in dashboard telemetry.
- Cache durability and dashboard rendering are Nexus-BS operational hardening around clause-scoped behaviour, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/net_dashboard/html.rs`
  - Browser `ms_energy_saving` now calls `ensureMsEntry(msg.issi)` and updates `_last_seen_ts`.
- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Added browser reducer test for EG-before-registration ordering.
- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Added `remember_restart_recovery_issi_with_remaining_persist` and `remember_restart_recovery_issi_after_group_change`.
  - Group affiliation/deaffiliation paths now force a restart-cache flush after real group-state mutation while preserving coalescing for simple registration updates.
  - SwMI ACK detach/replace now emits the final `MsGroupsSnapshot`.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added SwMI detach dashboard snapshot tests.
  - Added immediate cache-flush tests for standalone group affiliation and SwMI group detach.

Verification:

- `cargo test -p tetra-entities dashboard_browser_creates_ms_entry_for_energy_saving_before_registration --locked` -> 1 passed.
- `cargo test -p tetra-entities dashboard_preserves_energy_saving_if_energy_event_precedes_registration --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs test_swmi_group_ack_detach_publishes_dashboard_group_snapshot --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs test_swmi_group_ack_detach_all_then_attach_publishes_final_dashboard_group_snapshot --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs test_group_affiliation_update_forces_restart_recovery_cache_flush --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs test_swmi_group_detach_forces_restart_recovery_cache_flush --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_cache_coalesces_multiple_updates_until_flush --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs swmi_group_ack --locked` -> 14 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 32 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `rustfmt --edition 2024 --check crates/tetra-entities/src/mm/mm_bs.rs crates/tetra-entities/src/net_dashboard/server.rs crates/tetra-entities/tests/test_mm_bs.rs` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add stress-aftercare regression: large group storm, then simple private simplex and duplex still work.
2. Add deterministic over-cap tests/telemetry for CMCE/UMAC fixed queue ceilings so thousands-scale contention fails explicitly, not silently.
3. Add long-run no-terminal-loss model test: repeated attach/affiliate/EG7/PTT cycles must leave subscriber/group registries coherent.

## 2026-06-05 21:36 EEST - CMCE 10k-member group floor overflow evidence

Component in simple technical terms:

- CMCE is call control: it receives group/private call setup and PTT/floor requests, then decides whether a terminal is granted, queued, or denied.
- The group floor FIFO is deliberately bounded. Above the local bound, ETSI-compatible behaviour is an explicit floor-control denial, not silent loss or call teardown.

Problem covered:

- Existing evidence covered 4096 waiters and 4098-member overflow.
- The missing evidence for "mii de terminale pe grup" was a much larger affiliated GSSI, with more contenders than the local FIFO can hold.
- New test registers 10,000 members on one GSSI, starts a group call, submits 9,999 simultaneous `U-TX DEMAND` contenders, and verifies:
  - exactly 4096 contenders receive `RequestQueued`;
  - every over-cap contender receives explicit `NotGranted`;
  - the active group call is not released or closed by the overflow;
  - when the current speaker ceases, the FIFO head receives the next `Granted` floor and listeners receive the GSSI notification.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: group call floor-control grant/queued/not-granted responses.
- EN 300 392-2 clause 23.5: assigned-channel FACCH/STCH floor-control delivery.
- This is engineering evidence that local bounded overload is explicit and deterministic; it is not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_ten_thousand_member_group_floor_overflow_is_explicit_and_preserves_handoff`.
  - The fixture sets `call_timeout_secs = 0` to isolate overflow handling from normal call timeout expiry.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs test_ten_thousand_member_group_floor_overflow_is_explicit_and_preserves_handoff --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs large_group_floor --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 152 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `rustfmt --edition 2024 --check crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add UMAC over-cap protected-control evidence: all-protected backlog must preserve requester/listener/floor-withdraw semantics or fail with explicit telemetry.
2. Add stress-aftercare regression: large group storm, then simple private simplex and duplex still work.
3. Add long-run no-terminal-loss model test: repeated attach/affiliate/EG7/PTT cycles must leave subscriber/group registries coherent.

## 2026-06-05 21:40 EEST - UMAC protected floor-control admission over backlog

Component in simple technical terms:

- UMAC is the radio scheduler and MAC admission layer. CMCE decides floor-control, but UMAC must still admit and report those TM-SDUs under queue pressure.
- TMA report tracking is bounded locally so thousands of pending downlinks cannot grow memory without limit.
- Not all protected floor-control messages have equal urgency: listener floor notifications are important, but requester positive grants and floor withdrawal/ceased signalling are more urgent.

Problem covered:

- Existing UMAC tests covered lower-priority busy/denial backlogs and 4096-member mixed EG7/StayAlive storms.
- The audit gap was a protected backlog: the queue is full of floor-control listener grants, then more urgent floor-control arrives.
- New test fills the pending TMA report cap with GSSI `GrantedToOtherUser` listener grants, then submits:
  - an ISSI positive `D-TX GRANTED` with uplink allocation;
  - a GSSI `D-TX CEASED` floor-withdrawal PDU.
- UMAC must keep the pending report count bounded, admit both more urgent floor-control requests, and emit explicit `FragmentationFailure` TMA reports for the evicted lower-priority listener grants.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: group floor-control grant/ceased semantics.
- EN 300 392-2 clause 20.4.1.1.3: TMA-REPORT indication for MAC transfer outcome.
- EN 300 392-2 clause 23.5: assigned-channel FACCH/STCH delivery of floor-control signalling.
- This is bounded admission/robustness evidence around standard PDUs, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `DTxCeased` import and `d_tx_ceased_sdu` helper.
  - Added `test_tma_report_cap_admits_higher_priority_floor_control_over_protected_backlog`.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs test_tma_report_cap_admits_higher_priority_floor_control_over_protected_backlog --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 64 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `rustfmt --edition 2024 --check crates/tetra-entities/tests/test_umac_bs.rs` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add stress-aftercare regression: large group storm, then simple private simplex and duplex still work.
2. Add long-run no-terminal-loss model test: repeated attach/affiliate/EG7/PTT cycles must leave subscriber/group registries coherent.
3. Continue dashboard/telemetry scaling work: bounded client queues and coalesced station rendering for 10k MS snapshots.

## 2026-06-05 21:43 EEST - CMCE large-group storm aftercare for simple private call

Component in simple technical terms:

- CMCE owns both group floor control and private call control.
- A large GSSI PTT storm must not leave CMCE in a state that breaks later simple private call setup.

Problem covered:

- The 10k GSSI overflow test proved explicit `RequestQueued`/`NotGranted` behaviour and preserved group handoff.
- The remaining aftercare question was whether the same runtime can still start a simple private call after that storm.
- The test now registers a private caller/callee after the 10k group overflow and handoff, then starts a normal simplex private call.
- It verifies that CMCE allocates a distinct call identifier, opens one shared traffic bearer, sends `D-CONNECT`, and sends `D-CONNECT-ACKNOWLEDGE`.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1: group floor-control outcomes under contention.
- EN 300 392-2 clauses 14.5.1.1.2 and 14.7.2.3: simple individual call setup and connect acknowledgement.
- This is clause-scoped regression evidence, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Renamed and extended the 10k overflow test to `test_ten_thousand_member_group_floor_overflow_is_explicit_and_private_call_still_works`.
  - Added private-call aftercare assertions in the same CMCE runtime.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs test_ten_thousand_member_group_floor_overflow_is_explicit_and_private_call_still_works --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 152 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `rustfmt --edition 2024 --check crates/tetra-entities/tests/test_cmce_bs.rs` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add long-run no-terminal-loss model test: repeated attach/affiliate/EG7/PTT cycles must leave subscriber/group registries coherent.
2. Continue dashboard/telemetry scaling work: bounded client queues and coalesced station rendering for 10k MS snapshots.
3. Add SDS/status aftercare under large registered MS set.

## 2026-06-05 21:46 EEST - Dashboard websocket broadcast queue bounded

Component in simple technical terms:

- Dashboard websocket broadcast sends telemetry updates to browser clients.
- A slow or frozen browser must not create an unbounded queue inside the BS process when thousands of MS/group/EG events are flowing.

Problem covered:

- The server state now handles group/EG event ordering correctly, but per-client websocket queues were unbounded.
- A slow dashboard tab could accumulate unlimited broadcast messages during a high-rate 10k-station event storm.
- The patch bounds each websocket broadcast queue at 4096 messages. If a browser cannot drain that queue, the server drops that slow client instead of growing memory without limit.

ETSI clause scope:

- This patch is not an ETSI air-interface change. It is operational hardening around dashboard observability for clause-scoped MM/CMCE/UMAC events.
- It does not change over-air TETRA PDUs, floor control, attach, SDS, or Energy Economy behaviour.

Patch summary:

- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Added `DASHBOARD_WS_BROADCAST_QUEUE_MAX`.
  - Changed websocket broadcast channels from unbounded to bounded.
  - Changed `broadcast()` to use `try_send`; full queues drop the slow client with a warning.
  - Added `dashboard_broadcast_drops_slow_ws_client_at_bounded_queue_cap`.

Verification:

- `cargo test -p tetra-entities dashboard_broadcast_drops_slow_ws_client_at_bounded_queue_cap --locked` -> 1 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `rustfmt --edition 2024 --check crates/tetra-entities/src/net_dashboard/server.rs` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Coalesce/limit browser station rendering for 10k MS snapshots so UI remains responsive.
2. Add long-run no-terminal-loss model test: repeated attach/affiliate/EG7/PTT cycles must leave subscriber/group registries coherent.
3. Add SDS/status aftercare under large registered MS set.

## 2026-06-05 21:49 EEST - Dashboard station render coalescing

Component in simple technical terms:

- The browser dashboard keeps a local station table and redraws it when MS registration, RSSI, group, or Energy Economy telemetry arrives.
- With thousands of terminals, many station events can arrive in one browser frame. Redrawing the whole table for every single event can freeze or lag the UI.

Problem covered:

- Station-state updates are still applied immediately, but table rendering is now coalesced with `requestAnimationFrame`.
- Multiple MS events in the same browser frame produce one station-table redraw.
- A `setTimeout(..., 16)` fallback keeps compatibility if `requestAnimationFrame` is unavailable.

ETSI clause scope:

- This is dashboard/browser scalability, not an ETSI air-interface behaviour change.
- It supports reliable operator visibility for clause-scoped MM/CMCE/UMAC events without changing TETRA PDUs.

Patch summary:

- `crates/tetra-entities/src/net_dashboard/html.rs`
  - Added `scheduleRenderStations()`.
  - Changed MS event handlers (`ms_registered`, `ms_deregistered`, `ms_rssi`, `ms_groups`, `ms_groups_detach`, `ms_groups_all`, `ms_energy_saving`) to schedule one station redraw instead of rendering immediately.
- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Added `dashboard_browser_coalesces_station_event_rendering` string regression.

Verification:

- `cargo test -p tetra-entities dashboard_browser_coalesces_station_event_rendering --locked` -> 1 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `rustfmt --edition 2024 --check crates/tetra-entities/src/net_dashboard/server.rs` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Add long-run no-terminal-loss model test: repeated attach/affiliate/EG7/PTT cycles must leave subscriber/group registries coherent.
2. Add SDS/status aftercare under large registered MS set.
3. Continue telemetry channel policy review; keep core telemetry lossless unless a protocol-safe coalescing layer is added.

## 2026-06-05 21:57 EEST - Large-group MM/SDS robustness evidence

Component in simple technical terms:

- MM is the registration and affiliation ledger: it tracks each ISSI, which GSSI groups it belongs to, and the negotiated Energy Economy mode.
- SDS/status is short data and pre-coded status delivery. For a group address it must stay one GSSI transmission, not fan out into thousands of per-ISSI messages.
- CMCE controls who has the PTT floor in a group call. UMAC is the radio scheduler that has to deliver the resulting floor-control signalling while respecting EG7 listen windows.

Problem covered:

- Added a long-run MM model test with 4096 registered members on one GSSI in EG7. After initial attach and EG7 confirmation, every member performs three group-less `DemandLocationUpdating` refresh cycles.
- The test proves that these refreshes do not create a D-LOCATION-UPDATE-COMMAND loop, do not emit `Deregister`/`Deaffiliate`, keep all 4096 members in the GSSI registry, preserve sampled EG7 assignments, and keep the restart-recovery cache coherent.
- Added SDS/status large-GSSI tests with 1024 locally registered and affiliated members. U-SDS and U-STATUS to the group produce exactly one GSSI-addressed downlink PDU using unacknowledged L2, with no per-ISSI fan-out and no Brew forwarding.
- Re-ran existing CMCE 4096/10k large-group floor-control tests and UMAC large-group PTT storm tests so this checkpoint ties MM persistence, SDS/status group routing, CMCE floor admission, and UMAC scheduling evidence together.

ETSI clause scope:

- EN 300 392-2 clauses 16.4.4, 16.8.0, 16.9.3.4, and 16.10.35a for location update/group-affiliation persistence during group-less refresh.
- EN 300 392-2 clauses 16.7.1, 16.10.9, 16.10.10, 23.7.6, and T.210 for preserving negotiated EG7 state until a real energy-economy procedure changes it.
- EN 300 392-2 clauses 13.2, 14.7.1.11, 14.7.2.7, and 18.3.5.3.1 for SDS/status group delivery over unacknowledged GSSI addressing.
- EN 300 392-2 clause 14.5.2.2.1 and clause 23.5 for existing group floor-control grant/queued/not-granted signalling and assigned-channel delivery checks.
- This remains clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added `test_large_eg7_group_repeated_group_less_updates_preserve_all_affiliations`.
- `crates/tetra-entities/tests/test_sds_bs.rs`
  - Added a 1024-member local group helper.
  - Added large-GSSI U-SDS and U-STATUS routing tests.

Verification:

- `cargo test -p tetra-entities --test test_mm_bs test_large_eg7_group_repeated_group_less_updates_preserve_all_affiliations --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery_large --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_sds_bs large_local_group --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_sds_bs --locked` -> 119 passed.
- `cargo test -p tetra-entities --test test_cmce_bs large_group --locked` -> 8 passed.
- `cargo test -p tetra-entities --test test_cmce_bs ten_thousand --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs large_group_ptt_storm --locked` -> 4 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `rustfmt --edition 2024 --check crates/tetra-entities/tests/test_mm_bs.rs crates/tetra-entities/tests/test_sds_bs.rs` -> pass.

Next non-repeating execution:

1. Add deterministic MM restart-recovery startup sweep evidence for a large cached group where overdue probes stay ordered by ISSI and do not burst if ticks are delayed.
2. Add UMAC same-priority protected-control admission/coalescing evidence for over-cap floor-withdraw storms, especially when every queued element is already protected.
3. Add operator-visible metrics or config documentation for hard local scale caps: CMCE floor FIFO 4096, UMAC pending TMA 4096, and RA ACK queue 8192.

## 2026-06-05 22:49 EEST - WAP SDS live send exposed TxReporter late-completion crash

Component in simple technical terms:

- WAP MVP currently sends the Nexus-BS browser page as raw SDS Type4 payload.
- CMCE/SDS builds `D-SDS-DATA`, LLC handles acknowledged BL-DATA delivery for ISSI targets, and UMAC/fragger/scheduler split large payloads into MAC fragments until MAC-END.
- `TxReporter` is local bookkeeping shared by LLC and UMAC. It records whether a submitted downlink PDU is still pending, transmitted, discarded, lost, or acknowledged.

Live issue covered:

- Sent the WAP SDS page to ISSI `2260618` using `nexus-bs-control-command sendwap 16777215 2260618 false`.
- Control accepted the command and BS logged `SDS: received raw from Control ... sdti=3, 1984 bits`.
- The first live send then panicked with `TxReporter: invalid transition Pending -> Transmitted (actual state: Discarded)`.
- Root cause: stale/late MAC completion can arrive through a cloned `TxReporter` after LLC/UMAC has already marked the request discarded due cancellation, timeout, or retry state. That must be a failed/late local report, not a process-fatal panic.

ETSI clause scope:

- EN 300 392-2 clause 13.2 covers SDS service scope.
- EN 300 392-2 clause 20.4.1.1.3 covers MAC `TMA-REPORT` completion/failure reporting toward LLC.
- EN 300 392-2 clause 22.3.2.3 covers LLC acknowledged BL-DATA retry/failure handling.
- This patch does not change over-air SDS/CMCE PDUs or formal conformance status. It hardens local TxReporter state handling so late MAC reports cannot crash Nexus-BS.

Patch summary:

- `crates/tetra-core/src/tx_receipt.rs`
  - Added non-panicking `try_mark_transmitted()` and `try_mark_discarded()` for asynchronous MAC/LLC report paths.
  - Kept existing strict `mark_*()` methods and panic tests for code paths that require invariant checking.
- `crates/tetra-entities/src/llc/llc_bs_ms.rs`
  - Made LLC helper marking atomic with the new try-mark operations.
- `crates/tetra-entities/src/umac/subcomp/bs_frag.rs`
  - MAC fragment completion now ignores/logs a late complete report if the reporter is already final.
  - Added regression test for MAC-END completion after local discard.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Group and stealing completion reports now tolerate late reporter state.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - TMA admission/timeout discard paths now use non-panicking pending-only discard.

Verification:

- `cargo test -p tetra-core tx_receipt --locked` -> 7 passed.
- `cargo test -p tetra-entities --lib test_late_fragment_completion_after_discard_is_ignored --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_out_fragmented_resource --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs tma_report --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_sds_bs network_origin --locked` -> 10 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 80 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Cross-build Nexus-BS/control locally for aarch64 and deploy directly to `~/nexus-bs` on `chris@192.168.1.179`, no build on Pi and no backup binary.
2. Restart `nexus-bs@chris.service`, resend WAP SDS to `2260618`, and check journal for `SendRawSdsResponse { success: true }` with no panic.
3. Continue SDS/WAP accepted-vs-transmitted observability so operator UI distinguishes accepted command, MAC transmitted, failed transfer, and terminal ACK where available.

## 2026-06-05 22:03 EEST - Restart-recovery overdue sweep pacing

Component in simple technical terms:

- MM restart recovery is the BS-side process that re-probes known ISSIs after a BS restart using `D-LOCATION-UPDATE-COMMAND`.
- With thousands of cached terminals, many probes can become due if the process stalls or the scheduler tick is delayed. The BS must still pace commands; otherwise it can overload RF/control signalling and leave terminals in inconsistent "Unit Not Attached" states.

Problem covered:

- Added a 4096-member cached GSSI test that initializes MM, jumps TDMA time far enough for many restart probes to be overdue, and verifies only one acknowledged `D-LOCATION-UPDATE-COMMAND(group_identity_report=1)` is emitted.
- The test then verifies the 72-timeslot inter-ISSI pacing window stays quiet, the next ISSI is probed in deterministic order, and a second large delayed tick still emits only one further probe.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4 covers SwMI-commanded location updating using `D-LOCATION-UPDATE-COMMAND`.
- The 72-timeslot inter-ISSI delay is Nexus-BS local RF robustness policy around that standardized procedure, not an ETSI timer.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added `test_restart_recovery_large_cache_overdue_sweep_remains_ordered_and_paced`.

Verification:

- `cargo test -p tetra-entities --test test_mm_bs test_restart_recovery_large_cache_overdue_sweep_remains_ordered_and_paced --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery_large --locked` -> 4 passed.
- `cargo check -p tetra-config -p tetra-entities --locked` -> pass.
- `rustfmt --edition 2024 --check crates/tetra-entities/tests/test_mm_bs.rs` -> pass.

Next non-repeating execution:

1. Add UMAC same-priority protected-control admission/coalescing evidence for over-cap floor-withdraw storms, especially when every queued element is already protected.
2. Add operator-visible metrics or config documentation for hard local scale caps: CMCE floor FIFO 4096, UMAC pending TMA 4096, and RA ACK queue 8192.
3. Add cross-layer long-run SDS/status plus group/PTT aftercare in one runtime so data service storms cannot regress call-control floor handling.
## 2026-06-06 00:42 EEST - Private simplex repeated Motorola PTT LMAC stolen-half scoping

Component in simple technical terms:

- LMAC is the lower MAC that decides whether an uplink burst half is traffic speech (`TCH/S`) or stolen signalling (`STCH`).
- UMAC parses first-half STCH/MAC signalling and tells LMAC when the second half of the same burst is also stolen.
- In private simplex, a Motorola can send `U-TX CEASED`, then later press PTT again on the same assigned timeslot; the later speech half-slot must not inherit an old stolen-half marker.

Live issue covered:

- User reported `2260082 -> 2260618`, both Motorola: first PTT from `2260082` had voice, but subsequent PTTs from `2260082` had no useful voice/static while signalling looked normal.
- Fresh journal evidence for call_id `4` showed initial speech (`speech_present=true`), then `U-TX CEASED`, `D-TX CEASED`, a later `U-TX DEMAND`/`D-TX GRANTED`/`UMAC floor granted`, but no accepted uplink speech before `UL inactivity timeout`.
- Patch scopes `blk2_stolen` to the exact uplink `TdmaTime`, preventing a stale STCH second-half indication from suppressing later valid `TCH/S` Block2 on private-simplex re-entry.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1 b/e covers private simplex `U-TX DEMAND`, `D-TX GRANTED`, and end-of-transmission `D-TX CEASED`.
- EN 300 392-2 clause 21.4.5 covers STCH use and the second-half-stolen indication.
- EN 300 392-2 clauses 23.8.4.2.2 and 23.8.5 require the BS to preserve valid non-stolen `TCH/S` half-slot speech instead of interpreting it as signalling.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - `TmvConfigureReq { blk2_stolen: Some(true) }` now carries the exact uplink `TdmaTime`.
- `crates/tetra-entities/src/lmac/lmac_bs.rs`
  - Replaced the global `blk2_stolen` bool with per-timeslot `Option<TdmaTime>`.
  - LMAC applies the stolen second-half marker only to Block2 from the same received UL burst and clears stale markers.
- `crates/tetra-entities/tests/test_lmac_bs.rs`
  - Added `bs_lmac_ignores_stale_blk2_stolen_marker_for_later_tch_s_block2`.

Verification:

- `cargo fmt --check --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 7 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 70 passed.
- `cargo check -p tetra-entities --locked` -> pass.

Next non-repeating execution:

1. Cross-build locally and deploy direct to `/home/chris/nexus-bs`; do not compile on Pi and do not create backup binaries.
2. Restart `nexus-bs@chris.service`.
3. Retest `2260082 -> 2260618` with repeated PTT from `2260082`; expected post-fix log has `U-TX DEMAND`, `D-TX GRANTED`, `UMAC floor granted`, then accepted `TCH/S`/no `UL inactivity timeout`.

## 2026-06-05 23:12 EEST - Private simplex first-PTT floor seeding and LLC reporter split completed

Component in simple technical terms:

- CMCE private call control decides who owns the simplex P2P transmit floor during setup.
- UMAC uses that floor owner to accept and route the first TCH/S speech burst instead of treating it as unknown/static media.
- LLC handles ACK/retry for SDS/WAP BL-DATA; its service reporter is the user-visible transfer result, while each MAC attempt gets its own reporter.

Live issue covered:

- User reported a private simplex P2P regression: first PTT opens the call but carries no voice; second PTT carries voice.
- Root cause matched setup-time ordering: the internal UMAC `FloorGranted` must be seeded immediately after the traffic bearer opens and before `D-CONNECT` / `D-CONNECT-ACKNOWLEDGE` can let an MS send the first TCH/S burst.
- Also completed the unfinished LLC split so SDS/WAP acknowledged transfers keep correct service-level `Transmitted -> Acknowledged/Lost` state while retries use fresh per-attempt MAC reporters.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1 covers private call setup and initial simplex transmit permission.
- EN 300 392-2 table 14.74 covers the `request_to_transmit_send_data` setup bit used to decide caller-first vs called-first initial floor.
- EN 300 392-2 clauses 23.5 and 23.5.2.2.1 cover assigned-channel traffic/signalling handling used by UMAC to route TCH/S.
- EN 300 392-2 clause 22.3.2.3 covers LLC acknowledged BL-DATA, T.251/N.252 retry, ACK, and failed-transfer handling.
- This remains clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Moved private-simplex initial floor seeding before over-air connect PDUs.
  - Kept simplex as one shared assigned bearer and did not add non-standard setup-time `D-TX GRANTED`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added direct FIFO `CmceBs` tests for `Open -> FloorGranted -> D-CONNECT/D-CONNECT-ACKNOWLEDGE` ordering.
  - Covered both caller-first and called-MS-first hook setup cases.
- `crates/tetra-entities/src/llc/llc_bs_ms.rs`
  - Finished service reporter vs per-attempt MAC reporter split for acknowledged BL-DATA.
  - Fresh MAC reporter is attached for each TMA attempt/retry; service reporter receives first-complete, ACK, and lost/failed state.
- `crates/tetra-entities/tests/test_llc_bs.rs`
  - Updated BL-DATA/BL-ADATA ACK, wrong-ACK, fragmentation, endpoint, and T.251 exhaustion tests to assert service reporter and MAC reporter states separately.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 69 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 82 passed.
- `cargo test -p tetra-core tx_receipt --locked` -> 7 passed.
- `cargo test -p tetra-entities --lib test_late_fragment_completion_after_discard_is_ignored --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_sds_bs network_origin --locked` -> 10 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Live deploy / runtime verification:

- Built locally for `aarch64-unknown-linux-gnu` with the Nexus-BS cross-build environment; no build was run on Pi.
- Deployed directly to `/home/chris/nexus-bs`, no backup binaries:
  - `/home/chris/nexus-bs/nexus-bs` sha256 `8a3f8d05307d50ee196140d6975925d95fa562029c3202ceaf1bb6a43027ff0f`.
  - `/home/chris/nexus-bs/nexus-bs-control-service` sha256 `d5dbaff8d4e6bac01c708ffa5e903759c47e466a207e5e16cbaac00ae0e1a17f`.
- `nexus-bs@chris.service` and `nexus-bs-control@chris.service` restarted and reported `active`.
- Restart recovery observed ISSIs `2260082`, `2260616`, and `2260618`; `2260618`, `2260616`, and `2260082` re-registered/affiliated to GSSI `226333`.
- Sent WAP color SDS to ISSI `2260618`; control logged `SendRawSds`, CMCE logged raw SDS Type4 for `2260618`, and control returned `SendRawSdsResponse { success: true }`.
- No `TxReporter` panic was observed after the WAP SDS send.

Next non-repeating execution:

1. Run live private simplex first-PTT retest and inspect the journal for `Simplex P2P ... initial floor_holder`, `UMAC floor granted`, and absence of first-burst static/no-voice symptoms.
2. If first PTT is now clean, run a reverse-direction private simplex test and then a short GSSI PTT sequence to ensure group call floor handling did not regress.
3. Continue with MM/EG7 restart-recovery and SDS/WAP long-run soak after voice basics are stable.

## 2026-06-05 23:42 EEST - Private simplex first reverse-PTT media preservation

Component in simple technical terms:

- CMCE is the call-control layer. It decides which radio owns the private simplex PTT floor and sends `D-TX GRANTED`.
- UMAC is the traffic scheduler. It receives CMCE `FloorGranted`, routes uplink TCH/S voice to the peer downlink timeslot, and clears stale audio when the speaker changes.
- `CircuitMgr` is the small UMAC queue that holds speech blocks waiting to be transmitted on a downlink traffic slot.

Live issue covered:

- User reported Hytera `2260616` -> Motorola MXP600 `2260618`: first Hytera PTT makes the MXP600 beep twice and show `Private`, but no voice; second PTT carries voice.
- The earlier setup-floor patch fixed initial call floor seeding. This report is the next path: called MS initially owns the simplex floor, then Hytera asks for floor with `U-TX DEMAND`.
- The risk was that UMAC `FloorGranted` cleared both crossed P2P media queues before preserving media that had already arrived from the newly granted UL timeslot. That can erase the first valid TCH/S Block2 from the requester and leave only FACCH/STCH control on the peer downlink.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1 b) covers private simplex `U-TX DEMAND` / `D-TX GRANTED`; the SwMI grants one MS and informs the peer.
- EN 300 392-2 clause 14.5.1.4.2 says `D-TX GRANTED` switches U-plane on for transmit or receive according to the grant.
- EN 300 392-2 clauses 23.5, 23.8.4.2.2, and 23.8.5 allow FACCH/STCH stealing but require valid non-stolen TCH/S half-slot timing/order to be preserved by the BS.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/subcomp/circuit_mgr.rs`
  - Added source metadata to queued DL speech blocks: source UL timeslot and optional speaker ISSI.
  - Added `clear_tx_data_except_source()` so a private floor change can discard stale media while preserving media from the newly granted speaker.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Exposed source-aware DL media scheduling and source-aware queue clearing to UMAC.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Tags locally routed ACELP and raw TCH/S Block2 with source metadata.
  - On private `FloorGranted`, validates the participant before clearing queues, sets the current source speaker, then clears only media that does not belong to the newly granted source.
  - Group-call `FloorGranted` cleanup keeps the existing GSSI behavior.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_private_simplex_floor_grant_preserves_first_requester_raw_block2`, using lab-shaped ISSIs `2260616` and `2260618`.
  - The regression submits the first requester raw Block2 before/around `FloorGranted` and proves it reaches the MXP600 peer DL slot instead of being purged.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs test_private_simplex_floor_grant_preserves_first_requester_raw_block2 --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 5 passed.
- `cargo test -p tetra-entities --lib circuit_mgr --locked` -> 6 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 69 passed, no warnings after cleanup.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Cross-build locally and deploy direct to `~/nexus-bs` on `chris@192.168.1.179`; do not compile on Pi and do not create backup binaries.
2. Restart `nexus-bs@chris.service`.
3. Retest private simplex `2260616 -> 2260618`: first Hytera PTT should produce `U-TX DEMAND`, `D-TX GRANTED`, `UMAC floor granted`, and then TCH/S voice on the first attempt, without the two-beep/no-voice symptom.

Deploy/runtime follow-up:

- Built locally for `aarch64-unknown-linux-gnu`; no compile was run on Pi.
- Deployed directly into flat final layout `/home/chris/nexus-bs`, no backup binaries:
  - `/home/chris/nexus-bs/nexus-bs` sha256 `01c6d2822e8ac727593595bee7810f28bd607c6b339437cf17c6331e81604aee`.
  - `/home/chris/nexus-bs/nexus-bs-control-service` sha256 `6f56ac2e660f1888ac3998fca34775d2415f917efd610f90aa8ce6ccbeeaa915`.
- Restarted systemd services:
  - `nexus-bs@chris.service` active, main PID `42312`.
  - `nexus-bs-control@chris.service` active, control PID `42306`.
- Fresh journal from restart `2026-06-05 23:46:25 EEST`:
  - Build line: `Build: v0.1.55-332fa519-modified`.
  - `2260616` registered and affiliated to group `[91]`.
  - `2260618` registered and affiliated to group `[226333]`.
  - `2260082` registered and affiliated to group `[226333]`.
  - No post-deploy `PTT denied`, `Service unavailable`, `Unit Not Attached`, panic, or error appeared in the checked startup filter.

Next live action:

1. Operator should retest private simplex `2260616 -> 2260618`, first Hytera PTT.
2. Inspect only journal entries after `2026-06-05 23:46:25 EEST` for `U-TX DEMAND`, `D-TX GRANTED`, `UMAC floor granted`, `UMAC voice route`, and `rx_blk_traffic`.
3. For group-call validation, first move `2260616` back to GSSI `226333`; current startup affiliation shows `[91]`.

## 2026-06-05 23:59 EEST - Private simplex shared-timeslot first requester media tag fix deployed

Component in simple technical terms:

- CMCE decides which private-call participant gets the simplex PTT floor and emits `D-TX GRANTED`.
- UMAC tracks that floor and queues TCH/S speech blocks for the active traffic channel.
- On one shared private-simplex timeslot, raw TCH/S does not contain an ISSI, so UMAC must not label the first requester speech half-slot with the previous floor holder before `FloorGranted` arrives.

Live issue covered:

- Live test `2260616 -> 2260618` still produced two MXP600 beeps and no voice on the first Hytera PTT on the previously deployed binary.
- Journal evidence from call IDs 5 and 6 showed shared private-simplex bearer `peer_ts=None`, initial floor holder `2260618`, later `U-TX DEMAND` from `2260616`, immediate `UMAC floor granted`, then two FACCH/STCH frames with `speech_present=false` before speech appeared.
- Patch prevents first requester media from being purged if raw Block2 arrives before `current_ul_speaker` is switched from the old floor holder on a shared private-simplex bearer.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1 b) requires an explicit `D-TX GRANTED` response to a private-simplex `U-TX DEMAND` and informs the other MS with "granted to another user".
- EN 300 392-2 clause 14.5.1.4.2 switches U-plane transmit/receive according to `D-TX GRANTED`.
- EN 300 392-2 clauses 23.5 and 23.8.5 cover assigned-channel FACCH/STCH stealing while preserving valid TCH/S half-slot media.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added `ul_media_speaker_tag()`.
  - For shared private-simplex bearers (`private_participant_scoped && peer_ts=None`), locally queued UL media is tagged by source timeslot but not by the current speaker ISSI, because the current speaker can still be the previous floor holder during the first requester media race.
  - Cross-routed P2P and group bearers keep speaker tagging for stale-media filtering.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_private_simplex_shared_ts_floor_grant_preserves_first_requester_raw_block2` with field ISSIs `2260616` and `2260618`.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs test_private_simplex_shared_ts_floor_grant_preserves_first_requester_raw_block2 --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_private_simplex_floor_grant_preserves_first_requester_raw_block2 --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 6 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 69 passed.
- `cargo test -p tetra-entities --lib circuit_mgr --locked` -> 6 passed.
- `git diff --check` -> pass.

Deploy/runtime:

- Built locally for `aarch64-unknown-linux-gnu`; no compile was run on Pi.
- Deployed directly into flat final layout `/home/chris/nexus-bs`, no backup binaries:
  - `/home/chris/nexus-bs/nexus-bs` sha256 `89538e2374a664fa25eb3e130631ed4375b8608ab29f9574b92a929f4b671cf8`.
  - `/home/chris/nexus-bs/nexus-bs-control-service` sha256 `825fb34108f0fbd57255d02739e4549b92091c01184c1c7d4c2cc8ec1b11587a`.
- Restarted with sudo systemd after normal user stop was blocked by masked polkit:
  - `nexus-bs@chris.service` active.
  - `nexus-bs-control@chris.service` active.
- Fresh startup at `2026-06-05 23:58:54 EEST`:
  - Build line `v0.1.55-332fa519-modified`.
  - Restart recovery armed for `2260082`, `2260616`, `2260618`.
  - `2260618`, `2260616`, and `2260082` registered; all observed on group `[226333]` after startup/recovery.

Next non-repeating execution:

1. Retest private simplex `2260616 -> 2260618`, first Hytera PTT, on the new post-23:58 deploy.
2. Inspect journal after `2026-06-05 23:58:54 EEST` for `U-TX DEMAND`, `D-TX GRANTED`, `UMAC floor granted`, `speech_present`, raw TCH/S preservation, and absence/presence of two-beep/no-voice.
3. If the MXP600 still hears two beeps with no voice, add a scheduler-level regression proving the first post-grant TCH/S Block2 is not starved by RA ACK plus requester/peer `D-TX GRANTED` STCH sequence.

## 2026-06-06 00:50 EEST - Private simplex repeated Motorola PTT deployed

Component in simple technical terms:

- LMAC is the lower MAC that classifies each uplink burst half as speech (`TCH/S`) or stolen signalling (`STCH`).
- UMAC is the upper MAC scheduler that grants the PTT floor and forwards accepted speech to the peer downlink.
- CMCE is call control; for private simplex it processes `U-TX DEMAND`, sends `D-TX GRANTED`, and keeps the call/floor state coherent.

Live issue covered:

- User reported `2260082 -> 2260618`, both Motorola: the first PTT from `2260082` carried voice, but following PTTs from `2260082` had normal signalling and no useful voice/static.
- The expected failure mode after `U-TX CEASED`/`D-TX CEASED` is a stale second-half-stolen marker incorrectly treating a later valid speech half-slot as signalling.
- Patch scopes `blk2_stolen` to the exact uplink `TdmaTime` and clears stale markers before later `TCH/S` Block2 processing.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1 b/e covers private simplex `U-TX DEMAND`, `D-TX GRANTED`, `U-TX CEASED`, and `D-TX CEASED`.
- EN 300 392-2 clause 21.4.5 covers stolen signalling channel handling and second-half-stolen indication.
- EN 300 392-2 clauses 23.8.4.2.2 and 23.8.5 require preserving valid non-stolen `TCH/S` media timing/order instead of forwarding signalling bits as speech or suppressing speech.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Verification:

- `cargo fmt --check --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 11 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 7 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 70 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- Local AArch64 build completed for `nexus-bs` and `nexus-bs-control-service`; no build was run on Pi.

Deploy/runtime:

- Deployed directly into flat final layout `/home/chris/nexus-bs`, no backup binaries:
  - `/home/chris/nexus-bs/nexus-bs` sha256 `84555da3ac376ac5523084a10d8e8ae09407da206f446528077d078e2a155fb9`.
  - `/home/chris/nexus-bs/nexus-bs-control-service` sha256 `df54f6383b8eaf5fd18aca8b41a4940491cedd2ed022855223accb0fafe3b7f9`.
- Restarted systemd services:
  - `nexus-bs-control@chris.service` active from `2026-06-06 00:46:50 EEST`.
  - `nexus-bs@chris.service` active from `2026-06-06 00:46:51 EEST`.
- Fresh startup journal:
  - Build line `Build: v0.1.55-332fa519-modified`.
  - Restart recovery armed for `{2260082, 2260616, 2260618}`.
  - `2260616`, `2260618`, and `2260082` registered and affiliated to group `[226333]`.
  - No post-restart `Unit Not Attached`, `PTT denied`, `Service unavailable`, panic, or error appeared in the checked startup filter.

Next non-repeating execution:

1. Retest private simplex `2260082 -> 2260618` on the post-`00:46:51 EEST` deploy; press PTT from `2260082` repeatedly in the same call.
2. Inspect only journal after `2026-06-06 00:46:51 EEST` for `U-TX DEMAND`, `D-TX GRANTED`, `UMAC floor granted`, `rx_blk_traffic`, `UMAC voice route`, `lmac_bs: dropping stale blk2_stolen marker`, `UL inactivity timeout`, and any `PTT denied`.
3. If repeated PTT from `2260082` still has static/no voice, capture the post-restart call log and distinguish stale STCH classification from downlink traffic queue starvation.

## 2026-06-06 00:53 EEST - SDS/status reporter late-discard hardening

Component in simple technical terms:

- SDS-BS is the CMCE short data/status sub-entity for network-origin and mobile-origin SDS.
- `TxReporter` is the small shared receipt object used by control/Brew/dashboard and lower layers to say whether one SDS/status request was transmitted, acknowledged, lost, or discarded.
- A local reject path must fail a pending SDS/status request, but it must not panic if an async air-delivery path has already completed the same receipt.

Issue covered:

- Existing UMAC/LLC paths already use non-panicking `try_mark_*` reporter completion for late async reports.
- SDS/status local reject paths still used strict `mark_discarded()`.
- If a stale cloned reporter reached a local reject path after the same handle was already marked transmitted, Nexus-BS could panic instead of preserving the earlier transfer result.

ETSI clause scope:

- EN 300 392-2 clause 13.3.2.2 defines one `TNSDS-REPORT` transfer result for the handle belonging to a `TNSDS-UNITDATA` or `TNSDS-STATUS` request.
- EN 300 392-2 clause 18.3.5.3.1 maps higher-layer requests through MLE/LLC reporting and confirms/failed reports.
- This patch is internal report-state robustness for SDS/status; it does not change over-air `D-SDS-DATA`, `D-STATUS`, `U-SDS-DATA`, or `U-STATUS` encoding.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-entities/src/cmce/subentities/sds_bs.rs`
  - `discard_status_reporter()` now uses `try_mark_discarded()`.
  - `discard_sds_reporter()` now uses `try_mark_discarded()`.
  - Pending requests still become `Discarded`; late discards after `Transmitted`/final state are ignored.
- `crates/tetra-entities/tests/test_sds_bs.rs`
  - Added late-discard regression for network-origin SDS data.
  - Added late-discard regression for network-origin SDS status.

Verification:

- `cargo fmt --check --package tetra-entities` -> pass.
- `cargo test -p tetra-entities --test test_sds_bs late_discard --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_sds_bs raw_sds --locked` -> 20 passed.
- `cargo test -p tetra-entities --test test_sds_bs network_origin --locked` -> 11 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.
- `cargo test -p tetra-entities --test test_sds_bs --locked` -> 121 passed.

Deploy/runtime:

- Not deployed yet. The active BS remains the post-`2026-06-06 00:46:51 EEST` private-simplex test build so live `2260082 -> 2260618` repeated-PTT validation is not interrupted.

Next non-repeating execution:

1. Keep the current BS running for the private simplex repeated-PTT retest.
2. Include the SDS/status reporter hardening in the next AArch64 build/deploy after the current P2P voice retest window.
3. Continue LLC report observability so control/dashboard can distinguish command accepted, over-air transmitted, acknowledged, lost, and locally discarded.

## 2026-06-06 00:58 EEST - LLC acknowledged-transfer late-loss reporter hardening

Component in simple technical terms:

- `TxReporter` is the shared transmit receipt used by MAC/LLC/control code.
- LLC basic-link acknowledged transfer sends a `BL-DATA`/`BL-ADATA`, reports first complete transmission, waits for `BL-ACK`, and finally marks the service request as acknowledged or lost.
- The service-level reporter and per-MAC-attempt reporter are deliberately separate so retries do not reset the user-visible SDS/MM/CMCE request state.

Issue covered:

- Existing code already had non-panicking `try_mark_transmitted()` and `try_mark_discarded()` for async MAC/LLC report paths.
- The failure side for an already-transmitted acknowledged TL-SDU still used strict `mark_lost()`.
- A late T.251 expiry, wrong-ACK exhaustion, fragmentation exhaustion, or MAC failure observed after another async path completed the reporter could panic instead of preserving one final transfer result.

ETSI clause scope:

- EN 300 392-2 clause 22.3.2.3(f) defines first complete transmission reporting and T.251 start for acknowledged BL-DATA/BL-ADATA.
- EN 300 392-2 clause 22.3.2.3(g/h/i/k) defines failed-transfer handling for MAC failure, fragmentation failure, T.251 expiry, and wrong `BL-ACK N(R)`.
- EN 300 392-2 Annex A.1 defines T.251 in downlink signalling frames.
- This patch is internal reporter-state robustness; it does not change over-air LLC PDU encoding or sequence numbering.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch summary:

- `crates/tetra-core/src/tx_receipt.rs`
  - Added `TxReporter::try_mark_lost()` for non-panicking `Transmitted -> Lost`.
  - Added tests for normal lost marking and late lost after acknowledgement.
- `crates/tetra-entities/src/llc/llc_bs_ms.rs`
  - `mark_ack_service_failed()` now uses `try_mark_discarded()` / `try_mark_lost()` instead of strict final-state transitions.
  - Pending transfers still become `Discarded`; transmitted ACK-wait transfers still become `Lost`; already-final transfers are left alone.

Verification:

- `cargo test -p tetra-core tx_receipt --locked` -> 9 passed.
- `cargo test -p tetra-entities --test test_llc_bs random_access_failure --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_llc_bs wrong_bl_ack --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 82 passed.
- `cargo fmt --check --package tetra-core --package tetra-entities` -> pass.
- `cargo check -p tetra-core -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deploy/runtime:

- Not deployed yet. The active BS remains the post-`2026-06-06 00:46:51 EEST` private-simplex test build so live `2260082 -> 2260618` repeated-PTT validation is not interrupted.

Next non-repeating execution:

1. Keep current live BS untouched for the P2P repeated-PTT test unless the user requests deployment.
2. Include SDS/status reporter hardening and LLC late-loss hardening in the next local AArch64 build/deploy.
3. Continue with LLC report observability and per-link admission/backpressure after voice retest evidence is collected.

## 2026-06-06 01:17 EEST - Private simplex first/repeated PTT media guard

Component in simple technical terms:

- CMCE is call control: it decides which terminal owns private-call PTT/floor and emits the standardized grant/ceased messages.
- UMAC is the upper MAC scheduler: it turns CMCE floor state into assigned-channel traffic, STCH/FACCH signalling, and TCH/S speech toward the peer terminal.
- TCH/S is the radio speech channel. STCH/FACCH is signalling that can steal part of a traffic burst, for example to carry floor control.

Live issue covered:

- User reported Motorola still beep-beep/signalling on private simplex, but the first/repeated PTT from the terminal sometimes carried no useful voice.
- Audit found a race where valid private-call speech from LMAC could arrive during the short hangtime window before the internal UMAC `FloorGranted` event was processed.
- Old UMAC handling kept only one raw Block2 and dropped it on the next tick if the floor was still in hangtime; ACELP full-slot speech had no equivalent deferral path.

Patch summary:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Replaced the single pending raw Block2 slot with a bounded per-timeslot pending private UL media queue.
  - Supports both raw TCH/S Block2 and ACELP full-slot speech.
  - Applies only to private participant-scoped, non-SwMI media paths; group-call hangtime media is still dropped.
  - Retains media only briefly while UL/DL floor is still in hangtime, then flushes immediately after private `FloorGranted`.
  - Drops pending media on circuit replacement/close, `FloorReleased`, `CallEnded`, source/peer change, inactive circuit, SwMI media source, or TTL expiry.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added delayed private raw Block2 floor-grant regression.
  - Added delayed private ACELP floor-grant regression.
  - Added stale private media expiry regression.
- `README.md` and `example_config/config.toml`
  - Normal HMD/SDS examples now show PID 130 text only; PID 220/0xDC is no longer advertised in user-facing example config for HMD.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.2.1 b/e covers private simplex `U-TX DEMAND`, `D-TX GRANTED`, `U-TX CEASED`, and floor handoff.
- EN 300 392-2 clause 14.5.1.4.2 covers lower-layer U-plane switching from CMCE transmission grants/ceased state.
- EN 300 392-2 clause 23.5 covers assigned-channel signalling transfer.
- EN 300 392-2 clauses 23.8.4.1.4 and 23.8.5 cover TCH/S half-slot/traffic preservation around stolen signalling.
- The bounded queue and TTL are Nexus-BS implementation guards around that clause-scoped behavior. This is engineering evidence only, not formal ETSI/TETRA certification.

Verification:

- `cargo fmt --package tetra-entities --check` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 10 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_group_ul_raw_block2_is_dropped_during_hangtime --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 70 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deploy/runtime:

- Not deployed yet in this entry. Local code is verified and ready for the next local AArch64 build/deploy batch.

Next non-repeating execution:

1. Build locally only for AArch64 using the Nexus-BS SoapySDR sysroot command; do not build on Pi.
2. Deploy directly to `/home/chris/nexus-bs` flat layout, no backup binary.
3. Restart `nexus-bs@chris.service` and `nexus-bs-control@chris.service`.
4. Retest private simplex repeated PTT, especially `2260082 -> 2260618` and `2260616 -> 2260618`, and inspect post-restart logs for `UMAC voice route: deferred`, `D-TX GRANTED`, and absence of no-voice/static on first/repeated PTT.

## 2026-06-06 01:31 EEST - Private simplex release duplicate D-RELEASE guard

Component in simple technical terms:

- CMCE release is the call-control part that ends an individual/private call when a terminal presses the red key or a timer expires.
- FACCH/STCH release means `D-RELEASE` is sent on the already assigned traffic channel.
- MCCH release means `D-RELEASE` is sent again on the common control channel.

Live issue covered:

- Post-deploy log from the live call `2260082 -> 2260618`, call_id `4`, showed `U-DISCONNECT` followed by two `D-RELEASE` logs with `UserRequestedDisconnection`, then peer clear followed by two more `D-RELEASE` logs with `SwmiRequestedDisconnection`.
- The duplicates came from established-call release sending both assigned-channel FACCH/STCH and MCCH fallback copies for the same terminal leg.
- This is suspicious for Motorola peer end-call behaviour because the same call clear can be seen more than once, and the peer can later see a second release cause.

Patch summary:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Established individual-call `D-RELEASE` now sends one reporter-tracked FACCH/STCH release per selected terminal leg.
  - Removed MCCH fallback copy for calls that already have an assigned traffic circuit.
  - Setup-phase releases still use the existing MCCH repeated path because no assigned traffic circuit is established yet.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated established P2P release assertions to require one FACCH/STCH `D-RELEASE` per expected leg and no duplicate MCCH fallback.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.3.1 defines the individual-call disconnect request path where the MS that sent `U-DISCONNECT` waits for `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.2 defines SwMI release of individual calls by `D-RELEASE`.
- EN 300 392-2 clause 23.5 covers assigned-channel signalling transfer; an established assigned-channel MS should receive the call-clear control message on that assigned channel.
- This patch removes a Nexus-BS duplicate-delivery fallback for established calls. It is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Verification:

- `cargo fmt --package tetra-entities --check` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs p2p_u_disconnect --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 70 passed.
- `cargo check -p tetra-entities --locked` -> pass.

Deploy/runtime:

- Not deployed yet in this entry. Include in the next immediate local AArch64 build/deploy with the UMAC private-media guard.

Next non-repeating execution:

1. Run `git diff --check`.
2. Build locally for AArch64.
3. Deploy direct to `/home/chris/nexus-bs`, restart services, and retest private simplex end-call from both sides.
4. In post-restart logs, confirm one `D-RELEASE` log per established release leg rather than FACCH+MCCH duplicate pairs.

## 2026-06-06 02:10 EEST - Private direct setup Annex D.4 called-leg ACK guard

Component in simple technical terms:

- CMCE individual-call setup is the call-control part that turns `U-SETUP` and `U-CONNECT` into the downlink messages each terminal needs before voice may flow.
- `D-CONNECT ACKNOWLEDGE` is the accept/through-connect command sent to the called terminal.
- The L2 ACK is the lower-layer confirmation that the called terminal actually received that command.
- `D-CONNECT` is the through-connect command sent to the calling terminal; after it, the caller may start using the traffic bearer according to the grant.

Live issue covered:

- Motorola-like terminals could show private-call screen/beeps but miss first speech if the caller side was authorized before the called side had confirmed `D-CONNECT ACKNOWLEDGE`.
- This matches the risk described by EN 300 392-2 Annex D.5; Annex D.4 gives the conservative sequence that waits for the called MS acknowledgement first.

Patch summary:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Direct local private setup now sends one acknowledged `D-CONNECT ACKNOWLEDGE` with channel allocation to the called MS first.
  - CMCE stores a pending called-leg ACK state and sends caller `D-CONNECT` only after the `TxReporter` reaches L2 acknowledged.
  - Initial private-simplex `FloorGranted` is also delayed until that called-leg ACK, so the first PTT floor is not released before the called MS is synchronized.
  - A bounded retry/fail guard releases the setup if the called `D-CONNECT ACKNOWLEDGE` is not acknowledged after the configured attempts.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/mod.rs`, `shared.rs`, `timers.rs`
  - Added pending called-leg ACK tracking and timer drain integration, cleaned on call release/cleanup.
- `crates/tetra-config/src/bluestation/sec_cell.rs`, `parsing.rs`, `README.md`, `example_config/config.toml`, `tests/common/default_stack.rs`
  - Removed the stale `private_simplex_force_caller_initial_floor` compatibility switch so private-simplex setup follows ETSI field semantics instead of a caller-first workaround.
- `crates/tetra-pdus/src/cmce/pdus/d_connect.rs`, `d_connect_acknowledge.rs`, `d_setup.rs`, `d_tx_interrupt.rs`, `d_tx_wait.rs`
  - Corrected comments for `transmission_request_permission`: raw bit `0`/`false` means allowed to request transmission; raw bit `1`/`true` means not allowed.
  - Added bit-level tests for `D-CONNECT` and `D-CONNECT ACKNOWLEDGE` polarity.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Private simplex `Open` on one shared assigned channel now keeps bearer/participant/EG context but does not seed the current UL speaker.
  - UMAC authorizes the private-simplex U-plane speaker only on CMCE `FloorGranted`, which now arrives after called `D-CONNECT ACKNOWLEDGE` L2 ACK in the direct setup path.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated private-call setup tests to check the two phases: called `D-CONNECT ACKNOWLEDGE` first, then caller `D-CONNECT` and initial floor after L2 ACK.
  - Updated established network-origin private release expectation to one assigned-channel `D-RELEASE`, matching the duplicate-release guard.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added/updated STCH private signalling tests so private-simplex STCH is dropped before a valid floor and attributed only after `FloorGranted`.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 require incoming/outgoing individual setup messages to indicate the party allowed to transmit.
- EN 300 392-2 clause 14.5.1.2.1 keeps SwMI control of private-simplex transmit permission and request handling.
- EN 300 392-2 clause 14.5.1.4 covers U-plane switching according to the transmit grant.
- EN 300 392-2 tables 14.74 and 14.81 define the request-to-transmit and transmission-request-permission bit semantics.
- EN 300 392-2 Annex D.4 is the informative conservative direct private setup sequence: `D-CONNECT ACKNOWLEDGE` to the called MS, wait for L2 ACK, then `D-CONNECT` to the calling MS.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Verification:

- `cargo check -p tetra-config -p tetra-pdus -p tetra-entities --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 70 passed.
- `cargo test -p tetra-entities --test test_cmce_bs private --locked` -> 19 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 155 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 10 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 70 passed.
- `cargo test -p tetra-config --lib bluestation::parsing --locked` -> 29 passed.
- `cargo test -p tetra-pdus --lib d_connect --locked` -> 12 passed.

Deploy/runtime:

- Not deployed yet in this entry.
- Local AArch64 release build completed for `nexus-bs` and `nexus-bs-control`; no build was run on Pi.
- Next live test should focus on first PTT audio for `2260616 -> 2260618` and repeated Motorola-to-Motorola private-simplex PTT after the called-leg ACK guard is deployed.

Next non-repeating execution:

1. Run formatting and `git diff --check`.
2. Build locally only for AArch64 using the Nexus-BS SoapySDR sysroot command.
3. Deploy direct to `/home/chris/nexus-bs` flat layout, restart services, and retest first PTT audio plus private-call end-call from both sides.

## 2026-06-06 02:20 EEST - UMAC TMA timeout context for live debugging

Component in simple technical terms:

- UMAC is the upper MAC scheduler: it converts LLC/MLE/CMCE downlink requests into MAC resources, STCH/FACCH stealing, fragments, and traffic bursts.
- TMA is the MAC service boundary used by LLC for a `TMA-UNITDATA` request and the matching `TMA-REPORT`.
- `TxReporter` is the local receipt object used to know whether the scheduled MAC item was transmitted, discarded, or left pending.

Issue covered:

- Live logs showed warnings like `UMAC: TMA report req_handle=42 timed out after local pending-report guard`.
- The warning did not say whether the stalled request was `D-TX GRANTED`, `D-TX CEASED`, SDS/HMD, a channel allocation, or ordinary MM/CMCE data.
- That made private/group-call debugging too slow because the handle alone is not enough after a busy run.

Patch summary:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added retained context to each pending TMA report: target address, endpoint, PDU bit length, PDU priority, stealing flag, stealing-repeat flag, channel-allocation summary, admission priority, and detected CMCE downlink PDU type when present.
  - Timeout, cap-drop, and priority-eviction warnings now include that context.
  - The over-air MAC scheduling and the TMA report result values are unchanged.

ETSI clause scope:

- EN 300 392-2 clause 20.4.1.1.3 defines `TMA-REPORT` as the MAC progress/failure report for a `TMA-UNITDATA` request.
- EN 300 392-2 clause 23.1.2.1.1 covers complete MAC PDU transmission reporting.
- EN 300 392-2 clause 23.5 covers assigned-channel signalling/stealing such as `D-TX GRANTED` and `D-TX CEASED`.
- This patch is observability for clause-scoped verification; it does not claim formal ETSI/TETRA certification.

Verification:

- `cargo check -p tetra-entities --locked` -> pass.
- `cargo fmt --package tetra-entities --check` -> pass after formatting.
- `cargo test -p tetra-entities --test test_umac_bs tma --locked` -> 7 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 10 passed before formatting rewrite.
- `git diff --check` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_u_connect_waits_for_called_l2_ack_before_caller_d_connect --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs energy_saving_does_not_allocate_frame_18_start --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_sds_bs status --locked` -> 50 passed.

Deploy/runtime:

- Not deployed yet.
- On the next live run, any future TMA pending-report timeout should identify whether the stalled item was floor control, SDS/HMD, channel allocation, or ordinary data.

Next non-repeating execution:

1. Run `git diff --check`.
2. Run one combined focused verification batch for CMCE private setup, UMAC private-simplex, TMA reports, MM EG frame-18 guards, and SDS/status.
3. Build locally only for AArch64 and deploy direct to `/home/chris/nexus-bs` when ready for the next live test.

## 2026-06-06 02:31 EEST - Annex D.4 negative-path evidence for private direct setup

Component in simple technical terms:

- CMCE/CC individual-call setup is the call-control state machine for private calls.
- `D-CONNECT ACKNOWLEDGE` prepares the called MS and carries the assigned-channel allocation.
- The L2 ACK proves the called MS received that setup command; without it, the BS must not authorize the caller's first traffic burst.
- UMAC `FloorGranted` is the internal switch that lets the traffic scheduler accept voice from the selected ISSI.

Issue covered:

- The direct local private setup logic already followed the conservative Annex D.4 order: called `D-CONNECT ACKNOWLEDGE`, wait for L2 ACK, then `FloorGranted` and caller `D-CONNECT`.
- The remaining evidence gap was the negative path: if the called `D-CONNECT ACKNOWLEDGE` is lost or never L2-acknowledged, tests did not prove that CMCE keeps the caller blocked through retries and releases setup only after exhaustion.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added helper extraction for the called-leg `D-CONNECT ACKNOWLEDGE` `TxReporter`.
  - Added `test_p2p_called_d_connect_ack_l2_loss_never_authorizes_caller_before_release`.
  - The test drives three lost L2 ACK attempts and asserts:
    - one called `D-CONNECT ACKNOWLEDGE` retry per attempt;
    - no caller `D-CONNECT` before called L2 ACK;
    - no UMAC `FloorGranted` before called L2 ACK;
    - setup is released with `AcknowledgedServiceNotComplete` after retry exhaustion.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 define individual-call setup/accept signalling and the `D-CONNECT ACKNOWLEDGE` / `D-CONNECT` through-connect roles.
- EN 300 392-2 clause 14.5.1.2.1 keeps private-simplex transmit permission under SwMI control.
- EN 300 392-2 Annex D.4 describes the conservative direct individual-call setup method: send `D-CONNECT ACKNOWLEDGE` to the called MS, receive L2 ACK, then authorize the calling MS with `D-CONNECT`.
- EN 300 392-2 Annex D.5 warns that the faster alternative can lose first traffic if the called MS misses `D-CONNECT ACKNOWLEDGE`.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_called_d_connect_ack_l2_loss_never_authorizes_caller_before_release --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_u_connect_waits_for_called_l2_ack_before_caller_d_connect --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 71 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 10 passed.
- `cargo fmt --package tetra-entities --check` -> pass.

Deploy/runtime:

- Not deployed in this entry. This patch is test/evidence only; no over-air behavior changed.

Next non-repeating execution:

1. Run `cargo check -p tetra-entities --locked` and `git diff --check`.
2. Continue LLC acknowledged basic-link audit or build/deploy the current local release batch for live private-simplex retest.

## 2026-06-06 02:38 EEST - Annex D.4 direct P2P setup policy extended to duplex evidence

Component in simple technical terms:

- CMCE/CC is the private-call controller. It decides when the called MS is ready and when the caller may receive `D-CONNECT`.
- L2 ACK is the lower-layer proof that the radio actually received the `D-CONNECT ACKNOWLEDGE`.
- Duplex private call has no simplex `FloorGranted`, but the caller still must not receive `D-CONNECT` before the called side is prepared.

User directive implemented:

- Keep the conservative Annex D.4 order for Motorola-like terminals: send `D-CONNECT ACKNOWLEDGE` to the called MS, wait for the L2 ACK, then send `D-CONNECT` to the caller.
- The runtime CMCE path already used `pending_individual_connect_acks` for direct local P2P setup. This entry adds explicit duplex regression evidence so future edits cannot silently reintroduce the faster Annex D.5 order.

Patch summary:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_p2p_duplex_u_connect_waits_for_called_l2_ack_before_caller_d_connect`.
  - The test asserts:
    - no caller `D-CONNECT` before the called `D-CONNECT ACKNOWLEDGE` L2 ACK;
    - called `D-CONNECT ACKNOWLEDGE` uses `Layer2Service::Acknowledged`, has `tx_reporter`, and carries channel allocation;
    - after called L2 ACK, exactly one caller `D-CONNECT` is sent with acknowledged service and channel allocation;
    - no simplex `FloorGranted` is synthesized for duplex setup.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 define the called/calling individual-call through-connect signalling roles.
- EN 300 392-2 Annex D.4 gives the conservative direct individual-call setup sequence: called `D-CONNECT ACKNOWLEDGE`, L2 ACK, then caller `D-CONNECT`.
- EN 300 392-2 Annex D.5 warns that the faster alternative can lose first traffic if the called MS misses `D-CONNECT ACKNOWLEDGE`.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_duplex_u_connect_waits_for_called_l2_ack_before_caller_d_connect --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 72 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 157 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 10 passed.
- `cargo fmt --package tetra-entities --check` -> pass.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deploy/runtime:

- Not deployed in this entry. Runtime behavior was already on the Annex D.4 pending-ACK path; this patch adds duplex regression coverage.

Next non-repeating execution:

1. Continue MM/group-affiliation restart robustness audit against EN 300 392-2 clauses 16.4/16.8/16.10.
2. Then build locally and deploy the current batch for live private-call retest only after the full focused verification batch remains green.

## 2026-06-06 02:46 EEST - Annex D.4 directive revalidated without protocol changes

Component in simple technical terms:

- CMCE/CC private call is the control layer that prepares both radios before voice traffic starts.
- `D-CONNECT ACKNOWLEDGE` prepares the called radio; the L2 ACK proves that radio actually received that command.
- `D-CONNECT` to the caller is the authorization that lets the caller side start the connected private call path.

User directive checked:

- Conservative Motorola-like sequence remains enforced: `D-CONNECT ACKNOWLEDGE` to the called MS, wait for L2 ACK, then `D-CONNECT` to the calling MS.
- No protocol code was changed in this entry because the runtime path already uses `pending_individual_connect_acks` and `TxReporter` to block caller `D-CONNECT` until called-leg L2 ACK.

ETSI clause scope:

- EN 300 392-2 Annex D.4 describes the conservative direct individual-call setup method: called `D-CONNECT ACKNOWLEDGE`, L2 ACK, then caller `D-CONNECT`.
- EN 300 392-2 Annex D.5 describes the faster alternative and warns that first traffic can be missed if the called MS misses `D-CONNECT ACKNOWLEDGE`.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs u_connect_waits_for_called_l2_ack_before_caller_d_connect --locked` -> 2 passed.

Next non-repeating execution:

1. Patch MM restart/group-affiliation pending-report handling so incomplete group reports do not publish a stable `registered + no groups` state.
2. Verify against EN 300 392-2 clauses 16.4/16.8/16.10 and then rerun focused MM restart recovery tests.

## 2026-06-06 02:52 EEST - MM restart recovery no longer publishes incomplete No Group state

Component in simple technical terms:

- MM BS is the base-station mobility manager. It decides whether an ISSI is registered and which GSSI groups it may listen to.
- `D-LOCATION UPDATE COMMAND` with `group_identity_report=true` is the SwMI asking a terminal to refresh its group list after restart.
- `group report complete` is the explicit terminal signal that the requested group report is finished; without it, a group-less `DemandLocationUpdating` is not proof that the terminal has no groups.

Patch summary:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Added a narrow pending-report guard for `DemandLocationUpdating` responses that have no `group_identity_location_demand` and no `group_report_complete` while a SwMI-requested group report is pending.
  - MM still accepts the radio internally and keeps it addressable for follow-up PDUs.
  - Shared subscriber state and CMCE/Brew `Register` are deferred until a standardized completion arrives or cached groups are restored, avoiding a stable dashboard/CMCE `registered + No Group` state.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Updated restart-recovery follow-up tests so incomplete reports emit no stable Register.
  - Verified that a subsequent `U-ATTACH/DETACH GROUP IDENTITY` report completion publishes `Register` first, then `Affiliate`.
  - Kept explicit empty `group_report_complete` behavior authoritative: it still produces registered with no groups when the MS really reports no groups.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4 allows SwMI-initiated registration with `D-LOCATION UPDATE COMMAND` and defines the group-report expectation after `group_identity_report=true`.
- EN 300 392-2 clause 16.8.3 covers infrastructure-initiated group reporting via `U-ATTACH/DETACH GROUP IDENTITY`.
- EN 300 392-2 clause 16.10.27a defines the `group report complete` information element.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Verification:

- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` -> 33 passed.
- `cargo test -p tetra-entities --test test_mm_bs --locked` -> 144 passed.
- `cargo fmt --package tetra-entities --check` -> pass.
- `git diff --check` -> pass.
- `cargo check -p tetra-entities --locked` -> pass.

Deploy/runtime:

- Not deployed in this entry. Local code and tests are ready for the next controlled build/deploy batch.

Next non-repeating execution:

1. Continue group-call/PTT audio robustness audit across CMCE floor control and UMAC circuit media routing, especially Motorola MTP3550 repeated PTT static.
2. Before deployment, rerun focused CMCE/UMAC private and group call suites, then build locally and deploy only the resulting Nexus-BS release binary/config to `/home/<user>/nexus-bs`.

## 2026-06-06 02:58 EEST - Group-call first post-grant TCH/S preserved through hangtime

Component in simple technical terms:

- UMAC BS is the traffic scheduler between CMCE floor-control and LMAC voice bursts.
- Hangtime is the short receive-only state after `D-TX CEASED`, where the group call stays allocated but nobody may transmit until SwMI grants a new floor.
- TCH/S Block2 is the second half-slot speech payload; if it arrives in the same transition where `D-TX GRANTED` is being processed, dropping it can sound like accepted PTT with static/no voice.

Patch summary:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Generalized the short deferred UL media guard so group calls, not only private simplex, can retain valid TCH/S media received during hangtime when a matching `FloorGranted` immediately follows.
  - Added `deferred_during_hangtime` tracking so group floor grant preserves only the media from the newly granted source/timeslot.
  - Kept stale-media protection: `FloorReleased`, call end, circuit close, unmatched speaker, and no-grant hangtime still purge or expire deferred media.
  - Flushes pending media after both private and group floor grants, once hangtime has been cleared.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_group_floor_grant_preserves_first_hangtime_requester_raw_block2`.
  - The regression uses GSSI 226333 and Motorola-like ISSI 2260082: first speaker releases, 2260082 sends raw TCH/S Block2 just before `FloorGranted`, and the exact 216-bit speech half-slot must appear on DL after the grant.

ETSI clause scope:

- EN 300 392-2 clause 14.5.2.2.1 keeps group transmission permission under SwMI floor control.
- EN 300 392-2 clause 23.5 permits FACCH/STCH signalling and traffic operation on the assigned channel during floor control.
- EN 300 392-2 clause 23.8.5 requires TCH/S half-slot timing/content to be preserved once valid traffic is accepted.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs group_floor --locked` -> 6 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 10 passed.
- `cargo test -p tetra-entities --test test_umac_bs group_ul --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 71 passed.
- `cargo test -p tetra-entities --test test_lmac_bs tch_s --locked` -> 10 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group --locked` -> 69 passed.
- `cargo fmt --package tetra-entities --check` -> pass.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Deploy/runtime:

- Not deployed in this entry. The change is ready for the next controlled local build and live group-call retest.

Next non-repeating execution:

1. Run an integrated local verification batch that includes MM restart recovery, CMCE P2P/group, UMAC, LMAC, SDS/WAP smoke if time permits.
2. Build locally and deploy the tested Nexus-BS binary/config to the flat `/home/<user>/nexus-bs` layout, then restart the BS for live tests.

## 2026-06-06 03:19 EEST - Annex D.4 private-call setup sequence revalidated

Component in simple technical terms:

- CMCE BS is the call-control state machine for private calls.
- LLC/L2 acknowledged service is the delivery confirmation path for an addressed downlink PDU.
- UMAC is the traffic scheduler; it opens the assigned bearer and only routes the first simplex speaker after CMCE releases the floor.

Annex D.4 implementation status:

- Local ISSI-to-ISSI private call setup sends `D-CONNECT ACKNOWLEDGE` to the called MS first with `Layer2Service::Acknowledged`, channel allocation, and a `TxReporter`.
- CMCE stores this reporter in `pending_individual_connect_acks` and keeps caller `D-CONNECT` blocked until the reporter reaches L2 acknowledged state.
- If the called leg is lost or times out, CMCE retries the called `D-CONNECT ACKNOWLEDGE`; after retry exhaustion it releases setup and never authorizes the caller.
- Only after the called L2 ACK does CMCE complete the setup by sending caller `D-CONNECT`; for simplex private calls it also synchronizes the initial UMAC `FloorGranted` state so first speech has a valid traffic owner.
- Bypass audit: normal local private-call setup uses this path. Existing direct `D-CONNECT` builders outside this path are for group calls, echo service, or Brew/network-routed calls, not the Motorola-like local direct private setup path.

ETSI clause scope:

- EN 300 392-2 Annex D.4 describes the conservative direct individual-call setup where the BS sends `D-CONNECT ACK` to the called MS and waits for the called MS layer-2 acknowledgement before authorizing the calling MS with `D-CONNECT`.
- Annex D.5 is the faster alternative, but it can lose the first traffic if the called MS misses `D-CONNECT ACK`; we are keeping the conservative D.4 path for Motorola-like terminals.
- EN 300 392-2 clause 14.5.1.2.1 remains the private-call floor-control scope for simplex PTT after setup.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p_u_connect_waits_for_called_l2_ack_before_caller_d_connect --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p_called_d_connect_ack_l2_loss_never_authorizes_caller_before_release --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs simple_private_call_full_direct_setup_and_release_workflow --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 72 passed.
- `cargo test -p tetra-entities --test test_cmce_bs private --locked` -> 19 passed.

Deploy/runtime:

- No deploy was needed for this checkpoint because the requested Annex D.4 sequencing was already present in the current tree and had been deployed in the previous tested batch. If protocol code changes again, rebuild locally and redeploy the flat `/home/<user>/nexus-bs` binaries/config before live RF retest.

Next non-repeating execution:

1. Continue live RF validation for private simplex first-PTT audio and repeated same-speaker Motorola PTT on the currently deployed Annex D.4 build.
2. Continue group-call alternating PTT robustness validation and inspect logs only from the current restart window after each real RF test.
3. Finish the flat-folder `nexus-bs-control` operator wrapper mismatch so WAP/SDS test sending works from `/home/<user>/nexus-bs` with no subfolders.

## 2026-06-06 03:31 EEST - Annex D.4 Motorola-like private-call gate rechecked

Component in simple technical terms:

- CMCE is the call-control state machine that decides when the private-call caller and called MS may move to the assigned channel.
- LLC/L2 acknowledged service is the radio delivery confirmation path; CMCE must wait for it, not just for a local software queue.
- UMAC is the traffic bearer/floor owner; for simplex it must not release the first speaker before the called leg is confirmed.

Decision:

- Keep the conservative Annex D.4 sequence for local ISSI-to-ISSI private calls: send `D-CONNECT ACKNOWLEDGE` with channel allocation to the called MS, wait for L2 ACK, then send `D-CONNECT` to the caller.
- Keep Annex D.5 faster sequencing out of this path because Annex D.5 warns that the called MS may miss the first traffic if it misses `D-CONNECT ACK`.
- No code patch was required in this step because `CcBsSubentity::send_private_called_d_connect_ack_waiting_for_l2_ack` and `drain_pending_individual_connect_acks` already implement this gate in the current tree.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p_u_connect_waits_for_called_l2_ack_before_caller_d_connect --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p_called_d_connect_ack_l2_loss_never_authorizes_caller_before_release --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex_initial_floor_routes_without_extra_floor_grant --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p_duplex_u_connect_waits_for_called_l2_ack_before_caller_d_connect --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs simple_private_call_full_direct_setup_and_release_workflow --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p_hook_other_ms_initial_floor_precedes_caller_d_connect --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 72 passed.
- `git diff --check` -> pass.

## 2026-06-06 03:36 EEST - Annex D.4 private-simplex U-plane gate hardened

Component in simple technical terms:

- CMCE decides when the private caller is authorized by `D-CONNECT`.
- UMAC owns the actual traffic bearer and current simplex speaker.
- `CallControl::Open` now means "bearer exists"; `CallControl::FloorGranted` means "this ISSI may send voice".

Patch summary:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added a private-simplex guard for a participant-scoped bearer with no peer timeslot and no current speaker.
  - If UL speech arrives after `Open` but before `FloorGranted`, UMAC defers it briefly instead of routing it to DL.
  - Deferred media is kept while no authorized speaker exists, then flushed after `FloorGranted`; stale media still expires under the existing bounded guard.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Reworked the old "Open routes initial floor" test into `test_private_simplex_pre_floor_voice_waits_for_floor_granted`.
  - The test proves `Open` alone does not route private-simplex TCH/S, and the retained first burst routes after `FloorGranted`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_p2p_called_d_connect_ack_transmitted_without_l2_ack_does_not_authorize_caller`.
  - The test proves a merely transmitted called `D-CONNECT ACKNOWLEDGE` is not enough; caller `D-CONNECT` and UMAC floor stay blocked until L2 ACK.

ETSI clause scope:

- EN 300 392-2 Annex D.4: conservative individual direct setup waits for the called MS layer-2 acknowledgement to `D-CONNECT ACK` before authorizing the calling MS with `D-CONNECT`.
- EN 300 392-2 Annex D.5 remains deliberately not used here because the faster method can lose the first traffic if the called MS misses `D-CONNECT ACK`.
- EN 300 392-2 clause 14.5.1.2.1: private-simplex transmission is governed by SwMI floor control; UMAC must not treat bearer open as speaker authorization.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p_called_d_connect_ack_transmitted_without_l2_ack_does_not_authorize_caller --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex_pre_floor_voice_waits_for_floor_granted --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 73 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 10 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 71 passed.
- `cargo fmt --package tetra-entities --check` -> pass.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Rebuild locally and deploy the tested binary/config to the flat `/home/<user>/nexus-bs` layout.
2. Live RF retest: Hytera 2260616 -> Motorola 2260618 first private-simplex PTT should no longer leak or lose audio before the Annex D.4 gate completes.
3. Continue the separate audit for Brew/network-origin private-call paths; the current D.4 hardening is for local ISSI-to-ISSI setup.

## 2026-06-06 03:39 EEST - Annex D.4 U-plane gate deployed to test BS

Deploy summary:

- Built locally for AArch64 with the Nexus-BS v0.1.55 SoapySDR cross-build command from memory.
- Deployed directly, no backup binary, to `/home/chris/nexus-bs/nexus-bs`.
- Remote binary SHA-256: `4b8493e5135e462935a4248ff9c7c5a1042ad79abe75e65ed0b140d3d530b4fe`.
- Restarted `nexus-bs@chris.service`; `nexus-bs@chris.service` and `nexus-bs-control@chris.service` are active.

Post-start RF/runtime observations:

- Startup build line: `Nexus-BS v0.1.55`, build `v0.1.55-332fa519-modified`.
- Dashboard is listening on `http://0.0.0.0:8080`.
- Restart recovery is armed for `{2260082, 2260616, 2260618}`.
- After restart, local terminals registered and affiliated:
  - `2260618` registered and affiliated to `[226333]`.
  - `2260616` registered and affiliated to `[226333]`.
  - `2260082` registered and affiliated to `[226333]`; then soft re-attach preserved `[226333]`.

Next non-repeating execution:

1. User RF retest: private simplex `2260616 -> 2260618`, first PTT, then repeated PTT from the Motorola side.
2. If any first-PTT audio/beep/static issue remains, read only logs since `2026-06-06 03:38:51 EEST` and inspect CMCE `D-CONNECT ACK`/L2 ACK, UMAC `FloorGranted`, and TCH/S route events.

## 2026-06-06 03:49 EEST - Annex D.4 Brew/private local RF ACK gate

Component in simple technical terms:

- CMCE controls private-call signalling: `D-CONNECT ACKNOWLEDGE` tells the called-side terminal the call is connected, and `D-CONNECT` tells the caller side.
- MLE/LLC layer-2 ACK is the proof that the local terminal received the downlink connect PDU.
- UMAC owns the traffic bearer and only starts private-simplex voice for a speaker after CMCE emits `FloorGranted`.
- Brew is the IP/network side; for Brew-bridged private calls it must not receive media-ready/confirm before the local RF leg has acknowledged the connect PDU.

Patch summary:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/network.rs`
  - Changed Brew private-call local `D-CONNECT` and local `D-CONNECT ACKNOWLEDGE` deliveries from unacknowledged to `Layer2Service::Acknowledged` with `TxReporter`.
  - Added pending Brew-private connect state and a timer drain that completes only after local L2 ACK.
  - Delays UMAC `FloorGranted`, Brew `NetworkCircuitConnectConfirm`, and Brew `NetworkCircuitMediaReady` until the local RF leg is L2-acknowledged.
  - Releases the private call with `AcknowledgedServiceNotComplete` if the local RF connect PDU reaches a final failed state or times out.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated Brew local-origin and network-origin private-simplex tests so initial floor/media are expected only after local L2 ACK.
  - Added regression tests proving "transmitted but not L2-acknowledged" is not enough to open floor/media.

ETSI clause scope:

- EN 300 392-2 Annex D.4: conservative individual direct setup waits for the called MS layer-2 acknowledgement to `D-CONNECT ACK` before authorizing the calling MS with `D-CONNECT`.
- EN 300 392-2 Annex D.5: the faster alternative can lose initial traffic if the called MS misses `D-CONNECT ACK`; this patch deliberately follows the conservative D.4 gate for Motorola-like robustness.
- EN 300 392-2 clauses 14.5.1.1.1/14.5.1.1.2: `D-CONNECT ACKNOWLEDGE`/`D-CONNECT` carry the connect state, transmission grant, and channel allocation for private calls.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs local_origin_brew_private --locked` -> 3 passed.
- `cargo test -p tetra-entities --test test_cmce_bs network_origin_brew_private --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs network_origin_private --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 73 passed.
- `cargo fmt --package tetra-entities --check` -> pass.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Rebuild locally for AArch64 with the memory command and deploy directly to `/home/chris/nexus-bs/nexus-bs`.
2. Restart BS and live RF retest first-PTT private simplex on `2260616 -> 2260618`, plus reverse direction and Motorola-to-Motorola repeated PTT.
3. If first PTT still has beeps/no audio, inspect logs for local `D-CONNECT ACK`/`D-CONNECT` reporter state, actual L2 ACK arrival, and whether UMAC `FloorGranted` is emitted only after that ACK.

## 2026-06-06 08:15 EEST - Annex D.4 Brew/private gate deployed to Nexus-BS test service

Deploy summary:

- Built locally for AArch64 with the Nexus-BS v0.1.55 SoapySDR cross-build command from memory.
- Deployed directly, no backup binary, to `/home/chris/nexus-bs/nexus-bs`.
- Local and remote binary SHA-256: `652c441ef31670f5ac36ecc0c9ab53e97476f0822ea26ed1a5483815c1c25dab`.
- Restarted `nexus-bs@chris.service`; `nexus-bs@chris.service` and `nexus-bs-control@chris.service` are active.
- Dashboard is listening on `0.0.0.0:8080`; control service is listening on `127.0.0.1:9002`.

Post-start observations:

- Startup banner confirms `Nexus-BS v0.1.55`, build `v0.1.55-332fa519-modified`.
- Brew/TetraPack transport connected and backhaul reached `CONNECTED`.
- MM restart recovery armed only `{2260082}` in this run.
- `2260082` re-registered and affiliated to `[226333]`; BS then attempted Eg7 assignment but T352 expired and kept `StayAlive`.
- `2260616` and `2260618` did not appear in the bounded post-restart log window checked after deploy.
- No formal certification claim; this deploy only carries the clause-scoped Annex D.4/Brew-private hardening described above.

Next non-repeating execution:

1. User RF retest: `2260616 -> 2260618` private simplex first PTT, reverse direction, and Motorola-to-Motorola repeated PTT.
2. If `2260616`/`2260618` still show attached on terminal but not in BS logs, continue the restart-recovery/subscriber-state bug separately from the Annex D.4 first-PTT fix.
3. If first PTT still beeps/no audio, inspect logs since `2026-06-06 08:15:15 EEST` for CMCE connect reporter ACK state, UMAC `FloorGranted`, and TCH/S route timing.

## 2026-06-06 08:27 EEST - Annex D.4 conservative private-call sequence rechecked

Component in simple technical terms:

- CMCE is the call-control state machine for private/simplex and duplex calls.
- L2 ACK is the radio-layer proof that the called terminal received `D-CONNECT ACKNOWLEDGE`.
- UMAC floor control must not authorize first private-simplex voice before CMCE has that proof.

Finding:

- No new protocol patch was required in this step.
- Current `CcBsSubentity::send_private_called_d_connect_ack_waiting_for_l2_ack` sends `D-CONNECT ACKNOWLEDGE` to the called ISSI using `Layer2Service::Acknowledged` and a `TxReporter`.
- Current `drain_pending_individual_connect_acks` calls `complete_private_connect_after_called_ack` only when that reporter is L2-acknowledged.
- Current `complete_private_connect_after_called_ack` then emits the caller-side `D-CONNECT` and only then releases the initial private-simplex `FloorGranted`.

ETSI clause scope:

- EN 300 392-2 Annex D.4: for individual direct setup, the BS sends `D-CONNECT ACK` with channel allocation to the called MS, waits for layer-2 acknowledgement, then authorizes the calling MS with `D-CONNECT`.
- EN 300 392-2 Annex D.5 was rechecked as the faster alternative; it explicitly risks the called MS missing the first traffic if it misses `D-CONNECT ACK`, so Nexus-BS keeps the conservative D.4 path for Motorola-like robustness.
- This is clause-scoped engineering evidence only, not formal TETRA certification.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p_u_connect_waits_for_called_l2_ack_before_caller_d_connect --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p_called_d_connect_ack_transmitted_without_l2_ack_does_not_authorize_caller --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p_duplex_u_connect_waits_for_called_l2_ack_before_caller_d_connect --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex_pre_floor_voice_waits_for_floor_granted --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 73 passed.
- `cargo test -p tetra-entities --test test_cmce_bs simple_private_call_full_direct_setup_and_release_workflow --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p_hook_other_ms_initial_floor_precedes_caller_d_connect --locked` -> 1 passed.

Next non-repeating execution:

1. Live RF retest on the deployed binary: `2260616 -> 2260618`, called side ACK path, first PTT audio, then reverse/repeated Motorola PTT.
2. If beeps/no-audio remain, read logs since the latest service restart and correlate `D-CONNECT ACK` reporter ACK, caller `D-CONNECT`, UMAC `FloorGranted`, and first TCH/S route timing.
3. Continue separate hardening for long-running attach/group affiliation recovery and large-group floor robustness; do not mix those with the already verified Annex D.4 gate.

## 2026-06-06 08:33 EEST - P2P log audit and LLC late-BL-ACK grace deployed

Live log finding:

- Read the complete `nexus-bs@chris.service` journal from the previous restart at `2026-06-06 08:15:15 EEST`.
- Two local P2P setup failures were present:
  - `08:21:33` / `08:22:02`: `2260616 -> 2260618`, call_id `4`.
  - `08:22:12`: `2260618 -> 2260616`, call_id `5`.
- In both cases CMCE followed the Annex D.4 order: opened the local UMAC bearer, sent called-side `D-CONNECT ACKNOWLEDGE`, and did not send caller `D-CONNECT` before L2 ACK.
- The failure was in LLC acknowledged delivery: the called-side `D-CONNECT ACKNOWLEDGE` exhausted N.252 retransmissions before BL-ACK arrived.
- For call_id `5`, the terminal BL-ACK arrived after release as `received unexpected ACK for SSI 2260616 endpoint 0 N(R) 0` at `08:22:15.579`, proving the BS deleted the acknowledged-transfer context too early for this Motorola/Hytera direct-setup timing.

Component in simple technical terms:

- LLC is the layer that numbers acknowledged BL-DATA with `N(S)` and waits for a matching BL-ACK `N(R)`.
- CMCE depends on the LLC `TxReporter` to know when the called MS really received `D-CONNECT ACKNOWLEDGE`.
- A channel allocation PDU can move the MS to an assigned channel, so an ACK may arrive slightly after the normal retransmission-exhaustion edge.

Patch:

- `crates/tetra-entities/src/llc/llc_bs_ms.rs`
  - Added `t_retransmissions_exhausted` to retained acknowledged transfers.
  - Added a bounded late-ACK grace of 18 signalling frames only for retained TMA requests carrying `chan_alloc`.
  - During that grace, LLC does not queue more retransmissions and does not mark the service reporter lost.
  - If the matching BL-ACK arrives during the grace, the existing transfer is confirmed normally; if the grace expires, LLC reports failed transfer as before.
- `crates/tetra-entities/tests/test_llc_bs.rs`
  - Added `test_channel_allocation_t251_exhaustion_keeps_late_ack_grace`.
  - The regression proves a channel-allocation BL-DATA keeps original `N(S)` through N.252, avoids premature failed-transfer after exhaustion, and confirms on a late matching BL-ACK.

ETSI clause scope:

- EN 300 392-2 clause 22.3.2.3: acknowledged BL-DATA uses `N(S)`/`N(R)`, T.251, N.252, and BL-ACK confirmation/failure.
- EN 300 392-2 Annex A.1: T.251 is counted in downlink signalling frames and assigned-channel monitoring can constrain when the MS can receive/control-signal.
- EN 300 392-2 Annex D.4: direct individual setup with channel allocation waits for called-side L2 ACK before caller authorization; the BS can repeat `D-CONNECT ACK` and page/monitor the assigned slot until that L2 ACK is received.
- This is clause-scoped engineering evidence plus a bounded interoperability guard for channel-allocation ACK timing, not formal ETSI/TETRA certification.

Verification:

- `cargo test -p tetra-entities --test test_llc_bs channel_allocation_t251 --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_llc_bs t251_retransmission_exhaustion_emits_failed_transfer_report --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p_called_d_connect_ack_l2_loss_never_authorizes_caller_before_release --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 83 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 73 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 10 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo fmt --package tetra-entities --check` -> pass.
- `git diff --check` -> pass.

Deploy/runtime:

- Built locally only with the Nexus-BS AArch64 SoapySDR cross-build command.
- Deployed directly, no backup binary, to `/home/chris/nexus-bs/nexus-bs`.
- Local and remote binary SHA-256: `251a541966cdc15f6136f0e7829291b8e6b1dae882bb9b84553e7828f1eec652`.
- Restarted `nexus-bs@chris.service`; service is active from `2026-06-06 08:32:34 EEST`.
- `nexus-bs-control@chris.service` stayed active.
- Startup after deploy:
  - Dashboard listening on `0.0.0.0:8080`.
  - Brew/control transports connected.
  - Restart recovery armed `{2260082, 2260616, 2260618}`.
  - `2260618`, `2260082`, and `2260616` registered and affiliated to `[226333]` in the post-start window.

Next non-repeating execution:

1. Live RF retest on the deployed `08:32:34` build:
   - `2260616 -> 2260618` private simplex first PTT.
   - `2260618 -> 2260616` reverse private simplex.
   - Repeat PTT after call setup to verify no static/no-audio regression.
2. If P2P still fails, read logs only since `2026-06-06 08:32:34 EEST` and check for the new LLC line `retaining channel-allocation transfer for 18 signalling-frame late-ACK grace`, followed by BL-ACK success or grace expiry.
3. Continue separate group-call and long-running affiliation robustness validation after the private setup ACK timing is confirmed live.

## 2026-06-06 08:58 EEST - P2P Annex D.4 circular ACK path patched, pending deploy

Live failure after the 08:32 deploy:

- User retested P2P and reported failure.
- Journal for `2260616 -> 2260618` call_id `4/5` showed CMCE opening TS2 and sending called-side `D-CONNECT ACKNOWLEDGE`, but no useful called BL-ACK reached LLC.
- The earlier LLC late-ACK grace was active, but it could not help because the ACK was not delivered upward.

Root cause:

- UMAC opened the same-timeslot private simplex bearer with no current UL speaker, correctly blocking U-plane voice until `FloorGranted`.
- The called MS BL-ACK for `D-CONNECT ACKNOWLEDGE` arrives as addressless MAC-U-SIGNAL on STCH.
- UMAC was using the same `current_ul_speaker` guard for STCH signalling, so the called BL-ACK was dropped before LLC could confirm the pending `D-CONNECT ACK`.
- That created a circular dependency: CMCE waits for BL-ACK before `FloorGranted`, while UMAC required `FloorGranted` before forwarding the BL-ACK.

Patch:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added a pre-floor private-simplex exception only for LLC ACK responses (`BL-ACK`, `BL-ACK-FCS`, `BL-ADATA`, `BL-ADATA-FCS`).
  - Before `FloorGranted`, addressless STCH ACKs are attributed to the private circuit primary ISSI, which CMCE already seeds as the called MS for Annex D.4 called-leg confirmation.
  - Generic MAC-U-SIGNAL and voice remain blocked until `FloorGranted`.
  - Tightened direct CMCE payload detection so a short LLC ACK is not misclassified as raw CMCE.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added UL circuit primary address accessor.
  - Added higher STCH priority for CMCE channel-allocation payloads over ACK-only channel-allocation STCH.
  - This makes setup-critical called `D-CONNECT ACKNOWLEDGE` preempt the 5-bit ACK-only FACCH block seen in the log.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Called-side `D-CONNECT ACKNOWLEDGE` with channel allocation now requests FACCH/STCH delivery for the assigned-channel leg while still waiting for L2 ACK before caller authorization.
- Tests:
  - Added UMAC regression for called BL-ACK before `FloorGranted`.
  - Added scheduler regression for `D-CONNECT ACKNOWLEDGE` STCH priority over ACK-only FACCH.
  - Updated CMCE p2p expectations for called `D-CONNECT ACKNOWLEDGE` FACCH/STCH delivery.

ETSI clause scope:

- EN 300 392-2 Annex D.4: direct private call setup sends `D-CONNECT ACK` with channel allocation to the called MS and waits for called L2 ACK on the assigned channel before caller `D-CONNECT`.
- EN 300 392-2 clauses 21.4.5 and 23.8.4.1.4: MAC-U-SIGNAL on STCH carries TM-SDU signalling without an SSI field, so the BS must bind it to the assigned-channel call context.
- EN 300 392-2 clause 22.3.2.3: BL-ACK/BL-ADATA confirm acknowledged BL-DATA and must reach LLC with the correct basic-link address.
- This is clause-scoped hardening evidence, not formal certification.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs stch_bl_ack_before_private_floor_granted --locked` -> 1 passed.
- `cargo test -p tetra-entities --lib private_called_d_connect_ack_stch_preempts_ack_only_facch --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 10 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 73 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 83 passed.
- `cargo test -p tetra-entities --test test_umac_bs stch --locked` -> 12 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo fmt --package tetra-entities --check` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Build locally for AArch64 with the Nexus-BS SoapySDR command.
2. Deploy directly to `/home/chris/nexus-bs/nexus-bs` with no backup binary and restart `nexus-bs@chris.service`.
3. Live retest:
   - `2260616 -> 2260618`, first private simplex PTT.
   - `2260618 -> 2260616`, reverse private simplex.
   - Check logs for called `D-CONNECT ACK L2 ACK received`, caller `D-CONNECT`, `FloorGranted`, and no retry exhaustion.

## 2026-06-06 09:05 EEST - P2P Annex D.4 ACK-path build deployed to live test BS

Live failure confirmation before deploy:

- User reported the latest P2P test still failed.
- Logs since the `08:32:34 EEST` restart showed `2260616 -> 2260618`, `call_id=4`.
- CMCE opened the private simplex bearer on TS2 and sent called-side `D-CONNECT ACKNOWLEDGE`, then waited for the called MS L2 ACK before caller `D-CONNECT`.
- LLC exhausted retransmissions for SSI `2260618`; CMCE retried the called `D-CONNECT ACKNOWLEDGE` three times and released with `AcknowledgedServiceNotComplete`.
- This matched the local root cause already patched: pre-floor private-simplex addressless STCH BL-ACK could be dropped before LLC saw it.

Verification before deploy:

- `cargo test -p tetra-entities --test test_umac_bs stch_bl_ack_before_private_floor_granted --locked` -> pass.
- `cargo test -p tetra-entities --lib private_called_d_connect_ack_stch_preempts_ack_only_facch --locked` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> pass.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> pass.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> pass.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo fmt --package tetra-entities --check` -> pass.
- `git diff --check` -> pass.

Deploy:

- Built locally only with the Nexus-BS AArch64 SoapySDR cross-build command.
- Deployed directly, no backup binary, to `/home/chris/nexus-bs/nexus-bs`.
- Local and remote binary SHA-256: `f55fc896a287ee7e2f77ffef4dcdd91c6eb477ede37eda9f88a1d1d10e5526ed`.
- Restarted `nexus-bs@chris.service`; service active from `2026-06-06 09:02:42 EEST`.
- Dashboard listening on `0.0.0.0:8080`.
- Post-restart recovery registered and affiliated:
  - `2260082 -> [226333]`
  - `2260618 -> [226333]`
  - `2260616 -> [226333]`
- Updated `scripts/nexus-bs-test-deploy.sh` to deploy future builds to the flat final folder `/home/chris/nexus-bs/nexus-bs` and restart `nexus-bs@chris.service` instead of using the legacy test subfolder/pidfile flow.

Next non-repeating execution:

1. Live RF retest on the `09:02:42 EEST` build:
   - `2260616 -> 2260618`, private simplex first PTT.
   - `2260618 -> 2260616`, reverse private simplex first PTT.
   - repeat PTT after setup in both directions.
2. For each P2P attempt, check logs since `2026-06-06 09:02:42 EEST` for:
   - `CMCE: called D-CONNECT ACK L2 ACK received`;
   - caller-side `D-CONNECT`;
   - `CallControl::FloorGranted` / `UMAC floor granted`;
   - no `called D-CONNECT ACK did not receive L2 ACK`;
   - no `AcknowledgedServiceNotComplete`.

## 2026-06-06 09:31 EEST - P2P Annex D.4 caller D-CONNECT ACK gate patched, pending deploy

Latest live RF report:

- User reported another private simplex P2P failure after the `09:02:42 EEST` deploy.
- Full journal since the `09:02:42 EEST` restart was read. The service banner still showed build `v0.1.55-332fa519-modified`; this was the earlier ACK-path build, not the new caller-side ACK gate.
- No formal certification claim. This entry is clause-scoped hardening evidence only.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Split accepted local private-call setup into two L2 ACK gates:
    1. send called MS `D-CONNECT ACKNOWLEDGE` with channel allocation;
    2. wait called MS L2 ACK;
    3. send caller MS `D-CONNECT` with channel allocation;
    4. wait caller MS L2 ACK;
    5. only then activate the individual call and emit initial private-simplex `FloorGranted` to UMAC.
  - Added retry/guard handling for both called and caller connect messages.
  - Duplicate `U-CONNECT` is ignored while either connect ACK gate is pending.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/mod.rs`
  - Added pending caller-connect state.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Initializes and cleans up pending caller-connect state during release/deregistration paths.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Drains pending caller-connect ACK state from the CMCE timer tick.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated P2P helpers and full setup/release workflow so an established private call means both called and caller connect messages were L2-acknowledged.
  - Added/kept regressions proving caller `D-CONNECT` precedes initial floor and that a transmitted but unacknowledged caller `D-CONNECT` does not seed U-plane audio.

Technical explanation:

- CMCE is the call-control layer. It decides when a private call is actually connected.
- UMAC is the radio bearer/floor layer. It should not pass private-simplex voice until CMCE says which ISSI owns the floor.
- The field symptom `first PTT opens the private screen but no voice/beep` matches a race where UMAC floor could start before the caller radio had confirmed `D-CONNECT`.
- This patch keeps the traffic bearer ready, but blocks the initial U-plane floor until both ETSI call-control connect legs have been confirmed at L2.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1, 14.5.1.1.2 and 14.5.1.2.1: private call setup/acceptance and simple private-call transmission grant semantics.
- EN 300 392-2 Annex D.4: conservative direct private-call sequence with called-side `D-CONNECT ACKNOWLEDGE` before caller `D-CONNECT`.
- EN 300 392-2 clause 22.3.2.3: acknowledged downlink delivery must be confirmed by L2 before treating the transfer as complete.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 74 passed.
- `cargo test -p tetra-entities --test test_cmce_bs group --locked` -> 69 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 161 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 10 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo fmt --package tetra-entities --check` -> pass.
- `git diff --check` -> pass.

Next non-repeating execution:

1. Deploy direct to `/home/chris/nexus-bs/nexus-bs` with `scripts/nexus-bs-test-deploy.sh`; build locally only, no Pi compile, no backup binary.
2. Record local and remote SHA-256 plus new `ActiveEnterTimestamp`.
3. Confirm the startup log contains the new build and the service is `active`.
4. Live RF retest:
   - `2260616 -> 2260618`, private simplex first PTT.
   - `2260618 -> 2260616`, reverse private simplex first PTT.
   - repeated PTT in both directions.
   - group `226333` alternating PTT after private retest.
5. For every P2P attempt, inspect logs for:
   - called `D-CONNECT ACK L2 ACK received`;
   - caller `D-CONNECT L2 ACK received`;
   - `FloorGranted` only after caller ACK;
   - no connect ACK retry exhaustion;
   - no `AcknowledgedServiceNotComplete`;
   - no static-only audio after floor grant.

## 2026-06-06 09:34 EEST - P2P caller D-CONNECT ACK gate deployed to live BS

Deploy:

- Built locally only with the Nexus-BS AArch64 SoapySDR cross-build command.
- Deployed directly, no backup binary, to `/home/chris/nexus-bs/nexus-bs`.
- Local/remote binary SHA-256 verified by deploy script:
  - `24a9335aa0c54eb8e6edc3068a821801154212ffe6f8dfce5eb08a211c389939`
- Restarted `nexus-bs@chris.service`.
- Service state after deploy:
  - `ActiveState=active`
  - `SubState=running`
  - `MainPID=45745`
  - `ActiveEnterTimestamp=Sat 2026-06-06 09:33:01 EEST`
- Startup banner still shows `Build: v0.1.55-332fa519-modified` because this is the same git commit with a dirty worktree; the binary SHA above is the deploy identity for this stage.
- Dashboard is listening on `0.0.0.0:8080`; Brew/control transports connected.

Post-restart affiliation evidence:

- `2260616` restored cached restart group affiliation and CMCE affiliated it to `[226333]`.
- `2260082` registered and CMCE affiliated it to `[226333]`.
- `2260618` registered and CMCE affiliated it to `[226333]`.
- EG7 allocation is active in config for these terminals during this test window.

Immediate RF validation required:

1. `2260616 -> 2260618`, private simplex first PTT.
2. `2260618 -> 2260616`, reverse private simplex first PTT.
3. Repeat PTT both directions after call setup.
4. Then group `226333` alternating PTT.
5. If P2P still fails, inspect logs since `2026-06-06 09:33:01 EEST` for:
   - whether called `D-CONNECT ACKNOWLEDGE` got L2 ACK;
   - whether caller `D-CONNECT` got L2 ACK;
   - whether `FloorGranted` occurs only after caller ACK;
   - whether static/no-voice happens after both ACK gates are complete, which would move root cause from CMCE setup into UMAC/LMAC media routing.

## 2026-06-06 09:38 EEST - P2P caller ACK gate removed; Annex D.4 scope corrected, pending redeploy

Correction:

- The `09:33:01 EEST` binary is superseded before RF validation.
- Agent CMCE review caught that waiting for caller `D-CONNECT` L2 ACK before initial `FloorGranted` is not required by EN 300 392-2 Annex D.4 and can recreate the exact first-PTT/no-audio race:
  - the caller MS may receive `D-CONNECT` and consider the private call ready;
  - CMCE/UMAC would still suppress U-plane until the caller ACK reporter flipped.
- Correct clause-scoped behavior for local direct private call:
  1. called MS receives `D-CONNECT ACKNOWLEDGE` with channel allocation;
  2. CMCE waits for called MS L2 ACK;
  3. CMCE queues caller MS `D-CONNECT` with channel allocation;
  4. CMCE immediately activates the individual call and emits initial `FloorGranted` after caller `D-CONNECT` is queued, without waiting for caller L2 ACK.
- Caller `D-CONNECT` still uses `Layer2Service::Acknowledged`; LLC may retransmit it, but CMCE does not hold the first private-simplex floor on the caller ACK.

Patch delta:

- Removed `pending_individual_caller_connects` state and timer drain.
- `complete_private_connect_after_called_ack()` now queues caller `D-CONNECT` and immediately enables initial private U-plane.
- Tests now assert:
  - caller `D-CONNECT` is blocked until called `D-CONNECT ACKNOWLEDGE` is L2-acknowledged;
  - `FloorGranted` follows caller `D-CONNECT` in the same post-called-ACK phase;
  - a later caller `D-CONNECT` L2 ACK does not emit duplicate floor.

Verification after correction:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 74 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 161 passed.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` -> 10 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `cargo fmt --package tetra-entities --check` -> pass.
- `git diff --check` -> pass.

Next:

- Redeploy immediately; do not RF-test the `09:33:01 EEST` binary.
- New RF test target will be the next restart timestamp and SHA.

## 2026-06-06 09:40 EEST - Corrected Annex D.4 P2P setup deployed to live BS

Deploy:

- Built locally only with the Nexus-BS AArch64 SoapySDR cross-build command.
- Deployed directly, no backup binary, to `/home/chris/nexus-bs/nexus-bs`.
- Local/remote binary SHA-256 verified by deploy script:
  - `45ef1535c2bec273291117fe52a808ee5e96cb920b112a5765bf308d1e83e552`
- Restarted `nexus-bs@chris.service`.
- Service state after deploy:
  - `ActiveState=active`
  - `SubState=running`
  - `MainPID=45915`
  - `ActiveEnterTimestamp=Sat 2026-06-06 09:39:35 EEST`
- Startup banner still shows `Build: v0.1.55-332fa519-modified`; use the SHA above to identify this corrected deploy.

Post-restart affiliation evidence:

- `2260616` restored cached restart group affiliation and CMCE affiliated it to `[226333]`.
- `2260082` registered and CMCE affiliated it to `[226333]`.
- `2260618` registered and CMCE affiliated it to `[226333]`.
- EG7 allocation is active in config for this test window.

RF validation target:

1. Test `2260616 -> 2260618` private simplex first PTT.
2. Test `2260618 -> 2260616` reverse private simplex first PTT.
3. Repeat PTT both directions.
4. Then test group `226333` alternating PTT.
5. If P2P fails, inspect logs since `2026-06-06 09:39:35 EEST` for the corrected sequence:
   - called `D-CONNECT ACK L2 ACK received`;
   - caller `D-CONNECT queued`;
   - `enabling initial private U-plane without waiting for caller L2 ACK`;
   - initial `FloorGranted`;
   - no `called D-CONNECT ACK did not receive L2 ACK`;
   - no `AcknowledgedServiceNotComplete`.

## 2026-06-06 09:53 EEST - P2P RF fail follow-up; UMAC direct D-TX CEASED classifier fixed, pending deploy

Trigger:

- User reported a fresh P2P RF test failed on the `09:39:35 EEST` live build.
- Full journal from the latest restart showed attach/affiliation and SDS/Brew activity, but no clearly prefixed P2P CMCE setup lines in the visible window.

Findings:

- CMCE direct private setup already follows the corrected EN 300 392-2 Annex D.4 sequence:
  1. called MS receives `D-CONNECT ACKNOWLEDGE` with channel allocation;
  2. CMCE waits for called-leg L2 ACK;
  3. CMCE queues caller `D-CONNECT`;
  4. CMCE emits initial private-simplex `FloorGranted` immediately after caller `D-CONNECT` is queued, without waiting for caller L2 ACK.
- The accepted local P2P setup log previously started with `rx_u_setup_p2p`, not `CMCE:`, and inbound `U-SETUP` was only DEBUG/TRACE. That made journal diagnosis too weak.
- UMAC verification exposed a real floor-control priority bug:
  - direct CMCE `D-TX CEASED` has 5-bit type `01001`;
  - its first 4 bits are `0100`, which collide with LLC `BL-ADATA-FCS`;
  - `cmce_dl_payload_from_tma_sdu()` rejected the direct floor-withdrawal SDU as LLC before checking the 5-bit CMCE floor-control PDU type;
  - `D-TX CEASED` was therefore classified as `Ordinary` instead of `FloorWithdraw` under TMA pending-report pressure.
- Clause scope: EN 300 392-2 clause 14.5.2.2.1 floor control, clause 23.5 STCH/FACCH delivery, and downlink CMCE floor-control PDUs `D-TX CEASED`, `D-TX GRANTED`, `D-TX INTERRUPT`.

Patch:

- `umac_bs.rs` TMA admission now accepts direct 5-bit CMCE `D-TX GRANTED`, `D-TX CEASED`, and `D-TX INTERRUPT` before applying the 4-bit LLC ACK/data rejection.
- `bs_sched.rs` scheduler backpressure classification uses the same direct floor-control exception.
- CMCE logging now emits:
  - `CMCE: <- U-SETUP ...`
  - `CMCE: rx_u_setup_p2p ...`
  - `CMCE: initial private-simplex FloorGranted ...`

Verification:

- `cargo fmt --package tetra-entities --check` -> pass.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 74 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 74 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 161 passed.
- `cargo check -p tetra-entities --locked` -> pass.
- `git diff --check` -> pass.

Next:

- Deploy this stage directly to `/home/chris/nexus-bs/nexus-bs` with no backup binary.
- After restart, RF test:
  1. `2260616 -> 2260618` private simplex first PTT and repeated PTT;
  2. `2260618 -> 2260616` reverse private simplex first PTT and repeated PTT;
  3. `226333` group alternating PTT, including repeated `D-TX CEASED` / handoff.
- If P2P still fails, inspect new logs for `CMCE: <- U-SETUP`, `CMCE: rx_u_setup_p2p`, called `D-CONNECT ACK L2 ACK received`, caller `D-CONNECT queued`, and `CMCE: initial private-simplex FloorGranted`.

## 2026-06-06 09:56 EEST - UMAC floor-control fix deployed to live BS

Deploy:

- Built locally only with `scripts/nexus-bs-test-deploy.sh`; no Pi build.
- Deployed directly, no backup binary, to `/home/chris/nexus-bs/nexus-bs`.
- Remote binary SHA-256:
  - `8b996549b57bf5a35c1c94d0ded1b27c68e13afbdc960de6a00f74d066a11579`
- Restarted `nexus-bs@chris.service`.
- Service state after deploy:
  - `ActiveState=active`
  - `SubState=running`
  - `MainPID=46126`
  - `ActiveEnterTimestamp=Sat 2026-06-06 09:54:18 EEST`
- Startup banner still shows `Build: v0.1.55-332fa519-modified`; use the SHA above and restart timestamp to identify this stage.

Post-restart evidence:

- `2260616` restored cached restart group affiliation and CMCE affiliated it to `[226333]`.
- `2260082` registered and CMCE affiliated it to `[226333]`.
- `2260618` registered and CMCE affiliated it to `[226333]`.
- BS-initiated EG7 assignment timed out on all three terminals:
  - `2260616`: T352 expired at `09:54:49.585`, kept `StayAlive`.
  - `2260082`: T352 expired at `09:54:49.981`, kept `StayAlive`.
  - `2260618`: T352 expired at `09:54:51.115`, kept `StayAlive`.
- For immediate P2P/group RF validation, terminals are therefore effectively StayAlive rather than active EG7.

RF validation target:

1. Test `2260616 -> 2260618` private simplex first PTT and repeated PTT.
2. Test `2260618 -> 2260616` reverse private simplex first PTT and repeated PTT.
3. Test `2260082 -> 2260618` repeated Motorola-to-Motorola PTT.
4. Test group `226333` alternating PTT, including `2260082` repeated entries.
5. If any call fails, inspect only logs since `2026-06-06 09:54:18 EEST` and correlate:
   - `CMCE: <- U-SETUP`
   - `CMCE: rx_u_setup_p2p`
   - called `D-CONNECT ACK L2 ACK received`
   - caller `D-CONNECT queued`
   - `CMCE: initial private-simplex FloorGranted`
   - `D-TX CEASED` / `D-TX GRANTED` TMA reports and UMAC floor handoff.

## 2026-06-06 10:11 EEST - P2P 2260082 -> 2260618 called-leg ACK failure triage

Trigger:

- Live RF private simplex `2260082 -> 2260618` failed on the deployed stage from `2026-06-06 09:54:18 EEST`.
- Log window `09:59:39..09:59:53 EEST` shows two setup attempts:
  - `CMCE: <- U-SETUP from ISSI 2260082 called_party=Some(2260618) comm_type=P2p simplex=true hook=false priority=0`.
  - CMCE opened a shared private simplex bearer on `ts=2` for `2260618` and `2260082`.
  - CMCE sent called-leg `D-CONNECT ACKNOWLEDGE` to `2260618` with `TransmissionGrant::GrantedToOtherUser`.
  - LLC retransmitted the acknowledged transfer to `2260618` until exhaustion.
  - CMCE never observed the called-leg L2 ACK and released with `DisconnectCause::AcknowledgedServiceNotComplete`.

ETSI basis checked:

- EN 300 392-2 clause 14.5.1.1.1: called MS direct setup answers with `U-CONNECT` and expects `D-CONNECT ACKNOWLEDGE`; that PDU carries transmit permission.
- EN 300 392-2 clause 14.5.1.1.2: caller receives `D-CONNECT`; that PDU carries transmit permission.
- EN 300 392-2 clause 14.5.1.2.1: SwMI controls simplex transmit permission; normal direct setup grants the caller first unless the setup requests otherwise.
- EN 300 392-2 clause 14.5.1.4.1: `D-CONNECT` / `D-CONNECT ACKNOWLEDGE` grant values switch U-plane on for transmit or receive.
- EN 300 392-2 clause 14.5.3.1: late individual assignment may indicate traffic channels with `D-CONNECT` / `D-CONNECT ACKNOWLEDGE`; early/medium assigned channels start as FACCH until U-plane is switched on, and late assignment may switch U-plane when moving.
- EN 300 392-2 clause 23.8.2.2: if BS does not receive the expected L2 ACK for a message giving/withdrawing traffic receive authorization, it cannot know whether the MS is in signalling or traffic mode; retransmission must be interpretable in that ambiguity.
- EN 300 392-2 Annex D is informative. D.4 is still the conservative compatibility pattern: called `D-CONNECT ACK` with channel allocation first, wait for called L2 ACK, then authorize caller. D.5 is the faster path and explicitly risks first traffic loss if the called MS misses `D-CONNECT ACK`.

Swarm read-only audit result:

- ETSI agent: Annex D is informative, not mandatory, but D.4-style sequencing is a conservative terminal-compatible baseline.
- CMCE architect: current STCH-only called `D-CONNECT ACKNOWLEDGE` before caller authorization is likely wrong for the RF failure; first authoritative called `D-CONNECT ACKNOWLEDGE` should be MCCH/SCH-F with channel allocation, with D.4-style dual recovery on retry if needed.
- UMAC/LLC architect: the earlier suspected STCH speaker circular dependency is mostly refuted in current code because pre-floor ACK-carrying LLC PDUs have a narrow exception; remaining risks are wrong endpoint correlation and using the wrong MAC form for C-plane ACK tests.
- QA: add failing tests before any behavior change; do not patch from RF observation alone.

Patch gate:

- Do not change caller `D-CONNECT` / initial `FloorGranted` ordering until a test proves a concrete defect there.
- First local failing test target: called `D-CONNECT ACKNOWLEDGE` with channel allocation must be deliverable on a pre-traffic signalling path, not STCH-only, while caller `D-CONNECT` stays blocked until the called-leg L2 ACK.
- Secondary local failing test target: if an assigned-channel L2 ACK is received before `FloorGranted`, UMAC/LLC must attribute it to called ISSI `2260618` and the same endpoint used by the pending acknowledged transfer.
- Allowed behavior patch scope after failing test: CMCE/MLE/UMAC routing of called `D-CONNECT ACKNOWLEDGE` and its L2 ACK correlation only.
- This is clause-scoped engineering evidence, not formal ETSI/TETRA certification.

Next:

1. Add a focused failing CMCE/UMAC test for MCCH/SCH-F first delivery of called `D-CONNECT ACKNOWLEDGE` with channel allocation and no premature caller authorization.
2. Add or adjust an ACK-correlation test for pre-floor assigned-channel ACK from `2260618`, ensuring endpoint and LLC `N(R)` match the pending called-leg reporter.
3. Patch only the proven layer.
4. Verify with focused CMCE/UMAC/LLC tests, `cargo check -p tetra-entities --locked`, `cargo fmt --package tetra-entities --check`, and `git diff --check`.
5. Deploy locally built binary only, then RF retest `2260082 -> 2260618` first PTT and repeated PTT.

## 2026-06-06 10:54 EEST - P2P delayed U-CONNECT root cause: stale timed-out TMA request cancelled

Trigger:

- Live RF private simplex `2260082 -> 2260618` was tested after the current-channel setup patch.
- Log evidence after restart `2026-06-06 10:36:50 EEST`:
  - `10:41:13.397`: `U-SETUP` from `2260082` to `2260618`, `call_id=4`.
  - No immediate `U-CONNECT`, `D-CONNECT ACKNOWLEDGE`, caller `D-CONNECT`, or initial `FloorGranted`.
  - `10:41:43/44`: `UMAC: TMA report req_handle=18/19 timed out` for `addr=ISSI:2260618`, `cmce_pdu=Some(DSetup)`.
  - `10:42:14/45`: repeated timeouts for the same D-SETUP retry path.
  - `10:42:55/56`: `U-CONNECT for unknown call_id=4`, proving the called MS reacted after CMCE had already released the setup state.

Confirmed cause:

- UMAC `emit_completed_tma_reports()` reported `TmaReport::FragmentationFailure` after the local 30 s retained-report guard, but did not cancel the matching queued scheduler element.
- A timed-out D-SETUP could therefore still be transmitted later on SCH/F, causing delayed terminal reaction to an already dead CMCE call.
- This is an internal MAC consistency bug, not a vendor-specific call-control workaround.

ETSI basis checked:

- EN 300 392-2 clause 20.4.1.1.3: MAC reports complete or failed TMA-UNITDATA transfer to the upper layer using TMA-REPORT.
- EN 300 392-2 clause 22.3.2.3: LLC/upper retry/failure handling depends on that MAC transfer result.
- Once the local MAC reports the request failed, the same TM-SDU must not remain queued for later RF transmission under the same request context.

Patch:

- File: `crates/tetra-entities/src/umac/umac_bs.rs`.
- On pending TMA report timeout, UMAC now calls `channel_scheduler.dl_cancel_by_reporter(&report.tx_reporter)` before emitting `FragmentationFailure`.
- Timeout logs now include `cancelled_queued=N`.
- Added debug-only test helpers for retained-report timeout simulation.

Focused test:

- File: `crates/tetra-entities/tests/test_umac_bs.rs`.
- Added `test_tma_report_timeout_cancels_queued_mac_resource`.
- The test forces a retained TMA request for ISSI `2260618` past the local guard, verifies `FragmentationFailure`, then advances UMAC and asserts no later MAC-RESOURCE for `2260618` appears.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs test_tma_report_timeout_cancels_queued_mac_resource --locked` passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` passed: 75 tests.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_pending_setup_retry_is_reporter_throttled_before_timeout --locked` passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check -- crates/tetra-entities/src/umac/umac_bs.rs crates/tetra-entities/tests/test_umac_bs.rs` passed.

Next RF gate:

1. Deploy locally built `nexus-bs` to `/home/chris/nexus-bs/nexus-bs`.
2. Restart `nexus-bs@chris.service` and clear/vacuum volatile journal for a clean RF window.
3. Ask user to perform one controlled P2P simplex call `2260082 -> 2260618`: first PTT short voice, wait, repeated PTT, close.
4. Inspect logs specifically for `DSetup` timeout absence, prompt `U-CONNECT`, called `D-CONNECT ACK` L2 ACK, caller `D-CONNECT`, and initial `FloorGranted`.

## 2026-06-06 10:57 EEST - P2P stale-TMA fix deployed, RF window opened

Deploy:

- Ran `RUN_TESTS=1 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Local verification inside deploy passed:
  - `test_cmce_bs repeated_group_u_setup`: 3 passed.
  - `test_cmce_bs`: 161 passed.
  - `test_umac_bs`: 75 passed, including `test_tma_report_timeout_cancels_queued_mac_resource`.
  - `test_mm_bs restart_recovery`: 33 passed.
  - `cargo check -p tetra-entities --locked`: passed.
  - local release build for `nexus-bs v0.1.55`: passed.
- Deployed binary SHA-256: `b75b58f99cade0a31485c381b3a121bae0963c3911413767b35768896992a5ba`.
- Service: `nexus-bs@chris.service`.
- Restart timestamp: `Sat 2026-06-06 10:56:35 EEST`.
- PID after restart: `46927`.

Post-start attach state before RF:

- `2260616`: T352 expired for BS-initiated EG assignment at `10:57:06`, kept `StayAlive`.
- `2260618`: T352 expired for BS-initiated EG assignment at `10:57:08`, kept `StayAlive`.
- `2260082`: T352 expired for BS-initiated EG assignment at `10:57:09`, kept `StayAlive`.

RF test gate:

- Volatile journal rotated/vacuumed again at `2026-06-06 10:57:45 EEST`.
- User instructed to perform controlled private simplex `2260082 -> 2260618`, first PTT short voice, second PTT short voice, normal close.
- Next read should inspect journal since `2026-06-06 10:57:45 EEST`.

## 2026-06-06 11:18 EEST - P2P D-SETUP EG7 priority and duplicate setup suppression

Trigger:

- User ran clean RF P2P test after journal cleanup at `2026-06-06 11:04:32 EEST`; result: failed.
- Log evidence:
  - `11:04:41.849`: `U-SETUP` private simplex `2260082 -> 2260618`, `call_id=6`.
  - No `U-CONNECT`, `D-CONNECT ACKNOWLEDGE`, caller `D-CONNECT`, or floor setup.
  - `11:05:11`: user disconnected; CMCE emitted setup-phase `D-RELEASE` repeats because no traffic circuit existed.
  - `11:05:12`: two `DSetup` TMA requests to ISSI `2260618` timed out with `cancelled_queued=1`.

Confirmed cause:

- The stale queued timeout bug was fixed, but the live setup still failed because the P2P `D-SETUP` was a normal MCCH/SCH-F TMA request with `chan_alloc=[none]`.
- UMAC classified that non-stealing/no-chan-alloc `D-SETUP` as `Ordinary` even though `cmce_pdu=Some(DSetup)`.
- Under EG7, `elem_is_ready_for_tx()` correctly waits for the called MS receive window, but at that window ordinary backlog/fragments could still be selected before the call setup.
- CMCE also had two local ways to produce duplicate pending private `D-SETUP`: the generic `CircuitMgr` backup path and the dedicated EE retry path. The initial D-SETUP had no reporter in `cached.last_resend_reporter`, so retries did not know the first lower-layer delivery was still pending.

ETSI basis checked:

- EN 300 392-2 clause 14.5.1.1.1: the SwMI sends `D-SETUP` to the called MS; `U-CONNECT` cannot occur before the called MS receives setup.
- EN 300 392-2 clause 14.5.1.1.2: caller-side progress follows the called-side response.
- EN 300 392-2 clause 20.4.1.1.3: MAC reports TMA transfer completion/failure via TMA-REPORT.
- EN 300 392-2 clauses 23.5.2.2.7 and 23.7.6: downlink must account for energy economy receive opportunities. The patch does not bypass EG gating; it only prioritizes call-control once the addressed MS can listen.

Patch:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added `TmaAdmissionPriority::CallControl`.
  - Non-stealing CMCE `D-SETUP`, `D-RELEASE`, `D-CONNECT`, `D-CONNECT ACKNOWLEDGE`, etc. without channel allocation are no longer admitted as ordinary SDS/data.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Added scheduler recognition for LLC-wrapped CMCE call-control resources.
  - Ready call-control resources are selected after grants/ACKs/channel-allocation traffic but before ordinary resources and fragment backlog.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Initial private `D-SETUP` now carries a `TxReporter` and stores it in `cached.last_resend_reporter`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Generic circuit backup/late-entry `D-SETUP` is suppressed for individual calls; private setup retry remains owned by the dedicated EE retry path and is reporter-throttled.

Focused tests:

- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_eg7_p2p_d_setup_preempts_ordinary_backlog_at_receive_window`.
  - Uses a real serialized `DSetup` wrapped in LLC BL-UDATA/CMCE, places the called ISSI in valid EG7, queues ordinary backlog first, then asserts `D-SETUP` is the first SCH/F MAC-RESOURCE at the EG7 receive frame and reports TMA success.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated pending setup retry coverage to `test_p2p_pending_setup_does_not_duplicate_while_initial_reporter_pending`.
  - Asserts the initial private `D-SETUP` has a reporter and no generic backup/EE duplicate is sent while that reporter is pending.

Verification:

- `cargo test -p tetra-entities --test test_umac_bs test_eg7_p2p_d_setup_preempts_ordinary_backlog_at_receive_window --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_p2p_pending_setup_does_not_duplicate_while_initial_reporter_pending --locked` passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` passed: 76 tests.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` passed: 161 tests.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check -- crates/tetra-entities/src/umac/umac_bs.rs crates/tetra-entities/src/umac/subcomp/bs_sched.rs crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs crates/tetra-entities/tests/test_umac_bs.rs crates/tetra-entities/tests/test_cmce_bs.rs` passed.

Next RF gate:

1. Deploy this local build to `/home/chris/nexus-bs/nexus-bs`.
2. Restart `nexus-bs@chris.service`.
3. Clear/vacuum volatile journal.
4. Ask user to run one controlled P2P simplex call `2260082 -> 2260618`, with the called `2260618` still allowed to use EG7 if configured.
5. Inspect for: no `DSetup` timeout, one initial D-SETUP delivery, prompt `U-CONNECT`, called ACK path, caller `D-CONNECT`, first PTT audio.

Deploy:

- Ran `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh` after local focused/full verification above.
- Local release build for `nexus-bs v0.1.55` passed.
- Deployed commit label: `332fa519`.
- Deployed binary SHA-256: `b19cb7cee50f36d6a79798952776ed5d204a0fb2ce96d12245f0f4713c249c0d`.
- Service: `nexus-bs@chris.service`.
- Restart timestamp: `Sat 2026-06-06 11:19:45 EEST`.
- PID after restart: `47225`.
- Journal rotated/vacuumed for the RF gate at `2026-06-06T11:20:07+03:00`.
- Next read should inspect journal since `2026-06-06 11:20:07 EEST`.

## 2026-06-06 11:52 EEST - P2P called D-CONNECT ACK assigned-channel recovery

Trigger:

- User ran private simplex `2260082 -> 2260618` with three short PTT presses after the `11:19:45 EEST` deploy.
- RF log since latest restart showed the setup now reached `U-CONNECT`, but the called leg did not L2-ACK `D-CONNECT ACKNOWLEDGE`.
- Sequence observed:
  - `11:20:56`: `U-SETUP` from `2260082` to `2260618`, `call_id=4`.
  - `11:21:03`: UMAC circuit opened on TS2; CMCE sent called `D-CONNECT ACKNOWLEDGE`.
  - LLC retransmitted BL-DATA to `2260618`, exhausted attempts, and CMCE released setup with `AcknowledgedServiceNotComplete`.
- The failure point was not audio yet: caller `D-CONNECT` and initial private floor correctly remained blocked because the called `D-CONNECT ACKNOWLEDGE` lacked BL-ACK.

ETSI basis checked:

- EN 300 392-2 clause 14.5.1.1.1: after called `U-CONNECT`, the SwMI sends `D-CONNECT ACKNOWLEDGE` to the called MS.
- Clause 14.5.3.1: late assignment individual call may indicate traffic channels in `D-CONNECT` and `D-CONNECT ACKNOWLEDGE`.
- Clause 23.5.4.3.1 note 8: with L2 acknowledged service and channel allocation, the BS should grant current-channel ACK capacity when the MS may reject the allocation; in other cases grant on allocated channel may be appropriate.
- Clause 23.8.2.2: assigned-channel STCH/FACCH carries signalling around switch-to-U-plane timing.
- Annex D.4 remains informative ordering evidence: do not authorize the caller before the called leg is acknowledged. This patch is clause-scoped engineering evidence only, not formal certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Added explicit called `D-CONNECT ACKNOWLEDGE` delivery mode:
    - first attempt: `CurrentChannel`, `Layer2Service::Acknowledged`, `stealing_permission=false`;
    - retry after missing BL-ACK: `AssignedChannelRecovery`, same acknowledged service, `stealing_permission=true`, same channel allocation.
  - Caller `D-CONNECT` and initial private floor remain blocked until called `D-CONNECT ACKNOWLEDGE` reporter reaches L2 acknowledged.
- `crates/tetra-entities/src/llc/llc_bs_ms.rs`
  - Non-stealing channel-allocation BL-DATA now expects peer BL-ACK on current control TS1.
  - Stealing/recovery channel-allocation BL-DATA still expects peer BL-ACK on the assigned traffic timeslot.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Acknowledged channel-allocation TMA on the current channel receives an immediate current-channel BL-ACK grant.
  - Assigned-channel recovery with `stealing_permission=true` is emitted as STCH/FACCH on the active traffic channel.
- `crates/tetra-entities/src/umac/subcomp/bs_sched.rs`
  - Fixed direct CMCE floor-control classification so `D-TX INTERRUPT` is recognized before LLC `BL-UDATA-FCS` ambiguity and preempts positive `D-TX GRANTED`.

Focused tests added/updated:

- `test_p2p_called_d_connect_ack_l2_loss_never_authorizes_caller_before_release`
  - Proves first called `D-CONNECT ACKNOWLEDGE` is current-channel and retries switch to assigned-channel recovery while caller/floor remain blocked.
- `test_acked_channel_allocation_tma_carries_current_channel_ack_grant`
  - Proves current-channel acknowledged channel allocation carries a real BL-ACK grant.
- `test_acked_channel_allocation_stealing_tma_uses_assigned_channel_stch`
  - Proves assigned-channel recovery emits STCH MAC-RESOURCE with channel allocation and TMA success.
- `acked_channel_allocation_sent_on_mcch_expects_peer_ack_on_current_channel`
  - Proves LLC timeslot selection differs correctly between current-channel first attempt and assigned-channel recovery.
- `test_preemptive_floor_interrupt_stch_stays_ahead_of_positive_grant`
  - Proves `D-TX INTERRUPT` is not delayed behind `D-TX GRANTED`.

Verification:

- `cargo test -p tetra-entities --lib --locked` -> 255 passed, 5 ignored.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 78 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 83 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 161 passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

Operational target update:

- User clarified the practical Nexus-BS target is 100 simultaneous terminals, not thousands.
- Do not continue adding broad "thousands of terminals" refactors before core RF call paths are stable.
- Existing large-storm tests may remain as regression/backpressure evidence, but the next cleanup should document a 100-terminal operational target and reduce or make explicit any internal caps that could hold excessive floor/PTT backlog ahead of real lab traffic.

Next RF gate:

1. Build locally and deploy directly to `/home/chris/nexus-bs/nexus-bs`.
2. Restart `nexus-bs@chris.service`.
3. Rotate/vacuum volatile journal.
4. Ask user for one controlled P2P simplex test `2260082 -> 2260618`, first PTT short voice, then two more short PTT turns.
5. Inspect for called `D-CONNECT ACKNOWLEDGE` first current-channel attempt, assigned-channel retry only if needed, caller `D-CONNECT`, initial `FloorGranted`, and first-PTT audio.

Deploy:

- Ran `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh` after local verification above.
- Local release build for `nexus-bs v0.1.55` passed.
- Deployed commit label: `332fa519`.
- Deployed binary SHA-256: `395bcfe77c5939319191caed82c5a9eecd7b8826961a648523406383d185001f`.
- Service: `nexus-bs@chris.service`.
- Restart timestamp: `Sat 2026-06-06 11:53:25 EEST`.
- PID after restart: `47469`.
- Startup showed restart recovery armed for local ISSIs `{2260082, 2260616, 2260618}` and group `226333` restored for visible subscribers.
- Journal rotated/vacuumed for the next RF gate at `2026-06-06 11:54:02 EEST`.
- Next read should inspect journal since `2026-06-06 11:54:02 EEST`.

## 2026-06-06 13:13 EEST - P2P close hardening and frame-18 STCH audit

Trigger:

- User reported a later P2P `2260618 -> 2260082` failure with terminal `Network trouble`, plus a GSSI call glitch where `2260082` briefly showed very low signal.
- Last log review showed the P2P failure was setup-side: called `D-CONNECT ACKNOWLEDGE` was sent to `2260082`, but no L2 BL-ACK arrived; CMCE released with `AcknowledgedServiceNotComplete`.
- A separate live regression showed Motorola MXP600 could soft-reset or display `Not Answered` when the peer was cleared by `D-DISCONNECT` after a simplex private call.

ETSI basis checked:

- EN 300 392-2 14.5.1.1.1/14.5.1.1.2: called `U-CONNECT` is followed by SwMI `D-CONNECT ACKNOWLEDGE`, then caller `D-CONNECT`.
- EN 300 392-2 14.5.3.1 and Annex D.4: late channel assignment in `D-CONNECT`/`D-CONNECT ACKNOWLEDGE` can leave ambiguity between current-channel and assigned-channel listening; caller authorization remains gated until the called leg is L2-acknowledged.
- EN 300 392-2 14.5.1.3.1: after `U-DISCONNECT`, the initiating MS waits for `D-RELEASE`; SwMI may inform the other MS by either `D-DISCONNECT` or `D-RELEASE`.
- EN 300 392-2 14.5.1.3.3: `D-DISCONNECT` expects `U-RELEASE`, while `D-RELEASE` does not. For local simplex peer clear, use `D-RELEASE(UserRequestedDisconnection)` as the clause-permitted route that avoids the Motorola response-exchange crash.
- EN 300 392-2 frame-18 handling remains conservative: no frame-18 receive extension is advertised; assigned-channel STCH recovery must survive a frame-18 traffic gap and transmit on the next legal traffic slot.

Patch:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Converted simplex private-call peer-clear tests from old peer `D-DISCONNECT -> U-RELEASE` expectations to peer `D-RELEASE(UserRequestedDisconnection)` after bounded tail drain.
  - Preserved explicit duplex/fallback `D-DISCONNECT` tests for the paths that intentionally require a peer `U-RELEASE`.
  - Covered caller hangup, called-party hangup, pending release, call-id wrap, full simple private workflow, unsolicited `U-RELEASE`, and MXP600 last/current-speaker regressions.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added `test_assigned_channel_stch_recovery_survives_frame_18_gap`.
  - Proves a TS2 assigned-channel STCH recovery queued at `f=18,t=2` is not emitted illegally, is not reported as transmitted early, stays queued, and is emitted with channel allocation/usage marker on the next legal TS2 traffic opportunity.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 74 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 161 passed.
- `cargo test -p tetra-entities --test test_umac_bs test_assigned_channel_stch_recovery_survives_frame_18_gap --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 81 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 84 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 11 passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

Current conclusion:

- The previous `Network trouble` remains a real RF/setup failure, but focused UMAC evidence now shows queued assigned-channel STCH recovery is not lost at frame 18. Next live logs should determine whether `2260082` actually decodes and answers the current-channel or assigned-channel `D-CONNECT ACKNOWLEDGE`.
- Do not claim formal ETSI certification. This is clause-scoped engineering evidence and lab hardening.

Next RF gate:

1. Deploy the verified local build directly to `/home/chris/nexus-bs/nexus-bs`.
2. Restart `nexus-bs@chris.service`.
3. Rotate/vacuum volatile journal.
4. Ask user for one controlled P2P simplex `2260618 -> 2260082`, then one `2260082 -> 2260618`.
5. Inspect for called `D-CONNECT ACKNOWLEDGE` current-channel attempt, assigned-channel retry only if needed, BL-ACK arrival, caller `D-CONNECT`, initial `FloorGranted`, first-PTT audio, and final peer `D-RELEASE` close without `Not Answered`/soft reboot.

Deploy:

- Ran `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh` after the verification above.
- Local release build for `nexus-bs v0.1.55` passed.
- Deployed commit label: `332fa519`.
- Deployed binary SHA-256: `0c03206ffc72631e5d3541547952fd457055ebe1d3ab9cc9ce6279ff3e17c803`.
- Service: `nexus-bs@chris.service`.
- Restart timestamp: `Sat 2026-06-06 13:14:59 EEST`.
- PID after restart: `48039`.
- Startup showed restart recovery armed for local ISSIs `{2260082, 2260616, 2260618}`. `2260082` answered registration after two restart-recovery refresh attempts, so the next RF gate must watch its BL-ACK reliability closely.
- Journal rotated/vacuumed for the RF gate at `2026-06-06T13:15:20+03:00`; immediately after vacuum, `journalctl -u nexus-bs@chris.service -n 12` had no entries.
- Next read should inspect journal since `2026-06-06 13:15:20 EEST`.

## 2026-06-06 13:37 EEST - P2P direct setup no-BL-ACK fallback for real terminals

Trigger:

- User completed the controlled P2P RF gate after the previous deploy.
- Observed result: both P2P directions ended with `No answer`, no Motorola reboot, but no voice on any PTT.
- Log review showed both calls failed before active voice:
  - `2260618 -> 2260082`, `call_id=4`: called `D-CONNECT ACKNOWLEDGE` was sent/retried, but no BL-ACK arrived from `2260082`; caller `D-CONNECT` and initial `FloorGranted` were never sent.
  - `2260082 -> 2260618`, `call_id=6`: same failure pattern toward `2260618`.
- Conclusion: this was not a vocoder/audio-static failure. The CMCE direct setup gate was too strict for the observed terminal behavior because it treated called-side BL-ACK absence as a hard setup failure.

ETSI basis checked:

- EN 300 392-2 14.5.1.1.1/14.5.1.1.2: after called `U-CONNECT`, SwMI sends `D-CONNECT ACKNOWLEDGE`; caller reaches active call on `D-CONNECT`.
- EN 300 392-2 14.5.3.1: late individual channel assignment may be carried in `D-CONNECT` and `D-CONNECT ACKNOWLEDGE`.
- EN 300 392-2 Annex D.4: conservative sequence may wait for called MS L2 ACK, but if no PDU arrives in the granted subslot BS cannot know whether downlink failed or only the uplink ACK failed.
- EN 300 392-2 Annex D.5 adjacent note: repeat signalling may use unacknowledged service, or the BS must otherwise provide an MCCH ACK subslot/delay ACK handling.
- EN 300 392-2 20.3.1 and 22.3.2.4.1: basic-link unacknowledged service and BL-UDATA repetition are standard layer-2 tools for repeated point-to-point signalling when selected by the service user.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Reworked local RF P2P direct setup from a hard "wait for called BL-ACK before caller D-CONNECT" gate into two delivery guards:
    1. called `D-CONNECT ACKNOWLEDGE` is sent as repeated `Layer2Service::Unacknowledged` BL-UDATA (`unacked_bl_repetitions = 2`) with the late channel allocation;
    2. once local MAC/LLC reports the called `D-CONNECT ACKNOWLEDGE` transmitted, CMCE queues caller `D-CONNECT`;
    3. initial private-simplex `FloorGranted` is emitted only after caller `D-CONNECT` is locally transmitted.
  - Kept retry/release behavior for true local delivery failure: if the called `D-CONNECT ACKNOWLEDGE` is discarded or never locally transmitted, CMCE retries current-channel and assigned-channel recovery attempts; after bounded failure it releases with `AcknowledgedServiceNotComplete`.
  - Caller `D-CONNECT` remains acknowledged service, but CMCE does not require its BL-ACK for first floor; it requires only local transmission so the terminal has been told the call is connected before voice is enabled.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/mod.rs`
  - Added staged pending direct-setup state for called-leg delivery and caller `D-CONNECT` delivery guards.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Replaced BL-ACK-hard-gate P2P expectations with clause-scoped delivery-order tests:
    - called `D-CONNECT ACKNOWLEDGE` is repeated unacknowledged with channel allocation;
    - pending local delivery does not authorize caller or floor;
    - no called BL-ACK still falls forward after local delivery instead of releasing;
    - caller `D-CONNECT` precedes initial `FloorGranted`;
    - local MAC discard retries before release.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 74 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 161 passed.
- `cargo test -p tetra-entities --test test_umac_bs --locked` -> 81 passed.
- `cargo test -p tetra-entities --test test_llc_bs --locked` -> 84 passed.
- `cargo test -p tetra-entities --test test_lmac_bs --locked` -> 11 passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed before the final comment cleanup; re-run `git diff --check` before deploy.

Next RF gate after deploy:

1. Clear journal.
2. Ask user for one P2P simplex `2260618 -> 2260082`, short first PTT voice, then close.
3. Ask user for one P2P simplex `2260082 -> 2260618`, short first PTT voice, then close.
4. Inspect logs for repeated unacknowledged called `D-CONNECT ACKNOWLEDGE`, caller `D-CONNECT` local transmission, then initial `FloorGranted`.
5. Required field outcome: first PTT must carry voice; close must not show `No answer` and must not reboot MXP600.

## 2026-06-06 13:46 EEST - P2P direct setup restored to Annex D.4 called BL-ACK gate

Trigger:

- User completed the post-deploy P2P RF test after the repeated-unacknowledged D.5-style fallback.
- Field result: both call directions ended with `No answer`, no Motorola reboot, but no voice on any PTT.
- Journal after the gate did not contain the expected `U-SETUP`/`U-CONNECT`/`D-CONNECT`/`FloorGranted` info lines, so the next RF gate must keep the journal clear and verify the exact call timestamps. However, the RF result invalidated the previous "local transmission is enough" called-leg fallback for real Motorola/Hytera terminals.

ETSI basis re-checked:

- EN 300 392-2 clauses 14.5.1.1.1/14.5.1.1.2 require `D-CONNECT ACKNOWLEDGE` to the called MS after `U-CONNECT`, with an indication of which party may transmit.
- EN 300 392-2 Annex D.4 gives the conservative same-cell direct setup sequence: called `D-CONNECT ACK` with channel allocation, wait for the called MS layer-2 ACK, then send caller `D-CONNECT`.
- EN 300 392-2 Annex D.5 allows a faster alternative, but explicitly warns that if the called MS misses `D-CONNECT ACK`, it will not receive the first part of traffic. The field result matched that risk, so the D.5-style fallback is not acceptable as the default P2P setup path.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Restored called-leg `D-CONNECT ACKNOWLEDGE` to `Layer2Service::Acknowledged`.
  - Removed repeated unacknowledged BL-UDATA setup completion from the called leg.
  - CMCE now treats called `D-CONNECT ACKNOWLEDGE` `TxReporter::is_acknowledged()` as the only success condition that releases caller `D-CONNECT`.
  - Local transmission without BL-ACK no longer authorizes caller `D-CONNECT` or private-simplex `FloorGranted`.
  - Existing bounded retry/recovery remains: current-channel retry, assigned-channel recovery retry, then setup release with `AcknowledgedServiceNotComplete` if the called leg cannot be locally transmitted/acknowledged.
  - Caller `D-CONNECT` remains acknowledged service, but initial private-simplex floor still waits only for caller `D-CONNECT` local transmission, not caller BL-ACK, matching the existing clause-scoped guard.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated P2P tests to assert called `D-CONNECT ACKNOWLEDGE` uses acknowledged L2 service and no unacknowledged repetitions.
  - Added `test_p2p_called_d_connect_ack_transmitted_without_l2_ack_does_not_authorize_caller`.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 75 passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` -> 162 passed.
- `cargo test -p tetra-entities --test test_umac_bs private --locked` -> 18 passed.
- `cargo test -p tetra-entities --test test_llc_bs ack --locked` -> 38 passed.
- `cargo check -p tetra-entities --locked` passed.
- `cargo fmt -p tetra-entities` completed.
- `git diff --check` passed.

Next RF gate:

1. Deploy local build to `/home/chris/nexus-bs/nexus-bs`.
2. Restart `nexus-bs@chris.service`.
3. Clear volatile journal.
4. Ask user for exactly one P2P simplex call, first `2260618 -> 2260082`, with one short PTT and close.
5. Read logs immediately and verify:
   - called `D-CONNECT ACKNOWLEDGE` is acknowledged before caller `D-CONNECT`;
   - `UMAC floor granted` appears only after caller `D-CONNECT` local transmission;
   - first PTT voice is routed;
   - close uses the current D-RELEASE path without `No answer` or MXP600 reboot.

## 2026-06-06 14:05 EEST - Brew GSSI 91 group subscription no longer blocked by local P2P ISSI range

Trigger:

- User reported GSSI 91 was no longer audible on local TETRA terminals even though it should arrive via Brew to affiliated stations.
- Journal since the last restart showed Brew SDS forwarding and external subscriber deregistration events, but no `GROUP_TX`, `NetworkCallStart`, `no listeners`, or `not routable` events for GSSI 91.
- Runtime subscriber recovery cache in `/run/nexus-bs-chris/config.toml.subscribers` contained `2260618 91:0:4`, so the cell had a recoverable local listener for GSSI 91.

Analysis:

- The real Pi config keeps private-call lab ISSIs local with `local_ssi_ranges = [[0, 90], [2260000, 2269999]]`. That is correct for local P2P routing.
- `BrewEntity` was also using `is_brew_issi_routable()` for group affiliation and local group `GROUP_TX` forwarding. This incorrectly filtered local ISSIs such as 2260618 before they could publish a Brew-routable GSSI 91 subscription.
- This is not an ETSI air-interface PDU change. It is Brew interconnect policy around subscriber registration/affiliation. The ETSI-relevant air path remains normal group call setup/traffic handling under EN 300 392-2 clause 14.5.2 and MM group affiliation/recovery handling under clause 16 procedures.

Patch:

- `crates/tetra-entities/src/net_brew/entity.rs`
  - Added GSSI-scoped filtering for Brew group subscriptions.
  - Preserved `local_ssi_ranges` as the P2P/private-call ISSI policy.
  - Allowed a local private-call ISSI to send `REGISTER + AFFILIATE` when the group itself is Brew-routable, e.g. `2260618 -> GSSI 91`.
  - Resync after Brew reconnect now replays local ISSI group subscriptions when they contain Brew-routable groups.
  - Local group PTT forwarding now checks `dest_gssi` routing policy, not `source_issi` P2P routing policy.
  - Local GSSI 90 remains suppressed because it is inside the configured local range.

Verification:

- `cargo fmt -p tetra-entities` completed.
- `cargo test -p tetra-entities --lib local_private_issi --locked` -> 4 passed.
- `cargo test -p tetra-entities --lib net_brew::entity --locked` -> 16 passed.
- `cargo test -p tetra-entities --test test_cmce_bs brew --locked` -> 9 passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

Next RF/network gate:

1. Deploy local build to `/home/chris/nexus-bs/nexus-bs`.
2. Restart `nexus-bs@chris.service`.
3. Clear volatile journal.
4. Verify startup/recovery logs contain `local ISSI 2260618 group-subscribe` or `resync ... groups=[91]`.
5. Ask user for one controlled GSSI 91 Brew audio test.
6. Expected logs: Brew subscriber `REGISTER + AFFILIATE groups=[91]`, then `GROUP_TX` for GSSI 91, then CMCE network group call and UMAC downlink audio to affiliated members.

## 2026-06-06 14:12 EEST - Config exceptions narrowed to 1-90 and 226333

Trigger:

- User required `config.toml` to keep only `1-90` and `226333` as local exceptions.
- Private calls between locally attached terminals must still stay inside Nexus-BS even when terminal ISSIs are not listed statically in TOML.

Analysis:

- `local_ssi_ranges` is a Brew/local routing exception policy, not the live list of terminals served by the cell.
- Runtime-local private call selection remains CMCE/MM state based: a called ISSI that is registered in the SwMI state routes locally without a static TOML range.
- Restart recovery uses the volatile subscriber cache and security policy; cached terminal ISSIs are not limited by `local_ssi_ranges`.
- This checkpoint does not claim whole-stack ETSI certification. The ETSI-facing behaviour is clause-scoped to EN 300 392-2 MM registration/group recovery and CMCE private-call local setup; the TOML exception list itself is deployment policy.

Patch:

- `example_config/config.toml`
  - `local_ssi_ranges = [[1, 90], [226333, 226333]]`.
  - Removed the numeric `restart_recovery_issis` example so no terminal ISSI is presented as a static exception.
- `crates/tetra-entities/src/net_brew/components/brew_routable.rs`
  - Updated tests so 42 and 226333 are the configured local exceptions.
  - Added an assertion that terminal ISSI 2260616 is not blocked by TOML policy; runtime CMCE/MM state decides local P2P.
- `crates/tetra-entities/src/net_brew/entity.rs`
  - Updated Brew entity tests to match the narrowed config policy: runtime subscribers can register/affiliate with Brew for interconnect while local P2P remains CMCE-state based.
- Live Pi config `/home/chris/nexus-bs/config.toml`
  - Confirmed active `local_ssi_ranges` contains only `[1, 90]` and `[226333, 226333]`.
  - Removed old commented `restart_recovery_issis` entries for 2260082/2260616/2260618.

Verification:

- `cargo fmt -p tetra-entities` completed.
- `cargo test -p tetra-entities --lib brew_routable --locked` -> 3 passed.
- `cargo test -p tetra-entities --lib net_brew::entity --locked` -> 16 passed.
- `cargo test -p tetra-config --lib bluestation::parsing --locked` -> 29 passed.
- `cargo test -p tetra-entities --test test_cmce_bs runtime_registered_local_issis --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery_cache_is_not_limited --locked` -> 1 passed.
- `git diff --check` passed.

Next gate:

1. Do not re-add `[2260000, 2269999]` to `local_ssi_ranges`.
2. For P2P regression work, validate local routing through runtime registration/attach state, not static TOML ISSI lists.

## 2026-06-06 15:10 EEST - Private simplex close peer cause corrected for Motorola UI

Trigger:

- User re-framed the private simplex first-PTT behaviour correctly: for hook/simplex setup, the first caller PTT can initiate the call and the called user confirms/gets transmit permission before useful speech from the caller.
- Remaining defect: Motorola peer showed `No answer` / `Not Answered` on normal private simplex call close, even when audio and setup were otherwise working.

Technical explanation:

- CMCE is the call-control component. It sends setup, floor, and release PDUs for private and group calls.
- In private simplex, the MS that sends `U-DISCONNECT` is the disconnect initiator. It must receive `D-RELEASE` promptly.
- The other MS is only the peer being informed that an already established call is being cleared. It did not request the disconnect itself.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.3.1: the MS sending `U-DISCONNECT` waits for `D-RELEASE`.
- EN 300 392-2 clauses 14.5.1.3.1/14.5.1.3.3: the SwMI may inform the other individual-call MS by `D-DISCONNECT` or by `D-RELEASE`; `D-RELEASE` requires no MS response.
- Mapping the peer-facing `D-RELEASE` cause from `UserRequestedDisconnection` to `SwmiRequestedDisconnection` is ETSI-compatible implementation policy, not an ETSI-mandated remap. It matches the peer perspective and avoids recent Motorola terminals rendering the normal close as setup `No answer`.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Simplex peer clear now always sends assigned-channel reporter-tracked `D-RELEASE` after the bounded bearer tail drain.
  - Initiator `D-RELEASE` keeps the original `UserRequestedDisconnection` cause.
  - Peer `D-RELEASE` maps `UserRequestedDisconnection` to `SwmiRequestedDisconnection`; other causes are preserved.
  - Peer timeout logging now reports the actual peer-facing release cause.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Updated the private simplex disconnect comment so it no longer says the peer preserves the initiator cause.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated all private-simplex peer release expectations to `SwmiRequestedDisconnection`.
  - Kept initiator expectations as `UserRequestedDisconnection`.
  - Added/renamed MXP600 regression coverage for peer clear after the Motorola peer was the last floor holder.
  - Duplex/explicit `D-DISCONNECT -> U-RELEASE` tests remain separate because that is a different ETSI-valid path.

Verification:

- `cargo fmt -p tetra-entities` completed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 77 passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.
- Read-only CMCE telecom audit confirmed the PDU-level simplex disconnect flow is clause-consistent and that the peer cause remap is ETSI-compatible but should be documented as implementation policy.

Next RF gate:

1. Deploy the local build directly to `/home/chris/nexus-bs/nexus-bs`.
2. Restart `nexus-bs@chris.service`.
3. Clear volatile journal.
4. Ask user for one private simplex close test:
   - recommended first scenario: `2260616 -> 2260618`, let `2260618` speak last, then close with red key from `2260616`;
   - required outcome: `2260618` should show normal call disconnected/end-call state, not `No answer`, and must not reboot.
5. Read logs immediately after the RF test and verify:
   - initiator `D-RELEASE` cause is `UserRequestedDisconnection`;
   - peer `D-RELEASE` cause is `SwmiRequestedDisconnection`;
   - no peer `D-DISCONNECT` is emitted on this simplex close path;
   - circuit closes only after reporter delivery or bounded guard.

## 2026-06-06 16:28 EEST - Private simplex setup no longer falls forward after missing called BL-ACK

Trigger:

- RF test after the peer-cause deploy still failed: user reported `PTT denied` during the call and `No Answer` at final clear.
- Journal from the clean test window showed call `2260616 -> 2260618`, call_id `4`, at `2026-06-06 16:24:25`.

Log finding:

- `D-CONNECT ACKNOWLEDGE` to called Motorola `2260618` was sent with acknowledged L2 service.
- LLC retransmitted it 3 times and then reported exhausted retransmissions / no called BL-ACK.
- CMCE then used the previous `current-channel unacknowledged repeat recovery`, treated local transmission of that repeat as enough, sent caller `D-CONNECT`, and enabled initial private U-plane.
- After that, floor grants appeared normal in BS logs, but the terminal-side result was `PTT denied` and final `No Answer`.

Technical explanation:

- CMCE must not treat "I transmitted a repeated D-CONNECT ACK" as equivalent to "the called MS acknowledged D-CONNECT ACK".
- For Motorola-like RF terminals this creates a false-active private call: BS thinks setup is complete, but the called terminal has not completed the CMCE/L2 setup state machine.

ETSI clause scope:

- EN 300 392-2 clauses 14.5.1.1.1/14.5.1.1.2 require called-leg `D-CONNECT ACKNOWLEDGE` before caller `D-CONNECT`.
- EN 300 392-2 Annex D.4 is the conservative same-cell direct setup sequence: called `D-CONNECT ACK` with channel allocation, wait for called MS L2 ACK, then send caller `D-CONNECT`.
- The previous Annex D.5-style fallback is not acceptable as a default for these real terminals because the field result reproduced Annex D.5's risk: called MS missed/failed setup and then did not behave as a valid private-call participant.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Removed `CurrentChannelUnacknowledgedRepeat` from called `D-CONNECT ACK` recovery.
  - All called `D-CONNECT ACK` retries now use `Layer2Service::Acknowledged`.
  - Lost called BL-ACK retries alternate current-channel and assigned-channel recovery, but caller `D-CONNECT` is released only after real called BL-ACK.
  - If called BL-ACK never arrives after bounded retries, CMCE releases setup with `AcknowledgedServiceNotComplete` instead of creating a false-active P2P call.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Replaced the old test that expected unacknowledged fallback with `test_p2p_called_d_connect_ack_lost_does_not_fall_forward_without_bl_ack`.
  - New regression asserts no caller `D-CONNECT`, no `FloorGranted`, and setup release after exhausted called BL-ACK retries.

Verification:

- `cargo fmt -p tetra-entities` completed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p_called_d_connect_ack --locked` -> 4 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 77 passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

Next RF gate:

1. Deploy local build directly to `/home/chris/nexus-bs/nexus-bs`.
2. Restart `nexus-bs@chris.service`.
3. Clear volatile journal.
4. Ask user for the same private simplex test: `2260616 -> 2260618`, let `2260618` speak last, close from `2260616`.
5. Required log result:
   - if called BL-ACK arrives: caller `D-CONNECT`, then floor grants, then close path as above;
   - if called BL-ACK does not arrive: no caller `D-CONNECT`, no floor, release with `AcknowledgedServiceNotComplete`.
6. Required RF result for success: no `PTT denied` during an active established call and no false `No Answer` at normal close.

## 2026-06-06 16:46 EEST - Deployed private simplex peer clear via D-DISCONNECT/U-RELEASE

Trigger:

- RF test after the previous deploy still reported `PTT denied` and `No Answer` at final close.
- The previous timeline checkpoint still expected peer `D-RELEASE(SwmiRequestedDisconnection)`, but live Motorola MXP600 behavior invalidated that as the normal local simplex peer-clear path.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.3.1: the MS that sends `U-DISCONNECT` waits for `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3: the other MS can be explicitly cleared by `D-DISCONNECT`, then responds with `U-RELEASE`.
- This is clause-scoped implementation evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Local simplex `U-DISCONNECT` now keeps prompt initiator `D-RELEASE`, then tail-drains and clears the peer with `D-DISCONNECT`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Removed the normal peer `D-RELEASE` branch and stale peer-release reporter model.
  - Normal peer clear is `D-DISCONNECT -> U-RELEASE`; `D-RELEASE` remains only as bounded fallback if `D-DISCONNECT` delivery/response fails.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated P2P/MXP600 regression coverage to require peer `D-DISCONNECT`, reporter delivery, peer `U-RELEASE`, and only then final circuit close after the initiator `D-RELEASE` reporter.

Verification:

- `cargo fmt -p tetra-entities` completed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 77 passed.
- `cargo test -p tetra-entities --test test_cmce_bs mxp600 --locked` -> 2 passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.
- Local aarch64 release build completed through `scripts/nexus-bs-test-deploy.sh`.

Deploy:

- Deployed directly to `/home/chris/nexus-bs/nexus-bs`.
- Restarted `nexus-bs@chris.service`.
- Running PID after deploy: `50516`.
- Deployed binary SHA: `69b964ebd843a3324fbd34705d85e54a361591206d7cd0cf812b8a703b860e9b`.
- Build marker: `v0.1.55-332fa519-modified`.
- Journal was rotated/vacuumed at `2026-06-06 16:45:58 EEST`.

Next RF gate:

1. User performs private simplex `2260616 -> 2260618`.
2. Preferably let `2260618` speak last.
3. Close with red key from `2260616`.
4. Expected normal log path:
   - caller `U-DISCONNECT(UserRequestedDisconnection)`;
   - initiator `D-RELEASE(UserRequestedDisconnection)`;
   - after tail drain, peer `D-DISCONNECT(UserRequestedDisconnection)`;
   - peer `U-RELEASE`;
   - UMAC circuit closes only after peer clear and initiator release reporter/guard.
5. Required RF result for success: no terminal-visible `PTT denied` in an established call, no false `No Answer` on `2260618`, and no MXP600 soft reboot.

## 2026-06-06 16:52 EEST - Local PTT-denied regressions for P2P disconnect windows

Context:

- While the deployed BS was left running for the RF gate, expert-agent review identified two precise PTT-denial risk windows: `U-TX DEMAND` during simplex disconnect tail-drain and `U-TX DEMAND` after peer `D-DISCONNECT` delivery but before peer `U-RELEASE`.

Patch:

- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_p2p_disconnect_tail_drain_ignores_late_tx_demands_without_not_granted`.
  - Added `test_p2p_disconnect_pending_ignores_tx_demands_before_peer_u_release`.
  - Both assert zero `D-TX GRANTED/NotGranted`, zero floor events, and no premature close while the EN 300 392-2 clause 14.5.1.3.1/14.5.1.3.3 disconnect sequence is pending.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/{call.rs,timers.rs,shared.rs}`
  - Cleaned comments to cite clause 14.5.1.3.3 for `D-DISCONNECT -> U-RELEASE`.
- Renamed stale MXP600 test wording from "swmi_release" to "d_disconnect".

Verification:

- `cargo fmt -p tetra-entities` completed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p_disconnect_tail_drain --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p_disconnect_pending_ignores_tx_demands --locked` -> 1 passed.
- `cargo test -p tetra-entities --test test_cmce_bs mxp600 --locked` -> 2 passed.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 79 passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

Deploy status:

- No runtime redeploy needed for this checkpoint; changes after the `69b964...` deploy are comments/tests only.
- Current RF test gate remains the already deployed PID `50516` binary.

## 2026-06-06 18:14 EEST - Private simplex close changed to peer D-RELEASE after Motorola reboot

Context:

- RF test after the previous deploy soft-rebooted a Motorola after local private simplex close.
- Live log showed call `2260082 -> 2260616`, floor turns worked, then close sent:
  - initiator `D-RELEASE(UserRequestedDisconnection)`;
  - peer `D-DISCONNECT(UserRequestedDisconnection)`;
  - no peer `U-RELEASE`;
  - timeout fallback `D-RELEASE(UserRequestedDisconnection)`;
  - Motorola re-attached shortly after.
- Clause-scoped ETSI review reconfirmed EN 300 392-2 clause 14.5.1.3.1 requires `D-RELEASE` to the MS that sent `U-DISCONNECT`, and clause 14.5.1.3.3 permits the other MS to be informed by either `D-DISCONNECT` or `D-RELEASE`. `D-RELEASE` is final and expects no MS response.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Local simplex `U-DISCONNECT` keeps prompt initiator `D-RELEASE`, then uses peer `D-RELEASE` after the bearer tail drain.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Pending individual disconnect release ack now tracks peer `D-RELEASE` reporters separately.
  - Final cleanup waits for initiator `D-RELEASE` delivery and peer `D-RELEASE` delivery, or local guard expiry.
  - Late `U-TX DEMAND` during the close window remains ignored to avoid terminal-visible `NotGranted` / PTT denied.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - If a legacy/duplex `D-DISCONNECT` path times out after delivery, BS now closes the local circuit without emitting another peer clear PDU.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated local private simplex and MXP600 close tests to expect peer `D-RELEASE` instead of peer `D-DISCONNECT -> U-RELEASE`.
  - Updated stuck `D-DISCONNECT` timeout test to assert no fallback clear PDU and local cleanup.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 79 passed.
- `cargo test -p tetra-entities --test test_cmce_bs mxp600 --locked` -> 2 passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

Deploy status:

- Deployed directly to `/home/chris/nexus-bs/nexus-bs`.
- Running service: `nexus-bs@chris.service`, `MainPID=50813`, active since `2026-06-06 18:15:28 EEST`.
- Deployed binary SHA: `47391218eab933c880e36d06074fe4c03c2cef2c06aa0141aa8e3be07bdbec2e`.
- Journal was rotated/vacuumed after deploy for a clean RF gate.

Next RF gate:

1. User performs private simplex `2260616 -> 2260618`.
2. Preferably include at least one normal PTT turn, then close with red key from `2260616`.
3. Expected normal log path:
   - caller `U-DISCONNECT(UserRequestedDisconnection)`;
   - initiator `D-RELEASE(UserRequestedDisconnection)`;
   - after tail drain, peer `D-RELEASE(UserRequestedDisconnection)`;
   - no peer `D-DISCONNECT` and no peer `U-RELEASE` requirement;
   - UMAC circuit closes after both release reporters or local guard.
4. Required RF result for success: no terminal-visible `PTT denied` in an established call, no false `No Answer`, and no Motorola soft reboot.

## 2026-06-07 01:40 EEST - Private simplex setup grant now seeds initial UMAC floor

Context:

- Live RF regression after the 01:31 deploy: private simplex `2260616 -> 2260618` reached `Active`, but no `U-TX DEMAND` and no `FloorGranted` appeared after caller `D-CONNECT` was L2-acknowledged.
- Field symptom matched the log: terminal opened the private-call channel with no speech because CMCE told the MS `TransmissionGrant::Granted` in setup, while BS internals kept UMAC media closed waiting for a later `U-TX DEMAND`.
- This is not a formal certification claim; it is a clause-scoped correction against ETSI EN 300 392-2.

ETSI basis:

- Clause 14.5.1.1.1: `D-CONNECT ACKNOWLEDGE` shall indicate which party is permitted to transmit.
- Clause 14.5.1.1.2: `D-CONNECT` shall indicate which party is permitted to transmit.
- Clause 14.5.1.2.1 b): during call setup, the response to the setup-phase transmit request is handled by 14.5.1.1.1/14.5.1.1.2, and the MS given permission starts T311.
- Therefore the setup `TransmissionGrant` is the initial simplex floor. Later `U-TX DEMAND` / `D-TX GRANTED` remains the in-call floor change/refresh procedure.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Kept the ETSI grant polarity restored earlier:
    - called-first setup: called `D-CONNECT ACKNOWLEDGE = Granted`, caller `D-CONNECT = GrantedToOtherUser`;
    - caller-first setup: called `D-CONNECT ACKNOWLEDGE = GrantedToOtherUser`, caller `D-CONNECT = Granted`.
  - After caller `D-CONNECT` is L2-acknowledged and the call transitions active, CMCE now sets `floor_holder` to the setup-granted ISSI and emits one UMAC `FloorGranted` for that ISSI.
  - Still blocks U-plane before caller `D-CONNECT` delivery, so the called-leg ACK alone cannot open media early.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated P2P setup tests to enforce no floor before caller `D-CONNECT` ACK and exactly one setup-granted initial floor afterward.
  - Added/updated coverage for default direct setup floor to caller and hook/request-other-MS floor to called MS.
  - Kept later `U-TX DEMAND` semantics as explicit floor refresh/change, not the first floor.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` passed: 81/81.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` passed: 168/168.
- `cargo test -p tetra-entities --test test_umac_bs private --locked` passed: 18/18.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

Next RF gate:

- Deploy direct to `/home/chris/nexus-bs/nexus-bs`.
- Clear the volatile journal.
- Ask for one structured test: `2260616 -> 2260618` private simplex. For this hook/request-other-MS path, expected BS log after caller `D-CONNECT` ACK is `setup grant seeds U-plane floor for ISSI 2260618`; first PTT from `2260616` should only open/setup, and speech should be valid when `2260618` responds.

## 2026-06-06 19:45 EEST - PHY RF timing-collapse log storm guard

Context:

- After the P2P RF gate, the service did not process-crash: systemd still showed `ActiveState=active`, `MainPID=51128`, `NRestarts=0`, and no coredump.
- The RF side collapsed instead: logs showed `Discarding TX samples in the past`, `RX buffer overrun`, then thousands of `Too late to produce TX block` warnings per second.
- This patch is implementation hardening in PHY/Soapy I/O. It does not change ETSI CMCE/MM/UMAC/SDS PDUs or call-control semantics.

Patch:

- `crates/tetra-entities/src/phy/components/soapy_dev.rs`
  - Added `SdrTimingEventLog` for one-second aggregation of repeated real-time timing warnings.
  - RX lost-sample and TX-late hot paths now emit the first warning immediately, then summarize suppressed events/blocks/samples instead of logging every missed block.
  - Purpose: keep circular journald usable and avoid warning spam making a recoverable SDR timing slip worse.

Verification:

- `cargo fmt --package tetra-entities` completed.
- `cargo check -p tetra-entities --locked` passed.
- `cargo test -p tetra-entities --lib soapy --locked` passed, no matching tests.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` -> 79 passed.
- `git diff --check` passed.

Deploy status:

- Service was restarted once to recover the live SDR state. New running service before deploy: `nexus-bs@chris.service`, `MainPID=51592`, active since `2026-06-06 19:42:43 EEST`.
- Deployed directly to `/home/chris/nexus-bs/nexus-bs`.
- Running service after deploy: `nexus-bs@chris.service`, `MainPID=52040`, active since `2026-06-06 19:46:27 EEST`.
- Deployed binary SHA: `57ec48041095febd3f03df6bbc430f139a8f8850914014b6fb30e50f61c5da5f`.
- Boot still emitted normal one-shot timing warnings (`Lost ... samples`, `Too late ...`) while Soapy streams initialized, but the repeated warning path is now rate-limited/summarized.

## 2026-06-07 00:27 EEST - Nexus-BS test deploy for RF gate

Context:

- User returned in range of TetraHS and requested deploy/test.
- ETSI law memory was reloaded before deploy; this step changed no protocol semantics.
- Build remained local; no Rust compilation was performed on TetraHS.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs repeated_group_u_setup --locked` passed: 3 tests.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` passed: 168 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked` passed: 81 tests.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` passed: 34 tests.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.
- `cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs` passed.

Deploy:

- Deployed directly to `/home/chris/nexus-bs/nexus-bs`.
- Running service: `nexus-bs@chris.service`, `MainPID=53350`, `NRestarts=0`, active since `2026-06-07 00:27:16 EEST`.
- Deployed binary SHA: `19d7900d3767055978582ded7d1a31cab2814389bd24f2e2da25abd0276031a1`.
- Build banner: `Nexus-BS v0.1.55`, build `v0.1.55-332fa519-modified`.

Observed restart recovery after deploy:

- `2260616` restored cached group `[226333]`.
- `2260082` registered and affiliated group `[91]`.
- `2260618` initially needed several restart-recovery retries, then registered at `00:27:39` and affiliated group `[226333]`.

Next RF gate:

- Clear volatile journal before test.
- Test local private simplex `2260616 -> 2260618` and close from the caller after at least one called-side PTT turn.
- Also test one group PTT sequence on `226333` if private simplex is clean.
- Required result: no false `No Answer`, no Motorola soft reboot, no established-call `PTT denied`, and no static-only audio after floor grant.

## 2026-06-07 00:49 EEST - LLC early BL-ACK acceptance for P2P setup race

Context:

- RF gate after the 00:27 deploy failed for private simplex `2260616 -> 2260618`.
- Log path:
  - `U-SETUP` from `2260616` to `2260618`, `call_id=17`.
  - BS sent called-leg `D-CONNECT ACKNOWLEDGE` with late channel allocation.
  - LLC logged `received ACK for SSI 2260618 endpoint 0 N(R) 1 before a complete UMAC transmission. Ignoring`.
  - BS then treated the called ACK path as still unsettled/retry-prone; shortly after caller `D-CONNECT`, `2260618` sent `U-DISCONNECT(InvalidCallIdentifier)` while CMCE was in `CallerConnectAckPending`.
- Clause-scoped ETSI check:
  - EN 300 392-2 clause 22.3.2.3(j): a matching BL-ACK `N(R)` confirms the waiting acknowledged BL-DATA transfer.
  - EN 300 392-2 clause 22.3.2.3(f): first complete transmission starts the ACK wait/report path.
  - EN 300 392-2 Annex D.4 supports waiting for the called MS L2 ACK before sending caller `D-CONNECT`.
- Agent reviews agreed that the safe first patch is LLC ACK ordering only; no CMCE grant-field change was made.

Patch:

- `crates/tetra-entities/src/llc/llc_bs_ms.rs`
  - If a BL-ACK arrives before LLC has observed the local UMAC completion reporter, and the basic-link context plus `N(R)` match the stored `N(S)`, LLC now accepts the ACK as proof that the peer received the downlink.
  - It synthesizes first-complete reporting, marks the service transfer acknowledged, and removes the waiting transfer instead of retransmitting the already-acknowledged call-control PDU.
  - Wrong/stale `N(R)` before first-complete remains ignored and does not confirm the transfer.
- `crates/tetra-entities/tests/test_llc_bs.rs`
  - Added regression for matching BL-ACK before local UMAC completion.
  - Existing wrong-ACK, endpoint isolation, T.251, and BL-ADATA tests continue to cover stale/incorrect ACK risk.

Verification:

- `cargo test -p tetra-entities --test test_llc_bs matching_bl_ack --locked` passed: 3 tests.
- `cargo test -p tetra-entities --test test_llc_bs --locked` passed: 85 tests.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` passed: 81 tests.
- `cargo test -p tetra-entities --test test_cmce_bs --locked` passed during deploy: 168 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked` passed during deploy: 81 tests.
- `cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked` passed during deploy: 34 tests.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.
- `cargo fmt --package tetra-entities` completed.

Deploy:

- Deployed directly to `/home/chris/nexus-bs/nexus-bs`.
- Running service: `nexus-bs@chris.service`, `MainPID=53628`, `NRestarts=0`, active since `2026-06-07 00:49:44 EEST`.
- Deployed binary SHA: `c0a9ef2b8d92316c343cd59a4f67835cf73d0a6f9eebef0905d79a9fcd66ccd5`.
- Build banner: `Nexus-BS v0.1.55`, build `v0.1.55-332fa519-modified`.

Next RF gate:

- Clear volatile journal before test.
- Repeat private simplex `2260616 -> 2260618`.
- Watch specifically for absence of `received ACK ... before a complete UMAC transmission. Ignoring`, absence of `U-DISCONNECT(InvalidCallIdentifier)` from `2260618`, and successful caller/called `D-CONNECT` activation.

## 2026-06-07 09:24 EEST - Private simplex connected notification without release-cause regression

Context:

- User reported the current RF-good base has first-PTT voice from the initiating PTT, but Motorola still shows `No answer` instead of a normal disconnected/end-call UI at private simplex close.
- Agents were closed/restarted for a focused read-only audit. The ETSI audit rejected changing the peer-facing RF release cause from `UserRequestedDisconnection` to `SwmiRequestedDisconnection`; clause 14.5.1.3.1/14.5.1.3.3 allow peer `D-RELEASE`, but table 14.55 still identifies the reason as user-requested disconnection.
- Live log for `2260082 -> 2260618`, `call_id=4`, showed the RF release path was already an established-call close:
  - `U-DISCONNECT` from `2260082` while CMCE state was `Active`;
  - prompt `D-RELEASE(UserRequestedDisconnection)` to `2260082`;
  - tail-drained peer `D-RELEASE(UserRequestedDisconnection)` to `2260618`;
  - UMAC circuit closed only after release delivery.
- The same log confirmed first-PTT recovery behaviour remained the current best base:
  - caller `D-CONNECT` current-channel was not L2-ACKed;
  - assigned-channel recovery `D-CONNECT` was ACKed;
  - `FloorGranted` was emitted only after caller `D-CONNECT` ACK;
  - deferred private media then flowed with speech present.

ETSI clause scope:

- EN 300 392-2 clause 14.5.1.1.1/14.5.1.1.2: individual call setup uses `D-CONNECT ACKNOWLEDGE`/`D-CONNECT` to carry transmit state and enter active call.
- EN 300 392-2 clause 14.5.1.2.2: most downlink CC PDUs may carry a Notification indicator to inform the user about offered or connected service.
- EN 300 392-9 clause 7.2.2 notification value `19`: `Called user connected`.
- EN 300 392-2 clause 14.5.1.3.1/14.5.1.3.3: normal private-call close remains `U-DISCONNECT` followed by `D-RELEASE`; peer may also be informed by `D-RELEASE`, with no response expected.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/individual.rs`
  - Added `NOTIFICATION_CALLED_USER_CONNECTED = 19`.
  - Local private `D-CONNECT ACKNOWLEDGE` to the called MS now carries notification indicator `19`.
  - Local private caller `D-CONNECT` now carries notification indicator `19`.
  - No changes to `D-RELEASE`, `D-DISCONNECT`, floor grants, caller `D-CONNECT` retry/recovery, or media routing.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Extended the full simple private setup/release workflow to assert notification indicator `19` on both connect PDUs.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs --locked test_simple_private_call_full_direct_setup_and_release_workflow -- --exact` passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked p2p` passed: 82 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked private` passed: 19 tests.
- `cargo check -p tetra-entities --locked` passed.
- `cargo fmt --package tetra-entities` completed.
- `git diff --check` passed.

Next RF gate:

1. Commit and deploy directly to `/home/chris/nexus-bs/nexus-bs`.
2. Clear volatile journal.
3. Repeat local private simplex, preferably `2260082 -> 2260618` first because it is the latest good baseline.
4. Required result:
   - first initiating PTT still carries voice after caller `D-CONNECT` recovery;
   - close remains established-call `D-RELEASE(UserRequestedDisconnection)`;
   - peer Motorola should render normal end/disconnected state, not `No answer`;
   - no Motorola reboot, no `PTT denied`, no `Network trouble`, no BS restart.
5. If `No answer` persists, do not change release cause next. Inspect whether terminal UI saw notification indicator `19` in both connect PDUs and consider a separate `D-INFO` imminent-disconnection experiment with notification value `26` only after EN 300 392-9 semantics are rechecked.

## 2026-06-07 22:22 EEST - Private simplex peer clear cleanup and Tetra-Core/SmartConnect analysis agent

Context:

- Latest RF loop before this patch: `v0.1.57-783780c3` kept first-PTT/private audio usable, but `2260082 -> 2260618` followed by red-button close on `2260082` still soft-rebooted the MXP600 peer.
- Log evidence from that loop showed:
  - `U-DISCONNECT(UserRequestedDisconnection)` from `2260082`;
  - prompt `D-RELEASE` to the requesting MS;
  - delayed peer `D-DISCONNECT(UserRequestedDisconnection)` to `2260618`;
  - no peer `U-RELEASE`;
  - MXP600 reattach after reboot.
- Historical RF attempts showed the same two repeated bad outcomes:
  - peer `D-DISCONNECT` can reboot MXP600;
  - blind peer `D-RELEASE`/cause experiments can render terminal-visible `No Answer`.
- A new read-only agent, `Halley the 5th`, was added to inspect `/Users/ctermure/Work/Tetra-Core` SmartConnect/IP TETRA support and protocol captures as inspiration only. It must not be treated as an ETSI normative reference.

Clause-scoped ETSI check:

- EN 300 392-2 clause 14.5.1.3.1: after `U-DISCONNECT`, the requesting MS waits for `D-RELEASE`; the SwMI may inform the other MS by `D-DISCONNECT` or `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3: only `D-DISCONNECT` expects peer `U-RELEASE`.
- EN 300 392-2 clause 23.8.2.2: after `U-TX CEASED` or `U-DISCONNECT`, BS should allow bearer tail bits before `D-TX CEASED`, `D-RELEASE`, or `D-DISCONNECT`.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Local simplex peer clear after tail drain now uses the clause 14.5.1.3.1 `D-RELEASE` alternative.
  - It no longer starts the `D-DISCONNECT -> U-RELEASE` wait on local simplex release.
  - The traffic circuit still stays open until peer and requester `D-RELEASE` TxReporters complete or guard out.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - Comments now describe the actual local simplex release path: prompt requester `D-RELEASE`, tail-drain, peer `D-RELEASE`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated the full simple private workflow and MXP600 regression tests so local simplex peer clear asserts no `D-DISCONNECT`.
  - Kept `D-DISCONNECT -> U-RELEASE` tests only for duplex/fallback paths where `D-DISCONNECT` is deliberately sent.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs --locked test_simple_private_call_full_direct_setup_and_release_workflow -- --exact` passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked mxp600` passed: 2 tests.
- `cargo test -p tetra-entities --test test_cmce_bs --locked p2p` passed: 82 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked private` passed: 20 tests.
- `cargo check -p tetra-entities --locked` passed.
- `cargo fmt --package tetra-entities` completed.
- `git diff --check` passed.

Next RF gate:

1. Wait for the Tetra-Core/SmartConnect agent report and record any useful inspiration separately from ETSI requirements.
2. Commit and deploy directly to `/home/chris/nexus-bs/nexus-bs`.
3. Clear volatile journal.
4. Controlled test: `2260082 -> 2260618`, one PTT each direction, close from `2260082`.
5. Required log/UI result:
   - no peer `D-DISCONNECT` to `2260618` on local simplex close;
   - peer `D-RELEASE` only after bearer-tail drain;
   - no MXP600 soft reboot/reattach;
   - no `No Answer` if the terminal maps the established-call release correctly;
   - first PTT/media path remains unchanged from the current RF-good base.

Tetra-Core/SmartConnect agent result:

- Agent `Halley the 5th` completed read-only analysis and was closed.
- `/Users/ctermure/Work/Tetra-Core` did not contain the raw `artifacts/smartconnect/...` captures referenced by docs; it contained docs, parsers, tests, and capture tools.
- Tetra-Core itself labels SmartConnect/DIMETRA Connect as WIP/lab-only. Treat it as inspiration only, not ETSI evidence and not Motorola RF proof.
- Useful transferable idea:
  - SmartConnect private flow separates setup/provision/grant/ACK from media.
  - Media/floor starts only after the accepting side has completed setup.
  - Release/cleanup waits for queued media/release guards instead of tearing state down immediately after enqueue.
- Nexus-BS already has the analogous RF gates and must preserve them:
  - no traffic channel before called response;
  - called `D-CONNECT ACKNOWLEDGE` before caller `D-CONNECT`;
  - `FloorGranted` only after caller `D-CONNECT` delivery/L2 ACK;
  - setup reject/no-answer stays setup-phase and does not arm media/floor;
  - local simplex close with peer `D-RELEASE` must not wait for peer `U-RELEASE`.
- Not useful for the RF fix:
  - BEL byte layouts, UDP/1200 RTP PT96, SSRC/request-id formulas, `media_auth`, STUN, AES-GCM, synthetic lab auth, and native lab mode immediate grants.

## 2026-06-07 23:40 EEST - Private simplex peer release notification and Parrot/Papagal 99999 MVP

Context:

- RF user report after the last deployed private-simplex close work: Motorola still rendered `No answer` on some established-call close paths.
- Do not repeat the previous loops:
  - peer `D-DISCONNECT` on local simplex close can reboot MXP600;
  - blind release-cause changes did not prove a stable fix;
  - setup/media path around the RF-good base must remain unchanged.
- User also requested a separate Parrot/Papagal simplex service on ISSI `99999`, limited to 20 seconds, which records what the caller says, plays it back as a P2P-like response, then closes.

Clause-scoped ETSI check:

- EN 300 392-2 clause 14.5.1.3.1: the MS that sends `U-DISCONNECT` waits for `D-RELEASE`; the other MS may be informed by `D-DISCONNECT` or `D-RELEASE`.
- EN 300 392-2 clause 14.5.1.3.3: `D-RELEASE` expects no MS response; `D-DISCONNECT` expects `U-RELEASE`.
- EN 300 392-2 clause 14.5.1.2.2 f: before actual disconnection the SwMI may send `D-INFO` with notification "Notice of imminent call disconnection".
- EN 300 392-2 table 14.12 / local PDU implementation: `D-RELEASE` also carries an optional Notification indicator.
- Notification value `26` remains project evidence from EN 300 392-9 notes/tests and must not be described as full-stack certification evidence until the 392-9 source is rechecked.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added `D-INFO` imminent-disconnection helper.
  - Added `D-RELEASE` builder support for optional `notification_indicator`.
  - Local simplex caller hangup still sends prompt `D-RELEASE` to the requester without extra `D-INFO`.
  - Tail-drained peer clear now sends `D-INFO(notification=26)` before peer `D-RELEASE(notification=26)`.
  - Setup, `D-CONNECT ACK`, caller `D-CONNECT`, floor grants, and media gating were not changed.
- `crates/tetra-saps/src/control/call_control.rs`
  - Added `CircuitDlMediaSource::LocalParrot` so UMAC can distinguish Parrot media from normal group/simplex `LocalLoopback` and network `SwMI`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/parrot.rs`
  - New isolated Parrot/Papagal session implementation for ISSI `99999`.
  - Records validated incoming TMD frames, capped at 20 seconds (`18 * 20 = 360` TCH/S frames).
  - Plays frames back paced by TDMA timeslot, one frame per owned traffic opportunity.
  - Releases only the real caller leg after playback; it does not treat `99999` as a real RF peer.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Routes P2P setup to `99999` into `fsm_on_u_setup_parrot`.
  - Rejects duplex setup to `99999`; Parrot MVP is simplex-only.
  - Opens a local `LocalParrot` circuit and seeds caller floor; no Brew routing.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - On caller `U-TX CEASED`, starts Parrot playback, marks virtual source `99999`, sends `D-TX GRANTED` as "other user transmitting" to caller.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Drives Parrot playback in `tick_start`.
  - If UL inactivity fires before `U-TX CEASED`, starts playback instead of falling into generic P2P floor release.
- `crates/tetra-entities/src/umac/umac_bs.rs`
  - `LocalParrot` UL media is forwarded to CMCE as `TmdCircuitDataInd` and not looped to DL immediately.
  - `TmdCircuitDataReq` now honors `raw_tch_s_block`; raw Block2 is scheduled with the existing raw TCH/S half-slot path.
  - ACELP DL requests are normalized through existing `pack_ul_acelp_bits`, so 274 one-bit-per-byte frames and 35-byte packed frames are both handled safely.
- Tests:
  - Added CMCE Parrot setup/reject/playback/release tests.
  - Added UMAC LocalParrot ACELP/raw no-loopback tests.
  - Added raw Block2 `TmdCircuitDataReq` playback preservation test.
  - Added unit test for the 20-second Parrot recording cap.
  - Updated private-simplex close tests to assert no `D-INFO` on requester ACK and peer `D-INFO(26)` + peer `D-RELEASE(notification=26)`.

Verification:

- `cargo fmt --package tetra-saps --package tetra-entities` passed.
- `cargo check -p tetra-saps -p tetra-entities --tests --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked parrot` passed: 3 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked parrot` passed: 2 tests.
- `cargo test -p tetra-entities --lib parrot --locked` passed: 1 test.
- `cargo test -p tetra-entities --test test_umac_bs --locked test_tmd_dl_req_raw_block2_playback_preserves_tch_s_halfslot -- --exact` passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked p2p` passed: 84 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked voice` passed: 5 tests.
- `cargo test -p tetra-entities --test test_cmce_bs --locked mxp600 -- --nocapture` passed before the broader `p2p` run.
- `git diff --check` passed.

Next RF gate:

1. Build locally only; deploy directly to test Pi with the approved Nexus-BS deploy path.
2. Clear volatile journal.
3. Test normal private simplex first (`2260082`/`2260618` or current available pair):
   - first PTT/media path must remain as previous RF-good base;
   - close should show normal disconnected/end state, not `No answer`;
   - no MXP600 reboot/reattach.
4. Test Parrot:
   - call ISSI `99999`;
   - speak less than 20 seconds;
   - release PTT;
   - terminal should hear exact recorded speech replay from virtual peer `99999`;
   - call should then close from SwMI side.
5. If `No answer` persists, do not alter setup/media path or alternate `D-DISCONNECT`/`D-RELEASE` loops. Inspect the actual final downlink sequence and consider whether notification value `26` needs EN 300 392-9 revalidation or vendor-specific guarded handling.

## 2026-06-07 23:15 EEST - Parrot/Papagal RF hang recovery and floor-grant flood fix

Field result:

- RF test to ISSI `99999` made Nexus-BS unusable without a clean call release.
- Service did not crash immediately, but the old process later failed to stop gracefully and systemd killed it after stop timeout.
- Log evidence showed Parrot call `call_id=5` on TS3 recorded 141 frames and started playback, but there was no `parrot playback complete` / release cleanup.
- During recording, every validated TCH/S frame emitted a `FloorGranted` to UMAC. That flooded control state on the Parrot circuit and was not needed because setup already grants the caller floor.

Clause-scoped ETSI check:

- Parrot is a local SwMI test service on virtual ISSI `99999`; it is not a normal RF peer and does not change the protected real-terminal P2P setup/media path.
- Release still uses the existing individual-call `D-RELEASE` mechanism for the real caller leg, matching EN 300 392-2 clause 14.5.1.3.3 behavior for SwMI-side release.
- This remains clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/parrot.rs`
  - Recording remains capped at 20 seconds / 360 TCH/S frames.
  - Added playback start timestamp and a bounded playback guard (`20s + 2s`) so Parrot cannot hold a circuit indefinitely.
  - Added unit coverage for forced playback completion if the guard expires.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Removed per-frame Parrot `FloorGranted` refresh during recording. Setup already grants floor; recording should only store media.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - `U-TX CEASED` now starts Parrot playback with the current TDMA time so the guard can run.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - UL-inactivity fallback playback start also records the current TDMA time.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added RF-like test with 141 recorded frames:
    - no recording-time floor-grant flood;
    - exact playback count;
    - `D-RELEASE` to the real caller;
    - UMAC close / `CallEnded` cleanup after reporter completion.

Verification:

- `cargo fmt --package tetra-entities` passed.
- `cargo test -p tetra-entities --lib parrot --locked` passed: 2 tests.
- `cargo test -p tetra-entities --test test_cmce_bs --locked parrot` passed: 4 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked parrot` passed: 2 tests.
- `cargo check -p tetra-entities -p tetra-saps --tests --locked` passed.
- `git diff --check` passed.

Deploy:

- Built locally for AArch64; no compile on TetraHS/Pi.
- First scripted deploy hit remote stop timeout because the old stuck service did not exit; systemd killed old PID `62544`.
- Copied new binary manually to `/home/chris/nexus-bs/nexus-bs`.
- Remote SHA-256 after deploy: `6f92c0879871dd96ea0e0ebe89eac8a8e1fc56f3927668f24e4b72766f45591b`.
- Restarted `nexus-bs@chris.service`; service active/running with PID `63492`.
- Cleared volatile journal for the next RF test.

Next RF gate:

1. Test Parrot again: private simplex call to `99999`, speak under 20 seconds, release PTT.
2. Expected: no BS hang, no per-frame floor-grant flood, playback occurs, then call releases.
3. After user says `gata papagal`, inspect journal for `parrot playback complete`, release cleanup, and absence of repeated `UMAC floor granted` lines during recording.

## 2026-06-07 23:34 EEST - Parrot non-blocking playback and CPU spin hardening

Field/CPU context:

- User reported one core held near 100% after the Parrot RF test.
- Live old PID `64630` showed process CPU around `134%` earlier and a stop timeout; systemd later killed it on deploy stop after SIGTERM did not complete.
- After the final redeploy, `nexus-bs@chris.service` is active/running as PID `66946`.
- Remote binary SHA-256: `8598f46dfdd50f4d7bd270171f5b97c073231e54daf0d88f6d7c2d078a7f8c7c`.
- Post-start CPU thread sample:
  - main `nexus-bs` thread about `51.8%`;
  - `brew-worker` about `1.2%`;
  - `dashboard-log` about `2.6%`;
  - other worker threads near zero.
- Conclusion: the previous post-Parrot high CPU/futex churn is not present after this patch. The remaining main-thread CPU is the radio/TDMA loop, not an observed Parrot playback flood.

Clause-scoped ETSI check:

- Parrot ISSI `99999` remains a local SwMI test service, not a real RF peer and not part of the protected normal P2P path.
- Release still uses caller-facing individual-call `D-RELEASE` and bounded assigned-channel release cleanup, aligned with EN 300 392-2 individual call clearing principles in clause 14.5.1.3. This is engineering evidence only, not formal certification.
- Normal private P2P setup/media code was not intentionally changed by this patch.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/parrot.rs`
  - Playback remains TDMA-paced: at most one recorded frame is emitted when `dltime.t == session.ts`.
  - `finish_without_playback()` no longer marks `playback_finished`, preventing duplicate release from the fail-safe path.
  - Added `owns_ts()` so CMCE can consume late Parrot-owned UL frames without recording them.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - `handle_parrot_ul_frame()` now returns consumed for all Parrot-owned traffic-slot UL while a Parrot session exists.
  - Frames are recorded only in `Recording`; late frames in `Playing/Releasing` are consumed locally and do not leak to Brew.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - On Parrot `U-TX CEASED`, if frames exist, start paced playback instead of immediate release.
  - If no frames exist, release caller cleanly.
  - During playback, CMCE floor holder becomes virtual ISSI `99999`; UMAC receives one virtual `FloorGranted`.
  - The real caller also receives assigned-channel `D-TX GRANTED` with `GrantedToOtherUser`, so terminal floor/UI state follows the virtual peer during playback.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - UL-inactivity fallback mirrors `U-TX CEASED`: recorded frames trigger paced playback; empty recordings release cleanly.
  - The timeout fallback also sends the caller `D-TX GRANTED GrantedToOtherUser` before virtual playback.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Parrot tests now prove no same-drain playback flood, exact frame replay order/metadata, no late-UL Brew leak, caller-only release, and bounded RF-length pacing.

Verification:

- `cargo fmt --package tetra-entities` passed.
- `cargo test -p tetra-entities --lib parrot --locked` passed: 2 tests.
- `cargo test -p tetra-entities --test test_cmce_bs --locked parrot` passed: 4 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked parrot` passed: 2 tests.
- `cargo check -p tetra-entities -p tetra-saps --tests --locked` passed.
- `git diff --check` passed.

Deploy:

- Build was local only; no Rust compile on the Pi/TetraHS.
- Initial `RUN_TESTS=0 scripts/nexus-bs-test-deploy.sh` built successfully but timed out during remote service stop because the old service did not exit before systemd kill.
- Manual direct deploy then copied `target/aarch64-unknown-linux-gnu/release/nexus-bs` to `/home/chris/nexus-bs/nexus-bs`, with no backup binary.
- Final D-TX-GRANTED patch was rebuilt locally with the remembered `cargo zigbuild` command, service stopped cleanly to `inactive/dead`, binary copied directly to `/home/chris/nexus-bs/nexus-bs`, and service restarted.
- `systemctl show` reports `ActiveState=active`, `SubState=running`, `Result=success`, `MainPID=66946`.

Next RF gate:

1. User can test Parrot: private simplex call to ISSI `99999`, speak under 20 seconds, release PTT.
2. Expected: terminal hears paced playback; BS stays responsive; call releases after playback.
3. After test, inspect logs for `starting paced playback`, `parrot playback complete`, caller-facing `DRelease`, UMAC `Close`/`CallEnded`, and no CPU jump over one core.

## 2026-06-08 00:19 EEST - Parrot dashboard Calls/Last Heard telemetry

Field context:

- User reported that after Parrot tests, TS3 still showed traffic in the dashboard, and requested that the Papagal service appear in both `Calls` and `Last Heard`.
- Log evidence from the previous stage showed CMCE/UMAC closing the Parrot TS3 circuit correctly, so this stage treats the visible gap as dashboard telemetry/representation, not as an RF circuit leak.

Clause-scoped ETSI check:

- This patch does not alter air-interface PDU order, grants, media, timers, or release behavior.
- It only emits the same internal `IndividualCallStarted` observability event already used by normal private calls after the local SwMI Parrot call state is created.
- Parrot ISSI `99999` remains a local SwMI test service. This is dashboard/status evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - `fsm_on_u_setup_parrot()` now emits `TelemetryEvent::IndividualCallStarted` with caller ISSI, called ISSI `99999`, `simplex=true`, and the allocated traffic TS.
  - Normal RF P2P setup path remains untouched.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Parrot setup test now attaches a telemetry sink to CMCE and proves the `IndividualCallStarted` event is emitted.
  - The emitted event is fed through `DashboardServer::handle_telemetry()` and asserted to populate both dashboard `Calls` and `Last Heard`.

Verification:

- `cargo fmt --package tetra-entities` passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked parrot` passed: 4 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked parrot` passed: 2 tests.
- `cargo check -p tetra-entities -p tetra-saps --tests --locked` passed.
- `git diff --check` passed.

Deploy:

- Built locally for AArch64 with the remembered Nexus-BS `cargo zigbuild` command; no Rust compile on the Pi/TetraHS.
- Local SHA-256: `ce4275b80aa047eae9be3715830e488e2457e2fde7ed97742e9165d880575569`.
- Stopped `nexus-bs@chris.service` cleanly.
- Copied the binary directly to `/home/chris/nexus-bs/nexus-bs`; no remote binary backup.
- Remote SHA-256 matches local: `ce4275b80aa047eae9be3715830e488e2457e2fde7ed97742e9165d880575569`.
- Restarted `nexus-bs@chris.service`; systemd reports `ActiveState=active`, `SubState=running`, `Result=success`, `MainPID=71192`.
- Startup log showed Nexus-BS `v0.1.57` and restart recovery/registration for local ISSIs `2260082`, `2260618`, and `2260616`, with CMCE affiliations to GSSI `226333`.

RF/UI gate:

- Call ISSI `99999`.
- Expected: dashboard `Calls` shows an active individual/simplex call from the caller to `99999` while the call is active.
- Expected: dashboard `Last Heard` gets an entry from the caller ISSI to destination `99999` with activity `call_individual`.

## 2026-06-08 00:26 EEST - Parrot playback DL-only grant for virtual peer

Field context:

- User reported Parrot/Papagal audio was truncated earlier than the 20 second cap and sounded like the radio microphone/duplex echo-cancel path was active during playback.
- Live log for the latest post-deploy RF test showed:
  - `U-SETUP` to `99999` at `00:21:15.260`.
  - `U-TX CEASED` from ISSI `2260616` at `00:21:23.037`.
  - Parrot started playback with `recorded_frames=129`, which is about 7.2 seconds at 18 TCH/S frames/sec.
  - Therefore this observed playback length was caused by the terminal sending `U-TX CEASED`, not by the 20 second recording cap.
- The playback floor handoff did reveal a real bug: Parrot sent `D-TX GRANTED = GrantedToOtherUser` while also carrying channel allocation `UlDlAssignment::Both`.

Clause-scoped ETSI check:

- EN 300 392-2 clause 14.5.1.2.1 says `D-TX GRANTED` with `transmission granted to another user` switches the MS to U-plane receive, not transmit.
- For a virtual local service (`99999`) there is no RF uplink peer, so the caller-facing playback grant should keep the assigned channel downlink-only.
- Normal real-terminal P2P floor handoff remains untouched; that code still uses the existing bidirectional allocation policy for two real MS legs.
- This is clause-scoped engineering evidence only, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/uplink.rs`
  - On Parrot `U-TX CEASED`, playback `D-TX GRANTED(GrantedToOtherUser)` now carries `UlDlAssignment::Dl` instead of `Both`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - The UL-inactivity fallback path for Parrot playback now uses the same downlink-only allocation.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Parrot playback test now asserts the caller receives `D-TX GRANTED = GrantedToOtherUser` with FACCH/STCH channel allocation `Dl`.

Verification:

- `cargo fmt --package tetra-entities` passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked parrot` passed: 4 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked parrot` passed: 2 tests.
- `cargo check -p tetra-entities -p tetra-saps --tests --locked` passed.
- `git diff --check` passed.

Next deploy/test:

- Built locally and deployed directly to `/home/chris/nexus-bs/nexus-bs`.
- Local and remote SHA-256: `029aeac5cf2796cc1040ce483159752c9f1fedce0cbbd11e2e58ee17114c84ae`.
- Restarted `nexus-bs@chris.service`; systemd reports `ActiveState=active`, `SubState=running`, `Result=success`, `MainPID=71973`.
- Startup log showed restart recovery and affiliation for `2260082`, `2260616`, and `2260618` to GSSI `226333`.
- RF gate: call `99999`, speak while holding PTT, release PTT.
- Expected: playback starts only after `U-TX CEASED`; audio should sound receive-only, not duplex/echo-cancel-like.
- If the terminal still sends `U-TX CEASED` before the operator releases PTT, inspect exact MS timer/floor behavior next; do not change normal P2P.

## 2026-06-08 00:35 EEST - Parrot Hytera/Motorola hook-method normalization

Field context:

- User ran two Parrot/Papagal RF tests and reported: Hytera playback sounded bad, Motorola playback sounded OK.
- Live log on deployed SHA `029aeac5cf2796cc1040ce483159752c9f1fedce0cbbd11e2e58ee17114c84ae` showed three current-service Parrot calls after service start:
  - `00:28:22` Hytera `2260616 -> 99999`, `U-SETUP ... simplex=true hook=true`, call `4`, TS2, `recorded_frames=83`.
  - `00:28:41` Hytera `2260616 -> 99999`, `U-SETUP ... simplex=true hook=true`, call `5`, TS2, `recorded_frames=176`.
  - `00:29:10` Motorola `2260082 -> 99999`, `U-SETUP ... simplex=true hook=false`, call `6`, TS2, `recorded_frames=167`.
- All three calls used the same `LocalParrot` media path, same TS2, no Brew route, playback started after `U-TX CEASED`, and CMCE/UMAC released the circuit cleanly.
- The material signalling difference was therefore `hook_method_selection`: Hytera requested hook signalling, Motorola requested direct through-connect.

Clause-scoped ETSI check:

- EN 300 392-2 table 14.62 defines `hook_method_selection=0` as direct through-connect and `hook_method_selection=1` as hook on/off signalling.
- EN 300 392-2 clauses 14.5.1.1.1 and 14.5.1.1.2 separate direct setup from called-user alert/answer handling.
- Parrot ISSI `99999` is an automatic local SwMI test service with no real called user to answer on/off-hook; for this service only, the BS should respond as direct through-connect even if the caller requested hook signalling.
- Normal private P2P remains untouched. This is clause-scoped engineering evidence, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/setup.rs`
  - Removed a misplaced Parrot comment from the normal local setup path.
  - In `fsm_on_u_setup_parrot()`, `D-CALL PROCEEDING` and `D-CONNECT` are now explicitly sent with `hook_method_selection=false`.
  - The ETSI comment is kept only inside the Parrot handler, so normal RF P2P retains the caller/called hook method semantics.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Parrot setup test now sends a Hytera-like `U-SETUP hook_method_selection=true`.
  - Test asserts Parrot replies with `D-CALL PROCEEDING hook=false` and `D-CONNECT hook=false`.

Verification:

- `cargo fmt --package tetra-entities` passed.
- `cargo test -p tetra-entities --test test_cmce_bs --locked parrot` passed: 4 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked parrot` passed: 2 tests.
- `cargo check -p tetra-entities -p tetra-saps --tests --locked` passed.
- `git diff --check` passed.

Next deploy/test:

1. Build locally only with the remembered AArch64 Nexus-BS command or `scripts/nexus-bs-test-deploy.sh`.
2. Deploy directly to `/home/chris/nexus-bs/nexus-bs`; no remote binary backup.
3. Retest Hytera `2260616 -> 99999` first. Expected log after deploy: `U-SETUP ... hook=true`, but `CMCE: parrot service -> DConnect ... hook_method_selection: false`.
4. If Hytera playback is still bad after hook normalization, inspect raw recorded/playback frame metadata next; do not touch normal P2P while debugging Parrot.

Deploy:

- Ran `RUN_TESTS=0 scripts/nexus-bs-test-deploy.sh` after focused local tests had already passed.
- Build was local AArch64 only; no Rust compile on TetraHS/Pi.
- Old service stopped cleanly and deregistered `2260082`, `2260616`, and `2260618`.
- Copied the binary directly to `/home/chris/nexus-bs/nexus-bs`; no remote binary backup.
- Deployed commit marker: `9773aee2`.
- Local/remote SHA-256: `c944ba8223a00b659ce7b171f2dbe9f64360e4b97d0fc04c2f0997679cd2092f`.
- Restarted `nexus-bs@chris.service`; systemd reports `MainPID=73250`, `ActiveState=active`, `SubState=running`, `Result=success`, `NRestarts=0`.
- Startup log showed restart recovery and re-registration/affiliation for `2260616`, `2260082`, and `2260618` to GSSI `226333`.

Current RF gate:

1. Ask user to test Hytera `2260616 -> 99999`, speak briefly under 20 seconds, release PTT.
2. Required log evidence: `U-SETUP ... hook=true`, followed by `CMCE: parrot service -> DConnect ... hook_method_selection: false`.
3. Expected RF behavior: playback quality should match Motorola more closely; playback still starts only after `U-TX CEASED`.
4. If still bad, compare recorded/playback frame metadata and raw/ACELP packing for Hytera only; keep normal private P2P untouched.

## 2026-06-08 00:43 EEST - PHY NormalTrainSeq log flood demoted

Field context:

- User reported excessive dashboard/journal lines like:
  - `rx_tpsap_prim got NormalTrainSeq1 in fullslot`
  - `rx_tpsap_prim got NormalTrainSeq2 in fullslot`
- These are normal PHY receive burst detections on the raw radio timeslot path. At `INFO`, they can appear once per active timeslot and overload useful BS logs.

Clause-scoped ETSI check:

- No TETRA protocol behavior changed. This patch only changes logging verbosity after PHY burst detection and before the existing LMAC handoff.
- NormalTrainSeq handling, slot splitting, LMAC/UMAC routing, grants, timers, and CMCE signalling are untouched.

Patch:

- `crates/tetra-entities/src/phy/phy_bs.rs`
  - Demoted the three high-rate receive-burst logs from `tracing::info!` to `tracing::debug!`:
    - fullslot;
    - subslot1;
    - subslot2.

Verification:

- `cargo fmt --package tetra-entities` passed.
- `cargo check -p tetra-entities --tests --locked` passed.
- `git diff --check` passed.

Deploy:

- Built locally only with `RUN_TESTS=0 scripts/nexus-bs-test-deploy.sh`; no Rust compile on TetraHS/Pi.
- Copied directly to `/home/chris/nexus-bs/nexus-bs`; no remote binary backup.
- Deployed commit marker: `9773aee2`.
- Remote SHA-256: `e36c36eb511266ac3337d5f0fa5f79357e4cc2319cef88593b232aa31e92391e`.
- Restarted `nexus-bs@chris.service`; systemd reports `MainPID=73498`, `ActiveState=active`, `SubState=running`, `Result=success`, `NRestarts=0`.
- Post-restart journal grep since `2026-06-08 00:43:11` for `rx_tpsap_prim got` returned no entries.

Next:

- Continue RF Parrot test with Hytera `2260616 -> 99999`.
- If low-level PHY burst evidence is needed again, temporarily run with debug logging or add a bounded/rate-limited diagnostic, not high-rate `INFO`.

## 2026-06-08 11:50 EEST - Brew v1 wire-format hardening for external group audio

Field context:

- User reported dashboard showing `Brew v0`, but current TetraPack Brew documentation requires client-side `X-Brew-Version: 1`.
- RF group call from `2260082` to Brew-routable GSSI `22699` signalled correctly, but no audio reached Brew externally.
- Live logs showed `GROUP_TX`/floor signalling succeeded and one transmit epoch reached `frames=43`, while TetraPack returned repeated `server error type=0`.
- Brew error type `0` is `BREW_TYPE_MALFORMED`, so the failure points to Brew wire-format compatibility, not a CMCE group-floor denial.

Clause/protocol scope:

- No ETSI air-interface CMCE/UMAC/P2P/Parrot behavior was changed.
- ETSI-side behavior remains the existing EN 300 392-2 group-call floor model: local RF group calls notify Brew on `FloorGranted`; user-plane media is forwarded only after valid decoded ACELP TCH/S.
- Changed only the external Brew/TetraPack interconnect serialization and dashboard-reported negotiated Brew version.

Patch:

- `crates/tetra-entities/src/network/transports/websocket.rs`
  - WebSocket transport now reports requested Brew v1 by default instead of v0/unknown.
  - WebSocket upgrade request now includes `X-Brew-Version: 1` and `X-Brew-Mode: Basestation`, matching the discovery GET headers.
- `crates/tetra-entities/src/net_brew/protocol.rs`
  - Added `EMPTY_BREW_MNEMONIC`.
  - `GROUP_TX` v1 with a zero-length mnemonic is still parsed as v1 when the 34-byte field is present.
  - `SETUP_REQUEST` now always serializes the v1 34-byte mnemonic field; `CONNECT_REQUEST` remains without mnemonic.
- `crates/tetra-entities/src/net_brew/worker.rs`
  - Outbound `GROUP_TX` now sends the v1 layout with the empty 34-byte mnemonic field.
- `crates/tetra-entities/src/net_brew/entity.rs`
  - Outbound voice frames are now 36-byte STE frames with header bit 7 set (`0x80`) and final unused tail bits set (`0x3f`).
  - Brew voice `length_bits` now describes the full 36-byte STE payload (`288` bits), not only the 274 ACELP speech bits.
- Release identity bumped from `v0.1.58` to `v0.1.59` across workspace metadata, dashboard/control/telemetry protocol strings, README and example config.

Verification:

- `cargo fmt --all` passed.
- `git diff --check` passed.
- `cargo test -p tetra-entities --lib network::transports::websocket --locked` passed.
- `cargo test -p tetra-entities --lib net_brew --locked` passed: 31 tests.
- `cargo test -p tetra-core --locked product_identity_tracks_workspace_version` passed.
- `cargo test -p tetra-entities --lib net_control --locked` passed: 22 tests.
- `cargo test -p tetra-entities --lib net_telemetry --locked` passed: 8 passed, 5 ignored.
- `cargo test -p tetra-entities --lib net_dashboard --locked` passed: 53 tests.
- `cargo check -p tetra-core -p tetra-entities --tests --locked` passed.

Next deploy/test:

1. Deploy with `scripts/nexus-bs-test-deploy.sh` only; build locally, deploy directly to `/home/chris/nexus-bs/nexus-bs`, no remote binary backup.
2. Confirm dashboard shows `Brew v1` after reconnect.
3. RF test: `2260082` group call to GSSI `22699`; expected Brew logs should no longer show repeated `server error type=0` for 56-byte voice frames.
4. If `frames=0` still appears, inspect whether LMAC produced only raw Block2 half-slot media for that transmit epoch; do not change P2P/Parrot while debugging this.

Deploy:

- Amended release commit after finding stale `v0.1.58` in systemd sample descriptions.
- Updated live `/etc/systemd/system/nexus-bs@.service` description to `Nexus-BS v0.1.59 TETRA base station service for %i` and ran `systemctl daemon-reload`.
- Built locally only with `RUN_TESTS=0 scripts/nexus-bs-test-deploy.sh` after the focused local tests above had passed.
- Copied directly to `/home/chris/nexus-bs/nexus-bs`; no remote binary backup.
- Deployed commit marker: `9a75c837`.
- Remote SHA-256: `67ac6c0a1141c09a390d5000794994eb7f13ff821b15cb90270b07a491f236d1`.
- Restarted `nexus-bs@chris.service`; systemd reports `MainPID=77876`, `ActiveState=active`, `SubState=running`, `ActiveEnterTimestamp=Mon 2026-06-08 11:54:24 EEST`.
- Startup banner reports `Version: Nexus-BS v0.1.59`, `Build: v0.1.59-9a75c837`.
- Post-start Brew log reports `WebSocketTransport: connected, using Brew v1` and `BrewEntity: backhaul CONNECTED`.

RF gate:

1. User should retest `2260082` group call to GSSI `22699`.
2. Expected: dashboard reports Brew v1; no repeated `BREW_TYPE_MALFORMED`/`server error type=0` on transmitted voice frames.
3. If audio still fails but malformed errors disappear, inspect the LMAC full-slot ACELP vs raw Block2 path for that RF epoch.

## 2026-06-08 14:18 EEST - Energy Economy auto policy

Field context:

- User asked why `2260082` and `2260618` showed EE/EG behavior and whether EE only negotiates at power-on.
- Live logs showed two distinct cases:
  - With live config `energy_saving_mode = "eg7"`, Nexus-BS imposed BS-initiated EG7 after registration for stations that did not request EE, then T352 could expire and fall back to StayAlive.
  - During a later registration, a terminal requesting `Eg1` was allocated configured `Eg7`, which is ETSI-permitted but not the desired operator policy for mixed real terminals.
- User decision: EE must be `auto`; BS must accept what the terminal requests, support all modes, and not impose an EE mode.

Clause scope:

- EN 300 392-2 clause 16.7.1 permits the BS to allocate a different energy saving mode than requested, but does not require it.
- Nexus-BS `auto` is a local operational policy over the ETSI procedure: echo the terminal-requested StayAlive/EG1..EG7 value; when the terminal does not request EE, keep StayAlive and do not send a BS-initiated D-CHANGE OF ENERGY SAVING MODE REQUEST.
- Clauses 16.10.9/16.10.10 define energy saving mode/information; clause 23.7.6/table 23.9 defines EG1..EG7 receive cycles and T.210 return-to-sleep behavior.

Patch:

- `crates/tetra-config/src/bluestation/sec_cell.rs`
  - Added `ENERGY_SAVING_MODE_AUTO = 255` as internal config sentinel.
  - Default energy saving mode is now auto.
  - Parser accepts `"auto"`, `"terminal"`, `"terminal_request"`, `"ms"`, and `"ms_request"`.
  - Explicit `stay_alive`/`off`/`0` and `eg1..eg7`/`1..7` remain supported.
- `crates/tetra-entities/src/mm/mm_bs.rs`
  - Configured EE now maps to `Option<EnergySavingMode>`: `None` means auto.
  - Auto accepts the MS-requested mode in `U-LOCATION UPDATE DEMAND` or `U-MM STATUS`.
  - Auto keeps a new/no-request MS in StayAlive and does not enqueue BS-initiated EG/T352.
  - Explicit configured EG still forces BS-initiated allocation for controlled lab tests.
- `example_config/config.toml` and `README.md`
  - Default documented config changed from `eg3` to `auto`.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Added tests proving example config auto does not impose EG without an MS request.
  - Added tests proving example config auto accepts MS-requested EG1 in the location update accept.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated example config private-call sanity test to expect auto EE instead of EG3.

Verification:

- `cargo fmt --all` passed.
- `cargo test -p tetra-config --lib energy_saving_mode --locked` passed: 3 tests.
- `cargo test -p tetra-entities --test test_mm_bs example_config_auto_energy_saving --locked` passed: 2 tests.
- `cargo test -p tetra-entities --test test_mm_bs bs_initiated_energy_saving --locked` passed: 7 tests.
- `cargo test -p tetra-entities --test test_cmce_bs example_config_simple_private_call --locked` passed: 1 test.
- `cargo check -p tetra-config -p tetra-entities --tests --locked` passed.
- `git diff --check` passed.

Deploy:

- Built locally only with `RUN_TESTS=0 scripts/nexus-bs-test-deploy.sh`; no Rust compile on TetraHS/Pi.
- First deploy used the modified worktree build for RF validation, then live config was changed from `energy_saving_mode = "eg7"` to `"auto"` in `/home/chris/nexus-bs/config.toml`.
- Clean committed redeploy followed with the same script, directly to `/home/chris/nexus-bs/nexus-bs`; no remote binary backup.
- Restarted `nexus-bs@chris.service`; systemd reports `ActiveState=active`, `SubState=running`.
- Startup banner after the clean redeploy no longer showed `-modified`.
- Live post-restart behavior with config `auto`:
  - `2260082` and `2260618` registered/affiliated without `MM: allocating energy saving mode Eg7 ... after registration`.
  - `2260616` explicitly requested `Eg7`; Nexus-BS logged `MS 2260616 requested energy saving mode Eg7; accepting`.

Next:

1. Watch RF/dashboard EE display: `2260082`/`2260618` should remain StayAlive unless they request EG; `2260616` may show EG7 because it requested EG7.
2. Continue P2P/group-call validation without touching the protected private-simplex setup path unless new RF evidence requires it.

## 2026-06-08 15:30 EEST - RF glitch log audit: frame 18 ruled down, periodic registration route preserved

Field context:

- User asked to inspect live logs because recent RF glitches might be related to frame 18.
- Live service `nexus-bs@chris.service` had been running since `2026-06-08 14:21:01 EEST`, build `v0.1.59-6a846240`.
- Live config had `energy_saving_mode = "auto"`, `periodic_registration_secs = 3600`, and `frame_18_ext = true`.

Log findings:

- No live journal entries from the last restart matched `frame_18`, `frame 18`, or direct frame-18 failure markers.
- `Failed parsing MacAccess: BufferEnded { field: Some("ssi") }` appeared only a few times shortly after startup and around `14:37-14:39`; this looks like malformed/random-access RF decode noise, not direct frame-18 evidence.
- The repeated `CircuitMgr: dropping oldest queued DL ACELP block ... media queue reached 18 block(s)` around `15:18:14-15:18:18` is not frame 18. That `18` is the per-timeslot DL media queue cap.
- The same window showed Brew jitter/backlog and `rx_tmd_prim: dropping DL voice on inactive circuit ts=2 src=Brew`, meaning IP/Brew voice arrived after the UMAC circuit had closed.
- At `15:21:02`, `2260616` hit the local periodic-registration watchdog while a GSSI/Brew call was active. The old first-expiry path sent `D-LOCATION-UPDATE-COMMAND` and immediately published `Deaffiliate` + `Deregister`, which could make CMCE/Brew drop listeners or active group calls before the 60 s grace window expired.

Frame-18 audit:

- `frame_18_ext` is present in config, but UMAC does not advertise full frame-18 receive support in MAC-SYNC unless the stack supports it.
- Assigned-channel traffic treats frame 18 as non-traffic; TCH/S and FACCH/STCH wait for a non-frame-18 opportunity.
- Brew and Parrot playout skip frame 18; Energy Economy assignment avoids start/recurrent receive windows on frame 18.
- Clause-scoped reasoning: EN 300 392-2 21.4.6.5/21.4.7.2 frame-18 signalling and 23.7.6/T.210 energy-economy scheduling are handled conservatively here. This is not formal certification evidence.

Patch:

- `crates/tetra-entities/src/mm/mm_bs.rs`
  - First local periodic-registration expiry now sends `D-LOCATION-UPDATE-COMMAND`, abandons stale SwMI group transactions, and marks the 60 s pending window, but preserves the shared subscriber/GSSI route in CMCE/Brew.
  - Final removal remains on second expiry/no response: `D-LOCATION-UPDATE-REJECT(ExpiryOfTimer)`, energy-saving cleanup, subscriber deaffiliate/deregister.
- `crates/tetra-entities/tests/test_mm_bs.rs`
  - Updated periodic-registration tests so first expiry must preserve registration and group membership.
  - Kept grace-expiry coverage proving final reject/removal still deaffiliates/deregisters after no response.

ETSI clause scope:

- EN 300 392-2 clause 16.4.4 permits SwMI-initiated location update using `D-LOCATION-UPDATE-COMMAND`.
- Clauses 16.9.2.8 and 16.9.3.4 cover the MS response to that command.
- Clause 16.8.x group identity validity supports keeping accepted persistent groups valid until explicit detach/replacement or real timeout/removal; a first refresh command is not itself proof that the MS left the cell.

Verification:

- `cargo fmt --all` passed.
- `cargo test -p tetra-entities --test test_mm_bs periodic_registration --locked` passed: 3 tests.
- `cargo test -p tetra-entities --test test_mm_bs periodic_command --locked` passed: 3 tests.
- `cargo test -p tetra-entities --test test_mm_bs group_less_coverage_return --locked` passed: 1 test.
- `cargo test -p tetra-entities --test test_mm_bs group_report_request_restores --locked` passed: 1 test.
- `cargo test -p tetra-entities --test test_mm_bs frame_18 --locked` passed: 3 tests.
- `cargo test -p tetra-entities --test test_umac_bs frame_18 --locked` passed: 4 tests.
- `cargo test -p tetra-entities --test test_mm_bs --locked` passed: 146 tests.
- `cargo check -p tetra-entities --tests --locked` passed.
- `git diff --check` passed before timeline update.

Next:

1. Commit and deploy this MM watchdog fix.
2. Re-test live GSSI/Brew RF around the 3600 s periodic-registration boundary or temporarily lower `periodic_registration_secs` in a controlled test to prove no listener drop occurs during the grace window.
3. Continue watching Brew jitter separately; queue cap `18 block(s)` is a media-buffer limit, not direct frame-18 evidence.

## 2026-06-08 18:06 EEST - Frame-18 extension disabled in live/example config for RF isolation

Field context:

- User reported local `226333` group calls stopped flowing during three-terminal ping-pong testing and asked whether frame 18 could be involved.
- Live logs for `226333` showed CMCE affiliation and floor grants were present, but UMAC repeatedly hit `UL inactivity timeout` after `D-TX GRANTED`; this points to missing/late valid UL TCH/S media after floor grant, not to a proven frame-18-extension advertisement fault.
- To remove frame-18 ambiguity from live RF testing, the live config and example config now explicitly disable `frame_18_ext`.

Patch/config:

- `example_config/config.toml`
  - Added `frame_18_ext = false` with a note that full all-slot frame-18 receive support must be implemented and verified before enabling.
- `/home/chris/nexus-bs/config.toml`
  - Changed live `frame_18_ext = true` to `frame_18_ext = false`.
  - Restarted `nexus-bs@chris.service`; post-restart PID `80964`, service active/running.
  - Post-restart live log showed `2260082`, `2260616`, and `2260618` registering/affiliating to `226333`.

Clause scope:

- EN 300 392-2 clause 21.4.6.5 frame-18 extension is kept disabled as the conservative operator default.
- This does not claim formal certification; it simply removes an optional frame-18 behaviour from the test profile while the group-call U-plane timeout is investigated.

Verification:

- `cargo test -p tetra-config --lib bluestation::parsing --locked` passed: 29 tests.
- `cargo test -p tetra-entities --test test_cmce_bs example_config_simple_private_call --locked` passed: 1 test.
- `git diff --check` passed before this timeline entry.

Next:

1. Retest local `226333` group ping-pong after the `18:05:40 EEST` restart.
2. If calls still stop, focus on UMAC/LMAC U-plane after `D-TX GRANTED`: floor is granted, but valid TCH/S is not arriving before the local inactivity guard.

## 2026-06-08 19:07 EEST - 2260082 group-floor self-demotion guard for local groups

Field context:

- User reported that `2260082` "died" when entering the `226333` group call.
- Live service did not crash: `nexus-bs@chris.service` remained active with PID `80964`.
- Live logs since the `18:05:40 EEST` restart showed the critical pattern:
  - `2260616` started local GSSI `226333`, call `4`, on TS2.
  - `2260082` sent `U-TX DEMAND`.
  - CMCE sent individual `D-TX GRANTED(Granted)` to `2260082`.
  - UMAC entered `FloorGranted` for `2260082`.
  - On later `2260082` entries, no valid UL TCH/S arrived before the local guard, and UMAC raised `UL inactivity timeout`, forcing TX ceased.
- Live config already had `energy_saving_mode = "auto"` and `frame_18_ext = false`, so this incident was not treated as BS-imposed EG7 or frame-18-extension behavior.

Clause-scoped reasoning:

- EN 300 392-2 clause 14.5.2.2.1 covers group floor control using `D-TX GRANTED`; clause 23.5 covers assigned-channel FACCH/STCH delivery.
- The SwMI must grant the new transmitting MS and inform listeners. For local small groups, sending a GSSI `D-TX GRANTED(GrantedToOtherUser)` immediately after the speaker's individual positive grant can also be received by the newly granted MS because it is still a group member.
- Nexus-BS now keeps the positive speaker grant individual, then informs only the other local listeners individually for groups up to the supported local fanout cap. This is a compatibility-safe delivery change over the same floor-control procedure; it is not formal ETSI certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added `send_group_listener_d_tx_granted_facch`.
  - For local speakers with up to `100` local listeners, sends `GrantedToOtherUser` as individual listener FACCH/STCH copies, excluding the newly granted speaker.
  - For more than `100` listeners or non-local source traffic, keeps the old one-GSSI listener grant to avoid unbounded fanout.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Group handoff/current-speaker/timeout paths now use the listener-safe helper.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated existing small-group expectations from one GSSI listener grant to per-ISSI listener grants.
  - Added `226333` three-local-member regression: `2260616`, `2260082`, and `2260618`; handoff to `2260082` must not include GSSI/self `GrantedToOtherUser`.
  - Added threshold regression: `source + 100 listeners` uses individual listener grants and no GSSI; `source + 101 listeners` intentionally uses the bounded GSSI fallback.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs group_226333 --locked` passed: 2 tests.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_226333_three_local_members_listener_grants_exclude_new_speaker --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs test_group_local_listener_floor_grant_fanout_threshold --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs group --locked` passed: 71 tests.
- `cargo test -p tetra-entities --test test_umac_bs group_floor --locked` passed: 6 tests.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` passed: 84 tests.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` passed: 11 tests.
- `cargo check -p tetra-entities --tests --locked` passed.

RF gate:

1. Deploy this build locally-built only, then restart `nexus-bs@chris.service`.
2. Test exact sequence on GSSI `226333`:
   - `2260616` starts/holds group floor.
   - `2260082` presses PTT while `2260616` holds floor and is queued.
   - `2260616` releases PTT.
   - `2260082` should enter with voice, no static-only TX, no `PTT denied`, no `UL inactivity timeout`.
3. After RF test, inspect logs for:
   - individual positive grant to `2260082`;
   - no immediate GSSI listener `GrantedToOtherUser` for the same local handoff;
   - valid TCH/S from `2260082` before the inactivity guard.

Limit:

- The local-speaker self-demotion guard is guaranteed only up to `100` local listeners. Above that, the old GSSI listener notification remains intentionally enabled for bounded airtime/resource use.

## 2026-06-08 19:21 EEST - Bounded current-speaker regrant after group UL inactivity

Field context:

- After deploying commit `ad85935`, the live RF log confirmed the intended local-listener delivery shape:
  - `2260082` received individual `D-TX GRANTED(Granted)`.
  - `2260618` and `2260616` received individual `D-TX GRANTED(GrantedToOtherUser)`.
  - No immediate GSSI/self `GrantedToOtherUser` appeared for that local `226333` handoff.
- The call still failed for `2260082`: after the positive grant, no valid UL TCH/S was accepted before the watchdog, and UMAC emitted `UL inactivity timeout`.
- This means the previous patch removed the self-demotion candidate, but not the missed/corrupted grant or late uplink-start case.

Clause-scoped reasoning:

- EN 300 392-2 clause 14.5.2.2.1 requires the SwMI to grant transmit permission before the MS starts U-plane traffic.
- EN 300 392-2 clause 23.5.2.2.7 says that if the BS does not receive an uplink message after an individual grant, the BS may send another slot granting PDU to the same MS because the MS may have missed the downlink grant or the uplink may have been corrupted.
- Nexus-BS now applies this as a bounded group-call robustness guard: one regrant per current group floor epoch, only when there is no queued requester. If a queued requester exists, the existing ETSI floor handoff behavior remains unchanged.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/call.rs`
  - Added `ul_inactivity_regrant_used` to `ActiveCall`.
  - Resets on `grant_floor` and `enter_hangtime`.
  - Allows exactly one current-speaker regrant per floor epoch.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - On group `UL inactivity` with no queued requester, first timeout re-sends individual `D-TX GRANTED(Granted)` to the current speaker and sends internal `FloorGranted` to UMAC to restart the local guard.
  - It does not repeat listener notifications or D-INFO because the floor owner has not changed.
  - A second timeout in the same floor epoch still sends `D-TX CEASED`/`FloorReleased`.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Added `test_group_ul_inactivity_regrants_current_speaker_once_before_tx_ceased`.
  - Kept `test_group_ul_inactivity_hands_floor_to_queued_requester` unchanged for queued waiter handoff.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs test_group_ul_inactivity_regrants_current_speaker_once_before_tx_ceased --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs group_ul_inactivity --locked` passed: 2 tests.
- `cargo test -p tetra-entities --test test_cmce_bs group --locked` passed: 72 tests.
- `cargo test -p tetra-entities --test test_cmce_bs p2p --locked` passed: 84 tests.
- `cargo test -p tetra-entities --test test_umac_bs group_floor --locked` passed: 6 tests.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` passed: 11 tests.

RF gate:

1. Deploy this patch and repeat `226333` with `2260082` holding/re-entering PTT.
2. Expected log if first grant is missed:
   - first timeout: `regranting current speaker ISSI 2260082 before forced TX ceased`;
   - no immediate `D-TX CEASED`;
   - if the regrant is decoded, valid TCH/S should arrive before the second timeout.
3. If the second timeout still occurs with no TCH/S, next layer is RF/LMAC decode or Motorola grant decode timing, not CMCE floor ownership.

## 2026-06-08 19:38 EEST - UMAC diagnostic counter for post-grant UL media

Field context:

- Build `v0.1.59-484113c0` was deployed and immediately exercised on local GSSI `226333`.
- The log showed `2260082` receiving the intended individual positive `D-TX GRANTED`, followed by the bounded current-speaker regrant.
- No valid UMAC voice route appeared before the second inactivity timeout, so the remaining fault is below CMCE floor ownership: either STCH/FACCH grant delivery timing, LMAC/PHY decode/classification, or real RF/uplink timing for `2260082`.

Clause-scoped reasoning:

- EN 300 392-2 clause 14.5.2.2.1 keeps the group floor under SwMI control; the SwMI must not infer successful transmission merely because it queued a grant.
- EN 300 392-2 clauses 23.5 and 23.8 require assigned-channel STCH/FACCH signalling and TCH/S media to be kept distinguishable. The next RF test therefore needs evidence of whether UMAC accepted any valid UL media after the floor grant.

Patch:

- `crates/tetra-entities/src/umac/umac_bs.rs`
  - Added `ul_media_events_since_floor`, a bounded per-timeslot diagnostic counter.
  - Reset the counter on circuit open/close, floor release, call end, and each `FloorGranted`.
  - Increment the counter only after UMAC validates a UL voice indication from LMAC as ACELP or raw TCH/S Block2.
  - Include the counter in `UL inactivity timeout` warnings as `accepted_ul_media_since_floor=...`.

Verification:

- `cargo fmt --all --check` passed.
- `cargo test -p tetra-entities --test test_umac_bs group_floor --locked` passed: 6 tests.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` passed: 11 tests.
- `cargo test -p tetra-entities --test test_cmce_bs group_ul_inactivity --locked` passed: 2 tests.
- `cargo check -p tetra-entities --tests --locked` passed.

RF gate:

1. Deploy this diagnostic build and repeat GSSI `226333` with `2260082`.
2. If timeout shows `accepted_ul_media_since_floor=0`, the BS did not accept any valid TCH/S after grant; inspect LMAC/PHY grant delivery and uplink decode.
3. If timeout shows `accepted_ul_media_since_floor>0`, the BS accepted media but did not refresh/reroute it correctly; inspect UMAC deferred raw TCH/S flushing and DL scheduling.

## 2026-06-08 22:51 EEST - Group floor grant reporter gate for rapid 2260082 PTT

Field context:

- User reported that two rapid PTT presses from Motorola `2260082` on local GSSI `226333` could kill group voice / leave static.
- Previous RF logs showed `D-TX GRANTED(Granted)` to `2260082` and immediate internal UMAC `FloorGranted`, followed by `accepted_ul_media_since_floor=0`.
- The concrete race is CMCE/UMAC ordering: CMCE was opening the local U-plane as soon as it queued the positive grant, before the requester-positive `D-TX GRANTED` was actually transmitted by UMAC/STCH.

Component explanation:

- CMCE group floor control is the call-control state machine that decides which ISSI is allowed to talk in a group call.
- `D-TX GRANTED(Granted)` is the air-interface permission sent to the requesting terminal.
- UMAC `FloorGranted` is Nexus-BS internal state that lets the lower layers accept and route that terminal's uplink voice.
- For rapid PTT retakes, UMAC must not accept speech before the positive grant has really left the BS on RF.

Clause-scoped reasoning:

- EN 300 392-2 clause 14.5.2.2.1 makes group transmission permission a SwMI-controlled floor-control decision carried by `D-TX GRANTED`.
- EN 300 392-2 clauses 23.5 and 23.8.5 require assigned-channel signalling and speech bearer timing to remain ordered.
- This patch is clause-scoped engineering hardening for local group floor control only. It is not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/mod.rs`
  - Added `PendingGroupFloorActivation`.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added `queue_group_floor_activation()` and `drain_pending_group_floor_activations()`.
  - Emits UMAC/Brew `FloorGranted` only after a positive requester `D-TX GRANTED` `TxReporter` reports transmitted.
  - Suppresses the internal floor activation if the positive grant is discarded.
  - Cancels pending activation on group release or new group TX-ceased tail drain.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - Local group positive grant paths now create a reporter and queue the floor activation:
    current-speaker reassert, preemptive group grant, hangtime/no-active-speaker retake, and queued requester handoff.
  - Same-speaker rapid retake during a pending group `U-TX CEASED` tail is answered as `RequestQueued`; positive grant is deferred until the tail drain completes.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Drains pending group floor activations at CMCE tick start.
  - Suppresses late-entry `D-SETUP` resend and UL inactivity timeout while a group floor activation is pending.
  - Group UL-inactivity handoff/regrant now uses the same reporter-gated activation.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated group tests to assert: positive `D-TX GRANTED` first, no immediate UMAC `FloorGranted`, then `FloorGranted` after reporter transmission.
  - Added/kept RF-specific regressions for `226333`, rapid same-speaker retake, repeated `U-SETUP`, large FIFO groups, deaffiliation, withdrawal, network-hangtime local retake, and P2P non-regression.
- `crates/tetra-entities/tests/test_umac_bs.rs`
  - Added same-speaker group retake media-path guard proving UMAC/LMAC reopens the traffic/TP uplink path after `FloorGranted`.

Read-only agent audit:

- ETSI/UMAC audit confirmed no remaining early `FloorGranted` in active local group positive `D-TX GRANTED` paths.
- Remaining direct `FloorGranted` paths are outside this local group-positive-grant scope: initial group setup, network-origin group speaker start, echo/parrot, and protected private-simplex/P2P paths.
- P2P/private simplex was intentionally not changed because protected RF-good checkpoints `63c3b2f` / `87b8f11` remain the baseline for private simplex.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs --locked` passed: 179 tests.
- `cargo test -p tetra-entities --test test_umac_bs --locked` passed: 88 tests.
- `cargo test -p tetra-entities --test test_umac_bs test_group_same_speaker_floor_retake_reopens_ul_traffic_for_lmac_tch_s_decode --locked` passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.
- `rustfmt --edition 2024` was run on the touched Rust files.

Next RF gate after deploy:

1. Clean/restart deployed BS and test local GSSI `226333` with `2260082`, `2260616`, and `2260618`.
2. Required field behaviour: rapid 2x PTT from `2260082` must not leave static/no-voice and must not kill the group floor state.
3. Watch for `accepted_ul_media_since_floor` after each grant. If it remains `0`, continue below CMCE at real RF grant decode / LMAC / PHY timing; the local group positive-grant race is now guarded by reporter delivery.

Commit/deploy:

- Committed as `8344a89d` (`Gate group floor activation on grant transmission`).
- Built locally only with `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`; no Rust compile on TetraHS/Pi.
- Deployed directly to `/home/chris/nexus-bs/nexus-bs`; no remote binary backup.
- Remote SHA-256: `374c973ef0e80c46681eb5e9ac12c91174491ce04235a8daaa8b832c4e1460e5`.
- Restarted `nexus-bs@chris.service`; systemd reports `MainPID=83769`, `ActiveState=active`, `SubState=running`, `ActiveEnterTimestamp=Mon 2026-06-08 22:53:24 EEST`.
- Startup banner shows `Build: v0.1.59-8344a89d`; `WebSocketTransport: connected, using Brew v1`; restart recovery restored local ISSIs `2260616`, `2260082`, and `2260618` with GSSI `226333`.
- Journal was rotated/vacuumed after deploy for a clean RF gate; `journalctl -u nexus-bs@chris.service -n 20` returned `-- No entries --`.

## 2026-06-09 00:32 EEST - Upstream FlowStation GSSI group-call comparison

Scope:

- Reloaded ETSI compliance law before protocol analysis.
- Assigned telecom explorer agent to compare upstream FlowStation clone `/private/tmp/flowstation-upstream-compare` at `fcac34e` against local Nexus-BS `95e9a81`.
- Read-only comparison only; no code changed.

Clause-scoped comparison:

- EN 300 392-2 clause 14.5.2.2.1(b): group floor authorization is `D-TX GRANTED`.
  - Upstream emits internal `FloorGranted` immediately after enqueueing the grant.
  - Nexus-BS waits for the positive requester grant `TxReporter` before UMAC/Brew floor activation.
  - Local ordering is safer; do not restore upstream here.
- EN 300 392-2 clauses 21.4.3.1 and 23.5: RA ACK/AACH/channel allocation must be coherent with assigned-channel usage.
  - Upstream STCH grant path has simpler RA handling and no requester-specific uplink-capable channel allocation in the stolen `MAC-RESOURCE`.
  - Nexus-BS carries `UlDlAssignment::Both` for the positive requester grant and only uses pending/ready RA ACK state.
  - Local path is safer for real terminals.
- EN 300 392-2 clauses 23.8.4.1.4 and 23.8.5: NTS2 Block2 remains TCH/S unless first-half MAC says it is stolen; bearer tail ordering must be preserved.
  - Upstream ignores partial TCH/S and cannot preserve raw Block2.
  - Nexus-BS forwards valid raw Block2 and tail-drains group `U-TX CEASED` before `FloorReleased`.
  - Local path is safer.
- EN 300 392-2 clause 14.5.2.2.2(c): `D-INFO reset T310` is timer signalling, not transmit authorization.
  - Upstream has no comparable group timer reset path.
  - Nexus-BS sends timer-only `D-INFO reset T310` after grants, with on-air channel allocation/usage marker stripped.
  - This remains the only credible compatibility A/B suspect if RF logs show grants are transmitted but Motorola terminals do not enter valid uplink.

Conclusion:

- Upstream FlowStation does not contain a safer GSSI group-call implementation to restore wholesale.
- Keep the local Nexus-BS CMCE/UMAC/LMAC hardening.
- Next evidence gate is RF logging from current deploy: correlate `UMAC RF diag: selected STCH D-TX GRANTED`, `LMAC RF diag: armed post-grant UL window`, post-grant candidates/results, and UMAC `first accepted UL media` or inactivity timeout.
- If grants/AACH are coherent and no valid post-grant UL appears, test a default-off compatibility switch to suppress/defer group `D-INFO reset T310` after positive floor grant. Label that as terminal compatibility, not ETSI baseline.

## 2026-06-08 21:20 EEST - Group U-TX CEASED tail-drain before FloorReleased

Field context:

- After commit `09c71fc`, RF logs showed group call `226333` floor grants and valid deferred raw TCH/S candidates, followed by repeated `UMAC: dropped deferred private raw TCH/S ... because floor released; U-plane enters hangtime`.
- The wording is from a shared UMAC queue, but the affected path was GSSI group call media.
- This made short or boundary PTTs vulnerable to static/silence: CMCE sent `FloorReleased` in the same control drain as `U-TX CEASED`, so UMAC correctly purged pending media before the final TCH/S half-slot could be scheduled to DL.

Clause-scoped reasoning:

- EN 300 392-2 clause 14.5.2.2.1 e) defines the group end-of-transmission path: the MS sends `U-TX CEASED`, and the SwMI ends the current transmission / floor state.
- EN 300 392-2 clauses 23.8.2.2 and 23.8.5 require bearer tail ordering to be preserved around circuit-mode speech/data transitions.
- EN 300 392-2 clause 23.8.4.1.4 keeps NTS2 Block2 as TCH/S unless explicitly stolen by the first-half MAC header. Therefore CMCE must not force UMAC into hangtime before a deferred non-stolen TCH/S Block2 has a bounded chance to flush.
- This is clause-scoped engineering hardening, not formal ETSI/TETRA certification.

Patch:

- `crates/tetra-entities/src/cmce/subentities/cc_bs/mod.rs`
  - Added `PendingGroupTxCeasedTailDrain`, separate from private simplex state.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/shared.rs`
  - Added a group TX-ceased tail-drain queue with a short N=4-equivalent guard.
  - Completion either grants the first still-affiliated queued requester, or enters hangtime and sends `D-TX CEASED` + UMAC/Brew `FloorReleased`.
  - Cancels stale group tail-drains on release, same-speaker reassert, pre-emptive/network floor movement.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/fsm/group.rs`
  - No-queue `U-TX CEASED` now starts tail-drain instead of immediate hangtime/FloorReleased.
  - Existing queued-requester handoff path stays immediate and unchanged.
- `crates/tetra-entities/src/cmce/subentities/cc_bs/timers.rs`
  - Drains pending group TX-ceased tails at CMCE tick start.
  - Ignores UL inactivity on a call while a group TX-ceased tail-drain is pending.
- `crates/tetra-entities/tests/test_cmce_bs.rs`
  - Updated no-queue group TX-ceased tests to expect delayed `D-TX CEASED`/`FloorReleased`.
  - Added `test_group_tx_ceased_tail_drain_then_grants_requester_queued_during_tail`.
  - Updated hangtime and large-FIFO tests to distinguish tail-pending from true hangtime.

Verification:

- `cargo test -p tetra-entities --test test_cmce_bs group_ --locked` passed: 73 tests.
- `cargo test -p tetra-entities --test test_umac_bs group_ul_raw_block2 --locked` passed: 2 tests.
- `cargo test -p tetra-entities --test test_umac_bs group_floor --locked` passed: 6 tests.
- `cargo test -p tetra-entities --test test_cmce_bs simplex_p2p --locked` passed: 13 tests.
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked` passed: 11 tests.
- `cargo check -p tetra-entities --tests --locked` passed.
- `cargo fmt --package tetra-entities -- --check` passed.
- `git diff --check` passed.

RF gate:

1. Deploy and retest local GSSI `226333` with fast ping-pong PTT from `2260616`, `2260618`, and `2260082`.
2. Expected improvement: no more dropped deferred raw TCH/S immediately caused by `FloorReleased` after `U-TX CEASED`.
3. If `accepted_ul_media_since_floor=0` still appears for `2260082`, continue below CMCE at grant decode / LMAC / RF timing, because CMCE no longer purges the tail immediately.

## 2026-06-09 - Runtime/dashboard P0 hardening and external dashboard module

Scope:

- Reloaded ETSI compliance law and EG/SwMI resume before continuing. This patch
  is runtime/dashboard/backhaul infrastructure, not a new air-interface protocol
  claim and not formal TETRA certification.
- Spawned three read-only specialist reviews: runtime QA, dashboard/API
  decoupling review and TETRA protocol safety guard.

Patch:

- `bins/nexus-bs/src/main.rs`
  - Dashboard config APIs edit persistent `NEXUS_BS_PERSISTENT_CONFIG` while the
    RF core may run a volatile `/run/.../config.toml` copy.
  - External dashboard static assets can come from `[dashboard].static_dir` or
    `NEXUS_BS_DASHBOARD_STATIC_DIR`.
  - systemd watchdog helper is started and `READY=1` / `STOPPING=1` are sent.
- `crates/tetra-entities/src/net_dashboard/server.rs`
  - Added bounded HTTP connection cap, bounded body reads, bounded header reads,
    `431` for excessive headers, `413` for excessive bodies/static assets,
    exact API route matching and reserved `/api/*` 404 responses.
  - Serves external dashboard files from `static_dir` with path traversal
    rejection and embedded fallback.
  - Streams static files with a 2 MiB cap instead of reading whole files into
    memory.
  - Dashboard WS restart/shutdown/SDS commands now report enqueue failure instead
    of logging success on a full control channel.
- `dashboard/`
  - Added first no-build external Nexus-BS operator dashboard using `/api/system`
    and `/ws`.
  - Fallback product version displays `--` until `/api/system` provides current
    identity.
- `scripts/nexus-bs-test-deploy.sh`
  - Copies `dashboard/index.html`, `dashboard/assets/app.js` and
    `dashboard/assets/styles.css` beside the deployed binary.
- `crates/tetra-config/src/bluestation/sec_dashboard.rs`
  - Added `static_dir` parsing. Missing asset directories no longer make config
    parsing fail; existing non-directory paths are still rejected.
  - Updated auth comments to describe the current form-login + `fs_session`
    cookie model.
- `crates/tetra-entities/src/net_brew/entity.rs`
  - Bounded Brew command queue remains in place. Critical lifecycle/control/SDS
    commands use a short bounded timeout before drop; voice/RSSI/DTMF remain
    best-effort overload traffic.
- `crates/tetra-entities/src/net_brew/worker.rs`
  - Bounded Brew worker event queue remains in place. Critical lifecycle/control
    events use a short bounded timeout; voice and server-error event drops are
    rate-limited and observable.
- `crates/tetra-entities/src/service_control.rs`
  - A missed RF router tick withholds `WATCHDOG=1` but no longer exits the
    watchdog helper permanently.
- `contrib/systemd/`
  - Service templates use `Type=notify`, `NotifyAccess=main`, `WatchdogSec=30s`
    and bounded resource settings. The `@` unit points config and dashboard
    assets at `/home/%i/nexus-bs`.

Specialist review outcomes:

- Protocol guard found no accidental RF-good P2P/GSSI semantic changes in MM,
  LLC, UMAC/MAC or CMCE call control. Direct CMCE touch is WX SDS queue
  injection only.
- Runtime QA flagged remaining helper-thread supervision as a real gap:
  current watchdog proves RF router ticks, not dashboard/control/telemetry/Brew
  helper liveness. This is now tracked as P1/P0-next acceptance work.
- Dashboard review flagged stale auth wording, optional `static_dir` startup
  risk, `/api/*` SPA fallback and hardcoded static version; all were patched.

Verification:

- `cargo fmt -p tetra-config -p tetra-entities -p nexus-bs -p nexus-bs-control`
  passed.
- `cargo test -p tetra-config dashboard_static_dir --locked` passed.
- `cargo test -p tetra-entities dashboard_http_body_reader --locked` passed.
- `cargo test -p tetra-entities dashboard_unknown_api_paths --locked` passed.
- `cargo test -p tetra-entities external_dashboard_asset_manifest --locked`
  passed.
- `cargo test -p tetra-entities dashboard_static_dir --locked` passed.
- `cargo test -p tetra-entities dashboard_connection_guard --locked` passed.
- `cargo test -p tetra-entities brew_command_channel_is_bounded --locked`
  passed.
- `cargo test -p tetra-entities service_control --locked` passed.
- `cargo check -p nexus-bs --locked` passed.
- `cargo check -p nexus-bs-control --locked` passed.
- `rg -n "crossbeam_channel::unbounded|\bunbounded\(" crates bins` found no
  runtime unbounded channel use.
- `git diff --check` passed.

Next:

1. Deploy this stage only after final local review/commit, then verify systemd
   `Type=notify` and `WatchdogSec` fields on `nexus-bs@chris.service`.
2. On Pi, test dashboard external assets load from
   `/home/chris/nexus-bs/dashboard` and persistent config edits survive restart.
3. Run synthetic dashboard HTTP/header/body/static flood and Brew/control
   reconnect tests while watching RSS and RF continuity.
4. Add explicit helper-thread liveness tracking for dashboard/control/telemetry
   and Brew before claiming the runtime watchdog covers all service health.

## 2026-06-09 - Nexus-BS v0.1.61 runtime/dashboard checkpoint

Scope:

- Bumped current workspace identity from `v0.1.60` to `v0.1.61` after the
  runtime/dashboard P0 hardening stage.
- This is a runtime/dashboard/backhaul overload checkpoint. It does not supersede
  the protected RF-good semantics of `v0.1.60` for legacy local GSSI or earlier
  private-simplex checkpoints, and it is not formal TETRA certification.

Updated identity surfaces:

- Workspace/package version and `Cargo.lock` path-package versions.
- `tetra_core::PRODUCT_VERSION_TAG`, User-Agent, control protocol and telemetry
  protocol tests.
- README, example config, systemd unit descriptions and dashboard product
  identity tests.

Verification:

- `cargo check -p tetra-core` passed and updated `Cargo.lock`.
- `cargo test -p tetra-core product_identity_tracks_workspace_version --locked`
  passed.
- `cargo test -p tetra-entities control_protocol_tracks_nexus_bs_product_version --locked`
  passed.
- `cargo test -p tetra-entities telemetry_protocol_tracks_nexus_bs_product_version --locked`
  passed.

Deploy verification:

- Tagged local release commit `750b21c` as `v0.1.61`.
- Built locally only and deployed with
  `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Remote core binary SHA-256:
  `bed6c97174be62a2e2e0fb6a37be547c66ee84e1e0d9bf395d6f241c7fd4ed59`.
- Remote control-service SHA-256:
  `9108adf547aeec4aa9383b0e418c9d978c6c83796d42c6f658dc001c42cac329`.
- Remote service `nexus-bs@chris.service` restarted successfully and showed
  `Build: v0.1.61-750b21ce`.
- Installed updated `nexus-bs@.service` and `nexus-bs-control@.service` on the
  Pi, ran `systemctl daemon-reload`, and restarted both services.
- `systemctl show nexus-bs@chris.service` confirmed:
  - `Type=notify`
  - `NotifyAccess=main`
  - `WatchdogUSec=30s`
  - `ActiveState=active`
  - `SubState=running`
  - `Environment=NEXUS_BS_PERSISTENT_CONFIG=/home/chris/nexus-bs/config.toml`
  - `Environment=NEXUS_BS_DASHBOARD_STATIC_DIR=/home/chris/nexus-bs/dashboard`
- `/api/system` on the deployed dashboard returned:
  - `product_user_agent=Nexus-BS/v0.1.61`
  - `stack_version=v0.1.61-750b21ce`
  - `config_path=/home/chris/nexus-bs/config.toml`
  - `runtime_config_path=/run/nexus-bs-chris/config.toml`
  - `cpu_model=Broadcom Cortex-A53 1GHz 64-bit`
  - `sdr_name=SXceiver`
- Dashboard `/` served the external static `dashboard/index.html`, not the
  embedded compatibility dashboard.
- `GET /api/system2` returned HTTP `404` JSON instead of SPA HTML fallback.
- Startup logs after the final systemd restart showed `Control transport
  connected`; the earlier mismatch where the old control service expected
  `nexus-bs-control-v0.1.55` is fixed by deploying
  `nexus-bs-control-service` with the core binary.
- Startup logs after the final systemd restart showed restart recovery armed for
  local ISSIs `2260082`, `2260616`, `2260618`; the three registered/affiliated
  back to GSSI `226333` during recovery.

Residual deploy observations:

- The current watchdog proves RF router tick progress. It still does not prove
  independent helper-thread liveness for dashboard/control/telemetry/Brew; this
  remains tracked in `MISSION_READINESS.md`.

## 2026-06-09 10:55 EEST - Dashboard active-call reconciliation and BS uptime

Scope:

- Dashboard/API-only observability patch. No CMCE, UMAC, MM, SDS, LLC, MAC or
  RF scheduling behavior was changed.
- Added `GET /api/snapshot`, reusing the same in-memory state payload that is
  sent on WebSocket connect: registered MS list, active calls, log ring,
  last-heard ring, Brew state and health snapshots.
- Updated the external dashboard to poll `/api/snapshot` every 3 seconds as a
  cheap reconciliation path when very short call events are missed or the
  browser WebSocket stream briefly lags.
- Preserved local group-call hangtime rows during snapshot reconciliation so the
  UI does not erase the row between short Brew/TG91 talkspurts.
- Added Nexus-BS process uptime as `bs_uptime_secs` in `/api/system`, while
  keeping host uptime as `host_uptime_secs` and the legacy `uptime_secs`.
- Added `BS Uptime` and `Host Uptime` fields to the System page; both tick in
  the browser from the last `/api/system` sample.

Verification:

- `node --check dashboard/assets/app.js` passed.
- `cargo fmt -p tetra-entities` passed/applied formatting.
- `cargo test -p tetra-entities external_dashboard_asset_manifest --locked`
  passed.
- `cargo test -p tetra-entities dashboard_unknown_api_paths_are_reserved_from_spa_fallback --locked`
  passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

## 2026-06-09 11:14 EEST - Hard split dashboard process from RF core

Scope:

- Runtime architecture patch. No TETRA air-interface protocol, CMCE call
  control, MAC/UMAC scheduling, LLC/MM/SDS or RF audio behavior was changed.
- Added new workspace binary/package `nexus-bs-dashboard`.
- `nexus-bs-dashboard` serves public browser assets and proxies `/api/*`, `/ws`,
  `/login` and `/logout` to the core dashboard API over loopback.
- Added environment overrides in `nexus-bs`:
  - `NEXUS_BS_CORE_DASHBOARD_BIND`
  - `NEXUS_BS_CORE_DASHBOARD_PORT`
- Updated systemd split:
  - `nexus-bs@USER.service`: RF/TETRA core, loopback dashboard API on
    `127.0.0.1:18080`, `CPUAffinity=0 1 2`, `CPUWeight=900`.
  - `nexus-bs-dashboard@USER.service`: public dashboard front-end on
    `0.0.0.0:8080`, proxy target `127.0.0.1:18080`, `CPUAffinity=3`,
    `CPUWeight=20`, `Nice=5`, `MemoryMax=192M`.
  - `nexus-bs-control@USER.service`: local control service retained separately,
    `CPUAffinity=3`, `CPUWeight=50`, `Nice=5`.
- Updated deploy script to build and deploy `nexus-bs-dashboard`, install all
  three unit files, run `systemctl daemon-reload`, and restart
  control -> core -> dashboard.
- Updated example config and README: recommended deployment uses `[dashboard]`
  as loopback-only core API (`127.0.0.1:18080`) and the separate dashboard
  service as the public web UI.

Verification:

- `cargo check -p nexus-bs-dashboard --offline` updated `Cargo.lock` for the new
  workspace member without downloading dependencies.
- `cargo test -p nexus-bs-dashboard --locked` passed.
- `cargo check -p nexus-bs -p nexus-bs-dashboard --locked` passed.
- `cargo test -p tetra-entities external_dashboard_asset_manifest --locked`
  passed.
- `cargo test -p tetra-entities dashboard_unknown_api_paths_are_reserved_from_spa_fallback --locked`
  passed.
- `node --check dashboard/assets/app.js` passed.
- `sh -n scripts/nexus-bs-test-deploy.sh` passed.
- `git diff --check` passed.

Deploy verification:

- Built locally only and deployed to `chris@192.168.1.179` with
  `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
- Remote service state after deploy:
  - `nexus-bs@chris.service` active/running, main PID `98791`.
  - `nexus-bs-control@chris.service` active/running, main PID `98777`.
  - `nexus-bs-dashboard@chris.service` active/running, main PID `98807`.
- Remote SHA-256:
  - core `3ddbd52504979bcda32cfd679e8f7d2f16ac4026e3881296ecfa6efc54ac500c`
  - control `d6c2678f29ec64342a5a16a0b93202bc9a1e8590138dab0059f9014449185f45`
  - dashboard `f046e4e70e77c17b432b9c5947e623fbaaecd390bda1262ed743f74fdc60647f`
- Remote socket verification:
  - `127.0.0.1:18080` is owned by `nexus-bs` PID `98791`.
  - `0.0.0.0:8080` is owned by `nexus-bs-dashboard` PID `98807`.
- Remote cgroup/affinity verification:
  - core: `CPUAffinity=0-2`, `CPUWeight=900`, `MemoryMax=512M`.
  - dashboard: `CPUAffinity=3`, `CPUWeight=20`, `Nice=5`, `MemoryMax=192M`.
  - control: `CPUAffinity=3`, `CPUWeight=50`, `Nice=5`.
- Public front-end verification through `http://127.0.0.1:8080/` returned the
  external dashboard HTML.
- Public API proxy verification through `http://127.0.0.1:8080/api/calls`
  returned `type=calls`, `calls=1`, `brew_online=true`, `brew_version=1`; the
  loopback core API at `http://127.0.0.1:18080/api/calls` returned matching
  call count.
- Enabled `nexus-bs-dashboard@chris.service` for boot:
  `systemctl is-enabled` returned `enabled`.

Deploy verification:

- Built locally only and deployed to `chris@192.168.1.179` with
  `RUN_TESTS=0 POST_START_SLEEP=8 scripts/nexus-bs-test-deploy.sh`.
- Remote services restarted successfully at `2026-06-09 10:57:03 EEST`:
  `nexus-bs@chris.service` main PID `96123`,
  `nexus-bs-control@chris.service` main PID `96110`.
- Remote core binary SHA-256:
  `cc209965746a83c34488b96d763245005802b7660f57de01ed58fe2f2c07790c`.
- Remote control-service SHA-256:
  `beaf8f1563265ece4f5c70ff4db9a0a87c7e0ce72a6b6d46e56aad095d278c15`.
- Deployed dashboard assets contain `SNAPSHOT_REFRESH_MS` and `bsUptime`.
- Live `/api/system` returned:
  - `product_version_tag=v0.1.61`
  - `stack_version=v0.1.61-4ab64b2e-modified`
  - `bs_uptime_secs=29`
  - `host_uptime_secs=602896`
  - `cpu_model=Broadcom Cortex-A53 1GHz 64-bit`
  - `sdr_name=SXceiver`
- Live `/api/snapshot` returned:
  - `type=snapshot`
  - `ms=3`
  - `calls=1`
  - `last_heard=1`
  - `brew_online=true`
  - `brew_version=1`
- A live snapshot sample showed a recent group call row:
  `call_id=6`, `gssi=91`, `active_speaker=2357620`,
  `started_secs_ago=7`, with last-heard timestamps `10:57:44` and `10:57:34`.

## 2026-06-09 11:04 EEST - One-second active-call dashboard refresh

Scope:

- Dashboard/API-only observability patch. No TETRA air-interface protocol,
  CMCE call control, MAC/UMAC scheduling, LLC/MM/SDS or RF audio behavior was
  changed.
- Added lightweight `GET /api/calls` for one-second active-call reconciliation:
  it returns only active calls, last-heard summary and Brew status from the
  in-memory dashboard state.
- Updated the browser dashboard to poll `/api/calls` every second so Active
  Calls and `active_speaker`/who-is-speaking update at one-second cadence.
- Reduced full `/api/snapshot` polling from every 3 seconds to every 10 seconds;
  it remains a broader reconciliation path for radios/logs/last-heard.
- Removed the whole-dashboard `renderAll()` interval. The one-second UI tick now
  updates only call duration cells and System uptime labels, avoiding expensive
  table rebuilds.

Important architecture note:

- The dashboard static assets are external to the core binary and dashboard
  HTTP/WS handling runs on separate threads, but the dashboard API is still
  inside the `nexus-bs` process. A fully isolated dashboard binary/process with
  its own systemd CPU affinity remains a separate P1 architecture item.

Verification:

- `node --check dashboard/assets/app.js` passed.
- `cargo fmt -p tetra-entities` passed/applied formatting.
- `cargo test -p tetra-entities external_dashboard_asset_manifest --locked`
  passed.
- `cargo test -p tetra-entities dashboard_unknown_api_paths_are_reserved_from_spa_fallback --locked`
  passed.
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

## 2026-06-09 12:32 EEST - Dashboard visual polish pass

Scope:

- Dashboard UI-only polish pass. No CMCE, MM, MAC/UMAC, LLC, SDS, WAP, Brew
  routing, RF audio, or air-interface protocol behavior was changed.
- Integrated the two web-design audit findings into the external dashboard
  assets:
  - tightened the Nexus-BS operations-console visual hierarchy;
  - made Traffic collapse before medium-width overflow;
  - removed global table-cell nowrap so subscriber identities, destinations and
    activity fields can fit better;
  - added safer ellipsis constraints for topbar, panel headers, nav labels and
    status tiles;
  - capped long config/Soapy fields and log rows so they do not stretch cards;
  - removed browser-dependent `:has()` status styling and now relies on explicit
    JS state classes;
  - changed narrow layouts so the sidebar and topbar no longer depend on a
    fragile hard-coded sticky offset.

Verification:

- `node --check dashboard/assets/app.js` passed.
- `cargo test -p tetra-entities external_dashboard_asset_manifest_is_coherent --locked`
  passed.
- `cargo test -p nexus-bs-dashboard --locked` passed.
- `git diff --check` passed.
- CSS scan confirmed no `:has()`, `:contains()` or negative letter-spacing in
  `dashboard/assets/styles.css`.

Deploy verification:

- Deployed directly to `chris@192.168.1.179` with
  `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
- Remote services restarted successfully at `2026-06-09 12:33:14 EEST`:
  - `nexus-bs@chris.service` main PID `122267`;
  - `nexus-bs-control@chris.service` main PID `122252`;
  - `nexus-bs-dashboard@chris.service` main PID `122282`.
- Remote dashboard binary SHA-256:
  `6d60d6c3c56685c872a73d0d9149b7789163c0e975d6d62093338c14fbad1d3a`.
- Public dashboard check from the workstation returned `HTTP 200` for
  `http://192.168.1.179:8080/`.
- Remote `/api/system` returned `product_user_agent=Nexus-BS/v0.1.61`,
  `product_version_tag=v0.1.61`, `cpu_model=Broadcom Cortex-A53 1GHz 64-bit`,
  `sdr_name=SXceiver`, and live process uptime.
- Remote `/api/calls` returned `brew_online=true`, `brew_version=1`, and a live
  group call row on GSSI 91, proving the dashboard has current live data to
  render.
- Remote CSS asset contains the new responsive `.traffic-grid table` rules and
  has no `:has()` selectors.

## 2026-06-09 13:09 EEST - Field log check: apparent group-call hang on GSSI 226777

Scope:

- Runtime log analysis only. No CMCE, MM, MAC/UMAC, LLC, SDS, WAP, Brew routing,
  RF audio, dashboard, or config files were changed.

Findings:

- The reported apparent hang was observed on GSSI `226777`.
- Remote config currently treats only `[1,90]` and `226333` as local-only
  `local_ssi_ranges`; therefore `226777` is Brew-routable by design.
- `call_id=75` from local ISSI `2260616` to GSSI `226777` started at `13:06:19`,
  sent first accepted UL media, then released after tail-drained `D-TX CEASED`
  and `D-RELEASE`.
- `call_id=77` was not a local loop: it was a Brew/network-origin call from ISSI
  `2260136` to GSSI `226777`. Local MS snapshot showed registered local ISSIs
  `2260616`, `2260618`, and `2260082`; `2260136` was not local.
- After Brew `GROUP_IDLE` for `call_id=77` at `13:07:45`, local ISSI `2260616`
  requested floor on the same maintained call at `13:07:47`, was granted, and
  forwarded to TetraPack as a new local Brew UUID.
- The local over sent `GROUP_IDLE` at `13:07:56`; the maintained group call
  expired hangtime and closed DL/UL TS2 at `13:08:01`.

Conclusion:

- The evidence does not show a permanently stuck BS call. It shows Brew-routed
  group-call hangtime and network/local retake behavior on `226777`.
- For strictly local RF group-call tests, use `226333` under the current config.
  Testing on `226777` exercises Brew/TetraPack behavior and may stay open until
  network `GROUP_IDLE` plus group hangtime expiry.

## 2026-06-09 13:50 EEST - Brew/GSSI group-call lifecycle hardening before self-healing stage

Scope:

- Protocol repair for Brew-routed GSSI group calls only.
- CMCE = call control on the TETRA RF side: owns D-SETUP, floor grant/cease and
  D-RELEASE state.
- Brew = external TetraPack/interconnect bridge: carries GROUP_TX/GROUP_IDLE,
  voice frames, SDS and subscriber updates.
- UMAC = lower RF scheduling/floor/media activation owner for assigned
  timeslots.
- No private simplex/P2P, duplex, parrot, MM attach, LLC timers, WAP, dashboard
  UI, or encryption behavior was intentionally changed.

ETSI clause-scoped basis:

- EN 300 392-2 clause 14.5.2.1: group-call setup uses D-SETUP before the MS
  treats the group call as established on RF.
- EN 300 392-2 clause 14.5.2.2.1: D-TX GRANTED is the RF floor-control edge
  before U-plane media is accepted for a speaker.
- EN 300 392-2 clause 14.5.2.3: group-call teardown uses D-RELEASE; ordinary
  floor idle is separate from call release.
- EN 300 392-2 clause 23.8.5 traffic-tail ordering is preserved by keeping
  existing reporter/tail-drain release paths.
- This is clause-scoped engineering evidence only, not formal TETRA
  certification.

Implemented repair:

- `BrewWorker::enqueue_event()` now applies lossless back-pressure only for
  call/subscriber lifecycle events that can corrupt CMCE state if dropped
  (`GROUP_TX`, `GROUP_IDLE`, circuit setup/connect/release, connected and
  subscriber lifecycle). Voice, SDS, DTMF and server-error events remain bounded
  so the network worker is not stalled by non-lifecycle flood.
- `BrewEntity` no longer drops outbound critical lifecycle commands when the
  worker command channel is full. It keeps a local retry queue for critical
  commands and flushes it every tick.
- `BrewEntity` caps non-critical command backlog with a media watermark so
  accepted voice/RSSI/DTMF traffic cannot sit ahead of `GROUP_IDLE`/release for
  an unbounded time.
- `BrewEntity` now distinguishes a full worker channel from a disconnected
  worker channel. Full means retry; disconnected means mark Brew down and avoid
  an impossible-to-flush critical backlog.
- CMCE network-origin group-call setup and speaker-change readiness remains
  RF-reporter gated: `NetworkCallReady` is emitted only after D-SETUP or
  D-TX GRANTED has actually been reported as transmitted.
- New CMCE edge case: if Brew sends `GROUP_IDLE` before the RF-ready reporter
  fires, CMCE cancels pending readiness and releases the reserved group circuit
  with D-RELEASE instead of emitting false D-TX CEASED/FloorReleased and leaving
  the call in hangtime.

Verification:

- `cargo test -p tetra-entities --lib net_brew::entity --locked` passed
  (`21` tests).
- `cargo test -p tetra-entities --lib net_brew --locked` passed (`35` tests).
- `cargo test -p tetra-entities --test test_cmce_bs network_group --locked`
  passed (`14` tests).
- `cargo check -p tetra-entities --locked` passed.
- `git diff --check` passed.

Deploy verification:

- Deployed directly to `chris@192.168.1.179` with
  `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
- Build was local; no Rust compilation was performed on the Pi/TetraHS.
- Remote deploy output:
  - core binary SHA-256:
    `5c873df9a56f7291f632f356d3e9727fb517883ab58549126cba01e9a6fd168e`;
  - control binary SHA-256:
    `bdfe4d58fe7bbac2da23eb2cf2874ed78ec994a9ca1593e2a2c46f01233171e7`;
  - dashboard binary SHA-256:
    `6d60d6c3c56685c872a73d0d9149b7789163c0e975d6d62093338c14fbad1d3a`.
- Remote services restarted successfully at `2026-06-09 13:52:22 EEST`:
  - `nexus-bs@chris.service` main PID `150039`;
  - `nexus-bs-control@chris.service` main PID `150025`;
  - `nexus-bs-dashboard@chris.service` main PID `150054`.
- Public dashboard API check returned `/api/system` with
  `product_user_agent=Nexus-BS/v0.1.61`, `cpu_model=Broadcom Cortex-A53 1GHz
  64-bit`, `sdr_name=SXceiver`, runtime config
  `/run/nexus-bs-chris/config.toml`, and live BS uptime.
- Public `/api/calls` returned `brew_online=true`, `brew_version=1`, and a live
  group-call row on GSSI `91`, proving the deployed dashboard/core path is
  receiving live call telemetry after restart.

Focused new/updated tests:

- Critical `GROUP_IDLE` is deferred instead of dropped when the worker command
  channel is full.
- Non-critical media backlog is capped before `GROUP_IDLE`, bounding lifecycle
  latency.
- Worker channel disconnect does not leave unflushable critical backlog.
- Network `GROUP_IDLE` before RF-ready releases with D-RELEASE and does not
  emit false floor idle.

Next stage after deploy/test:

- Implement the separate self-healing/health-monitor goal as a sidecar-style
  architecture, not mixed into this protocol repair: passive health snapshots
  first, then bounded entity-owned remediation actions for service, voice, SDS,
  P2P, congestion and RF desynchronization.

## 2026-06-09 14:01 EEST - Self-healing P0 observe-only health monitor

Scope:

- First self-healing stage only: passive health observability.
- No automatic RF, CMCE, UMAC, SDS, P2P, Brew, service restart, or PHY recovery
  action is enabled in this patch.
- No TETRA air-interface PDU, timer, field, call-control state transition, SDS
  OTA delivery semantic, encryption path, or WAP behavior was intentionally
  changed.

Implemented:

- Added `crates/tetra-entities/src/health/`:
  - `HealthDomain`, `HealthSeverity`, `HealthMetric`,
    `HealthDomainSnapshot`, `HealthSnapshot`, `HealthActionRecord`;
  - global `HealthRegistry` backed by atomics;
  - 1 Hz `health-monitor` sidecar thread that emits through the existing
    bounded telemetry channel.
- Added `TelemetryEvent::HealthSnapshot`.
- Instrumented the router RF stack loop with:
  - tick count;
  - last tick age;
  - last loop duration;
  - current/max message queue length.
- Instrumented Brew with:
  - connected/disconnected state and server version;
  - worker command queue length;
  - pending critical command backlog;
  - non-critical command drop counter.
- Dashboard caches and serializes the latest health snapshot in the existing
  snapshot/site/WS telemetry path. Serialization keeps the existing
  clone-then-release-lock pattern so large health JSON does not hold dashboard
  state locks while rendering.

Technical intent:

- Health monitor is a non-blocking observer. Hot paths only store atomics or
  send through bounded telemetry.
- P1 remediation must be entity-owned and bounded: the monitor may request
  actions, but CMCE/UMAC/PHY/Brew must execute them through their existing safe
  procedures. It must not mutate protocol tables directly.

Verification:

- `cargo test -p tetra-entities --lib health --locked` passed.
- `cargo test -p tetra-entities --lib
  net_telemetry::channel::tests::telemetry_health_snapshots_are_bounded_and_non_blocking_on_overflow
  --locked` passed.
- `cargo test -p tetra-entities --lib
  net_dashboard::server::tests::dashboard_caches_and_serializes_health_snapshot
  --locked` passed.
- `cargo test -p tetra-entities --lib net_brew::entity --locked` passed
  (`21` tests).
- `cargo check -p tetra-entities -p nexus-bs --locked` passed.
- `git diff --check` passed.

Deploy verification:

- Deployed directly to `chris@192.168.1.179` with
  `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
- Remote services restarted successfully at `2026-06-09 14:02:38 EEST`:
  - `nexus-bs@chris.service` main PID `154291`;
  - `nexus-bs-control@chris.service` main PID `154275`;
  - `nexus-bs-dashboard@chris.service` main PID `154305`.
- Public `/api/snapshot` returned `last_health.overall=ok` with:
  - service `tick_age_ms=12`, `last_loop_us=15001`, queue length `0`;
  - Brew connected, server version `1`, command queue length `0`,
    pending critical command backlog `0`, non-critical drops `0`.
- Public `/api/site` returned the same health snapshot under `rf_cached.health`.

## 2026-06-09 14:09 EEST - Health config plumbing and wording cleanup

Scope:

- Configuration and observability cleanup only.
- No automatic remediation, service restart, RF resync, call release, SDS retry,
  P2P, group-call, WAP, MM attach, LLC, UMAC scheduling, or PHY protocol
  behavior was changed.

Implemented:

- Added `[health]` configuration in `tetra-config`:
  - `enabled = true` by default;
  - `snapshot_interval_secs = 1`;
  - `core_stall_critical_ms = 10000`;
  - `restart_on_core_stall = false`;
  - `restart_after_critical_secs = 30`;
  - `restart_cooldown_secs = 600`.
- Health monitor startup now respects `[health].enabled` and
  `snapshot_interval_secs`.
- Example config documents that health is observe-only in Nexus-BS v0.1.61 and
  that automatic restart is intentionally default-off.
- Dashboard `/api/site` now exposes the configured health settings alongside
  the live health snapshot.
- Reworded comments from "mission-critical health snapshot" to "operational
  health snapshot" to avoid implying formal certification or validated
  remediation behavior.

Verification:

- `cargo test -p tetra-config --lib health --locked` passed.
- `cargo test -p tetra-config --lib bluestation::parsing::tests::test_health
  --locked` passed.
- `cargo test -p tetra-entities --lib health --locked` passed.
- `cargo test -p tetra-entities --lib
  net_dashboard::server::tests::dashboard_caches_and_serializes_health_snapshot
  --locked` passed.
- `cargo test -p tetra-entities --lib
  net_telemetry::channel::tests::telemetry_health_snapshots_are_bounded_and_non_blocking_on_overflow
  --locked` passed.
- `cargo check -p tetra-config -p tetra-entities -p nexus-bs --locked` passed.
- `git diff --check` passed.

## 2026-06-09 14:29 EEST - Self-healing P1 health domains and bounded action bus

Scope:

- Added self-healing infrastructure and health observability only.
- No CMCE private-simplex setup/release logic, parrot media logic, SDS routing
  rules, MM attach semantics, LLC timers, or UMAC grant priority behavior was
  changed.
- Clause-scoped ETSI policy remains: CMCE/SDS/UMAC observations do not mutate
  protocol state from the health monitor. Future remediation must use existing
  EN 300 392-2 procedures such as D-RELEASE/D-DISCONNECT, D-SDS-DATA/D-STATUS
  report handling, and MAC/UMAC owned queue/backpressure paths. No formal
  certification is claimed.

Implemented:

- Added bounded health action bus:
  - capacity `64`, `try_send` only;
  - action backlog/drop counters;
  - recent action ring bounded to `16` records;
  - active action currently limited to `RestartService`.
- Wired `[health]` core-stall policy into the monitor:
  - `core_stall_critical_ms` feeds the service critical threshold;
  - `restart_on_core_stall = false` remains the default;
  - when explicitly enabled, restart requires persistent critical stall,
    `restart_after_critical_secs`, and `restart_cooldown_secs`.
- Extended health snapshots with domains:
  - `service`: RF stack loop tick age, loop duration, message queue length;
  - `telemetry`: bounded telemetry queue length and drops;
  - `brew`: backhaul connected/version, command queue, critical backlog, drops;
  - `voice`: CMCE group-call active/pending/floor waiter counts;
  - `p2p`: CMCE private-call active/pending/release/floor waiter counts;
  - `sds`: live SDS queue, pending SDS actions, SDS-TL context pressure;
  - `congestion`: UMAC DL queue, RA ACK, TMA report and private UL media
    pressure;
  - `rf`: Soapy/SXceiver rxtx duration, TX-late and RX-loss counters.
- Added non-blocking owner-side instrumentation:
  - `TelemetrySink::send` now records sent/full/disconnected counters without
    blocking;
  - `CmceBs::tick_start` publishes CMCE and SDS stats after normal drains;
  - `UmacBs::tick_start` publishes scheduler/TMA/private-media pressure after
    normal finalization;
  - Soapy RF timing updates the RF health counters at existing TX-late/RX-loss
    points.
- Updated example config wording to describe the expanded health domains while
  keeping automatic restart explicit/default-off.

Technical notes:

- The health monitor does not call RF, CMCE, SDS, UMAC or Brew methods directly.
- CMCE/SDS/UMAC/RF remediations beyond service restart are intentionally not
  active yet; they require separate clause-scoped entity-owned actions and RF
  field validation.
- This gives Nexus-BS immediate detection/visibility for service, voice, SDS,
  P2P, congestion and RF-desync symptoms, plus a safe default-off service
  restart guard for persistent core-loop stalls.

Verification:

- `cargo test -p tetra-entities --lib health --locked` passed (`8` tests).
- `cargo test -p tetra-entities --lib
  telemetry_health_snapshots_are_bounded_and_non_blocking_on_overflow --locked`
  passed.
- `cargo test -p tetra-config --lib health --locked` passed (`5` tests).
- `cargo check -p tetra-config -p tetra-entities -p nexus-bs --locked` passed.
- `cargo test -p tetra-entities --test test_cmce_bs network_group --locked`
  passed (`14` tests).
- `cargo test -p tetra-entities --test test_sds_bs --locked` passed (`121`
  tests).
- `cargo test -p tetra-entities --test test_umac_bs tma_report --locked` passed
  (`5` tests).
- `cargo test -p tetra-entities --test test_umac_bs private_simplex --locked`
  passed (`11` tests).
- `cargo test -p tetra-entities --test test_umac_bs parrot --locked` passed
  (`3` tests).
- `git diff --check` passed.

Next:

- Deploy to the test BS with
  `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
- After field traffic, inspect `/api/snapshot.last_health` for `voice`, `p2p`,
  `sds`, `congestion` and `rf` domain changes before implementing any
  entity-owned remediation actions.

Follow-up fix before redeploy:

- Live `/api/snapshot` after the first deploy showed `rf.rx_lost_samples`
  overflowed when the SDR timing delta was negative (`-1200` samples from the
  SXceiver startup path). The RF metric now clamps signed timing counters to
  non-negative health counts before publishing them.
- Added
  `phy::components::soapy_dev::tests::health_counter_conversion_clamps_negative_timing_counts`.
- Re-verified:
  - `cargo test -p tetra-entities --lib
    health_counter_conversion_clamps_negative_timing_counts --locked` passed.
  - `cargo check -p tetra-config -p tetra-entities -p nexus-bs --locked`
    passed.
  - `git diff --check` passed.

Warning audit and mitigation:

- `dl_build_traffic_block: queued signaling on ts 2 but no stealing item`
  occurred repeatedly while TS2 was carrying traffic and one queued DL
  signalling item was not yet eligible as FACCH/STCH stealing. Health showed
  `congestion.dl_queue_total=1`, so this was not observed as growing UMAC
  congestion. The log level was reduced from `warn` to `debug`; real queue
  pressure remains visible through the `congestion` health domain.
- `SX1255 temperature read failed: SX1255 temperature sensor requires inactive
  RX/TX streams` came from the periodic SDR health read. SXceiver/µCell are now
  treated as devices where runtime temperature sensor reads are unsupported, so
  Nexus-BS no longer calls the temp sensor while streams are active.
- Startup RF timing warnings (`Lost -1200 samples`, `Too late to produce TX
  block`, `Discarding TX samples in the past`) are still left visible because
  they are useful startup/desync indicators if they persist; health counters now
  report them without signed-count overflow.
- One-shot corrupted uplink decode warnings such as `Failed parsing MacAccess:
  BufferEnded { field: Some("ssi") }` are treated as RF/noise or partial burst
  symptoms unless repeated; no protocol change was made.

Additional verification:

- `cargo test -p tetra-entities --lib
  sxceiver_like_devices_skip_runtime_temperature_reads --locked` passed.
- `cargo test -p tetra-entities --lib
  health_counter_conversion_clamps_negative_timing_counts --locked` passed.
- `cargo check -p tetra-config -p tetra-entities -p nexus-bs --locked` passed.
- `git diff --check` passed.

Live warning follow-up - 2026-06-09 14:43 EEST:

- Rechecked the BS journal around the user-reported `14:36` warning window.
  The repeated `dl_build_traffic_block: queued signaling on ts 2 but no
  stealing item` entries were emitted by the previous `nexus-bs` process
  (`pid=166330`) during rapid Brew TG91 floor changes and hangtime reuse.
- The current deployed service was restarted at `14:41:26` (`pid=169061`).
  In the current code this diagnostic is `debug`, not `warn`, and no
  `queued signaling` WARN entries appeared after the restart.
- Current `/api/snapshot` health after the check: `overall=ok`, with
  `congestion=ok`, `rf=ok`, `brew=ok`, `voice=ok`, `p2p=ok`, and `sds=ok`.

Group hangtime observation - 2026-06-09:

- User clarified that "floor hold" means the visible terminal group-call
  hangtime after releasing PTT. Live logs confirmed that the current lab config
  has `legacy_gssi_group_call = true` and no explicit `hangtime_secs`, so
  Nexus-BS uses `hangtime_secs=5` but the legacy compatibility path releases
  local GSSI calls immediately after the tail-drained `D-TX CEASED`.
- This is why Motorola/Hytera terminals can leave the group-call screen after
  about 1-2 seconds: `D-TX CEASED` is followed immediately by `D-RELEASE`
  (`SwmiRequestedDisconnection`) in the legacy no-handoff path.
- ETSI EN 300 392-2 clause 14.5.2.2.1(e) defines the normal end-of-transmission
  edge (`U-TX CEASED` then `D-TX CEASED`), while clause 3.1 defines
  quasi-transmission-trunking channel hang-time as a short delayed channel
  deallocation period. The current immediate release is a local Motorola/MR
  compatibility policy, not a mandated ETSI hangtime value.

System-code broadcast audit - 2026-06-09:

- User asked whether the BlueStation/Nexus-BS `system_code = 3` logic is ETSI
  correct. Rechecked EN 300 392-2 clause 21.4.4.2 table 21.76: SYNC system
  code `0011b` identifies ETSI EN 300 392-2 V3.1.1 through the present
  document family and related EN 300 392-7 V3.x references. The Nexus-BS
  default `system_code = 3` in MAC-SYNC is clause-correct for the current
  V+D stack target; it is not a terminal "class 3" setting.
- Found adjacent broadcast inconsistency: UMAC precompute advertised security
  class 1/2 support in MAC-SYSINFO extended services even when
  `aie_service=false`. Because Nexus-BS does not implement air-interface
  encryption/security procedures yet, MAC-SYSINFO security classes and
  D-MLE-SYSINFO AIE advertisement are now fail-closed.
- Clause scope: EN 300 392-2 clause 21.4.4.1 table 21.67 references the
  EN 300 392-7 security information element; clause 21.4.4.2 table 21.76
  covers `system_code`; the related security-class notes in clause 21.4.4.2a
  reinforce that security-class bits have no meaning when AIE is unavailable.
- Added tests:
  - `test_mac_sync_defaults_to_etsi_v3_system_code`
  - `test_sysinfo_does_not_advertise_aie_or_security_classes_until_security_is_implemented`
- Re-verified:
  - `rustfmt --edition 2024 crates/tetra-entities/src/umac/umac_bs.rs crates/tetra-entities/tests/test_umac_bs.rs --check`
  - `cargo test -p tetra-entities --test test_umac_bs system_code --locked`
  - `cargo test -p tetra-entities --test test_umac_bs security_classes --locked`
  - `cargo test -p tetra-entities --test test_umac_bs sndcp_service --locked`
  - `cargo check -p tetra-entities --locked`

Dashboard Settings Config Manager - 2026-06-09:

- User reported that Settings was incomplete: no current-config editor, no
  config selection/activation, no duplicate/copy flow, and no persistent
  selected config across reboot.
- Scope is dashboard/admin config management only. No TETRA air-interface PDU,
  CMCE, UMAC/MAC, MM, LLC, SDS, Brew media, WAP, encryption or RF behaviour was
  intentionally changed.
- Added a Settings `Config Manager` to the external dashboard assets:
  - current `config.toml` editor via existing `/api/config`;
  - profile selector backed by `/api/configs`;
  - profile load/save via `/api/configs/<name>`;
  - persistent activation via `/api/configs/activate`;
  - duplicate flow that creates the next free `config+N.toml` in the same flat
    Nexus-BS folder and loads it into the editor.
- Backend hardening:
  - profile names are percent-decoded from URL paths, so `config%2B1.toml`
    becomes `config+1.toml`;
  - profile names are restricted to flat `.toml` filenames with
    letters/numbers/`.`/`_`/`-`/`+`, rejecting traversal and backup suffixes;
  - activation copies the selected profile over persistent `config.toml` and
    writes `config.toml.active` as a small marker so the dashboard remembers
    the last selected profile after restart/reboot;
  - selecting `config.toml` itself as active updates only the marker and does
    not copy the file onto itself.
- Runtime note: because the RF core normally runs a volatile `/run/.../config.toml`
  copy, profile activation is persistent for the next service restart/reboot;
  it does not hot-reconfigure the live TETRA stack.
- Verification:
  - `node --check dashboard/assets/app.js` passed.
  - `cargo test -p tetra-entities --lib dashboard_config_profile --locked`
    passed.
  - `cargo test -p tetra-entities external_dashboard_asset_manifest --locked`
    passed.
  - `cargo check -p tetra-entities -p nexus-bs-dashboard --locked` passed.
  - `cargo fmt -p tetra-entities -p nexus-bs-dashboard --check` passed.
  - `git diff --check` passed.
- Deploy verification:
  - Deployed to `chris@192.168.1.179` with
    `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
  - Local build only; no Rust compilation on the Pi/TetraHS.
  - Remote SHA-256 after deploy:
    - core `03bd301b7517d925a1154781871de9835fe741e8af1a46fbeac8fb83045f9b8f`;
    - control `8d80f02c27a8eac780a8d20027a9931bfc5c461badb759501439bb2a8a37a683`;
    - dashboard `6d60d6c3c56685c872a73d0d9149b7789163c0e975d6d62093338c14fbad1d3a`.
  - Remote services active/running since `2026-06-09 16:21:15 EEST`:
    - `nexus-bs@chris.service` PID `195383`;
    - `nexus-bs-control@chris.service` PID `195367`;
    - `nexus-bs-dashboard@chris.service` PID `195398`.
  - Remote asset check passed:
    - `dashboard/index.html` contains `configManager`;
    - `dashboard/assets/app.js` contains `duplicateSelectedConfig`;
    - `dashboard/assets/styles.css` contains `config-editor`.
  - Remote `GET /api/configs` through public dashboard returned HTTP 200 with
    `config.toml` active/runtime.
  - Remote `GET /api/system` through public dashboard returned HTTP 200 with
    persistent config path `/home/chris/nexus-bs/config.toml`.

Dashboard Service Controls and Log/Profile Operations - 2026-06-09:

- User reported missing BS restart/shutdown/stop&go controls, missing config
  profile delete, and weak Logs UX.
- Scope is dashboard/admin lifecycle and volatile dashboard log handling only.
  No TETRA air-interface protocol, CMCE call state machine, UMAC/MAC RF
  scheduling, SDS, WAP, Brew voice, encryption, attach, affiliation, or floor
  control behaviour was intentionally changed.
- Added Settings/Admin service controls:
  - `Restart BS` posts to `/api/service/restart`;
  - `Shutdown BS` posts to `/api/service/shutdown`;
  - `Stop & Go 5s` posts to `/api/service/stop-go`.
- `Stop & Go 5s` semantics are explicit: the core exits immediately through
  the normal lifecycle path, clearing volatile buffers/state by process stop;
  systemd then starts it again after `RestartSec=5`. The button does not keep
  the RF core alive for five seconds before stopping.
- Added `ControlCommand::StopGoService { start_delay_secs: 5 }`, routed it to
  CMCE, and mapped it to the existing systemd restart exit path. Updated
  `contrib/systemd/nexus-bs@.service` and the legacy sample service to
  `RestartSec=5`.
- Added config profile deletion:
  - UI button `Delete`;
  - backend `DELETE /api/configs/<name>`;
  - refuses to delete the runtime `config.toml` or active selected profile;
  - keeps the same flat filename validation as the config manager.
- Added Logs page operations:
  - chronological log direction, newest entries at the bottom;
  - pause/play autoscroll control;
  - clear logs through `POST /api/logs/clear` against the in-memory ring;
  - browser export as `LogYYYYMMDD-HHMMSS.log`.
- Verification:
  - `node --check dashboard/assets/app.js` passed.
  - `cargo fmt -p tetra-entities -p nexus-bs-dashboard --check` passed.
  - `cargo test -p tetra-entities --test test_dashboard_assets external_dashboard_asset_manifest_is_coherent --locked` passed.
  - `cargo test -p tetra-entities --lib dashboard_config_profile --locked`
    passed.
  - `cargo test -p tetra-entities --lib dashboard_unknown_api_paths_are_reserved_from_spa_fallback --locked` passed.
  - `cargo test -p tetra-entities --lib test_route_stop_go_service_to_cmce --locked` passed.
  - `cargo check -p tetra-entities -p nexus-bs-dashboard --locked` passed.
  - `git diff --check` passed.
- Deploy verification:
  - Deployed to `chris@192.168.1.179` with
    `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
  - Local build only; no Rust compilation on the Pi/TetraHS.
  - Remote services active/running since `2026-06-09 16:39:02 EEST`:
    - `nexus-bs@chris.service` PID `202567`;
    - `nexus-bs-control@chris.service` PID `202553`;
    - `nexus-bs-dashboard@chris.service` PID `202584`.
  - Remote SHA-256 after deploy:
    - core `04723de9524ef048a1c30e2ab243710fdbb1828c9f8c1cba308d00966e0440f4`;
    - control `c46c2c45770ae793ac9eafb03299c023c5a007a20e4f680f1219794435c5c129`;
    - dashboard binary `6d60d6c3c56685c872a73d0d9149b7789163c0e975d6d62093338c14fbad1d3a`.
  - Remote `systemctl show nexus-bs@chris.service -p RestartUSec`
    returned `RestartUSec=5s`.
  - Remote dashboard assets contain `serviceStopGoBtn`, `configDeleteBtn`,
    `logAutoScrollBtn`, `/api/service/stop-go`, and `/api/logs/clear`.
  - Remote core dashboard APIs responded HTTP 200 for `/api/system` and
    `/api/configs`.

Rohde & Schwarz Style Dashboard Consolidation - 2026-06-09 21:35 EEST:

- Scope is external dashboard UI/UX only. No TETRA air-interface protocol,
  RF scheduling, CMCE private/group call control, SDS, WAP, Brew voice,
  attach, affiliation, energy economy, encryption, or terminal compatibility
  behaviour was intentionally changed.
- Reworked the dashboard visual language toward the provided
  Rohde & Schwarz transmitter-console reference:
  - metallic grey/light-blue SCADA console shell;
  - cyan active navigation/title treatment;
  - green/amber/red status meters;
  - beveled industrial panels, buttons, slot cards, and component nodes;
  - functional RF device-map view with Core, RF Path, RF Device, Brew,
    UMAC Scheduler, Output Stage, Antenna, Program, and RF Program Path.
- Consolidated duplicated operator workflow:
  - removed separate `Radios`, `Calls`, and `Last Heard` nav tabs/pages;
  - kept their dynamic render targets and detailed tables under the
    single `Traffic` page via `traffic-detail-stack`;
  - preserved JS contracts for `overviewRadios`, `overviewCalls`,
    `overviewHeard`, `radiosTable`, `callsTable`, and `heardTable`.
- Removed the permanently disabled OTA update button from the operator
  topbar.
- Replaced static rail labels (`LOCAL`/`LIVE`) with live
  `railConsoleState` and `railTelemetryState` values driven by the existing
  core/WebSocket health model.
- Wired RF Program Path to live RF state:
  - `ON AIR` when RF config/visual/SDR health is available;
  - `STANDBY` otherwise;
  - SCADA tone follows `is-ok`/`is-warn`/`is-bad`.
- Responsive/DPI hardening:
  - native `rem` sizing and system fonts, no viewport-width font scaling;
  - touch/coarse-pointer target sizing;
  - high-DPI rendering polish;
  - single-column operator status and meters at phone widths;
  - horizontal scroll only where technical tables are intentionally wide;
  - browser text-size adjustment left to OS/browser scaling.
- Added dashboard asset tests for:
  - consolidated Traffic workflow;
  - absence of duplicate traffic nav/pages;
  - absence of the disabled OTA button;
  - live rail status IDs and JS wiring;
  - touch/high-DPI CSS media support;
  - no `font-size` rules based on `vw`.
- Local visual QA screenshots:
  - desktop: `/private/tmp/nexus-bs-rs-dashboard-responsive-desktop.png`;
  - tablet: `/private/tmp/nexus-bs-rs-dashboard-responsive-tablet.png`;
  - phone: `/private/tmp/nexus-bs-rs-dashboard-responsive-phone-2.png`.
- Verification:
  - `node --check dashboard/assets/app.js` passed.
  - `cargo test -p tetra-entities --test test_dashboard_assets external_dashboard_asset_manifest_is_coherent --locked` passed.
  - `git diff --check` passed.

Traffic TG91 Speaker/ISSI Reconciliation - 2026-06-09 23:51 EEST:

- Scope is dashboard telemetry/state and external UI only. No ETSI TETRA
  air-interface protocol, CMCE call setup/release, UMAC scheduling, SDS, WAP,
  Brew media framing, MM attach, energy economy, or RF timing behaviour was
  intentionally changed.
- User reported that TG91 still did not show the expected speaker ISSI/country;
  the current remote `/api/calls` response had no active call while
  `/api/calls.last_heard` contained real TG91 speaker ISSIs, including US
  RadioID-resolvable speakers. This pointed to dashboard reconciliation/stale
  state rather than RF signalling.
- Hardened group-call display for fast Brew/TG91 floors:
  - dashboard backend now treats `GroupCallSpeakerChanged` as the operational
    caller for the reused group-call record and resets the dashboard call timer;
  - browser-side speaker normalization rejects a group GSSI value as a speaker
    ISSI, so `TG91` cannot be rendered as the current talker;
  - TG country/flag selection now considers the latest same-GSSI `last_heard`
    speaker before falling back to the target GSSI/WW mapping;
  - the large `Active Calls` board renders only currently active calls, while
    CMCE detail rows may still show clearly labelled hangtime/last-speaker
    records for short post-floor continuity.
- Verification before deploy:
  - `node --check dashboard/assets/app.js` passed.
  - `cargo test -p tetra-entities --test test_dashboard_assets external_dashboard_asset_manifest_is_coherent --locked` passed.
  - `git diff --check` passed.
- Deploy verification:
  - Deployed to `chris@192.168.1.179` with
    `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
  - Local build only; no Rust compilation on the Pi/TetraHS.
  - Remote services active/running since `2026-06-09 23:53:03 EEST`:
    - `nexus-bs@chris.service` PID `280402`;
    - `nexus-bs-control@chris.service` PID `280388`;
    - `nexus-bs-dashboard@chris.service` PID `280419`.
  - Remote asset contains `function normalizedSpeakerIssi`,
    `const currentCalls = activeCalls()`, and
    `caller_issi: msg.speaker_issi`.
  - Remote `/api/calls` confirmed an active TG91 record with
    `active_speaker=3144335`, `caller_issi=3144335`, `gssi=91`, `ts=3`,
    and `started_secs_ago=2`.
  - Remote RadioID proxy confirmed TG91 speakers `3219149` and `3223542` as
    United States entries.

Dashboard Brew Group Speaker Change Fix - 2026-06-10 00:24 EEST:

- Scope is CMCE dashboard telemetry for Brew/network-origin group speaker
  changes plus focused tests. No TETRA RF message encoding, UMAC scheduling,
  floor-control grant construction, SDS, WAP, MM attach, energy economy, or
  private-call protocol behaviour was intentionally changed.
- Field report: dashboard showed `YO3TCO` while `YO8TEH` was speaking.
  Remote checks confirmed:
  - RadioID: `2260616 = YO3TCO`;
  - RadioID: `2261313 = YO8TEH`;
  - logs showed `CMCE: network call speaker change gssi=226333
    new_speaker=2261313 (was 2260616)`;
  - `/api/calls` still reported `active_speaker=2260616`.
- Root cause:
  - local group floor changes already emitted `GroupCallSpeakerChanged`;
  - Brew/network group speaker changes reused the existing call and sent RF
    D-TX GRANTED/network-ready, but did not publish the dashboard speaker
    change event on that path.
- Fix:
  - `PendingNetworkGroupReady` now carries `notify_speaker_changed`;
  - initial Brew network group start queues network-ready without the
    speaker-change flag;
  - Brew speaker-change reuse queues network-ready with the flag;
  - after the RF control reporter confirms D-TX GRANTED/D-SETUP was
    transmitted, CMCE emits `GroupCallSpeakerChanged`, which updates dashboard
    `active_speaker` and operational `caller_issi`.
- Verification before deploy:
  - `rustfmt --edition 2024` on touched CMCE/test files passed.
  - `cargo test -p tetra-entities --test test_cmce_bs test_network_group_speaker_change_updates_dashboard_after_rf_grant --locked` passed.
  - `cargo test -p tetra-entities --test test_cmce_bs test_network_group_call_start_propagates_priority_to_d_setup --locked` passed.
  - `cargo test -p tetra-entities --test test_dashboard_assets external_dashboard_asset_manifest_is_coherent --locked` passed.
  - `git diff --check` passed.
- Deploy verification:
  - Deployed to `chris@192.168.1.179` with
    `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
  - Local build only; no Rust compilation on the Pi/TetraHS.
  - Remote services active/running since `2026-06-10 00:26:12 EEST` for core
    and dashboard, `00:26:11 EEST` for control:
    - `nexus-bs@chris.service` PID `289626`;
    - `nexus-bs-control@chris.service` PID `289611`;
    - `nexus-bs-dashboard@chris.service` PID `289642`.
  - Remote `/api/calls` after restart showed a new active network group call
    with `active_speaker=2260580`, `caller_issi=2260580`, `gssi=226333`,
    matching the log line `source ISSI 2260580 GSSI 226333`.
  - A live post-deploy YO3TCO->YO8TEH speaker-change was not yet observed in
    the short verification window; the new focused CMCE test covers that exact
    reused-call-id path.
- Deploy verification:
  - Deployed to `chris@192.168.1.179` with
    `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
  - Local build only; no Rust compilation on the Pi/TetraHS.
  - Remote services active/running since `2026-06-09 23:35:30 EEST`:
    - `nexus-bs@chris.service` PID `272987`;
    - `nexus-bs-control@chris.service` PID `272972`;
    - `nexus-bs-dashboard@chris.service` PID `273003`.
  - Remote dashboard asset contains `CALLS_FETCH_TIMEOUT_MS`,
    `fetchDashboardJson("/api/calls"`, `speakerChanged`, and
    `reusesEndedCall`.
  - Remote `/api/calls` through the external dashboard returned a fresh TG91
    active call with speaker `4220146` and `started_secs_ago=19`.
  - Remote `/api/system` through the external dashboard returned HTTP 200.
- Deploy verification:
  - Deployed to `chris@192.168.1.179` with
    `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
  - Local build only; no Rust compilation on the Pi/TetraHS.
  - Remote services active/running since `2026-06-09 22:20:42 EEST`:
    - `nexus-bs@chris.service` PID `246257`;
    - `nexus-bs-control@chris.service` PID `246240`;
    - `nexus-bs-dashboard@chris.service` PID `246274`.
  - Remote dashboard asset check passed:
    - `active-calls-panel` in `dashboard/index.html`;
    - `MCC_TO_ISO` and `instantSpeakerHtml` in `dashboard/assets/app.js`;
    - `speaker-issi` and `call-country` in `dashboard/assets/styles.css`.
  - Remote core dashboard APIs responded HTTP 200 for `/api/system`,
    `/api/calls`, and `/api/site`.
  - `rg` confirmed no dashboard `font-size` rule uses viewport-width units.
- Deploy verification:
  - Deployed to `chris@192.168.1.179` with
    `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
  - Local build only; no Rust compilation on the Pi/TetraHS.
  - Remote services active/running since `2026-06-09 21:36:36 EEST`:
    - `nexus-bs@chris.service` PID `233952`;
    - `nexus-bs-control@chris.service` PID `233938`;
    - `nexus-bs-dashboard@chris.service` PID `233969`.
  - Remote dashboard asset check passed:
    - `traffic-detail-stack`, `railConsoleState`, and `RF Program Path`
      are present in `dashboard/index.html`;
    - `pointer: coarse` and `min-resolution: 2dppx` are present in
      `dashboard/assets/styles.css`;
    - `diagramPathToggleState` is present in `dashboard/assets/app.js`.
  - Remote core dashboard APIs responded HTTP 200 for `/api/system`,
    `/api/site`, and `/api/configs`.

Traffic Active Calls Board - 2026-06-09 22:19 EEST:

- Scope is external dashboard UI/UX only. No TETRA air-interface protocol,
  RF scheduling, CMCE call handling, SDS, WAP, Brew voice, attach,
  affiliation, energy economy, encryption, or terminal compatibility
  behaviour was intentionally changed.
- Reworked the `Traffic` page around `Active Calls`:
  - removed the useless `Current Floor` overview panel;
  - removed the `Live Radios` overview panel from the top of Traffic;
  - kept the full Subscriber Registry table under detailed traffic views;
  - made `Active Calls` a large full-width operator board.
- Active call cards now show:
  - aligned country flag and country code;
  - mode pill (`group`, `simplex`, `duplex`, `hangtime`);
  - allocated timeslot as a fixed `TSn` badge;
  - target talkgroup/ISSI;
  - instant speaker ISSI as the dominant speaker field;
  - RadioID callsign/name as secondary detail when already cached;
  - call time in live seconds.
- Speaker display source:
  - `speaker_changed` WebSocket events update `active_speaker` immediately;
  - the card displays that ISSI directly and does not depend on the removed
    `Current Floor` UI.
- Country flag handling:
  - replaced the small hardcoded country list with a broad MCC-to-ISO map;
  - flags are generated from ISO region code using Unicode regional
    indicators;
  - country names use browser `Intl.DisplayNames` with overrides for
    worldwide/Kosovo.
- Added asset-test coverage for:
  - no `Current Floor` / `Live Radios` top Traffic panels;
  - Active Calls as the primary board;
  - aligned country flag/code, TS, and instant speaker ISSI;
  - broad MCC-to-country support examples.
- Local visual QA:
  - `/private/tmp/nexus-bs-traffic-active-calls.png` empty board;
  - `/private/tmp/nexus-bs-traffic-active-calls-mock-3.png` mock group call
    with RO flag, TS3, speaker ISSI, and 42s call time.
- Verification:
  - `node --check dashboard/assets/app.js` passed.
  - `cargo test -p tetra-entities --test test_dashboard_assets external_dashboard_asset_manifest_is_coherent --locked` passed.
  - `git diff --check` passed.

Dashboard Scroll Stability Cleanup - 2026-06-09 22:55 EEST:

- Scope is external dashboard UI/UX only. No TETRA air-interface protocol,
  RF scheduling, CMCE call handling, SDS, WAP, Brew voice, attach,
  affiliation, energy economy, encryption, or terminal compatibility
  behaviour was intentionally changed.
- Removed stale Traffic/Floor code paths after the Active Calls revamp:
  - deleted the live tick call to the removed `renderCurrentFloor` path;
  - replaced the old "no active floor" slot label with a neutral idle state.
- Hardened tab scroll behaviour:
  - dashboard now remembers `window.scrollY` per tab;
  - switching tabs restores that tab's previous scroll position;
  - live `renderAll`, one-second live ticks, and `/api/calls` refreshes
    preserve the current tab scroll instead of allowing layout reflow jumps.
- Reduced hidden-tab work:
  - log WebSocket events no longer trigger full dashboard redraws;
  - log rows are rebuilt only when the Logs tab is active;
  - log auto-scroll is gated to the Logs tab and only scrolls the log list,
    not the page viewport.
- CSS cleanup:
  - replaced the old "Legacy" dashboard comment with the current Nexus-BS
    base-layout contract;
  - consolidated page visibility under `.page:not(.active)` / `.page.active`;
  - removed duplicate page padding overrides;
  - disabled scroll anchoring on live dashboard containers.
- Added asset-test coverage for:
  - no stale `renderCurrentFloor` code;
  - per-tab scroll state;
  - Logs-only autoscroll;
  - explicit page visibility and scroll-anchor rules.
- Verification:
  - `node --check dashboard/assets/app.js` passed.
  - `cargo test -p tetra-entities --test test_dashboard_assets external_dashboard_asset_manifest_is_coherent --locked` passed.
  - `git diff --check` passed.
  - Local Chrome headless scroll QA passed on static dashboard:
    - Traffic hidden-log injection delta `0`;
    - Traffic live-event injection delta `0`;
    - Settings live-event injection delta `0`;
  - Logs visible-log injection delta `0`;
  - log list autoscroll remained internal and at bottom.

Traffic Speaker Country Flag - 2026-06-09 23:05 EEST:

- Scope is dashboard/RadioID display only. No TETRA air-interface protocol,
  RF scheduling, CMCE call handling, SDS, WAP, Brew voice, attach,
  affiliation, energy economy, encryption, or terminal compatibility
  behaviour was intentionally changed.
- User reported that TG91 active calls showed the `WW` worldwide flag, but the
  useful operator view is the country of the current speaker.
- Changed active-call country selection:
  - group calls now prefer the live speaker ISSI, then caller/active speaker,
    then called party, and only then the GSSI/TG fallback;
  - TG91 still falls back to `WW` if no speaker/caller country is known.
- Extended the same-origin RadioID proxy and browser cache payload:
  - `/api/radioid` now preserves the RadioID `country` field;
  - dashboard RadioID browser cache moved to `v2` so old cached entries without
    country are not reused forever;
  - active-call flag rendering first checks cached RadioID country, then falls
    back to the numeric prefix/MCC map.
- Field check basis:
  - RadioID showed `3215250` / `3224585` as United States speakers and
    `2320856` as Austria, confirming that the previous TG91 `WW` override hid
    the actual speaker country.
- Verification:
  - `node --check dashboard/assets/app.js` passed.
  - `rustfmt --edition 2024 --check crates/tetra-entities/src/net_dashboard/radioid.rs` passed.
  - `cargo test -p tetra-entities --test test_dashboard_assets external_dashboard_asset_manifest_is_coherent --locked` passed.
  - `cargo test -p tetra-entities --lib normalizes_radioid_payload --locked` passed.
  - `git diff --check` passed.
- Deploy verification:
  - Deployed to `chris@192.168.1.179` with
    `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
  - Local build only; no Rust compilation on the Pi/TetraHS.
  - Remote services active/running since `2026-06-09 23:04:35 EEST`:
    - `nexus-bs@chris.service` PID `263487`;
    - `nexus-bs-control@chris.service` PID `263473`;
    - `nexus-bs-dashboard@chris.service` PID `263504`.
  - Remote dashboard asset contains `nexus-bs.radioid.cache.v2`,
    `countryByRadioId`, and `callCountryCandidates`.
  - Remote `GET /api/radioid?id=3224428` returned callsign `KO6NCI` with
    `country="United States"`, confirming TG91 speaker country data is
    available to the dashboard.
  - Remote `/api/system` through the external dashboard returned HTTP 200.

Traffic Active Call Live Refresh Hardening - 2026-06-09 23:34 EEST:

- Scope is external dashboard UI client only. No TETRA air-interface protocol,
  RF scheduling, CMCE call handling, SDS, WAP, Brew voice, attach,
  affiliation, energy economy, encryption, or terminal compatibility
  behaviour was intentionally changed.
- User reported that the Traffic page stayed on an old TG91 card
  (`ISSI 3212802`, `259s`) while `/api/calls` already reported a newer active
  speaker. Remote API check confirmed the core state was fresh:
  `/api/calls` returned active speaker `3106191` with `started_secs_ago=16`.
- Hardened the browser live update path:
  - `/api/calls` polling now uses a bounded `fetchDashboardJson` helper with
    `CALLS_FETCH_TIMEOUT_MS=2500`, so a stuck request cannot leave
    `callsInflight=true` forever and freeze live updates;
  - reused group `call_id` records reset `_startedMs` when an ended/hangtime
    call is reused or when speaker/caller/target changes;
  - WebSocket `speaker_changed` events without `started_secs_ago` also reset
    the displayed call timer to the event time.
- Local Chrome headless regression reproduced the stale-card scenario and
  passed after the fix:
  - before snapshot: `ISSI 3212802`, `259s`;
  - after same `call_id=14` snapshot: `ISSI 3106191`, `16s`.
- Verification:
  - `node --check dashboard/assets/app.js` passed.
  - `cargo test -p tetra-entities --test test_dashboard_assets external_dashboard_asset_manifest_is_coherent --locked` passed.
  - `git diff --check` passed.

Brew-Origin GSSI Downlink Media Source Hardening - 2026-06-10 01:43 EEST:

- Scope is clause-scoped CMCE/UMAC group-call bearer control for Brew/TetraPack
  network-origin group calls. No private call, SDS, WAP, parrot, encryption, or
  dashboard UI behaviour was intentionally changed.
- User reported that GSSI `226333` showed emission/signalling but terminals did
  not decode audio. Remote logs before the patch showed:
  - `GROUP_TX ... dst=226333`;
  - `CMCE opening UMAC circuit ... media_source=LocalLoopback`;
  - `BrewEntity: voice frame #1 ... len=36 bytes ts=2`.
- ETSI air-interface baseline remains EN 300 392-2 clause 14.5.2.1 for group
  setup, 14.5.2.2.1 for floor control, and 14.5.2.3 for release. The Brew
  media-source selection is an internal SwMI/interconnect implementation detail,
  not a formal conformance claim.
- Fix:
  - new Brew-origin group calls now open UMAC with
    `CircuitDlMediaSource::SwMI`, so UMAC treats downlink speech as coming from
    Brew/TetraPack rather than local RF loopback;
  - added internal `CallControl::SetDlMediaSource` so maintained/reused group
    bearers can switch media policy without tearing down the assigned channel;
  - network speaker activation switches the bearer to `SwMI` before
    `FloorGranted`;
  - local RF floor retake switches the bearer back to `LocalLoopback` before
    `FloorGranted`, preventing a regression where local group speech would be
    suppressed after a Brew speaker ended.
- Verification:
  - `cargo fmt --check` passed.
  - `cargo test -p tetra-entities --test test_cmce_bs test_network_group_call_start_propagates_priority_to_d_setup --locked` passed.
  - `cargo test -p tetra-entities --test test_cmce_bs test_network_group_local_retake_after_network_end_does_not_transfer_call_ownership --locked` passed.
  - `cargo test -p tetra-entities --test test_cmce_bs test_network_group_speaker_change_updates_dashboard_after_rf_grant --locked` passed.
  - `cargo test -p tetra-entities --test test_cmce_bs test_network_group_call_end_from_active_network_speaker_enters_hangtime_without_release --locked` passed.
  - `cargo test -p tetra-entities --test test_cmce_bs test_group_setup_sends_proceeding_connect_and_group_setup_with_allocations --locked` passed.
  - `cargo check -p tetra-entities --locked` passed.
  - `git diff --check` passed.
- Deploy verification:
  - Deployed to `chris@192.168.1.179` with
    `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
  - Local build only; no Rust compilation on the Pi/TetraHS.
  - Remote services active/running since `2026-06-10 01:42:00 EEST`:
    - `nexus-bs@chris.service` PID `310398`;
    - `nexus-bs-control@chris.service` PID `310384`;
    - `nexus-bs-dashboard@chris.service` PID `310416`.
  - Deployed core SHA:
    `ba2c6056ba0c6a9721e4c5eea0b492806f85f0697d00a15af972aa3d4df89984`.
  - Remote `/api/snapshot` reported overall health `ok`, Brew v1 online, and
    no active calls at the time of post-deploy inspection.
  - No post-final-deploy `GROUP_TX` on GSSI `226333` had arrived yet; next field
    step is a controlled 226333 RF/Brew audio test and log readback.

Brew External Subscriber Accounting Hardening and Nexus-BS v0.1.62 - 2026-06-10 09:38 EEST:

- Scope is CMCE/Brew interconnect subscriber accounting plus release identity
  bump. No private simplex/duplex, SDS, WAP, parrot, encryption, or RF U-plane
  vocoder behaviour was intentionally changed.
- User reported outside/Brew inconsistency:
  `CMCE: not syncing affiliate for unknown ISSI 2261313 into shared subscriber registry`.
- ETSI baseline remains clause-scoped: local RF group affiliation is an MM
  air-interface procedure under EN 300 392-2, while Brew subscriber events are
  interconnect state. This is not a formal TETRA certification claim.
- Fix:
  - CMCE now keeps Brew-origin subscriber updates out of the shared local RF
    subscriber registry;
  - Brew/external subscriber groups are tracked separately from local MM/RF
    subscriber groups;
  - external Brew listeners still count for local RF -> Brew group setup/routing;
  - Brew -> RF network group setup now requires a real local RF listener, so an
    external-only affiliate does not open a local RF downlink;
  - Brew echo affiliate/deaffiliate for the same ISSI no longer removes the
    local RF affiliation for that terminal.
- Release bump:
  - workspace/package identity moved from `v0.1.61` to `v0.1.62`;
  - control protocol is now `nexus-bs-control-v0.1.62`;
  - telemetry protocol is now `nexus-bs-telemetry-v0.1.62`;
  - README, example config, and systemd sample descriptions now reference
    `Nexus-BS v0.1.62`.
- Verification:
  - `cargo fmt --check` passed.
  - `git diff --check` passed.
  - `cargo test -p tetra-core product_identity_tracks_workspace_version --locked` passed.
  - `cargo test -p tetra-entities control_protocol_tracks_nexus_bs_product_version --locked` passed.
  - `cargo test -p tetra-entities telemetry_protocol_tracks_nexus_bs_product_version --locked` passed.
  - `cargo test -p tetra-entities --test test_cmce_bs brew --locked` passed: 12 tests.
  - `cargo test -p tetra-entities --test test_cmce_bs network_group --locked` passed: 15 tests.
- Deploy verification:
  - Deployed locally built binaries to `chris@192.168.1.179` with
    `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`.
  - Remote services active/running since `2026-06-10 09:35:59 EEST`.
  - Remote `/api/system` returned `product_user_agent=Nexus-BS/v0.1.62`,
    `product_version_tag=v0.1.62`, and `stack_version=v0.1.62-4ab64b2e-modified`
    for the pre-commit field deploy.
  - Journal since `2026-06-10 09:35:59 EEST` had no entries matching
    `not.*syncing.*affiliate`.
  - Journal showed Brew echo affiliate handled on the external path:
    `CMCE: external subscriber affiliate source=Brew issi=2260616 groups=[226333]`.

Dashboard Logs Pause Scroll Fix - 2026-06-10 09:48 EEST:

- Scope is external dashboard JavaScript only. No TETRA air-interface, RF
  scheduling, CMCE, SDS, Brew protocol, WAP, parrot, or service-control
  behaviour was intentionally changed.
- User reported that the Logs page still fought manual scrolling while log
  auto-scroll was paused: the operator could not read logs because the view
  kept returning upward.
- Root cause: live log/snapshot rerenders rebuild the log list DOM; replacing
  the list content reset the log viewer scroll offset. The generic page scroll
  preservation also still ran on the Logs page while auto-scroll was paused.
- Fix:
  - when `logAutoScroll=false`, `renderLogs()` preserves the current
    `#logList.scrollTop` across every rerender;
  - generic active-page scroll restoration is skipped for Logs while paused, so
    manual operator scrolling wins until Play is pressed again;
  - when auto-scroll is enabled, Logs still follows the bottom as before.
- Verification:
  - `node --check dashboard/assets/app.js` passed.
  - `cargo test -p tetra-entities --test test_dashboard_assets --locked` passed.

Dashboard Traffic Consolidation and Parrot Identity Fix - 2026-06-10 10:14 EEST:

- Scope is external dashboard HTML/CSS/JavaScript plus the dashboard asset
  manifest test only. No TETRA RF, UMAC/MAC, MM, CMCE, SDS protocol handling,
  Brew protocol, WAP, parrot audio path, or service-control behaviour was
  intentionally changed.
- User reported the Traffic tab still fought manual scrolling, had redundant
  `Call Control` and `Activity Log` panels, and displayed local Parrot service
  ISSI `99999` as RadioID `not found`.
- Fix:
  - live renders no longer perform generic `window.scrollTo()` restoration on
    any active page, so one-second telemetry/call/snapshot updates do not fight
    manual operator scrolling in Traffic or System;
  - pending page-scroll restoration is cancelled when the operator scrolls;
  - per-page scroll restoration is kept only for explicit dashboard tab
    switches;
  - moved the System `Timeslots` panel above `Host` and `Carrier Plan`, keeping
    TDMA slot state visible before administrative host/carrier details;
  - removed the redundant Traffic `Call Control` and `Activity Log` panels;
  - kept Active Calls as the live voice board and made Last Heard the unified
    voice/SDS event stream;
  - Last Heard now labels events as `Group voice`, `Private voice`, or `SDS`
    instead of exposing raw activity keys;
  - added built-in dashboard identity for local Parrot service:
    callsign `Parrot`, ISSI `99999`, no external RadioID lookup.
- Verification:
  - `node --check dashboard/assets/app.js` passed.
  - `cargo test -p tetra-entities --test test_dashboard_assets --locked` passed.
  - `git diff --check` passed.
- Deploy note:
  - pre-commit field deploy succeeded with
    `RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh`;
  - services were active/running since `2026-06-10 10:14:16 EEST`;
  - this committed hotfix is intended for the follow-up redeploy so the live
    build id points at the traceable dashboard hotfix commit instead of the
    previous `099df5fd`.
