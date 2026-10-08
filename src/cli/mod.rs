mod args;

use std::ffi::OsString;

use clap::{Parser, error::ErrorKind};

use crate::app::AppError;

use args::Cli;
pub(crate) use args::{CodeChange, Command, HistoricalScopeArgs, StatsGroup};

pub(crate) enum Parsed {
    Command(Box<Command>),
    Display(String),
}

pub(crate) fn parse<I, T>(arguments: I) -> Result<Parsed, AppError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    match Cli::try_parse_from(arguments) {
        Ok(cli) => {
            validate_trace(&cli.command)?;
            Ok(Parsed::Command(Box::new(cli.command)))
        }
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

fn validate_trace(command: &args::Command) -> Result<(), AppError> {
    if matches!(
        command,
        args::Command::Search {
            trace: true,
            change: Some(args::CodeChange::Added),
            ..
        }
    ) {
        return Err(AppError::input("--trace requires --change removed"));
    }
    Ok(())
}
