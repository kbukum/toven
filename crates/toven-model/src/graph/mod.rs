//! Federated dependency graph: the validated [`Graph`] type plus pure traversal
//! algorithms (topo wave-leveling and reverse-dependents closure).

mod dot;
mod model;
mod topo;

pub use dot::render;
pub use model::Graph;
