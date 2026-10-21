//! The top-level clap command: global flags and the reserved-verb tree.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use super::execution::ExecutionArgs;
use super::values::{EnforcementArg, GraphFormat, parse_percentage_arg};

/// Top-level `--help` examples covering the most common argv-first workflows.
const TOP_LEVEL_EXAMPLES: &str = "\
Examples:
  toven test                 Run the `test` task across the affected modules
  toven build --dry-run      Preview what would run, without executing anything
  toven test -- --nocapture  Splice passthrough args at each task's `{args}`
  toven modules              List the discovered modules
  toven tasks                List every runnable task, per ecosystem
  toven graph --format dot   Emit the dependency graph as Graphviz DOT
  toven coverage             Run coverage, aggregate profiles, and gate
  toven release plan         Show the release plan (bumps, changelog, order)
  toven completions zsh      Print a zsh completion script

Any non-reserved token is an argv-first task name. Run `toven <command> --help`
for command-specific examples.";

/// `run` verb examples (the argv-first escape hatch for shadowed task names).
const RUN_EXAMPLES: &str = "\
Examples:
  toven run test             Run a task whose name shadows a reserved verb
  toven run test -- -q       Forward `-q` to the task at its `{args}` slot";

/// `plan` verb examples.
const PLAN_EXAMPLES: &str = "\
Examples:
  toven plan build           Preview what `build` would run (waves + units)";

/// `explain` verb examples.
const EXPLAIN_EXAMPLES: &str = "\
Examples:
  toven explain test                Explain every module's unit for `test`
  toven explain test --module core  Explain the unit(s) for one module + task";

/// `affected` verb examples.
const AFFECTED_EXAMPLES: &str = "\
Examples:
  toven affected test        Project the modules a `test` run would touch";

/// `coverage` verb examples.
const COVERAGE_EXAMPLES: &str = "\
Examples:
  toven coverage                 Run coverage, aggregate profiles, and gate
  toven coverage --line 90       Override the line floor for this run
  toven coverage --enforcement advisory  Report shortfalls without failing";

/// `modules` verb examples.
const MODULES_EXAMPLES: &str = "\
Examples:
  toven modules              List every discovered module";

/// `graph` verb examples.
const GRAPH_EXAMPLES: &str = "\
Examples:
  toven graph                Print the dependency graph as indented text
  toven graph --format dot   Emit Graphviz DOT for `dot -Tsvg`";

/// `tasks` verb examples.
const TASKS_EXAMPLES: &str = "\
Examples:
  toven tasks                List every runnable task, per ecosystem
  toven tasks format         Show one task's argv template and inputs";

/// `completions` verb examples.
const COMPLETIONS_EXAMPLES: &str = "\
Examples:
  toven completions zsh > _toven      Install zsh completions
  source <(toven completions bash)    Load bash completions for this shell";

/// `doctor` verb examples.
const DOCTOR_EXAMPLES: &str = "\
Examples:
  toven doctor               Report which tools the resolved task graph needs, and which are missing
  toven doctor --output jsonl   Emit the audit as a machine-parseable stream
  toven doctor --ensure      Exit with a typed error if any required tool is missing (never installs)";

/// `commit-lint` verb examples.
const COMMIT_LINT_EXAMPLES: &str = "\
Examples:
  toven commit-lint \"feat(cli): add commit-lint verb\"   Lint a subject given as an argument
  git log -1 --pretty=%B | toven commit-lint             Lint HEAD's message from stdin
  toven commit-lint --output jsonl \"fix: bug\"            Emit the verdict as a JSON line";

/// `init` verb examples.
const INIT_EXAMPLES: &str = "\
Examples:
  toven init                 Detect ecosystems and write a `toven.toml` for the current repo
  toven init --non-interactive   Take questionnaire defaults with no prompts (CI)
  toven init --print         Preview the rendered `toven.toml` on stdout without writing
  toven init --force rust    Regenerate just the `[ecosystems.rust]` block";

