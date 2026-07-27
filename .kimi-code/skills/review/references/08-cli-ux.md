# Pass 08 — CLI user experience

Toven is a command-line tool; its user surface *is* the product. This pass reviews everything a user reads or types — help text, error messages, flags, exit codes, and first-run flow — against the standard that a competent user should never have to guess or read the source.

> **Run in a separate, clean-context agent** — ideally with the built binary in hand (`cargo run -p toven -- …`). Judge the surface as a new user would: run the verbs, hit the errors, read the `--help`. A plan may scope the review; it never excuses a UX defect.

**Scope note.** *Changes mode:* review the user-facing surface touched by the diff — new/changed flags, help strings, error text, exit paths. *Project mode:* exercise every verb's `--help`, the common error paths (no config, unknown task, unknown module), and the new-user path (`init` → first run). This pass is about the user's experience, not internal structure; it complements pass 02's output/flag discipline rather than repeating it.

## The principle

A CLI is a conversation. Every message states **what happened, why, and the next step** in the user's vocabulary. Flags are scoped so the parser — not a hand-rolled runtime matrix — decides what applies where. Machine-facing contracts (exit codes, JSONL) are stable and documented. Internal architecture vocabulary never leaks to the user.

## What good looks like

- **Errors are actionable.** Every error states what failed, why, and the next command when one exists (`no toven.toml found in this directory or any parent — run \`toven init\` to create one`). One clean sentence, not a struct field stuffed into a name slot. Where a close match exists, offer a did-you-mean.
- **Flags are scoped at the parser.** A verb's `--help` shows only the flags that apply to it. Inapplicable flags are rejected by clap's own grammar, not by a runtime rejection matrix — help, shell completions, and error UX all follow from correct scoping.
- **User vocabulary, not internals.** Help and messages speak in preview/run/task terms. Architecture words (PLAN, APPLY, event-sink, "cut") stay in `docs/architecture.md`, never in `--help`.
- **Machine contracts are stable and documented.** The exit-code taxonomy is documented (`docs/commands/README.md`) and pinned by a test. JSONL is one object per line on stdout; human framing stays on stderr. Dry-runs are explicitly labeled so a glance at the summary distinguishes them from a real run.
- **The new-user path is first-class.** No config → `init` → first run works, is documented, and is tested. `init` reports what it detected even in `--print` mode, and never writes config that later commands will choke on (e.g. a `base_ref` for a remote that does not exist).
- **Human output reads naturally.** Counts are pluralized (`1 unit in 1 wave`), status reads as a word (`status: ok`/`failed`) not a bare exit number, and run-id noise is demoted to `-v`/JSONL.

## What to flag and fix

- **Garbled or unhelpful errors.** Double-wrapped messages, a sentence in an identifier slot, no recovery hint, or an undocumented failure mode.
- **Global flags that should be per-verb.** Every verb's help dumping every flag group; a runtime "flag X does not apply to verb Y" matrix standing in for parser-level scoping.
- **Internal vocabulary in user output.** PLAN/APPLY/event-sink/"cut"/"task-APPLY" in any `--help` string or user message.
- **Undocumented or unpinned machine contracts.** Exit codes not documented or not pinned by a test; stdout/stderr channel leaks; a dry-run summary indistinguishable from a real run.
- **New-user traps.** `init` writing unusable defaults, emitting no detection feedback, or generating optional-tool tasks without probing (or annotating) them; an unbounded "unknown module" list on a large repo.
- **Ungrammatical or noisy output.** `1 units`, raw run-ids in interactive output, box-drawing tables emitted to a non-tty pipe where plain columns would be grep-friendly.

## How to apply

This is a fix pass, not just a report: **fix what you find** in the same change.

1. Run each in-scope verb's `--help` and the common error paths against the built binary.
2. For each defect, fix at the right layer — restructure flags at the parser rather than patching the rejection matrix; rewrite the message in user language with a recovery step; add a did-you-mean where a close match exists.
3. Keep the machine contracts stable and covered: document exit codes and pin them with a test; label dry-runs; keep stdout/stderr channels clean.
4. Re-run the CLI tests and the byte-level stream assertions so the fix holds.

## Detection starters

```bash
# internal vocabulary leaking into user-facing strings
rg -n 'PLAN|APPLY|event-sink|event sink|\bcut\b' crates/toven-cli/src
# hand-rolled flag-applicability rejection (should be parser-scoped)
rg -n 'does not apply|not applicable|unconsumed flag|reject' crates/toven-cli/src/flags.rs
# un-pluralized counts and bare exit labels in human output
rg -n '\{[a-z_]+\} (units|waves)|"exit"' crates/toven-cli/src/report
```

Then the gate: `cargo test -p toven-cli --all-features -q` and, for a real check, drive the built binary through the flagged paths.
