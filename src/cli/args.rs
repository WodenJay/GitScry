use clap::{ArgGroup, Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    bin_name = "gitscry",
    name = "gitscry",
    version,
    color = clap::ColorChoice::Never,
    help_template = root_help_template(),
)]
pub(super) struct Cli {
    #[command(subcommand)]
    pub(super) command: Command,
}

const TESTS_ABOUT: &str = "Find current tests changed alongside files, a line, or a symbol";
// Keep the overview grouped without rebuilding every command for help generation.
fn root_help_template() -> String {
    use std::fmt::Write;

    const MANAGEMENT: &[&str] = &["index", "prune", "clear", "update", "stats", "help"];
    let mut command = Command::augment_subcommands(clap::Command::new("gitscry"));
    command.build();
    let commands: Vec<_> = command
        .get_subcommands()
        .filter(|subcommand| !subcommand.is_hide_set())
        .collect();
    let width = commands
        .iter()
        .map(|subcommand| subcommand.get_name().len())
        .max()
        .unwrap_or(0);
    let mut template = String::from("{usage-heading} {usage}\n");
    for (heading, management) in [("Query commands", false), ("Management commands", true)] {
        writeln!(template, "\n{heading}:").expect("write help heading");
        for subcommand in &commands {
            if MANAGEMENT.contains(&subcommand.get_name()) == management {
                writeln!(
                    template,
                    "  {:width$}  {}",
                    subcommand.get_name(),
                    subcommand.get_about().expect("command description")
                )
                .expect("write help template");
            }
        }
    }
    template.push_str("\nOptions:\n{options}\nRun `gitscry <command> --help` for arguments, options, and examples.\n");
    template
}
#[derive(Debug, Args, Default)]
pub(crate) struct HistoricalScopeArgs {
    /// Exclude this commit and its ancestors.
    #[arg(long = "from-rev", value_name = "REV")]
    pub(crate) from_rev: Option<String>,
    /// Include this commit and its ancestors; defaults to the target revision.
    #[arg(long = "to-rev", value_name = "REV")]
    pub(crate) to_rev: Option<String>,
    /// Include commits at or after this UTC date or RFC 3339 timestamp with an offset.
    #[arg(long, value_name = "DATE_OR_TIMESTAMP")]
    pub(crate) since: Option<String>,
    /// Include commits through this UTC date or RFC 3339 timestamp with an offset.
    #[arg(long, value_name = "DATE_OR_TIMESTAMP")]
    pub(crate) until: Option<String>,
}

#[derive(Debug, Args, Default)]
pub(crate) struct GithubLinkArgs {
    /// Fetch PRs and linked issues for returned commits; requires gh login.
    #[arg(long)]
    pub(crate) github_links: bool,
    /// Repository for --github-links; defaults to the unique github.com remote.
    #[arg(long = "github-repo", value_name = "OWNER/REPO")]
    pub(crate) github_repo: Option<String>,
}

