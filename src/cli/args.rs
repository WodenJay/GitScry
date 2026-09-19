use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "gitscry", version, color = clap::ColorChoice::Never)]
pub(super) struct Cli {
    #[command(subcommand)]
    command: CommandArgs,
}

#[derive(Debug, Subcommand)]
enum CommandArgs {
    /// Build the local cache from the default branch history.
    Index,
}

#[derive(Debug)]
pub(crate) enum Command {
    Index,
}

impl From<Cli> for Command {
    fn from(cli: Cli) -> Self {
        match cli.command {
            CommandArgs::Index => Self::Index,
        }
    }
}
