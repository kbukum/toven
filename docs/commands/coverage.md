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

Rust supports line, function, and region metrics. Go coverage supplies line metrics. Unsupported dimensions do not fail measurement.

`exclude` and profile `modules` take bare module names, because the block already belongs to one ecosystem. A name that matches no discovered module in that ecosystem fails the run, so a typo cannot silently stop excluding a module. An `ecosystem:name` entry fails with a hint to use the bare name. This check runs before the coverage task, so a typo fails fast without a measurement.

`changed_line` only applies when the run selects changes: `--base <ref>`, or `--merge-base` with the configured `base_ref`. A plain `toven coverage` measures every module and does not check it. Toven compares changes against the baseline ref; if that ref does not exist (for example the default `origin/main` in a repository with no `origin` remote), the run fails and suggests `--base <ref>`.

### Go profiles

A Go coverage task can be a plain `go test -coverprofile=target/toven/coverage/go-{module.name}.out …`. Go writes import paths (`example.com/repo/svc/x.go`) into the profile. Toven maps them back to repository paths using each discovered module's path from `go.mod`, so files in nested modules are attributed to the right module and matched against changed files. No path rewriting is needed in the task. Go profiles are only credited to Go modules and LCOV profiles only to the others, so a Go module and, say, a command module that share a directory never take each other's files. If two discovered modules declare the same Go module path (for example in two federation members), Toven cannot tell their files apart and the report fails with an error instead of guessing.

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
