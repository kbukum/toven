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

Persistent tasks, tasks with `cacheable = false`, and units whose mutable dependencies are not fully represented in the discovered source graph are never cached. Discovery metadata must advertise complete inputs; missing metadata is not trusted. This restriction propagates to consumers and batched units; `--refresh` cannot override it.

Go discovery checks local replacements in both `go.mod` and a root `go.work`. A replacement keeps caching only when its dependency edge resolves to the same discovered module directory. External or unrepresented local replacements run uncached without walking their source trees. Filesystem errors while resolving a represented replacement are surfaced rather than treated as verified inputs.

Changed-work selection also keeps these modules and their consumers active: repository diffs cannot prove that an unrepresented dependency is unchanged.

Workspace-wide tasks key and check the entire workspace's source closure, even when a selector names only one member. Batched selector-based tasks retain their selected-member scope.

## Force execution

```bash
toven test --workspace rust --refresh
toven test --workspace rust --no-cache
```

These flags live on task-running commands, not on `toven cache` itself. `--refresh` replaces records after success. `--no-cache` leaves cache state untouched. See [running tasks](run.md#cache-control).
