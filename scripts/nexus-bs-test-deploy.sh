#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

REMOTE="${REMOTE:-chris@192.168.1.179}"
REMOTE_BASE="${REMOTE_BASE:-/home/chris/nexus-bs-v0.1.55-test}"
BIN="$ROOT/target/aarch64-unknown-linux-gnu/release/nexus-bs"
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
    cargo zigbuild --release -p nexus-bs --target aarch64-unknown-linux-gnu --locked --bin nexus-bs

local_sha="$(shasum -a 256 "$BIN" | awk '{print $1}')"
commit="$(git rev-parse --short=8 HEAD)"

ssh $SSH_OPTS "$REMOTE" "timeout 12s sh -lc '
for pidfile in \"$REMOTE_BASE/nexus-bs.pid\" \"$REMOTE_BASE/control.pid\"; do
    if [ -f \"\$pidfile\" ]; then
        pid=\$(cat \"\$pidfile\" 2>/dev/null || true)
        if [ -n \"\$pid\" ]; then
            kill -TERM -\"\$pid\" 2>/dev/null || kill -TERM \"\$pid\" 2>/dev/null || true
        fi
        rm -f \"\$pidfile\"
    fi
done
sleep 1
'"

scp $SSH_OPTS "$BIN" "$REMOTE:$REMOTE_BASE/bin/nexus-bs"

remote_sha="$(ssh $SSH_OPTS "$REMOTE" "timeout 10s sha256sum '$REMOTE_BASE/bin/nexus-bs' | awk '{print \$1}'")"
if [ "$remote_sha" != "$local_sha" ]; then
    echo "remote sha mismatch: local=$local_sha remote=$remote_sha" >&2
    exit 1
fi

ssh $SSH_OPTS "$REMOTE" "timeout 15s '$REMOTE_BASE/start-test.sh'"
sleep "${POST_START_SLEEP:-8}"

ssh $SSH_OPTS "$REMOTE" "timeout 10s sh -lc '
echo deployed_commit=$commit
echo deployed_sha=$remote_sha
pgrep -af nexus-bs
grep -aE \"Build:|subscriber register|subscriber affiliate|rejecting colliding|RequestedServiceNotAvailable|Service unavailable|PTT denied|Unit Not Attached|mapping repeated U-SETUP|U-SETUP|U-TX DEMAND|D-TX GRANTED|FloorGranted|UMAC floor\" \"$REMOTE_BASE/nexus-bs.log\" | tail -n 260 || true
'"
