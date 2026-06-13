<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Fortgeschritten: aus Quellen bauen

Sprachen: [English](Build-from-Source) | [Română](Build-from-Source-ro) | **Deutsch** | [Español](Build-from-Source-es)

Nutze dies, wenn du Nexus-BS selbst kompilieren willst oder das Release-`.deb`
nicht zu deinem Zielsystem passt.

Der Installer installiert fehlende Debian-Buildpakete, installiert Rust falls
nötig, baut Nexus-BS, erstellt die Konfiguration bei der ersten Installation
und installiert ein einziges Service-Kommando:

```sh
nexus-bs-service start
```

## Zum Installieren kopieren

Wenn der Konfigurationseditor geöffnet wird, speichern und schließen; der
nächste Befehl startet Nexus-BS.

```sh
sudo apt update
sudo apt install -y git curl ca-certificates
git clone https://github.com/invictus737/nexus-bs.git ~/nexus-bs-source
cd ~/nexus-bs-source
./scripts/install-from-source.sh
nexus-bs-service edit-config
nexus-bs-service start
```

Die Live-Konfiguration ist:

```text
/etc/nexus-bs/config.toml
```

## Was zu bearbeiten ist

Vor dem RF-Start ändere nur Einstellungen, die du kennst:

- legale TX/RX Frequenzen;
- SDR-Gerät;
- Antenne und Gain-Einstellungen;
- MCC/MNC und lokale Gruppen-IDs;
- Dashboard-Passwort;
- Brew/TetraPack-Zugangsdaten nur, wenn du gültige Zugangsdaten hast.

## Nützliche Service-Kommandos

```sh
nexus-bs-service status
nexus-bs-service logs
nexus-bs-service restart
```

Dashboard:

```text
http://<target-ip>:8080
```

## Später aktualisieren

```sh
cd ~/nexus-bs-source
git pull --ff-only
./scripts/install-from-source.sh
nexus-bs-service restart
```

Der Installer behält vorhandene Konfigurationsdateien:

```text
/etc/nexus-bs/config.toml
/etc/nexus-bs/config.toml.fallback
```
