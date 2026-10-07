//! Bundled C extraction. Trees and grammar details never escape the locator.
use super::{BodySpan, Location, Selection, Span};
use tree_sitter::{Node, Parser};

pub(super) fn extract(source: &[u8]) -> Result<Vec<Location>, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_c::LANGUAGE.into())
        .map_err(|error| format!("C grammar unavailable: {error}"))?;
    let tree = parser.parse(source, None).ok_or("C parse failed")?;
    let root = tree.root_node();
    if root.has_error() || invalid(root) {
        return Err("C parse failed (error or missing node)".to_owned());
    }
    let mut locations = Vec::new();
    collect(root, source, &mut locations);
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

// The identifier declared by a declarator chain, wherever it sits inside
// pointers, arrays, parentheses, and parameter lists.
fn declarator(node: Node<'_>, source: &[u8]) -> Option<(String, usize, usize)> {
    match node.kind() {
        "identifier" => Some((
            text(node, source).to_owned(),
            node.start_position().row + 1,
            node.start_position().column,
        )),
        "pointer_declarator"
        | "parenthesized_declarator"
        | "array_declarator"
        | "init_declarator"
        | "attributed_declarator"
        | "function_declarator" => node
            .named_children(&mut node.walk())
            .find_map(|child| declarator(child, source)),
        _ => None,
    }
}

fn has_function_declarator(node: Node<'_>) -> bool {
    node.kind() == "function_declarator" || {
        let mut cursor = node.walk();
        node.children(&mut cursor).any(has_function_declarator)
    }
}

