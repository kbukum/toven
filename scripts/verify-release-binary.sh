#!/usr/bin/env bash
# Verify one target's packaged `toven` release archive: extract it and check
# that the binary reports the expected released version. With `--download`,
# first fetch the archive and the signed SHA256SUMS from the hosted GitHub
# Release (using the ambient `gh` auth) and verify the archive's checksum
# before extracting it — this is the "download every published binary and
# verify it runs and reports the expected version" step of the release
# approval pipeline (docs/self-hosting.md). With `--no-run`, the archive and
# its checksum are still verified, but the binary is not executed — for the
# cross-compiled Linux ARM64 target, which cannot run on any x86_64 build or
# verify runner (see .github/workflows/release.yml).
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $(basename "$0") <rust-target-triple> [--download] [--no-run]" >&2
  exit 2
fi

target="$1"
shift
download=0
run_binary=1
for arg in "$@"; do
  case "${arg}" in
    --download) download=1 ;;
    --no-run) run_binary=0 ;;
    *)
      echo "verify-release-binary: unknown flag '${arg}'" >&2
      exit 2
      ;;
  esac
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

expected_version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
if [[ -z "${expected_version}" ]]; then
  echo "verify-release-binary: could not read the workspace version from Cargo.toml" >&2
  exit 1
fi

case "${target}" in
  *windows*) archive_name="toven-${target}.zip" ;;
  *) archive_name="toven-${target}.tar.gz" ;;
esac

work_dir="$(mktemp -d)"
trap 'rm -rf "${work_dir}"' EXIT

if [[ "${download}" -eq 1 ]]; then
  tag="v${expected_version}"
  echo "verify-release-binary: downloading ${archive_name} and SHA256SUMS from release ${tag}" >&2
  gh release download "${tag}" \
    --dir "${work_dir}" \
    --pattern "${archive_name}" \
    --pattern "SHA256SUMS"

  (
    cd "${work_dir}"
    grep -F " ${archive_name}" SHA256SUMS | shasum -a 256 -c -
  )
  archive_path="${work_dir}/${archive_name}"
else
  archive_path="dist/${archive_name}"
fi

if [[ ! -f "${archive_path}" ]]; then
  echo "verify-release-binary: archive not found at '${archive_path}'" >&2
  exit 1
fi

if [[ "${run_binary}" -eq 0 ]]; then
  echo "verify-release-binary: ${target} archive present and checksum-verified (not executed on this runner)" >&2
  exit 0
fi

extract_dir="${work_dir}/extracted"
mkdir -p "${extract_dir}"
case "${archive_name}" in
  *.zip) unzip -q "${archive_path}" -d "${extract_dir}" ;;
  *) tar -xzf "${archive_path}" -C "${extract_dir}" ;;
esac

binary="${extract_dir}/toven"
if [[ "${target}" == *windows* ]]; then
  binary="${extract_dir}/toven.exe"
fi
chmod +x "${binary}" 2>/dev/null || true

reported="$("${binary}" --version)"
expected="toven ${expected_version}"
if [[ "${reported}" != "${expected}" ]]; then
  echo "verify-release-binary: expected '${expected}', got '${reported}'" >&2
  exit 1
fi

echo "verify-release-binary: ${target} reports '${reported}'" >&2
