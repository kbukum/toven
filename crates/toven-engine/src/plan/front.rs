//! Shared PLAN front half: Configure → Discover → Graph.
//!
//! Task planning and release planning both need the same validated federation
//! before they diverge into their domain-specific tails. This module keeps that
//! seam engine-internal while avoiding duplicate discovery/graph policy.

use rskit_errors::AppResult;
use toven_model::{AbsPath, Event, Graph, Phase};
use toven_ports::{Provider, Reporter};

use crate::config::Document;
use crate::federation::resolve::{self, DriverLocator};

use super::configure::ConfiguredSet;
use super::discover::Federation;
use super::{configure, discover, graph};

/// Validated shared state produced before a PLAN tail diverges.
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct PlanContext {
    pub(crate) adapters: ConfiguredSet,
    pub(crate) federation: Federation,
    pub(crate) graph: Graph,
}

/// Run the reusable Configure → Discover → Graph front half.
///
/// # Errors
/// Propagates configuration, discovery, graph construction, or semantic
/// validation failures.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn prepare(
    project_root: &AbsPath,
    document: &Document,
    providers: &[&dyn Provider],
    locator: &dyn DriverLocator,
    reporter: &mut dyn Reporter,
) -> AppResult<PlanContext> {
    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Configure,
    })?;
    let mut adapters = configure::configure(document, providers)?;
    // Four-way dispatch (step 11): canonical-but-unloaded ecosystems with a
    // resolved driver are connected out-of-proc behind the same trait; absent
    // drivers warn + skip. A resolved driver that fails is a hard PLAN error.
    let remote = resolve::resolve_adapters(document, providers, locator)?;
    let remote_warnings = remote.warnings;
    for (id, adapter) in remote.adapters {
        adapters.insert(id, adapter);
    }
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Configure,
    })?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Discover,
    })?;
    let mut federation = discover::discover(project_root, &adapters, document)?;
    federation.warnings.extend(remote_warnings);
    // Surface every non-fatal diagnostic (adapter discovery warnings plus the
    // absent-driver skips) so a warn-and-skip is observable, not silently dropped.
    for warning in &federation.warnings {
        reporter.emit(&Event::Warning {
            message: warning.clone(),
        })?;
    }
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Discover,
    })?;

    reporter.emit(&Event::PhaseStarted {
        phase: Phase::Graph,
    })?;
    let graph = graph::build(&federation)?;
    graph::validate_semantics(&graph, document)?;
    reporter.emit(&Event::PhaseFinished {
        phase: Phase::Graph,
    })?;

    Ok(PlanContext {
        adapters,
        federation,
        graph,
    })
}

/// Build the validated discovered module dependency graph without scheduling a task.
///
/// This runs the shared Configure → Discover → Graph front half and stops before
/// affected selection, toolchain probing, scheduling, or cache decisions. Use it
/// for introspection surfaces whose output is about the discovered topology rather
/// than a task-specific PLAN cut. The injected `locator` resolves out-of-process
/// drivers for canonical-but-unloaded ecosystems, so federation resolution stays
/// deterministic in tests.
///
/// # Errors
/// Propagates configuration, discovery, graph construction, or semantic
/// validation failures.
pub fn dependency_graph(
    project_root: &AbsPath,
    document: &Document,
    providers: &[&dyn Provider],
    locator: &dyn DriverLocator,
    reporter: &mut dyn Reporter,
) -> AppResult<Graph> {
    prepare(project_root, document, providers, locator, reporter).map(|context| context.graph)
}
