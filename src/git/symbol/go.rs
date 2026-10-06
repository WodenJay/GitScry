//! Bundled Go extraction. Trees and grammar details never escape the locator.
use super::{Location, Selection, Span};
use tree_sitter::{Node, Parser};

pub(super) fn extract(source: &[u8]) -> Result<Vec<Location>, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .map_err(|error| format!("Go grammar unavailable: {error}"))?;
    let tree = parser.parse(source, None).ok_or("Go parse failed")?;
    let root = tree.root_node();
    if root.has_error() || invalid(root) {
        return Err("Go parse failed (error or missing node)".to_owned());
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

fn location(
    qualified: &[String],
    kind: &str,
    node: Node<'_>,
    identifier: Node<'_>,
    source: &[u8],
) -> Location {
    let start = node.start_position().row + 1;
    let end = node.end_position().row + usize::from(node.end_position().column > 0);
    let identifier_line = identifier.start_position().row + 1;
    let identifier_column = identifier.start_position().column;
    Location {
        name: text(identifier, source).to_owned(),
        selection: Selection {
            input_selector: qualified.join("."),
            qualified_name: qualified.join("."),
            kind: kind.to_owned(),
            start_line: start,
            end_line: end,
            identifier_line,
            identifier_column,
            language: "go".to_owned(),
            mode: "structured".to_owned(),
        },
        identifier_line,
        identifier_column,
        span: Span { start, end },
        notice: None,
    }
}

// A receiver's named type, not its pointer form or its type arguments, supplies
// ownership. Value and pointer receivers normalize to the same owner, generic
// arguments are stripped, and imported receiver types are never resolved.
fn receiver_owner(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "type_identifier" => Some(text(node, source).to_owned()),
        "pointer_type" => receiver_owner(node.named_child(0)?, source),
        "generic_type" => receiver_owner(node.child_by_field_name("type")?, source),
        _ => None,
    }
}

fn visit(node: Node<'_>, source: &[u8], owners: &[String], locations: &mut Vec<Location>) {
    if node.kind() == "method_declaration" {
        // Only body-bearing methods become method targets; a receiver's named type
        // owns the method regardless of the pointer or value form.
        if let (Some(receiver), Some(name), Some(_body)) = (
            node.child_by_field_name("receiver"),
            node.child_by_field_name("name"),
            node.child_by_field_name("body"),
        ) {
            let owner = receiver
                .named_child(0)
                .filter(|decl| decl.kind() == "parameter_declaration")
                .and_then(|decl| decl.child_by_field_name("type"))
                .and_then(|receiver| receiver_owner(receiver, source));
            if let Some(owner) = owner {
                let mut qualified = owners.to_vec();
                qualified.push(owner);
                qualified.push(text(name, source).to_owned());
                locations.push(location(&qualified, "method", node, name, source));
            }
        }
        // Methods never scope nested declarations beyond their own name.
        if let Some(name) = node.child_by_field_name("name") {
            let mut nested = owners.to_vec();
            nested.push(text(name, source).to_owned());
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                visit(child, source, &nested, locations);
            }
        }
        return;
    }
    let kind = match node.kind() {
        "function_declaration" if node.child_by_field_name("body").is_some() => Some("function"),
        "type_spec" | "type_alias" => Some("type"),
        "const_spec" => Some("constant"),
        "var_spec" => Some("variable"),
        _ => None,
    };
    if let (Some(kind), Some(identifier)) = (kind, node.child_by_field_name("name")) {
        let mut qualified = owners.to_vec();
        qualified.push(text(identifier, source).to_owned());
        locations.push(location(&qualified, kind, node, identifier, source));
    }
    let mut nested = owners.to_vec();
    if matches!(
        node.kind(),
        "function_declaration" | "type_spec" | "type_alias"
    ) && let Some(name) = node.child_by_field_name("name")
    {
        nested.push(text(name, source).to_owned());
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, source, &nested, locations);
    }
}

