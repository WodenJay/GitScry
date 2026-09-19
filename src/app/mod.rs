mod error;
mod index;

use crate::cli::Command;

pub(crate) use error::AppError;

pub(crate) struct Outcome {
    pub(crate) progress: Vec<String>,
    pub(crate) message: String,
}

pub(crate) fn execute(command: Command) -> Result<Outcome, AppError> {
    match command {
        Command::Index => index::run(),
    }
}
