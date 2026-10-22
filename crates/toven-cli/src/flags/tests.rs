//! Clap surface and gate tests.

use super::{Cli, Command, ReleaseAction};
use clap::Parser;

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("toven").chain(args.iter().copied()))
}

#[test]
fn bare_task_is_captured_as_external() {
    let cli = parse(&["test"]).expect("parses");
    match &cli.command {
        Command::External(tokens) => assert_eq!(tokens, &["test"]),
        other => panic!("expected external, got {other:?}"),
    }
}

#[test]
fn bare_task_trailing_flags_are_captured_as_external_tokens() {
    let cli = parse(&["test", "--dry-run", "--", "--flag"]).expect("parses");
    assert!(!cli.execution.dry_run);
    match &cli.command {
        Command::External(tokens) => {
            assert_eq!(tokens, &["test", "--dry-run", "--", "--flag"]);
        }
        other => panic!("expected external, got {other:?}"),
    }
}

#[test]
fn reserved_verb_dispatches_builtin() {
    let cli = parse(&["plan", "test"]).expect("parses");
    assert!(matches!(cli.command, Command::Plan { .. }));
}

#[test]
fn explain_takes_a_task_positional_and_reuses_the_module_flag() {
    let cli = parse(&["explain", "build", "--module", "rust:app"]).expect("parses");
    match &cli.command {
        Command::Explain { task } => assert_eq!(task, "build"),
        other => panic!("expected explain, got {other:?}"),
    }
    assert_eq!(cli.execution.module, vec!["rust:app".to_string()]);
}

#[test]
fn modules_aliases_resolve() {
    for alias in ["modules", "list", "ls"] {
        let cli = parse(&[alias]).expect("parses");
        assert!(matches!(cli.command, Command::Modules), "alias {alias}");
    }
}

#[test]
fn graph_deps_alias_resolves() {
    assert!(matches!(parse(&["deps"]).unwrap().command, Command::Graph));
}

#[test]
fn color_flag_defaults_to_none_and_parses_each_choice() {
    use super::ColorWhen;
    // Unset means "no explicit policy"; `color_choice()` defaults it to auto.
    assert_eq!(parse(&["run", "test"]).unwrap().execution.color, None);
    assert_eq!(
        parse(&["run", "test"]).unwrap().execution.color_choice(),
        ColorWhen::Auto
    );
    assert_eq!(
        parse(&["--color", "always", "run", "test"])
            .unwrap()
            .execution
            .color,
        Some(ColorWhen::Always)
    );
    assert_eq!(
        parse(&["--color", "never", "run", "test"])
            .unwrap()
            .execution
            .color,
        Some(ColorWhen::Never)
    );
    // An unknown policy is a clap parse error, never a silent fallback.
    assert!(parse(&["--color", "sometimes", "run", "test"]).is_err());
}

#[test]
fn color_accepted_on_every_verb_like_git_and_cargo() {
    // `--color` is global and harmless where no human reporter is built (unlike
    // `-v`/`-q`), so it must be accepted on every verb rather than failing a
    // `--color never` CI wrapper on some of them.
    for args in [
        ["--color", "always", "modules"].as_slice(),
        ["--color", "never", "graph"].as_slice(),
        ["--color", "auto", "cache", "path"].as_slice(),
        ["--color", "always", "tasks"].as_slice(),
    ] {
        let cli = parse(args).expect("parses");
        assert!(super::gate(&cli).is_ok(), "{args:?}");
    }
}

#[test]
fn color_accepted_on_execution_verbs() {
    for args in [
        ["--color", "always", "run", "test"].as_slice(),
        ["--color", "never", "plan", "test"].as_slice(),
        ["--color", "auto", "release", "publish"].as_slice(),
        ["--color", "always", "test"].as_slice(),
    ] {
        let cli = parse(args).expect("parses");
        assert!(super::gate(&cli).is_ok(), "{args:?}");
    }
}

#[test]
fn color_choice_maps_onto_the_rskit_policy() {
    use super::ColorWhen;
    use rskit_cli::ColorChoice;
    assert!(matches!(
        ColorChoice::from(ColorWhen::Auto),
        ColorChoice::Auto
    ));
    assert!(matches!(
        ColorChoice::from(ColorWhen::Always),
        ColorChoice::Always
    ));
    assert!(matches!(
        ColorChoice::from(ColorWhen::Never),
        ColorChoice::Never
    ));
}

#[test]
fn help_carries_worked_examples_at_the_top_level_and_per_verb() {
    use clap::CommandFactory;
    let mut command = Cli::command();
    let top = command.render_long_help().to_string();
    assert!(
        top.contains("Examples:") && top.contains("toven tasks"),
        "top-level help is missing its examples block: {top}"
    );
    // The top-level list advertises the coverage and release-lifecycle verbs
    // alongside the task workflows.
    assert!(
        top.contains("toven coverage") && top.contains("toven release"),
        "top-level examples omit the coverage/release surface: {top}"
    );
    let tasks = command
        .get_subcommands_mut()
        .find(|sub| sub.get_name() == "tasks")
        .expect("tasks subcommand")
        .render_long_help()
        .to_string();
    assert!(
        tasks.contains("Examples:") && tasks.contains("argv template"),
        "tasks help is missing its examples block: {tasks}"
    );
}

