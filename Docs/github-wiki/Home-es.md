<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Nexus-BS Wiki

## Elegir idioma

| Idioma | Inicio | Instalación fácil | Instalación desde fuente |
|---|---|---|---|
| English | [Start](Home) | [Easy `.deb`](Install-from-APT) | [Build from source](Build-from-Source) |
| Română | [Start](Home-ro) | [Instalare `.deb`](Install-from-APT-ro) | [Build din surse](Build-from-Source-ro) |
| Deutsch | [Start](Home-de) | [`.deb` Installation](Install-from-APT-de) | [Aus Quellen bauen](Build-from-Source-de) |
| Español | **Inicio** | [Instalación `.deb`](Install-from-APT-es) | [Compilar desde fuente](Build-from-Source-es) |

## Empieza aquí: instalación fácil (.deb)

Usa este método en un sistema Debian/Ubuntu/Raspberry Pi `arm64`.

Comprueba primero:

```sh
dpkg --print-architecture
```

Si muestra `arm64`, copia esto:

Cuando se abra el editor de configuración, guarda y ciérralo; el siguiente
comando inicia Nexus-BS.

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

Página completa: [Instalación fácil (.deb)](Install-from-APT-es)

## Avanzado: compilar desde fuente

Usa esto solo si quieres compilar Nexus-BS tú mismo o si el sistema no es
`arm64`.

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

Página completa: [Compilar desde fuente](Build-from-Source-es)

## Un solo comando después de instalar

```sh
nexus-bs-service start
nexus-bs-service status
nexus-bs-service logs
nexus-bs-service restart
```

La configuración activa está aquí:

```text
/etc/nexus-bs/config.toml
```

Antes de transmitir RF, cambia solo los ajustes que conoces: frecuencias legales
TX/RX, dispositivo SDR, antena/ganancias, MCC/MNC, grupos locales, contraseña
del dashboard y credenciales Brew/TetraPack si las tienes.
