//! Intra-ecosystem edges and external cache roots from package dependencies.
//!
//! A dependency becomes an edge when it resolves to a discovered package:
//!
//! - `workspace:` always links inside the declaring root, by name (`*`, `^`,
//!   `~`, or a range), by alias (`workspace:other@*`), or by relative path. A
//!   missing target is an error, because the package manager would fail too.
//! - `file:`, `link:`, `portal:`, and bare relative paths resolve against the
//!   declaring package's directory. A path at a package root is an edge; any
//!   other path outside the declaring package (a sibling checkout, a packed
//!   tarball) becomes an external cache root, because its content changes what
//!   the package builds against.
//! - A plain range links to a package of the same root with that name when
//!   the package's version satisfies the range. pnpm resolves plain ranges from
//!   the registry unless `link-workspace-packages` is on, so for pnpm this edge
//!   can be an over-approximation; it only adds ordering and invalidation, never
//!   drops it. This is a "for now" choice that can be refined per manager.
//! - `catalog:`, `npm:` aliases, Git, and URL specs stay external.
//!
//! `devDependencies` give dev edges; `dependencies`, `peerDependencies`, and
//! `optionalDependencies` give normal edges. When a package declares the same
//! target as both, the normal edge wins.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use rskit_errors::{AppError, AppResult, ErrorCode};
use rskit_fs::sync_io::file;
use toven_model::{DepKind, Edge, ModuleRef};

use crate::discovery::scan::Package;
use crate::range;
use crate::roots;

/// The protocol linking a workspace package.
const WORKSPACE: &str = "workspace:";

/// Protocols whose value is a filesystem path.
const PATH_PROTOCOLS: &[&str] = &["file:", "link:", "portal:"];

/// What one dependency spec points at.
#[derive(Debug, Eq, PartialEq)]
enum Target<'a> {
    /// A workspace package of the declaring root, by name.
    Workspace(&'a str),
    /// A path relative to the declaring package.
    Path(&'a str),
    /// A registry range that may link a same-root package.
    Range(&'a str),
    /// Anything that never links a local package.
    External,
}

/// Classify the dependency `name` declared with `spec`.
fn classify<'a>(name: &'a str, spec: &'a str) -> Target<'a> {
    let spec = spec.trim();
    if let Some(rest) = spec.strip_prefix(WORKSPACE) {
        if rest.starts_with("./") || rest.starts_with("../") {
            return Target::Path(rest);
        }
        if let Some(at) = rest.rfind('@').filter(|&at| at > 0) {
            return Target::Workspace(&rest[..at]);
        }
        if rest.contains('/') && !rest.starts_with('@') {
            return Target::Path(rest);
        }
        return Target::Workspace(name);
    }
    if let Some(path) = PATH_PROTOCOLS
        .iter()
        .find_map(|protocol| spec.strip_prefix(protocol))
    {
        return Target::Path(path);
    }
    if spec.starts_with("./") || spec.starts_with("../") {
        return Target::Path(spec);
    }
    if spec.contains(':') || spec.contains('/') {
        return Target::External;
    }
    Target::Range(spec)
}

/// The registry range `name` is declared with, or `None` when `spec` is a
/// `workspace:`, path, alias, Git, or URL spec that release leaves unchanged.
pub(crate) fn plain_range<'a>(name: &'a str, spec: &'a str) -> Option<&'a str> {
    match classify(name, spec) {
        Target::Range(range) => Some(range),
        _ => None,
    }
}

/// Link every package's dependencies into edges, and record path targets
/// outside the module graph as external cache roots on the declaring module.
///
/// # Errors
/// A `workspace:` dependency naming a package that is not in its root.
pub(crate) fn link(
    project_root: &Path,
    packages: &mut [Package],
    warnings: &mut Vec<String>,
) -> AppResult<Vec<Edge>> {
    let by_name: BTreeMap<(usize, &str), usize> = packages
        .iter()
        .enumerate()
        .filter_map(|(index, package)| {
            package
                .manifest
                .name
                .as_deref()
                .map(|name| ((package.root, name), index))
        })
        .collect();
    let by_dir: BTreeMap<&Path, usize> = packages
        .iter()
        .enumerate()
        .map(|(index, package)| (package.dir.as_path(), index))
        .collect();

    let mut kinds: BTreeMap<(usize, usize), DepKind> = BTreeMap::new();
    let mut externals: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut incomplete: Vec<usize> = Vec::new();
    for (from, package) in packages.iter().enumerate() {
        let manifest = &package.manifest;
        let groups = [
            (&manifest.dependencies, DepKind::Normal),
            (&manifest.peer_dependencies, DepKind::Normal),
            (&manifest.optional_dependencies, DepKind::Normal),
            (&manifest.dev_dependencies, DepKind::Dev),
        ];
        for (dependencies, kind) in groups {
            for (name, spec) in dependencies {
                let to = match classify(name, spec) {
                    Target::Workspace(target) => Some(
                        by_name
                            .get(&(package.root, target))
                            .copied()
                            .ok_or_else(|| missing_workspace(package, name, spec))?,
                    ),
                    Target::Range(range) => by_name
                        .get(&(package.root, name.as_str()))
                        .copied()
                        .filter(|&to| {
                            packages[to]
                                .manifest
                                .version
                                .as_deref()
                                .is_some_and(|version| range::satisfies(version, range))
                        }),
                    Target::Path(path) => {
                        let Some(target) = resolve_path(&package.dir, path) else {
                            warnings.push(format!(
                                "module '{}': dependency '{name}' path '{spec}' is not \
                                 project-relative; caching is disabled for this module",
                                package.module.id
                            ));
                            incomplete.push(from);
                            continue;
                        };
                        if let Some(&to) = by_dir.get(target.as_path()) {
                            Some(to)
                        } else {
                            if !target.starts_with(&package.dir) || target.starts_with("..") {
                                let label = path_label(&target);
                                if exists(project_root, &target)? {
                                    externals.entry(from).or_default().push(label);
                                } else {
                                    warnings.push(format!(
                                        "module '{}': dependency '{name}' path '{label}' does not \
                                         exist yet; caching is disabled for this module",
                                        package.module.id
                                    ));
                                    incomplete.push(from);
                                }
                            }
                            None
                        }
                    }
                    Target::External => None,
                };
                if let Some(to) = to.filter(|&to| to != from) {
                    let entry = kinds.entry((from, to)).or_insert(kind);
                    if kind == DepKind::Normal {
                        *entry = DepKind::Normal;
                    }
                }
            }
        }
    }

    for (index, roots) in externals {
        packages[index].module.external_roots = outermost(roots);
    }
    for index in incomplete {
        packages[index].module.cache_inputs_complete = false;
    }
    let id = |index: usize| -> ModuleRef { packages[index].module.id.clone() };
    let mut edges: Vec<Edge> = kinds
        .into_iter()
        .map(|((from, to), kind)| Edge::new(id(from), id(to), kind))
        .collect();
    edges.sort();
    Ok(edges)
}

/// The project-relative target of `path` declared in `dir`, normalized
/// lexically (it may start with `..`), or `None` for an absolute path.
fn resolve_path(dir: &Path, path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    if path.has_root() {
        return None;
    }
    let mut parts: Vec<Component<'_>> = dir.components().collect();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(parts.last(), Some(Component::Normal(_))) {
                    parts.pop();
                } else {
                    parts.push(Component::ParentDir);
                }
            }
            Component::Normal(_) => parts.push(component),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.iter().collect())
}