#[test]
fn every_verb_help_carries_an_examples_block() {
    use clap::CommandFactory;
    let mut command = Cli::command();
    // Every reserved verb the user can invoke reads coherently: its long help ends
    // in a worked `Examples:` block. `help` is clap's built-in and `external` is
    // the argv-first task escape hatch (no clap help of its own).
    for sub in command.get_subcommands_mut() {
        let name = sub.get_name().to_string();
        if name == "help" {
            continue;
        }
        let help = sub.render_long_help().to_string();
        assert!(
            help.contains("Examples:"),
            "`toven {name} --help` is missing its examples block: {help}"
        );
    }
}

#[test]
fn every_release_action_help_carries_an_examples_block() {
    use clap::CommandFactory;
    let mut command = Cli::command();
    let release = command
        .get_subcommands_mut()
        .find(|sub| sub.get_name() == "release")
        .expect("release subcommand");
    for action in [
        "plan",
        "status",
        "tag",
        "publish",
        "readiness",
        "sbom",
        "depgraphs",
    ] {
        let help = release
            .get_subcommands_mut()
            .find(|sub| sub.get_name() == action)
            .unwrap_or_else(|| panic!("release {action} action"))
            .render_long_help()
            .to_string();
        assert!(
            help.contains("Examples:"),
            "`toven release {action} --help` is missing its examples block: {help}"
        );
    }
}

#[test]
fn release_publish_help_describes_ecosystem_targets() {
    use clap::CommandFactory;
    let mut command = Cli::command();
    let release = command
        .get_subcommands_mut()
        .find(|sub| sub.get_name() == "release")
        .expect("release subcommand");
    let help = release
        .get_subcommands_mut()
        .find(|sub| sub.get_name() == "publish")
        .expect("release publish action")
        .render_long_help()
        .to_string();

    assert!(help.contains("configured ecosystem target"), "{help}");
    assert!(!help.contains("through the registry"), "{help}");
}

#[test]
fn unknown_subcommand_action_suggests_the_nearest_action() {
    // argv stays sacred: an unrecognized lifecycle action is never rewritten, but
    // clap surfaces the nearest real action so the typo is actionable.
    for (args, suggestion) in [
        (["release", "publsh"].as_slice(), "publish"),
        (["cache", "statss"].as_slice(), "stats"),
        (["driver", "instal"].as_slice(), "install"),
        (["federation", "statuss"].as_slice(), "status"),
    ] {
        let error = parse(args).expect_err("unknown action must fail to parse");
        let rendered = error.to_string();
        assert!(
            rendered.contains(suggestion),
            "unknown action `{args:?}` should suggest `{suggestion}`, got: {rendered}"
        );
    }
}

#[test]
fn global_flag_accepted_before_verb() {
    let cli = parse(&["--dry-run", "plan", "test"]).expect("parses");
    assert!(cli.execution.dry_run);
}

#[test]
fn a_valueless_level_flag_is_repo_wide_at_either_placement() {
    // The valueless form takes the missing-value default (`*`, repo-wide)
    // without consuming the next token, so global placement before the
    // subcommand keeps `release` as the verb rather than swallowing it as
    // the module value.
    let before = parse(&["--minor", "release", "bump"]).expect("parses");
    assert_eq!(before.minor, ["*"]);
    let after = parse(&["release", "bump", "--minor"]).expect("parses");
    assert_eq!(after.minor, ["*"]);
}

#[test]
fn a_valued_level_flag_requires_equals_at_either_placement() {
    let before = parse(&["--minor=rust:core", "release", "bump"]).expect("parses");
    assert_eq!(before.minor, ["rust:core"]);
    let after = parse(&["release", "bump", "--minor=rust:core"]).expect("parses");
    assert_eq!(after.minor, ["rust:core"]);
    // The space-separated form is never read as a valued level flag:
    // `--minor` keeps its repo-wide default and the bare token parses as a
    // task, which the gate then rejects as a misplaced bump flag rather
    // than silently planning the wrong thing.
    let spaced = parse(&["--minor", "rust:core", "release", "bump"]).expect("parses");
    assert!(matches!(spaced.command, Command::External(_)));
    assert!(super::gate(&spaced).is_err());
}

#[test]
fn release_only_flag_on_other_reserved_verb_is_gated() {
    let cli = parse(&["--no-push", "plan", "test"]).expect("parses");
    assert!(super::gate(&cli).is_err());
}

#[test]
fn allow_dirty_is_no_longer_a_flag() {
    // The clean-tree guardrail has no bypass; `--allow-dirty` was removed and is
    // now an unknown flag on the mutating release actions.
    assert!(parse(&["--allow-dirty", "release", "publish"]).is_err());
}

#[test]
fn release_accepts_its_own_flags() {
    let cli = parse(&["--no-push", "release", "publish"]).expect("parses");
    assert!(super::gate(&cli).is_ok());
}

