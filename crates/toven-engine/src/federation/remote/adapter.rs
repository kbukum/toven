//! [`RemoteAdapter`] — a [`ConfiguredAdapter`] that proxies each port call to a
//! driven `toven-<eco> __serve` subprocess (or, in tests, an in-process server).
//!
//! Construction performs the handshake and **prefetches** every infallible query
//! the planner later asks for (`toolchain_probe`, the per-kind `run_strategy`
//! defaults, and the common config). Those trait methods cannot return errors,
//! so they are resolved once up front where a transport failure *can* be
//! surfaced; only the fallible [`discover`](ConfiguredAdapter::discover) stays a
//! live RPC. The runnable task table is not an RPC — it is the driver's resolved
//! config, carried in the [`Welcome`]'s common config.
//!
//! Release is capability-gated off for driven ecosystems:
//! [`release_target`](ConfiguredAdapter::release_target) returns `None`. The
//! umbrella keeps all orchestration; this proxy only answers port calls.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;

use rskit_config::RawValue;
use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_model::EcosystemId;
use toven_ports::{
    CommonEcosystemConfig, ConfiguredAdapter, DiscoverRequest, DiscoverResponse, ReleaseTarget,
    RunStrategy, TaskKind, ToolchainProbe,
};

use super::super::protocol::envelope::{ENVELOPE_SCHEMA_VERSION, Hello, Request, Response};
use super::super::protocol::handshake::{
    DriverFault, PROTOCOL_VERSION, negotiate, protocol_version,
};
use super::client::{DEFAULT_RPC_TIMEOUT, RpcClient};
use super::process::{self, ChildHandle};

/// Every built-in [`TaskKind`] whose default run strategy is prefetched.
const BUILTIN_KINDS: [TaskKind; 7] = [
    TaskKind::Build,
    TaskKind::Check,
    TaskKind::Format,
    TaskKind::Lint,
    TaskKind::Test,
    TaskKind::Doc,
    TaskKind::Run,
];

/// A driven ecosystem adapter that mirrors the port surface over the transport.
pub struct RemoteAdapter {
    ecosystem: EcosystemId,
    client: Mutex<RpcClient>,
    common: CommonEcosystemConfig,
    probe: ToolchainProbe,
    /// Prefetched run strategy per built-in kind name.
    run_strategies: std::collections::HashMap<String, RunStrategy>,
    /// Prefetched run strategy per **declared** custom task name (those present in
    /// the driver's resolved config task table). A driver may vary its default
    /// ordering by custom name, so each declared name is resolved up front.
    custom_run_strategies: std::collections::HashMap<String, RunStrategy>,
    /// Fallback run strategy for a [`TaskKind::Custom`] the driver did not declare
    /// in its config task table (resolved once via a sentinel probe).
    custom_run_strategy: RunStrategy,
}

impl RemoteAdapter {
    /// Spawn `program __serve` and connect a [`RemoteAdapter`] for `ecosystem`.
    ///
    /// `config` is the ecosystem's `[ecosystems.<id>]` raw subtree, handed to the
    /// driver's own `configure`.
    ///
    /// # Errors
    /// Returns a typed PLAN error if the driver cannot be spawned, the handshake
    /// is incompatible, or a prefetched port call fails. A resolved driver that
    /// fails is always a hard error — never a silent skip.
    pub fn spawn(program: &Path, ecosystem: EcosystemId, config: RawValue) -> AppResult<Self> {
        let driver = process::spawn(program, "__serve")
            .map_err(|fault| fault.into_app_error(ecosystem.as_str()))?;
        let child = ChildHandle::new(driver.child);
        let client = RpcClient::new(
            Box::new(driver.stdout),
            Box::new(driver.stdin),
            Some(child),
            DEFAULT_RPC_TIMEOUT,
            ecosystem.to_string(),
        );
        Self::connect(client, ecosystem, config)
    }

    /// Connect over an arbitrary framed reader/writer (the in-process test path).
    ///
    /// # Errors
    /// Returns a typed PLAN error on an incompatible handshake or a failed
    /// prefetch.
    pub fn connect_io<R, W>(
        reader: R,
        writer: W,
        ecosystem: EcosystemId,
        config: RawValue,
    ) -> AppResult<Self>
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let client = RpcClient::new(
            Box::new(reader),
            Box::new(writer),
            None,
            DEFAULT_RPC_TIMEOUT,
            ecosystem.to_string(),
        );
        Self::connect(client, ecosystem, config)
    }

    /// Handshake + prefetch the infallible port surface, building the adapter.
    fn connect(mut client: RpcClient, ecosystem: EcosystemId, config: RawValue) -> AppResult<Self> {
        let hello = Hello::new(PROTOCOL_VERSION.to_string(), ecosystem.clone(), config);
        let welcome = client
            .handshake(&hello)
            .map_err(|fault| fault.into_app_error(ecosystem.as_str()))?;

        if welcome.schema_version != ENVELOPE_SCHEMA_VERSION {
            return Err(AppError::new(
                ErrorCode::Conflict,
                format!(
                    "ecosystem '{}' driver speaks envelope schema v{}, but this Toven requires v{ENVELOPE_SCHEMA_VERSION}",
                    ecosystem.as_str(),
                    welcome.schema_version
                ),
            ));
        }

        negotiate(&protocol_version(), &welcome.protocol)
            .map_err(|fault| fault.into_app_error(ecosystem.as_str()))?;

        let missing = welcome.capabilities.missing_required();
        if !missing.is_empty() {
            return Err(AppError::new(
                ErrorCode::Conflict,
                format!(
                    "ecosystem '{}' driver is missing required PLAN capabilities: {}",
                    ecosystem.as_str(),
                    missing.join(", ")
                ),
            ));
        }

        let probe = match call(&mut client, &Request::ToolchainProbe)? {
            Response::ToolchainProbe(probe) => probe,
            other => return Err(unexpected(ecosystem.as_str(), "toolchain_probe", &other)),
        };

        let mut run_strategies = std::collections::HashMap::new();
        for kind in BUILTIN_KINDS {
            let strategy = fetch_run_strategy(&mut client, ecosystem.as_str(), &kind)?;
            run_strategies.insert(kind.name().to_string(), strategy);
        }

        // A driver's `run_strategy_default` may branch on the custom task name, so
        // prefetch the real default for every distinct custom task it declared in
        // its resolved config task table rather than collapsing them onto one
        // sentinel value.
        let mut custom_run_strategies = std::collections::HashMap::new();
        for name in distinct_custom_names(&welcome.common) {
            let kind = TaskKind::Custom(name.clone());
            let strategy = fetch_run_strategy(&mut client, ecosystem.as_str(), &kind)?;
            custom_run_strategies.insert(name, strategy);
        }
        // The fallback for an *undeclared* custom task must reflect how the driver
        // answers for a name it does not know, so probe with a sentinel guaranteed
        // not to collide with any declared custom task (otherwise a real task named
        // like the sentinel would leak its per-name strategy onto every unknown one).
        let sentinel = unique_probe_name(&custom_run_strategies);
        let custom_run_strategy =
            fetch_run_strategy(&mut client, ecosystem.as_str(), &TaskKind::Custom(sentinel))?;

        Ok(Self {
            ecosystem,
            client: Mutex::new(client),
            common: welcome.common,
            probe,
            run_strategies,
            custom_run_strategies,
            custom_run_strategy,
        })
    }
}

