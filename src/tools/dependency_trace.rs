use std::{fs, path::Path};

use globset::Glob;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    error::ToolError,
    index::{RelationCache, source_files},
    languages::RelationKind,
};

const MAX_EDGES: usize = 1_000;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DependencyTraceRequest {
    /// Absolute repository root.
    pub repo_root: String,
    /// Symbol, module specifier, or repository-relative source path to trace.
    pub target: String,
    /// Interpret target as a symbol (default) or repository-relative file path.
    #[serde(default)]
    pub target_type: TargetType,
    /// Return imports, exports, references, or all categories (default).
    #[serde(default)]
    pub mode: TraceMode,
    /// Optional repository-relative subtree constraint.
    pub path: Option<String>,
    /// Optional gitignore-style source-file glob.
    pub glob: Option<String>,
    /// Maximum dependency/reference edges. Defaults to 200; hard limit is 1000.
    #[serde(default = "default_max_edges")]
    pub max_edges: usize,
}

#[derive(Debug, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetType {
    #[default]
    Symbol,
    File,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceMode {
    Imports,
    Exports,
    References,
    #[default]
    All,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DependencyKind {
    Import,
    Export,
    Reference,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DependencyEdge {
    pub kind: DependencyKind,
    pub path: String,
    pub line: usize,
    pub target: String,
    pub evidence: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DependencyTraceResult {
    pub edges: Vec<DependencyEdge>,
    pub files_scanned: usize,
    pub truncated: bool,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn execute(
    request: &DependencyTraceRequest,
) -> Result<DependencyTraceResult, ToolError> {
    if request.target.trim().is_empty() {
        return Err(ToolError::InvalidInput(
            "target must not be empty".to_owned(),
        ));
    }
    if request.max_edges == 0 || request.max_edges > MAX_EDGES {
        return Err(ToolError::LimitExceeded(format!(
            "max_edges must be between 1 and {MAX_EDGES}"
        )));
    }
    let root = Path::new(&request.repo_root)
        .canonicalize()
        .map_err(ToolError::Io)?;
    let files = source_files(&root)?;
    let cache = RelationCache::open(&root);
    let target_word = Regex::new(&format!(r"\b{}\b", regex::escape(request.target.trim())))
        .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
    let target_normalized = request.target.replace('\\', "/");
    let target_stem = Path::new(&target_normalized)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(&target_normalized);
    let mut edges = Vec::new();
    let mut truncated = false;
    let subtree = request
        .path
        .as_deref()
        .map(|value| value.replace('\\', "/"));
    let glob = request
        .glob
        .as_deref()
        .map(Glob::new)
        .transpose()
        .map_err(|error| ToolError::InvalidInput(format!("invalid glob: {error}")))?
        .map(|value| value.compile_matcher());

    'files: for file in &files {
        if subtree
            .as_ref()
            .is_some_and(|value| !file.relative.starts_with(value))
            || glob
                .as_ref()
                .is_some_and(|value| !value.is_match(&file.relative))
        {
            continue;
        }
        let Ok(source) = fs::read_to_string(&file.absolute) else {
            continue;
        };
        let parsed = cache
            .as_ref()
            .map_or_else(
                || crate::languages::relations(&file.absolute, &source),
                |cache| cache.get_or_parse(&file.absolute, &source),
            )
            .unwrap_or_default();
        for relation in parsed {
            let category_allowed = match relation.kind {
                RelationKind::Import => mode_allows(&request.mode, DependencyKind::Import),
                RelationKind::Export => mode_allows(&request.mode, DependencyKind::Export),
            };
            let relevant = match request.target_type {
                TargetType::File => match relation.kind {
                    RelationKind::Import => {
                        file.relative != target_normalized && relation.target.contains(target_stem)
                    }
                    RelationKind::Export => file.relative == target_normalized,
                },
                TargetType::Symbol => {
                    relation.target == request.target || target_word.is_match(&relation.evidence)
                }
            };
            if category_allowed && relevant {
                let kind = match relation.kind {
                    RelationKind::Import => DependencyKind::Import,
                    RelationKind::Export => DependencyKind::Export,
                };
                push_edge(
                    &mut edges,
                    request.max_edges,
                    DependencyEdge {
                        kind,
                        path: file.relative.clone(),
                        line: relation.line,
                        target: relation.target,
                        evidence: relation.evidence,
                    },
                    &mut truncated,
                );
            }
            if truncated {
                break 'files;
            }
        }
        for (index, line) in source.lines().enumerate() {
            if request.target_type == TargetType::Symbol
                && mode_allows(&request.mode, DependencyKind::Reference)
                && target_word.is_match(line)
                && !line.trim_start().starts_with("//")
            {
                push_edge(
                    &mut edges,
                    request.max_edges,
                    DependencyEdge {
                        kind: DependencyKind::Reference,
                        path: file.relative.clone(),
                        line: index + 1,
                        target: request.target.clone(),
                        evidence: line.chars().take(300).collect(),
                    },
                    &mut truncated,
                );
            }
            if truncated {
                break 'files;
            }
        }
    }
    edges.sort_by(|left, right| {
        (&left.path, left.line, &left.kind).cmp(&(&right.path, right.line, &right.kind))
    });
    Ok(DependencyTraceResult {
        edges,
        files_scanned: files.len(),
        truncated,
    })
}

fn mode_allows(mode: &TraceMode, kind: DependencyKind) -> bool {
    matches!(
        (mode, kind),
        (TraceMode::All, _)
            | (TraceMode::Imports, DependencyKind::Import)
            | (TraceMode::Exports, DependencyKind::Export)
            | (TraceMode::References, DependencyKind::Reference)
    )
}

fn push_edge(
    edges: &mut Vec<DependencyEdge>,
    limit: usize,
    edge: DependencyEdge,
    truncated: &mut bool,
) {
    if edges.len() == limit {
        *truncated = true;
    } else {
        edges.push(edge);
    }
}

const fn default_max_edges() -> usize {
    200
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{DependencyKind, DependencyTraceRequest, execute};

    #[test]
    fn finds_import_and_reference_evidence() {
        let root = tempdir().expect("test directory");
        fs::write(
            root.path().join("a.ts"),
            "import { Widget } from './widget';\nnew Widget();",
        )
        .expect("fixture");
        let result = execute(&DependencyTraceRequest {
            repo_root: root.path().display().to_string(),
            target: "Widget".to_owned(),
            target_type: super::TargetType::Symbol,
            mode: super::TraceMode::All,
            path: None,
            glob: None,
            max_edges: 20,
        })
        .expect("trace");
        assert!(
            result
                .edges
                .iter()
                .any(|edge| edge.kind == DependencyKind::Import)
        );
    }
}
