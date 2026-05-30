//! Cache input hashing for module sources and shared workspace files.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use rskit_git::{
    Differ, EntryKind, EntryState, Executor, IndexReader, Repository, StatusEntry, TreeReader,
};

use crate::core::{AppError, AppResult, ErrorCode, Module, ModuleId, Workspace};

/// Source hash inputs for all discovered modules.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SourceHashes {
    /// Hash of tracked and dirty files not owned by any module.
    pub global_hash: String,
    /// Per-module source hash with the global hash folded in.
    pub modules: BTreeMap<ModuleId, String>,
}

/// Compute source hashes for every discovered module.
pub fn compute_source_hashes(workspace: &Workspace, modules: &[Module]) -> AppResult<SourceHashes> {
    let repo = rskit_git::discover(&workspace.root).map_err(|error| {
        AppError::invalid_input(
            "workspace.root",
            format!(
                "failed to discover git repository from '{}'",
                workspace.root.display()
            ),
        )
        .with_cause(error)
    })?;
    let workspace_prefix = workspace_prefix(repo.root(), &workspace.root)?;
    let ignore = IgnoreMatcher::new(&repo, &workspace_prefix)?;
    let owners = PathOwners::new(modules);
    let mut buckets = HashBuckets::new(modules);

    collect_head_entries(
        &repo,
        &workspace_prefix,
        Path::new(""),
        &owners,
        &mut buckets,
    )?;
    collect_worktree_status(
        &repo,
        &workspace.root,
        &workspace_prefix,
        &ignore,
        &owners,
        &mut buckets,
    )?;

    Ok(buckets.finish())
}

/// Compute a hash for shared inputs that affect every module using a command preset.
pub fn compute_shared_inputs_hash(
    workspace: &Workspace,
    shared_inputs: &[String],
) -> AppResult<String> {
    let repo = rskit_git::discover(&workspace.root).map_err(|error| {
        AppError::invalid_input(
            "workspace.root",
            format!(
                "failed to discover git repository from '{}'",
                workspace.root.display()
            ),
        )
        .with_cause(error)
    })?;
    let workspace_prefix = workspace_prefix(repo.root(), &workspace.root)?;
    let ignore = IgnoreMatcher::new(&repo, &workspace_prefix)?;
    let mut inputs = shared_inputs
        .iter()
        .map(|input| normalize_path(Path::new(input)))
        .collect::<Vec<_>>();
    inputs.sort();
    inputs.dedup();

    let mut hasher = blake3::Hasher::new();
    hash_field(&mut hasher, b"shared-inputs-v1");
    for input in &inputs {
        if is_internal_cache_path(input) {
            continue;
        }
        hash_field(&mut hasher, b"shared-input");
        hash_field(&mut hasher, path_to_string(input).as_bytes());
        hash_field(
            &mut hasher,
            hash_worktree_path(&workspace.root, input, &ignore)?.as_bytes(),
        );
        collect_shared_status_entries(
            &repo,
            &workspace.root,
            &workspace_prefix,
            &ignore,
            input,
            &mut hasher,
        )?;
    }
    Ok(hasher.finalize().to_hex().to_string())
}

struct HashBuckets {
    global: blake3::Hasher,
    modules: BTreeMap<ModuleId, blake3::Hasher>,
}

impl HashBuckets {
    fn new(modules: &[Module]) -> Self {
        let modules = modules
            .iter()
            .map(|module| {
                let mut hasher = blake3::Hasher::new();
                hash_field(&mut hasher, b"module");
                hash_field(&mut hasher, module.name.as_str().as_bytes());
                (module.name.clone(), hasher)
            })
            .collect();
        let mut global = blake3::Hasher::new();
        hash_field(&mut global, b"global");
        Self { global, modules }
    }

    fn update(&mut self, owner: Option<&ModuleId>, tag: &str, path: &Path, fields: &[String]) {
        let hasher = owner
            .and_then(|owner| self.modules.get_mut(owner))
            .unwrap_or(&mut self.global);
        hash_field(hasher, tag.as_bytes());
        hash_field(hasher, path_to_string(path).as_bytes());
        for field in fields {
            hash_field(hasher, field.as_bytes());
        }
    }

