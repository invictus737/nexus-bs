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

The package installs binaries and dashboard assets under `/opt/nexus-bs`,
systemd service templates under `/lib/systemd/system`, and config examples
under `/etc/nexus-bs/examples`. On first install, `postinst` creates
`/etc/nexus-bs/config.toml` and `/etc/nexus-bs/config.toml.fallback` from those
examples only if they do not already exist.

Package upgrades, `apt remove`, and `apt purge` do not overwrite or delete the
live `/etc/nexus-bs/config.toml` files because they are not owned by the Debian
package. Review the global live config before starting services:

```sh
sudoedit /etc/nexus-bs/config.toml
sudo systemctl enable --now nexus-bs-control@chris.service
sudo systemctl enable --now nexus-bs@chris.service
sudo systemctl enable --now nexus-bs-dashboard@chris.service
```
