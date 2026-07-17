---
name: validate
description: >-
    Build, test, lint, format-check, and structure-check Toven changes through cargo and make —
    scoped to the crates that actually changed. Use whenever you need to validate a Toven change,
    run tests for a crate, reproduce CI locally, or check the blast radius of an edit before
    committing.
user-invocable: true
---

# Validating Toven changes with cargo/make

Toven is one hexagonal Cargo workspace (`members = ["crates/*"]`, `exclude = ["rskit"]`) on top of the vendored `rskit/` submodule. The `Makefile` runs the canonical gates, but **they run against the whole workspace** (the cargo-based ones — `lint`, `test`, `doc` — with `--all-features`) — so to stay scoped to a changed crate, drive `cargo` directly with `-p <crate>`. Always scope to what changed; the full-workspace gates are for audits and CI sign-off.

## Prerequisite: initialize the submodule

rskit lives in the `rskit/` submodule; nothing builds without it:

```bash
git submodule update --init --recursive
```

## Golden rule: scope to what changed with `cargo -p`

```bash
cargo clippy -p <crate> --all-targets --all-features -- -D warnings   # e.g. -p toven-engine
cargo test   -p <crate> --all-features -q
cargo test   -p <crate> --all-features <test-name-filter> -q          # a single test / module
```

Crates: `toven-model` (L0), `toven-ports` (L1), `toven-engine` / `toven-rust` / `toven-go` / `toven-command` (L2), `toven-cli` (L3), and dev-only `toven-testkit`.

## Whole-tree gates (fast ones are fine per change)

| Intent | Command | Notes |
|---|---|---|
| Format (check) | `make fmt-check` | fast, whole-tree |
| Format (write) | `make fmt` | rustfmt |
| Structure guard | `make structure` | `lib.rs`/`mod.rs` declare-only + placement (cheap, run on structural changes) |
| Lint | `make lint` | clippy `-D warnings`, `--workspace` |
| Test | `make test` | nextest + doctests, `--workspace` (needs cargo-nextest) |
| Doc | `make doc` | `-D warnings` |
| Deny | `make deny` | cargo-deny: licenses, advisories, sources |
| Benchmark | `make benchmark` | required evidence for any performance claim |
| Full gate | `make check` | fmt-check + lint + test + structure + doc + deny + release build |

`make fmt-check` and `make structure` are cheap enough to run on every change; prefer `cargo -p` for lint/test during iteration and reserve `make lint`/`make test`/`make check` for audits or a final sign-off.

## Before you hand work off

For a self-contained change, the minimum green bar is: `cargo clippy -p <crate> --all-features -- -D warnings`, `cargo test -p <crate> --all-features` (deterministic, no real network), `make fmt-check`, and `make structure` on any structural change. Escalate to `make check` only when the change is genuinely tree-wide or you are preparing a release. Back any performance claim with `make benchmark`.

Treat a green run as **necessary but not sufficient**: it does not catch layering-by-convention, cascade gaps (a model change not flowed through planner/executor/output/docs), rskit-reuse violations, silent argv rewrites, or weak tests. Those are on the reviewer.

Per repo workflow, **create the branch and make edits only** — the maintainer commits and pushes.