    fn finish(self) -> SourceHashes {
        let global_hash = self.global.finalize().to_hex().to_string();
        let modules = self
            .modules
            .into_iter()
            .map(|(module, hasher)| {
                let mut combined = blake3::Hasher::new();
                hash_field(&mut combined, b"module-source-v1");
                hash_field(&mut combined, global_hash.as_bytes());
                hash_field(&mut combined, hasher.finalize().to_hex().as_bytes());
                (module, combined.finalize().to_hex().to_string())
            })
            .collect();
        SourceHashes {
            global_hash,
            modules,
        }
    }
}

struct PathOwners {
    roots: BTreeMap<ModuleId, ModuleRoot>,
}

impl PathOwners {
    fn new(modules: &[Module]) -> Self {
        let roots = modules
            .iter()
            .map(|module| {
                (
                    module.name.clone(),
                    ModuleRoot {
                        root: normalize_path(&module.root),
                        source_patterns: module.source_patterns.clone(),
                    },
                )
            })
            .collect();
        Self { roots }
    }

    fn owner(&self, path: &Path) -> Option<&ModuleId> {
        let path = normalize_path(path);
        if path.components().count() == 1 {
            return None;
        }
        self.roots
            .iter()
            .filter(|(_, root)| path_matches_module(&path, root))
            .max_by_key(|(_, root)| root.root.components().count())
            .map(|(module, _)| module)
    }
}

struct ModuleRoot {
    root: PathBuf,
    source_patterns: Vec<String>,
}

struct IgnoreMatcher {
    ignored: BTreeSet<PathBuf>,
}

impl IgnoreMatcher {
    fn new(repo: &rskit_git::Repo, workspace_prefix: &Path) -> AppResult<Self> {
        let output = git_output_bytes(
            repo,
            &[
                "status",
                "--ignored",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
            ],
        )?;
        let mut ignored = BTreeSet::new();
        for record in output
            .split(|byte| *byte == 0)
            .filter(|record| record.starts_with(b"!! "))
        {
            let path = std::str::from_utf8(&record[3..]).map_err(|error| {
                AppError::new(
                    ErrorCode::Internal,
                    format!(
                        "git listed a non-UTF-8 ignored path in '{}'",
                        repo.root().display()
                    ),
                )
                .with_cause(error)
            })?;
            let repo_path = normalize_path(Path::new(path));
            if workspace_prefix.as_os_str().is_empty() {
                ignored.insert(repo_path);
            } else if let Ok(workspace_path) = repo_path.strip_prefix(workspace_prefix) {
                ignored.insert(normalize_path(workspace_path));
            }
        }
        Ok(Self { ignored })
    }

    fn is_ignored(&self, path: &Path) -> bool {
        let path = normalize_path(path);
        self.ignored
            .iter()
            .any(|ignored| &path == ignored || path.starts_with(ignored))
    }
}

fn collect_head_entries(
    repo: &rskit_git::Repo,
    workspace_prefix: &Path,
    relative: &Path,
    owners: &PathOwners,
    buckets: &mut HashBuckets,
) -> AppResult<()> {
    let repo_path = join_repo_path(workspace_prefix, relative)?;
    let entries = repo
        .list_entries("HEAD", &path_to_string(&repo_path))
        .map_err(|error| {
            AppError::invalid_input("git", "failed to list HEAD tree entries").with_cause(error)
        })?;

    for entry in entries {
        let path = relative.join(&entry.name);
        if is_internal_cache_path(&path) {
            continue;
        }
        if entry.kind == EntryKind::Tree {
            collect_head_entries(repo, workspace_prefix, &path, owners, buckets)?;
        } else {
            let owner = owners.owner(&path);
            buckets.update(
                owner,
                "head",
                &path,
                &[
                    entry.kind.to_string(),
                    entry.filemode.to_string(),
                    entry.oid.to_string(),
                ],
            );
        }
    }
    Ok(())
}

