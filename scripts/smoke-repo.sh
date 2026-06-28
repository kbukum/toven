#!/usr/bin/env bash
# Managed binary smoke for the umbrella `toven` app against an arbitrary real
# repository.
#
# Unlike the in-tree app smoke integration tests (`apps/toven-rs/tests/smoke.rs`
# runs a full PLAN+APPLY, `apps/toven/tests/smoke.rs` a read-only PLAN cut, and
# `apps/toven-go/tests/federation_smoke.rs` the driver handshake, all via
# `make smoke`), this builds the umbrella `toven` binary from this checkout and
# runs that debug binary over a caller-supplied checkout, stopping at PLAN so it
# stays read-only and safe to point at any working tree. It proves Toven
# discovers modules and renders a reviewable plan for a real repo — the rehearsal
# the release policy calls for.
#
# Usage:
#   scripts/smoke-repo.sh <repo-path> [task]
#
# The repo must carry its own `toven.toml`; the smoke never synthesizes config.
# `task` defaults to `build`. PLAN only — no task command is executed.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

repo="${1:-}"
task="${2:-build}"
if [[ -z "${repo}" ]]; then
  echo "error: repo path is required" >&2
  echo "usage: scripts/smoke-repo.sh <repo-path> [task]" >&2
  exit 2
fi

if [[ ! -d "${repo}" ]]; then
  echo "error: repo path is not a directory: ${repo}" >&2
  exit 2
fi
repo="$(cd "${repo}" && pwd)"
if [[ ! -f "${repo}/toven.toml" ]]; then
  echo "error: ${repo} has no toven.toml" >&2
  echo "hint: run 'toven generate --write' in the repo to scaffold one before smoking it" >&2
  exit 2
fi

echo "smoke-repo: building toven (umbrella)" >&2
cargo build --manifest-path "${repo_root}/Cargo.toml" -p toven
bin="${repo_root}/target/debug/toven"
[ -x "${bin}" ] || { echo "smoke-repo: toven binary not found at ${bin}" >&2; exit 1; }

run() {
  echo "smoke-repo: toven $*" >&2
  ( cd "${repo}" && "${bin}" "$@" )
}

modules_out="$(run modules)"
printf '%s\n' "${modules_out}"
printf '%s\n' "${modules_out}" | grep -q ':' \
  || { echo "smoke-repo: modules listed no 'ecosystem:module' entries" >&2; exit 1; }

run plan "${task}"

echo "smoke-repo: OK" >&2
