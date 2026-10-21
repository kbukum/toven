//! Per-root package manager detection.
//!
//! The package manager is a fact of each root, read from the root's own
//! signals in priority order: an explicit `[ecosystems.js] package_manager`
//! override, the Corepack `packageManager` pin, `devEngines.packageManager`,
//! then the single lockfile present. Disagreeing signals fail with the exact
//! fix rather than a guess. Without any of those, a `pnpm-workspace.yaml`
//! selects pnpm and a `.yarnrc.yml` selects Yarn Berry; with no signal at all
//! the root uses npm, the platform default that ships with Node.

use std::path::Path;

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::sync_io::file;
use serde::Deserialize;

use crate::manifest::{PNPM_WORKSPACE, PackageJson};

/// The Yarn Berry settings file; its presence marks a Yarn 2+ root.
pub(crate) const YARNRC_YML: &str = ".yarnrc.yml";

/// Every lockfile name, with the tool that owns it.
const LOCKFILES: &[(Tool, &str)] = &[
    (Tool::Npm, "package-lock.json"),
    (Tool::Npm, "npm-shrinkwrap.json"),
    (Tool::Pnpm, "pnpm-lock.yaml"),
    (Tool::Yarn, "yarn.lock"),
    (Tool::Bun, "bun.lock"),
    (Tool::Bun, "bun.lockb"),
];

/// A package manager tool, as named in `packageManager`, `devEngines`, and the
/// `[ecosystems.js] package_manager` override.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    /// npm, bundled with Node.
    Npm,
    /// pnpm.
    Pnpm,
    /// Yarn, classic (1.x) or Berry (2+).
    Yarn,
    /// Bun, which is its own runtime.
    Bun,
}

impl Tool {
    /// The tool's command name, also used as the workspace toolchain tag.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
        }
    }

    /// Parse a tool from its command name.
    fn parse(name: &str) -> Option<Self> {
        match name {
            "npm" => Some(Self::Npm),
            "pnpm" => Some(Self::Pnpm),
            "yarn" => Some(Self::Yarn),
            "bun" => Some(Self::Bun),
            _ => None,
        }
    }

    /// Parse a tool from a toolchain tag name.
    pub(crate) fn from_tag(name: &str) -> Option<Self> {
        Self::parse(name)
    }

    /// The lockfile names this tool writes.
    pub(crate) fn lockfiles(self) -> impl Iterator<Item = &'static str> {
        LOCKFILES
            .iter()
            .filter(move |(tool, _)| *tool == self)
            .map(|(_, name)| *name)
    }
}

/// A detected package manager, with the Yarn generation resolved.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum Manager {
    /// npm.
    Npm,
    /// pnpm.
    Pnpm,
    /// Yarn 1.x.
    YarnClassic,
    /// Yarn 2 and later.
    YarnBerry,
    /// Bun.
    Bun,
}

impl Manager {
    /// The tool behind this manager.
    pub(crate) const fn tool(self) -> Tool {
        match self {
            Self::Npm => Tool::Npm,
            Self::Pnpm => Tool::Pnpm,
            Self::YarnClassic | Self::YarnBerry => Tool::Yarn,
            Self::Bun => Tool::Bun,
        }
    }

