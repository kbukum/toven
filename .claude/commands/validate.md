---
description: Build, test, lint, format-check, and structure-check Toven changes through cargo and make — scoped to the crates that actually changed. Use whenever you need to validate a Toven change, run tests for a crate, reproduce CI locally, or check the affected area of an edit before committing.
---

# /validate — router to the canonical skill

This command is a **thin router**. The single source of truth for this workflow is the
project skill at [`.github/skills/validate/SKILL.md`](../../.github/skills/validate/SKILL.md).

**Do this now:** read `.github/skills/validate/SKILL.md` in full — plus every reference file it
links — and execute it exactly as written, applying it to the scope below. Do not act on any
summary; the skill file is authoritative and kept up to date. This router only exists so the
Claude Code slash command and the Copilot skill never drift.

Scope / arguments: $ARGUMENTS
