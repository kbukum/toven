//! Driver ports — the out-of-process `toven-<eco>` driver seams the engine
//! injects: locating a driver binary and driving one through its onboarding
//! wizard.
//!
//! Both contracts keep PATH discovery and subprocess onboarding out of the pure
//! resolution/init logic, so they stay testable without touching the real `PATH`
//! or spawning a real driver. Their concrete adapters live in the engine.

mod locator;
mod wizard;

pub use locator::DriverLocator;
pub use wizard::DriverWizard;
