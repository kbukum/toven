//! The public CLI entry point: parse argv, gate flag applicability, load the
//! project, warn on task/reserved collisions for argv-first task dispatch,
//! dispatch the verb, and map any typed error to a process [`ExitCode`].
//!
//! This is the one place that ties the argv-first grammar to the engine spine.
//! The three apps (`apps/{toven, toven-rs, toven-go}`) are thin: each builds
//! its provider set and calls [`run`]. Everything user-facing is printed by the
//! reporter sinks or the verb projections; this module prints only the final
//! rendered error.

use std::sync::Arc;

use clap::Parser;
use clap::error::ErrorKind;
use rskit_cli::{ErrorRenderer, ExitCode};
use rskit_errors::{AppError, AppResult};
use toven_core::config::VerbId;
use toven_core::plan::addressable_task_names;
use toven_exec::ProcessSupervisor;
use toven_ports::{Provider, TaskIntent};

use crate::flags::{Cli, Command, ExecutionArgs, GraphFormat};
use crate::host::{Project, Report};
use crate::{collision, commands, flags, grammar, host};

/// Run the Toven CLI against the compiled-in `providers`.
///
/// Parses the process argv, dispatches the argv-first grammar, and returns the
/// process exit code. `supervisor` is the one shared process supervisor the app
/// composition root also built the provider tool runner with, so every child
/// spawned across PLAN and APPLY reaps through a single process-level backstop.
/// Never panics: clap parse outcomes and typed engine errors are both mapped to
/// an [`ExitCode`]; the apps pass the result to [`std::process::exit`].
#[must_use]
pub fn run(providers: &[&dyn Provider], supervisor: &Arc<ProcessSupervisor>) -> ExitCode {
    run_from(providers, supervisor, std::env::args_os())
}

/// Run the CLI against an explicit argument vector (the testable core of
/// [`run`]).
///
/// `args` includes the program name as its first element, exactly like
/// [`std::env::args_os`]. Embedders and tests call this directly to drive a
/// specific argv without touching the process environment. `supervisor` is the
/// one shared process supervisor the app composition root also registered the
/// provider tool runner with, so every child spawned across PLAN and APPLY reaps
/// through a single process-level backstop.
#[must_use]
pub fn run_from<I, T>(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    args: I,
) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    // The hidden `__serve` port-server entry is intercepted before clap: a driven
    // `toven-<eco> __serve` runs the engine's framed stdio loop over the in-proc
    // providers and never touches the reserved-verb grammar. stdout carries the
    // frame stream; diagnostics (and any error) go to stderr.
    if is_serve_invocation(&args) {
        return commands::driver::serve(providers);
    }
    // The sibling `__init` entry runs the config-less wizard exchange so `toven
    // init` can probe an out-of-process `toven-<eco>` driver.
    if is_init_invocation(&args) {
        return commands::driver::init_wizard(providers);
    }

    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => return clap_exit(&error),
    };

    match dispatch(providers, supervisor, &cli) {
        Ok(code) => code,
        Err(error) => {
            let (rendered, code) = ErrorRenderer::default().render(&error);
            eprintln!("{rendered}");
            code
        }
    }
}

/// The reserved hidden subcommand that puts a driver into port-server mode.
const SERVE_SUBCOMMAND: &str = "__serve";

/// The reserved hidden subcommand that runs the config-less wizard exchange.
const INIT_SUBCOMMAND: &str = "__init";

/// Whether the argv selects the hidden `__serve` port-server entry.
///
/// Requires `__serve` to be the *sole* argument after the program name: the
/// port-server loop takes no flags or positional arguments (everything is
/// driven over the framed stdio transport), so `toven-<eco> __serve <anything
/// else>` must fall through to clap and fail fast rather than silently starting
/// the loop and blocking on stdin.
fn is_serve_invocation(args: &[std::ffi::OsString]) -> bool {
    is_sole_subcommand(args, SERVE_SUBCOMMAND)
}

/// Whether the argv selects the hidden `__init` entry (same sole-argument
/// discipline as [`is_serve_invocation`]).
fn is_init_invocation(args: &[std::ffi::OsString]) -> bool {
    is_sole_subcommand(args, INIT_SUBCOMMAND)
}

