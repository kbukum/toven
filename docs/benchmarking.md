# Benchmarking

Performance claims require measurements against the native commands Toven orchestrates.

## Run a benchmark case

```bash
make benchmark CASE=bench/cases/rskit.sh
```

The case must define equivalent native and Toven operations. Use an installed release binary when measuring user-visible performance.

## Record the environment

Include:

- operating system and architecture
- CPU and memory
- Rust, Go, Cargo, and Toven versions
- repository commit
- module count and dependency shape
- warm or cold filesystem cache
- warm or cold Toven cache
- configured and CLI concurrency

## Compare equivalent work

Do not compare different scopes or flags. Record the exact argv for both paths.

```bash
toven explain test
```

Use `--jobs 1` when measuring scheduling overhead without parallel execution. Use the same concurrency when comparing throughput.

## Repeat measurements

Run enough iterations to expose variance. Report median and spread rather than one favorable run. Separate:

- discovery and planning
- cold execution
- warm Toven cache
- affected execution

## Output

Benchmark scripts should emit machine-readable raw measurements and a concise human summary. Store durable evidence only when it supports a documented claim.
