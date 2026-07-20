//! Read emitted coverage profiles from a directory.
//!
//! The coverage task writes its profile(s) into a Toven-owned staging directory
//! (the user's argv chooses the output path; Toven reads whatever landed
//! there), keeping the runner flags in argv and the aggregation in Toven. Every
//! regular file in the directory is read (bounded) and parsed by
//! content-detected format ([`CoverageFormat::detect`]), so a directory may mix
//! an lcov tracefile and a Go coverprofile from a federated run.

use std::path::Path;

use rskit_errors::{AppError, AppResult};
use rskit_fs::sync_io::dir;
use rskit_fs::sync_io::file::read_string_bounded;

use super::profile::{CoverageFormat, CoverageProfile};
use super::{goprofile, lcov};

/// The Toven-owned directory a coverage task stages its profiles in, relative
/// to the project root. The coverage task's argv writes profiles here; Toven
/// reads and aggregates them.
pub const COVERAGE_DIR: &str = "target/toven/coverage";

/// The maximum profile file size read, bounding untrusted input (64 MiB).
const MAX_PROFILE_BYTES: u64 = 64 * 1024 * 1024;

/// Read and parse every coverage profile in `dir_path`.
///
/// # Errors
/// Propagates a missing or empty profile directory, a directory-listing failure,
/// a bounded-read failure, or a profile parse error.
pub(super) fn read_profiles(dir_path: &Path) -> AppResult<Vec<CoverageProfile>> {
    if !dir::exists(dir_path)? {
        return Err(no_profiles_error(dir_path));
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
    if profiles.is_empty() {
        return Err(no_profiles_error(dir_path));
    }
    Ok(profiles)
}

fn no_profiles_error(dir_path: &Path) -> AppError {
    AppError::invalid_input(
        "coverage profiles",
        format!(
            "no coverage profiles were emitted under {}",
            dir_path.display()
        ),
    )
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
    fn missing_directory_is_a_measurement_failure() {
        let temp = TempDir::new().expect("temp dir");
        let error =
            read_profiles(&temp.path().join("absent")).expect_err("missing profile rejected");
        assert!(
            error.to_string().contains("no coverage profiles"),
            "{error}"
        );
    }

    #[test]
    fn empty_directory_is_a_measurement_failure() {
        let temp = TempDir::new().expect("temp dir");
        let dir = temp.path().join("coverage");
        create_all(&dir).expect("mkdir");
        let error = read_profiles(&dir).expect_err("empty profile rejected");
        assert!(
            error.to_string().contains("no coverage profiles"),
            "{error}"
        );
    }
}
