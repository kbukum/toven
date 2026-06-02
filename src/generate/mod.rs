//! User-facing config generation workflow.

mod cli;
mod global;
mod model;
mod path;
mod render;
mod workflow;
mod writer;

pub use cli::{GenerateCliOptions, run_generate};
pub use model::{
    GenerateContext, GenerateContributor, GenerateDocument, GenerateRequest, GeneratedProfile,
    GeneratedTask, TomlValue,
};
pub use path::toml_path;
pub use render::render_document;
pub use workflow::{GenerateOutcome, generate_config, request};
pub use writer::write_document;