#[derive(Debug, Args, Default)]
pub(crate) struct TestsTargetArgs {
    /// Track changes to this one-based line number.
    #[arg(long, value_parser = parse_line)]
    pub(crate) line: Option<usize>,
    /// Select actual changes throughout one uniquely resolved symbol in a file.
    #[arg(long)]
    pub(crate) symbol: Option<String>,
    /// Revision containing the selected target; defaults to current HEAD.
    #[arg(long, requires = "tests_anchor", value_name = "REV")]
    pub(crate) at: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    #[command(
        about = "Find historical file changes for each side of a merge conflict",
        after_help = r#"Requires an ordinary two-side merge with one merge base and an initialized cache
(`gitscry index`). Refreshes local history from both merge endpoints. Text and
regular file-level conflicts are supported; binary and non-regular files are skipped.

Examples:
  gitscry conflicts
  gitscry conflicts --path src/lib.rs --limit 3 --json"#
    )]
    Conflicts {
        /// Exact repository-relative conflict path; repeatable.
        #[arg(long = "path", value_name = "PATH")]
        paths: Vec<String>,
        /// Maximum historical leads per file and side.
        #[arg(long, default_value_t = 5, value_parser = parse_limit)]
        limit: usize,
        /// Maximum for historical merge-case replay checks; zero disables them.
        #[arg(long, value_name = "CHECKS", value_parser = parse_zeroable_limit)]
        max_historical_checks: Option<usize>,
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    #[command(
        about = "Find later reverts and changes to a commit's files or added lines",
        after_help = r#"Run `gitscry index` first. Queries refresh locally available HEAD history; index
another branch before using an uncached revision from it. The seed must be an
ancestor of the endpoint. Only its descendants are inspected.

Results group explicit revert references, overlapping changed lines, then other
same-file changes. Deleted seed lines have no tracked region. Detected renames
are followed; incomplete patches reduce tracking to file-level matches.

The time window ends at the seed's committer time plus --days. --max-commits bounds
inspection, including nonmatches; --limit bounds displayed results. Incomplete
history and exhausted budgets are reported.

Examples:
  gitscry followups HEAD~10
  gitscry followups v1.0 --path src/lib.rs --to-rev release --patch
  gitscry followups HEAD~10 --days 180 --max-commits 4000 --limit 40 --json"#
    )]
    Followups {
        /// Seed revision; must be available after refresh or already published.
        revision: String,
        /// Repeatable exact repository-relative original changed path.
        #[arg(long = "path")]
        paths: Vec<String>,
        /// Endpoint available after refresh; defaults to latest cached commit reachable from pinned current HEAD.
        #[arg(long, value_name = "REV")]
        to_rev: Option<String>,
        /// Inclusive observation ceiling in days; positive integer.
        #[arg(long, default_value = "90", value_parser = parse_limit)]
        days: usize,
        /// Eligible candidate inspection budget; positive integer.
        #[arg(long, default_value = "2000", value_parser = parse_limit)]
        max_commits: usize,
        /// Displayed associated commit limit; positive integer.
        #[arg(long, default_value = "20", value_parser = parse_limit)]
        limit: usize,
        /// Include bounded supporting cached patches.
        #[arg(long)]
        patch: bool,
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    #[command(
        about = "Rank files by commit count and added/deleted lines",
        after_help = r#"Run `gitscry index` first. Queries refresh locally available HEAD history; index
another branch before selecting an uncached --to-rev from it.

Ranks files present at the target revision (HEAD by default). Non-merge commits
count, including branch history and file introductions. Detected renames preserve
file identity; deletion and recreation start a new history. Partial line counts
are marked with *, unavailable counts with an em dash.

Revision and time bounds intersect; --from-rev must be an ancestor of the target.
Dates use YYYY-MM-DD; timestamps require Z or an explicit UTC offset.

Examples:
  gitscry hotspots
  gitscry hotspots --path-prefix src --since 2025-01-01 --limit 10 --json"#
    )]
    Hotspots {
        /// Maximum number of files to return.
        #[arg(long, default_value = "20", value_parser = parse_limit)]
        limit: usize,
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
        /// Literal repository-relative directory, not a glob.
        #[arg(long, value_name = "DIR")]
        path_prefix: Option<String>,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
    },
    #[command(
        about = "Find historical code matches, abandoned changes, related files and tests",
        after_help = r#"Run inside a worktree. By default, compares HEAD with the working tree and includes
unignored untracked files. Unresolved conflicts fail; unchanged input returns no
results without needing a cache.

For changed input, run `gitscry index` first. Queries refresh locally available
HEAD history. Revision and time filters narrow history, not the current change;
--from-rev must be an ancestor of --to-rev (HEAD by default). Dates use YYYY-MM-DD;
timestamps require Z or an explicit UTC offset.

Matches changed code against historical added/removed lines and finds files that
changed together or shortly afterward. Test results are existing worktree files.
Results include historical excerpts and supporting commits; skipped content and
incomplete history are reported.

Content-origin follow-up observes later same-file changes within 20 parent edges.
Its controls do not affect the separate seven-day path-level follow-on analysis.
Each category has at most three results. --limit changes the total only.

Examples:
  gitscry context
  gitscry context --staged --json --limit 4
  gitscry context --hybrid --from-rev v1.0 --until 2025-01-31"#
    )]
    Context {
        /// Select HEAD-to-index paths/status only.
        #[arg(long)]
        staged: bool,
        /// Add semantic candidates; first run gitscry index --semantic.
        #[arg(long)]
        hybrid: bool,
        /// Disable content-origin follow-up only; path-level analysis remains active.
        #[arg(long)]
        no_historical_followup: bool,
        /// Content-origin follow-up window in days; path-level window stays seven days.
        #[arg(long = "followup-days", default_value = "7", value_parser = parse_followup_days)]
        followup_days: usize,
        /// Positive maximum for content-origin candidate and baseline checks only.
        #[arg(long, value_name = "CHECKS", value_parser = parse_limit)]
        max_followup_checks: Option<usize>,
        /// Total ceiling; the three-per-category ceiling remains fixed.
        #[arg(long, default_value = "8", value_parser = parse_limit)]
        limit: usize,
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
    },
    #[command(
        about = "Show local command counts and elapsed time",
        after_help = r#"Works outside a repository. Includes completed commands, including failures;
