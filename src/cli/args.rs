use clap::{ArgGroup, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "gitscry",
    version,
    color = clap::ColorChoice::Never,
    before_help = ROOT_LONG_HELP,
)]
pub(super) struct Cli {
    #[command(subcommand)]
    pub(super) command: Command,
}

const ROOT_LONG_HELP: &str = "\
Your Git history is a treasure trove. GitScry uncovers the implementation examples, failed approaches, code relationships, and regression context hidden inside.

Pick one command by intent, then run `gitscry <command> --help` for inputs, options, and examples.";

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    #[command(
        about = "Build the local cache from the default branch history",
        long_about = "Build the local cache from the default branch history.\n\nUse `gitscry index` when you want to construct or refresh the local cache explicitly. The cache is a rebuildable local representation of the repository's Git history; Git remains the source of truth, and rerunning `index` rebuilds the cache from the current default branch tip.\n\nRequired input: none.\n\nExamples:\n\n  gitscry index"
    )]
    Index,

    #[command(
        about = "Search the default branch history for relevant material",
        long_about = "Search the default branch history for relevant material.\n\nUse `gitscry search` when you want general historical material about a topic and no specialized command fits: it returns commits whose subjects, bodies, and touched paths answer the query words.\n\nRequired input: one or more QUERY words describing the topic.\n\nExamples:\n\n  gitscry search retry backoff\n\n  gitscry search connection pool --limit 5"
    )]
    Search {
        /// Query words matched against commit subjects, bodies, and touched paths.
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        /// Maximum number of matches to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
    },

    #[command(
        about = "Find historical examples of a similar change or migration",
        long_about = "Find historical examples of a similar change or migration.\n\nUse `gitscry examples` before making a planned change when you want reusable precedents: commits that implemented something similar, with their change steps and touched paths.\n\nRequired input: one or more QUERY words describing the change. `--path` narrows the material to a repository-relative path the change touched and may be repeated.\n\nExamples:\n\n  gitscry examples retry backoff\n\n  gitscry examples retire provider --path src/lib.rs"
    )]
    Examples {
        /// Query words matched against commit subjects, bodies, and touched paths.
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        /// Repository-relative path the change touched; repeatable.
        #[arg(long = "path", value_name = "PATH")]
        paths: Vec<String>,
        /// Maximum number of matches to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
    },

    #[command(
        about = "Find abandoned or reverted approaches, their recorded reason, and safe retry conditions",
        long_about = "Find abandoned or reverted approaches, their recorded reason, and safe retry conditions.\n\nUse `gitscry failures` when you are considering an approach and want to know whether it was tried and abandoned: it returns reverted or superseded commits together with the recorded reason and retry conditions from their commit bodies.\n\nRequired input: one or more QUERY words describing the approach. `--path` narrows the material to a repository-relative path the change touched and may be repeated.\n\nExamples:\n\n  gitscry failures provider normalization\n\n  gitscry failures hand rolled parser --path src/parse.rs"
    )]
    Failures {
        /// Query words matched against commit subjects, bodies, and touched paths.
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        /// Repository-relative path the change touched; repeatable.
        #[arg(long = "path", value_name = "PATH")]
        paths: Vec<String>,
        /// Maximum number of matches to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
    },

    #[command(
        about = "Find historical paths changed alongside one or more seed paths",
        long_about = "Find historical paths changed alongside one or more seed paths.\n\nUse `gitscry related` when you are changing one or more paths and want to know which other paths historically changed together with them, such as mirrored files or coupled modules.\n\nRequired input: one or more repository-relative seed PATHS.\n\nExamples:\n\n  gitscry related src/lib.rs\n\n  gitscry related src/cli.rs src/render.rs"
    )]
    Related {
        /// Repository-relative seed paths matched against history.
        #[arg(required = true, num_args = 1..)]
        paths: Vec<String>,
        /// Maximum number of matches to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
    },

    #[command(
        about = "Find current test paths historically changed alongside one or more seed paths",
        long_about = "Find current test paths historically changed alongside one or more seed paths.\n\nUse `gitscry tests` when you changed code and want the test files that historically changed with it: it returns existing test paths ranked by co-change history.\n\nRequired input: one or more repository-relative seed PATHS.\n\nExamples:\n\n  gitscry tests src/lib.rs"
    )]
    Tests {
        /// Repository-relative seed paths matched against history.
        #[arg(required = true, num_args = 1..)]
        paths: Vec<String>,
        /// Maximum number of matches to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
    },

    #[command(
        about = "Locate historical commits that may have introduced a regression",
        long_about = "Locate historical commits that may have introduced a regression.\n\nUse `gitscry regression` when a regression is observable and you want suspects: commits in the requested revision range whose material supports them as candidates that may have introduced the regression. Suspects are historical candidates; they do not replace an executable `git bisect`.\n\nRequired inputs: one or more SYMPTOM words and `--path`, the repository-relative path affected by the regression. `--symbol` narrows the suspect history to a symbol in that path. `--good` pins the last known good revision so suspects are limited to the good..bad range; `--bad` pins the last known bad revision and defaults to HEAD.\n\nExamples:\n\n  gitscry regression provider normalization --path src/lib.rs\n\n  gitscry regression slow startup --path src/main.rs --good v0.1.0 --symbol main"
    )]
    Regression {
        /// Words describing the observable regression symptom.
        #[arg(required = true, num_args = 1..)]
        symptom: Vec<String>,
        /// Repository-relative path affected by the regression.
        #[arg(long = "path", required = true, value_name = "PATH")]
        path: String,
        /// Symbol in the affected path to narrow the suspect history.
        #[arg(long)]
        symbol: Option<String>,
        /// Last known good revision; suspects are limited to the good..bad range.
        #[arg(long)]
        good: Option<String>,
        /// Last known bad revision; defaults to HEAD.
        #[arg(long, default_value = "HEAD")]
        bad: String,
        /// Maximum number of matches to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
    },

    #[command(group(
        ArgGroup::new("anchor")
            .required(true)
            .args(["line", "symbol"]),
    ))]
    #[command(
        about = "Explain the local history behind one line or symbol",
        long_about = "Explain the local history behind one line or symbol.\n\nUse `gitscry why` when a line or symbol raises a question and you want the history behind it: the commits whose material explains why the line or symbol looks the way it does at the target revision.\n\nRequired inputs: a repository-relative PATH and exactly one anchor, `--line` (a one-based line number) or `--symbol` (a symbol name). `--at` pins the local revision containing the target and defaults to HEAD.\n\nExamples:\n\n  gitscry why src/lib.rs --line 12\n\n  gitscry why src/lib.rs --symbol provider --at HEAD~1"
    )]
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
        /// Maximum number of matches to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
    },

    #[command(
        about = "Trace a fix back to the introducing change and observed failure",
        long_about = "Trace a fix back to the introducing change and observed failure.\n\nUse `gitscry trace-fix` when a fix commit is known and you want what it fixed: the introducing change identified from the fix's parent diff and the observed failure recorded in the fix commit message.\n\nRequired input: FIX_REVISION, a local revision containing the fix commit. `--path` narrows the material to a repository-relative path changed by the fix and may be repeated.\n\nExamples:\n\n  gitscry trace-fix HEAD\n\n  gitscry trace-fix HEAD~1 --path src/lib.rs"
    )]
    TraceFix {
        /// Local revision containing the fix commit.
        fix_revision: String,
        /// Repository-relative path changed by the fix; repeatable.
        #[arg(long = "path", value_name = "PATH")]
        paths: Vec<String>,
        /// Maximum number of matches to return.
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
