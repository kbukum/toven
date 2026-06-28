//! Explicit member-repo provisioning.
//!
//! Clones absent member repos and guards present ones — the cross-repo analogue
//! of [`provision`](super::provision)'s explicit driver install.
//!
//! Provisioning is **never implicit during a run** (the same supply-chain purity
//! rule as driver install): a normal PLAN treats an absent declared member as a
//! hard error, and only a separate explicit member-repo provisioning surface
//! clones or checks out member repos. Cloning reuses `rskit-git` directly rather
//! than introducing a separate git path; the clean-tree guardrail per present
//! member repo reuses
//! [`Repository::is_dirty`](rskit_git::Repository).

use rskit_errors::{AppError, AppResult};
use toven_model::AbsPath;

/// Where one declared member repo is provisioned from and to.
///
/// The clone `url` is treated as untrusted input validated by `rskit-git` at the
/// clone boundary; `root` is the confined absolute member root resolved by
/// [`enumerate_members`](super::members::enumerate_members).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MemberRemote {
    name: String,
    url: String,
    root: AbsPath,
    checkout: Option<String>,
}

impl MemberRemote {
    /// Describe a member repo to provision at `root` from `url`.
    #[must_use]
    pub fn new(name: impl Into<String>, url: impl Into<String>, root: AbsPath) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            root,
            checkout: None,
        }
    }

    /// Check out `reference` after cloning (e.g. a pinned member ref).
    #[must_use]
    pub fn with_checkout(mut self, reference: impl Into<String>) -> Self {
        self.checkout = Some(reference.into());
        self
    }

    /// The member name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// How one member repo was handled during a sync.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[non_exhaustive]
pub enum MemberSyncStatus {
    /// The repo was absent and was cloned into place.
    Cloned,
    /// The repo was already present (and passed the clean-tree guardrail).
    AlreadyPresent,
}

/// The per-member outcome of [`sync_members`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MemberSyncReport {
    entries: Vec<(String, MemberSyncStatus)>,
}

impl MemberSyncReport {
    /// The per-member name → status outcomes, in input order.
    #[must_use]
    pub fn entries(&self) -> &[(String, MemberSyncStatus)] {
        &self.entries
    }
}

/// Provision every member in `remotes`: clone the absent ones, guard the present
/// ones.
///
/// An absent member root is cloned from its `url` (and checked out at its pinned
/// reference when one is given). A present member root must already be a git repo
/// and — unless `allow_dirty` — must be clean: a dirty present member is a hard
/// error so sync never silently disturbs local work. This function performs no
/// implicit provisioning on a normal run; only the explicit sync verbs call it.
///
/// # Errors
/// Returns a typed error when a present member root is not a git repo, a present
/// member repo is dirty (without `allow_dirty`), a clone fails, or a post-clone
/// checkout fails.
pub fn sync_members(remotes: &[MemberRemote], allow_dirty: bool) -> AppResult<MemberSyncReport> {
    let mut entries = Vec::with_capacity(remotes.len());
    for remote in remotes {
        let status = sync_one(remote, allow_dirty)?;
        entries.push((remote.name.clone(), status));
    }
    Ok(MemberSyncReport { entries })
}

/// Provision a single member repo.
fn sync_one(remote: &MemberRemote, allow_dirty: bool) -> AppResult<MemberSyncStatus> {
    if remote.root.as_path().is_dir() {
        guard_present(remote, allow_dirty)?;
        return Ok(MemberSyncStatus::AlreadyPresent);
    }
    if remote.root.as_path().exists() {
        return Err(AppError::invalid_input(
            "members.root",
            format!(
                "member '{}' root '{}' exists but is not a directory",
                remote.name, remote.root
            ),
        ));
    }
    clone_member(remote)?;
    Ok(MemberSyncStatus::Cloned)
}

/// Verify a present member root is a clean git repo before leaving it untouched.
fn guard_present(remote: &MemberRemote, allow_dirty: bool) -> AppResult<()> {
    // Anchor the check at the member directory itself with `open` rather than
    // `discover`: discovery walks up to parent directories, so a non-repo member
    // dir nested inside a git umbrella would be misread as the umbrella repo and
    // wrongly reported `AlreadyPresent` instead of being cloned.
    let repo = rskit_git::open(remote.root.as_path()).map_err(|error| {
        AppError::invalid_input(
            "members.root",
            format!(
                "member '{}' at '{}' exists but is not a git repository",
                remote.name, remote.root
            ),
        )
        .with_cause(error)
    })?;
    if !allow_dirty && rskit_git::Repository::is_dirty(&repo)? {
        return Err(AppError::invalid_input(
            "members.worktree",
            format!(
                "member '{}' has a dirty working tree; commit, stash, or pass --allow-dirty",
                remote.name
            ),
        ));
    }
    Ok(())
}

