# Reviewing and updating Toven's docs

Documentation drifts two ways: it drifts out of **standards** (hard-wrapped prose, `tmp/` references, history narration) and it drifts out of **up-to-date accuracy** (commands, flags, structure, and examples that no longer match the code). This command sweeps both. It is the standing owner of Toven's doc quality — run it over the whole `docs/` tree, a single file, or the docs touched by a change set.

The authoritative doc policy lives in the Documentation section of [`docs/engineering.md`](docs/engineering.md). The baseline wins over any local habit.

## The doc surface

Sweep every committed prose surface, not just `docs/`:

- `docs/**` — stable project docs (`architecture.md`, `engineering.md`, `getting-started.md`, `installation.md`, `product.md`, `scenarios.md`, `concern-owners.md`, `benchmarking.md`), plus `docs/commands/*.md` and `docs/config/*.md`.
- `README.md`, `CHANGELOG.md`, `MAINTAINERS.md`, and any top-level `*.md`.
- `.github/skills/**/SKILL.md` and their `references/*.md`.
- `///` rustdoc and `//` comments in the crates in scope (these are docs too).

Never touch `tmp/` (gitignored scratch) and never add a committed doc that references it.

## Pass 1 — Standards (how it reads)

- **Flowing Markdown prose.** A Markdown paragraph is one continuous source line. Do not hard-wrap prose to a column limit or add source newlines to control how it looks at one editor width; GitHub and other renderers wrap it for the reader's viewport. Collapse AI-generated hard wraps only within the same logical paragraph.
- **Preserve intentional structure.** Keep blank-line paragraph boundaries, headings, list items, blockquotes, tables, link definitions, HTML blocks, mermaid diagrams, and fenced or indented code blocks. Never join separate list items or paragraphs. Preserve hard line breaks that are semantically meaningful (`<br>` or two trailing spaces).
- **No history/plan/process narration.** A doc or comment describes the system as it is now, not how it got here, what it used to do, or what a future plan intends. Delete "previously…", "we changed…", batch/plan/PR references, and TODO-narration.
- **`tmp/` stays uncommitted.** No committed doc references a `tmp/` plan or handoff note.
- **Frontmatter exemption.** YAML folded scalars (e.g. a skill's `description: >-`) already collapse to one logical line — leave their wrapping alone.

## Pass 2 — Up-to-date (whether it's still true)

Verify each doc against the code it describes; a doc that lies is worse than no doc:

- **Commands & gates** match the `Makefile` (`make check`, `make structure`, `make test`, `make doc`, `make deny`, `make release-*`) — no renamed or removed target lingers in the docs.
- **CLI surface** matches the real binary: verbs, flags, defaults, and output-stream behavior (stdout = machine-readable JSONL; human/status = stderr) match `toven --help` and `crates/toven-cli`. Removed flags are removed from docs; documented flags actually exist and change behavior.
- **Workspace structure** matches reality: the crate/layer list (`crates/toven-{model,ports,engine,rust,go,command,cli,testkit}`, `apps/*`) and the hexagonal layering description match the tree; renamed/added/dropped crates are reflected.
- **Config & schema** examples are valid `toven.toml` for the current strict loader — keys round-trip and no outdated field survives.
- **Examples run.** Code/command examples reflect current behavior; doctests compile under `make doc`.
- **Links resolve.** Internal relative links and cross-references point at files that exist; other-repo references use full URLs, never bare `#123`.

## Apply, then validate

Fix every instance of a pattern across the whole surface in scope, not just the first hit. When repairing hard wraps, read and judge the Markdown structure rather than applying a blind line-joining script. Then validate what you touched:

```bash
make doc                                            # rustdoc builds with -D warnings (validates /// docs + doctests)
```

Docs/prose-only changes need no build/test gate beyond `make doc` when rustdoc changed. Verify internal links by path before finishing.

## Commit

Use the `/commit` command — one compact `docs:` Conventional-Commit line stating the change (e.g. `docs: repair hard-wrapped prose and sync command docs`). No `Co-authored-by` trailer, no plan/batch/tool narration. Group by intent when it aids the reader (a prose-flow repair and an up-to-date accuracy update read as separate commits).
