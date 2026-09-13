use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::{error::ToolError, platform::path::resolve_existing};

const DEFAULT_MAX_BYTES: usize = 48 * 1024;
const ABSOLUTE_MAX_BYTES: usize = 256 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RepoReadRequest {
    /// Absolute repository root used as the path-security boundary.
    pub repo_root: String,
    /// Repository-relative file path. Absolute paths are allowed only below `repo_root`.
    pub path: String,
    /// First 1-based line to return. Defaults to 1.
    #[serde(default = "default_start_line")]
    pub start_line: usize,
    /// Maximum number of lines to return. Defaults to 400.
    #[serde(default = "default_max_lines")]
    pub max_lines: usize,
    /// Maximum UTF-8 response bytes. Defaults to 49152; hard limit is 262144.
    #[serde(default = "default_max_bytes")]
    pub max_bytes: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RepoReadResult {
    pub path: String,
    pub start_line: usize,
    pub end_line: Option<usize>,
    pub content: String,
    pub truncated: bool,
}

pub(crate) async fn execute(request: &RepoReadRequest) -> Result<RepoReadResult, ToolError> {
    validate(request)?;
    let (root, file) = resolve_existing(Path::new(&request.repo_root), Path::new(&request.path))?;
    if !file.is_file() {
        return Err(ToolError::InvalidInput("path must name a file".to_owned()));
    }

    let handle = tokio::fs::File::open(&file).await?;
    let mut lines = BufReader::new(handle).lines();
    let mut current = 0_usize;
    let mut content = String::new();
    let mut truncated = false;

    while let Some(line) = lines.next_line().await? {
        current += 1;
        if current < request.start_line {
            continue;
        }
        if current >= request.start_line.saturating_add(request.max_lines) {
            truncated = true;
            break;
        }

        let addition = line.len().saturating_add(1);
        if content.len().saturating_add(addition) > request.max_bytes {
            truncated = true;
            break;
        }
        content.push_str(&line);
        content.push('\n');
    }

    let returned_lines = content.lines().count();
    let end_line =
        (returned_lines > 0).then(|| request.start_line.saturating_add(returned_lines - 1));
    let relative = file
        .strip_prefix(root)
        .map_err(|_| ToolError::PathOutsideRoot)?
        .to_string_lossy()
        .replace('\\', "/");

    Ok(RepoReadResult {
        path: relative,
        start_line: request.start_line,
        end_line,
        content,
        truncated,
    })
}

fn validate(request: &RepoReadRequest) -> Result<(), ToolError> {
    if request.start_line == 0 {
        return Err(ToolError::InvalidInput(
            "start_line must be at least 1".to_owned(),
        ));
    }
    if request.max_lines == 0 {
        return Err(ToolError::InvalidInput(
            "max_lines must be at least 1".to_owned(),
        ));
    }
    if request.max_bytes == 0 || request.max_bytes > ABSOLUTE_MAX_BYTES {
        return Err(ToolError::LimitExceeded(format!(
            "max_bytes must be between 1 and {ABSOLUTE_MAX_BYTES}"
        )));
    }
    Ok(())
}

const fn default_start_line() -> usize {
    1
}

const fn default_max_lines() -> usize {
    400
}

const fn default_max_bytes() -> usize {
    DEFAULT_MAX_BYTES
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{RepoReadRequest, execute};

    #[tokio::test]
    async fn reads_a_bounded_line_range() {
        let root = tempdir().expect("test directory");
        fs::write(root.path().join("sample.txt"), "one\ntwo\nthree\n").expect("fixture write");
        let result = execute(&RepoReadRequest {
            repo_root: root.path().display().to_string(),
            path: "sample.txt".to_owned(),
            start_line: 2,
            max_lines: 1,
            max_bytes: 100,
        })
        .await
        .expect("read result");
        assert_eq!(result.content, "two\n");
        assert_eq!(result.end_line, Some(2));
        assert!(result.truncated);
    }
}
