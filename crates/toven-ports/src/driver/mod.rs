//! Driver ports — the out-of-process `toven-<eco>` driver seams the engine
//! injects: locating a driver binary and probing one for scaffold fragments.
//!
//! Both contracts keep PATH discovery and subprocess scaffolding out of the pure
//! resolution/generation logic, so they stay testable without touching the real
//! `PATH` or spawning a real driver. Their concrete adapters live in the engine.

mod locator;
mod scaffolder;

pub use locator::DriverLocator;
pub use scaffolder::DriverScaffolder;
