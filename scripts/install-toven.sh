#!/usr/bin/env bash
# Reference direct-download install for a released Toven binary.
#
# This is the canonical downstream install contract: pin an immutable release
# version, download the matching per-target archive together with `SHA256SUMS`,
# verify the checksum (and, when `cosign` is present, the keyless Sigstore
# signature on `SHA256SUMS`) before extracting, then place `toven` on a target
# directory. A future `toven-action` must reproduce exactly this behavior; the
# same procedure is documented for humans in `docs/installation.md`.
#
# It never uses an unpinned latest-release URL, never trusts an archive before
# its checksum verifies, and passes no secrets on argv.
#
# Usage:
#   scripts/install-toven.sh <version> [install-dir]
#
# Arguments and environment:
#   <version>       Immutable release tag to pin, e.g. `v0.1.0-alpha.2`.
#                   Also accepted via TOVEN_VERSION.
#   [install-dir]   Directory to place the `toven` binary in (default: ./bin).
#                   Also accepted via TOVEN_INSTALL_DIR.
#   TOVEN_TARGET    Override the auto-detected Rust target triple.
#   TOVEN_REPO      Override the source repository (default: kbukum/toven).
set -euo pipefail

version="${1:-${TOVEN_VERSION:-}}"
install_dir="${2:-${TOVEN_INSTALL_DIR:-bin}}"
repo="${TOVEN_REPO:-kbukum/toven}"

if [[ -z "${version}" ]]; then
  echo "install-toven: a release version is required (e.g. v0.1.0-alpha.2)" >&2
  echo "usage: scripts/install-toven.sh <version> [install-dir]" >&2
  exit 2
fi

# Resolve the Rust target triple from the host unless one is pinned explicitly.
detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "${os}" in
    Linux) os_part="unknown-linux-gnu" ;;
    Darwin) os_part="apple-darwin" ;;
    *)
      echo "install-toven: unsupported OS '${os}'; set TOVEN_TARGET explicitly" >&2
      exit 1
      ;;
  esac
  case "${arch}" in
    x86_64 | amd64) arch_part="x86_64" ;;
    arm64 | aarch64) arch_part="aarch64" ;;
    *)
      echo "install-toven: unsupported architecture '${arch}'; set TOVEN_TARGET explicitly" >&2
      exit 1
      ;;
  esac
  printf '%s-%s' "${arch_part}" "${os_part}"
}

target="${TOVEN_TARGET:-$(detect_target)}"
archive="toven-${target}.tar.gz"
base="https://github.com/${repo}/releases/download/${version}"

workdir="$(mktemp -d)"
trap 'rm -rf "${workdir}"' EXIT

echo "install-toven: downloading ${archive} and SHA256SUMS for ${version}" >&2
curl --fail --silent --show-error --location \
  --output "${workdir}/${archive}" "${base}/${archive}"
curl --fail --silent --show-error --location \
  --output "${workdir}/SHA256SUMS" "${base}/SHA256SUMS"

# Optionally verify the keyless Sigstore signature on SHA256SUMS before trusting
# the checksums it carries. The keyless identity/issuer match Toven's release
# workflow; skipped with a warning when cosign is not installed.
if command -v cosign >/dev/null 2>&1; then
  echo "install-toven: verifying the Sigstore signature on SHA256SUMS" >&2
  curl --fail --silent --show-error --location \
    --output "${workdir}/SHA256SUMS.sig" "${base}/SHA256SUMS.sig"
  curl --fail --silent --show-error --location \
    --output "${workdir}/SHA256SUMS.pem" "${base}/SHA256SUMS.pem"
  cosign verify-blob \
    --certificate "${workdir}/SHA256SUMS.pem" \
    --signature "${workdir}/SHA256SUMS.sig" \
    --certificate-identity-regexp "https://github.com/${repo}/.github/workflows/release.yml@.*" \
    --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
    "${workdir}/SHA256SUMS" >&2
else
  echo "install-toven: cosign not found; skipping signature verification (checksum still enforced)" >&2
fi

echo "install-toven: verifying ${archive} against SHA256SUMS" >&2
(
  cd "${workdir}"
  # --ignore-missing: SHA256SUMS covers every target archive and the SBOM; only
  # the one archive we downloaded is present, and it must verify.
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum --ignore-missing -c SHA256SUMS
  else
    shasum --ignore-missing -a 256 -c SHA256SUMS
  fi
)

echo "install-toven: extracting ${archive}" >&2
tar -xzf "${workdir}/${archive}" -C "${workdir}"

mkdir -p "${install_dir}"
install_dir_abs="$(cd "${install_dir}" && pwd)"
mv "${workdir}/toven" "${install_dir_abs}/toven"
chmod +x "${install_dir_abs}/toven"

installed="$("${install_dir_abs}/toven" --version)"
echo "install-toven: installed ${installed} at ${install_dir_abs}/toven" >&2