/// Whether `args` is exactly `<program> <subcommand>` (the hidden-entry shape).
fn is_sole_subcommand(args: &[std::ffi::OsString], subcommand: &str) -> bool {
    args.len() == 2 && args[1].as_os_str() == std::ffi::OsStr::new(subcommand)
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
    // `error.print()` writes help/usage (or the parse error) to stderr/stdout as
    // clap chooses. A failure to write that diagnostic is itself unrecoverable here
    // — there is no second channel to report it on, and we still owe the caller the
    // mapped exit code — so the write result is intentionally ignored.
    let _ = error.print();
    match error.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayVersion
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => ExitCode::Success,
        _ => ExitCode::Usage,
    }
}

/// Gate flag applicability, then route the parsed verb to its command module.
/// Resolve the reporter binding for an execution verb from the global flags and
/// the loaded project document.
fn resolve_report(execution: &ExecutionArgs, project: &Project) -> Report {
    Report::resolve(
        execution.output,
        execution.verbosity(),
        execution.color_choice(),
        &project.document,
    )
}

/// Wrap a verb body in its project-level lifecycle hooks.
///
/// Resolves the composed `pre`/`post` hooks for `verb` from the loaded document
/// and runs `body` through [`commands::hook::run_with_lifecycle`] using the real
/// [`CliHookRunner`]: every `pre` reference runs fail-closed before the body, and
/// every `post` reference runs only after the body exits successfully. An
/// unconfigured verb runs the body directly, so it pays no hook cost.
fn with_hooks(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    execution: &ExecutionArgs,
    project: &Project,
    verb: VerbId,
    body: impl FnOnce() -> AppResult<ExitCode>,
) -> AppResult<ExitCode> {
    let hooks = project.document.hooks_for(verb);
    if hooks.is_empty() {
        return body();
    }
    let runner = commands::hook::CliHookRunner::new(providers, supervisor, project, execution);
    commands::hook::run_with_lifecycle(&hooks, &runner, body)
}

#[allow(clippy::too_many_lines)] // a flat verb→handler dispatch table: one arm per reserved verb
fn dispatch(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    cli: &Cli,
) -> AppResult<ExitCode> {
    flags::gate(cli)?;

    match &cli.command {
        Command::Init => commands::init::execute(providers, cli),
        Command::Driver { action } => {
            let project = load(providers, cli, false)?;
            commands::driver::driver(providers, &project, action, cli.auto_install)
        }
        Command::Federation { action } => {
            let project = load(providers, cli, false)?;
            commands::driver::federation(providers, &project, action, cli.auto_install)
        }
        Command::External(tokens) => dispatch_task(providers, supervisor, cli, tokens),
        Command::Run { task, passthrough } => {
            let project = load(providers, cli, true)?;
            run_tasks(
                providers,
                supervisor,
                &cli.execution,
                &project,
                task,
                passthrough.clone(),
            )
        }
        Command::Plan { task } => {
            let project = load(providers, cli, true)?;
            with_hooks(
                providers,
                supervisor,
                &cli.execution,
                &project,
                VerbId::Plan,
                || plan_command(providers, supervisor, cli, &project, task),
            )
        }
        Command::Release { action } => {
            let project = load(providers, cli, false)?;
            commands::release::execute(providers, supervisor, &project, cli, *action)
        }
        Command::Coverage => {
            let project = load(providers, cli, true)?;
            with_hooks(
                providers,
                supervisor,
                &cli.execution,
                &project,
                VerbId::Coverage,
                || commands::coverage::execute(providers, supervisor, &project, cli),
            )
        }
        Command::Explain { task } => {
            let project = load(providers, cli, false)?;
            let selection = commands::selection::TaskSelection::from(&cli.execution);
            commands::introspect::explain(
                providers,
                &project,
                intents_for(&project, task)?,
                &selection,
            )
        }
        Command::Affected { task } => {
            let project = load(providers, cli, false)?;
            commands::introspect::affected(
                providers,
                &project,
                intents_for(&project, task)?,
                &commands::selection::TaskSelection::from(&cli.execution),
                cli.execution.output,
            )
        }
        Command::Modules => {
            let project = load(providers, cli, false)?;
            commands::introspect::modules(providers, &project, cli.execution.output)
        }
        Command::Groups => {
            let project = load(providers, cli, false)?;
            commands::introspect::groups(providers, &project, cli.execution.output)
        }
        Command::Graph => {
            let project = load(providers, cli, false)?;
            commands::introspect::graph(
                providers,
                &project,
                cli.format.unwrap_or(GraphFormat::Text),
            )
        }
        Command::Tasks { name } => {
            let project = load(providers, cli, false)?;
            commands::tasks::tasks(providers, &project, name.as_deref(), cli.execution.output)
        }
        Command::Doctor { ensure, profile } => {
            let project = load(providers, cli, false)?;
            with_hooks(
                providers,
                supervisor,
                &cli.execution,
                &project,
                VerbId::Doctor,
                || {
                    let report = resolve_report(&cli.execution, &project);
                    commands::doctor::doctor(
                        providers,
                        &project,
                        report,
                        *ensure || cli.auto_install,
                        profile,
                    )
                },
            )
        }
        Command::CommitLint { message } => {
            commands::commit_lint::commit_lint(message.as_deref(), cli.execution.output)
        }
        Command::Completions { shell } => Ok(commands::completions::completions(*shell)),
        Command::Cache { action } => {
            let project = load(providers, cli, false)?;
            commands::cache::execute(&project, action)
        }
    }
}

