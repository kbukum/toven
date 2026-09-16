# Cache

Inspect the local task cache, see where it lives, and clear it when you need a clean slate.

## Quickstart

```bash
toven cache path
toven cache stats
toven cache clean
```

## Subcommands

| Command | What it does |
|---|---|
| `toven cache path` | Print the resolved local cache directory |
| `toven cache stats` | Summarize the local cache directory |
| `toven cache clean` | Remove the local cache directory |

Toven caches successful, cacheable task units. It skips them while their inputs still match.

## Resolve the cache path

```bash
toven cache path
```

Example stdout:

```text
/Users/example/Library/Caches/toven/<workspace-hash>/v3
```

Path precedence:

1. `TOVEN_CACHE_DIR`, which must be absolute
2. `[toven.cache].dir`, relative to the workspace
3. Platform user cache, namespaced by workspace identity

```bash
TOVEN_CACHE_DIR="$PWD/.toven/cache" toven test --workspace rust
```

```toml
[toven.cache]
dir = ".toven/cache"
```

Do not commit repository-local cache records.

## Inspect cache usage

```bash
toven cache stats
```

Example stdout:

```text
       path:  /Users/example/Library/Caches/toven/<workspace-hash>/v3
    entries:  24
      bytes:  18432
  truncated:  false
```

## Clear workspace cache

```bash
toven cache clean
```

Human confirmation is written to stderr. A missing cache directory counts as already clean.

## Cache inputs

A unit re-runs when any of these inputs change:

- module source
- dependency results
- task argv or scheduling configuration
- declared `shared_inputs`
- adapter toolchain identity
- passthrough arguments when `cache_args = true`

Persistent tasks and tasks with `cacheable = false` are never cached.

## Force execution

```bash
toven test --workspace rust --refresh
toven test --workspace rust --no-cache
```

These flags live on task-running commands, not on `toven cache` itself. `--refresh` replaces records after success. `--no-cache` leaves cache state untouched. See [running tasks](run.md#cache-control).
