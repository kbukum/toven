//! CLI mapping for `toven generate`.

use std::{io::Write, path::PathBuf};

use clap::ArgMatches;

use crate::{
    core::{AdapterId, AppResult},
    generate::{GenerateRequest, generate_config},
};

/// Parsed generate options.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GenerateCliOptions {
    /// Project root to inspect.
    pub root: PathBuf,
    /// Generated profile name.
    pub profile_name: String,
    /// Optional adapter filter.
    pub adapter: Option<AdapterId>,
    /// Explicit manifest hints.
    pub manifests: Vec<PathBuf>,
    /// Whether to write `toven.toml`.
    pub write: bool,
    /// Whether an existing config may be replaced.
    pub overwrite: bool,
}

/// Run `toven generate`.
pub fn run_generate(matches: &ArgMatches, stdout: &mut impl Write) -> AppResult<()> {
    let options = GenerateCliOptions::from_matches(matches)?;
    let outcome = generate_config(GenerateRequest {
        root: options.root,
        profile_name: options.profile_name,
        adapter: options.adapter,
        manifests: options.manifests,
        write: options.write,
        overwrite: options.overwrite,
    })?;
    if !matches.get_flag("write") {
        write!(stdout, "{}", outcome.rendered).map_err(crate::core::AppError::internal)?;
    }
    Ok(())
}

impl GenerateCliOptions {
    fn from_matches(matches: &ArgMatches) -> AppResult<Self> {
        let adapter = matches
            .get_one::<String>("adapter")
            .map(|adapter| AdapterId::new(adapter.clone()))
            .transpose()?;

        Ok(Self {
            root: PathBuf::from(
                matches
                    .get_one::<String>("root")
                    .expect("clap supplies root default"),
            ),
            profile_name: matches
                .get_one::<String>("profile")
                .expect("clap supplies profile default")
                .clone(),
            adapter,
            manifests: matches
                .get_many::<String>("manifest")
                .map(|values| values.map(PathBuf::from).collect())
                .unwrap_or_default(),
            write: matches.get_flag("write"),
            overwrite: matches.get_flag("overwrite"),
        })
    }
}
