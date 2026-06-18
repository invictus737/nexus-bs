<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Instalare ușoară: pachet release (.deb)

Limbi: [English](Install-from-APT) | **Română** | [Deutsch](Install-from-APT-de) | [Español](Install-from-APT-es)

Folosește asta dacă targetul afișează `arm64`:

```sh
dpkg --print-architecture
```

## Copiază asta pentru instalare

Când se deschide editorul de config, salvează și închide; comanda următoare
pornește Nexus-BS.

```sh
sudo apt update
sudo apt install -y curl ca-certificates
cd /tmp
curl -fL -o nexus-bs_0.1.71_arm64.deb https://github.com/invictus737/nexus-bs/releases/download/v0.1.71/nexus-bs_0.1.71_arm64.deb
curl -fL -o SHA256SUMS https://github.com/invictus737/nexus-bs/releases/download/v0.1.71/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
sudo apt install /tmp/nexus-bs_0.1.71_arm64.deb
sudo chown "$USER:$USER" /etc/nexus-bs/config.toml /etc/nexus-bs/config.toml.fallback
chmod 600 /etc/nexus-bs/config.toml /etc/nexus-bs/config.toml.fallback
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

Descarcă pachetul `.deb` mai nou, apoi:

```sh
sudo apt install /tmp/nexus-bs_NEW_VERSION_arm64.deb
nexus-bs-service restart
```

Update-ul pachetului nu înlocuiește:

```text
/etc/nexus-bs/config.toml
/etc/nexus-bs/config.toml.fallback
```
