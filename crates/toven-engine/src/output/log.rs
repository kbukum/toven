//! [`UnitLogs`] — one private log file per unit for a run.
//!
//! Every unit's full output is appended to `<root>/<run>/<unit>.log`, so a
//! quiet terminal can show only a failure's tail and still point at the whole
//! story. Files are created owner-only (they may hold secrets a tool printed),
//! and at most [`UnitLogs::create`] `keep_runs` run directories are kept, so
//! the log store never grows without bound. A live run holds an exclusive lock
//! on its directory's lock file, so another process (or a long watch session)
//! never prunes logs that are still being written. A short root-level lock
//! serializes creating, recreating and pruning run directories, so a new run
//! can never lock a directory a pruner is removing.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::sync_io::dir;
use rskit_util::hash::hash_hex;

/// Prefix shared by every run directory, so pruning never touches other files.
const RUN_PREFIX: &str = "run-";

/// Lock file a live run holds exclusively for as long as it writes logs; at
/// the log root, the lock that serializes run directory changes.
const LOCK_FILE: &str = ".lock";

/// Bound on waiting for the root lock, which is only held while run
/// directories are created or pruned.
const ROOT_LOCK_ATTEMPTS: u32 = 200;
const ROOT_LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(10);

/// Upper bound on suffix attempts when two runs start in the same second.
const MAX_RUN_DIR_ATTEMPTS: usize = 64;

/// The per-run log directory and the open file for each unit.
pub struct UnitLogs {
    root: PathBuf,
    dir: PathBuf,
    files: HashMap<String, (PathBuf, File)>,
    /// Held (locked) until drop, marking this run as live to [`prune`].
    lock: File,
}

impl UnitLogs {
    /// Create and lock a fresh run directory under `root` named after
    /// `run_id`, then remove the oldest finished run directories until at most
    /// `keep_runs` remain. Live (locked) runs are never removed, so more may
    /// remain while several runs are live.
    ///
    /// # Errors
    /// Returns an error if the directory cannot be created, locked or listed,
    /// or if the log root stays locked by another run.
    pub fn create(root: &Path, run_id: &str, keep_runs: usize) -> AppResult<Self> {
        dir::create_all(root)?;
        let guard = RootLock::acquire(root)?;
        let (dir, lock) = create_run_dir(root, run_id)?;
        prune(root, keep_runs.max(1))?;
        guard.release()?;
        Ok(Self {
            root: root.to_path_buf(),
            dir,
            files: HashMap::new(),
            lock,
        })
    }

    /// The run directory every unit log lives in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Append `bytes` to `unit_id`'s log, creating the file on first write.
    ///
    /// A unit that runs again in the same run (a watch rerun) starts its log
    /// afresh, so the file always holds the latest attempt.
    ///
    /// # Errors
    /// Returns an error if the file cannot be created or written.
    pub fn append(&mut self, unit_id: &str, bytes: &[u8]) -> AppResult<()> {
        if !self.files.contains_key(unit_id) {
            let path = self.dir.join(file_name(unit_id));
            let file = match open_private(&path) {
                Ok(file) => file,
                // The directory vanished (removed by hand or by a pruner that
                // raced this run's lock): recreate it rather than fail the unit.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.recreate_dir()?;
                    open_private(&path).map_err(|error| log_error(&path, error))?
                }
                Err(error) => return Err(log_error(&path, error)),
            };
            self.files.insert(unit_id.to_string(), (path, file));
        }
        let Some((path, file)) = self.files.get_mut(unit_id) else {
            return Err(AppError::new(
                ErrorCode::Internal,
                "unit log missing after open",
            ));
        };
        file.write_all(bytes)
            .map_err(|error| log_error(path, error))
    }

    /// Close `unit_id`'s log and return its path, if the unit wrote anything.
    ///
    /// # Errors
    /// Returns an error if the final flush fails.
    pub fn close(&mut self, unit_id: &str) -> AppResult<Option<PathBuf>> {
        let Some((path, mut file)) = self.files.remove(unit_id) else {
            return Ok(None);
        };
        file.flush().map_err(|error| log_error(&path, error))?;
        Ok(Some(path))
    }

    /// Flush `unit_id`'s still-open log and return its path, keeping it open
    /// so later output (a persistent unit draining after it finished) still
    /// lands in the same file.
    ///
    /// # Errors
    /// Returns an error if the flush fails.
    pub fn flush(&mut self, unit_id: &str) -> AppResult<Option<PathBuf>> {
        let Some((path, file)) = self.files.get_mut(unit_id) else {
            return Ok(None);
        };
        file.flush().map_err(|error| log_error(path, error))?;
        Ok(Some(path.clone()))
    }

    fn recreate_dir(&mut self) -> AppResult<()> {
        let guard = RootLock::acquire(&self.root)?;
        match create_private_dir(&self.dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(log_error(&self.dir, error)),
        }
        let lock = lock_run(&self.dir).map_err(|error| log_error(&self.dir, error))?;
        // The old lock file went with its directory; closing it is enough.
        let _ = std::mem::replace(&mut self.lock, lock).unlock();
        guard.release()
    }
}

