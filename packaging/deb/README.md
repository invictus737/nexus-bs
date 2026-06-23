<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Nexus-BS Debian Package Builder

Build a Debian binary package from a temporary package payload:

```sh
packaging/deb/build-deb.sh
```

The output is written to `packaging/deb/dist/nexus-bs_VERSION_ARCH.deb`.
For public tag releases, do not call this builder directly. Use the release
artifact script so the ARM64 binaries are rebuilt, copied into a temporary
payload directory, the tag is checked against the Cargo workspace version, and
the repo is not dirtied by generated binaries:

```sh
scripts/build-release-artifacts.sh
```

Defaults:

- `VERSION`: parsed from the root `Cargo.toml` workspace package version.
- `ARCH`: detected from the temporary payload binary when possible.
- `MAINTAINER`: `Nexus-BS Project <noreply@nexus-bs.local>`.

Overrides:

```sh
VERSION=0.1.65 ARCH=arm64 MAINTAINER="Name <name@example.com>" packaging/deb/build-deb.sh
```

Other optional overrides:

- `DEPENDS`: Debian control dependency line.
- `OUT_DIR`: package output directory.
- `WORK_DIR`: temporary staging directory.
- `KEEP_BUILD_DIR=1`: keep the staging tree after a build.

The builder rejects `_dev`, `-modified`, and wrong-version binaries by default.
This prevents publishing a `.deb` whose metadata says one version while the
generated binary payload contains another. Local field tests can explicitly set
`ALLOW_NONRELEASE_BINARIES=1`, but the binary base version must still match
`VERSION`.

The package installs binaries and the dashboard under `/opt/nexus-bs`,
`nexus-bs-service` on `PATH`, systemd service units under
`/lib/systemd/system`, and config examples under `/etc/nexus-bs/examples`. On
first install, `postinst` creates `/etc/nexus-bs/config.toml` from the example
only if the live file is missing. It detects the local operator user, makes
`/etc/nexus-bs` writable by that user, and installs small systemd drop-ins so
the core, control, and dashboard services run as the same user. The fallback
example is shipped under `/etc/nexus-bs/examples` but is not created as a live
config unless an operator chooses to install it.

Package upgrades, `apt remove`, and `apt purge` intentionally leave the live
`/etc/nexus-bs/config.toml` file in place. Review the global live config before
starting services:

```sh
nexus-bs-service edit-config
nexus-bs-service start
```

## Update

Install the newer `.deb` over the existing package. The live config is kept:

```sh
sudo apt install /tmp/nexus-bs_NEW_VERSION_arm64.deb
nexus-bs-service restart
```

Dashboard-driven package update/downgrade must follow the same clean-service
contract as a manual install:

1. Stop `nexus-bs.service` and `nexus-bs-control.service` before package
   installation.
2. Install the selected `.deb` with `apt-get install -y`.
3. Run `systemctl daemon-reload`.
4. Run `systemctl reset-failed` for the Nexus-BS services.
5. Wait 5 seconds so RF drops cleanly and terminals re-affiliate.
6. Start `nexus-bs-control.service`.
7. Start `nexus-bs.service`.
8. Restart `nexus-bs-dashboard.service` from the installed package.

Do not finish package updates by calling the dashboard Restart BS HTTP API from
inside the updating dashboard process. That created mixed-version windows where
the dashboard and core used different internal telemetry protocol versions, so
telemetry WebSocket handshakes could fail until a full OS reboot.

Because a dashboard self-update starts from the old dashboard binary, the
package `postinst` also schedules a root-side service realignment when the
dashboard and at least one core/control service were still active during
install. Keep that safety net in the package scripts: it is what lets a newly
installed `.deb` repair an update initiated by an older dashboard without
forcing a full OS reboot.

The dashboard sudoers file is intentionally narrow. It grants package update,
Nexus-BS service lifecycle, host poweroff, factory reset helper, and `nmcli`
for Wi-Fi profile changes. If Wi-Fi connect fails from the dashboard but works
with `sudo nmcli` on the host, inspect `/etc/sudoers.d/nexus-bs-dashboard` and
reinstall the package so `postinst` refreshes it.

## Clean Reinstall / Recovery

Use this when a target has stale manual systemd units or a broken package
state. It removes old units from `/etc/systemd/system`, `/lib/systemd/system`,
and `/usr/lib/systemd/system`, reinstalls the `.deb`, restores the live config,
then prints service status and logs:

```sh
cd /tmp
curl -fL -o nexus-bs_0.1.78_arm64.deb https://github.com/invictus737/nexus-bs/releases/download/v0.1.78/nexus-bs_0.1.78_arm64.deb
curl -fL -o nexus-bs-reinstall.sh https://github.com/invictus737/nexus-bs/raw/v0.1.78/scripts/tetrahs-reinstall-nexus-bs.sh
sudo env DEB_PATH=/tmp/nexus-bs_0.1.78_arm64.deb bash /tmp/nexus-bs-reinstall.sh
```

Expected live config permissions after install:

```text
/etc/nexus-bs             <user>:<user> 0750
/etc/nexus-bs/config.toml <user>:<user> 0600
```
