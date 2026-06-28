# Command reference

Toven commands are grouped by workflow rather than listed as one flat page.

| Topic | Commands |
|-------|----------|
| [Generating config](generate.md) | `toven generate` |
| [Running tasks](run.md) | `toven <task>`, `toven run <task>` |
| [Inspecting work](inspect.md) | `plan`, `affected`, `explain`, `modules`, `list`, `ls`, `graph`, `deps` |
| [Managing cache](cache.md) | `cache path`, `cache stats`, `cache clean` |

## Shared behavior

Most commands load `toven.toml` from the current directory. Use `--config <PATH>` to point at another config file.

Task-oriented commands take the task as a positional argument, such as `toven plan check` or `toven affected test`. `toven modules` and `toven graph` inspect the discovered workspace without a task argument.

Affected commands use git changes between a baseline and the working tree:

- `--base <REF>` selects a baseline ref or SHA.
- `--merge-base` compares from the merge-base of `HEAD` and the selected baseline.
- `project.base_ref` in `toven.toml` supplies the default baseline when no `--base` is provided.
- Without `--base` or `[project].base_ref`, affected detection has no baseline and fails with a `no baseline reference` error.

Cache behavior is controlled by config (`[toven.cache]` settings, per-task `cache_args`, `shared_inputs`, and `persistent`) plus `TOVEN_CACHE_DIR`; there are no cache-bypassing per-invocation flags.

Passthrough args after `--` are expanded into `{args}` in configured task argv. They disable cache by default unless the task sets `cache_args = true`.

Toven stores task cache records in the platform user cache directory by default, under a workspace-specific hash and cache-format version. Set `TOVEN_CACHE_DIR` to an absolute path for CI or benchmark isolation, or configure `toven.cache.dir` with a workspace-relative path (such as `.toven/cache`) to keep records inside the repository.