    /// The stable id onboarding records for this manager.
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::YarnClassic => "yarn-classic",
            Self::YarnBerry => "yarn-berry",
            Self::Bun => "bun",
        }
    }

    /// Parse a manager from its [`id`](Self::id).
    pub(crate) fn from_id(id: &str) -> Option<Self> {
        match id {
            "npm" => Some(Self::Npm),
            "pnpm" => Some(Self::Pnpm),
            "yarn-classic" => Some(Self::YarnClassic),
            "yarn-berry" => Some(Self::YarnBerry),
            "bun" => Some(Self::Bun),
            _ => None,
        }
    }

    /// The argv running the manager's `command` in the repo-relative `dir`, or
    /// in the current directory when `dir` is `None`.
    ///
    /// Each tool takes its directory flag in a different place: Bun reads
    /// `--cwd` only after the subcommand (`bun --cwd web run` prints help), and
    /// Yarn Berry reads `--cwd` only as the first argument.
    pub(crate) fn in_dir(self, dir: Option<&str>, command: &[&str]) -> Vec<String> {
        let tool = self.tool().name();
        let mut argv = vec![tool.to_string()];
        let Some(dir) = dir else {
            argv.extend(command.iter().map(ToString::to_string));
            return argv;
        };
        let flag = match self {
            Self::Npm => "--prefix",
            Self::Pnpm => "--dir",
            Self::YarnClassic | Self::YarnBerry | Self::Bun => "--cwd",
        };
        let (head, rest) = match (self, command.split_first()) {
            (Self::Bun, Some((subcommand, rest))) => (vec![*subcommand], rest),
            _ => (Vec::new(), command),
        };
        argv.extend(head.into_iter().map(ToString::to_string));
        argv.push(flag.to_string());
        argv.push(dir.to_string());
        argv.extend(rest.iter().map(ToString::to_string));
        argv
    }

    /// The argv running `script` in `dir`, forwarding `extra` and then the
    /// run's extra arguments (`{args}`) to the script. npm needs `--` to
    /// forward flags.
    pub(crate) fn run_script(self, dir: &str, script: &str, extra: &[String]) -> Vec<String> {
        let mut command = vec!["run", script];
        if self == Self::Npm {
            command.push("--");
        }
        command.extend(extra.iter().map(String::as_str));
        command.push("{args}");
        self.in_dir(Some(dir), &command)
    }

    /// The install command that installs exactly the lockfile and fails when
    /// the lockfile is out of date.
    pub(crate) const fn frozen_install(self) -> &'static [&'static str] {
        match self {
            Self::Npm => &["ci"],
            Self::Pnpm | Self::YarnClassic | Self::Bun => &["install", "--frozen-lockfile"],
            Self::YarnBerry => &["install", "--immutable"],
        }
    }

    /// The argv packing the package in the working directory into the empty
    /// directory `destination`.
    ///
    /// The manager packs, rather than npm, so `workspace:` and `catalog:`
    /// ranges resolve the way that manager's users expect.
    pub(crate) fn pack(self, destination: &Path) -> Vec<String> {
        let destination = destination.display().to_string();
        let tarball = || format!("{destination}/package.tgz");
        let argv: Vec<String> = match self {
            Self::Npm => vec![
                "npm".into(),
                "pack".into(),
                "--pack-destination".into(),
                destination,
            ],
            Self::Pnpm => vec![
                "pnpm".into(),
                "pack".into(),
                "--pack-destination".into(),
                destination,
            ],
            Self::YarnClassic => vec!["yarn".into(), "pack".into(), "--filename".into(), tarball()],
            Self::YarnBerry => vec!["yarn".into(), "pack".into(), "--out".into(), tarball()],
            Self::Bun => vec![
                "bun".into(),
                "pm".into(),
                "pack".into(),
                "--destination".into(),
                destination,
            ],
        };
        argv
    }

    /// The install that rewrites only the lockfile after a release changes
    /// workspace versions and ranges, without running scripts, or `None` when
    /// the lockfile records no workspace versions (Yarn classic).
    pub(crate) const fn lockfile_refresh(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Npm => Some(&["npm", "install", "--package-lock-only", "--ignore-scripts"]),
            Self::Pnpm => Some(&["pnpm", "install", "--lockfile-only", "--ignore-scripts"]),
            Self::YarnBerry => Some(&["yarn", "install", "--mode=update-lockfile"]),
            Self::Bun => Some(&["bun", "install", "--lockfile-only", "--ignore-scripts"]),
            Self::YarnClassic => None,
        }
    }

    /// The file an install writes, relative to the root, which records that
    /// dependencies are installed. Bun writes no such file, so its marker is
    /// the `node_modules` directory.
    pub(crate) const fn install_marker(self) -> &'static str {
        match self {
            Self::Npm => "node_modules/.package-lock.json",
            Self::Pnpm => "node_modules/.modules.yaml",
            Self::YarnClassic => "node_modules/.yarn-integrity",
            Self::YarnBerry => ".yarn/install-state.gz",
            Self::Bun => "node_modules",
        }
    }
}

