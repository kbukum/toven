use std::collections::BTreeSet;
use std::path::Path;

use rskit_codec::YamlCodec;
use rskit_errors::{AppError, AppResult};
use rskit_fs::sync_io::file;

use super::model::{Effect, MatcherKind, Scenario, Step, StreamExpectation};

/// The scenario definition filename inside a scenario directory.
pub const SCENARIO_FILENAME: &str = "scenario.yaml";

impl Scenario {
    /// Load and validate the `scenario.yaml` inside `dir`.
    ///
    /// # Errors
    ///
    /// Returns a `NotFound`-class error when the file is missing, and an
    /// actionable `InvalidInput`-class error (cause preserved, path named) for
    /// malformed YAML, unknown keys, duplicate or unsafe step ids, a
    /// traversing or non-TOML `config`, or misplaced frame fields.
    pub fn load(dir: &Path) -> AppResult<Self> {
        let path = dir.join(SCENARIO_FILENAME);
        if !file::exists(&path)? {
            return Err(AppError::not_found(
                "scenario definition",
                Some(&path.display().to_string()),
            ));
        }
        let contents = file::read_string(&path)?;
        let with_path = |err: AppError| {
            err.context(format!("invalid scenario definition at {}", path.display()))
        };
        let scenario: Self = rskit_codec::decode(&YamlCodec, &contents).map_err(with_path)?;
        validate(&scenario).map_err(with_path)?;
        Ok(scenario)
    }
}

fn validate(scenario: &Scenario) -> AppResult<()> {
    if scenario.steps.is_empty() {
        return Err(AppError::invalid_input("steps", "scenario has no steps"));
    }
    let mut seen_ids = BTreeSet::new();
    for step in &scenario.steps {
        validate_step_id(&step.id)?;
        if !seen_ids.insert(step.id.as_str()) {
            return Err(AppError::invalid_input(
                "steps",
                format!("duplicate step id '{}'", step.id),
            ));
        }
        validate_step(step)?;
    }
    Ok(())
}

/// Step ids become golden-file basenames, so they must be bare, portable
/// filename fragments.
fn validate_step_id(id: &str) -> AppResult<()> {
    let filename_safe = !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if filename_safe {
        Ok(())
    } else {
        Err(AppError::invalid_input(
            "step id",
            format!("'{id}' is not filename-safe (allowed: ASCII alphanumerics, '-', '_', '.')"),
        ))
    }
}

fn validate_step(step: &Step) -> AppResult<()> {
    if step.argv.is_empty() {
        return Err(AppError::invalid_input(
            "argv",
            format!("step '{}' has an empty argv", step.id),
        ));
    }
    if let Some(config) = &step.config {
        validate_config(config, &step.id)?;
    }
    for effect in &step.effects {
        if let Effect::FileMatches { golden, .. } = effect {
            validate_effect_golden(golden, &step.id)?;
        }
    }
    validate_stream(step.stdout.as_ref(), &step.id, "stdout")?;
    validate_stream(step.stderr.as_ref(), &step.id, "stderr")?;
    Ok(())
}

/// An effect golden names a file *inside* the scenario directory — a bare
/// filename, never a path. This is a write target in bless mode, so a
/// traversing name must be rejected before anything runs.
fn validate_effect_golden(golden: &str, step_id: &str) -> AppResult<()> {
    let bare = !golden.is_empty()
        && golden != "."
        && golden != ".."
        && !golden.contains('/')
        && !golden.contains('\\');
    if bare {
        Ok(())
    } else {
        Err(AppError::invalid_input(
            "golden",
            format!(
                "step '{step_id}': effect golden '{golden}' must be a bare filename inside the scenario directory"
            ),
        ))
    }
}

/// A config variant names a file *inside* the materialized repo, so it must be
/// a bare filename — and Toven's own loader accepts TOML only.
fn validate_config(config: &str, step_id: &str) -> AppResult<()> {
    let bare = !config.is_empty()
        && config != "."
        && config != ".."
        && !config.contains('/')
        && !config.contains('\\');
    if !bare {
        return Err(AppError::invalid_input(
            "config",
            format!("step '{step_id}': '{config}' must be a bare filename inside the repo"),
        ));
    }
    // Strict lowercase, matching the published JSON schema exactly.
    if Path::new(config)
        .extension()
        .is_none_or(|ext| ext != "toml")
    {
        return Err(AppError::invalid_input(
            "config",
            format!("step '{step_id}': '{config}' must be a `.toml` file (Toven reads TOML only)"),
        ));
    }
    Ok(())
}

fn validate_stream(
    expectation: Option<&StreamExpectation>,
    step_id: &str,
    stream: &str,
) -> AppResult<()> {
    let Some(expectation) = expectation else {
        return Ok(());
    };
    if expectation.matcher != MatcherKind::LineSet
        && (expectation.frame_prefix.is_some() || expectation.frame_suffix.is_some())
    {
        return Err(AppError::invalid_input(
            "frame",
            format!(
                "step '{step_id}' {stream}: frame_prefix/frame_suffix apply only to `line-set`"
            ),
        ));
    }
    Ok(())
}
