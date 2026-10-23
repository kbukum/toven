# Review changes

Review a change set and its blast radius. Follow the [review skill](../SKILL.md) for lenses, severity, and finding format.

## Scope

- Get the diff: `git diff <base>...HEAD --stat`, `git diff --stat` and `git diff --cached --stat` for worktree changes, then per file.
- Read the rest of each touched file, its close callers and callees, and the contracts it changes. Pre-existing defects in that area are in scope; do not widen into an unrelated audit.
- For each changed model or schema type, list the layers that should change with it (schema, normalization, planner, executor, output, tests, docs). Incomplete cascades are the most common defect.
- Initialize the submodule if needed: `git submodule update --init --recursive`.

## Lenses

Pick lenses by trigger from the [review skill](../SKILL.md#lenses). Stop acceptance on a placement or reuse blocker.

## Validation

Scope gates to the touched crates with the commands in the [validate skill](../../validate/SKILL.md). Run `make check` only for workspace-wide or release-bound changes, or leave full sign-off to CI and say so.
