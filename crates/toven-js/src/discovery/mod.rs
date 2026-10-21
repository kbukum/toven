//! JavaScript discovery: manifest scanning, dependency edges, and
//! blast-radius annotation.

mod blast;
mod edges;
mod scan;

pub(crate) use blast::files as install_inputs;
pub(crate) use edges::plain_range;
pub(crate) use scan::{JS, discover, root_packages};
