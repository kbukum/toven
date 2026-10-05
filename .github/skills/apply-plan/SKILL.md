---
name: apply-plan
description: "toven: Implement an existing plan in dependency order, completing and validating each step."
user-invocable: true
---

# Apply a plan

Given `tmp/<plan>/` or an existing plan path:

1. Read its README and find the first unfinished step whose prerequisites are complete.
2. Use [apply-step](../apply-step/SKILL.md) to implement, validate, and review that entire step.
3. Record its result, then continue through the requested plan in dependency order.

Follow the [project standards](../../copilot-instructions.md). Preserve existing edits, each step's PR boundary, and Git permissions. Do not combine separately scoped changes on one branch without authorization.

Internal milestones are not stopping points. Do not proceed past a failed prerequisite or mark incomplete work done. If a real blocker prevents progress, record it in the existing plan; no separate handoff is required.
