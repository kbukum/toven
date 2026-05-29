#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fail=0

while IFS= read -r file; do
  invalid_lines="$(awk '
    /^[[:space:]]*$/ { next }
    /^[[:space:]]*\/\/!/ { next }
    /^[[:space:]]*\/\/\// { next }
    /^[[:space:]]*(pub([[:space:]]*\([^)]*\))?[[:space:]]+)?mod[[:space:]]+[A-Za-z_][A-Za-z0-9_]*;[[:space:]]*$/ { next }
    /^[[:space:]]*pub([[:space:]]*\([^)]*\))?[[:space:]]+use[[:space:]].+;[[:space:]]*$/ { next }
    /^[[:space:]]*pub([[:space:]]*\([^)]*\))?[[:space:]]+use[[:space:]].+\{[[:space:]]*$/ { next }
    /^[[:space:]]*[A-Za-z_][A-Za-z0-9_:]*(,[[:space:]]*[A-Za-z_][A-Za-z0-9_:]*)*,?[[:space:]]*$/ { next }
    /^[[:space:]]*\};[[:space:]]*$/ { next }
    { print }
  ' "$file")"
  if [ -n "$invalid_lines" ]; then
    printf 'mod.rs contains logic or private items: %s\n%s\n' "${file#$root/}" "$invalid_lines" >&2
    fail=1
  fi
done < <(find "$root/src" -name mod.rs -type f | sort)

check_forbidden() {
  local dir="$1"
  local pattern="$2"
  local matches

  if [ ! -d "$root/$dir" ]; then
    return
  fi

  matches="$(grep -R --include='*.rs' -nE "$pattern" "$root/$dir" || true)"
  if [ -n "$matches" ]; then
    printf 'forbidden upward import under %s:\n%s\n' "$dir" "$matches" >&2
    fail=1
  fi
}

check_forbidden "src/core" 'crate::(config|lang|preset|engine|exec|report|cli)::'
check_forbidden "src/config" 'crate::(engine|exec|report|cli)::'
check_forbidden "src/lang" 'crate::(config|engine|exec|report|cli)::'
check_forbidden "src/preset" 'crate::(engine|exec|report|cli)::'
check_forbidden "src/engine" 'crate::(report|cli)::'
check_forbidden "src/exec" 'crate::(report|cli)::'
check_forbidden "src/report" 'crate::cli::'

exit "$fail"
