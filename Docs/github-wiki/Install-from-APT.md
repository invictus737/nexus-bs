# Install from APT

This page explains how to package Nexus-BS as a Debian package, publish it in a
static APT repository, and install it on a Debian-family target host.

The package path is built from the repository's existing
`compiled_distribution/` bundle. It does not compile Nexus-BS on the target
host. Build the binaries locally, place them in `compiled_distribution/`, then
build the `.deb`.

The Debian package installs:

- Runtime binaries under `/opt/nexus-bs/bin`.
- Dashboard assets under `/opt/nexus-bs/dashboard`.
- Systemd template units under `/lib/systemd/system`.
- Example configuration under `/etc/nexus-bs/examples`.

It does not enable or start a live base-station instance automatically. Review
the global live configuration generated on first install, then enable the
systemd services.

## Install the Release .deb

For normal users, install the published `.deb` from GitHub Releases. The current
prebuilt package is Linux `arm64` / `aarch64`.

Install download tools:

```sh
sudo apt update
sudo apt install ca-certificates curl
```

Download the package and checksum file:

```sh
export NEXUS_BS_VERSION=0.1.65

curl -fLO "https://github.com/invictus737/nexus-bs/releases/download/v${NEXUS_BS_VERSION}/nexus-bs_${NEXUS_BS_VERSION}_arm64.deb"
curl -fLO "https://github.com/invictus737/nexus-bs/releases/download/v${NEXUS_BS_VERSION}/SHA256SUMS"
```

Verify the package checksum:

```sh
sha256sum -c SHA256SUMS --ignore-missing
```

Install it with `apt` so dependencies are handled by the package manager:

```sh
sudo apt install "./nexus-bs_${NEXUS_BS_VERSION}_arm64.deb"
```

