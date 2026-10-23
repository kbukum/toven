# Review project

Audit a crate or the whole workspace, independent of any diff. Follow the [review skill](../SKILL.md) for lenses, severity, and finding format.

## Scope

State the surface up front: the workspace or named crates. Then get the shape:

```bash
git submodule update --init --recursive
ls crates apps
for c in crates/*/Cargo.toml apps/*/Cargo.toml; do echo "== $c"; rg '^(toven|rskit)-' "$c"; done
```

Compare the dependency blocks with the [architecture layer map](../../../../docs/architecture.md#workspace-layers).

## Priority order

Run every lens, in this order, so the costliest problems surface first:

1. Placement and layering, then rskit reuse.
2. Security, then runtime correctness.
3. Public API and stability.
4. Tests and proof.
5. CLI experience.
6. Code quality, then comments and rustdoc.
7. Docs and supply chain.
8. Gates: compare what the [gates section](../../../../docs/engineering.md#gates) claims with the `Makefile` and `.github/workflows/`, and check the [known gaps](../../../../docs/engineering.md#known-gaps) are still accurate.

## Validation

```bash
make doctor
make check
make coverage
make docs-build
```

Group findings by crate, then by lens.
