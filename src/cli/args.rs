use clap::{ArgGroup, Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    bin_name = "gitscry",
    name = "gitscry",
    version,
    color = clap::ColorChoice::Never,
    before_help = ROOT_LONG_HELP,
    help_template = root_help_template(),
)]
pub(super) struct Cli {
    #[command(subcommand)]
    pub(super) command: Command,
}

// `gitscry --help` is the top-level overview; keep command `about`s concise and put details
// in each command's `long_about` for the `gitscry <command> --help` entry point.
const ROOT_LONG_HELP: &str = "\
Your Git history is a treasure trove. GitScry uncovers the implementation examples, failed approaches, code relationships, and regression context hidden inside.

Pick one command by intent, then run `gitscry <command> --help` for inputs, options, and examples.";

// Derive the command map from clap metadata so descriptions stay owned by each command.
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
    let mut template = String::from("{before-help}{usage-heading} {usage}\n");
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
    template.push_str("\nOptions:\n{options}{after-help}");
    template
}
const MATERIAL_LINKS_LONG_HELP: &str = "`--github-links` walks explicit commit→PR→issue associations for returned material through the authenticated `gh` CLI. It uses coverage-first pagination: all eligible commit PR homepages and discovered PR issue homepages precede continuation pages. Both connections share a 15-second timeout, 20 API requests, 50 results per page, and 200 deduplicated PR+issue objects. These fixed limits are conservative starting values, not empirically optimized. Human and JSON output mark each layer complete, partial, not queried, or failed; missing or unreturned associations do not prove none exist. Partial data and local object/field errors preserve usable links and continue other lookups; global authentication, rate-limit, network, or timeout failures stop link fetching only and preserve Git materials and command exit status. No automatic retries. Issue links are explicit `closingIssuesReferences`, not arbitrary mentions, and are navigation evidence, not proof of closure or causality. `--github-repo OWNER/REPO` selects a repository but does not itself contact GitHub; without it, repository inference follows `search` rules. Without `--github-links`, no GitHub lookup occurs. Links add navigation only, do not change Git material, and do not promise command-specific benefits. In particular, test co-change history does not prove assertions exist, regression suspects are not root-cause findings, and participant descriptions do not outweigh changes visible in Git.";
#[derive(Debug, Args, Default)]
pub(crate) struct HistoricalScopeArgs {
    /// Exclude this commit and its ancestors; it must be available after refresh or already published.
    #[arg(long = "from-rev", value_name = "REV")]
    pub(crate) from_rev: Option<String>,
    /// Include this commit and its ancestors; it must be available after refresh or already published. Defaults to the effective target.
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
    /// Retrieve cached historical leads for an ordinary two-side merge conflict.
    #[command(
        long_about = "Retrieve cached file-change leads for unmerged text and regular file-level conflicts in an ordinary two-endpoint merge with one merge base. Requires a compatible published cache; run `gitscry index` first. Before selecting material, synchronously refresh missing locally available history reachable from both pinned endpoints. Previously cached history is preserved; no fetch or unrelated-ref scan is performed. Repeat --path for exact repository-relative paths. Reports index stages and paths observed on each side. Detected Git rename lineage is used to navigate history; this does not assert semantic responsibility or cross-file migration. Binary and non-regular conflicts are reported as unsupported. No LLM analysis or generated resolution is used."
    )]
    Conflicts {
        #[arg(long = "path", value_name = "PATH")]
        paths: Vec<String>,
        /// Maximum historical leads per file and side.
        #[arg(long, default_value_t = 5, value_parser = parse_limit)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    #[command(
        about = "Inspect bounded later follow-up material",
        long_about = r#"Inspect bounded follow-up material after a seed commit.

The compatible published cache must already be initialized. Before selecting material, each query refreshes it with locally available history reachable from invocation-pinned current HEAD, regardless of --to-rev. The seed REV and explicit --to-rev must be available after refresh or already published; uncached revisions outside pinned HEAD still require `gitscry index`. The seed must be an ancestor of the endpoint. Without --to-rev, the endpoint is the latest cached commit reachable from pinned current HEAD; safe refresh fallback omits unavailable commits and reports incomplete coverage. Equal endpoints succeed with no material. Only strict seed descendants reachable from that endpoint are inspected, not parallel work or all refs. Queries never fetch, initialize, repair, or upgrade the cache, and do not change Git refs or working-tree files. Shallow and missing-history coverage is reported.

Repeat --path with exact repository-relative original changed paths, without glob or hunk selection. Either old or new side of a seed rename selects that change; omission selects all original changes. Unmatched selections are errors.

--days (default 90), --max-commits (default 2000), and --limit (default 20) require positive, representable integers. The inclusive committer-time ceiling is seed time plus --days; there is no lower timestamp bound. Timestamp-inverted descendants keep signed negative elapsed times and warnings. Deterministic forward topological inspection prioritizes early history; nonmatching eligible commits count against --max-commits. --limit separately bounds displayed matches. Out-of-window lineage inspection has an additional --max-commits budget; exhausted or unavailable correspondence is disclosed. Inspected extent and traversal/display truncation are separate.

An explicit revert reference requires a trimmed commit-message line of the form `This reverts commit <OID>` (optional final period; surrounding whitespace is ignored), where <OID> is a full object ID or an unambiguous cached hexadecimal prefix of at least 7 digits. A generic revert subject or shared path is insufficient. References are commit-level: `--path` filters seed paths but does not hide a reference when its commit touches no selected path. References use the same endpoint, inclusive time ceiling, and inspection budget as path associations. Explicit references precede region-overlap material, which precedes same-file-only material; each commit appears once in its strongest group and the display limit applies in that priority, with forward topological ordering within each group. A reference is a declaration only; it does not verify patch inversion or selected-path reversal, and no association establishes causality or stability. Changed regions start from seed-added lines. Complete cached text patches map line shifts and detected renames; region hits require tracked-line overlap and unrelated same-file changes remain same-file-only. Pure seed deletions create no tracked region, so nearby insertions are not overlap. A directly replaced tracked region continues only through additions in the same patch group. Missing or truncated patch material, tracking-budget exhaustion, unparseable hunks, or divergent parent coordinates downgrade region tracking to same-file-only and identify the reason. Deletion ends an incarnation; later recreation and copies are not continuations. Merge diffs use the first parent; imported work is not described as a fresh correction. Empty success means only no associations in the inspected scope.

No full diff by default. --patch includes bounded supporting cached hunks (64 scanned, 16 displayed, 8 KiB per hunk, 32 KiB per result) and discloses unavailable/truncated content. --json exposes equivalent material, effective scope and coverage.

Examples:
  gitscry followups <REV>
  gitscry followups <REV> --path src/a.rs --path src/b.rs --to-rev release --json
  gitscry followups <REV> --days 180 --max-commits 4000 --limit 40 --patch"#
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
        /// Return equivalent structured material.
        #[arg(long)]
        json: bool,
    },
    #[command(
        about = "Rank files by historical touches and textual churn",
        long_about = "Rank tracked files present at the selected target (current HEAD by default) by distinct non-merge touching commits. Default limit: 20. Reachable branch commits and root introductions count; merge-only conflict resolutions do not. Detected renames preserve identity, copies do not; deletion/recreation starts a new incarnation. Binary, permission and pure rename changes count. Textual churn sums cached added/deleted lines for those touches; JSON counts are null when no diff is usable and churn_complete is false when any eligible diff is unavailable. Text output marks partial counts with * and unavailable counts with —. No generated/vendor/lockfile or large-commit exclusions. Last changed is the maximum eligible committer time in UTC. `--from-rev` excludes its commit and ancestors; `--to-rev` includes the selected target and ancestors. `--since` and `--until` filter inclusive committer-time bounds; all bounds combine and never reset rename lineage. `--path-prefix DIR` selects target-present files below a literal repository-relative directory (not a glob; `src` matches `src/` descendants, not `src-old`). Each query refreshes the initialized compatible published cache with all locally available history reachable from invocation-pinned current HEAD before checking explicit --to-rev against the published cache or selecting material; filters do not narrow refresh. Without an explicit --to-rev, the target is pinned current HEAD; eligible history intersects the selected target history with the refreshed published cache. Safe fallback may leave uncached reachable commits, reported as incomplete coverage. Explicit --to-rev revisions must be present after refresh; revisions outside pinned HEAD must already be published. History and rename continuity are bounded by available cached objects. Queries never initialize, repair, upgrade, or fetch; they do not change Git refs or working-tree files.\n\nExamples:\n  gitscry hotspots\n  gitscry hotspots --json --limit 10"
    )]
    Hotspots {
        #[arg(long, default_value = "20", value_parser = parse_limit)]
        limit: usize,
        #[arg(long)]
        json: bool,
        #[arg(long, value_name = "DIR")]
        path_prefix: Option<String>,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
    },
    #[command(
        about = "Discover historical changes, abandonments, paths and tests",
        long_about = r#"Select traceable changed-code and path-association material for the current change.

