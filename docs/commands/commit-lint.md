# Lint a commit message

Check a commit subject (or PR title) against the Conventional Commits grammar:

```bash
toven commit-lint "feat(cli): add commit-lint verb"
```

`toven commit-lint` validates a subject line against the `type(scope)!: description` grammar Toven's release changelog relies on. It is the strict counterpart of the classification Toven already runs when it generates a changelog, so a subject that lints clean here is exactly one the changelog can group without falling through to `Other`.

## Syntax

```text
toven commit-lint [MESSAGE] [OUTPUT_OPTIONS]
```

```bash
toven commit-lint "fix: correct the config-not-found error"
git log -1 --pretty=%B | toven commit-lint
toven commit-lint --output jsonl "feat(release)!: host the binary on GitHub Releases"
```

The subject comes from the `MESSAGE` argument, or — when it is omitted — the first line of the message read from stdin. That matches both the `commit-msg` hook shape (`toven commit-lint < "$1"`) and a PR-title check (`toven commit-lint "$PR_TITLE"`). Only the first line is linted; a commit body is not part of the header.

## What it checks

A subject is valid when it has an accepted lowercase type, an optional `(scope)`, an optional `!` breaking marker, a `: ` separator, and a non-empty description. The accepted types are:

```text
feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert
```

Each rejection is a specific, actionable reason:

| Reason | Example subject |
|---|---|
| Empty subject | (blank) |
| Missing type — no `type:` structure | `add a thing` |
| Unknown type — including a capitalized type | `wip: half done`, `Feat: add` |
| Missing space after the colon | `feat:add a thing` |
| Empty description | `fix:` |

## Output and exit status

The verdict renders on stdout — a human line by default, or one stable JSON object under `--output jsonl`:

```bash
$ toven commit-lint "feat(cli): add commit-lint verb"
valid Conventional Commit [feat]: feat(cli): add commit-lint verb

$ toven --output jsonl commit-lint "wip: nope"
{"valid":false,"subject":"wip: nope","breaking":false,"violation":"unknown Conventional Commit type `wip`: expected one of feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert"}
```

The process exits `0` for a conforming subject and `1` when it does not conform, so `commit-lint` drops into a git hook or CI gate unchanged. A non-conforming subject is a lint verdict, not a usage error — the same report-then-fail contract [`doctor`](doctor.md) uses.

| Flag | Effect |
|---|---|
| `--output human\|jsonl` | Select the human verdict line or the machine-readable JSON record on stdout |

## Why it lives in the CLI

The Conventional Commit grammar is owned by `toven-version`, which already parses it to classify commits for the release changelog. `commit-lint` exposes that one pure check as a verb rather than duplicating the grammar in a shell regex, so the linter and the changelog can never drift.
