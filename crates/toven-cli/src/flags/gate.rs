//! The per-verb flag applicability gate.

use rskit_errors::{AppError, AppResult};

use super::cli::{Cli, Command, DriverAction, FederationAction, ReleaseAction};

/// Reject any verb-specific flag used with a verb it does not apply to.
///
/// # Errors
/// Returns a typed usage error naming the misused flag and the verb it belongs
/// to.
pub fn gate(cli: &Cli) -> AppResult<()> {
    let verb_owned = verb_name(&cli.command);
    let verb = verb_owned.as_str();
    let mutating_release = release_action(&cli.command).is_some_and(ReleaseAction::is_mutating);
    let is_bump = matches!(release_action(&cli.command), Some(ReleaseAction::Bump));
    let is_init = matches!(cli.command, Command::Init);
    let is_graph = matches!(cli.command, Command::Graph);
    let is_coverage = matches!(cli.command, Command::Coverage);

    // `--no-push` is a tag/publish rehearsal flag: `bump` never pushes (it only
    // mutates + stages) and the read-only projections never touch history, so it
    // belongs only to the pushing mutating actions.
    if cli.no_push && (!mutating_release || is_bump) {
        return Err(only_applies("--no-push", "toven release tag/publish", verb));
    }
    gate_out_dir_flag(cli, verb)?;
    gate_package_flags(cli, verb)?;
    gate_verify_flags(cli, verb)?;
    gate_bump_flags(cli, verb, mutating_release)?;
    gate_init_flags(cli, verb, is_init)?;
    gate_coverage_flags(cli, verb, is_coverage)?;
    if cli.format.is_some() && !is_graph {
        return Err(only_applies("--format", "toven graph", verb));
    }
    let accepts_auto_install = matches!(
        cli.command,
        Command::Driver {
            action: DriverAction::List
        } | Command::Federation {
            action: FederationAction::Sync
        } | Command::Doctor { .. }
    );
    if cli.auto_install && !accepts_auto_install {
        return Err(only_applies(
            "--auto-install",
            "toven driver list / toven federation sync / toven doctor",
            verb,
        ));
    }
    // `--dry-run` is a PLAN cut for task verbs and a no-mutation rehearsal for
    // `release publish` (and a no-op on the already-dry `release plan`);
    // `--explain` adds task-planning reasoning detail. Neither has meaning on the
    // introspection/maintenance verbs, and `--dry-run` is a no-op on the mutating
    // `release tag` and read-only `release status`, so reject it there.
    if cli.execution.dry_run && !accepts_dry_run(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!("`--dry-run` does not apply to `toven {verb}`"),
        ));
    }
    if cli.execution.explain && !accepts_explain(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!("`--explain` does not apply to `toven {verb}`"),
        ));
    }
    // `--output` selects the event-sink/projection format; the execution verbs, the
    // release lifecycle actions, the `tasks`/`modules` listings and the `affected`
    // projection render a chooseable format, but the other verbs print their own
    // fixed rendering.
    if cli.execution.output.is_some() && !accepts_output_format(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!("`--output` does not apply to `toven {verb}`"),
        ));
    }
    // `-v`/`-q` only shape the human run reporter, which the execution verbs and
    // the mutating release actions build; the introspection/maintenance verbs and
    // the read-only release projections render their own projection and would
    // silently ignore the flag, so reject it rather than advertise a no-op.
    if (cli.execution.verbose > 0 || cli.execution.quiet > 0)
        && !accepts_reporter_shaping(&cli.command)
    {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "reporter verbosity (-v/--verbose, -q/--quiet) does not apply to `toven {verb}`"
            ),
        ));
    }
    // `--color` is accepted on every verb (like `git`/`cargo`): the verbs that build
    // a human reporter or colorized table honor it, and the rest treat it as a
    // harmless no-op. Accepting it globally keeps a `alias toven='toven --color never'`
    // / CI wrapper working regardless of the verb, rather than failing on some.
    // `--view` shapes only the live APPLY output rendering, so it is meaningful
    // only on the task-APPLY verbs that stream child output. On `plan` (PLAN-only),
    // `release`, and every introspection/maintenance verb it would be a silent
    // no-op, so reject it rather than advertise one.
    gate_view_flag(cli, verb)?;
    // `--fail-fast` shapes APPLY scheduling, so it is meaningful only on the
    // task-APPLY verbs. `plan` stops at PLAN and `release` never multiplexes
    // independent units, so the flag is a no-op there and is rejected.
    reject_apply_only_flag(cli.execution.fail_fast, "--fail-fast", &cli.command, verb)?;
    // `--no-cache` shapes the PLAN cache verdict, so it is meaningful only on the
    // execution verbs that build a cache-aware `PlanRequest` (`run`/`plan`/a bare
    // task). `release` runs its own pipeline without the task cache, so the flag is
    // a no-op there and is rejected.
    if cli.execution.no_cache && !accepts_cache_mode(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--no-cache` only applies to task verbs (`toven run`/`toven plan`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    // `--refresh` shapes the same PLAN cache verdict as `--no-cache` (force a
    // re-run) and so is gated to the same cache-aware execution verbs.
    if cli.execution.refresh && !accepts_cache_mode(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--refresh` only applies to task verbs (`toven run`/`toven plan`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    // `--timeout` bounds APPLY execution and `--jobs` caps its concurrency, so —
    // like `--fail-fast`/`--watch` — both are meaningful only on the task-APPLY
    // verbs that actually run units.
    reject_apply_only_flag(
        cli.execution.timeout.is_some(),
        "--timeout",
        &cli.command,
        verb,
    )?;
    reject_apply_only_flag(cli.execution.jobs.is_some(), "--jobs", &cli.command, verb)?;
    // `--compute-budget` bounds the CPU parallelism handed to each spawned tool
    // during APPLY fan-out, so it is meaningful on the task-APPLY verbs and on
    // `coverage`, which runs its task the same way; elsewhere it is a no-op.
    if cli.execution.compute_budget.is_some() && !matches!(cli.command, Command::Coverage) {
        reject_apply_only_flag(true, "--compute-budget", &cli.command, verb)?;
    }
    gate_watch_flags(cli, verb)?;
    // `--base`/`--merge-base` only shape changed selection, and
    // `--module`/`--workspace`/`--with-dependents` shape explicit selection — both
    // belong to the same selection verbs; other verbs would ignore them.
    gate_selection_flags(cli, verb)?;
    Ok(())
}

/// Reject an APPLY-only flag (`--fail-fast`/`--timeout`/`--jobs`/
/// `--compute-budget`) on any verb that never runs units; elsewhere the flag
/// would be a silent no-op.
fn reject_apply_only_flag(
    present: bool,
    flag: &str,
    command: &Command,
    verb: &str,
) -> AppResult<()> {
    if present && !accepts_fail_fast(command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`{flag}` only applies when running tasks (`toven run`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    Ok(())
}

/// Reject `--view` on any verb other than the task-APPLY verbs that stream live
/// child output; elsewhere it is a silent no-op.
fn gate_view_flag(cli: &Cli, verb: &str) -> AppResult<()> {
    if cli.execution.view.is_some() && !accepts_fail_fast(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--view` only applies when running tasks (`toven run`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    Ok(())
}

/// Reject `--download`/`--no-run` on anything but `release verify`, the only
/// action that downloads hosted assets or runs the packaged binary; elsewhere
/// both flags are silent no-ops.
fn gate_verify_flags(cli: &Cli, verb: &str) -> AppResult<()> {
    let is_verify = matches!(release_action(&cli.command), Some(ReleaseAction::Verify));
    if is_verify {
        return Ok(());
    }
    for (present, flag) in [(cli.download, "--download"), (cli.no_run, "--no-run")] {
        if present {
            return Err(only_applies(flag, "toven release verify", verb));
        }
    }
    Ok(())
}

/// Reject `--target`/`--binary` on anything but `release package`, the only
/// action that packages a built binary into a declared archive asset.
fn gate_package_flags(cli: &Cli, verb: &str) -> AppResult<()> {
    let is_package = matches!(release_action(&cli.command), Some(ReleaseAction::Package));
    if is_package {
        return Ok(());
    }
    for (present, flag) in [
        (cli.target.is_some(), "--target"),
        (cli.binary.is_some(), "--binary"),
    ] {
        if present {
            return Err(only_applies(flag, "toven release package", verb));
        }
    }
    Ok(())
}

/// Reject `--out-dir` on anything but the artifact-writing release actions
/// (`release sbom`/`release depgraphs`); elsewhere it has no target directory.
fn gate_out_dir_flag(cli: &Cli, verb: &str) -> AppResult<()> {
    let writes_artifacts =
        release_action(&cli.command).is_some_and(ReleaseAction::writes_artifacts);
    if cli.out_dir.is_some() && !writes_artifacts {
        return Err(only_applies(
            "--out-dir",
            "toven release sbom/depgraphs",
            verb,
        ));
    }
    Ok(())
}

/// Reject the per-run bump argv (`--patch`/`--minor`/`--major`/`--set-version`/
/// `--pre`/`--offline`) on anything but the mutating release actions, which are
/// the only place a version cut happens.
fn gate_bump_flags(cli: &Cli, verb: &str, mutating_release: bool) -> AppResult<()> {
    if mutating_release {
        return Ok(());
    }
    for (present, flag) in [
        (!cli.patch.is_empty(), "--patch"),
        (!cli.minor.is_empty(), "--minor"),
        (!cli.major.is_empty(), "--major"),
        (!cli.set_version.is_empty(), "--set-version"),
        (cli.pre.is_some(), "--pre"),
        (cli.offline, "--offline"),
    ] {
        if present {
            return Err(only_applies(flag, "toven release bump/tag/publish", verb));
        }
    }
    Ok(())
}

/// Reject the `init`-only wizard flags (`--force`/`--root`/`--non-interactive`/
/// `--print`) on any other verb, and the release-only `--yes` anywhere but a
/// mutating release action.
fn gate_init_flags(cli: &Cli, verb: &str, is_init: bool) -> AppResult<()> {
    let release_confirmation =
        release_action(&cli.command).is_some_and(ReleaseAction::requires_confirmation);
    if cli.force.is_some() && !is_init {
        return Err(only_applies("--force", "toven init", verb));
    }
    if cli.root.is_some() && !is_init {
        return Err(only_applies("--root", "toven init", verb));
    }
    if cli.non_interactive && !is_init {
        return Err(only_applies("--non-interactive", "toven init", verb));
    }
    if cli.confirm_release && !release_confirmation {
        return Err(only_applies(
            "--yes",
            "toven release bump/tag/publish/image",
            verb,
        ));
    }
    if cli.print && !is_init {
        return Err(only_applies("--print", "toven init", verb));
    }
    Ok(())
}

/// Reject the coverage threshold-override flags (`--line`/`--function`/
/// `--region`/`--changed-line`/`--enforcement`) on any verb but `coverage`,
/// which is the only place a coverage gate runs.
fn gate_coverage_flags(cli: &Cli, verb: &str, is_coverage: bool) -> AppResult<()> {
    if is_coverage {
        return Ok(());
    }
    for (present, flag) in [
        (cli.line.is_some(), "--line"),
        (cli.function.is_some(), "--function"),
        (cli.region.is_some(), "--region"),
        (cli.changed_line.is_some(), "--changed-line"),
        (cli.enforcement.is_some(), "--enforcement"),
    ] {
        if present {
            return Err(only_applies(flag, "toven coverage", verb));
        }
    }
    Ok(())
}

/// Reject the selection flags (`--base`/`--merge-base`,
/// `--module`/`--workspace`/`--dependents`/`--dependencies`) on a verb that
/// performs no selection.
fn gate_selection_flags(cli: &Cli, verb: &str) -> AppResult<()> {
    // `--base` is also the release diff baseline for the mutating release actions;
    // `--merge-base` stays restricted to the changed-selection verbs.
    let mutating_release = release_action(&cli.command).is_some_and(ReleaseAction::is_mutating);
    if cli.execution.base.is_some() && !accepts_baseline(&cli.command) && !mutating_release {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--base` only applies to change-selection commands (`toven run`/`toven plan`/`toven affected`/`toven <task>`) and `toven release tag/publish`; it has no effect on `toven {verb}`"
            ),
        ));
    }
    if cli.execution.merge_base && !accepts_baseline(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--merge-base` only applies to change-selection commands (`toven run`/`toven plan`/`toven affected`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    if !accepts_selection(&cli.command)
        && (!cli.execution.module.is_empty()
            || !cli.execution.workspace.is_empty()
            || cli.execution.with_dependents
            || cli.execution.with_dependencies)
    {
        let flag = if !cli.execution.module.is_empty() {
            "--module"
        } else if !cli.execution.workspace.is_empty() {
            "--workspace"
        } else if cli.execution.with_dependents {
            "--dependents"
        } else {
            "--dependencies"
        };
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`{flag}` only applies to selection commands (`toven run`/`toven plan`/`toven affected`/`toven explain`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    Ok(())
}

/// Reject the watch flags (`--watch`, `--watch-debounce-ms`) on a verb that
/// runs no APPLY loop, then reject contradictory execution flag combinations.
fn gate_watch_flags(cli: &Cli, verb: &str) -> AppResult<()> {
    if cli.execution.watch && !accepts_watch(&cli.command) {
        return Err(AppError::invalid_input(
            "flags",
            format!(
                "`--watch` only applies when running tasks (`toven run`/`toven <task>`); it has no effect on `toven {verb}`"
            ),
        ));
    }
    // A bare task's trailing flags are merged first; `ExecutionArgs::for_task`
    // checks the merged set.
    if matches!(cli.command, Command::External(_)) {
        return Ok(());
    }
    cli.execution.check_combinations(cli.is_plan_only())
}

/// Whether `command` is a task-APPLY verb that consumes `--watch`.
///
/// Watch reruns the affected subgraph through APPLY, so — like `--fail-fast` —
/// only the verbs that drive multi-unit APPLY scheduling read it: `plan` stops
/// at PLAN and `release` runs a single linear pipeline.
const fn accepts_watch(command: &Command) -> bool {
    matches!(command, Command::Run { .. } | Command::External(_))
}

/// The release lifecycle action `command` carries, if it is `toven release`.
const fn release_action(command: &Command) -> Option<ReleaseAction> {
    match command {
        Command::Release { action } => Some(*action),
        _ => None,
    }
}

/// Whether `--dry-run` is meaningful for `command`: a PLAN cut for the task
/// verbs and a no-mutation rehearsal for the rehearsable release actions.
const fn accepts_dry_run(command: &Command) -> bool {
    match command {
        Command::Run { .. } | Command::Plan { .. } | Command::External(_) => true,
        Command::Release { action } => action.accepts_dry_run(),
        _ => false,
    }
}

/// Whether `--explain` (task-planning reasoning detail) is meaningful for
/// `command`; only the task-planning verbs surface it.
const fn accepts_explain(command: &Command) -> bool {
    matches!(
        command,
        Command::Run { .. } | Command::Plan { .. } | Command::External(_)
    )
}

/// Whether `command` builds the human run reporter that `-v`/`-q`/`--color`
/// shape: the execution verbs, the mutating release actions, and `coverage`
/// (which runs the coverage task through a human reporter).
const fn accepts_reporter_shaping(command: &Command) -> bool {
    match command {
        Command::Run { .. } | Command::Plan { .. } | Command::External(_) | Command::Coverage => {
            true
        }
        Command::Release { action } => action.is_mutating(),
        _ => false,
    }
}

/// Whether `command` renders a projection whose format `--output` selects: the
/// execution verbs, every release lifecycle action, `coverage`, the
/// `tasks`/`modules` listings, and the `affected` projection.
const fn accepts_output_format(command: &Command) -> bool {
    matches!(
        command,
        Command::Run { .. }
            | Command::Plan { .. }
            | Command::External(_)
            | Command::Release { .. }
            | Command::Coverage
            | Command::Tasks { .. }
            | Command::Doctor { .. }
            | Command::CommitLint { .. }
            | Command::Modules
            | Command::Affected { .. }
    )
}

/// Whether `command` is a task-APPLY verb that consumes `--fail-fast`.
///
/// Only the verbs that drive multi-unit APPLY scheduling read fail-fast; `plan`
/// stops at PLAN and `release` runs a single linear pipeline, so the flag is a
/// no-op for them.
const fn accepts_fail_fast(command: &Command) -> bool {
    matches!(command, Command::Run { .. } | Command::External(_))
}

/// Whether `command` builds a cache-aware `PlanRequest` and thus consumes
/// `--no-cache`.
///
/// `run`/`plan`/a bare task all run the cache-aware PLAN spine; `release` runs
/// a separate pipeline without the task cache, so the flag is a no-op there.
const fn accepts_cache_mode(command: &Command) -> bool {
    matches!(
        command,
        Command::Run { .. } | Command::Plan { .. } | Command::External(_)
    )
}

/// Whether `command` performs changed selection and thus consumes the baseline
/// flags (`--base`/`--merge-base`).
const fn accepts_baseline(command: &Command) -> bool {
    matches!(
        command,
        Command::Run { .. }
            | Command::Plan { .. }
            | Command::Affected { .. }
            | Command::Coverage
            | Command::External(_)
    )
}

/// Whether `command` resolves an explicit graph selection and thus consumes the
/// selection flags (`--module`/`--workspace`/`--dependents`/`--dependencies`).
const fn accepts_selection(command: &Command) -> bool {
    matches!(
        command,
        Command::Run { .. }
            | Command::Plan { .. }
            | Command::Affected { .. }
            | Command::Explain { .. }
            | Command::Coverage
            | Command::External(_)
    )
}

fn only_applies(flag: &str, owner: &str, verb: &str) -> AppError {
    AppError::invalid_input(
        "flags",
        format!("`{flag}` only applies to `{owner}` (used with `toven {verb}`)"),
    )
}

/// The user-facing name of the dispatched verb (for error messages). Release
/// actions include the action so a gating error names the exact subcommand
/// (`release status`) rather than the bare `release`.
fn verb_name(command: &Command) -> String {
    match command {
        Command::Run { .. } => "run".to_string(),
        Command::Plan { .. } => "plan".to_string(),
        Command::Release { action } => format!("release {}", action.as_str()),
        Command::Coverage => "coverage".to_string(),
        Command::Explain { .. } => "explain".to_string(),
        Command::Init => "init".to_string(),
        Command::Affected { .. } => "affected".to_string(),
        Command::Modules => "modules".to_string(),
        Command::Graph => "graph".to_string(),
        Command::Tasks { .. } => "tasks".to_string(),
        Command::Doctor { .. } => "doctor".to_string(),
        Command::CommitLint { .. } => "commit-lint".to_string(),
        Command::Completions { .. } => "completions".to_string(),
        Command::Driver { .. } => "driver".to_string(),
        Command::Federation { .. } => "federation".to_string(),
        Command::Cache { .. } => "cache".to_string(),
        Command::External(tokens) => tokens.first().map_or("<task>", String::as_str).to_string(),
    }
}
