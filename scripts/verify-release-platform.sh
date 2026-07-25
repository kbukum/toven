#!/usr/bin/env bash
# Run the representative release smoke matrix (Rust + Go fixture trains) as one
# deterministic go/no-go command.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

echo "verify-release-platform: running release fixture matrix (Rust + Go)" >&2
cargo test --locked -p toven --test golden -- \
  publish-train/release-

echo "verify-release-platform: OK" >&2
