# Build from Source

Nexus-BS is source-available for permitted noncommercial use under the
PolyForm Noncommercial License 1.0.0. Commercial use requires a separate written
agreement. Review `LICENSE`, `COMMERCIAL-LICENSE.md`, and `NOTICE` before
deploying or redistributing builds.

Nexus-BS is engineering work aligned against TETRA standards in specific
project areas. It does not claim formal ETSI/TETRA certification.

Before transmitting, verify your legal operating authority, RF frequency plan,
MCC/MNC, SDR configuration, antennas, gains, dashboard exposure, and any
external network credentials.

## Supported install layout

The tracked systemd templates in `contrib/systemd/` use a flat runtime
directory for binaries and dashboard assets, plus global configuration under
`/etc/nexus-bs`:

```text
/home/<run-user>/nexus-bs/
  nexus-bs
  nexus-bs-control-service
  nexus-bs-dashboard
  nexus-bs-control                  # optional helper script
  dashboard/
    index.html
    assets/
      app.js
      styles.css
      nexus-bs-logo.svg
      nexus-bs-logo.png

/etc/nexus-bs/
  config.toml
  config.toml.fallback
```

The template services are:

```text
contrib/systemd/nexus-bs@.service
contrib/systemd/nexus-bs-control@.service
contrib/systemd/nexus-bs-dashboard@.service
contrib/systemd/journald-nexus-bs-volatile.conf
```

For `NEXUS_USER=nexus`, the enabled units are
`nexus-bs@nexus.service`, `nexus-bs-control@nexus.service`, and
`nexus-bs-dashboard@nexus.service`.

## Prerequisites

Use a Linux build host, or cross-build for the target Linux architecture. The
target runtime must have:

- systemd;
- SoapySDR runtime libraries;
- the SoapySDR hardware module for your SDR;
- permission for the service user to access the SDR device;
- network access if Brew, telemetry, remote control, or a public dashboard is
  used.

On Debian-like systems, install build basics and SoapySDR development files:

```sh
sudo apt update
sudo apt install -y git build-essential pkg-config cmake clang \
  libsoapysdr-dev soapysdr-tools
```

Install Rust with rustup and use a stable toolchain with Rust 2024 edition
support:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
rustup toolchain install stable
rustup default stable
```

Check that SoapySDR can see the target SDR before relying on Nexus-BS:

```sh
SoapySDRUtil --info
SoapySDRUtil --find
SoapySDRUtil --probe
```

Install the hardware-specific SoapySDR module if `--find` or `--probe` does not
show your SDR.

## Clone

```sh
git clone https://github.com/invictus737/nexus-bs.git
cd nexus-bs
git status --short
```

Use a release tag or known branch if you need reproducible behavior:

```sh
git checkout <tag-or-branch>
```

## Build

For a native Linux build on the same architecture as the target:

```sh
cargo check --locked
cargo test -p tetra-config --locked
cargo test -p tetra-entities --locked
cargo build --release -p nexus-bs -p nexus-bs-control -p nexus-bs-dashboard --locked --bins
```

The native build outputs are:

```text
target/release/nexus-bs
target/release/nexus-bs-control-service
target/release/nexus-bs-dashboard
```

For an aarch64 Linux target from another host, use `cargo-zigbuild` and a
target-architecture SoapySDR sysroot. The important rule is that `pkg-config`
must resolve the target SoapySDR `.pc` file, not the build host's SoapySDR:

```sh
cargo install cargo-zigbuild --locked
rustup target add aarch64-unknown-linux-gnu

export SOAPY_SYSROOT=/path/to/aarch64-soapysdr
env \
  PKG_CONFIG_ALLOW_CROSS=1 \
  PKG_CONFIG_LIBDIR="$SOAPY_SYSROOT/lib/pkgconfig" \
  PKG_CONFIG_PATH="$SOAPY_SYSROOT/lib/pkgconfig" \
  LIBRARY_PATH="$SOAPY_SYSROOT/lib" \
  cargo zigbuild --release \
    -p nexus-bs -p nexus-bs-control -p nexus-bs-dashboard \
    --target aarch64-unknown-linux-gnu \
    --locked --bins
