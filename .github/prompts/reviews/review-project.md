# Review project

Standing, re-runnable **whole-project audit**, independent of any diff. Use it periodically, before a release, when onboarding to a crate, or whenever you want assurance the tree as a whole still honors the baseline. It sequences the same seven focused passes in [`reviews/`](./) but over the existing code rather than a change set.

## Run this in a separate, clean-context agent

**Always dispatch this audit to a fresh agent with no shared session context.** The point of a full audit is an independent read of the code as it exists — not filtered through whatever a prior session believed about it. Do not run it inline in a session that has been editing the same code.

- Hand the agent: the crate(s)/area to audit (or "the whole workspace"), this file, and the [`reviews/`](./) folder.
- The agent judges the code as written, against the principles in [`docs/engineering.md`](../../../docs/engineering.md) and [`docs/architecture.md`](../../../docs/architecture.md) — not against any session's recollection.
- **Optional plan/roadmap check.** If there is a roadmap, phase plan, or release-readiness doc (e.g. under `tmp/` or an issue), pass it in *as context for intended state only* — "here is where the project is meant to be; flag where the tree has not caught up." It frames expectations; it never excuses a baseline violation.

## Pass 0 — Scope and context

- Choose the audit surface: the whole workspace, or a specific crate/area. State it up front so findings are bounded.
- Initialize the submodule: `git submodule update --init --recursive` (pass `01` reads `rskit/`).
- Get a structural picture before diving in: list crates and their dependency blocks, skim each `src/` tree.

```bash
ls crates
for c in crates/*/Cargo.toml; do echo "== $c =="; rg '^toven-|^rskit-' "$c"; done
```

## Passes — run in order

Work the focused files top to bottom; each carries a "Project mode" scope note describing how to sweep the whole tree for that lens.

1. [`00-structure-placement.md`](./00-structure-placement.md) — layering invariants, port placement, `mod.rs` guard, file homes across every crate.
2. [`01-rskit-reuse.md`](./01-rskit-reuse.md) — sweep for local forks of rskit-owned concerns (errors, config, validation, fs, git, process, logging, hashing). *(blocker class)*
3. [`02-principles.md`](./02-principles.md) — print/panic/argv/security invariants across the full library surface; spot-check end-to-end cascades.
4. [`03-quality.md`](./03-quality.md) — dead code, lingering compatibility shims, outdated patterns, style gates.
5. [`04-tests-tdd.md`](./04-tests-tdd.md) — coverage of behavior and failure paths, fixtures vs. inline TOML, stranded doubles, determinism.
6. [`05-docs-supply-chain.md`](./05-docs-supply-chain.md) — docs policy (`tmp/` refs, hard-wrapping), Conventional Commits, `Cargo.lock`, rskit pin/submodule parity, `cargo-deny`, SHA-pinned actions.
7. [`06-comments-rustdoc.md`](./06-comments-rustdoc.md) — sweep all source prose: comments and `///` docs describe the current code, not plans/history; rewrite or delete the rest.

When you only need one lens across the project (e.g. a standalone security or TDD sweep), run that focused file directly with its "Project mode" note.

## Findings

Record every finding as:

```
severity (blocker / should-fix / nit) — file:line — what's wrong — which principle — suggested fix
```

Group findings by crate and by pass so the report is actionable. See [`README.md`](./README.md) for severity definitions.

## Validation

Run the full gate; for a scoped audit, also run the focused crate tests:

```bash
git submodule update --init --recursive
make structure && make fmt-check && make lint && make test && make doc && make deny
make coverage    # workspace coverage gate
make check       # full canonical gate
```

A green `make check` is necessary but **not sufficient** — layering-by-convention, cascade gaps, rskit-reuse violations, and weak tests are on the reviewer, not the gate.
