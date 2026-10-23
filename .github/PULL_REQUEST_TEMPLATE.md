<!--
Thank you for contributing to Toven.

Keep the description reviewer-focused: explain the intent, behavior, and review focus at a high level. Do not restate the full file list; GitHub already shows that in the Files changed tab.
-->

## Summary

<!--
A few high-level bullet points: what this PR changes and why it is the right next step. Keep it itemized and easy to scan; do not restate the file list.
-->

-
-

## Review context

<!--
A short itemized list to help reviewers and automated review tools understand the shape of the change: the problem being solved, important design choices, and anything intentionally left for a follow-up PR.
-->

-
-

## Type of change

<!-- Mark every relevant option with an "x". -->

- [ ] Documentation or community files
- [ ] Build, tooling, CI, or release infrastructure
- [ ] CLI behavior
- [ ] Planning, discovery, preset, or execution logic
- [ ] Tests or fixtures
- [ ] Breaking change

## Validation

<!-- List the checks actually run. Prefer scoped checks for the changed crate; run `make check` for cross-cutting or release-bound changes. Prose-only changes need doc checks only. -->

- [ ] Scoped checks: 
- [ ] `make check`
- [ ] Doc checks (`make docs-build`, link review)

## Risk and rollout

<!--
Call out breaking changes (and their CHANGELOG entry), migration notes, operational risk, or why there is no user-visible risk.
-->

## Checklist

Matches the [Definition of Done](https://github.com/kbukum/toven/blob/main/docs/engineering.md#definition-of-done).

- [ ] The PR is one reviewable step, and the code sits in the right crate and layer and reuses the rskit owner.
- [ ] The cascade is complete: schema, normalization, planner, executor, output, tests, and docs.
- [ ] Behavior changes have tests written first, with failure paths; fixes have regression tests.
- [ ] Trust boundaries are validated, argv is untouched, I/O is bounded, cancellation is honored, and no secrets are logged.
- [ ] Public API, errors, flags, and output follow the stability rules; breaking changes are in `CHANGELOG.md`.
- [ ] User docs, rustdoc, and examples match the new behavior.
- [ ] New dependencies are justified, pass `make deny`, and come with lockfile updates.
- [ ] Follow-up work is listed in review context instead of bundled here.
