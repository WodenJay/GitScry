//! Bundled JavaScript/TypeScript extraction. Trees and grammar details never
//! escape the locator.
use super::{BodySpan, Location, Selection, Span};
use tree_sitter::{Node, Parser};

#[derive(Clone, Copy)]
pub(super) enum Language {
    JavaScript,
    TypeScript,
    Tsx,
}

impl Language {
    fn name(self) -> &'static str {
        match self {
            Language::JavaScript => "javascript",
            Language::TypeScript | Language::Tsx => "typescript",
        }
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        }
    }
}

pub(super) fn extract(source: &[u8], language: Language) -> Result<Vec<Location>, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&language.grammar())
        .map_err(|error| format!("{} grammar unavailable: {error}", language.name()))?;
    let name = language.name();
    let tree = parser
        .parse(source, None)
        .ok_or(format!("{name} parse failed"))?;
    let root = tree.root_node();
    if root.has_error() || invalid(root) {
        return Err(format!("{name} parse failed (error or missing node)"));
    }
    let mut locations = Vec::new();
    visit(root, source, language, &[], &mut locations);
    Ok(locations)
}

fn invalid(node: Node<'_>) -> bool {
    node.is_error() || node.is_missing() || {
        let mut cursor = node.walk();
        node.children(&mut cursor).any(invalid)
    }
}

fn text<'a>(node: Node<'_>, source: &'a [u8]) -> &'a str {
    std::str::from_utf8(&source[node.byte_range()]).unwrap_or("")
}

fn visit(
    node: Node<'_>,
    source: &[u8],
    language: Language,
    owners: &[String],
    locations: &mut Vec<Location>,
) {
    let kind = match node.kind() {
        "function_declaration" | "generator_function_declaration" => Some("function"),
        "class_declaration" | "abstract_class_declaration" => Some("class"),
        "interface_declaration" => Some("interface"),
        "type_alias_declaration" => Some("type"),
        "enum_declaration" => Some("enum"),
        "method_definition" => Some(method_kind(node, source)),
        "variable_declarator" => Some("variable"),
        _ => None,
    };
    let identifier = match node.kind() {
        "variable_declarator" => node
            .child_by_field_name("name")
            .filter(|name| name.kind() == "identifier"),
        _ => node.child_by_field_name("name"),
    };
    if let (Some(kind), Some(identifier)) = (kind, identifier) {
        let name = text(identifier, source).to_owned();
        let mut qualified = owners.to_vec();
        qualified.push(name.clone());
        let first = attached(node);
        let start = first.start_position().row + 1;
        let end = node.end_position().row + usize::from(node.end_position().column > 0);
        let identifier_line = identifier.start_position().row + 1;
        let identifier_column = identifier.start_position().column;
        locations.push(Location {
            name,
            selection: Selection {
                input_selector: qualified.join("."),
                qualified_name: qualified.join("."),
                kind: kind.to_owned(),
                start_line: start,
                end_line: end,
                identifier_line,
                identifier_column,
                language: language.name().to_owned(),
                mode: "structured".to_owned(),
            },
            identifier_line,
            identifier_column,
            span: Span { start, end },
            body_span: node
                .child_by_field_name("body")
                .or_else(|| {
                    node.child_by_field_name("value")
                        .and_then(|value| value.child_by_field_name("body"))
                })
                .map(BodySpan::from_node),
            notice: None,
        });
    }
    let mut nested = owners.to_vec();
    match node.kind() {
        "class_declaration"
        | "abstract_class_declaration"
        | "function_declaration"
        | "generator_function_declaration"
        | "method_definition" => {
            if let Some(name) = node.child_by_field_name("name") {
                nested.push(text(name, source).to_owned());
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, source, language, &nested, locations);
    }
}

// The earliest attached syntax of a declaration: its own decorator field,
// sibling decorators before class members, or the wrapping export statement.
fn attached(node: Node<'_>) -> Node<'_> {
    let mut first = node.child_by_field_name("decorator").unwrap_or(node);
    if let Some(parent) = node.parent()
        && (parent.kind() == "class_body" || parent.kind() == "export_statement")
        && node.child_by_field_name("decorator").is_none()
    {
        while let Some(previous) = first.prev_named_sibling() {
            if previous.kind() != "decorator" {
                break;
            }
            first = previous;
        }
    }
    first
}

fn method_kind(node: Node<'_>, source: &[u8]) -> &'static str {
    if let Some(name) = node.child_by_field_name("name")
        && text(name, source) == "constructor"
    {
        return "constructor";
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            continue;
        }
        match child.kind() {
            "get" | "static get" => return "getter",
            "set" => return "setter",
            _ => {}
        }
    }
    "method"
}
