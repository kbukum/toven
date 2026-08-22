//! The concrete CLI [`HookRunner`]: run a configured lifecycle-hook task
//! reference through the standard PLAN → APPLY run path, plus the
//! [`run_with_lifecycle`] driver that wraps any unit in its before/after hooks.
//!
//! A `[hooks.<verb>]` entry names a task the workspace already knows, so a hook
//! reuses the exact run path (output, caching, and failure semantics) that
//! drives `toven <task>` instead of a bespoke execution engine. The CLI dispatch
//! seam calls [`run_with_lifecycle`] to run a unit's resolved `before` hooks
//! (fail-closed) before the unit body and its `after` hooks after the body
//! succeeds; the same [`CliHookRunner`] also serves the bump
//! [`HookInvocation::OnResolved`] mid-mutation seam. The runner owns the async
//! task-apply runtime internally, keeping async out of the L2 engine path.

use rskit_cli::ExitCode;
use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{HookInvocation, HookRunner, HooksConfig, Provider, TaskIntent};

use crate::commands::run::WatchFlags;
use crate::commands::selection::TaskSelection;
use crate::flags::Cli;
use crate::host::{Project, Report};

/// Run `body` wrapped in its lifecycle `hooks`: every `before` reference first
/// (fail-closed — a failing `before` aborts before the unit body runs), then the
/// unit body, then every `after` reference only when the body exits
/// [`ExitCode::Success`].
///
/// This is the general per-unit lifecycle driver the CLI dispatch seam uses for
/// any unit. Hook references run through the injected `runner` (the real
/// [`CliHookRunner`] in production, a recording double in tests) — the same one
/// mechanism that serves the [`HookInvocation::OnResolved`] seam. When no hook is
/// configured the body runs unwrapped, so an unconfigured unit pays nothing.
///
/// # Errors
/// Propagates a `before` hook failure (before the body runs), the body's own
/// failure, or an `after` hook failure (after a successful body).
pub(crate) fn run_with_lifecycle(
    hooks: &HooksConfig,
    runner: &dyn HookRunner,
    body: impl FnOnce() -> AppResult<ExitCode>,
) -> AppResult<ExitCode> {
    for reference in &hooks.pre {
        runner.run_hook(HookInvocation::Before, reference)?;
    }
    let code = body()?;
    if code == ExitCode::Success {
        for reference in &hooks.post {
            runner.run_hook(HookInvocation::After, reference)?;
        }
    }
    Ok(code)
}

/// Runs a configured hook reference as a whole-workspace task via the run verb.
///
/// Holds only shared borrows of the compiled providers, the loaded project, and
/// the parsed argv, so it can build a fresh reporter + runtime per hook without
/// contending with the release reporter the engine already holds.
pub(crate) struct CliHookRunner<'a> {
    providers: &'a [&'a dyn Provider],
    supervisor: &'a std::sync::Arc<toven_exec::ProcessSupervisor>,
    project: &'a Project,
    cli: &'a Cli,
}

impl<'a> CliHookRunner<'a> {
    /// Bind the runner to the providers, project, and argv it plans/applies with.
    pub(crate) fn new(
        providers: &'a [&'a dyn Provider],
        supervisor: &'a std::sync::Arc<toven_exec::ProcessSupervisor>,
        project: &'a Project,
        cli: &'a Cli,
    ) -> Self {
        Self {
            providers,
            supervisor,
            project,
            cli,
        }
    }
}

impl CliHookRunner<'_> {
    /// Run the task named `reference` across the whole workspace (default
    /// selection), fail-fast, appending `passthrough` to its argv — the shared
    /// task-run path behind both the whole-unit before/after hooks and the bump
    /// `on-resolved` seam.
    ///
    /// An unknown reference fails during scheduling; a non-zero task result is
    /// mapped to a typed error by the caller.
    fn run_task(&self, reference: &str, passthrough: Vec<String>) -> AppResult<ExitCode> {
        let report = Report::resolve(
            self.cli.output,
            self.cli.verbosity(),
            self.cli.color_choice(),
            &self.project.document,
        );
        crate::commands::run::execute(
            self.providers,
            self.supervisor,
            self.project,
            report,
            TaskIntent::resolve(reference),
            passthrough,
            true,
            self.cli.no_cache,
            self.cli.refresh,
            self.cli.timeout,
            false,
            WatchFlags {
                enabled: false,
                debounce_ms: 0,
            },
            None,
            self.cli.jobs,
            self.cli.compute_budget,
            &TaskSelection::default(),
        )
    }
}

impl HookRunner for CliHookRunner<'_> {
    fn run_hook(&self, invocation: HookInvocation<'_>, reference: &str) -> AppResult<()> {
        // A hook runs the named task across the whole workspace (default
        // selection), fail-fast so a gate stops on the first failing unit. The
        // `on-resolved` seam additionally hands the task the authoritative
        // version-map path argv-first (appended to its argv, no implicit shell),
        // so it can rewrite related files the native version-reference sync does
        // not cover; before/after lifecycle hooks pass no extra argv. An unknown
        // reference fails during scheduling; a non-zero task result is mapped to
        // a typed error so the run aborts (before/on-resolved) or is reported as
        // failed (after).
        let passthrough = invocation
            .version_map()
            .map(|path| vec![path.to_string_lossy().into_owned()])
            .unwrap_or_default();
        let code = self.run_task(reference, passthrough)?;
        gate_hook_result(
            code,
            &format!(
                "the {} hook task '{reference}'",
                invocation.phase().as_str()
            ),
        )
    }
}

