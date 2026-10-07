//! Bundled Rust extraction. Trees and grammar details never escape the locator.
use super::{BodySpan, Location, Selection, Span};
use tree_sitter::{Node, Parser};

pub(super) fn extract(source: &[u8]) -> Result<Vec<Location>, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|error| format!("Rust grammar unavailable: {error}"))?;
    let tree = parser.parse(source, None).ok_or("Rust parse failed")?;
    let root = tree.root_node();
    if root.has_error() || invalid(root) {
        return Err("Rust parse failed (error or missing node)".to_owned());
    }
    let mut locations = Vec::new();
    visit(root, source, &[], &mut locations);
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

// An implementation's self type, not its trait or signature, supplies ownership.
// Strip generic arguments structurally, including arguments on scoped path segments.
fn owner(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" | "identifier" | "primitive_type" => Some(text(node, source).to_owned()),
        "generic_type" => owner(node.child_by_field_name("type")?, source),
        "scoped_type_identifier" | "scoped_identifier" => {
            let name = owner(node.child_by_field_name("name")?, source)?;
            match node.child_by_field_name("path") {
                Some(path) => Some(format!("{}::{name}", owner(path, source)?)),
                None => Some(name),
            }
        }
        _ => None,
    }
}

fn visit(node: Node<'_>, source: &[u8], owners: &[String], locations: &mut Vec<Location>) {
    let kind = match node.kind() {
        "function_item" if node.child_by_field_name("body").is_some() => Some("function"),
        "mod_item" => Some("module"),
        "struct_item" => Some("struct"),
        "enum_item" => Some("enum"),
        "union_item" => Some("union"),
        "trait_item" => Some("trait"),
        "type_item" => Some("type"),
        "const_item" => Some("constant"),
        "static_item" => Some("static"),
        "let_declaration" => Some("variable"),
        _ => None,
    };
    let identifier = if node.kind() == "let_declaration" {
        node.child_by_field_name("pattern")
            .filter(|pattern| pattern.kind() == "identifier")
    } else {
        node.child_by_field_name("name")
    };
    if let (Some(mut kind), Some(identifier)) = (kind, identifier) {
        if kind == "function"
            && node
                .parent()
                .and_then(|parent| parent.parent())
                .is_some_and(|parent| matches!(parent.kind(), "impl_item" | "trait_item"))
        {
            kind = "method";
        }
        let name = text(identifier, source).to_owned();
        let mut qualified = owners.to_vec();
        qualified.push(name.clone());
        let mut first = node;
        while let Some(previous) = first.prev_named_sibling() {
            if previous.kind() != "attribute_item" {
                break;
            }
            first = previous;
        }
        let start = first.start_position().row + 1;
        let end = node.end_position().row + usize::from(node.end_position().column > 0);
        let identifier_line = identifier.start_position().row + 1;
        let identifier_column = identifier.start_position().column;
        locations.push(Location {
            name,
            selection: Selection {
                input_selector: qualified.join("::"),
                qualified_name: qualified.join("::"),
                kind: kind.to_owned(),
                start_line: start,
                end_line: end,
                identifier_line,
                identifier_column,
                language: "rust".to_owned(),
                mode: "structured".to_owned(),
            },
            identifier_line,
            identifier_column,
            span: Span { start, end },
            body_span: node.child_by_field_name("body").map(BodySpan::from_node),
            notice: None,
        });
    }
    let mut nested = owners.to_vec();
    match node.kind() {
        "mod_item" | "trait_item" | "function_item" => {
            if let Some(name) = node.child_by_field_name("name") {
                nested.push(text(name, source).to_owned());
            }
        }
        "impl_item" => {
            if let Some(name) = node
                .child_by_field_name("type")
                .and_then(|node| owner(node, source))
            {
                nested.push(name);
            }
            // Composite self types still contribute methods and simple-name collisions.
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, source, &nested, locations);
    }
}
