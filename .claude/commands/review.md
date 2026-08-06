---
description: Run Toven's standing engineering-baseline review over a change set (a branch, commit range, or HEAD~1) or over a whole crate/area/tree. Sequences nine focused passes — structure & placement, rskit reuse, principles, security & privacy, quality, tests/TDD, docs & supply chain, comments & rustdoc, CLI UX. Use before merging a change, when auditing a crate, or before a release. Always run it in a fresh, clean-context reviewer.
---

# /review — router to the canonical skill

This command is a **thin router**. The single source of truth for this workflow is the
project skill at [`.github/skills/review/SKILL.md`](../../.github/skills/review/SKILL.md).

**Do this now:** read `.github/skills/review/SKILL.md` in full — plus every reference file it
links — and execute it exactly as written, applying it to the scope below. Do not act on any
summary; the skill file is authoritative and kept up to date. This router only exists so the
Claude Code slash command and the Copilot skill never drift.

Scope / arguments: $ARGUMENTS
