---
description: Cut a release of Toven — decide the semver bump, update the CHANGELOG, set the workspace version, run the full pre-release gate and supply-chain sweep, land the version commit on protected `main` through a reviewed PR, then dispatch the gated Release workflow whose `toven release publish` step creates the tag and hosted Release with per-target signed binaries, SBOM, and provenance. Toven ships tagged, signed binary artifacts (all crates are publish = false) — it does not publish to crates.io. Use when preparing or publishing a Toven release or checking release readiness.
---

# /release — router to the canonical skill

This command is a **thin router**. The single source of truth for this workflow is the
project skill at [`.github/skills/release/SKILL.md`](../../.github/skills/release/SKILL.md).

**Do this now:** read `.github/skills/release/SKILL.md` in full — plus every reference file it
links — and execute it exactly as written, applying it to the scope below. Do not act on any
summary; the skill file is authoritative and kept up to date. This router only exists so the
Claude Code slash command and the Copilot skill never drift.

Scope / arguments: $ARGUMENTS
