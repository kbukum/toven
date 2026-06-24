# Pass 01 — rskit reuse

The single highest-value check, especially for vibe-coded work: **did the change reuse rskit, or quietly reimplement something rskit already owns?** AI-generated code defaults to writing a local helper rather than finding the canonical one, so assume duplication until proven otherwise. Treat findings here as a blocker class.

> **Run in a separate, clean-context agent** — never inline in the session that wrote the code. An independent reviewer re-derives every judgment from the code and the principles instead of trusting prior reasoning. A plan/spec may be passed in as a scope checklist only; it never excuses a baseline violation.

**Scope note.** *Changes mode:* for each new helper/type/util in the diff, ask whether it is an rskit-owned concern. *Project mode:* sweep `crates/*/src` for the patterns below and reconcile each against the rskit owner — long-lived local forks are exactly what this pass exists to surface.

## The rule

Before any shared concern is written here, the canonical rskit owner must be reused or enhanced. The owned concerns are **errors, config, validation, filesystem, git, process, and logging**. A Toven-local copy of any of these is a blocker.

If rskit's capability is missing or inadequate, the fix is to **enhance rskit generically** — never fork a Toven-specific copy, and never bend rskit to be Toven-specific. rskit is a foundational, multi-purpose framework that any project could consume; a local solution to a missing rskit capability is still a should-fix carrying an explicit "upstream to rskit" note. (See the user/repository convention: improve rskit generically when Toven exposes a gap.)

## How to check, not just glance

For each candidate, locate the rskit owner in the `rskit/` submodule and confirm the new code calls it rather than reimplementing it:

- **Errors.** Must be rskit `AppError` / `AppResult` with the cause preserved. A hand-rolled error enum, a `thiserror` type, or a `String` error for a shared concern is duplication. Check that `?` / `map_err` chains do not drop context.
- **Filesystem / git / process.** Any direct `std::fs`, `std::process::Command`, `Command::new`, `git2`, or shelling-out-to-git that rskit already wraps is duplication — route through the rskit abstraction (`rskit-fs`, `rskit-git`, `rskit-process`) so its validation / bounds / argv guarantees hold.
- **Config.** Strict loading goes through the engine's `Document` loader built on rskit config helpers — not a fresh `toml::from_str` / `serde` parse path bolted on elsewhere.
- **Validation / template / merge / hashing.** Reuse the `toven-ports` helpers and rskit primitives (e.g. `rskit-validation`, `rskit_util::hash` for content hashing) rather than inlining `blake3` or re-rolling a validator.
- **"Almost the same" counts.** A near-copy with one tweaked line is still a fork — the correct move is to enhance the rskit owner to cover the new case.

## Detection starters

These flag candidates, not verdicts — read each hit, then name the rskit owner that should have been used.

```bash
rg 'std::fs::|std::process::Command|Command::new' crates/*/src
rg 'thiserror|#\[derive\(.*Error|impl .*Error for' crates/*/src
rg 'toml::from_str|serde_json::from_str|fs::read_to_string' crates/*/src
rg 'git2|process::Command.*git|"git"' crates/*/src
rg 'blake3|sha2|Sha256|Hasher' crates/*/src
```

For each hit: is there an rskit owner for this concern? If yes and the code does not use it → **blocker** (reuse). If no rskit owner exists → the change should **add it to rskit**, not solve it locally; a local solution is a **should-fix** with an "upstream to rskit" note.

## Output for this pass

Per finding, name the concrete rskit module/type that should have been used (e.g. "use `rskit_util::hash::hash_hex` instead of calling `blake3` directly", "wrap with `rskit-process` rather than `std::process::Command`").