#[test]
fn release_requires_a_lifecycle_action() {
    assert!(parse(&["release"]).is_err());
}

#[test]
fn release_actions_parse_to_their_variants() {
    for (arg, want) in [
        ("plan", ReleaseAction::Plan),
        ("status", ReleaseAction::Status),
        ("tag", ReleaseAction::Tag),
        ("bump", ReleaseAction::Bump),
        ("publish", ReleaseAction::Publish),
        ("readiness", ReleaseAction::Readiness),
        ("sbom", ReleaseAction::Sbom),
        ("depgraphs", ReleaseAction::Depgraphs),
        ("package", ReleaseAction::Package),
        ("checksums", ReleaseAction::Checksums),
        ("sign", ReleaseAction::Sign),
        ("verify", ReleaseAction::Verify),
        ("image", ReleaseAction::Image),
        ("provenance", ReleaseAction::Provenance),
    ] {
        let cli = parse(&["release", arg]).expect("parses");
        match cli.command {
            Command::Release { action } => {
                assert_eq!(action.as_str(), want.as_str(), "{arg}");
            }
            other => panic!("expected release, got {other:?}"),
        }
    }
}

#[test]
fn release_projections_are_plan_only() {
    assert!(parse(&["release", "plan"]).unwrap().is_plan_only());
    assert!(parse(&["release", "status"]).unwrap().is_plan_only());
    assert!(parse(&["release", "readiness"]).unwrap().is_plan_only());
    assert!(parse(&["release", "sbom"]).unwrap().is_plan_only());
    assert!(parse(&["release", "depgraphs"]).unwrap().is_plan_only());
    assert!(parse(&["release", "package"]).unwrap().is_plan_only());
    assert!(parse(&["release", "checksums"]).unwrap().is_plan_only());
    assert!(parse(&["release", "sign"]).unwrap().is_plan_only());
    assert!(parse(&["release", "verify"]).unwrap().is_plan_only());
    assert!(!parse(&["release", "publish"]).unwrap().is_plan_only());
}

#[test]
fn target_and_binary_only_apply_to_release_package() {
    let ok = parse(&[
        "--target",
        "x86_64-unknown-linux-gnu",
        "--binary",
        "target/release/toven",
        "release",
        "package",
    ])
    .expect("parses");
    assert!(super::gate(&ok).is_ok());

    for action in ["plan", "sbom", "tag"] {
        let cli =
            parse(&["--target", "x86_64-unknown-linux-gnu", "release", action]).expect("parses");
        assert!(super::gate(&cli).is_err(), "--target on {action}");
    }
    let on_task = parse(&["--binary", "x", "plan", "build"]).expect("parses");
    assert!(super::gate(&on_task).is_err(), "--binary on a task verb");
}

#[test]
fn download_and_no_run_only_apply_to_release_verify() {
    let ok = parse(&["--download", "--no-run", "release", "verify"]).expect("parses");
    assert!(super::gate(&ok).is_ok());

    for action in ["plan", "package", "tag"] {
        let downloaded = parse(&["--download", "release", action]).expect("parses");
        assert!(super::gate(&downloaded).is_err(), "--download on {action}");
        let no_run = parse(&["--no-run", "release", action]).expect("parses");
        assert!(super::gate(&no_run).is_err(), "--no-run on {action}");
    }
    let on_task = parse(&["--no-run", "plan", "build"]).expect("parses");
    assert!(super::gate(&on_task).is_err(), "--no-run on a task verb");
}

#[test]
fn no_push_only_on_mutating_release_actions() {
    for action in ["tag", "publish"] {
        let cli = parse(&["--no-push", "release", action]).expect("parses");
        assert!(super::gate(&cli).is_ok(), "{action}");
    }

    for action in [
        "plan",
        "status",
        "readiness",
        "sbom",
        "depgraphs",
        "package",
        "bump",
    ] {
        let no_push = parse(&["--no-push", "release", action]).expect("parses");
        assert!(super::gate(&no_push).is_err(), "no-push {action}");
    }
}

#[test]
fn no_commit_flag_is_removed() {
    // `release bump` is write-only by default (it stages, never commits), so
    // the `--no-commit` flag was removed entirely — clap must reject it.
    assert!(
        parse(&["--no-commit", "--yes", "release", "bump"]).is_err(),
        "--no-commit is no longer a recognized flag"
    );
}

#[test]
fn yes_confirmation_only_applies_to_mutating_release_actions() {
    for action in ["bump", "tag", "publish", "image"] {
        let cli = parse(&["--yes", "release", action]).expect("parses");
        assert!(cli.confirm_release);
        // `--yes` is release-only now, not an alias of init's `--non-interactive`.
        assert!(!cli.non_interactive);
        assert!(super::gate(&cli).is_ok(), "{action}");
    }
    // `provenance` is read-only verification, so `--yes` does not apply.
    for action in [
        "plan",
        "status",
        "readiness",
        "sbom",
        "depgraphs",
        "provenance",
    ] {
        let cli = parse(&["--yes", "release", action]).expect("parses");
        assert!(super::gate(&cli).is_err(), "{action}");
    }
}

