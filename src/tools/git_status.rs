use std::{path::Path, process::Command};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{error::ToolError, tools::diff_inspect::canonical_git_root};

const MAX_LOG: usize = 50;
const MAX_CHANGES: usize = 1000;
const MAX_SUBJECT_CHARS: usize = 200;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitStatusRequest {
    /// Absolute Git repository or worktree root.
    pub repo_root: String,
    /// Recent commits to list. Defaults to 5; hard limit is 50; 0 skips the log.
    #[serde(default = "default_log")]
    pub log: usize,
    /// Maximum changed paths to list. Defaults to 200; hard limit is 1000.
    #[serde(default = "default_max_changes")]
    pub max_changes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct GitChange {
    /// Porcelain XY code: index then worktree; `??` is untracked.
    pub status: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct GitCommit {
    pub sha: String,
    pub subject: String,
}

#[derive(Debug, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct GitStatusResult {
    /// Current branch; absent when HEAD is detached.
    pub branch: Option<String>,
    /// Abbreviated HEAD commit; absent before the first commit.
    pub head: Option<String>,
    pub upstream: Option<String>,
    /// Commits on HEAD that the upstream lacks.
    pub ahead: u64,
    /// Commits on the upstream that HEAD lacks.
    pub behind: u64,
    /// Total changed paths before truncation.
    pub change_count: usize,
    /// Changed paths sorted by path.
    pub changes: Vec<GitChange>,
    /// Most recent commits first.
    pub commits: Vec<GitCommit>,
    pub truncated: bool,
}

pub(crate) fn execute(request: &GitStatusRequest) -> Result<GitStatusResult, ToolError> {
    if request.log > MAX_LOG {
        return Err(ToolError::LimitExceeded(format!(
            "log must be at most {MAX_LOG}"
        )));
    }
    if request.max_changes == 0 || request.max_changes > MAX_CHANGES {
        return Err(ToolError::LimitExceeded(format!(
            "max_changes must be between 1 and {MAX_CHANGES}"
        )));
    }
    let root = canonical_git_root(&request.repo_root)?;
    let status = run_git(&root, &["status", "--porcelain=v2", "--branch", "-z"])?;
    let mut result = parse_status(&status);
    result.change_count = result.changes.len();
    result
        .changes
        .sort_by(|left, right| left.path.cmp(&right.path));
    result.truncated = result.changes.len() > request.max_changes;
    result.changes.truncate(request.max_changes);
    if request.log > 0 && result.head.is_some() {
        let count = format!("-n{}", request.log);
        let log = run_git(&root, &["log", &count, "--format=%h%x09%s"])?;
        result.commits = parse_log(&log);
    }
    Ok(result)
}

// Optional locks off: `git status` would otherwise refresh and rewrite the index.
fn run_git(root: &Path, args: &[&str]) -> Result<String, ToolError> {
    let output = Command::new("git")
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err(ToolError::InvalidInput(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_status(text: &str) -> GitStatusResult {
    let mut result = GitStatusResult::default();
    let mut fields = text.split('\0');
    while let Some(entry) = fields.next() {
        if let Some(oid) = entry.strip_prefix("# branch.oid ") {
            if oid != "(initial)" {
                result.head = Some(oid.chars().take(12).collect());
            }
        } else if let Some(head) = entry.strip_prefix("# branch.head ") {
            if head != "(detached)" {
                result.branch = Some(head.to_owned());
            }
        } else if let Some(upstream) = entry.strip_prefix("# branch.upstream ") {
            result.upstream = Some(upstream.to_owned());
        } else if let Some(counts) = entry.strip_prefix("# branch.ab ") {
            for count in counts.split(' ') {
                if let Some(ahead) = count.strip_prefix('+') {
                    result.ahead = ahead.parse().unwrap_or(0);
                } else if let Some(behind) = count.strip_prefix('-') {
                    result.behind = behind.parse().unwrap_or(0);
                }
            }
        } else if let Some(change) = parse_entry(entry) {
            // A rename entry is followed by its original path as a separate field.
            if entry.starts_with("2 ") {
                fields.next();
            }
            result.changes.push(change);
        }
    }
    result
}

/// Parses one porcelain v2 change entry; `fixed` counts the fields between XY and the path.
fn parse_entry(entry: &str) -> Option<GitChange> {
    let (kind, rest) = entry.split_once(' ')?;
    let fixed = match kind {
        "1" => 7,
        "2" => 8,
        "u" => 9,
        "?" => {
            return Some(GitChange {
                status: "??".to_owned(),
                path: rest.to_owned(),
            });
        }
        _ => return None,
    };
    let mut parts = rest.splitn(fixed + 1, ' ');
    let status = parts.next()?.to_owned();
    let path = parts.nth(fixed - 1)?.to_owned();
    Some(GitChange { status, path })
}

fn parse_log(text: &str) -> Vec<GitCommit> {
    text.lines()
        .filter_map(|line| {
            let (sha, subject) = line.split_once('\t')?;
            Some(GitCommit {
                sha: sha.to_owned(),
                subject: subject.chars().take(MAX_SUBJECT_CHARS).collect(),
            })
        })
        .collect()
}

const fn default_log() -> usize {
    5
}

const fn default_max_changes() -> usize {
    200
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::{fs, process::Command};

    use super::{GitChange, GitStatusRequest, execute, parse_status};

    #[test]
    fn parses_branch_headers_and_every_entry_kind() {
        let text = "# branch.oid 0123456789abcdef\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\x001 .M N... 100644 100644 100644 aa bb src/a b.rs\x002 R. N... 100644 100644 100644 aa bb R100 new.rs\0old.rs\0u UU N... 100644 100644 100644 100644 aa bb cc both.rs\0? notes.txt\0";
        let result = parse_status(text);
        assert_eq!(result.branch.as_deref(), Some("main"));
        assert_eq!(result.head.as_deref(), Some("0123456789ab"));
        assert_eq!(result.upstream.as_deref(), Some("origin/main"));
        assert_eq!((result.ahead, result.behind), (2, 1));
        let entries: Vec<_> = result
            .changes
            .iter()
            .map(|change| (change.status.as_str(), change.path.as_str()))
            .collect();
        assert_eq!(
            entries,
            [
                (".M", "src/a b.rs"),
                ("R.", "new.rs"),
                ("UU", "both.rs"),
                ("??", "notes.txt"),
            ]
        );
    }

    #[test]
    fn reports_real_repository_state() {
        let root = tempfile::tempdir().expect("test directory");
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(root.path())
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        fs::write(root.path().join("a.txt"), "a\n").expect("fixture");
        git(&["add", "."]);
        git(&["commit", "-qm", "first"]);
        fs::write(root.path().join("a.txt"), "b\n").expect("fixture");
        fs::write(root.path().join("new.txt"), "n\n").expect("fixture");
        let request = |max_changes: usize| GitStatusRequest {
            repo_root: root.path().display().to_string(),
            log: 5,
            max_changes,
        };
        let result = execute(&request(10)).expect("status");
        assert_eq!(result.branch.as_deref(), Some("main"));
        assert!(result.head.is_some());
        assert_eq!(result.upstream, None);
        assert_eq!(
            result.changes,
            [
                GitChange {
                    status: ".M".to_owned(),
                    path: "a.txt".to_owned(),
                },
                GitChange {
                    status: "??".to_owned(),
                    path: "new.txt".to_owned(),
                },
            ]
        );
        assert_eq!(result.commits.len(), 1);
        assert_eq!(result.commits[0].subject, "first");
        let bounded = execute(&request(1)).expect("status");
        assert_eq!(bounded.change_count, 2);
        assert_eq!(bounded.changes.len(), 1);
        assert!(bounded.truncated);
    }

    #[test]
    fn rejects_out_of_range_limits() {
        let request = |log: usize, max_changes: usize| GitStatusRequest {
            repo_root: ".".to_owned(),
            log,
            max_changes,
        };
        assert!(execute(&request(51, 10)).is_err());
        assert!(execute(&request(5, 0)).is_err());
    }
}
