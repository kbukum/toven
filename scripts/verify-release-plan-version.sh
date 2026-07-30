#!/usr/bin/env bash
# Preflight invariant for Toven's own release pipeline: the version the release
# engine will cut MUST equal `v${workspace Cargo.toml version}`.
#
# The release workflow (.github/workflows/release.yml) builds and packages every
# target binary from the checked-out Cargo.toml in the `build` job *before* the
# `publish` job creates the tag, so each packaged `toven --version` reports the
# Cargo.toml version (scripts/verify-release-binary.sh checks exactly that). If
# the engine plans a different version — a stray/orphan `v*` tag flipping the
# release baseline off "initial" and bumping/finalizing the version, a forgotten
# workspace version bump, or an explicit override Cargo.toml does not reflect —
# the published tag and the built binaries diverge, and every downstream Verify
# job fails with an opaque "release not found" only after five binaries were
# built and the human publish gate was approved.
#
# Fail closed here instead, in the mutation-free preview, with the precise
# divergence and its fixes, long before anything is built or approved. This
# reads the plan and Cargo.toml only; it mutates nothing.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

cargo_version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
if [[ -z "${cargo_version}" ]]; then
  echo "verify-release-plan-version: could not read the workspace version from Cargo.toml" >&2
  exit 1
fi
expected_tag="v${cargo_version}"

# The umbrella `toven` app is the binary the release ships and Verify executes,
# so its planned tag is THE hosted Release tag. The trailing quote in the match
# token keeps it from also matching `rust:toven-model`, `rust:toven-ports`, etc.
plan_line="$(cargo run --locked -p toven -- release plan --output jsonl \
  | grep -F '"module":"rust:toven"' || true)"
if [[ -z "${plan_line}" ]]; then
  echo "verify-release-plan-version: the release plan has no entry for the umbrella 'rust:toven' app (nothing to release, or the plan output shape changed)" >&2
  exit 1
fi

planned_tag="$(printf '%s' "${plan_line}" | sed -n 's/.*"tag":"\([^"]*\)".*/\1/p')"
planned_version="$(printf '%s' "${plan_line}" | sed -n 's/.*"planned_version":"\([^"]*\)".*/\1/p')"

if [[ "${planned_tag}" != "${expected_tag}" ]]; then
  cat >&2 <<EOF
verify-release-plan-version: release/binary version divergence.

  workspace Cargo.toml version : ${cargo_version}  (every built 'toven --version' reports this)
  engine will cut tag          : ${planned_tag}  (planned version ${planned_version})

The build job packages binaries from Cargo.toml *before* publish creates the
tag, so the published tag must be '${expected_tag}' or the Verify jobs cannot
match the binaries to the release. This divergence is usually one of:

  * a stray/orphan release or v* tag left by a failed run at or above the
    declared version, flipping the release baseline off "initial" and
    bumping/finalizing the version. List them and delete the offender:
        gh release list        # then: gh release delete <tag> --cleanup-tag
        git tag --list 'v*'    # tag only: git push origin --delete <tag>
  * a forgotten workspace version bump in Cargo.toml;
  * an explicit --set-version / --minor / --major that Cargo.toml does not
    reflect.

Align Cargo.toml with the intended release version (and remove any stray tag),
then re-run.
EOF
  exit 1
fi

echo "verify-release-plan-version: engine will cut '${planned_tag}', matching Cargo.toml '${cargo_version}'" >&2
