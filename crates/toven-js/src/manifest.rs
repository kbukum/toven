//! Bounded, typed reads of `package.json` and `pnpm-workspace.yaml`.
//!
//! Only the fields discovery needs are modeled; every other field is ignored.
//! Each read is capped so a pathological file cannot exhaust memory, and a
//! malformed file fails with its repo-relative path.

use std::collections::BTreeMap;
use std::path::Path;

use rskit_codec::{JsonCodec, YamlCodec, decode};
use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::sync_io::file;
use serde::Deserialize;

/// The manifest file name of every JavaScript package.
pub(crate) const PACKAGE_JSON: &str = "package.json";

/// The pnpm workspace definition file.
pub(crate) const PNPM_WORKSPACE: &str = "pnpm-workspace.yaml";

/// Per-file read cap (1 MiB): far above any real manifest, bounded so a
/// pathological file cannot exhaust memory.
pub(crate) const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// The `workspaces` field: an array of patterns, or the object form whose
/// `packages` holds them (Yarn classic `nohoist`, Bun `catalog`, and other
/// keys are ignored).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum Workspaces {
    /// `"workspaces": ["packages/*"]`.
    List(Vec<String>),
    /// `"workspaces": { "packages": ["packages/*"], ... }`.
    Object {
        /// The member patterns.
        #[serde(default)]
        packages: Vec<String>,
    },
}

/// One `devEngines.packageManager` entry.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DevEngine {
    /// The package manager name (`pnpm`).
    pub(crate) name: String,
}

/// `devEngines.packageManager`: one entry or a list of alternatives.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum DevEngineSpec {
    /// A single entry.
    One(DevEngine),
    /// A list of alternatives; the first is the preferred one.
    Many(Vec<DevEngine>),
}

/// The `devEngines` field.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevEngines {
    /// The declared package manager.
    #[serde(default)]
    package_manager: Option<DevEngineSpec>,
}

/// The subset of `package.json` discovery reads.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PackageJson {
    /// The npm package name (`@scope/name`).
    #[serde(default)]
    pub(crate) name: Option<String>,
    /// The package version.
    #[serde(default)]
    pub(crate) version: Option<String>,
    /// Whether the package is private (`"private": true`): npm refuses to
    /// publish it.
    #[serde(default)]
    pub(crate) private: bool,
    /// The package's scripts, by name.
    #[serde(default)]
    pub(crate) scripts: BTreeMap<String, String>,
    /// Workspace member patterns (npm, Yarn, Bun).
    #[serde(default)]
    workspaces: Option<Workspaces>,
    /// The Corepack pin (`pnpm@9.1.0+sha512...`).
    #[serde(default)]
    pub(crate) package_manager: Option<String>,
    /// The `devEngines` declaration.
    #[serde(default)]
    dev_engines: Option<DevEngines>,
    /// Production dependencies.
    #[serde(default)]
    pub(crate) dependencies: BTreeMap<String, String>,
    /// Development dependencies.
    #[serde(default)]
    pub(crate) dev_dependencies: BTreeMap<String, String>,
    /// Peer dependencies.
    #[serde(default)]
    pub(crate) peer_dependencies: BTreeMap<String, String>,
    /// Optional dependencies.
    #[serde(default)]
    pub(crate) optional_dependencies: BTreeMap<String, String>,
    /// The `require` entry point.
    #[serde(default)]
    main: Option<PathTree>,
    /// The ES module entry point.
    #[serde(default)]
    module: Option<PathTree>,
    /// The type declarations entry point.
    #[serde(default)]
    types: Option<PathTree>,
    /// The legacy name for `types`.
    #[serde(default)]
    typings: Option<PathTree>,
    /// The executables the package installs.
    #[serde(default)]
    bin: Option<PathTree>,
    /// The conditional export map.
    #[serde(default)]
    exports: Option<PathTree>,
    /// The files the published package contains.
    #[serde(default)]
    files: Option<PathTree>,
}

/// A `package.json` field holding paths: a path, a list, or a map of them
/// (`exports` conditions nest). Other values (`null`, booleans) hold none.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum PathTree {
    /// One path.
    Path(String),
    /// A list of path trees.
    List(Vec<Self>),
    /// A map of path trees; the keys are not paths.
    Map(BTreeMap<String, Self>),
    /// A value with no paths.
    Other(serde::de::IgnoredAny),
}

