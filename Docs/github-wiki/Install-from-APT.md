<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Easy Install: Release Package (.deb)

Languages: **English** | [Română](Install-from-APT-ro) | [Deutsch](Install-from-APT-de) | [Español](Install-from-APT-es)

Use this if your target prints `arm64`:

```sh
dpkg --print-architecture
```

## Copy This To Install

When the config editor opens, save and close it; the next command starts
Nexus-BS.

```sh
sudo apt update
sudo apt install -y curl ca-certificates
cd /tmp
curl -fL -o nexus-bs_0.1.71_arm64.deb https://github.com/invictus737/nexus-bs/releases/download/v0.1.71/nexus-bs_0.1.71_arm64.deb
curl -fL -o SHA256SUMS https://github.com/invictus737/nexus-bs/releases/download/v0.1.71/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
sudo apt install /tmp/nexus-bs_0.1.71_arm64.deb
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

Download the newer `.deb`, then:

```sh
sudo apt install /tmp/nexus-bs_NEW_VERSION_arm64.deb
nexus-bs-service restart
```

Package updates do not replace:

```text
/etc/nexus-bs/config.toml
/etc/nexus-bs/config.toml.fallback
```
