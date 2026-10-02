use clap::{ArgGroup, Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    bin_name = "gitscry",
    name = "gitscry",
    version,
    color = clap::ColorChoice::Never,
    before_help = ROOT_LONG_HELP,
)]
pub(super) struct Cli {
    #[command(subcommand)]
    pub(super) command: Command,
}

// `gitscry --help` is the top-level overview; keep command `about`s concise and put details
// in each command's `long_about` for the `gitscry <command> --help` entry point.
const ROOT_LONG_HELP: &str = "\
Your Git history is a treasure trove. GitScry uncovers the implementation examples, failed approaches, code relationships, and regression context hidden inside.

Pick one command by intent, then run `gitscry <command> --help` for inputs, options, and examples.\n\nQuery commands support `--json` for structured output.";

const MATERIAL_LINKS_LONG_HELP: &str = "`--github-links` walks explicit commit→PR→issue associations for returned material through the authenticated `gh` CLI. It uses coverage-first pagination: all eligible commit PR homepages and discovered PR issue homepages precede continuation pages. Both connections share a 15-second timeout, 20 API requests, 50 results per page, and 200 deduplicated PR+issue objects. These fixed limits are conservative starting values, not empirically optimized. Human and JSON output mark each layer complete, partial, not queried, or failed; missing or unreturned associations do not prove none exist. Partial data and local object/field errors preserve usable links and continue other lookups; global authentication, rate-limit, network, or timeout failures stop link fetching only and preserve Git materials and command exit status. No automatic retries. Issue links are explicit `closingIssuesReferences`, not arbitrary mentions, and are navigation evidence, not proof of closure or causality. `--github-repo OWNER/REPO` selects a repository but does not itself contact GitHub; without it, repository inference follows `search` rules. Without `--github-links`, no GitHub lookup occurs. Links add navigation only, do not change Git material, and do not promise command-specific benefits. In particular, test co-change history does not prove assertions exist, regression suspects are not root-cause findings, and participant descriptions do not outweigh changes visible in Git.";
#[derive(Debug, Args, Default)]
pub(crate) struct HistoricalScopeArgs {
    /// Exclude this cached commit and its ancestors from the query scope.
    #[arg(long = "from-rev", value_name = "REV")]
    pub(crate) from_rev: Option<String>,
    /// Include this cached commit and its ancestors; defaults to the command's effective target.
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
    /// Fetch coverage-first GitHub PR and issue associations for returned material commits; requires an authenticated gh CLI.
    #[arg(long)]
    pub(crate) github_links: bool,
    /// GitHub repository to query; does not enable link fetching by itself.
    #[arg(long = "github-repo", value_name = "OWNER/REPO")]
    pub(crate) github_repo: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    #[command(
        about = "Discover historical changes, co-changing paths and tests",
        long_about = r#"Select traceable changed-code and path-association material for the current change.

Default input is the net HEAD-to-worktree path/status difference plus Git-unignored untracked paths. Staged/unstaged cancellations disappear. --staged selects HEAD-to-index only, excluding untracked paths and unstaged edits. Run inside a worktree; paths are root-relative. Unresolved conflicts fail. Detected rename sides are retained and excluded from suggestions; detection is not exhaustive.

Changed-code material uses distinctive exact identities from individual current hunk sides and verifies them in historical added/removed lines, even with generic commit descriptions. Matches retain current/historical paths, locations and directions with short real excerpts. Opposite-direction matches are not a same-kind conclusion. Recorded abandonment, mandatory edits, coverage verdicts and required test execution are not supplied. No external symlink targets or recursive submodule contents are read; binary/non-UTF-8/oversized/unsafe content retains path/status with explicit omissions. No semantic resources or external LLM are used.

History comes from one published cache session, not necessarily current HEAD. Nonempty input needs a usable cache: run `gitscry index` explicitly; context never builds or updates it. No-change input succeeds without opening cache. Cache and coverage limitations are reported.

--from-rev excludes that cached commit and its ancestors; --to-rev includes that cached commit and its ancestors (default cache tip when scoped). The lower revision must be an ancestor of the upper revision. --since and --until filter committer time using inclusive UTC dates (YYYY-MM-DD) or RFC 3339 instants with Z or an explicit offset; timezone-free timestamps fail. Bounds combine and never change the current-change baseline. Without scope flags, all available cached history is eligible.

Path suggestions exclude every selected current path; historical commit material can concern those paths. Test-shaped candidates must exist as safe regular files in this worktree and appear once as tests, merging co-change and test-path bases. Path association history excludes merge commits and commits changing over 50 paths; content matching independently uses cached hunks. Weak associations are not filled to meet a quota; zero results is valid. Historical matching uses exact path bytes, without basename or case-fold fallback. At most 256 byte-sorted input paths are queried; all input paths retain exclusion identity and omitted retrieval is reported. Each result includes at most three historical citations with its full support count and citation truncation flag.

Default total ceiling is eight, with at most three entries per category. Positive --limit changes only the total ceiling. Distinct supported current paths order the unified list; historical changes precede tests, then co-changing files on equal strength, then latest supporting committer time and byte-sorted path break ties. --json expresses the same material, input, scope, limitations and truncation as text.

Content budgets: first 128 selected files; 256 KiB per current file/patch, 4 MiB total; 512 local hunk sides, 24 signals each. One shared historical pass reads at most 20,000 hunks/64 MiB payload text (256 KiB per hunk), retains at most 128 verified commits and 16 match excerpts per commit (240 bytes each). Limits and skipped content are reported separately from cache coverage and output truncation. Lockfiles and generated-looking paths are not blanket-excluded.

Examples:
  gitscry context
  gitscry context --staged --json --limit 4
  gitscry context --from-rev v1.0 --until 2025-01-31"#
    )]
    Context {
        /// Select HEAD-to-index paths/status only.
        #[arg(long)]
        staged: bool,
        /// Expand candidates with ready local semantic resources; never falls back.
        #[arg(long)]
        hybrid: bool,
        /// Total ceiling; the three-per-category ceiling remains fixed.
        #[arg(long, default_value = "8", value_parser = parse_limit)]
        limit: usize,
        /// Return typed structured material instead of terminal text.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
    },
    #[command(
        about = "Update GitScry to the latest stable release",
        long_about = "Update GitScry to the latest stable release.\n\nUse `gitscry update` to check GitHub for a newer release, verify its SHA-256 checksum, and replace this executable. The update is refused when the release is older than the running version.\n\nExamples:\n\n  gitscry update\n\n  gitscry upgrade",
        alias = "upgrade"
    )]
    Update,

    #[command(
        about = "Build or refresh the local history cache",
        long_about = "Build the local cache from the default branch history.\n\nUse `gitscry index` when you want to construct or refresh the local cache explicitly. The cache is a rebuildable local representation of the repository's Git history; Git remains the source of truth, and rerunning `index` rebuilds the cache from the current default branch tip.\n\nSemantic indexing is off by default; ordinary indexing without saved opt-in does not load or download a model. `--semantic` enables it and persists that choice; later `gitscry index` runs maintain the semantic index. `--no-semantic` disables it and removes semantic vectors. These options conflict. Enabling semantic indexing may download about 90 MB of pinned, hash-verified model resources shared across repositories. Subsequent indexing works offline when compatible resources and the verified ONNX Runtime 1.23.2 library are available. Missing or corrupt semantic resources or a missing/incompatible runtime return nonzero; the ordinary history cache remains usable.\n\nRequired input: none.\n\nExamples:\n\n  gitscry index\n\n  gitscry index --semantic\n\n  gitscry index --no-semantic"
    )]
    Index {
        /// Generate and maintain the local semantic index.
        #[arg(long, conflicts_with = "no_semantic")]
        semantic: bool,
        /// Disable semantic indexing and remove stored vectors.
        #[arg(long, conflicts_with = "semantic")]
        no_semantic: bool,
    },

    #[command(
        group(ArgGroup::new("search-mode").required(true).args(["query", "code"])),
        about = "Search history or literal changed-code lines",
        long_about = r#"Search the published cache's default branch history for relevant commits or literal changed-code lines.

