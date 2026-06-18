# Toven engineering

## Principles

1. Discover before deciding; reuse rskit and existing project helpers where they
   fit.
2. Toven and rskit are pre-stable, so prefer clean redesigns over compatibility
   detours.
3. Cascade-complete model changes through schema, normalization, planner,
   executor, output, tests, and docs.
4. User-owned argv is sacred: Toven validates and expands selectors, but does
   not infer hidden flags or silently rewrite commands.
5. Libraries do not print; CLI/reporting layers own user-facing output.
6. Performance claims require benchmark evidence.

## Local validation

| Command | Purpose |
|---------|---------|
| `make check` | Canonical full gate: fmt, clippy, tests, docs, deny, structure, release build. |
| `make fmt` | Format code. |
| `make lint` | Clippy with denied warnings. |
| `make test` | Workspace cargo tests. |
| `make coverage` | Workspace coverage gate. |
| `make structure` | `mod.rs` declare-only guard across `crates/*`. |

The binary smoke and benchmark harnesses return alongside the CLI apps later in
the workspace redesign.

Prefer validating changed modules/areas unless a broader gate is clearly
necessary.

## Testing standards

- Use reusable fixtures and declarative case files instead of embedding large
  config strings in tests.
- Tests should be deterministic and avoid real network access.
- Runtime paths should surface typed errors instead of panics.

## Documentation policy

- Stable project documentation belongs in `docs/`.
- `tmp/` is only for active plans, handoff notes, or temporary research that is
  still being worked.
- Completed phase-history documents should be removed or summarized into stable
  docs instead of accumulating in `tmp/`.

## Release policy

The remaining release work must prove Toven works as an external tool:

1. Remove publish-blocking local path dependency assumptions.
2. Rehearse installation through the intended release path.
3. Use the installed `toven` binary for rskit adoption and benchmarks.
4. Produce checksums, release metadata, and signed/provenance-ready artifacts.