#[test]
fn yes_and_non_interactive_are_decoupled_across_init_and_release() {
    // `--yes` on init is rejected (release-only), and `--non-interactive` on a
    // release action is rejected (init-only): authorizing a release can never be
    // learned as a harmless init habit.
    let yes_on_init = parse(&["--yes", "init"]).expect("parses");
    assert!(super::gate(&yes_on_init).is_err());
    let non_interactive_on_release =
        parse(&["--non-interactive", "release", "tag"]).expect("parses");
    assert!(super::gate(&non_interactive_on_release).is_err());
}

#[test]
fn bump_argv_only_on_mutating_release_actions() {
    let flags = [
        vec!["--minor=rust:core"],
        vec!["--major=rust:core"],
        vec!["--patch=rust:core"],
        vec!["--set-version", "rust:core=1.0.0"],
        vec!["--pre", "rc"],
        vec!["--offline"],
    ];
    for flag in &flags {
        for action in ["tag", "publish"] {
            let mut argv = flag.clone();
            argv.extend(["release", action]);
            let cli = parse(&argv).expect("parses");
            assert!(super::gate(&cli).is_ok(), "{flag:?} on {action}");
        }
        for action in [
            "plan",
            "status",
            "readiness",
            "sbom",
            "depgraphs",
            "package",
        ] {
            let mut argv = flag.clone();
            argv.extend(["release", action]);
            let cli = parse(&argv).expect("parses");
            assert!(super::gate(&cli).is_err(), "{flag:?} on {action}");
        }
    }
}

#[test]
fn out_dir_only_on_artifact_release_actions() {
    for action in ["sbom", "depgraphs"] {
        let cli = parse(&["--out-dir", "/tmp/out", "release", action]).expect("parses");
        assert!(super::gate(&cli).is_ok(), "{action}");
    }
    for action in ["plan", "status", "readiness", "tag", "publish", "package"] {
        let cli = parse(&["--out-dir", "/tmp/out", "release", action]).expect("parses");
        assert!(super::gate(&cli).is_err(), "{action}");
    }
}

#[test]
fn base_accepted_on_mutating_release_but_merge_base_is_not() {
    for action in ["tag", "publish"] {
        let cli = parse(&["--base", "v1.0.0", "release", action]).expect("parses");
        assert!(super::gate(&cli).is_ok(), "base {action}");
        let merge = parse(&["--merge-base", "release", action]).expect("parses");
        assert!(super::gate(&merge).is_err(), "merge-base {action}");
    }
    for action in ["plan", "status"] {
        let cli = parse(&["--base", "v1.0.0", "release", action]).expect("parses");
        assert!(super::gate(&cli).is_err(), "base {action}");
    }
}

#[test]
fn dry_run_only_on_rehearsable_release_actions() {
    for action in ["plan", "bump", "publish", "image", "provenance"] {
        let cli = parse(&["--dry-run", "release", action]).expect("parses");
        assert!(super::gate(&cli).is_ok(), "{action}");
    }
    for action in ["status", "tag", "readiness", "sbom", "depgraphs"] {
        let cli = parse(&["--dry-run", "release", action]).expect("parses");
        assert!(super::gate(&cli).is_err(), "{action}");
    }
}

#[test]
fn explain_rejected_on_every_release_action() {
    for action in [
        "plan",
        "status",
        "tag",
        "publish",
        "readiness",
        "sbom",
        "depgraphs",
    ] {
        let cli = parse(&["--explain", "release", action]).expect("parses");
        assert!(super::gate(&cli).is_err(), "{action}");
    }
}

#[test]
fn verbosity_shaping_only_on_mutating_release_actions() {
    // `-v`/`-q` shape the human run reporter that only the mutating actions build,
    // so they stay gated. `--color`, by contrast, is now accepted on every action
    // (like `git`/`cargo`) and is asserted separately below.
    for action in ["tag", "publish"] {
        let cli = parse(&["--verbose", "release", action]).expect("parses");
        assert!(super::gate(&cli).is_ok(), "{action}");
    }
    for action in ["plan", "status", "readiness", "sbom", "depgraphs"] {
        let cli = parse(&["--verbose", "release", action]).expect("parses");
        assert!(super::gate(&cli).is_err(), "{action}");
        // `--color` is accepted regardless — it is a harmless no-op here.
        let colored = parse(&["--color", "auto", "release", action]).expect("parses");
        assert!(super::gate(&colored).is_ok(), "color {action}");
    }
}

#[test]
fn output_format_accepted_on_every_release_action() {
    for action in [
        "plan",
        "status",
        "tag",
        "publish",
        "readiness",
        "sbom",
        "depgraphs",
        "package",
        "checksums",
        "sign",
        "verify",
        "image",
        "provenance",
    ] {
        let cli = parse(&["--output", "jsonl", "release", action]).expect("parses");
        assert!(super::gate(&cli).is_ok(), "{action}");
    }
}