excludes help, version, parse errors, and stats itself.

Defaults to the last 30 local calendar days, including today. When either date
bound is supplied, an omitted start includes all earlier history and an omitted
end means today. Partial trend buckets include only the selected dates.

Usage is stored locally in:
  Windows: %LOCALAPPDATA%\GitScry\usage.sqlite
  macOS:   ~/Library/Application Support/GitScry/usage.sqlite
  Linux:   $XDG_DATA_HOME/gitscry/usage.sqlite (default ~/.local/share/gitscry/usage.sqlite)
Delete this database to reset the report.

Examples:
  gitscry stats
  gitscry stats --since 2025-01-01 --group week
  gitscry stats --all --json"#
    )]
    Stats {
        /// Include every recorded date; conflicts with --since and --until.
        #[arg(long, conflicts_with_all = ["since", "until"])]
        all: bool,
        /// Inclusive local calendar start date.
        #[arg(long, value_name = "YYYY-MM-DD")]
        since: Option<String>,
        /// Inclusive local calendar end date; defaults to today.
        #[arg(long, value_name = "YYYY-MM-DD")]
        until: Option<String>,
        /// Trend buckets: day, week (starting Monday), or month.
        #[arg(long, value_enum, default_value = "day")]
        group: StatsGroup,
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
    },

    #[command(
        about = "Update GitScry to the latest stable release",
        after_help = "Downloads a newer release from GitHub, verifies its SHA-256 checksum, and\nreplaces this executable. Alias: gitscry upgrade.\n\nExample:\n  gitscry update",
        alias = "upgrade"
    )]
    Update,

    #[command(
        group(
            ArgGroup::new("trace-removal-mode")
                .required(true)
                .multiple(false)
                .args(["code", "code_file"]),
        ),
        about = "Find commits that deleted matching code",
        after_help = r#"Choose exactly one input: --code or --code-file. Only removed lines are searched.
Fragment matching treats LF and CRLF equally; all other bytes are literal.

Run `gitscry index` first. Queries refresh locally available HEAD history; index
another branch before querying an uncached revision from it. Revision and time
bounds intersect; --from-rev must be an ancestor of --to-rev (HEAD by default).
Dates use YYYY-MM-DD; timestamps require Z or an explicit UTC offset.

Results group matches by removal commit and historical path, newest first.
Merge diffs compare with the first parent. Missing history is reported.

Examples:
  gitscry trace-removal --code 'legacy()'
  gitscry trace-removal --code 'old.key = ' --path config.rs --to-rev v1.0 --json
  gitscry trace-removal --code-file removed-block.txt --limit 5"#
    )]
    TraceRemoval {
        /// Non-empty, single-line, case-sensitive literal substring in deleted lines.
        #[arg(long, value_name = "TEXT", value_parser = parse_code_query)]
        code: Option<String>,
        /// Exact whole-line fragment from a file; - reads stdin through EOF.
        #[arg(long, value_name = "PATH")]
        code_file: Option<std::path::PathBuf>,
        /// Exact historical old path; does not follow aliases.
        #[arg(long, value_name = "PATH")]
        path: Option<String>,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
        /// Maximum deletion events, not matching lines.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    #[command(
        about = "Build or refresh the repository history cache",
        after_help = r#"Adds locally available history reachable from HEAD. Previously cached commits
remain available across branch switches; fetch remote history with Git first.
The cache is shared by linked worktrees under the Git common directory's gitscry/.

Semantic indexing is off by default. Enabling it saves the choice for later index
runs and may download about 90 MB of shared model resources. Offline use requires
compatible local model resources and ONNX Runtime 1.23.2. Semantic setup failures
leave the ordinary history cache usable. Queries use local resources only.

Examples:
  gitscry index
  gitscry index --semantic
  gitscry index --no-semantic"#
    )]
    Index {
        /// Generate and maintain the local semantic index.
        #[arg(long, conflicts_with = "no_semantic")]
        semantic: bool,
        /// Disable semantic indexing and remove all repository semantic vectors.
        #[arg(long, conflicts_with = "semantic")]
        no_semantic: bool,
    },

    #[command(
        about = "Delete the repository's GitScry cache and settings",
        after_help = r#"Affects every linked worktree and waits for active cache users. Git history,
usage statistics, and shared model/runtime resources are kept. Rebuild the cache
with `gitscry index`.

Reports deleted records and before/after cache file sizes.

Examples:
  gitscry clear --dry-run
  gitscry clear"#
    )]
    Clear {
        /// Preview planned deletions without changing cache data.
        #[arg(long)]
        dry_run: bool,
    },

    #[command(
        about = "Remove cached commits whose Git objects no longer exist locally",
        after_help = r#"Keeps commits whose objects Git still retains, even after branch deletion or
