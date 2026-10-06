//! Source-only symbol location, shared by why and regression targets.
//! Git objects and target anchors stay outside this module.

mod rust;

/// The actual source interpretation and declaration selected at a pinned revision.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Selection {
    pub(crate) input_selector: String,
    pub(crate) qualified_name: String,
    pub(crate) kind: String,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) identifier_line: usize,
    pub(crate) identifier_column: usize,
    pub(crate) language: String,
    pub(crate) mode: String,
}
use crate::app::AppError;

pub(super) struct Span {
    pub(super) start: usize,
    pub(super) end: usize,
}

/// A local declaration, with its identifier kept separate from its source range.
pub(super) struct Location {
    pub(super) name: String,
    pub(super) selection: Selection,
    pub(super) notice: Option<String>,
    /// One-based line and zero-based byte column in the original source.
    pub(super) identifier_line: usize,
    pub(super) identifier_column: usize,
    pub(super) span: Span,
}

/// Require a unique declaration; a clean structured parse is authoritative.
pub(super) fn locate_unique(content: &[u8], name: &str, path: &str) -> Result<Location, AppError> {
    if name.trim().is_empty() {
        return Err(AppError::input("symbol must not be empty"));
    }
    let (locations, notice) = declarations(content, path);
    if notice.is_some() && name.contains("::") {
        return Err(AppError::input(format!(
            "qualified symbol {name} requires structured Rust parsing in {path}: {}",
            notice.unwrap()
        )));
    }
    let mut matches = locations
        .into_iter()
        .filter(|location| {
            let qualified = &location.selection.qualified_name;
            qualified == name
                || qualified
                    .strip_suffix(name)
                    .is_some_and(|prefix| prefix.ends_with("::"))
        })
        .collect::<Vec<_>>();
    match matches.len() {
        0 => Err(AppError::input(format!(
            "symbol {name} has no supported declaration in {path} at the target revision{}",
            notice
                .map(|notice| format!("; {notice}"))
                .unwrap_or_default()
        ))),
        1 => {
            let mut location = matches.pop().unwrap();
            location.selection.input_selector = name.to_owned();
            Ok(location)
        }
        _ => Err(AppError::input(format!(
            "symbol {name} is ambiguous in {path} at the target revision; declarations: {}{}",
            matches
                .iter()
                .map(|location| format!(
                    "{} ({}, lines {}-{}, identifier {}:{}): {}",
                    location.selection.qualified_name,
                    location.selection.kind,
                    location.span.start,
                    location.span.end,
                    location.identifier_line,
                    location.identifier_column,
                    content
                        .split(|byte| *byte == b'\n')
                        .nth(location.identifier_line - 1)
                        .map(|line| String::from_utf8_lossy(line).trim().to_owned())
                        .unwrap_or_default()
                ))
                .collect::<Vec<_>>()
                .join("; "),
            notice
                .map(|notice| format!("; {notice}"))
                .unwrap_or_default()
        ))),
    }
}

pub(super) fn declaration_lines(content: &[u8], name: &str, path: &str) -> Vec<usize> {
    declarations(content, path)
        .0
        .into_iter()
        .filter_map(|location| {
            (location.selection.qualified_name == name || location.name == name)
                .then_some(location.identifier_line)
        })
        .collect()
}

/// Enumerate only the interpretation selected by the extension, never try grammars.
pub(super) fn declarations(content: &[u8], path: &str) -> (Vec<Location>, Option<String>) {
    let (language, reason) = if path.ends_with(".rs") {
        match rust::extract(content) {
            Ok(locations) => return (locations, None),
            Err(reason) => ("rust", reason),
        }
    } else {
        ("unsupported", "unsupported language".to_owned())
    };
    let notice = format!(
        "lightweight symbol selection in {path}: {reason}; structural ownership and ranges are unverified"
    );
    let locations = declaration_positions(content, path.ends_with(".rs"))
        .into_iter()
        .map(|(name, line, column)| {
            let end = symbol_span_end(content, line, path.ends_with(".rs"));
            Location {
                selection: Selection {
                    input_selector: name.clone(),
                    qualified_name: name.clone(),
                    kind: "declaration".to_owned(),
                    start_line: line,
                    end_line: end,
                    identifier_line: line,
                    identifier_column: column,
                    language: language.to_owned(),
                    mode: "lightweight".to_owned(),
                },
                name,
                identifier_line: line,
                identifier_column: column,
                span: Span { start: line, end },
                notice: Some(notice.clone()),
            }
        })
        .collect();
    (locations, Some(notice))
}