fn collect_worktree_status(
    repo: &rskit_git::Repo,
    workspace_root: &Path,
    workspace_prefix: &Path,
    ignore: &IgnoreMatcher,
    owners: &PathOwners,
    buckets: &mut HashBuckets,
) -> AppResult<()> {
    let mut seen = BTreeSet::new();
    for entry in repo.status().map_err(|error| {
        AppError::invalid_input("workspace.root", "failed to read git status").with_cause(error)
    })? {
        let Some(path) = workspace_relative_status_path(workspace_prefix, &entry) else {
            continue;
        };
        if should_skip_worktree_path(&path, ignore) {
            continue;
        }
        if !seen.insert((path.clone(), entry.state.to_string())) {
            continue;
        }
        let owner = owners.owner(&path);
        if entry.state == EntryState::Staged {
            buckets.update(
                owner,
                "index",
                &path,
                &[
                    entry.state.to_string(),
                    hash_index_path(repo, workspace_prefix, &path)?,
                ],
            );
        }
        let content_hash = hash_worktree_path(workspace_root, &path, ignore)?;
        buckets.update(
            owner,
            "worktree",
            &path,
            &[entry.state.to_string(), content_hash],
        );
    }
    Ok(())
}

fn collect_shared_status_entries(
    repo: &rskit_git::Repo,
    workspace_root: &Path,
    workspace_prefix: &Path,
    ignore: &IgnoreMatcher,
    input: &Path,
    hasher: &mut blake3::Hasher,
) -> AppResult<()> {
    let mut seen = BTreeSet::new();
    for entry in repo.status().map_err(|error| {
        AppError::invalid_input("workspace.root", "failed to read git status").with_cause(error)
    })? {
        let Some(path) = workspace_relative_status_path(workspace_prefix, &entry) else {
            continue;
        };
        if !path_is_or_is_inside(&path, input) || should_skip_worktree_path(&path, ignore) {
            continue;
        }
        if !seen.insert((path.clone(), entry.state.to_string())) {
            continue;
        }
        hash_field(hasher, b"shared-status");
        hash_field(hasher, path_to_string(&path).as_bytes());
        hash_field(hasher, entry.state.to_string().as_bytes());
        if entry.state == EntryState::Staged {
            hash_field(
                hasher,
                hash_index_path(repo, workspace_prefix, &path)?.as_bytes(),
            );
        }
        hash_field(
            hasher,
            hash_worktree_path(workspace_root, &path, ignore)?.as_bytes(),
        );
    }
    Ok(())
}

fn should_skip_worktree_path(path: &Path, ignore: &IgnoreMatcher) -> bool {
    is_internal_cache_path(path) || ignore.is_ignored(path)
}

fn is_internal_cache_path(path: &Path) -> bool {
    let path = normalize_path(path);
    path == Path::new(".toven/cache") || path.starts_with(".toven/cache")
}

fn workspace_relative_status_path(workspace_prefix: &Path, entry: &StatusEntry) -> Option<PathBuf> {
    let repo_path = PathBuf::from(&entry.path);
    if workspace_prefix.as_os_str().is_empty() {
        Some(normalize_path(&repo_path))
    } else {
        repo_path
            .strip_prefix(workspace_prefix)
            .map(normalize_path)
            .ok()
    }
}

fn hash_index_path(
    repo: &rskit_git::Repo,
    workspace_prefix: &Path,
    relative_path: &Path,
) -> AppResult<String> {
    let repo_path = join_repo_path(workspace_prefix, relative_path)?;
    let Some(entry) = repo
        .index_entry(&path_to_string(&repo_path))
        .map_err(|error| {
            AppError::invalid_input("git", "failed to read git index").with_cause(error)
        })?
    else {
        return Ok("missing".to_string());
    };
    let mut hasher = blake3::Hasher::new();
    hash_field(&mut hasher, b"index");
    hash_field(&mut hasher, entry.kind.to_string().as_bytes());
    hash_field(&mut hasher, entry.filemode.to_string().as_bytes());
    hash_field(&mut hasher, entry.oid.to_string().as_bytes());
    Ok(hasher.finalize().to_hex().to_string())
}

