# Review changes

Standing, re-runnable review of a **change set** in this repository — a branch, a commit range, or `HEAD~1`. Use it after every change set, especially fast/"vibe-coded" work. It sequences the nine focused passes in [`references/`](./) over a diff and adds scope handling; the actual checks live in the focused files.

## Run this in a separate, clean-context agent

**Always dispatch this review to a fresh reviewer agent with no shared session context.** A reviewer that "remembers" writing the code rationalizes it; an independent agent re-derives every judgment from the diff and the principles. Do not run it inline in the same session/context that produced the change.

- Hand the reviewer agent: the diff (or base ref), this file, and the [`references/`](./) folder. Nothing else from the authoring session.
- The reviewer reads the code as-is; it does not trust prior reasoning about why the code "should" be correct.
- **Optional plan check.** If a plan/spec exists (e.g. the session `plan.md`, an issue, or a design doc), pass it in *as a scope checklist only* — "here is what this change set claimed to do; verify the diff actually did it, cascade-complete, with tests." The plan defines intended scope; it never excuses a principle violation. If the diff diverges from the plan, report the divergence; do not assume the plan is authoritative over the baseline in [`docs/engineering.md`](../../../../docs/engineering.md).

## Pass 0 — Scope and context

- Get the actual diff: `git diff <base>...HEAD --stat`, then per file. Review what changed **plus its blast radius** — the rest of each touched file, the code the change calls and is called by, and closely-related files in the same crate. Do not audit the whole repo (that is [`review-project.md`](./review-project.md)), but do not tunnel-vision on the diff lines either.
- **Pre-existing problems in the blast radius are in scope.** A defect, dead code, duplicated concern, or design smell you read while reviewing is reported like any other finding — the change set is not a shield for the code around it. Because Toven (and vendored rskit) is pre-stable with **no backward compatibility owed**, prefer a root-cause **redesign** over patching the symptom (decide Redesign / Align / Enhance / Drop; "leave it patched" is not an option). Flag when a fix reaches beyond the touched files and keep it coherent; never silently refactor unrelated code.
- For every changed model/schema type, list every layer that *should* have changed with it (the cascade — see pass `02`). Hold that list; incomplete cascades are the most common vibe-coding defect.
- Note which crates are touched and confirm the change belongs in those crates at all.
- Initialize the submodule if needed: `git submodule update --init --recursive` (pass `01` reads `rskit/`).

## Passes — run in order, stop early on a structural failure

Work the focused files top to bottom. **Stop and reject as soon as a change fails pass `00` or `01`** — misplaced or duplicated code makes every later pass moot.

1. [`00-structure-placement.md`](./00-structure-placement.md) — layering, port placement, `mod.rs` guard, file homes.
2. [`01-rskit-reuse.md`](./01-rskit-reuse.md) — reuse vs. reimplementation of an rskit-owned concern. *(blocker class)*
3. [`02-principles.md`](./02-principles.md) — cascade-complete, argv unchanged, libraries-don't-print, typed/no-panic, security, performance evidence.
4. [`03-security-privacy.md`](./03-security-privacy.md) — trust-boundary validation, argv-only/no-shell execution, bounded input/output, secret hygiene, path/traversal safety. *(blocker class)*
5. [`04-quality.md`](./04-quality.md) — simplicity/root-cause, dead code, outdated patterns, style gates.
6. [`05-tests-tdd.md`](./05-tests-tdd.md) — TDD, fixtures, failure paths, shared doubles, determinism.
7. [`06-docs-supply-chain.md`](./06-docs-supply-chain.md) — docs policy, Conventional Commits, `Cargo.lock`, `cargo-deny`, SHA-pinned actions.
8. [`07-comments-rustdoc.md`](./07-comments-rustdoc.md) — comments and `///` docs explain the code as it is; rewrite or delete plan/history/process prose.
9. [`08-cli-ux.md`](./08-cli-ux.md) — user-facing surface: actionable errors, parser-scoped flags, user vocabulary, documented+pinned exit codes, labeled dry-runs, first-run flow.

Each focused file carries a "Changes mode" scope note — follow that mode here. When you only need one lens (e.g. just TDD, just security), run that focused file directly instead of this orchestrator.

## Findings

Record every finding as:

```
severity (blocker / should-fix / nit) — file:line — what's wrong — which principle — suggested fix
```

See [`SKILL.md`](../SKILL.md) for severity definitions.

## Validation

**For a change set, scope validation to the changed crates.** Note Toven's `make` gate targets (`make lint`, `make test`, `make doc`) all run `--workspace` — they are the full gate, not scoped. To scope a per-change review, drive `cargo` directly with `-p <crate>`:

```bash
git submodule update --init --recursive     # once, if rskit/ isn't initialized
cargo clippy -p <crate> --all-targets --all-features -- -D warnings   # e.g. -p toven-engine
cargo test   -p <crate> --all-features -q                              # only the touched crate(s)
make fmt-check                                          # fast, whole-tree formatting check
make structure                                          # cheap mod.rs / placement guard (run if structure changed)
```

Run the full `make check` (fmt, clippy, tests, docs, deny, structure, release build) — or its whole-workspace `make lint` / `make test` pieces — only when the change is genuinely workspace-wide, or leave it to CI for sign-off. A green `make check` is necessary but **not sufficient** — it will not catch layering-by-convention, cascade gaps, rskit-reuse violations, or weak tests. Those are on the reviewer.
