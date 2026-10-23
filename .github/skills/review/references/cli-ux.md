# Lens: CLI experience

The CLI is the product. A user should never need to read the source to know what a flag does or what went wrong. Rules: [CLI and consumer experience](../../../../docs/engineering.md#cli-and-consumer-experience). Contracts: [command reference](../../../../docs/commands/README.md).

## Check

- **Errors.** Say what failed, why, and the next step. One clean sentence; did-you-mean when a close match exists.
- **Flags.** Scoped at the parser: a verb's `--help` shows only flags that change it. No silent no-op flags; value flags reject a missing value and `--`; trailing `--` argv passes through untouched.
- **Streams.** stdout carries tables, projections, and JSONL (one document per line); stderr carries progress, child output, and diagnostics. `NO_COLOR` is respected.
- **Vocabulary.** No PLAN, APPLY, event sink, or "cut" in `--help` or messages.
- **Exit codes and dry runs.** Exit codes match the reference and are pinned by tests. Dry runs are labeled.
- **First run.** `toven init` reports what it detected and never writes config later commands reject.
- **Readable output.** Counts are pluralized; run ids stay in `-v` or JSONL.

## Detection starters

```bash
rg -n 'PLAN|APPLY|event.sink|\bcut\b' crates/toven-cli/src
rg -n 'println!|print!|io::stdout' crates/toven-cli/src
cargo run -q -p toven -- --help
cargo run -q -p toven -- run --help
cargo nextest run -p toven-cli --all-features
```

Drive the built binary through the error paths you changed: no config, unknown task, unknown module.
