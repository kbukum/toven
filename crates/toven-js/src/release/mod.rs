//! [`NpmRegistryTarget`] — the `js` release target: npm-registry publication
//! of `package.json` packages.
//!
//! It reads and writes versions in `package.json` with format-preserving
//! edits, refreshes the root's lockfile once the manifests change, packs with
//! the root's package manager (so `workspace:` and `catalog:` ranges resolve
//! the way that manager's users expect) or the root's configured pack
//! command, and publishes the packed tarball
//! with npm. Every external command is argv-only, bounded, and timed out; the
//! registry token reaches npm through the child environment, never argv.

mod manifest;
pub(crate) mod pack;
mod registry;
mod target;
#[cfg(test)]
mod tests;

pub(crate) use target::NpmRegistryTarget;
