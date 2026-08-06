---
description: Execute an existing plan folder under tmp/ from its first unfinished step onward — read the plan's README for order and dependencies, then apply each remaining step in turn (via the apply-step workflow), validating after each. Resumable and idempotent. Use when asked to apply, execute, continue, or resume a plan.
---

# /apply-plan — router to the canonical skill

This command is a **thin router**. The single source of truth for this workflow is the
project skill at [`.github/skills/apply-plan/SKILL.md`](../../.github/skills/apply-plan/SKILL.md).

**Do this now:** read `.github/skills/apply-plan/SKILL.md` in full — plus every reference file it
links — and execute it exactly as written, applying it to the scope below. Do not act on any
summary; the skill file is authoritative and kept up to date. This router only exists so the
Claude Code slash command and the Copilot skill never drift.

Scope / arguments: $ARGUMENTS
