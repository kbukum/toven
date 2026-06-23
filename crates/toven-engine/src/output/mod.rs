//! The per-unit raw child-output channel (event-report Decision C).
//!
//! Structured lifecycle flows through the typed [`Event`](toven_model::Event)
//! stream and a [`Reporter`](toven_ports::Reporter). Raw child output is
//! deliberately *not* per-line vocabulary: it travels on this separate, coarse
//! [`UnitOutput`](toven_model::UnitOutput) channel so high-throughput build
//! output never pays per-line (de)serialization.
//!
//! This module owns the concurrency-ordering *policy* and never prints: it
//! buffers normal units and flushes one labeled block on finish, live-tails
//! persistent units, and routes the bytes to an injected
//! [`RawOutputSink`](toven_ports::RawOutputSink) adapter (the port lives in
//! `toven-ports`, beside [`Reporter`](toven_ports::Reporter); the CLI provides
//! the concrete, terminal-bound sink). The APPLY exec layer (step 8) feeds it
//! `UnitOutput` chunks plus lifecycle signals.

mod channel;

pub use channel::{OutputMode, UnitOutputChannel};
