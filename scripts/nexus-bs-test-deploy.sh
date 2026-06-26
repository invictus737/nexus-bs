#!/bin/sh
# SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
# SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0

set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

REMOTE="${REMOTE:-chris@192.168.1.179}"
REMOTE_BASE="${REMOTE_BASE:-/home/chris/nexus-bs}"
INSTALL_PREFIX="${INSTALL_PREFIX:-/opt/nexus-bs}"
REMOTE_SERVICE="${REMOTE_SERVICE:-nexus-bs.service}"
REMOTE_CONTROL_SERVICE="${REMOTE_CONTROL_SERVICE:-nexus-bs-control.service}"
REMOTE_DASHBOARD_SERVICE="${REMOTE_DASHBOARD_SERVICE:-nexus-bs-dashboard.service}"
BIN="$ROOT/target/aarch64-unknown-linux-gnu/release/nexus-bs"
CONTROL_SERVICE_BIN="$ROOT/target/aarch64-unknown-linux-gnu/release/nexus-bs-control-service"
DASHBOARD_BIN="$ROOT/target/aarch64-unknown-linux-gnu/release/nexus-bs-dashboard"
SSH_OPTS="-o BatchMode=yes -o ConnectTimeout=5 -o ServerAliveInterval=2 -o ServerAliveCountMax=2"

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

render_systemd_unit() {
    src="$1"
    dst="$2"
    version="$3"
    sed -E "s/Nexus-BS v[0-9]+[.][0-9]+[.][0-9]+/Nexus-BS v${version}/g" "$src" > "$dst"
}

if [ "${RUN_TESTS:-1}" = "1" ]; then
    cargo test -p tetra-entities --test test_cmce_bs repeated_group_u_setup --locked
    cargo test -p tetra-entities --test test_cmce_bs --locked
    cargo test -p tetra-entities --test test_umac_bs --locked
    cargo test -p tetra-entities --test test_mm_bs restart_recovery --locked
    cargo check -p tetra-entities --locked
    git diff --check
fi

env \
    PKG_CONFIG_ALLOW_CROSS=1 \
    PKG_CONFIG_LIBDIR=/private/tmp/soapy-aarch64/lib/pkgconfig \
    PKG_CONFIG_PATH=/private/tmp/soapy-aarch64/lib/pkgconfig \
    LIBRARY_PATH=/private/tmp/soapy-aarch64/lib \
    ZIG_GLOBAL_CACHE_DIR=/private/tmp/nexushs-zig-cache \
    ZIG_LOCAL_CACHE_DIR=/private/tmp/nexushs-zig-local \
    cargo zigbuild --release -p nexus-bs -p nexus-bs-control -p nexus-bs-dashboard --target aarch64-unknown-linux-gnu --locked --bins

local_sha="$(shasum -a 256 "$BIN" | awk '{print $1}')"
control_service_sha="$(shasum -a 256 "$CONTROL_SERVICE_BIN" | awk '{print $1}')"
dashboard_sha="$(shasum -a 256 "$DASHBOARD_BIN" | awk '{print $1}')"
commit="$(git rev-parse --short=8 HEAD)"
version="$(project_version)"
unit_dir="$(mktemp -d "${TMPDIR:-/tmp}/nexus-bs-units.XXXXXX")"
trap 'rm -rf "$unit_dir"' EXIT INT TERM
render_systemd_unit contrib/systemd/nexus-bs.service "$unit_dir/nexus-bs.service" "$version"
render_systemd_unit contrib/systemd/nexus-bs-control.service "$unit_dir/nexus-bs-control.service" "$version"
render_systemd_unit contrib/systemd/nexus-bs-dashboard.service "$unit_dir/nexus-bs-dashboard.service" "$version"

ssh $SSH_OPTS "$REMOTE" "timeout 12s sh -lc '
sudo -n systemctl daemon-reload || true
sudo -n systemctl stop \"$REMOTE_DASHBOARD_SERVICE\" || true
sudo -n systemctl stop \"$REMOTE_SERVICE\"
sudo -n systemctl stop \"$REMOTE_CONTROL_SERVICE\"
'"

ssh $SSH_OPTS "$REMOTE" "timeout 10s sh -lc '
mkdir -p \"$REMOTE_BASE/dashboard/assets\"
'"
scp $SSH_OPTS "$BIN" "$REMOTE:$REMOTE_BASE/nexus-bs"
scp $SSH_OPTS "$CONTROL_SERVICE_BIN" "$REMOTE:$REMOTE_BASE/nexus-bs-control-service"
scp $SSH_OPTS "$DASHBOARD_BIN" "$REMOTE:$REMOTE_BASE/nexus-bs-dashboard"
scp $SSH_OPTS dashboard/index.html "$REMOTE:$REMOTE_BASE/dashboard/index.html"
scp $SSH_OPTS dashboard/assets/app.js "$REMOTE:$REMOTE_BASE/dashboard/assets/app.js"
scp $SSH_OPTS dashboard/assets/styles.css "$REMOTE:$REMOTE_BASE/dashboard/assets/styles.css"
scp $SSH_OPTS dashboard/assets/nexus-bs-logo.svg "$REMOTE:$REMOTE_BASE/dashboard/assets/nexus-bs-logo.svg"
scp $SSH_OPTS scripts/nexus-bs-factory-reset-clean "$REMOTE:$REMOTE_BASE/nexus-bs-factory-reset-clean"
scp $SSH_OPTS "$unit_dir/nexus-bs.service" "$REMOTE:/tmp/nexus-bs.service"
scp $SSH_OPTS "$unit_dir/nexus-bs-control.service" "$REMOTE:/tmp/nexus-bs-control.service"
scp $SSH_OPTS "$unit_dir/nexus-bs-dashboard.service" "$REMOTE:/tmp/nexus-bs-dashboard.service"

