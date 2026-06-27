//! Umbrella federation — hybrid loading of ecosystem adapters behind one port
//! trait.
//!
//! Bundled adapters are linked in-proc; any other canonical ecosystem is driven
//! out-of-process via a [`RemoteAdapter`] proxy that forwards each port call to a
//! separately-installed `toven-<eco> __serve` subprocess over a thin, framed
//! stdio transport. The engine's discover/configure loop is unchanged — an
//! out-of-proc adapter is just another `dyn ConfiguredAdapter`.
//!
//! ## Modules
//! - [`protocol`] — the wire transport: length-delimited framing, the versioned
//!   envelope of model/port types, and semver handshake negotiation.
//! - [`remote`] — the umbrella-side [`RemoteAdapter`] proxy (argv-only spawn,
//!   bounded RPC).
//! - [`mod@serve`] — the driven-binary `__serve` port-server loop.
//! - [`resolve`] — the four-way dispatch and remote-adapter resolution.
//! - [`provision`] — the explicit `driver install` / `federation sync` surface.

pub mod protocol;
pub mod provision;
pub mod remote;
pub mod resolve;
pub mod serve;

pub use remote::RemoteAdapter;
pub use resolve::{
    DriverBinary, DriverLocator, PathDriverLocator, RemoteResolution, Resolution, resolve_adapters,
    resolve_ecosystem,
};
pub use serve::serve;