impl Drop for UnitLogs {
    fn drop(&mut self) {
        // Unlock explicitly: closing alone keeps the lock while a child
        // process forked by another thread still shares the descriptor. If
        // this fails, the close still releases it.
        let _ = self.lock.unlock();
    }
}

/// The log root's lock, held while run directories are created or pruned.
struct RootLock(File);

impl RootLock {
    fn acquire(root: &Path) -> AppResult<Self> {
        let path = root.join(LOCK_FILE);
        let file = open_lock(&path).map_err(|error| log_error(&path, error))?;
        for _ in 0..ROOT_LOCK_ATTEMPTS {
            match file.try_lock() {
                Ok(()) => return Ok(Self(file)),
                Err(std::fs::TryLockError::WouldBlock) => std::thread::sleep(ROOT_LOCK_RETRY),
                Err(std::fs::TryLockError::Error(error)) => return Err(log_error(&path, error)),
            }
        }
        Err(AppError::new(
            ErrorCode::Timeout,
            format!(
                "unit log root {} stayed locked by another run",
                root.display()
            ),
        ))
    }

    fn release(self) -> AppResult<()> {
        self.0
            .unlock()
            .map_err(|error| AppError::internal(error).context("unlock unit log root"))
    }
}

impl Drop for RootLock {
    fn drop(&mut self) {
        // Unlocks on early error returns too; after `release` this is a no-op.
        let _ = self.0.unlock();
    }
}

/// A filesystem-safe, collision-free file name for a unit id.
fn file_name(unit_id: &str) -> String {
    let readable: String = unit_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    let digest = hash_hex(unit_id.as_bytes());
    format!("{readable}-{}.log", &digest[..8])
}

fn create_run_dir(root: &Path, run_id: &str) -> AppResult<(PathBuf, File)> {
    let base = if run_id.starts_with(RUN_PREFIX) {
        run_id.to_string()
    } else {
        format!("{RUN_PREFIX}{run_id}")
    };
    for attempt in 0..MAX_RUN_DIR_ATTEMPTS {
        let name = if attempt == 0 {
            base.clone()
        } else {
            format!("{base}.{attempt}")
        };
        let path = root.join(name);
        match create_private_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(log_error(&path, error)),
        }
        match lock_run(&path) {
            Ok(lock) => return Ok((path, lock)),
            Err(error) => return Err(log_error(&path, error)),
        }
    }
    Err(AppError::new(
        ErrorCode::Internal,
        format!(
            "could not create a unique log directory for {run_id} under {}",
            root.display()
        ),
    ))
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().create(path)
}

fn open_private(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    // The run directory is owner-only, so truncating an earlier attempt's file
    // cannot be raced by another user.
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)
}

fn open_lock(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)
}

/// Create `dir`'s lock file and hold an exclusive lock on it.
fn lock_run(dir: &Path) -> std::io::Result<File> {
    let lock = open_lock(&dir.join(LOCK_FILE))?;
    lock.try_lock().map_err(|error| match error {
        std::fs::TryLockError::Error(error) => error,
        std::fs::TryLockError::WouldBlock => std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "run log directory is locked by another run",
        ),
    })?;
    Ok(lock)
}

/// Whether `dir` belongs to a finished run that may be removed: not while a
/// live run still holds its lock, nor once it is already gone. A directory
/// without a lock file belongs to no live run. Callers hold the root lock, so
/// no new run can lock `dir` after this check.
fn is_finished(dir: &Path) -> AppResult<bool> {
    let path = dir.join(LOCK_FILE);
    match lock_run(dir) {
        // Released before removal: an open lock file blocks deleting its
        // directory on Windows.
        Ok(lock) => lock
            .unlock()
            .map(|()| true)
            .map_err(|error| log_error(&path, error)),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::NotFound
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(log_error(&path, error)),
    }
}

/// Keep only the newest `keep` run directories (names sort by start time),
/// removing the oldest finished ones first and never a live one.
fn prune(root: &Path, keep: usize) -> AppResult<()> {
    let mut runs: Vec<_> = dir::list(root)?
        .into_iter()
        .filter(|entry| entry.is_dir && entry.file_name.to_string_lossy().starts_with(RUN_PREFIX))
        .collect();
    runs.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    let mut remaining = runs.len();
    for entry in runs {
        if remaining <= keep {
            break;
        }
        if !is_finished(&entry.path)? {
            continue;
        }
        dir::remove_all_if_exists(&entry.path)?;
        remaining -= 1;
    }
    Ok(())
}

