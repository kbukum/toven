#!/usr/bin/env bash

CASE_NAME="rskit"
TARGET_REPO="${TARGET_REPO:-$ROOT/rskit}"
BASE_REF="${BASE_REF:-HEAD}"
OUTPUT_DIR="${OUTPUT_DIR:-$ROOT/bench/out/rskit}"
WARMUPS="${WARMUPS:-1}"
ITERATIONS="${ITERATIONS:-3}"
RSKIT_BENCH_PACKAGES=(${RSKIT_BENCH_PACKAGES:-rskit-errors rskit-config})
RSKIT_LEAF_SOURCE="${RSKIT_LEAF_SOURCE:-core/rskit-errors/src/lib.rs}"
RSKIT_SHARED_SOURCE="${RSKIT_SHARED_SOURCE:-core/rskit-util/src/lib.rs}"
RSKIT_SHARED_INPUT="${RSKIT_SHARED_INPUT:-rust-toolchain.toml}"
RSKIT_TOVEN_CONFIG="${RSKIT_TOVEN_CONFIG:-toven.toml}"

APPROACHES=(
  cargo_test_packages
  cargo_test_workspace
  cargo_nextest_packages
  cargo_nextest_workspace
  toven_test
  toven_test_affected
  toven_nextest_affected
)

SCENARIOS=(
  cold
  warm
  leaf_source_change
  shared_dependency_change
  shared_input_change
  toven_config_change
  cache_clean
)

reset_target_repo() {
  git -C "$TARGET_REPO" reset --quiet --hard HEAD
  git -C "$TARGET_REPO" clean --quiet -fd
}

require_target_file() {
  local path="$1"
  if [[ ! -f "$TARGET_REPO/$path" ]]; then
    echo "error: benchmark scenario requires '$TARGET_REPO/$path'" >&2
    exit 2
  fi
}

preflight_case() {
  require_target_file "$RSKIT_LEAF_SOURCE"
  require_target_file "$RSKIT_SHARED_SOURCE"
  require_target_file "$RSKIT_SHARED_INPUT"
  if [[ ! -f "$TARGET_REPO/$RSKIT_TOVEN_CONFIG" ]]; then
    echo "error: rskit benchmark requires '$TARGET_REPO/$RSKIT_TOVEN_CONFIG'" >&2
    echo "hint: complete rskit adoption with installed 'toven generate' before running this benchmark case" >&2
    exit 2
  fi
  if ! cargo nextest --version >/dev/null 2>&1; then
    echo "error: rskit benchmark requires cargo-nextest for nextest comparisons" >&2
    exit 2
  fi
}

append_mutation() {
  local path="$1"
  local comment="#"
  require_target_file "$path"
  if [[ "$path" == *.rs ]]; then
    comment="//"
  fi
  printf '\n%s toven benchmark mutation: %s %s %s %s\n' \
    "$comment" \
    "$BENCH_SCENARIO" "$BENCH_APPROACH" "$BENCH_PHASE" "$BENCH_ITERATION" \
    >>"$TARGET_REPO/$path"
}

clean_toven_cache() {
  (
    cd "$TARGET_REPO"
    if [[ -f "$RSKIT_TOVEN_CONFIG" ]]; then
      toven cache clean >/dev/null
    else
      rm -rf .toven/cache
    fi
  )
}

clean_cargo_target() {
  rm -rf "$TARGET_REPO/.toven/cache" "$BENCH_RUN_DIR/cargo-target/$BENCH_SCENARIO/$BENCH_APPROACH"
}

prepare_cold() {
  reset_target_repo
}

before_iteration_cold() {
  reset_target_repo
  clean_toven_cache
  clean_cargo_target
}

restore_cold() {
  reset_target_repo
}

prepare_warm() {
  reset_target_repo
}

restore_warm() {
  reset_target_repo
}

prepare_leaf_source_change() {
  reset_target_repo
}

before_iteration_leaf_source_change() {
  reset_target_repo
  append_mutation "$RSKIT_LEAF_SOURCE"
}

restore_leaf_source_change() {
  reset_target_repo
}

prepare_shared_dependency_change() {
  reset_target_repo
}

before_iteration_shared_dependency_change() {
  reset_target_repo
  append_mutation "$RSKIT_SHARED_SOURCE"
}

restore_shared_dependency_change() {
  reset_target_repo
}

prepare_shared_input_change() {
  reset_target_repo
}

before_iteration_shared_input_change() {
  reset_target_repo
  append_mutation "$RSKIT_SHARED_INPUT"
}

restore_shared_input_change() {
  reset_target_repo
}

prepare_toven_config_change() {
  reset_target_repo
  require_target_file "$RSKIT_TOVEN_CONFIG"
}

before_iteration_toven_config_change() {
  reset_target_repo
  append_mutation "$RSKIT_TOVEN_CONFIG"
}

restore_toven_config_change() {
  reset_target_repo
}

prepare_cache_clean() {
  reset_target_repo
}

before_iteration_cache_clean() {
  reset_target_repo
  clean_toven_cache
}

restore_cache_clean() {
  reset_target_repo
}

run_cargo_test_workspace() {
  (
    cd "$TARGET_REPO"
    CARGO_TARGET_DIR="$BENCH_RUN_DIR/cargo-target/$BENCH_SCENARIO/$BENCH_APPROACH" \
      cargo test --workspace
  )
}

run_cargo_test_packages() {
  (
    cd "$TARGET_REPO"
    for package in "${RSKIT_BENCH_PACKAGES[@]}"; do
      CARGO_TARGET_DIR="$BENCH_RUN_DIR/cargo-target/$BENCH_SCENARIO/$BENCH_APPROACH" \
        cargo test -p "$package"
    done
  )
}

run_cargo_nextest_workspace() {
  (
    cd "$TARGET_REPO"
    CARGO_TARGET_DIR="$BENCH_RUN_DIR/cargo-target/$BENCH_SCENARIO/$BENCH_APPROACH" \
      cargo nextest run --workspace
  )
}

run_cargo_nextest_packages() {
  (
    cd "$TARGET_REPO"
    for package in "${RSKIT_BENCH_PACKAGES[@]}"; do
      CARGO_TARGET_DIR="$BENCH_RUN_DIR/cargo-target/$BENCH_SCENARIO/$BENCH_APPROACH" \
        cargo nextest run -p "$package"
    done
  )
}

run_toven_test() {
  (
    cd "$TARGET_REPO"
    CARGO_TARGET_DIR="$BENCH_RUN_DIR/cargo-target/$BENCH_SCENARIO/$BENCH_APPROACH" \
      toven test
  )
}

run_toven_test_affected() {
  (
    cd "$TARGET_REPO"
    CARGO_TARGET_DIR="$BENCH_RUN_DIR/cargo-target/$BENCH_SCENARIO/$BENCH_APPROACH" \
      toven test --affected --base "$BASE_REF" --output jsonl
  )
}

run_toven_nextest_affected() {
  (
    cd "$TARGET_REPO"
    CARGO_TARGET_DIR="$BENCH_RUN_DIR/cargo-target/$BENCH_SCENARIO/$BENCH_APPROACH" \
      toven nextest --affected --base "$BASE_REF" --output jsonl
  )
}
