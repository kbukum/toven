# Generating config

`toven generate` scaffolds a reviewable `toven.toml` for a repository. It detects each ecosystem present (Rust by `Cargo.toml`, Go by `go.mod`) and emits a minimal config that relies on smart defaults.

```bash
toven generate [--root PATH] [--force ID] [--stdout | --write]
```

## Preview or write

By default the rendered TOML prints to stdout (diagnostics go to stderr), so you can review or redirect it. Pass `--stdout` to make that preview explicit:

```bash
toven generate            # preview on stdout
toven generate --stdout   # same, explicit
toven generate > toven.toml
```

`--write` writes `<root>/toven.toml` atomically and confirms on stderr:

```bash
toven generate --write
```

`--stdout` and `--write` are mutually exclusive — one previews and writes nothing, the other persists the file.

A first run writes `[project]` plus one `[ecosystems.<id>]` section per detected ecosystem, carrying only the discovery hints:

```toml
[project]
name = "my-repo"
root = "."
base_ref = "origin/main"

# Smart defaults fill in tasks, run strategy, and toolchain probes.
# Uncomment to override, e.g.:
#   run_strategy = "leaf-to-top"
[ecosystems.rust]
manifests = ["Cargo.toml"]
```

## Re-running

Re-running against an existing config adds only `[ecosystems.<id>]` sections that are missing. It warns on sections that already exist, leaves `[project]` and `[toven]` untouched, and preserves your formatting and comments. A re-run that adds nothing leaves the file byte-identical.

Regenerate one section — for example after restructuring a workspace — with `--force`:

```bash
toven generate --write --force rust
```

`--force` replaces exactly that section and leaves everything else alone.

## What it detects

Generation runs before any config exists, so it probes a bootstrap set: every adapter linked into the binary, plus any `toven-<eco>` driver on `PATH`. Each self-detects whether it applies and contributes its `[ecosystems.<id>]` fragment. A linked adapter wins over a `PATH` driver for the same ecosystem.

The umbrella `toven generate` scaffolds every detected ecosystem; a focused driver such as `toven-rs generate` scaffolds only its own.

Generation emits `[project]`, `[toven]`, and `[ecosystems.*]` sections. It leaves `[groups.*]` and `[[overlays]]` to you — group membership and cross-ecosystem edges are human-declared, and Cargo/Go metadata already covers native dependency edges.

## Options

| Option | Purpose |
|--------|---------|
| `--root PATH` | Project root to inspect and scaffold against. Defaults to `.`. |
| `--force ID` | Regenerate exactly one `[ecosystems.<id>]` section. |
| `--stdout` | Render `toven.toml` to stdout and write nothing (the default preview, made explicit). Mutually exclusive with `--write`. |
| `--write` | Write `<root>/toven.toml` atomically instead of printing it. Mutually exclusive with `--stdout`. |

Some legacy `generate` flags are intentionally not carried over. `--profile` is gone with the profile model (replaced by ecosystems); `--adapter` and repeated `--manifest` are unnecessary because ecosystem discovery is automatic and `--force <id>` already targets one section; and `--overwrite` is dropped because generation is additive and never clobbers — `--force <id>` regenerates exactly one section instead.

## After generating

Review the config, then inspect before running:

```bash
toven modules
toven graph
toven plan check
```

Keep workflow policy visible: if a task needs a flag, put it in the task argv. See [inspecting work](inspect.md) and [running tasks](run.md).
