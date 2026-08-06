---
description: Review and update Toven's documentation so it reads naturally and reflects the project as it is today — keep Markdown paragraphs flowing without hard column wrapping, preserve intentional document structure, sync commands, structure, flags, and examples with the actual Makefile/CLI/crates, fix outdated links and dead references, drop history/plan narration, keep prose humanized and scannable with a task-first quickstart, and add mermaid diagrams where they clarify architecture or flow. Use when writing or auditing docs, repairing AI-generated hard wraps, after a behavior change that outdated docs, or before a release.
---

# /docs — router to the canonical skill

This command is a **thin router**. The single source of truth for this workflow is the
project skill at [`.github/skills/docs/SKILL.md`](../../.github/skills/docs/SKILL.md).

**Do this now:** read `.github/skills/docs/SKILL.md` in full — plus every reference file it
links — and execute it exactly as written, applying it to the scope below. Do not act on any
summary; the skill file is authoritative and kept up to date. This router only exists so the
Claude Code slash command and the Copilot skill never drift.

Scope / arguments: $ARGUMENTS
