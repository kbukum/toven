//! The bootstrap probe set: every in-proc provider plus any `toven-<eco>` driver
//! on `PATH`, each running its onboarding wizard under the project root.
//!
//! This is the **one** place init uses `PATH` discovery without config —
//! precisely because config does not exist yet at init time (driver resolution
//! normally keys off `toven.toml`). In-proc providers win over a PATH driver for
//! the same ecosystem, so a linked adapter is never shadowed by an
//! out-of-process one.

use std::collections::BTreeSet;
use std::path::Path;

use rskit_errors::{AppError, AppResult};
use toven_model::EcosystemId;
use toven_ports::{AnswerProvider, DriverLocator, DriverWizard, EcosystemFragment, Provider};

use crate::config::CanonicalRegistry;
use crate::federation;

/// The production [`DriverWizard`]: spawns `program __init` and runs the
/// federated config-less two-round-trip wizard exchange, answering each
/// questionnaire through the injected [`AnswerProvider`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessDriverWizard;

impl ProcessDriverWizard {
    /// Construct the production driver-wizard.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl DriverWizard for ProcessDriverWizard {
    fn run(
        &self,
        program: &Path,
        project_root: &Path,
        answers: &dyn AnswerProvider,
    ) -> AppResult<Vec<EcosystemFragment>> {
        federation::run_driver_wizard(program, project_root, answers)
    }
}

/// Probe the bootstrap set and collect every rendered `[ecosystems.<id>]`
/// fragment, answering each provider's questionnaire through `answers`.
///
/// In-proc providers are probed first (detect → questionnaire → answer →
/// render); each canonical ecosystem with no in-proc provider is then driven via
/// its `toven-<id>` driver if one is on `PATH`. A fragment from an in-proc
/// provider always wins over a driver's for the same ecosystem id.
///
/// A `toven-<id>` driver may only render its **own** ecosystem: a located driver
/// returning a fragment for any other ecosystem id is misbehavior across the
/// `PATH`-discovery trust boundary and is rejected as a hard error rather than
/// silently merged.
///
/// # Errors
/// Propagates a provider's own detect/questionnaire/render failure, an answering
/// failure, or a *located* driver that fails the wizard exchange or returns a
/// fragment for an ecosystem other than the one it was probed for (an absent
/// driver is simply not probed).
pub(super) fn probe(
    providers: &[&dyn Provider],
    wizard: &dyn DriverWizard,
    locator: &dyn DriverLocator,
    answers: &dyn AnswerProvider,
    project_root: &Path,
) -> AppResult<Vec<EcosystemFragment>> {
    let mut detected: BTreeSet<EcosystemId> = BTreeSet::new();
    let mut fragments: Vec<EcosystemFragment> = Vec::new();

    let loaded: BTreeSet<EcosystemId> = providers
        .iter()
        .map(|provider| provider.ecosystem_id().clone())
        .collect();

    for provider in providers {
        if let Some(detection) = provider.detect(project_root)? {
            let questionnaire = provider.questionnaire(&detection)?;
            let resolved = answers.answers_for(&questionnaire)?;
            let fragment = provider.render(&detection, &resolved)?;
            if detected.insert(fragment.ecosystem.clone()) {
                fragments.push(fragment);
            }
        }
    }

    let canonical = CanonicalRegistry::model();
    for id in canonical.ids() {
        if loaded.contains(&id) {
            continue;
        }
        let Some(program) = locator.locate(&federation::driver_binary_name(&id))? else {
            continue;
        };
        for fragment in wizard.run(&program, project_root, answers)? {
            if fragment.ecosystem != id {
                return Err(AppError::invalid_input(
                    "init.wizard",
                    format!(
                        "driver '{}' rendered ecosystem '{}', but a 'toven-{id}' driver may \
                         only render its own '{id}' ecosystem",
                        program.display(),
                        fragment.ecosystem,
                    ),
                ));
            }
            if detected.insert(fragment.ecosystem.clone()) {
                fragments.push(fragment);
            }
        }
    }

    Ok(fragments)
}
