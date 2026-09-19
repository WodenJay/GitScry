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
    /// Search the default branch history for relevant material.
    Search {
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
    },
}

fn parse_limit(value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| "limit must be a positive integer".to_owned())?;
    (limit > 0)
        .then_some(limit)
        .ok_or_else(|| "limit must be greater than zero".to_owned())
}
