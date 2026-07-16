//! argv templating — the Toven [`TaskVar`] vocabulary over rskit-util's
//! [`Template`](rskit_util::Template), plus the two-template [`CommandTemplate`].

mod command;
mod release_var;
mod var;

pub use command::CommandTemplate;
pub use release_var::ReleaseVar;
pub use var::TaskVar;