Use `gitscry search QUERY...` to search commit subjects, bodies, and touched paths. Use `--code TEXT` to find a case-sensitive literal substring in added or removed lines from cached diffs. Code queries are non-empty, single-line text; punctuation and spaces are matched literally. Unchanged context lines are not searched.

Use `--hybrid` with text queries to merge lexical ranking with exact semantic cosine retrieval. It requires a ready semantic index; search never downloads or repairs model resources. Run `gitscry index --semantic` to enable or repair semantic coverage while online. The short-query limit is 256 total tokens, including special tokens `[CLS]` and `[SEP]`; longer queries are tokenized once into chunks of 220 content tokens with 40 tokens of overlap (180-token stride). Chunks are embedded in batches of up to 8. Each commit receives its maximum cosine similarity across chunks for one semantic ranking, fused once with the lexical ranking; the complete original query still goes to lexical search. Queries requiring more than 32 chunks are rejected before inference or semantic retrieval; shorten the query or rerun ordinary lexical search without `--hybrid`. Chunking does not guarantee that procedural context is retained or that instructions are followed. Long-query quality is not guaranteed to match concise queries. Hybrid search scans eligible vectors once and keeps `max(100, --limit)` candidates per branch. In JSON, `matched_count` is the deduplicated union of those bounded branch lists, not a corpus-wide match count; a notice reports the candidate depth.