Default input is the net HEAD-to-worktree path/status difference plus Git-unignored untracked paths. Staged/unstaged cancellations disappear. --staged selects HEAD-to-index only, excluding untracked paths and unstaged edits. Run inside a worktree; paths are root-relative. Unresolved conflicts fail. Detected rename sides are retained and excluded from suggestions; detection is not exhaustive. Results apply only to the selected baseline: default mode analyzes HEAD-to-worktree plus untracked paths, while --staged analyzes HEAD-to-index only; a result makes no claim about changes visible only in the other mode.

Changed-code material uses distinctive exact identities from individual current hunk sides and verifies them in historical added/removed lines, even with generic commit descriptions. Matches retain current/historical paths, locations and directions with short real excerpts. Opposite-direction matches are not a same-kind conclusion. Associated reverted changes appear once as recorded_abandonment, citing the underlying original, recorded revert and any relevant recorded corrective follow-up. Existing revert interpretation supplies reasons and recorded retry conditions; missing values remain unknown, not claims about current code or safe retry. Revert markers or complete provenance alone never qualify unrelated history. Original/revert matches merge current bases; each excerpt identifies its actual historical commit. Mandatory edits, coverage verdicts and required test execution are not supplied. No external symlink targets or recursive submodule contents are read; binary/non-UTF-8/oversized/unsafe content retains path/status with explicit omissions. Ordinary context retrieval does not use semantic results. After explicit cache initialization, shared query preparation refreshes missing local HEAD history before retrieval; it never implicitly initializes, repairs or upgrades the cache. Queries never download model or runtime resources. External LLMs are never used.

Content-origin historical follow-up checks: `--max-followup-checks CHECKS` sets an optional positive maximum shared by candidate-origin and baseline relationship checks; no maximum or timeout applies by default. One check is a distinct eligible, non-merge descendant commit in an origin's configured observation window (default seven days), within 20 parent edges. Stronger content origins run first (ties: newer origin, then OID); baseline origins use stable graph order. A completed candidate window reused for baseline statistics is counted once. On exhaustion, incomplete windows do not contribute support or baseline statistics; complete material may still qualify. Text and JSON report counts, budget, and limited coverage.

Historical queries use one published cache session and default to cached commits reachable from the HEAD pinned for this invocation. Nonempty input needs an explicitly initialized compatible published cache: run `gitscry index` once. Subsequent queries synchronously refresh missing locally available HEAD history before selecting material, independently of query filters and the follow-up relationship-check budget. Refresh preserves previously cached history and never fetches, initializes, repairs or upgrades a cache. Safe refresh failures and remaining incomplete coverage are reported. No-change input succeeds without opening cache.

