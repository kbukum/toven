//! Length-delimited frame codec for the driver transport (thin, Toven-owned).
//!
//! Each frame is a 4-byte big-endian unsigned length prefix followed by exactly
//! that many payload bytes. The payload is a JSON-serialized envelope value (see
//! [`envelope`](super::envelope)). Frames are length-bounded by
//! [`MAX_FRAME_BYTES`] so a malformed or hostile peer can never make a reader
//! allocate without limit.
//!
//! This is the local stand-in recorded as the generic rskit follow-up **D5** (a
//! reusable length-delimited serde framing codec). It is deliberately not built
//! on `rskit-codec` — that is a structured-text value-tree codec, not wire
//! framing — and stays here until D5 lands upstream.

use std::io::{Read, Write};

use rskit_errors::{AppError, AppResult, ErrorCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Maximum accepted payload size for a single frame (16 MiB).
///
/// Generous enough for the largest realistic discovery response yet bounded so a
/// corrupt length prefix cannot trigger an unbounded allocation.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Width of the big-endian length prefix that precedes every payload.
const LEN_PREFIX_BYTES: usize = 4;

/// Write one length-delimited frame carrying `payload`.
///
/// # Errors
/// Returns an error if `payload` exceeds [`MAX_FRAME_BYTES`] or the underlying
/// writer fails.
pub fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> AppResult<()> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(AppError::invalid_input(
            "frame",
            format!(
                "payload of {} bytes exceeds the {MAX_FRAME_BYTES}-byte frame limit",
                payload.len()
            ),
        ));
    }
    // `payload.len() <= MAX_FRAME_BYTES` (< u32::MAX), so the cast cannot truncate.
    let len = u32::try_from(payload.len())
        .map_err(|_| AppError::invalid_input("frame", "payload length exceeds u32 range"))?;
    writer
        .write_all(&len.to_be_bytes())
        .map_err(|error| transport_error("write frame length", &error))?;
    writer
        .write_all(payload)
        .map_err(|error| transport_error("write frame payload", &error))?;
    writer
        .flush()
        .map_err(|error| transport_error("flush frame", &error))?;
    Ok(())
}

/// Read one length-delimited frame.
///
/// Returns `Ok(None)` on a clean end-of-stream observed *before* any length byte
/// (the peer closed the connection between frames). A partial prefix or payload
/// is a hard transport error.
///
/// # Errors
/// Returns an error on a truncated frame, a length above [`MAX_FRAME_BYTES`], or
/// any underlying read failure.
pub fn read_frame<R: Read>(reader: &mut R, max_bytes: usize) -> AppResult<Option<Vec<u8>>> {
    let mut prefix = [0u8; LEN_PREFIX_BYTES];
    match read_exact_or_eof(reader, &mut prefix)? {
        ReadEnd::Eof => return Ok(None),
        ReadEnd::Filled => {}
    }
    let len = u32::from_be_bytes(prefix) as usize;
    if len > max_bytes {
        return Err(AppError::invalid_input(
            "frame",
            format!("incoming frame length {len} exceeds the {max_bytes}-byte limit"),
        ));
    }
    let mut payload = vec![0u8; len];
    reader
        .read_exact(&mut payload)
        .map_err(|error| transport_error("read frame payload", &error))?;
    Ok(Some(payload))
}

/// Serialize `value` to JSON and write it as one frame.
///
/// # Errors
/// Returns an error if serialization or the frame write fails.
pub fn write_value<W: Write, T: Serialize>(writer: &mut W, value: &T) -> AppResult<()> {
    let payload = serde_json::to_vec(value).map_err(|error| {
        AppError::new(
            ErrorCode::Internal,
            format!("could not serialize driver frame: {error}"),
        )
    })?;
    write_frame(writer, &payload)
}

/// Read one frame and deserialize it from JSON into `T`.
///
/// Returns `Ok(None)` on a clean end-of-stream between frames.
///
/// # Errors
/// Returns an error on a transport failure or a payload that does not
/// deserialize into `T`.
pub fn read_value<R: Read, T: DeserializeOwned>(
    reader: &mut R,
    max_bytes: usize,
) -> AppResult<Option<T>> {
    let Some(payload) = read_frame(reader, max_bytes)? else {
        return Ok(None);
    };
    decode_value(&payload).map(Some)
}

