# Nexus-BS Mission Readiness Backlog

This file tracks the engineering path toward mission-critical Nexus-BS operation.
It is not a formal ETSI/TETRA certification claim and it is not an uptime
guarantee. Protocol changes remain clause-scoped under ETSI EN 300 392-2 and
must be documented in `timeline.md` with tests and RF evidence.

Current protected checkpoints:

- `v0.1.57`: private simplex RF-good checkpoint.
- `v0.1.60`: legacy local GSSI group-call compatibility checkpoint.

## Active Goals

Primary goal:

- Robust 24x7/365 engineering target for local group calls, private simplex,
  private duplex, SDS/WAP, restart recovery, affiliation retention, group/scan
  stability, bounded queues, volatile/circular logging, systemd recovery and Pi
  appliance operation.

Secondary goal:

- Move the operator dashboard out of the Rust binary and replace it with a
  Nexus-BS-owned operational dashboard. The UI must be dense, technical and
  useful, with no marketing/landing-page layout and no FlowStation product
  framing.

## P0 Protocol Backlog

1. Local GSSI group calls
   - Clauses: EN 300 392-2 14.5.2.2.1(b/e), 14.5.2.3, 21.4.3.1, 23.5, 23.8.1, 23.8.5.
   - Risk: legacy `legacy_gssi_group_call` is a compatibility profile, not a
     general protocol rule. Regression risks are same-speaker retake ordering,
     stale retake suppression, different-speaker queue preservation, pending
     release versus fresh `U-SETUP`, and large listener-group behavior.
   - Required tests: old Motorola repeated PTT on GSSI `226333`, 3-radio
     ping-pong, same-speaker retake before different-speaker queue, pending
     release plus new setup, unaffiliated requester rejection, large local group
     listener fallback, UL inactivity regrant then forced cease.

2. Private simplex and duplex
   - Clauses: 14.5.1.1.1, 14.5.1.1.2, 14.5.1.2.1, 14.5.1.3.1, 14.5.1.3.3, Annex D.4, 23.5.
   - Risk: setup grant polarity, caller/called connect delivery, first PTT,
     release drain, peer reboot/No Answer behavior and duplex fallback remain
     edge-sensitive.
   - Required tests: caller-first and called-first simplex, duplicate
     `U-CONNECT`, lost called/caller ACK recovery, same-speaker retake after
     tail, queued peer handoff, close from holder/passive peer, duplex two-slot
     audio path, requested duplex with called simplex offer.

3. MM attach, affiliation and restart recovery
   - Clauses: 16.4.1.1, 16.4.3, 16.4.4, 16.8, 16.9.2.8, 16.9.3.4, 16.10.17,
     16.10.20, 16.10.27a, 16.10.35a, T352/T353.
   - Risk: restart recovery and periodic registration must not drop CMCE/Brew
     group routes while a terminal is recoverable.
   - Required tests: restart cache with no report, empty group report clears
     cache, split reports above group limit, detach-all plus retained groups,
     lost group ACK with T353 rollback, first periodic expiry preserves active
     route, second expiry removes.

4. Energy economy auto/EG scheduling
   - Clauses: 16.7.1, 16.10.9, 16.10.10, 16.10.35a, 22.3.2.3, 23.5.2.2.7,
     23.7.6, T.210.
   - Risk: `auto` must accept terminal-requested mode without imposing EG.
     Once EG is active, UMAC scheduling must preserve group, private and SDS/WAP
     delivery windows.
   - Required tests: mixed StayAlive/EG group call, private setup to sleeping
     EG MS, group affiliation changes during active suspension, restart recovery
     with EG terminal, SDS/WAP delivery to EG listeners, explicit EG lab mode
     timeout/fallback.

## P0 Runtime Backlog

1. Fix dashboard config persistence with `/run` runtime copy
   - Current risk: systemd copies `/home/%i/nexus-bs/config.toml` to
     `/run/nexus-bs-%i/config.toml`, but dashboard edits write the runtime copy.
     Edits can be lost on restart.
   - Acceptance: edit config through dashboard, restart service, prove persistent
     home config and runtime copy both retain the change. Corrupt primary config,
     boot fallback, repair through dashboard, restart and prove repair survives.

2. Bound cross-thread and network queues
   - Current risk: some telemetry/control/Brew/log channels are unbounded.
   - Acceptance: overflow drops or coalesces non-critical telemetry/log/control
     messages without blocking RF; synthetic flood keeps RSS under a fixed limit.

