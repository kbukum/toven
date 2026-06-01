use std::path::Path;

use crate::binary::TovenOutput;

pub fn normalize_output(output: &TovenOutput, repo: &Path) -> String {
    format!(
        "status: {}\nstdout:\n{}stderr:\n{}",
        output.status_code,
        normalize_stream(&output.stdout, repo),
        normalize_stream(&output.stderr, repo)
    )
}

fn normalize_stream(stream: &str, repo: &Path) -> String {
    let repo_path = repo.display().to_string();
    let canonical_repo_path = std::fs::canonicalize(repo)
        .ok()
        .map(|path| path.display().to_string());
    let mut normalized = stream.replace(&repo_path, "<repo>");
    if let Some(canonical_repo_path) = canonical_repo_path
        && canonical_repo_path != repo_path
    {
        normalized = normalized.replace(&canonical_repo_path, "<repo>");
    }

    let normalized = normalized
        .lines()
        .map(normalize_line)
        .collect::<Vec<_>>()
        .join("\n");

    if normalized.is_empty() {
        "<empty>\n".to_owned()
    } else {
        format!("{normalized}\n")
    }
}

fn normalize_line(line: &str) -> String {
    if line.starts_with("baseline: ") {
        return "baseline: <sha>".to_owned();
    }
    if line.starts_with("done: ")
        && line.ends_with("s)")
        && let Some(duration_start) = line.rfind("] (")
    {
        return format!("{}] (<duration>s)", &line[..duration_start]);
    }
    line.to_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn normalizes_only_toven_done_line_duration() {
        assert_eq!(
            super::normalize_line("done: rust/test/w0/api [ok] (1.23s)"),
            "done: rust/test/w0/api [ok] (<duration>s)"
        );
        assert_eq!(
            super::normalize_line("done: child output"),
            "done: child output"
        );
    }
}
