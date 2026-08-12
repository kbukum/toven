//! Provenance flow and published subject collection.

use std::collections::BTreeMap;
use std::path::Path;

use rskit_errors::{AppError, AppResult};
use rskit_fs::safe_join;
use rskit_fs::sync_io::file::read_string_bounded;
use toven_core::config::Document;
use toven_core::federation::resolve::PathDriverLocator;
use toven_core::plan::{PlanRequest, prepare_front};
use toven_ports::{ProvenancePhase, ProvenanceSubject, Provider, Reporter};

use crate::artifacts::{assets, image};
use crate::planning::plan::{release_targets, resolve_release_settings};

/// The canonical file name of the checksum manifest whose entries are the
/// provenance subjects. A declared asset whose file name is exactly this is the
/// manifest; one beginning `SHA256SUMS.` is a signature sidecar, not a subject.
const MANIFEST_NAME: &str = "SHA256SUMS";

/// Hard bound on the untrusted `SHA256SUMS` manifest read (1 MiB) — a checksum
/// line is ~80 bytes, so this admits thousands of assets while rejecting a
/// pathological file.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// The resolved status of the provenance phase over the release scope.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProvenancePhaseStatus {
    /// An enforced run: every published subject carries an attestation.
    Verified,
    /// `--dry-run`: every subject already carries an attestation.
    Present,
    /// `--dry-run`: at least one subject lacks an attestation.
    Missing,
}

impl ProvenancePhaseStatus {
    /// Canonical wire/report name for the status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Present => "present",
            Self::Missing => "missing",
        }
    }
}

/// A single subject's provenance result within a [`ProvenanceReport`].
///
/// Each subject carries its own resolved [`ProvenancePhaseStatus`] so that a
/// `--dry-run` preview reports `present`/`missing` **per subject** — an attested
/// subject is never masked by an unattested sibling. In an enforced run every
/// listed subject is `Verified` (the phase fails closed before producing a
/// report when any subject is missing).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProvenanceSubjectReport {
    /// The published subject this result covers.
    pub subject: ProvenanceSubject,
    /// This subject's resolved status.
    pub status: ProvenancePhaseStatus,
}

/// A read-only projection of the provenance phase over the release scope.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProvenanceReport {
    /// Whether this report is a mutation-free `--dry-run` preview.
    pub preview: bool,
    /// The per-subject results, in manifest order.
    pub subjects: Vec<ProvenanceSubjectReport>,
    /// The aggregate status of the phase.
    pub status: ProvenancePhaseStatus,
}

/// Options controlling the provenance phase.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProvenanceOptions {
    /// Preview the phase in report-only mode: report whether an attestation
    /// exists for each subject without failing when one is missing.
    pub dry_run: bool,
}

