use clap::{ArgGroup, Parser, Subcommand};
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
    /// Find historical examples of a similar change or migration.
    Examples {
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        /// Repository-relative path the change touched; repeatable.
        #[arg(long = "path", value_name = "PATH")]
        paths: Vec<String>,
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
    },
    /// Find abandoned or reverted approaches, their recorded reason, and safe retry conditions.
    Failures {
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        /// Repository-relative path the change touched; repeatable.
        #[arg(long = "path", value_name = "PATH")]
        paths: Vec<String>,
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
    },
    /// Explain the local history behind one line or symbol.
    #[command(group(
        ArgGroup::new("anchor")
            .required(true)
            .args(["line", "symbol"]),
    ))]
    Why {
        /// Repository-relative path at the target revision.
        path: String,
        /// One-based line number to explain.
        #[arg(long, conflicts_with = "symbol", value_parser = parse_line)]
        line: Option<usize>,
        /// Symbol name to explain.
        #[arg(long, conflicts_with = "line")]
        symbol: Option<String>,
        /// Local revision containing the target; defaults to HEAD.
        #[arg(long, default_value = "HEAD")]
        at: String,
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
    },
}

fn parse_line(value: &str) -> Result<usize, String> {
    let line = value
        .parse::<usize>()
        .map_err(|_| "line must be a positive integer".to_owned())?;
    (line > 0)
        .then_some(line)
        .ok_or_else(|| "line must be greater than zero".to_owned())
}

fn parse_limit(value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| "limit must be a positive integer".to_owned())?;
    (limit > 0)
        .then_some(limit)
        .ok_or_else(|| "limit must be greater than zero".to_owned())
}
