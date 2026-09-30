//! First gate: execute the pinned graph before integrating an encoder into GitScry.
use ort::{session::Session, value::Tensor};
use sha2::{Digest, Sha256};
use std::{error::Error, fs::File, io::Read, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    let model = std::env::args_os()
        .nth(1)
        .ok_or("usage: gitscry-encoder-gate MODEL.onnx")?;
    verify_model(Path::new(&model))?;
    let mut session = Session::builder()?
        .with_intra_threads(1)?
        .with_inter_threads(1)?
        .commit_from_file(model)?;
    // Pinned MiniLM tokenizer: [CLS] hello [SEP]. This is a graph/linking
    // probe, NOT evidence of document preprocessing or encoder parity.
    let ids = Tensor::from_array(([1usize, 3], vec![101i64, 7592, 102].into_boxed_slice()))?;
    let mask = Tensor::from_array(([1usize, 3], vec![1i64; 3].into_boxed_slice()))?;
    let types = Tensor::from_array(([1usize, 3], vec![0i64; 3].into_boxed_slice()))?;
    let output = session.run(ort::inputs![
        "input_ids" => ids,
        "attention_mask" => mask,
        "token_type_ids" => types
    ])?;
    let (shape, values) = output[0].try_extract_tensor::<f32>()?;
    if shape.as_ref() != [1, 3, 384] || values.iter().any(|v| !v.is_finite()) {
        return Err("invalid MiniLM token output".into());
    }
    let mut vector = [0f32; 384];
    for token in values.as_chunks::<384>().0 {
        for (mean, value) in vector.iter_mut().zip(token) {
            *mean += value / 3.0;
        }
    }
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if !norm.is_finite() || norm <= 0.0 {
        return Err("invalid pooled norm".into());
    }
    for value in &mut vector {
        *value /= norm;
    }
    let normalized = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if (normalized - 1.0).abs() > 1e-5 {
        return Err("normalization failed".into());
    }
    println!("pinned FP32 graph executed: shape={shape:?}, vector_dim=384, norm={normalized}");
    Ok(())
}

fn verify_model(path: &Path) -> Result<(), Box<dyn Error>> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() != 90_387_630 {
        return Err("pinned model size mismatch".into());
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if format!("{:x}", hash.finalize())
        != "bbd7b466f6d58e646fdc2bd5fd67b2f5e93c0b687011bd4548c420f7bd46f0c5"
    {
        return Err("pinned model digest mismatch".into());
    }
    Ok(())
}
