//! argv templating — the Toven [`TaskVar`] vocabulary over rskit-util's
//! [`Template`](rskit_util::Template), plus the two-template [`CommandTemplate`].

mod command;
mod var;

pub use command::CommandTemplate;
pub use var::TaskVar;
