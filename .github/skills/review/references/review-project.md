# Review project

Standing, re-runnable **whole-project audit**, independent of any diff. Use it periodically, before a release, when onboarding to a crate, or whenever you want assurance the tree as a whole still honors the baseline. It sequences the same nine focused passes in [`references/`](./) but over the existing code rather than a change set.

## Execution

Follow [the review skill](../SKILL.md): direct review by default; independent agents only on request. Read current source and relevant contracts. A plan is a scope checklist, not a justification for a baseline violation.

## Pass 0 — Scope and context

- Choose the audit surface: the whole workspace, or a specific crate/area. State it up front so findings are bounded.
- Initialize the submodule: `git submodule update --init --recursive` (pass `01` reads `rskit/`).
- Get a structural picture before diving in: list crates and their dependency blocks, skim each `src/` tree.

```bash
ls crates
for c in crates/*/Cargo.toml; do echo "== $c =="; rg '^toven-|^rskit-' "$c"; done
```

## Passes

Follow the trigger table and order in [the review skill](../SKILL.md). Use each checklist's project scope. Load applicable files only; report incomplete checks and stop acceptance on structural/reuse blockers.

## Findings

Record every finding as:

```
severity (blocker / should-fix / nit) — file:line — what's wrong — which principle — suggested fix
```

Group findings by crate and by pass so the report is actionable. See [`SKILL.md`](../SKILL.md) for severity definitions.

## Validation

Run the full gate; for a scoped audit, also run the focused crate tests:

```bash
git submodule update --init --recursive
make structure && make fmt-check && make lint && make test && make doc && make deny
make coverage    # workspace coverage gate
make check       # full canonical gate
```

A green `make check` is necessary but **not sufficient** — layering-by-convention, cascade gaps, rskit-reuse violations, and weak tests are on the reviewer, not the gate.
