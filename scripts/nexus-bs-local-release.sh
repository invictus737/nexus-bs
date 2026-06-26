#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
# SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0

set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION=""
RUN_BUILD=1
RUN_TESTS=1
ALLOW_DIRTY=0
LOCAL_COMMIT=0
DEPLOY_REMOTE=""
DEPLOY_BASE=""

log() {
    printf '[local-release] %s\n' "$*"
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Usage: scripts/nexus-bs-local-release.sh --version X.Y.Z [options]

Local-only release preparation. It never pushes, tags, or talks to GitHub.

Options:
  --version X.Y.Z       Required target workspace version.
  --no-tests           Skip QA tests/checks.
  --no-build           Skip local ARM64 release build.
  --allow-dirty        Allow starting from a dirty worktree.
  --commit             Create a local git commit after QA passes.
  --deploy REMOTE      Deploy with scripts/nexus-bs-test-deploy.sh after build.
  --remote-base PATH   Remote staging base for deploy, e.g. /home/pi/nexus-bs.
  -h, --help           Show this help.

Recommended field test:
  scripts/nexus-bs-local-release.sh --version 0.1.81 --commit --deploy pi@192.168.1.179 --remote-base /home/pi/nexus-bs
EOF
}

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

workspace_packages() {
    cargo metadata --format-version 1 --locked --no-deps | python3 -c '
import json
import pathlib
import sys

metadata = json.load(sys.stdin)
packages = sorted(
    metadata["packages"],
    key=lambda package: pathlib.Path(package["manifest_path"]).as_posix(),
)
for package in packages:
    print(package["name"])
'
}

parse_args() {
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --version)
                [ "$#" -ge 2 ] || die "--version needs a value"
                VERSION="$2"
                shift 2
                ;;
            --no-tests)
                RUN_TESTS=0
                shift
                ;;
            --no-build)
                RUN_BUILD=0
                shift
                ;;
            --allow-dirty)
                ALLOW_DIRTY=1
                shift
                ;;
            --commit)
                LOCAL_COMMIT=1
                shift
                ;;
            --deploy)
                [ "$#" -ge 2 ] || die "--deploy needs a remote, e.g. pi@192.168.1.179"
                DEPLOY_REMOTE="$2"
                shift 2
                ;;
            --remote-base)
                [ "$#" -ge 2 ] || die "--remote-base needs a path"
                DEPLOY_BASE="$2"
                shift 2
                ;;
            -h|--help)
                usage
                exit 0
                ;;
            *)
                die "unknown argument: $1"
                ;;
        esac
    done
}

