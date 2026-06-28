//! The umbrella-side driver proxy: a [`RemoteAdapter`] that forwards each port
//! call to a driven `toven-<eco> __serve` subprocess.
//!
//! Declare-only: the transport client lives in `client`, the argv-only
//! subprocess spawn + kill watchdog in `process`, and the
//! [`ConfiguredAdapter`](toven_ports::ConfiguredAdapter) proxy in `adapter`.

mod adapter;
pub(crate) mod client;
pub(crate) mod process;

pub use adapter::RemoteAdapter;