Continue with [Create Global Configuration](#create-global-configuration) and
[Enable Systemd Services](#enable-systemd-services).

## Build the Debian Package

From the repository root, make sure `compiled_distribution/` contains the files
required by `packaging/deb/build-deb.sh`:

```text
compiled_distribution/
  bin/nexus-bs
  bin/nexus-bs-control-service
  bin/nexus-bs-dashboard
  config/config.toml
  config/config.toml.fallback
  dashboard/index.html
  dashboard/assets/app.js
  dashboard/assets/styles.css
  dashboard/assets/nexus-bs-logo.svg
  dashboard/assets/nexus-bs-logo.png
  systemd/nexus-bs@.service
  systemd/nexus-bs-control@.service
  systemd/nexus-bs-dashboard@.service
  systemd/journald-nexus-bs-volatile.conf
```

Install the package build tools on the packaging machine:

```sh
sudo apt update
sudo apt install dpkg-dev file
```

Build the package:

```sh
packaging/deb/build-deb.sh
```

The default output is:

```text
packaging/deb/dist/nexus-bs_VERSION_ARCH.deb
```

The builder reads the default `VERSION` from the workspace `Cargo.toml` and
detects `ARCH` from `compiled_distribution/bin/nexus-bs` when possible. Override
metadata when needed:

```sh
VERSION=0.1.65 \
ARCH=arm64 \
MAINTAINER="Maintainer Name <maintainer@example.org>" \
packaging/deb/build-deb.sh
```

Useful optional overrides:

```sh
OUT_DIR=/tmp/nexus-bs-debs packaging/deb/build-deb.sh
WORK_DIR=/tmp/nexus-bs-package-build packaging/deb/build-deb.sh
DEPENDS="libc6, libgcc-s1, libsoapysdr0.8, systemd" packaging/deb/build-deb.sh
KEEP_BUILD_DIR=1 packaging/deb/build-deb.sh
```

The builder refuses to package active credential-like settings such as
`password`, `token`, `secret`, `api_key`, or `client_secret` from the example
config files. Keep private values out of `compiled_distribution/config/`.

## Generate a Static APT Repository

Install the repository generation tools:

```sh
sudo apt update
sudo apt install dpkg-dev gzip apt-utils
```

Create a static repository from one or more `.deb` files:

```sh
packaging/apt/generate-apt-repo.sh \
  --output /tmp/nexus-bs-apt \
  --suite stable \
  --component main \
  --repo-url https://example.org/nexus-bs-apt \
  packaging/deb/dist/nexus-bs_*.deb
```

You can also import every package directly inside a directory:

```sh
packaging/apt/generate-apt-repo.sh \
  --output /tmp/nexus-bs-apt \
  --suite stable \
  --component main \
  --input-dir packaging/deb/dist
```

The generated tree is static and can be published with GitHub Pages, nginx,
Apache, object storage, or any plain file server:

```text
dists/stable/Release
dists/stable/InRelease
dists/stable/Release.gpg
dists/stable/main/binary-arm64/Packages
dists/stable/main/binary-arm64/Packages.gz
pool/main/nexus-bs_VERSION_ARCH.deb
```

`InRelease` and `Release.gpg` are created only when signing is enabled.

By default, the generator cleans the output directory before writing the new
repository. Use `--no-clean` only when you intentionally want to keep existing
published files.

## Optional GPG Signing

Set `GPG_KEY` to the signing key ID, fingerprint, or identity accepted by
`gpg --local-user`:

```sh
GPG_KEY=0123456789ABCDEF \
packaging/apt/generate-apt-repo.sh \
  --output /tmp/nexus-bs-apt \
  --suite stable \
  --component main \
  --repo-url https://example.org/nexus-bs-apt \
  packaging/deb/dist/nexus-bs_*.deb
```

Publish the public key next to the repository so target hosts can use
`signed-by`:

```sh
gpg --export 0123456789ABCDEF | gpg --dearmor > /tmp/nexus-bs-apt/nexus-bs-archive-keyring.gpg
```

If `GPG_KEY` is not set, the repository remains unsigned. The generator prints a
`deb [trusted=yes] ...` source line when `--repo-url` is provided. Use unsigned
repositories only for controlled local testing.

## Configure the APT Source on a Target Host

For a signed repository, install the archive key and add the source list:

```sh
sudo apt update
sudo apt install ca-certificates curl

curl -fsSL https://example.org/nexus-bs-apt/nexus-bs-archive-keyring.gpg \
  | sudo tee /usr/share/keyrings/nexus-bs-archive-keyring.gpg >/dev/null

echo "deb [signed-by=/usr/share/keyrings/nexus-bs-archive-keyring.gpg] https://example.org/nexus-bs-apt stable main" \
  | sudo tee /etc/apt/sources.list.d/nexus-bs.list >/dev/null

sudo apt update
```

For an unsigned test repository:

```sh
echo "deb [trusted=yes] https://example.org/nexus-bs-apt stable main" \
  | sudo tee /etc/apt/sources.list.d/nexus-bs.list >/dev/null

sudo apt update
```

Replace `https://example.org/nexus-bs-apt`, `stable`, and `main` with the URL,
suite, and component used when generating the repository.

## Install the Package

Install Nexus-BS:

```sh
sudo apt install nexus-bs
```

The package depends on systemd and the SoapySDR runtime library. Install the
SoapySDR hardware module required by your SDR separately if your distribution
does not pull it in automatically.

Confirm the package contents:

```sh
dpkg -L nexus-bs
apt-cache policy nexus-bs
```

## Create Global Configuration

Choose the Linux user that will run the services. The user must already exist:

```sh
export NEXUS_USER=nexusbs
id "$NEXUS_USER"
```

The package creates `/etc/nexus-bs/config.toml` and
`/etc/nexus-bs/config.toml.fallback` from `/etc/nexus-bs/examples/` only when
the live files do not already exist. It does not overwrite existing live
configuration during install or upgrade.

Review and edit the live config before starting the RF service:

```sh
sudo -e /etc/nexus-bs/config.toml
```

At minimum, verify:

- SDR driver, device arguments, antennas, gains, and sample rates.
- TX and RX frequencies for your licensed RF plan.
- Network and cell identity values for your deployment.
- Dashboard authentication if the dashboard is reachable by other users.
- Any external service credentials. Keep real credentials out of example files
  and package artifacts.

Keep `config.toml.fallback` as a known-good fallback config.

## Enable Systemd Services

The package installs three systemd templates:

- `nexus-bs-control@USER.service` for the local command bridge.
- `nexus-bs@USER.service` for the RF/TETRA base-station process.
- `nexus-bs-dashboard@USER.service` for the external dashboard front end.

Enable them in dependency order:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now nexus-bs-control@"$NEXUS_USER".service
sudo systemctl enable --now nexus-bs@"$NEXUS_USER".service
sudo systemctl enable --now nexus-bs-dashboard@"$NEXUS_USER".service
```

The packaged units use `/etc/nexus-bs/config.toml` and
`/etc/nexus-bs/config.toml.fallback`, copy runtime config to
`/run/nexus-bs-$NEXUS_USER/`, run binaries from `/opt/nexus-bs/bin`, and serve
dashboard assets from `/opt/nexus-bs/dashboard`.

The dashboard front end listens on port `8080` by default. The core dashboard
API is loopback-only on `127.0.0.1:18080`.

## Verify the Install

Check service state:

```sh
systemctl is-active nexus-bs-control@"$NEXUS_USER".service
systemctl is-active nexus-bs@"$NEXUS_USER".service
systemctl is-active nexus-bs-dashboard@"$NEXUS_USER".service
```

Inspect recent logs:

```sh
journalctl -u nexus-bs-control@"$NEXUS_USER".service -n 80 --no-pager
journalctl -u nexus-bs@"$NEXUS_USER".service -n 160 --no-pager
journalctl -u nexus-bs-dashboard@"$NEXUS_USER".service -n 80 --no-pager
```

Check that the dashboard responds locally:

```sh
curl -f http://127.0.0.1:8080/ >/dev/null
```

From another machine, open:

```text
http://<target-ip>:8080
```

## Update

Publish the new package version to the APT repository, then update the target:

```sh
sudo apt update
sudo apt install --only-upgrade nexus-bs
```

Restart the services after an upgrade:

```sh
sudo systemctl restart nexus-bs-control@"$NEXUS_USER".service
sudo systemctl restart nexus-bs@"$NEXUS_USER".service
sudo systemctl restart nexus-bs-dashboard@"$NEXUS_USER".service
```

Package upgrades replace `/opt/nexus-bs` and systemd unit templates. They do not
overwrite `/etc/nexus-bs/config.toml` or `/etc/nexus-bs/config.toml.fallback`.

## Stop, Remove, or Purge

Stop the running instance:

```sh
sudo systemctl stop nexus-bs-dashboard@"$NEXUS_USER".service
sudo systemctl stop nexus-bs@"$NEXUS_USER".service
sudo systemctl stop nexus-bs-control@"$NEXUS_USER".service
```

Disable it:

```sh
sudo systemctl disable nexus-bs-dashboard@"$NEXUS_USER".service
sudo systemctl disable nexus-bs@"$NEXUS_USER".service
sudo systemctl disable nexus-bs-control@"$NEXUS_USER".service
```

Remove the package while keeping configuration:

```sh
sudo apt remove nexus-bs
```

Purge package-owned files:

```sh
sudo apt purge nexus-bs
```

The live configuration files are intentionally not package-owned. `apt remove`,
`apt purge`, and package upgrades do not delete
`/etc/nexus-bs/config.toml` or `/etc/nexus-bs/config.toml.fallback`.

## Troubleshooting

`apt update` says the repository is unsigned:

- For a signed repository, make sure the `signed-by` path exists and contains
  the public key exported from the signing key.
- For a local unsigned test repository, use `deb [trusted=yes] ...`.

`apt install nexus-bs` cannot find the package:

- Check the URL, suite, and component in `/etc/apt/sources.list.d/nexus-bs.list`.
- Make sure the generated repository contains a `binary-<arch>` index matching
  the target's `dpkg --print-architecture`.
- Regenerate the repository with `--architecture arm64` or the target
  architecture if auto-detection does not match.

The package build fails with missing `compiled_distribution` files:

- Rebuild or refresh `compiled_distribution/` before running
  `packaging/deb/build-deb.sh`.
- Do not remove required dashboard assets or systemd templates from the bundle.

The service fails before the main process starts:

- Check that `/etc/nexus-bs/config.toml` exists. The packaged unit copies it to
  `/run/nexus-bs-USER/config.toml` during root pre-start, then runs the core as
  `USER`.
- Check that `/etc/nexus-bs/config.toml.fallback` exists if you expect fallback
  startup behavior.
- Run `systemctl cat nexus-bs@USER.service` to confirm the packaged paths point
  at `/opt/nexus-bs` and `/etc/nexus-bs`.

The RF process starts and then exits:

- Inspect `journalctl -u nexus-bs@USER.service -n 160 --no-pager`.
- Verify the SoapySDR runtime and the hardware-specific SoapySDR module are
  installed.
- Verify the SDR device string, antennas, gains, and frequencies in the live
  config.

The dashboard opens but API calls fail:

- Check `systemctl is-active nexus-bs@USER.service`.
- Check `systemctl is-active nexus-bs-dashboard@USER.service`.
- Confirm that the core dashboard API is bound to `127.0.0.1:18080` and the
  external dashboard service is listening on port `8080`.

The control service fails:

- Check `journalctl -u nexus-bs-control@USER.service -n 80 --no-pager`.
- Confirm that `/run/nexus-bs-$NEXUS_USER/control.commands` is created by the
  service and owned by the service user.