/// The lockfiles present in `root`, in a stable order.
///
/// # Errors
/// A lockfile path that cannot be inspected.
pub(crate) fn lockfiles_in(root: &Path) -> AppResult<Vec<(Tool, &'static str)>> {
    let mut present = Vec::new();
    for (tool, name) in LOCKFILES {
        if file::exists(&root.join(name))? {
            present.push((*tool, *name));
        }
    }
    Ok(present)
}

/// Detect the package manager of the root at `root` (labelled `label`, its
/// repo-relative directory) from its manifest, lockfiles, and an optional
/// configured override.
///
/// # Errors
/// A malformed `packageManager` or `devEngines` name, or signals that name
/// different package managers.
pub(crate) fn detect(
    root: &Path,
    label: &str,
    manifest: &PackageJson,
    override_tool: Option<Tool>,
) -> AppResult<Manager> {
    let pin = manifest
        .package_manager
        .as_deref()
        .map(|value| parse_pin(label, value))
        .transpose()?;
    if let Some(tool) = override_tool {
        let pinned = pin
            .filter(|(pinned, _)| *pinned == tool)
            .map(|(_, version)| version);
        return resolve(root, tool, pinned.as_deref());
    }

    let engine = manifest
        .dev_engine()
        .map(|engine| {
            Tool::parse(&engine.name)
                .ok_or_else(|| unknown_tool(label, "devEngines.packageManager", &engine.name))
        })
        .transpose()?;
    if let (Some((pinned, _)), Some(engine)) = (&pin, engine)
        && *pinned != engine
    {
        return Err(AppError::new(
            ErrorCode::Conflict,
            format!(
                "JavaScript root '{label}' declares packageManager '{}' but \
                 devEngines.packageManager '{}'; make them name the same package manager, or set \
                 [ecosystems.js] package_manager = {{ \"{label}\" = \"<tool>\" }}",
                pinned.name(),
                engine.name()
            ),
        ));
    }

    let lockfiles = lockfiles_in(root)?;
    let mut locked: Vec<Tool> = lockfiles.iter().map(|(tool, _)| *tool).collect();
    locked.dedup();
    if locked.len() > 1 {
        return Err(AppError::new(
            ErrorCode::Conflict,
            format!(
                "JavaScript root '{label}' has lockfiles for several package managers ({}); \
                 delete the stale lockfiles so one package manager owns the root",
                describe_lockfiles(&lockfiles)
            ),
        ));
    }

    let declared = pin
        .as_ref()
        .map(|(tool, _)| (*tool, "packageManager"))
        .or_else(|| engine.map(|tool| (tool, "devEngines.packageManager")));
    match (declared, locked.first()) {
        (Some((tool, source)), Some(lock)) if tool != *lock => Err(AppError::new(
            ErrorCode::Conflict,
            format!(
                "JavaScript root '{label}' declares {} in {source} but has a {} lockfile ({}); \
                 delete the stale lockfile or change {source}",
                tool.name(),
                lock.name(),
                describe_lockfiles(&lockfiles)
            ),
        )),
        (Some((tool, _)), _) => resolve(
            root,
            tool,
            pin.as_ref().map(|(_, version)| version.as_str()),
        ),
        (None, Some(lock)) => resolve(root, *lock, None),
        (None, None) => unlocked(root),
    }
}

/// The `packageManager` pin's version when it pins `manager`'s tool (an
/// override can select a different tool than the pin names).
///
/// # Errors
/// A malformed `packageManager` value.
pub(crate) fn pinned_version(
    label: &str,
    manifest: &PackageJson,
    manager: Manager,
) -> AppResult<Option<String>> {
    Ok(manifest
        .package_manager
        .as_deref()
        .map(|value| parse_pin(label, value))
        .transpose()?
        .filter(|(tool, _)| *tool == manager.tool())
        .map(|(_, version)| version))
}

