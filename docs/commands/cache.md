# Manage cache

Toven caches successful, cacheable task units and skips them while all inputs still match.

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
TOVEN_CACHE_DIR=/tmp/toven-cache toven test
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
Path                                           Entries  Bytes  Truncated
/Users/example/Library/Caches/toven/.../v3     24       18432  no
```

## Clear workspace cache

```bash
toven cache clean
```

Human confirmation is written to stderr. A missing cache directory is treated as already clean.

## Cache inputs

A unit re-runs when any of these change:

- module source
- dependency results
- task argv or scheduling configuration
- declared `shared_inputs`
- adapter toolchain identity
- passthrough arguments when `cache_args = true`

Persistent tasks and tasks with `cacheable = false` are never cached.

## Force execution

```bash
toven test --refresh
toven test --no-cache
```

`--refresh` replaces records after success. `--no-cache` leaves cache state untouched. See [running tasks](run.md#cache-control).
