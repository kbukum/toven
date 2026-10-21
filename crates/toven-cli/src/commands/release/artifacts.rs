//! The artifact actions: `sbom`, `depgraphs`, `package`, `checksums`,
//! `sign`, `verify`, `image` and `provenance`.

use rskit_cli::ExitCode;
use rskit_errors::{AppError, AppResult};
use std::sync::Arc;
use toven_core::vcs::BaselineFlags;
use toven_exec::{ProcessSupervisor, ProcessToolRunner};
use toven_ports::{Provider, ToolRunner};
use toven_release::{
    BuildxImagePhase, CosignSigner, CosignVerifier, GhAssetDownloader, GhAttestationProvenance,
    ImageOptions, ProcessVersionProbe, ProvenanceOptions, VerifyOptions, checksums_operation,
    depgraph_operation, image_operation, package_operation, provenance_operation, sbom_operation,
    sign_operation, verify_operation,
};

use crate::commands::support::QuietReporter;
use crate::flags::Cli;
use crate::host::{Project, resolve_output};

use super::dispatch::{drive_streamed, engine_jobs, release_request, resolve_out_dir};
use super::lifecycle::require_release_confirmation;
use super::projectors::{
    ChecksumProjector, DepgraphProjector, ImageProjector, PackageProjector, ProvenanceProjector,
    SbomProjector, SignProjector, VerifyProjector,
};

/// `release sbom`: generate a `CycloneDX` SBOM per releasable module under the
/// resolved output directory, mutating nothing outside it.
pub(super) fn sbom(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let out_dir = resolve_out_dir(cli, project);
    let mut reporter = QuietReporter;
    let (operation, units) = sbom_operation(
        &request,
        &project.document,
        providers,
        &readers,
        &out_dir,
        &mut reporter,
    )?;
    // Retain the gathered inputs so the declared hosted assets are staged from
    // the streamed artifacts once the parallel wave settles.
    let inputs = operation.inputs();

    let output = resolve_output(cli.execution.output, &project.document);
    // Independent modules — one bounded-parallel wave.
    let jobs = engine_jobs(cli, project);
    let mut projector = SbomProjector::new(output);
    projector.open();

    let (_summary, cancelled) =
        drive_streamed(&units, operation, jobs, supervisor, &mut projector)?;
    if cancelled {
        return Ok(ExitCode::Cancelled);
    }

    // Assemble the trailing aggregate from the streamed outcomes: stage the
    // declared hosted-release assets, then report staged assets and skips.
    let staged = inputs.stage(&projector.artifacts)?;
    projector.finish(&staged)?;
    Ok(ExitCode::Success)
}

/// `release depgraphs`: render the dependency graph to a DOT artifact under the
/// resolved output directory, mutating nothing outside it.
pub(super) fn depgraphs(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let out_dir = resolve_out_dir(cli, project);
    let mut reporter = QuietReporter;
    let (operation, units) = depgraph_operation(
        &request,
        &project.document,
        providers,
        &out_dir,
        &mut reporter,
    )?;

    let output = resolve_output(cli.execution.output, &project.document);
    let jobs = engine_jobs(cli, project);
    let mut projector = DepgraphProjector::new(output);
    projector.open();

    let (_summary, cancelled) =
        drive_streamed(&units, operation, jobs, supervisor, &mut projector)?;
    if cancelled {
        return Ok(ExitCode::Cancelled);
    }
    Ok(ExitCode::Success)
}

