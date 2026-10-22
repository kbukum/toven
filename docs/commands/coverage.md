# Coverage

Run the configured coverage task, aggregate the emitted profiles per module, and gate the result against the resolved thresholds.

## Quickstart

Run coverage across the Rust workspace:

```bash
toven coverage --workspace rust
```

`toven coverage` runs the configured coverage task, attributes emitted profiles to modules, aggregates metrics, and applies configured thresholds.

## Syntax

```text
toven coverage [OPTIONS]
```

```bash
toven coverage --workspace rust
toven coverage --module rust:toven-cli
toven coverage --base origin/main --merge-base
toven coverage --line 90 --enforcement advisory
toven coverage --output jsonl
```

## Output and exit status

In human mode, `coverage` writes measurement progress, per-module verdicts, and the final tally to stderr. With `--output jsonl`, stdout receives one JSON object per module.

Human output lists one verdict line per module, followed by a tally that names the non-zero groups and the gate verdict:

```text
Coverage
  coverage rust:core: passed (line 92.4%)
  coverage rust:cli: failed (line 40.0% (<90.0%))
coverage: 1 passed, 1 failed — gate failed
```

The command returns non-zero when measurement fails or a module with `block` enforcement misses a threshold; `advisory` reports a shortfall without failing. The exit status reflects the tally, not any single module. `--output jsonl` emits one module record per stdout line, order-stable:

```json
{"module":"rust:core","status":"passed","enforcement":"block","line":{"measured":92.4,"threshold":90.0,"passed":true}}
```

## Configure thresholds

```toml
[ecosystems.rust.coverage]
line = 90.0
function = 85.0
region = 80.0
changed_line = 85.0
enforcement = "block"
exclude = ["generated"]

[ecosystems.rust.coverage.profiles.security]
modules = ["toven-security"]
line = 95.0

[modules."rust:core".coverage]
line = 95.0
enforcement = "advisory"
```

Rust supports line, function, and region metrics. Go coverage supplies line metrics. JavaScript LCOV supplies line and function metrics, and its branch records (`BRDA`) report as `region`. Unsupported dimensions do not fail measurement.

`exclude` and profile `modules` take bare module names, because the block already belongs to one ecosystem. A name that matches no discovered module in that ecosystem fails the run, so a typo cannot silently stop excluding a module. An `ecosystem:name` entry fails with a hint to use the bare name. This check runs before the coverage task, so a typo fails fast without a measurement.

`changed_line` only applies when the run selects changes: `--base <ref>`, or `--merge-base` with the configured `base_ref`. A plain `toven coverage` measures every module and does not check it. Toven compares changes against the baseline ref; if that ref does not exist (for example the default `origin/main` in a repository with no `origin` remote), the run fails and suggests `--base <ref>`.

### Go profiles

A Go coverage task can be a plain `go test -coverprofile=target/toven/coverage/go-{module.name}.out …`. Go writes import paths (`example.com/repo/svc/x.go`) into the profile. Toven maps them back to repository paths using each discovered module's path from `go.mod`, so files in nested modules are attributed to the right module and matched against changed files. No path rewriting is needed in the task. Go profiles are only credited to Go modules and LCOV profiles only to the others, so a Go module and, say, a command module that share a directory never take each other's files. If two discovered modules declare the same Go module path (for example in two federation members), Toven cannot tell their files apart and the report fails with an error instead of guessing.

### Profile names

The coverage task writes its profiles under `target/toven/coverage/`. A profile's name says which module produced it: either a file `<ecosystem>-<module>.<ext>` (`go-svc.out`, `rust-core.lcov`) or a directory `<ecosystem>-<module>/` whose top-level files are the profiles (`js-packages-app/lcov.info`). Deeper directories are skipped, so runner HTML reports and scratch files do not count. Absolute paths in a profile are made relative to the project root, and a relative path is resolved against the producing module's directory. A profile with an unknown or missing producer keeps its relative paths as repository-relative. Paths that leave the repository are dropped.

### JavaScript profiles

JavaScript test runners write LCOV paths relative to the package they ran in. `toven init` renders a `coverage` task when every package's `test` script runs the same runner, and points it at `target/toven/coverage/js-{module.name}`. Each package's paths then resolve against that package, so a file one package measures in another (`../core/src/index.js`) is credited to the package that owns it, never to the workspace root.

| Runner | Flags the rendered task adds | Notes |
|---|---|---|
| Vitest | `--coverage.enabled --coverage.reporter=lcovonly --coverage.reportsDirectory=<dir>` | Needs a coverage provider installed, `@vitest/coverage-v8` or `@vitest/coverage-istanbul`. |
| Jest | `--coverage --coverageReporters=lcovonly --coverageDirectory=<dir>` | |
| `bun test` | `--coverage --coverage-reporter=lcov --coverage-dir=<dir>` | Writes no branch records, so there is no `region` metric. |
| c8 | Not rendered; write the task by hand, for example `c8 --reporter=lcovonly --reports-dir=<dir> node --test` | Wrappers such as c8 are not detected from the `test` script yet. |

Toven recognizes a runner only when the `test` script is a single command (`vitest run`, `jest --ci`, `bun test`); scripts that chain commands get no rendered task. For now one rendered task serves every package, so packages with different runners get none; write the task by hand in that case.

### Files measured more than once

Several profiles can measure the same file, for example per-module Go runs with a shared `-coverpkg`. Toven counts the file once. A line counts as covered when any run covered it. LCOV functions and branches merge the same way when the tracefile has per-item `FN`/`FNDA` and `BRDA` records. When it only has the `FNF`/`FNH`/`BRF`/`BRH` summaries, items cannot be matched, so Toven keeps the larger count instead of adding them.

Threshold precedence:

```text
CLI override > module override > named profile > ecosystem setting > adapter default
```

## Threshold options

| Option | Effect |
|---|---|
| `--line <PCT>` | Override line threshold |
| `--function <PCT>` | Override function threshold |
| `--region <PCT>` | Override region threshold |
| `--changed-line <PCT>` | Override changed-line threshold |
| `--enforcement block\|advisory` | Override failure policy |

Percentages must be in `0..=100`. Selection options match [task selection](run.md#select-scope).

The coverage task fans out like any run, so it uses the configured [compute budget](../config/README.md#compute-budget); `--compute-budget <auto|inherit|N>` overrides it for one run.
