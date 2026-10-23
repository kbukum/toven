# Lens: tests and proof

Tests written after the fact often assert only the path the author already saw working. Rules: [tests and proof](../../../../docs/engineering.md#tests-and-proof). Harness: [testing](../../../../docs/testing.md).

## Check

- **Same change.** Every behavior change ships with its test. A fix has a regression test that fails without it.
- **Failure paths.** Typed errors are asserted, not only success.
- **Behavioral.** Assert plans, output, and errors, not internal call order unless order is the contract. Output tests assert exact ordering and escaping, not loose `contains`.
- **Deterministic.** No real network, wall clock, or working-directory dependence.
- **Fixtures and doubles.** `toven-testkit` fixtures instead of large inline TOML; the shared port double instead of a one-off fake.
- **Coverage.** `make coverage` holds the thresholds in `toven.toml`.

## Detection starters

```bash
rg -n '\[ecosystems|\[tasks|\[release' crates/*/tests
rg -n 'struct \w*(Fake|Mock|Stub|Dummy)' crates/*/tests crates/*/src
ls crates/toven-testkit/src/doubles
cargo nextest run -p <crate> --all-features
make coverage
```
