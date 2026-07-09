# Running tasks

Run a task from the config task table directly:

```bash
toven check
toven test
```

The task token is the task's addressable name — any entry in the config task table. `init` seeds a starter set (`build`, `check`, `format`, `lint`, `test`, `doc`, `run`), and you add, rename, or remove entries under `[ecosystems.<id>.tasks.<name>]` (for example `toven test-integration`). A task's optional `kind` is a recognition hint, not a fixed catalog. Run `toven tasks` to list every runnable task.

Use `toven run <task>` when the task name shadows a reserved verb (`run`, `plan`, `init`, `graph`, `cache`, …):

```bash
toven run check
```

A run loads config, discovers modules, builds dependency waves, decides cache hits, and executes the misses. Modules in the same wave run in parallel when the plan allows; dependency order holds across waves.

## Passing arguments to the task's command

Toven consumes only its own flags that appear as a contiguous prefix right after the task name. The first token it does not own — a positional argument or an unrecognized flag — starts the command's argument vector: it and everything after it are appended to the command verbatim.

```bash
toven test integration --nocapture   # runs: <test command> integration --nocapture
toven test --release --features full  # --release and --features go to the command
toven test --dry-run integration      # --dry-run is Toven's; integration... is the command's
```

When a leading flag matches one of Toven's own, Toven claims it. So `toven test --dry-run` runs Toven's dry-run planning. To send a colliding flag to the command, use `--` or place it after a positional token:

```bash
toven test -- --dry-run          # --dry-run goes to the command
toven test integration --dry-run  # positional ends Toven's prefix; --dry-run goes to the command
```

