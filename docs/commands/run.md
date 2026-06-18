# Running tasks

Run a configured or adapter-provided task directly:

> Target behavior; returns as the redesign steps land (the CLI is being rebuilt on the `crates/*` + `apps/*` stack).

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
toven check --affected --base origin/main --merge-base
toven check --no-cache
toven check --force
toven check --timeout-seconds 300
toven test -- --no-capture
```

## Output modes

Human output is the default:

```bash
toven check --output human
```

Human mode streams child process bytes for terminal use and reports Toven lifecycle lines such as `run:`, `done:`, `ready:`, cache hit, miss, forced, disabled states, and final timing.

JSONL output is intended for tools:

```bash
toven check --output jsonl
```

JSONL mode reserves stdout for newline-delimited Toven events. Child stdout is forwarded to stderr so consumers can parse every stdout line as JSON. Cache decision events expose the same structured cache states and reasons.

## Watch mode

Watch mode runs the task once, watches the project root, then reruns affected modules and reverse dependents after file changes:

```bash
toven test --watch
toven check --watch --watch-debounce-ms 500
```

Watch mode ignores generated/noisy paths such as `.git/`, `.toven/`, `target/`, and `node_modules/`. If the Toven config changes, watch mode reloads the workspace and performs a broader rerun because discovery or task policy may have changed.

Persistent tasks opt out of cache, can wait for readiness, and are restarted when watch invalidation requires a new affected set.

## Cache location

Task cache records use the platform user cache directory by default so normal runs do not create repository file changes. Use `toven cache path` to inspect the resolved directory. Set `TOVEN_CACHE_DIR` to an absolute path for isolated CI/benchmark runs, or configure `[cache] location = "workspace"` to store under `.toven/cache`.

## Options

| Option | Purpose |
|--------|---------|
| `--affected` | Run only modules affected by the selected git baseline. |
| `--base REF` | Baseline ref or SHA for affected detection. |
| `--merge-base` | Use the merge-base of `HEAD` and the selected baseline. |
| `--no-cache` | Disable cache reads and writes. |
| `--force` | Skip cache reads and write fresh success records. |
| `--timeout-seconds SECONDS` | Bound child process execution time. |
| `--output human\|jsonl` | Select human or machine-readable run events. |
| `--watch` | Watch files and rerun affected work. |
| `--watch-debounce-ms MILLIS` | Debounce interval for watch events. |

## Discussion points

- Whether human output is close enough to native tool output.
- Whether JSONL events expose enough plan, cache, lifecycle, and timing data.
- Whether ready-wave parallelism preserves expected ordering.
- Whether any remaining output-fidelity gap justifies an opt-in PTY path.