/// `release package`: archive the already-built binary for `--target` into its
/// declared hosted-release asset, mutating no history. `--target` is required;
/// `--binary` overrides the default `target/<triple>/release/<binary>` source.
pub(super) fn package(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let target = cli.target.as_deref().ok_or_else(|| {
        AppError::invalid_input(
            "release.package.target",
            "`toven release package` requires `--target <triple>` (e.g. \
             x86_64-unknown-linux-gnu)",
        )
    })?;
    let mut reporter = QuietReporter;
    let tool_runner = ProcessToolRunner::new().with_supervisor(Arc::clone(supervisor));
    let (operation, units) = package_operation(
        &request,
        &project.document,
        providers,
        &readers,
        target,
        cli.binary.as_deref(),
        &tool_runner,
        &mut reporter,
    )?;
    let inputs = operation.inputs();

    let output = resolve_output(cli.execution.output, &project.document);
    let jobs = engine_jobs(cli, project);
    let mut projector = PackageProjector::new(output, inputs.target().to_string());
    projector.open();

    let (_summary, cancelled) =
        drive_streamed(&units, operation, jobs, supervisor, &mut projector)?;
    if cancelled {
        return Ok(ExitCode::Cancelled);
    }
    Ok(ExitCode::Success)
}

/// `release checksums`: emit the `SHA256SUMS` manifest over the declared
/// release assets to its declared asset path, mutating no history.
pub(super) fn checksums(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let mut reporter = QuietReporter;
    let (operation, units) = checksums_operation(
        &request,
        &project.document,
        providers,
        &readers,
        &mut reporter,
    )?;
    let inputs = operation.inputs();

    let output = resolve_output(cli.execution.output, &project.document);
    let jobs = engine_jobs(cli, project);
    let mut projector = ChecksumProjector::new(output, inputs.manifest().to_string());
    projector.open();

    let (_summary, cancelled) =
        drive_streamed(&units, operation, jobs, supervisor, &mut projector)?;
    if cancelled {
        return Ok(ExitCode::Cancelled);
    }

    let report = inputs.write_manifest(&projector.entries)?;
    projector.finish(&report.manifest);
    Ok(ExitCode::Success)
}

/// `release sign`: sign the declared `SHA256SUMS` manifest into its declared
/// self-contained Sigstore bundle with cosign, mutating no history.
pub(super) fn sign(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let runner: Arc<dyn ToolRunner> =
        Arc::new(ProcessToolRunner::new().with_supervisor(Arc::clone(supervisor)));
    let signer: Arc<dyn toven_ports::Signer> = Arc::new(CosignSigner::new(runner.clone()));
    let mut reporter = QuietReporter;
    let (operation, units) = sign_operation(
        &request,
        &project.document,
        providers,
        &readers,
        signer,
        runner.as_ref(),
        &mut reporter,
    )?;

    let output = resolve_output(cli.execution.output, &project.document);
    let jobs = engine_jobs(cli, project);
    let mut projector = SignProjector::new(output);
    projector.open();

    let (_summary, cancelled) =
        drive_streamed(&units, operation, jobs, supervisor, &mut projector)?;
    if cancelled {
        return Ok(ExitCode::Cancelled);
    }
    Ok(ExitCode::Success)
}

/// `release verify`: verify the declared release archives — locally (presence +
/// reported version) or, with `--download`, against the hosted release
/// (signature + checksum + reported version) — mutating nothing.
pub(super) fn verify(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let runner: Arc<dyn ToolRunner> =
        Arc::new(ProcessToolRunner::new().with_supervisor(Arc::clone(supervisor)));
    let downloader = GhAssetDownloader::new(runner.clone());
    let verifier = CosignVerifier::new(runner.clone());
    let probe: Arc<dyn toven_ports::VersionProbe> = Arc::new(ProcessVersionProbe::new(runner));
    let options = VerifyOptions {
        download: cli.download,
        run: !cli.no_run,
    };
    let mut reporter = QuietReporter;
    let (operation, units) = verify_operation(
        &request,
        &project.document,
        providers,
        &readers,
        options,
        &downloader,
        &verifier,
        probe,
        &mut reporter,
    )?;
    let inputs = operation.inputs();

    let output = resolve_output(cli.execution.output, &project.document);
    let jobs = engine_jobs(cli, project);
    let mut projector = VerifyProjector::new(
        output,
        inputs.mode().as_str().to_string(),
        inputs.tag().map(str::to_string),
        inputs.expected_version(),
    );
    projector.open();

    let (_summary, cancelled) =
        drive_streamed(&units, operation, jobs, supervisor, &mut projector)?;
    if cancelled {
        return Ok(ExitCode::Cancelled);
    }
    Ok(ExitCode::Success)
}

