//! The per-unit raw child-output channel.
//!
//! Structured lifecycle flows through the typed [`Event`](toven_model::Event)
//! stream and a [`Reporter`](toven_ports::Reporter). Raw child output is
//! deliberately *not* per-line vocabulary: it travels on this separate, coarse
//! [`UnitOutput`](toven_model::UnitOutput) channel so high-throughput build
//! output never pays per-line (de)serialization.
//!
//! This module owns the concurrency-ordering *policy* and never prints: it
//! buffers normal units and flushes a labeled block on finish (spilling extra
//! blocks early if a unit exceeds the per-unit buffer cap, to bound any single
//! unit's buffer),
//! live-tails persistent units, and routes the bytes to an injected
//! [`RawOutputSink`](toven_ports::RawOutputSink) adapter (the port lives in
//! `toven-ports`, beside [`Reporter`](toven_ports::Reporter); the CLI provides
//! the concrete, terminal-bound sink). The APPLY exec layer feeds it
//! `UnitOutput` chunks plus lifecycle signals.

mod channel;

pub use channel::{OutputMode, UnitOutputChannel};
