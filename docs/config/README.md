# Configuration guide

Toven loads one strict `toven.toml`. Unknown fields and invalid combinations fail instead of being ignored.

## Minimal configuration

```toml
[project]
name = "example"
root = "."
base_ref = "origin/main"

[ecosystems.rust]
manifests = "auto"
```

Generate a starter file with [`toven init`](../commands/init.md).

## Top-level sections

| Section | Purpose |
|---|---|
| `[project]` | Repository identity, root, and affected baseline |
| `[toven]` | Output, concurrency, view, includes, cache, and git push auth |
| `[ecosystems.<id>]` | Discovery, tasks, coverage, and release policy |
| `[modules."<ecosystem:name>"]` | Per-module policy overrides |
| `[groups.<name>]` | Named module sets and task overrides |
| `[[overlays]]` | Explicit dependency edges |
| `[[members]]` | Federated repository members |

## Project

```toml
[project]
name = "example"
root = "."
base_ref = "origin/main"
```

`root` is resolved relative to `toven.toml`. `base_ref` is used when affected commands do not receive `--base`.

## Runtime

```toml
[toven]
max_parallel = 8
view = "auto"
include = ["ci/shared-tasks.toml"]

[toven.cache]
dir = ".toven/cache"

[toven.git]
push_token_env = ["GITHUB_TOKEN", "GH_TOKEN"]
```

Included files provide defaults. The canonical `toven.toml` wins on scalar and table conflicts. Included files must be committed.

`[toven.git].push_token_env` lists, in order, the environment variables the embedded git backend consults for a push/fetch token during a mutating release. The first present, non-empty value is used as the HTTPS token-as-password, so an authenticated push (for example a tag push to a protected branch in CI) succeeds without relying on ambient git credential helpers. It is forge-agnostic: the default names suit GitHub Actions, but any forge's token variable (such as `GITLAB_TOKEN`) can be substituted. When none of the variables are set — the usual local-development case — the backend falls back to its ambient transport default, so nothing changes for day-to-day work.

## Rust discovery

```toml
[ecosystems.rust]
manifests = "auto"
exclude = ["fuzz"]
```

`"auto"` discovers the root Cargo workspace or first-level workspace roots during every plan. Use an explicit list to freeze the managed set:

```toml
[ecosystems.rust]
manifests = ["Cargo.toml", "tools/Cargo.toml"]
```

## Go discovery

```toml
[ecosystems.go]
modules = "auto"
```

With `go.work`, `"auto"` uses its members. Without it, Toven discovers the root module and first-level nested modules. Use an explicit `go.mod` list to freeze the set.

## Tasks

```toml
[ecosystems.rust.tasks.test]
kind = "test"
argv = ["cargo", "nextest", "run", "--manifest-path", "{module.manifest}", "{module.selector}", "{args}"]
selector = ["-p", "{module.package}"]
fan_out = "batchable"
shared_inputs = ["Cargo.lock", "rust-toolchain.toml"]
cache_args = true
```

Task names are user-owned identities. `kind` is an optional recognition attribute.

Common fields:

| Field | Purpose |
|---|---|
| `argv` | Exact argument vector template |
| `selector` | Module selector inserted at `{module.selector}` |
| `fan_out` | Per-module or batchable scheduling |
| `persistent` | Keep the process alive after readiness |
| `readiness` | Decide when a persistent task is ready |
| `cacheable` | Allow successful result reuse |
| `cache_args` | Include passthrough arguments in cache keys |
| `shared_inputs` | Repository files that invalidate every matching unit |
| `fail_if_output` | Fail when the command emits output |

## Groups and overrides

```toml
[groups.integration]
run_strategy = "unordered"

[groups.integration.tasks.test]
argv = ["cargo", "nextest", "run", "--profile", "ci"]
```

Group task entries are sparse overrides over an existing ecosystem task. Scalars and lists replace the base value; `shared_inputs` is additive. Conflicting group overrides fail.

## Overlays

Use overlays when native metadata cannot express a dependency:

```toml
[[overlays]]
from = "rust:cli"
to = "go:api"
```

The edge participates in selection, scheduling, and release cascades.

## Federation

An umbrella configuration can compose independently configured repositories:

```toml
[[members]]
name = "service"
root = "services/service"
base_ref = "origin/main"
```

Each member retains its own `toven.toml`. The umbrella composes their modules into one graph.

## Coverage and release

- [Coverage command and configuration](../commands/coverage.md#configuration)
- [Release configuration](release.md)

## Validate configuration

Run a read-only command:

```bash
toven modules
toven tasks
toven plan check
```

Configuration errors use stderr and return non-zero.