remote_sha="$(ssh $SSH_OPTS "$REMOTE" "timeout 10s sha256sum '$REMOTE_BASE/nexus-bs' | awk '{print \$1}'")"
remote_control_service_sha="$(ssh $SSH_OPTS "$REMOTE" "timeout 10s sha256sum '$REMOTE_BASE/nexus-bs-control-service' | awk '{print \$1}'")"
remote_dashboard_sha="$(ssh $SSH_OPTS "$REMOTE" "timeout 10s sha256sum '$REMOTE_BASE/nexus-bs-dashboard' | awk '{print \$1}'")"
if [ "$remote_sha" != "$local_sha" ]; then
    echo "remote sha mismatch: local=$local_sha remote=$remote_sha" >&2
    exit 1
fi
if [ "$remote_control_service_sha" != "$control_service_sha" ]; then
    echo "remote control service sha mismatch: local=$control_service_sha remote=$remote_control_service_sha" >&2
    exit 1
fi
if [ "$remote_dashboard_sha" != "$dashboard_sha" ]; then
    echo "remote dashboard sha mismatch: local=$dashboard_sha remote=$remote_dashboard_sha" >&2
    exit 1
fi

ssh $SSH_OPTS "$REMOTE" "timeout 15s sh -lc '
chmod 755 \"$REMOTE_BASE/nexus-bs\"
chmod 755 \"$REMOTE_BASE/nexus-bs-control-service\"
chmod 755 \"$REMOTE_BASE/nexus-bs-dashboard\"
chmod 755 \"$REMOTE_BASE/nexus-bs-factory-reset-clean\"
sudo -n install -d -m 0755 \"$INSTALL_PREFIX/bin\" \"$INSTALL_PREFIX/dashboard/assets\"
sudo -n install -m 0755 \"$REMOTE_BASE/nexus-bs\" \"$INSTALL_PREFIX/bin/nexus-bs\"
sudo -n install -m 0755 \"$REMOTE_BASE/nexus-bs-control-service\" \"$INSTALL_PREFIX/bin/nexus-bs-control-service\"
sudo -n install -m 0755 \"$REMOTE_BASE/nexus-bs-dashboard\" \"$INSTALL_PREFIX/bin/nexus-bs-dashboard\"
sudo -n install -m 0755 \"$REMOTE_BASE/nexus-bs-factory-reset-clean\" \"$INSTALL_PREFIX/bin/nexus-bs-factory-reset-clean\"
sudo -n install -m 0644 \"$REMOTE_BASE/dashboard/index.html\" \"$INSTALL_PREFIX/dashboard/index.html\"
sudo -n install -m 0644 \"$REMOTE_BASE/dashboard/assets/app.js\" \"$INSTALL_PREFIX/dashboard/assets/app.js\"
sudo -n install -m 0644 \"$REMOTE_BASE/dashboard/assets/styles.css\" \"$INSTALL_PREFIX/dashboard/assets/styles.css\"
sudo -n install -m 0644 \"$REMOTE_BASE/dashboard/assets/nexus-bs-logo.svg\" \"$INSTALL_PREFIX/dashboard/assets/nexus-bs-logo.svg\"
sudo -n install -m 0644 /tmp/nexus-bs.service /etc/systemd/system/nexus-bs.service
sudo -n install -m 0644 /tmp/nexus-bs-control.service /etc/systemd/system/nexus-bs-control.service
sudo -n install -m 0644 /tmp/nexus-bs-dashboard.service /etc/systemd/system/nexus-bs-dashboard.service
for cfg in /etc/nexus-bs/config.toml /etc/nexus-bs/config.toml.fallback; do
    if [ -f \"\$cfg\" ]; then
        sudo -n sed -i -E \"s/^[[:space:]]*service_name[[:space:]]*=.*/service_name = \\\"nexus-bs.service\\\"/\" \"\$cfg\"
        if ! sudo -n grep -q \"^\\[telemetry\\]\" \"\$cfg\"; then
            printf \"\\n[telemetry]\\nhost = \\\"127.0.0.1\\\"\\nport = 9001\\nuse_tls = false\\n\" | sudo -n tee -a \"\$cfg\" >/dev/null
        fi
    fi
done
sudo -n systemctl daemon-reload
sudo -n systemctl restart --no-block \"$REMOTE_CONTROL_SERVICE\"
sudo -n systemctl restart --no-block \"$REMOTE_SERVICE\"
sudo -n systemctl restart --no-block \"$REMOTE_DASHBOARD_SERVICE\"
'"
sleep "${POST_START_SLEEP:-8}"

ssh $SSH_OPTS "$REMOTE" "timeout 10s sh -lc '
echo deployed_commit=$commit
echo deployed_sha=$remote_sha
echo deployed_control_service_sha=$remote_control_service_sha
echo deployed_dashboard_sha=$remote_dashboard_sha
systemctl show \"$REMOTE_SERVICE\" -p ActiveState -p SubState -p ActiveEnterTimestamp -p MainPID --no-pager
systemctl show \"$REMOTE_CONTROL_SERVICE\" -p ActiveState -p SubState -p ActiveEnterTimestamp -p MainPID --no-pager
systemctl show \"$REMOTE_DASHBOARD_SERVICE\" -p ActiveState -p SubState -p ActiveEnterTimestamp -p MainPID --no-pager
journalctl -u \"$REMOTE_SERVICE\" --since \"2 minutes ago\" --no-pager | tail -n 260
journalctl -u \"$REMOTE_DASHBOARD_SERVICE\" --since \"2 minutes ago\" --no-pager | tail -n 80
'"
