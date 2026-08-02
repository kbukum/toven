# Audit required tools

`toven doctor` audits the tools the resolved task graph needs and reports which are present or missing. It is Toven's single source of truth for *what* a repository must have installed: tool identity comes from the ecosystem adapter probes and the resolved task argv, so the audit tracks the configured task table rather than a hand-maintained list.

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

`doctor` walks the resolved task graph, collects the distinct tools its tasks invoke (for example `cargo` for the rust tasks, `ast-grep` for `structure`, `mdbook` for `docs-build`), probes each once, and reports its presence and version:

```text
  tool mdbook (mdbook): present (mdbook v0.5.4)
  tool ast-grep (ast-grep): present (ast-grep 0.44.1)
  tool cargo (cargo): present (cargo 1.95.0)
doctor: 3 checked, 0 missing
```

A shared tool is audited once even when several tasks use it. Human output uses stderr; `--output jsonl` emits one tool record per stdout line followed by a terminal summary record, so a script can branch on the machine surface.

## Exit status and provisioning

`doctor` is **report-only by default** and never dials the network or installs anything. The process exit is non-zero when any required tool is missing, so it gates a script the way a failing task does even without opting into provisioning.

`--ensure` turns a gap into a typed, actionable error naming every missing tool (the global `--auto-install` flag is accepted as an equivalent). `doctor` still never installs a per-task tool — it has no installer for them and says so, pointing at the [driver](driver.md) verbs for the only auto-provisionable surface. Use `--ensure` in CI to fail fast with the aggregated report before the gate runs:

```bash
toven doctor --ensure
```

| Flag | Effect |
|---|---|
| `--ensure` | Turn any missing tool into a typed, actionable error instead of a report-only non-zero exit |
| `--auto-install` (global) | Accepted as an equivalent to `--ensure`; `doctor` never installs per-task tools |
| `--output human\|jsonl` | Select the human report (stderr) or the machine-readable stream (stdout) |

## Why it lives in core

Auditing whether the resolved graph has its tools is a language- and tool-agnostic *mechanism*, so it is a Toven-core verb — but tool *identity* still comes from the adapters and the task argv, never a list baked into core. See the [language- and tool-agnostic core](../engineering.md#language--and-tool-agnostic-core) principle.
