#!/usr/bin/env bash
# Run the representative release smoke matrix (Rust + Go fixture trains) as one
# deterministic go/no-go command.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

echo "verify-release-platform: running release engine and fixture matrix" >&2
cargo test --locked -p toven-engine-release
cargo test --locked -p toven-ports --test release_fixture_matrix

echo "verify-release-platform: running CLI release scenarios (Rust + Go)" >&2
filter="${TOVEN_RELEASE_SCENARIO_FILTER:-publish-train/release-}"
status=0
output="$(cargo test --locked -p toven --test golden -- "${filter}" 2>&1)" || status=$?
printf '%s\n' "${output}"
if [[ "${status}" -ne 0 ]]; then
  exit "${status}"
fi

# Fail closed: a renamed or moved scenario makes the substring filter match
# nothing, which libtest reports as a green "0 passed" run. Assert a nonzero
# tests-run count so the gate can never silently pass having run nothing.
passed="$(printf '%s\n' "${output}" | sed -n 's/^test result: ok\. \([0-9][0-9]*\) passed;.*/\1/p' | tail -1)"
if [[ -z "${passed}" || "${passed}" -eq 0 ]]; then
  echo "verify-release-platform: filter '${filter}' matched no release scenarios (expected >0)" >&2
  exit 1
fi

echo "verify-release-platform: OK (${passed} release scenarios)" >&2
