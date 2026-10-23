# Review and fix

Use only when the user asked for fixes. Follow the [review skill](../SKILL.md) for lenses, severity, and finding format.

1. **Review.** Run [changes](mode-changes.md) or [project](mode-project.md) mode and collect findings.
2. **Plan.** Group findings by lens and severity. For each group write one line: what changes, where, and how it is verified. Flag real choices (behavior change vs. strict fix, breaking change vs. redesign, doc fix vs. code fix) with a proposed default.
3. **Confirm.** Pause for the user to confirm the fix scope before editing, unless they already approved it.
4. **Apply.** Fix in plan order, test-first for behavior. Re-run the matching scoped check after each fix; stop and report on failure.
5. **Close.** Re-run the scoped gates from the [validate skill](../../validate/SKILL.md), then report fixed, deferred, and skipped items.

Preserve the worktree and index. Commit, amend, or push only with separate authorization.
