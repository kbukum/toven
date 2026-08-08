# Testing Toven

Toven's end-to-end coverage is a **data-driven golden suite**. A test is a *scenario*: an ordered session of `toven` invocations run inside a real, git-initialized fixture repo, where each invocation's streams, exit code, and side-effects are compared against reviewable golden files. Adding coverage for a new command or flag means dropping a fixture repo, a `scenario.yaml`, and its golden output files into an organized folder tree — **no Rust changes**.

The engine that discovers, materializes, runs, normalizes, matches, and (on demand) regenerates scenarios lives in `toven-testkit` (`toven_testkit::scenario`). The `apps/toven/tests/golden.rs` harness is a thin `libtest-mimic` main that turns every `scenario.yaml` under `apps/toven/tests/golden/` into one reported test. You never edit it.

## The scenario model

A scenario is a directory containing a `scenario.yaml` and its golden files. The YAML names a fixture repo, an optional scripted git history, a required-toolchain gate, deterministic environment overrides, and an ordered list of **steps**. Each step is one `toven` invocation with its expected exit code, per-stream golden references plus a **matcher**, and declarative **effects**.

A step may add its own `requires:` gate for tools beyond the scenario-level gate (for example `requires: [cargo-cyclonedx]` on a step that shells out to the plugin). A step whose toolchain is absent is skipped green, and later steps still run.

```yaml
# apps/toven/tests/golden/command/single/apply-cache-session/scenario.yaml
repo: command/single          # a fixture under crates/toven-testkit/fixtures/repos/
requires: [cargo]             # optional toolchain gate; absent tool => skip green
env: { TOVEN_LOG: warn }      # optional env overrides on top of the deterministic base
git:                          # optional scripted history, applied after the import commit
  commits:
    - msg: touch app
      touch: [crates/app/src/main.rs]
  tags: [v1]
  branches: [feature]
steps:
  - id: 01-cold                # golden files are named <id>.stdout / <id>.stderr
    argv: [build]              # passed to `toven` verbatim — never rewritten
    config: toven.unordered.toml  # optional --config variant inside the repo
    exit: 0                    # expected exit code (default 0)
    stdout: { match: exact }   # per-stream matcher; omit a stream to leave it unasserted
    stderr: { match: normalized }
    effects:
      - cache_entries: ">0"    # side-effect assertions checked after the step
      - file_exists: out.txt
```

Ordering is first-class: it is how cold → warm caching, idempotency, and "affected since a commit" are exercised. Execution stops at the first failed step.

### Effects

Effects assert side-effects after a step runs: `cache_entries` (a count comparison like `3`, `">0"`, `">=2"`), `file_exists` / `path_absent` (any repo-relative path), `file_matches` (a repo file against a golden in the scenario directory), and `git_tag_exists` / `git_tag_absent` (a release tag that must be present or, for rehearsals and rejected mutations, must not exist).

## The fixture catalog

Fixture repos are real, minimal, buildable trees under `crates/toven-testkit/fixtures/repos/`, named by ecosystem and topology so `repo ↔ scenario ↔ output` is obvious. Each is materialized to a temp dir and `git init`-ed with pinned identity and dates before a scenario runs, and every step runs with the repo root as the working directory.

- `rust/` — `single`, `workspace-linear` (`app → corelib → util`), `workspace-diamond`, `multi-workspace`, `workspace-inherited`, `publish-train` (release config), `onboarding` (no `toven.toml`).
- `go/` — `single`, `work-linear` (`go.work`), `versioned`.
- `command/` — `single` (echo-only, deterministic), `failing`, `multi-task`.
- `polyglot/umbrella` — rust + go + command in one tree.
- `federation/cross-repo` — a `[[members]]` federation.
- `edge/` — `empty`, `no-ecosystem`.

The per-ecosystem task grammar is defined once in `fixtures/repos/_profiles/{rust,go,command}-tasks.toml` and injected into every materialized repo, so a fixture `toven.toml` declares only its project identity and discovery shape and `include`s the shared profile. It never restates the task grammar.

### Config variants

