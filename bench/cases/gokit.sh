#!/usr/bin/env bash
# Benchmark case: compare Toven orchestration against native `go test` over the
# sibling gokit Go workspace.
#
# Sourced by scripts/benchmark.sh, which provides $ROOT, $TOVEN_BIN, and the
# per-iteration BENCH_* environment. The case declares the approaches (the
# commands under comparison), the scenarios (repository states), and the hooks
# that reset/mutate the target repo between iterations.
#
# Where the rskit case measures the Rust `Batchable` collapse (one cargo
# invocation cargo parallelizes internally), this case measures the Go
# `PerModule` fan-out: Toven spawns one `go`/lint/vuln process per module and
# parallelizes them across its own worker pool. That shape is where global
# compute-budget oversubscription shows up — left unbounded, every spawned `go`
# defaults its internal `GOMAXPROCS` to the full core count, so `max_parallel`
# modules × per-process parallelism approaches cores² threads.
#
# The compute-budget feature now bounds that by default: `compute_budget = "auto"`
# splits a host-sized thread budget across the concurrently running units and
# injects each share as `GOMAXPROCS`. So the plain `toven test` below is already
# bounded — it is the shipping default. The A/B that isolates the feature's win
# is default `auto` against an explicit opt-out:
#
#   toven_test           default `compute_budget = "auto"` (bounded) — SHIPPING
#   toven_test_inherit   `--compute-budget inherit` — old unbounded behavior (A/B)
#   toven_test_budget    `--compute-budget <N>` — explicit fixed total budget
#   toven_test_jobs      Toven worker pool narrowed via `--jobs`
#
# The budget is injected as an environment variable, never added to argv.
#
# Every Toven approach drives the installed `toven` binary against gokit's own
# `toven.toml`. Changed-module selection is the global `--base <ref>` baseline;
# `toven <task>` without a baseline plans every module.

CASE_NAME="gokit"
TARGET_REPO="${TARGET_REPO:-$ROOT/../gokit}"
BASE_REF="${BASE_REF:-HEAD}"
OUTPUT_DIR="${OUTPUT_DIR:-$ROOT/bench/out/gokit}"
WARMUPS="${WARMUPS:-1}"
ITERATIONS="${ITERATIONS:-3}"

# Modules the native per-module comparison drives directly (a representative
# leaf + a widely-imported shared module), and the source files the mutation
# scenarios touch. Overridable so the case can be pointed at a different repo
# shape without editing the file.
GOKIT_BENCH_MODULES=(${GOKIT_BENCH_MODULES:-errors util})
GOKIT_LEAF_SOURCE="${GOKIT_LEAF_SOURCE:-errors/errors.go}"
GOKIT_SHARED_SOURCE="${GOKIT_SHARED_SOURCE:-util/bytes.go}"
GOKIT_SHARED_INPUT="${GOKIT_SHARED_INPUT:-go.work}"
GOKIT_TOVEN_CONFIG="${GOKIT_TOVEN_CONFIG:-toven.toml}"

# The explicit fixed total budget for the flag probe. The engine splits this
# total across the concurrently running units, so a total near (worker-pool
# width) × (a small per-process share) keeps peak thread pressure close to the
# core count instead of cores².
GOKIT_BENCH_JOBS="${GOKIT_BENCH_JOBS:-4}"
GOKIT_BENCH_BUDGET="${GOKIT_BENCH_BUDGET:-12}"

APPROACHES=(
  go_test_modules
  toven_test
  toven_test_inherit
  toven_test_budget
  toven_test_jobs
  toven_test_changed
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
  require_target_file "$GOKIT_LEAF_SOURCE"
  require_target_file "$GOKIT_SHARED_SOURCE"
  require_target_file "$GOKIT_SHARED_INPUT"
  if [[ ! -f "$TARGET_REPO/$GOKIT_TOVEN_CONFIG" ]]; then
    echo "error: gokit benchmark requires '$TARGET_REPO/$GOKIT_TOVEN_CONFIG'" >&2
    echo "hint: complete gokit adoption with installed 'toven generate' before running this benchmark case" >&2
    exit 2
  fi
  if ! command -v go >/dev/null 2>&1; then
    echo "error: gokit benchmark requires the 'go' toolchain on PATH" >&2
    exit 2
  fi
  local module
  for module in "${GOKIT_BENCH_MODULES[@]}"; do
    require_target_file "$module/go.mod"
  done
}

