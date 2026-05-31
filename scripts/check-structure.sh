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
  local modules="$2"
  local matches
  local grouped_matches

  if [ ! -d "$root/$dir" ]; then
    return
  fi

  matches="$(grep -R --include='*.rs' -nE "crate::(${modules})(::|[[:space:]]+as|[[:space:];,{])" "$root/$dir" || true)"
  if [ -n "$matches" ]; then
    printf 'forbidden upward import under %s:\n%s\n' "$dir" "$matches" >&2
    fail=1
  fi

  grouped_matches="$(
    while IFS= read -r -d '' file; do
      FORBIDDEN_MODULES="$modules" perl -0ne '
        BEGIN { $modules = $ENV{"FORBIDDEN_MODULES"}; }
        sub check_group {
          my ($body) = @_;
          my $depth = 0;
          my $entry = "";
          for my $index (0 .. length($body)) {
            my $char = $index == length($body) ? "," : substr($body, $index, 1);
            if ($char eq "{" ) {
              $depth++;
            } elsif ($char eq "}") {
              $depth--;
            }

            if ($char eq "," && $depth == 0) {
              $entry =~ s/^\s+|\s+$//g;
              if ($entry =~ /^($modules)(?:::|\s+as\b|\s*$)/m) {
                return $1;
              }
              $entry = "";
            } else {
              $entry .= $char;
            }
          }
          return;
        }

        my $offset = 0;
        while ((my $start = index($_, "crate::{", $offset)) >= 0) {
          my $body_start = $start + length("crate::{");
          my $depth = 1;
          my $index = $body_start;
          for (; $index < length($_); $index++) {
            my $char = substr($_, $index, 1);
            if ($char eq "{") {
              $depth++;
            } elsif ($char eq "}") {
              $depth--;
              last if $depth == 0;
            }
          }
          last if $depth != 0;

          my $body = substr($_, $body_start, $index - $body_start);
          if (my $module = check_group($body)) {
            print "$ARGV: grouped crate import contains forbidden module $module\n";
            last;
          }
          $offset = $index + 1;
        }
      ' "$file"
    done < <(find "$root/$dir" -name '*.rs' -type f -print0)
  )"
  if [ -n "$grouped_matches" ]; then
    printf 'forbidden grouped upward import under %s:\n%s\n' "$dir" "$grouped_matches" >&2
    fail=1
  fi
}

check_forbidden "src/core" 'config|adapter|preset|engine|exec|report|cli'
check_forbidden "src/config" 'engine|exec|report|cli'
check_forbidden "src/adapter" 'config|engine|exec|report|cli'
check_forbidden "src/preset" 'engine|exec|report|cli'
check_forbidden "src/engine" 'report|cli'
check_forbidden "src/exec" 'report|cli'
check_forbidden "src/report" 'cli'

exit "$fail"
