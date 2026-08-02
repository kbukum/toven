//! Commit summary records returned by
//! [`VcsReader::commits_since`](super::VcsReader::commits_since).

use serde::{Deserialize, Serialize};

/// One commit reachable in a `since..HEAD` range, carrying exactly the fields a
/// changelog generator needs: a short id, the split subject/body of the
/// message, and the author identity for attribution.
///
/// The reader stays forge-agnostic — it exposes only git data (the author name
/// and email), never a resolved forge handle. Mapping an email to an `@handle`
/// is a rendering concern the engine owns, so the same record drives a GitHub,
/// GitLab, or bare-remote changelog identically.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct CommitSummary {
    /// Abbreviated commit object id (for an optional trailing reference).
    pub id: String,
    /// First line of the commit message (the Conventional Commit header).
    pub subject: String,
    /// Message body after the subject: the blank-line-separated paragraphs and
    /// trailers (`BREAKING CHANGE:`, `Co-authored-by:`) the classifier reads.
    pub body: String,
    /// Commit author display name.
    pub author_name: String,
    /// Commit author email (the source an `@handle` may be derived from).
    pub author_email: String,
}

impl CommitSummary {
    /// Construct a summary from an already-split subject.
    #[must_use]
    pub fn new(id: impl Into<String>, subject: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            subject: subject.into(),
            body: String::new(),
            author_name: String::new(),
            author_email: String::new(),
        }
    }

    /// Split a full commit `message` into its subject (first line) and body
    /// (the remainder, trimmed), preserving trailers for the classifier.
    #[must_use]
    pub fn from_message(id: impl Into<String>, message: &str) -> Self {
        let mut lines = message.splitn(2, '\n');
        let subject = lines.next().unwrap_or("").trim().to_string();
        let body = lines.next().unwrap_or("").trim().to_string();
        Self {
            id: id.into(),
            subject,
            body,
            author_name: String::new(),
            author_email: String::new(),
        }
    }

    /// Attach the author identity.
    #[must_use]
    pub fn with_author(mut self, name: impl Into<String>, email: impl Into<String>) -> Self {
        self.author_name = name.into();
        self.author_email = email.into();
        self
    }

    /// Attach an explicit body (for records not built from a full message).
    #[must_use]
    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::CommitSummary;

    #[test]
    fn from_message_splits_subject_and_body() {
        let commit = CommitSummary::from_message(
            "abc1234",
            "feat(cache): add fs backend\n\nDetails here.\n\nBREAKING CHANGE: renamed field",
        );
        assert_eq!(commit.id, "abc1234");
        assert_eq!(commit.subject, "feat(cache): add fs backend");
        assert_eq!(
            commit.body,
            "Details here.\n\nBREAKING CHANGE: renamed field"
        );
    }

    #[test]
    fn from_message_with_no_body_leaves_body_empty() {
        let commit = CommitSummary::from_message("deadbee", "fix: correct off-by-one");
        assert_eq!(commit.subject, "fix: correct off-by-one");
        assert!(commit.body.is_empty());
    }

    #[test]
    fn with_author_records_identity() {
        let commit = CommitSummary::new("abc", "feat: x").with_author("Ada", "ada@example.com");
        assert_eq!(commit.author_name, "Ada");
        assert_eq!(commit.author_email, "ada@example.com");
    }

    #[test]
    fn round_trips_through_toml() {
        let commit = CommitSummary::from_message("abc1234", "feat: add thing\n\nbody")
            .with_author("Ada", "ada@example.com");
        let serialized = toml::to_string(&commit).expect("serialize");
        let back: CommitSummary = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(commit, back);
    }
}