/// Plan-only dispatch for `toven plan`: build the report and delegate to the
/// shared run pipeline with watch disabled and no live view.
fn plan_command(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    cli: &Cli,
    project: &Project,
    task: &str,
) -> AppResult<ExitCode> {
    let report = resolve_report(&cli.execution, project);
    commands::run::execute(
        providers,
        supervisor,
        project,
        report,
        intents_for(project, task)?,
        Vec::new(),
        cli.execution.fail_fast,
        cli.execution.no_cache,
        cli.execution.refresh,
        cli.execution.timeout,
        true,
        commands::run::WatchFlags {
            enabled: false,
            debounce_ms: flags::DEFAULT_WATCH_DEBOUNCE_MS,
        },
        None,
        cli.execution.jobs,
        cli.execution.compute_budget,
        &commands::selection::TaskSelection::from(&cli.execution),
    )
}

/// Print help for a bare task without running it. The `run` verb's help documents
/// every flag a bare task accepts.
fn task_help(task: &str) -> AppResult<ExitCode> {
    use std::io::Write;

    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{}", task_help_text(task)).map_err(AppError::internal)?;
    Ok(ExitCode::Success)
}

/// The help for a bare task: a short note plus `toven run --help`, rendered
/// from the built command so global options and the binary name are included.
fn task_help_text(task: &str) -> String {
    use clap::CommandFactory;

    let mut command = Cli::command();
    command.build();
    let help = command
        .find_subcommand_mut("run")
        .map(|run| run.render_long_help().to_string())
        .unwrap_or_default();
    format!(
        "`toven {task}` runs the `{task}` task, the same as `toven run {task}`.\n\
         Use `toven {task} -- --help` to pass `--help` to the task's tool.\n\n{help}"
    )
}

/// Dispatch a bare argv-first task: parse its flag prefix, merge it over the
/// pre-token globals, and run it exactly like `toven run <task>`.
fn dispatch_task(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    cli: &Cli,
    tokens: &[String],
) -> AppResult<ExitCode> {
    let invocation = grammar::parse_task(tokens)?;
    if invocation.help {
        return task_help(&invocation.task);
    }
    // The trailing flags arrive inside `Command::External`, so `gate` never saw
    // them: check the merged set before loading the project.
    let execution = cli.execution.clone().for_task(invocation.flags)?;
    let project = load_with_config(providers, execution.config.as_deref(), true)?;
    run_tasks(
        providers,
        supervisor,
        &execution,
        &project,
        &invocation.task,
        invocation.passthrough,
    )
    .map_err(|error| advise_builtin_typo(&invocation.task, error))
}

/// Run `task` with its run hooks: the shared body of `toven run <task>` and a
/// bare `toven <task>`.
fn run_tasks(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    execution: &ExecutionArgs,
    project: &Project,
    task: &str,
    passthrough: Vec<String>,
) -> AppResult<ExitCode> {
    with_hooks(
        providers,
        supervisor,
        execution,
        project,
        VerbId::Run,
        || {
            commands::run::execute(
                providers,
                supervisor,
                project,
                resolve_report(execution, project),
                intents_for(project, task)?,
                passthrough,
                execution.fail_fast,
                execution.no_cache,
                execution.refresh,
                execution.timeout,
                execution.is_plan_only(),
                watch_flags(execution),
                execution.view.map(Into::into),
                execution.jobs,
                execution.compute_budget,
                &commands::selection::TaskSelection::from(execution),
            )
        },
    )
}

