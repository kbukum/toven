# Command reference

| Topic | Commands |
|-------|----------|
| [Generating config](generate.md) | `toven generate` |
| [Running tasks](run.md) | `toven <task>`, `toven run <task>` |
| [Inspecting work](inspect.md) | `plan`, `affected`, `explain`, `modules` (`list`, `ls`), `graph` (`deps`) |
| [Managing cache](cache.md) | `cache path`, `cache stats`, `cache clean` |

Advanced verbs: `toven release` plans and publishes a release, `toven driver install|list` manages out-of-process ecosystem drivers, and `toven federation sync|status` manages federated member repos.

## Loading config

Commands load `toven.toml` from the current directory, discovering upward if needed. Point at another file with `--config <PATH>`.

Task commands take the task as a positional argument (`toven plan check`, `toven affected test`). `toven modules` and `toven graph` take no task.

## Selecting a baseline

Affected and changed-selection commands diff a git baseline against your working tree:

- `--base <REF>` sets the baseline ref or SHA.
- `--merge-base` diffs from `merge-base(<REF>, HEAD)` instead of `<REF>` directly.
- `[project].base_ref` in `toven.toml` supplies the default baseline.

With neither `--base` nor `[project].base_ref`, affected detection has no baseline and fails with a `no baseline reference` error.

## Passthrough args

Arguments after the task name that Toven does not own are spliced into the task's `{args}` placeholder and sent to the command verbatim. They disable caching unless the task sets `cache_args = true`. See [passing arguments](run.md#passing-arguments-to-the-tasks-command).

## Cache location

Cache records live in the platform user cache directory by default. Override with `TOVEN_CACHE_DIR` or `[toven.cache].dir`. See [managing cache](cache.md).
