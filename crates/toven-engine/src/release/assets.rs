//! The release scope's hosted assets: the union of every module's per-module
//! `[…release.host].assets` contribution.
//!
//! Assets are owned **per module**, not per ecosystem: in a mixed repo the
//! binary app declares the archive/checksum assets while the registry libraries
//! declare none (they contribute release notes only). The asset phases
//! (`package`, `checksums`, `sign`, `verify`) therefore operate over the union
//! of the per-module contributions — which, because only the binary-producing
//! modules declare archives, is exactly those modules' asset set — rather than a
//! single ecosystem-wide list.
//!
//! The union is taken deterministically — modules in `ModuleKey` order, each
//! module's assets in declared order — and de-duplicated so the emitted
//! `SHA256SUMS` manifest stays byte-stable and reviewable, and so a shared asset
//! declared by more than one module is packaged and checksummed once.

use std::collections::BTreeSet;

use toven_model::ModuleKey;

use super::ResolvedReleaseSettings;

/// The de-duplicated union of every releasable module's declared hosted-release
/// assets — modules in `ModuleKey` order, each module's assets in declared
/// order.
///
/// Each entry is one module's contribution to the shared release: a
/// binary-producing module contributes its archives (and the `SHA256SUMS`
/// manifest and signature sidecars), while a library module that declares no
/// `host.assets` contributes nothing. The result is therefore scoped to the
/// binary-producing modules without any ecosystem-wide asset list.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn declared_release_assets(
    settings: &std::collections::BTreeMap<ModuleKey, ResolvedReleaseSettings>,
) -> Vec<&String> {
    let mut seen = BTreeSet::new();
    let mut ordered = Vec::new();
    for resolved in settings.values() {
        for asset in &resolved.host.assets {
            if seen.insert(asset.as_str()) {
                ordered.push(asset);
            }
        }
    }
    ordered
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use toven_model::{EcosystemId, ModuleKey, ModuleRef};
    use toven_ports::{HostConfig, ReleaseConfig};

    use super::{ResolvedReleaseSettings, declared_release_assets};

    fn eid() -> EcosystemId {
        EcosystemId::new("rust").unwrap()
    }

    fn mkey(name: &str) -> ModuleKey {
        ModuleKey::bare(ModuleRef::new(eid(), name).unwrap())
    }

    fn settings_with_assets(assets: Option<Vec<&str>>) -> ResolvedReleaseSettings {
        let host = assets.map(|assets| HostConfig {
            forge: Some("github".into()),
            assets: Some(assets.into_iter().map(str::to_string).collect()),
            ..HostConfig::default()
        });
        let config = ReleaseConfig {
            host,
            ..ReleaseConfig::default()
        };
        ResolvedReleaseSettings::resolve(&config, None).unwrap()
    }

    #[test]
    fn a_library_module_without_assets_contributes_nothing() {
        // A registry library (notes only) plus a binary app (archives): the
        // union is scoped to exactly the binary module's assets.
        let mut settings = BTreeMap::new();
        settings.insert(mkey("corelib"), settings_with_assets(None));
        settings.insert(
            mkey("app"),
            settings_with_assets(Some(vec![
                "dist/app-x86_64-unknown-linux-gnu.tar.gz",
                "dist/SHA256SUMS",
            ])),
        );

        let assets = declared_release_assets(&settings);

        assert_eq!(
            assets,
            vec![
                "dist/app-x86_64-unknown-linux-gnu.tar.gz",
                "dist/SHA256SUMS"
            ]
        );
    }

    #[test]
    fn a_shared_asset_declared_by_two_modules_is_unioned_once() {
        let mut settings = BTreeMap::new();
        settings.insert(
            mkey("app"),
            settings_with_assets(Some(vec!["dist/SHA256SUMS", "dist/app.tar.gz"])),
        );
        settings.insert(
            mkey("tool"),
            settings_with_assets(Some(vec!["dist/SHA256SUMS", "dist/tool.tar.gz"])),
        );

        let assets = declared_release_assets(&settings);

        // Deterministic (module then declared) order, deduped: the shared
        // `SHA256SUMS` appears once.
        assert_eq!(
            assets,
            vec!["dist/SHA256SUMS", "dist/app.tar.gz", "dist/tool.tar.gz"]
        );
    }
}
