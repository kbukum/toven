//! Command discovery protocol wiring.
//!
//! Command discovery uses the same `DiscoverRequest` and `DiscoverResponse`
//! structs as native adapters, serialized as JSON over process stdin/stdout.

pub use crate::core::{DiscoverRequest, DiscoverResponse};
