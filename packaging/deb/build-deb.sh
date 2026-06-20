#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
# SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0

set -euo pipefail

umask 022

PACKAGE_NAME="nexus-bs"
INSTALL_PREFIX="/opt/nexus-bs"
ETC_PREFIX="/etc/nexus-bs"
SYSTEMD_UNIT_DIR="/lib/systemd/system"
DEFAULT_MAINTAINER="Nexus-BS Project <noreply@nexus-bs.local>"
DEFAULT_DEPENDS="libc6, libgcc-s1, libsoapysdr0.8 | libsoapysdr0.7, systemd"

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"
DIST_DIR="${DIST_DIR:-${REPO_ROOT}/compiled_distribution}"
TEMPLATE_DIR="${SCRIPT_DIR}/templates"

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
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
    ' "${REPO_ROOT}/Cargo.toml"
}

detect_arch() {
    local binary_file file_out machine
    binary_file="${DIST_DIR}/bin/nexus-bs"

    if command -v file >/dev/null 2>&1 && [ -f "$binary_file" ]; then
        file_out="$(file "$binary_file" 2>/dev/null || true)"
        case "$file_out" in
            *"ARM aarch64"*|*"ARM64"*) printf 'arm64\n'; return 0 ;;
            *"x86-64"*|*"x86_64"*) printf 'amd64\n'; return 0 ;;
            *"ARM,"*|*"ARM EABI"*) printf 'armhf\n'; return 0 ;;
        esac
    fi

    if command -v dpkg >/dev/null 2>&1; then
        dpkg --print-architecture
        return 0
    fi

    machine="$(uname -m)"
    case "$machine" in
        aarch64|arm64) printf 'arm64\n' ;;
        x86_64|amd64) printf 'amd64\n' ;;
        armv7l|armv7*) printf 'armhf\n' ;;
        *) die "cannot infer Debian architecture from ${machine}; set ARCH=..." ;;
    esac
}

validate_debian_field() {
    local name value pattern
    name="$1"
    value="$2"
    pattern="$3"

    [ -n "$value" ] || die "${name} must not be empty"
    case "$value" in
        *$'\n'*|*$'\r'*) die "${name} must be a single line" ;;
    esac
    if ! [[ "$value" =~ $pattern ]]; then
        die "${name} contains characters not accepted by this builder: ${value}"
    fi
}

require_distribution_files() {
    local required file
    required=(
        "bin/nexus-bs"
        "bin/nexus-bs-control-service"
        "bin/nexus-bs-dashboard"
        "dashboard/index.html"
        "dashboard/assets/app.js"
        "dashboard/assets/styles.css"
        "dashboard/assets/nexus-bs-logo.svg"
        "dashboard/assets/nexus-bs-logo.png"
        "config/config.toml"
        "config/config.toml.fallback"
        "config/config_template.toml"
        "systemd/nexus-bs.service"
        "systemd/nexus-bs-control.service"
        "systemd/nexus-bs-dashboard.service"
        "systemd/journald-nexus-bs-volatile.conf"
    )

    [ -d "$DIST_DIR" ] || die "missing distribution directory: ${DIST_DIR}"
    for file in "${required[@]}"; do
        [ -f "${DIST_DIR}/${file}" ] || die "missing compiled_distribution file: ${file}"
    done
}

reject_private_config_values() {
    local file matches

    for file in "${DIST_DIR}/config/config.toml" "${DIST_DIR}/config/config.toml.fallback"; do
        matches="$(
            LC_ALL=C grep -nE '^[[:space:]]*(username|password|token|secret|api[_-]?key|client[_-]?secret)[[:space:]]*=' "$file" || true
        )"
        if [ -n "$matches" ]; then
            printf 'Refusing to bundle active credential-like settings from %s:\n%s\n' "$file" "$matches" >&2
            printf 'Keep private values commented out of compiled_distribution/config before packaging.\n' >&2
            exit 1
        fi
    done
}

