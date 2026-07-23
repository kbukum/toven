//! The `init` dispatch: build the prompt seam, drive the engine init flow, and
//! route the outcome to stdout/stderr.
//!
//! Channel discipline (cli-taxonomy): the rendered config is the **product** —
//! with `--print` it goes to **stdout** (writing nothing); otherwise it is
//! written atomically to `toven.toml` and only a summary is emitted. Every
//! diagnostic (an additive re-run skipping an existing section, an ineffective
//! `--force`, a write confirmation) goes to **stderr**, so `toven init --print >toven.toml`
//! captures only the document.

use std::path::{Path, PathBuf};

use rskit_cli::{ExitCode, PromptMode};
use rskit_errors::AppResult;
use toven_engine::init::InitOutcome;
use toven_model::EcosystemId;
use toven_ports::Provider;

use super::prompt::PromptAnswers;
use crate::flags::Cli;

/// Run `toven init [--force <id>] [--root <path>] [--non-interactive]
/// [--print]`.
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

    let report = Report::from_init_outcome(&outcome, defaults_reason(cli));
    for line in &report.diagnostics {
        eprintln!("{line}");
    }
    if let Some(document) = &report.document {
        // The document is the product: stdout only, no trailing extras.
        print!("{document}");
    }

    Ok(ExitCode::Success)
}

/// Why the wizard resolved every question to its default instead of prompting,
/// or `None` when it prompted interactively.
#[derive(Clone, Copy)]
enum DefaultsReason {
    /// The user passed `--non-interactive`/`--yes`.
    Explicit,
    /// Stdio is not an interactive terminal (a pipe/CI, or a redirected prompt
    /// sink), as resolved by [`PromptMode::from_env`].
    NonInteractiveEnv,
}

impl DefaultsReason {
    /// The lead-in clause naming the cause, for the "used defaults" note.
    const fn lead_in(self) -> &'static str {
        match self {
            Self::Explicit => "ran with `--non-interactive`",
            Self::NonInteractiveEnv => "no terminal detected",
        }
    }
}

/// Classify why the wizard skipped prompting. The explicit
/// `--non-interactive`/`--yes` flag takes precedence over an inferred
/// non-interactive stdio environment; an interactive run yields `None`.
fn defaults_reason(cli: &Cli) -> Option<DefaultsReason> {
    if cli.non_interactive {
        Some(DefaultsReason::Explicit)
    } else if PromptMode::from_env().is_interactive() {
        None
    } else {
        Some(DefaultsReason::NonInteractiveEnv)
    }
}

/// The channel-routed output of an init run.
///
/// Encodes the cli-taxonomy contract as plain data so it is unit-testable: the
/// rendered document goes to **stdout** (only when it was not written to disk),
/// while every diagnostic — re-run/`--force` warnings and the write summary —
/// goes to **stderr**.
struct Report {
    /// The document destined for stdout, present only when it was not
    /// persisted.
    document: Option<String>,
    /// Diagnostics destined for stderr (warnings, then any write summary).
    diagnostics: Vec<String>,
}

impl Report {
    /// Route a finished [`InitOutcome`] into its stdout/stderr channels.
    ///
    /// When `defaults` is set and the run wrote a config, a one-line note is
    /// prepended to the write summary telling the user prompts were skipped
    /// (and why) and how to preview (`--print`) or regenerate (`--force <id>`)
    /// — so a flagged or pipe/CI run never silently accepts every default
    /// without a signal.
    fn from_init_outcome(outcome: &InitOutcome, defaults: Option<DefaultsReason>) -> Self {
        // The detected-ecosystems line leads the stderr diagnostics in both write and
        // `--print` modes, so a `--print` run (stdout reserved for the TOML) still tells
        // the user what was detected instead of appearing to do nothing.
        let detected = detected_line(&outcome.detected);
        if outcome.written {
            let touched = touched_sections(&outcome.added, &outcome.regenerated);
            let summary = write_summary(&outcome.path, outcome.created, &touched);
            let mut diagnostics = write_diagnostics(&outcome.warnings, defaults, summary);
            prepend(&mut diagnostics, detected);
            Self {
                document: None,
                diagnostics,
            }
        } else {
            let mut diagnostics = outcome.warnings.clone();
            prepend(&mut diagnostics, detected);
            Self {
                document: Some(outcome.rendered.clone()),
                diagnostics,
            }
        }
    }
}

/// The `detected: <ecosystems>` stderr line, or `None` when nothing was detected
/// (the engine already emits a dedicated no-ecosystem hint in that case).
fn detected_line(detected: &[EcosystemId]) -> Option<String> {
    if detected.is_empty() {
        return None;
    }
    let names: Vec<&str> = detected.iter().map(EcosystemId::as_str).collect();
    Some(format!("detected: {}", names.join(", ")))
}

/// Insert `line` (when present) at the front of the diagnostics.
fn prepend(diagnostics: &mut Vec<String>, line: Option<String>) {
    if let Some(line) = line {
        diagnostics.insert(0, line);
    }
}

/// Assemble the stderr diagnostics for a write run: existing `warnings`, then
/// the "used defaults" note (only when `defaults` is set), then the write
/// `summary` last as the closing confirmation.
fn write_diagnostics(
    warnings: &[String],
    defaults: Option<DefaultsReason>,
    summary: String,
) -> Vec<String> {
    let mut diagnostics = warnings.to_vec();
    if let Some(reason) = defaults {
        diagnostics.push(defaults_note(reason));
    }
    diagnostics.push(summary);
    diagnostics
}

/// The stderr note shown when a write run resolved every prompt to its default,
/// led by the clause naming why prompting was skipped.
fn defaults_note(reason: DefaultsReason) -> String {
    format!(
        "{}; used the default answer for every prompt. \
         Preview with `toven init --print`; regenerate a section with \
         `toven init --force <ecosystem>`.",
        reason.lead_in()
    )
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

    use super::{DefaultsReason, touched_sections, write_diagnostics, write_summary};

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

    #[test]
    fn write_diagnostics_appends_the_defaults_note_before_the_summary() {
        let diagnostics = write_diagnostics(
            &[],
            Some(DefaultsReason::NonInteractiveEnv),
            "wrote /repo/toven.toml".to_string(),
        );
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics[0].starts_with("no terminal detected"));
        assert!(diagnostics[0].contains("used the default answer"));
        assert!(diagnostics[0].contains("--print"));
        assert!(diagnostics[0].contains("--force"));
        // The write confirmation always closes the diagnostics.
        assert_eq!(diagnostics[1], "wrote /repo/toven.toml");
    }

    #[test]
    fn write_diagnostics_note_names_the_explicit_flag_cause() {
        let diagnostics = write_diagnostics(
            &[],
            Some(DefaultsReason::Explicit),
            "wrote /repo/toven.toml".to_string(),
        );
        assert!(
            diagnostics[0].starts_with("ran with `--non-interactive`"),
            "explicit flag must not claim a missing terminal, got: {}",
            diagnostics[0]
        );
        assert!(diagnostics[0].contains("used the default answer"));
    }

    #[test]
    fn write_diagnostics_omits_the_note_when_prompts_were_answered() {
        let warnings = vec!["skipped [ecosystems.rust]".to_string()];
        let diagnostics = write_diagnostics(&warnings, None, "wrote /repo/toven.toml".to_string());
        assert_eq!(
            diagnostics,
            vec![
                "skipped [ecosystems.rust]".to_string(),
                "wrote /repo/toven.toml".to_string()
            ]
        );
    }
}
