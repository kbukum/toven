//! Watch-mode file invalidation and rerun orchestration.

use std::{
    collections::BTreeSet,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    time::{Duration, Instant},
};

use clap::ArgMatches;
use notify::{RecursiveMode, Watcher};

use crate::{
    adapter::AdapterRegistry,
    cli::{
        affected::modules_from_discovered,
        run::{ActivePersistentProcess, run_task_once_for_watch},
    },
    config::load_workspace,
    core::{AppError, AppResult, ScopedModuleKey, scoped_module_display},
    engine::{
        affected::{ChangedPath, affected_modules},
        discover_workspace_task_profiles,
    },
    exec::{
        CtrlCHandler, SharedCancellation, spawn_ctrl_c_handler_with_notify, stop_ctrl_c_handler,
    },
    report::OutputFormat,
};

const WATCH_EVENT_QUEUE_CAPACITY: usize = 1;

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

    let (event_tx, event_rx) = mpsc::sync_channel(WATCH_EVENT_QUEUE_CAPACITY);
    let pending_events = Arc::new(Mutex::new(PendingWatchEvents::default()));
    let cancellation = SharedCancellation::new();
    let watch_event_tx = event_tx.clone();
    let watch_pending_events = Arc::clone(&pending_events);
    let watch_root = watched_root.clone();
    let mut watcher = notify::recommended_watcher(move |event| {
        queue_watch_event(&watch_event_tx, &watch_pending_events, &watch_root, event);
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
    let ctrl_c_handler = spawn_watch_ctrl_c_listener(event_tx, cancellation.clone())?;

    // One-shot cancellation is shared with every run; once Ctrl-C trips it, watch exits.
    let run_result = run_watch_loop(WatchLoop {
        matches,
        stdout,
        stderr,
        event_rx: &event_rx,
        pending_events: &pending_events,
        config: &config,
        task,
        watched_root: &watched_root,
        output_format,
        debounce,
        watch_once,
        cancellation,
    });
    if let Err(error) = run_result {
        let _ = stop_ctrl_c_handler(Some(ctrl_c_handler));
        return Err(error);
    }
    stop_ctrl_c_handler(Some(ctrl_c_handler))?;
    Ok(())
}

struct WatchLoop<'a, Out, Err> {
    matches: &'a ArgMatches,
    stdout: &'a mut Out,
    stderr: &'a mut Err,
    event_rx: &'a Receiver<WatchEvent>,
    pending_events: &'a SharedPendingWatchEvents,
    config: &'a Path,
    task: &'a str,
    watched_root: &'a Path,
    output_format: OutputFormat,
    debounce: Duration,
    watch_once: bool,
    cancellation: SharedCancellation,
}

