# Releasing

`toven release <action>` walks a release through a reviewable lifecycle. The read-only actions preview what a release would do; the actions that change state drive the release pipeline over the federated dependency graph. `release tag` cuts the release (commit, tag, push); `release publish` continues through to the registry.

```text
toven release plan       # show the release PLAN cut, mutating nothing
toven release status     # declared vs published/tagged per module
toven release readiness  # fail-closed go/no-go release preflight
toven release sbom       # generate a CycloneDX SBOM per releasable module
toven release depgraphs  # render the dependency graph to a DOT artifact
toven release tag        # cut the release: commit, tag, push (no registry publish)
toven release publish    # run the full pipeline through publish (--dry-run previews it)
```

An action is required; `toven release` alone is a usage error.

Release behavior is declarative: bump defaults, prerelease channels, tag/commit templates, changelog, push/branch gating, registry, signing, and hooks are owned through the `[…release]` config block, per ecosystem and per module. The full block is parsed, validated, and resolved with documented precedence; the bump policy and per-run bump argv are consumed by the release engine, while the remaining target/signing/hooks fields are schema-and-resolution only for now. See [release configuration](../config/release.md) for every field, its default, the per-module override, and precedence.

## Read-only previews

`release plan` and `release status` never change a manifest, tag, commit, or registry (though both may issue read-only registry queries to resolve published versions). `release readiness` is likewise read-only; `release sbom` and `release depgraphs` write generated artifacts only inside their `--out-dir` and touch nothing else. They all render on stdout (warnings go to stderr) and honor `--output human` (default) or `--output jsonl`.

### `toven release plan`

Shows the release PLAN cut: per releasable module, its current version, the version that would be released, the resolved bump level, why the module is bumped, which input won under precedence, whether a publish is needed, and the changelog summary — in deterministic publish order.

```bash
toven release plan
toven release plan --output jsonl
```

The human table carries `Level`, `Reason`, and `Input` columns; the `--output jsonl` record additionally exposes `cascade_origin` (the changed module that triggered a dependency cascade), `prerelease_channel`, and `up_to_date` (a planned version already at/above the registry — reported as a no-op instead of a re-publish).

### `toven release status`

Shows each releasable module's declared version, whether that version is already published, and the newest release tag cut for it. The default human table carries a yes/no `Published` column; `--output jsonl` additionally lists the full set of versions the registry reports. Registry lookups are best-effort, so a partial published set still yields a status.

```bash
toven release status
```

### `toven release readiness`

Evaluates the fail-closed release preflight: each configured go/no-go check runs and reports pass/fail with a short detail, and the command exits non-zero the moment any check fails so CI gates on it. The checks are declared through `[…release.readiness]`; a `clean-tree` check fails when a member worktree is dirty, and a `registry-idempotent` check fails when a module declares a version behind what the registry already published. An unrecognized check name is a typed usage error rather than a silent pass. The default human table carries a `Result`/`Detail` column and a `go`/`no-go` verdict; `--output jsonl` emits one record per check.

```bash
toven release readiness
toven release readiness --output jsonl
```

### `toven release sbom`