/// Clone an absent member repo and check out its pinned reference, if any.
fn clone_member(remote: &MemberRemote) -> AppResult<()> {
    let repo = rskit_git::clone(&remote.url, remote.root.as_path()).map_err(|error| {
        AppError::new(
            rskit_errors::ErrorCode::Internal,
            format!(
                "failed to clone member '{}' into '{}'",
                remote.name, remote.root
            ),
        )
        .with_cause(error)
    })?;
    if let Some(reference) = &remote.checkout {
        rskit_git::CheckoutManager::checkout(&repo, reference, None).map_err(|error| {
            AppError::invalid_input(
                "members.checkout",
                format!(
                    "cloned member '{}' but could not check out '{reference}'",
                    remote.name
                ),
            )
            .with_cause(error)
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use toven_model::AbsPath;
    use toven_testkit::git::GitScenario;

    use super::{MemberRemote, MemberSyncStatus, sync_members};

    #[test]
    fn clones_an_absent_member_repo() {
        let ws = toven_testkit::workspace::workspace("sync-clone");
        // A local "remote" repo with one commit, cloned by filesystem path.
        let remote_dir = ws.child("remote").unwrap();
        let remote = GitScenario::init(&remote_dir).unwrap();
        remote.commit_file("README.md", "hi", "init").unwrap();

        let target = ws.child("repos/core").unwrap();
        let request = MemberRemote::new(
            "core",
            remote_dir.to_string_lossy().into_owned(),
            AbsPath::new(target.clone()).unwrap(),
        );

        let report = sync_members(&[request], false).unwrap();

        assert_eq!(
            report.entries(),
            &[("core".to_string(), MemberSyncStatus::Cloned)]
        );
        assert!(target.join("README.md").is_file());
        assert!(rskit_git::discover(&target).is_ok());
    }

    #[test]
    fn present_clean_member_is_left_untouched() {
        let ws = toven_testkit::workspace::workspace("sync-present");
        let root = ws.child("repos/core").unwrap();
        let repo = GitScenario::init(&root).unwrap();
        repo.commit_file("README.md", "hi", "init").unwrap();

        let request = MemberRemote::new("core", "unused", AbsPath::new(root).unwrap());

        let report = sync_members(&[request], false).unwrap();

        assert_eq!(
            report.entries(),
            &[("core".to_string(), MemberSyncStatus::AlreadyPresent)]
        );
    }

    #[test]
    fn dirty_present_member_is_rejected_without_allow_dirty() {
        let ws = toven_testkit::workspace::workspace("sync-dirty");
        let root = ws.child("repos/core").unwrap();
        let repo = GitScenario::init(&root).unwrap();
        repo.commit_file("README.md", "hi", "init").unwrap();
        repo.write_file("README.md", "dirty change").unwrap();

        let request = MemberRemote::new("core", "unused", AbsPath::new(root).unwrap());

        let error = sync_members(std::slice::from_ref(&request), false).unwrap_err();
        assert!(error.to_string().contains("dirty"));
        // --allow-dirty bypasses the guardrail.
        assert!(sync_members(&[request], true).is_ok());
    }

    #[test]
    fn present_non_repo_directory_is_rejected() {
        let ws = toven_testkit::workspace::workspace("sync-nonrepo");
        ws.write_file("repos/core/.keep", b"").unwrap();
        let root = ws.child("repos/core").unwrap();

        let request = MemberRemote::new("core", "unused", AbsPath::new(root).unwrap());

        let error = sync_members(&[request], false).unwrap_err();
        assert!(error.to_string().contains("not a git repository"));
    }

    #[test]
    fn present_file_at_member_root_is_rejected() {
        let ws = toven_testkit::workspace::workspace("sync-file-root");
        let root = ws.write_file("repos/core", b"not a directory").unwrap();

        let request = MemberRemote::new("core", "unused", AbsPath::new(root).unwrap());

        let error = sync_members(&[request], false).unwrap_err();
        assert!(error.to_string().contains("not a directory"));
    }
}
