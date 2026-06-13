<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Nexus-BS Wiki

## Alege limba

| Limbă | Start | Instalare ușoară | Instalare din surse |
|---|---|---|---|
| English | [Start](Home) | [Easy `.deb`](Install-from-APT) | [Build from source](Build-from-Source) |
| Română | **Start** | [Instalare `.deb`](Install-from-APT-ro) | [Build din surse](Build-from-Source-ro) |
| Deutsch | [Start](Home-de) | [`.deb` Installation](Install-from-APT-de) | [Aus Quellen bauen](Build-from-Source-de) |
| Español | [Inicio](Home-es) | [Instalación `.deb`](Install-from-APT-es) | [Compilar desde fuente](Build-from-Source-es) |

## Începe aici: instalare ușoară (.deb)

Folosește metoda asta pe un sistem Debian/Ubuntu/Raspberry Pi `arm64`.

Verifică întâi:

```sh
dpkg --print-architecture
```

Dacă afișează `arm64`, copiază asta:

Când se deschide editorul de config, salvează și închide; comanda următoare
pornește Nexus-BS.

```sh
sudo apt update
sudo apt install -y curl ca-certificates
cd ~
curl -fLO https://github.com/invictus737/nexus-bs/releases/download/v0.1.65/nexus-bs_0.1.65_arm64.deb
curl -fLO https://github.com/invictus737/nexus-bs/releases/download/v0.1.65/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
sudo apt install ./nexus-bs_0.1.65_arm64.deb
sudo chown "$USER:$USER" /etc/nexus-bs/config.toml /etc/nexus-bs/config.toml.fallback
chmod 600 /etc/nexus-bs/config.toml /etc/nexus-bs/config.toml.fallback
nexus-bs-service edit-config
nexus-bs-service start
```

Pagina completă: [Instalare ușoară (.deb)](Install-from-APT-ro)

## Avansat: build din surse

Folosește asta doar dacă vrei să compilezi Nexus-BS sau dacă targetul nu este
`arm64`.

Când se deschide editorul de config, salvează și închide; comanda următoare
pornește Nexus-BS.

```sh
sudo apt update
sudo apt install -y git curl ca-certificates
git clone https://github.com/invictus737/nexus-bs.git ~/nexus-bs-source
cd ~/nexus-bs-source
./scripts/install-from-source.sh
nexus-bs-service edit-config
nexus-bs-service start
```

Pagina completă: [Build din surse](Build-from-Source-ro)

## O singură comandă după instalare

```sh
nexus-bs-service start
nexus-bs-service status
nexus-bs-service logs
nexus-bs-service restart
```

Config-ul live este aici:

```text
/etc/nexus-bs/config.toml
```

Înainte să transmiți RF, modifică doar setările pe care le știi: frecvențele
legale TX/RX, SDR-ul, antena/gain-urile, MCC/MNC, grupurile locale, parola de
dashboard și credențialele Brew/TetraPack dacă le ai.
