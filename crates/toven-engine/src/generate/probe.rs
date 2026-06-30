//! The bootstrap probe set: every in-proc provider plus any `toven-<eco>` driver
//! on `PATH`, each self-detecting whether it applies under the project root.
//!
//! This is the **one** place generate uses `PATH` discovery without config —
//! precisely because config does not exist yet at generate time (driver
//! resolution normally keys off `toven.toml`). In-proc providers win over a PATH
//! driver for the same ecosystem, so a linked adapter is never shadowed by an
//! out-of-process one.

use std::collections::BTreeSet;
use std::path::Path;

use rskit_errors::{AppError, AppResult};
use toven_model::EcosystemId;
use toven_ports::{DriverLocator, DriverScaffolder, EcosystemFragment, Provider};

use crate::config::CanonicalRegistry;
use crate::federation;

/// The production [`DriverScaffolder`]: spawns `program __scaffold` and runs the
/// federated config-less scaffold exchange.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessDriverScaffolder;

impl ProcessDriverScaffolder {
    /// Construct the production driver-scaffolder.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl DriverScaffolder for ProcessDriverScaffolder {
    fn scaffold(&self, program: &Path, project_root: &Path) -> AppResult<Vec<EcosystemFragment>> {
        federation::probe_driver(program, project_root)
    }
}

/// Probe the bootstrap set and collect every detected `[ecosystems.<id>]` fragment.
///
/// In-proc providers are probed first; each canonical ecosystem with no in-proc
/// provider is then probed via its `toven-<id>` driver if one is on `PATH`. A
/// fragment from an in-proc provider always wins over a driver's for the same
/// ecosystem id.
///
/// A `toven-<id>` driver may only scaffold its **own** ecosystem: a located
/// driver returning a fragment for any other ecosystem id is misbehavior across
/// the `PATH`-discovery trust boundary and is rejected as a hard error rather
/// than silently merged.
///
/// # Errors
/// Propagates a provider's own scaffold failure, or a *located* driver that
/// fails the scaffold exchange or returns a fragment for an ecosystem other than
/// the one it was probed for (an absent driver is simply not probed).
pub(super) fn probe(
    providers: &[&dyn Provider],
    scaffolder: &dyn DriverScaffolder,
    locator: &dyn DriverLocator,
    project_root: &Path,
) -> AppResult<Vec<EcosystemFragment>> {
    let mut detected: BTreeSet<EcosystemId> = BTreeSet::new();
    let mut fragments: Vec<EcosystemFragment> = Vec::new();

    let loaded: BTreeSet<EcosystemId> = providers
        .iter()
        .map(|provider| provider.ecosystem_id().clone())
        .collect();

    for provider in providers {
        if let Some(fragment) = provider.scaffold(project_root)?
            && detected.insert(fragment.ecosystem.clone())
        {
            fragments.push(fragment);
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
        for fragment in scaffolder.scaffold(&program, project_root)? {
            if fragment.ecosystem != id {
                return Err(AppError::invalid_input(
                    "generate.scaffold",
                    format!(
                        "driver '{}' scaffolded ecosystem '{}', but a 'toven-{id}' driver may \
                         only scaffold its own '{id}' ecosystem",
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
