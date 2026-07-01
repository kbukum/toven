# Running tasks

Run a configured or adapter-provided task directly:

```bash
toven check
toven test
```

Use `toven run <task>` when the task name is reserved by a built-in command:

```bash
toven run check
```

## Behavior

Task execution loads config, discovers modules for the task, builds dependency waves, prepares cache decisions, and executes cache misses. Cache hits are skipped unless the planned unit is workspace-once and must run as part of a coalesced command.

Modules in the same ready wave can execute in parallel when the plan and resource grouping allow it. Dependency order is still preserved across waves.

## Passing arguments to the task's command

Toven consumes only its own flags when they appear as a *contiguous prefix* right after the task name. The first token that is not a Toven flag — a positional argument or a flag Toven does not own — begins the task's own argument vector: it and everything after it are appended to the command verbatim, never interpreted.

```bash
toven test integration --nocapture      # runs: <test command> integration --nocapture
toven test --release --features full     # --release/--features go straight to the command
toven test --dry-run integration         # --dry-run is Toven's; integration... is the command's
```

An explicit `--` still forces the boundary early, which is only needed when the very first task argument would otherwise be read as a Toven flag:

```bash
toven test -- --explain                  # pass --explain to the command, not to Toven
```

A name collision is resolved deterministically: when a leading token in the prefix exactly matches one of Toven's own flags, Toven claims it. So `toven test --dry-run` runs Toven's dry-run planning, not a `--dry-run` flag on your test command. To send a colliding flag to the command instead, use `--` (`toven test -- --dry-run`), or place it after any positional or non-Toven token (`toven test integration --dry-run`), both of which end Toven's prefix. Only the leading, contiguous run of Toven-owned flags is ever absorbed; Toven never rewrites the tokens it passes through — it only decides where its prefix ends.

Because the boundary is "the first token Toven does not recognize", a *misspelled* Toven flag passes through to the command rather than erroring: `toven test --moduel rust:core` sends `--moduel rust:core` to your command verbatim (it does not select the `rust:core` module). Use `toven explain <module> <task>` to see the exact `argv` Toven planned if a Toven flag seems to have no effect.

## Selecting which modules run

By default a task plans every module (or, with a baseline, only changed modules). Select the graph explicitly instead:

```bash
toven test --module rust:core            # only the rust:core module
toven test --workspace rust              # every module owned by the rust workspace
toven test --module rust:core --with-dependents   # rust:core and everything that depends on it
```

`--module`/`--workspace` are repeatable and mutually exclusive with the changed-selection flags (`--base`/`--merge-base`).

## Common examples

```bash
toven check
toven check --dry-run
toven check --explain
toven check --fail-fast
toven test --nocapture
```

## Output modes

Human output is the default:

```bash
toven check --output human
```

Human mode streams child process bytes for terminal use and reports Toven lifecycle lines such as `run:`, `done:`, `ready:`, cache hit, miss, disabled states, and final timing.

JSONL output is intended for tools:

```bash
toven check --output jsonl
```

JSONL mode reserves stdout for newline-delimited Toven events. Child stdout is forwarded to stderr so consumers can parse every stdout line as JSON. Cache decision events expose the same structured cache states and reasons.

## Cache location

Task cache records use the platform user cache directory by default so normal runs do not create repository file changes. Use `toven cache path` to inspect the resolved directory. Set `TOVEN_CACHE_DIR` to an absolute path for isolated CI/benchmark runs, or configure the workspace-relative cache directory in TOML:

```toml
[toven.cache]
dir = ".toven/cache"
```

## Options

| Option | Purpose |
|--------|---------|
| `--dry-run` | Stop after PLAN and report what would run. |
| `--explain` | Stop after PLAN with reasoning detail. |
| `--fail-fast` | Stop scheduling new work after the first failure. |
| `--no-cache` | Bypass the task cache: every unit re-runs and no records are read or written. |
| `--base <ref>` | Changed selection: diff against `<ref>` (overrides configured `base_ref`). |
| `--merge-base` | Changed selection: diff against `merge-base(<ref>, HEAD)`. |
| `--module <ecosystem:name>` | Explicit selection: activate this module (repeatable). |
| `--workspace <id>` | Explicit selection: activate every module owned by the workspace (repeatable). |
| `--with-dependents` | With `--module`/`--workspace`, also activate the reverse-dependents closure. |
| `--output human\|jsonl` | Select human or machine-readable run events. |
| `-v` / `-q` | Raise or lower reporter verbosity (repeatable): quiet shows only the run summary, verbose adds per-phase, cache, and unit-lifecycle lines. Affects human output only; the JSONL stream always carries every event. |

## Discussion points

- Whether human output is close enough to native tool output.
- Whether JSONL events expose enough plan, cache, lifecycle, and timing data.
- Whether ready-wave parallelism preserves expected ordering.
- Whether any remaining output-fidelity gap justifies an opt-in PTY path.
