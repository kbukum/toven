#!/usr/bin/env bash
# Render the Homebrew formula and Scoop manifest for a Toven release.
#
# Both are thin, verifiable projections of an existing signed release: the
# per-target archive URLs point at the immutable release tag, and every hash is
# taken from that release's `SHA256SUMS` (do not hand-edit hashes). Use it after
# `SHA256SUMS` exists for the release, e.g. in the package-publish workflow or
# locally against a downloaded checksums file.
#
# Usage:
#   scripts/gen-packaging.sh <version-tag> <SHA256SUMS-file> <out-dir>
#
# Example:
#   scripts/gen-packaging.sh v0.1.0-alpha.2 dist/SHA256SUMS build/packaging
#
# Produces:
#   <out-dir>/homebrew/toven.rb
#   <out-dir>/scoop/toven.json
set -eu

tag="${1:-}"
sums="${2:-}"
out="${3:-}"

if [ -z "${tag}" ] || [ -z "${sums}" ] || [ -z "${out}" ]; then
  echo "usage: scripts/gen-packaging.sh <version-tag> <SHA256SUMS-file> <out-dir>" >&2
  exit 2
fi
[ -f "${sums}" ] || {
  echo "gen-packaging: checksums file not found: ${sums}" >&2
  exit 1
}

here="$(cd "$(dirname "$0")/.." && pwd)"
version="${tag#v}" # templates re-add the leading `v` in URLs

# Look up the checksum for one archive from SHA256SUMS (format: `<sha>␠␠<name>`).
sha_for() {
  archive="$1"
  value="$(awk -v a="${archive}" '$2 == a { print $1 }' "${sums}")"
  [ -n "${value}" ] || {
    echo "gen-packaging: no SHA256SUMS entry for ${archive}" >&2
    exit 1
  }
  printf '%s' "${value}"
}

sha_linux_x86="$(sha_for toven-x86_64-unknown-linux-gnu.tar.gz)"
sha_linux_arm="$(sha_for toven-aarch64-unknown-linux-gnu.tar.gz)"
sha_darwin_x86="$(sha_for toven-x86_64-apple-darwin.tar.gz)"
sha_darwin_arm="$(sha_for toven-aarch64-apple-darwin.tar.gz)"
sha_windows_x86="$(sha_for toven-x86_64-pc-windows-msvc.zip)"

render() {
  sed \
    -e "s/__VERSION__/${version}/g" \
    -e "s/__SHA_X86_64_UNKNOWN_LINUX_GNU__/${sha_linux_x86}/g" \
    -e "s/__SHA_AARCH64_UNKNOWN_LINUX_GNU__/${sha_linux_arm}/g" \
    -e "s/__SHA_X86_64_APPLE_DARWIN__/${sha_darwin_x86}/g" \
    -e "s/__SHA_AARCH64_APPLE_DARWIN__/${sha_darwin_arm}/g" \
    -e "s/__SHA_X86_64_PC_WINDOWS_MSVC__/${sha_windows_x86}/g" \
    "$1"
}

mkdir -p "${out}/homebrew" "${out}/scoop"
render "${here}/packaging/homebrew/toven.rb.template" >"${out}/homebrew/toven.rb"
render "${here}/packaging/scoop/toven.json.template" >"${out}/scoop/toven.json"

echo "gen-packaging: wrote ${out}/homebrew/toven.rb and ${out}/scoop/toven.json for ${tag}" >&2