/// `release` verb examples (the umbrella over the lifecycle action tree).
const RELEASE_EXAMPLES: &str = "\
Examples:
  toven release plan               Show the release plan (bumps, changelog, order)
  toven release status             Compare declared vs published/tagged versions
  toven release readiness          Run the fail-closed preflight go/no-go checks
  toven release publish --dry-run  Rehearse the full pipeline without mutating
  toven release tag --minor=rust:core  Cut a minor bump for one module";

/// `release plan` action examples.
const RELEASE_PLAN_EXAMPLES: &str = "\
Examples:
  toven release plan               Show the release plan (read-only)
  toven release plan --output jsonl  Emit the plan as a machine-readable stream";

/// `release status` action examples.
const RELEASE_STATUS_EXAMPLES: &str = "\
Examples:
  toven release status             Show declared vs published/tagged versions";

/// `release tag` action examples.
const RELEASE_TAG_EXAMPLES: &str = "\
Examples:
  toven release tag --yes          Bump, commit, tag, and push (no publish)
  toven release tag --minor=rust:core --yes  Force a minor bump for one module
  toven release tag --no-push --yes  Cut the tag locally without pushing";

/// `release bump` action examples.
const RELEASE_BUMP_EXAMPLES: &str = "\
Examples:
  toven release bump --yes         Stage the version + changelog change for a pull request (no commit/tag/push)
  toven release bump --dry-run     Preview the version + changelog mutation without writing
  toven release bump               Show the pending cut, then fail closed asking for --yes";

/// `release publish` action examples.
const RELEASE_PUBLISH_EXAMPLES: &str = "\
Examples:
  toven release publish --yes      Publish through each configured ecosystem target
  toven release publish --dry-run  Rehearse the publish order without mutating
  toven release publish --pre rc --yes  Publish a prerelease on the rc channel";

/// `release readiness` action examples.
const RELEASE_READINESS_EXAMPLES: &str = "\
Examples:
  toven release readiness          Evaluate the fail-closed preflight checks";

/// `release sbom` action examples.
const RELEASE_SBOM_EXAMPLES: &str = "\
Examples:
  toven release sbom               Write a CycloneDX SBOM per releasable module
  toven release sbom --out-dir target/sbom  Choose the artifact directory";

/// `release depgraphs` action examples.
const RELEASE_DEPGRAPHS_EXAMPLES: &str = "\
Examples:
  toven release depgraphs          Write a DOT dependency graph per module
  toven release depgraphs --out-dir target/graphs  Choose the artifact directory";

/// `release package` action examples.
const RELEASE_PACKAGE_EXAMPLES: &str = "\
Examples:
  toven release package --target x86_64-unknown-linux-gnu   Archive the built binary into its declared asset
  toven release package --target x86_64-pc-windows-msvc --binary path/to/toven.exe  Package an explicit binary";

/// `release checksums` action examples.
const RELEASE_CHECKSUMS_EXAMPLES: &str = "\
Examples:
  toven release checksums          Write SHA256SUMS over the declared release assets";

/// `release sign` action examples.
const RELEASE_SIGN_EXAMPLES: &str = "\
Examples:
  toven release sign               Sign SHA256SUMS into its declared .bundle sidecar (cosign)";

/// `release verify` action examples.
const RELEASE_VERIFY_EXAMPLES: &str = "\
Examples:
  toven release verify                    Verify local dist/ archives present and reporting the expected version
  toven release verify --no-run           Verify presence without executing the packaged binary
  toven release verify --download         Download the hosted assets, verify signature + checksum, then run";

/// `release image` action examples.
const RELEASE_IMAGE_EXAMPLES: &str = "\
Examples:
  toven release image --dry-run    Preview the image references and existing digests without building or pushing
  toven release image --yes        Build once, push to the primary registry plus mirrors, and cosign-sign the digest";

/// `release provenance` action examples.
const RELEASE_PROVENANCE_EXAMPLES: &str = "\
Examples:
  toven release provenance --dry-run  Report whether the published subjects carry an attestation, without failing
  toven release provenance            Verify every published subject carries a build-provenance attestation";

/// `driver` verb examples.
const DRIVER_EXAMPLES: &str = "\
Examples:
  toven driver list                List the installed out-of-process drivers
  toven driver install rust        Install the `rust` ecosystem driver";

