#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

REMOTE="${REMOTE:-chris@192.168.1.179}"
REMOTE_BASE="${REMOTE_BASE:-/home/chris/nexus-bs}"
REMOTE_SERVICE="${REMOTE_SERVICE:-nexus-bs@chris.service}"
REMOTE_CONTROL_SERVICE="${REMOTE_CONTROL_SERVICE:-nexus-bs-control@chris.service}"
BIN="$ROOT/target/aarch64-unknown-linux-gnu/release/nexus-bs"
CONTROL_SERVICE_BIN="$ROOT/target/aarch64-unknown-linux-gnu/release/nexus-bs-control-service"
SSH_OPTS="-o BatchMode=yes -o ConnectTimeout=5 -o ServerAliveInterval=2 -o ServerAliveCountMax=2"

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
    cargo zigbuild --release -p nexus-bs -p nexus-bs-control --target aarch64-unknown-linux-gnu --locked --bins

local_sha="$(shasum -a 256 "$BIN" | awk '{print $1}')"
control_service_sha="$(shasum -a 256 "$CONTROL_SERVICE_BIN" | awk '{print $1}')"
commit="$(git rev-parse --short=8 HEAD)"

ssh $SSH_OPTS "$REMOTE" "timeout 12s sh -lc '
sudo -n systemctl stop \"$REMOTE_SERVICE\"
sudo -n systemctl stop \"$REMOTE_CONTROL_SERVICE\"
'"

ssh $SSH_OPTS "$REMOTE" "timeout 10s sh -lc '
mkdir -p \"$REMOTE_BASE/dashboard/assets\"
'"
scp $SSH_OPTS "$BIN" "$REMOTE:$REMOTE_BASE/nexus-bs"
scp $SSH_OPTS "$CONTROL_SERVICE_BIN" "$REMOTE:$REMOTE_BASE/nexus-bs-control-service"
scp $SSH_OPTS dashboard/index.html "$REMOTE:$REMOTE_BASE/dashboard/index.html"
scp $SSH_OPTS dashboard/assets/app.js "$REMOTE:$REMOTE_BASE/dashboard/assets/app.js"
scp $SSH_OPTS dashboard/assets/styles.css "$REMOTE:$REMOTE_BASE/dashboard/assets/styles.css"

remote_sha="$(ssh $SSH_OPTS "$REMOTE" "timeout 10s sha256sum '$REMOTE_BASE/nexus-bs' | awk '{print \$1}'")"
remote_control_service_sha="$(ssh $SSH_OPTS "$REMOTE" "timeout 10s sha256sum '$REMOTE_BASE/nexus-bs-control-service' | awk '{print \$1}'")"
if [ "$remote_sha" != "$local_sha" ]; then
    echo "remote sha mismatch: local=$local_sha remote=$remote_sha" >&2
    exit 1
fi
if [ "$remote_control_service_sha" != "$control_service_sha" ]; then
    echo "remote control service sha mismatch: local=$control_service_sha remote=$remote_control_service_sha" >&2
    exit 1
fi

ssh $SSH_OPTS "$REMOTE" "timeout 15s sh -lc '
chmod 755 \"$REMOTE_BASE/nexus-bs\"
chmod 755 \"$REMOTE_BASE/nexus-bs-control-service\"
sudo -n systemctl restart --no-block \"$REMOTE_CONTROL_SERVICE\"
sudo -n systemctl restart --no-block \"$REMOTE_SERVICE\"
'"
sleep "${POST_START_SLEEP:-8}"

ssh $SSH_OPTS "$REMOTE" "timeout 10s sh -lc '
echo deployed_commit=$commit
echo deployed_sha=$remote_sha
echo deployed_control_service_sha=$remote_control_service_sha
systemctl show \"$REMOTE_SERVICE\" -p ActiveState -p SubState -p ActiveEnterTimestamp -p MainPID --no-pager
systemctl show \"$REMOTE_CONTROL_SERVICE\" -p ActiveState -p SubState -p ActiveEnterTimestamp -p MainPID --no-pager
journalctl -u \"$REMOTE_SERVICE\" --since \"2 minutes ago\" --no-pager | tail -n 260
'"
