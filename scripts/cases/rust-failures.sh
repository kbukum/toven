#!/usr/bin/env bash
# Case: Rust Release Failures and Guardrails
set -euo pipefail

CASE_NAME="rust-failures"
RUST_FIXTURE="${ROOT_DIR}/tests/fixtures/rust-release-repository/multi-workspace"
RUST_WORK_DIR="${TMP_DIR}/rust-failures"
cp -r "${RUST_FIXTURE}" "${RUST_WORK_DIR}"

cd "${RUST_WORK_DIR}"
"${REAL_GIT}" init
"${REAL_GIT}" branch -m main || true
"${REAL_GIT}" config user.name "Toven Test"
"${REAL_GIT}" config user.email "test@toven.dev"
"${REAL_GIT}" add .
"${REAL_GIT}" commit -m "initial commit"

echo "--- Verifying Dirty Tree Guard ---"
echo "// dirty change" >> "${RUST_WORK_DIR}/workspace-a/crates/leaf-a/src/lib.rs"
if "${TOVEN_BIN}" release tag --yes --offline 2> "${TMP_DIR}/${CASE_NAME}-dirty.log"; then
  echo "Error: Allowed release tag on dirty tree without --allow-dirty!"
  exit 1
else
  echo "Rejected dirty tree correctly"
  cat "${TMP_DIR}/${CASE_NAME}-dirty.log"
fi
"${REAL_GIT}" checkout -- .

echo "--- Verifying Malformed Config Guard ---"
cd "${TMP_DIR}"
mkdir -p "${CASE_NAME}-malformed"
cd "${CASE_NAME}-malformed"
"${REAL_GIT}" init
"${REAL_GIT}" branch -m main || true
echo "invalid config toml body: {{" > toven.toml
if "${TOVEN_BIN}" release plan 2> "${TMP_DIR}/${CASE_NAME}-malformed.log"; then
  echo "Error: Allowed malformed toven.toml configuration!"
  exit 1
else
  echo "Rejected malformed configuration correctly"
  cat "${TMP_DIR}/${CASE_NAME}-malformed.log"
fi

echo "--- Verifying Invalid Version Guard ---"
cd "${TMP_DIR}"
mkdir -p "${CASE_NAME}-invalid-ver"
cd "${CASE_NAME}-invalid-ver"
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
if "${TOVEN_BIN}" release plan 2> "${TMP_DIR}/${CASE_NAME}-invalid-ver.log"; then
  echo "Error: Allowed invalid semver in Cargo.toml!"
  exit 1
else
  echo "Rejected invalid version correctly"
  cat "${TMP_DIR}/${CASE_NAME}-invalid-ver.log"
fi

echo "Case ${CASE_NAME} PASSED"
