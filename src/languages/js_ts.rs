use std::path::Path;

use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;

use crate::{
    error::ToolError,
    languages::{Relation, RelationKind, line_number, line_text},
};

pub(super) fn relations(path: &Path, source: &str) -> Result<Vec<Relation>, ToolError> {
    let source_type = SourceType::from_path(path).map_err(|error| {
        ToolError::InvalidInput(format!("unsupported JS/TS source type: {error}"))
    })?;
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.fatal_error {
        return Err(ToolError::InvalidInput(format!(
            "Oxc could not parse {}",
            path.display()
        )));
    }

    let mut output = Vec::new();
    for (module, occurrences) in &parsed.module_record.requested_modules {
        for occurrence in occurrences {
            let line = line_number(source, occurrence.statement_span.start as usize);
            output.push(Relation {
                kind: if occurrence.is_import {
                    RelationKind::Import
                } else {
                    RelationKind::Export
                },
                target: module.to_string(),
                line,
                evidence: line_text(source, line),
            });
        }
    }
    for (binding, span) in &parsed.module_record.exported_bindings {
        let line = line_number(source, span.start as usize);
        output.push(Relation {
            kind: RelationKind::Export,
            target: binding.to_string(),
            line,
            evidence: line_text(source, line),
        });
    }
    output.sort_by(|left, right| (left.line, &left.target).cmp(&(right.line, &right.target)));
    Ok(output)
}
