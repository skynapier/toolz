use std::path::Path;

use regex::Regex;

use crate::{
    error::ToolError,
    languages::{Relation, RelationKind},
};

pub(super) fn relations(_path: &Path, source: &str) -> Result<Vec<Relation>, ToolError> {
    let import = Regex::new(
        r#"(?:^\s*(?:from|import)\s+|\brequire\s*\()[^\n]*?["']?(?P<module>[A-Za-z0-9_./: -]+)["']?|^\s*use\s+(?P<rust>[^;]+)"#,
    )
    .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
    let export = Regex::new(
        r"^\s*(?:pub\s+(?:fn|struct|enum|trait|type|mod|const|static)\b|(?:def|class)\s+)",
    )
    .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
    let mut output = Vec::new();
    for (index, line) in source.lines().enumerate() {
        if let Some(captures) = import.captures(line)
            && let Some(target) = captures.name("module").or_else(|| captures.name("rust"))
        {
            output.push(Relation {
                kind: RelationKind::Import,
                target: target.as_str().trim().to_owned(),
                line: index + 1,
                evidence: line.chars().take(300).collect(),
            });
        }
        if export.is_match(line) {
            output.push(Relation {
                kind: RelationKind::Export,
                target: line.trim().to_owned(),
                line: index + 1,
                evidence: line.chars().take(300).collect(),
            });
        }
    }
    Ok(output)
}
