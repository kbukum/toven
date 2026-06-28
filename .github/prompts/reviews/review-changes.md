# Review changes

Standing, re-runnable review of a **change set** in this repository — a branch, a commit range, or `HEAD~1`. Use it after every change set, especially fast/"vibe-coded" work. It sequences the seven focused passes in [`reviews/`](./) over a diff and adds scope handling; the actual checks live in the focused files.

## Run this in a separate, clean-context agent

**Always dispatch this review to a fresh reviewer agent with no shared session context.** A reviewer that "remembers" writing the code rationalizes it; an independent agent re-derives every judgment from the diff and the principles. Do not run it inline in the same session/context that produced the change.

- Hand the reviewer agent: the diff (or base ref), this file, and the [`reviews/`](./) folder. Nothing else from the authoring session.
- The reviewer reads the code as-is; it does not trust prior reasoning about why the code "should" be correct.
- **Optional plan check.** If a plan/spec exists (e.g. the session `plan.md`, an issue, or a design doc), pass it in *as a scope checklist only* — "here is what this change set claimed to do; verify the diff actually did it, cascade-complete, with tests." The plan defines intended scope; it never excuses a principle violation. If the diff diverges from the plan, report the divergence; do not assume the plan is authoritative over the baseline in [`docs/engineering.md`](../../../docs/engineering.md).

## Pass 0 — Scope and context

- Get the actual diff: `git diff <base>...HEAD --stat`, then per file. Review only what changed plus its blast radius; do not audit the whole repo (that is [`review-project.md`](./review-project.md)).
- For every changed model/schema type, list every layer that *should* have changed with it (the cascade — see pass `02`). Hold that list; incomplete cascades are the most common vibe-coding defect.
- Note which crates are touched and confirm the change belongs in those crates at all.
- Initialize the submodule if needed: `git submodule update --init --recursive` (pass `01` reads `rskit/`).

## Passes — run in order, stop early on a structural failure

Work the focused files top to bottom. **Stop and reject as soon as a change fails pass `00` or `01`** — misplaced or duplicated code makes every later pass moot.

1. [`00-structure-placement.md`](./00-structure-placement.md) — layering, port placement, `mod.rs` guard, file homes.
2. [`01-rskit-reuse.md`](./01-rskit-reuse.md) — reuse vs. reimplementation of an rskit-owned concern. *(blocker class)*
3. [`02-principles.md`](./02-principles.md) — cascade-complete, argv-is-sacred, libraries-don't-print, typed/no-panic, security, performance evidence.
4. [`03-quality.md`](./03-quality.md) — simplicity/root-cause, dead code, outdated patterns, style gates.
5. [`04-tests-tdd.md`](./04-tests-tdd.md) — TDD, fixtures, failure paths, shared doubles, determinism.
6. [`05-docs-supply-chain.md`](./05-docs-supply-chain.md) — docs policy, Conventional Commits, `Cargo.lock`, `cargo-deny`, SHA-pinned actions.
7. [`06-comments-rustdoc.md`](./06-comments-rustdoc.md) — comments and `///` docs explain the code as it is; rewrite or delete plan/history/process prose.

Each focused file carries a "Changes mode" scope note — follow that mode here. When you only need one lens (e.g. just TDD, just security), run that focused file directly instead of this orchestrator.

## Findings

Record every finding as:

```
severity (blocker / should-fix / nit) — file:line — what's wrong — which principle — suggested fix
```

See [`README.md`](./README.md) for severity definitions.

## Validation

**For a change set, scope validation to the changed crates.** Note Toven's `make` gate targets (`make lint`, `make test`, `make doc`) all run `--workspace` — they are the full gate, not scoped. To scope a per-change review, drive `cargo` directly with `-p <crate>`:

```bash
git submodule update --init --recursive     # once, if rskit/ isn't initialized
cargo clippy -p <crate> --all-targets -- -D warnings   # e.g. -p toven-engine
cargo test   -p <crate> -q                              # only the touched crate(s)
make fmt-check                                          # fast, whole-tree formatting check
make structure                                          # cheap mod.rs / placement guard (run if structure changed)
```

Run the full `make check` (fmt, clippy, tests, docs, deny, structure, release build) — or its whole-workspace `make lint` / `make test` pieces — only when the change is genuinely workspace-wide, or leave it to CI for sign-off. A green `make check` is necessary but **not sufficient** — it will not catch layering-by-convention, cascade gaps, rskit-reuse violations, or weak tests. Those are on the reviewer.