/// The `/`-separated label of a project-relative path.
fn path_label(path: &Path) -> String {
    roots::label(path)
}

/// Whether the project-relative `target` exists as a file or directory.
fn exists(project_root: &Path, target: &Path) -> AppResult<bool> {
    let absolute = project_root.join(target);
    Ok(absolute.is_dir() || file::exists(&absolute)?)
}

/// Keep only the outermost of the given paths, sorted, so a subtree is hashed
/// once.
fn outermost(mut roots: Vec<String>) -> Vec<String> {
    roots.sort();
    roots.dedup();
    let mut kept: Vec<String> = Vec::new();
    for root in roots {
        if !kept
            .iter()
            .any(|outer| Path::new(&root).starts_with(Path::new(outer)))
        {
            kept.push(root);
        }
    }
    kept
}

/// A `workspace:` dependency whose target is not a member of the root.
fn missing_workspace(package: &Package, name: &str, spec: &str) -> AppError {
    AppError::new(
        ErrorCode::NotFound,
        format!(
            "'{}' depends on '{name}' through '{spec}', but no package of that name is in its \
             JavaScript root; add the package to the root's workspaces",
            roots::manifest_label(&package.dir)
        ),
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Target, classify, outermost, resolve_path};

    #[test]
    fn classifies_every_spec_shape() {
        assert_eq!(classify("a", "workspace:*"), Target::Workspace("a"));
        assert_eq!(classify("a", "workspace:^1.0.0"), Target::Workspace("a"));
        assert_eq!(
            classify("a", "workspace:@acme/b@*"),
            Target::Workspace("@acme/b")
        );
        assert_eq!(classify("a", "workspace:../b"), Target::Path("../b"));
        assert_eq!(classify("a", "workspace:libs/b"), Target::Path("libs/b"));
        assert_eq!(classify("a", "file:../b.tgz"), Target::Path("../b.tgz"));
        assert_eq!(classify("a", "link:../b"), Target::Path("../b"));
        assert_eq!(classify("a", "portal:../b"), Target::Path("../b"));
        assert_eq!(classify("a", "../b"), Target::Path("../b"));
        assert_eq!(classify("a", "^1.2.0"), Target::Range("^1.2.0"));
        for external in [
            "catalog:",
            "catalog:react18",
            "npm:b@^1",
            "github:a/b",
            "a/b",
            "https://x",
        ] {
            assert_eq!(classify("a", external), Target::External, "{external}");
        }
    }

    #[test]
    fn paths_resolve_lexically_from_the_declaring_dir() {
        assert_eq!(
            resolve_path(Path::new("web"), "../target/std.tgz"),
            Some(PathBuf::from("target/std.tgz"))
        );
        assert_eq!(
            resolve_path(Path::new(""), "../sibling"),
            Some(PathBuf::from("../sibling"))
        );
        assert_eq!(
            resolve_path(Path::new("a/b"), "./c/../d"),
            Some(PathBuf::from("a/b/d"))
        );
        assert_eq!(resolve_path(Path::new("web"), "/abs/x"), None);
    }

    #[test]
    fn outermost_drops_nested_roots() {
        assert_eq!(
            outermost(vec![
                "../kit/ai".to_string(),
                "../kit".to_string(),
                "target/a.tgz".to_string(),
                "../kit".to_string(),
            ]),
            ["../kit", "target/a.tgz"]
        );
    }
}