fn declaration_positions(content: &[u8], rust_source: bool) -> Vec<(String, usize, usize)> {
    let mut positions = Vec::new();
    let mut state = BraceState::Code;
    for (index, line) in content.split(|byte| *byte == b'\n').enumerate() {
        for start in scan_line(line, &mut state, rust_source).declaration_starts {
            if let Some((name, column)) = declaration(&line[start..]) {
                positions.push((name.to_owned(), index + 1, start + column));
            }
        }
    }
    positions
}

/// Ignore only the declaration's identifier and outer indentation, not body tokens.
pub(super) fn identity(content: &[u8], location: &Location) -> Vec<u8> {
    let mut result = Vec::new();
    for (index, line) in content
        .split(|byte| *byte == b'\n')
        .enumerate()
        .skip(location.span.start - 1)
        .take(location.span.end - location.span.start + 1)
    {
        if index + 1 == location.identifier_line {
            let column = location.identifier_column;
            result.extend_from_slice(line[..column].trim_ascii_start());
            result.extend_from_slice(b"<symbol>");
            result.extend_from_slice(line[column + location.name.len()..].trim_ascii_end());
        } else {
            result.extend_from_slice(line.trim_ascii());
        }
        result.push(b'\n');
    }
    result
}

/// Read the name immediately after a supported declaration keyword, not an arbitrary
/// occurrence of the requested name. Stop at expression punctuation so initializer
/// functions and textual mentions cannot become declarations of their own.
fn declaration(line: &[u8]) -> Option<(&str, usize)> {
    let mut index = leading_indent(line);
    while index < line.len() {
        let start = index;
        while line.get(index).is_some_and(|byte| is_symbol_byte(*byte)) {
            index += 1;
        }
        if start == index {
            return None;
        }
        let token = &line[start..index];
        if token == b"const" && line[index..].trim_ascii_start().starts_with(b"fn ") {
            index += line[index..]
                .iter()
                .take_while(|byte| byte.is_ascii_whitespace())
                .count();
            continue;
        }
        if token == b"function" && line.get(index) == Some(&b'*') {
            index += 1;
        }
        if matches!(
            token,
            b"fn"
                | b"func"
                | b"function"
                | b"def"
                | b"class"
                | b"struct"
                | b"enum"
                | b"trait"
                | b"interface"
                | b"type"
                | b"const"
                | b"let"
                | b"var"
                | b"module"
                | b"namespace"
                | b"macro"
                | b"impl"
        ) {
            index += line[index..]
                .iter()
                .take_while(|byte| byte.is_ascii_whitespace())
                .count();
            let name_start = index;
            while line.get(index).is_some_and(|byte| is_symbol_byte(*byte)) {
                index += 1;
            }
            if index == name_start || line[name_start].is_ascii_digit() {
                return None;
            }
            let after = line[index..].trim_ascii_start();
            if !matches!(after.first(), Some(b'(' | b'{' | b':' | b'=' | b'<')) {
                return None;
            }
            return Some((
                std::str::from_utf8(&line[name_start..index]).ok()?,
                name_start,
            ));
        }
        // Rust visibility can contain the same identifier as the declaration.
        if token == b"pub" && line.get(index) == Some(&b'(') {
            index += line[index..].iter().position(|byte| *byte == b')')? + 1;
        }
        if token == b"extern" {
            let abi_start = index
                + line[index..]
                    .iter()
                    .take_while(|byte| byte.is_ascii_whitespace())
                    .count();
            if line.get(abi_start) == Some(&b'"') {
                index = abi_start
                    + 1
                    + line[abi_start + 1..]
                        .iter()
                        .position(|byte| *byte == b'"')?
                    + 1;
            }
        }
        if !line.get(index).is_some_and(u8::is_ascii_whitespace) {
            return None;
        }
        index += line[index..]
            .iter()
            .take_while(|byte| byte.is_ascii_whitespace())
            .count();
    }
    None
}