Generates a [CycloneDX](https://cyclonedx.org/) SBOM per releasable module, orchestrating each ecosystem's SBOM tool argv-first and writing the artifacts into the directory named by `--out-dir` (default `target/toven/release`, created if absent). A module whose ecosystem has no SBOM tooling is reported as a skip on stderr rather than a failure. The command writes only inside the output directory and mutates nothing else. The human table lists each module and its artifact path; `--output jsonl` emits one record per artifact.

```bash
toven release sbom
toven release sbom --out-dir dist/sbom --output jsonl
```

### `toven release depgraphs`

Renders the validated federation dependency graph to a Graphviz DOT artifact under `--out-dir` (default `target/toven/release`, created if absent), reusing the same DOT renderer as `toven graph`. It writes only inside the output directory and mutates nothing else. The human table lists the graph label and its artifact path; `--output jsonl` emits one record per artifact.

```bash
toven release depgraphs
toven release depgraphs --out-dir dist/graphs
```

## Dry run — `--dry-run`

For `release publish`, `--dry-run` is a real **dry run**: it resolves the same release plan a real run would and reports the resolved publish order and per-module `would-publish`/`already-published` verdicts, plus any hosted forge Releases it would cut, without changing any manifest, tag, or registry, without running any publish, and without calling the forge. `release plan` is already a preview, so `--dry-run` is a no-op there; it is rejected on `release status` and `release tag` (which never publishes).

```bash
toven release publish --dry-run
toven release publish --dry-run --output jsonl
```

## Actions that change state

`release tag` and `release publish` run the release pipeline and report progress through the human run reporter (or the `--output jsonl` event stream). `release tag` stops after the release commit, tags, and push; `release publish` also publishes the packaged artifacts to the registry. They accept the safety-bypass flags:

- `--allow-dirty` — proceed even when the worktree has uncommitted changes.
- `--no-push` — skip pushing commits and tags to the remote.

Both flags are rejected on every non-mutating action (`plan`, `status`, `readiness`, `sbom`, `depgraphs`) with a typed usage error.

```bash
toven release tag
toven release publish --allow-dirty --no-push
```

## Hosted forge releases

When a module's `[…release.host]` block names a `forge`, `release publish` cuts a Release on that forge after the tag is pushed and the registry publish succeeds. Only `github` is supported today; it shells out to the `gh` CLI argv-first (never passing a token on the command line — `gh` reads the ambient `GH_TOKEN`/`GITHUB_TOKEN`), so `gh` must be installed and authenticated. The hosted phase is idempotent: it creates the Release, and if one already exists for the tag it edits the Release and re-uploads assets with `--clobber`.

The Release title is the tag, its notes come from the module's changelog body (or the `notes` override), it is marked a prerelease when the plan cut a prerelease channel (or when `prerelease` overrides), and any configured `assets` are resolved relative to the project root and uploaded. The phase is skipped entirely under `--no-push` (a hosted Release needs the pushed tag) and under `--dry-run` (which only previews the Release). GitLab is a documented same-port seam; a non-`github` forge is a typed error.

```bash
toven release publish              # tag, publish, then cut the hosted Release
toven release publish --dry-run    # preview the hosted Release without calling the forge
```

## Go module tags

Go releases are tag-only: there is no registry publish or manifest version rewrite, so Toven treats the git tag as the released version and the generic publish loop records the tag-only target as published after the release commit. The root Go module is tagged as `vX.Y.Z`; each submodule is tagged with its repo-relative module root followed by the version, for example `cache/redis/v1.2.3`. `--no-push` still skips pushing the release commit and all tags, and `--allow-dirty` is still required to bypass the clean-tree guardrail.

Go rejects a configured `tag_format` because the Go module tag convention fixes the grammar; Rust and other registry targets may honor `tag_format` overrides.

## Bump policy and per-run bump flags

Each module bumps independently. By default a changed module takes a **patch** bump; a breaking signal forces a **minor** bump; a **major** bump is only ever explicit. A dependency-floor bump cascades into dependents, and a module already at/above the registry's max published version is a reported no-op ("up to date"), never re-published. "Breaking" is driven by an explicit signal only — a `--minor`/`--major` override or an explicit per-module config `level` — never inferred from raw argv.

The mutating actions (`release tag`/`release publish`) accept per-run bump argv that layers over the config. Each flag is rejected on every non-mutating action (`plan`, `status`, `readiness`, `sbom`, `depgraphs`) with a typed usage error.

- `--patch <module>` / `--minor <module>` / `--major <module>` (each repeatable) — force a module's bump level.
- `--set-version <module>=<x.y.z>` (repeatable) — pin an explicit target version for a module.
- `--pre <channel>` — cut a prerelease on a configured channel (`rc`/`alpha`/`beta`), validated against the `[…release.prerelease]` channels.
- `--base <ref>` — git ref to diff against for change detection (default: the latest release tag).
- `--offline` — skip registry `published_versions` lookups and anchor idempotency on the release tag only.

A module named in two level flags, or in both a level flag and `--set-version`, is a typed usage error; an override naming an unknown module is rejected with an error naming the module.

### Precedence

Each module's bump resolves from highest to lowest precedence:

```text
per-run bump argv  >  [modules.<ecosystem:module>.release]  >  [ecosystems.<id>.release]  >  built-in adapter default
```

Per-run argv (`--patch`/`--minor`/`--major`/`--set-version`/`--pre`) wins over both config levels; the per-module override wins over the ecosystem default, which wins over the adapter default. The `release plan` `Input` column reports which input won for each module (`argv`, `set-version`, `config`, `changelog`, `default`, or `cascade`).

```bash
toven release publish --minor rust:core --set-version rust:app=2.0.0
toven release tag --pre rc --base v1.4.0 --offline
```

## Which flags apply

| Flag | plan | status | readiness | sbom | depgraphs | tag | publish |
|------|:----:|:------:|:---------:|:----:|:---------:|:---:|:-------:|
| `--dry-run` (preview) | no-op | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ |
| `--allow-dirty` / `--no-push` | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ | ✓ |
| `--patch` / `--minor` / `--major` / `--set-version` / `--pre` / `--offline` | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ | ✓ |
| `--base` | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ | ✓ |
| `--out-dir` | ✗ | ✗ | ✗ | ✓ | ✓ | ✗ | ✗ |
| `-v`/`-q` / `--color` | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ | ✓ |
| `--output human\|jsonl` | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |

A rejected flag/action combination fails fast with a typed `InvalidInput` error, mapped to the usage exit code.
