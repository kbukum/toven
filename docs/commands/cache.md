# Managing cache

Toven caches successful task results locally and skips a module on the next run when its inputs still match.

## Where records live

The default path is workspace-specific and versioned:

```text
<platform-user-cache>/toven/<workspace-hash>/v3
```

The `v3` segment is the cache-record format version; a new format starts a new directory.

Resolution precedence, highest first:

1. `TOVEN_CACHE_DIR` — an absolute base, for CI or benchmark isolation.
2. `[toven.cache].dir` — a workspace-relative path, to keep records in the repo.
3. The platform user cache directory, namespaced by a hash of the workspace root.

Each option appends the current format version.

```bash
TOVEN_CACHE_DIR=/absolute/path/to/toven-cache toven check
```

```powershell
$Env:TOVEN_CACHE_DIR = "C:\cache\toven"; toven check
```

`TOVEN_CACHE_DIR` must be an absolute path. To keep records in the repository instead:

```toml
[toven.cache]
dir = ".toven/cache"
```

Records then live under `.toven/cache/v3`; do not commit them.

## What invalidates cache

A module re-runs when any of these change since its last success:

- module source files
- dependency results
- task argv and task configuration
- `shared_inputs` declared by the task
- adapter toolchain version probes (Cargo and rustc for Rust)
- passthrough args, when `cache_args = true`

`shared_inputs` are task-owned, workspace-relative paths that invalidate every module using the task — lockfiles, toolchain files, lint config, CI config. Write plain paths inside the workspace (`Cargo.lock`, not `./Cargo.lock`); templates, globs, `.` components, parent paths, and absolute paths are rejected.

Passthrough args disable caching unless the task opts in with `cache_args = true`, because arbitrary flags can change command semantics.

Persistent tasks are never cached: readiness and process lifetime are runtime behavior, not reusable results.

To force a re-run for one invocation, use [`--refresh` or `--no-cache`](run.md#cache-control---refresh-vs---no-cache).

## `toven cache path`

Prints the resolved cache directory — the default user cache, `TOVEN_CACHE_DIR`, or `[toven.cache].dir`:

```bash
toven cache path
toven cache path --config path/to/toven.toml
```

## `toven cache stats`

Reports the cache directory, entry count, and total bytes on disk. A `truncated` flag marks a scan that hit the size cap:

```bash
toven cache stats
```

## `toven cache clean`

Removes the workspace's cache directory. A missing directory counts as already clean:

```bash
toven cache clean
```
