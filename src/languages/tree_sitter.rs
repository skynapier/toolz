use tree_sitter::{Language, Node, Parser};

use crate::{
    error::ToolError,
    languages::{Relation, RelationKind},
};

pub(super) fn python_relations(source: &str) -> Result<Vec<Relation>, ToolError> {
    parse_relations(
        &tree_sitter_python::LANGUAGE.into(),
        source,
        LanguageFlavor::Python,
    )
}

pub(super) fn csharp_relations(source: &str) -> Result<Vec<Relation>, ToolError> {
    parse_relations(
        &tree_sitter_c_sharp::LANGUAGE.into(),
        source,
        LanguageFlavor::CSharp,
    )
}

#[derive(Clone, Copy)]
enum LanguageFlavor {
    Python,
    CSharp,
}

fn parse_relations(
    language: &Language,
    source: &str,
    flavor: LanguageFlavor,
) -> Result<Vec<Relation>, ToolError> {
    let mut parser = Parser::new();
    parser
        .set_language(language)
        .map_err(|error| ToolError::InvalidInput(format!("tree-sitter language error: {error}")))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| ToolError::InvalidInput("tree-sitter parsing was cancelled".to_owned()))?;
    let mut stack = vec![tree.root_node()];
    let mut output = Vec::new();
    while let Some(node) = stack.pop() {
        if let Some(relation) = relation_for_node(node, source, flavor) {
            output.push(relation);
        }
        let mut cursor = node.walk();
        let mut children: Vec<_> = node.named_children(&mut cursor).collect();
        children.reverse();
        stack.extend(children);
    }
    output.sort_by(|left, right| (left.line, &left.target).cmp(&(right.line, &right.target)));
    Ok(output)
}

fn relation_for_node(node: Node<'_>, source: &str, flavor: LanguageFlavor) -> Option<Relation> {
    let kind = node.kind();
    let (relation_kind, target_node) = match flavor {
        LanguageFlavor::Python => match kind {
            "import_from_statement" => (
                RelationKind::Import,
                node.child_by_field_name("module_name"),
            ),
            "import_statement" => (RelationKind::Import, node.child_by_field_name("name")),
            "function_definition" | "class_definition" => {
                (RelationKind::Export, node.child_by_field_name("name"))
            }
            _ => return None,
        },
        LanguageFlavor::CSharp => match kind {
            "using_directive" => (RelationKind::Import, node.named_child(0)),
            "class_declaration"
            | "interface_declaration"
            | "struct_declaration"
            | "enum_declaration" => (RelationKind::Export, node.child_by_field_name("name")),
            _ => return None,
        },
    };
    let target_node = target_node?;
    let target = target_node
        .utf8_text(source.as_bytes())
        .ok()?
        .trim()
        .to_owned();
    let line = node.start_position().row + 1;
    Some(Relation {
        kind: relation_kind,
        target,
        line,
        evidence: node
            .utf8_text(source.as_bytes())
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(300)
            .collect(),
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{csharp_relations, python_relations};

    #[test]
    fn parses_python_imports() {
        let relations = python_relations("from app.models import User\nclass Service: pass\n")
            .expect("python parse");
        assert!(
            relations
                .iter()
                .any(|relation| relation.target == "app.models")
        );
    }

    #[test]
    fn parses_csharp_using_directives() {
        let relations =
            csharp_relations("using System.Text;\npublic class Service {}").expect("C# parse");
        assert!(
            relations
                .iter()
                .any(|relation| relation.target.contains("System.Text"))
        );
    }
}
