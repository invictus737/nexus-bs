#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
# SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0

set -Eeuo pipefail

DEB_PATH="${DEB_PATH:-/tmp/nexus-bs_0.1.72_arm64.deb}"
EXPECTED_SHA256="${EXPECTED_SHA256:-}"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
BACKUP_DIR="/tmp/nexus-bs-reinstall-backup-${STAMP}"
LOG_FILE="/tmp/nexus-bs-reinstall-${STAMP}.log"
RUN_USER="${NEXUS_BS_RUN_USER:-${SUDO_USER:-}}"

log() {
    printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*"
}

require_root() {
    if [ "$(id -u)" -ne 0 ]; then
        printf 'Run with sudo: sudo bash %s\n' "$0" >&2
        exit 1
    fi
}

main() {
    require_root
    exec > >(tee -a "$LOG_FILE") 2>&1
    if [ -z "$RUN_USER" ] || ! id "$RUN_USER" >/dev/null 2>&1; then
        RUN_USER="$(awk -F: '($3 >= 1000 && $3 < 60000 && $1 != "nobody") { print $1; exit }' /etc/passwd)"
    fi
    RUN_GROUP="$(id -gn "$RUN_USER" 2>/dev/null || printf '%s' "$RUN_USER")"

    log "Starting Nexus-BS clean reinstall"
    log "Package: ${DEB_PATH}"
    log "Runtime user: ${RUN_USER}:${RUN_GROUP}"
    test -f "$DEB_PATH"

    actual_sha="$(sha256sum "$DEB_PATH" | awk '{print $1}')"
    log "Package sha256: ${actual_sha}"
    if [ -n "$EXPECTED_SHA256" ] && [ "$actual_sha" != "$EXPECTED_SHA256" ]; then
        log "ERROR: package checksum mismatch, expected ${EXPECTED_SHA256}"
        exit 1
    fi

    install -d -o root -g root -m 0700 "$BACKUP_DIR"
    if [ -e /etc/nexus-bs/config.toml ]; then
        install -o root -g root -m 0600 /etc/nexus-bs/config.toml "$BACKUP_DIR/config.toml"
        log "Backed up /etc/nexus-bs/config.toml"
    fi

    log "Stopping services"
    systemctl stop nexus-bs-dashboard.service nexus-bs.service nexus-bs-control.service 2>/dev/null || true
    systemctl disable nexus-bs-dashboard.service nexus-bs.service nexus-bs-control.service 2>/dev/null || true

    log "Purging package"
    apt-get purge -y nexus-bs || true

    log "Removing old runtime files"
    rm -rf /etc/nexus-bs /opt/nexus-bs /run/nexus-bs
    rm -f /usr/bin/nexus-bs-service
    rm -f /lib/systemd/system/nexus-bs.service
    rm -f /lib/systemd/system/nexus-bs-control.service
    rm -f /lib/systemd/system/nexus-bs-dashboard.service
    rm -f /usr/lib/systemd/system/nexus-bs.service
    rm -f /usr/lib/systemd/system/nexus-bs-control.service
    rm -f /usr/lib/systemd/system/nexus-bs-dashboard.service
    rm -f /etc/systemd/system/nexus-bs.service
    rm -f /etc/systemd/system/nexus-bs-control.service
    rm -f /etc/systemd/system/nexus-bs-dashboard.service
    rm -f /etc/systemd/system/multi-user.target.wants/nexus-bs.service
    rm -f /etc/systemd/system/multi-user.target.wants/nexus-bs-control.service
    rm -f /etc/systemd/system/multi-user.target.wants/nexus-bs-dashboard.service
    systemctl daemon-reload
    systemctl reset-failed nexus-bs-dashboard.service nexus-bs.service nexus-bs-control.service 2>/dev/null || true

    log "Installing package"
    apt-get install -y "$DEB_PATH"

    log "Restoring live config if backup exists"
    install -d -o "$RUN_USER" -g "$RUN_GROUP" -m 0750 /etc/nexus-bs
    if [ -f "$BACKUP_DIR/config.toml" ]; then
        install -o "$RUN_USER" -g "$RUN_GROUP" -m 0600 "$BACKUP_DIR/config.toml" /etc/nexus-bs/config.toml
    fi
    chown -R "$RUN_USER:$RUN_GROUP" /etc/nexus-bs
    find /etc/nexus-bs -type d -exec chmod 0750 {} +
    find /etc/nexus-bs -type f -name '*.toml' -exec chmod 0600 {} +
    find /etc/nexus-bs -type f -name '*.toml.active' -exec chmod 0600 {} + 2>/dev/null || true

    log "Starting Nexus-BS services"
    nexus-bs-service start
    sleep 10

    log "Service status"
    systemctl --no-pager --full status nexus-bs-control.service nexus-bs.service nexus-bs-dashboard.service || true

    log "Recent logs"
    journalctl -u nexus-bs-control.service -u nexus-bs.service -u nexus-bs-dashboard.service -n 180 --no-pager || true

    log "Config permissions"
    ls -ld /etc/nexus-bs
    ls -l /etc/nexus-bs/config.toml

    log "Reinstall complete. Log saved at ${LOG_FILE}; backup at ${BACKUP_DIR}"
}

main "$@"
