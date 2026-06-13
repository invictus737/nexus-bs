#!/bin/sh
# SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
# SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0

set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

tmp_dir="${TMPDIR:-/tmp}/nexus-bs-license-audit.$$"
mkdir -p "$tmp_dir"
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

files_file="$tmp_dir/files"
reuse_paths_file="$tmp_dir/reuse-paths"
missing_file="$tmp_dir/missing"
invalid_file="$tmp_dir/invalid"
forbidden_file="$tmp_dir/forbidden"
public_audit="${NEXUS_BS_PUBLIC_AUDIT:-0}"

: >"$files_file"
: >"$reuse_paths_file"
: >"$missing_file"
: >"$invalid_file"
: >"$forbidden_file"

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

append_forbidden() {
    printf '%s\t%s\n' "$1" "$2" >>"$forbidden_file"
}

is_local_private_file() {
    case "$1" in
        AGENTS.md|example_trace.txt|MISSION_READINESS.md|NEXUS_BS_FLOWSTATION_DELTA_REPORT.md|timeline.md)
            return 0
            ;;
    esac
    return 1
}

if [ ! -f REUSE.toml ]; then
    die "REUSE.toml is missing"
fi

if ! grep -Eq '^[[:space:]]*version[[:space:]]*=[[:space:]]*1[[:space:]]*$' REUSE.toml; then
    printf '%s\n' "REUSE.toml: missing 'version = 1'" >>"$invalid_file"
fi

awk '
function emit_quoted(line, value) {
    while (match(line, /"([^"\\]|\\.)*"/)) {
        value = substr(line, RSTART + 1, RLENGTH - 2)
        gsub(/\\"/, "\"", value)
        print value
        line = substr(line, RSTART + RLENGTH)
    }
}

{
    line = $0
    sub(/[[:space:]]+#.*/, "", line)
    if (!in_path && line ~ /^[[:space:]]*path[[:space:]]*=/) {
        in_path = 1
        path_is_array = (line ~ /\[/)
    }
    if (in_path) {
        emit_quoted(line)
        if (!path_is_array || line ~ /\]/) {
            in_path = 0
            path_is_array = 0
        }
    }
}
' REUSE.toml >"$reuse_paths_file"

if [ ! -s "$reuse_paths_file" ]; then
    printf '%s\n' "REUSE.toml: no annotation paths found" >>"$invalid_file"
fi

awk -F '"' '
/^[[:space:]]*SPDX-License-Identifier[[:space:]]*=/ {
    expr = $2
    if (expr != "Apache-2.0" &&
        expr != "PolyForm-Noncommercial-1.0.0" &&
        expr != "Apache-2.0 AND PolyForm-Noncommercial-1.0.0" &&
        expr != "LicenseRef-ThirdParty-ETSI-Standards") {
        print "REUSE.toml: unsupported SPDX expression: " expr
    }
}
' REUSE.toml >>"$invalid_file"

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git ls-files -co --exclude-standard | sort -u >"$files_file"
else
    find . \
        -path './.git' -prune -o \
        -path './target' -prune -o \
        -path './.private_backups' -prune -o \
        -path './packaging/deb/dist' -prune -o \
        -type f -print |
        sed 's#^\./##' |
        sort -u >"$files_file"
fi

has_inline_spdx() {
    LC_ALL=C grep -q 'SPDX-License-Identifier:' "$1" 2>/dev/null
}

covered_by_reuse() {
    file="$1"
    while IFS= read -r pattern; do
        [ -n "$pattern" ] || continue
        case "$file" in
            $pattern)
                return 0
                ;;
        esac
    done <"$reuse_paths_file"
    return 1
}

checked_count=0
while IFS= read -r file; do
    [ -n "$file" ] || continue
    [ -f "$file" ] || continue
    if is_local_private_file "$file"; then
        continue
    fi
    checked_count=$((checked_count + 1))

    if has_inline_spdx "$file"; then
        continue
    fi
    if covered_by_reuse "$file"; then
        continue
    fi

    printf '%s\n' "$file" >>"$missing_file"
done <"$files_file"

if [ "$public_audit" = "1" ]; then
    if [ -d .private_backups ]; then
        append_forbidden ".private_backups/" "private backup directory must not be published"
    fi

    if [ -d packaging/deb/dist ]; then
        append_forbidden "packaging/deb/dist/" "generated package output must not be published from the workspace"
    fi

    for private_file in AGENTS.md example_trace.txt MISSION_READINESS.md NEXUS_BS_FLOWSTATION_DELTA_REPORT.md timeline.md; do
        if [ -e "$private_file" ]; then
            append_forbidden "$private_file" "local-private file must not be published"
        fi
    done

    find . \
        -path './.git' -prune -o \
        -path './target' -prune -o \
        -path './.private_backups' -prune -o \
        -path './packaging/deb/dist' -prune -o \
        -name .DS_Store -print |
        sed 's#^\./##' |
        while IFS= read -r artifact; do
            [ -n "$artifact" ] || continue
            append_forbidden "$artifact" "macOS metadata artifact must not be published"
        done

    find . \
        -path './.git' -prune -o \
        -path './target' -prune -o \
        -path './.private_backups' -prune -o \
        -path './packaging/deb/dist' -prune -o \
        -type f \( \
            -name '.env' -o \
            -name '.env.*' -o \
            -name '*.pem' -o \
            -name '*.key' -o \
            -name '*.p12' -o \
            -name '*.pfx' -o \
            -name 'id_rsa' -o \
            -name 'id_ed25519' -o \
            -iname '*secret*' -o \
            -iname '*credential*' \
        \) -print |
        sed 's#^\./##' |
        while IFS= read -r private_file; do
            [ -n "$private_file" ] || continue
            append_forbidden "$private_file" "private credential-like file must not be published"
        done

    find . \
        -path './.git' -prune -o \
        -path './target' -prune -o \
        -path './.private_backups' -prune -o \
        -path './packaging/deb/dist' -prune -o \
        -type f \( \
            -name '*.deb' -o \
            -name '*.rpm' -o \
            -name '*.pkg' -o \
            -name '*.tar' -o \
            -name '*.tar.gz' -o \
            -name '*.tgz' -o \
            -name '*.zip' -o \
            -name '*.7z' -o \
            -name '*.dmg' -o \
            -name '*.apk' \
        \) -print |
        sed 's#^\./##' |
        while IFS= read -r published_file; do
            [ -n "$published_file" ] || continue
            append_forbidden "$published_file" "published package/archive artifact must not live in the workspace"
        done
fi

invalid_count="$(wc -l <"$invalid_file" | tr -d ' ')"
missing_count="$(wc -l <"$missing_file" | tr -d ' ')"
forbidden_count="$(wc -l <"$forbidden_file" | tr -d ' ')"

if [ "$invalid_count" -ne 0 ]; then
    printf 'Invalid license metadata:\n' >&2
    sed 's/^/  /' "$invalid_file" >&2
fi

if [ "$missing_count" -ne 0 ]; then
    printf 'Files missing SPDX-License-Identifier or REUSE.toml coverage:\n' >&2
    sed 's/^/  /' "$missing_file" >&2
fi

if [ "$forbidden_count" -ne 0 ]; then
    printf 'Forbidden private/published artifacts found:\n' >&2
    sed 's/^/  /' "$forbidden_file" >&2
fi

if [ "$invalid_count" -ne 0 ] || [ "$missing_count" -ne 0 ] || [ "$forbidden_count" -ne 0 ]; then
    exit 1
fi

printf 'License audit passed: checked %s files for inline SPDX or REUSE.toml coverage.\n' "$checked_count"
