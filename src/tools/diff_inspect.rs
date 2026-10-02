use std::{
    path::{Component, Path},
    process::Command,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ToolError;

const MAX_DIFF_BYTES: usize = 256 * 1024;
const MAX_PATHS: usize = 100;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DiffInspectRequest {
    /// Absolute Git repository root.
    pub repo_root: String,
    /// Include staged changes instead of working-tree changes.
    #[serde(default)]
    pub staged: bool,
    /// Optional Git revision or range, for example HEAD~1 or main...HEAD.
    pub revision: Option<String>,
    /// Include a bounded patch in the result. Defaults to false.
    #[serde(default)]
    pub include_patch: bool,
    /// Maximum ranked hotspots. Defaults to 10; hard limit is 100.
    #[serde(default = "default_max_files")]
    pub max_files: usize,
    /// Repository-relative paths that limit the diff; matched literally. Hard limit is 100.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Ignore whitespace changes, like `git diff -w`.
    #[serde(default)]
    pub ignore_whitespace: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DiffFile {
    pub path: String,
    pub added: u64,
    pub deleted: u64,
    pub binary: bool,
    pub risk_score: f64,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DiffInspectResult {
    pub files: Vec<DiffFile>,
    pub hotspots: Vec<DiffFile>,
    pub risk_level: RiskLevel,
    pub warnings: Vec<String>,
    pub patch: Option<String>,
    pub truncated: bool,
}

pub(crate) fn execute(request: &DiffInspectRequest) -> Result<DiffInspectResult, ToolError> {
    if request
        .revision
        .as_ref()
        .is_some_and(|value| value.starts_with('-'))
    {
        return Err(ToolError::InvalidInput(
            "revision must not start with '-'".to_owned(),
        ));
    }
    if request.max_files == 0 || request.max_files > 100 {
        return Err(ToolError::LimitExceeded(
            "max_files must be between 1 and 100".to_owned(),
        ));
    }
    validate_paths(&request.paths)?;
    let root = canonical_git_root(&request.repo_root)?;
    let numstat = run_git(&root, request, "--numstat")?;
    let mut files = parse_numstat(&numstat);
    for file in &mut files {
        apply_risk(file);
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let mut hotspots = files.clone();
    hotspots.sort_by(|left, right| {
        right
            .risk_score
            .total_cmp(&left.risk_score)
            .then_with(|| left.path.cmp(&right.path))
    });
    hotspots.truncate(request.max_files);
    let top_score = hotspots.first().map_or(0.0, |file| file.risk_score);
    let risk_level = if top_score > 40.0 {
        RiskLevel::High
    } else if top_score > 20.0 {
        RiskLevel::Medium
    } else {
        RiskLevel::Low
    };
    let changed_source = files.iter().any(|file| {
        file.path.starts_with("src/")
            || file.path.starts_with("app/")
            || file.path.starts_with("lib/")
    });
    let changed_tests = files.iter().any(|file| {
        let lower = file.path.to_ascii_lowercase();
        lower.contains("test") || lower.contains("spec")
    });
    let warnings = if changed_source && !changed_tests {
        vec!["source files changed without obvious test changes".to_owned()]
    } else {
        Vec::new()
    };
    let (patch, truncated) = if request.include_patch {
        let bytes = run_git_bytes(&root, request, "--patch")?;
        let truncated = bytes.len() > MAX_DIFF_BYTES;
        let bounded = &bytes[..bytes.len().min(MAX_DIFF_BYTES)];
        (
            Some(String::from_utf8_lossy(bounded).into_owned()),
            truncated,
        )
    } else {
        (None, false)
    };
    Ok(DiffInspectResult {
        files,
        hotspots,
        risk_level,
        warnings,
        patch,
        truncated,
    })
}

pub(crate) fn canonical_git_root(value: &str) -> Result<std::path::PathBuf, ToolError> {
    let root = Path::new(value).canonicalize().map_err(ToolError::Io)?;
    if !root.join(".git").exists() {
        return Err(ToolError::InvalidInput(
            "repo_root is not a Git worktree root".to_owned(),
        ));
    }
    Ok(root)
}

fn validate_paths(paths: &[String]) -> Result<(), ToolError> {
    if paths.len() > MAX_PATHS {
        return Err(ToolError::LimitExceeded(format!(
            "paths must list at most {MAX_PATHS} entries"
        )));
    }
    let invalid = paths.iter().find(|path| {
        path.is_empty()
            || path.starts_with('-')
            || path.starts_with(':')
            || Path::new(path).is_absolute()
            || Path::new(path)
                .components()
                .any(|component| matches!(component, Component::ParentDir))
    });
    match invalid {
        Some(path) => Err(ToolError::InvalidInput(format!("invalid path: {path}"))),
        None => Ok(()),
    }
}

fn git_args(request: &DiffInspectRequest, format: &str) -> Vec<String> {
    let mut args = vec![
        "diff".to_owned(),
        format.to_owned(),
        "--no-ext-diff".to_owned(),
    ];
    if request.ignore_whitespace {
        args.push("-w".to_owned());
    }
    if request.staged {
        args.push("--cached".to_owned());
    }
    if let Some(revision) = &request.revision {
        args.push(revision.clone());
    }
    args.push("--".to_owned());
    args.extend(request.paths.iter().cloned());
    args
}

fn run_git(root: &Path, request: &DiffInspectRequest, format: &str) -> Result<String, ToolError> {
    Ok(String::from_utf8_lossy(&run_git_bytes(root, request, format)?).into_owned())
}

fn run_git_bytes(
    root: &Path,
    request: &DiffInspectRequest,
    format: &str,
) -> Result<Vec<u8>, ToolError> {
    let output = Command::new("git")
        .args(git_args(request, format))
        .env("GIT_LITERAL_PATHSPECS", "1")
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err(ToolError::InvalidInput(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

fn parse_numstat(text: &str) -> Vec<DiffFile> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let added = fields.next()?;
            let deleted = fields.next()?;
            let path = fields.next()?.to_owned();
            let binary = added == "-" || deleted == "-";
            Some(DiffFile {
                path,
                added: added.parse().unwrap_or(0),
                deleted: deleted.parse().unwrap_or(0),
                binary,
                risk_score: 0.0,
                reasons: Vec::new(),
            })
        })
        .collect()
}

fn apply_risk(file: &mut DiffFile) {
    const RISK_TERMS: &[&str] = &[
        "auth",
        "security",
        "permission",
        "payment",
        "billing",
        "invoice",
        "migration",
        "schema",
        "database",
        "db",
        "api",
        "token",
        "secret",
        "crypto",
        "login",
        "session",
    ];
    const GENERATED: &[&str] = &[
        "lock",
        "generated",
        "dist/",
        "build/",
        ".min.",
        ".map",
        "snapshot",
    ];
    let lower = file.path.to_ascii_lowercase();
    let volume = u32::try_from(file.added.saturating_add(file.deleted).min(300)).unwrap_or(300);
    file.risk_score = f64::from(volume) / 10.0;
    for term in RISK_TERMS {
        if lower.contains(term) {
            file.risk_score += 18.0;
            file.reasons.push(format!("path contains {term}"));
        }
    }
    if lower.ends_with("package.json") || lower.contains("config") || lower.contains(".github/") {
        file.risk_score += 12.0;
        file.reasons
            .push("configuration or dependency surface".to_owned());
    }
    if GENERATED.iter().any(|term| lower.contains(term)) {
        file.risk_score -= 25.0;
        file.reasons
            .push("likely generated or low-signal file".to_owned());
    }
    if file.added.saturating_add(file.deleted) > 200 {
        file.reasons.push("large change volume".to_owned());
    }
}

const fn default_max_files() -> usize {
    10
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::{fs, process::Command};

    use super::{DiffInspectRequest, DiffInspectResult, execute, parse_numstat, validate_paths};

    #[test]
    fn rejects_paths_that_escape_or_look_like_options() {
        for path in ["../x", "/etc/passwd", "-p", ":(glob)*", ""] {
            assert!(validate_paths(&[path.to_owned()]).is_err(), "{path}");
        }
        assert!(validate_paths(&["src/main.rs".to_owned()]).is_ok());
    }

    #[test]
    fn limits_diff_to_paths_and_ignores_whitespace() {
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
        git(&["init", "-q"]);
        fs::write(root.path().join("a.txt"), "a\n").expect("fixture");
        fs::write(root.path().join("b.txt"), "b\n").expect("fixture");
        git(&["add", "."]);
        git(&["commit", "-qm", "initial"]);
        fs::write(root.path().join("a.txt"), format!("a{}\n", " ".repeat(2))).expect("fixture");
        fs::write(root.path().join("b.txt"), "c\n").expect("fixture");
        let request = |paths: Vec<String>, ignore_whitespace: bool| DiffInspectRequest {
            repo_root: root.path().display().to_string(),
            staged: false,
            revision: None,
            include_patch: false,
            max_files: 10,
            paths,
            ignore_whitespace,
        };
        let changed = |result: DiffInspectResult| -> Vec<String> {
            result
                .files
                .into_iter()
                .filter(|file| file.added + file.deleted > 0)
                .map(|file| file.path)
                .collect()
        };
        let all = execute(&request(Vec::new(), false)).expect("diff");
        assert_eq!(changed(all), ["a.txt", "b.txt"]);
        let only_b = execute(&request(vec!["b.txt".to_owned()], false)).expect("diff");
        assert_eq!(changed(only_b), ["b.txt"]);
        let no_whitespace = execute(&request(Vec::new(), true)).expect("diff");
        assert_eq!(changed(no_whitespace), ["b.txt"]);
    }

    #[test]
    fn parses_text_and_binary_numstat() {
        let files = parse_numstat("3\t2\tsrc/main.rs\n-\t-\timage.png\n");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].added, 3);
        assert!(files[1].binary);
    }
}
