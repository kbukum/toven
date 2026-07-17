# Scenarios

Diagrams of the core runtime flows, each inspectable with `plan`, `affected`, `explain`, and JSONL output.

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

A full run uses the module graph. Toven skips valid cache hits and keeps dependency order for everything that must run.

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

Affected mode starts with changed files, maps them to owning modules, and adds dependent modules so downstream breakage is caught. `toven affected <task>` lists the module set; `toven plan <task> -v` shows the units and cache verdicts; `toven explain <task> --module <sel>` shows one unit's argv, dependencies, and persistence.

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

Modules in the same wave have no pending dependency between them. A `batchable` task runs a wave as one command, splitting only when a Cargo manifest boundary makes one command unsafe.

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

Task-level `shared_inputs` invalidate every module using the task — `Cargo.lock`, `rust-toolchain.toml`, lint config, CI config. Write plain workspace paths (`Cargo.lock`, not `./Cargo.lock`). See [what invalidates cache](commands/cache.md#what-invalidates-cache).

## Installed-binary dry run

```mermaid
flowchart TD
    Install[Install toven binary] --> Init[toven init in target repo]
    Init --> Review[Review generated toven.toml]
    Review --> Plan[toven plan / affected]
    Plan --> Run[toven test / check]
    Run --> Bench[Benchmark against direct commands]
```

Adoption uses the installed binary and a generated config. Start with `toven init`, then add hand-written policy only where the real workflow needs it. See [benchmarking](benchmarking.md).
