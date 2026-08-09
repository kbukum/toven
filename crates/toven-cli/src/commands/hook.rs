//! The concrete CLI [`HookRunner`]: run a configured lifecycle-hook task
//! reference through the standard PLAN → APPLY run path, plus the
//! [`run_with_lifecycle`] driver that wraps any verb in its `pre`/`post` hooks.
//!
//! A `[hooks.<verb>]` entry names a task the workspace already knows, so a hook
//! reuses the exact run path (output, caching, and failure semantics) that
//! drives `toven <task>` instead of a bespoke execution engine. The CLI dispatch
//! seam calls [`run_with_lifecycle`] to run a verb's resolved `pre` hooks
//! (fail-closed) before the verb body and its `post` hooks after the body
//! succeeds; the runner owns the async task-apply runtime internally, keeping
//! async out of the L2 engine path.

use rskit_cli::ExitCode;
use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{HookPhase, HookRunner, HooksConfig, Provider, TaskIntent};

use crate::commands::run::WatchFlags;
use crate::commands::selection::TaskSelection;
use crate::flags::Cli;
use crate::host::{Project, Report};

/// Run `body` wrapped in its lifecycle `hooks`: every `pre` reference first
/// (fail-closed — a failing `pre` aborts before the verb body runs), then the
/// verb body, then every `post` reference only when the body exits
/// [`ExitCode::Success`].
///
/// This is the general per-verb lifecycle driver the CLI dispatch seam uses for
/// any verb. Hook references run through the injected `runner` (the real
/// [`CliHookRunner`] in production, a recording double in tests). When no hook
/// is configured the body runs unwrapped, so an unconfigured verb pays nothing.
///
/// # Errors
/// Propagates a `pre` hook failure (before the body runs), the body's own
/// failure, or a `post` hook failure (after a successful body).
pub(crate) fn run_with_lifecycle(
    hooks: &HooksConfig,
    runner: &dyn HookRunner,
    body: impl FnOnce() -> AppResult<ExitCode>,
) -> AppResult<ExitCode> {
    for reference in &hooks.pre {
        runner.run_hook(HookPhase::Pre, reference)?;
    }
    let code = body()?;
    if code == ExitCode::Success {
        for reference in &hooks.post {
            runner.run_hook(HookPhase::Post, reference)?;
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
    project: &'a Project,
    cli: &'a Cli,
}

impl<'a> CliHookRunner<'a> {
    /// Bind the runner to the providers, project, and argv it plans/applies with.
    pub(crate) fn new(
        providers: &'a [&'a dyn Provider],
        project: &'a Project,
        cli: &'a Cli,
    ) -> Self {
        Self {
            providers,
            project,
            cli,
        }
    }
}

impl HookRunner for CliHookRunner<'_> {
    fn run_hook(&self, phase: HookPhase, reference: &str) -> AppResult<()> {
        // A hook runs the named task across the whole workspace (default
        // selection), fail-fast so a gate stops on the first failing unit. An
        // unknown reference fails during scheduling; a non-zero task result is
        // mapped to a typed error so the release aborts (pre) or is reported
        // as failed (post).
        let report = Report::resolve(
            self.cli.output,
            self.cli.verbosity(),
            self.cli.color_choice(),
            &self.project.document,
        );
        let code = crate::commands::run::execute(
            self.providers,
            self.project,
            report,
            TaskIntent::resolve(reference),
            Vec::new(),
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
            &TaskSelection::default(),
        )?;
        if code == ExitCode::Success {
            Ok(())
        } else {
            Err(AppError::new(
                ErrorCode::ExternalService,
                format!(
                    "the {} release hook task '{reference}' failed (exit code {})",
                    phase.as_str(),
                    code.as_i32()
                ),
            ))
        }
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
    fn pre_runs_before_the_verb_and_post_after_success() {
        let runner = RecordingHookRunner::new();
        let ran = Cell::new(false);

        let code = run_with_lifecycle(&hooks(&["gate"], &["notify"]), &runner, || {
            // The body runs only after every `pre` hook has been recorded.
            assert_eq!(runner.references(HookPhase::Pre), vec!["gate".to_string()]);
            assert!(runner.references(HookPhase::Post).is_empty());
            ran.set(true);
            Ok(ExitCode::Success)
        })
        .expect("the verb runs");

        assert_eq!(code, ExitCode::Success);
        assert!(ran.get(), "the verb body ran");
        assert_eq!(
            runner.references(HookPhase::Post),
            vec!["notify".to_string()]
        );
    }

    #[test]
    fn a_failing_pre_hook_aborts_before_the_verb_body() {
        let runner = RecordingHookRunner::failing_on("gate");
        let ran = Cell::new(false);

        let error = run_with_lifecycle(&hooks(&["gate"], &["notify"]), &runner, || {
            ran.set(true);
            Ok(ExitCode::Success)
        })
        .expect_err("a failing pre hook fails the verb closed");

        assert!(error.to_string().contains("gate"), "{error}");
        assert!(!ran.get(), "the verb body never ran");
        assert!(
            runner.references(HookPhase::Post).is_empty(),
            "no post hook runs once pre aborts"
        );
    }

    #[test]
    fn a_failing_verb_skips_post_hooks() {
        let runner = RecordingHookRunner::new();

        let code = run_with_lifecycle(&hooks(&["gate"], &["notify"]), &runner, || {
            Ok(ExitCode::Failure)
        })
        .expect("the driver returns the verb's non-success code");

        assert_eq!(code, ExitCode::Failure);
        assert_eq!(runner.references(HookPhase::Pre), vec!["gate".to_string()]);
        assert!(
            runner.references(HookPhase::Post).is_empty(),
            "post hooks are skipped when the verb does not succeed"
        );
    }
}
