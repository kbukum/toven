# Inspecting work

Inspection commands are read-only and should be the first step when adopting Toven in a repository.

> Target behavior; returns as the redesign steps land (the CLI is being rebuilt on the `crates/*` + `apps/*` stack).

## `toven plan`

Renders a reviewable execution plan without running subprocesses:

```bash
toven plan
toven plan --task check
toven plan --task check --affected --base origin/main --merge-base
toven plan --task test -- --no-capture
```

The plan includes selected modules, dependency order, execution units, and rendered argv. With `--affected`, Toven resolves changed files first and plans only directly affected modules plus dependents. `--base` and `--merge-base` are valid only with `--affected`.

## `toven affected`

Shows changed paths and the affected module closure for a task:

```bash
toven affected
toven affected --task check
toven affected --task check --base origin/main --merge-base
```

Output includes the baseline provider/OID, changed paths, and modules marked as:

| Reason | Meaning |
|--------|---------|
| `direct` | The module owns at least one changed file. |
| `dependent` | The module depends on a directly affected module. |
| `global` | A shared input or path outside known module ownership invalidates broad work. |

## `toven explain <module> <task>`

Explains affected and cache reasoning for one module/task pair:

```bash
toven explain rskit-config check
toven explain rskit-config check --base origin/main --merge-base
toven explain rskit-config check --force
toven explain rskit-config check --no-cache
```

The command prints module scope, adapter, task, dependencies, affected reason, changed/global paths when present, cache state, and cache hashes. Cache state can be hit, forced, disabled, or miss with a reason. If the same module name exists in multiple scopes, explanation is printed for each matching scope.

Persistent tasks report cache as disabled because they are never persisted as cache hits.

## `toven modules`, `toven list`, `toven ls`

Lists modules discovered for a task:

```bash
toven modules
toven modules --task check
toven list --task test
toven ls --task test
```

Each module is printed as `scope/module`, with adapter, root, optional package name, and dependencies. The command is task-aware because scopes and profiles may expose different modules or task availability.

## `toven graph`, `toven deps`

Renders the discovered dependency graph for a task:

```bash
toven graph
toven graph --task check
toven graph --format dot
toven deps --task test
```

Text format prints each module and its dependencies. Overlay-derived edges are marked with `overlay`. DOT format emits a Graphviz `digraph` for visualization.

## Review checklist

- `plan` shows exact argv and batching clearly enough to review before running.
- `affected` makes baseline and changed paths understandable.
- `explain` exposes enough cache key inputs to diagnose hits and misses.
- `modules` and `graph` make scope-qualified module identity clear.