/// `federation` verb examples.
const FEDERATION_EXAMPLES: &str = "\
Examples:
  toven federation status          Report federated member-repo status
  toven federation sync            Synchronize federated member repositories";

/// `cache` verb examples.
const CACHE_EXAMPLES: &str = "\
Examples:
  toven cache path                 Print the resolved cache directory
  toven cache stats                Summarize the local cache directory
  toven cache clean                Remove the local cache directory";

/// The parsed top-level CLI surface: global flags plus the dispatched verb.
#[derive(Debug, Parser)]
#[command(
    name = "toven",
    version,
    about = "Toven — argv-first development and CI task planner for multi-module repositories",
    after_help = TOP_LEVEL_EXAMPLES,
    allow_external_subcommands = true,
    subcommand_required = true,
    arg_required_else_help = true
)]
#[allow(clippy::struct_excessive_bools)]
pub struct Cli {
    /// The execution flags a bare task also accepts.
    #[command(flatten)]
    pub execution: ExecutionArgs,
    /// Release only: skip pushing the release commit and tags.
    #[arg(long, global = true, help_heading = "Release")]
    pub no_push: bool,
    /// Release bump/tag/publish only: force `<module>` to bump at the patch
    /// level (repeatable; valued form `--patch=<module>`). Given with no value,
    /// bumps every in-scope module at patch, each advancing from its own
    /// baseline (repo-wide). Highest precedence with the other level
    /// flags and `--set-version`; a module named in two level flags or a level
    /// flag plus `--set-version` is a usage error.
    #[arg(
        long = "patch",
        global = true,
        value_name = "MODULE",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "*",
        help_heading = "Release"
    )]
    pub patch: Vec<String>,
    /// Release bump/tag/publish only: force `<module>` to bump at the minor
    /// level (repeatable; valued form `--minor=<module>`). Given with no value,
    /// bumps every in-scope module at minor, each advancing from its own
    /// baseline (repo-wide).
    #[arg(
        long = "minor",
        global = true,
        value_name = "MODULE",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "*",
        help_heading = "Release"
    )]
    pub minor: Vec<String>,
    /// Release bump/tag/publish only: force `<module>` to bump at the major
    /// level (repeatable; valued form `--major=<module>`). Given with no value,
    /// bumps every in-scope module at major, each advancing from its own
    /// baseline (repo-wide).
    #[arg(
        long = "major",
        global = true,
        value_name = "MODULE",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "*",
        help_heading = "Release"
    )]
    pub major: Vec<String>,
    /// Release bump/tag/publish only: pin `<module>=<x.y.z>` to an explicit
    /// target version (repeatable), or a bare `<x.y.z>` to set every in-scope
    /// module to that version (lock-step, root included).
    #[arg(
        long = "set-version",
        global = true,
        value_name = "[MODULE=]VERSION",
        help_heading = "Release"
    )]
    pub set_version: Vec<String>,
    /// Release bump/tag/publish only: cut a prerelease on a configured channel
    /// (`rc`/`alpha`/`beta`).
    #[arg(
        long = "pre",
        global = true,
        value_name = "CHANNEL",
        help_heading = "Release"
    )]
    pub pre: Option<String>,
    /// Release bump/tag/publish only: skip registry `published_versions`
    /// lookups and anchor idempotency on the release tag only.
    #[arg(long, global = true, help_heading = "Release")]
    pub offline: bool,
    /// Release sbom/depgraphs only: directory to write generated artifacts into
    /// (created if absent; defaults to `target/toven/release`).
    #[arg(long, global = true, value_name = "PATH", help_heading = "Release")]
    pub out_dir: Option<PathBuf>,
    /// Release package only: the target triple to package (e.g.
    /// `x86_64-unknown-linux-gnu`); selects the declared archive asset and the
    /// built binary under `target/<triple>/release/`.
    #[arg(long, global = true, value_name = "TRIPLE", help_heading = "Release")]
    pub target: Option<String>,
    /// Release package only: explicit path to the already-built binary to
    /// archive; defaults to `target/<triple>/release/<binary>`.
    #[arg(long, global = true, value_name = "PATH", help_heading = "Release")]
    pub binary: Option<PathBuf>,
    /// Release verify only: download the declared assets from the hosted
    /// release and verify signature + checksum before extraction, instead of
    /// verifying the local `dist/` archives.
    #[arg(long, global = true, help_heading = "Release")]
    pub download: bool,
    /// Release verify only: skip executing the packaged binary; presence,
    /// signature, and checksum are still enforced.
    #[arg(long = "no-run", global = true, help_heading = "Release")]
    pub no_run: bool,
    /// Coverage only: override the absolute line-coverage floor for this run
    /// (percentage, `0..=100`); wins over the `[…coverage].line` config
    /// default.
    #[arg(long, global = true, value_name = "PCT", value_parser = parse_percentage_arg, help_heading = "Coverage")]
    pub line: Option<f64>,
    /// Coverage only: override the function-coverage floor for this run
    /// (percentage; gated only where the ecosystem measures functions).
    #[arg(long, global = true, value_name = "PCT", value_parser = parse_percentage_arg, help_heading = "Coverage")]
    pub function: Option<f64>,
    /// Coverage only: override the region-coverage floor for this run
    /// (percentage; gated only where the ecosystem measures regions).
    #[arg(long, global = true, value_name = "PCT", value_parser = parse_percentage_arg, help_heading = "Coverage")]
    pub region: Option<f64>,
    /// Coverage only: override the changed-lines floor for this run
    /// (percentage; applied to changed files under a changed selection).
    #[arg(long = "changed-line", global = true, value_name = "PCT", value_parser = parse_percentage_arg, help_heading = "Coverage")]
    pub changed_line: Option<f64>,
    /// Coverage only: override how a below-threshold verdict is enforced
    /// (`block` fails the gate; `advisory` reports without failing).
    #[arg(long, global = true, value_name = "MODE", help_heading = "Coverage")]
    pub enforcement: Option<EnforcementArg>,
    /// Init only: regenerate one `[ecosystems.<id>]` section.
    #[arg(long, global = true, value_name = "ID", help_heading = "Init")]
    pub force: Option<String>,
    /// Init only: project root to onboard against.
    #[arg(long, global = true, value_name = "PATH", help_heading = "Init")]
    pub root: Option<PathBuf>,
    /// Init only: answer the wizard non-interactively, accepting every default.
    #[arg(long = "non-interactive", global = true, help_heading = "Init")]
    pub non_interactive: bool,
    /// Release bump/tag/publish/image only: confirm the real mutation. Kept
    /// separate from
    /// `--non-interactive` so authorizing a release is always a deliberate flag,
    /// never something learned as harmless on `init`.
    #[arg(long = "yes", global = true, help_heading = "Release")]
    pub confirm_release: bool,
    /// Init only: render the `toven.toml` to stdout and write nothing.
    #[arg(long, global = true, help_heading = "Init")]
    pub print: bool,
    /// Graph only: dependency-graph rendering format.
    #[arg(long, global = true, value_name = "FORMAT", help_heading = "Graph")]
    pub format: Option<GraphFormat>,
    /// Driver/federation only: provision missing drivers automatically.
    #[arg(long, global = true, help_heading = "Driver")]
    pub auto_install: bool,
    /// The dispatched verb (a reserved built-in or a bare task name).
    #[command(subcommand)]
    pub command: Command,
}

