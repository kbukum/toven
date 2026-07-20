# Onboarding a repository

`toven init` runs an interactive wizard that writes a reviewable `toven.toml` for a repository. It derives the project name from the enclosing git repository, detects each ecosystem present (Rust by `Cargo.toml`, Go by `go.mod`), and authors a config from your answers — adding a starter task table you own for each detected ecosystem. If none is detected it writes a `[project]`-only config and warns with the next steps.

```bash
toven init [--root PATH] [--force ID] [--non-interactive | --yes] [--print]
```

## The wizard

By default `init` prompts interactively on a terminal: it shows what it detected, asks each ecosystem's questions (for example, which Rust test runner to wire up), previews the result, and writes `<root>/toven.toml` atomically, confirming on stderr.

On a non-terminal (a pipe, CI, or a redirected prompt sink) the wizard never blocks: it resolves every question to its declared default. When it writes a config that way it also notes on stderr that prompts were skipped and points at `--print` (preview) and `--force <id>` (regenerate a section). Force non-interactive resolution explicitly with `--non-interactive` (alias `--yes`):

```bash
toven init --non-interactive   # take every default, no prompts
```

## Preview without writing

Pass `--print` to render the config to stdout and write nothing (diagnostics still go to stderr), so you can review or redirect it:

```bash
toven init --print > toven.toml
```

A first run writes `[project]` plus one `[ecosystems.<id>]` section per detected ecosystem, each carrying its discovery hints and the complete task table the planner runs:

```toml
[project]
name = "my-repo"
root = "."
base_ref = "origin/main"

[ecosystems.rust]
manifests = "auto"

[ecosystems.rust.tasks.build]
argv = ["cargo", "build", "--manifest-path", "{module.manifest}", "{module.selector}", "{args}"]
fan_out = "batchable"
selector = ["-p", "{module.package}"]
shared_inputs = ["Cargo.lock"]

# ... check, doc, format, format-check, lint, run, test ...
```

`manifests = "auto"` re-discovers the Cargo workspace roots on every plan (a root `Cargo.toml`, or each first-level `<dir>/Cargo.toml` for a multi-workspace repo), so a workspace added later is picked up without editing the config. Narrow the wizard's workspace selection to freeze an explicit list instead (`manifests = ["core/Cargo.toml", ...]`); with `auto`, add `exclude = ["fuzz"]` to skip a workspace by directory. Each task's `shared_inputs` lists the tracked `Cargo.lock` beside every managed workspace, so a lockfile change invalidates the cache.

The Go section mirrors this with `modules = "auto"`: on every plan Toven enumerates the managed `go.mod` modules from a root `go.work`'s `use` list (at any depth), so a `go.work` spanning dozens of member modules is discovered without hand-listing them, and a module added to `go.work` later is picked up automatically. With no `go.work`, `auto` falls back to the root `go.mod` plus every first-level nested `go.mod`. Freeze the set with an explicit list instead (`modules = ["go.mod", "auth/go.mod", ...]`). Each Go module's identity comes from its repo-relative directory (`connect/testutil` → `go:connect-testutil`), so sibling modules that share a leaf directory name stay distinct; the repository-root module keeps the final segment of its module path (`github.com/kbukum/gokit` → `go:gokit`).

The task table is authoritative: the config is the single source of runnable tasks, so what `init` writes is exactly what `toven run` executes. Edit any argv to change what a task does.

When you choose the `cargo-nextest` runner, the generated `test` task carries `--no-tests=pass`, so a crate with no test targets is reported as a passing unit rather than a failure (nextest otherwise exits non-zero with "no tests to run").

