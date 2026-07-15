---
name: create-branch
description: >-
    Create a new git branch for a piece of work the canonical way — branch off an up-to-date
    main by default (or an explicitly named base branch), and name it by the high-level change
    only, never by internal/local scaffolding like batch numbers, plan numbers, or task IDs. Use
    whenever you start new work, cut a branch, or are unsure what to name a branch.
user-invocable: true
---

# Creating a branch for Toven work

A branch and its eventual PR must read as a **standalone unit of change** to a reviewer who has
no knowledge of how the work was planned or sequenced. Two rules make that true: branch off the
right, up-to-date base, and name the branch after the actual change — nothing else.

## Golden rule: branch off an up-to-date main

Unless the request explicitly says to build on another branch, always base new work on the latest
`origin/main`. Never branch off a stale local `main` or whatever happens to be checked out.

```bash
git fetch origin                       # refresh remote refs first — always
git switch -c <branch-name> origin/main
```

If (and only if) the request says to stack on top of another branch, base on that branch instead
and keep it current:

```bash
git fetch origin
git switch -c <branch-name> origin/<base-branch>   # explicit base, per the request
```

Before switching, check the working tree with `git status`. Uncommitted changes follow you onto
the new branch — confirm that is intentional rather than dragging along unrelated edits. Note the
`rskit/` submodule is a separate checkout; branch names here refer to the Toven superproject.

## Naming: describe the change, not the plumbing

The branch name captures **what the change does at a high level** — the capability, fix, or
refactor a reviewer will see. It must not leak how the work was organized internally.

- **Prefix with your username**, then a short kebab-case summary of the change:
  `<username>/<short-change-summary>` (e.g. `kbukum/argv-selector-expansion`).
- Name by the actual change: the crate/capability touched and the outcome
  (`kbukum/engine-plan-caching`, `kbukum/rust-adapter-toolchain-prober`).
- Keep it short, lowercase, hyphen-separated; no spaces, no `wip`, no trailing noise.

**Never** put internal, sequencing, or local-only information in the name:

- ❌ plan/batch/group numbers: `batch-5-engine`, `group7-cleanup`, `phase2`
- ❌ task/ticket scaffolding that isn't the change itself, internal roadmap step numbers
- ❌ session-, machine-, or tool-local detail

If you catch yourself writing `batch-5-...`, ask "what does this change actually do?" and name it
that instead. Each branch stands alone; there is no "batch 5" from a reviewer's perspective.

## After the branch exists

Per repo workflow, this skill **creates the branch and leaves the edits uncommitted** — the
maintainer commits and pushes, and opens a PR only when explicitly asked. Do not commit, push, or
open a PR as part of creating the branch.
