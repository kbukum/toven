//! The `init` dispatch: build the prompt seam, drive the engine init flow, and
//! route the outcome to stdout/stderr.
//!
//! Channel discipline (cli-taxonomy): the rendered config is the **product** —
//! with `--print` it goes to **stdout** (writing nothing); otherwise it is
//! written atomically to `toven.toml` and only a summary is emitted. Every
//! diagnostic (an additive re-run skipping an existing section, an ineffective
//! `--force`, a write confirmation) goes to **stderr**, so `toven init --print >
//! toven.toml` captures only the document.

use std::path::{Path, PathBuf};

use rskit_cli::ExitCode;
use rskit_errors::AppResult;
use toven_engine::init::InitOutcome;
use toven_model::EcosystemId;
use toven_ports::Provider;

use super::prompt::PromptAnswers;
use crate::flags::Cli;

/// Run `toven init [--force <id>] [--root <path>] [--non-interactive] [--print]`.
///
/// Detects the ecosystems present under the root, runs each provider's wizard
/// (answered interactively, or from defaults with `--non-interactive`), then
/// renders a minimal first-run document or additively merges into an existing
/// one. Writes `toven.toml` unless `--print` previews it on stdout. Returns the
/// process exit code.
///
/// # Errors
/// Propagates a wizard-probe failure, an answering failure, an unreadable or
/// invalid existing config, or a failed atomic write from the engine flow.
pub(crate) fn execute(providers: &[&dyn Provider], cli: &Cli) -> AppResult<ExitCode> {
    let root = cli.root.clone().unwrap_or_else(|| PathBuf::from("."));
    let color = cli.color_choice().into();
    let answers = PromptAnswers::new(color, cli.non_interactive);
    let write = !cli.print;
    let outcome =
        toven_engine::init::init(&root, providers, &answers, cli.force.as_deref(), write)?;

    let report = Report::from_init_outcome(&outcome);
    for line in &report.diagnostics {
        eprintln!("{line}");
    }
    if let Some(document) = &report.document {
        // The document is the product: stdout only, no trailing extras.
        print!("{document}");
    }

    Ok(ExitCode::Success)
}

/// The channel-routed output of an init run.
///
/// Encodes the cli-taxonomy contract as plain data so it is unit-testable: the
/// rendered document goes to **stdout** (only when it was not written to disk),
/// while every diagnostic — re-run/`--force` warnings and the write summary —
/// goes to **stderr**.
struct Report {
    /// The document destined for stdout, present only when it was not persisted.
    document: Option<String>,
    /// Diagnostics destined for stderr (warnings, then any write summary).
    diagnostics: Vec<String>,
}

impl Report {
    /// Route a finished [`InitOutcome`] into its stdout/stderr channels.
    fn from_init_outcome(outcome: &InitOutcome) -> Self {
        let mut diagnostics = outcome.warnings.clone();
        let document = if outcome.written {
            let touched = touched_sections(&outcome.added, &outcome.regenerated);
            diagnostics.push(write_summary(&outcome.path, outcome.created, &touched));
            None
        } else {
            Some(outcome.rendered.clone())
        };
        Self {
            document,
            diagnostics,
        }
    }
}

/// The sections touched by a write run, sorted for a stable summary line.
fn touched_sections(added: &[EcosystemId], regenerated: &[EcosystemId]) -> Vec<String> {
    let mut touched: Vec<String> = added
        .iter()
        .chain(regenerated)
        .map(|id| id.as_str().to_string())
        .collect();
    touched.sort();
    touched
}

/// The stderr confirmation summarizing a write run.
fn write_summary(path: &Path, created: bool, touched: &[String]) -> String {
    let path = path.display();
    if touched.is_empty() {
        if created {
            format!("wrote {path}")
        } else {
            format!("{path} is up to date; no sections added")
        }
    } else {
        format!("wrote {path} ({})", touched.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use toven_model::EcosystemId;

    use super::{touched_sections, write_summary};

    fn eco(id: &str) -> EcosystemId {
        EcosystemId::new(id).expect("valid ecosystem id")
    }

    #[test]
    fn touched_sections_merges_and_sorts_added_and_regenerated() {
        let touched = touched_sections(&[eco("rust")], &[eco("go")]);
        assert_eq!(touched, vec!["go".to_string(), "rust".to_string()]);
    }

    #[test]
    fn write_summary_for_a_fresh_file_with_no_sections() {
        let summary = write_summary(Path::new("/repo/toven.toml"), true, &[]);
        assert_eq!(summary, "wrote /repo/toven.toml");
    }

    #[test]
    fn write_summary_for_an_additive_rerun_that_added_nothing_is_up_to_date() {
        let summary = write_summary(Path::new("/repo/toven.toml"), false, &[]);
        assert_eq!(summary, "/repo/toven.toml is up to date; no sections added");
    }

    #[test]
    fn write_summary_lists_touched_sections() {
        let touched = vec!["go".to_string(), "rust".to_string()];
        let summary = write_summary(&PathBuf::from("/repo/toven.toml"), true, &touched);
        assert_eq!(summary, "wrote /repo/toven.toml (go, rust)");
    }
}
