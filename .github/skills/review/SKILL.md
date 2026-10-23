---
name: review
description: "toven: Review a diff, package, or tree for architecture, correctness, security, tests, and documentation."
user-invocable: true
---

# Review

Judge the code and evidence, not the author's rationale. A plan or PR description sets scope; it never excuses a rule. The rules live in the [engineering standard](../../../docs/engineering.md); lenses below say how to check them.

## Modes

| Mode | Use when | Procedure |
|---|---|---|
| Changes | Reviewing a diff, branch, commit range, or the worktree | [mode-changes.md](references/mode-changes.md) |
| Project | Auditing a crate or the whole workspace, in priority order | [mode-project.md](references/mode-project.md) |
| Review and fix | The user asked for fixes, not only findings | [mode-fix.md](references/mode-fix.md) |

Reviews are read-only unless fixes are authorized. Review directly; start an independent reviewer only when the user asks. Preserve the worktree and index. Never commit, amend, push, resolve threads, or post replies as part of a review.

## Lenses

Pick lenses by trigger and run them in this order. Load only the files you need. Placement and reuse come first: a blocker there stops acceptance.

| Lens | Trigger | File |
|---|---|---|
| Placement and layering | New code, imports, exports, Cargo wiring | [placement.md](references/placement.md) |
| rskit reuse | A new helper, adapter, error type, or shared concern | [rskit-reuse.md](references/rskit-reuse.md) |
| Security | Untrusted input, paths, subprocesses, secrets, crypto | [security.md](references/security.md) |
| Runtime correctness | Errors, async, concurrency, lifecycle, model cascade | [runtime-correctness.md](references/runtime-correctness.md) |
| Public API and stability | `pub` items, error types, JSONL, exit codes, config schema | [public-api.md](references/public-api.md) |
| Tests and proof | Any behavior change, fixture, double, or coverage | [tests.md](references/tests.md) |
| CLI experience | Flags, help, messages, output streams, `init` | [cli-ux.md](references/cli-ux.md) |
| Code quality | Shims, dead code, outdated idioms, readability | [quality.md](references/quality.md) |
| Comments and rustdoc | Changed or stale comments and API docs | [comments-rustdoc.md](references/comments-rustdoc.md) |
| Docs and supply chain | Markdown, dependencies, CI workflows, release | [docs-supply-chain.md](references/docs-supply-chain.md) |

Prose-only changes need only the docs lens. Name every lens you skipped and why.

## Severity

| Severity | Meaning | Gate |
|---|---|---|
| **blocker** | Wrong behavior, a broken contract, a security hole, or a hard-rule violation | Must be fixed before acceptance |
| **should-fix** | A real, evidenced maintainability or consistency defect | Fix in this change unless the user defers it |
| **nit** | Optional style or wording | Never blocks |

For every real defect choose **Redesign / Align / Enhance / Drop** with a one-line reason. Toven is pre-stable, so prefer a root-cause redesign over a shim.

## Findings

One line per finding:

```text
severity | file:line | problem and impact | rule link | action
```

The rule link points to an [engineering](../../../docs/engineering.md) anchor. Group by crate in project mode. Detection commands find candidates, not proof: read each hit before reporting it.

## Evidence

Use the [validate skill](../validate/SKILL.md) for gates. A green gate is necessary, not sufficient: rules marked **Review** in the engineering standard are checked only by you. List checks you did not run and anything you are unsure of; never claim a skipped check passed.
