#!/bin/sh
# SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
# SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0

set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
RUN_USER="${NEXUS_BS_USER:-${SUDO_USER:-${USER:-$(id -un)}}}"
RUN_HOME="${NEXUS_BS_HOME:-/home/${RUN_USER}}"
RUN_DIR="${NEXUS_BS_RUNTIME_DIR:-${RUN_HOME}/nexus-bs}"
CONFIG_DIR="${NEXUS_BS_CONFIG_DIR:-/etc/nexus-bs}"
CONFIG_FILE="${CONFIG_DIR}/config.toml"
FALLBACK_FILE="${CONFIG_FILE}.fallback"
SYSTEMD_UNIT_DIR="${NEXUS_BS_SYSTEMD_UNIT_DIR:-/etc/systemd/system}"
PATH_HELPER_DIR="${NEXUS_BS_PATH_HELPER_DIR:-/usr/local/bin}"
BIN_DIR="${NEXUS_BS_BIN_DIR:-${ROOT}/target/release}"
SUDO_CMD="${NEXUS_BS_SUDO-sudo}"
SYSTEMCTL_CMD="${NEXUS_BS_SYSTEMCTL:-systemctl}"
BUILD=1
INSTALL_PREREQS=1

usage() {
    cat <<EOF
Usage: ./scripts/install-from-source.sh [--no-build] [--skip-prereqs]

Builds Nexus-BS from this source checkout and installs/updates the local runtime.

Defaults:
  Service user: ${RUN_USER}
  Runtime dir:  ${RUN_DIR}
  Config file:  ${CONFIG_FILE}
  Prereqs:      install missing Debian build packages and Rust automatically

Environment overrides:
  NEXUS_BS_USER          Service user
  NEXUS_BS_HOME          User home, default: /home/\$NEXUS_BS_USER
  NEXUS_BS_RUNTIME_DIR   Runtime directory
  NEXUS_BS_CONFIG_DIR    Config directory
EOF
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --no-build)
            BUILD=0
            ;;
        --skip-prereqs)
            INSTALL_PREREQS=0
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1"
            ;;
    esac
    shift
done

[ "$(id -u)" -ne 0 ] || die "run this as the service user, not as root"
[ -d "$ROOT" ] || die "missing source root: $ROOT"
[ -d "$RUN_HOME" ] || die "service home directory not found: $RUN_HOME"

RUN_GROUP="$(id -gn "$RUN_USER")"

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

as_root() {
    if [ "$(id -u)" -eq 0 ]; then
        "$@"
    elif [ -n "$SUDO_CMD" ]; then
        "$SUDO_CMD" "$@"
    else
        "$@"
    fi
}

require_command install
require_command awk

if [ "$(id -u)" -ne 0 ] && [ -n "$SUDO_CMD" ]; then
    require_command "$SUDO_CMD"
fi

install_debian_prereqs() {
    if ! command -v apt-get >/dev/null 2>&1 || ! command -v dpkg-query >/dev/null 2>&1; then
        return 0
    fi

    missing=""
    for pkg in git curl ca-certificates build-essential pkg-config cmake clang libsoapysdr-dev soapysdr-tools; do
        if ! dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null | grep -q 'install ok installed'; then
            missing="${missing} ${pkg}"
        fi
    done

    if [ -n "$missing" ]; then
        printf 'Installing missing Debian packages:%s\n' "$missing"
        as_root apt-get update
        as_root apt-get install -y $missing
    fi
}

ensure_rust_toolchain() {
    if command -v cargo >/dev/null 2>&1; then
        return 0
    fi

    require_command curl
    printf 'Rust/Cargo not found. Installing Rust toolchain with rustup.\n'
    rustup_script="${TMPDIR:-/tmp}/nexus-bs-rustup-init.$$"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$rustup_script"
    sh "$rustup_script" -y
    rm -f "$rustup_script"

    PATH="${HOME}/.cargo/bin:${PATH}"
    export PATH
    require_command cargo
}

atomic_install_user_file() {
    src="$1"
    dst="$2"
    tmp="${dst}.new.$$"
    install -m 0755 "$src" "$tmp"
    mv -f "$tmp" "$dst"
}

copy_dashboard() {
    tmp="${RUN_DIR}/dashboard.new.$$"
    rm -rf "$tmp"
    mkdir -p "$tmp"
    cp -R "$ROOT/dashboard/." "$tmp/"
    find "$tmp" -name .DS_Store -type f -exec rm -f {} +
    rm -rf "${RUN_DIR}/dashboard.old.$$"
    if [ -d "${RUN_DIR}/dashboard" ]; then
        mv "${RUN_DIR}/dashboard" "${RUN_DIR}/dashboard.old.$$"
    fi
    mv "$tmp" "${RUN_DIR}/dashboard"
    rm -rf "${RUN_DIR}/dashboard.old.$$"
}

set_service_name_in_file() {
    file="$1"
    tmp="${TMPDIR:-/tmp}/nexus-bs-config.$$"
    awk -v unit="nexus-bs.service" '
        BEGIN { done = 0 }
        /^[[:space:]]*service_name[[:space:]]*=/ && done == 0 {
            print "service_name = \"" unit "\""
            done = 1
            next
        }
        { print }
    ' "$file" > "$tmp"
    as_root install -o "$RUN_USER" -g "$RUN_GROUP" -m 0600 "$tmp" "$file"
    rm -f "$tmp"
}