impl ConfiguredAdapter for RemoteAdapter {
    fn discover(&self, request: &DiscoverRequest) -> AppResult<DiscoverResponse> {
        let response = {
            let mut client = self
                .client
                .lock()
                .map_err(|_| AppError::new(ErrorCode::Internal, "driver RPC lock poisoned"))?;
            client
                .call(&Request::Discover(request.clone()))
                .map_err(|fault| fault.into_app_error(self.ecosystem.as_str()))?
        };
        match response {
            Response::Discover(response) => Ok(response),
            other => Err(unexpected(self.ecosystem.as_str(), "discover", &other)),
        }
    }

    fn toolchain_probe(&self) -> ToolchainProbe {
        self.probe.clone()
    }

    fn run_strategy_default(&self, kind: &TaskKind) -> RunStrategy {
        match kind {
            // A declared custom task uses its driver's prefetched default; an
            // undeclared one falls back to the generic custom default.
            TaskKind::Custom(name) => self
                .custom_run_strategies
                .get(name)
                .copied()
                .unwrap_or(self.custom_run_strategy),
            builtin => self
                .run_strategies
                .get(builtin.name())
                .copied()
                .unwrap_or(self.custom_run_strategy),
        }
    }

    fn release_target(&self) -> AppResult<Option<Box<dyn ReleaseTarget>>> {
        // Capability-gated: driven ecosystems are not publishable through the
        // umbrella transport.
        Ok(None)
    }

    fn common(&self) -> &CommonEcosystemConfig {
        &self.common
    }
}

/// Issue one prefetch call, mapping a transport fault to a typed PLAN error.
fn call(client: &mut RpcClient, request: &Request) -> AppResult<Response> {
    let ecosystem = client.ecosystem().to_string();
    client
        .call(request)
        .map_err(|fault| fault.into_app_error(&ecosystem))
}

/// The distinct custom task names declared in the driver's resolved config task
/// table, in table (sorted) order.
///
/// Used to prefetch each declared custom task's real run-strategy default rather
/// than collapsing every custom kind onto a single sentinel value. A custom task
/// is a config entry whose derived kind is [`TaskKind::Custom`].
fn distinct_custom_names(common: &CommonEcosystemConfig) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut names = Vec::new();
    for (key, entry) in &common.tasks {
        if let (TaskKind::Custom(name), _) = entry.kind_and_name(key)
            && seen.insert(name.clone())
        {
            names.push(name);
        }
    }
    names
}

/// A custom-task name guaranteed not to be one of the `declared` custom tasks.
///
/// The fallback run strategy is meant to capture how a driver answers for a
/// custom task it never declared, so the probe name must not collide with any
/// declared name. Starts from a fixed sentinel and extends it until unique.
fn unique_probe_name(declared: &std::collections::HashMap<String, RunStrategy>) -> String {
    let mut name = "__toven_unknown_custom_probe__".to_string();
    while declared.contains_key(&name) {
        name.push('_');
    }
    name
}

/// Prefetch the run strategy for one kind.
fn fetch_run_strategy(
    client: &mut RpcClient,
    ecosystem: &str,
    kind: &TaskKind,
) -> AppResult<RunStrategy> {
    match call(client, &Request::RunStrategy { kind: kind.clone() })? {
        Response::RunStrategy(strategy) => Ok(strategy),
        other => Err(unexpected(ecosystem, "run_strategy", &other)),
    }
}

/// Build the "driver answered the wrong response kind" protocol error.
fn unexpected(ecosystem: &str, method: &str, response: &Response) -> AppError {
    if let Response::Error(wire) = response {
        return DriverFault::Remote {
            code: wire.code.clone(),
            message: wire.message.clone(),
        }
        .into_app_error(ecosystem);
    }
    AppError::new(
        ErrorCode::Internal,
        format!("ecosystem '{ecosystem}' driver returned an unexpected response to {method}"),
    )
}
