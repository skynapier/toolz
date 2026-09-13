use std::{collections::BTreeMap, path::Path, process::Command};

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ToolError;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanTarget {
    #[default]
    Staged,
    Unstaged,
    Head,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SecretScanRequest {
    /// Absolute Git repository root.
    pub repo_root: String,
    /// Diff to scan: staged (default), unstaged, or all changes from HEAD.
    #[serde(default)]
    pub target: ScanTarget,
    /// Report findings without setting blocked=true.
    #[serde(default)]
    pub allow_secrets: bool,
    /// Detect high-entropy quoted or assigned strings. Defaults to true.
    #[serde(default = "default_true")]
    pub include_entropy: bool,
    /// Shannon entropy threshold. Defaults to 3.5.
    #[serde(default = "default_entropy")]
    pub min_entropy: f64,
    /// Maximum findings. Defaults to 100; hard limit is 500.
    #[serde(default = "default_max_findings")]
    pub max_findings: usize,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SecretFinding {
    pub path: String,
    pub line: Option<usize>,
    pub rule: String,
    pub severity: Severity,
    pub redacted_preview: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SecretScanResult {
    pub blocked: bool,
    pub findings: Vec<SecretFinding>,
    pub scanned_lines: usize,
    pub truncated: bool,
}

pub(crate) fn execute(request: &SecretScanRequest) -> Result<SecretScanResult, ToolError> {
    validate(request)?;
    let root = Path::new(&request.repo_root)
        .canonicalize()
        .map_err(ToolError::Io)?;
    if !root.join(".git").exists() {
        return Err(ToolError::InvalidInput(
            "repo_root is not a Git worktree root".to_owned(),
        ));
    }
    let mut args = vec!["diff", "--unified=0", "--no-ext-diff"];
    match request.target {
        ScanTarget::Staged => args.push("--cached"),
        ScanTarget::Unstaged => {}
        ScanTarget::Head => args.push("HEAD"),
    }
    args.push("--");
    let output = Command::new("git").args(args).current_dir(root).output()?;
    if !output.status.success() {
        return Err(ToolError::InvalidInput(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    scan_patch(&String::from_utf8_lossy(&output.stdout), request)
}

fn validate(request: &SecretScanRequest) -> Result<(), ToolError> {
    if request.max_findings == 0 || request.max_findings > 500 {
        return Err(ToolError::LimitExceeded(
            "max_findings must be between 1 and 500".to_owned(),
        ));
    }
    if !(0.0..=8.0).contains(&request.min_entropy) {
        return Err(ToolError::InvalidInput(
            "min_entropy must be between 0 and 8".to_owned(),
        ));
    }
    Ok(())
}

fn scan_patch(diff: &str, request: &SecretScanRequest) -> Result<SecretScanResult, ToolError> {
    let rules = rules()?;
    let entropy =
        Regex::new(r#"["'`]([A-Za-z0-9_/+=-]{20,})["'`]|[:=]\s*([A-Za-z0-9_/+=-]{20,})\b"#)
            .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
    let mut current_path = String::new();
    let mut new_line = None;
    let mut findings = Vec::new();
    let mut scanned_lines = 0;
    let mut truncated = false;
    for line in diff.lines() {
        if let Some(value) = line.strip_prefix("+++ b/") {
            value.clone_into(&mut current_path);
            continue;
        }
        if let Some(header) = line.strip_prefix("@@ ") {
            new_line = header
                .split_whitespace()
                .nth(1)
                .and_then(|range| range.strip_prefix('+'))
                .and_then(|value| value.split(',').next())
                .and_then(|number| number.parse().ok());
            continue;
        }
        if line.starts_with('+') && !line.starts_with("+++") {
            scanned_lines += 1;
            let value = &line[1..];
            let mut finding = match_rule(value, &rules, &current_path, new_line);
            if finding.is_none() && request.include_entropy {
                finding = entropy
                    .captures_iter(value)
                    .filter_map(|captures| captures.get(1).or_else(|| captures.get(2)))
                    .find(|token| shannon_entropy(token.as_str()) >= request.min_entropy)
                    .map(|token| SecretFinding {
                        path: current_path.clone(),
                        line: new_line,
                        rule: "high_entropy_string".to_owned(),
                        severity: Severity::Medium,
                        redacted_preview: replace_redacted(value, token.start(), token.end()),
                    });
            }
            if let Some(finding) = finding {
                if findings.len() == request.max_findings {
                    truncated = true;
                    break;
                }
                findings.push(finding);
            }
            new_line = new_line.map(|value| value.saturating_add(1));
        } else if line.starts_with(' ') {
            new_line = new_line.map(|value| value.saturating_add(1));
        }
    }
    Ok(SecretScanResult {
        blocked: !request.allow_secrets && !findings.is_empty(),
        findings,
        scanned_lines,
        truncated,
    })
}

fn rules() -> Result<BTreeMap<&'static str, (Severity, Regex)>, ToolError> {
    let definitions = [
        (
            "private_key",
            Severity::Critical,
            r"-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----",
        ),
        (
            "aws_access_key_id",
            Severity::Critical,
            r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
        ),
        (
            "github_token",
            Severity::Critical,
            r"\bgh[pousr]_[A-Za-z0-9_]{30,}\b",
        ),
        (
            "gitlab_pat",
            Severity::Critical,
            r"\bglpat-[A-Za-z0-9_-]{20,}\b",
        ),
        (
            "stripe_key",
            Severity::Critical,
            r"\b(?:sk|pk)_(?:live|test)_[A-Za-z0-9]{24,}\b",
        ),
        (
            "jwt",
            Severity::High,
            r"\beyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b",
        ),
        (
            "slack_token",
            Severity::High,
            r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b",
        ),
        (
            "generic_secret_assign",
            Severity::High,
            r#"(?i)(?:api[_-]?key|api[_-]?secret|access[_-]?token|auth[_-]?token|client[_-]?secret|secret[_-]?key)\s*[:=]\s*["'][A-Za-z0-9_/+=-]{20,}["']"#,
        ),
        (
            "hardcoded_password",
            Severity::Medium,
            r#"(?i)(?:password|passwd|pwd)\s*[:=]\s*["'][^"']{8,}["']"#,
        ),
    ];
    definitions
        .into_iter()
        .map(|(name, severity, pattern)| {
            Regex::new(pattern)
                .map(|regex| (name, (severity, regex)))
                .map_err(|error| ToolError::InvalidInput(error.to_string()))
        })
        .collect()
}

fn match_rule(
    value: &str,
    rules: &BTreeMap<&str, (Severity, Regex)>,
    path: &str,
    line: Option<usize>,
) -> Option<SecretFinding> {
    rules.iter().find_map(|(rule, (severity, regex))| {
        regex.find(value).map(|found| SecretFinding {
            path: path.to_owned(),
            line,
            rule: (*rule).to_owned(),
            severity: *severity,
            redacted_preview: replace_redacted(value, found.start(), found.end()),
        })
    })
}

fn replace_redacted(value: &str, start: usize, end: usize) -> String {
    let matched = &value[start..end];
    let first: String = matched.chars().take(4).collect();
    let last: String = matched
        .chars()
        .rev()
        .take(2)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{}{}***{}{}", &value[..start], first, last, &value[end..])
        .chars()
        .take(160)
        .collect()
}

fn shannon_entropy(value: &str) -> f64 {
    let mut frequencies = BTreeMap::new();
    for byte in value.bytes() {
        *frequencies.entry(byte).or_insert(0_u32) += 1;
    }
    let length = f64::from(u32::try_from(value.len()).unwrap_or(u32::MAX));
    frequencies
        .values()
        .map(|count| {
            let probability = f64::from(*count) / length;
            -probability * probability.log2()
        })
        .sum()
}

const fn default_true() -> bool {
    true
}
const fn default_entropy() -> f64 {
    3.5
}
const fn default_max_findings() -> usize {
    100
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{ScanTarget, SecretScanRequest, scan_patch};

    #[test]
    fn reports_secret_without_returning_complete_value() {
        let secret = ["ghp", "_abcdefghijklmnopqrstuvwxyz1234567890"].concat();
        let patch = format!("+++ b/config.txt\n@@ -0,0 +1 @@\n+token={secret}\n");
        let request = SecretScanRequest {
            repo_root: String::new(),
            target: ScanTarget::Staged,
            allow_secrets: false,
            include_entropy: true,
            min_entropy: 3.5,
            max_findings: 10,
        };
        let result = scan_patch(&patch, &request).expect("scan");
        assert_eq!(result.findings.len(), 1);
        assert!(result.blocked);
        assert!(!result.findings[0].redacted_preview.contains(&secret));
    }
}
