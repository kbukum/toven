//! [`DriverWizard`] — drive an out-of-process driver's onboarding wizard.

use std::path::Path;

use rskit_errors::AppResult;

use crate::EcosystemFragment;
use crate::wizard::AnswerProvider;

/// Drives the `detect → questionnaire → render` wizard of an out-of-process
/// `toven-<eco>` driver, answering its questionnaire through the injected
/// [`AnswerProvider`].
///
/// Injected so `toven init` stays testable without spawning a real subprocess;
/// the engine's production adapter runs the federated two-round-trip `__init`
/// exchange, keeping the driver alive across the prompt so a single detection is
/// answered and rendered without re-probing.
pub trait DriverWizard {
    /// Ask the driver at `program` to detect its ecosystems under `project_root`,
    /// prompt each detected questionnaire via `answers`, and return the rendered
    /// `[ecosystems.<id>]` fragments.
    ///
    /// # Errors
    /// Returns a typed error if the driver cannot be reached, the exchange fails
    /// or times out, answering fails, or the driver reports a render failure. A
    /// *located* driver that misbehaves is a hard error, never a silent skip.
    fn run(
        &self,
        program: &Path,
        project_root: &Path,
        answers: &dyn AnswerProvider,
    ) -> AppResult<Vec<EcosystemFragment>>;
}
