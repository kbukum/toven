# Toven scenarios

This document describes the core runtime scenarios the CLI should make easy to
inspect with `plan`, `affected`, `explain`, JSON, and JSONL output.

## Full task run

```mermaid
sequenceDiagram
    participant Dev as Developer
    participant CLI as toven CLI
    participant Engine as Planner
    participant Exec as Executor
    participant Cache as Cache store

    Dev->>CLI: toven test
    CLI->>Engine: load config, discover modules, plan task
    Engine->>Cache: ask for per-module decisions
    Cache-->>Engine: hit / miss / disabled
    Engine->>Exec: execution units in dependency waves
    Exec-->>CLI: run events and exit status
    CLI-->>Dev: human, JSON, or JSONL report
```

Full runs discover every configured module for the selected task. Cache hits can
skip individual modules, but dependency order still shapes the execution waves.

## Affected run

```mermaid
flowchart TD
    Base[Base ref] --> Diff[Changed files]
    Head[Working tree / head] --> Diff
    Diff --> Direct[Directly changed modules]
    Direct --> Closure[Dependents that must be rechecked]
    Closure --> Plan[Plan only affected modules]
    Plan --> Cache[Apply cache decisions]
    Cache --> Output[Explain why each module ran or skipped]
```

Affected mode starts with changed files, maps them to owning modules, and adds
dependent modules so downstream breakage is not missed. `toven explain` should
show which baseline, files, dependency edges, and cache inputs contributed to a
module decision.

## Wave bundling

```mermaid
flowchart LR
    A[crate-a] --> B[crate-b]
    A --> C[crate-c]
    B --> D[crate-d]
    C --> D

    subgraph Wave 1
      A
    end
    subgraph Wave 2
      B
      C
    end
    subgraph Wave 3
      D
    end
```

Modules in the same wave are ready at the same time. For `batch-ready`, Toven
bundles ready modules into execution units, then splits by manifest when a wave
contains modules discovered from different manifests.

## Shared-input invalidation

```mermaid
flowchart TD
    Task[Task definition] --> Shared[shared_inputs]
    Shared --> Hash[Shared input hash]
    Module[Module source hash] --> Key[Cache key components]
    Deps[Dependency hashes] --> Key
    Hash --> Key
    Key --> Decision{Cache record matches?}
    Decision -->|yes| Skip[Skip module]
    Decision -->|no| Run[Run module]
```

Use task-level `shared_inputs` for files and directories that can invalidate all
modules using the task, such as `Cargo.lock`, `rust-toolchain.toml`, deny/lint
configuration, and CI configuration.

## Installed-binary rehearsal

```mermaid
flowchart TD
    Install[Install local toven binary] --> Generate[toven generate in target repo]
    Generate --> Review[Review generated toven.toml]
    Review --> Plan[toven plan / affected]
    Plan --> Run[toven test/check/nextest]
    Run --> Bench[Benchmark installed binary against direct commands]
    Bench --> Fix[Record Toven or generic rskit gaps]
```

The rskit adoption path should use the installed binary and generated
instructions. Repository-specific config should come from `toven generate` first
and only add hand-written policy where the real workflow needs it.
