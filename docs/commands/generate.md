# Generating config

`toven generate` creates an initial reviewable `toven.toml`.

```bash
toven generate [--root PATH] [--profile NAME] [--adapter ID] \
  [--manifest PATH ...] [--stdout | --write] [--overwrite]
```

## Behavior

By default, generation prints TOML to stdout:

```bash
toven generate --stdout
```

`--write` writes `<root>/toven.toml`:

```bash
toven generate --write
```

Toven refuses to replace an existing config unless overwrite is explicit:

```bash
toven generate --write --overwrite
```

For Rust repositories, `--manifest` can be repeated when the repository has
multiple independent Cargo manifests:

```bash
toven generate \
  --manifest core/Cargo.toml \
  --manifest contrib/Cargo.toml \
  --stdout
```

Rust generation records manifest discovery in the profile. Cargo metadata stays
the source of truth for local path dependencies, so generated overlays are
reserved for relationships native metadata cannot prove.

## Options

| Option | Purpose |
|--------|---------|
| `--root PATH` | Project root to inspect. Defaults to `.`. |
| `--profile NAME` | Generated profile name. Defaults to `main`. |
| `--adapter ID` | Limit generation to one adapter. |
| `--manifest PATH` | Rust Cargo manifest to include, relative to `--root`; repeatable. |
| `--stdout` | Print generated config. This is the default. |
| `--write` | Write `toven.toml` under the selected root. |
| `--overwrite` | Allow `--write` to replace an existing config. |

## Review checklist

- Generated tasks are understandable and minimal.
- Repository-specific workflow policy remains visible in task argv.
- Multiple Cargo manifests are represented in discovery settings.
- Dependency overlays are used only for relationships the adapter cannot infer.
