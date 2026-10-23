---
name: create-pr
description: "toven: Open a draft pull request using the repository template; only when explicitly asked."
user-invocable: true
---

# Opening a reviewer-friendly pull request

A PR is a reviewer's entry point, not a changelog of your keystrokes. This skill turns a pushed branch into a PR whose description explains the change **at a high level, organized and simplified**, and whose template sections are filled **honestly** against Toven's baseline.

Create a PR **only when explicitly asked** — never as a side effect of finishing work.

## 1. Preconditions

- The branch is committed and **pushed** to `origin`. Commit and push only with the user's authorization. Confirm before opening:

```bash
git rev-parse --abbrev-ref HEAD
git status --short                     # expect clean; uncommitted work is not in the PR
git log --oneline origin/main..HEAD    # the commits this PR will contain
```

- Base is `main`. If the branch isn't on the remote yet, ask the maintainer to push rather than pushing for them.

## 2. Understand the change at a high level

Read the actual diff and group it by concern — do not narrate per file:

```bash
git diff origin/main...HEAD --stat
git diff origin/main...HEAD            # skim for the shape of the change, not to transcribe it
```

Answer, in your head: what capability/fix/refactor is this, which crates and layers it touches, whether the change is **cascade-complete** (model → planner → executor → output → tests → docs), and what a reviewer must understand to judge it.

## 3. Write the description — high level, organized, simplified

Fill every section of the [pull request template](../../PULL_REQUEST_TEMPLATE.md). A reviewer should grasp the change from the description alone, without a file list.

- **Title:** Conventional Commit style naming the change, such as `feat(engine): plan-result cache`. No plan, batch, or step numbers. CI checks the title.
- **Summary:** a few bullets on what changed and why, at the level of capabilities and decisions. Link issues as `Fixes #123`; link other repositories by full URL.
- **Review context:** the problem, key design choices, and anything left for a follow-up.
- **Type of change:** tick every option that applies, including **Breaking change** with its `CHANGELOG.md` entry.
- **Validation:** tick only the checks you ran, using the [validate skill](../validate/SKILL.md). Never fabricate output.
- **Risk and rollout:** breaking changes, migration notes, or why there is no user-visible risk.
- **Checklist:** it mirrors the [Definition of Done](../../../docs/engineering.md#definition-of-done). Tick only what is true; an unticked box is honest signal.

Keep prose tight: no process narration, no "previously we…", no code snippets.

## 4. Create it with `gh`

Write the body to a file so formatting survives, then open a draft against `main`. The maintainer marks it ready for review:

```bash
gh pr create --draft --base main --title "<conventional-title>" --body-file <path>
```

- Do **not** add reviewers or request Copilot review unless explicitly asked.
- Report the PR URL back. If asked to follow up on review threads, resolve them without posting replies under the maintainer's name.

## Baseline

The PR claims the change meets the [Definition of Done](../../../docs/engineering.md#definition-of-done). If it does not yet, run the [review skill](../review/SKILL.md) and fix findings first.
