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

The repository uses the Rust toolchain pinned by `rust-toolchain.toml`.

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
cargo test -p <crate> --doc
```

## Security

- Treat repository files and task commands as untrusted input.
- Validate at every trust boundary.
- Execute subprocesses as argv unless shell mode is explicitly selected.
- Never log credentials or place tokens in argv.
- Bound input, output, and process lifetime.
- Require explicit approval before release mutation.

## Supply chain

- Pin GitHub Actions by commit SHA.
- Keep `Cargo.lock` committed.
- Enforce advisory, source, and license policy with `cargo-deny`.
- Generate release SBOM and provenance.
- Sign distributed artifacts.

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
