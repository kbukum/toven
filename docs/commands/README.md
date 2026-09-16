# Commands

Reference for Toven's built-in CLI commands and the pages that explain them.

## Quickstart

Start with the built-in help, then inspect the repository shape:

```bash
toven --help
toven modules
toven doctor
```

Toven has a fixed set of **reserved commands**. Any other token is treated as a repository-defined task name from `toven.toml`. Run `toven <command> --help` for the exact CLI surface.

## Built-in commands

| Command | What it does | Docs |
|---|---|---|
| `run` | Run a task by name when the task name would otherwise shadow a reserved command | [`run`](run.md) |
| `plan` | Preview what a task would run | [`inspect`](inspect.md) |
| `release` | Plan, inspect, and publish releases | [`release`](release.md) |
| `coverage` | Run coverage and gate it against configured thresholds | [`coverage`](coverage.md) |
| `explain` | Show what a task would run and why | [`inspect`](inspect.md) |
| `init` | Detect ecosystems and write or preview `toven.toml` | [`init`](init.md) |
| `affected` | Project the affected-module set for a task | [`inspect`](inspect.md) |
| `modules` | List discovered modules (`list`, `ls`) | [`inspect`](inspect.md) |
| `graph` | Project the dependency graph (`deps`) | [`inspect`](inspect.md) |
| `tasks` | List runnable tasks by ecosystem | [`inspect`](inspect.md) |
| `doctor` | Audit required tools | [`doctor`](doctor.md) |
| `commit-lint` | Lint a commit subject or PR title | [`commit-lint`](commit-lint.md) |
| `completions` | Print shell completion scripts | [`completions`](completions.md) |
| `driver` | Manage out-of-process drivers | [`driver`](driver.md) |
| `federation` | Manage composed multi-repo driver state | [`federation`](federation.md) |
| `cache` | Inspect or clear the local task cache | [`cache`](cache.md) |
| `help` | Show built-in help for any command | Use `toven help <command>` or `toven <command> --help` |

## Help

```bash
toven --help
toven help doctor
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
