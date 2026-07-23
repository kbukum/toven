# Reusing rskit, not reimplementing it

Toven is built on the [rskit](https://github.com/kbukum/rskit) foundation framework, vendored as a git submodule under `rskit/` (a separate Cargo workspace). Toven depends on individual rskit core crates via path deps pinned to the submodule's prerelease version. rskit is a **foundational, multi-purpose framework any project can consume** — not a Toven-specific library. The reuse-first rule keeps that boundary clean.

## The rule

Before writing a shared concern, reuse or enhance the canonical **rskit** owner:

- **errors** → rskit `AppError`/`AppResult` with `ErrorCode`; preserve the cause.
- **config** → rskit config loading/precedence, not a hand-rolled parser.
- **validation**, **filesystem**/path safety, **git**, **process** (argv-only subprocess), **logging**, retries, HTTP — all have canonical rskit owners. Reuse them.

If you find yourself defining a type or helper that a lower-level, general-purpose framework should own, that is the signal to reach into rskit instead.

The canonical owner of each concern (rskit-reused vs toven-owned) is documented in [`docs/concern-owners.md`](docs/concern-owners.md) — consult it before writing any shared helper, and keep the two tables in sync with this command.

## Initialize the submodule first

rskit must be on disk to inspect or build against:

```bash
git submodule update --init --recursive
```

Browse `rskit/core/rskit-*/` for the owning crate and study its public API, invariants, and error model before deciding it is missing something.

## When rskit is missing or inadequate

**Improve rskit generically — never fork a Toven-specific copy and never make rskit Toven-specific.**

1. Confirm the gap: the capability genuinely isn't in rskit, or its shape is inadequate for a general consumer (not just for Toven's convenience).
2. Design the enhancement as a **general-purpose** addition to the owning rskit crate — the kind any downstream would want. rskit is in active development, so improving it is wanted; surface the change as an rskit improvement.
3. Make the change in the `rskit/` submodule against rskit's own baseline and skills, then consume it from Toven via the path dep. Do not vendor a divergent copy into a Toven crate.
4. If the enhancement is out of scope for the current task, flag it clearly (an rskit issue/note, referenced by **full URL**, never a bare `#123`) rather than working around it with a local reimplementation.

## Validate

```bash
git submodule update --init --recursive
cargo test -p <crate> --all-features -q
```

For a real audit of a reuse claim, run the `/review` command's `01-rskit-reuse` pass in a fresh agent. Per repo workflow, **create the branch and make edits only** — the maintainer commits and pushes.
