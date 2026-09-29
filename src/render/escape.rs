use base64::{Engine as _, engine::general_purpose::STANDARD};
/// Escape a path the way Git renders one: unquoted when plain, C-quoted when not.
pub(super) fn path(path: &[u8]) -> String {
    let quoted = path
        .iter()
        .any(|byte| !(0x21..=0x7e).contains(byte) || *byte == b'"' || *byte == b'\\');
    let mut rendered = String::with_capacity(path.len() + usize::from(quoted) * 2);
    if quoted {
        rendered.push('"');
    }
    for byte in path {
        match byte {
            b'\\' => rendered.push_str("\\\\"),
            b'"' => rendered.push_str("\\\""),
            b'\n' => rendered.push_str("\\n"),
            b'\r' => rendered.push_str("\\r"),
            b'\t' => rendered.push_str("\\t"),
            0x21..=0x7e => rendered.push(*byte as char),
            _ => rendered.push_str(&format!("\\{byte:03o}")),
        }
    }
    if quoted {
        rendered.push('"');
    }
    rendered
}

/// Keep changed source text on one safe output line; non-UTF-8 bytes are base64 encoded.
pub(super) fn code_line(line: &[u8]) -> String {
    match std::str::from_utf8(line) {
        Ok(line) => subject(line),
        Err(_) => format!("base64:{}", STANDARD.encode(line)),
    }
}

/// Escape a commit subject so the line-oriented output stays one line per field.
pub(super) fn subject(subject: &str) -> String {
    let mut rendered = String::with_capacity(subject.len());
    for character in subject.chars() {
        if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') {
            rendered.extend(character.escape_default());
        } else {
            rendered.push(character);
        }
    }
    rendered
}
