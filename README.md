# Toven

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/kbukum/toven/actions/workflows/ci.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/ci.yml)
[![Supply Chain](https://github.com/kbukum/toven/actions/workflows/supply-chain.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/supply-chain.yml)
[![Release Readiness](https://github.com/kbukum/toven/actions/workflows/release-readiness.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/release-readiness.yml)

Toven is a fast, argv-first development and CI task planner for multi-module
repositories. It discovers workspace modules, orders work by dependency graph,
and renders reviewable command batches before execution.

## Status

**Pre-alpha.** The current implementation includes strict configuration loading,
filesystem preset resolution, Rust workspace discovery, dependency-aware batching,
affected-module planning, and human-readable plan output. Command execution,
cache-backed skipping, and additional language adapters will be added in
follow-up phases.

Toven is not published to crates.io yet. Until the first alpha release, install
from source after cloning the repository.

## Design

- **Language-agnostic engine** — scheduling and planning stay separate from
  language-specific discovery.
- **Explicit argv rendering** — generated commands are argument vectors by
  default; shell execution must be opted into intentionally.
- **Preset catalog** — reusable task definitions are TOML data, not hard-coded
  command branches.
- **Real repository fixtures** — integration fixtures dogfood Toven against
  checked-in kit submodules, starting with `rskit`, as language support lands.

## Local development

```bash
git submodule update --init --recursive
make check
make coverage
make release-artifacts
cargo run -- --help
```

The current scaffold builds as a standalone Rust CLI with configuration,
preset-loading, Rust discovery, affected detection, and reviewable planning
foundations. Toven will add execution wiring and cache-backed skipping in
focused follow-up pull requests.

## Smoke testing real repositories

Toven has two smoke modes:

- Managed smoke tests run the real `toven` binary against committed cases under
  `smoke/cases/` and compare normalized output with `smoke/expected/`.
- Ad-hoc smoke runs let contributors test any local or cloned repository without
  committing an expectation file.

```bash
make smoke
make smoke-repo REPO=/path/to/repo ARGS="--release"
make smoke-clone URL=https://github.com/kbukum/rskit NAME=rskit
make smoke-add-submodule URL=https://github.com/example/repo NAME=repo
make smoke-add-case NAME=repo REPO=smoke/repos/repo ARGS="--all-features"
make smoke-add-managed-submodule URL=https://github.com/example/repo NAME=repo ARGS="--all-features"
TOVEN_SMOKE_BLESS=1 make smoke-update NAME=rskit-core
make smoke-purge NAME=rskit
```

Managed cases should use stable public repositories or pinned submodules. Local
scratch clones live under ignored `.toven/smoke/repos/`; committed smoke
repositories belong under `smoke/repos/`. Managed tests run against temporary
copies and verify both normalized binary output and Cargo workspace dependency
waves.

## Configuration preview

Toven loads strict TOML from `toven.toml`. Unknown fields are rejected early,
workspace roots are resolved relative to the config file, and command templates
are validated before planning.

```toml
[workspace]
name = "demo"
root = "."

[profiles.rust]
language = "rust"
execution = "batch-ready"
module_arg_template = ["-p", "{module.package}"]
resource_group = "cargo:{workspace.root}"

[profiles.rust.tasks]
test = { argv = ["cargo", "test", "{module.args}", "{args}"] }
```

Tasks can also reference preset files. Project presets are resolved before user
presets from `.toven/lang/<language>/presets/<name>.toml`; user presets use the
same layout under the current user's home directory.

Affected planning narrows a plan to modules changed since a git baseline plus
their reverse dependents:

```bash
cargo run -- plan --affected --base origin/main --merge-base
cargo run -- affected --base origin/main --merge-base
```

Set `workspace.base_ref` in `toven.toml` to provide a default baseline. Without
`--base` or `workspace.base_ref`, affected detection compares `HEAD` to `HEAD`
and only local staged, unstaged, and untracked changes are considered.

`make release-artifacts` stages the crates.io package and checksum manifest in
`dist/`. CI also generates a CycloneDX SBOM and checks Sigstore tooling without
publishing the crate; version-tag runs attach GitHub provenance attestations.

## Community

Contributions and issue reports are welcome. Please read:

- [Contributing Guide](CONTRIBUTING.md)
- [Code of Conduct](CODE_OF_CONDUCT.md)
- [Security Policy](SECURITY.md)
- [Governance](GOVERNANCE.md)
- [Maintainers](MAINTAINERS.md)

## Repository workflow

Changes use Conventional Commits and small pull requests. Start with the
community and policy files on `main`, then add implementation, CI, and real
fixture coverage through focused review branches.

## License

Toven is distributed under the terms of the [MIT License](LICENSE).
