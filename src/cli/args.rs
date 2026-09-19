use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "gitscry", version, color = clap::ColorChoice::Never)]
pub(super) struct Cli {
    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Build the local cache from the default branch history.
    Index,
}
