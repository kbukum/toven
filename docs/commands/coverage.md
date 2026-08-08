# Measure coverage

Run the coverage gate for Rust modules:

```bash
toven coverage --workspace rust
```

`toven coverage` runs the configured coverage task, attributes emitted profiles to modules, aggregates metrics, and applies configured thresholds.

## Syntax

```text
toven coverage [SELECTION_OPTIONS] [THRESHOLD_OPTIONS] [OUTPUT_OPTIONS]
```

```bash
toven coverage --workspace rust
toven coverage --module rust:toven-cli
toven coverage --base origin/main --merge-base
toven coverage --line 90 --enforcement advisory
toven coverage --output jsonl
```

## Output and exit status

The per-module verdict table uses stdout:

```text
Module      Status  Line   Function  Region  Changed  Enforcement
rust:core   passed  92.4%  88.0%     86.2%   -        block
```

Measurement progress, child output, and the summary use stderr. The command returns non-zero when measurement fails or a module with `block` enforcement misses a threshold. `advisory` reports a shortfall without failing.

JSONL mode emits one module record per stdout line:

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
exclude = ["rust:generated"]

[ecosystems.rust.coverage.profiles.security]
modules = ["toven-security"]
line = 95.0

[modules."rust:core".coverage]
line = 95.0
enforcement = "advisory"
```

Rust supports line, function, and region metrics. Go coverage supplies line metrics. Unsupported dimensions do not fail measurement.

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
