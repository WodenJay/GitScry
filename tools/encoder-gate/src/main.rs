//! Release smoke harness: exercises the reusable encoder, not a second implementation.
use gitscry_embedding::{DIMENSIONS, Document, Encoder, Vector, Workload};
use serde::Deserialize;
use std::{error::Error, fs, path::Path};

#[derive(Deserialize)]
struct Fixtures {
    max_absolute_error: f64,
    max_cosine_distance: f64,
    documents: Vec<ReferenceDocument>,
    queries: Vec<ReferenceQuery>,
}
#[derive(Deserialize)]
struct ReferenceDocument {
    identity: String,
    message: Vec<u8>,
    paths: Vec<Vec<u8>>,
    vector: Vec<f32>,
}
#[derive(Deserialize)]
struct ReferenceQuery {
    text: String,
    vectors: Vec<Vec<f32>>,
}

fn compare(actual: &Vector, expected: &[f32], fixtures: &Fixtures) -> Result<(), Box<dyn Error>> {
    if expected.len() != DIMENSIONS || actual.iter().chain(expected).any(|v| !v.is_finite()) {
        return Err("invalid reference/output dimensions or finite values".into());
    }
    let norm = |v: &[f32]| v.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>().sqrt();
    if (norm(actual) - 1.0).abs() > 1e-5 {
        return Err("output is not normalized".into());
    }
    let absolute = actual
        .iter()
        .zip(expected)
        .map(|(a, b)| (f64::from(*a) - f64::from(*b)).abs())
        .fold(0.0, f64::max);
    let dot = actual
        .iter()
        .zip(expected)
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum::<f64>();
    let cosine = 1.0 - dot / (norm(actual) * norm(expected));
    println!("parity: dimension={DIMENSIONS}, max_abs={absolute:.9}, cosine_distance={cosine:.9}");
    if absolute > fixtures.max_absolute_error || cosine > fixtures.max_cosine_distance {
        return Err("independent reference tolerance exceeded".into());
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let directory = std::env::args_os()
        .nth(1)
        .ok_or("usage: gitscry-encoder-gate RESOURCE_DIRECTORY")?;
    let fixtures: Fixtures = serde_json::from_str(include_str!(
        "../../../src/analysis/retrieval/embedding/fixtures/reference.json"
    ))?;
    let mut encoder = Encoder::open(Path::new(&directory), Workload::Index)?;
    // Reversed, varied lengths, repeated inputs, and >8 rows exercise padding,
    // internal length ordering, identity restoration, and batch boundaries.
    let references = fixtures
        .documents
        .iter()
        .rev()
        .cycle()
        .take(13)
        .collect::<Vec<_>>();
    let documents = references
        .iter()
        .map(|d| Document {
            identity: &d.identity,
            message: &d.message,
            paths: &d.paths,
        })
        .collect::<Vec<_>>();
    let vectors = encoder.documents(&documents)?;
    if vectors.len() != references.len() {
        return Err("document output count mismatch".into());
    }
    for ((identity, vector), reference) in vectors.iter().zip(&references) {
        if identity != &reference.identity {
            return Err("vector identity/order mismatch".into());
        }
        compare(vector, &reference.vector, &fixtures)?;
        let single = encoder.documents(&[Document {
            identity,
            message: &reference.message,
            paths: &reference.paths,
        }])?;
        compare(&single[0].1, &reference.vector, &fixtures)?;
    }
    if !encoder.documents(&[])?.is_empty() {
        return Err("empty batch was not empty".into());
    }
    let mut encoder = Encoder::open(Path::new(&directory), Workload::Query)?;
    for query in &fixtures.queries {
        let vectors = encoder.query(&query.text)?;
        if vectors.len() != query.vectors.len() {
            return Err("query window count mismatch".into());
        }
        for (actual, expected) in vectors.iter().zip(&query.vectors) {
            compare(actual, expected, &fixtures)?;
        }
    }
    // Corrupt a small resource in a fresh directory. Verification must reject
    // it before tokenizer initialization, and must not repair/download anything.
    let corrupt = tempfile::tempdir()?;
    for name in [
        "model.onnx",
        "tokenizer.json",
        "tokenizer_config.json",
        "special_tokens_map.json",
        "config.json",
        "README.md",
    ] {
        fs::copy(Path::new(&directory).join(name), corrupt.path().join(name))?;
    }
    let config = corrupt.path().join("config.json");
    let size = fs::metadata(&config)?.len() as usize;
    fs::write(&config, vec![b' '; size])?;
    let rejected = Encoder::open(corrupt.path(), Workload::Query).err();
    if !rejected.is_some_and(|error| error.to_string().contains("config.json: SHA-256 mismatch")) {
        return Err("corrupt pinned resource was not rejected".into());
    }
    println!(
        "PASS: offline canonical document/query parity, FP32 normalization, padding, input identity and verified resources"
    );
    Ok(())
}
