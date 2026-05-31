//! Safe generated-config writes.

use std::{fs, path::Path};

use crate::core::{AppError, AppResult};

/// Write generated TOML to `root/toven.toml`.
pub fn write_document(root: &Path, rendered: &str, overwrite: bool) -> AppResult<()> {
    if !root.is_dir() {
        return Err(AppError::invalid_input(
            "generate.root",
            format!("root '{}' is not a directory", root.display()),
        ));
    }
    let path = root.join("toven.toml");
    if path.exists() && !overwrite {
        return Err(AppError::invalid_input(
            "generate.write",
            format!(
                "{} already exists; pass --overwrite to replace it",
                path.display()
            ),
        ));
    }

    let temp = root.join(format!(".toven.toml.tmp-{}", std::process::id()));
    fs::write(&temp, rendered).map_err(AppError::internal)?;
    fs::rename(&temp, &path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        AppError::internal(error)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::write_document;

    #[test]
    fn refuses_existing_config_without_overwrite() {
        let root = rskit_testutil::test_workspace!("generate-writer");
        fs::write(root.path().join("toven.toml"), "existing").expect("write existing config");

        let error = write_document(root.path(), "new", false).expect_err("existing config fails");

        assert!(error.message.contains("already exists"));
        assert_eq!(
            fs::read_to_string(root.path().join("toven.toml")).expect("read config"),
            "existing"
        );
    }
}
