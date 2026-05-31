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
flowchart TD
    Config[toven.toml] --> Normalize[Strict config normalization]
    Normalize --> Presets[Resolve presets and task defaults]
    Presets --> Discover[Adapter discovery]
    Discover --> Graph[Validate module graph]
    Graph --> Schedule[Build readiness waves]
    Schedule --> Units[Plan execution units]
    Units --> Render[Render argv and resource groups]
    Render --> Execute[Execute or report]
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
    Modules[Discovered modules] --> Dependencies[Dependency edges]
    Dependencies --> Waves[Topological readiness waves]
    Waves --> Mode{Execution mode}
    Mode -->|batch-ready| ManifestGroups[Split each wave by manifest]
    Mode -->|per-module| SingleModule[One module per execution unit]
    ManifestGroups --> Units[Execution units]
    SingleModule --> Units
    Units --> Rendered[Rendered argv + resource group]
```

Readiness waves preserve dependency order: a module can run only after its
dependencies have completed or been skipped as valid cache hits. `batch-ready`
keeps every ready module in the same wave together when that is safe, then
splits by Cargo manifest so workspace manifests do not receive selectors from a
different manifest root.

## Affected and cache decision flow

```mermaid
flowchart TD
    Changed[Changed files from baseline] --> Owners[Owning modules]
    Owners --> Dependents[Reverse dependency closure]
    Dependents --> Filter[Filtered plan]
    Filter --> Inputs[Hash module, deps, task, shared inputs]
    Inputs --> Args{Passthrough args?}
    Args -->|yes and cache_args=false| Disabled[Cache disabled]
    Args -->|no or cache_args=true| Lookup[Lookup cache record]
    Lookup -->|match| Hit[Skip as cache hit]
    Lookup -->|missing/mismatch| Miss[Run unit]
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
