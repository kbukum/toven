# Command reference

Toven commands are grouped by workflow rather than listed as one flat page.

> The Toven CLI runs on the hexagonal `crates/*` + `apps/*` stack. The execution (`run`/`task`/`release`), inspection (`plan`/`affected`/`explain`/`modules`/`graph`), and `cache` verbs are wired end to end; `generate` and the `driver`/`federation` verbs are stubbed pending their later redesign steps.

| Topic | Commands |
|-------|----------|
| [Generating config](generate.md) | `toven generate` |
| [Running tasks](run.md) | `toven <task>`, `toven run <task>` |
| [Inspecting work](inspect.md) | `plan`, `affected`, `explain`, `modules`, `list`, `ls`, `graph`, `deps` |
| [Managing cache](cache.md) | `cache path`, `cache stats`, `cache clean` |

## Shared behavior

Most commands load `toven.toml` from the current directory. Use `--config <PATH>` to point at another config file.

Task-oriented commands discover only the profiles and scopes that define or inherit the selected task. Commands that accept `--task <NAME>` default to `test`.

Affected commands use git changes between a baseline and the working tree:

- `--base <REF>` selects a baseline ref or SHA.
- `--merge-base` compares from the merge-base of `HEAD` and the selected baseline.
- `project.base_ref` in `toven.toml` supplies the default baseline when no `--base` is provided.
- Without `--base` or `project.base_ref`, affected detection compares against `HEAD`, so only staged, unstaged, and untracked local changes are considered.

Execution and explanation commands share cache mode flags:

| Flag | Behavior |
|------|----------|
| default | Read cache records and write fresh success records. |
| `--force` | Skip cache reads, run work, and write fresh success records. |
| `--no-cache` | Disable cache reads and writes for that invocation. |

Passthrough args after `--` are expanded into `{args}` in configured task argv. They disable cache by default unless the task sets `cache_args = true`.

Toven stores task cache records in the platform user cache directory by default, under a workspace-specific hash and cache-format version. Set `TOVEN_CACHE_DIR` to an absolute path for CI or benchmark isolation, or configure `[toven.cache].dir` with a workspace-relative path (such as `.toven/cache`) to keep records inside the repository.
