# `ecosystems/js/` — JavaScript adapter fixtures

Mirrors the `go/` layout. Discovery is offline, so lockfiles here are stand-ins: discovery checks only that they exist.

- `adapter/` — `[ecosystems.js]` config fragments parsed by `JsProvider::configure`.
- `workspaces/` — sample projects discovery runs against:
  - `npm/` — an npm workspace with a named root package whose `test` orchestrator is not a root script; plain-range edges (normal and dev) and a range the local version does not satisfy.
  - `pnpm/` — a pnpm workspace with `catalog:`, a `workspace:` alias, a `**` pattern, and a negated pattern; the unnamed root package is not a module.
  - `multi-root/` — independent roots per tool (Yarn Berry, Yarn classic, Bun) with no repository-root `package.json`; a cross-root `link:` edge, a packed-tarball external root, a missing path, and a stray `examples/` package.
  - `conflicting-lockfiles/`, `stale-lockfile/` — package-manager conflicts.
  - `malformed/`, `escaping-pattern/`, `duplicate-name/`, `missing-workspace/` — discovery failures.