Explicit --hybrid expands candidates using the existing local CPU encoder and compatible published semantic cache, then applies the same exact changed-code verification and identity-based selection. Refresh maintains previously enabled semantic coverage using compatible local resources; Context --hybrid requires the resulting published semantic coverage to be ready. Missing/incompatible resources leave ordinary history usable; --hybrid still fails unless semantic coverage is ready. Queries never download model or runtime resources. Enable semantic indexing with `gitscry index --semantic` or repair/reinstall the runtime. No-change input skips cache and semantic initialization even with --hybrid.

Semantic budgets: first 16 eligible local hunk sides, first eight distinctive signals per side, at most 64 independent query chunks. Eligible vectors are scanned once, retaining 64 commits by each commit's best chunk score; cosine scores never affect final ranking. Target verification reads at most 64 hunks per candidate and 16 KiB per hunk (64 MiB total), merging with ordinary matches before the unified output ceilings (at most 192 verified commits combined). Omitted semantic bases and target-content truncation are machine-readable; the bounded candidate count does not claim exhaustive retrieval.

--from-rev excludes that cached commit and its ancestors; --to-rev includes that cached commit and its ancestors; without it, history defaults to current HEAD's reachable commits intersected with the published cache. The lower revision must be an ancestor of the upper revision. --since and --until filter committer time using inclusive UTC dates (YYYY-MM-DD) or RFC 3339 instants with Z or an explicit offset; timezone-free timestamps fail. Bounds combine and never change the current-change baseline. Uncached reachable commits are omitted and incomplete coverage is reported.

Path suggestions exclude every selected current path; historical commit material can concern those paths. Test-shaped candidates must exist as safe regular files in this worktree and appear once as tests, merging co-change and test-path bases. Path association history excludes merge commits and commits changing over 50 paths; content matching independently uses cached hunks. Weak associations are not filled to meet a quota; zero results is valid. Historical matching uses exact path bytes, without basename or case-fold fallback. At most 256 byte-sorted input paths are queried; all input paths retain exclusion identity and omitted retrieval is reported. Path results include at most three historical citations with their full support count and citation truncation flag. Abandonment results cite one original, one recorded revert, and at most one relevant corrective follow-up, all within the selected historical scope.

Path-level directional follow-on is also enabled by default, including when changed code has no historical match. Each current path uses the shared related policy independently: a fixed seven-day/twenty-parent-edge window, proper descendants, complete observations, detected file incarnations, and conservative independent-chain, proportion and sampled-background qualification. Candidates appear once across co-change and both follow-on routes, retaining separate content-origin and per-source path-origin denominators, statistics and examples. Historical commit and abandonment items remain distinct commit material. `--no-historical-followup`, `--followup-days` and `--max-followup-checks` control only content-origin follow-up; they do not disable, shorten or budget path-level analysis. Path-level analysis has no default execution cutoff. Each source basis reports representative commit/path pairs, graph/time distances, omitted examples and incomplete observations.

Historical follow-up is enabled by default. It observes same-file descendant changes within `--followup-days` for each content origin (default 7 days); this window applies to both support and baseline comparisons, with at most 20 parent edges. `--followup-days` must be a positive integer whose seconds fit a signed timestamp window. `--no-historical-followup` skips only this analysis; changed-code, ordinary co-change and abandonment material remain active. A candidate needs at least two independent chains, support from at least half of complete origins, and at least twice the sampled baseline rate from up to 100 complete origins. These are observational associations, not causal claims, recommendations, defect/regression findings, quality verdicts or predictions. A zero sampled baseline is shown as no observed baseline, never numeric infinity. Empty or missing results mean no candidate qualified within the selected input, cached scope, observation window and analysis budgets.

Default total ceiling is eight, with at most three entries per category. Positive --limit changes only the total ceiling. Non-follow-up items order by distinct supported current paths; equal strengths put abandonments before historical changes, tests, then co-changing files. Historical-follow-up items rank by independent-chain count, then supporting-origin proportion, then baseline lift, with latest matched follow-up committer time and byte-sorted path as tie-breaks. A path returned by both follow-up and co-change appears once while retaining separate relationship bases, counts and citation sets; the merged item counts toward each category ceiling. Provenance completeness adds no relevance weight. Latest verified supporting committer time, byte-sorted path and canonical commit identity break ties for other categories. --json expresses the same material, input, scope, limitations and truncation as text.

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
        /// Return typed structured material instead of terminal text.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        scope: HistoricalScopeArgs,
    },
    #[command(
        about = "Summarize local GitScry command usage",
        long_about = r#"Show aggregate call counts and elapsed time for recognized GitScry commands. This report does not access Git history and works outside a repository.

After command-line parsing succeeds, GitScry records completed commands (including commands that later fail). It excludes help/version output, argument-parse failures, and this stats command. Timing starts after parsing and stops after command output is written. `upgrade` is recorded as `update`.

Only the local calendar date, canonical command name, call count, and cumulative elapsed nanoseconds are stored. Arguments, paths, repository identity, output, and file contents are never stored; usage data is never uploaded. Recording is best effort and cannot change another command's output or exit status. Aggregates are retained until you delete the database to reset history.

The database is user-scoped: `%LOCALAPPDATA%\GitScry\usage.sqlite` on Windows, `~/Library/Application Support/GitScry/usage.sqlite` on macOS, and `$XDG_DATA_HOME/gitscry/usage.sqlite` (or `~/.local/share/gitscry/usage.sqlite`) on Linux.

