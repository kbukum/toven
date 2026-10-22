//! Provider port — the two-level `Provider → ConfiguredAdapter` seam plus the
//! wizard `render` fragment.

mod configured;
mod doctor;
mod entry;
mod fragment;

pub use configured::ConfiguredAdapter;
pub use doctor::{CheckVerdict, DoctorScope, EcosystemCheck};
pub use entry::Provider;
pub use fragment::EcosystemFragment;
