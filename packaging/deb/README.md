<!--
SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
-->

# Nexus-BS Debian Package Builder

Build a Debian binary package from the repo's existing `compiled_distribution`
bundle:

```sh
packaging/deb/build-deb.sh
```

The output is written to `packaging/deb/dist/nexus-bs_VERSION_ARCH.deb`.

Defaults:

- `VERSION`: parsed from the root `Cargo.toml` workspace package version.
- `ARCH`: detected from `compiled_distribution/bin/nexus-bs` when possible.
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

The package installs binaries and the dashboard under `/opt/nexus-bs`,
`nexus-bs-service` on `PATH`, systemd service units under
`/lib/systemd/system`, and config examples under `/etc/nexus-bs/examples`. On
first install, `postinst` creates `/etc/nexus-bs/config.toml` from the example
only if the live file is missing. The fallback example is shipped under
`/etc/nexus-bs/examples` but is not created as a live config unless an operator
chooses to install it.

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

## Clean Reinstall / Recovery

Use this when a target has stale manual systemd units or a broken package
state. It removes old units from `/etc/systemd/system`, `/lib/systemd/system`,
and `/usr/lib/systemd/system`, reinstalls the `.deb`, restores the live config,
then prints service status and logs:

```sh
cd /tmp
curl -fL -o nexus-bs_0.1.72_arm64.deb https://github.com/invictus737/nexus-bs/releases/download/v0.1.72/nexus-bs_0.1.72_arm64.deb
curl -fL -o nexus-bs-reinstall.sh https://github.com/invictus737/nexus-bs/raw/main/scripts/tetrahs-reinstall-nexus-bs.sh
sudo bash /tmp/nexus-bs-reinstall.sh
```

Expected live config permissions after install:

```text
/etc/nexus-bs             root:root 0755
/etc/nexus-bs/config.toml root:root 0600
```
