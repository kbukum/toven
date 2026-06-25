# Pass 02 — Principle conformance

Each item here is a hard principle from [`docs/engineering.md`](../../../docs/engineering.md), not a preference. This is where vibe coding usually drifts.

> **Run in a separate, clean-context agent** — never inline in the session that wrote the code. An independent reviewer re-derives every judgment from the code and the principles instead of trusting prior reasoning. A plan/spec may be passed in as a scope checklist only; it never excuses a baseline violation.

**Scope note.** *Changes mode:* walk the cascade list you built for the diff and grep the touched crates. *Project mode:* the print/panic/argv/security invariants below hold for the entire library surface — sweep all of `crates/toven-{model,ports,engine,rust,go,command}/src`, not just a diff.

## rskit-first

Covered in depth in pass `01`. Confirm it was done before continuing.

## Cascade-complete

A model change must flow through **schema → normalization → planner → executor → output → tests → docs in the same change**. Walk the list of layers that *should* have changed with the touched model/schema type; any half-applied edit is a blocker. Common miss: a new field added to the model and serde schema but never normalized, never reaching the planner, or with no test. *Project mode:* spot-check that existing model fields are actually consumed end-to-end — an orphaned field that nothing reads is dead schema (hand to pass `03`).

## argv is sacred

User-owned argv is never silently rewritten. Validation and selector expansion are allowed; inferring hidden flags is not. Generated commands must be argument vectors by default; any shell execution must be an explicit opt-in, not a default. Look for string-concatenated commands or implicit shell invocation.

## Libraries do not print

Only `toven-cli` produces user-facing output. Any `println!` / `print!` / `eprintln!` / `eprint!` (or logging-used-as-output) in `toven-model`, `toven-ports`, `toven-engine`, `toven-rust`, `toven-go`, or `toven-command` is a blocker — libraries return typed data and typed errors; the CLI/reporting layer renders.

## Typed, minimal, no panics

- No `unwrap()` / `expect()` or swallowed errors on runtime paths (tests excepted).
- No success-shaped fallbacks that mask failure.
- No `Any`-style escape hatches (`Box<dyn Any>`, stringly-typed returns) on public surfaces.
- Errors use rskit `AppError` / `AppResult` and preserve the cause.

## No blocking on the async runtime

On an async runtime path, never run a synchronous, potentially long-blocking call — process shutdown/join, a `blocking_send` into a bounded channel, blocking filesystem IO — inline on a task that must keep making progress. It can park the runtime and deadlock a bounded live-output / IPC channel whose reader thread is waiting on that same task (this is exactly the APPLY persistent-teardown class of hang). Offload blocking work via `tokio::task::spawn_blocking` (or an async API) and `await` it so draining continues while the blocking work waits. Every subprocess/RPC call carries a timeout and honors the cancellation token.

## CLI output and flag discipline

The complement to *libraries don't print* (this section is the one place `toven-cli` is in scope): the CLI is the only layer that prints, but it must keep the channels clean. `stdout` is reserved for the machine-readable stream — the JSONL event projection — so human progress, status, and diagnostics go to `stderr`; a consumer of `--output jsonl` must never have to filter human chrome out of stdout. A global flag must be rejected (or scoped) on any verb that does not consume it: never advertise a flag that is silently a no-op for the dispatched verb (e.g. `--fail-fast` on a verb that never schedules multiple units, or `-v`/`-q` on a verb with no reporter). Each accepted flag must actually change that verb's behavior.

## Security

User commands and repository files are untrusted at the CLI boundary. Validate at every trust boundary, use argv-only subprocess execution, never log secrets, bound input and output. Flag any unbounded read of repo files or any unvalidated path/selector flowing into execution. (For a deeper, dedicated sweep, pair this with a security-focused review; this pass covers the baseline.)

## Performance

Any performance claim in the diff or commit message needs `make benchmark` evidence. No evidence → drop the claim.

## Detection starters

Exclude `#[cfg(test)]` blocks and `tests/` when judging runtime-path hits.

```bash
rg '\.unwrap\(\)|\.expect\(' crates/*/src
rg 'println!|print!|eprintln!|eprint!' crates/toven-{model,ports,engine,rust,go,command}/src
rg 'dyn Any|Box<dyn Any>' crates/*/src
rg 'format!\(.*\)\s*(\+|\.push_str)|sh -c|"bash"|"sh"' crates/*/src   # string-built / shelled commands
rg 'blocking_send|\.shutdown\(\)|\.join\(\)|std::fs::' crates/toven-engine/src   # blocking call on an async path? must be spawn_blocking
rg 'println!|print!|io::stdout|Stdout' crates/toven-cli/src   # human/progress/diagnostics must be stderr; stdout only for the jsonl sink
```
