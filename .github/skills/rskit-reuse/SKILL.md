---
name: rskit-reuse
description: "toven: Reuse or enhance rskit before adding shared infrastructure to Toven."
user-invocable: true
---

# rskit reuse

[rskit](https://github.com/kbukum/rskit) is a general-purpose framework vendored in the `rskit/` submodule. Toven reuses its owners for shared concerns. The rule is [reuse rskit first](../../../docs/engineering.md#reuse-rskit-first); owners are listed in [concern ownership](../../../docs/concern-owners.md).

## Steps

1. **Load rskit.** `git submodule update --init --recursive`.
2. **Find the owner.** Check [concern ownership](../../../docs/concern-owners.md), then the crate under `rskit/core/rskit-*/`. Read its public API, invariants, and error model.
3. **Reuse it** if it fits. Call it directly; do not wrap it in a Toven copy.
4. **Enhance rskit** if the capability is missing or too narrow for any consumer, not just Toven:
   - Design a general-purpose addition in the owning rskit crate, following rskit's own `.github/copilot-instructions.md` and skills.
   - Consume it from Toven through the path dependency.
   - Never make rskit depend on Toven concepts.
5. **Defer only when out of scope.** Record the gap as an rskit issue linked by full URL, and say so in the change. Do not ship a local reimplementation.
6. **Update owners.** If ownership changed, update [concern ownership](../../../docs/concern-owners.md).

## Validate

```bash
cargo nextest run -p <toven-crate> --all-features
```

Run rskit's own validation for any rskit change. For an audit, use the review skill's [rskit reuse lens](../review/references/rskit-reuse.md).

Changing rskit changes the submodule revision. Do that only as part of the requested work, and never commit, push, or create branches without authorization.
