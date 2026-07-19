//! Raw child output vocabulary: [`OutputStream`] and [`UnitOutput`].
//!
//! Carried on a separate channel from [`Event`](crate::event::Event):
//! coarse-grained and not part of the typed event union, so high-throughput
//! build output never pays per-line (de)serialization.

use serde::{Deserialize, Serialize};

/// The output stream a [`UnitOutput`] chunk came from.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputStream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// A chunk of raw child output, attributed to a unit.
#[derive(Debug, Clone, Eq, PartialEq, Deserialize, Serialize)]
pub struct UnitOutput {
    /// Unit the output belongs to.
    pub unit_id: String,
    /// Stream the bytes came from.
    pub stream: OutputStream,
    /// Raw bytes (not interpreted as UTF-8).
    pub bytes: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::{OutputStream, UnitOutput};

    #[test]
    fn unit_output_round_trips() {
        let output = UnitOutput {
            unit_id: "u1".into(),
            stream: OutputStream::Stderr,
            bytes: b"hello".to_vec(),
        };
        let json = serde_json::to_string(&output).unwrap();
        let back: UnitOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(output, back);
    }
}
