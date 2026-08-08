# Toven development skills

[Agent Skills](https://docs.github.com/copilot/concepts/agents/about-agent-skills) for developing **Toven itself** — loaded on demand by GitHub Copilot (CLI, coding agent, code review, IDEs) when a task matches a skill's description. These are **project skills** for contributors; they do not affect anyone who consumes Toven or its crates.

Each skill is a folder with a `SKILL.md` (YAML frontmatter + workflow) and optional bundled reference files loaded only when the skill activates (progressive disclosure). They encode Toven's permanent engineering baseline (see [`../copilot-instructions.md`](../copilot-instructions.md) and [`docs/engineering.md`](../../docs/engineering.md)) and its hexagonal architecture (see [`docs/architecture.md`](../../docs/architecture.md)).

## Skills

| Skill | Use when |
|---|---|
| [`create-branch`](create-branch/SKILL.md) | Cut a branch off an up-to-date main, named by the high-level change (no batch/plan/internal detail). |
| [`create-plan`](create-plan/SKILL.md) | Turn a non-trivial change into a reviewable plan under `tmp/` — README + numbered step files, bound to the baseline. |
| [`apply-plan`](apply-plan/SKILL.md) | Execute a `tmp/` plan from its first unfinished step onward, validating after each; resumable. |
| [`apply-step`](apply-step/SKILL.md) | Apply one plan step in context (README + prior steps), test-first against the baseline, then mark it done. |
| [`commit`](commit/SKILL.md) | Commit staged work with one compact, developer-friendly message — no co-author trailer or plan/batch/tool narration. |
| [`create-pr`](create-pr/SKILL.md) | Open a reviewer-friendly PR — high-level summary, honest template sections, bound to the baseline. |
| [`fix-issue`](fix-issue/SKILL.md) | Fix a GitHub issue to root cause — understand and reproduce it, investigate against the baseline, plan, implement completely (redesign over patching, no compat shims), and validate. |
| [`fix-reviews`](fix-reviews/SKILL.md) | Act on PR review comments by pattern — fix every instance across the change set, then commit and resolve the threads. |
| [`validate`](validate/SKILL.md) | Build/test/lint/format/doc/deny a change through cargo/make, scoped to the affected crates. |
| [`review`](review/SKILL.md) | Run the nine-pass engineering-baseline review over a diff, crate, or the tree. |
| [`new-crate`](new-crate/SKILL.md) | Scaffold a new crate — hexagonal layer placement, port rule, workspace wiring, testkit double. |
| [`rskit-reuse`](rskit-reuse/SKILL.md) | Reuse the vendored rskit foundation for shared concerns; improve rskit generically instead of forking Toven-local copies. |
| [`release`](release/SKILL.md) | Cut a release — semver bump, CHANGELOG, workspace version, full gates, then tag so CI ships the signed source artifact, SBOM, and provenance (no crates.io publish). |
| [`docs`](docs/SKILL.md) | Review/update docs to the repo's standards (flowing paragraphs without hard column wrapping) and up-to-date accuracy (commands, flags, structure, examples match the code). |

## Conventions

- Skills are discoverable in Copilot CLI via `/skills`; project skills live under `.github/skills/` (also `.claude/skills` / `.agents/skills` are honored), personal skills under `~/.copilot/skills`.
- Claude Code slash commands under [`.claude/commands/`](../../.claude/commands/) are **thin routers** to these skills — each `/<name>` points at `.github/skills/<name>/SKILL.md`, which is the single source of truth. Edit the `SKILL.md`, never the router body.
- Run reviews (`review`) in a **fresh, clean-context agent**, never inline in the session that wrote the code.
- Validation is scoped to the changed crates: prefer `cargo clippy -p <crate>` / `cargo test -p <crate>` and targeted `make` gates over blanket `--workspace` runs.
- Initialize the rskit submodule before building or validating: `git submodule update --init --recursive`.
- `tmp/` is gitignored working scratch for plans; it is never committed.
