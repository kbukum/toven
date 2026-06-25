#!/usr/bin/env bash
# Managed end-to-end smoke for the `toven-rs` app against a fixture repo.
#
# Builds the standalone Rust app, materializes the committed `single-rust`
# fixture into a throwaway git working tree, and drives the argv-first surface
# end to end: an introspection projection (`modules`), a PLAN-only cut
# (`plan build`), and a full PLAN+APPLY run (`build`). Each step must exit zero.
# This is the first point the whole stack is exercised as a real binary.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixture="${repo_root}/crates/toven-testkit/fixtures/repos/single-rust"

echo "smoke: building toven-rs" >&2
cargo build -p toven-rs
bin="${repo_root}/target/debug/toven-rs"
[ -x "${bin}" ] || { echo "smoke: toven-rs binary not found at ${bin}" >&2; exit 1; }

mkdir -p "${repo_root}/target"
work="$(mktemp -d "${repo_root}/target/smoke.XXXXXX")"
trap 'rm -rf "${work}"' EXIT
cp -R "${fixture}/." "${work}/"

echo "smoke: initializing fixture git tree in ${work}" >&2
git -C "${work}" init -q
git -C "${work}" add -A
git -C "${work}" -c user.email=smoke@toven.dev -c user.name=smoke commit -q -m "smoke fixture"

run() {
  echo "smoke: toven-rs $*" >&2
  ( cd "${work}" && "${bin}" "$@" )
}

modules_out="$(run modules)"
printf '%s\n' "${modules_out}"
printf '%s\n' "${modules_out}" | grep -q "rust:app" \
  || { echo "smoke: modules did not list rust:app" >&2; exit 1; }
run plan build
run build

echo "smoke: OK" >&2