The Go wizard offers four selections that shape its task table: the **lint backend** (`go vet` — the default, folded into `check`; or the external `golangci-lint` / `staticcheck`), the **formatter** (`gofmt` default, or `gofumpt` / `goimports`), the **test runner** (`go test` default, or `gotestsum`), and whether to **harden tests** with `-race -shuffle=on`. When a `.golangci.yml` is present the wizard recommends `golangci-lint`; otherwise it defaults to the toolchain-native tools so onboarding never forces an external dependency. The generated Go catalog is `build`, `check` (`go vet`), `format` / `format-check`, `tidy` / `tidy-fix`, `test`, `vuln` (`govulncheck`), `run`, and — only when an external backend is chosen — a distinct `lint` (so `check` and `lint` never share a cache key). Each CI gate is a read-only verification and pairs with an explicit state-changing twin: `format-check` (`gofmt -l`) with `format` (`gofmt -w`), and `tidy` (`go mod tidy -diff`) with `tidy-fix` (`go mod tidy`). The `tidy` gate's `-diff` flag requires Go 1.23+; on an older toolchain run `tidy-fix` and check the working tree instead. List-mode formatters print offenders but exit `0`, so `format-check` is marked `fail_if_output` — the engine's executor fails the unit when the command emits any stdout, turning `gofmt -l` into a real CI gate. The state-changing twins are authored `cacheable = false` — a mutation must run every time, never be suppressed by an outdated content-key hit. When `golangci-lint` is the backend the `lint` argv carries `--allow-parallel-runners`, so Toven's per-module fan-out runs concurrently despite golangci-lint's shared-cache lock.

## Release automation (opt-in)

After the ecosystem questions the wizard asks a single confirm — *Configure release automation for this ecosystem?* — that defaults to **no**. Decline it (the default, and what every non-interactive run resolves to) and no `[ecosystems.<id>].release` block is authored at all; the repository stays release-free until you opt in. This keeps `toven init --non-interactive` from ever committing a repository to a release policy it did not ask for.

Accept it and the wizard asks a few follow-ups, then writes a minimal, valid `[ecosystems.<id>].release` block from your answers:

- **Registry** (registry-capable ecosystems only): which registry releasable modules publish to. Rust offers `crates-io` (recommended) or *no registry (tag-only)*; a tag-only ecosystem such as Go module tags skips this question entirely and is always registry-less.
- **Prerelease channels**: any of `alpha`, `beta`, `rc` (choose none for stable-only). Selected channels are authored in menu order.
- **Hosted Release**: whether to cut a GitHub Release after publishing.

Every opted-in block also carries a `clean-tree` readiness check, and a `registry-idempotent` check is added whenever a registry is set. A Rust repository publishing to crates.io with an alpha channel and a hosted Release renders:

```toml
[ecosystems.rust.release]
readiness = ["clean-tree", "registry-idempotent"]
registry = "crates-io"

[ecosystems.rust.release.host]
forge = "github"

[ecosystems.rust.release.prerelease]
channels = ["alpha"]
```

A tag-only ecosystem (Go, or Rust with *no registry*) omits `registry` and drops the `registry-idempotent` check, leaving the release anchored on tags. The follow-up questions are gated on the opt-in confirm, so a non-interactive run never reaches them and never authors a release block.

## Re-running

Re-running against an existing config adds only `[ecosystems.<id>]` sections that are missing. It warns on sections that already exist, leaves `[project]` and `[toven]` untouched, and preserves your formatting and comments. A re-run that adds nothing leaves the file byte-identical.

Regenerate one section — for example after restructuring a workspace — with `--force`:

```bash
toven init --force rust
```

`--force` replaces exactly that section and leaves everything else alone.

## What it detects

Init runs before any config exists, so it probes a bootstrap set: every adapter linked into the binary, plus any `toven-<eco>` driver on `PATH`. Each self-detects whether it applies, asks its questionnaire, and contributes its `[ecosystems.<id>]` fragment. A linked adapter wins over a `PATH` driver for the same ecosystem.

The umbrella `toven init` onboards every detected ecosystem; a focused driver such as `toven-rs init` onboards only its own.

Init emits `[project]`, `[toven]`, and `[ecosystems.*]` sections. It leaves `[groups.*]` and `[[overlays]]` to you — group membership and cross-ecosystem edges are human-declared, and Cargo/Go metadata already covers native dependency edges.

## Options

| Option | Purpose |
|--------|---------|
| `--root PATH` | Project root to inspect and onboard against. Defaults to `.`. |
| `--force ID` | Regenerate exactly one `[ecosystems.<id>]` section. |
| `--non-interactive` / `--yes` | Answer the wizard from defaults, never prompting (CI-friendly). |
| `--print` | Render `toven.toml` to stdout and write nothing. |

## After onboarding

Review the config, then inspect before running:

```bash
toven modules
toven graph
toven plan check
```

Keep workflow policy visible: if a task needs a flag, put it in the task argv. See [inspecting work](inspect.md) and [running tasks](run.md).
