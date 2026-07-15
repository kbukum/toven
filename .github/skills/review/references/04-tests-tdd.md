# Pass 04 — TDD and tests

Toven's bar: behavioral, deterministic, failure paths covered, a regression test for every fix, tests added in the **same** change. This pass catches the classic vibe-coding tell — tests written after the fact that only assert the happy path the author already saw working.

> **Run in a separate, clean-context agent** — never inline in the session that wrote the code. An independent reviewer re-derives every judgment from the code and the principles instead of trusting prior reasoning. A plan/spec may be passed in as a scope checklist only; it never excuses a baseline violation.

**Scope note.** *Changes mode:* every behavioral change in the diff must ship its test in the same diff. *Project mode:* assess coverage of each crate's public behavior and failure paths, audit for inline config strings and stranded doubles, and confirm `make coverage` holds.

## Checks

- **Test in the same change.** Every behavioral change has a corresponding test in the same diff. A feature/fix with no test is a blocker — this is the TDD failure to catch.
- **Regression test per fix.** Every bug fix has a regression test that fails without the fix and passes with it.
- **Failure paths tested.** Not just the happy path. Typed errors are asserted; there is no panic-on-error path in production code standing in for error handling.
- **Fixtures, not inline TOML.** Tests use `toven-testkit` fixtures and declarative case files. Large inline TOML/config strings in a test are a should-fix — move them to fixtures (e.g. `toven_testkit::document_path(rel)` for on-disk config fixtures). Add/extend a fixture rather than embedding config text.
- **Behavioral, not implementation-coupled.** Assert observed plan/output/error, not internal call order — unless that ordering *is* the contract.
- **Determinism, no network.** No real network access; no time/ordering flakiness.
- **One shared double per port.** Each port is exercised through its single `toven-testkit` double, not a one-off local mock. A bespoke inline mock duplicating a port's behavior is a should-fix (and a pass `00` placement smell).

## The vibe-coding tell

If tests were clearly written *after* the implementation and only assert the happy path the author already saw working — no failure-path assertions, no regression case for the bug being fixed — call it out. That is the signal that TDD was not followed, even when coverage numbers look fine.

## Detection starters

```bash
# inline TOML/config in tests instead of fixtures
rg 'r#"|"""|toml::from_str' crates/*/tests crates/*/src --glob '*test*'
rg '\[ecosystems|\[tasks|\[release' crates/*/tests
# bespoke mocks that should be the shared double
rg 'struct .*(Fake|Mock|Stub|Dummy)' crates/*/tests
# which ports have doubles
ls crates/toven-testkit/src/doubles
```

Then run the focused crate tests (e.g. `cargo test -p toven-engine -q`) and `make coverage` for the gate.
