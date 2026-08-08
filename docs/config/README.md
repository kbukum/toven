# Configuration guide

Toven reads one strict `toven.toml`. Unknown keys fail fast, so typos do not change a plan silently.

Start with the smallest file that names the project and enables an ecosystem:

```toml
[project]
name = "example"

[ecosystems.rust]
manifests = "auto"
```

Then check what Toven sees:

```bash
toven modules
toven tasks
toven plan check
```

Generate a starter file with [`toven init`](../commands/init.md).

## Top-level sections

| Section | Required? | Purpose |
|---|---:|---|
| `[project]` | Yes | Repository identity, root, and affected baseline |
| `[toven]` | No | Output, concurrency, live view, includes, cache, drivers, and git auth |
| `[ecosystems.<id>]` | No | Discovery, run strategy, tasks, coverage, and release policy for one adapter |
| `[modules."<ecosystem>:<name>"]` | No | Per-module release and coverage overrides |
| `[groups.<name>]` | No | Named module sets, guardrails, run strategy, and sparse task overrides |
| `[[overlays]]` | No | Explicit dependency edges native metadata cannot express |
| `[[members]]` | No | Federated repository members |

## Project

```toml
[project]
name = "example"
root = "."
base_ref = "origin/main"
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | Required | Human-facing project name |
| `root` | string | `"."` | Workspace root, resolved relative to `toven.toml` |
| `base_ref` | string | None | Git ref used when affected commands do not receive `--base` |

## Runtime settings

```toml
[toven]
report = "human"
max_parallel = 8
view = "auto"
include = ["ci/shared-tasks.toml"]

[toven.cache]
dir = ".toven/cache"

[toven.git]
push_token_env = ["GITHUB_TOKEN", "GH_TOKEN"]

[toven.drivers]
go = { version = "0.4.1" }
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `report` | `"human"` or `"json"` | `"human"` | Default report format; the CLI override is `--output human|jsonl` |
| `max_parallel` | integer | Engine default | Global concurrency ceiling |
| `view` | `"auto"`, `"tiles"`, `"panes"`, or `"stream"` | `"auto"` | Live per-unit output shape for interactive runs |
| `include` | string list | `[]` | Committed TOML files merged beneath the canonical file as defaults |
| `drivers` | table | `{}` | Out-of-process driver settings kept for federation |
| `cache.dir` | string | Engine default, with `TOVEN_CACHE_DIR` override | Task-cache root |
| `git.push_token_env` | string list | `["GITHUB_TOKEN", "GH_TOKEN"]` | Environment variables checked, in order, for git push/fetch auth |

Included files provide defaults. The canonical `toven.toml` wins on scalar and table conflicts, and included files must be committed.

`[toven.git].push_token_env` is forge-agnostic. The embedded git backend uses the first present, non-empty value as the HTTPS token password for engine-owned git network operations, including release pushes and planning fetches. If none are set, local development falls back to the ambient git transport.

## Ecosystem discovery

### Rust

```toml
[ecosystems.rust]
manifests = "auto"
exclude = ["fuzz"]
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `manifests` | `"auto"` or string list | `"auto"` | Cargo workspace roots to manage |
| `exclude` | string list | `[]` | Workspace directories or manifest paths skipped only when `manifests = "auto"` |

`"auto"` discovers the root Cargo workspace or first-level workspace roots during each plan. Use an explicit list to freeze the managed set:

```toml
[ecosystems.rust]
manifests = ["Cargo.toml", "tools/Cargo.toml"]
```

### Go

```toml
[ecosystems.go]
modules = "auto"
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `modules` | `"auto"` or string list | `"auto"` | `go.mod` modules to manage |

With `go.work`, `"auto"` uses its members. Without `go.work`, Toven discovers the root `go.mod` plus first-level nested modules. Use an explicit list to freeze the set:

```toml
[ecosystems.go]
modules = ["go.mod", "auth/go.mod"]
```

### Command

