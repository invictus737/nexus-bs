<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Avanzado: compilar desde fuente

Idiomas: [English](Build-from-Source) | [Română](Build-from-Source-ro) | [Deutsch](Build-from-Source-de) | **Español**

Usa esto si quieres compilar Nexus-BS tú mismo o si el `.deb` release no sirve
para tu sistema.

El instalador instala paquetes Debian faltantes, instala Rust si hace falta,
compila Nexus-BS, crea la configuración en la primera instalación e instala un
solo comando de servicio:

```sh
nexus-bs-service start
```

## Copia esto para instalar

Cuando se abra el editor de configuración, guarda y ciérralo; el siguiente
comando inicia Nexus-BS.

```sh
sudo apt update
sudo apt install -y git curl ca-certificates
git clone https://github.com/invictus737/nexus-bs.git ~/nexus-bs-source
cd ~/nexus-bs-source
./scripts/install-from-source.sh
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

```sh
cd ~/nexus-bs-source
git pull --ff-only
./scripts/install-from-source.sh
nexus-bs-service restart
```

El instalador conserva las configuraciones existentes:

```text
/etc/nexus-bs/config.toml
/etc/nexus-bs/config.toml.fallback
```
