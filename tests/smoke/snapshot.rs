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
    line.to_owned()
}
