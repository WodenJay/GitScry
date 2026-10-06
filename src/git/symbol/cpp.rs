//! Bundled C++ extraction. Trees and grammar details never escape the locator.
use super::{Location, Selection, Span};
use tree_sitter::{Node, Parser};

pub(super) fn extract(source: &[u8]) -> Result<Vec<Location>, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .map_err(|error| format!("C++ grammar unavailable: {error}"))?;
    let tree = parser.parse(source, None).ok_or("C++ parse failed")?;
    let root = tree.root_node();
    if root.has_error() || invalid(root) {
        return Err("C++ parse failed (error or missing node)".to_owned());
    }
    let mut locations = Vec::new();
    collect(root, source, &[], None, &mut locations);
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
// pointers, arrays, parentheses, qualification, and parameter lists.
fn declarator(node: Node<'_>, source: &[u8]) -> Option<(String, usize, usize)> {
    match node.kind() {
        "identifier" | "field_identifier" | "destructor_name" | "operator_name" => {
            let line = node.start_position().row + 1;
            let column = node.start_position().column;
            Some((text(node, source).to_owned(), line, column))
        }
        "qualified_identifier" => qualified_name(node, source),
        "pointer_declarator"
        | "parenthesized_declarator"
        | "array_declarator"
        | "init_declarator"
        | "attributed_declarator"
        | "reference_declarator"
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

fn has_const_qualifier(declaration: Node<'_>, source: &[u8]) -> bool {
    let mut cursor = declaration.walk();
    declaration
        .children(&mut cursor)
        .take_while(|child| child.kind() != "init_declarator" && child.kind() != "identifier")
        .any(|child| child.kind() == "type_qualifier" && text(child, source) == "const")
}

// A body-bearing node is a definition; a specifier without one is only a
// type use, such as a parameter or cast.
fn is_definition(specifier: Node<'_>) -> bool {
    specifier.child_by_field_name("body").is_some_and(|body| {
        matches!(
            body.kind(),
            "compound_statement"
                | "field_declaration_list"
                | "field_initializer_list"
                | "enumerator_list"
        )
    })
}

fn collect(
    node: Node<'_>,
    source: &[u8],
    scope: &[String],
    owner: Option<&str>,
    locations: &mut Vec<Location>,
) {
    match node.kind() {
        "namespace_definition" => {
            // An anonymous namespace contributes no name segment; declarations
            // inside it keep the outer scope.
            let inner = match node
                .child_by_field_name("name")
                .and_then(|name| namespace_segments(name, source))
            {
                Some(segments) => owned_scope(scope, &segments),
                None => scope.to_vec(),
            };
            if let Some(body) = node.child_by_field_name("body") {
                collect_named_children(body, source, &inner, None, locations);
            }
        }
        "function_definition" => {
            // A standalone prototype is not an independent function target;
            // parameter names are not declarations of their own.
            if let Some(decl) = node.child_by_field_name("declarator") {
                let kind = if owner.is_some() {
                    "method"
                } else {
                    "function"
                };
                if let Some((name, line, column)) = declarator(decl, source) {
                    push(node, name, scope, line, column, kind, locations);
                }
            }
        }
        "field_declaration" => {}
        "declaration" => {
            // Out-of-class definitions and legacy constants/variables: a
            // declarator chain containing a function declarator is a function
            // definition target; otherwise each declarator is a constant or
            // explicit variable. Constructors and destructors only count
            // inside an owner, where they are methods; a prototype carries
            // the class name as its identifier but has no body, and a
            // declaration qualified with an owner segment never is one.
            // Out-of-class definitions and legacy constants/variables: each
            // declarator is a constant or explicit variable. A declarator
            // chain containing a function declarator is a prototype, never a
            // variable target, the way the C module treats it.
            let legacy = if has_const_qualifier(node, source) {
                "constant"
            } else {
                "variable"
            };
            for decl in declarator_nodes(node) {
                if !has_function_declarator(decl)
                    && let Some((name, line, column)) = declarator(decl, source)
                {
                    push(node, name, scope, line, column, legacy, locations);
                }
            }
        }
        "struct_specifier" | "class_specifier" | "union_specifier" | "enum_specifier" => {
            if is_definition(node)
                && let Some(name) = node.child_by_field_name("name")
                && let Some(segment) = type_name(name, source)
            {
                // "struct_specifier" | "class_specifier" | "union_specifier" |
                // "enum_specifier" always split into a non-empty first segment.
                let kind = node.kind().split('_').next().unwrap_or_default();
                let inner = owned_scope(scope, &[segment.to_owned()]);
                push(
                    node,
                    segment.to_owned(),
                    scope,
                    name.start_position().row + 1,
                    name.start_position().column,
                    kind,
                    locations,
                );
                collect_named_children(
                    node.child_by_field_name("body").unwrap(),
                    source,
                    &inner,
                    Some(&inner.join("::")),
                    locations,
                );
            }
        }
        "template_declaration" => {
            // The template wrapper contributes no name; the templated
            // declaration itself is the target.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor).skip(1) {
                collect(child, source, scope, owner, locations);
            }
        }
        "alias_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                push(
                    node,
                    text(name, source).to_owned(),
                    scope,
                    name.start_position().row + 1,
                    name.start_position().column,
                    "type",
                    locations,
                );
            }
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect(child, source, scope, owner, locations);
            }
        }
    }
}

