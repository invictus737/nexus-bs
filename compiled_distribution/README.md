<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Nexus-BS Compiled Distribution

Minimal Linux/aarch64 binary bundle for Nexus-BS `v0.1.66-fb39e15f`.

This bundle is for manual installs. Beginners should normally use the GitHub
Release `.deb` package instead.

## Contents

```text
bin/                         binaries
config/                      example config and fallback
dashboard/                   dashboard files
scripts/nexus-bs-service     one command for start/status/logs/restart
systemd/                     service unit templates
```

## Copy This To Install

Run this from inside `compiled_distribution/` on the target Linux machine.

When the config editor opens, save and close it; the next command starts
Nexus-BS.

```sh
NEXUS_USER="$(id -un)"
NEXUS_GROUP="$(id -gn)"

sudo install -d -o "$NEXUS_USER" -g "$NEXUS_GROUP" -m 0755 /home/"$NEXUS_USER"/nexus-bs
sudo install -d -o "$NEXUS_USER" -g "$NEXUS_GROUP" -m 0755 /etc/nexus-bs

sudo install -o "$NEXUS_USER" -g "$NEXUS_GROUP" -m 0755 bin/nexus-bs /home/"$NEXUS_USER"/nexus-bs/nexus-bs
sudo install -o "$NEXUS_USER" -g "$NEXUS_GROUP" -m 0755 bin/nexus-bs-control-service /home/"$NEXUS_USER"/nexus-bs/nexus-bs-control-service
sudo install -o "$NEXUS_USER" -g "$NEXUS_GROUP" -m 0755 bin/nexus-bs-dashboard /home/"$NEXUS_USER"/nexus-bs/nexus-bs-dashboard
sudo install -o "$NEXUS_USER" -g "$NEXUS_GROUP" -m 0755 scripts/nexus-bs-service /home/"$NEXUS_USER"/nexus-bs/nexus-bs-service

sudo install -d -o "$NEXUS_USER" -g "$NEXUS_GROUP" -m 0755 /home/"$NEXUS_USER"/nexus-bs/dashboard
sudo cp -R dashboard/. /home/"$NEXUS_USER"/nexus-bs/dashboard/
sudo chown -R "$NEXUS_USER:$NEXUS_GROUP" /home/"$NEXUS_USER"/nexus-bs/dashboard

sudo install -d -m 0755 /etc/systemd/system /usr/local/bin
sudo install -m 0644 systemd/nexus-bs-control@.service /etc/systemd/system/nexus-bs-control@.service
sudo install -m 0644 systemd/nexus-bs@.service /etc/systemd/system/nexus-bs@.service
sudo install -m 0644 systemd/nexus-bs-dashboard@.service /etc/systemd/system/nexus-bs-dashboard@.service
sudo ln -sfn /home/"$NEXUS_USER"/nexus-bs/nexus-bs-service /usr/local/bin/nexus-bs-service

if [ ! -e /etc/nexus-bs/config.toml ]; then
  sudo install -o "$NEXUS_USER" -g "$NEXUS_GROUP" -m 0600 config/config.toml /etc/nexus-bs/config.toml
fi
if [ ! -e /etc/nexus-bs/config.toml.fallback ]; then
  sudo install -o "$NEXUS_USER" -g "$NEXUS_GROUP" -m 0600 /etc/nexus-bs/config.toml /etc/nexus-bs/config.toml.fallback
fi

ln -sfn /etc/nexus-bs/config.toml /home/"$NEXUS_USER"/nexus-bs/config.toml
ln -sfn /etc/nexus-bs/config.toml.fallback /home/"$NEXUS_USER"/nexus-bs/config.toml.fallback

sudo systemctl daemon-reload
nexus-bs-service edit-config
nexus-bs-service start
```

## Useful Commands

```sh
nexus-bs-service status
nexus-bs-service logs
nexus-bs-service restart
```

Dashboard:

```text
http://<target-ip>:8080
```

Live config:

```text
/etc/nexus-bs/config.toml
```

Before transmitting, edit only the settings you know: legal TX/RX frequencies,
SDR device, antenna/gains, MCC/MNC, local groups, dashboard password, and
Brew/TetraPack credentials if you have them.