/// When a bare-task dispatch fails because the token is not a resolvable task,
/// add an advisory hint if the token is a near-miss of a reserved built-in
/// verb.
///
/// argv stays sacred: a real task named `modual` still runs, so the hint is
/// only appended *after* the token failed to resolve as a task, and never
/// rewrites the invocation — it is advisory text on the already-typed error.
///
/// The predicate matches only the planner's typed missing-task error for the
/// attempted token (its `missing_task` detail, the canonical intent name), so
/// unrelated `InvalidInput` failures never pick up the hint.
fn advise_builtin_typo(task: &str, error: AppError) -> AppError {
    let wanted = intent_for(task);
    let missing = error
        .details()
        .get("missing_task")
        .and_then(serde_json::Value::as_str);
    if error.code() != rskit_errors::ErrorCode::InvalidInput || missing != Some(wanted.name()) {
        return error;
    }
    let Some(reserved) = grammar::nearest_reserved(task) else {
        return error;
    };
    error.hint(format!(
        "If you meant the built-in, run 'toven {reserved}'."
    ))
}

/// The watch settings an execution verb runs with.
fn watch_flags(execution: &ExecutionArgs) -> commands::run::WatchFlags {
    commands::run::WatchFlags {
        enabled: execution.watch,
        debounce_ms: execution
            .watch_debounce_ms
            .unwrap_or(flags::DEFAULT_WATCH_DEBOUNCE_MS),
    }
}

/// Load the project using the verb's `--config` global flag.
fn load(
    providers: &[&dyn Provider],
    cli: &Cli,
    warn_task_dispatch_collisions: bool,
) -> AppResult<Project> {
    load_with_config(
        providers,
        cli.execution.config.as_deref(),
        warn_task_dispatch_collisions,
    )
}

/// Load the project at the resolved config path and optionally emit
/// task-dispatch collision warnings.
fn load_with_config(
    providers: &[&dyn Provider],
    config: Option<&std::path::Path>,
    warn_task_dispatch_collisions: bool,
) -> AppResult<Project> {
    let config_path = host::discover_config(config)?;
    let project = host::load_project(&config_path, providers)?;
    let sets = project.document.sets.keys().map(String::as_str);
    collision::reject_set_clashes(sets, [])?;
    if warn_task_dispatch_collisions {
        warn_collisions(&project, providers)?;
    }
    Ok(project)
}

/// Emit a stderr warning for every argv-first task name that shadows a reserved
/// verb, and reject a set named like an addressable task.
///
/// Only argv-first task dispatch surfaces (`toven <task>`, `toven run <task>`,
/// and `toven plan <task>`) need this warning. Introspection/cache verbs do not
/// route ambiguous task names and stay on the cheaper load path.
///
/// Best-effort: a configuration that cannot even enumerate its task names will
/// surface that same error from the verb's own PLAN call with full context, so
/// the warning pass stays silent rather than pre-empting it.
fn warn_collisions(project: &Project, providers: &[&dyn Provider]) -> AppResult<()> {
    let Ok(names) = addressable_task_names(&project.document, providers) else {
        return Ok(());
    };
    for collision in collision::detect(names.iter().map(String::as_str)) {
        eprintln!("{}", collision.message());
    }
    collision::reject_set_clashes(
        project.document.sets.keys().map(String::as_str),
        names.iter().map(String::as_str),
    )
}

/// Expand a run token — one task, a comma list (`build,test`), or a
/// configured `[sets.<name>]` — into the intents one plan runs together.
fn intents_for(project: &Project, token: &str) -> AppResult<Vec<TaskIntent>> {
    Ok(project
        .document
        .task_list(token)?
        .iter()
        .map(|task| intent_for(task))
        .collect())
}

/// Resolve a task token to its [`TaskIntent`] (name + recognized kind).
fn intent_for(task: &str) -> TaskIntent {
    TaskIntent::resolve(task)
}

