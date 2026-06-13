#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
# SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0

set -euo pipefail

SCRIPT_NAME="$(basename "$0")"

usage() {
    cat <<'USAGE'
Usage:
  generate-apt-repo.sh --output OUT_DIR [options] DEB [DEB ...]
  generate-apt-repo.sh --output OUT_DIR [options] --input-dir DEB_DIR

Create a simple static APT repository for publishing on GitHub Pages or any
plain web server.

Required:
  -o, --output OUT_DIR       Repository output directory.

Inputs:
  DEB                        One or more .deb files may be passed directly.
      --input-dir DIR        Add regular *.deb files directly inside DIR.
                             May be repeated.

Options:
  -s, --suite NAME           Distribution suite/codename path. Default: stable.
  -c, --component NAME       Repository component. Default: main.
  -a, --architecture ARCH    Architecture index to generate. May be repeated.
                             Default: auto-detect from .deb metadata.
      --origin NAME          Release Origin. Default: Nexus-BS.
      --label NAME           Release Label. Default: Nexus-BS.
      --codename NAME        Release Codename. Default: suite value.
      --description TEXT     Release Description.
      --no-clean             Keep existing output directory contents.
      --repo-url URL         Print a usable apt source line with this base URL.
  -h, --help                 Show this help.

Signing:
  If GPG_KEY is set, the script signs dists/SUITE/Release and creates both
  dists/SUITE/InRelease and dists/SUITE/Release.gpg with gpg --local-user.

Required tools:
  dpkg-deb, dpkg-scanpackages, gzip. apt-ftparchive and gpg are optional unless
  signing is requested.
USAGE
}

die() {
    printf '%s: error: %s\n' "$SCRIPT_NAME" "$*" >&2
    exit 1
}

info() {
    printf '%s\n' "$*"
}

require_tool() {
    local tool="$1"
    local package_hint="${2:-}"

    if ! command -v "$tool" >/dev/null 2>&1; then
        if [[ -n "$package_hint" ]]; then
            die "required tool '$tool' was not found. Install $package_hint and retry."
        fi
        die "required tool '$tool' was not found."
    fi
}

command_exists() {
    command -v "$1" >/dev/null 2>&1
}

abs_path_for_output() {
    local path="$1"
    local parent
    local base

    parent="$(dirname "$path")"
    base="$(basename "$path")"
    mkdir -p "$parent"

    (
        cd "$parent"
        printf '%s/%s\n' "$(pwd -P)" "$base"
    )
}

abs_existing_path() {
    local path="$1"
    local parent
    local base

    parent="$(dirname "$path")"
    base="$(basename "$path")"
    (
        cd "$parent"
        printf '%s/%s\n' "$(pwd -P)" "$base"
    )
}

append_deb() {
    local deb="$1"

    [[ -e "$deb" ]] || die "input does not exist: $deb"
    [[ -f "$deb" ]] || die "input is not a regular file: $deb"
    [[ ! -L "$deb" ]] || die "refusing symlink input to avoid copying unintended files: $deb"
    [[ "$deb" == *.deb ]] || die "input is not a .deb file: $deb"

    debs+=("$(abs_existing_path "$deb")")
}

add_input_dir() {
    local dir="$1"
    local found=0
    local deb

    [[ -d "$dir" ]] || die "input directory does not exist: $dir"

    while IFS= read -r -d '' deb; do
        append_deb "$deb"
        found=1
    done < <(find "$dir" -maxdepth 1 -type f -name '*.deb' -print0 | sort -z)

    [[ "$found" -eq 1 ]] || die "input directory contains no regular .deb files: $dir"
}

normalize_list() {
    local value="$1"

    value="${value//,/ }"
    # shellcheck disable=SC2086
    printf '%s\n' $value
}

array_contains() {
    local needle="$1"
    local value

    shift
    for value in "$@"; do
        [[ "$value" == "$needle" ]] && return 0
    done
    return 1
}

detect_deb_architecture() {
    local deb="$1"
    local arch

    if ! arch="$(dpkg-deb -f "$deb" Architecture 2>/dev/null)"; then
        die "failed to read Architecture field from: $deb"
    fi

    [[ -n "$arch" ]] || die "missing Architecture field in: $deb"
    printf '%s\n' "$arch"
}

