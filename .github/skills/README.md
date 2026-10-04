# toven skills

Apply the [repository baseline](../copilot-instructions.md). Each skill is the canonical procedure for this repository.

Skill names/descriptions are discovery context; bodies load on activation; references load only when their task applies. Do not preload the catalog or recursively follow links. Review directly by default; independent agents require the user's request. Routing to a skill is not delegation to another agent.

| Skill | Use when |
|---|---|
| [apply-plan](apply-plan/SKILL.md) | Resume an existing plan in dependency order, validating each bounded work order. |
| [apply-step](apply-step/SKILL.md) | Implement one plan step test-first, validate its acceptance criteria, and record progress. |
| [commit](commit/SKILL.md) | Commit the authorized staged change with a concise Conventional Commit message; only when asked. |
| [create-branch](create-branch/SKILL.md) | Create a change-named kbukum/ branch from an up-to-date main when starting new work. |
| [create-plan](create-plan/SKILL.md) | Write or revise a high-level implementation plan with dependencies and acceptance criteria. |
| [create-pr](create-pr/SKILL.md) | Open a draft pull request using the repository template; only when explicitly asked. |
| [docs](docs/SKILL.md) | Update or audit documentation for accuracy, clear prose, working examples, and links. |
| [fix-issue](fix-issue/SKILL.md) | Investigate a GitHub issue, fix its root cause, and validate affected behavior. |
| [fix-reviews](fix-reviews/SKILL.md) | Evaluate PR review comments and fix valid patterns across the affected change; only when asked. |
| [new-crate](new-crate/SKILL.md) | Add a Rust crate in the correct layer with workspace wiring, lints, docs, and shared tests. |
| [release](release/SKILL.md) | Prepare or publish a release through the repository's version, validation, and supply-chain gates. |
| [review](review/SKILL.md) | Review a diff, package, or tree for architecture, correctness, security, tests, and documentation. |
| [rskit-reuse](rskit-reuse/SKILL.md) | Reuse or enhance rskit before adding shared infrastructure to Toven. |
| [validate](validate/SKILL.md) | Run the repository's build, test, lint, and documentation gates scoped to the change. |

Preserve worktree/index state. Planning or review does not authorize source changes; Git mutations/publication require the corresponding authorization. Alternate command/agent entry points route here; edit procedures in the canonical skill, not duplicated copies. Keep descriptions short, requirements explicit, and links valid.
