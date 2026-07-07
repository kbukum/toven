//! The umbrella-side config-less wizard client.
//!
//! It spawns a `toven-<eco> __init` driver, asks what it detects under a
//! project root, answers its questionnaire locally, and collects the rendered
//! [`EcosystemFragment`]s.
//!
//! This is the federated half of `toven init`. Unlike [`RemoteAdapter`](super::RemoteAdapter)
//! it is a **two-round-trip** exchange: spawn, send a [`WizardProbe`], read a
//! [`WizardOffer`], answer each offered [`Questionnaire`](toven_ports::Questionnaire) through the injected
//! [`AnswerProvider`], send a [`WizardAnswers`], read a [`WizardResult`], and
//! tear the child down. **The driver stays alive across the prompt** so a single
//! detection is answered and rendered without re-probing.
//!
//! Each driver round trip is bounded by a fresh kill-watchdog armed only around
//! that RPC: the probe→offer read and the answers→result read. The local
//! questionnaire between them — human think-time — runs **unbounded**, so a user
//! taking their time answering never trips a driver timeout. A wedged driver
//! read still unblocks via the watchdog and is classified as a
//! [`DriverFault::Timeout`]. The `ChildHandle` drop-kill remains the backstop
//! against a zombie child.

use std::io::{Read, Write};
use std::path::Path;

use rskit_errors::{AppError, AppResult, ErrorCode};
use toven_ports::{AnswerProvider, EcosystemFragment};

use super::super::protocol::codec::{self, MAX_FRAME_BYTES};
use super::super::protocol::handshake::DriverFault;
use super::super::protocol::wizard::{
    WizardAnswerEntry, WizardAnswers, WizardOffer, WizardOffering, WizardProbe, WizardResult,
};
use super::client::DEFAULT_RPC_TIMEOUT;
use super::process::{self, ChildHandle, Watchdog};

/// Spawn `program __init` and run its onboarding wizard under `project_root`,
/// answering each questionnaire through `answers`.
///
/// Each of the two driver round trips is bounded by a fresh watchdog armed only
/// around that RPC; the local questionnaire between them runs unbounded. A fired
/// watchdog is classified as a [`DriverFault::Timeout`], and the `ChildHandle`
/// drop-kill reaps the child on the way out.
///
/// # Errors
/// Returns a typed error if the driver cannot be spawned, either round trip
/// fails or times out, a reply is malformed, answering fails, or the driver
/// reports a typed detect/render failure. A located driver that misbehaves is a
/// hard error, never a silent skip (the caller decides whether an *absent*
/// driver is skipped).
pub fn run_driver_wizard(
    program: &Path,
    project_root: &Path,
    answers: &dyn AnswerProvider,
) -> AppResult<Vec<EcosystemFragment>> {
    let driver = process::spawn(program, "__init")
        .map_err(|fault| fault.into_app_error(&program.display().to_string()))?;
    let child = ChildHandle::new(driver.child);

    let label = program.display().to_string();
    let mut stdin = driver.stdin;
    let mut stdout = driver.stdout;
    // Arm a fresh per-RPC watchdog against the live child; the exchange leaves
    // the answering phase unbounded on its own.
    let result = run_wizard(
        &mut stdout,
        &mut stdin,
        &label,
        project_root,
        answers,
        &mut || Some(child.arm_watchdog(DEFAULT_RPC_TIMEOUT)),
    );
    // Signal EOF so the driver exits cleanly; `child` reaps it on drop.
    drop(stdin);
    result
}

/// Run the wizard exchange over an arbitrary framed reader/writer.
///
/// The shared core of [`run_driver_wizard`]: writes a [`WizardProbe`], reads a
/// [`WizardOffer`], answers each questionnaire via `answers`, writes a
/// [`WizardAnswers`], and reads a [`WizardResult`]. This unbounded seam is what
/// tests drive directly over in-process pipes; [`run_driver_wizard`] runs the
/// same exchange with a per-RPC watchdog around the spawned child's stdio.
///
/// # Errors
/// Returns a typed error on a transport failure, a malformed reply, an answering
/// failure, or a driver detect/render failure.
pub fn wizard_io<R: Read, W: Write>(
    mut reader: R,
    mut writer: W,
    program_label: &str,
    project_root: &Path,
    answers: &dyn AnswerProvider,
) -> AppResult<Vec<EcosystemFragment>> {
    run_wizard(
        &mut reader,
        &mut writer,
        program_label,
        project_root,
        answers,
        &mut || None,
    )
}

