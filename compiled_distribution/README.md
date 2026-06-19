<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Nexus-BS Compiled Distribution

Minimal Linux/aarch64 binary bundle for Nexus-BS `v0.1.72`.

This bundle is for manual installs. Beginners should normally use the GitHub
Release `.deb` package instead.

## Contents

```text
bin/                         binaries
config/                      example config
dashboard/                   dashboard files
scripts/nexus-bs-service     one command for start/status/logs/restart
systemd/                     service unit files
```

## Copy This To Install

Run this from inside `compiled_distribution/` on the target Linux machine.

When the config editor opens, save and close it; the next command starts
Nexus-BS.

```sh
sudo systemctl stop nexus-bs-dashboard.service nexus-bs.service nexus-bs-control.service 2>/dev/null || true

sudo install -d -o root -g root -m 0755 /opt/nexus-bs/bin
sudo install -d -o root -g root -m 0755 /opt/nexus-bs/dashboard
sudo install -d -o root -g root -m 0755 /etc/nexus-bs

sudo install -o root -g root -m 0755 bin/nexus-bs /opt/nexus-bs/bin/nexus-bs
sudo install -o root -g root -m 0755 bin/nexus-bs-control-service /opt/nexus-bs/bin/nexus-bs-control-service
sudo install -o root -g root -m 0755 bin/nexus-bs-dashboard /opt/nexus-bs/bin/nexus-bs-dashboard
sudo install -o root -g root -m 0755 scripts/nexus-bs-service /opt/nexus-bs/bin/nexus-bs-service

sudo cp -R dashboard/. /opt/nexus-bs/dashboard/
sudo chown -R root:root /opt/nexus-bs/dashboard

sudo install -d -m 0755 /etc/systemd/system /usr/local/bin
sudo install -m 0644 systemd/nexus-bs-control.service /etc/systemd/system/nexus-bs-control.service
sudo install -m 0644 systemd/nexus-bs.service /etc/systemd/system/nexus-bs.service
sudo install -m 0644 systemd/nexus-bs-dashboard.service /etc/systemd/system/nexus-bs-dashboard.service
sudo ln -sfn /opt/nexus-bs/bin/nexus-bs-service /usr/local/bin/nexus-bs-service

if [ ! -e /etc/nexus-bs/config.toml ]; then
  sudo install -o root -g root -m 0600 config/config.toml /etc/nexus-bs/config.toml
fi

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
sudo install -o root -g root -m 0755 bin/nexus-bs /opt/nexus-bs/bin/nexus-bs
sudo install -o root -g root -m 0755 bin/nexus-bs-control-service /opt/nexus-bs/bin/nexus-bs-control-service
sudo install -o root -g root -m 0755 bin/nexus-bs-dashboard /opt/nexus-bs/bin/nexus-bs-dashboard
sudo install -o root -g root -m 0755 scripts/nexus-bs-service /opt/nexus-bs/bin/nexus-bs-service

sudo install -d -o root -g root -m 0755 /opt/nexus-bs/dashboard
sudo cp -R dashboard/. /opt/nexus-bs/dashboard/
sudo chown -R root:root /opt/nexus-bs/dashboard

sudo install -m 0644 systemd/nexus-bs-control.service /etc/systemd/system/nexus-bs-control.service
sudo install -m 0644 systemd/nexus-bs.service /etc/systemd/system/nexus-bs.service
sudo install -m 0644 systemd/nexus-bs-dashboard.service /etc/systemd/system/nexus-bs-dashboard.service
sudo systemctl daemon-reload
nexus-bs-service restart
```

## SHA-256 Checksums

Run checksum verification from inside `compiled_distribution/`. The README file
itself is not listed because embedding its checksum would change the checksum.

```text
6aabbea37d31fe0639817aaa9730e8be500ce40a963ce9144f9ea8e815aabae0  bin/nexus-bs
65fa00a957797f60c3bdb7de74f557a9a90bde259265ad871eafa7a0af575ed1  bin/nexus-bs-control-service
77ed66ddba373a74328da69ad064a131d96f2f0a0814447621c87fa6543006fd  bin/nexus-bs-dashboard
5fc10d095d2fd5434b8cec365867f71a6fa4895e91d98b709d25b30f91a6c0be  config/config.toml
5fc10d095d2fd5434b8cec365867f71a6fa4895e91d98b709d25b30f91a6c0be  config/config.toml.fallback
6c14628cd23c898b98593568d446a7fe19d010a2fbf71c8d518d3cfeeb9e60ea  dashboard/assets/app.js
4a8db4661e48b6555912e1e6114ed2205ef5c9ffbeed997464a3d3dd7a5e06d0  dashboard/assets/nexus-bs-logo.png
92d6127820d318a5df91bc60c60297e3b74068ff322e13c5b651fdee4075e9a5  dashboard/assets/nexus-bs-logo.svg
dfe0e36649e3d28d23199804ddb9292be55839c8ff55d50b5c0f4f57e6d0288d  dashboard/assets/styles.css
4e8710052ea4d7edbdb971225d0dea06157c057b798a7428f83f67f2dffaa667  dashboard/index.html
b5b55c0ce1919cfa53daa2aa4b87105a0584414f21c7f0b6ce58ff48f37fd84b  scripts/nexus-bs-service
5416b165bd62b12fb1b655dcc05c0f6aaa54c0f92c39d0bb4fcf937439077c16  systemd/journald-nexus-bs-volatile.conf
e6420f28322ad1abd2b86e92268c21fdd74651abbf37ed6f0bbbbef350e6b533  systemd/nexus-bs-control.service
c433ed4149eac3a2016b4a98c2e80b892809e1e12b7b215868eec404da2d9181  systemd/nexus-bs-dashboard.service
7aebeb05241aa90bb2a744f1ae068136a69f0f937a4b76f9fe0ae1efa2a65163  systemd/nexus-bs.service
```
