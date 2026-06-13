<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Optional .deb Install

The recommended operator path is still [Classic Source Install](Build-from-Source).
Use the `.deb` only when you want a prebuilt `arm64` package from GitHub
Releases.

The package installs binaries under `/opt/nexus-bs`, dashboard assets under
`/opt/nexus-bs/dashboard`, systemd unit templates under `/lib/systemd/system`,
and example config files under `/etc/nexus-bs/examples`.

## Install

```sh
export NEXUS_BS_VERSION=0.1.65

curl -fLO "https://github.com/invictus737/nexus-bs/releases/download/v${NEXUS_BS_VERSION}/nexus-bs_${NEXUS_BS_VERSION}_arm64.deb"
curl -fLO "https://github.com/invictus737/nexus-bs/releases/download/v${NEXUS_BS_VERSION}/SHA256SUMS"
sha256sum -c SHA256SUMS --ignore-missing

sudo apt install "./nexus-bs_${NEXUS_BS_VERSION}_arm64.deb"
```

## Configure

Choose the Linux user that will run the services:

```sh
export NEXUS_USER=nexusbs
```

The package creates `/etc/nexus-bs/config.toml` and
`/etc/nexus-bs/config.toml.fallback` only if they do not already exist.

Edit the live config:

```sh
sudoedit /etc/nexus-bs/config.toml
```

Verify RF frequencies, SDR device, gains, MCC/MNC, carrier plan, dashboard
access and Brew/TetraPack credentials before transmitting.

## Start

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now nexus-bs-control@"$NEXUS_USER".service
sudo systemctl enable --now nexus-bs@"$NEXUS_USER".service
sudo systemctl enable --now nexus-bs-dashboard@"$NEXUS_USER".service
```

Dashboard:

```text
http://<target-ip>:8080
```

Logs:

```sh
journalctl -u nexus-bs@"$NEXUS_USER".service -n 160 --no-pager
```

## Update

```sh
sudo apt update
sudo apt install --only-upgrade nexus-bs
sudo systemctl restart nexus-bs-control@"$NEXUS_USER".service nexus-bs@"$NEXUS_USER".service nexus-bs-dashboard@"$NEXUS_USER".service
```

Package updates do not overwrite `/etc/nexus-bs/config.toml` or
`/etc/nexus-bs/config.toml.fallback`.
