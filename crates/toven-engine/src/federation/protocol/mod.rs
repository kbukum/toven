//! The driver wire protocol — thin, Toven-owned framing + a versioned envelope
//! of the shared vocabulary, plus semver handshake negotiation.
//!
//! Split into three declare-only responsibilities:
//! - [`codec`] — length-delimited frame read/write.
//! - [`envelope`] — the [`Hello`]/[`Welcome`] handshake and the
//!   [`Request`]/[`Response`] RPC mirror, built only from model/port types.
//! - [`wizard`] — the config-less two-round-trip [`WizardProbe`]/[`WizardOffer`]
//!   → [`WizardAnswers`]/[`WizardResult`] exchange that powers federated
//!   `toven init`.
//! - [`handshake`] — protocol-version negotiation and the typed [`DriverFault`]
//!   classification.

pub mod codec;
pub mod envelope;
pub mod handshake;
pub mod wizard;

pub use codec::{MAX_FRAME_BYTES, read_value, write_value};
pub use envelope::{
    Capabilities, ENVELOPE_SCHEMA_VERSION, Hello, Request, Response, Welcome, WireError,
};
pub use handshake::{DriverFault, PROTOCOL_VERSION, negotiate, protocol_version};
pub use wizard::{
    WizardAnswerEntry, WizardAnswers, WizardOffer, WizardOffering, WizardProbe, WizardResult,
};