fn hash_worktree_path(
    workspace_root: &Path,
    relative_path: &Path,
    ignore: &IgnoreMatcher,
) -> AppResult<String> {
    let path = workspace_root.join(relative_path);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = fs::read_link(&path).map_err(|error| {
                AppError::new(
                    ErrorCode::Internal,
                    format!("failed to read symlink '{}'", path.display()),
                )
                .with_cause(error)
            })?;
            let mut hasher = blake3::Hasher::new();
            hash_field(&mut hasher, b"symlink");
            hash_field(&mut hasher, target.to_string_lossy().as_bytes());
            Ok(hasher.finalize().to_hex().to_string())
        }
        Ok(metadata) if metadata.is_file() => hash_file(&path),
        Ok(metadata) if metadata.is_dir() && path.join(".git").exists() => {
            hash_nested_git_repo(&path)
        }
        Ok(metadata) if metadata.is_dir() => hash_directory(workspace_root, relative_path, ignore),
        Ok(_) => {
            let mut hasher = blake3::Hasher::new();
            hash_field(&mut hasher, b"special");
            Ok(hasher.finalize().to_hex().to_string())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok("missing".to_string()),
        Err(error) => Err(AppError::new(
            ErrorCode::Internal,
            format!("failed to inspect changed path '{}'", path.display()),
        )
        .with_cause(error)),
    }
}

fn hash_file(path: &Path) -> AppResult<String> {
    let mut file = fs::File::open(path).map_err(|error| {
        AppError::new(
            ErrorCode::Internal,
            format!("failed to open changed file '{}'", path.display()),
        )
        .with_cause(error)
    })?;
    let mut hasher = blake3::Hasher::new();
    hash_field(&mut hasher, b"file");
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                format!("failed to read changed file '{}'", path.display()),
            )
            .with_cause(error)
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn hash_nested_git_repo(path: &Path) -> AppResult<String> {
    let repo = rskit_git::open(path).map_err(|error| {
        AppError::invalid_input(
            "git",
            format!("failed to open nested git repository '{}'", path.display()),
        )
        .with_cause(error)
    })?;
    let head = git_output(&repo, &["rev-parse", "HEAD"])?;
    let status = git_output_bytes(
        &repo,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )?;
    let diff = git_output_bytes(&repo, &["diff", "--binary", "HEAD", "--"])?;
    let mut hasher = blake3::Hasher::new();
    hash_field(&mut hasher, b"nested-git");
    hash_field(&mut hasher, head.as_bytes());
    hash_field(&mut hasher, &status);
    hash_field(&mut hasher, &diff);
    hash_nested_untracked_files(&repo, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn git_output(repo: &rskit_git::Repo, args: &[&str]) -> AppResult<String> {
    String::from_utf8(git_output_bytes(repo, args)?).map_err(|error| {
        AppError::new(
            ErrorCode::Internal,
            format!("git output in '{}' was not UTF-8", repo.root().display()),
        )
        .with_cause(error)
    })
}

fn git_output_bytes(repo: &rskit_git::Repo, args: &[&str]) -> AppResult<Vec<u8>> {
    repo.exec(args).map_err(|error| {
        AppError::new(
            ErrorCode::Internal,
            format!("failed to run git in '{}'", repo.root().display()),
        )
        .with_cause(error)
    })
}

fn hash_nested_untracked_files(
    repo: &rskit_git::Repo,
    hasher: &mut blake3::Hasher,
) -> AppResult<()> {
    let output = git_output_bytes(repo, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for raw_path in output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let relative = std::str::from_utf8(raw_path).map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                format!("git listed a non-UTF-8 path in '{}'", repo.root().display()),
            )
            .with_cause(error)
        })?;
        let relative_path = Path::new(relative);
        hash_field(hasher, b"nested-untracked");
        hash_field(hasher, relative.as_bytes());
        hash_field(
            hasher,
            hash_nested_untracked_path(&repo.root().join(relative_path))?.as_bytes(),
        );
    }
    Ok(())
}

fn hash_nested_untracked_path(path: &Path) -> AppResult<String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        AppError::new(
            ErrorCode::Internal,
            format!("failed to inspect untracked path '{}'", path.display()),
        )
        .with_cause(error)
    })?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(path).map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                format!("failed to read symlink '{}'", path.display()),
            )
            .with_cause(error)
        })?;
        let mut hasher = blake3::Hasher::new();
        hash_field(&mut hasher, b"symlink");
        hash_field(&mut hasher, target.to_string_lossy().as_bytes());
        return Ok(hasher.finalize().to_hex().to_string());
    }
    hash_file(path)
}

