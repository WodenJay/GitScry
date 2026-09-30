//! Offline, pinned MiniLM encoding. No cache, CLI, resource acquisition, or provider selection.
#[cfg(any(feature = "runtime", test))]
mod pooling;
#[cfg(any(feature = "runtime", test))]
mod protocol;
#[cfg(any(feature = "runtime", test))]
mod resources;
#[cfg(feature = "runtime")]
mod runtime;
#[cfg(feature = "runtime")]
pub use runtime::{Encoder, Workload};

use std::fmt;

pub const DIMENSIONS: usize = 384;
pub type Vector = [f32; DIMENSIONS];

/// Raw Git text; paths remain in the caller's canonical order.
pub struct Document<'a> {
    pub identity: &'a str,
    pub message: &'a [u8],
    pub paths: &'a [Vec<u8>],
}

#[derive(Debug)]
pub enum Error {
    Resource(String),
    Tokenization(String),
    Runtime(String),
    InvalidOutput(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, detail) = match self {
            Self::Resource(detail) => ("resource", detail),
            Self::Tokenization(detail) => ("tokenization", detail),
            Self::Runtime(detail) => ("runtime", detail),
            Self::InvalidOutput(detail) => ("output", detail),
        };
        write!(f, "MiniLM {kind}: {detail}")
    }
}

impl std::error::Error for Error {}
