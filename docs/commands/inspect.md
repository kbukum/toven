# Inspect plans and graphs

Inspection commands are read-only. Use them before executing unfamiliar configuration.

## Plan a task

```text
toven plan <task> [SELECTION_OPTIONS]
```

```bash
toven plan check
toven plan check --base origin/main --merge-base
toven plan check --module rust:core --dependents
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
toven affected test --base origin/main --merge-base
```

Example stdout:

```text
Module
rust:core
rust:cli
```

Repository-level changes that cannot be assigned to one module activate the full scope and include an explanatory diagnostic.

## Explain execution units

```text
toven explain <task> [SELECTION_OPTIONS]
```

```bash
toven explain test
toven explain test --module rust:core
```

Example stdout:

```text
unit: rust:test:core
modules: rust:core
task: test
argv: cargo nextest run -p core
persistent: false
```

Focused explanations show the real execution unit containing the selected module, including co-batched modules.

## List modules

```bash
toven modules
toven modules --output jsonl
```

Example JSONL stdout:

```json
{"module":"rust:core","workspace":"rust"}
{"module":"rust:cli","workspace":"rust"}
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

Without an argument, the command lists resolved tasks by ecosystem. With a task name, it shows the argv template, fan-out policy, persistence, and cache inputs.

## Generate shell completions

```text
toven completions <bash|zsh|fish|powershell|elvish>
```

```bash
toven completions zsh > _toven
source <(toven completions bash)
```

The completion script is written to stdout. Repository-defined task names are not embedded; use `toven tasks` to discover them.