fn hash_directory(
    workspace_root: &Path,
    relative_path: &Path,
    ignore: &IgnoreMatcher,
) -> AppResult<String> {
    let path = workspace_root.join(relative_path);
    let mut hasher = blake3::Hasher::new();
    hash_field(&mut hasher, b"directory");
    hash_directory_into(workspace_root, &path, ignore, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn hash_directory_into(
    workspace_root: &Path,
    path: &Path,
    ignore: &IgnoreMatcher,
    hasher: &mut blake3::Hasher,
) -> AppResult<()> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                format!("failed to read directory '{}'", path.display()),
            )
            .with_cause(error)
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                format!("failed to read directory '{}'", path.display()),
            )
            .with_cause(error)
        })?;
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        let rel = path.strip_prefix(workspace_root).unwrap_or(&path);
        if should_skip_worktree_path(rel, ignore) {
            continue;
        }
        hash_field(hasher, path_to_string(rel).as_bytes());
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            AppError::new(
                ErrorCode::Internal,
                format!("failed to inspect '{}'", path.display()),
            )
            .with_cause(error)
        })?;
        if metadata.file_type().is_symlink() {
            hash_field(
                hasher,
                hash_worktree_path(workspace_root, rel, ignore)?.as_bytes(),
            );
        } else if metadata.is_dir() && path.join(".git").exists() {
            hash_field(hasher, hash_nested_git_repo(&path)?.as_bytes());
        } else if metadata.is_dir() {
            hash_field(hasher, b"directory");
            hash_directory_into(workspace_root, &path, ignore, hasher)?;
        } else if metadata.is_file() {
            hash_field(hasher, hash_file(&path)?.as_bytes());
        } else {
            hash_field(hasher, b"special");
        }
    }
    Ok(())
}

fn workspace_prefix(repo_root: &Path, workspace_root: &Path) -> AppResult<PathBuf> {
    rskit_git::repo_relative_path(repo_root, workspace_root).map_err(|error| {
        AppError::invalid_input("workspace.root", error.message.clone()).with_cause(error)
    })
}

fn join_repo_path(workspace_prefix: &Path, relative: &Path) -> AppResult<PathBuf> {
    rskit_git::join_repo_path(workspace_prefix, relative)
}

fn path_matches_module(path: &Path, module: &ModuleRoot) -> bool {
    let root = &module.root;
    if root.as_os_str().is_empty() || root == Path::new(".") {
        if module.source_patterns.is_empty() {
            return true;
        }
        return module
            .source_patterns
            .iter()
            .any(|pattern| path_matches_source_pattern(path, pattern));
    }
    path == root || path.starts_with(root)
}

fn path_matches_source_pattern(path: &Path, pattern: &str) -> bool {
    let pattern = Path::new(pattern);
    let Some(prefix) = pattern
        .to_string_lossy()
        .strip_suffix("/**")
        .map(PathBuf::from)
    else {
        return path == normalize_path(pattern);
    };
    path == prefix || path.starts_with(prefix)
}

fn path_is_or_is_inside(path: &Path, parent: &Path) -> bool {
    path == parent || path.starts_with(parent)
}

