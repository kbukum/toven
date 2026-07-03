# Benchmarking

Compare Toven against the native commands it runs to check timing and output fidelity.

## Run a benchmark

Install the binary, then drive a case file through `make benchmark`:

```bash
cargo install --path apps/toven --locked --force
make benchmark CASE=bench/cases/rskit.sh
```

The harness (`scripts/benchmark.sh`) resolves `toven` from `PATH`. A case file (see `bench/cases/rskit.sh`) declares the approaches under comparison, the repository scenarios, and the per-iteration reset hooks.

Each run writes a timestamped directory under the case's `OUTPUT_DIR` (default `bench/out/<case>`):

- `results.csv` — per-iteration timings
- `summary.csv` — min/median/max per scenario, approach, and phase
- `metadata.env` — binary, repo, and toolchain provenance
- raw per-run logs

## Comparison matrix

The `rskit` case runs Toven task verbs against the equivalent Cargo and nextest invocations:

```bash
toven test                     # plan and run every module
toven test --base HEAD         # run only modules changed vs HEAD
cargo test --workspace         # native baseline
cargo nextest run --workspace  # native nextest baseline
```

Isolate cache state between runs so cold and warm timings stay separate:

```bash
TOVEN_CACHE_DIR=/tmp/toven-rskit-cache make benchmark CASE=bench/cases/rskit.sh
```

`toven cache clean` between runs works too. See [cache commands](commands/cache.md) for how cache location resolves.

## For reliable numbers

- Use the installed `toven` binary, not `cargo run`.
- Freeze behavior changes before collecting timings.
- Run each case repeatedly under the same shell and repository state.
- Capture raw stdout, stderr, and exit status for every run.

Record wall-clock time, cache hit/miss/disabled counts, selected module counts, and any output differences against the native command.