append_mutation() {
  local path="$1"
  local comment="#"
  require_target_file "$path"
  if [[ "$path" == *.go ]]; then
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
    if [[ -f "$GOKIT_TOVEN_CONFIG" ]]; then
      "$TOVEN_BIN" cache clean >/dev/null
    else
      rm -rf .toven/cache
    fi
  )
}

# Per-approach isolated Go build/test cache so the `cold` scenario truly starts
# from an empty compiler cache — the Go analog of the rskit case's private
# CARGO_TARGET_DIR.
bench_gocache_dir() {
  printf '%s/gocache/%s/%s' "$BENCH_RUN_DIR" "$BENCH_SCENARIO" "$BENCH_APPROACH"
}

clean_go_cache() {
  rm -rf "$(bench_gocache_dir)"
}

prepare_cold() {
  reset_target_repo
}

before_iteration_cold() {
  reset_target_repo
  clean_toven_cache
  clean_go_cache
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
  append_mutation "$GOKIT_LEAF_SOURCE"
}

restore_leaf_source_change() {
  reset_target_repo
}

prepare_shared_dependency_change() {
  reset_target_repo
}

before_iteration_shared_dependency_change() {
  reset_target_repo
  append_mutation "$GOKIT_SHARED_SOURCE"
}

restore_shared_dependency_change() {
  reset_target_repo
}

prepare_shared_input_change() {
  reset_target_repo
}

before_iteration_shared_input_change() {
  reset_target_repo
  append_mutation "$GOKIT_SHARED_INPUT"
}

restore_shared_input_change() {
  reset_target_repo
}

prepare_toven_config_change() {
  reset_target_repo
  require_target_file "$GOKIT_TOVEN_CONFIG"
}

before_iteration_toven_config_change() {
  reset_target_repo
  append_mutation "$GOKIT_TOVEN_CONFIG"
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

run_go_test_modules() {
  (
    cd "$TARGET_REPO"
    for module in "${GOKIT_BENCH_MODULES[@]}"; do
      GOCACHE="$(bench_gocache_dir)/go" \
        go -C "$module" test ./...
    done
  )
}

run_toven_test() {
  (
    cd "$TARGET_REPO"
    GOCACHE="$(bench_gocache_dir)/go" \
      "$TOVEN_BIN" test
  )
}

# A/B baseline: restore the old unbounded behavior with `--compute-budget
# inherit`, so every spawned `go` inherits the full-core GOMAXPROCS. Compared
# against the default (auto-bounded) `run_toven_test`, this isolates the
# feature's win.
run_toven_test_inherit() {
  (
    cd "$TARGET_REPO"
    GOCACHE="$(bench_gocache_dir)/go" \
      "$TOVEN_BIN" test --compute-budget inherit
  )
}

# Explicit fixed total budget through the flag: the engine splits it across the
# wave and injects each share as GOMAXPROCS (never argv).
run_toven_test_budget() {
  (
    cd "$TARGET_REPO"
    GOCACHE="$(bench_gocache_dir)/go" \
      "$TOVEN_BIN" test --compute-budget "$GOKIT_BENCH_BUDGET"
  )
}

# Oversubscription probe: narrow Toven's own worker pool instead of the
# per-process budget.
run_toven_test_jobs() {
  (
    cd "$TARGET_REPO"
    GOCACHE="$(bench_gocache_dir)/go" \
      "$TOVEN_BIN" test --jobs "$GOKIT_BENCH_JOBS"
  )
}

run_toven_test_changed() {
  (
    cd "$TARGET_REPO"
    GOCACHE="$(bench_gocache_dir)/go" \
      "$TOVEN_BIN" test --base "$BASE_REF" --output jsonl
  )
}
