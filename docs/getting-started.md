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
- one or more profiles using the Rust adapter
- discovery settings, such as Cargo manifest paths
- task argv templates for standard Rust commands such as `check`, `build`, `clippy`, `fmt-check`, and `test`
- shared inputs that should invalidate broad work, such as lockfiles or toolchain files

Toven should not hide workflow policy. If a command needs a flag, keep that flag visible in the task argv.

Generated Rust task argv uses `{module.args}` with the profile `module_arg_template`, so package selection is visible and not duplicated inside each task.

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

Pass extra tool arguments after `--`:

```bash
toven test -- --no-capture
```

Passthrough args disable cache by default unless the task definition explicitly sets `cache_args = true`, because arbitrary flags can change command semantics.

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
