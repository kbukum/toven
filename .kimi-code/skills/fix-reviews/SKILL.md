---
name: fix-reviews
description: >-
    Evaluate a pull request's review comments as signals of an underlying pattern, not one-off
    spot fixes — judge each comment against Toven's engineering baseline, then apply the pattern
    across the whole change set (e.g. one typo comment → sweep every changed file for typos),
    validate, commit the fixes, and resolve the threads. Use when asked to go over, address, or
    act on PR reviews.
user-invocable: true
---

# Fixing PR reviews by pattern

A review comment points at one spot, but it almost always describes a *class* of problem. The value of this skill is **generalization**: treat each comment as a probe into a pattern, fix every instance of that pattern across the change set, and only then resolve the thread. A reviewer who flags one typo, one silent argv rewrite, or one library print is telling you where to look — not the full extent of the fix.

Act on reviews **only when explicitly asked** — never as a side effect of finishing work.

## 1. Gather the reviews

Identify the PR (current branch's PR unless one is named) and pull every review, inline comment, and thread with its resolution state and node IDs:

```bash
gh pr view --json number,title,url,headRefName
gh pr view <n> --json reviews,comments                      # summary bodies
gh api repos/{owner}/{repo}/pulls/<n>/comments --paginate   # inline review comments (path/line/body)
```

For resolvable thread IDs (needed in step 5), read the review threads via GraphQL:

```bash
gh api graphql -f query='
  query($owner:String!,$repo:String!,$pr:Int!){
    repository(owner:$owner,name:$repo){
      pullRequest(number:$pr){
        reviewThreads(first:100){ nodes{
          id isResolved isOutdated
          comments(first:20){ nodes{ path body author{login} } }
        }}
      }
    }
  }' -F owner=<owner> -F repo=<repo> -F pr=<n>
```

## 2. Evaluate each comment — the baseline wins

For every comment, decide before touching code:

- **Valid?** Judge it against Toven's baseline ([`docs/engineering.md`](../../../docs/engineering.md)), not the reviewer's authority. If a suggestion conflicts with the baseline or is simply wrong, **do not apply it** — note why (you may leave the thread unresolved for the maintainer, but never argue under their name).
- **What is the real pattern?** Look past the wording to the class of issue:
  - a typo/grammar note → *spelling & wording across all changed prose and `///` docs*
  - a library that prints in one spot → *every library crate touched (only the CLI layer prints)*
  - a silently-rewritten argv → *every place user argv is expanded*
  - a stranded test double / inline TOML → *every test in the change set (use `toven-testkit`)*
  - a duplicated-concern note → *every place that reimplements an rskit owner*
- **Scope of the sweep.** Default to the PR's change set (`git diff origin/main...HEAD`). Widen to neighbouring files only when the pattern clearly extends there and the fix stays coherent; note the widening. Do not silently refactor unrelated code.

## 3. Apply the pattern across the change set

Fix **all** instances of each validated pattern, not just the flagged line:

```bash
git diff origin/main...HEAD --name-only     # the files in scope
```

- Search the whole change set for the pattern (grep/glob) and fix every occurrence.
- Make the same class of fix consistently; prefer a root-cause change over repeating a patch.
- Keep each pattern's fixes cohesive so the follow-up commit reads as one intent.

## 4. Validate — scoped to what changed

Run the smallest gates that cover the touched crates (see the `validate` skill):

```bash
cargo test -p <crate> --all-features -q
cargo clippy -p <crate> --all-targets --all-features -- -D warnings
make structure
```

Docs/prose-only sweeps need no build/test gates. Never resolve a thread whose fix hasn't been validated.

## 5. Commit, push, and resolve

Commit the pattern fixes using the [`commit`](../commit/SKILL.md) skill — a single compact, developer-friendly Conventional-Commit message that states the change as it stands, **no `Co-authored-by` trailer** and no review/plan/batch narration. Group commits by pattern when it aids the reader, or a single tidy commit when the sweep is small.

```bash
git push
```

Then resolve the threads you genuinely addressed — no reply comments posted under the maintainer's name:

```bash
gh api graphql -f query='
  mutation($id:ID!){ resolveReviewThread(input:{threadId:$id}){ thread{ isResolved } } }' \
  -F id=<threadId>
```

Leave unresolved only the threads you deliberately rejected (baseline conflict) or that need a maintainer decision; briefly report those back rather than resolving them silently.

## Baseline

Every fix must still satisfy Toven's engineering baseline ([`docs/engineering.md`](../../../docs/engineering.md)) — a review-driven change is held to the same bar as any other. If acting on a comment would push code below the baseline, reject the comment instead.
