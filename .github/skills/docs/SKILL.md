---
name: docs
description: "toven: Update or audit documentation for accuracy, clear prose, working examples, and links."
user-invocable: true
---

# Documentation

Apply the [baseline](../../copilot-instructions.md). Scope to requested docs and directly affected references, including rustdoc and agent docs.

1. Verify Makefile targets, CLI verbs/flags/defaults/streams, strict config keys, crate structure, and examples against current code. Human/status output and machine output must be documented accurately.
2. Lead how-to pages with the shortest working example. Use plain active sentences, useful headings, and option tables. Add a focused captioned Mermaid diagram for non-trivial PLAN/APPLY, dependency, or state flow.
3. Do not hard-wrap Markdown or Rust prose. Preserve lists, directives, code, paragraphs, and meaningful hard breaks; never blindly join lines.
4. Remove stale usage/process narration, not historical changelogs or accepted ADRs. Stable docs live in `docs/` and must not link to temporary plans.
5. Check links/anchors. Run `make doc` when rustdoc changes and scoped doctests for executable examples; a docs build alone does not execute doctests. Prose-only changes need no application build.

For agent docs, keep startup rules and descriptions short. Put task steps/acceptance in the skill and load detailed references only as needed. Preserve hard requirements, valid metadata, and links.

Commit only when explicitly requested, using the commit skill.
