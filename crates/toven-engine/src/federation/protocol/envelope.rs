//! The versioned wire envelope — a 1:1 RPC mirror of the port methods built
//! **only** from the shared [`toven_model`]/[`toven_ports`] vocabulary.
//!
//! There are no bespoke DTOs: every payload field is an existing model or port
//! value type ([`DiscoverRequest`], [`DiscoverResponse`],
//! [`ToolchainProbe`], [`TaskKind`], [`RunStrategy`], [`CommonEcosystemConfig`]),
//! so a model change cascades straight to the wire. The umbrella sends a
//! [`Hello`] and a stream of [`Request`]s; the driven server answers with a
//! [`Welcome`] then one [`Response`] per request.

use rskit_config::RawValue;
use serde::{Deserialize, Serialize};
use toven_model::EcosystemId;
use toven_ports::{
    CommonEcosystemConfig, DiscoverRequest, DiscoverResponse, RunStrategy, TaskKind, ToolchainProbe,
};

/// Envelope schema version, bumped on any breaking wire-shape change.
pub const ENVELOPE_SCHEMA_VERSION: u16 = 1;

/// The opening handshake the umbrella sends to a freshly spawned driver.
///
/// Carries the protocol version (negotiated by [`handshake`](super::handshake)),
/// the ecosystem the umbrella wants this server to act as, and that ecosystem's
/// raw `[ecosystems.<id>]` config as the canonical [`RawValue`] subtree the
/// server hands straight to its own `configure`.
// `config` is a `RawValue` (JSON), which is not `Eq`, so only `PartialEq` is derivable.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Hello {
    /// Envelope schema version ([`ENVELOPE_SCHEMA_VERSION`]).
    pub schema_version: u16,
    /// Semver protocol version the umbrella speaks (e.g. `1.0.0`).
    pub protocol: String,
    /// Ecosystem the server should configure and answer for.
    pub ecosystem: EcosystemId,
    /// The ecosystem's `[ecosystems.<id>]` subtree as a canonical raw value.
    pub config: RawValue,
}

impl Hello {
    /// Build a hello for `ecosystem` carrying its raw config subtree.
    #[must_use]
    pub const fn new(protocol: String, ecosystem: EcosystemId, config: RawValue) -> Self {
        Self {
            schema_version: ENVELOPE_SCHEMA_VERSION,
            protocol,
            ecosystem,
            config,
        }
    }
}

/// The server's handshake reply: protocol echo, capabilities, resolved common config.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Welcome {
    /// Envelope schema version ([`ENVELOPE_SCHEMA_VERSION`]).
    pub schema_version: u16,
    /// Semver protocol version the server speaks.
    pub protocol: String,
    /// The port methods this server supports.
    pub capabilities: Capabilities,
    /// The resolved engine-common config the umbrella caches for `common()`.
    pub common: CommonEcosystemConfig,
}

/// Which port methods a driver advertises after configure.
///
/// The PLAN-side surface — `discover`, `toolchain`, and
/// `run_strategy` — is **required**: the [`RemoteAdapter`](super::super::remote)
/// proxy cannot function without it, so the umbrella treats any driver that
/// reports a required capability as `false` as an incompatible driver and fails
/// fast (see [`Capabilities::missing_required`]). `release` is the only optional
/// surface (`release = false` ⇒ no release target). New capability flags default
/// to `false` so an older umbrella reading a newer server's set degrades safely.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default, Deserialize, Serialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)] // a flag-per-port capability set
pub struct Capabilities {
    /// Server answers [`Request::Discover`].
    pub discover: bool,
    /// Server answers [`Request::ToolchainProbe`].
    pub toolchain: bool,
    /// Server answers [`Request::RunStrategy`].
    pub run_strategy: bool,
    /// Server exposes a release target.
    pub release: bool,
}

impl Capabilities {
    /// The capability set every first-party driver advertises: the PLAN-side port
    /// surface, with release capability-gated off.
    #[must_use]
    pub const fn plan_surface() -> Self {
        Self {
            discover: true,
            toolchain: true,
            run_strategy: true,
            release: false,
        }
    }

