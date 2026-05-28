#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  scripts/smoke.sh run
  scripts/smoke.sh repo <path> [passthrough args...]
  scripts/smoke.sh clone <url> <name>
  scripts/smoke.sh add-submodule <url> <name>
  scripts/smoke.sh add-case <name> <repo> [passthrough args...]
  scripts/smoke.sh add-managed-submodule <url> <name> [passthrough args...]
  scripts/smoke.sh purge <name>
  scripts/smoke.sh update <case-name>
USAGE
}

binary_path() {
  cargo build --quiet --bin toven
  printf '%s/target/debug/toven\n' "$ROOT"
}

generated_config() {
  local name="$1"
  local repo="$2"
  local config="$3"

  cat >"$config" <<EOF
[workspace]
name = "$name"
root = "$repo"

[profiles.rust]
language = "rust"
execution = "batch-ready"
module_arg_template = ["-p", "{module.package}"]
resource_group = "cargo:{workspace.root}"

[profiles.rust.tasks]
test = { argv = ["cargo", "test", "{module.args}", "{args}"] }
EOF
}

run_repo() {
  local repo="${1:-}"
  if [[ -z "$repo" ]]; then
    echo "error: PATH is required" >&2
    usage >&2
    exit 2
  fi
  shift || true

  repo="$(cd "$repo" && pwd)"
  local config="$repo/toven.toml"
  local temp_config=""
  if [[ ! -f "$config" ]]; then
    temp_config="$(mktemp "${TMPDIR:-/tmp}/toven-smoke.XXXXXX.toml")"
    generated_config "$(basename "$repo")" "$repo" "$temp_config"
    config="$temp_config"
    echo "warning: $repo has no toven.toml; using generated Rust planning config for smoke only" >&2
  fi

  local bin
  bin="$(binary_path)"
  "$bin" plan --config "$config" --task test -- "$@"

  if [[ -n "$temp_config" ]]; then
    rm -f "$temp_config"
  fi
}

clone_repo() {
  local url="${1:-}"
  local name="${2:-}"
  if [[ -z "$url" || -z "$name" ]]; then
    echo "error: URL and NAME are required" >&2
    usage >&2
    exit 2
  fi
  mkdir -p "$ROOT/.toven/smoke/repos"
  git clone "$url" "$ROOT/.toven/smoke/repos/$name"
}

add_submodule() {
  local url="${1:-}"
  local name="${2:-}"
  if [[ -z "$url" || -z "$name" ]]; then
    echo "error: URL and NAME are required" >&2
    usage >&2
    exit 2
  fi
  mkdir -p "$ROOT/smoke/repos"
  git submodule add "$url" "$ROOT/smoke/repos/$name"
}

write_case() {
  local name="$1"
  local repo="$2"
  shift 2

  mkdir -p "$ROOT/smoke/cases" "$ROOT/smoke/expected"
  {
    printf 'name = "%s"\n' "$name"
    printf 'repo = "%s"\n' "$repo"
    printf 'task = "test"\n'
    printf 'args = ['
    local first=1
    for arg in "$@"; do
      if [[ "$first" -eq 0 ]]; then
        printf ', '
      fi
      first=0
      printf '"%s"' "$arg"
    done
    printf ']\n'
    printf 'expected = "smoke/expected/%s.plan.txt"\n' "$name"
  } >"$ROOT/smoke/cases/$name.toml"
}

add_case() {
  local name="${1:-}"
  local repo="${2:-}"
  if [[ -z "$name" || -z "$repo" ]]; then
    echo "error: NAME and REPO are required" >&2
    usage >&2
    exit 2
  fi
  shift 2

  write_case "$name" "$repo" "$@"
  TOVEN_SMOKE_BLESS=1 update_case "$name"
}

add_managed_submodule() {
  local url="${1:-}"
  local name="${2:-}"
  if [[ -z "$url" || -z "$name" ]]; then
    echo "error: URL and NAME are required" >&2
    usage >&2
    exit 2
  fi
  shift 2

  add_submodule "$url" "$name"
  add_case "$name" "smoke/repos/$name" "$@"
}

purge_repo() {
  local name="${1:-}"
  if [[ -z "$name" ]]; then
    echo "error: NAME is required" >&2
    usage >&2
    exit 2
  fi

  if [[ -d "$ROOT/.toven/smoke/repos/$name" ]]; then
    rm -rf "$ROOT/.toven/smoke/repos/$name"
  fi

  if [[ -e "$ROOT/smoke/repos/$name" ]]; then
    git submodule deinit -f -- "smoke/repos/$name" 2>/dev/null || true
    git rm -f -- "smoke/repos/$name"
    rm -rf "$ROOT/.git/modules/smoke/repos/$name"
  fi

  rm -f "$ROOT/smoke/cases/$name.toml" "$ROOT/smoke/expected/$name.plan.txt"
}

update_case() {
  local name="${1:-}"
  if [[ -z "$name" ]]; then
    echo "error: NAME is required" >&2
    usage >&2
    exit 2
  fi
  if [[ "${TOVEN_SMOKE_BLESS:-}" != "1" ]]; then
    echo "error: set TOVEN_SMOKE_BLESS=1 to update managed smoke expectations" >&2
    exit 2
  fi
  TOVEN_SMOKE_UPDATE=1 TOVEN_SMOKE_CASE="$name" cargo test --test smoke --all-features
  git --no-pager diff -- "smoke/expected/$name.plan.txt"
}

case "${1:-}" in
  run)
    cargo test --test smoke --all-features
    ;;
  repo)
    shift
    run_repo "$@"
    ;;
  clone)
    shift
    clone_repo "$@"
    ;;
  add-submodule)
    shift
    add_submodule "$@"
    ;;
  add-case)
    shift
    add_case "$@"
    ;;
  add-managed-submodule)
    shift
    add_managed_submodule "$@"
    ;;
  purge)
    shift
    purge_repo "$@"
    ;;
  update)
    shift
    update_case "$@"
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
