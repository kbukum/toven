# `ecosystems/go/` — Go adapter fixtures

Mirrors the `rust/` layout. Adding the Go ecosystem never edits another
ecosystem's fixtures.

- `adapter/` — `[ecosystems.go]` config fragments parsed by `GoProvider::configure`.
- `workspaces/` — sample Go projects discovery runs against (real `go mod edit`):
  - `single-module/` — one root `go.mod`, no `go.work`.
  - `work/` — a `go.work` grouping two member modules (`app` → `core` edge).
  - `broken/` — a malformed `go.mod` that makes `go mod edit` fail.
