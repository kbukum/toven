//! Umbrella federation — hybrid loading of ecosystem adapters behind one port
//! trait.
//!
//! Bundled adapters are linked in-proc; any other canonical ecosystem is driven
//! out-of-process via a [`RemoteAdapter`] proxy that forwards each port call to
//! a separately-installed `toven-<eco> __serve` subprocess over a thin, framed
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
//! - [`provision`] — the explicit driver install surface.
//! - [`members`] — umbrella `[[members]]` enumeration across repos.
//! - [`compose`] — member-config composition + cross-member overlay/group
//!   layer.
//! - [`identity`] — member metadata stamping on the cross-repo union.
//! - [`rebase`] — rebase one member's discovery output into umbrella
//!   coordinates.
//! - [`baseline`] — per-member baseline specs and VCS reader views.
//! - [`project`] — open one deduped rskit-git reader/writer per member repo.
//! - [`release`] — federated release planning and per-member APPLY sharding.
//! - [`spine`] — the N-member Configure → Discover spine that unions members.
//! - [`sync`] — explicit member-repo provisioning (clone + clean-tree guard).

pub mod baseline;
pub mod compose;
pub mod identity;
pub mod members;
pub mod project;
pub mod protocol;
pub mod provision;
pub mod rebase;
pub mod release;
pub mod remote;
pub mod resolve;
pub mod serve;
pub mod spine;
pub mod sync;

pub use baseline::{MemberVcsReaders, OpenMemberVcsReaders};
pub use project::open_project_vcs;
pub use remote::RemoteAdapter;
pub use remote::wizard::{run_driver_wizard, wizard_io};
pub(crate) use resolve::driver_binary_name;
pub use resolve::{
    DriverBinary, PathDriverLocator, RemoteResolution, Resolution, resolve_adapters,
    resolve_ecosystem,
};
pub use serve::{serve, serve_wizard};
