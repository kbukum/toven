# Getting started

This guide adopts Toven in a Rust repository. Toven discovers modules and plans execution; the actual tool argv stays reviewable in `toven.toml`.

## 1. Onboard with the wizard

From the repository you want Toven to manage:

```bash
toven init
```

The wizard detects each ecosystem, asks a short questionnaire, and writes `toven.toml`. When there is no root `Cargo.toml`, Toven also discovers first-level nested Cargo manifests, skipping hidden and Git-ignored directories. Onboard a different directory with `--root`:

```bash
toven init --root ../other-repo
```

Preview without writing using `--print`, or take every default without prompting with `--non-interactive`:

```bash
toven init --print
```

Re-running is additive: it adds missing `[ecosystems.<id>]` sections and leaves existing sections, `[project]`, and `[toven]` alone. Regenerate one section with `--force rust`. See [onboarding a repository](commands/init.md).

## 2. Review `toven.toml`

The generated config describes:

- project name, root, and optional default baseline (`base_ref`)
- one or more `[ecosystems.*]` sections, such as `[ecosystems.rust]`
- discovery settings, such as Cargo manifest paths

`init` seeds starter tasks such as `check`, `build`, `test`, `lint`, and `format`. Treat them like npm scripts: edit `[ecosystems.rust.tasks.<name>]` to change what `toven <name>` runs. See [running tasks](commands/run.md) for the full task model.

Generated Rust task argv uses the selector model: `{module.selector}` marks the splice point in `argv`, and the task's `selector` fragment renders the concrete package selection (`-p {module.package}`). Keep workflow policy visible — if a task needs a flag, put it in the argv.

## 3. Inspect before running

```bash
toven modules
toven graph
toven plan check
```

To see only work related to changes since a baseline:

```bash
toven plan check --base origin/main --merge-base
toven affected check --base origin/main --merge-base
```

See [inspecting work](commands/inspect.md).

## 4. Run a task

```bash
toven check
```

Pass extra tool arguments straight through:

```bash
toven test --nocapture
```

Toven consumes only its own flags that immediately follow the task name; the first argument it does not own, and everything after, goes to the command verbatim. Use `--` to force the boundary when your first argument looks like a Toven flag:

```bash
toven test -- --explain
```

Passthrough args disable caching unless the task sets `cache_args = true`. See [passing arguments](commands/run.md#passing-arguments-to-the-tasks-command).

Keep a task running across edits with `--watch`: Toven reruns the affected subgraph on each save, and Ctrl+C exits.

```bash
toven test --watch
```

## 5. Inspect and manage cache

Show the planned unit(s) — argv, dependencies, persistence — for a task, optionally filtered to one module:

```bash
toven explain check --module rust:rskit-config
```

Inspect and clean the cache:

```bash
toven cache path
toven cache stats
toven cache clean
```

See [managing cache](commands/cache.md).

## 6. Measure coverage and rehearse a release

When a `coverage` task is configured, run it to aggregate per-module profiles and gate them against the resolved `[…coverage]` thresholds:

```bash
toven coverage
toven coverage --line 90 --enforcement advisory
```

`--enforcement advisory` reports shortfalls without failing, and the threshold flags (`--line`/`--function`/`--region`/`--changed-line`) override the config for a single run. See [measuring coverage](commands/coverage.md).

Before cutting a real release, rehearse the whole pipeline with `--dry-run` — it resolves the same plan and reports the publish order and per-module verdicts without mutating any manifest, tag, or registry:

```bash
toven release plan
toven release publish --dry-run
```

See [releasing](commands/release.md).

## Related docs

- [Command reference](commands/README.md)
- [What Toven does](product.md)
- [Architecture](architecture.md)
- [Benchmarking](benchmarking.md)
