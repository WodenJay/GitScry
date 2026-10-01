mod encoder;
mod resources;

pub(crate) use encoder::{CommitDocument, Encoder, InputPreprocessor, PreparedInput};

pub(crate) fn encoder_fingerprint() -> String {
    resources::encoder_fingerprint()
}
