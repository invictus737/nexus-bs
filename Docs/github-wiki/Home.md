<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Nexus-BS Wiki

This wiki is written for a normal operator install. The default path is the
classic source install:

```sh
git pull
cargo build --release --locked
sudo install ...
sudoedit /etc/nexus-bs/config.toml
sudo systemctl start nexus-bs@USER.service
```

## Start Here

- [Classic Source Install](Build-from-Source) - build from the repository,
  install the binaries, install the systemd service files, edit `config.toml`,
  and start the base station.
- [Optional .deb Install](Install-from-APT) - install the prebuilt GitHub
  Release package when you want package-manager ownership of `/opt/nexus-bs`.

## Runtime Layout

Classic source installs use:

```text
/home/<run-user>/nexus-bs/       binaries, dashboard assets, helper script
/etc/nexus-bs/config.toml        live operator configuration
/etc/systemd/system/             service units copied from contrib/systemd
```

The shipped systemd templates expect the runtime folder under
`/home/<run-user>/nexus-bs`. The install page keeps the live config in
`/etc/nexus-bs/config.toml` and links it into the runtime folder.

Before transmitting, verify RF authority, frequency plan, SDR hardware,
antenna/gain settings, MCC/MNC, carrier plan, dashboard access, and any Brew or
external-service credentials.
