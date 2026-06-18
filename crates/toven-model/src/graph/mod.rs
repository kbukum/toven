//! Federated dependency graph: the validated [`Graph`] type plus pure traversal
//! algorithms (topo wave-leveling and reverse-dependents closure).

mod model;
mod topo;

pub use model::Graph;