fn is_symbol_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn symbol_span_end(content: &[u8], start: usize, rust_source: bool) -> usize {
    let lines = content.split(|byte| *byte == b'\n').collect::<Vec<_>>();
    let start_index = start.saturating_sub(1).min(lines.len().saturating_sub(1));
    let start_line = lines.get(start_index).copied().unwrap_or_default();
    let start_indent = leading_indent(start_line);
    let indentation_body = start_line.trim_ascii_end().ends_with(b":");
    let mut state = BraceState::Code;
    let scan = scan_line(start_line, &mut state, rust_source);
    let mut braces = scan.braces;
    if (scan.has_brace || (scan.statement_end && !indentation_body)) && braces <= 0 {
        return start;
    }
    for (index, line) in lines.iter().enumerate().skip(start_index + 1) {
        if braces <= 0
            && !line.is_empty()
            && leading_indent(line) <= start_indent
            && matches!(state, BraceState::Code)
            && is_declaration_start(line)
        {
            return index.max(start_index);
        }
        let scan = scan_line(line, &mut state, rust_source);
        braces += scan.braces;
        if (scan.has_brace || (scan.statement_end && !indentation_body)) && braces <= 0 {
            return index + 1;
        }
    }
    lines.len().max(start)
}

fn leading_indent(line: &[u8]) -> usize {
    line.iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

#[derive(Clone, Copy)]
enum BraceState {
    Code,
    String { quote: u8, escaped: bool },
    RawString(usize),
    BlockComment(usize),
}

struct LineScan {
    braces: i32,
    has_brace: bool,
    statement_end: bool,
    declaration_starts: Vec<usize>,
}

fn scan_line(line: &[u8], state: &mut BraceState, rust_source: bool) -> LineScan {
    let mut scan = LineScan {
        braces: 0,
        has_brace: false,
        statement_end: false,
        declaration_starts: if matches!(state, BraceState::Code) {
            vec![0]
        } else {
            Vec::new()
        },
    };
    let mut index = 0;
    while index < line.len() {
        match *state {
            BraceState::Code => {
                if let Some((quote, hashes)) = raw_string_start(line, index) {
                    *state = BraceState::RawString(hashes);
                    index = quote + 1;
                    continue;
                }
                if line[index] == b'/' && line.get(index + 1) == Some(&b'/') {
                    break;
                }
                if line[index] == b'/' && line.get(index + 1) == Some(&b'*') {
                    *state = BraceState::BlockComment(1);
                    index += 2;
                    continue;
                }
                match line[index] {
                    b'"' | b'`' => {
                        *state = BraceState::String {
                            quote: line[index],
                            escaped: false,
                        }
                    }
                    b'\'' => {
                        if let Some(end) = char_literal_end(line, index) {
                            index = end;
                            continue;
                        } else if !rust_source || !is_rust_lifetime(line, index) {
                            *state = BraceState::String {
                                quote: b'\'',
                                escaped: false,
                            };
                        }
                    }
                    b'{' => {
                        scan.braces += 1;
                        scan.has_brace = true;
                        scan.declaration_starts.push(index + 1);
                    }
                    b'}' => {
                        scan.braces -= 1;
                        scan.has_brace = true;
                        scan.declaration_starts.push(index + 1);
                    }
                    b';' => {
                        scan.statement_end = true;
                        scan.declaration_starts.push(index + 1);
                    }
                    _ => {}
                }
            }
            BraceState::String { quote, escaped } => {
                if escaped {
                    *state = BraceState::String {
                        quote,
                        escaped: false,
                    };
                } else if line[index] == b'\\' {
                    *state = BraceState::String {
                        quote,
                        escaped: true,
                    };
                } else if line[index] == quote {
                    *state = BraceState::Code;
                }
            }
            BraceState::RawString(hashes) => {
                if line[index] == b'"' {
                    let suffix = &line[index + 1..];
                    let closing_hashes = suffix.iter().take_while(|byte| **byte == b'#').count();
                    if closing_hashes == hashes {
                        *state = BraceState::Code;
                        index += 1 + hashes;
                        continue;
                    }
                }
            }
            BraceState::BlockComment(depth) => {
                if line[index] == b'/' && line.get(index + 1) == Some(&b'*') {
                    *state = BraceState::BlockComment(depth + 1);
                    index += 2;
                    continue;
                }
                if line[index] == b'*' && line.get(index + 1) == Some(&b'/') {
                    if depth == 1 {
                        *state = BraceState::Code;
                    } else {
                        *state = BraceState::BlockComment(depth - 1);
                    }
                    index += 2;
                    continue;
                }
            }
        }
        index += 1;
    }
    scan
}

fn raw_string_start(line: &[u8], index: usize) -> Option<(usize, usize)> {
    let hashes_start = match line.get(index..index + 2) {
        Some(b"br") => index + 2,
        _ if line.get(index) == Some(&b'r') => index + 1,
        _ => return None,
    };
    let mut quote = hashes_start;
    while line.get(quote) == Some(&b'#') {
        quote += 1;
    }
    (line.get(quote) == Some(&b'"')).then_some((quote, quote - hashes_start))
}

fn char_literal_end(line: &[u8], start: usize) -> Option<usize> {
    let content = start + 1;
    let first = *line.get(content)?;
    let closing = if first == b'\\' {
        match *line.get(content + 1)? {
            b'x' => content + 4,
            b'u' if line.get(content + 2) == Some(&b'{') => line
                .iter()
                .enumerate()
                .skip(content + 3)
                .find(|(_, byte)| **byte == b'}')
                .map(|(index, _)| index + 1)?,
            b'n' | b'r' | b't' | b'0' | b'\\' | b'\'' | b'"' => content + 2,
            _ => return None,
        }
    } else {
        let width = match first {
            0x00..=0x7f => 1,
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => return None,
        };
        content + width
    };
    (line.get(closing) == Some(&b'\'')).then_some(closing + 1)
}

fn is_rust_lifetime(line: &[u8], start: usize) -> bool {
    let mut end = start + 1;
    let Some(first) = line.get(end) else {
        return false;
    };
    if !first.is_ascii_alphabetic() && *first != b'_' {
        return false;
    }
    end += 1;
    while let Some(byte) = line.get(end) {
        if byte.is_ascii_alphanumeric() || *byte == b'_' {
            end += 1;
        } else {
            break;
        }
    }

    let previous = line[..start]
        .iter()
        .rev()
        .find(|byte| !byte.is_ascii_whitespace())
        .copied();
    matches!(previous, Some(b'&' | b'<' | b',' | b':' | b'+'))
        || matches!(line.get(end).copied(), Some(b'>' | b',' | b':'))
}

fn is_declaration_start(line: &[u8]) -> bool {
    declaration(line).is_some()
}

#[cfg(test)]
mod tests {
    use super::declaration_lines;
    use super::locate_unique as locate;

    #[test]
    fn qualified_rust_selection_tracks_attributes_and_identifier_separately() {
        let source = b"// outside\nmod net {\n    /// outside\n    #[inline]\n    pub fn parse<T>(\n        value: T,\n    ) -> T {\n        value\n    }\n    fn neighbor() {}\n}\n";
        let selected = locate(source, "net::parse", "source.rs").unwrap();
        assert_eq!((selected.span.start, selected.span.end), (4, 9));
        assert_eq!(
            (selected.identifier_line, selected.identifier_column),
            (5, 11)
        );
    }

    #[test]
    fn rust_owners_suffixes_and_impl_collisions_are_structural() {
        let source = b"mod outer { mod inner { struct Item<T>(T); impl<T> Item<T> { pub fn read(&self) {} } trait Access { fn required(); fn defaulted() {} } impl Access for Item<u8> { fn read(&self) {} } } }";
        let collision = locate(source, "inner::Item::read", "source.rs")
            .err()
            .unwrap()
            .to_string();
        assert!(collision.contains("ambiguous"));
        assert!(collision.contains("outer::inner::Item::read"));
        assert!(collision.contains("method"));
        assert!(collision.contains("impl<T>"));
        for selector in ["Access", "inner::Access", "outer::inner::Access"] {
            assert_eq!(
                locate(source, selector, "source.rs")
                    .unwrap()
                    .selection
                    .kind,
                "trait"
            );
        }
        assert!(locate(source, "required", "source.rs").is_err());
        assert!(locate(source, "nner::Access", "source.rs").is_err());
        assert!(locate(source, "access", "source.rs").is_err());
        assert!(locate(source, "Item<T>::read", "source.rs").is_err());
        assert_eq!(
            locate(source, "Access::defaulted", "source.rs")
                .unwrap()
                .selection
                .kind,
            "method"
        );
    }

    #[test]
    fn clean_parses_are_authoritative_and_degradation_is_explicit() {
        let clean = b"trait Trait { fn signature(); } fn real() {}";
        assert!(locate(clean, "signature", "source.rs").is_err());
        assert!(locate(clean, "missing", "source.rs").is_err());
        for broken in [
            b"fn real() {}\nfn broken(\n".as_slice(),
            b"fn real() {}\nfn broken() {\n".as_slice(),
        ] {
            let selected = locate(broken, "real", "source.rs").unwrap();
            assert_eq!(selected.selection.mode, "lightweight");
            assert!(selected.notice.unwrap().contains("parse failed"));
            assert!(locate(broken, "owner::real", "source.rs").is_err());
        }
        let unknown = locate(b"fn real() {}", "real", "source.custom").unwrap();
        assert_eq!(unknown.selection.language, "unsupported");
        assert_eq!(unknown.selection.mode, "lightweight");
        assert!(unknown.notice.unwrap().contains("unsupported language"));
        assert!(
            locate(
                b"mod owner { fn real() {} }",
                "owner::real",
                "source.custom"
            )
            .is_err()
        );
        assert!(locate(b"fn real() {} fn real() {}", "real", "source.rs").is_err());
        assert!(locate(b"fn real() {}\nfn real() {}\n@", "real", "source.rs").is_err());
        assert!(locate(b"// fn mention() {}\nfn other() {}", "mention", "source.rs").is_err());
    }

    #[test]
    fn rust_named_types_and_explicit_variables_use_complete_syntax_ranges() {
        let source = b"#[doc = \"syntactic docs\"]\npub struct Data<T> {\n value: T,\n}\nmod empty;\ntype Alias = Data<u8>;\nstatic FLAG: bool = true;\nfn holder() {\n let value: usize = {\n  2\n };\n}\n";
        for (name, kind, span) in [
            ("Data", "struct", (1, 4)),
            ("empty", "module", (5, 5)),
            ("Alias", "type", (6, 6)),
            ("FLAG", "static", (7, 7)),
            ("holder::value", "variable", (9, 11)),
        ] {
            let selected = locate(source, name, "source.rs").unwrap();
            assert_eq!(selected.selection.kind, kind);
            assert_eq!((selected.span.start, selected.span.end), span);
            assert_eq!(selected.selection.mode, "structured");
        }
    }
    #[test]
    fn legitimate_constants_variables_and_function_values_keep_useful_ranges() {
        for (path, source, name, expected) in [
            (
                "source.rs",
                "pub const LIMIT: usize = 3;\nconst NEXT: usize = 4;",
                "LIMIT",
                (1, 1),
            ),
            (
                "source.ts",
                "export let count: number = 3;\nconst next = 4;",
                "count",
                (1, 1),
            ),
            (
                "source.js",
                "var count = 3;\nvar next = 4;",
                "count",
                (1, 1),
            ),
            (
                "source.ts",
                "export const parse = (value: string) => {\n    return value.trim();\n};\nconst next = 4;",
                "parse",
                (1, 3),
            ),
            (
                "source.js",
                "let parse = function(value) {\n    return value;\n};\nlet next = 4;",
                "parse",
                (1, 3),
            ),
            (
                "source.js",
                "var parse = value => value;\nvar next = 4;",
                "parse",
                (1, 1),
            ),
        ] {
            let span = super::locate_unique(source.as_bytes(), name, path).unwrap();
            assert_eq!(
                (span.span.start, span.span.end),
                expected,
                "{path}: {source}"
            );
        }
    }
    #[test]
    fn modified_functions_keep_their_declaration_ranges() {
        for source in [
            "pub const fn parse() {\n    1\n}\n",
            "pub unsafe extern \"C\" fn parse() {\n    1\n}\n",
            "export function* parse() {\n    yield 1;\n}\n",
        ] {
            let location = locate(source.as_bytes(), "parse", "source.rs").unwrap();
            assert_eq!((location.span.start, location.span.end), (1, 3));
        }
    }

    #[test]
    fn declaration_identifier_is_not_an_earlier_prefix_mention() {
        let source = b"pub(in parse) fn parse() {}\n";
        let renamed = b"pub(in parse) fn renamed() {}\n";
        let original = super::locate_unique(source, "parse", "source.rs").unwrap();
        let renamed_span = super::locate_unique(renamed, "renamed", "source.rs").unwrap();
        assert_eq!(
            super::identity(source, &original),
            super::identity(renamed, &renamed_span),
        );
        assert_eq!(
            super::declaration_lines(source, "parse", "source.rs"),
            vec![1]
        );
    }

    #[test]
    fn unbraced_declarations_end_before_the_next_declaration() {
        let source = b"const FOO: usize = 1;\nconst BAR: usize = 2;";
        let span = locate(source, "FOO", "source.rs").unwrap();
        assert_eq!((span.span.start, span.span.end), (1, 1));
    }

    #[test]
    fn symbol_span_end_ignores_javascript_object_string_braces() {
        let source = concat!(
            "function parse() {\n",
            "    const object = { value: 'closer } stays string' };\n",
            "\n",
            "\n",
            "\n",
            "\n",
            "\n",
            "\n",
            "\n",
            "\n",
            "    const result = 1;\n",
            "}\n",
        );
        let span = locate(source.as_bytes(), "parse", "source.js").unwrap();
        assert_eq!((span.span.start, span.span.end), (1, 12));
    }

    #[test]
    fn declaration_wins_over_mentions_and_longer_identifiers() {
        let source = b"// parse handles input\nfn parse_more() {}\nfn parse() {\n    parse_more();\n}\nfn next() {}\n";
        let span = locate(source, "parse", "source.rs").unwrap();
        assert_eq!((span.span.start, span.span.end), (3, 5));
    }

    #[test]
    fn rust_span_ignores_raw_strings_nested_comments_chars_and_lifetimes() {
        let source = concat!(
            "fn parse<'a>(input: &'a str) -> &'a str {\n",
            "    let raw = br##\"closing }\n",
            "        still } raw\"##;\n",
            "    /* outer } /* inner } */ still } */\n",
            "    let character = '\\u{7d}';\n",
            "    input\n",
            "}\n",
            "fn next() {}\n",
        );
        let span = locate(source.as_bytes(), "parse", "source.rs").unwrap();
        assert_eq!((span.span.start, span.span.end), (1, 7));
    }

    #[test]
    fn indentation_bounds_python_symbols() {
        let source = b"def parse():\n    return 1\n\ndef next():\n    return 2\n";
        let span = locate(source, "parse", "source.py").unwrap();
        assert_eq!((span.span.start, span.span.end), (1, 3));
    }

    #[test]
    fn mention_only_symbols_are_rejected() {
        assert!(locate(b"parse(value)\n", "parse", "source.py").is_err());
    }

    #[test]
    fn every_same_line_declaration_is_considered() {
        let source = b"const other = 1; const target = 2;\n";
        let location = locate(source, "target", "source.js").unwrap();
        assert_eq!(location.identifier_column, 23);
        assert_eq!((location.span.start, location.span.end), (1, 1));
        assert!(
            locate(
                b"let target = 1; { let target = 2; }\n",
                "target",
                "source.js"
            )
            .is_err()
        );
        assert_eq!(
            declaration_lines(b"fn target() {} fn target() {}\n", "target", "source.rs"),
            vec![1, 1]
        );
    }

    #[test]
    fn declaration_shapes_in_strings_and_comments_are_not_candidates() {
        for source in [
            "const other = \"; const target = 2;\";\n",
            "/*\nfn target() {}\n*/\n",
            "const text = `\nfunction target() {}\n`;\n",
        ] {
            assert!(locate(source.as_bytes(), "target", "source.js").is_err());
        }
    }

    #[test]
    fn statement_ends_preserve_complete_ranges() {
        let source = b"const target = [\n    1,\n    2,\n];\nconsole.log(target);\n";
        let location = locate(source, "target", "source.js").unwrap();
        assert_eq!((location.span.start, location.span.end), (1, 4));
        let source = b"def target():\n    first(); second()\n    finish()\ndef next():\n    pass\n";
        let location = locate(source, "target", "source.py").unwrap();
        assert_eq!((location.span.start, location.span.end), (1, 3));
    }

    #[test]
    fn invalid_symbols_keep_the_existing_input_errors() {
        for (name, message) in [
            ("  ", "symbol must not be empty"),
            (
                "parse",
                "symbol parse has no supported declaration in source.rs at the target revision",
            ),
        ] {
            let error = locate(b"fn parse_more() {}\n", name, "source.rs")
                .err()
                .unwrap();
            assert_eq!(error.to_string(), message);
        }
    }
}
