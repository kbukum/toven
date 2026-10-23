# Lens: runtime correctness

Code that compiles and passes the happy path can still leak, hang, or fail silently. Rules: [security and runtime correctness](../../../../docs/engineering.md#security-and-runtime-correctness), [public API](../../../../docs/engineering.md#public-api-and-stability), and [observability](../../../../docs/engineering.md#observability).

## Check

- **Cascade.** A model change flows through schema, normalization, planner, executor, output, tests, and docs together. A field nothing reads is dead schema.
- **Errors.** No runtime `unwrap`/`expect`, no `let _ =` that drops an error without a reason, no success-shaped fallback. `?` and `map_err` keep the cause.
- **Async.** No blocking call (process join, `blocking_send`, blocking file I/O) inline on an async task; use `spawn_blocking`. No lock guard held across `.await`. Channels are bounded.
- **Lifecycle.** Every spawned task and child process has an owner, cancellation, and bounded teardown on every exit path, including errors and signals.
- **Globals.** No mutable globals or import-time I/O; clients and policies are injected.
- **Config and platform.** Precedence (CLI, env, file, default) is tested; paths use `Path::join`; platform branches use explicit `#[cfg]`.
- **Libraries are silent.** Only `toven-cli` prints; diagnostics use rskit-logging.

## Detection starters

Exclude `#[cfg(test)]` and `tests/` when judging hits.

```bash
rg -n '\.unwrap\(\)|\.expect\(' crates/*/src
rg -n 'let _ = ' crates/*/src
rg -n 'blocking_send|\.join\(\)|std::fs::' crates/toven-{engine,runtime,exec}/src
rg -n 'static .*(Mutex|RwLock|OnceLock|LazyLock)' crates/*/src
rg -n 'println!|eprintln!|print!|eprint!' crates/toven-{model,ports,core,engine,release,version,runtime,exec,vcs,rust,go,js,command}/src
```
