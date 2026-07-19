//! Read emitted coverage profiles from a directory.
//!
//! The coverage task writes its profile(s) into a Toven-owned staging directory
//! (the user's argv chooses the output path; Toven reads whatever landed there),
//! keeping the runner flags in argv and the aggregation in Toven. Every regular
//! file in the directory is read (bounded) and parsed by content-detected format
//! ([`CoverageFormat::detect`]), so a directory may mix an lcov tracefile and a
//! Go coverprofile from a federated run.

use std::path::Path;

use rskit_errors::AppResult;
use rskit_fs::sync_io::dir;
use rskit_fs::sync_io::file::read_string_bounded;

use super::profile::{CoverageFormat, CoverageProfile};
use super::{goprofile, lcov};

/// The Toven-owned directory a coverage task stages its profiles in, relative to
/// the project root. The coverage task's argv writes profiles here; Toven reads
/// and aggregates them.
pub const COVERAGE_DIR: &str = "target/toven/coverage";

/// The maximum profile file size read, bounding untrusted input (64 MiB).
const MAX_PROFILE_BYTES: u64 = 64 * 1024 * 1024;

/// Read and parse every coverage profile in `dir_path`.
///
/// Returns an empty vector when the directory does not exist (a run that emitted
/// nothing is a measured-nothing report, not an error).
///
/// # Errors
/// Propagates a directory-listing failure, a bounded-read failure, or a profile
/// parse error.
pub(super) fn read_profiles(dir_path: &Path) -> AppResult<Vec<CoverageProfile>> {
    if !dir::exists(dir_path)? {
        return Ok(Vec::new());
    }
    let mut profiles = Vec::new();
    for entry in dir::list(dir_path)? {
        if !entry.is_file {
            continue;
        }
        let contents = read_string_bounded(&entry.path, MAX_PROFILE_BYTES)?;
        let profile = match CoverageFormat::detect(&contents) {
            CoverageFormat::Lcov => lcov::parse(&contents)?,
            CoverageFormat::GoProfile => goprofile::parse(&contents)?,
        };
        profiles.push(profile);
    }
    Ok(profiles)
}

#[cfg(test)]
mod tests {
    use super::read_profiles;
    use rskit_fs::TempDir;
    use rskit_fs::sync_io::dir::create_all;
    use rskit_fs::sync_io::file::write_atomic_replace;

    #[test]
    fn reads_and_detects_mixed_profiles() {
        let temp = TempDir::new().expect("temp dir");
        let dir = temp.path().join("coverage");
        create_all(&dir).expect("mkdir");
        // Stage the shared lcov and Go coverprofile fixtures through the real
        // read/detect/parse path, so the fixtures exercise the feature code.
        let lcov = toven_testkit::coverage_profile_string("rust.lcov").expect("lcov fixture");
        let go = toven_testkit::coverage_profile_string("go.out").expect("go fixture");
        write_atomic_replace(&dir.join("rust.lcov"), lcov.as_bytes(), "read-test")
            .expect("write lcov");
        write_atomic_replace(&dir.join("go.out"), go.as_bytes(), "read-test").expect("write go");

        let profiles = read_profiles(&dir).expect("reads");
        assert_eq!(profiles.len(), 2);
        assert!(
            profiles.iter().any(|profile| !profile.files.is_empty()),
            "fixtures parse into per-file coverage"
        );
    }

    #[test]
    fn missing_directory_is_an_empty_report() {
        let temp = TempDir::new().expect("temp dir");
        let profiles = read_profiles(&temp.path().join("absent")).expect("reads");
        assert!(profiles.is_empty());
    }
}
