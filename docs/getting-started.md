# Getting started

This guide walks through adopting Toven in a Rust repository. Toven keeps command policy in your repository config: it discovers modules and plans execution, but the actual tool argv remains reviewable in `toven.toml`.

## 1. Generate a starter config

From the repository you want Toven to manage:

```bash
toven generate
```

When a repository has no root `Cargo.toml`, Toven also discovers first-level nested Cargo manifests automatically, excluding manifests ignored by Git. Use `--root <PATH>` when you want to scaffold a repository other than the current directory:

```bash
toven generate \
  --root ../other-repo
```

Review the generated TOML before committing it. To write the file directly:

```bash
toven generate --write
```

Re-running against an existing config is additive: Toven adds missing `[ecosystems.<id>]` sections, preserves existing sections and `[project]`/`[toven]`, and leaves the file unchanged when there is nothing to add. To regenerate one ecosystem section, use `--force <id>`:

```bash
toven generate --write --force rust
```

## 2. Review `toven.toml`

The generated config should describe:

- project name, root, and optional default baseline
- cache policy, which defaults to the platform user cache directory
- one or more `[ecosystems.*]` sections, such as `[ecosystems.rust]`
- discovery settings, such as Cargo manifest paths
- task argv templates for standard Rust commands such as `check`, `build`, `clippy`, `fmt-check`, and `test`
- shared inputs that should invalidate broad work, such as lockfiles or toolchain files

Toven should not hide workflow policy. If a command needs a flag, keep that flag visible in the task argv.

Generated Rust task argv uses the selector model: `{module.selector}` marks the splice point in `argv`, and the task's `selector` fragment renders the concrete package selection.

## 3. Inspect before running

Start with read-only inspection commands:

```bash
toven modules
toven graph
toven plan check
```

Use affected planning when you want to see only work related to changes since a baseline:

```bash
toven affected check --base origin/main --merge-base
toven plan check --base origin/main --merge-base
```

## 4. Run a task

Run a configured task directly:

```bash
toven check
```

Pass extra tool arguments straight through — no separator needed for the common case:

```bash
toven test --nocapture
```

Toven consumes only its own flags that immediately follow the task name (as a contiguous prefix); the first argument it does not own (and everything after) goes to the task's command verbatim. Use `--` to force the boundary when your first argument would otherwise look like a Toven flag:

```bash
toven test -- --explain
```

Passthrough args disable cache by default unless the task definition explicitly sets `cache_args = true`, because arbitrary flags can change command semantics.

Keep a task running across edits with `--watch`: after the first run Toven reruns only the affected subgraph each time you save a tracked file, and Ctrl+C exits.

```bash
toven test --watch
```

## 5. Inspect planned units

Show the planned unit(s) for one module/task — argv, dependencies, and persistence:

```bash
toven explain rust:rskit-config check
```

Check cache size:

```bash
toven cache stats
```

Show the resolved local cache directory:

```bash
toven cache path
```

Clean cache records when you need a fresh local run:

```bash
toven cache clean
```

## Related docs

- [Command reference](commands/README.md)
- [Running tasks](commands/run.md)
- [Inspection commands](commands/inspect.md)
- [Cache commands](commands/cache.md)
- [Benchmarking](benchmarking.md)