detect_architectures() {
    local deb
    local arch
    local -a detected=()
    local saw_all=0

    for deb in "${debs[@]}"; do
        arch="$(detect_deb_architecture "$deb")"
        if [[ "$arch" == "all" ]]; then
            saw_all=1
            continue
        fi
        if ! array_contains "$arch" "${detected[@]}"; then
            detected+=("$arch")
        fi
    done

    if [[ "${#detected[@]}" -eq 0 && "$saw_all" -eq 1 ]]; then
        detected+=("all")
    fi

    printf '%s\n' "${detected[@]}"
}

hash_file() {
    local alg="$1"
    local file="$2"

    case "$alg" in
        md5)
            if command_exists md5sum; then
                md5sum "$file" | awk '{print $1}'
            elif command_exists md5; then
                md5 -q "$file"
            elif command_exists openssl; then
                openssl dgst -r -md5 "$file" | awk '{print $1}'
            else
                die "cannot compute MD5 checksums; install md5sum, md5, or openssl"
            fi
            ;;
        sha1)
            if command_exists sha1sum; then
                sha1sum "$file" | awk '{print $1}'
            elif command_exists shasum; then
                shasum -a 1 "$file" | awk '{print $1}'
            elif command_exists openssl; then
                openssl dgst -r -sha1 "$file" | awk '{print $1}'
            else
                die "cannot compute SHA1 checksums; install sha1sum, shasum, or openssl"
            fi
            ;;
        sha256)
            if command_exists sha256sum; then
                sha256sum "$file" | awk '{print $1}'
            elif command_exists shasum; then
                shasum -a 256 "$file" | awk '{print $1}'
            elif command_exists openssl; then
                openssl dgst -r -sha256 "$file" | awk '{print $1}'
            else
                die "cannot compute SHA256 checksums; install sha256sum, shasum, or openssl"
            fi
            ;;
        *)
            die "unsupported checksum algorithm: $alg"
            ;;
    esac
}

file_size() {
    local file="$1"

    if command_exists stat; then
        if stat -c '%s' "$file" >/dev/null 2>&1; then
            stat -c '%s' "$file"
            return
        fi
        if stat -f '%z' "$file" >/dev/null 2>&1; then
            stat -f '%z' "$file"
            return
        fi
    fi

    wc -c < "$file" | tr -d '[:space:]'
}

