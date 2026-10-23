# Lens: comments and rustdoc

A wrong comment is worse than none. Rules: [code style](../../../../docs/engineering.md#code-style) and [documentation](../../../../docs/engineering.md#documentation).

## Check

- **Explain why.** Keep comments that state a non-obvious invariant, edge case, trade-off, or safety boundary. Delete comments that restate the next line.
- **API docs.** `//!` and `///` describe the contract: what it is, what it guarantees, and how to use it. Document errors and panics when they are not obvious from the type. The `missing_errors_doc` and `missing_panics_doc` lints are allowed, so this is review judgment, not a gate. Doc examples compile.
- **No process history.** Remove plan numbers ("step 3", "phase 2", "Decision N"), PR references, design-doc file names, numeric section refs, and "now"/"previously"/"for now" narration. Keep the rationale as plain prose; real domain names stay.
- **Stale comments.** A comment that disagrees with the code is a blocker; fix it to match the code.
- **No chatter.** No commented-out code, apologies, author names, or dates.
- **Prose flow.** Do not hard-wrap prose in Markdown or rustdoc.

## Detection starters

```bash
rg -n 'Decision [0-9A-Z]|\bstep [0-9]|\bphases? [0-9]|review pass|slice [0-9]|gap #?[0-9]' crates/*/src crates/*/tests
rg -n '§|\b[a-z-]+-design\.md\b' crates/*/src
rg -n '\b(newly|recently|previously|used to|as of|for now)\b' crates/*/src
make doc
```