#[cfg(test)]
mod tests {
    use super::super::locate_unique as locate;

    #[test]
    fn go_declarations_select_qualified_structured_ranges() {
        let source = concat!(
            "// leading comment\n",
            "package net\n",
            "\n",
            "type client[T any] struct {\n",
            "\tinner T\n",
            "}\n",
            "\n",
            "func (c *client[T]) Request(path string) error {\n",
            "\treturn nil\n",
            "}\n",
            "\n",
            "func (c client[T]) Value() int { return 1 }\n",
            "\n",
            "func Parse[T any](value T) T {\n",
            "\treturn value\n",
            "}\n",
        );
        let client = locate(source.as_bytes(), "client", "source.go").unwrap();
        assert_eq!(client.selection.kind, "type");
        assert_eq!((client.span.start, client.span.end), (4, 6));
        assert_eq!((client.identifier_line, client.identifier_column), (4, 5));
        let request = locate(source.as_bytes(), "client.Request", "source.go").unwrap();
        assert_eq!(request.selection.qualified_name, "client.Request");
        assert_eq!(request.selection.kind, "method");
        assert_eq!((request.span.start, request.span.end), (8, 10));
        assert_eq!(
            (request.identifier_line, request.identifier_column),
            (8, 20)
        );
        let value = locate(source.as_bytes(), "client.Value", "source.go").unwrap();
        assert_eq!(value.selection.qualified_name, "client.Value");
        assert_eq!(value.selection.kind, "method");
        assert_eq!((value.span.start, value.span.end), (12, 12));
        let parse = locate(source.as_bytes(), "Parse", "source.go").unwrap();
        assert_eq!(parse.selection.kind, "function");
        assert_eq!((parse.span.start, parse.span.end), (14, 16));
        for selected in [client, request, value, parse] {
            assert_eq!(selected.selection.language, "go");
            assert_eq!(selected.selection.mode, "structured");
            assert!(selected.notice.is_none());
        }
    }

    #[test]
    fn go_grouped_specs_and_receiver_collisions_stay_explicit() {
        let source = concat!(
            "package net\n",
            "\n",
            "const (\n",
            "\tlimit = 3\n",
            ")\n",
            "\n",
            "var (\n",
            "\tglobal = 4\n",
            ")\n",
            "\n",
            "type Alias = client[int]\n",
            "\n",
            "func (a Inner) m() {}\n",
            "func (b Outer) m() {}\n",
        );
        let limit = locate(source.as_bytes(), "limit", "source.go").unwrap();
        assert_eq!(limit.selection.kind, "constant");
        assert_eq!((limit.span.start, limit.span.end), (4, 4));
        let global = locate(source.as_bytes(), "global", "source.go").unwrap();
        assert_eq!(global.selection.kind, "variable");
        let alias = locate(source.as_bytes(), "Alias", "source.go").unwrap();
        assert_eq!(alias.selection.kind, "type");
        let collision = locate(source.as_bytes(), "m", "source.go").err().unwrap();
        let message = collision.to_string();
        assert!(message.contains("ambiguous"), "{message}");
        assert!(message.contains("Inner.m"), "{message}");
        assert!(message.contains("Outer.m"), "{message}");
        let method = locate(source.as_bytes(), "Inner.m", "source.go").unwrap();
        assert_eq!(method.selection.kind, "method");
    }

    #[test]
    fn go_failed_parses_degrade_to_lightweight_without_owner_stripping() {
        let broken = b"func real() {\n}\nfunc broken(\n";
        let selected = locate(broken, "real", "source.go").unwrap();
        assert_eq!(selected.selection.mode, "lightweight");
        assert_eq!(selected.selection.language, "go");
        assert!(selected.notice.unwrap().contains("parse failed"));
        assert!(locate(broken, "owner.real", "source.go").is_err());
    }
}
