# Toven architecture

Toven is one Rust product crate with internal modules. Domain types stay in
`core`; higher layers consume normalized data instead of parsing config or
reaching upward into CLI code.

## Module layout

```text
src/
  core/        # identifiers, models, protocols, errors, templates
  config/      # strict TOML loading and normalization
  adapter/     # language/package discovery implementations
    rust/
      cargo/   # Cargo metadata loading and normalization
      generate/# Rust config generation contributor
  preset/      # preset resolution
  git/         # baseline and changed-file detection
  cache/       # successful-run cache storage and decisions
  engine/      # graph validation, scheduling, planning
  exec/        # command rendering and process execution
  generate/    # user-facing config generation workflow
  report/      # human/JSON/JSONL reporting
  cli/         # clap parsing and process IO
```

## Layering rules

```text
L0  core/
L1  config/, adapter/, preset/, git/, cache/
L2  engine/, exec/, generate/
L3  report/
L4  cli/
```

`mod.rs` files are declaration/re-export roots only. CI enforces this with
`make structure`.

Key import boundaries:

- `core/` has no upward imports.
- `config/`, `adapter/`, and `preset/` do not import `engine/`, `exec/`,
  `report/`, or `cli/`.
- `engine/` receives normalized workspace/modules/tasks as data.
- `exec/` renders and runs planned execution units; it does not parse config.
- `generate/` orchestrates config generation and calls adapter-owned generation
  contributors.
- `cli/` is the only layer that handles process stdio and human command parsing.

## Config and discovery flow

```mermaid
flowchart LR
    subgraph Input
        Config["toven.toml"]
        Cli["CLI flags"]
    end

    subgraph Normalize["Normalize project intent"]
        Strict["Strict TOML validation"]
        Presets["Resolve presets and task defaults"]
    end

    subgraph Discover["Discover work"]
        Adapter["Adapter discovery"]
        Graph["Module dependency graph"]
    end

    subgraph Plan["Plan execution"]
        Waves["Readiness waves"]
        Units["Execution units"]
        Render["Rendered argv"]
    end

    Config --> Strict
    Cli --> Strict
    Strict --> Presets
    Presets --> Adapter
    Adapter --> Graph
    Graph --> Waves
    Waves --> Units
    Units --> Render
    Render --> Output["Run or report"]
```

Rust discovery is Cargo-metadata backed. Profile-level `discovery.manifests`
allows multi-manifest repositories, and Cargo path dependencies are inferred
across configured manifests.

Explicit `[[overlays]]` are top-level dependency edges for relationships that
adapter metadata cannot prove.

## Config generation flow

```mermaid
flowchart TD
    Generate[toven generate] --> Workflow[Generic generate workflow]
    Workflow --> Contributors[Adapter contributors]
    Contributors --> Fragments[Structured config fragments]
    Fragments --> RenderToml[Deterministic TOML renderer]
    RenderToml --> Preview[stdout preview]
    RenderToml --> Write[Safe root/toven.toml write]
```

Existing configs are never replaced by default. Writes use safe temporary-file
creation and an explicit overwrite path.

## Planning, waves, and bundling

```mermaid
flowchart TD
    Graph["Module graph"] --> W1["Wave 1: roots with no pending deps"]
    W1 --> W2["Wave 2: modules unblocked by Wave 1"]
    W2 --> W3["Wave 3: downstream modules"]

    W2 --> Mode{"execution mode"}
    Mode -->|"per-module"| PerModule["one execution unit per module"]
    Mode -->|"batch-ready"| Batch["bundle ready modules"]
    Batch --> Manifest{"same Cargo manifest?"}
    Manifest -->|"yes"| OneUnit["one batched unit"]
    Manifest -->|"no"| Split["split by manifest root"]
    PerModule --> Rendered["render argv + resource group"]
    OneUnit --> Rendered
    Split --> Rendered
```

Think of a wave as “everything that is safe to start now.” A module joins a
later wave when one of its dependencies must finish first. `batch-ready` keeps
ready modules together when the command can handle them together, but it still
splits by Cargo manifest so selectors are never sent to the wrong workspace.

## Affected and cache decision flow

```mermaid
flowchart TD
    Base["base ref"] --> Diff["changed files"]
    Worktree["HEAD / worktree"] --> Diff
    Diff --> Owners["modules that own changed files"]
    Owners --> Closure["dependent modules"]
    Closure --> Plan["affected execution plan"]

    Plan --> Inputs["module + dependency + task + shared inputs"]
    Inputs --> Args{"passthrough args?"}
    Args -->|"yes, cache_args=false"| Disabled["cache disabled"]
    Args -->|"no, or cache_args=true"| Lookup["cache lookup"]
    Lookup -->|"record matches"| Hit["skip"]
    Lookup -->|"missing or changed"| Miss["run"]
    Disabled --> Miss
```

`shared_inputs` are task-owned, workspace-relative paths that participate in the
shared hash for every module in the task. They are for broad invalidators such
as lockfiles, toolchain files, lint config, and CI-relevant config.

## Extension points

- New language/package-manager adapters should live under `src/adapter/<name>/`.
- Adapter-specific config generation should live under
  `src/adapter/<name>/generate/`.
- Generic generation orchestration belongs under `src/generate/`.
- Shared foundational capabilities should be improved in rskit generically when
  Toven exposes a reusable framework gap.
