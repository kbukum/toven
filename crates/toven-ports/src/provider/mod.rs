//! Provider port — the two-level `Provider → ConfiguredAdapter` seam plus the
//! config-less scaffold fragment.

mod configured;
mod entry;
mod scaffold;

pub use configured::ConfiguredAdapter;
pub use entry::Provider;
pub use scaffold::EcosystemFragment;
