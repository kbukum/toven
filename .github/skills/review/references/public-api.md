# Lens: public API and stability

Tools depend on Toven's machine contracts, and every `pub` item is surface to maintain. Rules and the stability policy: [public API and stability](../../../../docs/engineering.md#public-api-and-stability).

## Check

- **Surface size.** New `pub` items are intentional; prefer `pub(crate)`. No `Any`-style escape hatch or stringly-typed return.
- **Types.** Borrow in parameters (`&str`, `&[T]`) when ownership is not needed. Growable public enums are `#[non_exhaustive]`. Long positional argument lists become a parameter struct or builder.
- **Errors.** Typed `AppError` with the right kind and cause; messages a user can act on.
- **Test-only surface.** No production accessor that exists only for tests.
- **Machine contracts.** Changes to the JSONL event stream, [exit codes](../../../../docs/commands/README.md#exit-codes), or the strict `toven.toml` schema are breaking. They need a `CHANGELOG.md` entry, updated docs, and a test that pins the new contract.
- **Breaking by design.** Toven is pre-stable: a breaking change is fine when it gives a cleaner design, but it must be recorded, never hidden behind a shim.

## Detection starters

```bash
git diff <base>...HEAD -- 'crates/*/src' | rg '^\+\s*pub (fn|struct|enum|trait|type|mod)'
rg -n -B1 'pub enum' crates/*/src
rg -n 'pub fn (into_inner|into_sink)' crates/*/src
git diff <base>...HEAD --stat -- docs/commands crates/toven-core/src/config crates/toven-model/src
```

For the `pub enum` hits, check the line above for `#[non_exhaustive]`. No semver-checks gate exists; see [known gaps](../../../../docs/engineering.md#known-gaps).
