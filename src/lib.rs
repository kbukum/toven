//! Library entrypoint for Toven.
//!
//! The public API exposes the CLI entrypoint and core planning contracts used
//! by upcoming discovery, scheduling, and rendering work.

pub mod cli;
pub mod config;
pub mod core;
pub mod engine;
pub mod exec;
pub mod lang;
pub mod preset;
pub mod report;

/// Current package version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Current package version metadata using the shared rskit version shape.
#[must_use]
pub fn version_info() -> rskit_version::VersionInfo {
    rskit_version::VersionInfo {
        version: VERSION.to_string(),
        git_commit: String::new(),
        git_branch: String::new(),
        build_time: String::new(),
        rust_version: String::new(),
        is_release: VERSION != "dev" && !VERSION.contains("dirty"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_info_reports_toven_package_version() {
        assert_eq!(crate::version_info().package_version(), crate::VERSION);
    }
}
