# Concern owners

The canonical **concern → owning crate** map for toven. Before adding any shared helper, type, or capability, find the concern below and **reuse or extend the named owner** — do not fork a local copy.

toven reuses its foundations from **vendored rskit** and owns only its domain crates. The two tables below make that boundary explicit. The reuse-from-rskit judging procedure lives in the [`rskit-reuse` skill](../.github/skills/rskit-reuse/SKILL.md) and the review pass [`.github/skills/review/references/01-rskit-reuse.md`](../.github/skills/review/references/01-rskit-reuse.md); start here, then reconcile each low-level operation against them.

## Reused from rskit (foundations)

Consume via the path dep; if a foundation is inadequate, **enhance rskit generically**, then consume — never fork a local copy in toven.

| Concern | Owner (rskit) | Reuse this, not |
|---|---|---|
| Data formats (JSON/TOML/…) | `rskit-codec` | hand-rolled `serde_json` / `toml` wrappers |
| Generic helpers | `rskit-util` + std | a fresh local helper |
| Filesystem / path safety / atomic writes | `rskit-fs` | raw `std::fs` + manual escape checks |
| Config loading / precedence | `rskit-config` | bespoke precedence logic |
| Errors | `rskit-errors` | ad-hoc error enums, `Box<dyn Error>` in public APIs |
| Git repository access / diffing | `rskit-git` | shelling out to the `git` CLI, hand-rolled libgit2 bindings |
| Semantic version parsing / bumping | `rskit-version` (`semver::Version`) | a local semver parser |
| Logging / tracing | `rskit-logging` | `println!`, direct subscriber wiring |
| Subprocess | `rskit-process` | bare `std::process::Command` |
| Schema / validation | `rskit-schema` / `rskit-validation` | hand-rolled validation walks |

## Owned by toven (domain)

| Concern | Owner | Notes |
|---|---|---|
| Domain model / normalization | `toven-model` | schema, selectors, planner types |
| Injected contracts (ports) | `toven-ports` | every injected trait lives here; adapter stays in the consumer, double in `toven-testkit` |
| Module discovery / dependency ordering / batching | `toven-engine` | planner + executor |
| Command definitions / expansion | `toven-command` | argv-only; never infers hidden flags |
| Go toolchain adapter | `toven-go` | |
| Rust toolchain adapter | `toven-rust` | |
| CLI / reporting | `toven-cli` | only layer that prints; stdout reserved for machine stream |
| Test doubles | `toven-testkit` | |

## How to use this map

1. Name the concern before writing the code.
2. If it is a foundation, consume the `rskit-*` owner (enhance rskit generically if inadequate). If it is a toven domain concern, use the `toven-*` owner above.
3. Put every injected contract in `toven-ports`; keep its concrete adapter in the consuming crate and its double in `toven-testkit`.
4. Never fork a foundation into toven; never re-own an rskit concern locally.
