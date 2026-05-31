#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/benchmark.sh <case-file>

Case files are shell fragments that declare CASE_NAME, TARGET_REPO, APPROACHES,
SCENARIOS, WARMUPS, ITERATIONS, and run_<approach>/prepare_<scenario> functions.
They may also declare before_iteration_<scenario>/after_iteration_<scenario>
hooks for scenarios that need per-iteration reset or mutation.
USAGE
}

timestamp_ms() {
  perl -MTime::HiRes=time -e 'printf "%.0f\n", time() * 1000'
}

require_function() {
  local name="$1"
  if ! declare -F "$name" >/dev/null; then
    echo "error: benchmark case is missing function $name" >&2
    exit 2
  fi
}

call_function_if_exists() {
  local name="$1"
  if declare -F "$name" >/dev/null; then
    "$name"
  fi
}

resolve_installed_toven() {
  local bin
  bin="$(command -v toven || true)"
  if [[ -z "$bin" ]]; then
    echo "error: installed 'toven' binary was not found on PATH" >&2
    echo "hint: run 'cargo install --path . --locked --force' before benchmarking" >&2
    exit 2
  fi
  if [[ -n "${TOVEN_EXPECTED_BIN_PREFIX:-}" && "$bin" != "$TOVEN_EXPECTED_BIN_PREFIX"* ]]; then
    echo "error: resolved toven binary '$bin' is outside TOVEN_EXPECTED_BIN_PREFIX='$TOVEN_EXPECTED_BIN_PREFIX'" >&2
    exit 2
  fi
  printf '%s\n' "$bin"
}

sha256_file() {
  local file="$1"
  if command -v shasum >/dev/null; then
    shasum -a 256 "$file" | awk '{print $1}'
  elif command -v sha256sum >/dev/null; then
    sha256sum "$file" | awk '{print $1}'
  else
    printf 'unavailable'
  fi
}

write_metadata() {
  local output="$1"
  local toven_bin="$2"
  {
    printf 'case=%s\n' "$CASE_NAME"
    printf 'target_repo=%s\n' "$TARGET_REPO"
    printf 'target_repo_sha=%s\n' "$(git -C "$TARGET_REPO" rev-parse HEAD)"
    printf 'toven_bin=%s\n' "$toven_bin"
    printf 'toven_bin_sha256=%s\n' "$(sha256_file "$toven_bin")"
    printf 'toven_version=%s\n' "$("$toven_bin" --version)"
    printf 'toven_git_sha=%s\n' "$(git -C "$ROOT" rev-parse HEAD)"
    printf 'os=%s\n' "$(uname -a)"
    if [[ "$(uname -s)" == "Darwin" ]]; then
      printf 'cpu=%s\n' "$(sysctl -n machdep.cpu.brand_string 2>/dev/null || true)"
    elif [[ -r /proc/cpuinfo ]]; then
      printf 'cpu=%s\n' "$(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ //')"
    fi
    printf 'rustc=%s\n' "$(rustc --version 2>/dev/null || printf 'unavailable')"
    printf 'cargo=%s\n' "$(cargo --version 2>/dev/null || printf 'unavailable')"
    printf 'nextest=%s\n' "$(cargo nextest --version 2>/dev/null || printf 'unavailable')"
  } >"$output"
}

write_summary() {
  local input="$1"
  local output="$2"

  perl -MList::Util=min,max -e '
    use strict;
    use warnings;
    my ($input, $output) = @ARGV;
    open my $in, "<", $input or die "failed to read $input: $!";
    <$in>;
    my %groups;
    while (my $line = <$in>) {
      chomp $line;
      my ($scenario, $approach, $phase, $iteration, $status, $duration) = split /,/, $line;
      my $key = join "\x1f", $scenario, $approach, $phase;
      push @{ $groups{$key}{durations} }, 0 + $duration;
      $groups{$key}{runs}++;
      if ($status == 0) {
        $groups{$key}{passed}++;
      } else {
        $groups{$key}{failed}++;
      }
    }
    open my $out, ">", $output or die "failed to write $output: $!";
    print {$out} "scenario,approach,phase,runs,passed,failed,min_ms,median_ms,max_ms\n";
    for my $key (sort keys %groups) {
      my ($scenario, $approach, $phase) = split /\x1f/, $key;
      my @durations = sort { $a <=> $b } @{ $groups{$key}{durations} };
      my $mid = int(@durations / 2);
      my $median = @durations % 2
        ? $durations[$mid]
        : int(($durations[$mid - 1] + $durations[$mid]) / 2);
      printf {$out} "%s,%s,%s,%d,%d,%d,%d,%d,%d\n",
        $scenario,
        $approach,
        $phase,
        $groups{$key}{runs} // 0,
        $groups{$key}{passed} // 0,
        $groups{$key}{failed} // 0,
        min(@durations),
        $median,
        max(@durations);
    }
  ' "$input" "$output"
}

