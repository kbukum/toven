# Releasing

`toven release <action>` walks a release through a reviewable lifecycle. The read-only actions project what a release would do; the mutating actions drive the release pipeline over the federated dependency graph. `release tag` cuts the release (commit, tag, push); `release publish` continues through to the registry.

```text
toven release plan       # show the release PLAN cut, mutating nothing
toven release status     # declared vs published/tagged per module
toven release tag        # cut the release: commit, tag, push (no registry publish)
toven release publish    # run the full pipeline through publish (--dry-run rehearses it)
```

An action is required; `toven release` alone is a usage error.

Release behavior is declarative: bump defaults, prerelease channels, tag/commit templates, changelog, push/branch gating, registry, signing, and hooks are owned through the `[…release]` config block, per ecosystem and per module. See [release configuration](../config/release.md) for every field, its default, the per-module override, and precedence.

## Read-only projections

`release plan` and `release status` never mutate a manifest, tag, commit, or registry (though both may issue read-only registry queries to resolve published versions). They render on stdout (warnings go to stderr) and honor `--output human` (default) or `--output jsonl`.

### `toven release plan`

Shows the release PLAN cut: per releasable module, its current version, the version that would be released, whether a publish is needed, and the changelog summary — in deterministic publish order.

```bash
toven release plan
toven release plan --output jsonl
```

### `toven release status`

Shows each releasable module's declared version, whether that version is already published, and the newest release tag cut for it. The default human table carries a yes/no `Published` column; `--output jsonl` additionally lists the full set of versions the registry reports. Registry lookups are best-effort, so a partial published set still yields a status.

```bash
toven release status
```

## Rehearsal — `--dry-run`

For `release publish`, `--dry-run` is a real **rehearsal**: it resolves the same release plan a real run would and reports the resolved publish order and per-module `would-publish`/`already-published` verdicts, without mutating any manifest, tag, or registry and without invoking any publish. `release plan` is already a projection, so `--dry-run` is a no-op there; it is rejected on `release status` and `release tag` (which never publishes).

```bash
toven release publish --dry-run
toven release publish --dry-run --output jsonl
```

## Mutating actions

`release tag` and `release publish` run the release pipeline and report progress through the human run reporter (or the `--output jsonl` event stream). `release tag` stops after the release commit, tags, and push; `release publish` also publishes the packaged artifacts to the registry. They accept the guardrail-bypass flags:

- `--allow-dirty` — proceed even when the worktree has uncommitted changes.
- `--no-push` — skip pushing commits and tags to the remote.

Both flags are rejected on the read-only actions (`plan`, `status`) with a typed usage error.

```bash
toven release tag
toven release publish --allow-dirty --no-push
```

## Flag applicability

| Flag | plan | status | tag | publish |
|------|:----:|:------:|:---:|:-------:|
| `--dry-run` (rehearsal) | no-op | ✗ | ✗ | ✓ |
| `--allow-dirty` / `--no-push` | ✗ | ✗ | ✓ | ✓ |
| `-v`/`-q` / `--color` | ✗ | ✗ | ✓ | ✓ |
| `--output human|jsonl` | ✓ | ✓ | ✓ | ✓ |

A rejected flag/action combination fails fast with a typed `InvalidInput` error, mapped to the usage exit code.