rebase. Does not run Git GC. Affects the shared cache for all linked worktrees
and waits for active cache users.

Reports deleted records and before/after cache file sizes. Preview reports
candidate counts; reclaimed bytes are measured only when pruning runs.

Examples:
  gitscry prune --dry-run
  gitscry prune"#
    )]
    Prune {
        /// Preview eligible deletions without changing cache data.
        #[arg(long)]
        dry_run: bool,
    },

    #[command(
        group(
            ArgGroup::new("search-mode")
                .required(true)
                .multiple(false)
                .args(["query", "code", "code_regex", "code_file"]),
        ),
        group(
            ArgGroup::new("code-mode")
                .multiple(false)
                .args(["code", "code_regex", "code_file"]),
        ),
        about = "Search commit messages, paths, or changed code",
        after_help = r#"Choose exactly one input: QUERY words, --code, --code-regex, or --code-file.
Code modes search added/removed lines only, not unchanged context. --change and
--path apply to code modes; --hybrid and --patch apply to QUERY mode only.

Regex uses Rust regex syntax, one changed line at a time. Matching is Unicode-aware
and case-sensitive; (?i) ignores case. Look-around and backreferences are unsupported.
Patterns must be nonempty, single-line, and at most 16,384 UTF-8 bytes.

File fragments match whole consecutive lines in one hunk and one change direction.
LF and CRLF are equivalent; other bytes are literal. Matched content is included.

Run `gitscry index` first. Queries refresh locally available HEAD history; index
another branch before querying an uncached revision from it. Revision and time
bounds intersect and apply before ranking; --from-rev must be an ancestor of
--to-rev (HEAD by default). Dates use YYYY-MM-DD; timestamps require Z or an explicit
UTC offset. Missing history is reported. --limit bounds output, not scan work.

Examples:
  gitscry search retry backoff --from-rev v1.0 --to-rev release
  gitscry search --code 'unwrap()?' --since 2025-01-01 --limit 5
  gitscry search --code-regex 'Old[A-Z][A-Za-z0-9_]*' --change removed --path src/lib.rs
  gitscry search --code-file change.txt --github-links --json"#
    )]
    Search {
        /// Query words matched against commit subjects, bodies, and touched paths.
        #[arg(num_args = 1..)]
        query: Option<Vec<String>>,
        /// Add semantic search; first run gitscry index --semantic.
        #[arg(long, requires = "query", conflicts_with_all = ["code", "code_regex"])]
        hybrid: bool,
        /// Case-sensitive literal substring matched on changed lines.
        #[arg(long, value_name = "TEXT", value_parser = parse_code_query)]
        code: Option<String>,
        /// Regex matched independently against each changed line.
        #[arg(long, value_name = "PATTERN")]
        code_regex: Option<String>,
        /// Exact whole-line fragment from a file; - reads stdin through EOF.
        #[arg(long, value_name = "PATH")]
        code_file: Option<std::path::PathBuf>,
        /// Restrict code matches to added or removed lines.
        #[arg(long, value_enum, requires = "code-mode", conflicts_with = "query")]
        change: Option<CodeChange>,
        /// Exact historical path; additions use the new path, removals the old path.
        #[arg(
            long = "path",
            value_name = "PATH",
            requires = "code-mode",
            conflicts_with = "query"
        )]
        path: Option<String>,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
        /// Maximum number of matching commits, changed lines, or fragment occurrences to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
        /// Include bounded relevant cached text hunks; unavailable history is reported.
        #[arg(long, requires = "query", conflicts_with_all = ["code", "code_regex"])]
        patch: bool,
        /// Fetch PRs and linked issues for returned commits; requires gh login.
        #[arg(long)]
        github_links: bool,
        /// Repository for --github-links; defaults to the unique github.com remote.
        #[arg(long = "github-repo", value_name = "OWNER/REPO")]
        github_repo: Option<String>,
    },

    #[command(
        about = "Find past implementations of a similar change or migration",
        after_help = r#"Returns matching commits with change steps and touched paths.

Run `gitscry index` first. Queries refresh locally available HEAD history; index
another branch before querying an uncached revision from it. Revision and time
bounds intersect and apply before ranking; --from-rev must be an ancestor of
--to-rev (HEAD by default). Dates use YYYY-MM-DD; timestamps require Z or an explicit
UTC offset. Missing history is reported.