3. Cap dashboard HTTP bodies and connection concurrency
   - Current risk: large `Content-Length` or many idle sockets can consume memory
     and threads.
   - Acceptance: oversized POST returns `413` or closes early without RSS growth;
     slow clients hit a connection cap.

4. Add readiness/watchdog and supervise worker death
   - Current risk: `Type=simple` only proves process liveness, not RF/control
     worker health.
   - Acceptance: systemd watchdog/readiness is configured; injected worker exit
     causes degraded health alarm or controlled restart.

## P1 Backlog

- SDS/WAP Type4 hardening: PID and length boundaries, report suppression for
  group/broadcast, remembered PID delivery reports, ambiguous ISSI/GSSI drop.
- UMAC/LMAC grant timing: requester RA ACK ownership, STCH floor-grant priority,
  deferred Block2/TCH/S expiry, frame-18 boundaries, pending RA ACK bounds.
- Systemd resource policy: MemoryMax, TasksMax, LimitNOFILE, OOMPolicy,
  NoNewPrivileges, ProtectSystem, writable-path exceptions and documented SDR
  exceptions.
- Panic-free volatile logging under read-only roots.
- Atomic, health-gated deploy with rollback and companion binary/script update.
- Brew/control reconnect outage testing without queue growth.

## Dashboard Migration

Add a new dashboard asset path instead of reusing `[dashboard].source_dir`, which
is reserved for OTA source override.

Target config:

```toml
[dashboard]
bind = "0.0.0.0"
port = 8080
static_dir = "/usr/share/nexus-bs/dashboard"
```

Serving strategy:

- Keep the Rust dashboard server as the same-origin API/control gateway.
- Serve `/` and static assets from `static_dir`.
- Keep `/login`, `/api/login`, `/api/logout`, `/api/*` and `/ws` in Rust.
- Keep embedded HTML only as temporary fallback during migration.
- Do not make normal Cargo tests depend on Node/frontend tooling.

Operator UI scope:

- Keep: RF health, service state, subscribers, affiliations, active calls, last
  heard, SDS/WAP, Brew state, logs, alarms, system health and config/fallback.
- Remove: FlowStation product framing, lineage marketing, visible
  OTA controls until implemented, `fs_*` localStorage naming.
- Gate: restart/shutdown, kick, raw config editor, WiFi mutation, WAP shortcut
  and live SDS broadcast.

Dashboard acceptance gates:

- Dashboard can change/build without recompiling the TETRA core.
- Product identity is Nexus-BS only.
- No FlowStation product or current-governance strings in shipped dashboard assets.
- WebSocket snapshot and deltas remain compatible or are versioned as
  `nexus-bs.dashboard.v1`.
- Slow clients are dropped at bounded queue caps.

## QA Gates

Stage 1 automated gate:

```sh
cargo test -p tetra-entities --locked
cargo test -p tetra-pdus --locked
cargo test -p tetra-config --locked
cargo test -p tetra-core --locked
cargo check -p nexus-bs --locked
git diff --check
```

Stage 2 focused mission tests:

```sh
cargo test -p tetra-entities --test test_cmce_bs --locked
cargo test -p tetra-entities --test test_umac_bs --locked
cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked
cargo test -p tetra-entities --test test_sds_bs wap --locked
cargo test -p tetra-entities --test test_cmce_bs p2p --locked
```

Stage 3 deploy gate:

```sh
REMOTE=chris@192.168.1.179 REMOTE_SERVICE=nexus-bs@chris.service ./scripts/nexus-bs-test-deploy.sh
```

RF gates:

- Clean journal, restart service, verify attach/affiliation.
- GSSI `226333`: same-speaker repeated PTT, alternating speakers, rapid retake.
- Private simplex both directions with release from both parties.
- Private duplex where terminal support exists.
- SDS ISSI, SDS GSSI and WAP MVP delivery.
- Restart BS while idle and after recent RF activity; verify groups return.

Soak gates:

- 8 hour smoke.
- 24 hour acceptance.
- 72 hour mission soak.
- Watch for unexpected restarts, stuck calls, subscriber loss, sustained late TX
  blocks, sustained lost samples, panics, unbounded RSS and fallback config outside
  a deliberate fallback drill.

## Readiness Metric

This metric is internal mission-readiness evidence, not certification:

- 25% automated Rust gates.
- 15% deploy/restart recovery.
- 20% RF attach/group/private-call behavior.
- 15% SDS/WAP/dashboard externalization.
- 15% soak stability.
- 10% legacy terminal matrix.

Current status after v0.1.60: architecture and evidence map are defined; mission
readiness is not yet proven by RF/soak.
