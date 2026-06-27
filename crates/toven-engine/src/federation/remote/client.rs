//! [`RpcClient`] — the umbrella-side request/response engine over one driver
//! connection.
//!
//! Owns the framed reader/writer (a subprocess's pipes in production, an
//! in-process pipe in tests) and, when driving a subprocess, the child handle
//! used for the per-RPC timeout watchdog and teardown. Every call is bounded:
//! the watchdog kills a wedged driver so a blocked read cannot hang the
//! synchronous PLAN spine forever.

use std::io::{Read, Write};
use std::time::Duration;

use rskit_errors::ErrorCode;

use super::super::protocol::codec::{self, MAX_FRAME_BYTES};
use super::super::protocol::envelope::{Hello, Request, Response, Welcome};
use super::super::protocol::handshake::DriverFault;
use super::process::{ChildHandle, Watchdog};

/// Default per-RPC deadline for a driven ecosystem (one minute).
#[allow(clippy::redundant_pub_crate)]
pub(crate) const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_mins(1);

/// The umbrella's client over a single driver connection.
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct RpcClient {
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    child: Option<ChildHandle>,
    timeout: Duration,
    ecosystem: String,
    shutdown_sent: bool,
}

impl RpcClient {
    /// Build a client over an arbitrary framed reader/writer (used by tests and
    /// as the core both connection paths share).
    pub(crate) fn new(
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        child: Option<ChildHandle>,
        timeout: Duration,
        ecosystem: String,
    ) -> Self {
        Self {
            reader,
            writer,
            child,
            timeout,
            ecosystem,
            shutdown_sent: false,
        }
    }

    /// Send the opening [`Hello`] and read the [`Welcome`] reply.
    ///
    /// # Errors
    /// Returns a typed driver fault on a transport failure, a malformed/short
    /// reply, or a remote handshake rejection.
    pub(crate) fn handshake(&mut self, hello: &Hello) -> Result<Welcome, DriverFault> {
        self.send_hello(hello)?;
        let payload = self.recv_frame()?.ok_or_else(|| {
            DriverFault::Transport("driver closed the stream during handshake".to_string())
        })?;
        // The server replies with a `Welcome` on success but a `Response::Error`
        // on a rejected handshake (unknown ecosystem, incompatible protocol, bad
        // config). Decode the same frame as either so a remote rejection keeps its
        // typed code/message instead of collapsing into an opaque transport error.
        if let Ok(welcome) = codec::decode_value::<Welcome>(&payload) {
            return Ok(welcome);
        }
        match codec::decode_value::<Response>(&payload) {
            Ok(Response::Error(wire)) => Err(DriverFault::Remote {
                code: wire.code,
                message: wire.message,
            }),
            _ => Err(DriverFault::Malformed(
                "handshake reply was neither a Welcome nor a typed error".to_string(),
            )),
        }
    }

    /// Issue one [`Request`] and return its [`Response`].
    ///
    /// # Errors
    /// Returns a typed driver fault on transport failure, timeout, a malformed
    /// reply, or a typed remote error response.
    pub(crate) fn call(&mut self, request: &Request) -> Result<Response, DriverFault> {
        self.send_request(request)?;
        let response = self.recv::<Response>()?.ok_or_else(|| {
            DriverFault::Transport("driver closed the stream mid-call".to_string())
        })?;
        if let Response::Error(wire) = response {
            return Err(DriverFault::Remote {
                code: wire.code,
                message: wire.message,
            });
        }
        Ok(response)
    }

    /// The ecosystem id this client drives (for error tagging).
    pub(crate) fn ecosystem(&self) -> &str {
        &self.ecosystem
    }

    /// Write a request frame.
    fn send_request(&mut self, request: &Request) -> Result<(), DriverFault> {
        codec::write_value(&mut self.writer, request)
            .map_err(|error| DriverFault::Transport(error.message().to_string()))
    }

    /// Write a hello frame.
    fn send_hello(&mut self, hello: &Hello) -> Result<(), DriverFault> {
        codec::write_value(&mut self.writer, hello)
            .map_err(|error| DriverFault::Transport(error.message().to_string()))
    }

    /// Read one framed value under the RPC deadline (killing a wedged driver).
    fn recv<T: serde::de::DeserializeOwned>(&mut self) -> Result<Option<T>, DriverFault> {
        self.recv_frame()?
            .map(|payload| {
                codec::decode_value::<T>(&payload)
                    .map_err(|error| DriverFault::Malformed(error.message().to_string()))
            })
            .transpose()
    }

    /// Read one raw frame under the RPC deadline (killing a wedged driver).
    ///
    /// Returns `Ok(None)` on a clean end-of-stream between frames. A watchdog
    /// kill is classified as [`DriverFault::Timeout`] whether it surfaces as a
    /// read error or as a clean EOF at the frame boundary.
    fn recv_frame(&mut self) -> Result<Option<Vec<u8>>, DriverFault> {
        let watchdog: Option<Watchdog> = self
            .child
            .as_ref()
            .map(|child| child.arm_watchdog(self.timeout));
        let outcome = codec::read_frame(&mut self.reader, MAX_FRAME_BYTES);
        let timed_out = watchdog.map_or(false, Watchdog::disarm);
        // A watchdog kill unblocks the read either as an error or as a clean EOF at
        // the frame boundary (`Ok(None)`); classify both as a timeout, not transport.
        if timed_out {
            return Err(DriverFault::Timeout);
        }
        match outcome {
            Ok(value) => Ok(value),
            // `read_frame` returns `InvalidInput` for a protocol-level malformed
            // frame (e.g. an oversized length prefix) and `ServiceUnavailable` for
            // a genuine transport failure (truncation, broken pipe). Preserve that
            // distinction in the fault taxonomy instead of flattening both.
            Err(error) => Err(if error.code() == ErrorCode::InvalidInput {
                DriverFault::Malformed(error.message().to_string())
            } else {
                DriverFault::Transport(error.message().to_string())
            }),
        }
    }
}

impl Drop for RpcClient {
    fn drop(&mut self) {
        if !self.shutdown_sent {
            // Best-effort graceful shutdown; dropping the writer also signals EOF.
            let _ = codec::write_value(&mut self.writer, &Request::Shutdown);
            self.shutdown_sent = true;
        }
        // `child` (if any) reaps the subprocess on its own drop.
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{DriverFault, Request, RpcClient};

    /// A frame whose length prefix advertises a payload above `MAX_FRAME_BYTES`
    /// is a protocol-level malformed frame, not a transport failure, so the
    /// client must classify it as [`DriverFault::Malformed`] (preserving the
    /// typed `InvalidInput` taxonomy) rather than [`DriverFault::Transport`].
    #[test]
    fn oversized_frame_prefix_is_classified_as_malformed() {
        // 0xFFFF_FFFF bytes far exceeds the 16 MiB frame cap.
        let oversized_prefix = vec![0xFFu8, 0xFF, 0xFF, 0xFF];
        let mut client = RpcClient::new(
            Box::new(std::io::Cursor::new(oversized_prefix)),
            Box::new(Vec::new()),
            None,
            Duration::from_mins(1),
            "go".to_string(),
        );

        let fault = client
            .call(&Request::DefaultTasks)
            .expect_err("an oversized frame prefix must fault");
        assert!(
            matches!(fault, DriverFault::Malformed(_)),
            "oversized frame must be Malformed, got {fault:?}"
        );
    }
}
