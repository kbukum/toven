//! The `__serve` port-server loop — the driven-binary side of the transport.
//!
//! A `toven-<eco> __serve` process runs this loop: it reads a [`Hello`], finds
//! the matching in-proc [`Provider`], configures it, replies with a [`Welcome`]
//! (protocol + capabilities + resolved common config), then answers one
//! [`Request`] per port method until the umbrella sends [`Request::Shutdown`] or
//! closes the stream.
//!
//! The server answers **port calls only** — it never runs builds and never
//! writes child output. The frame stream owns `stdout`; all human-facing
//! diagnostics belong on `stderr` (the caller's responsibility). The loop is
//! fully synchronous and runs on its own process/thread, so blocking frame I/O
//! never touches an async runtime.

use std::io::{Read, Write};

use rskit_errors::AppResult;
use toven_ports::{ConfiguredAdapter, Provider};

use super::protocol::codec::{self, MAX_FRAME_BYTES};
use super::protocol::envelope::{Capabilities, Hello, Request, Response, Welcome, WireError};
use super::protocol::handshake::{PROTOCOL_VERSION, negotiate, protocol_version};
use super::protocol::scaffold::{ScaffoldOutcome, ScaffoldRequest};

/// Run the port-server loop over `reader`/`writer` using the in-proc `providers`.
///
/// Reads the opening [`Hello`], configures the requested ecosystem's provider,
/// emits the [`Welcome`], then serves requests until shutdown or EOF.
///
/// # Errors
/// Returns an error on a transport failure or a handshake the server cannot
/// honor (unknown ecosystem, incompatible protocol, or a `configure` rejection)
/// — after first attempting to report it to the umbrella as a typed frame.
pub fn serve<R: Read, W: Write>(
    providers: &[&dyn Provider],
    mut reader: R,
    mut writer: W,
) -> AppResult<()> {
    let Some(hello) = codec::read_value::<_, Hello>(&mut reader, MAX_FRAME_BYTES)? else {
        // Peer closed before sending a hello: nothing to serve.
        return Ok(());
    };

    let adapter = match handshake(providers, &hello) {
        Ok(adapter) => adapter,
        Err(wire) => {
            // Best-effort: tell the umbrella why, then surface the failure,
            // preserving the remote classification (e.g. a protocol-major
            // mismatch stays a Conflict) instead of flattening to InvalidInput.
            let _ = codec::write_value(&mut writer, &Response::Error(wire.clone()));
            let code = rskit_errors::ErrorCode::from_wire(&wire.code)
                .unwrap_or(rskit_errors::ErrorCode::InvalidInput);
            return Err(rskit_errors::AppError::new(
                code,
                format!("driver handshake failed: {}: {}", wire.code, wire.message),
            ));
        }
    };

    let welcome = Welcome {
        schema_version: super::protocol::envelope::ENVELOPE_SCHEMA_VERSION,
        protocol: PROTOCOL_VERSION.to_string(),
        capabilities: Capabilities::plan_surface(),
        common: adapter.common().clone(),
    };
    codec::write_value(&mut writer, &welcome)?;

    serve_requests(adapter.as_ref(), &mut reader, &mut writer)
}

/// Validate the handshake and configure the requested ecosystem's adapter.
fn handshake(
    providers: &[&dyn Provider],
    hello: &Hello,
) -> Result<Box<dyn ConfiguredAdapter>, WireError> {
    if hello.schema_version != super::protocol::envelope::ENVELOPE_SCHEMA_VERSION {
        return Err(WireError::new(
            rskit_errors::ErrorCode::Conflict.as_str(),
            format!(
                "umbrella speaks envelope schema v{}, but this driver requires v{}",
                hello.schema_version,
                super::protocol::envelope::ENVELOPE_SCHEMA_VERSION
            ),
        ));
    }

    if let Err(fault) = negotiate(&protocol_version(), &hello.protocol) {
        let error = fault.into_app_error(hello.ecosystem.as_str());
        return Err(WireError::new(
            error.code().as_str(),
            error.message().to_string(),
        ));
    }

    let provider = providers
        .iter()
        .find(|provider| provider.ecosystem_id() == &hello.ecosystem)
        .ok_or_else(|| {
            WireError::new(
                rskit_errors::ErrorCode::NotFound.as_str(),
                format!("this driver does not serve ecosystem '{}'", hello.ecosystem),
            )
        })?;

    let value: toml::Value = toml::from_str(&hello.config_toml).map_err(|error| {
        WireError::new(
            rskit_errors::ErrorCode::InvalidInput.as_str(),
            format!("could not parse driver config TOML: {error}"),
        )
    })?;

    provider
        .configure(value)
        .map_err(|error| WireError::new(error.code().as_str(), error.message().to_string()))
}

