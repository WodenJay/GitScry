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
        let runtime = load_pinned_runtime()?;
        let assets = resources::ensure()?;
        let mut tokenizer = Tokenizer::from_file(&assets.tokenizer).map_err(|error| {
            AppError::operational(format!(
                "error: loading the pinned semantic tokenizer: {error}"
            ))
        })?;
        tokenizer.with_truncation(None).map_err(|error| {
            AppError::operational(format!(
                "error: configuring semantic tokenizer truncation: {error}"
            ))
        })?;
        tokenizer.with_padding(None);
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
            "ort={};onnxruntime={};api={};sha256={}",
            runtime.binding_version,
            runtime.runtime_version,
            runtime.api_version,
            runtime.library_sha256
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

fn load_pinned_runtime() -> Result<crate::runtime::RuntimeArtifact, AppError> {
    let executable = std::env::current_exe().map_err(|error| {
        AppError::operational(format!("error: locating the GitScry executable: {error}"))
    })?;
    let install_dir = executable.parent().ok_or_else(|| {
        AppError::operational("error: the GitScry executable has no installation directory")
    })?;
    let target = match (
        std::env::consts::OS,
        std::env::consts::ARCH,
        cfg!(target_env = "gnu"),
    ) {
        ("windows", "x86_64", _) => "x86_64-pc-windows-msvc",
        ("linux", "x86_64", true) => "x86_64-unknown-linux-gnu",
        ("macos", "aarch64", _) => "aarch64-apple-darwin",
        _ => {
            return Err(AppError::operational(format!(
                "error: semantic indexing has no pinned ONNX Runtime package for {}-{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            )));
        }
    };
    let pins = crate::runtime::RuntimePins::pinned().map_err(AppError::operational)?;
    let runtime = crate::runtime::resolve_installation(
        install_dir,
        env!("CARGO_PKG_VERSION"),
        target,
        &pins,
    )
    .map_err(|error| {
        AppError::operational(format!(
            "error: semantic runtime package is unavailable: {error}; reinstall GitScry from an official release (cargo install does not bundle ONNX Runtime)"
        ))
    })?;
    if ort::MINOR_VERSION != runtime.api_version {
        return Err(AppError::operational(format!(
            "error: ort binding requires ONNX Runtime C API v{}, got v{}",
            runtime.api_version,
            ort::MINOR_VERSION
        )));
    }
    let loaded = ort::init_from(&runtime.library_path)
        .map_err(|error| {
            AppError::operational(format!(
                "error: loading packaged ONNX Runtime library `{}`: {error}",
                runtime.library_path.display()
            ))
        })?
        .commit();
    if !loaded {
        return Err(AppError::operational(
            "error: another ONNX Runtime environment is already active in this process",
        ));
    }
    Ok(runtime)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::{CommitDocument, Encoder, resources};

    const REFERENCE_FIXTURE: &str =
        include_str!("../../tests/fixtures/minilm-document-embeddings.json");

    #[derive(Deserialize)]
    struct ReferenceFixture {
        model_revision: String,
        model_sha256: String,
        tokenizer_sha256: String,
        cases: Vec<ReferenceCase>,
    }

    #[derive(Deserialize)]
    struct ReferenceCase {
        name: String,
        title: String,
        body: String,
        paths: Vec<String>,
        expected_input_ids: Vec<Vec<u32>>,
        reference_vectors: Vec<Vec<f32>>,
    }

    #[derive(Default)]
    struct MaximumErrors {
        component: f64,
        cosine: f64,
        norm: f64,
    }

    #[test]
    #[ignore = "requires the pinned ONNX Runtime and MiniLM model assets"]
    fn matches_fixed_reference_embeddings() {
        let fixture: ReferenceFixture = serde_json::from_str(REFERENCE_FIXTURE).unwrap();
        assert_eq!(fixture.model_revision, resources::REVISION);
        assert_eq!(fixture.model_sha256, resources::MODEL_SHA256);
        assert_eq!(fixture.tokenizer_sha256, resources::TOKENIZER_SHA256);
        assert_eq!(fixture.cases.len(), 3);

        let mut encoder = Encoder::load().unwrap();
        let inputs = fixture
            .cases
            .iter()
            .map(|case| {
                assert_eq!(case.expected_input_ids.len(), 1, "{}", case.name);
                assert_eq!(case.reference_vectors.len(), 1, "{}", case.name);
                let input = encoder
                    .prepare(&CommitDocument {
                        title: case.title.clone(),
                        body: case.body.clone(),
                        paths: case.paths.clone(),
                    })
                    .unwrap();
                assert_eq!(input.tokens, case.expected_input_ids[0], "{}", case.name);
                input
            })
            .collect::<Vec<_>>();

        let batch = inputs.iter().collect::<Vec<_>>();
        let embeddings = encoder.embed(&batch).unwrap();
        assert_eq!(embeddings.len(), fixture.cases.len());

        let mut errors = MaximumErrors::default();
        for (case, actual) in fixture.cases.iter().zip(&embeddings) {
            assert_vector_matches(case, actual, &mut errors);
        }

        let reversed_batch = inputs.iter().rev().collect::<Vec<_>>();
        let reversed_embeddings = encoder.embed(&reversed_batch).unwrap();
        for (case, actual) in fixture.cases.iter().rev().zip(&reversed_embeddings) {
            assert_vector_matches(case, actual, &mut errors);
        }

        eprintln!(
            "reference parity passed: target={}-{} model={} model_sha256={} {} max_component_error={:.8e} max_cosine_distance={:.8e} max_norm_error={:.8e}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            fixture.model_revision,
            fixture.model_sha256,
            encoder.runtime_provenance(),
            errors.component,
            errors.cosine,
            errors.norm,
        );
    }

    fn assert_vector_matches(case: &ReferenceCase, actual: &[f32], errors: &mut MaximumErrors) {
        let expected = &case.reference_vectors[0];
        assert_eq!(actual.len(), 384, "{} embedding dimension", case.name);
        assert_eq!(expected.len(), 384, "{} reference dimension", case.name);
        assert!(
            actual.iter().all(|value| value.is_finite()),
            "{} non-finite output",
            case.name
        );

        assert!(
            expected.iter().all(|value| value.is_finite()),
            "{} non-finite reference value",
            case.name
        );

        let actual_norm = actual
            .iter()
            .map(|value| f64::from(*value).powi(2))
            .sum::<f64>()
            .sqrt();
        let expected_norm = expected
            .iter()
            .map(|value| f64::from(*value).powi(2))
            .sum::<f64>()
            .sqrt();

        assert!(
            actual_norm.is_finite() && actual_norm > 0.0,
            "{} invalid output norm",
            case.name
        );
        assert!(
            expected_norm.is_finite() && expected_norm > 0.0,
            "{} invalid reference norm",
            case.name
        );
        assert!(
            (expected_norm - 1.0).abs() <= 1e-5,
            "{} reference is not L2 normalized",
            case.name
        );
        let dot = actual
            .iter()
            .zip(expected)
            .map(|(actual, expected)| f64::from(*actual) * f64::from(*expected))
            .sum::<f64>();
        let component_error = actual
            .iter()
            .zip(expected)
            .map(|(actual, expected)| (f64::from(*actual) - f64::from(*expected)).abs())
            .fold(0.0_f64, f64::max);
        let cosine_distance = (1.0 - dot / (actual_norm * expected_norm)).abs();
        let norm_error = (actual_norm - 1.0).abs();

        assert!(
            component_error <= 3e-5,
            "{} component error {component_error}",
            case.name
        );
        assert!(
            cosine_distance <= 1e-6,
            "{} cosine distance {cosine_distance}",
            case.name
        );
        assert!(norm_error <= 1e-5, "{} norm error {norm_error}", case.name);
        errors.component = errors.component.max(component_error);
        errors.cosine = errors.cosine.max(cosine_distance);
        errors.norm = errors.norm.max(norm_error);
    }
}
