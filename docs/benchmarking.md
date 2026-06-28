# Benchmarking

Benchmarking is release-readiness evidence. Use it to compare Toven orchestration against the native commands it runs, and to identify any remaining output fidelity gaps.

## Rules

1. Use the installed `toven` binary, not `cargo run`.
2. Freeze behavior changes before collecting timings.
3. Run each case repeatedly under the same shell and repository state.
4. Capture raw stdout/stderr and exit status for every run.
5. Separate timing, cache behavior, and output-fidelity observations.
6. Set `TOVEN_CACHE_DIR` to an absolute run-specific directory when isolating Toven cache state.

## Harness

The benchmark harness is `scripts/benchmark.sh`, driven through `make benchmark CASE=<case-file>`. A case file is a shell fragment (see `bench/cases/rskit.sh`) that declares the approaches under comparison, the repository scenarios, and the per-iteration reset/mutation hooks. The harness resolves the `toven` binary from `PATH`, so install it first:

```bash
cargo install --path apps/toven --locked --force
make benchmark CASE=bench/cases/rskit.sh
```

Each run writes a timestamped directory under the case's `OUTPUT_DIR` (default `bench/out/<case>`) containing `results.csv` (per-iteration timings), `summary.csv` (min/median/max per scenario/approach/phase), `metadata.env` (binary, repo, and toolchain provenance), and raw per-run logs.

## rskit comparison matrix

The `rskit` case compares, from the vendored rskit workspace, the installed `toven` task verbs against the equivalent native Cargo and nextest invocations. Toven changed-module selection uses the global `--base <ref>` baseline (there is no `--affected` flag); `toven <task>` without a baseline plans every module:

```bash
toven test                       # plan + run every module
toven test --base HEAD            # run only modules with local working-tree changes vs HEAD
cargo test --workspace            # native baseline
cargo nextest run --workspace     # native nextest baseline
```

Isolate Toven cache state with `TOVEN_CACHE_DIR=/tmp/toven-rskit-cache` (or `toven cache clean` between runs) so cache reads/writes do not contaminate cold-vs-warm timings. The native Cargo/nextest runs provide the baseline for raw timing, output shape, color behavior, and stream behavior.

## What to record

- wall-clock time per run
- Toven human timing output
- JSONL run summary when useful
- cache hit, miss, forced, and disabled counts
- selected module/package counts
- raw stdout/stderr logs
- output differences compared with native Cargo

## Output fidelity review

Before adding any PTY execution path, confirm whether the current raw-byte streaming behavior still has a user-visible gap. Review:

- color behavior
- stdout/stderr ordering
- buffering and line timing
- interactive expectations
- JSONL stdout cleanliness

PTY support should remain opt-in unless native command fidelity cannot be preserved well enough through normal streaming.
