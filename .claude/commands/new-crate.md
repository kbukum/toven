---
description: Scaffold a new crate in Toven's hexagonal Cargo workspace the canonical way — place it in the right layer (model/ports/engine/adapter/cli), honor the binding port-placement rule, wire the workspace, inherit workspace lints (#![forbid(unsafe_code)], missing_docs), and add its shared double to toven-testkit. Use when adding a capability, port, adapter, or crate to Toven.
---

# /new-crate — router to the canonical skill

This command is a **thin router**. The single source of truth for this workflow is the
project skill at [`.github/skills/new-crate/SKILL.md`](../../.github/skills/new-crate/SKILL.md).

**Do this now:** read `.github/skills/new-crate/SKILL.md` in full — plus every reference file it
links — and execute it exactly as written, applying it to the scope below. Do not act on any
summary; the skill file is authoritative and kept up to date. This router only exists so the
Claude Code slash command and the Copilot skill never drift.

Scope / arguments: $ARGUMENTS
