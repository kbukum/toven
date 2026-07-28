#!/usr/bin/env bash
# Package one already-built `toven` binary into the fixed-name archive that
# `toven.toml`'s `[ecosystems.rust.release.host]` `assets` list declares.
#
# Asset paths in that config are exact, non-templated project-relative paths
# (see crates/toven-ports/src/config/release/host.rs), so archive names never
# embed the release version; the version lives in the release tag and Release
# title instead. Each per-target CI job runs this script once after its native
# `cargo build --release` (or `cross build --release` for the cross-compiled
# `aarch64-unknown-linux-gnu` target); the assembling job then collects every
# archive under `dist/` before checksumming, signing, and publishing.
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $(basename "$0") <rust-target-triple> <built-binary-path>" >&2
  exit 2
fi

target="$1"
built_binary="$2"

# The target triple names the archive written under dist/; reject anything
# outside the triple alphabet (lowercase letters, digits, `_`, `-`) so a
# manual invocation cannot traverse paths.
if [[ ! "${target}" =~ ^[a-z0-9_-]+$ ]]; then
  echo "package-release-binary: invalid target triple '${target}' (expected e.g. x86_64-unknown-linux-gnu)" >&2
  exit 2
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dist_dir="${repo_root}/dist"
mkdir -p "${dist_dir}"

if [[ ! -f "${built_binary}" ]]; then
  echo "package-release-binary: built binary not found at '${built_binary}'" >&2
  exit 1
fi

stage_dir="$(mktemp -d)"
trap 'rm -rf "${stage_dir}"' EXIT

case "${target}" in
  *windows*)
    binary_name="toven.exe"
    archive_path="${dist_dir}/toven-${target}.zip"
    cp "${built_binary}" "${stage_dir}/${binary_name}"
    (cd "${stage_dir}" && zip -q -X "${archive_path}" "${binary_name}")
    ;;
  *)
    binary_name="toven"
    archive_path="${dist_dir}/toven-${target}.tar.gz"
    cp "${built_binary}" "${stage_dir}/${binary_name}"
    chmod 755 "${stage_dir}/${binary_name}"
    # Normalize archived ownership for reproducible checksums. GNU tar and
    # bsdtar spell these flags differently; macOS runners ship bsdtar.
    if tar --version 2>/dev/null | grep -q 'GNU tar'; then
      tar --numeric-owner --owner=0 --group=0 -czf "${archive_path}" -C "${stage_dir}" "${binary_name}"
    else
      tar --numeric-owner --uid 0 --gid 0 -czf "${archive_path}" -C "${stage_dir}" "${binary_name}"
    fi
    ;;
esac

echo "package-release-binary: wrote ${archive_path}" >&2
