# Command reference

| Topic | Commands |
|-------|----------|
| [Onboarding a repository](init.md) | `toven init` |
| [Running tasks](run.md) | `toven <task>`, `toven run <task>` |
| [Inspecting work](inspect.md) | `plan`, `affected`, `explain`, `modules` (`list`, `ls`), `graph` (`deps`), `tasks`, `completions` |
| [Releasing](release.md) | `release plan`, `release status`, `release readiness`, `release sbom`, `release depgraphs`, `release tag`, `release publish` |
| [Measuring coverage](coverage.md) | `coverage` |
| [Managing cache](cache.md) | `cache path`, `cache stats`, `cache clean` |

Advanced verbs: `toven release plan|status|readiness|sbom|depgraphs|tag|publish` walks a release through its lifecycle, `toven driver install|list` manages out-of-process ecosystem drivers, and `toven federation sync|status` manages federated member repos.

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

## Output streams

Toven keeps a consistent stdout/stderr contract so a projection can be piped while diagnostics stay visible:

- **stdout** carries the requested data projection — the introspection renderings (`modules`/`list`/`ls`, `graph`/`deps`, `affected`, `explain`, `tasks`), the read-only release projections (`release plan`/`status`/`readiness`/`sbom`/`depgraphs`), the `release publish --dry-run` rehearsal tables, the `coverage` per-module verdict table, `cache path`/`stats`, and — for every verb that accepts it — the `--output jsonl` JSON-lines projection. stdout is the only stream a machine consumer needs to read.
- **stderr** carries human progress and diagnostics — the run reporter (PLAN/APPLY summaries and per-unit results for `run`/`plan`/`<task>`/`coverage`, and the mutating `release tag`/`publish`), the `coverage` summary line, the coverage/readiness/sbom skip and warning lines, `driver`/`federation` status output, `cache clean`, task/reserved collision warnings, and the final rendered error.

`--output jsonl` reserves stdout for the JSON-lines projection so a consumer parses one JSON object per line without interleaved human text — typed records for the introspection and release verbs (e.g. `modules` emits `ModuleRecord`, `release plan` emits `PlanRecord`), and the `toven_model::Event` stream for the task verbs (`run`/`plan`/`<task>`/`coverage`). Any human framing for that run moves to stderr. Redirecting stdout therefore captures exactly the data projection, and redirecting stderr captures exactly the diagnostics.