Examples:
  gitscry examples retry backoff --from-rev v1.2.0 --to-rev v1.3.0
  gitscry examples retire provider --since 2025-01-01 --path src/lib.rs --patch"#
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
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
        /// Include bounded cached text hunks from paths matching this query.
        #[arg(long)]
        patch: bool,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
        #[command(flatten)]
        github: GithubLinkArgs,
    },

    #[command(
        about = "Find reverted or abandoned approaches and their recorded reasons",
        after_help = r#"Returns reverted or superseded commits, with reasons and retry conditions when
recorded in commit messages.

Run `gitscry index` first. Queries refresh locally available HEAD history; index
another branch before querying an uncached revision from it. Revision and time
bounds intersect and apply before ranking; --from-rev must be an ancestor of
--to-rev (HEAD by default). Dates use YYYY-MM-DD; timestamps require Z or an explicit
UTC offset. Missing history is reported.

Examples:
  gitscry failures provider normalization --from-rev v1.2.0 --to-rev v1.3.0
  gitscry failures parser migration --since 2025-01-01T09:00:00-05:00 --json"#
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
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
        #[command(flatten)]
        github: GithubLinkArgs,
    },

    #[command(
        about = "Find files changed with or shortly after seed files or directories",
        after_help = r#"Run `gitscry index` first. Queries refresh locally available HEAD history; index
another branch before querying an uncached revision from it. Revision and time
bounds intersect; --from-rev must be an ancestor of --to-rev (HEAD by default).
Dates use YYYY-MM-DD; timestamps require Z or an explicit UTC offset.

Default mode finds paths changed in the same commits or within seven days and
20 parent edges afterward. Existing directories are recognized automatically;
a trailing / selects a directory even if deleted. Directory matching is recursive.
File and directory inputs can be mixed; matched seed paths are excluded from results.

Line/symbol mode requires exactly one file. --at selects target content independently
of historical scope. Only actual changes to the target count, with first-parent
tracking; uncertain history is reported instead of falling back to whole-file matches.
This mode omits file-level follow-on results.

Pattern mode requires file seeds, not directories, and returns groups of at least
three files changed together in commits touching every seed. Equal-support subsets
are omitted in favor of larger groups. Default minimum support is three commits.
Cannot combine with line/symbol mode or GitHub link options. --limit caps groups,
not members or analysis work. Merges and commits changing over 50 paths are excluded.

Examples:
  gitscry related src/lib.rs src/cli.rs
  gitscry related src/analysis/ --json
  gitscry related src/lib.rs --line 12 --at release
  gitscry related src/a.rs src/b.rs --patterns --min-support 2"#
    )]
    #[command(
        group(
            ArgGroup::new("related_anchor")
                .args(["line", "symbol"])
                .multiple(false),
        ),
    )]
    Related {
        /// Repository-relative seed paths matched against history.
        #[arg(required = true, num_args = 1..)]
        paths: Vec<String>,
        /// Select actual changed-line history for one exact file (not its enclosing function).
        #[arg(long, conflicts_with = "patterns")]
        line: Option<usize>,
        /// Select actual changes throughout one uniquely resolved symbol.
        #[arg(long, conflicts_with = "patterns")]
        symbol: Option<String>,
        /// Resolve the selected target at this revision (default HEAD), independently of historical scope.
        #[arg(long, requires = "related_anchor")]
        at: Option<String>,
        /// Find groups of files repeatedly changed together with every seed.
        #[arg(long, conflicts_with_all = ["github_links", "github_repo"])]
        patterns: bool,
        /// Minimum distinct supporting commits (at least two); pattern mode only.
        #[arg(long, requires = "patterns", value_parser = parse_min_support)]
        min_support: Option<usize>,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
        #[command(flatten)]
        github: GithubLinkArgs,
        /// Maximum number of matches to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
    },

    #[command(
        group(
            ArgGroup::new("tests_anchor")
                .args(["line", "symbol"])
                .multiple(false),
        ),
        about = TESTS_ABOUT,
        after_help = r#"Run `gitscry index` first. Queries refresh locally available HEAD history; index
another branch before querying an uncached revision from it. Revision and time
bounds intersect; --from-rev must be an ancestor of --to-rev (HEAD by default).
Dates use YYYY-MM-DD; timestamps require Z or an explicit UTC offset.

Default mode uses commits touching any seed. Line/symbol mode requires exactly
one file and counts only actual changes to that target. --at selects target
content independently of historical scope. Uncertain target history is reported
instead of falling back to whole-file matches.

Results are test paths currently present in the worktree, even when historical
scope ends earlier. Deleted test paths are omitted.

