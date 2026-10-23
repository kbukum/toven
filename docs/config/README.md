# `toven.toml` configuration

Define project, ecosystem, and release settings in one strict `toven.toml`. Unknown keys fail fast, so typos do not change a plan silently.

## Quickstart

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
| `[sets.<name>]` | No | Named task lists that run together, such as `toven verify` |
| `[doctor.checks.<id>]` | No | Project health checks that `toven doctor` runs; see [doctor](../commands/doctor.md#project-checks) |
| `[units.<name>]` | No | User-declared composite units that chain existing units into one action |
| `[[overlays]]` | No | Explicit dependency edges native metadata cannot express |
| `[[members]]` | No | Federated repository members |

## Project

```toml
[project]
name = "example"
root = "."
base_ref = "origin/main"
global_inputs = [".github/workflows/*", "Makefile", "toven.toml"]
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | Required | Human-facing project name |
| `root` | string | `"."` | Workspace root, resolved relative to `toven.toml` |
| `base_ref` | string | None | Git ref used when affected commands do not receive `--base` |
| `global_inputs` | array of globs | `[]` | Repo-wide files, relative to `root`. A change to one runs every module in affected selection. `*` and `?` match across `/`. In an umbrella, the umbrella's patterns are relative to the umbrella root and activate every member; each member's own patterns are relative to that member. Release ignores them |

Changed files belong to the modules whose root is their longest prefix; modules from different ecosystems that share a root all own the file. Files under a root module (`root = "."`) such as CI workflows or build scripts would otherwise only select that module and its dependents, so list them in `global_inputs`.

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
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `report` | `"human"` or `"json"` | `"human"` | Default report format; `json` emits JSON Lines, and the CLI override is `--output human|jsonl` |
| `max_parallel` | integer | Engine default | Global concurrency ceiling |
| `compute_budget` | `"auto"`, `"inherit"`, or integer | `"auto"` | CPU parallelism handed to each spawned tool (see [Compute budget](#compute-budget)) |
| `view` | `"auto"`, `"tiles"`, `"panes"`, or `"stream"` | `"auto"` | Live per-unit output shape for interactive runs |
| `include` | string list | `[]` | Safe relative TOML file paths merged beneath the canonical file as defaults |
| `drivers` | table | `{}` | Raw out-of-process driver settings kept for federation |
| `cache.dir` | string | Engine default, with `TOVEN_CACHE_DIR` override | Task-cache root |
| `git.push_token_env` | string list | `["GITHUB_TOKEN", "GH_TOKEN"]` | Environment variables checked, in order, for git push/fetch auth |

Included files provide defaults. The canonical `toven.toml` wins on scalar and table conflicts.

`[toven.git].push_token_env` is forge-agnostic. The embedded git backend uses the first present, non-empty value as the HTTPS token password for engine-owned git network operations, including release pushes and planning fetches. If none are set, local development falls back to the ambient git transport.

`[toven.drivers]` is intentionally untyped here. Toven preserves each driver subtree as raw TOML for federation and does not define a shared key set in the core loader.

## Compute budget

`max_parallel` bounds how many units run at once; `compute_budget` bounds how much CPU parallelism each of those units gets *internally*. They solve different halves of the same problem. A per-module task (Go's `go test ./...` per module) fans out into one child process per module, and the worker pool runs several of those children at once. Left alone, each child also defaults its own internal parallelism to the whole machine, so peak thread pressure climbs toward cores² and the machine thrashes instead of getting faster.

`compute_budget` caps that. The engine resolves a total thread budget, divides it across the units running concurrently in a wave, and hands each fanned-out tool its share through an ecosystem environment variable — never through argv, so your commands are never rewritten. It therefore only affects ecosystems that expose a supported variable: Go reads `GOMAXPROCS`. A self-balancing single-invocation toolchain such as Cargo (one `cargo` build parallelizes internally) registers no such variable and is left entirely unchanged — it runs with its own default parallelism regardless of the budget.

Injection never overrides a value you set yourself. If the same variable is already set explicitly for the run (through the invocation environment) or is inherited non-empty from the parent process (an exported `GOMAXPROCS`), that value wins and the computed share is not applied. So `auto` may inject nothing when the environment already pins the variable — the budget only fills in a share where you have not.

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

Toven reads `go.mod` and `go.work` with the `go` command and your environment, just like your tasks. It does not set `GOTOOLCHAIN`, so your value applies, or Go's default `auto`, which switches to the Go version your `go.mod` or `go.work` asks for (and may download it once). Each `go.mod` is read from its own directory, so that choice follows the `go` and `toolchain` lines Go would use there: the enclosing `go.work` when there is one, otherwise that module's `go.mod`. Set `GOTOOLCHAIN=local` if discovery must never download a toolchain. When the Go in use is too old to read your `go.mod`, the error says so and suggests a fix.

### JavaScript and TypeScript

```toml
[ecosystems.js]
roots = "auto"
exclude = ["examples"]

[ecosystems.js.package_manager]
"tools/legacy" = "npm"

[ecosystems.js.pack]
"." = ["bun", "scripts/pack.ts", "{module.root}", "--destination", "{destination}"]
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `roots` | `"auto"` or string list | `"auto"` | Root `package.json` files to manage |
| `exclude` | string list | `[]` | Repo-relative directories skipped by discovery |
| `package_manager` | table of root directory → `"npm"`, `"pnpm"`, `"yarn"`, or `"bun"` | `{}` | Per-root package manager override; the key is the root directory (`"."` for the repository root), and a key that is not a root is an error |
| `pack` | table of root directory → argv list | `{}` | Per-root release pack command that replaces the package manager's `pack`; see [release packing](release.md#registry-tag-only-and-excluded-modules) |

A root is a directory with its own install: one `package.json` plus its lockfile and workspace settings. One repository can hold several independent roots, each with its own package manager. `"auto"` walks the project, skipping `node_modules`, hidden directories, test-data directories (`testdata`, `fixtures`, `__fixtures__`), Git-ignored directories, and `exclude`. List a root explicitly to manage one inside a skipped directory. The repository-root `package.json` is a root. A nested `package.json` that a root's workspace patterns select is a member of that root. Any other nested `package.json` becomes its own root when it has a lockfile, a workspace definition, or a `packageManager` pin, and is ignored otherwise. A `package.json` outside every root is always a root. Use an explicit list to freeze the set; a listed root that another root also selects as a member is an error:

```toml
[ecosystems.js]
roots = ["web/package.json", "gen/ts/package.json"]
```

Workspace members come from the `workspaces` field (a list, or an object with `packages`) or, for pnpm, `pnpm-workspace.yaml`. Patterns support `*`, `**`, and `!` exclusions.

Each root's package manager comes from the `package_manager` override, then the `packageManager` pin, then `devEngines.packageManager`, then the one lockfile present. Yarn 1 and Yarn 2+ (Berry) are told apart by the pin, or by `.yarnrc.yml` when there is no pin. With no signal at all, `pnpm-workspace.yaml` selects pnpm, `.yarnrc.yml` selects Yarn, and otherwise npm is used. Disagreeing signals, such as a pin for pnpm next to `package-lock.json` or two lockfiles from different tools, fail with the fix instead of a guess.

Every member is a module, and so is the root package when it has a `name` or `scripts`, or is not a workspace. A module's name is its repo-relative directory with `/` replaced by `-` (`packages/std` → `packages-std`); the repository-root package uses its unscoped package name (`@acme/platform` → `platform`). `{module.package}` is the full npm name. Each module records a `script:<name>` capability per script, so a task can use `requires = ["script:test"]`. A workspace root's script that a member also declares (`"test": "turbo run test"`) is an orchestrator over the members, so it is not one of the root package's capabilities. Package names must be unique within a root.

A dependency becomes an edge when it resolves to a discovered package: `workspace:` by name, alias, or path; `file:`, `link:`, and `portal:` paths at a package root; and a plain range whose same-root package version satisfies it. pnpm resolves plain ranges from the registry unless `link-workspace-packages` is on, so for pnpm such an edge can be extra; it only adds ordering, never removes it. `catalog:`, `npm:` aliases, Git, and URL specs never link. `devDependencies` give dev edges; `dependencies`, `peerDependencies`, and `optionalDependencies` give normal edges. Path dependencies outside the module graph are cache inputs; see [cache](../commands/cache.md). Use [overlays](#overlays) for edges `package.json` cannot express, such as a tsconfig path alias.

`toven init` writes a task per script it finds, run through the detected package manager in each package that declares the script. It asks which scripts become tasks (standard names are preselected) and whether `build`, `test`, and `typecheck` build workspace dependencies first. For an npm workspace it writes, in part:

```toml
[ecosystems.js.tasks.build]
argv = ["npm", "--prefix", "{module.root}", "run", "build", "--", "{args}"]
depends_on = ["^build"]
fan_out = "per-module"
outputs = ["dist"]
requires = ["script:build"]

[ecosystems.js.tasks.typecheck]
argv = ["npm", "--prefix", "{module.root}", "run", "typecheck", "--", "{args}"]
depends_on = ["^build"]
fan_out = "per-module"
kind = "check"
requires = ["script:typecheck"]

[ecosystems.js.tasks.install]
argv = ["npm", "--prefix", "{workspace.root}", "ci"]
cacheable = false
fan_out = "whole-workspace"
workspace_closure = true
```

- `typecheck` is a `check` task, `format:check` becomes `format-check`, and `dev` is a persistent `run` task.
- `build` declares `outputs = ["dist"]` when a package with a `build` script points an entry point, export, executable, or published file into `dist`.
- `format`, `dev`, and any non-standard script you select are not cached, because Toven cannot tell that they are pure.
- `install` runs each root's frozen-lockfile install. Toven never installs on its own; run `toven install` or the install command `toven doctor` suggests.
- When every package's `test` script runs the same runner (Vitest, Jest, or `bun test`), `init` also writes an uncached `coverage` task that adds the runner's LCOV flags to the `test` script, plus an advisory `[ecosystems.js.coverage]` starter block. See [JavaScript profiles](../commands/coverage.md#javascript-profiles).
- For now, every root must use the same package manager for `init` to write the table, and each task runs once per package rather than batching packages into one call.

Discovery reads `package.json`, `pnpm-workspace.yaml`, and lockfile presence directly, with bounded reads. It needs no `node_modules`, no network, and no package manager binary, and it never runs a command. Each root's lockfile, `package.json`, `.npmrc`, and its manager's settings file (`pnpm-workspace.yaml`, `.yarnrc.yml`, or `bunfig.toml`) affect every package in that root. The toolchain identity is the Node.js version plus the package manager version, or the Bun version alone.

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
| `modules[].capabilities` | string list | `[]` | Capabilities this module provides, matched by a task's `requires` |
| `toolchain.program` | string | Each task's own program | Program used for toolchain probing. Without it, each task probes its `argv[0]` with `--version`, except a path such as `scripts/check.sh`, which is never run to probe |
| `toolchain.args` | string list | `[]` | Probe arguments |
| `toolchain.label` | string | `program` | Human-readable diagnostic label |

## Common ecosystem keys

Every ecosystem also accepts these shared keys:

| Key | Type | Default | Meaning |
|---|---|---|---|
| `run_strategy` | `"leaf-to-top"` or `"unordered"` | Adapter default | Wave ordering for the ecosystem |
| `compute_budget` | `"auto"`, `"inherit"`, or integer | Inherit from `[toven].compute_budget` | Ecosystem-specific compute-budget override |
| `tasks` | table | `{}` | Complete authored task table |
| `coverage` | table | See [coverage configuration](../commands/coverage.md#configure-thresholds) | Coverage policy |
| `release` | table | See [release configuration](release.md) | Release policy |

The adapter default `run_strategy` depends on the task kind. `format`, `lint` and `vuln` are unordered everywhere. Go and JavaScript also run `test` and `coverage` unordered. Rust runs every kind except `run` and plain tasks unordered, because one cargo invocation already builds its path dependencies in order. Everything else runs leaf-to-top.

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
| `workspace_closure` | boolean | `false` | Whether a `whole-workspace` invocation resolves its own cross-workspace dependency closure, so it may be co-scheduled inside an irreducible facade back-dependency cycle. Only honored with `fan_out = "whole-workspace"`; adapters set it on tool invocations that operate on the whole workspace atomically (`cargo … --workspace`, `go … ./...`). It stays off for arbitrary custom commands, so a cycle touching one keeps failing closed until its author opts in. |
| `persistent` | boolean | `false` | Keep the process alive after readiness |
| `readiness` | readiness enum | `"started"` | Signal for persistent tasks |
| `readiness_timeout_secs` | integer | `30` | Readiness timeout for persistent tasks |
| `cacheable` | boolean | `true` | Allow successful result reuse |
| `cache_args` | boolean | `false` | Include passthrough args in cache keys |
| `shared_inputs` | string list | `[]` | Repository files that invalidate every matching unit |
| `modules` | string list | `[]` (every module) | Module names in this ecosystem the task runs on; see [where a task runs](#where-a-task-runs) |
| `requires` | string list | `[]` | Capabilities a module must provide to run this task; see [where a task runs](#where-a-task-runs) |
| `fail_if_output` | boolean | `false` | Treat any stdout output as failure |
| `depends_on` | string list | `[]` | Tasks that must finish first: `"build"` on the same module, `"^build"` on each module this one depends on; see [task prerequisites](#task-prerequisites) |
| `outputs` | string list | `[]` | Files the task produces, relative to the module root; a cached result replays only while they exist |

Toven keeps authored argv unchanged. Templates expand selectors and Toven variables, but they do not infer hidden flags.

Every task runs with the project root as its working directory. Path variables other than `{project.root}` are relative to that root, so they stay stable across machines and in cache keys. If a command changes directory first (for example `sh -c 'cd {module.root} && …'`), a later relative path no longer points where you expect; use `{project.root}` to build an absolute path instead.

| Variable | Value |
|---|---|
| `{args}` | Passthrough arguments after `--`, spliced as separate argv items |
| `{project.root}` | Absolute path of the project root |
| `{workspace.root}` | The module's workspace root, relative to the project root (`.` for the root) |
| `{module.name}` | Module name |
| `{module.package}` | Package name (Go module path, Cargo package); falls back to the module name |
| `{module.root}` | Module directory, relative to the project root |
| `{module.manifest}` | Manifest path, relative to the project root |
| `{module.selector}` | The task's `selector` list, spliced as separate argv items |

### Where a task runs

By default a task runs on every module of its ecosystem. Two keys narrow that:

- `modules` names the modules the task runs on. A name that matches no discovered module is an error, so a typo never silently disables the task.
- `requires` lists capabilities a module must provide. Discovery reports what each module provides: a Rust package with a binary target provides `bin`, and command modules list theirs in `capabilities`.

A module the task does not apply to is skipped, and the plan, `explain`, and JSONL output say which module was skipped and why. Modules reached through `--dependents`, `--dependencies`, a glob, or a whole workspace are skipped the same way. Naming such a module exactly with `--module` is an error instead when the task runs alone. In a task list or set it is skipped too, and the run fails only if no task applies to any selected module.

Modules of an ecosystem that does not define the task are left out. A changed-scope run whose changes touch only such ecosystems plans nothing and succeeds; a task no ecosystem defines is an error.

```toml
[ecosystems.rust.tasks.run]
argv = ["cargo", "run", "--manifest-path", "{module.manifest}", "{module.selector}", "{args}"]
persistent = true
requires = ["bin"]
selector = ["-p", "{module.package}"]
```

A group task override that sets `requires` replaces the whole list.

### Task prerequisites

`depends_on` lists tasks that must finish before this one. A bare name runs that task on the same module first. A name with `^` runs it first on every module this module depends on. Toven adds the prerequisite units to the plan, orders them into earlier waves, and follows each prerequisite's own `depends_on` too.

```toml
[ecosystems.command.tasks.build]
argv = ["sh", "-c", "mkdir -p {module.root}/dist && echo built > {module.root}/dist/out"]
depends_on = ["^build"]
outputs = ["dist/out"]

[ecosystems.command.tasks.test]
argv = ["echo", "test {module.name}"]
depends_on = ["^build"]
```

With modules `app -> lib -> core`, `toven --module command:app test` runs `core#build`, then `lib#build`, then `app#test`.

- A prerequisite must be a task in the same ecosystem, must not be `persistent` (it would never finish), and the tasks must not form a cycle. `"^build"` on `build` itself is fine, because it walks down the module graph.
- Prerequisites run without the passthrough args given after `--`; those belong to the requested task.
- A module the prerequisite task does not apply to (see [where a task runs](#where-a-task-runs)) is skipped, and its own prerequisites still run.
- A group task override may replace `depends_on` for its members only, such as a package whose tests read its own build output (`depends_on = ["^build", "build"]`). It is checked against each member's task table with the same rules, so a group also cannot make a task `persistent` while a task on the same module, or on a module that depends on it, still waits for it. A group can also replace `outputs`, such as an app whose build writes `.next` instead of `dist`.

`outputs` makes the cache check the files a task produces. A cached result replays only when every listed file still exists with the size and modification time it had when it was recorded; otherwise the task runs again. Outputs must be git-ignored: the source hash skips ignored files, and a tracked output would change the source hash every time the task runs.

## Groups and overrides

```toml
[groups.integration]
description = "Cross-crate integration tests"
ecosystem = "rust"
modules = ["rust:toven-cli"]
run_strategy = "unordered"

[groups.integration.tasks.test]
argv = ["cargo", "nextest", "run", "--profile", "ci"]

[groups.integration.guardrails]
forbid = ["rust:toven-engine"]
allow_groups = ["core"]
```

| Key | Type | Default | Meaning |
|---|---|---|---|
| `description` | string | None | What the group is; shown by `toven groups` |
| `ecosystem` | string | None | Default ecosystem for bare module names |
| `modules` | string list | `[]` | Group members, bare names or `ecosystem:module` refs |
| `run_strategy` | `"leaf-to-top"` or `"unordered"` | Inherit | Group-scoped wave-ordering override |
| `tasks` | table | `{}` | Sparse task overrides |
| `guardrails.forbid` | string list | `[]` | Fully qualified edges the group must not depend on |
| `guardrails.allow` | string list | `[]` | Optional fully qualified edge allowlist |
| `guardrails.allow_groups` | string list | Unset | Groups in the same file the members may depend on; `[]` allows none |

Group dependencies come from the real module graph; you never list them by hand. Guardrails only limit them. When `allow` is non-empty or `allow_groups` is set, every dependency that leaves the group must land in an allowed group or edge, so `allow_groups` enforces layering (for example, `core` may not depend on `apps`), and `allow_groups = []` keeps the lowest layer free of other groups. Only direct dependencies are checked, and `allow_groups` is not transitive: list every group the members depend on directly. `toven groups` lists each group with its layering.

A group is also a named selection: `toven test --group integration` runs the task on its members (see [select scope](../commands/run.md#select-scope)). Same-named groups declared by a federation member and by the umbrella select together.

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

`[modules."<ecosystem>:<name>".coverage]` accepts only threshold and `enforcement` keys. `exclude` and `profiles` are ecosystem-level coverage settings and fail validation inside a per-module override.

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

## Task sets

`[sets.<name>]` names a list of tasks that run together. `toven <name>` then works like a task:

```toml
[sets.verify]
tasks = ["lint", "typecheck", "test"]
```

Toven plans every task into one plan and runs them side by side, so list order is not run order. Keep tasks that change files (such as `fmt`) out of a set with tasks that read them.

A set name must be a safe identifier. It cannot match a task kind (`build`, `test`, …), a configured task, or a built-in command, and its list cannot name another set or repeat a task.

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
