# Lens: security

Toven turns untrusted repository files and user argv into running processes, so a gap here runs attacker-influenced input. Rules: [security and runtime correctness](../../../../docs/engineering.md#security-and-runtime-correctness). Threat model: [SECURITY](../../../../SECURITY.md#threat-model).

## Check

- **Trust boundaries.** Trace each new input (argv, `toven.toml`, discovered manifests, federation frames in `toven-core/src/federation/`) to where it reaches a path, selector, subprocess, or deserializer. Unvalidated flow is a blocker.
- **Argv only.** Commands are argument vectors. No `sh -c` built from repo or argv data; shell execution is explicit opt-in.
- **Paths.** Config and discovery paths are canonicalized and kept inside the workspace root.
- **Bounds and cancellation.** Every untrusted read, subprocess stream, and RPC frame has a size cap; calls honor cancellation.
- **Secrets.** None in logs, errors, argv, URLs, JSONL, or plans. Secrets reach children through the environment.
- **Crypto.** Current primitives from the canonical owner; no hand-rolled signing, MD5/SHA-1 for security, or hard-coded keys.

## Detection starters

Exclude `#[cfg(test)]` and `tests/` when judging hits.

```bash
rg -n 'sh -c|"bash"|"sh"' crates/*/src
rg -n '(info|debug|warn|trace|error)!\(.*\b(token|secret|password|api_?key)\b' crates/*/src
rg -n 'read_to_end|read_to_string' crates/*/src
rg -n 'PathBuf::from|Path::new' crates/toven-{core,engine,rust,go,js,command}/src
```