/// Verify SLSA provenance over exactly the published subjects.
///
/// The subjects are the entries of the declared `SHA256SUMS` manifest plus the
/// live digest of every pushed image reference. Toven never cuts the
/// attestation itself (the CI trusted builder does); this asserts that every
/// published subject carries one.
///
/// With `options.dry_run`, the phase is a report-only preview: it reports
/// whether an attestation exists for each subject but never fails on a missing
/// one. Otherwise it verifies every subject and fails closed if any is missing.
///
/// # Errors
/// Fails closed with a typed error when neither a `SHA256SUMS` manifest nor a
/// published image is available, the manifest file is missing or malformed,
/// nothing resolves to a subject, or (outside `--dry-run`) any subject lacks an
/// attestation — and propagates configuration/discovery/graph failures and
/// attestation-tool failures.
pub fn release_provenance(
    request: &PlanRequest,
    document: &Document,
    providers: &[&dyn Provider],
    provenance_phase: &dyn ProvenancePhase,
    image_phase: &dyn toven_ports::ImagePhase,
    options: ProvenanceOptions,
    reporter: &mut dyn Reporter,
) -> AppResult<ProvenanceReport> {
    let locator = PathDriverLocator::new();
    let context = prepare_front(
        &request.project_root,
        document,
        providers,
        &locator,
        reporter,
    )?;
    let targets = release_targets(&context)?;
    let settings = resolve_release_settings(&context, &targets)?;
    let project_root = request.project_root.as_path();

    let image_requests = image::resolved_image_requests(&context, &targets, &settings)?;
    let subjects = published_subjects(project_root, &settings, &image_requests, image_phase)?;

    let (subject_reports, status) = if options.dry_run {
        let mut reports = Vec::with_capacity(subjects.len());
        let mut all_present = true;
        for subject in subjects {
            let present = provenance_phase.attestation_exists(project_root, &subject)?;
            if !present {
                all_present = false;
            }
            let status = if present {
                ProvenancePhaseStatus::Present
            } else {
                ProvenancePhaseStatus::Missing
            };
            reports.push(ProvenanceSubjectReport { subject, status });
        }
        let aggregate = if all_present {
            ProvenancePhaseStatus::Present
        } else {
            ProvenancePhaseStatus::Missing
        };
        (reports, aggregate)
    } else {
        provenance_phase.verify(project_root, &subjects)?;
        let reports = subjects
            .into_iter()
            .map(|subject| ProvenanceSubjectReport {
                subject,
                status: ProvenancePhaseStatus::Verified,
            })
            .collect();
        (reports, ProvenancePhaseStatus::Verified)
    };

    Ok(ProvenanceReport {
        preview: options.dry_run,
        subjects: subject_reports,
        status,
    })
}

/// Collect the published subjects: the entries of the declared `SHA256SUMS`
/// manifest (each a `sha256:`-prefixed subject, in manifest order) plus the live
/// digest of every published image reference. Provenance attests exactly what
/// was actually published, so an image that has not been pushed contributes no
/// subject.
///
/// # Errors
/// Fails closed when neither a manifest nor an image block is declared, when a
/// declared manifest is missing/malformed or lists no subjects, or when nothing
/// resolves to a subject at all.
fn published_subjects(
    project_root: &Path,
    settings: &BTreeMap<toven_model::ModuleKey, crate::ResolvedReleaseSettings>,
    image_requests: &[(String, toven_ports::ImageRequest)],
    image_phase: &dyn toven_ports::ImagePhase,
) -> AppResult<Vec<ProvenanceSubject>> {
    let declared = assets::declared_release_assets(settings);
    let manifest = declared
        .iter()
        .find(|asset| asset_file_name(asset) == Some(MANIFEST_NAME))
        .copied();

    let mut subjects = Vec::new();
    if let Some(manifest) = manifest {
        let path = safe_join(project_root, manifest).map_err(|error| {
            AppError::invalid_input(
                "release.host.assets",
                format!("asset '{manifest}' is not a safe project-relative path"),
            )
            .with_cause(error)
        })?;
        let manifest_subjects = parse_manifest_subjects(manifest, &path)?;
        if manifest_subjects.is_empty() {
            return Err(AppError::invalid_input(
                "release.provenance.subjects",
                format!("manifest '{manifest}' lists no subjects to verify"),
            ));
        }
        subjects.extend(manifest_subjects);
    } else if image_requests.is_empty() {
        return Err(AppError::invalid_input(
            "release.provenance.subjects",
            format!(
                "no '{MANIFEST_NAME}' manifest and no image are declared; provenance verifies \
                 exactly the published subjects (manifest entries and pushed image digests)"
            ),
        ));
    }

    subjects.extend(image_subjects(project_root, image_requests, image_phase)?);

    if subjects.is_empty() {
        return Err(AppError::invalid_input(
            "release.provenance.subjects",
            "nothing published to verify: the declared image references resolve to no pushed \
             digest — run `toven release image` first",
        ));
    }
    Ok(subjects)
}

