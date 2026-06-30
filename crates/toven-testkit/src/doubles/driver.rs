//! Shared driver-port doubles: [`FakeDriverLocator`] and [`FakeDriverScaffolder`].
//!
//! These substitute the out-of-process driver seams so federation/generation
//! tests stay deterministic without touching the real `PATH` or spawning a
//! subprocess. The locator resolves only the names it was seeded with (and can
//! be told to fail a lookup, modeling a filesystem error); the scaffolder
//! returns canned fragments keyed by the probed program path.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{DriverLocator, DriverScaffolder, EcosystemFragment};

/// A [`DriverLocator`] that resolves only the names it was seeded with.
#[derive(Debug, Default, Clone)]
pub struct FakeDriverLocator {
    resolved: BTreeMap<String, PathBuf>,
    failing: BTreeSet<String>,
}

impl FakeDriverLocator {
    /// Construct a locator that resolves nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve `name` to `path`.
    #[must_use]
    pub fn with_driver(mut self, name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        self.resolved.insert(name.into(), path.into());
        self
    }

    /// Resolve `name` to a conventional `/usr/bin/<name>` path.
    #[must_use]
    pub fn with_conventional(self, name: impl Into<String>) -> Self {
        let name = name.into();
        let path = PathBuf::from(format!("/usr/bin/{name}"));
        self.with_driver(name, path)
    }

    /// Make a lookup of `name` fail, modeling a filesystem inspection error.
    #[must_use]
    pub fn with_failing(mut self, name: impl Into<String>) -> Self {
        self.failing.insert(name.into());
        self
    }
}

impl DriverLocator for FakeDriverLocator {
    fn locate(&self, binary_name: &str) -> AppResult<Option<PathBuf>> {
        if self.failing.contains(binary_name) {
            return Err(AppError::new(
                ErrorCode::Internal,
                format!("driver locate failed for '{binary_name}'"),
            ));
        }
        Ok(self.resolved.get(binary_name).cloned())
    }
}

/// A [`DriverScaffolder`] that returns canned fragments keyed by program path.
#[derive(Debug, Default, Clone)]
pub struct FakeDriverScaffolder {
    fragments: BTreeMap<PathBuf, Vec<EcosystemFragment>>,
}

impl FakeDriverScaffolder {
    /// Construct a scaffolder that returns nothing for any program.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Make probing `program` return `fragments`.
    #[must_use]
    pub fn with_fragments(
        mut self,
        program: impl Into<PathBuf>,
        fragments: Vec<EcosystemFragment>,
    ) -> Self {
        self.fragments.insert(program.into(), fragments);
        self
    }
}

impl DriverScaffolder for FakeDriverScaffolder {
    fn scaffold(&self, program: &Path, _project_root: &Path) -> AppResult<Vec<EcosystemFragment>> {
        Ok(self.fragments.get(program).cloned().unwrap_or_default())
    }
}
