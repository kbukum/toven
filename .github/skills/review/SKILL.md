---
name: review
description: >-
    Run Toven's standing engineering-baseline review over a change set (a branch, commit range,
    or HEAD~1) or over a whole crate/area/tree. Sequences seven focused passes — structure &
    placement, rskit reuse, principles, quality, tests/TDD, docs & supply chain, comments &
    rustdoc. Use before merging a change, when auditing a crate, or before a release. Always run
    it in a fresh, clean-context reviewer.
user-invocable: true
---

# Reviewing Toven against its engineering baseline

Toven is an argv-first task planner built on the vendored rskit foundation (a git submodule). A
defect in a lower crate propagates up the hexagonal stack to the CLI and every generated command
batch. This skill encodes Toven's permanent review baseline as seven focused passes plus
orchestrators.

The authoritative baseline lives in [`docs/engineering.md`](../../../docs/engineering.md) (and
[`docs/architecture.md`](../../../docs/architecture.md)); see also
[`.github/copilot-instructions.md`](../../copilot-instructions.md). A plan, spec, issue, or
roadmap may be passed in **as a scope checklist only** — it defines intended scope, never excuses
a baseline violation. If the code diverges from the plan, report the divergence; the baseline
wins.

## Run in a separate, clean-context agent

**Always dispatch a review to a fresh reviewer with no shared session context** — never inline in
the session that wrote the code. A reviewer that "remembers" writing the change rationalizes it;
an independent agent re-derives every judgment from the code and the principles. Hand it only the
scope (diff or crate/area) and this skill.

## Initialize the submodule first

rskit lives in the `rskit/` submodule and the rskit-reuse pass needs it on disk:

```bash
git submodule update --init --recursive
```

## Pick a driver

- **Change set** → [`references/review-changes.md`](references/review-changes.md). A diff (branch,
  commit range, or `HEAD~1`). Use after every change set, especially fast/"vibe-coded" work.
- **Whole tree / crate** → [`references/review-project.md`](references/review-project.md). A
  standing audit independent of any diff. Use periodically, before a release, or when onboarding.
- **Review → fix in one pass** → [`references/review-details.md`](references/review-details.md).
  Fans the review into parallel subagent passes, then plans and applies fixes.

## The seven focused passes (run in order)

Stop and reject as soon as a change fails pass `00` or `01` — misplaced or duplicated code makes
every later pass moot. Each file can be run standalone when you need only one lens.

1. [`references/00-structure-placement.md`](references/00-structure-placement.md) — layering, port
   placement, `mod.rs` guard, file homes.
2. [`references/01-rskit-reuse.md`](references/01-rskit-reuse.md) — did the code reuse rskit, or
   quietly reimplement a concern rskit already owns? *(blocker class)*
3. [`references/02-principles.md`](references/02-principles.md) — cascade-complete, argv-is-sacred,
   libraries-don't-print, CLI output/flag discipline, typed/no-panic, security, performance
   evidence.
4. [`references/03-quality.md`](references/03-quality.md) — simplicity/root-cause, dead code,
   outdated patterns, style gates.
5. [`references/04-tests-tdd.md`](references/04-tests-tdd.md) — TDD, fixtures, failure paths,
   shared doubles, determinism.
6. [`references/05-docs-supply-chain.md`](references/05-docs-supply-chain.md) — docs policy,
   docs-match-the-live-schema, Conventional Commits, `Cargo.lock`, `cargo-deny`, SHA-pinned
   actions.
7. [`references/06-comments-rustdoc.md`](references/06-comments-rustdoc.md) — comments and `///`
   docs describe the code as it is, not plans/history/process.

## Severity and finding format

```
severity (blocker / should-fix / nit) — file:line — what's wrong — which principle — suggested fix
```

- **blocker** — hard-principle violation (upward dependency, rskit concern reimplemented, library
  prints, panic on a runtime path, `unsafe`, argv silently rewritten, behavioral change with no
  test). Fix before merge.
- **should-fix** — real defect or debt that isn't a baseline violation (stranded test double,
  inline TOML in a test, compat shim, missing regression test on a non-behavioral tidy-up).
- **nit** — minor/style, take-it-or-leave-it.

## Validation commands

**Scope to the touched crate(s)** — Toven's cargo-based `make` gates (`lint`, `test`, `doc`,
`coverage`) run `--workspace --all-features`, so drive `cargo` directly with `-p <crate>` to stay
scoped:

```bash
git submodule update --init --recursive                # if not already
cargo clippy -p <crate> --all-targets --all-features -- -D warnings   # e.g. -p toven-engine
cargo test   -p <crate> --all-features -q
make fmt-check                                          # fast, whole-tree formatting check
make structure                                          # mod.rs declare-only + placement guard
```

For a project audit or final sign-off run the full gates: `make lint`, `make test`, `make doc`,
`make deny`, `make check`. Treat green `make check` as **necessary but not sufficient**: it does
not catch layering-by-convention, cascade gaps, rskit-reuse violations, or weak tests. Those are
on the reviewer.
