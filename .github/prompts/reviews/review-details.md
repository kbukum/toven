# Rust PR Review — Plan, Clarify, Apply

You are reviewing a Rust pull request in this repository. Work in three phases. Do not skip phases. Do not start editing during Phase 1 or 2. Do not trigger or retrigger Copilot review at any point.

---

## Phase 1 — Review (read-only)

Gather full context before forming opinions.

1. Run `git status`, `git diff`, and `git diff --stat` on the working tree. If there are uncommitted changes, read them in full and preserve them — integrate your work on top, never discard.
2. Pull all PR context with `gh`: `gh pr view`, `gh pr diff`, `gh api repos/{owner}/{repo}/pulls/{N}/reviews`, `.../comments`, `.../issues/{N}/comments`. Capture every unresolved review thread verbatim with file + line.
3. Identify changed crates/packages/modules. Note cross-cutting changes (workspace `Cargo.toml`, shared error types, public re-exports, lint config) that force broader validation.
4. Walk the checklist below. For each item, record one of: **finding** (with file:line and quoted snippet), **already addressed**, or **not applicable, because …**. No silent skips.

### Checklist

1. **Edition, MSRV, lints.** Crate uses `edition = "2024"` unless justified; `unsafe(...)` attribute wrapping applied where required. `rust-version` is declared in `Cargo.toml` and enforced in CI. Workspace lints live in the `[lints]` table, not scattered `#![deny(...)]`.
2. **Compile and ownership correctness.** Temporary-borrow lifetimes resolved. Partial moves fixed via destructuring or consistent cloning. No needless clones where a borrow suffices. No `unwrap`/`expect` on values that can legitimately fail — use `?` with a typed error, or document the invariant. No unjustified `unsafe impl Send`/`Sync`.
3. **Error handling discipline.** Libraries use `thiserror` (or hand-rolled enums) with stable variants; binaries use `anyhow` or the crate's top-level error type at the boundary. Errors carry context, not stringified inners. No swallowed errors (`let _ = …` without a comment). `From` impls don't lose information. Panics reserved for true invariants and documented.
4. **Public API surface and semver.** New `pub` items are intentional — prefer `pub(crate)`. Trait bounds, generics, re-exports don't break downstream. `String` flagged where `&str` would do, `Vec<T>` where `&[T]` would do. `#[non_exhaustive]` on growable enums/structs. `#[must_use]` on result-like and builder types. `cargo semver-checks` clean for public crates.
5. **CLI flag behavior matches command behavior.** Execution-only flags (`--dry-run`, `--explain`, `--fail-fast`, `--output`, `--watch`, …) only attach to verbs where they have effect. Introspection verbs reject no-op execution/reporting flags rather than silently ignoring. Flags that promise extra output produce it, including trailing a bare positional. Defaults and `ArgAction` match docs.
6. **Argv grammar preserves user input.** Trailing flags after a bare positional are captured as external tokens and re-parsed by the right sub-grammar, not dropped. Anything after `--` passes through untouched. Value-taking flags reject missing values and refuse `--` as a value. Short/long pairs consistent. Unknown flags surface a clear error.
7. **Configuration consistency.** Path-shaped env vars require absolute paths or document expansion explicitly. Config keys use one canonical spelling — dotted keys and TOML tables round-trip. Stale or invalid examples removed. Precedence (CLI > env > file > default) documented and tested.
8. **Cross-platform correctness.** Path tests use `PathBuf`/`Path::join`, not hardcoded separators. No assumptions about case sensitivity, line endings, or executable extensions. `tempfile` over hand-rolled `/tmp/...`. Platform-specific behavior uses explicit `#[cfg(...)]` with both branches exercised.
9. **Shared utilities vs ad-hoc duplication.** Error rendering, exit-code mapping, logging setup, config loading, and progress reporting use the crate's shared helpers instead of bare `eprintln!`/`println!`. Examples and integration binaries follow the same convention as the main entry point.
10. **Behavior intentionality and documentation.** Warnings/diagnostics fire consistently for every code path that warrants them, or the narrower scope is documented and justified. No silent best-effort where a hard error is expected. Expensive subsystems aren't configured on code paths that don't need them.
11. **Semantic correctness of operations.** An operation does what its name implies. Introspection commands project real underlying data, not synthesized stand-ins. Commands that disable a subsystem don't print verdicts from it. Cache hits/misses, dry-runs, and explain output reflect real state.
12. **Output format consistency.** Multiple representations (text, DOT, JSON, human) agree on edge direction, ordering, identifiers, and escaping. Identifiers with special characters are escaped per target format rules. Tests assert specific output, not loose `.contains()` matches.
13. **stdout/stderr discipline.** stdout carries only structured/projection output a script would consume. Diagnostics, progress, warnings, human commentary go to stderr — including in helper scripts and examples. Color/TTY detection respects `NO_COLOR` and is off when stdout is not a TTY. JSON modes emit one document per invocation, nothing else on stdout.
14. **Cancellation and signal semantics.** Ctrl+C is cooperative teardown yielding a terminal summary and meaningful exit code, not an error variant. Names cover both fail-fast and externally-triggered cancellation. Async tasks honor cancellation tokens; blocking work is interruptible or documented as not.
15. **Async and concurrency correctness.** No `block_on` inside async contexts (especially not `futures::executor::block_on` under tokio). No `MutexGuard`/`RefCell` borrow held across `.await`. `Arc<Mutex<…>>` justified vs channels or `Arc<RwLock<…>>`. CPU/blocking work uses `spawn_blocking`. Structured concurrency via `JoinSet` over loose `spawn` + `Vec<JoinHandle>`. Channel capacities are not arbitrary. CLI binaries default to `flavor = "current_thread"` unless they need the threaded runtime.
16. **Unsafe code.** Every `unsafe` block has a `// SAFETY:` comment naming the invariants relied on. Public wrappers around `unsafe` are sound for all inputs allowed by their signature, or marked `unsafe fn`. No `transmute` where a safer alternative exists. Edition 2024 `#[unsafe(no_mangle)]` / `#[unsafe(link_section)]` wrapping applied.
17. **Resource cleanup.** Files, sockets, child processes, tempdirs are closed/awaited on every path including error returns. `Drop` impls don't panic. Long-running background work shuts down gracefully on exit.
18. **Logging and tracing.** `tracing` spans/events use consistent target naming and levels. No `println!`/`eprintln!` for what should be a log. Structured fields preferred over format-string interpolation when downstream consumers parse logs.
19. **Tests.** New behavior has tests. Bug fixes have regression tests that fail without the fix. Tests assert corrected behavior (direction, escaping, exit code, exact output), not loose substrings. Snapshot tests are reviewed, not blindly accepted. Tests don't depend on wall clock, network, or working directory unless intentional.
20. **Documentation accuracy.** README, `--help`, doc comments, examples, and changelog describe implemented behavior. Removed flags actually removed from docs. Examples run. Doc tests compile. Public crates use `#![warn(missing_docs)]` and CI runs `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`.
21. **Dependency and supply-chain hygiene.** New dependencies justified. `default-features = false` where applicable. No avoidable duplicate crates from version skew. `cargo deny` / advisory concerns addressed. `cargo-machete` (or `cargo +nightly udeps`) clean. `Cargo.lock` committed. Release profile (`lto`, `codegen-units`, `strip`, `panic = "abort"` for bins) reviewed.