record_iteration() {
  local scenario="$1"
  local approach="$2"
  local iteration="$3"
  local measured="$4"
  local output_dir="$5"
  local command="run_${approach}"
  local before_hook="before_iteration_${scenario}"
  local after_hook="after_iteration_${scenario}"
  local phase

  if [[ "$measured" == "1" ]]; then
    phase="measured"
  else
    phase="warmup"
  fi

  local log_prefix="$output_dir/logs/${scenario}.${approach}.${phase}.${iteration}"
  local started
  local finished
  local status=0

  export BENCH_PHASE="$phase"
  export BENCH_ITERATION="$iteration"
  call_function_if_exists "$before_hook"
  started="$(timestamp_ms)"
  "$command" >"${log_prefix}.stdout" 2>"${log_prefix}.stderr" || status=$?
  finished="$(timestamp_ms)"
  call_function_if_exists "$after_hook"

  printf '%s,%s,%s,%s,%s,%s\n' \
    "$scenario" "$approach" "$phase" "$iteration" "$status" "$((finished - started))" \
    >>"$output_dir/results.csv"
}

main() {
  local case_file="${1:-}"
  if [[ -z "$case_file" ]]; then
    usage >&2
    exit 2
  fi
  if [[ ! -f "$case_file" ]]; then
    echo "error: case file not found: $case_file" >&2
    exit 2
  fi

  # shellcheck source=/dev/null
  source "$case_file"

  : "${CASE_NAME:?CASE_NAME is required}"
  : "${TARGET_REPO:?TARGET_REPO is required}"
  : "${OUTPUT_DIR:?OUTPUT_DIR is required}"
  : "${WARMUPS:?WARMUPS is required}"
  : "${ITERATIONS:?ITERATIONS is required}"

  TARGET_REPO="$(cd "$TARGET_REPO" && pwd)"
  if [[ -n "$(git -C "$TARGET_REPO" status --porcelain)" ]]; then
    echo "error: target repository has uncommitted changes: $TARGET_REPO" >&2
    exit 2
  fi
  if [[ -n "${BASE_REF:-}" ]]; then
    git -C "$TARGET_REPO" rev-parse --verify "$BASE_REF" >/dev/null
  fi

  local toven_bin
  toven_bin="$(resolve_installed_toven)"
  call_function_if_exists preflight_case

  local run_dir="$OUTPUT_DIR/$(date +%Y%m%d-%H%M%S)"
  mkdir -p "$run_dir/logs"
  printf 'scenario,approach,phase,iteration,status,duration_ms\n' >"$run_dir/results.csv"
  write_metadata "$run_dir/metadata.env" "$toven_bin"

  for scenario in "${SCENARIOS[@]}"; do
    require_function "prepare_${scenario}"
    require_function "restore_${scenario}"
    for approach in "${APPROACHES[@]}"; do
      require_function "run_${approach}"
      export BENCH_RUN_DIR="$run_dir"
      export BENCH_SCENARIO="$scenario"
      export BENCH_APPROACH="$approach"
      "prepare_${scenario}"
      for ((iteration = 1; iteration <= WARMUPS; iteration++)); do
        record_iteration "$scenario" "$approach" "$iteration" 0 "$run_dir"
      done
      for ((iteration = 1; iteration <= ITERATIONS; iteration++)); do
        record_iteration "$scenario" "$approach" "$iteration" 1 "$run_dir"
      done
      "restore_${scenario}"
    done
  done

  write_summary "$run_dir/results.csv" "$run_dir/summary.csv"
  echo "benchmark output: $run_dir"
}

main "$@"
