---
name: fix-issue
description: "toven: Investigate a GitHub issue, fix its root cause, and validate affected behavior."
user-invocable: true
---

# Fixing a GitHub issue to root cause

An issue reports a *symptom*. The value of this skill is **diagnosis over patching**: understand what the reporter actually hit, trace it to the underlying cause, and fix that — never the surface. Toven is pre-stable, so the correct fix is the one that leaves the design clean, even when that means a redesign or refactor rather than a targeted patch.

Fix issues **only when explicitly asked**, and one issue (or one coherent cluster of duplicates) at a time.

## 1. Understand the issue

Identify the issue (the number/URL given, else ask) and read it in full — title, body, labels, and the whole comment thread, which often carries the real repro, the maintainer's intended direction, and linked duplicates or PRs:

```bash
gh issue view <n> --json number,title,body,labels,state,url,comments
gh issue view <n> --comments        # human-readable thread
```

Extract, before touching code:

- **The actual symptom** — what the reporter observed, in their words, versus what they expected.
- **A concrete reproduction** — the exact argv, config, or repository shape that triggers it. If the issue lacks one, derive the smallest case that reproduces the behavior; if you cannot reproduce it, say so and ask rather than guessing at a fix.
- **Scope signals** — the labels and any maintainer comments that point at intended direction, an owning crate, or an accepted redesign.

## 2. Investigate to the root cause

Reproduce first, then trace the symptom **down the hexagonal stack** (CLI → engine → adapters → ports → model) to where the defect actually originates — a bug usually surfaces a layer or more above its cause. Judge everything against the [engineering standard](../../../docs/engineering.md); consult [`docs/architecture.md`](../../../docs/architecture.md) for where responsibility lives and [`docs/concern-owners.md`](../../../docs/concern-owners.md) for whether the broken concern is an rskit owner (fix it in rskit generically, never fork a Toven-local copy).

- **Find the origin, not the first visible frame.** A wrong output in the CLI may be a model, normalization, or planner defect; fix it at its home layer so every caller benefits — do not compensate downstream.
- **Classify the fix** (the `create-plan` discovery→decide phase): redesign / align / enhance / drop / leave. Prefer the root-cause option.
- **Reproduce the failure as a test first.** Write a failing, deterministic test (using `toven-testkit` fixtures, not inline TOML) that encodes the reported behavior — it is both your repro and the regression guard.

## 3. Plan the fix

For anything beyond a truly localized one-file change, write a plan with the [`create-plan`](../create-plan/SKILL.md) skill (a `tmp/<change>/` folder bound to the baseline) so the fix is reviewable and cascade-complete. For a small, single-concern fix, an inline mental plan is enough — but still enumerate every layer the change must cascade through (schema, normalization, planner, executor, output, tests, docs) so no edit is left half-applied.

**Pre-stable — fix, don't patch.** Backward compatibility is not a goal yet. When the clean fix is a redesign or refactor, do it: replace the broken path outright and delete the old one. Do **not** add compatibility shims, dual code paths, feature-gated fallbacks, or success-shaped error hiding to preserve old behavior. The bar for the fix is the same as any other change: the [review skill](../review/SKILL.md) lenses apply in full.

## 4. Implement completely

Apply the fix test-first (failing test → minimal code → refactor while green), cascading through every layer the change touches so nothing is left half-applied:

- Fix at the home layer and follow the [placement](../../../docs/engineering.md#placement-and-layering), [runtime](../../../docs/engineering.md#security-and-runtime-correctness), and [API](../../../docs/engineering.md#public-api-and-stability) rules.
- Cover failure paths, not only the happy path.

## 5. Validate — scoped to what changed

Run the smallest gates that cover the touched crates (see the [`validate`](../validate/SKILL.md) skill); initialize the submodule first:

Confirm the regression test from step 2 now passes and the original reproduction no longer triggers the symptom. Then review the diff with the [`review`](../review/SKILL.md) skill directly (use an independent agent only when requested) — a green `validate` is necessary but not sufficient.

## 6. Land and reference the issue

Work on the current branch. Create a branch with the [`create-branch`](../create-branch/SKILL.md) skill only when asked. Leave the edits **uncommitted** for the maintainer unless asked to commit; when you do commit, use the [`commit`](../commit/SKILL.md) skill's compact Conventional-Commit message (`fix: …`) with no plan/issue narration in the body. Open a PR only when explicitly asked, via [`create-pr`](../create-pr/SKILL.md); let the PR (not the commit) close the issue with a `Closes #<n>` line so the reviewed change is what resolves it.

If investigation shows the issue is invalid, already fixed, or a design decision the baseline supports, **do not force a fix** — report the finding with evidence and let the maintainer decide.

## Baseline

Every fix must meet the [Definition of Done](../../../docs/engineering.md#definition-of-done). If a requested fix would push code below the baseline, redesign to the baseline instead of patching to the request.
