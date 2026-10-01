use std::{
    env,
    fs::File,
    io::{self, Read},
    path::PathBuf,
};

use ort::{session::Session, value::Tensor};
use sha2::{Digest, Sha256};
use tokenizers::{Encoding, Tokenizer};

use crate::app::AppError;

use super::resources;

const TOTAL_TOKEN_LIMIT: usize = 256;
const TITLE_TOKEN_LIMIT: usize = 64;
const PATH_TOKEN_LIMIT: usize = 32;
const EMBEDDING_DIMENSION: usize = 384;
const CPU_THREADS: usize = 10;

const MAX_TOKENIZER_INPUT_CHARS: usize = 4096;
pub(crate) struct CommitDocument {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) paths: Vec<String>,
}

pub(crate) struct PreparedInput {
    tokens: Vec<u32>,
    fingerprint: String,
}

impl PreparedInput {
    pub(crate) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn token_count(&self) -> usize {
        self.tokens.len()
    }
}

pub(crate) struct Encoder {
    tokenizer: Tokenizer,
    session: Session,
    runtime_provenance: String,
}

impl Encoder {
    pub(crate) fn load() -> Result<Self, AppError> {
        let runtime_hash = load_pinned_runtime()?;
        let assets = resources::ensure()?;
        let tokenizer = Tokenizer::from_file(&assets.tokenizer).map_err(|error| {
            AppError::operational(format!(
                "error: loading the pinned semantic tokenizer: {error}"
            ))
        })?;
        if tokenizer.token_to_id("[PAD]").is_none() {
            return Err(AppError::operational(
                "error: the pinned semantic tokenizer has no [PAD] token",
            ));
        }
        let session = Session::builder()
            .map_err(|error| {
                AppError::operational(format!("error: creating semantic ONNX session: {error}"))
            })?
            .with_intra_threads(
                std::thread::available_parallelism()
                    .map(|count| count.get().min(CPU_THREADS))
                    .unwrap_or(1),
            )
            .map_err(|error| {
                AppError::operational(format!("error: configuring semantic CPU threads: {error}"))
            })?
            .commit_from_file(&assets.model)
            .map_err(|error| {
                AppError::operational(format!(
                    "error: loading the pinned semantic ONNX model: {error}"
                ))
            })?;
        let runtime_provenance = format!(
            "ort=2.0.0-rc.11;onnxruntime=1.23.2;api={};sha256={runtime_hash}",
            ort::MINOR_VERSION
        );
        Ok(Self {
            tokenizer,
            session,
            runtime_provenance,
        })
    }

    pub(crate) fn runtime_provenance(&self) -> &str {
        &self.runtime_provenance
    }

    pub(crate) fn prepare(&self, document: &CommitDocument) -> Result<PreparedInput, AppError> {
        let tokens = bounded_input_ids(&self.tokenizer, document)?;
        let fingerprint = token_fingerprint(&tokens);
        Ok(PreparedInput {
            tokens,
            fingerprint,
        })
    }