In code mode, `--change added|removed` selects one direction and `--path PATH` matches an exact historical path: additions use the new path and removals use the old path. Rename history is not followed. No file-type filter is applied.

Scope applies to both search modes and is limited to the published cache. `--from-rev REV` excludes that commit and its ancestors; `--to-rev REV` includes that commit and its ancestors. The lower revision must be an ancestor of the upper revision. Revisions must exist in the published cache. If `--to-rev` is omitted, the effective upper revision is the cache tip.

`--since` and `--until` filter committer time. Use `YYYY-MM-DD` for an inclusive UTC calendar day, or an RFC 3339 timestamp with `Z` or an explicit UTC offset for an inclusive instant. Timezone-free timestamps are rejected. Bounds combine with revision scope, and filtering happens before ranking and `--limit`. Scoped results show the normalized bounds, resolved revisions, and effective cache tip in human and JSON output.

Run `gitscry index` to refresh the cache; incomplete or shallow history is reported as a warning. `--limit` limits matching commits in ordinary mode and matching lines in code mode. `--json` returns structured output.

`--github-links` fetches explicit `Commit.associatedPullRequests` and `PullRequest.closingIssuesReferences` for distinct complete commit IDs represented in returned text or code results. It reads PR number, title, URL, repository identity, plus issue number, title, URL, and repository identity, using the `gh` CLI's existing GitHub login; GitScry does not audit or claim a minimum permission set. Coverage comes first: request every returned commit's PR homepage, then every discovered PR's issue homepage, then continuation pages; PRs discovered later still get an issue homepage before further pagination. Both connections use opaque cursors within a shared 15-second timeout, 20-request, 50-results-per-page, 200-deduplicated-PR+issue-object budget. These fixed limits are conservative starting values, not empirically optimized. Each commit's PR layer and each PR's issue layer reports complete, partial, not queried, or failed, with a stop reason where relevant. Missing data or permissions do not prove that no association exists; GitScry cannot guarantee every PR containing a commit or every issue association, and the limited lookup may omit many issue mentions. Partial GraphQL data and local object/field errors preserve usable associations and continue other lookups; global authentication, rate-limit, network, or timeout failures stop link fetching but preserve Git materials and command success. No automatic retries. These links are navigation evidence, not proof of closure or causality, intent, or runtime call chains; in code mode, a match identifies changed lines, not a proven runtime call chain. Issue links cover explicit associations, not arbitrary mentions; the lookup does not fetch PR bodies or commits outside returned results. `--github-repo OWNER/REPO` alone does not contact GitHub; if omitted, a repository is inferred only from one unique local github.com remote. Without `--github-links`, search does not contact GitHub. JSON `schema_version` is 1 by default, 2 for `--patch` alone, and 4 whenever `--github-links` is enabled, except `why` reports use version 5. Version 4 may also include the optional `patch` field; inspect optional fields instead of inferring enabled options from the version.

Required input: choose one mode—one or more QUERY words, or `--code TEXT`.

