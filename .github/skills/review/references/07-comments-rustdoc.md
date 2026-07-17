# Pass 07 — Comments and rustdoc

Every comment and `///` doc earns its place by explaining the code as it exists now. This pass sweeps all prose in the source and rewrites or deletes anything that documents history, plans, or the author's process instead of the code.

> **Run in a separate, clean-context agent** — never inline in the session that wrote the code. An independent reader judges each comment against the code in front of it, with no memory of why it was written. A plan/spec may be passed in as a scope checklist only; it never excuses a baseline violation.

**Scope note.** *Changes mode:* review every comment and rustdoc touched by (or owed by) the diff, including comments left outdated by a code change. *Project mode:* sweep all prose across `crates/*/src`, `crates/*/tests`, and any other first-party source — module headers (`//!`), item docs (`///`), and inline (`//`) comments alike. This pass is about prose, not code; never weaken a check from another pass to make a comment "true".

## The principle

A comment describes **what the code does and why**, for a reader who has the code but not its history. It is read far more often than it is written, and a wrong or outdated comment is worse than none. Treat every comment as code that must stay correct: if the surrounding code changes, the comment changes with it or it goes.

## What good looks like

- **Explains intent and rationale, not mechanics.** Prefer *why* over *what*; the code already says what. A good comment captures a non-obvious invariant, a subtle edge case, a deliberate trade-off, a safety/security boundary, or a reason the obvious alternative was not taken.
- **Self-contained and durable.** Understandable from the code alone, and still true a year from now regardless of which branch, PR, or plan shipped it.
- **rustdoc is for the reader of the API.** Module (`//!`) and item (`///`) docs describe the contract: what it is, what it guarantees, how to use it, when it errors or panics. Public items document `# Errors` (and `# Panics` where any panic is reachable) per the crate's conventions. Doc examples compile and reflect real usage.
- **Domain vocabulary, not project trivia.** Architecture and domain terms that appear in the code or docs (e.g. phase names, strategy names, port/seam names that are real types or concepts) are fine. They describe the system, not the project's history.

## What to remove or rewrite

Flag and fix every comment that documents the *process* rather than the *code*:

- **Plan / roadmap / process artifacts.** References to plan ordinals or design-doc bookkeeping: "Decision N", "step N", "phase N"/"phases N–M" as plan numbering, "slice N", "the Nth seam", "review pass NN", "self-audit gap #N", "as landed in PR #…", design-doc filenames (`*-design.md`, `something-port.md`). Keep the underlying rationale as plain prose; drop the numbering and the back-reference. (Real domain phase/strategy/port *names* stay — it is the plan-ordinal bookkeeping that goes.)
- **Drifting cross-references.** Numeric section refs ("principles §4", "see §3.2"), line numbers, or "see the doc above/below" that silently rot. Restate the actual rule or point at a stable, named anchor instead.
- **Temporal / narrative phrasing.** "now", "new", "newly added", "recently changed", "as of this PR", "previously we…", "used to…", "for now", "temporary", "TODO from the old design". Either the statement is a durable fact (rephrase it as one) or it is process noise (delete it). A genuine, actionable `TODO`/`FIXME` stays only if it names what and why — ideally with a tracked issue link — otherwise remove it.
- **Restating the code.** Comments that paraphrase the next line (`// increment i`), obvious getters/setters, or type signatures already visible. Delete them; they only add drift surface.
- **Commented-out code and dead prose.** Old implementations left in comments, scaffolding notes, "left here in case", debug breadcrumbs. Delete — version control is the history.
- **Outdated or contradicted comments.** Any comment the current code no longer matches. Correct it to the code, or remove it. A comment that disagrees with the code is a blocker — the reader can no longer trust either.
- **Apology / chatter / attribution.** "hacky", "not sure why this works", "sorry", banner art, author names, dates, changelog lines inside source. Replace genuine uncertainty with a precise statement of the invariant, or remove.

## How to apply

This is a refactor pass, not just a report: **fix what you find** in the same change.

1. Sweep the in-scope prose (use the starters below to surface the usual offenders, then read for the subtler cases — staleness and code-restating do not grep).
2. For each finding, decide: **rewrite** (the rationale is worth keeping — restate it as a durable, code-grounded statement) or **delete** (it is process noise, dead, or redundant).
3. Preserve correct domain vocabulary and real rationale. When in doubt about an invariant the comment claims, verify it against the code before rewriting — never launder a wrong comment into a confident-sounding wrong comment.
4. Keep prose style consistent with the crate: complete sentences, present tense, describing the code as it is. Markdown in docs stays one line per paragraph (no hard-wrapping); preserve code blocks, lists, and tables.
5. Re-run the doc gate so nothing breaks: rustdoc intra-doc links still resolve and `make doc` (`-D warnings`) passes.

## Detection starters

These surface the mechanical offenders; the judgment calls (outdated, redundant, narrative) still need a human/agent read.

```bash
# plan/roadmap bookkeeping in source prose
rg -n 'Decision [0-9A-Z]|[^a-z]step [0-9]|phases? [0-9]|review pass|slice [0-9]|gap #?[0-9]|self-audit' crates/*/src crates/*/tests
# design-doc back-references and drifting section/line refs
rg -n '\b[a-z-]+\.md\b|§|principles? §|development-principles|see (the )?(line|section|above|below)' crates/*/src
# temporal / narrative phrasing
rg -n '\b(now|newly|recently|previously|used to|as of|for now|temporary|TODO|FIXME|HACK|XXX)\b' crates/*/src
# commented-out code (lines that are themselves code under a //)
rg -n '^\s*// *(let |fn |if |for |match |use |pub |struct |impl |return )' crates/*/src
```

Then the gate: `make doc` (`-D warnings`) must pass, and `cargo fmt --check` stays clean. Scope the doc build with `cargo doc -p <crate> --no-deps` in changes mode.