reject_nonrelease_binaries() {
    local file found line
    found=0

    for file in \
        "${DIST_DIR}/bin/nexus-bs" \
        "${DIST_DIR}/bin/nexus-bs-control-service" \
        "${DIST_DIR}/bin/nexus-bs-dashboard"; do
        while IFS= read -r line; do
            if [ "$found" -eq 0 ]; then
                printf 'Refusing to package non-release binary version strings:\n' >&2
            fi
            found=1
            printf '%s: %s\n' "$file" "$line" >&2
        done < <(strings "$file" | grep -E 'v[0-9][0-9.]*(_dev|-[0-9a-f]{8}-modified)' || true)
    done

    if [ "$found" -ne 0 ]; then
        printf 'Commit the release source, rebuild compiled_distribution/bin from a clean tree, then rebuild the .deb.\n' >&2
        exit 1
    fi
}

render_template() {
    local src dst line
    src="$1"
    dst="$2"

    : > "$dst"
    while IFS= read -r line || [ -n "$line" ]; do
        line="${line//@PACKAGE@/${PACKAGE_NAME}}"
        line="${line//@VERSION@/${VERSION}}"
        line="${line//@ARCH@/${ARCH}}"
        line="${line//@MAINTAINER@/${MAINTAINER}}"
        line="${line//@DEPENDS@/${DEPENDS}}"
        line="${line//@INSTALLED_SIZE@/${INSTALLED_SIZE}}"
        printf '%s\n' "$line" >> "$dst"
    done < "$src"
}

sed_replacement_escape() {
    printf '%s' "$1" | sed 's/[\/&]/\\&/g'
}

install_transformed_unit() {
    local src dst version_escaped
    src="$1"
    dst="$2"
    version_escaped="$(sed_replacement_escape "$VERSION")"

    sed \
        -e "s|/home/%i/nexus-bs/config.toml.fallback|${ETC_PREFIX}/config.toml.fallback|g" \
        -e "s|/home/%i/nexus-bs/config.toml|${ETC_PREFIX}/config.toml|g" \
        -e "s|/home/%i/nexus-bs/dashboard|${INSTALL_PREFIX}/dashboard|g" \
        -e "s|/home/%i/nexus-bs/nexus-bs-control-service|${INSTALL_PREFIX}/bin/nexus-bs-control-service|g" \
        -e "s|/home/%i/nexus-bs/nexus-bs-dashboard|${INSTALL_PREFIX}/bin/nexus-bs-dashboard|g" \
        -e "s|/home/%i/nexus-bs/nexus-bs|${INSTALL_PREFIX}/bin/nexus-bs|g" \
        -e "s|/home/%i/nexus-bs|${INSTALL_PREFIX}|g" \
        -e "s|/home/chris/nexus-bs/config.toml.fallback|${ETC_PREFIX}/config.toml.fallback|g" \
        -e "s|/home/chris/nexus-bs/config.toml|${ETC_PREFIX}/config.toml|g" \
        -e "s|/home/chris/nexus-bs/dashboard|${INSTALL_PREFIX}/dashboard|g" \
        -e "s|/home/chris/nexus-bs/nexus-bs-control-service|${INSTALL_PREFIX}/bin/nexus-bs-control-service|g" \
        -e "s|/home/chris/nexus-bs/nexus-bs-dashboard|${INSTALL_PREFIX}/bin/nexus-bs-dashboard|g" \
        -e "s|/home/chris/nexus-bs/nexus-bs|${INSTALL_PREFIX}/bin/nexus-bs|g" \
        -e "s|/home/chris/nexus-bs|${INSTALL_PREFIX}|g" \
        -e "/^User=chris$/d" \
        -e "s|^ExecStartPre=/usr/bin/install -m 0600 ${ETC_PREFIX}/config.toml /run/nexus-bs-%i/config.toml$|ExecStartPre=+/usr/bin/install -o %i -g %i -m 0600 ${ETC_PREFIX}/config.toml /run/nexus-bs-%i/config.toml|g" \
        -e "s|^ExecStartPre=/bin/sh -c 'if \[ -f \"\$1\" \]; then /usr/bin/install -m 0600 \"\$1\" \"\$2\"; fi' nexus-bs-copy-fallback ${ETC_PREFIX}/config.toml.fallback /run/nexus-bs-%i/config.toml.fallback$|ExecStartPre=+/bin/sh -c 'if [ -f \"\$1\" ]; then /usr/bin/install -o \"\$3\" -g \"\$3\" -m 0600 \"\$1\" \"\$2\"; fi' nexus-bs-copy-fallback ${ETC_PREFIX}/config.toml.fallback /run/nexus-bs-%i/config.toml.fallback %i|g" \
        -e "s|^ExecStartPre=/usr/bin/install -m 0600 ${ETC_PREFIX}/config.toml /run/nexus-bs/config.toml$|ExecStartPre=/usr/bin/install -m 0600 ${ETC_PREFIX}/config.toml /run/nexus-bs/config.toml|g" \
        -e "s|^ExecStartPre=/bin/sh -c 'if \[ -f \"\$1\" \]; then /usr/bin/install -m 0600 \"\$1\" \"\$2\"; fi' nexus-bs-copy-fallback ${ETC_PREFIX}/config.toml.fallback /run/nexus-bs/config.toml.fallback$|ExecStartPre=/bin/sh -c 'if [ -f \"\$1\" ]; then /usr/bin/install -m 0600 \"\$1\" \"\$2\"; fi' nexus-bs-copy-fallback ${ETC_PREFIX}/config.toml.fallback /run/nexus-bs/config.toml.fallback|g" \
        -e "s|Nexus-BS v[0-9][0-9.]*|Nexus-BS v${version_escaped}|g" \
        "$src" > "$dst"
    chmod 0644 "$dst"
}

