use std::{fs::File, io::BufRead, io::BufReader, path::Path};

use globset::{Glob, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ToolError;

const MAX_MATCHES: usize = 1_000;
const MAX_PREVIEW_CHARS: usize = 500;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RepoSearchRequest {
    /// Absolute path and security boundary.
    pub repo_root: String,
    #[serde(default)]
    pub mode: SearchMode,
    /// Required in content mode.
    pub query: Option<String>,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub ignore_case: bool,
    /// Globset glob supporting brace alternatives, for example `{src,tests}/**/*.rs`.
    #[serde(alias = "path_glob")]
    pub include: Option<String>,
    #[serde(default)]
    pub include_hidden: bool,
    /// Also search files excluded by .gitignore, .ignore, and git exclude files.
    #[serde(default)]
    pub include_ignored: bool,
    /// Return one compact text row per matching line. Defaults to false.
    #[serde(default)]
    pub compact: bool,
    /// Hard limit: 1000.
    #[serde(default = "default_max_matches")]
    pub max_matches: usize,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Content,
    Files,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SearchMatch {
    pub path: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub preview: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RepoSearchResult {
    pub matches: Vec<SearchMatch>,
    pub files_scanned: usize,
    pub truncated: bool,
}

pub(crate) fn execute(request: &RepoSearchRequest) -> Result<RepoSearchResult, ToolError> {
    validate(request)?;
    let root = Path::new(&request.repo_root)
        .canonicalize()
        .map_err(ToolError::Io)?;
    if !root.is_dir() {
        return Err(ToolError::InvalidInput(
            "repo_root must be a directory".to_owned(),
        ));
    }
    let pattern_matcher = build_matcher(request)?;
    let glob = build_glob(request.include.as_deref())?;
    let mut results = Vec::new();
    let mut files_scanned = 0;
    let mut truncated = false;

    for entry in WalkBuilder::new(&root)
        .hidden(!request.include_hidden)
        .git_ignore(!request.include_ignored)
        .git_exclude(!request.include_ignored)
        .ignore(!request.include_ignored)
        .require_git(false)
        .sort_by_file_path(std::path::Path::cmp)
        .build()
    {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(&root)
            .map_err(|_| ToolError::PathOutsideRoot)?;
        if glob.as_ref().is_some_and(|set| !set.is_match(relative)) {
            continue;
        }
        files_scanned += 1;
        if matches!(request.mode, SearchMode::Files) {
            if results.len() == request.max_matches {
                truncated = true;
                break;
            }
            results.push(SearchMatch {
                path: relative.to_string_lossy().replace('\\', "/"),
                line: None,
                column: None,
                preview: None,
            });
            continue;
        }
        let Ok(file) = File::open(entry.path()) else {
            continue;
        };
        for (index, line) in BufReader::new(file).lines().enumerate() {
            let Ok(line) = line else { break };
            for found in pattern_matcher
                .as_ref()
                .into_iter()
                .flat_map(|matcher| matcher.find_iter(&line))
            {
                if request.compact
                    && results.last().is_some_and(|previous: &SearchMatch| {
                        previous.path == relative.to_string_lossy().replace('\\', "/")
                            && previous.line == Some(index + 1)
                    })
                {
                    continue;
                }
                if results.len() == request.max_matches {
                    truncated = true;
                    break;
                }
                results.push(SearchMatch {
                    path: relative.to_string_lossy().replace('\\', "/"),
                    line: Some(index + 1),
                    column: Some(line[..found.start()].chars().count() + 1),
                    preview: Some(line.chars().take(MAX_PREVIEW_CHARS).collect()),
                });
            }
            if truncated {
                break;
            }
        }
        if truncated {
            break;
        }
    }
    results.sort_by(|left, right| {
        (&left.path, left.line, left.column).cmp(&(&right.path, right.line, right.column))
    });
    Ok(RepoSearchResult {
        matches: results,
        files_scanned,
        truncated,
    })
}

fn validate(request: &RepoSearchRequest) -> Result<(), ToolError> {
    if matches!(request.mode, SearchMode::Content)
        && request.query.as_ref().is_none_or(String::is_empty)
    {
        return Err(ToolError::InvalidInput(
            "query must not be empty".to_owned(),
        ));
    }
    if request.max_matches == 0 || request.max_matches > MAX_MATCHES {
        return Err(ToolError::LimitExceeded(format!(
            "max_matches must be between 1 and {MAX_MATCHES}"
        )));
    }
    Ok(())
}

fn build_matcher(request: &RepoSearchRequest) -> Result<Option<Regex>, ToolError> {
    let Some(query) = request.query.as_ref() else {
        return Ok(None);
    };
    let pattern = if request.regex {
        query.clone()
    } else {
        regex::escape(query)
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(request.ignore_case)
        .build()
        .map(Some)
        .map_err(|error| ToolError::InvalidInput(format!("invalid regex: {error}")))
}

fn build_glob(pattern: Option<&str>) -> Result<Option<globset::GlobSet>, ToolError> {
    let Some(pattern) = pattern else {
        return Ok(None);
    };
    let mut builder = GlobSetBuilder::new();
    builder.add(
        Glob::new(pattern)
            .map_err(|error| ToolError::InvalidInput(format!("invalid include glob: {error}")))?,
    );
    builder
        .build()
        .map(Some)
        .map_err(|error| ToolError::InvalidInput(error.to_string()))
}

const fn default_max_matches() -> usize {
    100
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{RepoSearchRequest, execute};

    #[test]
    fn honors_literal_search_and_result_limit() {
        let root = tempdir().expect("test directory");
        fs::write(root.path().join("sample.rs"), "needle\nnone\nneedle\n").expect("fixture");
        let result = execute(&RepoSearchRequest {
            repo_root: root.path().display().to_string(),
            mode: super::SearchMode::Content,
            query: Some("needle".to_owned()),
            regex: false,
            ignore_case: false,
            include: Some("**/*.rs".to_owned()),
            include_hidden: false,
            include_ignored: false,
            compact: false,
            max_matches: 1,
        })
        .expect("search");
        assert_eq!(result.matches.len(), 1);
        assert!(result.truncated);
    }

    #[test]
    fn searches_ignored_files_only_when_asked() {
        let root = tempdir().expect("test directory");
        fs::write(root.path().join(".gitignore"), "todo/\n").expect("gitignore fixture");
        fs::create_dir_all(root.path().join("todo")).expect("todo directory");
        fs::write(root.path().join("todo/plan.md"), "needle\n").expect("todo fixture");

        let base_request = |include_ignored: bool| RepoSearchRequest {
            repo_root: root.path().display().to_string(),
            mode: super::SearchMode::Content,
            query: Some("needle".to_owned()),
            regex: false,
            ignore_case: false,
            include: None,
            include_hidden: false,
            include_ignored,
            compact: false,
            max_matches: 10,
        };

        let result = execute(&base_request(false)).expect("search without ignored");
        assert_eq!(result.matches.len(), 0);

        let result = execute(&base_request(true)).expect("search with ignored");
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].path, "todo/plan.md");
    }

    #[test]
    fn aliases_globs_compact_rows_and_unknown_fields() {
        let root = tempdir().expect("test directory");
        for dir in ["src", "tests", "docs"] {
            fs::create_dir(root.path().join(dir)).expect("directory");
            fs::write(root.path().join(dir).join("sample.rs"), "needle needle\n").expect("fixture");
        }
        let args = serde_json::json!({"repo_root": root.path().display().to_string(), "query": "needle", "path_glob": "{src,tests}/**/*.rs", "compact": true});
        let request: RepoSearchRequest = serde_json::from_value(args.clone()).expect("request");
        let result = execute(&request).expect("search");
        assert_eq!(result.matches.len(), 2);
        assert_eq!(result.matches[0].path, "src/sample.rs");
        assert_eq!(result.matches[1].path, "tests/sample.rs");
        let mut legacy = args.clone();
        legacy["compact"] = serde_json::json!(false);
        let request: RepoSearchRequest = serde_json::from_value(legacy).expect("request");
        assert_eq!(execute(&request).expect("legacy search").matches.len(), 4);
        let mut unknown = args;
        unknown["path_globb"] = serde_json::json!("docs/**");
        assert!(serde_json::from_value::<RepoSearchRequest>(unknown).is_err());
    }
}
