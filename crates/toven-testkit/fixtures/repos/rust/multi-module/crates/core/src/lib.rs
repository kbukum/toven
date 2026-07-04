//! Middle crate depending on `util`.

/// Compose the greeting from `util`.
#[must_use]
pub fn greeting() -> String {
    util::greeting()
}
