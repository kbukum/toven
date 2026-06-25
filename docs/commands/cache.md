# Managing cache

Toven stores local successful-result cache records outside the repository by default, under the platform user cache directory. Cache records are local execution evidence; they are not a remote or distributed cache.

> Target behavior; returns as the redesign steps land (the CLI is being rebuilt on the `crates/*` + `apps/*` stack).

The default path is workspace-specific and versioned:

```text
<platform-user-cache>/toven/workspaces/<workspace-hash>/v3
```

The `v3` segment is the task-cache record/key format version. Toven starts a new directory when that format changes instead of trying to read incompatible records.

Use `TOVEN_CACHE_DIR` to override the base cache directory for CI or benchmark isolation:

```bash
TOVEN_CACHE_DIR=/tmp/toven-cache toven check
```

The override must be an absolute path. Toven still appends the current cache format version, such as `v3`.

To opt into repository-local cache records:

```toml
[toven.cache]
dir = ".toven/cache"
```

Workspace-local cache records live under `.toven/cache/v3` and should not be committed.

## Cache modes during execution

| Mode | Command | Behavior |
|------|---------|----------|
| Default | `toven check` | Read existing records and write fresh success records. |
| Force | `toven check --force` | Skip cache reads, run work, and write fresh success records. |
| Disabled | `toven check --no-cache` | Disable cache reads and writes for that invocation. |

## `toven cache stats`, `toven cache info`

Shows local cache size and age information:

```bash
toven cache stats
toven cache info
toven cache stats --config path/to/toven.toml
```

The command loads the workspace root from config and inspects the Toven cache directory. It reports cache directory, entry count, total bytes, oldest/newest entry age, and notes that hit rate is per-run only.

## `toven cache path`

Shows the resolved local cache directory:

```bash
toven cache path
toven cache path --config path/to/toven.toml
```

Use this when you need to verify whether Toven is using the default user cache, `TOVEN_CACHE_DIR`, or `toven.cache.dir`.

## `toven cache clean`, `toven cache clear`

Removes local cache records for the workspace:

```bash
toven cache clean
toven cache clear
toven cache clean --config path/to/toven.toml
```

Missing cache directories are treated as already clean. The command reports how many entries and bytes were removed.

## What invalidates cache

Cache decisions include task inputs such as:

- module source files
- dependency results
- task argv and task configuration
- shared inputs declared by the task
- adapter-provided toolchain version probes, such as Cargo and rustc for Rust profiles
- relevant toolchain/config files when configured as shared inputs, such as `rust-toolchain.toml`
- passthrough args when `cache_args = true`

Persistent tasks are never cached because readiness and process lifetime are runtime behavior, not reusable success records.

Run output and JSONL cache events distinguish `hit`, `miss`, `forced`, and `disabled` states. Use [inspection commands](inspect.md) for detailed hit, miss, forced, or disabled reasoning for one module/task pair.

## Review checklist

- Shared inputs include files that should invalidate broad work, such as lockfiles, toolchain files, lint config, and CI-relevant config.
- Passthrough args are cached only when the task explicitly opts in with `cache_args = true`.
- Whole-cache cleanup is sufficient for the current release scope.
