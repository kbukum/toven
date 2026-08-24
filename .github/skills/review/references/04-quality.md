# Pass 04 — Quality: simplicity, maintainability, flexibility, up-to-date code

Catch debt and drift that compiles cleanly but should not land. None of this is style-bikeshedding — it maps to Toven's pre-stable, redesign-first stance.

> **Run in a separate, clean-context agent** — never inline in the session that wrote the code. An independent reviewer re-derives every judgment from the code and the principles instead of trusting prior reasoning. A plan/spec may be passed in as a scope checklist only; it never excuses a baseline violation.

**Scope note.** *Changes mode:* judge the diff against simpler alternatives and check the style gates on touched public items. *Project mode:* hunt for dead code, lingering compatibility shims, and outdated patterns across the crate(s) — accumulated leftover code is exactly what a full audit should find.

## Checks

- **Simplicity / root-cause.** Toven and rskit are pre-stable; backward compatibility is *not* a goal. A compatibility shim, an adapter-over-old-behavior, or a "leave the old path too" hedge is wrong here — the correct move is a clean redesign. Flag shims as should-fix with a redesign suggestion.
- **Dead / useless code.** New (or existing, in project mode) code with no caller, speculative generality (a trait/param with one impl and no near-term second), commented-out blocks, leftover scaffolding. Remove.
- **Outdated patterns.** Edition 2024 / Rust 1.97 is the floor — flag patterns superseded by current idioms: manual impls where `derive` suffices, pre-2024 borrow gymnastics, needless clones that clippy-pedantic would catch.
- **Maintainability.** Is the change obvious to the next reader without the original author? Do names match Toven vocabulary? Is there hidden coupling across layers? Prefer focused, well-named files over piling functionality into one large file.
- **Style gates.**
  - `#[must_use]` on `with_*` builder methods.
  - `#[non_exhaustive]` on public enums that may grow.
  - All public items carry `///` docs (`missing_docs = "warn"`).
  - `unsafe_code = "forbid"` — any `unsafe` is a blocker.
  - `cargo fmt` (max_width 100) and clippy `all` / `pedantic` / `nursery` clean.
- **No test-only escape hatches on production surfaces.** A recover-the-inner accessor (`into_inner`, `into_sink`, …) used only by tests must be `#[cfg(test)]`-gated or removed; shared doubles expose recording accessors (cloneable shared state) instead. A public `into_inner` reachable in release builds is a should-fix.

## Detection starters

```bash
rg 'unsafe ' crates/*/src
rg 'pub fn (into_inner|into_sink)' crates/*/src        # test-only escape hatches on public surfaces?
rg 'TODO|FIXME|XXX|HACK|deprecated|legacy|back.?compat|for now' crates/*/src
rg 'pub fn with_' crates/*/src                          # confirm each carries #[must_use]
rg '^\s*//\s*(let|fn|if|match|self\.)' crates/*/src      # commented-out code
```

Then let clippy do the mechanical pass: `make lint` (clippy `-D warnings`, with `pedantic`/`nursery` warn).
