# Getting started

This guide adopts Toven in a Rust repository. Toven discovers modules and plans execution; the actual tool argv stays reviewable in `toven.toml`.

## 1. Onboard with the wizard

From the repository you want Toven to manage:

```bash
toven init
```

The wizard detects each ecosystem, asks a short questionnaire, and writes `toven.toml`. When there is no root `Cargo.toml`, Toven also discovers first-level nested Cargo manifests, skipping any ignored by Git. Onboard a different directory with `--root`:

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

Smart defaults fill in the standard tasks (`check`, `build`, `clippy`, `fmt-check`, `test`), their run strategy, and toolchain probes. Override a task by adding `[ecosystems.rust.tasks.<name>]`.

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

## Related docs

- [Command reference](commands/README.md)
- [What Toven does](product.md)
- [Architecture](architecture.md)
- [Benchmarking](benchmarking.md)
