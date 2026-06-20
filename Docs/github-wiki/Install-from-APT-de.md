<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Einfache Installation: Release-Paket (.deb)

Sprachen: [English](Install-from-APT) | [Română](Install-from-APT-ro) | **Deutsch** | [Español](Install-from-APT-es)

Nutze dies, wenn dein Zielsystem `arm64` ausgibt:

```sh
dpkg --print-architecture
```

## Zum Installieren kopieren

Wenn der Konfigurationseditor geöffnet wird, speichern und schließen; der
nächste Befehl startet Nexus-BS.

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

Lade das neuere `.deb` herunter, dann:

```sh
sudo apt install /tmp/nexus-bs_NEW_VERSION_arm64.deb
nexus-bs-service restart
```

Paketupdates ersetzen diese Dateien nicht:

```text
/etc/nexus-bs/config.toml
/etc/nexus-bs/config.toml.fallback
```
