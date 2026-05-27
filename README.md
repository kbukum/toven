# Toven

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![CI](https://github.com/kbukum/toven/actions/workflows/ci.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/ci.yml)
[![Supply Chain](https://github.com/kbukum/toven/actions/workflows/supply-chain.yml/badge.svg)](https://github.com/kbukum/toven/actions/workflows/supply-chain.yml)

Toven is a fast, argv-first development and CI task planner for multi-module
repositories. It discovers workspace modules, orders work by dependency graph,
and renders reviewable command batches before execution.

## Status

**Pre-alpha.** The current implementation focuses on deterministic planning:
configuration loading, preset resolution, Rust workspace discovery, dependency
batching, and human-readable plan output. Command execution, cache-backed
skipping, and additional language adapters will be added in follow-up phases.

Toven is not published to crates.io yet. Until the first alpha release, install
from source after cloning the repository.

## Design

- **Language-agnostic engine** — scheduling and planning stay separate from
  language-specific discovery.
- **Explicit argv rendering** — generated commands are argument vectors by
  default; shell execution must be opted into intentionally.
- **Preset catalog** — reusable task definitions are TOML data, not hard-coded
  command branches.
- **Real repository fixtures** — integration fixtures dogfood Toven against the
  sibling kits (`rskit`, `gokit`, and `pykit`) as language support lands.

## Local development

```bash
make check
make coverage
cargo run -- --help
```

The current scaffold builds as a standalone Rust CLI. Toven will add the
planning engine, language discovery, preset resolution, and execution wiring in
focused follow-up pull requests.

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