```

The cross-build outputs are:

```text
target/aarch64-unknown-linux-gnu/release/nexus-bs
target/aarch64-unknown-linux-gnu/release/nexus-bs-control-service
target/aarch64-unknown-linux-gnu/release/nexus-bs-dashboard
```

## Install copied files

Set the service user and build output directory. Use a non-root account for the
runtime service:

```sh
export NEXUS_USER=nexus
export BUILD_DIR=target/release
```

For a cross-build, use the target output directory instead:

```sh
export BUILD_DIR=target/aarch64-unknown-linux-gnu/release
```

Create the runtime and config directories:

```sh
sudo install -d -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 /home/"$NEXUS_USER"/nexus-bs
sudo install -d -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 /home/"$NEXUS_USER"/nexus-bs/dashboard/assets
sudo install -d -o root -g "$NEXUS_USER" -m 0750 /etc/nexus-bs
```

Copy binaries and the optional local control helper:

```sh
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 "$BUILD_DIR"/nexus-bs /home/"$NEXUS_USER"/nexus-bs/nexus-bs
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 "$BUILD_DIR"/nexus-bs-control-service /home/"$NEXUS_USER"/nexus-bs/nexus-bs-control-service
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 "$BUILD_DIR"/nexus-bs-dashboard /home/"$NEXUS_USER"/nexus-bs/nexus-bs-dashboard
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 scripts/nexus-bs-control /home/"$NEXUS_USER"/nexus-bs/nexus-bs-control
```

Seed the example config as both primary and fallback only when the live files do
not already exist. The primary config is the live file edited by operators and
by the dashboard:

```sh
if [ ! -e /etc/nexus-bs/config.toml ]; then
  sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0600 example_config/config.toml /etc/nexus-bs/config.toml
fi

if [ ! -e /etc/nexus-bs/config.toml.fallback ]; then
  sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0600 example_config/config.toml /etc/nexus-bs/config.toml.fallback
