# Security Policy

# Security Policy

Report vulnerabilities privately through GitHub security advisories for
`kbukum/toven`. Do not open public issues for suspected vulnerabilities.

Toven treats user commands and repository files as untrusted inputs at the CLI
boundary. Core behavior is argv-first, shell execution must be explicit, and
cache records only suppress work after a successful verification.

Dependency policy is enforced with cargo-deny, pinned GitHub Actions, minimal
workflow permissions, SBOM generation in release automation, and Sigstore/SLSA
release provenance targets.
