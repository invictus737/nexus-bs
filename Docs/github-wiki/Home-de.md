<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Nexus-BS Wiki

## Sprache wählen

| Sprache | Start | Einfache Installation | Quelleninstallation |
|---|---|---|---|
| English | [Start](Home) | [Easy `.deb`](Install-from-APT) | [Build from source](Build-from-Source) |
| Română | [Start](Home-ro) | [Instalare `.deb`](Install-from-APT-ro) | [Build din surse](Build-from-Source-ro) |
| Deutsch | **Start** | [`.deb` Installation](Install-from-APT-de) | [Aus Quellen bauen](Build-from-Source-de) |
| Español | [Inicio](Home-es) | [Instalación `.deb`](Install-from-APT-es) | [Compilar desde fuente](Build-from-Source-es) |

## Hier starten: einfache Installation (.deb)

Nutze diese Methode auf einem `arm64` Debian/Ubuntu/Raspberry-Pi-System.

Zuerst prüfen:

```sh
dpkg --print-architecture
```

Wenn `arm64` angezeigt wird, kopiere dies:

Wenn der Konfigurationseditor geöffnet wird, speichern und schließen; der
nächste Befehl startet Nexus-BS.

```sh
sudo apt update
sudo apt install -y curl ca-certificates
cd ~
curl -fLO https://github.com/invictus737/nexus-bs/releases/download/v0.1.66/nexus-bs_0.1.66_arm64.deb
curl -fLO https://github.com/invictus737/nexus-bs/releases/download/v0.1.66/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
sudo apt install ./nexus-bs_0.1.66_arm64.deb
sudo chown "$USER:$USER" /etc/nexus-bs/config.toml /etc/nexus-bs/config.toml.fallback
chmod 600 /etc/nexus-bs/config.toml /etc/nexus-bs/config.toml.fallback
nexus-bs-service edit-config
nexus-bs-service start
```

Vollständige Seite: [Einfache Installation (.deb)](Install-from-APT-de)

## Fortgeschritten: aus Quellen bauen

Nutze dies nur, wenn du Nexus-BS selbst kompilieren willst oder das Zielsystem
nicht `arm64` ist.

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

Vollständige Seite: [Aus Quellen bauen](Build-from-Source-de)

## Eine Service-Kommandoschnittstelle

```sh
nexus-bs-service start
nexus-bs-service status
nexus-bs-service logs
nexus-bs-service restart
```

Die Live-Konfiguration liegt hier:

```text
/etc/nexus-bs/config.toml
```

Vor dem Senden über RF ändere nur Einstellungen, die du kennst: legale TX/RX
Frequenzen, SDR-Gerät, Antenne/Gain, MCC/MNC, lokale Gruppen,
Dashboard-Passwort und Brew/TetraPack-Zugangsdaten, falls vorhanden.