fi
```

Copy dashboard assets:

```sh
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0644 dashboard/index.html /home/"$NEXUS_USER"/nexus-bs/dashboard/index.html
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0644 dashboard/assets/app.js /home/"$NEXUS_USER"/nexus-bs/dashboard/assets/app.js
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0644 dashboard/assets/styles.css /home/"$NEXUS_USER"/nexus-bs/dashboard/assets/styles.css
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0644 dashboard/assets/nexus-bs-logo.svg /home/"$NEXUS_USER"/nexus-bs/dashboard/assets/nexus-bs-logo.svg
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0644 dashboard/assets/nexus-bs-logo.png /home/"$NEXUS_USER"/nexus-bs/dashboard/assets/nexus-bs-logo.png
```

If the SDR appears as a device owned by a hardware-access group, add the runtime
user to the appropriate group for your distribution and device. Common examples
are `plugdev` and `dialout`, but verify locally:

```sh
sudo usermod -aG plugdev,dialout "$NEXUS_USER"
```

Log out and back in, or restart the service user session, after changing group
membership.

## Prepare config

Edit the live config:

```sh
sudoedit /etc/nexus-bs/config.toml
```

At minimum, review and set:

- `service_name`: for template systemd installs, set it to
  `nexus-bs@<run-user>.service`;
- `[phy_io.soapysdr]`: TX/RX frequencies, sample rate, SDR device selector,
  antennas, gains, and calibration settings;
- `[net_info]`: MCC and MNC values appropriate for your operation;
- `[cell_info]`: carrier plan, duplex spacing, frequency offset, colour code,
  location area, local SSI/GSSI routing policy, and enabled services;
- `[dashboard]`: core API bind and port. The recommended split deployment uses
  `bind = "127.0.0.1"` and `port = 18080`;
- `[command]`: the local control WebSocket. The service template uses
  `127.0.0.1:9002`;
- `[brew]`: only enable this section when you have valid operator permission and
  non-placeholder credentials;
- dashboard username/password if the dashboard is reachable by other users.

Keep `/etc/nexus-bs/config.toml.fallback` as a known-good copy. If the primary
config fails to parse, `nexus-bs` tries the fallback file; the dashboard and
logs report that fallback mode is active.

## Install and enable systemd services

Install the tracked service templates:

```sh
sudo install -m 0644 contrib/systemd/nexus-bs@.service /etc/systemd/system/nexus-bs@.service
sudo install -m 0644 contrib/systemd/nexus-bs-control@.service /etc/systemd/system/nexus-bs-control@.service
sudo install -m 0644 contrib/systemd/nexus-bs-dashboard@.service /etc/systemd/system/nexus-bs-dashboard@.service
```

Install a core-service drop-in that uses the global live config and fallback
paths:

```sh
sudo install -d -m 0755 /etc/systemd/system/nexus-bs@.service.d
sudo tee /etc/systemd/system/nexus-bs@.service.d/10-global-config.conf >/dev/null <<'EOF'
[Service]
Environment=NEXUS_BS_PERSISTENT_CONFIG=/etc/nexus-bs/config.toml
ExecStartPre=
ExecStartPre=/usr/bin/install -m 0600 /etc/nexus-bs/config.toml /run/nexus-bs-%i/config.toml
ExecStartPre=/bin/sh -c 'if [ -f "$1" ]; then /usr/bin/install -m 0600 "$1" "$2"; fi' nexus-bs-copy-fallback /etc/nexus-bs/config.toml.fallback /run/nexus-bs-%i/config.toml.fallback
ExecStart=
ExecStart=/home/%i/nexus-bs/nexus-bs /run/nexus-bs-%i/config.toml
EOF
```

Optional: install the volatile circular journald profile for appliance-style
systems:

```sh
sudo install -D -m 0644 contrib/systemd/journald-nexus-bs-volatile.conf /etc/systemd/journald.conf.d/90-nexus-bs-volatile.conf
sudo systemctl restart systemd-journald.service
```

Reload systemd and start services in dependency order:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now nexus-bs-control@"$NEXUS_USER".service
sudo systemctl enable --now nexus-bs@"$NEXUS_USER".service
sudo systemctl enable --now nexus-bs-dashboard@"$NEXUS_USER".service
```

The core service copies `/etc/nexus-bs/config.toml` into a volatile runtime
file before startup. Dashboard config edits still target
`/etc/nexus-bs/config.toml`.

## Verify

Check service state:

```sh
systemctl status nexus-bs-control@"$NEXUS_USER".service
systemctl status nexus-bs@"$NEXUS_USER".service
systemctl status nexus-bs-dashboard@"$NEXUS_USER".service
```

Check recent logs:

```sh
journalctl -u nexus-bs-control@"$NEXUS_USER".service -n 80 --no-pager
journalctl -u nexus-bs@"$NEXUS_USER".service -n 160 --no-pager
journalctl -u nexus-bs-dashboard@"$NEXUS_USER".service -n 80 --no-pager
```

Check the core dashboard API on the target host:

```sh
curl --fail http://127.0.0.1:18080/api/system
```

Open the public dashboard from a browser on the same trusted network:

```text
http://<target-ip>:8080
```

If dashboard login is enabled, authenticate through the dashboard login page.
For internet-facing access, place an HTTPS reverse proxy in front of the
dashboard rather than exposing clear-text login traffic.

## Manual smoke test without systemd

For a short local smoke test, run the control service, core, and dashboard in
separate shells from the install directory:

```sh
cd /home/"$NEXUS_USER"/nexus-bs
./nexus-bs-control-service --listen 127.0.0.1:9002
```

```sh
cd /home/"$NEXUS_USER"/nexus-bs
NEXUS_BS_PERSISTENT_CONFIG=/etc/nexus-bs/config.toml \
NEXUS_BS_DASHBOARD_STATIC_DIR=/home/"$NEXUS_USER"/nexus-bs/dashboard \
NEXUS_BS_CORE_DASHBOARD_BIND=127.0.0.1 \
NEXUS_BS_CORE_DASHBOARD_PORT=18080 \
./nexus-bs /etc/nexus-bs/config.toml
```