#[test]
fn release_action_classification() {
    assert!(ReleaseAction::Plan.is_projection());
    assert!(ReleaseAction::Status.is_projection());
    assert!(ReleaseAction::Readiness.is_projection());
    assert!(ReleaseAction::Sbom.is_projection());
    assert!(ReleaseAction::Depgraphs.is_projection());
    assert!(ReleaseAction::Package.is_projection());
    assert!(!ReleaseAction::Package.is_mutating());
    assert!(!ReleaseAction::Package.writes_artifacts());
    assert!(!ReleaseAction::Package.accepts_dry_run());
    assert!(ReleaseAction::Checksums.is_projection());
    assert!(!ReleaseAction::Checksums.is_mutating());
    assert!(!ReleaseAction::Checksums.writes_artifacts());
    assert!(!ReleaseAction::Checksums.accepts_dry_run());
    assert!(ReleaseAction::Sign.is_projection());
    assert!(!ReleaseAction::Sign.is_mutating());
    assert!(!ReleaseAction::Sign.writes_artifacts());
    assert!(!ReleaseAction::Sign.accepts_dry_run());
    assert!(ReleaseAction::Verify.is_projection());
    assert!(!ReleaseAction::Verify.is_mutating());
    assert!(!ReleaseAction::Verify.writes_artifacts());
    assert!(!ReleaseAction::Verify.accepts_dry_run());
    assert!(ReleaseAction::Provenance.is_projection());
    assert!(!ReleaseAction::Provenance.is_mutating());
    assert!(!ReleaseAction::Provenance.writes_artifacts());
    assert!(!ReleaseAction::Provenance.requires_confirmation());
    assert!(ReleaseAction::Provenance.accepts_dry_run());
    assert!(!ReleaseAction::Tag.is_projection());
    assert!(ReleaseAction::Tag.is_mutating());
    assert!(ReleaseAction::Publish.is_mutating());
    assert!(!ReleaseAction::Plan.is_mutating());
    assert!(!ReleaseAction::Readiness.is_mutating());
    assert!(ReleaseAction::Sbom.writes_artifacts());
    assert!(ReleaseAction::Depgraphs.writes_artifacts());
    assert!(!ReleaseAction::Readiness.writes_artifacts());
    assert!(ReleaseAction::Plan.accepts_dry_run());
    assert!(ReleaseAction::Publish.accepts_dry_run());
    assert!(!ReleaseAction::Status.accepts_dry_run());
    assert!(!ReleaseAction::Tag.accepts_dry_run());
    assert!(!ReleaseAction::Readiness.accepts_dry_run());
    // `image` is a registry mutation: it requires `--yes` but is not a
    // version cut, so it classifies via `requires_confirmation`, honors
    // `--dry-run`, and is neither a projection nor a version-cut mutation.
    assert!(!ReleaseAction::Image.is_projection());
    assert!(!ReleaseAction::Image.is_mutating());
    assert!(!ReleaseAction::Image.writes_artifacts());
    assert!(ReleaseAction::Image.accepts_dry_run());
    assert!(ReleaseAction::Image.requires_confirmation());
    assert!(ReleaseAction::Bump.requires_confirmation());
    assert!(ReleaseAction::Tag.requires_confirmation());
    assert!(ReleaseAction::Publish.requires_confirmation());
    assert!(!ReleaseAction::Sign.requires_confirmation());
    assert!(!ReleaseAction::Plan.requires_confirmation());
}

#[test]
fn init_flags_only_on_init() {
    for args in [
        ["--non-interactive", "init"].as_slice(),
        ["--print", "init"].as_slice(),
        ["--root", "/tmp", "init"].as_slice(),
        ["--force", "rust", "init"].as_slice(),
    ] {
        assert!(super::gate(&parse(args).unwrap()).is_ok(), "{args:?}");
    }
    for args in [
        ["--non-interactive", "plan", "test"].as_slice(),
        ["--print", "plan", "test"].as_slice(),
        ["--root", "/tmp", "modules"].as_slice(),
        ["--force", "rust", "graph"].as_slice(),
    ] {
        assert!(super::gate(&parse(args).unwrap()).is_err(), "{args:?}");
    }
}

#[test]
fn init_accepts_non_interactive_but_not_the_release_yes_flag() {
    let non_interactive = parse(&["--non-interactive", "init"]).expect("parses");
    assert!(non_interactive.non_interactive);
    assert!(super::gate(&non_interactive).is_ok());
    // `--yes` is release confirmation only and must not slip in on init.
    let yes = parse(&["--yes", "init"]).expect("parses");
    assert!(super::gate(&yes).is_err());
}

#[test]
fn graph_format_only_on_graph() {
    assert!(super::gate(&parse(&["--format", "dot", "graph"]).unwrap()).is_ok());
    assert!(super::gate(&parse(&["--format", "dot", "plan", "t"]).unwrap()).is_err());
}