fn collect_named_children(
    node: Node<'_>,
    source: &[u8],
    scope: &[String],
    owner: Option<&str>,
    locations: &mut Vec<Location>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect(child, source, scope, owner, locations);
    }
}

fn owned_scope(scope: &[String], segments: &[String]) -> Vec<String> {
    let mut inner = scope.to_vec();
    inner.extend(segments.iter().cloned());
    inner
}

fn declarator_nodes(declaration: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = declaration.walk();
    declaration
        .children_by_field_name("declarator", &mut cursor)
        .collect()
}

fn name_text<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    match node.kind() {
        "namespace_identifier" | "type_identifier" => Some(text(node, source)),
        _ => None,
    }
}

// "namespace a::b" nests through a nested_namespace_specifier; a plain name
// is a single segment.
fn namespace_segments(node: Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    match node.kind() {
        "nested_namespace_specifier" => {
            let mut cursor = node.walk();
            Some(
                node.named_children(&mut cursor)
                    .filter_map(|child| name_text(child, source))
                    .map(str::to_owned)
                    .collect(),
            )
        }
        _ => name_text(node, source).map(|segment| vec![segment.to_owned()]),
    }
}

// A templated name such as "Holder<int>" keeps only its primary identifier.
fn type_name<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    match node.kind() {
        "template_type" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .find(|child| child.kind() == "type_identifier")
                .and_then(|child| name_text(child, source))
        }
        _ => name_text(node, source),
    }
}

// The identifier declared by a qualified chain, with the owning segments in
// front: "outer::inner::lone" or "net::Widget::~Widget".
fn qualified_name(node: Node<'_>, source: &[u8]) -> Option<(String, usize, usize)> {
    let mut segments = match node.child_by_field_name("scope") {
        Some(scope) => match scope.kind() {
            "qualified_identifier" => qualified_name(scope, source)?.0,
            "template_type" => type_name(scope, source)?.to_owned(),
            _ => name_text(scope, source)?.to_owned(),
        },
        None => String::new(),
    };
    let final_node = node.child_by_field_name("name")?;
    let (final_name, line, column) = declarator(final_node, source)?;
    segments.push_str("::");
    segments.push_str(&final_name);
    Some((segments, line, column))
}