/// Deserialize an already-read frame `payload` from JSON into `T`.
///
/// Split from [`read_value`] so a caller that must inspect one frame as more than
/// one shape (e.g. a handshake reply that may be a `Welcome` or an error) can
/// decode the same bytes without re-reading the stream.
///
/// # Errors
/// Returns an error if the payload does not deserialize into `T`.
pub fn decode_value<T: DeserializeOwned>(payload: &[u8]) -> AppResult<T> {
    serde_json::from_slice(payload).map_err(|error| {
        AppError::new(
            ErrorCode::InvalidInput,
            format!("could not deserialize driver frame: {error}"),
        )
    })
}

/// Whether a fixed-size read filled the buffer or hit a clean EOF first.
enum ReadEnd {
    /// The buffer was completely filled.
    Filled,
    /// End-of-stream was reached before any byte was read.
    Eof,
}

/// Fill `buf` exactly, distinguishing a clean leading EOF from a truncated read.
fn read_exact_or_eof<R: Read>(reader: &mut R, buf: &mut [u8]) -> AppResult<ReadEnd> {
    let mut read = 0;
    while read < buf.len() {
        match reader.read(&mut buf[read..]) {
            Ok(0) => {
                if read == 0 {
                    return Ok(ReadEnd::Eof);
                }
                return Err(AppError::new(
                    ErrorCode::ServiceUnavailable,
                    "driver transport: driver stream ended mid-frame (truncated length prefix)",
                ));
            }
            Ok(count) => read += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(transport_error("read frame length", &error)),
        }
    }
    Ok(ReadEnd::Filled)
}

/// Build a typed transport error preserving the underlying I/O cause.
fn transport_error(context: &str, error: &std::io::Error) -> AppError {
    AppError::new(
        ErrorCode::ServiceUnavailable,
        format!("driver transport: {context}: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::{MAX_FRAME_BYTES, read_frame, read_value, write_frame, write_value};

    #[test]
    fn round_trips_a_frame() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, b"hello driver").expect("write");
        let mut cursor = std::io::Cursor::new(buffer);
        let frame = read_frame(&mut cursor, MAX_FRAME_BYTES)
            .expect("read")
            .expect("frame present");
        assert_eq!(frame, b"hello driver");
    }

    #[test]
    fn clean_eof_between_frames_is_none() {
        let mut cursor = std::io::Cursor::new(Vec::new());
        let frame = read_frame(&mut cursor, MAX_FRAME_BYTES).expect("read");
        assert!(frame.is_none());
    }

    #[test]
    fn truncated_prefix_is_a_transport_error() {
        let mut cursor = std::io::Cursor::new(vec![0u8, 0u8]);
        let error = read_frame(&mut cursor, MAX_FRAME_BYTES).expect_err("truncated prefix errors");
        // A mid-prefix EOF is a transport truncation (peer closed/crashed), not a
        // user-input fault, so it is classified like every other transport failure.
        assert_eq!(error.code(), rskit_errors::ErrorCode::ServiceUnavailable);
    }

    #[test]
    fn oversized_outgoing_frame_is_rejected() {
        let mut buffer = Vec::new();
        let payload = vec![0u8; 8];
        // A tiny cap proves the bound is enforced without allocating 16 MiB.
        assert!(
            read_frame(
                &mut std::io::Cursor::new({
                    write_frame(&mut buffer, &payload).expect("write");
                    buffer
                }),
                4
            )
            .is_err()
        );
    }

    #[test]
    fn value_round_trips_as_json_frame() {
        let mut buffer = Vec::new();
        write_value(&mut buffer, &vec!["a".to_string(), "b".to_string()]).expect("write");
        let mut cursor = std::io::Cursor::new(buffer);
        let value: Vec<String> = read_value(&mut cursor, MAX_FRAME_BYTES)
            .expect("read")
            .expect("present");
        assert_eq!(value, vec!["a".to_string(), "b".to_string()]);
    }
}
