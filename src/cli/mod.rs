mod args;

use std::ffi::OsString;

use clap::Parser;

use crate::app::AppError;

use args::Cli;
pub(crate) use args::Command;

pub(crate) fn parse<I, T>(arguments: I) -> Result<Command, AppError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    Cli::try_parse_from(arguments)
        .map(Into::into)
        .map_err(|error| AppError::input(error.to_string()))
}
