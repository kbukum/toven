# Benchmarking

Benchmarking is release-readiness evidence. Use it to compare Toven orchestration
against the native commands it runs, and to identify any remaining output
fidelity gaps.

## Rules

1. Use the installed `toven` binary, not `cargo run`.
2. Freeze behavior changes before collecting timings.
3. Run each case repeatedly under the same shell and repository state.
4. Capture raw stdout/stderr and exit status for every run.
5. Separate timing, cache behavior, and output-fidelity observations.
6. Set `TOVEN_CACHE_DIR` to an absolute run-specific directory when isolating
   Toven cache state.

## rskit comparison matrix

From the rskit repository root:

```bash
toven check --no-cache
TOVEN_CACHE_DIR=/tmp/toven-rskit-cache toven check
cargo check --manifest-path core/Cargo.toml --workspace
cargo check --manifest-path contrib/Cargo.toml --workspace
```

`toven check --no-cache` measures Toven orchestration plus native command
execution without Toven cache reads or writes. Warm `toven check` measures cache
decisions and skipped work. Native Cargo checks provide the baseline for raw
Cargo timing, output shape, color behavior, and stream behavior.

## What to record

- wall-clock time per run
- Toven human timing output
- JSONL run summary when useful
- cache hit, miss, forced, and disabled counts
- selected module/package counts
- raw stdout/stderr logs
- output differences compared with native Cargo

## Output fidelity review

Before adding any PTY execution path, confirm whether the current raw-byte
streaming behavior still has a user-visible gap. Review:

- color behavior
- stdout/stderr ordering
- buffering and line timing
- interactive expectations
- JSONL stdout cleanliness

PTY support should remain opt-in unless native command fidelity cannot be
preserved well enough through normal streaming.
