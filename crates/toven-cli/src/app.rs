//! The public CLI entry point: parse argv, gate flag applicability, load the
//! project, warn on task/reserved collisions, dispatch the verb, and map any
//! typed error to a process [`ExitCode`].
//!
//! This is the one place that ties the argv-first grammar to the engine spine.
//! The three apps (`apps/{toven, toven-rs, toven-go}`) are thin: each builds its
//! provider set and calls [`run`]. Everything user-facing is printed by the
//! reporter sinks or the verb projections; this module prints only the final
//! rendered error.

use clap::Parser;
use clap::error::ErrorKind;
use rskit_cli::{ErrorRenderer, ExitCode};
use rskit_errors::{AppError, AppResult};
use toven_engine::plan::addressable_task_names;
use toven_ports::{Provider, TaskKind};

use crate::flags::{Cli, Command, GraphFormat};
use crate::host::{Project, Report};
use crate::{collision, commands, flags, grammar, host};

/// Run the Toven CLI against the compiled-in `providers`.
///
/// Parses the process argv, dispatches the argv-first grammar, and returns the
/// process exit code. Never panics: clap parse outcomes and typed engine errors
/// are both mapped to an [`ExitCode`]; the apps pass the result to
/// [`std::process::exit`].
#[must_use]
pub fn run(providers: &[&dyn Provider]) -> ExitCode {
    run_from(providers, std::env::args_os())
}

/// Run the CLI against an explicit argument vector (the testable core of
/// [`run`]).
///
/// `args` includes the program name as its first element, exactly like
/// [`std::env::args_os`]. Embedders and tests call this directly to drive a
/// specific argv without touching the process environment.
#[must_use]
pub fn run_from<I, T>(providers: &[&dyn Provider], args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => return clap_exit(&error),
    };

    match dispatch(providers, &cli) {
        Ok(code) => code,
        Err(error) => {
            let (rendered, code) = ErrorRenderer::default().render(&error);
            eprintln!("{rendered}");
            code
        }
    }
}

/// Render a typed wiring/bootstrap failure and map it to a process exit code.
///
/// The apps (`apps/{toven, toven-rs, toven-go}`) construct their provider set
/// before [`run`] takes over; a construction failure there has no reporter yet.
/// Routing it through this helper keeps the apps free of any user-facing
/// formatting — the shared [`ErrorRenderer`] is the one place errors are
/// rendered, exactly as the dispatch loop does for engine errors.
#[must_use]
pub fn report_error(error: &AppError) -> ExitCode {
    let (rendered, code) = ErrorRenderer::default().render(error);
    eprintln!("{rendered}");
    code
}

/// Print a clap parse outcome and map it to an exit code (help/version succeed;
/// everything else is a usage error).
fn clap_exit(error: &clap::Error) -> ExitCode {
    let _ = error.print();
    match error.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayVersion
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => ExitCode::Success,
        _ => ExitCode::Usage,
    }
}

/// Gate flag applicability, then route the parsed verb to its command module.
fn dispatch(providers: &[&dyn Provider], cli: &Cli) -> AppResult<ExitCode> {
    flags::gate(cli)?;

    match &cli.command {
        Command::Generate => commands::generate::execute(),
        Command::Driver { action } => commands::driver::driver(action, cli.auto_install),
        Command::Federation { action } => commands::driver::federation(action, cli.auto_install),
        Command::External(tokens) => dispatch_task(providers, cli, tokens),
        Command::Run { task, passthrough } => {
            let project = load(providers, cli)?;
            let report = Report::resolve(cli.output, cli.verbosity(), &project.document);
            commands::run::execute(
                providers,
                &project,
                report,
                intent_for(task),
                passthrough.clone(),
                cli.fail_fast,
                cli.is_plan_only(),
            )
        }
        Command::Plan { task } => {
            let project = load(providers, cli)?;
            let report = Report::resolve(cli.output, cli.verbosity(), &project.document);
            commands::run::execute(
                providers,
                &project,
                report,
                intent_for(task),
                Vec::new(),
                cli.fail_fast,
                true,
            )
        }
        Command::Release => {
            let project = load(providers, cli)?;
            let report = Report::resolve(cli.output, cli.verbosity(), &project.document);
            commands::run::release(
                providers,
                &project,
                report,
                cli.allow_dirty,
                cli.no_push,
                cli.is_plan_only(),
            )
        }
        Command::Explain { module, task } => {
            let project = load(providers, cli)?;
            commands::introspect::explain(providers, &project, module, intent_for(task))
        }
        Command::Affected { task } => {
            let project = load(providers, cli)?;
            commands::introspect::affected(providers, &project, intent_for(task))
        }
        Command::Modules => {
            let project = load(providers, cli)?;
            commands::introspect::modules(providers, &project)
        }
        Command::Graph => {
            let project = load(providers, cli)?;
            commands::introspect::graph(
                providers,
                &project,
                cli.format.unwrap_or(GraphFormat::Text),
            )
        }
        Command::Cache { action } => {
            let project = load(providers, cli)?;
            commands::cache::execute(&project, action)
        }
    }
}

/// Dispatch a bare argv-first task: re-parse its trailing flags + passthrough,
/// then merge them with the pre-token global flags and run.
fn dispatch_task(providers: &[&dyn Provider], cli: &Cli, tokens: &[String]) -> AppResult<ExitCode> {
    let invocation = grammar::parse_task(tokens)?;
    let flags = &invocation.flags;

    let config = flags.config.clone().or_else(|| cli.config.clone());
    let project = load_with_config(providers, config.as_deref())?;

    let output = flags.output.or(cli.output);
    let verbosity = flags::Verbosity::from_counts(
        cli.verbose.saturating_add(flags.verbose),
        cli.quiet.saturating_add(flags.quiet),
    );
    let report = Report::resolve(output, verbosity, &project.document);
    let plan_only = cli.is_plan_only() || flags.dry_run || flags.explain;
    let fail_fast = cli.fail_fast || flags.fail_fast;

    commands::run::execute(
        providers,
        &project,
        report,
        intent_for(&invocation.task),
        invocation.passthrough,
        fail_fast,
        plan_only,
    )
}

/// Load the project using the verb's `--config` global flag.
fn load(providers: &[&dyn Provider], cli: &Cli) -> AppResult<Project> {
    load_with_config(providers, cli.config.as_deref())
}

/// Load the project at the resolved config path and emit collision warnings.
fn load_with_config(
    providers: &[&dyn Provider],
    config: Option<&std::path::Path>,
) -> AppResult<Project> {
    let config_path = host::discover_config(config)?;
    let project = host::load_project(&config_path, providers)?;
    warn_collisions(&project, providers);
    Ok(project)
}

/// Emit a stderr warning for every task name that shadows a reserved verb.
///
/// Best-effort: a configuration that cannot even enumerate its task names will
/// surface that same error from the verb's own PLAN call with full context, so
/// the warning pass stays silent rather than pre-empting it.
fn warn_collisions(project: &Project, providers: &[&dyn Provider]) {
    let Ok(names) = addressable_task_names(&project.document, providers) else {
        return;
    };
    for collision in collision::detect(names.iter().map(String::as_str)) {
        eprintln!("{}", collision.message());
    }
}

/// Resolve a task token to a built-in [`TaskKind`] or a [`TaskKind::Custom`].
fn intent_for(task: &str) -> TaskKind {
    TaskKind::builtin(task).unwrap_or_else(|| TaskKind::Custom(task.to_string()))
}