fn push(
    node: Node<'_>,
    name: String,
    scope: &[String],
    identifier_line: usize,
    identifier_column: usize,
    kind: &str,
    locations: &mut Vec<Location>,
) {
    let start = node.start_position().row + 1;
    let end = node.end_position().row + usize::from(node.end_position().column > 0);
    let mut qualified = scope.to_vec();
    qualified.push(name.clone());
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
            language: "cpp".to_owned(),
            mode: "structured".to_owned(),
        },
        identifier_line,
        identifier_column,
        span: Span { start, end },
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
                    location.selection.qualified_name,
                    location.selection.kind,
                    location.span.start,
                    location.span.end,
                )
            })
            .collect()
    }

    #[test]
    fn finds_functions_namespaces_and_named_types_with_bodies() {
        let source = b"namespace net {\nint compute(int a) {\n    return a;\n}\nint compute(int a);\nstruct Point {\n    int x;\n};\nclass C { };\nunion U { int i; };\nenum E { A };\nenum class M { On };\nint plain = 3;\nconst int LIMIT = 4;\n}\n";
        assert_eq!(
            summaries(source),
            vec![
                ("net::compute".to_owned(), "function".to_owned(), 2, 4),
                ("net::Point".to_owned(), "struct".to_owned(), 6, 8),
                ("net::C".to_owned(), "class".to_owned(), 9, 9),
                ("net::U".to_owned(), "union".to_owned(), 10, 10),
                ("net::E".to_owned(), "enum".to_owned(), 11, 11),
                ("net::M".to_owned(), "enum".to_owned(), 12, 12),
                ("net::plain".to_owned(), "variable".to_owned(), 13, 13),
                ("net::LIMIT".to_owned(), "constant".to_owned(), 14, 14),
            ]
        );
    }

    #[test]
    fn member_and_out_of_class_definitions_are_targets_with_native_qualification() {
        let source = concat!(
            "namespace net {\n",
            "class Widget {\n",
            "public:\n",
            "    Widget(int v);\n",
            "    ~Widget();\n",
            "    int size() const { return v; }\n",
            "    int count();\n",
            "private:\n",
            "    int v;\n",
            "};\n",
            "}\n",
            "int net::Widget::size() const { return 0; }\n",
            "net::Widget::Widget(int v) : v(v) {}\n",
            "net::Widget::~Widget() {}\n",
        );
        assert_eq!(
            summaries(source.as_bytes()),
            vec![
                ("net::Widget".to_owned(), "class".to_owned(), 2, 10),
                ("net::Widget::size".to_owned(), "method".to_owned(), 6, 6),
                (
                    "net::Widget::size".to_owned(),
                    "function".to_owned(),
                    12,
                    12
                ),
                (
                    "net::Widget::Widget".to_owned(),
                    "function".to_owned(),
                    13,
                    13
                ),
                (
                    "net::Widget::~Widget".to_owned(),
                    "function".to_owned(),
                    14,
                    14
                ),
            ]
        );
    }

    #[test]
    fn templates_and_alias_declarations_are_targets_without_template_syntax_in_names() {
        let source = b"template<typename T>\nclass Holder {\npublic:\n    T get() { return item; }\nprivate:\n    T item;\n};\ntemplate<typename T>\nT Holder<T>::get() { return item; }\ntemplate<>\nclass Holder<int> { };\nusing Int = int;\n";
        assert_eq!(
            summaries(source),
            vec![
                ("Holder".to_owned(), "class".to_owned(), 2, 7),
                ("Holder::get".to_owned(), "method".to_owned(), 4, 4),
                ("Holder::get".to_owned(), "function".to_owned(), 9, 9),
                ("Holder".to_owned(), "class".to_owned(), 11, 11),
                ("Int".to_owned(), "type".to_owned(), 12, 12),
            ]
        );
        let locations = extract(source).unwrap();
        let out_of_class = &locations[2];
        assert_eq!(
            (out_of_class.identifier_line, out_of_class.identifier_column),
            (9, 13)
        );
    }

    #[test]
    fn nested_namespaces_qualify_and_anonymous_namespaces_do_not() {
        let source = b"namespace outer {\nnamespace inner {\nvoid lone() {}\n}\nnamespace {\nvoid hidden() {}\n}\nnamespace a::b { void deep() {} }\n}\nvoid outer::inner::lone() {}\n";
        assert_eq!(
            summaries(source),
            vec![
                ("outer::inner::lone".to_owned(), "function".to_owned(), 3, 3),
                ("outer::hidden".to_owned(), "function".to_owned(), 6, 6),
                ("outer::a::b::deep".to_owned(), "function".to_owned(), 8, 8),
                (
                    "outer::inner::lone".to_owned(),
                    "function".to_owned(),
                    10,
                    10
                ),
            ]
        );
    }

    #[test]
    fn prototypes_type_uses_and_member_prototypes_are_not_targets() {
        let source = b"int compute(int a);\nvoid handler(int);\nusing Callback = void (*)(int);\ntypedef int (*other)(int);\nstruct Uses { struct Inner *next; void m(); };\nint cast(int value);\nint cast(int value) { return value; }\nstruct Missing;\nenum E : int;\nvoid fwd();\n";
        assert_eq!(
            summaries(source),
            vec![
                ("Callback".to_owned(), "type".to_owned(), 3, 3),
                ("Uses".to_owned(), "struct".to_owned(), 5, 5),
                ("cast".to_owned(), "function".to_owned(), 7, 7),
            ]
        );
    }

    #[test]
    fn multiline_declarations_keep_complete_ranges_and_identifier_positions() {
        let source = b"namespace net {\nstruct Point {\n    int x;\n    int y;\n};\nstatic int\ncompute(\n    int a\n) {\n    return a;\n}\nconst char *\nmsg = \"hi\";\n}\n";
        assert_eq!(
            summaries(source),
            vec![
                ("net::Point".to_owned(), "struct".to_owned(), 2, 5),
                ("net::compute".to_owned(), "function".to_owned(), 6, 11),
                ("net::msg".to_owned(), "constant".to_owned(), 12, 13),
            ]
        );
        let locations = extract(source).unwrap();
        let compute = &locations[1];
        assert_eq!((compute.identifier_line, compute.identifier_column), (7, 0));
        let msg = locations.last().unwrap();
        assert_eq!((msg.identifier_line, msg.identifier_column), (13, 0));
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

    #[test]
    fn overloads_stay_separate_candidates_and_parse_failure_is_reported() {
        let source = b"int compute(int a) { return a; }\nint compute(double a) { return 1; }\n";
        assert_eq!(extract(source).unwrap().len(), 2);
        assert!(extract(b"int compute(int a) {\n").is_err());
    }
}
