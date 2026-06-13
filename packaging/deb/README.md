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
`nexus-bs-service` on `PATH`, systemd service templates under
`/lib/systemd/system`, and config examples under `/etc/nexus-bs/examples`. On
first install, `postinst` creates `/etc/nexus-bs/config.toml` and
`/etc/nexus-bs/config.toml.fallback` from those examples only if they do not
already exist.

Package upgrades, `apt remove`, and `apt purge` intentionally leave the live
`/etc/nexus-bs/config.toml` files in place. Review the global live config before
starting services:

```sh
nexus-bs-service edit-config
nexus-bs-service start
```
