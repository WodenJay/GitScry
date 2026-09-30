use crate::{DIMENSIONS, Document, Error, Vector, pooling, protocol, resources};
use ort::{session::Session, value::Tensor};
use std::path::Path;
use tokenizers::{Encoding, Tokenizer};

/// Fixed thread policy, not a caller-selected tuning parameter.
#[derive(Clone, Copy)]
pub enum Workload {
    Index,
    Query,
}

/// One offline CPU session, reused for sequential bounded batches.
pub struct Encoder {
    tokenizer: Tokenizer,
    session: Session,
}

impl Encoder {
    /// Verify every pinned resource before parsing the tokenizer or creating ORT.
    /// The caller explicitly acquires resources; this module never downloads.
    pub fn open(directory: &Path, workload: Workload) -> Result<Self, Error> {
        resources::verify(directory)?;
        let mut tokenizer = Tokenizer::from_file(directory.join("tokenizer.json"))
            .map_err(|error| Error::Tokenization(error.to_string()))?;
        tokenizer.with_padding(None);
        tokenizer
            .with_truncation(None)
            .map_err(|error| Error::Tokenization(error.to_string()))?;
        let cap = match workload {
            Workload::Index => 10,
            Workload::Query => 4,
        };
        let threads = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(cap);
        let session = (|| -> ort::Result<Session> {
            Session::builder()?
                .with_intra_threads(threads)?
                .with_inter_threads(1)?
                .with_parallel_execution(false)?
                .commit_from_file(directory.join("model.onnx"))
        })()
        .map_err(runtime_error)?;
        Ok(Self { tokenizer, session })
    }

    /// Results preserve caller order and identity, even after internal bucketing.
    /// At most eight documents/tensor rows are in flight; no concurrent inference.
    pub fn documents(
        &mut self,
        documents: &[Document<'_>],
    ) -> Result<Vec<(String, Vector)>, Error> {
        let mut results = Vec::with_capacity(documents.len());
        for batch in documents.chunks(8) {
            let mut inputs = batch
                .iter()
                .enumerate()
                .map(|(index, document)| {
                    protocol::document(&self.tokenizer, document)
                        .map(|(_, encoding)| (index, encoding))
                })
                .collect::<Result<Vec<_>, _>>()?;
            inputs.sort_by_key(|(_, encoding)| encoding.len());
            let indices = inputs.iter().map(|(index, _)| *index).collect::<Vec<_>>();
            let encodings = inputs
                .into_iter()
                .map(|(_, encoding)| encoding)
                .collect::<Vec<_>>();
            let vectors = self.infer(&encodings)?;
            let mut ordered = vec![[0.0; DIMENSIONS]; batch.len()];
            for (index, vector) in indices.into_iter().zip(vectors) {
                ordered[index] = vector;
            }
            results.extend(
                batch
                    .iter()
                    .zip(ordered)
                    .map(|(document, vector)| (document.identity.to_owned(), vector)),
            );
        }
        Ok(results)
    }

    /// Short input gives one vector; longer input gives original-ID windows in
    /// head-to-tail order (220 content tokens, 40 overlap). Ranking is not here.
    pub fn query(&mut self, text: &str) -> Result<Vec<Vector>, Error> {
        let encodings = protocol::query(&self.tokenizer, text)?;
        let mut results = Vec::with_capacity(encodings.len());
        for batch in encodings.chunks(8) {
            results.extend(self.infer(batch)?);
        }
        Ok(results)
    }

    fn infer(&mut self, inputs: &[Encoding]) -> Result<Vec<Vector>, Error> {
        let width = inputs.iter().map(Encoding::len).max().unwrap_or(0);
        if inputs.is_empty() || inputs.len() > 8 || width > 256 || width == 0 {
            return Err(Error::Tokenization("invalid inference batch".into()));
        }
        let mut ids = vec![0i64; inputs.len() * width];
        let mut mask = vec![0i64; ids.len()];
        let mut types = vec![0i64; ids.len()];
        let lengths = inputs.iter().map(Encoding::len).collect::<Vec<_>>();
        for (row, input) in inputs.iter().enumerate() {
            for token in 0..input.len() {
                let at = row * width + token;
                ids[at] = i64::from(input.get_ids()[token]);
                types[at] = i64::from(input.get_type_ids()[token]);
                mask[at] = 1;
            }
        }
        let shape = [inputs.len(), width];
        let ids = Tensor::from_array((shape, ids.into_boxed_slice())).map_err(runtime_error)?;
        let mask = Tensor::from_array((shape, mask.into_boxed_slice())).map_err(runtime_error)?;
        let types = Tensor::from_array((shape, types.into_boxed_slice())).map_err(runtime_error)?;
        let outputs = self.session.run(ort::inputs!["input_ids" => ids, "attention_mask" => mask, "token_type_ids" => types]).map_err(runtime_error)?;
        let (shape, values) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(runtime_error)?;
        if shape.as_ref() != [inputs.len() as i64, width as i64, DIMENSIONS as i64] {
            return Err(Error::InvalidOutput(format!(
                "unexpected token shape {shape:?}"
            )));
        }
        pooling::mean(values, width, &lengths)
    }
}

fn runtime_error(error: ort::Error) -> Error {
    Error::Runtime(error.to_string())
}
