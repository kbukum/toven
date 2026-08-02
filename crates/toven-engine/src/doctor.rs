//! Doctor: audit the tools the resolved task graph needs.
//!
//! `doctor` answers "does this repository have the tools its tasks will run?"
//! It reuses the [Configure](crate::plan) phase to bake every declared
//! ecosystem's [`ConfiguredAdapter`](toven_ports::ConfiguredAdapter), then — per
//! resolved task — collects the [`ToolchainProbe`]s that task needs (the same
//! per-task probe seam the planner uses via
//! [`toolchain_probes_for`](toven_ports::ConfiguredAdapter::toolchain_probes_for))
//! and evaluates each through the injected [`ToolchainProber`], **without
//! aborting** on the first absent tool. The result is a typed [`ToolAudit`] the
//! CLI projects through the reporter sinks; this module never prints.
//!
//! Tool identity is never invented here: each probe's program and label come
//! from the adapter (a tooling adapter's ecosystem-wide probe, or the command
//! adapter's per-task `argv[0]`). The engine only aggregates and classifies, so
//! core stays language- and tool-agnostic.

use std::collections::BTreeSet;

use rskit_errors::{AppResult, ErrorCode};
use toven_model::{AbsPath, ToolStatus};
use toven_ports::{Provider, TaskIntent, ToolchainProbe, ToolchainProber};

use crate::config::Document;
use crate::plan::configure::configure;

/// One audited tool: the probe that identifies it plus its presence status.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolProbeOutcome {
    /// The probe's human-readable label (e.g. `"cargo"`).
    pub label: String,
    /// The program that was probed (`argv[0]`).
    pub program: String,
    /// Whether the tool is present (and its version) or missing.
    pub status: ToolStatus,
}

impl ToolProbeOutcome {
    /// Construct an outcome for a probed tool.
    #[must_use]
    pub fn new(label: impl Into<String>, program: impl Into<String>, status: ToolStatus) -> Self {
        Self {
            label: label.into(),
            program: program.into(),
            status,
        }
    }
}

/// The classified tool set of a project's resolved task graph.
///
/// Ordered and de-duplicated: a tool many tasks share is one entry, and the
/// order is deterministic (by ecosystem, then task, then probe order) so a
/// projection is byte-stable.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolAudit {
    /// Every unique tool the resolved task graph needs, classified.
    pub tools: Vec<ToolProbeOutcome>,
}

impl ToolAudit {
    /// Construct an audit from its classified tool set.
    #[must_use]
    pub const fn new(tools: Vec<ToolProbeOutcome>) -> Self {
        Self { tools }
    }

    /// How many audited tools are missing.
    #[must_use]
    pub fn missing_count(&self) -> usize {
        self.tools
            .iter()
            .filter(|tool| tool.status.is_missing())
            .count()
    }

    /// Whether every audited tool is present.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.missing_count() == 0
    }
}

/// Audit the tools the resolved task graph of `document` needs.
///
/// Configures every declared ecosystem, collects the de-duplicated probe set
/// across every resolved task, and classifies each probe by running it through
/// `prober` in `project_root`. A probe that finds its program yields
/// [`ToolStatus::Present`] (with a version when one is parseable); a spawn
/// `NotFound` yields [`ToolStatus::Missing`].
///
/// # Errors
/// Propagates configuration failures and any probe failure that is *not* a
/// missing tool — a hang, a permission error, or an output overrun is a hard
/// error, never silently reported as "missing".
pub fn audit(
    project_root: &AbsPath,
    document: &Document,
    providers: &[&dyn Provider],
    prober: &dyn ToolchainProber,
) -> AppResult<ToolAudit> {
    let needed = collect_probes(document, providers)?;
    let mut tools = Vec::with_capacity(needed.len());
    for probe in needed {
        let status = classify(prober, &probe, project_root)?;
        tools.push(ToolProbeOutcome::new(probe.label, probe.program, status));
    }
    Ok(ToolAudit::new(tools))
}

/// Collect the de-duplicated probe set across every resolved task of every
/// configured ecosystem, preserving a deterministic order.
///
/// De-duplication is by `(program, args)` so a tool many tasks share is probed
/// once; the first label seen for that key wins.
fn collect_probes(
    document: &Document,
    providers: &[&dyn Provider],
) -> AppResult<Vec<ToolchainProbe>> {
    let configured = configure(document, providers)?;
    let mut probes = Vec::new();
    let mut seen: BTreeSet<(String, Vec<String>)> = BTreeSet::new();
    for (ecosystem, adapter) in &configured {
        for (key, entry) in &adapter.common().tasks {
            let task = entry.materialize(ecosystem.as_str(), key)?;
            let intent = TaskIntent::resolve(&task.name).with_kind(task.kind);
            for probe in adapter.toolchain_probes_for(&intent) {
                if seen.insert((probe.program.clone(), probe.args.clone())) {
                    probes.push(probe);
                }
            }
        }
    }
    Ok(probes)
}