Examples:
  gitscry tests src/lib.rs --line 12
  gitscry tests src/engine.rs --symbol Engine::run --at release --from-rev v1.0
  gitscry tests src/lib.rs src/cli.rs --to-rev release"#
    )]
    Tests {
        /// Repository-relative seed paths matched against history.
        #[arg(required = true, num_args = 1..)]
        paths: Vec<String>,
        #[command(flatten)]
        target: TestsTargetArgs,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
        #[command(flatten)]
        github: GithubLinkArgs,
        /// Maximum number of matches to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
    },

    #[command(
        about = "Find commits that may have introduced a regression",
        after_help = r#"Run `gitscry index` first. Queries refresh locally available HEAD history; index
another branch before selecting an uncached revision from it.

Revision and time filters narrow the suspect window without changing --bad.
--to-rev defaults to --bad; --from-rev must be an ancestor of that upper bound.
Dates use YYYY-MM-DD; timestamps require Z or an explicit UTC offset.

Examples:
  gitscry regression provider normalization --path src/lib.rs
  gitscry regression slow startup --path src/main.rs --good v0.1.0 --symbol main
  gitscry regression timeout --path src/app.py --good v0.2 --bad v0.3 --patch"#
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
        /// Last known bad revision; defaults to current HEAD; explicit targets must be available after refresh or already published.
        #[arg(long, value_name = "REV")]
        bad: Option<String>,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
        #[command(flatten)]
        github: GithubLinkArgs,
        /// Maximum number of matches to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
        /// Include bounded cached hunks matching the symptom or selected symbol.
        #[arg(long)]
        patch: bool,
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
    },

    #[command(group(
        ArgGroup::new("anchor")
            .required(true)
            .args(["line", "symbol"]),
    ))]
    #[command(
        about = "Show blame attribution and changes to a line or symbol",
        after_help = r#"Choose exactly one target: --line or --symbol. Attribution uses the target's
starting line; modification history tracks the full selected range. Changes are
traced through first-parent history, including merges.

Run `gitscry index` first. Queries refresh locally available HEAD history; index
another branch before selecting an uncached --at revision from it. Historical
bounds intersect with the target's history; --to-rev defaults to --at (HEAD if
omitted). --from-rev must be an ancestor of the upper bound. Dates use YYYY-MM-DD;
timestamps require Z or an explicit UTC offset. Missing history is reported.

Examples:
  gitscry why src/lib.rs --line 12
  gitscry why src/lib.rs --symbol provider --at HEAD~1 --from-rev HEAD~5 --patch
  gitscry why src/lib.rs --line 12 --since 2025-01-01 --until 2025-01-31"#
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
        /// Local revision containing the target; defaults to current HEAD. Explicit targets must be available after refresh or already published.
        #[arg(long, value_name = "REV")]
        at: Option<String>,
        /// Exclude this revision and its ancestors from eligible history; it must be available after refresh or already published.
        #[arg(long = "from-rev", value_name = "REV")]
        from_rev: Option<String>,
        /// Include this revision and its ancestors, intersected with the target; it must be available after refresh or already published.
        #[arg(long = "to-rev", value_name = "REV")]
        to_rev: Option<String>,
        /// Include commits at or after this UTC date or RFC 3339 timestamp.
        #[arg(long, value_name = "DATE_OR_TIMESTAMP")]
        since: Option<String>,
        /// Include commits through this UTC date or RFC 3339 timestamp.
        #[arg(long, value_name = "DATE_OR_TIMESTAMP")]
        until: Option<String>,
        /// Maximum standalone target-related modifications to return; attribution is independent.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
        /// Include bounded cached hunks tied to the target line or symbol.
        #[arg(long)]
        patch: bool,
        #[command(flatten)]
        github: GithubLinkArgs,
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
    },

    #[command(
        about = "Trace a fix to earlier changes and its recorded failure",
        after_help = r#"Uses the fix's parent diff to identify earlier changes and its commit message
for the recorded failure.

Run `gitscry index` first. Queries refresh locally available HEAD history; index
another branch before selecting an uncached fix revision from it. Historical
filters narrow earlier-change candidates without changing the fix target.
--to-rev defaults to the fix revision; --from-rev must be an ancestor of the upper
bound. Dates use YYYY-MM-DD; timestamps require Z or an explicit UTC offset.
Missing history is reported.

Examples:
  gitscry trace-fix HEAD
  gitscry trace-fix HEAD~1 --path src/lib.rs --patch
  gitscry trace-fix HEAD --from-rev HEAD~5 --since 2025-01-01"#
    )]
    TraceFix {
        /// Local revision containing the fix commit.
        fix_revision: String,
        /// Repository-relative path changed by the fix; repeatable.
        #[arg(long = "path", value_name = "PATH")]
        paths: Vec<String>,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
        #[command(flatten)]
        github: GithubLinkArgs,
        /// Maximum number of matches to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
        /// Include bounded cached hunks attributed to the introducing change.
        #[arg(long)]
        patch: bool,
    },
    #[command(
        about = "Show a file's history from earliest to latest",
        after_help = r#"Run `gitscry index` first. Queries refresh locally available HEAD history; index
