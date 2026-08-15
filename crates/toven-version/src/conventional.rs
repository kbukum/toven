//! Conventional Commit classification for changelog generation.
//!
//! Parses a commit's `type(scope)!: description` header and `BREAKING CHANGE:`
//! footer into a typed [`ChangeGroup`] plus a cleaned description and author
//! attribution. Pure and forge-agnostic: it reads only the commit text a
//! [`CommitSummary`] carries, so the same classification drives a GitHub,
//! GitLab, or bare-remote changelog identically.

use std::fmt;

use toven_ports::CommitSummary;

/// The Conventional Commit types Toven's `commit-lint` accepts.
///
/// The Angular-convention set the project's PR-title check enforces, so a
/// commit or PR title that lints clean here is exactly one the release changelog
/// can classify without falling through to `Other`.
pub const CONVENTIONAL_COMMIT_TYPES: [&str; 11] = [
    "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore", "revert",
];

/// A commit subject that parses as a valid Conventional Commit header.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ConventionalHeader {
    /// The commit type; always one of [`CONVENTIONAL_COMMIT_TYPES`].
    pub kind: String,
    /// The optional scope (`feat(scope): …`).
    pub scope: Option<String>,
    /// Whether the header marks a breaking change (a `!` before the colon).
    pub breaking: bool,
    /// The description after `: `, trimmed.
    pub description: String,
}

/// Why a commit subject is not a valid Conventional Commit header.
#[derive(Debug, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum CommitLintViolation {
    /// The subject is empty or whitespace-only.
    Empty,
    /// No `type: ` prefix — the subject has no Conventional Commit structure.
    MissingType,
    /// A `word:` prefix whose type is not an accepted lowercase type.
    UnknownType(String),
    /// A recognized type not followed by `": "` (a colon then a space).
    MissingSpaceAfterColon,
    /// A valid `type: ` prefix with an empty description.
    EmptyDescription,
}

impl fmt::Display for CommitLintViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "commit subject is empty"),
            Self::MissingType => write!(
                f,
                "missing Conventional Commit type: expected `<type>[(scope)][!]: <description>`"
            ),
            Self::UnknownType(kind) => write!(
                f,
                "unknown Conventional Commit type `{kind}`: expected one of {}",
                CONVENTIONAL_COMMIT_TYPES.join(", ")
            ),
            Self::MissingSpaceAfterColon => {
                write!(
                    f,
                    "missing space after the colon: expected `<type>: <description>`"
                )
            }
            Self::EmptyDescription => write!(f, "empty description after the type"),
        }
    }
}

impl std::error::Error for CommitLintViolation {}

/// Validate a commit **subject** (its first line) against the Conventional
/// Commits `type(scope)!: description` grammar Toven enforces.
///
/// Pure and forge-agnostic: the strict counterpart of the changelog
/// classification path, sharing its header tokenizer but returning a typed
/// pass/fail so a CLI can lint a commit message or a PR title. On success it
/// yields the parsed [`ConventionalHeader`]; otherwise the first
/// [`CommitLintViolation`] that makes `subject` non-conforming.
///
/// # Errors
/// Returns the [`CommitLintViolation`] describing why `subject` is not a valid
/// Conventional Commit header.
pub fn validate_conventional_subject(
    subject: &str,
) -> Result<ConventionalHeader, CommitLintViolation> {
    let trimmed = subject.trim();
    if trimmed.is_empty() {
        return Err(CommitLintViolation::Empty);
    }
    let parts = split_header(trimmed);
    let Some(kind) = parts.kind else {
        return Err(CommitLintViolation::MissingType);
    };
    if !CONVENTIONAL_COMMIT_TYPES.contains(&kind.as_str()) {
        return Err(CommitLintViolation::UnknownType(kind));
    }
    if parts.rest.is_empty() {
        return Err(CommitLintViolation::EmptyDescription);
    }
    if !parts.rest.starts_with(' ') {
        return Err(CommitLintViolation::MissingSpaceAfterColon);
    }
    let description = parts.rest.trim().to_string();
    if description.is_empty() {
        return Err(CommitLintViolation::EmptyDescription);
    }
    Ok(ConventionalHeader {
        kind,
        scope: parts.scope,
        breaking: parts.breaking,
        description,
    })
}

