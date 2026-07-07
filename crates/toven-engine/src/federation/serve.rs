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
use super::protocol::wizard::{
    WizardAnswers, WizardOffer, WizardOffering, WizardProbe, WizardResult,
};

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

    provider
        .configure(hello.config.clone())
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

/// Run the config-less wizard exchange over `reader`/`writer` — the driven half
/// of federated `toven init`.
///
/// This is a **two-round-trip** exchange. First, read one [`WizardProbe`], ask
/// every in-proc provider to self-detect under the named root and build its
/// [`Questionnaire`](toven_ports::Questionnaire), and reply with a [`WizardOffer`]. The driver then stays
/// alive holding its detections; it reads one [`WizardAnswers`], re-associates
/// each answer set with the matching stored detection, renders, and replies with
/// a [`WizardResult`]. A peer that closes before the probe (or before the
/// answers) is a clean no-op.
///
/// # Errors
/// Returns an error only on a transport failure; a provider's own detect /
/// questionnaire / render failure is reported to the umbrella as a typed
/// [`WizardOffer::Error`] or [`WizardResult::Error`].
pub fn serve_wizard<R: Read, W: Write>(
    providers: &[&dyn Provider],
    mut reader: R,
    mut writer: W,
) -> AppResult<()> {
    let Some(probe) = codec::read_value::<_, WizardProbe>(&mut reader, MAX_FRAME_BYTES)? else {
        // Peer closed before sending a probe: nothing to onboard.
        return Ok(());
    };

    if probe.schema_version != super::protocol::envelope::ENVELOPE_SCHEMA_VERSION {
        let offer = WizardOffer::Error(schema_mismatch(probe.schema_version));
        return codec::write_value(&mut writer, &offer);
    }

    let offerings = match probe_offerings(providers, &probe) {
        Ok(offerings) => offerings,
        Err(error) => {
            let offer = WizardOffer::Error(WireError::new(
                error.code().as_str(),
                error.message().to_string(),
            ));
            return codec::write_value(&mut writer, &offer);
        }
    };
    codec::write_value(&mut writer, &WizardOffer::Detected(offerings.clone()))?;

    let Some(answers) = codec::read_value::<_, WizardAnswers>(&mut reader, MAX_FRAME_BYTES)? else {
        // Peer closed after the offer without answering: a clean no-op.
        return Ok(());
    };

    let result = match render_fragments(providers, &offerings, &answers) {
        Ok(fragments) => WizardResult::Fragments(fragments),
        Err(error) => WizardResult::Error(WireError::new(
            error.code().as_str(),
            error.message().to_string(),
        )),
    };
    codec::write_value(&mut writer, &result)
}

/// The schema-mismatch wire error shared by the wizard's two reply points.
fn schema_mismatch(umbrella: u16) -> WireError {
    WireError::new(
        rskit_errors::ErrorCode::Conflict.as_str(),
        format!(
            "umbrella speaks envelope schema v{umbrella}, but this driver requires v{}",
            super::protocol::envelope::ENVELOPE_SCHEMA_VERSION
        ),
    )
}

/// Detect + build a questionnaire for every provider that applies under the root.
fn probe_offerings(
    providers: &[&dyn Provider],
    probe: &WizardProbe,
) -> AppResult<Vec<WizardOffering>> {
    let mut offerings = Vec::new();
    for provider in providers {
        if let Some(detection) = provider.detect(&probe.project_root)? {
            let questionnaire = provider.questionnaire(&detection)?;
            offerings.push(WizardOffering {
                detection,
                questionnaire,
            });
        }
    }
    Ok(offerings)
}

/// Render each answered ecosystem, re-associating answers with the stored
/// detection and dispatching to the provider that serves that ecosystem.
fn render_fragments(
    providers: &[&dyn Provider],
    offerings: &[WizardOffering],
    answers: &WizardAnswers,
) -> AppResult<Vec<toven_ports::EcosystemFragment>> {
    let mut fragments = Vec::new();
    for entry in &answers.entries {
        let offering = offerings
            .iter()
            .find(|offering| offering.detection.ecosystem == entry.ecosystem)
            .ok_or_else(|| {
                rskit_errors::AppError::invalid_input(
                    "wizard.answers",
                    format!(
                        "umbrella answered ecosystem '{}', which this driver did not offer",
                        entry.ecosystem
                    ),
                )
            })?;
        let provider = providers
            .iter()
            .find(|provider| provider.ecosystem_id() == &entry.ecosystem)
            .ok_or_else(|| {
                rskit_errors::AppError::invalid_input(
                    "wizard.answers",
                    format!("this driver does not serve ecosystem '{}'", entry.ecosystem),
                )
            })?;
        fragments.push(provider.render(&offering.detection, &entry.answers)?);
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
        Request::ToolchainProbe => Response::ToolchainProbe(adapter.toolchain_probe()),
        Request::RunStrategy { kind } => Response::RunStrategy(adapter.run_strategy_default(&kind)),
        // Handled by the caller before reaching here.
        Request::Shutdown => Response::Bye,
    }
}
