# Lens: rskit reuse

Generated code tends to write a local helper instead of finding the owner, so assume duplication until shown otherwise. Rule: [reuse rskit first](../../../../docs/engineering.md#reuse-rskit-first). Owners: [concern ownership](../../../../docs/concern-owners.md).

## Check

- **Errors.** rskit `AppError` / `AppResult` with the cause kept. A local error enum, `thiserror` type, or `String` error for a shared concern is duplication.
- **Filesystem, Git, process.** Direct `std::fs`, `Command::new`, or shelling out to `git` where rskit or `toven-exec` already owns the concern.
- **Config and validation.** Go through the `Document` loader and rskit validation, not a fresh `toml::from_str`.
- **Hashing and helpers.** Use `rskit_util::hash` and existing `toven-ports` helpers instead of calling `blake3` directly.
- **Near-copies.** One changed line is still a fork; enhance the owner.

If rskit has an owner and the code ignores it: **blocker**. If rskit lacks the capability, the fix is a generic rskit enhancement; a local solution is a **should-fix** with an "upstream to rskit" note. Name the concrete rskit module the code should use.

## Detection starters

```bash
rg -n 'std::fs::|std::process::Command|Command::new' crates/*/src
rg -n 'thiserror|impl .*Error for' crates/*/src
rg -n 'toml::from_str|serde_json::from_str' crates/*/src
rg -n 'blake3|sha2|Sha256' crates/*/src
```
