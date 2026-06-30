//! JSON value framing for the driver transport.
//!
//! Thin bindings over [`rskit_codec::framing`]: the generic length-delimited
//! frame transport carries one compact-JSON envelope per frame (see
//! [`envelope`](super::envelope)). Reads are bounded by [`MAX_FRAME_BYTES`] so a
//! malformed or hostile peer can never make a reader allocate without limit.

use std::io::{Read, Write};

use rskit_codec::{JsonCodec, framing};
use rskit_errors::AppResult;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Maximum accepted payload size for a single frame (16 MiB).
pub const MAX_FRAME_BYTES: usize = framing::DEFAULT_MAX_FRAME_BYTES;

/// Compact JSON keeps each envelope to one frame's worth of minimal bytes.
const CODEC: JsonCodec = JsonCodec::compact();

/// Serialize `value` to JSON and write it as one frame.
///
/// # Errors
/// Returns an error if serialization or the frame write fails.
pub fn write_value<W: Write, T: Serialize>(writer: &mut W, value: &T) -> AppResult<()> {
    framing::write_value(writer, &CODEC, value, MAX_FRAME_BYTES)
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
    framing::read_value(reader, &CODEC, max_bytes)
}

/// Deserialize an already-read frame `payload` from JSON into `T`.
///
/// Split from [`read_value`] so a caller that must inspect one frame as more
/// than one shape (a handshake reply that may be a `Welcome` or an error) can
/// decode the same bytes without re-reading the stream.
///
/// # Errors
/// Returns an error if the payload does not deserialize into `T`.
pub fn decode_value<T: DeserializeOwned>(payload: &[u8]) -> AppResult<T> {
    framing::decode_value(&CODEC, payload)
}

/// Read one raw length-delimited frame, bounded by `max_bytes`.
///
/// # Errors
/// Returns an error on a truncated frame or an underlying read failure.
pub fn read_frame<R: Read>(reader: &mut R, max_bytes: usize) -> AppResult<Option<Vec<u8>>> {
    framing::read_frame(reader, max_bytes)
}
