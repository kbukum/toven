# Opening a reviewer-friendly pull request

A PR is a reviewer's entry point, not a changelog of your keystrokes. This command turns a pushed branch into a PR whose description explains the change **at a high level, organized and simplified**, and whose template sections are filled **honestly** against Toven's baseline.

Create a PR **only when explicitly asked** — never as a side effect of finishing work.

## 1. Preconditions

- The branch is committed and **pushed** to `origin` (the maintainer commits and pushes, per repo workflow). Confirm before opening:

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

Fill every section of [`.github/PULL_REQUEST_TEMPLATE.md`](.github/PULL_REQUEST_TEMPLATE.md). The guiding rule: **a reviewer should grasp the change from the description alone**, without reconstructing it from the diff. Toven's template asks for reviewer-focused intent, not a file list.

- **Title** — Conventional Commit style, naming the change: `feat(engine): plan-result cache`, `refactor(ports): move ToolchainProber double to testkit`. No plan/batch/step numbers.
- **Description** — a few sentences of *what changed and why it's shaped this way*, at the level of capabilities and decisions, not code lines.
- **Motivation** — the problem it solves. Link issues as `Fixes #123`; reference **other repos as full URLs** (e.g. `https://github.com/kbukum/rskit/issues/45`), never a bare `#45`.
- **Changes / crates affected** — a short, grouped bullet list of the *key* changes by concern, not one bullet per file and not a commit log.
- **Testing** — check only the gates you actually ran; scope to affected crates. These map to the `/validate` command: `cargo test -p <crate>`, `cargo clippy -p <crate> -- -D warnings`, `make structure`. Paste real evidence if useful; don't fabricate output.
- **Checklist** — tick only what is genuinely true. An unchecked box is honest signal; a falsely checked one wastes reviewer trust. Do not narrate prior bugs or how they were fixed.

Keep prose tight: no process narration, no "previously we…", no restating the diff.

## 4. Create it with `gh`

Write the body to a file so formatting survives, then open against `main`:

```bash
gh pr create --base main --title "<conventional-title>" --body-file <path>
```

- Do **not** add reviewers unless explicitly asked.
- Report the PR URL back. If asked to follow up on review threads, resolve them without posting replies under the maintainer's name.

## Baseline

The PR asserts the change meets Toven's engineering baseline ([`docs/engineering.md`](docs/engineering.md)); if it doesn't yet, run the `/review` command first and fix findings rather than opening a PR that fails its own checklist.
