//! Go discovery: `go mod edit -json` / `go work edit -json` parsing plus
//! blast-radius annotation.

mod blast;
mod metadata;

pub(crate) use metadata::discover;
