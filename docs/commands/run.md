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

## Common examples

```bash
toven check
toven check --dry-run
toven check --explain
toven check --fail-fast
toven test -- --no-capture
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
| `--output human\|jsonl` | Select human or machine-readable run events. |
| `-v` / `-q` | Raise or lower reporter verbosity (repeatable): quiet shows only the run summary, verbose adds per-phase, cache, and unit-lifecycle lines. Affects human output only; the JSONL stream always carries every event. |

## Discussion points

- Whether human output is close enough to native tool output.
- Whether JSONL events expose enough plan, cache, lifecycle, and timing data.
- Whether ready-wave parallelism preserves expected ordering.
- Whether any remaining output-fidelity gap justifies an opt-in PTY path.
