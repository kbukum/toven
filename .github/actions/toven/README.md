# `toven` action

Install a released, integrity-verified Toven binary in a GitHub Actions workflow and, optionally, run a Toven command with it. The action is a thin, reusable wrapper around Toven's single reference install contract, [`scripts/install.sh`](../../../scripts/install.sh): it downloads the pinned release archive, verifies the archive checksum and — with `cosign` present — the keyless Sigstore signature over `SHA256SUMS`, then forwards the requested command unchanged. It is a convenience and consistency layer, not a second release engine, and it never publishes.

## Usage

Pin the action by the commit SHA of a Toven release. It installs the binary of that same release:

```yaml
- uses: kbukum/toven/.github/actions/toven@<release-commit-sha> # v0.1.0-alpha.3
  with:
    args: modules
```

Install only, then invoke Toven yourself (the binary is on `PATH` and exposed as an output):

```yaml
- id: toven
  uses: kbukum/toven/.github/actions/toven@<release-commit-sha> # v0.1.0-alpha.3
- run: "${{ steps.toven.outputs.toven }}" graph
```

## Inputs

| Input | Default | Description |
|---|---|---|
| `version` | `""` (the action's release) | Immutable release tag to install (e.g. `v0.1.0-alpha.3`). Empty installs the release the pinned action commit belongs to, read from the workspace version in Toven's root `Cargo.toml` at that SHA. Set it only to install a different release. |
| `repo` | `kbukum/toven` | Source repository that publishes the release. |
| `target` | `""` (auto) | Override the auto-detected Rust target triple. |
| `install-dir` | `""` (auto) | Install directory. Empty derives a versioned tool-cache path when caching, else a temp directory. |
| `args` | `""` | Optional Toven command to run after install, forwarded unchanged to one `toven` invocation. Word-split into argv — pass only trusted, workflow-authored arguments. |
| `working-directory` | `.` | Directory to run the optional `args` command in. |
| `install-cosign` | `true` | Install `cosign` so the installer can verify the Sigstore signature over `SHA256SUMS`. Controls only whether cosign is installed — the installer verifies whenever cosign is present, and the checksum is always enforced. It does not disable verification. |
| `cache` | `true` | Reuse a previously installed binary from `${RUNNER_TOOL_CACHE}/toven/<version>/<target>`. A hit requires an exact version match and skips re-download. |

## Outputs

| Output | Description |
|---|---|
| `toven` | Absolute path to the installed binary. |
| `version` | Version string reported by `toven --version`. |
| `cache-hit` | `"true"` when the binary was served from the runner tool cache. |

## Pinning and integrity

- **Pin the action by the commit SHA of a release**, never by a moving tag — `uses: kbukum/toven/.github/actions/toven@<release-commit-sha>`. The trailing `# vX` comment is documentation only; the SHA is the trust anchor. Dependabot's `github-actions` updates bump the SHA and the comment together.
- **The SHA pins the binary too.** With no `version` input, the action installs the release named by Toven's root `Cargo.toml` at the pinned SHA, so the action and binary cannot drift apart. Pin a non-release commit and the install fails because that release does not exist. If the version cannot be read, the action fails rather than falling back to the latest release.
- **Integrity is verified on install.** The bundled `scripts/install.sh` always enforces the `SHA256SUMS` checksum, and when `cosign` is present it verifies the keyless Sigstore signature over `SHA256SUMS` first. There is no second, parallel verification path — the action and the direct-download procedure share one contract.
- **A cache hit reuses a previously verified binary, it does not re-verify.** With `cache: true`, a version-exact hit at `${RUNNER_TOOL_CACHE}/toven/<version>/<target>` reuses a binary this runner already installed and verified, skipping the download, checksum, and signature checks. On GitHub-hosted runners the tool cache is per-job, so a hit means an earlier step in the same job installed and verified it; on a persistent (self-hosted) tool cache it trusts a prior verified install of that exact version. Set `cache: false` to force a fresh, fully verified install every run.

## Relationship to the direct-download procedure

The action reproduces, and does not replace, the reference direct download documented in [`docs/self-hosting.md`](../../../docs/self-hosting.md). Repositories that prefer an explicit `curl … | sh -s -- --version <tag>` step may keep it; the action collapses that step into a single pinned `uses:` line with the same integrity guarantees, plus a runner tool-cache and typed outputs. Real publication stays behind each repository's approved release environment — the action only installs and runs read-only or dry-run commands.
