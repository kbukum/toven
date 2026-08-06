---
description: Reuse the vendored rskit foundation before writing any shared concern in Toven (errors, config, validation, filesystem, git, process, logging) — and when rskit is missing or inadequate, improve rskit generically rather than forking a Toven-specific copy. Use before adding cross-cutting infrastructure, or when a review flags a reimplemented concern.
---

# /rskit-reuse — router to the canonical skill

This command is a **thin router**. The single source of truth for this workflow is the
project skill at [`.github/skills/rskit-reuse/SKILL.md`](../../.github/skills/rskit-reuse/SKILL.md).

**Do this now:** read `.github/skills/rskit-reuse/SKILL.md` in full — plus every reference file it
links — and execute it exactly as written, applying it to the scope below. Do not act on any
summary; the skill file is authoritative and kept up to date. This router only exists so the
Claude Code slash command and the Copilot skill never drift.

Scope / arguments: $ARGUMENTS
