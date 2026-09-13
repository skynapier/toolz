use std::collections::BTreeSet;

use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ToolError;

const ABSOLUTE_MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FailureExtractRequest {
    /// Build, test, compiler, or runtime output to inspect.
    pub text: String,
    /// Maximum failures to return. Defaults to 50; hard limit is 500.
    #[serde(default = "default_max_failures")]
    pub max_failures: usize,
    /// Maximum unique stack frames. Defaults to 100; hard limit is 500.
    #[serde(default = "default_max_frames")]
    pub max_frames: usize,
}

#[derive(Debug, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Compiler,
    Test,
    Runtime,
    Generic,
}

#[derive(Debug, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DetectedLanguage {
    Python,
    Node,
    Dotnet,
    Java,
    Unknown,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FailureRecord {
    pub kind: FailureKind,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StackFrame {
    pub file: String,
    pub line: u32,
    pub column: Option<u32>,
    pub symbol: Option<String>,
    pub external: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FailureExtractResult {
    pub failures: Vec<FailureRecord>,
    pub failing_tests: Vec<String>,
    pub stack_frames: Vec<StackFrame>,
    pub detected_language: DetectedLanguage,
    pub external_frame_count: usize,
    pub scanned_lines: usize,
    pub truncated: bool,
}

pub(crate) fn execute(request: &FailureExtractRequest) -> Result<FailureExtractResult, ToolError> {
    validate(request)?;
    let cleaned = strip_compose_prefix(&request.text)?;
    let patterns = Patterns::new()?;
    let mut failures: Vec<FailureRecord> = Vec::new();
    let mut tests = BTreeSet::new();
    let mut frames = Vec::new();
    let mut frame_keys = BTreeSet::new();
    let mut truncated = false;

    for raw in cleaned.lines() {
        let line = raw.trim();
        if let Some(test) = patterns
            .pytest
            .captures(line)
            .and_then(|value| value.name("test"))
        {
            tests.insert(test.as_str().to_owned());
        }
        if let Some(frame) = patterns.frame(line) {
            let key = format!("{}:{}", frame.file, frame.line);
            if frame_keys.insert(key) {
                if frames.len() == request.max_frames {
                    truncated = true;
                } else {
                    frames.push(frame);
                }
            }
        }
        if let Some(location) = patterns.location(line)
            && !patterns.failure.is_match(line)
            && let Some(previous) = failures.last_mut()
            && previous.file.is_none()
        {
            previous.file = Some(location.0);
            previous.line = location.1;
            previous.column = location.2;
        }
        if !line.is_empty() && patterns.failure.is_match(line) {
            if failures.len() == request.max_failures {
                truncated = true;
                continue;
            }
            let location = patterns.location(line);
            failures.push(FailureRecord {
                kind: classify(line),
                message: line.chars().take(500).collect(),
                file: location.as_ref().map(|value| value.0.clone()),
                line: location.as_ref().and_then(|value| value.1),
                column: location.and_then(|value| value.2),
            });
        }
    }
    let detected_language = detect_language(&frames);
    let external_frame_count = frames.iter().filter(|frame| frame.external).count();
    Ok(FailureExtractResult {
        failures,
        failing_tests: tests.into_iter().take(100).collect(),
        stack_frames: frames,
        detected_language,
        external_frame_count,
        scanned_lines: cleaned.lines().count(),
        truncated,
    })
}

fn validate(request: &FailureExtractRequest) -> Result<(), ToolError> {
    if request.text.len() > ABSOLUTE_MAX_INPUT_BYTES {
        return Err(ToolError::LimitExceeded(format!(
            "text is larger than {ABSOLUTE_MAX_INPUT_BYTES} bytes"
        )));
    }
    if request.max_failures == 0
        || request.max_failures > 500
        || request.max_frames == 0
        || request.max_frames > 500
    {
        return Err(ToolError::LimitExceeded(
            "max_failures and max_frames must be between 1 and 500".to_owned(),
        ));
    }
    Ok(())
}

struct Patterns {
    failure: Regex,
    location_re: Regex,
    pytest: Regex,
    python: Regex,
    node: Regex,
    dotnet: Regex,
    java: Regex,
}

impl Patterns {
    fn new() -> Result<Self, ToolError> {
        let compile =
            |value| Regex::new(value).map_err(|error| ToolError::InvalidInput(error.to_string()));
        Ok(Self {
            failure: compile(
                r"(?i)(?:\b(?:error|exception|fatal|failed|failure|panic)\b|assert(?:ion)?\s+(?:failed|error))",
            )?,
            location_re: compile(
                r"(?P<file>(?:[A-Za-z]:)?[^:\s][^:]*?):(?P<line>\d+)(?::(?P<column>\d+))?",
            )?,
            pytest: compile(r"^FAILED\s+(?P<test>\S+::\S+)")?,
            python: compile(r#"File\s+"(?P<file>[^"]+)",\s+line\s+(?P<line>\d+)"#)?,
            node: compile(
                r"at\s+(?:(?P<symbol>[^\s(]+)\s+)?\(?(?P<file>[^()]+?):(?P<line>\d+):(?P<column>\d+)\)?$",
            )?,
            dotnet: compile(r"at\s+(?P<symbol>\S.*?)\s+in\s+(?P<file>.+?):line\s+(?P<line>\d+)")?,
            java: compile(r"at\s+(?P<symbol>[\w$.]+)\((?P<file>[\w$.]+\.java):(?P<line>\d+)\)")?,
        })
    }

    fn location(&self, line: &str) -> Option<(String, Option<u32>, Option<u32>)> {
        let captures = self.location_re.captures(line)?;
        Some((
            captures.name("file")?.as_str().trim().to_owned(),
            capture_number(&captures, "line"),
            capture_number(&captures, "column"),
        ))
    }

    fn frame(&self, line: &str) -> Option<StackFrame> {
        for regex in [&self.python, &self.node, &self.dotnet, &self.java] {
            if let Some(captures) = regex.captures(line) {
                let file = captures.name("file")?.as_str().to_owned();
                return Some(StackFrame {
                    line: capture_number(&captures, "line")?,
                    column: capture_number(&captures, "column"),
                    symbol: captures
                        .name("symbol")
                        .map(|value| value.as_str().to_owned()),
                    external: is_external(&file),
                    file,
                });
            }
        }
        None
    }
}

fn strip_compose_prefix(text: &str) -> Result<String, ToolError> {
    let regex = Regex::new(r"^[\w-]+\s*\|\s+")
        .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
    Ok(text
        .lines()
        .map(|line| regex.replace(line, ""))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn capture_number(captures: &regex::Captures<'_>, name: &str) -> Option<u32> {
    captures.name(name)?.as_str().parse().ok()
}

fn is_external(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        "node_modules/",
        ".venv/",
        "site-packages/",
        "/usr/lib/",
        "/usr/local/",
        "dist/",
        "build/",
    ]
    .iter()
    .any(|hint| lower.contains(hint))
}

fn detect_language(frames: &[StackFrame]) -> DetectedLanguage {
    if frames
        .iter()
        .any(|frame| has_extension(&frame.file, &["py"]))
    {
        DetectedLanguage::Python
    } else if frames
        .iter()
        .any(|frame| has_extension(&frame.file, &["js", "jsx", "mjs", "ts", "tsx"]))
    {
        DetectedLanguage::Node
    } else if frames
        .iter()
        .any(|frame| has_extension(&frame.file, &["cs"]))
    {
        DetectedLanguage::Dotnet
    } else if frames
        .iter()
        .any(|frame| has_extension(&frame.file, &["java"]))
    {
        DetectedLanguage::Java
    } else {
        DetectedLanguage::Unknown
    }
}

fn has_extension(path: &str, expected: &[&str]) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| expected.iter().any(|item| value.eq_ignore_ascii_case(item)))
}

fn classify(line: &str) -> FailureKind {
    let lower = line.to_ascii_lowercase();
    if lower.contains("test") || lower.contains("assert") || line.starts_with("FAILED ") {
        FailureKind::Test
    } else if lower.contains("exception")
        || lower.contains("panic")
        || lower.contains("stack trace")
    {
        FailureKind::Runtime
    } else if lower.starts_with("error[") || lower.starts_with("error:") {
        FailureKind::Compiler
    } else {
        FailureKind::Generic
    }
}

const fn default_max_failures() -> usize {
    50
}
const fn default_max_frames() -> usize {
    100
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{DetectedLanguage, FailureExtractRequest, FailureKind, execute};

    #[test]
    fn extracts_compiler_test_and_python_frame() {
        let result = execute(&FailureExtractRequest {
            text: "error[E0425]: missing value\n --> src/main.rs:10:7\nFAILED tests/test_x.py::test_y\nFile \"app/main.py\", line 42, in run\nValueError: bad".to_owned(),
            max_failures: 50, max_frames: 50,
        }).expect("extract failures");
        assert_eq!(result.failures[0].kind, FailureKind::Compiler);
        assert_eq!(result.failures[0].line, Some(10));
        assert_eq!(result.detected_language, DetectedLanguage::Python);
        assert_eq!(result.failing_tests, ["tests/test_x.py::test_y"]);
    }
}
