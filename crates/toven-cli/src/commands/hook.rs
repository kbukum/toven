//! The concrete CLI [`HookRunner`]: run a configured lifecycle-hook task
//! reference through the standard PLAN → APPLY run path.
//!
//! A `[…release].hooks` entry names a task the workspace already knows, so a
//! hook reuses the exact run path (output, caching, and failure semantics) that
//! drives `toven <task>` instead of a bespoke execution engine. The engine's
//! sync release flow calls this injected runner at the pre/post points; the
//! runner owns the async task-apply runtime internally, keeping async out of the
//! L2 release path.

use rskit_cli::ExitCode;
use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{HookPhase, HookRunner, Provider, TaskIntent};

use crate::commands::run::WatchFlags;
use crate::commands::selection::TaskSelection;
use crate::flags::Cli;
use crate::host::{Project, Report};

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