another branch before selecting an uncached --at revision from it. The path must
be a file at the target revision.

Includes cached branch history reachable from the target, ordered topologically.
Detected renames are followed; copies and earlier histories before deletion and
recreation are separate. Merge entries compare with the first parent.

Revision and time bounds intersect with the target's history before pagination.
--from-rev must be an ancestor of --to-rev (the target by default). Dates use
YYYY-MM-DD; timestamps require Z or an explicit UTC offset. Missing history is
reported.

Examples:
  gitscry timeline src/lib.rs
  gitscry timeline src/lib.rs --at release --last --patch
  gitscry timeline src/lib.rs --since 2025-01-01 --limit 5 --offset 5 --json"#
    )]
    Timeline {
        /// Repository-relative file path at the selected revision.
        path: String,
        /// Revision containing the file; defaults to current HEAD. Explicit targets must be available after refresh or already cached.
        #[arg(long, value_name = "REV")]
        at: Option<String>,
        /// Exclude this revision and its ancestors from eligible history; it must be available after refresh or already published.
        #[arg(long = "from-rev", value_name = "REV")]
        from_rev: Option<String>,
        /// Include this revision and its ancestors, intersected with the target; it must be available after refresh or already published.
        #[arg(long = "to-rev", value_name = "REV")]
        to_rev: Option<String>,
        /// Include commits at or after this UTC date or RFC 3339 timestamp.
        #[arg(long, value_name = "DATE_OR_TIMESTAMP")]
        since: Option<String>,
        /// Include commits through this UTC date or RFC 3339 timestamp.
        #[arg(long, value_name = "DATE_OR_TIMESTAMP")]
        until: Option<String>,
        /// Maximum number of timeline entries to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
        /// Zero-based offset into the complete timeline.
        #[arg(long, conflicts_with = "last", value_parser = parse_offset)]
        offset: Option<usize>,
        /// Jump to the final page; conflicts with --offset.
        #[arg(long, conflicts_with = "offset")]
        last: bool,
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
        /// Include bounded cached text hunks for each timeline entry.
        #[arg(long)]
        patch: bool,
        /// Fetch PRs and linked issues for this page's commits; requires gh login.
        #[arg(long)]
        github_links: bool,
        /// Repository for --github-links; defaults to the unique github.com remote.
        #[arg(long = "github-repo", value_name = "OWNER/REPO")]
        github_repo: Option<String>,
    },
    #[command(
        about = "Check whether a commit is contained in specific target branches",
        after_help = r#"Requires an initialized cache (`gitscry index`). Refreshes local history from the
requested targets only; never fetches, scans unrelated branches, or moves HEAD.
Source and targets are resolved to commits once at invocation start.

A source commit reachable from a target, including through merged
non-first-parent ancestry, is `contained`. Exact non-reachability is not a
completed negative search: patch equivalence is not checked yet, so unmatched
targets stay `indeterminate` with that reason.

Examples:
  gitscry propagation 4ca5b49 --to main --to release-1.0
  gitscry propagation HEAD~3 --to origin/main --json"#
    )]
    Propagation {
        /// Source commit whose propagation is checked.
        source: String,
        /// Target ref to check containment against; repeatable, order preserved.
        #[arg(long = "to", value_name = "REF")]
        targets: Vec<String>,
        /// Output JSON instead of text.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum StatsGroup {
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum CodeChange {
    Added,
    Removed,
}

impl Command {
    pub(crate) fn uses_json(&self) -> bool {
        match self {
            Self::Context { json, .. }
            | Self::Conflicts { json, .. }
            | Self::Search { json, .. }
            | Self::Examples { json, .. }
            | Self::Failures { json, .. }
            | Self::Related { json, .. }
            | Self::Tests { json, .. }
            | Self::Regression { json, .. }
            | Self::Why { json, .. }
            | Self::TraceFix { json, .. }
            | Self::Followups { json, .. }
            | Self::TraceRemoval { json, .. }
            | Self::Hotspots { json, .. }
            | Self::Timeline { json, .. }
            | Self::Propagation { json, .. }
            | Self::Stats { json, .. } => *json,
            Self::Clear { .. } | Self::Prune { .. } => false,
            Self::Update | Self::Index { .. } => false,
        }
    }

    pub(crate) fn usage_name(&self) -> Option<&'static str> {
        match self {
            Self::Stats { .. } => None,
            Self::Followups { .. } => Some("followups"),
            Self::Hotspots { .. } => Some("hotspots"),
            Self::Context { .. } => Some("context"),
            Self::Conflicts { .. } => Some("conflicts"),
            Self::Update => Some("update"),
            Self::TraceRemoval { .. } => Some("trace-removal"),
            Self::Index { .. } => Some("index"),
            Self::Clear { .. } => Some("clear"),
            Self::Prune { .. } => Some("prune"),
            Self::Search { .. } => Some("search"),
            Self::Examples { .. } => Some("examples"),
            Self::Failures { .. } => Some("failures"),
            Self::Related { .. } => Some("related"),
            Self::Tests { .. } => Some("tests"),
            Self::Regression { .. } => Some("regression"),
            Self::Why { .. } => Some("why"),
            Self::TraceFix { .. } => Some("trace-fix"),
            Self::Timeline { .. } => Some("timeline"),
            Self::Propagation { .. } => Some("propagation"),
        }
    }
}

