# Command reference

Start by listing the modules Toven discovered:

```bash
toven modules
```

Toven accepts **reserved commands** and repository-defined **task names**. Run `toven <command> --help` for command-specific examples.

## Command groups

| Goal | Command |
|---|---|
| Create or extend `toven.toml` | [`toven init`](init.md) |
| Run a configured task | [`toven <task>`](run.md) |
| Preview or explain work | [`plan`, `affected`, `explain`](inspect.md) |
| Inspect modules, tasks, or dependencies | [`modules`, `tasks`, `graph`](inspect.md) |
| Inspect or clear task-cache records | [`cache`](cache.md) |
| Measure and gate coverage | [`coverage`](coverage.md) |
| Audit required tools | [`doctor`](doctor.md) |
| Lint a commit message or PR title | [`commit-lint`](commit-lint.md) |
| Plan or execute a release | [`release`](release.md) |
| Provision ecosystem drivers | [`driver`](driver.md) |
| Provision drivers across composed repos | [`federation`](federation.md) |
| Generate shell completion scripts | [`completions`](completions.md) |

## Help

```bash
toven --help
toven release --help
```

Help text is written to stdout. Invalid syntax is reported on stderr and exits with code 2.

## Configuration discovery

Commands search upward from the current directory for `toven.toml`. Pass `--config` to choose a file directly.

```bash
toven --config path/to/toven.toml modules
```

## Task passthrough

For a bare task command, Toven consumes recognized options right after the task name. The first unrecognized token starts task argv passthrough.

```bash
toven test --module rust:core --nocapture
```

Here `--module rust:core` belongs to Toven. `--nocapture` belongs to the configured test command. Use `--` when a task flag collides with a Toven flag.

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

JSONL mode reserves stdout for one JSON object per line. Human framing stays on stderr.

## Common options

| Option | Effect |
|---|---|
| `--config <PATH>` | Load a specific `toven.toml` |
| `--output human\|jsonl` | Select human tables or machine-readable JSONL |
| `--color auto\|always\|never` | Control human status color |
| `-v`, `--verbose` | Increase human verbosity on execution verbs; repeatable |
| `-q`, `--quiet` | Reduce human verbosity on execution verbs; repeatable |

`NO_COLOR`, when set to a non-empty value, disables color.

## Exit codes

Automation can branch on exit codes without parsing text. Clap usage errors, such as unknown flags or bad subcommands, exit 2.

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Unclassified failure, including a task or unit that ran and failed |
| 2 | Usage error: invalid input, invalid format, or a missing required argument |
| 3 | Permission: authentication or authorization failure |
| 4 | Not found: a required resource is missing, such as `toven.toml` |
| 5 | Conflict: the request conflicts with immutable state, such as a divergent existing release |
| 69 | Unavailable: a remote dependency or service failed |
| 75 | Rate limited |
| 124 | Timed out |
| 130 | Cancelled, such as Ctrl+C |

A clean task run exits 0. A failing task run exits non-zero.

## Baseline selection

```bash
toven affected test --base origin/main --merge-base
```

- `--base <REF>` selects a Git ref or commit.
- `--merge-base` compares from `merge-base(<REF>, HEAD)`.
- `[project].base_ref` supplies the default when `--base` is absent.

Affected planning fails when no baseline is available.