/// A changelog section, ordered by prominence.
///
/// Every commit lands in exactly one group: a breaking change is surfaced under
/// [`Breaking`](ChangeGroup::Breaking) rather than duplicated under its type,
/// and any commit that is not a recognized `feat`/`fix`/`perf`/`refactor` type
/// falls through to [`Other`](ChangeGroup::Other) so nothing is silently
/// dropped.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) enum ChangeGroup {
    /// A breaking change (`type!:` or a `BREAKING CHANGE:` footer).
    Breaking,
    /// A new feature (`feat`).
    Added,
    /// A bug fix (`fix`).
    Fixed,
    /// A behavior-preserving change (`perf`, `refactor`, `revert`).
    Changed,
    /// Any other commit (`docs`, `chore`, `test`, non-conforming, …).
    Other,
}

impl ChangeGroup {
    /// The Keep a Changelog-style heading for this group.
    pub(crate) const fn heading(self) -> &'static str {
        match self {
            Self::Breaking => "Breaking changes",
            Self::Added => "Added",
            Self::Fixed => "Fixed",
            Self::Changed => "Changed",
            Self::Other => "Other",
        }
    }

    /// Every group in render order.
    pub(crate) const fn ordered() -> [Self; 5] {
        [
            Self::Breaking,
            Self::Added,
            Self::Fixed,
            Self::Changed,
            Self::Other,
        ]
    }

    /// The group whose [`heading`](Self::heading) equals `heading`, if any.
    ///
    /// The inverse of [`heading`](Self::heading), used to re-derive a section's
    /// group from a parsed note body so merged sections can be restored to
    /// canonical order.
    pub(crate) fn from_heading(heading: &str) -> Option<Self> {
        Self::ordered()
            .into_iter()
            .find(|group| group.heading() == heading)
    }
}

/// A commit classified for the changelog.
#[derive(Debug, Clone, Eq, PartialEq)]
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct ClassifiedCommit {
    /// The section this commit belongs to.
    pub(crate) group: ChangeGroup,
    /// Optional Conventional Commit scope (`feat(scope):`).
    pub(crate) scope: Option<String>,
    /// The description with the `type(scope)!:` prefix stripped.
    pub(crate) description: String,
    /// GitHub-style `@handle` when derivable from the author identity, else the
    /// plain author display name.
    pub(crate) author: String,
    /// Abbreviated commit id.
    pub(crate) id: String,
}

/// Classify a commit into its changelog group with a cleaned description and
/// author attribution.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn classify(commit: &CommitSummary) -> ClassifiedCommit {
    let parts = split_header(&commit.subject);
    let breaking = parts.breaking || body_flags_breaking(&commit.body);
    let kind = parts.kind.as_deref().map(str::to_ascii_lowercase);
    let group = if breaking {
        ChangeGroup::Breaking
    } else {
        group_for_kind(kind.as_deref())
    };
    ClassifiedCommit {
        group,
        scope: parts.scope,
        description: parts.rest.trim().to_string(),
        author: attribution(commit),
        id: commit.id.clone(),
    }
}

/// Map a Conventional Commit type to its non-breaking group; an unrecognized or
/// absent type is [`Other`](ChangeGroup::Other).
fn group_for_kind(kind: Option<&str>) -> ChangeGroup {
    match kind {
        Some("feat") => ChangeGroup::Added,
        Some("fix") => ChangeGroup::Fixed,
        Some("perf" | "refactor" | "revert") => ChangeGroup::Changed,
        _ => ChangeGroup::Other,
    }
}

/// The raw, case-preserving parts of a `type(scope)!: description` header.
struct HeaderParts {
    /// The commit type exactly as written, or `None` when the subject has no
    /// `type:` prefix (no colon, or a prefix that cannot be a type).
    kind: Option<String>,
    /// The optional scope (`feat(scope): …`).
    scope: Option<String>,
    /// Whether a `!` precedes the colon.
    breaking: bool,
    /// The remainder after the first `:`, **untrimmed** (so a validator can
    /// require a leading space), or the whole subject when `kind` is `None`.
    rest: String,
}