Examples:

  gitscry search retry backoff --from-rev <base> --to-rev release

  gitscry search --code 'unwrap()?' --since 2025-01-01 --until 2025-01-31 --limit 5"#
    )]
    Search {
        /// Query words matched against commit subjects, bodies, and touched paths.
        #[arg(num_args = 1..)]
        query: Option<Vec<String>>,
        /// Combine lexical ranking with exact semantic retrieval.
        #[arg(long, requires = "query", conflicts_with = "code")]
        hybrid: bool,
        /// Case-sensitive literal substring matched on changed lines.
        #[arg(long, value_name = "TEXT", value_parser = parse_code_query)]
        code: Option<String>,
        /// Restrict code matches to added or removed lines.
        #[arg(long, value_enum, requires = "code", conflicts_with = "query")]
        change: Option<CodeChange>,
        /// Exact historical path; additions use the new path, removals the old path.
        #[arg(
            long = "path",
            value_name = "PATH",
            requires = "code",
            conflicts_with = "query"
        )]
        path: Option<String>,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
        /// Maximum number of matching commits or changed lines to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
        /// Output a stable structured JSON report instead of human-readable text.
        #[arg(long)]
        json: bool,
        /// Include bounded relevant cached text hunks; unavailable history is reported.
        #[arg(long, requires = "query", conflicts_with = "code")]
        patch: bool,
        /// Fetch coverage-first GitHub PR and issue associations for returned commits; requires an authenticated gh CLI.
        #[arg(long)]
        github_links: bool,
        /// GitHub repository to query; does not enable link fetching by itself.
        #[arg(long = "github-repo", value_name = "OWNER/REPO")]
        github_repo: Option<String>,
    },

    #[command(
        about = "Find historical examples of a similar change or migration",
        long_about = r#"Find historical examples of a similar change or migration.

Use `gitscry examples` before making a planned change when you want reusable precedents: commits that implemented something similar, with their change steps and touched paths.

Required input: one or more QUERY words describing the change. `--path` narrows the material to a repository-relative path the change touched and may be repeated.

Historical scope: `--from-rev REV` excludes that commit and its ancestors. `--to-rev REV` includes that commit and its ancestors, and defaults to the published cache tip. The lower revision must be an ancestor of the upper revision; revisions must exist in the published cache. `--since` and `--until` are inclusive bounds: use `YYYY-MM-DD` for a UTC calendar day or an RFC 3339 timestamp with `Z` or an explicit UTC offset for an instant. Timezone-free timestamps are rejected. Revision and time bounds combine. Scope filters commits before ranking and `--limit`; scoped output shows the resolved revisions, normalized time bounds, and effective cache tip in human and JSON output.

Examples:

  gitscry examples retry backoff --from-rev v1.2.0 --to-rev v1.3.0

  gitscry examples retire provider --since 2025-01-01 --until 2025-01-31 --path src/lib.rs"#
    )]
    #[command(after_long_help = MATERIAL_LINKS_LONG_HELP)]
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
        /// Output a stable structured JSON report instead of human-readable text.
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
        about = "Find abandoned or reverted approaches, their recorded reason, and safe retry conditions",
        long_about = r#"Find abandoned or reverted approaches, their recorded reason, and safe retry conditions.

Use `gitscry failures` when you are considering an approach and want to know whether it was tried and abandoned: it returns reverted or superseded commits together with the recorded reason and retry conditions from their commit bodies.

Required input: one or more QUERY words describing the approach. `--path` narrows the material to a repository-relative path the change touched and may be repeated.

Historical scope: `--from-rev REV` excludes that commit and its ancestors. `--to-rev REV` includes that commit and its ancestors, and defaults to the published cache tip. The lower revision must be an ancestor of the upper revision; revisions must exist in the published cache. `--since` and `--until` are inclusive bounds: use `YYYY-MM-DD` for a UTC calendar day or an RFC 3339 timestamp with `Z` or an explicit UTC offset for an instant. Timezone-free timestamps are rejected. Revision and time bounds combine. Scope filters commits before ranking and `--limit`; scoped output shows the resolved revisions, normalized time bounds, and effective cache tip in human and JSON output.

