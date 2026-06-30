# Toven scenarios

This document describes the core runtime scenarios the CLI makes easy to inspect with `plan`, `affected`, `explain`, human output, and JSONL output.

## Full task run

```mermaid
flowchart LR
    Dev["developer"] --> Cmd["toven test"]
    Cmd --> Load["load config"]
    Load --> Discover["discover modules"]
    Discover --> Plan["build waves"]
    Plan --> Cache["cache decisions"]
    Cache --> Run["run misses"]
    Cache --> Skip["skip hits"]
    Run --> Report["human / JSONL"]
    Skip --> Report
```

A full run still uses the module graph. Toven skips modules that are valid cache hits, but it keeps dependency order for everything that must run.

## Affected run

```mermaid
flowchart TD
    Change["file changed in crate-a"] --> Direct["crate-a is directly affected"]
    Direct --> Downstream["crate-b depends on crate-a"]
    Downstream --> AlsoRun["crate-b is affected too"]
    Direct --> Plan["plan affected modules only"]
    AlsoRun --> Plan
    Plan --> Explain["inspect plan and per-unit cache verdicts"]
```

Affected mode starts with changed files, maps them to owning modules, and adds dependent modules so downstream breakage is not missed. `toven affected <task>` lists the resulting module set for a baseline, and `toven plan <task>` (with `-v`) shows the planned units and their cache verdicts. `toven explain <ecosystem:module> <task>` is a planned-unit view: it prints the unit's argv, dependencies, and persistence for one module and task.

## Wave bundling

```mermaid
flowchart TD
    subgraph Graph["dependency graph"]
      A["crate-a"] --> B["crate-b"]
      A --> C["crate-c"]
      B --> D["crate-d"]
      C --> D
    end

    subgraph Waves["ready waves"]
      W1["wave 1: crate-a"] --> W2["wave 2: crate-b + crate-c"]
      W2 --> W3["wave 3: crate-d"]
    end
```

Modules in the same wave have no pending dependency between them. In `batchable`, Toven tries to run a wave as one command, then splits only when a manifest boundary makes one command unsafe.

## Shared-input invalidation

```mermaid
flowchart LR
    Shared["Cargo.lock / toolchain / lint config"] --> SharedHash["shared hash"]
    Module["module files"] --> ModuleHash["module hash"]
    Deps["dependency results"] --> DepHash["dependency hash"]
    SharedHash --> Key["cache key"]
    ModuleHash --> Key
    DepHash --> Key
    Key --> Decision{"same as last success?"}
    Decision -->|"yes"| Skip["skip"]
    Decision -->|"no"| Run["run"]
```

Use task-level `shared_inputs` for files and directories that can invalidate all modules using the task, such as `Cargo.lock`, `rust-toolchain.toml`, deny/lint configuration, and CI configuration. Write them as plain paths inside the workspace, for example `Cargo.lock` instead of `./Cargo.lock`; templates, globs, `.` components, parent paths, and absolute paths are rejected.

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

The rskit adoption path should use the installed binary and generated instructions. Repository-specific config should come from `toven generate` first and only add hand-written policy where the real workflow needs it. The rskit benchmark case intentionally requires that adopted `toven.toml` before it runs.
