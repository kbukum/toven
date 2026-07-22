# Reviewing Toven against its engineering baseline

Toven is an argv-first task planner built on the vendored rskit foundation (a git submodule). A defect in a lower crate propagates up the hexagonal stack to the CLI and every generated command batch. This command encodes Toven's permanent review baseline as eight focused passes plus orchestrators.

The authoritative baseline lives in [`docs/engineering.md`](docs/engineering.md) (and [`docs/architecture.md`](docs/architecture.md)). A plan, spec, issue, or roadmap may be passed in **as a scope checklist only** — it defines intended scope, never excuses a baseline violation. If the code diverges from the plan, report the divergence; the baseline wins.

## Run in a separate, clean-context agent

**Always dispatch a review to a fresh agent with no shared session context** — use the Agent tool to spawn a dedicated reviewer. A reviewer that "remembers" writing the change rationalizes it; an independent agent re-derives every judgment from the code and the principles. Hand it only the scope (diff or crate/area) and this command's instructions.

## Initialize the submodule first

rskit lives in the `rskit/` submodule and the rskit-reuse pass needs it on disk:

```bash
git submodule update --init --recursive
```

## Pick a driver

- **Change set** → read `.github/skills/review/references/review-changes.md`. A diff (branch, commit range, or `HEAD~1`). Use after every change set, especially fast/"vibe-coded" work.
- **Whole tree / crate** → read `.github/skills/review/references/review-project.md`. A standing audit independent of any diff. Use periodically, before a release, or when onboarding.
- **Review → fix in one pass** → read `.github/skills/review/references/review-details.md`. Fans the review into parallel subagent passes, then plans and applies fixes.

## The eight focused passes (run in order)

Stop and reject as soon as a change fails pass `00` or `01` — misplaced or duplicated code makes every later pass moot. Each reference file can be run standalone when you need only one lens.

1. `.github/skills/review/references/00-structure-placement.md` — layering, port placement, `mod.rs` guard, file homes.
2. `.github/skills/review/references/01-rskit-reuse.md` — did the code reuse rskit, or quietly reimplement a concern rskit already owns? *(blocker class)*
3. `.github/skills/review/references/02-principles.md` — cascade-complete, argv unchanged, libraries-don't-print, CLI output/flag discipline, typed/no-panic, security, performance evidence.
4. `.github/skills/review/references/03-security-privacy.md` — trust-boundary validation, argv-only/no-shell execution, bounded input/output, secret hygiene, path/traversal safety.
5. `.github/skills/review/references/04-quality.md` — simplicity/root-cause, dead code, outdated patterns, style gates.
6. `.github/skills/review/references/05-tests-tdd.md` — TDD, fixtures, failure paths, shared doubles, determinism.
7. `.github/skills/review/references/06-docs-supply-chain.md` — docs policy, docs-match-the-live-schema, Conventional Commits, `Cargo.lock`, `cargo-deny`, SHA-pinned actions.
8. `.github/skills/review/references/07-comments-rustdoc.md` — comments and `///` docs describe the code as it is, not plans/history/process.

## Severity and finding format

```
severity (blocker / should-fix / nit) — file:line — what's wrong — which principle — suggested fix
```

- **blocker** — hard-principle violation (upward dependency, rskit concern reimplemented, library prints, panic on a runtime path, `unsafe`, argv silently rewritten, untrusted input into a path/command/deserialization without validation, behavioral change with no test). Fix before merge.
- **should-fix** — real defect or debt that isn't a baseline violation (stranded test double, inline TOML in a test, compat shim, missing regression test on a non-behavioral tidy-up).
- **nit** — minor/style, take-it-or-leave-it.

## Validation commands

**Scope to the touched crate(s)** — Toven's `make` gates run against the whole workspace, so drive `cargo` directly with `-p <crate>` to stay scoped:

```bash
git submodule update --init --recursive                # if not already
cargo clippy -p <crate> --all-targets --all-features -- -D warnings   # e.g. -p toven-engine
cargo test   -p <crate> --all-features -q
make fmt-check                                          # fast, whole-tree formatting check
make structure                                          # mod.rs declare-only + placement guard
```

For a project audit or final sign-off run the full gates: `make lint`, `make test`, `make doc`, `make deny`, `make check`. Treat green `make check` as **necessary but not sufficient**: it does not catch layering-by-convention, cascade gaps, rskit-reuse violations, or weak tests. Those are on the reviewer.
