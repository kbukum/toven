# Rust Review — Plan, Clarify, Apply

Run each pass as a **separate subagent with clean context**. The orchestrator (this file) sequences them and collects findings. Do not concatenate passes into one prompt.

Mode is either **changes** (a diff: branch, commit range, `HEAD~1`) or **project** (whole tree, no diff). State the mode up front.

---

## Phase 1 — Scope

1. `git status`, `git diff --stat`, `git diff` (changes mode) or `ls crates/` + dependency map (project mode). Preserve uncommitted changes; integrate on top, never discard.
2. List the surface to review: changed crates (changes mode) or chosen crates/workspace (project mode). Note cross-cutting touches: workspace `Cargo.toml`, shared error types, public re-exports, `[lints]`.
3. Determine which passes apply via the triggers below. Skip non-applicable passes explicitly in the final report.

The reviewer judges code as written, against the rules below. PR descriptions, commit messages, or plan docs are scope hints only — never justifications.

## Phase 2 — Passes

Run **A first** (cheap, gates the rest). Then **B–F in parallel** where independent. Then **G last** (cross-references everything).

Each subagent receives: its scope, the pass spec below, and nothing else. Each returns findings in the shared format.

### Pass A — Mechanical (always runs)

Tool output only, no judgment.

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo deny check
cargo machete                          # or: cargo +nightly udeps
cargo semver-checks                    # if public API in scope
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps   # if public docs in scope
```

Report pass/fail per command with the first failure block verbatim.

### Pass B — Correctness

**Scope:** all in-scope `.rs` files.

Check: ownership and lifetimes; partial moves; `unwrap`/`expect` on fallible runtime paths (excluding tests); error context preserved through `?`/`From`; no swallowed errors (`let _ = …` without comment); panics only on documented invariants; `unsafe` blocks carry `// SAFETY:` comments; Edition 2024 `#[unsafe(no_mangle)]` wrapping where required; resource cleanup on every return path including errors; `Drop` impls don't panic.

Skip if: scope is docs-only or config-only.

### Pass C — Concurrency

**Scope:** files importing `tokio`, `async_std`, `std::thread`, `std::sync`, `futures`, `rayon`, or containing `async fn`/`.await`.

Check: no `MutexGuard`/`RefCell` borrow across `.await`; no `block_on` under tokio; `Arc<Mutex<…>>` justified vs channels or `Arc<RwLock<…>>`; CPU/blocking work uses `spawn_blocking`; structured concurrency via `JoinSet` over loose `spawn`; bounded channels; cancellation tokens honored; `Send`/`Sync` not added unsoundly.

Skip if: no async/threading surface in scope.

### Pass D — CLI surface

**Scope:** crates with a `bin` target, `clap` derives, command dispatch, output formatters.

Check: execution-only flags (`--dry-run`, `--explain`, `--fail-fast`, `--output`, `--watch`) only attach to verbs where they have effect; introspection verbs reject no-op flags; trailing flags after a bare positional are captured and re-parsed, not dropped; `--` passthrough untouched; value-taking flags reject missing values and refuse `--` as a value; defaults match docs; stdout carries only structured output a script would consume, diagnostics go to stderr; `NO_COLOR` respected; JSON modes emit one document per invocation; exit codes mean what they claim; Ctrl+C is cooperative teardown with a terminal summary, not an error variant.

Skip if: no CLI in scope.

### Pass E — Config and platform

**Scope:** config loaders, env var handling, path tests, docs describing config or env.

Check: path-shaped env vars require absolute paths or document expansion explicitly; config keys round-trip (dotted ↔ TOML table); stale or invalid example snippets removed; precedence (CLI > env > file > default) tested; path tests use `PathBuf`/`Path::join`, not hardcoded separators; `tempfile` over `/tmp/...`; platform-specific behavior uses explicit `#[cfg(...)]` with both branches exercised.

Skip if: no config, env, path, or cross-platform code in scope.

### Pass F — API surface and dependencies

**Scope:** `lib.rs`, `mod.rs`, `Cargo.toml`, anything changing `pub` items.

Check: new `pub` items intentional (prefer `pub(crate)`); `&str` over `String`, `&[T]` over `Vec<T>` in parameters where ownership isn't needed; `#[non_exhaustive]` on growable enums/structs; `#[must_use]` on result-like and builder types; new deps justified; `default-features = false` where applicable; no avoidable version skew; `Cargo.lock` committed; release profile reviewed if changed; `rust-version` (MSRV) declared and CI-checked; crate uses `edition = "2024"` unless justified; lints live in `[lints]` table.

Skip if: no public items, deps, or `Cargo.toml` in scope.

### Pass G — Tests, docs, semantics (runs last)

**Scope:** the in-scope code plus findings from A–F.

Check: behavioral code in scope has tests covering it (changes mode: in the same diff; project mode: anywhere in the tree); bug fixes have regression tests that would fail without them; failure paths asserted, not just happy paths; tests don't depend on wall clock, network, or working directory unless intentional; snapshot tests reviewed; output-format tests assert exact direction/escaping/ordering, not loose `contains`; an operation does what its name implies (introspection projects real data, not a synthesized stand-in); README, `--help`, doc comments, examples, and changelog match implemented behavior; removed flags actually removed from docs; doc tests compile; `#![warn(missing_docs)]` on public crates.

Always runs.

## Phase 3 — Consolidate

Orchestrator collects findings into one table:

```
pass | severity (blocker/should-fix/nit) | file:line | finding | suggested fix
```

Severity rule: **blocker** = principle violation, behavior is wrong, or a contract is broken. Otherwise should-fix or nit.

Group by file in the final report. State explicitly any pass that was **skipped** (with the trigger that failed) and any pass that was **deferred** (with reason).

## Phase 4 — Plan and clarify

Group findings by pass, order by severity. For each group write a one-line fix plan: what changes, where, how it's verified. Flag ambiguities (behavior change vs strict fix, breaking API vs deprecation, doc-only vs behavior-aligning) with a proposed default and the alternative. **Pause for user confirmation before editing.**

## Phase 5 — Apply

After confirmation:

1. Apply fixes in plan order, one pass per commit where reasonable.
2. Re-run the matching pass's validation after each fix. Stop and report if anything fails.
3. Final step: re-run Pass A across in-scope crates. Push.

## Reviewer notes

- Code judges itself. External narrative (PR description, commit message, plan doc) is scope only, not justification.
- Detection commands (`rg`, `cargo`) are loaded by the subagent when it searches, not held in the resident prompt.
- If scope is trivial (docs-only, single-line fix), run only A and G; skip the rest with explicit reason.