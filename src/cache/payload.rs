use lz4_flex::{compress, decompress};

use crate::app::AppError;

const MAX_HUNK_BYTES: usize = 1 << 30;

pub(super) fn encode(bytes: &[u8]) -> (Vec<u8>, i64) {
    (compress(bytes), bytes.len() as i64)
}

pub(super) fn decode(bytes: &[u8], length: i64, material: &str) -> Result<Vec<u8>, AppError> {
    let length = usize::try_from(length).map_err(|_| corruption(material, "invalid length"))?;
    if length > MAX_HUNK_BYTES {
        return Err(corruption(
            material,
            "length exceeds the decompression limit",
        ));
    }
    let decoded =
        decompress(bytes, length).map_err(|error| corruption(material, error.to_string()))?;
    if decoded.len() != length {
        return Err(corruption(
            material,
            format!("decoded length {} does not match {length}", decoded.len()),
        ));
    }
    Ok(decoded)
}

fn corruption(material: &str, reason: impl std::fmt::Display) -> AppError {
    AppError::operational(format!(
        "error: cache corruption in hunk {material}: {reason}; delete .gitscry and retry"
    ))
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn compressed_payloads_round_trip_bytes() {
        for bytes in [
            b"".as_slice(),
            b"not utf-8: \xff\xfe",
            vec![b'x'; 1_000_000].as_slice(),
        ] {
            let (compressed, length) = encode(bytes);
            assert_eq!(decode(&compressed, length, "commit/change").unwrap(), bytes);
        }
    }

    #[test]
    fn malformed_payload_is_rejected() {
        let (compressed, length) = encode(b"valid");
        let error = decode(&compressed, length + 1, "commit/change").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cache corruption in hunk commit/change")
        );
    }
}
