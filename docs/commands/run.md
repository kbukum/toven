# Run tasks

Run any task defined under `[ecosystems.<id>.tasks.<name>]`.

```bash
toven run test --workspace rust --dry-run
```

## Syntax

```text
toven <task> [TOVEN_OPTIONS] [TASK_ARGUMENTS...]
toven run <task> [TOVEN_OPTIONS] -- [TASK_ARGUMENTS...]
```

The explicit `run` form handles task names that collide with reserved commands.

```bash
toven check --workspace rust
toven run structure
```

## Execution

A run loads configuration, discovers modules, builds the dependency graph, selects scope, renders argv, checks cache records, and executes misses in dependency waves.

Typical stderr:

```text
plan: 2 units in 2 waves
ok rust:core test
ok rust:cli test
summary: 2 passed
```

Under `--output jsonl`, Toven events use stdout and child output uses stderr.

## Pass task arguments

Toven consumes recognized options at the start of the task argument list. The first token it does not own and everything after it are appended unchanged at `{args}`.

```bash
toven test --workspace rust integration --nocapture
```

Use `--` to pass a colliding flag:

```bash
toven test --workspace rust -- --dry-run
```

Inspect the resulting argv:

```bash
toven explain test --module rust:toven-cli
```

Toven does not correct unknown task flags because they may belong to the underlying command.

## Tool gates as tasks

Tool-specific gates are ordinary tasks, resolved and run through the same machinery — there is no bespoke verb for any of them. Toven drives its own quality gates this way:

| Task | Ecosystem | Resolves to |
|---|---|---|
| `structure` | `command` | `ast-grep scan` (the declare-only `lib.rs`/`mod.rs` guard) |
| `docs-build` | `command` | `mdbook build docs` |
| `deny` | `rust` | `cargo deny check advisories bans licenses sources` |
| `doctest` | `rust` | `cargo test --doc …` (the doctests nextest cannot run) |

```bash
toven run structure
toven run deny --workspace rust
toven run docs-build
toven run doctest --workspace rust -- --all-features
```

The `command` ecosystem is Toven's generic-tool adapter: a tool-specific but language-agnostic gate that has no cargo/go home is declared under `[ecosystems.command.tasks.<name>]`. A cargo-specific but repo-opt-in gate like `deny` (it needs a `deny.toml`) is a declared `[ecosystems.rust.tasks.deny]` task rather than a baked-in adapter default. See the [language- and tool-agnostic core](../engineering.md#language--and-tool-agnostic-core) principle for why each gate lives where it does. Inspect the resolved argv for any of them with `toven explain <task>`.

## Select scope

```bash
toven test --module toven-cli
toven test --module rust:toven-cli
toven test --module 'rust:*'
toven test --workspace rust
toven test --module rust:toven-cli --dependents
toven test --module rust:toven-cli --dependencies
```

Selectors may be a bare module name, canonical `ecosystem:name`, `workspace/name`, or glob. Ambiguous bare names fail and list qualified candidates.

`--module` and `--workspace` are repeatable. They cannot be combined with `--base` or `--merge-base`.

## Select changed work

```bash
toven structure --base origin/main --merge-base
```

Changed modules and their dependents run. See [baseline selection](README.md#baseline-selection).

## Cache control

```bash
toven test --workspace rust --refresh
toven test --workspace rust --no-cache
```

- `--refresh` ignores existing records and writes successful replacements.
- `--no-cache` reads and writes no cache records.

The options are mutually exclusive. See [cache management](cache.md).

## Concurrency

```bash
toven test --workspace rust --jobs 1
toven test --workspace rust --jobs 4
toven test --workspace rust -j 4
```

`--jobs <N>` overrides `[toven].max_parallel`. `--jobs 1` executes serially and uses an inline stream under the default view.

## Live output

```bash
toven test --workspace rust --view auto
toven test --workspace rust --view tiles
toven test --workspace rust --view panes
toven test --workspace rust --view stream
```

| View | Behavior |
|---|---|
| `auto` | Select panes, tiles, or stream from terminal and concurrency conditions |
| `tiles` | One terminal region per active unit |
| `panes` | tmux panes when supported, otherwise tiles |
| `stream` | Deterministic linear output |

Non-terminal and JSONL runs always use stream behavior.

## Watch mode

```bash
toven test --workspace rust --watch
toven test --workspace rust --watch --watch-debounce-ms 500
```

Watch mode reruns the affected subgraph after file changes. Ctrl+C cancels active work and exits.

## Timeout and failure policy

```bash
toven test --workspace rust --timeout 90s --fail-fast
```

`--timeout` applies per execution unit. `--fail-fast` stops scheduling new work after the first failure.

## Options

| Option | Effect |
|---|---|
| `--dry-run` | Plan without execution |
| `--explain` | Plan and report reasoning without execution |
| `--fail-fast` | Stop scheduling after the first failure |
| `--no-cache` | Bypass cache reads and writes |
| `--refresh` | Re-run and replace successful cache records |
| `--timeout <DURATION>` | Bound each unit, such as `30s` or `5m` |
| `--jobs <N>`, `-j <N>` | Limit concurrent units |
| `--base <REF>` | Select changed work against a Git baseline |
| `--merge-base` | Compare from the baseline's merge base |
| `--module <SELECTOR>` | Select modules; repeatable |
| `--workspace <SELECTOR>` | Select workspaces; repeatable |
| `--dependents` | Include reverse dependencies |
| `--dependencies` | Include forward dependencies |
| `--watch` | Re-run after source changes |
| `--watch-debounce-ms <N>` | Set watch debounce; default `200` |
| `--output human\|jsonl` | Select output format |
| `--view auto\|tiles\|panes\|stream` | Select output renderer |
| `--color auto\|always\|never` | Control status color |
