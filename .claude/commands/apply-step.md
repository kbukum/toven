---
description: Apply a single step of a tmp/ plan — read the plan README and all previous steps for accumulated context and decisions, then implement the current step test-first against Toven's engineering baseline, validate the affected crates, and mark the step done. Use to execute one specific plan step, or as the per-step unit that apply-plan drives.
---

# /apply-step — router to the canonical skill

This command is a **thin router**. The single source of truth for this workflow is the
project skill at [`.github/skills/apply-step/SKILL.md`](../../.github/skills/apply-step/SKILL.md).

**Do this now:** read `.github/skills/apply-step/SKILL.md` in full — plus every reference file it
links — and execute it exactly as written, applying it to the scope below. Do not act on any
summary; the skill file is authoritative and kept up to date. This router only exists so the
Claude Code slash command and the Copilot skill never drift.

Scope / arguments: $ARGUMENTS
