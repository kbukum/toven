# Lens: placement and layering

Misplaced code makes every other finding moot, so check this first. Rules: [placement and layering](../../../../docs/engineering.md#placement-and-layering) and [file layout](../../../../docs/engineering.md#file-layout). The layer map and crate roles live in [architecture](../../../../docs/architecture.md#workspace-layers); read it rather than trusting a copied list.

## Check

- **Downward only.** Compare each touched crate's `Cargo.toml` and `use` lines with its layer. An upward edge is a blocker; Cargo only rejects cycles.
- **Adapters stay thin.** `toven-rust`, `toven-go`, `toven-js`, and `toven-command` depend on `toven-ports` and `toven-model` only.
- **CLI and apps.** `toven-cli` never depends on a concrete adapter; `apps/*` only wire.
- **Ports.** Traits live in `toven-ports` and reference only model, rskit, std, or port-owned types. The production adapter lives in the consuming crate. Each port has one shared double in `toven-testkit/src/doubles/`.
- **Agnostic core.** No cargo, go, mdbook, or ast-grep knowledge in core crates; tool gates go in an adapter or `[ecosystems.command.tasks.*]`.
- **Config.** One strict `Document` loader (`toven-core/src/config/`). A second parse path is a should-fix.
- **File layout.** Focused, concern-named files; split by concern, not by length alone.

## Detection starters

```bash
make structure   # declare-only aggregators, single exec home
for c in crates/*/Cargo.toml apps/*/Cargo.toml; do echo "== $c"; rg '^toven-' "$c"; done
rg -l 'use toven_(core|engine|release|cli)' crates/toven-{model,ports,rust,go,js,command}/src
rg -l 'use toven_(rust|go|js|command)' crates/toven-cli/src
rg -n '"(cargo|go|mdbook|ast-grep)"' crates/toven-{core,engine,runtime}/src
```