/// Run one probe and classify its outcome, mapping only a spawn `NotFound` to
/// [`ToolStatus::Missing`] and propagating every other failure.
fn classify(
    prober: &dyn ToolchainProber,
    probe: &ToolchainProbe,
    project_root: &AbsPath,
) -> AppResult<ToolStatus> {
    match prober.probe(probe, project_root.as_path()) {
        Ok(version) => Ok(ToolStatus::Present {
            version: (!version.is_empty()).then_some(version),
        }),
        Err(error) if error.code() == ErrorCode::NotFound => Ok(ToolStatus::Missing),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use rskit_errors::{AppError, AppResult, ErrorCode};
    use toven_model::{AbsPath, EcosystemId, ToolStatus};
    use toven_ports::{FanOut, Provider, Task, ToolchainProbe, ToolchainProber};
    use toven_testkit::{FakeConfiguredAdapter, FakeProvider, ScriptedToolchainProber};

    use super::audit;
    use crate::config::{Document, ProjectConfig, TovenConfig};

    fn eid(id: &str) -> EcosystemId {
        EcosystemId::new(id).unwrap()
    }

    fn root() -> AbsPath {
        AbsPath::new("/repo").expect("absolute")
    }

    fn document_with(ecosystems: &[&str]) -> Document {
        let mut sections = BTreeMap::new();
        for id in ecosystems {
            sections.insert(eid(id), rskit_config::RawValue::Null);
        }
        Document {
            project: ProjectConfig {
                name: "t".to_string(),
                root: ".".to_string(),
                base_ref: None,
            },
            toven: TovenConfig::default(),
            groups: BTreeMap::new(),
            overlays: Vec::new(),
            ecosystems: sections,
            modules: BTreeMap::new(),
            members: Vec::new(),
        }
    }

    /// A provider whose single toolchain probe targets `program` and which
    /// declares one task (so the probe is enumerated by the audit).
    fn provider_probing(id: &str, program: &str) -> FakeProvider {
        let adapter = FakeConfiguredAdapter::new(eid(id))
            .with_probe(ToolchainProbe::new(
                program,
                program,
                vec!["--version".into()],
            ))
            .with_tasks(vec![Task::new(
                "build",
                vec![program.into(), "build".into()],
                FanOut::Batchable,
            )]);
        FakeProvider::new(eid(id)).with_adapter(adapter)
    }

    #[test]
    fn classifies_present_and_missing_tools_across_the_graph() {
        let rust = provider_probing("rust", "cargo");
        let command = provider_probing("command", "mdbook");
        let providers: Vec<&dyn Provider> = vec![&rust, &command];
        let prober = ScriptedToolchainProber::new()
            .with_version("cargo 1.94.0")
            .with_absent("mdbook");

        let audit = audit(
            &root(),
            &document_with(&["rust", "command"]),
            &providers,
            &prober,
        )
        .expect("audit succeeds");

        assert_eq!(audit.tools.len(), 2);
        let cargo = audit
            .tools
            .iter()
            .find(|tool| tool.program == "cargo")
            .expect("cargo audited");
        assert_eq!(
            cargo.status,
            ToolStatus::Present {
                version: Some("cargo 1.94.0".to_string()),
            }
        );
        let mdbook = audit
            .tools
            .iter()
            .find(|tool| tool.program == "mdbook")
            .expect("mdbook audited");
        assert_eq!(mdbook.status, ToolStatus::Missing);
        assert_eq!(audit.missing_count(), 1);
        assert!(!audit.is_healthy());
    }

    #[test]
    fn a_present_tool_reporting_no_version_is_present_without_a_version() {
        let rust = provider_probing("rust", "cargo");
        let providers: Vec<&dyn Provider> = vec![&rust];
        // An empty version line means "present, version unknown".
        let prober = ScriptedToolchainProber::new().with_version("");

        let audit =
            audit(&root(), &document_with(&["rust"]), &providers, &prober).expect("audit succeeds");

        assert_eq!(audit.tools.len(), 1);
        assert_eq!(audit.tools[0].status, ToolStatus::Present { version: None });
        assert!(audit.is_healthy());
    }

    #[test]
    fn a_shared_tool_is_audited_once() {
        // Two tasks that both shell `cargo` share one probe (the fake adapter's
        // single ecosystem-wide probe), so the audit reports one tool and probes
        // it once.
        let adapter = FakeConfiguredAdapter::new(eid("rust"))
            .with_probe(ToolchainProbe::new(
                "cargo",
                "cargo",
                vec!["--version".into()],
            ))
            .with_tasks(vec![
                Task::new(
                    "build",
                    vec!["cargo".into(), "build".into()],
                    FanOut::Batchable,
                ),
                Task::new(
                    "test",
                    vec!["cargo".into(), "test".into()],
                    FanOut::Batchable,
                ),
            ]);
        let provider = FakeProvider::new(eid("rust")).with_adapter(adapter);
        let providers: Vec<&dyn Provider> = vec![&provider];
        let prober = ScriptedToolchainProber::new();

        let audit =
            audit(&root(), &document_with(&["rust"]), &providers, &prober).expect("audit succeeds");

        assert_eq!(audit.tools.len(), 1);
        assert_eq!(prober.calls(), 1);
    }

    /// A prober that fails every probe with a non-`NotFound` error, modeling a
    /// hang or a permission problem the audit must surface, not swallow.
    #[derive(Default)]
    struct FailingProber;

    impl ToolchainProber for FailingProber {
        fn probe(&self, probe: &ToolchainProbe, _root: &Path) -> AppResult<String> {
            Err(AppError::new(
                ErrorCode::Timeout,
                format!("probe '{}' hung", probe.program),
            ))
        }
    }

    #[test]
    fn a_non_missing_probe_failure_is_propagated_not_reported_as_missing() {
        let rust = provider_probing("rust", "cargo");
        let providers: Vec<&dyn Provider> = vec![&rust];

        let error = audit(
            &root(),
            &document_with(&["rust"]),
            &providers,
            &FailingProber,
        )
        .expect_err("a probe hang must fail the audit");

        assert_eq!(error.code(), ErrorCode::Timeout);
    }
}
