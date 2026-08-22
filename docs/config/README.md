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
| `[hooks.<verb>]` | No | Project-level `pre`/`post` lifecycle hooks that wrap a command |
| `[units.<name>]` | No | User-declared composite units that chain existing units into one action |
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
compute_budget = "auto"
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
| `compute_budget` | `"auto"`, `"inherit"`, or integer | `"auto"` | CPU parallelism handed to each spawned tool (see [Compute budget](#compute-budget)) |
| `view` | `"auto"`, `"tiles"`, `"panes"`, or `"stream"` | `"auto"` | Live per-unit output shape for interactive runs |
| `include` | string list | `[]` | Committed TOML files merged beneath the canonical file as defaults |
| `drivers` | table | `{}` | Out-of-process driver settings kept for federation |
| `cache.dir` | string | Engine default, with `TOVEN_CACHE_DIR` override | Task-cache root |
| `git.push_token_env` | string list | `["GITHUB_TOKEN", "GH_TOKEN"]` | Environment variables checked, in order, for git push/fetch auth |

Included files provide defaults. The canonical `toven.toml` wins on scalar and table conflicts, and included files must be committed.

`[toven.git].push_token_env` is forge-agnostic. The embedded git backend uses the first present, non-empty value as the HTTPS token password for engine-owned git network operations, including release pushes and planning fetches. If none are set, local development falls back to the ambient git transport.

## Compute budget

`max_parallel` bounds how many units run at once; `compute_budget` bounds how much CPU parallelism each of those units gets *internally*. They solve different halves of the same problem. A per-module task (Go's `go test ./...` per module) fans out into one child process per module, and the worker pool runs several of those children at once. Left alone, each child also defaults its own internal parallelism to the whole machine, so peak thread pressure climbs toward cores² and the machine thrashes instead of getting faster.

`compute_budget` caps that. The engine resolves a total thread budget, divides it across the units running concurrently in a wave, and hands each fanned-out tool its share through an ecosystem environment variable — never through argv, so your commands are never rewritten. It therefore only affects ecosystems that expose a supported variable: Go reads `GOMAXPROCS`. A self-balancing single-invocation toolchain such as Cargo (one `cargo` build parallelizes internally) registers no such variable and is left entirely unchanged — it runs with its own default parallelism regardless of the budget.

| Value | Meaning |
|---|---|
| `"auto"` (default) | Size the total budget to the host's available CPUs, then split it across the wave |
| integer (`8`) | Use a fixed total thread budget, split across the wave |
| `"inherit"` or `0` | Inject nothing; every tool keeps its own default parallelism |

The per-process share is `clamp(ceil(budget / concurrent), min(2, budget), budget)`: ceiling division so every concurrent unit gets a whole-thread share, a floor of `2` so a saturated wave never starves a child down to a single thread, and a ceiling of the whole budget so a lone unit is never handed more than exists. The floor is itself capped at the budget (`min(2, budget)`) so the accepted value `compute_budget = 1` stays valid and resolves to a single thread rather than an impossible range.

The budget is a soft per-unit allocation target, not a hard cap on the wave total: rounding each share up (and applying the floor) can push the summed allocation above the nominal budget — three concurrent units against a budget of `10` get `ceil(10 / 3) = 4` threads each (`12` total), and four units against a budget of `4` are floored to `2` each (`8` total). It bounds what any single unit receives, not what the wave spends in aggregate.

The budget is expressed once on `[toven]` and may be overridden per ecosystem. An `[ecosystems.<id>].compute_budget` override wins over the global value, so a polyglot repo can bound its Go fan-out while leaving another ecosystem on `auto` or opting it out entirely:

```toml
[toven]
compute_budget = "auto"

[ecosystems.go]
compute_budget = 12
```

The `--compute-budget <auto|inherit|N>` CLI flag overrides both for a single run. See [run options](../commands/run.md#compute-budget).

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

## Lifecycle hooks

`[hooks.<verb>]` attaches project-level `pre`/`post` task references to a Toven command, so a workspace can wrap a verb with checks and follow-ups without changing argv. The verb key is a fixed, validated name — an unknown key fails config validation.

```toml
[hooks.run]
pre = ["fmt-check"]

[hooks.release]
pre = ["fmt-check", "lint"]
post = ["notify-release"]

[hooks.bump]
pre = ["validate"]
```

| Verb key | Wraps |
|---|---|
| `run` | `toven run <task>` (and the bare argv-first task form) |
| `plan` | `toven plan <task>` |
| `coverage` | `toven coverage` |
| `doctor` | `toven doctor` |
| `release` | Every release mutation — the umbrella around `bump`/`tag`/`publish` |
| `bump` | `toven release bump` |
| `tag` | `toven release tag` |
| `publish` | `toven release publish` |

Each block takes two optional string lists:

| Key | Type | Default | Meaning |
|---|---|---|---|
| `pre` | string list | `[]` | Task references run before the verb, fail-closed |
| `post` | string list | `[]` | Task references run after the verb succeeds |

Hooks are ordinary task references, resolved through the same task model as [`toven run`](../commands/run.md) and executed argv-first with no implicit shell. A `pre` hook runs before the command does any work; a non-zero `pre` hook aborts the command before any mutation. A `post` hook runs only after the command succeeds.

The release family composes with its umbrella: `[hooks.release]` wraps `[hooks.bump]`, `[hooks.tag]`, and `[hooks.publish]`, with the specific verb innermost. For a `release bump`, the effective `pre` sequence is the umbrella's `pre` followed by `bump`'s `pre`, and the effective `post` sequence is `bump`'s `post` followed by the umbrella's `post`. A `release` reconcile that short-circuits (nothing to cut) skips the `post` hooks.

For the bump-specific mid-mutation `on_resolved` seam — which runs *inside* the bump after versions are decided and is handed the resolved version map — see [Release configuration](release.md#bump-on-resolved-hooks).

## Composite units

`[units.<name>]` declares a composite unit: an ordered chain that composes existing units into one named action, so a workspace can express a release-like flow without changing argv.

```toml
[units.release]
chain = ["bump", "tag", "publish"]

[units.ship]
chain = ["release", "coverage"]
```

Each `chain` entry names another unit, in declaration order. A member is either a built-in native capability (`bump`, `tag`, `publish`, `coverage`) or another declared composite — a composite may build on top of another. The chain is an ordered list, not a set: a member listed more than once is kept once per occurrence rather than de-duplicated.

Composite declarations are parsed and validated at load time and fail closed on a malformed chain. A member that names no known unit is rejected as an unknown unit; a name that shadows a built-in unit, an empty chain, or a blank member is rejected; and a chain that references itself directly or transitively is rejected as a cycle.

> `[units.*]` is declaration-and-validation-only. Declared composite chains are not invocable and do not change what any command runs.

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