Where one source tree must exercise several situations, a repo carries sibling `toven.<variant>.toml` files (for example `rust/workspace-linear/toven.unordered.toml`, `toven.json-report.toml`, `toven.custom-cache-dir.toml`). A step selects one with `config:`; the default `toven.toml` is the happy path.

## Matcher tiers

Pick the strictest matcher that is sound for the stream:

- **`exact`** — byte-for-byte equality. Use for machine surfaces that are deterministic under the pinned clock: the `--output jsonl` event stream, `tasks --output jsonl`, and `completions` scripts.
- **`normalized`** — byte equality after the default normalizer scrubs volatile tokens: the materialized repo root (`<REPO>`), the scoped cache dir (`<CACHE>`), content hashes (`<SHA>`), and durations, including both `12ms`/`1.9s` spans and the human summary's bare `duration-ms:  N` line (`<DUR>`). Use for human output that carries paths or timings — PLAN/APPLY reporter streams, `explain`, `cache path`, `init`.
- **`line-set`** — a positional leading/trailing frame plus an order-insensitive middle band. Use for output whose *content* is deterministic but whose *line order* is not (parallel waves). It does not scrub, so it suits only volatile-token-free output.
- **`subset`** — every non-blank expected line present, in order. Reserve it for genuinely noisy real-toolchain output where nothing stricter holds.

Because `line-set` does not normalize and every APPLY summary carries a `duration-ms` line, a parallel multi-unit APPLY is authored with `--jobs 1` to pin a deterministic order and matched with `normalized`, rather than with `line-set`.

## Determinism

The engine pins everything a scenario needs to be reproducible and safe under the harness's own parallelism:

- A fixed wall clock (`TOVEN_CLOCK_EPOCH`), so the only wall-clock field — the `run_id` — is stable.
- Pinned git identity and commit dates, `LC_ALL=C`, and `TERM=dumb`.
- A **scenario-scoped cache dir**.
- **Per-scenario toolchain homes** (`CARGO_HOME`/`GOCACHE`/`GOPATH`), so concurrent real-toolchain steps never contend on a shared package-cache lock.

Prefer the toolchain-independent `command` ecosystem for exact APPLY goldens. Gate real `cargo`/`go` scenarios with `requires:` so a runner without that toolchain skips the scenario green, and gate a step needing an extra plugin (such as `cargo-cyclonedx`) with a step-level `requires:` so only that step skips.

## Doctests

Doctests are a **Rust-adapter task, not a core Toven concept**. Nextest — the `test` task — cannot run documentation tests, so the `toven-rust` adapter ships a default `doctest` task that runs `cargo test --doc`. It reuses `TaskKind::Test` (a doctest is a test) under the distinct name `doctest`, so it never collides with the nextest `test` task, and it fans out per module exactly like `test`. The Go adapter has no `doctest` task by design — that asymmetry proves the capability lives in the correct layer.

The canonical gate runs both: `make test` is `test-nextest` plus `doctest`, so a broken doctest fails `make check`. Run doctests alone with `toven run doctest` (or the low-level `cargo test -p <crate> --doc`).

## The check / bless loop

```bash
make golden   # run the whole matrix — one reported case per scenario
make bless    # regenerate goldens from live output (RSKIT_BLESS=1), then re-check
```

`make bless` writes each golden from the actual captured output, then runs `make golden` to prove the regenerated tree is clean. After blessing, review every generated file by eye — the goldens are the contract — and run `make golden` twice to confirm the tree is deterministic. The matrix also runs inside the canonical gate (`make check` picks up the `golden` harness through nextest).

## Add coverage in three files, no code

To cover a new command or flag end-to-end, add only data:

1. Pick (or add) a fixture repo under `crates/toven-testkit/fixtures/repos/` that has the shape you need.
2. Create `apps/toven/tests/golden/<ecosystem>/<shape>/<scenario>/scenario.yaml` describing the invocation, its expected exit, matcher, and effects.
3. Run `make bless` to generate the golden files, review them, then `make golden` to lock them in.

No `src/` or `tests/*.rs` change is required — the harness discovers the new `scenario.yaml` automatically.