    /// The names of the **required** PLAN-side capabilities this driver fails to
    /// advertise, in declaration order.
    ///
    /// The proxy cannot answer port calls without `discover`, `toolchain`, and
    /// `run_strategy`, so any of these reported as `false` marks the driver
    /// incompatible. An empty result means the required surface is satisfied;
    /// `release` is optional and never reported here. The runnable task table is
    /// not an RPC — it travels in the [`Welcome`]'s resolved common config.
    #[must_use]
    pub fn missing_required(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if !self.discover {
            missing.push("discover");
        }
        if !self.toolchain {
            missing.push("toolchain");
        }
        if !self.run_strategy {
            missing.push("run_strategy");
        }
        missing
    }
}

/// One request mirroring a [`ConfiguredAdapter`](toven_ports::ConfiguredAdapter)
/// method (plus a graceful `Shutdown`).
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub enum Request {
    /// Mirror of `discover`.
    Discover(DiscoverRequest),
    /// Mirror of `toolchain_probe`.
    ToolchainProbe,
    /// Mirror of `run_strategy_default`.
    RunStrategy {
        /// Task kind whose default ordering is requested.
        kind: TaskKind,
    },
    /// Graceful teardown request; the server replies [`Response::Bye`] and exits.
    Shutdown,
}

/// One response mirroring a port method's return (or a typed error).
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
#[non_exhaustive]
pub enum Response {
    /// Result of `discover`.
    Discover(DiscoverResponse),
    /// Result of `toolchain_probe`.
    ToolchainProbe(ToolchainProbe),
    /// Result of `run_strategy_default`.
    RunStrategy(RunStrategy),
    /// Acknowledgement of [`Request::Shutdown`].
    Bye,
    /// The server failed to answer; carries a typed, displayable cause.
    Error(WireError),
}

/// A serialized adapter failure carried back over the wire.
///
/// The engine never reconstructs the remote's full error tree; it preserves the
/// classification (`code`) and message so the umbrella surfaces an actionable,
/// typed PLAN error.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct WireError {
    /// Stable error-code label (the remote [`ErrorCode`](rskit_errors::ErrorCode) name).
    pub code: String,
    /// Human-readable failure message.
    pub message: String,
}

impl WireError {
    /// Build a wire error from a code label and message.
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Capabilities, Hello, Request, Response, WireError};
    use toven_model::EcosystemId;
    use toven_ports::DiscoverRequest;

    fn rust() -> EcosystemId {
        EcosystemId::new("rust").expect("valid id")
    }

    #[test]
    fn hello_round_trips_through_json() {
        let hello = Hello::new(
            "1.0.0".to_string(),
            rust(),
            serde_json::json!({ "manifests": [] }),
        );
        let json = serde_json::to_string(&hello).expect("serialize");
        let back: Hello = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(hello, back);
    }

    #[test]
    fn request_round_trips_through_json() {
        let request = Request::Discover(DiscoverRequest::new(
            toven_model::AbsPath::new("/repo").expect("absolute"),
        ));
        let json = serde_json::to_string(&request).expect("serialize");
        let back: Request = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(request, back);
    }

    #[test]
    fn response_error_round_trips_through_json() {
        let response = Response::Error(WireError::new(
            rskit_errors::ErrorCode::InvalidInput.as_str(),
            "bad subtree",
        ));
        let json = serde_json::to_string(&response).expect("serialize");
        let back: Response = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(response, back);
    }

    #[test]
    fn plan_surface_omits_release() {
        let caps = Capabilities::plan_surface();
        assert!(caps.discover && caps.toolchain && caps.run_strategy);
        assert!(!caps.release);
    }

    #[test]
    fn plan_surface_satisfies_required_capabilities() {
        assert!(Capabilities::plan_surface().missing_required().is_empty());
    }

    #[test]
    fn missing_required_reports_absent_plan_capabilities_but_not_release() {
        let caps = Capabilities::default();
        assert_eq!(
            caps.missing_required(),
            vec!["discover", "toolchain", "run_strategy"],
        );
    }
}
