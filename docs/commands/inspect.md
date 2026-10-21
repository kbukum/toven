# Inspect plans and graphs

Start by listing modules:

```bash
toven modules
```

Inspection commands are read-only. Use them before executing unfamiliar configuration. There is no `toven inspect` command; this page groups `plan`, `affected`, `explain`, `modules`, `groups`, `graph`, and `tasks`.

`plan`, `affected` and `explain` accept the same task forms as `toven run`: one task, a comma list (`build,test`) or a `[sets.<name>]` set. A list or set shows the combined plan, its affected modules, or its units.

## Plan a task

```text
toven plan <task> [SELECTION_OPTIONS]
```

```bash
toven plan check --workspace rust
toven plan structure --base origin/main --merge-base
toven plan check --module rust:toven-cli --dependents
```

Human planning reports unit and wave counts on stderr:

```text
plan: 2 units in 2 waves
```

## List affected modules

```text
toven affected <task> [SELECTION_OPTIONS]
```

```bash
toven affected structure --base origin/main --merge-base
```

Example stdout:

```text
Module
command:repo
```

Repository-level changes that cannot be assigned to one module activate the full scope and include an explanatory diagnostic.

For tooling such as a CI matrix, `--output jsonl` prints one record per affected module, in the same shape as `toven modules --output jsonl`. The full-activation diagnostic then goes to stderr, so stdout holds only records:

```bash
toven --output jsonl affected test --base origin/main --merge-base
```

```json
{"module":"rust:toven-cli","workspace":"rust","path":"crates/toven-cli"}
```

## Explain execution units

```text
toven explain <task> [SELECTION_OPTIONS]
```

```bash
toven explain test --workspace rust
toven explain test --module rust:toven-cli
```

Example stdout:

```text
unit:  rust@rust#test
  representative:  rust:toven-model
         modules:  rust:toven-model, rust:toven-ports, rust:toven-core, rust:toven-release, rust:toven-engine, rust:toven-rust, rust:toven-go, rust:toven-command, rust:toven-cli, rust:toven-testkit, rust:toven, rust:toven-rs, rust:toven-go-app
          target:  rust:toven-cli
            task:  test
            argv:  ["cargo", "nextest", "run", "--no-tests=pass", "--manifest-path", "crates/toven-model/Cargo.toml", "-p", "toven-model", "-p", "toven-ports", "-p", "toven-core", "-p", "toven-release", "-p", "toven-engine", "-p", "toven-rust", "-p", "toven-go", "-p", "toven-command", "-p", "toven-cli", "-p", "toven-testkit", "-p", "toven", "-p", "toven-rs", "-p", "toven-go-app"]
      persistent:  false
```

Focused explanations show the real execution unit containing the selected module, including modules batched with it.

## List modules

```bash
toven modules
toven modules --output jsonl
```

Example JSONL stdout:

```json
{"module":"rust:toven-cli","workspace":"rust","path":"crates/toven-cli"}
{"module":"rust:toven-engine","workspace":"rust","path":"crates/toven-engine"}
```

`path` is the module root relative to the directory that holds `toven.toml`, so it already includes `[project].root`. A module at that directory is `.`.

Aliases: `toven list`, `toven ls`.

## List groups

```bash
toven groups
toven groups --output jsonl
```

`toven groups` lists each declared [`[groups.<name>]`](../config/README.md#groups-and-overrides) with its description, the groups it may depend on (`null` in JSON when unrestricted), and its members. The human view is a summary table followed by one members line per group. Group guardrails are checked first, so a layering violation fails here too. Scripts that need a domain list (module indexes, per-domain checks) should read this instead of keeping their own copy.

Example JSONL stdout, one record per group:

```json
{"group":"apps","description":"Shipped binaries","allow_groups":["foundation"],"members":[{"module":"rust:app","workspace":"rust","path":"crates/app"}]}
```

Members use the same record shape as `toven modules --output jsonl`.

## Render the dependency graph

```bash
toven graph
toven graph --format dot
```

Text and DOT output use stdout. Alias: `toven deps`.

## List tasks

```bash
toven tasks
toven tasks test
toven tasks --output jsonl
```

Without an argument, `tasks` lists resolved tasks by ecosystem. With a task name, it shows the argv template, fan-out policy, persistence, and cache inputs.

## Generate shell completions

```bash
toven completions zsh > _toven
source <(toven completions bash)
```

Completion scripts support `bash`, `zsh`, `fish`, `powershell`, and `elvish`. The script is written to stdout. Repository-defined task names are not embedded; use `toven tasks` to discover them.
