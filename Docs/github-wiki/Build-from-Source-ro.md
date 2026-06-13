<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Avansat: build din surse

Limbi: [English](Build-from-Source) | **Română** | [Deutsch](Build-from-Source-de) | [Español](Build-from-Source-es)

Folosește asta când vrei să compilezi Nexus-BS sau când pachetul release `.deb`
nu se potrivește cu targetul tău.

Installerul instalează pachetele Debian lipsă, instalează Rust dacă este nevoie,
compilează Nexus-BS, creează config-ul la prima instalare și instalează o
singură comandă de service:

```sh
nexus-bs-service start
```

## Copiază asta pentru instalare

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

Config-ul live este:

```text
/etc/nexus-bs/config.toml
```

## Ce trebuie editat

Înainte să pornești RF, modifică doar setările pe care le știi:

- frecvențele legale TX/RX;
- dispozitivul SDR;
- antena și gain-urile;
- MCC/MNC și grupurile locale;
- parola dashboard-ului;
- credențiale Brew/TetraPack doar dacă ai deja credențiale valide.

## Comenzi utile

```sh
nexus-bs-service status
nexus-bs-service logs
nexus-bs-service restart
```

Dashboard:

```text
http://<target-ip>:8080
```

## Update mai târziu

```sh
cd ~/nexus-bs-source
git pull --ff-only
./scripts/install-from-source.sh
nexus-bs-service restart
```

Installerul păstrează config-urile existente:

```text
/etc/nexus-bs/config.toml
/etc/nexus-bs/config.toml.fallback
```