path_is_inside() {
    local child="$1"
    local parent="$2"

    [[ "$child" == "$parent" || "$child" == "$parent"/* ]]
}

assert_safe_clean_target() {
    local path="$1"
    local deb

    case "$path" in
        /|/tmp|/private/tmp|/var|/private/var|"$HOME")
            die "refusing to clean unsafe output directory: $path"
            ;;
    esac

    for deb in "${debs[@]}"; do
        if path_is_inside "$deb" "$path"; then
            die "refusing to clean output directory because it contains an input package: $deb"
        fi
    done
}

release_date_utc() {
    if date -u '+%a, %d %b %Y %H:%M:%S UTC' >/dev/null 2>&1; then
        date -u '+%a, %d %b %Y %H:%M:%S UTC'
    else
        date
    fi
}

write_release_fallback() {
    local release_file="$1"
    local dist_dir="$2"
    local architectures="$3"
    local relative
    local file
    local size
    local digest
    local -a index_files=()

    while IFS= read -r -d '' file; do
        relative="${file#"$dist_dir"/}"
        case "$relative" in
            Release|Release.gpg|InRelease)
                continue
                ;;
        esac
        index_files+=("$relative")
    done < <(find "$dist_dir" -type f -print0 | sort -z)

    {
        printf 'Origin: %s\n' "$origin"
        printf 'Label: %s\n' "$label"
        printf 'Suite: %s\n' "$suite"
        printf 'Codename: %s\n' "$codename"
        printf 'Date: %s\n' "$(release_date_utc)"
        printf 'Architectures: %s\n' "$architectures"
        printf 'Components: %s\n' "$component"
        printf 'Description: %s\n' "$description"
        printf 'MD5Sum:\n'
        for relative in "${index_files[@]}"; do
            file="$dist_dir/$relative"
            digest="$(hash_file md5 "$file")"
            size="$(file_size "$file")"
            printf ' %s %16s %s\n' "$digest" "$size" "$relative"
        done
        printf 'SHA1:\n'
        for relative in "${index_files[@]}"; do
            file="$dist_dir/$relative"
            digest="$(hash_file sha1 "$file")"
            size="$(file_size "$file")"
            printf ' %s %16s %s\n' "$digest" "$size" "$relative"
        done
        printf 'SHA256:\n'
        for relative in "${index_files[@]}"; do
            file="$dist_dir/$relative"
            digest="$(hash_file sha256 "$file")"
            size="$(file_size "$file")"
            printf ' %s %16s %s\n' "$digest" "$size" "$relative"
        done
    } > "$release_file"
}

sign_release() {
    local dist_dir="$1"
    local release_file="$dist_dir/Release"

    [[ -n "${GPG_KEY:-}" ]] || return 0
    require_tool gpg "gnupg"

    rm -f "$dist_dir/InRelease" "$dist_dir/Release.gpg"

    if ! gpg --batch --yes --local-user "$GPG_KEY" --output "$dist_dir/InRelease" --clearsign "$release_file"; then
        die "failed to create InRelease with GPG_KEY='$GPG_KEY'"
    fi

    if ! gpg --batch --yes --local-user "$GPG_KEY" --output "$dist_dir/Release.gpg" --detach-sign "$release_file"; then
        die "failed to create Release.gpg with GPG_KEY='$GPG_KEY'"
    fi
}

output_dir=""
suite="${SUITE:-stable}"
component="${COMPONENT:-main}"
origin="${ORIGIN:-Nexus-BS}"
label="${LABEL:-Nexus-BS}"
codename="${CODENAME:-}"
description="${DESCRIPTION:-Nexus-BS static APT repository}"
repo_url="${REPO_URL:-}"
clean_output=1
declare -a debs=()
declare -a input_dirs=()
declare -a requested_arches=()

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        -o|--output)
            [[ "$#" -ge 2 ]] || die "$1 requires a value"
            output_dir="$2"
            shift 2
            ;;
        --output=*)
            output_dir="${1#*=}"
            shift
            ;;
        --input-dir)
            [[ "$#" -ge 2 ]] || die "$1 requires a value"
            input_dirs+=("$2")
            shift 2
            ;;
        --input-dir=*)
            input_dirs+=("${1#*=}")
            shift
            ;;
        -s|--suite)
            [[ "$#" -ge 2 ]] || die "$1 requires a value"
            suite="$2"
            shift 2
            ;;
        --suite=*)
            suite="${1#*=}"
            shift
            ;;
        -c|--component)
            [[ "$#" -ge 2 ]] || die "$1 requires a value"
            component="$2"
            shift 2
            ;;
        --component=*)
            component="${1#*=}"
            shift
            ;;
        -a|--architecture|--arch)
            [[ "$#" -ge 2 ]] || die "$1 requires a value"
            while IFS= read -r arch; do
                [[ -n "$arch" ]] && requested_arches+=("$arch")
            done < <(normalize_list "$2")
            shift 2
            ;;
        --architecture=*|--arch=*)
            value="${1#*=}"
            while IFS= read -r arch; do
                [[ -n "$arch" ]] && requested_arches+=("$arch")
            done < <(normalize_list "$value")
            shift
            ;;
        --origin)
            [[ "$#" -ge 2 ]] || die "$1 requires a value"
            origin="$2"
            shift 2
            ;;
        --origin=*)
            origin="${1#*=}"
            shift
            ;;
        --label)
            [[ "$#" -ge 2 ]] || die "$1 requires a value"
            label="$2"
            shift 2
            ;;
        --label=*)
            label="${1#*=}"
            shift
            ;;
        --codename)
            [[ "$#" -ge 2 ]] || die "$1 requires a value"
            codename="$2"
            shift 2
            ;;
        --codename=*)
            codename="${1#*=}"
            shift
            ;;
        --description)
            [[ "$#" -ge 2 ]] || die "$1 requires a value"
            description="$2"
            shift 2
            ;;
        --description=*)
            description="${1#*=}"
            shift
            ;;
        --repo-url)
            [[ "$#" -ge 2 ]] || die "$1 requires a value"
            repo_url="$2"
            shift 2
            ;;
        --repo-url=*)
            repo_url="${1#*=}"
            shift
            ;;
        --no-clean)
            clean_output=0
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            while [[ "$#" -gt 0 ]]; do
                append_deb "$1"
                shift
            done
            ;;
        -*)
            die "unknown option: $1"
            ;;
        *)
            append_deb "$1"
            shift
            ;;
    esac
done

[[ -n "$output_dir" ]] || die "--output is required"
[[ -n "$suite" ]] || die "--suite cannot be empty"
[[ -n "$component" ]] || die "--component cannot be empty"
codename="${codename:-$suite}"

for input_dir in "${input_dirs[@]}"; do
    add_input_dir "$input_dir"
done

[[ "${#debs[@]}" -gt 0 ]] || die "provide at least one .deb file or --input-dir containing .deb files"

require_tool dpkg-deb "dpkg-dev or dpkg"
require_tool dpkg-scanpackages "dpkg-dev"
require_tool gzip "gzip"

output_dir="$(abs_path_for_output "$output_dir")"
dist_dir="$output_dir/dists/$suite"
pool_dir="$output_dir/pool/$component"

if [[ "$clean_output" -eq 1 && -e "$output_dir" ]]; then
    assert_safe_clean_target "$output_dir"
    rm -rf "$output_dir"
fi

mkdir -p "$dist_dir" "$pool_dir"

declare -a copied_debs=()
for deb in "${debs[@]}"; do
    target="$pool_dir/$(basename "$deb")"
    if [[ -e "$target" && "$deb" != "$target" ]]; then
        die "refusing to overwrite duplicate package filename in pool: $(basename "$deb")"
    fi
    cp -p "$deb" "$target"
    copied_debs+=("$target")
done

declare -a arches=()
if [[ "${#requested_arches[@]}" -gt 0 ]]; then
    for arch in "${requested_arches[@]}"; do
        if ! array_contains "$arch" "${arches[@]}"; then
            arches+=("$arch")
        fi
    done
else
    while IFS= read -r arch; do
        [[ -n "$arch" ]] && arches+=("$arch")
    done < <(detect_architectures)
fi

[[ "${#arches[@]}" -gt 0 ]] || die "could not determine architectures; use --architecture"

for arch in "${arches[@]}"; do
    packages_dir="$dist_dir/$component/binary-$arch"
    packages_file="$packages_dir/Packages"
    mkdir -p "$packages_dir"

    (
        cd "$output_dir"
        if ! dpkg-scanpackages --multiversion --arch "$arch" "pool/$component" /dev/null > "$packages_file"; then
            die "dpkg-scanpackages failed for architecture '$arch'"
        fi
    )

    if [[ ! -s "$packages_file" ]]; then
        die "generated empty Packages for architecture '$arch'. Check .deb Architecture fields or pass a matching --architecture."
    fi

    gzip -9 -n -c "$packages_file" > "$packages_file.gz"
done

architectures="${arches[*]}"
release_file="$dist_dir/Release"

if command_exists apt-ftparchive; then
    (
        cd "$output_dir"
        apt-ftparchive \
            -o "APT::FTPArchive::Release::Origin=$origin" \
            -o "APT::FTPArchive::Release::Label=$label" \
            -o "APT::FTPArchive::Release::Suite=$suite" \
            -o "APT::FTPArchive::Release::Codename=$codename" \
            -o "APT::FTPArchive::Release::Architectures=$architectures" \
            -o "APT::FTPArchive::Release::Components=$component" \
            -o "APT::FTPArchive::Release::Description=$description" \
            release "dists/$suite" > "$release_file"
    )
else
    info "apt-ftparchive not found; writing minimal Release file with local checksum tools." >&2
    write_release_fallback "$release_file" "$dist_dir" "$architectures"
fi

sign_release "$dist_dir"

info "APT repository generated:"
info "  root: $output_dir"
info "  pool: $pool_dir"
info "  release: $release_file"
for arch in "${arches[@]}"; do
    info "  packages[$arch]: $dist_dir/$component/binary-$arch/Packages"
    info "  packages_gz[$arch]: $dist_dir/$component/binary-$arch/Packages.gz"
done

if [[ -n "${GPG_KEY:-}" ]]; then
    info "  inrelease: $dist_dir/InRelease"
    info "  release_gpg: $dist_dir/Release.gpg"
else
    info "  signing: skipped because GPG_KEY is not set"
fi

if [[ -n "$repo_url" ]]; then
    info "APT source line:"
    if [[ -n "${GPG_KEY:-}" ]]; then
        info "  deb $repo_url $suite $component"
    else
        info "  deb [trusted=yes] $repo_url $suite $component"
    fi
fi
