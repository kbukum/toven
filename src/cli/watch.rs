//! Watch-mode file invalidation and rerun orchestration.

use std::{
    collections::BTreeSet,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

use clap::ArgMatches;
use notify::{RecursiveMode, Watcher};

use crate::{
    cli::{
        affected::modules_from_discovered,
        run::{ActivePersistentProcess, run_task_once_for_watch},
    },
    config::load_workspace,
    core::{AppError, AppResult, ModuleId},
    engine::{
        affected::{ChangedPath, affected_modules},
        discover_workspace_task_profiles,
    },
    lang::LangRegistry,
    report::OutputFormat,
};

pub(super) fn run_watch(
    matches: &ArgMatches,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> AppResult<()> {
    let config = PathBuf::from(
        matches
            .get_one::<String>("config")
            .expect("clap supplies the run config default"),
    );
    let task = matches
        .get_one::<String>("task")
        .expect("clap requires a run task")
        .as_str();
    let output_format = OutputFormat::parse(
        matches
            .get_one::<String>("output")
            .expect("clap supplies output default"),
    )?;
    let debounce = Duration::from_millis(
        *matches
            .get_one::<u64>("watch-debounce-ms")
            .expect("clap supplies watch debounce default"),
    );
    let watch_once = matches.get_flag("watch-once");
    let workspace = load_workspace(config.clone())?;
    let watched_root = workspace.root;
    write_watch_started(stdout, stderr, output_format, &watched_root)?;

    let (event_tx, event_rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = event_tx.send(event);
    })
    .map_err(|error| {
        AppError::new(
            crate::core::ErrorCode::Internal,
            format!("failed to initialize watcher: {error}"),
        )
    })?;
    watcher
        .watch(&watched_root, RecursiveMode::Recursive)
        .map_err(|error| {
            AppError::new(
                crate::core::ErrorCode::Internal,
                format!("failed to watch '{}': {error}", watched_root.display()),
            )
        })?;

    let mut persistent_processes = run_task_once_for_watch(matches, stdout, stderr, None)?;
    loop {
        let changed = next_changed_paths(&event_rx, &watched_root, debounce)?;
        if changed.is_empty() {
            continue;
        }
        let workspace = load_workspace(config.clone())?;
        validate_watch_root_unchanged(&watched_root, &workspace.root)?;
        let discovered =
            discover_workspace_task_profiles(&workspace, task, &LangRegistry::default())?;
        let modules = modules_from_discovered(&discovered)?;
        if config_changed(&watched_root, &config, &changed) {
            write_watch_change(stdout, stderr, output_format, &changed, None)?;
            shutdown_persistent_processes(std::mem::take(&mut persistent_processes))?;
            persistent_processes = run_task_once_for_watch(matches, stdout, stderr, None)?;
        } else {
            let affected = affected_modules(&modules, &changed)?;
            if affected.closure.is_empty() {
                continue;
            }
            write_watch_change(
                stdout,
                stderr,
                output_format,
                &changed,
                Some(&affected.closure),
            )?;
            shutdown_affected_persistent_processes(&mut persistent_processes, &affected.closure)?;
            persistent_processes.extend(run_task_once_for_watch(
                matches,
                stdout,
                stderr,
                Some(&affected.closure),
            )?);
        }
        if watch_once {
            break;
        }
    }
    shutdown_persistent_processes(persistent_processes)?;
    Ok(())
}

fn shutdown_persistent_processes(processes: Vec<ActivePersistentProcess>) -> AppResult<()> {
    let mut first_error = None;
    for process in processes {
        if let Err(error) = process.shutdown() {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or_else(|| Ok(()), Err)
}

fn shutdown_affected_persistent_processes(
    processes: &mut Vec<ActivePersistentProcess>,
    affected: &BTreeSet<ModuleId>,
) -> AppResult<()> {
    let mut kept = Vec::new();
    let mut first_error = None;
    for process in std::mem::take(processes) {
        if process.is_affected_by(affected) {
            if let Err(error) = process.shutdown() {
                first_error.get_or_insert(error);
            }
        } else {
            kept.push(process);
        }
    }
    *processes = kept;
    first_error.map_or_else(|| Ok(()), Err)
}

fn next_changed_paths(
    event_rx: &mpsc::Receiver<notify::Result<notify::Event>>,
    root: &Path,
    debounce: Duration,
) -> AppResult<Vec<ChangedPath>> {
    let mut paths = BTreeSet::new();
    let first = event_rx.recv().map_err(|error| {
        AppError::new(
            crate::core::ErrorCode::Internal,
            format!("watch event channel closed: {error}"),
        )
    })?;
    collect_event_paths(first, root, &mut paths)?;

    let deadline = Instant::now() + debounce;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match event_rx.recv_timeout(remaining) {
            Ok(event) => collect_event_paths(event, root, &mut paths)?,
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(error) => {
                return Err(AppError::new(
                    crate::core::ErrorCode::Internal,
                    format!("watch event channel closed: {error}"),
                ));
            }
        }
    }

    Ok(paths.into_iter().map(ChangedPath::new).collect())
}

fn collect_event_paths(
    event: notify::Result<notify::Event>,
    root: &Path,
    paths: &mut BTreeSet<PathBuf>,
) -> AppResult<()> {
    let event = event.map_err(|error| {
        AppError::new(
            crate::core::ErrorCode::Internal,
            format!("failed to read watch event: {error}"),
        )
    })?;
    for path in event.paths {
        if let Some(relative) = normalize_changed_path(root, &path)
            && !is_ignored(&relative)
        {
            paths.insert(relative);
        }
    }
    Ok(())
}

fn normalize_changed_path(root: &Path, path: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix(root).ok()?;
    Some(relative.components().collect())
}

fn is_ignored(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::Normal(name)
                if matches!(
                    name.to_str(),
                    Some(".git" | ".toven" | "target" | "node_modules")
                )
        )
    })
}

