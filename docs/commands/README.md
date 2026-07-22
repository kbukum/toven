# Command reference

Toven accepts reserved commands and repository-defined task names.

## Command groups

| Goal | Command |
|---|---|
| Create or extend `toven.toml` | [`toven init`](init.md) |
| Run a configured task | [`toven <task>`](run.md) |
| Preview or explain work | [`plan`, `affected`, `explain`](inspect.md) |
| Inspect modules, tasks, or dependencies | [`modules`, `tasks`, `graph`](inspect.md) |
| Inspect or clear cache records | [`cache`](cache.md) |
| Measure and gate coverage | [`coverage`](coverage.md) |
| Plan or execute a release | [`release`](release.md) |

Run command help:

```bash
toven --help
toven release --help
```

Help text is written to stdout. Invalid syntax is reported on stderr and returns a usage exit status.

## Configuration discovery

Commands search from the current directory upward for `toven.toml`. Select another file explicitly:

```bash
toven --config path/to/toven.toml modules
```

## Task passthrough

For a bare task command, Toven consumes its recognized options immediately after the task name. The first unrecognized token starts task argv passthrough.

```bash
toven test --module rust:core --nocapture
```

Here `--module rust:core` belongs to Toven and `--nocapture` belongs to the configured test command. Use `--` when a task flag collides with a Toven flag:

```bash
toven test -- --dry-run
```

See [task argument parsing](run.md#pass-task-arguments).

## Output streams

| Stream | Content |
|---|---|
| stdout | Read-only tables, generated projections, and `--output jsonl` records |
| stderr | Human progress, child-process output, warnings, summaries, and final errors |

```bash
toven modules > modules.txt 2> diagnostics.txt
toven modules --output jsonl > modules.jsonl
```

JSONL mode reserves stdout for one JSON object per line. Human framing remains on stderr.

## Global output options

| Option | Effect |
|---|---|
| `--output human\|jsonl` | Select human tables or machine-readable JSONL |
| `--color auto\|always\|never` | Control human status color |
| `-v` | Increase human verbosity |
| `-q` | Reduce human verbosity |

`NO_COLOR` disables color when set to a non-empty value.

## Baseline selection

```bash
toven affected test --base origin/main --merge-base
```

- `--base <REF>` selects a Git ref or commit.
- `--merge-base` compares from `merge-base(<REF>, HEAD)`.
- `[project].base_ref` supplies the default when `--base` is absent.

Affected planning fails when no baseline is available.