/// The reserved built-in verbs plus the argv-first task escape hatch.
#[derive(Debug, Subcommand)]
#[non_exhaustive]
pub enum Command {
    /// Run a task by name (escape hatch for task names that shadow a reserved
    /// word).
    #[command(after_long_help = RUN_EXAMPLES)]
    Run {
        /// Task name to run.
        task: String,
        /// Passthrough args after `--`, spliced verbatim at each task's
        /// `{args}`.
        #[arg(last = true)]
        passthrough: Vec<String>,
    },
    /// Preview what a task would run, without executing it (`run <task> --dry-run`).
    #[command(after_long_help = PLAN_EXAMPLES)]
    Plan {
        /// Task to plan.
        task: String,
    },
    /// Plan, inspect, and publish a release through its lifecycle actions.
    #[command(after_long_help = RELEASE_EXAMPLES)]
    Release {
        /// Release lifecycle action.
        #[command(subcommand)]
        action: ReleaseAction,
    },
    /// Run the coverage task, aggregate the emitted profiles per module, and
    /// gate them against the resolved coverage thresholds.
    #[command(after_long_help = COVERAGE_EXAMPLES)]
    Coverage,
    /// Explain what a task would run, optionally filtered to a `--module`
    /// selection.
    #[command(after_long_help = EXPLAIN_EXAMPLES)]
    Explain {
        /// Task, comma list of tasks, or `[sets]` name to explain.
        task: String,
    },
    /// Detect ecosystems and write (or preview) a `toven.toml` via the wizard.
    #[command(after_long_help = INIT_EXAMPLES)]
    Init,
    /// Project the affected-module set for a task.
    #[command(after_long_help = AFFECTED_EXAMPLES)]
    Affected {
        /// Task, comma list of tasks, or `[sets]` name whose blast radius is
        /// projected.
        task: String,
    },
    /// List discovered modules.
    #[command(visible_aliases = ["list", "ls"], after_long_help = MODULES_EXAMPLES)]
    Modules,
    /// Project the dependency graph (text or dot).
    #[command(visible_alias = "deps", after_long_help = GRAPH_EXAMPLES)]
    Graph,
    /// List the runnable tasks resolved for each ecosystem.
    #[command(after_long_help = TASKS_EXAMPLES)]
    Tasks {
        /// Optional task name to show in detail (argv template, inputs).
        name: Option<String>,
    },
    /// Audit the tools the resolved task graph needs and report which are
    /// present or missing.
    #[command(after_long_help = DOCTOR_EXAMPLES)]
    Doctor {
        /// Turn any missing tool into a hard failure: exit with a typed error
        /// naming the gaps instead of only reporting them (report-only is the
        /// secure-by-default otherwise). `doctor` never installs tools; the
        /// global `--auto-install` flag is accepted as an equivalent.
        #[arg(long)]
        ensure: bool,
        /// Also run the `[doctor.checks]` that list this profile (repeatable).
        /// Checks without profiles always run.
        #[arg(long = "profile", value_name = "NAME")]
        profile: Vec<String>,
    },
    /// Lint a commit message (or PR title) against the Conventional Commits
    /// `type(scope)!: description` grammar Toven's changelog relies on.
    #[command(name = "commit-lint", after_long_help = COMMIT_LINT_EXAMPLES)]
    CommitLint {
        /// The commit subject to lint. Omit to read the full message from stdin
        /// and lint its first line.
        message: Option<String>,
    },
    /// Print a shell completion script
    /// (`bash`/`zsh`/`fish`/`powershell`/`elvish`).
    #[command(after_long_help = COMPLETIONS_EXAMPLES)]
    Completions {
        /// Target shell for the emitted completion script.
        shell: clap_complete::Shell,
    },
    /// Out-of-process driver management.
    #[command(after_long_help = DRIVER_EXAMPLES)]
    Driver {
        /// Driver action.
        #[command(subcommand)]
        action: DriverAction,
    },
    /// Cross-repo federation management.
    #[command(after_long_help = FEDERATION_EXAMPLES)]
    Federation {
        /// Federation action.
        #[command(subcommand)]
        action: FederationAction,
    },
    /// Task-cache maintenance.
    #[command(after_long_help = CACHE_EXAMPLES)]
    Cache {
        /// Cache action.
        #[command(subcommand)]
        action: CacheAction,
    },
    /// A bare top-level token: an argv-first task name plus its trailing args.
    #[command(external_subcommand)]
    External(Vec<String>),
}

