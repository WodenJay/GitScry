mod encoder;
mod resources;

pub(crate) const EMBEDDING_DIMENSION: usize = 384;
pub(crate) const EMBEDDING_BATCH_SIZE: usize = 8;
pub(crate) use encoder::{CommitDocument, Encoder, InputPreprocessor, PreparedInput};

pub(crate) fn encoder_fingerprint() -> String {
    resources::encoder_fingerprint()
}
