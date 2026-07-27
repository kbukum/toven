#!/usr/bin/env bash
# Verify one target's packaged `toven` release archive: extract it and check
# that the binary reports the expected released version. With `--download`,
# first fetch the archive, the combined SHA256SUMS, and that file's keyless
# Sigstore/cosign signature and certificate from the hosted GitHub Release
# (using the ambient `gh` auth), verify the signature on SHA256SUMS before
# trusting it, then verify the archive's checksum before extracting it —
# this is the "download every published binary and verify it runs and
# reports the expected version" step of the release approval pipeline
# (docs/self-hosting.md). With `--no-run`, the signature and checksum are
# still verified, but the binary is not executed — for the cross-compiled
# Linux ARM64 target, which cannot run on any x86_64 build or verify runner
# (see .github/workflows/release.yml).
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

# Checksum-verify stdin against SHA256SUMS-format lines, using whichever of
# shasum / sha256sum the runner provides (shasum is not installed everywhere,
# e.g. Windows runners); verification must fail closed, never be skipped.
sha256_check() {
  if command -v shasum >/dev/null; then
    shasum -a 256 -c -
  elif command -v sha256sum >/dev/null; then
    sha256sum -c -
  else
    echo "verify-release-binary: neither shasum nor sha256sum is available" >&2
    exit 1
  fi
}

if [[ "${download}" -eq 1 ]]; then
  tag="v${expected_version}"
  if ! command -v cosign >/dev/null; then
    echo "verify-release-binary: cosign is required to verify the SHA256SUMS signature" >&2
    exit 1
  fi
  echo "verify-release-binary: downloading ${archive_name}, SHA256SUMS, and its signature from release ${tag}" >&2
  gh release download "${tag}" \
    --dir "${work_dir}" \
    --pattern "${archive_name}" \
    --pattern "SHA256SUMS" \
    --pattern "SHA256SUMS.sig" \
    --pattern "SHA256SUMS.pem"

  # The checksums are only trustworthy once the keyless Sigstore signature on
  # the SHA256SUMS file itself verifies against the release workflow identity
  # (the same command docs/installation.md gives installers).
  cosign verify-blob \
    --certificate "${work_dir}/SHA256SUMS.pem" \
    --signature "${work_dir}/SHA256SUMS.sig" \
    --certificate-identity-regexp 'https://github.com/kbukum/toven/.github/workflows/release.yml@.*' \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com \
    "${work_dir}/SHA256SUMS"

  (
    cd "${work_dir}"
    grep -F " ${archive_name}" SHA256SUMS | sha256_check
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
