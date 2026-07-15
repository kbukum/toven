---
name: create-plan
description: >-
    Turn a non-trivial change into a written, reviewable plan under the repo's gitignored tmp/
    folder — a README overview plus, when the work is multi-step, numbered step markdown files
    that can be applied iteratively. Every plan is bound to Toven's engineering baseline. Use
    when scoping a feature, refactor, or release, or when asked to plan or break down work.
user-invocable: true
---

# Planning Toven work as applyable step files

A plan is a written contract for a change set: what to do, in what order, and how you will know
each part is done. In this repo a plan is **not prose to admire** — it is a folder of markdown
that the `apply-plan` / `apply-step` skills execute iteratively.

## Where plans live: `tmp/<plan-name>/`

Always create plans under `tmp/` at the repo root. `tmp/` is **gitignored** (`/tmp/*`,
`!/tmp/.keep`) — plans and handoff notes are local working scratch, never committed and never
referenced from committed docs. Name the folder by the change itself in kebab-case
(`tmp/engine-plan-caching/`, `tmp/rust-adapter-toolchain-prober/`) — the same high-level naming
rule as branches: no `batch-N`, plan numbers, or internal/session detail in the folder name.

```bash
mkdir -p tmp/<plan-name>
```

## Structure

Match this shape (the same layout every plan folder under `tmp/` uses):

- **`README.md`** (always) — the overview: goal, how to read the folder, an ordered index of the
  step files with their dependency order, and the cross-cutting rules that apply to every step.
- **`NN-topic.md`** step files (when the work is multi-step) — zero-padded and ordered by
  dependency layer (`01-model.md`, `02-...`), each a self-contained unit of work. A genuinely
  small single-shot change can be one `README.md` with an inline step list — split into step files
  as soon as the work is iterative or spans layers.

Numbering orders the plan; it is **internal to the plan folder only**. When a step becomes a
branch/PR, name that branch/PR by the change (see the `create-branch` skill) — never `step-3` or
`batch-N`.

### Each step file contains

```markdown
# <Step title — the change, not "step N">

**Layer:** L<n> · **Depends on:** <steps> · **Blocks:** <steps> · **Status:** pending

## Scope
What this step changes and, explicitly, what it does not.

## Steps
1. Numbered, concrete actions at real file paths.
2. ...

## Files touched
- `crates/toven-<x>/**`, ...

## Acceptance criteria
- [ ] Behavior written test-first; deterministic tests green on affected crates.
- [ ] <step-specific, verifiable outcomes>
```

`Status: pending` and the `- [ ]` boxes are the progress signal `apply-plan` reads to find the
first unfinished step. `apply-step` flips them to `done`/`- [x]` when a step lands.

## Bind every plan to the baseline

A plan may **not** invent a lighter standard than Toven's. Its cross-cutting rules restate — and
link to — the engineering baseline in [`docs/engineering.md`](../../../docs/engineering.md) /
[`.github/copilot-instructions.md`](../../copilot-instructions.md) and defer detailed judgment to
the `review` skill's seven passes. In every plan's README, make these load-bearing:

- **Test-first (TDD).** Failing test → minimal code → refactor while green, failure paths
  included. Use `toven-testkit` fixtures over inline TOML; never batch code and bolt tests on.
- **Reuse rskit first.** Reuse or enhance the canonical rskit owner before writing a shared
  concern; if rskit is inadequate, improve it generically — never fork a Toven-specific copy.
- **Cascade-complete.** A model change flows through schema, normalization, planner, executor,
  output, tests, and docs in the same change.
- **Structure & placement.** Downward-only layering (L0→L1→L2→L3); a port trait in `toven-ports`,
  its adapter in the consuming crate, one shared double per port in `toven-testkit`; `mod.rs`
  declare-only.
- **argv is sacred; libraries don't print.** User argv is never silently rewritten; only the
  CLI/reporting layer produces user-facing output.
- **Typed & no panic.** No broad `Any` on public surfaces; no `unwrap`/`expect`/swallowed errors
  on runtime paths; rskit `AppError`/`AppResult` preserving cause.
- **Root-cause, no shims.** Pre-stable: redesign cleanly and remove the old path.
- **Readable files.** Split by concern into focused files — never pile into one file.

Order steps so each starts only when its dependencies are green, and so each maps to a
**standalone, reviewable change**.

## Handoff

Creating the plan is a docs-only act under `tmp/` — no source edits, no branch, no commit. Apply
it later with the `apply-plan` skill (whole plan) or `apply-step` (one step).