/// `toven release <action>`.
///
/// A reviewable release lifecycle: read-only projections (`plan`, `status`)
/// that never mutate, and mutating actions (`tag`, `publish`) that run the
/// release pipeline. `tag` stops after the release commit/tag/push; `publish`
/// continues to the registry. `--dry-run` turns `publish` into a no-mutation
/// rehearsal that reports the resolved publish order and per-module verdicts.
#[derive(Debug, Clone, Copy, Subcommand)]
#[non_exhaustive]
pub enum ReleaseAction {
    /// Show the release plan — bumped versions, changelog, and publish order —
    /// without mutating anything.
    #[command(after_long_help = RELEASE_PLAN_EXAMPLES)]
    Plan,
    /// Show each module's declared version versus what is published and tagged
    /// (read-only).
    #[command(after_long_help = RELEASE_STATUS_EXAMPLES)]
    Status,
    /// Cut the release: bump manifests, commit, tag, and push — without
    /// publishing to the registry.
    #[command(after_long_help = RELEASE_TAG_EXAMPLES)]
    Tag,
    /// Run only the version + changelog mutation phase: rewrite each module's
    /// manifest version and dependency floors and, where configured, roll the
    /// changelog, then stage the mutation for a pull request. Never commits,
    /// tags, pushes, or publishes — the commit/tag/push is `release tag` /
    /// `release publish` after the staged change merges.
    #[command(after_long_help = RELEASE_BUMP_EXAMPLES)]
    Bump,
    /// Run the full release pipeline (commit, tag, push, publish); `--dry-run`
    /// rehearses the publish order and per-module
    /// would-publish/already-published verdicts without mutating anything.
    #[command(after_long_help = RELEASE_PUBLISH_EXAMPLES)]
    Publish,
    /// Evaluate the fail-closed release-readiness preflight (configured
    /// go/no-go checks) without mutating anything.
    #[command(after_long_help = RELEASE_READINESS_EXAMPLES)]
    Readiness,
    /// Generate a CycloneDX SBOM per releasable module under `--out-dir`
    /// (read-only).
    #[allow(clippy::doc_markdown)]
    #[command(after_long_help = RELEASE_SBOM_EXAMPLES)]
    Sbom,
    /// Render the dependency graph to a DOT artifact under `--out-dir`
    /// (read-only).
    #[command(after_long_help = RELEASE_DEPGRAPHS_EXAMPLES)]
    Depgraphs,
    /// Package already-built binaries into the fixed-name archive assets the
    /// hosted release declares (`--target <triple>`); non-mutating.
    #[command(after_long_help = RELEASE_PACKAGE_EXAMPLES)]
    Package,
    /// Write the `SHA256SUMS` manifest over the declared release assets to its
    /// declared asset path; non-mutating.
    #[allow(clippy::doc_markdown)]
    #[command(after_long_help = RELEASE_CHECKSUMS_EXAMPLES)]
    Checksums,
    /// Sign the `SHA256SUMS` manifest into its declared self-contained Sigstore
    /// bundle asset with cosign; non-mutating.
    #[allow(clippy::doc_markdown)]
    #[command(after_long_help = RELEASE_SIGN_EXAMPLES)]
    Sign,
    /// Verify the declared release archives — locally (presence + reported
    /// version) or, with `--download`, against the hosted release (signature +
    /// checksum + reported version); non-mutating.
    #[allow(clippy::doc_markdown)]
    #[command(after_long_help = RELEASE_VERIFY_EXAMPLES)]
    Verify,
    /// Build the configured container image once, push it to the primary
    /// registry plus mirrors immutably, and cosign-sign the pushed digest;
    /// `--dry-run` previews the references and existing digests mutation-free,
    /// the real push requires `--yes`.
    #[command(after_long_help = RELEASE_IMAGE_EXAMPLES)]
    Image,
    /// Verify that exactly the published subjects (declared `SHA256SUMS`
    /// entries plus pushed image digests) carry a build-provenance attestation
    /// cut by the CI trusted builder; `--dry-run` reports presence without
    /// failing, the default run fails closed if any subject lacks one.
    #[allow(clippy::doc_markdown)]
    #[command(after_long_help = RELEASE_PROVENANCE_EXAMPLES)]
    Provenance,
}

