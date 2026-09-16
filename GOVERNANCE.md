# Governance

How Toven makes decisions, who can make them, and how releases and security issues are handled.

## Project Status

Toven is in **alpha**. Backward compatibility is not guaranteed while the core planner, execution model, cache semantics, and discovery adapter interfaces are still settling. Breaking changes are acceptable when they produce a cleaner long-term design.

## Roles

### Contributors

Anyone who opens an issue, discussion, or pull request is a contributor. Contributors are expected to follow the [Code of Conduct](CODE_OF_CONDUCT.md) and the [Contributing Guide](CONTRIBUTING.md).

### Reviewers

Reviewers are contributors who have shown sustained engagement and are trusted to review pull requests in specific areas of the project.

### Maintainers

Maintainers have merge rights and are responsible for project direction, security review, releases, and community health. The current list is maintained in [MAINTAINERS.md](MAINTAINERS.md).

## Decision Making

Routine fixes and focused features land through pull request review. Significant architectural changes should start as an issue or discussion before implementation.

Changes that affect the execution model, cache behavior, security posture, release process, or discovery adapter protocol require maintainer consensus.

## Release Process

Releases are cut by maintainers. Each release includes a changelog entry. The `Release` workflow is dispatched manually and gated by a protected environment's required-reviewer approval; it creates the version tag itself and attaches signed build artifacts, an SBOM, and build provenance. See [self-hosting and CI](docs/self-hosting.md).

## Security Issues

Security issues follow the dedicated process in [SECURITY.md](SECURITY.md) and are not handled through public issues.

## Amendments

This document can be updated through a pull request.
