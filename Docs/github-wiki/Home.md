<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Nexus-BS Wiki

## Choose Language

| Language | Start | Easy install | Source install |
|---|---|---|---|
| English | **Start** | [Easy `.deb`](Install-from-APT) | [Build from source](Build-from-Source) |
| Română | [Start](Home-ro) | [Instalare `.deb`](Install-from-APT-ro) | [Build din surse](Build-from-Source-ro) |
| Deutsch | [Start](Home-de) | [`.deb` Installation](Install-from-APT-de) | [Aus Quellen bauen](Build-from-Source-de) |
| Español | [Inicio](Home-es) | [Instalación `.deb`](Install-from-APT-es) | [Compilar desde fuente](Build-from-Source-es) |

## Start Here: Easy Install (.deb)

Use this on an `arm64` Debian/Ubuntu/Raspberry Pi style target.

Check first:

```sh
dpkg --print-architecture
```

If it prints `arm64`, copy this:

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

Full page: [Easy Install (.deb)](Install-from-APT)

## Advanced: Build From Source

Use this only when you want to compile Nexus-BS yourself or your target is not
`arm64`.

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

Full page: [Build From Source](Build-from-Source)

## One Command After Install

```sh
nexus-bs-service start
nexus-bs-service status
nexus-bs-service logs
nexus-bs-service restart
```

Config path:

```text
/etc/nexus-bs/config.toml
```

Before transmitting, edit only the settings you know: legal TX/RX frequencies,
SDR device, antenna/gains, MCC/MNC, local groups, dashboard password, and
Brew/TetraPack credentials if you have them.
