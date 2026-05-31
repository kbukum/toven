//! Core protocols shared by native and command adapters.

pub mod discovery;

pub use discovery::{
    AdapterOptions, DISCOVERY_SCHEMA_VERSION, DiscoverRequest, DiscoverResponse, DiscoveredModule,
};
pub(crate) use discovery::{validate_discovery_request_schema, validate_discovery_response};
