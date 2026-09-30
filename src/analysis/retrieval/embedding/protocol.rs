use crate::{Document, Error};
use tokenizers::{Encoding, Tokenizer};

pub(crate) fn document(
    tokenizer: &Tokenizer,
    document: &Document<'_>,
) -> Result<(String, Encoding), Error> {
    let message = String::from_utf8_lossy(document.message);
    let (title, body) = message.split_once('\n').unwrap_or((&message, ""));
    let title = prefix(tokenizer, title.trim_end_matches('\r'), 64)?;
    let paths = document
        .paths
        .iter()
        .map(|path| String::from_utf8_lossy(path))
        .collect::<Vec<_>>()
        .join("\n");
    let paths = prefix(tokenizer, &paths, 32)?;
    let build = |body: &str| format!("Title: {title}\nBody: {body}\nPaths: {paths}");
    let reserved = encode(tokenizer, &build(""), true)?.len();
    let budget = 256usize
        .checked_sub(reserved)
        .ok_or_else(|| Error::Tokenization("reserved fields exceed model budget".into()))?;
    let mut body = prefix(tokenizer, body, budget)?;
    loop {
        let canonical = build(body);
        let encoded = encode(tokenizer, &canonical, true)?;
        if encoded.len() <= 256 {
            return Ok((canonical, encoded));
        }
        let content = encode(tokenizer, body, false)?;
        body = if content.len() > 1 {
            &body[..content.get_offsets()[content.len() - 2].1]
        } else {
            ""
        };
    }
}

// Offsets refer to original UTF-8 bytes, not decoded WordPiece strings. Grow
// only the prefix: exceptional commit bodies must not all enter the tokenizer.
fn prefix<'a>(tokenizer: &Tokenizer, text: &'a str, budget: usize) -> Result<&'a str, Error> {
    let mut characters = 4096usize.max(budget * 16);
    loop {
        let end = text
            .char_indices()
            .nth(characters)
            .map_or(text.len(), |(byte, _)| byte);
        let encoded = encode(tokenizer, &text[..end], false)?;
        if encoded.len() > budget {
            return Ok(&text[..if budget == 0 {
                0
            } else {
                encoded.get_offsets()[budget - 1].1
            }]);
        }
        if end == text.len() {
            return Ok(text);
        }
        characters = characters.saturating_mul(2);
    }
}

fn encode(tokenizer: &Tokenizer, text: &str, special: bool) -> Result<Encoding, Error> {
    tokenizer
        .encode(text, special)
        .map_err(|error| Error::Tokenization(error.to_string()))
}

pub(crate) fn query(tokenizer: &Tokenizer, text: &str) -> Result<Vec<Encoding>, Error> {
    let content = encode(tokenizer, text, false)?;
    if content.len() <= 254 {
        return Ok(vec![encode(tokenizer, text, true)?]);
    }
    let mut windows = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + 220).min(content.len());
        let mut ids = vec![101];
        ids.extend_from_slice(&content.get_ids()[start..end]);
        ids.push(102);
        let width = ids.len();
        // Preserve the original token IDs; never decode and re-tokenize a window.
        windows.push(Encoding::new(
            ids,
            vec![0; width],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![1; width],
            vec![],
            Default::default(),
        ));
        if end == content.len() {
            return Ok(windows);
        }
        start += 180;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn tokenizer() -> Tokenizer {
        let mut tokenizer =
            Tokenizer::from_bytes(include_bytes!("fixtures/tokenizer.json")).unwrap();
        tokenizer.with_padding(None);
        tokenizer.with_truncation(None).unwrap();
        tokenizer
    }

    #[test]
    fn document_inputs_match_independent_reference() {
        let fixtures: Value =
            serde_json::from_str(include_str!("fixtures/reference.json")).unwrap();
        for fixture in fixtures["documents"].as_array().unwrap() {
            let bytes = |v: &Value| {
                v.as_array()
                    .unwrap()
                    .iter()
                    .map(|b| b.as_u64().unwrap() as u8)
                    .collect::<Vec<_>>()
            };
            let message = bytes(&fixture["message"]);
            let paths = fixture["paths"]
                .as_array()
                .unwrap()
                .iter()
                .map(bytes)
                .collect::<Vec<_>>();
            let input = Document {
                identity: fixture["identity"].as_str().unwrap(),
                message: &message,
                paths: &paths,
            };
            let (canonical, encoded) = document(&tokenizer(), &input).unwrap();
            assert_eq!(
                canonical,
                fixture["canonical"].as_str().unwrap(),
                "{}",
                input.identity
            );
            assert_eq!(
                serde_json::to_value(encoded.get_ids()).unwrap(),
                fixture["ids"],
                "{}",
                input.identity
            );
            assert_eq!(
                serde_json::to_value(encoded.get_type_ids()).unwrap(),
                fixture["type_ids"]
            );
            assert!(encoded.len() <= 256);
        }
    }

    #[test]
    fn query_boundaries_and_original_ids_match_reference() {
        let fixtures: Value =
            serde_json::from_str(include_str!("fixtures/reference.json")).unwrap();
        for fixture in fixtures["queries"].as_array().unwrap() {
            let encoded = query(&tokenizer(), fixture["text"].as_str().unwrap()).unwrap();
            assert_eq!(
                serde_json::to_value(encoded.iter().map(Encoding::get_ids).collect::<Vec<_>>())
                    .unwrap(),
                fixture["ids"]
            );
            assert_eq!(
                serde_json::to_value(
                    encoded
                        .iter()
                        .map(Encoding::get_type_ids)
                        .collect::<Vec<_>>()
                )
                .unwrap(),
                fixture["type_ids"]
            );
        }
        let text = "unaffordable ".repeat(400);
        let tokenizer = tokenizer();
        let content = tokenizer.encode(text.as_str(), false).unwrap();
        let chunks = query(&tokenizer, &text).unwrap();
        for (i, chunk) in chunks.iter().enumerate() {
            let start = i * 180;
            assert_eq!(
                &chunk.get_ids()[1..chunk.len() - 1],
                &content.get_ids()[start..(start + 220).min(content.len())]
            );
        }
        assert_eq!(
            (chunks.len() - 1) * 180 + chunks.last().unwrap().len() - 2,
            content.len()
        );
    }

    #[test]
    fn exceptional_body_keeps_document_without_full_text_tokenization() {
        let message = format!("Keep commit\n{}", "body ".repeat(4_000_000));
        let (_, encoding) = document(
            &tokenizer(),
            &Document {
                identity: "large",
                message: message.as_bytes(),
                paths: &[],
            },
        )
        .unwrap();
        assert!(encoding.len() <= 256);
    }
}