By default, include the last 30 local calendar days, including today. `--since` and `--until` accept inclusive local dates in YYYY-MM-DD format; an omitted lower bound means all earlier history, and an omitted upper bound means today. `--all` includes all recorded dates and conflicts with both date options. `--group` selects daily, Monday-starting weekly, or calendar-month buckets. Partial buckets contain only calls inside the selected date range.

`--json` returns schema version 1 with range, group, per-command totals, overall totals, and a chronological call trend.

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
        /// Output a stable structured JSON report instead of human-readable text.
        #[arg(long)]
        json: bool,
    },

    #[command(
        about = "Update GitScry to the latest stable release",
        long_about = "Update GitScry to the latest stable release.\n\nUse `gitscry update` to check GitHub for a newer release, verify its SHA-256 checksum, and replace this executable. The update is refused when the release is older than the running version.\n\nExamples:\n\n  gitscry update\n\n  gitscry upgrade",
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
        about = "Trace deleted code text or fragments in cached history",
        long_about = "Find case-sensitive literal substrings in deleted lines with --code, or exact whole-line deleted fragments from --code-file PATH (- reads stdin through EOF). Choose exactly one mode. --code is non-empty single-line text, not a resolved symbol. Additions and unchanged context are not searched.\n\nEvents group matching removed lines or complete fragment occurrences by removal commit and exact historical old path. Diffs compare with the first parent; root commits produce no deletion events. Events remain even if text survives elsewhere or later returns; no retirement, replacement or cross-file identity is inferred. Newest committer time comes first, with commit-ID/path ties and ordered matching positions. --limit defaults to 10 events, not lines.\n\n--path filters the exact historical old path without following aliases. --from-rev excludes that cached revision and its ancestors; --to-rev includes that cached revision and its ancestors. Without it, history defaults to current HEAD's reachable commits intersected with cached commits; explicit scope revisions must be cached and incomplete coverage is reported. The lower revision must be an ancestor of the upper. --since/--until are inclusive committer-time UTC dates or RFC 3339 instants with offsets. Filters apply before grouping and limits.\n\nRun `gitscry index` once to initialize a compatible published cache. Shared query preparation automatically refreshes missing locally available history reachable from the invocation's pinned HEAD before discovery, independently of filters. Discovery then uses only published history. No implicit initialization, repair, upgrade, fetch or all-ref scan. Covered history stays quiet; refresh and lock waits report progress on stderr without a default timeout. Safe fallback after refresh failure discloses the failure and incomplete coverage; unmet query prerequisites fail explicitly. Incomplete/shallow coverage remains visible. No matches succeeds with an empty report. Events include full commit messages, first-parent locators, detected file changes and all matching positions. Literal mode uses JSON schema version 1. Fragment mode uses schema version 2 (`trace-removal-fragments`), with each occurrence carrying commit_id, path, direction, start_line, end_line and complete content; bounded surrounding patch material remains event-level.\n\nExamples:\n  gitscry trace-removal --code 'legacy()'\n  gitscry trace-removal --code 'old.key = ' --path config.rs --to-rev v1.0 --json\n  gitscry trace-removal --code-file removed-block.txt --limit 5 --json"
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
        /// Output a stable versioned JSON report.
        #[arg(long)]
        json: bool,
    },
    #[command(
        about = "Build or refresh the shared repository history cache",
        long_about = "Build or refresh shared history reachable from the current HEAD.\n\nThe cache lives under `gitscry/` in the repository's Git common directory, so all linked worktrees share it. Git remains the source of truth; the cache is rebuildable. Each index run adds the history reachable from the current HEAD without collecting commits that were cached earlier. Shared ancestors are stored once, so indexed history remains available across branch switches, rewrites, and detached HEADs.\n\nUse `gitscry index` when you want to construct or refresh the cache explicitly.\n\nSemantic indexing is off by default; ordinary indexing without saved opt-in does not load or download a model. `--semantic` enables it and persists that choice; later `gitscry index` runs maintain semantic coverage, and subsequent queries make a local-only best-effort attempt to maintain it. `--no-semantic` disables it and deletes every repository semantic vector, including vectors outside current HEAD history. These options conflict. Enabling semantic indexing may download about 90 MB of pinned, hash-verified model resources shared across repositories. Subsequent indexing works offline when compatible resources and the verified ONNX Runtime 1.23.2 library are available. Missing or corrupt semantic resources or a missing/incompatible runtime return nonzero; the ordinary history cache remains usable.\n\nRequired input: none.\n\nExamples:\n\n  gitscry index\n\n  gitscry index --semantic\n\n  gitscry index --no-semantic"
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
        about = "Clear the repository-wide GitScry cache",
        long_about = r#"Clear all GitScry-owned data and settings from the repository-wide GitScry cache.

The cache lives under `gitscry/` in the repository's Git common directory, so all linked worktrees share it. This command acquires the shared cache's exclusive lock and waits for active readers or writers. It reports cached commits, changes, path records, hunks and semantic vectors, lists each affected data file with its measured file size, and reports measured before/after file sizes.

`--dry-run` inspects the same locked cache and lists planned deletions and projected file-size savings without deleting or compacting cache data. Clear preserves `gitscry/cache.lock`, `gitscry/.gitignore`, Git history, user-scoped usage statistics and globally shared model/runtime resources. Reported file sizes may differ from physical filesystem allocation. Clearing cannot be undone; rebuild repository cache data with `gitscry index`.

Examples:
  gitscry clear
  gitscry clear --dry-run"#
    )]
    Clear {
        /// Preview planned deletions without changing cache data.
        #[arg(long)]
        dry_run: bool,
    },

    #[command(
        about = "Prune material for Git objects that no longer exist",
        long_about = r#"Prune repository-wide GitScry cache material only for commits whose Git objects are no longer available locally. If Git still retains an object, prune preserves its history even when its branch was deleted, rebased, or is unreachable from current HEAD. Git remains the source of truth; prune does not run Git GC or modify Git objects, refs, or retention policy.