### Phase 1 output

Produce a `REVIEW.md` containing:

- **Summary** — one paragraph: scope of PR, scope of changes, blast radius.
- **Findings** — table of `#`, item, file:line, severity (`blocker` / `should-fix` / `nit`), one-line description.
- **Unresolved review threads** — mapped to checklist items they correspond to.
- **Open questions** — anything genuinely ambiguous (carry forward to Phase 2).

Stop. Do not edit code.

---

## Phase 2 — Plan and clarify

1. Group findings by checklist item, then by file. Order by severity: blockers first, nits last.
2. For each group, write a one- or two-line **fix plan**: what changes, where, and how it will be verified.
3. Flag ambiguities explicitly. For each, propose your default choice and the alternative. **Wait for the user to confirm or override before proceeding.** Ambiguity examples: behavior change vs strict bug fix, breaking API change vs deprecation, doc-only vs behavior-aligning fix, scope creep into untouched modules.
4. List the validation commands you will run, scoped to changed crates unless the change is cross-cutting:
```
   cargo fmt --check
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test
   cargo doc --no-deps        # if public docs changed
   cargo semver-checks        # if public API changed
   cargo machete              # if deps changed
```
   Plus any touched smoke scripts or examples.
5. Output the plan as `PLAN.md` and pause for user confirmation.

Do not edit code until the user confirms the plan.

---

## Phase 3 — Apply

After confirmation:

1. Apply fixes in plan order. One checklist item per commit where reasonable; commit message references the item number and the review thread URL(s) it resolves.
2. After each item, run the scoped validation commands. If anything fails, stop and report — don't paper over.
3. Resolve a PR review thread **only after** the matching checklist item is fixed across **all** affected files. Use `gh api ... -X PATCH` or the web UI as appropriate.
4. Do not post `@copilot review` or any variant.
5. Final step: run the full validation suite once more on the changed crates. Push.

### Phase 3 output

A summary comment on the PR (or printed to terminal) with, per checklist item:

- What was changed.
- Files touched.
- Validation result.
- Threads resolved.

State explicitly any item that was **not applicable** and why, and any item **deferred** with justification.