A misspelled Toven flag passes through to the command rather than erroring: `toven test --moduel rust:core` sends `--moduel rust:core` to your command. Use [`toven explain`](inspect.md#toven-explain-task) to see the exact argv Toven planned.

`toven run <task>` is a reserved subcommand, so its passthrough is collected only after an explicit `--`. Write `toven run test -- integration --nocapture`. Prefer the bare `toven test …` form for friction-free passthrough.

## Selecting which modules run

By default a task plans every module, or — with a baseline — only changed modules and their dependents. Select the graph explicitly instead. A `--module` value is lenient: a bare name (`core`), an `ecosystem:name` ref (`rust:core`), a `workspace/name` ref (`backend/api`), or a glob (`rust:*`, `rskit-*`). A bare name that matches modules in more than one ecosystem is a typed ambiguity error listing the qualified candidates; a glob is an explicit set. Output always stays the canonical `ecosystem:module` form.

```bash
toven test --module core                          # the module named core (any ecosystem, if unambiguous)
toven test --module rust:core                     # only rust:core
toven test --module 'rust:*'                       # every rust module (glob)
toven test --workspace rust                        # every module in the rust workspace
toven test --module rust:core --dependents         # rust:core and everything that depends on it
toven test --module rust:core --dependencies       # rust:core and everything it needs, in order
```

`--module` and `--workspace` are repeatable and mutually exclusive with the baseline flags (`--base`/`--merge-base`). `--dependents` and `--dependencies` are only valid alongside an explicit selection and may be combined. See [selecting a baseline](README.md#selecting-a-baseline).

## Watch mode

`--watch` keeps a task running: after the first run it reruns the affected subgraph each time a source file changes. Ctrl+C cancels any in-flight run and exits.

```bash
toven test --watch
toven test --watch --watch-debounce-ms 500   # coalesce bursts over a 500ms window
```

Each change batch is relativized against the workspace root and filtered to drop paths inside `.git` and paths the repo ignores. If the watcher drops events under a large burst, Toven reruns the whole watched scope rather than trusting a partial list. `--watch-debounce-ms` sets the trailing-edge debounce window (default 200).

Watch is a task-APPLY loop: it works on `toven <task>` and `toven run <task>`, not on inspection or `release` verbs, and cannot combine with `--dry-run`/`--explain`.

Reruns follow the detected changes, not the initial selection. When you start watch with an explicit scope (`--module`/`--workspace`), the first run honors that scope, but each rerun plans the affected subgraph of what changed — the changed module plus every module that depends on it. Editing a foundational module therefore reruns every dependent, which can be broader than the module you selected. This matches `affected` semantics.

## Cache control: `--refresh` vs `--no-cache`

Both flags force every selected unit to re-run:

- `--refresh` ignores existing records, re-runs, and writes the fresh results back. Use it to rebuild a result you distrust.
- `--no-cache` bypasses the cache entirely — nothing is read or written. Use it for a one-off run that should not touch the cache.

The two are mutually exclusive. See [what invalidates cache](cache.md#what-invalidates-cache).

## Per-unit timeout

`--timeout <duration>` bounds how long any single unit may run (`30s`, `5m`, `2h`). On overrun the unit is cooperatively cancelled — the same teardown as `--fail-fast` and Ctrl+C, so no child process leaks — and recorded as a timeout failure that drives a non-zero exit. It applies to normal units only; persistent tasks use their configured readiness timeout.

```bash
toven test --timeout 90s
```

## Output modes

Human output is the default. It streams child process bytes and reports lifecycle lines (`run:`, `done:`, `ready:`, cache states, final timing):

```bash
toven check --output human
```

JSONL output reserves stdout for newline-delimited Toven events; child stdout is forwarded to stderr so every stdout line parses as JSON:

```bash
toven check --output jsonl
```

`-v` adds per-phase, cache, and unit-lifecycle lines to human output; `-q` shows only the run summary. The JSONL stream always carries every event.

At the default verbosity the run summary collapses the failure counters (`failed`, `blocked`, `cancelled`, `failed-readiness`, `timed-out`) to only the ones that are non-zero, so a clean run stays terse; `-v` restores the full fixed-width table. Status labels are colorized on a terminal — see [color output](README.md#color-output).

If you run an unknown task, Toven suggests the nearest valid task name and points you at [`toven tasks`](inspect.md#toven-tasks). For example `toven fmt` is rejected with a "Did you mean 'format'?" hint — Toven does not silently rewrite it, since argv is never inferred.

When human output goes to a real terminal, serially-run commands (serial or single-unit runs, no held persistent unit) execute attached to a pseudoterminal sized to that terminal, so their output renders exactly as it would interactively — colors, progress bars, and other tty-gated styling are preserved verbatim. When output is redirected, captured, or units run in parallel, Toven falls back to deterministic pipe capture (no tty), so tools that gate styling on a terminal emit plain text. Pseudoterminal streaming is currently Unix-only; on other platforms Toven always uses pipe capture. Selection is automatic from whether **stderr** (where live output lands) is a terminal — there is no flag to force or disable it; redirect or pipe stderr (e.g. `2>&1 | cat`, or `2>file`) to force deterministic pipe capture even from a terminal.

## Options

| Option | Purpose |
|--------|---------|
| `--dry-run` | Stop after PLAN and report what would run. |
| `--explain` | Stop after PLAN with reasoning detail. |
| `--fail-fast` | Stop scheduling new work after the first failure. |
| `--no-cache` | Bypass the cache: every unit re-runs; nothing is read or written. |
| `--refresh` | Re-run every unit and write fresh records back (mutually exclusive with `--no-cache`). |
| `--timeout <duration>` | Bound each unit's runtime (`30s`, `5m`); overrun is reported as a timeout failure. |
| `--base <ref>` | Diff against `<ref>` for changed selection (overrides `[project].base_ref`). |
| `--merge-base` | Diff against `merge-base(<ref>, HEAD)`. |
| `--module <selector>` | Activate modules by selector — bare name, `ecosystem:name`, `workspace/name`, or glob (repeatable). |
| `--workspace <selector>` | Activate every module owned by a workspace, by id or glob (repeatable). |
| `--dependents` | With `--module`/`--workspace`, also activate the reverse-dependents closure. |
| `--dependencies` | With `--module`/`--workspace`, also activate the forward-dependencies closure. |
| `--watch` | Rerun the affected subgraph on every watched source change (Ctrl+C exits). |
| `--watch-debounce-ms <n>` | Trailing-edge debounce window in ms for `--watch` (default 200). |
| `--output human\|jsonl` | Select human or machine-readable run events. |
| `--color auto\|always\|never` | Colorize human status labels (`auto` follows the terminal; `NO_COLOR` overrides `always`). |
| `-v` / `-q` | Raise or lower human-output verbosity (repeatable). |
