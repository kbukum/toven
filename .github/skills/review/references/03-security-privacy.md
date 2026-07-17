# Pass 03 — Security & privacy

A dedicated pass because a vibe-coded path that "just works" usually skips boundary validation, and Toven's whole job is to turn untrusted repository files and user-owned argv into executed subprocesses — a gap here runs attacker-influenced input. The principle-level summary lives in pass `02`; this pass is the standing baseline. For a deeper sweep on security-sensitive changes, pair it with a dedicated security review.

> **Run in a separate, clean-context agent** — never inline in the session that wrote the code. An independent reviewer re-derives every judgment from the code and the principles instead of trusting prior reasoning. A plan/spec may be passed in as a scope checklist only; it never excuses a baseline violation.

**Scope note.** *Changes mode:* trace each new input path from its trust boundary — user argv, `toven.toml`/config, discovered module manifests, federation wire frames — to where it flows into a path, a selector, a subprocess, or a deserialization. *Project mode:* audit Toven's untrusted surfaces: the CLI argv/flag boundary (`toven-cli`), config and repo-file loading and the ecosystem adapters that shell out to toolchains (`toven-engine`, `toven-rust`, `toven-go`, `toven-command`), and the federation RPC protocol (`toven-engine/src/federation/`). See the Security section of [`docs/engineering.md`](../../../../docs/engineering.md).

## Checks

- **Validate at every trust boundary.** User argv and repository files are untrusted. A selector, module name, path, or config value that flows into a filesystem read, a generated command, or a deserialization without validation is a blocker. Least-privilege and secure-by-default.
- **argv-only subprocess execution.** Generated commands are argument vectors; no shell interpolation of untrusted input. Shell execution is an explicit, opted-into mode — never a silent default. Route process spawning through `rskit-process`; never hand-build `sh -c "…"` from repo or argv data.
- **Bounded input and output.** Every read of an untrusted repo file, subprocess stream, or federation frame has an explicit size/time bound. Unbounded reads of untrusted input, or a subprocess/RPC call with no timeout and no cancellation-token honoring, are findings.
- **Token & secret hygiene.** No secrets in logs, error messages, generated commands, or the JSONL event stream; redact sensitive fields. Credentials/tokens ride in headers or env, never in argv or query strings that land in a plan or log.
- **Path & traversal safety.** Paths derived from config or discovery are canonicalized (via `rskit_fs::canonicalize`) and confined to the workspace root; reject `..`-escapes and absolute paths that leave the repo before they reach the filesystem or a subprocess `cwd`.
- **Current crypto, not hand-rolled.** Any signing/verification/hashing on the federation or supply-chain path uses current primitives from the canonical rskit owner — no MD5/SHA-1-for-security, ECB, static IVs, or hard-coded keys.

## Detection starters

Read each hit to judge intent — these flag candidates, not verdicts. Exclude `#[cfg(test)]` and `tests/`.

```bash
# shelled commands / string-built argv from untrusted input
rg 'sh -c|"bash"|"sh"|format!\(.*\)\s*(\+|\.push_str)' crates/*/src
# secrets in logs / errors / generated output
rg '(info|debug|warn|trace|error)!\(.*\b(token|secret|password|api_?key)\b' crates/*/src
rg '(token|secret|password|api_?key)\s*=\s*"' crates/*/src        # hard-coded credentials
# unbounded / un-cancelled subprocess & RPC calls
rg 'spawn|read_to_end|read_to_string' crates/toven-engine/src     # bound + timeout + cancellation?
# path handling that should be canonicalized/confined
rg 'PathBuf::from|Path::new|\.\.|join\(' crates/toven-engine/src crates/toven-{rust,go,command}/src
```

Flag any path/selector from config, discovery, or a federation frame that flows into filesystem or process execution without validation, and any read of untrusted input with no explicit size limit.
