//! Source-only symbol location, shared by why and regression targets.
//! Git objects and target anchors stay outside this module.

use crate::app::AppError;

pub(super) struct Span {
    pub(super) start: usize,
    pub(super) end: usize,
}

/// Prefer a declaration over a mention, then bound its span using source heuristics.
/// Line numbers are one-based; this does not claim compiler-level symbol resolution.
pub(super) fn locate(content: &[u8], name: &str, path: &str) -> Result<Span, AppError> {
    if name.trim().is_empty() {
        return Err(AppError::input("symbol must not be empty"));
    }
    let lines = content
        .split(|byte| *byte == b'\n')
        .enumerate()
        .filter_map(|(index, line)| contains_symbol(line, name.as_bytes()).then_some(index + 1))
        .collect::<Vec<_>>();
    let start = lines
        .iter()
        .copied()
        .find(|line| {
            let bytes = content
                .split(|byte| *byte == b'\n')
                .nth(line.saturating_sub(1))
                .unwrap_or_default();
            is_declaration(bytes, name.as_bytes())
        })
        .or_else(|| lines.first().copied())
        .ok_or_else(|| {
            AppError::input(format!(
                "symbol {name} was not found in {path} at the target revision"
            ))
        })?;
    Ok(Span {
        start,
        end: symbol_span_end(content, start, path.ends_with(".rs")),
    })
}