```toml
[ecosystems.command]

[[ecosystems.command.modules]]
name = "site"
root = "site"
manifest = "site/package.json"
depends_on = ["api"]

[[ecosystems.command.modules]]
name = "api"
root = "api"

[ecosystems.command.toolchain]
program = "npm"
args = ["--version"]
label = "npm"
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `modules` | array of tables | `[]` | Declared modules; the command adapter does not probe modules |
| `modules[].name` | string | Required | Module name, unique within `command` |
| `modules[].root` | string | Required | Repo-relative module root |
| `modules[].manifest` | string | None | Informational manifest path |
| `modules[].depends_on` | string list | `[]` | Other declared command modules this module depends on |
| `toolchain.program` | string | Derived from the first task | Program used for toolchain probing |
| `toolchain.args` | string list | `[]` | Probe arguments |
| `toolchain.label` | string | `program` | Human-readable diagnostic label |

## Common ecosystem keys

Every ecosystem also accepts these shared keys:

| Key | Type | Default | Meaning |
|---|---|---|---|
| `run_strategy` | `"leaf-to-top"` or `"unordered"` | Adapter default | Wave ordering for the ecosystem |
| `tasks` | table | `{}` | Complete authored task table |
| `coverage` | table | See [coverage configuration](../commands/coverage.md#configure-thresholds) | Coverage policy |
| `release` | table | See [release configuration](release.md) | Release policy |

## Tasks

Task names are user-owned identities. `kind` is optional; if omitted, Toven derives a recognized kind from the task name when it can.

```toml
[ecosystems.rust.tasks.test]
kind = "test"
argv = ["cargo", "nextest", "run", "--manifest-path", "{module.manifest}", "{module.selector}", "{args}"]
selector = ["-p", "{module.package}"]
fan_out = "batchable"
shared_inputs = ["Cargo.lock", "rust-toolchain.toml"]
cache_args = true
cacheable = true
fail_if_output = false
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `kind` | task kind | Derived from the task name, else default | Recognition attribute |
| `argv` | string list | Required | Exact argument-vector template |
| `selector` | string list | `[]` | Module selector inserted at `{module.selector}` |
| `fan_out` | `"per-module"`, `"batchable"`, or `"whole-workspace"` | `"per-module"` | Maximum batching shape the task supports |
| `persistent` | boolean | `false` | Keep the process alive after readiness |
| `readiness` | readiness enum | `"started"` | Signal for persistent tasks |
| `readiness_timeout_secs` | integer | `30` | Readiness timeout for persistent tasks |
| `cacheable` | boolean | `true` | Allow successful result reuse |
| `cache_args` | boolean | `false` | Include passthrough args in cache keys |
| `shared_inputs` | string list | `[]` | Repository files that invalidate every matching unit |
| `fail_if_output` | boolean | `false` | Treat any stdout output as failure |

Toven keeps authored argv unchanged. Templates expand selectors and Toven variables, but they do not infer hidden flags.

## Groups and overrides

```toml
[groups.integration]
ecosystem = "rust"
modules = ["rust:toven-cli"]
run_strategy = "unordered"

[groups.integration.tasks.test]
argv = ["cargo", "nextest", "run", "--profile", "ci"]

[groups.integration.guardrails]
forbid = ["rust:toven-engine"]
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `ecosystem` | string | None | Default ecosystem for bare module names |
| `modules` | string list | `[]` | Group members, bare names or `ecosystem:module` refs |
| `run_strategy` | `"leaf-to-top"` or `"unordered"` | Inherit | Group-scoped wave-ordering override |
| `tasks` | table | `{}` | Sparse task overrides |
| `guardrails.forbid` | string list | `[]` | Fully qualified edges the group must not depend on |
| `guardrails.allow` | string list | `[]` | Optional fully qualified edge allowlist |

Group task entries are sparse overrides over existing ecosystem tasks. Scalars and lists replace the base value, except `shared_inputs`, which is additive. If two groups override the same task or run strategy for one module, config resolution fails.

## Module overrides

Per-module blocks override release and coverage settings for one discovered module:

```toml
[modules."rust:toven-cli".release]
publish = false

[modules."rust:toven-cli".coverage]
line = 90.0
```

Release overrides merge field by field over `[ecosystems.<id>.release]`. Coverage overrides follow the same pattern.

## Overlays

Use overlays when native metadata cannot express a dependency:

```toml
[[overlays]]
from = { ecosystem = "rust", module = "cli" }
to = { ecosystem = "go", module = "api" }
```

`from` is the dependent module, and `to` is the module it depends on. The edge participates in selection, scheduling, and release cascades.

## Federation

An umbrella configuration can compose independently configured repositories:

```toml
[[members]]
name = "service"
root = "services/service"
base_ref = "origin/main"
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | Required | Unique member name |
| `root` | string | Required | Member repository root, relative to the umbrella config |
| `base_ref` | string | Member default | Per-member change baseline |

Each member keeps its own `toven.toml`. The umbrella composes their modules into one graph.

## Coverage and release

- [Coverage command and configuration](../commands/coverage.md#configure-thresholds)
- [Release configuration](release.md)

## Validation

Run read-only commands after editing config:

```bash
toven modules
toven tasks
toven plan check
```

Configuration errors go to stderr and return a non-zero exit code.
