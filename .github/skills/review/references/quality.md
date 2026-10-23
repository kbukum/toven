# Lens: code quality

Catch debt that compiles cleanly but should not land. Rules: [stance](../../../../docs/engineering.md#stance), [code style](../../../../docs/engineering.md#code-style), and [file layout](../../../../docs/engineering.md#file-layout).

## Check

- **Root cause.** A shim, adapter over old behavior, or "keep the old path too" is wrong in a pre-stable project; propose the redesign.
- **Dead code.** No caller, speculative generality, commented-out code, or leftover scaffolding.
- **Current idioms.** Edition 2024 and the toolchain in `rust-toolchain.toml`; derive where possible; no needless clones.
- **Readability.** Names match Toven vocabulary; no hidden coupling across layers; focused files.
- **Mechanical style.** `make fmt-check` and `make lint` cover formatting, clippy `pedantic`/`nursery`, `#[must_use]` on `Self`-returning methods, and missing docs. Run them instead of reporting what they already catch.

## Detection starters

```bash
rg -n 'TODO|FIXME|XXX|HACK|legacy|back.?compat|for now' crates/*/src
rg -n '^\s*// *(let|fn|if|match|use|pub|impl|return) ' crates/*/src
make lint
```
