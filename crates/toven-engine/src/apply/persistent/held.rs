//! Reference-counted held set for persistent units.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::Plan;
use toven_ports::HeldProcess;

use super::teardown::TeardownRegistry;

/// Cloneable handle around a single owned [`HeldProcess`].
#[derive(Clone)]
pub(in crate::apply) struct SharedHeldProcess {
    unit_id: String,
    inner: Arc<Mutex<Option<Box<dyn HeldProcess>>>>,
}

impl SharedHeldProcess {
    /// Wrap a held process.
    #[must_use]
    pub(in crate::apply) fn new(process: Box<dyn HeldProcess>) -> Self {
        let unit_id = process.unit_id().to_string();
        Self {
            unit_id,
            inner: Arc::new(Mutex::new(Some(process))),
        }
    }

    /// Unit id for this process.
    pub(in crate::apply) fn unit_id(&self) -> &str {
        &self.unit_id
    }

    /// Shut down the process once; later calls are no-ops.
    pub(in crate::apply) fn shutdown(&self) -> AppResult<()> {
        let process = self
            .inner
            .lock()
            .map_err(|_| AppError::new(ErrorCode::Internal, "held process lock poisoned"))?
            .take();
        if let Some(process) = process {
            process.shutdown()?;
        }
        Ok(())
    }
}

struct HeldEntry {
    process: SharedHeldProcess,
    remaining: BTreeSet<String>,
    hold_until_end: bool,
}

/// Tracks ready persistent units until their dependents drain.
pub(in crate::apply) struct HeldSet {
    dependencies: BTreeMap<String, BTreeSet<String>>,
    entries: BTreeMap<String, HeldEntry>,
    order: Vec<String>,
    teardown: TeardownRegistry,
}

impl HeldSet {
    /// Build a held-set policy for `plan`.
    #[must_use]
    pub(in crate::apply) fn new(plan: &Plan) -> Self {
        Self {
            dependencies: dependent_sets(plan),
            entries: BTreeMap::new(),
            order: Vec::new(),
            teardown: TeardownRegistry::new(),
        }
    }

    /// Register a ready persistent process.
    pub(in crate::apply) fn hold(&mut self, process: SharedHeldProcess) {
        let unit_id = process.unit_id().to_string();
        let remaining = self.dependencies.get(&unit_id).cloned().unwrap_or_default();
        let hold_until_end = remaining.is_empty();
        self.teardown.register(process.clone());
        self.order.push(unit_id.clone());
        self.entries.insert(
            unit_id,
            HeldEntry {
                process,
                remaining,
                hold_until_end,
            },
        );
    }

    /// Mark `unit_id` drained and return held units whose dependent sets are empty.
    pub(in crate::apply) fn dependent_finished(&mut self, unit_id: &str) -> Vec<String> {
        let mut ready = Vec::new();
        for (held_id, entry) in &mut self.entries {
            if entry.hold_until_end {
                continue;
            }
            entry.remaining.remove(unit_id);
            if entry.remaining.is_empty() {
                ready.push(held_id.clone());
            }
        }
        ready
    }

    /// Tear down one held unit if present.
    pub(in crate::apply) fn teardown_one(&mut self, unit_id: &str) -> AppResult<bool> {
        let Some(entry) = self.entries.remove(unit_id) else {
            return Ok(false);
        };
        entry.process.shutdown()?;
        Ok(true)
    }

    /// Tear down everything still held through the registry's LIFO backstop.
    pub(in crate::apply) async fn teardown_all(&mut self) -> AppResult<Vec<String>> {
        let torn_down = self
            .order
            .iter()
            .rev()
            .filter(|unit_id| self.entries.contains_key(*unit_id))
            .cloned()
            .collect::<Vec<_>>();
        self.teardown.stop_all().await?;
        for unit_id in &torn_down {
            self.entries.remove(unit_id);
        }
        self.order.clear();
        Ok(torn_down)
    }
}

fn dependent_sets(plan: &Plan) -> BTreeMap<String, BTreeSet<String>> {
    let mut direct: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for unit in &plan.units {
        for dependency in &unit.depends_on {
            direct
                .entry(dependency.clone())
                .or_default()
                .push(unit.id.clone());
        }
    }
    plan.units
        .iter()
        .filter(|unit| unit.persistent)
        .map(|unit| (unit.id.clone(), transitive_dependents(&unit.id, &direct)))
        .collect()
}

fn transitive_dependents(
    unit_id: &str,
    direct: &BTreeMap<String, Vec<String>>,
) -> BTreeSet<String> {
    let mut dependents = BTreeSet::new();
    let mut pending = direct.get(unit_id).cloned().unwrap_or_default();
    while let Some(next) = pending.pop() {
        if dependents.insert(next.clone()) {
            pending.extend(direct.get(&next).cloned().unwrap_or_default());
        }
    }
    dependents
}
