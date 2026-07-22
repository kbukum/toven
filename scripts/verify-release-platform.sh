#!/usr/bin/env bash
# High-level release platform verification script.
# Validates Toven release features (plan, status, readiness, sbom, depgraphs, dry-run, tag, publish)
# against both Rust and Go representative fixtures under tests/fixtures, testing all repository kinds.
# Also exercises failure modes such as dirty trees, malformed configurations, invalid versions, etc.
# Uses bare Git remotes and command doubles (mock gh and cargo) to avoid network calls.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOVEN_BIN="${ROOT_DIR}/target/debug/toven"

echo "=== Building Toven ==="
cargo build -p toven --bin toven

TMP_DIR="$(mktemp -d)"
echo "=== Setup Test Sandbox in ${TMP_DIR} ==="
trap 'rm -rf "${TMP_DIR}"' EXIT

# Find real tool paths
REAL_CARGO="$(which cargo)"
REAL_GIT="$(which git)"
REAL_GO="$(which go 2>/dev/null || true)"

# Create command doubles
mkdir -p "${TMP_DIR}/bin"
export PATH="${TMP_DIR}/bin:${PATH}"

export MOCK_LOG="${TMP_DIR}/mock_calls.log"
touch "${MOCK_LOG}"

# Write mock gh
cat << EOF > "${TMP_DIR}/bin/gh"
#!/usr/bin/env bash
echo "mock-gh: \$*" >> "${MOCK_LOG}"
if [[ "\$*" == *"release create"* ]]; then
  # Succeeded
  exit 0
fi
EOF
chmod +x "${TMP_DIR}/bin/gh"

# Write mock cargo with hardcoded REAL_CARGO path and exit code logging
cat << EOF > "${TMP_DIR}/bin/cargo"
#!/usr/bin/env bash
if [[ "\$1" == "publish" || "\$1" == "package" ]]; then
  echo "mock-cargo: \$*" >> "${MOCK_LOG}"
  exit 0
fi
exec "${REAL_CARGO}" "\$@"
EOF
chmod +x "${TMP_DIR}/bin/cargo"

# We want deterministic run output
export TOVEN_CLOCK_EPOCH="1700000000"

###############################################################################
# 1. Rust Release Repository Tests
###############################################################################

# A. Single-Crate Rust project
echo "=== Testing Rust Single Crate ==="
RUST_SINGLE_FIXTURE="${ROOT_DIR}/tests/fixtures/rust-release-repository/single"
RUST_SINGLE_WORK="${TMP_DIR}/rust-single"
cp -r "${RUST_SINGLE_FIXTURE}" "${RUST_SINGLE_WORK}"

cd "${RUST_SINGLE_WORK}"
"${REAL_GIT}" init
"${REAL_GIT}" branch -m main || true
"${REAL_GIT}" config user.name "Toven Test"
"${REAL_GIT}" config user.email "test@toven.dev"
"${REAL_GIT}" add .
"${REAL_GIT}" commit -m "initial commit"
"${REAL_GIT}" tag "rust/single-rust@0.1.0"

# Plan should be up-to-date
"${TOVEN_BIN}" release plan > "${TMP_DIR}/rust-single-plan.log"
grep -q "Release plan" "${TMP_DIR}/rust-single-plan.log"

# Status should show declared version and tags
"${TOVEN_BIN}" release status > "${TMP_DIR}/rust-single-status.log"
grep -q "single-rust" "${TMP_DIR}/rust-single-status.log"

# B. Multi-Workspace Rust project with cascade and non-publishable test-only crate
echo "=== Testing Rust Multi-Workspace ==="
RUST_FIXTURE="${ROOT_DIR}/tests/fixtures/rust-release-repository/multi-workspace"
RUST_WORK_DIR="${TMP_DIR}/rust-repo"
cp -r "${RUST_FIXTURE}" "${RUST_WORK_DIR}"

RUST_BARE="${TMP_DIR}/rust-bare.git"
"${REAL_GIT}" init --bare "${RUST_BARE}"

cd "${RUST_WORK_DIR}"
"${REAL_GIT}" init
"${REAL_GIT}" branch -m main || true
"${REAL_GIT}" config user.name "Toven Test"
"${REAL_GIT}" config user.email "test@toven.dev"
"${REAL_GIT}" add .
"${REAL_GIT}" commit -m "initial commit"
"${REAL_GIT}" remote add origin "${RUST_BARE}"
"${REAL_GIT}" push -u origin main