fn normalize_path(path: &Path) -> PathBuf {
    if path.as_os_str().is_empty() {
        return PathBuf::new();
    }
    path.components().collect()
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn hash_field(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_be_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Command,
    };

    use crate::{
        cache::input::{compute_shared_inputs_hash, compute_source_hashes},
        core::{Module, ModuleId, Workspace},
    };

    #[test]
    fn staged_index_content_changes_module_hash_when_worktree_matches_head() {
        let root = rskit_testutil::test_workspace!("cache-staged-index-module");
        let repo = root.path().join("repo");
        fs::create_dir_all(repo.join("src")).expect("create repo tree");
        fs::write(repo.join("src/lib.rs"), "base\n").expect("write source");
        init_git_repo(&repo);
        let workspace = workspace(&repo);
        let modules = [module("fixture", "src")];

        let first = module_hash_with_staged_content(&repo, &workspace, &modules, "one\n");
        let second = module_hash_with_staged_content(&repo, &workspace, &modules, "two\n");

        assert_ne!(first, second);
    }

    #[test]
    fn staged_index_content_changes_shared_input_hash_when_worktree_matches_head() {
        let root = rskit_testutil::test_workspace!("cache-staged-index-shared");
        let repo = root.path().join("repo");
        fs::create_dir_all(&repo).expect("create repo");
        fs::write(repo.join("shared.lock"), "base\n").expect("write shared input");
        init_git_repo(&repo);
        let workspace = workspace(&repo);

        let first = shared_hash_with_staged_content(&repo, &workspace, "one\n");
        let second = shared_hash_with_staged_content(&repo, &workspace, "two\n");

        assert_ne!(first, second);
    }

    #[test]
    fn ignored_files_do_not_change_shared_directory_hash() {
        let root = rskit_testutil::test_workspace!("cache-ignored-shared-directory");
        let repo = root.path().join("repo");
        fs::create_dir_all(repo.join("src")).expect("create repo tree");
        fs::write(repo.join("src/lib.rs"), "base\n").expect("write source");
        fs::write(repo.join(".gitignore"), "*.ignored\n").expect("write gitignore");
        init_git_repo(&repo);
        let workspace = workspace(&repo);

        let before = compute_shared_inputs_hash(&workspace, &["src".to_string()])
            .expect("shared input hash computes");
        fs::write(repo.join("src/generated.ignored"), "ignored\n").expect("write ignored file");
        let after = compute_shared_inputs_hash(&workspace, &["src".to_string()])
            .expect("shared input hash recomputes");

        assert_eq!(before, after);
    }

    #[test]
    fn ignored_files_do_not_change_subdirectory_workspace_hash() {
        let root = rskit_testutil::test_workspace!("cache-ignored-subworkspace");
        let repo = root.path().join("repo");
        let workspace_root = repo.join("work");
        fs::create_dir_all(workspace_root.join("src")).expect("create workspace tree");
        fs::write(workspace_root.join("src/lib.rs"), "base\n").expect("write source");
        fs::write(repo.join(".gitignore"), "work/src/*.ignored\n").expect("write gitignore");
        init_git_repo(&repo);
        let workspace = workspace(&workspace_root);

        let before = compute_shared_inputs_hash(&workspace, &["src".to_string()])
            .expect("shared input hash computes");
        fs::write(workspace_root.join("src/generated.ignored"), "ignored\n")
            .expect("write ignored file");
        let after = compute_shared_inputs_hash(&workspace, &["src".to_string()])
            .expect("shared input hash recomputes");

        assert_eq!(before, after);
    }

    fn module_hash_with_staged_content(
        repo: &Path,
        workspace: &Workspace,
        modules: &[Module],
        staged_content: &str,
    ) -> String {
        git(repo, ["reset", "--hard", "HEAD", "--quiet"]);
        fs::write(repo.join("src/lib.rs"), staged_content).expect("write staged source");
        git(repo, ["add", "src/lib.rs"]);
        fs::write(repo.join("src/lib.rs"), "base\n").expect("restore worktree source");
        compute_source_hashes(workspace, modules)
            .expect("source hashes compute")
            .modules
            .get(&modules[0].name)
            .expect("module hash exists")
            .clone()
    }

    fn shared_hash_with_staged_content(
        repo: &Path,
        workspace: &Workspace,
        staged_content: &str,
    ) -> String {
        git(repo, ["reset", "--hard", "HEAD", "--quiet"]);
        fs::write(repo.join("shared.lock"), staged_content).expect("write staged shared input");
        git(repo, ["add", "shared.lock"]);
        fs::write(repo.join("shared.lock"), "base\n").expect("restore worktree shared input");
        compute_shared_inputs_hash(workspace, &["shared.lock".to_string()])
            .expect("shared input hash computes")
    }

    fn workspace(root: &Path) -> Workspace {
        Workspace {
            schema: 1,
            name: "fixture".to_string(),
            root: root.to_path_buf(),
            base_ref: None,
            profiles: Vec::new(),
        }
    }

    fn module(name: &str, root: &str) -> Module {
        Module {
            name: ModuleId::new(name).expect("module id"),
            package: None,
            root: PathBuf::from(root),
            dependencies: Vec::new(),
            source_patterns: Vec::new(),
        }
    }

    fn init_git_repo(repo: &Path) {
        git(repo, ["init", "--initial-branch=main", "--quiet"]);
        git(repo, ["config", "user.name", "Toven Test"]);
        git(repo, ["config", "user.email", "toven@example.invalid"]);
        git(repo, ["add", "-A"]);
        git(repo, ["commit", "--quiet", "-m", "baseline"]);
    }

    fn git<const N: usize>(repo: &Path, args: [&str; N]) {
        let output = Command::new("git")
            .current_dir(repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git failed with {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
