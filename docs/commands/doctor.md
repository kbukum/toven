# Audit required tools

Check the tools the resolved task graph needs:

```bash
toven doctor
```

`toven doctor` reports which tools are present or missing. Tool identity comes from ecosystem adapter probes and resolved task argv, so the audit follows the configured task table instead of a hand-maintained list.

## Syntax

```text
toven doctor [--ensure] [OUTPUT_OPTIONS]
```

```bash
toven doctor
toven doctor --output jsonl
toven doctor --ensure
```

## What it reports

`doctor` walks the resolved task graph, collects distinct tools, probes each once, and reports presence and version. Shared tools are audited once even when several tasks use them.

```text
  tool mdbook (mdbook): present (mdbook v0.5.4)
  tool ast-grep (ast-grep): present (ast-grep 0.44.1)
  tool cargo (cargo): present (cargo 1.97.1)
doctor: 3 checked, 0 missing
```

Examples include `cargo` for Rust tasks, `ast-grep` for `structure`, and `mdbook` for `docs-build`. Human output uses stderr. With `--output jsonl`, stdout receives one tool record per line plus a terminal summary record.

## Exit status and provisioning

`doctor` is report-only by default. It never dials the network or installs tools. The process exits non-zero when a required tool is missing, so scripts can gate on it before a task run.

`--ensure` turns gaps into a typed error that names every missing tool. The global `--auto-install` flag is accepted as an equivalent. `doctor` still never installs per-task tools; it points to [driver](driver.md) for the only auto-provisionable surface.

```bash
toven doctor --ensure
```

| Flag | Effect |
|---|---|
| `--ensure` | Turn any missing tool into a typed, actionable error instead of a report-only non-zero exit |
| `--auto-install` (global) | Accepted as an equivalent to `--ensure`; `doctor` never installs per-task tools |
| `--output human\|jsonl` | Select the human report on stderr or the machine-readable stream on stdout |

## Why it lives in core

Tool auditing is a language- and tool-agnostic mechanism, so it is a Toven-core verb. Tool identity still comes from adapters and task argv, never from a list baked into core. See the [language- and tool-agnostic core](../engineering.md#language--and-tool-agnostic-core) principle.
