//! Freeform adapter metadata shared by modules and workspaces.

use std::collections::BTreeMap;

use serde_json::Value;

/// Freeform, adapter-supplied metadata keyed by name.
///
/// A *safe unknown* (`serde_json::Value`) rather than an unchecked any: adapters
/// attach ecosystem-specific facts (topology hints, release coordinates, report
/// fields) that the engine carries opaquely. Ordered (`BTreeMap`) so serialized
/// payloads are deterministic across the driver boundary.
pub type Metadata = BTreeMap<String, Value>;