Use `--dry-run` to inspect candidate counts and current cache usage under the shared repository lock. A preview does not change cache data and does not estimate reclaimed bytes because that requires compaction. Execution reports deleted commits, changes, path records, hunks and semantic vectors, measured before/after cache data-file sizes, and bytes actually released. File sizes may differ from physical filesystem allocation. Execution rechecks Git object availability; a preview does not reserve a deletion plan.

Examples:
  gitscry prune
  gitscry prune --dry-run"#
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
        about = "Search history or changed-code lines",
        long_about = r#"Search the published cache's history reachable from the current HEAD for relevant commits or changed-code lines.

Use `gitscry search QUERY...` to search commit subjects, bodies, and touched paths. Use `--code TEXT` to find a case-sensitive literal substring in added or removed lines from cached diffs. Code queries are non-empty, single-line text; punctuation and spaces are matched literally. Unchanged context lines are not searched.
Use `--code-regex PATTERN` for a Rust `regex`-syntax expression matched independently against each added or removed line. The default is Unicode-aware and case-sensitive; inline flags such as `(?i)` and explicit byte-mode groups such as `(?-u:...)` are supported. Anchors address one changed line (`^$` finds a changed empty line); no expression crosses lines. Look-around and backreferences are unsupported. Patterns must be non-empty, single-line, and at most 16,384 UTF-8 bytes; compilation is limited to 10 MiB with a nesting limit of 250 and a 2 MiB DFA cache budget. Invalid, unsupported, and over-limit patterns are rejected. Each matching line appears once, and output uses the same report fields as `--code`. Matching ignores LF and the CR immediately before LF in CRLF lines without changing displayed content. `--limit` bounds returned lines, not scan work; use exact paths or historical scope to reduce work. `--hybrid` and `--patch` are text-search-only.

Use `--hybrid` with text queries to merge lexical ranking with exact semantic cosine retrieval. Before retrieval, shared query preparation makes a local-only best-effort update if semantic indexing is already enabled; failures leave ordinary search available, but hybrid still requires ready coverage. Queries never download model or runtime resources. Run `gitscry index --semantic` to enable semantic indexing or repair coverage while online. The short-query limit is 256 total tokens, including special tokens `[CLS]` and `[SEP]`; longer queries are tokenized once into chunks of 220 content tokens with 40 tokens of overlap (180-token stride). Chunks are embedded in batches of up to 8. Each commit receives its maximum cosine similarity across chunks for one semantic ranking, fused once with the lexical ranking; the complete original query still goes to lexical search. Queries requiring more than 32 chunks are rejected before inference or semantic retrieval; shorten the query or rerun ordinary lexical search without `--hybrid`. Chunking does not guarantee that procedural context is retained or that instructions are followed. Long-query quality is not guaranteed to match concise queries. Hybrid search scans eligible vectors once and keeps `max(100, --limit)` candidates per branch. In JSON, `matched_count` is the deduplicated union of those bounded branch lists, not a corpus-wide match count; a notice reports the candidate depth.

`--code-file PATH` reads an exact whole-line fragment; `-` reads stdin through EOF. LF and CRLF are equivalent for matching; all other bytes remain literal. Matches are entirely added or removed in one hunk; context breaks continuity, opposite-direction rows do not. Overlaps are distinct occurrences. Full matched content and bounded surrounding patch material are included by default. Fragment JSON uses schema version 6 (`code-fragment-search`), including with GitHub links; existing modes retain their versions.
In all code modes, `--change added|removed` selects one direction and `--path PATH` matches an exact historical path: additions use the new path and removals use the old path. Rename history is not followed. No file-type filter is applied.

Scope applies to all search modes and is limited to the published cache. `--from-rev REV` excludes that commit and its ancestors; `--to-rev REV` includes that commit and its ancestors. The lower revision must be an ancestor of the upper revision. Explicit scope revisions must exist in the published cache. If `--to-rev` is omitted, the effective upper revision is HEAD pinned at invocation start. Before scope resolution, missing locally available HEAD history is synchronously added to an explicitly initialized cache, independently of query filters.

`--since` and `--until` filter committer time. Use `YYYY-MM-DD` for an inclusive UTC calendar day, or an RFC 3339 timestamp with `Z` or an explicit UTC offset for an inclusive instant. Timezone-free timestamps are rejected. Bounds combine with revision scope, and filtering happens before ranking and `--limit`. Scoped results show the normalized bounds, resolved revisions, and published cache tip and coverage status in human and JSON output.

Run `gitscry index` once to initialize the cache explicitly. Search never initializes, repairs, upgrades, fetches, or scans all refs. Refresh retains previously cached branches; covered HEAD history stays quiet. Actual refresh and lock waiting progress go to stderr, without a default timeout. Shallow or missing local history remains disclosed; safe fallback after refresh failure reports failure and incomplete coverage, otherwise the query fails. Semantic opt-in is unchanged; enabled coverage may be refreshed locally, while hybrid prerequisites remain mandatory. `--limit` limits matching commits in ordinary mode and matching lines in literal/regex code modes, and fragment occurrences in file mode. `--json` returns structured output with warnings.

