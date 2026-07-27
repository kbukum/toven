# Pass 06 — Docs and supply chain

The last gate: public surfaces are documented, docs policy is honored, and the supply chain stays pinned and clean.

> **Run in a separate, clean-context agent** — never inline in the session that wrote the code. An independent reviewer re-derives every judgment from the code and the principles instead of trusting prior reasoning. A plan/spec may be passed in as a scope checklist only; it never excuses a baseline violation.

**Scope note.** *Changes mode:* check the docs touched by (or owed by) the diff, and any new CI action it introduces. *Project mode:* sweep `docs/` for `tmp/` references and hard-wrapping, and audit the whole CI/supply-chain surface (`cargo-deny` config, action pins, `Cargo.lock`).

## Docs

- **Public API documented.** Public API changes are reflected in `///` docs and `make doc` (`-D warnings`) passes. Every public item carries docs (`missing_docs = "warn"`).
- **Examples match the live schema.** Config and CLI snippets in `docs/` and `README` must parse/validate against the *current* `Document` loader and CLI surface, not a removed schema. A snippet using a dropped key (e.g. an outdated `[profiles.*]` / `[scopes.*]` section), an outdated task shape, or a wrong default path is a should-fix — and prefer asserting at least one representative doc example round-trips through the loader in a test so the doc cannot silently drift again.
- **Docs live in `docs/`.** Stable project documentation belongs in `docs/`. `tmp/` is for active plans/handoff/research only and must **not** be referenced from committed docs — a committed doc pointing at `tmp/` is a should-fix. Completed phase-history docs should be summarized into stable docs or removed, not accumulated in `tmp/`.
- **Natural prose flow.** Markdown paragraphs stay continuous in source instead of being hard-wrapped to a fixed column; renderers own viewport-aware wrapping. Preserve intentional paragraph, list, blockquote, table, mermaid, and code-block structure.
- **No drifting section refs.** Do not cite numeric section refs (e.g. "principles §4") in code comments or rustdoc — they drift from `docs/engineering.md`. Keep the rationale prose without the number.

## Supply chain

- **Conventional Commits.** `feat` / `fix` / `docs` / `refactor` / `test` / `chore`.
- **`Cargo.lock` committed.** And consistent with the manifests.
- **rskit version pins match.** Each `rskit-*` version pin in the root `Cargo.toml` exactly matches that crate's per-crate version in the `rskit/` submodule (rskit uses independent per-crate versioning). A mismatch breaks the path-dep build.
- **`cargo-deny` clean.** Licenses, advisories, and sources all pass (`make deny`).
- **No unused / misfiled dependencies.** `cargo machete` is clean for the workspace crates (test fixtures and the `rskit/` submodule are out of scope), and a dependency used only by tests lives in `[dev-dependencies]`, not `[dependencies]`.
- **CI actions pinned by SHA.** Any new or changed GitHub Actions step is pinned to a commit SHA, not a floating tag.

## Detection starters

```bash
# committed docs that point at tmp/
rg 'tmp/' docs/
# stale config schema in committed docs/README (keys the strict Document no longer accepts)
rg '\[profiles|\[scopes|profiles\.|scopes\.' docs/ README.md
# drifting numeric section refs in code
rg '§|principles? §|development-principles' crates/*/src
# unused deps / test-only deps in the wrong table
cargo machete crates
# unpinned actions (uses: owner/repo@vX or @branch instead of @<sha>)
rg 'uses:\s+\S+@(?!.{40})' .github/workflows
# rskit pin vs submodule versions
rg '^rskit-' Cargo.toml
```

Then `make doc` and `make deny` for the gates, and confirm `Cargo.lock` is staged.