Examples:

  gitscry failures provider normalization --from-rev v1.2.0 --to-rev v1.3.0

  gitscry failures parser migration --since 2025-01-01T09:00:00-05:00"#
    )]
    #[command(after_long_help = MATERIAL_LINKS_LONG_HELP)]
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
        /// Output a stable structured JSON report instead of human-readable text.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
        #[command(flatten)]
        github: GithubLinkArgs,
    },

    #[command(
        about = "Find historical paths changed alongside one or more seed paths",
        long_about = "Find historical paths changed alongside one or more seed paths.\n\nUse `gitscry related` when you are changing one or more paths and want to know which other paths historically changed together with them, such as mirrored files or coupled modules.\n\nRequired input: one or more repository-relative seed PATHS.\n\nScope applies only to cached history. `--from-rev REV` excludes that commit and its ancestors; `--to-rev REV` includes that commit and its ancestors and defaults to the published cache tip. Revisions must exist in the published cache, and the lower revision must be an ancestor of the upper revision. `--since` and `--until` filter committer time: `YYYY-MM-DD` means an inclusive UTC calendar day, while RFC 3339 timestamps with `Z` or an explicit offset mean inclusive instants. Timezone-free timestamps are rejected. All bounds intersect. Scope filters co-change counts, scoring denominators, and supporting commits before ranking and `--limit`; scoped output reports resolved bounds and the effective cache tip. Without scope flags, all cached history is used.\n\nExamples:\n\n  gitscry related src/lib.rs --from-rev <base> --to-rev release\n\n  gitscry related src/cli.rs --since 2025-01-01 --until 2025-01-31"
    )]
    #[command(after_long_help = MATERIAL_LINKS_LONG_HELP)]
    Related {
        /// Repository-relative seed paths matched against history.
        #[arg(required = true, num_args = 1..)]
        paths: Vec<String>,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
        #[command(flatten)]
        github: GithubLinkArgs,
        /// Maximum number of matches to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
        /// Output a stable structured JSON report instead of human-readable text.
        #[arg(long)]
        json: bool,
    },

    #[command(
        about = "Find current test paths historically changed alongside one or more seed paths",
        long_about = "Find current test paths historically changed alongside one or more seed paths.\n\nUse `gitscry tests` when you changed code and want the test files that historically changed with it: it returns existing test paths ranked by co-change history.\n\nRequired input: one or more repository-relative seed PATHS.\n\nTest candidates remain current test paths in the working tree; scope narrows historical support, not the current test-path target set. `--from-rev REV` excludes that commit and its ancestors; `--to-rev REV` includes that commit and its ancestors and defaults to the published cache tip. Revisions must exist in the published cache, and the lower revision must be an ancestor of the upper revision. `--since` and `--until` filter committer time: `YYYY-MM-DD` means an inclusive UTC calendar day, while RFC 3339 timestamps with `Z` or an explicit offset mean inclusive instants. Timezone-free timestamps are rejected. All bounds intersect. Historical support and scoring are filtered before ranking and `--limit`; scoped output reports resolved bounds and the effective cache tip. Without scope flags, all cached history is used.\n\nExamples:\n\n  gitscry tests src/lib.rs --from-rev <base> --to-rev release\n\n  gitscry tests src/cli.rs --since 2025-01-01 --until 2025-01-31"
    )]
    #[command(after_long_help = MATERIAL_LINKS_LONG_HELP)]
    Tests {
        /// Repository-relative seed paths matched against history.
        #[arg(required = true, num_args = 1..)]
        paths: Vec<String>,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
        #[command(flatten)]
        github: GithubLinkArgs,
        /// Maximum number of matches to return.
        #[arg(long, default_value = "10", value_parser = parse_limit)]
        limit: usize,
        /// Output a stable structured JSON report instead of human-readable text.
        #[arg(long)]
        json: bool,
    },

    #[command(
        about = "Locate historical commits that may have introduced a regression",
        long_about = "Locate historical commits that may have introduced a regression.\n\nUse `gitscry regression` when a regression is observable and you want suspects: commits in the requested revision range whose material supports them as candidates that may have introduced the regression. Suspects are historical candidates; they do not replace an executable `git bisect`.\n\nRequired inputs: one or more SYMPTOM words and `--path`, the repository-relative path affected by the regression. `--symbol` narrows the suspect history to a symbol in that path. `--good` pins the last known good revision so suspects are limited to the good..bad range; `--bad` pins the last known bad revision and defaults to the published cache tip.\n\nHistory scope narrows that pinned suspect window: `--from-rev` excludes that revision and its ancestors; `--to-rev` is inclusive and defaults to `--bad`. `--since` and `--until` intersect with the suspect window using committer time. Date-only bounds cover inclusive UTC calendar days; timestamps require RFC 3339 with `Z` or an explicit UTC offset.\n\nExamples:\n\n  gitscry regression provider normalization --path src/lib.rs\n\n  gitscry regression slow startup --path src/main.rs --good v0.1.0 --symbol main\n\n  gitscry regression timeout --path src/app.py --good v0.2 --bad v0.3 --since 2024-01-01"
    )]
    #[command(after_long_help = MATERIAL_LINKS_LONG_HELP)]
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
        /// Last known bad revision; defaults to the published cache tip.
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
        /// Output a stable structured JSON report instead of human-readable text.
        #[arg(long)]
        json: bool,
    },

    #[command(group(
        ArgGroup::new("anchor")
            .required(true)
            .args(["line", "symbol"]),
    ))]
    #[command(
        about = "Show Git blame attribution and target changes for a line or symbol",
        long_about = "Show Git blame attribution and target-related modifications for a line or symbol.\n\nUse `gitscry why` when you need factual target attribution and modification history. The report separates target-line attribution (for a symbol, the starting line only), standalone target-related modifications supported by actual changed target lines (the full symbol range is tracked), and a count of other file history. This is not a root-cause explanation; commit messages and co-changed paths do not establish target relevance. Attribution and a modification from the same commit are shown once. `--limit` caps only standalone target-related modifications. Out-of-scope commits may be traversed to track line positions but are not returned as eligible modifications. Line tracking follows first-parent history across merges; changes reachable only from other parents are not classified.\n\nRequired inputs: a repository-relative PATH and exactly one anchor, `--line` (a one-based line number) or `--symbol` (a symbol name). `--at` pins the local revision containing the target and defaults to the published cache tip.\n\nHistorical scope: `--from-rev REV` excludes REV and its ancestors; `--to-rev REV` includes REV and its ancestors, intersected with the target revision selected by `--at`. Without `--to-rev`, the target revision is the upper bound. `--since` and `--until` filter by committer time and accept UTC dates or RFC 3339 timestamps with offsets. Scope flags combine, and revisions must be in the published cache.\n\nExamples:\n\n  gitscry why src/lib.rs --line 12\n\n  gitscry why src/lib.rs --symbol provider --at HEAD~1 --from-rev HEAD~5 --to-rev HEAD~1\n\n  gitscry why src/lib.rs --line 12 --since 2025-01-01 --until 2025-01-31"
    )]
    #[command(after_long_help = MATERIAL_LINKS_LONG_HELP)]
    Why {
        /// Repository-relative path at the target revision.
        path: String,
        /// One-based line number to explain.
        #[arg(long, conflicts_with = "symbol", value_parser = parse_line)]
        line: Option<usize>,
        /// Symbol name to explain.
        #[arg(long, conflicts_with = "line")]
        symbol: Option<String>,
        /// Local revision containing the target; defaults to the published cache tip.
        #[arg(long, value_name = "REV")]
        at: Option<String>,
        /// Exclude this cached revision and its ancestors from eligible history.
        #[arg(long = "from-rev", value_name = "REV")]
        from_rev: Option<String>,
        /// Include this cached revision and its ancestors, intersected with the target.
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
        /// Output a stable structured JSON report instead of human-readable text.
        #[arg(long)]
        json: bool,
    },

    #[command(
        about = "Trace a fix back to the introducing change and observed failure",
        long_about = "Trace a fix back to the introducing change and observed failure.\n\nUse `gitscry trace-fix` when a fix commit is known and you want what it fixed: the introducing change identified from the fix's parent diff and the observed failure recorded in the fix commit message.\n\nRequired input: FIX_REVISION is the fix target, a local revision containing the fix commit. `--path` narrows the material to a repository-relative path changed by the fix and may be repeated.\n\nHistory scope narrows introducing-change candidates without retargeting FIX_REVISION: `--from-rev` excludes that revision and its ancestors; `--to-rev` is inclusive and defaults to FIX_REVISION. `--since` and `--until` intersect with that commit range using committer time. Date-only bounds cover inclusive UTC calendar days; timestamps require RFC 3339 with `Z` or an explicit UTC offset.\n\nExamples:\n\n  gitscry trace-fix HEAD\n\n  gitscry trace-fix HEAD~1 --path src/lib.rs\n\n  gitscry trace-fix HEAD --from-rev HEAD~5 --since 2024-01-01"
    )]
    #[command(after_long_help = MATERIAL_LINKS_LONG_HELP)]
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
        /// Output a stable structured JSON report instead of human-readable text.
        #[arg(long)]
        json: bool,
        /// Include bounded cached hunks attributed to the introducing change.
        #[arg(long)]
        patch: bool,
    },
    #[command(
        after_long_help = MATERIAL_LINKS_LONG_HELP,
        about = "Show a file's complete evolution in the published cache",
        long_about = "Show a file's complete, chronological evolution within the published GitScry cache. This is a factual history, not a ranking of important changes. The default target is the cache's completed tip, not the working tree or an unindexed HEAD. The selected path must be a file at the target revision. All reachable cached commits are included, including merged-branch commits; entries are ordered by Git topological position from earliest to latest. Detected renames are followed; copies and older file incarnations after deletion/recreation are not. Merge entries compare against the first parent. Use the commit ID and historical path to inspect the underlying change. `--patch` adds bounded cached text hunks for each listed change; unavailable text is reported rather than guessed, and excerpts are not full diffs. `--json` returns structured entries and page metadata (schema v1 by default, v2 with `--patch`, v4 with `--github-links`). `--github-links` adds associations for commits on the returned page; it does not retrieve uncached history or commits outside the returned page. See the association details below.\n\nHistorical scope: `--from-rev REV` excludes REV and its ancestors; `--to-rev REV` includes REV and its ancestors, intersected with the target revision selected by `--at` (or the cache tip). `--since` and `--until` filter by committer time and accept UTC dates or RFC 3339 timestamps with offsets. Scope filters apply before pagination; rename traversal and the target file incarnation remain unchanged. Scope revisions must be in the published cache.\n\nRequired input: PATH, a repository-relative file path.\n\nOptions: `--at REV` selects a revision in the published cache; `--from-rev REV`, `--to-rev REV`, `--since DATE_OR_TIMESTAMP`, and `--until DATE_OR_TIMESTAMP` restrict history; `--limit N` sets the page size (default 10); `--offset N` selects a zero-based page offset; `--last` jumps to the final page and conflicts with `--offset`; `--patch` adds per-entry text excerpts; `--github-links` fetches associations for commits on the returned page; `--github-repo OWNER/REPO` selects a repository; `--json` returns structured output.\n\nExamples:\n\n  gitscry timeline src/lib.rs\n\n  gitscry timeline src/lib.rs --to-rev HEAD~5 --since 2025-01-01 --limit 5 --json"
    )]
    Timeline {
        /// Repository-relative file path at the selected revision.
        path: String,
        /// Cached revision containing the file; defaults to the cache's completed tip.
        #[arg(long, value_name = "REV")]
        at: Option<String>,
        /// Exclude this cached revision and its ancestors from eligible history.
        #[arg(long = "from-rev", value_name = "REV")]
        from_rev: Option<String>,
        /// Include this cached revision and its ancestors, intersected with the target.
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
        /// Output a stable structured JSON report instead of human-readable text.
        #[arg(long)]
        json: bool,
        /// Include bounded cached text hunks for each timeline entry.
        #[arg(long)]
        patch: bool,
        /// Fetch coverage-first PR and issue associations for returned-page commits; requires an authenticated gh CLI.
        #[arg(long)]
        github_links: bool,
        /// GitHub repository to query; does not enable link fetching by itself.
        #[arg(long = "github-repo", value_name = "OWNER/REPO")]
        github_repo: Option<String>,
    },
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
            | Self::Search { json, .. }
            | Self::Examples { json, .. }
            | Self::Failures { json, .. }
            | Self::Related { json, .. }
            | Self::Tests { json, .. }
            | Self::Regression { json, .. }
            | Self::Why { json, .. }
            | Self::TraceFix { json, .. }
            | Self::Timeline { json, .. } => *json,
            Self::Update | Self::Index { .. } => false,
        }
    }
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
    fn update_command_accepts_canonical_name_and_alias() {
        for name in ["update", "upgrade"] {
            let cli = Cli::try_parse_from(["gitscry", name]).unwrap();
            assert!(matches!(cli.command, Command::Update));
        }
    }
}
