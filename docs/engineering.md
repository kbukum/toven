# Engineering standard

This page owns the rules every Toven change follows. Other documents, agent instructions, and skills link here instead of restating these rules.

Each rule says how it is checked. **Gate** means a command fails when the rule is broken. **Review** means only a reviewer checks it today. Review-only rules are listed again under [known gaps](#known-gaps).

## Read when

| You are… | Read |
|---|---|
| Starting any change | [Stance](#stance), [workflow](#workflow), [Definition of Done](#definition-of-done) |
| Adding a crate, module, port, or adapter | [Placement and layering](#placement-and-layering), [file layout](#file-layout), [reuse rskit first](#reuse-rskit-first) |
| Adding shared helpers (errors, fs, git, process, config, logging) | [Reuse rskit first](#reuse-rskit-first) |
| Handling repo files, argv, subprocesses, secrets, or concurrency | [Security and runtime correctness](#security-and-runtime-correctness) |
| Changing a public type, error, config key, flag, or output | [Public API and stability](#public-api-and-stability), [CLI and consumer experience](#cli-and-consumer-experience) |
| Writing or changing tests | [Tests and proof](#tests-and-proof) |
| Writing docs, comments, or rustdoc | [Documentation](#documentation) |
| Touching dependencies, CI, or release | [Supply chain and release](#supply-chain-and-release) |
| Running checks | [Gates](#gates) |

## Where guidance lives

| Topic | Owner |
|---|---|
| What Toven is, status, quickstart | [README](https://github.com/kbukum/toven/blob/main/README.md) |
| Product concepts | [Core concepts](product.md) |
| Where code goes: layers, crate roles, flows | [Architecture](architecture.md) |
| Which crate owns a shared concern | [Concern ownership](concern-owners.md) |
| Rules every change follows | This page |
| Contributor setup and pull requests | [CONTRIBUTING](https://github.com/kbukum/toven/blob/main/CONTRIBUTING.md) |
| Vulnerability reporting and threat model | [SECURITY](https://github.com/kbukum/toven/blob/main/SECURITY.md) |
| Roles and decisions | [GOVERNANCE](https://github.com/kbukum/toven/blob/main/GOVERNANCE.md) |
| Golden test harness | [Testing](testing.md) |
| CI and the canonical gate in practice | [Self-hosting and CI](self-hosting.md) |
| Agent startup rules | `.github/copilot-instructions.md` |
| Procedures (review, validate, release, …) | `.github/skills/` |
| The exact gate list | `Makefile` (`check`, `check-static`) and `.github/workflows/` |

## Stance

Toven is alpha and pre-stable. Backward compatibility is not a goal yet.

- Fix root causes. Prefer a clean redesign over a compatibility shim, a dual code path, or a "keep the old path too" fallback. (Review)
- For each real defect, pick **Redesign**, **Align**, **Enhance**, or **Drop**, with a one-line reason. Leave sound code alone. (Review)
- Use the simplest design that fully solves the problem. Complexity must earn its place. (Review)

## Workflow

1. **Discover** current behavior and owners.
2. **Decide**: redesign, align, enhance, drop, or leave.
3. **Implement** the whole change. A model change flows through schema, normalization, planner, executor, output, tests, and docs in the same change. No half-applied edits. (Review)
4. **Validate** the smallest affected scope first, then the full gate when the change is cross-cutting or release-bound. See [gates](#gates).

## Toolchain

`rust-toolchain.toml` pins the toolchain. `[workspace.package]` in `Cargo.toml` declares `edition` and `rust-version` once; every crate inherits them. Do not restate a different floor elsewhere. CI installs the same version from its own matrix (`.github/workflows/ci.yml`); nothing checks that the two values match (see [known gaps](#known-gaps)).

## Gates

Start here:

```bash
git submodule update --init --recursive
make doctor   # fail closed if a tool the task graph needs is missing
make check    # the canonical full gate
```

`make check` runs `check-static` plus `test`. Read those two recipes in the `Makefile` for the exact list; this page does not copy it. Most gates run through Toven itself. [Self-hosting and CI](self-hosting.md#canonical-gate) explains how.

Other targets cover one concern: list them with `grep -E '^[a-z-]+:' Makefile`. During iteration, scope to the changed crate with `cargo clippy -p <crate>` and `cargo test -p <crate>`; the [validate skill](https://github.com/kbukum/toven/blob/main/.github/skills/validate/SKILL.md) shows the exact commands.

CI runs these gates on every pull request: `ci.yml` (`make doctor`, `make check-static`, `make doctest`, sharded nextest), `supply-chain.yml` (`make deny`, `make coverage`, CodeQL), `pr-title.yml` (Conventional Commit title), and `release-readiness.yml` (release dry run and previews). `self-canary.yml` runs the gates through a freshly packaged binary on pushes to `main`.

A green gate is necessary, not sufficient. Rules marked **Review** below are not checked by any gate.

## Placement and layering

[Architecture](architecture.md#workspace-layers) owns the layer map and each crate's role. The rules:

- Dependencies point downward only. A lower layer never imports a higher one. (Review; Cargo rejects only the upward edges that form a cycle.)
- Ecosystem adapters (`toven-rust`, `toven-go`, `toven-js`, `toven-command`) depend on `toven-ports` and `toven-model` only, never on an engine crate or the CLI. (Review)
- `toven-cli` never depends on a concrete ecosystem adapter. Only `apps/*` wire adapters in. `apps/*` hold no capability. (Review)
- A port trait lives in `toven-ports` and references only `toven-model`, rskit, std, or port-owned values. Its production adapter lives in the consuming crate, never beside the trait. (Review)
- Each port has exactly one shared double in `toven-testkit` (`doubles/<port>.rs`). (Review)
- `lib.rs` and `mod.rs` declare and re-export only: no functions, types, impls, or logic. (Gate: `make structure`, rule `declare-only-aggregator`)
- Raw subprocess spawns appear only at the allow-listed execution homes in `toven-exec`. Everything else goes through an injected runner. (Gate: `make structure`, rule `single-exec-home`)

### Language- and tool-agnostic core

The core crates orchestrate a task graph but know nothing about cargo, go, mdbook, or ast-grep. A language- or tool-specific gate lives in an adapter or in configuration, never as a hard-coded verb in core. Place a gate by what it is bound to:

| Gate | Nature | Home |
|---|---|---|
| `doctest` | Rust-specific | A `toven-rust` default task reusing `TaskKind::Test` |
| `deny` (cargo-deny) | Rust-specific, opt-in (needs `deny.toml`) | A repo-declared `[ecosystems.rust.tasks.deny]` task |
| `structure` (ast-grep), `docs-build` (mdbook) | Tool-specific, language-agnostic | `[ecosystems.command.tasks.*]`, the generic-tool adapter |
| `doctor` tool audit | Agnostic mechanism | Toven core; tool identity still comes from adapter probes and task argv |

When a new tool gate is needed, add an adapter task or a `[ecosystems.command.tasks.*]` entry. Never teach core about the tool. (Review)

## File layout

Organize code into focused, concern-named files and folders. Splitting is a judgment call, not a size rule. (Review)

- Look at splitting a file when it passes roughly 300–400 lines of real code (excluding tests, comments, and blank lines) **and** mixes several concerns. Length alone is never the reason.
- Look at splitting a module when it holds more than about ten non-test files that fall into two or more loosely coupled groups. Lift each group into its own folder with a declare-only `mod.rs`.
- A cohesive single-concern file is fine at any size.

## Reuse rskit first

Before adding errors, configuration, validation, filesystem, Git, process, logging, or other shared infrastructure, check [concern ownership](concern-owners.md) and reuse the rskit owner. (Review)

- If rskit is missing the capability, improve rskit generically in the `rskit/` submodule. Never fork a Toven-specific copy, and never make rskit depend on Toven concepts. (Review)
- A near-copy with one changed line is still a fork. (Review)
- Do not change the `rskit/` submodule revision as a side effect of other work. (Review)

## Security and runtime correctness

[SECURITY](https://github.com/kbukum/toven/blob/main/SECURITY.md) owns the threat model. The rules:

- Treat `toven.toml`, repository files, discovered manifests, federation frames, and passthrough argv as untrusted. Validate at every trust boundary. (Review)
- Never silently rewrite user argv. Toven validates and expands selectors but never infers hidden flags. (Review)
- Generated commands are argument vectors. Shell execution is an explicit opt-in, never a default. (Review; raw spawn sites are gated by `single-exec-home`)
- Keep paths from config or discovery inside the workspace root; reject `..` escapes and stray absolute paths before they reach the filesystem or a subprocess. (Review)
- Never log secrets or put tokens in argv or URLs. Secrets reach children through the environment. (Review)
- Bound input and output with hard caps. (Review)
- Every subprocess and RPC call honors cancellation. Any stop signal (`SIGINT`, `SIGTERM`, `SIGHUP`) tears down in-flight children and reaps their process groups. `--timeout` sets an opt-in per-unit ceiling; task processes have no default wall-clock limit because real builds run long. Short tool calls (probes, `gh`, VCS queries) keep fixed internal timeouts. (Review)
- Never run blocking work (process join, `blocking_send` into a bounded channel, blocking file I/O) inline on an async task; use `spawn_blocking` or an async API. (Review)
- Inject clients and policies. No mutable globals or import-time I/O. (Review)
- Release mutation requires explicit approval. (Gate: `toven release publish` needs `--yes`, and `release.yml` runs it behind the `release` environment's required reviewers)
- No `unsafe` code. (Gate: `unsafe_code = "forbid"`)

## Public API and stability

- Public APIs are typed and minimal. No `Any`-style escape hatches or stringly-typed returns. Prefer `pub(crate)`. (Review; `unreachable_pub` is linted by `make lint`)
- Errors use rskit `AppError` / `AppResult` and preserve the cause. (Review)
- No `unwrap()`, `expect()`, swallowed errors, or success-shaped fallbacks on runtime paths. Tests may unwrap. (Review)
- Methods that return `Self` carry `#[must_use]`. (Gate: clippy `return_self_not_must_use` via `make lint`)
- Public enums that may grow are `#[non_exhaustive]`. (Review)
- Every public item has `///` docs. (Gate: `missing_docs` warns and `make lint` denies warnings)
- No production API exists only for tests. A test-only accessor such as `into_inner` is `#[cfg(test)]` or removed; shared doubles expose recording accessors instead. (Review)
- When several arguments form one group, prefer a parameter struct or builder over a long positional list. This is guidance, not a hard limit. (Review)

**Stability policy.** Toven follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html). While `0.x`, a breaking change bumps MINOR; otherwise PATCH. Breaking changes are allowed when they give a cleaner design; record them in `CHANGELOG.md`. The stable contracts for tools are the JSONL event stream, the documented [exit codes](commands/README.md#exit-codes), and the strict `toven.toml` schema. Human output may change. No crate is published to crates.io, so Rust APIs carry no external stability promise.

## CLI and consumer experience

- Only `toven-cli` prints. Library crates return typed data and errors. (Review)
- stdout carries tables, projections, and JSONL. stderr carries progress, child output, warnings, summaries, and errors. See [output streams](commands/README.md#output-streams). (Review)
- Scope flags at the parser. A verb's `--help` shows only flags that change its behavior; never accept a flag that is a silent no-op. (Review)
- Errors say what failed, why, and the next step, in user vocabulary. Offer a did-you-mean when a close match exists. Architecture words (PLAN, APPLY, event sink) stay out of `--help` and messages. (Review)
- Exit codes are documented in the [command reference](commands/README.md#exit-codes); keep them stable. (Review)
- The new-user path (`toven init` → first run) works and never writes config that later commands reject. (Review)

## Observability

- User-facing progress and summaries go through the reporter sinks, never ad hoc prints. (Review)
- Internal diagnostics go through rskit-logging. Libraries stay silent by default. (Review)
- Every run carries a `run_id` for correlation. It is never a cache key or a path. (Review)

## Tests and proof

[Testing](testing.md) explains the golden harness and fixtures.

- Write a failing behavioral test first, then the code. Cover failure paths, not only the happy path. (Review)
- Every bug fix has a regression test that fails without the fix. (Review)
- Tests are deterministic and use no real network. (Review)
- Use `toven-testkit` fixtures and declarative case files, not large inline TOML strings. Use the shared port doubles, not one-off fakes. (Review)
- Assert observed plans, output, and errors, not internal call order (unless order is the contract). (Review)
- Coverage meets the thresholds in `[ecosystems.rust.coverage]` in `toven.toml`. (Gate: `make coverage`)
- Doctests run as the Rust `doctest` task: `toven run doctest --workspace rust -- --all-features`, or `cargo test -p <crate> --doc`. (Gate: `make test`)
- A performance claim needs benchmark evidence: `make benchmark CASE=bench/cases/<case>.sh`. See [benchmarking](benchmarking.md). (Review)

## Code style

- `rustfmt` with the settings in `rustfmt.toml`. (Gate: `make fmt-check`)
- Clippy with `all`, `pedantic`, and `nursery` from `[workspace.lints]`, warnings denied. (Gate: `make lint`)
- Rustdoc builds with warnings denied. (Gate: `make doc`)
- Comments explain why, not what. No plan numbers, step references, "for now" narration, or commented-out code. (Review)

## Documentation

- Stable docs live in `docs/`. `tmp/` holds active plans only and is never linked from committed files. (Review)
- One source line per Markdown paragraph. Do not hard-wrap prose. (Review)
- Organize user docs by task. Lead with the shortest working example. (Review)
- Include exact syntax, options, stdout/stderr behavior, and failure cases. Examples match the current strict config and CLI. (Review)
- Keep `docs/SUMMARY.md` aligned with the files. (Gate: `make docs-build`, run in `self-canary.yml`)
- Use relative links between doc pages. (Review)
- Do not cite numeric section references (for example "principles §4") in code or rustdoc; they drift. (Review)

## Supply chain and release

- `Cargo.lock` is committed and in sync. (Gate: `--locked` on Makefile and CI Cargo runs)
- Advisories, licenses, bans, and sources pass `deny.toml`. (Gate: `make deny`)
- `rskit-*` version pins in the root `Cargo.toml` match the submodule crates. (Gate: path dependency resolution fails on a mismatch)
- GitHub Actions are pinned by commit SHA. (Review; Dependabot keeps pins current)
- New dependencies are justified; test-only ones go in `[dev-dependencies]`. (Review)
- Pull request titles follow Conventional Commits (`feat`, `fix`, `docs`, `refactor`, `test`, `chore`). (Gate: `pr-title.yml`)
- Releases ship signed checksums, an SBOM, and build provenance. (Gate: `release.yml`) The [release skill](https://github.com/kbukum/toven/blob/main/.github/skills/release/SKILL.md) owns the procedure.

## Known gaps

These rules are written down but no gate checks them yet. That is intentional for now and open to revisit; a reviewer covers them meanwhile.

| Rule | Why no gate yet |
|---|---|
| Layer direction beyond cycles | No dependency-direction check; candidate: a small `cargo metadata` script |
| No `unwrap`/`expect`, no printing in libraries | The clippy `unwrap_used`, `expect_used`, `print_stdout`, `print_stderr` restriction lints are not enabled |
| `#[non_exhaustive]` on growing enums | No lint exists for intent |
| One shared double per port, no inline TOML in tests | No structural rule yet |
| SHA-pinned actions | No workflow linter (for example `zizmor` or `actionlint`) in CI |
| Toolchain pin matches CI matrix | No check compares `rust-toolchain.toml` with workflow matrices |
| Markdown links, anchors, and hard-wrapping | No Markdown link or style checker in CI |
| Unused dependencies | `cargo machete` is not in CI |
| Public API change detection | `cargo semver-checks` is not used; nothing is published to crates.io |
| `# Errors` / `# Panics` rustdoc sections | `missing_errors_doc` and `missing_panics_doc` are allowed in `[workspace.lints]` |
| Decision records | No ADR folder; rationale lives in architecture prose and the changelog |

## Definition of Done

A change is done when:

- [ ] It sits in the right crate and layer and reuses the rskit owner for shared concerns.
- [ ] The cascade is complete: schema, normalization, planner, executor, output, tests, and docs changed together.
- [ ] Behavior changes have tests written first, including failure paths; fixes have regression tests.
- [ ] Trust boundaries are validated, argv is untouched, I/O is bounded, and cancellation is honored.
- [ ] Public API, errors, flags, and output follow the rules above; breaking changes are in `CHANGELOG.md`.
- [ ] User docs, rustdoc, and examples match the new behavior.
- [ ] The scoped checks pass, and `make check` passes for cross-cutting or release-bound changes.
- [ ] Review-only rules were checked by a reviewer, not assumed.
