# Packaging

Distribution manifests for the released Toven binary. Each is a thin, verifiable
projection of an already published, signed GitHub Release: archive URLs point at
the immutable release tag and every hash comes from that release's
`SHA256SUMS`. Nothing here builds or re-signs binaries.

## Layout

- `homebrew/toven.rb.template` — Homebrew formula, rendered per release.
- `scoop/toven.json.template` — Scoop manifest, rendered per release.
- `../scripts/gen-packaging.sh` — renders both from a version tag and a
  `SHA256SUMS` file.

## How manifests are published

`.github/workflows/publish-packages.yml` runs when a release is published (or on
manual dispatch): it downloads the release `SHA256SUMS`, runs `gen-packaging.sh`,
and pushes the rendered files to the distribution repositories. It is a no-op
until the maintainer configures `HOMEBREW_TAP_TOKEN`, so it is safe to keep
enabled beforehand. Each channel is also published independently — a
distribution repository that does not yet exist is detected and skipped — so the
Homebrew tap can go live before the Scoop bucket (or vice versa).

## One-time maintainer setup

1. Create `kbukum/homebrew-tap` (the tap) and `kbukum/scoop-bucket` (the
   bucket) as public repositories. Either may be created first; the workflow
   skips whichever is still missing.
2. Create a fine-grained PAT with `contents:write` on both repositories and add
   it to this repository's Actions secrets as `HOMEBREW_TAP_TOKEN`.

After that, `brew tap kbukum/tap && brew install toven` (or the one-shot
`brew install kbukum/tap/toven`) and
`scoop bucket add toven https://github.com/kbukum/scoop-bucket; scoop install toven`
track every release automatically.

## Render locally

```bash
scripts/gen-packaging.sh v0.1.0-alpha.2 dist/SHA256SUMS build/packaging
```