install_payload() {
    local root prefix_rel etc_rel systemd_rel asset asset_name
    root="$1"
    prefix_rel="${INSTALL_PREFIX#/}"
    etc_rel="${ETC_PREFIX#/}"
    systemd_rel="${SYSTEMD_UNIT_DIR#/}"

    install -d -m 0755 "${root}/${prefix_rel}/bin"
    install -m 0755 "${DIST_DIR}/bin/nexus-bs" "${root}/${prefix_rel}/bin/nexus-bs"
    install -m 0755 "${DIST_DIR}/bin/nexus-bs-control-service" "${root}/${prefix_rel}/bin/nexus-bs-control-service"
    install -m 0755 "${DIST_DIR}/bin/nexus-bs-dashboard" "${root}/${prefix_rel}/bin/nexus-bs-dashboard"
    install -m 0755 "${REPO_ROOT}/scripts/nexus-bs-service" "${root}/${prefix_rel}/bin/nexus-bs-service"
    install -d -m 0755 "${root}/usr/bin"
    install -m 0755 "${REPO_ROOT}/scripts/nexus-bs-service" "${root}/usr/bin/nexus-bs-service"

    install -d -m 0755 "${root}/${prefix_rel}/dashboard/assets"
    install -m 0644 "${DIST_DIR}/dashboard/index.html" "${root}/${prefix_rel}/dashboard/index.html"
    for asset in "${DIST_DIR}/dashboard/assets/"*; do
        [ -f "$asset" ] || continue
        asset_name="$(basename "$asset")"
        case "$asset_name" in
            app.js|styles.css|nexus-bs-logo.svg|nexus-bs-logo.png)
                install -m 0644 "$asset" "${root}/${prefix_rel}/dashboard/assets/${asset_name}"
                ;;
            *)
                die "unexpected dashboard asset in compiled_distribution: ${asset_name}"
                ;;
        esac
    done

    install -d -m 0755 "${root}/${etc_rel}/examples/systemd"
    install -m 0644 "${DIST_DIR}/config/config.toml" "${root}/${etc_rel}/examples/config.toml"
    install -m 0644 "${DIST_DIR}/config/config.toml.fallback" "${root}/${etc_rel}/examples/config.toml.fallback"
    install -m 0644 "${DIST_DIR}/config/config_template.toml" "${root}/${etc_rel}/examples/config_template.toml"
    install -m 0644 "${DIST_DIR}/systemd/journald-nexus-bs-volatile.conf" "${root}/${etc_rel}/examples/systemd/journald-nexus-bs-volatile.conf"

    install -d -m 0755 "${root}/${systemd_rel}"
    install_transformed_unit "${DIST_DIR}/systemd/nexus-bs.service" "${root}/${systemd_rel}/nexus-bs.service"
    install_transformed_unit "${DIST_DIR}/systemd/nexus-bs-control.service" "${root}/${systemd_rel}/nexus-bs-control.service"
    install_transformed_unit "${DIST_DIR}/systemd/nexus-bs-dashboard.service" "${root}/${systemd_rel}/nexus-bs-dashboard.service"
}

