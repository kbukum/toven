//! `cargo cyclonedx` SBOMs: produce one module's SBOM into the output
//! directory and remove the copies cyclonedx leaves next to sibling members.

use std::path::{Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::canonicalize;
use rskit_fs::sync_io::dir::create_all;
use rskit_fs::sync_io::file::{exists, move_file, remove_if_exists};
use toven_model::Module;
use toven_ports::{Artifact, SbomProducer};

use super::target::{CargoRegistryTarget, package_name};

impl CargoRegistryTarget {
    fn remove_sbom_strays(
        &self,
        manifest: &Path,
        stem: &str,
        preserve: Option<&Path>,
    ) -> AppResult<()> {
        remove_stray_sbom_files(&self.workspace_member_dirs(manifest)?, stem, preserve)
    }

    /// The directory of each workspace member manifest, resolved via
    /// `cargo metadata --no-deps` (which reports members only).
    fn workspace_member_dirs(&self, manifest: &Path) -> AppResult<Vec<PathBuf>> {
        let metadata = self.cargo_metadata(manifest)?;
        let workspace_root = metadata.workspace_root.as_std_path();
        Ok(metadata
            .packages
            .iter()
            .filter(|package| metadata.workspace_members.contains(&package.id))
            .filter_map(|package| package.manifest_path.parent())
            .filter(|dir| dir.starts_with(workspace_root))
            .map(|dir| dir.as_std_path().to_path_buf())
            .collect())
    }
}

impl SbomProducer for CargoRegistryTarget {
    fn sbom(&self, module: &Module, out_dir: &Path) -> AppResult<Option<Artifact>> {
        let manifest = Self::manifest_path(module)?;
        create_all(out_dir)?;
        let stem = package_name(module);
        let output = self.cargo(Self::working_root()?, sbom_argv(&manifest, &stem))?;
        output.require_success("sbom tool `cargo`")?;
        // `cargo cyclonedx` ignores the process working directory and writes its
        // output next to the manifest; the filename suffix is version-specific (0.5.x
        // writes `<stem>.json`). Locate whichever file it produced, then move it into
        // `out_dir` under Toven's canonical `<stem>.cdx.json` name so the manifest tree
        // is left clean and callers get a stable artifact path.
        let produced =
            first_existing(&sbom_output_candidates(&manifest, &stem))?.ok_or_else(|| {
                AppError::new(
                    ErrorCode::Internal,
                    format!(
                        "cargo cyclonedx reported success but wrote no SBOM next to '{}'",
                        manifest.display()
                    ),
                )
            })?;
        let artifact = out_dir.join(format!("{stem}.{SBOM_FILE_SUFFIX}"));
        move_file(&produced, &artifact)?;
        // `cargo cyclonedx` resolves the whole workspace and writes a copy of the
        // SBOM next to *every* member manifest, not just the requested one. Remove
        // those sibling copies so the manifest tree is left clean and only the
        // artifact under `out_dir` remains.
        self.remove_sbom_strays(&manifest, &stem, Some(&artifact))?;
        Ok(Some(Artifact::new(artifact)))
    }
}

/// `CycloneDX` SBOM output suffix for the JSON format (`<stem>.cdx.json`).
const SBOM_FILE_SUFFIX: &str = "cdx.json";

/// Candidate on-disk paths `cargo cyclonedx` may write for `stem`, in priority
/// order, resolved next to `manifest` (the tool ignores the process working
/// directory). The suffix is version-specific — 0.5.x writes `<stem>.json`,
/// while other versions emit `<stem>.cdx.json` — so both are probed.
fn sbom_output_candidates(manifest: &Path, stem: &str) -> Vec<PathBuf> {
    let dir = manifest.parent().unwrap_or_else(|| Path::new("."));
    [SBOM_FILE_SUFFIX, "json"]
        .into_iter()
        .map(|suffix| dir.join(format!("{stem}.{suffix}")))
        .collect()
}

/// Return the first path in `candidates` that exists on disk, or `None`.
fn first_existing(candidates: &[PathBuf]) -> AppResult<Option<PathBuf>> {
    for candidate in candidates {
        if exists(candidate)? {
            return Ok(Some(candidate.clone()));
        }
    }
    Ok(None)
}

/// Remove the stray `<stem>` SBOM files `cargo cyclonedx` wrote next to sibling
/// workspace members. The requested module's own copy has already been moved
/// into `out_dir`, so every remaining `<stem>.{cdx.json,json}` under a member
/// directory is a redundant sibling copy safe to delete.
/// Delete every `<stem>.{cdx.json,json}` file found directly in `dirs`.
fn remove_stray_sbom_files(dirs: &[PathBuf], stem: &str, preserve: Option<&Path>) -> AppResult<()> {
    for dir in dirs {
        for suffix in [SBOM_FILE_SUFFIX, "json"] {
            let candidate = dir.join(format!("{stem}.{suffix}"));
            if should_remove_sbom(&candidate, preserve)? {
                remove_if_exists(&candidate)?;
            }
        }
    }
    Ok(())
}

fn should_remove_sbom(candidate: &Path, preserve: Option<&Path>) -> AppResult<bool> {
    if !exists(candidate)? {
        return Ok(false);
    }
    let Some(preserve) = preserve else {
        return Ok(true);
    };
    Ok(canonicalize(candidate)? != canonicalize(preserve)?)
}

/// Build the argv-only `cargo cyclonedx` invocation for `manifest`.
///
/// cyclonedx ignores the process working directory and writes next to each
/// member manifest, so `--override-filename` pins the output stem
/// deterministically instead of relying on the crate name.
fn sbom_argv(manifest: &Path, stem: &str) -> Vec<String> {
    vec![
        "cyclonedx".to_string(),
        "--manifest-path".to_string(),
        manifest.display().to_string(),
        "--format".to_string(),
        "json".to_string(),
        "--override-filename".to_string(),
        stem.to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use rskit_fs::sync_io::dir::create_all;

    use super::{remove_stray_sbom_files, sbom_argv, sbom_output_candidates};

    #[test]
    fn sbom_argv_is_an_argv_only_cyclonedx_invocation() {
        let manifest = Path::new("/repo/crates/core/Cargo.toml");
        assert_eq!(
            sbom_argv(manifest, "core"),
            vec![
                "cyclonedx".to_string(),
                "--manifest-path".to_string(),
                "/repo/crates/core/Cargo.toml".to_string(),
                "--format".to_string(),
                "json".to_string(),
                "--override-filename".to_string(),
                "core".to_string(),
            ]
        );
    }

    #[test]
    fn sbom_output_candidates_probe_both_suffixes_next_to_the_manifest() {
        let manifest = Path::new("/repo/crates/core/Cargo.toml");
        assert_eq!(
            sbom_output_candidates(manifest, "core"),
            vec![
                PathBuf::from("/repo/crates/core/core.cdx.json"),
                PathBuf::from("/repo/crates/core/core.json"),
            ]
        );
    }

    #[test]
    fn remove_stray_sbom_files_deletes_only_the_stem_copies() {
        use rskit_fs::TempDir;
        use rskit_fs::sync_io::file::exists;

        let root = TempDir::new().expect("temp dir");
        let member_a = root.path().join("crates/a");
        let member_b = root.path().join("crates/b");
        create_all(&member_a).expect("member a");
        create_all(&member_b).expect("member b");

        // cargo cyclonedx scattered `core.json` next to each member; an unrelated
        // committed file with a different stem must survive.
        for dir in [&member_a, &member_b] {
            std::fs::write(dir.join("core.json"), b"{}").expect("stray");
            std::fs::write(dir.join("keep.json"), b"{}").expect("keep");
        }
        std::fs::write(member_a.join("core.cdx.json"), b"{}").expect("stray cdx");

        remove_stray_sbom_files(&[member_a.clone(), member_b.clone()], "core", None)
            .expect("cleanup");

        assert!(!exists(&member_a.join("core.json")).expect("a json"));
        assert!(!exists(&member_a.join("core.cdx.json")).expect("a cdx"));
        assert!(!exists(&member_b.join("core.json")).expect("b json"));
        assert!(exists(&member_a.join("keep.json")).expect("keep a"));
        assert!(exists(&member_b.join("keep.json")).expect("keep b"));
    }

    #[test]
    fn remove_stray_sbom_files_preserves_the_final_artifact() {
        use rskit_fs::TempDir;
        use rskit_fs::sync_io::file::exists;

        let root = TempDir::new().expect("temp dir");
        let member = root.path().join("crates/core");
        let output = member.join("core.cdx.json");
        create_all(&member).expect("member");
        std::fs::write(&output, b"{}").expect("artifact");

        remove_stray_sbom_files(std::slice::from_ref(&member), "core", Some(&output))
            .expect("cleanup");

        assert!(exists(&output).expect("artifact exists"));
    }
}
