---
name: create-plan
description: "toven: Write or revise a high-level implementation plan with dependencies and acceptance criteria."
user-invocable: true
---

# Plan a change

Create `tmp/<plan>/`, or use the existing folder the user supplied. Planning changes documents only, not implementation or Git state.

## Keep it small

- `README.md`: goal, scope, and the ordered steps.
- `01-topic.md`, `02-topic.md`, etc.: outcome, dependencies, implementation outline, and acceptance checks.

Use `Status: pending` and `Depends on:` in each step. Keep steps reviewable: one step per PR, unless the user combines related steps. A small single-step plan can stay in its README.

No required `plans/` layer, parent indexes, handoff, or extra document set. Add supporting material only when it helps the work.

## Make it useful

Inspect the relevant source first. Describe the intended behavior, owning modules, important decisions, and how success and failure will be verified. Include affected callers and removal of replaced paths. Prefer plain language and outcomes over file-by-file recipes.

Follow the [project standards](../../copilot-instructions.md). Link required [validation](../validate/SKILL.md) rather than copying rules into every step. Preserve useful existing decisions and evidence; update stale notes instead of adding competing instructions.

Apply a numbered file with [apply-step](../apply-step/SKILL.md), or the whole folder with [apply-plan](../apply-plan/SKILL.md).