generate_debian_metadata() {
    local root
    root="$1"

    install -d -m 0755 "${root}/DEBIAN"
    INSTALLED_SIZE="$(du -sk "$root" | awk '{print $1}')"
    render_template "${TEMPLATE_DIR}/control.in" "${root}/DEBIAN/control"
    render_template "${TEMPLATE_DIR}/postinst.in" "${root}/DEBIAN/postinst"
    render_template "${TEMPLATE_DIR}/postrm.in" "${root}/DEBIAN/postrm"
    chmod 0644 "${root}/DEBIAN/control"
    chmod 0755 "${root}/DEBIAN/postinst" "${root}/DEBIAN/postrm"
    cat > "${root}/DEBIAN/conffiles" <<EOF
${ETC_PREFIX}/examples/config.toml
${ETC_PREFIX}/examples/config.toml.fallback
${ETC_PREFIX}/examples/config_template.toml
${ETC_PREFIX}/examples/systemd/journald-nexus-bs-volatile.conf
EOF
}

build_package() {
    local root deb_path dpkg_help
    root="$1"
    deb_path="$2"

    command -v dpkg-deb >/dev/null 2>&1 || die "dpkg-deb is required to build the Debian package"

    rm -f "$deb_path"
    dpkg_help="$(dpkg-deb --help 2>&1 || true)"
    if [[ "$dpkg_help" == *"--root-owner-group"* ]]; then
        dpkg-deb --build --root-owner-group "$root" "$deb_path"
    else
        dpkg-deb --build "$root" "$deb_path"
    fi
}

VERSION="${VERSION:-$(project_version)}"
ARCH="${ARCH:-$(detect_arch)}"
MAINTAINER="${MAINTAINER:-$DEFAULT_MAINTAINER}"
DEPENDS="${DEPENDS:-$DEFAULT_DEPENDS}"
OUT_DIR="${OUT_DIR:-${SCRIPT_DIR}/dist}"
WORK_DIR="${WORK_DIR:-${SCRIPT_DIR}/.build}"
DEB_ROOT="${WORK_DIR}/${PACKAGE_NAME}_${VERSION}_${ARCH}"
DEB_PATH="${OUT_DIR}/${PACKAGE_NAME}_${VERSION}_${ARCH}.deb"
INSTALLED_SIZE="0"

validate_debian_field "VERSION" "$VERSION" '^[0-9A-Za-z.+:~-]+$'
validate_debian_field "ARCH" "$ARCH" '^[0-9A-Za-z][0-9A-Za-z-]*$'
validate_debian_field "MAINTAINER" "$MAINTAINER" '^[^<>]+ <[^<>[:space:]]+@[^<>[:space:]]+>$'
validate_debian_field "DEPENDS" "$DEPENDS" '^[0-9A-Za-z.,+~:|()<>= _-]+$'
require_distribution_files
reject_private_config_values
reject_nonrelease_binaries

mkdir -p "$OUT_DIR" "$WORK_DIR"
rm -rf "$DEB_ROOT"
install -d -m 0755 "$DEB_ROOT"

install_payload "$DEB_ROOT"
generate_debian_metadata "$DEB_ROOT"
build_package "$DEB_ROOT" "$DEB_PATH"

if [ "${KEEP_BUILD_DIR:-0}" != "1" ]; then
    rm -rf "$DEB_ROOT"
fi

printf 'Built %s\n' "$DEB_PATH"
