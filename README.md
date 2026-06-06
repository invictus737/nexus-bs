# Nexus-BS v0.1.56

> **TETRA base station software for amateur radio operators and researchers.**
> Built in Rust. Runs on a Raspberry Pi with a LimeSDR. Works with real TETRA radios.

Nexus-BS is a TETRA base-station stack derived from the BlueStation and FlowStation project lineages, with ongoing dashboard, SDR and ETSI EN 300 392-2 clause-scoped hardening work.

Tested hardware: **LimeSDR Mini 2.0** · **Motorola MXP600** · **Motorola MTM800E** · **Motorola MTM5400**

---

## What it does

Nexus-BS implements a functional TETRA base station (BS) in software. You plug in a supported SDR, point it at your TETRA radios, and get:

- Group calls, individual (P2P) calls, half-duplex PTT — all working
- SDS messaging (text messages between radios)
- Network interconnect via [Brew / TetraPack](https://wiki.tetrapack.online/books/tetra/page/brew) — connects your local cell to BrandMeister or TetraPack
- UTC time broadcast so radios sync their clocks automatically
- A web dashboard at `http://<bts-ip>:8080` for monitoring and remote management

---

## Feature overview

| Feature | Status |
|---|---|
| Group calls (local) | ✅ |
| Group calls via Brew (BrandMeister / TetraPack) | ✅ |
| Full-duplex P2P calls (local + Brew) | ✅ |
| Half-duplex P2P calls (simplex PTT) | ✅ |
| SDS forwarding (local + Brew) | ✅ |
| WAP MVP over SDS Type4 | ✅ |
| UTC time broadcast (D-NWRK-BROADCAST) | ✅ |
| T351 periodic re-registration | ✅ |
| Home Mode Display (SDS-TL text) | ✅ |
| Supplemental SDS broadcast (custom PID) | ✅ |
| ISSI whitelist (access control) | ✅ |
| Local SSI ranges (local-only traffic) | ✅ |
| Remote control via U-STATUS from radio | ✅ |
| Neighbor cell broadcast | ✅ |
| Web dashboard | ✅ |
| OTA update button | Disabled for now |
| HTTP Basic Auth on dashboard | ✅ |
| Fallback config on bad edit | ✅ |
| Live SDS broadcast queue | ✅ |
| Edit inactive config profiles in dashboard | ✅ |
| System / RF hardware tab | ✅ |
| Coordinated handover | 🔜 |
| Emergency calls | 🔜 |
| Authentication (TEA) | 🔜 |
| AIE encryption | 🔜 |
| Multi-carrier (2× SDR) | 🔜 |

---

## Installation

### Requirements

- **Rust** — latest stable (`rustup update stable`)
- **SoapySDR** with drivers for your SDR
- A supported SDR — LimeSDR Mini 2.0 is the reference hardware

### From git

```bash
git clone <nexus-bs-repository-url>
cd nexus-bs
cp example_config/config.toml ./config.toml
# Edit config.toml — at minimum set tx_freq, rx_freq, mcc, mnc
cargo build --release
./target/release/nexus-bs config.toml
```

### As a systemd service

The current service layout is flat under the runtime user's home directory:

```text
/home/<user>/nexus-bs/nexus-bs
/home/<user>/nexus-bs/nexus-bs-control-service
/home/<user>/nexus-bs/nexus-bs-control
/home/<user>/nexus-bs/config.toml
```

Install the templated units from `contrib/systemd/` and start them for the target user:

```bash
install -m 0644 contrib/systemd/nexus-bs@.service /etc/systemd/system/
install -m 0644 contrib/systemd/nexus-bs-control@.service /etc/systemd/system/
install -D -m 0644 contrib/systemd/journald-nexus-bs-volatile.conf /etc/systemd/journald.conf.d/90-nexus-bs-volatile.conf
systemctl daemon-reload
systemctl enable --now nexus-bs-control@<user>.service nexus-bs@<user>.service
```

The service name (`nexus-bs`) must match the `service_name` used in any restart/shutdown commands. The legacy `contrib/systemd/nexus-bs.service` remains only as a reference for old single-user installs.

---

## Configuration

The full config is documented in `example_config/config.toml`. Key sections:

### Mandatory

```toml
[phy_io.soapysdr]
tx_freq = 438025000   # DL frequency in Hz
rx_freq = 433025000   # UL frequency in Hz

[net_info]
mcc = 204             # Mobile Country Code
mnc = 1337            # Mobile Network Code

[cell_info]
freq_band = 4         # 400 MHz band
main_carrier = 1521
duplex_spacing = 4
location_area = 2
colour_code = 1
```

### Timing (Nexus-BS-specific)

| Parameter | Default | Description |
|---|---|---|
| `hangtime_secs` | `5` | Hold group call circuit after floor release (0–300s) |
| `call_timeout_secs` | `120` | Max call duration before forced D-RELEASE (0 = unlimited) |
| `ul_inactivity_secs` | `3` | UL silence before forced TX-CEASED (1–30s) |
| `call_preemptive` | `false` | Enable CMCE D-TX INTERRUPT for configured pre-emptive group-call floor withdrawal. Alias: `transmission_interruption_enabled` |
| `energy_saving_mode` | `eg3` | Energy economy group used by MM/UMAC scheduling; set `stay_alive` for continuous monitoring during diagnostics |
| `periodic_registration_secs` | `3600` | Local periodic-registration watchdog; `0` = disabled |
| `allowed_gssi_ranges` | unset | Optional MM group provisioning ranges; unset accepts dynamic GSSIs, set ranges reject unprovisioned group attach as unknown group identity |

### Dashboard

```toml
[dashboard]
port = 8080

# Optional: HTTP Basic Auth
username = "admin"
password = "changeme"

# Optional: reserved git source path for future OTA updates.
# OTA update is disabled in Nexus-BS v0.1.56.
# source_dir = "/opt/nexus-bs"
```

### Fallback config

If Nexus-BS fails to parse `config.toml` at startup (e.g. after a bad edit in the dashboard), it automatically tries `config.toml.fallback`. Create it once from a known-good config:

```bash
cp config.toml config.toml.fallback
```

When running on fallback, the dashboard shows a persistent red warning banner with the parse error, so you can fix the primary config remotely without losing access.

### Home Mode Display (callsign on radio screen)

```toml
[cell_info.home_mode_display]
text = "Nexus-BS"            # Shown on radio home screen
interval_multiframes = 96    # ≈ 96 seconds
protocol_id = 130            # 0x82 SDS-TL text
text_coding_scheme = "LATIN"
```

### Access control

```toml
[security]
issi_whitelist = [2260571, 2260572]   # Only these ISSIs can register
```

### Remote control from radio (U-STATUS)

```toml
[cell_info.sds_command_control]
authorized_issis = [2260570, 2260571]

[[cell_info.sds_command_control.commands]]
status_code = 36865
action = "restart"        # restart / shutdown / kick_all

[[cell_info.sds_command_control.commands]]
status_code = 36867
action = "kick_all"
```

### Brew (TetraPack / BrandMeister interconnect)

```toml
[brew]
host = "core.tetraflow.ro"
port = 9000
tls = true
username = 123456700
password = "hotspot_password"
```

---

## Web dashboard

Available at `http://<bts-ip>:8080` when `[dashboard]` is configured.

**Radios tab** — live table of registered terminals with ISSI, groups, RSSI signal bar, energy saving mode, last seen time. Kick button forces immediate re-registration. SDS button sends a text message. Timeslot visualizer shows TS2–TS4 state in real time — idle (grey), call allocated (amber), voice active (red flash with animated waveform).

**Calls tab** — active calls with caller, destination, duration, simplex/duplex.

**Last Heard** — rolling history of call starts and SDS activity.

**Log tab** — live log with level filter and autoscroll.

**Config tab** — edit the active `config.toml` directly. Save writes to disk; restart applies changes. Backup and restore buttons.

**System tab:**
- BTS / Brew connection status
- System uptime, hostname
- CPU model, core count, load bar, RAM usage bar
- CPU temperature (where available)
- RF hardware info (SoapySDR probe output)
- Auto-refresh checkbox (5s interval)
- Config profiles — activate, edit inactive profiles directly in a modal editor
- Live SDS broadcast queue — broadcast a text message to all radios on the cell, repeating at the HMD interval until deleted or repeat count exhausted
- OTA update is intentionally disabled for now; update controls are visible but grayed out

### WAP MVP over SDS

Nexus-BS v0.1.56 includes an operator-triggered WAP MVP carried as SDS Type4. This is not a full SNDCP/IP packet-data bearer, so keep `sndcp_service = false` unless that bearer is implemented and verified.

From the flat install directory, send the default WML page to a terminal ISSI:

```bash
./nexus-bs-control sendwap 16777215 2260618 false
```

The default page contains:

```text
Hello! You are running Nexus-BS. Gretings and 73! from Chris YO3TCO!
```

For terminal browsers that support basic HTML/color handling, use:

```bash
./nexus-bs-control sendwapcolor 16777215 2260618 false
```

---

## Key fixes vs upstream

**ExpiryOfTimer crash loop** — `release_group_call` now sends `NetworkCallEnd` to Brew when a network-initiated group call expires. Without this, Brew kept the call alive and re-issued `NetworkCallStart` with new speakers, generating thousands of `ExpiryOfTimer` releases per minute and crashing the stack.

**Simplex P2P (half-duplex PTT)** — `transmission_request_permission` correctly set to `false` in `D-CONNECT`, `D-CONNECT-ACK`, `D-TX-CEASED`, and `D-TX-GRANTED` (`false` encodes EN 300 392-2 14.8.43/table 14.81 value 0, allowed to request transmission). On `U-TX-CEASED`, BS sends `D-TX-CEASED` to both private-call parties so both terminals leave the active transmission state; it sends `D-TX-GRANTED(Granted)` only after a queued or new `U-TX-DEMAND`, avoiding an unsolicited grant while still unlocking the next PTT request.

**Sepura post-PTT RoamingLocationUpdating** — Sepura terminals send `RoamingLocationUpdating` after every PTT release, not just on power cycle. Without the heuristic (< 60s since last registration → treat as soft re-attach), CMCE briefly loses track of the terminal and the next PTT is denied. Fixed with timing-based soft re-attach detection.

**BCD external subscriber number** — decoder was shifting from nibble count instead of from bit 64, producing incorrect ISSI values in certain call scenarios.

**UL audio routing to Brew** — `TmdCircuitDataInd` was not routed to Brew in `cmce_bs.rs`, causing one-way audio on Brew-interconnected calls.

**SDS ACK for ISSI 9999** — SDS ACK for the local BS control ISSI was being forwarded to Brew, generating spurious traffic. Now absorbed locally.

**Chan_alloc in DConnect for echo service 999** — echo service calls were being allocated without a traffic channel, causing audio to fail.

---

## Branches

| Branch | Purpose |
|---|---|
| `main` | Stable, tested releases |
| `beta` | Work in progress, new features |

---

## Credits

- **Harald Welte** and the **osmocom** team for the foundational osmocom-tetra work
- **Tatu Peltola** for rust-soapysdr timestamping and the native Rust Viterbi encoder/decoder used in LMAC
- **BlueStation Project** for the upstream TETRA BS foundation and protocol structure
- **FlowStation Project** for operational field features, dashboard evolution and deployment hardening
- **SXCEIVER** for SDR hardware ecosystem and station-oriented RF integration context
- **Stichting NLnet** for partially funding this work through the [RETETRA3 grant](https://nlnet.nl/project/RETETRA3/)
- All upstream, fork, testing, documentation, hardware, dashboard and integration contributors, including the operator community — ON6RF, EA7KEN, BU2GQ, DK5RTA, DO5MF, ES4TIX, DK5RTA and others — for testing, bug reports, and feature requests that shaped this release

---

## License

Apache 2.0 — see [LICENSE](LICENSE)
