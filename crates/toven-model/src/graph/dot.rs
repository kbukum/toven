//! Graphviz DOT rendering for the validated [`Graph`].
//!
//! A single, pure renderer shared by every DOT consumer (the `graph --format
//! dot` projection and the `release depgraphs` artifact writer) so node naming
//! and identifier escaping never drift between them.

use crate::graph::Graph;

/// Render a [`Graph`] as a Graphviz DOT digraph.
///
/// Nodes are the member-scoped module keys; edges are directed `from` → `to` in
/// insertion order. Identifiers are escaped so a module key containing a quote
/// or backslash cannot break the DOT syntax.
#[must_use]
pub fn render(graph: &Graph) -> String {
    let mut out = String::from("digraph toven {\n");
    for module in graph.modules() {
        out.push_str("  \"");
        out.push_str(&escape_id(&module.key().to_string()));
        out.push_str("\";\n");
    }
    for edge in graph.edges() {
        out.push_str("  \"");
        out.push_str(&escape_id(&edge.from.to_string()));
        out.push_str("\" -> \"");
        out.push_str(&escape_id(&edge.to.to_string()));
        out.push_str("\";\n");
    }
    out.push_str("}\n");
    out
}

/// Escape a DOT quoted-string identifier (`"` and `\`).
#[must_use]
fn escape_id(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::{escape_id, render};
    use crate::{
        edge::{DepKind, Edge},
        graph::Graph,
        identity::{EcosystemId, MemberId, ModuleRef, RepoPath},
        module::Module,
    };

    fn module(name: &str) -> Module {
        Module::new(
            ModuleRef::new(EcosystemId::new("rust").expect("valid id"), name).expect("valid ref"),
            RepoPath::new(format!("crates/{name}")).expect("valid path"),
        )
    }

    fn graph() -> Graph {
        let core = module("core");
        let app = module("app");
        let edge = Edge::new(app.key(), core.key(), DepKind::Normal);
        Graph::build(vec![core, app], vec![edge]).expect("valid graph")
    }

    #[test]
    fn render_emits_a_digraph_with_directed_edges() {
        let rendered = render(&graph());
        assert!(rendered.starts_with("digraph toven {"));
        assert!(rendered.contains("\"rust:app\" -> \"rust:core\";"));
        assert!(rendered.trim_end().ends_with('}'));
    }

    #[test]
    fn render_uses_member_scoped_node_names() {
        let mut core = module("core");
        core.member = Some(MemberId::new("lib").expect("member"));
        let rendered = render(&Graph::build(vec![core], Vec::new()).expect("valid graph"));
        assert!(rendered.contains("\"lib/rust:core\";"));
    }

    #[test]
    fn escape_id_escapes_quotes_and_backslashes() {
        assert_eq!(
            escape_id("rust:app\"#build\\dev"),
            "rust:app\\\"#build\\\\dev"
        );
    }
}
