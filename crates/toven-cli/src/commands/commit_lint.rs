//! `toven commit-lint [message]`: lint a commit subject (or PR title) against
//! the Conventional Commits grammar.
//!
//! A thin CLI over the pure [`validate_conventional_subject`] check that
//! `toven-version` already uses to classify commits for the release changelog —
//! so what lints clean here is exactly what the changelog can classify without
//! falling through to `Other`. The subject comes from the `[message]` argument
//! or, when omitted, the first line of the message on stdin (the `commit-msg`
//! hook and PR-title shapes). The typed verdict renders on stdout — a human line
//! by default, a stable JSON line under `--output jsonl` — and the process exits
//! non-zero when the subject does not conform, so it drops into a gate unchanged.

use std::io::{self, Read};

use rskit_cli::ExitCode;
use rskit_errors::AppResult;
use toven_version::{CommitLintViolation, ConventionalHeader, validate_conventional_subject};

use crate::flags::OutputKind;

/// `toven commit-lint [message] [--output human|jsonl]`.
///
/// Lints the `[message]` argument, or the first line of stdin when it is
/// omitted. Renders the verdict on stdout and returns
/// [`ExitCode::Success`](ExitCode) for a conforming subject or
/// [`ExitCode::Failure`](ExitCode) otherwise — the failure is a lint verdict,
/// not a usage error, so it mirrors `doctor`'s report-then-fail contract.
///
/// # Errors
/// Propagates an I/O error when the message must be read from stdin and the read
/// fails.
pub(crate) fn commit_lint(
    message: Option<&str>,
    output: Option<OutputKind>,
) -> AppResult<ExitCode> {
    let raw = match message {
        Some(message) => message.to_string(),
        None => read_stdin()?,
    };
    let subject = raw.lines().next().unwrap_or("").trim();
    let verdict = validate_conventional_subject(subject);

    match output.unwrap_or(OutputKind::Human) {
        OutputKind::Jsonl => render_jsonl(subject, &verdict)?,
        OutputKind::Human => render_human(subject, &verdict),
    }

    Ok(match verdict {
        Ok(_) => ExitCode::Success,
        Err(_) => ExitCode::Failure,
    })
}

/// Read the entire commit message from stdin.
///
/// # Errors
/// Propagates a read failure as an internal error preserving the cause.
fn read_stdin() -> AppResult<String> {
    let mut buffer = String::new();
    io::stdin()
        .read_to_string(&mut buffer)
        .map_err(rskit_errors::AppError::internal)?;
    Ok(buffer)
}

/// Render the human verdict line for `subject` on stdout.
fn render_human(subject: &str, verdict: &Result<ConventionalHeader, CommitLintViolation>) {
    match verdict {
        Ok(header) => {
            let breaking = if header.breaking { " (breaking)" } else { "" };
            println!(
                "valid Conventional Commit [{}]{breaking}: {subject}",
                header.kind
            );
        }
        Err(violation) => {
            println!("invalid Conventional Commit: {subject}");
            println!("  {violation}");
        }
    }
}

/// Render the stable one-line JSON verdict for `subject` on stdout.
///
/// # Errors
/// Propagates a serialization failure (never expected for these plain fields).
fn render_jsonl(
    subject: &str,
    verdict: &Result<ConventionalHeader, CommitLintViolation>,
) -> AppResult<()> {
    let record = match verdict {
        Ok(header) => CommitLintRecord {
            valid: true,
            subject,
            r#type: Some(&header.kind),
            scope: header.scope.as_deref(),
            breaking: header.breaking,
            description: Some(&header.description),
            violation: None,
        },
        Err(violation) => CommitLintRecord {
            valid: false,
            subject,
            r#type: None,
            scope: None,
            breaking: false,
            description: None,
            violation: Some(violation.to_string()),
        },
    };
    let line = serde_json::to_string(&record).map_err(rskit_errors::AppError::internal)?;
    println!("{line}");
    Ok(())
}

/// The stable JSON record for one `commit-lint` verdict.
#[derive(serde::Serialize)]
struct CommitLintRecord<'a> {
    valid: bool,
    subject: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    r#type: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<&'a str>,
    breaking: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    violation: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::commit_lint;
    use crate::flags::OutputKind;
    use rskit_cli::ExitCode;

    #[test]
    fn valid_subject_argument_exits_success() {
        let code = commit_lint(
            Some("feat(cli): add commit-lint verb"),
            Some(OutputKind::Human),
        )
        .expect("linting an argument does not perform I/O");
        assert_eq!(code, ExitCode::Success);
    }

    #[test]
    fn invalid_subject_argument_exits_failure() {
        let code = commit_lint(Some("wip: half done"), Some(OutputKind::Jsonl))
            .expect("linting an argument does not perform I/O");
        assert_eq!(code, ExitCode::Failure);
    }

    #[test]
    fn only_the_first_line_is_linted() {
        // A conforming subject with a non-conforming body still passes: the body
        // is not part of the Conventional Commit header.
        let code = commit_lint(
            Some("fix: correct bug\n\nthis body line is not a header"),
            Some(OutputKind::Human),
        )
        .expect("linting an argument does not perform I/O");
        assert_eq!(code, ExitCode::Success);
    }
}
