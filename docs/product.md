# Core concepts

Toven is an argv-first planner and executor for repositories containing multiple modules. It adds repository discovery, graph-aware selection, scheduling, caching, coverage aggregation, and release coordination without taking ownership of the commands a repository runs.

## Repository-owned commands

Tasks are named entries in `toven.toml`. Each task contains an argv template and scheduling policy. Toven validates and expands selectors but does not infer hidden flags or invoke a shell unless shell execution is explicitly configured.

```toml
[ecosystems.rust.tasks.test]
argv = ["cargo", "nextest", "run", "--manifest-path", "{module.manifest}", "{module.selector}", "{args}"]
selector = ["-p", "{module.package}"]
fan_out = "batchable"
```

Running `toven test --nocapture` appends `--nocapture` at `{args}` unchanged.

## Modules and workspaces

A module is the smallest discovered unit Toven plans. Rust modules are Cargo packages. Go modules are identified by their repository-relative module roots. A workspace groups modules discovered from the same Cargo workspace or Go workspace.

Canonical module references include the ecosystem:

```text
rust:toven-engine
go:cache-redis
```

## Dependency graph

Adapters derive native dependency edges from Cargo or Go metadata. Explicit overlays describe cross-ecosystem edges that native tools cannot prove. The resulting graph controls affected selection, dependency expansion, execution waves, and release cascades.

## Plan and apply

Every task follows two phases:

1. **Plan:** load configuration, discover modules, build the graph, select scope, render argv, and decide cache use.
2. **Apply:** execute planned units, observe readiness, update successful cache records, and report results.

Read-only commands stop after planning. Task commands apply unless `--dry-run` or `--explain` is set.

## Affected work

With a Git baseline, Toven maps changed paths to modules and includes their dependents. Shared or repository-level inputs can activate the full graph because they may affect every module.

```bash
toven affected test --base origin/main --merge-base
```

## Execution waves

Dependency-respecting tasks run dependencies before dependents. Independent units in one wave run concurrently. Tasks configured with `run_strategy = "unordered"` may run the selected scope in one wave.

## Cache

Successful, cacheable units are reused only when source content, dependency results, task configuration, shared inputs, toolchain identity, and opted-in passthrough arguments still match. Persistent and state-changing tasks are not cached.

See [cache management](commands/cache.md).

## Release product

Toven's release product is a reviewable decision followed by guarded mutation. The maintainer sees what changed, which modules join through dependency cascades, each proposed version and tag, changelog evidence, readiness results, publication order, hosted assets, and prerelease state before approval.

Rust repositories use independent crate versions and can choose registry or tag-only outcomes. Go repositories release changed modules through root or path-prefixed tags and explicitly classify test-only and benchmark modules rather than relying on path heuristics. Toven itself is a tag-only Rust workspace distributed as compiled binaries; none of its crates are published to crates.io.

The final safety contract requires mutation-free previews, explicit approval, clean release trees, immutable published results, and forward-fix recovery. Some policy surfaces are defined but not executable yet; the exact boundary is maintained in [release configuration](config/release.md) and the [release workflow](commands/release.md).

## Toven distribution

The first Toven release is the alpha prerelease `v0.1.0-alpha.1`. It will provide checksum-verified, keyless Sigstore-signed binaries for Linux x86-64 and ARM64 with glibc, macOS x86-64 and Apple silicon, and Windows x86-64. The hosted Release also carries a CycloneDX SBOM and build provenance.

Until those assets exist, install Toven from source. See [installation](installation.md).

## Output contract

- stdout: requested tables, generated projections, and JSONL
- stderr: progress, child output, warnings, summaries, and errors

This separation lets automation parse stdout without losing human diagnostics. See [command output](commands/README.md#output-streams).
