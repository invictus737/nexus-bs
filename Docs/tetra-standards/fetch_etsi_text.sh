#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
manifest="${script_dir}/standards.tsv"
cache_dir="${script_dir}/cache"

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required" >&2
  exit 1
fi

if ! command -v pdftotext >/dev/null 2>&1; then
  echo "pdftotext is required. Install poppler first." >&2
  exit 1
fi

mkdir -p "${cache_dir}"

tmp_dir="$(mktemp -d)"
cleanup() {
  rm -rf "${tmp_dir}"
}
trap cleanup EXIT

tail -n +2 "${manifest}" | while IFS=$'\t' read -r standard_id version filename url; do
  pdf_path="${tmp_dir}/${filename}.pdf"
  txt_path="${cache_dir}/${filename}.txt"
  meta_path="${cache_dir}/${filename}.source"

  echo "Fetching ${standard_id} ${version}..."
  curl -fL --retry 3 --retry-delay 2 \
    -A "Mozilla/5.0 Nexus-BS local standards cache" \
    -o "${pdf_path}" "${url}"
  pdftotext -layout -enc UTF-8 "${pdf_path}" "${txt_path}"
  {
    echo "id=${standard_id}"
    echo "version=${version}"
    echo "source_url=${url}"
    echo "generated_utc=$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  } >"${meta_path}"
  echo "Wrote ${txt_path}"
done