#[test]
fn no_cache_only_on_task_verbs() {
    for args in [
        ["--no-cache", "run", "test"].as_slice(),
        ["--no-cache", "plan", "test"].as_slice(),
    ] {
        assert!(super::gate(&parse(args).unwrap()).is_ok(), "{args:?}");
    }
    for args in [
        ["--no-cache", "release", "publish"].as_slice(),
        ["--no-cache", "affected", "test"].as_slice(),
        ["--no-cache", "modules"].as_slice(),
    ] {
        assert!(super::gate(&parse(args).unwrap()).is_err(), "{args:?}");
    }
}

#[test]
fn execution_flags_rejected_on_cache() {
    assert!(super::gate(&parse(&["--dry-run", "cache", "path"]).unwrap()).is_err());
}

#[test]
fn refresh_only_on_cache_aware_verbs() {
    for args in [
        ["--refresh", "run", "test"].as_slice(),
        ["--refresh", "plan", "test"].as_slice(),
    ] {
        assert!(super::gate(&parse(args).unwrap()).is_ok(), "{args:?}");
    }
    for args in [
        ["--refresh", "release", "publish"].as_slice(),
        ["--refresh", "affected", "test"].as_slice(),
        ["--refresh", "modules"].as_slice(),
    ] {
        assert!(super::gate(&parse(args).unwrap()).is_err(), "{args:?}");
    }
}

#[test]
fn refresh_and_no_cache_are_mutually_exclusive() {
    assert!(super::gate(&parse(&["--refresh", "--no-cache", "run", "test"]).unwrap()).is_err());
}

#[test]
fn timeout_only_on_task_apply_verbs() {
    for args in [
        ["--timeout", "5s", "run", "test"].as_slice(),
        ["--timeout", "2m", "test"].as_slice(),
    ] {
        assert!(super::gate(&parse(args).unwrap()).is_ok(), "{args:?}");
    }
    // `plan` stops at PLAN and `release`/introspection never run bounded units, so
    // the bound is a no-op and is rejected.
    for args in [
        ["--timeout", "5s", "plan", "test"].as_slice(),
        ["--timeout", "5s", "release", "publish"].as_slice(),
        ["--timeout", "5s", "affected", "test"].as_slice(),
    ] {
        assert!(super::gate(&parse(args).unwrap()).is_err(), "{args:?}");
    }
}

#[test]
fn compute_budget_only_on_task_apply_verbs() {
    for args in [
        ["--compute-budget", "8", "run", "test"].as_slice(),
        ["--compute-budget", "inherit", "test"].as_slice(),
        // `coverage` fans its task out per module like any run.
        ["--compute-budget", "8", "coverage"].as_slice(),
    ] {
        assert!(super::gate(&parse(args).unwrap()).is_ok(), "{args:?}");
    }
    // `plan` stops at PLAN and `release`/introspection never run bounded
    // units, so the budget is a no-op and is rejected rather than silently
    // ignored.
    for args in [
        ["--compute-budget", "8", "plan", "test"].as_slice(),
        ["--compute-budget", "8", "release", "publish"].as_slice(),
        ["--compute-budget", "8", "affected", "test"].as_slice(),
        ["--compute-budget", "8", "doctor"].as_slice(),
    ] {
        assert!(super::gate(&parse(args).unwrap()).is_err(), "{args:?}");
    }
}

#[test]
fn timeout_rejects_a_malformed_or_zero_duration() {
    assert!(parse(&["--timeout", "soon", "test"]).is_err());
    assert!(parse(&["--timeout", "0s", "test"]).is_err());
}

#[test]
fn watch_only_on_task_apply_verbs() {
    for args in [
        ["--watch", "run", "test"].as_slice(),
        ["--watch", "test"].as_slice(),
    ] {
        assert!(super::gate(&parse(args).unwrap()).is_ok(), "{args:?}");
    }
    for args in [
        ["--watch", "plan", "test"].as_slice(),
        ["--watch", "affected", "test"].as_slice(),
        ["--watch", "release", "publish"].as_slice(),
        ["--watch", "graph"].as_slice(),
    ] {
        assert!(super::gate(&parse(args).unwrap()).is_err(), "{args:?}");
    }
}

#[test]
fn watch_rejects_plan_only_cuts() {
    assert!(super::gate(&parse(&["--watch", "--dry-run", "run", "test"]).unwrap()).is_err());
    assert!(super::gate(&parse(&["--watch", "--explain", "run", "test"]).unwrap()).is_err());
}

#[test]
fn apply_execution_flags_reject_plan_only_cuts() {
    // `--fail-fast`/`--timeout`, like `--watch`, drive real unit execution, so
    // combining them with a PLAN-only cut (`--dry-run`/`--explain`) — even on an
    // APPLY verb that otherwise accepts them — is rejected rather than silently
    // ignored.
    for args in [
        ["--fail-fast", "--dry-run", "run", "test"].as_slice(),
        ["--timeout", "5s", "--dry-run", "run", "test"].as_slice(),
    ] {
        assert!(super::gate(&parse(args).unwrap()).is_err(), "{args:?}");
    }
    for args in [
        ["--fail-fast", "--explain", "test"].as_slice(),
        ["--timeout", "5s", "--explain", "test"].as_slice(),
        ["test", "--fail-fast", "--explain"].as_slice(),
    ] {
        assert!(bare_task_execution(args).is_err(), "{args:?}");
    }
}

