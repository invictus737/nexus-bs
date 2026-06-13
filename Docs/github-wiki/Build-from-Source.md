<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Classic Source Install

This is the normal operator path: pull the code, build release binaries, copy
the files into a simple runtime folder, edit `config.toml`, and start systemd.

Nexus-BS is source-available under the repository licensing terms. It does not
claim formal ETSI/TETRA certification.

## 1. Install Prerequisites

On the target Linux host:

```sh
sudo apt update
sudo apt install -y git build-essential pkg-config cmake clang \
  libsoapysdr-dev soapysdr-tools
```

Install Rust if it is not already installed:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
rustup default stable
```

Check the SDR before starting the base station:

```sh
SoapySDRUtil --find
SoapySDRUtil --probe
```

Install the SoapySDR hardware module required by your SDR if it is missing.

## 2. Get or Update the Source

First install:

```sh
git clone https://github.com/invictus737/nexus-bs.git
cd nexus-bs
```

Existing install:

```sh
cd nexus-bs
git pull --ff-only
```

Use a release tag if you need a fixed version:

```sh
git checkout v0.1.65
```

## 3. Build Release Binaries

```sh
cargo build --release --locked \
  -p nexus-bs \
  -p nexus-bs-control \
  -p nexus-bs-dashboard \
  --bins
```

The binaries are created here:

```text
target/release/nexus-bs
target/release/nexus-bs-control-service
target/release/nexus-bs-dashboard
```

## 4. Install Files

Choose the Linux user that will run the service:

```sh
export NEXUS_USER=nexusbs
```

Create it if needed:

```sh
sudo useradd -m -s /bin/bash "$NEXUS_USER"
```

Create the runtime and config folders:

```sh
sudo install -d -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 /home/"$NEXUS_USER"/nexus-bs
sudo install -d -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 /home/"$NEXUS_USER"/nexus-bs/dashboard/assets
sudo install -d -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0750 /etc/nexus-bs
```

Copy binaries:

```sh
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0755 \
  target/release/nexus-bs \
  target/release/nexus-bs-control-service \
  target/release/nexus-bs-dashboard \
  scripts/nexus-bs-control \
  /home/"$NEXUS_USER"/nexus-bs/
```

Copy dashboard assets:

```sh
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0644 \
  dashboard/index.html \
  /home/"$NEXUS_USER"/nexus-bs/dashboard/

sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0644 \
  dashboard/assets/app.js \
  dashboard/assets/styles.css \
  dashboard/assets/nexus-bs-logo.svg \
  dashboard/assets/nexus-bs-logo.png \
  /home/"$NEXUS_USER"/nexus-bs/dashboard/assets/
```

Seed the config only on first install:

```sh
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0600 example_config/config.toml /etc/nexus-bs/config.toml
sudo install -o "$NEXUS_USER" -g "$NEXUS_USER" -m 0600 example_config/config.toml /etc/nexus-bs/config.toml.fallback
sudo ln -sf /etc/nexus-bs/config.toml /home/"$NEXUS_USER"/nexus-bs/config.toml
sudo ln -sf /etc/nexus-bs/config.toml.fallback /home/"$NEXUS_USER"/nexus-bs/config.toml.fallback
```

For updates, do not overwrite `/etc/nexus-bs/config.toml` unless you
intentionally want to reset the live station configuration.

## 5. Install Systemd Service Files

```sh
sudo install -m 0644 \
  contrib/systemd/nexus-bs-control@.service \
  contrib/systemd/nexus-bs@.service \
  contrib/systemd/nexus-bs-dashboard@.service \
  /etc/systemd/system/
sudo systemctl daemon-reload
```

The service files run:

- `nexus-bs-control@USER.service` - local command bridge.
- `nexus-bs@USER.service` - RF/TETRA base-station core.
- `nexus-bs-dashboard@USER.service` - browser dashboard on port `8080`.

## 6. Edit config.toml

```sh
sudoedit /etc/nexus-bs/config.toml
```

At minimum, verify:

- SDR driver/device, antennas, gains, sample rate and clock settings.
- TX/RX frequencies and duplex spacing for your authorized RF plan.
- MCC/MNC, carrier plan, colour code, LA and local SSI/GSSI policy.
- `service_name = "nexus-bs@<run-user>.service"`.
- Dashboard username/password if other users can reach the dashboard.
- Brew/TetraPack settings only if you have valid credentials and permission.

Keep `/etc/nexus-bs/config.toml.fallback` as a known-good fallback.

## 7. Start the Base Station

```sh
sudo systemctl enable --now nexus-bs-control@"$NEXUS_USER".service
sudo systemctl enable --now nexus-bs@"$NEXUS_USER".service
sudo systemctl enable --now nexus-bs-dashboard@"$NEXUS_USER".service
```

Check status:

```sh
systemctl status nexus-bs-control@"$NEXUS_USER".service
systemctl status nexus-bs@"$NEXUS_USER".service
systemctl status nexus-bs-dashboard@"$NEXUS_USER".service
```

Open the dashboard:

```text
http://<target-ip>:8080
```

Check logs:

```sh
journalctl -u nexus-bs@"$NEXUS_USER".service -n 160 --no-pager
journalctl -u nexus-bs-dashboard@"$NEXUS_USER".service -n 80 --no-pager
```

## Updating Later

```sh
cd nexus-bs
git pull --ff-only
cargo build --release --locked -p nexus-bs -p nexus-bs-control -p nexus-bs-dashboard --bins
sudo systemctl stop nexus-bs-dashboard@"$NEXUS_USER".service nexus-bs@"$NEXUS_USER".service nexus-bs-control@"$NEXUS_USER".service
```

Repeat the binary, dashboard asset and service-file copy steps. Do not overwrite
`/etc/nexus-bs/config.toml`.

```sh
sudo systemctl daemon-reload
sudo systemctl start nexus-bs-control@"$NEXUS_USER".service nexus-bs@"$NEXUS_USER".service nexus-bs-dashboard@"$NEXUS_USER".service
```

## Troubleshooting

- `cargo` cannot find SoapySDR: install `libsoapysdr-dev` and `pkg-config`.
- No SDR is found: run `SoapySDRUtil --find` and install the SDR hardware
  module.
- The BS fails immediately: check `journalctl -u nexus-bs@USER.service -n 160
  --no-pager` and validate `/etc/nexus-bs/config.toml`.
- Dashboard loads but API calls fail: confirm `nexus-bs@USER.service` is active
  and the core API is listening on `127.0.0.1:18080`.
- Control commands fail: confirm `nexus-bs-control@USER.service` is active.
