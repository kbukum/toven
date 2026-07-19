# Coverage

`toven coverage` runs the coverage task, aggregates the profiles it emits per module, and gates them against the resolved `[…coverage]` thresholds. Coverage is a recognized task kind: the ecosystem tool measures (cargo llvm-cov for Rust, `go test -coverprofile` for Go) and stages its profiles under `target/toven/coverage`; Toven then attributes each profile to a module, folds its metrics, and decides the pass/fail verdict. The measurement is the ecosystem tool's job; the aggregation and the threshold verdict are Toven's.

```bash
toven coverage                       # measure, aggregate, and gate every module
toven coverage --module rust:core    # narrow to one module (and, with --dependents, its dependents)
toven coverage --base main           # gate changed lines against the main baseline
toven coverage --output jsonl        # machine-readable per-module records
```

The verb runs the `coverage` task through the human run reporter (progress and the run summary on stderr), then renders the per-module verdict table on stdout. It exits non-zero when the measurement task fails or when any `block`-enforced module is below threshold.

## Configuration model

Coverage is config-driven, mirroring release: a typed, validated `[…coverage]` block, not a bespoke side-file. Toven config owns only the verdict inputs — the per-dimension floors, the enforcement mode, and which modules to exclude or elevate. The measurement flags (`--html`, profraw cleanup, the tool's own `--jobs`, the profile output path) stay in the coverage task's argv you author. Under Toven's baseline the ecosystem tool measures and Toven decides only ordering, aggregation, and the verdict, so runner flags belong to argv.

```toml
[ecosystems.rust.coverage]
line = 90.0                 # absolute per-module floor (codecov `project`)
function = 85.0             # optional, Rust-only
region = 80.0               # optional, Rust-only
changed_line = 85.0         # changed-lines floor under a baseline (codecov `patch`)
enforcement = "block"       # block (fail-closed) | advisory (measure + report)
exclude = ["toven-suite"]   # measured but never gated

# Optional sugar: one elevated threshold set applied to many modules.
# Resolves below a per-module override, above the ecosystem default.
[ecosystems.rust.coverage.profiles.security]
line = 95.0
modules = ["toven-auth", "toven-authz", "toven-encryption"]

# Per-module override.
[modules."rust:toven-process".coverage]
line = 85.0
enforcement = "advisory"

[ecosystems.go.coverage]
line = 80.0                 # Go emits line coverage only
changed_line = 85.0
```

Every field is optional with a validated default, and the block is `deny_unknown_fields`, so an existing `toven.toml` with no `[…coverage]` block keeps parsing and inherits the adapter default. A per-module `[modules.<ref>.coverage]` override may set only the threshold floors and `enforcement`; `exclude` and `profiles` are ecosystem-level decisions and are rejected inside a per-module block rather than silently ignored.

### Threshold dimensions

The dimensions map to what each ecosystem can actually measure. Rust llvm-cov emits line, function, and region coverage; Go `-coverprofile` emits line (statement) coverage only. `function` and `region` are optional, and the gate skips any dimension the ecosystem does not emit rather than failing on a missing metric — so `function`/`region` floors gate Rust modules but are ignored for Go.

`line` (and `function`/`region`) is the absolute per-module floor. `changed_line` is the floor applied to changed lines under a baseline selection (`--base`/`--merge-base`), wiring codecov's `patch` floor onto Toven's affected engine.

### Enforcement

`enforcement = "block"` (the default) fails the gate closed when a dimension is below its floor; `enforcement = "advisory"` measures and reports the shortfall without failing. `exclude` lists modules that are measured but never gated.

### Precedence

Each module's settings resolve from highest to lowest precedence, identical to release:

```text
--line/--function/…/--enforcement argv  >  [modules.<ecosystem:module>.coverage]  >  matching profiles.<name>  >  [ecosystems.<id>.coverage]  >  adapter default
```

## Per-run threshold overrides

The verb accepts threshold-override flags that layer over the resolved config for one run — argv wins, config is the durable default. Each is rejected on every other verb with a typed usage error.

- `--line <PCT>` / `--function <PCT>` / `--region <PCT>` / `--changed-line <PCT>` — override a dimension's floor (a percentage in `0..=100`).
- `--enforcement <block|advisory>` — override how a below-threshold verdict is enforced.

```bash
toven coverage --line 95                 # raise the line floor for this run only
toven coverage --enforcement advisory    # report shortfalls without failing the run
```

## Selection

The selection flags narrow the measured scope exactly as the task verbs do: `--module`/`--workspace` pick modules, `--dependents`/`--dependencies` widen the closure, and `--base`/`--merge-base` select a changed baseline against which `changed_line` is gated. See [selecting a baseline](README.md#selecting-a-baseline).

## Output

The per-module verdict table renders on stdout (`Module`, `Status`, `Line`, `Function`, `Region`, `Changed`, `Enforcement`); an unmeasured dimension shows `-`. The run summary and the gate verdict line ride stderr alongside the measurement's progress. `--output jsonl` emits one record per module on stdout — each carrying the module's status, enforcement, measured metrics, and the per-dimension `measured`/`threshold`/`passed` outcomes — while the human progress stays on stderr.
