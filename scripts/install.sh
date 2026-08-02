#!/bin/sh
# Install a released Toven binary on Linux or macOS.
#
# Designed to be run either as a checked-in script or piped from a URL:
#
#   # latest release, default location (~/.toven/bin)
#   curl -fsSL https://raw.githubusercontent.com/kbukum/toven/main/scripts/install.sh | sh
#
#   # pin an immutable version and/or choose a directory (note the `-s --`)
#   curl -fsSL https://raw.githubusercontent.com/kbukum/toven/main/scripts/install.sh \
#     | sh -s -- --version v0.1.0-alpha.2 --dir /usr/local/bin
#
# Regardless of how the version is chosen, the download is always by an
# immutable release tag: with no --version the latest published tag (including
# prereleases) is resolved first, then its exact assets are fetched. The archive
# is never trusted before its SHA-256 checksum verifies; when `cosign` is
# present the keyless Sigstore signature over `SHA256SUMS` is checked first. No
# secret is ever passed on argv.
#
# CI note: pin the version explicitly (`--version` / `TOVEN_VERSION`) and pin
# this script itself to a tag, e.g.
#   .../kbukum/toven/v0.1.0-alpha.2/scripts/install.sh
# so an unpinned latest-release URL never enters an automated pipeline.
#
# Flags (env fallbacks in parentheses):
#   --version <tag>  Release tag to install; default: latest    (TOVEN_VERSION)
#   --dir <path>     Install directory; default: ~/.toven/bin    (TOVEN_INSTALL_DIR)
#   --target <triple> Override the auto-detected Rust target      (TOVEN_TARGET)
#   --repo <owner/repo> Source repository; default kbukum/toven   (TOVEN_REPO)
#   --help           Show this help and exit.
set -eu

version="${TOVEN_VERSION:-}"
install_dir="${TOVEN_INSTALL_DIR:-}"
target="${TOVEN_TARGET:-}"
repo="${TOVEN_REPO:-kbukum/toven}"

log() { printf 'install: %s\n' "$*" >&2; }
die() {
  printf 'install: %s\n' "$*" >&2
  exit 1
}

usage() {
  sed -n '2,40p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//'
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      version="${2:-}"
      shift 2
      ;;
    --version=*)
      version="${1#*=}"
      shift
      ;;
    --dir)
      install_dir="${2:-}"
      shift 2
      ;;
    --dir=*)
      install_dir="${1#*=}"
      shift
      ;;
    --target)
      target="${2:-}"
      shift 2
      ;;
    --target=*)
      target="${1#*=}"
      shift
      ;;
    --repo)
      repo="${2:-}"
      shift 2
      ;;
    --repo=*)
      repo="${1#*=}"
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      die "unknown argument '$1' (try --help)"
      ;;
  esac
done

install_dir="${install_dir:-${HOME}/.toven/bin}"

need() { command -v "$1" >/dev/null 2>&1 || die "required tool '$1' not found on PATH"; }
need curl
need tar
need mktemp

# Resolve the Rust target triple from the host unless one is pinned explicitly.
detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"
  case "${os}" in
    Linux) os_part="unknown-linux-gnu" ;;
    Darwin) os_part="apple-darwin" ;;
    *) die "unsupported OS '${os}'; set --target explicitly (Windows: use install.ps1)" ;;
  esac
  case "${arch}" in
    x86_64 | amd64) arch_part="x86_64" ;;
    arm64 | aarch64) arch_part="aarch64" ;;
    *) die "unsupported architecture '${arch}'; set --target explicitly" ;;
  esac
  printf '%s-%s' "${arch_part}" "${os_part}"
}

# Discover the newest published release tag, including prereleases (GitHub's
# `releases/latest` endpoint hides prereleases, and Toven currently ships only
# alpha prereleases, so list releases and take the first).
resolve_latest_version() {
  api="https://api.github.com/repos/${repo}/releases?per_page=1"
  auth_file=""
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    auth_file="$(mktemp)"
    chmod 600 "${auth_file}"
    printf 'Authorization: Bearer %s\n' "${GITHUB_TOKEN}" >"${auth_file}"
  fi
  if [ -n "${auth_file}" ]; then
    tag="$(curl -fsSL -H "@${auth_file}" "${api}" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)"
    rm -f "${auth_file}"
  else
    tag="$(curl -fsSL "${api}" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)"
  fi
  [ -n "${tag}" ] || die "could not resolve the latest release tag from ${api}"
  printf '%s' "${tag}"
}

target="${target:-$(detect_target)}"
if [ -z "${version}" ]; then
  log "resolving the latest release tag for ${repo}"
  version="$(resolve_latest_version)"
fi

archive="toven-${target}.tar.gz"
base="https://github.com/${repo}/releases/download/${version}"

workdir="$(mktemp -d)"
trap 'rm -rf "${workdir}"' EXIT INT TERM

log "downloading ${archive} and SHA256SUMS for ${version}"
curl -fsSL --output "${workdir}/${archive}" "${base}/${archive}"
curl -fsSL --output "${workdir}/SHA256SUMS" "${base}/SHA256SUMS"

# Verify the keyless Sigstore signature over SHA256SUMS before trusting the
# checksums it carries. Skipped with a warning when cosign is absent; the
# checksum verification below is always enforced.
if command -v cosign >/dev/null 2>&1; then
  log "verifying the Sigstore signature on SHA256SUMS"
  curl -fsSL --output "${workdir}/SHA256SUMS.sig" "${base}/SHA256SUMS.sig"
  curl -fsSL --output "${workdir}/SHA256SUMS.pem" "${base}/SHA256SUMS.pem"
  cosign verify-blob \
    --certificate "${workdir}/SHA256SUMS.pem" \
    --signature "${workdir}/SHA256SUMS.sig" \
    --certificate-identity-regexp "https://github.com/${repo}/.github/workflows/release.yml@.*" \
    --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
    "${workdir}/SHA256SUMS" >&2
else
  log "cosign not found; skipping signature verification (checksum still enforced)"
fi

log "verifying ${archive} against SHA256SUMS"
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

log "extracting ${archive}"
tar -xzf "${workdir}/${archive}" -C "${workdir}" -- toven

mkdir -p "${install_dir}"
install_dir_abs="$(cd "${install_dir}" && pwd)"
mv "${workdir}/toven" "${install_dir_abs}/toven"
chmod +x "${install_dir_abs}/toven"

installed="$("${install_dir_abs}/toven" --version)"
log "installed ${installed} at ${install_dir_abs}/toven"

# Nudge the user if the install directory is not already on PATH.
case ":${PATH}:" in
  *":${install_dir_abs}:"*) : ;;
  *)
    log "add it to PATH, e.g.:"
    log "  export PATH=\"${install_dir_abs}:\$PATH\""
    ;;
esac
