# Toven review prompts

A set of standing, re-runnable review prompts for this repository. They encode Toven's permanent engineering baseline (see [`docs/engineering.md`](../../../docs/engineering.md) and [`docs/architecture.md`](../../../docs/architecture.md)) so any change set — or the whole project — can be reviewed the same way every time.

Each prompt is written to work as either a human checklist or the instruction block you hand an AI reviewer ("review this against the following"). Nothing here is specific to a single review.

## What is here

Two orchestrators that run the full review:

- [`review-changes.md`](./review-changes.md) — review a diff (a branch, commit, or `HEAD~1`). Use after every change set, especially fast/"vibe-coded" work.
- [`review-project.md`](./review-project.md) — audit the whole tree, independent of any diff. Use periodically, before a release, or when onboarding to a crate.

Six focused passes, each runnable on its own when you only need one lens:

- [`00-structure-placement.md`](./00-structure-placement.md) — layering, port placement, `mod.rs` guard, file homes.
- [`01-rskit-reuse.md`](./01-rskit-reuse.md) — did the code reuse rskit, or quietly reimplement a concern rskit already owns?
- [`02-principles.md`](./02-principles.md) — cascade-complete, argv-is-sacred, libraries-don't-print, CLI output/flag discipline, no-blocking-on-the-async-runtime, typed/no-panic, security, performance evidence.
- [`03-quality.md`](./03-quality.md) — simplicity/root-cause, dead code, outdated patterns, style gates.
- [`04-tests-tdd.md`](./04-tests-tdd.md) — TDD, fixtures, failure paths, shared doubles, determinism.
- [`05-docs-supply-chain.md`](./05-docs-supply-chain.md) — docs policy, docs-examples-match-the-live-schema, Conventional Commits, `Cargo.lock`, `cargo-deny`, no-unused-deps, SHA-pinned actions.

The orchestrators just sequence these six passes and add scope handling; the focused files hold the actual checks. Read the focused file you need and run it directly when a full review is overkill.

## Run reviews in a separate, clean-context agent

Always dispatch a review to a **fresh reviewer agent with no shared session context** — never inline in the session that produced the code. A reviewer that "remembers" writing the change rationalizes it; an independent agent re-derives every judgment from the code and the principles. Hand the agent only the scope (diff or crate/area), the relevant prompt, and this `reviews/` folder.

A plan, spec, issue, or roadmap may be passed in *as a scope checklist only* — it defines intended scope ("verify the change did what it claimed, cascade-complete, with tests") but never excuses a baseline violation. If the code diverges from the plan, report the divergence; the baseline in [`docs/engineering.md`](../../../docs/engineering.md) wins over any plan.

## How to run any prompt

1. **Pick scope.** Changes review: set a base ref and get the diff (`git diff <base>...HEAD --stat`, then per file). Project review: pick the crate(s)/area or the whole workspace.
2. **Initialize the submodule** if it is not already: `git submodule update --init --recursive`. rskit lives in the `rskit/` submodule and the rskit-reuse pass needs it on disk.
3. **Work passes in order** (00 → 05). Stop and reject as soon as a change fails pass `00` or `01` — misplaced or duplicated code makes every later pass moot.
4. **Run the validation commands** (below). Treat green `make check` as necessary but not sufficient: it does not catch layering-by-convention, cascade gaps, rskit-reuse violations, or weak tests. Those are on the reviewer.

## Severity and finding format

Record every finding as:

```
severity (blocker / should-fix / nit) — file:line — what's wrong — which principle — suggested fix
```

- **blocker** — violates a hard principle (upward dependency, rskit concern reimplemented, library prints, panic on a runtime path, `unsafe`, argv silently rewritten, behavioral change with no test). Must be fixed before merge.
- **should-fix** — real defect or debt that should be addressed but is not a baseline violation (stranded test double, inline TOML in a test, compat shim, missing regression test on a non-behavioral tidy-up).
- **nit** — minor/style, take-it-or-leave-it.

## Validation commands

For a change set, scope to the touched crate(s) — Toven's `make` targets all run `--workspace`, so drive `cargo` directly with `-p <crate>` to stay scoped:

```bash
git submodule update --init --recursive          # if not already
cargo clippy -p <crate> --all-targets -- -D warnings   # e.g. -p toven-engine
cargo test   -p <crate> -q
make fmt-check                                    # fast, whole-tree formatting check
make structure                                    # mod.rs declare-only + placement guard (cheap)
```

For a project audit or final sign-off, run the full gates:

```bash
make lint        # clippy -D warnings, --workspace
make test        # --workspace
make doc         # -D warnings
make deny        # cargo-deny: licenses, advisories, sources
make check       # full gate before sign-off
```

Treat green `make check` as necessary but **not sufficient**: it does not catch layering-by-convention, cascade gaps, rskit-reuse violations, or weak tests. See [`docs/engineering.md`](../../../docs/engineering.md) for the canonical command table.
