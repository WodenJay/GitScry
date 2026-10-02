//! The cache representation and validity rules for fixed-encoder vectors.
use crate::{app::AppError, semantic::EMBEDDING_DIMENSION};

pub(super) const EMBEDDING_BYTES: usize = EMBEDDING_DIMENSION * std::mem::size_of::<f32>();
const NORMALIZATION_TOLERANCE: f64 = 1e-3;

pub(super) fn valid_fingerprint(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn normalized_values(values: impl Iterator<Item = f32>) -> bool {
    let mut norm_squared = 0.0_f64;
    for value in values {
        if !value.is_finite() {
            return false;
        }
        norm_squared += f64::from(value).powi(2);
    }
    (norm_squared.sqrt() - 1.0).abs() <= NORMALIZATION_TOLERANCE
}

pub(super) fn normalized_vector(vector: &[f32]) -> bool {
    vector.len() == EMBEDDING_DIMENSION && normalized_values(vector.iter().copied())
}

pub(super) fn valid_embedding(embedding: Option<&[u8]>) -> bool {
    embedding.is_some_and(|bytes| {
        bytes.len() == EMBEDDING_BYTES
            && normalized_values(
                bytes
                    .as_chunks::<{ std::mem::size_of::<f32>() }>()
                    .0
                    .iter()
                    .map(|chunk| f32::from_le_bytes(*chunk)),
            )
    })
}

pub(super) fn decode(bytes: &[u8]) -> Option<Vec<f32>> {
    if !valid_embedding(Some(bytes)) {
        return None;
    }
    Some(
        bytes
            .as_chunks::<{ std::mem::size_of::<f32>() }>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect(),
    )
}

pub(super) fn encode(vector: &[f32]) -> Result<Vec<u8>, AppError> {
    if vector.len() != EMBEDDING_DIMENSION || vector.iter().any(|value| !value.is_finite()) {
        return Err(AppError::operational(
            "error: semantic encoder returned an invalid 384-dimensional vector",
        ));
    }
    if !normalized_vector(vector) {
        return Err(AppError::operational(
            "error: semantic encoder returned an invalid normalized vector",
        ));
    }
    Ok(vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect())
}
