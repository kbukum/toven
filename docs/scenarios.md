# Worked scenarios

## Inspect before execution

```bash
toven modules
toven graph
toven plan test
toven explain test
```

Use this sequence after onboarding or after changing task configuration. The first three commands are read-only; `explain` shows the exact rendered argv.

## Run only affected tests

```bash
toven test --base origin/main --merge-base
```

Toven diffs the merge base against the working tree, selects changed modules, adds their dependents, and executes the resulting graph in dependency order.

## Focus on one module and its dependencies

```bash
toven test --module rust:cli --dependencies
```

The selected module and everything it requires are planned. Use `--dependents` instead to include modules that require it.

## Rebuild a cached result

```bash
toven test --module rust:core --refresh
```

Existing records are ignored. Successful results replace the previous records. Use `--no-cache` when the run must neither read nor write cache data.

## Produce machine-readable output

```bash
toven modules --output jsonl > modules.jsonl
toven release plan --output jsonl > release-plan.jsonl
```

JSONL records use stdout. Warnings and errors remain on stderr.

## Rehearse a release

```bash
toven release plan
toven release readiness
toven release sbom --out-dir target/toven/release/sbom
toven release depgraphs --out-dir target/toven/release/depgraphs
toven release publish --dry-run
```

The sequence inspects versions, runs release checks, creates local evidence artifacts, and previews publication without changing manifests, tags, registries, or hosted releases.

## Verify release features end-to-end (dry-runs, cascades, and failure guards)

To test the entire Toven release platform end-to-end under realistic local fixtures, use:

```bash
./scripts/verify-release-platform.sh
./scripts/verify-real-repositories.sh
```

These scripts verify that previews do not mutate tags or files, cascading requirements are updated correctly, and bad states are caught before committing or pushing.
