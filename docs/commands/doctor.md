# Doctor

Audit the tools the resolved task graph needs and see which ones are present or missing.

## Quickstart

Run the audit:

```bash
toven doctor
```

`toven doctor` reports which tools are present or missing. Tool identity comes from ecosystem adapter probes and resolved task argv, so the audit follows the configured task table instead of a hand-maintained list.

## Syntax

```text
toven doctor [OPTIONS]
```

```bash
toven doctor
toven doctor --output jsonl
toven doctor --ensure
toven doctor --profile app
```

## What it reports

`doctor` walks the resolved task graph, collects distinct tools, probes each once, and reports presence and version. Shared tools are audited once even when several tasks use them.

```text
  tool mdbook (mdbook): present (mdbook <version>)
  tool ast-grep (ast-grep): present (ast-grep <version>)
  tool cargo (cargo): present (cargo <version>)
doctor: 3 checked, 0 missing
```

Examples include `cargo` for Rust tasks, `ast-grep` for `structure`, and `mdbook` for `docs-build`. Human output uses stderr. With `--output jsonl`, stdout receives one `tool-audited` record per tool, one `check-audited` record per check (`id`, plus `failure` and `fix` when it fails), and a final `doctor-finished` record with `checked`, `missing`, `checks` and `failed`.

## Project checks

A project can add its own health checks under `[doctor.checks.<id>]`. Each check is a simple yes/no test:

- `path` passes when that project-relative file or directory exists.
- `argv` passes when the command exits 0 within 30 seconds. Add `version` to also require that text in its output, as whole numbers: `1.3.6` does not match `1.3.60` or `11.3.6`, while a prefix such as `go1.26` or `v24.` matches `go1.26.1` or `v24.11.0`.
- `fix` is the hint printed when the check fails.
- `profiles` keeps a check out of the everyday run. It runs only when `--profile` names one of its profiles.

```toml
[doctor.checks.packs]
path = "target/plainworks/REVISION"
fix = "make web-deps"

[doctor.checks.node]
argv = ["node", "--version"]
version = "v24."
fix = "mise install"

[doctor.checks.docker]
argv = ["docker", "info"]
fix = "start Docker"
profiles = ["app"]
```

```text
  check docker: `docker info` failed (exit 1) (fix: start Docker)
  check node: ok
  check packs: ok
doctor: 3 checked, 0 missing, 3 checks, 1 failed
```

Checks run in id order after the tool audit. An unknown `--profile` is an input error. Commands run as argv with no shell, and Toven never runs a `fix` for you.

## Exit status and provisioning

`doctor` is report-only by default. It never dials the network or installs tools. The process exits non-zero when a required tool is missing or a check fails, so scripts can gate on it before a task run.

`--ensure` turns gaps into a typed error that names every missing tool or failed check. The global `--auto-install` flag is accepted as an equivalent. `doctor` still never installs per-task tools; it points to [driver](driver.md) for the only auto-provisionable surface.

```bash
toven doctor --ensure
```

| Flag | Effect |
|---|---|
| `--ensure` | Turn any missing tool into a typed, actionable error instead of a report-only non-zero exit |
| `--profile NAME` | Also run checks tagged with that profile; repeat for several |
| `--auto-install` (global) | Accepted as an equivalent to `--ensure`; `doctor` never installs per-task tools |
| `--output human\|jsonl` | Select the human report on stderr or the machine-readable stream on stdout |

## Why it lives in core

Tool auditing is a language- and tool-agnostic mechanism, so it is a Toven-core verb. Tool identity still comes from adapters and task argv, never from a list baked into core. See the [language- and tool-agnostic core](../engineering.md#language--and-tool-agnostic-core) principle.
