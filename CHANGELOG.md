# Changelog

All notable changes to Toven are documented here. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

### Changed

### Deprecated

### Removed

### Fixed

### Security

## [0.1.0-alpha.10] - 2026-08-25

### Changed

- The release clean-tree guard now names the offending worktree paths in its error instead of reporting only a count: a rejected release reads `the working tree has N uncommitted change(s); commit or stash them before releasing: modified go.sum, …`, with each path rendered as a sorted `status path` entry so a CI-only dirty file (e.g. a regenerated `go.sum`) is diagnosable from the message alone. The list is bounded to 20 named paths with an `… and N more` tail so a pathologically dirty tree cannot produce an unbounded message.
- Raised the supported Rust toolchain floor to **1.97** (MSRV now equals the pinned stable), matching the rskit foundation which moved to a single 1.97 floor. `rust-toolchain.toml`, `[workspace.package].rust-version`, `clippy.toml`, and the CI/release/supply-chain workflows now pin 1.97; the previous dual `1.94`/`1.95` CI matrix collapses to a single 1.97 lane.
- Bumped the vendored `rskit` submodule to `v0.2.0-alpha.8-11` (`cfa7bc6`), advancing the consumed rskit core crates (`rskit-fs`, `rskit-git`, `rskit-process`, `rskit-util`, `rskit-version`, `rskit-config`, `rskit-cli`, `rskit-testutil`) and refreshing `Cargo.lock` accordingly.

## [0.1.0-alpha.9] - 2026-08-23

### Fixed

- Whole-workspace tasks (e.g. `toven coverage`) no longer fail closed on a repository whose workspaces form a facade back-dependency cycle. The schedule leveler now condenses the strongly-connected components of the unit graph before leveling (iterative Tarjan): an acyclic graph levels byte-identically to before, while an irreducible facade cycle whose units are all whole-workspace invocations that resolve their own cross-workspace dependency closure condenses into a single co-scheduled wave instead of erroring with `condensed unit graph is cyclic after layering`. Eligibility is a verified task capability (`workspace_closure`), not the fan-out ceiling alone — ecosystem adapters set it on tool invocations that operate on the whole workspace atomically (`cargo … --workspace`, `go … ./...`), while an arbitrary custom whole-workspace command stays ineligible so a cycle touching one keeps failing closed. The mutual gating edges inside a co-scheduled cycle are stripped before APPLY, so its peers launch concurrently in one wave with no residual intra-cycle gate to deadlock or mutually block; the real cross-cycle handoffs are preserved. A consumer with four Cargo workspaces in a `core ⇄ contrib` facade cycle (plus `examples`/`fuzz`) now gates all of them green in one `toven coverage` invocation.
- Releasing a single-version Cargo workspace now routes every inheriting member's bump to the shared `[workspace.package].version` exactly once. The crates.io target rewrites the workspace root only on the first member that requests the bump and reports no path for later members that find the root already at target, instead of restaging the untouched root per member. Divergent sibling bumps to the same root (e.g. a per-member minor vs patch cascade) fail closed with a typed `release.version` conflict rather than silently last-writer-winning a version that earlier siblings already tagged.

## [0.1.0-alpha.8] - 2026-08-22

### Added

