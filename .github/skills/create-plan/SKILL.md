---
name: create-plan
description: "toven: Write or revise a high-level implementation plan with dependencies and acceptance criteria."
user-invocable: true
---

# Plan a change

Planning writes task documents only: no source edits, branch changes, staging, commits, or PRs. Apply the [baseline](../../copilot-instructions.md); a plan cannot weaken it.

## Scope and storage

Investigate the current owning modules and contracts before deciding. Record the goal, non-goals, constraints, decisions, and measurable acceptance. Prefer owner-level outcomes over prescriptive filenames or code recipes; investigate exact implementation at apply time. Name known removal targets when needed to prove complete replacement.

Reuse the existing task folder. New plans live in gitignored `tmp/plans/<task>/`, named for the change. Update `tmp/plans/README.md` and the task README; update `tmp/README.md` if it indexes plans. Never link stable docs to temporary task notes.

## Executable shape

- `README.md`: goal, scope, ordered step index, dependencies, and links to binding rules.
- `NN-topic.md`: one reviewable step/PR with `**Status:** pending`, `**Depends on:**`, scope, owner-level work order, removals, and `- [ ]` acceptance checks. Numbering orders documents, not branch names. Use work orders within a step to bound sessions; do not split one step across multiple PRs.
- `handoff.md`: under 500 words; branch/Git restrictions, current capabilities, decisions, remaining work, next action, evidence freshness/paths, owned resources, and continuation prompt.

A small single-step plan may keep the work order in its README. Add separate context, decisions, current-state, references, or open-question documents only when their content is needed; avoid empty boilerplate and repeated policy.

## Acceptance and continuation

Order dependencies before consumers. Require test-first behavior/failure coverage, canonical ownership, correct layering, complete dependent call-site/removal updates, and the relevant [validation](../validate/SKILL.md) and [review](../review/SKILL.md) checks. Use the repository's real gate names and integration evidence; do not claim a configured threshold from an old example. Keep required release/Changeset and UI acceptance where applicable. Link rules once rather than copying the baseline into every step.

A step becomes `done` only when its acceptance is verified. The handoff should let the next session read the current step and needed dependency contracts, not every previous step. Keep capability summaries rather than transcripts; flag evidence predating edits. Finish one bounded work order, checkpoint, and stop.

Apply later with [apply-plan](../apply-plan/SKILL.md) or [apply-step](../apply-step/SKILL.md).
