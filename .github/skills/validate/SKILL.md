---
name: validate
description: "toven: Run the repository's build, test, lint, and documentation gates scoped to the change."
user-invocable: true
---

# Validate

Run the smallest checks that cover the change, then the full gate when the change is cross-cutting or release-bound. The [gates section](../../../docs/engineering.md#gates) says what each gate checks; the `Makefile` is the source of truth for recipes.

## Setup

```bash
git submodule update --init --recursive
make doctor   # fails if a tool the gates need is missing
```

## Scoped checks while iterating

The `make` gates run the whole workspace. Scope to the touched crates with `cargo -p`; crate names and layers are in [architecture](../../../docs/architecture.md#workspace-layers).

```bash
cargo clippy -p <crate> --all-targets --all-features -- -D warnings
cargo nextest run -p <crate> --all-features
cargo nextest run -p <crate> --all-features <filter>   # one test or module
cargo test -p <crate> --doc --all-features
make fmt-check
make structure   # when files, modules, or subprocess code changed
```

## Pick the gate

| Change | Run |
|---|---|
| Prose-only Markdown | `make docs-build` for book pages, plus a link check. No Rust build. |
| One or a few crates | The scoped checks above |
| Public API, workspace `Cargo.toml`, shared types, or several layers | `make check` |
| Dependencies or `Cargo.lock` | `make deny`, then `make check` |
| Coverage-sensitive behavior | `make coverage` |
| Performance claim | `make benchmark CASE=bench/cases/<case>.sh` |
| Release, packaging, or CI | `make check`, `make release-plan`, and `make act-ci` or the matching `act-*` target |

List every target with `grep -E '^[a-z-]+:' Makefile`.

## Report

Record each command you ran and its result. Name checks you skipped and why; never claim a skipped check passed. A green gate does not cover rules marked **Review** in the engineering standard; those need the [review skill](../review/SKILL.md).

Validation does not authorize Git changes. Commit, push, or create branches only when asked.
