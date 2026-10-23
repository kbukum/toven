# Lens: docs and supply chain

Docs that drift from the code and unpinned dependencies both erode trust. Rules: [documentation](../../../../docs/engineering.md#documentation) and [supply chain and release](../../../../docs/engineering.md#supply-chain-and-release).

## Docs

- **Accurate.** Config and CLI examples match the current strict schema and `--help`. Removed flags and keys are gone from docs.
- **Owned once.** Each topic has one owning file (see [where guidance lives](../../../../docs/engineering.md#where-guidance-lives)); other files link to it instead of restating it.
- **Honest gates.** A doc never says a rule is enforced unless a gate checks it.
- **Placement.** Stable docs live in `docs/`; nothing committed links `tmp/`. `docs/SUMMARY.md` lists every book page.
- **Prose.** One source line per paragraph; relative links between pages; no numeric section refs.

## Supply chain

- `Cargo.lock` changes match the manifest changes.
- New dependencies are justified; test-only ones are in `[dev-dependencies]`.
- `make deny` passes.
- `rskit-*` pins match the submodule crate versions.
- Every `uses:` in `.github/workflows/` and `.github/actions/` is pinned to a full commit SHA.
- Release changes keep signing, SBOM, provenance, and the protected `release` environment intact.

## Detection starters

```bash
rg -n 'tmp/' docs README.md CONTRIBUTING.md SECURITY.md
rg -nP -g '*.yml' '^\s*-?\s*uses:\s+[^#\s]+@(?![0-9a-f]{40}\b)' .github/workflows .github/actions
rg -n '^rskit-' Cargo.toml
make deny
make docs-build
```