/// The live digest of each published primary image reference, as a provenance
/// subject. A primary reference the registry does not yet resolve (the image was
/// not pushed) contributes nothing — provenance attests only what was actually
/// published by the authoritative registry. Mirrors never substitute for a
/// missing primary.
fn image_subjects(
    project_root: &Path,
    image_requests: &[(String, toven_ports::ImageRequest)],
    image_phase: &dyn toven_ports::ImagePhase,
) -> AppResult<Vec<ProvenanceSubject>> {
    let mut subjects = Vec::new();
    for (_module, request) in image_requests {
        let Some(reference) = request.references().into_iter().next() else {
            continue;
        };
        if let Some(digest) = image_phase.resolve_digest(project_root, &reference)? {
            subjects.push(ProvenanceSubject::image(reference, digest));
        }
    }
    Ok(subjects)
}

/// Parse a `SHA256SUMS` body (`shasum -a 256` two-space format) into provenance
/// subjects, `sha256:`-prefixing each lowercase-hex digest. Each subject is
/// located for verification by its project-relative path — the entry's name
/// joined onto the manifest's own directory (`manifest_rel`), since a manifest
/// entry names a file sitting beside the manifest.
fn parse_manifest_subjects(manifest_rel: &str, path: &Path) -> AppResult<Vec<ProvenanceSubject>> {
    let text = read_string_bounded(path, MAX_MANIFEST_BYTES)?;
    let mut subjects = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (hex, name) = line.split_once("  ").ok_or_else(|| {
            AppError::invalid_input(
                "release.provenance.manifest",
                format!("malformed manifest line '{line}' (expected '<hex>  <name>')"),
            )
        })?;
        validate_manifest_entry(line, hex, name)?;
        let subject_path = subject_file_path(manifest_rel, name);
        subjects.push(ProvenanceSubject::file(
            name,
            format!("sha256:{hex}"),
            subject_path,
        ));
    }
    Ok(subjects)
}

/// The project-relative path of a manifest entry: its `name` joined onto the
/// manifest's own directory. A manifest at `dist/SHA256SUMS` with entry
/// `toven.tar.gz` yields `dist/toven.tar.gz`; a manifest at the project root
/// yields the bare name. Forward slashes are used so the path is a stable,
/// platform-independent `gh attestation verify` argument.
pub(super) fn subject_file_path(manifest_rel: &str, name: &str) -> String {
    Path::new(manifest_rel)
        .parent()
        .and_then(Path::to_str)
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| name.to_string(), |dir| format!("{dir}/{name}"))
}

/// Reject a manifest entry whose digest is not a 64-char lowercase-hex sha256,
/// or whose name is not a safe bare file name sitting beside the manifest. The
/// name flows into the `gh attestation verify` argv as the subject's
/// project-relative path (joined onto the manifest directory) *and* is hashed
/// off disk, so a name that is empty, could be mistaken for a flag (a leading
/// `-`), or escapes the manifest directory (a path separator, `.`, or `..`)
/// would let an untrusted manifest read or verify a file outside the release
/// directory. It is validated at this trust boundary; Toven's own manifests
/// list bare file names.
fn validate_manifest_entry(line: &str, hex: &str, name: &str) -> AppResult<()> {
    let is_lower_hex = hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
    if !is_lower_hex {
        return Err(AppError::invalid_input(
            "release.provenance.manifest",
            format!(
                "malformed manifest line '{line}' (expected a 64-char lowercase-hex sha256 digest)"
            ),
        ));
    }
    if name.is_empty() || name.starts_with('-') {
        return Err(AppError::invalid_input(
            "release.provenance.manifest",
            format!(
                "malformed manifest line '{line}' (subject name must be non-empty and not begin \
                 with '-')"
            ),
        ));
    }
    if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err(AppError::invalid_input(
            "release.provenance.manifest",
            format!(
                "unsafe manifest entry '{name}': a subject name must be a bare file name beside \
                 the manifest, with no path separators or '..'"
            ),
        ));
    }
    Ok(())
}

/// The final path component of a declared asset path.
fn asset_file_name(asset: &str) -> Option<&str> {
    Path::new(asset).file_name().and_then(|name| name.to_str())
}
