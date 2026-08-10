# Inspect plans and graphs

Start by listing modules:

```bash
toven modules
```

Inspection commands are read-only. Use them before executing unfamiliar configuration. There is no `toven inspect` command; this page groups `plan`, `affected`, `explain`, `modules`, `graph`, and `tasks`.

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
{"module":"rust:toven-cli","workspace":"rust"}
{"module":"rust:toven-engine","workspace":"rust"}
```

Aliases: `toven list`, `toven ls`.

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
