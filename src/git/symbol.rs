//! Source-only symbol location, shared by why and regression targets.
//! Git objects and target anchors stay outside this module.

use crate::app::AppError;

pub(super) struct Span {
    pub(super) start: usize,
    pub(super) end: usize,
}

/// A local declaration, with its identifier kept separate from its source range.
pub(super) struct Location {
    pub(super) name: String,
    /// One-based line and zero-based byte column in the original source.
    pub(super) identifier_line: usize,
    pub(super) identifier_column: usize,
    pub(super) span: Span,
}

/// Require exactly one supported declaration; never fall back to a text mention.
/// This lightweight locator does not claim compiler-level symbol resolution.
pub(super) fn locate_unique(content: &[u8], name: &str, path: &str) -> Result<Location, AppError> {
    if name.trim().is_empty() {
        return Err(AppError::input("symbol must not be empty"));
    }
    let mut matches = declarations(content, path)
        .into_iter()
        .filter(|declaration| declaration.name == name)
        .collect::<Vec<_>>();
    match matches.len() {
        0 => Err(AppError::input(format!(
            "symbol {name} has no supported declaration in {path} at the target revision"
        ))),
        1 => Ok(matches.pop().unwrap()),
        _ => Err(AppError::input(format!(
            "symbol {name} is ambiguous in {path} at the target revision; declarations found at lines {}",
            matches
                .iter()
                .map(|location| location.identifier_line.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

pub(super) fn declaration_lines(content: &[u8], name: &str) -> Vec<usize> {
    content
        .split(|byte| *byte == b'\n')
        .enumerate()
        .filter_map(|(index, line)| {
            let (declared, _) = declaration(line)?;
            (declared == name).then_some(index + 1)
        })
        .collect()
}

/// Enumerate supported declarations for conservative same-change correspondence.
pub(super) fn declarations(content: &[u8], path: &str) -> Vec<Location> {
    content
        .split(|byte| *byte == b'\n')
        .enumerate()
        .filter_map(|(index, line)| {
            let (name, column) = declaration(line)?;
            Some(Location {
                name: name.to_owned(),
                identifier_line: index + 1,
                identifier_column: column,
                span: Span {
                    start: index + 1,
                    end: symbol_span_end(content, index + 1, path.ends_with(".rs")),
                },
            })
        })
        .collect()
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
    let mut state = BraceState::Code;
    let (mut braces, has_brace) = brace_delta(start_line, &mut state, rust_source);
    if has_brace && braces <= 0 {
        return start;
    }
    if braces > 0 {
        for (index, line) in lines.iter().enumerate().skip(start_index + 1) {
            let (delta, has_brace) = brace_delta(line, &mut state, rust_source);
            braces += delta;
            if has_brace && braces <= 0 {
                return index + 1;
            }
        }
    }
    for (index, line) in lines.iter().enumerate().skip(start_index + 1) {
        if line.is_empty() || leading_indent(line) > start_indent {
            continue;
        }
        if is_declaration_start(line) {
            return index.max(start_index);
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

fn brace_delta(line: &[u8], state: &mut BraceState, rust_source: bool) -> (i32, bool) {
    let mut balance = 0;
    let mut has_brace = false;
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
                        balance += 1;
                        has_brace = true;
                    }
                    b'}' => {
                        balance -= 1;
                        has_brace = true;
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
    (balance, has_brace)
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
    use super::locate_unique as locate;

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
        assert_eq!(super::declaration_lines(source, "parse"), vec![1]);
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
