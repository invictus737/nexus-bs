<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Advanced: Build From Source

Languages: **English** | [Română](Build-from-Source-ro) | [Deutsch](Build-from-Source-de) | [Español](Build-from-Source-es)

Use this when you want to compile Nexus-BS yourself or when the release `.deb`
does not match your target.

The installer installs missing Debian build packages, installs Rust if needed,
builds Nexus-BS, creates config on first install, and installs one service
command:

```sh
nexus-bs-service start
```

## Copy This To Install

When the config editor opens, save and close it; the next command starts
Nexus-BS.

```sh
sudo apt update
sudo apt install -y git curl ca-certificates
git clone https://github.com/invictus737/nexus-bs.git ~/nexus-bs-source
cd ~/nexus-bs-source
./scripts/install-from-source.sh
nexus-bs-service edit-config
nexus-bs-service start
```

The live config is:

```text
/etc/nexus-bs/config.toml
```

## What To Edit

Before you start RF, edit only the settings you know:

- legal TX/RX frequencies;
- SDR device;
- antenna and gain settings;
- MCC/MNC and local group IDs;
- dashboard password;
- Brew/TetraPack credentials only if you already have valid credentials.

## Useful Service Commands

```sh
nexus-bs-service status
nexus-bs-service logs
nexus-bs-service restart
```

Dashboard:

```text
http://<target-ip>:8080
```

## Update Later

```sh
cd ~/nexus-bs-source
git pull --ff-only
./scripts/install-from-source.sh
nexus-bs-service restart
```

The installer keeps existing config files:

```text
/etc/nexus-bs/config.toml
/etc/nexus-bs/config.toml.fallback
```
