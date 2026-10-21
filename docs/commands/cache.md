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
/Users/example/Library/Caches/toven/<workspace-hash>/v4
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
       path:  /Users/example/Library/Caches/toven/<workspace-hash>/v4
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

Go discovery checks local replacements in both `go.mod` and a root `go.work`. A replacement that points at the same package's discovered module is covered by that module's own hash. Any other local replacement, such as a sibling checkout (`replace example.com/kit => ../../kit`), becomes an external root of the module: its tree is hashed into the module's cache key, and the module always counts as changed for `--base` selection because version control cannot see it. A cache hit keeps that rerun cheap. A missing external directory is an error, not a silent miss. A replacement that points at the project root or one of its parents, other than a module discovered there, is a discovery error, since hashing it would fold the whole project into every key.
JavaScript discovery treats `file:`, `link:`, and `portal:` dependencies the same way. A path at a discovered package root is an edge. Any other path outside the declaring package, such as a packed tarball (`file:../target/std.tgz`) or a sibling checkout, becomes an external root; a single file is hashed as one input. A path that does not exist yet (a tarball a sibling task will pack) is reported as a warning, and the module is not cached.

Changed-work selection also keeps these modules and their consumers active: repository diffs cannot prove that an unrepresented dependency is unchanged.

Workspace-wide tasks key and check the entire workspace's source closure, even when a selector names only one member. Batched selector-based tasks retain their selected-member scope.

## Force execution

```bash
toven test --workspace rust --refresh
toven test --workspace rust --no-cache
```

These flags live on task-running commands, not on `toven cache` itself. `--refresh` replaces records after success. `--no-cache` leaves cache state untouched. See [running tasks](run.md#cache-control).