/// The execution a bare task dispatches with: gate, then merge its own flags.
fn bare_task_execution(args: &[&str]) -> rskit_errors::AppResult<super::ExecutionArgs> {
    let cli = parse(args).unwrap();
    super::gate(&cli)?;
    let Command::External(tokens) = &cli.command else {
        panic!("{args:?} is not a bare task");
    };
    let invocation = crate::grammar::parse_task(tokens)?;
    cli.execution.clone().for_task(invocation.flags)
}

#[test]
fn check_combinations_rejects_each_execution_flag_under_plan_only() {
    use super::ExecutionArgs;
    let watch = ExecutionArgs {
        watch: true,
        ..ExecutionArgs::default()
    };
    let fail_fast = ExecutionArgs {
        fail_fast: true,
        ..ExecutionArgs::default()
    };
    let timeout = ExecutionArgs {
        timeout: Some(std::time::Duration::from_secs(1)),
        ..ExecutionArgs::default()
    };
    for args in [&watch, &fail_fast, &timeout] {
        assert!(args.check_combinations(true).is_err(), "{args:?}");
        // No PLAN-only cut: every execution flag is accepted.
        assert!(args.check_combinations(false).is_ok(), "{args:?}");
    }
    let debounce_alone = ExecutionArgs {
        watch_debounce_ms: Some(5),
        ..ExecutionArgs::default()
    };
    assert!(debounce_alone.check_combinations(false).is_err());
    let both_cache_modes = ExecutionArgs {
        refresh: true,
        no_cache: true,
        ..ExecutionArgs::default()
    };
    assert!(both_cache_modes.check_combinations(false).is_err());
}

#[test]
fn merged_with_lets_task_values_win_and_adds_up_the_rest() {
    let global = parse(&[
        "--timeout",
        "5s",
        "-v",
        "--module",
        "a",
        "--fail-fast",
        "test",
    ])
    .unwrap()
    .execution;
    let task = super::ExecutionArgs {
        timeout: Some(std::time::Duration::from_secs(9)),
        verbose: 2,
        module: vec!["b".to_owned()],
        dry_run: true,
        ..super::ExecutionArgs::default()
    };
    let merged = global.merged_with(task);
    assert_eq!(merged.timeout, Some(std::time::Duration::from_secs(9)));
    assert_eq!(merged.verbose, 3);
    assert_eq!(merged.module, vec!["a", "b"]);
    assert!(merged.fail_fast && merged.dry_run);
    assert!(merged.check_combinations(merged.is_plan_only()).is_err());
}

#[test]
fn watch_debounce_requires_watch() {
    assert!(super::gate(&parse(&["--watch-debounce-ms", "500", "run", "test"]).unwrap()).is_err());
    assert!(
        super::gate(&parse(&["--watch", "--watch-debounce-ms", "500", "run", "test"]).unwrap())
            .is_ok()
    );
}

#[test]
fn a_bare_task_defers_flag_combinations_to_its_merged_flags() {
    assert!(bare_task_execution(&["--watch-debounce-ms", "500", "test", "--watch"]).is_ok());
    assert!(bare_task_execution(&["--watch-debounce-ms", "500", "test"]).is_err());
}

#[test]
fn execution_flags_rejected_on_introspection_verbs() {
    for args in [
        ["--dry-run", "explain", "test"].as_slice(),
        ["--explain", "affected", "test"].as_slice(),
        ["--fail-fast", "modules"].as_slice(),
        ["--output", "jsonl", "graph"].as_slice(),
    ] {
        let cli = parse(args).expect("parses");
        assert!(super::gate(&cli).is_err(), "{args:?}");
    }
}

#[test]
fn execution_flags_accepted_on_execution_verbs() {
    for args in [
        ["--dry-run", "run", "test"].as_slice(),
        ["--explain", "plan", "test"].as_slice(),
        ["--output", "jsonl", "release", "plan"].as_slice(),
        ["--fail-fast", "test"].as_slice(),
    ] {
        let cli = parse(args).expect("parses");
        assert!(super::gate(&cli).is_ok(), "{args:?}");
    }
}

#[test]
fn output_format_accepted_on_the_modules_projection() {
    let cli = parse(["--output", "jsonl", "modules"].as_slice()).expect("parses");
    assert!(super::gate(&cli).is_ok());
}

#[test]
fn output_format_accepted_on_the_affected_projection() {
    let cli = parse(["--output", "jsonl", "affected", "test"].as_slice()).expect("parses");
    assert!(super::gate(&cli).is_ok());
}

#[test]
fn fail_fast_rejected_on_non_apply_verbs() {
    // `--fail-fast` shapes APPLY scheduling, so it is a no-op on `plan` (PLAN-only)
    // and `release` (single linear pipeline) and rejected there.
    for args in [
        ["--fail-fast", "plan", "test"].as_slice(),
        ["--fail-fast", "release", "publish"].as_slice(),
    ] {
        let cli = parse(args).expect("parses");
        assert!(super::gate(&cli).is_err(), "{args:?}");
    }
}