/// The manager of a root with no pin and no lockfile: a `pnpm-workspace.yaml`
/// marks pnpm and a `.yarnrc.yml` marks Yarn Berry; otherwise npm.
fn unlocked(root: &Path) -> AppResult<Manager> {
    if file::exists(&root.join(PNPM_WORKSPACE))? {
        Ok(Manager::Pnpm)
    } else if file::exists(&root.join(YARNRC_YML))? {
        Ok(Manager::YarnBerry)
    } else {
        Ok(Manager::Npm)
    }
}

/// Resolve a tool to a manager, splitting Yarn by generation: a pinned 1.x is
/// classic, any other pin is Berry, and without a pin a `.yarnrc.yml` marks
/// Berry.
fn resolve(root: &Path, tool: Tool, pinned: Option<&str>) -> AppResult<Manager> {
    Ok(match tool {
        Tool::Npm => Manager::Npm,
        Tool::Pnpm => Manager::Pnpm,
        Tool::Bun => Manager::Bun,
        Tool::Yarn => {
            let berry = match pinned {
                Some(version) => !version.starts_with("1."),
                None => file::exists(&root.join(YARNRC_YML))?,
            };
            if berry {
                Manager::YarnBerry
            } else {
                Manager::YarnClassic
            }
        }
    })
}

/// Parse a Corepack `packageManager` pin (`name@version`, optionally followed
/// by `+<hash>`) into its tool and version.
fn parse_pin(label: &str, value: &str) -> AppResult<(Tool, String)> {
    let invalid = || {
        AppError::invalid_input(
            "packageManager",
            format!(
                "'{}' packageManager '{value}' must be '<name>@<version>' with name npm, pnpm, \
                 yarn, or bun",
                manifest_label(label)
            ),
        )
    };
    let (name, version) = value.split_once('@').ok_or_else(invalid)?;
    let tool = Tool::parse(name).ok_or_else(invalid)?;
    let version = version
        .split_once('+')
        .map_or(version, |(version, _)| version);
    if version.is_empty() {
        return Err(invalid());
    }
    Ok((tool, version.to_string()))
}

/// An unknown package manager name in `field`.
fn unknown_tool(label: &str, field: &str, name: &str) -> AppError {
    AppError::invalid_input(
        field,
        format!(
            "'{}' {field} names '{name}'; expected npm, pnpm, yarn, or bun",
            manifest_label(label)
        ),
    )
}

/// The repo-relative `package.json` path of the root directory `label`.
fn manifest_label(label: &str) -> String {
    if label == "." {
        "package.json".to_string()
    } else {
        format!("{label}/package.json")
    }
}