fn log_error(path: &Path, error: std::io::Error) -> AppError {
    AppError::internal(error).context(format!("unit log {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{RootLock, UnitLogs, file_name, is_finished, lock_run};

    #[test]
    fn writes_each_unit_to_its_own_file() {
        let root = rskit_fs::TempDir::new().unwrap();
        let mut logs = UnitLogs::create(root.path(), "run-1", 5).unwrap();
        logs.append("go~build", b"one\n").unwrap();
        logs.append("go~build", b"two\n").unwrap();
        let path = logs.close("go~build").unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n");
        assert!(path.starts_with(logs.dir()));
        assert_eq!(logs.close("silent").unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn log_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = rskit_fs::TempDir::new().unwrap();
        let mut logs = UnitLogs::create(root.path(), "run-1", 5).unwrap();
        logs.append("u", b"secret\n").unwrap();
        let path = logs.close("u").unwrap().unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let dir_mode = std::fs::metadata(logs.dir()).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700);
    }

    #[test]
    fn same_second_runs_get_distinct_directories() {
        let root = rskit_fs::TempDir::new().unwrap();
        let first = UnitLogs::create(root.path(), "run-7", 5).unwrap();
        let second = UnitLogs::create(root.path(), "run-7", 5).unwrap();
        assert_ne!(first.dir(), second.dir());
    }

    #[test]
    fn pruning_preserves_a_live_runs_closed_and_future_logs() {
        let root = rskit_fs::TempDir::new().unwrap();
        let mut live = UnitLogs::create(root.path(), "run-1", 1).unwrap();
        live.append("first", b"retained\n").unwrap();
        let first = live.close("first").unwrap().unwrap();
        for id in ["run-2", "run-3"] {
            UnitLogs::create(root.path(), id, 1).unwrap();
        }
        assert_eq!(std::fs::read_to_string(first).unwrap(), "retained\n");
        live.append("later", b"still live\n").unwrap();
        let later = live.close("later").unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(later).unwrap(), "still live\n");
        let old = live.dir().to_path_buf();
        drop(live);
        UnitLogs::create(root.path(), "run-4", 1).unwrap();
        assert!(!old.exists(), "released runs become eligible for pruning");
    }

    #[test]
    fn pruning_preserves_a_live_run_even_with_a_lexically_older_new_run() {
        let root = rskit_fs::TempDir::new().unwrap();
        let live = UnitLogs::create(root.path(), "run-9", 1).unwrap();
        let mut older = UnitLogs::create(root.path(), "run-1", 1).unwrap();
        older.append("u", b"older\n").unwrap();
        assert!(live.dir().exists());
        assert!(older.dir().exists());
    }

    // Windows cannot delete a directory that holds the run's open lock file.
    #[cfg(unix)]
    #[test]
    fn a_vanished_run_directory_is_recreated_on_the_next_write() {
        let root = rskit_fs::TempDir::new().unwrap();
        let mut logs = UnitLogs::create(root.path(), "run-1", 5).unwrap();
        std::fs::remove_dir_all(logs.dir()).unwrap();
        logs.append("u", b"after removal\n").unwrap();
        let path = logs.close("u").unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "after removal\n");
        UnitLogs::create(root.path(), "run-0", 1).unwrap();
        assert!(logs.dir().exists(), "the recreated run is locked again");
    }

    #[test]
    fn only_finished_runs_are_eligible_for_removal() {
        let root = rskit_fs::TempDir::new().unwrap();
        let live = UnitLogs::create(root.path(), "run-1", 5).unwrap();
        let dir = live.dir().to_path_buf();
        assert!(!is_finished(&dir).unwrap(), "a live run is never removed");
        drop(live);
        assert!(is_finished(&dir).unwrap());
        assert!(
            lock_run(&dir).is_ok(),
            "checking does not keep the run locked"
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(!is_finished(&dir).unwrap(), "a removed run is skipped");
    }

    #[test]
    fn a_held_root_lock_bounds_how_long_a_new_run_waits() {
        let root = rskit_fs::TempDir::new().unwrap();
        let held = RootLock::acquire(root.path()).unwrap();
        let error = UnitLogs::create(root.path(), "run-1", 5)
            .err()
            .expect("a new run cannot start while the root stays locked");
        assert!(error.to_string().contains("stayed locked"), "{error}");
        held.release().unwrap();
        assert!(UnitLogs::create(root.path(), "run-1", 5).is_ok());
    }

    #[test]
    fn prunes_old_runs_beyond_the_keep_limit() {
        let root = rskit_fs::TempDir::new().unwrap();
        for id in ["run-1", "run-2", "run-3"] {
            UnitLogs::create(root.path(), id, 2).unwrap();
        }
        std::fs::write(root.path().join("keep.txt"), b"x").unwrap();
        let mut names: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, [".lock", "keep.txt", "run-2", "run-3"]);
    }

    #[test]
    fn a_rerun_unit_keeps_only_its_latest_attempt() {
        let root = rskit_fs::TempDir::new().unwrap();
        let mut logs = UnitLogs::create(root.path(), "run-1", 5).unwrap();
        logs.append("u", b"first attempt\n").unwrap();
        logs.close("u").unwrap();
        logs.append("u", b"second\n").unwrap();
        let path = logs.close("u").unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "second\n");
    }

    #[test]
    fn unit_ids_map_to_safe_distinct_names() {
        let a = file_name("go/a~build");
        let b = file_name("go_a~build");
        assert_ne!(a, b);
        assert!(!a.contains('/'));
        assert_eq!(std::path::Path::new(&a).extension(), Some("log".as_ref()));
    }
}