impl PathTree {
    /// Whether any path in the tree lies under `dist`.
    fn references_dist(&self) -> bool {
        match self {
            Self::Path(path) => {
                let path = path.trim_start_matches("./");
                path == "dist" || path.starts_with("dist/")
            }
            Self::List(items) => items.iter().any(Self::references_dist),
            Self::Map(entries) => entries.values().any(Self::references_dist),
            Self::Other(_) => false,
        }
    }
}

impl PackageJson {
    /// Whether the package publishes built files from `dist`: an entry point,
    /// export, executable, or published file lies under it.
    pub(crate) fn references_dist(&self) -> bool {
        [
            &self.main,
            &self.module,
            &self.types,
            &self.typings,
            &self.bin,
            &self.exports,
            &self.files,
        ]
        .into_iter()
        .flatten()
        .any(PathTree::references_dist)
    }

    /// Whether the manifest declares a `workspaces` field.
    pub(crate) const fn declares_workspaces(&self) -> bool {
        self.workspaces.is_some()
    }

    /// The declared workspace member patterns.
    pub(crate) fn workspace_patterns(&self) -> &[String] {
        match &self.workspaces {
            Some(Workspaces::List(patterns) | Workspaces::Object { packages: patterns }) => {
                patterns
            }
            None => &[],
        }
    }

    /// The preferred `devEngines.packageManager` entry, when declared.
    pub(crate) fn dev_engine(&self) -> Option<&DevEngine> {
        match self.dev_engines.as_ref()?.package_manager.as_ref()? {
            DevEngineSpec::One(engine) => Some(engine),
            DevEngineSpec::Many(engines) => engines.first(),
        }
    }
}

/// The subset of `pnpm-workspace.yaml` discovery reads (`catalog` and other
/// settings are ignored).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct PnpmWorkspace {
    /// Workspace member patterns.
    #[serde(default)]
    pub(crate) packages: Vec<String>,
}

/// Read and parse the `package.json` at `absolute`; `label` is its
/// repo-relative path for errors.
pub(crate) fn read_package_json(absolute: &Path, label: &str) -> AppResult<PackageJson> {
    let text = read_text(absolute, label)?;
    decode::<PackageJson>(&JsonCodec::default(), strip_bom(&text))
        .map_err(|error| malformed(label, error))
}

/// Read and parse the `pnpm-workspace.yaml` at `absolute`; `label` is its
/// repo-relative path for errors. An empty file is an empty definition.
pub(crate) fn read_pnpm_workspace(absolute: &Path, label: &str) -> AppResult<PnpmWorkspace> {
    let text = read_text(absolute, label)?;
    if text.trim().is_empty() {
        return Ok(PnpmWorkspace::default());
    }
    decode::<PnpmWorkspace>(&YamlCodec, strip_bom(&text)).map_err(|error| malformed(label, error))
}

/// Read a manifest under the size cap, naming the file in every error.
fn read_text(absolute: &Path, label: &str) -> AppResult<String> {
    file::read_string_bounded(absolute, MAX_MANIFEST_BYTES).map_err(|error| {
        if file::is_file_too_large_error(&error) {
            AppError::invalid_input(
                "ecosystems.js",
                format!("'{label}' exceeds the {MAX_MANIFEST_BYTES}-byte manifest limit"),
            )
            .with_cause(error)
        } else {
            AppError::new(error.code(), format!("failed to read '{label}'")).with_cause(error)
        }
    })
}

/// A malformed-manifest error naming the file.
fn malformed(label: &str, error: AppError) -> AppError {
    AppError::new(
        ErrorCode::InvalidFormat,
        format!("'{label}' is not valid: {error}"),
    )
    .with_cause(error)
}

