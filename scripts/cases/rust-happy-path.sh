#!/usr/bin/env bash
# Case: Rust Release Happy Path & Dependent Cascade
set -euo pipefail

CASE_NAME="rust-happy-path"
RUST_FIXTURE="${ROOT_DIR}/tests/fixtures/rust-release-repository/multi-workspace"
RUST_WORK_DIR="${TMP_DIR}/rust-happy-path"
cp -r "${RUST_FIXTURE}" "${RUST_WORK_DIR}"

RUST_BARE="${TMP_DIR}/rust-happy-path-bare.git"
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
"${REAL_GIT}" -C "${RUST_BARE}" symbolic-ref HEAD refs/heads/main

# Initial tags
"${REAL_GIT}" tag "rust/leaf-a@1.0.0"
"${REAL_GIT}" tag "rust/shared-c@0.5.0"
"${REAL_GIT}" tag "rust/leaf-b@2.0.0"
"${REAL_GIT}" tag "rust/test-only@0.1.0"
"${REAL_GIT}" push --tags origin

# Make changes to trigger cascading updates
echo "pub fn updated() {}" >> "${RUST_WORK_DIR}/workspace-b/crates/shared-c/src/lib.rs"
echo "pub fn modified() {}" >> "${RUST_WORK_DIR}/workspace-a/crates/leaf-a/src/lib.rs"
"${REAL_GIT}" commit -am "modify shared-c and leaf-a"

# Verify plan cascade
"${TOVEN_BIN}" release plan > "${TMP_DIR}/${CASE_NAME}-plan.log"
grep -q "shared-c" "${TMP_DIR}/${CASE_NAME}-plan.log"
grep -q "leaf-b" "${TMP_DIR}/${CASE_NAME}-plan.log"

# Run mutating tag
"${TOVEN_BIN}" release tag --yes --allow-dirty --offline

# Verify mutations on disk and pushed tags
grep -q 'version = "0.5.1"' "${RUST_WORK_DIR}/workspace-b/crates/shared-c/Cargo.toml"
"${REAL_GIT}" tag | grep "rust/shared-c@0.5.1"
cd "${RUST_BARE}"
"${REAL_GIT}" tag | grep "rust/shared-c@0.5.1"

echo "Case ${CASE_NAME} PASSED"
