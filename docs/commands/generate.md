# Generating config

`toven generate` is the onboarding workflow: point it at a repo and get a working, reviewable `toven.toml`. It detects each ecosystem present (Rust by `Cargo.toml`, Go by `go.mod`, …) and emits a minimal config that leans on smart defaults — only the discovery hints plus a few commented override hints, never a full dump of the default surface.

```bash
toven generate [--root PATH] [--force ID] [--write]
```

## Behavior

By default, generation prints the rendered TOML to stdout (diagnostics go to stderr), so you can review or redirect it:

```bash
toven generate            # preview on stdout
toven generate > toven.toml
```

`--write` writes `<root>/toven.toml` atomically and prints a one-line confirmation to stderr:

```bash
toven generate --write
```

A first run writes a minimal document: `[project]` plus one `[ecosystems.<id>]` section per detected ecosystem, carrying only the discovery hints.

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

## Re-running is additive and idempotent

A polyglot config grows over time. Re-running `toven generate` against an existing config:

- adds only `[ecosystems.<id>]` sections that are **not already present**;
- **warns** (to stderr) and leaves any section that already exists untouched;
- **never modifies** `[project]`/`[toven]` or any existing section;
- preserves your formatting and comments (the edit goes through a format-preserving TOML editor, not a destructive rewrite).

A re-run that adds nothing leaves the file byte-identical.

To regenerate one section on demand — for example after restructuring a workspace — name it with `--force`:

```bash
toven generate --write --force rust
```

`--force` replaces exactly that one section; every other section, and `[project]`/`[toven]`, are left alone.

## Polyglot and federation

Generation runs **before** any config exists, so it cannot resolve drivers from `toven.toml`. Instead it probes a bootstrap set: every adapter linked into the running binary, plus any `toven-<eco>` driver found on `PATH`. Each self-detects whether it applies and contributes its `[ecosystems.<id>]` fragment. This is the one command that uses `PATH` discovery without config — precisely because the config does not exist yet. A linked adapter always wins over a `PATH` driver for the same ecosystem.

The umbrella `toven generate` scaffolds **all** detected ecosystems; a standalone driver such as `toven-rs generate` scaffolds only its own.

## Scope

Generation emits config only — `[project]`, `[toven]`, and `[ecosystems.*]`:

- **No automatic `[groups.*]`.** Group membership is human-declared; generation cannot prove it, so at most a commented example.
- **No automatic `[[overlays]]`.** Cross-ecosystem dependency edges are relationships generation cannot infer; Cargo/Go metadata stays the source of truth for native edges, and overlays are reserved for what native metadata cannot prove.
- **No workspace or CI scaffolding.** Generation detects existing workspaces; it never creates them, and CI-file generation is a separate concern.

## Options

| Option | Purpose |
|--------|---------|
| `--root PATH` | Project root to inspect and scaffold against. Defaults to `.`. |
| `--force ID` | Regenerate exactly one `[ecosystems.<id>]` section. |
| `--write` | Write `<root>/toven.toml` (atomically). Without it, the document is printed to stdout. |

## Review checklist

- Generated sections are minimal — discovery hints, with overrides left commented.
- Repository-specific workflow policy stays visible once you uncomment and edit task argv.
- Existing sections and `[project]`/`[toven]` survive a re-run untouched.

After writing a config, inspect it before running tasks:

```bash
toven modules
toven graph
toven plan check
```