/// Require one actual declaration; unlike `locate`, never fall back to a mention.
pub(super) fn locate_unique(content: &[u8], name: &str, path: &str) -> Result<Span, AppError> {
    if name.trim().is_empty() {
        return Err(AppError::input("symbol must not be empty"));
    }
    let declarations = declaration_lines(content, name);
    match declarations.as_slice() {
        [] => Err(AppError::input(format!(
            "symbol {name} has no supported declaration in {path} at the target revision"
        ))),
        [start] => Ok(Span {
            start: *start,
            end: symbol_span_end(content, *start, path.ends_with(".rs")),
        }),
        _ => Err(AppError::input(format!(
            "symbol {name} is ambiguous in {path} at the target revision; declarations found at lines {}",
            declarations
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

pub(super) fn declaration_lines(content: &[u8], name: &str) -> Vec<usize> {
    content
        .split(|byte| *byte == b'\n')
        .enumerate()
        .filter_map(|(index, line)| is_declaration(line, name.as_bytes()).then_some(index + 1))
        .collect()
}

/// Enumerate supported declarations for conservative same-change correspondence.
pub(super) fn declarations(content: &[u8], path: &str) -> Vec<(String, Span)> {
    let mut declarations = Vec::new();
    for (index, line) in content.split(|byte| *byte == b'\n').enumerate() {
        for token in line
            .split(|byte| !is_symbol_byte(*byte))
            .filter(|token| !token.is_empty())
        {
            if is_declaration(line, token)
                && let Ok(name) = std::str::from_utf8(token)
            {
                declarations.push((
                    name.to_owned(),
                    Span {
                        start: index + 1,
                        end: symbol_span_end(content, index + 1, path.ends_with(".rs")),
                    },
                ));
            }
        }
    }
    declarations
}

/// Ignore only the declaration's identifier and outer indentation, not body tokens.
pub(super) fn identity(content: &[u8], name: &str, span: &Span) -> Vec<u8> {
    let mut result = Vec::new();
    for (offset, line) in content
        .split(|byte| *byte == b'\n')
        .skip(span.start - 1)
        .take(span.end - span.start + 1)
        .enumerate()
    {
        let line = line.trim_ascii();
        if offset == 0 {
            let index = line
                .windows(name.len())
                .enumerate()
                .find(|(index, token)| {
                    *token == name.as_bytes()
                        && (*index == 0 || !is_symbol_byte(line[*index - 1]))
                        && (index + name.len() == line.len()
                            || !is_symbol_byte(line[index + name.len()]))
                })
                .unwrap()
                .0;
            result.extend_from_slice(&line[..index]);
            result.extend_from_slice(b"<symbol>");
            result.extend_from_slice(&line[index + name.len()..]);
        } else {
            result.extend_from_slice(line);
        }
        result.push(b'\n');
    }
    result
}
fn contains_symbol(line: &[u8], symbol: &[u8]) -> bool {
    if symbol.is_empty() {
        return false;
    }
    line.windows(symbol.len())
        .enumerate()
        .any(|(index, window)| {
            window == symbol
                && (index == 0 || !is_symbol_byte(line[index - 1]))
                && (index + symbol.len() == line.len()
                    || !is_symbol_byte(line[index + symbol.len()]))
        })
}

fn is_declaration(line: &[u8], symbol: &[u8]) -> bool {
    let trimmed = line
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .collect::<Vec<_>>();
    if trimmed.starts_with(b"//")
        || trimmed.starts_with(b"#")
        || trimmed.starts_with(b"/*")
        || trimmed.starts_with(b"*")
        || trimmed.starts_with(b"\"")
        || trimmed.starts_with(b"'")
    {
        return false;
    }
    let Some(index) = line
        .windows(symbol.len())
        .position(|window| window == symbol)
    else {
        return false;
    };
    if index > 0 && is_symbol_byte(line[index - 1])
        || index + symbol.len() < line.len() && is_symbol_byte(line[index + symbol.len()])
    {
        return false;
    }
    let before = &line[..index];
    let after = line[index + symbol.len()..]
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .collect::<Vec<_>>();
    let shape = after.starts_with(b"(")
        || after.starts_with(b"{")
        || after.starts_with(b":")
        || after.starts_with(b"=")
        || after.starts_with(b"<");
    let tokens = before
        .split(|byte| !is_symbol_byte(*byte))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let keyword = tokens.last().is_some_and(|token| {
        matches!(
            *token,
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
        )
    });
    keyword && shape
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
    line.split(|byte| !is_symbol_byte(*byte))
        .filter(|token| !token.is_empty())
        .any(|token| is_declaration(line, token))
}

#[cfg(test)]
mod tests {
    use super::locate;

    #[test]
    fn unbraced_declarations_end_before_the_next_declaration() {
        let source = b"const FOO: usize = 1;\nconst BAR: usize = 2;";
        let span = locate(source, "FOO", "source.rs").unwrap();
        assert_eq!((span.start, span.end), (1, 1));
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
        assert_eq!((span.start, span.end), (1, 12));
    }

    #[test]
    fn declaration_wins_over_mentions_and_longer_identifiers() {
        let source = b"// parse handles input\nfn parse_more() {}\nfn parse() {\n    parse_more();\n}\nfn next() {}\n";
        let span = locate(source, "parse", "source.rs").unwrap();
        assert_eq!((span.start, span.end), (3, 5));
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
        assert_eq!((span.start, span.end), (1, 7));
    }

    #[test]
    fn indentation_bounds_python_symbols() {
        let source = b"def parse():\n    return 1\n\ndef next():\n    return 2\n";
        let span = locate(source, "parse", "source.py").unwrap();
        assert_eq!((span.start, span.end), (1, 3));
    }

    #[test]
    fn mention_only_symbols_keep_the_existing_fallback() {
        let span = locate(b"parse(value)\n", "parse", "source.py").unwrap();
        assert_eq!((span.start, span.end), (1, 2));
    }

    #[test]
    fn invalid_symbols_keep_the_existing_input_errors() {
        for (name, message) in [
            ("  ", "symbol must not be empty"),
            (
                "parse",
                "symbol parse was not found in source.rs at the target revision",
            ),
        ] {
            let error = locate(b"fn parse_more() {}\n", name, "source.rs")
                .err()
                .unwrap();
            assert_eq!(error.to_string(), message);
        }
    }
}