#[test]
fn fail_fast_accepted_on_task_apply_verbs() {
    for args in [
        ["--fail-fast", "run", "test"].as_slice(),
        ["--fail-fast", "test"].as_slice(),
    ] {
        let cli = parse(args).expect("parses");
        assert!(super::gate(&cli).is_ok(), "{args:?}");
    }
}

#[test]
fn verbosity_rejected_on_non_execution_verbs() {
    // `-v`/`-q` only shape the human reporter the execution verbs build; the
    // introspection/maintenance verbs ignore it, so it is rejected there.
    for args in [
        ["-v", "modules"].as_slice(),
        ["--verbose", "graph"].as_slice(),
        ["-q", "cache", "path"].as_slice(),
        ["--quiet", "explain", "test"].as_slice(),
    ] {
        let cli = parse(args).expect("parses");
        assert!(super::gate(&cli).is_err(), "{args:?}");
    }
}

#[test]
fn verbosity_accepted_on_execution_verbs() {
    for args in [
        ["-v", "run", "test"].as_slice(),
        ["-q", "plan", "test"].as_slice(),
        ["--verbose", "release", "publish"].as_slice(),
        ["-vv", "test"].as_slice(),
    ] {
        let cli = parse(args).expect("parses");
        assert!(super::gate(&cli).is_ok(), "{args:?}");
    }
}

#[test]
fn baseline_flags_accepted_on_changed_selection_verbs() {
    for args in [
        ["--base", "origin/main", "run", "test"].as_slice(),
        ["--base", "origin/main", "plan", "test"].as_slice(),
        ["--merge-base", "affected", "test"].as_slice(),
        ["--base", "origin/main", "test"].as_slice(),
    ] {
        let cli = parse(args).expect("parses");
        assert!(super::gate(&cli).is_ok(), "{args:?}");
    }
}

#[test]
fn baseline_flags_rejected_on_other_verbs() {
    for args in [
        ["--merge-base", "release", "publish"].as_slice(),
        ["--merge-base", "modules"].as_slice(),
        ["--base", "origin/main", "graph"].as_slice(),
        ["--merge-base", "explain", "test"].as_slice(),
    ] {
        let cli = parse(args).expect("parses");
        assert!(super::gate(&cli).is_err(), "{args:?}");
    }
}

#[test]
fn baseline_flags_resolve_into_engine_flags() {
    let cli =
        parse(&["--base", "origin/main", "--merge-base", "affected", "test"]).expect("parses");
    let flags = cli.execution.baseline_flags();
    assert_eq!(flags.base.as_deref(), Some("origin/main"));
    assert!(flags.merge_base);
}

#[test]
fn verbosity_resolves_from_the_net_of_verbose_and_quiet() {
    use super::Verbosity;
    assert_eq!(Verbosity::from_counts(0, 0), Verbosity::Normal);
    assert_eq!(Verbosity::from_counts(2, 0), Verbosity::Verbose);
    assert_eq!(Verbosity::from_counts(0, 1), Verbosity::Quiet);
    // A balanced net is normal; an excess on either side wins.
    assert_eq!(Verbosity::from_counts(1, 1), Verbosity::Normal);
    assert_eq!(Verbosity::from_counts(2, 1), Verbosity::Verbose);
    assert_eq!(Verbosity::from_counts(1, 2), Verbosity::Quiet);
}

#[test]
fn explain_raises_execution_verbosity() {
    use super::Verbosity;
    assert_eq!(Verbosity::for_execution(0, 0, true), Verbosity::Verbose);
    assert_eq!(Verbosity::for_execution(0, 1, true), Verbosity::Normal);
}

#[test]
fn compute_budget_flag_accepts_words_and_integers() {
    use toven_ports::ComputeBudget;
    assert_eq!(
        parse(&["--compute-budget", "auto", "test"])
            .expect("parses")
            .execution
            .compute_budget,
        Some(ComputeBudget::Auto)
    );
    assert_eq!(
        parse(&["--compute-budget", "inherit", "test"])
            .expect("parses")
            .execution
            .compute_budget,
        Some(ComputeBudget::Inherit)
    );
    assert_eq!(
        parse(&["--compute-budget", "8", "test"])
            .expect("parses")
            .execution
            .compute_budget,
        Some(ComputeBudget::fixed(8))
    );
    // Zero is the numeric spelling of the opt-out, matching `inherit`.
    assert_eq!(
        parse(&["--compute-budget", "0", "test"])
            .expect("parses")
            .execution
            .compute_budget,
        Some(ComputeBudget::Inherit)
    );
}

#[test]
fn compute_budget_flag_rejects_an_unknown_word() {
    let error = parse(&["--compute-budget", "turbo", "test"]).expect_err("rejected");
    assert!(error.to_string().contains("compute-budget"), "{error}");
}
