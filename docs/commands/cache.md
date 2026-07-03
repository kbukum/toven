# Managing cache

Toven stores local successful-result cache records outside the repository by default, under the platform user cache directory. Cache records are local execution evidence; they are not a remote or distributed cache.

The default path is workspace-specific and versioned:

```text
<platform-user-cache>/toven/<workspace-hash>/v3
```

The `v3` segment is the task-cache record/key format version. Toven starts a new directory when that format changes instead of trying to read incompatible records.

Resolution follows a fixed precedence (highest first): the `TOVEN_CACHE_DIR` environment override (an absolute base), then a workspace-relative `[toven.cache].dir`, then the platform user-cache directory namespaced by a stable hash of the workspace root. Each appends the current cache format version.

Use `TOVEN_CACHE_DIR` to override the base cache directory for CI or benchmark isolation:

```bash
TOVEN_CACHE_DIR=/absolute/path/to/toven-cache toven check
```

```powershell
$Env:TOVEN_CACHE_DIR = "C:\cache\toven"; toven check
```

The override must be an absolute path. Toven still appends the current cache format version, such as `v3`.

To opt into repository-local cache records:

```toml
[toven.cache]
dir = ".toven/cache"
```

Workspace-local cache records live under `.toven/cache/v3` and should not be committed.

## Cache modes during execution

Run output and JSONL cache events distinguish active cache decisions from disabled ones. Cache is configured by `[toven.cache]` settings, per-task `cache_args`, `shared_inputs`, `persistent`, `TOVEN_CACHE_DIR`, and `[toven.cache].dir`. Two per-invocation execution flags override the cache for a single run: `--no-cache` bypasses it entirely (no record is read or written), while `--refresh` ignores existing records and re-runs every unit but still writes the fresh results back — use `--refresh` to rebuild a distrusted entry and `--no-cache` for a one-off run that must not touch the cache. The two are mutually exclusive.

## `toven cache stats`

Shows the resolved local cache directory and its size:

```bash
toven cache stats
toven cache stats --config path/to/toven.toml
```

The command loads the workspace root from config and inspects the Toven cache directory. It reports the cache directory path, the entry count, the total bytes on disk, and a `truncated` flag that is set when a very large cache exceeds the scan cap.

## `toven cache path`

Shows the resolved local cache directory:

```bash
toven cache path
toven cache path --config path/to/toven.toml
```

Use this when you need to verify whether Toven is using the default user cache, `TOVEN_CACHE_DIR`, or `toven.cache.dir`.

## `toven cache clean`

Removes local cache records for the workspace:

```bash
toven cache clean
toven cache clean --config path/to/toven.toml
```

Missing cache directories are treated as already clean. The command reports whether it removed the cache directory or found it already absent.

## What invalidates cache

Cache decisions include task inputs such as:

- module source files
- dependency results
- task argv and task configuration
- shared inputs declared by the task
- adapter-provided toolchain version probes, such as Cargo and rustc for Rust ecosystems
- relevant toolchain/config files when configured as shared inputs, such as `rust-toolchain.toml`
- passthrough args when `cache_args = true`

Persistent tasks are never cached because readiness and process lifetime are runtime behavior, not reusable success records.

Run output and JSONL cache events distinguish `hit`, `miss`, and `disabled` states per unit. Run a task with `-v` (or read the JSONL `cache-decided` events) to see the per-unit verdict for one module/task pair.

## Review checklist

- Shared inputs include files that should invalidate broad work, such as lockfiles, toolchain files, lint config, and CI-relevant config.
- Passthrough args are cached only when the task explicitly opts in with `cache_args = true`.
- Whole-cache cleanup is sufficient for the current release scope.
