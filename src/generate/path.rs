//! Shared generated-config path formatting.

use std::path::Path;

/// Render a path for generated TOML with stable forward slashes.
pub fn toml_path(path: &Path) -> String {
    let normalized = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if normalized.is_empty() {
        ".".to_string()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::toml_path;

    #[test]
    fn renders_stable_forward_slash_paths() {
        assert_eq!(
            toml_path(&PathBuf::from("core").join("Cargo.toml")),
            "core/Cargo.toml"
        );
    }
}
