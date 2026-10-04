---
name: review
description: "toven: Review a diff, package, or tree for architecture, correctness, security, tests, and documentation."
user-invocable: true
---

# Review

Apply the [baseline](../../copilot-instructions.md). Review code and evidence, not the author's rationale; plans define scope, not exceptions to requirements.

## Scope and execution

Review directly by default. Launch an independent reviewer only when the user requests it; use one bounded reviewer unless distinct additional scopes are explicitly requested. Pass the scope, constraints, and relevant files, not the authoring transcript. Reevaluate returned findings against source before acting.

For changes, include staged/unstaged/untracked work when requested, plus close callers, callees, and affected contracts. Report pre-existing defects in that impact area; do not silently widen into an unrelated audit. For project review, state the package/domain boundary.

Reviews are read-only unless fixes are authorized. For each real defect choose **Redesign / Align / Enhance / Drop** with a one-line reason. Preserve user staging and unrelated work; do not commit, amend, push, resolve remote threads, or post replies implicitly.

## Load checks by trigger

Check placement and canonical reuse first for code changes. A blocker there stops acceptance; report any unperformed checks rather than claiming a complete review. Apply every relevant lens below, in order; load only its file, not the entire reference directory. For prose-only work, use documentation/claim checks rather than runtime test suites.

| Trigger | Checklist |
|---|---|
| Code placement, imports, exports, or package wiring | [Pass 00 — Structure and placement](references/00-structure-placement.md) |
| Shared behavior or a proposed helper/adapter | [Pass 01 — rskit reuse](references/01-rskit-reuse.md) |
| Runtime APIs, errors, concurrency, or composition | [Pass 02 — Following principles](references/02-principles.md) |
| Trust boundaries, auth, crypto, privacy, or external input | [Pass 03 — Security & privacy](references/03-security-privacy.md) |
| Behavior, readability, maintainability, or dead code | [Pass 04 — Quality: simplicity, maintainability, flexibility, up-to-date code](references/04-quality.md) |
| Behavior changes, tests, fixtures, or coverage | [Pass 05 — TDD and tests](references/05-tests-tdd.md) |
| Documentation, dependencies, CI, or release | [Pass 06 — Docs and supply chain](references/06-docs-supply-chain.md) |
| Comments and public API documentation | [Pass 07 — Comments and rustdoc](references/07-comments-rustdoc.md) |
| CLI behavior or output | [Pass 08 — CLI user experience](references/08-cli-ux.md) |

## Modes

- [Changes](references/review-changes.md): diff and affected contracts.
- [Project](references/review-project.md): bounded tree audit.
- [Review and fix](references/review-details.md): select only when fixes are requested; confirm the proposed fix scope before editing.

## Evidence and output

Use [validate](../validate/SKILL.md) for scoped gates; reuse fresh evidence and retain required full acceptance gates. Green tooling is necessary, not proof of lifecycle/security correctness.

Report `severity | file:line | defect and impact | evidence | proposed action`. **Blocker** means broken behavior/contract or a hard-rule violation; **should-fix** means a substantiated maintainability defect; **nit** is optional style, never a gate. List incomplete/skipped checks and uncertainty. Do not invent findings or treat grep hits as proof.
