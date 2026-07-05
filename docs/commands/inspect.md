# Inspecting work

These commands are read-only. Run them first when adopting Toven in a repository.

## `toven plan`

Renders the execution plan without running anything:

```bash
toven plan check
toven plan check --base origin/main --merge-base
toven plan check --module rust:core --with-dependents
```

Output is an event stream: a `plan: N units in M waves` line plus the run summary. Add `-v` for per-phase markers and a `cache <unit>: <verdict>` line per unit.

With `--base`/`--merge-base` it plans directly affected modules plus dependents; with `--module`/`--workspace` it plans the named targets (the two are mutually exclusive). It does not print argv — use [`toven explain`](#toven-explain-module-task) — and takes no passthrough args.

## `toven affected`

Lists the modules with a scheduled unit for a task:

```bash
toven affected check --base origin/main --merge-base
toven affected check --module rust:core --with-dependents
```

Output is a table of `ecosystem:module` refs: the changed modules plus dependents (with a baseline), or the explicitly selected targets (with `--module`/`--workspace`).

## `toven explain <module> <task>`

Shows the planned unit(s) for one module and task:

```bash
toven explain rust:rskit-config check
```

For each matching unit it prints a key/value block: `unit`, `module`, `task`, `argv`, `persistent`, and `depends_on`. The module argument is an `ecosystem:module` ref. `explain` plans every module and does not use `--base`/`--merge-base`.

## `toven modules` (`list`, `ls`)

Lists discovered modules as scope-qualified `ecosystem:module` refs. Takes no task argument:

```bash
toven modules
```

## `toven graph` (`deps`)

Renders the dependency graph. Text prints each module and its dependencies; DOT emits a Graphviz `digraph`:

```bash
toven graph
toven graph --format dot
```

## `toven tasks`

Lists the runnable tasks resolved for each ecosystem, so you can see every valid task name before running one. Task names are the *canonical* form (for example `format`, not the `fmt` shorthand):

```bash
toven tasks
toven tasks format
toven tasks --output jsonl
```

Without an argument it prints one table per ecosystem (task name, run strategy, and origin). Pass a task name to show that task's detail — its canonical name, argv template, and cache inputs. `--output jsonl` emits the same catalog as a machine-readable stream. If you run an unknown task, Toven suggests the nearest valid name and points you back at `toven tasks`.

## `toven completions <shell>`

Prints a shell completion script to stdout for `bash`, `zsh`, `fish`, `powershell`, or `elvish`:

```bash
toven completions zsh > _toven          # save for your fpath
source <(toven completions bash)        # load into the current shell
```

The script completes the reserved verbs, their flags, and the global options. It does not complete argv-first task names (those are repository-specific — use `toven tasks` to list them).