require_version() {
    [ -n "$VERSION" ] || die "--version X.Y.Z is required"
    [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must be strict X.Y.Z"
}

require_clean_start() {
    if [ "$ALLOW_DIRTY" = "1" ]; then
        log "Starting from dirty worktree because --allow-dirty was provided"
        return
    fi
    git diff --quiet || die "worktree has unstaged changes; commit/stash first or use --allow-dirty"
    git diff --cached --quiet || die "index has staged changes; commit/stash first or use --allow-dirty"
}

update_versions() {
    local packages
    packages=()
    while IFS= read -r package; do
        [ -n "$package" ] || continue
        packages+=("$package")
    done < <(workspace_packages)
    [ "${#packages[@]}" -gt 0 ] || die "cargo metadata returned no workspace packages"

    log "Updating workspace metadata to v${VERSION}"
    python3 - "$VERSION" "${packages[@]}" <<'PY'
import pathlib
import re
import sys

version = sys.argv[1]
packages = set(sys.argv[2:])

def replace_once(path, pattern, repl):
    text = path.read_text()
    new, count = re.subn(pattern, repl, text, count=1, flags=re.M)
    if count != 1:
        raise SystemExit(f"{path}: expected exactly one version replacement, got {count}")
    path.write_text(new)

replace_once(
    pathlib.Path("Cargo.toml"),
    r'(^\[workspace\.package\]\n(?:[^\[]*\n)*?version\s*=\s*")[0-9]+\.[0-9]+\.[0-9]+(")',
    rf'\g<1>{version}\2',
)

lock = pathlib.Path("Cargo.lock")
text = lock.read_text()
lines = text.splitlines()
current = None
for index, line in enumerate(lines):
    if line.startswith("name = "):
        current = line.split('"', 2)[1]
    elif current in packages and line.startswith("version = "):
        lines[index] = f'version = "{version}"'
        current = None
lock.write_text("\n".join(lines) + "\n")

for file_name in [
    "contrib/systemd/nexus-bs.service",
    "contrib/systemd/nexus-bs-control.service",
    "contrib/systemd/nexus-bs-dashboard.service",
]:
    path = pathlib.Path(file_name)
    path.write_text(re.sub(r"Nexus-BS v[0-9]+\.[0-9]+\.[0-9]+", f"Nexus-BS v{version}", path.read_text()))

path = pathlib.Path("scripts/tetrahs-reinstall-nexus-bs.sh")
path.write_text(re.sub(r"nexus-bs_[0-9]+\.[0-9]+\.[0-9]+_arm64[.]deb", f"nexus-bs_{version}_arm64.deb", path.read_text()))
PY
}

verify_version_consistency() {
    local current pkg packages
    current="$(project_version)"
    [ "$current" = "$VERSION" ] || die "Cargo.toml version ${current} != ${VERSION}"
    packages=()
    while IFS= read -r package; do
        [ -n "$package" ] || continue
        packages+=("$package")
    done < <(workspace_packages)
    [ "${#packages[@]}" -gt 0 ] || die "cargo metadata returned no workspace packages"

    for unit in \
        contrib/systemd/nexus-bs.service \
        contrib/systemd/nexus-bs-control.service \
        contrib/systemd/nexus-bs-dashboard.service; do
        grep -F "Nexus-BS v${VERSION}" "$unit" >/dev/null || die "$unit does not mention Nexus-BS v${VERSION}"
        ! grep -E "Nexus-BS v[0-9]+\.[0-9]+\.[0-9]+" "$unit" | grep -Fv "Nexus-BS v${VERSION}" >/dev/null \
            || die "$unit contains a stale Nexus-BS version"
    done

    for pkg in "${packages[@]}"; do
        awk -v pkg="$pkg" -v version="$VERSION" '
            $0 == "name = \"" pkg "\"" { want = 1; next }
            want && /^version = / {
                if ($0 != "version = \"" version "\"") exit 2
                found = 1
                want = 0
            }
            END { if (!found) exit 1 }
        ' Cargo.lock || die "Cargo.lock package ${pkg} does not track ${VERSION}"
    done

    grep -F "nexus-bs_${VERSION}_arm64.deb" scripts/tetrahs-reinstall-nexus-bs.sh >/dev/null \
        || die "tetrahs reinstall script does not point at ${VERSION} package"

    if LC_ALL=C grep -nE '^[[:space:]]*(username|password|token|secret|api[_-]?key|client[_-]?secret)[[:space:]]*=' example_config/config.toml >/dev/null; then
        die "example_config/config.toml contains active credential-like values"
    fi
}

run_qa() {
    [ "$RUN_TESTS" = "1" ] || return 0
    log "Running dashboard and release QA guards"
    node --check dashboard/assets/app.js
    cargo test -p tetra-core product_identity_tracks_workspace_version --locked
    cargo test -p tetra-config dashboard_auth --locked
    cargo test -p tetra-config test_example_config_keeps_transmission_interruption_disabled --locked
    cargo test -p tetra-entities dashboard_product_identity_tracks_workspace_version --locked
    cargo test -p tetra-entities --test test_dashboard_assets --locked
    bash -n scripts/nexus-bs-test-deploy.sh
    bash -n scripts/build-release-artifacts.sh
    bash -n packaging/deb/build-deb.sh
    git diff --check
}

build_local() {
    [ "$RUN_BUILD" = "1" ] || return 0
    log "Building local ARM64 release binaries"
    env \
        PKG_CONFIG_ALLOW_CROSS=1 \
        PKG_CONFIG_LIBDIR="${PKG_CONFIG_LIBDIR:-/private/tmp/soapy-aarch64/lib/pkgconfig}" \
        PKG_CONFIG_PATH="${PKG_CONFIG_PATH:-/private/tmp/soapy-aarch64/lib/pkgconfig}" \
        LIBRARY_PATH="${LIBRARY_PATH:-/private/tmp/soapy-aarch64/lib}" \
        ZIG_GLOBAL_CACHE_DIR="${ZIG_GLOBAL_CACHE_DIR:-${TMPDIR:-/tmp}/nexus-bs-zig-cache}" \
        ZIG_LOCAL_CACHE_DIR="${ZIG_LOCAL_CACHE_DIR:-${TMPDIR:-/tmp}/nexus-bs-zig-local}" \
        cargo zigbuild --release -p nexus-bs -p nexus-bs-control -p nexus-bs-dashboard --target aarch64-unknown-linux-gnu --locked --bins
}

commit_local() {
    [ "$LOCAL_COMMIT" = "1" ] || return 0
    log "Creating local release commit"
    git add \
        Cargo.toml \
        Cargo.lock \
        contrib/systemd/nexus-bs.service \
        contrib/systemd/nexus-bs-control.service \
        contrib/systemd/nexus-bs-dashboard.service \
        scripts/nexus-bs-local-release.sh \
        scripts/nexus-bs-test-deploy.sh \
        scripts/tetrahs-reinstall-nexus-bs.sh \
        dashboard/assets/app.js \
        crates/tetra-entities/tests/test_dashboard_assets.rs
    git commit -m "chore: prepare Nexus-BS v${VERSION}"
}

deploy_local() {
    [ -n "$DEPLOY_REMOTE" ] || return 0
    log "Deploying local build to ${DEPLOY_REMOTE}"
    if [ -n "$DEPLOY_BASE" ]; then
        REMOTE="$DEPLOY_REMOTE" REMOTE_BASE="$DEPLOY_BASE" RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh
    else
        REMOTE="$DEPLOY_REMOTE" RUN_TESTS=0 POST_START_SLEEP=10 scripts/nexus-bs-test-deploy.sh
    fi
}

main() {
    parse_args "$@"
    require_version
    require_clean_start
    update_versions
    verify_version_consistency
    run_qa
    build_local
    commit_local
    deploy_local
    log "Local release preparation complete for v${VERSION}. No GitHub push or tag was performed."
}

main "$@"
