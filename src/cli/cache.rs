//! `toven cache` commands.

use std::{
    fs, io,
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

use clap::ArgMatches;

use crate::{
    cache::path::resolve_task_cache_root,
    config::load_workspace,
    core::{AppError, AppResult},
};

pub(super) fn run_cache(matches: &ArgMatches, stdout: &mut impl Write) -> AppResult<()> {
    match matches.subcommand() {
        Some(("stats" | "info", matches)) => run_cache_stats(matches, stdout),
        Some(("clean" | "clear", matches)) => run_cache_clean(matches, stdout),
        Some(("path", matches)) => run_cache_path(matches, stdout),
        _ => Err(AppError::invalid_input(
            "cache",
            "cache subcommand is required",
        )),
    }
}

fn run_cache_stats(matches: &ArgMatches, stdout: &mut impl Write) -> AppResult<()> {
    let cache_dir = cache_dir(matches)?;
    let stats = collect_cache_stats(&cache_dir)?;

    writeln!(stdout, "cache_dir: {}", cache_dir.display()).map_err(AppError::internal)?;
    writeln!(stdout, "entries: {}", stats.entries).map_err(AppError::internal)?;
    writeln!(stdout, "bytes: {}", stats.bytes).map_err(AppError::internal)?;
    writeln!(
        stdout,
        "oldest_age_seconds: {}",
        stats
            .oldest_age_seconds()
            .map_or_else(|| "n/a".to_string(), |age| age.to_string())
    )
    .map_err(AppError::internal)?;
    writeln!(
        stdout,
        "newest_age_seconds: {}",
        stats
            .newest_age_seconds()
            .map_or_else(|| "n/a".to_string(), |age| age.to_string())
    )
    .map_err(AppError::internal)?;
    writeln!(stdout, "hit_rate: per-run only").map_err(AppError::internal)?;
    Ok(())
}

fn run_cache_clean(matches: &ArgMatches, stdout: &mut impl Write) -> AppResult<()> {
    let cache_dir = cache_dir(matches)?;
    let stats = collect_cache_stats(&cache_dir)?;

    match fs::remove_dir_all(&cache_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(AppError::new(
                crate::core::ErrorCode::Internal,
                format!("failed to remove cache directory '{}'", cache_dir.display()),
            )
            .with_cause(error));
        }
    }

    writeln!(
        stdout,
        "removed {} entries ({} bytes)",
        stats.entries, stats.bytes
    )
    .map_err(AppError::internal)
}

fn run_cache_path(matches: &ArgMatches, stdout: &mut impl Write) -> AppResult<()> {
    let cache_dir = cache_dir(matches)?;
    writeln!(stdout, "cache_dir: {}", cache_dir.display()).map_err(AppError::internal)
}

fn cache_dir(matches: &ArgMatches) -> AppResult<PathBuf> {
    let config = PathBuf::from(
        matches
            .get_one::<String>("config")
            .expect("clap supplies the cache config default"),
    );
    load_workspace(config).and_then(|workspace| resolve_task_cache_root(&workspace))
}

fn collect_cache_stats(path: &Path) -> AppResult<CacheStats> {
    let mut stats = CacheStats::default();
    visit_cache_dir(path, &mut stats)?;
    Ok(stats)
}

fn visit_cache_dir(path: &Path, stats: &mut CacheStats) -> AppResult<()> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(AppError::new(
                crate::core::ErrorCode::Internal,
                format!("failed to read cache directory '{}'", path.display()),
            )
            .with_cause(error));
        }
    };

    for entry in entries {
        let entry = entry.map_err(|error| {
            AppError::new(
                crate::core::ErrorCode::Internal,
                "failed to read cache entry",
            )
            .with_cause(error)
        })?;
        let file_type = entry.file_type().map_err(|error| {
            AppError::new(
                crate::core::ErrorCode::Internal,
                format!("failed to inspect cache entry '{}'", entry.path().display()),
            )
            .with_cause(error)
        })?;
        if file_type.is_dir() {
            visit_cache_dir(&entry.path(), stats)?;
        } else if file_type.is_file() {
            let metadata = entry.metadata().map_err(|error| {
                AppError::new(
                    crate::core::ErrorCode::Internal,
                    format!("failed to inspect cache file '{}'", entry.path().display()),
                )
                .with_cause(error)
            })?;
            stats.entries += 1;
            stats.bytes += metadata.len();
            if let Ok(modified) = metadata.modified() {
                stats.record_modified(modified);
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct CacheStats {
    entries: usize,
    bytes: u64,
    oldest: Option<SystemTime>,
    newest: Option<SystemTime>,
}

impl CacheStats {
    fn record_modified(&mut self, modified: SystemTime) {
        self.oldest = Some(self.oldest.map_or(modified, |oldest| oldest.min(modified)));
        self.newest = Some(self.newest.map_or(modified, |newest| newest.max(modified)));
    }

    fn oldest_age_seconds(&self) -> Option<u64> {
        self.oldest.and_then(age_seconds)
    }

    fn newest_age_seconds(&self) -> Option<u64> {
        self.newest.and_then(age_seconds)
    }
}

fn age_seconds(time: SystemTime) -> Option<u64> {
    SystemTime::now()
        .duration_since(time)
        .ok()
        .map(|age| age.as_secs())
}
