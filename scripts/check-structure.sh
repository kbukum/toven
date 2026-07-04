#!/usr/bin/env bash
# Structure guard (development principles §4): `mod.rs` files declare and re-export
# only — they must never contain logic or private items. Applied to every crate
# under `crates/*/src`. Attribute-only lines (e.g. `#[cfg(unix)]`) are permitted
# because they annotate a following declare/re-export without introducing logic.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fail=0

while IFS= read -r file; do
  invalid_lines="$(awk '
    /^[[:space:]]*$/ { next }
    /^[[:space:]]*\/\/!/ { next }
    /^[[:space:]]*\/\/\// { next }
    /^[[:space:]]*#\[.*\][[:space:]]*$/ { next }
    /^[[:space:]]*(pub([[:space:]]*\([^)]*\))?[[:space:]]+)?mod[[:space:]]+[A-Za-z_][A-Za-z0-9_]*;[[:space:]]*$/ { next }
    /^[[:space:]]*pub([[:space:]]*\([^)]*\))?[[:space:]]+use[[:space:]].+;[[:space:]]*$/ { next }
    /^[[:space:]]*pub([[:space:]]*\([^)]*\))?[[:space:]]+use[[:space:]].+\{[[:space:]]*$/ { next }
    /^[[:space:]]*[A-Za-z_][A-Za-z0-9_:]*(,[[:space:]]*[A-Za-z_][A-Za-z0-9_:]*)*,?[[:space:]]*$/ { next }
    /^[[:space:]]*\};[[:space:]]*$/ { next }
    { print }
  ' "$file")"
  if [ -n "$invalid_lines" ]; then
    printf 'mod.rs contains logic or private items: %s\n%s\n' "${file#"$root"/}" "$invalid_lines" >&2
    fail=1
  fi
done < <(find "$root/crates" -path '*/src/*' -name mod.rs -type f | sort)

exit "$fail"
