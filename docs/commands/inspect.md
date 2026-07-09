# Inspecting work

These commands are read-only. Run them first when adopting Toven in a repository.

## `toven plan`

Renders the execution plan without running anything:

```bash
toven plan check
toven plan check --base origin/main --merge-base
toven plan check --module rust:core --dependents
```

Output is an event stream: a `plan: N units in M waves` line plus the run summary. Add `-v` for per-phase markers and a `cache <unit>: <verdict>` line per unit.

With `--base`/`--merge-base` it plans directly affected modules plus dependents; with `--module`/`--workspace` it plans the named targets (the two are mutually exclusive). It does not print argv — use [`toven explain`](#toven-explain-task) — and takes no passthrough args.

## `toven affected`

Lists the modules with a scheduled unit for a task:

```bash
toven affected check --base origin/main --merge-base
toven affected check --module rust:core --dependents
```

Output is a table of `ecosystem:module` refs: the changed modules plus dependents (with a baseline), or the explicitly selected targets (with `--module`/`--workspace`).

When a changed path cannot be attributed to any module — a config edit (`toven.toml`), a root-level file (`README`, CI, `LICENSE`, `.gitignore`), or the untracked `toven.toml` right after `init` — Toven forces **full activation** (every module) and emits a diagnostic naming the path(s): `full activation: toven.toml (affects all modules)`. This is correct: the config defines every module's tasks, argv, run strategy, and selectors, so an unattributable change can alter any module's plan. A precisely attributable source edit prints no such line. The diagnostic rides the `affected` projection's stdout stream; during a `toven <task>` run it goes to the human reporter on stderr.

## `toven explain <task>`

Shows the planned unit(s) for a task, optionally focused to a `--module`/`--workspace` selection:

```bash
toven explain check                        # every planned unit for the task
toven explain check --module rust:core     # the real unit(s) rust:core runs in
```

With `--module`/`--workspace`, `explain` *focuses* the projection: it builds the full task plan and shows only the real batched unit(s) containing the selected module(s) — those members marked in a `target` field, their co-batched siblings still listed in `modules` — never a synthetic single-module cut. Without a focus it shows every planned unit for the scope, honoring `--base`/`--merge-base` changed selections. Selectors use the shared grammar (bare name, `ecosystem:name`, `workspace/name`, or a glob); output stays the canonical `ecosystem:module` form.

Each unit prints: `unit`, `representative`, `modules`, `task`, `origin`, `argv`, `persistent`, `depends_on`, and `target` when focused.

## `toven modules` (`list`, `ls`)

Lists discovered modules as scope-qualified `ecosystem:module` refs. Takes no task argument:

```bash
toven modules
toven modules --output jsonl
```

The human table carries a `Module` and a `Workspace` column, so modules that share a name across workspaces stay distinguishable in a large polyrepo. `--output jsonl` emits one object per module — its `module` (`ecosystem:module`) key and owning `workspace` (`null` when the module is not in a named workspace) — as a machine-readable stream on stdout.

## `toven graph` (`deps`)

Renders the dependency graph. Text prints each module and its dependencies; DOT emits a Graphviz `digraph`:

```bash
toven graph
toven graph --format dot
```

## `toven tasks`

Lists the runnable tasks resolved for each ecosystem, so you can see every valid task name before running one. Task names are the identities defined in the config — the Rust format task is named `format`, even though the command it runs is `cargo fmt`:

```bash
toven tasks
toven tasks format
toven tasks --output jsonl
```

Without an argument it prints one table per ecosystem (task name, origin, fan-out, and whether the task is persistent). Pass a task name to show that task's detail — its canonical name, argv template, and cache inputs. `--output jsonl` emits the same catalog as a machine-readable stream. If you run an unknown task, Toven suggests the nearest valid name and points you back at `toven tasks`.

## `toven completions <shell>`

Prints a shell completion script to stdout for `bash`, `zsh`, `fish`, `powershell`, or `elvish`:

```bash
toven completions zsh > _toven          # save for your fpath
source <(toven completions bash)        # load into the current shell
```

The script completes the reserved verbs, their flags, and the global options. It does not complete argv-first task names (those are repository-specific — use `toven tasks` to list them).