/// Drive the two-round-trip wizard, bounding **only** the driver reads.
///
/// `arm` yields a fresh [`Watchdog`] for each driver RPC (or `None` for the
/// unbounded [`wizard_io`] seam). The probe→offer and answers→result reads are
/// each guarded; the local `answer_offerings` questionnaire between them runs
/// with no watchdog, so human think-time never trips a driver timeout.
fn run_wizard<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    program_label: &str,
    project_root: &Path,
    answers: &dyn AnswerProvider,
    arm: &mut dyn FnMut() -> Option<Watchdog>,
) -> AppResult<Vec<EcosystemFragment>> {
    let offer = guarded(arm, program_label, || {
        read_offer(writer, reader, project_root)
    })?;
    let offerings = decode_offer(Path::new(program_label), offer)?;
    if offerings.is_empty() {
        return Ok(Vec::new());
    }

    // The local questionnaire is unbounded: no watchdog is armed here.
    let entries = answer_offerings(&offerings, answers)?;

    let result = guarded(arm, program_label, || send_answers(writer, reader, entries))?;
    decode_result(Path::new(program_label), result)
}

/// Run one guarded driver RPC: arm a watchdog, run the framed I/O, disarm, and
/// classify a fired timer as a [`DriverFault::Timeout`].
///
/// A fired watchdog means the blocking read was unblocked by the deadline action
/// (a child kill in production), so any resulting transport error is reported as
/// a timeout rather than an opaque transport failure. When `arm` yields `None`
/// (the unbounded seam), the RPC runs without a deadline.
fn guarded<T>(
    arm: &mut dyn FnMut() -> Option<Watchdog>,
    program_label: &str,
    rpc: impl FnOnce() -> Result<T, DriverFault>,
) -> AppResult<T> {
    let watchdog = arm();
    let outcome = rpc();
    let timed_out = watchdog.is_some_and(Watchdog::disarm);
    match outcome {
        Ok(value) => Ok(value),
        Err(_) if timed_out => Err(DriverFault::Timeout.into_app_error(program_label)),
        Err(fault) => Err(fault.into_app_error(program_label)),
    }
}

/// Write the probe and read the offer over the framed transport.
fn read_offer<W: Write, R: Read>(
    writer: &mut W,
    reader: &mut R,
    project_root: &Path,
) -> Result<WizardOffer, DriverFault> {
    let probe = WizardProbe::new(project_root.to_path_buf());
    codec::write_value(writer, &probe)
        .map_err(|error| DriverFault::Transport(error.message().to_string()))?;
    codec::read_value::<_, WizardOffer>(reader, MAX_FRAME_BYTES)
        .map_err(|error| DriverFault::Malformed(error.message().to_string()))?
        .ok_or_else(|| {
            DriverFault::Transport("driver closed the stream before offering".to_string())
        })
}

/// Write the answers and read the result over the framed transport.
fn send_answers<W: Write, R: Read>(
    writer: &mut W,
    reader: &mut R,
    entries: Vec<WizardAnswerEntry>,
) -> Result<WizardResult, DriverFault> {
    let answers = WizardAnswers::new(entries);
    codec::write_value(writer, &answers)
        .map_err(|error| DriverFault::Transport(error.message().to_string()))?;
    codec::read_value::<_, WizardResult>(reader, MAX_FRAME_BYTES)
        .map_err(|error| DriverFault::Malformed(error.message().to_string()))?
        .ok_or_else(|| {
            DriverFault::Transport("driver closed the stream before replying".to_string())
        })
}

/// Answer every offered questionnaire through the injected [`AnswerProvider`].
fn answer_offerings(
    offerings: &[WizardOffering],
    answers: &dyn AnswerProvider,
) -> AppResult<Vec<WizardAnswerEntry>> {
    offerings
        .iter()
        .map(|offering| {
            Ok(WizardAnswerEntry {
                ecosystem: offering.detection.ecosystem.clone(),
                answers: answers.answers_for(&offering.questionnaire)?,
            })
        })
        .collect()
}

/// Map a driver's [`WizardOffer`] into its offerings or a typed error.
fn decode_offer(program: &Path, offer: WizardOffer) -> AppResult<Vec<WizardOffering>> {
    match offer {
        WizardOffer::Detected(offerings) => Ok(offerings),
        WizardOffer::Error(wire) => Err(wire_error(program, "detect", &wire)),
    }
}

/// Map a driver's [`WizardResult`] into the rendered fragments or a typed error.
fn decode_result(program: &Path, result: WizardResult) -> AppResult<Vec<EcosystemFragment>> {
    match result {
        WizardResult::Fragments(fragments) => Ok(fragments),
        WizardResult::Error(wire) => Err(wire_error(program, "render", &wire)),
    }
}