fn config_changed(root: &Path, config: &Path, changed: &[ChangedPath]) -> bool {
    let relative_config = config
        .strip_prefix(root)
        .map_or_else(|_| config.to_path_buf(), Path::to_path_buf);
    changed
        .iter()
        .any(|path| path.path == relative_config || path.path == Path::new("toven.toml"))
}

fn validate_watch_root_unchanged(watched_root: &Path, current_root: &Path) -> AppResult<()> {
    if watched_root == current_root {
        return Ok(());
    }
    Err(AppError::invalid_input(
        "workspace.root",
        format!(
            "workspace root changed from '{}' to '{}' while watch mode is running; restart watch to use the new root",
            watched_root.display(),
            current_root.display()
        ),
    ))
}

fn write_watch_started(
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    output_format: OutputFormat,
    root: &Path,
) -> AppResult<()> {
    let writer: &mut dyn Write = match output_format {
        OutputFormat::Human => stdout,
        OutputFormat::Jsonl => stderr,
    };
    writeln!(writer, "watch: {}", root.display()).map_err(AppError::internal)
}

fn write_watch_change(
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    output_format: OutputFormat,
    changed: &[ChangedPath],
    modules: Option<&BTreeSet<ModuleId>>,
) -> AppResult<()> {
    let writer: &mut dyn Write = match output_format {
        OutputFormat::Human => stdout,
        OutputFormat::Jsonl => stderr,
    };
    writeln!(writer, "watch: change").map_err(AppError::internal)?;
    for path in changed {
        writeln!(writer, "- {}", path.path.display()).map_err(AppError::internal)?;
    }
    if let Some(modules) = modules {
        writeln!(
            writer,
            "watch: rerun {}",
            modules
                .iter()
                .map(ModuleId::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .map_err(AppError::internal)
    } else {
        writeln!(writer, "watch: rerun all").map_err(AppError::internal)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        fs,
        path::{Path, PathBuf},
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    use crate::{
        cli::commands::run_command, core::ModuleId, engine::affected::ChangedPath,
        report::OutputFormat,
    };

    use super::{
        config_changed, is_ignored, next_changed_paths, run_watch, validate_watch_root_unchanged,
        write_watch_change, write_watch_started,
    };

    #[test]
    fn ignores_generated_and_dependency_directories() {
        assert!(is_ignored(Path::new(".git/index")));
        assert!(is_ignored(Path::new(".toven/cache/v2/record")));
        assert!(is_ignored(Path::new("target/debug/app")));
        assert!(is_ignored(Path::new("ui/node_modules/pkg/index.js")));
        assert!(!is_ignored(Path::new("crates/core/src/lib.rs")));
    }

    #[test]
    fn jsonl_watch_status_uses_stderr() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        write_watch_started(
            &mut stdout,
            &mut stderr,
            OutputFormat::Jsonl,
            Path::new("/workspace"),
        )
        .expect("watch start writes");
        let modules = BTreeSet::from([ModuleId::new("api").expect("module id")]);
        write_watch_change(
            &mut stdout,
            &mut stderr,
            OutputFormat::Jsonl,
            &[ChangedPath::new("api/src/lib.rs")],
            Some(&modules),
        )
        .expect("watch change writes");

        assert!(stdout.is_empty());
        let stderr = String::from_utf8(stderr).expect("stderr is utf-8");
        assert!(stderr.contains("watch: /workspace"));
        assert!(stderr.contains("watch: rerun api"));
    }

    #[test]
    fn config_change_forces_unfiltered_rerun() {
        assert!(config_changed(
            Path::new("/workspace"),
            Path::new("/workspace/toven.toml"),
            &[ChangedPath::new("toven.toml")],
        ));
        assert!(!config_changed(
            Path::new("/workspace"),
            Path::new("/workspace/toven.toml"),
            &[ChangedPath::new("api/src/lib.rs")],
        ));
    }

    #[test]
    fn watch_root_change_requires_restart() {
        let error =
            validate_watch_root_unchanged(Path::new("/workspace/old"), Path::new("/workspace/new"))
                .expect_err("root change should fail");

        assert_eq!(error.code, crate::core::ErrorCode::InvalidInput);
        assert!(error.message.contains("restart watch"));
    }

    #[test]
    fn next_changed_paths_debounces_and_filters_events() {
        let root = Path::new("/workspace");
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(
            notify::Event::new(notify::EventKind::Any).add_path(root.join("src/lib.rs"))
        ))
        .expect("send source event");
        tx.send(Ok(
            notify::Event::new(notify::EventKind::Any).add_path(root.join("target/debug/app"))
        ))
        .expect("send ignored event");

        let changed =
            next_changed_paths(&rx, root, Duration::from_millis(1)).expect("events are collected");

        assert_eq!(changed, [ChangedPath::new("src/lib.rs")]);
    }

    #[test]
    fn watch_once_reruns_after_change() {
        let root = rskit_testutil::test_workspace!("watch-once-rerun");
        let workspace_path = root.path().join("rust-workspace");
        copy_fixture_tree(&root, "rust-workspace", &workspace_path);
        let config_path = write_run_config(&root, &workspace_path);
        init_git_repo(&workspace_path);
        let matches = run_command()
            .try_get_matches_from([
                "toven",
                "smoke",
                "--config",
                &config_path.display().to_string(),
                "--watch",
                "--watch-once",
                "--watch-debounce-ms",
                "20",
            ])
            .expect("watch args parse");
        let changed_source = workspace_path.join("crates/core/src/lib.rs");
        let count_file = workspace_path.join(".toven-run-count");
        let (done_tx, done_rx) = mpsc::channel();

        thread::spawn(move || {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let result = run_watch(&matches, &mut stdout, &mut stderr)
                .map(|()| (stdout, stderr))
                .map_err(|error| error.message);
            let _ = done_tx.send(result);
        });

        wait_for_run_count(&count_file, 2);
        fs::write(
            changed_source,
            "pub fn core() -> &'static str { \"watch-change\" }\n",
        )
        .expect("write changed source");
        let (stdout, stderr) = done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("watch run completes")
            .expect("watch run succeeds");

        assert!(
            stderr.is_empty(),
            "stderr: {}",
            String::from_utf8_lossy(&stderr)
        );
        let stdout = String::from_utf8(stdout).expect("stdout is utf-8");
        assert!(stdout.contains("watch: change"));
        assert!(stdout.contains("watch: rerun"));
        wait_for_run_count(&count_file, 4);
    }

    fn write_run_config(root: &rskit_testutil::TestWorkspace, workspace: &Path) -> PathBuf {
        let config = workspace.join("toven-run.toml");
        let template = root
            .read_fixture_string("run-cache/toven.toml.template")
            .expect("read run-cache config template fixture");
        fs::write(
            &config,
            template.replace("__WORKSPACE_ROOT__", &workspace.display().to_string()),
        )
        .expect("write run config");
        config
    }

    fn copy_fixture_tree(root: &rskit_testutil::TestWorkspace, fixture: &str, destination: &Path) {
        rskit_fs::sync_io::tree::copy_tree(
            &root.fixture_path(fixture).expect("fixture path"),
            destination,
            rskit_fs::sync_io::tree::CopyTreeOptions::default(),
        )
        .expect("copy fixture tree");
    }

    fn init_git_repo(workspace: &Path) {
        git(workspace, ["init", "--initial-branch=main", "--quiet"]);
        git(workspace, ["config", "user.name", "Toven Test"]);
        git(workspace, ["config", "user.email", "toven@example.invalid"]);
        git(workspace, ["add", "-A"]);
        git(workspace, ["commit", "--quiet", "-m", "baseline"]);
    }

    fn git<const N: usize>(workspace: &Path, args: [&str; N]) {
        let output = std::process::Command::new("git")
            .current_dir(workspace)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .expect("git command runs");
        assert!(
            output.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn wait_for_run_count(count_file: &Path, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let count = fs::read_to_string(count_file)
                .map(|value| value.lines().count())
                .unwrap_or_default();
            if count >= expected {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("run count did not reach {expected}");
    }
}
