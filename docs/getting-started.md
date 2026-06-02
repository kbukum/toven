# Getting started

This guide walks through adopting Toven in a Rust repository. Toven keeps command
policy in your repository config: it discovers modules and plans execution, but
the actual tool argv remains reviewable in `toven.toml`.

## 1. Generate a starter config

From the repository you want Toven to manage:

```bash
toven generate --stdout
```

When a repository has no root `Cargo.toml`, Toven also discovers first-level
nested Cargo manifests automatically, excluding manifests ignored by Git. Pass
`--manifest` when you want to pin the generated config to specific manifests:

```bash
toven generate \
  --manifest core/Cargo.toml \
  --manifest contrib/Cargo.toml \
  --stdout
```

Review the generated TOML before committing it. To write the file directly:

```bash
toven generate --write
```

Replacing an existing config requires an explicit overwrite:

```bash
toven generate --write --overwrite
```

## 2. Review `toven.toml`

The generated config should describe:

- project name, root, and optional default baseline
- cache policy, which defaults to the platform user cache directory
- one or more profiles using the Rust adapter
- discovery settings, such as Cargo manifest paths
- task argv templates for standard Rust commands such as `check`, `build`,
  `clippy`, `fmt-check`, and `test`
- shared inputs that should invalidate broad work, such as lockfiles or
  toolchain files

Toven should not hide workflow policy. If a command needs a flag, keep that flag
visible in the task argv.

Generated Rust task argv uses `{module.args}` with the profile
`module_arg_template`, so package selection is visible and not duplicated inside
each task.

## 3. Inspect before running

Start with read-only inspection commands:

```bash
toven modules --task check
toven graph --task check
toven plan --task check
```

Use affected planning when you want to see only work related to changes since a
baseline:

```bash
toven affected --task check --base origin/main --merge-base
toven plan --task check --affected --base origin/main --merge-base
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

Passthrough args disable cache by default unless the task definition explicitly
sets `cache_args = true`, because arbitrary flags can change command semantics.

## 5. Understand cache and affected decisions

Explain one module/task decision:

```bash
toven explain rskit-config check --base origin/main --merge-base
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

## 6. Watch during development

Watch mode runs once, then reruns affected modules and dependents after file
changes:

```bash
toven test --watch
```

Use `--watch-debounce-ms <MILLIS>` if your editor or generated files produce
bursty file events.

## Related docs

- [Command reference](commands/README.md)
- [Running tasks](commands/run.md)
- [Inspection commands](commands/inspect.md)
- [Cache commands](commands/cache.md)
- [Benchmarking](benchmarking.md)