/// Build a typed error from a driver's wire failure at `stage`.
fn wire_error(program: &Path, stage: &str, wire: &super::super::protocol::WireError) -> AppError {
    let code = ErrorCode::from_wire(&wire.code).unwrap_or(ErrorCode::Internal);
    AppError::new(
        code,
        format!(
            "driver '{}' failed to {stage}: {}: {}",
            program.display(),
            wire.code,
            wire.message
        ),
    )
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;

    use rskit_errors::{AppResult, ErrorCode};
    use toven_model::EcosystemId;
    use toven_ports::{AnswerProvider, Answers, Detection, Questionnaire};

    use super::super::super::protocol::codec;
    use super::super::super::protocol::wizard::{WizardOffer, WizardOffering, WizardResult};
    use super::super::process::Watchdog;
    use super::run_wizard;
    use std::path::Path;

    fn go() -> EcosystemId {
        EcosystemId::new("go").expect("valid id")
    }

    /// An [`AnswerProvider`] that sleeps `delay` before answering — a stand-in
    /// for a human taking their time at the local questionnaire prompt.
    struct SlowAnswers {
        delay: Duration,
        calls: AtomicUsize,
    }

    impl AnswerProvider for SlowAnswers {
        fn answers_for(&self, _questionnaire: &Questionnaire) -> AppResult<Answers> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            Ok(Answers::new())
        }
    }

    /// A reader whose first read blocks until the watchdog's deadline action
    /// signals it, then returns EOF — modelling a driver that never replies and
    /// is only unblocked when the umbrella kills it.
    struct WedgedReader {
        unblock: mpsc::Receiver<()>,
    }

    impl Read for WedgedReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            // Park until the deadline action fires (or the sender is dropped),
            // then report EOF so the framed read resolves to a closed stream.
            let _ = self.unblock.recv();
            Ok(0)
        }
    }

    fn offer_frame() -> Vec<u8> {
        let offer = WizardOffer::Detected(vec![WizardOffering {
            detection: Detection::bare(go()),
            questionnaire: Questionnaire::empty(go()),
        }]);
        let mut buf = Vec::new();
        codec::write_value(&mut buf, &offer).expect("encode offer");
        buf
    }

    #[test]
    fn slow_local_answering_does_not_trip_the_per_rpc_watchdog() {
        // The offer and result are pre-buffered so the two guarded driver reads
        // return instantly; the only slow step is answering, which runs between
        // them WITHOUT a watchdog. Answering for 200ms under a 20ms per-RPC
        // timeout must still succeed: human think-time is unbounded by design.
        let mut framed = offer_frame();
        codec::write_value(&mut framed, &WizardResult::Fragments(Vec::new()))
            .expect("encode result");
        let mut reader = Cursor::new(framed);
        let mut writer: Vec<u8> = Vec::new();

        let answers = SlowAnswers {
            delay: Duration::from_millis(200),
            calls: AtomicUsize::new(0),
        };
        // The per-RPC timeout (20ms) is far shorter than the answering delay.
        let mut arm = || Some(Watchdog::arm(Duration::from_millis(20), || {}));

        let fragments = run_wizard(
            &mut reader,
            &mut writer,
            "toven-go",
            Path::new("/repo"),
            &answers,
            &mut arm,
        )
        .expect("a slow human answer must not trip the driver timeout");

        assert!(fragments.is_empty());
        assert_eq!(answers.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_wedged_driver_read_is_classified_as_a_timeout() {
        // The driver never replies to the probe: the umbrella's offer read blocks
        // until the watchdog's deadline action unblocks it (EOF). A fired watchdog
        // must classify the resulting closed-stream error as a typed Timeout.
        let (tx, rx) = mpsc::channel::<()>();
        let mut reader = WedgedReader { unblock: rx };
        let mut writer: Vec<u8> = Vec::new();
        let sender = Arc::new(Mutex::new(Some(tx)));

        let mut arm = || {
            let sender = Arc::clone(&sender);
            Some(Watchdog::arm(Duration::from_millis(20), move || {
                let taken = sender
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(tx) = taken {
                    let _ = tx.send(());
                }
            }))
        };

        let answers = SlowAnswers {
            delay: Duration::from_millis(0),
            calls: AtomicUsize::new(0),
        };
        let error = run_wizard(
            &mut reader,
            &mut writer,
            "toven-go",
            Path::new("/repo"),
            &answers,
            &mut arm,
        )
        .expect_err("a wedged driver read must fault");

        assert_eq!(error.code(), ErrorCode::Timeout, "{error}");
        // Answering is never reached when the very first driver read wedges.
        assert_eq!(answers.calls.load(Ordering::SeqCst), 0);
    }
}
