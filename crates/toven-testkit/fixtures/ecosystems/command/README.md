# `ecosystems/command/` — command (escape-hatch) adapter fixtures

Mirrors the `rust/`/`go/` layout. Adding the command ecosystem never edits another ecosystem's fixtures.

The command adapter declares everything (no tooling probe, no filesystem walk), so there are no sample `workspaces/` to discover against — only `[ecosystems.command]` config fragments parsed by `CommandProvider::configure`.

- `adapter/` — config fragments:
  - `declared-modules.toml` — two modules with a `depends_on` edge and user-owned tasks.
  - `with-toolchain.toml` — an explicit `[toolchain]` overriding the first-task default.
  - `unknown-dependency.toml` — a `depends_on` referencing an undeclared module (rejected).
  - `modules-without-toolchain.toml` — modules declared without tasks or `[toolchain]` (rejected).
