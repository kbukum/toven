---
description: Evaluate a pull request's review comments as signals of an underlying pattern, not one-off spot fixes — judge each comment against Toven's engineering baseline, then apply the pattern across the whole change set (e.g. one typo comment → sweep every changed file for typos), validate, commit the fixes, and resolve the threads. Use when asked to go over, address, or act on PR reviews.
---

# /fix-reviews — router to the canonical skill

This command is a **thin router**. The single source of truth for this workflow is the
project skill at [`.github/skills/fix-reviews/SKILL.md`](../../.github/skills/fix-reviews/SKILL.md).

**Do this now:** read `.github/skills/fix-reviews/SKILL.md` in full — plus every reference file it
links — and execute it exactly as written, applying it to the scope below. Do not act on any
summary; the skill file is authoritative and kept up to date. This router only exists so the
Claude Code slash command and the Copilot skill never drift.

Scope / arguments: $ARGUMENTS
