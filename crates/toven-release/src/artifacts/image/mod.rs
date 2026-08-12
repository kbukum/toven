//! `release image` verb and the buildx/cosign [`ImagePhase`](toven_ports::ImagePhase) adapter.
//!
//! The engine owns image *policy*: which modules run the image phase (those
//! declaring `[…release.image]`), the resolved image name/tag rendered from the
//! module's declared version, the primary-plus-mirror registry set, whether the
//! pushed digest is signed, and the mutation-free `--dry-run` preview. The only
//! reusable primitive is "run a subprocess" (the shared [`ToolRunner`](toven_ports::ToolRunner));
//! [`BuildxImagePhase`] shells to `docker buildx` and `cosign` argv-only,
//! inheriting the ambient registry credentials — it embeds no secret and
//! captures none.
//!
//! Image publication is immutable: pushing a tag that already exists at a
//! *different* digest fails closed, and recovery is a forward-fix version, never
//! a moved tag.

mod buildx;
mod phase;

#[cfg(test)]
mod tests;

pub use buildx::BuildxImagePhase;
pub(super) use phase::resolved_image_requests;
pub use phase::{ImageModuleOutcome, ImageOptions, ImagePhaseStatus, ImageReport, release_image};
