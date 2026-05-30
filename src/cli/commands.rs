//! Clap command definitions.

use clap::{Arg, ArgAction, Command};

/// Subcommands reserved by the CLI entrypoint.
pub(super) const RESERVED_SUBCOMMANDS: &[&str] = &[
    "help", "run", "plan", "affected", "explain", "modules", "list", "ls", "graph", "deps", "cache",
];

/// Build the top-level Toven command.
#[must_use]
pub(super) fn command() -> Command {
    Command::new("toven")
        .about("Fast, argv-first development and CI task planning")
        .version(crate::VERSION)
        .subcommand_precedence_over_arg(true)
        .subcommand(run_subcommand())
        .subcommand(plan_command())
        .subcommand(affected_command())
        .subcommand(explain_command())
        .subcommand(modules_command())
        .subcommand(list_command())
        .subcommand(graph_command())
        .subcommand(cache_command())
}

/// Build the root task invocation parser.
#[must_use]
pub(super) fn run_command() -> Command {
    run_args(Command::new("toven"))
}

fn run_subcommand() -> Command {
    run_args(Command::new("run").about("Execute a task, including names reserved by subcommands"))
}

fn run_args(command: Command) -> Command {
    command
        .arg(required_positional("task", "TASK", "Task name to execute"))
        .arg(config_arg())
        .args(affected_args(
            "Execute only modules affected by the selected git baseline",
        ))
        .args(cache_mode_args(
            "Disable cache reads and writes for execution",
            "Skip cache reads but write successful execution records",
        ))
        .arg(
            Arg::new("timeout-seconds")
                .long("timeout-seconds")
                .value_name("SECONDS")
                .value_parser(clap::value_parser!(u64))
                .help("Optional process timeout in seconds"),
        )
        .arg(
            Arg::new("output")
                .long("output")
                .value_name("FORMAT")
                .default_value("human")
                .value_parser(["human", "jsonl"])
                .help("Output format for run events"),
        )
        .arg(
            Arg::new("watch")
                .long("watch")
                .action(ArgAction::SetTrue)
                .help("Watch the workspace and rerun affected modules after file changes"),
        )
        .arg(
            Arg::new("watch-debounce-ms")
                .long("watch-debounce-ms")
                .value_name("MILLIS")
                .default_value("250")
                .value_parser(clap::value_parser!(u64))
                .help("Debounce interval for --watch file changes"),
        )
        .arg(
            Arg::new("watch-once")
                .long("watch-once")
                .hide(true)
                .action(ArgAction::SetTrue),
        )
        .arg(passthrough_args())
}

fn plan_command() -> Command {
    Command::new("plan")
        .about("Render a reviewable task execution plan")
        .arg(config_arg())
        .arg(defaulted_task_arg("Task name to plan"))
        .arg(passthrough_args())
        .args(affected_args(
            "Plan only modules affected by the selected git baseline",
        ))
}

fn affected_command() -> Command {
    Command::new("affected")
        .about("Show modules affected by a git baseline")
        .arg(config_arg())
        .arg(defaulted_task_arg(
            "Task name used to select profiles/modules",
        ))
        .args(baseline_args())
}

fn modules_command() -> Command {
    Command::new("modules")
        .about("List discovered modules")
        .arg(config_arg())
        .arg(defaulted_task_arg(
            "Task name used to select profiles/modules",
        ))
}

fn list_command() -> Command {
    Command::new("list")
        .about("Alias for modules")
        .alias("ls")
        .arg(config_arg())
        .arg(defaulted_task_arg(
            "Task name used to select profiles/modules",
        ))
}

fn graph_command() -> Command {
    Command::new("graph")
        .visible_alias("deps")
        .about("Render the discovered module dependency graph")
        .arg(config_arg())
        .arg(defaulted_task_arg(
            "Task name used to select profiles/modules",
        ))
        .arg(
            Arg::new("format")
                .long("format")
                .value_name("FORMAT")
                .default_value("text")
                .value_parser(["text", "dot"])
                .help("Graph output format"),
        )
}

fn cache_command() -> Command {
    Command::new("cache")
        .about("Inspect and clean the local Toven cache")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(cache_stats_command())
        .subcommand(cache_clean_command())
}

fn cache_stats_command() -> Command {
    Command::new("stats")
        .visible_alias("info")
        .about("Show local cache size and entry count")
        .arg(config_arg())
}

fn cache_clean_command() -> Command {
    Command::new("clean")
        .visible_alias("clear")
        .about("Remove local cache records")
        .arg(config_arg())
}

fn explain_command() -> Command {
    Command::new("explain")
        .about("Explain affected and cache reasoning for a module task")
        .arg(required_positional(
            "module",
            "MODULE",
            "Module name to explain",
        ))
        .arg(required_positional("task", "TASK", "Task name to explain"))
        .arg(config_arg())
        .args(baseline_args())
        .args(cache_mode_args(
            "Explain with cache disabled",
            "Explain with cache reads skipped",
        ))
}

fn required_positional(id: &'static str, value_name: &'static str, help: &'static str) -> Arg {
    Arg::new(id)
        .value_name(value_name)
        .required(true)
        .help(help)
}

fn config_arg() -> Arg {
    Arg::new("config")
        .long("config")
        .value_name("PATH")
        .default_value("toven.toml")
        .help("Path to the Toven config file")
}

fn defaulted_task_arg(help: &'static str) -> Arg {
    Arg::new("task")
        .long("task")
        .value_name("NAME")
        .default_value("test")
        .help(help)
}

fn passthrough_args() -> Arg {
    Arg::new("args")
        .num_args(0..)
        .trailing_var_arg(true)
        .allow_hyphen_values(true)
        .help("Arguments passed through to {args}")
}

fn affected_args(affected_help: &'static str) -> [Arg; 3] {
    [
        Arg::new("affected")
            .long("affected")
            .action(ArgAction::SetTrue)
            .help(affected_help),
        base_arg(),
        merge_base_arg(),
    ]
}

fn baseline_args() -> [Arg; 2] {
    [base_arg(), merge_base_arg()]
}

fn base_arg() -> Arg {
    Arg::new("base")
        .long("base")
        .value_name("REF")
        .help("Explicit baseline ref or SHA for affected detection")
}

fn merge_base_arg() -> Arg {
    Arg::new("merge-base")
        .long("merge-base")
        .action(ArgAction::SetTrue)
        .help("Use the merge-base of HEAD and the selected baseline ref")
}

fn cache_mode_args(no_cache_help: &'static str, force_help: &'static str) -> [Arg; 2] {
    [
        Arg::new("no-cache")
            .long("no-cache")
            .action(ArgAction::SetTrue)
            .conflicts_with("force")
            .help(no_cache_help),
        Arg::new("force")
            .long("force")
            .action(ArgAction::SetTrue)
            .conflicts_with("no-cache")
            .help(force_help),
    ]
}
