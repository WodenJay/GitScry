mod args;

use std::ffi::OsString;

use clap::{Parser, error::ErrorKind};

use crate::app::AppError;

use args::Cli;
pub(crate) use args::Command;

pub(crate) enum Parsed {
    Command(Command),
    Display(String),
}

pub(crate) fn parse<I, T>(arguments: I) -> Result<Parsed, AppError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    match Cli::try_parse_from(arguments) {
        Ok(cli) => Ok(Parsed::Command(cli.command)),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            Ok(Parsed::Display(error.to_string()))
        }
        Err(error) => Err(AppError::input(error.to_string())),
    }
}
