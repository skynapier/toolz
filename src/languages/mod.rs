mod js_ts;
mod textual;
mod tree_sitter;

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::ToolError;

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub(crate) enum RelationKind {
    Import,
    Export,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Relation {
    pub kind: RelationKind,
    pub target: String,
    pub line: usize,
    pub evidence: String,
}

pub(crate) fn relations(path: &Path, source: &str) -> Result<Vec<Relation>, ToolError> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("js" | "jsx" | "mjs" | "ts" | "tsx") => js_ts::relations(path, source),
        Some("py") => tree_sitter::python_relations(source),
        Some("cs") => tree_sitter::csharp_relations(source),
        _ => textual::relations(path, source),
    }
}

pub(crate) fn line_number(source: &str, byte_offset: usize) -> usize {
    bytecount::count(&source.as_bytes()[..byte_offset.min(source.len())], b'\n') + 1
}

pub(crate) fn line_text(source: &str, line: usize) -> String {
    source
        .lines()
        .nth(line.saturating_sub(1))
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect()
}
