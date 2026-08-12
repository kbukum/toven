//! Unit vocabulary — the single action shape ([`Unit`]) every Toven capability
//! takes, its [`Backing`] (how the action is satisfied), and the [`Composite`]
//! chain of units.
//!
//! Layer-0 vocabulary only: it *names* the one spine on which tasks, native
//! capabilities, delegated tools, and composite chains are otherwise uniform —
//! discovered, planned mutation-free, gated on apply, and reported the same way
//! — differing only in their [`Backing`]. It holds no behavior; the execution
//! spine that consumes it uniformly is built in later steps.

mod backing;
mod composite;
mod spec;

pub use backing::Backing;
pub use composite::Composite;
pub use spec::Unit;