impl ReleaseAction {
    /// The action's canonical name (for error messages and help).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Status => "status",
            Self::Tag => "tag",
            Self::Bump => "bump",
            Self::Publish => "publish",
            Self::Readiness => "readiness",
            Self::Sbom => "sbom",
            Self::Depgraphs => "depgraphs",
            Self::Package => "package",
            Self::Checksums => "checksums",
            Self::Sign => "sign",
            Self::Verify => "verify",
            Self::Image => "image",
            Self::Provenance => "provenance",
        }
    }

    /// Whether the action performs a version-cut mutation of the working tree,
    /// history, or registry (a version cut, changelog roll, commit, tag, or
    /// publish): `bump`, `tag`, and `publish`. These accept the per-run bump
    /// argv and require `--yes`. The registry mutation `image` is *not* a
    /// version cut — it takes no bump argv — so it is classified by
    /// [`Self::requires_confirmation`] instead.
    #[must_use]
    pub const fn is_mutating(self) -> bool {
        matches!(self, Self::Bump | Self::Tag | Self::Publish)
    }

    /// Whether the action performs a real mutation that must be authorized with
    /// `--yes`: the version-cut family (`bump` / `tag` / `publish`) plus the
    /// registry mutation `image` (which pushes to registries). Each honors
    /// `--dry-run` as a mutation-free preview that needs no confirmation.
    /// `provenance` is read-only verification and is deliberately excluded.
    #[must_use]
    pub const fn requires_confirmation(self) -> bool {
        matches!(self, Self::Bump | Self::Tag | Self::Publish | Self::Image)
    }

    /// Whether the action is non-mutating and therefore a PLAN-only cut with no
    /// APPLY half (`plan` / `status` / `readiness` / `sbom` / `depgraphs` /
    /// `package` / `checksums` / `sign` / `verify` / `provenance`). These never
    /// bump a manifest, tag, or publish; `sbom` and `depgraphs` write artifacts
    /// under `--out-dir` (see [`Self::writes_artifacts`]) while `package`,
    /// `checksums`, and `sign` materialize their declared release assets in
    /// place, and `verify`/`provenance` only read — none touch history.
    #[must_use]
    pub const fn is_projection(self) -> bool {
        matches!(
            self,
            Self::Plan
                | Self::Status
                | Self::Readiness
                | Self::Sbom
                | Self::Depgraphs
                | Self::Package
                | Self::Checksums
                | Self::Sign
                | Self::Verify
                | Self::Provenance
        )
    }

    /// Whether the action writes artifacts under `--out-dir` (`sbom` /
    /// `depgraphs`).
    #[must_use]
    pub const fn writes_artifacts(self) -> bool {
        matches!(self, Self::Sbom | Self::Depgraphs)
    }

    /// Whether `--dry-run` is meaningful for the action: `plan` (already a
    /// projection), the rehearsable `publish` pipeline, `bump` (which previews
    /// the version + changelog mutation without writing), the registry mutation
    /// `image` (which previews its references/digests without building or
    /// pushing), and `provenance` (whose preview reports subject presence
    /// without failing on a missing attestation).
    #[must_use]
    pub const fn accepts_dry_run(self) -> bool {
        matches!(
            self,
            Self::Plan | Self::Bump | Self::Publish | Self::Image | Self::Provenance
        )
    }
}

/// `toven driver <action>`.
#[derive(Debug, Subcommand)]
#[non_exhaustive]
pub enum DriverAction {
    /// Install an out-of-process driver by id.
    Install {
        /// Driver id to install.
        id: String,
    },
    /// List installed drivers.
    List,
}

/// `toven federation <action>`.
#[derive(Debug, Subcommand)]
#[non_exhaustive]
pub enum FederationAction {
    /// Synchronize federated member repositories.
    Sync,
    /// Report federation status.
    Status,
}

/// `toven cache <action>`.
#[derive(Debug, Subcommand)]
#[non_exhaustive]
pub enum CacheAction {
    /// Summarize the local cache directory.
    Stats,
    /// Remove the local cache directory.
    Clean,
    /// Print the resolved local cache directory.
    Path,
}

impl Cli {
    /// Whether the verb produces a PLAN-only cut (no APPLY half).
    ///
    /// Either an explicit `--dry-run`/`--explain` on an execution verb, or an
    /// introspection verb that is a projection over the PLAN cut by definition.
    #[must_use]
    pub const fn is_plan_only(&self) -> bool {
        if self.execution.is_plan_only() {
            return true;
        }
        matches!(
            self.command,
            Command::Release { action } if action.is_projection()
        )
    }
}