/// `file (tool), ...` for an error message.
fn describe_lockfiles(lockfiles: &[(Tool, &str)]) -> String {
    lockfiles
        .iter()
        .map(|(tool, name)| format!("{name} ({})", tool.name()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use rskit_errors::ErrorCode;

    use super::{Manager, Tool, detect, pinned_version};

    fn joined(argv: &[String]) -> String {
        argv.join(" ")
    }

    #[test]
    fn every_manager_runs_a_script_in_a_package_directory() {
        let run = |manager: Manager| joined(&manager.run_script("{module.root}", "build", &[]));
        assert_eq!(
            run(Manager::Npm),
            "npm --prefix {module.root} run build -- {args}"
        );
        assert_eq!(
            run(Manager::Pnpm),
            "pnpm --dir {module.root} run build {args}"
        );
        assert_eq!(
            run(Manager::YarnClassic),
            "yarn --cwd {module.root} run build {args}"
        );
        assert_eq!(
            run(Manager::YarnBerry),
            "yarn --cwd {module.root} run build {args}"
        );
        assert_eq!(
            run(Manager::Bun),
            "bun run --cwd {module.root} build {args}"
        );
        assert_eq!(
            joined(&Manager::Npm.run_script("p", "test", &["--coverage".to_string()])),
            "npm --prefix p run test -- --coverage {args}"
        );
    }

    #[test]
    fn every_manager_installs_from_its_lockfile_without_changing_it() {
        let install =
            |manager: Manager, dir| joined(&manager.in_dir(dir, manager.frozen_install()));
        assert_eq!(install(Manager::Npm, None), "npm ci");
        assert_eq!(install(Manager::Npm, Some("web")), "npm --prefix web ci");
        assert_eq!(
            install(Manager::Pnpm, Some("web")),
            "pnpm --dir web install --frozen-lockfile"
        );
        assert_eq!(
            install(Manager::YarnClassic, Some("web")),
            "yarn --cwd web install --frozen-lockfile"
        );
        assert_eq!(
            install(Manager::YarnBerry, Some("web")),
            "yarn --cwd web install --immutable"
        );
        assert_eq!(
            install(Manager::Bun, Some("web")),
            "bun install --cwd web --frozen-lockfile"
        );
    }

    #[test]
    fn every_manager_packs_into_a_directory() {
        let pack = |manager: Manager| joined(&manager.pack(std::path::Path::new("/out")));
        assert_eq!(pack(Manager::Npm), "npm pack --pack-destination /out");
        assert_eq!(pack(Manager::Pnpm), "pnpm pack --pack-destination /out");
        assert_eq!(
            pack(Manager::YarnClassic),
            "yarn pack --filename /out/package.tgz"
        );
        assert_eq!(pack(Manager::YarnBerry), "yarn pack --out /out/package.tgz");
        assert_eq!(pack(Manager::Bun), "bun pm pack --destination /out");
    }

    #[test]
    fn lockfile_refresh_skips_scripts_and_yarn_classic() {
        let refresh = |manager: Manager| manager.lockfile_refresh().map(|argv| argv.join(" "));
        assert_eq!(
            refresh(Manager::Npm).unwrap(),
            "npm install --package-lock-only --ignore-scripts"
        );
        assert_eq!(
            refresh(Manager::Pnpm).unwrap(),
            "pnpm install --lockfile-only --ignore-scripts"
        );
        assert_eq!(
            refresh(Manager::YarnBerry).unwrap(),
            "yarn install --mode=update-lockfile"
        );
        assert_eq!(
            refresh(Manager::Bun).unwrap(),
            "bun install --lockfile-only --ignore-scripts"
        );
        assert_eq!(refresh(Manager::YarnClassic), None);
    }

    #[test]
    fn manager_ids_round_trip() {
        for manager in [
            Manager::Npm,
            Manager::Pnpm,
            Manager::YarnClassic,
            Manager::YarnBerry,
            Manager::Bun,
        ] {
            assert_eq!(Manager::from_id(manager.id()), Some(manager));
        }
        assert_eq!(Manager::from_id("yarn"), None);
    }
    use crate::manifest::PackageJson;

    fn root(files: &[&str]) -> rskit_fs::TempDir {
        let dir = rskit_fs::TempDir::new().expect("tempdir");
        for file in files {
            dir.write_file(file, b"").expect("write");
        }
        dir
    }

    fn manifest(json: &str) -> PackageJson {
        rskit_codec::decode(&rskit_codec::JsonCodec::default(), json).expect("json")
    }

    #[test]
    fn lockfile_alone_selects_its_tool() {
        for (file, expected) in [
            ("package-lock.json", Manager::Npm),
            ("pnpm-lock.yaml", Manager::Pnpm),
            ("yarn.lock", Manager::YarnClassic),
            ("bun.lock", Manager::Bun),
            ("bun.lockb", Manager::Bun),
        ] {
            let dir = root(&[file]);
            assert_eq!(
                detect(dir.path(), ".", &PackageJson::default(), None).expect("detects"),
                expected,
                "{file}"
            );
        }
    }

    #[test]
    fn unlocked_roots_fall_back_to_config_files_then_npm() {
        for (files, expected) in [
            (&[][..], Manager::Npm),
            (&["pnpm-workspace.yaml"][..], Manager::Pnpm),
            (&[".yarnrc.yml"][..], Manager::YarnBerry),
        ] {
            let dir = root(files);
            assert_eq!(
                detect(dir.path(), ".", &PackageJson::default(), None).expect("detects"),
                expected,
                "{files:?}"
            );
        }
    }

    #[test]
    fn yarn_generation_follows_the_pin_then_yarnrc() {
        let dir = root(&["yarn.lock", ".yarnrc.yml"]);
        assert_eq!(
            detect(dir.path(), ".", &PackageJson::default(), None).expect("detects"),
            Manager::YarnBerry
        );
        let pinned = manifest(r#"{"packageManager":"yarn@1.22.22+sha512.abc"}"#);
        assert_eq!(
            detect(dir.path(), ".", &pinned, None).expect("detects"),
            Manager::YarnClassic
        );
        let berry = manifest(r#"{"packageManager":"yarn@4.5.0"}"#);
        let bare = root(&[]);
        assert_eq!(
            detect(bare.path(), ".", &berry, None).expect("detects"),
            Manager::YarnBerry
        );
    }

    #[test]
    fn pinned_version_belongs_to_the_detected_tool() {
        let pinned = manifest(r#"{"packageManager":"pnpm@9.1.0+sha512.abc"}"#);
        assert_eq!(
            pinned_version(".", &pinned, Manager::Pnpm)
                .expect("pin")
                .as_deref(),
            Some("9.1.0")
        );
        assert_eq!(
            pinned_version(".", &pinned, Manager::Bun).expect("pin"),
            None
        );
        assert_eq!(
            pinned_version(".", &PackageJson::default(), Manager::Npm).expect("pin"),
            None
        );
    }

    #[test]
    fn pin_wins_over_dev_engines_absence_and_agrees_with_lockfile() {
        let dir = root(&["pnpm-lock.yaml"]);
        let pinned = manifest(r#"{"packageManager":"pnpm@9.1.0"}"#);
        assert_eq!(
            detect(dir.path(), ".", &pinned, None).expect("detects"),
            Manager::Pnpm
        );
        let engine = manifest(r#"{"devEngines":{"packageManager":{"name":"pnpm"}}}"#);
        assert_eq!(
            detect(dir.path(), ".", &engine, None).expect("detects"),
            Manager::Pnpm
        );
    }

    #[test]
    fn conflicting_lockfiles_fail_with_the_fix() {
        let dir = root(&["package-lock.json", "bun.lock"]);
        let error = detect(dir.path(), "web", &PackageJson::default(), None).expect_err("conflict");
        assert_eq!(error.code(), ErrorCode::Conflict);
        let message = error.to_string();
        assert!(message.contains("'web'"), "{message}");
        assert!(message.contains("bun.lock (bun)"), "{message}");
        assert!(message.contains("delete the stale lockfiles"), "{message}");
    }

    #[test]
    fn pin_disagreeing_with_lockfile_or_engine_fails() {
        let dir = root(&["bun.lock"]);
        let pinned = manifest(r#"{"packageManager":"pnpm@9.1.0"}"#);
        let error = detect(dir.path(), ".", &pinned, None).expect_err("ambiguous");
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert!(
            error.to_string().contains("delete the stale lockfile"),
            "{error}"
        );

        let both = manifest(
            r#"{"packageManager":"pnpm@9.1.0","devEngines":{"packageManager":[{"name":"npm"}]}}"#,
        );
        let bare = root(&[]);
        let error = detect(bare.path(), ".", &both, None).expect_err("ambiguous");
        assert!(error.to_string().contains("package_manager"), "{error}");
    }

    #[test]
    fn override_wins_over_every_signal() {
        let dir = root(&["package-lock.json", "bun.lock"]);
        let pinned = manifest(r#"{"packageManager":"pnpm@9.1.0"}"#);
        assert_eq!(
            detect(dir.path(), ".", &pinned, Some(Tool::Bun)).expect("override"),
            Manager::Bun
        );
    }

    #[test]
    fn malformed_pin_and_unknown_engine_are_rejected() {
        let dir = root(&[]);
        for json in [
            r#"{"packageManager":"pnpm"}"#,
            r#"{"packageManager":"deno@2.0.0"}"#,
            r#"{"packageManager":"pnpm@"}"#,
            r#"{"devEngines":{"packageManager":{"name":"deno"}}}"#,
        ] {
            let error = detect(dir.path(), "web", &manifest(json), None).expect_err(json);
            assert_eq!(error.code(), ErrorCode::InvalidInput, "{json}");
            assert!(error.to_string().contains("web/package.json"), "{error}");
        }
    }
}
