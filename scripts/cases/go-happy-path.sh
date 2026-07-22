#!/usr/bin/env bash
# Case: Go Release Happy Path and Tag Verification
set -euo pipefail

CASE_NAME="go-happy-path"

if [ -n "${REAL_GO}" ]; then
  GO_FIXTURE="${ROOT_DIR}/tests/fixtures/go-release-repository/multi-module"
  GO_WORK_DIR="${TMP_DIR}/go-happy-path"
  cp -r "${GO_FIXTURE}" "${GO_WORK_DIR}"

  GO_BARE="${TMP_DIR}/go-happy-path-bare.git"
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
  "${REAL_GIT}" -C "${GO_BARE}" symbolic-ref HEAD refs/heads/main

  # Add initial tags
  "${REAL_GIT}" tag "v1.0.0"
  "${REAL_GIT}" tag "nested/v1.1.0"
  "${REAL_GIT}" push --tags origin

  # Plan should be up-to-date
  "${TOVEN_BIN}" release plan > "${TMP_DIR}/${CASE_NAME}-clean.log"
  grep -q "Release plan" "${TMP_DIR}/${CASE_NAME}-clean.log"

  # Make changes
  echo "// update" >> "${GO_WORK_DIR}/main.go"
  "${REAL_GIT}" commit -am "modify root module"

  # Plan should propose release bump
  "${TOVEN_BIN}" release plan > "${TMP_DIR}/${CASE_NAME}-changed.log"
  grep -q "go-release-repository" "${TMP_DIR}/${CASE_NAME}-changed.log"

  # Run tag mutation
  "${TOVEN_BIN}" release tag --yes --allow-dirty --offline
  "${REAL_GIT}" tag | grep "v1.0.1"
  ! "${REAL_GIT}" tag | grep "testmod/v"
else
  echo "Skipping Go Happy Path: no go toolchain found on PATH"
fi

echo "Case ${CASE_NAME} PASSED"
