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
    let normalized = stream
        .replace(&repo.display().to_string(), "<repo>")
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