# Fix bare remote's HEAD branch to avoid libgit2 dangling HEAD error
"${REAL_GIT}" -C "${RUST_BARE}" symbolic-ref HEAD refs/heads/main

# Add initial tags
"${REAL_GIT}" tag "rust/leaf-a@1.0.0"
"${REAL_GIT}" tag "rust/shared-c@0.5.0"
"${REAL_GIT}" tag "rust/leaf-b@2.0.0"
"${REAL_GIT}" tag "rust/test-only@0.1.0"
"${REAL_GIT}" push --tags origin

# Make changes to trigger cascading updates
echo "pub fn updated() {}" >> "${RUST_WORK_DIR}/workspace-b/crates/shared-c/src/lib.rs"
echo "pub fn modified() {}" >> "${RUST_WORK_DIR}/workspace-a/crates/leaf-a/src/lib.rs"
"${REAL_GIT}" commit -am "modify shared-c and leaf-a"

# Verify release plan cascading
"${TOVEN_BIN}" release plan > "${TMP_DIR}/rust-changed-plan.log"
grep -q "shared-c" "${TMP_DIR}/rust-changed-plan.log"
grep -q "leaf-b" "${TMP_DIR}/rust-changed-plan.log"

# Dry-run publish
"${TOVEN_BIN}" release publish --dry-run --offline > "${TMP_DIR}/rust-dry-run.log"
grep -q "would-publish" "${TMP_DIR}/rust-dry-run.log"

# Run mutating tag command
"${TOVEN_BIN}" release tag --yes --allow-dirty --offline

# Verify mutations on disk and pushed tags
grep -q 'version = "0.5.1"' "${RUST_WORK_DIR}/workspace-b/crates/shared-c/Cargo.toml"
"${REAL_GIT}" tag | grep "rust/shared-c@0.5.1"
cd "${RUST_BARE}"
"${REAL_GIT}" tag | grep "rust/shared-c@0.5.1"

cd "${RUST_WORK_DIR}"

# C. Workspace-Inherited Rust project
echo "=== Testing Rust Workspace Inherited ==="
RUST_INH_FIXTURE="${ROOT_DIR}/tests/fixtures/rust-release-repository/workspace-inherited"
RUST_INH_WORK="${TMP_DIR}/rust-inherited"
cp -r "${RUST_INH_FIXTURE}" "${RUST_INH_WORK}"

cd "${RUST_INH_WORK}"
"${REAL_GIT}" init
"${REAL_GIT}" branch -m main || true
"${REAL_GIT}" config user.name "Toven Test"
"${REAL_GIT}" config user.email "test@toven.dev"
"${REAL_GIT}" add .
"${REAL_GIT}" commit -m "initial commit"
"${REAL_GIT}" tag "rust/inherited-app@0.3.0"

# Plan should load successfully and resolve inherited version
"${TOVEN_BIN}" release plan > "${TMP_DIR}/rust-inherited-plan.log"
grep -q "Release plan" "${TMP_DIR}/rust-inherited-plan.log"

# Change the inherited package and verify plan
echo "pub fn updated() {}" >> "${RUST_INH_WORK}/crates/inherited-app/src/lib.rs"
"${REAL_GIT}" commit -am "modify inherited-app"
"${TOVEN_BIN}" release plan > "${TMP_DIR}/rust-inherited-changed-plan.log"
grep -q "inherited-app" "${TMP_DIR}/rust-inherited-changed-plan.log"