#[cfg(test)]
mod tests {
    #[test]
    fn task_help_lists_the_run_options() {
        let help = super::task_help_text("tidy");
        assert!(help.contains("`toven tidy` runs the `tidy` task"), "{help}");
        assert!(help.contains("Usage: toven run [OPTIONS] <TASK>"), "{help}");
        for flag in [
            "--dry-run",
            "--module <SELECTOR>",
            "--workspace <SELECTOR>",
            "--compute-budget <BUDGET>",
        ] {
            assert!(help.contains(flag), "missing {flag}: {help}");
        }
    }

    use super::{INIT_SUBCOMMAND, SERVE_SUBCOMMAND, is_init_invocation, is_serve_invocation};
    use std::ffi::OsString;

    fn argv(tokens: &[&str]) -> Vec<OsString> {
        tokens.iter().map(OsString::from).collect()
    }

    #[test]
    fn bare_serve_token_selects_the_server_loop() {
        assert!(is_serve_invocation(&argv(&["toven-go", SERVE_SUBCOMMAND])));
    }

    #[test]
    fn serve_with_trailing_arguments_falls_through_to_clap() {
        // `__serve --help` (or any extra token) must NOT start the loop: it should fall
        // through to clap and fail fast rather than blocking on stdin.
        assert!(!is_serve_invocation(&argv(&[
            "toven-go",
            SERVE_SUBCOMMAND,
            "--help"
        ])));
        assert!(!is_serve_invocation(&argv(&[
            "toven-go",
            SERVE_SUBCOMMAND,
            "extra"
        ])));
    }

    #[test]
    fn serve_not_in_first_position_is_not_a_server_loop() {
        assert!(!is_serve_invocation(&argv(&["toven-go", "plan"])));
        assert!(!is_serve_invocation(&argv(&[
            "toven-go",
            "run",
            SERVE_SUBCOMMAND
        ])));
    }

    #[test]
    fn empty_or_program_only_argv_is_not_a_server_loop() {
        assert!(!is_serve_invocation(&argv(&[])));
        assert!(!is_serve_invocation(&argv(&["toven-go"])));
    }

    #[test]
    fn bare_init_token_selects_the_wizard_exchange() {
        assert!(is_init_invocation(&argv(&["toven-go", INIT_SUBCOMMAND])));
    }

    #[test]
    fn init_with_trailing_arguments_falls_through_to_clap() {
        assert!(!is_init_invocation(&argv(&[
            "toven-go",
            INIT_SUBCOMMAND,
            "--help"
        ])));
        assert!(!is_init_invocation(&argv(&[
            "toven-go",
            INIT_SUBCOMMAND,
            "extra"
        ])));
    }

    #[test]
    fn init_and_serve_tokens_do_not_cross_match() {
        assert!(!is_init_invocation(&argv(&["toven-go", SERVE_SUBCOMMAND])));
        assert!(!is_serve_invocation(&argv(&["toven-go", INIT_SUBCOMMAND])));
    }

    #[test]
    fn empty_or_program_only_argv_is_not_a_wizard_exchange() {
        assert!(!is_init_invocation(&argv(&[])));
        assert!(!is_init_invocation(&argv(&["toven-go"])));
    }

    #[test]
    fn exit_code_taxonomy_matches_the_documented_contract() {
        use rskit_cli::ExitCode;
        use rskit_errors::ErrorCode;

        // Pins the exit-code contract published in docs/commands/README.md so a change
        // to the numeric values (or the error-class mapping the CLI relies on) fails
        // here — automation authors branch on these numbers.
        assert_eq!(ExitCode::Success.as_i32(), 0);
        assert_eq!(ExitCode::Failure.as_i32(), 1);
        assert_eq!(ExitCode::Usage.as_i32(), 2);
        assert_eq!(ExitCode::Permission.as_i32(), 3);
        assert_eq!(ExitCode::NotFound.as_i32(), 4);
        assert_eq!(ExitCode::Conflict.as_i32(), 5);
        assert_eq!(ExitCode::Unavailable.as_i32(), 69);
        assert_eq!(ExitCode::RateLimited.as_i32(), 75);
        assert_eq!(ExitCode::Timeout.as_i32(), 124);
        assert_eq!(ExitCode::Cancelled.as_i32(), 130);

        // The error classes the CLI produces map to the documented codes.
        assert_eq!(ExitCode::from(ErrorCode::InvalidInput).as_i32(), 2);
        assert_eq!(ExitCode::from(ErrorCode::NotFound).as_i32(), 4);
        assert_eq!(ExitCode::from(ErrorCode::Conflict).as_i32(), 5);
    }
}