/// Split a `type(scope)!: description` header into its raw parts, preserving the
/// type's case. A header with no `type:` prefix (no colon, or a prefix that is
/// not a bare word) yields `kind = None` with the whole subject as `rest`, so it
/// still renders as an `Other` changelog entry and lints as [`MissingType`].
///
/// [`MissingType`]: CommitLintViolation::MissingType
fn split_header(subject: &str) -> HeaderParts {
    let verbatim = || HeaderParts {
        kind: None,
        scope: None,
        breaking: false,
        rest: subject.to_string(),
    };
    let Some((prefix, rest)) = subject.split_once(':') else {
        return verbatim();
    };
    let prefix = prefix.trim();
    let (type_and_scope, breaking) = prefix
        .strip_suffix('!')
        .map_or((prefix, false), |stripped| (stripped.trim_end(), true));

    let (kind, scope) = match type_and_scope.split_once('(') {
        Some((kind, rest)) => {
            let scope = rest.strip_suffix(')').unwrap_or(rest).trim();
            let scope = (!scope.is_empty()).then(|| scope.to_string());
            (kind.trim(), scope)
        }
        None => (type_and_scope, None),
    };

    // A "type" with whitespace is not a Conventional Commit type (e.g. a prose
    // subject that merely contains a colon); treat the whole subject as verbatim.
    if kind.is_empty() || kind.contains(char::is_whitespace) {
        return verbatim();
    }

    HeaderParts {
        kind: Some(kind.to_string()),
        scope,
        breaking,
        rest: rest.to_string(),
    }
}

/// Whether a commit body carries a `BREAKING CHANGE:` / `BREAKING-CHANGE:`
/// footer (the Conventional Commits breaking marker).
fn body_flags_breaking(body: &str) -> bool {
    body.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("BREAKING CHANGE:") || line.starts_with("BREAKING-CHANGE:")
    })
}

/// Render the author attribution: a GitHub `@handle` derived from a
/// `users.noreply.github.com` email (honoring `Co-authored-by:` trailers), else
/// the plain git author display name.
fn attribution(commit: &CommitSummary) -> String {
    if let Some(handle) = github_handle(&commit.author_email) {
        return format!("@{handle}");
    }
    if let Some(handle) = coauthor_handle(&commit.body) {
        return format!("@{handle}");
    }
    if !commit.author_name.is_empty() {
        commit.author_name.clone()
    } else if !commit.author_email.is_empty() {
        commit.author_email.clone()
    } else {
        "unknown".to_string()
    }
}

/// Extract a GitHub login from a `users.noreply.github.com` email.
///
/// GitHub's noreply addresses are `login@users.noreply.github.com` or
/// `ID+login@users.noreply.github.com`, so the login is deterministically
/// recoverable from git data alone — no forge API call.
fn github_handle(email: &str) -> Option<String> {
    let local = email
        .trim()
        .to_ascii_lowercase()
        .strip_suffix("@users.noreply.github.com")?
        .to_string();
    let login = match local.split_once('+') {
        Some((_id, login)) => login,
        None => local.as_str(),
    };
    let login = login.trim();
    (!login.is_empty() && !login.contains(char::is_whitespace)).then(|| login.to_string())
}

/// Extract the first `Co-authored-by: Name <email>` trailer's GitHub handle.
fn coauthor_handle(body: &str) -> Option<String> {
    for line in body.lines() {
        let line = line.trim();
        let Some(rest) = strip_prefix_ci(line, "co-authored-by:") else {
            continue;
        };
        let email = rest
            .rsplit_once('<')
            .and_then(|(_, tail)| tail.strip_suffix('>'))?;
        if let Some(handle) = github_handle(email) {
            return Some(handle);
        }
    }
    None
}

/// Case-insensitive prefix strip, returning the trimmed remainder.
fn strip_prefix_ci<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let head = line.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| line[prefix.len()..].trim())
}

#[cfg(test)]
mod tests {
    use toven_ports::CommitSummary;

    use super::{
        ChangeGroup, CommitLintViolation, ConventionalHeader, classify,
        validate_conventional_subject,
    };

    fn commit(subject: &str) -> CommitSummary {
        CommitSummary::new("abc123def456", subject)
    }

    #[test]
    fn feat_is_added_with_scope_stripped() {
        let classified = classify(&commit("feat(cache): add fs backend"));
        assert_eq!(classified.group, ChangeGroup::Added);
        assert_eq!(classified.scope.as_deref(), Some("cache"));
        assert_eq!(classified.description, "add fs backend");
    }

    #[test]
    fn fix_is_fixed() {
        assert_eq!(
            classify(&commit("fix: correct bug")).group,
            ChangeGroup::Fixed
        );
    }

    #[test]
    fn perf_and_refactor_are_changed() {
        assert_eq!(
            classify(&commit("perf: speed up")).group,
            ChangeGroup::Changed
        );
        assert_eq!(
            classify(&commit("refactor(x): tidy")).group,
            ChangeGroup::Changed
        );
    }

    #[test]
    fn bang_marks_breaking() {
        let classified = classify(&commit("feat(api)!: drop legacy field"));
        assert_eq!(classified.group, ChangeGroup::Breaking);
        assert_eq!(classified.description, "drop legacy field");
    }