    pub(crate) fn embed(&mut self, inputs: &[&PreparedInput]) -> Result<Vec<Vec<f32>>, AppError> {
        let pad_id = self.tokenizer.token_to_id("[PAD]").ok_or_else(|| {
            AppError::operational("error: semantic tokenizer lost its [PAD] token")
        })? as i64;
        let sequences = inputs
            .iter()
            .map(|input| input.tokens.as_slice())
            .collect::<Vec<_>>();
        let batch_size = sequences.len();
        let sequence_length = sequences
            .iter()
            .map(|sequence| sequence.len())
            .max()
            .unwrap_or(0);
        if batch_size == 0 || sequence_length == 0 {
            return Err(AppError::operational(
                "error: semantic encoder received an empty batch",
            ));
        }

        let mut input_ids = vec![pad_id; batch_size * sequence_length];
        let mut attention_mask = vec![0_i64; batch_size * sequence_length];
        for (row, sequence) in sequences.iter().enumerate() {
            let start = row * sequence_length;
            for (column, token) in sequence.iter().enumerate() {
                input_ids[start + column] = i64::from(*token);
                attention_mask[start + column] = 1;
            }
        }
        let token_type_ids = vec![0_i64; batch_size * sequence_length];
        let input_ids = Tensor::<i64>::from_array(([batch_size, sequence_length], input_ids))
            .map_err(|error| {
                AppError::operational(format!("error: creating token tensor: {error}"))
            })?;
        let attention_mask =
            Tensor::<i64>::from_array(([batch_size, sequence_length], attention_mask)).map_err(
                |error| AppError::operational(format!("error: creating attention tensor: {error}")),
            )?;
        let token_type_ids =
            Tensor::<i64>::from_array(([batch_size, sequence_length], token_type_ids)).map_err(
                |error| {
                    AppError::operational(format!("error: creating token type tensor: {error}"))
                },
            )?;
        let outputs = self
            .session
            .run(ort::inputs! {
                "input_ids" => input_ids,
                "attention_mask" => attention_mask,
                "token_type_ids" => token_type_ids,
            })
            .map_err(|error| {
                AppError::operational(format!("error: running semantic ONNX inference: {error}"))
            })?;
        let hidden_states = outputs.get("last_hidden_state").ok_or_else(|| {
            AppError::operational(
                "error: the pinned semantic ONNX model has no `last_hidden_state` output",
            )
        })?;
        let (_, hidden_states) = hidden_states.try_extract_tensor::<f32>().map_err(|error| {
            AppError::operational(format!(
                "error: reading semantic ONNX hidden states: {error}"
            ))
        })?;
        let expected = batch_size * sequence_length * EMBEDDING_DIMENSION;
        if hidden_states.len() != expected {
            return Err(AppError::operational(format!(
                "error: semantic ONNX output has {} values; expected {expected}",
                hidden_states.len()
            )));
        }

        sequences
            .iter()
            .enumerate()
            .map(|(row, tokens)| {
                let mut pooled = vec![0.0_f32; EMBEDDING_DIMENSION];
                for token in 0..tokens.len() {
                    let offset = (row * sequence_length + token) * EMBEDDING_DIMENSION;
                    for (dimension, value) in pooled.iter_mut().enumerate() {
                        *value += hidden_states[offset + dimension];
                    }
                }
                let divisor = tokens.len() as f32;
                for value in &mut pooled {
                    *value /= divisor;
                }
                let norm = pooled.iter().map(|value| value * value).sum::<f32>().sqrt();
                if !norm.is_finite() || norm <= f32::EPSILON {
                    return Err(AppError::operational(
                        "error: semantic model produced a zero or non-finite embedding",
                    ));
                }
                for value in &mut pooled {
                    *value /= norm;
                }
                Ok(pooled)
            })
            .collect()
    }
}

fn bounded_input_ids(
    tokenizer: &Tokenizer,
    document: &CommitDocument,
) -> Result<Vec<u32>, AppError> {
    let title = truncate_tokens(tokenizer, &document.title, TITLE_TOKEN_LIMIT)?;
    let paths = truncate_tokens(tokenizer, &document.paths.join("\n"), PATH_TOKEN_LIMIT)?;
    let empty_body = format!("Title: {title}\nBody: \nPaths: {paths}");
    let base_length = encode(tokenizer, &empty_body, true)?.get_ids().len();
    let mut body = truncate_tokens(
        tokenizer,
        &document.body,
        TOTAL_TOKEN_LIMIT.saturating_sub(base_length),
    )?;

    loop {
        let text = format!("Title: {title}\nBody: {body}\nPaths: {paths}");
        let encoding = encode(tokenizer, &text, true)?;
        if encoding.get_ids().len() <= TOTAL_TOKEN_LIMIT {
            return Ok(encoding.get_ids().to_vec());
        }
        if body.is_empty() {
            return Err(AppError::operational(
                "error: semantic input labels exceed the 256-token limit",
            ));
        }
        let shortened = drop_last_token(tokenizer, &body)?;
        if shortened.len() == body.len() {
            return Err(AppError::operational(
                "error: could not fit semantic input within the 256-token limit",
            ));
        }
        body = shortened;
    }
}

