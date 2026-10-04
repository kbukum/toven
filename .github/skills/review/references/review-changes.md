# Review changes

Standing, re-runnable review of a **change set** in this repository — a branch, a commit range, or `HEAD~1`. Use it after every change set, especially fast/"vibe-coded" work. It sequences the nine focused passes in [`references/`](./) over a diff and adds scope handling; the actual checks live in the focused files.

## Execution

Follow [the review skill](../SKILL.md): direct review by default; independent agents only on request. Read current source and relevant contracts. A plan is a scope checklist, not a justification for a baseline violation.

## Pass 0 — Scope and context

- Get the actual diff: `git diff <base>...HEAD --stat`, then per file. Review what changed **plus its blast radius** — the rest of each touched file, the code the change calls and is called by, and closely-related files in the same crate. Do not audit the whole repo (that is [`review-project.md`](./review-project.md)), but do not tunnel-vision on the diff lines either.
- **Pre-existing problems in the blast radius are in scope.** A defect, dead code, duplicated concern, or design smell you read while reviewing is reported like any other finding — the change set is not a shield for the code around it. Because Toven (and vendored rskit) is pre-stable with **no backward compatibility owed**, prefer a root-cause **redesign** over patching the symptom (decide Redesign / Align / Enhance / Drop; "leave it patched" is not an option). Flag when a fix reaches beyond the touched files and keep it coherent; never silently refactor unrelated code.
- For every changed model/schema type, list every layer that *should* have changed with it (the cascade — see pass `02`). Hold that list; incomplete cascades are the most common vibe-coding defect.
- Note which crates are touched and confirm the change belongs in those crates at all.
- Initialize the submodule if needed: `git submodule update --init --recursive` (pass `01` reads `rskit/`).

## Passes

Follow the trigger table and order in [the review skill](../SKILL.md). Use each checklist's changes scope. Load applicable files only; report incomplete checks and stop acceptance on structural/reuse blockers.

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
