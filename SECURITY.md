# Security Policy

## Supported Versions

Toven is pre-alpha: no versions have been tagged or published to crates.io yet. Until the first prerelease is cut, security fixes target the `main` branch only. Once prereleases exist, this section will list the supported prerelease lines.

| Version | Supported |
|---------|-----------|
| `main` (development branch) | :white_check_mark: |
| Tagged releases | None yet |

## Reporting a Vulnerability

If you discover a security vulnerability in Toven, please report it **privately** through [GitHub Security Advisories](https://github.com/kbukum/toven/security/advisories/new), which opens a private disclosure thread visible only to maintainers.

Do **not** open a public GitHub issue for security reports.

### What to Include

- A clear description of the issue and its potential impact.
- Steps to reproduce, including a minimal proof-of-concept if possible.
- The affected version(s), `toven.toml` shape, and `rustc` toolchain.
- Any suggested mitigations or fixes.

### Response SLA

| Step | Target |
|------|--------|
| Acknowledgment | 48 hours |
| Review & severity | 5 business days |
| Fix available | 30 calendar days (critical), 90 days (high/medium) |
| Public disclosure | 90 days after report (coordinated with reporter) |

### What to Expect

- **Acknowledgment** within 48 hours of your report.
- **Status update** within 5 business days with an assessment.
- **Fix timeline** communicated once the issue is confirmed.
- **CVE assignment** for confirmed vulnerabilities affecting released versions, requested via GitHub Security Advisories. Where applicable, a matching [RUSTSEC](https://rustsec.org/) advisory is also filed.
- **Credit** in the release notes and the advisory (unless you prefer to remain anonymous).

### Disclosure Policy

- We follow [coordinated disclosure](https://en.wikipedia.org/wiki/Coordinated_vulnerability_disclosure).
- Please allow a reasonable embargo period (typically 90 days) before any public disclosure, extendable by mutual agreement when a fix requires coordination.
- Once a fix is released, the advisory is published and any CVE / RUSTSEC IDs are made public.

## Threat Model

Toven runs against untrusted repositories and forwards user-authored commands, so the security boundary is the CLI input surface:

- **Untrusted inputs:** `toven.toml`, repository files, and passthrough argv are treated as untrusted. Strict config loading rejects unknown fields early, and project roots are resolved relative to the config file.
- **argv-first execution:** generated commands are argument vectors by default; shell execution must be opted into explicitly, never inferred.
- **Cache integrity:** cache records only suppress work after a successful verification — a cache hit never fabricates a success that did not happen.
- **No secret leakage:** Toven does not log secrets and bounds the input/output it captures from subprocesses.

## Security Best Practices for Users

- Keep dependencies current and run `make deny` before shipping dependency changes.
- Review `cargo deny` advisory and license findings regularly; CI runs supply-chain checks on changes that affect Rust code, Cargo metadata, or workflow inputs.
- Treat `toven.toml` as code: review changes to command templates and adapters as carefully as source.
- Never commit secrets — use environment variables or a secret manager.

## Supply Chain

- All GitHub Actions used in CI are pinned to commit SHAs (see `.github/workflows/`).
- Dependency updates are automated via Dependabot ([`.github/dependabot.yml`](.github/dependabot.yml)).
- The Rust toolchain is pinned via `rust-toolchain.toml`; CI enforces this rule. `Cargo.lock` is committed and checked via locked Cargo operations.
- Supply-chain policy is enforced by [`cargo-deny`](https://embarkstudios.github.io/cargo-deny/) via [`deny.toml`](deny.toml): advisory database, license allowlist, banned crates, and source allowlist.
- Release automation generates an SBOM and targets Sigstore signing and SLSA provenance for published artifacts.
