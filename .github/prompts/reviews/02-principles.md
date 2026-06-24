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
```