fn run_watch_loop<Out, Err>(mut ctx: WatchLoop<'_, Out, Err>) -> AppResult<()>
where
    Out: Write,
    Err: Write,
{
    let mut persistent_processes = run_task_once_for_watch(
        ctx.matches,
        ctx.stdout,
        ctx.stderr,
        None,
        ctx.cancellation.clone(),
    )?;
    let loop_result = run_watch_reruns(&mut persistent_processes, &mut ctx);
    if let Err(error) = loop_result {
        let _ = shutdown_persistent_processes(persistent_processes);
        return Err(error);
    }
    shutdown_persistent_processes(persistent_processes)
}

fn run_watch_reruns<Out, Err>(
    persistent_processes: &mut Vec<ActivePersistentProcess>,
    ctx: &mut WatchLoop<'_, Out, Err>,
) -> AppResult<()>
where
    Out: Write,
    Err: Write,
{
    loop {
        let changed = next_changed_paths(ctx.event_rx, ctx.pending_events, ctx.debounce)?;
        if changed.is_empty() {
            continue;
        }
        let workspace = load_workspace(ctx.config)?;
        validate_watch_root_unchanged(ctx.watched_root, &workspace.root)?;
        let discovered =
            discover_workspace_task_profiles(&workspace, ctx.task, &AdapterRegistry::default())?;
        let modules = modules_from_discovered(&discovered)?;
        if config_changed(ctx.watched_root, ctx.config, &changed)? {
            write_watch_change(ctx.stdout, ctx.stderr, ctx.output_format, &changed, None)?;
            shutdown_persistent_processes(std::mem::take(persistent_processes))?;
            *persistent_processes = run_task_once_for_watch(
                ctx.matches,
                ctx.stdout,
                ctx.stderr,
                None,
                ctx.cancellation.clone(),
            )?;
        } else {
            let affected = affected_modules(&modules, &changed, &workspace.dependency_overlays)?;
            if affected.closure.is_empty() {
                continue;
            }
            write_watch_change(
                ctx.stdout,
                ctx.stderr,
                ctx.output_format,
                &changed,
                Some(&affected.closure),
            )?;
            shutdown_affected_persistent_processes(persistent_processes, &affected.closure)?;
            persistent_processes.extend(run_task_once_for_watch(
                ctx.matches,
                ctx.stdout,
                ctx.stderr,
                Some(&affected.closure),
                ctx.cancellation.clone(),
            )?);
        }
        if ctx.watch_once {
            return Ok(());
        }
    }
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
    affected: &BTreeSet<ScopedModuleKey>,
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
    event_rx: &Receiver<WatchEvent>,
    pending_events: &SharedPendingWatchEvents,
    debounce: Duration,
) -> AppResult<Vec<ChangedPath>> {
    let mut paths = BTreeSet::new();
    let first = event_rx.recv().map_err(|error| {
        AppError::new(
            crate::core::ErrorCode::Internal,
            format!("watch event channel closed: {error}"),
        )
    })?;
    collect_event_paths(first, pending_events, &mut paths)?;

    let deadline = Instant::now() + debounce;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match event_rx.recv_timeout(remaining) {
            Ok(event) => collect_event_paths(event, pending_events, &mut paths)?,
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
    event: WatchEvent,
    pending_events: &SharedPendingWatchEvents,
    paths: &mut BTreeSet<PathBuf>,
) -> AppResult<()> {
    match event {
        WatchEvent::Wake => drain_pending_watch_events(pending_events, paths),
        WatchEvent::Error(message) => Err(AppError::new(crate::core::ErrorCode::Internal, message)),
        WatchEvent::CtrlC => Err(AppError::new(
            crate::core::ErrorCode::Cancelled,
            "watch cancelled by ctrl-c",
        )),
    }
}

enum WatchEvent {
    Wake,
    CtrlC,
    Error(String),
}

fn spawn_watch_ctrl_c_listener(
    event_tx: SyncSender<WatchEvent>,
    cancellation: SharedCancellation,
) -> AppResult<CtrlCHandler> {
    spawn_ctrl_c_handler_with_notify(cancellation.clone(), move || {
        send_watch_ctrl_c(&event_tx, &cancellation);
    })
}

fn send_watch_ctrl_c(event_tx: &SyncSender<WatchEvent>, cancellation: &SharedCancellation) {
    cancellation.cancel();
    let _ = event_tx.send(WatchEvent::CtrlC);
}

#[derive(Default)]
struct PendingWatchEvents {
    paths: BTreeSet<PathBuf>,
    error: Option<String>,
}

type SharedPendingWatchEvents = Arc<Mutex<PendingWatchEvents>>;

fn queue_watch_event(
    event_tx: &SyncSender<WatchEvent>,
    pending_events: &SharedPendingWatchEvents,
    root: &Path,
    event: notify::Result<notify::Event>,
) {
    match pending_events.lock() {
        Ok(mut pending_events) => {
            match event {
                Ok(event) => {
                    for path in event.paths {
                        if let Some(relative) = normalize_changed_path(root, &path)
                            && !is_ignored(&relative)
                        {
                            pending_events.paths.insert(relative);
                        }
                    }
                }
                Err(error) => {
                    pending_events.error = Some(format!("failed to read watch event: {error}"));
                }
            }
            wake_watch_loop(event_tx);
        }
        Err(_) => {
            let _ = event_tx.try_send(WatchEvent::Error(
                "watch pending event state is unavailable".to_string(),
            ));
        }
    }
}

fn wake_watch_loop(event_tx: &SyncSender<WatchEvent>) {
    match event_tx.try_send(WatchEvent::Wake) {
        Ok(()) | Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {}
    }
}

fn drain_pending_watch_events(
    pending_events: &SharedPendingWatchEvents,
    paths: &mut BTreeSet<PathBuf>,
) -> AppResult<()> {
    let mut pending_events = pending_events.lock().map_err(|_| {
        AppError::new(
            crate::core::ErrorCode::Internal,
            "watch pending event state is unavailable",
        )
    })?;
    if let Some(error) = pending_events.error.take() {
        return Err(AppError::new(crate::core::ErrorCode::Internal, error));
    }
    paths.append(&mut pending_events.paths);
    drop(pending_events);
    Ok(())
}

fn normalize_changed_path(root: &Path, path: &Path) -> Option<PathBuf> {
    let relative = path.strip_prefix(root).ok()?;
    Some(normalize_relative_path(relative))
}

fn normalize_relative_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(_) | Component::RootDir | Component::Prefix(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
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

fn config_changed(root: &Path, config: &Path, changed: &[ChangedPath]) -> AppResult<bool> {
    let current_dir = std::env::current_dir().map_err(AppError::internal)?;
    Ok(config_changed_from_dir(root, &current_dir, config, changed))
}

fn config_changed_from_dir(
    root: &Path,
    current_dir: &Path,
    config: &Path,
    changed: &[ChangedPath],
) -> bool {
    let config_path = if config.is_absolute() {
        config.to_path_buf()
    } else {
        current_dir.join(config)
    };
    let relative_config = config_path
        .strip_prefix(root)
        .map_or_else(|_| normalize_relative_path(config), normalize_relative_path);
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
    modules: Option<&BTreeSet<ScopedModuleKey>>,
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
                .map(scoped_module_display)
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
        sync::{Arc, Mutex, mpsc},
        thread,
        time::{Duration, Instant},
    };

    use crate::{
        cli::commands::run_command, core::ModuleId, engine::affected::ChangedPath,
        exec::SharedCancellation, report::OutputFormat,
    };

    use super::{
        PendingWatchEvents, WatchEvent, config_changed_from_dir, is_ignored, next_changed_paths,
        queue_watch_event, run_watch, send_watch_ctrl_c, validate_watch_root_unchanged,
        write_watch_change, write_watch_started,
    };

    #[test]
    fn ignores_generated_and_dependency_directories() {
        assert!(is_ignored(Path::new(".git/index")));
        assert!(is_ignored(Path::new(".toven/cache/v3/record")));
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
        let modules =
            BTreeSet::from([("rust".to_string(), ModuleId::new("api").expect("module id"))]);
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
        assert!(stderr.contains("watch: rerun rust/api"));
    }

    #[test]
    fn config_change_forces_unfiltered_rerun() {
        assert!(config_changed_from_dir(
            Path::new("/workspace"),
            Path::new("/workspace"),
            Path::new("/workspace/toven.toml"),
            &[ChangedPath::new("toven.toml")],
        ));
        assert!(config_changed_from_dir(
            Path::new("/workspace"),
            Path::new("/workspace"),
            Path::new("./toven.toml"),
            &[ChangedPath::new("toven.toml")],
        ));
        assert!(config_changed_from_dir(
            Path::new("/workspace"),
            Path::new("/workspace"),
            Path::new("/workspace/sub/../toven.toml"),
            &[ChangedPath::new("toven.toml")],
        ));
        assert!(config_changed_from_dir(
            Path::new("/workspace"),
            Path::new("/workspace/sub"),
            Path::new("../toven.toml"),
            &[ChangedPath::new("toven.toml")],
        ));
        assert!(!config_changed_from_dir(
            Path::new("/workspace"),
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
        let (tx, rx) = mpsc::sync_channel(1);
        let pending_events = Arc::new(Mutex::new(PendingWatchEvents::default()));
        queue_watch_event(
            &tx,
            &pending_events,
            root,
            Ok(notify::Event::new(notify::EventKind::Any).add_path(root.join("src/lib.rs"))),
        );
        queue_watch_event(
            &tx,
            &pending_events,
            root,
            Ok(notify::Event::new(notify::EventKind::Any).add_path(root.join("target/debug/app"))),
        );

        let changed = next_changed_paths(&rx, &pending_events, Duration::from_millis(1))
            .expect("events are collected");

        assert_eq!(changed, [ChangedPath::new("src/lib.rs")]);
    }

    #[test]
    fn next_changed_paths_reports_ctrl_c_cancellation() {
        let (tx, rx) = mpsc::sync_channel(1);
        let pending_events = Arc::new(Mutex::new(PendingWatchEvents::default()));
        tx.send(WatchEvent::CtrlC).expect("send ctrl-c event");

        let error = next_changed_paths(&rx, &pending_events, Duration::from_millis(1))
            .expect_err("ctrl-c should cancel watch");

        assert_eq!(error.code, crate::core::ErrorCode::Cancelled);
    }

    #[test]
    fn watch_ctrl_c_event_cancels_running_work() {
        let cancellation = SharedCancellation::new();
        let (tx, rx) = mpsc::sync_channel(1);

        send_watch_ctrl_c(&tx, &cancellation);

        assert!(cancellation.token().is_cancelled());
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)),
            Ok(WatchEvent::CtrlC)
        ));
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
