//! The public CLI entry point: parse argv, gate flag applicability, load the
//! project, warn on task/reserved collisions for argv-first task dispatch,
//! dispatch the verb, and map any typed error to a process [`ExitCode`].
//!
//! This is the one place that ties the argv-first grammar to the engine spine.
//! The three apps (`apps/{toven, toven-rs, toven-go}`) are thin: each builds
//! its provider set and calls [`run`]. Everything user-facing is printed by the
//! reporter sinks or the verb projections; this module prints only the final
//! rendered error.

use clap::Parser;
use clap::error::ErrorKind;
use rskit_cli::{ErrorRenderer, ExitCode};
use rskit_errors::{AppError, AppResult};
use toven_engine::plan::addressable_task_names;
use toven_engine::vcs::BaselineFlags;
use toven_ports::{Provider, TaskIntent};

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

    match dispatch(providers, &cli) {
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
fn resolve_report(cli: &Cli, project: &Project) -> Report {
    Report::resolve(
        cli.output,
        cli.verbosity(),
        cli.color_choice(),
        &project.document,
    )
}

fn dispatch(providers: &[&dyn Provider], cli: &Cli) -> AppResult<ExitCode> {
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
        Command::External(tokens) => dispatch_task(providers, cli, tokens),
        Command::Run { task, passthrough } => {
            let project = load(providers, cli, true)?;
            let report = resolve_report(cli, &project);
            commands::run::execute(
                providers,
                &project,
                report,
                intent_for(task),
                passthrough.clone(),
                cli.fail_fast,
                cli.no_cache,
                cli.refresh,
                cli.timeout,
                cli.is_plan_only(),
                global_watch(cli),
                cli.view.map(Into::into),
                cli.jobs,
                &global_selection(cli),
            )
        }
        Command::Plan { task } => {
            let project = load(providers, cli, true)?;
            plan_command(providers, cli, &project, task)
        }
        Command::Release { action } => {
            let project = load(providers, cli, false)?;
            commands::release::execute(providers, &project, cli, *action)
        }
        Command::Coverage => {
            let project = load(providers, cli, true)?;
            commands::coverage::execute(providers, &project, cli)
        }
        Command::Explain { task } => {
            let project = load(providers, cli, false)?;
            let selection = global_selection(cli);
            commands::introspect::explain(providers, &project, intent_for(task), &selection)
        }
        Command::Affected { task } => {
            let project = load(providers, cli, false)?;
            commands::introspect::affected(
                providers,
                &project,
                intent_for(task),
                &global_selection(cli),
            )
        }
        Command::Modules => {
            let project = load(providers, cli, false)?;
            commands::introspect::modules(providers, &project, cli.output)
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
            commands::tasks::tasks(providers, &project, name.as_deref(), cli.output)
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
    cli: &Cli,
    project: &Project,
    task: &str,
) -> AppResult<ExitCode> {
    let report = resolve_report(cli, project);
    commands::run::execute(
        providers,
        project,
        report,
        intent_for(task),
        Vec::new(),
        cli.fail_fast,
        cli.no_cache,
        cli.refresh,
        cli.timeout,
        true,
        commands::run::WatchFlags {
            enabled: false,
            debounce_ms: flags::DEFAULT_WATCH_DEBOUNCE_MS,
        },
        None,
        cli.jobs,
        &global_selection(cli),
    )
}

/// Dispatch a bare argv-first task: re-parse its trailing flags + passthrough,
/// then merge them with the pre-token global flags and run.
fn dispatch_task(providers: &[&dyn Provider], cli: &Cli, tokens: &[String]) -> AppResult<ExitCode> {
    let invocation = grammar::parse_task(tokens)?;
    let flags = &invocation.flags;

    // Trailing task flags land in `Command::External` tokens, so the pre-token
    // `gate` never sees them — enforce the APPLY-execution flag combination
    // invariants here on the merged global+task values, before the project load.
    let watch_enabled = cli.watch || flags.watch;
    let plan_only = cli.is_plan_only() || flags.dry_run || flags.explain;
    let debounce_present = flags.watch_debounce_ms.is_some() || cli.watch_debounce_ms.is_some();
    flags::gate_apply_flag_combination(
        watch_enabled,
        cli.fail_fast || flags.fail_fast,
        flags.timeout.or(cli.timeout).is_some(),
        plan_only,
        debounce_present,
    )?;

    let config = flags.config.clone().or_else(|| cli.config.clone());
    let project = load_with_config(providers, config.as_deref(), true)?;

    let output = flags.output.or(cli.output);
    let verbosity = flags::Verbosity::for_execution(
        cli.verbose.saturating_add(flags.verbose),
        cli.quiet.saturating_add(flags.quiet),
        cli.explain || flags.explain,
    );
    let color = flags.color.or(cli.color).unwrap_or_default();
    let report = Report::resolve(output, verbosity, color, &project.document);
    let fail_fast = cli.fail_fast || flags.fail_fast;
    let no_cache = cli.no_cache || flags.no_cache;
    let refresh = cli.refresh || flags.refresh;
    // Trailing task flags bypass the pre-token `gate`, so enforce the same
    // `--refresh`/`--no-cache` contradiction here on the merged values.
    if refresh && no_cache {
        return Err(flags::refresh_no_cache_conflict());
    }
    let unit_timeout = flags.timeout.or(cli.timeout);

    let mut baseline = BaselineFlags::new().with_merge_base(cli.merge_base || flags.merge_base);
    if let Some(reference) = flags.base.clone().or_else(|| cli.base.clone()) {
        baseline = baseline.with_base(reference);
    }

    let mut modules = cli.module.clone();
    modules.extend(flags.modules.iter().cloned());
    let mut workspaces = cli.workspace.clone();
    workspaces.extend(flags.workspaces.iter().cloned());
    let selection = commands::selection::TaskSelection {
        baseline,
        modules,
        workspaces,
        with_dependents: cli.with_dependents || flags.with_dependents,
        with_dependencies: cli.with_dependencies || flags.with_dependencies,
    };

    let watch = commands::run::WatchFlags {
        enabled: watch_enabled,
        debounce_ms: flags
            .watch_debounce_ms
            .or(cli.watch_debounce_ms)
            .unwrap_or(flags::DEFAULT_WATCH_DEBOUNCE_MS),
    };
    let view = flags.view.or(cli.view).map(Into::into);
    let jobs = flags.jobs.or(cli.jobs);

    commands::run::execute(
        providers,
        &project,
        report,
        intent_for(&invocation.task),
        invocation.passthrough,
        fail_fast,
        no_cache,
        refresh,
        unit_timeout,
        plan_only,
        watch,
        view,
        jobs,
        &selection,
    )
    .map_err(|error| advise_builtin_typo(&invocation.task, error))
}

/// When a bare-task dispatch fails because the token is not a resolvable task,
/// add an advisory hint if the token is a near-miss of a reserved built-in
/// verb.
///
/// argv stays sacred: a real task named `modual` still runs, so the hint is
/// only appended *after* the token failed to resolve as a task, and never
/// rewrites the invocation — it is advisory text on the already-typed error.
///
/// The predicate matches only the specific missing-task message for the
/// attempted token (`has no '<name>' task`, the canonical intent name the
/// scheduler emits), so unrelated `InvalidInput` failures whose text happens to
/// contain "has no" (for example a config "... has no parent ..." error) never
/// pick up the hint.
fn advise_builtin_typo(task: &str, error: AppError) -> AppError {
    let missing_task = format!("has no '{}' task", intent_for(task).name());
    if error.code() != rskit_errors::ErrorCode::InvalidInput
        || !error.message().contains(&missing_task)
    {
        return error;
    }
    let Some(reserved) = grammar::nearest_reserved(task) else {
        return error;
    };
    error.hint(format!(
        "If you meant the built-in, run 'toven {reserved}'."
    ))
}

/// Bundle the pre-token global watch flags for the reserved execution verbs.
fn global_watch(cli: &Cli) -> commands::run::WatchFlags {
    commands::run::WatchFlags {
        enabled: cli.watch,
        debounce_ms: cli
            .watch_debounce_ms
            .unwrap_or(flags::DEFAULT_WATCH_DEBOUNCE_MS),
    }
}

/// Bundle the pre-token global selection flags for the reserved execution
/// verbs.
fn global_selection(cli: &Cli) -> commands::selection::TaskSelection {
    commands::selection::TaskSelection {
        baseline: cli.baseline_flags(),
        modules: cli.module.clone(),
        workspaces: cli.workspace.clone(),
        with_dependents: cli.with_dependents,
        with_dependencies: cli.with_dependencies,
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
        cli.config.as_deref(),
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
    if warn_task_dispatch_collisions {
        warn_collisions(&project, providers);
    }
    Ok(project)
}

/// Emit a stderr warning for every argv-first task name that shadows a reserved
/// verb.
///
/// Only argv-first task dispatch surfaces (`toven <task>`, `toven run <task>`,
/// and `toven plan <task>`) need this warning. Introspection/cache verbs do not
/// route ambiguous task names and stay on the cheaper load path.
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

/// Resolve a task token to its [`TaskIntent`] (name + recognized kind).
fn intent_for(task: &str) -> TaskIntent {
    TaskIntent::resolve(task)
}

#[cfg(test)]
mod tests {
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
}
