---
name: apply-step
description: "toven: Implement one plan step test-first, validate its acceptance criteria, and record progress."
user-invocable: true
---

# Apply one step

Given a step file such as `tmp/<plan>/02-topic.md`, implement the whole step through acceptance. Do not stop at internal phases or create routine handoffs.

1. **Read.** Open the plan README, selected step, and relevant dependency contracts. Check current source and Git state; preserve existing edits/index. If resuming, verify earlier evidence still matches. Confirm prerequisites are complete.
2. **Implement.** Follow the [project standards](../../copilot-instructions.md). Work test-first, including failure paths. Fix the owning code, update affected callers/docs, and remove superseded paths. Use the simplest complete design; reassess unfinished code rather than preserving defects.
3. **Verify.** Run the relevant [validation](../validate/SKILL.md) and [review](../review/SKILL.md) checks. Include required generated outputs, release notes, and real integration/UI proof. Fix in-scope failures and rerun affected checks.
4. **Finish.** Mark the step `done` only when every acceptance check passes. Record the result and evidence in the step. Leave changes uncommitted unless the user authorizes Git actions.

Continue authorized work without asking at each milestone. Ask only for a decision, permission, or scope change that genuinely blocks progress. Do not waive a failed gate; if work must pause, note the exact blocker and remaining work in the existing plan. Applying this step does not authorize starting the next one.
