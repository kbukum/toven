# Release configuration

Toven's release behavior is declarative. You own it through the `[…release]` config block, so a release runs *your* way — bump defaults, prerelease channels, tag/commit templates, changelog, push/branch gating, registry, signing, and hooks — without a per-run flag for every choice.

> **Status:** the full block is parsed, validated, and resolved with the precedence below. The bump policy (`strategy`, `level`, `dependent_version`, `prerelease`), the per-run bump argv, and the `[…release.host]` hosted-forge Release settings are consumed by the release engine; the remaining target/signing/hooks fields are schema-and-resolution only for now and are wired into the pipeline in later work.

The same block is available at two levels:

- `[ecosystems.<id>.release]` — the ecosystem-wide default for every module in that ecosystem.
- `[modules.<ecosystem:module>.release]` — a per-module override whose set fields win over the ecosystem default.

Both are strict (`deny_unknown_fields`) and every field defaults, so an existing `toven.toml` keeps parsing unchanged and an unset override field inherits the ecosystem default.

## Precedence

The engine folds each module's settings from lowest to highest precedence:

```text
per-run bump argv  >  [modules.<ecosystem:module>.release]  >  [ecosystems.<id>.release]  >  built-in adapter default
```

Per-run argv (`--patch`/`--minor`/`--major`/`--pre <channel>` …) is layered by `toven release` at run time; the config levels resolve to a single per-module settings value first. A field set in a per-module override replaces only that field; every other field still inherits the ecosystem default (and, in turn, the built-in default). A list or sub-table override replaces the inherited value too — including clearing it: an explicit `branches = []` opts one module out of an ecosystem branch restriction, while omitting `branches` inherits it.

## Fields

Every field is optional. The **Default** column is the built-in value applied when neither config level sets it.

| Field | Type | Default | Purpose |
|-------|------|---------|---------|
| `strategy` | string | `semver-cascade` | Named bump policy. Currently resolves to the single `semver-cascade` matrix (patch by default, minor on a breaking signal, major on request, cascading a dependency-floor bump into dependents); the field is a named selector so more policies can be added later. Every module in one plan must resolve the same policy. |
| `level` | `patch` \| `minor` \| `major` \| `auto` | `auto` | Default bump level for a changed module. `auto` defers to change classification (patch unless a breaking signal forces minor). |
| `dependent_version` | `bump` \| `upgrade` | `bump` | How a dependency-floor bump cascades: `bump` re-releases the dependent; `upgrade` only raises its floor. |
| `tag_format` | template | target default | Optional release tag name template override. Placeholders: `{version}`, `{ecosystem}`, `{module}` (`{channel}` is rejected — the prerelease channel is already part of `{version}`); when unset, each release target owns its default grammar (Rust uses `{ecosystem}/{module}@{version}`, Go rejects overrides and uses Go module tags). |
| `tag_message` | template | — | Annotated-tag message template; unset cuts a lightweight tag. |
| `commit_message` | template | adapter default | Release commit message template. |
| `push` | bool | `true` | Whether the release commit and tags are pushed. |
| `remote` | string | `origin` | Git remote pushed to. |
| `branches` | list of string | any branch | Allowed release branches; empty allows any branch. |
| `registry` | string | — | Target registry identifier (e.g. `crates-io`); unset means not publishable. |
| `offline` | bool | `false` | Skip registry lookups and anchor idempotency on release tags only. |
| `token_env` | string | — | Environment-variable **name** holding the registry token (never the secret itself). |
| `readiness` | list of string | — | Ordered checks composing `release readiness`; each must be a recognized name (`clean-tree`, `registry-idempotent`). |

### `[…release.prerelease]`

Prerelease channels and the optional branch→channel mapping.

| Field | Type | Default | Purpose |
|-------|------|---------|---------|
| `channels` | list of string | empty | Recognized prerelease channels (e.g. `rc`, `alpha`, `beta`) that `--pre <channel>` resolves against. |
| `branch_channels` | map string→string | empty | Maps a release branch to the channel it cuts (each channel must be in `channels`). |

### `[…release.changelog]`

| Field | Type | Default | Purpose |
|-------|------|---------|---------|
| `path` | string | `CHANGELOG.md` | Workspace-relative changelog path (must stay inside the workspace). |
| `required` | bool | `false` | Fail a release when a changed module has no changelog entry. |

### `[…release.host]`

The forge Release cut after the tag is pushed and the registry publish succeeds. With no `forge`, the pipeline stops after publish — a hosted Release is opt-in. Only `github` is supported today (via the argv-first `gh` CLI); GitLab is a documented same-port seam and a non-`github` forge is a typed error.

| Field | Type | Default | Purpose |
|-------|------|---------|---------|
| `forge` | string | — | Forge that hosts the Release (`github`); unset means no hosted Release. |
| `draft` | bool | `false` | Cut the Release as a draft. |
| `prerelease` | bool | derived | Mark the Release as a prerelease; unset derives from the released version's prerelease channel. |
| `notes` | string | changelog | Explicit release-note body; unset sources notes from the module's changelog. |
| `assets` | list of string | empty | Project-relative artifact paths uploaded to the Release. |

### `[…release.sign]`

| Field | Type | Default | Purpose |
|-------|------|---------|---------|
| `enabled` | bool | `false` | Whether release artifacts are signed. |
| `signer` | string | — | Signer identity/key selection (never a secret); rejected unless `enabled = true`. |

### `[…release.hooks]`

Both lists name **recognized task references** (argv-first, no shell unless the task opts in), composed from the same task model that drives every other verb.

| Field | Type | Default | Purpose |
|-------|------|---------|---------|
| `pre` | list of string | empty | Task references run before the release mutation. |
| `post` | list of string | empty | Task references run after a successful release. |

## Example

```toml
[ecosystems.rust.release]
strategy = "semver-cascade"
level = "auto"
tag_format = "{ecosystem}/{module}@{version}"
registry = "crates-io"
token_env = "CARGO_REGISTRY_TOKEN"
readiness = ["clean-tree", "registry-idempotent"]

[ecosystems.rust.release.prerelease]
channels = ["rc", "beta"]
branch_channels = { next = "beta" }

[ecosystems.rust.release.changelog]
path = "CHANGELOG.md"
required = true

[ecosystems.rust.release.host]
forge = "github"
assets = ["target/toven/release/core.cdx.json"]

# One module cuts its own major-versioned tags.
[modules."rust:core".release]
level = "major"
tag_format = "core-v{version}"
```

Here `rust:core` releases with a `major` level and a `core-v{version}` tag, but still inherits the ecosystem `registry`, `readiness`, prerelease channels, and changelog settings. If `tag_format` is omitted, the Rust target uses its own default instead of an engine-wide default; the Go target rejects a configured `tag_format` and always uses Go module tag conventions.

See [`toven release`](../commands/release.md) for the lifecycle actions these settings drive.
