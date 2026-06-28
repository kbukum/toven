//! The umbrella-side config-less scaffold probe: spawn a `toven-<eco>
//! __scaffold` driver, ask what it detects under a project root, and collect its
//! [`EcosystemFragment`]s.
//!
//! This is the federated half of `toven generate`. Unlike [`RemoteAdapter`](super::RemoteAdapter)
//! it is a **one-shot** exchange — there is no config to bake and no streaming
//! port surface — so it spawns, sends a single [`ScaffoldRequest`], reads a
//! single [`ScaffoldOutcome`], and tears the child down. The blocking read is
//! bounded by the same kill-watchdog the port protocol uses so a wedged driver
//! cannot hang the (synchronous) generate spine forever.

use std::io::{Read, Write};
use std::path::Path;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::EcosystemFragment;

use super::super::protocol::codec::{self, MAX_FRAME_BYTES};
use super::super::protocol::handshake::DriverFault;
use super::super::protocol::scaffold::{ScaffoldOutcome, ScaffoldRequest};
use super::client::DEFAULT_RPC_TIMEOUT;
use super::process::{self, ChildHandle};

/// Spawn `program __scaffold` and probe it for the fragments it detects under
/// `project_root`.
///
/// # Errors
/// Returns a typed error if the driver cannot be spawned, the exchange fails or
/// times out, the reply is malformed, or the driver reports a typed scaffold
/// failure. A located driver that misbehaves is a hard error, never a silent
/// skip (the caller decides whether an *absent* driver is skipped).
pub fn probe_driver(program: &Path, project_root: &Path) -> AppResult<Vec<EcosystemFragment>> {
    let driver = process::spawn(program, "__scaffold")
        .map_err(|fault| fault.into_app_error(&program.display().to_string()))?;
    let child = ChildHandle::new(driver.child);
    let watchdog = child.arm_watchdog(DEFAULT_RPC_TIMEOUT);

    let label = program.display().to_string();
    let mut stdin = driver.stdin;
    let mut stdout = driver.stdout;
    let result = probe_io(&mut stdout, &mut stdin, &label, project_root);
    // Signal EOF so the driver exits cleanly, then resolve the watchdog: a fired
    // timer means the blocking read was unblocked by a kill, so classify it as a
    // timeout rather than an opaque transport error.
    drop(stdin);
    let timed_out = watchdog.disarm();

    match result {
        Ok(fragments) => Ok(fragments),
        Err(_error) if timed_out => Err(DriverFault::Timeout.into_app_error(&label)),
        Err(error) => Err(error),
    }
}

/// Run the scaffold exchange over an arbitrary framed reader/writer.
///
/// The shared core of [`probe_driver`]: writes one [`ScaffoldRequest`], reads one
/// [`ScaffoldOutcome`], and decodes it into fragments or a typed error.
/// [`probe_driver`] wraps this over the spawned child's stdio (adding the
/// kill-watchdog timeout classification); tests drive it directly over in-process
/// pipes without a subprocess.
///
/// # Errors
/// Returns a typed error on a transport failure, a malformed reply, or a driver
/// scaffold failure.
pub fn probe_io<R: Read, W: Write>(
    mut reader: R,
    mut writer: W,
    program_label: &str,
    project_root: &Path,
) -> AppResult<Vec<EcosystemFragment>> {
    let outcome = exchange(&mut writer, &mut reader, project_root)
        .map_err(|fault| fault.into_app_error(program_label))?;
    decode_outcome(Path::new(program_label), outcome)
}

/// Write the request and read the reply over the framed transport.
fn exchange<W: Write, R: Read>(
    writer: &mut W,
    reader: &mut R,
    project_root: &Path,
) -> Result<ScaffoldOutcome, DriverFault> {
    let request = ScaffoldRequest::new(project_root.to_path_buf());
    codec::write_value(writer, &request)
        .map_err(|error| DriverFault::Transport(error.message().to_string()))?;
    codec::read_value::<_, ScaffoldOutcome>(reader, MAX_FRAME_BYTES)
        .map_err(|error| DriverFault::Malformed(error.message().to_string()))?
        .ok_or_else(|| {
            DriverFault::Transport("driver closed the stream before replying".to_string())
        })
}

/// Map a driver's [`ScaffoldOutcome`] into the detected fragments or a typed error.
fn decode_outcome(program: &Path, outcome: ScaffoldOutcome) -> AppResult<Vec<EcosystemFragment>> {
    match outcome {
        ScaffoldOutcome::Fragments(fragments) => Ok(fragments),
        ScaffoldOutcome::Error(wire) => {
            let code = ErrorCode::from_wire(&wire.code).unwrap_or(ErrorCode::Internal);
            Err(AppError::new(
                code,
                format!(
                    "driver '{}' failed to scaffold: {}: {}",
                    program.display(),
                    wire.code,
                    wire.message
                ),
            ))
        }
    }
}