## Update Later

Run this from inside the newer `compiled_distribution/`. It does not replace
`/etc/nexus-bs/config.toml`.

```sh
NEXUS_USER="$(id -un)"
NEXUS_GROUP="$(id -gn)"

sudo install -o "$NEXUS_USER" -g "$NEXUS_GROUP" -m 0755 bin/nexus-bs /home/"$NEXUS_USER"/nexus-bs/nexus-bs
sudo install -o "$NEXUS_USER" -g "$NEXUS_GROUP" -m 0755 bin/nexus-bs-control-service /home/"$NEXUS_USER"/nexus-bs/nexus-bs-control-service
sudo install -o "$NEXUS_USER" -g "$NEXUS_GROUP" -m 0755 bin/nexus-bs-dashboard /home/"$NEXUS_USER"/nexus-bs/nexus-bs-dashboard
sudo install -o "$NEXUS_USER" -g "$NEXUS_GROUP" -m 0755 scripts/nexus-bs-service /home/"$NEXUS_USER"/nexus-bs/nexus-bs-service

sudo install -d -o "$NEXUS_USER" -g "$NEXUS_GROUP" -m 0755 /home/"$NEXUS_USER"/nexus-bs/dashboard
sudo cp -R dashboard/. /home/"$NEXUS_USER"/nexus-bs/dashboard/
sudo chown -R "$NEXUS_USER:$NEXUS_GROUP" /home/"$NEXUS_USER"/nexus-bs/dashboard

sudo install -m 0644 systemd/nexus-bs-control@.service /etc/systemd/system/nexus-bs-control@.service
sudo install -m 0644 systemd/nexus-bs@.service /etc/systemd/system/nexus-bs@.service
sudo install -m 0644 systemd/nexus-bs-dashboard@.service /etc/systemd/system/nexus-bs-dashboard@.service
sudo systemctl daemon-reload
nexus-bs-service restart
```

## SHA-256 Checksums

Run checksum verification from inside `compiled_distribution/`. The README file
itself is not listed because embedding its checksum would change the checksum.

```text
ee49139735f8652834b72667fdce02499a74c165ad002a392033477a7f82b263  bin/nexus-bs
72d01f6fbe919854f776b5204daf85e4e109015f84596f4456bf8ece9d69457d  bin/nexus-bs-control-service
eee3eece1a9e131e3c5ac2e636850ee939740961102156968f594496f83614b5  bin/nexus-bs-dashboard
15fe8d5ed3c66849775db7f607ea988ebdd8daafada5d658b86b3dced2428f6e  config/config.toml
b74a37bc3af8083482f26ceb170b0974668fb78131f40e60098f152e5a55643c  config/config.toml.fallback
6c14628cd23c898b98593568d446a7fe19d010a2fbf71c8d518d3cfeeb9e60ea  dashboard/assets/app.js
4a8db4661e48b6555912e1e6114ed2205ef5c9ffbeed997464a3d3dd7a5e06d0  dashboard/assets/nexus-bs-logo.png
92d6127820d318a5df91bc60c60297e3b74068ff322e13c5b651fdee4075e9a5  dashboard/assets/nexus-bs-logo.svg
dfe0e36649e3d28d23199804ddb9292be55839c8ff55d50b5c0f4f57e6d0288d  dashboard/assets/styles.css
4e8710052ea4d7edbdb971225d0dea06157c057b798a7428f83f67f2dffaa667  dashboard/index.html
d94ba7342579ab9b874a53069dabc8f678d252f71de72f1671d36a90ff24cca7  scripts/nexus-bs-service
5416b165bd62b12fb1b655dcc05c0f6aaa54c0f92c39d0bb4fcf937439077c16  systemd/journald-nexus-bs-volatile.conf
36e1b2fe4c3d302b7f21d614d74d7921f9dd4cdb920d71f3965f16a9e1d26707  systemd/nexus-bs-control@.service
50496b0e3bddb6a69908b3917912914b786a8c3c3100b0c1bba9a81cdf4228a8  systemd/nexus-bs-dashboard@.service
d72cc269a28a201c8c83b2dce3432b3574f0dc8fa4ca8620e171d3ff421bb79f  systemd/nexus-bs@.service
```
