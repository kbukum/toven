//! Safe generated-config writes.

use std::{
    fs,
    path::{Path, PathBuf},
};

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
    replace_config(&temp, &path, overwrite)?;
    Ok(())
}

fn replace_config(temp: &Path, path: &Path, overwrite: bool) -> AppResult<()> {
    if !path.exists() {
        return rename_temp(temp, path);
    }

    if !overwrite {
        let _ = fs::remove_file(temp);
        return Err(AppError::invalid_input(
            "generate.write",
            format!(
                "{} already exists; pass --overwrite to replace it",
                path.display()
            ),
        ));
    }

    let backup = unique_backup_path(path);
    fs::rename(path, &backup).map_err(|error| {
        let _ = fs::remove_file(temp);
        AppError::internal(error)
    })?;

    if let Err(error) = rename_temp(temp, path) {
        let _ = fs::rename(&backup, path);
        return Err(error);
    }

    fs::remove_file(&backup).map_err(AppError::internal)?;
    Ok(())
}

fn rename_temp(temp: &Path, path: &Path) -> AppResult<()> {
    fs::rename(temp, path).map_err(|error| {
        let _ = fs::remove_file(temp);
        AppError::internal(error)
    })
}

fn unique_backup_path(path: &Path) -> PathBuf {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    for attempt in 0.. {
        let candidate = directory.join(format!(
            ".toven.toml.backup-{}-{attempt}",
            std::process::id()
        ));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("unbounded backup path search should always find a free path")
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

    #[test]
    fn overwrites_existing_config_when_requested() {
        let root = rskit_testutil::test_workspace!("generate-writer-overwrite");
        fs::write(root.path().join("toven.toml"), "existing").expect("write existing config");

        write_document(root.path(), "new", true).expect("overwrite succeeds");

        assert_eq!(
            fs::read_to_string(root.path().join("toven.toml")).expect("read config"),
            "new"
        );
    }
}
