# Inspecting work

Inspection commands are read-only and should be the first step when adopting Toven in a repository.

## `toven plan`

Renders a reviewable execution plan without running subprocesses:

```bash
toven plan check
toven plan check --base origin/main --merge-base
toven plan check --module rust:core --with-dependents
toven plan test
```

`toven plan` renders the plan as an event stream rather than executing it: a `plan: N units in M waves` line plus the terminal run summary. Add `-v` (verbose) to also see per-phase markers and a per-unit `cache <unit>: <verdict>` line for each unit. With `--base` and/or `--merge-base`, Toven resolves changed files first and plans only directly affected modules plus dependents. With `--module`/`--workspace` (optionally `--with-dependents`) it plans exactly the named targets instead; the explicit and changed-selection flags are mutually exclusive. It does not print argv (use `toven explain` for argv) and does not accept passthrough args.

## `toven affected`

Lists the modules with a scheduled unit for a task, given a baseline or an explicit selection:

```bash
toven affected check
toven affected check --base origin/main --merge-base
toven affected check --module rust:core --with-dependents
```

The output is a table of affected `ecosystem:module` refs (the directly changed modules plus their dependents). It does not currently surface the baseline OID, changed paths, or a per-module reason category.

## `toven explain <module> <task>`

Shows the planned unit(s) for one module/task pair:

```bash
toven explain rust:rskit-config check
```

For each matching unit the command prints a key/value block: `unit` id, `module`, `task`, `argv`, `persistent`, and `depends_on`. The module argument is an `ecosystem:module` ref, such as `rust:rskit-config`; `explain` plans every module (it does not use `--base` or `--merge-base`) and does not surface cache or affected reasoning.

## `toven modules`, `toven list`, `toven ls`

Lists modules discovered for a task:

```bash
toven modules
toven list
toven ls
```

Each discovered module is printed as a scope-qualified `ecosystem:module` ref in a table. The command takes no task argument.

## `toven graph`, `toven deps`

Renders the discovered dependency graph:

```bash
toven graph
toven graph --format dot
toven deps
```

Text format prints each module and its dependencies. DOT format emits a Graphviz `digraph` for visualization.

## Review checklist

- `plan` shows the unit/wave counts and (at `-v`) per-unit cache verdicts clearly enough to review before running.
- `affected` makes the affected module set understandable for a baseline.
- `explain` shows the planned unit details (argv, dependencies, persistence) for a module/task.
- `modules` and `graph` make scope-qualified module identity clear.