`--github-links` fetches explicit `Commit.associatedPullRequests` and `PullRequest.closingIssuesReferences` for distinct complete commit IDs represented in returned text or code results. It reads PR number, title, URL, repository identity, plus issue number, title, URL, and repository identity, using the `gh` CLI's existing GitHub login; GitScry does not audit or claim a minimum permission set. Coverage comes first: request every returned commit's PR homepage, then every discovered PR's issue homepage, then continuation pages; PRs discovered later still get an issue homepage before further pagination. Both connections use opaque cursors within a shared 15-second timeout, 20-request, 50-results-per-page, 200-deduplicated-PR+issue-object budget. These fixed limits are conservative starting values, not empirically optimized. Each commit's PR layer and each PR's issue layer reports complete, partial, not queried, or failed, with a stop reason where relevant. Missing data or permissions do not prove that no association exists; GitScry cannot guarantee every PR containing a commit or every issue association, and the limited lookup may omit many issue mentions. Partial GraphQL data and local object/field errors preserve usable associations and continue other lookups; global authentication, rate-limit, network, or timeout failures stop link fetching but preserve Git materials and command success. No automatic retries. These links are navigation evidence, not proof of closure or causality, intent, or runtime call chains; in code mode, a match identifies changed lines, not a proven runtime call chain. Issue links cover explicit associations, not arbitrary mentions; the lookup does not fetch PR bodies or commits outside returned results. `--github-repo OWNER/REPO` alone does not contact GitHub; if omitted, a repository is inferred only from one unique local github.com remote. Without `--github-links`, search does not contact GitHub. JSON `schema_version` is 1 by default, 2 for `--patch` alone, and 4 whenever `--github-links` is enabled, except `why` reports use version 5. Version 4 may also include the optional `patch` field; inspect optional fields instead of inferring enabled options from the version.

Required input: choose exactly one mode—one or more QUERY words, `--code TEXT`, `--code-regex PATTERN`, or `--code-file PATH`.

