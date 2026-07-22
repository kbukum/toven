# Release migration

Migrate a repository to Toven without replacing a working release path before parity is proven.

## Migration stages

1. Inventory the current release behavior.
2. Configure equivalent Toven release policy.
3. Run representative local fixtures.
4. Compare Toven previews against isolated copies of the real repository.
5. Publish the first Toven binary.
6. Run a direct-binary CI canary.
7. Adopt a pinned, checksum-verifying `toven-action`.
8. Remove competing release logic after parity.

## Inventory

Record:

- module selection rules
- version and prerelease rules
- dependency cascades
- tag grammar
- changelog requirements
- registry targets
- hosted assets
- signing, SBOM, and provenance
- approval and permissions
- partial-publication recovery

Every retained behavior must map to Toven configuration, a Toven command, or an explicitly native gate.

## Preview parity

Run:

```bash
toven release plan --output jsonl
toven release readiness --output jsonl
toven release sbom --out-dir target/toven/release/sbom
toven release depgraphs --out-dir target/toven/release/depgraphs
toven release publish --dry-run --output jsonl
```

Compare selected modules, target versions, cascade reasons, tags, order, prerelease classification, and hosted assets.

## Isolation

Use temporary clones or worktrees with local Git remotes and controlled registry or forge doubles. Preview verification must not mutate source repositories, manifests, tags, registries, or hosted releases.

## Bootstrap dependency

Downstream CI cannot install a released Toven binary until Toven has published one. Complete Toven's first binary release before requiring it in rskit or gokit.

## CI canary

The first CI integration should download a versioned binary directly and verify its checksum. Keep the existing release implementation available for comparison.

After the direct contract is proven, replace duplicated installation snippets with the pinned action.

## Cutover

Cut over only when:

- fixture and real-repository previews agree
- dry-runs are mutation-free
- safety failures are covered
- released binaries install and run in CI
- action results match direct invocation
- recovery procedures are documented

Remove obsolete scripts, tests, workflow branches, and documentation in the same change. Keep native developer commands only when they do not compete with release behavior.

## Recovery

Never rewrite published versions or pushed release tags. Inspect current state with:

```bash
toven release status
```

Correct the repository and publish a forward-fix version for incomplete or inconsistent release trains.