/// Map a hook task's terminal [`ExitCode`] to `Ok`/`Err` uniformly.
///
/// Every lifecycle phase — before, on-resolved, and after — runs a named task
/// through the same run path and gates its result identically: success passes,
/// any non-success exit becomes a typed [`ErrorCode::ExternalService`] error
/// naming the task and its exit code, so a failing hook fails closed the same
/// way everywhere.
///
/// # Errors
/// Returns [`ErrorCode::ExternalService`] when `code` is not
/// [`ExitCode::Success`].
fn gate_hook_result(code: ExitCode, context: &str) -> AppResult<()> {
    if code == ExitCode::Success {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorCode::ExternalService,
            format!("{context} failed (exit code {})", code.as_i32()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use rskit_cli::ExitCode;
    use toven_ports::{HookPhase, HooksConfig};
    use toven_testkit::RecordingHookRunner;

    use super::run_with_lifecycle;

    fn hooks(pre: &[&str], post: &[&str]) -> HooksConfig {
        HooksConfig {
            pre: pre
                .iter()
                .map(|reference| (*reference).to_string())
                .collect(),
            post: post
                .iter()
                .map(|reference| (*reference).to_string())
                .collect(),
        }
    }

    #[test]
    fn before_runs_before_the_unit_and_after_after_success() {
        let runner = RecordingHookRunner::new();
        let ran = Cell::new(false);

        let code = run_with_lifecycle(&hooks(&["gate"], &["notify"]), &runner, || {
            // The body runs only after every `before` hook has been recorded.
            assert_eq!(
                runner.references(HookPhase::Before),
                vec!["gate".to_string()]
            );
            assert!(runner.references(HookPhase::After).is_empty());
            ran.set(true);
            Ok(ExitCode::Success)
        })
        .expect("the unit runs");

        assert_eq!(code, ExitCode::Success);
        assert!(ran.get(), "the unit body ran");
        assert_eq!(
            runner.references(HookPhase::After),
            vec!["notify".to_string()]
        );
    }

    #[test]
    fn one_mechanism_wraps_an_argv_unit_and_a_native_unit_identically() {
        // Acceptance: the *same* lifecycle mechanism wraps an Argv task unit and
        // a Native capability unit identically — the driver is unit-agnostic, so
        // however differently the two unit bodies behave, the before/after hook
        // wiring around them is identical. The bodies below genuinely differ
        // (each records its own kind), yet the recorded hook invocations match.
        let argv_runner = RecordingHookRunner::new();
        let native_runner = RecordingHookRunner::new();
        let body_log: Cell<&str> = Cell::new("");

        // An `Argv` task unit body (e.g. `toven test`).
        run_with_lifecycle(&hooks(&["gate"], &["notify"]), &argv_runner, || {
            body_log.set("argv-task");
            Ok(ExitCode::Success)
        })
        .expect("the argv unit runs");
        assert_eq!(body_log.get(), "argv-task", "the argv body ran");

        // A `Native` capability unit body (e.g. the bump mutation).
        run_with_lifecycle(&hooks(&["gate"], &["notify"]), &native_runner, || {
            body_log.set("native-bump");
            Ok(ExitCode::Success)
        })
        .expect("the native unit runs");
        assert_eq!(body_log.get(), "native-bump", "the native body ran");

        assert_eq!(
            argv_runner.calls(),
            native_runner.calls(),
            "the one hook mechanism wraps an Argv and a Native unit identically"
        );
    }

    #[test]
    fn a_failing_before_hook_aborts_before_the_unit_body() {
        let runner = RecordingHookRunner::failing_on("gate");
        let ran = Cell::new(false);

        let error = run_with_lifecycle(&hooks(&["gate"], &["notify"]), &runner, || {
            ran.set(true);
            Ok(ExitCode::Success)
        })
        .expect_err("a failing before hook fails the unit closed");

        assert!(error.to_string().contains("gate"), "{error}");
        assert!(!ran.get(), "the unit body never ran");
        assert!(
            runner.references(HookPhase::After).is_empty(),
            "no after hook runs once before aborts"
        );
    }

    #[test]
    fn a_failing_unit_skips_after_hooks() {
        let runner = RecordingHookRunner::new();

        let code = run_with_lifecycle(&hooks(&["gate"], &["notify"]), &runner, || {
            Ok(ExitCode::Failure)
        })
        .expect("the driver returns the unit's non-success code");

        assert_eq!(code, ExitCode::Failure);
        assert_eq!(
            runner.references(HookPhase::Before),
            vec!["gate".to_string()]
        );
        assert!(
            runner.references(HookPhase::After).is_empty(),
            "after hooks are skipped when the unit does not succeed"
        );
    }
}
