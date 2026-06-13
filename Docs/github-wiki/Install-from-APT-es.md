<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Instalación fácil: paquete release (.deb)

Idiomas: [English](Install-from-APT) | [Română](Install-from-APT-ro) | [Deutsch](Install-from-APT-de) | **Español**

Usa esto si tu sistema muestra `arm64`:

```sh
dpkg --print-architecture
```

## Copia esto para instalar

Cuando se abra el editor de configuración, guarda y ciérralo; el siguiente
comando inicia Nexus-BS.

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

La configuración activa es:

```text
/etc/nexus-bs/config.toml
```

## Qué editar

Antes de iniciar RF, cambia solo los ajustes que conoces:

- frecuencias legales TX/RX;
- dispositivo SDR;
- antena y ganancias;
- MCC/MNC e IDs de grupos locales;
- contraseña del dashboard;
- credenciales Brew/TetraPack solo si ya tienes credenciales válidas.

## Comandos útiles

```sh
nexus-bs-service status
nexus-bs-service logs
nexus-bs-service restart
```

Dashboard:

```text
http://<target-ip>:8080
```

## Actualizar después

Descarga el `.deb` nuevo, luego:

```sh
sudo apt install ./nexus-bs_NEW_VERSION_arm64.deb
nexus-bs-service restart
```

Las actualizaciones del paquete no reemplazan:

```text
/etc/nexus-bs/config.toml
/etc/nexus-bs/config.toml.fallback
```
