//! Bundled Python extraction. Trees and grammar details never escape the locator.
use super::{BodySpan, Location, Selection, Span};
use tree_sitter::{Node, Parser};

pub(super) fn extract(source: &[u8]) -> Result<Vec<Location>, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .map_err(|error| format!("Python grammar unavailable: {error}"))?;
    let tree = parser.parse(source, None).ok_or("Python parse failed")?;
    let root = tree.root_node();
    if root.has_error() || invalid(root) {
        return Err("Python parse failed (error or missing node)".to_owned());
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

// A definition directly in a class body is a method, decorated or not.
fn class_member(node: Node<'_>) -> bool {
    let mut parent = node.parent();
    if parent.is_some_and(|parent| parent.kind() == "decorated_definition") {
        parent = parent.and_then(|parent| parent.parent());
    }
    parent
        .filter(|parent| parent.kind() == "block")
        .is_some_and(|block| {
            block
                .parent()
                .is_some_and(|body| body.kind() == "class_definition")
        })
}

fn visit(node: Node<'_>, source: &[u8], owners: &[String], locations: &mut Vec<Location>) {
    let kind = match node.kind() {
        "function_definition" if node.child_by_field_name("body").is_some() => {
            Some(if class_member(node) {
                "method"
            } else {
                "function"
            })
        }
        "class_definition" => Some("class"),
        _ => None,
    };
    if let (Some(kind), Some(identifier)) = (kind, node.child_by_field_name("name")) {
        let name = text(identifier, source).to_owned();
        let mut qualified = owners.to_vec();
        qualified.push(name.clone());
        // Attached decorators open the range; independent comments stay outside.
        let first = node
            .parent()
            .filter(|parent| parent.kind() == "decorated_definition")
            .unwrap_or(node);
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
                language: "python".to_owned(),
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
    if let Some(name) = node
        .child_by_field_name("name")
        .filter(|_| matches!(node.kind(), "function_definition" | "class_definition"))
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
    fn python_definitions_select_qualified_structured_ranges() {
        let source = concat!(
            "# leading comment\n",
            "\n",
            "@exported\n",
            "class Store(Base):\n",
            "    attr = 1\n",
            "\n",
            "    @staticmethod\n",
            "    async def run(self, value: int) -> int:\n",
            "        \"\"\"docs\"\"\"\n",
            "        return value\n",
            "\n",
            "def top():\n",
            "    def nested():\n",
            "        pass\n",
            "    return nested\n",
        );
        let store = locate(source.as_bytes(), "Store", "source.py").unwrap();
        assert_eq!(store.selection.kind, "class");
        assert_eq!((store.span.start, store.span.end), (3, 10));
        assert_eq!((store.identifier_line, store.identifier_column), (4, 6));
        let run = locate(source.as_bytes(), "run", "source.py").unwrap();
        assert_eq!(run.selection.qualified_name, "Store.run");
        assert_eq!(run.selection.kind, "method");
        assert_eq!((run.span.start, run.span.end), (7, 10));
        assert_eq!((run.identifier_line, run.identifier_column), (8, 14));
        let top = locate(source.as_bytes(), "top", "source.py").unwrap();
        assert_eq!(top.selection.kind, "function");
        assert_eq!((top.span.start, top.span.end), (12, 15));
        let nested = locate(source.as_bytes(), "top.nested", "source.py").unwrap();
        assert_eq!(nested.selection.qualified_name, "top.nested");
        assert_eq!(nested.selection.kind, "function");
        assert_eq!((nested.span.start, nested.span.end), (13, 14));
        for selected in [store, run, top, nested] {
            assert_eq!(selected.selection.language, "python");
            assert_eq!(selected.selection.mode, "structured");
            assert!(selected.notice.is_none());
        }
    }

    #[test]
    fn python_owner_collisions_stay_explicit_ambiguities() {
        let source = concat!(
            "class Inner:\n",
            "    class Collide:\n",
            "        def m(self):\n",
            "            pass\n",
            "\n",
            "class Other:\n",
            "    class Collide:\n",
            "        def m(self):\n",
            "            pass\n",
        );
        let collision = locate(source.as_bytes(), "m", "source.py").err().unwrap();
        let message = collision.to_string();
        assert!(message.contains("ambiguous"), "{message}");
        assert!(message.contains("Inner.Collide.m"), "{message}");
        assert!(message.contains("Other.Collide.m"), "{message}");
        assert!(locate(source.as_bytes(), "Collide", "source.py").is_err());
        let method = locate(source.as_bytes(), "Inner.Collide.m", "source.py").unwrap();
        assert_eq!(method.selection.kind, "method");
        let class = locate(source.as_bytes(), "Other.Collide", "source.py").unwrap();
        assert_eq!(class.selection.kind, "class");
    }

    #[test]
    fn ordinary_assignments_lambdas_and_fields_are_not_discovered() {
        let source = concat!(
            "x = 1\n",
            "y: int = 2\n",
            "handler = lambda value: value\n",
            "class C:\n",
            "    z = 3\n",
            "    m2 = m\n",
        );
        for name in ["x", "y", "handler", "z", "m2", "value"] {
            assert!(
                locate(source.as_bytes(), name, "source.py").is_err(),
                "{name}"
            );
        }
    }
    #[test]
    fn independent_comments_and_neighbors_stay_outside_ranges() {
        let source = b"# leading\n\ndef f():\n    pass\n\n# trailing\ndef g():\n    pass\n";
        let f = locate(source, "f", "source.py").unwrap();
        assert_eq!((f.span.start, f.span.end), (3, 4));
        let g = locate(source, "g", "source.py").unwrap();
        assert_eq!((g.span.start, g.span.end), (7, 8));
    }

    #[test]
    fn failed_parses_degrade_to_lightweight_without_owner_stripping() {
        let broken = b"def real():\n    pass\nx = (\n";
        let selected = locate(broken, "real", "source.py").unwrap();
        assert_eq!(selected.selection.mode, "lightweight");
        assert_eq!(selected.selection.language, "python");
        assert!(selected.notice.unwrap().contains("parse failed"));
        assert!(locate(broken, "Store.real", "source.py").is_err());
    }
}