install_config_if_missing() {
    as_root install -d -o "$RUN_USER" -g "$RUN_GROUP" -m 0750 "$CONFIG_DIR"

    if [ ! -e "$CONFIG_FILE" ]; then
        if [ -f "${RUN_DIR}/config.toml" ] && [ ! -L "${RUN_DIR}/config.toml" ]; then
            as_root install -o "$RUN_USER" -g "$RUN_GROUP" -m 0600 "${RUN_DIR}/config.toml" "$CONFIG_FILE"
        else
            as_root install -o "$RUN_USER" -g "$RUN_GROUP" -m 0600 "$ROOT/example_config/config.toml" "$CONFIG_FILE"
            set_service_name_in_file "$CONFIG_FILE"
        fi
        printf 'Created config: %s\n' "$CONFIG_FILE"
    else
        printf 'Kept existing config: %s\n' "$CONFIG_FILE"
    fi

    if [ ! -e "$FALLBACK_FILE" ]; then
        as_root install -o "$RUN_USER" -g "$RUN_GROUP" -m 0600 "$CONFIG_FILE" "$FALLBACK_FILE"
        printf 'Created fallback config: %s\n' "$FALLBACK_FILE"
    else
        printf 'Kept existing fallback config: %s\n' "$FALLBACK_FILE"
    fi
}

link_runtime_config() {
    stamp="$(date +%Y%m%d%H%M%S)"
    if [ -e "${RUN_DIR}/config.toml" ] && [ ! -L "${RUN_DIR}/config.toml" ]; then
        mv "${RUN_DIR}/config.toml" "${RUN_DIR}/config.toml.local-before-etc-${stamp}"
    fi
    if [ -e "${RUN_DIR}/config.toml.fallback" ] && [ ! -L "${RUN_DIR}/config.toml.fallback" ]; then
        mv "${RUN_DIR}/config.toml.fallback" "${RUN_DIR}/config.toml.fallback.local-before-etc-${stamp}"
    fi
    ln -sfn "$CONFIG_FILE" "${RUN_DIR}/config.toml"
    ln -sfn "$FALLBACK_FILE" "${RUN_DIR}/config.toml.fallback"
}

install_systemd_units() {
    as_root install -d -m 0755 "$SYSTEMD_UNIT_DIR"
    tmpdir="${TMPDIR:-/tmp}/nexus-bs-units.$$"
    mkdir -p "$tmpdir"
    for unit in nexus-bs.service nexus-bs-control.service nexus-bs-dashboard.service; do
        sed \
            -e "/^\\[Service\\]$/a\\
User=${RUN_USER}\\
Group=${RUN_GROUP}" \
            -e "s#/home/chris/nexus-bs#${RUN_DIR}#g" \
            "$ROOT/contrib/systemd/$unit" > "$tmpdir/$unit"
        as_root install -m 0644 "$tmpdir/$unit" "$SYSTEMD_UNIT_DIR/$unit"
    done
    rm -rf "$tmpdir"
    "$SYSTEMCTL_CMD" daemon-reload
}

install_service_helper_on_path() {
    if [ -n "$PATH_HELPER_DIR" ]; then
        as_root install -d -m 0755 "$PATH_HELPER_DIR"
        as_root ln -sfn "${RUN_DIR}/nexus-bs-service" "$PATH_HELPER_DIR/nexus-bs-service"
    fi
}

cd "$ROOT"

if [ "$INSTALL_PREREQS" = "1" ]; then
    install_debian_prereqs
fi

require_command "$SYSTEMCTL_CMD"

if [ "$BUILD" = "1" ]; then
    ensure_rust_toolchain
    cargo build --release --locked -p nexus-bs -p nexus-bs-control -p nexus-bs-dashboard --bins
fi

[ -x "$BIN_DIR/nexus-bs" ] || die "missing built binary: ${BIN_DIR}/nexus-bs"
[ -x "$BIN_DIR/nexus-bs-control-service" ] || die "missing built binary: ${BIN_DIR}/nexus-bs-control-service"
[ -x "$BIN_DIR/nexus-bs-dashboard" ] || die "missing built binary: ${BIN_DIR}/nexus-bs-dashboard"

mkdir -p "$RUN_DIR"
atomic_install_user_file "$BIN_DIR/nexus-bs" "${RUN_DIR}/nexus-bs"
atomic_install_user_file "$BIN_DIR/nexus-bs-control-service" "${RUN_DIR}/nexus-bs-control-service"
atomic_install_user_file "$BIN_DIR/nexus-bs-dashboard" "${RUN_DIR}/nexus-bs-dashboard"
atomic_install_user_file "$ROOT/scripts/nexus-bs-control" "${RUN_DIR}/nexus-bs-control"
atomic_install_user_file "$ROOT/scripts/nexus-bs-service" "${RUN_DIR}/nexus-bs-service"
atomic_install_user_file "$ROOT/scripts/nexus-bs-factory-reset-clean" "${RUN_DIR}/nexus-bs-factory-reset-clean"
copy_dashboard
install_config_if_missing
link_runtime_config
install_systemd_units
install_service_helper_on_path

cat <<EOF

Nexus-BS source install/update complete.

Live config:
  ${CONFIG_FILE}

Edit config:
  nexus-bs-service edit-config

Start all Nexus-BS services:
  nexus-bs-service start

Useful commands:
  nexus-bs-service status
  nexus-bs-service logs
  nexus-bs-service restart

Dashboard after start:
  http://<target-ip>:8080
EOF