- Lock-step, forced, and first-release version bumps. A lock-step repository can now cut a first release — including brand-new, never-tagged modules — instead of failing with "no reachable release tag": `plan_bumps` seeds from `changed ∪ forced` and the planner forces override modules active. New force/level controls: `--set-version <VERSION>` (bare form applies workspace lock-step) and valueless `--patch` / `--minor` / `--major` (repo-wide level), with per-module overrides beating the workspace value. Bare `--set-version` still requires a value and the level flags are parser-scoped, so user argv is never rewritten (#195).

### Changed

- A module's declared/tagged version is now optional throughout the version→release pipeline: `VersionSource::declared_version` returns `AppResult<Option<Version>>` (no tag means "unreleased", not an error), with a `declared_version_required` default that fails closed with an actionable, module-named error only where a concrete version is genuinely required. The `Option` flows through `VersionInputs` / `BumpEntry` / `BumpResolution` / `ReleaseEntry` / `BumpModuleOutcome` / `Event::ModuleReleaseResolved` and the human/jsonl projections render `unreleased` / `initial release` rather than a bogus `X → X`. Pre-1.0 port-trait break absorbed within the workspace (Go/Rust adapters, testkit double, object-safety fixture) (#195).

## [0.1.0-alpha.7] - 2026-08-22

### Added

- Compute budget (`[toven].compute_budget`, per-ecosystem overrides, and `--compute-budget <auto|inherit|N>`) that bounds per-tool CPU parallelism during fan-out by injecting each unit's share of a host-sized thread budget through an env var (Go's `GOMAXPROCS`), never argv. Defaults to `auto`; `inherit`/`0` opts out. Cargo self-balances and is unaffected. Stops a per-module fan-out from oversubscribing toward cores² threads (#193).
- `toven-runtime`: a generic streaming unit-operation engine (shared GATHER, then per-unit results emitted as they settle, wave-scheduled and job-bounded). The `release` read/artifact verbs now stream per-item output instead of per-phase (#189).

### Changed

- Go `test` and `coverage` now default to the `Unordered` strategy (single wave, no build-order barriers), since `go test` resolves cross-module builds through Go's own cache; a 50-module `go test -race` run dropped ~153s → ~80s. `build`/`check` keep `LeafToTop` for compile fail-fast (#191).
- Path-to-module attribution is now per-caller: `run`/affected fails open, release gating fails closed, so a lockfile- or docs-only diff no longer over-publishes (#189).
- `release publish` now honors an accepted registry `Retry-After` for up to 15 minutes (was 2) before giving up, so a rate-limited publish can block that much longer instead of failing fast (#189).

### Fixed

- `entrypoint = "maintainer"` is now honored in the hosted-Release phase: Toven only verifies the Release exists for the tag (never creating, editing, or reconciling it) and fails closed when it is missing, instead of hitting a `CONFLICT` on maintainer-authored notes (#190).

## [0.1.0-alpha.6] - 2026-08-12

### Added

- Capability crates for versioning and VCS. A pure `toven-semver` toolkit owns the bump math and the release-tag codec/selection (`next_version`, `TagScheme`, `latest_matching`) over `rskit_version::semver`; a focused `toven-vcs` git-mechanism crate owns the rskit-git-backed `VcsReader`/`VcsWriter` adapter, the reusable diff foundation, and the per-repo reader-set fan-out; and a pure `toven-version` decision crate makes its git-free `plan_bumps` (independent bump → cascade floors → pre-skip released) the single path every `release plan`/`bump`/`tag`/`publish` version decision flows through (#163, #164, #171, #172, #174).
- User-declared composite units. `[units.<name>]` declares an ordered chain that composes existing units (`bump`, `tag`, `publish`, `coverage`, or another declared composite) into one named action, parsed and validated at load time and failing closed on an unknown member, a name that shadows a built-in unit, an empty chain, or a self/mutual cycle. This release adds declaration and validation only — composite execution lands in a follow-up (#178).
- System-wide `Unit`/`Backing` vocabulary in `toven-model` that generalizes the per-phase backing (`Argv` tasks, `Native` capabilities, `Delegated` tools, `Composite` chains) across bump, tag, publish, and coverage (#176).
- Flexible release tag modes and adapter-declared baselines: per-adapter release-baseline and tag-mode defaults, an umbrella/registry `BaselineSource` foundation, and a shared path-ownership resolver consumed by both affected selection and release change gating (#165, #166).

### Changed

- Hooks are unified behind one `HookRunner` that wraps any unit's pre/post hooks, replacing the per-verb hook plumbing (#177).
- Subprocess execution is consolidated into the focused `toven-exec` crate — the concrete `ProcessToolRunner`/`ProcessCommandRunner` (plus persistent spawn), a synchronous one-shot `ToolRunner` seam, and the shared CLI runner assembly (#167, #168).
- `toven-release` is slimmed to the release flow and composes `toven-version` for the bump decision; Go tag reads and federation sync now route through the `VcsReader` port so an in-memory VCS works everywhere; and the two engine crates were renamed to `toven-core`/`toven-release` (#169, #173, #174).
- Documentation now describes the new crate layers, the GATHER→DECIDE→MUTATE versioning path, tag modes, baseline sources, the change foundation, and the runner seams (#170).

### Fixed

- Umbrella baseline anchoring now anchors on the umbrella's own version and change-gates the maintainer echo. Making baseline anchoring a pure-function input (`VersionInputs::baseline`) rather than a step interleaved with the decision closes the two version-decision bugs that hid there and covers them with git-free regression tests (#171, #174).
- `release provenance` verify treats a 404 from the attestations endpoint as "absent" rather than a hard error (#162).

## [0.1.0-alpha.5] - 2026-08-09

### Added

- Change-gated, stage-only `release bump`: the verb now resolves an authoritative `module → post-bump version` map across the project and seeds only modules with a genuine diff since baseline, staging the manifest edits without committing so a tag-only run that rewrites nothing stages nothing. Symmetric per-verb hooks (`[hooks.bump]` / `[hooks.tag]` / `[hooks.publish]`) compose with the umbrella hooks in nested order (`[umbrella.pre, own.pre] → body → [own.post, umbrella.post]`), and a native, format-preserving, idempotent version-reference sync rewrites declared files' pinned version tokens to the post-bump versions during `bump` (rskit `Template` `{module}`/`{version}` placeholders, no `regex` dependency); a synced-only diff is treated as tool-generated and never re-triggers a bump (#159).
- A bump `on-resolved` hook seam: argv-first task references that run after every member's version decision and native version-reference sync but before staging, each handed the authoritative post-bump `module → version` map materialized to a generated file whose path is passed argv-first (no implicit shell). Such a hook can rewrite related files the native sync doesn't cover, and its edits join the same staged set as the manifests. The seam fails closed with no partial state — a failing hook, or a failing working-tree read on either side of it, restores every mutated member and deletes exactly the untracked files the hooks created (bounded by `MAX_UNTRACKED_PATHS`) so nothing wedges the next bump's clean-tree guard (#160).

### Changed

- `release provenance` is now a read-only, verify-only projection: attestation creation moves to the trusted builder (`actions/attest-build-provenance`) in the release workflow, and the verb asserts every published subject already carries a build-provenance attestation via the real `gh attestation verify` (file subjects by project-relative path, image subjects by `oci://` reference), failing closed when any subject is unattested. Being read-only, it no longer requires `--yes`, and its `--dry-run` reports subject presence (`present`/`missing`) without failing. Previously it shelled to nonexistent `gh attestation` surfaces and failed closed the first time CI exercised it (#158).
- Toven now self-hosts its release provenance and assembly verbs in CI: `release.yml` drives `toven release provenance` instead of a third-party action, and the self-canary is a 5-target build matrix plus a dogfood job exercising `package` → `checksums` → `sbom` → `sign` → `verify` → `provenance` over the real asset set. Keyless `release sign` is gated to manual `workflow_dispatch` to avoid stray public Rekor transparency-log entries on merges. Documentation was swept for accuracy and clarity across the command and config reference (#150).

### Fixed

- `release provenance` now classifies an unattested subject correctly: a current `gh` (>= 2.67.0) reports "no attestation exists for this digest" as an HTTP 404 from the repository attestations lookup endpoint and exits non-zero, which the verb previously misread as a hard failure and failed closed. The digest-absence 404 on the `/attestations/` endpoint is now treated as "missing" — distinct from an auth/repository 404, which still fails closed — so `--dry-run` reports unattested subjects as `missing` instead of erroring.

## [0.1.0-alpha.4] - 2026-08-08

### Added

- Release tags can be signed as annotated Git tags with `[ecosystems.<id>.release] sign_tags = true`. `tag_message` is required for signed tags, `sign_format` selects the Git signing backend (`openpgp`/`gpg`, `ssh`, or `x509`), and `signing_key` pins the non-secret key identifier while still allowing repository Git config inheritance.

### Changed

- Release APPLY stages exactly the manifest paths reported by the ecosystem release target before committing, so version bumps land in the release commit without unrelated working-tree files. Tag-only Go releases that rewrite no `go.mod` dependency floors tag the existing `HEAD` instead of creating an empty release commit.

## [0.1.0-alpha.3] - 2026-08-02

### Added

- `toven doctor`, a language-agnostic tool-audit command. It walks the resolved task graph, probes each task's required toolchain, and reports every tool as present or missing through the reporter sinks, exiting non-zero when any tool is missing. `--ensure` (and `--auto-install`) turns a missing tool into a typed, actionable error rather than fabricating or provisioning an installer — the verb only audits. The audit reuses the configure phase and per-task toolchain probes, so core stays tool-agnostic, and Toven's own CI workflows run it as a gate.
- One-line binary installers and package-manager distribution. `scripts/install.sh` (Linux/macOS) and `scripts/install.ps1` (Windows) install a released binary by pinned tag or, by default, the latest release — resolving the tag first, then verifying the archive's SHA-256 checksum (and the keyless Sigstore signature on `SHA256SUMS` when `cosign` is present) before extraction. Both are pipe-safe (`curl -fsSL … | sh`, `irm … | iex`). A templated Homebrew formula and Scoop manifest (`packaging/`) plus `scripts/gen-packaging.sh` render each channel from a release's `SHA256SUMS`, and `.github/workflows/publish-packages.yml` pushes them to the tap and bucket on every published release (a no-op until the `HOMEBREW_TAP_TOKEN` secret and distribution repositories are configured).

### Changed

- Toven drives its own tool gates through declared `toven.toml` tasks: structure (ast-grep), docs-build (mdbook), and deny (cargo-deny) are now tasks backed by per-task toolchain probes and task-aware ecosystem activation in the engine, so classified probe errors propagate for non-executable tools instead of being hand-rolled outside the task graph (#129).
- Hosted GitHub/GitLab release bodies are now a Conventional-Commit-grouped, `@handle`-attributed changelog walked from git commits, replacing the raw file-path dump. It is forge-agnostic and deterministic with no `CHANGELOG.md` dependency, and fully previewable via `toven release publish --dry-run` (#128).

## [0.1.0-alpha.2] - 2026-08-01

### Added

- Executable release `hooks`, backed by a generic, verb-agnostic `HookRunner` port (`toven-ports`). `[ecosystems.<id>.release] hooks` (`pre`/`post` task references) now run around the release instead of being rejected: every `pre` task runs — deduplicated and in configured order — before any mutation, so a non-success aborts the release fail-closed before any tag, push, or publish, and every `post` task runs only after a successful release (the reconcile early-return path skips post-hooks). The engine depends only on the injected `HookRunner` port and stays synchronous; the CLI's concrete `CliHookRunner` resolves each hook reference through the `run` verb. The port is a reusable seam — release is its first consumer, not its only possible one — with a `RecordingHookRunner` testkit double. Previously a non-empty `hooks` was rejected as not-yet-executable.
- Engine-owned release artifact pipeline as general, config-driven capability. A second version strategy, `manifest` (`[ecosystems.<id>.release] strategy = "manifest"`), cuts exactly the declared `v${Cargo.toml}` version when it is strictly ahead of the last release tag and fails closed otherwise — so successive `0.1.0-alpha.N` prereleases can be cut from a curated manifest, where the default `semver-cascade` always computes past them. New non-mutating verbs assemble and check the fixed `host.assets` set: `toven release package` archives an already-built binary into its declared per-target asset (via the rskit-fs archive primitive), `toven release checksums` writes a SHA-256 `SHA256SUMS` over every declared archive and the SBOM, and `toven release verify [--download]` presence/version-checks local archives or, in download mode, verifies the Sigstore signature on `SHA256SUMS` before each archive's checksum before extraction. `[ecosystems.<id>.release.sign]` is now executable — `toven release sign` produces the keyless Sigstore/cosign signature and certificate over `SHA256SUMS` (off by default; keyless default signer; `identity`/`issuer` as the non-secret keyless-verification inputs) — instead of rejecting `enabled = true`.
- Registry publish credential injection via `token_env`: `[ecosystems.<id>.release] token_env` now names the environment variable holding the registry token, and the publishing adapter reads it at the toolchain boundary and forwards the credential to its toolchain (for cargo, as `CARGO_REGISTRY_TOKEN` on the `cargo publish` child process — never on argv, in a log, or in engine memory). A configured-but-absent variable fails the publish closed rather than silently attempting an unauthenticated publish; `None` uses the toolchain's ambient credential. Previously `token_env` on a registry-published module was rejected as not-yet-honored.
- Release `visibility` (`[ecosystems.<id>.release] visibility`, one of `public`/`private`/`internal`, default `public`): the requested exposure resolves like the other release fields (ecosystem-inherited, per-module override, default public) and is enforced fail-closed at the registry-publish boundary. A non-public visibility against a public-only registry (crates.io publishes every version world-readable) is rejected at plan time with a typed `release.visibility` error, before any tag, push, or publish; the crates.io adapter enforces the same rule at the toolchain boundary as a last line of defense. Tag-only releases may carry any visibility. The tag push and hosted GitHub Release are visibility-agnostic — their exposure follows the remote repository, which Toven does not own — so visibility is recorded intent there, not a per-Release forge flag.
- GitLab hosted-release forge (`[ecosystems.<id>.release.host] forge = "gitlab"`, alongside the existing `github`): a `glab`-backed `ReleaseHost` adapter that cuts a GitLab Release argv-only, immutable create-or-verify via `glab release create --no-update` (an existing tag is refused, never edited) with notes piped through stdin and the token read from the ambient environment by `glab`. Because GitLab models releases differently from GitHub, the adapter honors what the platform can represent: a `draft = true` release is rejected fail-closed (GitLab has no draft), `prerelease` is recorded intent that carries no forge flag, and an existing Release is verified by title, notes, and asset name (GitLab release assets are links with no byte size). An unsupported `forge` remains rejected when the config is parsed.
- Named alternate cargo registry publication (`[ecosystems.<id>.release] registry = "<name>"` for any value other than `crates-io`): the Rust adapter routes the publish via `cargo publish --registry <name>` and injects the `token_env` credential into cargo's per-registry variable `CARGO_REGISTRIES_<NAME>_TOKEN` (never on argv), with crates.io publishing unchanged. A non-public `visibility` is allowed against a named registry — it is not the public-only crates.io, so its own access controls define the exposure — while crates.io still fails a non-public release closed.
- Branch-driven prerelease channels: a configured `[ecosystems.<id>.release.prerelease] branch_channels` map (release branch → channel) now selects the prerelease channel from the checked-out branch when no explicit `--pre` is given, so releasing from e.g. a `next` branch cuts a `beta` train without a per-run flag. An explicit `--pre <channel>` still wins, an unmapped branch or a detached HEAD cuts a stable release, and every mapped channel must be one of the declared `channels`. Previously a non-empty `branch_channels` was rejected as not-yet-executable.
- `push_branch` release setting under `[ecosystems.<id>.release]` (default `true`). When `false`, `release tag`/`release publish` push only the release tags and leave the branch ref untouched — the tag-only mode a protected release branch requires, where the version/CHANGELOG commit lands through a pull request. Toven's own release now sets it for protected `main`.
- The first directly downloadable Toven binary release pipeline: `.github/workflows/release.yml` builds a real per-target `toven` archive for every supported target (cross-compiling `aarch64-unknown-linux-gnu` through `cross`) with `toven release package`, then drives `toven release sbom`, `toven release checksums`, and `toven release sign` to assemble a CycloneDX SBOM, a combined `SHA256SUMS` covering it and every per-target archive, and that checksum file's keyless Sigstore/cosign signature and certificate. It then runs `toven release publish` behind the protected, manually approved `release` environment, attests GitHub build provenance over the published `SHA256SUMS`, and verifies every published asset with `toven release verify --download` (the keyless Sigstore signature on `SHA256SUMS` first, then each archive's checksum). `toven.toml`'s `[ecosystems.rust.release.host]` declares the fixed, version-free `dist/` asset paths this pipeline produces — every one engine-produced. CI provisions the tools (cosign, cargo-cyclonedx, `cross`) and holds the human approval gate; Toven drives them.
- On-disk content cache backend (`FsContentCache` in `toven-engine`): a synchronous, content-addressed presence cache built on rskit-fs atomic writes that implements both injected cache ports — the read-only `CacheStore` queried by PLAN and the write-only `CacheWriter` driven by APPLY — so cache verdicts and successful-run records persist across invocations without bridging an async runtime into the pure planner.
- Task-level `shared_inputs` for broad cache invalidation and the initial installed-binary benchmark harness scaffold.
- APPLY execution over the planned unit graph, including fail-closed dependency gating, fail-fast cancellation, persistent readiness/teardown lifecycle, live persistent raw output routing, safe explicit command environment policy, and successful-run cache recording.
- User-facing `toven init` workflow with safe stdout/write modes, deterministic TOML rendering, and Rust adapter config contributions.
- Project/group/scope adapter configuration, Rust multi-manifest discovery, adapter-owned default Rust tasks, and explicit cross-scope dependency overlays.
- Developer workflow inspection commands, watch mode, persistent task readiness, JSONL run events, and cache stats/clean.
- Task execution, local successful-run cache records, cache-hit skipping, `toven explain`, and opt-in cached passthrough args via `cache_args = true`.
- Git-baseline affected-module planning with reverse-dependent closure, root-file fail-closed behavior, and `toven affected` / `plan` CLI surfaces.
- Strict `toven.toml` loading, normalized workspace/group/task config, and filesystem preset resolution backed by rskit config, validation, and filesystem utilities.

### Changed

- Release change detection anchors only on a module's own release tag. `[project].base_ref` / `[[members]].base_ref` no longer stand in as a release baseline (an explicit `--base` still overrides the diff ref), so a repository with no release tag plans a real first release instead of diffing the release branch against itself and reporting nothing to release. A never-released module now joins the plan with reason `initial-release` and cuts the version it already declares rather than bumping past it, so `0.1.0-alpha.1` is released as `0.1.0-alpha.1`; explicit `--patch`/`--minor`/`--major`/`--set-version`/`--pre` still win.
- One release tag is now one hosted Release: modules that a module-free `tag_format` (e.g. `v{version}`) collapses onto the same tag produce a single hosted Release with the deduplicated union of their assets and notes, instead of one identical Release plan per module. Modules sharing a tag but disagreeing on `draft`/`prerelease` are a typed configuration error. A planned version carrying a prerelease identifier now marks the hosted Release as a prerelease even without an explicit `--pre` channel.
- Build provenance for Toven's own binaries is attested inside the approved `publish` job of `.github/workflows/release.yml`, over the published `SHA256SUMS` subjects. The former tag-triggered `provenance` job in the release-readiness workflow is removed: `toven release publish` creates the tag with `GITHUB_TOKEN`, which never triggers a workflow, and that job re-built a different artifact set than the one published.
- Reuse rskit foundations instead of hand-rolled standard-library code: content hashing for cache keys and source digests now goes through the new `rskit_util::hash` helper (replacing direct `blake3` use), the source-tree digest walk uses `rskit-fs` `sync_io::tree::walk_tree` (replacing a hand-rolled recursive `std::fs::read_dir`), and the default APPLY environment reads `PATH` via `rskit_util::env::get`. No behavior change.