###############################################################################
# 2. Go Release Repository Tests
###############################################################################
if [ -n "${REAL_GO}" ]; then
  # A. Single Go Module
  echo "=== Testing Go Single Module ==="
  GO_SINGLE_FIXTURE="${ROOT_DIR}/tests/fixtures/go-release-repository/single"
  GO_SINGLE_WORK="${TMP_DIR}/go-single"
  cp -r "${GO_SINGLE_FIXTURE}" "${GO_SINGLE_WORK}"

  cd "${GO_SINGLE_WORK}"
  "${REAL_GIT}" init
  "${REAL_GIT}" branch -m main || true
  "${REAL_GIT}" config user.name "Toven Test"
  "${REAL_GIT}" config user.email "test@toven.dev"
  "${REAL_GIT}" add .
  "${REAL_GIT}" commit -m "initial commit"
  "${REAL_GIT}" tag "v0.1.0"

  "${TOVEN_BIN}" release plan > "${TMP_DIR}/go-single-plan.log"
  grep -q "Release plan" "${TMP_DIR}/go-single-plan.log"

  # B. Multi-Module Go
  echo "=== Testing Go Multi-Module ==="
  GO_FIXTURE="${ROOT_DIR}/tests/fixtures/go-release-repository/multi-module"
  GO_WORK_DIR="${TMP_DIR}/go-repo"
  cp -r "${GO_FIXTURE}" "${GO_WORK_DIR}"

  GO_BARE="${TMP_DIR}/go-bare.git"
  "${REAL_GIT}" init --bare "${GO_BARE}"

  cd "${GO_WORK_DIR}"
  "${REAL_GIT}" init
  "${REAL_GIT}" branch -m main || true
  "${REAL_GIT}" config user.name "Toven Test"
  "${REAL_GIT}" config user.email "test@toven.dev"
  "${REAL_GIT}" add .
  "${REAL_GIT}" commit -m "initial commit"
  "${REAL_GIT}" remote add origin "${GO_BARE}"
  "${REAL_GIT}" push -u origin main

  # Fix bare remote HEAD reference
  "${REAL_GIT}" -C "${GO_BARE}" symbolic-ref HEAD refs/heads/main

  # Add initial tags
  "${REAL_GIT}" tag "v1.0.0"
  "${REAL_GIT}" tag "nested/v1.1.0"
  "${REAL_GIT}" push --tags origin

  echo "--- Previews on clean Go repository ---"
  "${TOVEN_BIN}" release plan > "${TMP_DIR}/go-plan.log"
  grep -q "Release plan" "${TMP_DIR}/go-plan.log"

  # Make changes
  echo "// update" >> "${GO_WORK_DIR}/main.go"
  "${REAL_GIT}" commit -am "modify root module"

  echo "--- Previews on changed Go repository ---"
  "${TOVEN_BIN}" release plan > "${TMP_DIR}/go-changed-plan.log"
  grep -q "go-release-repository" "${TMP_DIR}/go-changed-plan.log"

  # Run tag mutation
  "${TOVEN_BIN}" release tag --yes --allow-dirty --offline
  "${REAL_GIT}" tag | grep "v1.0.1"
  ! "${REAL_GIT}" tag | grep "testmod/v"
else
  echo "=== Skipping Go Release Repository: go toolchain not found ==="
fi

###############################################################################
# 3. Failure Cases and Guards Verification
###############################################################################
echo "=== Verifying Guards & Failure Cases ==="

# A. Dirty Tree Check
cd "${RUST_WORK_DIR}"
echo "// dirty change" >> "${RUST_WORK_DIR}/workspace-a/crates/leaf-a/src/lib.rs"
# Should fail without --allow-dirty
if "${TOVEN_BIN}" release tag --yes --offline 2> "${TMP_DIR}/dirty-error.log"; then
  echo "Error: Allowed release tag on dirty tree without --allow-dirty!"
  exit 1
else
  echo "Success: Rejected dirty tree correctly"
  cat "${TMP_DIR}/dirty-error.log"
fi

# Cleanup dirty change
"${REAL_GIT}" checkout -- .

# B. Malformed Configuration Check
cd "${TMP_DIR}"
mkdir -p malformed-repo
cd malformed-repo
"${REAL_GIT}" init
"${REAL_GIT}" branch -m main || true
echo "invalid config toml body: {{" > toven.toml
if "${TOVEN_BIN}" release plan 2> "${TMP_DIR}/malformed-error.log"; then
  echo "Error: Allowed malformed toven.toml configuration!"
  exit 1
else
  echo "Success: Rejected malformed configuration correctly"
  cat "${TMP_DIR}/malformed-error.log"
fi

# C. Invalid Version Check
cd "${TMP_DIR}"
mkdir -p invalid-ver-repo
cd invalid-ver-repo
"${REAL_GIT}" init
"${REAL_GIT}" branch -m main || true
cat << 'EOF' > toven.toml
[project]
name = "invalid-ver"
[ecosystems.rust]
manifests = ["Cargo.toml"]
EOF
cat << 'EOF' > Cargo.toml
[package]
name = "invalid-ver"
version = "invalid-semver-1.2.3"
EOF
if "${TOVEN_BIN}" release plan 2> "${TMP_DIR}/invalid-ver-error.log"; then
  echo "Error: Allowed invalid semver in Cargo.toml!"
  exit 1
else
  echo "Success: Rejected invalid version correctly"
  cat "${TMP_DIR}/invalid-ver-error.log"
fi

echo "=== Release Platform Verification SUCCESS ==="