Examples:

  gitscry search retry backoff --from-rev <base> --to-rev release

  gitscry search --code 'unwrap()?' --since 2025-01-01 --until 2025-01-31 --limit 5

  gitscry search --code-regex 'Old[A-Z][A-Za-z0-9_]*' --change removed --path src/lib.rs

  gitscry search --code-regex '^$' --limit 5"#
    )]
    Search {
        /// Query words matched against commit subjects, bodies, and touched paths.
        #[arg(num_args = 1..)]
        query: Option<Vec<String>>,
        /// Combine lexical ranking with exact semantic retrieval.
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
        /// Output a stable structured JSON report instead of human-readable text.
        #[arg(long)]
        json: bool,
        /// Include bounded relevant cached text hunks; unavailable history is reported.
        #[arg(long, requires = "query", conflicts_with_all = ["code", "code_regex"])]
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

Historical scope: `--from-rev REV` excludes that commit and its ancestors. `--to-rev REV` includes that commit and its ancestors, and defaults to current HEAD's reachable history, intersected with cached commits. The lower revision must be an ancestor of the upper revision; explicit scope revisions must exist in the published cache. `--since` and `--until` are inclusive bounds: use `YYYY-MM-DD` for a UTC calendar day or an RFC 3339 timestamp with `Z` or an explicit UTC offset for an instant. Timezone-free timestamps are rejected. Revision and time bounds combine. Scope filters commits before ranking and `--limit`; scoped output shows the resolved revisions, normalized time bounds, and effective cache tip in human and JSON output.

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

Historical scope: `--from-rev REV` excludes that commit and its ancestors. `--to-rev REV` includes that commit and its ancestors, and defaults to current HEAD's reachable history, intersected with cached commits. The lower revision must be an ancestor of the upper revision; explicit scope revisions must exist in the published cache. `--since` and `--until` are inclusive bounds: use `YYYY-MM-DD` for a UTC calendar day or an RFC 3339 timestamp with `Z` or an explicit UTC offset for an instant. Timezone-free timestamps are rejected. Revision and time bounds combine. Scope filters commits before ranking and `--limit`; scoped output shows the resolved revisions, normalized time bounds, and effective cache tip in human and JSON output.

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
        long_about = "Find historical paths changed alongside one or more seed paths.\n\nUse `gitscry related` when you are changing one or more paths and want to know which other paths historically changed together with them, such as mirrored files or coupled modules.\n\nRequired input: one or more repository-relative seed PATHS.\n\nDefault mode ranks individual paths using commits touching any seed. A query containing one or more directories returns external paths with co-change support, one module-touch denominator per eligible commit touching any source, contributing internal paths, per-input attribution and commit citations. Existing directories are recognized automatically; a trailing `/` explicitly requests a directory, including a deleted historical directory. Matching is recursive with path-component boundaries and uses paths at each historical commit. Other inputs retain file interpretation; mixed directory/file inputs are supported, identical normalized inputs collapse, and candidates exclude every historical path matched by any input. Output discloses source kinds, per-input matched paths and historical match status. Directory queries currently cover co-change only, not follow-on relations. `--patterns` instead requires every seed in each supporting commit and returns closed file-incarnation combinations with at least three distinct members and at least one non-seed. Paths are case-sensitive; duplicate seeds and aliases to one incarnation collapse, and directories do not expand. `--min-support N` sets the minimum distinct-commit support (default 3, minimum 2, pattern mode only). Equal-support subsets are omitted in favor of their closed supersets; incidental extra paths do not prevent support. `--limit` caps combinations, not members or mining work.\n\nPattern history excludes merges and commits touching more than 50 distinct paths; exclusion counts are scoped. Seed-only and two-path commits still count in the eligible all-seed denominator. Each group reports exact support and its proportion of eligible all-seed commits, up to five supporting commit citations, and omitted reference counts. Members are file incarnations marked as seeds or non-seeds, with their introduction commit and current target paths (or missing status), even when the historical scope ends earlier. Detected Git rename records connect paths into one incarnation; deletion ends an incarnation, while recreation and copies start new ones; undetected renames are not inferred. Each citation lists the actual changed path for every member. Empty results never fall back to individual paths. This is historical material, not a required-edit checklist. Pattern mode is local-only and cannot be combined with `--github-links` or `--github-repo`. `--json` provides a structured grouped report.\n\nScope applies only to cached history. `--from-rev REV` excludes that commit and its ancestors; `--to-rev REV` includes that commit and its ancestors and defaults to current HEAD's reachable history, intersected with cached commits. Explicit scope revisions must exist in the published cache, and the lower revision must be an ancestor of the upper revision. `--since` and `--until` filter committer time: `YYYY-MM-DD` means an inclusive UTC calendar day, while RFC 3339 timestamps with `Z` or an explicit offset mean inclusive instants. Timezone-free timestamps are rejected. All bounds intersect. Scope filters co-change counts, scoring denominators, and supporting commits before ranking and `--limit`; scoped output reports resolved bounds and the published cache tip and coverage status. Without scope flags, current HEAD's reachable history intersected with cached commits is used; incomplete coverage is warned and can be improved with `gitscry index`.\n\nLine mode: `--line N` selects actual changed-line history for exactly one exact repository-relative file; directories, multiple seeds, and `--patterns` are rejected. `--at REV` resolves the file and line at that revision (default HEAD), independently of historical scope. Context-only hunks and edits elsewhere in an enclosing function do not count. Confirmed introductions and target-only commits count in the eligible target-touch denominator. First-parent tracing stops at uncertain history boundaries; partial or unavailable history is disclosed rather than broadened to whole-file history. Merge replay and changes to more than 50 paths are excluded. Independent file-level follow-on material is omitted. Symbol mode: `--symbol NAME` resolves one uniquely named symbol in the selected file at `--at` (default HEAD), independently of historical scope. Existing conservative symbol localization and first-parent continuity tracing determine the full range; support is not universal across programming languages. Actual modifications anywhere in that range and reliably confirmed introductions count as target touches. Unrelated symbols and context-only hunks do not contribute. Uncertain introductions or continuity yield partial or unavailable history, never whole-file fallback; known path-move, merge, shallow-history, and missing-object limitations are disclosed. Historical scope filters symbol target-touch commits before relation aggregation, with the same target-only denominator and mass-change exclusions. Independent file-level follow-on material is omitted. These associations do not establish causation or recommend edits.\n\nExamples:\n\n  gitscry related src/lib.rs --from-rev <base> --to-rev release\n\n  gitscry related src/cli.rs --since 2025-01-01 --until 2025-01-31"
    )]
    #[command(
        group(
            ArgGroup::new("related_anchor")
                .args(["line", "symbol"])
                .multiple(false),
        ),
        after_long_help = MATERIAL_LINKS_LONG_HELP
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
        /// Discover closed combinations supported by commits containing all seeds.
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
        /// Output a stable structured JSON report instead of human-readable text.
        #[arg(long)]
        json: bool,
    },

    #[command(
        about = "Find current test paths historically changed alongside one or more seed paths",
        long_about = "Find current test paths historically changed alongside one or more seed paths.\n\nUse `gitscry tests` when you changed code and want the test files that historically changed with it: it returns existing test paths ranked by co-change history.\n\nRequired input: one or more repository-relative seed PATHS.\n\nTest candidates remain current test paths in the working tree; scope narrows historical support, not the current test-path target set. `--from-rev REV` excludes that commit and its ancestors; `--to-rev REV` includes that commit and its ancestors and defaults to current HEAD's reachable history, intersected with cached commits. Explicit scope revisions must exist in the published cache, and the lower revision must be an ancestor of the upper revision. `--since` and `--until` filter committer time: `YYYY-MM-DD` means an inclusive UTC calendar day, while RFC 3339 timestamps with `Z` or an explicit offset mean inclusive instants. Timezone-free timestamps are rejected. All bounds intersect. Historical support and scoring are filtered before ranking and `--limit`; scoped output reports resolved bounds and the published cache tip and coverage status. Without scope flags, current HEAD's reachable history intersected with cached commits is used; incomplete coverage is warned and can be improved with `gitscry index`.\n\nExamples:\n\n  gitscry tests src/lib.rs --from-rev <base> --to-rev release\n\n  gitscry tests src/cli.rs --since 2025-01-01 --until 2025-01-31"
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
        long_about = "Locate historical commits that may have introduced a regression.\n\nUse `gitscry regression` when a regression is observable and you want suspects: commits in the requested revision range whose material supports them as candidates that may have introduced the regression. Suspects are historical candidates; they do not replace an executable `git bisect`.\n\nRequired inputs: one or more SYMPTOM words and `--path`, the repository-relative path affected by the regression. `--symbol` narrows the suspect history to a symbol in that path. `--good` pins the last known good revision so suspects are limited to the good..bad range; `--bad` pins the last known bad revision and defaults to current HEAD. Before checking explicit targets against the published cache or selecting historical material, the query refreshes the initialized compatible cache with all locally available history reachable from invocation-pinned current HEAD, independent of --bad, --good, and scope. Explicit revisions are accepted if present after refresh or already published; uncached revisions outside pinned HEAD still require `gitscry index`. Safe refresh fallback reports incomplete coverage.\n\nHistory scope narrows that pinned suspect window: `--from-rev` excludes that revision and its ancestors; `--to-rev` is inclusive and defaults to `--bad`. `--since` and `--until` intersect with the suspect window using committer time. Date-only bounds cover inclusive UTC calendar days; timestamps require RFC 3339 with `Z` or an explicit UTC offset.\n\nExamples:\n\n  gitscry regression provider normalization --path src/lib.rs\n\n  gitscry regression slow startup --path src/main.rs --good v0.1.0 --symbol main\n\n  gitscry regression timeout --path src/app.py --good v0.2 --bad v0.3 --since 2024-01-01"
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
        long_about = "Show Git blame attribution and target-related modifications for a line or symbol.\n\nUse `gitscry why` when you need factual target attribution and modification history. The report separates target-line attribution (for a symbol, the starting line only), standalone target-related modifications supported by actual changed target lines (the full symbol range is tracked), and a count of other file history. This is not a root-cause explanation; commit messages and co-changed paths do not establish target relevance. Attribution and a modification from the same commit are shown once. `--limit` caps only standalone target-related modifications. Out-of-scope commits may be traversed to track line positions but are not returned as eligible modifications. Line tracking follows first-parent history across merges; changes reachable only from other parents are not classified.\n\nRequired inputs: a repository-relative PATH and exactly one anchor, `--line` (a one-based line number) or `--symbol` (a symbol name). `--at` selects the local revision containing the target and defaults to current HEAD. Before checking explicit targets against the published cache or selecting historical material, the query refreshes the initialized compatible cache with all locally available history reachable from invocation-pinned current HEAD, independent of --at and scope. Explicit --at targets are accepted if present after refresh or already published; uncached targets outside pinned HEAD still require `gitscry index`. Safe refresh fallback reports incomplete coverage.\n\nHistorical scope: `--from-rev REV` excludes REV and its ancestors; `--to-rev REV` includes REV and its ancestors, intersected with the target revision selected by `--at`, and must be present in the cache after refresh. Without `--to-rev`, the upper bound is current HEAD unless `--at` is set; only cached commits reachable from that target are eligible. `--since` and `--until` filter by committer time and accept UTC dates or RFC 3339 timestamps with offsets. Scope flags combine.\n\nExamples:\n\n  gitscry why src/lib.rs --line 12\n\n  gitscry why src/lib.rs --symbol provider --at HEAD~1 --from-rev HEAD~5 --to-rev HEAD~1\n\n  gitscry why src/lib.rs --line 12 --since 2025-01-01 --until 2025-01-31"
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
        /// Output a stable structured JSON report instead of human-readable text.
        #[arg(long)]
        json: bool,
    },

    #[command(
        about = "Trace a fix back to the introducing change and observed failure",
        long_about = "Trace a fix back to the introducing change and observed failure.\n\nUse `gitscry trace-fix` when a fix commit is known and you want what it fixed: the introducing change identified from the fix's parent diff and the observed failure recorded in the fix commit message.\n\nRequired input: FIX_REVISION is the fix target, a local revision containing the fix commit. `--path` narrows the material to a repository-relative path changed by the fix and may be repeated. Each query refreshes the initialized compatible cache with locally available history reachable from invocation-pinned current HEAD before checking FIX_REVISION against the published cache or resolving scope; filters never narrow refresh. Revisions reachable from that HEAD are available after refresh, while uncached revisions outside it must already be published. Safe refresh fallback is disclosed. Queries never fetch, initialize, repair, or upgrade the cache.\n\nHistory scope narrows introducing-change candidates without retargeting FIX_REVISION: `--from-rev` excludes that revision and its ancestors; `--to-rev` is inclusive and defaults to FIX_REVISION. `--since` and `--until` intersect with that commit range using committer time. Date-only bounds cover inclusive UTC calendar days; timestamps require RFC 3339 with `Z` or an explicit UTC offset.\n\nExamples:\n\n  gitscry trace-fix HEAD\n\n  gitscry trace-fix HEAD~1 --path src/lib.rs\n\n  gitscry trace-fix HEAD --from-rev HEAD~5 --since 2024-01-01"
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
        about = "Show cached history for a file at current HEAD",
        long_about = "Show a file's complete, chronological evolution within the published GitScry cache. This is a factual history, not a ranking of important changes. Each query refreshes the initialized compatible published cache with all locally available history reachable from invocation-pinned current HEAD before checking explicit targets against the published cache or applying scope; filters and explicit targets never narrow that refresh. The default target is pinned HEAD. Explicit `--at` revisions are accepted if present after refresh or already published; uncached revisions outside HEAD still require `gitscry index`. Safe refresh fallback may omit local commits, which are reported as incomplete coverage. Queries never fetch, initialize, repair, or upgrade the cache. The selected path must be a file at the target revision. All reachable cached commits are included, including merged-branch commits; entries are ordered by Git topological position from earliest to latest. Detected renames are followed; copies and older file incarnations after deletion/recreation are not. Merge entries compare against the first parent. Use the commit ID and historical path to inspect the underlying change. `--patch` adds bounded cached text hunks for each listed change; unavailable text is reported rather than guessed, and excerpts are not full diffs. `--json` returns structured entries and page metadata (schema v1 by default, v2 with `--patch`, v4 with `--github-links`). `--github-links` adds associations for commits on the returned page; it does not retrieve uncached history or commits outside the returned page. See the association details below.\n\nHistorical scope: `--from-rev REV` excludes REV and its ancestors; `--to-rev REV` includes REV and its ancestors, intersected with the target revision selected by `--at` (or current HEAD). `--since` and `--until` filter by committer time and accept UTC dates or RFC 3339 timestamps with offsets. Scope filters apply before pagination; rename traversal and the target file incarnation remain unchanged. Scope revisions must be available after refresh or already published; uncached revisions outside pinned HEAD still require `gitscry index`.\n\nRequired input: PATH, a repository-relative file path.\n\nOptions: `--at REV` selects a revision in the published cache; `--from-rev REV`, `--to-rev REV`, `--since DATE_OR_TIMESTAMP`, and `--until DATE_OR_TIMESTAMP` restrict history; `--limit N` sets the page size (default 10); `--offset N` selects a zero-based page offset; `--last` jumps to the final page and conflicts with `--offset`; `--patch` adds per-entry text excerpts; `--github-links` fetches associations for commits on the returned page; `--github-repo OWNER/REPO` selects a repository; `--json` returns structured output.\n\nExamples:\n\n  gitscry timeline src/lib.rs\n\n  gitscry timeline src/lib.rs --to-rev HEAD~5 --since 2025-01-01 --limit 5 --json"
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
