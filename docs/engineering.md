# Engineering guide

This guide defines the contribution baseline for Toven.

## Development phases

1. Discover the current behavior and ownership boundaries.
2. Decide whether to redesign, align, enhance, drop, or leave it.
3. Implement the complete change across affected layers.
4. Validate the smallest affected scope, then broader gates when required.

Toven is pre-stable. Prefer root-cause redesigns over compatibility shims.

## Setup

```bash
git submodule update --init --recursive
make check
```

The repository uses the Rust toolchain pinned by `rust-toolchain.toml`. The language floor is **edition 2024** at a **minimum Rust version of 1.94**, declared once in `[workspace.package]` (`edition`, `rust-version`) and inherited by every crate. Treat those two fields as the single source of truth; do not restate a different floor elsewhere.

## Canonical commands

| Command | Purpose |
|---|---|
| `make fmt` | Format the workspace |
| `make fmt-check` | Check formatting |
| `make lint` | Run Clippy with warnings denied |
| `make test` | Run nextest and doctests |
| `make structure` | Enforce declare-only aggregators |
| `make doc` | Build local crate documentation with warnings denied |
| `make deny` | Check advisories, licenses, bans, and sources |
| `make check` | Run the canonical full gate |
| `make coverage` | Run configured coverage gates |
| `make benchmark` | Run an evidence benchmark case |
| `make docs-build` | Build this mdBook |
| `make docs-serve` | Serve this mdBook locally |

## Layering

Dependencies point from applications toward the model:

```text
model -> ports -> engine/adapters -> CLI -> apps
```

Lower layers never import higher layers. Ecosystem adapters do not import the engine or CLI.

## Language- and tool-agnostic core

Toven's core (`toven-model`, `toven-ports`, `toven-engine-core`, `toven-engine-release`, `toven-engine`, `toven-cli`) is language- and tool-agnostic: it orchestrates a task graph but knows nothing about cargo, go, mdbook, or ast-grep. Every language- or tool-specific gate lives in an adapter or in configuration, never as a hard-coded verb in core. The test for where a gate belongs is what it is bound to, not what is convenient:

| Gate | Nature | Home |
|---|---|---|
| `doctest` | Rust/cargo-specific (Go has no analog) | A `toven-rust` adapter default task reusing `TaskKind::Test` |
| `deny` (cargo-deny) | cargo-specific but repo-opt-in (needs `deny.toml`) | A repo-declared `[ecosystems.rust.tasks.deny]` task |
| `structure` (ast-grep), `docs-build` (mdbook) | tool-specific, language-agnostic | `[ecosystems.command.tasks.*]` — the command ecosystem is Toven's generic-tool adapter |
| `doctor` tool audit | "does the resolved graph have its tools?" — agnostic mechanism | Toven core; tool *identity* still comes from adapter probes and task argv |

`doctest` introduces no new core concept — the Rust adapter simply declares one more default task the way it declares `build`/`test`/`lint`, and the Go adapter never gains it, which is the proof it sits in the right layer. When a new tool gate is needed, add an adapter task or a `[ecosystems.command.tasks.*]` declaration; never teach core about the tool.

## Reuse rskit first

Before adding shared errors, configuration, validation, filesystem, Git, process, or logging behavior, check [concern ownership](concern-owners.md). Improve rskit generically when its implementation is incomplete; do not fork a Toven-specific copy.

## API and runtime rules

- Use typed, minimal public APIs.
- Preserve error causes with rskit `AppError` and `AppResult`.
- Do not use `unwrap()` or `expect()` on runtime paths.
- Do not swallow errors or return success-shaped fallbacks.
- Keep user argv unchanged.
- Keep libraries silent; only the CLI/reporting layer prints.
- Keep `lib.rs` and `mod.rs` declare-only.
- Document every public item.

## Test-first changes

Write a failing behavioral test before implementation. Cover success and failure paths. Use `toven-testkit` fixtures and shared doubles instead of large inline configuration strings.

Run Rust tests through nextest:

```bash
cargo nextest run -p <crate>
```

Run doctests separately when affected:

```bash
toven run doctest -p <crate>   # the gate's Toven-driven form (cargo test --doc)
cargo test -p <crate> --doc    # the equivalent low-level cargo escape hatch
```

## Security

- Treat repository files and task commands as untrusted input.
- Validate at every trust boundary.
- Execute subprocesses as argv unless shell mode is explicitly selected.
- Never log credentials or place tokens in argv.
- Bound input and output with hard caps.
- Bound process lifetime through cooperative cancellation: Ctrl+C tears down every in-flight child, and `--timeout` sets an opt-in per-unit ceiling. Task processes carry no default wall-clock limit, because legitimate build/test tasks run arbitrarily long. Short-lived tool calls (metadata probes, `gh`, VCS queries) keep their own fixed internal timeouts.
- Require explicit approval before release mutation.

## Observability

- Emit user-facing progress, status, and summaries through the reporter sinks (human on stderr, JSONL on stdout) — never ad hoc prints from library code.
- Route internal diagnostics through rskit-logging, not `println!`/`eprintln!`; keep libraries silent by default so a caller controls verbosity.
- Keep the machine-readable Event stream (`--output jsonl`) the stable contract for tooling; treat human framing as diagnostics that may change.
- Every run carries a `run_id` for correlation; it is observability-only and never a cache key or path.

## Supply chain

- Pin GitHub Actions by commit SHA.
- Keep `Cargo.lock` committed.
- Enforce advisory, source, and license policy with `cargo-deny`.
- Generate release SBOM and provenance.
- Sign distributed artifacts.
- Use Conventional Commits. The PR title is the squash-merge subject and is CI-enforced (`.github/workflows/pr-title.yml`), so a non-Conventional subject cannot land on `main`.

## Documentation

- Write one natural source line per Markdown paragraph.
- Organize user docs by task and outcome.
- Include exact syntax, options, stdout/stderr behavior, and failure conditions.
- Keep committed docs free of migration-plan narration and `tmp/` references.
- Keep `SUMMARY.md` paths aligned with the real mdBook tree.
- Use relative links between documentation pages.

## Performance

Do not claim speedups without benchmark evidence:

```bash
make benchmark CASE=bench/cases/<case>.sh
```

Record the environment, native command, Toven command, cache state, and repeated measurements.