fn collect(node: Node<'_>, source: &[u8], locations: &mut Vec<Location>) {
    match node.kind() {
        "function_definition" => {
            // A standalone prototype is not an independent function target;
            // parameter names are not declarations of their own.
            if let Some(decl) = node.child_by_field_name("declarator")
                && let Some((name, line, column)) = declarator(decl, source)
            {
                push(node, name, line, column, "function", locations);
            }
        }
        "struct_specifier" | "union_specifier" | "enum_specifier" => {
            // A named specifier with a body is a type definition; a specifier
            // without a body is only a type use, such as a parameter or cast.
            if node.child_by_field_name("body").is_some()
                && let Some(name) = node.child_by_field_name("name")
            {
                let name_text = text(name, source).to_owned();
                // "struct_specifier" | "union_specifier" | "enum_specifier"
                // always split into a non-empty first segment.
                let kind = node.kind().split('_').next().unwrap_or_default();
                push(
                    node,
                    name_text,
                    name.start_position().row + 1,
                    name.start_position().column,
                    kind,
                    locations,
                );
            }
        }
        "declaration" => {
            // Keep applicable legacy targets structurally: constants and
            // explicit variable declarations, including local ones. A
            // declarator chain containing a function declarator is a
            // prototype, never a variable target. Every declarator in the
            // chain is a separate target, not just the first.
            let kind = if is_constant(node, source) {
                "constant"
            } else {
                "variable"
            };
            let mut cursor = node.walk();
            let declarators = node
                .children_by_field_name("declarator", &mut cursor)
                .collect::<Vec<_>>();
            for decl in declarators {
                if has_function_declarator(decl) {
                    continue;
                }
                if let Some((name, line, column)) = declarator(decl, source) {
                    push(decl, name, line, column, kind, locations);
                }
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect(child, source, locations);
    }
}

fn is_constant(declaration: Node<'_>, source: &[u8]) -> bool {
    let mut cursor = declaration.walk();
    declaration
        .children(&mut cursor)
        .take_while(|child| child.kind() != "init_declarator" && child.kind() != "identifier")
        .any(|child| child.kind() == "type_qualifier" && text(child, source) == "const")
}

fn push(
    node: Node<'_>,
    name: String,
    identifier_line: usize,
    identifier_column: usize,
    kind: &str,
    locations: &mut Vec<Location>,
) {
    let start = node.start_position().row + 1;
    let end = node.end_position().row + usize::from(node.end_position().column > 0);
    locations.push(Location {
        name: name.clone(),
        selection: Selection {
            input_selector: name.clone(),
            qualified_name: name,
            kind: kind.to_owned(),
            start_line: start,
            end_line: end,
            identifier_line,
            identifier_column,
            language: "c".to_owned(),
            mode: "structured".to_owned(),
        },
        identifier_line,
        identifier_column,
        span: Span { start, end },
        body_span: node.child_by_field_name("body").map(BodySpan::from_node),
        notice: None,
    });
}

#[cfg(test)]
mod tests {
    use super::extract;

    fn summaries(source: &[u8]) -> Vec<(String, String, usize, usize)> {
        extract(source)
            .unwrap()
            .into_iter()
            .map(|location| {
                (
                    location.name,
                    location.selection.kind,
                    location.span.start,
                    location.span.end,
                )
            })
            .collect()
    }

    #[test]
    fn finds_functions_and_named_types_with_bodies() {
        let source = b"static int compute(int a) {\n    return a + 1;\n}\nint compute(void);\nstruct Point {\n    int x;\n};\nunion Blob { int i; };\nenum Color { RED };\n";
        assert_eq!(
            summaries(source),
            vec![
                ("compute".to_owned(), "function".to_owned(), 1, 3),
                ("Point".to_owned(), "struct".to_owned(), 5, 7),
                ("Blob".to_owned(), "union".to_owned(), 8, 8),
                ("Color".to_owned(), "enum".to_owned(), 9, 9),
            ]
        );
    }

    #[test]
    fn constants_and_explicit_variables_keep_legacy_targets() {
        let source = b"const int LIMIT = 3;\nstatic const unsigned long MAX = 10UL;\nint counter;\nint counter2 = 5;\nstruct Point p;\nNode *head;\n";
        assert_eq!(
            summaries(source),
            vec![
                ("LIMIT".to_owned(), "constant".to_owned(), 1, 1),
                ("MAX".to_owned(), "constant".to_owned(), 2, 2),
                ("counter".to_owned(), "variable".to_owned(), 3, 3),
                ("counter2".to_owned(), "variable".to_owned(), 4, 4),
                ("p".to_owned(), "variable".to_owned(), 5, 5),
                ("head".to_owned(), "variable".to_owned(), 6, 6),
            ]
        );
    }

    #[test]
    fn prototypes_and_type_uses_are_not_targets() {
        let source = b"int compute(int a);\nextern int compute(int a);\nint (*get(void))(int);\ntypedef int (*handler)(int);\nhandler h;\nstruct Uses { struct Inner *next; };\nint cast(int value);\nint cast(int value) { return (int)(value); }\n";
        assert_eq!(
            summaries(source),
            vec![
                ("h".to_owned(), "variable".to_owned(), 5, 5),
                ("Uses".to_owned(), "struct".to_owned(), 6, 6),
                ("cast".to_owned(), "function".to_owned(), 8, 8),
            ]
        );
    }

    #[test]
    fn multiline_declarations_keep_complete_ranges_and_identifier_positions() {
        let source = b"struct Point {\n    int x;\n    int y;\n};\nstatic int\ncompute(\n    int a\n) {\n    return a;\n}\nconst char *\nmsg = \"hi\";\n";
        assert_eq!(
            summaries(source),
            vec![
                ("Point".to_owned(), "struct".to_owned(), 1, 4),
                ("compute".to_owned(), "function".to_owned(), 5, 10),
                ("msg".to_owned(), "constant".to_owned(), 11, 12),
            ]
        );
        let locations = extract(source).unwrap();
        let msg = locations.last().unwrap();
        assert_eq!((msg.identifier_line, msg.identifier_column), (12, 0));
        let compute = &locations[1];
        assert_eq!((compute.identifier_line, compute.identifier_column), (6, 0));
    }

    #[test]
    fn conditional_branches_yield_colliding_candidates() {
        let source = b"#ifdef FAST\nint compute(void) {\n    return 1;\n}\n#else\nint compute(void) {\n    return 2;\n}\n#endif\n";
        let locations = extract(source).unwrap();
        assert_eq!(locations.len(), 2);
        assert!(locations.iter().all(|location| location.name == "compute"));
        assert_eq!(locations[0].span.start, 2);
        assert_eq!(locations[1].span.start, 6);
    }
}
