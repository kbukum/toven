//! Discovery request/response vocabulary exchanged with a configured adapter.

mod request;
mod response;

pub use request::{DISCOVERY_SCHEMA_VERSION, DiscoverContext, DiscoverRequest};
pub use response::DiscoverResponse;