    #[test]
    fn breaking_change_footer_marks_breaking() {
        let commit = CommitSummary::from_message(
            "abc123def456",
            "feat: add thing\n\nBREAKING CHANGE: removed old thing",
        );
        assert_eq!(classify(&commit).group, ChangeGroup::Breaking);
    }

    #[test]
    fn chore_and_docs_fall_through_to_other() {
        assert_eq!(
            classify(&commit("chore: bump dep")).group,
            ChangeGroup::Other
        );
        assert_eq!(classify(&commit("docs: update")).group, ChangeGroup::Other);
    }

    #[test]
    fn non_conventional_subject_is_other_verbatim() {
        let classified = classify(&commit("just a plain message: with colon"));
        assert_eq!(classified.group, ChangeGroup::Other);
        assert_eq!(classified.description, "just a plain message: with colon");
    }

    #[test]
    fn github_noreply_email_yields_handle() {
        let commit =
            commit("feat: x").with_author("Ada", "12345+ada-lovelace@users.noreply.github.com");
        assert_eq!(classify(&commit).author, "@ada-lovelace");
    }

    #[test]
    fn plain_noreply_email_yields_handle() {
        let commit = commit("feat: x").with_author("Octocat", "octocat@users.noreply.github.com");
        assert_eq!(classify(&commit).author, "@octocat");
    }

    #[test]
    fn non_github_email_falls_back_to_name() {
        let commit = commit("feat: x").with_author("Ada Lovelace", "ada@example.com");
        assert_eq!(classify(&commit).author, "Ada Lovelace");
    }

    #[test]
    fn coauthor_trailer_yields_handle() {
        let commit = CommitSummary::from_message(
            "abc123def456",
            "feat: pair work\n\nCo-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>",
        )
        .with_author("Ada", "ada@example.com");
        // The primary author is non-GitHub, so the co-author handle attributes it.
        assert_eq!(classify(&commit).author, "@copilot");
    }

    #[test]
    fn valid_header_with_scope_and_bang_parses() {
        let header = validate_conventional_subject("feat(cli)!: add commit-lint verb")
            .expect("a well-formed header validates");
        assert_eq!(
            header,
            ConventionalHeader {
                kind: "feat".to_string(),
                scope: Some("cli".to_string()),
                breaking: true,
                description: "add commit-lint verb".to_string(),
            }
        );
    }

    #[test]
    fn every_accepted_type_validates() {
        for kind in super::CONVENTIONAL_COMMIT_TYPES {
            let subject = format!("{kind}: a change");
            assert!(
                validate_conventional_subject(&subject).is_ok(),
                "`{subject}` should lint clean"
            );
        }
    }

    #[test]
    fn blank_subject_is_empty() {
        assert_eq!(
            validate_conventional_subject("   "),
            Err(CommitLintViolation::Empty)
        );
    }

    #[test]
    fn subject_without_colon_is_missing_type() {
        assert_eq!(
            validate_conventional_subject("add a thing"),
            Err(CommitLintViolation::MissingType)
        );
    }

    #[test]
    fn prose_subject_with_colon_is_missing_type() {
        assert_eq!(
            validate_conventional_subject("just a plain message: with colon"),
            Err(CommitLintViolation::MissingType)
        );
    }

    #[test]
    fn unrecognized_type_is_unknown_type() {
        assert_eq!(
            validate_conventional_subject("wip: half done"),
            Err(CommitLintViolation::UnknownType("wip".to_string()))
        );
    }

    #[test]
    fn uppercase_type_is_rejected() {
        // The accepted set is lowercase, matching the retired PR-title regex, so a
        // capitalized type is an unknown type rather than a silent pass.
        assert_eq!(
            validate_conventional_subject("Feat: add a thing"),
            Err(CommitLintViolation::UnknownType("Feat".to_string()))
        );
    }

    #[test]
    fn missing_space_after_colon_is_rejected() {
        assert_eq!(
            validate_conventional_subject("feat:add a thing"),
            Err(CommitLintViolation::MissingSpaceAfterColon)
        );
    }

    #[test]
    fn empty_description_is_rejected() {
        // A bare `type:` (whitespace-only description collapses under trimming).
        assert_eq!(
            validate_conventional_subject("fix:"),
            Err(CommitLintViolation::EmptyDescription)
        );
        assert_eq!(
            validate_conventional_subject("fix:   "),
            Err(CommitLintViolation::EmptyDescription)
        );
    }

    #[test]
    fn violation_messages_are_actionable() {
        assert!(
            CommitLintViolation::UnknownType("wip".to_string())
                .to_string()
                .contains("feat")
        );
    }
}