fn parse_min_support(value: &str) -> Result<usize, String> {
    let count = parse_limit(value)?;
    if count < 2 {
        return Err("minimum support must be at least two".to_owned());
    }
    Ok(count)
}

fn parse_line(value: &str) -> Result<usize, String> {
    let line = value
        .parse::<usize>()
        .map_err(|_| "line must be a positive integer".to_owned())?;
    (line > 0)
        .then_some(line)
        .ok_or_else(|| "line must be greater than zero".to_owned())
}

fn parse_offset(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| "offset must be a non-negative integer".to_owned())
}

fn parse_limit(value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| "limit must be a positive integer".to_owned())?;
    (limit > 0)
        .then_some(limit)
        .ok_or_else(|| "limit must be greater than zero".to_owned())
}

/// Like `parse_limit` but admits zero, for budgets that fully disable a
/// bounded phase while keeping its reporting.
fn parse_zeroable_limit(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| "limit must be a non-negative integer".to_owned())
}

fn parse_followup_days(value: &str) -> Result<usize, String> {
    const SECONDS_PER_DAY: i64 = 24 * 60 * 60;
    let days = value
        .parse::<usize>()
        .map_err(|_| "follow-up days must be a positive representable integer".to_owned())?;
    if days == 0
        || i64::try_from(days)
            .ok()
            .and_then(|days| days.checked_mul(SECONDS_PER_DAY))
            .is_none()
    {
        return Err("follow-up days must be positive and fit a signed timestamp window".to_owned());
    }
    Ok(days)
}

fn parse_code_query(value: &str) -> Result<String, String> {
    if value.is_empty()
        || value
            .chars()
            .any(|character| matches!(character, '\n' | '\r'))
    {
        return Err("code query must be a non-empty single line".to_owned());
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_is_progressive_and_identical_for_short_and_long_flags() {
        use clap::CommandFactory;

        let mut root = Cli::command();
        assert_eq!(
            root.render_help()
                .to_string()
                .split_whitespace()
                .collect::<Vec<_>>(),
            root.render_long_help()
                .to_string()
                .split_whitespace()
                .collect::<Vec<_>>()
        );
        let overview = root.render_help().to_string();
        assert!(overview.contains("gitscry <command> --help"));
        assert!(!overview.contains("--semantic"));
        for command in root.get_subcommands_mut() {
            assert_eq!(
                command
                    .render_help()
                    .to_string()
                    .split_whitespace()
                    .collect::<Vec<_>>(),
                command
                    .render_long_help()
                    .to_string()
                    .split_whitespace()
                    .collect::<Vec<_>>(),
                "{}",
                command.get_name()
            );
            if command.get_name() != "help" {
                let help = command.render_help().to_string();
                assert!(
                    help.find("Usage:").unwrap()
                        < help
                            .find("Examples:")
                            .or_else(|| help.find("Example:"))
                            .unwrap()
                );
            }
        }
    }

    #[test]
    fn update_command_accepts_canonical_name_and_alias() {
        for name in ["update", "upgrade"] {
            let cli = Cli::try_parse_from(["gitscry", name]).unwrap();
            assert_eq!(cli.command.usage_name(), Some("update"));
            assert!(matches!(cli.command, Command::Update));
        }
    }

    #[test]
    fn followup_days_require_a_positive_representable_window() {
        assert_eq!(parse_followup_days("7"), Ok(7));
        assert!(parse_followup_days("0").is_err());
        assert!(parse_followup_days("106751991168000").is_err());
    }
}