```sh
cd /home/"$NEXUS_USER"/nexus-bs
NEXUS_BS_DASHBOARD_BIND=0.0.0.0 \
NEXUS_BS_DASHBOARD_PORT=8080 \
NEXUS_BS_DASHBOARD_CORE=127.0.0.1:18080 \
NEXUS_BS_DASHBOARD_STATIC_DIR=/home/"$NEXUS_USER"/nexus-bs/dashboard \
./nexus-bs-dashboard
```

Stop the manual processes before starting the systemd services.

## Updating an existing source install

Build the new revision, stop services, copy binaries/assets, then restart:

```sh
sudo systemctl stop nexus-bs-dashboard@"$NEXUS_USER".service
sudo systemctl stop nexus-bs@"$NEXUS_USER".service
sudo systemctl stop nexus-bs-control@"$NEXUS_USER".service
```

Repeat the binary, dashboard asset, and systemd template copy steps. Do not
overwrite `/etc/nexus-bs/config.toml` or `/etc/nexus-bs/config.toml.fallback`
unless you intentionally want to replace the live configuration.

```sh
sudo systemctl daemon-reload
sudo systemctl start nexus-bs-control@"$NEXUS_USER".service
sudo systemctl start nexus-bs@"$NEXUS_USER".service
sudo systemctl start nexus-bs-dashboard@"$NEXUS_USER".service
```

## Troubleshooting

`cargo` cannot find `SoapySDR`:

- Install `libsoapysdr-dev` and `pkg-config` on native builds.
- For cross builds, point `PKG_CONFIG_LIBDIR` and `LIBRARY_PATH` at a SoapySDR
  sysroot for the target architecture.
- Do not let cross builds link against the build host's SoapySDR library.

No SDR is found at runtime:

- Run `SoapySDRUtil --find` and `SoapySDRUtil --probe` on the target.
- Install the hardware-specific SoapySDR module.
- Check USB/network visibility and service-user device permissions.
- Confirm `[phy_io.soapysdr].device`, antenna names, and gain names match
  `SoapySDRUtil --probe`.

`nexus-bs@<user>.service` fails immediately:

- Check `journalctl -u nexus-bs@<user>.service -n 160 --no-pager`.
- Confirm `/home/<user>/nexus-bs/nexus-bs` exists and is executable.
- Confirm `/etc/nexus-bs/config.toml` exists, is readable by the run user, and
  parses.
- Confirm `/etc/nexus-bs/config.toml.fallback` is a known-good config.
- Check that `service_name` in `config.toml` matches the template unit name.

Dashboard loads but API calls fail:

- Confirm `nexus-bs@<user>.service` is active.
- Confirm the core API is on `127.0.0.1:18080`.
- Confirm `nexus-bs-dashboard@<user>.service` uses
  `NEXUS_BS_DASHBOARD_CORE=127.0.0.1:18080`.
- Check for port conflicts on `8080` and `18080`.

Control commands do not work:

- Confirm `nexus-bs-control@<user>.service` is active.
- Confirm `/run/nexus-bs-<user>/control.commands` exists and is a FIFO.
- Use `/home/<user>/nexus-bs/nexus-bs-control help` from the target host.

Service start fails with scheduling or permission errors:

- The core template requests real-time round-robin scheduling and CPU affinity
  for RF/DSP timing. Check systemd logs for `SETSCHED`, `RTPRIO`, or cgroup
  errors.
- Verify the host allows the configured scheduling policy, or adapt the unit for
  your platform before enabling RF transmission.

Config edits break startup:

- Fix `/etc/nexus-bs/config.toml`.
- Keep `/etc/nexus-bs/config.toml.fallback` as the last known-good file.
- Restart services after fixing the primary config.

RF behavior is poor after a successful start:

- Recheck frequency plan, duplex spacing, `reverse_operation`, SDR clock error,
  antennas, gains, and local legal limits.
- Use low-risk lab checks before live operation.
- Do not treat successful compilation or service startup as conformance or RF
  performance validation.