/// Serve port-call requests until shutdown or a clean end-of-stream.
fn serve_requests<R: Read, W: Write>(
    adapter: &dyn ConfiguredAdapter,
    reader: &mut R,
    writer: &mut W,
) -> AppResult<()> {
    while let Some(request) = codec::read_value::<_, Request>(reader, MAX_FRAME_BYTES)? {
        if matches!(request, Request::Shutdown) {
            // The client is leaving; acknowledging is best-effort. It may have
            // already closed its read end (a broken pipe here is not a failure).
            let _ = codec::write_value(writer, &Response::Bye);
            break;
        }
        let response = answer(adapter, request);
        codec::write_value(writer, &response)?;
    }
    Ok(())
}

/// Run the config-less scaffold exchange over `reader`/`writer`.
///
/// Reads one [`ScaffoldRequest`], asks every in-proc provider to self-detect its
/// ecosystem under the named root, and replies with a single [`ScaffoldOutcome`]
/// carrying the detected fragments (or a typed error). This is the driven half
/// of federated `toven generate`; a peer that closes before sending a request is
/// a clean no-op.
///
/// # Errors
/// Returns an error only on a transport failure; a provider's own scaffold
/// failure is reported to the umbrella as a typed [`ScaffoldOutcome::Error`].
pub fn serve_scaffold<R: Read, W: Write>(
    providers: &[&dyn Provider],
    mut reader: R,
    mut writer: W,
) -> AppResult<()> {
    let Some(request) = codec::read_value::<_, ScaffoldRequest>(&mut reader, MAX_FRAME_BYTES)?
    else {
        // Peer closed before sending a request: nothing to scaffold.
        return Ok(());
    };

    if request.schema_version != super::protocol::envelope::ENVELOPE_SCHEMA_VERSION {
        let outcome = ScaffoldOutcome::Error(WireError::new(
            rskit_errors::ErrorCode::Conflict.as_str(),
            format!(
                "umbrella speaks envelope schema v{}, but this driver requires v{}",
                request.schema_version,
                super::protocol::envelope::ENVELOPE_SCHEMA_VERSION
            ),
        ));
        return codec::write_value(&mut writer, &outcome);
    }

    let outcome = match detect_fragments(providers, &request) {
        Ok(fragments) => ScaffoldOutcome::Fragments(fragments),
        Err(error) => ScaffoldOutcome::Error(WireError::new(
            error.code().as_str(),
            error.message().to_string(),
        )),
    };
    codec::write_value(&mut writer, &outcome)
}

/// Run every provider's config-less detection under the request's root.
fn detect_fragments(
    providers: &[&dyn Provider],
    request: &ScaffoldRequest,
) -> AppResult<Vec<toven_ports::EcosystemFragment>> {
    let mut fragments = Vec::new();
    for provider in providers {
        if let Some(fragment) = provider.scaffold(&request.project_root)? {
            fragments.push(fragment);
        }
    }
    Ok(fragments)
}

/// Compute the response for one request, mapping adapter failures to a typed
/// [`Response::Error`].
fn answer(adapter: &dyn ConfiguredAdapter, request: Request) -> Response {
    match request {
        Request::Discover(req) => match adapter.discover(&req) {
            Ok(response) => Response::Discover(response),
            Err(error) => Response::Error(WireError::new(
                error.code().as_str(),
                error.message().to_string(),
            )),
        },
        Request::DefaultTasks => Response::DefaultTasks(adapter.default_tasks()),
        Request::ToolchainProbe => Response::ToolchainProbe(adapter.toolchain_probe()),
        Request::RunStrategy { kind } => Response::RunStrategy(adapter.run_strategy_default(&kind)),
        // Handled by the caller before reaching here.
        Request::Shutdown => Response::Bye,
    }
}