/// Drop a UTF-8 byte-order mark, which some editors write and JSON forbids.
fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use rskit_errors::ErrorCode;

    use super::{MAX_MANIFEST_BYTES, read_package_json, read_pnpm_workspace};

    fn parsed(text: &str) -> super::PackageJson {
        let dir = rskit_fs::TempDir::new().expect("tempdir");
        let path = write(dir.path(), "package.json", text);
        read_package_json(&path, "package.json").expect("parse")
    }

    #[test]
    fn built_output_is_found_in_any_entry_point_field() {
        for json in [
            r#"{"main":"dist/index.js"}"#,
            r#"{"module":"./dist/index.mjs"}"#,
            r#"{"types":"dist/index.d.ts"}"#,
            r#"{"bin":{"cli":"./dist/cli.js"}}"#,
            r#"{"files":["dist"]}"#,
            r#"{"exports":{".":{"import":{"types":"./dist/a.d.ts","default":"./dist/a.js"}}}}"#,
            r#"{"exports":["./dist/a.js"]}"#,
        ] {
            assert!(parsed(json).references_dist(), "{json}");
        }
        for json in [
            r#"{"main":"src/index.ts"}"#,
            r#"{"exports":{".":"./src/index.ts","./package.json":null}}"#,
            r#"{"files":["distribution"],"bin":"cli.js"}"#,
            r#"{"name":"x"}"#,
        ] {
            assert!(!parsed(json).references_dist(), "{json}");
        }
    }

    fn write(dir: &std::path::Path, name: &str, text: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, text).expect("write");
        path
    }

    #[test]
    fn reads_the_fields_discovery_needs() {
        let dir = tempfile_dir();
        let path = write(
            dir.path(),
            "package.json",
            "\u{feff}{\"name\":\"@acme/app\",\"version\":\"1.0.0\",\"private\":true,\
             \"scripts\":{\"build\":\"tsc\"},\"workspaces\":{\"packages\":[\"packages/*\"],\
             \"catalog\":{\"react\":\"^19\"}},\"devEngines\":{\"packageManager\":\
             [{\"name\":\"pnpm\",\"version\":\"^9\"},{\"name\":\"npm\"}]},\
             \"devDependencies\":{\"@acme/lib\":\"workspace:*\"}}",
        );
        let manifest = read_package_json(&path, "package.json").expect("parses");
        assert_eq!(manifest.name.as_deref(), Some("@acme/app"));
        assert_eq!(manifest.workspace_patterns(), ["packages/*"]);
        assert_eq!(manifest.dev_engine().map(|e| e.name.as_str()), Some("pnpm"));
        assert_eq!(manifest.scripts.keys().collect::<Vec<_>>(), ["build"]);
        assert_eq!(manifest.dev_dependencies["@acme/lib"], "workspace:*");
    }

    #[test]
    fn malformed_manifest_names_the_file() {
        let dir = tempfile_dir();
        let path = write(dir.path(), "package.json", "{\"name\": ");
        let error = read_package_json(&path, "web/package.json").expect_err("malformed");
        assert_eq!(error.code(), ErrorCode::InvalidFormat);
        assert!(error.to_string().contains("web/package.json"), "{error}");
    }

    #[test]
    fn oversized_manifest_is_rejected() {
        let dir = tempfile_dir();
        let big = format!(
            "{{\"name\":\"x\",\"description\":\"{}\"}}",
            "a".repeat(usize::try_from(MAX_MANIFEST_BYTES).unwrap())
        );
        let path = write(dir.path(), "package.json", &big);
        let error = read_package_json(&path, "package.json").expect_err("too large");
        assert!(error.to_string().contains("manifest limit"), "{error}");
    }

    #[test]
    fn pnpm_workspace_reads_packages_and_tolerates_empty() {
        let dir = tempfile_dir();
        let path = write(
            dir.path(),
            "pnpm-workspace.yaml",
            "packages:\n  - 'packages/*'\n  - '!packages/skip'\ncatalog:\n  react: ^19\n",
        );
        let workspace = read_pnpm_workspace(&path, "pnpm-workspace.yaml").expect("parses");
        assert_eq!(workspace.packages, ["packages/*", "!packages/skip"]);
        let empty = write(dir.path(), "empty.yaml", "\n");
        assert!(
            read_pnpm_workspace(&empty, "empty.yaml")
                .expect("empty")
                .packages
                .is_empty()
        );
    }

    fn tempfile_dir() -> rskit_fs::TempDir {
        rskit_fs::TempDir::new().expect("tempdir")
    }
}
