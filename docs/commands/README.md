# Command reference

| Topic | Commands |
|-------|----------|
| [Onboarding a repository](init.md) | `toven init` |
| [Running tasks](run.md) | `toven <task>`, `toven run <task>` |
| [Inspecting work](inspect.md) | `plan`, `affected`, `explain`, `modules` (`list`, `ls`), `graph` (`deps`), `tasks`, `completions` |
| [Managing cache](cache.md) | `cache path`, `cache stats`, `cache clean` |

Advanced verbs: `toven release` plans and publishes a release, `toven driver install|list` manages out-of-process ecosystem drivers, and `toven federation sync|status` manages federated member repos.

Many commands carry worked examples in their `--help`; run `toven <command> --help` to see the usage and any examples.

## Loading config

Commands load `toven.toml` from the current directory, discovering upward if needed. Point at another file with `--config <PATH>`.

Task commands take the task as a positional argument (`toven plan check`, `toven affected test`). `toven modules` and `toven graph` take no task.

## Selecting a baseline

Affected and changed-selection commands diff a git baseline against your working tree:

- `--base <REF>` sets the baseline ref or SHA.
- `--merge-base` diffs from `merge-base(<REF>, HEAD)` instead of `<REF>` directly.
- `[project].base_ref` in `toven.toml` supplies the default baseline; under a federation each member can supply its own `[[members]].base_ref`.

When no baseline is available — no `--base`, and no `[project].base_ref` or `[[members]].base_ref` — affected detection fails with a `no baseline reference` error.

## Passthrough args

Arguments after the task name that Toven does not own are spliced into the task's `{args}` placeholder and sent to the command verbatim. They disable caching unless the task sets `cache_args = true`. See [passing arguments](run.md#passing-arguments-to-the-tasks-command).

## Cache location

Cache records live in the platform user cache directory by default. Override with `TOVEN_CACHE_DIR` or `[toven.cache].dir`. See [managing cache](cache.md).

## Color output

The human reporter colorizes status labels (green success, red failure, yellow blocked/cancelled, dim cache hit) when writing to a terminal. Control it with `--color`:

- `--color auto` (default) colorizes only when stderr is a terminal.
- `--color always` forces color on (still overridden by `NO_COLOR`).
- `--color never` disables color.

Setting the [`NO_COLOR`](https://no-color.org) environment variable to any non-empty value disables color regardless of `--color`. The machine-readable `--output jsonl` stream is never colorized.