fn token_fingerprint(tokens: &[u32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"gitscry-semantic-input-v1\0");
    hasher.update((tokens.len() as u64).to_le_bytes());
    for token in tokens {
        hasher.update(token.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn truncate_tokens(tokenizer: &Tokenizer, text: &str, limit: usize) -> Result<String, AppError> {
    if limit == 0 {
        return Ok(String::new());
    }
    let mut char_limit = limit.saturating_mul(2).clamp(1, MAX_TOKENIZER_INPUT_CHARS);
    loop {
        let (prefix, is_complete) = tokenizer_input_prefix(text, char_limit);
        let encoding = encode(tokenizer, prefix, false)?;
        let token_count = encoding.get_ids().len();
        if token_count >= limit || is_complete || char_limit == MAX_TOKENIZER_INPUT_CHARS {
            if token_count <= limit {
                return Ok(prefix.to_owned());
            }
            let end = encoding
                .get_offsets()
                .get(limit - 1)
                .map(|(_, end)| *end)
                .unwrap_or_default()
                .min(prefix.len());
            if !prefix.is_char_boundary(end) {
                return Err(AppError::operational(
                    "error: tokenizer returned a non-UTF-8 semantic input boundary",
                ));
            }
            return Ok(prefix[..end].to_owned());
        }
        char_limit = char_limit.saturating_mul(2).min(MAX_TOKENIZER_INPUT_CHARS);
    }
}

fn tokenizer_input_prefix(text: &str, char_limit: usize) -> (&str, bool) {
    match text.char_indices().nth(char_limit) {
        Some((end, _)) => (&text[..end], false),
        None => (text, true),
    }
}

fn drop_last_token(tokenizer: &Tokenizer, text: &str) -> Result<String, AppError> {
    let encoding = encode(tokenizer, text, false)?;
    let start = encoding
        .get_offsets()
        .last()
        .map(|(start, _)| *start)
        .unwrap_or_default()
        .min(text.len());
    let start = (0..=start)
        .rev()
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or_default();
    Ok(text[..start].trim_end().to_owned())
}

fn encode(
    tokenizer: &Tokenizer,
    text: &str,
    add_special_tokens: bool,
) -> Result<Encoding, AppError> {
    tokenizer.encode(text, add_special_tokens).map_err(|error| {
        AppError::operational(format!("error: tokenizing semantic input: {error}"))
    })
}

fn load_pinned_runtime() -> Result<String, AppError> {
    let path = env::var_os("ORT_DYLIB_PATH")
        .map(PathBuf::from)
        .ok_or_else(|| {
            AppError::operational(
                "error: ONNX Runtime 1.23.2 is not installed; for controlled semantic-index testing, set ORT_DYLIB_PATH to the verified platform library. Runtime packaging is handled separately.",
            )
        })?;
    if !path.is_file() {
        return Err(AppError::operational(format!(
            "error: ORT_DYLIB_PATH does not name an ONNX Runtime library: `{}`",
            path.display()
        )));
    }
    let expected = expected_runtime_hash()?;
    let path = path.canonicalize().map_err(|error| {
        AppError::operational(format!(
            "error: resolving ONNX Runtime library `{}`: {error}",
            path.display()
        ))
    })?;
    let actual = hash_file(&path).map_err(|error| {
        AppError::operational(format!(
            "error: verifying ONNX Runtime library `{}`: {error}",
            path.display()
        ))
    })?;
    if actual != expected {
        return Err(AppError::operational(format!(
            "error: ONNX Runtime library `{}` has SHA-256 {actual}; expected pinned 1.23.2 library {expected}",
            path.display()
        )));
    }
    if ort::MINOR_VERSION != 23 {
        return Err(AppError::operational(format!(
            "error: ort binding requires ONNX Runtime C API v23, got v{}",
            ort::MINOR_VERSION
        )));
    }
    let loaded = ort::init_from(&path)
        .map_err(|error| {
            AppError::operational(format!(
                "error: loading pinned ONNX Runtime library `{}`: {error}",
                path.display()
            ))
        })?
        .commit();
    if !loaded {
        return Err(AppError::operational(
            "error: another ONNX Runtime environment is already active in this process",
        ));
    }
    Ok(actual)
}

fn expected_runtime_hash() -> Result<&'static str, AppError> {
    match (env::consts::OS, env::consts::ARCH, cfg!(target_env = "gnu")) {
        ("windows", "x86_64", _) => {
            Ok("dec964ab1ee36cc9b0ae247d13b376627992fc57dec0454354017ab8fd84f1ea")
        }
        ("linux", "x86_64", true) => {
            Ok("13ab8084954fa4a47c777880180b90810d6020f021441395712b48a75b74c68b")
        }
        ("macos", "aarch64", _) => {
            Ok("d306d2bc768540766c7ed8a1e0ff05d2870c77a934ebeee4a7bafa1b732ef299")
        }
        _ => Err(AppError::operational(format!(
            "error: semantic indexing has no pinned ONNX Runtime 1.23.2 artifact for {}-{}",
            env::consts::OS,
            env::consts::ARCH
        ))),
    }
}

fn hash_file(path: &std::path::Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
