#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
# SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0

set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PROJECTS=(-p nexus-bs -p nexus-bs-control -p nexus-bs-dashboard)
BIN_NAMES=(nexus-bs nexus-bs-control-service nexus-bs-dashboard)
DEB_DIST_DIR="${DEB_DIST_DIR:-$ROOT/packaging/deb/dist}"
RELEASE_DIR="${RELEASE_DIR:-$ROOT/packaging/release}"
DIST_DIR="${DIST_DIR:-}"
TEMP_DIST_DIR=""

log() {
    printf '[release] %s\n' "$*"
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

cleanup() {
    if [ -n "$TEMP_DIST_DIR" ]; then
        rm -rf "$TEMP_DIST_DIR"
    fi
}

trap cleanup EXIT

project_version() {
    awk '
        /^\[workspace\.package\][[:space:]]*$/ { in_workspace_package = 1; next }
        /^\[/ { in_workspace_package = 0 }
        in_workspace_package && /^[[:space:]]*version[[:space:]]*=/ {
            sub(/^[^=]*=[[:space:]]*/, "", $0)
            gsub(/"/, "", $0)
            gsub(/[[:space:]]/, "", $0)
            print
            exit
        }
    ' Cargo.toml
}

sha256_cmd() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$@"
    else
        shasum -a 256 "$@"
    fi
}

validate_tag() {
    version="$1"
    tag="${RELEASE_TAG:-${GITHUB_REF_NAME:-}}"
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "Cargo workspace version '${version}' must be strict X.Y.Z for release artifacts"
    if [ "${GITHUB_REF_TYPE:-}" = "tag" ] || [ -n "${RELEASE_TAG:-}" ]; then
        [ -n "$tag" ] || die "release tag is missing"
        [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "release tag '${tag}' must be strict vX.Y.Z"
        [ "$tag" = "v${version}" ] || die "release tag '${tag}' does not match Cargo workspace version 'v${version}'"
        git diff --quiet || die "release tag builds require a clean worktree before compiling"
        git diff --cached --quiet || die "release tag builds require a clean index before compiling"
    else
        tag="v${version}"
    fi
    RELEASE_TAG="$tag"
    export RELEASE_TAG
    log "Release tag ${RELEASE_TAG}"
}

build_binaries() {
    if [ "${FORCE_ZIGBUILD:-0}" = "1" ] || [ "$(uname -m)" != "aarch64" ]; then
        command -v cargo-zigbuild >/dev/null 2>&1 || die "cargo-zigbuild is required for non-aarch64 release builds"
        log "Building optimized ARM64 binaries with cargo zigbuild"
        env \
            PKG_CONFIG_ALLOW_CROSS=1 \
            PKG_CONFIG_LIBDIR="${PKG_CONFIG_LIBDIR:-/private/tmp/soapy-aarch64/lib/pkgconfig}" \
            PKG_CONFIG_PATH="${PKG_CONFIG_PATH:-/private/tmp/soapy-aarch64/lib/pkgconfig}" \
            LIBRARY_PATH="${LIBRARY_PATH:-/private/tmp/soapy-aarch64/lib}" \
            ZIG_GLOBAL_CACHE_DIR="${ZIG_GLOBAL_CACHE_DIR:-${TMPDIR:-/tmp}/nexus-bs-zig-cache}" \
            ZIG_LOCAL_CACHE_DIR="${ZIG_LOCAL_CACHE_DIR:-${TMPDIR:-/tmp}/nexus-bs-zig-local}" \
            cargo zigbuild --release "${PROJECTS[@]}" --target aarch64-unknown-linux-gnu --locked --bins
        BIN_DIR="$ROOT/target/aarch64-unknown-linux-gnu/release"
    else
        log "Building optimized ARM64 binaries natively"
        cargo build --release "${PROJECTS[@]}" --locked --bins
        BIN_DIR="$ROOT/target/release"
    fi
    export BIN_DIR
}

sync_distribution_payload() {
    version="$1"

    if [ -z "$DIST_DIR" ]; then
        DIST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/nexus-bs-dist.XXXXXX")"
        TEMP_DIST_DIR="$DIST_DIR"
        export DIST_DIR
    fi

    log "Syncing temporary package payload"
    rm -rf "$DIST_DIR"
    install -d -m 0755 "$DIST_DIR/bin" "$DIST_DIR/config" "$DIST_DIR/dashboard/assets" "$DIST_DIR/scripts" "$DIST_DIR/systemd"
    for name in "${BIN_NAMES[@]}"; do
        install -m 0755 "$BIN_DIR/$name" "$DIST_DIR/bin/$name"
    done

    install -m 0644 example_config/config_template.toml "$DIST_DIR/config/config_template.toml"
    install -m 0644 dashboard/index.html "$DIST_DIR/dashboard/index.html"
    install -m 0644 dashboard/assets/app.js "$DIST_DIR/dashboard/assets/app.js"
    install -m 0644 dashboard/assets/styles.css "$DIST_DIR/dashboard/assets/styles.css"
    install -m 0644 dashboard/assets/nexus-bs-logo.svg "$DIST_DIR/dashboard/assets/nexus-bs-logo.svg"
    install -m 0644 dashboard/assets/nexus-bs-logo.png "$DIST_DIR/dashboard/assets/nexus-bs-logo.png"
    install -m 0755 scripts/nexus-bs-service "$DIST_DIR/scripts/nexus-bs-service"

    install -m 0644 contrib/systemd/nexus-bs.service "$DIST_DIR/systemd/nexus-bs.service"
    install -m 0644 contrib/systemd/nexus-bs-control.service "$DIST_DIR/systemd/nexus-bs-control.service"
    install -m 0644 contrib/systemd/nexus-bs-dashboard.service "$DIST_DIR/systemd/nexus-bs-dashboard.service"
    install -m 0644 contrib/systemd/journald-nexus-bs-volatile.conf "$DIST_DIR/systemd/journald-nexus-bs-volatile.conf"

    VERSION="$version" perl -0pi -e 's/Nexus-BS v[0-9]+\.[0-9]+\.[0-9]+/Nexus-BS v$ENV{VERSION}/g' \
        "$DIST_DIR/systemd/nexus-bs.service" \
        "$DIST_DIR/systemd/nexus-bs-control.service" \
        "$DIST_DIR/systemd/nexus-bs-dashboard.service"

}

strip_release_binaries() {
    local strip_cmd

    if [ -n "${STRIP_CMD:-}" ]; then
        strip_cmd="$STRIP_CMD"
    elif command -v aarch64-linux-gnu-strip >/dev/null 2>&1; then
        strip_cmd="aarch64-linux-gnu-strip"
    elif command -v llvm-strip >/dev/null 2>&1; then
        strip_cmd="llvm-strip"
    elif [ "$(uname -m)" = "aarch64" ] && command -v strip >/dev/null 2>&1; then
        strip_cmd="strip"
    else
        die "an ARM64-capable strip tool is required; set STRIP_CMD or install binutils"
    fi

    log "Stripping release binaries with ${strip_cmd}"
    for name in "${BIN_NAMES[@]}"; do
        "$strip_cmd" --strip-unneeded "$DIST_DIR/bin/$name"
    done
}

update_distribution_readme() {
    version="$1"
    checksum_file="$DIST_DIR/.release-checksums"
    (
        cd "$DIST_DIR"
        sha256_cmd \
            bin/nexus-bs \
            bin/nexus-bs-control-service \
            bin/nexus-bs-dashboard \
            config/config_template.toml \
            dashboard/assets/app.js \
            dashboard/assets/nexus-bs-logo.png \
            dashboard/assets/nexus-bs-logo.svg \
            dashboard/assets/styles.css \
            dashboard/index.html \
            scripts/nexus-bs-service \
            systemd/journald-nexus-bs-volatile.conf \
            systemd/nexus-bs-control.service \
            systemd/nexus-bs-dashboard.service \
            systemd/nexus-bs.service
    ) > "$checksum_file"

    python3 - "$version" "$checksum_file" "$DIST_DIR/README.md" <<'PY'
import pathlib
import sys

version, checksum_path, readme_path = sys.argv[1:]
checksums = pathlib.Path(checksum_path).read_text().strip()
pathlib.Path(readme_path).write_text(f"""# Nexus-BS Compiled Distribution

Minimal Linux/aarch64 binary bundle for Nexus-BS `v{version}`.

This directory is a temporary package payload generated by
`scripts/build-release-artifacts.sh`; it is not kept in git.

## Contents

- `bin/`: optimized Linux/aarch64 service binaries.
- `dashboard/`: external dashboard static assets.
- `systemd/`: service units used by the package builder.
- `scripts/`: `nexus-bs-service` helper.
- `config/config_template.toml`: generic Easy Start template only.

## SHA-256 Checksums

```text
{checksums}
```
""")
PY
    rm -f "$checksum_file"
}

verify_distribution() {
    local version name dirty_matches
    version="$1"

    log "Verifying release distribution binary identity"
    strings "$DIST_DIR/bin/nexus-bs" | grep -F "Nexus-BS/v${version}" >/dev/null
    strings "$DIST_DIR/bin/nexus-bs-control-service" | grep -F "nexus-bs-control-v${version}" >/dev/null
    strings "$DIST_DIR/bin/nexus-bs-dashboard" | grep -F "nexus-bs-telemetry-v${version}" >/dev/null
    for name in "${BIN_NAMES[@]}"; do
        dirty_matches="$(strings "$DIST_DIR/bin/$name" | grep -E 'v[0-9][0-9.]*(_dev|-[0-9a-f]{8}-modified)' || true)"
        if [ -n "$dirty_matches" ]; then
            die "release binary ${name} contains dirty/dev version strings"
        fi
    done

    for name in "${BIN_NAMES[@]}"; do
        file "$DIST_DIR/bin/$name" | grep -Eq 'ARM aarch64|ARM64' || die "$name is not an ARM64 binary"
    done
}

build_deb() {
    version="$1"

    log "Building Debian package"
    rm -rf "$DEB_DIST_DIR"
    mkdir -p "$DEB_DIST_DIR"
    DIST_DIR="$DIST_DIR" OUT_DIR="$DEB_DIST_DIR" VERSION="$version" ARCH=arm64 packaging/deb/build-deb.sh
    dpkg-deb -f "$DEB_DIST_DIR/nexus-bs_${version}_arm64.deb" Package Version Architecture
}

bundle_release_artifacts() {
    version="$1"

    log "Bundling release artifacts"
    rm -rf "$RELEASE_DIR"
    mkdir -p "$RELEASE_DIR"
    cp "$DEB_DIST_DIR/nexus-bs_${version}_arm64.deb" "$RELEASE_DIR/"
    (
        cd "$RELEASE_DIR"
        sha256_cmd *.deb > SHA256SUMS
    )
    cp "$RELEASE_DIR/SHA256SUMS" "$DEB_DIST_DIR/SHA256SUMS"
}

main() {
    version="$(project_version)"
    [ -n "$version" ] || die "could not read Cargo workspace version"
    validate_tag "$version"
    build_binaries
    sync_distribution_payload "$version"
    strip_release_binaries
    update_distribution_readme "$version"
    verify_distribution "$version"
    build_deb "$version"
    bundle_release_artifacts "$version"
    log "Release artifacts ready in $RELEASE_DIR"
}

main "$@"