/// `release image`: build the configured container image once, push it to the
/// primary registry plus mirrors immutably, and cosign-sign the pushed digest.
/// `--dry-run` previews the references and existing digests mutation-free, so it
/// needs no `--yes`; the real push requires confirmation.
pub(super) fn image(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    if !cli.execution.dry_run {
        require_release_confirmation(cli.confirm_release)?;
    }
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let runner: Arc<dyn ToolRunner> =
        Arc::new(ProcessToolRunner::new().with_supervisor(Arc::clone(supervisor)));
    let image_phase: Arc<dyn toven_ports::ImagePhase> = Arc::new(BuildxImagePhase::new(runner));
    let options = ImageOptions {
        dry_run: cli.execution.dry_run,
    };
    let mut reporter = QuietReporter;
    let (operation, units) = image_operation(
        &request,
        &project.document,
        providers,
        &readers,
        image_phase,
        options,
        &mut reporter,
    )?;
    let inputs = operation.inputs();

    let output = resolve_output(cli.execution.output, &project.document);
    let jobs = engine_jobs(cli, project);
    let mut projector = ImageProjector::new(output, inputs.preview());
    projector.open();

    let (_summary, cancelled) =
        drive_streamed(&units, operation, jobs, supervisor, &mut projector)?;
    if cancelled {
        return Ok(ExitCode::Cancelled);
    }
    Ok(ExitCode::Success)
}

/// `release provenance`: verify that exactly the published subjects (the
/// declared `SHA256SUMS` entries plus pushed image digests) carry a
/// build-provenance attestation cut by the CI trusted builder. `--dry-run`
/// reports presence without failing; the default run fails closed if any
/// subject lacks an attestation. Read-only, so it needs no `--yes`.
pub(super) fn provenance(
    providers: &[&dyn Provider],
    supervisor: &Arc<ProcessSupervisor>,
    project: &Project,
    cli: &Cli,
) -> AppResult<ExitCode> {
    let request = release_request(project)?;
    let opened = project.open_member_vcs(providers, &BaselineFlags::new())?;
    let readers = opened.readers();
    let runner: Arc<dyn ToolRunner> =
        Arc::new(ProcessToolRunner::new().with_supervisor(Arc::clone(supervisor)));
    let provenance_phase: Arc<dyn toven_ports::ProvenancePhase> =
        Arc::new(GhAttestationProvenance::new(runner.clone()));
    let image_phase = BuildxImagePhase::new(runner);
    let options = ProvenanceOptions {
        dry_run: cli.execution.dry_run,
    };
    let mut reporter = QuietReporter;
    let (operation, units) = provenance_operation(
        &request,
        &project.document,
        providers,
        &readers,
        provenance_phase,
        &image_phase,
        options,
        &mut reporter,
    )?;
    let inputs = operation.inputs();

    let output = resolve_output(cli.execution.output, &project.document);
    let jobs = engine_jobs(cli, project);
    let mut projector = ProvenanceProjector::new(output, inputs.preview());
    projector.open();

    let (summary, cancelled) = drive_streamed(&units, operation, jobs, supervisor, &mut projector)?;
    if cancelled {
        return Ok(ExitCode::Cancelled);
    }
    // An enforced run fails closed if any published subject lacked an
    // attestation; the preview only reports presence and never fails.
    if !inputs.preview() && summary.has_failures() {
        return Err(AppError::invalid_input(
            "release.provenance.subject",
            format!(
                "{} of {} published subject(s) lack a build-provenance attestation",
                summary.failed + summary.blocked,
                summary.total
            ),
        ));
    }
    Ok(ExitCode::Success)
}
